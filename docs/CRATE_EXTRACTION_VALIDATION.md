# Crate extraction validation

Validated on Linux with socket-capable execution on 2026-09-05. Integration runs used 16 workers.

## Gates

| Stage | Checks | Result |
| --- | --- | --- |
| Before extraction | `cargo x ci check` | Formatting, lint, 7,272 unit tests, doctests, and 348 integration tests passed |
| Process extraction | `cargo x test -p zellij-process` | 390 tests and doctests passed |
| Process server boundary | Filtered server OS and callback tests | 42 tests passed |
| Process extraction | `cargo x ci check` | Formatting, lint, 7,273 unit tests, doctests, and 348 integration tests passed |
| Process real SSH | `cargo x remote-test --build`, then `--test` | Build and all 41 tests passed |
| Process real VM | `cargo x firecracker-test --build`, then `--test` | Build and all 3 tests passed with host `/dev/kvm` access |
| Terminal extraction | `cargo x test -p zellij-terminal` | 835 tests and doctests passed |
| Terminal server boundary | Filtered terminal and plugin pane tests | 66 tests passed |
| Final extraction | `cargo x ci check` | Formatting, lint, 7,273 unit tests, doctests, and 348 integration tests passed on the repeated gate |

The first final gate passed 347 of 348 integration tests. `fullscreen_suspends_the_list_and_restores_it` captured an extra `[2]` tab-bar marker.
The pane list matched its snapshot. The test waits for that list but does not wait for the plugin's pane marker to catch up.
The nearby `tab_bar_names_the_selected_member` helper documents this asynchronous update.
The named test passed with `cargo x integration-test -j 4 -E 'test(fullscreen_suspends_the_list_and_restores_it)'`.
The complete gate then passed without changing the test or its snapshot.

## Test and fixture reconciliation

`cargo nextest list --workspace --message-format json` discovered 7,665 tests before extraction and 7,666 after each extraction.
This includes the 45 tests skipped by the ordinary unit gate. Real SSH and VM tests retain their existing server integration targets.

Every original test remains accounted for by its binary and test name. Reconciliation maps the extracted library binaries back to `zellij-server`.
The one moved orchestration case, `terminal_pane_grid_shows_ab_around_captured_apc`, maps from the Kitty interceptor module to the server's terminal-pane tests.
The only added test is `os_input_output_unix::callback_tests::terminal_exit_callback_preserves_id_status_and_command`.
It verifies the terminal ID, optional exit status, and complete command at the server callback boundary.

| Library | Discovered tests after extraction |
| --- | ---: |
| `zellij-process` | 390 |
| `zellij-terminal` | 835 |
| `zellij-server` | 2,289 |

All 1,093 snapshot bodies match their original SHA-256 hashes after mapping their relocated paths.
Only snapshot paths, crate/module prefixes, and source metadata changed.
All 82 moved grid-only fixtures match the original bytes. Shared replay fixtures remain in `src/tests/fixtures`.

Cargo metadata confirms that neither extracted crate depends on `zellij-server` or on the other extracted crate, including development dependencies.
The lockfile adds only the two workspace packages. No external package versions changed.

## Outstanding validation

The final gate has not been run on macOS. Run `cargo x ci check` on a macOS host before treating cross-platform validation as complete.

## Bus extraction (2026-09-11)

`zellij-bus` takes the thread bus out of `zellij-server`: `ThreadSenders`/`Bus`, the six
instruction enums with their payload types, `ServerOsApi` and its Unix implementation,
`route_action`, warps, background jobs, the session recorder, session layout metadata, the global
tokio runtime, `LoadingIndication` and `FrameColorOverride`. A preparatory commit first carved the
instruction types out of their thread modules and split `route_thread_main` (which holds
`SessionMetaData` and stays in the server as `client_router.rs`) from `route_action`, so the
extraction itself is `git mv` plus re-export shims.

| Stage | Checks | Result |
| --- | --- | --- |
| Before extraction | `cargo nextest list --workspace` | 8,381 tests discovered |
| Carve-out commit | `cargo x lint`, `cargo x test -p zellij-server` | Lint clean, 2,491 tests passed; 8,381 discovered, four `route::tests` now under `client_router::tests` |
| Bus extraction | `cargo x test -p zellij-bus` | 191 tests passed |
| Bus extraction | `cargo x test -p zellij-server` | 2,300 tests passed |
| Bus extraction | `cargo x ci check` | Formatting, lint, dependency boundaries, unit tests, doctests, and 390 integration tests passed at 16 workers |

