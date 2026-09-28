use super::*;
use hegel::generators::{self, Generator};
use yinz_types::internal_ids::DataRevision;

/// A row of the two-column entity the oracle property runs against.
type OracleRow = (Option<String>, Option<i64>);

/// Strings chosen to break a naive `LIKE` implementation: the wildcards, the escape character,
/// mixed case (SQLite's `LIKE` is ASCII-case-insensitive by default while `=` is not), and
/// multi-byte text so that byte-wise collation is exercised.
#[hegel::composite]
fn oracle_text(tc: &hegel::TestCase) -> String {
    tc.draw(generators::sampled_from(vec![
        "", "a", "A", "ab", "aB", "%", "_", "\\", "a%b", "a_b", "100%", "1000", "a\\b", "ß", "é",
        "日本", "aa", "Aa",
    ]))
    .to_owned()
}

#[hegel::composite]
fn oracle_rows(tc: &hegel::TestCase) -> Vec<OracleRow> {
    let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(12));
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        rows.push((
            tc.draw(generators::optional(oracle_text())),
            tc.draw(generators::optional(
                generators::integers::<i64>().min_value(-3).max_value(3),
            )),
        ));
    }
    rows
}

/// A type-correct predicate over the oracle entity's two columns.
///
/// Type-correct because comparing a column against the wrong kind is rejected outright, which
/// is its own (separately tested) behaviour rather than something this property is about.
#[hegel::composite]
fn oracle_pred(tc: &hegel::TestCase, depth: usize) -> Pred {
    let leaves = 11u8;
    let choice = if depth == 0 {
        tc.draw(generators::integers::<u8>().max_value(leaves - 1))
    } else {
        tc.draw(generators::integers::<u8>().max_value(leaves + 2))
    };
    // Whether this leaf talks about the text column or the integer one.
    let on_text = tc.draw(generators::booleans());
    let attr = if on_text { "t" } else { "n" };
    let value = |tc: &hegel::TestCase| -> DataValue {
        if tc.draw(generators::booleans()) {
            DataValue::Null
        } else if on_text {
            DataValue::Text(tc.draw(oracle_text()))
        } else {
            DataValue::Int(tc.draw(generators::integers::<i64>().min_value(-3).max_value(3)))
        }
    };
    // `In` lists exclude null: `x IN (a, NULL)` is three-valued in a way that says more about
    // SQL than about this compiler, and the null cases are covered by the comparisons.
    let non_null = |tc: &hegel::TestCase| -> DataValue {
        if on_text {
            DataValue::Text(tc.draw(oracle_text()))
        } else {
            DataValue::Int(tc.draw(generators::integers::<i64>().min_value(-3).max_value(3)))
        }
    };
    match choice {
        0 => Pred::Eq(attr.to_owned(), value(tc)),
        1 => Pred::Ne(attr.to_owned(), value(tc)),
        2 => Pred::Gt(attr.to_owned(), value(tc)),
        3 => Pred::Gte(attr.to_owned(), value(tc)),
        4 => Pred::Lt(attr.to_owned(), value(tc)),
        5 => Pred::Lte(attr.to_owned(), value(tc)),
        6 => {
            let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
            Pred::In(attr.to_owned(), (0..count).map(|_| non_null(tc)).collect())
        },
        7 => Pred::IsNull(attr.to_owned()),
        8 => Pred::NotNull(attr.to_owned()),
        // Always the text column: a prefix or substring match against the integer one is
        // rejected outright, which `prefix_and_contains_are_rejected_on_a_non_text_column`
        // covers rather than this property.
        9 => Pred::Prefix("t".to_owned(), tc.draw(oracle_text())),
        10 => Pred::Contains("t".to_owned(), tc.draw(oracle_text())),
        11 => {
            let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
            Pred::And(
                (0..count)
                    .map(|_| tc.draw(oracle_pred(depth - 1)))
                    .collect(),
            )
        },
        12 => {
            let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
            Pred::Or(
                (0..count)
                    .map(|_| tc.draw(oracle_pred(depth - 1)))
                    .collect(),
            )
        },
        _ => Pred::Not(Box::new(tc.draw(oracle_pred(depth - 1)))),
    }
}

