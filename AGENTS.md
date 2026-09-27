# Zellij

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
integration-test workers. Docker and VM suites run separately:

| Command | Requirement |
|---|---|
| `cargo x remote-test` | Docker with a real SSH server |
| `cargo x firecracker-test` | Docker for the build, real VM through `/dev/kvm` |
| `cargo x postgres-test` | Docker with Postgres |

For each of these three suites, run `--build` before `--test`.

Remote SSH, Firecracker, and the older E2E suite build GNU release binaries in
Debian 13 slim through Docker. The builder matches the host architecture
(x86_64 or ARM64); Firecracker requires x86_64 Linux. Cargo caches stay under
`target/linux-build/`. Remote SSH also accepts `--build --native` on Linux for
a debug build if the host binary runs with Debian 13's libraries.

For the older E2E suite, run `cargo x ci e2e --build`, then
`docker compose up -d --build`, then `cargo x ci e2e --test tests::e2e::`.
The filter selects the five terminal tests and leaves Postgres to its own suite.
The SSH fixture listens on localhost port 2222 with user/password `test`/`test`.

**Never set `CARGO_TARGET_DIR` or change `build.target-dir`.** Checkout plugin
paths resolve at compile time to `<repo>/target/wasm32-wasip1/{debug,release}`.
Symlink `target/` if needed. Builtin Wasm is not checked in.

`cargo x install <destination>` defaults to `release` and installs Wasm to
`<data dir>/builtins/<version>`. Use `--builtins-dir <dir>` or `--no-builtins`
to override. `--profile dev` leaves the host unoptimized. Wasm in the debug
artifact directory defaults to opt-level 2. `CARGO_PROFILE_DEV_OPT_LEVEL` and
`CARGO_PROFILE_DEV_DEBUG` override its profile settings. Embedded TypeScript
builtins live in `zellij-deno-builtins/assets/plugins/`.

### Arguments and test selection

Put xtask flags first. Remaining arguments go to:

| Command | Forwards to |
|---|---|
| `x test`, `x integration-test` | `cargo nextest run` |
| `x build`, `x install` | Native `cargo build`; excludes the Wasm build |
| `x run` | Zellij |
| `x bench` | Criterion |
| `x ci e2e` | E2E runner |

Default to tests for the changed crate or behavior:

```sh
cargo x test -p zellij-server
cargo x test some_test_name
cargo x test -p zellij-screen -E 'test(/^screen::screen_tests::tabs::/)'
cargo x integration-test -E 'test(resize)' -j 16
```

Use nextest's `--no-capture` and `-j N`, not libtest's `--nocapture` or
`--test-threads`. Package selection also narrows doctests. A name or filterset
skips doctests because `cargo test --doc` cannot express that selection.
To list tests, including ignored ones, use
`cargo nextest list --workspace --all-features --run-ignored all`.
Doctests are not listable.

Remote tests accept `--test -- --exact <name>`. Postgres tests accept
`--test -- -E 'test(/postgres::oracle/)'` and enable the `postgres-tests` feature.

For a bare `cargo test` inner loop, run from the repo root without `--target`.
Build plugins first with `cargo x build --plugins-only` if tests load Wasm.
Nextest runs each test in a separate process; bare `cargo test` does not.

Run full `cargo x test` and `cargo x integration-test -j 16` for changes to:

- Shared libraries, plugin-facing APIs, or assets under `zellij-config/`,
  `zellij-project-config/`, `zellij-deno-builtins/`, `zellij-plugin-api/`,
  `zellij-plugin-location/`, `zellij-ipc/`, `zellij-host-protocol/`, or
  `default-plugins/hocus-data/` (linked by the hocus plugins and the plugin host).
- Render paths in `zellij-panes/`, `zellij-terminal/` or `zellij-terminal-graphics/`,
  key encoding in `zellij-key-codec/`, screen/tab/pty plumbing (`zellij-screen/`,
  `zellij-tab/`, `zellij-pty/`), `zellij-plugin-host/`, or `zellij-plugin-authoring/`.
