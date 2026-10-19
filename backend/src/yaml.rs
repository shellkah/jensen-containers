//! A YAML tree whose every node keeps the byte range it was written at.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use granit_parser::{Event, Marker, Parser, ScalarStyle, ScanError};

use crate::source::FileId;

/// The most nodes one parse creates, alias and anchor copies included. Alias expansion (a "billion
/// laughs" file) would otherwise exhaust the wasm call's memory; a real Compose file holds a few
/// thousand.
pub const MAX_NODES: usize = 200_000;

/// The most scalar text one parse creates, alias and anchor copies included: aliases copying a long
/// scalar exhaust memory long before they pass [`MAX_NODES`].
pub const MAX_SCALAR_BYTES: usize = 16 << 20;

/// The deepest the tree goes once aliases are expanded. The parser stops at 255 flow and 255 block
/// levels, but aliases of aliases stack deeper, and cloning or dropping a node recurses through
/// its depth: about 5,500 levels already overflow a 1 MiB stack, the wasm default.
pub const MAX_DEPTH: usize = 1_000;

/// A YAML value and the bytes of `file` it was written at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub file: FileId,
    pub span: Range<usize>,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Scalar(String),
    Seq(Vec<Node>),
    Map(Vec<Entry>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: Node,
    pub value: Node,
}

/// Why a file isn't YAML the tree can hold, and the byte it stopped at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YamlError {
    pub message: String,
    pub at: usize,
}

/// Reads the first YAML document of `text` into a tree; an empty document is `Null` at `0..0`.
///
/// An alias is a copy of its anchored node, spans included, so they point where the anchor is
/// written. A plain `<<` key whose value is a mapping, or a list of mappings, is replaced by their
/// entries, appended after the mapping's own; keys the mapping already has are skipped, and in a
/// list the earlier mappings win. Tags are ignored.
pub fn parse(file: FileId, text: &str) -> Result<Node, YamlError> {
    let mut tree = Tree {
        file,
        text,
        open: Vec::new(),
        anchors: HashMap::new(),
        made: Made::default(),
        root: None,
    };
    for item in Parser::new_from_str(text) {
        let (event, span) = item.map_err(|error| scan_error(&error, text))?;
        let bytes = span
            .byte_range()
            .unwrap_or_else(|| byte_of(&span.start, text)..byte_of(&span.end, text));
        match event {
            Event::DocumentEnd | Event::StreamEnd => break,
            Event::Alias(id) => tree.alias(id, bytes)?,
            Event::Scalar(value, style, anchor, _) => tree.scalar(&value, style, anchor, bytes)?,
            Event::SequenceStart(_, anchor, _) => {
                tree.open(Open::Seq(Vec::new()), anchor, bytes)?
            }
            Event::MappingStart(_, anchor, _) => tree.open(
                Open::Map {
                    entries: Vec::new(),
                    merges: Vec::new(),
                    key: None,
                },
                anchor,
                bytes,
            )?,
            Event::SequenceEnd | Event::MappingEnd => tree.close(bytes)?,
            _ => {}
        }
    }
    Ok(tree.root.unwrap_or(Node {
        file,
        span: 0..0,
        value: Value::Null,
    }))
}

impl Node {
    /// The value of this mapping's first entry whose key is the scalar `key`.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entry(key).map(|entry| &entry.value)
    }

    /// This mapping's first entry whose key is the scalar `key`.
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries()
            .iter()
            .find(|entry| entry.key.as_str() == Some(key))
    }

    /// This mapping's entries, or none when it isn't a mapping.
    pub fn entries(&self) -> &[Entry] {
        match &self.value {
            Value::Map(entries) => entries,
            _ => &[],
        }
    }

    /// This sequence's items, or none when it isn't a sequence.
    pub fn items(&self) -> &[Node] {
        match &self.value {
            Value::Seq(items) => items,
            _ => &[],
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Scalar(text) => Some(text),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    /// Where this node's own text ends: the end of the last scalar written inside it. A block
    /// collection's span runs on over the comments and blank lines after it; this doesn't. Values
    /// that came from an anchor elsewhere don't count, and a node with no scalar of its own, such as
    /// `{}`, ends where its span does.
    pub fn last_byte(&self) -> usize {
        let mut last = None;
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            match &node.value {
                Value::Null | Value::Scalar(_) => {
                    let inside = node.file == self.file
                        && node.span.start >= self.span.start
                        && node.span.end <= self.span.end;
                    if inside {
                        last = last.max(Some(node.span.end));
                    }
                }
                Value::Seq(items) => pending.extend(items),
                Value::Map(entries) => {
                    pending.extend(entries.iter().flat_map(|entry| [&entry.key, &entry.value]));
                }
            }
        }
        last.unwrap_or(self.span.end)
    }
}

