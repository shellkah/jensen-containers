//! Loads Compose files into the project model, with the facts the readiness rules read.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use crate::files::ReadFile;
use crate::interpolate::interpolate;
use crate::model::{
    Build, Edge, EdgeKind, LoadError, MountKind, NameOrigin, PortBinding, ProjectModel,
    ProjectName, ResourceNode, ServiceNode, Unresolved, VolumeMount,
};
use crate::paths::{Checkout, compose_project_name, join, normalize, parent};
use crate::short_syntax::{PortParts, mount_kind, split_port, split_volume};
use crate::source::{FileId, SourceText, Sources};
use crate::yaml::{self, Entry, Node, Value};

/// What loading a project's Compose files gives: the model, the variables nothing provides, what
/// couldn't be loaded, the files' texts, and the facts the readiness rules are written against.
#[derive(Clone, Debug)]
pub struct Project {
    pub model: ProjectModel,
    pub unresolved: Vec<Unresolved>,
    pub errors: Vec<LoadError>,
    pub sources: Sources,
    pub facts: Facts,
}

/// Where the things the readiness rules judge are written. Plan 2's rules are written against this
/// shape: change it only together with them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// One per ports entry of every service, in model order.
    pub ports: Vec<PortFact>,
    /// The services that set `container_name`.
    pub container_names: Vec<EntryFact>,
    /// Top-level volumes and networks with `name:` or `external:`.
    pub resources: Vec<ResourceFact>,
    /// The top-level `name:` that set the project name.
    pub name_entry: Option<EntryFact>,
    /// The first Compose file, relative to the checkout root.
    pub first_file: Option<String>,
    /// Each use of a variable with no default, in `unresolved`'s order.
    pub variables: Vec<VariableUse>,
}

/// A `key: value` entry: the key's bytes, the value's from its first byte to its `last_byte()`, and
/// the value interpolated (a collection's text as written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryFact {
    pub service: Option<String>,
    pub file: FileId,
    pub key: Range<usize>,
    pub value: Range<usize>,
    pub text: String,
}

/// A published port. `bytes` covers the short syntax's scalar with its quotes, or the long
/// syntax's `published` value (its `target` value when there is no `published`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortFact {
    pub service: String,
    pub file: FileId,
    pub bytes: Range<usize>,
    pub form: PortForm,
    pub host: Option<String>,
    pub fixed: bool,
}

/// How a port is written: short syntax, with its parts as written, or a long-syntax mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortForm {
    Short(PortParts),
    Long,
}

/// A top-level volume or network that sets `name:` or is `external:`, with those entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceFact {
    pub kind: ResourceKind,
    pub name: String,
    pub file: FileId,
    pub name_entry: Option<EntryFact>,
    pub external_entry: Option<EntryFact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Volume,
    Network,
}

/// A use of a variable with no default: its name and the bytes of its reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableUse {
    pub name: String,
    pub file: FileId,
    pub bytes: Range<usize>,
}

/// Loads each Compose file of `paths`, relative to the checkout root, read under `checkout`. Each
/// file loads on its own. A path that leaves the checkout, a file that can't be read, and a part
/// of a file that has the wrong shape are [`LoadError`]s, and the rest still loads.
pub fn load_project(files: &dyn ReadFile, checkout: &Checkout, paths: &[String]) -> Project {
    let mut sources = Sources::default();
    let mut loaded = Loaded::default();
    let mut first_file = None;
    for written in paths {
        let path = match normalize(written) {
            Ok(path) => path,
            Err(message) => {
                loaded.errors.push(LoadError {
                    file: written.clone(),
                    message,
                    source: None,
                });
                continue;
            }
        };
        first_file.get_or_insert_with(|| path.clone());
        match files.read(&checkout.read_path(&path)) {
            Ok(text) => {
                // The source drops a leading byte order mark, so its text is what gets parsed.
                let file = sources.add(SourceText::new(path, text));
                if let Some(source) = sources.get(file) {
                    FileLoader {
                        source,
                        file,
                        out: &mut loaded,
                    }
                    .load();
                }
            }
            Err(message) => loaded.errors.push(LoadError {
                file: path,
                message,
                source: None,
            }),
        }
    }
    loaded.finish(sources, checkout, first_file)
}

