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
