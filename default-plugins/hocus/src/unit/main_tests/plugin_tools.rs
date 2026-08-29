//! The plugin tools as the tool loop actually runs them: which answer at once, which defer on
//! the host, and what happens when there is no plugin folder to write into. Off wasm the host's
//! file commands are stubs that say so, which is what the file tools answer here.

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

/// The four that touch the plugin folder go through the host's file commands and answer at
/// once - off wasm, with the stub's own sentence. Only `plugin_log` still shells out and waits.
#[test]
fn the_file_tools_answer_at_once_and_only_the_log_defers() {
    for (name, arguments) in [
        ("plugin_list", "{}"),
        ("plugin_read", r#"{"name":"keys"}"#),
        ("plugin_write", r#"{"name":"keys","source":"local M = {}"}"#),
        ("plugin_reload", r#"{"name":"keys"}"#),
    ] {
        let mut state = plugin_state();
        assert!(
            run_file_tool(&mut state, name, arguments),
            "{name} must answer rather than defer"
        );
        assert!(!matches!(state.status, Status::AwaitingCommand));
        let results = tool_results(&state);
        assert_eq!(results.len(), 1, "{name}");
        assert_eq!(
            results[0].content,
            tools::PLUGIN_FOLDER_UNREACHABLE,
            "{name}"
        );
    }

    let mut state = plugin_state();
    assert!(
        !run_file_tool(&mut state, "plugin_log", r#"{"name":"keys"}"#),
        "plugin_log must defer rather than answer"
    );
    assert!(matches!(state.status, Status::AwaitingCommand));
    assert!(tool_results(&state).is_empty());
}

/// A reload whose file the host said is there: the load is asked for and the head stays
/// pending, as `start_plugin_tool` leaves it.
fn cleared_reload(state: &mut State, arguments: &str) {
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "plugin_reload".into(),
        arguments: arguments.into(),
    }]
    .into();
    assert!(
        state.start_plugin_load(),
        "a reload changes state rather than answering"
    );
}

/// The plugin is started once the host says the file is there, and not before. A reload is
/// fire-and-forget on the host, so that is where the plugin is *asked* to load - not where the
/// tool answers. The answer waits for the pane.
#[test]
fn a_cleared_reload_waits_to_look_at_the_pane_before_answering() {
    let mut state = plugin_state();
    cleared_reload(&mut state, r#"{"name":"keys"}"#);
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
    cleared_reload(&mut state, r#"{"name":"keys"}"#);
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
    cleared_reload(&mut state, r#"{"name":"keys"}"#);
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
