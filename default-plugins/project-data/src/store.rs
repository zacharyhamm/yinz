//! Shared entity definitions and backend operations for project and agent data.
//! [`Local`] uses the real SQLite store for native model tests. Scheme plugins send
//! data commands through their confined host bridge and keep transactions in guest code.

use yinz_tile::prelude::DataRevision;
use yinz_tile::prelude::HostId;
use yinz_tile::prelude::*;

/// Which server a row belongs to: the machine, and the session on it.
///
/// One type rather than two arguments because forgetting the host half is exactly the bug
/// this exists to prevent, and a pair of `&str` parameters invites it — the two were a
/// single `yinz_session_name` for as long as the store was per-machine, and every call
/// site that scopes a row would otherwise have to remember to grow a second one.
///
/// Host identity distinguishes installations; session names and pane ids are only local.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Owner {
    pub host_id: HostId,
    pub yinz_session_name: SessionName,
}

impl Owner {
    pub fn new(host_id: HostId, yinz_session_name: SessionName) -> Self {
        Owner {
            host_id,
            yinz_session_name,
        }
    }

    /// Whether this names a session at all.
    ///
    /// The session name arrives from the daemon's first snapshot and is empty until then, so
    /// callers hold back their per-pane reads and writes until it is known — see
    /// `restore_pane_rows`. The host id is available from load and is not part of the test.
    pub fn is_known(&self) -> bool {
        !self.yinz_session_name.is_empty()
    }
}

/// Strictly match both halves of a server's row ownership.
pub fn owned_by(owner: &Owner) -> Pred {
    Pred::eq("host_id", owner.host_id.clone()).and(Pred::eq(
        "yinz_session_name",
        owner.yinz_session_name.clone(),
    ))
}

/// Fetch only rows belonging to this host and session, before applying a limit.
pub fn owned_rows_request(entity: &str, owner: &Owner) -> FetchRequest {
    FetchRequest::entity(entity).filter(owned_by(owner))
}

/// The namespace agent and project-store share.
///
/// Shared rather than private because a plugin's private namespace is derived
/// from its URL, so agent and project-store would otherwise be unable to see each
/// other's rows — which is the whole reason the store used to replicate its
/// state over a pipe.
///
/// One namespace for both entities rather than one each, because a batch of
/// mutations is a transaction over a single namespace and discovery has to move
/// a project's root and its todo list together.
pub const SHARED: &str = "shared:yinz";

/// The page size for list reads; lists may span any number of pages.
pub const LIST_FETCH_LIMIT: u32 = MAX_FETCH_ROWS;

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------
//
// A note that applies to all of them: declare the *minimum*. Adding an
// attribute later is a migration the host performs without being asked;
// removing one needs `allow_destructive`, a version above the stored one, and
// the original declaring plugin. So an attribute that is merely nice to have is
// cheaper to add later than to carry now.

/// A project in the registry, keyed by name.
///
/// `name` is both the uid and an ordinary attribute. The duplication is
/// deliberate — a uid cannot be selected or sorted on, and every caller looks a
/// project up by name.
///
/// `root` is indexed but not unique: nothing forbids two projects rooted at one
/// folder, and discovery resolves a cwd by longest matching root rather than by
/// assuming uniqueness.
pub fn project_def() -> EntityDef {
    EntityDef::new("project", 1)
        .attr(AttrDef::text("name").required())
        .attr(AttrDef::text("root").indexed().required())
        // Retained because dropping an existing attribute requires a destructive migration.
        .attr(AttrDef::text("firecracker_image"))
        .attr(AttrDef::text("worktree_dir"))
        .attr(AttrDef::text("remote"))
        // The snake_case strings serde already emits, rather than an integer
        // that would need a mapping table to read in the file.
        .attr(AttrDef::text("worktree_status"))
        .attr(AttrDef::text("common_dir"))
        .attr(AttrDef::text("provider_identity"))
        .attr(AttrDef::json("replaced_ids"))
        .attr(AttrDef::boolean("discovered").required().default(false))
}

/// One todo item. The tree is edges, not nesting: `parent_id` holds the store id
/// of the item above, and is null for a top-level item.
///
/// There is deliberately no `position` column. Siblings are only ever appended,
/// so `id` ascending *is* insertion order — and a stored ordering that nothing
/// maintains is a column that can go wrong. Nor is there a `depth`: it follows
/// from the parent chain, and a copy could disagree with it.
///
/// Nor is there an `item_id`. The store's own `id` is the id the model names in
/// a tool call, which is what retires the per-list `next_id` counter and the
/// allocation race that came with it.
pub fn todo_def() -> EntityDef {
    EntityDef::new("todo", 1)
        .attr(AttrDef::text("root").indexed().required())
        .attr(AttrDef::int("parent_id"))
        .attr(AttrDef::text("text").full_text().required())
        .attr(AttrDef::boolean("done").indexed().required().default(false))
}

/// One note. `root` is the normalized project root the note belongs to, as a
/// todo's is, or null for a note tied to no project at all.
///
/// Both `title` and `body` are full-text: a search for a word should find it
/// whichever of the two it is in, and FTS5 answers one row per note however
/// many of its columns match. A listing selects everything but `body`, which is
/// the one attribute with a real size.
///
/// The store's own `created_at` and `updated_at` are the note's timestamps;
/// nothing here duplicates them.
pub fn note_def() -> EntityDef {
    EntityDef::new("note", 1)
        .attr(AttrDef::text("root").indexed())
        .attr(AttrDef::text("title").full_text().required())
        .attr(AttrDef::text("body").full_text().required())
        .attr(AttrDef::text("author").required().default("user"))
        .attr(
            AttrDef::boolean("archived")
                .indexed()
                .required()
                .default(false),
        )
}

