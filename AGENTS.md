# AGENTS.md

Terminal workspace, Rust cargo workspace. Server/client split over IPC, plus builtin
plugins compiled to wasm.

## Build & test — use `cargo xtask`, not bare `cargo`

```sh
cargo x build            # builds plugins (wasm32-wasip1) then the binary
cargo x test             # the whole workspace in one run (needs cargo-nextest)
cargo x lint             # clippy at -D warnings; --fix applies the suggestions
cargo x format           # rustfmt; --check to verify instead of rewrite
cargo x ci check         # the pre-handoff gate: format --check, lint, assets --check, test
cargo x run -- -l strider
cargo x integration-test # whole-app tests (needs cargo-nextest)
cargo x bench            # render throughput benchmarks (criterion); not part of the gate
cargo x remote-test --build && cargo x remote-test --test  # remote panes, real ssh in docker
```

`cargo x` is an alias for `cargo xtask`. `cargo x --help` lists everything.

Requires `protoc` (protobufs cross the plugin wasm boundary).

**Never set `CARGO_TARGET_DIR` or change `build.target-dir`.** Debug builds embed
plugins from `<repo>/target/wasm32-wasip1/debug`, resolved at compile time; moving the
target dir breaks debug builds. Symlink `target/` instead. See CONTRIBUTING.md.

Every build rewrites the checked-in `.wasm` files under `zellij-utils/assets/plugins/`,
so `git status` shows a pile of modified binaries after any `cargo x build` or test run.
This is normal and expected — wasm builds are not byte-reproducible. Restore them with
`git checkout -- zellij-utils/assets/plugins` before committing unless the plugin sources
actually changed, and don't report the churn.

**There is no CI in this fork.** `.github/workflows` was deleted deliberately (`c456d1cdf`), so
nothing checks a branch unless a person runs it. The gate is:

```sh
cargo x ci check     # format --check, lint, assets --check, test
```

Run it before handing off a branch or opening a PR. Every phase runs even if an earlier one
fails, so one failure does not hide the rest. `cargo x integration-test -- -j 4` is a separate
run — see "Reading a red run" below for why it is not part of the gate.

### Passing arguments

xtask's own flags come first (`--no-web`, `--release`, …); everything else is forwarded — but
each command forwards to a different place:

| command | trailing args land in |
|---|---|
| `x test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x integration-test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x build`, `x install` | the native `cargo build`, repeated per crate (never the wasm plugin build) |
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

`x test`'s separate doctest pass follows the narrowing: a `-p`-selected run runs only that
package's doctests, and a run narrowed by a test name or filterset skips them (`cargo test --doc`
cannot express either filter), which it says on stdout.

Bare cargo still works for an inner loop and is worth knowing, because nextest runs every test in
its own process and `cargo test` does not: `cargo test -p zellij-server -- some_filter`, from the
repo root, never with `CARGO_TARGET_DIR` set. Don't pass `--target`; the plugin crates' wasm pin
in `default-plugins/*/.cargo/config.toml` is only read when cargo is invoked from inside those
directories, and passing it splits the artifact tree. Tests that load a plugin at runtime need
`cargo x build -p` (plugins only) first.

### Choosing what to run

Default to the tests that cover what you touched, using the narrowing recipes above — `-p` for a
crate, an `-E 'test(...)'` expression for a pattern. A full `cargo x test` still links every
crate's test binary, and rebuilds the wasm plugins whenever their sources changed, so it is not
the default finishing move.

Escalate to the full `cargo x test` (and `cargo x integration-test`) when the change reaches
past one crate:

- `zellij-utils/` — IPC contracts, config/KDL parsing, errors, plugin API; nearly everything
  depends on it
- `zellij-tile/`, `zellij-tile-utils/`, or anything under `zellij-utils/assets/` (protobufs,
  embedded plugins — also regenerate with `cargo x proto` / `cargo x assets`)
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

- **Run `integration-test` with `-j 4`.** At default parallelism on a many-core machine it
  produces ~100 timeouts purely from contention. Even at `-j 4` a handful (`no_focus`,
  `no_ui_fullscreen`) time out at 60s and pass alone.