/// The tree being folded from the parser's events.
struct Tree<'t> {
    file: FileId,
    text: &'t str,
    /// The collections started and not yet ended, innermost last.
    open: Vec<Frame>,
    /// A copy of each finished anchored node and its weight, by the parser's anchor id.
    anchors: HashMap<usize, (Node, Weight)>,
    made: Made,
    root: Option<Node>,
}

/// What a node holds: its nodes, itself included, the bytes of its scalars, and its height.
#[derive(Clone, Copy, Debug, Default)]
struct Weight {
    nodes: usize,
    bytes: usize,
    height: usize,
}

impl Weight {
    fn leaf(bytes: usize) -> Weight {
        Weight {
            nodes: 1,
            bytes,
            height: 1,
        }
    }

    /// Adds a child's weight to its collection's.
    fn hold(&mut self, child: Weight) {
        self.nodes += child.nodes;
        self.bytes += child.bytes;
        self.height = self.height.max(child.height + 1);
    }

    /// Takes away the nodes and bytes that left the collection; its height stays an upper bound.
    fn shed(&mut self, gone: Weight) {
        self.nodes = self.nodes.saturating_sub(gone.nodes);
        self.bytes = self.bytes.saturating_sub(gone.bytes);
    }
}

/// The nodes and scalar bytes one parse has made, alias and anchor copies included, held under
/// [`MAX_NODES`] and [`MAX_SCALAR_BYTES`].
#[derive(Default)]
struct Made {
    nodes: usize,
    bytes: usize,
}

impl Made {
    fn add(&mut self, weight: Weight, at: usize) -> Result<(), YamlError> {
        let nodes = self.nodes.saturating_add(weight.nodes);
        let bytes = self.bytes.saturating_add(weight.bytes);
        let message = if nodes > MAX_NODES {
            format!(
                "the file is too large: it would hold more than {MAX_NODES} YAML nodes once its aliases are expanded"
            )
        } else if bytes > MAX_SCALAR_BYTES {
            format!(
                "the file is too large: its values would hold more than {} MiB of text once its aliases are expanded",
                MAX_SCALAR_BYTES >> 20
            )
        } else {
            *self = Made { nodes, bytes };
            return Ok(());
        };
        Err(YamlError { message, at })
    }
}

/// A collection that has started and not yet ended.
struct Frame {
    start: usize,
    anchor: usize,
    /// What the collection holds so far.
    weight: Weight,
    kind: Open,
}

enum Open {
    Seq(Vec<Node>),
    Map {
        entries: Vec<Entry>,
        /// The values of plain `<<` keys, merged in when the mapping ends.
        merges: Vec<Node>,
        /// A key waiting for its value, and whether it is a plain `<<`.
        key: Option<(Node, bool)>,
    },
}