/// A note referencing one todo item. Its uid is `"{note_id}:{todo_id}"`, so
/// linking the same pair twice is one row. Both sides are indexed: a note's
/// items and an item's notes are each one lookup.
pub fn note_link_def() -> EntityDef {
    EntityDef::new("note_link", 1)
        .attr(AttrDef::int("note_id").indexed().required())
        .attr(AttrDef::int("todo_id").indexed().required())
}

/// One tag on one note, uid `"{note_id}:{tag}"`. A row per tag rather than a
/// text column of them, so a listing can be narrowed to a tag by the store.
pub fn note_tag_def() -> EntityDef {
    EntityDef::new("note_tag", 1)
        .attr(AttrDef::int("note_id").indexed().required())
        .attr(AttrDef::text("tag").indexed().required())
}

/// A note that one of the old JSON files has been read in already.
///
/// Written in the same transaction as the rows it describes, so the import is
/// atomic and idempotent. A marker rather than "is the table empty?", because
/// emptiness is a state a user can reach honestly — crossing off and clearing
/// the last todo would otherwise resurrect the file on the next launch.
pub fn import_marker_def() -> EntityDef {
    EntityDef::new("import_marker", 2)
        .attr(AttrDef::text("what").required())
        // The files this marker is about are on one machine's disk. Without the host, the first
        // machine to import would mark the job done for every machine sharing the database, and
        // the rest would never read their own files.
        .attr(AttrDef::text("host_id").indexed().required().default(""))
}

/// The write that records a one-time import as done, and the read that asks whether it is.
///
/// Shared by both plugins that import, rather than a copy each, because the pair only works if
/// the two agree about the uid — and they now have a `host_id` to agree about as well.
///
/// The files an import reads are on one machine's disk. Without the host in the key, the first
/// machine to import would mark the job done for everyone sharing the database, and the rest
/// would never read their own.
pub fn import_marker_mutation(host_id: &HostId, what: &str) -> Mutation {
    Mutation::put("import_marker")
        .uid(format!("{host_id}:{what}"))
        .set("what", what)
        .set("host_id", host_id)
        .build()
}

/// The host a marker written before the key carried one has, once the migration has filled the
/// column in. Empty rather than absent: the store cannot add a `NOT NULL` column without a
/// default, and this is the default the declaration gives it.
const LEGACY_HOST: &str = "";

/// This machine's rows, and the ones the `host_id` migration left naming no machine.
///
/// Adding the column was an `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT ''`, so every row
/// written before it came out of the migration naming nobody. A reader that wants those rows
/// counted as its own asks with this; strict readers use [`owned_by`] or, for jobs,
/// host equality without accepting legacy rows.
pub fn host_or_legacy(host_id: &HostId) -> Pred {
    Pred::eq("host_id", host_id).or(Pred::eq("host_id", LEGACY_HOST))
}

/// Markers for the named import that this machine may answer to: its own, and one written
/// before the key carried a host.
///
/// The tolerance is not decoration. The uid used to be the bare `what`, so every marker already
/// written came out of the migration naming no machine. Read strictly, a machine that had
/// imported years ago would be told it had not on the first launch after the upgrade — and the
/// legacy JSON is never unlinked, nor is `legacy_id` a dedupe key, so it would read the whole
/// archive in a second time. Use [`import_recorded`] rather than this directly; it claims a
/// marker of its own on the way past, without disturbing the one it found.
pub fn import_marker_request(host_id: &HostId, what: &str) -> FetchRequest {
    FetchRequest::entity("import_marker")
        .filter(host_or_legacy(host_id).and(Pred::eq("what", what)))
}

/// Whether this machine has already done the named import — writing a marker of its own on the
/// way past, and leaving alone any marker that predates the host key.
///
/// **A legacy marker is never deleted, and the accommodation is therefore permanent.** A bare
/// `''` row names no machine, so reading one is not evidence that *this* machine wrote it, and
/// nothing else in the store can supply the missing attribution: the rows a legacy import
/// created carry the same backfilled empty host. Deleting it on the strength of having read it
/// can erase another installation's import marker — the machine that
/// had genuinely imported found no marker on its next launch and read its entire archive in a
/// second time, with no dedupe key to collapse the duplicates.
///
/// This is why the move is *not* the one agent-proto's
/// `agents::rows::stale_unowned_delete_mutations` makes for latches and summaries, despite the
/// shape being the same. An unowned latch is litter, worth nothing to whoever wrote it; an
/// unowned import marker is the only record that somebody has already done an expensive,
/// non-idempotent thing.
///
/// The residual, accepted deliberately: a machine that joins a store already carrying a
/// pre-host marker is told the import is done and will not read its own legacy files. That is
/// what today's code does too — it returns `true` for exactly this case — so declining to delete
/// costs nothing extra, and the files stay on disk either way.
///
/// The claim itself still earns its keep. Once this machine's own row exists the fetch answers
/// from it, so the reasoning above is a single launch's worth of work rather than every
/// launch's.
pub fn import_recorded(backend: &impl Backend, host_id: &HostId, what: &str) -> bool {
    let Ok(rows) = backend.fetch(&import_marker_request(host_id, what)) else {
        // The store is unreachable, and the import would fail too. Claiming it is done is the
        // safe direction: a retry next launch is fine, a double import is not.
        return true;
    };
    if rows.is_empty() {
        return false;
    }
    let claimed = rows.rows.iter().any(
        |row| matches!(rows.get(row, "host_id"), Some(DataValue::Text(host)) if host == host_id.as_str()),
    );
    // Nothing to claim it *with* when this machine has no id of its own — which should not
    // happen, since the host is resolved before a plugin loads, but writing the legacy uid back
    // over the legacy row is not an improvement.
    if !claimed && !host_id.is_empty() {
        let _ = backend.apply(vec![import_marker_mutation(host_id, what)]);
    }
    true
}

