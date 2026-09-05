# Maintainability audit findings checklist

Audited project: `/home/zacharyhamm/hocuspocus/zellij` — all 30 workspace members.

Audited revision: `6e84ff04a169b59e6486ac4734986afae10194a9` (`main`). This document records the completed static audit; line references describe that revision. The branch advanced before this report was written. Unless an entry has a revalidation note below, it has not been revalidated against the newer HEAD.

Checkboxes track findings for review. They do not represent approved fixes or a remediation plan. All findings were unchecked when this report was created.

The A-group findings were revalidated against `4f3245ef4` and remediated on 2026-09-04. The B and C findings were revalidated against `e8376b353` and remediated the same day. The E findings were revalidated against `dd11e2b16` and remediated the same day. The D finding was revalidated against `5170e4628` and remediated the same day. Their original evidence below describes the audited code; the remediation notes describe the fixes and regression coverage.

A-group validation: all 20 focused configuration/recorder tests passed. `cargo x ci check` passed format, lint, all 6,798 selected workspace tests (45 skipped), and doctests. `cargo x integration-test -j 16` passed all 344 whole-app tests with no skips.

B/C-group validation: the five permission-denial tests pass 10/10 in a loop and all fail with `check_command_permission` forced to grant; the seven new layout-manager tests pass. `cargo x ci check` passed format, lint, all 6,841 workspace tests (45 skipped), and doctests. `cargo x integration-test -j 16` passed all 344 whole-app tests with no skips.

Follow-up review of the A.1, E.1 and E.2 remediations (search reflow by grid width, save-guard comparison of referenced layouts and pane colours, relative references kept verbatim, plain `e` in the no-primary presets) was remediated on 2026-09-04; validation: `NEXTEST_TEST_THREADS=16 cargo x ci check` passed format, lint, all 6,963 workspace tests (45 skipped), and doctests.

E-group validation: `NEXTEST_TEST_THREADS=16 cargo x ci check` passed format, lint, all 6,845 workspace tests (45 skipped), and doctests. An earlier gate also passed at default concurrency; a subsequent default-concurrency run produced empty-observer failures in `dump_session_layout_plugin_command`, `shared_plugin_has_one_instance_serving_all_clients`, `plugin_workers`, and `plugin_workers_persist_state`. All four passed in isolation and in the full gate with 16 test workers. The preset integration snapshots were reviewed and updated to include the restored Agent entry. `cargo x integration-test -j 16` then passed all 344 whole-app tests with no skips.

D-group validation: the 16 focused downloader tests pass 10/10 in a loop, and the 7 `wasm_bridge` tests pass. `NEXTEST_TEST_THREADS=16 cargo x ci check` passed format, lint, all 6,954 workspace tests (45 skipped), and doctests. `cargo x integration-test -j 16` passed all 344 whole-app tests with no skips.

The "Runtime and UI contracts" findings F.2–F.5 were revalidated against `93fa6bbb0` and remediated on 2026-09-04. F-group validation: the six duplicate-block tests, the fifteen layout-manager native tests (nine new), and the resizer, stacked-pane, tab-resize and overlay-focus tests (301, three new) pass. `NEXTEST_TEST_THREADS=16 cargo x ci check` passed format, lint, all 7,064 workspace tests (45 skipped), and doctests. `cargo x integration-test -j 16` passed 342 of 344 whole-app tests; the two failures, `hocus_plugin_authoring::hocus_writes_a_lua_plugin_and_loads_it` (a `.bak` file listed by `/plugin_list`) and `hocus_agent_picker::the_agent_list_and_conversation_are_one_panel` (the panel not put away on Esc), fail the same way alone and on the untouched `93fa6bbb0` sources, so they predate this group and are not covered by it.

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
  - Remediation: Preserve explicit layout references in `Action::NewTab`, KDL, IPC, and plugin action messages. Whole-config saves compare intended `NewTab` bindings with reparsed serialization before making a backup or writing. A binding with a layout reference is compared by what the save writes (the reference, name, cwd, and focus), so the referenced file may change on disk between session start and the save; a binding on the implicit default layout is compared by content, which is the only way to catch a plugin-built inline layout that `NewTab {}` would silently drop, so editing the default layout still needs the config file re-read (save or touch it) before a save, and the refusal says so. The comparison ignores what the plugin wire cannot carry: an unspecified border, empty swap lists, and pane colours. A relative layout reference is kept as written and only resolved for loading, so a save writes the user's reference back rather than an absolute path.
  - Regression coverage: Named, absolute, and relative layout references; tiled commands, floating panes, both swap-layout kinds; implicit defaults; focus overrides; unrelated key rebinding and disk persistence; rejected writes leaving the original file and backups untouched; optional protocol fields, empty swap lists, and pane colours after IPC; a referenced layout edited on disk and then saved; a plugin-built inline layout refused; a relative reference serialized verbatim.

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

