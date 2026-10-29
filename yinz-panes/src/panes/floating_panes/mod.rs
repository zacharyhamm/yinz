pub mod floating_pane_grid;
use yinz_types::data::PaneInfo;
use yinz_types::geometry::{Direction, FloatingPaneCoordinates, ResizeStrategy};
use yinz_types::multiplayer::AttachedClients;
use yinz_types::position::Position;

use crate::pane::{pane_info_for_pane, Pane};
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::resize_pty;
use crate::ui::damage::CellRect;
use floating_pane_grid::{geom_moved_within, FloatingPaneGrid};

use crate::{
    os_input_output::ServerOsApi,
    output::{FloatingPanesStack, Output},
    panes::{ActivePanes, PaneId},
    plugins::PluginInstruction,
    thread_bus::ThreadSenders,
    ui::pane_contents_and_ui::PaneContentsAndUi,
    ClientId,
};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;
use yinz_runtime::errors::prelude::*;
use yinz_types::pane_size::{Dimension, Offset, PaneGeom, Size, SizeInPixels, Viewport};
use yinz_types::style::{Style, Styling};
use {
    yinz_types::data::ModeInfo, yinz_types::input::command::RunCommand,
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::options::PaneFrameStyle,
};

const RESIZE_INCREMENT_WIDTH: usize = 5;
const RESIZE_INCREMENT_HEIGHT: usize = 2;

/// Whether any part of `geom` lies outside `viewport` - for a floating pane, whether it hangs off
/// its tab's viewport and onto the chrome above or below it.
pub fn geom_is_outside_viewport(geom: &PaneGeom, viewport: &Viewport) -> bool {
    geom.x < viewport.x
        || geom.y < viewport.y
        || geom.x + geom.cols.as_usize() > viewport.x + viewport.cols
        || geom.y + geom.rows.as_usize() > viewport.y + viewport.rows
}

pub struct FloatingPanes {
    panes: BTreeMap<PaneId, Box<dyn Pane>>,
    display_area: Rc<RefCell<Size>>,
    viewport: Rc<RefCell<Viewport>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<AttachedClients>>,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    default_mode_info: ModeInfo,
    style: Style,
    session_is_mirrored: bool,
    desired_pane_positions: HashMap<PaneId, PaneGeom>, // this represents the positions of panes the user moved with intention, rather than by resizing the terminal window
    width_anchored_panes: HashSet<PaneId>,
    z_indices: Vec<PaneId>,
    active_panes: ActivePanes,
    show_panes: bool,
    pane_being_moved_with_mouse: Option<(PaneId, Position, Position)>, // (pane-id,
    // initial_position,
    // last_position)
    senders: ThreadSenders,
    window_title: Option<String>,
    dimmed_clients: HashSet<ClientId>,
    fullscreen_covers_ui: Rc<RefCell<bool>>,
    fullscreen_pane_id: Option<PaneId>,
    floating_fullscreen_covers_ui: bool,
    pane_frame_style: PaneFrameStyle,
}

#[allow(clippy::borrowed_box)]
#[allow(clippy::too_many_arguments)]
impl FloatingPanes {
    pub fn new(
        display_area: Rc<RefCell<Size>>,
        viewport: Rc<RefCell<Viewport>>,
        connected_clients: Rc<RefCell<HashSet<ClientId>>>,
        connected_clients_in_app: Rc<RefCell<AttachedClients>>,
        mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        fullscreen_covers_ui: Rc<RefCell<bool>>,
        pane_frame_style: PaneFrameStyle,
        session_is_mirrored: bool,
        default_mode_info: ModeInfo,
        style: Style,
        os_input: Box<dyn ServerOsApi>,
        senders: ThreadSenders,
    ) -> Self {
        FloatingPanes {
            panes: BTreeMap::new(),
            display_area,
            viewport,
            connected_clients,
            connected_clients_in_app,
            mode_info,
            character_cell_size,
            session_is_mirrored,
            default_mode_info,
            style,
            desired_pane_positions: HashMap::new(),
            width_anchored_panes: HashSet::new(),
            z_indices: vec![],
            show_panes: false,
            active_panes: ActivePanes::new(&os_input),
            pane_being_moved_with_mouse: None,
            senders,
            window_title: None,
            dimmed_clients: HashSet::new(),
            fullscreen_covers_ui,
            fullscreen_pane_id: None,
            floating_fullscreen_covers_ui: false,
            pane_frame_style,
        }
    }
    pub fn agent_widths(&self) -> HashMap<PaneId, (usize, usize)> {
        self.width_anchored_panes
            .iter()
            .filter_map(|id| {
                let pane = self.panes.get(id)?;
                if pane.geom_override().is_some() {
                    return None;
                }
                let requested = pane.current_geom();
                let mut displayed = pane.constrain_content_width(requested);
                // Window resizing may shift the requested rectangle to fit its uncapped width.
                displayed.x = self.desired_pane_positions.get(id).unwrap_or(&requested).x;
                let displayed = geom_moved_within(displayed, self.move_surface(), 0, 0);
                Some((*id, (displayed.x, displayed.cols.as_usize())))
            })
            .collect()
    }
    pub fn set_pane_frame_style(&mut self, pane_frame_style: PaneFrameStyle) {
        self.pane_frame_style = pane_frame_style;
        if (self.fullscreen_pane_id.is_some() && !self.floating_fullscreen_covers_ui)
            || self.panes.values().any(|pane| pane.subtitle().is_some())
        {
            let _ = self.set_pane_frames();
        }
    }
    pub fn set_client_dimmed(&mut self, client_id: ClientId, dimmed: bool) {
        if dimmed {
            self.dimmed_clients.insert(client_id);
        } else {
            self.dimmed_clients.remove(&client_id);
        }
        self.set_force_render();
    }
    /// The panes making up the occlusion stack, bottom-most first.
    ///
    /// `stack()` turns these into the geometries `Output` clips against, and `render` sends
    /// each pane's position in this list as its z-index - `visible_character_chunks` then
    /// skips that many layers. The two must be derived from the same list: a pane given an
    /// index that is not its own is clipped against its own geometry and erases itself.
    fn stack_layer_ids(&self) -> Vec<PaneId> {
        if *self.fullscreen_covers_ui.borrow() {
            return vec![];
        }
        if let Some(fullscreen_pane_id) = self.fullscreen_pane_id {
            return self
                .panes
                .contains_key(&fullscreen_pane_id)
                .then_some(vec![fullscreen_pane_id])
                .unwrap_or_default();
        }
        if self.all_panes_are_visible() {
            self.z_indices
                .iter()
                .filter(|pane_id| self.panes.contains_key(pane_id))
                .copied()
                .collect()
        } else if self.has_pinned_panes() {
            self.z_indices
                .iter()
                .filter(|pane_id| {
                    self.panes
                        .get(pane_id)
                        .is_some_and(|p| p.position_and_size().is_pinned)
                })
                .copied()
                .collect()
        } else {
            vec![]
        }
    }
    pub fn stack(&self) -> Option<FloatingPanesStack> {
        // a fullscreen floating pane covers the surface with its own geometry rather than the
        // one it would return to
        let fullscreen = self.fullscreen_pane_id.is_some();
        let layers: Vec<PaneGeom> = self
            .stack_layer_ids()
            .iter()
            .filter_map(|pane_id| {
                self.panes.get(pane_id).map(|p| {
                    if fullscreen {
                        p.current_geom()
                    } else {
                        p.render_geom()
                    }
                })
            })
            .collect();
        if layers.is_empty() {
            None
        } else {
            Some(FloatingPanesStack { layers })
        }
    }
    /// The absolute geometries of the panes that sit outside the tab's viewport - dragged onto the
    /// tab bar or the status bar, or opened over one. Nothing about such a pane depends on the
    /// tab's layout, so it is taken out of a resize and put back afterwards - see
    /// `restore_geoms_outside_viewport`.
    /// Each entry keeps the requested geometry and the displayed geometry, respectively.
    pub fn geoms_outside_viewport(&self) -> HashMap<PaneId, (PaneGeom, PaneGeom)> {
        let viewport = *self.viewport.borrow();
        self.panes
            .iter()
            .filter_map(|(pane_id, pane)| {
                let geom = pane.position_and_size();
                let displayed = pane.render_geom();
                geom_is_outside_viewport(&displayed, &viewport)
                    .then_some((*pane_id, (geom, displayed)))
            })
            .collect()
    }

