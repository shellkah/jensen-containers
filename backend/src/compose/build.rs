//! Turns the services, volumes and networks the files define into the model and the facts. Every
//! position comes from its node's own file, so a merged service keeps where each value is written.

use std::collections::HashSet;

use crate::interpolate::interpolate;
use crate::model::{
    Build, Edge, EdgeKind, LoadError, MountKind, NameOrigin, PortBinding, ProjectModel,
    ProjectName, ResourceNode, ServiceNode, SourceRef, Span, VolumeMount,
};
use crate::paths::{Checkout, compose_project_name, join, parent};
use crate::short_syntax::{mount_kind, split_port, split_volume};
use crate::source::{FileId, Sources};
use crate::yaml::{Entry, Node, Value};

use super::load::{Defined, Loaded};
use super::{
    EntryFact, Facts, PortFact, PortForm, Project, ResourceFact, ResourceKind, error_at,
    interpolated, position, value,
};

/// The project the loaded files define.
pub(super) fn project(loaded: Loaded, checkout: &Checkout) -> Project {
    let Loaded {
        sources,
        services,
        volumes,
        networks,
        name,
        first_file,
        errors,
        unresolved,
        variables,
    } = loaded;
    let mut builder = Builder {
        sources: &sources,
        errors,
        facts: Facts::default(),
    };
    let services: Vec<_> = services
        .iter()
        .filter_map(|service| builder.service(service))
        .collect();
    let volumes = builder.resources(ResourceKind::Volume, &volumes);
    let networks = builder.resources(ResourceKind::Network, &networks);
    let name =
        name.and_then(|entry| Some((value(Some(&entry.value))?, builder.entry_fact(None, &entry))));
    let Builder { errors, facts, .. } = builder;

    let edges = edges(&services);
    let (project_name, name_entry) = match name {
        Some((value, entry)) => (
            ProjectName {
                value,
                origin: NameOrigin::Name,
            },
            Some(entry),
        ),
        None => (
            ProjectName {
                value: folder_name(first_file.as_deref(), checkout),
                origin: NameOrigin::Folder,
            },
            None,
        ),
    };
    Project {
        model: ProjectModel {
            project_name,
            files: sources.paths(),
            services: services.into_iter().map(|(service, _)| service).collect(),
            volumes,
            networks,
            edges,
        },
        unresolved,
        errors,
        sources,
        facts: Facts {
            name_entry,
            first_file,
            variables,
            ..facts
        },
    }
}

/// The project name Compose derives from the first file's folder. A file at the checkout root
/// takes the checkout's folder name, which the backend can't see in the primary checkout.
fn folder_name(first_file: Option<&str>, checkout: &Checkout) -> String {
    let folder = first_file.map_or("", parent);
    let name = if folder.is_empty() {
        checkout.folder_name()
    } else {
        folder.rsplit('/').next()
    };
    name.map(compose_project_name).unwrap_or_default()
}

/// Service by service: `depends_on`, then links, then the networks it names, then its named
/// volumes, each edge once.
fn edges(services: &[(ServiceNode, Vec<String>)]) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for (service, links) in services {
        let volumes = service
            .volumes
            .iter()
            .filter(|mount| mount.kind == MountKind::Named)
            .filter_map(|mount| mount.source.as_ref());
        let targets = service
            .depends_on
            .iter()
            .map(|to| (to, EdgeKind::DependsOn))
            .chain(links.iter().map(|to| (to, EdgeKind::Links)))
            .chain(service.networks.iter().map(|to| (to, EdgeKind::Network)))
            .chain(volumes.map(|to| (to, EdgeKind::Volume)));
        for (to, kind) in targets {
            if seen.insert((service.name.as_str(), to.as_str(), kind as u8)) {
                edges.push(Edge {
                    from: service.name.clone(),
                    to: to.clone(),
                    kind,
                });
            }
        }
    }
    edges
}

/// Builds the model's parts, with the facts and the errors it finds.
struct Builder<'s> {
    sources: &'s Sources,
    errors: Vec<LoadError>,
    facts: Facts,
}

