use super::*;

fn options(default_cwd: Option<&str>) -> Options {
    Options {
        default_cwd: default_cwd.map(PathBuf::from),
        ..Default::default()
    }
}

#[test]
fn the_clients_directory_beats_the_configured_default() {
    assert_eq!(
        session_start_cwd(Some(Path::new("/asked/for")), &options(Some("/configured"))),
        Some(PathBuf::from("/asked/for"))
    );
}

#[test]
fn the_configured_default_is_used_when_the_client_names_no_directory() {
    assert_eq!(
        session_start_cwd(None, &options(Some("/configured"))),
        Some(PathBuf::from("/configured"))
    );
}

#[test]
fn no_directory_is_chosen_when_neither_names_one() {
    assert_eq!(session_start_cwd(None, &options(None)), None);
}

#[test]
fn a_deleted_startup_directory_returns_an_error() {
    const CHILD: &str = "YINZ_TEST_DELETED_SERVER_CWD";
    const NAME: &str = "session_start_cwd_tests::a_deleted_startup_directory_returns_an_error";
    if std::env::var_os(CHILD).is_some() {
        let directory = tempfile::tempdir().unwrap();
        std::env::set_current_dir(directory.path()).unwrap();
        directory.close().unwrap();
        let error = start_server(
            Box::new(crate::capturing_os::CapturingOs::default()),
            PathBuf::from("unused.sock"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("read server startup directory"));
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", NAME])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
