//! Reads Compose files the way `docker compose -f a -f b` does: each path once, a file's includes
//! before its own services, and each service with the service it extends merged under it. What
//! they define is kept as YAML trees whose nodes keep the file and bytes they are written at.
//!
//! Relative paths resolve as Compose resolves them: in every `-f` file against the first one's
//! folder, in an included file against its own folder (the first file's, for an include that
//! lists several), and in an extended file against its own folder.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::files::ReadFile;
use crate::model::{LoadError, Unresolved};
use crate::paths::{Checkout, join, normalize, parent};
use crate::source::{FileId, SourceText, Sources};
use crate::yaml::{self, Entry, Node, Value};

use super::merge;
use super::variables::variables;
use super::{VariableUse, error_at, value};

/// How many levels includes nest, and extends do, before the next level is an error.
const MAX_NESTING: usize = 20;

/// The most YAML nodes extends and repeated includes copy for one project: as many as one parse
/// may create, alias copies included.
const MAX_COPIED: usize = yaml::MAX_NODES;

const INCLUDE_ENTRY: &str =
    "an include entry is a file's path, or a mapping whose path is a path or a list of paths";

/// A service, volume or network: its name, the key it is first defined at, and its settings.
#[derive(Clone, Debug)]
pub(super) struct Defined {
    pub name: String,
    pub key: Node,
    pub node: Node,
    /// The folder a service's build context resolves against, relative to the checkout root.
    pub folder: String,
}

/// What the files define, and what was found on the way.
#[derive(Default)]
pub(super) struct Loaded {
    pub sources: Sources,
    pub services: Vec<Defined>,
    pub volumes: Vec<Defined>,
    pub networks: Vec<Defined>,
    /// The top-level `name:` that names the project; a later file's wins.
    pub name: Option<Entry>,
    /// The first Compose file, relative to the checkout root.
    pub first_file: Option<String>,
    pub errors: Vec<LoadError>,
    pub unresolved: Vec<Unresolved>,
    pub variables: Vec<VariableUse>,
}

/// What reading a path gave.
#[derive(Clone)]
enum Read {
    /// Its file and tree.
    Tree(FileId, Rc<Node>),
    /// Read, but not YAML the tree can hold; the error is reported in the file.
    Broken,
    /// Not read, with the reader's message.
    Unreadable(String),
}

/// What one Compose file defines, or several merged like `-f`.
#[derive(Clone, Default)]
struct Config {
    /// The top-level `name:`, when it isn't empty.
    name: Option<Entry>,
    services: Vec<Defined>,
    volumes: Vec<Defined>,
    networks: Vec<Defined>,
}

impl Config {
    /// Merges what a later file defines over this, like `docker compose -f`. A definition keeps
    /// where it is first defined, and a later one's new names come after the earlier ones.
    fn merge(&mut self, later: Config) {
        if later.name.is_some() {
            self.name = later.name;
        }
        by_name(&mut self.services, later.services, |base, later| {
            merge_service(&mut base.node, &mut base.folder, later.node, later.folder);
        });
        for (into, from) in [
            (&mut self.volumes, later.volumes),
            (&mut self.networks, later.networks),
        ] {
            by_name(into, from, |base, later| {
                merge::merge(&mut base.node, later.node)
            });
        }
    }
}

/// Adds each of `later` to `into`, merging it into the one of the same name with `merge`.
fn by_name(into: &mut Vec<Defined>, later: Vec<Defined>, merge: impl Fn(&mut Defined, Defined)) {
    let mut index: HashMap<String, usize> = into
        .iter()
        .enumerate()
        .map(|(at, defined)| (defined.name.clone(), at))
        .collect();
    for defined in later {
        match index.get(&defined.name).and_then(|&at| into.get_mut(at)) {
            Some(base) => merge(base, defined),
            None => {
                index.insert(defined.name.clone(), into.len());
                into.push(defined);
            }
        }
    }
}

