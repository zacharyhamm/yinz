use super::parse_osc7_path;
use std::path::PathBuf;

#[test]
fn osc7_parser_extracts_path() {
    let raw = b"file://host/home/user/foo";
    assert_eq!(parse_osc7_path(raw), Some(PathBuf::from("/home/user/foo")));
}

#[test]
fn osc7_parser_decodes_percent_encoded_path() {
    let raw = b"file://localhost/tmp/with%20space";
    assert_eq!(parse_osc7_path(raw), Some(PathBuf::from("/tmp/with space")));
}

#[test]
fn osc7_parser_rejects_missing_scheme() {
    assert_eq!(parse_osc7_path(b"/home/user"), None);
}

#[test]
fn osc7_parser_rejects_missing_path() {
    assert_eq!(parse_osc7_path(b"file://host"), None);
}

#[test]
fn osc7_parser_rejects_truncated_percent_escape() {
    assert_eq!(parse_osc7_path(b"file://host/foo%2"), None);
}

#[test]
fn osc7_parser_rejects_non_utf8_input() {
    let raw = &[0xFFu8, 0xFE, 0xFD];
    assert_eq!(parse_osc7_path(raw), None);
}
