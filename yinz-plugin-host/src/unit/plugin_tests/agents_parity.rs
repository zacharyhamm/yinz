use super::{
    data_parity::DataHost,
    runtime_parity::{default_reply, Pair, Responder},
};
use agent_proto::{agents, store, wire};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use yinz_plugin_api::plugin_command::*;
use yinz_types::{data::*, internal_ids::PluginId};

fn sessions() -> SessionListSnapshot {
    let mut session = SessionInfo {
        is_current_session: true,
        ..Default::default()
    };
    session.plugins.insert(
        PluginId::new(2),
        PluginInfo {
            location: "yinz:agent-service".into(),
            ..Default::default()
        },
    );
    SessionListSnapshot {
        live_sessions: vec![session],
        ..Default::default()
    }
}
fn run(id: &str, pane: u32, ended: bool) -> agents::AgentRun {
    serde_json::from_value(json!({"id":id,"pane_id":pane,"kind":"claude","session_id":id,"cwd":"/work/topic",
      "started_at_ms":1_000,"ended_at_ms":if ended {json!(60_000)}else{Value::Null},
      "entries":[{"revision":1,"kind":"prompt","text":"Investigate **history** with 日本語 and wrapping text.","created_at_ms":2_000},
       {"revision":2,"kind":"message","text":"# Plan\n\nFirst **step**\n\n- one\n- two\n\nA long paragraph with enough words to wrap across the panel.","created_at_ms":3_000},
       {"revision":3,"kind":"prompt","text":"<task-notification><task-id>task-7</task-id><status>completed</status><output-file>result.txt</output-file><summary>Finished</summary><result>first line\nsecond **line**\nthird line</result></task-notification>","created_at_ms":4_000}]
    })).unwrap()
}
fn current_run(run: &agents::AgentRun) -> agents::AgentRun {
    // The recordings left revisions at zero; Steel needs matching stored and advertised revisions.
    let mut run = run.clone();
    run.revision = run
        .entries
        .iter()
        .map(|entry| entry.revision)
        .max()
        .unwrap_or(run.revision);
    run
}
fn host(runs: &[agents::AgentRun], clock: Arc<AtomicU64>) -> (Arc<DataHost>, Responder) {
    let data = DataHost::with_clock("agents-parity", clock);
    let responder = data.responder(Arc::new(|command| {
        Some(match command {
            PluginCommand::GetAgentProviders => ProtobufAgentProvidersResponse {
                json: "{\"Ok\":[]}".into(),
            }
            .encode_to_vec(),
            PluginCommand::PaneTabs(..) => ProtobufPaneTabsResponse {
                json: "{\"Ok\":null}".into(),
            }
            .encode_to_vec(),
            PluginCommand::GetSessionList => {
                ProtobufGetSessionListResponse::from(GetSessionListResponse::Ok(sessions()))
                    .encode_to_vec()
            },
            PluginCommand::OpenFileFloating(..) => {
                ProtobufOpenFileFloatingResponse::default().encode_to_vec()
            },
            PluginCommand::OpenCommandPaneNearPlugin(..) => {
                ProtobufOpenCommandPaneNearPluginResponse::default().encode_to_vec()
            },
            _ => return default_reply(command),
        })
    }));
    for def in store::shared_defs() {
        responder(&PluginCommand::DataDefineEntity {
            namespace: Some(store::SHARED.into()),
            def,
            allow_destructive: false,
        })
        .unwrap();
    }
    for run in runs {
        data.apply(store::SHARED, &agents::rows::run_mutations(run));
    }
    (data, responder)
}
fn pair(
    runs: &[agents::AgentRun],
    view: bool,
) -> (Pair, Arc<AtomicU64>, Arc<DataHost>, Arc<DataHost>) {
    let clock = Arc::new(AtomicU64::new(90_000));
    let (wasm, wh) = host(runs, clock.clone());
    let (steel, sh) = host(
        &runs.iter().map(current_run).collect::<Vec<_>>(),
        clock.clone(),
    );
    let config = if view {
        BTreeMap::from([("view".into(), "agents".into())])
    } else {
        BTreeMap::new()
    };
    (
        Pair::with_hosts("agents", config, wh, sh, Some(clock.clone())),
        clock,
        wasm,
        steel,
    )
}
fn snapshot(pair: &mut Pair, seq: u64, runs: &[agents::AgentRun], source: u32) {
    pair.wasm.pipe(&snapshot_message(seq, runs, source));
    let current: Vec<_> = runs.iter().map(current_run).collect();
    let message = opening_reply(&mut pair.steel, snapshot_message(seq, &current, source));
    pair.steel.pipe(&message);
    pair.commands();
}
fn snapshot_message(seq: u64, runs: &[agents::AgentRun], source: u32) -> PipeMessage {
    let snapshot: wire::PollReply=serde_json::from_value(json!({"panes":[
      {"pane_id":7,"title":"claude","command":"claude","cwd":"/work/topic","status":"Running","tab":[1,"work"]},
      {"pane_id":8,"title":"codex","command":"codex","cwd":"/work/other","status":"Running","tab":[2,"other"]}],
      "activity":{},"manual_kinds":{"7":"claude","8":"codex"},"focused":7,
      "hooks":{"8":{"phase":"Waiting","waiting_on":"approve this action"}},
      "runs":runs.iter().map(agents::AgentRun::header).collect::<Vec<_>>() })).unwrap();
    PipeMessage {
        source: PipeSource::Plugin(PluginId::new(source)),
        name: "agent_snapshot".into(),
        payload: Some(serde_json::to_string(&snapshot).unwrap()),
        args: BTreeMap::from([("seq".into(), seq.to_string())]),
        is_private: false,
        pane_id: None,
    }
}
fn viewports(pair: &mut Pair) {
    pair.viewports();
    for size in [(0, 0), (1, 1), (3, 8), (5, 21), (8, 22), (20, 40), (40, 80)] {
        pair.render(size.0, size.1);
    }
}
fn press(pair: &mut Pair, key: &str) {
    eprintln!("agents key: {key}");
    pair.key(key);
}
#[test]
fn agents_actual_runtimes_match_startup_empty_key_sheet_and_focus_ring() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_startup_empty_key_sheet_and_focus_ring",
    );
    for view in [false, true] {
        let (mut pair, _, _, _) = pair(&[], view);
        viewports(&mut pair);
        for key in [
            "?",
            "j",
            "PageDown",
            "PageUp",
            "Esc",
            "Tab",
            "Shift Tab",
            "Esc",
            "/",
            "?",
            "Ctrl u",
            "Esc",
            "Ctrl r",
        ] {
            press(&mut pair, key);
            viewports(&mut pair);
        }
    }
}
#[test]
fn agents_actual_runtimes_match_snapshot_trust_waiting_and_navigation() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_snapshot_trust_waiting_and_navigation",
    );
    let (mut pair, _, _, _) = pair(&[], true);
    snapshot(&mut pair, 1, &[], 99);
    snapshot(&mut pair, 1, &[], 2);
    snapshot(&mut pair, 1, &[], 2);
    viewports(&mut pair);
    for key in [
        "Down",
        "Up",
        "w",
        "?",
        "Esc",
        "w",
        "p",
        "c",
        "s",
        "D",
        "F",
        "Tab",
        "Shift Tab",
        "Enter",
    ] {
        press(&mut pair, key);
        viewports(&mut pair);
    }
}
#[test]
fn agents_actual_runtimes_match_archive_history_reader_folds_and_clipboard() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_archive_history_reader_folds_and_clipboard",
    );
    let runs = vec![
        run("claude:active", 7, false),
        run("claude:archived", 9, true),
    ];
    let (mut pair, clock, _, _) = pair(&runs, true);
    snapshot(&mut pair, 1, &runs, 2);
    viewports(&mut pair);
    for key in [
        "Tab",
        "Up",
        "Down",
        "Enter",
        "y",
        "j",
        "Enter",
        "v",
        "y",
        "v",
        "k",
        "Enter",
        "PageDown",
        "PageUp",
        "Esc",
        "Shift Tab",
        "Down",
        "Down",
        "r",
        "n",
        "r",
        "y",
        "s",
        "?",
    ] {
        press(&mut pair, key);
        viewports(&mut pair);
    }
    clock.store(200_000, Ordering::SeqCst);
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(2.0)));
    viewports(&mut pair);
}
#[test]
fn agents_actual_runtimes_match_search_drafts_results_history_and_clear() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_search_drafts_results_history_and_clear",
    );
    let runs = vec![
        run("claude:active", 7, false),
        run("claude:archived", 9, true),
    ];
    let (mut pair, _, _, _) = pair(&runs, true);
    snapshot(&mut pair, 1, &runs, 2);
    pair.render(24, 90);
    pair.key("/");
    pair.event(Event::PastedText("history\n".into()));
    viewports(&mut pair);
    for key in [
        "Tab",
        "/",
        "Enter",
        "Enter",
        "Enter",
        "Esc",
        "Shift Tab",
        "/",
        "Ctrl u",
        "z",
        "Enter",
        "Esc",
        "/",
    ] {
        press(&mut pair, key);
        viewports(&mut pair);
    }
}

