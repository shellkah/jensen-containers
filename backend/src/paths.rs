//! Paths relative to the checkout root, and the checkout they are read under.

/// Why a checkout was refused.
pub const CHECKOUT_REFUSED: &str =
    "checkout must be a folder inside the project, such as .jensen/worktrees/<name>";

/// The folder a project's files are read under: the primary checkout (the project root) or a
/// worktree folder inside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkout {
    /// Relative to the project root, normalized, with no trailing `/`; empty for the primary checkout.
    root: String,
}

impl Checkout {
    pub fn primary() -> Checkout {
        Checkout::default()
    }

    /// A checkout as callers name it: none or `""` is the primary checkout; otherwise a folder
    /// inside the project, never absolute, never through `..`, never with `\`.
    pub fn parse(value: Option<&str>) -> Result<Checkout, String> {
        let value = value.unwrap_or_default();
        if value.split('/').any(|segment| segment == "..") {
            return Err(CHECKOUT_REFUSED.to_owned());
        }
        let root = normalize(value).map_err(|_| CHECKOUT_REFUSED.to_owned())?;
        Ok(Checkout { root })
    }

    /// The checkout's folder relative to the project root; empty for the primary checkout.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Where Jensen reads `path`, a path relative to the checkout root: under the checkout's
    /// folder, or as is in the primary checkout.
    pub fn read_path(&self, path: &str) -> String {
        if self.root.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.root)
        }
    }

    /// The checkout folder's own name; none for the primary checkout, whose name the backend can't
    /// see.
    pub fn folder_name(&self) -> Option<&str> {
        self.root.rsplit('/').next().filter(|name| !name.is_empty())
    }

    /// Whether the checkout is a worktree folder: `.jensen/worktrees/<name>` or
    /// `.claude/worktrees/<name>`, or a folder below one. Not the primary checkout, and not the
    /// `worktrees` folder itself.
    pub fn is_worktree(&self) -> bool {
        let mut segments = self.root.split('/');
        matches!(
            (segments.next(), segments.next(), segments.next()),
            (Some(".jensen" | ".claude"), Some("worktrees"), Some(name)) if !name.is_empty()
        )
    }
}

/// A file path from a caller, relative to the checkout root, normalized. One that leaves the
/// checkout is refused, and so is one that names the checkout root itself (`""`, `.`), which would
/// read the folder.
pub fn file_path(written: &str) -> Result<String, String> {
    match normalize(written)? {
        path if path.is_empty() => Err(format!(
            "{written:?} names the checkout root, not a Compose file"
        )),
        path => Ok(path),
    }
}

/// `path` with `.` and empty segments dropped and `a/../b` folded. A path that climbs above the
/// root, an absolute one, one starting with `~`, and one with `\` are refused.
pub fn normalize(path: &str) -> Result<String, String> {
    if path.contains('\\') {
        return Err(format!("{path} uses \\; write paths with /"));
    }
    if is_absolute(path) {
        return Err(format!(
            "{path} is not inside the checkout: paths are relative to its root"
        ));
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(format!("{path} leaves the checkout"));
                }
            }
            _ => segments.push(segment),
        }
    }
    Ok(segments.join("/"))
}

/// `/x`, `~/x` and a drive such as `C:/x` all name a place outside the checkout.
fn is_absolute(path: &str) -> bool {
    let drive = matches!(path.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic());
    path.starts_with('/') || path.starts_with('~') || drive
}

/// `path` relative to the folder `dir`, normalized; an absolute `path` is refused, not joined.
pub fn join(dir: &str, path: &str) -> Result<String, String> {
    if dir.is_empty() || is_absolute(path) {
        normalize(path)
    } else {
        normalize(&format!("{dir}/{path}"))
    }
}