`cargo nextest list --workspace` discovers 8,381 tests after the extraction, the same as before.
Every test keeps its name apart from its crate and module prefix: `frame_color_override` and
`loading_indication` tests no longer sit under `panes` and `ui`, and the route tests that moved
with `route_thread_main` are under `client_router`. No snapshot moved in this step.

The server drops `isahc`, `futures-lite`, `sysinfo`, `close_fds`, `signal-hook`, `once_cell` and
`rusqlite`, which only the moved code used. `xtask`'s dependency table gains rows for
`zellij-bus`, and for `zellij-terminal` and `zellij-process`, which the earlier round left
unenforced. The gate was run on Linux; the macOS gate remains outstanding.

## Plugin host extraction (2026-09-11)

`zellij-plugin-host` takes `zellij-server/src/plugins/` whole (wasm bridge, loader, plugin map,
pinned executor, pipes, workers, host functions, the Lua runtime and its tests), plus
`logging_pipe.rs`, `plugin_authoring.rs` and `get_engine`. Its outward calls (`route_action`,
`warp`, `session_scan_state`, `server_binary`) all resolve in `zellij-bus`, so no dependency
injection was needed. The server aliases the crate as `crate::plugins` and re-exports
`check_lua_plugin`, `get_engine` and `plugin_authoring`, so `src/` and the integration tests are
unchanged. The `hocus` plugin's `include_str!` of the Lua prelude follows the files.

| Stage | Checks | Result |
| --- | --- | --- |
| Host extraction | `cargo x test -p zellij-plugin-host` | 534 tests passed |
| Host extraction | `cargo x test -p zellij-server` | 1,766 tests passed |
| Host extraction | `cargo x test -p zellij-bus` | 191 tests passed |
| Host extraction | `cargo x ci check` | Formatting, lint, dependency boundaries, 7,945 unit tests, doctests, and 390 integration tests passed at 16 workers |

`cargo nextest list --workspace` discovers 8,381 tests, the same as before either extraction;
the plugin tests are named `plugin_tests::…` in `zellij-plugin-host` instead of
`plugins::plugin_tests::…` in `zellij-server`. All 146 moved snapshots match their original
bodies by SHA-256 after the `zellij_server__plugins__` → `zellij_plugin_host__` rename; only the
`source:` header changed (`cargo insta test --force-update-snapshots`).

The first gate run had one red test, `zellij-session warped_tab::test::a_stacks_members_arrive_in_the_order_they_were_stacked`,
a hegel property test in a crate this extraction does not touch. It passed 25 of 25 runs alone
and in a 150-run loop, and the repeated gate passed without any change, so it is recorded here as
a flake to watch rather than as a regression.

The server drops `mlua`, `wasmi`, `wasmi_core`, `wasmi_wasi`, `prost`, `notify-debouncer-full`,
`highway`, `num_cpus`, `chrono`, `zellij-plugin-api`, `zellij-tile-markdown`, `serial_test` and
`hocus-proto`. The gate was run on Linux; the macOS gate remains outstanding.

## Pane stack extraction (2026-09-11)

`zellij-panes` takes `panes/`, `tab/`, `ui/` and `pane_groups.rs` whole, with their tests and
299 snapshots. The moved files keep their `crate::` paths through aliases at the crate root
(`crate::os_input_output`, `crate::plugins`, `crate::screen`, ... resolve to the bus modules), the
same device `zellij-server` uses. The `pub(crate)` pane API becomes `pub`, and the test helpers
the server's screen tests drive sit behind a `test-support` feature that the server's
normal dependency enables for consistent build and test selection. The server re-exports `panes`
and `tab` so external paths are unchanged.

| Stage | Checks | Result |
| --- | --- | --- |
| Pane extraction | `cargo x test -p zellij-panes` | 919 tests passed |
| Pane extraction | `cargo x test -p zellij-server` | 847 tests passed |
| Pane extraction | `cargo x test -p zellij-bus`, `-p zellij-plugin-host` | 191 and 534 tests passed |
| Pane extraction | `cargo x ci check` | Formatting, lint, dependency boundaries, 7,945 unit tests, doctests, and 390 integration tests passed at 16 workers |

`cargo nextest list --workspace` discovers 8,381 tests, as before every extraction. All 299 moved
snapshots match their original bodies by SHA-256 after the `zellij_server__` → `zellij_panes__`
rename; only the `source:` headers changed. The server drops `kasuari`, `zellij-tile`,
`ansi_term`, `base64` and `png`. The gate was run on Linux; the macOS gate remains outstanding.

After the three extractions the workspace discovers the same 8,381 tests, distributed as
`zellij-bus` 191, `zellij-panes` 919, `zellij-plugin-host` 534, `zellij-server` 847 (plus its
two `#[ignore]`d integration targets), with the remainder unchanged.

