//! The plugin tools as the tool loop actually runs them: which answer at once, which defer on
//! the host, and what happens when there is no plugin folder to write into. Off wasm the host's
//! file commands are stubs that say so, which is what the file tools answer here.

use super::*;
use zellij_tile::prelude::TabPosition;

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
        "plugin_outline",
        "plugin_search",
        "plugin_write",
        "plugin_reload",
        "plugin_log",
    ] {
        let mut state = ready_state();
        assert!(
            run_file_tool(
                &mut state,
                name,
                r#"{"name":"keys","source":"x","query":"x"}"#
            ),
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
        r#"{"section":"lua_events"}"#
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
        ("plugin_outline", r#"{"name":"keys"}"#),
        ("plugin_search", r#"{"query":"x"}"#),
        ("plugin_search", r#"{"name":"keys","query":"x"}"#),
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
    crate::tools::test_seams::PLUGIN_ENTRIES.with_borrow_mut(|entries| *entries = Some(vec![]));
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

/// The session's plugin list, with the plugin at `path` running as `id` - in a pane when
/// `pane` says so, headless otherwise.
fn session_running(path: &str, id: u32, pane: bool) -> SessionInfo {
    let mut panes = PaneManifest::default();
    if pane {
        panes.panes.insert(
            TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(id, true),
                ..PaneInfo::default()
            }],
        );
    }
    SessionInfo {
        is_current_session: true,
        panes,
        plugins: BTreeMap::from([(
            PluginId::new(id),
            PluginInfo {
                location: format!("file:{path}"),
                configuration: BTreeMap::new(),
            },
        )]),
        ..SessionInfo::default()
    }
}

/// A headless reload replaces the instance without a pane, and only that one: a pane showing
/// the same file is somebody's copy, and reloading it in place would leave no headless
/// instance running while the report said there was one.
#[test]
fn a_headless_reload_names_the_headless_instance_and_never_a_panes() {
    crate::tools::test_seams::reset();
    let path = "/home/someone/.config/zellij/plugins/harness.lua";

    let mut state = plugin_state();
    state.on_sessions(vec![session_running(path, 9, true)]);
    cleared_reload(&mut state, r#"{"name":"harness","headless":true}"#);
    assert_eq!(
        crate::tools::test_seams::HEADLESS_RELOADS.with_borrow(Clone::clone),
        vec![None],
        "a pane's copy is not the instance to reload"
    );

    crate::tools::test_seams::reset();
    let mut state = plugin_state();
    state.on_sessions(vec![session_running(path, 9, false)]);
    cleared_reload(&mut state, r#"{"name":"harness","headless":true}"#);
    assert_eq!(
        crate::tools::test_seams::HEADLESS_RELOADS.with_borrow(Clone::clone),
        vec![Some(PluginId::new(9))],
        "the headless instance is reloaded in place"
    );
}

/// `headless` loads without a pane and answers from the log rather than a render: off wasm
/// the log cannot be read, and that is what the answer says.
#[test]
fn a_headless_reload_answers_from_the_plugins_log() {
    let mut state = plugin_state();
    cleared_reload(&mut state, r#"{"name":"harness","headless":true}"#);
    let watch = state.plugin_load.clone().expect("a watch");
    assert!(watch.headless, "{watch:?}");
    assert_eq!(watch.name, "harness");
    assert!(matches!(state.status, Status::AwaitingPluginLoad));

    state.timer_armed = false;
    state.on_timer((PLUGIN_LOAD_MS as f64) / 1000.0);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("headless")
            && results[0].content.contains("log could not be read"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.status, Status::Idle));
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

/// One batch is one confirmation, and the permissions it grants are the difference between the
/// source before and the source after every edit - not those of any one edit.
#[test]
fn a_batch_edit_asks_once_and_grants_before_versus_final() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::set_plugin_source(
        "zellij.request_permission { \"read_files\" }\nreturn {}\n",
    );
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "edit".into(),
        name: "plugin_edit".into(),
        arguments: serde_json::json!({
            "name": "keys",
            "edits": [
                { "old_string": "\"read_files\"", "new_string": "\"read_files\", \"write_files\"" },
                { "old_string": "\"write_files\"", "new_string": "\"write_files\", \"run_commands\"" },
            ],
        })
        .to_string(),
    }]
    .into();

    state.pump_tools();
    let prompt = state.confirmation_prompt().expect("confirmation");
    let consequence = &prompt.detail[0].0;
    assert!(consequence.contains("in 2 places"), "{consequence}");
    assert!(
        consequence.contains("grants it: write_files, run_commands"),
        "{consequence}"
    );

    state.confirm_tool();

    assert_eq!(
        crate::tools::test_seams::granted_permissions(),
        vec![PermissionType::WriteFiles, PermissionType::RunCommands]
    );
    let results = tool_results(&state);
    assert_eq!(results.len(), 1, "one confirmation, one answer");
    let report = &results[0].content;
    assert!(
        report.starts_with(
            "edited keys.lua at line 1 (now 2 lines) and granted it write_files, run_commands;"
        ),
        "{report}"
    );
    crate::tools::test_seams::reset();
}

/// A miss anywhere in a batch is refused before anyone is asked to approve it - the diagnosis is
/// the whole answer - and nothing is written or granted.
#[test]
fn a_batch_whose_second_edit_misses_is_refused_before_the_prompt() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::set_plugin_source(
        "zellij.request_permission { \"read_files\" }\nreturn {}\n",
    );
    let mut state = plugin_state();
    state.pending = vec![ToolCall {
        id: "edit".into(),
        name: "plugin_edit".into(),
        arguments: serde_json::json!({
            "name": "keys",
            "edits": [
                { "old_string": "\"read_files\"", "new_string": "\"read_files\", \"write_files\"" },
                { "old_string": "nothing like this", "new_string": "x" },
            ],
        })
        .to_string(),
    }]
    .into();

    state.pump_tools();

    assert!(
        state.confirmation_prompt().is_none(),
        "a miss is refused before anyone is asked"
    );
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .content
            .starts_with("edit 2 of 2: old_string is not in the file"),
        "{}",
        results[0].content
    );
    assert!(crate::tools::test_seams::granted_permissions().is_empty());
    crate::tools::test_seams::reset();
}

/// The outline and a search confined to one plugin read that plugin through the host and answer
/// at once, in the shapes the pure tests pin.
#[test]
fn an_outline_and_a_named_search_are_answered_from_the_plugin_source() {
    crate::tools::test_seams::reset();
    crate::tools::test_seams::set_plugin_source(
        "local M = {}\nfunction M.render()\n  return \"hi\"\nend\nreturn M\n",
    );
    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_outline",
        r#"{"name":"keys"}"#
    ));
    assert_eq!(
        tool_results(&state)[0].content,
        "keys.lua: 5 lines\n2: function M.render()"
    );

    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_search",
        r#"{"name":"keys","query":"render"}"#
    ));
    assert_eq!(
        tool_results(&state)[0].content,
        "keys.lua:2:function M.render()"
    );

    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_search",
        r#"{"name":"keys","query":"zzz"}"#
    ));
    assert_eq!(tool_results(&state)[0].content, "no matches in keys.lua");
    crate::tools::test_seams::reset();
}

