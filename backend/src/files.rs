//! Where Compose files come from: Jensen's `fs` in the plugin, memory or a folder under test.

use std::cell::RefCell;
use std::collections::BTreeMap;

/// Reads a file's text. Paths are relative to the project root, as Jensen's `fs` takes them.
pub trait ReadFile {
    fn read(&self, path: &str) -> Result<String, String>;
}

/// Reads through Jensen's `fs::read`, within the scopes the person granted the plugin.
pub struct HostFiles;

impl ReadFile for HostFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        utf8(jensen_plugin_backend::fs::read(path)?)
    }
}

/// The text of a file's bytes; Compose files are UTF-8.
fn utf8(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "is not UTF-8 text".to_owned())
}

/// Files held in memory, by path.
#[derive(Clone, Debug, Default)]
pub struct MemFiles(pub BTreeMap<String, String>);

impl<const N: usize> From<[(&str, &str); N]> for MemFiles {
    fn from(files: [(&str, &str); N]) -> MemFiles {
        MemFiles(
            files
                .into_iter()
                .map(|(path, text)| (path.to_owned(), text.to_owned()))
                .collect(),
        )
    }
}

impl ReadFile for MemFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| "no such file".to_owned())
    }
}

/// Wraps a reader and keeps every path asked of it, in order, whether the read worked or not.
pub struct RecordingFiles<R: ReadFile> {
    inner: R,
    reads: RefCell<Vec<String>>,
}

impl<R: ReadFile> RecordingFiles<R> {
    pub fn new(inner: R) -> RecordingFiles<R> {
        RecordingFiles {
            inner,
            reads: RefCell::new(Vec::new()),
        }
    }

    /// Every path asked for so far, in order.
    pub fn reads(&self) -> Vec<String> {
        self.reads
            .try_borrow()
            .map(|reads| reads.clone())
            .unwrap_or_default()
    }
}

impl<R: ReadFile> ReadFile for RecordingFiles<R> {
    fn read(&self, path: &str) -> Result<String, String> {
        // Nothing else holds the list while a read runs, so the borrow always succeeds.
        if let Ok(mut reads) = self.reads.try_borrow_mut() {
            reads.push(path.to_owned());
        }
        self.inner.read(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_files_answer_their_text_or_no_such_file() {
        let files = MemFiles::from([("compose.yaml", "services: {}\n"), ("a/b.yml", "x: 1\n")]);
        assert_eq!(files.read("compose.yaml").as_deref(), Ok("services: {}\n"));
        assert_eq!(files.read("a/b.yml").as_deref(), Ok("x: 1\n"));
        assert_eq!(files.read("missing.yml"), Err("no such file".to_string()));
    }

    #[test]
    fn recording_files_list_every_path_asked_for() {
        let files = RecordingFiles::new(MemFiles::from([("a.yml", "a: 1\n")]));
        assert!(files.read("a.yml").is_ok());
        assert!(files.read("b.yml").is_err());
        assert!(files.read("a.yml").is_ok());
        assert_eq!(files.reads(), ["a.yml", "b.yml", "a.yml"]);
    }

    #[test]
    fn host_files_read_text_and_refuse_other_bytes() {
        // `cargo test` runs in the crate's folder, where the host reads the native file system.
        let manifest = HostFiles.read("Cargo.toml").unwrap();
        assert!(manifest.contains("[package]"), "{manifest}");
        assert!(HostFiles.read("no/such/file.yml").is_err());
        assert_eq!(
            utf8(vec![b'a', 0xff, b'b']),
            Err("is not UTF-8 text".to_string())
        );
        assert_eq!(utf8("é: x\n".into()), Ok("é: x\n".to_string()));
    }
}
