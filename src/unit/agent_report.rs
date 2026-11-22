use super::journal_data_dir;
use std::path::Path;

#[test]
fn hooks_find_the_journal_in_their_server_s_data_directory() {
    let default = yinz_runtime::home::get_default_data_dir();
    let session = || Some("/srv/session-data".into());
    // An explicit `--data-dir` wins over the pane's server.
    assert_eq!(
        journal_data_dir(Some(Path::new("/explicit")), session()),
        Path::new("/explicit")
    );
    assert_eq!(
        journal_data_dir(None, session()),
        Path::new("/srv/session-data")
    );
    // An unconfigured server exports an empty value, and only absolute paths are trusted:
    // a hook runs in its pane's working directory, not the server's.
    for unusable in [None, Some("".into()), Some("relative/data".into())] {
        assert_eq!(journal_data_dir(None, unusable), default);
    }
}
