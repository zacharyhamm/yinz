# Code quality follow-up — 2026-09-22

This pass starts from the completed [previous review](CODE_QUALITY_REVIEW.md).
It ranks ten additional confirmed issues by data loss, permission boundaries, availability, and maintenance cost.
The review covered plugin execution, process supervision, IPC, terminal rendering, configuration, session discovery, and storage.
These are the strongest confirmed findings from this pass, not an exhaustive ranking of every code path.

| Rank | Issue and effect | Change |
| --- | --- | --- |
| 1 | **Kitty temporary-file cleanup trusted the spelling of a path.** Traversal or a directory symlink could authorize deletion outside temporary directories. | Resolve the path and temporary roots before checking deletion eligibility. Check the protocol marker on the resolved path. |
| 2 | **Plugin authoring permission depended on a virtual path prefix.** Access through `/host` or another alias could change the same plugin directory without `AuthorPlugins`. | Check the resolved physical directory. Protect its contents and ancestor operations that could remove or rename it. |
| 3 | **Filesystem watchers had session-wide ownership.** One plugin's request watched the server directory and broadcast paths to unrelated plugins. Host-folder changes did not update that watch. | Give each plugin environment its own watcher. Target its client and generation, restart on folder changes, and release the watcher with its owner. |
| 4 | **Permission saves used stale whole-cache snapshots and truncated the live file.** Concurrent saves could lose grants or restore revoked permissions. | Queue replacements and additive grants separately. Apply them to current data under a file lock, then publish a complete staged file atomically. |
| 5 | **Remote replay could race with live output.** A reconnect could replay a chunk between its insertion into the ring and selection of its live connection. | Select the live recipient while holding the same ring lock as replay completion. Each chunk takes one delivery path. |
| 6 | **Remote resume offsets excluded lost bytes.** After replay-buffer loss, later reconnects and warps requested an old stream offset and repeated output. | Count delivered and reported-lost bytes. Add a protocol message for further loss during catch-up, preserving output and notice ordering. |
| 7 | **SSH output pipes were not drained while children ran.** Verbose bootstrap or forwarding processes could block. Bootstrap timeout also left its child unreaped. | Share bounded output capture across bootstrap and forwarding. Retain the last 64 KiB per stream, cancel readers during teardown, and reap children on failure. |
| 8 | **UI coordinate arithmetic could overflow.** Extreme coordinates sent through DCS could panic during one-based cursor conversion. The component parser also mixed character and byte offsets. | Saturate coordinate additions while preserving existing terminal clamping. Validate full coordinate syntax and use byte-aware search in component parsing. |
| 9 | **Legacy OSC colors lacked length validation.** Oversized hexadecimal colors could overflow a shift and panic. Malformed lengths were otherwise accepted. | Accept only the supported component lengths before conversion. |
| 10 | **Clipboard input errors skipped child cleanup.** A command that closed stdin early left its process unreaped. | Always transfer the child to the existing reaper, then return the input error. |

## Regression coverage

1. [Kitty parser regressions](../zellij-terminal/src/panes/kitty_graphics/unit/parser_spec_tests.rs) cover discarded traversal components and directory symlinks.
2. [Filesystem permission tests](../zellij-plugin-host/src/zellij_exports.rs) cover virtual aliases, ancestor operations, reads, and explicit authoring grants.
3. [Watcher tests](../zellij-plugin-host/src/watch_filesystem.rs) cover separate roots, clients, generations, and host-folder changes.
   [Deno plugin tests](../zellij-plugin-host/src/unit/plugin_tests/deno_plugins.rs) cover event delivery through the plugin thread and reload.
4. [Permission-cache tests](../zellij-config/src/unit/permission_cache_tests.rs) cover concurrent writers, revocations, additive grants, corrupt files, symlinks, and readers holding the old inode.
5. [Remote session tests](../zellij-process/src/unit/pty_host_remote_session_tests.rs) cover replay versus live delivery at attachment.
6. [Remote reader tests](../zellij-process/src/unit/pty_host_remote_reader_tests.rs) cover consecutive lossy reconnects, additional gaps, notice ordering, and resulting offsets.
   [Broker tests](../zellij-process/src/unit/pty_host_remote_broker_tests.rs) force replay-buffer loss during catch-up.
