//! Reading back what agents have done: the `agent_run` and `agent_run_entry` rows, queried.
//!
//! The rows themselves, and every write to them, belong to [`crate::agents`] — this module
//! never writes. It exists because the panel and the model want opposite things from the same
//! history. The panel has a run in hand and wants its newest entries, which is
//! `agents::rows::hydrate_newest_entries` and nothing more. The model has a *question* —
//! "has an agent looked at this before", "what did the one in that pane conclude" — and
//! answering it means searching a machine's whole history and then reading around a hit.
//!
//! Store-wide is the default rather than a widening, and that is the store's doing: it is not
//! per session, so a run recorded in a session that has since been closed is as legible as one
//! still going. SQLite includes all sessions using the data directory. Narrowing by
//! session or host identity is optional.
//!
//! Its own module rather than more of `agents.rs` for the reason [`crate::notes`] is its own:
//! reading is a different job from recording, and `agents.rs` is already the largest file in
//! the crate.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use yinz_tile::prelude::HostId;
use yinz_types::internal_ids::SessionName;

use crate::agents::{self, rows, AgentRunHeader, RunEntry, RunEntryKind};
use crate::projects;
use crate::store::{self, Backend};
use yinz_tile::prelude::{FetchRequest, Pred, SortDir};

/// Stable archive header shape shared by panel and CLI readers.
pub fn header_reply(
    header: &AgentRunHeader,
    projects: &BTreeMap<String, crate::projects::Project>,
    now_ms: i64,
) -> serde_json::Value {
    serde_json::json!({
        "run_id": header.id,
        "kind": header.kind,
        "title": header.title,
        "pane_id": header.pane_id,
        "session": header.yinz_session_name,
        "host_id": header.host_id,
        "cwd": header.cwd,
        "project": agents::checkout_location(
                header.host.as_deref(),
                header.cwd.as_deref(),
            )
            .and_then(|location| projects::for_cwd(projects, &location))
            .map(|project| project.name.clone()),
        "active": matches!(header.outcome, agents::RunOutcome::Active),
        "outcome": match header.outcome {
            agents::RunOutcome::Active => "active",
            agents::RunOutcome::Completed => "completed",
            agents::RunOutcome::Closed => "closed",
            agents::RunOutcome::Replaced => "replaced",
        },
        "started_at_ms": header.started_at_ms,
        "ended_at_ms": header.ended_at_ms,
        "age": agents::short_age(now_ms, header.started_at_ms),
        "entries": header.revision,
        "summary": header.summary.as_ref().map(|summary| summary.text.clone()),
    })
}

/// Runs one listing may return. A page the model reads, not a page a UI scrolls.
pub const MAX_RUNS: usize = 50;

/// Hits one search may return.
pub const MAX_MATCHES: usize = 50;

/// How much of an entry a search hit shows. `agent_run` is how the whole of one is read.
pub const SNIPPET_CHARS: usize = 200;

/// How much of one entry a *multi-entry* page carries.
///
/// Not a bound on what can be read: a page of one entry spends the whole of
/// [`PAGE_TEXT_BUDGET`] on it, so a two-thousand-word plan is reachable by asking for exactly
/// it. This is only what stops one long entry from crowding out the twenty around it.
pub const ENTRY_TEXT_CHARS: usize = 2_000;

/// How much entry text one page may carry in total.
///
/// A local tool result is *not* passed through `MAX_TOOL_OUTPUT` — that cap is on the command,
/// extension and web paths — so a page that does not budget itself is a page with no bound at
/// all, and a long run is megabytes. Well under the 40 000 that path allows, leaving room for
/// the header and the JSON around it.
pub const PAGE_TEXT_BUDGET: usize = 30_000;

/// Entries one page may carry, whatever they weigh.
pub const MAX_PAGE_ENTRIES: usize = 100;

/// How many entries before an anchored one are included, so a hit arrives with its lead-in.
///
/// Fixed rather than an argument: the model should not have to do arithmetic on revisions to
/// see what a result was answering.
pub const ANCHOR_LEAD_IN: u64 = 2;

/// The `IN` chunk that [`crate::notes`] uses, and for the same reason: one `Pred::is_in` of
/// unbounded width is a query the store will refuse.
const IN_CHUNK: usize = 500;

/// How many rows a listing will scan to fill a page it has to narrow in the guest.
///
/// Bounded because the narrowing throws rows away *after* the store has read them: a project
/// with one run in an archive of ten thousand would otherwise page the whole archive to find
/// it. Answering "no runs in this page" with an offset to retry is the honest alternative.
const MAX_RUN_SCAN: usize = 400;

/// How many runs a narrowed search will resolve into its scope before giving up on the rest.
///
/// A search that narrows resolves the candidate runs first (see [`search`]), and resolving
/// them one page at a time is what stops "no matches" from meaning "no matches in the newest
/// fifty". It still has to stop somewhere: an `IN` list of every run a machine ever recorded
/// is a query the store will refuse, and a scope this wide is already more history than a
/// question is really about. Reaching the cap sets `Found::truncated`, so a partial answer
/// says it is one.
const MAX_SCOPE_RUNS: usize = MAX_RUN_SCAN;

