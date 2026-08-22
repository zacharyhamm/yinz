//! Panes drawn over the whole session surface, above the global panes.
//!
//! An overlay is *not* on a tab's floating layer. It is a pane the session puts in front of
//! everything - `file-finder`, `hocus-ask`, `quit-confirmation`, `hocus-new-agent`. A modal
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
//! - **No `show_panes`.** There is no layer to raise or lower: an overlay is drawn for as long as
//!   it exists. The entire bug class this collection was made to remove came from code that read
//!   "floating layer down" as "nothing on it is focused".
//! - **No `fullscreen_pane_id`.** What a fullscreen overlay would mean is undefined - an overlay
//!   is drawn over the whole session surface already. It is not reachable from here, and since
//!   Phase 4 `Tab`'s fullscreen paths refuse an overlay outright rather than falling through to
//!   the tiled collection, which used to fullscreen the pane *behind* the overlay.
//! - **No `desired_pane_positions`.** A floating pane remembers where the user dragged it in a
//!   second map beside the panes. An overlay has no such history *beside* itself: a mouse drag
//!   rewrites the overlay's own stored `coordinates` (`move_pane_to_position`), so where the user
//!   put it is expressed in the same terms as where it was opened, and `reapply_coordinates`
//!   re-resolves a dragged overlay exactly as it re-resolves an untouched one. That is what keeps
//!   the sentence above - geometry is a function of coordinates and the surface - true of a
//!   draggable overlay.
//! - **No resize.** An overlay is moved by the user, never resized by one: `Tab::resize` no-ops
//!   for an overlay, the ctrl-drag branch of the mouse pipeline skips one, and there is no
//!   `ResizeStrategy` API here. An overlay's *size* stays a function of its coordinates, which is
//!   why a percentage-sized overlay still grows with the screen after being dragged.
//!
//! `pane_being_moved_with_mouse` used to be on this list, as "overlays are not user-draggable".
//! It is here now because the user asked to be able to move a popup out of the way of what it is
//! covering. Nothing else about the concept moved: the drag never changes focus (so a modal keeps
//! the keyboard throughout) and it edits the coordinates rather than introducing a position
//! remembered anywhere else.
//!
//! What the drag *does* add is a claim on half of those coordinates. A dragged overlay's position
//! is the user's (`position_is_user_stated`), so `change_pane_coordinates` takes only a plugin's
//! size from then on. Without that, an overlay whose plugin re-places itself on every `TabUpdate`
//! is undraggable and looks it: the drag reports session state, session state is a `TabUpdate`,
//! and the plugin answers each step of the drag by putting the overlay back.
//!
//! If one of the remaining absences turns out to be needed, that is a signal that the concept is
//! not as clean as assumed. Say so rather than adding it quietly.
//!
//! Not to be confused with the nested-session **guest modal**
//! (`crate::panes::nested_session_modal`), which shares the word and nothing else: it is a dialog
//! drawn onto an existing terminal pane's cells rather than a pane in its own right.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use zellij_utils::{
    data::{
        FloatingPaneCoordinates, ModeInfo, OverlayKind, OverlayPlacement, PaneInfo, Style, Styling,
    },
    errors::prelude::*,
    input::layout::{PercentOrFixed, RunPluginOrAlias},
    input::options::PaneFrameStyle,
    pane_size::{Offset, PaneGeom, Size, SizeInPixels, Viewport},
    position::Position,
};

