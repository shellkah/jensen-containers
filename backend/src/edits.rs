//! Builds the edits findings suggest, and applies them the way the editor's `replaceRange` will.
//! An edit's span counts UTF-16 offsets as [`SourceText::span`] does, a `\r\n` break as one.

use std::ops::Range;

use crate::compose::EntryFact;
use crate::model::TextEdit;
use crate::source::SourceText;

/// An edit replacing `bytes` of `source` with `new_text`, its span in the editor's offsets.
pub fn replace(
    source: &SourceText,
    bytes: Range<usize>,
    new_text: String,
    title: String,
) -> TextEdit {
    TextEdit {
        file: source.path.clone(),
        span: source.span(bytes),
        new_text,
        title,
    }
}

/// The byte of `text` at UTF-16 offset `utf16`, counted as [`SourceText::span`] counts: the `\r`
/// of a `\r\n` adds nothing, so the break's offset maps back to its `\r`. An offset inside a
/// character stops before it, and one past the end clamps to the end.
pub fn byte_offset(text: &str, utf16: u32) -> usize {
    let target = usize::try_from(utf16).unwrap_or(usize::MAX);
    let mut units = 0usize;
    for (byte, character) in text.char_indices() {
        if units >= target {
            return byte;
        }
        if character == '\r' && text.as_bytes().get(byte + 1) == Some(&b'\n') {
            continue;
        }
        units += character.len_utf16();
        if units > target {
            return byte;
        }
    }
    text.len()
}

/// An edit removing `entries` from `source`, the file of the first entry: whole lines, line breaks
/// included, from the first entry's line to the last's, the lines between that hold no entry kept
/// in `new_text`. None without entries, when they aren't all in one file, or when one doesn't
/// stand alone on its line (see [`alone`]): never an edit that could break the file.
pub fn remove_entries(
    source: &SourceText,
    entries: &[&EntryFact],
    title: String,
) -> Option<TextEdit> {
    let file = entries.first()?.file;
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.file != file {
            return None;
        }
        lines.push(alone(source, entry)?);
    }
    lines.sort_unstable();
    lines.dedup();
    let (&first, &last) = (lines.first()?, lines.last()?);
    let new_text: String = (first + 1..last)
        .filter(|line| lines.binary_search(line).is_err())
        .map(|line| source.slice(source.line_bytes(line)))
        .collect();
    let bytes = source.line_bytes(first).start..source.line_bytes(last).end;
    Some(replace(source, bytes, new_text, title))
}

/// The line `entry` stands alone on, as a block `key: value` whose value is a scalar on the key's
/// line: only spaces before the key; only spaces and one `:` between the key and the value, so
/// no anchor, tag or `? `; a value starting after the key, not with `{`, `[`, `|`, `>`, `*`, `&`
/// or `!`, and ending on the key's line; then only spaces, or spaces and a `#` comment. None
/// otherwise: an alias's bytes are at its anchor, and a collection's may be stale.
fn alone(source: &SourceText, entry: &EntryFact) -> Option<usize> {
    let text = source.text.as_str();
    let (key, value) = (&entry.key, &entry.value);
    if key.is_empty() || value.is_empty() || key.end > value.start {
        return None;
    }
    let line = source.line_of(key.start);
    let bytes = source.line_bytes(line);
    let content = text.get(bytes.clone())?;
    let content = content
        .strip_suffix('\n')
        .map_or(content, |rest| rest.strip_suffix('\r').unwrap_or(rest));
    let content_end = bytes.start + content.len();
    // The value starts after the key, so ending before the line's break keeps it on the key's line.
    if value.end > content_end {
        return None;
    }
    let before = text.get(bytes.start..key.start)?;
    let between = text.get(key.end..value.start)?;
    let after = text.get(value.end..content_end)?.trim_start_matches(' ');
    let first = *text.as_bytes().get(value.start)?;
    let stands_alone = before.bytes().all(|b| b == b' ')
        && between.bytes().all(|b| b == b' ' || b == b':')
        && between.bytes().filter(|&b| b == b':').count() == 1
        && !b"{[|>*&!".contains(&first)
        && (after.is_empty() || after.starts_with('#'));
    stands_alone.then_some(line)
}

