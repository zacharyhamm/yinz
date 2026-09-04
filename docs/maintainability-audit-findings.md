# Maintainability audit findings checklist

Audited project: `/home/zacharyhamm/hocuspocus/zellij` — all 30 workspace members.

Audited revision: `6e84ff04a169b59e6486ac4734986afae10194a9` (`main`). This document records the completed static audit; line references describe that revision. The branch advanced before this report was written. Unless an entry has a revalidation note below, it has not been revalidated against the newer HEAD.

Checkboxes track findings for review. They do not represent approved fixes or a remediation plan. All findings were unchecked when this report was created.

The A-group findings were revalidated against `4f3245ef4` and remediated on 2026-09-04. Their original evidence below describes the audited code; the remediation notes describe the fixes and regression coverage.

A-group validation: all 20 focused configuration/recorder tests passed. `cargo x ci check` passed format, lint, all 6,798 selected workspace tests (45 skipped), and doctests. `cargo x integration-test -j 16` passed all 344 whole-app tests with no skips.

## 1. Executive Summary (Minto Top)

- **Configuration saves can silently change existing behavior.** Custom `NewTab` layouts disappear during serialization, and the persistence check cannot detect that information loss.
- **Recording failures can leave incomplete data without an incomplete marker.** Failed event batches are discarded, while the available injected-backend tests exercise only successful writes.
- **Permission-denial tests provide false assurance.** Their shared observer stops before it can capture commands emitted after denial, leaving an important security regression check ineffective.
- **Remote plugin downloads confuse transport completion with artifact validity.** HTTP error bodies and incompatible resumed responses can become cached plugin files, causing persistent loading failures with misleading diagnostics.

## 2. Pyramid of Findings (Highest Impact First)

### Theme A: Persistence does not preserve the promised state — Impact: P1 — Confidence: High

Governing thought: Configuration and recording paths can lose information while their success checks fail to reveal the loss.

- [x] **Finding A.1: Saving configuration discards custom NewTab layouts** (P1, High)
  - Where: `zellij-utils/src/kdl/mod.rs:879`, `zellij-utils/src/kdl/mod.rs:1836`, `zellij-utils/src/kdl/mod.rs:5376`, `zellij-server/src/lib.rs:2891`, `zellij-utils/src/input/config.rs:459`, `zellij-utils/src/input/config.rs:489`.
  - Problem: The parser reads and loads a binding's custom layout, but the serializer explicitly ignores `tiled_layout`, `floating_layouts`, and both swap-layout fields. Configuration persistence compares two parses of the already serialized text, so its equality check cannot detect information discarded before writing.
  - Impact: When configuration containing a custom `NewTab` binding is rewritten, that binding can revert to the default layout. Saving an unrelated setting can therefore change which panes and commands an existing shortcut launches.
  - Trigger (revalidated): A custom-layout `NewTab` binding exists when a saved `RebindKeys` operation rewrites the whole configuration. Ordinary reconfiguration now persists a fragment and does not take this path.
  - Confidence basis: The parser, serializer, keybinding serialization, and persistence caller establish the complete information-loss path.
  - Dimensions flagged by: Duplication/Complexity.
  - Remediation: Preserve explicit layout references in `Action::NewTab`, KDL, IPC, and plugin action messages. Whole-config saves compare intended `NewTab` bindings with reparsed serialization before making a backup or writing. Unrepresentable bindings and externally changed layouts fail the save rather than silently changing the shortcut. Reload configuration after editing a referenced layout before saving keybindings.
  - Regression coverage: Named, absolute, and relative layout references; tiled commands, floating panes, both swap-layout kinds; implicit defaults; focus overrides; unrelated key rebinding and disk persistence; rejected writes leaving the original file and backups untouched; optional protocol fields and empty swap lists after IPC.

