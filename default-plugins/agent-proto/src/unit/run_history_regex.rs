use super::tests::{backend, entry, run, seeded, write};
use super::*;

fn archive(texts: &[&str]) -> store::Local {
    let backend = backend();
    let mut run = run("regex", "codex", "main", 1);
    run.entries = texts
        .iter()
        .enumerate()
        .map(|(i, text)| entry(i as u64 + 1, RunEntryKind::Result, text, i as i64))
        .collect();
    write(&backend, &run);
    backend
}

#[test]
fn regex_syntax_preserves_complete_original_entries() {
    let backend = archive(&[" ERROR ", "café (a.b)", "first\nsecond", "error"]);
    for (pattern, expected) in [
        ("panic|ERROR", 1),
        (r"\(a\.b\)", 1),
        ("café", 1),
        ("(?i)error", 2),
        ("(?m)^second$", 1),
        ("(?s)first.second", 1),
        (" ERROR ", 1),
        (" ", 2),
        ("^", 4),
        ("^second$", 0),
        ("ERROR.*café", 0),
    ] {
        let found = search(&backend, &Filter::default(), None, pattern, true, 50).unwrap();
        assert_eq!(found.hits.len(), expected, "{pattern}");
        assert!(!found.truncated, "{pattern}");
    }
}

#[test]
fn regex_validation_precedes_even_empty_scope_resolution() {
    let backend = backend();
    for pattern in [
        "",
        "\0",
        "[",
        "(?=a)",
        r"(a)\1",
        "a{1000000}",
        &"x".repeat(4097),
    ] {
        assert!(
            search(
                &backend,
                &Filter {
                    kind: Some("missing".into()),
                    ..Filter::default()
                },
                None,
                pattern,
                true,
                10
            )
            .is_err(),
            "{pattern}"
        );
    }
    assert!(search(
        &backend,
        &Filter::default(),
        None,
        &"x".repeat(4096),
        true,
        10
    )
    .is_ok());
}

#[test]
fn regex_pages_sparse_matches_and_reports_excess_hits() {
    let mut texts = vec!["nothing"; 205];
    texts[0] = "match";
    texts[103] = "match";
    let backend = archive(&texts);
    let exact = search(&backend, &Filter::default(), None, "match", true, 2).unwrap();
    assert_eq!(
        exact.hits.iter().map(|h| h.revision).collect::<Vec<_>>(),
        [104, 1]
    );
    assert!(!exact.truncated);
    let excess = search(&backend, &Filter::default(), None, "match", true, 1).unwrap();
    assert_eq!(excess.hits[0].revision, 104);
    assert!(excess.truncated);
}

#[test]
fn regex_budgets_include_nonmatches_and_never_match_partial_entries() {
    let backend = archive(&vec!["nothing"; REGEX_SCAN_ROWS + 1]);
    assert!(
        search(&backend, &Filter::default(), None, "match", true, 1)
            .unwrap()
            .truncated
    );
    let oversized = format!("match{}", "x".repeat(REGEX_SCAN_BYTES));
    let backend = archive(&[&oversized]);
    let found = search(&backend, &Filter::default(), None, "match", true, 1).unwrap();
    assert!(found.truncated);
    assert!(found.hits.is_empty());
    // Each page holds only one of these rows, despite a requested limit of 100.
    let heavy = "x".repeat(1100 * 1024);
    let backend = archive(&["match", &heavy, &heavy, &heavy]);
    let found = search(&backend, &Filter::default(), None, "match", true, 1).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(!found.truncated);
    let backend = archive(&[
        "match", &heavy, &heavy, &heavy, &heavy, &heavy, &heavy, &heavy, &heavy,
    ]);
    let found = search(&backend, &Filter::default(), None, "match", true, 1).unwrap();
    assert!(found.hits.is_empty());
    assert!(found.truncated);
}

#[test]
fn regex_and_keyword_exact_run_search_keep_the_host_scope() {
    let backend = seeded();
    let filter = Filter {
        host_id: Some(HostId::new("unrelated")),
        ..Filter::default()
    };
    for regex in [false, true] {
        for run_id in [None, Some("codex:two")] {
            assert!(search(&backend, &filter, run_id, "parser", regex, 10)
                .unwrap()
                .hits
                .is_empty());
        }
    }
}

#[test]
fn regex_snippets_center_on_unicode_match_before_collapsing_whitespace() {
    let text = format!("{}\nneedle\t{}", "界 ".repeat(300), "é ".repeat(300));
    let snippet = regex_snippet(&text, text.find("needle").unwrap());
    assert!(snippet.contains("needle"));
    assert!(snippet.starts_with('…') && snippet.ends_with('…'));
    assert!(!snippet.contains(['\n', '\t']));
    assert!(snippet.chars().count() <= SNIPPET_CHARS + 2);
    assert_eq!(regex_snippet("é", 2), "é");
}

#[test]
fn regex_counts_unrecognized_entries_and_exact_scan_bound_is_complete() {
    use yinz_tile::prelude::Mutation;
    let backend = backend();
    // Unknown kinds still consume the row budget even though they cannot become hits.
    for chunk in (0..REGEX_SCAN_ROWS).collect::<Vec<_>>().chunks(100) {
        backend
            .apply(
                chunk
                    .iter()
                    .map(|i| {
                        Mutation::put(rows::ENTRY_ENTITY)
                            .uid(format!("unknown-{i}"))
                            .set("run_id", "unknown")
                            .set("revision", *i as i64)
                            .set("kind", "future-kind")
                            .set("text", "match")
                            .build()
                    })
                    .collect(),
            )
            .unwrap();
    }
    let exact = search(&backend, &Filter::default(), None, "match", true, 1).unwrap();
    assert!(exact.hits.is_empty());
    assert!(!exact.truncated);
    backend
        .apply(vec![Mutation::put(rows::ENTRY_ENTITY)
            .uid("one-more")
            .set("run_id", "unknown")
            .set("revision", 4001i64)
            .set("kind", "future-kind")
            .set("text", "match")
            .build()])
        .unwrap();
    assert!(
        search(&backend, &Filter::default(), None, "match", true, 1)
            .unwrap()
            .truncated
    );
}

#[test]
fn regex_scope_respects_ownership_and_reports_incomplete_run_resolution() {
    let backend = backend();
    for (id, host) in [("local", "host-a"), ("legacy", ""), ("foreign", "host-b")] {
        let mut run = run(id, "codex", "main", 1);
        run.host_id = HostId::new(host);
        run.cwd = Some("/work/project".into());
        run.entries = vec![entry(1, RunEntryKind::Result, "match", 1)];
        write(&backend, &run);
    }
    for filter in [
        Filter {
            host_id: Some(HostId::new("host-a")),
            yinz_session_name: Some(SessionName::new("main")),
            ..Filter::default()
        },
        Filter {
            root: Some(RootScope::new("/work/project", HostId::new("host-a"))),
            ..Filter::default()
        },
    ] {
        let found = search(&backend, &filter, None, "match", true, 10).unwrap();
        assert_eq!(
            found.runs.keys().map(String::as_str).collect::<Vec<_>>(),
            ["legacy", "local"]
        );
        assert!(!found.truncated);
    }
    for i in 0..MAX_SCOPE_RUNS {
        write(
            &backend,
            &run(&format!("scope-{i}"), "codex", "scope", i as i64),
        );
    }
    let found = search(
        &backend,
        &Filter {
            yinz_session_name: Some(SessionName::new("scope")),
            ..Filter::default()
        },
        None,
        "match",
        true,
        10,
    )
    .unwrap();
    assert!(found.hits.is_empty());
    assert!(found.truncated);
}
