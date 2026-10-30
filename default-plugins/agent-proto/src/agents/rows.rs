//! Persisted agent rows and host/session ownership.
//!
//! The `agent_latch` and `agent_summary` rows in `shared:yinz`: what `agent_register` set by
//! hand, and what the daemon last said about each idle pane, so a agent reload keeps both.
//! agent is the only writer; see `store::agent_latch_def` and `store::agent_summary_def`.

use yinz_tile::prelude::HostId;

use super::{
    summary_says_same, AgentRun, AgentRunHeader, RunEntry, RunEntryKind, Summaries, Summary,
};
use crate::store::Backend;
use std::collections::{BTreeMap, BTreeSet};
use yinz_tile::prelude::{
    DataRevision, DataRows, DataValue, FetchRequest, Mutation, Pred, PutBuilder, SortDir,
    MAX_FETCH_ROWS,
};

use crate::store::owned_by;
pub use crate::store::{owned_rows_request, Owner};

pub const LATCH_ENTITY: &str = "agent_latch";
pub const SUMMARY_ENTITY: &str = "agent_summary";
pub const RUN_ENTITY: &str = "agent_run";
pub const ENTRY_ENTITY: &str = "agent_run_entry";

/// How much of a run's timeline is handed to the panel at once.
///
/// The one bound left on a run's size, and it is at the delivery seam rather than at the
/// record: an entry costs a row, but the panel is sent its run as a single JSON string
/// copied into the interpreter, and that is the only place the length of a long-lived run is
/// paid for. The newest end is the one that is kept, because that is the end the history
/// half is anchored to and scrolls back from.
///
/// `ponytail:` page older entries into the panel when it scrolls past the oldest of these.
/// Nothing does that yet, and a run of five hundred prompts, plans and questions is a very
/// long day's work.
pub const RUN_DETAIL_ENTRIES: u32 = 500;

/// The other half of that bound, and the half that actually holds.
///
/// A row count bounds nothing on its own now that an entry keeps the whole of what the
/// agent said: five hundred plans is megabytes, and the only ceiling left under it is the
/// store's own `MAX_FETCH_BYTES`. What the panel is sent has to fit in its memory as one
/// JSON string, so the entries are taken from the newest end until this much text has been
/// gathered. Generous next to any real run — a day of prompts and plans is tens of
/// kilobytes — and small next to the two megabytes a fetch may hand back.
pub const RUN_DETAIL_BYTES: usize = 256 * 1024;

/// This server's open runs, for the restore at load.
///
/// Scoped by [`owned_or_unclaimed`] rather than [`owned_by`]: a run recorded before the
/// store could be shared between machines names no machine, and it is this one's. Read
/// strictly, the first launch after the upgrade would restore none of its own open runs
/// and could not close them either — see [`owned_or_unclaimed`].
pub fn active_runs_request(owner: &Owner) -> FetchRequest {
    FetchRequest::entity(RUN_ENTITY)
        .filter(Pred::eq("active", true))
        .filter(owned_or_unclaimed(owner))
        .select(["uid", "rev", "payload", "host_id", "yinz_session_name"])
}

/// One run by its id, whatever session it belongs to and whether or not it is still open.
///
/// The lookup a session performs for a pane that arrived from somewhere else: the row is
/// under another session's name and has already been closed by the session that lost the
/// pane, so neither of [`active_runs_request`]'s filters would find it.
pub fn run_by_id_request(run_id: &str) -> FetchRequest {
    FetchRequest::entity(RUN_ENTITY)
        .filter(Pred::eq("uid", run_id))
        .select(["uid", "rev", "payload", "host_id", "yinz_session_name"])
        .limit(1)
}

/// The current revision of each listed run, to confirm in one fetch that a hydrated batch
/// is unchanged. Holds at most [`MAX_FETCH_ROWS`] ids, like [`entries_request`].
pub fn run_revisions_request(run_ids: impl IntoIterator<Item = String>) -> FetchRequest {
    FetchRequest::entity(RUN_ENTITY)
        .filter(Pred::is_in("uid", run_ids))
        .select(["uid", "rev"])
        .limit(MAX_FETCH_ROWS)
}

/// One entry's row key. Run-scoped and revision-keyed, so re-writing an entry the store
/// already holds upserts it rather than making a second copy of it — the same shape, and
/// for the same reason, as agent's `message_uid`.
fn entry_uid(run_id: &str, revision: u64) -> String {
    format!("{run_id}#{revision}")
}

