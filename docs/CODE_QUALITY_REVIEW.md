# Code quality review — 2026-09-22

This review selected ten confirmed issues by impact, reach, and reproducibility.
The review covered transport and process boundaries, plugin execution, terminal rendering, configuration persistence, and the local validation tools.
Security, data integrity, resource ownership, and reliable verification took priority over cosmetic changes or splitting large files.

All ten findings below have implementation changes and regression tests in this worktree.
This is a ranked set of confirmed findings, not a claim that every path in the repository has been audited.

| Rank | Issue and effect | Resolution |
| --- | --- | --- |
| 1 | **Plugin filesystem confinement could fail for dangling symlinks.** The resolver treated an unresolved link as a missing path. A later write could create its target outside the permitted root. | Reject unresolved symlinks and propagate resolution errors. Open resolved files with `O_NOFOLLOW`. Preserve valid links within roots and authorized absolute links. |
| 2 | **Decoded notifications could inject terminal commands.** OSC 99 accepts base64 text. Forwarding that text through another notification protocol could emit its embedded terminal controls. | Sanitize text in the shared notification renderer with the existing control-character filter. |
| 3 | **Wasm table allocations were unbounded.** The plugin memory limit covered linear memory, but a table without a module maximum could allocate host memory without an element ceiling. | Limit each table to 1,048,576 elements, alongside the existing limit of 16 tables. Enforce the ceiling during allocation and growth. |
| 4 | **Configuration saves could leave incomplete live files or overwrite backups.** Saves truncated the live configuration before validation completed. Initial creation and backup allocation used separate existence checks and writes. | Stage, sync, and validate the replacement before atomic publication. Use exclusive publication for initial files and backups. Preserve existing configuration symlinks and file permissions. |
| 5 | **IPC writes could corrupt subsequent frames after an error.** A sender remained usable after a partial write. Its buffered writer could also retry bytes when dropped. | Encode each complete frame into one buffer. Make transport failure terminal for that sender. Keep pre-write validation failures recoverable. |
| 6 | **Remote pane writes reported success after socket failure.** The shared flush function swallowed errors. Release could append a control frame while an earlier frame remained incomplete. | Return the first write error to input and control callers. Refuse release until queued frames have drained. |
| 7 | **Plugin writes could block an executor on a FIFO.** Write access opened arbitrary filesystem objects with blocking flags and truncated before checking the opened object. | Open without blocking and validate the file descriptor before truncation. Restrict writes to regular files, matching the read-side contract. |
| 8 | **Sixel image ownership and clipping were inconsistent.** IDs derived from store length could overwrite another pane's image. Closed panes retained images. Coordinate mismatches prevented reaping and miscropped negative scrollback positions. | Allocate IDs in the shared store. Finalize image creation in one operation. Give each grid responsibility for cleanup on drop. Reuse rectangle intersection for clipping and correct covered-image comparisons. |
| 9 | **Command preflight disagreed with child execution.** Non-executable files could pass preflight. Relative PATH entries used the server directory. Explicit paths could fall through to PATH lookup. | Check executable access and resolve paths from the child's effective directory. Search PATH only for bare command names. |
| 10 | **Filtered package tests silently widened to the whole workspace.** `Selection::Skip` erased package selection when a test filter was present. This defeated the documented focused test command. | Track package scope and test filtering independently. Skip unsupported doctests without adding `--workspace` to a package-selected run. |

## Implementation and regression coverage

1. [Plugin filesystem resolver and I/O](../zellij-plugin-host/src/zellij_exports.rs), with [Deno host tests](../zellij-plugin-host/src/unit/plugin_tests/deno_plugins.rs).
   Tests cover dangling final links, dangling ancestor links, trailing separators, valid absolute links, and permission checks.
2. [Notification renderer](../zellij-server/src/notifications.rs), with [renderer tests](../zellij-server/src/unit/notifications_tests.rs) and a [pane-to-host notification test](../zellij-server/src/unit/screen_tests/desktop_notifications.rs).
   The latter sends a base64-encoded clipboard escape through the pane and verifies safe host output.
3. [Wasm resource limits and tests](../zellij-plugin-host/src/plugin_limits.rs).
   Tests invoke the actual Wasmi table allocation and growth APIs.
4. [Configuration persistence](../zellij-config/src/config.rs), with [file publication tests](../zellij-config/src/unit/config_write_test.rs).
   An open reader retains the complete old file after replacement. Other checks cover backups, symlinks, permissions, and validation failure.
5. [IPC sender](../zellij-ipc/src/ipc.rs), with [socket regressions](../zellij-ipc/src/ipc/tests/socket_tests.rs).
   A faulting stream checks partial writes, flush failures, and suppression of later writes.
6. [Remote connection](../zellij-process/src/pty_host/remote/conn.rs), with [connection tests](../zellij-process/src/unit/pty_host_remote_conn_tests.rs).
   Socket-pair tests verify immediate failure reporting alongside the existing queue and backpressure checks.
7. The plugin I/O tests also open a FIFO with a reader present.
   This checks rejection of a non-regular file without leaving the test dependent on a blocked open.
8. [Sixel ownership and geometry](../zellij-terminal/src/panes/sixel.rs), with [store and clipping tests](../zellij-terminal/src/panes/unit/sixel_tests.rs) and [grid lifecycle tests](../zellij-terminal/src/panes/unit/grid_tests/sixel_and_ui.rs).
   Tests cover cross-pane IDs, overwrite reaping, scrollback clipping, pane destruction, and primary/alternate screen reset.
   `Grid`, `SixelGrid`, and `AlternateScreenState` no longer implement `Clone`. Repository callers transfer ownership instead of duplicating it.
9. [Command resolution](../zellij-process/src/command.rs), with [execution comparison tests](../zellij-process/src/unit/command_tests.rs).
   The test compares preflight results against actual child execution for each PATH scenario.
10. [Test selection and regressions](../xtask/src/test.rs).
    Tests cover package selectors before and after both name filters and filtersets.

## Validation

`cargo x ci check` passed on the final code and test changes on 2026-09-22.

| Check | Result |
| --- | --- |
| Dependency boundaries | Passed |
| Rust and TypeScript formatting | Passed |
| All-features Clippy and plugin authoring checks | Passed |
| Workspace tests | 8,359 passed, 51 skipped |
| Doctests | 46 passed, 3 ignored |
| Integration tests, 16 workers | 450 passed, none skipped |

The two existing spawn regressions now distinguish preflight rejection from a genuine exec failure.
Their executable missing-interpreter fixtures still verify exec errors, PTY cleanup, and supervisor teardown.
No snapshots changed and no dependencies were added.
The separate real-SSH, Firecracker, and Postgres suites were not run. Validation was performed on Linux.

## Remaining limits

- Filesystem confinement still canonicalizes paths before opening them. Concurrent replacement of an ancestor directory requires descriptor-relative filesystem operations to eliminate the race completely.
- Configuration publication is atomic for readers. The code syncs file contents, but does not sync parent directories or provide a full power-loss durability guarantee.
- Whole-configuration saves still use last-writer-wins semantics. They do not merge simultaneous edits from different sessions.
- The Wasm table ceiling is per table. It is separate from the linear-memory budget.
