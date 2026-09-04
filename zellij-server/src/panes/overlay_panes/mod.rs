//! Panes drawn over the whole session surface, above a tab's own panes and chrome.
//!
//! An overlay is *not* on a tab's floating layer. It is a pane the session puts in front of
//! everything - `file-finder`, `quit-confirmation`, `hocus-new-agent`. A modal
//! (`OverlayKind::Modal`) is an overlay that must not have focus taken from it, because it has no
//! way to learn that it lost it.
//!
//! Until this collection existed an overlay was born as an ordinary floating pane and promoted
//! afterwards, so overlay-ness was the presence of a key in `FloatingPanes::overlay_coordinates`
//! and every caller that rebuilt a pane's coordinates dropped it. Here a pane's `OverlayKind` and
//! the coordinates it resolves against sit beside the pane itself, so neither can be lost.
//!
//! **A live modal owns the keyboard and the top of the stack until it closes.** `z_indices` is
//! partitioned - every modal above every non-modal, plain raise order within each class - and
//! `focus_pane`/`focus_pane_for_all_clients` decline to move focus onto a non-modal while a modal
//! is open (`may_be_focused`, which also refuses an overlay the tab has marked non-selectable).
//! Both halves are needed: blocking focus alone would leave the modal holding a keyboard the user
//! cannot see it holding, underneath whatever opened last. The one exception is another modal, which does take both, because closing it walks focus back down to
//! the modal underneath (`move_clients_out_of_pane`) - without that a quit-confirmation opened
//! over an already open modal would come up unfocused and unable to close itself.
//!
//! Deliberately absent, and each absence is load-bearing rather than an oversight:
//!
//! - **No `show_panes`.** There is still no *layer* to raise or lower: the entire bug class this
//!   collection was made to remove came from code that read "floating layer down" as "nothing on
//!   it is focused", and that reading has nowhere to live here.
//!
//!   What does exist, and did not when this list was written, is **per-pane suppression**
//!   (`hide_pane`/`unhide` via `show_pane`) - a different thing, and added openly per the note at
//!   the end of this list. It names one overlay; every other overlay carries on being drawn. It
//!   exists because a sidebar-shaped overlay has to go away on Esc *without unloading*: hocus
//!   holds a live conversation, so closing it is not the same offer as hiding it.
//!
//!   The reason it cannot recreate the old bug class is that it never leaves a pane hidden and
//!   focused: `hide_pane` moves clients off the pane and drops any drag holding it, in the same
//!   breath that sets the flag, and `may_be_focused` refuses a hidden pane afterwards. So "not
//!   drawn" and "not focused" cannot drift apart - which is exactly what "floating layer down"
//!   allowed. `stack_layer_ids` is the single point the flag is read, so everything
//!   visibility-shaped (render, occlusion, hit-testing, focus targets) inherits it, while
//!   everything identity-shaped (`panes_contain`, `get_plugin_pane_id`, `placement_of`,
//!   `reapply_coordinates`) deliberately does not: a hidden overlay is still an overlay that a
//!   `LaunchOrFocusPlugin` has to be able to find, and still one whose geometry has to keep up
//!   with the surface so it comes back where it belongs rather than where it last was.
//! - ~~**No `fullscreen_pane_id`.**~~ This was on the list, as "what a fullscreen overlay would
//!   mean is undefined - an overlay is drawn over the whole session surface already". It is here
//!   now, as `fullscreen`, and - per the note at the end of this list - said out loud rather than
//!   added quietly. What was undefined was only ever *size*, and an overlay has one: hocus's
//!   panel is a corner-sized window onto a conversation, and wanting it to fill the screen for a
//!   minute and then go back is the same wish a tiled pane's fullscreen answers. `Ctrl+p f`
//!   expands an overlay over the tab's viewport - the chrome stays, exactly as it does for a
//!   tiled pane - and `Ctrl+p Shift+f` over the whole surface.
//!
//!   **It is a geometry override, not a placement**, and that is what keeps the sentence above -
//!   geometry is a function of coordinates and the surface - true of an expanded overlay. The
//!   coordinates, the user's claims on them and the link to a partner are all untouched
//!   underneath, so there is nothing to restore from a memory of its own: dropping the override
//!   *is* the way back, and the way back is therefore exact even if the surface changed size or
//!   the owning plugin re-placed the overlay while it was expanded. The one place this shows is
//!   `pane_info`, which reports the rectangle *underneath* an expansion together with
//!   `is_fullscreen` - the same split `hidden` makes between what a pane is and what is painted,
//!   and load-bearing here: hocus takes its panel's size from the manifest, and would otherwise
//!   answer an expansion by re-placing the panel at the size of the screen, overwriting the very
//!   rectangle the expansion exists to come back to.
//!
//!   The upkeep is one function, `refresh_fullscreen`, called from `set_pane_frames` - which
//!   every path that moves, resizes, places or replaces an overlay already ends in. So a terminal
//!   resize, a placement batch, a re-cut divider and a pane swapped into the slot all keep the
//!   expansion filling the screen without knowing it exists, and none of them can leave a stale
//!   override behind, because that one function owns every override in the collection. A linked
//!   pair is expanded as a panel, divider at its share, as it is dragged and resized as one.
//!
//!   Unlike the floating layer's, this fullscreen does **not** filter `stack_layer_ids`: a live
//!   modal must still draw above an expanded overlay, and the identity between that list and
//!   `render` is what this module stakes its "cannot drift" claim on. The expanded geometry
//!   occludes what is below it through `layers`, which is the whole job. A **modal** cannot be
//!   expanded, for the reason it cannot be resized. And an expansion is not carried across tabs -
//!   see [`OverlayCarry`], which carries what a pane *is*, where being expanded is where the user
//!   is looking.
//! - **No `desired_pane_positions`.** A floating pane remembers where the user dragged it in a
//!   second map beside the panes. An overlay has no such history *beside* itself: a mouse drag
//!   rewrites the overlay's own stored `coordinates` (`move_pane_to_position`), and so does a
//!   resize (`resize_pane_with_strategies`), so where the user put it and how big they made it
//!   are expressed in the same terms as where it was opened, and `reapply_coordinates`
//!   re-resolves a dragged or resized overlay exactly as it re-resolves an untouched one. That is
//!   what keeps the sentence above - geometry is a function of coordinates and the surface - true
//!   of an overlay the user has handled.
//!
//! `pane_being_moved_with_mouse` used to be on this list, as "overlays are not user-draggable",
//! and so did resize, as "an overlay is moved by the user, never resized by one". Both are here
//! now, and both for the same reason: the user asked to get a popup out of the way of what it is
//! covering, and moving it is only half of that. Nothing else about the concept moved. Neither
//! gesture changes focus (so a modal keeps the keyboard throughout) and both edit the overlay's
//! *coordinates* rather than introducing a geometry remembered anywhere else, which is what keeps
//! the sentence above - geometry is a function of coordinates and the surface - true of a
//! draggable, resizable overlay.
//!
//! What they add is a claim on those coordinates. A dragged overlay's position is the user's
//! (`position_is_user_stated`) and a resized overlay's size is (`size_is_user_stated`), so
//! `change_pane_coordinates` stops taking a plugin's answer for whichever half the user has
//! stated. Without that, an overlay whose plugin re-places itself on every `TabUpdate` is
//! immovable and looks it: the gesture reports session state, session state is a `TabUpdate`, and
//! the plugin answers each step by putting the overlay back. The two claims are independent - an
//! overlay that was only dragged still takes a plugin's size.
//!
//! The resize has one cost, and it is worth stating rather than discovering: the new size is
//! written back as `Fixed`, so an overlay the user has sized stops growing with the screen. That
//! is the same trade the drag already makes for position, and it is what "the user said so" has
//! to mean if it is to survive a re-resolution at all.
//!
//! There are three ways in, and they all end at `resize_pane_with_strategies`: the keyboard
//! (`Tab::resize`), a ctrl-drag of a corner, and ctrl+MouseScroll - which the frame has always
//! advertised, since an overlay's frame is drawn as a floating one.
//!
//! **Two overlays placed together as one panel are linked**, and the seam between them is a
//! fourth gesture that is deliberately *not* a claim. hocus draws its agent list and its
//! conversation as two panes that butt together, and the two touching frame columns read as a
//! divider; a plain left drag on that divider (`seam_at`, `drag_seam_to`) moves it, growing one
//! half by exactly what the other gives up and leaving the panel's outer edges alone. Which
//! pairs count is stated explicitly rather than inferred from adjacency: a pair is linked when
//! one `change_overlay_coordinates` batch places both panes and they land edge to edge
//! (`link_touching`, called by `Tab` once the batch is applied), and the link is dropped when
//! either pane is removed or the two are placed apart again. Two unrelated overlays the user
//! dragged next to each other keep moving on a frame drag, because nothing placed them together.
//!
//! **A linked pair is one panel to every gesture and every placement.** A frame drag on either
//! half carries the other with it (`move_pane_to_position`). A ctrl-resize of either half is a
//! resize of the panel. A *directed* one - a ctrl-drag of an edge or corner, a keyboard resize
//! with a direction - names an edge of the half (`resize_half_of_panel`): an outer or a
//! vertical edge grows or shrinks the panel, the other half's rows keeping in step, while the
//! seam-side edge is the seam, so moving it moves the divider - the other half gives up exactly
//! what this one takes, down to the floor a seam drag keeps - and the panel's outer edges stay.
//! An *undirected* one - ctrl+MouseScroll, a keyboard resize with no direction - names no edge,
//! so it is the panel's bounding box on all four sides, whichever half it landed on, with the
//! seam kept at its share of the width (`resize_panel`). The claims those gestures make -
//! `position_is_user_stated`, `size_is_user_stated` - are made on both halves and read off the
//! panel's bounding box, because they are claims about the panel; a resize that only moved the
//! seam claims nothing, like the seam drag below. A placement batch that names both halves is then applied to the panel
//! rather than to each half (`place_linked_pair`): the batch's two rectangles describe a
//! *layout* (which half has which share, where the divider is, whether one half has the whole
//! panel), and that layout is mapped onto the rectangle the user's gestures left the panel in,
//! axis by axis, as a single overlay keeps a dragged `x`/`y` and a resized width/height. This is what
//! lets the owning plugin keep computing both halves from one panel rectangle: it never has to
//! know where the user dragged the panel to, and a claimed `x` on one half can never disagree
//! with a fresh width on the other.
//!
//! A link survives a half being hidden. The hidden half is still placed by every batch (the
//! owning plugin keeps it at the share it will come back to), the pair's bounding box is still
//! the panel, and the reveal has to find the pair to map its layout onto that panel; only
//! `seam_at` refuses a hidden half, since a divider needs two panes to divide. A batch that
//! places a pair with one half hidden links them when the hidden share lies inside the shown
//! half, which is the one shape that placement has.
//!
//! The seam drag writes both halves' `x`/`width` back as `Fixed`, as a resize does, but sets
//! neither `position_is_user_stated` nor `size_is_user_stated`. The split is the owning plugin's
//! to interpret - hocus reads it back off the manifest and re-derives the panel from it on the
//! next placement - and a claim here would make the next placement keep the panel's size where
//! the plugin was asking for a re-cut of it.
//!
//! A **modal is not resizable** (`is_resizable`). It is drawn because something needs an answer
//! now, its content is sized to that question, and there is no case of it sitting over something
//! the user is trying to read. It is still draggable, like any other overlay. Because it cannot be
//! resized, it also does not say it can: `render` withholds the frame's help text from a modal,
//! that text being nothing but the resize hint.
//!
//! Neither claim is placement, and neither survives a move on its own: `extract_pane` drops the
//! wrapper holding them, and a plugin that re-places itself would answer the move by putting the
//! overlay back at its configured position and size. So both ride in [`OverlayCarry`], read from
//! the source tab before the extraction and handed back to `add_pane`. That is the answer to the
//! question this comment used to leave open - no `OverlayPlacement` field, and so no proto change,
//! because none of these bits is placement.
//!
//! If one of the remaining absences turns out to be needed, that is a signal that the concept is
//! not as clean as assumed. Say so rather than adding it quietly. Two have been needed so far -
//! per-pane suppression and fullscreen - and the two struck-through bullets above are that
//! saying-so. The concept survived both, and in each case only because the new thing turned out
//! to be nameable in the terms the module already had: suppression as one pane with its focus and
//! its drag released with it, read at one point; fullscreen as one override over a geometry that
//! is still a function of coordinates and the surface, re-derived at one point.
//!
//! One thing suppression is *not*, and must not become: a way for a *reveal* to be silently
//! undone. The cross-tab paths carry `hidden` deliberately, and the two that mean "the user asked
//! for this pane back" (`LaunchOrFocusPlugin { move_to_focused_tab }` and `break_pane_to_new_tab`)
//! override it to `false` at the call site, in the open, rather than relying on the bit being lost
//! in transit.
//!
//! Not to be confused with the nested-session **guest modal**
//! (`crate::panes::nested_session_modal`), which shares the word and nothing else: it is a dialog
//! drawn onto an existing terminal pane's cells rather than a pane in its own right.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use zellij_utils::{
    data::{
        FloatingPaneCoordinates, ModeInfo, OverlayKind, OverlayPlacement, PaneInfo, ResizeStrategy,
        Style, Styling,
    },
    errors::prelude::*,
    input::layout::{PercentOrFixed, RunPluginOrAlias},
    input::options::PaneFrameStyle,
    pane_size::{Offset, PaneGeom, Size, SizeInPixels, Viewport},
    position::Position,
};

use crate::panes::kitty_graphics::KittyHostSupport;
use crate::resize_pty;
use crate::tab::{pane_info_for_pane, Pane, MIN_TERMINAL_WIDTH};
use crate::ui::damage::CellRect;
use crate::{
    os_input_output::ServerOsApi,
    output::Output,
    panes::floating_panes::floating_pane_grid::{
        geom_moved_within, geom_resized_within, half_size_middle_geom,
    },
    panes::{geom_is_outside_viewport, ActivePanes, PaneId, PaneLookup},
    plugins::PluginInstruction,
    thread_bus::ThreadSenders,
    ui::pane_contents_and_ui::PaneContentsAndUi,
    ClientId,
};

/// A pane on the session surface, together with the two things that make it one: what it does when
/// focus moves away, and the coordinates it re-resolves against when the surface changes.
pub struct OverlayPane {
    pane: Box<dyn Pane>,
    kind: OverlayKind,
    coordinates: FloatingPaneCoordinates,
    /// The in-memory half of this wrapper - see [`OverlayCarry`], which is how it survives an
    /// extraction.
    carry: OverlayCarry,
}