## Post-extraction validation (2026-09-12)

A second pass over the finished branch, run on Linux (64 cores) from a warm tree, before the
branch is handed off. It re-checks the three extractions together rather than each in turn, and
runs the two gates the per-step tables above never repeated after the bus move.

| Stage | Checks | Result |
| --- | --- | --- |
| Static reconciliation | `cargo nextest list --workspace` | 8,381 tests: `zellij-bus` 191, `zellij-panes` 919, `zellij-plugin-host` 534, `zellij-server` 847 (+ 41 SSH, 4 VM, ignored) |
| Static reconciliation | SHA-256 of every snapshot body under the four crates, main vs branch | 655 on each side, 655 bodies identical, 0 added, 0 deleted, 445 relocated |
| Static reconciliation | `cargo metadata` workspace edges | No back-edge: bus never names panes, plugin-host or server; panes and plugin-host never name each other or server (normal, dev and build deps) |
| Static reconciliation | Every `#[path]` unit module and fixture path in the four crates | All 207 source files reachable; all 21 `#[path]` targets and every `../src/tests/fixtures`, `CARGO_MANIFEST_DIR/..` and `../example-plugins` path resolve, because each new crate is a depth-1 sibling of `zellij-server` |
| Dependency cleanup | `cargo x lint` after dropping what the move left unused | Clean. Removed: `zellij-server` `regex`, `strum`, `strum_macros`, dev `lazy_static` (`vte` moved to dev, its only use is a screen test); `zellij-bus` `serde`, `lazy_static`; `zellij-panes` `zellij-tile-utils` (a local `ellipsize` superseded it) and dev `zellij-test-support`. `get_engine`, `ServerInstruction` (server) and `PinnedExecutor` (host) narrowed to `pub(crate)`; the bus's unused `output` alias removed |
| Per-crate | `cargo x test -p zellij-bus` | 191 passed |
| Per-crate | `cargo x test -p zellij-panes` | 919 passed |
| Per-crate | `cargo x test -p zellij-plugin-host` | 534 passed |
| Per-crate | `cargo x test -p zellij-server` | 847 passed, 45 skipped (the ignored SSH and VM targets) |
| Per-crate | `cargo clean -p` then `cargo build -p zellij-panes`, `-p zellij-plugin-host` | Each builds without `zellij-server` in the graph |
| Full gate | `cargo x ci check` | Formatting, lint, dependency boundaries, 7,945 unit tests and doctests passed; integration 388 of 390 at 16 workers, see below |
| Full gate | `cargo x integration-test -j 16`, repeated | 390 passed |
| Real SSH | `cargo x remote-test --build`, then `--test` | Build and all 41 tests passed; `zellij-server/tests/remote_pane_ssh.rs` reaches `AsyncReader` through the bus re-export, so this exercises the moved boundary |
| Real VM | `cargo x firecracker-test --build`, then `--test` | Build and all 4 tests passed with host `/dev/kvm` access |
| Smoke | `cargo x build`, then a detached `strider` session driven over the CLI | `tab-bar`, `strider`, `status-bar` and `about` loaded through `zellij-plugin-host`; `zellij plugins check`, `write` and `reload` of `clock.lua` (the `plugin_authoring` path in `src/commands.rs`) worked with a client attached, the Lua plugin opened in a pane, `new-tab` and `dump-layout` answered, and the session was killed and deleted. Note that `reload` with no client attached is refused with "Plugins must have a client id", as it was before |

The first gate's two red integration tests were both timing, and both passed 10 of 10 in a loop
alone. A sibling worktree was running its own nextest at the time (load average 25 on 64 cores).

- `clients::multiple_users_in_different_tabs` waited for `Tab #2` to appear but the snapshot
  asserts the second client's `[1]` marker beside it, which the tab bar draws a frame later: the
  same predicate gap as `fullscreen_suspends_the_list_and_restores_it` above. Both waits in the
  test now require `[1] Tab #2`, so the test no longer depends on the frame it happens to catch.
- `hocus_agent_picker::vm_tab_focuses_a_pane_in_another_session_and_dismisses_the_panel` timed
  out its 10s wait for the remote VM pane to render, with an empty grid; alone it renders in
  under 2s. Nothing in it touches the moved code, and it is left as is.

The 16 relocated snapshots whose `source:` header still named `zellij-server/src/…` (insta did not
rewrite them because no test writes them, see the orphan note below) now carry their crate's path;
bodies are unchanged, 655 of 655 still hash identical to main.

