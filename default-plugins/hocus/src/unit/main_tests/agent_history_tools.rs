//! `agent_runs`, `agent_run` and `agent_history_search` — the tools that read back
//! what coding agents on this machine have already been asked and have already said.
//!
//! All three run against the temporary store, seeded with real `agent_run` rows
//! through the same `agents::rows` mutations hocus-agentd writes, so what is under
//! test is hocus's half and not a stand-in for the daemon's.

use super::*;
use hocus_proto::agents::{AgentRun, RunEntry, RunEntryKind, RunOutcome};
use hocus_proto::run_history;
use hocus_proto::store::Backend;

fn entry(revision: u64, kind: RunEntryKind, text: &str) -> RunEntry {
    RunEntry {
        revision,
        event_id: Some(format!("event-{revision}")),
        kind,
        text: text.to_owned(),
        created_at_ms: Some(1_000 + revision as i64),
    }
}

fn seed(state: &State, id: &str, kind: &str, session: &str, cwd: &str, entries: Vec<RunEntry>) {
    seed_run(state, id, kind, session, cwd, None, entries);
}

/// The same, for a run whose pane is on another machine.
fn seed_on_host(state: &State, id: &str, cwd: &str, host: &str) {
    seed_run(state, id, "claude", "main", cwd, Some(host), Vec::new());
}

/// And for a run in a Firecracker VM: its folder is a guest path, and `worktree` is the
/// folder on this machine the VM mirrored.
fn seed_in_vm(state: &State, id: &str, worktree: &str) {
    let run = AgentRun {
        worktree: Some(worktree.to_owned()),
        ..stored_run(
            id,
            "claude",
            "main",
            "/workspace",
            Some("firecracker:review"),
            Vec::new(),
        )
    };
    state
        .store()
        .apply(hocus_proto::agents::rows::run_mutations(&run))
        .expect("seeded the run");
}

fn seed_run(
    state: &State,
    id: &str,
    kind: &str,
    session: &str,
    cwd: &str,
    host: Option<&str>,
    entries: Vec<RunEntry>,
) {
    let run = stored_run(id, kind, session, cwd, host, entries);
    state
        .store()
        .apply(hocus_proto::agents::rows::run_mutations(&run))
        .expect("seeded the run");
}

fn stored_run(
    id: &str,
    kind: &str,
    session: &str,
    cwd: &str,
    host: Option<&str>,
    entries: Vec<RunEntry>,
) -> AgentRun {
    AgentRun {
        id: id.to_owned(),
        pane_id: 7,
        zellij_session_name: session.to_owned(),
        session_id: id.to_owned(),
        kind: kind.to_owned(),
        cwd: Some(cwd.to_owned()),
        host: host.map(str::to_owned),
        worktree: None,
        started_at_ms: 1_000,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: entries.len() as u64,
        summarized_revision: 0,
        summary: None,
        entries,
    }
}

/// A state with a project and two runs in it: one here, one in another session.
fn state_with_runs() -> State {
    let mut state = state_with_project();
    state.zellij_session_name = "main".to_owned();
    seed(
        &state,
        "claude:one",
        "claude",
        "main",
        "/work/hocus",
        vec![
            entry(1, RunEntryKind::Prompt, "fix the flaky render test"),
            entry(2, RunEntryKind::Plan, "rewrite the snapshot helper"),
            entry(3, RunEntryKind::Result, "the parser now handles tabs"),
        ],
    );
    seed(
        &state,
        "codex:two",
        "codex",
        "elsewhere",
        "/work/other",
        vec![entry(1, RunEntryKind::Prompt, "add a parser benchmark")],
    );
    state
}

fn tool(state: &mut State, name: &str, arguments: &str) -> serde_json::Value {
    let result = state
        .run_local(&agent_call(name, arguments))
        .unwrap_or_else(|| panic!("{name} is not a local tool"));
    serde_json::from_str(&result)
        .unwrap_or_else(|_| panic!("{name} answered with an error: {result}"))
}

