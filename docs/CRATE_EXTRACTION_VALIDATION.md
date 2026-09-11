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
dev-dependency enables. The server re-exports `panes` and `tab` so external paths are unchanged.

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