#[test]
fn agents_actual_runtimes_reject_invalid_sequences_and_reset_trusted_service() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_reject_invalid_sequences_and_reset_trusted_service",
    );
    let (mut pair, _, _, _) = pair(&[], true);
    for seq in [
        "",
        "1.0",
        "1e2",
        "#x10",
        "-1",
        " 1",
        "18446744073709551616",
        "+",
        "１２",
    ] {
        let mut message = snapshot_message(1, &[], 2);
        message.args.insert("seq".into(), seq.into());
        pair.pipe(message);
        pair.render(16, 70);
    }
    let mut valid = snapshot_message(1, &[], 2);
    valid.args.insert("seq".into(), "+1".into());
    pair.pipe(valid);
    for payload in [None, Some("{".into()), Some("null".into())] {
        let mut message = snapshot_message(2, &[], 2);
        message.payload = payload;
        pair.pipe(message);
    }
    for source in [99, 2] {
        let mut hello = snapshot_message(1, &[], source);
        hello.name = "agent_hello".into();
        pair.pipe(hello);
        snapshot(&mut pair, 1, &[], 2);
    }
    let mut ambiguous = sessions().live_sessions;
    ambiguous[0].plugins.insert(
        PluginId::new(3),
        PluginInfo {
            location: "yinz:agent-service".into(),
            ..Default::default()
        },
    );
    pair.event(Event::SessionUpdate(ambiguous, vec![]));
    snapshot(&mut pair, 2, &[], 2);
    pair.event(Event::SessionUpdate(sessions().live_sessions, vec![]));
    snapshot(&mut pair, 1, &[], 2);
    viewports(&mut pair);
}

#[test]
fn agents_actual_runtimes_match_mouse_release_drag_history_and_reader() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_mouse_release_drag_history_and_reader",
    );
    let runs = vec![
        run("claude:active", 7, false),
        run("claude:archived", 9, true),
    ];
    let (mut pair, _, _, _) = pair(&runs, true);
    snapshot(&mut pair, 1, &runs, 2);
    pair.render(26, 90);
    for mouse in [
        Mouse::Hover(-1, 0),
        Mouse::ScrollDown(0),
        Mouse::Hover(25, 0),
        Mouse::ScrollUp(2),
        Mouse::LeftClick(1, 4),
        Mouse::Hold(2, 4),
        Mouse::Release(2, 4),
        Mouse::LeftClick(0, 4),
        Mouse::Release(0, 4),
        Mouse::LeftClick(0, 4),
        Mouse::Release(1, 4),
        Mouse::Hover(19, 4),
        Mouse::ScrollUp(2),
        Mouse::ScrollDown(1),
        Mouse::LeftClick(20, 4),
        Mouse::Release(20, 4),
        Mouse::LeftClick(20, 4),
        Mouse::Release(20, 4),
        Mouse::LeftClick(1, 4),
        Mouse::LeftClick(1, 4),
        Mouse::ScrollDown(1),
        Mouse::ScrollUp(1),
    ] {
        eprintln!("agents mouse: {mouse:?}");
        pair.event(Event::Mouse(mouse));
        pair.render(26, 90);
    }
    pair.key("?");
    for mouse in [
        Mouse::ScrollDown(2),
        Mouse::ScrollUp(1),
        Mouse::RightClick(3, 4),
    ] {
        pair.event(Event::Mouse(mouse));
        pair.render(12, 60);
    }
}

#[test]
fn agents_actual_runtimes_match_overlay_resize_focus_ring_and_visibility() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_overlay_resize_focus_ring_and_visibility",
    );
    use yinz_types::{
        internal_ids::TabPosition,
        pane_id::PaneId,
        position::{Column, ColumnCount, Row, RowCount},
    };
    let (mut pair, _, _, _) = pair(&[], false);
    snapshot(&mut pair, 1, &[], 2);
    let tabs = serde_json::from_value(json!({"id":7,"pane_id":{"Plugin":1},"active":0,"bar_focused":[],"tabs":[{"descriptor":{"label":"Agents","location":"yinz:agents"},"plugin_id":1,"state":"Ready"}]})).unwrap();
    pair.event(Event::PaneTabsUpdate(vec![tabs]));
    let mut own = PaneInfo {
        id: PaneId::Plugin(PluginId::new(1)),
        is_floating: true,
        is_focused: true,
        pane_x: Column::new(3),
        pane_y: Row::new(5),
        pane_rows: RowCount::new(20),
        pane_columns: ColumnCount::new(60),
        ..Default::default()
    };
    let manifest = |pane: PaneInfo| PaneManifest {
        panes: std::collections::HashMap::from([(TabPosition::new(0), vec![pane])]),
    };
    pair.event(Event::PaneUpdate(manifest(own.clone())));
    for (rows, cols) in [(40, 120), (12, 22), (5, 12), (50, 150)] {
        pair.event(Event::TabUpdate(vec![TabInfo {
            active: true,
            display_area_rows: RowCount::new(rows),
            display_area_columns: ColumnCount::new(cols),
            ..Default::default()
        }]));
        pair.key("Ctrl o");
        pair.render(20, 60);
    }
    pair.event(Event::PluginConfigurationChanged(BTreeMap::from([
        ("width".into(), "70%".into()),
        ("height".into(), "14".into()),
        ("pad".into(), "1".into()),
    ])));
    for stop in ["tabs", "history", "management_tabs", "list", ""] {
        let mut message = snapshot_message(0, &[], 99);
        message.name = "agent_strip_take".into();
        message.args = BTreeMap::from([("stop".into(), stop.into())]);
        pair.pipe(message);
        pair.render(20, 60);
        for key in ["Shift Tab", "Tab", "Right", "Enter"] {
            press(&mut pair, key);
            pair.render(20, 60);
        }
    }
    own.is_focused = false;
    pair.event(Event::PaneUpdate(manifest(own.clone())));
    pair.render(20, 60);
    own.is_suppressed = true;
    pair.event(Event::PaneUpdate(manifest(own.clone())));
    pair.event(Event::Visible(false));
    own.is_suppressed = false;
    own.is_focused = true;
    pair.event(Event::PaneUpdate(manifest(own)));
    viewports(&mut pair);
}

#[test]
fn agents_actual_runtimes_match_streamed_history_and_summary_deadline() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_streamed_history_and_summary_deadline",
    );
    let mut run = run("claude:active", 7, false);
    let (mut pair, clock, wasm, steel) = pair(&[run.clone()], true);
    snapshot(&mut pair, 1, &[run.clone()], 2);
    pair.render(26, 80);
    pair.key("Tab");
    pair.key("Up");
    pair.key("Enter");
    pair.render(26, 80);
    run.entries.push(serde_json::from_value(json!({"revision":4,"kind":"message","text":"streamed **answer**","created_at_ms":91000})).unwrap());
    wasm.apply(store::SHARED, &agents::rows::run_mutations(&run));
    steel.apply(
        store::SHARED,
        &agents::rows::run_mutations(&current_run(&run)),
    );
    snapshot(&mut pair, 2, &[run.clone()], 2);
    viewports(&mut pair);
    pair.key("Esc");
    pair.key("Shift Tab");
    pair.key("s");
    pair.render(26, 80);
    clock.store(160000, Ordering::SeqCst);
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(2.0)));
    viewports(&mut pair);
    run.entries.push(serde_json::from_value(json!({"revision":5,"kind":"summary","text":"completed summary","created_at_ms":160000})).unwrap());
    wasm.apply(store::SHARED, &agents::rows::run_mutations(&run));
    steel.apply(
        store::SHARED,
        &agents::rows::run_mutations(&current_run(&run)),
    );
    snapshot(&mut pair, 3, &[run], 2);
    viewports(&mut pair);
}

#[test]
fn agents_actual_runtimes_match_archive_pagination_and_recorded_predecessors() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_archive_pagination_and_recorded_predecessors",
    );
    let mut runs: Vec<_> = (0..55)
        .map(|index| run(&format!("claude:archive-{index:02}"), index + 100, true))
        .collect();
    let mut active = run("claude:active", 7, false);
    active.previous_run_id = Some(runs[0].id.clone());
    runs[0].previous_run_id = Some(runs[1].id.clone());
    runs.push(active.clone());
    let (mut pair, _, _, _) = pair(&runs, true);
    snapshot(&mut pair, 1, &[active], 2);
    pair.render(16, 80);
    pair.key("p");
    viewports(&mut pair);
    pair.key("c");
    // The cursor returns to the focused live agent; one other live row and 50
    // stored runs precede the explicit pagination row.
    pair.key("Up");
    pair.key("Up");
    for _ in 0..52 {
        pair.key("Down");
        pair.render(12, 70);
    }
    let commands = pair.key("Enter");
    assert!(commands
        .iter()
        .any(|command| command.pointer("/DataFetch/request/offset") == Some(&json!(50))));
    viewports(&mut pair);
}

