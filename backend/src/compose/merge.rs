//! How a later definition merges over an earlier one, as `docker compose -f a -f b` merges files
//! and as a service merges over the service it extends. Every node keeps the file and bytes it is
//! written at, so a merged value still points where it is written.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::interpolate::interpolate;
use crate::short_syntax::split_volume;
use crate::yaml::{Entry, Node, Value};

use super::value;

/// The service keys whose lists concatenate when both definitions hold a list: the later items
/// come after the earlier ones, without exact duplicates. `volumes` is keyed by the container
/// target, as Compose keys it, so a later mount on a target takes the earlier one's place.
/// `depends_on` and `networks` may be mappings too, see [`NAMES`].
const CONCATENATED: [&str; 7] = [
    "ports",
    "volumes",
    "links",
    "profiles",
    "expose",
    "depends_on",
    "networks",
];

/// The service keys written as a list of names or as a mapping of those names to their settings.
/// Two mappings merge key by key; a list and a mapping merge as two mappings, the list's names
/// mapping to nothing, so the names of both are kept, as Compose merges them.
const NAMES: [&str; 2] = ["depends_on", "networks"];

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
/// [`CONCATENATED`] lists append the later items the earlier list doesn't hold already, a list of
/// [`NAMES`] merges with a mapping as a mapping, and a `build: <context>` merges as
/// `build: { context: <context> }`.
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
    for mut entry in later {
        let Some(existing) = keys.find(earlier, &entry.key) else {
            keys.add(earlier, entry);
            continue;
        };
        let key = entry.key.as_str().unwrap_or_default();
        if NAMES.contains(&key) {
            match (&existing.value.value, &entry.value.value) {
                (Value::Seq(_), Value::Map(_)) => names_mapping(&mut existing.value),
                (Value::Map(_), Value::Seq(_)) => names_mapping(&mut entry.value),
                _ => {}
            }
        }
        let lists = matches!(
            (&existing.value.value, &entry.value.value),
            (Value::Seq(_), Value::Seq(_))
        );
        if lists && CONCATENATED.contains(&key) {
            let by: fn(&Node) -> Option<String> = if key == "volumes" {
                mount_target
            } else {
                |_| None
            };
            if let (Value::Seq(items), Value::Seq(more)) =
                (&mut existing.value.value, entry.value.value)
            {
                concatenate(items, more, by);
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

/// A list of names as the mapping of those names to nothing: each text item, interpolated as
/// Compose reads it before merging, becomes a key at the item's own position. An item that isn't
/// text, or is empty once interpolated, names nothing, and a name given twice is kept once.
fn names_mapping(node: &mut Node) {
    let Value::Seq(items) = &node.value else {
        return;
    };
    let mut seen = HashSet::new();
    let entries = items
        .iter()
        .filter_map(|item| {
            let name = interpolate(item.as_str()?).value;
            if name.is_empty() || !seen.insert(name.clone()) {
                return None;
            }
            let key = Node {
                file: item.file,
                span: item.span.clone(),
                value: Value::Scalar(name),
            };
            let value = Node {
                file: item.file,
                span: item.span.end..item.span.end,
                value: Value::Null,
            };
            Some(Entry { key, value })
        })
        .collect();
    node.value = Value::Map(entries);
}

/// The container path a service volume mounts at, which Compose keys volumes by: a short volume's
/// target once interpolated, or a long one's `target`. None when it can't be read.
fn mount_target(node: &Node) -> Option<String> {
    match &node.value {
        Value::Scalar(text) => split_volume(&interpolate(text).value).map(|parts| parts.target),
        Value::Map(_) => value(node.get("target")),
        Value::Null | Value::Seq(_) => None,
    }
}

/// Appends the later items to `items`. A later item `by` gives a key takes the place of the earlier
/// item with that key, unless the two hold the same value, where the earlier stays where it is
/// written; one with a new key is appended. A later item `by` gives no key is appended unless an
/// earlier item holds the same value, compared as written.
fn concatenate(items: &mut Vec<Node>, later: Vec<Node>, by: fn(&Node) -> Option<String>) {
    let mut keyed: HashMap<String, usize> = HashMap::new();
    let mut earlier: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        match by(item) {
            Some(key) => {
                keyed.entry(key).or_insert(index);
            }
            None => earlier.entry(fingerprint(item)).or_default().push(index),
        }
    }
    for item in later {
        if let Some(key) = by(&item) {
            match keyed.get(&key).and_then(|&index| items.get_mut(index)) {
                Some(held) => {
                    if !same(held, &item) {
                        *held = item;
                    }
                }
                None => {
                    keyed.insert(key, items.len());
                    items.push(item);
                }
            }
            continue;
        }
        let held = earlier.get(&fingerprint(&item)).is_some_and(|indexes| {
            indexes
                .iter()
                .any(|&index| items.get(index).is_some_and(|earlier| same(earlier, &item)))
        });
        if !held {
            items.push(item);
        }
    }
}

/// Whether two nodes hold the same value, wherever they are written. A mapping is a set of
/// entries, so the order its keys are written in doesn't count; a list's order does.
fn same(a: &Node, b: &Node) -> bool {
    match (&a.value, &b.value) {
        (Value::Null, Value::Null) => true,
        (Value::Scalar(a), Value::Scalar(b)) => a == b,
        (Value::Seq(a), Value::Seq(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Map(a), Value::Map(b)) => a.len() == b.len() && same_entries(a, b),
        _ => false,
    }
}

/// Whether two mappings with as many entries hold the same entries, in any order. Their text keys
/// are each written once (the tree leaves out a key written again), so each entry is looked up by
/// its key; mappings whose keys aren't all text and distinct compare entry by entry, in order.
fn same_entries(a: &[Entry], b: &[Entry]) -> bool {
    match (by_key(a), by_key(b)) {
        (Some(a), Some(b)) => a
            .iter()
            .all(|(key, value)| b.get(key).is_some_and(|other| same(value, other))),
        _ => a
            .iter()
            .zip(b)
            .all(|(a, b)| same(&a.key, &b.key) && same(&a.value, &b.value)),
    }
}

/// A mapping's values by their keys; none when a key isn't text or is written twice.
fn by_key(entries: &[Entry]) -> Option<HashMap<&str, &Node>> {
    let map: HashMap<&str, &Node> = entries
        .iter()
        .map(|entry| Some((entry.key.as_str()?, &entry.value)))
        .collect::<Option<_>>()?;
    (map.len() == entries.len()).then_some(map)
}

/// A hash of a node's value that ignores where it is written, so equal values hash alike: a
/// mapping's entries add up, so their order doesn't count, as in [`same`].
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
                let sum = entries.iter().fold(0u64, |sum, entry| {
                    let mut one = DefaultHasher::new();
                    feed(&entry.key, &mut one);
                    feed(&entry.value, &mut one);
                    sum.wrapping_add(one.finish())
                });
                sum.hash(hasher);
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    feed(node, &mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::parse;

    /// `later`'s settings merged over `base`'s, two services' settings as `-f` merges them.
    fn merged(base: &str, later: &str) -> Node {
        let mut base = parse(0, base).unwrap();
        service(&mut base, parse(1, later).unwrap());
        base
    }

    /// A list's scalar items, or a mapping's keys.
    fn texts(node: Option<&Node>) -> Vec<&str> {
        let node = node.unwrap();
        match &node.value {
            Value::Map(entries) => entries.iter().filter_map(|e| e.key.as_str()).collect(),
            _ => node.items().iter().filter_map(Node::as_str).collect(),
        }
    }

    #[test]
    fn every_concatenated_key_concatenates() {
        let base = "ports: [\"80:80\"]\nvolumes: [\"a:/a\"]\nlinks: [db]\nprofiles: [dev]\nexpose: [\"3000\"]\ndepends_on: [db]\nnetworks: [front]\ncommand: [old]\n";
        let later = "ports: [\"80:80\", \"443:443\"]\nvolumes: [\"b:/b\"]\nlinks: [cache]\nprofiles: [debug]\nexpose: [\"3001\"]\ndepends_on: [cache]\nnetworks: [back]\ncommand: [new]\n";
        let node = merged(base, later);
        for (key, items) in [
            ("ports", ["80:80", "443:443"]),
            ("volumes", ["a:/a", "b:/b"]),
            ("links", ["db", "cache"]),
            ("profiles", ["dev", "debug"]),
            ("expose", ["3000", "3001"]),
            ("depends_on", ["db", "cache"]),
            ("networks", ["front", "back"]),
        ] {
            assert_eq!(texts(node.get(key)), items, "{key}");
        }
        // Any other list is replaced.
        assert_eq!(texts(node.get("command")), ["new"]);
    }

    #[test]
    fn a_list_merges_with_a_mapping_as_a_mapping_of_its_names() {
        for key in ["depends_on", "networks"] {
            let listed = format!("{key}: [db, cache]\n");
            let mapped = format!("{key}: {{db: {{condition: service_healthy}}, web: {{}}}}\n");
            // A list over a mapping adds its names, and leaves what the mapping says of the rest.
            let node = merged(&mapped, &listed);
            assert_eq!(texts(node.get(key)), ["db", "web", "cache"], "{key}");
            let db = node.get(key).unwrap().get("db").unwrap();
            assert_eq!(
                db.get("condition").unwrap().as_str(),
                Some("service_healthy")
            );
            // A mapping over a list merges into the list's names.
            let node = merged(&listed, &mapped);
            assert_eq!(texts(node.get(key)), ["db", "cache", "web"], "{key}");
            let db = node.get(key).unwrap().get("db").unwrap();
            assert_eq!(
                db.get("condition").unwrap().as_str(),
                Some("service_healthy")
            );
            // A name keeps where it is written.
            let names = node.get(key).unwrap().entries();
            assert_eq!((names[1].key.file, names[2].key.file), (0, 1));
        }
    }

    #[test]
    fn mappings_hold_the_same_value_whatever_their_key_order() {
        let a = parse(
            0,
            "{target: 80, published: \"8080\", nested: {x: 1, y: [2]}}",
        )
        .unwrap();
        let b = parse(
            1,
            "{nested: {y: [2], x: 1}, published: \"8080\", target: 80}",
        )
        .unwrap();
        assert!(same(&a, &b));
        assert_eq!(fingerprint(&a), fingerprint(&b));
        let c = parse(
            1,
            "{target: 80, published: \"8081\", nested: {x: 1, y: [2]}}",
        )
        .unwrap();
        assert!(!same(&a, &c));
        // Lists still compare in order.
        assert!(!same(
            &parse(0, "[1, 2]").unwrap(),
            &parse(0, "[2, 1]").unwrap()
        ));
    }
}
