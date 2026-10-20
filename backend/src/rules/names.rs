//! `container_name` and `shared_fixed_name`: names fixed in the Compose file, which every copy of
//! the stack would share.

use std::collections::HashSet;

use crate::compose::{EntryFact, Project, ResourceFact, ResourceKind};
use crate::edits::remove_entries;
use crate::model::{Affects, Finding, RuleId, SourceRef};

/// One finding per `container_name` as written: one an anchor or `extends` gives several services
/// is one finding, named after the service that writes it. A name holding a variable nothing
/// provides isn't fixed, so it is no finding. The edit removes the entry's line, unless the entry
/// is reached through an anchor (see [`owns`]).
pub fn container_name(project: &Project) -> Vec<Finding> {
    let names = &project.facts.container_names;
    let mut written = HashSet::new();
    let mut findings = Vec::new();
    for fact in names {
        if !written.insert((fact.file, fact.key.start, fact.key.end)) || unprovided(project, fact) {
            continue;
        }
        let Some(source) = project.sources.get(fact.file) else {
            continue;
        };
        let readers: Vec<&str> = names
            .iter()
            .filter(|other| other.file == fact.file && other.key == fact.key)
            .filter_map(|other| other.service.as_deref())
            .collect();
        let Some(service) = super::writer(project, source, fact.key.start, &readers) else {
            continue;
        };
        let owner = project
            .model
            .services
            .iter()
            .find(|node| node.name == service)
            .map(|node| &node.source);
        let edit = owns(project, owner, &[fact])
            .then(|| remove_entries(source, &[fact], "Remove container_name".to_owned()))
            .flatten();
        findings.push(Finding {
            rule: RuleId::ContainerName,
            service: Some(service.to_owned()),
            message: format!(
                "{service} sets container_name: {}. Compose won't start a second container with that name, so only one copy of the stack can run: an agent's copy fails while yours runs, and two copies started by hand collide. Removing it lets Compose name the container per project; other services still reach it as {service}.",
                fact.text
            ),
            affects: vec![Affects::Agents, Affects::ByHand],
            lead: false,
            source: source.source_ref(fact.value.start),
            span: Some(source.span(fact.value.clone())),
            edit,
        });
    }
    findings
}

/// One finding per top-level volume or network with a fixed `name:` or `external:`, at whichever
/// of the two is written first. A `name:` holding a variable nothing provides isn't fixed. The
/// edit removes the fixed entries, when they are in one file, each stands alone on its line, and
/// none is reached through an anchor (see [`owns`]).
pub fn shared_fixed_name(project: &Project) -> Vec<Finding> {
    project
        .facts
        .resources
        .iter()
        .filter_map(|resource| shared(project, resource))
        .collect()
}

fn shared(project: &Project, resource: &ResourceFact) -> Option<Finding> {
    let name = resource
        .name_entry
        .as_ref()
        .filter(|entry| !unprovided(project, entry));
    let external = resource.external_entry.as_ref();
    let mut fixed: Vec<&EntryFact> = name.into_iter().chain(external).collect();
    // Files come in reading order, so the first fixed entry is in the file read first.
    fixed.sort_by_key(|entry| (entry.file, entry.key.start));
    let first = *fixed.first()?;
    let source = project.sources.get(first.file)?;
    let (noun, nodes) = match resource.kind {
        ResourceKind::Volume => ("volume", &project.model.volumes),
        ResourceKind::Network => ("network", &project.model.networks),
    };
    let n = &resource.name;
    let node = nodes.iter().find(|node| node.name == *n);
    let edit = owns(project, node.map(|node| &node.source), &fixed)
        .then(|| remove_entries(source, &fixed, format!("Give each copy its own {noun}")))
        .flatten();
    let message = if external.is_some() {
        // The edit removes a name: entry only when it is a fixed mark; so does the wording, edit
        // or not.
        let and_name = if name.is_some() {
            " (and its name:)"
        } else {
            ""
        };
        match resource.kind {
            ResourceKind::Volume => format!(
                "Volume {n} is external, so every copy of the stack uses the same existing volume and shares its data. Removing external:{and_name} lets each copy create its own volume, which starts empty."
            ),
            ResourceKind::Network => format!(
                "Network {n} is external, so every copy of the stack joins the same existing network. Removing external:{and_name} lets each copy create its own network."
            ),
        }
    } else {
        // The model's name, as the files merge it; the entry's own text should it be missing.
        let x = node
            .and_then(|node| node.fixed_name.clone())
            .unwrap_or_else(|| first.text.clone());
        match resource.kind {
            ResourceKind::Volume => format!(
                "Volume {n} has the fixed name {x}, so every copy of the stack uses the same volume and shares its data. Removing name: gives each copy its own volume, named after its project; the data in {x} stays where it is."
            ),
            ResourceKind::Network => format!(
                "Network {n} has the fixed name {x}, so every copy of the stack joins the same network. Removing name: gives each copy its own network, named after its project."
            ),
        }
    };
    Some(Finding {
        rule: RuleId::SharedFixedName,
        service: None,
        message,
        affects: vec![Affects::Agents, Affects::ByHand],
        lead: false,
        source: source.source_ref(first.value.start),
        span: Some(source.span(first.value.clone())),
        edit,
    })
}

