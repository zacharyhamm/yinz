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