impl Builder<'_> {
    fn source_ref(&self, node: &Node) -> SourceRef {
        position(self.sources, node.file, node.span.start)
    }

    fn span(&self, node: &Node) -> Span {
        match self.sources.get(node.file) {
            Some(source) => source.span(node.span.clone()),
            None => Span { from: 0, to: 0 },
        }
    }

    fn slice(&self, file: FileId, bytes: std::ops::Range<usize>) -> &str {
        self.sources
            .get(file)
            .map_or("", |source| source.slice(bytes))
    }

    /// An error at the start of `node`.
    fn error(&mut self, node: &Node, message: String) {
        let error = error_at(self.sources, node.file, node.span.start, message);
        self.errors.push(error);
    }

    /// An entry's key and value bytes, the value ending at its own text, and the value interpolated
    /// (a collection's text as written).
    fn entry_fact(&self, service: Option<&str>, entry: &Entry) -> EntryFact {
        let value = entry.value.span.start..entry.value.last_byte();
        let text = match entry.value.as_str() {
            Some(text) => interpolate(text).value,
            None => self.slice(entry.value.file, value.clone()).to_owned(),
        };
        EntryFact {
            service: service.map(str::to_owned),
            file: entry.key.file,
            key: entry.key.span.clone(),
            value,
            text,
        }
    }

    fn service(&mut self, defined: &Defined) -> Option<(ServiceNode, Vec<String>)> {
        let name = defined.name.as_str();
        let node = &defined.node;
        if !matches!(node.value, Value::Map(_)) {
            self.error(
                node,
                format!("service {name} must be a mapping of its settings"),
            );
            return None;
        }
        let ports = self.ports(name, node.get("ports"));
        let volumes = self.volumes(name, node.get("volumes"));
        let mut container_name = None;
        if let Some(entry) = node.entry("container_name")
            && let Some(text) = value(Some(&entry.value))
        {
            let fact = self.entry_fact(Some(name), entry);
            self.facts.container_names.push(fact);
            container_name = Some(text);
        }
        let links = list(node.get("links"))
            .into_iter()
            .map(|link| match link.split_once(':') {
                Some((to, _)) => to.to_owned(),
                None => link,
            })
            .filter(|to| !to.is_empty())
            .collect();
        let service = ServiceNode {
            name: name.to_owned(),
            image: value(node.get("image")),
            build: build(node.get("build"), &defined.folder),
            ports,
            volumes,
            networks: names(node.get("networks")),
            depends_on: names(node.get("depends_on")),
            container_name,
            profiles: list(node.get("profiles")),
            source: self.source_ref(&defined.key),
        };
        Some((service, links))
    }

    fn ports(&mut self, service: &str, node: Option<&Node>) -> Vec<PortBinding> {
        let Some(node) = node else {
            return Vec::new();
        };
        match &node.value {
            Value::Seq(items) => items
                .iter()
                .filter_map(|item| self.port(service, item))
                .collect(),
            Value::Null => Vec::new(),
            Value::Scalar(_) | Value::Map(_) => {
                self.error(node, format!("ports of {service} must be a list"));
                Vec::new()
            }
        }
    }

    /// A short port is split as written, then each segment is interpolated; a mapping is the long
    /// syntax.
    fn port(&mut self, service: &str, node: &Node) -> Option<PortBinding> {
        let error = match &node.value {
            Value::Scalar(text) => match split_port(text) {
                Some(parts) => {
                    let form = PortForm::Short(parts.clone());
                    let written = Written {
                        ip: parts.ip.as_deref(),
                        host: parts.host.as_deref(),
                        container: &parts.container,
                        protocol: parts.protocol.as_deref(),
                    };
                    return Some(self.publish(service, written, node, form));
                }
                None => format!(
                    "port {text} of {service} isn't [ip:][host:]container[/protocol], such as 8080:80"
                ),
            },
            Value::Map(_) => {
                let scalar = |key| node.get(key).filter(|value| value.as_str().is_some());
                if let Some(target) = scalar("target") {
                    let published = scalar("published");
                    let written = Written {
                        ip: scalar("host_ip").and_then(Node::as_str),
                        host: published.and_then(Node::as_str),
                        container: target.as_str().unwrap_or_default(),
                        protocol: scalar("protocol").and_then(Node::as_str),
                    };
                    let at = published.unwrap_or(target);
                    return Some(self.publish(service, written, at, PortForm::Long));
                }
                format!("a port mapping of {service} needs a target")
            }
            Value::Null | Value::Seq(_) => format!(
                "a port of {service} must be text such as 8080:80, a number, or a mapping with a target"
            ),
        };
        self.error(node, error);
        None
    }

    /// The binding of a port written at `at`, and its fact. A host port is fixed when it is there,
    /// isn't empty, and uses no variable: a variable, even with a default, lets each copy of the
    /// stack choose its own.
    fn publish(
        &mut self,
        service: &str,
        written: Written<'_>,
        at: &Node,
        form: PortForm,
    ) -> PortBinding {
        let host = written.host.map(interpolate);
        let fixed = host
            .as_ref()
            .is_some_and(|host| !host.value.is_empty() && !host.uses_variables);
        let host = host.map(|host| host.value).filter(|host| !host.is_empty());
        let binding = PortBinding {
            host_ip: interpolated(written.ip),
            host: host.clone(),
            container: interpolate(written.container).value,
            protocol: interpolated(written.protocol),
            fixed,
            source: self.source_ref(at),
            span: self.span(at),
        };
        self.facts.ports.push(PortFact {
            service: service.to_owned(),
            file: at.file,
            bytes: at.span.clone(),
            form,
            host,
            fixed,
        });
        binding
    }

    fn volumes(&mut self, service: &str, node: Option<&Node>) -> Vec<VolumeMount> {
        let Some(node) = node else {
            return Vec::new();
        };
        match &node.value {
            Value::Seq(items) => items
                .iter()
                .filter_map(|item| self.volume(service, item))
                .collect(),
            Value::Null => Vec::new(),
            Value::Scalar(_) | Value::Map(_) => {
                self.error(node, format!("volumes of {service} must be a list"));
                Vec::new()
            }
        }
    }

    /// A short volume is interpolated, then split; a mapping is the long syntax, whose types other
    /// than `volume`, `bind` and `tmpfs` are skipped.
    fn volume(&mut self, service: &str, node: &Node) -> Option<VolumeMount> {
        let mount = match &node.value {
            Value::Scalar(text) => short_volume(text).map(Some).ok_or_else(|| {
                format!(
                    "volume {text} of {service} isn't [source:]target[:mode], such as data:/data"
                )
            }),
            Value::Map(_) => long_volume(node)
                .ok_or_else(|| format!("a volume mapping of {service} needs a type and a target")),
            Value::Null | Value::Seq(_) => Err(format!(
                "a volume of {service} must be text such as data:/data, or a mapping with a type and a target"
            )),
        };
        mount.unwrap_or_else(|message| {
            self.error(node, message);
            None
        })
    }

    /// Top-level volumes or networks: each a resource, and a fact when it sets `name:` or is
    /// external.
    fn resources(&mut self, kind: ResourceKind, defined: &[Defined]) -> Vec<ResourceNode> {
        let mut resources = Vec::new();
        for resource in defined {
            let settings = &resource.node;
            let name_entry = settings
                .entry("name")
                .filter(|entry| value(Some(&entry.value)).is_some());
            // `external: true`, or the old `external: { name: ... }`, whose name counts too.
            let external_entry = settings.entry("external").filter(|entry| {
                matches!(entry.value.value, Value::Map(_)) || is_true(&entry.value)
            });
            let fixed_name = name_entry
                .and_then(|entry| value(Some(&entry.value)))
                .or_else(|| external_entry.and_then(|entry| value(entry.value.get("name"))));
            if name_entry.is_some() || external_entry.is_some() {
                let fact = ResourceFact {
                    kind,
                    name: resource.name.clone(),
                    file: resource.key.file,
                    name_entry: name_entry.map(|entry| self.entry_fact(None, entry)),
                    external_entry: external_entry.map(|entry| self.entry_fact(None, entry)),
                };
                self.facts.resources.push(fact);
            }
            resources.push(ResourceNode {
                name: resource.name.clone(),
                fixed_name,
                external: external_entry.is_some(),
                source: self.source_ref(&resource.key),
            });
        }
        resources
    }
}

