//! The loader's tests: one file, then several, `include`, `extends` and checkouts.

use super::*;
use crate::files::{MemFiles, RecordingFiles};
use crate::model::{
    Build, Edge, EdgeKind, MountKind, NameOrigin, ResourceNode, ServiceNode, SourceRef, Span,
    VolumeMount,
};
use crate::short_syntax::PortParts;

fn name_of(text: &str, path: &str, checkout: Option<&str>) -> (String, NameOrigin) {
    let checkout = Checkout::parse(checkout).unwrap();
    let read_path = checkout.read_path(path);
    let files = MemFiles::from([(read_path.as_str(), text)]);
    let project = load_project(&files, &checkout, &[path.to_string()]);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let name = project.model.project_name;
    (name.value, name.origin)
}

fn at(file: &str, line: u32, column: u32) -> SourceRef {
    SourceRef {
        file: file.into(),
        line,
        column,
    }
}

/// The text of a span, for ASCII texts where UTF-16 offsets are byte offsets.
fn spanned<'t>(text: &'t str, span: &Span) -> &'t str {
    &text[span.from as usize..span.to as usize]
}

#[test]
fn ports_from_variables_are_not_fixed() {
    let project = one(
        "services:\n  db:\n    ports:\n      - \"${DB_PORT:-5432}:5432\"\n      - \"${PG_PORT}:5433\"\n",
    );
    let ports = &project.model.services[0].ports;
    assert_eq!(
        (ports[0].host.as_deref(), ports[0].fixed),
        (Some("5432"), false)
    );
    assert_eq!(
        (ports[1].host.as_deref(), ports[1].fixed),
        (Some("${PG_PORT}"), false)
    );
    assert_eq!(project.unresolved[0].variable, "PG_PORT");
}

#[test]
fn the_project_name_comes_from_name_or_the_folder() {
    assert_eq!(
        name_of("name: app\nservices: {}\n", "compose.yaml", None),
        ("app".into(), NameOrigin::Name)
    );
    assert_eq!(
        name_of("services: {}\n", "infra/local/docker-compose.yml", None),
        ("local".into(), NameOrigin::Folder)
    );
    assert_eq!(
        name_of("services: {}\n", "compose.yaml", None),
        ("".into(), NameOrigin::Folder)
    );
    assert_eq!(
        name_of(
            "services: {}\n",
            "compose.yaml",
            Some(".jensen/worktrees/Feat.X")
        ),
        ("featx".into(), NameOrigin::Folder)
    );
}

#[test]
fn unresolved_variables_point_at_their_dollar() {
    let text = "services:\n  db:\n    build:\n      context: .\n      args:\n        DEBIAN_RELEASE: ${DEBIAN_RELEASE}\n";
    let project = one(text);
    assert_eq!(
        project.unresolved[0].source,
        SourceRef {
            file: "compose.yaml".into(),
            line: 5,
            column: 24
        }
    );
    assert_eq!(
        &text[project.facts.variables[0].bytes.clone()],
        "${DEBIAN_RELEASE}"
    );
}

#[test]
fn services_keep_their_fields_and_positions() {
    let text = "services:\n  web:\n    image: nginx:${TAG:-1.27}\n    build:\n      context: ../app\n      dockerfile: Dockerfile.dev\n    container_name: web-1\n    profiles: [debug, \"${EXTRA:-tools}\"]\n    networks: [front, back]\n    depends_on: [db]\n  db:\n    build: .\n    networks:\n      back:\n        aliases: [database]\n    depends_on:\n      cache:\n        condition: service_healthy\n  cache:\n    build: https://github.com/x/cache.git#main\n  out:\n    build:\n      dockerfile: Dockerfile\n  far:\n    build: ../../../elsewhere\n";
    let path = "infra/local/compose.yaml";
    let files = MemFiles::from([(path, text)]);
    let project = load_project(&files, &Checkout::primary(), &[path.to_string()]);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let names: Vec<_> = project
        .model
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["web", "db", "cache", "out", "far"]);
    let [web, db, cache, out, far] = project.model.services.as_slice() else {
        panic!("five services");
    };

    assert_eq!(web.image.as_deref(), Some("nginx:1.27"));
    assert_eq!(
        web.build,
        Some(Build {
            context: "infra/app".into(),
            dockerfile: Some("Dockerfile.dev".into())
        })
    );
    assert_eq!(web.container_name.as_deref(), Some("web-1"));
    assert_eq!(web.profiles, ["debug", "tools"]);
    assert_eq!(web.networks, ["front", "back"]);
    assert_eq!(web.depends_on, ["db"]);
    assert_eq!(web.source, at(path, 1, 2));

    assert_eq!(db.image, None);
    assert_eq!(
        db.build,
        Some(Build {
            context: "infra/local".into(),
            dockerfile: None
        })
    );
    assert_eq!(db.networks, ["back"]);
    assert_eq!(db.depends_on, ["cache"]);
    assert_eq!(db.container_name, None);
    assert!(db.profiles.is_empty());
    assert_eq!(db.source, at(path, 10, 2));

    // A URL, and a context that leaves the checkout, stay as written.
    let context = |service: &ServiceNode| service.build.clone().unwrap().context;
    assert_eq!(context(cache), "https://github.com/x/cache.git#main");
    assert_eq!(context(far), "../../../elsewhere");
    // A build mapping without a context builds the file's folder.
    assert_eq!(
        out.build,
        Some(Build {
            context: "infra/local".into(),
            dockerfile: Some("Dockerfile".into())
        })
    );

    // At the checkout root the context is the root itself.
    let root = one("services:\n  app:\n    build: .\n");
    assert_eq!(context(&root.model.services[0]), ".");
}

