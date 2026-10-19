//! Compose variable interpolation with nothing set. The backend sees no environment, so defaults
//! resolve and every other reference stays verbatim and is reported, never guessed.

/// How deep defaults may nest before the rest is kept literal, so hostile input cannot overflow
/// the stack.
const MAX_DEPTH: usize = 32;

/// The result of interpolating one scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpolated {
    pub value: String,
    pub missing: Vec<Missing>,
    pub uses_variables: bool,
}

/// A reference that stayed verbatim because the variable has no value here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub name: String,
    /// The exact source text of the reference, such as `${PG_MAJOR}` or `$HOME`.
    pub reference: String,
}

/// Interpolates `text` the way Compose does with an empty environment.
pub fn interpolate(text: &str) -> Interpolated {
    let mut missing = Vec::new();
    let mut uses_variables = false;
    let value = expand(text, 0, &mut missing, &mut uses_variables);
    Interpolated {
        value,
        missing,
        uses_variables,
    }
}

/// Length in bytes of the variable name at the start of `s`, or 0 when there is none.
fn name_len(s: &str) -> usize {
    let mut len = 0;
    for (i, b) in s.bytes().enumerate() {
        let ok = b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit());
        if !ok {
            break;
        }
        len = i + 1;
    }
    len
}

/// Byte offset of the `}` that balances an already opened `{`, searching `s`.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn expand(text: &str, depth: usize, missing: &mut Vec<Missing>, uses: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        // `$` is one byte, so `after` starts on a char boundary.
        let after = &rest[at + 1..];
        let consumed = reference(after, depth, &mut out, missing, uses);
        match consumed {
            Some(n) => rest = &after[n..],
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Handles what follows a `$`. Returns the bytes of `after` it consumed, or `None` when the `$`
/// is literal.
fn reference(
    after: &str,
    depth: usize,
    out: &mut String,
    missing: &mut Vec<Missing>,
    uses: &mut bool,
) -> Option<usize> {
    if after.starts_with('$') {
        out.push('$');
        return Some(1);
    }
    if let Some(inner) = after.strip_prefix('{') {
        let n = name_len(inner);
        if n == 0 {
            return None;
        }
        let name = &inner[..n];
        let tail = &inner[n..];
        if tail.starts_with('}') {
            let reference = format!("${{{name}}}");
            out.push_str(&reference);
            missing.push(Missing {
                name: name.to_string(),
                reference,
            });
            *uses = true;
            return Some(1 + n + 1);
        }
        let colon = usize::from(tail.starts_with(':'));
        let op = tail.as_bytes().get(colon).copied()?;
        if !matches!(op, b'-' | b'+' | b'?') {
            return None;
        }
        let op_len = colon + 1;
        let body_start = n + op_len;
        let body_text = &inner[body_start..];
        let close = closing_brace(body_text)?;
        let body = &body_text[..close];
        let consumed = 1 + body_start + close + 1;
        *uses = true;
        match op {
            b'-' => {
                if depth >= MAX_DEPTH {
                    out.push_str(body);
                } else {
                    out.push_str(&expand(body, depth + 1, missing, uses));
                }
            }
            b'+' => {}
            _ => {
                let reference = format!("${{{}}}", &inner[..body_start + close]);
                out.push_str(&reference);
                missing.push(Missing {
                    name: name.to_string(),
                    reference,
                });
            }
        }
        return Some(consumed);
    }
    let n = name_len(after);
    if n == 0 {
        return None;
    }
    let reference = format!("${}", &after[..n]);
    out.push_str(&reference);
    missing.push(Missing {
        name: after[..n].to_string(),
        reference,
    });
    *uses = true;
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_variables_stay_verbatim_and_are_reported() {
        let out = interpolate("app-pg:${TAG}");
        assert_eq!(out.value, "app-pg:${TAG}");
        assert_eq!(
            out.missing,
            vec![Missing {
                name: "TAG".into(),
                reference: "${TAG}".into()
            }]
        );
    }

    #[test]
    fn double_dollar_is_a_literal() {
        let out = interpolate("echo $$HOME");
        assert_eq!(
            (out.value.as_str(), out.missing.len(), out.uses_variables),
            ("echo $HOME", 0, false)
        );
    }

    #[test]
    fn defaults_resolve() {
        for text in ["${TAG:-16}", "${TAG-16}"] {
            let out = interpolate(text);
            assert_eq!(out.value, "16");
            assert!(out.missing.is_empty());
            assert!(out.uses_variables);
        }
    }

    #[test]
    fn nested_defaults_resolve() {
        let out = interpolate("${A:-${B:-x}}");
        assert_eq!(out.value, "x");
        assert!(out.missing.is_empty());
        assert!(out.uses_variables);
    }

    #[test]
    fn missing_inside_a_default_is_reported() {
        let out = interpolate("${A:-${B}}");
        assert_eq!(out.value, "${B}");
        assert_eq!(
            out.missing,
            vec![Missing {
                name: "B".into(),
                reference: "${B}".into()
            }]
        );
    }

    #[test]
    fn bare_names_are_variables() {
        let out = interpolate("echo $HOME");
        assert_eq!(out.value, "echo $HOME");
        assert_eq!(
            out.missing,
            vec![Missing {
                name: "HOME".into(),
                reference: "$HOME".into()
            }]
        );
        assert!(out.uses_variables);
    }

    #[test]
    fn required_variables_are_missing() {
        for (text, name) in [("${DB:?set DB}", "DB"), ("${DB?set DB}", "DB")] {
            let out = interpolate(text);
            assert_eq!(out.value, text);
            assert_eq!(
                out.missing,
                vec![Missing {
                    name: name.into(),
                    reference: text.into()
                }]
            );
            assert!(out.uses_variables);
        }
    }

    #[test]
    fn alternatives_are_empty() {
        for text in ["${A:+x}", "${A+x}", "${A:+${B}}"] {
            let out = interpolate(text);
            assert_eq!(out.value, "");
            assert!(out.missing.is_empty());
            assert!(out.uses_variables);
        }
    }

    #[test]
    fn unclosed_braces_stay_literal() {
        let out = interpolate("a ${B:-x");
        assert_eq!(out.value, "a ${B:-x");
        assert!(out.missing.is_empty());
        assert!(!out.uses_variables);
        let out = interpolate("${A:-${B}");
        assert_eq!(out.value, "${A:-${B}");
        assert_eq!(out.missing.len(), 1);
    }

    #[test]
    fn other_dollars_stay_literal_and_unicode_is_safe() {
        for text in [
            "$",
            "a$",
            "$1",
            "${}",
            "${1A}",
            "${A:x}",
            "$ é",
            "${é}",
            "é$é",
            "price $€",
        ] {
            let out = interpolate(text);
            assert_eq!(out.value, text);
            assert!(out.missing.is_empty());
            assert!(!out.uses_variables);
        }
        assert_eq!(interpolate("${A:-é}").value, "é");
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let text = format!("{}x{}", "${A:-".repeat(5000), "}".repeat(5000));
        let _ = interpolate(&text);
    }
}
