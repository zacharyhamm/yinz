> **Status: parked.** This design is implemented through milestone 6 on the `assembly-lines` branch
> (no agent step yet) and is not on `main`. See `docs/SELF_PROGRAMMING.md` for why it is
> parked and what it would build on when picked up.

# Assembly Lines — deterministic workflows with decision gates

## Context

Nothing workflow-shaped exists in this fork. `hocus` has an in-memory `Job` scheduler
(`default-plugins/hocus/src/message.rs:159`, `transitions.rs:426 tick_jobs`) that fires prompts on
a timer, but it has no graph, no retry model and no run history. (Since R15 of the
self-programming roadmap its jobs persist as `job` rows in `shared:hocus` and survive a reload;
the rest of this sentence still holds.) Meanwhile the fork already has every *part* a workflow engine needs: a
transactional SQLite store with schema migration and reactive watches
(`zellij-utils/src/plugin_data_store/`), agent spawn/drive/monitor
(`default-plugins/hocus/src/tool_loop.rs:1089`, `hocus-proto/src/agents.rs`), git worktree identity
(`hocus-agentd/src/poll.rs:105`, `hocus-proto/src/projects.rs`), and a forms library
(`zellij-tile/src/ui_components/forms/`).

**Assembly lines** add the missing piece: a declarative, explicitly-enumerable graph of steps and
decision gates, executed by a native runtime that journals every transition — so a run is resumable
after a crash and auditable after the fact.

Motivating case: a local merge queue — rebase a worktree on main, hand conflicts to a coding agent,
continue the rebase, loop while conflicts remain, fast-forward main, delete the worktree and branch.

**Shape (decided):** native runtime in `zellij-server` + wasm plugin UI; KDL declares the graph and
wasm modules decide anything non-trivial; command steps first, a dedicated agent step type last.

---

## Crate layout

The feature gets its own crate so the parser and the state machine compile and test in seconds
without dragging in the server.

| Crate / path | Contents | Depends on |
|---|---|---|
| **`zellij-assembly/`** *(new)* | `input/` the model, `kdl/` parser + graph validation, `engine/` the pure state machine, `gate/` the wasmi runner | `zellij-utils`, `wasmi`, `wasmi_wasi`, `zellij-assembly-gate` |
| **`zellij-assembly-gate/`** *(new)* | `GateInput`, `Verdict`, `register_gate!` — the guest-facing crate for writing gates. Must stay dependency-light: builds for `wasm32-wasip1` **and** the host triple | `serde`, `serde_json` only |
| `zellij-server/src/assembly_line/` | `mod.rs` thread + instruction enum, `exec.rs` tokio command spawn, `store.rs` projection + resume scan | `zellij-assembly`, server internals |
| `src/` (the binary) | offline `list`/`validate`/`graph`/`status`/`log`/`replay` | `zellij-assembly` directly — no server needed |
| `default-plugins/assembly-lines/` *(new)* | the dashboard plugin | `zellij-tile`, `assembly-proto` |

Gate wire types live in `zellij-assembly-gate` and are re-exported by `zellij-assembly`, so host and
guest cannot drift without a fourth crate.

Two mechanical consequences: add both crates to `members` in the root `Cargo.toml`, and add them to
`workspace_members()` in `xtask/src/main.rs:36` (`build: false` for `zellij-assembly`, which is a
host crate; the plugin and the builtin gates need `build: true`).

Note this puts a KDL parser outside `zellij-utils`, mildly against CLAUDE.md's "zellij-utils =
shared config/KDL parsing". The trade is deliberate: the wasmi gate runner has no business in
zellij-utils, and splitting spec from engine is worse than keeping the feature whole. It still uses
`ConfigError`/`KdlError` from `zellij-utils/src/input/config.rs` and the macros from
`zellij-utils/src/kdl/mod.rs`, so diagnostics stay identical to layouts.

---

## Design

### 1. The spec language

A line lives in `<config>/assembly-lines/<name>.kdl`, discovered exactly like layouts
(mirror `Layout::list_available_layouts` at `zellij-utils/src/input/layout.rs:1275` and
`home::get_layout_dir`/`default_layout_dir` in `zellij-utils/src/home.rs:65`).

