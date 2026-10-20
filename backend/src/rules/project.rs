//! `same_project_name` and `unprovided_variable`: what a copy of the stack shares by name, and what
//! comes out empty when Run starts it. Editing the Compose file alone fixes neither, so neither
//! finding carries an edit.

use std::collections::HashSet;

use crate::compose::Project;
use crate::model::{Affects, Finding, NameOrigin, RuleId, SourceRef};
use crate::paths::parent;

/// A finding when every checkout of the project gets the same Compose project name: the name comes
/// from a top-level `name:`, or from the folder of a first file that sits in a subfolder (a file at
/// the checkout root takes each checkout's own folder name, which differs). Nothing when `name:`
/// holds a variable nothing provides, or when the first file wasn't among the files read.
pub fn same_project_name(project: &Project) -> Vec<Finding> {
    let Some(first) = project.facts.first_file.as_deref() else {
        return Vec::new();
    };
    if project.sources.find(first).is_none() {
        return Vec::new();
    }
    let name = &project.model.project_name;
    let value = &name.value;
    let (message, source, span) = match name.origin {
        NameOrigin::Name => {
            let Some(entry) = &project.facts.name_entry else {
                return Vec::new();
            };
            let Some(source) = project.sources.get(entry.file) else {
                return Vec::new();
            };
            if super::names::unprovided(project, entry) {
                return Vec::new();
            }
            (
                format!(
                    "name: {value} fixes the Compose project name, so copies started by hand from two checkouts are the same project: the second docker compose up takes over the first one's containers. Run's agents aren't affected, since Run passes its own -p. To run a second copy by hand, pass docker compose -p <name>; it overrides name:."
                ),
                source.source_ref(entry.value.start),
                Some(source.span(entry.value.clone())),
            )
        }
        NameOrigin::Folder => {
            let folder = parent(first);
            if folder.is_empty() {
                return Vec::new();
            }
            (
                format!(
                    "Every checkout names this Compose project \"{value}\", after its folder {folder}, so copies started by hand from two checkouts are the same project: the second docker compose up takes over the first one's containers. Run's agents aren't affected, since Run names each session's project. To run a second copy by hand, give it its own name with docker compose -p <name> or COMPOSE_PROJECT_NAME."
                ),
                SourceRef {
                    file: first.to_owned(),
                    line: 0,
                    column: 0,
                },
                None,
            )
        }
    };
    vec![Finding {
        rule: RuleId::SameProjectName,
        service: None,
        message,
        affects: vec![Affects::ByHand],
        lead: false,
        source,
        span,
        edit: None,
    }]
}

