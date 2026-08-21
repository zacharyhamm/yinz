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
//! Deliberately absent, and each absence is load-bearing rather than an oversight:
//!
//! - **No `show_panes`.** There is no layer to raise or lower: an overlay is drawn for as long as
//!   it exists. The entire bug class this collection was made to remove came from code that read
//!   "floating layer down" as "nothing on it is focused".
//! - **No `fullscreen_pane_id`.** What a fullscreen overlay would mean is undefined - an overlay
//!   is drawn over the whole session surface already. It is not reachable from here, and since
//!   Phase 4 `Tab`'s fullscreen paths refuse an overlay outright rather than falling through to
//!   the tiled collection, which used to fullscreen the pane *behind* the overlay.
//! - **No `desired_pane_positions`.** A floating pane remembers where the user dragged it. An
//!   overlay has no such history - it is re-resolved against the surface whenever the surface
//!   changes, which is what `reapply_coordinates` is for.
//! - **No `pane_being_moved_with_mouse`.** Overlays are not user-draggable.
//!
//! If one of these turns out to be needed, that is a signal that the concept is not as clean as
//! assumed. Say so rather than adding it quietly.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use zellij_utils::{
    data::{
        FloatingPaneCoordinates, ModeInfo, OverlayKind, OverlayPlacement, PaneInfo, Style, Styling,
    },
    errors::prelude::*,
    input::layout::RunPluginOrAlias,
    input::options::PaneFrameStyle,
    pane_size::{Offset, PaneGeom, Size, SizeInPixels, Viewport},
    position::Position,
};

use crate::resize_pty;
use crate::tab::{pane_info_for_pane, Pane};
use crate::{
    global_panes::{session_surface, GlobalPaneStrip},
    os_input_output::ServerOsApi,
    output::Output,
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

    // ---- membership -----------------------------------------------------------------------

    /// Put `pane` on the surface as an overlay of `kind`, resolving `coordinates` against the
    /// surface as it is now.
    ///
    /// Unlike `FloatingPanes::add_pane` this does the geometry itself rather than leaving it to
    /// the caller: an overlay's geometry is entirely a function of its coordinates and the
    /// surface, and letting a caller supply it is exactly how overlay-ness used to get lost.
    pub fn add_pane(
        &mut self,
        pane_id: PaneId,
        mut pane: Box<dyn Pane>,
        kind: OverlayKind,
        coordinates: FloatingPaneCoordinates,
    ) {
        let (surface, origin) = self.surface();
        let mut geom = pane.position_and_size();
        geom.adjust_coordinates(coordinates.clone(), surface, origin);
        pane.set_geom(geom);
        pane.set_should_render(true);
        self.panes.insert(
            pane_id,
            OverlayPane {
                pane,
                kind,
                coordinates,
            },
        );
        self.z_indices.retain(|p_id| *p_id != pane_id);
        self.z_indices.push(pane_id);
        self.set_pane_frames().non_fatal();
    }

    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        self.z_indices.retain(|p_id| *p_id != pane_id);
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

        with_pane.set_content_offset(Offset::frame(1));
        with_pane.set_geom(removed.pane.current_geom());
        self.panes.insert(
            with_pane_id,
            OverlayPane {
                pane: with_pane,
                kind: removed.kind,
                coordinates: removed.coordinates,
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
    /// Give every connected client this overlay.
    ///
    /// Overlays are focused for all clients at once, as floating panes are: the occlusion stack is
    /// shared, so a per-client focus would draw one client's overlay over another's anyway.
    pub fn focus_pane_for_all_clients(&mut self, pane_id: PaneId) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            self.active_panes.insert(client_id, pane_id, &self.panes);
            self.set_pane_active_at(pane_id);
        }
        self.z_indices.retain(|p_id| *p_id != pane_id);
        self.z_indices.push(pane_id);
        self.set_force_render();
    }
    /// Focus this overlay for one client, leaving every other client where it was.
    ///
    /// An overlay is drawn for everybody, but it is not focused for everybody: a second client
    /// looking at a tiled pane keeps looking at it. Raising the pane in the occlusion stack *is*
    /// shared, because the stack is.
    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        if let Some(previously_active) = self.active_pane_id(client_id) {
            self.active_panes
                .set_last_pane(client_id, previously_active);
        }
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.set_pane_active_at(pane_id);
        self.z_indices.retain(|p_id| *p_id != pane_id);
        self.z_indices.push(pane_id);
        self.set_force_render();
    }
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

    /// Move or resize an overlay, keeping it an overlay.
    ///
    /// `hocus-new-agent` re-asserts its placement on every resize, so overlay-ness has to survive a
    /// coordinate change. In `FloatingPanes` that was an explicit re-insert into a side map that
    /// callers could and did skip; here the stored `kind` is simply left alone.
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
        let mut geom = overlay.pane.position_and_size();
        geom.adjust_coordinates(new_coordinates.clone(), surface, origin);
        overlay.pane.set_geom(geom);
        overlay.pane.set_should_render(true);
        overlay.coordinates = new_coordinates;

        self.set_pane_frames().non_fatal();
        Ok(())
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
            let mut geom = overlay.pane.position_and_size();
            geom.adjust_coordinates(overlay.coordinates.clone(), surface, origin);
            overlay.pane.set_geom(geom);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
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