/// One entry on a run's timeline.
///
/// `host_id` is attribution rather than key — the uid is `{run_id}#{revision}`, and a run
/// id does not collide between machines: the ordinary shape carries the agent's own session
/// uuid, and the fallback for a hook that supplies none carries the machine. It is here
/// so that a search across a shared history can say where a hit happened.
pub fn entry_mutation(run_id: &str, host_id: &HostId, entry: &RunEntry) -> Mutation {
    let mut put = Mutation::put(ENTRY_ENTITY)
        .uid(entry_uid(run_id, entry.revision))
        .set("run_id", run_id)
        .set("host_id", host_id)
        .set("revision", entry.revision as i64)
        .set("kind", entry.kind.label())
        .set("text", entry.text.clone());
    if let Some(tool) = &entry.tool
        && let Ok(value) = serde_json::to_string(tool)
    {
        put = put.set("tool", value);
    }
    if let Some(event_id) = entry.event_id.as_ref() {
        put = put.set("event_id", event_id.clone());
    }
    if let Some(created_at_ms) = entry.created_at_ms {
        put = put.set("created_at_ms", created_at_ms);
    }
    put.build()
}

/// Every entry of these runs, a page at a time.
///
/// Deliberately unsorted. `after_id` is the only paging that does not have to re-scan what
/// it has already read, and the store refuses it beside a sort — so the fetch takes the
/// default `id` ordering and [`entries_from_rows`] sorts by revision in the guest, exactly
/// as agent's transcript loader does with `seq`. Row order agrees with revision order
/// anyway: a row is written when its entry is made.
pub fn entries_request(run_ids: impl IntoIterator<Item = String>) -> FetchRequest {
    FetchRequest::entity(ENTRY_ENTITY)
        .filter(Pred::is_in("run_id", run_ids))
        .limit(MAX_FETCH_ROWS)
}

/// The newest [`RUN_DETAIL_ENTRIES`] entries of one run, for the panel.
///
/// Sorted, which rules out a cursor — but one bounded fetch has nothing to page.
pub fn newest_entries_request(run_id: &str) -> FetchRequest {
    FetchRequest::entity(ENTRY_ENTITY)
        .filter(Pred::eq("run_id", run_id))
        .sort_by("revision", SortDir::Desc)
        .limit(RUN_DETAIL_ENTRIES)
}

fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<i64> {
    match rows.get(row, column) {
        Some(DataValue::Int(value)) => Some(*value),
        _ => None,
    }
}

/// Every entry in these rows, grouped by run and ordered oldest first.
///
/// A row whose `kind` this version does not know is dropped rather than guessed at: a
/// newer agent writing a kind this one cannot draw is a thing to ignore, not to crash on.
pub fn entries_from_rows(rows: &DataRows) -> BTreeMap<String, Vec<RunEntry>> {
    let mut grouped: BTreeMap<String, Vec<RunEntry>> = BTreeMap::new();
    for row in &rows.rows {
        let run_id = text_at(rows, row, "run_id");
        let (Some(revision), Some(kind)) = (
            int_at(rows, row, "revision").and_then(|value| u64::try_from(value).ok()),
            RunEntryKind::from_label(&text_at(rows, row, "kind")),
        ) else {
            continue;
        };
        let event_id = match rows.get(row, "event_id") {
            Some(DataValue::Text(id)) if !id.is_empty() => Some(id.clone()),
            _ => None,
        };
        grouped.entry(run_id).or_default().push(RunEntry {
            tool: match rows.get(row, "tool") {
                Some(DataValue::Text(value)) => serde_json::from_str(value).ok(),
                _ => None,
            },
            revision,
            event_id,
            kind,
            text: text_at(rows, row, "text"),
            created_at_ms: int_at(rows, row, "created_at_ms"),
        });
    }
    for entries in grouped.values_mut() {
        entries.sort_by_key(|entry| entry.revision);
    }
    grouped
}