#[test]
fn short_and_long_ports_become_bindings() {
    let text = "services:\n  db:\n    ports:\n      - 127.0.0.1:5432:5432\n      - \"53:53/udp\"\n      - 5432\n      - target: 80\n        published: 8080\n        host_ip: 0.0.0.0\n        protocol: tcp\n      - target: 443\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let ports = &project.model.services[0].ports;
    assert_eq!(ports.len(), 5);

    let fixed_ip = &ports[0];
    assert_eq!(
        (
            fixed_ip.host_ip.as_deref(),
            fixed_ip.host.as_deref(),
            fixed_ip.container.as_str(),
            fixed_ip.protocol.as_deref(),
            fixed_ip.fixed
        ),
        (Some("127.0.0.1"), Some("5432"), "5432", None, true)
    );
    assert_eq!(fixed_ip.source, at("compose.yaml", 3, 8));
    assert_eq!(spanned(text, &fixed_ip.span), "127.0.0.1:5432:5432");

    let udp = &ports[1];
    assert_eq!(
        (
            udp.host.as_deref(),
            udp.container.as_str(),
            udp.protocol.as_deref(),
            udp.fixed
        ),
        (Some("53"), "53", Some("udp"), true)
    );
    assert_eq!(spanned(text, &udp.span), "\"53:53/udp\"");

    let random = &ports[2];
    assert_eq!(
        (
            random.host.as_deref(),
            random.container.as_str(),
            random.fixed
        ),
        (None, "5432", false)
    );

    let long = &ports[3];
    assert_eq!(
        (
            long.host_ip.as_deref(),
            long.host.as_deref(),
            long.container.as_str(),
            long.protocol.as_deref(),
            long.fixed
        ),
        (Some("0.0.0.0"), Some("8080"), "80", Some("tcp"), true)
    );
    assert_eq!(spanned(text, &long.span), "8080");
    assert_eq!(long.source, at("compose.yaml", 7, 19));

    let long_random = &ports[4];
    assert_eq!(
        (long_random.host.as_deref(), long_random.fixed),
        (None, false)
    );
    assert_eq!(spanned(text, &long_random.span), "443");

    let facts = &project.facts.ports;
    assert_eq!(facts.len(), 5);
    assert_eq!(
        facts[0],
        PortFact {
            service: "db".into(),
            file: 0,
            bytes: text.find("127.0.0.1").unwrap()..text.find("\n      - \"53").unwrap(),
            form: PortForm::Short(PortParts {
                ip: Some("127.0.0.1".into()),
                host: Some("5432".into()),
                container: "5432".into(),
                protocol: None
            }),
            host: Some("5432".into()),
            fixed: true,
        }
    );
    assert_eq!(&text[facts[1].bytes.clone()], "\"53:53/udp\"");
    assert_eq!((facts[2].host.as_deref(), facts[2].fixed), (None, false));
    assert_eq!(
        (
            &text[facts[3].bytes.clone()],
            &facts[3].form,
            facts[3].fixed
        ),
        ("8080", &PortForm::Long, true)
    );
    assert_eq!(
        (
            &text[facts[4].bytes.clone()],
            &facts[4].form,
            facts[4].fixed
        ),
        ("443", &PortForm::Long, false)
    );
}

#[test]
fn volumes_become_mounts() {
    let text = "services:\n  db:\n    volumes:\n      - data:/var/lib/data\n      - ./init:/init:ro,z\n      - /anon\n      - ${DATA:-./d}:/d\n      - type: bind\n        source: ./src\n        target: /src\n        read_only: true\n      - type: tmpfs\n        target: /tmp\n      - type: volume\n        source: cache\n        target: /cache\n      - type: npipe\n        source: //./pipe/x\n        target: //./pipe/x\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let mount = |kind, source: Option<&str>, target: &str, read_only| VolumeMount {
        kind,
        source: source.map(Into::into),
        target: target.into(),
        read_only,
    };
    assert_eq!(
        project.model.services[0].volumes,
        [
            mount(MountKind::Named, Some("data"), "/var/lib/data", false),
            mount(MountKind::Bind, Some("./init"), "/init", true),
            mount(MountKind::Named, None, "/anon", false),
            mount(MountKind::Bind, Some("./d"), "/d", false),
            mount(MountKind::Bind, Some("./src"), "/src", true),
            mount(MountKind::Tmpfs, None, "/tmp", false),
            mount(MountKind::Named, Some("cache"), "/cache", false),
        ]
    );
}

#[test]
fn top_level_volumes_and_networks_become_resources() {
    let text = "services: {}\nvolumes:\n  data:\n    external: true\n    name: shared-data\n  cache:\n    name: app-cache\n  plain:\n  old:\n    external:\n      name: legacy\n  local:\n    external: false\nnetworks:\n  shared:\n    name: ${NET:-shared-net}\n  default:\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let summary = |nodes: &[ResourceNode]| {
        nodes
            .iter()
            .map(|node| (node.name.clone(), node.fixed_name.clone(), node.external))
            .collect::<Vec<_>>()
    };
    let named = |name: &str, fixed: Option<&str>, external| {
        (name.to_string(), fixed.map(String::from), external)
    };
    assert_eq!(
        summary(&project.model.volumes),
        [
            named("data", Some("shared-data"), true),
            named("cache", Some("app-cache"), false),
            named("plain", None, false),
            named("old", Some("legacy"), true),
            named("local", None, false),
        ]
    );
    assert_eq!(project.model.volumes[0].source, at("compose.yaml", 2, 2));
    assert_eq!(
        summary(&project.model.networks),
        [
            named("shared", Some("shared-net"), false),
            named("default", None, false)
        ]
    );
    assert_eq!(project.model.networks[0].source, at("compose.yaml", 14, 2));

    let resources = &project.facts.resources;
    let kinds: Vec<_> = resources
        .iter()
        .map(|r| (r.kind, r.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (ResourceKind::Volume, "data"),
            (ResourceKind::Volume, "cache"),
            (ResourceKind::Volume, "old"),
            (ResourceKind::Network, "shared"),
        ]
    );
    let slices = |entry: &Option<EntryFact>| {
        entry.as_ref().map(|entry| {
            (
                &text[entry.key.clone()],
                &text[entry.value.clone()],
                entry.text.clone(),
            )
        })
    };
    assert_eq!(
        slices(&resources[0].name_entry),
        Some(("name", "shared-data", "shared-data".into()))
    );
    assert_eq!(
        slices(&resources[0].external_entry),
        Some(("external", "true", "true".into()))
    );
    assert_eq!(
        slices(&resources[1].name_entry),
        Some(("name", "app-cache", "app-cache".into()))
    );
    assert_eq!(resources[1].external_entry, None);
    assert_eq!(resources[2].name_entry, None);
    assert_eq!(
        slices(&resources[2].external_entry),
        Some(("external", "name: legacy", "name: legacy".into()))
    );
    assert_eq!(
        slices(&resources[3].name_entry),
        Some(("name", "${NET:-shared-net}", "shared-net".into()))
    );
    assert!(resources.iter().all(|r| r.file == 0));
    assert!(resources[0].name_entry.as_ref().unwrap().service.is_none());
}