- [x] **Finding A.2: Failed recording batches disappear without recording an incomplete status** (P1, High)
  - Where: `zellij-server/src/session_recorder.rs:267`, `zellij-server/src/session_recorder.rs:470`, `zellij-server/src/session_recorder.rs:474`, `zellij-server/src/session_recorder.rs:550`, `zellij-server/src/session_recorder.rs:578`.
  - Problem: A failed `record_events` call is logged, then its batch is cleared. Finalization marks recordings incomplete for channel drops or disabled recording, but does not retain a preceding persistence failure; the injected test backend returns `Ok(())` for every write.
  - Impact: Playback can omit events without an incomplete marker. Existing channel-failure tests do not establish the behavior of this storage-failure path.
  - Trigger: An event write fails while recording remains enabled with zero channel drops, and later finalization succeeds.
  - Confidence basis: The error branch, batch disposal, finalization conditions, and success-only test backend are explicit.
  - Dimensions flagged by: Testing/Testability; principal source verification.
  - Remediation: All flush paths share one helper and retain the first contextual storage error. Metadata and finalization failures use the same error state. Finalization combines storage and channel-loss reasons in the existing incomplete marker. Failed batches are not retried because the backend contract does not promise retry safety; cleared drain batches cannot be written twice. The marker is written when a loss reason first appears or changes, before a closing pane gets its end timestamp, and a rejected write is retried at the next flush or pane operation; finalization still leaves the session unfinished if the marker cannot be saved.
  - Regression coverage: Injected failures followed by successful writes; size, timeout, and final flushes; metadata, pane-close, and session-finalization failures; combinations with dropped events or disabled recording; clean success and no duplicate writes during draining. Asynchronous observation uses `Watched`.

### Theme B: Security regression assertions stop observing too early — Impact: P1 — Confidence: High

Governing thought: A shared test helper makes the absence of forbidden commands inevitable after the permission handshake.