/// How many rows resolving a scope will read to find those runs.
///
/// The second half of the same bound, and the one that matters for a filter almost nothing
/// matches: a project with four runs in an archive of ten thousand would otherwise page the
/// whole archive on every search. Reaching this is reported the same way reaching
/// [`MAX_SCOPE_RUNS`] is.
const MAX_SCOPE_SCAN: usize = 4_000;

/// Which runs a caller is asking about. Every field `None` means every run in the store — which
/// includes every session using the data directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// One yinz session's runs, by name.
    ///
    /// A session name is only unique within a machine, so this is worth pairing with
    /// [`Filter::host_id`] on a shared store — two machines can each have a `main`.
    pub yinz_session_name: Option<SessionName>,
    /// One machine's runs, by its host id. See `agents::rows::Owner`.
    ///
    /// Left `None` by default, and deliberately: "have I had an agent look at this before" is a
    /// better question when it can answer across every machine the user works on, which is what
    /// a shared store now makes possible. Narrowing is the option, as it is for the session.
    pub host_id: Option<HostId>,
    /// A project, as a root folder and the machine that folder is on. Matched against a run's
    /// `cwd` by prefix, in the guest — see [`matches_root`].
    pub root: Option<RootScope>,
    /// An agent kind: `claude`, `codex`.
    pub kind: Option<String>,
    /// Only runs started at or after this epoch millisecond.
    pub since_ms: Option<i64>,
    /// `Some(true)` for runs still going, `Some(false)` for finished ones, `None` for both.
    pub active: Option<bool>,
}

impl Filter {
    /// The predicates the store can answer. `root` is deliberately absent: a run's `cwd` lives
    /// inside its `header` JSON rather than in a column, so narrowing by project is a pass over
    /// the fetched page and costs an over-fetch, which [`list_runs`] pays.
    pub fn pred(&self) -> Option<Pred> {
        let mut preds = Vec::new();
        if let Some(session) = self.session() {
            preds.push(Pred::eq("yinz_session_name", session));
        }
        if let Some(host_id) = self
            .host_id
            .as_ref()
            .map(|id| HostId::new(id.as_str().trim()))
            .filter(|id| !id.is_empty())
        {
            // A run recorded before the `host_id` column existed carries the migration's empty
            // default, and a closed run is never rewritten, so nothing will ever fill it in.
            // Paired with a session name this is `agents::rows::owned_or_unclaimed`'s bargain —
            // an unclaimed row is this session's, and a session name only means anything within
            // one machine — and it is what `this_session` asks, since that arm sets both. Asked
            // alone the question is about one machine, and a row that names none cannot answer
            // it: on a shared store the tolerance would hand back every machine's oldest runs.
            preds.push(match self.session() {
                Some(_) => store::host_or_legacy(&host_id),
                None => Pred::eq("host_id", host_id),
            });
        }
        if let Some(kind) = trimmed(&self.kind) {
            preds.push(Pred::eq("kind", kind));
        }
        if let Some(active) = self.active {
            preds.push(Pred::eq("active", active));
        }
        if let Some(since) = self.since_ms {
            preds.push(Pred::gte("started_at_ms", since));
        }
        (!preds.is_empty()).then(|| Pred::all(preds))
    }

    fn session(&self) -> Option<&str> {
        self.yinz_session_name
            .as_ref()
            .map(|name| name.as_str().trim())
            .filter(|name| !name.is_empty())
    }

    /// Whether this filter needs a guest-side pass, and so an over-fetch.
    pub fn narrows_in_guest(&self) -> bool {
        self.root.is_some()
    }

    pub fn keeps(&self, header: &AgentRunHeader) -> bool {
        match &self.root {
            Some(scope) => matches_root(
                header.cwd.as_deref(),
                header.host.as_deref(),
                &header.host_id,
                scope,
            ),
            None => true,
        }
    }
}

/// A project: a root folder, and the machine that folder is on.
///
/// The two together rather than a bare root, so that a caller cannot narrow to a project
/// without saying whose it is. See [`matches_root`] for why the machine is not optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RootScope {
    /// The normalized project root.
    pub root: String,
    /// The machine the root is a folder on — this one, for every caller there is. See
    /// `agents::rows::Owner`.
    pub host_id: HostId,
}