/// Merges a later definition of a service over an earlier one. Each one's build context
/// resolves against its own folder, so the merged context takes the folder of the definition
/// that writes it: the later one's when it writes a context or the earlier one has no build.
fn merge_service(base: &mut Node, base_folder: &mut String, later: Node, later_folder: String) {
    if writes_context(&later) || base.get("build").is_none_or(Node::is_null) {
        *base_folder = later_folder;
    }
    merge::service(base, later);
}

/// Whether a service's settings write its build context: `build: <context>`, or a `build`
/// mapping with a `context`.
fn writes_context(service: &Node) -> bool {
    service.get("build").is_some_and(|build| {
        build.as_str().is_some()
            || build
                .get("context")
                .is_some_and(|context| !context.is_null())
    })
}

/// A service's settings without their `extends` entry, and that entry's value.
fn without_extends(service: &Node) -> (Node, Option<Node>) {
    let mut own = service.clone();
    let extends = match &mut own.value {
        Value::Map(entries) => entries
            .iter()
            .position(|entry| entry.key.as_str() == Some("extends"))
            .map(|at| entries.remove(at).value),
        _ => None,
    };
    (own, extends)
}

/// Why `what` isn't merged in: copying it would pass [`MAX_COPIED`].
fn too_large(what: &str) -> String {
    format!(
        "the project is too large: its extends and includes would copy more than {MAX_COPIED} YAML nodes, so {what} isn't merged in"
    )
}

/// The YAML nodes in `node` and under it, keys included.
fn weight(node: &Node) -> usize {
    let mut nodes = 0;
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        nodes += 1;
        pending.extend(node.items());
        pending.extend(
            node.entries()
                .iter()
                .flat_map(|entry| [&entry.key, &entry.value]),
        );
    }
    nodes
}

/// The YAML nodes of everything a file defines.
fn config_weight(config: &Config) -> usize {
    [&config.services, &config.volumes, &config.networks]
        .into_iter()
        .flatten()
        .map(|defined| weight(&defined.key) + weight(&defined.node))
        .sum()
}

pub(super) struct Loader<'a> {
    files: &'a dyn ReadFile,
    checkout: &'a Checkout,
    out: Loaded,
    /// What reading each path gave, so each is read once.
    reads: HashMap<String, Read>,
    /// What each included file defines, by its file and the folder its paths resolve against.
    configs: HashMap<(FileId, String), Option<Config>>,
    /// The files whose definitions an include has used.
    used: HashSet<FileId>,
    /// The YAML nodes extends and repeated includes have copied so far.
    copied: usize,
    /// The errors reported so far, so a file loaded twice reports each once.
    reported: HashSet<Reported>,
}

/// An error's file, message, and the line and column it points at.
type Reported = (String, String, Option<(u32, u32)>);

