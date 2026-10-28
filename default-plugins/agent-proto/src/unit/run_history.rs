/// The machine these runs were recorded on.
const HOST: &str = "host-a";

use super::*;
use crate::agents::{AgentRun, RunOutcome};
use crate::store::{self, shared_defs};
use yinz_tile::prelude::EntityDef;

#[test]
fn predecessors_follow_only_links_and_report_incomplete_prefixes() {
    let mut anchor = run("anchor", "claude", "main", 30).header();
    let (empty, diagnostic) = predecessors(&anchor, |_| panic!("no link to load"));
    assert!(empty.is_empty());
    assert!(diagnostic.is_empty());
    anchor.previous_run_id = Some("near".into());
    let mut near = run("near", "claude", "main", 20).header();
    near.previous_run_id = Some("old".into());
    let old = run("old", "claude", "elsewhere", 10).header();
    let (chain, diagnostic) = predecessors(&anchor, |id| {
        Ok(match id {
            "near" => Some(near.clone()),
            "old" => Some(old.clone()),
            _ => panic!("unrecorded link"),
        })
    });
    assert_eq!(
        chain.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
        ["near", "old"]
    );
    assert!(diagnostic.is_empty());
    for failure in ["missing", "cycle", "self", "store"] {
        let mut near = near.clone();
        if failure == "cycle" {
            near.previous_run_id = Some("anchor".into());
        }
        if failure == "self" {
            near.previous_run_id = Some("near".into());
        }
        let (chain, diagnostic) = predecessors(&anchor, |id| {
            if id == "near" {
                return Ok(Some(near.clone()));
            }
            if failure == "store" {
                Err("store offline".into())
            } else {
                Ok(None)
            }
        });
        assert_eq!(chain.len(), 1);
        assert!(diagnostic.starts_with("Incomplete chain:"));
        assert!(diagnostic.contains(match failure {
            "missing" => "missing run old",
            "store" => "store offline",
            _ => "repeated run",
        }));
    }
}

pub(super) fn backend() -> store::Local {
    let backend = store::Local::shared("agents");
    backend
        .define_all(&shared_defs())
        .expect("declared the shared entities");
    backend
}

pub(super) fn entry(revision: u64, kind: RunEntryKind, text: &str, at: i64) -> RunEntry {
    RunEntry {
        tool: None,
        revision,
        event_id: Some(format!("event-{revision}")),
        kind,
        text: text.to_owned(),
        created_at_ms: Some(at),
    }
}

/// One entry row as the v1 definition held it: no `host_id`, which came later.
fn old_entry_put(run_id: &str, entry: &RunEntry) -> yinz_tile::prelude::Mutation {
    yinz_tile::prelude::Mutation::put(rows::ENTRY_ENTITY)
        .uid(format!("{run_id}#{}", entry.revision))
        .set("run_id", run_id)
        .set("revision", entry.revision as i64)
        .set("kind", entry.kind.label())
        .set("text", entry.text.clone())
        .build()
}

pub(super) fn run(id: &str, kind: &str, session: &str, started_at_ms: i64) -> AgentRun {
    AgentRun {
        id: id.to_owned(),
        pane_id: 7,
        yinz_session_name: session.to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: id.to_owned(),
        kind: kind.to_owned(),
        cwd: Some("/work/agent".to_owned()),
        host: None,
        bubblewrap: None,
        started_at_ms,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 0,
        summarized_revision: 0,
        summary: None,
        entries: Vec::new(),
        omitted_entries: 0,
    }
}

pub(super) fn write(backend: &impl Backend, run: &AgentRun) {
    backend
        .apply(rows::run_mutations(run))
        .expect("wrote the run");
}

pub(super) fn seeded() -> store::Local {
    let backend = backend();
    let mut first = run("claude:one", "claude", "main", 1_000);
    first.entries = vec![
        entry(1, RunEntryKind::Prompt, "fix the flaky render test", 1_000),
        entry(2, RunEntryKind::Plan, "rewrite the snapshot helper", 1_100),
        entry(
            3,
            RunEntryKind::Result,
            "the parser now handles tabs",
            1_200,
        ),
    ];
    let mut second = run("codex:two", "codex", "other", 2_000);
    second.cwd = Some("/work/other".to_owned());
    second.entries = vec![entry(
        1,
        RunEntryKind::Prompt,
        "add a parser benchmark",
        2_000,
    )];
    write(&backend, &first);
    write(&backend, &second);
    backend
}

#[test]
fn tool_pages_continue_when_the_store_byte_cap_shortens_a_fetch() {
    let backend = backend();
    let mut run = run("large-tools", "codex", "main", 1);
    let hook = yinz_tile::agent::HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("shell".into()),
        tool_response: Some(serde_json::json!("bounded-output ".repeat(8000))),
        ..Default::default()
    };
    let capture = yinz_tile::agent::tool_capture::prepare("codex", &hook).unwrap();
    for revision in 1..=100 {
        let mut e = entry(revision, RunEntryKind::ToolResult, &capture.text(), 1);
        e.tool = Some(capture.clone());
        run.entries.push(e);
    }
    write(&backend, &run);
    let mut from = Some(1);
    let mut revisions = Vec::new();
    while let Some(next) = from {
        let page = entry_page(&backend, &run.id, Some(next), 100).unwrap();
        assert!(!page.entries.is_empty());
        assert!(page
            .entries
            .iter()
            .all(|e| e.tool.as_ref().unwrap().payload_omitted));
        revisions.extend(page.entries.iter().map(|e| e.revision));
        from = page.next_revision;
    }
    assert_eq!(revisions, (1..=100).collect::<Vec<_>>());
}

