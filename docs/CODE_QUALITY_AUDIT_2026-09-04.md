## 1. Executive Summary (Minto Top)

- **Plugin trust boundaries permit host compromise.** A file planted in the startup directory can impersonate a privileged builtin; separate permission bypasses let untrusted plugins modify installed plugin code.
- **An empty layout search can still change the workspace.** Applying the invisible previous selection can close existing panes under the default retention settings.
- **One plugin can disrupt the entire session.** Uninterruptible guest execution can block shared plugin management, while unbounded event queues and command-output capture can exhaust server memory.
- **Recovery data can be lost without a reliable failure indication.** Checkpoint replacement can destroy the previous usable snapshot, and failed recording writes discard events while leaving recordings apparently complete.
- **Server disconnects can strand CLI automation.** Action and pipe clients repeatedly read EOF, hanging indefinitely and consuming CPU.

## 2. Pyramid of Findings (Highest Impact First)

### Theme A: Plugin privileges do not follow executable provenance — Impact: P1 — Confidence: High

Governing thought: The permission model trusts plugin identity and access paths that do not reliably constrain the code actually loaded or modified.

- Finding A.1: Startup-directory files can impersonate privileged builtins (P1, High)
  - Where: `zellij-utils/src/input/plugins.rs:119`, `zellij-utils/src/input/plugins.rs:185`, `zellij-utils/src/input/plugins.rs:283`, `zellij-utils/src/input/plugins.rs:408`, `zellij-server/src/plugins/zellij_exports.rs:7091`, `zellij-server/src/lib.rs:895`
  - Problem: A builtin declaration becomes a relative filename, and lookup tries that filename and its `.wasm` variant before installed builtins. The loaded bytes retain builtin status because `is_builtin()` checks the original location variant; builtin commands receive unconditional permission.
  - Impact: Starting a new session in an attacker-controlled checkout containing a valid `tab-bar.wasm`, for example, can execute host commands without a permission prompt. This requires a matching builtin in the layout and no successfully resolved explicit builtin-directory override.
  - Dimensions flagged by: Security/Supply Chain

- Finding A.2: WASI bypasses plugin-authoring permission (P1, High)
  - Where: `zellij-server/src/plugins/plugin_loader.rs:361`, `zellij-server/src/plugins/plugin_loader.rs:597`, `zellij-server/src/plugins/plugin_loader.rs:607`, `zellij-server/src/plugins/plugin_loader.rs:625`, `zellij-server/src/plugins/zellij_exports.rs:5522`
  - Problem: WASM instances receive `/plugins` as a preopened directory before permissions are granted. Direct WASI filesystem operations bypass the `Fs*` host commands that enforce `WriteFiles` and `AuthorPlugins`; the loader explicitly acknowledges this bypass.
  - Impact: Loading an untrusted WASM plugin when the user plugin directory exists allows modification of other plugin files, including after permission denial. Modified code executes when an affected plugin is subsequently loaded.
  - Dimensions flagged by: Security/Supply Chain

- Finding A.3: Alternate path spellings bypass authoring checks (P1, High)
  - Where: `zellij-server/src/plugins/zellij_exports.rs:5398`, `zellij-server/src/plugins/zellij_exports.rs:5414`, `zellij-server/src/plugins/zellij_exports.rs:5488`, `zellij-server/src/plugins/zellij_exports.rs:5522`, `zellij-server/src/plugins/zellij_exports.rs:5648`
  - Problem: The authoring check tests `resolved.root == Some(PLUGINS_FS_ROOT)`, preserving the caller’s virtual-root label rather than classifying the actual destination. A protected plugin file reached through an overlapping `/host` root therefore avoids the check.
  - Impact: A Lua plugin with `WriteFiles` can overwrite plugin code without `AuthorPlugins` when its working directory contains the plugin directory—for example, a session started in the user’s home directory. An absolute path with `FullHdAccess` provides another route.
  - Dimensions flagged by: Security/Supply Chain

### Theme B: Displayed selection and executed selection disagree — Impact: P1 — Confidence: High

Governing thought: Layout search can execute a workspace-changing action against an item the interface no longer displays.