/// The docs are compiled in, so a query or a page of them needs no plugin folder either.
#[test]
fn a_docs_query_and_a_docs_page_need_no_plugin_folder() {
    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_docs",
        r#"{"query":"run_command"}"#
    ));
    let hits = &tool_results(&state)[0].content;
    assert!(hits.contains("events:"), "{hits}");

    let mut state = ready_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_docs",
        r#"{"section":"lua_events","start_line":1,"line_count":3}"#
    ));
    let page = &tool_results(&state)[0].content;
    assert!(page.starts_with("lines 1-3 of "), "{page}");
    assert!(
        page.contains("(plugin_docs section=lua_events start_line=4)"),
        "{page}"
    );
}

#[test]
fn folder_search_reports_caps_failed_reads_and_oversize_files() {
    use crate::tools::{test_seams, FOLDER_FILES_CAP, FOLDER_READ_BYTES};
    test_seams::reset();
    test_seams::PLUGIN_ENTRIES.with_borrow_mut(|entries| {
        *entries = Some(
            (0..FOLDER_FILES_CAP + 2)
                .map(|n| FsEntry {
                    path: format!("/plugins/p{n:03}.lua"),
                    metadata: None,
                })
                .collect(),
        );
    });
    test_seams::set_plugin_source("needle\n");
    test_seams::PLUGIN_READS.with_borrow_mut(|reads| {
        reads.insert("/plugins/p000.lua".into(), Err("permission denied".into()));
        reads.insert(
            "/plugins/p001.lua".into(),
            Ok("x".repeat(FOLDER_READ_BYTES + 1)),
        );
        // These files are beyond the cap: their hits must not appear.
        reads.insert("/plugins/p064.lua".into(), Ok("outside".into()));
        reads.insert("/plugins/p065.lua".into(), Ok("outside".into()));
    });
    let warning = "Warning: incomplete search; 4 files skipped (file cap or failed reads).\n\n";
    for query in ["needle", "outside"] {
        let mut state = plugin_state();
        assert!(run_file_tool(
            &mut state,
            "plugin_search",
            &serde_json::json!({"query": query, "max_results": 200}).to_string()
        ));
        let results = tool_results(&state);
        let answer = &results[0].content;
        assert!(answer.starts_with(warning), "{answer}");
        if query == "needle" {
            assert_eq!(answer.matches(":1:needle").count(), FOLDER_FILES_CAP - 2);
        } else {
            assert!(answer.contains("no matches"), "{answer}");
        }
    }
    for (name, error) in [("p000", "permission denied"), ("p001", "file exceeds")] {
        let mut state = plugin_state();
        assert!(run_file_tool(
            &mut state,
            "plugin_search",
            &serde_json::json!({"name": name, "query": "needle"}).to_string()
        ));
        assert!(tool_results(&state)[0].content.starts_with(error));
    }
    test_seams::set_plugin_source(&"needle".repeat(MAX_TOOL_OUTPUT));
    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_search",
        r#"{"query":"needle"}"#
    ));
    let results = tool_results(&state);
    assert!(results[0].content.starts_with(warning));
    assert!(
        results[0].content.contains("[truncated"),
        "{}",
        results[0].content
    );
    test_seams::reset();
}