/// Fill in these runs' timelines from their entry rows.
///
/// A run's `entries` are only replaced when the store actually holds rows for it. That is
/// what makes the split backward compatible without a migration: a run recorded before
/// entries had rows of their own still carries them inside its `payload`, deserializes
/// with them, and is left alone here.
///
/// Returns the ids of the runs that *were* hydrated, which is exactly the question a
/// caller has to answer before deciding what still needs writing: a run left holding its
/// payload's entries has none of them stored as rows yet, and its next persist has to
/// write all of them — the payload it is written with no longer carries a timeline.
pub fn hydrate_entries(backend: &impl Backend, runs: &mut [AgentRun]) -> BTreeSet<String> {
    if runs.is_empty() {
        return BTreeSet::new();
    }
    let mut request = entries_request(runs.iter().map(|run| run.id.clone()));
    let mut grouped: BTreeMap<String, Vec<RunEntry>> = BTreeMap::new();
    loop {
        let Ok(rows) = backend.fetch(&request) else {
            return BTreeSet::new();
        };
        for (run_id, entries) in entries_from_rows(&rows) {
            grouped.entry(run_id).or_default().extend(entries);
        }
        // A run's timeline can outgrow one page, so keep going while the store says there
        // is more. `next_cursor` is only handed back under the default ordering, which is
        // why `entries_request` does not sort.
        match rows.truncated.then_some(rows.next_cursor).flatten() {
            Some(cursor) => request = request.after_id(cursor),
            None => break,
        }
    }
    let mut hydrated = BTreeSet::new();
    for run in runs {
        if let Some(mut entries) = grouped.remove(&run.id) {
            entries.sort_by_key(|entry| entry.revision);
            run.entries = entries;
            hydrated.insert(run.id.clone());
        }
    }
    hydrated
}

/// Fill in one run's timeline with its newest entries, for the panel. See
/// [`RUN_DETAIL_ENTRIES`] and [`RUN_DETAIL_BYTES`].
///
/// The budget is spent from the newest end, which is the end the history half is anchored
/// to, and the newest entry is kept whatever it costs: a run whose last plan is on its own
/// over the budget should show that plan, not an empty history.
///
/// What is left out is counted in [`AgentRun::omitted_entries`], so the half can say its
/// oldest row is not where the run began.
pub fn hydrate_newest_entries(backend: &impl Backend, run: &mut AgentRun) {
    run.omitted_entries = 0;
    let Ok(rows) = backend.fetch(&newest_entries_request(&run.id)) else {
        return;
    };
    let Some(mut entries) = entries_from_rows(&rows).remove(&run.id) else {
        return;
    };
    let mut spent = 0usize;
    let mut kept = 0;
    for entry in entries.iter().rev() {
        if kept > 0 && spent.saturating_add(entry.text.len()) > RUN_DETAIL_BYTES {
            break;
        }
        spent = spent.saturating_add(entry.text.len());
        kept += 1;
    }
    let dropped = entries.len() - kept;
    entries.drain(..dropped);
    // A truncated fetch stopped at the row limit with older rows still behind it. It does
    // not say how many, so they count as one: enough for "there is more".
    run.omitted_entries = dropped + usize::from(rows.truncated);
    run.entries = entries;
}

fn pane_id_at(rows: &DataRows, row: &[DataValue]) -> Option<u32> {
    match rows.get(row, "pane_id") {
        Some(DataValue::Int(id)) => u32::try_from(*id).ok(),
        _ => None,
    }
}

fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> String {
    match rows.get(row, column) {
        Some(DataValue::Text(text)) => text.clone(),
        _ => String::new(),
    }
}

/// A pane's row key: the machine, the session on it, and the pane.
///
/// Session and host identity keep local pane ids distinct across servers and installations.
fn pane_uid(owner: &Owner, pane_id: u32) -> String {
    format!("{}:{}:{}", owner.host_id, owner.yinz_session_name, pane_id)
}

/// This server's rows, and the rows that name no server at all — which are also this
/// server's, since there was only one machine when they were written.
///
/// The store migrates an entity unasked, and adding `host_id` to `agent_run` was an
/// `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT ''`: every run already recorded came out of
/// the migration with an empty host. Demanding this machine's id would therefore match none
/// of a daemon's own runs on the first launch after the upgrade. They would never be
/// restored, never adopted, and — since the guarded close in [`run_owned_mutation`] names
/// the host too — never closed, sitting `active` forever while a fresh run started beside
/// each of them for the same agent.
///
/// The session half stays strict, so this is no weaker than the guard was before the host
/// existed. It is weaker than the current guard in exactly one case: two machines sharing a
/// store, both running a session of the same name, over a run neither has claimed yet. That
/// needs a database written before the host column existed, and the first ordinary write of
/// such a run claims it — see [`claim_unowned_runs`].
fn owned_or_unclaimed(owner: &Owner) -> Pred {
    crate::store::host_or_legacy(&owner.host_id).and(Pred::eq(
        "yinz_session_name",
        owner.yinz_session_name.clone(),
    ))
}

