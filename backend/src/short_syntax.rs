//! Splitters for Compose's short `ports:` and `volumes:` strings. Segments come back as written,
//! variables unexpanded. A `${...}` and a `[...]` are atomic: whatever separator they contain
//! doesn't split them.

use crate::model::MountKind;

/// The parts of a short port string, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortParts {
    pub ip: Option<String>,
    pub host: Option<String>,
    pub container: String,
    pub protocol: Option<String>,
}

/// The parts of a short volume string, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeParts {
    pub source: Option<String>,
    pub target: String,
    pub mode: Option<String>,
}

/// Byte offsets of every `separator` outside a `${...}` and a `[...]`.
fn top_level(text: &str, separator: char) -> Vec<usize> {
    let mut found = Vec::new();
    let mut braces = 0usize;
    let mut brackets = 0usize;
    let mut previous = None;
    for (offset, c) in text.char_indices() {
        match c {
            '{' if braces > 0 || previous == Some('$') => braces += 1,
            '}' if braces > 0 => braces -= 1,
            '[' if braces == 0 => brackets += 1,
            ']' if braces == 0 && brackets > 0 => brackets -= 1,
            c if c == separator && braces == 0 && brackets == 0 => found.push(offset),
            _ => {}
        }
        previous = Some(c);
    }
    found
}

/// Cuts `text` at the given separator offsets (all single-byte characters).
fn cut<'a>(text: &'a str, at: &[usize]) -> Option<Vec<&'a str>> {
    let mut parts = Vec::with_capacity(at.len() + 1);
    let mut start = 0;
    for &offset in at {
        parts.push(text.get(start..offset)?);
        start = offset + 1;
    }
    parts.push(text.get(start..)?);
    Some(parts)
}

fn some(segment: &str) -> Option<String> {
    (!segment.is_empty()).then(|| segment.to_string())
}

/// Splits `[ip:][host:]container[/protocol]`. The protocol follows the last top-level `/`. An empty
/// host is `None`; no container, or more than three segments, is `None` overall.
pub fn split_port(text: &str) -> Option<PortParts> {
    let (body, protocol) = match top_level(text, '/').last() {
        Some(&slash) => {
            let protocol = text.get(slash + 1..)?;
            if protocol.is_empty() {
                return None;
            }
            (text.get(..slash)?, Some(protocol.to_string()))
        }
        None => (text, None),
    };
    let segments = cut(body, &top_level(body, ':'))?;
    let (ip, host, container) = match segments.as_slice() {
        [container] => (None, None, *container),
        [host, container] => (None, some(host), *container),
        [ip, host, container] => (Some(some(ip)?), some(host), *container),
        _ => return None,
    };
    if container.is_empty() {
        return None;
    }
    Some(PortParts {
        ip,
        host,
        container: container.to_string(),
        protocol,
    })
}

/// Splits `[source:]target[:mode]`. A single segment is an anonymous volume's target. An empty
/// source or target, or more than three segments, is `None`.
pub fn split_volume(text: &str) -> Option<VolumeParts> {
    let segments = cut(text, &top_level(text, ':'))?;
    let (source, target, mode) = match segments.as_slice() {
        [target] => (None, *target, None),
        [source, target] => (Some(some(source)?), *target, None),
        [source, target, mode] => (Some(some(source)?), *target, some(mode)),
        _ => return None,
    };
    if target.is_empty() {
        return None;
    }
    Some(VolumeParts {
        source,
        target: target.to_string(),
        mode,
    })
}