#[test]
fn folder_search_skips_invalid_names_but_named_search_rejects_them() {
    use crate::tools::test_seams;
    test_seams::reset();
    let invalid = [
        "has spaces.lua".to_owned(),
        "café.lua".to_owned(),
        format!("{}.lua", "a".repeat(65)),
    ];
    test_seams::PLUGIN_ENTRIES.with_borrow_mut(|entries| {
        *entries = Some(
            invalid
                .iter()
                .map(String::as_str)
                .chain(["keys.lua"])
                .map(|name| FsEntry {
                    path: format!("/plugins/{name}"),
                    metadata: None,
                })
                .collect(),
        );
    });
    test_seams::set_plugin_source("needle\n");
    let mut state = plugin_state();
    assert!(run_file_tool(
        &mut state,
        "plugin_search",
        r#"{"query":"needle"}"#
    ));
    let results = tool_results(&state);
    assert!(results[0]
        .content
        .starts_with("Warning: incomplete search; 3 files skipped"));
    assert!(results[0].content.contains("keys.lua:1:needle"));
    for name in invalid {
        let mut state = plugin_state();
        assert!(run_file_tool(
            &mut state,
            "plugin_search",
            &serde_json::json!({"name": name, "query": "needle"}).to_string()
        ));
        let expected = crate::tools::plugin_fs_path(Some(&name)).unwrap_err();
        assert_eq!(tool_results(&state)[0].content, expected);
    }
    test_seams::reset();
}

#[test]
fn edit_diagnostics_are_byte_bounded_before_confirmation_and_during_execution() {
    use crate::tools::test_seams;
    for unit in ["x", "界"] {
        for batch in [false, true] {
            for preflight in [false, true] {
                test_seams::reset();
                let source = format!("before\nanchor  {}\n", unit.repeat(MAX_TOOL_OUTPUT));
                test_seams::set_plugin_source(&source);
                let failed_edit =
                    serde_json::json!({"old_string": "anchor   ", "new_string": "replacement"});
                let arguments = if batch {
                    serde_json::json!({"name": "keys", "edits": [
                        {"old_string": "before", "new_string": "after"},
                        failed_edit
                    ]})
                } else {
                    serde_json::json!({"name": "keys", "old_string": "anchor   ", "new_string": "replacement"})
                };
                let mut state = plugin_state();
                let call = ToolCall {
                    id: "edit".into(),
                    name: "plugin_edit".into(),
                    arguments: arguments.to_string(),
                };
                state.pending = vec![call.clone()].into();
                if preflight {
                    state.pump_tools();
                } else {
                    assert!(state.start_plugin_tool(&call));
                }
                assert!(state.confirmation_prompt().is_none());
                let results = tool_results(&state);
                assert_eq!(results.len(), 1);
                let error = &results[0].content;
                assert!(error.len() <= MAX_TOOL_OUTPUT);
                assert!(error.contains("old_string is not in the file exactly"));
                assert!(error.ends_with("\n[truncated]"));
                if batch {
                    assert!(error.starts_with("edit 2 of 2:"));
                }
                assert_eq!(test_seams::PLUGIN_WRITE_CALLS.get(), 0);
                assert!(test_seams::granted_permissions().is_empty());
            }
        }
    }
    test_seams::reset();
}

#[test]
fn extension_reload_refuses_duplicate_namespaces_across_languages() {
    use crate::tools::test_seams;
    test_seams::reset();
    crate::extensions::test_seams::EXTENSION_LOAD_CALLS.set(0);
    test_seams::PLUGIN_FS_EXISTS.set(Some(true));
    test_seams::PLUGIN_ENTRIES.with_borrow_mut(|entries| {
        *entries = Some(
            ["lua", "ts"]
                .map(|suffix| zellij_tile::prelude::FsEntry {
                    path: format!("/plugins/hocus/keys.{suffix}"),
                    metadata: None,
                })
                .to_vec(),
        )
    });
    let mut state = plugin_state();
    state.pending.push_back(ToolCall {
        id: "duplicate".into(),
        name: "plugin_reload".into(),
        arguments: r#"{"name":"hocus/keys.ts"}"#.into(),
    });
    state.pump_tools();
    assert_eq!(crate::extensions::test_seams::EXTENSION_LOAD_CALLS.get(), 0);
    let results = tool_results(&state)
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        results.contains("duplicate extension namespace"),
        "{results}"
    );
    assert!(results.contains("keys.lua") && results.contains("keys.ts"));
    assert!(state.extension_files.is_empty());
    test_seams::reset();
}
