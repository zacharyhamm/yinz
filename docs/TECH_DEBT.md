# Tech debt — the two large refactors, and the bugs found looking for them

A survey of this fork found three dominant pieces of debt. The third — nothing verified anything —
is **done**, and is described in CLAUDE.md rather than here. This document is the research behind
the other two, written down so picking them up later does not mean deriving it again.

**The file split in section A is done**, and so are both riders it deliberately left alone (see
"What was actually done", below, and "What the follow-up pass did" at the end). Of B, only the
cheap round-trip coverage guard is done; B1 proper, B2 and B3 are untouched.

What follows is what was measured, what the seam actually is, and — as important — what was
considered and rejected.

---

# A. `screen.rs` was a 17,287-line god file — **split, done**

## The shape

| Item | Lines | Size |
|---|---|---|
| `pub enum ScreenInstruction` | 403–1121 | 719 lines, **253 variants** |
| `impl From<&ScreenInstruction> for ScreenContext` | 1123–1528 | 406 lines, 253 near-mechanical arms |
| `pub(crate) struct Screen` | 1760–1936 | **109 fields** |
| `impl Screen` | 2028–11311 | **9,284 lines, 266 methods** |
| `screen_thread_main` | 11485–17283 | **5,799 lines, one match with 253 arms** |

It is also the most-edited file in the repo — 128 commits in twelve months, ahead of
`tab/mod.rs` (102) and `data.rs` (80). Size and churn coincide, so the size is a recurring tax
rather than dormant weight.

There is no `zellij-server/src/screen/` directory at all. This is the anomaly: `tab/` already
extracted `mouse_handler.rs` (2,208), `layout_applier.rs` (1,445), `swap_layouts.rs`,
`clipboard.rs` and `copy_command.rs`; `panes/` is split further still, with
`floating_panes/`, `tiled_panes/` and `overlay_panes/` as separate directories. The precedent
exists and simply was not continued.

## Why it is a text move, not a redesign

Rust's private-field visibility is "the defining module **and all its descendants**", and inherent
impls may be split across modules within one crate. So `screen.rs` becomes `screen/mod.rs`, and a
child module `screen::mouse` may carry `impl Screen { … }` and touch all 109 private fields **with
no visibility changes at all**. Each child begins `use super::*;`.

The glob import is the pragmatic choice: `screen.rs`'s import block is enormous, and doing
per-file import surgery would turn a mechanical move into a judgement-laden one.

## Order — and why this order

The single best asset in this refactor is that **the tests are already partitioned**:
`zellij-server/src/unit/screen_tests/` has 29 files whose names map almost one-to-one onto the
topics inside `screen.rs`. Extract the runs that have a dedicated test file first, so every step
has a targeted verification command.

Step 0 is `git mv screen.rs screen/mod.rs` on its own, fixing the `#[path = "./unit/screen_tests/mod.rs"]`
at the bottom to `"../unit/..."`. That commit must show as a pure rename in `git log --stat`.

| # | New file | Source lines | Paired test |
|---|---|---|---|
| 1 | `screen/mouse.rs` | 10254–10520 | `screen_tests::mouse` |
| 2 | `screen/subscriptions.rs` | 11082–11310 | `screen_tests::subscribers` |
| 3 | `screen/capabilities.rs` | 4069–4434 | `capabilities`, `host_query` |
| 4 | `screen/input_modes.rs` | 7637–7928 | — |
| 5 | `screen/sizing.rs` | 3705–4068 | `sizing` |
| 6 | `screen/break_panes.rs` | 9305–9703 | `break_panes_to_tab` |
| 7 | `screen/warp_ops.rs` | 8505–9004 | `scroll_and_warp` |
| 8 | `screen/render.rs` | 5529–6052 | full suite + insta |
| 9 | `screen/tabs.rs` | 2200–2711 | `tabs` |
| 10 | `screen/plugin_state.rs` | 6737–7402 | `plugin_broadcasts` |
| 11 | `screen/global_pane_layout.rs` | 2712–3141 | `global_pane_layout` |
| 12 | `screen/global_pane_focus.rs` | 3142–3559 | `global_panes_focus` |
| 13 | `screen/nested.rs` | 4477–5528 | `nested_sessions` |
| 14 | `screen/instruction.rs` | 403–1528 | `cargo check` |
| 15 | `screen/thread_main.rs` | 11485–17283 | full suite |