/// The half of an overlay's wrapper that is *not* placement: whether it is set aside, and whether
/// where it sits is the user's doing.
///
/// Being expanded over the screen is deliberately *not* here, though it is in-memory and not
/// placement either. What this struct carries is what the pane **is** - the user set it aside,
/// the user put it there, the user made it that size - and all of that should survive the pane
/// being moved to another tab. Fullscreen is where the user is *looking*, which the tab they left
/// is the whole context for, so an overlay carried elsewhere arrives at its coordinates.
///
/// Both are in-memory only, which is why neither is in `OverlayPlacement` - that struct crosses
/// the protobuf boundary. `extract_pane` drops this wrapper on the way to another tab, so a mover
/// reads this first ([`OverlayPanes::carry_of`], [`crate::tab::Tab::overlay_carry_for`]) and hands
/// it back to `add_pane`. They travel together rather than one at a time because the failure mode
/// of forgetting one is invisible: the overlay arrives, simply not as itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayCarry {
    /// Set aside by `hide_pane`: the overlay still exists, still holds its plugin and its
    /// scrollback, and is still re-resolved against the surface - it is simply not drawn and
    /// cannot be focused until `show_pane` names it again.
    pub hidden: bool,
    /// The user dragged this overlay, so its `x`/`y` are theirs and a plugin re-asserting its
    /// placement no longer moves it (`change_pane_coordinates`).
    ///
    /// The *coordinates* the drag wrote travel in `OverlayPlacement` already; this is only the
    /// "the user said so" bit. It used to be dropped by a move, which made every tab switch undo
    /// the drag for a plugin that re-places itself - hocus's overlay column does, on every
    /// `TabUpdate`.
    pub position_is_user_stated: bool,
    /// The user resized this overlay, so its `width`/`height` are theirs and a plugin re-asserting
    /// its placement no longer resizes it (`change_pane_coordinates`).
    ///
    /// The size twin of `position_is_user_stated`, and here for the same reason: the *coordinates*
    /// the resize wrote travel in `OverlayPlacement` already, only the "the user said so" bit does
    /// not.
    pub size_is_user_stated: bool,
    /// This overlay is placed by its owner and has not been placed yet, so it is not painted.
    ///
    /// **Not `hidden`, deliberately.** `hidden` is the user setting a pane aside: it moves clients
    /// off the pane, refuses focus, and is reported as `is_suppressed` - which is the very field a
    /// plugin reads to tell that it was hidden or revealed. A pane that arrived a frame ago and is
    /// waiting to be told where it goes has been neither hidden nor revealed, and saying so in
    /// that field would have hocus believe the user had put it away. This suppresses *painting*
    /// and nothing else: the pane exists, can be focused, and reports itself visible.
    ///
    /// What it buys is the flash. An overlay that named no coordinates is born half the surface,
    /// centered (`OverlayPanes::base_geom`), and only its owner moves it out of there - a round
    /// trip later, and only once the pane is on a tab for the placement to find. Until then the
    /// panel that belongs in the corner is painted in the middle of the screen, on top of its
    /// other half. Asked for at launch with `awaits_placement true`
    /// (`RunPluginOrAlias::awaits_placement`), and released by the first
    /// `change_pane_coordinates` - or by a `reapply_coordinates`, which is the safety valve: an
    /// overlay whose owner never places it becomes visible at the default on the next resize
    /// rather than being invisible for as long as it lives.
    pub awaiting_placement: bool,
}

impl PaneLookup for BTreeMap<PaneId, OverlayPane> {
    fn pane(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.get(pane_id).map(|overlay| &overlay.pane)
    }
}

pub struct OverlayPanes {
    panes: BTreeMap<PaneId, OverlayPane>,
    /// Bottom-most first. Unlike the floating layer's, this list is never filtered on the way to
    /// being rendered - see `stack_layer_ids`.
    z_indices: Vec<PaneId>,
    active_panes: ActivePanes,
    /// `(pane id, where the press started, where the pointer was last seen)`, while a frame drag
    /// is in flight. Per-collection rather than per-client, as the floating layer's twin is.
    pane_being_moved_with_mouse: Option<(PaneId, Position, Position)>,
    /// Pairs of overlays placed together as one panel, `(left, right)` - the right one starting
    /// on the column where the left one ends. See the module doc; `link_touching` is the only
    /// way in, and hiding, removing or placing a member apart again is the way out.
    ///
    /// **A pane is in at most one pair.** `link_of` answers with the first pair naming a pane
    /// and every reader assumes that is the only one, so `link_touching` keeps the invariant
    /// rather than leaving a second pair nobody would consult.
    linked: Vec<(PaneId, PaneId)>,
    /// `(left, right, seam column − pointer column at the press)` while a seam drag is in flight.
    /// The seam twin of `pane_being_moved_with_mouse`, and exclusive with it: the pointer is one
    /// pointer. The offset rather than the last pointer position, so the seam is placed from
    /// where the pointer *is* and never drifts from it past a clamp.
    seam_being_dragged: Option<(PaneId, PaneId, isize)>,
    /// `(the overlay the user expanded over the screen, whether it covers the chrome too)`.
    ///
    /// One id even for a linked panel, both halves of which are expanded: which panes that is
    /// comes off the link as it is now (`fullscreen_pane_ids`), so the expansion survives its
    /// panel being re-cut or unlinked underneath it. See the module doc.
    fullscreen: Option<(PaneId, bool)>,
    display_area: Rc<RefCell<Size>>,
    /// The tab's own viewport, needed only to answer whether an overlay owns cells outside it -
    /// which, drawn over the session, it almost always does.
    viewport: Rc<RefCell<Viewport>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<HashSet<ClientId>>>,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    style: Style,
    session_is_mirrored: bool,
    default_mode_info: ModeInfo,
    senders: ThreadSenders,
    pane_frame_style: PaneFrameStyle,
    window_title: Option<String>,
    dimmed_clients: HashSet<ClientId>,
}

#[allow(clippy::borrowed_box)]
#[allow(clippy::too_many_arguments)]
impl OverlayPanes {
    pub fn new(
        display_area: Rc<RefCell<Size>>,
        viewport: Rc<RefCell<Viewport>>,
        connected_clients: Rc<RefCell<HashSet<ClientId>>>,
        connected_clients_in_app: Rc<RefCell<HashSet<ClientId>>>,
        mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        pane_frame_style: PaneFrameStyle,
        session_is_mirrored: bool,
        default_mode_info: ModeInfo,
        style: Style,
        os_input: Box<dyn ServerOsApi>,
        senders: ThreadSenders,
    ) -> Self {
        OverlayPanes {
            panes: BTreeMap::new(),
            z_indices: vec![],
            active_panes: ActivePanes::new(&os_input),
            display_area,
            viewport,
            connected_clients,
            connected_clients_in_app,
            mode_info,
            character_cell_size,
            style,
            session_is_mirrored,
            default_mode_info,
            senders,
            pane_frame_style,
            window_title: None,
            dimmed_clients: HashSet::new(),
            pane_being_moved_with_mouse: None,
            linked: vec![],
            seam_being_dragged: None,
            fullscreen: None,
        }
    }

    // ---- the coordinate space -------------------------------------------------------------

    /// The whole session surface an overlay's coordinates resolve against: the tab's display
    /// area, chrome included, which a tab always occupies in full from the screen origin.
    ///
    /// Read fresh on every use rather than cached: `Tab::resize_whole_tab_locally` updates the
    /// shared `display_area` cell partway through a resize, and an overlay resolved against the
    /// value from before that would keep the old surface's dimensions.
    fn surface(&self) -> Viewport {
        let display_area = *self.display_area.borrow();
        Viewport {
            x: 0,
            y: 0,
            cols: display_area.cols,
            rows: display_area.rows,
        }
    }

    /// The geometry an overlay resolves its coordinates against, before the coordinates are
    /// applied on top.
    ///
    /// `PaneGeom::adjust_coordinates` writes only the fields the coordinates actually name, so
    /// whatever this returns *is* what a coordinate that said nothing means. Half the surface,
    /// centered, is what a floating pane with no coordinates gets from
    /// `FloatingPanes::find_room_for_new_pane`, and it is what `OverlayPlacement::coordinates`
    /// has always documented `None` to mean.
    ///
    /// Deliberately *not* the pane's current geometry, which is what all three resolution points
    /// used to start from. An overlay has no positional history to preserve - it is re-resolved
    /// against the surface whenever the surface changes - and a pane built for this path arrives
    /// with `PaneGeom::default()`, whose 100%-constraint dimensions have an `inner` of 1: an
    /// overlay that named no size was born one cell square, and a framed one has no content area
    /// at all. Seeding here rather than at `add_pane` alone is what makes an overlay's geometry
    /// the pure function of its coordinates and the surface that this module claims it is.
    fn base_geom(surface: &Viewport, current: PaneGeom) -> PaneGeom {
        let mut geom = half_size_middle_geom(surface, 0);
        // `is_pinned` lives inside the geom rather than beside it, and `Tab::add_overlay_pane`
        // sets it on the pane before handing it over - seeding without carrying it across would
        // silently drop a flag the caller explicitly asked for. `stacked` is left at
        // `half_size_middle_geom`'s `None`, which is both correct and defensive: an overlay does
        // not tile, so it is never a member of a stack.
        geom.is_pinned = current.is_pinned;
        geom.logical_position = current.logical_position;
        geom
    }

    /// `coordinates` resolved against `surface`, for a pane whose geometry is `current` now -
    /// the one resolution `add_pane`, `change_pane_coordinates` and `reapply_coordinates` all
    /// make. Pure: nothing is written back, so a caller can ask what a placement *would* give
    /// before deciding how to apply it, which is what `place_linked_pair` does.
    fn resolved(
        surface: &Viewport,
        current: PaneGeom,
        coordinates: &FloatingPaneCoordinates,
    ) -> PaneGeom {
        let mut geom = Self::base_geom(surface, current);
        geom.adjust_coordinates(coordinates.clone(), *surface);
        geom
    }

    // ---- membership -----------------------------------------------------------------------

    /// Put `pane` on the surface as an overlay of `kind`, resolving `coordinates` against the
    /// surface as it is now.
    ///
    /// Unlike `FloatingPanes::add_pane` this does the geometry itself rather than leaving it to
    /// the caller: an overlay's geometry is entirely a function of its coordinates and the
    /// surface, and letting a caller supply it is exactly how overlay-ness used to get lost. The
    /// geometry the pane arrives with is therefore not consulted - see `base_geom`, which is what
    /// an overlay whose coordinates named no size resolves against.
    pub fn add_pane(
        &mut self,
        pane_id: PaneId,
        mut pane: Box<dyn Pane>,
        kind: OverlayKind,
        coordinates: FloatingPaneCoordinates,
        carry: OverlayCarry,
    ) {
        let surface = self.surface();
        // `adjust_coordinates` resolves the geometry and nothing else, so a coordinate that is
        // not geometry has to be applied by hand - as `change_pane_coordinates` does.
        if let Some(borderless) = coordinates.borderless {
            pane.set_borderless(borderless);
        }
        let geom = Self::resolved(&surface, pane.position_and_size(), &coordinates);
        pane.set_geom(geom);
        pane.set_should_render(true);
        self.panes.insert(
            pane_id,
            OverlayPane {
                pane,
                kind,
                coordinates,
                carry,
            },
        );
        self.raise_within_class(pane_id);
        self.set_pane_frames().non_fatal();
    }

    /// Set `pane_id` aside: still here, still loaded, no longer drawn and no longer focusable.
    ///
    /// This is per-pane suppression, the shape `Tab::suppress_pane` already has, and *not* a
    /// layer toggle - see the module doc. The two things it does beyond setting the flag are the
    /// two that keep the rest of the collection honest:
    ///
    /// - clients are moved off it, so `active_pane_id` never names a pane nobody can see. This is
    ///   what lets every focus query stay unfiltered.
    /// - a drag holding this pane is released, so the mouse is not left captured by a frame that
    ///   is no longer painted - the same reasoning as `remove_pane`'s.
    ///
    /// Returns whether anything changed, so a caller can skip a render it does not need.
    pub fn hide_pane(&mut self, pane_id: PaneId) -> bool {
        match self.panes.get_mut(&pane_id) {
            Some(overlay) if !overlay.carry.hidden => overlay.carry.hidden = true,
            _ => return false,
        }
        if matches!(self.pane_being_moved_with_mouse, Some((dragged, ..)) if dragged == pane_id) {
            self.pane_being_moved_with_mouse = None;
        }
        // a hidden half has no seam to drag, but it is still half of its panel: the link stays,
        // so the placement that reveals it finds the pair - see the module doc
        self.release_seam_drag_of(pane_id);
        // setting the pane the user is looking at in full aside is asking for it to go away, not
        // for the panel to carry on expanded without it - and an expansion anchored to a pane
        // nobody can see is a screenful of overlay with no way back.
        //
        // The flag is already set above, so for a *linked* pair this asks about the panel as it
        // is now: a pair with a hidden half is the shown half taking the whole panel, here as in
        // every other placement, and hiding the non-anchor half leaves the survivor expanded
        // rather than collapsing it. `remove_pane` is the one that must not read it that way,
        // there being no half left to come back.
        self.release_fullscreen_of(pane_id);
        if self.is_modal(&pane_id) {
            // A modal's clients may not simply fall to whatever is underneath. While this modal
            // was on screen `may_be_focused` refused every non-modal overlay the keyboard, and
            // setting the modal aside is not the user asking for that refusal to be reversed - it
            // is the same modal, merely not drawn. So focus walks to the next *modal* down, or
            // nowhere. `move_clients_out_of_pane`'s selectable-only rule is right for a
            // dismissible overlay and wrong here for exactly this reason.
            self.move_clients_out_of_modal(pane_id);
        } else {
            self.move_clients_out_of_pane(pane_id);
        }
        self.set_force_render();
        true
    }

    /// Put `pane_id` back on screen. Deliberately does not focus it - *unless it is a modal with
    /// no other modal above it*: `focus_pane_if_exists` is the path that reveals *and* focuses an
    /// ordinary overlay, and a caller that only wants the pane drawn - hocus restoring its agent
    /// list beside itself - should not move the keyboard by doing so.
    ///
    /// A lone modal is the exception because for a modal the two are not separable. The modal lock
    /// is read off `stack_layer_ids`, which filters `hidden`, so this statement is what re-arms
    /// it: the moment this pane is back in the stack, `may_be_focused` starts refusing every other
    /// overlay the keyboard. Leaving it unfocused would put a dialog on screen that nobody is in
    /// and that nothing else may be focused *past* - and the modal has no channel to learn it, so
    /// it would sit there taking no input and letting nothing else take any either. Whoever
    /// unsuppresses a modal is asking for the modal back, and a modal *is* its keyboard.
    ///
    /// **Unless another modal came up while this one was away**, which is the one case where that
    /// reasoning inverts: the newer modal is the dialog the user is actually in, and it has no
    /// channel to learn it lost either. So an outranked modal is neither raised nor focused - see
    /// `outranked` below. It is not stranded by that: it is back in the stack, so the lock holds
    /// either way, and closing the newer modal walks focus down onto this one
    /// (`move_clients_out_of_pane`).
    pub fn show_pane(&mut self, pane_id: PaneId) -> bool {
        match self.panes.get_mut(&pane_id) {
            Some(overlay) if overlay.carry.hidden => overlay.carry.hidden = false,
            _ => return false,
        }
        // Whether a modal that opened while this one was set aside is on screen now. Read after
        // the unhide, so `stack_layer_ids` is the stack as it will actually be.
        //
        // Doing *nothing* is what keeps this pane underneath that modal: it never gave up its
        // place in `z_indices` - only `remove_pane` does that - and it was pushed there before the
        // newer modal was, so its stored position is already below. `raise_within_class` would
        // undo exactly that, because for a modal the top of its class *is* the top of the stack.
        // A non-modal is never outranked here: `raise_within_class` already puts one below the
        // lowest modal, which is where it belongs.
        let outranked = self.is_modal(&pane_id)
            && self.stack_layer_ids().iter().any(|other| {
                *other != pane_id
                    && self.is_modal(other)
                    && self.get(other).is_some_and(|pane| pane.selectable())
            });
        if !outranked {
            self.raise_within_class(pane_id);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        if self.is_modal(&pane_id) && !outranked {
            self.focus_pane_for_all_clients(pane_id);
        }
        true
    }

    /// Put `pane_id` at the top of its class in the occlusion stack: a modal at the very top,
    /// anything else directly below the lowest modal.
    ///
    /// A modal owns the keyboard until it closes and has no way to learn otherwise, so an overlay
    /// that opens - or is clicked - later must not draw over it. A dialog the user cannot see is
    /// worse than one that is merely not topmost. Within a class this is still plain raise order,
    /// which is what it was for every overlay before modals were partitioned out.
    fn raise_within_class(&mut self, pane_id: PaneId) {
        self.z_indices.retain(|p_id| *p_id != pane_id);
        let lowest_modal = if self.is_modal(&pane_id) {
            None
        } else {
            self.z_indices.iter().position(|p_id| self.is_modal(p_id))
        };
        match lowest_modal {
            Some(index) => self.z_indices.insert(index, pane_id),
            None => self.z_indices.push(pane_id),
        }
    }

    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        self.z_indices.retain(|p_id| *p_id != pane_id);
        // an overlay can close while its own frame is held down - a popup that dismisses itself,
        // or its process exiting. Leaving the drag pointing at a pane that is gone would swallow
        // every mouse event until a release arrived to clear it.
        if matches!(self.pane_being_moved_with_mouse, Some((dragged, ..)) if dragged == pane_id) {
            self.pane_being_moved_with_mouse = None;
        }
        // and an expansion holding it puts the whole panel back, for the same reason: a panel
        // half of which no longer exists is no state to leave the survivor in. *Before* the
        // unlink, because `fullscreen_pane_ids` is derived from the link: once the pair is
        // forgotten it names the anchor alone, and the non-anchor half would slip past the guard.
        self.release_fullscreen_of(pane_id);
        self.unlink(pane_id);
        self.panes.remove(&pane_id).map(|overlay| overlay.pane)
    }