The merge queue, in full:

```kdl
assembly-line name="merge-queue" {
    description "Rebase a worktree on main, resolve conflicts with an agent, merge, clean up"

    // Mutual exclusion. Two runs whose `lock` expands to the same string never run
    // concurrently — a merge queue without this is not a queue.
    lock "repo:${repo_root}"

    inputs {
        input "worktree"  required=true
        input "branch"    required=true
        input "repo_root" required=true
        input "base"      default="main"
        input "agent"     default="claude"
    }

    start "fetch"

    step "fetch" {
        run "git" { args "fetch" "origin" "${base}"; cwd "${worktree}"; }
        on "success" goto="rebase"
        on "failure" goto="fail" message="could not fetch ${base}"
    }

    step "rebase" {
        max-visits 10                       // the loop bound
        run "git" { args "rebase" "origin/${base}"; cwd "${worktree}"; }
        on "success" goto="merge"
        on "failure" goto="classify"
    }

    // A gate is a pure wasm function over the run context and the previous step's result.
    gate "classify" {
        module "assembly:git-rebase-classifier"
        config { conflict-marker "CONFLICT"; }
        outcomes "conflicts" "other"
        on "conflicts" goto="resolve"
        on "other"     goto="fail" message="rebase failed for a reason we do not handle"
    }

    step "resolve" {
        max-visits 10
        // Phase 7 turns this into `agent { ... }`; a command is enough to ship.
        run "${agent}" {
            args "-p" "Resolve the rebase conflicts in this worktree. Do not run git rebase --continue."
            cwd "${worktree}"
            timeout 900
        }
        // Idempotency guard: exit 0 means the work is already done, skip the step.
        guard "git" { args "diff" "--check"; cwd "${worktree}"; }
        on-interrupt "retry-if-guard"       // crash policy; this is the default
        on "success" goto="continue"
        on "failure" goto="ask-human"
        on "timeout" goto="ask-human"
    }

    step "continue" {
        max-visits 10
        run "git" { args "-c" "core.editor=true" "rebase" "--continue"; cwd "${worktree}"; }
        on "success" goto="merge"
        on "failure" goto="classify"        // more conflicts: back round the loop
    }

    gate "ask-human" {
        manual
        prompt "The agent could not resolve the conflicts. What now?"
        outcomes "retry" "abandon"
        on "retry"   goto="resolve"
        on "abandon" goto="abort"
    }

    step "abort" {
        run "git" { args "rebase" "--abort"; cwd "${worktree}"; }
        on "any" goto="fail" message="rebase abandoned"
    }

    step "merge" {
        run "git" { args "merge" "--ff-only" "${branch}"; cwd "${repo_root}"; }
        capture "merged_sha" from="stdout" trim=true
        on "success" goto="cleanup"
        on "failure" goto="fail" message="fast-forward merge refused"
    }

    step "cleanup" {
        run "git" { args "worktree" "remove" "${worktree}"; cwd "${repo_root}"; }
        on "success" goto="delete-branch"
        on "failure" goto="fail"
    }

    step "delete-branch" {
        run "git" { args "branch" "-d" "${branch}"; cwd "${repo_root}"; }
        on "any" goto="done"
    }
}
```

Surface rules:

- **Targets are uniform.** `goto=` names a declared `step`/`gate`, or one of three reserved
  terminals `done` / `fail` / `abort`. Those three are rejected as step names.
- **Outcomes.** A command step produces `success` (exit 0), `failure`, or `timeout`; `on "any"`
  catches all. `on-exit 2 goto="…"` maps a specific code. A gate produces exactly one of its
  declared `outcomes`.
- **Loops are bounded by `max-visits`.** Entering a step more times than its bound is a run
  failure, not a silent stop.
- **Variables** are `${name}`, resolved from inputs then captures. `capture "x" from="stdout"`
  (or `stderr`, or `exit-code`) with optional `trim=true`. Gates can also return captures.
- **Idempotency** is `guard` — a command whose exit 0 means "already done, skip to the success
  edge without re-running". It is also the crash-recovery probe.