impl RootScope {
    pub fn new(root: impl Into<String>, host_id: HostId) -> Self {
        RootScope {
            root: root.into(),
            host_id,
        }
    }
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// Whether a run's folder is inside a project's root.
///
/// A prefix on path boundaries rather than `starts_with` on the raw string, so `/work/agent`
/// does not claim `/work/agent-fork`. A run with no `cwd` at all — one recorded before the
/// field existed — belongs to no project rather than to every one.
///
/// A run on another machine matches nothing, which is `projects::contains`' rule and is here
/// for its reason: `/home/me/work` on a build box and `/home/me/work` here are two different
/// folders, and a project is a folder on *this* machine. Filing a remote agent's run under a
/// local project because the two spell the same path would put work nobody did here into the
/// answer to "what has been done on this".
///
/// `host_id` is a second, independent way a run can be on another machine, checked for every
/// run: `host` is the SSH target the *pane* was opened on, which the run
/// has always carried, while `host_id` identifies the installation that recorded it.
///
/// An **empty** `host_id` matches. It is a row from before the column, and for a run `""` reads
/// as "this machine" — the same accommodation `agents::rows::owned_or_unclaimed` makes, and for
/// the same reason: there was only one machine when it was written, so refusing it would hide a
/// project's whole history from before the upgrade.
pub fn matches_root(
    cwd: Option<&str>,
    host: Option<&str>,
    host_id: &HostId,
    scope: &RootScope,
) -> bool {
    if host.is_some_and(|host| !host.trim().is_empty()) {
        return false;
    }
    let host_id = host_id.as_str().trim();
    if !host_id.is_empty() && host_id != scope.host_id.as_str().trim() {
        return false;
    }
    let (Some(cwd), root) = (cwd, scope.root.trim_end_matches('/')) else {
        return false;
    };
    let cwd = cwd.trim_end_matches('/');
    cwd == root
        || cwd
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

// ---------------------------------------------------------------------------
// Listing runs
// ---------------------------------------------------------------------------

/// One page of runs, and where the page after it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPage {
    pub headers: Vec<AgentRunHeader>,
    /// Whether more runs follow this page.
    pub more: bool,
    /// How many store rows this page consumed, which is what the next `offset` adds.
    ///
    /// Deliberately not `headers.len()`: a guest-side narrowing throws rows away *after* the
    /// store has counted them, so a page that kept three runs out of a hundred and sixty
    /// scanned starts the next page a hundred and sixty rows along. Paging on the kept count
    /// re-reads all but the first three of those rows for ever, and the caller sees the same
    /// runs on every page while the older ones stay out of reach.
    pub scanned: usize,
}

/// One page of runs, newest first, active and archived together.
///
/// Both, unlike the panel's archive pager, because the model's question is about the work and
/// not about the pane: "what has been done on this" is answered as badly by hiding the run
/// still doing it as by hiding the four before it.
pub fn list_runs(
    backend: &impl Backend,
    filter: &Filter,
    limit: usize,
    offset: usize,
) -> Result<RunPage, String> {
    let limit = limit.clamp(1, MAX_RUNS);
    // A guest-side narrowing throws rows away after the store has counted them, so ask for
    // more than the page when one is in play. Bounded: an over-fetch that pages the whole
    // archive to find nothing is worse than an honest "no runs here" with an offset to retry.
    let want = match filter.narrows_in_guest() {
        true => limit.saturating_mul(8).min(MAX_RUN_SCAN),
        false => limit.saturating_add(1),
    };
    let mut request = FetchRequest::entity(rows::RUN_ENTITY)
        // A run written before `header` existed has none to list, and its payload is the only
        // other copy — the same filter the panel's archive pager applies.
        .filter(Pred::not_null("header"))
        .sort_by("started_at_ms", SortDir::Desc)
        // A tiebreak, so two runs that began in the same millisecond page deterministically
        // rather than swapping places between one fetch and the next.
        .sort_by("uid", SortDir::Asc)
        // Older headers omit ownership, so recover it from the row columns.
        .select(["header", "host_id", "yinz_session_name"])
        .limit(want as u32)
        .offset(offset as u32);
    if let Some(pred) = filter.pred() {
        request = request.filter(pred);
    }
    let fetched = backend.fetch(&request)?;
    let raw = fetched.len();
    // Kept and counted in one pass, because the page has to report both what it kept and how
    // many rows it spent getting there. A row that overflows the page is the boundary: the
    // rows before it are this page's, and the next page starts on it.
    let mut headers: Vec<AgentRunHeader> = Vec::new();
    let mut scanned = raw;
    let mut overflowed = false;
    for (index, header) in rows::run_headers_from_rows(&fetched)
        .into_iter()
        .enumerate()
    {
        if !filter.keeps(&header) {
            continue;
        }
        if headers.len() == limit {
            overflowed = true;
            scanned = index;
            break;
        }
        headers.push(header);
    }
    // A fetch that returned rows always advances, whatever the arithmetic above made of them:
    // an offset that stands still is a caller paging for ever.
    let scanned = match raw {
        0 => 0,
        raw => scanned.clamp(1, raw),
    };
    // `truncated` as well as the row count, because [`crate::store::MAX_FETCH_BYTES`] cuts a
    // fetch by weight and not by `limit` — a scan stopped at three hundred rows of four
    // hundred asked for would otherwise report that the archive ends there.
    let more = overflowed || (filter.narrows_in_guest() && (raw >= want || fetched.truncated));
    Ok(RunPage {
        headers,
        more,
        scanned,
    })
}

/// One run's header, whatever session it belongs to and whether or not it is still open.
pub fn run_header(backend: &impl Backend, run_id: &str) -> Result<Option<AgentRunHeader>, String> {
    let rows = backend.fetch(&rows::run_by_id_request(run_id))?;
    Ok(rows::runs_from_rows(&rows)
        .into_iter()
        .next()
        .map(|run| run.header()))
}

/// Follow only recorded links, retaining the reachable prefix if the chain is incomplete.
pub fn predecessors(
    anchor: &AgentRunHeader,
    mut load: impl FnMut(&str) -> Result<Option<AgentRunHeader>, String>,
) -> (Vec<AgentRunHeader>, String) {
    let mut seen = BTreeSet::from([anchor.id.clone()]);
    let mut next = anchor.previous_run_id.clone();
    let mut runs = Vec::new();
    while let Some(id) = next {
        if !seen.insert(id.clone()) {
            return (runs, format!("Incomplete chain: repeated run {id}"));
        }
        let run = match load(&id) {
            Ok(Some(run)) => run,
            Ok(None) => return (runs, format!("Incomplete chain: missing run {id}")),
            Err(error) => {
                return (
                    runs,
                    format!("Incomplete chain: could not read {id}: {error}"),
                )
            },
        };
        next = run.previous_run_id.clone();
        runs.push(run);
    }
    (runs, String::new())
}

/// The headers of these runs, for annotating search hits.
fn headers_by_id(
    backend: &impl Backend,
    ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, AgentRunHeader>, String> {
    let ids: Vec<String> = ids.iter().cloned().collect();
    let mut found = BTreeMap::new();
    for chunk in ids.chunks(IN_CHUNK) {
        let rows = backend.fetch(
            &FetchRequest::entity(rows::RUN_ENTITY)
                .filter(Pred::is_in("uid", chunk.to_vec()))
                .select(["payload"])
                .limit(chunk.len() as u32),
        )?;
        for run in rows::runs_from_rows(&rows) {
            found.insert(run.id.clone(), run.header());
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// Reading one run's timeline
// ---------------------------------------------------------------------------

/// One page of a run's timeline, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryPage {
    pub entries: Vec<RunEntry>,
    /// Revisions whose preview text was shortened by this page read.
    pub truncated_revisions: Vec<u64>,
    /// Every entry the run has, not just this page's.
    pub total: u64,
    /// The revision to ask for next, or `None` when this page reached the end.
    pub next_revision: Option<u64>,
    /// Whether anything before this page's first entry was left unread.
    pub earlier: bool,
}

/// Shared JSON entry shape for the CLI and Agent archive tools.
pub fn entry_reply(entry: &RunEntry, now_ms: i64) -> serde_json::Value {
    let mut reply = serde_json::json!({
        "revision": entry.revision,
        "kind": entry.kind.label(),
        "created_at_ms": entry.created_at_ms,
        "age": entry.created_at_ms.map(|at| crate::agents::short_age(now_ms, at)),
        "text": entry.text,
    });
    if let Some(tool) = &entry.tool {
        reply["tool"] = serde_json::json!(tool);
    }
    reply
}

/// A page of `run_id`'s entries.
///
/// `from` is where to start. `None` means **the newest page**, which is what a bare read of a
/// long-lived run should answer: the end is where the work got to, and the panel's own
/// `hydrate_newest_entries` makes the same choice for the same reason.
///
/// `limit` bounds the count; [`PAGE_TEXT_BUDGET`] bounds the weight, and a page stops at
/// whichever it reaches first. A page of one is the way to read a single enormous entry whole:
/// [`ENTRY_TEXT_CHARS`] only applies once a page holds more than one.
pub fn entry_page(
    backend: &impl Backend,
    run_id: &str,
    from: Option<u64>,
    limit: usize,
) -> Result<EntryPage, String> {
    let limit = limit.clamp(1, MAX_PAGE_ENTRIES);
    let total = backend
        .count(&FetchRequest::entity(rows::ENTRY_ENTITY).filter(Pred::eq("run_id", run_id)))?;
    let mut request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", run_id))
        .limit(limit as u32);
    // Forward from a revision, or backward from the end when none was named. The second is
    // fetched newest-first and reversed here, since the store cannot sort ascending from an
    // unknown offset.
    request = match from {
        Some(from) => request
            .filter(Pred::gte("revision", from as i64))
            .sort_by("revision", SortDir::Asc),
        None => request.sort_by("revision", SortDir::Desc),
    };
    let fetched = backend.fetch(&request)?;
    let mut entries = rows::entries_from_rows(&fetched)
        .remove(run_id)
        .unwrap_or_default();
    // `entries_from_rows` sorts by revision whichever way the rows arrived, so the newest-page
    // fetch needs no reversing — only its own bound re-applying, since it took the *last*
    // `limit` entries and the budget below cuts from the wrong end for that case.
    let mut truncated_revisions = if from.is_some() && limit == 1 {
        vec![]
    } else {
        entries
            .iter()
            .filter(|entry| entry.text.chars().count() > ENTRY_TEXT_CHARS)
            .map(|entry| entry.revision)
            .collect()
    };
    budget(&mut entries, from.is_none(), from.is_some() && limit == 1);
    truncated_revisions.retain(|revision| entries.iter().any(|entry| entry.revision == *revision));
    let first = entries.first().map(|entry| entry.revision);
    let last = entries.last().map(|entry| entry.revision);
    // What is unread *after* this page, and only ever for a page read forward. A page taken
    // backward from the end of the run has nothing ahead of it however short the budget cut it:
    // what it dropped is older than what it kept, which is what `earlier` reports. Saying
    // `last + 1` there would send a reader to a revision the run does not have.
    //
    // Asked of the store rather than inferred from a full page, because "this page was as long
    // as you allowed" and "there is more" are not the same claim — a run of exactly thirty
    // entries read thirty at a time would otherwise advertise a thirty-first that is not there,
    // and the caller renders that as a sentence telling a reader to go and get it.
    let next_revision = match (last, from) {
        (Some(last), Some(_)) => {
            let after = backend.count(
                &FetchRequest::entity(rows::ENTRY_ENTITY)
                    .filter(Pred::eq("run_id", run_id))
                    .filter(Pred::gt("revision", last as i64)),
            )?;
            (after > 0).then(|| last.saturating_add(1))
        },
        _ => None,
    };
    // Measured from where the page starts, and from where it was *asked* to start when it
    // held nothing: a read from past the end of a run has the whole run behind it, and saying
    // `earlier: false` there tells a reader who followed a stale `next_revision` that a run it
    // can see the entry count of has nothing in it. A page read backward from the end that
    // came back empty is an empty run, which has nothing earlier and says so.
    let earlier = match first.or(from) {
        Some(boundary) => {
            let before = backend.count(
                &FetchRequest::entity(rows::ENTRY_ENTITY)
                    .filter(Pred::eq("run_id", run_id))
                    .filter(Pred::lt("revision", boundary as i64)),
            )?;
            before > 0
        },
        None => false,
    };
    Ok(EntryPage {
        entries,
        truncated_revisions,
        total,
        next_revision,
        earlier,
    })
}

/// Search detail with the complete matching entry and the usual surrounding context.
/// The selected entry may exceed the page's normal text budget.
pub fn search_detail(
    backend: &impl Backend,
    run_id: &str,
    revision: u64,
) -> Result<EntryPage, String> {
    let mut page = entry_page(
        backend,
        run_id,
        Some(anchored_from(revision)),
        MAX_PAGE_ENTRIES,
    )?;
    let full = entry_page(backend, run_id, Some(revision), 1)?;
    if let Some(entry) = full
        .entries
        .into_iter()
        .find(|entry| entry.revision == revision)
        && let Some(selected) = page
            .entries
            .iter_mut()
            .find(|entry| entry.revision == revision)
    {
        *selected = entry;
        page.truncated_revisions.retain(|r| *r != revision);
    }
    Ok(page)
}

/// Where a page anchored on `revision` starts: far enough back to carry the lead-in.
pub fn anchored_from(revision: u64) -> u64 {
    revision.saturating_sub(ANCHOR_LEAD_IN).max(1)
}

/// Cut a page down to [`PAGE_TEXT_BUDGET`], reporting whether anything was dropped.
///
/// `from_end` keeps the *newest* entries, for a page read backward from the end of a run;
/// otherwise the oldest are kept, since the caller asked to start there and expects to page
/// forward from what it got.
///
/// An exact revision with limit one is exempt from previews: it returns the retained payload.
pub fn budget(entries: &mut Vec<RunEntry>, from_end: bool, exact: bool) -> bool {
    if !exact {
        for entry in entries.iter_mut() {
            entry.text = cut(&entry.text, ENTRY_TEXT_CHARS);
            if let Some(tool) = &mut entry.tool {
                tool.omit_payload();
            }
        }
    }
    let mut spent = 0usize;
    let mut kept = 0usize;
    let ordered: Vec<usize> = match from_end {
        true => (0..entries.len()).rev().collect(),
        false => (0..entries.len()).collect(),
    };
    for index in ordered {
        let cost = serde_json::to_vec(&entries[index]).map_or(usize::MAX, |value| value.len());
        if kept > 0 && spent.saturating_add(cost) > PAGE_TEXT_BUDGET {
            break;
        }
        spent = spent.saturating_add(cost);
        kept += 1;
    }
    if kept >= entries.len() {
        return false;
    }
    match from_end {
        true => {
            entries.drain(..entries.len() - kept);
        },
        false => entries.truncate(kept),
    }
    true
}

// ---------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------

/// One entry that matched a search, with enough of itself to judge by and enough of its
/// address to read the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub run_id: String,
    pub revision: u64,
    pub kind: RunEntryKind,
    pub created_at_ms: Option<i64>,
    pub snippet: String,
}

/// What a search answers: the hits, and the runs they belong to.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub runs: BTreeMap<String, AgentRunHeader>,
    /// Whether this is less than the whole answer — either the hit limit cut the results
    /// short, or the scope was too wide to resolve in full and only part of the history was
    /// searched. One flag for both because they mean the same thing to a reader: "not
    /// finding it here does not mean it is not there".
    pub truncated: bool,
}

/// The runs a narrowed search will look inside, and whether the cap cut them short.
///
/// Paged rather than taken as one [`MAX_RUNS`] page, because that page is fifty runs and a
/// question like "has an agent looked at this before" is asked of a machine's whole history:
/// searching the newest fifty and answering "no" is the one wrong answer this module exists to
/// avoid. [`MAX_SCOPE_RUNS`] and [`MAX_SCOPE_SCAN`] bound it, and reaching either is reported
/// rather than hidden.
fn scope_runs(backend: &impl Backend, filter: &Filter) -> Result<(Vec<String>, bool), String> {
    let mut ids: Vec<String> = Vec::new();
    let mut offset = 0usize;
    loop {
        let page = list_runs(backend, filter, MAX_RUNS, offset)?;
        ids.extend(page.headers.into_iter().map(|header| header.id));
        if ids.len() >= MAX_SCOPE_RUNS {
            ids.truncate(MAX_SCOPE_RUNS);
            return Ok((ids, true));
        }
        // `scanned` is zero only when the fetch returned nothing at all, which is the end of
        // the archive however many rows the filter threw away on the way there.
        if !page.more || page.scanned == 0 {
            return Ok((ids, false));
        }
        offset = offset.saturating_add(page.scanned);
        if offset >= MAX_SCOPE_SCAN {
            return Ok((ids, true));
        }
    }
}

/// Search every recorded entry's text.
///
/// Two shapes, and which one runs is decided by whether anything narrows the search:
///
/// - **Unnarrowed**, which is the common one: search the entries directly and look the hit
///   runs' headers up afterwards. One FTS query over the whole machine's history.
/// - **Narrowed**: resolve the candidate runs first and search within them, so a filter that
///   matches four runs cannot be starved by five hundred hits that all belong to others.
///
/// With `regex`, scan bounded pages of complete entries, newest stored first.
/// Otherwise the FTS index is tried first and a substring pass is the fallback, exactly as
/// `notes::search` does it — a query FTS5 tokenizes into nothing still finds the entries that
/// literally contain it.
pub fn search(
    backend: &impl Backend,
    filter: &Filter,
    run_id: Option<&str>,
    query: &str,
    regex: bool,
    limit: usize,
) -> Result<Found, String> {
    let pattern = if regex {
        Some(search_pattern(query)?)
    } else {
        None
    };
    let query = if regex { query } else { query.trim() };
    if query.is_empty() {
        return Ok(Found::default());
    }
    let limit = limit.clamp(1, MAX_MATCHES);
    let mut partial_scope = false;
    let scope: Option<Vec<String>> = match run_id {
        Some(run_id) => Some(vec![run_id.to_owned()]),
        None if filter == &Filter::default() => None,
        None => {
            let (ids, cut) = scope_runs(backend, filter)?;
            if ids.is_empty() {
                return Ok(Found {
                    truncated: cut,
                    ..Found::default()
                });
            }
            partial_scope = cut;
            Some(ids)
        },
    };
    let (hits, scan_cut) = if let Some(pattern) = pattern {
        regex_hits(
            backend,
            &pattern,
            scope.map(|ids| Pred::is_in("run_id", ids)),
            limit,
        )?
    } else {
        match scope {
            Some(ids) => {
                let mut hits = Vec::new();
                let mut truncated = false;
                for chunk in ids.chunks(IN_CHUNK) {
                    let (page, cut) = hits_for(
                        backend,
                        query,
                        Some(Pred::is_in("run_id", chunk.to_vec())),
                        limit,
                    )?;
                    hits.extend(page);
                    truncated |= cut;
                }
                (hits, truncated)
            },
            None => hits_for(backend, query, None, limit)?,
        }
    };
    let truncated = hits.len() > limit || partial_scope || scan_cut;
    let hits: Vec<Hit> = hits.into_iter().take(limit).collect();
    let ids: BTreeSet<String> = hits.iter().map(|hit| hit.run_id.clone()).collect();
    let runs = headers_by_id(backend, &ids)?;
    Ok(Found {
        hits,
        runs,
        truncated,
    })
}

/// Compile a bounded history search pattern shared with the Scheme reader.
pub fn search_pattern(query: &str) -> Result<regex::Regex, String> {
    if query.is_empty() || query.contains('\0') || query.len() > 4096 {
        return Err("regex pattern must be nonempty, contain no NUL, and be at most 4 KiB".into());
    }
    regex::RegexBuilder::new(query)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map_err(|error| format!("invalid regex: {error}"))
}

const REGEX_SCAN_ROWS: usize = 4_000;
const REGEX_SCAN_BYTES: usize = 8 * 1024 * 1024;

fn regex_hits(
    backend: &impl Backend,
    pattern: &regex::Regex,
    scope: Option<Pred>,
    limit: usize,
) -> Result<(Vec<Hit>, bool), String> {
    use yinz_tile::prelude::DataValue;
    let mut request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .select(["id", "run_id", "revision", "kind", "created_at_ms", "text"])
        .sort_by("id", SortDir::Desc)
        .limit(100);
    if let Some(scope) = scope {
        request = request.filter(scope);
    }
    let mut last_id = None;
    let mut scanned = 0;
    let mut bytes = 0;
    let mut hits = Vec::new();
    let truncated = 'scan: loop {
        let mut page_request = request.clone();
        if let Some(id) = last_id {
            page_request = page_request.filter(Pred::lt("id", id));
        }
        let page = backend.fetch(&page_request)?;
        for row in &page.rows {
            if scanned == REGEX_SCAN_ROWS {
                break 'scan true;
            }
            let Some(DataValue::Int(id)) = page.get(row, "id") else {
                return Err("history entry has no database id".into());
            };
            last_id = Some(*id);
            scanned += 1;
            let Some(DataValue::Text(text)) = page.get(row, "text") else {
                continue;
            };
            if text.len() > REGEX_SCAN_BYTES - bytes {
                break 'scan true;
            }
            bytes += text.len();
            let Some(found) = pattern.find(text) else {
                continue;
            };
            let (
                Some(DataValue::Text(run_id)),
                Some(DataValue::Int(revision)),
                Some(DataValue::Text(kind)),
            ) = (
                page.get(row, "run_id"),
                page.get(row, "revision"),
                page.get(row, "kind"),
            )
            else {
                continue;
            };
            let (Ok(revision), Some(kind)) =
                (u64::try_from(*revision), RunEntryKind::from_label(kind))
            else {
                continue;
            };
            hits.push(Hit {
                run_id: run_id.clone(),
                revision,
                kind,
                created_at_ms: match page.get(row, "created_at_ms") {
                    Some(DataValue::Int(at)) => Some(*at),
                    _ => None,
                },
                snippet: regex_snippet(text, found.start()),
            });
            if hits.len() > limit {
                break 'scan true;
            }
        }
        if !page.truncated {
            break false;
        }
        if page.is_empty() || scanned == REGEX_SCAN_ROWS || bytes == REGEX_SCAN_BYTES {
            break true;
        }
    };
    // Retain the newest stored matches, then present them chronologically like keyword hits.
    hits.truncate(limit);
    hits.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| b.revision.cmp(&a.revision))
    });
    Ok((hits, truncated))
}