impl<'a> Loader<'a> {
    pub fn new(files: &'a dyn ReadFile, checkout: &'a Checkout) -> Loader<'a> {
        Loader {
            files,
            checkout,
            out: Loaded::default(),
            reads: HashMap::new(),
            configs: HashMap::new(),
            used: HashSet::new(),
            copied: 0,
            reported: HashSet::new(),
        }
    }

    /// Counts `nodes` more copied nodes, and whether they stay within [`MAX_COPIED`]. Each service
    /// that extends another copies it, so a file far under the parser's caps could otherwise
    /// make many times its size and exhaust the wasm call's memory.
    fn copy(&mut self, nodes: usize) -> bool {
        self.copied = self.copied.saturating_add(nodes);
        self.copied <= MAX_COPIED
    }

    /// Loads `paths`, relative to the checkout root, in order, merged like `docker compose -f`.
    /// A path given twice loads once.
    pub fn project(mut self, paths: &[String]) -> Loaded {
        let mut config = Config::default();
        let mut given = HashSet::new();
        for written in paths {
            let path = match normalize(written) {
                Ok(path) => path,
                Err(message) => {
                    self.report(LoadError {
                        file: written.clone(),
                        message,
                        source: None,
                    });
                    continue;
                }
            };
            // Every `-f` file's relative paths resolve against the first one's folder.
            let folder = parent(self.out.first_file.get_or_insert_with(|| path.clone())).to_owned();
            if !given.insert(path.clone()) {
                continue;
            }
            match self.read(&path) {
                Read::Tree(file, root) => {
                    let mut chain = vec![path];
                    if let Some(later) = self.file(file, &root, &folder, &mut chain, true) {
                        config.merge(later);
                    }
                }
                Read::Broken => {}
                Read::Unreadable(message) => self.report(LoadError {
                    file: path,
                    message,
                    source: None,
                }),
            }
        }
        let Config {
            name,
            services,
            volumes,
            networks,
        } = config;
        Loaded {
            services,
            volumes,
            networks,
            name,
            ..self.out
        }
    }

    /// Reads `path` under the checkout once; asking again answers what the first read gave.
    fn read(&mut self, path: &str) -> Read {
        if let Some(read) = self.reads.get(path) {
            return read.clone();
        }
        let read = self.read_once(path);
        self.reads.insert(path.to_owned(), read.clone());
        read
    }

    /// Reads `path` and parses it, reporting its variables.
    fn read_once(&mut self, path: &str) -> Read {
        let text = match self.files.read(&self.checkout.read_path(path)) {
            Ok(text) => text,
            Err(message) => return Read::Unreadable(message),
        };
        // The source drops a leading byte order mark, so its text is what gets parsed.
        let file = self.out.sources.add(SourceText::new(path.to_owned(), text));
        let parsed = self
            .out
            .sources
            .get(file)
            .map(|source| yaml::parse(file, &source.text));
        match parsed {
            Some(Ok(root)) => {
                for (unresolved, use_) in variables(&self.out.sources, &root) {
                    self.out.unresolved.push(unresolved);
                    self.out.variables.push(use_);
                }
                Read::Tree(file, Rc::new(root))
            }
            Some(Err(error)) => {
                self.error(file, error.at, error.message);
                Read::Broken
            }
            None => Read::Broken,
        }
    }

    fn report(&mut self, error: LoadError) {
        let at = error.source.as_ref().map(|at| (at.line, at.column));
        if self
            .reported
            .insert((error.file.clone(), error.message.clone(), at))
        {
            self.out.errors.push(error);
        }
    }

    fn error(&mut self, file: FileId, byte: usize, message: String) {
        let error = error_at(&self.out.sources, file, byte, message);
        self.report(error);
    }

    /// An error at the start of `node`.
    fn error_on(&mut self, node: &Node, message: String) {
        self.error(node.file, node.span.start, message);
    }

    /// The path of `file`, relative to the checkout root.
    fn path(&self, file: FileId) -> String {
        self.out
            .sources
            .get(file)
            .map_or_else(String::new, |source| source.path.clone())
    }

    /// What one parsed file defines: its includes load first, then its own services, each with
    /// the service it extends merged under it, and its volumes and networks; what its includes
    /// define comes after its own. Its relative paths resolve against `folder`; `chain` holds the
    /// files that include it, itself last. Only a file given with `-f` (`top`) names the project.
    fn file(
        &mut self,
        file: FileId,
        root: &Node,
        folder: &str,
        chain: &mut Vec<String>,
        top: bool,
    ) -> Option<Config> {
        match root.value {
            Value::Map(_) => {}
            Value::Null => {
                self.error(
                    file,
                    0,
                    "the file is empty; a Compose file is a mapping with keys such as services"
                        .to_owned(),
                );
                return None;
            }
            Value::Scalar(_) | Value::Seq(_) => {
                self.error(
                    file,
                    root.span.start,
                    "the top level must be a mapping with keys such as services".to_owned(),
                );
                return None;
            }
        }
        let included = self.includes(root.get("include"), folder, chain);
        let mut config = Config::default();
        if top
            && let Some(entry) = root.entry("name")
            && value(Some(&entry.value)).is_some()
        {
            config.name = Some(entry.clone());
        }
        let services = root.get("services");
        config.services = self.definitions(services, "service", folder);
        if let Some(services) = services {
            for service in &mut config.services {
                let mut extending = vec![(file, service.name.clone())];
                let (node, context) =
                    self.extended(file, services, &service.node, folder, &mut extending);
                service.node = node;
                service.folder = context;
            }
        }
        config.volumes = self.definitions(root.get("volumes"), "volume", folder);
        config.networks = self.definitions(root.get("networks"), "network", folder);
        for project in included {
            self.import(&mut config.services, project.services);
            self.import(&mut config.volumes, project.volumes);
            self.import(&mut config.networks, project.networks);
        }
        Some(config)
    }

    /// The entries of a top-level `services`, `volumes` or `networks` mapping whose names are text.
    /// A name written twice is an error on the second, which is dropped.
    fn definitions(&mut self, node: Option<&Node>, what: &str, folder: &str) -> Vec<Defined> {
        let Some(node) = node else {
            return Vec::new();
        };
        let entries = match &node.value {
            Value::Map(entries) => entries,
            Value::Null => return Vec::new(),
            Value::Scalar(_) | Value::Seq(_) => {
                self.error_on(
                    node,
                    format!("{what}s must be a mapping of {what} names to their settings"),
                );
                return Vec::new();
            }
        };
        let mut defined = Vec::new();
        let mut names = HashSet::new();
        for entry in entries {
            let Some(name) = entry.key.as_str() else {
                self.error_on(&entry.key, format!("a {what} name must be text"));
                continue;
            };
            if !names.insert(name) {
                let file = self.path(entry.key.file);
                self.error_on(
                    &entry.key,
                    format!("{name} is defined twice in {file}; Compose refuses this"),
                );
                continue;
            }
            defined.push(Defined {
                name: name.to_owned(),
                key: entry.key.clone(),
                node: entry.value.clone(),
                folder: folder.to_owned(),
            });
        }
        defined
    }

    /// Adds what an included project defines after what the including file does. A name already
    /// defined is an error on the later definition, which is dropped; the same definition reached
    /// through two includes counts once.
    fn import(&mut self, into: &mut Vec<Defined>, later: Vec<Defined>) {
        let mut index: HashMap<String, usize> = into
            .iter()
            .enumerate()
            .map(|(at, defined)| (defined.name.clone(), at))
            .collect();
        for defined in later {
            let Some(first) = index.get(&defined.name).and_then(|&at| into.get(at)) else {
                index.insert(defined.name.clone(), into.len());
                into.push(defined);
                continue;
            };
            if first.key.file == defined.key.file && first.key.span == defined.key.span {
                continue;
            }
            let message = format!(
                "{} is defined in both {} and {}; Compose refuses this",
                defined.name,
                self.path(first.key.file),
                self.path(defined.key.file)
            );
            self.error_on(&defined.key, message);
        }
    }

    /// The projects a file's `include` names, each loaded on its own: an entry is a path, or a
    /// mapping whose `path` is a path or a list of paths that merge like `-f`. `project_directory`
    /// and `env_file` are left alone: the backend reads YAML files only.
    fn includes(
        &mut self,
        node: Option<&Node>,
        folder: &str,
        chain: &mut Vec<String>,
    ) -> Vec<Config> {
        let Some(node) = node else {
            return Vec::new();
        };
        let items = match &node.value {
            Value::Seq(items) => items,
            Value::Null => return Vec::new(),
            Value::Scalar(_) | Value::Map(_) => {
                self.error_on(
                    node,
                    "include must be a list of files to include".to_owned(),
                );
                return Vec::new();
            }
        };
        let mut included = Vec::new();
        for item in items {
            let paths: Vec<&Node> = match (&item.value, item.get("path")) {
                (Value::Scalar(_), _) => vec![item],
                (Value::Map(_), Some(path)) if path.as_str().is_some() => vec![path],
                (Value::Map(_), Some(path))
                    if !path.items().is_empty()
                        && path.items().iter().all(|path| path.as_str().is_some()) =>
                {
                    path.items().iter().collect()
                }
                (Value::Map(_), Some(path)) => {
                    self.error_on(path, INCLUDE_ENTRY.to_owned());
                    continue;
                }
                _ => {
                    self.error_on(item, INCLUDE_ENTRY.to_owned());
                    continue;
                }
            };
            if let Some(project) = self.include(&paths, folder, chain) {
                included.push(project);
            }
        }
        included
    }

    /// One include entry's files, merged like `-f`. Their paths resolve against `folder`, the
    /// including file's, and what they write against the first one's folder. A cycle, a level
    /// past [`MAX_NESTING`], a path leaving the checkout and a file that can't be read are errors
    /// on the path that names them.
    fn include(
        &mut self,
        paths: &[&Node],
        folder: &str,
        chain: &mut Vec<String>,
    ) -> Option<Config> {
        let mut merged: Option<Config> = None;
        let mut own_folder: Option<String> = None;
        for written in paths {
            let Some((text, path)) = self.target(written, folder) else {
                continue;
            };
            if chain.contains(&path) {
                let cycle: Vec<&str> = chain.iter().chain([&path]).map(String::as_str).collect();
                let cycle = cycle.join(" → ");
                self.error_on(
                    written,
                    format!("include cycle: {cycle}; Compose refuses this"),
                );
                continue;
            }
            if chain.len() > MAX_NESTING {
                self.error_on(
                    written,
                    format!(
                        "includes nest more than {MAX_NESTING} levels deep here, so {text} isn't loaded"
                    ),
                );
                continue;
            }
            let own = own_folder
                .get_or_insert_with(|| parent(&path).to_owned())
                .clone();
            match self.read(&path) {
                Read::Tree(file, root) => {
                    let Some(project) = self.included(file, &root, &own, written, path, chain)
                    else {
                        continue;
                    };
                    match (&mut merged, project) {
                        (Some(merged), Some(project)) => merged.merge(project),
                        (None, project) => merged = project,
                        (Some(_), None) => {}
                    }
                }
                Read::Broken => {}
                Read::Unreadable(message) => {
                    self.error_on(written, format!("can't include {text}: {message}"));
                }
            }
        }
        merged
    }

    /// What an included file defines, its paths resolving against `folder`. A file included
    /// again with the same folder isn't loaded again: what it defined is copied, so includes that
    /// fan out level after level stay linear. Every use of a file's definitions after the first
    /// counts against [`MAX_COPIED`]; past it the include is refused (`None`), with an error on
    /// `written`. `Some(None)` is a file that defines nothing.
    fn included(
        &mut self,
        file: FileId,
        root: &Node,
        folder: &str,
        written: &Node,
        path: String,
        chain: &mut Vec<String>,
    ) -> Option<Option<Config>> {
        let key = (file, folder.to_owned());
        let again = !self.used.insert(file);
        if let Some(config) = self.configs.get(&key) {
            let nodes = config.as_ref().map_or(0, config_weight);
            if !self.copy(nodes) {
                self.error_on(written, too_large(&path));
                return None;
            }
            return self.configs.get(&key).cloned();
        }
        if again && self.copied > MAX_COPIED {
            self.error_on(written, too_large(&path));
            return None;
        }
        chain.push(path);
        let config = self.file(file, root, folder, chain, false);
        let path = chain.pop().unwrap_or_default();
        let nodes = config.as_ref().map_or(0, config_weight);
        if again && !self.copy(nodes) {
            self.error_on(written, too_large(&path));
            return None;
        }
        self.configs.insert(key, config.clone());
        Some(config)
    }

    /// The path `written` names, as written and resolved against `folder` relative to the
    /// checkout root. An empty path, one naming the checkout root, and one leaving the checkout
    /// are errors on it.
    fn target(&mut self, written: &Node, folder: &str) -> Option<(String, String)> {
        let Some(text) = value(Some(written)) else {
            let message = match written.value {
                Value::Map(_) | Value::Seq(_) => "a path must be text",
                Value::Null | Value::Scalar(_) => "the path is empty",
            };
            self.error_on(written, message.to_owned());
            return None;
        };
        match join(folder, &text) {
            Ok(path) if path.is_empty() => {
                self.error_on(written, format!("{text} is a folder, not a Compose file"));
                None
            }
            Ok(path) => Some((text, path)),
            Err(message) => {
                self.error_on(written, message);
                None
            }
        }
    }

    /// `service`, a service of `file` whose `services` mapping is `services`, with the service it
    /// extends merged under it: `extends: base`, or `{ service, file }` with `file` relative to
    /// `folder`, where the file's paths resolve. Also gives the folder its build context
    /// resolves against. `chain` holds the services being extended, this one last. A cycle, a
    /// level past [`MAX_NESTING`], a missing target and a path leaving the checkout are errors
    /// on the entry that names them, and the service keeps its own keys.
    fn extended(
        &mut self,
        file: FileId,
        services: &Node,
        service: &Node,
        folder: &str,
        chain: &mut Vec<(FileId, String)>,
    ) -> (Node, String) {
        let (settings, extends) = without_extends(service);
        let own = (settings, folder.to_owned());
        let Some(extends) = extends else {
            return own;
        };
        let name = chain
            .last()
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let named = match &extends.value {
            Value::Scalar(_) => Some((&extends, None)),
            Value::Map(_) => extends
                .get("service")
                .map(|service| (service, extends.get("file"))),
            Value::Null | Value::Seq(_) => None,
        };
        let Some((base, base_node, file_node)) =
            named.and_then(|(node, file)| Some((value(Some(node))?, node, file)))
        else {
            self.error_on(
                &extends,
                format!(
                    "extends of {name} must name a service, such as extends: base or extends: {{ service: base, file: base.yml }}"
                ),
            );
            return own;
        };

        // The base's file: this one, or the one `file` names, whose paths resolve in its folder.
        let (base_file, root, base_folder, file_text) = match file_node {
            None => (file, None, folder.to_owned(), self.path(file)),
            Some(file_node) => {
                let Some((text, path)) = self.target(file_node, folder) else {
                    return own;
                };
                match self.read(&path) {
                    Read::Tree(id, root) => (id, Some(root), parent(&path).to_owned(), text),
                    Read::Broken => return own,
                    Read::Unreadable(message) => {
                        self.error_on(
                            file_node,
                            format!("can't extend {base} from {text}: {message}"),
                        );
                        return own;
                    }
                }
            }
        };
        let base_services = match &root {
            Some(root) => root.get("services"),
            None => Some(services),
        };
        let found = base_services.and_then(|services| Some((services, services.get(&base)?)));
        let Some((base_services, base_service)) = found else {
            self.error_on(
                base_node,
                format!("{name} extends {base}, which {file_text} doesn't define"),
            );
            return own;
        };
        let link = (base_file, base.clone());
        if chain.contains(&link) {
            let cycle: Vec<&str> = chain
                .iter()
                .map(|(_, name)| name.as_str())
                .chain([base.as_str()])
                .collect();
            let cycle = cycle.join(" → ");
            self.error_on(
                base_node,
                format!("extends cycle: {cycle}; Compose refuses this"),
            );
            return own;
        }
        if chain.len() > MAX_NESTING {
            self.error_on(
                base_node,
                format!(
                    "extends nest more than {MAX_NESTING} levels deep here, so {base} isn't merged"
                ),
            );
            return own;
        }
        if !matches!(base_service.value, Value::Map(_)) {
            self.error_on(
                base_node,
                format!("{name} extends {base}, whose settings in {file_text} aren't a mapping"),
            );
            return own;
        }
        // Each service that extends a base copies it.
        if !self.copy(weight(base_service)) {
            self.error_on(base_node, too_large(&base));
            return own;
        }

        chain.push(link);
        let (mut node, mut context) =
            self.extended(base_file, base_services, base_service, &base_folder, chain);
        chain.pop();
        let (settings, own_folder) = own;
        merge_service(&mut node, &mut context, settings, own_folder);
        (node, context)
    }
}