#[test]
fn agents_actual_runtimes_match_store_errors_search_retry_and_resume_failure() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agents_actual_runtimes_match_store_errors_search_retry_and_resume_failure",
    );
    use std::sync::atomic::AtomicBool;
    let runs = vec![
        run("claude:active", 7, false),
        run("claude:archived", 9, true),
    ];
    let clock = Arc::new(AtomicU64::new(90000));
    let fail = Arc::new(AtomicBool::new(false));
    let (_, wasm) = host(&runs, clock.clone());
    let (_, steel) = host(
        &runs.iter().map(current_run).collect::<Vec<_>>(),
        clock.clone(),
    );
    let failing = |original: Responder| -> Responder {
        let fail = fail.clone();
        Arc::new(move |command| {
            if fail.load(Ordering::SeqCst) && matches!(command, PluginCommand::DataFetch { .. }) {
                Some(
                    ProtobufDataFetchResponse::from(DataFetchResponse::Err(
                        "archive temporarily unavailable".into(),
                    ))
                    .encode_to_vec(),
                )
            } else {
                original(command)
            }
        })
    };
    let mut pair = Pair::with_hosts(
        "agents",
        BTreeMap::from([("view".into(), "agents".into())]),
        failing(wasm),
        failing(steel),
        Some(clock),
    );
    snapshot(&mut pair, 1, &runs, 2);
    pair.render(24, 90);
    pair.key("Down");
    fail.store(true, Ordering::SeqCst);
    pair.key("Down");
    viewports(&mut pair);
    pair.key("/");
    pair.event(Event::PastedText("history".into()));
    pair.key("Enter");
    viewports(&mut pair);
    fail.store(false, Ordering::SeqCst);
    pair.key("Enter");
    viewports(&mut pair);
    pair.key("Esc");
    pair.key("Down");
    pair.key("Down");
    pair.key("r");
    viewports(&mut pair);
    let commands = pair.key("y");
    assert!(commands
        .iter()
        .any(|command| command.get("OpenCommandPaneNearPlugin").is_some()));
    viewports(&mut pair);
}

struct HistoryPanel {
    panel: super::runtime_parity::SteelReference,
    worker: super::runtime_parity::SteelReference,
    clock: Arc<AtomicU64>,
    data: Arc<DataHost>,
}

// Model the service's reply to the panel's currently pending subscription.
fn opening_reply(
    panel: &mut super::runtime_parity::SteelReference,
    mut message: PipeMessage,
) -> PipeMessage {
    let token = panel
        .runtime
        .invoke("a-get", vec![json!("select_on_open")])
        .unwrap();
    if let Some(token) = token.as_str() {
        message.args.insert("open_request".into(), token.into());
    }
    message
}

fn focus_message(seq: u64, focused: Option<u32>, last: u32) -> PipeMessage {
    let mut message = snapshot_message(seq, &[], 2);
    let mut snapshot: Value = serde_json::from_str(message.payload.as_ref().unwrap()).unwrap();
    snapshot["focused"] = json!(focused);
    snapshot["last_focused_agent"] = json!(last);
    message.payload = Some(snapshot.to_string());
    message
}

fn focus_snapshot(
    panel: &mut HistoryPanel,
    seq: u64,
    focused: Option<u32>,
    last: u32,
) -> Option<bool> {
    let message = opening_reply(&mut panel.panel, focus_message(seq, focused, last));
    panel.panel.pipe(&message)
}

fn panel_visibility(panel: &mut HistoryPanel, visible: bool, focused: bool) {
    panel.panel.update(&Event::PaneUpdate(PaneManifest {
        panes: std::collections::HashMap::from([(
            yinz_types::internal_ids::TabPosition::new(0),
            vec![PaneInfo {
                id: yinz_types::pane_id::PaneId::Plugin(PluginId::new(1)),
                is_suppressed: !visible,
                is_focused: focused,
                ..Default::default()
            }],
        )]),
    }));
}

#[test]
fn agents_panel_preserves_browsed_selection_across_agent_focus_changes() {
    let mut h = HistoryPanel::new(&[]);
    focus_snapshot(&mut h, 1, Some(7), 7);
    assert_eq!(h.selection(), "pane:7");
    focus_snapshot(&mut h, 2, Some(8), 8);
    assert_eq!(h.selection(), "pane:7");
    assert_eq!(h.get("focused"), 8);

    h.key("Up");
    assert_eq!(h.selection(), "pane:8");
    focus_snapshot(&mut h, 3, Some(7), 7);
    assert_eq!(h.selection(), "pane:8");
    assert_eq!(h.get("focused"), 7);
}

#[test]
fn agents_panel_selects_last_focused_agent_on_first_open_and_reopen() {
    let mut h = HistoryPanel::new(&[]);
    panel_visibility(&mut h, true, true);
    focus_snapshot(&mut h, 1, None, 7);
    assert_eq!(h.selection(), "pane:7");
    h.key("Up");
    assert_eq!(h.selection(), "pane:8");

    // Losing keyboard focus while still shown does not start another visit.
    panel_visibility(&mut h, true, false);
    focus_snapshot(&mut h, 2, Some(7), 7);
    assert_eq!(h.selection(), "pane:8");
    panel_visibility(&mut h, true, true);
    focus_snapshot(&mut h, 3, None, 7);
    assert_eq!(h.selection(), "pane:8");

    panel_visibility(&mut h, false, false);
    h.panel.take_commands();
    panel_visibility(&mut h, true, true);
    assert!(h.panel.take_commands().iter().any(|command| {
        command.pointer("/MessageToPlugin/message_name") == Some(&json!("agent_subscribe"))
    }));
    // Selection waits for the fresh snapshot requested when opening.
    assert_eq!(h.selection(), "pane:8");
    focus_snapshot(&mut h, 4, None, 7);
    assert_eq!(h.selection(), "pane:7");
    focus_snapshot(&mut h, 5, Some(8), 8);
    assert_eq!(h.selection(), "pane:7");
}

#[test]
fn agents_panel_open_skips_closed_agents_and_clears_filters_for_open_agents() {
    let mut h = HistoryPanel::new(&[]);
    focus_snapshot(&mut h, 1, None, 7);
    h.key("w");
    assert_ne!(h.get("waiting_base"), false);
    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    focus_snapshot(&mut h, 2, None, 7);
    assert_eq!(h.get("waiting_base"), false);
    assert_eq!(h.selection(), "pane:7");

    h.key("Up");
    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    focus_snapshot(&mut h, 3, None, 999);
    assert_eq!(h.selection(), "pane:8");
    assert_eq!(h.get("select_on_open"), false);

    h.key("w");
    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    assert_eq!(focus_snapshot(&mut h, 4, None, 8), Some(true));
    assert_eq!(h.get("waiting_base"), false);
    assert_eq!(h.selection(), "pane:8");
}

#[test]
fn agents_panel_open_ignores_inflight_publications_and_invalid_replies() {
    let mut h = HistoryPanel::new(&[]);
    focus_snapshot(&mut h, 1, None, 7);
    h.key("Up");
    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    let token = h.get("select_on_open");
    assert!(token.is_string());

    // This newer publication was already in flight when the panel reopened.
    h.panel.pipe(&focus_message(2, None, 8));
    assert_eq!(h.selection(), "pane:8");
    assert_eq!(h.get("select_on_open"), token);

    let mut earlier_visit = focus_message(3, None, 8);
    earlier_visit.args.insert("open_request".into(), "1".into());
    h.panel.pipe(&earlier_visit);
    assert_eq!(h.get("select_on_open"), token);

    let reply = opening_reply(&mut h.panel, focus_message(4, None, 7));
    let mut untrusted = reply.clone();
    untrusted.source = PipeSource::Plugin(PluginId::new(99));
    h.panel.pipe(&untrusted);
    let mut malformed = reply.clone();
    malformed.payload = Some("{".into());
    h.panel.pipe(&malformed);
    let mut stale = reply.clone();
    stale.args.insert("seq".into(), "3".into());
    h.panel.pipe(&stale);
    assert_eq!(h.selection(), "pane:8");
    assert_eq!(h.get("select_on_open"), token);

    h.panel.pipe(&reply);
    assert_eq!(h.selection(), "pane:7");
    assert_eq!(h.get("select_on_open"), false);
    h.panel.pipe(&focus_message(5, Some(8), 8));
    assert_eq!(h.selection(), "pane:7");
}

#[test]
fn agents_panel_open_retries_keep_the_token_and_user_input_cancels_it() {
    let mut h = HistoryPanel::new(&[]);
    let token = h.get("select_on_open");
    h.panel.take_commands();
    h.panel.update(&Event::SessionUpdate(
        sessions().live_sessions,
        Default::default(),
    ));
    let mut hello = focus_message(0, None, 7);
    hello.name = "agent_hello".into();
    h.panel.pipe(&hello);
    let subscriptions: Vec<_> = h
        .panel
        .take_commands()
        .into_iter()
        .filter_map(|command| {
            let send = command.get("MessageToPlugin")?;
            (send["message_name"] == "agent_subscribe").then(|| send.clone())
        })
        .collect();
    assert_eq!(subscriptions.len(), 2);
    for send in subscriptions {
        assert_eq!(send["message_args"]["open_request"], token);
    }
    focus_snapshot(&mut h, 1, None, 7);
    panel_visibility(&mut h, true, true);
    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    let reply = opening_reply(&mut h.panel, focus_message(2, None, 7));
    h.key("Up");
    assert_eq!(h.get("select_on_open"), false);
    h.panel.pipe(&reply);
    assert_eq!(h.selection(), "pane:8");

    panel_visibility(&mut h, false, false);
    panel_visibility(&mut h, true, true);
    let hidden_reply = opening_reply(&mut h.panel, focus_message(3, None, 7));
    panel_visibility(&mut h, false, false);
    h.panel.pipe(&hidden_reply);
    assert_eq!(h.get("select_on_open"), false);
    assert_eq!(h.selection(), "pane:8");
}