- Finding B.1: Zero search results leave a stale, actionable layout selection (P1, High)
  - Where: `default-plugins/layout-manager/src/screens/layout_list/mod.rs:25`, `default-plugins/layout-manager/src/screens/layout_list/mod.rs:154`, `default-plugins/layout-manager/src/screens/layout_list/mod.rs:320`, `default-plugins/layout-manager/src/screens/layout_list/mod.rs:637`, `default-plugins/layout-manager/src/screens/layout_list/mod.rs:670`, `zellij-server/src/tab/layout_applier.rs:342`
  - Problem: Rendering uses filtered results, but `selected_layout_index` changes only when a matching result exists. Enter and Alt+W still index the original collection; pane-retention flags default to false, and the server closes unmatched, nonretained panes.
  - Impact: With at least one valid layout, typing a query matching nothing and pressing Enter opens an unrelated layout. Pressing Alt+W can apply that invisible selection and close existing panes, risking running work.
  - Dimensions flagged by: Maintainability/Testing/Docs/UX

### Theme C: Plugin work lacks effective session-level resource isolation — Impact: P1 — Confidence: High

Governing thought: Guest memory limits and scheduling improvements do not bound shared locks, queued payloads, or captured subprocess output.

- Finding C.1: Runaway WASM can block the shared plugin dispatcher indefinitely (P1, High)
  - Where: `zellij-server/src/plugins/plugin_loader.rs:73`, `zellij-server/src/plugins/wasm_bridge.rs:474`, `zellij-server/src/plugins/wasm_bridge.rs:492`, `zellij-server/src/plugins/wasm_bridge.rs:1560`, `zellij-server/src/plugins/wasm_bridge.rs:2060`, `zellij-server/src/plugins/wasm_bridge.rs:2067`
  - Problem: Cancellation only takes effect at host-boundary crossings; CPU-only loops cannot be interrupted. Loading runs guest code while holding the global plugin-map lock, and subscription handling can hold that lock while waiting for a running instance.
  - Impact: A defective plugin looping during load—or after subscribing during an update—can freeze unrelated plugin event delivery and management while consuming a core. The cancellation watchdog cannot interrupt that execution.
  - Dimensions flagged by: Concurrency/Performance/Resources

- Finding C.2: Streaming traffic accumulates in unbounded queues (P1, High)
  - Where: `zellij-server/src/lib.rs:2278`, `zellij-server/src/background_jobs.rs:1076`, `zellij-server/src/background_jobs.rs:1217`, `zellij-server/src/plugins/wasm_bridge.rs:1601`, `zellij-server/src/plugins/pinned_executor.rs:318`, `zellij-server/src/plugins/pinned_executor.rs:338`
  - Problem: Coalescing limits only the producer’s current chunk. Flushed payloads enter an unbounded plugin channel and unbounded executor queues, with event cloning adding retained copies.
  - Impact: Streams arriving faster than a plugin processes them cause growing server memory and delayed interaction; sustained or concurrent streams can exhaust memory. Existing batching and input priority do not bound this backlog.
  - Dimensions flagged by: Concurrency/Performance/Resources

- Finding C.3: Standard command helpers capture unlimited output (P1, High)
  - Where: `zellij-tile/src/shim.rs:950`, `zellij-tile/src/shim.rs:965`, `zellij-server/src/background_jobs.rs:67`, `zellij-server/src/background_jobs.rs:475`, `default-plugins/hocus/src/tool_loop.rs:749`
  - Problem: Standard command helpers pass no output limit. The host appends all stdout and stderr to vectors and delivers the result only after the streams and process finish.
  - Impact: An authorized plugin launching a very verbose or continuously writing command can exhaust server memory independently of WASM limits. A bounded API exists, but these defaults and builtin callers do not use it.
  - Dimensions flagged by: Concurrency/Performance/Resources

### Theme D: Persistence failures undermine recovery guarantees — Impact: P1 — Confidence: High

Governing thought: Recovery paths can destroy or discard data without preserving an accurate account of what was saved.

**Remediation (2026-09-04):** Both findings were confirmed against the original code and fixed. The descriptions and line references below record the pre-fix behavior.

