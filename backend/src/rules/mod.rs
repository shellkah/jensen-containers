//! The readiness rules: what in a Compose project keeps two copies of it from running side by side.

pub mod names;
pub mod ports;

use crate::compose::Project;
use crate::model::Finding;
use crate::source::SourceText;

/// Every readiness finding of `project`, by rule in `RuleId` order, then by the file's place in
/// `model.files`, then by line and column.
pub fn readiness(project: &Project) -> Vec<Finding> {
    let mut findings = ports::fixed_host_port(project);
    findings.extend(names::container_name(project));
    findings.extend(names::shared_fixed_name(project));
    let files = &project.model.files;
    // `sort_by_key` is stable: findings at one position keep the order their rule gave them.
    findings.sort_by_key(|finding| {
        let file = files
            .iter()
            .position(|file| *file == finding.source.file)
            .unwrap_or(usize::MAX);
        (
            finding.rule as u8,
            file,
            finding.source.line,
            finding.source.column,
        )
    });
    findings
}

/// The service that writes what `readers`, the services reaching it through an anchor or `extends`,
/// all share at byte `at` of `source`: the reader whose key is in that file on the last line at or
/// before `at`'s; when none is (an anchor under an `x-` key, or a base outside the project), the
/// first reader. None only without readers.
pub(crate) fn writer<'r>(
    project: &Project,
    source: &SourceText,
    at: usize,
    readers: &[&'r str],
) -> Option<&'r str> {
    let line = source.line_of(at);
    let mut best: Option<(&str, u32)> = None;
    for &reader in readers {
        let Some(service) = project.model.services.iter().find(|s| s.name == reader) else {
            continue;
        };
        let key = service.source.line;
        let before =
            service.source.file == source.path && usize::try_from(key).is_ok_and(|key| key <= line);
        if before && best.is_none_or(|(_, best)| key > best) {
            best = Some((reader, key));
        }
    }
    best.map(|(reader, _)| reader)
        .or_else(|| readers.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{load_project, one};
    use crate::files::MemFiles;
    use crate::model::RuleId;
    use crate::paths::Checkout;

    #[test]
    fn the_writer_is_the_last_reader_whose_key_comes_before() {
        let project = one(
            "services:\n  web:\n    extends:\n      service: base\n  base:\n    image: nginx\n    container_name: x\n  late:\n    extends:\n      service: base\n",
        );
        let source = project.sources.get(0).unwrap();
        let at = source.text.find("container_name").unwrap();
        // base's key is the last at or before the entry's line; web's is earlier, late's after.
        assert_eq!(
            writer(&project, source, at, &["web", "base", "late"]),
            Some("base")
        );
        assert_eq!(writer(&project, source, at, &["late", "web"]), Some("web"));
        // With none at or before, the first; with none at all, none.
        assert_eq!(writer(&project, source, 0, &["late", "web"]), Some("late"));
        assert_eq!(
            writer(&project, source, at, &["nobody", "late"]),
            Some("nobody")
        );
        assert_eq!(writer(&project, source, at, &[]), None);
        // A key in another file never counts.
        let other = crate::source::SourceText::new("other.yaml".into(), source.text.clone());
        assert_eq!(writer(&project, &other, at, &["web", "base"]), Some("web"));
    }

    #[test]
    fn every_rule_runs_and_rules_come_in_their_order() {
        let project = one(
            "volumes:\n  cache:\n    name: app-cache\nservices:\n  db:\n    container_name: app-db\n    image: pg\n    ports:\n      - \"5432:5432\"\n",
        );
        let rules: Vec<_> = readiness(&project)
            .iter()
            .map(|finding| finding.rule)
            .collect();
        assert_eq!(
            rules,
            [
                RuleId::FixedHostPort,
                RuleId::ContainerName,
                RuleId::SharedFixedName
            ]
        );
    }

    #[test]
    fn findings_follow_the_files_reading_order_before_the_model_order() {
        // db comes first in the model, but its port is written in the second file.
        let files = MemFiles::from([
            (
                "a.yaml",
                "services:\n  db:\n    image: postgres\n  web:\n    ports:\n      - \"9000:90\"\n",
            ),
            (
                "b.yaml",
                "services:\n  db:\n    ports:\n      - \"5432:5432\"\n",
            ),
        ]);
        let paths = ["a.yaml".to_string(), "b.yaml".to_string()];
        let project = load_project(&files, &Checkout::primary(), &paths);
        assert!(project.errors.is_empty(), "{:?}", project.errors);
        let at: Vec<_> = readiness(&project)
            .iter()
            .map(|finding| (finding.source.file.clone(), finding.source.line))
            .collect();
        assert_eq!(at, [("a.yaml".to_string(), 5), ("b.yaml".to_string(), 3)]);
    }

    #[test]
    fn findings_come_in_position_order() {
        let project = one(
            "services:\n  web:\n    ports:\n      - \"9000:90\"\n  db:\n    ports:\n      - \"5432:5432\"\n",
        );
        let lines: Vec<_> = readiness(&project)
            .iter()
            .map(|finding| (finding.service.clone(), finding.source.line))
            .collect();
        assert_eq!(
            lines,
            [(Some("web".to_string()), 3), (Some("db".to_string()), 6)]
        );
    }
}
