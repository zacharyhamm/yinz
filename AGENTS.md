<!-- BEGIN swamp managed section - DO NOT EDIT -->
# Project

This repository is managed with [swamp](https://github.com/swamp-club/swamp).

## Rules

1. **Search before you build.** When automating AWS, APIs, or any external service: (a) search community extensions with `swamp extension search <query>` — prefer `@swamp/*` official extensions first, (b) search local/installed types with `swamp model type search <query>`, (c) if a community extension exists, install it with `swamp extension pull <package>` instead of building from scratch, (d) extend an existing type if it covers the domain but lacks the method you need, (e) only create a custom extension model in `extensions/models/` as a last resort. Use the `swamp` skill for guidance. The `command/shell` model is ONLY for ad-hoc one-off shell commands, NEVER for wrapping CLI tools or building integrations.
2. **Extend, don't be clever.** When a model covers the domain but lacks the method you need, extend it with `export const extension` — don't bypass it with shell scripts, CLI tools, or multi-step hacks. One method, one purpose. Use `swamp model type describe <type> --json` to check available methods.
3. **Use the data model.** Once data exists in a model (via `lookup`, `start`, `sync`, etc.), reference it with CEL expressions. Don't re-fetch data that's already available.
4. **CEL expressions everywhere.** Wire models together with CEL expressions. Always prefer `data.latest("<name>", "<dataName>").attributes.<field>` over the deprecated `model.<name>.resource.<spec>.<instance>.attributes.<field>` pattern.
5. **Verify before destructive operations.** Always `swamp model get <name> --json` and verify resource IDs before running delete/stop/destroy methods.
6. **Prefer fan-out methods over loops.** When operating on multiple targets, use a single method that handles all targets internally (factory pattern) rather than looping N separate `swamp model method run` calls against the same model. Multiple parallel calls against the same model contend on the per-model lock, causing timeouts. A single fan-out method acquires the lock once and produces all outputs in one execution. Check `swamp model type describe` for methods that accept filters or produce multiple outputs.
7. **Extension npm deps are bundled, not lockfile-tracked.** Swamp's bundler inlines all npm packages (except zod) into extension bundles at bundle time. `deno.lock` and `package.json` do NOT cover extension model dependencies — this is by design. Always pin explicit versions in `npm:` import specifiers (e.g., `npm:lodash-es@4.17.21`).
8. **Reports for reusable data pipelines.** When the task involves building a repeatable pipeline to transform, aggregate, or analyze model output (security reports, cost analysis, compliance checks, summaries), create a report extension. Use the `swamp` skill for guidance.
9. **"Workflow" means a swamp workflow.** In this repository the word "workflow" (and "create/run/execute/validate/debug workflow", "automate", "orchestrate", "automated/nightly job") refers to a swamp workflow — a declarative YAML DAG of model-method steps authored via `swamp workflow create`. Load and follow the `swamp` skill for these requests. Do NOT interpret these as a request to build an agent task list, spin up worktrees, or schedule a cron/remote agent. Only use those orchestration mechanisms when the user explicitly names one (e.g. "task list", "subagent", "worktree", "cron", "remote agent") or explicitly asks you to do the work yourself step by step rather than author a swamp workflow.
10. **Use swamp, don't bypass it.** Always work through swamp commands — don't go around them with raw shell tools. Use `swamp data query` to find data, not `grep`/`find` on `.swamp/` files. Use model methods to interact with resources, not `curl`/`aws`/`gcloud`/`kubectl` when a model type already wraps that API — check with `swamp model type search`. Use `swamp help` for CLI discovery, not guesswork. Composing with swamp output is fine (e.g. piping `--json` through `jq`) — the anti-pattern is bypassing swamp entirely.
11. **Inspect reports after failures.** When a model method or workflow run fails, inspect its generated reports before retrying or changing definitions. Reports run even on failure and capture structured diagnostics — error messages, execution status, arguments, and data output pointers. Use `swamp report get @swamp/method-summary --model <model> --json` for method failures or `swamp report get @swamp/workflow-summary --workflow <workflow> --json` for workflow failures. Run `swamp help report get` to confirm current retrieval syntax.

## Skills

**IMPORTANT:** Always load swamp skills, even when in plan mode. The skills provide
essential context for working with this repository.

- `swamp` - Swamp CLI — models, workflows, data, vaults, extensions, publishing, repos, reports, issues, and troubleshooting
- `swamp-getting-started` - Interactive onboarding for new swamp users

## Getting Started

**IMPORTANT:** At the start of every conversation, run
`swamp model search --json`. If no models are returned (empty result), you MUST
immediately invoke the `swamp-getting-started` skill before doing anything else.
This walks new users through an interactive onboarding tutorial.

If models already exist, start by using the `swamp` skill to work with
swamp models.

## Commands

Use `swamp --help` to see available commands. For a machine-readable JSON
schema of the CLI (commands, options, arguments) intended for agent
consumption, run `swamp help [<command>...]` — e.g. `swamp help` returns
the full tree, and `swamp help model method run` scopes to a subtree.
<!-- END swamp managed section -->

# AGENTS.md

Terminal workspace, Rust cargo workspace. Server/client split over IPC, plus builtin
plugins compiled to wasm.

## Build & test — use `cargo xtask`, not bare `cargo`

```sh
cargo x build            # builds plugins (wasm32-wasip1) then the binary
cargo x test             # the whole workspace in one run (needs cargo-nextest)
cargo x lint             # clippy at -D warnings; --fix applies the suggestions
cargo x format           # rustfmt; --check to verify instead of rewrite
cargo x ci check         # the pre-handoff gate: format --check, lint, test, integration-test
cargo x run -- -l strider
cargo x integration-test # whole-app tests (needs cargo-nextest)
cargo x bench            # render throughput benchmarks (criterion); not part of the gate
cargo x remote-test --build && cargo x remote-test --test  # remote panes, real ssh in docker
cargo x firecracker-test --build && cargo x firecracker-test --test  # real VM through /dev/kvm
cargo x postgres-test --build && cargo x postgres-test --test  # plugin-data store against a real Postgres in docker
```

`cargo x` is an alias for `cargo xtask`. `cargo x --help` lists everything.

Requires `protoc` (protobufs cross the plugin wasm boundary).

**Never set `CARGO_TARGET_DIR` or change `build.target-dir`.** Checkout builds look for
plugins in `<repo>/target/wasm32-wasip1/{debug,release}`, resolved at compile time; moving
the target dir leaves them with no builtins. Symlink `target/` instead. See CONTRIBUTING.md.

Builtin plugin wasm is not checked in: it is built under `target/wasm32-wasip1/` and
installed by `cargo x install` to `<data dir>/builtins/<version>` (`--builtins-dir`,
`--no-builtins` to override). `zellij-config/assets/plugins/` holds the embedded TypeScript builtins.

**There is no CI in this fork.** `.github/workflows` was deleted deliberately (`c456d1cdf`), so
nothing checks a branch unless a person runs it. The gate is:

```sh
cargo x ci check     # format --check, lint, test, integration-test
```

Run it before handing off a branch or opening a PR. Every phase runs even if an earlier one
fails, so one failure does not hide the rest. The integration phase uses 16 nextest workers;
Docker-dependent suites remain separate. See "Reading a red run" below for timeout diagnosis.

Temporary plugin data and test cache ownership, direct-test overrides, and offline legacy cleanup
are documented in [docs/TEMPORARY_DIRECTORIES.md](docs/TEMPORARY_DIRECTORIES.md).

### Passing arguments

xtask's own flags come first (`--release`, …); everything else is forwarded — but
each command forwards to a different place:

| command | trailing args land in |
|---|---|
| `x test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x integration-test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x build`, `x install` | the native `cargo build`, repeated per crate (never the wasm plugin build); `x install` also takes `--profile <name>` (default `release`; `--profile dev` for a fast install), `--builtins-dir <dir>` / `--no-builtins` |
| `x run` | zellij itself (after `cargo run --`) |
| `x bench` | criterion (after `cargo bench --`) — criterion flags, not libtest ones |
| `x ci e2e` | the e2e test runner |

`x test` is one `cargo nextest run` over the whole workspace, so **nextest's own selection flags
work directly**:

- **One crate:** `cargo x test -p zellij-server`.
- **One test, or a pattern:** `cargo x test some_test_name`, or
  `cargo x test -E 'test(/^screen_tests::/)'` for a nextest filterset.
- **Output and concurrency:** `cargo x test --no-capture`, `cargo x test -j 1`. Note these are
  nextest's spellings — libtest's `--nocapture` and `--test-threads` do not apply.
- **Integration tests:** `cargo x integration-test -E 'test(resize)'`, or a bare name filter.
- **Remote-pane tests:** `cargo x remote-test --test -- --exact <name>`. They are
  `#[ignore]`d and need `--build` first (docker); see CONTRIBUTING.md.
- **Firecracker tests:** `cargo x firecracker-test --test`. They are `#[ignore]`d and need
  `--build` first; see `docs/FIRECRACKER.md`.
- **Postgres plugin-data tests:** `cargo x postgres-test --test -- -E 'test(/postgres::oracle/)'`.
  The store's test files run once per engine; the Postgres half is behind the `postgres-tests`
  feature and needs `--build` first (docker). See CONTRIBUTING.md.

`x test`'s separate doctest pass follows the narrowing: a `-p`-selected run runs only that
package's doctests, and a run narrowed by a test name or filterset skips them (`cargo test --doc`
cannot express either filter), which it says on stdout.

