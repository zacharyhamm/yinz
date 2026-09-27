# Upstream Sync Decisions

## 2026-08-11 — Realign raw-byte buffer with keyboard parser

- Upstream commits: `1d1d5c1101e608dea8cfc429af44ed455b3b06ee`
- Upstream range: `5254e4fc1dd784ef872644190dc5e2bcb0981bed..c428ae93b9f21fc79f89b4377b3f561ec66cda7d`
- Fork baseline: `main` at `33e881122`
- Decision: Skip
- Reason: The fork already fixes the same root cause in commit `0f34b3737` ("fix: preserve enter after bracketed paste"), inside `InputParser::parse_with_consumed` in the vendored termwiz parser. The fork's fix corrects the byte-accounting at its source; upstream's fix realigns a separate buffer externally. Adding upstream's patch on top would introduce a second, competing correction mechanism for an already-solved bug. Verified by running upstream's three regression tests against the fork's existing fix: all three passed unmodified.
- Reconsider only if: A future regression shows the fork's `unreported_consumed` accounting approach misses a case that upstream's `buffered_len` approach catches, or the user reopens this decision.

## 2026-08-28 — Opt-out for scroll-mode sync (`scroll_mode_sync`)

- Upstream commits: `a6dbdad8ae0f855c3045308c359dca03fdd1cdce`
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: The fork already has scroll-mode sync on focus (`zellij-screen/src/screen/input_modes.rs`, `sync_scroll_mode_on_focus`) with tests. The upstream change only adds a preference toggle, at the cost of ~60 LOC across 9 files, a new `Options` protobuf field, and ~12 snapshot updates. Nobody on the fork has asked to turn the sync off.
- Reconsider only if: A fork user wants scroll-mode sync disabled, or the user reopens this decision. If adapted, copy the `mouse_scroll_resize` plumbing site for site and take only the early-return hunk in `input_modes.rs`; the `None` mode_info fallback is redundant because `zellij-server/src/lib.rs` sends `ChangeMode` right after `AddClient`.

## 2026-08-28 — Start a session with an initial command (`zellij attach -c name -- cmd`)

- Upstream commits: `a9bd448e669b124d1f296ffb1d54d20c4f4d51e5`
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: `zellij -s name --layout-string 'layout { pane command="cmd" }'` already covers the use case, and `zellij action new-tab -- cmd` covers a running session. The port is ~200 LOC of contract plumbing (`ClientInfo::New`, `CliAssets`, `spawn_tabs`, `actions.rs`, protobuf) through the fork's most-diverged files plus 26 test-literal edits, for an ergonomic gain no hocus workflow needs.
- Reconsider only if: A hocus workflow needs `zellij attach -c <name> -- <cmd>` directly, or the user reopens this decision.

## 2026-08-28 — Notify floating panes when their tab is hidden

- Upstream commits: `83472766d3b7386393e7b49afcd4718686df438f`
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: Already present. Fork commit `06414ce12` ("Dismiss the ask hocus popup when focus leaves it") made `Tab::visible` walk tiled, floating, and visible overlay panes (`zellij-tab/src/tab/mod.rs`), which is a superset of the upstream change. The fork session manager also gates its timer on visibility.
- Reconsider only if: The user reopens this decision. The upstream regression test can be lifted on its own at any time.

## 2026-08-28 — IPC: distinguish disconnect from an undecodable frame (`IpcReceiveError`, `try_recv_*`)

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-change: `zellij-utils/src/ipc.rs` `IpcReceiveError`, `zellij-server/src/route.rs`, `zellij-client/src/lib.rs` router loop)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: Already fixed differently. Fork commit `0e762f99c` added `IpcRecv::{Msg, Eof, Malformed, Idle}` with `classify_read_error`, `MAX_IPC_FRAME_LEN`, and a malformed-frame backoff on both the server (`route.rs`) and client (`router.rs`) sides. This is a superset of the upstream distinction. Equivalent socket tests exist.
- Reconsider only if: The user reopens this decision.