- [x] **Finding B.1: Permission-denial tests cannot observe post-denial violations** (P1, High)
  - Where: `zellij-server/src/plugins/unit/plugin_tests/mod.rs:191`, `zellij-server/src/plugins/unit/plugin_tests/mod.rs:201`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:878`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:925`, `zellij-server/src/plugins/unit/plugin_tests/plugin_lifecycle_commands.rs:947`, `zellij-server/src/plugins/unit/plugin_tests/config_events.rs:16`.
  - Problem: The denial helper sends `PermissionStatus::Denied` and immediately breaks its logging loop, dropping the screen receiver. Tests subsequently send protected commands and assert their absence from the already finished log.
  - Impact: A regression that permits a command after denial can escape these assertions. Waiting longer afterward cannot repair the missing observation; this affects negative coverage for clipboard and other protected actions.
  - Trigger: A denied plugin emits a protected command after the permission handshake.
  - Confidence basis: Observer termination and subsequent command/assertion ordering are directly visible. This finding does **not** establish a production permission bypass.
  - Dimensions flagged by: Testing/Testability.
  - Remediation: The denial helper is a function that answers `Denied` and then logs until its channel disconnects, which happens only after `teardown()` has joined the plugin thread and every executor worker and spawned thread has dropped its sender; a matching logger covers the server channel. Tests wait on content instead of sleeping: first for the logged permission request (the observer logs it after answering, and the answer and the key share the plugin thread's FIFO inbox), then for the frame in which the fixture prints the key, or for the command's outcome or a crash report where the fixture prints an outcome instead. Teardown precedes the observer join. Two oracles were wrong as well as blind: the mode-switch test asserted on the screen channel although a routed mode change is a `ServerInstruction`, and the random-name test searched for `Generated name:` although the fixture prints it without the colon. The fixture does not subscribe to `PermissionRequestResult`, so the denial itself cannot be read off a frame.
  - Regression coverage: With `check_command_permission` temporarily forced to grant everything, all five tests fail on their assertions rather than on a timeout; restored, they pass 10/10 in a loop at about 0.2 s each instead of 1.5 s of sleeps. The frame helpers moved from the batching tests into the shared harness, and the batching tests still pass.

### Theme C: Layout deletion reports success before the host confirms it — Impact: P2 — Confidence: High

Governing thought: The layout manager removes an entry from its visible list after discarding the host deletion result.

- [x] **Finding C.1: Failed layout deletion appears successful** (P2, High)
  - Where: `default-plugins/layout-manager/src/screens/layout_list/mod.rs:65`, `default-plugins/layout-manager/src/main.rs:176`, `zellij-server/src/plugins/zellij_exports.rs:6358`.
  - Problem: `let _ = delete_layout(...)` discards the synchronous host result before returning `OptimisticUpdate::Delete`. The visible list then removes the entry with `retain`.
  - Impact: A read-only filesystem, directory-permission error, or disappeared file produces a false removal signal.
  - Trigger: A layout deletion fails after the user presses Delete.
  - Confidence basis: Host errors, the discarded result, and list mutation are directly connected.
  - Dimensions flagged by: Docs/Onboarding/UX; Testing/Testability.
  - Remediation: The Delete key goes through `delete_selected_layout`, which takes the host call as a parameter, emits the optimistic removal only on `Ok`, and on `Err` returns an error screen carrying the host's message that leads back to the list with its selection, the same shape rename and import already use. Nothing is asked of the host when nothing deletable is selected.
  - Regression coverage: Native unit tests in the plugin crate for a confirmed delete, a refused delete (list untouched, error shown, return screen keeps the selection), no selection, a builtin, and an unparsable layout deleted by file name, plus state-level tests that a refused delete leaves `display_layouts` unchanged. They cover the delete flow only; F.14 stays open.

### Theme D: Remote plugin loading accepts invalid download outcomes — Impact: P1 — Confidence: High

Governing thought: The downloader's success result does not establish that its destination contains the requested artifact.

- [x] **Finding D.1: Download success can mean missing, invalid, or incorrectly resumed content** (P1, High)
  - Where: `zellij-utils/src/downloader.rs:44`, `zellij-utils/src/downloader.rs:73`, `zellij-utils/src/downloader.rs:90`, `zellij-utils/src/downloader.rs:96`, `zellij-utils/src/downloader.rs:121`, `zellij-utils/src/downloader.rs:138`, `zellij-server/src/plugins/wasm_bridge.rs:450`.
  - Problem: HTTP-client construction errors become `None`, and an absent client produces `Ok(())`. Download responses are streamed without checking HTTP status or validating the requested range; an existing partial file is opened for append, then promoted to the final cache path.
  - Impact: An HTTP error body or full response appended to a partial download can become the cached plugin. Later loads accept the existing path and fail during artifact loading, obscuring the original network failure and making the problem persistent.
  - Trigger: HTTP errors with readable bodies, interrupted downloads followed by servers ignoring `Range`, or HTTP-client initialization failure.
  - Confidence basis: Success branches, append behavior, cache promotion, and the consuming plugin-loading caller are explicit.
  - Dimensions flagged by: Readability/Maintainability.
  - Remediation: The HTTP client's build result is stored, and a failed build is an error whenever a request is needed; a file already in the cache is still returned without one. `download` measures the partial file, sends `Range` only when it is non-empty, and checks the status and encoding before opening anything: `200` or `203` replaces the partial file; `206` appends only when `Content-Range` starts at the partial file's length, otherwise the partial file is discarded; `416` on a resume means the server has nothing past the bytes on disk, so the partial file is discarded and the whole artifact fetched again in the same call; any other status is an error naming the URL and status that leaves both the partial file and the cache untouched. The request asks for an identity encoding with automatic decompression off, so `Content-Length` and `Content-Range` describe the stored bytes, and a response carrying any other `Content-Encoding` is rejected before anything is written rather than cached as the artifact. A length mismatch after the stream fails the download (a short body keeps the prefix for resume, a long one discards it). Writes are flushed before the partial file is measured, renamed, or left behind. `download` returns the cached path, and the plugin loader wraps a failure with the URL so the pane shows the cause chain. `download_without_cache`, which serves layouts loaded from URLs, applies the same status check.
  - Regression coverage: A local raw-TCP server in a temporary directory drives a full response, an error status with an existing partial file, a matching resume with the `Range` header asserted, a `200` and a `203` replacing a partial file, a `416` on a resume that discards the partial file and refetches, a `416` and a `204` rejected outright, a gzip-encoded body rejected before anything is written with the identity request asserted, wrong-offset, missing, and unsolicited `206` responses, an interrupted transfer that keeps the prefix and resumes on a second connection, a chunked body longer than its range and a shorter honoured sub-range, a cache hit without a client, a failed client build, the `Content-Range` parser, and `download_without_cache` on `500` and `200`.

### Theme E: Input behavior drifts across comments, presets, and implementations — Impact: P2 — Confidence: High

Governing thought: Users encounter different input capabilities depending on which separately maintained path handles their action.

- [x] **Finding E.1: Scrollback search silently rejects all non-ASCII text** (P2, High)
  - Where: `zellij-server/src/tab/mod.rs:7945`, `zellij-server/src/tab/mod.rs:7947`, `zellij-server/src/tab/mod.rs:7951`, `zellij-server/src/tab/mod.rs:7961`.
  - Problem: The comment promises "printable unicode" and describes only a wide-character limitation, but the predicate accepts printable ASCII bytes and selected control bytes. Any non-ASCII UTF-8 sequence rejects the entire input buffer, after which the function returns success.
  - Impact: Users cannot search accented letters, CJK text, or emoji. A mixed pasted search term is discarded in full, and the misleading comment points maintainers toward the wrong limitation.
  - Trigger: Type or paste a search buffer containing any non-ASCII character.
  - Confidence basis: The byte whitelist conclusively excludes every non-ASCII UTF-8 sequence.
  - Dimensions flagged by: Readability/Maintainability, Docs/Onboarding/UX.
  - Remediation: Search input validates decoded UTF-8 characters instead of ASCII bytes, retaining reset/backspace/delete and rejecting other control characters. Invalid UTF-8 is ignored like other non-text input; the update cannot fail, so the screen thread's `UpdateSearch` dispatch cannot become fatal. The matcher indexes a character vector and converts matched character indexes to terminal columns before producing selections, wrapping a position by the grid width on the row it sits on (as `Row::split_to_rows_of_length` does) rather than by the main row's width, which a wide character that did not fit leaves short; debug highlights preserve wide-character cell widths. Matching remains codepoint-based, with the existing ASCII case-folding/word-boundary rules and renderer limitations for combining marks and multi-codepoint graphemes.
  - Regression coverage: Accented text, CJK, emoji, mixed Unicode input, wide characters before and inside matches, wrapped matches, a match ending on the last cell of a full row below a wide-character wrap, whole-word retry, exact selection columns, debug highlights, Unicode deletion/reset, rejected controls, and invalid UTF-8 through the tab input boundary.

- [x] **Finding E.2: Configuration presets erase agent keyboard entry points** (P2, High)
  - Where: `default-plugins/configuration/src/presets.rs:242`, `default-plugins/configuration/src/presets_screen.rs:149`, `default-plugins/configuration/src/presets_screen.rs:213`, `zellij-utils/src/kdl/mod.rs:5179`, `zellij-utils/assets/config/default.kdl:176`, `zellij-utils/assets/config/default.kdl:208`, `zellij-utils/assets/config/default.kdl:227`.
  - Problem: Six separately maintained preset templates omit the canonical Agent-mode and Hocus bindings. Their `keybinds clear-defaults=true` replaces the base keymap, so those omissions remove working shortcuts rather than inheriting them.
  - Impact: Applying a preset, including during setup, removes keyboard entry points for agent creation, the agent list, and conversation panel. Saving the preset persists the omission across sessions.
  - Trigger: Apply a configuration preset; persistent impact occurs when the preset is saved.
  - Confidence basis: Template searches, UI application paths, and parser replacement semantics agree.
  - Dimensions flagged by: Duplication/Complexity, Docs/Onboarding/UX.
  - Remediation: All six preset templates insert one shared Agent keybinding fragment with the canonical creation, duplication, list, and conversation actions and their plugin placement flags. Mode entry follows the selected primary modifier plus `e`. A preset without a primary modifier keeps Normal free of modifier bindings, so it binds plain `e` outside the typing and text-entry modes the same way unlock-first does: unlock-first binds plain `e` after unlocking and returns to Locked after agent actions or Enter/Esc, the no-primary presets bind plain `e` in every mode except Normal, Locked and the text-entry modes and return to Normal, and both preserve Pane mode's embed action and Scroll mode's scrollback editor; the other presets return to Normal.
  - Regression coverage: Parsed effective keymaps for all six templates, alternate and combined modifiers, the no-primary plain-`e` entry outside Normal, locked/text-entry exclusions, exit modes, canonical plugin action equality, preservation through configuration serialization/reparse, and whole-app status-bar snapshots showing Agent entry after switching presets.

- [x] **Finding E.3: Production mouse encoders disagree on wheel bytes** (P2, High)
  - Where: `zellij-server/src/panes/grid.rs:4442`, `zellij-server/src/panes/grid.rs:4501`, `zellij-server/src/panes/grid.rs:4698`, `zellij-server/src/panes/grid.rs:4719`, `zellij-server/src/tab/mouse_handler.rs:2200`, `zellij-server/src/tab/mouse_handler.rs:2268`.
  - Problem: The generic X10/UTF8 encoder uses wheel base values `68`/`69`, while specialized wheel encoders use `96`/`97`. Normal wheel handling uses the specialized path; Alt+wheel forwarding uses the generic path, so the difference exceeds the intended modifier bits.
  - Impact: Applications using these mouse modes receive inconsistent wheel reports depending on the gesture path. Maintaining both implementations makes protocol changes vulnerable to further drift.
  - Trigger: A pane using X10/UTF8 mouse reporting receives Alt+wheel. This specific discrepancy does not apply to SGR reporting.
  - Confidence basis: Both emitted-byte constructions and their production callers are visible; application-specific reactions were not exercised.
  - Dimensions flagged by: Duplication/Complexity.
  - Remediation: The generic X10/UTF8 encoder uses wheel base values `96`/`97`. Specialized up/down methods now delegate to that encoder using the existing wheel-event constructors, so both production paths share encoding and tracking behavior.
  - Regression coverage: Exact up/down bytes for X10, UTF8, and SGR across all modifier combinations and enabled tracking modes; disabled tracking; and Alt+wheel forwarding through the tab in all three encodings.

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

- [x] **F.2 — Resize success can mean rejected geometry.** Invalid layouts
  return `Ok(())` without applying spans, while valid but unchanged geometry
  returns `PaneSizeUnchanged`. Callers distinguish those outcomes and can
  update viewport/display dimensions after rejection. The trigger is a layout
  below a stack's minimum height or unchanged geometry after rounding; return
  semantics are proven, but the resulting screen appearance was not exercised.
  Remediation: a refused layout returns the new `ZellijError::PaneLayoutRejected`
  from `PaneResizer::layout`, so no caller can mistake untouched panes for an
  applied layout; `PaneSizeUnchanged` keeps meaning a valid layout that rounded
  to the same geometry. `TiledPanes::resize` already treats any other error as
  "not laid out" and tries the other axis first, so a rejection now takes that
  path and is logged. Its screen-size bookkeeping moved out of the success arms:
  the viewport and display area now follow the screen whether or not the panes
  could, because updating the display area unconditionally while moving the
  viewport only on success left the viewport one resize behind after a refusal
  and applied the wrong delta on the next one. The interactive resize path
  (`change_pane_size` → `Tab::resize` / `Tab::resize_pane_with_id`) logs the
  new error non-fatally, as it does every non-fixed-pane error. The best-effort
  relayouts after a pane closes still discard their result on purpose.
  Regression coverage: the resizer fixture that locked in the silent `Ok` now
  asserts the rejection and that nothing moved; the property test treats a
  rejection like any other failure and checks no pane moved; tab-level tests
  resize a three-pane stack into two rows and assert every geometry is unchanged
  while the viewport tracks the display area, then resize into ten rows and
  assert the stack fills them and the viewport is exactly ten.
- [x] **F.3 — Focus dispatch discards the actual operation error.** Mapping the
  located tab produces an `Option<Result<()>>`; adding context creates an outer
  result, and `non_fatal()` checks only that outer layer. When the tab
  operation returns an error, its diagnostic is lost while screen focus
  dispatch returns success. Confidence is High for this result-handling
  mechanism; a separate conditional pane-loss concern remains in section 4.
  Remediation: the lookup is a `match`; the tab's own result gets the context
  and `non_fatal()`, and a tab that vanished between `go_to_tab` and the lookup
  is logged. The context names the pane instead of the unrelated
  `focus_plugin_pane`. The function still returns `Ok` after logging: two screen
  handlers `?` its result into the `.fatal()` screen thread, so propagating the
  tab's error would turn a focus failure into a server exit. `rg -U` finds no
  other `Option<Result>` reaching `non_fatal` in the server.
  Regression coverage: the existing screen and overlay focus tests. There is no
  injected-error test: `Tab::focus_pane_with_id` has no failing branch a test can
  reach without a fault seam (an unknown pane ends in `Ok(())`), and the fix is
  a shape change that the type system now enforces.
- [x] **F.4 — Repeated configuration blocks are silently overlooked.** One
  named-node lookup handles each top-level block, with an explicit TODO
  acknowledging repeated blocks and missing informative errors. A document
  containing two `keybinds` blocks, or another repeated supported block, can
  parse successfully without incorporating every occurrence. Confidence is High
  from the lookup paths and explicit comment; this differs from merging
  separate configurations with `base_config`.
  Remediation: `Config::from_kdl` reads `keybinds`, `themes`, `plugins`,
  `load_plugins`, `context_menu`, `hooks`, `ui` and `env` through one helper that
  returns a `KdlError` naming the block, with its span on the second occurrence,
  when a document repeats it; `Themes::from_string` (theme-directory files) uses
  the same helper. Merging across files is untouched, since each `config.d`
  fragment and each layout's embedded config is its own `from_kdl` call over the
  base. The check is per block rather than "no repeated top-level node": swap
  layout files legitimately repeat `swap_tiled_layout` at top level. Scalar
  options read by `Options::from_kdl` keep first-wins semantics; they are a
  different lookup family and were not part of this finding.
  Regression coverage: each of the eight blocks repeated is an error naming it
  and pointing at the second block; one of each block still parses; the shipped
  default config parses; a theme file with two `themes` blocks is rejected; the
  same block in a base config and a fragment still merges both bindings.
- [x] **F.5 — Layout-manager mixes width units.** Truncation interprets
  terminal-column budgets as character counts, wrapping uses byte lengths, and
  a separate ANSI helper uses terminal-cell widths. Wide names can exceed their
  assigned columns, while multibyte Latin text can wrap prematurely. Confidence
  is High for incorrect calculations; exact clipping depends on the renderer
  and terminal dimensions.
  Remediation: `truncate_with_ellipsis`, `truncate_with_ellipsis_start` and
  `wrap_text_to_width` budget by `unicode-width` columns through one shared
  prefix/suffix helper, keeping the plugin's `...` glyph so ASCII output is
  byte-for-byte what it was. The error screen's wrap decision and centring, and
  the import screen's save-as line (its fit check, its budget, and the cursor
  column, which is now the width of the characters before the cursor rather
  than their count) use the same unit.
  Regression coverage: ASCII results unchanged at every budget edge; CJK, emoji
  and a decomposed accent truncated from both ends within their column budget;
  multibyte Latin and CJK wrapping by column where byte counting broke early;
  wrap edge cases; and two property tests over mixed narrow, wide, zero-width
  and multibyte text: truncation never exceeds its budget, text that fits is
  returned verbatim, a cut with room carries the ellipsis, and wrapped lines fit
  and keep every word in order.

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