/// A kind the user set on a pane by hand with `agent_register`, keyed by host, session *and*
/// pane id. Pruned as panes go, so a stale row names a pane id that has since been reused only
/// until the next snapshot.
///
/// The session is part of the key because this namespace is not one: every yinz server on a
/// machine shares one store, so a bare pane id made session A's pane 5 and session B's pane 5
/// into one row — each overwriting the other's kind, and A's prune deleting B's rows outright.
///
/// Host identity also distinguishes installations with the same session and pane ids.
/// `pane_id` stays indexed for the prune, but is not unique and never was.
pub fn agent_latch_def() -> EntityDef {
    EntityDef::new("agent_latch", 3)
        .attr(AttrDef::int("pane_id").indexed().required())
        .attr(
            AttrDef::text("yinz_session_name")
                .indexed()
                .required()
                .default(""),
        )
        .attr(AttrDef::text("host_id").indexed().required().default(""))
        .attr(AttrDef::text("kind").required())
}

/// What the daemon's model last said about an idle agent's pane, keyed by host, session and
/// pane id for the same reason [`agent_latch_def`] is. The activity hash is deliberately not
/// here: it is not stable across runs.
pub fn agent_summary_def() -> EntityDef {
    EntityDef::new("agent_summary", 3)
        .attr(AttrDef::int("pane_id").indexed().required())
        .attr(
            AttrDef::text("yinz_session_name")
                .indexed()
                .required()
                .default(""),
        )
        .attr(AttrDef::text("host_id").indexed().required().default(""))
        .attr(AttrDef::text("text").required())
        .attr(AttrDef::text("title").required().default(""))
        .attr(AttrDef::boolean("awaiting").required().default(false))
        // Epoch milliseconds this summary was made. Nullable for the same reason
        // `agent_run_entry.created_at_ms` is: a row written before the attribute existed has
        // no value for it, and migration only fills new columns in for rows written after.
        .attr(AttrDef::int("created_at_ms"))
}

/// One hook-backed agent session. Archive pages fetch `header`; only the selected row fetches
/// `payload`, while the scalar columns provide cheap predicates and ordering.
pub fn agent_run_def() -> EntityDef {
    EntityDef::new("agent_run", 3)
        .attr(AttrDef::int("pane_id").indexed().required())
        .attr(AttrDef::text("session_id").indexed().required().default(""))
        .attr(AttrDef::text("yinz_session_name").indexed())
        // The machine whose server recorded this run. Half of the run's owner, with
        // `yinz_session_name` — see agent-proto's `agents::rows::Owner`. A run adopted after
        // a warp is claimed by writing both, and the guarded close that hands it over names
        // both, so a session called `main` on one machine cannot close a run belonging to
        // `main` on another. Empty on every run written before the store could be shared
        // between machines, which reads as "this one", and was true when there was only one.
        .attr(AttrDef::text("host_id").indexed().required().default(""))
        // Indexed so a listing can ask for one agent's runs. `cwd` deliberately is not a
        // column at all: it lives in `header`, and narrowing by project is a guest-side pass
        // over the page rather than a predicate.
        .attr(AttrDef::text("kind").indexed().required())
        .attr(
            AttrDef::boolean("active")
                .indexed()
                .required()
                .default(true),
        )
        .attr(AttrDef::int("started_at_ms").required())
        .attr(AttrDef::json("header"))
        .attr(AttrDef::json("payload").required())
}