## 2026-08-28 — Per-tab sizing fixes (recompute vacated tab, clear display for a larger arriving client)

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-change: `Screen::new_tab` vacated tabs, `move_clients_between_tabs` `clients_are_larger_than_tab`, `zellij-integration-tests/tests/clients.rs`), `60f7aa7c512905eea6c65507d91ee90a3ee310a3` (sub-change: `move_clients_from_closed_tab` `clients_are_larger_than_tab`)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: Not applicable. The fork sizes every tab to one session-wide surface (`zellij-screen/src/screen/tab_layout.rs`, `tab_surface_size` ignores the tab id and returns `session_size()`), so a vacated tab cannot change size and a client cannot be larger than the tab it lands in. Upstream's per-tab sizing model is a deliberate fork divergence. The upstream `clients.rs` tests would fail by design.
- Reconsider only if: The fork adopts per-tab sizing, or the user reopens this decision.

## 2026-08-28 — Warning cleanups: `from_i32` migration, `Path` import cfg, doubled `#[test]`, dead `stacked_pane_list` field, `base64` API

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-changes in `zellij-utils/src/plugin_api/*.rs`, `ipc/enum_conversions.rs`, `input/config.rs`, `kdl/mod.rs`, `zellij-server/src/panes/tiled_panes/mod.rs`, `zellij-client/src/lib.rs`, `src/commands.rs`, kitty `parser_tests.rs`)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: Already done or not applicable. Fork commit `5c8720f08` migrated `from_i32` to `try_from(..).ok()`; `TiledPanes` has no `stacked_pane_list` field; `zellij-client/src/lib.rs` already gates the async runtime; the kitty test already uses `BASE64_ENCODER`. The `Path` import in `input/config.rs` is used by ungated fork code (`write_config_to_disk`), so the cfg gate would break the wasm build. The kdl tests moved to `kdl/unit/` and have no doubled attribute.
- Reconsider only if: The user reopens this decision.

## 2026-08-28 — Alt-scroll `MouseEvent` test constructors

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-change: `zellij-utils/src/input/mouse.rs` `new_alt_scroll_up_event` / `new_alt_scroll_down_event`)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip as a standalone change
- Reason: Test-only helpers with no callers outside the alt+mouse tests. They are added as part of the alt+mouse adaptations (B2/B3 in the 2026-08-28 review) when those tests are ported, not on their own.
- Reconsider only if: The alt+mouse tests need them; that is not a reopen.

## 2026-08-28 — Bound the OSC 99 notification assembly buffer

- Upstream commits: `60f7aa7c512905eea6c65507d91ee90a3ee310a3` (sub-change: `MAX_NOTIFICATION_ASSEMBLY_BYTES`, `append_bounded` in `zellij-server/src/panes/grid.rs`)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip as a standalone change
- Reason: The buffer it bounds (`NotificationTracker`) does not exist in the fork until the host-notifications adaptation (B1 in the 2026-08-28 review) lands. It is folded into that adaptation.
- Reconsider only if: B1 lands without it; that is not a reopen.

## 2026-08-28 — `.gitattributes` forcing LF for bundled web client assets

- Upstream commits: `faebf2e1e39f637a4d0d992269830c5f5e1c0103`
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: It protects the web asset integrity hashes on Windows checkouts with `autocrlf`. The fork is developed on Linux only and has no CI.
- Reconsider only if: Someone develops the fork on Windows, or the user reopens this decision.

## 2026-08-28 — Development version bump to 0.46.0

- Upstream commits: `bf8d23a4f774abf27a108da2a1a2689e7d8d0d23`
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: The fork versions independently (`0.45.0` in `Cargo.toml`). The version string affects socket-directory naming and session compatibility, so it is not changed as a side effect of a sync.
- Reconsider only if: The fork chooses to track upstream version numbers, or the user reopens this decision.

## 2026-08-28 — Upstream CHANGELOG updates

- Upstream commits: `e839bfffa586992364309a685b2c71f3b23c247e`, and the `CHANGELOG.md` hunks of every commit in the range
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip
- Reason: The fork keeps its own `CHANGELOG.md`. Adapted changes are described in their fork commits.
- Reconsider only if: The user reopens this decision.

## 2026-08-28 — `mouse_hover_tips` option (deferred)

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-change: `Options::mouse_hover_tips`, `Tab::resolve_hint_text`, `PaneFrame` help rows, `title_frame_hints` tests)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip (deferred)
- Reason: A small preference toggle that costs ~140 LOC, a new `Options` protobuf field (upstream's 59 collides with the fork's `notification_position`; the fork must use 66 or higher), and every `Tab::new` / `Screen::new` test constructor. Low priority relative to the rest of the 2026-08-28 batch.
- Reconsider only if: A later sync touches the `Options` protobuf anyway, a user asks for hover tips off while keeping hover effects, or the user reopens this decision.