- Dependencies, toolchain, `xtask`, multiple crates, or regenerated snapshots.

In-process whole-app tests use a fake OS backend. The real `zellij-server-os/`
backend runs end to end only in tests that spawn the `zellij` binary, mainly the
root package's tests and the integration suite's plugin-test sessions. For
`zellij-server-os/` changes, run `cargo x test -p zellij-server-os -p zellij`
and `cargo x integration-test -j 16`, plus `cargo x remote-test` or
`cargo x firecracker-test` when remote or VM spawning changes.

For `zellij-storage/src/plugin_data_store/`, also run the Postgres suite.
Full test and integration suites bind TCP and Unix sockets: use sandbox
escalation on the first attempt.

### Failures and test timing

- Use `-j 16` for integration tests. Reduce to `-j 4` on smaller or busy machines.
  Check for competing runs in other worktrees and rerun failures alone before
  attributing timeouts to regressions. There are no known pre-existing
  integration failures.
- Use `zellij-test-support`'s `Watched<T>::wait_until(what, timeout, predicate)`
  instead of sleeps. Screen and plugin instruction logs already use it.
- Wait for render **content**, not `snapshots.last()` or a render count. See
  `wait_for_settled_render` in `zellij-screen/src/unit/screen_tests/mod.rs`.
  Count renders only when the test controls a batch with a fixed count, and
  explain why scheduling cannot change it in a comment.
- Use `zellij_test_support::write_warm_script` for executable stubs. The first
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
| `zellij-server/` | Session bootstrap, client routing, server thread |
| `zellij-screen/` | Screen thread: tabs, focus, overlays, rendering to clients |
| `zellij-pty/` | Pty thread: spawning and supervising pane processes |
| `zellij-tab/` | `Tab`: pane composition, focus, pane tabs, swap layouts, mouse, pane groups, clipboard |
| `zellij-panes/` | `Pane` trait, panes, pane managers, frames, layout application |
| `zellij-bus/` | Thread instructions, senders, OS API contract, background jobs, recorder |
| `zellij-server-os/` | OS API backend: ptys, supervisors, remote panes, client writers |
| `zellij-routing/` | Action dispatch and completion waits |
| `zellij-plugin-host/` | Wasm/Deno runtime, host functions, events, pipes, jobs |
| `zellij-plugin-authoring/` | V8-free plugin checks and writes; compilation-worker protocol and client |
| `zellij-process/` | Pane supervisors, local/remote protocols, SSH, Firecracker client |
| `zellij-firecracker-provider/` | The `zellij-firecracker` VM provider (Linux only) |
| `zellij-swamp/` | Managed Swamp servers and the workflow client |
| `zellij-codex/` | Codex conversation-to-pane binding for `zellij hocus codex` |
| `zellij-terminal/` | Terminal emulation (`Grid`) and output serialization; drives the graphics engine and emits host queries |
| `zellij-terminal-graphics/` | Kitty graphics and Sixel protocol engine; re-exported as `zellij_terminal::panes::{kitty_graphics, sixel}` |
| `zellij-client/` | Input handling and terminal I/O |
| `zellij-types/`, `zellij-runtime/`, `zellij-storage/` | Shared models, host services, persistence |
| `zellij-plugin-location/` | Plugin URL parsing, also built for Wasm; re-exported as `zellij_runtime::plugin_location` |
| `zellij-key-codec/` | Kitty and legacy key encoding and decoding, vendored termwiz input parser |
| `zellij-host-protocol/` | Host-query and desktop-notification (OSC 9/99/777) types shared by terminal, bus and screen |
| `zellij-config/`, `zellij-plugin-api/`, `zellij-ipc/` | Config, plugin wire API, transport |
| `zellij-project-config/` | Project workflow settings in `.zellij/swamp-workflows.kdl` |
| `zellij-deno-builtins/` | Embedded TypeScript builtin plugin sources |
| `zellij-session/`, `zellij-cli/` | Session lifecycle, serialization, CLI |
| `zellij-tile/`, `zellij-tile-utils/` | Plugin-facing API |
| `zellij-tile-markdown/` | CommonMark to styled terminal lines for plugins |
| `zellij-test-support/` | Condition waits and warm scripts for tests |
| `default-plugins/` | Builtin Wasm crates |
| `default-plugins/hocus-data/` | Hocus store seam, entity schemas, project/todo/note registry; re-exported by `hocus-proto`, linked by the plugin host |
| `zellij-integration-tests/` | Whole-app tests |
| `src/tests/e2e/`, `zellij-server/tests/` | Docker E2E and remote-pane tests |
| `docker/`, `docker-compose.postgres.yml` | Test containers |
| `xtask/` | Build and test commands |

