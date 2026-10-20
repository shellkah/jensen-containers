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

    /// The shared contract sample: `test/fixtures/analyze-sample.json` and the TypeScript literal in
    /// `test/backend.test.ts` say the same thing, so Rust, TypeScript and the file must all agree.
    fn sample() -> AnalyzeResult {
        let source = |line, column| SourceRef {
            file: "infra/local/docker-compose.yml".into(),
            line,
            column,
        };
        let span = Span { from: 412, to: 429 };
        AnalyzeResult {
            model: ProjectModel {
                project_name: ProjectName {
                    value: "local".into(),
                    origin: NameOrigin::Folder,
                },
                files: vec!["infra/local/docker-compose.yml".into()],
                services: vec![
                    ServiceNode {
                        name: "db".into(),
                        image: Some("postgres:16".into()),
                        build: Some(Build {
                            context: "infra/local".into(),
                            dockerfile: Some("Dockerfile.db".into()),
                        }),
                        ports: vec![PortBinding {
                            host_ip: Some("127.0.0.1".into()),
                            host: Some("5432".into()),
                            container: "5432".into(),
                            protocol: Some("tcp".into()),
                            fixed: true,
                            source: source(20, 8),
                            span: span.clone(),
                        }],
                        volumes: vec![
                            VolumeMount {
                                kind: MountKind::Named,
                                source: Some("data".into()),
                                target: "/var/lib/postgresql/data".into(),
                                read_only: false,
                            },
                            VolumeMount {
                                kind: MountKind::Bind,
                                source: Some("./init".into()),
                                target: "/docker-entrypoint-initdb.d".into(),
                                read_only: true,
                            },
                        ],
                        networks: vec!["backnet".into()],
                        depends_on: vec![],
                        container_name: Some("app-db".into()),
                        profiles: vec!["dev".into()],
                        source: source(12, 2),
                    },
                    ServiceNode {
                        name: "app".into(),
                        image: None,
                        build: None,
                        ports: vec![],
                        volumes: vec![],
                        networks: vec![],
                        depends_on: vec!["db".into()],
                        container_name: None,
                        profiles: vec![],
                        source: source(30, 2),
                    },
                ],
                volumes: vec![ResourceNode {
                    name: "data".into(),
                    fixed_name: Some("app-data".into()),
                    external: false,
                    source: source(40, 2),
                }],
                networks: vec![ResourceNode {
                    name: "backnet".into(),
                    fixed_name: None,
                    external: true,
                    source: source(44, 2),
                }],
                edges: vec![
                    Edge {
                        from: "app".into(),
                        to: "db".into(),
                        kind: EdgeKind::DependsOn,
                    },
                    Edge {
                        from: "db".into(),
                        to: "backnet".into(),
                        kind: EdgeKind::Network,
                    },
                    Edge {
                        from: "db".into(),
                        to: "data".into(),
                        kind: EdgeKind::Volume,
                    },
                    Edge {
                        from: "app".into(),
                        to: "db".into(),
                        kind: EdgeKind::Links,
                    },
                ],
            },
            findings: vec![Finding {
                rule: RuleId::FixedHostPort,
                service: Some("db".into()),
                message:
                    "db publishes host port 5432, so a second copy of this project can't start."
                        .into(),
                affects: vec![Affects::Agents, Affects::ByHand],
                lead: false,
                source: source(20, 8),
                span: Some(span.clone()),
                edit: Some(TextEdit {
                    file: "infra/local/docker-compose.yml".into(),
                    span,
                    new_text: "\"127.0.0.1:${DB_PORT:-5432}:5432\"".into(),
                    title: "Make db's host port configurable".into(),
                }),
            }],
            unresolved: vec![Unresolved {
                variable: "PG_MAJOR".into(),
                source: source(14, 20),
            }],
            errors: vec![LoadError {
                file: "infra/local/docker-compose.yml".into(),
                message: "can't include extra.yml: no such file".into(),
                source: Some(source(3, 4)),
            }],
        }
    }

    #[test]
    fn serializes_to_the_shared_sample() {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../../test/fixtures/analyze-sample.json")).unwrap();
        assert_eq!(serde_json::to_value(sample()).unwrap(), file);
        // The sample says what the code does: an error's source is in the file it is reported in.
        for error in sample().errors {
            if let Some(source) = &error.source {
                assert_eq!(source.file, error.file, "{error:?}");
            }
        }
    }
}