/// Whether every one of `entries` is the owner's own, the owner's key being at `owner`: in another
/// file, or after the owner's key line. An entry in the owner's file at or before that line is
/// reached through `<<:` or an alias, since an anchor always comes before its aliases: removing it
/// would empty the anchor for everything merging it. Not without an owner.
fn owns(project: &Project, owner: Option<&SourceRef>, entries: &[&EntryFact]) -> bool {
    let Some(owner) = owner else {
        return false;
    };
    entries.iter().all(|entry| {
        project.sources.get(entry.file).is_some_and(|source| {
            source.path != owner.file
                || usize::try_from(owner.line)
                    .is_ok_and(|line| source.line_of(entry.key.start) > line)
        })
    })
}

/// Whether `entry`'s value holds a variable nothing provides: a use of one in the entry's file,
/// inside the value's bytes.
pub(super) fn unprovided(project: &Project, entry: &EntryFact) -> bool {
    project.facts.variables.iter().any(|variable| {
        variable.file == entry.file
            && variable.bytes.start >= entry.value.start
            && variable.bytes.end <= entry.value.end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{load_project, one};
    use crate::edits::apply;
    use crate::files::MemFiles;
    use crate::model::{Affects, RuleId, SourceRef, Span};
    use crate::paths::Checkout;

    /// The text of a span, for ASCII texts where UTF-16 offsets are byte offsets.
    fn spanned<'t>(text: &'t str, span: &Span) -> &'t str {
        &text[span.from as usize..span.to as usize]
    }

    fn messages(findings: &[Finding]) -> Vec<&str> {
        findings
            .iter()
            .map(|finding| finding.message.as_str())
            .collect()
    }

    #[test]
    fn container_name_is_removed_with_its_line() {
        let text = "services:\n  db:\n    image: postgres:16\n    container_name: app-db\n";
        let finding = &container_name(&one(text))[0];
        assert_eq!(
            apply(text, finding.edit.as_ref().unwrap()),
            "services:\n  db:\n    image: postgres:16\n"
        );
    }

    #[test]
    fn container_name_in_flow_style_has_no_edit() {
        let findings = container_name(&one("services:\n  db: {image: pg, container_name: x}\n"));
        assert_eq!((findings.len(), findings[0].edit.is_none()), (1, true));
    }

    #[test]
    fn the_container_name_finding_says_what_it_breaks() {
        let text = "services:\n  db:\n    image: postgres:16\n    container_name: app-db\n";
        let findings = container_name(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::ContainerName);
        assert_eq!(
            finding.message,
            "db sets container_name: app-db. Compose won't start a second container with that name, so only one copy of the stack can run: an agent's copy fails while yours runs, and two copies started by hand collide. Removing it lets Compose name the container per project; other services still reach it as db."
        );
        assert_eq!(finding.affects, vec![Affects::Agents, Affects::ByHand]);
        assert_eq!(
            (finding.service.as_deref(), finding.lead),
            (Some("db"), false)
        );
        assert_eq!(
            finding.source,
            SourceRef {
                file: "compose.yaml".into(),
                line: 3,
                column: 20
            }
        );
        assert_eq!(spanned(text, finding.span.as_ref().unwrap()), "app-db");
        let edit = finding.edit.as_ref().unwrap();
        assert_eq!(edit.title, "Remove container_name");
        assert_eq!(edit.file, "compose.yaml");
        assert_eq!(spanned(text, &edit.span), "    container_name: app-db\n");
    }

    #[test]
    fn a_container_name_reached_twice_is_one_finding_named_after_its_writer() {
        // web comes first in the model, but the entry is written under base, which web extends.
        let project = one(
            "services:\n  web:\n    extends:\n      service: base\n  base:\n    image: nginx\n    container_name: app-web\n",
        );
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        assert_eq!(project.facts.container_names.len(), 2);
        let findings = container_name(&project);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].service.as_deref(), Some("base"));
        assert!(
            findings[0]
                .message
                .starts_with("base sets container_name: app-web. ")
        );
        assert!(findings[0].edit.is_some());
        // Merged from an anchor under an `x-` key, no service writes it: the first names it.
        let project = one(
            "x-db: &db\n  image: pg\n  container_name: app-db\nservices:\n  db:\n    <<: *db\n  replica:\n    <<: *db\n",
        );
        assert_eq!(project.facts.container_names.len(), 2);
        let findings = container_name(&project);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].service.as_deref(), Some("db"));
        assert_eq!(findings[0].source.line, 2);
    }

    #[test]
    fn a_container_name_holding_an_unprovided_variable_is_no_finding() {
        let project = one("services:\n  db:\n    image: pg\n    container_name: app-${X}\n");
        assert_eq!(project.facts.container_names.len(), 1);
        assert!(container_name(&project).is_empty());
        // A variable with a default is provided: the name is fixed to it.
        let project = one("services:\n  db:\n    image: pg\n    container_name: ${X:-app-db}\n");
        assert_eq!(
            messages(&container_name(&project))[0]
                .split('.')
                .next()
                .unwrap(),
            "db sets container_name: app-db"
        );
    }

    #[test]
    fn an_alias_or_anchored_container_name_has_no_edit() {
        for text in [
            "x-cn: &cn app-db\nservices:\n  db:\n    image: pg\n    container_name: *cn\n",
            "services:\n  db:\n    image: pg\n    container_name: &cn app-db\n",
        ] {
            let findings = container_name(&one(text));
            assert_eq!(findings.len(), 1, "{text:?}");
            assert!(findings[0].edit.is_none(), "{text:?}");
        }
    }

    #[test]
    fn an_entry_reached_through_an_anchor_has_no_edit() {
        // Removing the anchor's line would empty it for every resource merging it.
        let project = one(
            "x-external: &external\n  external: true\nvolumes:\n  a:\n    <<: *external\n    name: a\n  b:\n    <<: *external\n    name: b\n",
        );
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        let findings = shared_fixed_name(&project);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|finding| finding.edit.is_none()));
        // A container_name merged into two services from an anchor under an `x-` key.
        let project = one(
            "x-db: &db\n  image: pg\n  container_name: app-db\nservices:\n  db:\n    <<: *db\n  replica:\n    <<: *db\n",
        );
        let findings = container_name(&project);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].edit.is_none());
        // An extends base's own entry, written under its key, still gets its edit.
        let text = "services:\n  web:\n    extends:\n      service: base\n  base:\n    image: nginx\n    container_name: app-web\n";
        let findings = container_name(&one(text));
        assert_eq!(findings.len(), 1);
        assert_eq!(
            apply(text, findings[0].edit.as_ref().unwrap()),
            "services:\n  web:\n    extends:\n      service: base\n  base:\n    image: nginx\n"
        );
    }

    #[test]
    fn a_volume_with_a_fixed_name_loses_its_name_line() {
        let text = "volumes:\n  cache:\n    driver: local\n    name: app-cache\n";
        let findings = shared_fixed_name(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::SharedFixedName);
        assert_eq!(
            finding.message,
            "Volume cache has the fixed name app-cache, so every copy of the stack uses the same volume and shares its data. Removing name: gives each copy its own volume, named after its project; the data in app-cache stays where it is."
        );
        assert_eq!(finding.affects, vec![Affects::Agents, Affects::ByHand]);
        assert_eq!((finding.service.as_deref(), finding.lead), (None, false));
        assert_eq!(spanned(text, finding.span.as_ref().unwrap()), "app-cache");
        assert_eq!((finding.source.line, finding.source.column), (3, 10));
        let edit = finding.edit.as_ref().unwrap();
        assert_eq!(edit.title, "Give each copy its own volume");
        assert_eq!(apply(text, edit), "volumes:\n  cache:\n    driver: local\n");
    }

    #[test]
    fn an_external_volume_loses_external_and_its_name() {
        let text = "volumes:\n  data:\n    name: shared-data\n    # made by hand\n    external: true\nservices:\n  app:\n    image: app\n";
        let findings = shared_fixed_name(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(
            finding.message,
            "Volume data is external, so every copy of the stack uses the same existing volume and shares its data. Removing external: (and its name:) lets each copy create its own volume, which starts empty."
        );
        // The name is written first, so the finding is there.
        assert_eq!(spanned(text, finding.span.as_ref().unwrap()), "shared-data");
        let edit = finding.edit.as_ref().unwrap();
        assert_eq!(edit.title, "Give each copy its own volume");
        assert_eq!(
            apply(text, edit),
            "volumes:\n  data:\n    # made by hand\nservices:\n  app:\n    image: app\n"
        );
    }

    #[test]
    fn a_network_with_a_fixed_name_is_a_finding() {
        let text = "networks:\n  shared:\n    name: shared-net\n";
        let findings = shared_fixed_name(&one(text));
        assert_eq!(
            messages(&findings),
            [
                "Network shared has the fixed name shared-net, so every copy of the stack joins the same network. Removing name: gives each copy its own network, named after its project."
            ]
        );
        let edit = findings[0].edit.as_ref().unwrap();
        assert_eq!(edit.title, "Give each copy its own network");
        assert_eq!(apply(text, edit), "networks:\n  shared:\n");
    }

    #[test]
    fn an_external_network_is_a_finding() {
        let text = "networks:\n  outside:\n    external: true\n    name: lan\n";
        let findings = shared_fixed_name(&one(text));
        assert_eq!(
            messages(&findings),
            [
                "Network outside is external, so every copy of the stack joins the same existing network. Removing external: (and its name:) lets each copy create its own network."
            ]
        );
        assert_eq!(
            apply(text, findings[0].edit.as_ref().unwrap()),
            "networks:\n  outside:\n"
        );
        // Without a name: entry, the edit removes external: alone, and the message says so.
        let findings = shared_fixed_name(&one("networks:\n  outside:\n    external: true\n"));
        assert_eq!(
            messages(&findings),
            [
                "Network outside is external, so every copy of the stack joins the same existing network. Removing external: lets each copy create its own network."
            ]
        );
    }

    #[test]
    fn a_name_holding_an_unprovided_variable_is_not_fixed() {
        let project = one("volumes:\n  data:\n    name: ${DATA_VOLUME}\n");
        assert_eq!(project.facts.resources.len(), 1);
        assert!(shared_fixed_name(&project).is_empty());
        // external: true still counts; the edit removes it and leaves the name the variable sets.
        let text = "volumes:\n  data:\n    name: ${DATA_VOLUME}\n    external: true\n";
        let findings = shared_fixed_name(&one(text));
        // The edit keeps the name: entry, so the message doesn't say it goes.
        assert_eq!(
            messages(&findings),
            [
                "Volume data is external, so every copy of the stack uses the same existing volume and shares its data. Removing external: lets each copy create its own volume, which starts empty."
            ]
        );
        assert_eq!(spanned(text, findings[0].span.as_ref().unwrap()), "true");
        assert_eq!(
            apply(text, findings[0].edit.as_ref().unwrap()),
            "volumes:\n  data:\n    name: ${DATA_VOLUME}\n"
        );
    }

    #[test]
    fn a_legacy_external_name_has_no_edit() {
        let findings = shared_fixed_name(&one(
            "volumes:\n  data:\n    external:\n      name: shared\n",
        ));
        assert_eq!(findings.len(), 1);
        // No name: entry, so the wording leaves it out even with no edit.
        assert!(findings[0].message.ends_with(
            " Removing external: lets each copy create its own volume, which starts empty."
        ));
        assert!(findings[0].edit.is_none());
    }

    #[test]
    fn entries_in_two_files_are_a_finding_without_an_edit() {
        let files = MemFiles::from([
            ("a.yaml", "volumes:\n  data:\n    name: shared-data\n"),
            ("b.yaml", "volumes:\n  data:\n    external: true\n"),
        ]);
        let paths = ["a.yaml".to_string(), "b.yaml".to_string()];
        let project = load_project(&files, &Checkout::primary(), &paths);
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        let findings = shared_fixed_name(&project);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.starts_with("Volume data is external, "));
        // The file read first holds the first fixed entry.
        assert_eq!(findings[0].source.file, "a.yaml");
        assert!(findings[0].edit.is_none());
    }

    #[test]
    fn edits_leave_yaml_that_still_loads() {
        for text in [
            include_str!("../../../fixtures/external-volumes/compose.yaml"),
            include_str!("../../../fixtures/container-name/compose.yaml"),
        ] {
            let project = one(text);
            assert!(project.errors.is_empty(), "{:?}", project.errors);
            let findings: Vec<_> = container_name(&project)
                .into_iter()
                .chain(shared_fixed_name(&project))
                .collect();
            assert!(!findings.is_empty());
            for finding in findings {
                let edit = finding.edit.as_ref().expect("an edit");
                let fixed = one(&apply(text, edit));
                assert!(fixed.errors.is_empty(), "{edit:?}: {:?}", fixed.errors);
                let after: Vec<_> = container_name(&fixed)
                    .into_iter()
                    .chain(shared_fixed_name(&fixed))
                    .collect();
                assert!(
                    after
                        .iter()
                        .all(|other| (other.rule, &other.message)
                            != (finding.rule, &finding.message)),
                    "{edit:?} left {after:?}"
                );
            }
        }
    }
}