#[test]
fn tool_payload_round_trip_search_and_exact_retrieval() {
    let backend = backend();
    let mut run = run("tool-run", "codex", "main", 1);
    let hook = yinz_tile::agent::HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("exec_command".into()),
        tool_use_id: Some("one".into()),
        tool_response: Some(
            serde_json::json!({"text":"searchable needle", "password":"never-store-this"}),
        ),
        ..Default::default()
    };
    let capture = yinz_tile::agent::tool_capture::prepare("codex", &hook).unwrap();
    for revision in 1..=3 {
        let mut e = entry(revision, RunEntryKind::ToolResult, &capture.text(), 1);
        e.tool = Some(capture.clone());
        run.entries.push(e);
    }
    write(&backend, &run);
    let page = entry_page(&backend, &run.id, Some(1), 2).unwrap();
    assert_eq!(page.next_revision, Some(3));
    assert!(page.entries[0].tool.as_ref().unwrap().payload_omitted);
    assert!(page.entries[0]
        .tool
        .as_ref()
        .unwrap()
        .output
        .as_ref()
        .unwrap()
        .text
        .is_empty());
    let exact = entry_page(&backend, &run.id, Some(2), 1).unwrap();
    assert_eq!(exact.entries[0].tool, Some(capture));
    assert_eq!(
        search(
            &backend,
            &Filter::default(),
            Some(&run.id),
            "needle",
            false,
            10
        )
        .unwrap()
        .hits
        .len(),
        3
    );
    assert!(search(
        &backend,
        &Filter::default(),
        Some(&run.id),
        "never-store-this",
        false,
        10
    )
    .unwrap()
    .hits
    .is_empty());
}

// -- searching ------------------------------------------------------

#[test]
fn retained_tool_payloads_are_searchable_and_hits_stay_distinct() {
    let backend = backend();
    let mut r = run("tool-run", "codex", "main", 1);
    let mut e = entry(1, RunEntryKind::Message, "Tool finished", 1);
    e.tool = yinz_tile::agent::tool_capture::prepare(
        "codex",
        &yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("exec".into()),
            tool_response: Some(serde_json::json!("captured-payload-needle")),
            ..Default::default()
        },
    );
    assert!(e.tool.is_some());
    r.entries = vec![e];
    write(&backend, &r);
    let found = search_runs_filtered(&backend, "captured-payload-needle", |_| true).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(found.hits[0].snippet.contains("captured-payload-needle"));
    let found = search(
        &backend,
        &Default::default(),
        Some(&r.id),
        "captured-payload-needle",
        false,
        50,
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    r.entries[0].text = "captured-payload-needle".into();
    write(&backend, &r);
    assert_eq!(
        search_runs(&backend, "captured-payload-needle")
            .unwrap()
            .hits
            .len(),
        1
    );
}

#[test]
fn literal_tool_search_decodes_payloads_and_preserves_scope() {
    let backend = backend();
    let mut r = run("literal-tools", "codex", "main", 1);
    let payload = r#"said "Hello World" at C:\Work\File"#;
    let mut e = entry(1, RunEntryKind::Message, "Tool finished", 1);
    e.tool = yinz_tile::agent::tool_capture::prepare(
        "codex",
        &yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("exec".into()),
            tool_response: Some(serde_json::json!(payload)),
            ..Default::default()
        },
    );
    r.entries = vec![e];
    write(&backend, &r);
    let scoped = Filter {
        kind: Some("codex".into()),
        ..Default::default()
    };
    for query in [
        r#""hello world""#,
        r#"c:\work\file"#,
        "absent",
        "payload_omitted",
    ] {
        let expected = usize::from(payload.to_lowercase().contains(query));
        let mut results = vec![search_runs(&backend, query).unwrap()];
        for (filter, id) in [
            (Filter::default(), None),
            (scoped.clone(), None),
            (Filter::default(), Some(r.id.as_str())),
        ] {
            results.push(search(&backend, &filter, id, query, false, 50).unwrap());
        }
        for found in results {
            assert_eq!(found.hits.len(), expected, "{query}");
            assert!(!found.truncated);
            if expected > 0 {
                assert!(found.hits[0].snippet.to_lowercase().contains(query));
            }
        }
        assert!(search(
            &backend,
            &Filter {
                kind: Some("claude".into()),
                ..Default::default()
            },
            None,
            query,
            false,
            50,
        )
        .unwrap()
        .hits
        .is_empty());
        assert!(
            search_runs_filtered(&backend, query, |header| header.kind == "claude")
                .unwrap()
                .hits
                .is_empty()
        );
    }
    r.entries[0].text = payload.into();
    write(&backend, &r);
    for id in [None, Some(r.id.as_str())] {
        let found = search(&backend, &scoped, id, "Hello", false, 50).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert!(!found.truncated);
    }
    assert_eq!(search_runs(&backend, "Hello").unwrap().hits.len(), 1);
}