/// Stamp this machine onto restored runs that name none.
///
/// Not a write of its own: [`run_values`] takes `host_id` straight off the run, so the claim
/// lands with the run's next ordinary persist rather than costing a write per run at load.
/// A run that is never written again keeps its empty host, which [`owned_or_unclaimed`] goes
/// on reading — the accommodation is permanent on the read side and one-shot on the write
/// side, which is the right way round.
///
/// It also puts the machine back on the run's *entries*: `entry_mutation` is given
/// `run.host_id`, so an unclaimed run would go on recording entries that say where they
/// happened only by omission.
pub fn claim_unowned_runs(runs: &mut [AgentRun], owner: &Owner) {
    for run in runs.iter_mut().filter(|run| run.host_id.is_empty()) {
        run.host_id = owner.host_id.clone();
    }
}

/// The two columns that say who a row belongs to, for a `Put` that writes one.
///
/// Always both, and in one place, so that a row cannot be written with a uid naming a host
/// and a column that disagrees.
fn owner_values(put: PutBuilder, owner: &Owner) -> PutBuilder {
    put.set("host_id", owner.host_id.clone())
        .set("yinz_session_name", owner.yinz_session_name.clone())
}

/// Rows left behind by earlier versions of these entities: the one that keyed on the pane id
/// alone, and the one that keyed on the session without the machine.
///
/// Neither can be reached by a predicate that a current row matches, and their uids collide
/// with nothing, so they would simply accumulate. Dropped once, on the first write after the
/// migration.
///
/// An empty `host_id` is the marker for the second kind, since that is what the migration
/// fills the new column with. It cannot be told apart from a row this machine wrote before
/// upgrading, which is the point: the pane ids in it belong to a server that is gone either
/// way, and a stale latch is worth less than a wrong one.
pub fn stale_unowned_delete_mutations() -> Vec<Mutation> {
    [LATCH_ENTITY, SUMMARY_ENTITY]
        .into_iter()
        .map(|entity| Mutation::delete(entity, Pred::eq("host_id", "")))
        .collect()
}

pub fn latch_mutation(owner: &Owner, pane_id: u32, kind: &str) -> Mutation {
    owner_values(
        Mutation::put(LATCH_ENTITY)
            .uid(pane_uid(owner, pane_id))
            .set("pane_id", pane_id),
        owner,
    )
    .set("kind", kind)
    .build()
}

pub fn latch_delete_mutation(owner: &Owner, pane_ids: impl IntoIterator<Item = u32>) -> Mutation {
    Mutation::delete(
        LATCH_ENTITY,
        Pred::is_in("pane_id", pane_ids).and(owned_by(owner)),
    )
}

pub fn latches_from_rows(rows: &DataRows) -> BTreeMap<u32, String> {
    rows.rows
        .iter()
        .filter_map(|row| {
            let kind = text_at(rows, row, "kind");
            if kind.is_empty() {
                return None;
            }
            Some((pane_id_at(rows, row)?, kind))
        })
        .collect()
}

pub fn summary_mutation(owner: &Owner, pane_id: u32, summary: &Summary) -> Mutation {
    let mut put = owner_values(
        Mutation::put(SUMMARY_ENTITY)
            .uid(pane_uid(owner, pane_id))
            .set("pane_id", pane_id),
        owner,
    )
    .set("text", summary.text.clone())
    .set("title", summary.title.clone())
    .set("awaiting", summary.awaiting);
    if let Some(created_at_ms) = summary.created_at_ms {
        put = put.set("created_at_ms", created_at_ms);
    }
    put.build()
}

pub fn summary_delete_mutation(owner: &Owner, pane_ids: impl IntoIterator<Item = u32>) -> Mutation {
    Mutation::delete(
        SUMMARY_ENTITY,
        Pred::is_in("pane_id", pane_ids).and(owned_by(owner)),
    )
}

