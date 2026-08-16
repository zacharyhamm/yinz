# AGENTS.md

Terminal workspace, Rust cargo workspace. Server/client split over IPC, plus builtin
plugins compiled to wasm.

## Build & test — use `cargo xtask`, not bare `cargo`

```sh
cargo x build            # builds plugins (wasm32-wasip1) then the binary
cargo x test             # plugin tests + per-crate tests
cargo x format           # rustfmt; --check in CI
cargo x run -- -l strider
cargo x integration-test # whole-app tests (needs cargo-nextest)
cargo x bench            # render throughput benchmarks (criterion); not part of CI
cargo x remote-test --build && cargo x remote-test --test  # remote panes, real ssh in docker
```

`cargo x` is an alias for `cargo xtask`. `cargo x --help` lists everything.

Requires `protoc` (protobufs cross the plugin wasm boundary).

**Never set `CARGO_TARGET_DIR` or change `build.target-dir`.** Debug builds embed
plugins from `<repo>/target/wasm32-wasip1/debug`, resolved at compile time; moving the
target dir breaks debug builds. Symlink `target/` instead. See CONTRIBUTING.md.

CI runs: `cargo x build`, `cargo x test`, `cargo x test --no-web`,
`cargo x integration-test`, `cargo x assets --check`, `cargo x format --check`,
`cargo test -p xtask` — on Linux and macOS.

### Passing arguments

xtask's own flags come first (`--no-web`, `--release`, …); everything after `--` is
forwarded — but each command forwards to a different place:

| command | trailing args land in |
|---|---|
| `x test` | libtest, in **every** crate's test binary (after `cargo test --`) |
| `x integration-test` | `cargo nextest run` — nextest flags, not libtest ones |
| `x build`, `x install` | the native `cargo build`, repeated per crate (never the wasm plugin build) |
| `x run` | zellij itself (after `cargo run --`) |
| `x bench` | criterion (after `cargo bench --`) — criterion flags, not libtest ones |
| `x ci e2e` | the e2e test runner |

So `cargo x test -p zellij-server` is rejected by xtask (`unexpected flag: -p`), and
`cargo x test -- -p zellij-server` reaches every plugin test binary as a libtest option,
failing the plugin phase. **`x test` has no crate filter.** To narrow a run:

- **One test, every crate:** `cargo x test some_test_name` — a bare filter needs no `--`.
  Harness flags do: `cargo x test -- --nocapture --test-threads=1`.
- **One crate, inner loop:** bare cargo is fine here, and is exactly what the per-crate
  phase runs — `cargo test -p zellij-server --all-features -- some_filter`, from the repo
  root, never with `CARGO_TARGET_DIR` set. For a plugin crate add
  `--target "$(rustc -vV | sed -n 's/^host: //p')"`: `default-plugins/*/.cargo/config.toml`
  pins the build to wasm, and wasm tests can't run. Tests that load a plugin at runtime
  need `cargo x build -p` (plugins only) first.
- **Integration tests:** `cargo x integration-test -- -E 'test(resize)'` (nextest
  expression), or a bare name filter, which works on either runner.
- **Remote-pane tests:** `cargo x remote-test --test -- --exact <name>`. They are
  `#[ignore]`d and need `--build` first (docker); see CONTRIBUTING.md.

Finish with a full `cargo x test` before calling work done — that's what CI runs.

## Benchmarks

`cargo x bench` runs `zellij-server/benches/multi_pane_render.rs`: N terminal panes tiled over
one surface, all fed synthetic pty bytes at once, measured as three criterion groups —
`ingest` (vte parse only), `frame` (parse + diff + serialize, the headline number) and
`render_only` (diff + serialize with ingest untimed). Five workloads × pane counts 1–16.

Benchmarks are **not** run by CI and are not part of `cargo x test` (the `[[bench]]` target sets
`test = false`). Run them by hand around a change to the render path:

```sh
cargo x bench -- --save-baseline main     # before
cargo x bench -- --baseline main          # after; criterion prints the deltas
cargo x bench -- frame/plain_text         # a bare filter narrows the run
```

First build is slow — `[profile.bench]` optimizes the whole server. Timings are only comparable
within one machine, and the workloads are seeded so input never drifts between runs.

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
