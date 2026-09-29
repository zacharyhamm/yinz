use super::*;

#[test]
fn supervisor_starts_after_binary_replacement() {
    const CHILD_ENV: &str = "YINZ_TEST_SWAMP_BINARY_REPLACEMENT";
    if std::env::var_os(CHILD_ENV).is_none() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("server");
        std::fs::copy(std::env::current_exe().unwrap(), &binary).unwrap();
        let output = Command::new(binary)
            .args([
                "--exact",
                "supervisor_tests::supervisor_starts_after_binary_replacement",
            ])
            .env(CHILD_ENV, "1")
            .env_remove("YINZ_TEST_BINARY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }

    // The server initializes this cache before starting any managed supervisors.
    let binary = yinz_runtime::own_binary::server_binary().unwrap();
    let replacement = binary.with_extension("new");
    yinz_test_support::write_warm_script(
        &replacement,
        "printf '%s\\n' \"$@\" > \"$0.args\"\necho replacement-supervisor >&2\nexit 1\n",
    );
    std::fs::rename(replacement, binary).unwrap();

    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let error = prewarm(&root).unwrap_err();
    std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    assert_eq!(
        error.to_string(),
        "managed Swamp startup failed: replacement-supervisor",
        "{error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(binary.with_extension("args")).unwrap(),
        format!("{SUPERVISOR_FLAG}\n{}\n", root.display())
    );
}