/// Evaluate a predicate against one row the way SQL would.
///
/// `None` is SQL's third truth value: a comparison involving NULL is unknown, and `WHERE` keeps
/// a row only on `Some(true)`. `And`/`Or` follow Kleene logic, which is why they cannot simply
/// be `all`/`any` over booleans.
fn oracle_eval(pred: &Pred, row: &OracleRow) -> Option<bool> {
    let (text, number) = row;
    // SQLite's default collation compares text byte-wise, which is also how Rust orders `str`.
    let compare = |value: &DataValue| -> Option<std::cmp::Ordering> {
        match value {
            DataValue::Null => None,
            DataValue::Text(v) => text.as_ref().map(|t| t.as_str().cmp(v.as_str())),
            DataValue::Int(v) => number.map(|n| n.cmp(v)),
            other => panic!("the oracle entity has no column for {:?}", other),
        }
    };
    // `LIKE` is ASCII-case-insensitive by default in SQLite, unlike `=`.
    let like = |needle: &str, f: fn(&str, &str) -> bool| -> Option<bool> {
        text.as_ref()
            .map(|t| f(&t.to_ascii_lowercase(), &needle.to_ascii_lowercase()))
    };
    match pred {
        Pred::Eq(_, v) => compare(v).map(|o| o.is_eq()),
        Pred::Ne(_, v) => compare(v).map(|o| o.is_ne()),
        Pred::Gt(_, v) => compare(v).map(|o| o.is_gt()),
        Pred::Gte(_, v) => compare(v).map(|o| o.is_ge()),
        Pred::Lt(_, v) => compare(v).map(|o| o.is_lt()),
        Pred::Lte(_, v) => compare(v).map(|o| o.is_le()),
        Pred::In(_, values) => {
            let mut any_unknown = false;
            let mut found = false;
            for value in values {
                match compare(value) {
                    Some(std::cmp::Ordering::Equal) => found = true,
                    Some(_) => {},
                    None => any_unknown = true,
                }
            }
            if found {
                Some(true)
            } else if any_unknown {
                None
            } else {
                Some(false)
            }
        },
        Pred::IsNull(attr) => Some(if attr == "t" {
            text.is_none()
        } else {
            number.is_none()
        }),
        Pred::NotNull(attr) => Some(if attr == "t" {
            text.is_some()
        } else {
            number.is_some()
        }),
        Pred::Prefix(_, v) => like(v, |h, n| h.starts_with(n)),
        Pred::Contains(_, v) => like(v, |h, n| h.contains(n)),
        Pred::And(preds) => {
            let mut unknown = false;
            for inner in preds {
                match oracle_eval(inner, row) {
                    Some(false) => return Some(false),
                    None => unknown = true,
                    Some(true) => {},
                }
            }
            if unknown {
                None
            } else {
                Some(true)
            }
        },
        Pred::Or(preds) => {
            let mut unknown = false;
            for inner in preds {
                match oracle_eval(inner, row) {
                    Some(true) => return Some(true),
                    None => unknown = true,
                    Some(false) => {},
                }
            }
            if unknown {
                None
            } else {
                Some(false)
            }
        },
        Pred::Not(inner) => oracle_eval(inner, row).map(|b| !b),
    }
}

/// The rows a filter selects are exactly the rows that satisfy it.
///
/// This is the strongest property available for the query compiler: it checks the generated SQL
/// against an independent evaluator rather than against another query. It covers `LIKE`
/// escaping, three-valued null logic, collation, and boolean nesting in one place — the
/// escaping bug that the two hand-written `%`/`_` cases caught is the kind of thing it exists
/// to find more of.
#[hegel::test(test_cases = 60)]
fn a_filter_selects_exactly_the_rows_that_satisfy_it(tc: hegel::TestCase) {
    let rows = tc.draw(oracle_rows());
    let pred = tc.draw(oracle_pred(3));

    let (store, _dir) = store();
    let def = EntityDef::new("oracle", 1)
        .attr(AttrDef::text("t"))
        .attr(AttrDef::int("n"));
    store.define_entity(NS, OWNER, &def, false).unwrap();
    // One batch, so ids are 1..=rows.len() in the order they were generated.
    let mutations: Vec<Mutation> = rows
        .iter()
        .map(|(text, number)| {
            Mutation::put("oracle")
                .set("t", text.clone())
                .set("n", *number)
                .build()
        })
        .collect();
    store.apply(NS, &mutations).unwrap();

    let fetched = store
        .fetch(
            NS,
            &FetchRequest::entity("oracle")
                .filter(pred.clone())
                .select(["id"])
                .limit(MAX_FETCH_ROWS),
        )
        .unwrap();
    let actual: Vec<i64> = column(&fetched, "id")
        .into_iter()
        .map(|v| match v {
            DataValue::Int(i) => i,
            other => panic!("expected an id, got {:?}", other),
        })
        .collect();

    let expected: Vec<i64> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| oracle_eval(&pred, row) == Some(true))
        .map(|(index, _)| index as i64 + 1)
        .collect();

    assert_eq!(
        actual, expected,
        "filter {:?} over rows {:?} selected {:?}, expected {:?}",
        pred, rows, actual, expected
    );
}

