//! The fixture projects under `fixtures/`, loaded as `docker compose` reads them. Plan 2 pins its
//! findings on these files, so they change only together with its tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use containers::compose::{Project, ResourceKind, load_project};
use containers::model::{
    Edge, EdgeKind, MountKind, NameOrigin, ResourceNode, ServiceNode, SourceRef, VolumeMount,
};
use containers::paths::Checkout;

/// The project name of a Compose file at the root of the primary checkout: the folder's name,
/// which the backend can't see there.
const ROOT: (&str, NameOrigin) = ("", NameOrigin::Folder);

/// Loads `paths` of the fixture `case` in the primary checkout, checks it loads without errors and
/// gets the project name `name` (its value and origin), and gives the paths it read.
fn load(case: &str, paths: &[&str], name: (&str, NameOrigin)) -> (Project, Vec<String>) {
    let files = support::fixture(case);
    let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
    let project = load_project(&files, &Checkout::primary(), &paths);
    assert!(project.errors.is_empty(), "{case}: {:?}", project.errors);
    let project_name = &project.model.project_name;
    assert_eq!(
        (project_name.value.as_str(), project_name.origin),
        name,
        "{case}"
    );
    (project, files.reads())
}

fn names(project: &Project) -> Vec<&str> {
    project
        .model
        .services
        .iter()
        .map(|service| service.name.as_str())
        .collect()
}

fn service<'p>(project: &'p Project, name: &str) -> &'p ServiceNode {
    project
        .model
        .services
        .iter()
        .find(|service| service.name == name)
        .unwrap()
}

/// Each port as its host IP, host, container and whether it is fixed.
fn ports(service: &ServiceNode) -> Vec<(Option<&str>, Option<&str>, &str, bool)> {
    service
        .ports
        .iter()
        .map(|port| {
            (
                port.host_ip.as_deref(),
                port.host.as_deref(),
                port.container.as_str(),
                port.fixed,
            )
        })
        .collect()
}

fn at(file: &str, line: u32, column: u32) -> SourceRef {
    SourceRef {
        file: file.into(),
        line,
        column,
    }
}

fn mount(kind: MountKind, source: &str, target: &str, read_only: bool) -> VolumeMount {
    VolumeMount {
        kind,
        source: Some(source.into()),
        target: target.into(),
        read_only,
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        kind,
    }
}

/// Each resource as its name, fixed name and whether it is external.
fn resources(nodes: &[ResourceNode]) -> Vec<(&str, Option<&str>, bool)> {
    nodes
        .iter()
        .map(|node| {
            (
                node.name.as_str(),
                node.fixed_name.as_deref(),
                node.external,
            )
        })
        .collect()
}

fn unresolved(project: &Project) -> Vec<&str> {
    project
        .unresolved
        .iter()
        .map(|u| u.variable.as_str())
        .collect()
}

#[test]
fn root_file() {
    let (project, reads) = load("root-file", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["web", "db"]);

    let web = service(&project, "web");
    assert_eq!(web.build.as_ref().unwrap().context, ".");
    assert_eq!(ports(web), [(None, Some("8080"), "80", true)]);
    assert_eq!(web.ports[0].source, at("compose.yaml", 4, 8));
    assert_eq!(web.depends_on, ["db"]);

    let db = service(&project, "db");
    assert_eq!(db.image.as_deref(), Some("postgres:16"));
    assert_eq!(
        db.volumes,
        [mount(
            MountKind::Named,
            "dbdata",
            "/var/lib/postgresql/data",
            false
        )]
    );
    assert_eq!(resources(&project.model.volumes), [("dbdata", None, false)]);
    assert_eq!(
        project.model.edges,
        [
            edge("web", "db", EdgeKind::DependsOn),
            edge("db", "dbdata", EdgeKind::Volume)
        ]
    );
    assert!(project.unresolved.is_empty());
}

#[test]
fn horus_like_subfolder() {
    // The name comes from the file's folder, infra/local.
    let name = ("local", NameOrigin::Folder);
    let (project, reads) = load("horus-like", &["infra/local/docker-compose.yml"], name);
    // versions.env sits next to the stack and is never read.
    assert_eq!(reads, ["infra/local/docker-compose.yml"]);
    assert_eq!(names(&project), ["db"]);

    let db = service(&project, "db");
    assert_eq!(db.image.as_deref(), Some("app-pg:local"));
    assert_eq!(db.build.as_ref().unwrap().context, "infra/local");
    assert_eq!(ports(db), [(Some("127.0.0.1"), Some("5432"), "5432", true)]);
    assert_eq!(
        db.ports[0].source,
        at("infra/local/docker-compose.yml", 20, 8)
    );
    assert_eq!(
        db.volumes,
        [
            mount(
                MountKind::Named,
                "app_pgdata",
                "/var/lib/postgresql/data",
                false
            ),
            mount(
                MountKind::Bind,
                "./initdb",
                "/docker-entrypoint-initdb.d",
                true
            ),
            mount(MountKind::Bind, "../", "/infra", true),
            mount(MountKind::Bind, "../../db", "/db", true),
        ]
    );
    assert_eq!(
        resources(&project.model.volumes),
        [("app_pgdata", None, false)]
    );
    assert_eq!(
        unresolved(&project),
        [
            "DEBIAN_RELEASE",
            "PG_MAJOR",
            "POSTGRESQL_VERSION",
            "TIMESCALEDB_VERSION",
            "PGVECTOR_VERSION"
        ]
    );
    assert_eq!(
        project.unresolved[0].source,
        at("infra/local/docker-compose.yml", 5, 24)
    );
    assert_eq!(
        project.facts.first_file.as_deref(),
        Some("infra/local/docker-compose.yml")
    );
}

