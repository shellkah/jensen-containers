//! The plain texts agents read: the topology of a project and its readiness. Every path is
//! relative to the checkout root, lines are one-based as editors show them, and lines are joined
//! with `\n`, with no trailing one.

use crate::compose::Project;
use crate::edits::byte_offset;
use crate::model::{
    Affects, EdgeKind, Finding, LoadError, MountKind, NameOrigin, PortBinding, ResourceNode,
    RuleId, ServiceNode, SourceRef, TextEdit, VolumeMount,
};
use crate::paths::{Checkout, parent};

/// Services listed before the rest are summed up.
const MAX_SERVICES: usize = 30;
/// Readiness items listed before the rest are summed up.
const MAX_ITEMS: usize = 20;

const NOT_RUNNING: &str = "This describes what the files declare, not what is running.";

/// What the files of `project` declare: its name, services, volumes, networks and links, what
/// nothing provides, and what couldn't be loaded, ending in the next step.
pub fn topology_text(project: &Project, checkout: &Checkout) -> String {
    let model = &project.model;
    let mut lines = vec![name_line(project, checkout), files_line(project, checkout)];

    let count = model.services.len();
    lines.push(format!(
        "{count} {}:",
        if count == 1 { "service" } else { "services" }
    ));
    lines.extend(model.services.iter().take(MAX_SERVICES).map(service_line));
    if count > MAX_SERVICES {
        lines.push(format!(
            "... and {} more services; pass files to narrow it down.",
            count - MAX_SERVICES
        ));
    }

    let networks = if model.networks.is_empty() {
        "only the default network".to_owned()
    } else {
        resources(&model.networks)
    };
    let volumes = if model.volumes.is_empty() {
        "none".to_owned()
    } else {
        resources(&model.volumes)
    };
    lines.push(format!("Volumes: {volumes}. Networks: {networks}."));
    lines.push(links_line(project));

    let names = variable_names(project);
    if !names.is_empty() {
        lines.push(format!(
            "{} no default in the files: {}.",
            variables(names.len()),
            names.join(", ")
        ));
    }
    lines.extend(project.errors.iter().map(load_error_line));
    lines.push(NOT_RUNNING.to_owned());
    lines.push(
        "Next: call readiness with the same arguments to see what stops copies of this stack from running side by side."
            .to_owned(),
    );
    lines.join("\n")
}

/// What stops copies of the project from running side by side: `findings`, numbered, each with
/// what it affects and the fix when there is an edit, then what couldn't be loaded, ending in the
/// next step.
pub fn readiness_text(project: &Project, findings: &[Finding], checkout: &Checkout) -> String {
    let items = items(findings);
    let shown = &items[..items.len().min(MAX_ITEMS)];

    let mut header = format!("Readiness of {}", named_files(project));
    if !checkout.root().is_empty() {
        header.push_str(&format!(" in {}", checkout.root()));
    }
    header.push_str(&match items.len() {
        0 => ": nothing in the files stops copies of this stack from running side by side."
            .to_owned(),
        1 => ": 1 problem stops copies of this stack from running side by side.".to_owned(),
        n => format!(": {n} problems stop copies of this stack from running side by side."),
    });
    let mut lines = vec![header];

    let mut first_edit = None;
    for (index, item) in shown.iter().enumerate() {
        let number = index + 1;
        lines.push(match item {
            Item::Finding(finding) => {
                format!("{number}. {} ({})", finding.message, at(&finding.source))
            }
            Item::Variables(group) => format!("{number}. {}", lead_text(group)),
        });
        let affects = match item {
            Item::Finding(finding) => finding.affects.as_slice(),
            Item::Variables(_) => &[Affects::Agents],
        };
        if let Some(line) = affects_line(affects) {
            lines.push(line);
        }
        if let Item::Finding(Finding {
            edit: Some(edit), ..
        }) = item
        {
            lines.push(fix_line(project, edit));
            first_edit.get_or_insert((number, edit.file.as_str()));
        }
    }
    if items.len() > MAX_ITEMS {
        lines.push(format!(
            "... and {} more; pass files to narrow it down.",
            items.len() - MAX_ITEMS
        ));
    }

    lines.extend(project.errors.iter().map(load_error_line));
    lines.push(match (first_edit, items.is_empty(), project.errors.is_empty()) {
        (Some((number, file)), _, _) => format!(
            "Next: apply fix {number} in {file}, then call readiness again with the same arguments."
        ),
        (None, false, _) => {
            "Next: settle the points above, then start the stack with Run's run tool.".to_owned()
        }
        (None, true, true) => "Next: start the stack with Run's run tool.".to_owned(),
        (None, true, false) => {
            "Next: fix the load errors above, then call readiness again.".to_owned()
        }
    });
    lines.join("\n")
}

