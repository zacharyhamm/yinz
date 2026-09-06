This document details the different components of the code base in plain English. The intention is to provide a high level description rather than a more detailed drill-down of each internal mechanism.

## Shared library extraction

The extraction from `zellij-utils` is in progress. These crates now own independent parts of the shared library:

| Crate | Current ownership |
|---|---|
| `zellij-types` | Canonical pane IDs, terminal, plugin, and client ID wrappers, positions, mouse input, environment models, plugin-data contracts, recording summaries, Kitty placeholders, keys, palettes and styles, animation, text helpers, diagnostic context payloads, pane geometry, direction and resizing models, agent records and launch inputs, and command targets |
| `zellij-runtime` | Error presentation, thread-local context, channels, logging, paths, environment access and host-terminal capture, host identity, downloads, checkpoint publication, terminal key interpretation and encoding, and vendored terminal input parsing |
| `zellij-storage` | Plugin-data engine, SQLite and Postgres backends, recording storage, and `DataAccess` interfaces |

`zellij-types` has no Zellij dependencies. Runtime depends on types. Storage depends on types, without configuration parsing.
Consumers import moved modules from their owning crates. Temporary re-exports in `zellij-utils` support the remaining migration.
Configuration, plugin API, IPC, session, and CLI extraction remain unfinished. Most shared models still reside in `zellij-utils`.
Public domain scalars also need further migration to semantic newtypes.

`zellij-types::pane_size` owns pane dimensions, sizes, offsets, insets, and viewports.
`zellij-types::geometry` owns directions, resize strategies, split sizes, percentages, and floating-pane coordinates.
`zellij-types::agent` owns agent records and launch inputs. Hook interpretation remains in utils.
`zellij-types::command` owns command payloads, process targets, and Git checkout requests. Process execution remains in the host crates.
Consumers and the SDK import these models from types. Utils no longer re-exports them.
Fixed JSON and protobuf fixtures cover their existing representations.

`zellij-types::diagnostics::ErrorContext` holds the serializable six-entry instruction stack.
Its `add_call` method changes only that value. Runtime's `errors::add_call` also publishes the result to the current task or thread.
Use `errors::update_thread_ctx` to publish a received context and `errors::format_context` to format it for a diagnostic.
Color KDL parsing and serialization remain parser-owned free functions. Color models and pure conversion helpers reside in types.
Style KDL serialization also uses parser-owned free functions. Key string forms live in types and serve config and protocol adapters.
Runtime key interpretation receives a binding predicate, so runtime does not depend on configuration or actions.
Clap validates theme hue choices in the options adapter. The shared `ThemeHue` model has no Clap dependency.

The checkpoint reader retains its lock while configuration loading reads layout and pane-content files.
Runtime tests cover publication and lock behavior. Parser tests cover pane-content loading under that lock.

Run `cargo x test -p zellij-types -p zellij-runtime -p zellij-storage` for the extracted crates.
Run `cargo x postgres-test --build` followed by `cargo x postgres-test --test` for storage against Postgres.
The full handoff gate remains `cargo x ci check`.


## Data access

The server constructs one host-only `DataAccess` provider and shares it with the plugin runtime
and session recorder. Plugin data and recordings use backend-neutral synchronous interfaces
(`zellij-storage/src/data_access.rs`), and `DataAccess::from_backends` takes one backend for each.

Plugin data has two real backends. SQLite is the default: one `plugin-data.db` in the data dir.
Setting `plugin_data_url` to a `postgres://` URL (config file only, never the CLI or the client
socket — it may carry a password) points the store at that database instead, in the schema
named by `plugin_data_schema`, so every machine using it sees the same rows. Recordings stay in
SQLite either way; `DataAccess::postgres` is the pairing.

The two backends share one SQL engine (`zellij-storage/src/plugin_data_store/`): the predicate
compiler, query builders, migrations and registry are written once against an `Executor`, and a
`Dialect` names what the engines spell differently — placeholders, column types, `LIKE` versus
`ILIKE`, and the full-text index (FTS5 with triggers on SQLite, a generated `tsvector` column
with a GIN index on Postgres). Where the engines' defaults disagree — text collation, null
ordering, NUL bytes in text — the store picks one behaviour and enforces it on both, and the
store's test suite runs against both (`cargo x postgres-test`). CLI recording commands construct
the default SQLite provider at their entrypoint.

### Sharing a store between servers

The store has never been per-session: one `plugin-data.db` serves every zellij server on the
machine, and a Postgres one serves every machine pointed at it. Two things follow, and both are
in the data rather than in the engine.