    /// Put back what `geoms_outside_viewport` took out of a resize, keeping each pane on the
    /// surface as it is now - a surface that shrank out from under a pane still moves it.
    pub fn restore_geoms_outside_viewport(&mut self, geoms: HashMap<PaneId, (PaneGeom, PaneGeom)>) {
        if geoms.is_empty() {
            return;
        }
        let surface = self.move_surface();
        for (pane_id, (mut geom, displayed)) in geoms {
            let Some(pane) = self.panes.get_mut(&pane_id) else {
                continue;
            };
            geom.x = geom.x.min(
                (surface.x + surface.cols)
                    .saturating_sub(geom.cols.as_usize())
                    .max(surface.x),
            );
            geom.y = geom.y.min(
                (surface.y + surface.rows)
                    .saturating_sub(geom.rows.as_usize())
                    .max(surface.y),
            );
            pane.set_geom(geom);
            if self.width_anchored_panes.contains(&pane_id) && pane.geom_override().is_none() {
                // The displayed anchor fits by its capped width, not its requested width.
                let displayed = geom_moved_within(displayed, surface, 0, 0);
                geom.x = displayed.x;
                pane.set_render_width(Some((displayed.x, displayed.cols.as_usize())));
            }
            self.desired_pane_positions.insert(pane_id, geom);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
    }

    /// Whether any pane currently being painted owns cells outside the tab's own viewport - on the
    /// chrome above or below it. Nothing but this tab paints those cells, so a client that stops
    /// looking at this tab has to be handed a repaint of what is underneath.
    pub fn has_panes_outside_viewport(&self) -> bool {
        let viewport = *self.viewport.borrow();
        self.stack_layer_ids().iter().any(|pane_id| {
            self.panes
                .get(pane_id)
                .is_some_and(|pane| geom_is_outside_viewport(&pane.render_geom(), &viewport))
        })
    }
    /// The topmost pane actually being painted at `position`, if any. Derived from
    /// `stack_layer_ids` rather than from `panes` so it agrees with what is on the screen -
    /// `get_pane_id_at` answers for panes that are not currently drawn at all.
    pub fn visible_pane_id_at(&self, position: &Position) -> Option<PaneId> {
        self.stack_layer_ids()
            .iter()
            .rev()
            .find(|pane_id| {
                self.panes
                    .get(pane_id)
                    .is_some_and(|pane| pane.contains(position))
            })
            .copied()
    }
    pub fn has_pinned_panes(&self) -> bool {
        self.panes
            .iter()
            .any(|(_, p)| p.position_and_size().is_pinned)
    }
    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.keys()
    }
    pub fn rendered_pane_ids(&self) -> Vec<PaneId> {
        self.stack_layer_ids()
    }
    pub fn add_pane(&mut self, pane_id: PaneId, pane: Box<dyn Pane>) {
        self.desired_pane_positions
            .insert(pane_id, pane.position_and_size());
        self.panes.insert(pane_id, pane);
        self.z_indices.retain(|id| *id != pane_id);
        self.z_indices.push(pane_id);
        self.make_sure_pinned_panes_are_on_top();
    }
    pub fn replace_active_pane(
        &mut self,
        pane: Box<dyn Pane>,
        client_id: ClientId,
    ) -> Result<Box<dyn Pane>> {
        self.active_panes
            .get(&client_id)
            .with_context(|| format!("failed to determine active pane for client {client_id}"))
            .copied()
            .and_then(|active_pane_id| self.replace_pane(active_pane_id, pane))
            .with_context(|| format!("failed to replace active pane for client {client_id}"))
    }
    pub fn replace_pane(
        &mut self,
        pane_id: PaneId,
        mut with_pane: Box<dyn Pane>,
    ) -> Result<Box<dyn Pane>> {
        let with_pane_id = with_pane.pid();
        let removed_pane = self
            .panes
            .remove(&pane_id)
            .with_context(|| format!("failed to replace unknown floating pane {pane_id:?}"))?;
        with_pane.set_content_offset(Offset::frame(1));
        // Keep the base geometry separate so leaving fullscreen restores the original size.
        with_pane.set_geom(removed_pane.position_and_size());
        match removed_pane.geom_override() {
            Some(geom) => with_pane.set_geom_override(geom),
            None => with_pane.reset_size_and_position_override(),
        }
        self.panes.insert(with_pane_id, with_pane);
        if self.fullscreen_pane_id == Some(pane_id) {
            self.fullscreen_pane_id = Some(with_pane_id);
        }
        match self.z_indices.iter().position(|id| *id == pane_id) {
            Some(index) => self.z_indices[index] = with_pane_id,
            None => self.z_indices.push(with_pane_id),
        }
        self.make_sure_pinned_panes_are_on_top();
        if let Some(position) = self.desired_pane_positions.remove(&pane_id) {
            self.desired_pane_positions.insert(with_pane_id, position);
        }
        if self.width_anchored_panes.remove(&pane_id) {
            self.width_anchored_panes.insert(with_pane_id);
        }
        self.move_clients_between_panes(pane_id, with_pane_id);
        self.active_panes.retarget_last_pane(pane_id, with_pane_id);
        if let Some((id, _, _)) = self.pane_being_moved_with_mouse.as_mut()
            && *id == pane_id
        {
            *id = with_pane_id;
        }
        self.set_pane_frames().non_fatal();
        Ok(removed_pane)
    }
    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        if self.fullscreen_pane_id == Some(pane_id) {
            self.unset_fullscreen();
        }
        if self.pane_being_moved_with_mouse.map(|(id, _, _)| id) == Some(pane_id) {
            self.pane_being_moved_with_mouse = None;
        }
        self.z_indices.retain(|p_id| *p_id != pane_id);
        self.desired_pane_positions.remove(&pane_id);
        self.width_anchored_panes.remove(&pane_id);
        let removed_pane = self.panes.remove(&pane_id);
        self.move_clients_out_of_pane(pane_id);
        self.active_panes.remove_last_pane(pane_id);
        removed_pane
    }
    pub fn hold_pane(
        &mut self,
        pane_id: PaneId,
        exit_status: Option<i32>,
        is_first_run: bool,
        run_command: RunCommand,
    ) {
        if let Some(p) = self.panes.get_mut(&pane_id) {
            p.hold(exit_status, is_first_run, run_command);
        }
    }
    pub fn get(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.panes.get(pane_id)
    }
    pub fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut Box<dyn Pane>> {
        self.panes.get_mut(pane_id)
    }
    pub fn get_active_pane(&self, client_id: ClientId) -> Option<&Box<dyn Pane>> {
        self.active_panes
            .get(&client_id)
            .and_then(|active_pane_id| self.panes.get(active_pane_id))
    }
    pub fn get_active_pane_mut(&mut self, client_id: ClientId) -> Option<&mut Box<dyn Pane>> {
        self.active_panes
            .get(&client_id)
            .and_then(|active_pane_id| self.panes.get_mut(active_pane_id))
    }
    pub fn panes_are_visible(&self) -> bool {
        self.all_panes_are_visible()
    }
    pub fn all_panes_are_visible(&self) -> bool {
        self.show_panes && !*self.fullscreen_covers_ui.borrow()
    }
    /// Whether the floating surface is up, regardless of a fullscreen pane covering it.
    pub fn panes_are_shown(&self) -> bool {
        self.show_panes
    }
    pub fn has_active_panes(&self) -> bool {
        !self.active_panes.is_empty()
    }
    pub fn has_panes(&self) -> bool {
        !self.panes.is_empty()
    }
    pub fn active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get(&client_id).copied()
    }
    pub fn get_pane_z_index(&self, pane_id: PaneId) -> Option<usize> {
        self.stack_layer_ids().iter().position(|id| id == &pane_id)
    }
    pub fn active_pane_id_or_focused_pane_id(&self, client_id: Option<ClientId>) -> Option<PaneId> {
        // returns the focused pane of any client_id - should be safe because the way things are
        // set up at the time of writing, all clients are focused on the same floating pane due to
        // z_index issues
        client_id
            .and_then(|client_id| self.active_panes.get(&client_id).copied())
            .or_else(|| self.panes.keys().next().copied())
    }
    pub fn toggle_show_panes(&mut self, should_show_floating_panes: bool) {
        self.window_title = None; // clear so that it will be re-rendered once we toggle back
        self.show_panes = should_show_floating_panes;
        if should_show_floating_panes {
            self.active_panes
                .focus_all_panes(&self.panes, &self.connected_clients.borrow());
        } else {
            self.active_panes
                .unfocus_all_panes(&self.panes, &self.connected_clients.borrow());
        }
    }
    /// Which clients have this pane focused. Needed when a pane leaves this collection for
    /// another one while keeping its focus.
    pub fn clients_focused_on(&self, pane_id: &PaneId) -> Vec<ClientId> {
        self.active_panes
            .iter()
            .filter(|(_, p_id)| *p_id == pane_id)
            .map(|(c_id, _)| *c_id)
            .collect()
    }
    pub fn pane_is_focused(&self, pane_id: &PaneId) -> bool {
        self.active_panes.pane_id_is_focused(pane_id)
    }
    pub fn active_panes_contain(&self, client_id: &ClientId) -> bool {
        self.active_panes.contains_key(client_id)
    }
    pub fn forget_client(&mut self, client_id: ClientId) {
        self.active_panes.forget_client(&client_id);
    }
    pub fn panes_contain(&self, pane_id: &PaneId) -> bool {
        self.panes.contains_key(pane_id)
    }
    pub fn find_room_for_new_pane(&mut self) -> Option<PaneGeom> {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.find_room_for_new_pane()
    }
    pub fn move_client_focus_to_existing_panes(&mut self) {
        let existing_pane_ids: Vec<PaneId> = self.panes.keys().copied().collect();
        let nonexisting_panes_that_are_focused = self
            .active_panes
            .values()
            .filter(|pane_id| !existing_pane_ids.contains(pane_id))
            .copied()
            .collect::<Vec<_>>();
        for pane_id in nonexisting_panes_that_are_focused {
            self.move_clients_out_of_pane(pane_id);
        }
    }
    pub fn position_floating_pane_layout(
        &mut self,
        floating_pane_layout: &FloatingPaneLayout,
    ) -> Result<PaneGeom> {
        let pane_geoms: Vec<_> = self
            .panes
            .values()
            .map(|pane| pane.position_and_size())
            .collect();
        Self::position_layout_in_viewport(
            *self.viewport.borrow(),
            floating_pane_layout,
            &pane_geoms,
        )
    }

    pub(crate) fn find_room_in_viewport(
        viewport: Viewport,
        pane_geoms: &[PaneGeom],
    ) -> Option<PaneGeom> {
        FloatingPaneGrid::find_room_in_viewport(viewport, pane_geoms)
    }

    pub(crate) fn position_layout_in_viewport(
        viewport: Viewport,
        floating_pane_layout: &FloatingPaneLayout,
        pane_geoms: &[PaneGeom],
    ) -> Result<PaneGeom> {
        let mut position = FloatingPaneGrid::find_room_in_viewport(viewport, pane_geoms)
            .context("failed to find position for floating pane")?;
        position.apply_floating_pane_position(
            floating_pane_layout.x,
            floating_pane_layout.y,
            floating_pane_layout.width,
            floating_pane_layout.height,
            viewport.cols,
            viewport.rows,
        );
        if let Some(is_pinned) = &floating_pane_layout.pinned {
            position.is_pinned = *is_pinned;
        }
        if let Some(logical_position) = &floating_pane_layout.logical_position {
            position.logical_position = Some(*logical_position);
        }
        if position.cols.as_usize() > viewport.cols {
            position.cols = Dimension::fixed(viewport.cols);
        }
        if position.rows.as_usize() > viewport.rows {
            position.rows = Dimension::fixed(viewport.rows);
        }
        let right_edge = viewport.x + viewport.cols;
        let bottom_edge = viewport.y + viewport.rows;
        if position.x + position.cols.as_usize() > right_edge {
            position.x = position
                .x
                .saturating_sub((position.x + position.cols.as_usize()) - right_edge);
        }
        if position.y + position.rows.as_usize() > bottom_edge {
            position.y = position
                .y
                .saturating_sub((position.y + position.rows.as_usize()) - bottom_edge);
        }
        Ok(position)
    }
    pub fn first_floating_pane_id(&self) -> Option<PaneId> {
        self.panes.keys().next().copied()
    }
    pub fn last_floating_pane_id(&self) -> Option<PaneId> {
        self.panes.keys().last().copied()
    }
    pub fn last_selectable_floating_pane_id(&self) -> Option<PaneId> {
        self.panes
            .iter()
            .rfind(|(_p_id, p)| p.selectable())
            .map(|(p_id, _p)| *p_id)
    }
    pub fn has_selectable_panes(&self) -> bool {
        self.panes
            .iter()
            .rfind(|(_p_id, p)| p.selectable())
            .is_some()
    }
    pub fn first_active_floating_pane_id(&self) -> Option<PaneId> {
        self.active_panes.values().next().copied()
    }
    pub fn set_force_render(&mut self) {
        for pane in self.panes.values_mut() {
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
    }
    /// Repaint only the panes overlapping `damage` - the cells an overlay stopped covering.
    ///
    /// The narrowed twin of `set_force_render` above. A toast sits in a corner and overlaps a pane
    /// or two, so repainting the whole container every frame of a slide-out - which is what the
    /// unnarrowed version did - re-serializes grids that never had a toast anywhere near them.
    pub fn force_render_panes_intersecting(&mut self, damage: &[CellRect]) {
        for pane in self.panes.values_mut() {
            let geom = pane.render_geom();
            if !damage.iter().any(|rect| rect.intersects_geom(&geom)) {
                continue;
            }
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
    }
    pub fn set_pane_frames(&mut self) -> Result<()> {
        let err_context =
            |pane_id: &PaneId| format!("failed to activate frame on pane {pane_id:?}");

        let no_ui_fullscreen_pane_id = if self.floating_fullscreen_covers_ui {
            self.fullscreen_pane_id
        } else {
            None
        };
        let regular_fullscreen_pane_id = if self.floating_fullscreen_covers_ui {
            None
        } else {
            self.fullscreen_pane_id
        };
        let pane_frame_style = self.pane_frame_style;
        for pane in self.panes.values_mut() {
            pane.set_subtitle_enabled(
                pane_frame_style.draws_full_frames() || pane_frame_style.draws_titles(),
            );
            if no_ui_fullscreen_pane_id == Some(pane.pid()) {
                pane.set_frame(false);
                pane.set_content_offset(Offset::default());
                resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                    .with_context(|| err_context(&pane.pid()))?;
                continue;
            }
            if regular_fullscreen_pane_id == Some(pane.pid()) {
                if pane.borderless() {
                    pane.set_frame(false);
                    pane.set_content_offset(Offset::default());
                } else if pane_frame_style.draws_full_frames() {
                    pane.set_frame(true);
                    pane.set_content_offset(Offset::frame(1));
                } else if pane_frame_style.draws_titles() {
                    pane.set_frame(false);
                    pane.set_content_offset(Offset::shift_right_top_and_bottom(0, 1, 0));
                } else {
                    pane.set_frame(false);
                    pane.set_content_offset(Offset::default());
                }
                resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                    .with_context(|| err_context(&pane.pid()))?;
                continue;
            }
            // floating panes should always have a frame unless explicitly set otherwise
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
    pub fn render(
        &mut self,
        output: &mut Output,
        mouse_hover_pane_id: &HashMap<ClientId, PaneId>,
        current_pane_group: HashMap<ClientId, Vec<PaneId>>,
        client_id_override: Option<ClientId>,
        help_text_visible: &HashMap<ClientId, bool>,
        mouse_scroll_resize: bool,
        same_worktree_panes: &HashMap<ClientId, HashSet<PaneId>>,
        stack_list_header_rows: &HashMap<PaneId, usize>,
    ) -> Result<()> {
        let err_context = || "failed to render output";
        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };

        // If we have a client_id_override (for watcher rendering), add it temporarily
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }

        let connected_clients: Vec<ClientId> = connected_clients.into_iter().collect();
        let active_panes = if self.all_panes_are_visible() {
            self.active_panes.clone_active_panes()
        } else {
            Default::default()
        };

        for (kind, pane) in &self.panes {
            if let Some(fullscreen_pane_id) = self.fullscreen_pane_id
                && kind != &fullscreen_pane_id
            {
                continue;
            }
            match kind {
                PaneId::Terminal(_) => {
                    output.add_pane_contents(
                        &connected_clients,
                        pane.pid(),
                        pane.pane_contents(None, false, None),
                    );
                },
                PaneId::Plugin(_) => {
                    for client_id in &connected_clients {
                        output.add_pane_contents(
                            &[*client_id],
                            pane.pid(),
                            pane.pane_contents(Some(*client_id), false, None),
                        );
                    }
                },
            }
        }

        let fullscreen_pane_id = self.fullscreen_pane_id;
        let no_ui_fullscreen = self.floating_fullscreen_covers_ui && fullscreen_pane_id.is_some();
        let regular_fullscreen =
            fullscreen_pane_id.is_some() && !self.floating_fullscreen_covers_ui;
        let pane_frame_style = self.pane_frame_style;
        // a pane's place among the panes drawn here is not necessarily its place in the occlusion
        // stack (a fullscreen pane draws alone, pinned panes draw without the rest), so take the
        // index from the stack itself, or a pane ends up clipped against its own geometry
        let layer_ids = self.stack_layer_ids();
        let mut floating_panes: Vec<_> = if let Some(fullscreen_pane_id) = fullscreen_pane_id {
            self.panes
                .iter_mut()
                .filter(|(id, _)| **id == fullscreen_pane_id)
                .collect()
        } else if self.all_panes_are_visible() {
            self.panes.iter_mut().collect()
        } else if self.has_pinned_panes() {
            self.panes
                .iter_mut()
                .filter(|(_, pane)| pane.position_and_size().is_pinned)
                .collect()
        } else {
            vec![]
        };
        floating_panes.sort_by(|(a_id, _a_pane), (b_id, _b_pane)| {
            let a_pos = self.z_indices.iter().position(|id| id == *a_id);
            let b_pos = self.z_indices.iter().position(|id| id == *b_id);
            a_pos.cmp(&b_pos)
        });
        for (kind, pane) in floating_panes.iter_mut() {
            // +1 because 0 is reserved for non-floating panes
            let z_index = layer_ids
                .iter()
                .position(|id| id == *kind)
                .map(|index| index + 1);
            let mut active_panes = active_panes.clone();
            let multiplayer = self.connected_clients_in_app.borrow().snapshot();
            active_panes.retain(|c_id, _| self.connected_clients.borrow().contains(c_id));
            let pane_is_selectable = pane.selectable();
            let show_help_text = active_panes.iter().any(|(client_id, pane_id)| {
                pane_id == &pane.pid() && help_text_visible.get(client_id).copied().unwrap_or(false)
            });
            let pane_has_guest_modal = pane.has_guest_modal_for_any_client();
            let pane_is_regular_fullscreen =
                regular_fullscreen && Some(**kind) == fullscreen_pane_id;
            let should_draw_pane_frames = if pane_is_regular_fullscreen {
                pane_frame_style.draws_full_frames()
            } else {
                true
            };
            let reserved_rows = stack_list_header_rows.get(kind).copied().unwrap_or(0);
            let frame_geom_override = (reserved_rows > 0).then(|| {
                let mut geom = pane.render_geom();
                geom.y += reserved_rows;
                geom.rows.decrease_inner(reserved_rows);
                geom
            });
            let mut pane_contents_and_ui = PaneContentsAndUi::new(
                pane,
                output,
                self.style,
                &active_panes,
                multiplayer,
                z_index,
                false,
                false,
                should_draw_pane_frames,
                mouse_hover_pane_id,
                current_pane_group.clone(),
                show_help_text,
                false,
                mouse_scroll_resize,
                self.dimmed_clients.clone(),
            );
            // Keep the normal frame below stack entries even when their cached rows do not draw.
            pane_contents_and_ui.set_frame_geom_override(frame_geom_override);
            pane_contents_and_ui.set_blank_title(reserved_rows > 0);
            for client_id in &connected_clients {
                pane_contents_and_ui.set_same_worktree(
                    same_worktree_panes
                        .get(client_id)
                        .is_some_and(|panes| panes.contains(kind)),
                );
                let client_mode = self
                    .mode_info
                    .borrow()
                    .get(client_id)
                    .unwrap_or(&self.default_mode_info)
                    .mode;
                let is_floating = true;
                let skip_frame = no_ui_fullscreen
                    || (pane_is_regular_fullscreen
                        && !pane_frame_style.draws_full_frames()
                        && !pane_frame_style.draws_titles());
                if !skip_frame {
                    pane_contents_and_ui
                        .render_pane_frame(
                            *client_id,
                            client_mode,
                            self.session_is_mirrored,
                            is_floating,
                            pane_is_selectable,
                            // a floating pane always offers the pin toggle and the scroll
                            // indication; the pane that wanted neither was the overlay, and an
                            // overlay is no longer one of these
                            true,
                            true,
                        )
                        .with_context(err_context)?;
                }
                if (no_ui_fullscreen || pane_is_regular_fullscreen)
                    && !pane_frame_style.draws_full_frames()
                {
                    pane_contents_and_ui
                        .render_floating_gutter_edges(
                            *client_id,
                            client_mode,
                            if no_ui_fullscreen {
                                (*self.display_area.borrow()).into()
                            } else {
                                *self.viewport.borrow()
                            },
                            self.session_is_mirrored,
                        )
                        .with_context(err_context)?;
                }
                if let PaneId::Plugin(..) = kind {
                    pane_contents_and_ui
                        .render_pane_contents_for_client(*client_id)
                        .with_context(err_context)?;
                }
                pane_contents_and_ui.render_terminal_title_if_needed(
                    *client_id,
                    client_mode,
                    &mut self.window_title,
                );
                // this is done for panes that don't have their own cursor (eg. panes of
                // another user)
                pane_contents_and_ui
                    .render_fake_cursor_if_needed(*client_id)
                    .with_context(err_context)?;
            }
            if let PaneId::Terminal(..) = kind {
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

    pub fn resize(&mut self, new_screen_size: Size) {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.resize(new_screen_size).non_fatal();
        self.set_force_render();
    }

    // desired positions live in the same coordinate space as the panes themselves, so any caller
    // that translates the panes must translate these too - otherwise the next resize restores them
    // into the wrong space
    pub fn translate_desired_positions(&mut self, x_offset: isize, y_offset: isize) {
        fn translate_coordinate(coordinate: usize, offset: isize) -> usize {
            if offset.is_negative() {
                coordinate.saturating_sub(offset.unsigned_abs())
            } else {
                coordinate.saturating_add(offset as usize)
            }
        }
        for desired_pane_position in self.desired_pane_positions.values_mut() {
            desired_pane_position.x = translate_coordinate(desired_pane_position.x, x_offset);
            desired_pane_position.y = translate_coordinate(desired_pane_position.y, y_offset);
        }
    }

    pub fn resize_pty_all_panes(&mut self, _os_api: &mut Box<dyn ServerOsApi>) -> Result<()> {
        for pane in self.panes.values_mut() {
            pane.character_cell_size_possibly_changed();
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(|| format!("failed to resize PTY in pane {:?}", pane.pid()))?;
        }
        Ok(())
    }

    pub fn resize_active_pane(
        &mut self,
        client_id: ClientId,
        _os_api: &mut Box<dyn ServerOsApi>,
        strategy: &ResizeStrategy,
    ) -> Result<bool> {
        // true => successfully resized
        match self.active_panes.get(&client_id).copied() {
            Some(pane_id) if self.panes.contains_key(&pane_id) => {
                self.resize_pane_with_id(*strategy, pane_id)
            },
            _ => Ok(false),
        }
    }
    pub fn resize_pane_with_id(
        &mut self,
        strategy: ResizeStrategy,
        pane_id: PaneId,
    ) -> Result<bool> {
        // true => successfully resized
        let err_context = || format!("Failed to resize pane with id: {:?}", pane_id);
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid
            .change_pane_size(
                &pane_id,
                &strategy,
                (RESIZE_INCREMENT_WIDTH, RESIZE_INCREMENT_HEIGHT),
            )
            .with_context(err_context)?;

        for pane in self.panes.values_mut() {
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(err_context)?;
        }
        self.set_force_render();
        Ok(true)
    }

    pub fn resize_pane_with_strategies(
        &mut self,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> Result<()> {
        let err_context = || format!("Failed to resize pane {:?} with strategies", pane_id);
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );

        // Apply each strategy
        for strategy in strategies {
            floating_pane_grid
                .change_pane_size(&pane_id, strategy, change_by)
                .with_context(err_context)?;
        }

        for pane in self.panes.values_mut() {
            resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                .with_context(err_context)?;
        }
        self.set_force_render();
        Ok(())
    }

    fn set_pane_active_at(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.set_active_at(Instant::now());
        }
    }

    fn reenter_fullscreen_on_active_pane(
        &mut self,
        client_id: ClientId,
        previous_pane: Option<PaneId>,
        covers_ui: bool,
    ) {
        let pane_id = self
            .get_active_pane_id(client_id)
            .filter(|id| self.panes.contains_key(id))
            .or_else(|| previous_pane.filter(|id| self.panes.contains_key(id)));
        if let Some(pane_id) = pane_id {
            self.set_fullscreen(pane_id, covers_ui);
        }
    }
    pub fn move_focus(
        &mut self,
        client_id: ClientId,
        connected_clients: &HashSet<ClientId>,
        direction: &Direction,
    ) -> Result<bool> {
        // true => successfully moved
        let _err_context = || {
            format!("failed to move focus of floating pane {direction:?} for client {client_id}")
        };
        // An unfocused client has no starting pane; leave other clients and the stack alone.
        let Some(active_pane_id) = self
            .active_panes
            .get(&client_id)
            .copied()
            .filter(|id| self.panes.contains_key(id))
        else {
            return Ok(false);
        };
        if self.fullscreen_pane_id.is_some() {
            let covers_ui = self.floating_fullscreen_covers_ui;
            let previous_pane = self.fullscreen_pane_id;
            self.unset_fullscreen();
            let ret = self.move_focus(client_id, connected_clients, direction);
            self.reenter_fullscreen_on_active_pane(client_id, previous_pane, covers_ui);
            return ret;
        }

        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let updated_active_pane = {
            let rendered_pane_ids = self.rendered_pane_ids();
            let floating_pane_grid = FloatingPaneGrid::new(
                self.panes
                    .iter_mut()
                    .filter(|(pane_id, _)| rendered_pane_ids.contains(pane_id)),
                &mut self.desired_pane_positions,
                display_area,
                viewport,
            );
            let next_index = match direction {
                Direction::Left => {
                    floating_pane_grid.next_selectable_pane_id_to_the_left(&active_pane_id)
                },
                Direction::Down => {
                    floating_pane_grid.next_selectable_pane_id_below(&active_pane_id)
                },
                Direction::Up => floating_pane_grid.next_selectable_pane_id_above(&active_pane_id),
                Direction::Right => {
                    floating_pane_grid.next_selectable_pane_id_to_the_right(&active_pane_id)
                },
            };
            match next_index {
                Some(p) => {
                    // render previously active pane so that its frame does not remain actively
                    // colored
                    let Some(previously_active_pane) = self.get_active_pane_mut(client_id) else {
                        log::error!("Failed to get active pane");
                        return Ok(false);
                    };

                    previously_active_pane.set_should_render(true);
                    // we render the full viewport to remove any ui elements that might have been
                    // there before (eg. another user's cursor)
                    previously_active_pane.render_full_viewport();

                    let Some(next_active_pane) = self.get_pane_mut(p) else {
                        log::error!("Failed to get next active pane");
                        return Ok(false);
                    };
                    next_active_pane.set_should_render(true);
                    // we render the full viewport to remove any ui elements that might have been
                    // there before (eg. another user's cursor)
                    next_active_pane.render_full_viewport();

                    // move all clients
                    let connected_clients: Vec<ClientId> =
                        connected_clients.iter().copied().collect();
                    for client_id in connected_clients {
                        self.focus_pane(p, client_id);
                    }
                    self.set_pane_active_at(p);

                    self.set_force_render();
                    return Ok(true);
                },
                None => active_pane_id,
            }
        };
        let connected_clients: Vec<ClientId> = connected_clients.iter().copied().collect();
        for client_id in connected_clients {
            self.focus_pane(updated_active_pane, client_id);
        }
        self.set_pane_active_at(updated_active_pane);
        self.set_force_render();
        Ok(false)
    }
    pub fn focus_pane_on_edge(&mut self, direction: Direction, client_id: ClientId) {
        if self.fullscreen_pane_id.is_some() {
            let covers_ui = self.floating_fullscreen_covers_ui;
            let previous_pane = self.fullscreen_pane_id;
            self.unset_fullscreen();
            self.focus_pane_on_edge(direction, client_id);
            self.reenter_fullscreen_on_active_pane(client_id, previous_pane, covers_ui);
            return;
        }
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let rendered_pane_ids = self.rendered_pane_ids();
        let floating_pane_grid = FloatingPaneGrid::new(
            self.panes
                .iter_mut()
                .filter(|(pane_id, _)| rendered_pane_ids.contains(pane_id)),
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        if let Some(pane_id) = floating_pane_grid.pane_id_on_edge(direction) {
            self.focus_pane(pane_id, client_id);
            self.set_force_render();
        }
    }
    pub fn focus_pane_adjacent_to(
        &mut self,
        pane_id: PaneId,
        direction: Direction,
        client_id: ClientId,
    ) -> Option<PaneId> {
        if !self.panes.contains_key(&pane_id) {
            return None;
        }
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let rendered_pane_ids = self.rendered_pane_ids();
        let floating_pane_grid = FloatingPaneGrid::new(
            self.panes
                .iter_mut()
                .filter(|(pane_id, _)| rendered_pane_ids.contains(pane_id)),
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        let next_index = match direction {
            Direction::Left => floating_pane_grid.next_selectable_pane_id_to_the_left(&pane_id),
            Direction::Down => floating_pane_grid.next_selectable_pane_id_below(&pane_id),
            Direction::Up => floating_pane_grid.next_selectable_pane_id_above(&pane_id),
            Direction::Right => floating_pane_grid.next_selectable_pane_id_to_the_right(&pane_id),
        }?;
        if let Some(previously_active_pane) = self.get_active_pane_mut(client_id) {
            previously_active_pane.set_should_render(true);
            previously_active_pane.render_full_viewport();
        }
        if let Some(next_active_pane) = self.get_pane_mut(next_index) {
            next_active_pane.set_should_render(true);
            next_active_pane.render_full_viewport();
        }
        self.focus_pane(next_index, client_id);
        self.set_pane_active_at(next_index);
        self.set_force_render();
        Some(next_index)
    }

    pub fn focus_last_pane(&mut self, client_id: ClientId) {
        if self.fullscreen_pane_id.is_some() {
            let covers_ui = self.floating_fullscreen_covers_ui;
            let previous_pane = self.fullscreen_pane_id;
            self.unset_fullscreen();
            self.focus_last_pane(client_id);
            self.reenter_fullscreen_on_active_pane(client_id, previous_pane, covers_ui);
            return;
        }
        let rendered_pane_ids = self.rendered_pane_ids();
        let currently_active_pane_id = self.active_panes.get(&client_id).copied();
        let last_pane_id = self
            .active_panes
            .get_last(&client_id)
            .copied()
            .filter(|last_pane_id| {
                Some(*last_pane_id) != currently_active_pane_id
                    && rendered_pane_ids.contains(last_pane_id)
                    && self
                        .panes
                        .get(last_pane_id)
                        .map(|p| p.selectable())
                        .unwrap_or(false)
            });
        let target_pane_id = last_pane_id.or_else(|| {
            self.most_recently_active_selectable_pane(currently_active_pane_id, &rendered_pane_ids)
        });
        if let Some(target_pane_id) = target_pane_id {
            self.focus_pane(target_pane_id, client_id);
            self.set_force_render();
        }
    }
    fn most_recently_active_selectable_pane(
        &self,
        exclude: Option<PaneId>,
        rendered_pane_ids: &[PaneId],
    ) -> Option<PaneId> {
        self.panes
            .iter()
            .filter(|(pane_id, pane)| {
                Some(**pane_id) != exclude
                    && rendered_pane_ids.contains(pane_id)
                    && pane.selectable()
            })
            .max_by_key(|(_pane_id, pane)| pane.active_at())
            .map(|(pane_id, _pane)| *pane_id)
    }

    pub fn move_pane_down(&mut self, pane_id: PaneId) {
        if self.fullscreen_pane_id.is_some() {
            return;
        }
        let display_area = *self.display_area.borrow();
        let viewport = self.move_surface();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.move_pane_down(&pane_id).non_fatal();
        self.width_anchored_panes.insert(pane_id);
        self.set_force_render();
    }
    pub fn move_pane_up(&mut self, pane_id: PaneId) {
        if self.fullscreen_pane_id.is_some() {
            return;
        }
        let display_area = *self.display_area.borrow();
        let viewport = self.move_surface();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.move_pane_up(&pane_id).non_fatal();
        self.width_anchored_panes.insert(pane_id);
        self.set_force_render();
    }
    pub fn move_pane_left(&mut self, pane_id: PaneId) {
        if self.fullscreen_pane_id.is_some() {
            return;
        }
        let display_area = *self.display_area.borrow();
        let viewport = self.move_surface();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.move_pane_left(&pane_id).non_fatal();
        self.width_anchored_panes.insert(pane_id);
        self.set_force_render();
    }
    pub fn move_pane_right(&mut self, pane_id: PaneId) {
        if self.fullscreen_pane_id.is_some() {
            return;
        }
        let display_area = *self.display_area.borrow();
        let viewport = self.move_surface();
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.move_pane_right(&pane_id).non_fatal();
        self.width_anchored_panes.insert(pane_id);
        self.set_force_render();
    }
    pub fn move_pane(&mut self, search_backwards: bool, pane_id: PaneId) {
        if self.fullscreen_pane_id.is_some() {
            return;
        }
        let new_position_id = {
            let pane_grid = FloatingPaneGrid::new(
                &mut self.panes,
                &mut self.desired_pane_positions,
                *self.display_area.borrow(),
                *self.viewport.borrow(),
            );
            if search_backwards {
                pane_grid.previous_selectable_pane_id(&pane_id)
            } else {
                pane_grid.next_selectable_pane_id(&pane_id)
            }
        };
        if let Some(new_position_id) = new_position_id {
            let Some(current_position) = self.panes.get(&pane_id) else {
                log::error!("Failed to find current position");
                return;
            };
            let prev_geom = current_position.position_and_size();
            let prev_geom_override = current_position.geom_override();

            let Some(new_position) = self.panes.get_mut(&new_position_id) else {
                log::error!("Failed to find new position");
                return;
            };
            let next_geom = new_position.position_and_size();
            let next_geom_override = new_position.geom_override();
            new_position.set_geom(prev_geom);
            if let Some(geom) = prev_geom_override {
                new_position.set_geom_override(geom);
            }
            new_position.set_should_render(true);

            let Some(current_position) = self.panes.get_mut(&pane_id) else {
                log::error!("Failed to find current position");
                return;
            };
            current_position.set_geom(next_geom);
            if let Some(geom) = next_geom_override {
                current_position.set_geom_override(geom);
            }
            current_position.set_should_render(true);
            self.swap_position_metadata(pane_id, new_position_id);
            let _ = self.set_pane_frames();
        }
    }
    fn swap_position_metadata(&mut self, first: PaneId, second: PaneId) {
        let first_position = self.desired_pane_positions.remove(&first);
        let second_position = self.desired_pane_positions.remove(&second);
        if let Some(position) = first_position {
            self.desired_pane_positions.insert(second, position);
        }
        if let Some(position) = second_position {
            self.desired_pane_positions.insert(first, position);
        }
        let first_anchored = self.width_anchored_panes.remove(&first);
        let second_anchored = self.width_anchored_panes.remove(&second);
        if first_anchored {
            self.width_anchored_panes.insert(second);
        }
        if second_anchored {
            self.width_anchored_panes.insert(first);
        }
    }
    pub fn change_pane_coordinates(
        &mut self,
        pane_id: PaneId,
        new_coordinates: FloatingPaneCoordinates,
    ) -> Result<()> {
        let err_context = || "Failed to change_pane_coordinates".to_string();

        {
            let viewport = self.placement_space();
            let pane = self.get_pane_mut(pane_id).with_context(err_context)?;
            let mut pane_geom = pane.position_and_size();
            if let Some(pinned) = new_coordinates.pinned.as_ref() {
                pane.set_pinned(*pinned);
            }
            if let Some(should_be_borderless) = new_coordinates.borderless {
                if should_be_borderless {
                    pane.set_borderless(true);
                } else {
                    pane.set_borderless(false);
                };
            }
            pane_geom.adjust_coordinates(new_coordinates.clone(), viewport);
            pane.set_geom(pane_geom);
            pane.set_should_render(true);

            // we do this in case this moves the pane under another pane so that the pane user's
            // are focused on will always be on top
            let is_focused = self.active_panes.pane_id_is_focused(&pane_id);
            if is_focused {
                self.z_indices.retain(|p_id| *p_id != pane_id);
                self.z_indices.push(pane_id);
                self.make_sure_pinned_panes_are_on_top();
            }

            self.desired_pane_positions.insert(pane_id, pane_geom);
        }
        let _ = self.set_pane_frames();
        Ok(())
    }

    pub fn adjust_coordinates(
        &self,
        pane_geom: &mut PaneGeom,
        coordinates: FloatingPaneCoordinates,
    ) {
        pane_geom.adjust_coordinates(coordinates, self.placement_space());
    }

    /// The area a floating pane may be *moved* around in: the whole session surface, chrome
    /// included. Placement still resolves against the tab's own viewport - a default floating pane
    /// should open between the tab bar and the status bar - but once it exists, dragging it is
    /// direct manipulation and the viewport's edge is not a wall.
    fn move_surface(&self) -> Viewport {
        let display_area = *self.display_area.borrow();
        Viewport {
            x: 0,
            y: 0,
            cols: display_area.cols,
            rows: display_area.rows,
        }
    }

    /// Where a floating pane's coordinates are resolved: the tab's own viewport.
    fn placement_space(&self) -> Viewport {
        *self.viewport.borrow()
    }
    pub fn move_clients_out_of_pane(&mut self, pane_id: PaneId) {
        let active_panes: Vec<(ClientId, PaneId)> = self
            .active_panes
            .iter()
            .map(|(cid, pid)| (*cid, *pid))
            .collect();

        // find the most recently active pane
        let mut next_active_pane_candidates: Vec<(&PaneId, &Box<dyn Pane>)> = self
            .panes
            .iter()
            .filter(|(_p_id, p)| p.selectable())
            .collect();
        next_active_pane_candidates.sort_by(|(_pane_id_a, pane_a), (_pane_id_b, pane_b)| {
            pane_a.active_at().cmp(&pane_b.active_at())
        });
        let next_active_pane_id = next_active_pane_candidates
            .last()
            .map(|(pane_id, _pane)| **pane_id);
        let last_active_pane_id = next_active_pane_candidates
            .iter()
            .rev()
            .nth(1)
            .map(|(pane_id, _pane)| **pane_id);

        for (client_id, active_pane_id) in active_panes {
            if active_pane_id == pane_id {
                match next_active_pane_id {
                    Some(next_active_pane_id) => {
                        self.active_panes
                            .insert(client_id, next_active_pane_id, &self.panes);
                        self.focus_pane(next_active_pane_id, client_id);
                        if let Some(last_active_pane_id) = last_active_pane_id {
                            self.active_panes
                                .set_last_pane(client_id, last_active_pane_id);
                        }
                    },
                    None => {
                        self.defocus_pane(client_id);
                    },
                }
            }
        }
    }
    pub fn focus_pane_for_all_clients(&mut self, pane_id: PaneId) {
        if !self.pane_is_selectable(&pane_id) {
            return;
        }
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            self.active_panes.insert(client_id, pane_id, &self.panes);
        }
        self.raise_pane(pane_id);
        self.set_force_render();
    }
    pub fn fullscreen_is_active(&self) -> bool {
        self.fullscreen_pane_id.is_some()
    }
    pub fn fullscreen_pane_id(&self) -> Option<PaneId> {
        self.fullscreen_pane_id
    }
    pub fn fullscreen_covers_ui(&self) -> bool {
        self.fullscreen_pane_id.is_some() && self.floating_fullscreen_covers_ui
    }
    fn expand_pane_over_area(
        &mut self,
        pane_id: PaneId,
        x: usize,
        y: usize,
        rows: usize,
        cols: usize,
    ) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            let mut expanded_geom = PaneGeom {
                x,
                y,
                ..Default::default()
            };
            expanded_geom.rows.set_inner(rows);
            expanded_geom.cols.set_inner(cols);
            pane.set_geom_override(expanded_geom);
        }
    }
    fn set_fullscreen(&mut self, pane_id: PaneId, covers_ui: bool) {
        if !self.panes.contains_key(&pane_id) {
            log::error!("No floating pane with id: {:?} found", pane_id);
            return;
        }
        if covers_ui {
            let display_area = { *self.display_area.borrow() };
            self.expand_pane_over_area(pane_id, 0, 0, display_area.rows, display_area.cols);
        } else {
            let viewport = { *self.viewport.borrow() };
            self.expand_pane_over_area(
                pane_id,
                viewport.x,
                viewport.y,
                viewport.rows,
                viewport.cols,
            );
        }
        let pane_frame_style = self.pane_frame_style;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            // a pane covering the UI, or a borderless pane: no frame either way
            if covers_ui || pane.borderless() {
                pane.set_frame(false);
                pane.set_content_offset(Offset::default());
            } else if pane_frame_style.draws_full_frames() {
                pane.set_frame(true);
                pane.set_content_offset(Offset::frame(1));
            } else if pane_frame_style.draws_titles() {
                pane.set_frame(false);
                pane.set_content_offset(Offset::shift_right_top_and_bottom(0, 1, 0));
            } else {
                pane.set_frame(false);
                pane.set_content_offset(Offset::default());
            }
        }
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            resize_pty!(pane, os_api, self.senders, self.character_cell_size).non_fatal();
        }
        self.fullscreen_pane_id = Some(pane_id);
        self.floating_fullscreen_covers_ui = covers_ui;
        self.focus_pane_for_all_clients(pane_id);
        self.set_force_render();
    }
    pub fn unset_fullscreen(&mut self) {
        if let Some(fullscreen_pane_id) = self.fullscreen_pane_id.take() {
            if let Some(pane) = self.panes.get_mut(&fullscreen_pane_id) {
                pane.reset_size_and_position_override();
                if !pane.borderless() {
                    pane.set_frame(true);
                    pane.set_content_offset(Offset::frame(1));
                } else {
                    pane.set_content_offset(Offset::default());
                }
                resize_pty!(pane, os_api, self.senders, self.character_cell_size).non_fatal();
            }
            self.floating_fullscreen_covers_ui = false;
            self.set_force_render();
        }
    }
    pub fn toggle_pane_fullscreen(&mut self, pane_id: PaneId) {
        if !self.panes.contains_key(&pane_id) {
            return;
        }
        if self.fullscreen_pane_id.is_some() {
            let was_no_ui = self.floating_fullscreen_covers_ui;
            self.unset_fullscreen();
            if was_no_ui {
                self.set_fullscreen(pane_id, false);
            }
        } else {
            self.set_fullscreen(pane_id, false);
        }
    }
    pub fn toggle_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        if !self.panes.contains_key(&pane_id) {
            return;
        }
        if self.fullscreen_pane_id.is_some() {
            let was_no_ui = self.floating_fullscreen_covers_ui;
            self.unset_fullscreen();
            if !was_no_ui {
                self.set_fullscreen(pane_id, true);
            }
        } else {
            self.set_fullscreen(pane_id, true);
        }
    }
    pub fn toggle_active_pane_fullscreen(&mut self, client_id: ClientId) {
        if let Some(active_pane_id) = self
            .fullscreen_pane_id
            .or_else(|| self.get_active_pane_id(client_id))
        {
            self.toggle_pane_fullscreen(active_pane_id);
        }
    }
    pub fn toggle_active_pane_no_ui_fullscreen(&mut self, client_id: ClientId) {
        if let Some(active_pane_id) = self
            .fullscreen_pane_id
            .or_else(|| self.get_active_pane_id(client_id))
        {
            self.toggle_pane_no_ui_fullscreen(active_pane_id);
        }
    }
    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        if !self.pane_is_selectable(&pane_id) {
            log::error!("Cannot focus pane {:?} as it is not selectable!", pane_id);
            return;
        }
        if let Some(focused_pane) = self.active_panes.get(&client_id)
            && pane_id != *focused_pane
        {
            self.active_panes.set_last_pane(client_id, *focused_pane);
        }
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.focus_pane_for_all_clients(pane_id);
    }
    fn pane_is_selectable(&self, pane_id: &PaneId) -> bool {
        self.panes
            .get(pane_id)
            .is_some_and(|pane| pane.selectable())
    }
    fn raise_pane(&mut self, pane_id: PaneId) {
        self.z_indices.retain(|id| *id != pane_id);
        self.z_indices.push(pane_id);
        self.make_sure_pinned_panes_are_on_top();
        self.set_pane_active_at(pane_id);
    }
    pub fn focus_pane_for_joining_client(&mut self, client_id: ClientId) {
        let focused_by_others = {
            let connected_clients = self.connected_clients.borrow();
            self.active_panes
                .iter()
                .filter(|(id, _)| **id != client_id && connected_clients.contains(id))
                .map(|(_, pane_id)| *pane_id)
                .find(|pane_id| self.pane_is_selectable(pane_id))
        };
        let remembered = self
            .active_panes
            .get(&client_id)
            .copied()
            .filter(|pane_id| self.pane_is_selectable(pane_id));
        let pane_to_focus = focused_by_others
            .or(remembered)
            .or_else(|| self.most_recently_active_selectable_pane(None, &self.z_indices));
        if let Some(pane_id) = pane_to_focus {
            if let Some(previous) = remembered.filter(|previous| *previous != pane_id) {
                self.active_panes.set_last_pane(client_id, previous);
            }
            self.active_panes.insert(client_id, pane_id, &self.panes);
            self.raise_pane(pane_id);
        } else {
            self.active_panes.remove(&client_id, &self.panes);
        }
    }
    pub fn defocus_pane(&mut self, client_id: ClientId) {
        self.active_panes.remove(&client_id, &self.panes);
        self.set_force_render();
    }
    pub fn get_pane(&self, pane_id: PaneId) -> Option<&Box<dyn Pane>> {
        self.panes.get(&pane_id)
    }
    pub fn get_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.panes.get_mut(&pane_id)
    }
    pub fn get_pane_id_at(
        &self,
        point: &Position,
        search_selectable: bool,
    ) -> Result<Option<PaneId>> {
        Ok(self
            .z_indices
            .iter()
            .rev()
            .find(|pane_id| {
                self.panes.get(pane_id).is_some_and(|pane| {
                    (!search_selectable || pane.selectable()) && pane.contains(point)
                })
            })
            .copied())
    }
    pub fn get_pinned_pane_id_at(
        &self,
        point: &Position,
        search_selectable: bool,
    ) -> Result<Option<PaneId>> {
        Ok(self
            .z_indices
            .iter()
            .rev()
            .find(|pane_id| {
                self.panes.get(pane_id).is_some_and(|pane| {
                    (!search_selectable || pane.selectable())
                        && pane.current_geom().is_pinned
                        && pane.contains(point)
                })
            })
            .copied())
    }
    pub fn has_pinned_pane_at(&self, point: &Position) -> bool {
        self.panes
            .values()
            .any(|pane| pane.current_geom().is_pinned && pane.contains(point))
    }
    pub fn get_pane_at_mut(
        &mut self,
        position: &Position,
        search_selectable: bool,
    ) -> Option<&mut Box<dyn Pane>> {
        self.get_pane_id_at(position, search_selectable)
            .ok()?
            .and_then(|pane_id| self.panes.get_mut(&pane_id))
    }
    pub fn set_pane_being_moved_with_mouse(&mut self, pane_id: PaneId, last_position: Position) {
        if let Some((last_pane_id, initial_position)) = self
            .pane_being_moved_with_mouse
            .map(|(pane_id, initial_position, _)| (pane_id, initial_position))
            && last_pane_id == pane_id
        {
            // preserve initial_position
            self.pane_being_moved_with_mouse = Some((pane_id, initial_position, last_position));
            return;
        }
        self.pane_being_moved_with_mouse = Some((pane_id, last_position, last_position));
    }
    pub fn pane_is_being_moved_with_mouse(&self) -> bool {
        self.pane_being_moved_with_mouse.is_some()
    }
    pub fn move_pane_to_position(&mut self, click_position: &Position) -> bool {
        // true => changed position
        let display_area = *self.display_area.borrow();
        let viewport = self.move_surface();
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
        let mut floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid
            .move_pane_by(pane_id, move_x_by, move_y_by)
            .non_fatal();
        self.width_anchored_panes.insert(pane_id);
        self.set_pane_being_moved_with_mouse(pane_id, *click_position);
        self.set_force_render();
        true
    }
    pub fn move_pane_with_mouse(&mut self, position: Position, search_selectable: bool) -> bool {
        // true => handled, false => not handled (eg. no pane at this position)
        if *self.fullscreen_covers_ui.borrow() || self.fullscreen_pane_id.is_some() {
            return false;
        }
        let show_panes = self.show_panes;
        if self.pane_being_moved_with_mouse.is_some() {
            if self.move_pane_to_position(&position) {
                // pane was moved to a new position
                self.set_force_render();
                return true;
            }
        } else if let Some(pane) = self.get_pane_at_mut(&position, search_selectable) {
            let clicked_on_frame = pane.position_is_on_frame(&position);
            if (show_panes || pane.position_and_size().is_pinned) && clicked_on_frame {
                let pid = pane.pid();
                if self.pane_being_moved_with_mouse.is_none() {
                    self.set_pane_being_moved_with_mouse(pid, position);
                }
                self.move_pane_to_position(&position);
                self.set_force_render();
                return true;
            }
        };
        false
    }
    pub fn stop_moving_pane_with_mouse(&mut self, position: Position) -> bool {
        // bool -> this
        // pane was never
        // moved (initial
        // position ==
        // last_position)
        let mut never_moved = false;
        if let Some((_pane_id, initial_position, _last_position)) =
            self.pane_being_moved_with_mouse.take()
        {
            if initial_position == position {
                never_moved = true;
            } else {
                self.move_pane_to_position(&position);
            }
            self.set_force_render();
        };
        self.pane_being_moved_with_mouse = None;
        never_moved
    }
    pub fn get_active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get(&client_id).copied()
    }
    pub fn get_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.panes.iter()
    }
    pub fn visible_panes_count(&self) -> usize {
        self.panes.len()
    }
    pub fn drain(&mut self) -> BTreeMap<PaneId, Box<dyn Pane>> {
        self.unset_fullscreen();
        self.pane_being_moved_with_mouse = None;
        self.z_indices.clear();
        self.desired_pane_positions.clear();
        match self.panes.iter().next().map(|(pid, _p)| *pid) {
            Some(first_pid) => self.panes.split_off(&first_pid),
            None => BTreeMap::new(),
        }
    }
    fn move_clients_between_panes(&mut self, from_pane_id: PaneId, to_pane_id: PaneId) {
        let clients_in_pane: Vec<ClientId> = self
            .active_panes
            .iter()
            .filter(|(_cid, pid)| **pid == from_pane_id)
            .map(|(cid, _pid)| *cid)
            .collect();
        for client_id in clients_in_pane {
            self.active_panes.remove(&client_id, &self.panes);
            self.active_panes.insert(client_id, to_pane_id, &self.panes);
        }
    }
    pub fn reapply_pane_focus(&mut self) {
        let focused_by_connected_client = {
            let connected_clients = self.connected_clients.borrow();
            self.active_panes
                .iter()
                .filter(|(id, _)| connected_clients.contains(id))
                .map(|(_, pane_id)| *pane_id)
                .find(|pane_id| self.pane_is_selectable(pane_id))
        };
        if let Some(pane_id) = focused_by_connected_client {
            self.focus_pane_for_all_clients(pane_id);
        } else {
            let remembered = self
                .active_panes
                .values()
                .copied()
                .find(|pane_id| self.pane_is_selectable(pane_id))
                .or_else(|| self.most_recently_active_selectable_pane(None, &self.z_indices));
            if let Some(pane_id) = remembered {
                self.raise_pane(pane_id);
            }
        }
    }
    pub fn switch_active_pane_with(&mut self, _os_api: &mut Box<dyn ServerOsApi>, pane_id: PaneId) {
        if let Some(active_pane_id) = self.first_active_floating_pane_id() {
            let Some(current_position) = self.panes.get(&active_pane_id) else {
                log::error!("Can't find current position");
                return;
            };
            let prev_geom = current_position.position_and_size();
            let prev_geom_override = current_position.geom_override();

            let Some(new_position) = self.panes.get_mut(&pane_id) else {
                log::error!("Can't find position");
                return;
            };
            let next_geom = new_position.position_and_size();
            let next_geom_override = new_position.geom_override();
            new_position.set_geom(prev_geom);
            if let Some(geom) = prev_geom_override {
                new_position.set_geom_override(geom);
            }
            resize_pty!(new_position, os_api, self.senders, self.character_cell_size).non_fatal();
            new_position.set_should_render(true);

            let Some(current_position) = self.panes.get_mut(&active_pane_id) else {
                log::error!("Can't find current position");
                return;
            };
            current_position.set_geom(next_geom);
            if let Some(geom) = next_geom_override {
                current_position.set_geom_override(geom);
            }
            resize_pty!(
                current_position,
                os_api,
                self.senders,
                self.character_cell_size
            )
            .non_fatal();
            current_position.set_should_render(true);
            self.swap_position_metadata(active_pane_id, pane_id);
            self.focus_pane_for_all_clients(active_pane_id);
        }
    }
    pub fn get_plugin_pane_id(&self, run_plugin_or_alias: &RunPluginOrAlias) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_id, pane)| run_plugin_or_alias.is_equivalent_to_run(pane.invoked_with()))
            .map(|(id, _)| *id)
    }
    pub fn focus_pane_if_exists(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        if self.panes.contains_key(&pane_id) {
            self.focus_pane(pane_id, client_id);
            Ok(())
        } else {
            Err(anyhow!("Pane not found"))
        }
    }
    pub fn pane_info(&self, current_pane_group: &HashMap<ClientId, Vec<PaneId>>) -> Vec<PaneInfo> {
        let mut pane_infos = vec![];
        for (pane_id, pane) in self.panes.iter() {
            let mut pane_info_for_pane =
                pane_info_for_pane(pane_id, pane.as_ref(), current_pane_group);
            let is_focused = self.active_panes.pane_id_is_focused(pane_id);
            pane_info_for_pane.is_floating = true;
            pane_info_for_pane.is_suppressed = false;
            pane_info_for_pane.is_focused = is_focused;
            pane_info_for_pane.is_fullscreen = self.fullscreen_pane_id == Some(*pane_id);
            pane_infos.push(pane_info_for_pane);
        }
        pane_infos
    }
    pub fn set_geom_for_pane_with_run(&mut self, run: Option<Run>, geom: PaneGeom) {
        match self
            .panes
            .iter_mut()
            .find(|(_, p)| p.invoked_with() == &run)
        {
            Some((_, pane)) => {
                pane.set_geom(geom);
            },
            None => {
                log::error!("Failed to find pane with run: {:?}", run);
            },
        }
    }
    pub fn update_agent_theme_mapping(
        &mut self,
        mapping: yinz_types::agent::theme::AgentThemeMapping,
    ) {
        self.style.agent_theme_mapping = mapping;
        for pane in self.panes.values_mut() {
            pane.update_agent_theme_mapping(mapping);
        }
    }
    pub fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>) {
        self.style.terminal = theme;
        for pane in self.panes.values_mut() {
            pane.update_terminal_theme(theme);
        }
    }
    pub fn update_pane_themes(&mut self, theme: Styling) {
        self.style.colors = theme;
        for pane in self.panes.values_mut() {
            pane.update_theme(theme);
        }
    }
    pub fn update_pane_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        for pane in self.panes.values_mut() {
            pane.update_arrow_fonts(should_support_arrow_fonts);
        }
    }
    pub fn update_pane_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool) {
        for pane in self.panes.values_mut() {
            pane.update_render_options(styled_underlines, osc8_hyperlinks);
        }
    }
    pub fn update_pane_kitty_keyboard_protocol_disabled(&mut self, disabled: bool) {
        for pane in self.panes.values_mut() {
            pane.update_kitty_keyboard_protocol_disabled(disabled);
        }
    }
    pub fn set_session_is_mirrored(&mut self, session_is_mirrored: bool) {
        self.session_is_mirrored = session_is_mirrored;
    }
    pub fn update_pane_kitty_host_support(&mut self, supported: KittyHostSupport) {
        for pane in self.panes.values_mut() {
            pane.update_kitty_host_support(supported);
        }
    }
    pub fn update_pane_sixel_host_support(&mut self, supported: bool) {
        for pane in self.panes.values_mut() {
            pane.update_sixel_host_support(supported);
        }
    }
    pub fn update_pane_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        for pane in self.panes.values_mut() {
            pane.update_rounded_corners(rounded_corners);
        }
    }
    pub fn next_selectable_pane_id_above(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.next_selectable_pane_id_above(pane_id)
    }
    pub fn next_selectable_pane_id_below(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.next_selectable_pane_id_below(pane_id)
    }
    pub fn next_selectable_pane_id_to_the_left(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.next_selectable_pane_id_to_the_left(pane_id)
    }
    pub fn next_selectable_pane_id_to_the_right(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let display_area = *self.display_area.borrow();
        let viewport = *self.viewport.borrow();
        let floating_pane_grid = FloatingPaneGrid::new(
            &mut self.panes,
            &mut self.desired_pane_positions,
            display_area,
            viewport,
        );
        floating_pane_grid.next_selectable_pane_id_to_the_right(pane_id)
    }
    fn make_sure_pinned_panes_are_on_top(&mut self) {
        let pinned_status: std::collections::HashMap<_, _> = self
            .z_indices
            .iter()
            .map(|id| (*id, self.pane_id_is_pinned(id)))
            .collect();

        self.z_indices.sort_by(|a_id, b_id| {
            let a_is_pinned = pinned_status.get(a_id).copied().unwrap_or(false);
            let b_is_pinned = pinned_status.get(b_id).copied().unwrap_or(false);
            match (a_is_pinned, b_is_pinned) {
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => Ordering::Equal,
            }
        });
    }
    fn pane_id_is_pinned(&self, pane_id: &PaneId) -> bool {
        self.panes
            .get(pane_id)
            .map(|p| p.position_and_size().is_pinned)
            .unwrap_or(false)
    }
}