/// A port's segments as written, before interpolation.
struct Written<'s> {
    ip: Option<&'s str>,
    host: Option<&'s str>,
    container: &'s str,
    protocol: Option<&'s str>,
}

/// `build: <context>`, or a mapping with `context` (`.` when left out) and `dockerfile`. The
/// context resolves against `folder`.
fn build(node: Option<&Node>, folder: &str) -> Option<Build> {
    let node = node?;
    let (context, dockerfile) = match &node.value {
        Value::Scalar(text) => (interpolate(text).value, None),
        Value::Map(_) => (
            value(node.get("context")).unwrap_or_else(|| ".".to_owned()),
            value(node.get("dockerfile")),
        ),
        Value::Null | Value::Seq(_) => return None,
    };
    Some(Build {
        context: context_folder(folder, context),
        dockerfile,
    })
}

/// A build context as a folder relative to the checkout root, `.` for the root itself. A URL, or a
/// folder outside the checkout, stays as written.
fn context_folder(folder: &str, context: String) -> String {
    if is_remote(&context) {
        return context;
    }
    match join(folder, &context) {
        Ok(joined) if joined.is_empty() => ".".to_owned(),
        Ok(joined) => joined,
        Err(_) => context,
    }
}

/// A list's scalar items, interpolated.
fn list(node: Option<&Node>) -> Vec<String> {
    node.map_or(&[][..], Node::items)
        .iter()
        .filter_map(|item| value(Some(item)))
        .collect()
}