**Rows about a running server say which one.** A pane id means nothing outside the server that
handed it out, and a session name is unique only within a machine — two machines can each run a
session called `main`. So the entities that are about a live server (`agent_latch`,
`agent_summary`, `agent_run`, `job`) carry a `host_id` beside `zellij_session_name`, and
`hocus_proto::agents::rows::Owner` is the pair. The id is resolved once at server startup by
`zellij_runtime::host_identity`: `ZELLIJ_HOST_ID`, then the `host_id` config option, then
`<data dir>/host-id`, then a newly minted uuid written there. It identifies a *data dir on a
machine*, which is the granularity that matters — two servers sharing a data dir share the pane
ids and session names it qualifies. Plugins read it off `PluginIds` beside the hostname, which
stays the label to show rather than the key to store.

The same reasoning reaches ids and reads, not just columns. A uid built from a pane id needs the
machine in it — `rows::pane_uid` does this, and so does the fallback shape of `agents::run_id`,
which would otherwise let two servers mint one name for two agents' timelines and interleave
them under an unguarded put. And a read that narrows to a *folder* has to narrow to a machine
too: `/home/me/work` on a build box is not this project, so `run_history::matches_root` takes
the row's `host_id` beside the run's ssh `host`, and `run_history::RootScope` pairs the root with
the machine so a caller cannot ask for one without the other.

**An empty `host_id` is a row from before the column, and what to do with one differs.** The
store migrates an entity unasked, and adding the column was an `ALTER TABLE ADD COLUMN ... NOT
NULL DEFAULT ''`, so every row already stored came out naming no machine. There was only one
machine then, so `""` reads as "this one" — but only where adopting the row is the right answer.
An `agent_run` is history: `rows::owned_or_unclaimed` accepts it on the read and on the guarded
close, and `rows::claim_unowned_runs` stamps the machine on so the run's next ordinary write
narrows the row. `run_history::Filter::pred` accepts it too, but only when the filter names a
session as well — paired that way it is the same bargain `owned_or_unclaimed` makes, while a
question about one machine alone cannot be answered by a row naming none. An `import_marker`
adopts it on the read and stops there: `store::import_recorded` writes this machine's own marker
but **never deletes the legacy row**. A bare `''` names no machine, so reading one is no evidence
that this machine wrote it, and deleting it takes away another machine's only record that it has
already imported — after which that machine reads its whole legacy archive in a second time, with
nothing to dedupe the result. The cost of keeping it is milder and is accepted: a machine that
joins a store already carrying a pre-host marker is told the import is done and will not read its
own files, which is what tolerating the row means in the first place. An `agent_latch` or
`agent_summary` is the opposite:
it names a pane in a server that is gone, so `rows::stale_unowned_delete_mutations` deletes it
rather than adopting it, and `rows::owned_by` stays strict. A `job` is left alone entirely —
firing a dead server's prompts at whoever upgrades first is worse than dropping them.

**Writes can say what they expected to find.** Every row carries a `rev`, which the store
advances on every write that touches it, and a `Put` or `Update` may name the `rev` it expects —
a compare-and-set, refused with `DataError::Conflict` and rolling the whole batch back. Used
where a write replaces something composed from an earlier read (a todo's text, a note's body),
and deliberately not where it sets a flag, since two writers agreeing is not a conflict.
`ApplyReport.per_mutation` reports rows affected per mutation, for callers guarding by hand.

**The `rev` has to be the one the value's author read.** A writer that fetches the row, builds
the new value and guards on the `rev` it just fetched has guarded a window microseconds wide:
the row already carries the other writer's revision, so the write that should be refused lands.
The revision worth guarding on is the one whoever composed the value was looking at, which for a
model is several turns back — so it travels outward in what the reader is shown and back inward
as an argument. `todos::items_json` and `notes::note_json` emit it, `todo_edit` and `note_edit`
take it as `expect_rev`, and `store::guarded_rev` prefers it to the row's own. `note_edit`'s
reaches the tags as well as the text: a tag set is replaced whole, so it is composed from a
reading like any wording, and `notes::plan_set_tags` adds a guarded update of the note itself to
a batch that would otherwise touch only the tag rows. One call that edits both guards once —
two guarded updates of a row in one batch is the second finding the revision the first bumped. A caller that
supplies none falls back to the fresh read, which is the old behaviour and nearly no guard at
all. Note that a fetch naming its columns loses the ones the store adds unasked, `rev` among
them, so a projection can silently turn a guarded write into an unguarded one —
`notes::LIST_COLUMNS` names `rev` for that reason.