    /// Swap the pane behind `pane_id` for another, keeping its place in the occlusion stack, its
    /// kind and its coordinates. The replacement is an overlay because the slot is, which is the
    /// property `FloatingPanes::replace_pane` had to maintain by hand across two side maps.
    pub fn replace_pane(
        &mut self,
        pane_id: PaneId,
        mut with_pane: Box<dyn Pane>,
    ) -> Result<Box<dyn Pane>> {
        let err_context = || format!("failed to replace overlay pane {pane_id:?}");

        let with_pane_id = with_pane.pid();
        let removed = self
            .panes
            .remove(&pane_id)
            .with_context(|| format!("no overlay pane with ID {pane_id:?}"))
            .with_context(err_context)?;

        // the slot's borderless-ness travels with the slot; the `set_pane_frames` below turns
        // that into a frame and a content offset
        if let Some(borderless) = removed.coordinates.borderless {
            with_pane.set_borderless(borderless);
        } else {
            with_pane.set_content_offset(Offset::frame(1));
        }
        // the *base* geometry, not `current_geom`: an expansion is an override over the top of
        // it, and baking that in would leave the replacement with nothing to come back to. The
        // `set_pane_frames` below puts the override back on the new pane.
        with_pane.set_geom(removed.pane.position_and_size());
        self.panes.insert(
            with_pane_id,
            OverlayPane {
                pane: with_pane,
                kind: removed.kind,
                coordinates: removed.coordinates,
                // the slot keeps where the user put it and whether it was set aside, exactly as
                // it keeps its kind and its coordinates - a pane swapped into a dragged overlay
                // is the pane the user dragged, and one swapped into a hidden slot is not
                // suddenly on screen
                carry: removed.carry,
            },
        );
        if let Some(index) = self.z_indices.iter().position(|p_id| p_id == &pane_id) {
            self.z_indices[index] = with_pane_id;
        } else {
            self.z_indices.push(with_pane_id);
        }
        // the slot's place in a panel travels with the slot too: a pane swapped into the left
        // half of a panel is the left half of that panel
        for (left, right) in &mut self.linked {
            if *left == pane_id {
                *left = with_pane_id;
            }
            if *right == pane_id {
                *right = with_pane_id;
            }
        }
        // and its place in an expansion: a pane swapped into an expanded overlay is expanded.
        // `set_pane_frames` below re-derives the override itself, so only the anchor is fixed up
        // here - the same shape as the `z_indices` and `linked` fixups above.
        if let Some((anchor, _)) = self.fullscreen.as_mut() {
            if *anchor == pane_id {
                *anchor = with_pane_id;
            }
        }
        self.move_clients_between_panes(pane_id, with_pane_id);
        self.set_pane_frames().non_fatal();
        // The pane left `self.panes` at the top of this function, so `refresh_fullscreen`'s reset
        // loop - which owns every override in the collection - cannot reach it. Without this the
        // pane handed back to the caller wears a full-screen override belonging to a slot it is
        // no longer in, and whatever the caller does with it next is done at the wrong size.
        let mut replaced = removed.pane;
        replaced.reset_size_and_position_override();
        Ok(replaced)
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.keys()
    }
    pub fn panes_contain(&self, pane_id: &PaneId) -> bool {
        self.panes.contains_key(pane_id)
    }
    /// Whether anything here is on screen.
    ///
    /// Visible panes only, because `Tab::overlays_are_rendered` is this and it gates both
    /// `Tab::get_active_pane_id` and the occlusion stack: a tab holding nothing but a hidden
    /// overlay has to behave exactly like a tab holding no overlay at all.
    pub fn has_panes(&self) -> bool {
        self.panes.values().any(|overlay| !overlay.carry.hidden)
    }
    /// Whether this collection holds nothing *at all*, hidden panes included - the lifecycle
    /// question, as against `has_panes`'s drawing one.
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
    /// Whether `pane_id` is here but set aside. `false` for a pane that is not here at all.
    pub fn is_hidden(&self, pane_id: &PaneId) -> bool {
        self.carry_of(pane_id).hidden
    }
    /// The in-memory half of `pane_id`'s wrapper, for a caller about to extract it - see
    /// [`OverlayCarry`]. Defaults for a pane that is not here at all, which is what a pane with
    /// no wrapper to lose would arrive as anyway.
    pub fn carry_of(&self, pane_id: &PaneId) -> OverlayCarry {
        self.panes
            .get(pane_id)
            .map(|overlay| overlay.carry)
            .unwrap_or_default()
    }
    pub fn len(&self) -> usize {
        self.panes.len()
    }
    pub fn get(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.panes.get(pane_id).map(|overlay| &overlay.pane)
    }
    pub fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut Box<dyn Pane>> {
        self.panes.get_mut(pane_id).map(|overlay| &mut overlay.pane)
    }
    /// By-value twins of `get`/`get_mut`, so that the `.or_else(..)` chains in `Tab` that walk the
    /// collections in turn read the same for each one - `FloatingPanes::get_pane` takes a `PaneId`.
    pub fn get_pane(&self, pane_id: PaneId) -> Option<&Box<dyn Pane>> {
        self.get(&pane_id)
    }
    pub fn get_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.get_mut(&pane_id)
    }
    pub fn get_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.panes
            .iter()
            .map(|(pane_id, overlay)| (pane_id, &overlay.pane))
    }
    pub fn get_panes_mut(&mut self) -> impl Iterator<Item = (&PaneId, &mut Box<dyn Pane>)> {
        self.panes
            .iter_mut()
            .map(|(pane_id, overlay)| (pane_id, &mut overlay.pane))
    }
    pub fn get_plugin_pane_id(&self, run_plugin_or_alias: &RunPluginOrAlias) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_, overlay)| {
                run_plugin_or_alias.is_equivalent_to_run(overlay.pane.invoked_with())
            })
            .map(|(pane_id, _)| *pane_id)
    }
    /// How this overlay is placed - its kind and the coordinates it resolves against - or `None`
    /// if it is not an overlay at all. This is what has to travel when a pane moves between tabs.
    pub fn placement_of(&self, pane_id: &PaneId) -> Option<OverlayPlacement> {
        self.panes.get(pane_id).map(|overlay| OverlayPlacement {
            kind: overlay.kind,
            coordinates: Some(overlay.coordinates.clone()),
        })
    }
    /// What this overlay does when focus moves away, or `None` if it is not an overlay at all.
    pub fn kind_of(&self, pane_id: &PaneId) -> Option<OverlayKind> {
        self.panes.get(pane_id).map(|overlay| overlay.kind)
    }
    pub fn is_modal(&self, pane_id: &PaneId) -> bool {
        self.kind_of(pane_id).is_some_and(|kind| kind.is_modal())
    }
    /// Whether the user may resize this overlay: it is one, it is not a modal, and it is not
    /// expanded over the screen.
    ///
    /// A modal keeps the size its plugin asked for. It is drawn *because* something needs an
    /// answer right now, its content is sized to that question, and unlike a dismissible overlay
    /// there is no case of it sitting over something the user is trying to read.
    ///
    /// An expanded overlay is already the size of the screen, and the rectangle underneath it is
    /// the one it comes back to - a resize there would change a size the user cannot see, and
    /// write it into the coordinates as the user's. This one answer is what refuses the keyboard
    /// resize, ctrl+MouseScroll and the ctrl-drag of a border alike, and what stops the frame
    /// offering any of them.
    pub fn is_resizable(&self, pane_id: &PaneId) -> bool {
        self.kind_of(pane_id)
            .is_some_and(|kind| !matches!(kind, OverlayKind::Modal))
            && !self.pane_is_fullscreen(pane_id)
    }
    /// The last overlay a client could be sent to, bottom-of-stack-last. Used when focus has to
    /// land somewhere in this collection and nobody said where.
    pub fn last_selectable_pane_id(&self) -> Option<PaneId> {
        self.stack_layer_ids()
            .into_iter()
            .rev()
            .find(|pane_id| self.get(pane_id).is_some_and(|pane| pane.selectable()))
    }
    pub fn has_selectable_panes(&self) -> bool {
        self.last_selectable_pane_id().is_some()
    }
    pub fn active_panes_contain(&self, client_id: &ClientId) -> bool {
        self.active_panes.contains_key(client_id)
    }

    /// How each overlay reports itself to plugins and to the CLI.
    ///
    /// `is_floating` stays true: an overlay is not on the floating layer, but every consumer of
    /// `PaneInfo` reads that field as "not tiled", and a pane drawn over the session is certainly
    /// that. Changing the contract is a separate question from moving the pane.
    pub fn pane_info(&self, current_pane_group: &HashMap<ClientId, Vec<PaneId>>) -> Vec<PaneInfo> {
        self.panes
            .keys()
            .filter_map(|pane_id| self.pane_info_of(pane_id, current_pane_group))
            .collect()
    }

    /// One overlay's `PaneInfo`, the singular half of `pane_info` - `Tab`'s by-id lookup uses
    /// this rather than spelling the fields again beside it.
    pub fn pane_info_of(
        &self,
        pane_id: &PaneId,
        current_pane_group: &HashMap<ClientId, Vec<PaneId>>,
    ) -> Option<PaneInfo> {
        let overlay = self.panes.get(pane_id)?;
        let mut pane_info_for_pane =
            pane_info_for_pane(pane_id, overlay.pane.as_ref(), current_pane_group);
        pane_info_for_pane.is_floating = true;
        // a hidden overlay is set aside exactly as a suppressed tab pane is, and that is
        // the field every consumer already reads to mean "here but not on screen"
        pane_info_for_pane.is_suppressed = overlay.carry.hidden;
        pane_info_for_pane.is_focused =
            !overlay.carry.hidden && self.active_panes.pane_id_is_focused(pane_id);
        pane_info_for_pane.is_fullscreen = self.pane_is_fullscreen(pane_id);
        if pane_info_for_pane.is_fullscreen {
            // The *base* rectangle, not the expanded one. An expansion is the user looking at
            // this overlay closely for a moment; it is not a placement, and it must not read as
            // one to the plugin that owns the overlay. hocus takes its panel's size from this
            // manifest - that is how a user resize reaches it - and would answer an expansion by
            // re-placing the panel at the size of the screen, overwriting the very rectangle the
            // expansion is there to come back to. `is_fullscreen` above is how a plugin that
            // cares learns the difference.
            Self::report_base_geometry(&mut pane_info_for_pane, overlay);
        }
        Some(pane_info_for_pane)
    }

    /// Overwrite the geometry `pane_info_for_pane` read off an expanded pane with the rectangle
    /// underneath the expansion, content area included.
    fn report_base_geometry(pane_info: &mut PaneInfo, overlay: &OverlayPane) {
        let geom = overlay.pane.position_and_size();
        // the offset the frame loop would give this pane were it not expanded - the expanded one
        // is on the pane itself and belongs to a rectangle we are not reporting
        let offset = if overlay.pane.borderless() {
            Offset::default()
        } else {
            Offset::frame(1)
        };
        pane_info.pane_x = geom.x;
        pane_info.pane_y = geom.y;
        pane_info.pane_columns = geom.cols.as_usize();
        pane_info.pane_rows = geom.rows.as_usize();
        pane_info.pane_content_x = geom.x + offset.left;
        pane_info.pane_content_y = geom.y + offset.top;
        pane_info.pane_content_columns = geom
            .cols
            .as_usize()
            .saturating_sub(offset.left + offset.right);
        pane_info.pane_content_rows = geom
            .rows
            .as_usize()
            .saturating_sub(offset.top + offset.bottom);
    }

    // ---- z-order and occlusion ------------------------------------------------------------

    /// The panes making up the occlusion stack, bottom-most first.
    ///
    /// The one gate here is `hidden` - a pane `hide_pane` set aside. There is no *layer*
    /// visibility: an overlay that has not been set aside by name is drawn for as long as it
    /// exists. `FloatingPanes` has to filter this list (fullscreen, pinned-only, layer-hidden)
    /// and warns that `stack_layer_ids` and `render` must stay derived from the same list or a
    /// pane is clipped against its own geometry and erases itself - it maintains that by
    /// hand-duplicating the filter chain in two places. Here `render` iterates this method
    /// directly, so the two cannot drift, and so this one filter is also what keeps a hidden
    /// overlay out of the occlusion stack, out of hit-testing and out of every focus target.
    pub fn stack_layer_ids(&self) -> Vec<PaneId> {
        self.z_indices
            .iter()
            .filter(|pane_id| {
                self.panes.get(pane_id).is_some_and(|overlay| {
                    !overlay.carry.hidden && !overlay.carry.awaiting_placement
                })
            })
            .copied()
            .collect()
    }
    pub fn rendered_pane_ids(&self) -> Vec<PaneId> {
        self.stack_layer_ids()
    }
    pub fn get_pane_z_index(&self, pane_id: PaneId) -> Option<usize> {
        self.stack_layer_ids().iter().position(|id| id == &pane_id)
    }
    /// The geometries `Output` clips against, bottom-most first.
    ///
    /// Bare geoms rather than a `FloatingPanesStack`: the tab concatenates its floating layers and
    /// then these into one flat list, so that an overlay clips above every floating pane.
    ///
    /// `current_geom` rather than `position_and_size`: an expanded overlay covers what is under it
    /// with the rectangle it is *drawn* with, not the one it will come back to. This is the
    /// visibility-shaped side of the split the module doc draws - the placement-shaped readers
    /// keep reading the base.
    pub fn layers(&self) -> Vec<PaneGeom> {
        self.stack_layer_ids()
            .iter()
            .filter_map(|pane_id| self.get(pane_id).map(|pane| pane.current_geom()))
            .collect()
    }

    // ---- focus ----------------------------------------------------------------------------

    pub fn active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get(&client_id).copied()
    }
    pub fn has_active_panes(&self) -> bool {
        !self.active_panes.is_empty()
    }
    pub fn get_active_pane(&self, client_id: ClientId) -> Option<&Box<dyn Pane>> {
        self.active_pane_id(client_id)
            .and_then(|pane_id| self.get(&pane_id))
    }
    pub fn get_active_pane_mut(&mut self, client_id: ClientId) -> Option<&mut Box<dyn Pane>> {
        self.active_pane_id(client_id)
            .and_then(move |pane_id| self.get_mut(&pane_id))
    }
    /// The overlay a client that was not here when it opened has to be put on, or `None` if there
    /// is no modal.
    ///
    /// A modal is the one pane that must own the keyboard and has no way to learn that it doesn't,
    /// so it outranks whatever else is in the stack rather than being merely the topmost thing.
    pub fn topmost_modal_pane_id(&self) -> Option<PaneId> {
        self.stack_layer_ids().into_iter().rev().find(|pane_id| {
            self.is_modal(pane_id) && self.get(pane_id).is_some_and(|pane| pane.selectable())
        })
    }
    /// The overlay some other client is already looking at, if any - the overlay counterpart of
    /// `FloatingPanes::first_active_floating_pane_id`. `None` when nobody here has overlay focus,
    /// which is also what a dismissible overlay that gave its focus up looks like.
    pub fn first_active_pane_id(&self) -> Option<PaneId> {
        self.active_panes.values().next().copied()
    }
    /// Put a client that joined after this overlay opened onto it, unless it already has overlay
    /// focus of its own.
    ///
    /// Unlike `focus_pane` this leaves `z_indices` alone: the occlusion stack is shared by every
    /// client, and a joining client inheriting an overlay that is not the topmost one would
    /// reorder what everybody else sees. It records no `last_pane` and no `set_active_at` either -
    /// joining is not activity, and there is no previous overlay to go back to.
    pub fn focus_pane_for_joining_client(&mut self, pane_id: PaneId, client_id: ClientId) {
        if self.active_panes.contains_key(&client_id) || !self.may_be_focused(&pane_id) {
            return;
        }
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.set_force_render();
    }
    /// Give every connected client this overlay.
    ///
    /// Overlays are focused for all clients at once, as floating panes are: the occlusion stack is
    /// shared, so a per-client focus would draw one client's overlay over another's anyway.
    pub fn focus_pane_for_all_clients(&mut self, pane_id: PaneId) {
        if !self.may_be_focused(&pane_id) {
            return;
        }
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            self.active_panes.insert(client_id, pane_id, &self.panes);
            self.set_pane_active_at(pane_id);
        }
        self.raise_within_class(pane_id);
        self.set_force_render();
    }
    /// Whether focus may move onto this overlay right now.
    ///
    /// A live modal owns the keyboard until it closes: it has no way to learn that it lost focus,
    /// so an overlay that opened later - or that the mouse landed on - does not take it. This is
    /// the overlay twin of what `Tab::new_pane` already does for a tiled pane opened over a modal,
    /// which comes up unfocused rather than stealing the keyboard.
    ///
    /// Another modal may take it, because closing that one walks focus back down to this one
    /// (`move_clients_out_of_pane`); that is what keeps a quit-confirmation opened over an already
    /// open modal usable rather than stranding it unfocused with no way to close itself.
    ///
    /// An overlay the tab has marked non-selectable has already had its clients moved off it
    /// (`Tab::set_pane_selectable`), and putting one back would break the invariant that an active
    /// pane is selectable - a later `FocusPaneWithId` would route input to a pane that asked not
    /// to receive any. `FloatingPanes::focus_pane` refuses the same thing.
    /// A hidden overlay is refused for the same reason a non-selectable one is: it is not on
    /// screen, so a client left holding its keyboard could not see what was taking the input.
    pub fn may_be_focused(&self, pane_id: &PaneId) -> bool {
        if self.is_hidden(pane_id) {
            return false;
        }
        if !self.get(pane_id).is_some_and(|pane| pane.selectable()) {
            return false;
        }
        self.is_modal(pane_id) || self.topmost_modal_pane_id().is_none()
    }
    /// Focus this overlay for one client, leaving every other client where it was.
    ///
    /// An overlay is drawn for everybody, but it is not focused for everybody: a second client
    /// looking at a tiled pane keeps looking at it. Raising the pane in the occlusion stack *is*
    /// shared, because the stack is.
    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        if !self.may_be_focused(&pane_id) {
            return;
        }
        if let Some(previously_active) = self.active_pane_id(client_id) {
            self.active_panes
                .set_last_pane(client_id, previously_active);
        }
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.set_pane_active_at(pane_id);
        self.raise_within_class(pane_id);
        self.set_force_render();
    }
    /// `Ok` means "this overlay is mine", not "it is now focused" - a live modal may have declined
    /// to hand the keyboard over. `Tab::focus_pane_with_id` chains this through `.or_else` into the
    /// floating and suppressed collections, so an `Err` here would go on to focus some *other*
    /// pane rather than leaving focus where the modal wants it.
    ///
    /// This is also the reveal path: a request naming a hidden overlay by id is the user asking
    /// for it back (`LaunchOrFocusPlugin` reaches here through `Tab::focus_pane_with_id`), so it
    /// is shown before it is focused - `may_be_focused` would otherwise refuse it for being
    /// hidden and the pane would stay set aside.
    pub fn focus_pane_if_exists(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        if self.panes.contains_key(&pane_id) {
            self.show_pane(pane_id);
            self.focus_pane(pane_id, client_id);
            Ok(())
        } else {
            Err(anyhow!("Pane not found"))
        }
    }
    /// Give up this client's overlay focus, so that focus falls through to whatever is underneath.
    /// Only a dismissible overlay is ever asked to do this - a modal has no way to learn it
    /// happened.
    pub fn defocus_pane(&mut self, client_id: ClientId) {
        self.active_panes.remove(&client_id, &self.panes);
        self.set_force_render();
    }
    /// Hand this client's overlay focus over to a pane that is about to open, unless a modal is
    /// holding it. `true` means the keyboard is free to move - including when this client held no
    /// overlay focus to begin with.
    ///
    /// A dismissible overlay (file-finder and friends) watches `PaneUpdate` and closes itself once
    /// it sees it is no longer focused; giving the focus up is how it learns. A modal
    /// (hocus-new-agent) only closes on Esc/submit/`Visible(false)` and has no such channel, so
    /// taking the keyboard from it would leave it on screen while input went elsewhere. It keeps
    /// focus, and whatever opened comes up unfocused behind it.
    pub fn defocus_pane_if_dismissible(&mut self, client_id: ClientId) -> bool {
        match self.active_pane_id(client_id) {
            Some(pane_id) if self.is_modal(&pane_id) => false,
            Some(_) => {
                self.defocus_pane(client_id);
                true
            },
            None => true,
        }
    }
    /// Whether a modal is holding this client's keyboard.
    ///
    /// The question a lower layer asks before it moves focus: a modal owns the keyboard until it
    /// closes, so a focus move underneath it is one the user could neither see nor undo, and the
    /// answer is to not move at all. A dismissible overlay is `false` here - it does give focus
    /// up, but only once the move it is giving way to has actually happened, which is
    /// `defocus_pane_if_dismissible`'s job rather than this one's.
    pub fn a_modal_holds_focus(&self, client_id: ClientId) -> bool {
        self.active_pane_id(client_id)
            .is_some_and(|pane_id| self.is_modal(&pane_id))
    }
    /// The every-client form of `defocus_pane_if_dismissible`, for the paths that focus a new pane
    /// for everybody rather than for one client.
    pub fn defocus_dismissible_panes(&mut self) {
        let dismissible_clients: Vec<ClientId> = self
            .active_panes
            .iter()
            .filter(|(_, pane_id)| !self.is_modal(pane_id))
            .map(|(client_id, _)| *client_id)
            .collect();
        for client_id in dismissible_clients {
            self.defocus_pane(client_id);
        }
    }
    pub fn remove_client(&mut self, client_id: ClientId) {
        self.active_panes.remove(&client_id, &self.panes);
    }
    /// Move every client focused on `pane_id` to the next overlay down, or nowhere if this was the
    /// last one. Called as a pane is about to be removed.
    pub fn move_clients_out_of_pane(&mut self, pane_id: PaneId) {
        let next = self
            .stack_layer_ids()
            .into_iter()
            .rev()
            .find(|id| *id != pane_id && self.get(id).is_some_and(|pane| pane.selectable()));
        let focused_clients: Vec<ClientId> = self
            .active_panes
            .iter()
            .filter(|(_, p_id)| **p_id == pane_id)
            .map(|(c_id, _)| *c_id)
            .collect();
        for client_id in focused_clients {
            match next {
                Some(next_pane_id) => {
                    self.active_panes
                        .insert(client_id, next_pane_id, &self.panes)
                },
                None => {
                    self.active_panes.remove(&client_id, &self.panes);
                },
            }
        }
    }
    /// The modal twin of `move_clients_out_of_pane`: walk every client focused on `pane_id` down to
    /// the next modal, or off this collection entirely.
    ///
    /// Used when a modal is set aside rather than closed. A closing modal may hand the keyboard to
    /// whatever is underneath, because the lock goes with it; a hidden one keeps the lock (it is
    /// still in `self.panes`, and `show_pane` puts it straight back into the stack), so handing the
    /// keyboard down would focus a pane that `may_be_focused` refuses.
    fn move_clients_out_of_modal(&mut self, pane_id: PaneId) {
        let next = self.stack_layer_ids().into_iter().rev().find(|id| {
            *id != pane_id
                && self.is_modal(id)
                && self.get(id).is_some_and(|pane| pane.selectable())
        });
        let focused_clients: Vec<ClientId> = self
            .active_panes
            .iter()
            .filter(|(_, p_id)| **p_id == pane_id)
            .map(|(c_id, _)| *c_id)
            .collect();
        for client_id in focused_clients {
            match next {
                Some(next_pane_id) => {
                    self.active_panes
                        .insert(client_id, next_pane_id, &self.panes)
                },
                None => {
                    self.active_panes.remove(&client_id, &self.panes);
                },
            }
        }
    }
    fn move_clients_between_panes(&mut self, from_pane_id: PaneId, to_pane_id: PaneId) {
        let focused_clients: Vec<ClientId> = self
            .active_panes
            .iter()
            .filter(|(_, p_id)| **p_id == from_pane_id)
            .map(|(c_id, _)| *c_id)
            .collect();
        for client_id in focused_clients {
            self.active_panes.insert(client_id, to_pane_id, &self.panes);
        }
    }
    fn set_pane_active_at(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_mut(&pane_id) {
            pane.set_active_at(std::time::Instant::now());
        }
    }

    // ---- geometry -------------------------------------------------------------------------

    /// Move or resize an overlay, keeping it an overlay - and keeping it where the user put it.
    ///
    /// `hocus-new-agent` re-asserts its placement on every resize, so overlay-ness has to survive a
    /// coordinate change. In `FloatingPanes` that was an explicit re-insert into a side map that
    /// callers could and did skip; here the stored `kind` is simply left alone.
    ///
    /// **A position the user dragged outranks a position a plugin asks for.** Once
    /// `move_pane_to_position` has run, `x`/`y` here are dropped and the stored ones kept, so a
    /// plugin re-asserting its placement resizes the overlay *where it is* rather than snapping it
    /// back. This is the difference between a draggable overlay and one that appears immovable: a
    /// drag reports session state, session state is a `TabUpdate`, and a plugin that re-places
    /// itself on `TabUpdate` would otherwise undo every step of its own drag.
    ///
    /// `width`/`height` answer the same way once the user has resized the overlay
    /// (`size_is_user_stated`), and for the same reason. The two claims are independent: an
    /// overlay that was only dragged still takes a plugin's size, and one that was only resized
    /// still takes a plugin's position.
    ///
    /// **Unless the position is itself the user's.** `override_user_placement` says the plugin is
    /// relaying a gesture - a key the user pressed to move the overlay - which is as much theirs
    /// as the drag was, and later. The position claim is cleared, not bypassed for this one call;
    /// the independent size claim remains, because moving a pane is not resizing it.
    pub fn change_pane_coordinates(
        &mut self,
        pane_id: PaneId,
        new_coordinates: FloatingPaneCoordinates,
        override_user_placement: bool,
    ) -> Result<()> {
        let err_context = || format!("failed to change coordinates of overlay {pane_id:?}");
        let surface = self.surface();
        let overlay = self
            .panes
            .get_mut(&pane_id)
            .with_context(|| format!("no overlay pane with ID {pane_id:?}"))
            .with_context(err_context)?;

        if let Some(borderless) = new_coordinates.borderless {
            overlay.pane.set_borderless(borderless);
        }
        // the new coordinates *replace* the stored ones rather than merging into them, so an axis
        // this call does not name is resolved from the surface default rather than from whatever
        // a previous call happened to leave behind - which is also what the next
        // `reapply_coordinates` would do with it anyway. The one exception is a position the user
        // dragged: that one is carried over, because nothing else in the system remembers it.
        let mut new_coordinates = new_coordinates;
        if override_user_placement {
            overlay.carry.position_is_user_stated = false;
        }
        if overlay.carry.position_is_user_stated {
            new_coordinates.x = overlay.coordinates.x;
            new_coordinates.y = overlay.coordinates.y;
        }
        if overlay.carry.size_is_user_stated {
            new_coordinates.width = overlay.coordinates.width;
            new_coordinates.height = overlay.coordinates.height;
        }
        let geom = Self::resolved(&surface, overlay.pane.position_and_size(), &new_coordinates);
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates = new_coordinates;
        // Somebody has placed it, which is the whole of what it was waiting for. See
        // `OverlayCarry::awaiting_placement`.
        overlay.carry.awaiting_placement = false;

        self.set_pane_frames().non_fatal();
        Ok(())
    }

    /// Apply one placement batch: every pane named, each once.
    ///
    /// A pane whose linked partner is also in the batch is placed with it as one panel
    /// (`place_linked_pair`); every other pane goes through `change_pane_coordinates` on its own.
    /// The pair is looked up on the links as they are *before* the batch - which is the whole
    /// point, since the batch is what may be moving the halves relative to each other - and
    /// `link_touching`, which the caller runs once the batch is applied, decides the links after.
    pub fn change_pane_coordinates_batch(
        &mut self,
        batch: Vec<(PaneId, FloatingPaneCoordinates)>,
        override_user_placement: bool,
    ) -> Result<()> {
        let named = |pane_id: &PaneId| batch.iter().any(|(id, _)| id == pane_id);
        let mut placed: HashSet<PaneId> = HashSet::new();
        for (pane_id, coordinates) in &batch {
            if placed.contains(pane_id) {
                continue;
            }
            let pair = self
                .link_of(pane_id)
                .filter(|(left, right)| named(left) && named(right));
            match pair {
                Some((left, right)) => {
                    let request_of = |pane_id: PaneId| {
                        batch
                            .iter()
                            .find(|(id, _)| *id == pane_id)
                            .map(|(_, coordinates)| coordinates.clone())
                            .unwrap_or_default()
                    };
                    self.place_linked_pair(
                        (left, request_of(left)),
                        (right, request_of(right)),
                        override_user_placement,
                    )?;
                    placed.insert(left);
                    placed.insert(right);
                },
                None => {
                    self.change_pane_coordinates(
                        *pane_id,
                        coordinates.clone(),
                        override_user_placement,
                    )?;
                    placed.insert(*pane_id);
                },
            }
        }
        Ok(())
    }

    /// Place a linked pair as one panel: the batch's two rectangles are a layout, and the layout
    /// is mapped onto the rectangle the user's gestures have left the panel in.
    ///
    /// `R` is the bounding box of the two requests as they would resolve on their own, `C` the
    /// bounding box of the pair as it sits now - which is where a frame drag or a ctrl-resize put
    /// it, since both gestures move the pair. The target `T` takes each axis from the claim on
    /// it, exactly as `change_pane_coordinates` does for one pane: a claimed position keeps `C`'s
    /// origin, a claimed size keeps `C`'s extent, and an unclaimed axis is the request's - so a
    /// panel that was only dragged still grows with the screen. Each request's edges are then
    /// carried from `R` onto `T` in proportion, which keeps two abutting requests abutting and
    /// gives a request that spans `R` the whole of `T`. Both halves come out with the pair's
    /// claims, since a claim about a panel is a claim about both of its halves.
    fn place_linked_pair(
        &mut self,
        left: (PaneId, FloatingPaneCoordinates),
        right: (PaneId, FloatingPaneCoordinates),
        override_user_placement: bool,
    ) -> Result<()> {
        let surface = self.surface();
        let (left_id, left_coordinates) = left;
        let (right_id, right_coordinates) = right;
        let err_context =
            || format!("failed to place linked overlays {left_id:?} and {right_id:?}");
        let current_of = |panes: &Self, pane_id: &PaneId| {
            panes
                .get(pane_id)
                .map(|pane| pane.position_and_size())
                .with_context(|| format!("no overlay pane with ID {pane_id:?}"))
                .with_context(err_context)
        };
        let left_now = current_of(self, &left_id)?;
        let right_now = current_of(self, &right_id)?;
        let (left_carry, right_carry) = (self.carry_of(&left_id), self.carry_of(&right_id));
        let position_claimed = !override_user_placement
            && (left_carry.position_is_user_stated || right_carry.position_is_user_stated);
        let size_claimed = left_carry.size_is_user_stated || right_carry.size_is_user_stated;

        let left_request = Self::resolved(&surface, left_now, &left_coordinates);
        let right_request = Self::resolved(&surface, right_now, &right_coordinates);
        let requested = union_of(left_request, right_request);
        let current = union_of(left_now, right_now);
        let mut target = requested;
        if position_claimed {
            target.x = current.x;
            target.y = current.y;
        }
        if size_claimed {
            target.cols.set_inner(current.cols.as_usize());
            target.rows.set_inner(current.rows.as_usize());
        }
        // kept on the surface the way `adjust_coordinates` keeps a single pane: pulled back
        // first, then cut down if it still does not fit
        let max_x = surface.x.saturating_add(surface.cols);
        let max_y = surface.y.saturating_add(surface.rows);
        target.x = target
            .x
            .min(max_x.saturating_sub(target.cols.as_usize()))
            .max(surface.x);
        target.y = target
            .y
            .min(max_y.saturating_sub(target.rows.as_usize()))
            .max(surface.y);
        target
            .cols
            .set_inner(target.cols.as_usize().min(max_x.saturating_sub(target.x)));
        target
            .rows
            .set_inner(target.rows.as_usize().min(max_y.saturating_sub(target.y)));

        for (pane_id, request, coordinates) in [
            (left_id, left_request, left_coordinates),
            (right_id, right_request, right_coordinates),
        ] {
            let mut geom = request;
            let x = map_edge(
                request.x,
                requested.x,
                requested.cols.as_usize(),
                target.x,
                target.cols.as_usize(),
            );
            let right_edge = map_edge(
                request.x + request.cols.as_usize(),
                requested.x,
                requested.cols.as_usize(),
                target.x,
                target.cols.as_usize(),
            );
            let y = map_edge(
                request.y,
                requested.y,
                requested.rows.as_usize(),
                target.y,
                target.rows.as_usize(),
            );
            let bottom_edge = map_edge(
                request.y + request.rows.as_usize(),
                requested.y,
                requested.rows.as_usize(),
                target.y,
                target.rows.as_usize(),
            );
            geom.x = x;
            geom.y = y;
            geom.cols.set_inner(right_edge.saturating_sub(x).max(1));
            geom.rows.set_inner(bottom_edge.saturating_sub(y).max(1));
            let Some(overlay) = self.panes.get_mut(&pane_id) else {
                continue;
            };
            if let Some(borderless) = coordinates.borderless {
                overlay.pane.set_borderless(borderless);
            }
            overlay.pane.set_geom(geom);
            overlay.pane.set_should_render(true);
            overlay.coordinates = FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(geom.x)),
                y: Some(PercentOrFixed::Fixed(geom.y)),
                width: Some(PercentOrFixed::Fixed(geom.cols.as_usize())),
                height: Some(PercentOrFixed::Fixed(geom.rows.as_usize())),
                pinned: coordinates.pinned,
                borderless: coordinates.borderless,
            };
            overlay.carry.position_is_user_stated = position_claimed;
            overlay.carry.size_is_user_stated = size_claimed;
            overlay.carry.awaiting_placement = false;
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        Ok(())
    }

    /// Turn this overlay's frame off or on, reporting whether it was an overlay at all.
    ///
    /// The stored coordinates move with the pane, not just the pane's own flag: `placement_of`
    /// hands them over when an overlay follows a client to another tab, and an overlay that got
    /// its frame back on the way across would not be the pane the user was looking at.
    pub fn set_borderless(&mut self, pane_id: &PaneId, borderless: bool) -> bool {
        let Some(overlay) = self.panes.get_mut(pane_id) else {
            return false;
        };
        overlay.pane.set_borderless(borderless);
        overlay.coordinates.borderless = Some(borderless);
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        true
    }
    pub fn toggle_borderless(&mut self, pane_id: &PaneId) -> bool {
        match self.get(pane_id).map(|pane| pane.borderless()) {
            Some(borderless) => self.set_borderless(pane_id, !borderless),
            None => false,
        }
    }

    /// Re-resolve every overlay against the surface as it is now.
    ///
    /// An overlay was placed by coordinates, not by an absolute geometry, so a surface that
    /// changed size re-resolves rather than being translated: a 60%-wide overlay is 60% of
    /// whatever the screen is now.
    pub fn reapply_coordinates(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        let surface = self.surface();
        for overlay in self.panes.values_mut() {
            let geom = Self::resolved(
                &surface,
                overlay.pane.position_and_size(),
                &overlay.coordinates,
            );
            overlay.pane.set_geom(geom);
            // The safety valve for `awaiting_placement`: an overlay nobody ever placed would
            // otherwise be invisible for as long as it lived. A resize shows it where it was born,
            // which is the behaviour it would have had without the flag at all.
            overlay.carry.awaiting_placement = false;
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
    }

    // ---- being resized --------------------------------------------------------------------

    /// One keyboard resize step, matching the floating layer's.
    const RESIZE_INCREMENT_WIDTH: usize = 5;
    const RESIZE_INCREMENT_HEIGHT: usize = 2;

    /// Resize this client's overlay by one keyboard step; `true` => something changed.
    ///
    /// Deliberately answers `false` rather than falling through to another collection when the
    /// client is not on an overlay - `Tab::resize` dispatches on membership, and an overlay
    /// resize that quietly became a tiled one is exactly the bug the early return here used to
    /// prevent.
    pub fn resize_active_pane(
        &mut self,
        client_id: ClientId,
        strategy: &ResizeStrategy,
    ) -> Result<bool> {
        let Some(pane_id) = self.active_panes.get(&client_id).copied() else {
            return Ok(false);
        };
        self.resize_pane_with_id(pane_id, *strategy)
    }

    /// Resize `pane_id` by one keyboard step; `true` => something changed.
    pub fn resize_pane_with_id(
        &mut self,
        pane_id: PaneId,
        strategy: ResizeStrategy,
    ) -> Result<bool> {
        self.resize_pane_with_strategies(
            pane_id,
            &[strategy],
            (Self::RESIZE_INCREMENT_WIDTH, Self::RESIZE_INCREMENT_HEIGHT),
        )
    }

    /// Apply `strategies` in order to `pane_id`, each by `change_by`; `true` => something changed.
    ///
    /// The mouse entry point: a corner drag is two strategies (one per axis), exactly as it is
    /// for a floating pane.
    ///
    /// **The new size is written back into the overlay's coordinates**, as `Fixed`, for the same
    /// reason `move_pane_to_position` writes the new position back: this collection keeps no
    /// geometry of record beside the panes, so a size that lived only on the geometry would be
    /// undone by the next `reapply_coordinates` - a terminal resize, the
    /// overlay following a client to another tab. A resize that moved the left or upper border
    /// moved `x`/`y` too, so that half is written back as well and becomes the user's.
    ///
    /// The cost, stated plainly: an overlay the user has sized is `Fixed`, so it stops growing
    /// with the screen. That is the same trade the drag already makes for position, and it is
    /// what "the user said so" has to mean if it is to survive a re-resolution at all.
    ///
    /// A half of a linked panel is resized as the panel, unless the other half is hidden, in
    /// which case the shown half is the whole panel and the hidden one is kept at the same
    /// rectangle. Which way "as the panel" goes depends on the strategy: a *directed* resize
    /// names an edge of the half, and that edge is either the panel's or the seam
    /// (`resize_half_of_panel`); an *undirected* one - ctrl+MouseScroll, a keyboard resize with
    /// no direction - names no edge, so it is the panel's bounding box on all four sides, with
    /// the seam kept where it was in proportion (`resize_panel`).
    pub fn resize_pane_with_strategies(
        &mut self,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> Result<bool> {
        let err_context = || format!("failed to resize overlay {pane_id:?}");
        if !self.is_resizable(&pane_id) {
            return Ok(false);
        }
        let surface = self.surface();
        let before = self
            .get(&pane_id)
            .map(|pane| pane.position_and_size())
            .with_context(|| format!("no overlay pane with ID {pane_id:?}"))
            .with_context(err_context)?;

        let partner = self.linked_partner(&pane_id);
        let visible_partner = partner.filter(|partner| !self.is_hidden(partner));
        let undirected = strategies
            .iter()
            .all(|strategy| strategy.direction.is_none());
        if let (Some(partner), true) = (visible_partner, undirected) {
            // The strategies are applied to the panel, not to the half, so the half's own
            // resized geometry is never computed: a half flush with the surface's edge could
            // report "nothing changed" while the panel still has somewhere to go.
            if !self.resize_panel(pane_id, partner, strategies, change_by) {
                return Ok(false);
            }
            self.set_pane_frames().non_fatal();
            self.set_force_render();
            return Ok(true);
        }

        let mut geom = before;
        for strategy in strategies {
            geom = geom_resized_within(geom, surface, strategy, change_by);
        }
        // `PaneGeom`'s equality covers x, y and both dimensions' inner sizes, which is exactly
        // what a resize can change here
        if geom == before {
            return Ok(false);
        }

        match visible_partner {
            // A visible partner is the other half of the same panel, and the resize is the
            // panel's: see `resize_half_of_panel`.
            Some(partner) => {
                if !self.resize_half_of_panel(pane_id, partner, before, geom) {
                    return Ok(false);
                }
            },
            None => {
                let moved = geom.x != before.x || geom.y != before.y;
                if let Some(overlay) = self.panes.get_mut(&pane_id) {
                    Self::claim_geom(overlay, geom, moved);
                }
                // A hidden partner takes the whole resized rectangle, and the claim with it,
                // since the claim is the panel's (module doc) - it is placed at its share again
                // on the reveal, and until then it must not stretch the pair's bounding box
                // beyond what the user sized.
                if let Some(overlay) = partner.and_then(|partner| self.panes.get_mut(&partner)) {
                    let mut partner_geom = overlay.pane.position_and_size();
                    partner_geom.x = geom.x;
                    partner_geom.y = geom.y;
                    partner_geom.cols.set_inner(geom.cols.as_usize());
                    partner_geom.rows.set_inner(geom.rows.as_usize());
                    Self::claim_geom(overlay, partner_geom, moved);
                }
            },
        }

        // this is also what resizes the pane's pty to the new geometry
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        Ok(true)
    }

    /// Apply `geom`, the geometry a *directed* user resize gave `pane_id`, to the panel it forms
    /// with its visible `partner`; `false` => nothing could change. An undirected resize goes to
    /// `resize_panel` instead, since it names no edge for this function to read.
    ///
    /// The half's seam-side edge *is* the seam, so moving it moves the seam: the partner gives up
    /// or takes exactly what the half took or gave, its outer edge never moves, and the seam
    /// stops at the floor a seam drag keeps (`MIN_TERMINAL_WIDTH`) rather than folding the
    /// partner or inverting the strategy, as `geom_resized_within` inverts at the surface's
    /// edge. The half's other edges are the panel's: the partner's rows follow, since the panel
    /// has one top and one bottom, and the outer edge moving is the panel growing or shrinking.
    ///
    /// The claims are the panel's too, read off what happened to the pair's bounding box and
    /// made on both halves. A resize that only moved the seam therefore claims nothing, as
    /// `drag_seam_to` claims nothing (module doc): the split is the owning plugin's to read
    /// back, and a size claim here would freeze a panel whose size did not change. Both halves'
    /// geometry is written back as `Fixed` either way, so `reapply_coordinates` keeps the split.
    fn resize_half_of_panel(
        &mut self,
        pane_id: PaneId,
        partner: PaneId,
        before: PaneGeom,
        mut geom: PaneGeom,
    ) -> bool {
        let Some((left, _)) = self.link_of(&pane_id) else {
            return false;
        };
        let Some(partner_before) = self.get(&partner).map(|pane| pane.position_and_size()) else {
            return false;
        };
        let mut partner_geom = partner_before;
        let floor = MIN_TERMINAL_WIDTH;
        if pane_id == left {
            // the seam is this half's right edge; the partner keeps its right edge and at least
            // the floor, and the seam is never pushed further than it already was
            let partner_right = partner_before.x + partner_before.cols.as_usize();
            let seam_at_most = partner_right
                .saturating_sub(floor)
                .max(before.x + before.cols.as_usize());
            let seam = (geom.x + geom.cols.as_usize()).min(seam_at_most);
            geom.cols.set_inner(seam - geom.x);
            partner_geom.x = seam;
            partner_geom.cols.set_inner(partner_right - seam);
        } else {
            // the seam is this half's left edge; the partner keeps its left edge and at least
            // the floor, and this half keeps its right edge
            let seam_at_least = (partner_before.x + floor).min(before.x);
            let right = geom.x + geom.cols.as_usize();
            let seam = geom.x.max(seam_at_least);
            geom.x = seam;
            geom.cols.set_inner(right - seam);
            partner_geom.cols.set_inner(seam - partner_before.x);
        }
        partner_geom.y = geom.y;
        partner_geom.rows.set_inner(geom.rows.as_usize());
        if geom == before && partner_geom == partner_before {
            return false;
        }

        let panel_before = union_of(before, partner_before);
        let panel = union_of(geom, partner_geom);
        self.set_panel_halves(
            panel_before,
            panel,
            [(pane_id, geom), (partner, partner_geom)],
        );
        true
    }

    /// Apply an undirected resize - `strategies` that name no edge - to the panel `pane_id`
    /// forms with its visible `partner`; `false` => nothing could change.
    ///
    /// The resize is the panel's bounding box's, on all four sides, exactly as it would be a
    /// lone overlay's: which half the gesture landed on does not matter, and neither half's
    /// seam-side edge is read as the seam. The seam keeps its share of the panel's width,
    /// carried from the old box onto the new one in proportion as `place_linked_pair` carries a
    /// layout, and never closer than `MIN_TERMINAL_WIDTH` to either outer edge - the floor a seam
    /// drag keeps. A panel already at two of that floor is not shrunk in width at all, only in
    /// height. The claims are the panel's, read off the bounding box and made on both halves.
    fn resize_panel(
        &mut self,
        pane_id: PaneId,
        partner: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> bool {
        let Some((left, right)) = self.link_of(&pane_id) else {
            return false;
        };
        let (Some(left_before), Some(right_before)) = (
            self.get(&left).map(|pane| pane.position_and_size()),
            self.get(&right).map(|pane| pane.position_and_size()),
        ) else {
            return false;
        };
        debug_assert!(partner == left || partner == right);
        let surface = self.surface();
        let floor = MIN_TERMINAL_WIDTH;

        let panel_before = union_of(left_before, right_before);
        let mut panel = panel_before;
        for strategy in strategies {
            panel = geom_resized_within(panel, surface, strategy, change_by);
        }
        if panel.cols.as_usize() < floor * 2 {
            // each half keeps the floor, so the panel is never narrower than two of them: a
            // shrink that would overshoot stops there, centred as the step would have been, and
            // a panel already there gives up no more width
            let cols = panel_before.cols.as_usize().min(floor * 2);
            panel.x = panel_before.x + (panel_before.cols.as_usize() - cols) / 2;
            panel.cols.set_inner(cols);
        }
        if panel == panel_before {
            return false;
        }

        let panel_right = panel.x + panel.cols.as_usize();
        let seam = seam_mapped_onto(
            left_before.x + left_before.cols.as_usize(),
            panel_before,
            panel.x,
            panel.cols.as_usize(),
        );
        let mut left_geom = left_before;
        left_geom.x = panel.x;
        left_geom.y = panel.y;
        left_geom.cols.set_inner(seam - panel.x);
        left_geom.rows.set_inner(panel.rows.as_usize());
        let mut right_geom = right_before;
        right_geom.x = seam;
        right_geom.y = panel.y;
        right_geom
            .cols
            .set_inner(panel_right.saturating_sub(seam).max(1));
        right_geom.rows.set_inner(panel.rows.as_usize());

        self.set_panel_halves(
            panel_before,
            panel,
            [(left, left_geom), (right, right_geom)],
        );
        true
    }

    /// Give the two `halves` of a panel their new geometry, written back as `Fixed`, and make the
    /// panel's claims on both: the size is the user's if the panel's bounding box changed size
    /// from `panel_before` to `panel`, the position if it moved. A resize that changed neither -
    /// one that only moved the seam - claims nothing, as `drag_seam_to` claims nothing.
    fn set_panel_halves(
        &mut self,
        panel_before: PaneGeom,
        panel: PaneGeom,
        halves: [(PaneId, PaneGeom); 2],
    ) {
        let moved = panel.x != panel_before.x || panel.y != panel_before.y;
        let resized = panel.cols.as_usize() != panel_before.cols.as_usize()
            || panel.rows.as_usize() != panel_before.rows.as_usize();
        for (half_id, half_geom) in halves {
            let Some(overlay) = self.panes.get_mut(&half_id) else {
                continue;
            };
            Self::set_geom_fixed(overlay, half_geom);
            overlay.carry.size_is_user_stated |= resized;
            overlay.carry.position_is_user_stated |= moved;
        }
    }

    /// Give `overlay` `geom` and write all four of its coordinates back as `Fixed`, so
    /// `reapply_coordinates` reproduces it - and claim nothing. `Fixed` is not a claim: it is
    /// how a seam drag and a pair placement record geometry the user has not claimed, and a
    /// claim is the caller's to make.
    fn set_geom_fixed(overlay: &mut OverlayPane, geom: PaneGeom) {
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
        overlay.coordinates.y = Some(PercentOrFixed::Fixed(geom.y));
        overlay.coordinates.width = Some(PercentOrFixed::Fixed(geom.cols.as_usize()));
        overlay.coordinates.height = Some(PercentOrFixed::Fixed(geom.rows.as_usize()));
    }

    /// Give `overlay` the geometry a user resize produced, and write it back as the user's:
    /// the size always, the position when the resize moved it.
    fn claim_geom(overlay: &mut OverlayPane, geom: PaneGeom, moved: bool) {
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates.width = Some(PercentOrFixed::Fixed(geom.cols.as_usize()));
        overlay.coordinates.height = Some(PercentOrFixed::Fixed(geom.rows.as_usize()));
        overlay.carry.size_is_user_stated = true;
        if moved {
            // `PaneGeom::apply_floating_pane_position` reads `Fixed(n)` as a screen coordinate,
            // so the position round-trips exactly - the same arithmetic `move_pane_to_position` does
            overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
            overlay.coordinates.y = Some(PercentOrFixed::Fixed(geom.y));
            overlay.carry.position_is_user_stated = true;
        }
    }

    // ---- being expanded over the screen -------------------------------------------------------

    /// Whether an overlay is expanded over the screen right now.
    pub fn fullscreen_is_active(&self) -> bool {
        self.fullscreen.is_some()
    }

    /// The overlay the expansion is anchored to, if any. A linked panel is named by the half the
    /// user pressed on; `fullscreen_pane_ids` is what answers with the whole panel.
    pub fn fullscreen_pane_id(&self) -> Option<PaneId> {
        self.fullscreen.map(|(pane_id, _)| pane_id)
    }

    /// Whether the expansion covers the tab bar and the status bar as well as the panes.
    pub fn fullscreen_covers_ui(&self) -> bool {
        self.fullscreen.is_some_and(|(_, covers_ui)| covers_ui)
    }

    /// Whether `pane_id` is drawn expanded - the anchor, or the other half of its panel.
    pub fn pane_is_fullscreen(&self, pane_id: &PaneId) -> bool {
        self.fullscreen_pane_ids().contains(pane_id)
    }

    /// The panes the expansion is drawing over the screen: one overlay, or both halves of a
    /// linked panel. Empty when nothing is expanded.
    ///
    /// Derived from the link *now* rather than remembered from the keystroke, which is what makes
    /// the expansion survive its panel being re-cut, re-linked or unlinked underneath it - see
    /// `refresh_fullscreen`.
    pub fn fullscreen_pane_ids(&self) -> Vec<PaneId> {
        let Some((pane_id, _)) = self.fullscreen else {
            return vec![];
        };
        match self.link_of(&pane_id) {
            Some((left, right)) if !self.is_hidden(&left) && !self.is_hidden(&right) => {
                vec![left, right]
            },
            // A pair with a hidden half is the shown half taking the whole panel, which is how
            // every other placement already reads that shape.
            _ => vec![pane_id],
        }
    }

    /// The area an expansion fills: the tab's own viewport, or - `covers_ui` - the whole surface.
    ///
    /// The viewport rather than the surface for the plain keystroke, so that `Ctrl+p f` on an
    /// overlay leaves the tab bar and the status bar where `Ctrl+p f` on a tiled pane leaves
    /// them. An overlay may be *drawn* over the chrome, but being able to fill the screen without
    /// losing the chrome is the whole difference between the two keystrokes.
    fn fullscreen_area(&self, covers_ui: bool) -> Viewport {
        if covers_ui {
            self.surface()
        } else {
            *self.viewport.borrow()
        }
    }

    /// Whether `pane_id` may be expanded at all.
    ///
    /// A modal is refused for the reason it is not resizable: it is drawn because something needs
    /// an answer now and it is sized to that question. A hidden pane and one still awaiting its
    /// placement are refused because neither is on screen to expand.
    ///
    /// Spelled against `is_modal` rather than `is_resizable`, which an expanded overlay already
    /// answers `false` to: this is also the test `refresh_fullscreen` re-applies on every frame,
    /// and reading resizability there would have the expansion drop itself the moment it began.
    fn may_be_fullscreen(&self, pane_id: &PaneId) -> bool {
        !self.is_modal(pane_id)
            && self
                .panes
                .get(pane_id)
                .is_some_and(|overlay| !overlay.carry.hidden && !overlay.carry.awaiting_placement)
    }

    /// The geometry each member of the expansion is drawn with, against the area as it is now.
    fn fullscreen_geoms(&self) -> Vec<(PaneId, PaneGeom)> {
        let Some((_, covers_ui)) = self.fullscreen else {
            return vec![];
        };
        let area = self.fullscreen_area(covers_ui);
        // An area with no width has no seam to place either: `seam_mapped_onto` clamps its
        // answer to at least one column in, which is already past the right edge, and the right
        // half's width below would underflow. Nothing can be expanded into a surface this size.
        if area.cols == 0 || area.rows == 0 {
            return vec![];
        }
        let expanded = |geom: PaneGeom, x: usize, cols: usize| {
            let mut expanded = geom;
            expanded.x = x;
            expanded.y = area.y;
            expanded.cols.set_inner(cols);
            expanded.rows.set_inner(area.rows);
            expanded
        };
        match self.fullscreen_pane_ids().as_slice() {
            // A linked pair is one panel to every gesture, and this is a gesture: both halves
            // grow, and the divider lands at the share of the panel it had - the same
            // proportional step an undirected resize of the panel makes, so an uneven split
            // stays uneven.
            [left, right] => {
                let (Some(left_pane), Some(right_pane)) = (self.get(left), self.get(right)) else {
                    return vec![];
                };
                let left_geom = left_pane.position_and_size();
                let right_geom = right_pane.position_and_size();
                let panel = union_of(left_geom, right_geom);
                let seam = seam_mapped_onto(
                    left_geom.x + left_geom.cols.as_usize(),
                    panel,
                    area.x,
                    area.cols,
                );
                vec![
                    (*left, expanded(left_geom, area.x, seam - area.x)),
                    (
                        *right,
                        expanded(right_geom, seam, (area.x + area.cols).saturating_sub(seam)),
                    ),
                ]
            },
            [pane_id] => match self.get(pane_id) {
                Some(pane) => vec![(
                    *pane_id,
                    expanded(pane.position_and_size(), area.x, area.cols),
                )],
                None => vec![],
            },
            _ => vec![],
        }
    }

    /// Re-derive every expanded geometry from the bases as they are now, and drop the overrides
    /// of everything else.
    ///
    /// This is the whole of the expansion's upkeep, and it is called from `set_pane_frames`,
    /// which every path that touches an overlay's geometry already ends in. So a terminal resize,
    /// a placement batch from the owning plugin, a re-cut divider and a pane swapped into the
    /// slot all keep the expansion filling the screen without any of them knowing it exists -
    /// and none of them can leave a stale override behind, because the second loop below owns
    /// every override in the collection.
    ///
    /// It is also where an expansion that has stopped making sense is dropped: an anchor that was
    /// closed, set aside or is no longer expandable takes the expansion with it.
    fn refresh_fullscreen(&mut self) {
        if self
            .fullscreen
            .is_some_and(|(pane_id, _)| !self.may_be_fullscreen(&pane_id))
        {
            self.fullscreen = None;
        }
        let geoms = self.fullscreen_geoms();
        let members: HashSet<PaneId> = geoms.iter().map(|(pane_id, _)| *pane_id).collect();
        for (pane_id, overlay) in self.panes.iter_mut() {
            if !members.contains(pane_id) && overlay.pane.geom_override().is_some() {
                overlay.pane.reset_size_and_position_override();
            }
        }
        for (pane_id, geom) in geoms {
            if let Some(overlay) = self.panes.get_mut(&pane_id) {
                overlay.pane.set_geom_override(geom);
            }
        }
    }

    /// Expand `pane_id` - or the panel it is half of - over the screen; `true` => it is now.
    ///
    /// The expansion is a geometry *override* and nothing else. Coordinates, the user's claims on
    /// them and the link to a partner are all left as they are underneath, so `unset_fullscreen`
    /// has nothing to restore from a memory of its own: it drops the override and what was always
    /// there comes back - re-resolved against the surface as it is by then, if that changed.
    ///
    /// Focus is deliberately not moved, unlike `FloatingPanes::set_fullscreen`'s
    /// `focus_pane_for_all_clients`: the keyboard path is already on this pane, and a live modal
    /// must not have the keyboard taken from it by someone else's keystroke.
    fn set_fullscreen(&mut self, pane_id: PaneId, covers_ui: bool) -> bool {
        if !self.may_be_fullscreen(&pane_id) {
            return false;
        }
        self.fullscreen = Some((pane_id, covers_ui));
        for member_id in self.fullscreen_pane_ids() {
            // so a sibling overlay opened earlier cannot be painted over the pane the user just
            // asked to see in full. A live modal still outranks it - `raise_within_class` is what
            // says so, and a fullscreen keystroke does not suspend the modal lock.
            self.raise_within_class(member_id);
            // and a gesture in flight on a pane that now fills the screen has nothing left to
            // carry: there is nowhere to move it to and the split it would re-cut is the one the
            // expansion exists to come back to. Released here as `hide_pane` and `remove_pane`
            // release them, rather than refused at every step of the drag.
            if matches!(self.pane_being_moved_with_mouse, Some((dragged, ..)) if dragged == member_id)
            {
                self.pane_being_moved_with_mouse = None;
            }
            self.release_seam_drag_of(member_id);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        true
    }

    /// The frame an expanded overlay wears, matching the floating layer's: none at all when it
    /// covers the chrome, there being nothing beside it for a border to separate it from.
    fn set_fullscreen_frame(
        pane: &mut Box<dyn Pane>,
        pane_frame_style: PaneFrameStyle,
        covers_ui: bool,
    ) {
        if covers_ui || pane.borderless() || !pane_frame_style.draws_full_frames() {
            pane.set_frame(false);
            pane.set_content_offset(Offset::default());
        } else {
            pane.set_frame(true);
            pane.set_content_offset(Offset::frame(1));
        }
    }

    /// Put an expanded overlay back where its coordinates say it belongs; `true` => one was.
    pub fn unset_fullscreen(&mut self) -> bool {
        if self.fullscreen.take().is_none() {
            return false;
        }
        // `set_pane_frames` drops the overrides and puts the frames back: it is the one place
        // that decides what an overlay's geometry and frame are, expanded or not.
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        true
    }

    /// Expand `pane_id` over the viewport, or put it back; `true` => the keystroke was ours.
    pub fn toggle_pane_fullscreen(&mut self, pane_id: PaneId) -> bool {
        self.toggle_fullscreen_of(pane_id, false)
    }

    /// Expand `pane_id` over the whole screen, chrome included, or put it back.
    pub fn toggle_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) -> bool {
        self.toggle_fullscreen_of(pane_id, true)
    }

    /// The two toggles, which differ only in how much they cover. Pressing the *other* one while
    /// expanded switches between the two rather than collapsing, as the floating layer's do.
    ///
    /// Only a press naming the panel that is *already* expanded collapses it. One naming another
    /// overlay puts that panel back and expands the pane it asked for, which is what a keystroke
    /// aimed at a pane means everywhere else - and the alternative was worse than doing nothing:
    /// the expansion was dropped and the requested pane never expanded at all.
    fn toggle_fullscreen_of(&mut self, pane_id: PaneId, covers_ui: bool) -> bool {
        match self.fullscreen {
            Some((_, was_covering_ui)) => {
                // before the collapse, which is what `pane_is_fullscreen` is derived from
                let same_panel = self.pane_is_fullscreen(&pane_id);
                self.unset_fullscreen();
                if same_panel && was_covering_ui == covers_ui {
                    true
                } else {
                    self.set_fullscreen(pane_id, covers_ui)
                }
            },
            None => self.set_fullscreen(pane_id, covers_ui),
        }
    }

    /// This client's overlay, expanded over the viewport or put back; `true` => ours.
    pub fn toggle_active_pane_fullscreen(&mut self, client_id: ClientId) -> bool {
        match self.fullscreen_target(client_id) {
            Some(pane_id) => self.toggle_pane_fullscreen(pane_id),
            None => false,
        }
    }

    /// This client's overlay, expanded over the whole screen or put back; `true` => ours.
    pub fn toggle_active_pane_no_ui_fullscreen(&mut self, client_id: ClientId) -> bool {
        match self.fullscreen_target(client_id) {
            Some(pane_id) => self.toggle_pane_no_ui_fullscreen(pane_id),
            None => false,
        }
    }

    /// What a keystroke from `client_id` is about: the expanded overlay if there is one, else the
    /// overlay this client is on. The expanded one comes first so that a second press collapses
    /// it even if focus moved to another overlay in between - an overlay the user cannot get out
    /// of full screen is the failure this ordering exists to prevent.
    fn fullscreen_target(&self, client_id: ClientId) -> Option<PaneId> {
        self.fullscreen_pane_id()
            .or_else(|| self.active_pane_id(client_id))
    }

    /// Drop the expansion if it is drawing `pane_id`, putting every member back.
    ///
    /// Called where a member stops being something that can be expanded - it is closed, or the
    /// user sets it aside. `refresh_fullscreen` would drop an anchor in that state on its own,
    /// but only at the next `set_pane_frames`, and both those paths need the collection honest
    /// before they go on to move clients off the pane.
    fn release_fullscreen_of(&mut self, pane_id: PaneId) {
        if self.pane_is_fullscreen(&pane_id) {
            self.unset_fullscreen();
        }
    }

    // ---- being dragged with the mouse -------------------------------------------------------

    /// Note that `pane_id` is being dragged, and where the pointer is now.
    ///
    /// The initial position is preserved across calls for the same pane: `stop_moving_pane_with_mouse`
    /// compares it against the release position, which is how a press that never moved is told apart
    /// from a drag and turned back into a plain focus click.
    pub fn set_pane_being_moved_with_mouse(&mut self, pane_id: PaneId, last_position: Position) {
        if let Some((last_pane_id, initial_position)) = self
            .pane_being_moved_with_mouse
            .map(|(pane_id, initial_position, _)| (pane_id, initial_position))
        {
            if last_pane_id == pane_id {
                self.pane_being_moved_with_mouse = Some((pane_id, initial_position, last_position));
                return;
            }
        }
        self.pane_being_moved_with_mouse = Some((pane_id, last_position, last_position));
    }
    pub fn pane_is_being_moved_with_mouse(&self) -> bool {
        self.pane_being_moved_with_mouse.is_some()
    }

    /// Carry the dragged overlay to where the pointer is now, clamped inside the session surface.
    ///
    /// **The move is written back into the overlay's coordinates**, not only onto its geometry.
    /// That is what makes a dragged overlay survive everything that re-resolves one - a terminal
    /// resize, following a client to another tab - without this collection needing a
    /// `desired_pane_positions` of its own: where the user put it is stated in the same terms as
    /// where it was opened. `PaneGeom::apply_floating_pane_position` reads `Fixed(n)` as a screen
    /// coordinate, so the position round-trips exactly.
    ///
    /// `width`/`height` are deliberately left alone. Only the *position* becomes user-stated; a
    /// percentage-sized overlay still grows with the screen after being dragged.
    pub fn move_pane_to_position(&mut self, click_position: &Position) -> bool {
        // true => changed position
        let surface = self.surface();
        let Some((pane_id, _initial_position, previous_position)) =
            self.pane_being_moved_with_mouse
        else {
            return false;
        };
        if click_position == &previous_position {
            return false;
        }
        if self.pane_is_fullscreen(&pane_id) {
            // the drag began before the expansion did - `move_pane_with_mouse` refuses to start
            // one on a pane that fills the screen, and `set_fullscreen` releases one in flight,
            // but neither covers a caller stepping a drag it holds the state for itself
            return false;
        }
        let move_x_by = click_position.column() as isize - previous_position.column() as isize;
        let move_y_by = click_position.line() - previous_position.line();
        let Some(geom) = self.get(&pane_id).map(|pane| pane.position_and_size()) else {
            return false;
        };
        // A linked partner moves by the same delta, so the panel moves as one - and the delta is
        // clamped against the pair's bounding rectangle rather than against the dragged half
        // alone, so neither half is carried off the surface. A hidden partner comes too: it is
        // placed at the share it will come back to, inside the panel, and the reveal maps the
        // panel's layout onto the pair's bounding box - which a half left behind would stretch.
        let partner = self.linked_partner(&pane_id);
        let bounds = match partner.and_then(|partner| self.get(&partner)) {
            Some(partner_pane) => union_of(geom, partner_pane.position_and_size()),
            None => geom,
        };
        let moved_bounds = geom_moved_within(bounds, surface, move_x_by, move_y_by);
        let (move_x_by, move_y_by) = (
            moved_bounds.x as isize - bounds.x as isize,
            moved_bounds.y as isize - bounds.y as isize,
        );
        for moved_id in std::iter::once(pane_id).chain(partner) {
            let Some(overlay) = self.panes.get_mut(&moved_id) else {
                continue;
            };
            let mut geom = overlay.pane.position_and_size();
            geom.x = (geom.x as isize + move_x_by).max(0) as usize;
            geom.y = (geom.y as isize + move_y_by).max(0) as usize;
            overlay.pane.set_geom(geom);
            overlay.pane.set_should_render(true);
            overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
            overlay.coordinates.y = Some(PercentOrFixed::Fixed(geom.y));
            overlay.carry.position_is_user_stated = true;
        }
        self.set_pane_being_moved_with_mouse(pane_id, *click_position);
        self.set_force_render();
        true
    }

    /// Start or continue a frame drag at `position`; `true` => the event was ours.
    ///
    /// Unlike the floating twin there is no fullscreen check and no layer-visibility precondition:
    /// an overlay is drawn for as long as it exists, so there is no state in which its frame is on
    /// screen but not draggable. And the pane is resolved through this collection's own hit test,
    /// which is what makes this safe where the floating path was not - `visible_pane_id_at` cannot
    /// return a pane the overlay is covering.
    pub fn move_pane_with_mouse(&mut self, position: Position) -> bool {
        if self.pane_being_moved_with_mouse.is_some() {
            return self.move_pane_to_position(&position);
        }
        let Some(pane_id) = self.visible_pane_id_at(&position) else {
            return false;
        };
        if self.pane_is_fullscreen(&pane_id) {
            // an expanded overlay fills the screen, so there is nowhere to move it to, and the
            // rectangle a drag would write is the one it comes back to
            return false;
        }
        let on_frame = self
            .panes
            .get(&pane_id)
            .is_some_and(|overlay| overlay.pane.position_is_on_frame(&position));
        if !on_frame {
            // a borderless overlay has no frame to grab, exactly as a borderless floating pane
            // has none
            return false;
        }
        self.set_pane_being_moved_with_mouse(pane_id, position);
        self.move_pane_to_position(&position);
        self.set_force_render();
        true
    }

    /// End the drag; `true` => the pointer never left the cell it was pressed on, so the caller
    /// should treat the press as an ordinary focus click.
    pub fn stop_moving_pane_with_mouse(&mut self, position: Position) -> bool {
        let mut never_moved = false;
        if let Some((pane_id, initial_position, last_position)) =
            self.pane_being_moved_with_mouse.take()
        {
            if initial_position == position {
                never_moved = true;
            } else {
                // put the state back so the final step actually lands: the release carries its own
                // position, and it is the authoritative one. (The floating twin `take()`s first and
                // then calls its mover, which returns early on the `None` it just created - so its
                // last motion is the last move. Harmless there, because a release is preceded by a
                // motion at the same cell, but not worth copying.)
                self.pane_being_moved_with_mouse = Some((pane_id, initial_position, last_position));
                self.move_pane_to_position(&position);
                self.pane_being_moved_with_mouse = None;
            }
            self.set_force_render();
        }
        self.pane_being_moved_with_mouse = None;
        never_moved
    }

    // ---- linked panels and the seam between them ---------------------------------------------

    /// Link every pair among `pane_ids` that a placement has put together - edge to edge, the
    /// right one starting on the column where the left one ends, or with one half hidden and its
    /// share lying inside the other (`placed_together`) - and forget any earlier link naming one
    /// of them.
    ///
    /// Called by `Tab` once a `change_overlay_coordinates` batch has been applied, and by the
    /// cross-tab move, which carries a batch of panes at once: "placed together" is the whole
    /// of the test, so a batch naming one pane leaves every link alone, and a batch that places
    /// two visible panes apart unlinks them. A seam drag in flight on a pair the batch keeps
    /// together is left in flight: re-placing the panel under the pointer is no reason to drop
    /// the divider.
    ///
    /// A pane is in at most one pair (see `linked`). A batch that lands a pane against two
    /// others - three overlays in a row - links it with the first of them in the batch's order,
    /// and the pair that would name it again is not made.
    pub fn link_touching(&mut self, pane_ids: &[PaneId]) {
        if pane_ids.len() < 2 {
            return;
        }
        for pane_id in pane_ids {
            self.forget_links_of(*pane_id);
        }
        for (index, a) in pane_ids.iter().enumerate() {
            for b in &pane_ids[index + 1..] {
                // every link naming a batch member was just forgotten, so a pane already in a
                // pair got there earlier in this batch, and that pair is the one it keeps
                if self.link_of(a).is_some() || self.link_of(b).is_some() {
                    continue;
                }
                if let Some(pair) = self.placed_together(a, b) {
                    self.linked.push(pair);
                }
            }
        }
        if let Some((left, right, _)) = self.seam_being_dragged {
            if !self.linked.contains(&(left, right)) {
                self.seam_being_dragged = None;
            }
        }
    }

    /// Whether `a` and `b` form a panel, and in which order: `Some((left, right))` when they sit
    /// edge to edge, or when one is hidden and lies inside the other - the shape a placement
    /// with one half put away has, the hidden half kept at the share it will come back to. In
    /// that shape the left half is the one starting further left, and among two starting on the
    /// same column the one ending first, which is the hidden share at the panel's left edge.
    fn placed_together(&self, a: &PaneId, b: &PaneId) -> Option<(PaneId, PaneId)> {
        if self.touch_edge_to_edge(a, b) {
            return Some((*a, *b));
        }
        if self.touch_edge_to_edge(b, a) {
            return Some((*b, *a));
        }
        if !(self.is_hidden(a) || self.is_hidden(b)) {
            return None;
        }
        let (Some(geom_a), Some(geom_b)) = (
            self.get(a).map(|pane| pane.position_and_size()),
            self.get(b).map(|pane| pane.position_and_size()),
        ) else {
            return None;
        };
        let (hidden, shown) = match self.is_hidden(a) {
            true => (geom_a, geom_b),
            false => (geom_b, geom_a),
        };
        let inside = hidden.x >= shown.x
            && hidden.y >= shown.y
            && hidden.x + hidden.cols.as_usize() <= shown.x + shown.cols.as_usize()
            && hidden.y + hidden.rows.as_usize() <= shown.y + shown.rows.as_usize();
        if !inside {
            return None;
        }
        let key = |geom: PaneGeom| (geom.x, geom.x + geom.cols.as_usize());
        match key(geom_a) <= key(geom_b) {
            true => Some((*a, *b)),
            false => Some((*b, *a)),
        }
    }

    /// Drop every link naming `pane_id`, and a seam drag holding it: a seam is drawn between two
    /// panes, and one of them going away leaves nothing to drag.
    fn unlink(&mut self, pane_id: PaneId) {
        self.forget_links_of(pane_id);
        self.release_seam_drag_of(pane_id);
    }

    /// Drop every link naming `pane_id`. The seam drag, if any, is the caller's to settle:
    /// `link_touching` may be about to make the same link again.
    fn forget_links_of(&mut self, pane_id: PaneId) {
        self.linked
            .retain(|(left, right)| *left != pane_id && *right != pane_id);
    }

    /// End a seam drag holding `pane_id`, if one is in flight.
    fn release_seam_drag_of(&mut self, pane_id: PaneId) {
        if matches!(self.seam_being_dragged, Some((left, right, _)) if left == pane_id || right == pane_id)
        {
            self.seam_being_dragged = None;
        }
    }

    /// The `(left, right)` pair `pane_id` is a member of, if any.
    pub fn link_of(&self, pane_id: &PaneId) -> Option<(PaneId, PaneId)> {
        self.linked
            .iter()
            .find(|(left, right)| left == pane_id || right == pane_id)
            .copied()
    }

    /// The other half of the panel `pane_id` is in, if it is in one.
    pub fn linked_partner(&self, pane_id: &PaneId) -> Option<PaneId> {
        self.link_of(pane_id)
            .map(|(left, right)| if left == *pane_id { right } else { left })
    }

    /// Whether `right` starts on the column `left` ends on, with their rows overlapping.
    fn touch_edge_to_edge(&self, left: &PaneId, right: &PaneId) -> bool {
        let (Some(left), Some(right)) = (self.get(left), self.get(right)) else {
            return false;
        };
        let (left, right) = (left.position_and_size(), right.position_and_size());
        left.x + left.cols.as_usize() == right.x
            && left.y < right.y + right.rows.as_usize()
            && right.y < left.y + left.rows.as_usize()
    }

    /// The linked pair whose seam is under `position`: the right frame column of the left pane
    /// or the left frame column of the right pane, both panes visible and still touching.
    ///
    /// Resolved through this collection's own hit test, as the frame drag is, so a seam is
    /// never found underneath an overlay covering it.
    pub fn seam_at(&self, position: &Position) -> Option<(PaneId, PaneId)> {
        let pane_id = self.visible_pane_id_at(position)?;
        // An expanded panel's divider is drawn from an override, while the seam arithmetic below
        // reads the bases: the two disagree, and a drag would write a base the user cannot see
        // back as the user's. The panel is re-cut when it comes back, not while it is expanded.
        if self.pane_is_fullscreen(&pane_id) {
            return None;
        }
        if !self.get(&pane_id)?.position_is_on_frame(position) {
            return None;
        }
        let (left, right) = self.link_of(&pane_id)?;
        if self.is_hidden(&left)
            || self.is_hidden(&right)
            || !self.touch_edge_to_edge(&left, &right)
        {
            return None;
        }
        let left_geom = self.get(&left)?.position_and_size();
        let right_geom = self.get(&right)?.position_and_size();
        let column = position.column();
        let on_seam =
            column + 1 == left_geom.x + left_geom.cols.as_usize() || column == right_geom.x;
        on_seam.then_some((left, right))
    }

    /// Note that the seam between `left` and `right` is being dragged, from `position`.
    ///
    /// What is remembered is where the pointer took hold of the seam *relative to it* - the seam
    /// is two frame columns wide and the press may be on either - so that every later step can
    /// put the seam back under the pointer rather than move it by the pointer's last delta.
    pub fn start_seam_drag(&mut self, left: PaneId, right: PaneId, position: Position) {
        let Some(left_geom) = self.get(&left).map(|pane| pane.position_and_size()) else {
            return;
        };
        let seam_column = (left_geom.x + left_geom.cols.as_usize()) as isize - 1;
        let grab_offset = seam_column - position.column() as isize;
        self.seam_being_dragged = Some((left, right, grab_offset));
    }
    pub fn seam_is_being_dragged(&self) -> bool {
        self.seam_being_dragged.is_some()
    }

    /// Move the dragged seam to where the pointer is now; `true` => the split changed.
    ///
    /// The left half grows by what the right half gives up, or the other way round, and the
    /// panel's outer edges never move. Each half keeps the floor a resize keeps
    /// (`MIN_TERMINAL_WIDTH`), so the pointer running past it leaves the seam at the floor rather
    /// than folding one half to nothing - and because the seam is placed from the pointer's
    /// absolute column, it stays at the floor until the pointer comes back to it, rather than
    /// pulling away the moment the pointer turns round.
    ///
    /// Both halves' `x`/`width` are written back as `Fixed`, so `reapply_coordinates` keeps the
    /// split through a terminal resize - and **no claim is set**: see the module doc.
    pub fn drag_seam_to(&mut self, position: &Position) -> bool {
        let Some((left, right, grab_offset)) = self.seam_being_dragged else {
            return false;
        };
        // An expanded panel's divider is drawn from an override while the arithmetic below reads
        // the bases, so a step here would re-cut a panel the user cannot see and write it back as
        // `Fixed`. `seam_at` refuses to *start* such a drag; this refuses to continue one that
        // was already in flight when the expansion began.
        if self.pane_is_fullscreen(&left) || self.pane_is_fullscreen(&right) {
            return false;
        }
        let (Some(left_geom), Some(right_geom)) = (
            self.get(&left).map(|pane| pane.position_and_size()),
            self.get(&right).map(|pane| pane.position_and_size()),
        ) else {
            return false;
        };
        let left_cols = left_geom.cols.as_usize();
        let total = left_cols + right_geom.cols.as_usize();
        let floor = MIN_TERMINAL_WIDTH;
        if total < floor * 2 {
            return false;
        }
        // the seam column is the left half's last column, so the left half is one wider than
        // the seam's offset from its own first column
        let seam_column = position.column() as isize + grab_offset;
        let new_left_cols = (seam_column - left_geom.x as isize + 1)
            .clamp(floor as isize, (total - floor) as isize) as usize;
        if new_left_cols == left_cols {
            return false;
        }
        if let Some(overlay) = self.panes.get_mut(&left) {
            let mut geom = overlay.pane.position_and_size();
            geom.cols.set_inner(new_left_cols);
            overlay.pane.set_geom(geom);
            overlay.pane.set_should_render(true);
            overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
            overlay.coordinates.width = Some(PercentOrFixed::Fixed(new_left_cols));
        }
        if let Some(overlay) = self.panes.get_mut(&right) {
            let mut geom = overlay.pane.position_and_size();
            geom.x = left_geom.x + new_left_cols;
            geom.cols.set_inner(total - new_left_cols);
            overlay.pane.set_geom(geom);
            overlay.pane.set_should_render(true);
            overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
            overlay.coordinates.width = Some(PercentOrFixed::Fixed(total - new_left_cols));
        }
        // Only the two halves changed size, so only their ptys are told - a drag is one event
        // per pointer cell, and `set_pane_frames` would resize every overlay on the tab for each.
        self.resize_ptys_of(&[left, right]).non_fatal();
        self.set_force_render();
        true
    }

    /// Tell the ptys of `pane_ids` their panes' current sizes.
    fn resize_ptys_of(&mut self, pane_ids: &[PaneId]) -> Result<()> {
        for pane_id in pane_ids {
            let Some(overlay) = self.panes.get_mut(pane_id) else {
                continue;
            };
            let pane = &mut overlay.pane;
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(|| format!("failed to resize overlay {pane_id:?}"))?;
        }
        Ok(())
    }

    /// End the seam drag, landing the final step at the release position.
    pub fn stop_seam_drag(&mut self, position: Position) {
        self.drag_seam_to(&position);
        self.seam_being_dragged = None;
    }

    /// The absolute geometries of the overlays owning cells outside the tab's own viewport - which,
    /// being drawn over the session, is most of them. Nothing but this tab paints those cells.
    /// Hidden overlays are left out of both of these: they own no cells, and counting one would
    /// keep `Tab::has_floating_panes_outside_viewport` true forever with nothing to paint.
    pub fn geoms_outside_viewport(&self) -> HashMap<PaneId, PaneGeom> {
        let viewport = *self.viewport.borrow();
        self.panes
            .iter()
            .filter(|(_, overlay)| !overlay.carry.hidden)
            .filter_map(|(pane_id, overlay)| {
                // the geometry it is *drawn* with: an overlay expanded over the whole screen owns
                // the chrome's cells, and this is the question of which cells are owned
                let geom = overlay.pane.current_geom();
                geom_is_outside_viewport(&geom, &viewport).then_some((*pane_id, geom))
            })
            .collect()
    }
    pub fn has_panes_outside_viewport(&self) -> bool {
        let viewport = *self.viewport.borrow();
        self.panes
            .values()
            .filter(|overlay| !overlay.carry.hidden)
            .any(|overlay| geom_is_outside_viewport(&overlay.pane.current_geom(), &viewport))
    }

    pub fn update_pane_themes(&mut self, theme: Styling) {
        self.style.colors = theme;
        for overlay in self.panes.values_mut() {
            overlay.pane.update_theme(theme);
        }
    }
    pub fn update_pane_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        for overlay in self.panes.values_mut() {
            overlay.pane.update_arrow_fonts(should_support_arrow_fonts);
        }
    }
    pub fn update_pane_kitty_host_support(&mut self, supported: KittyHostSupport) {
        for overlay in self.panes.values_mut() {
            overlay.pane.update_kitty_host_support(supported);
        }
    }
    pub fn update_pane_sixel_host_support(&mut self, supported: bool) {
        for overlay in self.panes.values_mut() {
            overlay.pane.update_sixel_host_support(supported);
        }
    }
    pub fn update_pane_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        for overlay in self.panes.values_mut() {
            overlay.pane.update_rounded_corners(rounded_corners);
        }
    }
    pub fn set_pane_frame_style(&mut self, pane_frame_style: PaneFrameStyle) {
        self.pane_frame_style = pane_frame_style;
    }
    pub fn set_client_dimmed(&mut self, client_id: ClientId, dimmed: bool) {
        if dimmed {
            self.dimmed_clients.insert(client_id);
        } else {
            self.dimmed_clients.remove(&client_id);
        }
        self.set_force_render();
    }
    pub fn set_force_render(&mut self) {
        for overlay in self.panes.values_mut() {
            overlay.pane.set_should_render(true);
            overlay.pane.set_should_render_boundaries(true);
            overlay.pane.render_full_viewport();
        }
    }
    /// Repaint only the panes overlapping `damage` - the cells an overlay stopped covering.
    ///
    /// The narrowed twin of `set_force_render` above. A toast sits in a corner and overlaps a pane
    /// or two, so repainting the whole container every frame of a slide-out - which is what the
    /// unnarrowed version did - re-serializes grids that never had a toast anywhere near them.
    pub fn force_render_panes_intersecting(&mut self, damage: &[CellRect]) {
        for overlay in self.panes.values_mut() {
            let geom = overlay.pane.current_geom();
            if !damage.iter().any(|rect| rect.intersects_geom(&geom)) {
                continue;
            }
            overlay.pane.set_should_render(true);
            overlay.pane.set_should_render_boundaries(true);
            overlay.pane.render_full_viewport();
        }
    }
    /// Every overlay's frame, its content offset and its pty size, from its geometry.
    ///
    /// The one place that decides what an overlay's geometry *is*, expansion included: every path
    /// that moves, resizes, places or replaces an overlay ends here, so `refresh_fullscreen`
    /// running first is what keeps an expanded overlay expanded across all of them - and what
    /// stops the frame loop below putting a border back on one that covers the chrome.
    pub fn set_pane_frames(&mut self) -> Result<()> {
        let err_context =
            |pane_id: &PaneId| format!("failed to activate frame on overlay {pane_id:?}");
        self.refresh_fullscreen();
        let fullscreen: HashSet<PaneId> = self.fullscreen_pane_ids().into_iter().collect();
        let covers_ui = self.fullscreen_covers_ui();
        let pane_frame_style = self.pane_frame_style;
        for (pane_id, overlay) in self.panes.iter_mut() {
            let pane = &mut overlay.pane;
            if fullscreen.contains(pane_id) {
                Self::set_fullscreen_frame(pane, pane_frame_style, covers_ui);
            } else if !pane.borderless() {
                pane.set_frame(true);
                pane.set_content_offset(Offset::frame(1));
            } else {
                pane.set_content_offset(Offset::default());
            }
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(|| err_context(&pane.pid()))?;
        }
        Ok(())
    }
    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        for overlay in self.panes.values_mut() {
            let pane = &mut overlay.pane;
            pane.character_cell_size_possibly_changed();
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(|| format!("failed to resize PTY in overlay {:?}", pane.pid()))?;
        }
        Ok(())
    }

    // ---- mouse ----------------------------------------------------------------------------

    /// The topmost overlay actually being painted at `position`, if any.
    pub fn visible_pane_id_at(&self, position: &Position) -> Option<PaneId> {
        self.stack_layer_ids()
            .iter()
            .rev()
            .find(|pane_id| {
                self.get(pane_id)
                    .is_some_and(|pane| pane.contains(position))
            })
            .copied()
    }
    pub fn get_pane_id_at(&self, position: &Position) -> Option<PaneId> {
        self.visible_pane_id_at(position)
    }

    // ---- render ---------------------------------------------------------------------------

    /// Draw every overlay, bottom-most first.
    ///
    /// `z_index_offset` is how many layers the tab has already put into the flat
    /// `FloatingPanesStack` before these - the floating panes. A pane's z-index is its absolute
    /// position in that one list, so without the offset every overlay would be clipped against
    /// some floating pane's geometry instead of its own.
    pub fn render(
        &mut self,
        output: &mut Output,
        mouse_hover_pane_id: &HashMap<ClientId, PaneId>,
        current_pane_group: HashMap<ClientId, Vec<PaneId>>,
        client_id_override: Option<ClientId>,
        help_text_visible: &HashMap<ClientId, bool>,
        mouse_scroll_resize: bool,
        z_index_offset: usize,
    ) -> Result<()> {
        let err_context = || "failed to render overlays";

        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }
        let connected_clients: Vec<ClientId> = connected_clients.into_iter().collect();

        // no *layer* visibility filter: an overlay keeps its focus for as long as it exists, which
        // is the whole reason it is not a floating pane. A pane `hide_pane` set aside has already
        // had its clients moved off it, so it contributes nothing here either.
        let mut active_panes = self.active_panes.clone_active_panes();

        // This loop feeds `output.add_pane_contents` and does not derive from `stack_layer_ids`,
        // so it needs the hidden filter of its own - otherwise a hidden overlay's contents reach
        // the render report with no geometry ever drawn for them. Same for an overlay still
        // waiting to be placed, whose geometry is the centered default it was born with.
        for (pane_id, overlay) in self
            .panes
            .iter()
            .filter(|(_, overlay)| !overlay.carry.hidden && !overlay.carry.awaiting_placement)
        {
            match pane_id {
                PaneId::Terminal(_) => {
                    output.add_pane_contents(
                        &connected_clients,
                        overlay.pane.pid(),
                        overlay.pane.pane_contents(None, false, None),
                    );
                    if output.collect_ansi_pane_contents {
                        output.add_pane_contents_with_ansi(
                            &connected_clients,
                            overlay.pane.pid(),
                            overlay.pane.pane_contents_with_ansi(None, false, None),
                        );
                    }
                },
                PaneId::Plugin(_) => {
                    for client_id in &connected_clients {
                        output.add_pane_contents(
                            &[*client_id],
                            overlay.pane.pid(),
                            overlay.pane.pane_contents(Some(*client_id), false, None),
                        );
                        if output.collect_ansi_pane_contents {
                            output.add_pane_contents_with_ansi(
                                &[*client_id],
                                overlay.pane.pid(),
                                overlay
                                    .pane
                                    .pane_contents_with_ansi(Some(*client_id), false, None),
                            );
                        }
                    }
                },
            }
        }

        let multiple_users_exist_in_session = { self.connected_clients_in_app.borrow().len() > 1 };
        let style = self.style;
        let session_is_mirrored = self.session_is_mirrored;
        let dimmed_clients = self.dimmed_clients.clone();
        let default_mode_info = self.default_mode_info.clone();
        let client_modes: HashMap<ClientId, _> = connected_clients
            .iter()
            .map(|client_id| {
                let mode = self
                    .mode_info
                    .borrow()
                    .get(client_id)
                    .unwrap_or(&default_mode_info)
                    .mode;
                (*client_id, mode)
            })
            .collect();
        active_panes.retain(|c_id, _| self.connected_clients.borrow().contains(c_id));

        let fullscreen: HashSet<PaneId> = self.fullscreen_pane_ids().into_iter().collect();
        // derived from `stack_layer_ids` rather than from a second filter over `self.panes`, so
        // the index a pane is drawn with is always its index in the occlusion stack
        for (index, pane_id) in self.stack_layer_ids().into_iter().enumerate() {
            let Some(overlay) = self.panes.get_mut(&pane_id) else {
                continue;
            };
            let kind = overlay.kind;
            let pane = &mut overlay.pane;
            let pane_is_selectable = pane.selectable();
            let pane_has_guest_modal = pane.has_guest_modal_for_any_client();
            // +1 because 0 is reserved for non-floating panes
            let z_index = Some(z_index_offset + index + 1);
            // The frame's help text is nothing but the resize hint, and neither a modal nor an
            // overlay expanded over the screen is resizable (`is_resizable`), so withholding it is
            // exactly the right amount of silence: an overlay must not offer a
            // `Ctrl <MouseScroll>` / `Ctrl <drag borders>` that both paths then refuse. Computed
            // before the loop rather than from `self.is_resizable` inside it, which would want
            // `self` back while `overlay` still holds it.
            let is_resizable =
                !matches!(kind, OverlayKind::Modal) && !fullscreen.contains(&pane_id);
            let show_help_text = is_resizable
                && active_panes.iter().any(|(client_id, active_id)| {
                    active_id == &pane.pid()
                        && help_text_visible.get(client_id).copied().unwrap_or(false)
                });

            let mut pane_contents_and_ui = PaneContentsAndUi::new(
                pane,
                output,
                style,
                &active_panes,
                multiple_users_exist_in_session,
                z_index,
                false,
                false,
                true,
                mouse_hover_pane_id,
                current_pane_group.clone(),
                show_help_text,
                false,
                mouse_scroll_resize,
                dimmed_clients.clone(),
            );
            for client_id in &connected_clients {
                let client_mode = client_modes
                    .get(client_id)
                    .copied()
                    .unwrap_or(default_mode_info.mode);
                let is_floating = true;
                pane_contents_and_ui
                    .render_pane_frame(
                        *client_id,
                        client_mode,
                        session_is_mirrored,
                        is_floating,
                        pane_is_selectable,
                        // an overlay is never pinned - there is no layer for it to be pinned above
                        false,
                        // a modal has no scrollback worth advertising, and the indication would
                        // read as an invitation to scroll away from a pane that cannot be left
                        !kind.is_modal(),
                    )
                    .with_context(err_context)?;
                if let PaneId::Plugin(..) = pane_id {
                    pane_contents_and_ui
                        .render_pane_contents_for_client(*client_id)
                        .with_context(err_context)?;
                }
                pane_contents_and_ui.render_terminal_title_if_needed(
                    *client_id,
                    client_mode,
                    &mut self.window_title,
                );
                pane_contents_and_ui
                    .render_fake_cursor_if_needed(*client_id)
                    .with_context(err_context)?;
            }
            if let PaneId::Terminal(..) = pane_id {
                if pane_has_guest_modal {
                    let (modal_clients, plain_clients): (Vec<ClientId>, Vec<ClientId>) =
                        connected_clients.iter().copied().partition(|client_id| {
                            pane_contents_and_ui.client_has_guest_modal(*client_id)
                        });
                    if !plain_clients.is_empty() {
                        pane_contents_and_ui
                            .render_pane_contents_to_multiple_clients(plain_clients.iter().copied())
                            .with_context(err_context)?;
                    } else {
                        pane_contents_and_ui.drain_pane_render_state();
                    }
                    for client_id in modal_clients {
                        pane_contents_and_ui
                            .render_guest_modal_for_client(client_id)
                            .with_context(err_context)?;
                    }
                } else {
                    pane_contents_and_ui
                        .render_pane_contents_to_multiple_clients(connected_clients.iter().copied())
                        .with_context(err_context)?;
                }
            }
        }
        Ok(())
    }
}