#[test]
fn edges_follow_depends_on_links_networks_and_volumes() {
    let text = "services:\n  web:\n    depends_on: [db]\n    links: [\"cache:redis\", db]\n    networks: [front]\n    volumes: [\"data:/data\", \"./src:/src\", \"data:/again\", /anon]\n  db:\n    networks: {back: {}}\n    depends_on: {cache: {condition: service_started}}\n  cache: {}\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let edge = |from: &str, to: &str, kind| Edge {
        from: from.into(),
        to: to.into(),
        kind,
    };
    assert_eq!(
        project.model.edges,
        [
            edge("web", "db", EdgeKind::DependsOn),
            edge("web", "cache", EdgeKind::Links),
            edge("web", "db", EdgeKind::Links),
            edge("web", "front", EdgeKind::Network),
            edge("web", "data", EdgeKind::Volume),
            edge("db", "cache", EdgeKind::DependsOn),
            edge("db", "back", EdgeKind::Network),
        ]
    );
    // A service on the default network names none.
    assert!(project.model.services[2].networks.is_empty());
}

#[test]
fn facts_record_container_names_and_fixed_names() {
    let text = "name: app\nservices:\n  db:\n    image: postgres:16\n    container_name: ${CN:-app-db}\n  flow: {image: pg, container_name: x}\n  free:\n    image: redis\nvolumes:\n  cache:\n    name: app-cache\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let facts = &project.facts;

    let names: Vec<_> = facts
        .container_names
        .iter()
        .map(|entry| {
            (
                entry.service.as_deref(),
                &text[entry.key.clone()],
                &text[entry.value.clone()],
                entry.text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            (Some("db"), "container_name", "${CN:-app-db}", "app-db"),
            (Some("flow"), "container_name", "x", "x"),
        ]
    );
    assert_eq!(
        project.model.services[0].container_name.as_deref(),
        Some("app-db")
    );

    let name = facts.resources[0].name_entry.as_ref().unwrap();
    assert_eq!(
        (&text[name.key.clone()], &text[name.value.clone()]),
        ("name", "app-cache")
    );

    let project_name = facts.name_entry.as_ref().unwrap();
    assert_eq!(
        (
            project_name.service.as_deref(),
            &text[project_name.key.clone()],
            &text[project_name.value.clone()],
            project_name.text.as_str()
        ),
        (None, "name", "app", "app")
    );
    assert_eq!(facts.first_file.as_deref(), Some("compose.yaml"));
}

#[test]
fn bad_shapes_are_load_errors_not_panics() {
    let error_at = |project: &Project| {
        project
            .errors
            .iter()
            .map(|error| {
                let source = error.source.clone().unwrap();
                (error.file.clone(), source.line, source.column)
            })
            .collect::<Vec<_>>()
    };
    let file = |line, column| ("compose.yaml".to_string(), line, column);

    let list = one("- a\n- b\n");
    assert_eq!(error_at(&list), [file(0, 0)]);
    assert!(list.model.services.is_empty());

    let empty = one("# nothing yet\n");
    assert_eq!(error_at(&empty), [file(0, 0)]);

    let services_list = one("services: [a]\n");
    assert_eq!(error_at(&services_list), [file(0, 10)]);
    assert!(services_list.model.services.is_empty());

    let text = "services:\n  db: postgres\n  web:\n    image: nginx\n    ports: {a: b}\n  api:\n    image: node\n    ports:\n      - [8080]\n      - \"8080:\"\n      - 3000:3000\n      - {published: 1}\n    volumes:\n      - ~\n      - \"a:b:c:d\"\n      - {type: bind}\n      - data:/data\n  none:\n";
    let mixed = one(text);
    assert_eq!(
        error_at(&mixed),
        [
            file(1, 6),
            file(4, 11),
            file(8, 8),
            file(9, 8),
            file(11, 8),
            file(13, 8),
            file(14, 8),
            file(15, 8),
            // An empty value sits before its colon (Ruling 8).
            file(17, 6),
        ]
    );
    let names: Vec<_> = mixed
        .model
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["web", "api"]);
    let [web, api] = mixed.model.services.as_slice() else {
        panic!("two services");
    };
    assert_eq!((web.image.as_deref(), web.ports.len()), (Some("nginx"), 0));
    assert_eq!(api.ports.len(), 1);
    assert_eq!(api.ports[0].host.as_deref(), Some("3000"));
    assert_eq!(api.volumes.len(), 1);
    assert_eq!(api.volumes[0].source.as_deref(), Some("data"));
    assert_eq!(mixed.facts.ports.len(), 1);
    assert!(mixed.errors.iter().all(|error| !error.message.is_empty()));
}

#[test]
fn variables_are_reported_once_per_written_position() {
    let text = "x-env: &env\n  A: ${SHARED}\nx-base: &base\n  image: app:${TAG}\nservices:\n  a:\n    <<: *base\n    environment: *env\n    labels: {\"${KEY}\": \"$$KEY\"}\n    command: echo $$HOME ${A:+$HOMEDIR} $HOME ${X:?need X} $HOME\n  b:\n    <<: *base\n    environment: *env\n";
    let project = one(text);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    let unresolved: Vec<_> = project
        .unresolved
        .iter()
        .map(|u| (u.variable.as_str(), u.source.line, u.source.column))
        .collect();
    assert_eq!(
        unresolved,
        [
            ("SHARED", 1, 5),
            ("TAG", 3, 13),
            ("HOME", 9, 40),
            ("X", 9, 46),
            ("HOME", 9, 59),
        ]
    );
    let uses: Vec<_> = project
        .facts
        .variables
        .iter()
        .map(|use_| (use_.name.as_str(), &text[use_.bytes.clone()]))
        .collect();
    assert_eq!(
        uses,
        [
            ("SHARED", "${SHARED}"),
            ("TAG", "${TAG}"),
            ("HOME", "$HOME"),
            ("X", "${X:?need X}"),
            ("HOME", "$HOME"),
        ]
    );
    assert_eq!(
        project.model.services[1].image.as_deref(),
        Some("app:${TAG}")
    );
}

