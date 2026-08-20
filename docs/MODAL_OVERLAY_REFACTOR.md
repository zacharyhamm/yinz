# Modals and overlays: giving them their own collection

Working document for the `unify-modals-overlays` branch. Phases 0–3 have landed — overlays
are their own collection and the compensation layer is gone. Phase 4 is what is left: the
in-repo plugins still open floating and promote themselves, which is the last thing keeping
the two-stage birth alive.

## Why

An overlay is a pane drawn over the whole session surface, above the global panes,
deliberately *not* on a tab's floating layer — `file-finder`, `hocus-ask`,
`quit-confirmation`, `hocus-new-agent`, `hocus-duplicate-agent`. A modal is an overlay
that must not have focus taken from it, because it has no way to learn it lost it.

Neither existed as a thing. Overlay-ness was *presence of a key* in
`FloatingPanes::overlay_coordinates`; modal-ness was re-derived on every query by
string-matching `configuration["modal"]` through `pane.invoked_with()`. Callers that
rebuilt a pane's coordinates dropped the first, which is why the second existed, and why
the two were OR'd together at eight sites.

Worse, an overlay was **born as an ordinary floating pane and promoted afterwards** —
every popup plugin opens floating and then calls
`change_floating_panes_coordinates(..).as_overlay()`. Opening a floating pane raises the
floating layer, so the promotion has to undo that. `layer_visible_when_added`,
`take_layer_state_before_adding`, `hide_floating_panes_under_overlay` and
`ActivePanes::unfocus_all_panes_except` exist only to compensate for a state the pane
should never have been in.

That produced a recurring bug class, visible in the log as a run of corrective patches
(`fix overlay hidden floating pane behavior`, `Fix modals and overlays`, `Keep tiled
state and focus in sync when an overlay hides the floating layer`, `Keep modal overlays
focused`, `Don't let a self-upgrading popup reveal the hidden floating layer`). Each
fixes one path that assumed "floating layer down" meant "nothing on it is focused" — an
assumption only wrong because overlays live in the floating collection.

**Goal:** overlays and modals become a first-class collection, a sibling of
`tiled_panes` and `floating_panes` on `Tab`. An overlay is created directly into it and
is never a floating pane, so the compensation layer is deleted and the bug class becomes
unrepresentable.

As of Phase 3 that holds for every path *inside the server*. What still arrives the old
way is a popup plugin, which opens floating and promotes itself from `load()` — so the
promotion path survives as a genuine migration between collections, and one piece of the
compensation (`layer_visible_when_added`) survives with it. Phase 4 removes both.

---

## Done

### Phase 0 — the safety net (commit `c9d41cf57`)

Characterization tests, render snapshots, integration tests and structural invariants,
all landed and green against the pre-refactor code so the move can be shown to preserve
behavior.

`Tab::assert_invariants()` (`zellij-server/src/tab/mod.rs`, `#[cfg(test)]`) collects the
structural rules and is driven by a `hegel` property test over random operation
sequences (`unit/screen_tests/overlays.rs`, `mod invariant_props`). Three invariants are
active; two are named but switched off until they can hold — see Phase 3.

Two insta snapshots cover occlusion, which had none:
`frame_of_an_overlay_over_a_hidden_layer_beside_a_global_pane` and
`frame_of_an_overlay_with_a_pinned_and_a_plain_floating_pane`.

### Phase 1 — overlay-ness becomes a placement (commit `73c1603d0`)

`OverlayKind { Dismissible, Modal }` and `OverlayPlacement { kind, coordinates }` in
`zellij-utils/src/data.rs`, modelled on `GlobalPanePlacement`; `NewPanePlacement::Overlay`
carries it. Wire variants added to both contracts in the oneof slots kept for
backward-compatible additions. `zellij action new-pane --modal` added alongside
`--overlay`. A terminal pane can now be modal.

`RunPluginOrAlias::overlay_placement()` is the **single compatibility shim** for the old
`overlay`/`modal` configuration keys, resolved once at the
`ScreenInstruction::AddPlugin` entry point (`screen.rs:14527`).
`requests_overlay`/`requests_modal` are now thin wrappers over it.

