use super::Selection;
use std::ffi::OsString;

fn selection(args: &[&str]) -> Selection {
    super::selection(&args.iter().map(OsString::from).collect::<Vec<_>>()).0
}

fn filtered(args: &[&str]) -> bool {
    super::selection(&args.iter().map(OsString::from).collect::<Vec<_>>()).1
}

fn packages(args: &[&str]) -> Selection {
    Selection::Packages(args.iter().map(OsString::from).collect())
}

fn workspace(args: &[&str]) -> Selection {
    Selection::Workspace(args.iter().map(OsString::from).collect())
}

#[test]
fn no_arguments_means_the_whole_workspace() {
    assert_eq!(selection(&[]), workspace(&[]));
}

#[test]
fn package_selectors_carry_over() {
    assert_eq!(
        selection(&["-p", "yinz-server"]),
        packages(&["-p", "yinz-server"])
    );
    assert_eq!(
        selection(&["--package=yinz-types"]),
        packages(&["--package=yinz-types"])
    );
    assert_eq!(
        selection(&["--exclude", "yinz-server", "-p", "yinz-types"]),
        packages(&["--exclude", "yinz-server", "-p", "yinz-types"])
    );
}

#[test]
fn an_exclude_alone_leaves_the_run_at_workspace_scope() {
    assert_eq!(
        selection(&["--exclude", "yinz-server"]),
        workspace(&["--exclude", "yinz-server"])
    );
}

#[test]
fn a_filterset_skips_the_doctest_pass() {
    assert!(filtered(&["-E", "test(x)"]));
    assert!(filtered(&["--filterset=test(x)"]));
    assert!(filtered(&["-p", "yinz-server", "-E", "test(x)"]));
}

#[test]
fn test_filters_preserve_package_scope_in_either_order() {
    for args in [
        vec!["-p", "yinz-server", "-E", "test(x)"],
        vec!["-E", "test(x)", "-p", "yinz-server"],
        vec!["some_test", "-p", "yinz-server"],
        vec!["-p", "yinz-server", "some_test"],
    ] {
        assert_eq!(selection(&args), packages(&["-p", "yinz-server"]));
        assert!(filtered(&args));
    }
}

#[test]
fn a_bare_filter_skips_the_doctest_pass() {
    assert!(filtered(&["some_test_name"]));
    assert!(filtered(&["--no-capture", "some_test_name"]));
    assert!(filtered(&["-j", "1", "some_test_name"]));
}

#[test]
fn output_and_concurrency_flags_do_not_narrow() {
    assert!(!filtered(&["--no-capture", "-j", "1", "-p", "yinz-types"]));
    assert_eq!(selection(&["--no-capture"]), workspace(&[]));
    assert_eq!(selection(&["-j", "1"]), workspace(&[]));
    assert_eq!(
        selection(&["--no-capture", "-j", "1", "-p", "yinz-types"]),
        packages(&["-p", "yinz-types"])
    );
}

#[test]
fn package_names_are_read_out_of_the_selectors() {
    assert_eq!(
        super::selected_package_names(
            &[
                "--exclude",
                "yinz-server",
                "-p",
                "yinz-types",
                "--package=yinz-tile"
            ]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        ),
        ["yinz-types", "yinz-tile"]
    );
}

#[test]
fn a_value_of_an_unknown_flag_is_not_a_filter() {
    assert_eq!(
        selection(&["--future-nextest-flag", "value"]),
        workspace(&[])
    );
}