#[test]
fn a_reference_that_isnt_written_as_is_points_at_its_scalar() {
    // `\x24` is a `$` only once the double-quoted scalar is read.
    let text = "services:\n  db:\n    image: \"pg:\\x24{TAG}\"\n";
    let project = one(text);
    assert_eq!(
        project.model.services[0].image.as_deref(),
        Some("pg:${TAG}")
    );
    assert_eq!(project.unresolved[0].source, at("compose.yaml", 2, 11));
    assert_eq!(
        &text[project.facts.variables[0].bytes.clone()],
        "\"pg:\\x24{TAG}\""
    );
}

#[test]
fn a_bom_file_keeps_editor_positions() {
    let project = one("\u{feff}services:\n  db:\n    image: ${IMG}\n");
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    assert_eq!(project.model.services[0].source, at("compose.yaml", 1, 2));
    assert_eq!(project.unresolved[0].source, at("compose.yaml", 2, 11));
    let source = project.sources.get(0).unwrap();
    assert_eq!(
        &source.text[project.facts.variables[0].bytes.clone()],
        "${IMG}"
    );
}

#[test]
fn files_that_cant_be_read_or_parsed_are_load_errors() {
    let files = RecordingFiles::new(MemFiles::from([
        ("a.yml", "services:\n  web:\n    image: nginx\n"),
        ("b.yml", "services: [\n"),
    ]));
    let paths = ["a.yml", "b.yml", "c.yml", "../x.yml"].map(String::from);
    let project = load_project(&files, &Checkout::primary(), &paths);
    let names: Vec<_> = project
        .model
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["web"]);
    let errors: Vec<_> = project
        .errors
        .iter()
        .map(|error| (error.file.as_str(), error.source.is_some()))
        .collect();
    assert_eq!(
        errors,
        [("b.yml", true), ("c.yml", false), ("../x.yml", false)]
    );
    assert_eq!(project.errors[1].message, "no such file");
    assert!(project.errors[2].message.contains("../x.yml"));
    assert_eq!(files.reads(), ["a.yml", "b.yml", "c.yml"]);
    assert_eq!(project.model.files, ["a.yml", "b.yml"]);
    assert_eq!(project.facts.first_file.as_deref(), Some("a.yml"));
}

#[test]
fn odd_values_load_without_panicking() {
    for text in [
        "name: [x]\nservices:\n  a:\n    image: [x]\n    build: [x]\n    networks: 5\n    depends_on: {~: x}\n    links: {a: b}\n    profiles: x\n    container_name: {a: b}\n",
        "services:\n  a:\n    ports: [{target: [1]}, {target: 80, published: [1]}, \"\", \"${\", \"é:ü/ß\"]\n    volumes: [{type: 5}, {type: volume}, \"\", \":\", \"${A:-:/x}\"]\n",
        "services:\n  ? [a]\n  : {image: x}\n  b: {image: y}\nvolumes:\n  ? [v]\n  : {}\n  w: 5\nnetworks: [n]\n",
        "services:\n  a:\n    build: {context: [x], dockerfile: {}}\n    image: \"$\"\n    command: \"${A:+${B}} $$ $ ${ ${1} $é\"\n",
        "services: ~\nvolumes: ~\nnetworks: ~\nname: ~\n",
        "x: &x {a: *x}\n",
        "\u{feff}",
        "services:\r\n  db:\r\n    ports:\r\n      - \"${P}:5432\"\r\n",
    ] {
        let project = one(text);
        for use_ in &project.facts.variables {
            let source = project.sources.get(use_.file).unwrap();
            assert!(source.text.get(use_.bytes.clone()).is_some(), "{text:?}");
        }
        assert_eq!(project.unresolved.len(), project.facts.variables.len());
    }
    let keys = one("services:\n  ? [a]\n  : {image: x}\n  b: {image: y}\n");
    assert_eq!(keys.model.services.len(), 1);
    assert_eq!(keys.errors.len(), 1);
    let crlf = one("services:\r\n  db:\r\n    ports:\r\n      - \"${P}:5432\"\r\n");
    assert_eq!(crlf.unresolved[0].source, at("compose.yaml", 3, 9));
}

/// The project's service names, in model order.
fn service_names(project: &Project) -> Vec<&str> {
    project
        .model
        .services
        .iter()
        .map(|s| s.name.as_str())
        .collect()
}

/// Each error's file, and the line and column it points at. An error's source is in its own file.
fn errors_at(project: &Project) -> Vec<(&str, Option<(u32, u32)>)> {
    project
        .errors
        .iter()
        .map(|error| {
            if let Some(source) = &error.source {
                assert_eq!(source.file, error.file, "{error:?}");
            }
            let at = error.source.as_ref().map(|s| (s.line, s.column));
            (error.file.as_str(), at)
        })
        .collect()
}

/// Each port of a service as its host, and the file, line and column it is written at.
fn ports_of(service: &ServiceNode) -> Vec<(Option<&str>, &str, u32, u32)> {
    service
        .ports
        .iter()
        .map(|port| {
            let at = &port.source;
            (port.host.as_deref(), at.file.as_str(), at.line, at.column)
        })
        .collect()
}

fn context(service: &ServiceNode) -> Option<&str> {
    service.build.as_ref().map(|build| build.context.as_str())
}

