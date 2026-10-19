//! The variables nothing provides, found in every scalar value of every file read.

use std::collections::BTreeMap;

use crate::interpolate::interpolate;
use crate::model::Unresolved;
use crate::source::Sources;
use crate::yaml::{Node, Value};

use super::{VariableUse, position};

/// Reports the variables with no default in every scalar value of `root`, keys aside, once per
/// written position and in document order. An alias or a `<<` merge copies a node with the
/// position it is written at, so a scalar reached twice is keyed once, on its file and first byte.
pub(super) fn variables(sources: &Sources, root: &Node) -> Vec<(Unresolved, VariableUse)> {
    let mut scalars = BTreeMap::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match &node.value {
            Value::Scalar(text) if text.contains('$') => {
                scalars
                    .entry((node.file, node.span.start))
                    .or_insert((node.span.clone(), text.as_str()));
            }
            Value::Seq(items) => pending.extend(items),
            Value::Map(entries) => pending.extend(entries.iter().map(|entry| &entry.value)),
            Value::Scalar(_) | Value::Null => {}
        }
    }
    let mut found = Vec::new();
    for ((file, _), (span, text)) in scalars {
        let written = sources
            .get(file)
            .and_then(|source| source.text.get(span.clone()))
            .unwrap_or_default();
        let mut from = 0;
        for missing in interpolate(text).missing {
            // Where the scalar's text doesn't hold the reference as written (an escape, a folded
            // line), the scalar stands in.
            let bytes = match find_reference(written, from, &missing.reference) {
                Some(at) => {
                    from = at + missing.reference.len();
                    span.start + at..span.start + from
                }
                None => span.clone(),
            };
            let unresolved = Unresolved {
                variable: missing.name.clone(),
                source: position(sources, file, bytes.start),
            };
            let use_ = VariableUse {
                name: missing.name,
                file,
                bytes,
            };
            found.push((unresolved, use_));
        }
    }
    found
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