- Finding D.1: Checkpoint replacement can destroy the last usable recovery snapshot (P1, High)
  - Where: `zellij-server/src/background_jobs.rs:1347`, `zellij-server/src/background_jobs.rs:1357`, `zellij-server/src/background_jobs.rs:1364`, `zellij-utils/src/sessions.rs:666`, `zellij-utils/src/sessions.rs:828`
  - Problem: `File::create` truncates existing metadata, layout, and external files before replacement writes complete. Related files are updated independently, and metadata/layout write results are discarded.
  - Impact: Interruption or I/O failure during checkpointing can leave resurrection state empty, partial, or inconsistent. Concurrent session readers can also observe the truncation interval and omit otherwise live sessions.
  - Dimensions flagged by: Correctness/State
  - Fix: `zellij-utils/src/session_checkpoint.rs` stages a complete snapshot, syncs its files and directories, and publishes it through one atomic symlink replacement. A per-session file lock serializes writers and keeps readers on one snapshot through layout and pane-content loading. Unchanged files are reused, the preceding snapshot is retained, and legacy flat checkpoints remain readable. Periodic and explicit saves report errors; failed explicit saves do not update the reported save timestamp. Startup rejects unreadable committed checkpoints instead of silently loading a default layout.

- Finding D.2: Recording failures discard events without marking the recording incomplete (P1, High)
  - Where: `zellij-server/src/session_recorder.rs:267`, `zellij-server/src/session_recorder.rs:270`, `zellij-server/src/session_recorder.rs:470`, `zellij-server/src/session_recorder.rs:473`, `zellij-utils/src/session_recording.rs:170`
  - Problem: Failed transactional event writes are logged, followed by `batch.clear()`. These failures neither increment the dropped-event counter nor change the conditions used to mark a recording incomplete.
  - Impact: A transient storage failure followed by successful later writes can produce apparently complete recordings with missing terminal output or input, compromising replay and debugging.
  - Dimensions flagged by: Correctness/State
  - Fix: Every event batch passes through the shared flush helper in `zellij-server/src/session_recorder.rs`. A failed transaction increments the dropped-event count before the batch is cleared. The recorder retries the incomplete marker on later flushes and requires its final persistence before finalizing the session. Failed event batches are not retried. Consolidating the shutdown drain also fixes inventory item 15's duplicate tail write.

**Verification:** All 23 focused checkpoint, recorder, and session-list tests pass. Fault injection covers partial staged files, publication and final-sync failures, failed event batches, and failed incomplete markers. Additional cases cover concurrent readers/writers, legacy migration, metadata-only saves, unchanged-file reuse, cleanup, session rename, server startup, and exactly-once shutdown flushing. `cargo x ci check` passed (format check, clippy, all 6,802 tests, and doctests). `cargo x integration-test -- -j 16` passed all 344 tests, including viewport and stacked-pane resurrection. No snapshots were regenerated. New checkpoints require a binary that understands the snapshot format; existing flat checkpoints remain readable.

### Theme E: CLI receive loops fail to terminate on connection loss — Impact: P1 — Confidence: High

Governing thought: Transport termination is discarded before action clients decide whether to continue waiting.

- Finding E.1: Action and pipe clients busy-loop after EOF (P1, High)
  - Where: `zellij-client/src/cli_client.rs:167`, `zellij-client/src/cli_client.rs:213`, `zellij-client/src/cli_client.rs:237`, `zellij-client/src/cli_client.rs:265`, `zellij-utils/src/ipc.rs:369`, `zellij-utils/src/ipc.rs:391`
  - Problem: `.msg()` converts EOF into `None`, which both receive loops ignore. Subsequent reads of the closed socket return immediately, so the loops never recover or terminate.
  - Impact: A server disconnect while awaiting an action acknowledgement, pipe response, or blocking pane exit strands automation and can consume a CPU core per client.
  - Dimensions flagged by: Error Handling/Observability/Ops

## 3. Full Inventory (Grouped, Lower Impact Last)

The ten P1 findings are detailed above. The remaining fourteen findings are grouped below. Dimension abbreviations: **State**, **Ops**, **Resources**, **API**, and **Quality/UX** refer to their corresponding audit passes.

