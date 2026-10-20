//! Builds the edits findings suggest, and applies them the way the editor's `replaceRange` will.
//! An edit's span counts UTF-16 offsets as [`SourceText::span`] does, a `\r\n` break as one.

use std::ops::Range;

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

    #[test]
    fn an_edit_on_a_crlf_file_applies_at_its_bytes() {
        let text = "a: 1\r\nb: 2\r\nc: x\r\n";
        let source = SourceText::new("c.yaml".into(), text.into());
        let edit = replace(&source, 15..16, "y".into(), "t".into());
        assert_eq!(edit.span, Span { from: 13, to: 14 });
        assert_eq!(apply(text, &edit), "a: 1\r\nb: 2\r\nc: y\r\n");
    }
}
