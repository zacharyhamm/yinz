# Modals and overlays: giving them their own collection

Working document for the `unify-modals-overlays` branch. Phases 0–7 have landed. Overlays are
their own collection, overlay-ness is a placement rather than a flag on coordinates, the
two-stage birth is gone with the whole of its compensation layer, and a pane is now either born
an overlay or is never one. Phases 5 to 7 are review findings on top of that: behaviors the
floating path had and the new collection did not, and generic code that could now reach an
overlay and did not expect to. Phase 7 is the last of that second kind, and all of it is one
sentence — "not floating" read as "tiled".

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

As of Phase 4 that holds everywhere. There is no promotion path left: `FloatingPaneCoordinates`
carries no `overlay` flag, so nothing can ask for one after the fact, and the last of the
compensation (`layer_visible_when_added`) went with it.

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

**What the unit suite could not see** (`0136c611a`). With all 2500 unit tests green,
`cargo x integration-test` put 18 failures on the board, every one of them a *plugin*
overlay: `file-finder`, `hocus-ask` and `quit-confirmation` drew an empty box. Terminal
overlays were fine throughout, which is exactly why nothing caught it — the unit suite's
plugin panes are mocks that never render, so the whole plugin-output path is untested
there.

The cause was `Tab::has_plugin`, which `Screen::handle_plugin_render_assets` asks before
handing a tab the bytes a plugin just rendered. It did not know about `overlay_panes`, so
every popup's output was logged as belonging to no pane and dropped. Four smaller sites had
the same shape: `Tab::set_force_render`, `rename_pane` (how a plugin titles its own pane),
`set_pane_active_at` (what the mouse handler calls after focusing one), and
`undo_active_rename_pane`, which picked its collection from whether the floating layer
happens to be visible.

**The lesson, which Phase 4 inherited:** a change to how panes are stored is not verified by
the unit suite. Run `cargo x integration-test`, and drive the real binary — the fastest
reproduction here was `zellij action new-pane --plugin zellij:about --floating --overlay`
against a live session, which showed an empty frame in one command.

### Phase 4 — born as an overlay or never one (commits `9a965891a`, `10feb5220`)

Two commits: the mechanical move, then the one behavior flip that could be separated from it.

Overlay-ness stops being expressible as a flag on coordinates and becomes only a placement.
`FloatingPaneCoordinates::overlay` and `as_overlay()` are deleted along with the `overlay` field
in all three `.proto` contracts; `Action::NewFloatingPane`, `NewFloatingPluginPane` and `EditFile`
carry a `NewPanePlacement` instead; plugins get `change_overlay_coordinates` and
`open_command_pane_overlay`; and `Action::Quit` states `OverlayPlacement::modal(None)` rather than
injecting a `modal` configuration key for the server to read back out.

With no way to ask for a promotion, the promotion branch of
`Tab::change_floating_pane_coordinates` goes, and with it the last of the compensation:
`FloatingPanes::layer_visible_when_added` and its two accessors, `ActivePanes::insert_silently`,
`OverlayPanes::adopt_focus`, and the `PromoteToOverlay` exemption from the property test.

Two behavior decisions landed here as well — overlays now outrank a UI-covering fullscreen tiled
pane, and fullscreen is refused on an overlay rather than falling through. Both are written up
under Phase 4 below, with what the plan got wrong.

`requests_overlay`/`requests_modal`/`RunPluginOrAlias::overlay_placement` stay: they are the
documented compatibility shim for the `overlay`/`modal` keys that keybindings, layouts and
third-party plugins still write, and they go in a later release once those are out of
circulation.

### Phase 5 — the two things that did not travel

Found by review rather than by the suite, which is the point: both are behaviors the floating
path had, that a reader of `OverlayPanes` alone would not notice were missing.

**`coordinates.borderless` was dropped on the way in.** `Tab::add_floating_pane` applies it to
the pane before handing it over, and `OverlayPanes::change_pane_coordinates` applies it too — but
`OverlayPanes::add_pane` only resolved the geometry, and `PaneGeom::adjust_coordinates` touches
x/y/cols/rows and nothing else. `set_pane_frames` then read the pane's untouched default and gave
the overlay a frame it had explicitly asked not to have. `Tab::toggle_pane_borderless` and
`Tab::set_pane_borderless` searched tiled, floating and suppressed panes only, so there was no way
to correct it afterwards either — and because `Screen` picks the tab with `has_pane_with_pid`,
which *does* know about overlays, the request found the right tab and then silently did nothing.

`add_pane` now applies the coordinate the way `change_pane_coordinates` does, `replace_pane`
carries the slot's borderless-ness onto the replacement instead of hardcoding `Offset::frame(1)`,
and `OverlayPanes::set_borderless`/`toggle_borderless` give `Tab` an overlay branch. Those two
update the *stored coordinates* as well as the pane: `placement_of` hands them over when an
overlay follows a client to another tab, so an overlay that only changed its own flag would get
its frame back on the way across.