#[test]
fn agents_panel_open_loads_focused_history_before_the_waiting_agents_history() {
    for publication_first in [false, true] {
        let runs = [run("focused", 7, false), run("waiting", 8, false)];
        let mut h = HistoryPanel::new(&runs);
        panel_visibility(&mut h, true, true);
        if publication_first {
            // Pane 8 sorts first because it is waiting for approval.
            h.panel.pipe(&snapshot_message(1, &runs, 2));
            assert_eq!(h.selection(), "pane:8");
            h.panel.update(&Event::Visible(true));
            assert!(h.requests().is_empty());
        }
        h.snapshot(2, &runs);
        assert_eq!(h.selection(), "pane:7");
        let request = h.request();
        let payload: Value = serde_json::from_str(request["payload"].as_str().unwrap()).unwrap();
        assert_eq!(payload["id"], "focused");
        assert_eq!(h.complete(&request)["run"]["id"], "focused");

        h.key("Up");
        let request = h.request();
        h.complete(&request);
        panel_visibility(&mut h, false, false);
        panel_visibility(&mut h, true, true);
        assert!(
            h.requests().is_empty(),
            "opening must wait for fresh focus before loading cached rows"
        );
        h.snapshot(3, &runs);
        let request = h.request();
        let payload: Value = serde_json::from_str(request["payload"].as_str().unwrap()).unwrap();
        assert_eq!(payload["id"], "focused");
    }
}

impl HistoryPanel {
    fn selection(&mut self) -> Value {
        self.panel
            .runtime
            .invoke("a-selection-key", vec![])
            .unwrap()
    }
    fn new(runs: &[agents::AgentRun]) -> Self {
        let clock = Arc::new(AtomicU64::new(90_000));
        let (data, responder) = host(runs, clock.clone());
        let panel = super::runtime_parity::SteelReference::with_clock(
            "agents",
            BTreeMap::from([("view".into(), "agents".into())]),
            std::path::Path::new(""),
            responder.clone(),
            Some(clock.clone()),
        );
        let worker = super::runtime_parity::SteelReference::worker(
            "agents",
            responder,
            Some(clock.clone()),
            "history",
        );
        Self {
            panel,
            worker,
            clock,
            data,
        }
    }
    fn snapshot(&mut self, seq: u64, runs: &[agents::AgentRun]) {
        let message = opening_reply(&mut self.panel, snapshot_message(seq, runs, 2));
        self.panel.pipe(&message);
    }
    fn key(&mut self, key: &str) -> bool {
        self.panel.update(&Event::Key(key.parse().unwrap()))
    }
    fn tick(&mut self, elapsed_ms: u64) {
        self.clock.fetch_add(elapsed_ms, Ordering::SeqCst);
        self.panel
            .update(&Event::Timer(yinz_types::units::FractionalSeconds::new(
                2.0,
            )));
    }
    fn get(&mut self, key: &str) -> Value {
        self.panel
            .runtime
            .invoke("a-get", vec![json!(key)])
            .unwrap()
    }
    fn requests(&self) -> Vec<Value> {
        self.panel
            .take_commands()
            .into_iter()
            .filter_map(|c| c.get("PostMessageTo").cloned())
            .collect()
    }
    fn request(&self) -> Value {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests[0].clone()
    }
    fn reply(&mut self, request: &Value, payload: &str) -> bool {
        self.panel.update(&Event::CustomMessage(
            request["name"].as_str().unwrap().into(),
            payload.into(),
        ))
    }
    fn complete(&mut self, request: &Value) -> Value {
        self.worker
            .runtime
            .invoke(
                "yinz-worker-message",
                vec![request["name"].clone(), request["payload"].clone()],
            )
            .unwrap();
        let replies: Vec<_> = self
            .worker
            .take_commands()
            .into_iter()
            .filter_map(|c| c.get("PostMessageToPlugin").cloned())
            .collect();
        assert_eq!(replies.len(), 1);
        let payload = replies[0]["payload"].as_str().unwrap();
        assert!(self.reply(request, payload));
        serde_json::from_str(payload).unwrap()
    }
}

#[test]
fn agents_prompt_preserves_draft_without_writing_to_held_or_exited_agents() {
    for status in ["Held", "Exited"] {
        let mut h = HistoryPanel::new(&[]);
        h.snapshot(1, &[]);
        h.panel
            .runtime
            .invoke("a-set!", vec![json!("focus"), json!("prompt")])
            .unwrap();
        h.panel.update(&Event::PastedText("keep this draft".into()));

        let mut message = snapshot_message(2, &[], 2);
        let mut snapshot: Value = serde_json::from_str(message.payload.as_ref().unwrap()).unwrap();
        snapshot["panes"][0]["status"] = json!(status);
        message.payload = Some(snapshot.to_string());
        h.panel.pipe(&message);
        h.panel.take_commands();

        assert!(h.key("Enter"));
        assert!(h
            .panel
            .take_commands()
            .iter()
            .all(|command| command.get("WriteToPaneId").is_none()));
        assert_eq!(
            h.panel.runtime.invoke("a-prompt-text", vec![]).unwrap(),
            "keep this draft"
        );
        let error = "Agent is not running. Restart it before sending a prompt.";
        assert_eq!(h.get("prompt_error").as_str().unwrap().trim(), error);
        let frame = history_frame(&h.panel.render(24, 90));
        assert!(frame.contains("Send failed:"), "{frame}");

        h.snapshot(3, &[]);
        h.panel.take_commands();
        assert!(h.key("Enter"));
        let writes: Vec<_> = h
            .panel
            .take_commands()
            .into_iter()
            .filter_map(|command| command.get("WriteToPaneId").cloned())
            .collect();
        assert_eq!(
            writes,
            vec![
                json!([b"\x1b[200~keep this draft\x1b[201~", {"Terminal": 7}]),
                json!([[13], {"Terminal": 7}]),
            ]
        );
        assert_eq!(h.panel.runtime.invoke("a-prompt-text", vec![]).unwrap(), "");
        assert_eq!(h.get("prompt_error"), false);
    }
}

