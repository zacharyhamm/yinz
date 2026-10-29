//! Shared entity definitions and backend operations for project and agent data.
//! With `test-support`, `Local` uses the real SQLite store for native model tests. Scheme plugins send
//! data commands through their confined host bridge and keep transactions in guest code.

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
/// Shared project and agent data. A mutation batch is a transaction over one namespace.
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

/// Records that one of the old JSON files has been read in already.
///
/// Written in the same transaction as the rows it describes, so the import is
/// atomic and idempotent. A marker rather than "is the table empty?", because
/// emptiness is a state a user can reach honestly by forgetting all projects.
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

#[cfg(any(test, feature = "test-support"))]
mod local {
    use super::*;
    use yinz_storage::plugin_data_store::PluginDataStore;

    /// Put the backend's namespace on a request the caller built without one.
    fn stamped(namespace: Option<&str>, request: &FetchRequest) -> FetchRequest {
        let mut request = request.clone();
        request.namespace = namespace.map(str::to_owned);
        request
    }

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

#[cfg(any(test, feature = "test-support"))]
pub use local::{Joined, Local, LOCAL_WATCH_ID};

// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "unit/store.rs"]
mod unit;

#[cfg(test)]
#[path = "unit/store_inline_tests.rs"]
mod tests;