#[test]
fn tool_search_pages_nonmatches_and_reports_scan_ceiling() {
    let backend = backend();
    let mut r = run("paged-tools", "codex", "main", 1);
    let capture = yinz_tile::agent::tool_capture::prepare(
        "codex",
        &yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("exec".into()),
            tool_response: Some(serde_json::json!("unrelated output")),
            ..Default::default()
        },
    )
    .unwrap();
    r.entries = (1..=102)
        .map(|n| {
            let mut e = entry(n, RunEntryKind::Message, "Tool finished", n as i64);
            e.tool = Some(capture.clone());
            e
        })
        .collect();
    r.entries[0]
        .tool
        .as_mut()
        .unwrap()
        .output
        .as_mut()
        .unwrap()
        .text = r#"found "Needle" at C:\Work\File"#.into();
    write(&backend, &r);
    for query in [r#""needle""#, r#"c:\work\file"#] {
        let mut results = vec![search_runs(&backend, query).unwrap()];
        for id in [None, Some(r.id.as_str())] {
            results.push(search(&backend, &Filter::default(), id, query, false, 50).unwrap());
        }
        for found in results {
            assert_eq!(found.hits.len(), 1);
            assert_eq!(found.hits[0].revision, 1);
            assert!(!found.truncated);
        }
    }
    let mut older = run("older-text", "codex", "main", 0);
    older.entries = vec![entry(1, RunEntryKind::Message, "Needle", 0)];
    write(&backend, &older);
    let found = search(&backend, &Filter::default(), None, "needle", false, 1).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, r.id);
    assert!(found.truncated);
    r.entries.extend((103..=4001).map(|n| {
        let mut e = entry(n, RunEntryKind::Message, "Tool finished", n as i64);
        e.tool = Some(capture.clone());
        e
    }));
    write(&backend, &r);
    let mut results = vec![search_runs(&backend, r#"c:\work\file"#).unwrap()];
    for id in [None, Some(r.id.as_str())] {
        results.push(
            search(
                &backend,
                &Filter::default(),
                id,
                r#"c:\work\file"#,
                false,
                50,
            )
            .unwrap(),
        );
    }
    for found in results {
        assert!(found.hits.is_empty());
        assert!(found.truncated);
    }
}

#[test]
fn distinct_run_filters_apply_before_limits_and_report_scan_ceiling() {
    let backend = backend();
    for n in 0..65 {
        let mut r = run(
            &format!("run-{n}"),
            if n == 64 { "codex" } else { "claude" },
            "main",
            n,
        );
        r.entries = vec![entry(1, RunEntryKind::Message, "needle", n)];
        write(&backend, &r);
    }
    let found = search_runs_filtered(&backend, "needle", |r| r.kind == "codex").unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, "run-64");
    assert!(!found.truncated);
    let mut r = run("verbose", "claude", "main", 999);
    r.entries = (1..=4100)
        .map(|n| entry(n, RunEntryKind::Message, "ceiling", n as i64))
        .collect();
    write(&backend, &r);
    let found = search_runs_filtered(&backend, "ceiling", |_| false).unwrap();
    assert!(found.hits.is_empty());
    assert!(found.truncated);
}

#[test]
fn distinct_runs_page_past_a_talkative_run_and_measure_overflow() {
    let backend = backend();
    let mut talkative = run("talkative", "claude", "main", 1);
    talkative.entries = (1..=600)
        .map(|rev| entry(rev, RunEntryKind::Message, "needle", rev as i64))
        .collect();
    write(&backend, &talkative);
    // Orphaned entry rows are matches, but cannot take a slot from a valid run.
    backend
        .apply(vec![old_entry_put(
            "missing",
            &entry(1, RunEntryKind::Message, "needle", 1),
        )])
        .unwrap();
    for n in 1..=50 {
        let mut other = run(&format!("other-{n}"), "claude", "main", n);
        other.entries = vec![entry(1, RunEntryKind::Message, "needle", n)];
        write(&backend, &other);
        if n == 49 || n == 50 {
            let found = search_runs(&backend, "needle").unwrap();
            assert_eq!(found.hits.len(), 50);
            assert_eq!(found.truncated, n == 50);
            assert_eq!(found.runs.len(), 50);
            assert_eq!(
                found
                    .hits
                    .iter()
                    .map(|hit| &hit.run_id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                50
            );
        }
    }
    let page = entry_page(
        &backend,
        "talkative",
        Some(anchored_from(10)),
        MAX_PAGE_ENTRIES,
    )
    .unwrap();
    assert_eq!(page.entries.first().unwrap().revision, 8);
    assert!(page.entries.iter().any(|entry| entry.revision == 10));
    assert_eq!(page.total, 600);
}

#[test]
fn distinct_run_search_uses_substrings_only_when_fts_starts_empty() {
    let backend = backend();
    for (id, text) in [
        ("exact", "needle"),
        ("substring", "needlesuffix"),
        ("punctuation", "---"),
    ] {
        let mut recorded = run(id, "claude", "main", 1);
        recorded.entries = (1..=100)
            .map(|rev| entry(rev, RunEntryKind::Message, text, rev as i64))
            .collect();
        write(&backend, &recorded);
    }
    for (query, expected) in [
        ("needle", "exact"),
        ("suffix", "substring"),
        ("---", "punctuation"),
    ] {
        let found = search_runs(&backend, query).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].run_id, expected);
        assert!(found.hits[0].snippet.contains(query));
        assert!(!found.truncated);
    }
}

#[test]
fn assistant_messages_are_searchable_and_pageable_after_archival_and_summary() {
    let backend = backend();
    let mut recorded = run("claude:messages", "claude", "main", 1);
    recorded.entries = vec![
        entry(1, RunEntryKind::Message, "commentary about the lexer", 1),
        entry(2, RunEntryKind::Result, "lexer verified", 2),
    ];
    recorded.revision = 2;
    recorded.summarized_revision = 2;
    recorded.outcome = RunOutcome::Completed;
    write(&backend, &recorded);
    let found = search(&backend, &Filter::default(), None, "lexer", false, 10).unwrap();
    assert_eq!(found.hits.len(), 2);
    assert_eq!(found.hits[1].kind, RunEntryKind::Message);
    let page = entry_page(&backend, &recorded.id, Some(1), 1).unwrap();
    assert_eq!(page.entries[0], recorded.entries[0]);
    let history = crate::agents::run_history(&recorded, 80, 100, 0);
    assert!(history
        .entries
        .iter()
        .any(|row| row.text.contains("commentary about the lexer")));
    assert!(history
        .entries
        .iter()
        .any(|row| row.text.contains("result: lexer")));
}

