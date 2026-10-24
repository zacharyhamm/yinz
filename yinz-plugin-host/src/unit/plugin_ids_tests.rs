use super::plugins::max_session_name_bytes;
use std::path::Path;
use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;

#[test]
fn session_name_limit_reserves_directory_separator_and_nul_bytes() {
    for directory in ["/tmp/yinz", "/tmp/yinz/", "/tmp/é", "/", ""] {
        let directory = Path::new(directory);
        let max_bytes = max_session_name_bytes(directory);
        let longest_path = directory.join("x".repeat(max_bytes));
        assert_eq!(longest_path.as_os_str().len(), YINZ_SOCK_MAX_LENGTH - 1);
    }
    assert_eq!(
        max_session_name_bytes(Path::new(&"x".repeat(YINZ_SOCK_MAX_LENGTH))),
        0
    );
}