/// What to tell an agent when no Compose file could be found at the root of `checkout`.
pub fn no_compose_file_text(checkout: &Checkout) -> String {
    let place = match checkout.root() {
        "" => "the project",
        root => root,
    };
    format!(
        "No Compose file at the root of {place} (looked for compose.yaml, compose.yml, docker-compose.yaml and docker-compose.yml).\nNext: find the Compose files with your file tools and pass them in files, such as {{\"files\": [\"infra/local/docker-compose.yml\"]}}."
    )
}

/// One numbered entry of the readiness list.
enum Item<'a> {
    Finding(&'a Finding),
    /// The `unprovided_variable` findings, told as one.
    Variables(Vec<&'a Finding>),
}

/// The findings as items: the unprovided variables make one, placed where the first of them falls.
fn items(findings: &[Finding]) -> Vec<Item<'_>> {
    let mut items = Vec::new();
    let mut variables_at = None;
    let mut variables = Vec::new();
    for finding in findings {
        if finding.rule == RuleId::UnprovidedVariable {
            variables_at.get_or_insert(items.len());
            variables.push(finding);
        } else {
            items.push(Item::Finding(finding));
        }
    }
    if let Some(at) = variables_at {
        items.insert(at, Item::Variables(variables));
    }
    items
}

/// The text of the single item that tells of every unprovided variable.
fn lead_text(group: &[&Finding]) -> String {
    let named: Vec<(String, &SourceRef)> = group
        .iter()
        .map(|finding| {
            let name = finding.message.split(' ').next().unwrap_or_default();
            (name.to_owned(), &finding.source)
        })
        .collect();
    let list = named
        .iter()
        .map(|(name, source)| format!("{name} ({})", at(source)))
        .collect::<Vec<_>>()
        .join(", ");
    let first = named.first().map_or("", |(name, _)| name.as_str());
    if named.len() == 1 {
        format!(
            "Lead: 1 variable has no default, and nothing provides it when Run starts the stack (Run passes no --env-file), so it comes out empty there: {list}. Give it a default, such as ${{{first}:-<value>}}, or keep a .env file next to the Compose file."
        )
    } else {
        format!(
            "Lead: {} variables have no default, and nothing provides them when Run starts the stack (Run passes no --env-file), so they come out empty there: {list}. Give each a default, such as ${{{first}:-<value>}}, or keep a .env file next to the Compose file.",
            named.len()
        )
    }
}

fn affects_line(affects: &[Affects]) -> Option<String> {
    if affects.is_empty() {
        return None;
    }
    let who: Vec<&str> = affects
        .iter()
        .map(|who| match who {
            Affects::Agents => "agents' copies",
            Affects::ByHand => "copies started by hand",
        })
        .collect();
    Some(format!("   Affects: {}.", who.join(", ")))
}

/// How to apply `edit`: the text it replaces inside one line, or the lines it drops.
fn fix_line(project: &Project, edit: &TextEdit) -> String {
    let Some(source) = project
        .sources
        .find(&edit.file)
        .and_then(|id| project.sources.get(id))
    else {
        return format!("   Fix: {}.", edit.title);
    };
    let from = byte_offset(&source.text, edit.span.from);
    let to = byte_offset(&source.text, edit.span.to).max(from);
    let old = &source.text[from..to];
    // An edit that only removes text takes whole lines, even the last, which has no break.
    if !old.contains('\n') && !edit.new_text.is_empty() {
        return format!("   Fix: replace {old} with {}.", edit.new_text);
    }
    let first = source.line_of(from) + 1;
    let mut kept = edit.new_text.split_inclusive('\n').peekable();
    let mut gone = Vec::new();
    for (index, line) in old.split_inclusive('\n').enumerate() {
        if kept.peek() == Some(&line) {
            kept.next();
        } else {
            gone.push((first + index).to_string());
        }
    }
    match gone.as_slice() {
        [] => format!("   Fix: {}.", edit.title),
        [one] => format!("   Fix: {}, deleting line {one}.", edit.title),
        many => format!(
            "   Fix: {}, deleting lines {}.",
            edit.title,
            many.join(", ")
        ),
    }
}

