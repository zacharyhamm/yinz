# Yinz

Rust Cargo workspace with a client/server split over IPC, Wasm plugins, and
embedded TypeScript plugins running on Deno. Configuration uses KDL.

## Build and test

Use `cargo x` (`cargo xtask`). See `cargo x --help` for all commands.
The toolchain is `stable`, with no MSRV policy. Install `protoc` and `cargo-nextest`.

```sh
cargo x build                 # Wasm plugins, then native binary
cargo x test                  # workspace unit tests and doctests
cargo x integration-test -j 16 # whole-app tests
cargo x lint                  # deny warnings; --fix applies suggestions
cargo x format                # --check verifies without rewriting
cargo x ci check              # format --check, lint, test, integration-test
cargo x run -- -l strider
cargo x bench                 # render throughput; separate from the gate
```

**There is no CI in this fork.** Run `cargo x ci check` before handing off a
branch or opening a PR. It runs every phase even after failures and uses 16
integration-test workers. Docker suites run separately:

| Command | Requirement |
|---|---|
| `cargo x remote-test` | Docker with a real SSH server |

For the remote suite, run `--build` before `--test`.

Remote SSH and the older E2E suite build GNU release binaries in
Debian 13 slim through Docker. The builder matches the host architecture
(x86_64 or ARM64). Cargo caches stay under
`target/linux-build/`. Remote SSH also accepts `--build --native` on Linux for
a debug build if the host binary runs with Debian 13's libraries.

For the older E2E suite, run `cargo x ci e2e --build`, then
`docker compose up -d --build`, then `cargo x ci e2e --test tests::e2e::`.
The filter selects the five terminal tests.
The SSH fixture listens on localhost port 2222 with user/password `test`/`test`.

**Never set `CARGO_TARGET_DIR` or change `build.target-dir`.** Checkout plugin
paths resolve at compile time to `<repo>/target/wasm32-wasip1/{debug,release}`.
**In a worktree, NEVER use a shared `target/`. Isolate your builds and tests.**
Keep a separate, real `target/` directory in each worktree; do not symlink it to
the main checkout or another worktree, or hardlink mutable build artifacts.
Do not copy workspace binaries, Wasm plugins, or their build metadata between
worktrees, either.
Concurrent builds can replace the native binary and Wasm plugins while tests
are running, causing tests to execute another checkout's code. Build the
worktree's own binaries and plugins before testing. Builtin Wasm is not checked in.

`cargo x install <destination>` defaults to `release` and installs Wasm to
`<data dir>/builtins/<version>`. Use `--builtins-dir <dir>` or `--no-builtins`
to override. `--profile dev` leaves the host unoptimized. Wasm in the debug
artifact directory defaults to opt-level 2. `CARGO_PROFILE_DEV_OPT_LEVEL` and
`CARGO_PROFILE_DEV_DEBUG` override its profile settings. Embedded TypeScript
builtins live in `yinz-deno-builtins/assets/plugins/`.

### Arguments and test selection

Put xtask flags first. Remaining arguments go to:

| Command | Forwards to |
|---|---|
| `x test`, `x integration-test` | `cargo nextest run` |
| `x build`, `x install` | Native `cargo build`; excludes the Wasm build |
| `x run` | Yinz |
| `x bench` | Criterion |
| `x ci e2e` | E2E runner |

Default to tests for the changed crate or behavior:

```sh
cargo x test -p yinz-server
cargo x test some_test_name
cargo x test -p yinz-screen -E 'test(/^screen::screen_tests::tabs::/)'
cargo x integration-test -E 'test(resize)' -j 16
```

Use nextest's `--no-capture` and `-j N`, not libtest's `--nocapture` or
`--test-threads`. Package selection also narrows doctests. A name or filterset
skips doctests because `cargo test --doc` cannot express that selection.
To list tests, including ignored ones, use
`cargo nextest list --workspace --all-features --run-ignored all`.
Doctests are not listable.

Remote tests accept `--test -- --exact <name>`.

For a bare `cargo test` inner loop, run from the repo root without `--target`.
Build plugins first with `cargo x build --plugins-only` if tests load Wasm.
Nextest runs each test in a separate process; bare `cargo test` does not.

Run full `cargo x test` and `cargo x integration-test -j 16` for changes to:

- Shared libraries, plugin-facing APIs, or assets under `yinz-config/`,
  `yinz-project-config/`, `yinz-deno-builtins/`, `yinz-plugin-api/`,
  `yinz-plugin-location/`, `yinz-ipc/`, `yinz-host-protocol/`, or
  `default-plugins/project-data/` (linked by the agent plugins and the plugin host).