/// The names a list holds, interpolated, or a mapping's keys, as written: keys aren't interpolated.
fn names(node: Option<&Node>) -> Vec<String> {
    match node.map(|node| &node.value) {
        Some(Value::Map(entries)) => entries
            .iter()
            .filter_map(|entry| entry.key.as_str())
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => list(node),
    }
}

fn is_true(node: &Node) -> bool {
    value(Some(node)).is_some_and(|value| matches!(value.as_str(), "true" | "True" | "TRUE"))
}

/// A build context Compose fetches instead of reading a folder: Compose's own prefixes.
fn is_remote(context: &str) -> bool {
    [
        "https://",
        "http://",
        "git://",
        "ssh://",
        "github.com/",
        "git@",
    ]
    .iter()
    .any(|prefix| context.starts_with(prefix))
}

fn short_volume(text: &str) -> Option<VolumeMount> {
    let parts = split_volume(&interpolate(text).value)?;
    let read_only = parts
        .mode
        .as_deref()
        .is_some_and(|mode| mode.split(',').any(|option| option == "ro"));
    Some(VolumeMount {
        kind: mount_kind(parts.source.as_deref()),
        source: parts.source,
        target: parts.target,
        read_only,
    })
}

/// The long syntax: `Some(None)` for a type that isn't a volume, a bind or a tmpfs, `None` without
/// a type or a target.
fn long_volume(node: &Node) -> Option<Option<VolumeMount>> {
    let kind = match value(node.get("type"))?.as_str() {
        "volume" => MountKind::Named,
        "bind" => MountKind::Bind,
        "tmpfs" => MountKind::Tmpfs,
        _ => return Some(None),
    };
    Some(Some(VolumeMount {
        kind,
        source: value(node.get("source")),
        target: value(node.get("target"))?,
        read_only: node.get("read_only").is_some_and(is_true),
    }))
}