#[test]
fn several_files_merge_in_order() {
    let first = "name: first\nservices:\n  web:\n    image: nginx:1\n    container_name: web-a\n    ports: [\"80:80\", \"443:443\"]\n    depends_on: [db]\n  db:\n    image: postgres:16\nvolumes:\n  data:\n    name: data-a\n";
    let later = "name: second\nservices:\n  web:\n    image: nginx:2\n    container_name: web-b\n    ports: [\"443:443\", \"8080:8080\"]\n    depends_on: [cache]\n    build: ./web\n  db:\n    image:\n  cache:\n    image: redis\nvolumes:\n  data:\n    external: true\n";
    let files = RecordingFiles::new(MemFiles::from([
        ("compose.yaml", first),
        ("infra/override.yaml", later),
    ]));
    let paths = ["compose.yaml", "infra/override.yaml", "./compose.yaml"].map(String::from);
    let project = load_project(&files, &Checkout::primary(), &paths);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    // The same normalized path is read once, and merged once.
    assert_eq!(files.reads(), ["compose.yaml", "infra/override.yaml"]);
    assert_eq!(project.model.files, ["compose.yaml", "infra/override.yaml"]);
    assert_eq!(service_names(&project), ["web", "db", "cache"]);
    let [web, db, cache] = project.model.services.as_slice() else {
        panic!("three services");
    };

    // The later image wins; ports concatenate without the exact duplicate.
    assert_eq!(web.image.as_deref(), Some("nginx:2"));
    assert_eq!(
        ports_of(web),
        [
            (Some("80"), "compose.yaml", 5, 12),
            (Some("443"), "compose.yaml", 5, 21),
            (Some("8080"), "infra/override.yaml", 5, 23),
        ]
    );
    assert_eq!(web.depends_on, ["db", "cache"]);
    assert_eq!(web.container_name.as_deref(), Some("web-b"));
    // A later file's relative path resolves against the first file's folder (Ruling 16).
    assert_eq!(context(web), Some("web"));
    // A service is where it is first defined.
    assert_eq!(web.source, at("compose.yaml", 2, 2));
    // An empty later value leaves the earlier one.
    assert_eq!(db.image.as_deref(), Some("postgres:16"));
    assert_eq!(cache.source, at("infra/override.yaml", 10, 2));
    assert_eq!(
        project.model.edges,
        [
            Edge {
                from: "web".into(),
                to: "db".into(),
                kind: EdgeKind::DependsOn
            },
            Edge {
                from: "web".into(),
                to: "cache".into(),
                kind: EdgeKind::DependsOn
            },
        ]
    );

    // A later name wins.
    assert_eq!(
        (
            project.model.project_name.value.as_str(),
            project.model.project_name.origin
        ),
        ("second", NameOrigin::Name)
    );
    assert_eq!(project.facts.name_entry.as_ref().unwrap().file, 1);

    // Facts come from the merged model: one per port, the overridden container_name once.
    let facts = &project.facts;
    let port_files: Vec<_> = facts.ports.iter().map(|port| port.file).collect();
    assert_eq!(port_files, [0, 0, 1]);
    assert_eq!(&later[facts.ports[2].bytes.clone()], "\"8080:8080\"");
    let [container_name] = facts.container_names.as_slice() else {
        panic!("one container_name");
    };
    assert_eq!(
        (
            container_name.file,
            &later[container_name.key.clone()],
            container_name.text.as_str()
        ),
        (1, "container_name", "web-b")
    );

    // Volumes merge key by key, each entry keeping its own file.
    let [data] = project.model.volumes.as_slice() else {
        panic!("one volume");
    };
    assert_eq!(
        (data.fixed_name.as_deref(), data.external, &data.source),
        (Some("data-a"), true, &at("compose.yaml", 10, 2))
    );
    let resource = &facts.resources[0];
    let name = resource.name_entry.as_ref().unwrap();
    let external = resource.external_entry.as_ref().unwrap();
    assert_eq!((resource.file, name.file, external.file), (0, 0, 1));
    assert_eq!(&first[name.value.clone()], "data-a");
    assert_eq!(&later[external.value.clone()], "true");
}

#[test]
fn include_paths_are_relative_to_the_including_file() {
    let files = RecordingFiles::new(MemFiles::from([
        (
            "infra/compose.yaml",
            "include:\n  - db/compose.yaml\n  - path: [../shared/cache.yml, ../shared/cache.local.yml]\n    project_directory: ..\n    env_file: ../.env\nservices:\n  web:\n    build: .\n    depends_on: [db, cache]\n",
        ),
        (
            "infra/db/compose.yaml",
            "include: [extra.yml]\nservices:\n  db:\n    build: .\n    volumes: [\"pgdata:/data\"]\nvolumes:\n  pgdata:\n",
        ),
        (
            "infra/db/extra.yml",
            "services:\n  backup:\n    image: backup:1\nnetworks:\n  backnet:\n",
        ),
        (
            "shared/cache.yml",
            "services:\n  cache:\n    image: redis:7\n    ports: [\"6379:6379\"]\n",
        ),
        (
            "shared/cache.local.yml",
            "services:\n  cache:\n    image: redis:8\n    build: ./img\n",
        ),
        (".env", "NEVER_READ=1\n"),
    ]));
    let project = load_project(&files, &Checkout::primary(), &["infra/compose.yaml".into()]);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    // A file's includes load before its own services resolve, each include's own includes first;
    // `env_file` is never read.
    let order = [
        "infra/compose.yaml",
        "infra/db/compose.yaml",
        "infra/db/extra.yml",
        "shared/cache.yml",
        "shared/cache.local.yml",
    ];
    assert_eq!(files.reads(), order);
    assert_eq!(project.model.files, order);
    // The including file's own services come first.
    assert_eq!(service_names(&project), ["web", "db", "backup", "cache"]);
    let [web, db, backup, cache] = project.model.services.as_slice() else {
        panic!("four services");
    };
    // Each included file resolves against its own folder; an include's later files against its
    // first one's, whatever `project_directory` says.
    assert_eq!(context(web), Some("infra"));
    assert_eq!(context(db), Some("infra/db"));
    assert_eq!(context(cache), Some("shared/img"));
    // An include's files merge like `-f`.
    assert_eq!(cache.image.as_deref(), Some("redis:8"));
    assert_eq!(ports_of(cache), [(Some("6379"), "shared/cache.yml", 3, 12)]);
    assert_eq!(db.source, at("infra/db/compose.yaml", 2, 2));
    assert_eq!(backup.source, at("infra/db/extra.yml", 1, 2));
    let names = |nodes: &[ResourceNode]| nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&project.model.volumes), ["pgdata"]);
    assert_eq!(names(&project.model.networks), ["backnet"]);
    assert_eq!(
        project.model.volumes[0].source,
        at("infra/db/compose.yaml", 6, 2)
    );
    // Included files never name the project.
    assert_eq!(project.model.project_name.value, "infra");
}