- Render paths in `yinz-panes/`, `yinz-terminal/` or `yinz-terminal-graphics/`,
  key encoding in `yinz-key-codec/`, screen/tab/pty plumbing (`yinz-screen/`,
  `yinz-tab/`, `yinz-pty/`), `yinz-plugin-host/`, or `yinz-plugin-authoring/`.
- Dependencies, toolchain, `xtask`, multiple crates, or regenerated snapshots.

In-process whole-app tests use a fake OS backend. The real `yinz-server-os/`
backend runs end to end only in tests that spawn the `yinz` binary, mainly the
root package's tests and the integration suite's plugin-test sessions. For
`yinz-server-os/` changes, run `cargo x test -p yinz-server-os -p yinz`
and `cargo x integration-test -j 16`, plus `cargo x remote-test` when remote spawning changes.

Full test and integration suites bind TCP and Unix sockets: use sandbox
escalation on the first attempt.

### Failures and test timing

- Use `-j 16` for integration tests. Reduce to `-j 4` on smaller or busy machines.
  Check for competing runs in other worktrees before starting.
- Fix issues discovered during work in the current branch and worktree, as part
  of the same stream of work. Do not move them to separate branches or worktrees
  unless the user requests it.
- Treat every test flake or timeout as a defect. Record the failure, investigate
  its cause, and fix it in the current change. Diagnostic reruns help identify
  the cause. A passing rerun does not resolve the failure.
- Use `yinz-test-support`'s `Watched<T>::wait_until(what, timeout, predicate)`
  instead of sleeps. Screen and plugin instruction logs already use it.
- Wait for render **content**, not `snapshots.last()` or a render count. See
  `wait_for_settled_render` in `yinz-screen/src/unit/screen_tests/mod.rs`.
  Count renders only when the test controls a batch with a fixed count, and
  explain why scheduling cannot change it in a comment.
- Use `yinz_test_support::write_warm_script` for executable stubs. The first
  execution can take seconds on macOS. Give tests ample time unless timing is
  the behavior under test. Do not convert existing sleeps in bulk.

### Platform constraints

- Export `register_plugin!` entry points with `no_mangle` only on Wasm.
  A native `pipe` export shadows libc on macOS and breaks process spawning.
- On macOS, child waiters must poll `waitpid` with `WNOHANG` for continued
  children; blocking waits and `SIGCHLD` do not report them.
- A full Unix socket backlog returns `ECONNREFUSED` on BSD, `EAGAIN` on Linux.
  Retry for `IPC_CONNECT_TIMEOUT` before treating a BSD socket as stale.
- Canonicalize paths before comparing them: macOS aliases `/tmp` and `/var`.
  Use `/usr/bin/true`, which exists on both macOS and Linux.

## Layout

| Path | Purpose |
|---|---|
| `yinz-server/` | Session bootstrap, client routing, server thread |
| `yinz-screen/` | Screen thread: tabs, focus, overlays, rendering to clients |
| `yinz-pty/` | Pty thread: spawning and supervising pane processes |
| `yinz-tab/` | `Tab`: pane composition, focus, pane tabs, swap layouts, mouse, pane groups, clipboard |
| `yinz-panes/` | `Pane` trait, panes, pane managers, frames, layout application |
| `yinz-bus/` | Thread instructions, senders, OS API contract, background jobs |
| `yinz-server-os/` | OS API backend: ptys, supervisors, remote panes, client writers |
| `yinz-routing/` | Action dispatch and completion waits |
| `yinz-plugin-host/` | Wasm/Deno runtime, host functions, events, pipes, jobs |
| `yinz-plugin-authoring/` | V8-free plugin checks and writes; compilation-worker protocol and client |
| `yinz-process/` | Pane supervisors, local/remote protocols, SSH |
| `yinz-codex/` | Codex conversation-to-pane binding for `yinz agent codex` |
| `yinz-terminal/` | Terminal emulation (`Grid`) and output serialization; drives the graphics engine and emits host queries |
| `yinz-terminal-graphics/` | Kitty graphics and Sixel protocol engine; re-exported as `yinz_terminal::panes::{kitty_graphics, sixel}` |
| `yinz-client/` | Input handling and terminal I/O |
| `yinz-types/`, `yinz-runtime/`, `yinz-storage/` | Shared models, host services, persistence |
| `yinz-plugin-location/` | Plugin URL parsing, also built for Wasm; re-exported as `yinz_runtime::plugin_location` |
| `yinz-key-codec/` | Kitty and legacy key encoding and decoding, vendored termwiz input parser |
| `yinz-host-protocol/` | Host-query and desktop-notification (OSC 9/99/777) types shared by terminal, bus and screen |
| `yinz-config/`, `yinz-plugin-api/`, `yinz-ipc/` | Config, plugin wire API, transport |
| `yinz-project-config/` | Agent-exit settings in `.yinz/agent-exit.kdl` |
| `yinz-deno-builtins/` | Embedded TypeScript builtin plugin sources |
| `yinz-session/`, `yinz-cli/` | Session lifecycle, serialization, CLI |
| `yinz-tile/`, `yinz-tile-utils/` | Plugin-facing API |
| `yinz-tile-markdown/` | CommonMark to styled terminal lines for plugins |
| `yinz-test-support/` | Condition waits and warm scripts for tests |
| `default-plugins/` | Builtin Wasm crates |
| `default-plugins/project-data/` | Agent store seam, entity schemas, project/todo/note registry; re-exported by `agent-proto`, linked by the plugin host |
| `yinz-integration-tests/` | Whole-app tests |
| `src/tests/e2e/`, `yinz-server/tests/` | Docker E2E and remote-pane tests |
| `docker/` | Test containers |
| `xtask/` | Build and test commands |