/// What the files have given so far.
#[derive(Default)]
struct Loaded {
    /// Each service, with its links: the service names before their `:`.
    services: Vec<(ServiceNode, Vec<String>)>,
    volumes: Vec<ResourceNode>,
    networks: Vec<ResourceNode>,
    /// The project name a top-level `name:` sets, and that entry; a later file's wins.
    name: Option<(String, EntryFact)>,
    unresolved: Vec<Unresolved>,
    errors: Vec<LoadError>,
    facts: Facts,
}

impl Loaded {
    fn finish(self, sources: Sources, checkout: &Checkout, first_file: Option<String>) -> Project {
        let edges = edges(&self.services);
        let (project_name, name_entry) = match self.name {
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
                services: self
                    .services
                    .into_iter()
                    .map(|(service, _)| service)
                    .collect(),
                volumes: self.volumes,
                networks: self.networks,
                edges,
            },
            unresolved: self.unresolved,
            errors: self.errors,
            sources,
            facts: Facts {
                name_entry,
                first_file,
                ..self.facts
            },
        }
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

/// Loads one parsed file into [`Loaded`].
struct FileLoader<'a> {
    source: &'a SourceText,
    file: FileId,
    out: &'a mut Loaded,
}

impl FileLoader<'_> {
    fn load(&mut self) {
        let root = match yaml::parse(self.file, &self.source.text) {
            Ok(root) => root,
            Err(error) => {
                self.error(error.message, error.at);
                return;
            }
        };
        self.variables(&root);
        match root.value {
            Value::Map(_) => {}
            Value::Null => {
                self.error(
                    "the file is empty; a Compose file is a mapping with keys such as services"
                        .to_owned(),
                    0,
                );
                return;
            }
            Value::Scalar(_) | Value::Seq(_) => {
                self.error(
                    "the top level must be a mapping with keys such as services".to_owned(),
                    root.span.start,
                );
                return;
            }
        }
        if let Some(entry) = root.entry("name")
            && let Some(name) = value(Some(&entry.value))
        {
            let fact = self.entry_fact(None, entry);
            self.out.name = Some((name, fact));
        }
        if let Some(services) = root.get("services") {
            self.services(services);
        }
        self.resources(ResourceKind::Volume, root.get("volumes"));
        self.resources(ResourceKind::Network, root.get("networks"));
    }

    fn error(&mut self, message: String, at: usize) {
        self.out.errors.push(LoadError {
            file: self.source.path.clone(),
            message,
            source: Some(self.source.source_ref(at)),
        });
    }

    /// An entry's key and value bytes, the value ending at its own text, and the value interpolated
    /// (a collection's text as written).
    fn entry_fact(&self, service: Option<&str>, entry: &Entry) -> EntryFact {
        let value = entry.value.span.start..entry.value.last_byte();
        let text = match entry.value.as_str() {
            Some(text) => interpolate(text).value,
            None => self.source.slice(value.clone()).to_owned(),
        };
        EntryFact {
            service: service.map(str::to_owned),
            file: self.file,
            key: entry.key.span.clone(),
            value,
            text,
        }
    }

    fn services(&mut self, services: &Node) {
        match &services.value {
            Value::Map(entries) => {
                for entry in entries {
                    self.service(entry);
                }
            }
            Value::Null => {}
            Value::Scalar(_) | Value::Seq(_) => self.error(
                "services must be a mapping of service names to their settings".to_owned(),
                services.span.start,
            ),
        }
    }

    fn service(&mut self, entry: &Entry) {
        let Some(name) = entry.key.as_str() else {
            self.error(
                "a service name must be text".to_owned(),
                entry.key.span.start,
            );
            return;
        };
        let node = &entry.value;
        if !matches!(node.value, Value::Map(_)) {
            self.error(
                format!("service {name} must be a mapping of its settings"),
                node.span.start,
            );
            return;
        }
        let ports = self.ports(name, node.get("ports"));
        let volumes = self.volumes(name, node.get("volumes"));
        let mut container_name = None;
        if let Some(entry) = node.entry("container_name")
            && let Some(text) = value(Some(&entry.value))
        {
            let fact = self.entry_fact(Some(name), entry);
            self.out.facts.container_names.push(fact);
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
            build: self.build(node.get("build")),
            ports,
            volumes,
            networks: names(node.get("networks")),
            depends_on: names(node.get("depends_on")),
            container_name,
            profiles: list(node.get("profiles")),
            source: self.source.source_ref(entry.key.span.start),
        };
        self.out.services.push((service, links));
    }

    /// `build: <context>`, or a mapping with `context` (the file's folder when left out) and
    /// `dockerfile`.
    fn build(&self, node: Option<&Node>) -> Option<Build> {
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
            context: self.context(context),
            dockerfile,
        })
    }

    /// A build context as a folder relative to the checkout root, `.` for the root itself. A URL,
    /// or a folder outside the checkout, stays as written.
    fn context(&self, context: String) -> String {
        if is_remote(&context) {
            return context;
        }
        match join(parent(&self.source.path), &context) {
            Ok(folder) if folder.is_empty() => ".".to_owned(),
            Ok(folder) => folder,
            Err(_) => context,
        }
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
                self.error(
                    format!("ports of {service} must be a list"),
                    node.span.start,
                );
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
                    return Some(self.publish(service, written, node.span.clone(), form));
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
                    let bytes = published.unwrap_or(target).span.clone();
                    return Some(self.publish(service, written, bytes, PortForm::Long));
                }
                format!("a port mapping of {service} needs a target")
            }
            Value::Null | Value::Seq(_) => format!(
                "a port of {service} must be text such as 8080:80, a number, or a mapping with a target"
            ),
        };
        self.error(error, node.span.start);
        None
    }

    /// The binding of a port written at `bytes`, and its fact. A host port is fixed when it is
    /// there, isn't empty, and uses no variable: a variable, even with a default, lets each copy
    /// of the stack choose its own.
    fn publish(
        &mut self,
        service: &str,
        written: Written<'_>,
        bytes: Range<usize>,
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
            source: self.source.source_ref(bytes.start),
            span: self.source.span(bytes.clone()),
        };
        self.out.facts.ports.push(PortFact {
            service: service.to_owned(),
            file: self.file,
            bytes,
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
                self.error(
                    format!("volumes of {service} must be a list"),
                    node.span.start,
                );
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
            self.error(message, node.span.start);
            None
        })
    }

    /// Top-level `volumes` or `networks`: each a resource, and a fact when it sets `name:` or is
    /// external.
    fn resources(&mut self, kind: ResourceKind, node: Option<&Node>) {
        let Some(node) = node else {
            return;
        };
        let what = match kind {
            ResourceKind::Volume => "volume",
            ResourceKind::Network => "network",
        };
        let entries = match &node.value {
            Value::Map(entries) => entries,
            Value::Null => return,
            Value::Scalar(_) | Value::Seq(_) => {
                self.error(
                    format!("{what}s must be a mapping of {what} names to their settings"),
                    node.span.start,
                );
                return;
            }
        };
        for entry in entries {
            let Some(name) = entry.key.as_str() else {
                self.error(format!("a {what} name must be text"), entry.key.span.start);
                continue;
            };
            let settings = &entry.value;
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
            let resource = ResourceNode {
                name: name.to_owned(),
                fixed_name,
                external: external_entry.is_some(),
                source: self.source.source_ref(entry.key.span.start),
            };
            if name_entry.is_some() || external_entry.is_some() {
                let fact = ResourceFact {
                    kind,
                    name: name.to_owned(),
                    file: self.file,
                    name_entry: name_entry.map(|entry| self.entry_fact(None, entry)),
                    external_entry: external_entry.map(|entry| self.entry_fact(None, entry)),
                };
                self.out.facts.resources.push(fact);
            }
            match kind {
                ResourceKind::Volume => self.out.volumes.push(resource),
                ResourceKind::Network => self.out.networks.push(resource),
            }
        }
    }

    /// Reports the variables with no default in every scalar value of the file, keys aside, once
    /// per written position and in document order: an alias or a `<<` merge copies a node with
    /// the position it is written at.
    fn variables(&mut self, root: &Node) {
        let mut scalars = BTreeMap::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            match &node.value {
                Value::Scalar(text) if text.contains('$') => {
                    scalars
                        .entry(node.span.start)
                        .or_insert((node.span.clone(), text.as_str()));
                }
                Value::Seq(items) => pending.extend(items),
                Value::Map(entries) => pending.extend(entries.iter().map(|entry| &entry.value)),
                Value::Scalar(_) | Value::Null => {}
            }
        }
        for (span, text) in scalars.into_values() {
            let written = self.source.text.get(span.clone()).unwrap_or_default();
            let mut from = 0;
            for missing in interpolate(text).missing {
                // Where the scalar's text doesn't hold the reference as written (an escape, a
                // folded line), the scalar stands in.
                let bytes = match find_reference(written, from, &missing.reference) {
                    Some(at) => {
                        from = at + missing.reference.len();
                        span.start + at..span.start + from
                    }
                    None => span.clone(),
                };
                self.out.unresolved.push(Unresolved {
                    variable: missing.name.clone(),
                    source: self.source.source_ref(bytes.start),
                });
                self.out.facts.variables.push(VariableUse {
                    name: missing.name,
                    file: self.file,
                    bytes,
                });
            }
        }
    }
}

