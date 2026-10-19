//! Reads Compose files and gathers the services, volumes and networks each defines, as YAML trees
//! whose nodes keep the file and bytes they are written at.

use std::rc::Rc;

use crate::files::ReadFile;
use crate::model::{LoadError, Unresolved};
use crate::paths::{Checkout, normalize, parent};
use crate::source::{FileId, SourceText, Sources};
use crate::yaml::{self, Entry, Node, Value};

use super::variables::variables;
use super::{VariableUse, error_at, value};

/// A service, volume or network: its name, the key it is defined at, and its settings.
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
enum Read {
    /// Its file and tree.
    Tree(FileId, Rc<Node>),
    /// Read, but not YAML the tree can hold; the error is reported in the file.
    Broken,
    /// Not read, with the reader's message.
    Unreadable(String),
}

pub(super) struct Loader<'a> {
    files: &'a dyn ReadFile,
    checkout: &'a Checkout,
    out: Loaded,
}

impl<'a> Loader<'a> {
    pub fn new(files: &'a dyn ReadFile, checkout: &'a Checkout) -> Loader<'a> {
        Loader {
            files,
            checkout,
            out: Loaded::default(),
        }
    }

    /// Loads each of `paths`, relative to the checkout root, on its own.
    pub fn project(mut self, paths: &[String]) -> Loaded {
        for written in paths {
            let path = match normalize(written) {
                Ok(path) => path,
                Err(message) => {
                    self.out.errors.push(LoadError {
                        file: written.clone(),
                        message,
                        source: None,
                    });
                    continue;
                }
            };
            self.out.first_file.get_or_insert_with(|| path.clone());
            match self.read(&path) {
                Read::Tree(file, root) => self.file(file, &root, parent(&path)),
                Read::Broken => {}
                Read::Unreadable(message) => self.out.errors.push(LoadError {
                    file: path,
                    message,
                    source: None,
                }),
            }
        }
        self.out
    }

    /// Reads `path` under the checkout and parses it, reporting its variables.
    fn read(&mut self, path: &str) -> Read {
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

    fn error(&mut self, file: FileId, byte: usize, message: String) {
        let error = error_at(&self.out.sources, file, byte, message);
        self.out.errors.push(error);
    }

    /// Gathers what one parsed file defines. `folder` is where its relative paths resolve.
    fn file(&mut self, file: FileId, root: &Node, folder: &str) {
        match root.value {
            Value::Map(_) => {}
            Value::Null => {
                self.error(
                    file,
                    0,
                    "the file is empty; a Compose file is a mapping with keys such as services"
                        .to_owned(),
                );
                return;
            }
            Value::Scalar(_) | Value::Seq(_) => {
                self.error(
                    file,
                    root.span.start,
                    "the top level must be a mapping with keys such as services".to_owned(),
                );
                return;
            }
        }
        if let Some(entry) = root.entry("name")
            && value(Some(&entry.value)).is_some()
        {
            self.out.name = Some(entry.clone());
        }
        let services = self.definitions(root.get("services"), "service", folder);
        self.out.services.extend(services);
        let volumes = self.definitions(root.get("volumes"), "volume", folder);
        self.out.volumes.extend(volumes);
        let networks = self.definitions(root.get("networks"), "network", folder);
        self.out.networks.extend(networks);
    }

    /// The entries of a top-level `services`, `volumes` or `networks` mapping whose names are text.
    fn definitions(&mut self, node: Option<&Node>, what: &str, folder: &str) -> Vec<Defined> {
        let Some(node) = node else {
            return Vec::new();
        };
        let entries = match &node.value {
            Value::Map(entries) => entries,
            Value::Null => return Vec::new(),
            Value::Scalar(_) | Value::Seq(_) => {
                self.error(
                    node.file,
                    node.span.start,
                    format!("{what}s must be a mapping of {what} names to their settings"),
                );
                return Vec::new();
            }
        };
        let mut defined = Vec::new();
        for entry in entries {
            let Some(name) = entry.key.as_str() else {
                self.error(
                    entry.key.file,
                    entry.key.span.start,
                    format!("a {what} name must be text"),
                );
                continue;
            };
            defined.push(Defined {
                name: name.to_owned(),
                key: entry.key.clone(),
                node: entry.value.clone(),
                folder: folder.to_owned(),
            });
        }
        defined
    }
}