impl Tree<'_> {
    fn scalar(
        &mut self,
        text: &str,
        style: ScalarStyle,
        anchor: usize,
        bytes: Range<usize>,
    ) -> Result<(), YamlError> {
        let at = bytes.start;
        let plain = matches!(style, ScalarStyle::Plain);
        let value = if plain && matches!(text, "" | "~" | "null" | "Null" | "NULL") {
            Value::Null
        } else {
            Value::Scalar(text.to_owned())
        };
        let node = Node {
            file: self.file,
            span: bytes,
            value,
        };
        let weight = Weight::leaf(node.as_str().map_or(0, str::len));
        self.made.add(weight, at)?;
        self.finish(node, weight, anchor, plain && text == "<<", at)
    }

    fn open(&mut self, kind: Open, anchor: usize, bytes: Range<usize>) -> Result<(), YamlError> {
        let weight = Weight::leaf(0);
        self.made.add(weight, bytes.start)?;
        self.open.push(Frame {
            start: bytes.start,
            anchor,
            weight,
            kind,
        });
        Ok(())
    }

    fn close(&mut self, bytes: Range<usize>) -> Result<(), YamlError> {
        let Some(mut frame) = self.open.pop() else {
            return Ok(());
        };
        let value = match frame.kind {
            Open::Seq(items) => Value::Seq(items),
            Open::Map {
                entries, merges, ..
            } => Value::Map(merge(entries, merges, &mut frame.weight)),
        };
        let node = Node {
            file: self.file,
            span: frame.start..bytes.end.max(frame.start),
            value,
        };
        self.finish(node, frame.weight, frame.anchor, false, bytes.start)
    }

    /// Places a copy of the anchored node, once the caps allow it: the copy is what costs.
    fn alias(&mut self, id: usize, bytes: Range<usize>) -> Result<(), YamlError> {
        let at = bytes.start;
        let Some((node, weight)) = self.anchors.get(&id) else {
            // The parser knows the anchor, so the alias sits inside the node it names.
            let alias = self.text.get(bytes).unwrap_or_default();
            return Err(YamlError {
                message: format!(
                    "unknown alias {alias}: an alias can't refer to the node that holds it"
                ),
                at,
            });
        };
        if self.open.len() + weight.height > MAX_DEPTH {
            return Err(YamlError {
                message: format!(
                    "the file nests too deeply: more than {MAX_DEPTH} levels once its aliases are expanded"
                ),
                at,
            });
        }
        self.made.add(*weight, at)?;
        let (node, weight) = (node.clone(), *weight);
        self.place(node, weight, false);
        Ok(())
    }

    /// Keeps a copy of an anchored node, then places it in the collection that holds it.
    fn finish(
        &mut self,
        node: Node,
        weight: Weight,
        anchor: usize,
        merge_key: bool,
        at: usize,
    ) -> Result<(), YamlError> {
        if anchor != 0 {
            self.made.add(weight, at)?;
            self.anchors.insert(anchor, (node.clone(), weight));
        }
        self.place(node, weight, merge_key);
        Ok(())
    }

    fn place(&mut self, node: Node, weight: Weight, merge_key: bool) {
        let Some(frame) = self.open.last_mut() else {
            if self.root.is_none() {
                self.root = Some(node);
            }
            return;
        };
        frame.weight.hold(weight);
        match &mut frame.kind {
            Open::Seq(items) => items.push(node),
            Open::Map {
                entries,
                merges,
                key: waiting,
            } => match waiting.take() {
                None => *waiting = Some((node, merge_key)),
                Some((key, true)) if is_merge_source(&node) => {
                    // The `<<` key leaves the tree; its value's entries are merged in at the end.
                    frame.weight.shed(measure(&key));
                    merges.push(node);
                }
                Some((key, _)) => entries.push(Entry { key, value: node }),
            },
        }
    }
}

/// A mapping, or a list of mappings: what a `<<` key merges.
fn is_merge_source(node: &Node) -> bool {
    match &node.value {
        Value::Map(_) => true,
        Value::Seq(items) => items.iter().all(|item| matches!(item.value, Value::Map(_))),
        _ => false,
    }
}

/// Appends the entries of the `<<` values after the mapping's own, skipping keys it already has,
/// and sheds from `weight` what leaves the tree: the merged collections and the skipped entries.
fn merge(mut entries: Vec<Entry>, merges: Vec<Node>, weight: &mut Weight) -> Vec<Entry> {
    if merges.is_empty() {
        return entries;
    }
    let mut keys: HashSet<String> = entries
        .iter()
        .filter_map(|entry| entry.key.as_str().map(str::to_owned))
        .collect();
    for source in merges {
        // The collections themselves leave: the mapping, or the list and each of its mappings.
        let (maps, collections) = match source.value {
            Value::Map(map) => (vec![map], 1),
            Value::Seq(items) => {
                let collections = 1 + items.len();
                let maps = items
                    .into_iter()
                    .filter_map(|item| match item.value {
                        Value::Map(map) => Some(map),
                        _ => None,
                    })
                    .collect();
                (maps, collections)
            }
            Value::Null | Value::Scalar(_) => (Vec::new(), 0),
        };
        weight.shed(Weight {
            nodes: collections,
            ..Weight::default()
        });
        for entry in maps.into_iter().flatten() {
            let fresh = entry
                .key
                .as_str()
                .is_none_or(|key| keys.insert(key.to_owned()));
            if fresh {
                entries.push(entry);
            } else {
                weight.shed(measure(&entry.key));
                weight.shed(measure(&entry.value));
            }
        }
    }
    entries
}

