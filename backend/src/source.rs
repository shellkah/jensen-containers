//! Turns byte offsets into the positions the editor counts: zero-based lines, and columns and
//! offsets in UTF-16 code units.

use std::ops::Range;

use crate::model::{SourceRef, Span};

/// The index of a file in [`Sources`].
pub type FileId = usize;

/// A file's path and text, with where each line starts.
///
/// Lines break at `\n`, and a line's bytes include its break. A `\r` before the `\n` belongs to
/// the line but is never part of a column. A byte past the end, or inside a character, clamps to
/// the character boundary before it, so no method panics. The line starts are worked out by
/// `new`: build a new one rather than change `text`.
///
/// `new` drops a leading byte order mark, as editors and `TextDecoder` do, so lines and offsets
/// count the text the person sees. Parse `text`, not the file's raw contents, so the YAML tree's
/// byte ranges match.
#[derive(Clone, Debug)]
pub struct SourceText {
    pub path: String,
    pub text: String,
    lines: Vec<LineStart>,
}

/// Where a line starts, in bytes and in UTF-16 units from the start of the file.
#[derive(Clone, Copy, Debug, Default)]
struct LineStart {
    byte: usize,
    utf16: usize,
}

impl SourceText {
    pub fn new(path: String, mut text: String) -> SourceText {
        if text.starts_with('\u{feff}') {
            text.drain(..'\u{feff}'.len_utf8());
        }
        let mut lines = vec![LineStart { byte: 0, utf16: 0 }];
        let mut utf16 = 0;
        for (byte, character) in text.char_indices() {
            utf16 += character.len_utf16();
            if character == '\n' {
                lines.push(LineStart {
                    byte: byte + 1,
                    utf16,
                });
            }
        }
        SourceText { path, text, lines }
    }

    /// The zero-based line and UTF-16 column of `byte`.
    pub fn source_ref(&self, byte: usize) -> SourceRef {
        let byte = self.clamp(byte);
        let line = self.line_of(byte);
        let start = self.line_bytes(line).start;
        let column = self.utf16_len(start..byte.min(self.content_end(line)));
        SourceRef {
            file: self.path.clone(),
            line: to_u32(line),
            column: to_u32(column),
        }
    }

    /// `bytes` as UTF-16 offsets from the start of the file. A reversed range is empty at its start.
    pub fn span(&self, bytes: Range<usize>) -> Span {
        let bytes = self.clamp_range(bytes);
        Span {
            from: to_u32(self.utf16_offset(bytes.start)),
            to: to_u32(self.utf16_offset(bytes.end)),
        }
    }

    /// The zero-based line `byte` is on. A line's break is on that line.
    pub fn line_of(&self, byte: usize) -> usize {
        let byte = self.clamp(byte);
        self.lines
            .partition_point(|line| line.byte <= byte)
            .saturating_sub(1)
    }

    /// The bytes of `line` with its line break; an empty range at the end for a line past the last.
    pub fn line_bytes(&self, line: usize) -> Range<usize> {
        let end = self.text.len();
        match self.lines.get(line) {
            Some(start) => start.byte..self.lines.get(line + 1).map_or(end, |next| next.byte),
            None => end..end,
        }
    }

    /// The text of `bytes`, clamped as [`SourceText::span`] clamps them.
    pub fn slice(&self, bytes: Range<usize>) -> &str {
        &self.text[self.clamp_range(bytes)]
    }

    /// Where `line`'s columns end: before its `\n`, and before a `\r` just ahead of it.
    fn content_end(&self, line: usize) -> usize {
        let bytes = self.line_bytes(line);
        let content = self.text.get(bytes.clone()).unwrap_or_default();
        let content = content
            .strip_suffix('\n')
            .map_or(content, |rest| rest.strip_suffix('\r').unwrap_or(rest));
        bytes.start + content.len()
    }

    fn utf16_offset(&self, byte: usize) -> usize {
        let start = self
            .lines
            .get(self.line_of(byte))
            .copied()
            .unwrap_or_default();
        start.utf16 + self.utf16_len(start.byte..byte)
    }

    fn utf16_len(&self, bytes: Range<usize>) -> usize {
        let text = self.text.get(bytes).unwrap_or_default();
        text.chars().map(char::len_utf16).sum()
    }

    fn clamp(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    fn clamp_range(&self, bytes: Range<usize>) -> Range<usize> {
        let start = self.clamp(bytes.start);
        start..self.clamp(bytes.end).max(start)
    }
}

/// A position as the model writes it; a file past 4 GiB saturates instead of wrapping.
fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The files read for one answer, each known by the [`FileId`] `add` gave it.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    files: Vec<SourceText>,
}

impl Sources {
    pub fn add(&mut self, source: SourceText) -> FileId {
        self.files.push(source);
        self.files.len() - 1
    }

