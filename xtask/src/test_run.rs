use crate::owned_dir::OwnedDir;
use anyhow::Context;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct TestRun(Arc<OwnedDir>);

/// Call in the serial test driver: fixtures and child servers must start with private directories.
pub fn private_creation_mask() {
    // SAFETY: umask has no memory preconditions; the CLI sets it before launching test processes.
    unsafe { libc::umask(0o077) };
}

impl TestRun {
    pub fn new() -> anyhow::Result<Self> {
        // /tmp stays short on macOS too: its default temp_dir can exhaust sun_path.
        // SAFETY: geteuid has no preconditions.
        let parent = std::env::var_os("YINZ_TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(format!("/tmp/yz-runs-{}", unsafe { libc::geteuid() }))
            });
        let root = OwnedDir::new(&parent).context("failed to create the test run directory")?;
        root.inherit_lock()
            .context("failed to pass the test run lease to child processes")?;
        std::fs::create_dir(root.path().join("tmp"))
            .context("failed to create the test run's temporary directory")?;
        println!(
            ">> Temporary test files: {} (removed when the runner finishes)",
            root.path().display()
        );
        Ok(Self(root))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.path().join("tmp")
    }

    /// Snapshot once, before launching any tests. All nextest processes borrow this
    /// immutable generation, and the inherited run lease keeps it alive for them.
    /// Never link a mutable Cargo artifact into the run.
    pub fn retain_worker(&self, binary: &Path) -> anyhow::Result<PathBuf> {
        let directory = self.path().join("worker");
        std::fs::create_dir(&directory).context("create shared test worker directory")?;
        let worker = directory.join("yinz");
        std::fs::copy(binary, &worker).context("retain shared test worker generation")?;
        println!(">> Shared test worker generation: {}", worker.display());
        Ok(worker)
    }
}

#[cfg(test)]
#[path = "unit/test_run.rs"]
mod tests;