/// One finding per distinct variable with no default, at its first use. `unresolved` still lists
/// every use.
pub fn unprovided_variable(project: &Project) -> Vec<Finding> {
    let mut seen = HashSet::new();
    let mut findings = Vec::new();
    for variable in &project.facts.variables {
        let Some(source) = project.sources.get(variable.file) else {
            continue;
        };
        if !seen.insert(variable.name.as_str()) {
            continue;
        }
        let name = &variable.name;
        findings.push(Finding {
            rule: RuleId::UnprovidedVariable,
            service: None,
            message: format!(
                "{name} has no default, and nothing provides it when Run starts the stack: Run passes no --env-file, so it comes out empty there. Give it a default, such as ${{{name}:-<value>}}, or keep a .env file next to the Compose file. This is a lead: the plugin reads YAML files only, so it can't see a .env file."
            ),
            affects: vec![Affects::Agents],
            lead: true,
            source: source.source_ref(variable.bytes.start),
            span: Some(source.span(variable.bytes.clone())),
            edit: None,
        });
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{load_project, one};
    use crate::files::MemFiles;
    use crate::model::{Affects, RuleId, SourceRef};
    use crate::paths::Checkout;

    fn at(file: &str, line: u32, column: u32) -> SourceRef {
        SourceRef {
            file: file.into(),
            line,
            column,
        }
    }

    /// Loads `text` as the one Compose file `path` of the primary checkout.
    fn in_file(path: &str, text: &str) -> Project {
        let files = MemFiles::from([(path, text)]);
        let project = load_project(&files, &Checkout::primary(), &[path.to_string()]);
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        project
    }

    #[test]
    fn unprovided_variables_are_leads_for_agents() {
        let text = "services:\n  db:\n    image: postgres:${PG_MAJOR}\n";
        let finding = &unprovided_variable(&one(text))[0];
        assert_eq!(
            (
                finding.lead,
                finding.affects.clone(),
                finding.edit.is_none()
            ),
            (true, vec![Affects::Agents], true)
        );
        let span = finding.span.clone().unwrap();
        assert_eq!(&text[span.from as usize..span.to as usize], "${PG_MAJOR}");
    }

    #[test]
    fn the_unprovided_variable_finding_says_what_to_do() {
        let text = "services:\n  db:\n    image: postgres:${PG_MAJOR}\n";
        let findings = unprovided_variable(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::UnprovidedVariable);
        assert_eq!(
            finding.message,
            "PG_MAJOR has no default, and nothing provides it when Run starts the stack: Run passes no --env-file, so it comes out empty there. Give it a default, such as ${PG_MAJOR:-<value>}, or keep a .env file next to the Compose file. This is a lead: the plugin reads YAML files only, so it can't see a .env file."
        );
        assert_eq!(finding.service, None);
        assert_eq!(finding.source, at("compose.yaml", 2, 20));
    }

    #[test]
    fn one_finding_per_variable_at_its_first_use() {
        let text = "services:\n  db:\n    image: postgres:${PG_MAJOR}\n    environment:\n      MAJOR: ${PG_MAJOR}\n      HOST: ${DB_HOST}\n";
        let findings = unprovided_variable(&one(text));
        let seen: Vec<_> = findings
            .iter()
            .map(|finding| {
                let name = finding.message.split(' ').next().unwrap();
                (name, finding.source.line)
            })
            .collect();
        assert_eq!(seen, [("PG_MAJOR", 2), ("DB_HOST", 5)]);
        // A variable with a default is provided.
        assert!(unprovided_variable(&one("services:\n  a:\n    image: x:${V:-1}\n")).is_empty());
    }

    #[test]
    fn a_name_key_makes_every_checkout_the_same_project() {
        let text = "name: local\nservices:\n  web:\n    image: nginx\n";
        let findings = same_project_name(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::SameProjectName);
        assert_eq!(
            finding.message,
            "name: local fixes the Compose project name, so copies started by hand from two checkouts are the same project: the second docker compose up takes over the first one's containers. Run's agents aren't affected, since Run passes its own -p. To run a second copy by hand, pass docker compose -p <name>; it overrides name:."
        );
        assert_eq!(
            (
                finding.lead,
                finding.affects.clone(),
                finding.edit.is_none()
            ),
            (false, vec![Affects::ByHand], true)
        );
        assert_eq!(finding.service, None);
        assert_eq!(finding.source, at("compose.yaml", 0, 6));
        let span = finding.span.clone().unwrap();
        assert_eq!(&text[span.from as usize..span.to as usize], "local");
    }

    #[test]
    fn a_compose_file_in_a_subfolder_names_every_checkout_the_same() {
        let project = in_file(
            "infra/local/docker-compose.yml",
            "services:\n  web:\n    image: nginx\n",
        );
        let findings = same_project_name(&project);
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::SameProjectName);
        assert_eq!(
            finding.message,
            "Every checkout names this Compose project \"local\", after its folder infra/local, so copies started by hand from two checkouts are the same project: the second docker compose up takes over the first one's containers. Run's agents aren't affected, since Run names each session's project. To run a second copy by hand, give it its own name with docker compose -p <name> or COMPOSE_PROJECT_NAME."
        );
        assert_eq!(
            (
                finding.lead,
                finding.affects.clone(),
                finding.edit.is_none()
            ),
            (false, vec![Affects::ByHand], true)
        );
        assert_eq!(finding.span, None);
        assert_eq!(finding.source, at("infra/local/docker-compose.yml", 0, 0));
    }

    #[test]
    fn a_root_compose_file_is_fine() {
        let project = one("services:\n  web:\n    image: nginx\n");
        assert!(same_project_name(&project).is_empty());
    }

    #[test]
    fn a_name_holding_an_unprovided_variable_is_no_finding() {
        let project = one("name: ${PROJECT}\nservices:\n  web:\n    image: nginx\n");
        assert_eq!(project.facts.variables.len(), 1);
        assert!(same_project_name(&project).is_empty());
        // With a default the name is fixed to it.
        let project = one("name: ${PROJECT:-local}\nservices:\n  web:\n    image: nginx\n");
        assert_eq!(same_project_name(&project).len(), 1);
    }

    #[test]
    fn nothing_is_reported_when_the_first_file_wasnt_read() {
        let files = MemFiles::default();
        let paths = ["infra/local/docker-compose.yml".to_string()];
        let project = load_project(&files, &Checkout::primary(), &paths);
        assert!(!project.errors.is_empty());
        assert_eq!(
            project.facts.first_file.as_deref(),
            Some("infra/local/docker-compose.yml")
        );
        assert!(
            project
                .sources
                .find("infra/local/docker-compose.yml")
                .is_none()
        );
        assert!(same_project_name(&project).is_empty());
    }
}
