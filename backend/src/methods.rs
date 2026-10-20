//! The backend's methods: what Jensen's plugin can ask of the wasm program.

use serde_json::Value;

use crate::compose::load_project;
use crate::files::ReadFile;
use crate::model::AnalyzeResult;
use crate::paths::{CHECKOUT_REFUSED, Checkout, file_path};
use crate::rules::readiness;

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