## Conventions

- Return `Result` instead of unwrapping. Import
  `zellij_runtime::errors::prelude::*`, add `.context(...)` to every `Result`,
  use `anyhow!()` for ad-hoc errors, and log with `.non_fatal()`.
- Fix lint warnings or use `#[expect(lint, reason = "…")]` on the affected item.
  `cargo x lint` rejects file-level `allow`/`expect`, including through
  `cfg_attr`, unless `FILE_LEVEL_ALLOWS` in `xtask/src/lint.rs` records a reason.
  Put workspace-wide lint exceptions and their reasons in
  `[workspace.lints.clippy]` in `Cargo.toml`.
- Put tests in adjacent `unit/` directories, not top-level `tests/`.
  Review `.snap` diffs and commit snapshot updates for intentional changes.
- After changing protobufs in `zellij-plugin-api/assets/` or
  `zellij-ipc/assets/`, regenerate with `cargo x proto`.
- Debug logs: `$TMPDIR/zellij-<UID>/zellij-log/zellij.log`, truncated at 100KB.
  `--debug` also dumps raw pty bytes per pane.

## Shared plugin data

One `plugin-data.db` serves all local servers. With `plugin_data_url` set to
Postgres, one database can serve multiple machines.

- Scope server rows by **both `host_id` and `zellij_session_name`**, using
  `hocus_proto::agents::rows::Owner`. Carry `PluginIds.host_id` from startup.
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
- Watches react only to writes from their own server process. Another machine's
  Postgres write does not wake them.

## Swamp integration

Zellij manages local Swamp servers and runs project-owned Swamp workflows
through `zellij workflow`. The supervisor is in `zellij-swamp/src/lib.rs`.

Use the `swamp` skill for Swamp authoring, operations, and diagnostics:

- Confirm CLI arguments with `swamp help <command>`. Search existing extensions
  and model types before building integrations. Reuse or extend them first.
- Author workflows as Swamp YAML DAGs and validate before running them.
  Use report extensions for reusable analyses of model output.
- Reuse model data with CEL `data.latest(...)` expressions. Prefer one method
  that handles multiple targets over concurrent calls that contend on a model lock.
- Use Swamp commands to inspect its data and resources. Check resource IDs with
  `swamp model get <name> --json` before delete/stop/destroy operations.
  Inspect generated reports after failures before retrying.
- Pin versions in extension `npm:` imports. These dependencies are bundled,
  not tracked by `deno.lock` or `package.json`.

## Plugins, hooks, and skills

- Before changing the Deno runtime or SDK, read
  [its local instructions](zellij-plugin-host/src/deno/CLAUDE.md).
- For Kitty graphics conformance, read
  [the graphics instructions](zellij-terminal-graphics/src/kitty_graphics/CLAUDE.md).
- For render benchmarks, use [the bench skill](.agents/skills/bench/SKILL.md).
- `zellij hocus hooks install` connects Claude/Codex state hooks to
  `zellij action agent-report`, updating `PaneInfo.agent` and notifying plugins.
  This differs from the session `hooks { … }` config in `zellij-types/src/input/hooks.rs`.
- Hocus reads `SKILL.md` prompts for `/<name> [text]` only from
  `<project root>/.hocus/skills/<name>/` and
  `~/.config/zellij/plugins/hocus/skills/<name>/`, never `.claude/skills`.
  Implementation: `default-plugins/hocus/src/skills.rs`.