Model in `zellij-assembly/src/input/mod.rs`: `AssemblyLine`, `Node::{Step, Gate}`,
`StepSpec { run: RunCommand, guard: Option<RunCommand>, timeout, max_visits, captures, edges,
on_interrupt }`, `GateSpec { source: GateSource::{Module(RunPluginLocation-ish), Manual}, config,
outcomes, edges }`, `Edge { outcome, target: Target::{Node(String), Done, Fail, Abort}, message }`.

Reuse `RunCommand` (`zellij-utils/src/input/command.rs:60`) for both `run` and `guard` — **it needs
a new `env: BTreeMap<String,String>` field**, added as an additive `#[serde(default)]` field for the
reason documented at `command.rs:75-84`.

### 2. Parsing and validation

`zellij-assembly/src/kdl/mod.rs`, structured exactly like `KdlLayoutParser`
(`raw` + `file_name`, one `parse()` entry point), with an `AssemblyLine::from_kdl(raw, file_name)`
wrapper copying the error-remapping in `Layout::from_kdl` (`kdl/mod.rs:5266`). Add
`ConfigError::new_assembly_line_kdl_error(msg, offset, len)` beside the two existing constructors in
`zellij-utils/src/input/config.rs`. Reuse the macros in `kdl/mod.rs`
(`kdl_get_string_property_or_child_value!`, `kdl_children_nodes!`, `kdl_parsing_error!`, …) and the
command-parsing shape of `KdlLayoutParser::parse_pane_command` / `parse_args` / `parse_path`
(`kdl_layout_parser.rs:447,407,433`); reuse `parse_plugin_user_configuration` (`:357`) verbatim for
a gate's `config` block. Whitelist properties with `is_a_valid_*` / `assert_*` methods the way the
layout parser does.

The graph checks are what make "deterministic" real. All are **parse-time errors**:

1. Duplicate node name; reserved name used as a node name.
2. `goto=` naming an undeclared node.
3. A node unreachable from `start`.
4. A non-total outcome mapping — a command step missing either `any` or both `success` and
   `failure`; a gate not mapping every declared outcome.
5. **Every non-trivial strongly-connected component must contain at least one node with
   `max-visits`.** Computed with Tarjan over the edge set. This is the termination proof, and it is
   why the `rebase → classify → resolve → continue → classify` cycle is legal while an unbounded
   one is not.
6. `${var}` referencing neither a declared input nor any declared capture name.
7. `max-visits` above a global cap (`MAX_VISITS_CEILING = 1000`).

Tests: `zellij-assembly/src/kdl/unit/` with insta snapshots of each rejection's miette output, plus a
test asserting the merge-queue example above parses and round-trips.

### 3. The wasm gate ABI

**A separate narrow wasmi runner inside the engine — not the plugin host.** `wasmi` and `wasmi_wasi`
are already `zellij-server` deps (`zellij-server/Cargo.toml:45,50`). Reasons to keep gates out of the
plugin host: a gate with no host imports cannot observe or affect anything but its input, so replay
is exact and audit is meaningful; it sidesteps the per-client plugin lifecycle, the permission model,
and the broadcast-pipe duplicate-instance hazard documented in `hocus-proto/src/wire.rs`; and a
synchronous fuel- and memory-metered call is far simpler than an async pipe round-trip.

Contract: the module is a **plain WASI command**. It exports `_start`, reads a JSON `GateInput` on
stdin, writes a JSON `Verdict` on stdout. Nothing else — no preopens, no env, no args, clocks and
randomness denied. That means a gate is debuggable with `wasmtime run gate.wasm < input.json`.

```jsonc
// stdin
{ "run_id": "…", "line": "merge-queue", "step": "classify", "visit": 3,
  "context": { "branch": "…", "base": "main" },
  "config":  { "conflict-marker": "CONFLICT" },
  "previous": { "step": "rebase", "outcome": "failure", "exit_code": 1,
                "stdout": "…", "stderr": "…" } }
// stdout
{ "outcome": "conflicts", "capture": { "files": "a.rs b.rs" }, "reason": "3 conflicted paths" }
```