Bare cargo still works for an inner loop and is worth knowing, because nextest runs every test in
its own process and `cargo test` does not: `cargo test -p zellij-server -- some_filter`, from the
repo root, never with `CARGO_TARGET_DIR` set. Don't pass `--target`; the plugin crates' wasm pin
in `default-plugins/*/.cargo/config.toml` is only read when cargo is invoked from inside those
directories, and passing it splits the artifact tree. Tests that load a plugin at runtime need
`cargo x build -p` (plugins only) first - there is no checked-in wasm to fall back on.

### What tests exist

`docs/TEST_INVENTORY.md` is a generated checklist of every automated test - one item per test,
grouped by the gate that runs it, `#[ignore]`d ones tagged. Regenerate it with
`scripts/test-inventory.sh` (one `cargo nextest list --workspace --all-features --run-ignored
all`, plus a source scan that reports anything the listing missed). `--fast` skips the
feature-gated e2e and Postgres groups.

### Choosing what to run

Default to the tests that cover what you touched, using the narrowing recipes above — `-p` for a
crate, an `-E 'test(...)'` expression for a pattern. A full `cargo x test` still links every
crate's test binary, and rebuilds the wasm plugins whenever their sources changed, so it is not
the default finishing move.

Escalate to the full `cargo x test` (and `cargo x integration-test`) when the change reaches
past one crate:

- The shared libraries (`zellij-types`, `zellij-runtime`, `zellij-config`, `zellij-plugin-api`,
  `zellij-ipc`, `zellij-session`, `zellij-cli`, `zellij-storage`, `zellij-bus`, `zellij-routing`) — changes can reach
  every consumer; the bus in particular carries every thread's instruction enum
- `zellij-storage/src/plugin_data_store/` — also run `cargo x postgres-test`: the same test files
  run against SQLite and Postgres, and only the Postgres half needs a server
- `zellij-tile/`, `zellij-tile-utils/`, or assets under `zellij-config/`, `zellij-plugin-api/`,
  or `zellij-ipc/` (regenerate changed protobufs with `cargo x proto`)
- the render path in `zellij-panes/` and `zellij-terminal/` (grid, panes, output), which the
  `insta` snapshots and integration tests cover broadly; `-p zellij-panes` runs the tab and pane
  snapshots alone
- screen/tab/pty plumbing that both plugins and the client sit on
- `zellij-plugin-host/` — `-p zellij-plugin-host` covers the host's own tests; the screen tests
  in `zellij-server` and the integration tests cover what it sends
- dependency, toolchain, or `xtask` changes
- a batch of edits spanning several crates, or any run where snapshots were regenerated

Run it too when the user asks, or before handing off a branch or PR — that's what CI runs, so
anything skipped locally gets checked there.

The full test and integration-test suites bind local TCP and Unix sockets, so when you do run
them, use sandbox escalation on the first attempt.

### Reading a red run

Neither suite is reliable enough for a single red run to be evidence. Before believing a failure:

- **Run `integration-test` with `-j 16`.** At default parallelism on a many-core machine it
  produces ~100 timeouts purely from contention: nothing in the suite conflicts over a port or a
  directory — `test_env.rs` gives every test process its own `TMPDIR`, `ZELLIJ_SOCKET_DIR`, `HOME`
  and XDG dirs, and the app runs in-process behind fake OS APIs — but each test spawns a real
  server plus client threads (~30 threads, ~55 for a `nested_*` one), and the waits are
  wall-clock-bounded (`default_timeout()` is 10s; nextest kills at 30s × 2). One test per core
  means thousands of threads, and the contention comes back as timeouts.
  `-j 16` is the default because it leaves headroom on a 64-core machine while still running four
  times as wide as the `-j 4` this note used to recommend. Drop to `-j 4` on a smaller machine, or
  when a run at 16 produces timeouts you cannot reproduce by running the named tests alone. A
  handful (`no_focus`, `no_ui_fullscreen`) time out at 60s under load and pass alone at any width.
  Note also that this machine is shared: a sibling worktree running its own suite contends for the
  same cores, so check for another `cargo x integration-test` before believing a red run.