Line numbers drift after each step — re-derive from the topic markers, not from this table.

`warp_ops.rs` and `nested.rs` are named to avoid colliding with the existing flat `src/warp.rs`
and `src/nested_guest.rs`. Those siblings hold free functions and plain types, not `impl Screen`;
leave them where they are. `src/global_panes.rs` (4,062 lines) is a genuine standalone module and
should **not** move.

**The discipline is the whole safety mechanism.** One topic per commit, no body edits mixed in —
not a rename, not a `.context()`, not a lint fix. After each step,
`git diff -M -C --find-copies-harder --stat` should report a near-100% similarity move, and the
paired test command plus `cargo check -p zellij-server` should pass. Escalate to full
`cargo x test` at #8, #13 and #15.

## What was actually done

Seventeen commits, `2200b844e..f1fe10691`. `screen.rs` is now `screen/` — `mod.rs` at 4,016 lines
plus sixteen topic modules. Every commit was verified byte-identical against the previous `mod.rs`
before landing, and the full `cargo x test` suite is green at each of the three escalation points.

| File | Lines | | File | Lines |
|---|---|---|---|---|
| `mod.rs` | 4,016 | | `tabs.rs` | 520 |
| `thread_main.rs` | 5,848 | | `render.rs` | 535 |
| `instruction.rs` | 1,163 | | `break_panes.rs` | 415 |
| `nested.rs` | 1,071 | | `global_pane_focus.rs` | 441 |
| `warp_ops.rs` | 830 | | `sizing.rs` | 375 |
| `plugin_state.rs` | 689 | | `host_query.rs` | 319 |
| `mouse.rs` | 295 | | `notifications.rs` | 277 |
| `input_modes.rs` | 258 | | `subscriptions.rs` | 243 |
| `global_pane_layout.rs` | 179 | | `capabilities.rs` | 112 |

### Where this document was wrong, and what to trust instead

The plan above was written before the work and its line table had drifted. Corrections, for anyone
reading it as history:

- **The child modules keep `impl Screen`, but with explicit imports, not `use super::*;`.** The
  glob was proposed as the pragmatic choice; it turned out to be unnecessary. Deriving each
  import list mechanically — compile with the glob to prove the cut, then delete it and let rustc
  enumerate what is missing — is about as cheap and leaves files that read like `tab/`'s.
- **Privacy does *not* flow the way this document implied.** "The defining module and all its
  descendants" is right, but it means a *child* sees the *parent's* privates, never the reverse.
  Every method a moved run still owes to `mod.rs` needed `pub(super)`. That is 90-odd keywords
  across the split, and it is the only non-move content in any of the commits.
- **Four of the fifteen proposed ranges cut mid-run** and would have produced a broken tree:
  `capabilities` merged two topics and orphaned `resume_pane_after_forward`; `warp_ops` stopped
  inside `empty_and_close_tab`; `input_modes` cut into the focus-movement run; `tabs` started on
  `client_is_web`, which belongs to neither. Derive boundaries from method names, never line
  numbers.
- **Notifications became its own file**, rather than riding inside `global_pane_layout.rs`. It is
  a self-contained overlay subsystem sharing only `Viewport` with the geometry around it.
- **Extraction order barely matters.** Bottom-up was chosen so ranges would not drift, but adding
  each `mod x;` line shifts everything below it anyway, so boundaries have to be re-derived every
  step regardless.

### The two riders — both now done

Both riders below were in scope of section A as written and were left for a later pass. Both are
now done; see "What the follow-up pass did" at the end of this document.