An outcome outside the gate's declared set fails the run — the graph stays explicit.

Runner: `zellij-assembly/src/gate/mod.rs`. Reuse the stdio-pipe mechanism from
`PluginLoader::create_wasi_ctx` (`zellij-server/src/plugins/plugin_loader.rs:463`) —
`Arc<Mutex<VecDeque<u8>>>` for stdin/stdout — and the memory limiter pattern from
`zellij-server/src/plugins/plugin_limits.rs`. Add wasmi fuel metering with a per-invocation budget.
Cache compiled `Module`s by path+mtime, as `PluginCache` does.

Guest crate `zellij-assembly-gate` exposes `GateInput`, `Verdict`, and `register_gate!(f)`, which
expands to a `main` that does
stdin → `serde_json` → `f` → stdout. **Because a gate is a pure `fn(GateInput) -> Verdict`, its
logic unit-tests as ordinary Rust on the host triple**; only one integration test needs to compile a
fixture to wasm and push it through the real runner.

Builtin gates ship in `default-gates/` and resolve from `assembly:<name>` urls, mirroring `zellij:`
for plugins. Start with `git-rebase-classifier`; `regex-match` and `json-path` are obvious follow-ons.

### 4. Journal and state

**Reuse `zellij-utils/src/plugin_data_store/` in namespace `shared:assembly-lines`** rather than a
new DB. `PluginDataStore::apply` (`plugin_data_store/mod.rs:245`) is a host-side transactional API
the native engine can call directly; it brings schema migration (`migrate.rs`), FTS over the journal,
and — the decisive reason — **free reactivity for the UI plugin** via `data_watch` /
`Event::PluginDataChanged`, so the plugin needs no new plugin API to stay live.

One caveat to handle: watch refresh is currently driven from the plugin thread
(`zellij-server/src/plugins/zellij_exports.rs:4024 notify_watchers`). Factor that into a free
function over `(&PluginDataStore, &ThreadSenders, namespace, touched_entities)` in the plugins module
and call it from the engine after every `apply`.

Both an append-only journal **and** a projected state row — the journal is the truth, the row is the
index the UI reads.

- `al_run` — uid = run uuid. `line`, `spec_hash`, `spec_source` (the full KDL captured at
  start, so replay is exact even if the file changes), `state`
  (`pending|running|blocked|succeeded|failed|aborted`), `current_step`, `visits` (json),
  `inputs` (json), `context` (json), `lock_key`, `started_at`, `ended_at`, `message`, `seq`.
- `al_event` — the journal. `run` (indexed), `seq`, `at`, `kind`, `step`, `payload` (json,
  full-text). Kinds: `run_started`, `step_entered`, `guard_skipped`, `command_started`,
  `command_finished`, `gate_invoked`, `gate_verdict`, `manual_requested`, `manual_answered`,
  `capture_set`, `transition`, `run_interrupted`, `run_finished`.
- `al_gate_request` — open manual gates. `run`, `step`, `prompt`, `outcomes` (json), `answer`.

Entity defs live in a shared crate so the engine and the UI plugin cannot disagree — copy
`default-plugins/hocus-proto/src/store.rs` exactly: constructor functions are the only way to build a
def, plus a `Backend` trait with a wasm `Host` impl and a `cfg(not(target_family="wasm"))` `Local`
impl over a real `PluginDataStore` on a tempfile so tests hit genuine SQLite. Put it in
`default-plugins/assembly-proto/` (or a module inside `hocus-proto` if you'd rather not add a crate).

Journal entry + state update go in **one `apply` call**, so a transition is atomic. Command output is
truncated to 64 KiB in the journal (`MAX_FETCH_BYTES` is 2 MiB); full output goes to
`<cache>/assembly-lines/<run_id>/<seq>.{out,err}`.

**Resume.** The DB is in the global data dir, not session-scoped, so a run survives a full zellij
restart. On engine start, load every run in `running`/`blocked`. A `command_started` with no
matching `command_finished` means the server died mid-step: append `run_interrupted`, then apply the
step's `on-interrupt` policy — `retry-if-guard` (default: run the guard; exit 0 ⇒ take the success
edge, else re-enter the step), `retry`, or `fail`. This is precisely what makes the merge queue safe:
a `git rebase --continue` interrupted by a crash is re-probed by `git diff --check` rather than blindly
repeated.