use crate::panes::kitty_graphics::KittyHostSupport;
use crate::resize_pty;
use crate::tab::{pane_info_for_pane, Pane};
use crate::{
    global_panes::{session_surface, GlobalPaneStrip},
    os_input_output::ServerOsApi,
    output::Output,
    panes::floating_panes::floating_pane_grid::{geom_moved_within, half_size_middle_geom},
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
    /// The user dragged this overlay, so its `x`/`y` are theirs and a plugin re-asserting its
    /// placement no longer moves it (`change_pane_coordinates`).
    ///
    /// Deliberately not in `OverlayPlacement`, which is what travels when an overlay follows a
    /// client to another tab: that struct crosses the protobuf boundary, and the *coordinates* the
    /// drag wrote travel in it already. Only the "the user said so" bit is in-memory, and it is
    /// worth a proto change on the day a plugin re-places an overlay it has followed across.
    position_is_user_stated: bool,
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
    display_area: Rc<RefCell<Size>>,
    display_area_origin: Rc<RefCell<(usize, usize)>>,
    /// The tab's own viewport, needed only to answer whether an overlay owns cells outside it -
    /// which, drawn over the session, it almost always does.
    viewport: Rc<RefCell<Viewport>>,
    global_pane_strips: Rc<RefCell<Vec<GlobalPaneStrip>>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<HashMap<ClientId, bool>>>, // bool -> is_web_client
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
        display_area_origin: Rc<RefCell<(usize, usize)>>,
        viewport: Rc<RefCell<Viewport>>,
        global_pane_strips: Rc<RefCell<Vec<GlobalPaneStrip>>>,
        connected_clients: Rc<RefCell<HashSet<ClientId>>>,
        connected_clients_in_app: Rc<RefCell<HashMap<ClientId, bool>>>,
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
            display_area_origin,
            viewport,
            global_pane_strips,
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
        }
    }

    // ---- the coordinate space -------------------------------------------------------------

    /// The whole session surface, and the origin coordinates resolve from.
    ///
    /// Read fresh on every use rather than cached: `Tab::resize_whole_tab_locally` updates the
    /// shared `display_area` cell partway through a resize, and an overlay resolved against the
    /// value from before that would keep the old surface's dimensions.
    fn surface(&self) -> (Viewport, (usize, usize)) {
        let surface = session_surface(
            *self.display_area.borrow(),
            *self.display_area_origin.borrow(),
            &self.global_pane_strips.borrow(),
        );
        (surface, (surface.x, surface.y))
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
    ) {
        let (surface, origin) = self.surface();
        // `adjust_coordinates` resolves the geometry and nothing else, so a coordinate that is
        // not geometry has to be applied by hand - as `change_pane_coordinates` does.
        if let Some(borderless) = coordinates.borderless {
            pane.set_borderless(borderless);
        }
        let mut geom = Self::base_geom(&surface, pane.position_and_size());
        geom.adjust_coordinates(coordinates.clone(), surface, origin);
        pane.set_geom(geom);
        pane.set_should_render(true);
        self.panes.insert(
            pane_id,
            OverlayPane {
                pane,
                kind,
                coordinates,
                position_is_user_stated: false,
            },
        );
        self.raise_within_class(pane_id);
        self.set_pane_frames().non_fatal();
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
        with_pane.set_geom(removed.pane.current_geom());
        self.panes.insert(
            with_pane_id,
            OverlayPane {
                pane: with_pane,
                kind: removed.kind,
                coordinates: removed.coordinates,
                // the slot keeps where the user put it, exactly as it keeps its kind and its
                // coordinates - a pane swapped into a dragged overlay is the pane the user
                // dragged
                position_is_user_stated: removed.position_is_user_stated,
            },
        );
        if let Some(index) = self.z_indices.iter().position(|p_id| p_id == &pane_id) {
            self.z_indices[index] = with_pane_id;
        } else {
            self.z_indices.push(with_pane_id);
        }
        self.move_clients_between_panes(pane_id, with_pane_id);
        self.set_pane_frames().non_fatal();
        Ok(removed.pane)
    }

    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.keys()
    }
    pub fn panes_contain(&self, pane_id: &PaneId) -> bool {
        self.panes.contains_key(pane_id)
    }
    pub fn has_panes(&self) -> bool {
        !self.panes.is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
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
            .iter()
            .map(|(pane_id, overlay)| {
                let mut pane_info_for_pane =
                    pane_info_for_pane(pane_id, overlay.pane.as_ref(), current_pane_group);
                pane_info_for_pane.is_floating = true;
                pane_info_for_pane.is_suppressed = false;
                pane_info_for_pane.is_focused = self.active_panes.pane_id_is_focused(pane_id);
                // an overlay cannot go fullscreen - there is no `fullscreen_pane_id` here
                pane_info_for_pane.is_fullscreen = false;
                pane_info_for_pane
            })
            .collect()
    }

    // ---- z-order and occlusion ------------------------------------------------------------

    /// The panes making up the occlusion stack, bottom-most first.
    ///
    /// There is no visibility gate here and no filtering of any kind: an overlay is drawn for as
    /// long as it exists. `FloatingPanes` has to filter this list (fullscreen, pinned-only,
    /// layer-hidden) and warns that `stack_layer_ids` and `render` must stay derived from the same
    /// list or a pane is clipped against its own geometry and erases itself - it maintains that by
    /// hand-duplicating the filter chain in two places. Here `render` iterates this method
    /// directly, so the two cannot drift.
    pub fn stack_layer_ids(&self) -> Vec<PaneId> {
        self.z_indices
            .iter()
            .filter(|pane_id| self.panes.contains_key(pane_id))
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
    pub fn layers(&self) -> Vec<PaneGeom> {
        self.stack_layer_ids()
            .iter()
            .filter_map(|pane_id| self.get(pane_id).map(|pane| pane.position_and_size()))
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
    pub fn may_be_focused(&self, pane_id: &PaneId) -> bool {
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
    pub fn focus_pane_if_exists(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        if self.panes.contains_key(&pane_id) {
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
    /// A dismissible overlay (hocus-ask and friends) watches `PaneUpdate` and closes itself once
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
    /// itself on `TabUpdate` would otherwise undo every step of its own drag. `width`/`height`
    /// still apply either way - the user moved the overlay, they did not take over its size.
    pub fn change_pane_coordinates(
        &mut self,
        pane_id: PaneId,
        new_coordinates: FloatingPaneCoordinates,
    ) -> Result<()> {
        let err_context = || format!("failed to change coordinates of overlay {pane_id:?}");
        let (surface, origin) = self.surface();
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
        if overlay.position_is_user_stated {
            new_coordinates.x = overlay.coordinates.x;
            new_coordinates.y = overlay.coordinates.y;
        }
        let mut geom = Self::base_geom(&surface, overlay.pane.position_and_size());
        geom.adjust_coordinates(new_coordinates.clone(), surface, origin);
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates = new_coordinates;

        self.set_pane_frames().non_fatal();
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
        let (surface, origin) = self.surface();
        for overlay in self.panes.values_mut() {
            let mut geom = Self::base_geom(&surface, overlay.pane.position_and_size());
            geom.adjust_coordinates(overlay.coordinates.clone(), surface, origin);
            overlay.pane.set_geom(geom);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
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
    /// resize, a global pane appearing, following a client to another tab - without this collection
    /// needing a `desired_pane_positions` of its own: where the user put it is stated in the same
    /// terms as where it was opened. `PaneGeom::apply_floating_pane_position` reads `Fixed(n)` as
    /// `origin + n`, so the position round-trips exactly.
    ///
    /// `width`/`height` are deliberately left alone. Only the *position* becomes user-stated; a
    /// percentage-sized overlay still grows with the screen after being dragged.
    pub fn move_pane_to_position(&mut self, click_position: &Position) -> bool {
        // true => changed position
        let (surface, origin) = self.surface();
        let Some((pane_id, _initial_position, previous_position)) =
            self.pane_being_moved_with_mouse
        else {
            return false;
        };
        if click_position == &previous_position {
            return false;
        }
        let move_x_by = click_position.column() as isize - previous_position.column() as isize;
        let move_y_by = click_position.line() - previous_position.line();
        let Some(overlay) = self.panes.get_mut(&pane_id) else {
            return false;
        };
        let geom = geom_moved_within(
            overlay.pane.position_and_size(),
            surface,
            move_x_by,
            move_y_by,
        );
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x.saturating_sub(origin.0)));
        overlay.coordinates.y = Some(PercentOrFixed::Fixed(geom.y.saturating_sub(origin.1)));
        overlay.position_is_user_stated = true;
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

    /// The absolute geometries of the overlays owning cells outside the tab's own viewport - which,
    /// being drawn over the session, is most of them. Nothing but this tab paints those cells.
    pub fn geoms_outside_viewport(&self) -> HashMap<PaneId, PaneGeom> {
        let viewport = *self.viewport.borrow();
        self.panes
            .iter()
            .filter_map(|(pane_id, overlay)| {
                let geom = overlay.pane.position_and_size();
                geom_is_outside_viewport(&geom, &viewport).then_some((*pane_id, geom))
            })
            .collect()
    }
    pub fn has_panes_outside_viewport(&self) -> bool {
        let viewport = *self.viewport.borrow();
        self.panes
            .values()
            .any(|overlay| geom_is_outside_viewport(&overlay.pane.position_and_size(), &viewport))
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
    /// An overlay always has a frame unless it explicitly asked not to. There is no fullscreen
    /// branch here because an overlay cannot go fullscreen.
    pub fn set_pane_frames(&mut self) -> Result<()> {
        let err_context =
            |pane_id: &PaneId| format!("failed to activate frame on overlay {pane_id:?}");
        for overlay in self.panes.values_mut() {
            let pane = &mut overlay.pane;
            if !pane.borderless() {
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
        inactive_clients: &HashSet<ClientId>,
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

        // no visibility filter: an overlay keeps its focus for as long as it exists, which is the
        // whole reason it is not a floating pane
        let mut active_panes = self.active_panes.clone_active_panes();
        active_panes.retain(|client_id, _| !inactive_clients.contains(client_id));

        for (pane_id, overlay) in &self.panes {
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
            let show_help_text = active_panes.iter().any(|(client_id, active_id)| {
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

#[cfg(test)]
#[path = "./unit/overlay_panes_tests.rs"]
mod overlay_panes_tests;