- the `From<&ScreenInstruction> for ScreenContext` strum rewrite (next section);
- the `has_pane_with_pid` cleanup, which this document says to do per-module *after* the split.

The render/report epilogue remains rejected, for the reasons below.

## The 406-line `From` impl — a standalone rider

Worth doing on its own; it needs no file split and no new dependencies.

`zellij-utils/src/errors.rs:212` already carries the answer as a FIXME: *"Just deriving
EnumDiscriminants from strum will remove the need for any of this!!!"*. `strum`/`strum_macros` are
already workspace dependencies — but note they are *not* dependencies of `zellij-server`, only of
`zellij-utils`, so this rider starts with one line added to `zellij-server/Cargo.toml`.
`ScreenInstruction` now lives in `zellij-server/src/screen/instruction.rs`.

1. `strum_macros::IntoStaticStr` on `ScreenInstruction` — it works on data-carrying enums and
   yields the variant name, so no `EnumDiscriminants` layer is needed despite the FIXME's wording.
2. `strum_macros::EnumString` on `ScreenContext` (`errors.rs:215`).
3. Rewrite the impl as a small explicit match for the ~25 arms that are *not* 1:1 — `Resize` fans
   out to `ResizeIncrease{Left,Down,Up,Right,All}` by payload, `NewGlobalPane` maps to
   `ScreenContext::NewPane` — falling through to `ScreenContext::from_str(instruction.into())`.

406 lines becomes roughly 50.

> **Done.** It came out at 47 lines, and the arm count was better than this estimate: only **six**
> arms are not 1:1, not ~25. The plan was also right that `EnumDiscriminants` is unnecessary. Two
> things it did not anticipate: eighteen `ScreenContext` variants turned out to be unreachable
> (nothing constructs them at all), and the same treatment applies to the other five
> instruction/context pairs, which had two more name-drift bugs and five more dead variants
> between them.

**The cost, which must be paid, not skipped.** Today's match is compiler-exhaustive: add a variant
and the build breaks. `from_str` is not. Restore the guarantee with a test that derives
`strum::VariantNames` on `ScreenInstruction`, iterates `VARIANTS`, and asserts each name either
appears in the special-case list or resolves via `from_str`. That test is ~15 lines and is
*stronger* than the compiler check, because it also catches a name drifting apart between the two
enums — which a hand-written match cannot. Make `from_str` failure `panic!` under
`debug_assertions` rather than silently choosing a sentinel.

## Rejected: an automatic render/report epilogue

Inside `screen_thread_main`, `screen.render(None)` appears 199 times and
`screen.log_and_report_session_state()` 94 times, almost always as the tail of an arm. Collapsing
those into an automatic per-arm epilogue looks like deleting ~293 lines of boilerplate.

**Do not.** It is one behaviour change applied to 253 code paths at once, in the subsystem whose
tests were least trustworthy, and its failure symptom — an extra or missing render — is precisely
the symptom the flaky tests exhibited. The 54-arm gap between the two counts is meaningful and
undocumented; collapsing them means inferring intent for 253 arms from a diff. If it is ever done,
it belongs in its own project, arm-group by arm-group, with render counts asserted per group.