- **Known pre-existing failures**, confirmed against a stashed tree: `misc::quit_with_keybinding`
  (it waits for "Bye from Zellij!" but this fork puts a quit-confirmation modal in the way), the
  `nested_guest_modal` / `nested_hardening` / `nested_sessions` / `nested_session_handling_config`
  group, and `stacked_pane_list::a_stack_list_survives_session_resurrection`.
- **The two known-flaky `zellij-server` lib tests are fixed** and each now passes 20/20 in a
  loop: `screen_tests::cli_actions::send_cli_scroll_up_action` and
  `plugins::plugin_tests::keybinds::mode_update_payload_is_lightweight_for_opted_in_plugins`.
  Both were fixed in the test. A third, in the since-removed global panes suite, needed a
  **production** fix for real nondeterminism — see `docs/TECH_DEBT.md`.
- **A further flake, `pty_host_remote_reader_tests::a_resize_made_during_an_outage_is_sent_again_on_reconnect`,
  was found by the new gate** and fixed the same way. Its helper thread allowed 10s for the
  reconnect while the main flow gave up after ~5 and then read the slot, so a reconnect landing in
  between failed with "the reconnect never produced a socket". It passes alone either way — only
  a loaded machine reveals it, which is exactly what `cargo x ci check` provides.

### Waiting in tests: use `Watched`, not `sleep`

`zellij-test-support` provides `Watched<T>` — shared state plus a bounded
`wait_until(what, timeout, predicate)` that panics with a diagnostic instead of hanging. The test
loggers in `unit/screen_tests/` and `plugins/unit/plugin_tests/` already hold their instruction
logs in one, so a test can wait for the condition it actually cares about.

Two rules learned the hard way, both from tests that were flaky for years:

- **Never assert on `snapshots.last()` or on a render count.** Renders are asynchronous and
  coalesce, so *which* frame is last and *how many* arrive are properties of the scheduler, not of
  the behaviour under test. Wait for a frame whose **content** shows the state you mean —
  `wait_for_settled_render` in `unit/screen_tests/mod.rs` does exactly this.
- **A sleep is not a synchronisation primitive.** Instructions sent to the screen or plugin thread
  are drained in order, so ordering never needs a sleep; only *observing the result* does, and
  that is what `wait_until` is for.

~580 `sleep()` calls remain in other tests. They are a mechanical follow-up, deliberately not
converted in bulk: changing scheduling everywhere at once is a bigger perturbation than the
problem, and this suite's flakes shift when scheduling does.

Kitty graphics conformance: see `zellij-server/src/panes/kitty_graphics/CLAUDE.md`.

Benchmarks: see `.claude/skills/bench/SKILL.md` (`cargo x bench`).

## Layout

| Path | What |
|---|---|
| `zellij-server/` | screen, tabs, panes, pty, plugin host — most logic lives here |
| `zellij-client/` | input handling, terminal I/O, web client |
| `zellij-utils/` | shared: IPC contracts, config/KDL parsing, errors, plugin API |
| `zellij-tile/`, `zellij-tile-utils/` | plugin-facing API |
| `default-plugins/*` | builtin plugins, each a wasm crate |
| `zellij-integration-tests/` | whole-app tests |
| `src/tests/e2e/` | dockerized black-box e2e (`docker compose up -d`, `cargo x ci e2e`) |
| `zellij-server/tests/` | remote panes against a real sshd (`cargo x remote-test`) |
| `docker/` | the containers those two suites run against |
| `xtask/` | the build system |

Read `docs/ARCHITECTURE.md` before changing server internals, and
`docs/TERMINOLOGY.md` for pty/ANSI/CSI vocabulary.

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
- Changing anything under `zellij-utils/assets/` (plugins, protobufs): regenerate with
  `cargo x proto` / `cargo x assets`, and CI's `assets --check` must pass.

## Agent state from Claude Code hooks

A `claude` running in a pane can report its own busy/waiting/idle state instead of being
guessed at from its screen — `zellij hocus hooks install` wires Claude Code's hooks to
`hocus-agentd`. See `docs/AGENT_HOOKS.md`. Unrelated to the `hooks { … }` config block
(`zellij-utils/src/input/hooks.rs`), which binds a plugin to a moment in the session.

## Lua plugins

Plugins come in two kinds: wasm modules (every builtin) and Lua scripts. See
`zellij-server/src/plugins/lua/CLAUDE.md` before changing the Lua runtime; `docs/LUA_PLUGINS.md`
is the user-facing guide.