fn name_line(project: &Project, checkout: &Checkout) -> String {
    let name = &project.model.project_name;
    let value = &name.value;
    match name.origin {
        NameOrigin::Name => {
            let file = project
                .facts
                .name_entry
                .as_ref()
                .and_then(|entry| project.sources.get(entry.file))
                .map(|source| source.path.as_str())
                .or(project.facts.first_file.as_deref())
                .unwrap_or_default();
            format!(
                "Compose project \"{value}\", set by name: in {file}, so it is the same in every checkout."
            )
        }
        NameOrigin::Folder => {
            let folder = parent(project.facts.first_file.as_deref().unwrap_or_default());
            if !folder.is_empty() {
                format!(
                    "Compose project \"{value}\", named after its folder {folder}, so it is the same in every checkout."
                )
            } else if checkout.root().is_empty() {
                "Compose project named after the project's folder.".to_owned()
            } else {
                format!("Compose project \"{value}\", named after the checkout's folder.")
            }
        }
    }
}

/// The files the texts name, comma separated: the ones read or, when nothing could be read, the
/// ones the load errors name, each once, in order.
fn named_files(project: &Project) -> String {
    if !project.model.files.is_empty() {
        return project.model.files.join(", ");
    }
    let mut names: Vec<&str> = Vec::new();
    for error in &project.errors {
        if !names.contains(&error.file.as_str()) {
            names.push(&error.file);
        }
    }
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn files_line(project: &Project, checkout: &Checkout) -> String {
    let files = named_files(project);
    if checkout.root().is_empty() {
        format!("Files: {files}.")
    } else {
        format!("Checkout: {}. Files: {files}.", checkout.root())
    }
}

fn service_line(service: &ServiceNode) -> String {
    let mut line = format!("- {}", service.name);
    if !service.profiles.is_empty() {
        line.push_str(&format!(" (profiles: {})", service.profiles.join(", ")));
    }
    let mut parts = Vec::new();
    if let Some(build) = &service.build {
        let mut part = format!("builds {}", build.context);
        if let Some(dockerfile) = &build.dockerfile {
            part.push_str(&format!(" with {dockerfile}"));
        }
        parts.push(part);
    }
    if let Some(image) = &service.image {
        parts.push(format!("image {image}"));
    }
    if !service.ports.is_empty() {
        let ports: Vec<String> = service.ports.iter().map(port).collect();
        parts.push(format!("ports {}", ports.join(", ")));
    }
    if !service.volumes.is_empty() {
        let mounts: Vec<String> = service.volumes.iter().map(mount).collect();
        parts.push(format!("volumes {}", mounts.join(", ")));
    }
    if let Some(name) = &service.container_name {
        parts.push(format!("container_name {name}"));
    }
    if !parts.is_empty() {
        line.push_str(&format!(": {}", parts.join("; ")));
    }
    line
}

fn port(port: &PortBinding) -> String {
    let protocol = port
        .protocol
        .as_deref()
        .map_or(String::new(), |protocol| format!("/{protocol}"));
    let Some(host) = &port.host else {
        return format!("{}{protocol} (random host port)", port.container);
    };
    let ip = port
        .host_ip
        .as_deref()
        .map_or(String::new(), |ip| format!("{ip}:"));
    let fixed = if port.fixed { " (fixed host port)" } else { "" };
    format!("{ip}{host}->{}{protocol}{fixed}", port.container)
}

fn mount(mount: &VolumeMount) -> String {
    let target = &mount.target;
    match (mount.kind, &mount.source) {
        (MountKind::Tmpfs, _) => format!("tmpfs {target}"),
        (MountKind::Bind, source) => {
            let flags = if mount.read_only {
                "bind, read-only"
            } else {
                "bind"
            };
            match source {
                Some(source) => format!("{source}:{target} ({flags})"),
                None => format!("{target} ({flags})"),
            }
        }
        (MountKind::Named, None) => format!("{target} (anonymous)"),
        (MountKind::Named, Some(source)) => {
            let flag = if mount.read_only { " (read-only)" } else { "" };
            format!("{source}:{target}{flag}")
        }
    }
}

/// Top-level volumes or networks, comma separated.
fn resources(resources: &[ResourceNode]) -> String {
    resources
        .iter()
        .map(|resource| match (&resource.fixed_name, resource.external) {
            (_, true) => format!("{} (external)", resource.name),
            (Some(fixed), false) => format!("{} (fixed name {fixed})", resource.name),
            (None, false) => resource.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `Links: a depends on b, c; d links to e.`, grouped per service and kind in edge order.
fn links_line(project: &Project) -> String {
    let mut groups: Vec<(&str, EdgeKind, Vec<&str>)> = Vec::new();
    for edge in &project.model.edges {
        if !matches!(edge.kind, EdgeKind::DependsOn | EdgeKind::Links) {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(from, kind, _)| *from == edge.from && *kind == edge.kind)
        {
            Some((_, _, targets)) => targets.push(&edge.to),
            None => groups.push((&edge.from, edge.kind, vec![&edge.to])),
        }
    }
    if groups.is_empty() {
        return "Links: none.".to_owned();
    }
    let groups: Vec<String> = groups
        .iter()
        .map(|(from, kind, targets)| {
            let verb = if *kind == EdgeKind::DependsOn {
                "depends on"
            } else {
                "links to"
            };
            format!("{from} {verb} {}", targets.join(", "))
        })
        .collect();
    format!("Links: {}.", groups.join("; "))
}

/// The distinct names of the variables with no default, in first-use order.
fn variable_names(project: &Project) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for unresolved in &project.unresolved {
        if !names.contains(&unresolved.variable.as_str()) {
            names.push(&unresolved.variable);
        }
    }
    names
}

/// `1 variable has` or `<n> variables have`.
fn variables(count: usize) -> String {
    if count == 1 {
        "1 variable has".to_owned()
    } else {
        format!("{count} variables have")
    }
}

fn load_error_line(error: &LoadError) -> String {
    let line = error.source.as_ref().map_or(String::new(), |source| {
        format!(" (line {})", source.line + 1)
    });
    let message = error.message.strip_suffix('.').unwrap_or(&error.message);
    format!("Could not load {}{line}: {message}.", error.file)
}

/// `<file>:<line>` with a one-based line.
fn at(source: &SourceRef) -> String {
    format!("{}:{}", source.file, source.line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::load_project;
    use crate::files::MemFiles;
    use crate::tools::tool_findings;

    const HORUS: &str = include_str!("../../fixtures/horus-like/infra/local/docker-compose.yml");
    const HOST_APP: &str = include_str!("../../fixtures/host-app/compose.yaml");
    const FILE: &str = "infra/local/docker-compose.yml";

    fn feat_x() -> Checkout {
        Checkout::parse(Some(".jensen/worktrees/feat-x")).unwrap()
    }

    fn load(checkout: &Checkout, path: &str, text: &str) -> Project {
        let files = MemFiles::from([(checkout.read_path(path).as_str(), text)]);
        load_project(&files, checkout, &[path.to_owned()])
    }

    fn horus_like() -> Project {
        load(&feat_x(), FILE, HORUS)
    }

    fn readiness_of(project: &Project, checkout: &Checkout) -> String {
        readiness_text(project, &tool_findings(project, checkout), checkout)
    }

    #[test]
    fn topology_of_the_horus_like_layout() {
        let text = topology_text(&horus_like(), &feat_x());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "Compose project \"local\", named after its folder infra/local, so it is the same in every checkout."
        );
        assert_eq!(
            lines[1],
            "Checkout: .jensen/worktrees/feat-x. Files: infra/local/docker-compose.yml."
        );
        assert_eq!(
            lines[3],
            "- db: builds infra/local; image app-pg:local; ports 127.0.0.1:5432->5432 (fixed host port); volumes app_pgdata:/var/lib/postgresql/data, ./initdb:/docker-entrypoint-initdb.d (bind, read-only), ../:/infra (bind, read-only), ../../db:/db (bind, read-only)"
        );
        assert_eq!(
            lines[4],
            "Volumes: app_pgdata. Networks: only the default network."
        );
        assert_eq!(
            lines[6],
            "5 variables have no default in the files: DEBIAN_RELEASE, PG_MAJOR, POSTGRESQL_VERSION, TIMESCALEDB_VERSION, PGVECTOR_VERSION."
        );
        assert_eq!(
            lines.last(),
            Some(
                &"Next: call readiness with the same arguments to see what stops copies of this stack from running side by side."
            )
        );
    }

    #[test]
    fn readiness_of_the_horus_like_layout() {
        let project = horus_like();
        let text = readiness_text(&project, &tool_findings(&project, &feat_x()), &feat_x());
        assert!(text.starts_with("Readiness of infra/local/docker-compose.yml in .jensen/worktrees/feat-x: 3 problems stop copies of this stack from running side by side.\n1. db publishes host port 5432"), "{text}");
        assert!(text.contains("(infra/local/docker-compose.yml:21)\n   Affects: agents' copies, copies started by hand.\n   Fix: replace \"127.0.0.1:5432:5432\" with \"127.0.0.1:${DB_PORT:-5432}:5432\".\n"), "{text}");
        assert!(
            text.contains("\n2. Every checkout names this Compose project \"local\""),
            "{text}"
        );
        assert!(
            text.contains("\n3. Lead: 5 variables have no default"),
            "{text}"
        );
        assert!(text.ends_with("Next: apply fix 1 in infra/local/docker-compose.yml, then call readiness again with the same arguments."), "{text}");
    }

    #[test]
    fn the_lead_names_every_variable_with_its_line() {
        let text = readiness_of(&horus_like(), &feat_x());
        assert!(text.contains("so they come out empty there: DEBIAN_RELEASE (infra/local/docker-compose.yml:6), PG_MAJOR (infra/local/docker-compose.yml:7), POSTGRESQL_VERSION"), "{text}");
        assert!(text.contains("PGVECTOR_VERSION (infra/local/docker-compose.yml:10). Give each a default, such as ${DEBIAN_RELEASE:-<value>}, or keep a .env file next to the Compose file.\n   Affects: agents' copies.\n"), "{text}");
    }

    #[test]
    fn one_unprovided_variable_is_told_in_the_singular() {
        let primary = Checkout::primary();
        let project = load(
            &primary,
            "compose.yaml",
            "services:\n  db:\n    image: pg:${TAG}\n",
        );
        let text = readiness_of(&project, &primary);
        assert_eq!(
            text,
            "Readiness of compose.yaml: 1 problem stops copies of this stack from running side by side.\n1. Lead: 1 variable has no default, and nothing provides it when Run starts the stack (Run passes no --env-file), so it comes out empty there: TAG (compose.yaml:3). Give it a default, such as ${TAG:-<value>}, or keep a .env file next to the Compose file.\n   Affects: agents' copies.\nNext: settle the points above, then start the stack with Run's run tool."
        );
        assert!(
            topology_text(&project, &primary)
                .contains("\n1 variable has no default in the files: TAG.\n")
        );
    }

    #[test]
    fn readiness_outside_a_worktree_leaves_the_project_name_out() {
        let primary = Checkout::primary();
        let project = load(&primary, FILE, HORUS);
        let text = readiness_of(&project, &primary);
        assert!(text.starts_with("Readiness of infra/local/docker-compose.yml: 2 problems stop copies of this stack from running side by side.\n1. db publishes host port 5432"), "{text}");
        assert!(!text.contains("Every checkout names"), "{text}");
        assert!(
            text.contains("\n2. Lead: 5 variables have no default"),
            "{text}"
        );
        let topology = topology_text(&project, &primary);
        assert!(
            topology.starts_with("Compose project \"local\", named after its folder infra/local,")
        );
        assert!(topology.contains("\nFiles: infra/local/docker-compose.yml.\n"));
        assert!(!topology.contains("Checkout:"));
    }

    #[test]
    fn readiness_with_nothing_to_fix() {
        let primary = Checkout::primary();
        let project = load(&primary, "compose.yaml", HOST_APP);
        assert_eq!(
            readiness_of(&project, &primary),
            "Readiness of compose.yaml: nothing in the files stops copies of this stack from running side by side.\nNext: start the stack with Run's run tool."
        );
    }

    #[test]
    fn no_compose_file_text_asks_for_files() {
        assert_eq!(
            no_compose_file_text(&Checkout::primary()),
            "No Compose file at the root of the project (looked for compose.yaml, compose.yml, docker-compose.yaml and docker-compose.yml).\nNext: find the Compose files with your file tools and pass them in files, such as {\"files\": [\"infra/local/docker-compose.yml\"]}."
        );
        assert!(
            no_compose_file_text(&feat_x())
                .starts_with("No Compose file at the root of .jensen/worktrees/feat-x (looked for")
        );
    }

    #[test]
    fn long_lists_are_cut_with_a_hint() {
        let primary = Checkout::primary();
        let mut text = "services:\n".to_owned();
        for n in 0..35 {
            text.push_str(&format!(
                "  s{n}:\n    ports:\n      - \"{}:80\"\n",
                9000 + n
            ));
        }
        let project = load(&primary, "compose.yaml", &text);
        let topology = topology_text(&project, &primary);
        let lines: Vec<&str> = topology.lines().collect();
        assert_eq!(lines[2], "35 services:");
        assert_eq!(lines[3], "- s0: ports 9000->80 (fixed host port)");
        assert_eq!(lines[32], "- s29: ports 9029->80 (fixed host port)");
        assert_eq!(
            lines[33],
            "... and 5 more services; pass files to narrow it down."
        );
        assert_eq!(
            lines[34],
            "Volumes: none. Networks: only the default network."
        );

        let readiness = readiness_of(&project, &primary);
        assert!(readiness.contains("35 problems stop"), "{readiness}");
        assert!(readiness.contains("\n20. s19 publishes"), "{readiness}");
        assert!(!readiness.contains("\n21. "), "{readiness}");
        assert!(
            readiness.contains("\n... and 15 more; pass files to narrow it down.\n"),
            "{readiness}"
        );
    }

    #[test]
    fn load_errors_are_listed() {
        let primary = Checkout::primary();
        let files = MemFiles::from([(
            "compose.yaml",
            "include:\n  - missing.yaml\nservices:\n  web:\n    image: nginx\n",
        )]);
        let project = load_project(&files, &primary, &["compose.yaml".to_owned()]);
        assert!(!project.errors.is_empty());
        let expected = project
            .errors
            .iter()
            .map(|error| {
                let line = error.source.as_ref().map_or(String::new(), |source| {
                    format!(" (line {})", source.line + 1)
                });
                let message = error.message.strip_suffix('.').unwrap_or(&error.message);
                format!("Could not load {}{line}: {message}.", error.file)
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            expected.starts_with("Could not load compose.yaml (line 2): "),
            "{expected}"
        );
        assert!(!expected.contains(".."), "{expected}");

        let topology = topology_text(&project, &primary);
        assert!(
            topology.contains(&format!(
                "{expected}\nThis describes what the files declare"
            )),
            "{topology}"
        );
        let readiness = readiness_of(&project, &primary);
        assert_eq!(
            readiness,
            format!(
                "Readiness of compose.yaml: nothing in the files stops copies of this stack from running side by side.\n{expected}\nNext: fix the load errors above, then call readiness again."
            )
        );
    }

    #[test]
    fn nothing_read_names_the_files_that_failed() {
        let primary = Checkout::primary();
        let files = MemFiles::default();
        let paths = ["a.yml".to_owned(), "b.yml".to_owned()];
        let project = load_project(&files, &primary, &paths);
        assert!(project.model.files.is_empty());
        assert!(project.errors.len() >= 2, "{:?}", project.errors);

        let readiness = readiness_of(&project, &primary);
        assert!(
            readiness.starts_with("Readiness of a.yml, b.yml: "),
            "{readiness}"
        );
        let topology = topology_text(&project, &primary);
        assert!(topology.contains("\nFiles: a.yml, b.yml.\n"), "{topology}");

        // A file with several errors is named once, the others in order.
        let mut project = project;
        let again = project.errors[0].clone();
        project.errors.push(again);
        let readiness = readiness_of(&project, &primary);
        assert!(
            readiness.starts_with("Readiness of a.yml, b.yml: "),
            "{readiness}"
        );
    }

    #[test]
    fn a_message_loses_one_trailing_dot_and_a_missing_file_has_no_line() {
        let error = LoadError {
            file: "a.yaml".into(),
            message: "can't read it.".into(),
            source: None,
        };
        assert_eq!(
            load_error_line(&error),
            "Could not load a.yaml: can't read it."
        );
    }

    #[test]
    fn the_project_name_line_says_where_the_name_comes_from() {
        let primary = Checkout::primary();
        let named = load(
            &primary,
            "infra/compose.yaml",
            "name: shop\nservices:\n  web:\n    image: nginx\n",
        );
        assert_eq!(
            topology_text(&named, &primary).lines().next(),
            Some(
                "Compose project \"shop\", set by name: in infra/compose.yaml, so it is the same in every checkout."
            )
        );
        let root = "services:\n  web:\n    image: nginx\n";
        let at_root = load(&primary, "compose.yaml", root);
        assert_eq!(
            topology_text(&at_root, &primary).lines().next(),
            Some("Compose project named after the project's folder.")
        );
        let in_worktree = load(&feat_x(), "compose.yaml", root);
        assert_eq!(
            topology_text(&in_worktree, &feat_x()).lines().next(),
            Some("Compose project \"feat-x\", named after the checkout's folder.")
        );
    }

    #[test]
    fn services_resources_and_links_are_told_plainly() {
        let primary = Checkout::primary();
        let project = load(
            &primary,
            "compose.yaml",
            "services:\n  web:\n    build:\n      context: .\n      dockerfile: Dockerfile.web\n    profiles: [dev, test]\n    ports:\n      - \"8080:80/udp\"\n      - \"80\"\n      - \"127.0.0.1::90\"\n    volumes:\n      - cache:/c:ro\n      - /anon\n      - type: tmpfs\n        target: /tmp\n    depends_on: [db, cache]\n    links: [db]\n    container_name: shop-web\n  db:\n    image: postgres\n  cache:\n    image: redis\n  plain: {}\nvolumes:\n  cache:\n    external: true\n  data:\n    name: shared-data\nnetworks:\n  front: {}\n",
        );
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        let text = topology_text(&project, &primary);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "Files: compose.yaml.");
        assert_eq!(lines[2], "4 services:");
        assert_eq!(
            lines[3],
            "- web (profiles: dev, test): builds . with Dockerfile.web; ports 8080->80/udp (fixed host port), 80 (random host port), 90 (random host port); volumes cache:/c (read-only), /anon (anonymous), tmpfs /tmp; container_name shop-web"
        );
        assert_eq!(lines[4], "- db: image postgres");
        assert_eq!(lines[6], "- plain");
        assert_eq!(
            lines[7],
            "Volumes: cache (external), data (fixed name shared-data). Networks: front."
        );
        assert_eq!(
            lines[8],
            "Links: web depends on db, cache; web links to db."
        );
    }

    #[test]
    fn fixes_inside_a_line_name_the_text_and_fixes_over_lines_name_the_lines() {
        let primary = Checkout::primary();
        let project = load(
            &primary,
            "compose.yaml",
            "services:\n  db:\n    image: pg\n    container_name: app-db\n  web:\n    image: nginx\n    container_name: app-web\n",
        );
        let findings = tool_findings(&project, &primary);
        let text = readiness_text(&project, &findings, &primary);
        assert!(text.contains(", deleting line 4.\n"), "{text}");
        assert!(text.contains(", deleting line 7.\n"), "{text}");
        assert!(text.ends_with("Next: apply fix 1 in compose.yaml, then call readiness again with the same arguments."), "{text}");

        // The last line has no break, so the old text sits in one line, yet the edit drops a line.
        let project = load(
            &primary,
            "compose.yaml",
            "services:\n  db:\n    image: pg\n    container_name: app-db",
        );
        let text = readiness_text(&project, &tool_findings(&project, &primary), &primary);
        assert!(text.contains(", deleting line 4.\n"), "{text}");
    }

    #[test]
    fn the_lines_an_edit_keeps_are_not_called_deleted() {
        let primary = Checkout::primary();
        let project = load(
            &primary,
            "compose.yaml",
            "volumes:\n  data:\n    name: shared\n    # why\n    driver: local\n    external: true\n",
        );
        let text = readiness_of(&project, &primary);
        assert!(text.contains(", deleting lines 3, 6.\n"), "{text}");
    }
}