#[test]
fn include_cycles_and_missing_files_are_load_errors() {
    let files = RecordingFiles::new(MemFiles::from([
        (
            "compose.yaml",
            "include:\n  - a.yml\n  - missing.yml\n  - path: [a.yml]\n  - {path: {x: 1}}\n  - [x]\n  - dup.yml\nservices:\n  web:\n    image: web\nvolumes:\n  data:\n",
        ),
        (
            "a.yml",
            "include: [compose.yaml]\nservices:\n  a:\n    image: a\n",
        ),
        (
            "dup.yml",
            "services:\n  web:\n    image: other\n  d:\n    image: d\nvolumes:\n  data:\n",
        ),
    ]));
    let project = load_project(&files, &Checkout::primary(), &["compose.yaml".into()]);
    assert_eq!(
        errors_at(&project),
        [
            ("a.yml", Some((0, 10))),
            ("compose.yaml", Some((2, 4))),
            ("compose.yaml", Some((4, 11))),
            ("compose.yaml", Some((5, 4))),
            ("dup.yml", Some((1, 2))),
            ("dup.yml", Some((6, 2))),
        ]
    );
    let messages: Vec<_> = project.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages[0],
        "include cycle: compose.yaml → a.yml → compose.yaml; Compose refuses this"
    );
    assert_eq!(messages[1], "can't include missing.yml: no such file");
    assert_eq!(
        messages[4],
        "web is defined in both compose.yaml and dup.yml; Compose refuses this"
    );
    assert_eq!(
        messages[5],
        "data is defined in both compose.yaml and dup.yml; Compose refuses this"
    );
    // The rest loads: the first definition stays, and a file included twice is read once and
    // defines its services once.
    assert_eq!(service_names(&project), ["web", "a", "d"]);
    assert_eq!(project.model.services[0].image.as_deref(), Some("web"));
    assert_eq!(project.model.volumes.len(), 1);
    assert_eq!(
        files.reads(),
        ["compose.yaml", "a.yml", "missing.yml", "dup.yml"]
    );
}

#[test]
fn nesting_stops_at_twenty_levels() {
    // f0.yml includes f1.yml, which includes f2.yml, and so on.
    let chain = (0..22).map(|i| {
        let text = format!(
            "include: [f{}.yml]\nservices:\n  s{i}:\n    image: s{i}\n",
            i + 1
        );
        (format!("f{i}.yml"), text)
    });
    let files = RecordingFiles::new(MemFiles(chain.collect()));
    let project = load_project(&files, &Checkout::primary(), &["f0.yml".into()]);
    // The file and twenty levels of includes load; the include past them is an error.
    assert_eq!(files.reads().len(), 21);
    assert_eq!(project.model.services.len(), 21);
    assert_eq!(errors_at(&project), [("f20.yml", Some((0, 10)))]);

    // e0 extends e1, which extends e2, and so on to e21.
    let mut text = String::from("services:\n");
    for i in 0..22 {
        text += &format!("  e{i}:\n");
        if i < 21 {
            text += &format!("    extends: e{}\n", i + 1);
        }
        text += &format!("    ports: [\"{}:80\"]\n", 1000 + i);
    }
    let project = one(&text);
    // e0 inherits through twenty levels, e1 to e20; e20's own extends is the error.
    assert_eq!(errors_at(&project), [("compose.yaml", Some((62, 13)))]);
    let [e0, e1, ..] = project.model.services.as_slice() else {
        panic!("22 services");
    };
    assert_eq!(e0.ports.len(), 21);
    assert_eq!(e0.ports[0].host.as_deref(), Some("1020"));
    assert_eq!(e1.ports.len(), 21);
    assert_eq!(e1.ports[0].host.as_deref(), Some("1021"));
}

#[test]
fn extends_merges_the_base_under_the_service() {
    let files = RecordingFiles::new(MemFiles::from([
        (
            "compose.yaml",
            "services:\n  api:\n    extends:\n      file: common/base.yml\n      service: base\n    image: api:1\n    ports: [\"8000:8000\"]\n  worker:\n    extends: api\n    command: work\n",
        ),
        (
            "common/base.yml",
            "services:\n  base:\n    extends: root\n    image: app:dev\n    build: .\n    ports: [\"9000:9000\"]\n  root:\n    ports: [\"7000:7000\"]\n    depends_on: [db]\n",
        ),
    ]));
    let project = load_project(&files, &Checkout::primary(), &["compose.yaml".into()]);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    assert_eq!(files.reads(), ["compose.yaml", "common/base.yml"]);
    assert_eq!(project.model.files, ["compose.yaml", "common/base.yml"]);
    // The base file's services aren't the project's.
    assert_eq!(service_names(&project), ["api", "worker"]);
    let [api, worker] = project.model.services.as_slice() else {
        panic!("two services");
    };

    // The service's own keys win, and what it inherits keeps where it is written.
    assert_eq!(api.image.as_deref(), Some("api:1"));
    let inherited = [
        (Some("7000"), "common/base.yml", 7, 12),
        (Some("9000"), "common/base.yml", 5, 12),
        (Some("8000"), "compose.yaml", 6, 12),
    ];
    assert_eq!(ports_of(api), inherited);
    assert_eq!(api.depends_on, ["db"]);
    // The extended file's relative paths resolve against its own folder.
    assert_eq!(context(api), Some("common"));
    assert_eq!(api.source, at("compose.yaml", 1, 2));

    // A service extends another of its own file, with what that one inherits.
    assert_eq!(worker.image.as_deref(), Some("api:1"));
    assert_eq!(ports_of(worker), inherited);
    assert_eq!(worker.source, at("compose.yaml", 7, 2));
    let port_files: Vec<_> = project.facts.ports.iter().map(|port| port.file).collect();
    assert_eq!(port_files, [1, 1, 0, 1, 1, 0]);
}