/// A port's segments as written, before interpolation.
struct Written<'s> {
    ip: Option<&'s str>,
    host: Option<&'s str>,
    container: &'s str,
    protocol: Option<&'s str>,
}

/// A scalar's interpolated value; none for a collection, a null or an empty value.
fn value(node: Option<&Node>) -> Option<String> {
    interpolated(node.and_then(Node::as_str))
}

fn interpolated(text: Option<&str>) -> Option<String> {
    text.map(|text| interpolate(text).value)
        .filter(|value| !value.is_empty())
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

/// The byte of `reference`'s `$` in `written`, a scalar's source text, searching from `from`: the
/// first `$` past `$$` pairs that starts the reference, and for a bare `$NAME`, not a longer name.
///
/// A scalar's missing references come in text order and never overlap, so searching each from
/// where the previous one ended finds the one meant, even inside a default or past a `${A:?$B}`
/// that holds the same text. The one miss: a reference also written inside an alternative
/// (`${A:+$B} $B`), which interpolation never reads; the earlier `$B` is taken.
fn find_reference(written: &str, from: usize, reference: &str) -> Option<usize> {
    let bare = !reference.starts_with("${");
    let mut at = from;
    loop {
        let dollar = at + written.get(at..)?.find('$')?;
        let rest = written.get(dollar..)?;
        if rest.starts_with("$$") {
            at = dollar + 2;
            continue;
        }
        let longer = rest
            .as_bytes()
            .get(reference.len())
            .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'_');
        if rest.starts_with(reference) && !(bare && longer) {
            return Some(dollar);
        }
        at = dollar + 1;
    }
}

/// Loads `text` as `compose.yaml` in the primary checkout.
#[cfg(test)]
pub(crate) fn one(text: &str) -> Project {
    let files = crate::files::MemFiles::from([("compose.yaml", text)]);
    load_project(&files, &Checkout::primary(), &["compose.yaml".to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::{MemFiles, RecordingFiles};
    use crate::model::{SourceRef, Span};

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
}