/// The summaries these rows describe, with no activity hash: the hash is a fact about a
/// run, and a restored summary is refreshed by the daemon on its own schedule.
pub fn summaries_from_rows(rows: &DataRows) -> Summaries {
    rows.rows
        .iter()
        .filter_map(|row| {
            Some((
                pane_id_at(rows, row)?,
                Summary {
                    text: text_at(rows, row, "text"),
                    title: text_at(rows, row, "title"),
                    awaiting: matches!(rows.get(row, "awaiting"), Some(DataValue::Bool(true))),
                    hash: 0,
                    created_at_ms: int_at(rows, row, "created_at_ms"),
                },
            ))
        })
        .collect()
}

/// The writes that turn `before` into `after`, ignoring the hash and the time it was
/// made: an unchanged summary produces no write, so a poll that changes nothing is
/// silent - and a summary the daemon re-stamped without changing a word of is unchanged.
pub fn summary_diff_mutations(
    owner: &Owner,
    before: &Summaries,
    after: &Summaries,
) -> Vec<Mutation> {
    let mut mutations: Vec<Mutation> = after
        .iter()
        .filter(|(pane_id, summary)| {
            !before
                .get(pane_id)
                .is_some_and(|b| summary_says_same(b, summary))
        })
        .map(|(pane_id, summary)| summary_mutation(owner, *pane_id, summary))
        .collect();
    let gone: Vec<u32> = before
        .keys()
        .filter(|pane_id| !after.contains_key(pane_id))
        .copied()
        .collect();
    if !gone.is_empty() {
        mutations.push(summary_delete_mutation(owner, gone));
    }
    mutations
}

/// The columns a run is stored as. One list, so a `Put` and a guarded `Update` cannot
/// disagree about what a row holds.
fn run_values(run: &AgentRun) -> Vec<(&'static str, DataValue)> {
    vec![
        ("pane_id", DataValue::from(run.pane_id)),
        ("session_id", DataValue::from(run.session_id.clone())),
        (
            "yinz_session_name",
            DataValue::from(run.yinz_session_name.clone()),
        ),
        ("host_id", DataValue::from(run.host_id.clone())),
        ("kind", DataValue::from(run.kind.clone())),
        ("active", DataValue::from(run.ended_at_ms.is_none())),
        ("started_at_ms", DataValue::from(run.started_at_ms)),
        (
            "header",
            DataValue::from(
                serde_json::to_string(&run.header()).expect("AgentRunHeader serializes"),
            ),
        ),
        ("payload", DataValue::from(stored_payload(run))),
    ]
}

/// The run as its `payload` column holds it: everything except the timeline, which lives
/// in [`ENTRY_ENTITY`] rows.
///
/// Written from a cleared copy rather than by making `entries` a skipped serde field,
/// because the same struct is serialized again to reach the panel — where the timeline is
/// the whole point of sending it.
///
/// Field by field, and deliberately not `..run.clone()`: functional-update syntax builds
/// the whole value before overriding, so that spelling deep-clones every entry only to
/// drop it — on every hook, for a run that has been going all day. A literal without `..`
/// also cannot silently omit a field, since the compiler refuses one that is not listed.
fn stored_payload(run: &AgentRun) -> String {
    let stored = AgentRun {
        id: run.id.clone(),
        pane_id: run.pane_id,
        yinz_session_name: run.yinz_session_name.clone(),
        host_id: run.host_id.clone(),
        origin_run_id: run.origin_run_id.clone(),
        previous_run_id: run.previous_run_id.clone(),
        session_id: run.session_id.clone(),
        kind: run.kind.clone(),
        cwd: run.cwd.clone(),
        host: run.host.clone(),
        bubblewrap: run.bubblewrap.clone(),
        started_at_ms: run.started_at_ms,
        ended_at_ms: run.ended_at_ms,
        outcome: run.outcome,
        revision: run.revision,
        summarized_revision: run.summarized_revision,
        summary: run.summary.clone(),
        entries: Vec::new(),
        // A record is every entry the run has; only a copy sent to the panel leaves any out.
        omitted_entries: 0,
    };
    serde_json::to_string(&stored).expect("AgentRun serializes")
}

/// Guard metadata with the revision its author read; zero means a new run.
pub fn run_mutation(run: &AgentRun, expected_revision: DataRevision) -> Mutation {
    let mut put = Mutation::put(RUN_ENTITY)
        .uid(run.id.clone())
        .expect_rev(expected_revision);
    for (attr, value) in run_values(run) {
        put = put.set(attr, value);
    }
    put.build()
}