- **There are no known pre-existing integration failures.** The former quit, nested-session and
  resurrection failures shared a stale harness teardown: it typed the now-confirmed quit binding
  and silently ignored join timeouts. Teardown now uses lifecycle IPC and treats timeout as a test
  failure, so do not dismiss those tests when they are red.
- **The two known-flaky lib tests are fixed** and each now passes 20/20 in a loop:
  `screen_tests::cli_actions::send_cli_scroll_up_action` in `zellij-server` and
  `plugin_tests::keybinds::mode_update_payload_is_lightweight_for_opted_in_plugins` in
  `zellij-plugin-host`.
  Both were fixed in the test. A third, in the since-removed global panes suite, needed a
  **production** fix for real nondeterminism: two equally sized panes were ranked by `HashMap`
  order, so the same layout could grow a new pane in two different places on two runs.
- **A further flake, `pty_host_remote_reader_tests::a_resize_made_during_an_outage_is_sent_again_on_reconnect`,
  was found by the new gate** and fixed the same way. Its helper thread allowed 10s for the
  reconnect while the main flow gave up after ~5 and then read the slot, so a reconnect landing in
  between failed with "the reconnect never produced a socket". It passes alone either way — only
  a loaded machine reveals it, which is exactly what `cargo x ci check` provides.

### Three kernel differences the suite has already been bitten by

macOS is a supported development platform, and the gate ran red on it for a while because these
three read as flakes or as "environment" until someone checks the kernel. All are fixed; the note
is here so the next surprise is recognised rather than re-diagnosed.

- **`std` reaches `pipe(2)` on macOS, `pipe2(2)` on Linux.** `register_plugin!` used to export its
  host entry points with an unconditional `#[no_mangle]`, and in a plugin crate's *native* test
  binary the linker prefers a local `pipe` to libc's. Every `Command::spawn` in those tests
  therefore called the plugin, which read EOF from stdin and aborted the process — presenting as
  three `status-bar` tests dying with SIGABRT and a message about deserializing from stdin. The
  exports are `#[cfg_attr(target_family = "wasm", no_mangle)]` now. Watch for `SIGABRT` in a
  nextest summary: it counts in "N failed" but prints under its own heading.
- **XNU never wakes a blocking `waitpid` for a continue,** and raises no `SIGCHLD` for one either;
  only `WNOHANG` returns the status, and only until the process next `exec`s. Both child waiters
  (`pty_host::host::spawn_child_waiter`, `os_input_output_unix::wait_reporting_stops`) poll while
  the child is stopped for that reason. Left unfixed it made a `^Z`'d pane permanently input-dead.
- **A full `AF_UNIX` backlog is `ECONNREFUSED` on the BSDs, `EAGAIN` on Linux** — and on the BSDs
  that is the same errno as a socket whose server is gone, so `ipc_connect` retries it for the
  whole `IPC_CONNECT_TIMEOUT` before believing it (`consts::A_FULL_BACKLOG_IS_REFUSED`). Without
  that, a live server briefly behind on `accept` was read as absent and had its socket unlinked.

Two smaller ones worth knowing when a test compares paths or spawns a helper: macOS resolves
`/tmp` and `/var` through symlinks (`/private/...`), so paths only compare after `canonicalize`;
and `/bin/true` does not exist there — `/usr/bin/true` is on both.

### Waiting in tests: use `Watched`, not `sleep`

`zellij-test-support` provides `Watched<T>` — shared state plus a bounded
`wait_until(what, timeout, predicate)` that panics with a diagnostic instead of hanging. The test
loggers in `unit/screen_tests/` and `zellij-plugin-host/src/unit/plugin_tests/` already hold their instruction
logs in one, so a test can wait for the condition it actually cares about.

Three rules learned the hard way, all from tests that were flaky for years:

- **Never assert on `snapshots.last()` or on a render count.** Renders are asynchronous and
  coalesce, so *which* frame is last and *how many* arrive are properties of the scheduler, not of
  the behaviour under test. Wait for a frame whose **content** shows the state you mean —
  `wait_for_settled_render` in `unit/screen_tests/mod.rs` does exactly this.
  The rule is about coalescing a test does not control. It does not reach a batch the test itself
  hands over as a single instruction, where the render count is fixed by the code path rather than
  by timing — `zellij-plugin-host/src/unit/plugin_tests/batching.rs` counts renders on purpose, because "N events
  cost one render" *is* the behaviour, and no single frame's content can express it. Counting
  there is fine as long as the test can say why the count cannot vary, in a comment.
