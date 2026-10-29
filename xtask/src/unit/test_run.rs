use super::*;
use crate::owned_dir::TEST_CACHE_ROOT_ENV;
use std::io::{BufRead, Read};
use std::process::{Command, Stdio};

#[test]
fn test_creation_mask_is_private() {
    if std::env::var_os("YINZ_TEST_CREATION_MASK_CHILD").is_some() {
        private_creation_mask();
        let directory = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            directory.path().metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        return;
    }
    let output = Command::new("sh")
        .args(["-c", "umask 0002; exec \"$1\" --exact test_run::tests::test_creation_mask_is_private --nocapture", "sh"])
        .arg(std::env::current_exe().unwrap())
        .env("YINZ_TEST_CREATION_MASK_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_child() {
    let Ok(mode) = std::env::var("YINZ_TEST_RUN_CHILD") else {
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
            .env("YINZ_TEST_RUN_CHILD", mode)
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