/// A watch reports exactly when the rows it would fetch differ. Checking the digest against a
/// fresh fetch is the property the digest exists to provide — a false negative is a UI that
/// silently stops updating.
#[hegel::test(test_cases = 25)]
fn a_watch_reports_a_change_exactly_when_the_rows_differ(tc: hegel::TestCase) {
    #[derive(Debug, Clone)]
    enum Step {
        Insert(String),
        DeleteAll,
        RewriteSame,
    }

    let steps: Vec<Step> = tc.draw(
        generators::vecs(hegel::one_of!(
            generators::sampled_from(vec!["a", "b", "c"]).map(|s| Step::Insert(s.to_owned())),
            generators::just(Step::DeleteAll),
            generators::just(Step::RewriteSame),
        ))
        .min_size(1)
        .max_size(10),
    );

    let (store, _dir) = defined();
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("fixed")
                .set("project", "fixed")
                .build()],
        )
        .unwrap();
    let request = FetchRequest::entity("agent").select(["project"]);
    let handle = store
        .register_watch(NS, PLUGIN, CLIENT, request.clone(), WatchMode::Rows)
        .unwrap();
    let mut last_seen = handle.rows.clone();

    for step in steps {
        match step {
            Step::Insert(project) => put(&store, &project, "x", 1),
            Step::DeleteAll => {
                store
                    .apply(NS, &[Mutation::delete("agent", Pred::not_null("project"))])
                    .unwrap();
            },
            Step::RewriteSame => {
                store
                    .apply(
                        NS,
                        &[Mutation::put("agent")
                            .uid("fixed")
                            .set("project", "fixed")
                            .build()],
                    )
                    .unwrap();
            },
        }
        let expected = store.fetch(NS, &request).unwrap();
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        if expected == last_seen {
            assert!(
                changes.is_empty(),
                "reported a change when the rows were identical: {:?}",
                expected
            );
        } else {
            assert_eq!(
                changes.len(),
                1,
                "failed to report a change from {:?} to {:?}",
                last_seen,
                expected
            );
            assert_eq!(changes[0].rows, expected);
            last_seen = expected;
        }
    }
}

/// Two writers doing read-modify-write against one row, interleaved arbitrarily, and every write
/// guarded on the revision its writer last read.
///
/// The property is the one the guard exists to provide: **no update is silently lost**. Each
/// writer either lands its value — and can see it in the row afterwards — or is told it
/// conflicted. What must never happen is a write reporting success while another writer's value
/// is the one in the row, which is exactly what an unguarded upsert does.
///
/// A model rather than an oracle over SQL: the store is compared against what a single-threaded
/// reading of "compare the revision, then write" would have produced. The writers are simulated
/// rather than threaded because the interleaving is the interesting part and a scheduler would
/// make it unrepeatable — the store serializes concurrent callers anyway, so a sequence of
/// interleaved calls is precisely what two servers produce.
#[hegel::test(test_cases = 30)]
fn a_guarded_write_never_loses_an_update(tc: hegel::TestCase) {
    /// Which of the two writers acts, and whether it re-reads first.
    #[derive(Debug, Clone)]
    struct Step {
        writer: usize,
        reread: bool,
    }

    let steps: Vec<Step> = tc.draw(
        generators::vecs(
            generators::tuples!(
                generators::integers::<usize>().max_value(1),
                generators::booleans(),
            )
            .map(|(writer, reread)| Step { writer, reread }),
        )
        .min_size(1)
        .max_size(12),
    );

    let (store, _dir) = defined();
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("contested")
                .set("project", "start")
                .build()],
        )
        .unwrap();

    // The row as it stands: its revision and its value.
    let read = |store: &PluginDataStore| -> (i64, String) {
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        let rev = match column(&rows, "rev")[0] {
            DataValue::Int(rev) => rev,
            ref other => panic!("`rev` is {:?}, not an integer", other),
        };
        (rev, texts(&rows, "project").remove(0))
    };

    // What each writer last saw. Both start from the same read, which is the situation two
    // servers are in after fetching the same row.
    let start = read(&store);
    let mut held = [start.clone(), start];
    // What the store must hold, tracked independently of it.
    let mut expected = held[0].clone();

    for (n, step) in steps.iter().enumerate() {
        if step.reread {
            held[step.writer] = read(&store);
        }
        let (seen_rev, _) = held[step.writer].clone();
        let value = format!("w{}-{}", step.writer, n);
        let result = store.apply(
            NS,
            &[Mutation::put("agent")
                .uid("contested")
                .set("project", value.clone())
                .expect_rev(DataRevision::new(seen_rev))
                .build()],
        );

        // The model: the write lands exactly when the writer's revision is the stored one.
        let stale = seen_rev != expected.0;
        match result {
            Ok(_) => {
                assert!(
                    !stale,
                    "step {n}: a write holding revision {seen_rev} landed while the row was at \
                     {}",
                    expected.0
                );
                expected = (expected.0 + 1, value);
                held[step.writer] = expected.clone();
            },
            Err(e) => {
                assert_err!(e, DataError::Conflict(_));
                assert!(
                    stale,
                    "step {n}: a write holding the current revision {seen_rev} was refused"
                );
            },
        }

        // And the store agrees, every step of the way. This is what makes "no update is lost"
        // an assertion rather than a hope: a write that reported success is the one in the row.
        assert_eq!(
            read(&store),
            expected,
            "step {n}: the store and the model disagree"
        );
    }
}
