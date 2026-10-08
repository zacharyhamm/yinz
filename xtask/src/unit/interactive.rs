use super::*;
use std::io::Write;

#[test]
fn interactive_command_inherits_streams_and_preserves_command_context() {
    const CHILD: &str = "YINZ_XTASK_INTERACTIVE_TEST";
    if let Some(directory) = std::env::var_os(CHILD) {
        let sh = Shell::new().unwrap();
        sh.change_dir(directory);
        let _env = sh.push_env("YINZ_TEST_SHELL_ENV", "shell value");
        let script = r#"IFS= read -r input || exit 2
printf 'input=<%s> arg=<%s> shell=<%s> command=<%s>\n' "$input" "$1" "$YINZ_TEST_SHELL_ENV" "$YINZ_TEST_CMD_ENV"
pwd
printf 'inherited stderr\n' >&2"#;
        let arg = "one argument with spaces";
        run_interactive(
            cmd!(sh, "/bin/sh -c {script} -- {arg}").env("YINZ_TEST_CMD_ENV", "command value"),
        )
        .unwrap();
        let failure = run_interactive(cmd!(sh, "/bin/sh -c 'exit 7'")).unwrap_err();
        assert!(
            failure.to_string().contains("failed with exit status: 7"),
            "{failure}"
        );
        let missing = sh.current_dir().join("missing-command");
        let failure = run_interactive(sh.cmd(missing)).unwrap_err();
        assert!(failure.to_string().contains("failed to spawn"), "{failure}");
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pipelines::interactive_tests::interactive_command_inherits_streams_and_preserves_command_context", "--nocapture"])
        .env(CHILD, directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"inherited input\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("input=<inherited input> arg=<one argument with spaces> shell=<shell value> command=<command value>"), "{stdout}");
    assert!(
        stdout.contains(directory.path().canonicalize().unwrap().to_str().unwrap()),
        "{stdout}"
    );
    assert!(stderr.contains("inherited stderr"), "{stderr}");
}