/// One retained hook event on a run's timeline, addressed by `(run_id, revision)`.
///
/// Its own entity rather than a field of [`agent_run_def`]'s `payload`: a run's timeline grows as the agent
/// works, and a blob has to be rewritten whole every time it does. A row per event costs one
/// small insert per hook, lets the panel read a run's newest entries without reading all of
/// them, and — the point of the split — removes the only reason the recorder had to cut an
/// entry's text down to a size a blob could carry.
///
/// `revision` is the run's own monotonic counter, which is what orders the timeline; `id`
/// order agrees with it, since a row is written when its entry is made.
pub fn agent_run_entry_def() -> EntityDef {
    EntityDef::new("agent_run_entry", 4)
        .attr(AttrDef::json("tool"))
        .attr(AttrDef::text("run_id").indexed().required())
        // Attribution rather than key: a `run_id` is unique across machines either way — the
        // ordinary shape carries the agent's own session uuid, and the fallback for a hook
        // that supplies none carries the machine (agent-proto's `agents::run_id`) — so entries
        // do not collide. This is what lets a search over every machine's history say where a
        // hit happened.
        .attr(AttrDef::text("host_id").indexed().required().default(""))
        .attr(AttrDef::int("revision").required())
        // The provider's own id for the event behind this entry, where it has one. Nullable:
        // an entry the daemon made itself — a summary somebody asked for — has no event.
        .attr(AttrDef::text("event_id"))
        .attr(AttrDef::text("kind").indexed().required())
        // Full-text, which is what makes a machine's whole agent history answerable: the
        // panel only ever reads one run's newest entries, but the model is asked things like
        // "have I had an agent look at this before", and that is a search over every entry
        // ever recorded. The index is external-content — it stores no second copy of the
        // text — and `rebuild_full_text` backfills every row already in the table when the
        // migration runs, so history recorded before this existed is searchable too.
        .attr(AttrDef::text("text").full_text().required())
        // Epoch milliseconds this entry was recorded. Nullable: an entry written before this
        // attribute existed has no value for it, and migration only fills new columns in for
        // rows written after the column exists. Indexed for the time-window filter and for
        // the ordering the substring fallback needs.
        .attr(AttrDef::int("created_at_ms").indexed())
}

/// Every entity in [`SHARED`]. Both plugins declare all of them on load — the
/// latch and summary rows are the service's alone to write, but a declaration
/// has to agree wherever it is made.
pub fn shared_defs() -> Vec<EntityDef> {
    vec![
        project_def(),
        todo_def(),
        note_def(),
        note_link_def(),
        note_tag_def(),
        import_marker_def(),
        agent_latch_def(),
        EntityDef::new("agent_assignment", 1)
            .attr(AttrDef::text("host_id").indexed().required())
            .attr(AttrDef::text("yinz_session_name").indexed().required())
            .attr(AttrDef::int("tab_id").required())
            .attr(AttrDef::int("project_id").required())
            .attr(AttrDef::text("project").required()),
        agent_summary_def(),
        agent_run_def(),
        agent_run_entry_def(),
    ]
}

// ---------------------------------------------------------------------------
// Guarding a write on what its author read
// ---------------------------------------------------------------------------

/// The revision an edit guards on: what the caller says it read, else what the row said when it
/// was fetched, and nothing at all when neither is a real revision.
///
/// Two revisions rather than one because they answer different questions. The row's own `rev`
/// comes from a fetch the writer made microseconds earlier, in the same straight-line function,
/// so guarding on it catches only a writer landing inside that window. The revision worth
/// guarding on is the one the *author of the text* saw, which for a model is several turns back
/// and for a script is however long it held the value — and that one can only travel in, as an
/// argument. Where a caller supplies it, it wins.
///
/// A caller's zero is treated as absent rather than as `REV_ABSENT`. A reader is never shown a
/// zero — `notes::note_json` and `todos::items_json` leave it out — so a zero arriving here is a
/// caller filling the field in rather than a claim that the row must not exist, and the store
/// refuses `REV_ABSENT` on an update anyway.
///
/// A row `rev` of zero means there is no row to guard: a note or an item that came from the
/// legacy file rather than the store. It must never come to mean "a fetch projected the column
/// away" — see `notes::LIST_COLUMNS`, which names `rev` for that reason.
pub fn guarded_rev(
    expect_rev: Option<DataRevision>,
    row_rev: DataRevision,
) -> Option<DataRevision> {
    expect_rev
        .filter(|rev| *rev > DataRevision::new(0))
        .or(Some(row_rev).filter(|rev| *rev > DataRevision::new(0)))
}

// ---------------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------------

/// What a agent plugin may do to the store.
///
/// A backend is bound to one namespace and stamps it onto every request, so no
/// caller has to remember [`SHARED`] — and no caller can read one namespace
/// while writing another, which a per-call namespace argument would allow.
pub trait Backend {
    fn define(&self, def: &EntityDef) -> Result<DataSchemaInfo, String>;
    fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String>;
    fn count(&self, request: &FetchRequest) -> Result<u64, String>;
    /// Apply a batch, all or nothing.
    ///
    /// The error is typed rather than a string so a caller can tell a conflict — somebody wrote
    /// a guarded row first, so read it again — from a request that will not work however often
    /// it is sent. `?` still converts it to a `String` for the callers that do not care.
    fn apply(&self, mutations: Vec<Mutation>) -> Result<ApplyReport, WriteError>;

    /// Watch a query, and take its current results while registering.
    ///
    /// The initial rows come back with the handle so a caller cannot miss a
    /// change in the gap between fetching and watching.
    fn watch(&self, request: &FetchRequest, mode: WatchMode) -> Result<DataWatchHandle, String>;

    /// Declare a group of entities, stopping at the first refusal.
    ///
    /// The error names the entity, because the likely cause is two plugins
    /// declaring the same one differently and the definition at fault is the
    /// only thing worth saying.
    fn define_all(&self, defs: &[EntityDef]) -> Result<(), String> {
        for def in defs {
            self.define(def)
                .map_err(|error| format!("could not declare `{}`: {error}", def.name))?;
        }
        Ok(())
    }
}