**Replay/audit.** `zellij assembly-line log <run>` prints the journal.
`zellij assembly-line replay <run>` re-runs each recorded gate invocation against its recorded input
and asserts the same verdict — cheap precisely because gates are pure, and it turns "deterministic"
into something the tool can demonstrate.

### 5. The engine

The state machine itself is in `zellij-assembly/src/engine/`: **pure**, no I/O at all —
`step(&RunState, Stimulus) -> Transition { effects, journal, next }`, where `Effect` is
`RunCommand`/`RunGuard`/`InvokeGate`/`AskHuman`/`Finish` and the caller performs them.

`zellij-server/src/assembly_line/` is the thin I/O shell around it:

| File | Contents |
|---|---|
| `mod.rs` | `assembly_line_thread_main(bus)`, `enum AssemblyLineInstruction`, the effect-performing loop |
| `exec.rs` | command spawn + timeout on the shared tokio runtime |
| `store.rs` | entity defs, projection of journal → `al_run`, resume scan |

Wired in `zellij-server/src/lib.rs` beside the background-jobs thread (`:1973-2277`) —
channel, `Bus`, `thread::Builder::new().name("assembly_line")`, `Exit` on shutdown, and a
`to_assembly_line` sender on `ThreadSenders`.

`AssemblyLineInstruction`: `StartRun { line: AssemblyLine, inputs, client_id }`,
`CommandFinished { run_id, step, exit_code, stdout, stderr, timed_out }`,
`GuardFinished { … }`, `AnswerGate { run_id, step, outcome }`, `CancelRun(run_id)`,
`ListRuns { … }`, `Exit`.

The **pure core in `zellij-assembly` is the main testability move** — it mirrors the planner/runner split in
`default-plugins/hocus-store/src/ops.rs` (pure `StoreView → (Vec<Mutation>, Reply)` planner, 3-line
runner). Loop bounds, guards, edge resolution, visit counting and interrupt policy are all exercised
with no zellij, no SQLite and no processes running.

Command execution does **not** go through `BackgroundJob::RunCommand` — that routes its result to a
plugin id via `PluginInstruction::Update`, which is the wrong destination. Instead factor the
`tokio::process::Command` spawn out of `zellij-server/src/background_jobs.rs:377` into a shared helper
(adding `env` and `timeout`), run it on `zellij-server/src/global_async_runtime.rs`, and send
`CommandFinished` back to the engine's own bus.

Concurrency: the engine thread is a serial event loop; only the command futures are concurrent, so
many runs interleave safely. A run whose `lock` key is held queues in `pending` until the holder
finishes.

### 6. Control surfaces

**CLI `zellij assembly-line` (alias `al`)** in `zellij-utils/src/cli.rs`:

- Offline, handled early in `src/main.rs` with `exit(0)` like `Command::Recordings` (`main.rs:75`),
  delegating to `src/commands.rs` — `list`, `validate <file|name>` (miette diagnostics),
  `graph <name>` (mermaid/ascii), and also `status` / `log` / `replay`, which just **read the SQLite
  file directly**; no session required.
- Session-reaching — `run <name> --set k=v`, `answer <run> <outcome>`, `cancel <run>` — via a new
  `Action::AssemblyLine(AssemblyLineCommand)`, which means arms in
  `zellij-utils/src/input/actions.rs:801 actions_from_cli`, `zellij-utils/src/plugin_api/action.rs`,
  `zellij-utils/src/ipc/protobuf_conversion.rs`, and a route in `zellij-server/src/route.rs`.