#[test]
fn agents_prompt_search_applies_cancels_and_restores_drafts() {
    let runs = [run("prompt-run", 7, false)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    h.complete(&h.request());
    h.panel.render(24, 100);
    h.key("/");
    let query = "  prompt: history  ";
    h.panel.update(&Event::PastedText(query.into()));
    let frame = history_frame(&h.panel.render(24, 100));
    assert!(frame.contains("User prompts"));
    assert!(frame.contains("prompt: text"));
    assert!(!frame.contains("Ctrl-p"));
    assert!(h.panel.take_commands().iter().any(|command| {
        command.get("ShowCursor")
            == Some(&json!(["Search [User prompts]: ".len() + query.len(), 0]))
    }));
    h.key("Tab");
    h.key("/");
    assert_eq!(h.get("search_query"), query);
    h.key("Enter");
    assert_eq!(h.get("search_revisions")["prompt-run"], 1);
    assert_eq!(h.get("applied_search_query"), query);
    let title = h.get("rows")[0]["Run"]["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(title.starts_with('…'), "{title}");
    assert!(title.contains("history"), "{title}");
    let request = h.request();
    let payload: Value = serde_json::from_str(request["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["revision"], 1);
    let reply = h.complete(&request);
    assert!(reply["run"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "message"));
    h.panel.render(24, 100);
    assert_eq!(h.get("history_cursor_anchor"), json!([1]));
    // Changing the draft must not change the applied results when cancelled.
    h.key("/");
    assert_eq!(h.get("search_query"), query);
    h.key("Ctrl u");
    h.panel.update(&Event::PastedText("step".into()));
    h.key("Esc");
    assert_eq!(h.get("search_query"), query);
    assert!(history_frame(&h.panel.render(24, 100)).contains("User prompts"));
    assert_eq!(h.get("search_revisions")["prompt-run"], 1);
    h.key("/");
    h.key("Ctrl u");
    h.panel.update(&Event::PastedText("step".into()));
    h.key("Enter");
    assert_eq!(h.get("search_revisions")["prompt-run"], 2);
    assert_eq!(h.get("applied_search_query"), "step");
    assert!(history_frame(&h.panel.render(24, 100)).contains("All messages"));
    h.key("Esc");
    h.key("/");
    assert_eq!(h.get("search_query"), "");
    // A prefix-only draft also survives leaving the editor.
    h.panel.update(&Event::PastedText("prompt: ".into()));
    h.key("Tab");
    h.key("/");
    assert_eq!(h.get("search_query"), "prompt: ");
    h.key("Ctrl u");
    assert_eq!(h.get("search_query"), "");
    assert!(history_frame(&h.panel.render(24, 100)).contains("All messages"));
    h.panel.update(&Event::PastedText("prompt:".into()));
    h.key("Backspace");
    assert_eq!(h.get("search_query"), "prompt");
    assert!(history_frame(&h.panel.render(24, 100)).contains("All messages"));
    h.key("Esc");
    h.key("/");
    assert_eq!(h.get("search_query"), "");
}

#[test]
fn agents_prompt_search_reserves_only_a_leading_lowercase_prefix() {
    let mut assistant = run("assistant", 7, true);
    assistant.entries = vec![assistant.entries[1].clone()];
    assistant.entries[0].text = "Prompt: history prompts: history history prompt:".into();
    let mut h = HistoryPanel::new(&[assistant]);
    h.panel.render(24, 100);
    h.key("/");
    for query in ["", "   ", "prompt:", "  prompt:  \t"] {
        h.panel
            .runtime
            .invoke("a-set!", vec![json!("search_query"), json!(query)])
            .unwrap();
        h.panel.take_commands();
        let request = h.get("search_request");
        h.key("Enter");
        assert_eq!(h.get("search_query"), query);
        assert_eq!(h.get("search_request"), request);
        assert_eq!(h.get("search_editing"), true);
        assert_eq!(h.get("search_pending"), false);
        assert_eq!(h.get("applied_search_query"), false);
        assert!(!h
            .panel
            .take_commands()
            .iter()
            .any(|command| command.get("DataFetch").is_some()));
    }
    for (query, prompts_only) in [
        ("prompt:history", true),
        ("  prompt: history  ", true),
        ("history", false),
        ("Prompt: history", false),
        ("prompts: history", false),
        ("history prompt:", false),
    ] {
        h.key("Ctrl u");
        h.panel.update(&Event::PastedText(query.into()));
        assert_eq!(
            h.panel
                .runtime
                .invoke("a-search-prompts-only?", vec![])
                .unwrap(),
            prompts_only
        );
        h.key("Enter");
        assert_eq!(h.get("search_error"), false);
        assert_eq!(h.get("applied_search_query"), query);
        assert_eq!(
            h.get("search_revisions"),
            if prompts_only {
                json!({})
            } else {
                json!({"assistant": 2})
            }
        );
        h.key("/");
    }
}

#[test]
fn agents_prompt_search_filters_before_paging_and_substring_fallback() {
    let mut mixed = run("mixed", 7, true);
    let template = mixed.entries[0].clone();
    mixed.entries = (1..=250)
        .map(|revision| {
            let mut entry = template.clone();
            entry.revision = revision;
            entry.created_at_ms = Some(revision as i64);
            entry.kind = if revision <= 150 {
                agents::RunEntryKind::Message
            } else {
                agents::RunEntryKind::Prompt
            };
            entry.text = "needleword".into();
            entry
        })
        .collect();
    let mut prompt = run("prompt", 8, true);
    prompt.entries.truncate(1);
    prompt.entries[0].text = "needleword".into();
    let mut assistant = run("assistant", 9, true);
    assistant.entries = vec![mixed.entries[0].clone()];
    assistant.entries[0].text = "needleword needlewor".into();
    let mut tool = run("tool", 10, true);
    tool.entries = vec![mixed.entries[0].clone()];
    tool.entries[0].text = "Tool finished".into();
    tool.entries[0].tool = yinz_tile::agent::tool_capture::prepare(
        "codex",
        &yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("exec".into()),
            tool_response: Some(json!("needleword")),
            ..Default::default()
        },
    );
    assert!(tool.entries[0].tool.is_some());
    let runs = [mixed, prompt, assistant, tool];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    h.panel.render(24, 100);
    // An assistant's exact token must not prevent the prompt substring fallback.
    for query in ["needleword", "needlewor"] {
        h.key("/");
        h.key("Ctrl u");
        h.panel
            .update(&Event::PastedText(format!("prompt:{query}")));
        h.key("Enter");
        assert_eq!(h.get("search_error"), false);
        let revisions = h.get("search_revisions");
        assert_eq!(revisions.as_object().unwrap().len(), 2, "{revisions}");
        assert!(revisions["mixed"].as_u64().unwrap() > 150);
        assert_eq!(revisions["prompt"], 1);
        assert_eq!(h.get("search_truncated"), false);
        h.key("/");
        h.key("Ctrl u");
        h.panel.update(&Event::PastedText(query.into()));
        h.key("Enter");
        let expected = if query == "needleword" { 4 } else { 2 };
        assert_eq!(
            h.get("search_revisions").as_object().unwrap().len(),
            expected
        );
        h.key("Esc");
    }
    h.key("/");
    h.panel
        .update(&Event::PastedText("prompt: finished".into()));
    h.key("Enter");
    assert_eq!(h.get("search_revisions"), json!({}));
    assert!(history_frame(&h.panel.render(24, 100)).contains("no matches"));
}

#[test]
fn agents_follow_the_theme_on_startup_and_mode_updates() {
    use yinz_terminal::panes::{grid::Grid, AnsiCode};
    use yinz_types::style::{Style, StyleDeclaration};

    let theme = |foreground, background, selected_foreground, selected_background| {
        let declaration = |base, background| StyleDeclaration {
            base: PaletteColor::Rgb(base),
            background: PaletteColor::Rgb(background),
            emphasis_0: PaletteColor::Rgb(base),
            emphasis_1: PaletteColor::Rgb(base),
            emphasis_2: PaletteColor::Rgb(base),
            emphasis_3: PaletteColor::Rgb(base),
        };
        let mut style = Style::default();
        style.colors.text_unselected = declaration(foreground, background);
        style.colors.text_selected = declaration(selected_foreground, selected_background);
        style
    };
    let dark = theme((220, 225, 230), (20, 25, 30), (240, 245, 250), (50, 55, 60));
    let light = theme((30, 35, 40), (230, 235, 240), (10, 15, 20), (180, 185, 190));
    for themes in [[dark, light], [light, dark]] {
        let mut run = run("active", 7, false);
        run.entries.truncate(1);
        run.entries[0].text = "Themed history".into();
        let runs = [run];
        let mut h = HistoryPanel::new(&runs);
        assert!(h.panel.take_commands().iter().any(|command| command
            .get("Subscribe")
            .and_then(Value::as_array)
            .is_some_and(|events| events.contains(&json!("ModeUpdate")))));
        h.snapshot(1, &runs);
        h.complete(&h.request());
        let mut grid = Grid::new(
            24,
            90,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            themes[0],
            false,
            true,
            true,
            true,
            false,
        );
        for (index, style) in themes.into_iter().enumerate() {
            if index > 0 {
                grid.update_theme(style.colors);
                assert!(h.panel.update(&Event::ModeUpdate(ModeInfo {
                    style,
                    ..Default::default()
                })));
            }
            let frame = h.panel.render(24, 90);
            vte::Parser::new().advance(&mut grid, frame.as_bytes());
            let lines = grid.as_character_lines();
            for text in ["? claude", "codex", "Themed history", "Enter focus"] {
                let line = lines
                    .iter()
                    .find(|line| {
                        line.iter()
                            .map(|c| c.character)
                            .collect::<String>()
                            .contains(text)
                    })
                    .unwrap_or_else(|| panic!("missing {text}: {}", grid.dump_screen(false)));
                // The waiting agent's glyph and status take the attention color; its kind does not.
                // A cell index, not a byte offset: the glyphs before the kind are multibyte.
                let chars: String = line.iter().map(|c| c.character).collect();
                let start = chars[..chars.find(text).unwrap()].chars().count();
                let region = match text {
                    "? claude" => &line[..30],
                    "codex" => &line[start..start + text.chars().count()],
                    _ => &line[..],
                };
                for cell in region.iter().filter(|cell| !cell.character.is_whitespace()) {
                    let selected = cell.styles.background
                        == Some(style.colors.text_selected.background.into());
                    let foreground = if selected {
                        style.colors.text_selected.base
                    } else {
                        style.colors.text_unselected.base
                    };
                    assert_eq!(
                        cell.styles.foreground,
                        Some(foreground.into()),
                        "{text}: {cell:?}"
                    );
                    assert_ne!(cell.styles.dim, Some(AnsiCode::On), "{text}: {cell:?}");
                    assert_ne!(cell.styles.bold, Some(AnsiCode::On), "{text}: {cell:?}");
                }
                if text == "? claude" {
                    assert!(region.iter().all(|cell| cell.styles.background
                        == Some(style.colors.text_selected.background.into())));
                }
            }
        }
    }
}

#[test]
fn agents_pane_tabs_focus_updates_preserve_another_clients_history() {
    use yinz_plugin_api::plugin_ids::ProtobufPluginIds;

    let client = Arc::new(AtomicU64::new(1));
    let acting_client = client.clone();
    let (_, responder) = host(&[], Arc::new(AtomicU64::new(90_000)));
    let mut panel = super::runtime_parity::SteelReference::new(
        "agents",
        BTreeMap::from([("view".into(), "agents".into())]),
        std::path::Path::new(""),
        Arc::new(move |command| {
            if matches!(command, PluginCommand::GetPluginIds) {
                let mut ids =
                    ProtobufPluginIds::decode(default_reply(command)?.as_slice()).unwrap();
                ids.client_id = acting_client.load(Ordering::SeqCst) as u32;
                Some(ids.encode_to_vec())
            } else {
                responder(command)
            }
        }),
    );
    let tabs = |clients: &[u32]| {
        Event::PaneTabsUpdate(vec![serde_json::from_value(json!({
            "id":7,"pane_id":{"Plugin":1},"active":0,"bar_focused":clients,
            "tabs":[{"descriptor":{"label":"Agents","location":"yinz:agents"},"plugin_id":1,"state":"Ready"}]
        })).unwrap()])
    };
    let message = opening_reply(&mut panel, snapshot_message(1, &[], 2));
    panel.pipe(&message);
    panel.render(24, 90);
    assert!(panel.update(&Event::Key("Tab".parse().unwrap())));
    assert!(history_frame(&panel.render(24, 90)).contains("▸"));

    assert!(panel.update(&tabs(&[2])));
    // Shared renders leave the cached flag set for the last rendered client.
    client.store(2, Ordering::SeqCst);
    assert!(history_frame(&panel.render(24, 90)).contains("←/→ choose"));
    client.store(1, Ordering::SeqCst);
    assert!(panel.update(&tabs(&[])));
    assert!(history_frame(&panel.render(24, 90)).contains("▸"));

    assert!(panel.update(&tabs(&[1])));
    assert!(history_frame(&panel.render(24, 90)).contains("←/→ choose"));
    assert!(panel.update(&tabs(&[])));
    let frame = history_frame(&panel.render(24, 90));
    assert!(frame.contains("Enter focus"));
    assert!(!frame.contains("▸"));
}

#[test]
fn agents_history_navigation_renders_while_pending_and_only_loads_latest_selection() {
    let runs = [
        run("active", 7, false),
        run("archive-a", 9, true),
        run("archive-b", 10, true),
    ];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    let first = h.request();
    // The waiting pane precedes focused pane 7; visit it before the archives.
    for (key, expected) in [
        ("Up", "pane:8"),
        ("Down", "pane:7"),
        ("Down", "run:archive-a"),
        ("Down", "run:archive-b"),
    ] {
        assert!(h.key(key));
        assert_eq!(
            h.panel.runtime.invoke("a-selection-key", vec![]).unwrap(),
            expected
        );
        let frame = history_frame(&h.panel.render(24, 90));
        assert!(!frame.is_empty());
        if expected.starts_with("run:") {
            assert!(frame.contains("loading…"));
        }
        assert!(
            h.requests().is_empty(),
            "one history request may run at a time"
        );
    }
    // Invalid JSON proves stale bodies are discarded before decoding.
    assert!(!h.reply(&first, "obsolete body is not JSON"));
    assert_eq!(h.get("run_error"), false);
    let latest = h.request();
    assert_eq!(
        serde_json::from_str::<Value>(latest["payload"].as_str().unwrap()).unwrap()["id"],
        "archive-b"
    );
    h.complete(&latest);
    assert!(history_frame(&h.panel.render(24, 90)).contains("archive-b"));
    assert!(!history_frame(&h.panel.render(24, 90)).contains("loading…"));
    assert!(!h.reply(&first, "late duplicate"));
    assert!(h.requests().is_empty());
}

#[test]
fn agents_history_snapshot_sorts_newly_loaded_pages() {
    let runs: Vec<_> = (0..55)
        .map(|i| run(&format!("old-{i:02}"), i + 100, true))
        .collect();
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &[]);
    // Another server's inserts can move newer rows into the next offset page.
    for i in 0..55 {
        let mut new = run(&format!("new-{i:02}"), i + 200, true);
        new.started_at_ms = 9_000;
        h.data
            .apply(store::SHARED, &agents::rows::run_mutations(&new));
    }
    h.panel.runtime.invoke("a-load-more!", vec![]).unwrap();
    h.panel.runtime.invoke("a-refresh-strip!", vec![]).unwrap();
    h.snapshot(2, &[]);
    let headers = h.get("archived_runs");
    assert_eq!(headers.as_array().unwrap().len(), 55);
    assert_eq!(headers[0]["id"], "new-50");
    h.snapshot(3, &[]);
    assert_eq!(h.get("archived_runs"), headers);
    let mut updated = runs[0].clone();
    updated.started_at_ms = 20_000;
    h.snapshot(4, &[updated.clone()]);
    assert_eq!(h.get("archived_runs")[0]["id"], updated.id);
    updated.ended_at_ms = None;
    h.snapshot(5, &[updated]);
    assert_eq!(h.get("archived_runs")[0]["id"], "new-50");
    assert_eq!(h.get("archived_runs").as_array().unwrap().len(), 54);
}

#[test]
fn agents_history_snapshot_headers_refresh_errors_timeout_and_hide() {
    let mut runs = [run("archive", 9, true)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    h.key("Down");
    h.key("Down");
    let first = h.request();
    h.complete(&first);
    for seq in 2..12 {
        h.snapshot(seq, &runs);
    }
    assert!(
        h.requests().is_empty(),
        "unchanged snapshots must not fetch history"
    );
    runs[0].summary =
        Some(serde_json::from_value(json!({"text":"new summary","created_at_ms":91000})).unwrap());
    h.data
        .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
    h.snapshot(12, &runs);
    let changed = h.request();
    h.complete(&changed);
    h.snapshot(13, &runs);
    assert!(h.requests().is_empty());
    runs[0].revision += 1;
    h.snapshot(14, &runs);
    let revision = h.request();
    assert!(h.reply(&revision, r#"{"error":"archive unavailable"}"#));
    assert!(h
        .get("run_error")
        .to_string()
        .contains("archive unavailable"));
    assert!(history_frame(&h.panel.render(24, 90)).contains("archive unavailable"));
    h.snapshot(15, &runs);
    assert!(
        h.requests().is_empty(),
        "errors require a changed header or explicit retry"
    );
    assert!(h.key("Ctrl r"));
    let refresh = h.request();
    h.clock.fetch_add(30_000, Ordering::SeqCst);
    assert!(h
        .panel
        .update(&Event::Timer(yinz_types::units::FractionalSeconds::new(
            2.0
        ))));
    assert!(h.get("run_error").to_string().contains("did not answer"));
    assert!(!h.reply(&refresh, "expired"));
    h.key("Ctrl r");
    let hidden = h.request();
    h.panel.update(&Event::Visible(false));
    h.snapshot(16, &runs);
    assert!(h.requests().is_empty());
    assert!(!h.reply(&hidden, "hidden"));
    h.panel.update(&Event::Visible(true));
    assert!(h.requests().is_empty());
    // Without a remembered live agent, reopening retains the browsed archive.
    let mut message = snapshot_message(17, &runs, 2);
    let mut snapshot: Value = serde_json::from_str(message.payload.as_ref().unwrap()).unwrap();
    snapshot["last_focused_agent"] = Value::Null;
    snapshot["focused"] = Value::Null;
    message.payload = Some(snapshot.to_string());
    let message = opening_reply(&mut h.panel, message);
    h.panel.pipe(&message);
    assert_eq!(h.selection(), "run:archive");
    let reopened = h.request();
    h.data
        .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
    h.complete(&reopened);
    assert_eq!(h.get("run_error"), false);
    assert!(h.requests().is_empty());
}

#[test]
fn agents_history_retries_stale_replies_until_storage_catches_up_without_a_snapshot() {
    for change in ["revision", "summary", "completion"] {
        let mut runs = [current_run(&run("archive", 9, true))];
        let mut h = HistoryPanel::new(&runs);
        h.snapshot(1, &runs);
        h.key("Down");
        h.key("Down");
        let first = h.request();
        h.complete(&first);
        h.panel.render(24, 90);
        let installed = h.get("run_revision");
        match change {
            "revision" => {
                runs[0].revision += 1;
                runs[0].entries.push(
                    serde_json::from_value(json!({
                        "revision":4,"kind":"message","text":"caught up","created_at_ms":91000
                    }))
                    .unwrap(),
                );
            },
            "summary" => {
                runs[0].summary = Some(
                    serde_json::from_value(json!({
                        "text":"caught up","created_at_ms":91000
                    }))
                    .unwrap(),
                );
            },
            "completion" => runs[0].ended_at_ms = Some(61_000),
            _ => unreachable!(),
        }
        h.snapshot(2, &runs);
        let stale = h.request();
        let displayed = history_frame(&h.panel.render(24, 90));
        h.complete(&stale);
        assert_eq!(h.get("run_revision"), installed, "{change}");
        assert_eq!(
            history_frame(&h.panel.render(24, 90)),
            displayed,
            "{change}"
        );
        assert_eq!(h.get("run_error"), false);
        assert_eq!(h.get("history_retry_at"), 92_000);
        h.tick(1_999);
        h.snapshot(3, &runs);
        assert!(h.requests().is_empty(), "retry must wait two seconds");
        h.tick(1);
        let retry = h.request();
        h.tick(2_000);
        assert!(h.requests().is_empty(), "only one worker request at a time");
        h.complete(&retry);
        assert_eq!(h.get("history_retry_at"), 96_000);
        assert_eq!(h.get("run_revision"), installed);
        h.data
            .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
        h.tick(2_000);
        let retry = h.request();
        let reply = h.complete(&retry);
        assert_eq!(reply["run"]["revision"], runs[0].revision);
        assert_eq!(h.get("run_revision"), installed.as_u64().unwrap() + 1);
        assert_eq!(h.get("history_retry_at"), false);
        if change != "completion" {
            assert!(history_frame(&h.panel.render(24, 90)).contains("caught up"));
        }
        for seq in 4..7 {
            h.tick(2_000);
            h.snapshot(seq, &runs);
            assert!(h.requests().is_empty(), "success must stop retries");
        }
    }
}

#[test]
fn agents_history_accepts_replies_newer_than_the_advertised_header() {
    let advertised = current_run(&run("archive", 9, true));
    let mut stored = advertised.clone();
    stored.revision += 1;
    stored.summary = Some(
        serde_json::from_value(json!({
            "text":"newer summary","created_at_ms":91000
        }))
        .unwrap(),
    );
    stored.ended_at_ms = Some(61_000);
    let mut h = HistoryPanel::new(&[stored]);
    h.snapshot(1, std::slice::from_ref(&advertised));
    h.key("Down");
    h.key("Down");
    let request = h.request();
    h.complete(&request);
    assert_eq!(h.get("run_revision"), 1);
    assert_eq!(h.get("history_retry_at"), false);
    assert!(history_frame(&h.panel.render(24, 90)).contains("newer summary"));
    h.tick(2_000);
    h.snapshot(2, &[advertised]);
    assert!(h.requests().is_empty());
}

#[test]
fn agents_history_navigation_hiding_and_explicit_refresh_cancel_stale_retries() {
    for action in ["navigate", "hide", "refresh"] {
        let mut runs = [current_run(&run("active", 7, false))];
        let mut h = HistoryPanel::new(&runs);
        runs[0].revision += 1;
        h.snapshot(1, &runs);
        let stale = h.request();
        h.complete(&stale);
        assert_eq!(h.get("history_retry_at"), 92_000);
        assert_eq!(h.get("run_revision"), 0);
        match action {
            "navigate" => {
                assert!(h.key("Up"));
                assert_eq!(
                    h.panel.runtime.invoke("a-selection-key", vec![]).unwrap(),
                    "pane:8"
                );
            },
            "hide" => {
                h.panel.update(&Event::Visible(false));
            },
            "refresh" => {
                h.key("Ctrl r");
            },
            _ => unreachable!(),
        }
        assert_eq!(h.get("history_retry_at"), false, "{action}");
        if action == "refresh" {
            let refresh = h.request();
            h.tick(2_000);
            assert!(h.requests().is_empty());
            h.data
                .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
            h.complete(&refresh);
            assert_eq!(h.get("run_revision"), 1);
        }
        h.tick(2_000);
        assert!(h.requests().is_empty(), "{action}");
    }
}

#[test]
fn agents_history_search_revision_is_correlated_before_decoding() {
    let runs = [run("archive", 9, true)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    h.key("Down");
    h.key("Down");
    let first = h.request();
    h.key("/");
    h.panel.update(&Event::PastedText("history".into()));
    h.key("Enter");
    assert!(h.requests().is_empty());
    assert!(!h.reply(&first, "obsolete newest page"));
    let search = h.request();
    let body: Value = serde_json::from_str(search["payload"].as_str().unwrap()).unwrap();
    assert_eq!(body["revision"], 1);
    let reply = h.complete(&search);
    assert!(reply["run"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["revision"] == 1));
    h.panel.render(24, 90);
    assert_eq!(h.get("history_cursor_anchor"), json!([1]));
    h.key("c");
    let cleared = h.request();
    assert_eq!(
        serde_json::from_str::<Value>(cleared["payload"].as_str().unwrap()).unwrap()["revision"],
        false
    );
    h.complete(&cleared);
}

#[test]
fn agents_history_worker_preserves_utf8_budget_and_oversized_newest_entry() {
    for (older, newest, expected) in [
        ("old".into(), "界".repeat(100_000), 1),
        ("界".repeat(80_000), "界".repeat(10_000), 1),
        ("界".repeat(70_000), "界".repeat(10_000), 2),
    ] {
        let mut run = run("archive", 9, true);
        run.entries.truncate(2);
        run.entries[0].text = older;
        run.entries[1].text = newest.clone();
        let mut h = HistoryPanel::new(&[run.clone()]);
        h.snapshot(1, &[run]);
        h.key("Down");
        h.key("Down");
        let request = h.request();
        let reply = h.complete(&request);
        let entries = reply["run"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), expected);
        assert_eq!(entries.last().unwrap()["text"], newest);
        assert_eq!(reply["run"]["omitted_entries"], 2 - expected);
    }
}

fn history_frame(text: &str) -> String {
    let mut grid = yinz_terminal::panes::grid::Grid::new(
        24,
        90,
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(
        &mut grid,
        text.replace("\n\r", "\n").replace('\n', "\n\r").as_bytes(),
    );
    grid.as_character_lines()
        .iter()
        .map(|line| line.iter().map(|c| c.character).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn agents_history_queued_request_retains_its_client_and_unload_invalidates_it() {
    let runs = [run("active", 7, false), run("archive", 9, true)];
    let mut h = HistoryPanel::new(&runs);
    h.panel
        .runtime
        .eval(
            r#"
        (define sent-clients '())
        (define post-worker yinz-post-worker)
        (set! yinz-post-worker (lambda (worker name payload)
          (set! sent-clients (cons %async-client sent-clients))
          (post-worker worker name payload)))
        (yinz-async-enter 1)
    "#,
        )
        .unwrap();
    h.snapshot(1, &runs);
    let first = h.request();
    h.panel
        .runtime
        .invoke("yinz-async-enter", vec![json!(7)])
        .unwrap();
    h.key("Down");
    h.key("Down");
    assert!(h.requests().is_empty());
    h.panel
        .runtime
        .invoke("yinz-async-enter", vec![json!(1)])
        .unwrap();
    assert!(!h.reply(&first, "old client"));
    let latest = h.request();
    assert_eq!(
        h.panel.runtime.value("sent-clients").unwrap(),
        json!([7, 1])
    );
    h.panel.runtime.unload().unwrap();
    assert_eq!(h.get("history_desired"), false);
    assert!(!h.reply(&latest, "retired instance"));
}

#[test]
fn agents_history_footer_caches_prompt_document_and_skips_other_events() {
    let runs = [run("active", 7, false)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    let request = h.request();
    h.complete(&request);
    h.panel
        .runtime
        .eval(
            r#"
        (define document-parses 0)
        (define create-document yinz-document-create)
        (set! yinz-document-create (lambda (kind text)
          (set! document-parses (+ document-parses 1))
          (create-document kind text)))
    "#,
        )
        .unwrap();
    h.panel.render(24, 90);
    h.key("Tab");
    for _ in 0..4 {
        h.panel.runtime.eval("(yinz-request-render)").unwrap();
        h.panel.render(24, 90);
    }
    assert_eq!(h.panel.runtime.value("document-parses").unwrap(), 1);
    h.key("Enter");
    assert_eq!(
        h.panel.runtime.value("document-parses").unwrap(),
        1,
        "reader reuses footer document"
    );
    let sections = h
        .panel
        .runtime
        .invoke("a-reader", vec![json!("sections")])
        .unwrap();
    let fold = sections
        .as_array()
        .unwrap()
        .iter()
        .position(|section| section["lines"].as_u64().unwrap() > 1)
        .unwrap();
    for _ in 0..fold {
        h.key("j");
    }
    assert!(h.key("Enter"));
    assert_ne!(
        h.panel
            .runtime
            .invoke("a-reader", vec![json!("sections")])
            .unwrap(),
        sections
    );
    h.panel.render(24, 90);
    h.key("Esc");
    h.key("Enter");
    assert_eq!(
        h.panel
            .runtime
            .invoke("a-reader", vec![json!("sections")])
            .unwrap(),
        sections,
        "reopening a cached document keeps the original fold defaults"
    );
    h.key("Esc");
    h.key("Up");
    for _ in 0..4 {
        h.panel.runtime.eval("(yinz-request-render)").unwrap();
        h.panel.render(24, 90);
    }
    assert_eq!(
        h.panel.runtime.value("document-parses").unwrap(),
        1,
        "message footer needs no document"
    );
    h.key("Enter");
    assert_eq!(
        h.panel.runtime.value("document-parses").unwrap(),
        2,
        "message reader still parses its contents"
    );
}

type ProcessCommands = Arc<std::sync::Mutex<Vec<Value>>>;
fn history_process(
    responder: Responder,
    hold: Option<(
        yinz_test_support::Watched<bool>,
        std::sync::mpsc::Receiver<()>,
    )>,
) -> (yinz_steel::worker::WorkerRuntime, ProcessCommands) {
    use crate::response::{decode_response, DecodedResponse};
    let commands = ProcessCommands::default();
    let recorded = commands.clone();
    let mut hold = hold;
    let binary = std::env::var_os("YINZ_COMPILATION_WORKER")
        .expect("cargo x test builds the production worker");
    let mut runtime = yinz_steel::worker::WorkerRuntime::new(std::path::Path::new(&binary), move |value| {
        let command = yinz_steel::binary::decode_command(value.clone())?;
        recorded.lock().unwrap().push(value);
        if matches!(&command, PluginCommand::DataFetch { request } if request.entity == "agent_run")
            && let Some((waiting, release)) = hold.take() {
                *waiting.lock() = true;
                release.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
            }
        let Some(response) = responder(&command) else { return Ok(Value::Null); };
        Ok(match decode_response(CommandType::from(&command), &response).transpose()? {
            Some(DecodedResponse::Json(value)) => value,
            Some(DecodedResponse::Bytes(bytes)) => serde_json::to_value(bytes)?,
            None => serde_json::to_value(response)?,
        })
    }).unwrap();
    runtime.set_source_root(
        yinz_steel::modules::SourceRoot::file(
            &yinz_steel_builtins::asset_dir().join("agents.scm"),
            None,
        )
        .unwrap(),
    );
    runtime
        .eval(&yinz_steel_builtins::fixture_source("agents").unwrap())
        .unwrap();
    (runtime, commands)
}
fn process_messages(commands: &ProcessCommands, kind: &str) -> Vec<Value> {
    std::mem::take(&mut *commands.lock().unwrap())
        .into_iter()
        .filter_map(|c| c.get(kind).cloned())
        .collect()
}

#[test]
fn agents_history_real_worker_probes_navigation_during_snapshot_bursts() {
    use std::time::{Duration, Instant};
    for size in [1024, 256 * 1024, 2 * 1024 * 1024] {
        let mut runs = [
            run("active", 7, false),
            run("archive-a", 9, true),
            run("archive-b", 10, true),
        ];
        for run in &mut runs {
            run.entries[2].text = "x".repeat(size);
            run.revision = 3;
        }
        let (_, responder) = host(&runs, Arc::new(AtomicU64::new(90_000)));
        let waiting = yinz_test_support::Watched::new(false);
        let (release, held) = std::sync::mpsc::channel();
        let (mut worker, replies) =
            history_process(responder.clone(), Some((waiting.clone(), held)));
        worker
            .invoke("yinz-worker-bind", vec![json!("history")])
            .unwrap();
        let (mut panel, commands) = history_process(responder, None);
        panel.load(json!({"view":"agents"})).unwrap();
        commands.lock().unwrap().clear();
        let mut first = snapshot_message(1, &runs, 2);
        first.args.insert("open_request".into(), "1".into());
        panel.pipe(serde_json::to_value(first).unwrap()).unwrap();
        let requests = process_messages(&commands, "PostMessageTo");
        assert_eq!(requests.len(), 1);
        let request = requests[0].clone();
        let thread = std::thread::spawn(move || {
            worker
                .invoke(
                    "yinz-worker-message",
                    vec![request["name"].clone(), request["payload"].clone()],
                )
                .unwrap();
            worker
        });
        waiting.wait_until(
            "history fetch withheld",
            Duration::from_secs(30),
            |waiting| (*waiting).then_some(()),
        );
        // Starting at focused pane 7, the two archives follow it directly.
        for expected in ["run:archive-a", "run:archive-b"] {
            assert!(panel
                .update(crate::event::event_to_json(&Event::Key("Down".parse().unwrap())).unwrap())
                .unwrap());
            assert_eq!(panel.invoke("a-selection-key", vec![]).unwrap(), expected);
        }
        let mut latency = Vec::new();
        for i in 0..32 {
            let key = if i % 2 == 0 { "Up" } else { "Down" };
            let before = Instant::now();
            assert!(panel
                .update(crate::event::event_to_json(&Event::Key(key.parse().unwrap())).unwrap())
                .unwrap());
            let frame = panel.render(24, 90).unwrap();
            latency.push(before.elapsed());
            assert!(history_frame(&frame).contains("loading…"));
            assert_eq!(
                panel.invoke("a-selection-key", vec![]).unwrap(),
                if i % 2 == 0 {
                    "run:archive-a"
                } else {
                    "run:archive-b"
                }
            );
            for burst in 0..4 {
                panel
                    .pipe(
                        serde_json::to_value(snapshot_message(2 + i * 4 + burst, &runs, 2))
                            .unwrap(),
                    )
                    .unwrap();
            }
            assert!(process_messages(&commands, "PostMessageTo").is_empty());
        }
        release.send(()).unwrap();
        let mut worker = thread.join().unwrap();
        let obsolete = process_messages(&replies, "PostMessageToPlugin");
        assert_eq!(obsolete.len(), 1);
        assert!(!panel
            .update(json!({"CustomMessage":[obsolete[0]["name"], obsolete[0]["payload"]]}))
            .unwrap());
        let latest = process_messages(&commands, "PostMessageTo");
        assert_eq!(latest.len(), 1);
        let before = Instant::now();
        worker
            .invoke(
                "yinz-worker-message",
                vec![latest[0]["name"].clone(), latest[0]["payload"].clone()],
            )
            .unwrap();
        let loaded = process_messages(&replies, "PostMessageToPlugin");
        assert_eq!(loaded.len(), 1);
        assert!(panel
            .update(json!({"CustomMessage":[loaded[0]["name"], loaded[0]["payload"]]}))
            .unwrap());
        let frame = history_frame(&panel.render(24, 90).unwrap());
        assert!(frame.contains("archive-b"));
        assert!(!frame.contains("loading…"));
        latency.sort();
        eprintln!(
            "history {size} bytes: cursor+frame p95 {:?}, max {:?}; history install+frame {:?}",
            latency[30],
            latency[31],
            before.elapsed()
        );
        panel
            .pipe(serde_json::to_value(snapshot_message(200, &runs, 2)).unwrap())
            .unwrap();
        assert!(process_messages(&commands, "PostMessageTo").is_empty());
    }
}

#[test]
fn agents_history_entry_fetch_failure_is_visible_and_refresh_retries() {
    use std::sync::atomic::AtomicBool;
    let runs = [run("active", 7, false)];
    let mut h = HistoryPanel::new(&runs);
    let (_, responder) = host(&runs, h.clock.clone());
    let fail = Arc::new(AtomicBool::new(true));
    let failing = fail.clone();
    h.worker = super::runtime_parity::SteelReference::worker(
        "agents",
        Arc::new(move |command| {
            if failing.load(Ordering::SeqCst)
                && matches!(command, PluginCommand::DataFetch { request } if request.entity == "agent_run_entry")
            {
                Some(
                    ProtobufDataFetchResponse::from(DataFetchResponse::Err(
                        "entry read failed".into(),
                    ))
                    .encode_to_vec(),
                )
            } else {
                responder(command)
            }
        }),
        Some(h.clock.clone()),
        "history",
    );
    h.snapshot(1, &runs);
    let request = h.request();
    let reply = h.complete(&request);
    assert!(reply["error"]
        .as_str()
        .unwrap()
        .contains("entry read failed"));
    assert!(history_frame(&h.panel.render(24, 90)).contains("entry read failed"));
    fail.store(false, Ordering::SeqCst);
    h.key("Ctrl r");
    let request = h.request();
    h.complete(&request);
    assert_eq!(h.get("run_error"), false);
    assert!(history_frame(&h.panel.render(24, 90)).contains("Investigate history"));
}

#[test]
fn agents_history_changed_header_preserves_cursor_and_scroll_anchors() {
    let mut runs = [run("active", 7, false)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    let request = h.request();
    h.complete(&request);
    h.panel.render(12, 90);
    h.key("Tab");
    h.key("Up");
    h.panel.render(12, 90);
    // Scroll with list focus so the viewport stays anchored independently of cursor reveal.
    h.key("Shift Tab");
    h.key("PageUp");
    h.panel.render(12, 90);
    let cursor = h.get("history_cursor_anchor");
    let bottom = h.get("history_bottom_anchor");
    assert_ne!(bottom, false);
    runs[0].entries.push(
        serde_json::from_value(
            json!({"revision":4,"kind":"message","text":"new output","created_at_ms":91000}),
        )
        .unwrap(),
    );
    runs[0].revision = 4;
    h.data
        .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
    h.snapshot(2, &runs);
    let request = h.request();
    h.complete(&request);
    h.panel.render(12, 90);
    assert_eq!(h.get("history_cursor_anchor"), cursor);
    assert_eq!(h.get("history_bottom_anchor"), bottom);
}

#[test]
fn agents_history_search_refresh_uses_the_latest_complete_header() {
    let mut runs = [run("active", 7, false)];
    let mut h = HistoryPanel::new(&runs);
    h.snapshot(1, &runs);
    let request = h.request();
    h.complete(&request);
    h.key("/");
    h.panel.update(&Event::PastedText("history".into()));
    h.key("Enter");
    let request = h.request();
    h.complete(&request);
    h.snapshot(2, &runs);
    assert!(
        h.requests().is_empty(),
        "unchanged snapshots must preserve newer search headers"
    );
    runs[0].summary = Some(
        serde_json::from_value(
            json!({"text":"summary from search","title":"refreshed","created_at_ms":91000}),
        )
        .unwrap(),
    );
    h.data
        .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
    h.key("/");
    h.key("Enter");
    let request = h.request();
    h.complete(&request);
    h.snapshot(3, &runs);
    assert!(h.requests().is_empty());
    runs[0].summary.as_mut().unwrap().text = "summary from snapshot".into();
    h.data
        .apply(store::SHARED, &agents::rows::run_mutations(&runs[0]));
    h.snapshot(4, &runs);
    let request = h.request();
    h.complete(&request);
    h.snapshot(5, &runs);
    assert!(h.requests().is_empty());
}

#[path = "agents_render.rs"]
mod render_tests;

#[path = "agents_activity.rs"]
mod activity_tests;

#[path = "agents_usage.rs"]
mod usage_tests;
