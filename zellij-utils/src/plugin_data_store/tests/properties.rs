use super::*;
use hegel::generators::{self, Generator};

/// A string that exercises the FTS5 parser: bare words, the operators it recognises, and the
/// punctuation and unbalanced quotes that make it a syntax error.
#[hegel::composite]
fn search_string(tc: &hegel::TestCase) -> String {
    let piece_count = tc.draw(generators::integers::<usize>().max_value(6));
    let mut pieces: Vec<String> = Vec::new();
    for _ in 0..piece_count {
        pieces.push(tc.draw(hegel::one_of!(
            generators::text().max_size(8),
            generators::sampled_from(vec![
                "AND", "OR", "NOT", "NEAR", "\"", "'", "(", ")", "*", "^", ":", "-", "--",
                "a b", "",
            ])
            .map(str::to_owned),
        )));
    }
    pieces.join(" ")
}

/// Anything a person can paste has to search rather than error — the same property the
/// recording store holds, for the same reason: punctuation and unbalanced quotes are FTS5
/// syntax errors, and are also exactly what comes off a log line.
#[hegel::test(test_cases = 40)]
fn any_search_string_searches_rather_than_erroring(tc: hegel::TestCase) {
    let query = tc.draw(search_string());
    let (store, _dir) = defined();
    put(&store, "zellij", "fixing a rebase conflict", 1);
    let result = store.fetch(NS, &FetchRequest::entity("agent").search(&query));
    assert!(
        result.is_ok(),
        "searching for {:?} failed: {:?}",
        query,
        result.err()
    );
}

/// Declaring an index is a statement about how rows are reached, never about which rows come
/// back. This is the safety net under the adaptive indexing that comes later.
#[hegel::test(test_cases = 25)]
fn declaring_an_index_changes_the_plan_but_never_the_results(tc: hegel::TestCase) {
    let projects: Vec<String> = tc.draw(
        generators::vecs(generators::sampled_from(vec!["a", "b", "c"]).map(str::to_owned))
            .min_size(1)
            .max_size(12),
    );
    let needle = tc.draw(generators::sampled_from(vec!["a", "b", "c"]).map(str::to_owned));

    let results = |indexed: bool| {
        let (store, _dir) = store();
        let attr = if indexed {
            AttrDef::text("project").indexed()
        } else {
            AttrDef::text("project")
        };
        let def = EntityDef::new("agent", 1)
            .attr(attr)
            .attr(AttrDef::int("n"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        for (i, project) in projects.iter().enumerate() {
            store
                .apply(
                    NS,
                    &[Mutation::put("agent")
                        .set("project", project.as_str())
                        .set("n", i as i64)
                        .build()],
                )
                .unwrap();
        }
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .filter(Pred::eq("project", needle.as_str()))
                    .select(["n"]),
            )
            .unwrap();
        column(&rows, "n")
    };

    assert_eq!(results(true), results(false));
}

/// Walking a set by cursor has to see every row exactly once. Anything else and a plugin paging
/// through its own data silently loses or repeats rows.
#[hegel::test(test_cases = 25)]
fn paging_by_cursor_yields_every_row_exactly_once(tc: hegel::TestCase) {
    let row_count = tc.draw(generators::integers::<usize>().min_value(1).max_value(25));
    let page_size = tc.draw(generators::integers::<u32>().min_value(1).max_value(6));

    let (store, _dir) = defined();
    for i in 0..row_count {
        put(&store, "zellij", &format!("s{}", i), i as i64);
    }

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<i64> = None;
    // one iteration per row plus a margin, so that a bug which fails to advance terminates
    // instead of hanging
    for _ in 0..row_count + 5 {
        let mut request = FetchRequest::entity("agent").limit(page_size);
        if let Some(after) = cursor {
            request = request.after_id(after);
        }
        let rows = store.fetch(NS, &request).unwrap();
        seen.extend(texts(&rows, "summary"));
        match rows.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    let expected: Vec<String> = (0..row_count).map(|i| format!("s{}", i)).collect();
    assert_eq!(seen, expected);
}

/// What comes out of a put is what went in, whatever the mix of kinds.
///
/// NUL is left out of the text because the store refuses it in a value — Postgres cannot hold
/// one in `TEXT`, and a plugin's data should mean the same on every engine. The refusal has a
/// test of its own.
#[hegel::test(test_cases = 25)]
fn a_put_round_trips_through_a_fetch(tc: hegel::TestCase) {
    let written: Vec<(String, i64, bool, Vec<u8>)> = tc.draw(
        generators::vecs(hegel::tuples!(
            generators::text().max_size(20).exclude_characters("\0"),
            generators::integers::<i64>(),
            generators::booleans(),
            generators::binary().max_size(10),
        ))
        .min_size(1)
        .max_size(10),
    );

    let (store, _dir) = store();
    let def = EntityDef::new("mixed", 1)
        .attr(AttrDef::text("t"))
        .attr(AttrDef::int("i"))
        .attr(AttrDef::boolean("b"))
        .attr(AttrDef::blob("raw"));
    store.define_entity(NS, OWNER, &def, false).unwrap();

    for (t, i, b, raw) in &written {
        store
            .apply(
                NS,
                &[Mutation::put("mixed")
                    .set("t", t.clone())
                    .set("i", *i)
                    .set("b", *b)
                    .set("raw", raw.clone())
                    .build()],
            )
            .unwrap();
    }

    let fetched = store
        .fetch(NS, &FetchRequest::entity("mixed").limit(MAX_FETCH_ROWS))
        .unwrap();
    assert_eq!(fetched.len(), written.len());
    for (index, (t, i, b, raw)) in written.iter().enumerate() {
        let row = &fetched.rows[index];
        assert_eq!(fetched.get(row, "t"), Some(&DataValue::Text(t.clone())));
        assert_eq!(fetched.get(row, "i"), Some(&DataValue::Int(*i)));
        assert_eq!(fetched.get(row, "b"), Some(&DataValue::Bool(*b)));
        assert_eq!(fetched.get(row, "raw"), Some(&DataValue::Blob(raw.clone())));
    }
}