7. [SSH capture tests](../zellij-process/src/unit/pty_host_remote_ssh_tests.rs) produce more output than pipes can hold and check bounded tails with writers still open.
8. [Grid input tests](../zellij-terminal/src/panes/unit/grid_tests/sixel_and_ui.rs) feed extreme coordinates through DCS handling and malformed Unicode directly into component parsing.
   The current VTE parser discards non-ASCII DCS bytes, so the Unicode panic was not reachable through that production caller.
9. The same grid tests feed malformed legacy colors through OSC handling.
10. [Clipboard tests](../zellij-panes/src/tab/copy_command.rs) force a broken pipe and verify that the child is reaped.

The integration run also exposed two fixture defects in the workflow tests.
The [fixture](../zellij-integration-tests/tests/swamp_exit_fixture.ts) now assigns a distinct run ID to each launch, matching the production ownership checks.
The [click helper](../zellij-integration-tests/tests/swamp_exit.rs) waits for the workflow subtitle and bottom help before locating a button.
This waits for the content geometry to settle after the subtitle appears.

The native SSH follow-up exposed another environment-policy gap.
The broker trusted received environment pairs without applying the terminal-and-locale allowlist.
It now applies the existing allowlist when composing each pane's environment.
The regression checks the environment before a login profile can hide incorrect values.
The Ubuntu fixture also generates `en_US.UTF-8`, so its profile does not replace the test's locale with a fallback.

## Validation

`cargo x ci check` passed after the fixes:

- Formatting, dependency checks, Rust lint, and TypeScript checks passed.
- Workspace tests: **8,382 passed**, 51 skipped.
- Documentation examples: **46 passed**, 3 ignored.
- Whole-app integration tests: **450 passed**, none skipped.

No Rust or TypeScript dependencies or snapshots changed. `git diff --check` passed.

`cargo x remote-test --build` failed before starting containers or running SSH tests.
The V8 149.4.0 build requested
`librusty_v8_simdutf_release_x86_64-unknown-linux-musl.a.gz`; the download returned HTTP 404.
The [upstream release](https://github.com/denoland/rusty_v8/releases/expanded_assets/v149.4.0) has no musl artifacts.
Checks of the OpenAI, Openworkers, and Supabase releases did not find an exact compatible replacement.
The SSH harness now offers `cargo x remote-test --build --native` on Linux.
It uses this checkout's GNU development binary in an Ubuntu container and keeps the same SSH and network-interruption tests.
The default musl build remains available for environments that supply its dependencies.
`cargo x remote-test --build --native` succeeded, and `cargo x remote-test --test` passed **all 41 tests**.
The focused remote-environment suite also passed all 10 tests.
The test containers and generated credentials were removed with `cargo x remote-test --clean`.
The remote broker, replay, reconnect, and SSH capture unit tests passed in the workspace gate.

## Remaining limits

Remote peers must both support **remote protocol version 8**. Older peers are refused during the existing version handshake.

- Path checks still resolve names before filesystem operations. Concurrent replacement of an ancestor requires descriptor-relative operations to close that race.
- A filesystem event queued before a host-folder change can arrive afterward. It belongs to the same plugin's previously accessible root.
- Permission changes explicitly replacing the same plugin's grants remain last-writer-wins. Additive grants merge with the current persisted grants.
- Permission publication syncs file contents, but not the parent directory. It does not provide a complete power-loss durability guarantee.
- SSH capture retains only the last 64 KiB per stream. Cancellation permits a bounded final drain, so descendants cannot keep teardown alive indefinitely.
- The native SSH build uses the host's runtime libraries. Hosts that need libraries newer than Ubuntu 24.04 may need an updated test image.