/// Fetch every page in the default id order, or fail without returning a partial list.
pub fn fetch_all(backend: &impl Backend, request: FetchRequest) -> Result<DataRows, String> {
    let mut request = request.limit(LIST_FETCH_LIMIT);
    let mut all = backend.fetch(&request)?;
    while all.truncated {
        let cursor = all
            .next_cursor
            .ok_or("cannot finish pagination: missing cursor")?;
        if request.after_id.is_some_and(|previous| cursor <= previous) {
            return Err("cannot finish pagination: cursor did not advance".into());
        }
        request = request.after_id(cursor);
        let page = backend.fetch(&request)?;
        all.rows.extend(page.rows);
        all.truncated = page.truncated;
        all.next_cursor = page.next_cursor;
    }
    Ok(all)
}

/// A reference to a backend is a backend, so a caller can hold one without
/// owning it — which is what lets a plugin keep its store in a field.
impl<T: Backend + ?Sized> Backend for &T {
    fn define(&self, def: &EntityDef) -> Result<DataSchemaInfo, String> {
        (**self).define(def)
    }

    fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String> {
        (**self).fetch(request)
    }

    fn count(&self, request: &FetchRequest) -> Result<u64, String> {
        (**self).count(request)
    }

    fn apply(&self, mutations: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
        (**self).apply(mutations)
    }

    fn watch(&self, request: &FetchRequest, mode: WatchMode) -> Result<DataWatchHandle, String> {
        (**self).watch(request, mode)
    }
}

/// Put the backend's namespace on a request the caller built without one.
fn stamped(namespace: Option<&str>, request: &FetchRequest) -> FetchRequest {
    let mut request = request.clone();
    request.namespace = namespace.map(str::to_owned);
    request
}

mod local {
    use super::*;
    use yinz_storage::plugin_data_store::PluginDataStore;

    /// The id [`Local`] hands back from `watch`. Non-zero so that a plugin
    /// storing "no watch" as zero can still tell the two apart.
    pub const LOCAL_WATCH_ID: DataWatchId = DataWatchId::new(1);

    /// The real store over a temporary file, for tests and for anything that
    /// wants to exercise this crate without a running yinz.
    ///
    /// Not a fake: this is the same engine, so a predicate that compiles here
    /// compiles in a session, and a truncation rule that bites here bites there.
    pub struct Local {
        store: PluginDataStore,
        namespace: String,
        owner: String,
        // Held only so the directory outlives the store.
        _dir: tempfile::TempDir,
    }

    impl Local {
        /// A backend over [`SHARED`], owned by `owner`.
        ///
        /// The owner matters only to destructive migrations, which this crate
        /// never performs — but two `Local`s can name different owners to prove
        /// that two plugins really can declare the same entity.
        pub fn shared(owner: &str) -> Self {
            Self::new(SHARED, owner)
        }

        /// A backend over a private namespace, standing in for the one the host
        /// would derive from a plugin URL.
        pub fn private(owner: &str) -> Self {
            Self::new(&format!("plugin:{owner}"), owner)
        }

        fn new(namespace: &str, owner: &str) -> Self {
            let dir = tempfile::tempdir().expect("a temporary directory for the store");
            Local {
                store: PluginDataStore::new(dir.path()),
                namespace: namespace.to_owned(),
                owner: owner.to_owned(),
                _dir: dir,
            }
        }

        /// A second backend onto the same file, as another plugin.
        ///
        /// This is how a test reproduces the thing the shared namespace exists
        /// for: two plugins, one database. It borrows the directory rather than
        /// owning it, so the original must outlive it.
        pub fn joined(&self, namespace: &str, owner: &str) -> Joined<'_> {
            Joined {
                store: PluginDataStore::new(self._dir.path()),
                namespace: namespace.to_owned(),
                owner: owner.to_owned(),
                _borrowed: std::marker::PhantomData,
            }
        }

        /// The namespace this backend writes to.
        pub fn namespace(&self) -> &str {
            &self.namespace
        }
    }

    /// A second view of another [`Local`]'s file. See [`Local::joined`].
    pub struct Joined<'a> {
        store: PluginDataStore,
        namespace: String,
        owner: String,
        _borrowed: std::marker::PhantomData<&'a ()>,
    }

    /// Both types do the same four things; only their lifetimes differ.
    macro_rules! impl_backend {
        ($t:ty) => {
            impl Backend for $t {
                fn define(&self, def: &EntityDef) -> Result<DataSchemaInfo, String> {
                    self.store
                        .define_entity(&self.namespace, &self.owner, def, false)
                        .map_err(|e| e.to_string())
                }

                fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String> {
                    self.store
                        .fetch(&self.namespace, &stamped(None, request))
                        .map_err(|e| e.to_string())
                }

                fn count(&self, request: &FetchRequest) -> Result<u64, String> {
                    self.store
                        .count(&self.namespace, &stamped(None, request))
                        .map_err(|e| e.to_string())
                }

                fn apply(&self, mutations: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
                    self.store
                        .apply(&self.namespace, &mutations)
                        .map(|outcome| outcome.report)
                        // The same split the host makes on the way back to a plugin, so a
                        // test sees the error a session would.
                        .map_err(|e| match e {
                            DataError::Conflict(message) => WriteError::Conflict(message),
                            other => WriteError::Failed(other.to_string()),
                        })
                }

                /// Takes the rows, but never notifies.
                ///
                /// Watch delivery is the plugin thread's job — the host queues
                /// the event rather than dispatching it inline — so there is
                /// nothing here to deliver it *to*. A test drives the change
                /// handler directly instead, which is also the only way to test
                /// what it does with rows it did not expect.
                fn watch(
                    &self,
                    request: &FetchRequest,
                    _mode: WatchMode,
                ) -> Result<DataWatchHandle, String> {
                    Ok(DataWatchHandle {
                        watch_id: LOCAL_WATCH_ID,
                        rows: self.fetch(request)?,
                    })
                }
            }
        };
    }

    impl_backend!(Local);
    impl_backend!(Joined<'_>);
}

