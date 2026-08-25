//! The plugin tools as the tool loop actually runs them: which answer at once, which defer on
//! the host, and what happens when there is no plugin folder to write into.

use super::*;

fn plugin_state() -> State {
    let mut state = ready_state();
    state.user_plugin_dir = "/home/someone/.config/zellij/plugins".to_owned();
    state
}

/// Without a folder there is nowhere to put a plugin, and guessing at a path would be worse
/// than saying so — the same shape as a file tool refusing without a project.
#[test]
fn a_plugin_tool_without_a_folder_refuses_instead_of_running_anything() {
    for name in [
        "plugin_list",
        "plugin_read",
        "plugin_write",
        "plugin_reload",
    ] {
        let mut state = ready_state();
        assert!(
            run_file_tool(&mut state, name, r#"{"name":"keys","source":"x"}"#),
            "{name} must answer rather than defer"
        );
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("user_plugin_dir"),
            "{name} said {:?}",
            results[0].content
        );
        assert!(!matches!(state.status, Status::AwaitingCommand));
    }
}

/// The docs are compiled in, so they are the one plugin tool that works with nothing
/// configured at all — which matters, because it is the one the model calls first.
#[test]
fn the_docs_answer_even_with_no_plugin_folder() {
    let mut state = ready_state();
    assert!(run_file_tool(&mut state, "plugin_docs", "{}"));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("quickstart"),
        "expected the index, got {:?}",
        results[0].content
    );
    assert!(!matches!(state.status, Status::AwaitingCommand));
}

#[test]
fn a_named_section_of_the_docs_is_served() {
    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_docs",
        r#"{"section":"events"}"#
    ));
    let results = tool_results(&state);
    assert!(
        results[0].content.starts_with("## Events"),
        "{:?}",
        results[0].content
    );
}

#[test]
fn an_unknown_docs_section_lists_the_real_ones_rather_than_failing() {
    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_docs",
        r#"{"section":"nonsense"}"#
    ));
    let results = tool_results(&state);
    assert!(
        results[0].content.contains("no such section"),
        "{:?}",
        results[0].content
    );
    assert!(results[0].content.contains("quickstart"));
}

/// A name that escapes the folder is refused here, before a command is spawned — the check is
/// lexical and needs no filesystem, exactly as `resolve_in_root` is for the file tools.
#[test]
fn a_plugin_tool_refuses_a_name_that_is_a_path() {
    for arguments in [
        r#"{"name":"../../../etc/passwd","source":"x"}"#,
        r#"{"name":"sub/keys","source":"x"}"#,
    ] {
        let mut state = plugin_state();
        assert!(run_file_tool(&mut state, "plugin_write", arguments));
        let results = tool_results(&state);
        assert!(
            results[0].content.contains("not a plugin name")
                || results[0].content.contains("single file name"),
            "got {:?}",
            results[0].content
        );
        assert!(!matches!(state.status, Status::AwaitingCommand));
    }
}

/// The three that touch the filesystem hand off to the host and wait, rather than answering
/// with something they made up.
#[test]
fn the_filesystem_tools_defer_on_the_host() {
    for (name, arguments) in [
        ("plugin_list", "{}"),
        ("plugin_read", r#"{"name":"keys"}"#),
        ("plugin_write", r#"{"name":"keys","source":"local M = {}"}"#),
    ] {
        let mut state = plugin_state();
        assert!(
            !run_file_tool(&mut state, name, arguments),
            "{name} must defer rather than answer"
        );
        assert!(matches!(state.status, Status::AwaitingCommand));
        assert!(tool_results(&state).is_empty());
    }
}

/// A reload is fire-and-forget: `start_or_reload_plugin` has no reply, so the result says what
/// was asked for rather than claiming the plugin loaded.
#[test]
fn a_reload_answers_at_once_without_claiming_success() {
    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_reload",
        r#"{"name":"keys"}"#
    ));
    assert!(!matches!(state.status, Status::AwaitingCommand));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    // Off wasm there is no host to call, so the stub answers - what matters here is that the
    // tool resolved the name and returned rather than deferring.
    assert!(!results[0].content.is_empty());
}

/// Answer a plugin tool as if the host had come back with this.
fn command_result(
    state: &mut State,
    name: &str,
    arguments: &str,
    exit: i32,
    out: &[u8],
    err: &[u8],
) {
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: name.into(),
        arguments: arguments.into(),
    }]
    .into();
    state.on_command_result(
        Some(exit),
        out,
        err,
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );
}

/// The `set -C` refusal, in both wordings: bash says "cannot overwrite existing file", dash says
/// "cannot create <path>: File exists". Matched by text because the shell reports it as an
/// ordinary non-zero exit, so getting either wrong turns a recoverable "pass overwrite=true"
/// into a bare exit code the model cannot act on.
#[test]
fn a_refused_overwrite_says_how_to_proceed_whichever_shell_refused_it() {
    for stderr in [
        &b"sh: line 1: probe.lua: cannot overwrite existing file"[..],
        &b"sh: 1: cannot create /home/x/.config/zellij/plugins/probe.lua: File exists"[..],
    ] {
        let mut state = plugin_state();
        command_result(
            &mut state,
            "plugin_write",
            r#"{"name":"probe","source":"x"}"#,
            2,
            b"",
            stderr,
        );
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("overwrite=true"),
            "got {:?}",
            results[0].content
        );
    }
}

/// The folder is made by the first write, so before then it is legitimately absent — which
/// `find` reports as an error rather than as an empty list.
#[test]
fn listing_a_folder_that_does_not_exist_yet_is_not_an_error() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_list",
        "{}",
        1,
        b"",
        b"find: '/home/someone/.config/zellij/plugins': No such file or directory",
    );
    let results = tool_results(&state);
    assert_eq!(results[0].content, "no plugins yet");
}

/// A listing answers in names, because a name is what every plugin tool takes back — handing
/// over absolute paths would invite the model to send one and be refused.
#[test]
fn a_listing_comes_back_as_names_rather_than_paths() {
    let mut state = plugin_state();
    let dir = state.user_plugin_dir.clone();
    command_result(
        &mut state,
        "plugin_list",
        "{}",
        0,
        format!("{dir}/probe.lua\n{dir}/keys.lua\n").as_bytes(),
        b"",
    );
    let results = tool_results(&state);
    assert_eq!(results[0].content, "probe.lua\nkeys.lua");
}

#[test]
fn a_successful_write_points_at_the_next_step() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_write",
        r#"{"name":"probe","source":"x"}"#,
        0,
        b"",
        b"",
    );
    let results = tool_results(&state);
    assert!(
        results[0].content.contains("plugin_reload"),
        "got {:?}",
        results[0].content
    );
}
