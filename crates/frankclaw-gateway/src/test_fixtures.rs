use std::path::Path;

use tempfile::{Builder, TempDir};

pub(crate) struct TestTempDir {
    dir: TempDir,
}

impl TestTempDir {
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("temp dir should create");
        Self { dir }
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }
}