pub use local::{Joined, Local, LOCAL_WATCH_ID};

// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "unit/store.rs"]
mod unit;

#[cfg(test)]
mod tests {
    use super::*;
    fn record_def() -> EntityDef {
        EntityDef::new("record", 2)
            .attr(AttrDef::text("title").required())
            .attr(AttrDef::text("plan"))
    }
    fn fixture_defs() -> Vec<EntityDef> {
        vec![record_def(), import_marker_def()]
    }

    #[test]
    fn record_plan_is_an_additive_migration() {
        let backend = Local::private("agents");
        let mut old = record_def();
        old.attrs.retain(|attr| attr.name != "plan");
        backend.define(&old).expect("old schema");
        backend
            .apply(vec![Mutation::put("record")
                .set("title", "old record")
                .build()])
            .expect("old archive");
        backend.define(&record_def()).expect("additive migration");
        let rows = backend
            .fetch(&FetchRequest::entity("record"))
            .expect("read migrated record");
        assert_eq!(
            rows.get(&rows.rows[0], "title"),
            Some(&DataValue::Text("old record".to_owned()))
        );
        assert_eq!(rows.get(&rows.rows[0], "plan"), Some(&DataValue::Null));
    }

    /// Every definition has to survive the same validation the host applies, or
    /// the failure would land at plugin load with no test having noticed.
    #[test]
    fn every_definition_validates() {
        for def in shared_defs().iter().chain(fixture_defs().iter()) {
            def.validate()
                .unwrap_or_else(|e| panic!("`{}` is not a valid entity: {e}", def.name));
        }
    }

    /// Full-text search is only legal on an entity that declares a full-text
    /// attribute, and three of ours are searched. A note is searched on two
    /// columns, and one row must come back however many of them match.
    #[test]
    fn the_searched_entities_declare_a_full_text_attribute() {
        assert_eq!(todo_def().full_text_attrs().len(), 1);
        assert_eq!(note_def().full_text_attrs().len(), 2);
    }

    /// The marker entity is declared in both namespaces, so it has to be one
    /// definition rather than two that could drift.
    #[test]
    fn the_import_marker_is_declared_in_both_namespaces() {
        assert!(shared_defs().iter().any(|d| d.name == "import_marker"));
        assert!(fixture_defs().iter().any(|d| d.name == "import_marker"));
    }

    /// The machine these tests are on. Named rather than empty, so a helper that forgot to
    /// write `host_id` fails a test instead of matching the pre-migration rows.
    const HOST: &str = "test-host";

    /// A marker written before the uid carried a host still counts, and is claimed *beside*
    /// rather than instead of.
    ///
    /// Without the first half, the launch after the upgrade is told the import has not happened
    /// on every machine at once. The old JSON is never unlinked and nothing dedupes a record or a
    /// todo, so the whole archive would double. The second half is the same worry from the other
    /// end: the legacy row names no machine, so deleting it is deleting somebody else's only
    /// record that they have already imported — see
    /// `a_legacy_marker_is_not_deleted_out_from_under_the_machine_that_wrote_it`.
    #[test]
    fn an_import_marker_written_before_the_host_key_still_counts_and_is_claimed() {
        let backend = Local::private("agents");
        // The definition as it was, and a marker under the uid it used.
        backend
            .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
            .expect("the old definition still declares");
        backend
            .apply(vec![Mutation::put("import_marker")
                .uid("chats")
                .set("what", "chats")
                .build()])
            .expect("a marker from before the host key");
        backend
            .define_all(&fixture_defs())
            .expect("the new definition migrates rather than being refused");

        assert!(
            import_recorded(&backend, &HostId::new(HOST), "chats"),
            "the import would have been done a second time"
        );

        // Ours is there now, and the legacy row is still there too: it might be another
        // machine's, and nothing here can tell.
        let hosts = marker_hosts(&backend);
        assert_eq!(hosts, vec![String::new(), HOST.to_owned()]);

        // And asking again is a read, not another claim.
        assert!(import_recorded(&backend, &HostId::new(HOST), "chats"));
        assert_eq!(marker_hosts(&backend), hosts);
    }