| # | Title | Severity | Confidence | Locations | Dimensions | 1-line Why It Matters |
|---|---|---|---|---|---|---|
| 11 | Plugin action serialization silently loses valid actions | P2 | High | `zellij-utils/src/plugin_api/action.rs:2121`; `zellij-utils/src/plugin_api/plugin_command.rs:724`; `zellij-utils/src/plugin_api/event.rs:1381`; `zellij-server/src/lib.rs:369`; `zellij-tile/src/shim.rs:3931` | API | Unsupported actions such as `Copy` disappear from keybinding reports/rebindings, potentially persisting lost behavior; `run_action` instead panics. |
| 12 | Permission-denied WASM queries have no refusal response | P2 | High | `zellij-server/src/plugins/zellij_exports.rs:963`; `zellij-server/src/plugins/plugin_map.rs:471`; `zellij-tile/src/shim.rs:330`; `zellij-tile/src/shim.rs:375` | API | Denied pane/tab queries produce no reply; Option-returning getters unwrap empty-input decoding and crash the plugin. |
| 13 | Schema declarations diverge from existing SQL constraints | P2 | High | `zellij-utils/src/plugin_data_store/migrate.rs:94`; `zellij-utils/src/plugin_data_store/migrate.rs:146`; `zellij-utils/src/plugin_data_store/registry.rs:227` | State | Changing an existing attribute’s required/default settings updates metadata without changing its column, producing different behavior on upgraded and fresh databases. |
| 14 | Hocus project changes report success after failed persistence | P2 | High | `default-plugins/hocus/src/tool_loop.rs:1406`; `default-plugins/hocus/src/tool_loop.rs:1422`; `default-plugins/hocus/src/tool_loop.rs:1536`; `default-plugins/hocus/src/transitions.rs:653` | Ops | Remember/update/forget mutates memory, discards the synchronous store error, and confirms success; restart or refresh can reverse the apparent change. |
| 15 | Recorder fallback shutdown inserts its final batch twice — resolved with D.2 | P2 | High | `zellij-server/src/session_recorder.rs:446`; `zellij-server/src/session_recorder.rs:455`; `zellij-utils/src/session_recording.rs:195` | State | A failed Stop enqueue followed by fallback draining can persist the same nonempty tail twice, duplicating replay content. |
| 16 | Rerun recordings remain marked ended | P2 | High | `zellij-server/src/terminal_bytes.rs:53`; `zellij-server/src/pty.rs:3538`; `zellij-utils/src/session_recording.rs:174`; `zellij-utils/src/session_recording.rs:287`; `zellij-utils/src/session_recording.rs:730` | State | Rerunning a held command appends events under the same terminal ID without reopening its recording row, leaving status and duration stale. |
| 17 | Background work survives plugin abandonment | P2 | High | `zellij-server/src/background_jobs.rs:457`; `zellij-server/src/background_jobs.rs:585`; `zellij-server/src/plugins/wasm_bridge.rs:658` | Resources | Closing/reloading a plugin does not cancel its commands or active streams, leaving processes and transfers consuming resources. |
| 18 | Pane subscriptions ignore downstream closure | P2 | High | `zellij-client/src/cli_client.rs:328`; `zellij-client/src/cli_client.rs:344`; `zellij-client/src/cli_client.rs:361` | Ops | Ignored stdout errors keep subscriptions and pipeline waits alive after an early-exiting consumer has gone. |
| 19 | Non-UTF-8 directory entries panic session enumeration | P2 | High | `zellij-utils/src/sessions.rs:83`; `zellij-utils/src/sessions.rs:93`; `zellij-utils/src/sessions.rs:200` | Ops | Filename conversion unwraps before socket filtering; even an unrelated regular file can break session discovery. |
| 20 | Linux ARM container tests select an x86-64 binary | P2 | High | `xtask/src/ci.rs:112`; `xtask/src/remote_test.rs:76`; `docker-compose.remote-panes.yml:12` | Ops | ARM recognition is restricted to macOS, causing Linux ARM workflows to fail or depend on unintended cross-compilation/emulation. |
| 21 | Cargo-test fallback misroutes selection flags | P2 | High | `xtask/src/test.rs:52`; `xtask/src/test.rs:62`; `xtask/src/test.rs:71` | Ops | Without nextest, `-p`, `-j`, and filter flags land after Cargo’s `--`, breaking documented narrowed validation commands. |
| 22 | Integration runner lacks the documented concurrency default | P2 | High | `xtask/src/integration_test.rs:35`; `.config/nextest.toml:1`; `AGENTS.md:113`; `AGENTS.md:120` | Quality/UX | Plain invocations do not enforce 16 workers despite documented many-core contention; explicit `-j 16` runs are unaffected. |
| 23 | Regression assertions depend on scheduler-selected frames | P2 | Medium | `zellij-server/src/unit/screen_tests/cli_actions.rs:747`; `zellij-server/src/unit/screen_tests/cli_actions.rs:805`; `zellij-server/src/unit/screen_tests/scroll_and_warp.rs:75`; `zellij-server/src/unit/screen_tests/mod.rs:765` | Quality/UX | Fixed sleeps followed by `snapshots.last()` can observe stale frames or miss intermediate behavior; actual flakiness was not reproduced. |
| 24 | Unicode width conventions disagree across UI helpers | P2 | High | `zellij-tile/src/ui_components/forms/text_input.rs:80`; `default-plugins/layout-manager/src/screens/layout_list/search.rs:135`; `default-plugins/layout-manager/src/ui/text_utils.rs:9`; `zellij-tile-utils/src/lib.rs:20` | Quality/UX | Character indices used as columns misplace CJK/combining-text cursors; inconsistent truncation overflows labels or removes content that fits. |

