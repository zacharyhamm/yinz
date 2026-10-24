use super::*;

#[test]
fn socket_root_precedence_ignores_tmpdir() {
    const PROBE: &str = "YINZ_SOCKET_ROOT_PROBE";
    if let Some(expected) = std::env::var_os(PROBE) {
        assert_eq!(
            *YINZ_SOCK_DIR,
            PathBuf::from(expected).join(&*CLIENT_SERVER_CONTRACT_DIR)
        );
        return;
    }
    // Resolve the real lazy static in fresh processes, without changing the test runner's env.
    for (socket_override, runtime, expected) in [
        (
            Some("/socket-override"),
            Some("/runtime"),
            "/socket-override",
        ),
        #[cfg(not(target_os = "macos"))]
        (None, Some("/runtime"), "/runtime/yinz"),
        (None, None, "/Users/zacharyhamm/.yinz/run"),
    ] {
        for tmpdir in ["/tmp", "/var/folders/zz/zyxvpxvq6csfxvn_n0000000000000/T"] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "consts::socket_dir_tests::socket_root_precedence_ignores_tmpdir",
                ])
                .env(PROBE, expected)
                .env("HOME", "/Users/zacharyhamm")
                .env("TMPDIR", tmpdir)
                .env_remove("YINZ_SOCKET_DIR")
                .env_remove("XDG_RUNTIME_DIR");
            if let Some(dir) = socket_override {
                command.env("YINZ_SOCKET_DIR", dir);
            }
            if let Some(dir) = runtime {
                command.env("XDG_RUNTIME_DIR", dir);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "socket root {expected} with TMPDIR={tmpdir}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// Builds share one local socket directory per contract; `cv1` is where sessions from builds
/// before and after the session-switch capability meet.
#[test]
fn socket_dir_is_contract_one() {
    assert_eq!(&*CLIENT_SERVER_CONTRACT_DIR, "cv1");
    assert!(YINZ_SOCK_DIR.ends_with("cv1"));
}