    /// The bug the claim used to have, and the one that costs a whole archive.
    ///
    /// Two host identities in one store. Host A imported years ago, before the marker carried a host.
    /// Host B has never imported. B launches first: it is told the import is done — which is
    /// the accommodation working as designed, since a bare marker is one every machine answers
    /// to — and claims a marker of its own. What it must *not* do is take A's row away with it,
    /// because then A's next launch finds nothing that answers for it and reads its entire
    /// archive in a second time, with no dedupe key to collapse the result.
    #[test]
    fn a_legacy_marker_is_not_deleted_out_from_under_the_machine_that_wrote_it() {
        let backend = Local::private("agents");
        backend
            .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
            .expect("the old definition still declares");
        backend
            .apply(vec![Mutation::put("import_marker")
                .uid("chats")
                .set("what", "chats")
                .build()])
            .expect("host A imported, before the host key");
        backend.define_all(&fixture_defs()).expect("migrated");

        assert!(
            import_recorded(&backend, &HostId::new("host-b"), "chats"),
            "a marker naming no machine answers for every machine"
        );
        assert_eq!(
            marker_hosts(&backend),
            vec![String::new(), "host-b".to_owned()],
            "host B may claim a row of its own, but not take the legacy one"
        );

        assert!(
            import_recorded(&backend, &HostId::new("host-a"), "chats"),
            "host A, which actually did the import, must still be told so"
        );
    }

    /// Every marker in the store, by host, sorted — the shape these tests assert on.
    fn marker_hosts(backend: &Local) -> Vec<String> {
        let rows = backend
            .fetch(&FetchRequest::entity("import_marker"))
            .expect("fetched");
        let mut hosts: Vec<String> = rows
            .rows
            .iter()
            .map(|row| match rows.get(row, "host_id") {
                Some(DataValue::Text(host)) => host.clone(),
                other => panic!("a marker with no host: {other:?}"),
            })
            .collect();
        hosts.sort();
        hosts
    }

    /// The other direction, which the tolerance must not swallow: a machine that has not
    /// imported has to be told so, and another machine's claimed marker is not an answer for us.
    #[test]
    fn a_machine_that_has_not_imported_is_told_so() {
        let backend = Local::private("agents");
        backend.define_all(&fixture_defs()).expect("declared");
        assert!(!import_recorded(&backend, &HostId::new(HOST), "chats"));

        backend
            .apply(vec![import_marker_mutation(
                &HostId::new("another-host"),
                "chats",
            )])
            .expect("another machine imported");
        assert!(
            !import_recorded(&backend, &HostId::new(HOST), "chats"),
            "another machine's files are not ours"
        );
        assert!(
            import_recorded(&backend, &HostId::new("another-host"), "chats"),
            "and theirs still are"
        );
    }

    /// Each import is asked about separately, so claiming one does not answer for the other.
    #[test]
    fn claiming_one_marker_leaves_the_other_alone() {
        let backend = Local::private("agents");
        backend
            .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
            .expect("declared");
        backend
            .apply(vec![
                Mutation::put("import_marker")
                    .uid("chats")
                    .set("what", "chats")
                    .build(),
                Mutation::put("import_marker")
                    .uid("prompts")
                    .set("what", "prompts")
                    .build(),
            ])
            .expect("two markers from before the host key");
        backend.define_all(&fixture_defs()).expect("migrated");

        assert!(import_recorded(&backend, &HostId::new(HOST), "chats"));
        assert_eq!(
            marker_hosts(&backend),
            vec![String::new(), String::new(), HOST.to_owned()],
            "only `chats` should have been claimed, and neither legacy row touched"
        );
        assert!(import_recorded(&backend, &HostId::new(HOST), "prompts"));
        assert_eq!(
            marker_hosts(&backend),
            vec![
                String::new(),
                String::new(),
                HOST.to_owned(),
                HOST.to_owned()
            ]
        );
    }

    #[test]
    fn declaring_the_shared_entities_creates_them() {
        let backend = Local::shared("project_store");
        for def in shared_defs() {
            let info = backend.define(&def).expect("declared");
            assert!(info.created, "`{}` should have been created", def.name);
            assert_eq!(info.namespace, SHARED);
        }
    }

    /// The case the whole "one set of constructors" rule exists for: whichever
    /// plugin loads second declares the same entities again, and must find that
    /// a no-op rather than a migration.
    #[test]
    fn a_second_plugin_declaring_the_same_entities_changes_nothing() {
        let first = Local::shared("project_store");
        first.define_all(&shared_defs()).expect("first declared");

        let second = first.joined(SHARED, "agents");
        for def in shared_defs() {
            let info = second.define(&def).expect("second declared");
            assert!(!info.created, "`{}` already existed", def.name);
            assert!(info.attrs_added.is_empty(), "nothing should be added");
            assert!(!info.full_text_rebuilt, "the index should be left alone");
        }
    }

    /// And the failure that rule prevents. A definition missing an attribute
    /// reads as a request to drop it, which the store refuses — this is what
    /// would happen at load, in whichever plugin lost the race, if the two ever
    /// built their own `EntityDef`s.
    #[test]
    fn a_narrower_redeclaration_is_refused() {
        let first = Local::shared("project_store");
        first.define(&project_def()).expect("declared");

        let narrower = EntityDef::new("project", 1)
            .attr(AttrDef::text("name").required())
            .attr(AttrDef::text("root").indexed().required());
        let error = first
            .joined(SHARED, "agents")
            .define(&narrower)
            .expect_err("dropping attributes should be refused");
        assert!(
            error.contains("discard the stored values"),
            "the refusal should say what is at stake, got: {error}"
        );
    }