/// The folder holding `path`; `""` at the root.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The project name Compose derives from a folder name: lowercased, keeping `[a-z0-9_-]`, without
/// leading `_` and `-`.
pub fn compose_project_name(folder: &str) -> String {
    let kept: String = folder
        .to_lowercase()
        .chars()
        .filter(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
        .collect();
    kept.trim_start_matches(['_', '-']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_and_kept_inside() {
        for (path, normal) in [
            ("compose.yaml", "compose.yaml"),
            ("./infra/./local/compose.yaml", "infra/local/compose.yaml"),
            ("infra/../compose.yaml", "compose.yaml"),
            ("infra//local/", "infra/local"),
            ("a/b/../../c", "c"),
            (".", ""),
            ("", ""),
            ("a/.../b", "a/.../b"),
            ("a/~b", "a/~b"),
        ] {
            assert_eq!(normalize(path).as_deref(), Ok(normal), "{path:?}");
        }
        for path in [
            "..",
            "../compose.yaml",
            "a/../../b",
            "/etc/compose.yaml",
            "~/compose.yaml",
            "infra\\compose.yaml",
            "C:/compose.yaml",
        ] {
            let error = normalize(path).unwrap_err();
            assert!(error.contains(path), "{path:?}: {error}");
        }

        assert_eq!(join("infra/local", ".").as_deref(), Ok("infra/local"));
        assert_eq!(join("infra/local", "../app").as_deref(), Ok("infra/app"));
        assert_eq!(
            join("infra/local", "./db/x.yml").as_deref(),
            Ok("infra/local/db/x.yml")
        );
        assert_eq!(join("", "compose.yaml").as_deref(), Ok("compose.yaml"));
        assert_eq!(join("", ".").as_deref(), Ok(""));
        assert!(join("infra", "../../x.yml").is_err());
        assert!(join("", "../x.yml").is_err());
        assert!(join("infra", "/x.yml").is_err());
        assert!(join("infra", "~/x.yml").is_err());
        assert!(join("infra", "a\\x.yml").is_err());

        assert_eq!(Checkout::parse(None), Ok(Checkout::primary()));
        assert_eq!(Checkout::parse(Some("")), Ok(Checkout::primary()));
        assert_eq!(Checkout::parse(Some(".")), Ok(Checkout::primary()));
        let worktree = Checkout::parse(Some(".jensen/worktrees/feat-x/")).unwrap();
        assert_eq!(worktree.root(), ".jensen/worktrees/feat-x");
        assert_eq!(
            Checkout::parse(Some("./.jensen//worktrees/feat-x")).unwrap(),
            worktree
        );
        for value in [
            "/home/me/project",
            "../other",
            ".jensen/worktrees/../x",
            ".jensen\\worktrees\\x",
            "~/project",
        ] {
            assert_eq!(
                Checkout::parse(Some(value)),
                Err(CHECKOUT_REFUSED.to_string()),
                "{value:?}"
            );
        }
    }

    #[test]
    fn checkouts_prefix_reads_and_name_their_folder() {
        let primary = Checkout::primary();
        assert_eq!(primary.root(), "");
        assert_eq!(
            primary.read_path("infra/compose.yaml"),
            "infra/compose.yaml"
        );
        assert_eq!(primary.folder_name(), None);

        let worktree = Checkout::parse(Some(".jensen/worktrees/Feat.X")).unwrap();
        assert_eq!(
            worktree.read_path("infra/compose.yaml"),
            ".jensen/worktrees/Feat.X/infra/compose.yaml"
        );
        assert_eq!(worktree.folder_name(), Some("Feat.X"));
        let top = Checkout::parse(Some("copy")).unwrap();
        assert_eq!(top.folder_name(), Some("copy"));
        assert_eq!(top.read_path("compose.yaml"), "copy/compose.yaml");
    }

    #[test]
    fn parents_and_project_names() {
        assert_eq!(parent("infra/local/docker-compose.yml"), "infra/local");
        assert_eq!(parent("infra/compose.yaml"), "infra");
        assert_eq!(parent("compose.yaml"), "");
        assert_eq!(parent(""), "");

        for (folder, name) in [
            ("local", "local"),
            ("Feat.X", "featx"),
            ("My App", "myapp"),
            ("_-_app", "app"),
            ("-x_y-1", "x_y-1"),
            ("été-2", "t-2"),
            ("___", ""),
            ("", ""),
        ] {
            assert_eq!(compose_project_name(folder), name, "{folder:?}");
        }
    }
}