fn error(state: &mut State, name: &str, arguments: &str) -> String {
    let result = state
        .run_local(&agent_call(name, arguments))
        .unwrap_or_else(|| panic!("{name} is not a local tool"));
    assert!(
        serde_json::from_str::<serde_json::Value>(&result).is_err(),
        "{name} answered with JSON, not an error: {result}"
    );
    result
}

// -- listing ------------------------------------------------------------

/// The default is the machine, not the session. A run recorded by a session that
/// has since closed is the main thing this whole feature exists to reach.
#[test]
fn runs_from_every_session_are_listed_by_default() {
    let mut state = state_with_runs();
    let listed = tool(&mut state, "agent_runs", "{}");
    let ids: Vec<&str> = listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude:one", "codex:two"]);
    assert_eq!(listed["scope"], "all sessions");
    assert_eq!(listed["more"], false);
}

#[test]
fn a_listing_narrows_to_this_session() {
    let mut state = state_with_runs();
    let listed = tool(&mut state, "agent_runs", r#"{"this_session":true}"#);
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["runs"][0]["run_id"], "claude:one");
    assert_eq!(listed["scope"], "session main");
}

#[test]
fn a_listing_narrows_by_kind_and_by_project() {
    let mut state = state_with_runs();
    let by_kind = tool(&mut state, "agent_runs", r#"{"kind":"codex"}"#);
    assert_eq!(by_kind["count"], 1);
    assert_eq!(by_kind["runs"][0]["kind"], "codex");

    let by_project = tool(&mut state, "agent_runs", r#"{"project":"hocus"}"#);
    assert_eq!(by_project["count"], 1);
    assert_eq!(by_project["runs"][0]["run_id"], "claude:one");
    // And the project the run's folder belongs to is named back.
    assert_eq!(by_project["runs"][0]["project"], "hocus");
}

/// A folder on a build box that spells the same path as a local project is not that
/// project: the run is neither listed under it nor annotated with its name.
#[test]
fn a_run_from_another_machine_belongs_to_no_local_project() {
    let mut state = state_with_runs();
    seed_on_host(&state, "claude:remote", "/work/hocus", "build-box");

    let by_project = tool(&mut state, "agent_runs", r#"{"project":"hocus"}"#);
    assert_eq!(by_project["count"], 1);
    assert_eq!(by_project["runs"][0]["run_id"], "claude:one");

    let listed = tool(&mut state, "agent_runs", "{}");
    let remote = listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] == "claude:remote")
        .expect("the remote run is still listed");
    assert_eq!(remote["project"], serde_json::Value::Null);
}

/// The pair the test above reads with, and the one that has to hold in both directions
/// at once: a VM run is annotated with the project of the worktree its VM mirrored, and
/// the filter for that project lists it. Either half alone would let the listing say a
/// run is in a project it then leaves out of that project's page.
#[test]
fn a_firecracker_run_is_listed_and_annotated_with_the_project_its_vm_mirrored() {
    let mut state = state_with_runs();
    seed_in_vm(&state, "claude:vm", "/work/hocus");

    let by_project = tool(&mut state, "agent_runs", r#"{"project":"hocus"}"#);
    let listed_ids: Vec<&str> = by_project["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["run_id"].as_str().unwrap())
        .collect();
    assert!(listed_ids.contains(&"claude:vm"), "{listed_ids:?}");

    let vm = by_project["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] == "claude:vm")
        .expect("the VM run is listed under the project");
    assert_eq!(vm["project"], "hocus");
    // And the folder it names is still the guest one, which is where the process was.
    assert_eq!(vm["cwd"], "/workspace");
}

#[test]
fn a_listing_names_an_unknown_project_rather_than_answering_emptily() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_runs", r#"{"project":"nowhere"}"#);
    assert!(refused.starts_with("unknown project nowhere"), "{refused}");
    assert!(refused.contains("project_list"), "{refused}");
}

/// An unnamed session is not the same as "every session": matching on the empty
/// name would quietly select rows nobody asked about.
#[test]
fn this_session_before_the_session_is_named_says_so() {
    let mut state = state_with_runs();
    state.zellij_session_name = String::new();
    let refused = error(&mut state, "agent_runs", r#"{"this_session":true}"#);
    assert!(refused.contains("not named yet"), "{refused}");
}

#[test]
fn an_unparseable_window_names_the_forms_it_accepts() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_runs", r#"{"since":"last tuesday"}"#);
    assert!(refused.contains("30m"), "{refused}");
    assert!(refused.contains("7d"), "{refused}");
    // The last two are refused rather than crashing: their unit is a multi-byte
    // character, and a window is split on its last *character* for that reason.
    for bad in ["0d", "-1d", "5", "5y", "3ч", "7日"] {
        let refused = error(&mut state, "agent_runs", &format!(r#"{{"since":"{bad}"}}"#));
        assert!(refused.starts_with("since must be"), "{bad}: {refused}");
    }
}

#[test]
fn a_window_keeps_the_runs_inside_it() {
    let mut state = state_with_runs();
    // Both runs began at epoch millisecond 1 000, which is decades ago, so a
    // window of an hour excludes them and one of a thousand weeks does not.
    let recent = tool(&mut state, "agent_runs", r#"{"since":"1h"}"#);
    assert_eq!(recent["count"], 0);
    let ancient = tool(&mut state, "agent_runs", r#"{"since":"9999w"}"#);
    assert_eq!(ancient["count"], 2);
}

// -- reading one run ----------------------------------------------------

#[test]
fn a_run_reads_back_with_its_entries_in_order() {
    let mut state = state_with_runs();
    let read = tool(&mut state, "agent_run", r#"{"run_id":"claude:one"}"#);
    assert_eq!(read["run_id"], "claude:one");
    assert_eq!(read["entry_count"], 3);
    let kinds: Vec<&str> = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["prompt", "plan", "result"]);
    assert_eq!(read["entries"][0]["text"], "fix the flaky render test");
    assert_eq!(read["more"], serde_json::Value::Null);
}

#[test]
fn an_unknown_run_is_named_rather_than_answered_emptily() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_run", r#"{"run_id":"claude:nope"}"#);
    assert!(refused.starts_with("no such run"), "{refused}");
    assert!(refused.contains("agent_runs"), "{refused}");
}

#[test]
fn a_run_with_neither_a_run_id_nor_a_pane_says_what_it_needs() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_run", "{}");
    assert!(refused.contains("run_id is required"), "{refused}");
    assert!(refused.contains("pane_id"), "{refused}");
}

/// The live case: the model has a pane id from `agent_list` and no run id.
#[test]
fn a_pane_resolves_to_the_run_the_panel_holds_for_it() {
    let mut state = state_with_runs();
    let header = run_history::run_header(&state.store(), "claude:one")
        .unwrap()
        .unwrap();
    state.active_runs.insert(4, header);
    let read = tool(&mut state, "agent_run", r#"{"pane_id":4}"#);
    assert_eq!(read["run_id"], "claude:one");
}

#[test]
fn a_pane_with_no_run_says_so() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_run", r#"{"pane_id":99}"#);
    assert!(refused.contains("no agent run"), "{refused}");
}

/// A hit is read *around*: the entry before it comes too, so the result arrives
/// with the prompt it was answering.
#[test]
fn a_revision_is_read_around_rather_than_from() {
    let mut state = state_with_runs();
    let read = tool(
        &mut state,
        "agent_run",
        r#"{"run_id":"claude:one","revision":3}"#,
    );
    let revisions: Vec<u64> = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["revision"].as_u64().unwrap())
        .collect();
    assert_eq!(revisions, [1, 2, 3], "the hit arrived with its lead-in");
}

/// The bound that matters: a local tool result is never passed through
/// `MAX_TOOL_OUTPUT`, so the handler has to hold itself under it.
#[test]
fn a_long_run_is_paged_under_the_output_cap_and_says_how_to_continue() {
    let mut state = state_with_project();
    let text = "x".repeat(5_000);
    let entries: Vec<RunEntry> = (1..=100)
        .map(|n| entry(n, RunEntryKind::Result, &text))
        .collect();
    seed(
        &state,
        "claude:big",
        "claude",
        "main",
        "/work/hocus",
        entries,
    );

    let result = state
        .run_local(&agent_call(
            "agent_run",
            r#"{"run_id":"claude:big","from_revision":1,"limit":100}"#,
        ))
        .unwrap();
    assert!(
        result.len() < crate::MAX_TOOL_OUTPUT,
        "{} bytes is over the cap",
        result.len()
    );
    let read: serde_json::Value = serde_json::from_str(&result).expect("valid JSON, not a cut one");
    assert_eq!(read["entry_count"], 100);
    assert!(read["entries"].as_array().unwrap().len() < 100);
    let more = read["more"].as_str().expect("a way to read the rest");
    assert!(
        more.starts_with("agent_run run_id=claude:big from_revision="),
        "{more}"
    );
}

/// The escape hatch that keeps the cap from hiding anything: one entry at a time
/// carries that entry whole.
#[test]
fn a_page_of_one_carries_a_long_entry_whole() {
    let mut state = state_with_project();
    let text = "y".repeat(9_000);
    seed(
        &state,
        "claude:one-big",
        "claude",
        "main",
        "/work/hocus",
        vec![entry(1, RunEntryKind::Plan, &text)],
    );
    let read = tool(
        &mut state,
        "agent_run",
        r#"{"run_id":"claude:one-big","from_revision":1,"limit":1}"#,
    );
    assert_eq!(read["entries"][0]["text"].as_str().unwrap().len(), 9_000);
}

// -- searching ----------------------------------------------------------

#[test]
fn a_search_reaches_every_session_and_names_the_run_it_found() {
    let mut state = state_with_runs();
    let found = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"benchmark"}"#,
    );
    assert_eq!(found["count"], 1);
    assert_eq!(found["matches"][0]["run_id"], "codex:two");
    assert_eq!(found["matches"][0]["agent"], "codex");
    assert_eq!(found["matches"][0]["kind"], "prompt");
    assert_eq!(found["scope"], "all sessions");
}

#[test]
fn a_search_without_a_query_says_what_it_needs() {
    let mut state = state_with_runs();
    let refused = error(&mut state, "agent_history_search", "{}");
    assert_eq!(refused, "query is required");
}

#[test]
fn a_search_that_matches_nothing_answers_emptily_rather_than_failing() {
    let mut state = state_with_runs();
    let found = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"nothing here at all"}"#,
    );
    assert_eq!(found["count"], 0);
    assert_eq!(found["matches"], serde_json::json!([]));
}

