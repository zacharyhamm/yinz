# AGENTS.md

Terminal workspace, Rust cargo workspace. Server/client split over IPC, plus builtin
plugins compiled to wasm.

## Build & test — use `cargo xtask`, not bare `cargo`

```sh
cargo x build            # builds plugins (wasm32-wasip1) then the binary
cargo x test             # the whole workspace in one run (needs cargo-nextest)
cargo x lint             # clippy at -D warnings; --fix applies the suggestions
cargo x format           # rustfmt; --check to verify instead of rewrite
cargo x ci check         # the pre-handoff gate: format --check, lint, test
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
`--no-builtins` to override). `zellij-utils/assets/plugins/` holds only the Lua builtins.

**There is no CI in this fork.** `.github/workflows` was deleted deliberately (`c456d1cdf`), so
nothing checks a branch unless a person runs it. The gate is:

```sh
cargo x ci check     # format --check, lint, test
```

Run it before handing off a branch or opening a PR. Every phase runs even if an earlier one
fails, so one failure does not hide the rest. `cargo x integration-test -- -j 16` is a separate
run — see "Reading a red run" below for why it is not part of the gate.

### Passing arguments

xtask's own flags come first (`--release`, …); everything else is forwarded — but
each command forwards to a different place:

| command | trailing args land in |
|---|---|
| `x test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x integration-test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x build`, `x install` | the native `cargo build`, repeated per crate (never the wasm plugin build); `x install` also takes `--builtins-dir <dir>` / `--no-builtins` |
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

### Choosing what to run

Default to the tests that cover what you touched, using the narrowing recipes above — `-p` for a
crate, an `-E 'test(...)'` expression for a pattern. A full `cargo x test` still links every
crate's test binary, and rebuilds the wasm plugins whenever their sources changed, so it is not
the default finishing move.

Escalate to the full `cargo x test` (and `cargo x integration-test`) when the change reaches
past one crate:

- `zellij-utils/` — IPC contracts, config/KDL parsing, errors, plugin API; nearly everything
  depends on it
- `zellij-utils/src/plugin_data_store/` — also run `cargo x postgres-test`: the same test files
  run against SQLite and Postgres, and only the Postgres half needs a server
- `zellij-tile/`, `zellij-tile-utils/`, or anything under `zellij-utils/assets/` (protobufs,
  embedded plugins — also regenerate with `cargo x proto`)
- the render path in `zellij-server/` (grid, panes, output), which the `insta` snapshots and
  integration tests cover broadly
- screen/tab/pty plumbing that both plugins and the client sit on
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
- **The two known-flaky `zellij-server` lib tests are fixed** and each now passes 20/20 in a
  loop: `screen_tests::cli_actions::send_cli_scroll_up_action` and
  `plugins::plugin_tests::keybinds::mode_update_payload_is_lightweight_for_opted_in_plugins`.
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
loggers in `unit/screen_tests/` and `plugins/unit/plugin_tests/` already hold their instruction
logs in one, so a test can wait for the condition it actually cares about.

Three rules learned the hard way, all from tests that were flaky for years:

- **Never assert on `snapshots.last()` or on a render count.** Renders are asynchronous and
  coalesce, so *which* frame is last and *how many* arrive are properties of the scheduler, not of
  the behaviour under test. Wait for a frame whose **content** shows the state you mean —
  `wait_for_settled_render` in `unit/screen_tests/mod.rs` does exactly this.
  The rule is about coalescing a test does not control. It does not reach a batch the test itself
  hands over as a single instruction, where the render count is fixed by the code path rather than
  by timing — `plugins/unit/plugin_tests/batching.rs` counts renders on purpose, because "N events
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

Kitty graphics conformance: see `zellij-server/src/panes/kitty_graphics/CLAUDE.md`.

Benchmarks: see `.claude/skills/bench/SKILL.md` (`cargo x bench`).

## Layout

| Path | What |
|---|---|
| `zellij-server/` | screen, tabs, panes, pty, plugin host — most logic lives here |
| `zellij-client/` | input handling and terminal I/O |
| `zellij-utils/` | shared: IPC contracts, config/KDL parsing, errors, plugin API |
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

- **Errors**: return `Result`, don't `unwrap`. `use zellij_utils::errors::prelude::*;`,
  `.context(...)` every `Result`, `anyhow!()` for ad-hoc. Log with `.non_fatal()`
  rather than `log::error!`. Full rules: `docs/ERROR_HANDLING.md`.
- **Tests** live next to the code in `unit/` dirs (e.g. `zellij-server/src/unit/`), not
  in top-level `tests/`. Many are `insta` snapshot tests — review `.snap` diffs, and
  commit the updated snapshots when behavior intentionally changes.
- **Logs** while debugging: `$TMPDIR/zellij-<UID>/zellij-log/zellij.log` (truncated at
  100KB). `--debug` also dumps raw pty bytes per pane.
- Toolchain is `stable` per `rust-toolchain.toml`; no MSRV policy.
- Changing protobufs under `zellij-utils/assets/`: regenerate with `cargo x proto`.

## Self-programming

`docs/SELF_PROGRAMMING.md` summarises the workstream that made this fork drive and rewrite itself
through coding agents - the goal, what landed where, proto numbers in use, and the plugin-loading
lessons. Read it before touching agent records, `zellij events`, `config.d`, hooks or plugin
authoring.

## Agent state from coding-agent hooks

A `claude` or a `codex` running in a pane can report its own busy/waiting/idle state instead of
being guessed at from its screen — `zellij hocus hooks install` wires both agents' hooks to
`zellij action agent-report`, which sets the pane's agent record (`PaneInfo.agent`) for every
reader of the manifest and broadcasts the event itself to the plugins. See `docs/AGENT_HOOKS.md`. Unrelated to the `hooks { … }` config block
(`zellij-utils/src/input/hooks.rs`), which binds a plugin to a moment in the session.

## Lua plugins

Plugins come in two kinds: wasm modules (every builtin) and Lua scripts. See
`zellij-server/src/plugins/lua/CLAUDE.md` before changing the Lua runtime; `docs/LUA_PLUGINS.md`
is the user-facing guide.

## Hocus skills

A hocus skill is a `SKILL.md` prompt typed as `/<name> [text]`, read from
`<project root>/.hocus/skills/<name>/` and `~/.config/zellij/plugins/hocus/skills/<name>/` only
— never from `.claude/skills`, which are Claude Code's and name tools hocus does not have. See
`docs/SKILLS.md`; the code is `default-plugins/hocus/src/skills.rs`.