- **A sleep is not a synchronisation primitive.** Instructions sent to the screen or plugin thread
  are drained in order, so ordering never needs a sleep; only *observing the result* does, and
  that is what `wait_until` is for.
- **A stub script's first run is not free.** macOS evaluates a newly written executable the first
  time it runs, and under the suite's own spawn storm — nextest gives every test a process — that
  costs *seconds*: measured here, the first exec of a fresh `#!/bin/sh` stub took 2.4s while every
  later exec of the same path took 5ms. `Command::spawn` returns in microseconds either way, so a
  deadline armed at spawn is timing that check rather than the child. Write a stub with
  `zellij_test_support::write_warm_script`, which runs it once as a no-op first, and give a test
  whose subject is not the clock a budget it cannot reach (`firecracker::AMPLE_TIMEOUT`). The
  `firecracker` provider suite failed this way: at 200ms–1s budgets it lost 8–11 tests per run on
  macOS and passed serially.

~580 `sleep()` calls remain in other tests. They are a mechanical follow-up, deliberately not
converted in bulk: changing scheduling everywhere at once is a bigger perturbation than the
problem, and this suite's flakes shift when scheduling does.

Kitty graphics conformance: see `zellij-terminal/src/panes/kitty_graphics/CLAUDE.md`.

Benchmarks: see `.claude/skills/bench/SKILL.md` (`cargo x bench`).

## Layout

| Path | What |
|---|---|
| `zellij-server/` | screen, client routing, pty scheduling, the server thread |
| `zellij-panes/` | the `Pane` trait and its implementations, pane managers, frames, `Tab` and its layout applier |
| `zellij-bus/` | the server's thread bus: instruction enums and senders, `ServerOsApi`, warps, background jobs, session recorder |
| `zellij-routing/` | synchronous action dispatch and completion waits shared by server and plugin host |
| `zellij-plugin-host/` | plugin host: Wasm and Deno plugin loading, the plugin thread, host functions, pipes, job queues |
| `zellij-process/` | pane supervisors, local/remote protocols, SSH broker, Firecracker provider |
| `zellij-terminal/` | terminal emulation, graphics, output serialization, host queries, component parsing |
| `zellij-client/` | input handling and terminal I/O |
| `zellij-types/`, `zellij-runtime/`, `zellij-storage/` | shared models, host services, persistence |
| `zellij-config/`, `zellij-plugin-api/`, `zellij-ipc/` | config parsing, plugin wire API, client/server transport |
| `zellij-session/`, `zellij-cli/` | session lifecycle and serialization, command-line interpretation |
| `zellij-tile/`, `zellij-tile-utils/` | plugin-facing API |
| `default-plugins/*` | builtin plugins, each a wasm crate |
| `zellij-integration-tests/` | whole-app tests |
| `src/tests/e2e/` | dockerized black-box e2e (`docker compose up -d`, `cargo x ci e2e`) |
| `zellij-server/tests/` | remote panes against a real sshd (`cargo x remote-test`) |
| `docker/` | the containers those two suites run against |
| `docker-compose.postgres.yml` | the Postgres the plugin-data store suite runs against (`cargo x postgres-test`) |
| `xtask/` | the build system |

Read `docs/ARCHITECTURE.md` before changing server internals, and
`docs/TERMINOLOGY.md` for pty/ANSI/CSI vocabulary.
`docs/PLUGIN_RENDER_PIPELINE.md` is a standing write-up of why plugin input lags behind a busy
plugin — the four FIFO queues a keystroke crosses, and which of the candidate fixes have landed
and which have not. Read it before touching plugin event delivery or the plugin render path.

## Conventions

- **Errors**: return `Result`, don't `unwrap`. `use zellij_runtime::errors::prelude::*;`,
  `.context(...)` every `Result`, `anyhow!()` for ad-hoc. Log with `.non_fatal()`
  rather than `log::error!`. Full rules: `docs/ERROR_HANDLING.md`.
- **Tests** live next to the code in `unit/` dirs (e.g. `zellij-panes/src/tab/unit/`), not
  in top-level `tests/`. Many are `insta` snapshot tests — review `.snap` diffs, and
  commit the updated snapshots when behavior intentionally changes.
