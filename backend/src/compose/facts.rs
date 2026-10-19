//! Where the things the readiness rules judge are written.

use std::ops::Range;

use crate::short_syntax::PortParts;
use crate::source::FileId;

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
