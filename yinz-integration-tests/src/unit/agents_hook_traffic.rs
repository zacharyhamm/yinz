use super::*;
use agent_proto::{agents, store};
use serde_json::json;
use std::{collections::BTreeMap, io::Write, time::Instant};
use yinz_storage::data_access::DataAccess;
use yinz_test_support::Watched;
use yinz_types::data::*;

const SIZE: Size = Size {
    rows: 40,
    cols: 120,
};
const AGENTS: usize = 20;
const EVENTS: usize = 3_000;
const PERIOD: Duration = Duration::from_millis(10);
const NAV_PERIOD: Duration = Duration::from_millis(250);
const WAIT: Duration = Duration::from_secs(10);

// The same oversized-newest-entry case as the history-worker host fixture.
fn archive(id: &str, text: String) -> agents::AgentRun {
    serde_json::from_value(json!({
        "id":id,"pane_id":999,"kind":"claude","session_id":id,
        "started_at_ms":1000,"ended_at_ms":2000,"outcome":"completed","revision":2,
        "entries":[
            {"revision":1,"kind":"prompt","text":id,"created_at_ms":1000},
            {"revision":2,"kind":"message","text":text,"created_at_ms":2000}
        ]
    }))
    .unwrap()
}

fn panel_fixture(directory: &std::path::Path, address: std::net::SocketAddr) -> PathBuf {
    yinz_steel_builtins::BUILTIN_STEEL_FILES
        .extract(directory)
        .unwrap();
    let path = directory.join("agents.scm");
    let source = yinz_steel_builtins::builtin_steel_plugin("agents").unwrap();
    assert!(source.contains("(define (load configuration)"));
    let mut source = source.replacen(
        "(define (load configuration)",
        "(define (traffic-load configuration)",
        1,
    );
    // Only the first large-history request is gated. The panel and history handler stay real.
    source.push_str(&format!(
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "ChangeApplicationState"
      "PluginData" "SharedPluginData" "MessageAndLaunchOtherPlugins" "WebAccess" "RunCommands"))
    (lambda (_) (traffic-load configuration))))
(define traffic-held #f)
(yinz-register-workers (hash "history" (lambda (name payload)
  (if (and (not traffic-held) (equal? (hash-ref (yinz-json-parse payload) "id") "traffic-large"))
    (begin (set! traffic-held #t)
      (yinz-then (yinz-web-request (hash "url" "http://{address}/history"))
        (lambda (_) (a-history-worker name payload))))
    (a-history-worker name payload)))))
"#
    ));
    std::fs::write(&path, source).unwrap();
    path
}

fn hook(agent: usize, round: usize) -> (String, Vec<(String, String)>) {
    let phase = round % 10;
    let turn = format!("traffic-{agent}-{}", round / 10);
    let marker = format!("event-{agent}-{round}");
    let name = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "UserPromptSubmit",
        "SessionEnd",
    ][phase];
    let raw = json!({
        "hook_event_name":name,"session_id":turn,"turn_id":format!("{turn}-{phase}"),
        "prompt":marker,"tool_name":"Bash","tool_use_id":marker,
        "tool_input":{"command":format!("echo {marker}")},"tool_response":marker,
        "last_assistant_message":marker,"claude_turn_complete":true,
        "assistant_messages":[{"source_id":marker,"turn_id":format!("{turn}-{phase}"),"text":format!("observed:{marker}")}]
    });
    let (event, raw) =
        yinz_runtime::agent::hooks::parse_hook_event("claude", raw.to_string()).unwrap();
    let mut entries = vec![("message".into(), format!("observed:{marker}"))];
    if let Some(tool) = &event.tool_capture {
        entries.push((
            if tool.status == "requested" {
                "tool_call"
            } else {
                "tool_result"
            }
            .into(),
            tool.text(),
        ));
    }
    if let Some((kind, text)) = agents::run_entry(&event) {
        entries.push((kind.label().into(), text));
    }
    (raw, entries)
}

fn selected(frame: &GridSnapshot, title: &str) -> bool {
    frame
        .text
        .lines()
        .any(|line| line.contains(title) && line.trim_start_matches([' ', '│']).starts_with('>'))
}