- **Logs** while debugging: `$TMPDIR/zellij-<UID>/zellij-log/zellij.log` (truncated at
  100KB). `--debug` also dumps raw pty bytes per pane.
- Toolchain is `stable` per `rust-toolchain.toml`; no MSRV policy.
- Changing protobufs under `zellij-plugin-api/assets/` or `zellij-ipc/assets/`: regenerate with
  `cargo x proto`.

## Self-programming

`docs/SELF_PROGRAMMING.md` summarises the workstream that made this fork drive and rewrite itself
through coding agents - the goal, what landed where, proto numbers in use, and the plugin-loading
lessons. Read it before touching agent records, `zellij events`, `config.d`, hooks or plugin
authoring.

## The plugin data store is shared between servers

One `plugin-data.db` serves every zellij server on the machine; with `plugin_data_url` pointing
at Postgres, one database serves every machine. Two rules follow, and both bite silently.

- **A row about a running server names it.** `host_id` *and* `zellij_session_name` — the pair
  `hocus_proto::agents::rows::Owner` carries. Never scope by session alone: two machines can each
  run a session called `main`, and a pane id means nothing outside the server that allocated it.
  The id comes from `zellij_runtime::host_identity`, resolved once at server startup and read by
  plugins off `PluginIds.host_id`. Carry it; do not call `get_plugin_ids()` from library code,
  which reads real stdin in a native test binary. An **empty** `host_id` is a row from before the
  column, backfilled by the migration, and what to do with one differs per entity: a run or an
  import marker is claimed **beside, never instead of** — `store::import_recorded` writes this
  machine's marker and leaves the legacy row alone, because a row naming no machine may be
  another machine's only record of an import — a latch or summary is deleted, a job is left
  alone. Readers that adopt the empty host say so with `store::host_or_legacy`
  (`rows::owned_or_unclaimed`, `run_history::Filter::pred` when a session is named too); ones
  that must not (`rows::owned_by`, `jobs`) spell the strict equality. See
  `docs/ARCHITECTURE.md`.
- **A write that replaces what it read guards on `rev`.** `expect_rev` on a `Put` or `Update`
  makes it a compare-and-set, refused with `DataError::Conflict` and rolling the batch back. Use
  it where a write replaces something composed from an earlier read; do not use it where a write
  sets a flag, since two writers agreeing is not a conflict. **The rev must be the one the
  value's author read**, not one re-fetched just before writing — that window is microseconds
  wide and catches nothing. It travels out in what the reader is shown and back in as an
  argument (`todo_edit` / `note_edit` take `expect_rev` — `note_edit`'s covers a tags-only edit
  too, which replaces the whole set); `store::guarded_rev` prefers it to the row's own. A fetch
  that names its columns drops `rev`, which silently unguards the write — see
  `notes::LIST_COLUMNS`.

Not handled: a watch is only re-evaluated by writes from its own server process, so on a shared
Postgres another machine's write does not wake it. See `docs/ARCHITECTURE.md` ("Sharing a store
between servers") and `docs/DENO_PLUGINS.md` ("Persistent data").

## Agent state from coding-agent hooks

A `claude` or a `codex` running in a pane can report its own busy/waiting/idle state instead of
being guessed at from its screen — `zellij hocus hooks install` wires both agents' hooks to
`zellij action agent-report`, which sets the pane's agent record (`PaneInfo.agent`) for every
reader of the manifest and broadcasts the event itself to the plugins. See `docs/AGENT_HOOKS.md`. Unrelated to the `hooks { … }` config block
(`zellij-types/src/input/hooks.rs`), which binds a plugin to a moment in the session.

## Script plugins (Deno)

Plugins support Wasm and Deno (JavaScript/TypeScript). All script builtins use embedded TypeScript.
Configuration uses KDL. Read `zellij-plugin-host/src/deno/CLAUDE.md` before changing the runtime or SDK.
`docs/DENO_PLUGINS.md` is the user-facing guide. `docs/DENO_PARITY.md` records historical verification,
and `docs/LUA_REMOVAL.md` explains manual migration from the removed Lua runtime and configuration.

## Hocus skills

A hocus skill is a `SKILL.md` prompt typed as `/<name> [text]`, read from
`<project root>/.hocus/skills/<name>/` and `~/.config/zellij/plugins/hocus/skills/<name>/` only
— never from `.claude/skills`, which are Claude Code's and name tools hocus does not have. See
`docs/SKILLS.md`; the code is `default-plugins/hocus/src/skills.rs`.