**Plugin UI**: a new builtin `assembly-lines` plugin (register per the four edits — `BUILTIN_PLUGIN_NAMES`
in `zellij-utils/src/consts.rs:107`, root `Cargo.toml` members, `workspace_members()` in
`xtask/src/main.rs:36`, and the crate's own `.cargo/config.toml`). It renders a run list, a journal
detail view, and answers manual gates with `Select` from
`zellij-tile/src/ui_components/forms/select.rs`.

Notably it needs **no new plugin API at all**: it reads via the existing `data_watch`/`data_fetch`
shims on `shared:assembly-lines`, and writes by calling the existing `run_action(Action::AssemblyLine(…))`.
Both `SharedPluginData` and `RunActionsAsUser` are auto-granted to builtins, and it must never call
`request_permission()` if run headless (`hocus-agentd/src/main.rs` documents why).

### 7. Where this pushes back on the brief

The brief said workflows "could call out to WASM modules at each step". I'd keep wasm to **pure
decisions only**. Effectful steps should be commands: giving gate modules the host API would mean
re-exporting the whole plugin surface, would destroy replayability (a module that ran `git push` cannot
be re-run during audit), and would drag the permission model back in. Effects go through commands and,
later, agent steps; wasm decides.

---

## Milestones

Each is independently shippable and independently testable.

1. **The `zellij-assembly` crate: spec + parser + validator.** New crate scaffold (workspace member,
   xtask entry), `input/`, `kdl/`, `ConfigError::new_assembly_line_kdl_error`, `env` on `RunCommand`,
   and the offline `list`/`validate`/`graph` CLI. *Proves:* insta snapshots of every rejection above;
   the merge-queue example parses; a cycle without `max-visits` is rejected; an unreachable node is
   rejected. Runs as `cargo test -p zellij-assembly` in seconds, with no server in the build.
2. **Pure engine core.** `zellij-assembly/src/engine/` only, no wiring. *Proves:* the merge-queue line
   driven through scripted stimuli reaches `done`; the loop trips at `max-visits`; a passing guard
   skips its step; each `on-interrupt` policy behaves.
3. **Engine thread + store + command steps.** Thread wiring, entity defs, journal/projection,
   `exec.rs`, `Action::AssemblyLine`, `run`/`status`/`log`/`cancel`. *Proves:* an integration test in
   `zellij-integration-tests` using `write_git_repository`/`write_git_worktree`
   (`src/test_env.rs:282`) runs a real conflicting rebase end to end with a shell script standing in
   for the agent.
4. **Wasm gates.** `zellij-assembly/src/gate/`, the `zellij-assembly-gate` guest crate, builtin
   `git-rebase-classifier`. *Proves:* pure-Rust unit tests of the classifier; one wasm round-trip
   through the real runner; a fuel-exhaustion test; `replay` reproduces recorded verdicts.
5. **Manual gates + plugin UI.** `al_gate_request`, the `assembly-lines` builtin plugin on the forms
   library. *Proves:* an integration test that blocks on a manual gate, answers it via
   `zellij assembly-line answer`, and completes.
6. **Crash resume.** Resume scan on engine start. *Proves:* an integration test that kills the server
   mid-step, restarts, and asserts the run resumes under each policy.
7. **Agent step type.** `agent { kind="claude"; prompt="…"; done-when=… }` built on
   `hocus-proto::agents::{Activity, Summary}` — `Summary.awaiting` already distinguishes
   *blocked on the user* from *finished*, which is exactly the transition signal a step needs.

## Verification

- `cargo x test` after every milestone — that is what CI runs. Narrow the inner loop with
  `cargo test -p zellij-assembly --all-features` (from the repo root, never with `CARGO_TARGET_DIR`
  set); milestones 1–2 and 4 are fully covered by that one fast command.
- `cargo x integration-test -- -E 'test(assembly)'` for milestones 3, 5, 6.
- `cargo x build -p` before any test that loads a plugin or gate at runtime.
- End to end by hand: `cargo x run -- -l default`, then in a scratch repo with a conflicting
  worktree, `zellij assembly-line validate merge-queue`, `zellij assembly-line run merge-queue
  --set worktree=… --set branch=… --set repo_root=…`, watch it in the `assembly-lines` plugin pane,
  and confirm with `zellij assembly-line log <run>` that the journal shows the conflict loop.
- `cargo x format` and `cargo x assets` (new builtin plugin/gate wasm) before calling it done.