/// A run and every entry it holds, as one batch.
///
/// The whole-run write: [`run_mutation`] alone stores only the metadata, since the
/// timeline has rows of its own. agent-service does not use this — it knows which entries
/// the store already has and writes only the new ones — but anything holding a complete
/// run and no such bookkeeping wants all of it written.
pub fn run_mutations(run: &AgentRun, expected_revision: DataRevision) -> Vec<Mutation> {
    let mut mutations = Vec::with_capacity(run.entries.len() + 1);
    mutations.push(run_mutation(run, expected_revision));
    mutations.extend(
        run.entries
            .iter()
            .map(|entry| entry_mutation(&run.id, &run.host_id, entry)),
    );
    mutations
}

/// Write a run *only while it is still ours*.
///
/// Every session sharing a store can hold opinions about one run id — which is exactly what
/// a warp arranges. The session that lost the pane closes the run when it notices the pane
/// gone; the session that gained it has by then reopened the same row under its own name.
/// An unconditional `Put` from the loser would overwrite the winner's copy with a stale,
/// closed one, and the entries the agent has produced since would be gone.
///
/// The predicate settles that inside the transaction rather than by hoping about order: a
/// row already claimed by another session matches nothing, and the batch conflicts.
/// Adoption uses a `Put` guarded by the revision read before changing the owner.
///
/// The owner is the machine and the session, not the session alone. On a store shared
/// between machines a session called `main` on one of them would otherwise pass the guard
/// held by `main` on another and close a run it has nothing to do with — worse than no
/// guard, because it looks like one.
///
/// A run that names no machine passes it too ([`owned_or_unclaimed`]): it was recorded
/// before the column existed, and a guard this session can never satisfy would leave it
/// open for good. The write itself carries the run's host, so the row is claimed by the
/// same act that closes it.
pub fn run_owned_mutation(
    run: &AgentRun,
    owner: &Owner,
    expected_revision: DataRevision,
) -> Mutation {
    let mut update = Mutation::update(
        RUN_ENTITY,
        Pred::eq("uid", run.id.clone()).and(owned_or_unclaimed(owner)),
    )
    .expect_rev(expected_revision);
    for (attr, value) in run_values(run) {
        update = update.set(attr, value);
    }
    update.build()
}

fn decode_run<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    let mut value: serde_json::Value = serde_json::from_str(value).ok()?;
    // Ended legacy runs predate outcomes; preserve any explicitly recorded outcome.
    if value.get("outcome").is_none()
        && value
            .get("ended_at_ms")
            .is_some_and(|ended| !ended.is_null())
    {
        value["outcome"] = serde_json::Value::String("completed".into());
    }
    serde_json::from_value(value).ok()
}

pub fn runs_from_rows(rows: &DataRows) -> Vec<AgentRun> {
    rows.rows
        .iter()
        .filter_map(|row| {
            let Some(DataValue::Text(value)) = rows.get(row, "payload") else {
                return None;
            };
            let mut run: AgentRun = decode_run(value)?;
            if let Some(DataValue::Text(session)) = rows.get(row, "yinz_session_name") {
                run.yinz_session_name = session.clone().into();
            }
            if let Some(DataValue::Text(host_id)) = rows.get(row, "host_id") {
                run.host_id = host_id.clone().into();
            }
            Some(run)
        })
        .collect()
}

pub fn run_headers_from_rows(rows: &DataRows) -> Vec<AgentRunHeader> {
    rows.rows
        .iter()
        .filter_map(|row| header_at(rows, row))
        .collect()
}

pub(crate) fn header_at(rows: &DataRows, row: &[DataValue]) -> Option<AgentRunHeader> {
    let Some(DataValue::Text(value)) = rows.get(row, "header") else {
        return None;
    };
    let mut header: AgentRunHeader = decode_run(value)?;
    // Older stored headers omit ownership; the row columns are authoritative when fetched.
    if let Some(DataValue::Text(session)) = rows.get(row, "yinz_session_name") {
        header.yinz_session_name = session.clone().into();
    }
    if let Some(DataValue::Text(host_id)) = rows.get(row, "host_id") {
        header.host_id = host_id.clone().into();
    }
    Some(header)
}
