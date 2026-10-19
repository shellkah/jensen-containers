//! The project model the backend answers with. Lines and columns are zero-based, and columns and
//! offsets count UTF-16 code units, the way the editor counts them.

use serde::Serialize;

/// A position in a file: its path relative to the checkout root, a line and a column.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// A range of a file's text, as UTF-16 offsets from the start of the file.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub from: u32,
    pub to: u32,
}

/// A published port of a service, from short or long syntax.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortBinding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub fixed: bool,
    pub source: SourceRef,
    pub span: Span,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    Named,
    Bind,
    Tmpfs,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    pub kind: MountKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub target: String,
    pub read_only: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub context: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceNode {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<Build>,
    pub ports: Vec<PortBinding>,
    pub volumes: Vec<VolumeMount>,
    pub networks: Vec<String>,
    pub depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    pub profiles: Vec<String>,
    pub source: SourceRef,
}

/// A top-level volume or network.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceNode {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_name: Option<String>,
    pub external: bool,
    pub source: SourceRef,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    DependsOn,
    Network,
    Volume,
    Links,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NameOrigin {
    Name,
    Folder,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectName {
    pub value: String,
    pub origin: NameOrigin,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModel {
    pub project_name: ProjectName,
    pub files: Vec<String>,
    pub services: Vec<ServiceNode>,
    pub volumes: Vec<ResourceNode>,
    pub networks: Vec<ResourceNode>,
    pub edges: Vec<Edge>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleId {
    FixedHostPort,
    ContainerName,
    SharedFixedName,
    SameProjectName,
    UnprovidedVariable,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Affects {
    Agents,
    ByHand,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub file: String,
    pub span: Span,
    pub new_text: String,
    pub title: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule: RuleId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub message: String,
    pub affects: Vec<Affects>,
    pub lead: bool,
    pub source: SourceRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<TextEdit>,
}

/// A variable with no inline default, and where it is written.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Unresolved {
    pub variable: String,
    pub source: SourceRef,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadError {
    pub file: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// What `analyze` answers with.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeResult {
    pub model: ProjectModel,
    pub findings: Vec<Finding>,
    pub unresolved: Vec<Unresolved>,
    pub errors: Vec<LoadError>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn optional_fields_are_left_out_and_names_are_camel_case() {
        let binding = PortBinding {
            host_ip: None,
            host: Some("8080".into()),
            container: "80".into(),
            protocol: None,
            fixed: true,
            source: SourceRef {
                file: "compose.yaml".into(),
                line: 3,
                column: 8,
            },
            span: Span { from: 40, to: 49 },
        };
        assert_eq!(
            serde_json::to_value(&binding).unwrap(),
            json!({
                "host": "8080", "container": "80", "fixed": true,
                "source": { "file": "compose.yaml", "line": 3, "column": 8 }, "span": { "from": 40, "to": 49 }
            })
        );
        assert_eq!(
            serde_json::to_value(EdgeKind::DependsOn).unwrap(),
            json!("depends_on")
        );
        assert_eq!(
            serde_json::to_value(Affects::ByHand).unwrap(),
            json!("by_hand")
        );
        assert_eq!(
            serde_json::to_value(RuleId::UnprovidedVariable).unwrap(),
            json!("unprovided_variable")
        );
    }

    #[test]
    fn required_lists_are_always_written() {
        let service = ServiceNode {
            name: "db".into(),
            image: None,
            build: None,
            ports: vec![],
            volumes: vec![],
            networks: vec![],
            depends_on: vec![],
            container_name: None,
            profiles: vec![],
            source: SourceRef {
                file: "c.yaml".into(),
                line: 0,
                column: 2,
            },
        };
        assert_eq!(
            serde_json::to_value(&service).unwrap(),
            json!({
                "name": "db", "ports": [], "volumes": [], "networks": [], "dependsOn": [], "profiles": [],
                "source": { "file": "c.yaml", "line": 0, "column": 2 }
            })
        );
        let result = AnalyzeResult {
            model: ProjectModel {
                project_name: ProjectName {
                    value: "app".into(),
                    origin: NameOrigin::Folder,
                },
                files: vec![],
                services: vec![],
                volumes: vec![],
                networks: vec![],
                edges: vec![],
            },
            findings: vec![],
            unresolved: vec![],
            errors: vec![],
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "model": { "projectName": { "value": "app", "origin": "folder" }, "files": [],
                           "services": [], "volumes": [], "networks": [], "edges": [] },
                "findings": [], "unresolved": [], "errors": []
            })
        );
    }

    #[test]
    fn finding_and_mount_shapes_match_the_mirror() {
        let finding = Finding {
            rule: RuleId::FixedHostPort,
            service: Some("db".into()),
            message: "m".into(),
            affects: vec![Affects::Agents, Affects::ByHand],
            lead: false,
            source: SourceRef {
                file: "c.yaml".into(),
                line: 1,
                column: 1,
            },
            span: None,
            edit: Some(TextEdit {
                file: "c.yaml".into(),
                span: Span { from: 1, to: 2 },
                new_text: "x".into(),
                title: "t".into(),
            }),
        };
        let value = serde_json::to_value(&finding).unwrap();
        assert_eq!(value["rule"], json!("fixed_host_port"));
        assert_eq!(value["affects"], json!(["agents", "by_hand"]));
        assert_eq!(value["edit"]["newText"], json!("x"));
        assert!(value.get("span").is_none());
        let mount = VolumeMount {
            kind: MountKind::Tmpfs,
            source: None,
            target: "/t".into(),
            read_only: true,
        };
        assert_eq!(
            serde_json::to_value(&mount).unwrap(),
            json!({ "kind": "tmpfs", "target": "/t", "readOnly": true })
        );
        let resource = ResourceNode {
            name: "v".into(),
            fixed_name: Some("f".into()),
            external: false,
            source: finding.source.clone(),
        };
        assert_eq!(
            serde_json::to_value(&resource).unwrap()["fixedName"],
            json!("f")
        );
    }
}
