use super::*;

#[test]
fn terminal_titles_cannot_terminate_the_osc_or_inject_terminal_commands() {
    let title = make_terminal_title("日本語\x07\x1b]52;c;payload\x1b\\\r\n\u{9c}\u{2028}");
    let payload = title
        .strip_prefix("\x1b]0;")
        .unwrap()
        .strip_suffix('\x07')
        .unwrap();
    assert!(payload.contains("日本語"));
    assert!(!payload.chars().any(char::is_control));
    assert!(!payload.contains('\u{2028}'));
    assert!(make_terminal_title("plain title").ends_with("plain title\x07"));
}