fn pace(deadline: Instant) {
    // Absolute deadlines preserve the offered rate even if a send takes longer than one period.
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
}

fn cli_reply(session: &TestSession, action: CliAction) -> (Instant, String) {
    let (client, output) = session.spawn_cli_client(action);
    let completed = Watched::new(None);
    let done = completed.clone();
    let waiter = std::thread::spawn(move || {
        let code = client.join().unwrap();
        *done.lock() = Some((Instant::now(), code));
    });
    let (at, code) = completed.wait_until_described(
        "CLI reply after queued work",
        WAIT,
        |done| *done,
        |_| test_env::log_tail(40),
    );
    waiter.join().unwrap();
    let output = output
        .server_messages()
        .lock()
        .unwrap()
        .iter()
        .filter_map(|message| message.split_once(": "))
        .filter_map(|(kind, text)| {
            matches!(kind, "Log" | "LogError" | "CliPipeOutput").then_some(text)
        })
        .collect::<String>();
    assert_eq!(code, 0, "{output}");
    (at, output)
}

// Run on an idle machine with this worktree's release binary and optimized test harness:
// cargo x build --release
// YINZ_COMPILATION_WORKER="$PWD/target/release/yinz" cargo nextest run --release -p yinz-integration-tests --lib -E 'test(agents_panel_keeps_p95)' --no-capture
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "30-second performance gate requires an optimized production build"
)]
fn agents_panel_keeps_p95_below_100ms_at_100_hooks_per_second() {
    test_env::init();
    let directory = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let path = panel_fixture(directory.path(), listener.local_addr().unwrap());
    let (held_tx, held_rx) = mpsc::channel();
    let gate = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        held_tx.send(stream).unwrap();
    });
    let data = DataAccess::sqlite(&test_env::init().join("data"));
    for def in store::shared_defs() {
        data.plugin_data()
            .define_entity(store::SHARED, "traffic-fixture", &def, false)
            .unwrap();
    }
    let large = archive(
        "traffic-large",
        format!(
            "LARGE HISTORY LOADED\n{}",
            "x".repeat(2 * 1024 * 1024 - "LARGE HISTORY LOADED\n".len())
        ),
    );
    let small = archive("traffic-small", "SMALL HISTORY END".into());
    for run in [&large, &small] {
        data.plugin_data()
            .apply(store::SHARED, &agents::rows::run_mutations(run))
            .unwrap();
    }
    let terminals = (0..AGENTS)
        .map(|i| format!("pane command=\"claude\" name=\"agent-{i}\";"))
        .collect::<String>();
    let config = format!(
        r#"
agent_pane_min_columns 0
agent_pane_min_rows 0
plugins {{
  agent-service location="yinz:agent-service" {{ shared true; summaries false; }}
}}
load_plugins clear-defaults=true {{ "agent-service"; }}
keybinds {{
  normal {{
    bind "Ctrl b" {{ LaunchOrFocusPlugin "file:{}" {{ floating true; view "agents"; }}; }}
  }}
}}
"#,
        path.display()
    );
    let (stdout, output) = crossbeam::channel::unbounded();
    let mut session = TestRunner::new(SIZE)
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n pane stacked=true {{ {terminals} }}\n}}"
        )))
        .with_stdout_tap(stdout)
        .start();
    let panes: Vec<_> = (0..AGENTS)
        .map(|_| session.expect_pty_spawn().terminal_id())
        .collect();
    let reports = session
        .fake_server_os_api
        .agent_report
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let frames = Watched::new((
        Instant::now(),
        GridSnapshot {
            text: String::new(),
            cursor: None,
            styles: vec![],
        },
        Vec::<(Instant, String)>::new(),
    ));
    let observed = frames.clone();
    let observer_done = Watched::new(false);
    let finished = observer_done.clone();
    let observer = std::thread::spawn(move || {
        let mut pane = crate::client_screen::build_terminal_pane(SIZE);
        let mut parser = vte::Parser::new();
        while let Ok(bytes) = output.recv() {
            parser.advance(&mut pane.grid, &bytes);
            let frame = crate::client_screen::snapshot_terminal_pane(&mut pane);
            let at = Instant::now();
            let selection = ["traffic-large", "traffic-small"]
                .into_iter()
                .find(|title| selected(&frame, title));
            let mut observed = observed.lock();
            if let Some(selection) = selection {
                if observed.2.last().is_none_or(|(_, old)| old != selection) {
                    observed.2.push((at, selection.into()));
                }
            }
            observed.0 = at;
            observed.1 = frame;
        }
        *finished.lock() = true;
    });
    frames.wait_until_described(
        "all terminal panes rendered",
        WAIT,
        |(_, frame, _)| frame.contains("agent-19").then_some(()),
        |(_, frame, _)| format!("{}\n{}", frame.text, test_env::log_tail(40)),
    );
    session.send_stdin(&keys::ctrl('b'));
    frames.wait_until_described(
        "panel permission request",
        WAIT,
        |(_, frame, _)| frame.contains("Plugin permissions").then_some(()),
        |(_, frame, _)| format!("{}\n{}", frame.text, test_env::log_tail(40)),
    );
    session.send_stdin(b"\t\r");
    frames.wait_until_described(
        "initialized Agents panel",
        WAIT,
        |(_, frame, _)| {
            (frame.contains("Agents") && frame.contains("nothing to show yet")).then_some(())
        },
        |(_, frame, _)| frame.text.clone(),
    );
    session.send_stdin(&b"\x1b[B".repeat(AGENTS + 4));
    frames.wait_until_described(
        "small archive selected",
        WAIT,
        |(_, frame, _)| selected(frame, "traffic-small").then_some(()),
        |(_, frame, _)| frame.text.clone(),
    );
    session.send_stdin(b"\x1b[A");
    frames.wait_until("large history pending", WAIT, |(_, frame, _)| {
        (selected(frame, "traffic-large") && frame.contains("loading…")).then_some(())
    });
    let mut held = held_rx.recv_timeout(WAIT).unwrap();
    gate.join().unwrap();
    let initial_selections = frames.lock().2.len();

    let workload: Vec<_> = (0..EVENTS).map(|i| hook(i % AGENTS, i / AGENTS)).collect();
    let mut expected: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (i, (_, entries)) in workload.iter().enumerate() {
        expected
            .entry(format!("claude:traffic-{}-{}", i % AGENTS, i / AGENTS / 10))
            .or_default()
            .extend(entries.clone());
    }
    let start = Instant::now() + Duration::from_millis(100);
    let (produced, production) = mpsc::channel();
    let producer = std::thread::spawn(move || {
        let mut max_lag = Duration::ZERO;
        for (i, (raw, _)) in workload.into_iter().enumerate() {
            let due = start + PERIOD * i as u32;
            pace(due);
            max_lag = max_lag.max(due.elapsed());
            reports(panes[i % AGENTS], "claude".into(), raw);
        }
        produced.send((Instant::now(), max_lag)).unwrap();
    });
    let input = session.stdin_sender();
    let (navigated, navigation_done) = mpsc::channel();
    let navigation = std::thread::spawn(move || {
        let mut sent = Vec::new();
        for i in 0..120 {
            pace(start + NAV_PERIOD * i + Duration::from_millis(5));
            if i == 60 {
                held.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            }
            let at = Instant::now();
            let (bytes, title) = if i % 2 == 0 {
                (b"\x1b[B", "traffic-small")
            } else {
                (b"\x1b[A", "traffic-large")
            };
            input.send(bytes.to_vec()).unwrap();
            sent.push((at, title));
        }
        navigated.send(sent).unwrap();
    });
    let (offered_end, max_lag) = production
        .recv_timeout(Duration::from_secs(30) + WAIT)
        .unwrap();
    producer.join().unwrap();
    let sent = navigation_done.recv_timeout(WAIT).unwrap();
    navigation.join().unwrap();
    let rendered = frames.wait_until_described(
        "every navigation rendered during hook traffic",
        WAIT,
        |(_, _, changes)| {
            (changes.len() >= initial_selections + sent.len())
                .then(|| changes[initial_selections..].to_vec())
        },
        |(_, frame, changes)| {
            format!(
                "{} of {} selections; last frame:\n{}",
                changes.len() - initial_selections,
                sent.len(),
                frame.text
            )
        },
    );
    let mut latency = Vec::new();
    assert_eq!(
        rendered.len(),
        sent.len(),
        "unexpected selection changes under hook traffic"
    );
    for ((enqueued, wanted), (rendered, actual)) in sent.iter().zip(&rendered) {
        assert_eq!(actual, wanted);
        latency.push(
            rendered
                .checked_duration_since(*enqueued)
                .expect("selection must follow its input"),
        );
    }
    latency.sort();
    let p50 = latency[latency.len().div_ceil(2) - 1];
    let p95 = latency[(latency.len() * 95).div_ceil(100) - 1];
    let rate = (EVENTS - 1) as f64 / offered_end.duration_since(start).as_secs_f64();
    eprintln!(
        "offered rate {rate:.2}/s; lag {max_lag:?}; p50 {p50:?}, p95 {p95:?}, max {:?}",
        latency.last()
    );
    // The screen reply precedes the service reply, so both queues have processed every hook.
    let (_, panes) = cli_reply(
        &session,
        CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        },
    );
    let panes: Vec<PaneListEntry> = serde_json::from_str(&panes).unwrap();
    assert_eq!(
        panes
            .iter()
            .filter(|pane| !pane.pane_info.is_plugin())
            .count(),
        AGENTS
    );
    assert!(
        panes
            .iter()
            .filter(|pane| !pane.pane_info.is_plugin())
            .all(|pane| pane.pane_info.agent.is_none()),
        "final agent states: {panes:?}"
    );
    let (drained, context) = cli_reply(
        &session,
        CliAction::Operations(yinz_cli::operations::Operations::Context { pane_id: None }),
    );
    let context: serde_json::Value = serde_json::from_str(&context).unwrap();
    assert_eq!(context["ok"], true, "service context failed: {context}");
    let context = &context["data"];
    assert_eq!(context["pane_candidates"].as_array().unwrap().len(), AGENTS);
    assert!(
        context["pane_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|pane| pane["agent"].is_null()),
        "final service state: {context}"
    );
    let rows = data
        .plugin_data()
        .fetch(
            store::SHARED,
            &FetchRequest::entity("agent_run").limit(1000),
        )
        .unwrap();
    assert!(!rows.truncated);
    let runs = agents::rows::runs_from_rows(&rows);
    assert_eq!(runs.len(), expected.len() + 2);
    for id in expected.keys() {
        let run = runs.iter().find(|run| &run.id == id).unwrap();
        assert!(run.ended_at_ms.is_some(), "unfinished run: {id}");
        assert_eq!(run.outcome, agents::RunOutcome::Completed);
    }
    for (id, entries) in &expected {
        let rows = data
            .plugin_data()
            .fetch(store::SHARED, &agents::rows::entries_request([id.clone()]))
            .unwrap();
        assert!(!rows.truncated, "history unexpectedly truncated: {id}");
        let actual = agents::rows::entries_from_rows(&rows).remove(id).unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|entry| (entry.kind.label().into(), entry.text.clone()))
                .collect::<Vec<_>>(),
            *entries,
            "lost or reordered hooks in {id}"
        );
    }
    frames.wait_until_described(
        "large history rendered after release",
        WAIT,
        |(_, frame, _)| {
            (selected(frame, "traffic-large")
                && frame.contains("LARGE HISTORY LOADED")
                && !frame.contains("loading…"))
            .then_some(())
        },
        |(_, frame, _)| frame.text.clone(),
    );
    eprintln!("{EVENTS} hooks / {AGENTS} agents: {rate:.2}/s, producer lag {max_lag:?}; input-to-render p50 {p50:?}, p95 {p95:?}, max {:?}; drain {:?}", latency.last().unwrap(), drained.duration_since(offered_end));
    assert!(
        rate >= 99.0 && max_lag < Duration::from_millis(100),
        "producer failed to sustain 100 hooks/s"
    );
    assert!(
        p95 < Duration::from_millis(100),
        "navigation p95 {p95:?} exceeds 100 ms"
    );
    assert!(
        drained.duration_since(offered_end) < Duration::from_secs(1),
        "hook processing accumulated a backlog"
    );
    session.quit();
    drop(session);
    observer_done.wait_until("render observer stopped", WAIT, |done| {
        (*done).then_some(())
    });
    observer.join().unwrap();
}
