//! What the two agent tools share: how they read their input and which Compose files they load
//! when the input names none.

use serde_json::Value;

use crate::compose::{Project, load_project};
use crate::files::ReadFile;
use crate::paths::{Checkout, file_path};

const WRONG_TYPE: &str = "checkout must be text and files a list of paths";
const BAD_FILE: &str =
    "files must be paths inside the checkout, such as infra/local/docker-compose.yml";

/// The names Compose looks for at the root of a project, in its order.
pub const USUAL_NAMES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// What an agent tool is asked: where to read, and optionally which Compose files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInput {
    pub checkout: Checkout,
    /// Normalized paths relative to the checkout root; none when the agent named none.
    pub files: Option<Vec<String>>,
}

impl ToolInput {
    /// `{ checkout?: string, files?: string[] }`. A missing or null key counts as absent, and so
    /// does an empty `files`. Nothing is read here: a path that leaves the project is refused.
    pub fn parse(input: &Value) -> Result<ToolInput, String> {
        let (checkout, files) = match input {
            Value::Null => (None, None),
            Value::Object(map) => (map.get("checkout"), map.get("files")),
            _ => return Err(WRONG_TYPE.to_owned()),
        };
        let checkout = match checkout {
            None | Some(Value::Null) => None,
            Some(Value::String(path)) => Some(path.as_str()),
            Some(_) => return Err(WRONG_TYPE.to_owned()),
        };
        let written = match files {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => Some(
                list.iter()
                    .map(|path| path.as_str().ok_or(WRONG_TYPE))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Some(_) => return Err(WRONG_TYPE.to_owned()),
        };
        let checkout = Checkout::parse(checkout)?;
        let files = match written {
            Some(written) if !written.is_empty() => Some(
                written
                    .into_iter()
                    .map(|path| file_path(path).map_err(|_| BAD_FILE.to_owned()))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            _ => None,
        };
        Ok(ToolInput { checkout, files })
    }
}

/// The first of `USUAL_NAMES` that reads at the checkout root, named relative to that root.
pub fn default_file(files: &dyn ReadFile, checkout: &Checkout) -> Option<String> {
    USUAL_NAMES
        .into_iter()
        .find(|name| files.read(&checkout.read_path(name)).is_ok())
        .map(str::to_owned)
}

/// What a tool found to load.
// The brief pins `Loaded(Project)`; one value lives for one call, so its size doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum ToolLoad {
    /// No file was named and none of the usual names reads at the checkout root.
    NoComposeFile,
    Loaded(Project),
}

/// Loads the files the input names, or else the default file, through `load_project`.
pub fn load_for_tool(files: &dyn ReadFile, input: &ToolInput) -> ToolLoad {
    let paths = match &input.files {
        Some(paths) => paths.clone(),
        None => match default_file(files, &input.checkout) {
            Some(path) => vec![path],
            None => return ToolLoad::NoComposeFile,
        },
    };
    ToolLoad::Loaded(load_project(files, &input.checkout, &paths))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::files::{MemFiles, RecordingFiles};

    const HORUS: &str = include_str!("../../fixtures/horus-like/infra/local/docker-compose.yml");

    #[test]
    fn paths_outside_the_project_are_refused() {
        for input in [
            json!({ "checkout": "../x" }),
            json!({ "checkout": "/etc" }),
            json!({ "checkout": "a\\b" }),
            json!({ "files": ["../x.yml"] }),
            json!({ "files": ["/etc/x.yml"] }),
            json!({ "files": ["."] }),
            json!({ "files": [""] }),
            json!({ "files": ["compose.yaml", "a\\b.yml"] }),
        ] {
            assert!(ToolInput::parse(&input).is_err(), "{input}");
        }
        assert_eq!(
            ToolInput::parse(&json!({ "checkout": "../x" })).unwrap_err(),
            crate::paths::CHECKOUT_REFUSED
        );
        assert_eq!(
            ToolInput::parse(&json!({ "files": ["../x.yml"] })).unwrap_err(),
            "files must be paths inside the checkout, such as infra/local/docker-compose.yml"
        );
    }

    #[test]
    fn wrong_types_are_refused() {
        for input in [
            json!({ "checkout": 3 }),
            json!({ "checkout": ["a"] }),
            json!({ "files": "compose.yaml" }),
            json!({ "files": [1] }),
            json!({ "files": {} }),
            json!([]),
            json!("x"),
        ] {
            assert_eq!(
                ToolInput::parse(&input).unwrap_err(),
                "checkout must be text and files a list of paths",
                "{input}"
            );
        }
    }

    #[test]
    fn checkout_and_files_are_optional() {
        for input in [
            json!({}),
            json!(null),
            json!({ "checkout": null, "files": null }),
            json!({ "checkout": "", "files": [] }),
        ] {
            assert_eq!(
                ToolInput::parse(&input),
                Ok(ToolInput {
                    checkout: Checkout::primary(),
                    files: None
                }),
                "{input}"
            );
        }
        let input = ToolInput::parse(
            &json!({ "checkout": ".jensen/worktrees/a/", "files": ["./infra/../compose.yml", "x/y.yaml"] }),
        )
        .unwrap();
        assert_eq!(input.checkout.root(), ".jensen/worktrees/a");
        assert_eq!(
            input.files,
            Some(vec!["compose.yml".to_owned(), "x/y.yaml".to_owned()])
        );
    }

    #[test]
    fn the_first_usual_name_at_the_checkout_root_wins() {
        let files = MemFiles::from([
            (".jensen/worktrees/a/compose.yml", "services: {}\n"),
            (".jensen/worktrees/a/docker-compose.yml", "services: {}\n"),
            ("compose.yaml", "services: {}\n"),
        ]);
        let checkout = Checkout::parse(Some(".jensen/worktrees/a")).unwrap();
        assert_eq!(
            default_file(&files, &checkout).as_deref(),
            Some("compose.yml")
        );
        assert_eq!(
            default_file(&files, &Checkout::primary()).as_deref(),
            Some("compose.yaml")
        );

        let input = ToolInput {
            checkout,
            files: None,
        };
        let ToolLoad::Loaded(project) = load_for_tool(&files, &input) else {
            panic!("expected a project");
        };
        assert_eq!(project.model.files, ["compose.yml"]);
    }

    #[test]
    fn no_usual_name_means_no_compose_file() {
        // Only the root counts: nothing below it, and no override file.
        let files = RecordingFiles::new(MemFiles::from([
            ("infra/local/docker-compose.yml", HORUS),
            ("compose.override.yaml", "services: {}\n"),
            ("compose.json", "{}"),
        ]));
        let input = ToolInput::parse(&json!({})).unwrap();
        assert!(matches!(
            load_for_tool(&files, &input),
            ToolLoad::NoComposeFile
        ));
        assert_eq!(default_file(&files, &Checkout::primary()), None);
        assert_eq!(files.reads()[..4], USUAL_NAMES);
    }

    #[test]
    fn named_files_skip_discovery() {
        let files = RecordingFiles::new(MemFiles::from([
            ("compose.yaml", "services: {}\n"),
            ("infra/local/docker-compose.yml", HORUS),
        ]));
        let input =
            ToolInput::parse(&json!({ "files": ["infra/local/docker-compose.yml"] })).unwrap();
        let ToolLoad::Loaded(project) = load_for_tool(&files, &input) else {
            panic!("expected a project");
        };
        assert_eq!(project.model.files, ["infra/local/docker-compose.yml"]);
        assert_eq!(files.reads(), ["infra/local/docker-compose.yml"]);
    }

    #[test]
    fn the_default_file_follows_include() {
        let files = MemFiles::from([
            (
                "compose.yaml",
                "include:\n  - infra/db.yaml\nservices:\n  web:\n    image: nginx\n",
            ),
            ("infra/db.yaml", "services:\n  db:\n    image: postgres\n"),
        ]);
        let input = ToolInput::parse(&json!({})).unwrap();
        let ToolLoad::Loaded(project) = load_for_tool(&files, &input) else {
            panic!("expected a project");
        };
        assert_eq!(project.model.files, ["compose.yaml", "infra/db.yaml"]);
        let services: Vec<_> = project.model.services.iter().map(|s| &*s.name).collect();
        assert_eq!(services, ["web", "db"]);
    }
}