#[test]
fn an_entry_is_found_by_a_word_it_contains() {
    let backend = seeded();
    let found = search(&backend, &Filter::default(), None, "flaky", false, 10).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, "claude:one");
    assert_eq!(found.hits[0].revision, 1);
    assert_eq!(found.hits[0].kind, RunEntryKind::Prompt);
    assert!(found.runs.contains_key("claude:one"), "{:?}", found.runs);
}

/// The whole point of the machine-wide default: two sessions' runs, one search.
#[test]
fn a_search_reaches_every_session_by_default() {
    let backend = seeded();
    let found = search(&backend, &Filter::default(), None, "parser", false, 10).unwrap();
    let sessions: BTreeSet<&str> = found.hits.iter().map(|hit| hit.run_id.as_str()).collect();
    assert_eq!(
        sessions,
        BTreeSet::from(["claude:one", "codex:two"]),
        "{:?}",
        found.hits
    );
}

#[test]
fn a_search_is_narrowed_to_one_session() {
    let backend = seeded();
    let filter = Filter {
        yinz_session_name: Some("other".to_owned().into()),
        ..Filter::default()
    };
    let found = search(&backend, &filter, None, "parser", false, 10).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, "codex:two");
}

#[test]
fn a_search_is_narrowed_to_one_run() {
    let backend = seeded();
    let found = search(
        &backend,
        &Filter::default(),
        Some("codex:two"),
        "parser",
        false,
        10,
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, "codex:two");
}

#[test]
fn a_search_is_narrowed_by_project_root() {
    let backend = seeded();
    let filter = Filter {
        root: Some(RootScope::new("/work/agent", HostId::new(HOST))),
        ..Filter::default()
    };
    let found = search(&backend, &filter, None, "parser", false, 10).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].run_id, "claude:one");
}

/// A project is a folder on *this* machine, and on a shared store the same path on another
/// one is a different folder holding work nobody did here.
///
/// The header cannot answer this — it is JSON written when the run was recorded, and a run
/// older than the column has no machine in it — so the narrowing reads the row's own
/// `host_id` column. A row that names none is from before the column, and for a run `""`
/// reads as this machine.
#[test]
fn a_project_narrowing_does_not_reach_another_machines_runs() {
    let backend = backend();

    let mut mine = run("claude:mine", "claude", "main", 3_000);
    mine.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 3_000)];
    write(&backend, &mine);

    // The same path, on a machine sharing the store. Nothing about the run says "remote":
    // its pane was local to *that* machine, so `host` is empty exactly as mine is.
    let mut theirs = run("claude:theirs", "claude", "main", 4_000);
    theirs.host_id = HostId::new("another-machine");
    theirs.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 4_000)];
    write(&backend, &theirs);

    // And one from before the column, which there was only one machine to have written.
    let mut legacy = run("claude:legacy", "claude", "main", 2_000);
    legacy.host_id = HostId::default();
    legacy.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 2_000)];
    write(&backend, &legacy);

    let filter = Filter {
        root: Some(RootScope::new("/work/agent", HostId::new(HOST))),
        ..Filter::default()
    };
    let listed: BTreeSet<String> = list_runs(&backend, &filter, 10, 0)
        .unwrap()
        .headers
        .into_iter()
        .map(|header| header.id)
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from(["claude:mine".to_owned(), "claude:legacy".to_owned()]),
        "another machine's run is not this project's"
    );

    // A search narrows through the same pass, so it inherits the scoping.
    let found = search(&backend, &filter, None, "parser", false, 10).unwrap();
    let hits: BTreeSet<&str> = found.hits.iter().map(|hit| hit.run_id.as_str()).collect();
    assert_eq!(
        hits,
        BTreeSet::from(["claude:mine", "claude:legacy"]),
        "{:?}",
        found.hits
    );

    // Unnarrowed, every machine's history is the answer — which is the whole point of a
    // shared store, and what "have I had an agent look at this before" is really asking.
    let every = search(&backend, &Filter::default(), None, "parser", false, 10).unwrap();
    assert_eq!(every.hits.len(), 3, "{:?}", every.hits);
}

/// `this_session` has to reach the runs recorded before the `host_id` column existed.
///
/// Every one of them carries the migration's empty default, and a closed run is never
/// rewritten, so nothing will ever fill it in. Read strictly, the tool that narrows to this
/// session answers "no runs" for a session with years of them, while the unnarrowed search
/// still finds them — which reads as a bug in the history, not in the filter.
///
/// The session half is what makes the tolerance safe, and is `rows::owned_or_unclaimed`'s
/// bargain: a session name only means anything within one machine.
#[test]
fn narrowing_to_this_session_reaches_the_runs_that_predate_the_host_column() {
    let backend = backend();

    let mut mine = run("claude:mine", "claude", "main", 3_000);
    mine.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 3_000)];
    write(&backend, &mine);

    let mut legacy = run("claude:legacy", "claude", "main", 2_000);
    legacy.host_id = HostId::default();
    legacy.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 2_000)];
    write(&backend, &legacy);

    // Another machine's session, sharing both the store and the name `main`.
    let mut theirs = run("claude:theirs", "claude", "main", 4_000);
    theirs.host_id = HostId::new("another-machine");
    theirs.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 4_000)];
    write(&backend, &theirs);

    // A run of this machine's, in a different session.
    let mut elsewhere = run("claude:elsewhere", "claude", "other", 5_000);
    elsewhere.entries = vec![entry(1, RunEntryKind::Prompt, "fix the parser", 5_000)];
    write(&backend, &elsewhere);

    let this_session = Filter {
        yinz_session_name: Some("main".to_owned().into()),
        host_id: Some(HostId::new(HOST)),
        ..Filter::default()
    };
    assert_eq!(
        listed(&backend, &this_session),
        BTreeSet::from(["claude:mine".to_owned(), "claude:legacy".to_owned()]),
        "an unclaimed run is this session's; another machine's `main` is not"
    );

    // A search narrows through the same predicate.
    let found = search(&backend, &this_session, None, "parser", false, 10).unwrap();
    let hits: BTreeSet<&str> = found.hits.iter().map(|hit| hit.run_id.as_str()).collect();
    assert_eq!(
        hits,
        BTreeSet::from(["claude:mine", "claude:legacy"]),
        "{:?}",
        found.hits
    );

    // Asked about a machine on its own, with no session to pair it with, the question is
    // about that machine — and a row naming none cannot answer it. Tolerating `""` here
    // would hand every machine's oldest runs to whoever asked first.
    let host_alone = Filter {
        host_id: Some(HostId::new("another-machine")),
        ..Filter::default()
    };
    assert_eq!(
        listed(&backend, &host_alone),
        BTreeSet::from(["claude:theirs".to_owned()])
    );
}