pub fn regex_snippet(text: &str, at: usize) -> String {
    let start = text[..at]
        .char_indices()
        .rev()
        .nth(SNIPPET_CHARS / 3)
        .map_or(0, |(index, _)| index);
    let end = text[start..]
        .char_indices()
        .nth(SNIPPET_CHARS)
        .map_or(text.len(), |(index, _)| start + index);
    let window = text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        window,
        if end < text.len() { "…" } else { "" }
    )
}

/// Search the panel's distinct runs, retaining one representative entry per valid run.
pub fn search_runs(backend: &impl Backend, query: &str) -> Result<Found, String> {
    search_runs_filtered(backend, query, |_| true)
}

/// Distinct runs filtered before the result limit; scanning is explicitly bounded.
pub fn search_runs_filtered(
    backend: &impl Backend,
    query: &str,
    keeps: impl Fn(&AgentRunHeader) -> bool,
) -> Result<Found, String> {
    const PAGE: u32 = 100;
    let query = query.trim();
    let mut found = Found::default();
    if query.is_empty() {
        return Ok(found);
    }
    let mut request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .search(query)
        .limit(PAGE);
    let mut page = backend.fetch(&request)?;
    // Choose once: an exhausted FTS page must not start a second, substring search.
    if page.is_empty() {
        request = FetchRequest::entity(rows::ENTRY_ENTITY)
            .filter(Pred::contains("text", query))
            .sort_by("created_at_ms", SortDir::Desc)
            .sort_by("uid", SortDir::Asc)
            .limit(PAGE);
        page = backend.fetch(&request)?;
    }
    let mut offset = 0;
    let mut scanned = 0;
    let mut tools = false;
    let mut seen = BTreeSet::new();
    loop {
        scanned += page.len();
        let hits = hits_from_rows(&page, query, tools);
        let ids = hits
            .iter()
            .filter(|hit| !seen.contains(&hit.run_id))
            .map(|hit| hit.run_id.clone())
            .collect();
        let headers = headers_by_id(backend, &ids)?;
        for hit in hits {
            if !seen.insert(hit.run_id.clone()) {
                continue;
            }
            let Some(header) = headers.get(&hit.run_id) else {
                continue;
            };
            if !keeps(header) {
                continue;
            }
            if found.hits.len() == MAX_RUNS {
                found.truncated = true;
                return Ok(found);
            }
            found.runs.insert(hit.run_id.clone(), header.clone());
            found.hits.push(hit);
        }
        let exhausted = page.is_empty() || !page.truncated;
        if exhausted && tools {
            return Ok(found);
        }
        if scanned >= MAX_SCOPE_SCAN {
            found.truncated = true;
            return Ok(found);
        }
        if exhausted {
            tools = true;
            offset = 0;
            request = FetchRequest::entity(rows::ENTRY_ENTITY)
                .filter(Pred::not_null("tool"))
                .sort_by("created_at_ms", SortDir::Desc)
                .sort_by("uid", SortDir::Asc)
                .limit(PAGE);
        } else {
            offset += page.len() as u32;
        }
        page = backend.fetch(
            &request
                .clone()
                .offset(offset)
                .limit(PAGE.min((MAX_SCOPE_SCAN - scanned) as u32)),
        )?;
    }
}