- [ ] **Finding B.1: Permission-denial tests cannot observe post-denial violations** (P1, High)
  - Where: `zellij-server/src/plugins/unit/plugin_tests/mod.rs:191`, `zellij-server/src/plugins/unit/plugin_tests/mod.rs:201`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:878`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:925`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:947`, `zellij-server/src/plugins/unit/plugin_tests/config_events.rs:16`.
  - Problem: The denial helper sends `PermissionStatus::Denied` and immediately breaks its logging loop, dropping the screen receiver. Tests subsequently send protected commands and assert their absence from the already finished log.
  - Impact: A regression that permits a command after denial can escape these assertions. Waiting longer afterward cannot repair the missing observation; this affects negative coverage for clipboard and other protected actions.
  - Trigger: A denied plugin emits a protected command after the permission handshake.
  - Confidence basis: Observer termination and subsequent command/assertion ordering are directly visible. This finding does **not** establish a production permission bypass.
  - Dimensions flagged by: Testing/Testability.

### Theme C: Layout deletion reports success before the host confirms it — Impact: P2 — Confidence: High

Governing thought: The layout manager removes an entry from its visible list after discarding the host deletion result.

- [ ] **Finding C.1: Failed layout deletion appears successful** (P2, High)
  - Where: `default-plugins/layout-manager/src/screens/layout_list/mod.rs:65`, `default-plugins/layout-manager/src/main.rs:176`, `zellij-server/src/plugins/zellij_exports.rs:6358`.
  - Problem: `let _ = delete_layout(...)` discards the synchronous host result before returning `OptimisticUpdate::Delete`. The visible list then removes the entry with `retain`.
  - Impact: A read-only filesystem, directory-permission error, or disappeared file produces a false removal signal.
  - Trigger: A layout deletion fails after the user presses Delete.
  - Confidence basis: Host errors, the discarded result, and list mutation are directly connected.
  - Dimensions flagged by: Docs/Onboarding/UX; Testing/Testability.

### Theme D: Remote plugin loading accepts invalid download outcomes — Impact: P1 — Confidence: High

Governing thought: The downloader's success result does not establish that its destination contains the requested artifact.

- [ ] **Finding D.1: Download success can mean missing, invalid, or incorrectly resumed content** (P1, High)
  - Where: `zellij-utils/src/downloader.rs:44`, `zellij-utils/src/downloader.rs:73`, `zellij-utils/src/downloader.rs:90`, `zellij-utils/src/downloader.rs:96`, `zellij-utils/src/downloader.rs:121`, `zellij-utils/src/downloader.rs:138`, `zellij-server/src/plugins/wasm_bridge.rs:450`.
  - Problem: HTTP-client construction errors become `None`, and an absent client produces `Ok(())`. Download responses are streamed without checking HTTP status or validating the requested range; an existing partial file is opened for append, then promoted to the final cache path.
  - Impact: An HTTP error body or full response appended to a partial download can become the cached plugin. Later loads accept the existing path and fail during artifact loading, obscuring the original network failure and making the problem persistent.
  - Trigger: HTTP errors with readable bodies, interrupted downloads followed by servers ignoring `Range`, or HTTP-client initialization failure.
  - Confidence basis: Success branches, append behavior, cache promotion, and the consuming plugin-loading caller are explicit.
  - Dimensions flagged by: Readability/Maintainability.

### Theme E: Input behavior drifts across comments, presets, and implementations — Impact: P2 — Confidence: High

Governing thought: Users encounter different input capabilities depending on which separately maintained path handles their action.

- [ ] **Finding E.1: Scrollback search silently rejects all non-ASCII text** (P2, High)
  - Where: `zellij-server/src/tab/mod.rs:7945`, `zellij-server/src/tab/mod.rs:7947`, `zellij-server/src/tab/mod.rs:7951`, `zellij-server/src/tab/mod.rs:7961`.
  - Problem: The comment promises "printable unicode" and describes only a wide-character limitation, but the predicate accepts printable ASCII bytes and selected control bytes. Any non-ASCII UTF-8 sequence rejects the entire input buffer, after which the function returns success.
  - Impact: Users cannot search accented letters, CJK text, or emoji. A mixed pasted search term is discarded in full, and the misleading comment points maintainers toward the wrong limitation.
  - Trigger: Type or paste a search buffer containing any non-ASCII character.
  - Confidence basis: The byte whitelist conclusively excludes every non-ASCII UTF-8 sequence.
  - Dimensions flagged by: Readability/Maintainability, Docs/Onboarding/UX.

- [ ] **Finding E.2: Configuration presets erase agent keyboard entry points** (P2, High)
  - Where: `default-plugins/configuration/src/presets.rs:242`, `default-plugins/configuration/src/presets_screen.rs:149`, `default-plugins/configuration/src/presets_screen.rs:213`, `zellij-utils/src/kdl/mod.rs:5179`, `zellij-utils/assets/config/default.kdl:176`, `zellij-utils/assets/config/default.kdl:208`, `zellij-utils/assets/config/default.kdl:227`.
  - Problem: Six separately maintained preset templates omit the canonical Agent-mode and Hocus bindings. Their `keybinds clear-defaults=true` replaces the base keymap, so those omissions remove working shortcuts rather than inheriting them.
  - Impact: Applying a preset, including during setup, removes keyboard entry points for agent creation, the agent list, and conversation panel. Saving the preset persists the omission across sessions.
  - Trigger: Apply a configuration preset; persistent impact occurs when the preset is saved.
  - Confidence basis: Template searches, UI application paths, and parser replacement semantics agree.
  - Dimensions flagged by: Duplication/Complexity, Docs/Onboarding/UX.

- [ ] **Finding E.3: Production mouse encoders disagree on wheel bytes** (P2, High)
  - Where: `zellij-server/src/panes/grid.rs:4442`, `zellij-server/src/panes/grid.rs:4501`, `zellij-server/src/panes/grid.rs:4698`, `zellij-server/src/panes/grid.rs:4719`, `zellij-server/src/tab/mouse_handler.rs:2200`, `zellij-server/src/tab/mouse_handler.rs:2268`.
  - Problem: The generic X10/UTF8 encoder uses wheel base values `68`/`69`, while specialized wheel encoders use `96`/`97`. Normal wheel handling uses the specialized path; Alt+wheel forwarding uses the generic path, so the difference exceeds the intended modifier bits.
  - Impact: Applications using these mouse modes receive inconsistent wheel reports depending on the gesture path. Maintaining both implementations makes protocol changes vulnerable to further drift.
  - Trigger: A pane using X10/UTF8 mouse reporting receives Alt+wheel. This specific discrepancy does not apply to SGR reporting.
  - Confidence basis: Both emitted-byte constructions and their production callers are visible; application-specific reactions were not exercised.
  - Dimensions flagged by: Duplication/Complexity.

## 3. Full Inventory (Grouped, Lower Impact Last)

Together with the eight findings above, this inventory contains **20 retained findings**. The table indexes all remaining findings; the checklists below preserve their mechanisms, triggers, and uncertainty.

| # | Title | Severity | Confidence | Locations | Dimensions | 1-line Why It Matters |
|---|---|---|---|---|---|---|
| F.2 | Resize success can mean rejected geometry | P2 | High | `zellij-server/src/panes/tiled_panes/pane_resizer.rs:45`; `zellij-server/src/panes/tiled_panes/pane_resizer.rs:143`; `zellij-server/src/panes/tiled_panes/pane_resizer.rs:216`; `zellij-server/src/panes/tiled_panes/mod.rs:1599` | Readability | Callers can update display bookkeeping after a layout was rejected without resizing. |
| F.3 | Focus dispatch discards the actual operation error | P2 | High | `zellij-server/src/screen/mod.rs:2734`; `zellij-server/src/screen/mod.rs:2745`; `zellij-utils/src/errors.rs:149` | Readability | Nested results hide tab-focus failures from the intended diagnostics. |
| F.4 | Repeated configuration blocks are silently overlooked | P2 | High | `zellij-utils/src/kdl/mod.rs:5500`; `zellij-utils/src/kdl/mod.rs:5502`; `zellij-utils/src/kdl/mod.rs:5510`; `zellij-utils/src/kdl/mod.rs:5528` | Readability | Composing settings can silently omit bindings or plugin configuration. |
| F.5 | Layout-manager mixes bytes, characters, and terminal columns | P2 | High | `default-plugins/layout-manager/src/ui/text_utils.rs:9`; `default-plugins/layout-manager/src/ui/text_utils.rs:62`; `default-plugins/layout-manager/src/ui/text_utils.rs:119`; `default-plugins/layout-manager/src/ui/layout_table.rs:15` | Docs/UX | International names violate column budgets in constrained panes. |
| F.6 | Layout-deletion success test expects no result | P2 | High | `zellij-server/src/plugins/unit/plugin_tests/layout_api.rs:1428`; `zellij-server/src/plugins/unit/plugin_tests/layout_api.rs:1525`; `zellij-server/src/plugins/unit/plugin_tests/snapshots/zellij_server__plugins__plugin_tests__layout_api__delete_layout_success_plugin_command.snap:5` | Testing | An absent deletion result satisfies a success-labelled test. |
| F.7 | Screen/plugin tests synchronize on delays and incidental renders | P2 | High | `zellij-server/src/unit/screen_tests/scroll_and_warp.rs:49`; `zellij-server/src/unit/screen_tests/scroll_and_warp.rs:75`; `zellij-server/src/plugins/unit/plugin_tests/mod.rs:131`; `zellij-server/src/plugins/unit/plugin_tests/loading.rs:482` | Testing | Observation depends on scheduling rather than the intended state. |
| F.8 | Handoff verification depends on separate manual runs | P2 | High | `xtask/src/ci.rs:270`; `xtask/src/ci.rs:277`; `xtask/src/test.rs:6`; `xtask/src/test.rs:66` | Testing | A green manual gate does not establish that whole-app integration coverage ran. |
| F.9 | Parser boundary test processes 110 MiB for a weak oracle | P2 | High | `zellij-client/src/stdin_ansi_parser/seq_scan.rs:16`; `zellij-client/src/unit/stdin_ansi_parser_tests.rs:353`; `.config/nextest.toml:19` | Testing | A large fixture and special timeout buy only a narrow overflow assertion. |
| F.10 | Pane initialization and suppression have multiple authorities | P2 | High | `zellij-server/src/tab/mod.rs:2995`; `zellij-server/src/tab/mod.rs:3128`; `zellij-server/src/tab/mod.rs:3238`; `zellij-server/src/tab/mod.rs:3305`; `zellij-server/src/tab/mod.rs:3462` | Duplication | Shared behavior changes require coordinated edits across placement paths. |
| F.11 | Four copied algorithms maintain directional hole filling | P2 | High | `zellij-server/src/panes/tiled_panes/tiled_pane_grid.rs:1705`; `zellij-server/src/panes/tiled_panes/tiled_pane_grid.rs:1788`; `zellij-server/src/panes/tiled_panes/tiled_pane_grid.rs:1874`; `zellij-server/src/panes/tiled_panes/tiled_pane_grid.rs:1960` | Duplication | Shared geometry edge-case changes are difficult to review consistently. |
| F.13 | Architecture guide teaches an incorrect style invariant | P2 | High | `docs/ARCHITECTURE.md:37`; `docs/ARCHITECTURE.md:41`; `zellij-server/src/panes/grid.rs:2644`; `zellij-server/src/panes/grid.rs:3092`; `zellij-server/src/panes/grid.rs:4947` | Docs/UX | Contributors receive an incorrect basis for rendering changes and tests. |
| F.14 | No direct layout-manager behavioral coverage located | P2 | Medium | `default-plugins/layout-manager/src/screens/layout_list/mod.rs:51`; `default-plugins/layout-manager/src/screens/layout_list/mod.rs:100`; `default-plugins/layout-manager/src/main.rs:135` | Testing | Located API tests do not establish the builtin's keyboard or optimistic-state behavior. |

### Runtime and UI contracts

- [ ] **F.2 — Resize success can mean rejected geometry.** Invalid layouts return `Ok(())` without applying spans, while valid but unchanged geometry returns `PaneSizeUnchanged`. Callers distinguish those outcomes and can update viewport/display dimensions after rejection. The trigger is a layout below a stack's minimum height or unchanged geometry after rounding; return semantics are proven, but the resulting screen appearance was not exercised.
- [ ] **F.3 — Focus dispatch discards the actual operation error.** Mapping the located tab produces an `Option<Result<()>>`; adding context creates an outer result, and `non_fatal()` checks only that outer layer. When the tab operation returns an error, its diagnostic is lost while screen focus dispatch returns success. Confidence is High for this result-handling mechanism; a separate conditional pane-loss concern remains in section 4.
- [ ] **F.4 — Repeated configuration blocks are silently overlooked.** One named-node lookup handles each top-level block, with an explicit TODO acknowledging repeated blocks and missing informative errors. A document containing two `keybinds` blocks, or another repeated supported block, can parse successfully without incorporating every occurrence. Confidence is High from the lookup paths and explicit comment; this differs from merging separate configurations with `base_config`.
- [ ] **F.5 — Layout-manager mixes width units.** Truncation interprets terminal-column budgets as character counts, wrapping uses byte lengths, and a separate ANSI helper uses terminal-cell widths. Wide names can exceed their assigned columns, while multibyte Latin text can wrap prematurely. Confidence is High for incorrect calculations; exact clipping depends on the renderer and terminal dimensions.

### Verification reliability

- [ ] **F.6 — Layout-deletion success test expects no result.** The observer exits after three generic `PluginBytes` events, then the test searches its captured output for `Delete layout`. Its checked-in snapshot is literally `None`, and it does not assert that the saved file disappeared. A missing deletion result can therefore satisfy the oracle; confidence is High from the assertion and snapshot, without implying current production deletion fails.
- [ ] **F.7 — Tests synchronize on delays and incidental renders.** Fixed sleeps infer readiness, while last-frame assertions and generic render counts infer completion. Delayed scheduling, watcher startup, or changes in render coalescing can change the observed result independently of the intended behavior. Confidence is High that these mechanisms remain; current flake frequency was not measured.
- [ ] **F.8 — Handoff verification depends on separate manual runs.** The repository deliberately has no hosted CI, and `cargo x ci check` excludes the whole-app integration package. A contributor relying solely on a green gate has no evidence from that command that integration tests ran. Confidence is High because the exclusion and manual policy are explicit; no particular unverified handoff was identified.
- [ ] **F.9 — Parser boundary test processes 110 MiB for a weak oracle.** Exercising the fixed 100 MiB production cap requires feeding 110 one-MiB chunks, with a dedicated five-minute timeout allowance. The assertion checks only `total_residue > 0`, not the comment's promised clean-state recovery or exact byte preservation. Confidence is High from the input loop, assertion, and timeout configuration; runtime was not measured in this audit.

### Structural maintenance

- [ ] **F.10 — Pane initialization and suppression have multiple authorities.** No-preference, tiled, and stacked creation paths repeat initialization and geometry/PTY parking sequences already represented by `build_pane` and `park_new_pane` for floating/overlay paths. A change to shared pane state or suppression behavior requires coordinated edits across those copies. Confidence is High for the duplication and maintenance burden; no current placement failure is asserted.
- [ ] **F.11 — Directional hole filling has four copied algorithms.** The routines repeat pane collection, sorting, nearest-pane selection, uncovered-hole handling, and geometry mutation, with axis differences embedded inside long sequences. Changes to common edge behavior have four implementation sites, making intentional directional differences difficult to distinguish from drift. Confidence is High for copied control flow and production callers; no incorrect resulting layout is asserted.

### Documentation and onboarding

- [ ] **F.13 — Architecture guide teaches an incorrect style invariant.** The guide says newline or movement to another line resets character styles, whereas the grid's newline path retains `cursor.pending_styles` and later character construction copies them. Contributors using the required guide as a behavioral reference start with a false rendering contract. Confidence is High from the worked documentation example and traced newline/print paths.

### Uncertain coverage

- [ ] **F.14 — No direct layout-manager behavioral coverage located.** Package-wide and repository test-reference searches found no direct tests for its selection, keyboard, or optimistic-state transitions. Existing fixture-plugin API tests do not establish those UI behaviors, leaving changes to delete, rename, open, and apply flows without an identified direct oracle. Confidence is Medium: this is search-supported absence, not measured coverage or proof that no indirect execution reaches the builtin.

Low-confidence findings: **none retained**. Standalone P3 hygiene findings: **none retained**.

## 4. Coverage & Blind Spots

- **Audited snapshot:** `/home/zacharyhamm/hocuspocus/zellij`, branch `main`, commit `6e84ff04a169b59e6486ac4734986afae10194a9`. The root was clean before and after the original inspection. The branch advanced before report creation; source references and conclusions remain tied to the audited revision.
- **Method:** Exactly four dimension subagents performed the audit. The harness permits three simultaneous subagents, so the first three ran concurrently and the fourth started when a slot opened. The principal independently checked evidence and merged results.
- **Dimensions audited:** Duplication/Complexity, Readability/Maintainability, Testing/Testability, and Docs/Onboarding/UX. **Skipped dimensions: none.**
- **Workspace breadth:** Manifest and source/test inventory searches covered all 30 workspace members: root CLI, client, server, utils, tile, tile-utils, test-support, integration tests, xtask, and builtin plugin packages. This was a whole-workspace search with targeted source tracing, not an exhaustive reading of every file.
- **Detailed examination:** Configuration parsing/serialization and presets; recording persistence; plugin downloading and permission/layout test harnesses; pane construction, focus, resizing, mouse reporting and search; selected client-parser tests; layout-manager UI; release/install workflows; contributor and architecture documentation.
- **Sampled rather than deeply examined:** Most individual builtin plugin screens, the large Firecracker provider, Hocus agent implementations, Lua authoring/runtime surfaces, remote PTY tests, and broad IPC conversion/test inventories.
- **Exclusions:** `target/` and `.worktrees/` were excluded. Root `vendor/`: **none**. Embedded third-party code under `zellij-utils/src/vendored/` was excluded. Generated protocol outputs under `zellij-utils/assets/prost/`, `prost_ipc/`, and `prost_nested_session/` were excluded from substantive findings. Third-party plugin implementations: **none identified**; first-party builtins remained in scope.
- **Runtime blind spots:** No test suite, build, benchmark, terminal session, network interaction, Docker/SSH test, or Firecracker/KVM test ran. Actual failure frequency, current flake frequency, performance, platform-specific behavior, contrast, screen-reader behavior, and exact narrow-terminal rendering remain unverified.
- **Confidence meaning:** High establishes the stated mechanism from source or a direct documentation contradiction. It does not imply an observed incident or measured likelihood. No P0 claim was justified by this static audit.
- **Unresolved conditional risk:** Suppressed-pane restoration removes ownership before replacement at `zellij-server/src/tab/mod.rs:8352`, and replacement failure can be discarded at `zellij-server/src/tab/mod.rs:8369`. The audit did not establish user-reachable preconditions for losing that pane state. Only the independently proven focus-error suppression appears in F.3.
- **Deduplication:** Preset findings from Duplication and Docs/UX became E.2; search findings from Readability and Docs/UX became E.1. Recorder error handling and missing failure injection became A.2. Layout UI errors, UI coverage, and the fixture deletion oracle remain distinct because they concern different mechanisms. F.6 shares render-observation machinery with F.7; their impacts were not counted as independent evidence of production deletion failure.
- **Ranking disagreements:** Docs/UX suggested P1 for missing agent shortcuts; Duplication suggested P2. Final judgment is **P2** because the demonstrated impact is loss of keyboard entry points after preset replacement, without evidence that the underlying functionality becomes unavailable. Testing suggested P1 for manual-only verification; final judgment is **P2** because this is an explicit policy boundary and the audit found no evidence that a particular handoff skipped required runs.
- **Other disagreement:** No factual disagreement remained after source verification. File length, TODO counts, and repetitive presentation code were not promoted without concrete impact.
- **Audit constraints:** Inspection used static reads and searches. The requested report is the only repository file written for this task. No fixes, patches to source code, PRs, remediation plans, or effort estimates were produced.