Move the match into `screen/thread_main.rs` (#15) and stop there.

## Worth doing, after the split

`has_pane_with_pid` appears **88 times** in `screen.rs`, in three spellings of the same
"find the tab containing this pane, then do X" idiom. A helper for exactly this already exists —
`tab_id_containing_pane` (`screen.rs:3865`) — and is almost unused, while `with_pane` (`:3155`) is
used 36 times and is the model to copy. Do this **per extracted module after the split**, so each
batch is small and its tests are already targeted. Not before.

> **Done**, in two commits — `thread_main.rs` first, then the other seven modules. 88 uses became
> four. `with_pane` was indeed the model: the helpers are `tab_containing_pane{,_mut}` plus
> `report_pane_not_found` for the eighteen sites that copied the same "not found" tail verbatim.
> `tab_id_containing_pane` moved to `screen/tabs.rs` and gained a `tab_position_containing_pane`
> sibling for the callers that look the tab back up by position.

---

# B. The plugin/IPC conversion layer

## Lead with the testing gap, not the line count

The instinct is "collapse 9,000 lines". That is the wrong lead. The alarming number is this:

- `PluginCommand` has **228 variants**; its `mod tests` has **22 tests**.
- `Action` has **153 variants**; `plugin_api/action.rs`'s tests number **3**.
- Encode and decode are two *independently hand-written* matches roughly 1,500 lines apart.
- There is **no exhaustiveness check anywhere**, including in
  `ipc/tests/roundtrip_tests.rs`, whose 374 `test_client_roundtrip!` invocations are all
  hand-constructed values with nothing to guarantee a new variant is among them.

A transposed field or a wrong payload tag in an untested variant is silent **today**. Collapsing
the code first would mean rewriting 9,000 untested lines and verifying the rewrite with 25 tests.

**So: tests first, then the Lua unblock, then — maybe — the collapse.**

## The scale

412 hand-written `From`/`TryFrom` impls: `plugin_api/plugin_command.rs` 123 (6,761 lines),
`ipc/protobuf_conversion.rs` 86 (5,405), `plugin_api/action.rs` 75 (3,770),
`plugin_api/event.rs` 57 (3,605), rest of `plugin_api/` 71. The bulk sits in four enormous impls,
the largest being `plugin_command.rs:756` (~2,460 lines) and `:3229` (~2,060 lines).

## B1 — Round-trip property tests (do this first)

`hegeltest` is **already** a dev-dependency of `zellij-utils`, `zellij-server`, `zellij-client` and
every plugin crate; `expect-test` is too. No dependency work is needed. Use the repo's `hegel`
skill for the idioms.

1. One round-trip property per enum: generate an arbitrary `PluginCommand` / `Action` / `Event`,
   encode to protobuf, decode, assert equality. One property each covers 412 hand-written impls.
2. **Exhaustiveness guards, separate from the properties** — a generator can under-sample a
   228-variant enum. Per enum, a `VariantNames`-driven test asserting a checked-in sorted variant
   list via `expect-test`, so a new variant fails with a readable diff naming it.
3. The known non-round-trippers must be *listed and asserted*, not skipped: `ShowCursor`
   (`plugin_command.rs:789`, `pos.x as usize` — lossy widening), `SetTabProjects` (`:2163`,
   map→repeated, so ordering is not preserved — assert set equality), and `Notify` (`:6356`, a
   deliberate legacy string payload when `pane_id` is `None` — a wire contract, assert it
   explicitly).

Expect this step to find bugs. That is the point of doing it first.

Cheap and independent of everything else: add a `VariantNames`-vs-invocation-list assertion to
`ipc/tests/roundtrip_tests.rs` (~20 lines). 374 hand-built values with no completeness guarantee is
a lot of effort producing false confidence.

> **Done.** Not by comparing against an invocation list — the two macros record the variant name
> they were handed, and `server_client_contract` compares that against `VariantNames` for both
> enums after both halves have run. It found three untested `ClientToServerMsg` variants
> immediately (`AttachWatcherClient`, `DesktopNotificationResponse`,
> `SoftKeyboardVisibilityChanged`), all now covered. `ServerToClientMsg` was already complete.
> The prediction that this step finds things held on the first run.

## B2 — The Lua unblock, and the gate it must not skip

Lua currently reaches **7 of 228** commands (`plugins/lua/command.rs:21-35`). CLAUDE.md says the
intended fix is to derive `Deserialize` on `PluginCommand` and replace `plugin_command_from_lua`'s
body with `from_value`. That is correct, and the blocker is smaller than it sounds: **exactly seven
types** lack `Deserialize`, all plain data with fully-`Deserialize` fields —

| Type | `zellij-utils/src/data.rs` |
|---|---|
| `PluginPermission` | 1172 |
| `CommandToRun` | 2974 |
| `GitCheckout` | 2994 |
| `MessageToPlugin` | 3030 |
| `NewPluginArgs` | 3044 |
| `PluginMessage` | 3221 |
| `HttpVerb` | 3245 |

Derive on those seven, then on `PluginCommand` (`data.rs:3798`).

Three things that must not be discovered late:

1. **The existing 7 arms *are* the current Lua API.** `lua/prelude.lua` and any user script depend
   on their exact shapes, and serde's default representation will very likely not match. Pin the 7
   call shapes with a test **before** the derive lands, then either `#[serde(rename …)]` to
   reproduce them or bump the API and update `prelude.lua` and `docs/LUA_PLUGINS.md` deliberately.
2. **`from_value` covers requests only.** ~76 of the 228 commands reply, via `ResponseSink::Capture`
   (`plugins/plugin_map.rs:343`); marshalling responses *back* to Lua needs `Serialize` on the
   response types and a `to_value` path. So this takes Lua from 7 to roughly 152 fire-and-forget
   commands — a large real win, and an acceptable intermediate state, **provided** those 76 return
   a clear "this command replies and Lua responses are not yet wired" error. Silently dropping a
   reply is the failure mode to design against.
3. **The security gate — this one is load-bearing.** `command_permission` in
   `plugins/zellij_exports.rs` ends in `_ => return None`, and `check_command_permission` (`:6432`)
   treats `None` as **`PermissionStatus::Granted`**. Unclassified commands therefore fail *open*.
   Today 7 hand-audited commands reach Lua; after the derive ~152 do at once, and CLAUDE.md is
   explicit that the permission system is the only thing between a Lua plugin and the server's
   authority. **An exhaustiveness test asserting every `PluginCommand` variant has an explicit
   permission classification must gate the derive, not follow it** — and the catch-all arm should
   become explicit arms first.

## B3 — The collapse (last, or never)

### Correction to a natural first impression

It looks as though `Action` ↔ protobuf is implemented twice and one copy could be deleted. It is
not. The two contracts are different wire formats on different transports:

- `plugin_api/action.proto:190` — `ActionName name = 1;` plus a **sparse** `oneof optional_payload`
  (a payload field only where one is needed). Crosses the **wasm plugin boundary**.
- `client_server_contract/common_types.proto:137` — a **pure** oneof, one message per variant, no
  name field. Carries **client↔server IPC**.

Unifying them changes one of the two on the wire, breaking either every third-party wasm plugin or
client/server compatibility across versions. **Do not unify them.**

What *is* available: **one shared variant table, two macro expanders**. Write the 153 variants and
their field lists once; `plugin_api/action.rs` and `ipc/protobuf_conversion.rs` each consume that
table through a macro that knows its own proto's shape. That removes the duplicated *maintenance* —
one place to add a variant — without touching either format. It is the highest-value piece here
even though the line count falls less than the raw duplication suggests.

### Mechanism: prefer a `macro_rules!` table first

A proc-macro derive crate is the standing preference if a declarative table proves insufficient,
but it should not be the opening move. The ~10% of arms that deviate would each need to be
expressible as an attribute, and the two protos have different shapes, so the derive would need two
modes — you would be inventing a DSL to describe 412 impls, and the DSL would read worse than the
impls. With a `macro_rules!` table, a deviating variant is simply **omitted from the table** and
written as ordinary Rust beside it, which keeps the exception visible.

The precedent is already in this layer: `plugin_api/style.rs:80` `color_definitions!` and `:93`
`multiplayer_colors!` are the same idea applied to one 30-line corner, and they are why
`TryFrom<ProtobufStyling> for Styling` at `:104` reads as 20 lines instead of 200.

Sequence: `Action` first (two consumers, clearest payoff), then `Event`, then `PluginCommand`
(biggest, most deviations). **One enum per commit**, with B1's round-trip property as the
verification — which is what B1 exists for.

## What it costs to add one command today

Nine files, ten to twelve edit sites: `data.rs` variant → `.proto` (a `CommandName` value, a
payload `oneof` field with a hand-numbered tag, an `XPayload` message, plus a `Response` message if
it replies) → regenerate the checked-in prost output → `plugin_command.rs` (import, decode arm,
encode arm) → `zellij_exports.rs` (dispatch arm, permission classification, handler) →
`shim.rs` wrapper → `lua/command.rs` arm → `lua/prelude.lua` wrapper.

Only three of those carry information. The rest is transcription.

---

# Bugs and smells found while surveying

Logged, not fixed — except the first, which had to be fixed because it made a test unstabilisable.

- **FIXED — nondeterministic pane placement.** `TiledPanes::panes` is a `BTreeMap`, but
  `TiledPaneGrid::new` collected them into a `HashMap` (`tiled_pane_grid.rs:31,43`), and
  `find_room_for_new_pane` (`:1387`) folded over it picking the largest pane with a strict `>`.
  With two equally-sized panes — the common case in any symmetric layout — the winner was decided
  by Rust's per-process randomized hash seed, so the same layout could grow a new pane in two
  different places on two runs of the same binary. This was the true cause of the long-standing
  `global_panes_focus` flake (12/20 before, 20/20 after). Now ranked by `(size, id)`, which is
  total and reproducible. The `FloatingPaneGrid` has the same `HashMap` (`floating_pane_grid.rs:27`)
  but only uses the geoms as an overlap set, so it is order-independent and was left alone.

- **FIXED — a fourth flaky test, found by the new gate.**
  `pty_host_remote_reader_tests::a_resize_made_during_an_outage_is_sent_again_on_reconnect` had two
  disagreeing deadlines: a helper thread waited up to 10s for the reconnect socket, while the main
  flow waited ~5s and then read the slot unconditionally. A reconnect arriving in between failed
  with "the reconnect never produced a socket". It passes 10/10 alone; only a loaded machine
  reveals it. Now a `Watched<Option<UnixStream>>` with a single bounded wait — and 2.4s instead of
  up to 10. Its sibling `answer_next_reconnect` (`:334`) keeps its 5s sleep deliberately: that one
  holds the socket open against EOF rather than waiting for anything.

- **NOT a bug — `scroll_terminal_up` "missing" the vte flush.** `tab/mod.rs:7008` lacks the
  `needs_flush` / `process_pending_vte_events` block that `scroll_terminal_down` (`:7047`) has, and
  this reads like copy-paste divergence. It is not. `tab/mod.rs:4481` buffers pty bytes only *while*
  a pane `is_scrolled()`, and the flush releases them when a downward scroll returns the pane to the
  bottom. Scrolling up moves away from the bottom and can never un-scroll a pane. The same asymmetry
  holds for `scroll_active_terminal_up`/`_down`, and every flush site in the file is a
  down/page-down/to-bottom path. Deliberate. Recorded here so it is not "fixed" later.

- **Directional copy-paste in `tab/mod.rs`.** `move_active_pane_{down,up,right,left}` and
  `move_pane_{…}` (`:6214-6341`) are eight methods identical modulo `Direction`;
  `move_focus_left`/`move_focus_right` (`:6040`/`:6136`) differ by five tokens. A readability
  concern with no known divergence — the one suspected divergence is the entry above, and it was a
  false alarm.

- **FIXED — `global_panes.rs` used the wrong error type.** It was 24 signatures, not 21, and 14
  bridges rather than 12 — the file also bridged the other way, turning two `anyhow` send errors
  into strings. All of them are gone. The inline test module moved to
  `unit/global_panes_tests.rs`, taking the file from 4,060 lines to 2,302.

- **FIXED — `let _ =` on the scroll-mode sync.** The six calls to `sync_scroll_mode_on_focus` /
  `sync_scroll_mode_if_scroll_changed` now use `.non_fatal()`. The 202 `let _ = ….send(…)` sites
  and the `let _ = self.render(…)` / `log_and_report_session_state()` sites are untouched: those
  are the separate policy question about failures on shutdown paths, where logging would be noise.

- **`zellij-tile/src/shim.rs` — 341 identical `try_into().unwrap()`** at the **public plugin API**
  boundary (e.g. `:95`), each an unexplained plugin abort. One pattern and one decision, not 341
  sites; the workspace's raw 1,167-unwrap count is misleading for the same reason.

- **`zellij-server/src/lib.rs` — ~145 unwraps**, dominated by `RwLock` poisoning in the accept and
  shutdown paths. The honest fix is a poison policy, not `?`.

- **FIXED — `#[allow(dead_code)]` on the whole `TiledPanes` struct.** It hid exactly one unused
  field, `stacked_pane_list`, threaded in from `Tab` and never read there. Field, constructor
  parameter and allow are gone, so the type is under dead-code analysis again.

- **`default-plugins/status-bar/` — `one_line_ui.rs` and `second_line.rs` are a near-verbatim
  fork**, sharing the same typo (`dedpulicate`) and the same FIXME. Real duplication, unrelated to
  everything above, and deliberately out of scope for any of it.

---

# What the follow-up pass did

Nine commits after the `screen.rs` split, all against the "concrete and low-risk" end of the
list above. Both of section A's riders, the cheap half of B1, and four of the logged smells.

| # | What | Effect |
|---|---|---|
| 1 | `ScreenContext` resolved by variant name | 406 lines → 47, plus three drift guards |
| 2 | eighteen unreachable `ScreenContext` variants dropped | dead code, and a reverse guard so it does not recur |
| 3–4 | `has_pane_with_pid` → `tab_containing_pane{,_mut}` | 88 uses → 4, ~600 lines |
| 5 | `TiledPanes`' blanket `dead_code` allow | one dead field, analysis restored |
| 6 | round-trip macros record what they cover | three untested IPC variants found and covered |
| 7 | scroll-mode sync errors logged, not discarded | |
| 8–9 | `global_panes.rs` on the project's `Result`, tests to `unit/` | 14 bridges gone, 4,060 lines → 2,302 |
| 10 | the other five instruction/context pairs | 198 lines → 60, one mislabel fixed, five more dead variants |

## Two things worth knowing before the next pass

**The name-driven conversion generalises, and its guards are the point.** Six instruction enums
now resolve their `*Context` by variant name, with `unit/instruction_context_tests.rs` asserting
in both directions: every instruction has a context, and every context is reachable. Writing
those guards found a mislabelled arm (`BackgroundJob::RenderToClients` reported itself as
`ReportPluginList`) and 23 dead context variants that years of exhaustive matches
never surfaced, because an exhaustive match proves only that each arm exists — not that it is
right, and not that the target is used. **This is the pattern to reach for elsewhere in the
codebase: replace a mechanical match with a name lookup, then spend the saved lines on a test
that the match could not have been.**

**Coverage instrumentation beats a checked-in list.** B1's plan proposed pinning variant lists
with `expect-test`. For the IPC round-trips it was cheaper and stronger to have the test macros
record what they were actually handed and compare that to `VariantNames` — no list to update, and
it measures the tests rather than a transcription of them. Prefer that shape for the `PluginCommand`
/ `Action` / `Event` work in B1 proper.

## Still open, in rough order of value

- **B1 proper** — the round-trip *properties* for `PluginCommand`, `Action` and `Event`. The
  coverage guard above is only the cheap rider; 412 hand-written conversions still have 25 tests.
- **B2 and B3**, unchanged, and B2's security gate still has to precede the derive.
- **`zellij-tile/src/shim.rs`'s 224 `try_into().unwrap()`** (the doc said 341; it is 224 now).
  Still one decision, not 224 sites.
- **`zellij-server/src/lib.rs`'s RwLock unwraps** — still wants a poison policy.
- **`default-plugins/status-bar/`'s near-verbatim fork**, still out of scope of everything else.
- **`tab/mod.rs`'s eight directional copy-paste methods** — untouched; the same "give the shape a
  name" treatment applies, but the parameter is a `Direction` rather than a lookup, so it is a
  different-shaped edit.