Stale path citations from the move were corrected in `docs/SELF_PROGRAMMING.md` (its test recipe
now names the three crates, without which its `lua`, `pipe` and `plugin_` filters match nothing),
`docs/AGENT_HOOKS.md`, `docs/ERROR_HANDLING.md`, the bench's module doc, two doc comments in
`zellij-types`, and the kitty-graphics demo scripts (left over from the terminal extraction).
`docs/ZELLIJ-SERVER-TESTS.md` and `docs/ZELLIJ-SERVER-TEST-AUDIT.md` are pre-breakout audit
state and carry a banner saying so; `cargo nextest list --workspace` is the current per-crate listing.

### Outstanding

- **macOS gate.** Still not run on a macOS host, for this round or the previous one.
- **Crate-root `crate::` aliases.** `zellij-bus/src/lib.rs`, `zellij-panes/src/lib.rs` and
  `zellij-plugin-host/src/lib.rs` keep `pub use` aliases (`ClientId`, `channels`, `pty_host`, the
  bus module names) so moved files compile with their old `crate::` paths, and
  `zellij-server/src/route.rs` is a forwarding module. Roughly 3,600 sites, most of them in
  `zellij-panes` tests, still say `crate::TerminalId`. A mechanical follow-up can rewrite them to
  direct paths and delete the alias blocks; nothing depends on the order.
- **25 orphan snapshots**, all present on main before the breakout and carried across unchanged:
  the 13 `send_cli_*` files that are the whole of `zellij-server/src/unit/snapshots/`, 3 under
  `zellij-server/src/unit/screen_tests/snapshots/`, the 8 flat-named files that are the whole of
  `zellij-panes/src/tab/unit/snapshots/`, and
  `zellij-panes/src/panes/tiled_panes/unit/snapshots/…break_focused_pane_out_of_stack.snap`. No
  test writes them; `cargo insta test --unreferenced=delete` on each crate would remove them.
- The optional plugin-thread inversion and moving the bench into `zellij-panes`, as noted in the
  original plan, are not started.

## Dispatch and queue ownership (2026-09-12)

`zellij-routing` now owns the synchronous `route_action` dispatcher, its helpers, completion
waits and routing tests. It depends on Bus, Types, Runtime and IPC. The server and plugin host
call it directly; the server keeps its internal `crate::route` re-exports. Completion payloads
and their tests remain at `zellij_bus::route`. The malformed-frame limits live in the server's
client router. Plugin job queues and their tests now live in `zellij-plugin-host::queues`.

The moved dispatcher and completion-wait bodies are unchanged apart from bus import paths;
the queue implementation and tests are byte-for-byte moves. Clippy passes without the copied
allowance headers in bus `plugins.rs`, bus `pty.rs` and server `client_router.rs`.

Cargo's resolved dependency graph has no path from bus or panes to routing. The dependency
boundary test checks prohibited bus/panes → routing and routing → host/server/panes edges for
normal, build, dev, optional, target-specific and renamed dependencies, while permitting host → routing.

Validation on Linux:

- Focused routing, completion, queue, batching and xtask checks: 71 passed.
- First full gate: format, lint, dependency boundaries, doctests and all 390 integration tests
  passed. Of 7,945 unit tests, the notes/todos CLI test initially reported no running
  `hocus-store` immediately after session creation. It passed alone and in the next full unit
  run, without code changes.
- Second `cargo x ci check`: all checks passed, including 7,945 unit tests, doctests and
  390 integration tests at 16 workers.
- Test inventory regenerated with its existing `--fast` scope; test count unchanged.

## Panes feature selection (2026-09-12)

`zellij-server` enables `zellij-panes/test-support` through its normal dependency. The duplicate
dev-dependency is removed. Build, run, integration tests, unit tests and lint now select this
feature consistently. The helpers compile in release builds too, but production code does not
invoke them. Helper guards, behavior, public signatures, dependency boundaries and scheduling
are unchanged.

Other dependency features, profiles and Clippy artifacts can still require separate compilation.

Validation on Linux:

- `cargo tree -p zellij-server -e normal,build,features` and the same command with
  `-e normal,build,dev,features` both enable `zellij-panes/test-support`.
- `cargo x build` passed. Verbose compiler output confirms `feature="test-support"`.
- `cargo x ci check` passed with socket access: dependency boundaries, formatting, lint,
  7,945 unit tests, doctests and 390 integration tests at 16 workers.
- Verbose gate output shows cached panes artifacts for lint and unit tests. Integration
  compilation also selects `test-support`. Its separate panes artifact has the same feature
  and profile, but different dependency fingerprints.
