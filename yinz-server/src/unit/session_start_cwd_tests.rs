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