#[test]
fn extends_cycles_are_load_errors() {
    let files = RecordingFiles::new(MemFiles::from([
        (
            "compose.yaml",
            "services:\n  a:\n    extends: b\n    image: a\n  b:\n    extends: a\n  self:\n    extends: self\n  lost:\n    extends: nowhere\n  far:\n    extends: {file: other.yml, service: x}\n  gone:\n    extends: {file: missing.yml, service: x}\n  odd:\n    extends: [a]\n  ok:\n    image: ok\n",
        ),
        (
            "other.yml",
            "services:\n  x:\n    extends: {file: compose.yaml, service: far}\n",
        ),
    ]));
    let project = load_project(&files, &Checkout::primary(), &["compose.yaml".into()]);
    assert_eq!(
        errors_at(&project),
        [
            ("compose.yaml", Some((5, 13))),
            ("compose.yaml", Some((2, 13))),
            ("compose.yaml", Some((7, 13))),
            ("compose.yaml", Some((9, 13))),
            ("other.yml", Some((2, 43))),
            ("compose.yaml", Some((13, 20))),
            ("compose.yaml", Some((15, 13))),
        ]
    );
    let messages: Vec<_> = project.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages[0],
        "extends cycle: a → b → a; Compose refuses this"
    );
    assert_eq!(
        messages[2],
        "extends cycle: self → self; Compose refuses this"
    );
    assert_eq!(
        messages[3],
        "lost extends nowhere, which compose.yaml doesn't define"
    );
    assert_eq!(
        messages[4],
        "extends cycle: far → x → far; Compose refuses this"
    );
    assert_eq!(messages[5], "can't extend x from missing.yml: no such file");
    // Every service still loads, with its own keys.
    assert_eq!(
        service_names(&project),
        ["a", "b", "self", "lost", "far", "gone", "odd", "ok"]
    );
    assert_eq!(project.model.services[0].image.as_deref(), Some("a"));
    assert_eq!(files.reads(), ["compose.yaml", "other.yml", "missing.yml"]);
}

#[test]
fn paths_outside_the_checkout_are_refused() {
    let files = RecordingFiles::new(MemFiles::from([(
        "compose.yaml",
        "include: [../../x.yml]\nservices:\n  web:\n    extends: {file: /etc/base.yml, service: base}\n    image: web\n  api:\n    extends: {file: ../base.yml, service: base}\n",
    )]));
    let paths = ["compose.yaml", "../y.yml"].map(String::from);
    let project = load_project(&files, &Checkout::primary(), &paths);
    assert_eq!(
        errors_at(&project),
        [
            ("compose.yaml", Some((0, 10))),
            ("compose.yaml", Some((3, 20))),
            ("compose.yaml", Some((6, 20))),
            ("../y.yml", None),
        ]
    );
    let messages: Vec<_> = project.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages[0], "../../x.yml leaves the checkout");
    assert!(messages[1].contains("/etc/base.yml"), "{}", messages[1]);
    assert_eq!(messages[2], "../base.yml leaves the checkout");
    // Nothing outside is read, and the rest loads.
    assert_eq!(files.reads(), ["compose.yaml"]);
    assert_eq!(service_names(&project), ["web", "api"]);
    assert_eq!(project.model.services[0].image.as_deref(), Some("web"));
}

#[test]
fn a_checkout_prefixes_reads_but_not_reported_paths() {
    let files = RecordingFiles::new(MemFiles::from([
        (
            ".jensen/worktrees/a/compose.yaml",
            "include: [db/compose.yaml]\nservices:\n  web:\n    extends: {file: common.yml, service: base}\n    build: ./web\n    image: web:${TAG}\n",
        ),
        (
            ".jensen/worktrees/a/db/compose.yaml",
            "services:\n  db:\n    image: postgres:16\n    ports: [\"5432:5432\"]\n    volumes: [{type: volume}]\n",
        ),
        (
            ".jensen/worktrees/a/common.yml",
            "services:\n  base:\n    ports: [\"8080:80\"]\n",
        ),
        (
            "compose.yaml",
            "services:\n  primary:\n    image: primary\n",
        ),
    ]));
    let checkout = Checkout::parse(Some(".jensen/worktrees/a")).unwrap();
    let project = load_project(&files, &checkout, &["compose.yaml".into()]);
    assert_eq!(
        files.reads(),
        [
            ".jensen/worktrees/a/compose.yaml",
            ".jensen/worktrees/a/db/compose.yaml",
            ".jensen/worktrees/a/common.yml",
        ]
    );
    assert_eq!(
        project.model.files,
        ["compose.yaml", "db/compose.yaml", "common.yml"]
    );
    assert_eq!(service_names(&project), ["web", "db"]);
    let [web, db] = project.model.services.as_slice() else {
        panic!("two services");
    };
    assert_eq!(web.source, at("compose.yaml", 2, 2));
    assert_eq!(ports_of(web), [(Some("8080"), "common.yml", 2, 12)]);
    assert_eq!(context(web), Some("web"));
    assert_eq!(ports_of(db), [(Some("5432"), "db/compose.yaml", 3, 12)]);
    assert_eq!(project.unresolved[0].source, at("compose.yaml", 5, 15));
    assert_eq!(errors_at(&project), [("db/compose.yaml", Some((4, 14)))]);
    assert_eq!(
        (
            project.model.project_name.value.as_str(),
            project.model.project_name.origin
        ),
        ("a", NameOrigin::Folder)
    );
    assert_eq!(project.facts.first_file.as_deref(), Some("compose.yaml"));
}

#[test]
fn one_broken_file_does_not_sink_the_others() {
    let files = MemFiles::from([
        ("a.yml", "services:\n  web:\n    image: nginx\n"),
        ("b.yml", "services: [\n"),
    ]);
    let project = load_project(
        &files,
        &Checkout::primary(),
        &["a.yml".into(), "b.yml".into(), "c.yml".into()],
    );
    assert_eq!(
        project
            .model
            .services
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["web"]
    );
    assert_eq!(
        project
            .errors
            .iter()
            .map(|e| e.file.as_str())
            .collect::<Vec<_>>(),
        ["b.yml", "c.yml"]
    );
}