**Deviation from the original plan, deliberate.** The plan had Phase 1 also dropping
`FloatingPaneCoordinates::overlay` and threading `OverlayPlacement` onto
`Action::LaunchOrFocusPlugin`/`LaunchPlugin`. That is 46 construction sites, and dropping
the coordinates field means rewriting every server storage site in the same commit —
the big-bang the phasing exists to avoid. Instead the commit resolves to a placement at
the entry point and keeps the existing storage, with
`OverlayPlacement::as_floating_pane_coordinates()` as the one translation. It changes no
behavior. The storage swap is Phases 2–3; the field deletion moves to Phase 4.

### Phase 2 — the `OverlayPanes` collection (commit `af3ae0f67`)

`zellij-server/src/panes/overlay_panes/mod.rs`, with its own unit tests in
`overlay_panes/unit/`. Built, wired onto `Tab`, and consumed by nothing: every existing
test passes unchanged because no call path reaches it yet. Phase 3 routes to it.

The surface computation came out of `FloatingPanes::coordinate_space` as
`global_panes::session_surface(display_area, display_area_origin, strips) -> Viewport`,
which both collections now call. It was *extracted, not moved* - finding 6.

`Tab::assert_invariants()` gained `overlay_panes` in invariant 1 (a pane lives in exactly
one collection) and a second copy of invariant 4 (the occlusion stack agrees with every
pane's z-index) for the new collection. Invariants 2 and 3 are still off; they are
Phase 3's.

**Deviations from the Phase 2 sketch below, both forced:**

1. **`ActivePanes` is not "reused unchanged".** The sketch asks for both
   `panes: BTreeMap<PaneId, OverlayPane>` and an untouched `ActivePanes`, and those cannot
   both hold - `ActivePanes`' signatures named `BTreeMap<PaneId, Box<dyn Pane>>`
   concretely. It gained a one-method `PaneLookup` trait instead. This turned out to be
   nearly free: `ActivePanes` only ever calls `panes.get(&pane_id)`, read-only, so the
   change also drops a `&mut` that was never used at 27 call sites.
2. **`session_surface` returns only the `Viewport`.** `coordinate_space` returned
   `(Viewport, (usize, usize))`, but for the overlay arm the origin is always
   `(surface.x, surface.y)`, so callers derive it rather than being handed it.

Two things in `OverlayPanes::render` are worth knowing before Phase 3 touches it:

- It takes a **`z_index_offset`**. `Output` receives one flat `FloatingPanesStack` and a
  pane's z-index is its absolute position in it, so once `Tab::floating_panes_stack`
  concatenates floating-then-overlay layers, overlay indices must be offset by the floating
  layer count. The parameter exists now so the signature does not have to change later.
- `render` iterates `stack_layer_ids()` **directly** rather than re-deriving the list.
  `FloatingPanes` keeps two hand-duplicated filter chains in step by hand
  (`floating_panes/mod.rs:150-185` vs `:700-722`) and warns that a pane given an index that
  is not its own erases itself. That is the refactor's highest-risk site; here there is no
  second list to drift.

`OverlayPanes` deliberately has no `show_panes`, `fullscreen_pane_id`,
`desired_pane_positions` or `pane_being_moved_with_mouse`, and the module doc says why for
each. If Phase 3 finds it needs one of them, that is a signal about the concept - say so
rather than adding it quietly.


### Phase 3 — the behavior moves across (commits `c2a9470aa`…`f55cbef15`)

Five commits: route, delete, then one per behavior flip.

- **`c2a9470aa` route overlays into their own collection.** Creation, lookup, render,
  z-order, focus, mouse and lifecycle all reach `OverlayPanes`. `FloatingPanes` still
  carries its overlay members but nothing reaches them.
- **`f9116d985` delete the compensation layer.** `overlay_coordinates`, `is_overlay`,
  `has_overlays`, `pane_is_modal`, `remember_overlay_coordinates`,
  `reapply_overlay_coordinates`, `last_selectable_non_overlay_pane_id`,
  `hide_panes_keeping_overlay_focus`, `Tab::hide_floating_panes_under_overlay`,
  `Tab::show_floating_panes_for`, `ActivePanes::unfocus_all_panes_except`. Invariants 2
  and 3 switched on.
- **`017977bc5`** finding 1, **`ee30c151d`** findings 2 and 3, **`f55cbef15`** finding 4.

**Deviations from the Phase 3 plan, all forced and all recorded here:**

1. **`layer_visible_when_added` does not die yet.** The plan had the whole compensation
   layer going. It cannot: the in-repo popup plugins are still born as ordinary floating
   panes and promote themselves once running, and opening an ordinary floating pane raises
   the layer — so the promotion still has to put the layer back the way the pane found it.
   Everything *downstream* of that note does go, because the promotion is now a genuine
   migration out of `floating_panes`: with the overlay off the layer, the plain
   `hide_floating_panes` is correct, there being no focused pane down there to lie to.
   Phase 4's plugin migration removes the last use.
2. **Promotion survives as a migration, not a flag.** `change_floating_pane_coordinates`
   with `overlay: true` on a floating pane extracts it and re-adds it via
   `add_overlay_pane`, carrying its focus across *silently* — it neither gained nor lost
   focus, so it is told nothing. `ActivePanes::insert_silently` is the whole of what
   `unfocus_all_panes_except` used to do, and it is used once.
3. **Invariant 3 has two halves.** "Floating-layer visibility is never made true by
   opening or focusing an overlay" is a property of a *step*, not of a state — the layer
   can legitimately be up with nothing on it, because the user can just toggle it. The
   state half (no overlay is in the floating layer's rendered set) is in
   `assert_invariants`; the transition half is asserted around every step of the property
   test. `PromoteToOverlay` is exempt from the transition half, and is the case that proves
   it: the promoted pane was an ordinary floating pane a moment earlier and *that* raised
   the layer. The exemption goes with the compensation in Phase 4.
4. **Finding 4 was decided here, not deferred.** The split answers it by construction —
   `Tab::occlusion_stack` appends the overlay layers after every floating layer — so
   deferring it would have meant deliberately re-introducing the old order. A modal now
   outranks a pinned pane.
5. **`OverlayPanes::focus_pane` focuses one client, not all.** The module doc claimed
   overlays are focused for every client at once "as floating panes are". `FloatingPanes`
   does no such thing, and a second client looking at a tiled pane must keep looking at it
   (`overlay_does_not_show_hidden_floating_panes`). `focus_pane_for_all_clients` remains
   for the no-client path, and *that* is what masks every client's focus when an overlay
   opens (`an_overlay_masks_each_client_s_tiled_focus_until_it_closes`).
6. **Accessors added to `OverlayPanes`**, none of them from the four deliberate absences:
   `get_pane`/`get_pane_mut` (by value, so the `.or_else` chains in `Tab` read the same for
   every collection), `get_panes`, `pane_info`, `placement_of`, `get_plugin_pane_id`,
   `focus_pane_if_exists`, `defocus_pane`, `adopt_focus`, `update_pane_themes`,
   `last_selectable_pane_id`.

**A bug the split found**, fixed in `f55cbef15`: `Tab::render_cursor` asked only
`floating_panes` for the active pane's z-index, so an overlay's cursor was checked against
index 0 and clipped by every layer in the stack. Five lookup chains had the same shape and
reached past the overlay collection — they use `.get(&id)` rather than `.get_pane_mut(id)`,
which is why the bulk pass over the `.or_else` chains missed them.

---

## Findings that the remaining work depends on

Discovered while writing Phase 0. Each was pinned by a test asserting the pre-refactor
behavior. **1–4 and 9 are resolved by Phase 3**; the rest still describe the code.

1. **RESOLVED (`017977bc5`). An overlay was re-resolved against the stale surface on
   resize.**
   `reapply_overlay_coordinates` reads `display_area`, but in
   `Tab::resize_whole_tab_locally` that shared cell is only updated by
   `tiled_panes.resize()` — which runs *after* `floating_panes.resize()`
   (`tab/mod.rs:5423` vs `panes/tiled_panes/mod.rs:1623`). A 60%-wide overlay stays 48
   columns when the terminal grows 80 → 120.
   Now `an_overlay_is_re_resolved_against_the_surface_on_resize`: `OverlayPanes` is not
   part of the floating layer's resize at all, so `Tab` reapplies it after the surface is
   known.

2. **RESOLVED (`ee30c151d`). `visible_panes_count()` counted overlays**, which gates the
   auto-relayout in `Tab::extract_pane`. Now
   `an_overlay_is_not_counted_by_the_floating_collection`.

3. **RESOLVED (`ee30c151d`). Closing the last real floating pane under an overlay left the
   layer flagged visible**, because the overlay kept `has_selectable_panes()` true. Now
   `closing_the_last_floating_pane_under_an_overlay_takes_the_layer_down`.

4. **RESOLVED (`f55cbef15`). A pinned pane painted above an overlay.** Not by decision:
   overlays shared the floating layer's `z_indices` and `make_sure_pinned_panes_are_on_top`
   had already lifted the pinned pane. `Tab::occlusion_stack` appends the overlay layers
   after every floating layer, so a modal outranks a pinned pane — which is what the one
   pane the user has to answer should do. Two snapshots moved deliberately.

5. **An overlay masks each client's tiled focus until it closes** —
   `get_active_pane_id` prefers it. Deliberate for a modal; currently true for every
   overlay. The tiled focus underneath is still recorded and resurfaces on close.
   Pinned by `an_overlay_masks_each_client_s_tiled_focus_until_it_closes`.

6. **`coordinate_space(overlay: true)` is shared, not overlay-only.**
   `restore_geoms_outside_viewport` uses it via `move_surface()` for *plain* floating
   panes, because a floating pane can legitimately be dragged over a global pane. It was
   extracted, not moved, in Phase 2 (`global_panes::session_surface`); Phase 3 split the
   boolean parameter into `FloatingPanes::move_surface` and
   `FloatingPanes::placement_space`, which is what it always meant.

7. **`change_floating_panes_coordinates` must stay.** It is used for ordinary resizing by
   `compact-bar:450`, `multiple-select:491,512`, `about:300`, `hocus/tools.rs:2277`.
   Only the `.as_overlay()` promotion goes away.

8. **`hocus-new-agent` re-asserts `.as_overlay()` on every resize**
   (`default-plugins/hocus-new-agent/src/main.rs:228-241`), so overlay-ness must stay
   sticky across coordinate changes. Since Phase 3 it holds by construction:
   `OverlayPanes::change_pane_coordinates` leaves the stored `kind` alone.

9. **RESOLVED by construction. `layout_applier.rs:869,944` drained `floating_panes`** for
   swap layouts, sweeping overlays into relayouts. Those drains no longer see an overlay,
   which also fixes the silent demotion of an `--overlay` *terminal* pane — it had no
   plugin configuration for `add_floating_pane` to re-derive overlay-ness from.

10. **NEW, for Phase 4. An overlay is not drawn under a UI-covering fullscreen tiled
    pane**, and focus follows: `Tab::get_active_pane_id` hands the keyboard back to the
    tiled pane. That is arguably wrong for a *modal*, which would be focused and invisible,
    but it is what the code did before the split and changing it is a separate decision.
    Pinned by `overlay_focus_tracks_whether_the_overlay_is_rendered`; the single gate is
    `Tab::overlays_are_rendered`.

---

## Phase 2 — the `OverlayPanes` collection (**landed**; see Done, above)

Kept for the reasoning behind the shape, and for the deviations recorded above.
`zellij-server/src/panes/overlay_panes/mod.rs`. Structural precedent:
`GlobalPaneManager` (`global_panes.rs:436`), a pane collection owning its own placement
metadata and focus map.

```rust
struct OverlayPane {
    pane: Box<dyn Pane>,
    kind: OverlayKind,
    coordinates: FloatingPaneCoordinates,
}

pub struct OverlayPanes {
    panes: BTreeMap<PaneId, OverlayPane>,
    z_indices: Vec<PaneId>,
    active_panes: ActivePanes,          // reused unchanged from panes/active_panes.rs
    // shared cells, for the surface coordinate space
    display_area: Rc<RefCell<Size>>,
    display_area_origin: Rc<RefCell<(usize, usize)>>,
    global_pane_strips: Rc<RefCell<Vec<GlobalPaneStrip>>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<HashMap<ClientId, bool>>>,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    style: Style,
    session_is_mirrored: bool,
    default_mode_info: ModeInfo,
    senders: ThreadSenders,
    pane_frame_style: PaneFrameStyle,
}
```

Deliberately **absent**: `show_panes` (an overlay is always shown while it exists — there
is no layer to raise), `fullscreen_pane_id`, `desired_pane_positions`,
`pane_being_moved_with_mouse` (overlays are not user-draggable). If one of these turns
out to be needed, that is a signal the concept is not as clean as assumed — say so
rather than adding it quietly.

Also in this phase:

- Extract the surface computation out of `FloatingPanes::coordinate_space` into a free
  function beside `expand_viewport_over_global_panes` (`global_panes.rs:93`), and have
  both collections call it. See finding 6.
- Add the field to `Tab` (`tab/mod.rs:261-320`) and construct it in `Tab::new`
  (`tab/mod.rs:996-1076`), wired but not yet consumed.

**Gate:** the collection compiles and is covered by its own unit tests; every existing
test still passes because nothing routes to it yet.

---

## Phase 3 — move each behavior across (**landed**; see Done, above)

The site-by-site table that used to be here has been executed. What is worth keeping is
where the risk actually was, for whoever touches this next:

**The render/z-order split** was correctly identified as the highest-risk site, and the
shape of the danger was right: `stack_layer_ids` and `render` must be derived from the same
list, and a pane given an index that is not its own is clipped against its own geometry and
erases itself. What the plan did not anticipate is that the *same* discipline applies to
three more places, because `Output` takes one flat list:

- `Tab::occlusion_stack` (what is clipped against) and
  `Tab::render_floating_panes_after_global_panes` (what is drawn) must gate on the same
  condition — `Tab::overlays_are_rendered` is that condition, and exists so there is only
  one of it.
- `Tab::render_cursor` computes a z-index into the same list and must apply the same
  floating-layer offset. It did not, and an overlay's cursor was clipped away by every
  layer in the stack.
- `Screen::last_rendered_floating_stacks` caches the layer vector per tab to decide whether
  the global panes need a full repaint. It reads `Tab::floating_panes_stack`, which is now
  the concatenated list, so it picks up overlay movement.

**`focus_plugin_pane`'s cross-tab move** was the second-highest risk and was
straightforward: the tab a pane leaves now hands over an `OverlayPlacement` rather than
coordinates the far side has to re-interpret.

**What the table missed** were the roughly sixty `.or_else(|| self.floating_panes.get_…)`
and `panes_contain` dispatch sites in `tab/mod.rs`. They are mechanical, but they are not
optional: `Tab::assert_invariants`' fifth invariant goes through `get_pane_with_id`, so a
missed one fails the property test immediately — which is what it is for. The five that
*silently* did the wrong thing were the ones spelled `.get(&id)` rather than
`.get_pane_mut(id)`.

---

## Phase 4 — born-as-overlay, plugin migration, cleanup

- `route.rs:1213-1220`: `Action::Quit` stops injecting `configuration {"modal":"true"}`
  and passes `OverlayPlacement::modal(None)`.
- Add `change_overlay_coordinates(..)` to `zellij-tile/src/shim.rs` (beside `:3211`),
  its `zellij_exports.rs:4735` host function and a `ScreenInstruction`, so an overlay
  resizes itself *within* the overlay collection. `change_floating_panes_coordinates`
  stays for ordinary resizing — finding 7.
- Migrate the in-repo plugins off `.as_overlay()`: `hocus-new-agent/src/main.rs:232-241`,
  `hocus-ask/src/main.rs:382,411,469`, `file-finder/src/main.rs:324,369`,
  `quit-confirmation/src/main.rs:34-46`.
- Rewrite `zellij-integration-tests/tests/floating_pane_overlay.rs`
  (`open_self_upgrading_popup`, `a_self_upgrading_popup_does_not_reveal_a_hidden_floating_pane`,
  `closing_a_popup_leaves_the_hidden_floating_layer_hidden`) against the new mechanism.
  **Rewrite, do not delete** — the user-visible guarantee still holds; only the mechanism
  changes. They pass unchanged after Phase 3 because the promotion path still exists.
- Delete `FloatingPaneCoordinates::overlay` and `as_overlay()`. Phase 3 removed every
  *storage* use; what is left is a handful of read sites treating it as an input
  (`overlay_placement_for` in `tab/mod.rs`, and the plugin callers being migrated above).
  Keep `requests_overlay`/`requests_modal`/`overlay_placement` and
  `Tab::run_requests_overlay` as the documented compatibility shim; they go in a later
  release once the old configuration keys are out of circulation.
- **Delete the last of the compensation layer**, which Phase 3 could not:
  `FloatingPanes::layer_visible_when_added` with `note_layer_state_before_adding` and
  `take_layer_state_before_adding`, the promotion branch of
  `Tab::change_floating_pane_coordinates`, and the `PromoteToOverlay` exemption from the
  property test's invariant-3 transition check. All four exist for the two-stage birth; a
  pane born an overlay reveals nothing, so there is nothing to put back.
- Decide finding 10: should a modal be drawn over a UI-covering fullscreen tiled pane?
  Today it is not, and focus follows it away. `Tab::overlays_are_rendered` is the single
  place to change.
- Enforce that an overlay cannot go fullscreen, and say so in the module doc. `OverlayPanes`
  has no `fullscreen_pane_id`, so it is no longer *reachable* — but `Tab` still has
  `toggle_pane_fullscreen` paths that simply fall through for an overlay rather than
  refusing, which is luck rather than design.

---

## Do not touch

The nested-session **guest modal** — `zellij-server/src/panes/nested_session_modal.rs`,
`Pane::set_guest_modal`, `GuestModalOutcome`,
`mouse_handler::intercept_guest_modal_mouse_event`, and the
`nested_guest_modal` integration tests. Despite the name it is an in-pane choice dialog
drawn onto a terminal pane's cells, unrelated to this work.

---

## Verification

```sh
# inner loop
cargo test -p zellij-server --all-features -- overlays
cargo x build -p                       # plugins only, before tests that load one
# after proto/asset changes
cargo x proto && cargo x assets
# whole-repo gates (this change spans zellij-utils, zellij-server, zellij-tile and
# default-plugins, so the full suite is required — see AGENTS.md)
cargo x build && cargo x test && cargo x test --no-web
cargo x integration-test -- -j 4
cargo x assets --check && cargo x format --check
```

Notes from running these on this branch:

- **Use `-j 4` for `integration-test`.** At default parallelism on a 64-core machine the
  suite produced ~105 timeouts purely from contention; the same tests pass at `-j 4`.
- **Pre-existing failures**, confirmed by stashing the branch and re-running:
  `misc::quit_with_keybinding`, and the `nested_guest_modal` / `nested_hardening` group.
  Not caused by this work.
- **The `zellij-server` lib suite is flaky under parallel load, and the failing set changes
  from run to run.** Four runs on the same tree produced four different one- or two-test
  failure sets, drawn from
  `screen_tests::global_panes_focus::no_new_pane_reaches_the_global_area_through_the_screen_thread`,
  `plugins::plugin_tests::keybinds::mode_update_payload_is_lightweight_for_opted_in_plugins`
  and `screen_tests::cli_actions::send_cli_scroll_up_action`. Every one of them passes when
  run alone, and `global_panes_focus` also fails on a stashed (pre-branch) tree — so this is
  load, not a regression.

  The mechanism is visible in the tests themselves: `mode_update_payload...` loads a real
  wasm plugin and then synchronizes with bare `std::thread::sleep(500ms)`
  (`plugins/unit/plugin_tests/keybinds.rs:95,114,133`). Under contention the load overruns
  the sleep and the assertion reads a half-built state.

  **Consequence for anyone measuring this branch:** a single red run is not evidence. Re-run,
  and check the named test alone before believing it. Adding tests to this suite — Phase 2
  added 11 — changes the scheduling and so changes which of these surface.
- **`cargo x build` and `cargo x integration-test` rewrite every `.wasm` under
  `zellij-utils/assets/plugins/`.** Revert that churn (`git checkout --
  zellij-utils/assets/plugins/`) unless plugin source actually changed, or it lands in
  the commit.

**Snapshots expected to change legitimately:** the config snapshots under
`zellij-utils/src/kdl/snapshots/` if KDL serialization shifts, and any
`screen_tests`/`tab_integration_tests` snapshot whose frame contains an overlay. Frames
*without* an overlay must not move — a diff there is a real regression, not a
re-baseline.