/// The ids `list_runs` gives back for a filter, as a set.
fn listed(backend: &impl Backend, filter: &Filter) -> BTreeSet<String> {
    list_runs(backend, filter, 10, 0)
        .unwrap()
        .headers
        .into_iter()
        .map(|header| header.id)
        .collect()
}

/// The substring fallback: FTS5 tokenizes on word boundaries, so a
/// fragment matches nothing in the index and everything in a substring pass.
#[test]
fn a_fragment_falls_back_to_a_substring_pass() {
    let backend = seeded();
    let found = search(&backend, &Filter::default(), None, "snapsh", false, 10).unwrap();
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.hits[0].revision, 2);
}

/// The migration test. Entries written before `text` was `full_text` have to become
/// searchable when the index is declared, and they do because `rebuild_full_text` ends by
/// rebuilding the index from the content table. Nothing re-writes the rows.
#[test]
fn entries_written_before_the_index_existed_are_searchable() {
    let backend = store::Local::shared("agents");
    // The v1 definition, exactly: no `full_text` on `text`, no indexes on `kind` or
    // `created_at_ms`.
    let before = EntityDef::new("agent_run_entry", 1)
        .attr(
            yinz_tile::prelude::AttrDef::text("run_id")
                .indexed()
                .required(),
        )
        .attr(yinz_tile::prelude::AttrDef::int("revision").required())
        .attr(yinz_tile::prelude::AttrDef::text("event_id"))
        .attr(yinz_tile::prelude::AttrDef::text("kind").required())
        .attr(yinz_tile::prelude::AttrDef::text("text").required())
        .attr(yinz_tile::prelude::AttrDef::int("created_at_ms"));
    backend.define(&before).expect("the old definition");
    // Hand-written rather than through `rows::entry_mutation`, which sets columns this
    // definition does not have: the point is a row as the old schema really held it.
    backend
        .apply(vec![old_entry_put(
            "claude:old",
            &entry(1, RunEntryKind::Prompt, "an archaeological finding", 1),
        )])
        .expect("wrote an entry under the old schema");

    // Now the current definitions, which add the index.
    backend
        .define_all(&shared_defs())
        .expect("migrated to the indexed definition");

    let found = search(
        &backend,
        &Filter::default(),
        None,
        "archaeological",
        false,
        10,
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    assert_eq!(found.hits[0].run_id, "claude:old");
}

/// The realistic upgrade: whichever plugin declared the entity first owns it, and the one
/// that arrives with the indexed definition is usually a different one. A migration by a
/// foreign owner is refused only when it is *destructive*, and this crate never asks for
/// that — but the rule is worth a test rather than an argument, since getting it wrong
/// means a plugin that fails to load.
#[test]
fn a_second_plugin_may_add_the_index_to_an_entity_it_does_not_own() {
    let owner = store::Local::shared("project-store");
    let before = EntityDef::new("agent_run_entry", 1)
        .attr(
            yinz_tile::prelude::AttrDef::text("run_id")
                .indexed()
                .required(),
        )
        .attr(yinz_tile::prelude::AttrDef::int("revision").required())
        .attr(yinz_tile::prelude::AttrDef::text("event_id"))
        .attr(yinz_tile::prelude::AttrDef::text("kind").required())
        .attr(yinz_tile::prelude::AttrDef::text("text").required())
        .attr(yinz_tile::prelude::AttrDef::int("created_at_ms"));
    owner.define(&before).expect("the old definition");
    // The run the entry belongs to, so annotating a hit has somewhere to look.
    owner
        .define(&crate::store::agent_run_def())
        .expect("the run entity");
    owner
        .apply(vec![old_entry_put(
            "claude:old",
            &entry(1, RunEntryKind::Prompt, "a foreign finding", 1),
        )])
        .expect("wrote an entry");

    let other = owner.joined(store::SHARED, "agents");
    let info = other
        .define(&crate::store::agent_run_entry_def())
        .expect("a foreign owner may add an index");
    assert!(info.full_text_rebuilt, "the index was built");

    let found = search(&other, &Filter::default(), None, "foreign", false, 10).unwrap();
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
}

#[test]
fn an_empty_query_finds_nothing_rather_than_everything() {
    let backend = seeded();
    let found = search(&backend, &Filter::default(), None, "   ", false, 10).unwrap();
    assert!(found.hits.is_empty());
}

// -- listing --------------------------------------------------------

#[test]
fn runs_are_listed_newest_first_across_sessions() {
    let backend = seeded();
    let page = list_runs(&backend, &Filter::default(), 10, 0).unwrap();
    assert_eq!(
        page.headers
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        ["codex:two", "claude:one"],
    );
    assert!(!page.more);
}

#[test]
fn a_listing_is_narrowed_by_kind_session_and_window() {
    let backend = seeded();
    let by_kind = Filter {
        kind: Some("codex".to_owned()),
        ..Filter::default()
    };
    let by_kind = list_runs(&backend, &by_kind, 10, 0).unwrap().headers;
    assert_eq!(by_kind.len(), 1);
    assert_eq!(by_kind[0].kind, "codex");

    let by_session = Filter {
        yinz_session_name: Some("main".to_owned().into()),
        ..Filter::default()
    };
    let by_session = list_runs(&backend, &by_session, 10, 0).unwrap().headers;
    assert_eq!(by_session.len(), 1);
    assert_eq!(by_session[0].id, "claude:one");

    let recent = Filter {
        since_ms: Some(1_500),
        ..Filter::default()
    };
    let recent = list_runs(&backend, &recent, 10, 0).unwrap().headers;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, "codex:two");
}

