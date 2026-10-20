//! `fixed_host_port`: a host port written as a fixed number, which only one copy of the stack can
//! bind at a time.

use std::collections::HashSet;

use crate::compose::{PortFact, PortForm, Project};
use crate::edits::replace;
use crate::model::{Affects, Finding, RuleId};
use crate::source::SourceText;

/// One finding per fixed host port as written: a port an anchor or `extends` gives several
/// services is one finding, named after the service that writes it. Each finding's edit moves the
/// port to a variable defaulting to it, a different variable for every port.
pub fn fixed_host_port(project: &Project) -> Vec<Finding> {
    let ports = &project.facts.ports;
    let mut written = HashSet::new();
    let mut writers = HashSet::new();
    let mut taken = HashSet::new();
    let mut findings = Vec::new();
    for fact in ports.iter().filter(|fact| fact.fixed) {
        if !written.insert((fact.file, fact.bytes.start, fact.bytes.end)) {
            continue;
        }
        let (Some(source), Some(host)) = (project.sources.get(fact.file), fact.host.as_deref())
        else {
            continue;
        };
        let writer = writer(project, source, fact);
        let first = writers.insert(writer);
        let variable = unused(
            port_variable(writer, &container(project, source, fact), first),
            &mut taken,
        );
        let value = format!("${{{variable}:-{host}}}");
        let new_text = match &fact.form {
            PortForm::Short(parts) => {
                let ip = parts.ip.as_ref().map(|ip| format!("{ip}:"));
                let protocol = parts
                    .protocol
                    .as_ref()
                    .map(|protocol| format!("/{protocol}"));
                quoted(&format!(
                    "{}{value}:{}{}",
                    ip.unwrap_or_default(),
                    parts.container,
                    protocol.unwrap_or_default()
                ))
            }
            PortForm::Long => quoted(&value),
        };
        let title = format!("Make host port {host} a variable ({variable})");
        findings.push(Finding {
            rule: RuleId::FixedHostPort,
            service: Some(writer.to_owned()),
            message: message(writer, host, &variable),
            affects: vec![Affects::Agents, Affects::ByHand],
            lead: false,
            source: source.source_ref(fact.bytes.start),
            span: Some(source.span(fact.bytes.clone())),
            edit: Some(replace(source, fact.bytes.clone(), new_text, title)),
        });
    }
    findings
}

/// The variable a service's host port moves to: `<SERVICE>_PORT` for its first, then with the
/// container port appended. Every character outside `A-Z0-9` becomes `_`, and a name starting
/// with a digit gets a `_` in front.
pub fn port_variable(service: &str, container: &str, first: bool) -> String {
    let name = if first {
        format!("{service}_PORT")
    } else {
        format!("{service}_PORT_{container}")
    };
    let name: String = name
        .to_ascii_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{name}")
    } else {
        name
    }
}

/// The service that writes `fact`: among the services reaching the same written port, the one
/// whose key is in the port's file on the last line at or before the port's; when none is (an
/// anchor under an `x-` key, or a base outside the project), the first in model order.
fn writer<'p>(project: &'p Project, source: &SourceText, fact: &'p PortFact) -> &'p str {
    let line = source.line_of(fact.bytes.start);
    let mut readers = project
        .facts
        .ports
        .iter()
        .filter(|other| other.file == fact.file && other.bytes == fact.bytes)
        .map(|other| other.service.as_str());
    let mut best: Option<(&str, u32)> = None;
    for reader in readers.clone() {
        let Some(service) = project.model.services.iter().find(|s| s.name == reader) else {
            continue;
        };
        let at = service.source.line;
        let before =
            service.source.file == source.path && usize::try_from(at).is_ok_and(|at| at <= line);
        if before && best.is_none_or(|(_, best)| at > best) {
            best = Some((reader, at));
        }
    }
    best.map(|(reader, _)| reader)
        .or_else(|| readers.next())
        .unwrap_or(&fact.service)
}

/// The container port of `fact`, interpolated, from the binding the model holds for it.
fn container(project: &Project, source: &SourceText, fact: &PortFact) -> String {
    let span = source.span(fact.bytes.clone());
    project
        .model
        .services
        .iter()
        .filter(|service| service.name == fact.service)
        .flat_map(|service| &service.ports)
        .find(|port| port.source.file == source.path && port.span == span)
        .map(|port| port.container.clone())
        .unwrap_or_default()
}

/// `name`, or with `_2`, `_3`, ... appended when an earlier finding took it; taken from now on.
fn unused(name: String, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.clone();
    let mut number = 2;
    while !taken.insert(candidate.clone()) {
        candidate = format!("{name}_{number}");
        number += 1;
    }
    candidate
}