**What is not handled:** a watch is only re-evaluated by writes made in the same server process
(`refresh_watches` has one caller, in the local apply path), so on a shared Postgres another
machine's write does not wake a watcher here. There is no change feed and no polling fallback.
Recordings are likewise untouched: they stay in SQLite, are written by one owning thread, and
their `recording_sessions` rows are keyed by a bare local rowid with no host column — that would
need one before any Postgres backend for them.

## Screen (zellij-server/src/screen/)
The Screen is responsible for managing the relationship between the panes currently displayed on screen.
It is in charge of actions such as: 
  * Coordinating the resizing of panes
  * Creating new panes
  * Closing existing panes (and filling up their space by other panes)

## Terminal emulation (zellij-terminal/)

`zellij-terminal` owns the grid and rows, terminal characters, selection and search, hyperlinks, ANSI helpers, Sixel, and Kitty graphics.
It also owns output serialization, host-query types, component parsing, and damage geometry.
`PaneId` lives in `zellij-types::pane_id`, with `TerminalId` and `PluginId` payloads,
and is re-exported here. Host code and the plugin SDK use that same enum. The shared internal IDs
(`TerminalId(u32)`, `PluginId(u32)`, and `ClientId(u16)`) live in `zellij-types::internal_ids`
and are re-exported by the consuming crates. The server retains its existing re-export paths.
The pane enum preserves its JSON variants and protobuf values. Other public plugin IDs,
IPC, supervisor messages, and persistence records still keep primitive numbers;
internal code wraps incoming IDs and unwraps them when constructing boundary payloads.
Neither `zellij-terminal` nor `zellij-process` depends on the server or on the other extracted crate.

`TerminalPane`, `PluginPane`, pane managers, frames, tabs, screen coordination, and plugin hosting remain in `zellij-server`.
The terminal crate parses and renders terminal state. The server coordinates panes, clients, plugins, and threads.

## Terminal Pane (zellij-server/src/panes/terminal_pane.rs)
The TerminalPane represents a pane on screen that connects to a single pty (pseudo terminal) and (presumably) has one shell or other program running inside it.
The TerminalPane has two main roles:
  * Keeping track of the Scroll, which represents the buffer of lines in this terminal
  * Interpreting the ANSI/VT instructions coming from the pty in order to adjust the styling of characters, change the cursor position, etc.

### Scroll (zellij-server/src/panes/terminal_pane.rs and zellij-terminal/src/panes/grid.rs)
The Scroll holds the terminal buffer and is in charge of:
  * Keeping track of the viewport (which part of it we see) this can change when we scroll up/down
  * Keeping track of the cursor position
  * Controlling line-wrapping

### Terminal Character (zellij-terminal/src/panes/terminal_character.rs)
The `TerminalCharacter` represents a single character in the pane. It holds the char itself on the one hand, and an internal `CharacterStyles` struct representing the styling of this character.
This struct derives the `Copy` trait for performance reasons, because it is moved around quite a bit (eg. when line wrapping).

### How a terminal emulator draws characters?
The ANSI/VT instructions received from the pty include style instructions (eg. change foreground colour to red) as well as instructions to reposition the cursor (eg. go to line x/y) and to perform certain actions with the current buffer on-screen (eg. delete all lines after cursor).

When an instruction is received to change the character style, we know that all characters printed from now on should use that style. This is true until we move to a new line (with a newline instruction or a go to instruction) or until we receive a style reset code for this specific style. There are several kinds of reset codes that apply to different styles.

It's important to understand that these styles are ongoing relative to the current cursor position. It might be useful to imagine them as brushes: "pick up the red brush", "put down the bold brush", etc.

This is important to note because the styles themselves are saved only on characters that have already been printed. If we receive an instruction to change the style to blue, then print a (blue) character, then receive another instruction to move to a new line, print another (plain) character and then come back, the style would be reset. We would have to receive a new instruction to change the style to blue in order for the next character to be blue.