    pub fn get(&self, id: FileId) -> Option<&SourceText> {
        self.files.get(id)
    }

    /// The first file added with `path`.
    pub fn find(&self, path: &str) -> Option<FileId> {
        self.files.iter().position(|source| source.path == path)
    }

    /// Every file's path, in the order they were added.
    pub fn paths(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|source| source.path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_code_units() {
        let source = SourceText::new("a.yml".into(), "é🎵: x\n".into());
        assert_eq!(
            source.source_ref(8),
            SourceRef {
                file: "a.yml".into(),
                line: 0,
                column: 5
            }
        );
        assert_eq!(source.span(8..9), Span { from: 5, to: 6 });
    }

    #[test]
    fn crlf_lines_keep_columns_and_offsets() {
        let source = SourceText::new("a.yml".into(), "a: 1\r\nb: x\r\n".into());
        assert_eq!(
            source.source_ref(9),
            SourceRef {
                file: "a.yml".into(),
                line: 1,
                column: 3
            }
        );
        assert_eq!(source.span(9..10), Span { from: 9, to: 10 });
    }

    #[test]
    fn line_bytes_cover_the_line_break() {
        let source = SourceText::new("a.yml".into(), "a: 1\nb: 2\n".into());
        assert_eq!(source.line_bytes(1), 5..10);
        assert_eq!(source.line_of(9), 1);
        assert_eq!(source.slice(source.line_bytes(1)), "b: 2\n");
        // The text after the last break is an empty last line.
        assert_eq!(source.line_of(10), 2);
        assert_eq!(source.line_bytes(2), 10..10);
    }

    #[test]
    fn a_crlf_break_is_never_part_of_a_column() {
        let source = SourceText::new("a.yml".into(), "a: 1\r\nb: x\r\n".into());
        assert_eq!(source.source_ref(4).column, 4, "at the \\r");
        assert_eq!(source.source_ref(5).column, 4, "at the \\n");
        assert_eq!((source.line_of(5), source.line_of(6)), (0, 1));
        assert_eq!(source.line_bytes(0), 0..6);
    }

    #[test]
    fn bytes_inside_a_character_or_past_the_end_clamp() {
        let source = SourceText::new("a.yml".into(), "é🎵: x\n".into());
        // Byte 3 is inside 🎵 (bytes 2..6), so it clamps to 2, after é.
        assert_eq!(source.source_ref(3).column, 1);
        assert_eq!(source.span(1..4), Span { from: 0, to: 1 });
        assert_eq!(source.slice(1..7), "é🎵:");
        // Past the end clamps to the end: the empty line after the last break.
        assert_eq!(
            source.source_ref(99),
            SourceRef {
                file: "a.yml".into(),
                line: 1,
                column: 0
            }
        );
        assert_eq!(source.span(8..99), Span { from: 5, to: 7 });
        assert_eq!(source.slice(8..99), "x\n");
        assert_eq!(source.line_bytes(7), 10..10);
        // A reversed range is empty at its start.
        let reversed = Range { start: 8, end: 2 };
        assert_eq!(source.slice(reversed.clone()), "");
        assert_eq!(source.span(reversed), Span { from: 5, to: 5 });
    }

    #[test]
    fn a_leading_bom_is_dropped_as_editors_drop_it() {
        let source = SourceText::new("a.yml".into(), "\u{feff}a: x\n".into());
        assert_eq!(source.text, "a: x\n");
        let root = crate::yaml::parse(0, &source.text).unwrap();
        let entry = root.entry("a").unwrap();
        assert_eq!(entry.key.span, 0..1);
        assert_eq!(source.source_ref(entry.value.span.start).column, 3);
        assert_eq!(
            source.span(entry.value.span.clone()),
            Span { from: 3, to: 4 }
        );
        // Only a leading one: a U+FEFF later in the text is text.
        let later = SourceText::new("a.yml".into(), "a: \u{feff}\n".into());
        assert_eq!(later.text, "a: \u{feff}\n");
    }

    #[test]
    fn sources_hand_out_ids_and_find_files_by_path() {
        let mut sources = Sources::default();
        let a = sources.add(SourceText::new("a.yml".into(), "a: 1\n".into()));
        let b = sources.add(SourceText::new("infra/b.yml".into(), "b: 2\n".into()));
        assert_ne!(a, b);
        assert_eq!(sources.find("infra/b.yml"), Some(b));
        assert_eq!(sources.find("c.yml"), None);
        assert_eq!(sources.get(a).map(|s| s.text.as_str()), Some("a: 1\n"));
        assert!(sources.get(b + 1).is_none());
        assert_eq!(
            sources.paths(),
            vec!["a.yml".to_string(), "infra/b.yml".to_string()]
        );
    }
}