/// `text` as a double-quoted YAML scalar.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn message(service: &str, host: &str, variable: &str) -> String {
    format!(
        "{service} publishes host port {host} on a fixed number, so only one copy of the stack can run at a time: every agent's copy binds it, and on Linux so does the copy you start yourself. The edit keeps {host} by default and lets a copy you start by hand use another port by setting {variable}; agents' copies get the same once Run sets it per session."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::one;
    use crate::edits::apply;
    use crate::model::{Affects, RuleId, SourceRef, Span};

    /// The text of a span, for ASCII texts where UTF-16 offsets are byte offsets.
    fn spanned<'t>(text: &'t str, span: &Span) -> &'t str {
        &text[span.from as usize..span.to as usize]
    }

    fn new_texts(text: &str) -> Vec<String> {
        fixed_host_port(&one(text))
            .into_iter()
            .map(|finding| finding.edit.unwrap().new_text)
            .collect()
    }

    #[test]
    fn a_fixed_short_port_gets_a_variable_and_keeps_the_host_ip() {
        let project = one("services:\n  db:\n    ports:\n      - \"127.0.0.1:5432:5432\"\n");
        let finding = &fixed_host_port(&project)[0];
        assert_eq!(
            finding.edit.as_ref().unwrap().new_text,
            "\"127.0.0.1:${DB_PORT:-5432}:5432\""
        );
        assert_eq!(finding.affects, vec![Affects::Agents, Affects::ByHand]);
        assert_eq!(
            (finding.service.as_deref(), finding.lead),
            (Some("db"), false)
        );
    }

    #[test]
    fn the_finding_names_the_port_and_where_it_is_written() {
        let text = "services:\n  db:\n    ports:\n      - \"127.0.0.1:5432:5432\"\n";
        let findings = fixed_host_port(&one(text));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule, RuleId::FixedHostPort);
        assert_eq!(
            finding.message,
            "db publishes host port 5432 on a fixed number, so only one copy of the stack can run at a time: every agent's copy binds it, and on Linux so does the copy you start yourself. The edit keeps 5432 by default and lets a copy you start by hand use another port by setting DB_PORT; agents' copies get the same once Run sets it per session."
        );
        assert_eq!(
            finding.source,
            SourceRef {
                file: "compose.yaml".into(),
                line: 3,
                column: 8
            }
        );
        let span = finding.span.clone().unwrap();
        assert_eq!(spanned(text, &span), "\"127.0.0.1:5432:5432\"");
        let edit = finding.edit.as_ref().unwrap();
        assert_eq!(edit.file, "compose.yaml");
        assert_eq!(edit.span, span);
        assert_eq!(edit.title, "Make host port 5432 a variable (DB_PORT)");
    }

    #[test]
    fn applying_the_edit_clears_the_finding() {
        let text = "services:\n  db:\n    ports:\n      - \"127.0.0.1:5432:5432\"\n";
        let edit = fixed_host_port(&one(text))[0].edit.clone().unwrap();
        let fixed = one(&apply(text, &edit));
        assert!(fixed_host_port(&fixed).is_empty());
        assert_eq!(
            fixed.model.services[0].ports[0].host.as_deref(),
            Some("5432")
        );
    }

    #[test]
    fn long_syntax_replaces_only_published() {
        let text = "services:\n  web:\n    ports:\n      - target: 80\n        published: 8080\n";
        let finding = &fixed_host_port(&one(text))[0];
        let edit = finding.edit.as_ref().unwrap();
        assert_eq!(edit.new_text, "\"${WEB_PORT:-8080}\"");
        assert_eq!(spanned(text, &edit.span), "8080");
        assert_eq!(edit.title, "Make host port 8080 a variable (WEB_PORT)");
        let fixed = one(&apply(text, edit));
        assert!(fixed.errors.is_empty(), "{:?}", fixed.errors);
        assert!(fixed_host_port(&fixed).is_empty());
        let port = &fixed.model.services[0].ports[0];
        assert_eq!(
            (port.host.as_deref(), port.container.as_str()),
            (Some("8080"), "80")
        );
    }

    #[test]
    fn a_second_fixed_port_gets_its_container_port() {
        let texts = new_texts(
            "services:\n  api:\n    ports:\n      - \"8080:8080\"\n      - \"9229:9229\"\n",
        );
        assert_eq!(
            texts,
            [
                "\"${API_PORT:-8080}:8080\"",
                "\"${API_PORT_9229:-9229}:9229\""
            ]
        );
    }

    #[test]
    fn a_variable_already_taken_gets_a_number() {
        let texts = new_texts(
            "services:\n  api:\n    ports:\n      - \"8080:80\"\n      - \"8081:80\"\n      - \"8082:80\"\n  a-b:\n    ports: [\"1:1\"]\n  a_b:\n    ports: [\"2:2\"]\n",
        );
        assert_eq!(
            texts,
            [
                "\"${API_PORT:-8080}:80\"",
                "\"${API_PORT_80:-8081}:80\"",
                "\"${API_PORT_80_2:-8082}:80\"",
                "\"${A_B_PORT:-1}:1\"",
                "\"${A_B_PORT_2:-2}:2\"",
            ]
        );
    }

    #[test]
    fn service_names_become_variable_names() {
        assert_eq!(port_variable("web-api.v2", "80", true), "WEB_API_V2_PORT");
        assert_eq!(port_variable("1api", "80", true), "_1API_PORT");
        assert_eq!(port_variable("api", "9229", false), "API_PORT_9229");
        assert_eq!(
            port_variable("api", "8080-8081", false),
            "API_PORT_8080_8081"
        );
    }

    #[test]
    fn the_protocol_suffix_is_kept() {
        assert_eq!(
            new_texts("services:\n  dns:\n    ports:\n      - \"53:53/udp\"\n"),
            ["\"${DNS_PORT:-53}:53/udp\""]
        );
    }

    #[test]
    fn a_plain_scalar_becomes_a_quoted_one() {
        let text = "services:\n  web:\n    ports:\n      - 8080:80\n";
        let edit = fixed_host_port(&one(text))[0].edit.clone().unwrap();
        assert_eq!(spanned(text, &edit.span), "8080:80");
        assert_eq!(
            apply(text, &edit),
            "services:\n  web:\n    ports:\n      - \"${WEB_PORT:-8080}:80\"\n"
        );
    }

    #[test]
    fn ports_without_a_fixed_host_port_are_fine() {
        let project = one(
            "services:\n  db:\n    ports:\n      - 5432\n      - \"${DB_PORT:-5432}:5432\"\n      - \"127.0.0.1::5432\"\n",
        );
        assert_eq!(project.facts.ports.len(), 3);
        assert!(fixed_host_port(&project).is_empty());
    }

    #[test]
    fn a_port_written_once_for_two_services_is_one_finding() {
        // The anchor is under an `x-` key, so no service writes it: the first in model order names it.
        let project = one(
            "x-db: &db\n  ports:\n    - \"5432:5432\"\nservices:\n  db:\n    <<: *db\n  replica:\n    <<: *db\n",
        );
        assert_eq!(project.facts.ports.len(), 2);
        let findings = fixed_host_port(&project);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].service.as_deref(), Some("db"));
        assert_eq!(
            findings[0].edit.as_ref().unwrap().new_text,
            "\"${DB_PORT:-5432}:5432\""
        );
    }

    #[test]
    fn a_port_is_named_after_the_service_that_writes_it() {
        // web comes first in the model, but the port is written under base, which web extends.
        let project = one(
            "services:\n  web:\n    extends:\n      service: base\n  base:\n    image: nginx\n    ports:\n      - \"8080:80\"\n",
        );
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        assert_eq!(project.facts.ports.len(), 2);
        assert_eq!(project.facts.ports[0].service, "web");
        let findings = fixed_host_port(&project);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].service.as_deref(), Some("base"));
        assert_eq!(
            findings[0].edit.as_ref().unwrap().new_text,
            "\"${BASE_PORT:-8080}:80\""
        );
        assert!(
            findings[0]
                .message
                .starts_with("base publishes host port 8080 ")
        );
    }

    #[test]
    fn an_edit_on_a_crlf_file_applies_at_the_port() {
        let text =
            "services:\r\n  db:\r\n    ports:\r\n      - \"5432:5432\"\r\n      - 6379:6379\r\n";
        let project = one(text);
        let mut fixed = text.to_owned();
        // Apply the later edit first, so the earlier one's span still holds.
        for finding in fixed_host_port(&project).iter().rev() {
            fixed = apply(&fixed, finding.edit.as_ref().unwrap());
        }
        assert_eq!(
            fixed,
            "services:\r\n  db:\r\n    ports:\r\n      - \"${DB_PORT:-5432}:5432\"\r\n      - \"${DB_PORT_6379:-6379}:6379\"\r\n"
        );
        assert!(fixed_host_port(&one(&fixed)).is_empty());
    }
}