/// A source starting with `.`, `/` or `~` is a bind mount; anything else, or none, is a named volume.
pub fn mount_kind(source: Option<&str>) -> MountKind {
    match source.and_then(|s| s.chars().next()) {
        Some('.' | '/' | '~') => MountKind::Bind,
        _ => MountKind::Named,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MountKind;

    fn parts(text: &str) -> Option<PortParts> {
        split_port(text)
    }

    fn row(
        ip: Option<&str>,
        host: Option<&str>,
        container: &str,
        protocol: Option<&str>,
    ) -> Option<PortParts> {
        Some(PortParts {
            ip: ip.map(Into::into),
            host: host.map(Into::into),
            container: container.into(),
            protocol: protocol.map(Into::into),
        })
    }

    #[test]
    fn port_forms() {
        assert_eq!(
            parts("127.0.0.1:5432:5432"),
            row(Some("127.0.0.1"), Some("5432"), "5432", None)
        );
        assert_eq!(parts("8080:80"), row(None, Some("8080"), "80", None));
        assert_eq!(parts("5432"), row(None, None, "5432", None));
        assert_eq!(parts("53:53/udp"), row(None, Some("53"), "53", Some("udp")));
        assert_eq!(
            parts("[::1]:8080:80"),
            row(Some("[::1]"), Some("8080"), "80", None)
        );
        assert_eq!(
            parts("${HOST_IP:-127.0.0.1}:${DB_PORT:-5432}:5432"),
            row(
                Some("${HOST_IP:-127.0.0.1}"),
                Some("${DB_PORT:-5432}"),
                "5432",
                None
            )
        );
        assert_eq!(
            parts("127.0.0.1::80"),
            row(Some("127.0.0.1"), None, "80", None)
        );
        assert_eq!(
            parts("9090-9091:8080-8081"),
            row(None, Some("9090-9091"), "8080-8081", None)
        );
        assert_eq!(parts(""), None);
    }

    #[test]
    fn port_edge_cases_never_panic() {
        assert_eq!(
            parts("${A:-x/y}:80"),
            row(None, Some("${A:-x/y}"), "80", None)
        );
        assert_eq!(
            parts("8080:${P:-80/tcp}"),
            row(None, Some("8080"), "${P:-80/tcp}", None)
        );
        assert_eq!(parts("${A:-1:2"), row(None, None, "${A:-1:2", None));
        assert_eq!(parts("é:ü/ß"), row(None, Some("é"), "ü", Some("ß")));
        assert_eq!(parts("8080:"), None);
        assert_eq!(parts("/udp"), None);
        assert_eq!(parts("a:b:c:d"), None);
        for text in ["[", "]", "[::1", ":", "::", "/", "${", "}", "$", "😀:😀/😀"] {
            let _ = split_port(text);
            let _ = split_volume(text);
        }
    }

    fn vol(text: &str) -> Option<(Option<String>, String, Option<String>)> {
        split_volume(text).map(|v| (v.source, v.target, v.mode))
    }

    #[test]
    fn volume_forms() {
        let named = vol("app_pgdata:/var/lib/postgresql/data").unwrap();
        assert_eq!(
            named,
            (
                Some("app_pgdata".into()),
                "/var/lib/postgresql/data".into(),
                None
            )
        );
        assert_eq!(mount_kind(named.0.as_deref()), MountKind::Named);

        let bind = vol("./initdb:/docker-entrypoint-initdb.d:ro").unwrap();
        assert_eq!(
            bind,
            (
                Some("./initdb".into()),
                "/docker-entrypoint-initdb.d".into(),
                Some("ro".into())
            )
        );
        assert_eq!(mount_kind(bind.0.as_deref()), MountKind::Bind);

        let up = vol("../:/infra:ro").unwrap();
        assert_eq!(mount_kind(up.0.as_deref()), MountKind::Bind);
        assert_eq!(up.2.as_deref(), Some("ro"));
        let home = vol("~/cache:/cache").unwrap();
        assert_eq!(mount_kind(home.0.as_deref()), MountKind::Bind);

        assert_eq!(vol("/data"), Some((None, "/data".into(), None)));
        assert_eq!(vol(""), None);
        assert_eq!(
            vol("${DATA:-./d}:/data:rw"),
            Some((
                Some("${DATA:-./d}".into()),
                "/data".into(),
                Some("rw".into())
            ))
        );
        assert_eq!(vol("a:b:c:d"), None);
        assert_eq!(vol(":/x"), None);
        assert_eq!(vol("a:"), None);
    }

    #[test]
    fn mount_kind_cases() {
        assert_eq!(mount_kind(None), MountKind::Named);
        assert_eq!(mount_kind(Some("data")), MountKind::Named);
        assert_eq!(mount_kind(Some("")), MountKind::Named);
        assert_eq!(mount_kind(Some(".")), MountKind::Bind);
        assert_eq!(mount_kind(Some("./x")), MountKind::Bind);
        assert_eq!(mount_kind(Some("/abs")), MountKind::Bind);
        assert_eq!(mount_kind(Some("~")), MountKind::Bind);
        assert_eq!(mount_kind(Some("${V}")), MountKind::Named);
    }
}
