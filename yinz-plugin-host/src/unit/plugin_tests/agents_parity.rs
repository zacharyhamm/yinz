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
    let (steel, sh) = host(runs, clock.clone());
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
    pair.pipe(snapshot_message(seq, runs, source));
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
    for data in [&wasm, &steel] {
        data.apply(store::SHARED, &agents::rows::run_mutations(&run));
    }
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
    for data in [&wasm, &steel] {
        data.apply(store::SHARED, &agents::rows::run_mutations(&run));
    }
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
    let (_, steel) = host(&runs, clock.clone());
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
