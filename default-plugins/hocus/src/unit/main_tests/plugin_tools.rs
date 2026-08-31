//! The plugin tools as the tool loop actually runs them: which answer at once, which defer on
//! the host, and what happens when there is no plugin folder to write into. Off wasm the host's
//! file commands are stubs that say so, which is what the file tools answer here.

use super::*;

fn plugin_state() -> State {
    let mut state = ready_state();
    state.user_plugin_dir = "/home/someone/.config/zellij/plugins".to_owned();
    state
}

/// Disclosure, pre-granting and the report all use the same complete-source difference.
#[test]
fn a_plugin_edit_discloses_and_grants_only_the_permission_it_adds() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::set_plugin_source(r#"zellij.request_permission { "read_files" }"#);
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "edit".into(),
        name: "plugin_edit".into(),
        arguments: r#"{"name":"keys","old_string":"\"read_files\"","new_string":"\"read_files\", \"write_files\""}"#.into(),
    }]
    .into();

    state.pump_tools();
    let prompt = state.confirmation_prompt().expect("confirmation");
    let consequence = &prompt.detail[0].0;
    assert!(
        consequence.contains("grants it: write_files"),
        "{consequence}"
    );
    assert!(
        !consequence.contains("grants it: read_files"),
        "{consequence}"
    );

    state.confirm_tool();

    assert_eq!(
        crate::tools::test_seams::granted_permissions(),
        vec![PermissionType::WriteFiles]
    );
    let report = &tool_results(&state)[0].content;
    assert!(report.contains("granted it write_files"), "{report}");
    assert!(!report.contains("granted it read_files"), "{report}");
    crate::tools::test_seams::reset();
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

/// The four that touch the plugin folder go through the host's file commands, and the log
/// through the host's read of its own log; all answer at once - off wasm, with the stub's own
/// sentence.
#[test]
fn the_file_tools_and_the_log_answer_at_once() {
    for (name, arguments) in [
        ("plugin_list", "{}"),
        ("plugin_read", r#"{"name":"keys"}"#),
        ("plugin_write", r#"{"name":"keys","source":"local M = {}"}"#),
        ("plugin_reload", r#"{"name":"keys"}"#),
        ("plugin_log", r#"{"name":"keys"}"#),
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
        !state.start_plugin_load(),
        "a reload defers: the pump must stop here"
    );
}

/// The bug this guards against: `start_plugin_load` once returned `true` with the head still
/// pending, and `pump_tools` - a loop over the head - ran it again, and again, asking the host
/// for the same reload thousands of times a second without ever returning from `update()`.
/// Every plugin in the session froze behind it. So the pump itself is driven here, not
/// `run_head`, and the host stub counts (and past a handful, panics).
#[test]
fn a_cleared_reload_asks_the_host_once_and_stops_the_pump() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::PLUGIN_FS_EXISTS.set(Some(true));
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "plugin_reload".into(),
        arguments: r#"{"name":"keys"}"#.into(),
    }]
    .into();
    state.pump_tools();
    assert_eq!(
        crate::tools::test_seams::RELOAD_CALLS.get(),
        1,
        "one reload per call"
    );
    assert!(matches!(state.status, Status::AwaitingPluginLoad));
    assert_eq!(state.pending.len(), 1, "the head must stay pending");
    assert!(tool_results(&state).is_empty(), "answered before looking");
    assert_eq!(
        state
            .plugin_load
            .as_ref()
            .map(|watch| watch.target.as_str()),
        Some("/home/someone/.config/zellij/plugins/keys.lua")
    );
    crate::tools::test_seams::reset();
}

/// The same through `y` on a confirmation, which runs the head and then pumps: the pump must
/// find the head deferred and stop, not run it a second time.
#[test]
fn a_confirmed_reload_does_not_rerun_the_head() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::PLUGIN_FS_EXISTS.set(Some(true));
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "plugin_reload".into(),
        arguments: r#"{"name":"keys"}"#.into(),
    }]
    .into();
    state.status = Status::ConfirmingTool;
    assert!(state.confirm_tool());
    assert_eq!(
        crate::tools::test_seams::RELOAD_CALLS.get(),
        1,
        "one reload per call"
    );
    assert!(matches!(state.status, Status::AwaitingPluginLoad));
    assert_eq!(state.pending.len(), 1, "the head must stay pending");
    assert!(tool_results(&state).is_empty());
    crate::tools::test_seams::reset();
}

/// A hocus extension takes the other branch - a headless load or reload - and defers the same
/// way, so the pump must stop for it too.
#[test]
fn a_cleared_extension_reload_asks_the_host_once_and_stops_the_pump() {
    crate::tools::test_seams::reset();
    crate::extensions::test_seams::EXTENSION_LOAD_CALLS.set(0);
    crate::tools::test_seams::PLUGIN_FS_EXISTS.set(Some(true));
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "plugin_reload".into(),
        arguments: r#"{"name":"hocus/keys"}"#.into(),
    }]
    .into();
    state.pump_tools();
    assert_eq!(
        crate::extensions::test_seams::EXTENSION_LOAD_CALLS.get(),
        1,
        "one load per call"
    );
    assert_eq!(
        crate::tools::test_seams::RELOAD_CALLS.get(),
        0,
        "not a pane plugin"
    );
    assert!(matches!(state.status, Status::AwaitingPluginLoad));
    assert_eq!(state.pending.len(), 1, "the head must stay pending");
    assert!(state
        .plugin_load
        .as_ref()
        .is_some_and(|watch| watch.extension));
    crate::tools::test_seams::reset();
    crate::extensions::test_seams::EXTENSION_LOAD_CALLS.set(0);
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