#[test]
fn a_name_written_twice_keeps_the_first() {
    let project = one("services:\n  a:\n    image: x\n  a:\n    image: y\nvolumes:\n  v:\n  v:\n");
    assert_eq!(
        errors_at(&project),
        [
            ("compose.yaml", Some((3, 2))),
            ("compose.yaml", Some((7, 2)))
        ]
    );
    assert_eq!(
        project.errors[0].message,
        "a is defined twice in compose.yaml; Compose refuses this"
    );
    assert_eq!(service_names(&project), ["a"]);
    assert_eq!(project.model.services[0].image.as_deref(), Some("x"));
    assert_eq!(project.model.volumes.len(), 1);
}

#[test]
fn includes_reached_twice_load_once() {
    // Twenty levels of two files, each including both files of the next level: 2^20 ways down.
    let mut texts = vec![(
        "compose.yaml".to_string(),
        "include: [a0.yml, b0.yml]\n".to_string(),
    )];
    for level in 0..20 {
        for side in ["a", "b"] {
            let include = if level < 19 {
                format!("include: [a{0}.yml, b{0}.yml]\n", level + 1)
            } else {
                String::new()
            };
            let text = format!("{include}services:\n  {side}{level}:\n    image: x\n");
            texts.push((format!("{side}{level}.yml"), text));
        }
    }
    let files = RecordingFiles::new(MemFiles(texts.into_iter().collect()));
    let project = load_project(&files, &Checkout::primary(), &["compose.yaml".into()]);
    assert!(project.errors.is_empty(), "{:?}", project.errors);
    assert_eq!(files.reads().len(), 41);
    // Each service once, though it is reached many ways.
    let mut names = service_names(&project);
    names.sort_unstable();
    names.dedup();
    assert_eq!((names.len(), project.model.services.len()), (40, 40));
}

#[test]
fn copies_stop_at_the_node_cap() {
    // Each service extending `base` copies its 2,000 entries.
    let mut text = String::from("services:\n  base:\n    environment:\n");
    for i in 0..2000 {
        text += &format!("      - V{i}=1\n");
    }
    for i in 0..150 {
        text += &format!("  s{i}:\n    extends: base\n");
    }
    let project = one(&text);
    let refused = project.errors.len();
    assert!((1..150).contains(&refused), "{refused}");
    assert!(
        project
            .errors
            .iter()
            .all(|error| error.message.starts_with("the project is too large")),
        "{:?}",
        project.errors.first()
    );
    // The services still load, with their own keys.
    assert_eq!(project.model.services.len(), 151);

    // A file included again is copied, and counts the same way.
    let mut big = String::from("services:\n  big:\n    environment:\n");
    for i in 0..2000 {
        big += &format!("      - V{i}=1\n");
    }
    let include = format!("include:\n{}", "  - big.yml\n".repeat(150));
    let files = RecordingFiles::new(MemFiles::from([
        ("compose.yaml", include.as_str()),
        ("big.yml", big.as_str()),
    ]));
    let project = load_project(&files, &Checkout::primary(), &["compose.yaml".into()]);
    assert_eq!(files.reads(), ["compose.yaml", "big.yml"]);
    assert_eq!(service_names(&project), ["big"]);
    let refused = project.errors.len();
    assert!((1..150).contains(&refused), "{refused}");
    assert!(
        project.errors[0]
            .message
            .contains("big.yml isn't merged in")
    );
}

#[test]
fn odd_includes_and_extends_load_without_panicking() {
    for text in [
        "include: x.yml\nservices: {}\n",
        "include: [~, \"\", \".\", {path: []}, {path: [a.yml, ~]}, {env_file: x}, \"${DIR}/x.yml\"]\n",
        "services:\n  a:\n    extends: \"\"\n  b:\n    extends: {service: a, file: \"\"}\n  c:\n    extends: {service: a, file: [x]}\n  d:\n    extends: {file: x.yml}\n  e:\n    extends: ~\n  f: {extends: g}\n  g: x\n  h:\n    extends: {service: [a]}\n",
        "include: [compose.yaml]\n",
        "services:\n  a:\n    extends: a\n    build: [x]\n",
        "services:\n  a:\n    build: .\n  b:\n    extends: a\n    build: {dockerfile: D}\n  c:\n    extends: b\n    build: ctx\n",
    ] {
        let project = one(text);
        assert!(
            project.errors.iter().all(|e| !e.message.is_empty()),
            "{text:?}"
        );
        // Every error's source is in its own file.
        errors_at(&project);
        assert_eq!(project.unresolved.len(), project.facts.variables.len());
    }
    let errors = |text| one(text).errors.len();
    assert_eq!(errors("include: x.yml\nservices: {}\n"), 1);
    assert_eq!(
        errors(
            "include: [~, \"\", \".\", {path: []}, {path: [a.yml, ~]}, {env_file: x}, \"${DIR}/x.yml\"]\n"
        ),
        7
    );
    assert_eq!(
        errors(
            "services:\n  a:\n    extends: \"\"\n  b:\n    extends: {service: a, file: \"\"}\n  c:\n    extends: {service: a, file: [x]}\n  d:\n    extends: {file: x.yml}\n  e:\n    extends: ~\n  f: {extends: g}\n  g: x\n  h:\n    extends: {service: [a]}\n"
        ),
        8
    );
    // A `build: <context>` and a build mapping merge key by key.
    let builds = one(
        "services:\n  a:\n    build: ./app\n  b:\n    extends: a\n    build: {dockerfile: D}\n  c:\n    extends: b\n    build: ctx\n",
    );
    let build = |at: usize| builds.model.services[at].build.clone().unwrap();
    assert_eq!(
        (build(1).context.as_str(), build(1).dockerfile.as_deref()),
        ("app", Some("D"))
    );
    assert_eq!(
        (build(2).context.as_str(), build(2).dockerfile.as_deref()),
        ("ctx", Some("D"))
    );
}
