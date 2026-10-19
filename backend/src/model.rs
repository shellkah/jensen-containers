//! The project model the backend answers with. Lines and columns are zero-based, and columns and
//! offsets count UTF-16 code units, the way the editor counts them.

use serde::Serialize;

/// A position in a file: its path relative to the checkout root, a line and a column.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// A range of a file's text, as UTF-16 offsets from the start of the file.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub from: u32,
    pub to: u32,
}
