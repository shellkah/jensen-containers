//! Loads Compose files into the project model, with the facts the readiness rules read.
//!
//! Loading has two passes. [`load`] reads the files, follows `include` and `extends`, and merges
//! what they define the way `docker compose -f a -f b` does ([`merge`]), as YAML trees whose every
//! node keeps the file and bytes it is written at. [`build`] then turns the merged trees into the
//! model and the facts, taking each position from its node's own file through [`Sources`].

mod build;
mod facts;
mod load;
mod merge;
mod variables;

#[cfg(test)]
mod tests;

pub use facts::{EntryFact, Facts, PortFact, PortForm, ResourceFact, ResourceKind, VariableUse};

use crate::files::ReadFile;
use crate::interpolate::interpolate;
use crate::model::{LoadError, ProjectModel, SourceRef, Unresolved};
use crate::paths::Checkout;
use crate::source::{FileId, Sources};
use crate::yaml::Node;

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

/// Loads the Compose files of `paths`, relative to the checkout root, as `docker compose -f a
/// -f b` would: in order, each merged over the ones before, following `include` and `extends`.
/// Every read goes through `checkout`, and every path the result reports is relative to the
/// checkout root; `model.files` lists the files read, in reading order. A path that leaves the
/// checkout or isn't a `.yaml` or `.yml` file (never read), a file that can't be read, an include
/// or extends that can't be followed, and a part of a file that has the wrong shape are
/// [`LoadError`]s, and the rest still loads. A `LoadError`'s source, when it has one, is in its
/// `file`.
pub fn load_project(files: &dyn ReadFile, checkout: &Checkout, paths: &[String]) -> Project {
    let loaded = load::Loader::new(files, checkout).project(paths);
    build::project(loaded, checkout)
}

/// The zero-based line and UTF-16 column of `byte` in `file`.
fn position(sources: &Sources, file: FileId, byte: usize) -> SourceRef {
    match sources.get(file) {
        Some(source) => source.source_ref(byte),
        // Every node's file is one that was read, so this is never reached.
        None => SourceRef {
            file: String::new(),
            line: 0,
            column: 0,
        },
    }
}

/// A load error at `byte` of `file`.
fn error_at(sources: &Sources, file: FileId, byte: usize, message: String) -> LoadError {
    let at = position(sources, file, byte);
    LoadError {
        file: at.file.clone(),
        message,
        source: Some(at),
    }
}

/// A scalar's interpolated value; none for a collection, a null or an empty value.
fn value(node: Option<&Node>) -> Option<String> {
    interpolated(node.and_then(Node::as_str))
}

fn interpolated(text: Option<&str>) -> Option<String> {
    text.map(|text| interpolate(text).value)
        .filter(|value| !value.is_empty())
}

/// Loads `text` as `compose.yaml` in the primary checkout.
#[cfg(test)]
pub(crate) fn one(text: &str) -> Project {
    let files = crate::files::MemFiles::from([("compose.yaml", text)]);
    load_project(&files, &Checkout::primary(), &["compose.yaml".to_string()])
}