No P3-only hygiene items or Low-confidence findings were promoted.

## 4. Coverage & Blind Spots

- **Method deviation:** The harness rejected creation of the sixth distinct subagent with “agent thread limit reached.” Five distinct subagents completed six dimension-specific audits; the completed security worker performed the separate maintainability/testing/UX pass. Audits ran concurrently where permitted. No dimension was skipped.

- **Dimensions and substantive coverage:**
  - **Correctness/State:** checkpointing, resurrection readers, recording lifecycle, plugin-data schema/migration/write paths, and rerun behavior.
  - **Error Handling/Observability/Ops:** CLI receive/output loops, session discovery, Hocus persistence results, and xtask fallback/platform behavior.
  - **Security/Supply Chain:** builtin provenance, WASI capabilities, filesystem permissions, Lua sandbox entrypoints, SSH argument handling, socket permissions, manifests, and lockfile/toolchain searches.
  - **Concurrency/Performance/Resources:** plugin loading/execution/unload, streaming queues, background commands, and sampled PTY/client/remote backpressure.
  - **Architecture/API:** native–protobuf–SDK contracts, permission refusal, keybinding reporting and persistence.
  - **Maintainability/Testing/Docs/UX:** layout-manager interactions, shared text helpers, tab-bar/compact-bar callers, integration configuration, and targeted screen-test assertions. No merged dimension was N/A.

- **Breadth versus depth:** Every requested crate group was included in exploration. Builtin coverage was selective, with deeper inspection of Hocus and layout-manager and sampled coverage of other plugins. Complete grid/graphics correctness, SSH/Firecracker recovery state machines, IPC deserializer internals, every builtin interaction, every integration assertion, and all documentation were not exhaustively examined because of project size.

- **Runtime blind spots:** The original audit did not run tests, builds, benchmarks, exploit reproductions, fuzzing, or fault injection. Theme D remediation checks are recorded above. Memory-growth rates and timing-dependent failures remain unmeasured. Accessibility contrast, full keyboard reachability, and screen-reader behavior were not runtime-assessed. No current dependency-vulnerability or abandonment claim was established.

- **Deduplication and judgment:** Shared action-serialization failures were merged across reporting, rebinding, and execution; Unicode problems were grouped by their inconsistent width model. The two authoring bypasses remain separate because direct WASI access and alternate-root classification are independent mechanisms. No substantive inter-agent disagreement emerged. The final-frame test finding is rated Medium for its scheduling-dependent impact, despite certain evidence of the assertion pattern.

- **Counterevidence respected:** Transaction-wrapped schema migration was not labeled a partial-DDL failure. Previously documented repaired flaky tests were not reported as still flaky. File size, broad dependency ranges, and the absence of CI were not themselves treated as defects.

- **Exclusions:** `target/`, `.worktrees/` metadata, `target/wasm32-wasip1/` built WASM, `<data-dir>/builtins/<version>` installed artifacts, generated protobuf output, and `.git/`—**none beyond that**. Inspection used the requested active worktree. No files changed during the audit; Git status was clean when the audit concluded.