#[test]
fn a_listing_reports_that_more_follow_it() {
    let backend = seeded();
    let first = list_runs(&backend, &Filter::default(), 1, 0).unwrap();
    assert_eq!(first.headers.len(), 1);
    assert!(first.more);
    // The next page starts on the row this one stopped at, and an unnarrowed page
    // consumes exactly what it kept.
    assert_eq!(first.scanned, 1);
    let second = list_runs(&backend, &Filter::default(), 1, first.scanned).unwrap();
    assert_eq!(second.headers[0].id, "claude:one");
    assert!(!second.more);
}

/// Paging a narrowed listing has to advance by the rows it *scanned*, not the runs it
/// kept, or every page starts a few rows in and hands back the same runs for ever.
#[test]
fn a_narrowed_listing_pages_forward_instead_of_repeating_itself() {
    let backend = backend();
    // Thirty runs, three of them in the project, spread so that no single scan can reach
    // them all: at limit 1 the over-fetch is eight rows.
    for index in 0..30u64 {
        let mut run = run(
            &format!("claude:{index}"),
            "claude",
            "main",
            1_000 - index as i64,
        );
        run.cwd = Some(match index % 10 {
            0 => "/work/agent".to_owned(),
            _ => "/work/other".to_owned(),
        });
        write(&backend, &run);
    }
    let filter = Filter {
        root: Some(RootScope::new("/work/agent", HostId::new(HOST))),
        ..Filter::default()
    };
    let mut seen: Vec<String> = Vec::new();
    let mut offset = 0usize;
    // Bounded so a page that failed to advance fails the test rather than hanging it.
    for _ in 0..10 {
        let page = list_runs(&backend, &filter, 1, offset).unwrap();
        seen.extend(page.headers.iter().map(|header| header.id.clone()));
        if !page.more {
            break;
        }
        offset = offset.saturating_add(page.scanned);
    }
    assert_eq!(seen, ["claude:0", "claude:10", "claude:20"]);
}

/// A narrowed search resolves its scope a page at a time, so a match in a run older than
/// the newest [`MAX_RUNS`] is still found. Answering "no" from the newest fifty is the one
/// wrong answer this module exists to avoid.
#[test]
fn a_narrowed_search_reaches_past_the_newest_page_of_runs() {
    let backend = backend();
    for index in 0..(MAX_RUNS as u64 + 10) {
        let mut run = run(
            &format!("claude:{index}"),
            "claude",
            "main",
            1_000 - index as i64,
        );
        run.entries = vec![entry(
            1,
            RunEntryKind::Prompt,
            match index {
                // The oldest run, well past the first page of the listing.
                59 => "teach the tokenizer about zalgo",
                _ => "something else entirely",
            },
            1_000,
        )];
        write(&backend, &run);
    }
    let by_kind = Filter {
        kind: Some("claude".to_owned()),
        ..Filter::default()
    };
    let found = search(&backend, &by_kind, None, "zalgo", false, 10).unwrap();
    assert_eq!(
        found
            .hits
            .iter()
            .map(|hit| hit.run_id.as_str())
            .collect::<Vec<_>>(),
        ["claude:59"],
    );
    assert!(!found.truncated);
}

/// A run on another machine is not this project's work, however its folder is spelled.
#[test]
fn a_project_filter_does_not_claim_a_run_from_another_machine() {
    let backend = backend();
    let here = run("claude:here", "claude", "main", 1_000);
    let mut there = run("claude:there", "claude", "main", 2_000);
    there.host = Some("build-box".to_owned());
    write(&backend, &here);
    write(&backend, &there);
    let filter = Filter {
        root: Some(RootScope::new("/work/agent", HostId::new(HOST))),
        ..Filter::default()
    };
    let page = list_runs(&backend, &filter, 10, 0).unwrap();
    assert_eq!(
        page.headers
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        ["claude:here"],
    );
}

/// A prefix on path boundaries: a sibling folder whose name starts with the root is not
/// inside it.
#[test]
fn a_project_root_does_not_claim_a_sibling_that_shares_its_name() {
    let here = RootScope::new("/work/agent", HostId::new(HOST));
    assert!(matches_root(
        Some("/work/agent"),
        None,
        &HostId::new(HOST),
        &here
    ));
    assert!(matches_root(
        Some("/work/agent/src"),
        None,
        &HostId::new(HOST),
        &here
    ));
    assert!(matches_root(
        Some("/work/agent/"),
        None,
        &HostId::new(HOST),
        &here
    ));
    assert!(!matches_root(
        Some("/work/agent-fork"),
        None,
        &HostId::new(HOST),
        &here
    ));
    assert!(!matches_root(None, None, &HostId::new(HOST), &here));
    // The same path over ssh is another folder, not this project.
    assert!(!matches_root(
        Some("/work/agent"),
        Some("build-box"),
        &HostId::new(HOST),
        &here
    ));
    // And so is the same path on another machine sharing the store, where the pane is
    // local to *that* machine and says nothing about being remote.
    assert!(!matches_root(
        Some("/work/agent"),
        None,
        &HostId::new("another-machine"),
        &here
    ));
    // A run from before the column names no machine, and there was only one machine then.
    assert!(matches_root(
        Some("/work/agent"),
        None,
        &HostId::new(""),
        &here
    ));
}