/// The matching entries, FTS first and a substring pass behind it.
fn hits_for(
    backend: &impl Backend,
    query: &str,
    scope: Option<Pred>,
    limit: usize,
) -> Result<(Vec<Hit>, bool), String> {
    // One more than asked for, so the caller can tell a full page from a cut one.
    let want = limit.saturating_add(1) as u32;
    let scoped = |request: FetchRequest| match scope.clone() {
        Some(pred) => request.filter(pred),
        None => request,
    };
    // No sort on the FTS path: an unsorted search is ordered by relevance, and imposing one
    // would throw that away.
    let mut rows = backend.fetch(&scoped(
        FetchRequest::entity(rows::ENTRY_ENTITY)
            .search(query)
            .select([
                "run_id",
                "revision",
                "kind",
                "text",
                "created_at_ms",
                "host_id",
            ])
            .limit(want),
    ))?;
    if rows.is_empty() {
        rows = backend.fetch(&scoped(
            FetchRequest::entity(rows::ENTRY_ENTITY)
                .filter(Pred::contains("text", query))
                .select([
                    "run_id",
                    "revision",
                    "kind",
                    "text",
                    "created_at_ms",
                    "host_id",
                ])
                .sort_by("created_at_ms", SortDir::Desc)
                .limit(want),
        ))?;
    }
    let mut hits = hits_from_rows(&rows, query, false);
    let request = scoped(
        FetchRequest::entity(rows::ENTRY_ENTITY)
            .filter(Pred::not_null("tool"))
            .sort_by("created_at_ms", SortDir::Desc)
            .sort_by("uid", SortDir::Asc),
    );
    let mut scanned = 0;
    let mut scan_cut = false;
    let mut tool_matches = 0;
    // Keep the tool pass even when text filled the page: tool hits can be newer.
    while tool_matches < want as usize {
        let page = backend.fetch(
            &request
                .clone()
                .offset(scanned as u32)
                .limit(100.min((MAX_SCOPE_SCAN - scanned) as u32)),
        )?;
        scanned += page.len();
        for hit in hits_from_rows(&page, query, true) {
            tool_matches += 1;
            if !hits
                .iter()
                .any(|old| old.run_id == hit.run_id && old.revision == hit.revision)
            {
                hits.push(hit);
            }
        }
        if !page.truncated {
            break;
        }
        if page.is_empty() || scanned == MAX_SCOPE_SCAN || tool_matches >= want as usize {
            scan_cut = true;
            break;
        }
    }
    hits.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| b.revision.cmp(&a.revision))
    });
    Ok((hits, rows.truncated || scan_cut))
}