/// The nodes and scalar bytes of `node` and everything under it; its height isn't measured.
fn measure(node: &Node) -> Weight {
    let mut weight = Weight::default();
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        weight.nodes += 1;
        weight.bytes += node.as_str().map_or(0, str::len);
        pending.extend(node.items());
        pending.extend(
            node.entries()
                .iter()
                .flat_map(|entry| [&entry.key, &entry.value]),
        );
    }
    weight
}

fn scan_error(error: &ScanError, text: &str) -> YamlError {
    let message = error.info();
    YamlError {
        message: if message.is_empty() {
            "the file is not valid YAML".to_owned()
        } else {
            message
        },
        at: byte_of(error.marker(), text),
    }
}

/// A marker's byte offset. A parser over a `&str` always records it; the character index is the
/// fallback.
fn byte_of(marker: &Marker, text: &str) -> usize {
    marker.byte_offset().unwrap_or_else(|| {
        text.char_indices()
            .nth(marker.index())
            .map_or(text.len(), |(byte, _)| byte)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'t>(text: &'t str, node: &Node) -> &'t str {
        &text[node.span.clone()]
    }

    #[test]
    fn merge_keys_bring_the_anchor_entries_with_their_spans() {
        let text = "x-base: &base\n  image: pg\n  ports: [\"5432:5432\"]\nservices:\n  db:\n    <<: *base\n    image: own\n";
        let root = parse(0, text).unwrap();
        let db = root.get("services").unwrap().get("db").unwrap();
        assert_eq!(db.get("image").unwrap().as_str(), Some("own"));
        assert_eq!(
            &text[db.get("ports").unwrap().items()[0].span.clone()],
            "\"5432:5432\""
        );
        assert!(db.get("<<").is_none());
    }

    #[test]
    fn scalar_spans_cover_their_quotes() {
        let text = "a: \"x\"\nb: 'y'\nc: z\n\"d e\": [\"f\"]\n";
        let root = parse(7, text).unwrap();
        assert_eq!(at(text, root.get("a").unwrap()), "\"x\"");
        assert_eq!(at(text, root.get("b").unwrap()), "'y'");
        assert_eq!(at(text, root.get("c").unwrap()), "z");
        assert_eq!(at(text, &root.entry("a").unwrap().key), "a");
        let quoted = root.entry("d e").unwrap();
        assert_eq!(at(text, &quoted.key), "\"d e\"");
        assert_eq!(at(text, &quoted.value), "[\"f\"]");
        assert_eq!(quoted.value.items()[0].as_str(), Some("f"));
        assert_eq!(quoted.value.file, 7);
    }

    #[test]
    fn empty_and_null_values_are_null() {
        let text = "a:\nb: ~\nc: null\nd: ''\ne: Null\nf: NULL\ng: \"null\"\nh: 0\n";
        let root = parse(0, text).unwrap();
        for key in ["a", "b", "c", "e", "f"] {
            assert!(root.get(key).unwrap().is_null(), "{key} should be null");
        }
        assert_eq!(root.get("d").unwrap().as_str(), Some(""));
        assert_eq!(root.get("g").unwrap().as_str(), Some("null"));
        assert_eq!(root.get("h").unwrap().as_str(), Some("0"));
        assert!(!root.get("d").unwrap().is_null());
        assert_eq!(root.get("a").unwrap().as_str(), None);
    }

    #[test]
    fn an_empty_document_is_null_at_the_start() {
        for text in ["", "# only a comment\n", "\n\n"] {
            let root = parse(3, text).unwrap();
            assert_eq!(
                root,
                Node {
                    file: 3,
                    span: 0..0,
                    value: Value::Null
                },
                "{text:?}"
            );
        }
    }

    #[test]
    fn earlier_maps_win_in_a_merge_list() {
        let text = "x-a: &a\n  image: from-a\n  only_a: 1\nx-b: &b\n  image: from-b\n  only_b: 2\nsvc:\n  <<: [*a, *b]\n  own: 3\n";
        let root = parse(0, text).unwrap();
        let svc = root.get("svc").unwrap();
        let image = svc.get("image").unwrap();
        assert_eq!(image.as_str(), Some("from-a"));
        assert_eq!(image.span.start, text.find("from-a").unwrap());
        let keys: Vec<_> = svc
            .entries()
            .iter()
            .map(|entry| entry.key.as_str().unwrap())
            .collect();
        assert_eq!(keys, ["own", "image", "only_a", "only_b"]);
    }

    #[test]
    fn a_quoted_merge_key_or_a_scalar_merge_value_is_an_ordinary_entry() {
        let text = "x: &x {k: v}\na:\n  \"<<\": *x\nb:\n  <<: plain\n";
        let root = parse(0, text).unwrap();
        let a = root.get("a").unwrap();
        assert_eq!(a.get("<<").unwrap().get("k").unwrap().as_str(), Some("v"));
        assert!(a.get("k").is_none());
        assert_eq!(
            root.get("b").unwrap().get("<<").unwrap().as_str(),
            Some("plain")
        );
    }

    #[test]
    fn aliases_clone_the_anchored_node() {
        let text = "x-env: &env\n  A: \"1\"\nservices:\n  a:\n    environment: *env\n  b:\n    environment: *env\n";
        let root = parse(0, text).unwrap();
        let services = root.get("services").unwrap();
        let a = services.get("a").unwrap().get("environment").unwrap();
        let b = services.get("b").unwrap().get("environment").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, root.get("x-env").unwrap());
        let value = a.get("A").unwrap();
        assert_eq!(value.as_str(), Some("1"));
        assert_eq!(value.span.start, text.find("\"1\"").unwrap());
    }

    #[test]
    fn unknown_and_self_referring_aliases_are_errors() {
        let unknown = parse(0, "a: *nope\n").unwrap_err();
        assert!(unknown.message.contains("unknown"), "{unknown:?}");
        assert_eq!(unknown.at, 3);

        let inside = parse(0, "a: &a [*a]\n").unwrap_err();
        assert!(inside.message.contains("*a"), "{inside:?}");
        assert_eq!(inside.at, 7);
    }

    #[test]
    fn syntax_errors_carry_a_message_and_an_offset() {
        let error = parse(0, "services:\n  db:\n    ports: [\n  - x\n").unwrap_err();
        assert_eq!(error.at, 31);
        assert!(
            error.message.contains("only valid inside a block"),
            "{error:?}"
        );
    }

    #[test]
    fn only_the_first_document_counts() {
        let root = parse(0, "a: 1\n---\nb: 2\n").unwrap();
        assert_eq!(root.get("a").unwrap().as_str(), Some("1"));
        assert!(root.get("b").is_none());
        assert_eq!(root.entries().len(), 1);

        let broken_later = parse(0, "a: 1\n---\nb: [\n").unwrap();
        assert_eq!(broken_later.get("a").unwrap().as_str(), Some("1"));
    }

    #[test]
    fn a_block_value_ends_at_its_own_text() {
        let text = "a:\n  b: 1\n  c: two\n# trailing comment\n\nd: 2\n";
        let root = parse(0, text).unwrap();
        let a = root.get("a").unwrap();
        let two_end = text.find("two").unwrap() + 3;
        assert!(
            a.span.end > two_end,
            "the block span runs on to the next key"
        );
        assert_eq!(a.last_byte(), two_end);
        assert_eq!(root.get("d").unwrap().last_byte(), text.len() - 1);
        assert_eq!(root.last_byte(), text.len() - 1);
    }

    #[test]
    fn merged_and_aliased_values_leave_last_byte_on_the_node_text() {
        let text = "x: &x\n  k: v\ny:\n  <<: *x\n  own: 1\n# c\nz: [*x]\nw: {}\n";
        let root = parse(0, text).unwrap();
        let y = root.get("y").unwrap();
        assert_eq!(y.get("k").unwrap().as_str(), Some("v"));
        assert_eq!(y.last_byte(), text.find("own: 1").unwrap() + 6);
        let z = root.get("z").unwrap();
        assert_eq!(z.last_byte(), z.span.end);
        let w = root.get("w").unwrap();
        assert_eq!(at(text, w), "{}");
        assert_eq!(w.last_byte(), w.span.end);
    }

    #[test]
    fn accessors_answer_empty_for_the_wrong_shape() {
        let root = parse(0, "a: [1, 2]\nb: x\n").unwrap();
        let list = root.get("a").unwrap();
        assert_eq!(list.items().len(), 2);
        assert!(list.entries().is_empty());
        assert!(list.get("1").is_none());
        assert_eq!(list.as_str(), None);
        let scalar = root.get("b").unwrap();
        assert!(scalar.items().is_empty());
        assert!(scalar.entries().is_empty());
        assert!(root.items().is_empty());
    }

    #[test]
    fn alias_expansion_past_the_node_cap_is_an_error() {
        let level =
            |name: &str, alias: &str| format!("{name}: &{name} [{}]\n", [alias; 10].join(", "));
        let text = [
            level("a0", "x"),
            level("a1", "*a0"),
            level("a2", "*a1"),
            level("a3", "*a2"),
            format!("b: [{}]\n", ["*a3"; 20].join(", ")),
        ]
        .concat();
        assert!(text.len() < 400, "the text itself stays small");

        let error = parse(0, &text).unwrap_err();

        assert!(error.message.contains("too large"), "{error:?}");
        assert!(error.message.contains(&MAX_NODES.to_string()), "{error:?}");
        // a0..a3 hold 11, 111, 1111 and 11111 nodes, each created twice (the tree and the anchor's
        // copy), with the root and the keys; every *a3 then adds 11111, and the 16th passes the cap.
        let sixteenth = text.match_indices("*a3").nth(15).unwrap().0;
        assert_eq!(error.at, sixteenth);
    }

    #[test]
    fn alias_expansion_past_the_scalar_text_cap_is_an_error() {
        let level =
            |name: &str, alias: &str| format!("{name}: &{name} [{}]\n", [alias; 16].join(", "));
        let text = [
            format!("a: &a {}\n", "x".repeat(1024)),
            level("b", "*a"),
            level("c", "*b"),
            level("d", "*c"),
            format!("e: [{}]\n", ["*d"; 16].join(", ")),
        ]
        .concat();
        assert!(text.len() < 1500, "the text itself stays small");

        let error = parse(0, &text).unwrap_err();

        assert!(error.message.contains("too large"), "{error:?}");
        assert!(error.message.contains("16 MiB"), "{error:?}");
        // a..d hold 1 KiB, 16 KiB, 256 KiB and 4 MiB of text, each made twice (the tree and the
        // anchor's copy), 8.5 MiB in all; the 2nd *d brings it past 16 MiB.
        let second = text.match_indices("*d").nth(1).unwrap().0;
        assert_eq!(error.at, second);
    }

    #[test]
    fn alias_nesting_past_the_depth_cap_is_an_error() {
        let nested = |name: &str, inner: &str| {
            format!(
                "{name}: &{name} {}{inner}{}\n",
                "[".repeat(250),
                "]".repeat(250)
            )
        };
        let text = [
            nested("a0", "x"),
            nested("a1", "*a0"),
            nested("a2", "*a1"),
            nested("a3", "*a2"),
        ]
        .concat();

        let error = parse(0, &text).unwrap_err();

        assert!(error.message.contains("too deeply"), "{error:?}");
        assert!(error.message.contains(&MAX_DEPTH.to_string()), "{error:?}");
        // a2 is 751 levels deep (250 per line and the x), and *a2 sits 251 levels down in a3.
        assert_eq!(error.at, text.find("*a2").unwrap());
    }

    #[test]
    fn nesting_the_parser_allows_without_aliases_is_kept() {
        let text = format!("a: {}x{}\n", "[".repeat(250), "]".repeat(250));
        let mut node = parse(0, &text).unwrap().get("a").unwrap().clone();
        let mut depth = 1;
        while let [inner] = node.items() {
            node = inner.clone();
            depth += 1;
        }
        assert_eq!((depth, node.as_str()), (251, Some("x")));
    }
}