## Boundaries (zellij-server/src/ui/boundaries.rs)
The Boundaries refer to those lines that are drawn between terminal panes. A few things that happen here:
  * The Rect trait is here so that different panes can implement it, giving boundaries a generic way to calculate the size of the pane and draw boundaries around it.
  * Here we use the [unicode box drawing characters](https://en.wikipedia.org/wiki/Box-drawing_character) in order to draw the borders. There's some logic here about combining them together for all possible combinations of pane locations.

## Animation (zellij-types/src/animation.rs, zellij-terminal/src/ui/damage.rs)
Anything on screen that moves over time — toast notifications sliding in and out, the bell flash easing off a pane frame, the plugin loading dots — is built from the same two pieces.

`zellij-types/src/animation.rs` holds the pure part: an `Easing` curve, a `Tween` carrying one scalar from where it is to where it is going, and `blend` for interpolating a `PaletteColor`. The rule that makes it work is that **nothing in it reads the clock** — `now` is always a parameter. One frame therefore samples every live animation at a single instant, and a test drives an animation by handing in fabricated `Instant`s rather than by sleeping.

The frames themselves come from one shared clock: `BackgroundJob::StartAnimationClock` runs a `tokio::time::interval` that sends `ScreenInstruction::AnimationTick`. That tick advances *nothing*. It only asks for a render, and `Screen::render_to_clients` samples everything at the instant it composites. This is what makes the result frame-rate independent: a tick that arrives late, twice, or not at all costs a frame and never a position, so an animation always takes the wall-clock time it promised even when the screen thread is busy or renders are being held back.

`Screen::animation_clock_period` is the one place that decides what the clock does, derived from the animated state rather than from a registration list, so the clock cannot get stuck running over an idle session. It answers with a period rather than a yes/no, because the surfaces differ: continuous motion — a toast sliding, a bell fading — needs `FRAME_PERIOD` (~60fps), while the plugin loading dots are a four-character "still working" pulse that only changes every `DOT_PERIOD` (500ms). Holding the clock at 60fps for the dots would run the whole `render_to_clients` path sixty times a second for as long as any plugin is loading, which for a plugin that never finishes is forever. The aggregate is computed on the screen rather than requested by each surface so that a plugin load starting mid-slide cannot slow the clock underneath a moving toast.

Every tick asks for a frame, *including* the one that finds everything settled. That last frame is the one that actually paints the settled state: renders are debounced by `REPAINT_DELAY_MS`, so the frame a still-animating tick asks for is composited up to that long before the deadline it was aiming at. Without one more tick past the deadline, a dismissing toast would be left painted a sliver short of gone — and never reaped, since `reap_settled_notifications` only runs inside a frame — until unrelated output happened to repaint.

Anything drawn *over* a pane — currently just toasts — needs one more thing. A pane only emits cells its own grid thinks are dirty, so the moment a moving overlay stops covering a cell, that cell keeps whatever the overlay last drew there. `zellij-terminal/src/ui/damage.rs` diffs what the overlay painted last frame against what it is painting now, and `render_to_clients` repairs the difference *before* the panes render, narrowed to the panes actually overlapping it. An overlay that is only growing produces no damage at all, so arriving is free and only leaving costs repaints.

## Process ownership (zellij-process/)

`zellij-process` owns pane supervisors, local and remote protocols, SSH transport, the broker, and the Firecracker provider.
It also defines `AsyncReader` and the command-resolution helpers. It depends on shared utilities, not on the server.
Supervisor exit callbacks carry a `TerminalId`. The server converts it to `PaneId::Terminal` before calling its existing callback.
The server retains OS implementations, PTY scheduling, session coordination, and its existing public module paths through re-exports.
The `zellij-firecracker` binary calls the process crate directly.

## PTY Bus (zellij-server/src/pty.rs)
The PtyBus keeps track of several asynchronous streams that read from pty sockets (eg. /dev/pts/999), parse those bytes into ANSI/VT events and send them on to the Screen so that they can be received in the relevant TerminalPane.

A pane on another machine is spawned off the pty thread. Its `RunCommand` carries a `PaneTarget` (`Local`, `Ssh { host }` or `Firecracker { vm, image, worktree }`), and any spawn, layout, layout override or re-run whose command `defers_spawn()` — a remote target that is not held on start — goes to a worker thread as one `SpawnJob`. The worker asks the OS layer to `resolve_spawn` (an ssh host is used as is; a Firecracker VM is booted, or found, by the provider and its ssh endpoint returned as a `RemoteEndpoint`, separate from the target the pane shows) and then `spawn_resolved` (open the pane on the broker and register its transport), and sends one `PtyInstruction::SpawnResolved` back with the readers. The pty thread finishes the job exactly as it would a local spawn, so a VM that takes a minute to boot or a host that is slow to answer never stalls another pane's input. A layout's results keep one slot per run instruction so they line up with the layout when it is applied; a layout that cannot be applied past some slot closes the remote panes already started for the slots after it. Re-runs of one pane are serialised through a per-pane `RerunState`: a rerun asked for while one is off-thread queues behind it, whatever its target, so two commands never stream into one pane, and a result landing on a pane closed meanwhile is torn down through `clear_terminal_id`.

## Pane supervisors (zellij-process/src/pty_host/)
Each pane's process is owned by a small supervisor process ("pty-host") rather than by zellij-server. The supervisor calls `openpty`, forks the pane's command, and hands the server a dup of the pty *master* over `SCM_RIGHTS`. Everything downstream of that descriptor — the async reader, window-size ioctls, backpressure on writes — works exactly as it did when the server owned the pty.

What the split buys is lifetime. The child belongs to the supervisor, so a server that exits or is killed does not take its panes with it. The supervisor reads a closed control channel as "detached" rather than "shut down" and starts draining the pty into a bounded ring buffer, so the child never blocks on a full buffer. Only an explicit `Terminate` ends a supervisor.

Because a detached supervisor is still holding a live pane, it can be picked back up. It binds a named socket, and an `Attach` on that socket re-keys it to the attaching server's pane, hands the master over again, and replays whatever the ring caught in the meantime. Two things are built on that one primitive:

  * `zellij action warp-pane --to-session <name>` moves a pane between sessions. The source releases the supervisor without killing it, serializes the pane's screen, scrollback and agent record, and sends the other session an `AdoptPane`; the target attaches and replays them into a new pane. The agent's *history* is not in that payload and does not need to be: it lives in `agent_run` rows in the plugin data store, which is not per session — one file per machine, or one Postgres shared between machines — so what has to travel is the run's id rather than the run. That rides on the record, as `AgentInfo.run_id` — see `docs/AGENT_HOOKS.md`. The losing session closes the run under a predicate naming itself as its owner — the machine and the session both, since a session name alone would match another machine's session of that name — and the gaining session reopens it by id, so whichever write lands second the entries survive. Nothing else about the process changes — same pid, same working directory, same open files. A local child inherits its supervisor id as `ZELLIJ_PANE_REF`; because `Attach` re-keys the supervisor's current session and terminal id, an implicit `agent-report` can resolve that stable id after the move instead of using the stale per-session values in the child's environment.
  * `zellij action warp-tab --to-session <name>` moves a whole tab the same way, releasing every supervisor in it and sending an `AdoptTab`. What travels alongside the panes is a *layout* and each pane's agent record — run id and all, so each pane's history is picked up the same way — rather than their pane sizes, reconstructed from their geometry by the same code session resurrection uses — so the tab re-flows to fit a session whose terminal is a different size instead of arriving clipped. The tab's plugin panes are the one thing that does not survive intact: they have no process to hand over, so they ride along as `Run::Plugin` nodes and the receiving session starts them afresh in the same slots.
  * `zellij pty-host adopt <id>` does the same thing with no source session at all, which is how a pane that outlived a crashed server gets back on screen.

The controlling option is `detachable_panes`. With it off, panes are spawned in-process as before and neither operation is available.

A **remote pane** warps too, by the same shape but not the same primitive: its supervisor is on the far machine, behind the broker, so there is no local socket to attach to. What the broker offers instead is already there for reconnects — panes live in a registry keyed by a token, independent of any connection, and `Resume { pane_token, received }` re-attaches to one with a byte-exact replay. A warp is that same resume with a *different server* on the other end, each having built its own ssh forward. So the pane's address travels as `{host, pane_token, received}` (plus its `RunCommand`, which a resume does not carry and an `AttachAck` would have) in place of the supervisor socket, and the adopting session resumes through its own forward.

The one thing that had to be added is `Release`. The broker frees a pane when the old connection's blocking read returns, which is not a moment the releasing side can name — the adopting session's `Resume` races it and is refused as a double attach. `Release` makes the detach happen before the acknowledgement, exactly as `ServerToHostMsg::Release` does for a local supervisor, so the token can be handed over the instant the release returns. Recovering a remote pane stranded by a failed handoff is `zellij pty-host list --host <machine>` and `zellij pty-host adopt --host <machine> <token>`: the token only ever existed in the releasing server's memory and the broker's registry, so the broker is the only thing that can still answer.

A **Firecracker pane** is a remote pane whose SSH endpoint comes from a configured provider.
Zellij keeps the VM name, image, and host worktree on `RunCommand`. It calls the provider's
idempotent `ensure` operation for each launch. It then feeds the returned endpoint into the remote
broker path. A canonical host worktree identifies a VM. Sessions hold leases on that VM, and the
provider stops it after the last lease ends. See [`FIRECRACKER.md`](FIRECRACKER.md) for the
provider contract and lifecycle.