// -- paging one run -------------------------------------------------

#[test]
fn search_detail_preserves_the_full_match_and_surrounding_context() {
    for length in [ENTRY_TEXT_CHARS * 2, PAGE_TEXT_BUDGET * 2] {
        let backend = backend();
        let mut recorded = run("claude:match", "claude", "main", 1);
        let text = format!("{} needle near the end", "x".repeat(length));
        recorded.entries = (1..=6)
            .map(|rev| entry(rev, RunEntryKind::Message, "context", rev as i64))
            .collect();
        recorded.entries[3].text = text.clone();
        write(&backend, &recorded);

        let found = search_runs(&backend, "needle").unwrap();
        let hit = &found.hits[0];
        assert_eq!(hit.revision, 4);
        assert!(hit.snippet.contains("needle"));
        let mut expected = entry_page(
            &backend,
            &recorded.id,
            Some(anchored_from(hit.revision)),
            MAX_PAGE_ENTRIES,
        )
        .unwrap();
        assert!(!expected.entries[2].text.contains("needle"));
        expected.entries[2] = recorded.entries[3].clone();
        expected.truncated_revisions.retain(|r| *r != hit.revision);

        let detail = search_detail(&backend, &hit.run_id, hit.revision).unwrap();
        assert_eq!(detail.entries[2].text, text);
        assert_eq!(detail.entries, recorded.entries[1..]);
        assert_eq!(detail, expected);
    }
}

#[test]
fn search_detail_does_not_substitute_a_later_revision_for_a_missing_match() {
    let backend = backend();
    let mut recorded = run("claude:gap", "claude", "main", 1);
    recorded.entries = vec![
        entry(1, RunEntryKind::Prompt, "context", 1),
        entry(
            3,
            RunEntryKind::Message,
            &"x".repeat(ENTRY_TEXT_CHARS * 2),
            3,
        ),
    ];
    write(&backend, &recorded);
    for revision in [2, 4] {
        let expected = entry_page(
            &backend,
            &recorded.id,
            Some(anchored_from(revision)),
            MAX_PAGE_ENTRIES,
        )
        .unwrap();
        assert_eq!(
            search_detail(&backend, &recorded.id, revision).unwrap(),
            expected
        );
    }
}

#[test]
fn an_absent_start_reads_the_newest_page() {
    let backend = seeded();
    let page = entry_page(&backend, "claude:one", None, 2).unwrap();
    assert_eq!(
        page.entries.iter().map(|e| e.revision).collect::<Vec<_>>(),
        [2, 3],
    );
    assert_eq!(page.total, 3);
    assert!(page.earlier, "revision 1 was left unread");
    assert_eq!(page.next_revision, None, "the page reached the end");
}

#[test]
fn a_page_walks_forward_and_covers_every_entry_once() {
    let backend = backend();
    let mut long = run("claude:long", "claude", "main", 1);
    long.entries = (1..=10)
        .map(|n| entry(n, RunEntryKind::Prompt, &format!("entry {n}"), n as i64))
        .collect();
    write(&backend, &long);

    let mut seen = Vec::new();
    let mut from = Some(1);
    while let Some(start) = from {
        let page = entry_page(&backend, "claude:long", Some(start), 3).unwrap();
        assert_eq!(page.total, 10);
        seen.extend(page.entries.iter().map(|e| e.revision));
        from = page.next_revision;
    }
    assert_eq!(seen, (1..=10).collect::<Vec<_>>());
}

/// "The page was as long as you allowed" is not "there is more". A run read in one exact
/// page has nothing after it, and must not advertise a page that is not there.
#[test]
fn a_page_that_exactly_finishes_a_run_offers_no_next_page() {
    let backend = seeded();
    let page = entry_page(&backend, "claude:one", Some(1), 3).unwrap();
    assert_eq!(page.entries.len(), 3);
    assert_eq!(page.total, 3);
    assert_eq!(page.next_revision, None);
    assert!(!page.earlier);
}

#[test]
fn an_anchored_page_starts_before_the_hit_it_is_read_around() {
    assert_eq!(anchored_from(87), 85);
    // Never below the first revision, which is 1.
    assert_eq!(anchored_from(2), 1);
    assert_eq!(anchored_from(1), 1);
}

#[test]
fn a_page_of_many_entries_is_cut_to_the_budget() {
    let backend = backend();
    let mut heavy = run("claude:heavy", "claude", "main", 1);
    let text = "x".repeat(ENTRY_TEXT_CHARS * 2);
    heavy.entries = (1..=40)
        .map(|n| entry(n, RunEntryKind::Result, &text, n as i64))
        .collect();
    write(&backend, &heavy);

    let page = entry_page(&backend, "claude:heavy", Some(1), 40).unwrap();
    let spent: usize = page.entries.iter().map(|e| e.text.len()).sum();
    assert!(spent <= PAGE_TEXT_BUDGET, "{spent} bytes");
    assert!(page.entries.len() < 40, "the page was cut short");
    assert_eq!(
        page.truncated_revisions,
        page.entries.iter().map(|e| e.revision).collect::<Vec<_>>()
    );
    assert!(page.next_revision.is_some(), "and says where to resume");
    // Each entry was cut to the per-entry cap, since the page holds more than one.
    assert!(page
        .entries
        .iter()
        .all(|e| e.text.chars().count() <= ENTRY_TEXT_CHARS + 1));
}