    #[test]
    fn project_worktree_directory_migrates_and_round_trips() {
        let store = Local::shared("agents");
        let mut legacy = project_def();
        legacy.attrs.retain(|attr| attr.name != "worktree_dir");
        store.define(&legacy).unwrap();
        store
            .apply(vec![Mutation::put("project")
                .uid("old")
                .set("name", "old")
                .set("root", "/old")
                .build()])
            .unwrap();
        store.define(&project_def()).unwrap();
        let rows = store.fetch(&FetchRequest::entity("project")).unwrap();
        let mut projects = crate::projects::from_rows(&rows);
        assert_eq!(projects["old"].worktree_dir, None);
        projects.get_mut("old").unwrap().worktree_dir = Some("../trees with spaces".into());
        store
            .apply(vec![crate::projects::to_mutation(&projects["old"])])
            .unwrap();
        let loaded =
            crate::projects::from_rows(&store.fetch(&FetchRequest::entity("project")).unwrap());
        assert_eq!(loaded, projects);
        assert_eq!(
            crate::projects::from_rows(&crate::projects::to_rows(&projects)),
            projects
        );
        projects.get_mut("old").unwrap().worktree_dir = None;
        store
            .apply(vec![crate::projects::to_mutation(&projects["old"])])
            .unwrap();
        assert_eq!(
            crate::projects::from_rows(&store.fetch(&FetchRequest::entity("project")).unwrap()),
            projects
        );
    }

    /// The reason the namespace is shared at all: one plugin's write is another
    /// plugin's read, with no message in between.
    #[test]
    fn two_plugins_see_the_same_rows() {
        let store = Local::shared("project_store");
        store.define_all(&shared_defs()).expect("declared");
        let agent = store.joined(SHARED, "agents");
        agent.define_all(&shared_defs()).expect("declared");

        store
            .apply(vec![Mutation::put("project")
                .uid("yinz")
                .set("name", "yinz")
                .set("root", "/src/yinz")
                .build()])
            .expect("written");

        let rows = agent
            .fetch(&FetchRequest::entity("project"))
            .expect("fetched");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows.get(&rows.rows[0], "root"),
            Some(&DataValue::Text("/src/yinz".to_owned()))
        );
    }

    /// A private namespace is the opposite guarantee, and transcripts rely on
    /// it: another plugin declaring the same entity name gets its own table.
    #[test]
    fn a_private_namespace_is_not_shared() {
        let agent = Local::private("agents");
        agent.define_all(&fixture_defs()).expect("declared");
        agent
            .apply(vec![Mutation::put("record")
                .uid("1")
                .set("title", "a record")
                .build()])
            .expect("written");

        let other = agent.joined("plugin:somebody_else", "somebody_else");
        other.define(&record_def()).expect("declared");
        assert!(
            other
                .fetch(&FetchRequest::entity("record"))
                .expect("fetched")
                .is_empty(),
            "a private namespace should not show another plugin's rows"
        );
    }

    /// The backend owns the namespace, so a request that names the wrong one
    /// cannot reach the store. Without the stamp this would read `shared:yinz`.
    #[test]
    fn a_backend_overrides_the_namespace_on_a_request() {
        let agent = Local::private("agents");
        agent.define_all(&fixture_defs()).expect("declared");

        let misdirected = FetchRequest::entity("record").in_namespace(SHARED);
        assert!(
            agent.fetch(&misdirected).is_ok(),
            "the backend's namespace should win"
        );
    }

    /// `define_all` stops at the first refusal and says which entity it was,
    /// because the message is all a plugin author gets.
    #[test]
    fn define_all_names_the_entity_that_failed() {
        let backend = Local::shared("project_store");
        backend.define(&todo_def()).expect("declared");

        let narrower = EntityDef::new("todo", 1).attr(AttrDef::text("root").indexed().required());
        let error = backend
            .joined(SHARED, "agents")
            .define_all(&[project_def(), narrower])
            .expect_err("the second definition should be refused");
        assert!(
            error.starts_with("could not declare `todo`"),
            "got: {error}"
        );
    }
    #[test]
    fn incomplete_pagination_never_returns_a_partial_todo_tree() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        struct Pages(RefCell<VecDeque<Result<DataRows, String>>>);
        impl Backend for Pages {
            fn fetch(&self, _: &FetchRequest) -> Result<DataRows, String> {
                self.0
                    .borrow_mut()
                    .pop_front()
                    .expect("unexpected extra fetch")
            }
            fn define(&self, _: &EntityDef) -> Result<DataSchemaInfo, String> {
                unreachable!()
            }
            fn count(&self, _: &FetchRequest) -> Result<u64, String> {
                unreachable!()
            }
            fn apply(&self, _: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
                unreachable!()
            }
            fn watch(&self, _: &FetchRequest, _: WatchMode) -> Result<DataWatchHandle, String> {
                unreachable!()
            }
        }
        let page = |cursor: Option<i64>| {
            Ok(DataRows {
                truncated: true,
                next_cursor: cursor.map(DataRowId::new),
                ..Default::default()
            })
        };
        for (pages, message) in [
            (vec![Err("fetch failed".into())], "fetch failed"),
            (vec![page(None)], "missing cursor"),
            (
                vec![page(Some(1)), Err("fetch failed".into())],
                "fetch failed",
            ),
            (vec![page(Some(1)), page(None)], "missing cursor"),
            (vec![page(Some(1)), page(Some(1))], "did not advance"),
        ] {
            let backend = Pages(RefCell::new(pages.into()));
            assert!(crate::todos::fetch_list(&backend, "/work")
                .unwrap_err()
                .contains(message));
            assert!(backend.0.borrow().is_empty());
        }
    }
}