#[test]
fn a_search_narrows_by_session_project_and_run() {
    let mut state = state_with_runs();
    let here = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"parser","this_session":true}"#,
    );
    assert_eq!(here["count"], 1);
    assert_eq!(here["matches"][0]["run_id"], "claude:one");

    let by_project = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"parser","project":"hocus"}"#,
    );
    assert_eq!(by_project["count"], 1);
    assert_eq!(by_project["matches"][0]["run_id"], "claude:one");

    let in_run = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"parser","run_id":"codex:two"}"#,
    );
    assert_eq!(in_run["count"], 1);
    assert_eq!(in_run["matches"][0]["run_id"], "codex:two");
}

/// The whole navigation model, end to end: a hit names an address, and that
/// address is one `agent_run` accepts.
#[test]
fn a_search_hit_addresses_a_run_that_agent_run_reads() {
    let mut state = state_with_runs();
    let found = tool(
        &mut state,
        "agent_history_search",
        r#"{"query":"snapshot"}"#,
    );
    let hit = &found["matches"][0];
    let run_id = hit["run_id"].as_str().unwrap().to_owned();
    let revision = hit["revision"].as_u64().unwrap();
    assert_eq!(
        hit["read"].as_str().unwrap(),
        format!("agent_run run_id={run_id} revision={revision}")
    );

    let read = tool(
        &mut state,
        "agent_run",
        &format!(r#"{{"run_id":"{run_id}","revision":{revision}}}"#),
    );
    assert!(read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["revision"].as_u64() == Some(revision)));
}
