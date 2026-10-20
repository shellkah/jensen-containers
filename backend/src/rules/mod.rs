//! The readiness rules: what in a Compose project keeps two copies of it from running side by side.

pub mod ports;

use crate::compose::Project;
use crate::model::Finding;

/// Every readiness finding of `project`, by rule in `RuleId` order, then by the file's place in
/// `model.files`, then by line and column.
pub fn readiness(project: &Project) -> Vec<Finding> {
    let mut findings = ports::fixed_host_port(project);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{load_project, one};
    use crate::files::MemFiles;
    use crate::paths::Checkout;

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