/// `edge`, a coordinate inside the span `from .. from + from_len`, carried in proportion into the
/// span `to .. to + to_len` - how `place_linked_pair` maps a requested layout onto the panel the
/// user left. A span of no length maps everything onto `to`.
fn map_edge(edge: usize, from: usize, from_len: usize, to: usize, to_len: usize) -> usize {
    if from_len == 0 {
        return to;
    }
    to + edge.saturating_sub(from) * to_len / from_len
}

/// Where the divider of a panel goes when the panel becomes the rectangle `x .. x + cols`: at the
/// share of the width it had, and never closer than `MIN_TERMINAL_WIDTH` to either outer edge.
///
/// The one answer to "the seam keeps its share", shared by the undirected resize of a panel and
/// by a panel expanded over the screen, so a divider cannot land in two different places
/// depending on which gesture grew the panel.
fn seam_mapped_onto(seam_before: usize, panel_before: PaneGeom, x: usize, cols: usize) -> usize {
    // a panel narrower than two floors (a tiny surface) is cut evenly rather than not at all
    let half_floor = MIN_TERMINAL_WIDTH.min(cols / 2).max(1);
    let seam_at_least = x + half_floor;
    let seam_at_most = (x + cols).saturating_sub(half_floor).max(seam_at_least);
    map_edge(
        seam_before,
        panel_before.x,
        panel_before.cols.as_usize(),
        x,
        cols,
    )
    .clamp(seam_at_least, seam_at_most)
}

/// The smallest rectangle holding both geometries - what a linked pair is clamped against when
/// it is dragged as one, and the panel a linked pair is placed into. Everything but position and
/// size is `a`'s.
fn union_of(a: PaneGeom, b: PaneGeom) -> PaneGeom {
    let mut bounds = a;
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = (a.x + a.cols.as_usize()).max(b.x + b.cols.as_usize());
    let bottom = (a.y + a.rows.as_usize()).max(b.y + b.rows.as_usize());
    bounds.x = x;
    bounds.y = y;
    bounds.cols.set_inner(right - x);
    bounds.rows.set_inner(bottom - y);
    bounds
}

#[cfg(test)]
#[path = "./unit/overlay_panes_tests.rs"]
mod overlay_panes_tests;
