use super::*;

#[test]
fn socket_root_precedence_ignores_tmpdir() {
    const PROBE: &str = "ZELLIJ_SOCKET_ROOT_PROBE";
    if let Some(expected) = std::env::var_os(PROBE) {
        assert_eq!(
            *ZELLIJ_SOCK_DIR,
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
        (None, Some("/runtime"), "/runtime/zellij"),
        (None, None, "/Users/zacharyhamm/.zellij/run"),
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
                .env_remove("ZELLIJ_SOCKET_DIR")
                .env_remove("XDG_RUNTIME_DIR");
            if let Some(dir) = socket_override {
                command.env("ZELLIJ_SOCKET_DIR", dir);
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