/// The escape hatch: a page of one carries that entry whole, so nothing in the history is
/// unreachable however long it is.
/// A page read backward from the end keeps the *newest* entries, so what the
/// budget dropped is behind it, not ahead — `earlier` says so and
/// `next_revision` must stay empty. Pointing it past the last revision would
/// send a reader to a page that does not exist.
#[test]
fn a_budgeted_newest_page_points_backward_not_past_the_end() {
    let backend = backend();
    let mut heavy = run("claude:tail", "claude", "main", 1);
    let text = "z".repeat(ENTRY_TEXT_CHARS * 2);
    heavy.entries = (1..=40)
        .map(|n| entry(n, RunEntryKind::Result, &text, n as i64))
        .collect();
    write(&backend, &heavy);

    let page = entry_page(&backend, "claude:tail", None, 40).unwrap();
    assert!(page.entries.len() < 40, "the page was budgeted");
    assert_eq!(
        page.entries.last().map(|e| e.revision),
        Some(40),
        "and kept the newest end"
    );
    assert!(page.earlier, "what it dropped is behind it");
    assert_eq!(page.next_revision, None, "there is nothing ahead of it");
}

#[test]
fn a_page_of_one_entry_carries_it_whole() {
    let backend = backend();
    let mut heavy = run("claude:one-big", "claude", "main", 1);
    let text = "y".repeat(ENTRY_TEXT_CHARS * 3);
    heavy.entries = vec![entry(1, RunEntryKind::Plan, &text, 1)];
    write(&backend, &heavy);

    let page = entry_page(&backend, "claude:one-big", Some(1), 1).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].text.chars().count(), ENTRY_TEXT_CHARS * 3);
    assert!(page.truncated_revisions.is_empty());
}

#[test]
fn an_unknown_run_pages_to_nothing_rather_than_failing() {
    let backend = seeded();
    let page = entry_page(&backend, "claude:missing", None, 10).unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.total, 0);
    assert_eq!(page.next_revision, None);
    assert!(run_header(&backend, "claude:missing").unwrap().is_none());
}

/// A page that starts past the end of a run has the whole run behind it. Saying otherwise
/// tells a reader who followed a stale `next_revision` that the run it can see the entry
/// count of holds nothing.
#[test]
fn a_page_past_the_end_of_a_run_says_what_is_behind_it() {
    let backend = seeded();
    let page = entry_page(&backend, "claude:one", Some(999), 10).unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.total, 3);
    assert!(page.earlier);
    assert_eq!(page.next_revision, None);
}

#[test]
fn a_header_comes_back_for_a_run_that_exists() {
    let backend = seeded();
    let header = run_header(&backend, "claude:one").unwrap().unwrap();
    assert_eq!(header.kind, "claude");
    assert_eq!(header.cwd.as_deref(), Some("/work/agent"));
}

// -- snippets -------------------------------------------------------

#[test]
fn a_snippet_is_centred_on_the_match_rather_than_the_head() {
    let lead = "a ".repeat(400);
    let text = format!("{lead}needle {}", "b ".repeat(400));
    let snippet = snippet(&text, "needle");
    assert!(snippet.contains("needle"), "{snippet}");
    assert!(snippet.starts_with('…'), "{snippet}");
    assert!(snippet.chars().count() <= SNIPPET_CHARS + 2);
}

/// The match is found at an index into the *original* text: folding through
/// `str::to_lowercase` is not length-preserving — `İ` becomes two characters — and a
/// window placed with a shifted index misses the hit it was centred on.
#[test]
fn a_snippet_is_centred_on_the_match_through_text_that_lowercases_longer() {
    let text = format!(
        "{}{}needle {}",
        "İ".repeat(80),
        "a ".repeat(15),
        "b ".repeat(400)
    );
    let snippet = snippet(&text, "needle");
    assert!(snippet.contains("needle"), "{snippet}");
}

#[test]
fn a_snippet_falls_back_to_the_head_when_the_query_is_not_literal() {
    let text = "c ".repeat(400);
    let snippet = snippet(&text, "stemmed");
    assert!(!snippet.starts_with('…'), "{snippet}");
    assert!(snippet.starts_with('c'), "{snippet}");
}

#[test]
fn a_short_entry_is_its_own_snippet() {
    assert_eq!(snippet("just  this", "this"), "just this");
}

#[test]
fn a_snippet_matches_a_multi_word_query_by_its_first_word() {
    let lead = "d ".repeat(400);
    let text = format!("{lead}parser {}", "e ".repeat(400));
    let snippet = snippet(&text, "parser rewrite");
    assert!(snippet.contains("parser"), "{snippet}");
}

#[test]
fn search_run_pages_reach_all_matches_and_full_text() {
    let backend = backend();
    let mut recorded = run("claude:paged", "claude", "main", 1);
    recorded.entries = (1..=95)
        .map(|revision| {
            entry(
                revision,
                RunEntryKind::Message,
                &format!("{} needle", "x ".repeat(ENTRY_TEXT_CHARS)),
                revision as i64,
            )
        })
        .collect();
    write(&backend, &recorded);
    let mut from = 1;
    let mut revisions = std::collections::BTreeSet::new();
    loop {
        let (hits, next) = search_run_page(&backend, &recorded.id, "needle", from).unwrap();
        for hit in hits {
            assert!(hit.snippet.contains("needle"));
            assert!(revisions.insert(hit.revision));
        }
        let Some(next) = next else { break };
        assert!(next > from);
        from = next;
    }
    assert_eq!(
        revisions.into_iter().collect::<Vec<_>>(),
        (1..=95).collect::<Vec<_>>()
    );
}
