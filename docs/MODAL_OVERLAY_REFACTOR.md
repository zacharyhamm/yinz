# Modals and overlays: giving them their own collection

Working document for the `unify-modals-overlays` branch. Phases 0 and 1 have landed;
this records what is left, and what was learned along the way that the remaining work
depends on.

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

---

## Findings that the remaining work depends on

Discovered while writing Phase 0. Each is pinned by a test that asserts today's
behavior and is expected to flip.

1. **An overlay is re-resolved against the stale surface on resize.**
   `reapply_overlay_coordinates` reads `display_area`, but in
   `Tab::resize_whole_tab_locally` that shared cell is only updated by
   `tiled_panes.resize()` — which runs *after* `floating_panes.resize()`
   (`tab/mod.rs:5423` vs `panes/tiled_panes/mod.rs:1623`). A 60%-wide overlay stays 48
   columns when the terminal grows 80 → 120.
   Pinned by `an_overlay_is_re_resolved_against_the_stale_surface_on_resize_today`.
   *Fix falls out of Phase 3*: `OverlayPanes` reapplies after the surface is known.

2. **`visible_panes_count()` counts overlays**, which gates the auto-relayout in
   `Tab::extract_pane` (`floating_panes/mod.rs:1918`).
   Pinned by `an_overlay_is_counted_by_the_floating_collection_today`.

3. **Closing the last real floating pane under an overlay leaves the layer flagged
   visible**, because `floating_panes.has_panes()` is still true (`tab/mod.rs:6153`).
   Pinned by `closing_the_last_floating_pane_under_an_overlay_leaves_the_layer_up_today`.

4. **A pinned pane paints above an overlay** while the layer is visible —
   `stack_layer_ids` returns the plain `z_indices`, and
   `make_sure_pinned_panes_are_on_top` has already lifted the pinned pane. Whether a
   modal should outrank a pinned pane is an open question; recorded in the snapshot so
   the refactor does not change it by accident.

5. **An overlay masks each client's tiled focus until it closes** —
   `get_active_pane_id` prefers it. Deliberate for a modal; currently true for every
   overlay. The tiled focus underneath is still recorded and resurfaces on close.
   Pinned by `an_overlay_masks_each_client_s_tiled_focus_until_it_closes`.

6. **`coordinate_space(overlay: true)` is shared, not overlay-only.**
   `restore_geoms_outside_viewport` (`floating_panes/mod.rs:239`) uses it via
   `move_surface()` for *plain* floating panes, because a floating pane can legitimately
   be dragged over a global pane. It must be extracted, not moved.

7. **`change_floating_panes_coordinates` must stay.** It is used for ordinary resizing by
   `compact-bar:450`, `multiple-select:491,512`, `about:300`, `hocus/tools.rs:2277`.
   Only the `.as_overlay()` promotion goes away.

8. **`hocus-new-agent` re-asserts `.as_overlay()` on every resize**
   (`default-plugins/hocus-new-agent/src/main.rs:228-241`), so overlay-ness must stay
   sticky across coordinate changes. Today that is `floating_panes/mod.rs:1318`; after
   the refactor it holds by construction.

9. **`layout_applier.rs:869,944` drains `floating_panes`** for swap layouts, sweeping
   overlays into relayouts. Fixed by construction.

---

## Phase 2 — the `OverlayPanes` collection

New `zellij-server/src/panes/overlay_panes/mod.rs`. Structural precedent:
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

## Phase 3 — move each behavior across

The bulk of the work. Site by site:

| Site | Becomes |
|---|---|
| `floating_panes/mod.rs:150-181` `stack_layer_ids` | Drops the `\|\| has_overlays()` branches; overlays order themselves in `OverlayPanes`. |
| `floating_panes/mod.rs:188-209` `stack()` | `Tab::floating_panes_stack()` (`tab/mod.rs:4974`) concatenates floating layers then overlay layers, so overlays clip above. Only 3 call sites: `tab/mod.rs:4975,5077,5172`. |
| `tab/mod.rs:5148-5155` `has_floating_panes_to_render` | Split; a new `has_overlays_to_render()` drives an `OverlayPanes::render` call added after the floating one in `render_floating_panes_after_global_panes:5129` (reached from `screen.rs:5740,5961`). |
| `floating_panes/mod.rs:630-850` `render` | Overlay/modal branches at `:640-648`, `:704-719`, `:735-739`, `:781-791` all delete. `OverlayPanes::render` is a smaller copy: `show_pinned_indication: false`, `show_scroll_indication: kind != Modal`. `ui/pane_contents_and_ui.rs:325-334` and `pane_boundaries_frame.rs:96-97` signatures unchanged. |
| `tab/mod.rs:3966-3975` `get_active_pane_id` | `overlay_panes.active_pane_id(client)` first — unconditional, no layer-visibility filter, which is the whole point — then floating (gated on `panes_are_visible()`), then tiled. |
| `tab/mod.rs:2843-2890` `new_pane` | `active_pane_is_floating` loses its `&& !is_overlay` exclusion. The `match active_overlay` block queries `overlay_panes` and branches on the stored `OverlayKind` instead of `pane_is_modal`. |
| `tab/mod.rs:7282-7290` `show_floating_panes_for` | Deleted — nothing in the overlay collection can drag the layer up. |
| `tab/mod.rs:7303-7315` `hide_floating_panes_under_overlay`, `floating_panes/mod.rs:440` `hide_panes_keeping_overlay_focus`, `active_panes.rs:82-98` `unfocus_all_panes_except`, `layer_visible_when_added` + `note/take_layer_state_before_adding` | All deleted. |
| `tab/mod.rs:8141-8190` `change_floating_pane_coordinates` | Loses the whole `was_overlay`/`is_overlay` promotion block (`:8158-8187`); becomes a plain coordinate change. |
| `tab/mod.rs:2561` `toggle_floating_panes` | `last_selectable_non_overlay_pane_id` → plain `last_selectable_floating_pane_id`. |
| `mouse_handler.rs:1612-1623`, `:1827-1836` | Ask `overlay_panes.visible_pane_id_at` before `floating_panes`; the "is it an overlay?" test disappears. |
| `tab/mod.rs:6113-6171` `extract_pane` | Add an overlay branch. The `!has_panes() → hide_floating_panes()` check at `:6153` becomes correct — finding 3. |
| `screen.rs:7974-8010` `focus_plugin_pane` | Carries `OverlayPlacement` across tabs instead of re-deriving; the `is_overlay` OR at `:7999-8002` goes. |
| `screen.rs:3229-3288` `activate_overlay` / `take_global_focus_after_overlay_close` / `suspended_global_focus` | Genuinely overlay-specific; **stays**. Only `tab.pane_is_overlay` is re-pointed. |
| `floating_panes/mod.rs:221-231` `geoms_outside_viewport`, `:1428-1447` `reapply_overlay_coordinates`, `:264` `has_panes_outside_viewport` | Overlay filter drops out; `OverlayPanes` gets its own reapply-on-resize (fixing finding 1) and outside-viewport report, unioned at `tab/mod.rs:4960`. |
| `floating_panes/mod.rs:352,366` `replace_pane`/`remove_pane` | Overlay-map migration lines delete. |
| `layout_applier.rs:869,944` | Naturally excludes overlays — finding 9. |
| `plugins/mod.rs:390-397` | Rejection check re-pointed at the explicit placement. |

Then switch on the two deferred invariants in `Tab::assert_invariants()`:

1. No overlay pane id appears in `floating_panes`.
2. Floating-layer visibility is never made true by opening or focusing an overlay.

And flip the three bug-pinning tests to assert the correct behavior, each in the commit
that fixes it, so the change is visible as a deliberate flip rather than an unexplained
edit.

**Highest-risk site:** the render/z-order split. `stack_layer_ids` and `render` must stay
derived from the same list or a pane is clipped against its own geometry and erases
itself — the file warns about this at `floating_panes/mod.rs:150-155`. Splitting across
two collections is exactly the change that can break it; the Phase 0b snapshots are the
guard. **Second-highest:** `focus_plugin_pane`'s cross-tab move, which reconstructs
overlay-ness at the moment a pane has left one tab and not yet joined another.

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
- Rewrite `zellij-integration-tests/tests/floating_pane_overlay.rs:242,288`
  (`open_self_upgrading_popup`, `a_self_upgrading_popup_does_not_reveal_a_hidden_floating_pane`)
  against the new mechanism. **Rewrite, do not delete** — the user-visible guarantee
  still holds; only the mechanism changes.
- Delete `FloatingPaneCoordinates::overlay` and `as_overlay()`, and the remaining
  `FloatingPanes` overlay members. Keep `requests_overlay`/`requests_modal` and
  `overlay_placement` as the documented compatibility shim; they go in a later release
  once the old configuration keys are out of circulation.
- Decide finding 4: should a modal outrank a pinned pane? If yes, it is a deliberate
  snapshot change with its own commit.
- Enforce that an overlay cannot go fullscreen, and say so in the module doc.
  `FloatingPanes` has `fullscreen_pane_id` and `floating_fullscreen_covers_ui`; what a
  fullscreen overlay means is undefined today and happens to be reachable.

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
- `screen_tests::global_panes_focus::no_new_pane_reaches_the_global_area_through_the_screen_thread`
  is flaky — it failed once and passed on re-run with the same tree.
- **`cargo x build` and `cargo x integration-test` rewrite every `.wasm` under
  `zellij-utils/assets/plugins/`.** Revert that churn (`git checkout --
  zellij-utils/assets/plugins/`) unless plugin source actually changed, or it lands in
  the commit.

**Snapshots expected to change legitimately:** the config snapshots under
`zellij-utils/src/kdl/snapshots/` if KDL serialization shifts, and any
`screen_tests`/`tab_integration_tests` snapshot whose frame contains an overlay. Frames
*without* an overlay must not move — a diff there is a real regression, not a
re-baseline.
