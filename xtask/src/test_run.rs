use crate::owned_dir::OwnedDir;
use anyhow::Context;
use std::{path::Path, sync::Arc};

pub struct TestRun(Arc<OwnedDir>);

impl TestRun {
    pub fn new() -> anyhow::Result<Self> {
        // /tmp stays short on macOS too: its default temp_dir can exhaust sun_path.
        // SAFETY: geteuid has no preconditions.
        let parent =
            std::path::PathBuf::from(format!("/tmp/zj-runs-{}", unsafe { libc::geteuid() }));
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
mod tests {
    use super::*;
    use crate::owned_dir::TEST_CACHE_ROOT_ENV;
    use std::io::{BufRead, Read};
    use std::process::{Command, Stdio};

    #[test]
    fn test_child() {
        let Ok(mode) = std::env::var("ZELLIJ_TEST_RUN_CHILD") else {
            return;
        };
        let cache = std::path::PathBuf::from(std::env::var_os(TEST_CACHE_ROOT_ENV).unwrap())
            .join(std::process::id().to_string())
            .join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("artifact"), "test output").unwrap();
        println!("READY");
        if mode == "killed" {
            let _ = std::io::stdin().read(&mut [0]);
        }
        if mode == "failed" {
            std::process::exit(1);
        }
    }

    #[test]
    fn removes_artifacts_after_passing_failing_and_killed_children() {
        for mode in ["passed", "failed", "killed"] {
            let run = TestRun::new().unwrap();
            let path = run.path().to_path_buf();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "test_run::tests::test_child", "--nocapture"])
                .env(TEST_CACHE_ROOT_ENV, &path)
                .env("ZELLIJ_TEST_RUN_CHILD", mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    if line.unwrap() == "READY" {
                        send.send(()).unwrap();
                    }
                }
            });
            let ready = receive.recv_timeout(std::time::Duration::from_secs(30));
            if ready.is_err() || mode == "killed" {
                child.kill().unwrap();
            }
            let status = child.wait().unwrap();
            ready.unwrap();
            assert_eq!(status.success(), mode == "passed");
            assert!(path
                .join(child.id().to_string())
                .join("cache/artifact")
                .exists());
            drop(run);
            assert!(!path.exists(), "{mode} child's files survived runner exit");
        }
    }
}