pub fn hits_from_rows(
    rows: &yinz_tile::prelude::DataRows,
    query: &str,
    tools_only: bool,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    let query_lower = query.to_lowercase();
    for (run_id, entries) in rows::entries_from_rows(rows) {
        for entry in entries {
            let tool = entry.tool.as_ref().map(|tool| tool.text());
            let matching_tool = tool
                .as_deref()
                .filter(|text| text.to_lowercase().contains(&query_lower));
            if tools_only && matching_tool.is_none() {
                continue;
            }
            let text = matching_tool
                .filter(|_| !entry.text.to_lowercase().contains(&query_lower))
                .unwrap_or(&entry.text);
            hits.push(Hit {
                run_id: run_id.clone(),
                revision: entry.revision,
                kind: entry.kind,
                created_at_ms: entry.created_at_ms,
                snippet: snippet(text, query),
            });
        }
    }
    // `entries_from_rows` groups by run, which loses whatever order the rows arrived in.
    // Newest first is the order to restore: relevance cannot survive the grouping, and of the
    // orders that can, "most recent" is the one a person asking about past work means.
    hits.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| b.revision.cmp(&a.revision))
    });
    hits
}

/// A window of `text` around the first occurrence of `query`, on one line.
///
/// Centred on the hit rather than taken from the head, which is what makes a search result
/// worth reading: the match in a two-thousand-word plan is rarely in its first sentence. When
/// the query does not appear literally — FTS5 stems, so it often will not — this falls back to
/// the head of the text, which is `notes::snippet`'s behaviour.
pub fn snippet(text: &str, query: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= SNIPPET_CHARS {
        return one_line;
    }
    let chars: Vec<char> = one_line.chars().collect();
    let start = match find_fold(&one_line, query) {
        // Centre the window on the match, without running off the front.
        Some(at) => at.saturating_sub(SNIPPET_CHARS / 3),
        None => 0,
    }
    .min(chars.len().saturating_sub(SNIPPET_CHARS));
    let window: String = chars.iter().skip(start).take(SNIPPET_CHARS).collect();
    let lead = if start > 0 { "…" } else { "" };
    format!("{lead}{}…", window.trim())
}