#[test]
fn several_files() {
    let (project, reads) = load(
        "several-files",
        &["compose.yaml", "compose.override.yaml"],
        ROOT,
    );
    assert_eq!(reads, ["compose.yaml", "compose.override.yaml"]);
    assert_eq!(project.model.files, reads);
    assert_eq!(names(&project), ["api"]);

    let api = service(&project, "api");
    assert_eq!(api.image.as_deref(), Some("node:22"));
    assert_eq!(
        ports(api),
        [
            (None, Some("3000"), "3000", true),
            (None, Some("9229"), "9229", true)
        ]
    );
    assert_eq!(api.ports[0].source, at("compose.yaml", 4, 8));
    assert_eq!(api.ports[1].source, at("compose.override.yaml", 3, 8));
    assert_eq!(api.source, at("compose.yaml", 1, 2));
    let files: Vec<_> = project.facts.ports.iter().map(|port| port.file).collect();
    assert_eq!(files, [0, 1]);
}

#[test]
fn profiles() {
    let (project, reads) = load("profiles", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["app", "debug"]);
    let app = service(&project, "app");
    assert_eq!(app.image.as_deref(), Some("app:dev"));
    assert!(app.profiles.is_empty());
    let debug = service(&project, "debug");
    assert_eq!(debug.image.as_deref(), Some("busybox"));
    assert_eq!(debug.profiles, ["debug"]);
}

#[test]
fn env_files_are_never_read() {
    let (project, reads) = load("env-files", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["app"]);
    let app = service(&project, "app");
    assert_eq!(app.image.as_deref(), Some("app:dev"));
    assert_eq!(ports(app), [(None, Some("${APP_PORT}"), "80", false)]);
    assert_eq!(unresolved(&project), ["APP_PORT"]);
    assert_eq!(project.unresolved[0].source, at("compose.yaml", 5, 9));
}

#[test]
fn host_app() {
    let (project, reads) = load("host-app", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["db"]);
    let db = service(&project, "db");
    assert_eq!(db.image.as_deref(), Some("postgres:16"));
    // A variable with a default lets each copy choose its own port.
    assert_eq!(ports(db), [(None, Some("5432"), "5432", false)]);
    assert!(project.unresolved.is_empty());
    assert!(project.model.edges.is_empty());
}

#[test]
fn container_name() {
    let (project, reads) = load("container-name", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["db"]);
    let db = service(&project, "db");
    assert_eq!(db.image.as_deref(), Some("postgres:16"));
    assert_eq!(db.container_name.as_deref(), Some("app-db"));
    let [fact] = project.facts.container_names.as_slice() else {
        panic!("one container_name");
    };
    let source = project.sources.get(fact.file).unwrap();
    assert_eq!(
        (
            fact.service.as_deref(),
            source.slice(fact.key.clone()),
            fact.text.as_str()
        ),
        (Some("db"), "container_name", "app-db")
    );
    assert_eq!(source.line_of(fact.key.start), 3);
}

#[test]
fn external_volumes() {
    let (project, reads) = load("external-volumes", &["compose.yaml"], ROOT);
    assert_eq!(reads, ["compose.yaml"]);
    assert_eq!(names(&project), ["app"]);
    let app = service(&project, "app");
    assert_eq!(
        app.volumes,
        [
            mount(MountKind::Named, "data", "/data", false),
            mount(MountKind::Named, "cache", "/cache", false)
        ]
    );
    assert_eq!(app.networks, ["shared"]);
    assert_eq!(
        resources(&project.model.volumes),
        [
            ("data", Some("shared-data"), true),
            ("cache", Some("app-cache"), false)
        ]
    );
    assert_eq!(
        resources(&project.model.networks),
        [("shared", Some("shared-net"), false)]
    );
    let facts: Vec<_> = project
        .facts
        .resources
        .iter()
        .map(|fact| (fact.kind, fact.name.as_str()))
        .collect();
    assert_eq!(
        facts,
        [
            (ResourceKind::Volume, "data"),
            (ResourceKind::Volume, "cache"),
            (ResourceKind::Network, "shared")
        ]
    );
    assert_eq!(
        project.model.edges,
        [
            edge("app", "shared", EdgeKind::Network),
            edge("app", "data", EdgeKind::Volume),
            edge("app", "cache", EdgeKind::Volume)
        ]
    );
}

#[test]
fn include_and_extends() {
    let (project, reads) = load("include-extends", &["compose.yaml"], ROOT);
    let order = ["compose.yaml", "services/worker.yml", "common/base.yml"];
    assert_eq!(reads, order);
    assert_eq!(project.model.files, order);
    assert_eq!(names(&project), ["api", "worker"]);

    let api = service(&project, "api");
    assert_eq!(api.image.as_deref(), Some("app:dev"));
    assert_eq!(ports(api), [(None, Some("8000"), "8000", true)]);
    assert_eq!(api.source, at("compose.yaml", 3, 2));

    let worker = service(&project, "worker");
    assert_eq!(worker.image.as_deref(), Some("app:dev"));
    assert!(worker.ports.is_empty());
    assert_eq!(worker.source, at("services/worker.yml", 1, 2));
}
