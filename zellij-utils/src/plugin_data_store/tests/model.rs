use super::*;
use hegel::generators::{self, Generator};

/// The store driven against a `BTreeMap` holding what it ought to contain.
///
/// A `BTreeMap` keyed on `uid` is the right model because `uid` is the store's own upsert key
/// and the map's iteration order matches `ORDER BY uid` under SQLite's byte-wise collation, so
/// the two can be compared directly without sorting either side.
struct StoreModel {
    store: PluginDataStore,
    // held so the database outlives the store
    _guard: Guard,
    model: BTreeMap<String, (Option<String>, Option<i64>)>,
}

impl StoreModel {
    /// A small pool of keys, so that upserts and deletes actually collide instead of every
    /// operation touching a fresh row.
    fn any_uid(tc: &hegel::TestCase) -> String {
        tc.draw(generators::sampled_from(vec!["a", "b", "c", "d"]))
            .to_owned()
    }

    fn any_text(tc: &hegel::TestCase) -> Option<String> {
        tc.draw(generators::optional(
            generators::sampled_from(vec!["x", "y", "%", "_", "Z"]).map(str::to_owned),
        ))
    }

    fn any_number(tc: &hegel::TestCase) -> Option<i64> {
        tc.draw(generators::optional(
            generators::integers::<i64>().min_value(-2).max_value(2),
        ))
    }
}

#[hegel::state_machine]
impl StoreModel {
    #[rule]
    fn put(&mut self, tc: hegel::TestCase) {
        let uid = Self::any_uid(&tc);
        let text = Self::any_text(&tc);
        let number = Self::any_number(&tc);
        self.store
            .apply(
                NS,
                &[Mutation::put("row")
                    .uid(uid.clone())
                    .set("t", text.clone())
                    .set("n", number)
                    .build()],
            )
            .unwrap();
        self.model.insert(uid, (text, number));
    }

    #[rule]
    fn delete_by_uid(&mut self, tc: hegel::TestCase) {
        let uid = Self::any_uid(&tc);
        self.store
            .apply(NS, &[Mutation::delete("row", Pred::eq("uid", uid.clone()))])
            .unwrap();
        self.model.remove(&uid);
    }

    #[rule]
    fn update_text_by_uid(&mut self, tc: hegel::TestCase) {
        let uid = Self::any_uid(&tc);
        let text = Self::any_text(&tc);
        self.store
            .apply(
                NS,
                &[Mutation::update("row", Pred::eq("uid", uid.clone()))
                    .set("t", text.clone())
                    .build()],
            )
            .unwrap();
        if let Some((stored_text, _)) = self.model.get_mut(&uid) {
            *stored_text = text;
        }
    }

    /// Delete a whole range at once, which no single-row rule would reach.
    #[rule]
    fn delete_where_number_below(&mut self, tc: hegel::TestCase) {
        let cutoff = tc.draw(generators::integers::<i64>().min_value(-2).max_value(2));
        self.store
            .apply(NS, &[Mutation::delete("row", Pred::lt("n", cutoff))])
            .unwrap();
        self.model
            .retain(|_, (_, number)| !matches!(number, Some(n) if *n < cutoff));
    }

    #[invariant]
    fn the_store_holds_what_the_model_holds(&self, _: hegel::TestCase) {
        let rows = self
            .store
            .fetch(
                NS,
                &FetchRequest::entity("row")
                    .select(["uid", "t", "n"])
                    .sort_by("uid", SortDir::Asc)
                    .limit(MAX_FETCH_ROWS),
            )
            .unwrap();
        let actual: Vec<(String, Option<String>, Option<i64>)> = rows
            .rows
            .iter()
            .map(|row| {
                let text_of = |value: Option<&DataValue>| match value {
                    Some(DataValue::Text(s)) => Some(s.clone()),
                    Some(DataValue::Null) | None => None,
                    other => panic!("expected text, got {:?}", other),
                };
                let int_of = |value: Option<&DataValue>| match value {
                    Some(DataValue::Int(i)) => Some(*i),
                    Some(DataValue::Null) | None => None,
                    other => panic!("expected an int, got {:?}", other),
                };
                (
                    text_of(rows.get(row, "uid")).expect("uid is always set"),
                    text_of(rows.get(row, "t")),
                    int_of(rows.get(row, "n")),
                )
            })
            .collect();
        let expected: Vec<(String, Option<String>, Option<i64>)> = self
            .model
            .iter()
            .map(|(uid, (text, number))| (uid.clone(), text.clone(), *number))
            .collect();
        assert_eq!(actual, expected);
    }
}

/// Any sequence of writes leaves the store holding exactly what a plain map would.
///
/// Sequences are where stale-state bugs live — an upsert that fails to clear a previous value,
/// a range delete that misses a row, an update that touches rows its filter excluded. No single
/// example test covers the combinations.
#[hegel::test(test_cases = 25)]
fn the_store_agrees_with_a_map_over_any_sequence_of_writes(tc: hegel::TestCase) {
    let (store, guard) = store();
    let def = EntityDef::new("row", 1)
        .attr(AttrDef::text("t"))
        .attr(AttrDef::int("n").indexed());
    store.define_entity(NS, OWNER, &def, false).unwrap();
    let machine = StoreModel {
        store,
        _guard: guard,
        model: BTreeMap::new(),
    };
    hegel::stateful::run(machine, tc);
}
