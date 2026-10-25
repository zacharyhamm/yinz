use super::insert_forwarding_separator;
use std::ffi::OsString;

#[test]
fn publishing_is_not_a_command() {
    let help = super::flags::Xtask::from_vec(vec![OsString::from("--help")])
        .unwrap_err()
        .to_string();
    assert!(help.contains("install"), "{help}");
    assert!(!help.contains("publish"), "{help}");
    let error = super::flags::Xtask::from_vec(vec![OsString::from("publish")])
        .unwrap_err()
        .to_string();
    assert!(error.contains("unexpected argument"), "{error}");
}

fn split(args: &[&str]) -> Vec<String> {
    insert_forwarding_separator(args.iter().map(OsString::from).collect())
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_package_selector_is_forwarded() {
    assert_eq!(
        split(&["xtask", "test", "-p", "yinz-server"]),
        ["xtask", "test", "--", "-p", "yinz-server"]
    );
}

#[test]
fn a_bare_filter_is_forwarded() {
    assert_eq!(
        split(&["xtask", "test", "some_test_name"]),
        ["xtask", "test", "--", "some_test_name"]
    );
}

#[test]
fn an_explicit_separator_is_left_alone() {
    assert_eq!(
        split(&["xtask", "test", "--", "-p", "yinz-server"]),
        ["xtask", "test", "--", "-p", "yinz-server"]
    );
}

#[test]
fn a_bare_command_is_left_alone() {
    assert_eq!(split(&["xtask", "test"]), ["xtask", "test"]);
}

#[test]
fn integration_test_takes_flags_directly_too() {
    assert_eq!(
        split(&["xtask", "integration-test", "--serial", "-j", "4"]),
        ["xtask", "integration-test", "--serial", "--", "-j", "4"]
    );
}

#[test]
fn other_commands_and_their_positionals_are_untouched() {
    assert_eq!(
        split(&["xtask", "build", "-p", "--release"]),
        ["xtask", "build", "-p", "--release"]
    );
    assert_eq!(
        split(&["xtask", "install", "/path/to/yinz"]),
        ["xtask", "install", "/path/to/yinz"]
    );
    assert_eq!(split(&["xtask", "ci", "check"]), ["xtask", "ci", "check"]);
}
