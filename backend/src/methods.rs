//! The backend's methods: what Jensen's plugin can ask of the wasm program.

use serde_json::{Value, json};

use crate::compose::{Project, load_project};
use crate::files::ReadFile;
use crate::model::{AnalyzeResult, Finding};
use crate::paths::{CHECKOUT_REFUSED, Checkout, file_path};
use crate::render::{no_compose_file_text, readiness_text, topology_text};
use crate::rules::readiness;
use crate::tools::{ToolInput, ToolLoad, load_for_tool};

/// Loads the Compose files of `paths`, relative to the checkout root, into the project model.
/// Findings are the readiness rules' on that project, `same_project_name` included: the backend
/// can't list folders, so whoever shows them drops it when the project has no worktree folder.
pub fn analyze(files: &dyn ReadFile, checkout: &Checkout, paths: &[String]) -> AnalyzeResult {
    let project = load_project(files, checkout, paths);
    let findings = readiness(&project);
    AnalyzeResult {
        model: project.model,
        findings,
        unresolved: project.unresolved,
        errors: project.errors,
    }
}

/// `analyze` as Jensen calls it: `{ files: string[], checkout?: string }` in, an `AnalyzeResult` out.
pub fn analyze_method(files: &dyn ReadFile, input: Value) -> Result<Value, String> {
    let paths = input
        .get("files")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .ok_or("files must be a list of Compose file paths")?;
    let paths = paths
        .into_iter()
        .map(file_path)
        .collect::<Result<Vec<_>, _>>()?;
    let checkout = match input.get("checkout") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => Some(path.as_str()),
        Some(_) => return Err(CHECKOUT_REFUSED.to_owned()),
    };
    let checkout = Checkout::parse(checkout)?;
    serde_json::to_value(analyze(files, &checkout, &paths))
        .map_err(|error| format!("the result can't be written as json: {error}"))
}

/// `topology` as an agent calls it: `{ checkout?, files? }` in, `{ text }` out.
pub fn topology_method(files: &dyn ReadFile, input: Value) -> Result<Value, String> {
    answer(files, &input, |project, _, checkout| {
        topology_text(project, checkout)
    })
}

/// `readiness` as an agent calls it: `{ checkout?, files? }` in, `{ text }` out.
pub fn readiness_method(files: &dyn ReadFile, input: Value) -> Result<Value, String> {
    answer(files, &input, |project, findings, checkout| {
        readiness_text(project, findings, checkout)
    })
}

