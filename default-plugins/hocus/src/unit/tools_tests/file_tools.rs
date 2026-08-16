use super::*;

#[test]
fn a_relative_path_is_taken_from_the_project_root() {
    assert_eq!(
        resolve_in_root(&project(), Some("src/main.rs")).unwrap(),
        "/work/hocus/src/main.rs"
    );
    // The whole project when the model names no path at all.
    assert_eq!(resolve_in_root(&project(), None).unwrap(), "/work/hocus");
    assert_eq!(
        resolve_in_root(&project(), Some("  ")).unwrap(),
        "/work/hocus"
    );
}

/// An absolute path is what `find_files` and `search_files` hand back, so
/// feeding one straight to `read_file` has to work.
#[test]
fn an_absolute_path_inside_the_root_is_kept() {
    assert_eq!(
        resolve_in_root(&project(), Some("/work/hocus/src/main.rs")).unwrap(),
        "/work/hocus/src/main.rs"
    );
}

#[test]
fn a_path_that_climbs_out_of_the_project_is_refused() {
    assert_eq!(
        resolve_in_root(&project(), Some("../secrets")),
        Err("path must not contain a .. component".to_owned())
    );
    // Rejected even though it lands back inside: resolving it needs the
    // filesystem, and refusing is the answer that needs no filesystem.
    assert!(resolve_in_root(&project(), Some("src/../src/main.rs")).is_err());
}

#[test]
fn an_absolute_path_outside_the_project_is_refused() {
    assert_eq!(
        resolve_in_root(&project(), Some("/etc/passwd")),
        Err("path must be inside project hocus".to_owned())
    );
}

/// The trap a plain string prefix falls into: `/work/hocus-evil` starts with
/// `/work/hocus` as text but is a different folder.
#[test]
fn a_sibling_whose_name_extends_the_root_is_outside_it() {
    assert!(resolve_in_root(&project(), Some("/work/hocus-evil/x")).is_err());
}

#[test]
fn find_files_wraps_a_bare_name_and_leaves_a_glob_alone() {
    assert_eq!(
        file_argv_for("find_files", r#"{"pattern": "tool_loop"}"#).unwrap(),
        [
            "rg",
            "--files",
            "--glob",
            "*tool_loop*",
            "--",
            "/work/hocus"
        ]
    );
    assert_eq!(
        file_argv_for("find_files", r#"{"pattern": "*.rs", "path": "src"}"#).unwrap(),
        ["rg", "--files", "--glob", "*.rs", "--", "/work/hocus/src"]
    );
    // A pattern with a separator is a path shape the model meant literally.
    assert_eq!(glob_for("src/**/main.rs"), "src/**/main.rs");
}

#[test]
fn search_files_passes_the_query_after_a_flag_of_its_own() {
    assert_eq!(
        file_argv_for("search_files", r#"{"query": "fn main"}"#).unwrap(),
        [
            "rg",
            "--line-number",
            "--with-filename",
            "--color",
            "never",
            "-e",
            "fn main",
            "--",
            "/work/hocus"
        ]
    );
    // A query that looks like an option is still a pattern, because `-e`
    // consumes the next argument whatever it says.
    let argv = file_argv_for("search_files", r#"{"query": "--help"}"#).unwrap();
    assert_eq!(argv[argv.len() - 3..], ["--help", "--", "/work/hocus"]);
}

#[test]
fn search_files_adds_only_the_options_it_was_given() {
    let argv = file_argv_for(
        "search_files",
        r#"{"query": "x", "case_insensitive": true, "glob": "*.rs"}"#,
    )
    .unwrap();
    assert!(argv.contains(&"--ignore-case".to_owned()));
    assert_eq!(argv.iter().filter(|a| *a == "--glob").count(), 1);
    assert!(!file_argv_for("search_files", r#"{"query": "x"}"#)
        .unwrap()
        .contains(&"--ignore-case".to_owned()));
}

#[test]
fn read_file_asks_for_the_window_it_was_given() {
    assert_eq!(
        file_argv_for(
            "read_file",
            r#"{"path": "src/main.rs", "start_line": 10, "line_count": 5}"#
        )
        .unwrap(),
        ["sed", "-n", "10,14p", "--", "/work/hocus/src/main.rs"]
    );
}

/// An unbounded read would be cut mid-line by MAX_TOOL_OUTPUT after the host
/// had already read the whole file, so the default window is a real one.
#[test]
fn read_file_bounds_a_request_that_names_no_window() {
    assert_eq!(
        file_argv_for("read_file", r#"{"path": "x"}"#).unwrap()[2],
        format!("1,{FILE_TOOL_LINES}p")
    );
}

#[test]
fn file_tools_reject_a_missing_required_argument() {
    assert!(file_argv_for("find_files", "{}").is_err());
    assert!(file_argv_for("search_files", r#"{"query": "  "}"#).is_err());
    assert!(file_argv_for("read_file", "{}").is_err());
}

/// `read_file` returns what was asked for; a listing is capped so a search of
/// a large repository does not fill the transcript.
#[test]
fn only_listings_are_capped() {
    let args = Args::parse(r#"{"max_results": 5}"#).unwrap();
    assert_eq!(file_line_cap("read_file", &args), None);
    assert_eq!(file_line_cap("search_files", &args), Some(5));
    assert_eq!(
        file_line_cap("find_files", &Args::default()),
        Some(FILE_TOOL_LINES)
    );
    // Asking for everything still stops somewhere.
    let huge = Args::parse(r#"{"max_results": 99999}"#).unwrap();
    assert_eq!(
        file_line_cap("find_files", &huge),
        Some(MAX_FILE_TOOL_LINES)
    );
}