/// `text` with `edit` applied, as the editor's `replaceRange` applies it. A reversed span inserts
/// at its start.
pub fn apply(text: &str, edit: &TextEdit) -> String {
    let from = byte_offset(text, edit.span.from);
    let to = byte_offset(text, edit.span.to).max(from);
    let mut applied = String::with_capacity(text.len() + edit.new_text.len());
    applied.push_str(&text[..from]);
    applied.push_str(&edit.new_text);
    applied.push_str(&text[to..]);
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Span;

    #[test]
    fn byte_offset_inverts_span() {
        for text in [
            "a: 1\nb: x\n",
            "é🎵: x\n",
            "a: 1\r\nb: 2\r\nc: é\r\n",
            "x\r",
            "",
        ] {
            let source = SourceText::new("a.yml".into(), text.into());
            for byte in 0..=text.len() {
                if !text.is_char_boundary(byte) {
                    continue;
                }
                let offset = source.span(byte..byte).from;
                let back = byte_offset(text, offset);
                // The `\n` of a `\r\n` shares its offset with the `\r`, so it maps back there.
                let expected =
                    if byte > 0 && text[..byte].ends_with('\r') && text[byte..].starts_with('\n') {
                        byte - 1
                    } else {
                        byte
                    };
                assert_eq!(back, expected, "{text:?} at byte {byte}");
            }
        }
    }

    #[test]
    fn byte_offset_clamps_to_the_text() {
        assert_eq!(byte_offset("ab\n", 99), 3);
        assert_eq!(byte_offset("", 1), 0);
        // An offset inside a surrogate pair stops at the character before it.
        assert_eq!(byte_offset("🎵x", 1), 0);
        assert_eq!(byte_offset("🎵x", 2), 4);
    }

    #[test]
    fn replace_spans_the_bytes_and_apply_puts_the_text_there() {
        let source = SourceText::new("c.yaml".into(), "é: 80\n".into());
        let edit = replace(&source, 4..6, "\"90\"".into(), "t".into());
        assert_eq!(
            edit,
            TextEdit {
                file: "c.yaml".into(),
                span: Span { from: 3, to: 5 },
                new_text: "\"90\"".into(),
                title: "t".into(),
            }
        );
        assert_eq!(apply(&source.text, &edit), "é: \"90\"\n");
    }

    /// The edit removing the first `container_name` that `text`, loaded as a Compose file, sets.
    fn container_name_edit(text: &str) -> Option<TextEdit> {
        let project = crate::compose::one(text);
        let fact = project
            .facts
            .container_names
            .first()
            .expect("a container_name");
        let source = project.sources.get(fact.file).unwrap();
        remove_entries(source, &[fact], "t".into())
    }

    /// A hand-made entry of file 0: the key at `key`, the value at `value`.
    fn entry(key: Range<usize>, value: Range<usize>) -> EntryFact {
        EntryFact {
            service: None,
            file: 0,
            key,
            value,
            text: String::new(),
        }
    }

    #[test]
    fn remove_entries_takes_whole_lines_and_keeps_the_lines_between() {
        let text = "volumes:\n  data:\n    name: shared  # old\n    # why\n    driver: local\n    external: true\n  cache: {}\n";
        let project = crate::compose::one(text);
        let resource = &project.facts.resources[0];
        let name = resource.name_entry.as_ref().unwrap();
        let external = resource.external_entry.as_ref().unwrap();
        let source = project.sources.get(name.file).unwrap();
        // The order the entries come in doesn't matter.
        let edit = remove_entries(source, &[external, name], "t".into()).unwrap();
        assert_eq!(
            &text[edit.span.from as usize..edit.span.to as usize],
            "    name: shared  # old\n    # why\n    driver: local\n    external: true\n"
        );
        assert_eq!(edit.new_text, "    # why\n    driver: local\n");
        assert_eq!(
            apply(text, &edit),
            "volumes:\n  data:\n    # why\n    driver: local\n  cache: {}\n"
        );
        // The last line of a file without a line break goes too, and the break before it stays.
        let text = "services:\n  db:\n    image: pg\n    container_name: app-db";
        let edit = container_name_edit(text).unwrap();
        assert_eq!(apply(text, &edit), "services:\n  db:\n    image: pg\n");
    }

    #[test]
    fn remove_entries_on_a_crlf_file_removes_the_whole_break() {
        let text = "services:\r\n  db:\r\n    container_name: app-db\r\n    image: pg\r\n";
        let edit = container_name_edit(text).unwrap();
        assert_eq!(edit.new_text, "");
        assert_eq!(
            apply(text, &edit),
            "services:\r\n  db:\r\n    image: pg\r\n"
        );
    }

    #[test]
    fn remove_entries_refuses_an_entry_that_does_not_stand_alone() {
        for text in [
            // An alias: the value's bytes are at the anchor, before the key.
            "x-cn: &cn app-db\nservices:\n  db:\n    image: pg\n    container_name: *cn\n",
            // An anchored value, and a tagged one.
            "services:\n  db:\n    image: pg\n    container_name: &cn app-db\n",
            "services:\n  db:\n    image: pg\n    container_name: !!str app-db\n",
            // Flow style, the key sharing its line.
            "services:\n  db: {image: pg, container_name: app-db}\n",
            // A block scalar, and a plain scalar running onto the next line.
            "services:\n  db:\n    image: pg\n    container_name: |\n      app-db\n",
            "services:\n  db:\n    image: pg\n    container_name: app\n      db\n",
            // An explicit key.
            "services:\n  db:\n    image: pg\n    ? container_name\n    : app-db\n",
        ] {
            let project = crate::compose::one(text);
            assert!(project.errors.is_empty(), "{text:?}: {:?}", project.errors);
            let fact = project.facts.container_names.first().expect(text);
            let source = project.sources.get(fact.file).unwrap();
            assert_eq!(
                remove_entries(source, &[fact], "t".into()),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn remove_entries_refuses_collections_written_as_the_value() {
        for text in [
            "volumes:\n  data:\n    external:\n      name: shared\n",
            "volumes:\n  data:\n    external: {name: shared}\n",
        ] {
            let project = crate::compose::one(text);
            let fact = project.facts.resources[0].external_entry.as_ref().unwrap();
            let source = project.sources.get(fact.file).unwrap();
            assert_eq!(
                remove_entries(source, &[fact], "t".into()),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn remove_entries_refuses_what_shares_the_entry_line() {
        let source = SourceText::new("c.yaml".into(), "- container_name: app-db\n".into());
        // A dash before the key.
        assert_eq!(
            remove_entries(&source, &[&entry(2..16, 18..24)], "t".into()),
            None
        );
        // Text after the value that isn't a comment.
        let source = SourceText::new("c.yaml".into(), "container_name: app-db x\n".into());
        assert_eq!(
            remove_entries(&source, &[&entry(0..14, 16..22)], "t".into()),
            None
        );
        // A comment after it is fine.
        let source = SourceText::new("c.yaml".into(), "container_name: app-db # x\n".into());
        let edit = remove_entries(&source, &[&entry(0..14, 16..22)], "t".into()).unwrap();
        assert_eq!(apply(&source.text, &edit), "");
        // A value before its key, an empty value, and ranges past the text: never a reversed range.
        for (key, value) in [
            (16..22, 0..14),
            (0..14, 16..16),
            (0..14, 16..99),
            (90..95, 96..99),
        ] {
            assert_eq!(
                remove_entries(&source, &[&entry(key, value)], "t".into()),
                None
            );
        }
    }

    #[test]
    fn remove_entries_refuses_entries_in_two_files_or_none() {
        let source = SourceText::new("c.yaml".into(), "a: 1\nb: 2\n".into());
        let other = EntryFact {
            file: 1,
            ..entry(5..6, 8..9)
        };
        assert_eq!(
            remove_entries(&source, &[&entry(0..1, 3..4), &other], "t".into()),
            None
        );
        assert_eq!(remove_entries(&source, &[], "t".into()), None);
        let edit = remove_entries(&source, &[&entry(0..1, 3..4)], "t".into()).unwrap();
        assert_eq!(apply(&source.text, &edit), "b: 2\n");
    }

    #[test]
    fn an_edit_on_a_crlf_file_applies_at_its_bytes() {
        let text = "a: 1\r\nb: 2\r\nc: x\r\n";
        let source = SourceText::new("c.yaml".into(), text.into());
        let edit = replace(&source, 15..16, "y".into(), "t".into());
        assert_eq!(edit.span, Span { from: 13, to: 14 });
        assert_eq!(apply(text, &edit), "a: 1\r\nb: 2\r\nc: y\r\n");
    }
}
