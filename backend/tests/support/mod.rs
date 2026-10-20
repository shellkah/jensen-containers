//! The fixture projects under `fixtures/`, read from disk.

use std::path::PathBuf;

use containers::files::{ReadFile, RecordingFiles};

/// Reads a fixture project's files: paths are relative to its folder, as they are to a checkout.
pub struct DirFiles {
    pub root: PathBuf,
}

impl ReadFile for DirFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        std::fs::read_to_string(self.root.join(path)).map_err(|error| error.to_string())
    }
}

/// The fixture project `case`, recording every path read.
pub fn fixture(case: &str) -> RecordingFiles<DirFiles> {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/")).join(case);
    RecordingFiles::new(DirFiles { root })
}
