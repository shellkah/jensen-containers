//! How a later definition merges over an earlier one, as `docker compose -f a -f b` merges files
//! and as a service merges over the service it extends. Every node keeps the file and bytes it is
//! written at, so a merged value still points where it is written.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::yaml::{Entry, Node, Value};

/// The service keys whose lists concatenate when both definitions hold a list. `depends_on` and
/// `networks` may be mappings too, which merge key by key like any other.
const CONCATENATED: [&str; 7] = [
    "ports",
    "volumes",
    "links",
    "profiles",
    "expose",
    "depends_on",
    "networks",
];

/// Merges `later` into `base`: two mappings merge key by key, a null leaves the earlier value (as
/// Compose leaves it), and anything else replaces it.
pub(super) fn merge(base: &mut Node, later: Node) {
    match later.value {
        Value::Null => {}
        Value::Map(entries) if matches!(base.value, Value::Map(_)) => {
            if let Value::Map(earlier) = &mut base.value {
                merge_entries(earlier, entries);
            }
        }
        value => {
            *base = Node {
                file: later.file,
                span: later.span,
                value,
            }
        }
    }
}

/// Merges a later definition of a service over an earlier one, as [`merge`] does, except that the
/// [`CONCATENATED`] lists append the later items the earlier list doesn't hold already, and a
/// `build: <context>` merges as `build: { context: <context> }`.
pub(super) fn service(base: &mut Node, later: Node) {
    match later.value {
        Value::Null => {}
        Value::Map(entries) if matches!(base.value, Value::Map(_)) => {
            if let Value::Map(earlier) = &mut base.value {
                service_entries(earlier, entries);
            }
        }
        value => {
            *base = Node {
                file: later.file,
                span: later.span,
                value,
            }
        }
    }
}

fn service_entries(earlier: &mut Vec<Entry>, later: Vec<Entry>) {
    let mut keys = Keys::of(earlier);
    for entry in later {
        let Some(existing) = keys.find(earlier, &entry.key) else {
            keys.add(earlier, entry);
            continue;
        };
        let key = entry.key.as_str().unwrap_or_default();
        let lists = matches!(
            (&existing.value.value, &entry.value.value),
            (Value::Seq(_), Value::Seq(_))
        );
        if lists && CONCATENATED.contains(&key) {
            if let (Value::Seq(items), Value::Seq(more)) =
                (&mut existing.value.value, entry.value.value)
            {
                concatenate(items, more);
            }
        } else if key == "build" {
            let base = std::mem::replace(&mut existing.value, placeholder(&entry.value));
            existing.value = context_mapping(base);
            let later = Entry {
                key: entry.key,
                value: context_mapping(entry.value),
            };
            merge_entry(existing, later);
        } else {
            merge_entry(existing, entry);
        }
    }
}

fn merge_entries(earlier: &mut Vec<Entry>, later: Vec<Entry>) {
    let mut keys = Keys::of(earlier);
    for entry in later {
        match keys.find(earlier, &entry.key) {
            Some(existing) => merge_entry(existing, entry),
            None => keys.add(earlier, entry),
        }
    }
}

/// Where a mapping's entries with text keys sit, so a merge finds each key at once.
struct Keys(HashMap<String, usize>);

impl Keys {
    fn of(entries: &[Entry]) -> Keys {
        let mut keys = HashMap::new();
        for (at, entry) in entries.iter().enumerate() {
            if let Some(key) = entry.key.as_str() {
                keys.entry(key.to_owned()).or_insert(at);
            }
        }
        Keys(keys)
    }

    /// The first entry whose key is the same text as `key`; a key that isn't text matches none.
    fn find<'e>(&self, entries: &'e mut [Entry], key: &Node) -> Option<&'e mut Entry> {
        let at = *self.0.get(key.as_str()?)?;
        entries.get_mut(at)
    }

    fn add(&mut self, entries: &mut Vec<Entry>, entry: Entry) {
        if let Some(key) = entry.key.as_str() {
            self.0.entry(key.to_owned()).or_insert(entries.len());
        }
        entries.push(entry);
    }
}

/// A later entry over an earlier one with the same key: two mappings merge, a null leaves the
/// earlier entry, and anything else replaces it with its key, so a key and its value always come
/// from the same file. A merged mapping keeps its earlier key.
fn merge_entry(existing: &mut Entry, later: Entry) {
    match (&existing.value.value, &later.value.value) {
        (_, Value::Null) => {}
        (Value::Map(_), Value::Map(_)) => merge(&mut existing.value, later.value),
        _ => *existing = later,
    }
}

/// An empty node standing in for a value while it is taken out.
fn placeholder(near: &Node) -> Node {
    Node {
        file: near.file,
        span: near.span.start..near.span.start,
        value: Value::Null,
    }
}

/// `build: <context>` as `build: { context: <context> }`, the form Compose merges; its key is
/// placed over the context's own text.
fn context_mapping(node: Node) -> Node {
    if node.as_str().is_none() {
        return node;
    }
    let key = Node {
        file: node.file,
        span: node.span.clone(),
        value: Value::Scalar("context".to_owned()),
    };
    Node {
        file: node.file,
        span: node.span.clone(),
        value: Value::Map(vec![Entry { key, value: node }]),
    }
}

/// Appends the later items the earlier ones don't hold already, compared as written.
fn concatenate(items: &mut Vec<Node>, later: Vec<Node>) {
    let mut earlier: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        earlier.entry(fingerprint(item)).or_default().push(index);
    }
    let fresh: Vec<Node> = later
        .into_iter()
        .filter(|item| {
            let held = earlier.get(&fingerprint(item)).is_some_and(|indexes| {
                indexes
                    .iter()
                    .any(|&index| items.get(index).is_some_and(|earlier| same(earlier, item)))
            });
            !held
        })
        .collect();
    items.extend(fresh);
}

/// Whether two nodes hold the same value, wherever they are written.
fn same(a: &Node, b: &Node) -> bool {
    match (&a.value, &b.value) {
        (Value::Null, Value::Null) => true,
        (Value::Scalar(a), Value::Scalar(b)) => a == b,
        (Value::Seq(a), Value::Seq(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| same(&a.key, &b.key) && same(&a.value, &b.value))
        }
        _ => false,
    }
}

/// A hash of a node's value that ignores where it is written, so equal values hash alike.
fn fingerprint(node: &Node) -> u64 {
    fn feed(node: &Node, hasher: &mut DefaultHasher) {
        match &node.value {
            Value::Null => 0u8.hash(hasher),
            Value::Scalar(text) => {
                1u8.hash(hasher);
                text.hash(hasher);
            }
            Value::Seq(items) => {
                2u8.hash(hasher);
                items.len().hash(hasher);
                for item in items {
                    feed(item, hasher);
                }
            }
            Value::Map(entries) => {
                3u8.hash(hasher);
                entries.len().hash(hasher);
                for entry in entries {
                    feed(&entry.key, hasher);
                    feed(&entry.value, hasher);
                }
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    feed(node, &mut hasher);
    hasher.finish()
}