**`GetPaneInfo` returned `None` for an overlay.** `Tab::pane_infos` gained its overlay branch
when the collection landed; the singular `Tab::get_pane_info` behind the plugin API did not, so a
plugin that opened an overlay command pane and asked about the ID it had just been handed got
nothing — where the same call worked while overlays lived in `floating_panes`. It now answers with
the same fields `OverlayPanes::pane_info` sets: `is_floating` true (read everywhere as "not
tiled"), `is_fullscreen` false.

### Phase 6 — the geometry an overlay starts from, and the stack that ate its members

Found by review, like Phase 5, and for the same reason: neither is a behavior the floating path
had and lost. Both are consequences of overlays now being *reachable* from generic code that was
written when they were not.

**An overlay that named no size was born one cell square.** `PaneGeom::adjust_coordinates` writes
only the fields the coordinates actually name — with `width`/`height` `None` it touches neither
`cols` nor `rows`, and with `x`/`y` also `None` it does not even centre. All three of
`OverlayPanes`' resolution points (`add_pane`, `change_pane_coordinates`, `reapply_coordinates`)
started from `pane.position_and_size()`, and a pane built for this path arrives with
`PaneGeom::default()`, whose `Dimension::default()` is `percent(100.0)` with an `inner` of **1**.
So a coordinate-less overlay was a 1×1 pane, and a framed one had no content area at all.

Not hypothetical: `RunPlugin::overlay_placement` returns `modal(None)` / `dismissible(None)` for
the `modal`/`overlay` configuration keys, `route.rs` does the same for the quit-confirmation
modal, and `default.kdl` binds `hocus-new-agent` and `hocus-duplicate-agent` with `modal true`
and no coordinates. Those mostly recovered only because — as `change_pane_coordinates`' own doc
comment says — `hocus-new-agent` re-asserts its placement afterwards. An overlay that does not
self-assert stayed 1×1. And the contract was already written down: `OverlayPlacement::coordinates`
is documented as "`None` centres it at a default size".

All three points now seed from `OverlayPanes::base_geom` — `half_size_middle_geom` over the
session surface, which is what a floating pane with no coordinates gets from
`find_room_for_new_pane`. Seeding at `add_pane` alone would have been enough for the reported
symptom; doing it at all three is what makes an overlay's geometry the pure function of its
coordinates and the surface that this module's own header claims it is. Two consequences, both
intended: a partial `change_pane_coordinates` now resolves the axis it did not name from the
surface rather than from a stale absolute (the call replaces the stored coordinates wholesale, so
that axis was gone from `overlay.coordinates` either way), and a modal taken back out of the
suppressed panes with no coordinates is re-centred rather than restoring its pre-suppression
geometry — consistent with the collection's deliberate absence of `desired_pane_positions`.

The one trap: `is_pinned` lives *inside* the `PaneGeom` rather than beside it, and
`Tab::add_overlay_pane` sets it on the pane before handing it over. Seeding without carrying it
across would have silently dropped a flag the caller explicitly asked for, so `base_geom` takes
the current geom and carries `is_pinned` and `logical_position` over. `stacked` is deliberately
left at `None`: an overlay does not tile, so it is never a member of a stack.

**An overlay accepted as a stack root ate every pane stacked onto it.**
`Tab::has_room_for_stack` rejected a floating or suppressed root but not an overlay one, and it
measures the root through `Tab::get_pane_with_id` — which, since Phase 3, resolves overlays. A
tall enough overlay therefore answered "yes, there is room". `Screen::stack_panes` then extracted
every member out of its tab into a local `Vec<Box<dyn Pane>>`, `TiledPanes::stack_panes` could not
find the root in the tiled grid, and `Tab::stack_panes` hit its `stack_geoms.is_empty()` early
return — dropping the vec that was by then the only thing holding those panes, while their pty
processes went on running unowned. Nothing upstream caught it either: the route-level guard rejects
only *global* panes, and `Tab::pane_id_is_floating` is floating-only.

`has_room_for_stack` now rejects an overlay root alongside floating and suppressed. That is the
load-bearing half, because `Screen::stack_panes` consults it *before* extracting anything, so the
refusal is a clean no-op rather than a salvage. The `stack_geoms.is_empty()` branch also parks its
panes now instead of dropping them, matching what the `add_pane_to_stack` arm directly above it
already did — the extraction is irreversible by the time that branch runs, so no future path that
gets past the gate should be able to close panes out from under their processes.


### Phase 7 — one mistake, spelled nine ways

Found by review, like Phases 5 and 6, and it is the same class again — but this time all of it is
one sentence:

> **"not floating" was read as "tiled", and "the floating layer is up" was read as "this pane is
> on it".**

Since Phase 3 an overlay answers `false` to `floating_panes.panes_contain` and to
`Tab::pane_id_is_floating`, while still being reachable from `get_active_pane_id`,
`get_pane_with_id` and `MouseHandler::get_pane_at`. Every surviving two-way `if floating { … }
else { … tiled … }` therefore routed overlay work onto the pane the overlay was covering — quietly,
because the overlay is drawn on top and the user sees nothing move where they are looking.

Nine sites, in four groups: the three cross-tab move paths (and the `accept_extracted_pane` that
receives from all of them), `Tab::resize` with `active_pane_is_flush_with_edge` behind it, the
nonvisual bell scan, and three in the mouse pipeline.

**Cross-tab moves demoted a modal.** `extract_pane` drops the kind and coordinates on the way out,
so the placement has to be read *before* it. Exactly one caller did that — `focus_plugin_pane` —
and the other three asked `pane_id_is_floating` and landed a modal in the destination as a tiled
pane. That is generalized now: `ExtractedPanePlacement { Tiled, Floating, Overlay(_) }` and
`Tab::placement_for_extraction`, which `accept_extracted_pane` takes in place of its
`was_floating: bool`, and which `break_multiple_panes_to_tab_with_id`,
`break_multiple_panes_to_new_tab` and `break_pane_to_new_tab` all read before extracting.
The overlay arm of `accept_extracted_pane` has no room check and sets no geom, both deliberately:
an overlay is not on a layer and does not tile, so there is nothing for it to run out of, and its
geometry is resolved from its coordinates against the *destination's* surface — which is the whole
point of carrying the placement rather than the pane's absolute position across.

A terminal modal is the honest test of this and the reason the tests use one: a plugin's
overlay-ness can be re-derived on the far side from its `modal` configuration key, so a plugin
modal survives a path that drops the placement entirely.

**Resize keystrokes resized what the overlay was covering.** `Tab::resize` is the two-way split in
its purest form; with a modal focused it fell into the `else` arm, dissolved the tab's stack lists
and resized the tiled pane behind. **Decided against implementing overlay resize.** `OverlayPanes`
has no `ResizeStrategy` API and deliberately no `desired_pane_positions`; an overlay's geometry is a
function of its coordinates and the surface, which is exactly the property Phase 6 finished
establishing, and `Tab::resize_pane_with_id` already dispatches on membership and no-ops for an
overlay id. The keyboard path now agrees with it and does nothing.

`Tab::active_pane_is_flush_with_edge` went with it, and is the sharper half: it excused only
*floating* active panes, and it is the sole input to
`Screen::resize_global_pane_from_adjacent_tab_pane`. An overlay that happened to reach an edge of
the surface let a resize keystroke aimed at nothing in particular resize a **global pane**.

**A bell inside an overlay was never heard.** `check_and_handle_bell_notifications` had its overlay
link; `check_and_consume_bells_without_visual_notification` — the branch taken when `visual_bell`
is off, and then the *only* thing that sees a bell — was the same chain minus that link. The BEL
was neither forwarded to the client nor consumed, so it stayed latched on the pane forever.

**Pressing a modal's frame dragged the pane underneath it, through two separate doors.** This is
the one worth remembering, because fixing the obvious door left the bug alive:

1. *Classification.* `should_start_moving` asks whether the floating **layer** is up, not whether
   the clicked pane is on it, and `execute_move_floating_pane` then throws the resolved pane id
   away and re-resolves the position through `FloatingPanes` alone. `ClickedPaneDetails` now
   carries `is_overlay` — not the negation of `is_floating`, which is what left the frame branches
   unable to tell an overlay from a tiled pane — and both frame branches skip the move/resize block
   for one, falling through to ordinary focus handling. `frame_intercepted` is still checked first:
   a plugin overlay's frame buttons must keep working. With the layer *down* the same press instead
   reached `start_pane_resize_with_mouse`, which looks in the floating and tiled collections only
   and errored the whole mouse event on an overlay id.
2. *Execution.* `focus_pane_at` already swallowed the click correctly — and it was not enough.
   When the pressed overlay is not the focused one the press classifies as an ordinary focus, and
   `execute_focus_pane` went on to call `move_pane_with_mouse` anyway, straight past the swallow.
   Gated now on `Tab::position_is_on_an_overlay`, which is the fact stated about the *position*
   rather than about a pane, because the floating collection's hit tests re-resolve a position from
   scratch and have no way to know an overlay was drawn over the answer they find.

Each door has its own test, and each was confirmed to fail with only the other one fixed. That
pairing is the finding: a swallow at the top of a pipeline does not protect a re-resolve at the
bottom of it.

**Focus-follows-mouse skipped overlays**, for the third spelling of the same sentence:
`execute_focus_on_hover` asked whether the hovered pane was on the visible floating layer, so with
the layer up, hovering a modal reduced to a hover and left the keyboard where it was.

Nothing new was added to `OverlayPanes` — no resize, no drag state, no room check. Every fix is a
caller learning that the collection exists, which is what the four deliberate absences in its
module doc predicted would be the shape of this work.

### Phase 8 — a live modal owns the keyboard and the top of the stack

Found by review. `OverlayKind::Modal` exists because such an overlay has no way to learn it lost
focus, and `Tab::new_pane` already honoured that for a *tiled* pane opened over one — it comes up
unfocused rather than stealing the keyboard. Nothing enforced the same for another **overlay**.
Three doors reached `OverlayPanes::focus_pane` / `focus_pane_for_all_clients` with a pane that was
not the modal: `Tab::add_overlay_pane`, `MouseHandler::focus_pane_at`, and `focus_pane_with_id`.
Both of those methods insert into `active_panes` *and* raise the pane, so a dismissible overlay
opening over a live modal took the keyboard *and* drew over it. If it then gave that focus up
(`defocus_pane`, from the tiled-pane path at `tab/mod.rs`), focus fell through the whole stack and
the modal was left on screen owning nothing.

**The rule now lives in the collection, not at the three call sites.** `may_be_focused` is false for
a non-modal while any modal is open, and `focus_pane`/`focus_pane_for_all_clients` return early on
it. One guard closes all three doors, which is why none of them needed a change.

**Two decisions, both deliberate:**

- **Newest modal wins.** The guard exempts modals, so a second modal does take focus and the top of
  the stack. It has to: `quit-confirmation` and `hocus-new-agent` are both `Modal`, and a quit
  prompt raised over an open modal would otherwise come up unfocused with no way to close itself.
  Nothing new was needed to unwind it — `move_clients_out_of_pane` already walks focus down to the
  modal underneath when the top one closes.
- **`z_indices` is partitioned: every modal above every non-modal**, plain raise order within each
  class, via the new `raise_within_class` replacing the three open-coded `retain`/`push` pairs in
  `add_pane`, `focus_pane` and `focus_pane_for_all_clients`. Blocking focus alone was not enough:
  `add_pane` pushes to the top unconditionally, so the modal would have kept a keyboard the user
  could not see it holding. This also makes `topmost_modal_pane_id` literally the top of the stack
  rather than merely the thing that outranks it, which is what its own doc comment already claimed.

**Explicitly not done, and each is a decision rather than an omission:**

- **No pending/deferred-overlay queue.** The review suggested "defer the new overlay". That means
  state this collection does not have — a queue, an ordering policy, a drain point, and an answer
  for a modal that never closes. The four deliberate absences in the module doc say new state here
  is a signal the concept is wrong; the tiled precedent is "open it, don't focus it", and that is
  what this does.
- **`defocus_pane` still falls all the way through** rather than landing on a live modal. It is the
  other half of the stranding path and belongs to its own change.

**On the tests.** Each half was re-run with only itself reverted, as Phase 7 did. It matters here:
neutering the focus guard fails two tests, neutering the z-order partition fails four, and only one
test fails under both. A suite green after fixing one half of a two-half bug is not evidence.
`replacing_a_pane_keeps_its_slot_its_kind_and_its_coordinates` moved — it added a modal then a
dismissible, so the replaced slot became the top of the stack and "kept its slot" stopped being
distinguishable from "was pushed on top". A second modal above it restores the distinction.

One snapshot is new
(`an_overlay_opened_over_a_modal_takes_neither_the_keyboard_nor_the_top_of_the_stack`); none moved.

### Phase 9 — the keyboard follows the pane the user just asked for

Two review findings, both in the same seam: `Tab::get_active_pane_id` prefers an active overlay
over the floating and tiled layers, so overlay focus is not one opinion among three — it is the
answer. Anything that leaves stale focus in `OverlayPanes` therefore silently overrides every
other layer.

- **A focused floating pane opened over a dismissible overlay came up behind it.** Phase 7 taught
  the *tiled* path in `Tab::new_pane` to make a dismissible overlay give the keyboard up; the
  floating path never learned it. `add_floating_pane` raised and focused `floating_panes` and left
  `overlay_panes.active_panes` alone, so `new-pane --floating` landed behind the popup, input kept
  going to the popup, and the popup — which learns to close by seeing itself unfocused — never
  found out.
- **`FocusPaneWithId` could focus a non-selectable overlay.** `Tab::set_pane_selectable` marks the
  pane and moves clients off it, but `may_be_focused` only asked about modals, so a later focus
  request re-inserted it as active: input routed to a pane that had explicitly asked for none, and
  the tab's own invariant that active panes are selectable broken.
  `FloatingPanes::focus_pane` has always refused this; the overlay twin now does too.

**Both fixes are one line of policy each, placed in the collection rather than at the call sites**,
which is the shape Phase 8 settled on. `may_be_focused` gained the selectability clause — that
covers `focus_pane`, `focus_pane_for_all_clients`, `focus_pane_if_exists` and the mouse path
without any of them changing — and `focus_pane_for_joining_client`, which bypassed the guard
entirely, now routes through it as well.

The dismissible release became `defocus_pane_if_dismissible` (plus `defocus_dismissible_panes` for
the all-clients paths), replacing the hand-rolled `active_pane_id` + `is_modal` + `defocus_pane`
match that Phase 7 left inline in `new_pane`. It returns whether the keyboard is free, so the two
call sites read as the rule rather than re-deriving it. It went into `add_floating_pane` rather
than `new_floating_pane`, where review pointed: that is where the focusing actually happens, it is
the funnel every entry point reaches, and `new_floating_pane` passes no `client_id` at all — the
all-clients form was needed regardless.

**Explicitly not done** — *closed by Phase 10, below.* The same gap exists on
`MouseHandler::focus_pane_at` (clicking a floating or tiled pane under a dismissible overlay) and
on the keyboard focus-move family (`focus_next_pane`, `move_focus_left/down/up/right`, …). Neither
is one of these findings, and both are a different question — *moving* focus rather than *opening*
something with it. The new helper is the piece they would reuse.

**On the tests.** Three added, in `screen_tests/overlays.rs`. Each of the two bug-pinning ones was
re-run with only its own fix reverted and failed, as Phases 7 and 8 did; the modal counterpart
(`a_focused_floating_pane_opened_over_a_modal_leaves_the_keyboard_alone`) passes either way by
design — it guards the half of the rule that was already right. No snapshots added or moved: this
changes who holds the keyboard, not how anything is drawn.

**Run figures.** `cargo x test`: 2542 passed in `zellij-server`, 0 failed, whole suite green.
`cargo x integration-test` at default parallelism reproduced the contention collapse this document
already warns about (112 timeouts); RUNFIGURES `global_panes_focus` was checked the way the note
above prescribes — 8 runs alone on this tree (5 pass, 3 fail) against 8 runs alone on a clean HEAD
worktree (3 pass, 5 fail). Same both-outcomes behaviour on both trees, so not a regression.

### Phase 10 — moving focus, and a tab that travels without its overlays

Two review findings, unrelated to each other except in their cause: generic code that used to
reach overlays through `floating_panes` now either reaches them where it should not, or misses
them where it should.

**A warped tab stranded its overlay's child.** `check_tab_can_be_warped` builds the payload from
`tab_panes_for_layout`, which walks the tiled and floating collections — an overlay has no layout
slot, so its supervisor was never released. But `empty_and_close_tab` swept
`get_static_and_floating_pane_ids`, which Phase 4 taught to chain the overlays, and every close in
that function deliberately says nothing to the pty thread because those children belong to the
other session now. So the overlay was taken out of the tab *before* `close_tab_by_id` could name
it to the pty thread: the far end never received it, and this end left a child attached to nobody.

**The decision was to ignore overlays rather than carry them.** A terminal overlay cannot be
expressed in a layout at all — overlay-ness is re-derivable only from a plugin's configuration,
which is the same reason `placement_for_extraction` has to be asked before a pane leaves — so
there was no version of "carry it" that did not invent a wire format for one popup. It stays with
the source session and closes with the tab like anything else the tab still holds.
`Tab::tiled_and_floating_pane_ids` is the new accessor that says which panes a tab travels as;
`get_static_and_floating_pane_ids` keeps the overlays for its other callers. The same accessor
fixes the guard above it: "this tab has no panes with a process to move" was asked of
`get_all_pane_ids`, so a tab whose only terminal was an overlay passed it and then warped with an
empty payload.

**A dismissible overlay kept the keyboard when a lower layer took focus.** This is the gap Phase 9
recorded as explicitly not done. `get_active_pane_id` prefers an active overlay over every other
layer, so `MouseHandler::focus_pane_at` and the focus-move family — which updated
`floating_panes`/`tiled_panes` alone — left a stale overlay entry masking whatever they focused:
input went on reaching the popup, and a dismiss-on-defocus popup never learned it had lost the
keyboard.

**The rule has three parts, and the third is the one that is easy to miss:**

- **A modal makes the move a no-op.** It owns the keyboard until it closes, so focusing what it
  covers would rearrange focus where the user can neither see nor act on it — and leave the layer
  underneath pointing somewhere else for when the modal finally goes. `lower_layer_may_take_focus`
  is the question (`OverlayPanes::a_modal_holds_focus` under it); the focus-move family returns
  its own "nothing moved" value, and the mouse path swallows the click.
- **A dismissible overlay gives the keyboard up**, via `release_overlay_focus_to_lower_layer` —
  Phase 9's `defocus_pane_if_dismissible`, reused exactly as that phase predicted.
- **Only once the move has actually happened.** The release is *after* the move, and conditional
  on it. Released on the way in, a focus-move that found nothing in the direction asked for would
  dismiss the popup the user was reading and drop the keyboard onto the pane behind it — a dead
  keypress closing something. The existing
  `tab_integration_tests::floating::overlay_does_not_show_hidden_floating_panes` caught exactly
  this, which is the second time this document can record a pre-existing test paying for itself.

**`focus_pane_with_id` deliberately takes the other shape.** It is an explicit "focus *this* pane",
so it belongs with Phase 9's opening paths rather than with the focus-move family: a dismissible
overlay gives the keyboard up, a modal keeps it, and either way the focus underneath lands where it
was asked to — as `add_floating_pane` already does. It fires only for a pane that is not itself an
overlay (or a request to focus one overlay would defocus another on the way in) and only for a pane
this tab actually holds (a request naming a pane that is not here focuses nothing, and should
dismiss nothing).

**One test moved.** `an_overlay_masks_each_client_s_tiled_focus_until_it_closes` pinned masking for
a *dismissible* overlay, which is the behavior this phase changes; it is now
`a_modal_masks_each_client_s_tiled_focus_until_it_closes`, keeping the property for the kind whose
property it actually is, with
`focusing_a_pane_by_id_under_a_dismissible_overlay_takes_the_keyboard_from_it` for the other half.

**On the tests.** Nine added — six in `screen_tests/overlays.rs`, three in
`screen_tests/scroll_and_warp.rs`. Each half of each fix was re-run with only itself reverted, as
Phases 7–9 did, and each time exactly the tests that name it failed: the `empty_and_close_tab`
sweep (`warping_a_tab_closes_the_overlay_it_leaves_behind` — nothing closed the overlay at all),
the warp guard (`a_tab_whose_only_process_is_an_overlay_has_nothing_to_warp` — fell through to the
layout builder's less useful complaint), the release (the three dismissible focus tests), the
modal guard (both modal tests), the by-id release (its own test), and the release *ordering*
(`a_focus_move_that_goes_nowhere_leaves_a_dismissible_overlay_holding_the_keyboard`, plus the
pre-existing floating test). `a_warped_tab_leaves_its_overlay_behind` passes either way by design —
it guards the half that was already right. No snapshots added or moved: this changes who holds the
keyboard and what a tab travels as, not how anything is drawn.

**Run figures.** `cargo x test`: 2553 passed in `zellij-server`, 0 failed, whole suite green.
`cargo x integration-test -- -j 4`: RUNFIGURES.

---

## Findings that the remaining work depends on

Discovered while writing Phase 0. Each was pinned by a test asserting the pre-refactor
behavior. **1–4 and 9 were resolved by Phase 3, and 7, 8 and 10 by Phase 4**; 5 and 6 still
describe the code. 11 and 12 were found by review after Phase 4 and are resolved.

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

7. **RESOLVED (Phase 4). `change_floating_panes_coordinates` stays** — it is used for ordinary
   resizing by `compact-bar`, `multiple-select`, `about` and `hocus/tools.rs`. What went away is
   the `.as_overlay()` promotion, and an overlay resizing itself now has its own command,
   `change_overlay_coordinates`.

8. **RESOLVED (Phase 4). `hocus-new-agent` re-asserts its geometry on every resize**, so
   overlay-ness has to stay sticky across coordinate changes. It holds by construction:
   `OverlayPanes::change_pane_coordinates` leaves the stored `kind` alone, and since Phase 4 the
   plugin cannot say anything about overlay-ness in the call at all.

9. **RESOLVED by construction. `layout_applier.rs:869,944` drained `floating_panes`** for
   swap layouts, sweeping overlays into relayouts. Those drains no longer see an overlay,
   which also fixes the silent demotion of an `--overlay` *terminal* pane — it had no
   plugin configuration for `add_floating_pane` to re-derive overlay-ness from.

10. **RESOLVED (Phase 4), against the old behavior. An overlay used not to be drawn under a
    UI-covering fullscreen tiled pane**, and focus followed it away. That left a modal focused
    and invisible, which the user cannot get out of, so overlays now outrank fullscreen the way
    they outrank a pinned pane. `Tab::overlays_are_rendered` is the single gate and is now just
    `self.overlay_panes.has_panes()`. Pinned by
    `an_overlay_outranks_a_ui_covering_fullscreen_tiled_pane` and the new snapshot
    `frame_of_an_overlay_over_a_ui_covering_fullscreen_tiled_pane`.

11. **RESOLVED. A client that joined after an overlay opened had no overlay focus at all.**
    `focus_pane_for_all_clients` snapshots `connected_clients` at the moment the overlay opens, so
    a client attaching - or merely switching into the tab - afterwards fell through
    `Tab::get_active_pane_id` to the pane behind the modal: it saw the dialog drawn over its whole
    surface and typed underneath it, and a modal has no way to learn that. `Tab::add_client` now
    initializes overlay focus first, before the branch that returns early for a tab with no tiled
    panes: **the topmost selectable modal if there is one, otherwise whatever overlay another
    client is already on**. Both doors are covered, since tab switching reaches `add_client`
    through `move_clients_between_tabs`. The second half of the rule matters as much as the first -
    a dismissible overlay that gave its focus up reads as "nobody is on it" and stays given up,
    rather than being resurrected for the joining client.
    `OverlayPanes::focus_pane_for_joining_client` deliberately leaves `z_indices`, `last_panes` and
    `set_active_at` alone: the occlusion stack is shared, and joining is not activity. Pinned by
    `a_client_joining_while_a_modal_is_open_lands_on_the_modal`,
    `a_client_switching_into_a_tab_with_a_modal_lands_on_it` and
    `a_joining_client_does_not_pick_up_a_dismissible_overlay_that_gave_up_focus`.

12. **RESOLVED. Scrolling a terminal overlay moved the pane underneath it.**
    The scroll path resolved the position under the cursor twice.
    `MouseHandler::get_pane_at` hit-tested the overlay collection first and correctly returned the
    overlay, but the escape sequence it generated - an SGR mouse report, or the cursor keys an
    alternate screen scrolls with - was handed back as a *position* to
    `Tab::write_to_terminal_at`, which resolved it again through the floating and tiled layers
    only. It had never heard of overlays, so it delivered to the tiled pane beneath: the user
    scrolled a dialog and a shell behind it moved. The two resolvers also disagreed about pinned
    panes and about a floating layer that is down.
    Fixed by not resolving twice. The scroll handlers already hold the pane, so they take its
    `pid()` and go through `MouseHandler::write_to_pane` -> `Tab::write_to_pane_id`, and
    `write_to_terminal_at` - whose only callers those were - is gone with the second resolver
    rather than being taught about overlays. Pinned by
    `scrolling_a_terminal_overlay_in_alternate_mode_writes_to_the_overlay` and
    `scrolling_a_terminal_overlay_that_tracks_the_mouse_writes_to_the_overlay`, one per branch.

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

## Phase 4 — born-as-overlay, plugin migration, cleanup (**landed**; see Done, above)

The plan that used to be here has been executed. What is worth keeping is where the plan's
premise was wrong, and the four decisions taken while carrying it out.

**The premise was already half-true.** The plan said the in-repo popup plugins "still open
floating and promote themselves". They did call `.as_overlay()` from `load()`, but every one of
them is launched with `overlay`/`modal` in its configuration
(`zellij-utils/assets/config/default.kdl`, plus the key `route.rs` injected for
`quit-confirmation`), so `AddPlugin` had already resolved them to `NewPanePlacement::Overlay` and
they were *already* born into `OverlayPanes`. Their `.as_overlay()` calls hit the "already an
overlay" early return, not the promotion branch. The promotion branch's real users were
third-party plugins and `zellij action change-floating-pane-coordinates --overlay` — and Phase 4
deliberately stops supporting both.

**Deliberate user-visible removal:** `zellij action change-floating-pane-coordinates` loses its
`--overlay` and `--modal` flags. A floating pane cannot become an overlay any more; open one as an
overlay instead (`zellij action new-pane --overlay` / `--modal`). This is the point of the phase
rather than a casualty of it: overlay-ness decided after birth is the bug class the whole refactor
exists to make unrepresentable.

### The four decisions

1. **Finding 10, resolved against the old behavior.** *All* overlays, not only modals, now draw
   over a UI-covering fullscreen tiled pane and keep focus. Hiding them left a modal focused and
   invisible — a state the user cannot get out of, because the invisible thing is what has the
   keyboard. `Tab::overlays_are_rendered` is now just `self.overlay_panes.has_panes()`.
2. **Fullscreen is refused on an overlay, on both paths.** `Tab::refuses_fullscreen` covers the
   by-id ones, which used to log "no tiled pane with id" — a lookup failure, not a refusal — and
   `active_pane_refuses_fullscreen` covers `toggle_active_pane_fullscreen`/`_no_ui_`, which never
   consulted `get_active_pane_id` and so silently fullscreened the tiled pane *behind* the overlay
   the user was looking at. That was a live bug, not a tidy-up. `unset_fullscreen` needed nothing:
   an overlay can never be the fullscreen pane it would clear.
3. **The wire contract was broken outright.** The `overlay` field is deleted from all three
   `.proto` contracts rather than reserved or shimmed. A plugin built against an older
   `zellij-tile` that sets it silently loses overlay-ness.
4. **A dedicated `open_command_pane_overlay`**, matching the existing `_floating` / `_in_place`
   family, rather than a general placement-carrying open shim.

### What the plan did not anticipate

**`NewPanePlacement` had to replace bare coordinates on three actions, not two.**
`Action::NewFloatingPane`, `Action::NewFloatingPluginPane` *and* `Action::EditFile` all carried
overlay-ness as a bool on their coordinates, so `zellij action edit --floating --overlay` would
have regressed silently. All three now carry a `placement: NewPanePlacement`, with
`Action::NewBlockingPane` as the precedent. This also fixed a live gap: the
`--plugin --floating --modal` path flattened to a bare bool and lost the modal *kind*, which only
came back if the plugin's own configuration happened to say `modal`.

**An `Option<OverlayPlacement>` had to be threaded through the plugin-load round trip** —
`ScreenInstruction::LaunchPlugin` → `PtyInstruction::FillPluginCwd` → `PluginInstruction::Load` →
`ScreenInstruction::AddPlugin` — so that a launcher can state overlay-ness without going through
the configuration shim. At `AddPlugin` a stated placement wins over the one
`RunPluginOrAlias::overlay_placement()` derives from configuration. It is the last parameter of
each instruction rather than beside the coordinates, which kept the call-site sweep to appending
one argument.

**`ActivePanes::insert_silently` and `OverlayPanes::adopt_focus` died with the promotion.** Both
existed to carry focus across a collection migration; there is no migration left, so they had no
callers and are gone.

### Coverage that moved

- `Op::PromoteToOverlay` is gone from the property test, and with it the last exemption from
  invariant 3's transition half. Nothing is exempt now.
- `a_floating_pane_that_becomes_an_overlay_takes_the_floating_layer_down_with_it` and
  `a_pane_that_becomes_an_overlay_lands_where_a_pane_born_an_overlay_already_is` described a
  path that no longer exists. The first became
  `opening_an_overlay_does_not_raise_the_floating_layer`, which is the guarantee the user
  actually has; the second had nothing left to compare.
- `overlay_focus_tracks_whether_the_overlay_is_rendered` became
  `an_overlay_outranks_a_ui_covering_fullscreen_tiled_pane`, asserting decision 1.
- New: `frame_of_an_overlay_over_a_ui_covering_fullscreen_tiled_pane`. This combination had no
  snapshot because the overlay was simply not drawn, and it is the sharpest test of the flat-list
  discipline — a fullscreen tiled pane is the one case where the floating half of
  `occlusion_stack` is not drawn while the overlay half is. An empty box is the failure it pins.
- New: `an_overlay_cannot_be_fullscreened_by_id` and
  `a_fullscreen_keystroke_on_an_overlay_leaves_the_pane_behind_it_alone`, for decision 2.
- `zellij-integration-tests/tests/floating_pane_overlay.rs`: `open_self_upgrading_popup` became
  `open_popup`, opening `zellij:quit-confirmation` as a modal with the geometry the plugin used to
  set on itself. The assertions did not change — the user-visible guarantee is the same one; only
  the mechanism it rides on is gone.

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
  suite produced ~105 timeouts purely from contention; the same tests pass at `-j 4`. Even
  at `-j 4` a handful still time out at 60s (`no_focus`, `no_ui_fullscreen`) and pass when
  run alone — check a timeout in isolation before believing it.
- **Pre-existing failures**, confirmed by stashing the branch and re-running:
  `misc::quit_with_keybinding`, the `nested_guest_modal` / `nested_hardening` /
  `nested_sessions` / `nested_session_handling_config` group, and
  `stacked_pane_list::a_stack_list_survives_session_resurrection`. Not caused by this work.
  `quit_with_keybinding` is worth understanding rather than ignoring: it sends Ctrl-q and
  waits for "Bye from Zellij!", but this fork put a quit-confirmation modal in the way, so
  the test is stale against the fork's own feature.
- **The `zellij-server` lib suite is flaky under parallel load, and the failing set changes
  from run to run.** Four runs on the same tree produced four different one- or two-test
  failure sets, drawn from
  `screen_tests::global_panes_focus::no_new_pane_reaches_the_global_area_through_the_screen_thread`,
  `plugins::plugin_tests::keybinds::mode_update_payload_is_lightweight_for_opted_in_plugins`
  and `screen_tests::cli_actions::send_cli_scroll_up_action`. `global_panes_focus` also fails
  on a stashed (pre-branch) tree — so this is not a regression.

  **`global_panes_focus` is not merely load-sensitive; it is racy on its own.** Run alone five
  times against one unchanged binary at the end of Phase 4 it failed three times and passed
  twice, always with the same two-row shift in the "from anchor" box. Same binary, both
  outcomes, is the proof: a regression fails deterministically. So "check the named test alone"
  is not sufficient for *this* test — run it several times, and compare the failure to the
  snapshot diff above rather than to a pass/fail count. It touches no overlays.

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

- **The Phase 4 full run, for reference.** `cargo x test`: 2502 passed, 1 failed
  (`mode_update_payload_is_lightweight_for_opted_in_plugins`, green alone).
  `cargo x test --no-web`: green. `cargo x integration-test -- -j 4`: 324 tests, 305 passed,
  19 failed - exactly the pre-existing set named above, nothing else. `build`, `format --check`
  and `assets --check` clean. Note the integration suite took ~52 minutes at `-j 4`.

**Snapshots expected to change legitimately:** the config snapshots under
`zellij-utils/src/kdl/snapshots/` if KDL serialization shifts, and any
`screen_tests`/`tab_integration_tests` snapshot whose frame contains an overlay. Frames
*without* an overlay must not move — a diff there is a real regression, not a
re-baseline.

Phase 4 moved three: two `ScreenInstruction` dumps that gained the new trailing
`Option<OverlayPlacement>` field
(`send_cli_launch_or_focus_plugin_action`,
`send_cli_launch_or_focus_plugin_in_place_with_close_replaced_pane`) and the new
`frame_of_an_overlay_over_a_ui_covering_fullscreen_tiled_pane`.

Phase 7 moved none: it changes what happens to an overlay, never how one is drawn.

**A note on how Phase 7's tests were validated.** Every fix in that phase had its test re-run
with only that fix reverted, and each failed. This is worth doing rather than assuming, and the
mouse pair is why: with the classification fixed but `execute_focus_pane` still leaking, the
first drag test passed. It takes a *second* test - one where the pressed overlay is not the
focused one, so the press classifies as a focus rather than a selection - to reach the second
door at all. A green suite after fixing the obvious half of a two-door bug is not evidence.