/// The character index of `needle` in `haystack`, ignoring case. `None` for an empty needle.
///
/// Character index and not a byte offset, because the caller slices by characters — a byte
/// offset from `str::find` would cut a multi-byte character in half.
///
/// Folded one character to one, rather than through `str::to_lowercase`, for the same reason:
/// the index has to be an index into the *original* text, and `to_lowercase` is not
/// length-preserving — `İ` lowercases to two characters, so one of them anywhere before the
/// match would shift the caller's window off the hit it is supposed to be centred on.
fn find_fold(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }
    let hay: Vec<char> = haystack.chars().map(fold_char).collect();
    let needle: Vec<char> = needle.chars().map(fold_char).collect();
    // Also try the first word alone: a multi-word query rarely appears verbatim, and the
    // word it starts with usually does.
    let first: Vec<char> = needle
        .iter()
        .copied()
        .take_while(|c| !c.is_whitespace())
        .collect();
    window_at(&hay, &needle).or_else(|| window_at(&hay, &first))
}

/// One character folded to one, keeping the first of a multi-character lowercasing.
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn window_at(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&at| &hay[at..at + needle.len()] == needle)
}

/// `text` cut to `chars` characters, marked when anything was dropped.
///
/// By characters and not bytes, so a cut never lands inside a multi-byte character.
fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_owned();
    }
    let kept: String = text.chars().take(chars).collect();
    format!("{}…", kept.trim_end())
}

/// A run's age, for a listing. Re-exported so a caller needs only this module.
pub use agents::short_age;

#[cfg(test)]
#[path = "unit/run_history_regex.rs"]
mod regex_tests;

#[cfg(test)]
mod tests {

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
            .any(|row| row.text.contains("message: commentary")));
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

    /// The `notes::search` fallback, pinned here too: FTS5 tokenizes on word boundaries, so a
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
}