## Conventions

- Return `Result` instead of unwrapping. Import
  `yinz_runtime::errors::prelude::*`, add `.context(...)` to every `Result`,
  use `anyhow!()` for ad-hoc errors, and log with `.non_fatal()`.
- Fix lint warnings or use `#[expect(lint, reason = "…")]` on the affected item.
  `cargo x lint` rejects file-level `allow`/`expect`, including through
  `cfg_attr`, unless `FILE_LEVEL_ALLOWS` in `xtask/src/lint.rs` records a reason.
  Put workspace-wide lint exceptions and their reasons in
  `[workspace.lints.clippy]` in `Cargo.toml`.
- Put tests in adjacent `unit/` directories, not top-level `tests/`.
  Review `.snap` diffs and commit snapshot updates for intentional changes.
- After changing protobufs in `yinz-plugin-api/assets/` or
  `yinz-ipc/assets/`, regenerate with `cargo x proto`.
- Debug logs: `$TMPDIR/yinz-<UID>/yinz-log/yinz.log`, truncated at 100KB.
  `--debug` also dumps raw pty bytes per pane.

## Shared plugin data

One SQLite `plugin-data.db` serves all local servers using the same data directory.

- Scope server rows by **both `host_id` and `yinz_session_name`**, using
  `agent_proto::agents::rows::Owner`. Carry `PluginIds.host_id` from startup.
  Do not call `get_plugin_ids()` in library code: it reads real stdin in native tests.
- An empty `host_id` marks legacy data. Preserve legacy runs and import markers
  when adding host-specific claims (`store::import_recorded`). Delete legacy
  latches and summaries; leave legacy jobs alone. Use `store::host_or_legacy`
  only for readers that accept legacy rows (`rows::owned_or_unclaimed`, or
  `run_history::Filter::pred` with a session). `rows::owned_by` and jobs require
  strict host equality.
- Guard read-modify-write `Put`/`Update` operations with `expect_rev` set to the
  **revision the value's author read**. Do not re-fetch it just before writing.
  Conflicts return `DataError::Conflict` and roll back the batch. Include `rev`
  in selected columns and carry it through `todo_edit` and `note_edit`, including
  tags-only edits. Reuse `store::guarded_rev`. Setting a flag alone does not need a guard.
- Watches react only to writes from their own server process. Another server's
  database write does not wake them.

## Plugins, hooks, and skills

- Before changing the Deno runtime or SDK, read
  [its local instructions](yinz-plugin-host/src/deno/CLAUDE.md).
- For Kitty graphics conformance, read
  [the graphics instructions](yinz-terminal-graphics/src/kitty_graphics/CLAUDE.md).
- For render benchmarks, use [the bench skill](.agents/skills/bench/SKILL.md).
- `yinz hooks install` connects Claude/Codex state hooks to
  `yinz action agent-report`, updating `PaneInfo.agent` and notifying plugins.
  This differs from the session `hooks { … }` config in `yinz-types/src/input/hooks.rs`.
- `yinz skills list/show/install` manages portable skills for external coding agents.
  Implementation: `src/portable_skills.rs`.
