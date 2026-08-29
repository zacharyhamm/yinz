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
        "plugin_log",
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

/// `plugin_check` is answered from the host's compiler and nothing else, so it needs no folder
/// and never defers. Off wasm the stand-in compiles everything; the answer's shape is the test.
#[test]
fn a_compile_check_answers_at_once_and_needs_no_folder() {
    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_check",
        r#"{"name":"keys","source":"return {}"}"#
    ));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "keys.lua compiles");
    assert!(!matches!(state.status, Status::AwaitingCommand));

    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_check",
        r#"{"name":"keys"}"#
    ));
    assert!(tool_results(&state)[0]
        .content
        .contains("source is required"));
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
        r#"{"name":"deep/sub/keys","source":"x"}"#,
    ] {
        let mut state = plugin_state();
        assert!(run_file_tool(&mut state, "plugin_write", arguments));
        let results = tool_results(&state);
        assert!(
            results[0].content.contains("not a plugin name")
                || results[0].content.contains("file name"),
            "got {:?}",
            results[0].content
        );
        assert!(!matches!(state.status, Status::AwaitingCommand));
    }
}

/// The four that touch the filesystem hand off to the host and wait, rather than answering
/// with something they made up. `plugin_reload` is one of them because the file it is about to
/// run has to be checked for being a symlink out of the folder, which only the host can do.
#[test]
fn the_filesystem_tools_defer_on_the_host() {
    for (name, arguments) in [
        ("plugin_list", "{}"),
        ("plugin_read", r#"{"name":"keys"}"#),
        ("plugin_write", r#"{"name":"keys","source":"local M = {}"}"#),
        ("plugin_reload", r#"{"name":"keys"}"#),
        ("plugin_log", r#"{"name":"keys"}"#),
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

/// The plugin is started once the host says the path may be loaded, and not before. A reload is fire-and-forget on the host, so a clean exit from the guard is where the
/// plugin is *asked* to load - not where the tool answers. The answer waits for the pane.
#[test]
fn a_cleared_reload_waits_to_look_at_the_pane_before_answering() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_reload",
        r#"{"name":"keys"}"#,
        0,
        b"",
        b"",
    );
    assert!(tool_results(&state).is_empty(), "answered before looking");
    assert!(matches!(state.status, Status::AwaitingPluginLoad));
    assert_eq!(state.pending.len(), 1, "the head must stay pending");
    assert_eq!(
        state
            .plugin_load
            .as_ref()
            .map(|watch| watch.target.as_str()),
        Some("/home/someone/.config/zellij/plugins/keys.lua")
    );
    assert!(state.timer_armed, "nothing would ever go and look");

    // Too soon: the pane has not had time to show anything.
    state.timer_armed = false;
    state.on_timer(0.1);
    assert!(tool_results(&state).is_empty());
    assert!(matches!(state.status, Status::AwaitingPluginLoad));

    // Once the wait is up the look happens. Off wasm there is no host to ask, so what is found
    // is "no pane" - and that is reported as such rather than as success.
    state.timer_armed = false;
    state.on_timer((PLUGIN_LOAD_MS as f64) / 1000.0);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("no pane is running it yet"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.status, Status::Idle));
    assert!(state.plugin_load.is_none());
}

/// The watch belongs to the chat that asked for the reload, not to whatever is on screen:
/// `/history` mid-wait must not strand the reload, and a second chat's reload must not take the
/// first one's watch away.
#[test]
fn a_reload_awaited_by_a_parked_chat_is_still_answered() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_reload",
        r#"{"name":"keys"}"#,
        0,
        b"",
        b"",
    );
    assert!(matches!(state.status, Status::AwaitingPluginLoad));

    // `/history` moving to another conversation, which leaves an idle chat in front.
    let waiting = state.take_chat();
    state.parked.push(waiting);
    assert!(matches!(state.status, Status::Idle));
    assert!(
        state.plugin_load.is_none(),
        "the watch went with the chat that asked"
    );

    state.timer_armed = false;
    state.on_timer((PLUGIN_LOAD_MS as f64) / 1000.0);

    assert!(tool_results(&state).is_empty(), "not this chat's reload");
    let parked = state.parked.first().expect("still owed a reply");
    let results: Vec<&Message> = parked
        .messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("no pane is running it yet"),
        "{:?}",
        results[0].content
    );
    assert!(parked.plugin_load.is_none());
    assert!(!matches!(parked.status, Status::AwaitingPluginLoad));
}

/// Esc while the pane is being waited on answers the head like any other abort, so the model
/// is not left with a call it never heard back from.
#[test]
fn an_awaited_plugin_load_can_be_aborted() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_reload",
        r#"{"name":"keys"}"#,
        0,
        b"",
        b"",
    );
    assert!(matches!(state.status, Status::AwaitingPluginLoad));
    state.abort_tools();
    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty());
    // A later tick must not answer a head that is no longer there.
    state.plugin_load = None;
    state.timer_armed = false;
    state.on_timer(1.0);
    assert_eq!(tool_results(&state).len(), 1);
}

/// An empty grep is the ordinary first answer for a plugin that has not logged yet, and the
/// model should be told how to make it say something rather than shown nothing.
#[test]
fn an_empty_log_says_how_a_plugin_writes_to_it() {
    let mut state = plugin_state();
    command_result(&mut state, "plugin_log", r#"{"name":"keys"}"#, 0, b"", b"");
    let results = tool_results(&state);
    assert!(
        results[0].content.contains("zellij.log"),
        "{:?}",
        results[0].content
    );
}

#[test]
fn log_lines_come_back_as_they_are() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_log",
        r#"{"name":"keys"}"#,
        0,
        b"INFO [lua:keys.lua] pressed t\n",
        b"",
    );
    let results = tool_results(&state);
    assert_eq!(results[0].content, "INFO [lua:keys.lua] pressed t");
}

/// The point of making a reload wait for the host: a symlink in the plugin folder would
/// otherwise be *run*, which is further than reading one.
#[test]
fn a_reload_of_a_symlink_is_refused_with_what_the_guard_said() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_reload",
        r#"{"name":"keys"}"#,
        1,
        b"",
        b"/home/someone/.config/zellij/plugins/keys.lua is a symlink; refusing",
    );
    let results = tool_results(&state);
    assert!(
        results[0].content.contains("symlink"),
        "got {:?}",
        results[0].content
    );
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
    assert!(
        !results[0].content.contains("granted"),
        "{:?}",
        results[0].content
    );
}

/// A write whose source asks for permissions says they were granted, so the model does not
/// go looking for a prompt that will never come.
#[test]
fn a_successful_write_says_what_it_granted() {
    let mut state = plugin_state();
    command_result(
        &mut state,
        "plugin_write",
        r#"{"name":"probe","source":"zellij.request_permission { \"read_files\" }"}"#,
        0,
        b"",
        b"",
    );
    let results = tool_results(&state);
    assert!(
        results[0].content.contains("granted it read_files"),
        "got {:?}",
        results[0].content
    );
}
