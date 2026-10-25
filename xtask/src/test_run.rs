use crate::owned_dir::OwnedDir;
use anyhow::Context;
use std::{path::Path, sync::Arc};

pub struct TestRun(Arc<OwnedDir>);

impl TestRun {
    pub fn new() -> anyhow::Result<Self> {
        // /tmp stays short on macOS too: its default temp_dir can exhaust sun_path.
        // SAFETY: geteuid has no preconditions.
        let parent =
            std::path::PathBuf::from(format!("/tmp/yz-runs-{}", unsafe { libc::geteuid() }));
        let root = OwnedDir::new(&parent).context("failed to create the test run directory")?;
        root.inherit_lock()
            .context("failed to pass the test run lease to child processes")?;
        println!(
            ">> Temporary test files: {} (removed when the runner finishes)",
            root.path().display()
        );
        Ok(Self(root))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

#[cfg(test)]
#[path = "unit/test_run.rs"]
mod tests;