## 2026-08-28 — Upstream regression-test suites for existing behaviour (deferred)

- Upstream commits: `070b9c3d1cee6b9266e0b416cfb65ef0cb0940d2` (sub-changes: `grid_tests.rs` width tests; `zellij-integration-tests/tests/{rendering_width,osc133_selection,scroll,paste_read,theme_actions,input_fragmentation}.rs`; harness `cell_foreground`, `detach()`, `main_client()`; `base64` dev-dependency)
- Upstream range: `98a0837077492d53dd252ab30bc3e43e41e504f4..e839bfffa586992364309a685b2c71f3b23c247e`
- Fork baseline: `main` at `005accbb2`
- Decision: Skip (deferred)
- Reason: Tests only, ~1,400 LOC to port into the fork's split test modules and diverged integration harness, with 9 snapshots to regenerate. No production change. Deferred behind the production adaptations of the same review.
- Reconsider only if: The user wants the extra coverage, or the user reopens this decision.

## 2026-08-29 — Builtin plugin wasm is no longer checked in (standing note)

- Decision: `zellij-utils/assets/plugins/*.wasm` is gitignored; the wasm lives under `target/wasm32-wasip1/` and is installed by `cargo x install`.
- Consequence for syncs: an upstream commit that touches `assets/plugins/*.wasm` now merges as a modify/delete conflict. Resolve with `git rm zellij-utils/assets/plugins/<name>.wasm`; the plugin sources are what get adapted.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Session-probe receive timeout

- Upstream commits: `fa131c8b4a99bf2370a30bed112e168eec90df34`
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: The fork already sets 500 ms receive and send timeouts in zellij-session/src/sessions.rs. It also bounds connection attempts, probes sessions concurrently, and distinguishes an unresponsive server from an absent session. Existing real-socket tests cover this behavior. The upstream five-second receive timeout adds no protection and would increase the wait.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Spelling corrections in retained text

- Upstream commits: `af38660c5884f50bb3726682fb92961326c4268f`, `a162323384dff3fbd9c68a4c0971f4f6efca424a` (spelling corrections only)
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: The corrections affect comments, API documentation, the plugin issue template, and two diagnostic messages across numerous files. They provide little benefit for this sync. The layout-manager README target correction from a162323384dff3fbd9c68a4c0971f4f6efca424a is approved separately.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Corrections and compiler-guide removal in deleted documentation

- Upstream commits: `a162323384dff3fbd9c68a4c0971f4f6efca424a` (CONTRIBUTING.md, docs/RELEASE.md, and docs/TERMINOLOGY.md portions)
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: These three files are already absent from the fork. The obsolete compiler guide is therefore already removed. There is no remaining release or terminology text to correct. The deleted files will not be restored.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Upstream sponsor credit

- Upstream commits: `232c93721dee4ab7419db66f5f74a98c2e87236f`
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: The credit names an upstream sponsor. The fork replaced the upstream README with its own identity. This change has no functional benefit for the fork.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Upstream policy for generated issue and PR descriptions

- Upstream commits: `8037b970ec90bd199195c59afba2a8c5ce5f5ffe`
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: This is an upstream contribution policy. The fork has no CONTRIBUTING.md. Adopting the policy would require a separate decision.
- Reconsider only if: The user reopens this decision.

## 2026-09-22 — Upstream changelog edits

- Upstream commits: `a162323384dff3fbd9c68a4c0971f4f6efca424a`, `474ea0cef620c83d6ec05a7c28c7f80c4f63bbbb` (CHANGELOG.md portions only)
- Upstream range: `e839bfffa586992364309a685b2c71f3b23c247e..fa131c8b4a99bf2370a30bed112e168eec90df34`
- Fork baseline: `main` at `f2c1d9812c2053257c98fd86879bbf564ff6379f`
- Decision: Skip
- Reason: Upstream changelog edits remain excluded under the prior decision. The fork has since removed CHANGELOG.md. The OSC 7 behavior from 474ea0cef620c83d6ec05a7c28c7f80c4f63bbbb is approved separately.
- Reconsider only if: The user reopens this decision.