/// Parses the input, loads the project and writes the text `render` makes of it.
fn answer(
    files: &dyn ReadFile,
    input: &Value,
    render: impl FnOnce(&Project, &[Finding], &Checkout) -> String,
) -> Result<Value, String> {
    let input = ToolInput::parse(input)?;
    let text = match load_for_tool(files, &input) {
        ToolLoad::NoComposeFile => no_compose_file_text(&input.checkout),
        ToolLoad::Loaded(project) => {
            let findings = readiness(&project);
            render(&project, &findings, &input.checkout)
        }
    };
    Ok(json!({ "text": text }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::files::{MemFiles, RecordingFiles};

    #[test]
    fn analyze_method_reads_the_given_files() {
        let files = MemFiles::from([(
            ".jensen/worktrees/a/infra/compose.yaml",
            "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n",
        )]);

        let reply = analyze_method(
            &files,
            json!({ "files": ["infra/compose.yaml"], "checkout": ".jensen/worktrees/a" }),
        )
        .unwrap();

        assert_eq!(reply["model"]["files"], json!(["infra/compose.yaml"]));
        assert_eq!(reply["model"]["projectName"]["value"], json!("infra"));
        assert_eq!(reply["model"]["services"][0]["name"], json!("web"));
        assert_eq!(
            reply["model"]["services"][0]["ports"][0]["host"],
            json!("8080")
        );
        // The fixed port, then the folder name every checkout shares.
        let rules: Vec<_> = reply["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|finding| finding["rule"].clone())
            .collect();
        assert_eq!(
            rules,
            [json!("fixed_host_port"), json!("same_project_name")]
        );
        assert_eq!(
            reply["findings"][1]["source"],
            json!({ "file": "infra/compose.yaml", "line": 0, "column": 0 })
        );
        assert_eq!(reply["errors"], json!([]));
    }

    #[test]
    fn analyze_method_refuses_bad_input() {
        let files = MemFiles::default();
        for input in [
            json!({}),
            json!({ "files": "x" }),
            json!({ "files": [1] }),
            json!({ "files": ["../x.yml"] }),
            json!({ "files": [""] }),
            json!({ "files": ["."] }),
            json!({ "files": ["compose.yaml", "./"] }),
            json!({ "files": [], "checkout": "/abs" }),
            json!({ "files": [], "checkout": 3 }),
        ] {
            assert!(
                analyze_method(&files, input.clone()).is_err(),
                "expected an error for {input}"
            );
        }
        assert_eq!(
            analyze_method(&files, json!({})).unwrap_err(),
            "files must be a list of Compose file paths"
        );
        // A path that names the checkout root would read the folder.
        assert_eq!(
            analyze_method(&files, json!({ "files": ["."] })).unwrap_err(),
            "\".\" names the checkout root, not a Compose file"
        );
    }

    const HORUS: &str = include_str!("../../fixtures/horus-like/infra/local/docker-compose.yml");

    fn horus_in_a_worktree() -> MemFiles {
        MemFiles::from([(
            ".jensen/worktrees/feat-x/infra/local/docker-compose.yml",
            HORUS,
        )])
    }

    #[test]
    fn topology_and_readiness_answer_text() {
        let files = horus_in_a_worktree();
        let input = json!({
            "checkout": ".jensen/worktrees/feat-x",
            "files": ["infra/local/docker-compose.yml"],
        });

        let topology = topology_method(&files, input.clone()).unwrap();
        assert_eq!(topology.as_object().unwrap().len(), 1);
        let topology = topology["text"].as_str().unwrap();
        assert!(
            topology.starts_with("Compose project \"local\", named after its folder infra/local,"),
            "{topology}"
        );

        let readiness = readiness_method(&files, input).unwrap();
        assert_eq!(readiness.as_object().unwrap().len(), 1);
        let readiness = readiness["text"].as_str().unwrap();
        assert!(readiness.starts_with("Readiness of infra/local/docker-compose.yml in .jensen/worktrees/feat-x: 3 problems stop copies of this stack from running side by side.\n"), "{readiness}");
        assert!(
            readiness.contains(
                "Fix: replace \"127.0.0.1:5432:5432\" with \"127.0.0.1:${DB_PORT:-5432}:5432\"."
            ),
            "{readiness}"
        );
    }

    #[test]
    fn readiness_keeps_the_project_name_without_a_checkout() {
        // A session in a worktree passes no checkout: Jensen reads its own files, so the backend
        // sees the primary checkout and must still tell of the name every checkout shares.
        let files = MemFiles::from([("infra/local/docker-compose.yml", HORUS)]);
        let input = json!({ "files": ["infra/local/docker-compose.yml"] });
        let reply = readiness_method(&files, input).unwrap();
        let text = reply["text"].as_str().unwrap();
        assert!(text.starts_with("Readiness of infra/local/docker-compose.yml: 3 problems stop copies of this stack from running side by side.\n1. db publishes host port 5432"), "{text}");
        assert!(
            text.contains("\n2. Every checkout names this Compose project \"local\""),
            "{text}"
        );
    }

    #[test]
    fn a_checkout_with_no_compose_file_is_told_to_pass_files() {
        let files = horus_in_a_worktree();
        let input = json!({ "checkout": ".jensen/worktrees/feat-x" });
        for method in [topology_method, readiness_method] {
            let reply = method(&files, input.clone()).unwrap();
            assert!(
                reply["text"]
                    .as_str()
                    .unwrap()
                    .starts_with("No Compose file at the root of .jensen/worktrees/feat-x"),
                "{reply}"
            );
            assert!(
                reply["text"].as_str().unwrap().contains("\nIf you are working in .jensen/worktrees/feat-x, leave checkout out: Jensen already reads your own checkout.\nNext: "),
                "{reply}"
            );
        }
    }

    #[test]
    fn no_checkout_and_no_compose_file_names_this_checkout_without_a_hint() {
        let files = MemFiles::default();
        for method in [topology_method, readiness_method] {
            let reply = method(&files, json!({})).unwrap();
            let text = reply["text"].as_str().unwrap();
            assert!(
                text.starts_with("No Compose file at the root of this checkout (looked for"),
                "{text}"
            );
            assert!(!text.contains("leave checkout out"), "{text}");
        }
    }

    #[test]
    fn a_checkout_that_doubles_the_callers_path_reads_nothing_and_is_told_so() {
        // Jensen roots the read in the caller's worktree, so the worktree's own path leads nowhere.
        let files = MemFiles::default();
        let input = json!({
            "checkout": ".jensen/worktrees/feat-x",
            "files": ["infra/local/docker-compose.yml"],
        });
        let hint = "If you are working in .jensen/worktrees/feat-x, leave checkout out: Jensen already reads your own checkout.";
        let reply = topology_method(&files, input.clone()).unwrap();
        let topology = reply["text"].as_str().unwrap();
        assert!(
            topology.starts_with(
                "Topology of infra/local/docker-compose.yml: the files could not be read.\n"
            ),
            "{topology}"
        );
        assert!(
            topology.contains(&format!("\n{hint}\nNext: ")),
            "{topology}"
        );
        let reply = readiness_method(&files, input).unwrap();
        let readiness = reply["text"].as_str().unwrap();
        assert!(
            readiness.contains(&format!("\n{hint}\nNext: ")),
            "{readiness}"
        );
    }

    #[test]
    fn tool_input_errors_are_errors() {
        let files = MemFiles::default();
        for method in [topology_method, readiness_method] {
            for input in [
                json!({ "checkout": "../x" }),
                json!({ "checkout": 3 }),
                json!({ "files": ["../x.yml"] }),
                json!({ "files": "compose.yaml" }),
            ] {
                assert!(method(&files, input.clone()).is_err(), "{input}");
            }
        }
        assert_eq!(
            readiness_method(&files, json!({ "checkout": "/etc" })).unwrap_err(),
            crate::paths::CHECKOUT_REFUSED
        );
    }

    #[test]
    fn readiness_names_a_file_that_could_not_be_read() {
        let files = MemFiles::default();
        let reply = readiness_method(&files, json!({ "files": ["missing.yml"] })).unwrap();
        let text = reply["text"].as_str().unwrap();
        assert!(text.starts_with("Readiness of missing.yml: "), "{text}");
        assert!(text.contains("\nCould not load missing.yml"), "{text}");
        assert!(!text.contains("Readiness of :"), "{text}");
    }

    #[test]
    fn analyze_method_lists_a_file_that_isnt_yaml_as_an_error() {
        let files = RecordingFiles::new(MemFiles::from([(".env", "SECRET=1\n")]));
        let reply = analyze_method(&files, json!({ "files": [".env"] })).unwrap();
        assert_eq!(
            reply["errors"],
            json!([{
                "file": ".env",
                "message": ".env is not a Compose file: only .yaml and .yml files are read"
            }])
        );
        assert!(files.reads().is_empty(), "{:?}", files.reads());
    }
}
