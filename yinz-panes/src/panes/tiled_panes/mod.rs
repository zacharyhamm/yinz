mod pane_resizer;
mod stacked_panes;
mod tiled_pane_grid;

use crate::panes::kitty_graphics::KittyHostSupport;
use crate::ui::damage::CellRect;
use yinz_types::multiplayer::AttachedClients;

#[cfg(test)]
#[path = "./unit/mock_pane.rs"]
mod pane_resizer_test_mock;

#[cfg(test)]
#[path = "./unit/pane_resizer_tests.rs"]
mod pane_resizer_tests;

use crate::resize_pty;
pub use tiled_pane_grid::room_in_geometries;
use tiled_pane_grid::{split, TiledPaneGrid, RESIZE_PERCENT};

use crate::{
    os_input_output::ServerOsApi,
    output::Output,
    pane::{pane_info_for_pane, Pane, MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH},
    panes::{ActivePanes, PaneId},
    plugins::PluginInstruction,
    thread_bus::ThreadSenders,
    ui::boundaries::Boundaries,
    ui::pane_contents_and_ui::PaneContentsAndUi,
    ClientId,
};
use stacked_panes::StackedPanes;
use yinz_runtime::errors::prelude::*;
use yinz_types::style::{Style, Styling};
use yinz_types::{
    geometry::{Direction, Resize, ResizeStrategy, SplitDirection},
    pane_size::{Constraint, Offset, PaneGeom, Size, SizeInPixels, Viewport},
};
use {
    yinz_types::data::ModeInfo, yinz_types::data::PaneInfo, yinz_types::input::command::RunCommand,
    yinz_types::input::layout::Run, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::options::PaneFrameStyle,
};

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

pub fn pane_content_offset(position_and_size: &PaneGeom, viewport: &Viewport) -> (usize, usize) {
    // (columns_offset, rows_offset)
    // if the pane is not on the bottom or right edge on the screen, we need to reserve one space
    // from its content to leave room for the boundary between it and the next pane (if it doesn't
    // draw its own frame)
    let columns_offset =
        if position_and_size.x + position_and_size.cols.as_usize() < viewport.x + viewport.cols {
            1
        } else {
            0
        };
    let rows_offset =
        if position_and_size.y + position_and_size.rows.as_usize() < viewport.y + viewport.rows {
            1
        } else {
            0
        };
    (columns_offset, rows_offset)
}

/// Carry one axis of the viewport across a screen resize.
///
/// The viewport is the screen minus what the ui panes reserve at each end of the axis, so what
/// survives a resize is those two insets - `start`, and whatever lies past `start + extent` - not
/// the extent between them. On a screen no smaller than the chrome this is exactly the signed
/// delta it replaced: `new_total - start - trailing == new_total - old_total + extent`. On one
/// smaller than the chrome it clamps instead of subtracting past zero, so the result always
/// satisfies `start + extent <= new_total` - the invariant `is_inside_viewport` and
/// `pane_content_offset` read the viewport under, and the one the delta broke by wrapping around
/// `usize`. `FloatingPaneGrid::resize` carries its own viewport the same way.
fn resized_viewport_axis(
    start: usize,
    extent: usize,
    old_total: usize,
    new_total: usize,
) -> (usize, usize) {
    let trailing = old_total.saturating_sub(start.saturating_add(extent));
    let start = start.min(new_total);
    let extent = (new_total - start).saturating_sub(trailing);
    (start, extent)
}

/// The tab's display area as a viewport. A tab is laid out over the whole session surface, so its
/// origin is always `(0, 0)`.
fn display_area_viewport(display_area: &Rc<RefCell<Size>>) -> Viewport {
    let display_area = display_area.borrow();
    Viewport {
        x: 0,
        y: 0,
        cols: display_area.cols,
        rows: display_area.rows,
    }
}

/// A borderless plugin pane is part of the ui (eg. the tab-bar or the status-bar) and so is offset
/// out of the viewport just like a non-selectable pane - we cannot rely on selectability alone here,
/// because a plugin only reports it once it has loaded, and until then we'd be calculating (and
/// sending to the pty) wrong sizes for the other panes in this tab.
fn is_ui_pane(pane: &dyn Pane) -> bool {
    !pane.selectable() || (pane.borderless() && matches!(pane.pid(), PaneId::Plugin(_)))
}

pub struct TiledPanes {
    pub panes: BTreeMap<PaneId, Box<dyn Pane>>,
    display_area: Rc<RefCell<Size>>,
    viewport: Rc<RefCell<Viewport>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<AttachedClients>>,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    stacked_resize: Rc<RefCell<bool>>,
    reserved_top_rows: Rc<RefCell<HashMap<PaneId, usize>>>,
    default_mode_info: ModeInfo,
    style: Style,
    session_is_mirrored: bool,
    active_panes: ActivePanes,
    pane_frame_style: PaneFrameStyle,
    panes_to_hide: HashSet<PaneId>,
    fullscreen_is_active: Option<PaneId>,
    fullscreen_covers_ui: Rc<RefCell<bool>>,
    senders: ThreadSenders,
    window_title: Option<String>,
    client_id_to_boundaries: HashMap<ClientId, Boundaries>,
    tombstones_before_increase: Option<(PaneId, Vec<HashMap<PaneId, PaneGeom>>)>,
    tombstones_before_decrease: Option<(PaneId, Vec<HashMap<PaneId, PaneGeom>>)>,
    failed_tab_bar_resize: Option<(usize, Vec<(PaneId, PaneGeom)>)>,
    dimmed_clients: HashSet<ClientId>,
    agent_width_input: Vec<(PaneId, PaneGeom, usize, usize)>,
    agent_width_ui: Vec<PaneGeom>,
    agent_widths: HashMap<PaneId, (usize, usize)>,
}

impl TiledPanes {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        display_area: Rc<RefCell<Size>>,
        viewport: Rc<RefCell<Viewport>>,
        connected_clients: Rc<RefCell<HashSet<ClientId>>>,
        connected_clients_in_app: Rc<RefCell<AttachedClients>>,
        mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        stacked_resize: Rc<RefCell<bool>>,
        reserved_top_rows: Rc<RefCell<HashMap<PaneId, usize>>>,
        fullscreen_covers_ui: Rc<RefCell<bool>>,
        session_is_mirrored: bool,
        pane_frame_style: PaneFrameStyle,
        default_mode_info: ModeInfo,
        style: Style,
        os_api: Box<dyn ServerOsApi>,
        senders: ThreadSenders,
    ) -> Self {
        TiledPanes {
            panes: BTreeMap::new(),
            display_area,
            viewport,
            connected_clients,
            connected_clients_in_app,
            mode_info,
            character_cell_size,
            stacked_resize,
            reserved_top_rows,
            default_mode_info,
            style,
            session_is_mirrored,
            active_panes: ActivePanes::new(&os_api),
            pane_frame_style,
            panes_to_hide: HashSet::new(),
            fullscreen_is_active: None,
            fullscreen_covers_ui,
            senders,
            window_title: None,
            client_id_to_boundaries: HashMap::new(),
            tombstones_before_increase: None,
            tombstones_before_decrease: None,
            failed_tab_bar_resize: None,
            dimmed_clients: HashSet::new(),
            agent_width_input: Vec::new(),
            agent_width_ui: Vec::new(),
            agent_widths: HashMap::new(),
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
    pub fn boundary_viewport(&self) -> Viewport {
        let mut boundary_viewport = *self.viewport.borrow();
        if let Some(left) = self.agent_widths.values().map(|(x, _)| *x).min() {
            let right = self
                .agent_widths
                .values()
                .map(|(x, cols)| x + cols)
                .max()
                .unwrap_or(left);
            boundary_viewport.x = left;
            boundary_viewport.cols = right - left;
        }
        boundary_viewport
    }
    pub fn agent_widths(&mut self) -> Result<HashMap<PaneId, (usize, usize)>> {
        let input: Vec<_> = self
            .panes
            .iter()
            .filter(|(id, pane)| !self.panes_to_hide.contains(id) && !is_ui_pane(pane.as_ref()))
            .map(|(_, pane)| pane_resizer::agent_width_input(pane.as_ref()))
            .collect();
        let ui: Vec<_> = self
            .panes
            .iter()
            .filter(|(id, pane)| !self.panes_to_hide.contains(id) && is_ui_pane(pane.as_ref()))
            .map(|(_, pane)| pane.current_geom())
            .collect();
        if input != self.agent_width_input || ui != self.agent_width_ui {
            self.agent_widths = pane_resizer::agent_widths(&input, &ui)?;
            self.agent_width_ui = ui;
            self.agent_width_input = input;
        }
        Ok(self.agent_widths.clone())
    }
    pub fn add_pane_with_existing_geom(&mut self, pane_id: PaneId, mut pane: Box<dyn Pane>) {
        if self.pane_frame_style.draws_full_frames() {
            pane.set_content_offset(Offset::frame(1));
        }
        self.panes.insert(pane_id, pane);
    }
    /// Every pane's content size, taken before a resize so that afterwards only the
    /// panes it changed hear about it. Telling every pane made each step of a held
    /// resize key a SIGWINCH to every process in the tab.
    fn content_sizes(&self) -> std::collections::HashMap<PaneId, (usize, usize)> {
        self.panes
            .iter()
            .map(|(id, pane)| (*id, (pane.get_content_columns(), pane.get_content_rows())))
            .collect()
    }
    fn resize_ptys_of_panes_changed_since(
        &mut self,
        sizes_before: &std::collections::HashMap<PaneId, (usize, usize)>,
    ) {
        for (pane_id, pane) in self.panes.iter_mut() {
            let size = (pane.get_content_columns(), pane.get_content_rows());
            if sizes_before.get(pane_id) != Some(&size) {
                resize_pty!(pane, self.os_api, self.senders, self.character_cell_size).non_fatal();
            }
        }
    }
    /// Put a pane whose geometry a stack has already made room for into that stack, and
    /// tell the stack - and only the stack - what that did to its members' sizes.
    ///
    /// `add_pane_with_existing_geom` insets only under full frames, and the member the
    /// stack just collapsed to a title row keeps the offset it had as a flexible pane, so
    /// a bare insert leaves two panes whose content size is wrong until something
    /// relayouts. `reapply_pane_frames_in_stack_of` derives both from the style.
    fn insert_into_stack(&mut self, pane_id: PaneId, geom: PaneGeom, mut pane: Box<dyn Pane>) {
        pane.set_geom(geom);
        self.panes.insert(pane_id, pane);
        self.reapply_pane_frames_in_stack_of(&pane_id);
    }
    pub fn replace_active_pane(
        &mut self,
        pane: Box<dyn Pane>,
        client_id: ClientId,
    ) -> Option<Box<dyn Pane>> {
        let pane_id = pane.pid();
        // remove the currently active pane
        let previously_active_pane = self
            .active_panes
            .get(&client_id)
            .copied()
            .and_then(|active_pane_id| self.replace_pane(active_pane_id, pane));

        // move clients from the previously active pane to the new pane we just inserted
        if let Some(previously_active_pane) = previously_active_pane.as_ref() {
            let previously_active_pane_id = previously_active_pane.pid();
            self.move_clients_between_panes(previously_active_pane_id, pane_id);
        }
        previously_active_pane
    }
    pub fn replace_pane(
        &mut self,
        pane_id: PaneId,
        mut with_pane: Box<dyn Pane>,
    ) -> Option<Box<dyn Pane>> {
        let with_pane_id = with_pane.pid();
        if self.pane_frame_style.draws_full_frames() && !with_pane.borderless() {
            with_pane.set_content_offset(Offset::frame(1));
        }
        let removed_pane = self.panes.remove(&pane_id).inspect(|removed_pane| {
            let with_pane_id = with_pane.pid();
            let removed_pane_geom = removed_pane.position_and_size();
            let removed_pane_geom_override = removed_pane.geom_override();
            with_pane.set_geom(removed_pane_geom);
            match removed_pane_geom_override {
                Some(geom_override) => with_pane.set_geom_override(geom_override),
                None => with_pane.reset_size_and_position_override(),
            };
            self.panes.insert(with_pane_id, with_pane);
            // The replacement inherits the removed pane's geom_override (above);
            // if the removed pane was the fullscreen pane, retarget the fullscreen
            // bookkeeping so unset_fullscreen can later reset the override on the
            // pane that actually carries it.
            if self.fullscreen_is_active == Some(pane_id) {
                self.fullscreen_is_active = Some(with_pane_id);
            }
            if self.panes_to_hide.remove(&pane_id) {
                self.panes_to_hide.insert(with_pane_id);
            }
        });

        // move clients from the previously active pane to the new pane we just inserted
        self.move_clients_between_panes(pane_id, with_pane_id);
        self.reapply_pane_frames();
        removed_pane
    }
    /// Put a pane in the grid, or hand it back if there is nowhere to put it.
    ///
    /// `Some` means the pane was *not* placed and is still the caller's to
    /// dispose of. The alternative — swallowing the box on the failure paths —
    /// destroys the pane: no `PaneClosed` goes out, the pty thread is never told
    /// to close it, and nothing is left holding the id, so its process runs on
    /// with no owner and no way to reap it.
    #[must_use]
    pub fn insert_pane(
        &mut self,
        pane_id: PaneId,
        pane: Box<dyn Pane>,
        client_id: Option<ClientId>,
    ) -> Option<Box<dyn Pane>> {
        let should_relayout = true;
        self.add_pane(pane_id, pane, should_relayout, client_id)
    }
    /// Run insertion against empty pane contents and private geometry/focus state. The supplied
    /// panes have no terminal output or focus reports, and all thread sends stay disconnected.
    pub(crate) fn prepare_insertions(
        &self,
        panes: BTreeMap<PaneId, Box<dyn Pane>>,
        remaining: Vec<Box<dyn Pane>>,
        client_id: Option<ClientId>,
        os_api: Box<dyn ServerOsApi>,
    ) -> Result<BTreeMap<PaneId, PaneGeom>> {
        let mut scratch = Self::new(
            Rc::new(RefCell::new(*self.display_area.borrow())),
            Rc::new(RefCell::new(*self.viewport.borrow())),
            Rc::new(RefCell::new(self.connected_clients.borrow().clone())),
            Rc::new(RefCell::new(self.connected_clients_in_app.borrow().clone())),
            Rc::new(RefCell::new(self.mode_info.borrow().clone())),
            self.character_cell_size.clone(),
            self.stacked_resize.clone(),
            Rc::new(RefCell::new(self.reserved_top_rows.borrow().clone())),
            Rc::new(RefCell::new(*self.fullscreen_covers_ui.borrow())),
            self.session_is_mirrored,
            self.pane_frame_style,
            self.default_mode_info.clone(),
            self.style,
            os_api,
            ThreadSenders::default().silently_fail_on_send(),
        );
        scratch.panes = panes;
        scratch.active_panes = self.active_panes.clone();
        for pane in remaining {
            let id = pane.pid();
            let logical_position = pane.position_and_size().logical_position;
            if scratch.insert_pane(id, pane, client_id).is_some() {
                return Err(anyhow!("not enough room for retained pane {id:?}"));
            }
            if let Some(position) = logical_position {
                scratch.set_pane_logical_position(id, position);
            }
        }
        Ok(scratch
            .panes
            .into_iter()
            .map(|(id, pane)| (id, pane.position_and_size()))
            .collect())
    }

    pub fn set_pane_logical_position(&mut self, pane_id: PaneId, logical_position: usize) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            let mut position_and_size = pane.position_and_size();
            position_and_size.logical_position = Some(logical_position);
            pane.set_geom(position_and_size);
        }
    }
    /// As [`insert_pane`](Self::insert_pane), including handing the pane back.
    #[must_use]
    pub fn insert_pane_without_relayout(
        &mut self,
        pane_id: PaneId,
        pane: Box<dyn Pane>,
        client_id: Option<ClientId>,
    ) -> Option<Box<dyn Pane>> {
        let should_relayout = false;
        self.add_pane(pane_id, pane, should_relayout, client_id)
    }
    /// Only unselectable panes that do not touch the viewport remain, such as the UI
    /// panes a warp leaves behind when it takes a tab's last content pane.
    fn has_no_content_in_viewport(&self) -> bool {
        let viewport = *self.viewport.borrow();
        self.panes.values().all(|pane| {
            let geom = pane.current_geom();
            let overlaps = geom.x < viewport.x + viewport.cols
                && viewport.x < geom.x + geom.cols.as_usize()
                && geom.y < viewport.y + viewport.rows
                && viewport.y < geom.y + geom.rows.as_usize();
            !pane.selectable() && !overlaps
        })
    }
    pub fn has_room_for_new_pane(&mut self) -> bool {
        if self.has_no_content_in_viewport() {
            return true;
        }
        let cursor_height_width_ratio = self.cursor_height_width_ratio();
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let has_room_for_new_pane = pane_grid
            .find_room_for_new_pane(cursor_height_width_ratio)
            .is_some();
        has_room_for_new_pane || pane_grid.has_room_for_new_stacked_pane() || self.panes.is_empty()
    }
    pub fn room_left_in_stack_of_pane_id(&mut self, pane_id: &PaneId) -> Option<usize> {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        pane_grid.room_left_in_stack_of_pane_id(pane_id)
    }

    pub fn assign_geom_for_pane_with_run(&mut self, run: Option<Run>) {
        // here we're removing the first pane we find with this run instruction and re-adding it so
        // that it gets a new geom similar to how it would when being added to the tab originally
        if let Some(pane_id) = self
            .panes
            .iter()
            .find_map(|(pid, p)| {
                if p.invoked_with() == &run {
                    Some(pid)
                } else {
                    None
                }
            })
            .copied()
            && let Some(mut pane) = self.panes.remove(&pane_id)
        {
            // we must strip the logical position here because it's likely a straggler from
            // this pane's previous tab and would cause chaos if considered in the new one
            let mut pane_geom = pane.position_and_size();
            pane_geom.logical_position = None;
            pane.set_geom(pane_geom);
            self.add_pane_with_existing_geom(pane.pid(), pane);
        }
    }
    /// As [`insert_pane`](Self::insert_pane), including handing the pane back.
    /// Put every pane in `panes` into the stack of `pane_id_in_stack`, in order, and hand
    /// back the ones that could not be placed, as [`insert_pane`](Self::insert_pane) does.
    ///
    /// One pass, then one reframe: making room reshapes every member through
    /// `StackedPanes`, so reframing per insert would tell the stack `O(n^2)` sizes, all
    /// but the last stale by the time the next pane went in.
    #[must_use]
    pub fn add_panes_to_stack(
        &mut self,
        pane_id_in_stack: &PaneId,
        panes: Vec<Box<dyn Pane>>,
    ) -> Vec<Box<dyn Pane>> {
        let mut not_placed = vec![];
        for mut pane in panes {
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            match pane_grid.make_room_in_stack_of_pane_id_for_pane(pane_id_in_stack) {
                Ok(new_pane_geom) => {
                    pane.set_geom(new_pane_geom);
                    self.panes.insert(pane.pid(), pane);
                },
                Err(_e) => {
                    let should_relayout = false;
                    if let Some(pane) =
                        self.add_pane_without_stacked_resize(pane.pid(), pane, should_relayout)
                    {
                        not_placed.push(pane);
                    }
                },
            }
        }
        self.reapply_pane_frames_in_stack_of(pane_id_in_stack);
        not_placed
    }
    fn add_pane(
        &mut self,
        pane_id: PaneId,
        mut pane: Box<dyn Pane>,
        should_relayout: bool,
        client_id: Option<ClientId>,
    ) -> Option<Box<dyn Pane>> {
        if self.has_no_content_in_viewport() {
            // A warp can leave an empty tab or only UI panes outside the viewport.
            let viewport = *self.viewport.borrow();
            let mut geom = PaneGeom::from(&Size {
                rows: viewport.rows,
                cols: viewport.cols,
            });
            geom.x = viewport.x;
            geom.y = viewport.y;
            pane.set_geom(geom);
            self.panes.insert(pane_id, pane);
            return None;
        }
        let stacked_resize = { *self.stacked_resize.borrow() };

        if let Some(client_id) = client_id
            && stacked_resize
            && self.is_connected(&client_id)
        {
            return self.add_pane_with_stacked_resize(pane_id, pane, should_relayout, client_id);
        }
        self.add_pane_without_stacked_resize(pane_id, pane, should_relayout)
    }
    fn add_pane_without_stacked_resize(
        &mut self,
        pane_id: PaneId,
        mut pane: Box<dyn Pane>,
        should_relayout: bool,
    ) -> Option<Box<dyn Pane>> {
        let cursor_height_width_ratio = self.cursor_height_width_ratio();
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let pane_id_and_split_direction =
            pane_grid.find_room_for_new_pane(cursor_height_width_ratio);
        match pane_id_and_split_direction {
            Some((pane_id_to_split, split_direction)) => {
                // this unwrap is safe because floating panes should not be visible if there are no floating panes
                let pane_to_split = self.panes.get_mut(&pane_id_to_split).unwrap();
                let size_of_both_panes = pane_to_split.position_and_size();
                if let Some((first_geom, second_geom)) = split(split_direction, &size_of_both_panes)
                {
                    pane_to_split.set_geom(first_geom);
                    pane.set_geom(second_geom);
                    self.panes.insert(pane_id, pane);
                    if should_relayout {
                        self.relayout(!split_direction);
                    }
                    None
                } else {
                    // the pane it was going to be split off is too small to
                    // divide, which is a reason not to place this pane and not a
                    // reason to destroy it
                    Some(pane)
                }
            },
            None => {
                // we couldn't add the pane normally, let's see if there's room in one of the
                // stacks...
                match pane_grid.make_room_in_stack_for_pane() {
                    Ok(new_pane_geom) => {
                        pane.set_geom(new_pane_geom);
                        self.add_pane_with_existing_geom(pane_id, pane);
                        self.expand_pane_in_stack(pane_id);
                        None
                    },
                    Err(e) => {
                        log::error!("Failed to add pane to stack: {:?}", e);
                        Some(pane)
                    },
                }
            },
        }
    }
    fn add_pane_with_stacked_resize(
        &mut self,
        pane_id: PaneId,
        mut pane: Box<dyn Pane>,
        should_relayout: bool,
        client_id: ClientId,
    ) -> Option<Box<dyn Pane>> {
        let cursor_height_width_ratio = self.cursor_height_width_ratio();
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let Some(active_pane_id) = self.active_panes.get(&client_id) else {
            // Placed without reference to the client rather than not placed at
            // all: `pane` is owned here, so returning would drop it — the pane
            // would cease to exist with no `PaneClosed`, no `ClosePane` for the
            // pty thread and nothing left that could ever reap its process.
            // Stacked resize is about *where* a pane goes, and having nowhere in
            // particular to put it is not a reason to destroy it.
            //
            // Reachable whenever the client's active pane has left the grid — a
            // plugin hid it, a scrollback editor took its place — and the client
            // has not been focused anywhere since.
            drop(pane_grid);
            return self.add_pane_without_stacked_resize(pane_id, pane, should_relayout);
        };
        if pane_grid
            .get_pane_geom(active_pane_id)
            .map(|p| p.is_stacked())
            .unwrap_or(false)
        {
            // try to add the pane to the stack of the active pane
            match pane_grid.make_room_in_stack_of_pane_id_for_pane(active_pane_id) {
                Ok(new_pane_geom) => {
                    pane.set_geom(new_pane_geom);
                    self.panes.insert(pane_id, pane);

                    self.expand_pane_in_stack(pane_id);
                    return None;
                },
                Err(_e) => {
                    return self.add_pane_without_stacked_resize(pane_id, pane, should_relayout);
                },
            }
        }
        let pane_id_and_split_direction =
            pane_grid.split_pane(active_pane_id, cursor_height_width_ratio);
        match pane_id_and_split_direction {
            Some((pane_id_to_split, split_direction)) => {
                // this unwrap is safe because floating panes should not be visible if there are no floating panes
                let pane_to_split = self.panes.get_mut(&pane_id_to_split).unwrap();
                let size_of_both_panes = pane_to_split.position_and_size();
                if let Some((first_geom, second_geom)) = split(split_direction, &size_of_both_panes)
                {
                    pane_to_split.set_geom(first_geom);
                    pane.set_geom(second_geom);
                    self.panes.insert(pane_id, pane);
                    if should_relayout {
                        self.relayout(!split_direction);
                    }
                    None
                } else {
                    Some(pane)
                }
            },
            None => {
                // we couldn't add the pane normally, let's see if there's room in one of the
                // stacks...
                let newly_stacked = pane_grid.make_pane_stacked(active_pane_id).is_ok();
                match pane_grid.make_room_in_stack_of_pane_id_for_pane(active_pane_id) {
                    Ok(new_pane_geom) => {
                        self.insert_into_stack(pane_id, new_pane_geom, pane);
                        None
                    },
                    Err(_e) => {
                        if newly_stacked {
                            pane_grid.unstack_pane(active_pane_id);
                        }
                        self.add_pane_without_stacked_resize(pane_id, pane, should_relayout)
                    },
                }
            },
        }
    }
    /// Put `pane` in the stack of `client_id`'s active pane, making a stack of that pane
    /// if it is not in one, or hand the pane back if there is no room for it.
    ///
    /// `Some` means the pane was *not* placed and is still the caller's to dispose of,
    /// as with [`insert_pane`](Self::insert_pane).
    #[must_use]
    pub fn add_pane_to_stack_of_active_pane(
        &mut self,
        pane_id: PaneId,
        pane: Box<dyn Pane>,
        client_id: ClientId,
    ) -> Option<Box<dyn Pane>> {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let Some(active_pane_id) = self.active_panes.get(&client_id) else {
            log::error!("Could not find active pane id for client_id");
            return Some(pane);
        };
        let pane_id_is_stacked = pane_grid
            .get_pane_geom(active_pane_id)
            .map(|p| p.is_stacked())
            .unwrap_or(false);
        let newly_stacked =
            !pane_id_is_stacked && pane_grid.make_pane_stacked(active_pane_id).is_ok();
        match pane_grid.make_room_in_stack_of_pane_id_for_pane(active_pane_id) {
            Ok(new_pane_geom) => {
                self.insert_into_stack(pane_id, new_pane_geom, pane);
                None
            },
            Err(e) => {
                if newly_stacked {
                    pane_grid.unstack_pane(active_pane_id);
                }
                log::error!("Failed to add pane to stack: {}", e);
                Some(pane)
            },
        }
    }
    /// Put `pane` in the stack of `root_pane_id`, making a stack of that pane if it is
    /// not in one, or hand the pane back if there is no room for it.
    ///
    /// `Some` means the pane was *not* placed and is still the caller's to dispose of,
    /// as with [`insert_pane`](Self::insert_pane).
    #[must_use]
    pub fn add_pane_to_stack_of_pane_id(
        &mut self,
        pane_id: PaneId,
        pane: Box<dyn Pane>,
        root_pane_id: PaneId,
    ) -> Option<Box<dyn Pane>> {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let pane_id_is_stacked = pane_grid
            .get_pane_geom(&root_pane_id)
            .map(|p| p.is_stacked())
            .unwrap_or(false);
        let mut newly_stacked = false;
        if !pane_id_is_stacked {
            match pane_grid.make_pane_stacked(&root_pane_id) {
                Ok(()) => newly_stacked = true,
                Err(e) => log::error!("Failed to make pane stacked: {:?}", e),
            }
        }
        match pane_grid.make_room_in_stack_of_pane_id_for_pane(&root_pane_id) {
            Ok(new_pane_geom) => {
                self.insert_into_stack(pane_id, new_pane_geom, pane);
                None
            },
            Err(e) => {
                if newly_stacked {
                    pane_grid.unstack_pane(&root_pane_id);
                }
                log::error!("Failed to add pane to stack: {}", e);
                Some(pane)
            },
        }
    }
    pub fn fixed_pane_geoms(&self) -> Vec<Viewport> {
        self.panes
            .values()
            .filter_map(|p| {
                let geom = p.position_and_size();
                if geom.cols.is_fixed() || geom.rows.is_fixed() {
                    Some(geom.into())
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn borderless_pane_geoms(&self) -> Vec<Viewport> {
        self.panes
            .values()
            .filter_map(|p| {
                let geom = p.position_and_size();
                if p.borderless() {
                    Some(geom.into())
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn non_selectable_pane_geoms_inside_viewport(&self) -> Vec<Viewport> {
        self.panes
            .values()
            .filter_map(|p| {
                let geom = p.position_and_size();
                if is_ui_pane(&**p) && is_inside_viewport(&self.viewport.borrow(), p) {
                    Some(geom.into())
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn first_selectable_pane_id(&self) -> Option<PaneId> {
        self.panes
            .iter()
            .filter(|(_id, pane)| pane.selectable())
            .map(|(id, _)| id.to_owned())
            .next()
    }
    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.keys()
    }
    /// The panes whose content is on screen: hidden ones are out, and so are collapsed
    /// stack members, which show only their one-row title. Image placements belonging
    /// to a pane that is not in here are deleted from the host.
    pub fn rendered_pane_ids(&self) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|(pane_id, pane)| {
                if self.panes_to_hide.contains(pane_id) {
                    return false;
                }
                let geom = pane.current_geom();
                !(geom.is_stacked() && geom.rows.is_fixed())
            })
            .map(|(pane_id, _pane)| *pane_id)
            .collect()
    }
    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        for pane in self.panes.values_mut() {
            pane.character_cell_size_possibly_changed();
            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                .with_context(|| format!("failed to resize PTY in pane {:?}", pane.pid()))?;
        }
        Ok(())
    }
    pub fn relayout(&mut self, direction: SplitDirection) {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        match direction {
            SplitDirection::Horizontal => {
                pane_grid.layout(direction, self.display_area.borrow().cols)
            },
            SplitDirection::Vertical => {
                pane_grid.layout(direction, self.display_area.borrow().rows)
            },
        }
        .map_err(anyError::msg)
        .with_context(|| format!("{:?} relayout of tab failed", direction))
        .non_fatal();

        self.set_pane_frames(self.pane_frame_style);
    }
    pub fn reapply_pane_frames(&mut self) {
        // same as set_pane_frames except it reapplies the current situation
        self.set_pane_frames(self.pane_frame_style);
    }
    /// Give every member of `pane_id`'s stack the frame and content offset the current
    /// style calls for, and tell their ptys the size that leaves them.
    ///
    /// Making room for a new member reshapes the others through `StackedPanes`, which
    /// changes their geometry without touching offsets or ptys. A relayout would do both,
    /// but the stack paths run none, and `reapply_pane_frames` would do it for the whole
    /// tab: a SIGWINCH to every process in it for a change confined to one stack.
    pub fn reapply_pane_frames_in_stack_of(&mut self, pane_id: &PaneId) {
        let pane_ids = self.pane_ids_in_stack_of_pane_id(pane_id);
        let frame_context = self.frame_context();
        for id in pane_ids {
            if let Some(pane) = self.panes.get_mut(&id) {
                apply_frame(&frame_context, pane);
                resize_pty!(pane, self.os_api, self.senders, self.character_cell_size).non_fatal();
            }
        }
        self.reset_boundaries();
    }
    pub fn set_pane_frames(&mut self, pane_frame_style: PaneFrameStyle) {
        self.pane_frame_style = pane_frame_style;
        let frame_context = self.frame_context();
        for pane in self.panes.values_mut() {
            apply_frame(&frame_context, pane);
            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size).non_fatal();
        }
        self.reset_boundaries();
    }
    /// What `apply_frame` needs to know about the tab as a whole, gathered once: the
    /// stacks' rectangles and the count of selectable panes are answers about the whole
    /// map, and asking per pane would be quadratic.
    fn frame_context(&mut self) -> FrameContext {
        let draws_full_frames = self.pane_frame_style.draws_full_frames();
        let draws_titles = self.pane_frame_style.draws_titles();
        let single_selectable_tiled_pane = self
            .panes
            .values()
            .filter(|p| p.selectable() && !p.borderless())
            .count()
            == 1;
        let no_ui_fullscreen_pane_id = if *self.fullscreen_covers_ui.borrow() {
            self.fullscreen_is_active
        } else {
            None
        };
        let position_and_sizes_of_stacks =
            StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .positions_and_sizes_of_all_stacks()
                .unwrap_or_default();
        let reserved_top_rows = if self.fullscreen_is_active.is_some() {
            HashMap::new()
        } else {
            self.reserved_top_rows.borrow().clone()
        };
        FrameContext {
            draws_full_frames,
            draws_titles,
            single_selectable_tiled_pane,
            viewport: *self.viewport.borrow(),
            no_ui_fullscreen_pane_id,
            position_and_sizes_of_stacks,
            reserved_top_rows,
        }
    }
    pub fn can_split_pane_horizontally(&mut self, client_id: ClientId) -> bool {
        match self.active_panes.get(&client_id).copied() {
            Some(active_pane_id) => self.can_split_pane_id_horizontally(active_pane_id),
            None => false,
        }
    }
    pub fn can_split_pane_id_horizontally(&mut self, active_pane_id: PaneId) -> bool {
        if let Some(active_pane) = self.panes.get_mut(&active_pane_id) {
            let mut full_pane_size = active_pane.position_and_size();

            if full_pane_size.is_stacked() {
                let stack = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide);
                if stack.flexible_pane_id_in_stack(&active_pane_id).is_none() {
                    return false;
                }
                let Some(position_and_size_of_stack) =
                    stack.position_and_size_of_stack(&active_pane_id)
                else {
                    log::error!("Failed to find position and size of stack");
                    return false;
                };
                full_pane_size = position_and_size_of_stack;
            }

            if full_pane_size.rows.as_usize() < MIN_TERMINAL_HEIGHT * 2 {
                return false;
            } else {
                return split(SplitDirection::Horizontal, &full_pane_size).is_some();
            }
        }
        false
    }
    pub fn can_split_pane_vertically(&mut self, client_id: ClientId) -> bool {
        match self.active_panes.get(&client_id).copied() {
            Some(active_pane_id) => self.can_split_pane_id_vertically(active_pane_id),
            None => false,
        }
    }
    pub fn can_split_pane_id_vertically(&mut self, active_pane_id: PaneId) -> bool {
        if let Some(active_pane) = self.panes.get_mut(&active_pane_id) {
            let mut full_pane_size = active_pane.position_and_size();

            if full_pane_size.is_stacked() {
                let stack = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide);
                if stack.flexible_pane_id_in_stack(&active_pane_id).is_none() {
                    return false;
                }
                let Some(position_and_size_of_stack) =
                    stack.position_and_size_of_stack(&active_pane_id)
                else {
                    log::error!("Failed to find position and size of stack");
                    return false;
                };
                full_pane_size = position_and_size_of_stack;
            }

            if full_pane_size.cols.as_usize() < MIN_TERMINAL_WIDTH * 2 {
                return false;
            }
            return split(SplitDirection::Vertical, &full_pane_size).is_some();
        }
        false
    }
    pub fn split_pane_horizontally(
        &mut self,
        pid: PaneId,
        new_pane: Box<dyn Pane>,
        client_id: ClientId,
    ) {
        let active_pane_id = *self.active_panes.get(&client_id).unwrap();
        self.split_pane_id_horizontally(pid, new_pane, active_pane_id);
    }
    pub fn split_pane_id_horizontally(
        &mut self,
        pid: PaneId,
        mut new_pane: Box<dyn Pane>,
        active_pane_id: PaneId,
    ) {
        let mut full_pane_size = self
            .panes
            .get(&active_pane_id)
            .map(|p| p.position_and_size())
            .unwrap();
        if full_pane_size.is_stacked() {
            match StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .position_and_size_of_stack(&active_pane_id)
            {
                Some(position_and_size_of_stack) => {
                    full_pane_size = position_and_size_of_stack;
                },
                None => {
                    log::error!("Failed to find position and size of stack");
                },
            }
        }
        let active_pane = self.panes.get_mut(&active_pane_id).unwrap();
        if let Some((top_winsize, bottom_winsize)) =
            split(SplitDirection::Horizontal, &full_pane_size)
        {
            if active_pane.position_and_size().is_stacked() {
                match StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                    .resize_panes_in_stack(&active_pane_id, top_winsize)
                {
                    Ok(_) => {},
                    Err(e) => {
                        log::error!("Failed to resize stack: {}", e);
                    },
                }
            } else {
                active_pane.set_geom(top_winsize);
            }
            new_pane.set_geom(bottom_winsize);
            self.panes.insert(pid, new_pane);
            self.relayout(SplitDirection::Vertical);
        }
    }
    pub fn split_pane_vertically(
        &mut self,
        pid: PaneId,
        new_pane: Box<dyn Pane>,
        client_id: ClientId,
    ) {
        let active_pane_id = *self.active_panes.get(&client_id).unwrap();
        self.split_pane_id_vertically(pid, new_pane, active_pane_id);
    }
    pub fn split_pane_id_vertically(
        &mut self,
        pid: PaneId,
        mut new_pane: Box<dyn Pane>,
        active_pane_id: PaneId,
    ) {
        let mut full_pane_size = self
            .panes
            .get(&active_pane_id)
            .map(|p| p.position_and_size())
            .unwrap();
        if full_pane_size.is_stacked() {
            match StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .position_and_size_of_stack(&active_pane_id)
            {
                Some(position_and_size_of_stack) => {
                    full_pane_size = position_and_size_of_stack;
                },
                None => {
                    log::error!("Failed to find position and size of stack");
                },
            }
        }
        let active_pane = self.panes.get_mut(&active_pane_id).unwrap();
        if let Some((left_winsize, right_winsize)) =
            split(SplitDirection::Vertical, &full_pane_size)
        {
            if active_pane.position_and_size().is_stacked() {
                match StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                    .resize_panes_in_stack(&active_pane_id, left_winsize)
                {
                    Ok(_) => {},
                    Err(e) => {
                        log::error!("Failed to resize stack: {}", e);
                    },
                }
            } else {
                active_pane.set_geom(left_winsize);
            }
            new_pane.set_geom(right_winsize);
            self.panes.insert(pid, new_pane);
            self.relayout(SplitDirection::Horizontal);
        }
    }
    pub fn focus_pane_for_all_clients(&mut self, pane_id: PaneId) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            if self
                .panes
                .get(&pane_id)
                .map(|p| p.current_geom().is_stacked())
                .unwrap_or(false)
            {
                let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                    .expand_pane(&pane_id);
            }
            self.focus_pane(pane_id, client_id);
        }
        self.set_force_render();
        self.reapply_pane_frames();
    }
    pub fn pane_ids_in_stack_of_pane_id(&mut self, pane_id: &PaneId) -> Vec<PaneId> {
        if let Some(stack_id) = self
            .panes
            .get(pane_id)
            .and_then(|p| p.position_and_size().stacked)
        {
            StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .pane_ids_in_stack(stack_id)
        } else {
            vec![]
        }
    }
    pub fn focus_pane_for_all_clients_in_stack(&mut self, pane_id: PaneId, stack_id: usize) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        let pane_ids_in_stack = {
            StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .pane_ids_in_stack(stack_id)
        };
        if self
            .panes
            .get(&pane_id)
            .map(|p| p.current_geom().is_stacked())
            .unwrap_or(false)
        {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&pane_id);
        }
        for client_id in connected_clients {
            if self
                .active_panes
                .get(&client_id)
                .map(|p_id| pane_ids_in_stack.contains(p_id))
                .unwrap_or(false)
            {
                self.active_panes.insert(client_id, pane_id, &self.panes);
                self.set_pane_active_at(pane_id);
            }
        }
        self.set_force_render();
        self.reapply_pane_frames();
    }
    pub fn reapply_pane_focus(&mut self) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        let mut stack_ids_to_pane_ids_to_expand = vec![];
        for client_id in connected_clients {
            match &self.active_panes.get(&client_id).copied() {
                Some(pane_id) => {
                    if let Some(stack_id) = self
                        .panes
                        .get(pane_id)
                        .and_then(|p| p.current_geom().stacked)
                    {
                        stack_ids_to_pane_ids_to_expand.push((stack_id, *pane_id));
                    }
                    self.active_panes.insert(client_id, *pane_id, &self.panes);
                    self.set_pane_active_at(*pane_id);
                },
                None => {
                    if let Some(first_pane_id) = self.first_selectable_pane_id() {
                        let pane_id = first_pane_id;
                        if let Some(stack_id) = self
                            .panes
                            .get(&pane_id)
                            .and_then(|p| p.current_geom().stacked)
                        {
                            stack_ids_to_pane_ids_to_expand.push((stack_id, pane_id));
                        }
                        self.active_panes.insert(client_id, pane_id, &self.panes);
                        self.set_pane_active_at(pane_id);
                    }
                },
            }
        }
        for (stack_id, pane_id) in stack_ids_to_pane_ids_to_expand {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&pane_id);
            self.focus_pane_for_all_clients_in_stack(pane_id, stack_id);
        }
        self.set_force_render();
        self.reapply_pane_frames();
    }
    pub fn expand_pane_in_stack(&mut self, pane_id: PaneId) -> Vec<PaneId> {
        // returns all pane ids in stack
        match StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
            .expand_pane(&pane_id)
        {
            Ok(all_panes_in_stack) => {
                let connected_clients: Vec<ClientId> =
                    self.connected_clients.borrow().iter().copied().collect();
                for client_id in connected_clients {
                    if let Some(focused_pane_id_for_client) =
                        self.active_panes.get(&client_id).copied()
                        && all_panes_in_stack.contains(&focused_pane_id_for_client)
                    {
                        if focused_pane_id_for_client != pane_id {
                            self.active_panes
                                .set_last_pane(client_id, focused_pane_id_for_client);
                        }
                        self.active_panes.insert(client_id, pane_id, &self.panes);
                        self.set_pane_active_at(pane_id);
                    }
                }
                self.set_force_render();
                self.reapply_pane_frames();
                all_panes_in_stack
            },
            Err(e) => {
                log::error!("Failed to expand pane in stack: {:?}", e);
                vec![]
            },
        }
    }
    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        let pane_is_selectable = self
            .panes
            .get(&pane_id)
            .map(|p| p.selectable())
            .unwrap_or(false);
        if !pane_is_selectable {
            log::error!("Cannot focus pane {:?} as it is not selectable!", pane_id);
            return;
        }
        if let Some(focused_pane) = self.active_panes.get(&client_id).copied()
            && pane_id != focused_pane
        {
            self.active_panes.set_last_pane(client_id, focused_pane);
        }
        if self.panes_to_hide.contains(&pane_id) {
            // this means there is a fullscreen pane that is not the current pane, let's unset it
            // before changing focus
            self.unset_fullscreen();
        }
        if let Some(stack_id) = self
            .panes
            .get(&pane_id)
            .and_then(|p| p.current_geom().stacked)
        {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&pane_id);
            self.focus_pane_for_all_clients_in_stack(pane_id, stack_id);
            self.reapply_pane_frames();
        }
        self.active_panes.insert(client_id, pane_id, &self.panes);
        if self.session_is_mirrored {
            // move all clients
            let connected_clients: Vec<ClientId> =
                self.connected_clients.borrow().iter().copied().collect();
            for client_id in connected_clients {
                self.active_panes.insert(client_id, pane_id, &self.panes);
            }
        }
        self.reset_boundaries();
    }
    pub fn focus_pane_if_exists(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        if self.panes.contains_key(&pane_id) {
            self.focus_pane(pane_id, client_id);
            Ok(())
        } else {
            Err(anyhow!("Pane not found"))
        }
    }
    pub fn focus_pane_at_position(&mut self, position_and_size: PaneGeom, client_id: ClientId) {
        if let Some(pane_id) = self
            .panes
            .iter()
            .find(|(_pid, pane)| pane.position_and_size() == position_and_size)
            .map(|(pid, _p)| *pid)
            && let Some(currently_active_pane_id) = self.active_panes.get(&client_id)
        {
            let prev_geom = {
                if let Some(currently_focused_pane) = self.panes.get_mut(currently_active_pane_id) {
                    let prev_geom = currently_focused_pane.position_and_size();
                    currently_focused_pane.set_geom(position_and_size);
                    Some(prev_geom)
                } else {
                    None
                }
            };
            if let Some(prev_geom) = prev_geom
                && let Some(previous_pane) = self.panes.get_mut(&pane_id)
            {
                previous_pane.set_geom(prev_geom);
                self.reset_boundaries();
            }
        }
    }
    pub fn focus_pane_if_client_not_focused(&mut self, pane_id: PaneId, client_id: ClientId) {
        match self.active_panes.get(&client_id) {
            Some(already_focused_pane_id) => self.focus_pane(*already_focused_pane_id, client_id),
            None => self.focus_pane(pane_id, client_id),
        }
    }
    pub fn clear_active_panes(&mut self) {
        self.active_panes.clear(&self.panes);
        self.reset_boundaries();
    }
    pub fn first_active_pane_id(&self) -> Option<PaneId> {
        self.connected_clients
            .borrow()
            .iter()
            .next()
            .and_then(|first_client_id| self.active_panes.get(first_client_id).copied())
    }
    pub fn focused_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get(&client_id).copied()
    }
    #[allow(clippy::borrowed_box)]
    pub fn get_pane(&self, pane_id: PaneId) -> Option<&Box<dyn Pane>> {
        self.panes.get(&pane_id)
    }
    pub fn get_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.panes.get_mut(&pane_id)
    }
    pub fn get_active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get(&client_id).copied()
    }
    pub fn get_last_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.active_panes.get_last(&client_id).copied()
    }
    pub fn panes_contain(&self, pane_id: &PaneId) -> bool {
        self.panes.contains_key(pane_id)
    }
    pub fn set_force_render(&mut self) {
        for pane in self.panes.values_mut() {
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
        self.reset_boundaries();
    }
    /// Repaint only the panes overlapping `damage` - the cells an overlay stopped covering.
    ///
    /// The narrowed twin of `set_force_render` above. A toast sits in a corner and overlaps a pane
    /// or two, so repainting the whole container every frame of a slide-out - which is what the
    /// unnarrowed version did - re-serializes grids that never had a toast anywhere near them.
    pub fn force_render_panes_intersecting(&mut self, damage: &[CellRect]) {
        let mut hit = false;
        for pane in self.panes.values_mut() {
            let geom = pane.render_geom();
            if !damage.iter().any(|rect| rect.intersects_geom(&geom)) {
                continue;
            }
            hit = true;
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
        if hit {
            // the boundary cache is a whole-container diff, so any repaint inside it invalidates
            self.reset_boundaries();
        }
    }
    pub fn has_active_panes(&self) -> bool {
        !self.active_panes.is_empty()
    }
    pub fn has_panes(&self) -> bool {
        !self.panes.is_empty()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        output: &mut Output,
        floating_panes_are_visible: bool,
        mouse_hover_pane_id: &HashMap<ClientId, PaneId>,
        current_pane_group: HashMap<ClientId, Vec<PaneId>>,
        client_id_override: Option<ClientId>,
        help_text_visible: &HashMap<ClientId, bool>,
        mouse_scroll_resize: bool,
        same_worktree_panes: &HashMap<ClientId, HashSet<PaneId>>,
    ) -> Result<()> {
        let err_context = || "failed to render tiled panes";

        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };

        // If we have a client_id_override (for watcher rendering), add it temporarily
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }

        let connected_clients: Vec<ClientId> = connected_clients.into_iter().collect();
        let multiplayer = self.connected_clients_in_app.borrow().snapshot();
        let boundary_viewport = self.boundary_viewport();
        let mut client_id_to_boundaries: HashMap<ClientId, Boundaries> = HashMap::new();
        let active_panes = if floating_panes_are_visible {
            HashMap::new()
        } else {
            self.active_panes
                .iter()
                .filter(|(client_id, _pane_id)| connected_clients.contains(client_id))
                .map(|(client_id, pane_id)| (*client_id, *pane_id))
                .collect()
        };
        let (stacked_pane_ids_under_flexible_pane, stacked_pane_ids_over_flexible_pane) = {
            StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .stacked_pane_ids_under_and_over_flexible_panes()
                .with_context(err_context)?
        };
        let (stacked_pane_ids_on_top_of_stacks, stacked_pane_ids_on_bottom_of_stacks) = {
            StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .stacked_pane_ids_on_top_and_bottom_of_stacks()
                .with_context(err_context)?
        };
        let selectable_pane_count = self.panes.iter().filter(|(_, p)| p.selectable()).count();
        let content_pane_count = self
            .panes
            .iter()
            .filter(|(_, p)| p.selectable() && !p.borderless())
            .count();
        let omit_pane_title = self.pane_frame_style.draws_titles() && content_pane_count == 1;
        for (kind, pane) in self.panes.iter_mut() {
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

            if !self.panes_to_hide.contains(&pane.pid()) {
                let pane_is_stacked_under =
                    stacked_pane_ids_under_flexible_pane.contains(&pane.pid());
                let pane_is_stacked_over =
                    stacked_pane_ids_over_flexible_pane.contains(&pane.pid());
                let pane_is_on_top_of_stack =
                    stacked_pane_ids_on_top_of_stacks.contains(&pane.pid());
                let pane_is_on_bottom_of_stack =
                    stacked_pane_ids_on_bottom_of_stacks.contains(&pane.pid());
                let pane_is_no_ui_fullscreen = *self.fullscreen_covers_ui.borrow()
                    && self.fullscreen_is_active == Some(pane.pid());
                let should_draw_pane_frames =
                    self.pane_frame_style.draws_full_frames() && !pane_is_no_ui_fullscreen;
                let pane_is_stacked = pane.current_geom().is_stacked();
                let pane_is_one_liner_in_stack =
                    pane_is_stacked && pane.current_geom().rows.is_fixed();
                let pane_is_selectable = pane.selectable();
                let show_help_text = active_panes.iter().any(|(client_id, pane_id)| {
                    pane_id == &pane.pid()
                        && help_text_visible.get(client_id).copied().unwrap_or(false)
                }) && selectable_pane_count > 1
                    && self.fullscreen_is_active.is_none();
                let reserved_rows_for_pane = if self.fullscreen_is_active.is_some() {
                    0
                } else {
                    { self.reserved_top_rows.borrow().get(&pane.pid()).copied() }.unwrap_or(0)
                };
                let visible_member_frame_override = if reserved_rows_for_pane > 0 {
                    let mut geom = pane.render_geom();
                    geom.y += reserved_rows_for_pane;
                    geom.rows.decrease_inner(reserved_rows_for_pane);
                    Some(geom)
                } else {
                    None
                };
                let pane_has_guest_modal = pane.has_guest_modal_for_any_client();
                let omit_pane_title = omit_pane_title && pane.subtitle().is_none();
                let pane_has_subtitle = pane.subtitle_rows() > 0;
                let mut pane_contents_and_ui = PaneContentsAndUi::new(
                    pane,
                    output,
                    self.style,
                    &active_panes,
                    multiplayer,
                    None,
                    pane_is_stacked_under,
                    pane_is_stacked_over,
                    should_draw_pane_frames,
                    mouse_hover_pane_id,
                    current_pane_group.clone(),
                    show_help_text,
                    omit_pane_title && reserved_rows_for_pane == 0,
                    mouse_scroll_resize,
                    self.dimmed_clients.clone(),
                );
                pane_contents_and_ui.set_frame_geom_override(visible_member_frame_override);
                pane_contents_and_ui.set_blank_title(reserved_rows_for_pane > 0);
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
                    let err_context =
                        || format!("failed to render tiled panes for client {client_id}");
                    if let PaneId::Plugin(..) = kind
                        && !pane_is_one_liner_in_stack
                    {
                        pane_contents_and_ui
                            .render_pane_contents_for_client(*client_id)
                            .with_context(err_context)?;
                    }
                    let is_floating = false;
                    if pane_is_no_ui_fullscreen {
                    } else if self.pane_frame_style.draws_full_frames() {
                        pane_contents_and_ui
                            .render_pane_frame(
                                *client_id,
                                client_mode,
                                self.session_is_mirrored,
                                is_floating,
                                pane_is_selectable,
                                true,
                                true,
                            )
                            .with_context(err_context)?;
                    } else {
                        if (self.pane_frame_style.draws_titles() || pane_is_stacked)
                            && (reserved_rows_for_pane == 0 || pane_has_subtitle)
                        {
                            pane_contents_and_ui
                                .render_pane_frame(
                                    *client_id,
                                    client_mode,
                                    self.session_is_mirrored,
                                    is_floating,
                                    pane_is_selectable,
                                    true,
                                    true,
                                )
                                .with_context(err_context)?;
                        }
                        let boundaries = client_id_to_boundaries
                            .entry(*client_id)
                            .or_insert_with(|| Boundaries::new(boundary_viewport));
                        pane_contents_and_ui.render_pane_boundaries(
                            *client_id,
                            client_mode,
                            boundaries,
                            self.session_is_mirrored,
                            pane_is_on_top_of_stack,
                            pane_is_on_bottom_of_stack,
                        );
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
                if let PaneId::Terminal(..) = kind
                    && !pane_is_one_liner_in_stack
                {
                    if pane_has_guest_modal {
                        let (modal_clients, plain_clients): (Vec<ClientId>, Vec<ClientId>) =
                            connected_clients.iter().copied().partition(|client_id| {
                                pane_contents_and_ui.client_has_guest_modal(*client_id)
                            });
                        if !plain_clients.is_empty() {
                            pane_contents_and_ui
                                .render_pane_contents_to_multiple_clients(
                                    plain_clients.iter().copied(),
                                )
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
                            .render_pane_contents_to_multiple_clients(
                                connected_clients.iter().copied(),
                            )
                            .with_context(err_context)?;
                    }
                }
            }
        }
        // render boundaries if needed
        for (client_id, boundaries) in client_id_to_boundaries {
            let mut boundaries_to_render = boundaries
                .render(self.client_id_to_boundaries.get(&client_id))
                .with_context(err_context)?;
            self.client_id_to_boundaries.insert(client_id, boundaries);
            if output.pane_change_client == Some(client_id) && !boundaries_to_render.is_empty() {
                output.rendered_pane_boundaries = true;
            }
            if self.dimmed_clients.contains(&client_id) {
                crate::ui::pane_contents_and_ui::dim_character_chunks(&mut boundaries_to_render);
            }
            output
                .add_character_chunks_to_client(client_id, boundaries_to_render, None)
                .with_context(err_context)?;
        }
        if floating_panes_are_visible {
            // we do this here so that when they are toggled off, we will make sure to re-render the title
            self.window_title = None;
        }
        Ok(())
    }
    pub fn get_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.panes.iter()
    }
    pub fn position_and_size_of_stack(&mut self, pane_id: &PaneId) -> Option<PaneGeom> {
        StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
            .position_and_size_of_stack(pane_id)
    }
    pub fn set_geom_for_pane_with_run(
        &mut self,
        run: Option<Run>,
        geom: PaneGeom,
        should_be_borderless: Option<bool>,
    ) {
        match self
            .panes
            .iter_mut()
            .find(|(_, p)| p.invoked_with() == &run)
        {
            Some((_, pane)) => {
                pane.set_geom(geom);

                if let Some(should_be_borderless) = should_be_borderless {
                    pane.set_borderless(should_be_borderless);
                    if should_be_borderless {
                        pane.set_content_offset(Offset::default());
                    } else {
                        pane.set_content_offset(Offset::frame(1));
                    }
                } else if self.pane_frame_style.draws_full_frames() {
                    pane.set_content_offset(Offset::frame(1));
                }
            },
            None => {
                log::error!("Failed to find pane with run: {:?}", run);
            },
        }
    }
    pub fn set_geom_for_pane_with_id(&mut self, pane_id: &PaneId, geom: PaneGeom) {
        match self.panes.get_mut(pane_id) {
            Some(pane) => {
                pane.set_geom(geom);
            },
            None => {
                log::error!("Failed to find pane with id: {:?}", pane_id);
            },
        }
    }
    fn display_area_changed(&self, new_screen_size: Size) -> bool {
        let display_area = self.display_area.borrow();
        new_screen_size.rows != display_area.rows || new_screen_size.cols != display_area.cols
    }
    pub fn force_resize(&mut self) {
        let display_area = *self.display_area.borrow();
        self.resize(display_area);
    }

    fn tab_bar_resizes(&self, group_count: usize) -> impl Iterator<Item = (PaneId, PaneGeom)> + '_ {
        let rows = if group_count > 1 { 2 } else { 1 };
        self.panes.iter().filter_map(move |(id, pane)| {
            let mut geom = pane.position_and_size();
            let resize = pane.is_builtin_tab_bar()
                && pane.configured_rows().constraint == Constraint::Fixed(2)
                && !geom.is_stacked()
                && geom.rows.constraint != Constraint::Fixed(rows);
            geom.rows.constraint = Constraint::Fixed(rows);
            resize.then_some((*id, geom))
        })
    }

    pub fn tab_bar_needs_resize(&self, group_count: usize) -> bool {
        // A refused layout must not trigger another plugin resize on every render.
        if self
            .failed_tab_bar_resize
            .as_ref()
            .is_some_and(|(count, geoms)| {
                *count == group_count
                    && geoms.len() == self.panes.len()
                    && geoms.iter().all(|(id, geom)| {
                        self.panes
                            .get(id)
                            .is_some_and(|pane| pane.position_and_size() == *geom)
                    })
            })
        {
            return false;
        }
        self.tab_bar_resizes(group_count).next().is_some()
    }

    pub fn resize_tab_bar(&mut self, group_count: usize) -> Result<()> {
        self.failed_tab_bar_resize = None;
        let changes: Vec<_> = self.tab_bar_resizes(group_count).collect();
        if changes.is_empty() {
            return Ok(());
        }
        let previous: Vec<_> = self
            .panes
            .iter()
            .map(|(id, pane)| (*id, pane.position_and_size()))
            .collect();
        for (id, geom) in changes {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.set_geom(geom);
            }
        }
        let result = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        )
        .layout(SplitDirection::Vertical, self.display_area.borrow().rows);
        if result.is_err() {
            for (id, geom) in &previous {
                if let Some(pane) = self.panes.get_mut(id) {
                    pane.set_geom(*geom);
                }
            }
            self.failed_tab_bar_resize = Some((group_count, previous));
        }
        result.context("resize tab bar for tab groups")
    }

    pub fn resize(&mut self, new_screen_size: Size) {
        {
            if self.display_area_changed(new_screen_size) {
                self.clear_tombstones();
            }
            let mut display_area = self.display_area.borrow_mut();
            let mut viewport = self.viewport.borrow_mut();
            let Size { rows, cols } = new_screen_size;
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                Viewport {
                    x: 0,
                    y: 0,
                    cols: display_area.cols,
                    rows: display_area.rows,
                },
                *viewport,
            );

            // `PaneSizeUnchanged` is a valid layout that rounded to the same geometry, so it
            // counts as laid out. Anything else - `PaneLayoutRejected` when a stack cannot fit,
            // or the solver running out of room - left every pane where it was, and the other
            // axis is tried first in case that frees the space.
            let laid_out = |result: Result<()>| -> bool {
                match result {
                    Ok(_) => true,
                    Err(e) => matches!(
                        e.downcast_ref::<YinzError>(),
                        Some(YinzError::PaneSizeUnchanged)
                    ),
                }
            };
            let resize_horizontally = |pane_grid: &mut TiledPaneGrid| -> bool {
                laid_out(pane_grid.layout(SplitDirection::Horizontal, cols))
            };
            let resize_vertically = |pane_grid: &mut TiledPaneGrid| -> bool {
                laid_out(pane_grid.layout(SplitDirection::Vertical, rows))
            };

            if resize_horizontally(&mut pane_grid) {
                resize_vertically(&mut pane_grid);
            } else {
                log::warn!("Failed to resize horizontally, attempting to first resize vertically");
                if resize_vertically(&mut pane_grid) {
                    resize_horizontally(&mut pane_grid);
                } else {
                    log::error!("Failed to resize vertically, will not attempt again.");
                }
            }

            // The screen is the new size whether or not the panes could follow it: the viewport
            // and display area describe the screen, not the solver's outcome. Updating them only
            // on success used to leave the viewport one resize behind the display area after a
            // refused layout, so the next successful resize moved it by the wrong delta.
            //
            // The viewport is the screen minus what the ui panes reserve on each edge, so carry
            // those insets across rather than the raw delta - the same arithmetic, in the form
            // where clamping into the new screen is expressible. A screen too small for the
            // chrome leaves an empty viewport instead of one that wrapped around `usize`, which
            // `set_pane_frames` below reads as "every pane is short of the bottom edge" and,
            // wherever the chrome puts `viewport.y` past zero, overflows on. A bottom-only
            // chrome leaves `viewport.y` at zero, where `0 + usize::MAX` does not overflow, so
            // the wrap corrupted silently rather than panicking there.
            (viewport.x, viewport.cols) =
                resized_viewport_axis(viewport.x, viewport.cols, display_area.cols, cols);
            (viewport.y, viewport.rows) =
                resized_viewport_axis(viewport.y, viewport.rows, display_area.rows, rows);
            display_area.rows = rows;
            display_area.cols = cols;
        }
        self.set_pane_frames(self.pane_frame_style);
    }

    pub fn resize_active_pane(
        &mut self,
        client_id: ClientId,
        strategy: &ResizeStrategy,
    ) -> Result<()> {
        if *self.stacked_resize.borrow() && strategy.direction.is_none() {
            if let Some(active_pane_id) = self.get_active_pane_id(client_id) {
                self.stacked_resize_pane_with_id(active_pane_id, strategy, None)?;
                self.reapply_pane_frames();
            }
        } else {
            if let Some(active_pane_id) = self.get_active_pane_id(client_id) {
                self.resize_pane_with_id(*strategy, active_pane_id, None)?;
            }
        }

        Ok(())
    }
    fn resize_or_stack_pane_up(&mut self, pane_id: PaneId, resize_percent: (f64, f64)) -> bool {
        // true - successfully resized
        let mut strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Up));
        strategy.invert_on_boundaries = false;
        let successfully_resized_up = self
            .resize_pane_with_id(strategy, pane_id, Some(resize_percent))
            .unwrap_or(false);
        if successfully_resized_up {
            return true;
        } else {
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            if let Some(pane_ids_to_resize) = pane_grid.stack_pane_up(&pane_id) {
                for pane_id in pane_ids_to_resize {
                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                            .non_fatal();
                    }
                }
                return true;
            }
        }
        false
    }
    fn resize_or_stack_pane_down(&mut self, pane_id: PaneId, resize_percent: (f64, f64)) -> bool {
        // true - successfully resized
        let mut strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Down));
        strategy.invert_on_boundaries = false;
        let successfully_resized_up = self
            .resize_pane_with_id(strategy, pane_id, Some(resize_percent))
            .unwrap_or(false);
        if successfully_resized_up {
            return true;
        } else {
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            if let Some(pane_ids_to_resize) = pane_grid.stack_pane_down(&pane_id) {
                for pane_id in pane_ids_to_resize {
                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                            .non_fatal();
                    }
                }
                return true;
            }
        }
        false
    }
    fn resize_or_stack_pane_left(&mut self, pane_id: PaneId, resize_percent: (f64, f64)) -> bool {
        // true - successfully resized
        let mut strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Left));
        strategy.invert_on_boundaries = false;
        let successfully_resized_up = self
            .resize_pane_with_id(strategy, pane_id, Some(resize_percent))
            .unwrap_or(false);
        if successfully_resized_up {
            return true;
        } else {
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            if let Some(pane_ids_to_resize) = pane_grid.stack_pane_left(&pane_id) {
                for pane_id in pane_ids_to_resize {
                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                            .non_fatal();
                    }
                }
                return true;
            }
        }
        false
    }
    fn resize_or_stack_pane_right(&mut self, pane_id: PaneId, resize_percent: (f64, f64)) -> bool {
        // true - successfully resized
        let mut strategy = ResizeStrategy::new(Resize::Increase, Some(Direction::Right));
        strategy.invert_on_boundaries = false;
        let successfully_resized_up = self
            .resize_pane_with_id(strategy, pane_id, Some(resize_percent))
            .unwrap_or(false);
        if successfully_resized_up {
            return true;
        } else {
            let mut pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            if let Some(pane_ids_to_resize) = pane_grid.stack_pane_right(&pane_id) {
                for pane_id in pane_ids_to_resize {
                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                            .non_fatal();
                    }
                }
                return true;
            }
        }
        false
    }
    fn update_tombstones_before_increase(
        &mut self,
        focused_pane_id: PaneId,
        pane_state: HashMap<PaneId, PaneGeom>,
    ) {
        match self.tombstones_before_increase.as_mut() {
            Some((focused_pane_id_in_tombstone, pane_geoms)) => {
                let last_state = pane_geoms.last();
                let last_state_has_same_pane_count = last_state
                    .map(|last_state| last_state.len() == pane_state.len())
                    .unwrap_or(false);
                let last_state_equals_current_state = last_state
                    .map(|last_state| last_state == &pane_state)
                    .unwrap_or(false);
                if last_state_equals_current_state {
                    return;
                }
                if *focused_pane_id_in_tombstone == focused_pane_id
                    && last_state_has_same_pane_count
                {
                    pane_geoms.push(pane_state);
                } else {
                    self.clear_tombstones();
                    self.tombstones_before_increase = Some((focused_pane_id, vec![pane_state]));
                }
            },
            None => {
                self.clear_tombstones();
                self.tombstones_before_increase = Some((focused_pane_id, vec![pane_state]));
            },
        }
    }
    fn update_tombstones_before_decrease(
        &mut self,
        focused_pane_id: PaneId,
        pane_state: HashMap<PaneId, PaneGeom>,
    ) {
        match self.tombstones_before_decrease.as_mut() {
            Some((focused_pane_id_in_tombstone, pane_geoms)) => {
                let last_state = pane_geoms.last();
                let last_state_has_same_pane_count = last_state
                    .map(|last_state| last_state.len() == pane_state.len())
                    .unwrap_or(false);
                let last_state_equals_current_state = last_state
                    .map(|last_state| last_state == &pane_state)
                    .unwrap_or(false);
                if last_state_equals_current_state {
                    return;
                }
                if *focused_pane_id_in_tombstone == focused_pane_id
                    && last_state_has_same_pane_count
                {
                    pane_geoms.push(pane_state);
                } else {
                    self.clear_tombstones();
                    self.tombstones_before_decrease = Some((focused_pane_id, vec![pane_state]));
                }
            },
            None => {
                self.clear_tombstones();
                self.tombstones_before_decrease = Some((focused_pane_id, vec![pane_state]));
            },
        }
    }
    fn clear_tombstones(&mut self) {
        self.tombstones_before_increase = None;
        self.tombstones_before_decrease = None;
    }
    pub fn stacked_resize_pane_with_id(
        &mut self,
        pane_id: PaneId,
        strategy: &ResizeStrategy,
        // override as rarely as possible to maintain ux consistency
        resize_percent_override: Option<(f64, f64)>,
    ) -> Result<bool> {
        let resize_percent = resize_percent_override.unwrap_or((30.0, 30.0));
        match strategy.resize {
            Resize::Increase => {
                match self.tombstones_before_decrease.as_mut() {
                    Some((tombstone_pane_id, tombstone_pane_state))
                        if *tombstone_pane_id == pane_id =>
                    {
                        if let Some(last_state) = tombstone_pane_state.pop() {
                            if last_state.len() == self.panes.len() {
                                for (pane_id, pane_geom) in last_state {
                                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                                        pane.set_geom(pane_geom)
                                    }
                                }
                                self.reapply_pane_frames();
                                return Ok(true);
                            } else {
                                self.tombstones_before_decrease = None;
                            }
                        }
                    },
                    _ => {},
                }

                let mut current_pane_state = HashMap::new();
                for (pane_id, pane) in &self.panes {
                    current_pane_state.insert(*pane_id, pane.current_geom());
                }

                let (
                    direct_neighboring_pane_ids_above,
                    direct_neighboring_pane_ids_below,
                    direct_neighboring_pane_ids_to_the_left,
                    direct_neighboring_pane_ids_to_the_right,
                ) = {
                    let pane_grid = TiledPaneGrid::new(
                        &mut self.panes,
                        &self.panes_to_hide,
                        display_area_viewport(&self.display_area),
                        *self.viewport.borrow(),
                    );
                    (
                        pane_grid.direct_neighboring_pane_ids_above(&pane_id),
                        pane_grid.direct_neighboring_pane_ids_below(&pane_id),
                        pane_grid.direct_neighboring_pane_ids_to_the_left(&pane_id),
                        pane_grid.direct_neighboring_pane_ids_to_the_right(&pane_id),
                    )
                };
                if !direct_neighboring_pane_ids_above.is_empty()
                    && self.resize_or_stack_pane_up(pane_id, resize_percent)
                {
                    self.update_tombstones_before_increase(pane_id, current_pane_state);
                    return Ok(true);
                }
                if !direct_neighboring_pane_ids_below.is_empty()
                    && self.resize_or_stack_pane_down(pane_id, resize_percent)
                {
                    self.update_tombstones_before_increase(pane_id, current_pane_state);
                    return Ok(true);
                }
                if !direct_neighboring_pane_ids_to_the_left.is_empty()
                    && self.resize_or_stack_pane_left(pane_id, resize_percent)
                {
                    self.update_tombstones_before_increase(pane_id, current_pane_state);
                    return Ok(true);
                }
                if !direct_neighboring_pane_ids_to_the_right.is_empty()
                    && self.resize_or_stack_pane_right(pane_id, resize_percent)
                {
                    self.update_tombstones_before_increase(pane_id, current_pane_state);
                    return Ok(true);
                }
                // normal resize if we can't do anything...
                match self.resize_pane_with_id(*strategy, pane_id, None) {
                    Ok(size_changed) => {
                        if size_changed {
                            self.update_tombstones_before_increase(pane_id, current_pane_state);
                        } else {
                            if self.fullscreen_is_active.is_none() {
                                self.toggle_pane_fullscreen(pane_id);
                                return Ok(self.fullscreen_is_active.is_some());
                            } else {
                                return Ok(false);
                            }
                        }
                        Ok(size_changed)
                    },
                    Err(e) => Err(e),
                }
            },
            Resize::Decrease => {
                if self.fullscreen_is_active.is_some() {
                    self.unset_fullscreen();
                    return Ok(true);
                }

                match self.tombstones_before_increase.as_mut() {
                    Some((tombstone_pane_id, tombstone_pane_state))
                        if *tombstone_pane_id == pane_id =>
                    {
                        if let Some(last_state) = tombstone_pane_state.pop() {
                            if last_state.len() == self.panes.len() {
                                for (pane_id, pane_geom) in last_state {
                                    if let Some(pane) = self.panes.get_mut(&pane_id) {
                                        pane.set_geom(pane_geom)
                                    }
                                }
                                self.reapply_pane_frames();
                                return Ok(true);
                            } else {
                                self.tombstones_before_increase = None;
                            }
                        }
                    },
                    _ => {},
                }

                let mut current_pane_state = HashMap::new();
                for (pane_id, pane) in &self.panes {
                    current_pane_state.insert(*pane_id, pane.current_geom());
                }

                let mut pane_grid = TiledPaneGrid::new(
                    &mut self.panes,
                    &self.panes_to_hide,
                    display_area_viewport(&self.display_area),
                    *self.viewport.borrow(),
                );
                if let Some(pane_ids_to_resize) = pane_grid.unstack_pane_up(&pane_id) {
                    for pane_id in pane_ids_to_resize {
                        if let Some(pane) = self.panes.get_mut(&pane_id) {
                            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                                .non_fatal();
                        }
                    }
                    self.reapply_pane_frames();
                    self.update_tombstones_before_decrease(pane_id, current_pane_state);
                    Ok(true)
                } else {
                    // normal resize if we were not inside a stack
                    // first try with our custom resize_percent
                    match self.resize_pane_with_id(*strategy, pane_id, Some(resize_percent)) {
                        Ok(pane_size_changed) => {
                            if pane_size_changed {
                                self.update_tombstones_before_decrease(pane_id, current_pane_state);
                                self.reapply_pane_frames();
                                Ok(pane_size_changed)
                            } else {
                                // if it doesn't work, try with the default resize percent
                                match self.resize_pane_with_id(*strategy, pane_id, None) {
                                    Ok(pane_size_changed) => {
                                        if pane_size_changed {
                                            self.update_tombstones_before_decrease(
                                                pane_id,
                                                current_pane_state,
                                            );
                                            self.reapply_pane_frames();
                                        }
                                        Ok(pane_size_changed)
                                    },
                                    Err(e) => Err(e),
                                }
                            }
                        },
                        Err(e) => Err(e),
                    }
                }
            },
        }
    }
    pub fn resize_pane_with_id(
        &mut self,
        strategy: ResizeStrategy,
        pane_id: PaneId,
        resize_percent: Option<(f64, f64)>,
    ) -> Result<bool> {
        let err_context = || format!("failed to resize pane with id: {:?}", pane_id);
        let sizes_before = self.content_sizes();

        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );

        let mut pane_size_changed = false;
        match pane_grid
            .change_pane_size(
                &pane_id,
                &strategy,
                resize_percent.unwrap_or((RESIZE_PERCENT, RESIZE_PERCENT)),
            )
            .with_context(err_context)
        {
            Ok(changed) => {
                pane_size_changed = changed;
            },
            Err(err) => match err.downcast_ref::<YinzError>() {
                Some(YinzError::PaneSizeUnchanged) => {
                    // try once more with double the resize percent, but let's keep it at that
                    match pane_grid
                        .change_pane_size(
                            &pane_id,
                            &strategy,
                            (RESIZE_PERCENT * 2.0, RESIZE_PERCENT * 2.0),
                        )
                        .with_context(err_context)
                    {
                        Ok(changed) => pane_size_changed = changed,
                        Err(err) => match err.downcast_ref::<YinzError>() {
                            Some(YinzError::PaneSizeUnchanged) => Err::<(), _>(err).non_fatal(),
                            _ => {
                                return Err(err);
                            },
                        },
                    }
                },
                _ => {
                    return Err(err);
                },
            },
        }

        self.resize_ptys_of_panes_changed_since(&sizes_before);
        self.reset_boundaries();
        Ok(pane_size_changed)
    }

    pub fn resize_pane_with_strategies(
        &mut self,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (f64, f64),
    ) -> Result<bool> {
        let err_context = || format!("failed to resize pane {:?} with strategies", pane_id);
        let sizes_before = self.content_sizes();

        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );

        let result = strategies
            .iter()
            .filter(|strategy| match strategy.direction {
                Some(direction) if direction.is_horizontal() => change_by.0 != 0.0,
                Some(_) => change_by.1 != 0.0,
                None => change_by != (0.0, 0.0),
            })
            .try_fold(false, |changed, strategy| {
                match pane_grid.change_pane_size(&pane_id, strategy, change_by) {
                    Ok(resized) => Ok(changed || resized),
                    Err(err) => match err.downcast_ref::<YinzError>() {
                        Some(
                            YinzError::CantResizeFixedPanes { .. } | YinzError::PaneSizeUnchanged,
                        ) => Ok(changed),
                        _ => Err(err).with_context(err_context),
                    },
                }
            });
        drop(pane_grid);
        // A corner drag may have changed one axis before the other failed.
        self.resize_ptys_of_panes_changed_since(&sizes_before);
        self.reset_boundaries();
        result
    }

    /// Whether the client's active pane is the last one in focus-cycling order (ie. the next
    /// `focus_next_pane` would wrap around to the first pane).
    pub fn active_pane_is_last_in_cycle(&mut self, client_id: ClientId) -> bool {
        let Some(active_pane_id) = self.get_active_pane_id(client_id) else {
            return false;
        };
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        pane_grid.is_last_selectable_pane_id(&active_pane_id)
    }

    pub fn active_pane_is_first_in_cycle(&mut self, client_id: ClientId) -> bool {
        let Some(active_pane_id) = self.get_active_pane_id(client_id) else {
            return false;
        };
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        pane_grid.is_first_selectable_pane_id(&active_pane_id)
    }

    pub fn focus_next_pane(&mut self, client_id: ClientId) {
        let Some(active_pane_id) = self.get_active_pane_id(client_id) else {
            return;
        };
        let next_active_pane_id = {
            let pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            pane_grid.next_selectable_pane_id(&active_pane_id)
        };
        let Some(next_active_pane_id) = next_active_pane_id else {
            return;
        };
        if self
            .panes
            .get(&next_active_pane_id)
            .map(|p| p.current_geom().is_stacked())
            .unwrap_or(false)
        {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&next_active_pane_id);
            self.reapply_pane_frames();
        }

        if active_pane_id != next_active_pane_id {
            self.active_panes.set_last_pane(client_id, active_pane_id);
        }
        self.active_panes
            .insert(client_id, next_active_pane_id, &self.panes);
        self.set_pane_active_at(next_active_pane_id);
        self.reset_boundaries();
    }
    pub fn focus_previous_pane(&mut self, client_id: ClientId) {
        let Some(active_pane_id) = self.get_active_pane_id(client_id) else {
            return;
        };
        let next_active_pane_id = {
            let pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            pane_grid.previous_selectable_pane_id(&active_pane_id)
        };
        let Some(next_active_pane_id) = next_active_pane_id else {
            return;
        };

        if self
            .panes
            .get(&next_active_pane_id)
            .map(|p| p.current_geom().is_stacked())
            .unwrap_or(false)
        {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&next_active_pane_id);
            self.reapply_pane_frames();
        }
        if active_pane_id != next_active_pane_id {
            self.active_panes.set_last_pane(client_id, active_pane_id);
        }
        self.active_panes
            .insert(client_id, next_active_pane_id, &self.panes);
        self.set_pane_active_at(next_active_pane_id);
        self.reset_boundaries();
    }
    pub fn focus_last_pane(&mut self, client_id: ClientId) {
        let Some(last_pane_id) = self.get_last_pane_id(client_id) else {
            self.focus_previous_pane(client_id);
            return;
        };

        // active_panes is cleared when the last pane is removed, but last_panes is not, so
        // get_last_pane_id can hand us an id with nothing focused to move away from
        let Some(&previously_active_pane_id) = self.active_panes.get(&client_id) else {
            return;
        };
        if !self.panes.contains_key(&last_pane_id) || last_pane_id == previously_active_pane_id {
            self.focus_previous_pane(client_id);
            return;
        }

        if let Some(previously_active_pane) = self.panes.get_mut(&previously_active_pane_id) {
            previously_active_pane.set_should_render(true);
            previously_active_pane.render_full_viewport();
        }
        let Some(next_active_pane) = self.panes.get_mut(&last_pane_id) else {
            return;
        };
        let stacked = next_active_pane.current_geom().stacked;
        next_active_pane.set_should_render(true);
        // we render the full viewport to remove any ui elements that might have been
        // there before (eg. another user's cursor)
        next_active_pane.render_full_viewport();

        self.focus_pane(last_pane_id, client_id);
        self.set_pane_active_at(last_pane_id);
        if let Some(stack_id) = stacked {
            // we do this because a stack pane focus change also changes its
            // geometry and we need to let the pty know about this (like in a
            // normal size change)
            self.focus_pane_for_all_clients_in_stack(last_pane_id, stack_id);
            self.reapply_pane_frames();
        }
    }
    fn set_pane_active_at(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_mut(pane_id) {
            pane.set_active_at(Instant::now());
        }
    }
    pub fn cursor_height_width_ratio(&self) -> Option<usize> {
        let character_cell_size = self.character_cell_size.borrow();
        character_cell_size.map(|size_in_pixels| {
            (size_in_pixels.height as f64 / size_in_pixels.width as f64).round() as usize
        })
    }
    pub fn focus_pane_on_edge(&mut self, direction: Direction, client_id: ClientId) {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let Some(next_index) = pane_grid.pane_id_on_edge(direction) else {
            return;
        };
        // render previously active pane so that its frame does not remain actively
        // colored
        let Some(&previously_active_pane_id) = self.active_panes.get(&client_id) else {
            return;
        };
        let Some(previously_active_pane) = self.panes.get_mut(&previously_active_pane_id) else {
            return;
        };

        previously_active_pane.set_should_render(true);
        // we render the full viewport to remove any ui elements that might have been
        // there before (eg. another user's cursor)
        previously_active_pane.render_full_viewport();

        let Some(next_active_pane) = self.panes.get_mut(&next_index) else {
            return;
        };
        next_active_pane.set_should_render(true);
        // we render the full viewport to remove any ui elements that might have been
        // there before (eg. another user's cursor)
        next_active_pane.render_full_viewport();

        self.focus_pane(next_index, client_id);
        self.set_pane_active_at(next_index);
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
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let next_index = match direction {
            Direction::Left => pane_grid.next_selectable_pane_id_to_the_left(&pane_id),
            Direction::Down => pane_grid.next_selectable_pane_id_below(&pane_id, false, true),
            Direction::Up => pane_grid.next_selectable_pane_id_above(&pane_id, false, true),
            Direction::Right => pane_grid.next_selectable_pane_id_to_the_right(&pane_id),
        }?;
        if let Some(previously_active_pane) = self
            .active_panes
            .get(&client_id)
            .copied()
            .and_then(|active_pane_id| self.panes.get_mut(&active_pane_id))
        {
            previously_active_pane.set_should_render(true);
            previously_active_pane.render_full_viewport();
        }
        if let Some(next_active_pane) = self.panes.get_mut(&next_index) {
            next_active_pane.set_should_render(true);
            next_active_pane.render_full_viewport();
        }
        self.focus_pane(next_index, client_id);
        self.set_pane_active_at(next_index);
        Some(next_index)
    }
    pub fn move_focus_left(&mut self, client_id: ClientId) -> bool {
        match self.get_active_pane_id(client_id) {
            Some(active_pane_id) => {
                let pane_grid = TiledPaneGrid::new(
                    &mut self.panes,
                    &self.panes_to_hide,
                    display_area_viewport(&self.display_area),
                    *self.viewport.borrow(),
                );
                let next_index = pane_grid.next_selectable_pane_id_to_the_left(&active_pane_id);
                match next_index {
                    Some(p) => {
                        if let Some(previously_active_pane) = self.panes.get_mut(&active_pane_id) {
                            previously_active_pane.set_should_render(true);
                            previously_active_pane.render_full_viewport();
                        }

                        if let Some(next_active_pane) = self.panes.get_mut(&p) {
                            next_active_pane.set_should_render(true);
                            next_active_pane.render_full_viewport();
                        }

                        self.focus_pane(p, client_id);
                        self.set_pane_active_at(p);

                        true
                    },
                    None => false,
                }
            },
            None => false,
        }
    }
    pub fn move_focus_down(&mut self, client_id: ClientId) -> bool {
        match self.get_active_pane_id(client_id) {
            Some(active_pane_id) => {
                let mut pane_grid = TiledPaneGrid::new(
                    &mut self.panes,
                    &self.panes_to_hide,
                    display_area_viewport(&self.display_area),
                    *self.viewport.borrow(),
                );
                let next_index = pane_grid
                    .next_selectable_pane_id_below(&active_pane_id, false, true)
                    .or_else(|| pane_grid.progress_stack_down_if_in_stack(&active_pane_id, true));
                match next_index {
                    Some(p) => {
                        if let Some(previously_active_pane) = self.panes.get_mut(&active_pane_id) {
                            previously_active_pane.set_should_render(true);
                            previously_active_pane.render_full_viewport();
                        }

                        let mut next_active_pane_stack_id = None;
                        if let Some(next_active_pane) = self.panes.get_mut(&p) {
                            next_active_pane_stack_id = next_active_pane.current_geom().stacked;
                            next_active_pane.set_should_render(true);
                            next_active_pane.render_full_viewport();
                        }

                        self.focus_pane(p, client_id);
                        self.set_pane_active_at(p);
                        if let Some(stack_id) = next_active_pane_stack_id {
                            // we do this because a stack pane focus change also changes its
                            // geometry and we need to let the pty know about this (like in a
                            // normal size change)
                            self.focus_pane_for_all_clients_in_stack(p, stack_id);
                            self.reapply_pane_frames();
                        }

                        true
                    },
                    None => false,
                }
            },
            None => false,
        }
    }
    pub fn move_focus_up(&mut self, client_id: ClientId) -> bool {
        match self.get_active_pane_id(client_id) {
            Some(active_pane_id) => {
                let mut pane_grid = TiledPaneGrid::new(
                    &mut self.panes,
                    &self.panes_to_hide,
                    display_area_viewport(&self.display_area),
                    *self.viewport.borrow(),
                );
                let next_index = pane_grid
                    .next_selectable_pane_id_above(&active_pane_id, false, true)
                    .or_else(|| pane_grid.progress_stack_up_if_in_stack(&active_pane_id, true));
                match next_index {
                    Some(p) => {
                        if let Some(previously_active_pane) = self.panes.get_mut(&active_pane_id) {
                            previously_active_pane.set_should_render(true);
                            previously_active_pane.render_full_viewport();
                        }

                        let mut next_active_pane_stack_id = None;
                        if let Some(next_active_pane) = self.panes.get_mut(&p) {
                            next_active_pane_stack_id = next_active_pane.current_geom().stacked;
                            next_active_pane.set_should_render(true);
                            next_active_pane.render_full_viewport();
                        }

                        self.focus_pane(p, client_id);
                        self.set_pane_active_at(p);
                        if let Some(stack_id) = next_active_pane_stack_id {
                            // we do this because a stack pane focus change also changes its
                            // geometry and we need to let the pty know about this (like in a
                            // normal size change)
                            self.focus_pane_for_all_clients_in_stack(p, stack_id);
                            self.reapply_pane_frames();
                        }

                        true
                    },
                    None => false,
                }
            },
            None => false,
        }
    }
    pub fn move_focus_right(&mut self, client_id: ClientId) -> bool {
        match self.get_active_pane_id(client_id) {
            Some(active_pane_id) => {
                let pane_grid = TiledPaneGrid::new(
                    &mut self.panes,
                    &self.panes_to_hide,
                    display_area_viewport(&self.display_area),
                    *self.viewport.borrow(),
                );
                let next_index = pane_grid.next_selectable_pane_id_to_the_right(&active_pane_id);
                match next_index {
                    Some(p) => {
                        if let Some(previously_active_pane) = self.panes.get_mut(&active_pane_id) {
                            previously_active_pane.set_should_render(true);
                            previously_active_pane.render_full_viewport();
                        }

                        if let Some(next_active_pane) = self.panes.get_mut(&p) {
                            next_active_pane.set_should_render(true);
                            next_active_pane.render_full_viewport();
                        }

                        self.focus_pane(p, client_id);
                        self.set_pane_active_at(p);

                        true
                    },
                    None => false,
                }
            },
            None => false,
        }
    }
    /// Exchange the on-screen positions of two panes: each takes the other's geometry and
    /// geometry override, is told its new size, and is marked for render. Focus and frames are
    /// the caller's business. Nothing changes if either id is unknown.
    ///
    /// A failed resize is logged, not returned: by the time it is sent the geometry has already
    /// moved, and aborting would leave one pane moved and the other not. The next
    /// `set_pane_frames` resends every pane's size anyway.
    fn swap_pane_geoms(&mut self, a: PaneId, b: PaneId) -> Result<()> {
        let err_context = || format!("failed to swap panes {a:?} and {b:?}");
        let (a_geom, a_geom_override) = {
            let pane = self.panes.get(&a).with_context(err_context)?;
            (pane.position_and_size(), pane.geom_override())
        };

        let pane_b = self.panes.get_mut(&b).with_context(err_context)?;
        let (b_geom, b_geom_override) = (pane_b.position_and_size(), pane_b.geom_override());
        pane_b.set_geom(a_geom);
        if let Some(geom) = a_geom_override {
            pane_b.set_geom_override(geom);
        }
        resize_pty!(pane_b, self.os_api, self.senders, self.character_cell_size).non_fatal();
        pane_b.set_should_render(true);

        let pane_a = self.panes.get_mut(&a).with_context(err_context)?;
        pane_a.set_geom(b_geom);
        if let Some(geom) = b_geom_override {
            pane_a.set_geom_override(geom);
        }
        resize_pty!(pane_a, self.os_api, self.senders, self.character_cell_size).non_fatal();
        pane_a.set_should_render(true);
        Ok(())
    }
    pub fn switch_active_pane_with(&mut self, pane_id: PaneId) {
        if let Some(active_pane_id) = self.first_active_pane_id() {
            if let PaneId::Plugin(_) = active_pane_id {
                // we do not implicitly change the location of plugin panes
                return;
            }
            self.swap_pane_geoms(active_pane_id, pane_id).non_fatal();
            self.focus_pane_for_all_clients(active_pane_id);
            self.set_pane_frames(self.pane_frame_style);
        }
    }
    pub fn move_pane(&mut self, search_backwards: bool, pane_id: PaneId) {
        let new_position_id = {
            let pane_grid = TiledPaneGrid::new(
                &mut self.panes,
                &self.panes_to_hide,
                display_area_viewport(&self.display_area),
                *self.viewport.borrow(),
            );
            if search_backwards {
                pane_grid.previous_selectable_pane_id(&pane_id)
            } else {
                pane_grid.next_selectable_pane_id(&pane_id)
            }
        };
        let Some(new_position_id) = new_position_id else {
            return;
        };
        if self
            .panes
            .get(&new_position_id)
            .map(|p| p.current_geom().is_stacked())
            .unwrap_or(false)
        {
            let _ = StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
                .expand_pane(&new_position_id);
            self.reapply_pane_frames();
        }

        self.swap_pane_geoms(pane_id, new_position_id).non_fatal();
        self.reapply_pane_focus();
        self.set_pane_frames(self.pane_frame_style);
    }
    pub fn move_pane_down(&mut self, pane_id: PaneId) {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let next_index = pane_grid
            .next_selectable_pane_id_below(&pane_id, false, false)
            .or_else(|| pane_grid.progress_stack_down_if_in_stack(&pane_id, false));
        if let Some(p) = next_index {
            self.swap_pane_geoms(pane_id, p).non_fatal();
            self.reapply_pane_focus();
            self.set_pane_frames(self.pane_frame_style);
        }
    }
    pub fn move_pane_left(&mut self, pane_id: PaneId) {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let next_index = pane_grid.next_selectable_pane_id_to_the_left(&pane_id);
        if let Some(p) = next_index {
            self.swap_pane_geoms(pane_id, p).non_fatal();
            self.reapply_pane_focus();
            self.set_pane_frames(self.pane_frame_style);
        }
    }
    pub fn move_pane_right(&mut self, pane_id: PaneId) {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let next_index = pane_grid.next_selectable_pane_id_to_the_right(&pane_id);
        if let Some(p) = next_index {
            self.swap_pane_geoms(pane_id, p).non_fatal();
            self.reapply_pane_focus();
            self.set_pane_frames(self.pane_frame_style);
        }
    }
    pub fn move_pane_up(&mut self, pane_id: PaneId) {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let next_index = pane_grid
            .next_selectable_pane_id_above(&pane_id, false, false)
            .or_else(|| pane_grid.progress_stack_up_if_in_stack(&pane_id, false));
        if let Some(p) = next_index {
            self.swap_pane_geoms(pane_id, p).non_fatal();
            self.reapply_pane_focus();
            self.set_pane_frames(self.pane_frame_style);
        }
    }
    pub fn move_clients_out_of_pane(&mut self, pane_id: PaneId) {
        let active_panes: Vec<(ClientId, PaneId)> = self
            .active_panes
            .iter()
            .map(|(cid, pid)| (*cid, *pid))
            .collect();
        let clients_need_to_be_moved = active_panes.iter().any(|(_c_id, p_id)| *p_id == pane_id);
        if !clients_need_to_be_moved {
            return;
        }

        // find the most recently active pane
        let mut next_active_pane_candidates: Vec<(&PaneId, &Box<dyn Pane>)> = self
            .panes
            .iter()
            .filter(|(p_id, p)| !self.panes_to_hide.contains(p_id) && p.selectable())
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

        match next_active_pane_id {
            Some(next_active_pane_id) => {
                if self
                    .panes
                    .get(&next_active_pane_id)
                    .map(|p| p.current_geom().is_stacked())
                    .unwrap_or(false)
                {
                    self.expand_pane_in_stack(next_active_pane_id);
                }
                for (client_id, active_pane_id) in active_panes {
                    if active_pane_id == pane_id {
                        self.active_panes
                            .insert(client_id, next_active_pane_id, &self.panes);
                        if let Some(last_active_pane_id) = last_active_pane_id {
                            self.active_panes
                                .set_last_pane(client_id, last_active_pane_id);
                        }
                    }
                }
            },
            None => self.active_panes.clear(&self.panes),
        }
    }
    pub fn extract_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        self.reset_boundaries();
        self.panes.remove(&pane_id)
    }
    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        let mut pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        if pane_grid.fill_space_over_pane(pane_id) {
            // successfully filled space over pane
            let closed_pane = self.panes.remove(&pane_id);
            self.move_clients_out_of_pane(pane_id);
            self.set_pane_frames(self.pane_frame_style);
            closed_pane
        } else {
            let closed_pane = self.panes.remove(&pane_id);
            if closed_pane
                .as_ref()
                .map(|p| p.selectable())
                .unwrap_or(false)
            {
                // this is a bit of a roundabout way to say: this is the last pane and so the tab
                // should be destroyed
                self.active_panes.clear(&self.panes);
            }
            closed_pane
        }
    }
    pub fn hold_pane(
        &mut self,
        pane_id: PaneId,
        exit_status: Option<i32>,
        is_first_run: bool,
        run_command: RunCommand,
    ) {
        if let Some(p) = self.panes.get_mut(&pane_id) {
            p.hold(exit_status, is_first_run, run_command)
        }
    }
    pub fn panes_to_hide_contains(&self, pane_id: PaneId) -> bool {
        self.panes_to_hide.contains(&pane_id)
    }
    pub fn fullscreen_is_active(&self) -> bool {
        self.fullscreen_is_active.is_some()
    }
    pub fn fullscreen_pane_id(&self) -> Option<PaneId> {
        self.fullscreen_is_active
    }
    pub fn fullscreen_covers_ui(&self) -> bool {
        *self.fullscreen_covers_ui.borrow()
    }
    pub fn unset_fullscreen(&mut self) {
        if let Some(fullscreen_pane_id) = self.fullscreen_is_active {
            let panes_to_hide: Vec<_> = self.panes_to_hide.iter().copied().collect();
            for pane_id in panes_to_hide {
                if let Some(pane) = self.get_pane_mut(pane_id) {
                    pane.set_should_render(true);
                    pane.set_should_render_boundaries(true);
                }
            }
            let viewport = *self.viewport.borrow();
            for pane in self.panes.values_mut() {
                if !is_inside_viewport(&viewport, pane) {
                    pane.reset_size_and_position_override();
                }
            }
            self.panes_to_hide.clear();
            if let Some(fullscreen_pane) = self.get_pane_mut(fullscreen_pane_id) {
                fullscreen_pane.reset_size_and_position_override();
            }
            *self.fullscreen_covers_ui.borrow_mut() = false;
            self.rerun_layout_solver_and_rerender();
            self.fullscreen_is_active = None;
        }
    }
    pub fn toggle_active_pane_fullscreen(&mut self, client_id: ClientId) {
        if let Some(active_pane_id) = self.get_active_pane_id(client_id) {
            self.toggle_pane_fullscreen(active_pane_id);
        }
    }

    pub fn toggle_active_pane_no_ui_fullscreen(&mut self, client_id: ClientId) {
        if let Some(active_pane_id) = self.get_active_pane_id(client_id) {
            self.toggle_pane_no_ui_fullscreen(active_pane_id);
        }
    }

    pub fn toggle_pane_fullscreen(&mut self, pane_id: PaneId) {
        if !self.panes.contains_key(&pane_id) {
            return;
        }
        if self.fullscreen_is_active.is_some() {
            let was_no_ui = *self.fullscreen_covers_ui.borrow();
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
        if self.fullscreen_is_active.is_some() {
            let was_no_ui = *self.fullscreen_covers_ui.borrow();
            self.unset_fullscreen();
            if !was_no_ui {
                self.set_fullscreen(pane_id, true);
            }
        } else {
            self.set_fullscreen(pane_id, true);
        }
    }

    fn set_fullscreen(&mut self, pane_id: PaneId, covers_ui: bool) {
        if !self.panes.contains_key(&pane_id) {
            return;
        }
        self.panes_to_hide = self.panes_covered_by_fullscreen(pane_id, covers_ui);
        if self.panes_to_hide.is_empty() && !covers_ui {
            return;
        }
        if covers_ui {
            self.expand_pane_over_whole_display(pane_id);
        } else {
            self.pin_panes_outside_viewport_in_place();
            self.expand_pane_over_whole_viewport(pane_id);
        }
        self.focus_pane_for_all_clients(pane_id);
        self.fullscreen_is_active = Some(pane_id);
        *self.fullscreen_covers_ui.borrow_mut() = covers_ui;
        self.rerun_layout_solver_and_rerender();
    }

    fn panes_covered_by_fullscreen(
        &self,
        fullscreen_pane_id: PaneId,
        covers_ui: bool,
    ) -> HashSet<PaneId> {
        let viewport = *self.viewport.borrow();
        self.panes
            .iter()
            .filter(|(id, pane)| {
                **id != fullscreen_pane_id && (covers_ui || is_inside_viewport(&viewport, pane))
            })
            .map(|(id, _)| *id)
            .collect()
    }

    fn pin_panes_outside_viewport_in_place(&mut self) {
        let viewport = *self.viewport.borrow();
        for pane in self.panes.values_mut() {
            if !is_inside_viewport(&viewport, pane) {
                pane.set_geom_override(pane.position_and_size());
            }
        }
    }

    fn expand_pane_over_whole_display(&mut self, pane_id: PaneId) {
        let display_area = { *self.display_area.borrow() };
        self.expand_pane_over_area(pane_id, 0, 0, display_area.rows, display_area.cols);
    }

    fn expand_pane_over_whole_viewport(&mut self, pane_id: PaneId) {
        let viewport = { *self.viewport.borrow() };
        self.expand_pane_over_area(
            pane_id,
            viewport.x,
            viewport.y,
            viewport.rows,
            viewport.cols,
        );
    }

    fn expand_pane_over_area(
        &mut self,
        pane_id: PaneId,
        x: usize,
        y: usize,
        rows: usize,
        cols: usize,
    ) {
        if let Some(pane) = self.get_pane_mut(pane_id) {
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

    fn rerun_layout_solver_and_rerender(&mut self) {
        self.set_force_render();
        let display_area = *self.display_area.borrow();
        self.resize(display_area);
    }

    fn reenter_active_pane_fullscreen(&mut self, client_id: ClientId, covers_ui: bool) {
        if covers_ui {
            self.toggle_active_pane_no_ui_fullscreen(client_id);
        } else {
            self.toggle_active_pane_fullscreen(client_id);
        }
    }

    pub fn focus_pane_left_fullscreen(&mut self, client_id: ClientId) -> bool {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        let ret = self.move_focus_left(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
        ret
    }

    pub fn focus_pane_right_fullscreen(&mut self, client_id: ClientId) -> bool {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        let ret = self.move_focus_right(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
        ret
    }

    pub fn focus_pane_up_fullscreen(&mut self, client_id: ClientId) -> bool {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        let ret = self.move_focus_up(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
        ret
    }

    pub fn focus_pane_down_fullscreen(&mut self, client_id: ClientId) -> bool {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        let ret = self.move_focus_down(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
        ret
    }

    pub fn switch_next_pane_fullscreen(&mut self, client_id: ClientId) {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        self.focus_next_pane(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
    }

    pub fn switch_prev_pane_fullscreen(&mut self, client_id: ClientId) {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        self.focus_previous_pane(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
    }

    pub fn switch_last_pane_fullscreen(&mut self, client_id: ClientId) {
        let covers_ui = *self.fullscreen_covers_ui.borrow();
        self.unset_fullscreen();
        self.focus_last_pane(client_id);
        self.reenter_active_pane_fullscreen(client_id, covers_ui);
    }

    pub fn panes_to_hide_count(&self) -> usize {
        self.panes_to_hide.len()
    }
    pub fn visible_panes_count(&self) -> usize {
        self.panes.len().saturating_sub(self.panes_to_hide.len())
    }
    pub fn add_to_hidden_panels(&mut self, pid: PaneId) {
        self.panes_to_hide.insert(pid);
    }
    pub fn remove_from_hidden_panels(&mut self, pid: PaneId) {
        self.panes_to_hide.remove(&pid);
    }
    pub fn unfocus_all_panes(&mut self) {
        self.active_panes
            .unfocus_all_panes(&self.panes, &self.connected_clients.borrow());
    }
    pub fn focus_all_panes(&mut self) {
        self.active_panes
            .focus_all_panes(&self.panes, &self.connected_clients.borrow());
    }
    pub fn drain(&mut self) -> BTreeMap<PaneId, Box<dyn Pane>> {
        self.unset_fullscreen();
        match self.panes.iter().next().map(|(pid, _p)| *pid) {
            Some(first_pid) => self.panes.split_off(&first_pid),
            None => BTreeMap::new(),
        }
    }
    pub fn active_panes(&self) -> ActivePanes {
        self.active_panes.clone()
    }
    pub fn set_active_panes(&mut self, active_panes: ActivePanes) {
        self.active_panes = active_panes;
    }
    pub fn forget_client(&mut self, client_id: ClientId) {
        self.active_panes.forget_client(&client_id);
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
    pub fn invalidate_boundaries(&mut self, client_id: ClientId) {
        self.client_id_to_boundaries.remove(&client_id);
    }
    fn reset_boundaries(&mut self) {
        self.client_id_to_boundaries.clear();
    }
    pub fn get_plugin_pane_id(&self, run_plugin_or_alias: &RunPluginOrAlias) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_id, pane)| run_plugin_or_alias.is_equivalent_to_run(pane.invoked_with()))
            .map(|(id, _)| *id)
    }
    pub fn pane_info(&self, current_pane_group: &HashMap<ClientId, Vec<PaneId>>) -> Vec<PaneInfo> {
        let mut pane_infos = vec![];
        for (pane_id, pane) in self.panes.iter() {
            let mut pane_info_for_pane =
                pane_info_for_pane(pane_id, pane.as_ref(), current_pane_group);
            let is_focused = self.active_panes.pane_id_is_focused(pane_id);
            pane_info_for_pane.is_floating = false;
            pane_info_for_pane.is_suppressed = false;
            pane_info_for_pane.is_focused = is_focused;
            pane_info_for_pane.is_fullscreen = is_focused && self.fullscreen_is_active();
            pane_infos.push(pane_info_for_pane);
        }
        pane_infos
    }

    pub fn pane_id_is_focused(&self, pane_id: &PaneId) -> bool {
        self.active_panes.pane_id_is_focused(pane_id)
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
    pub fn stack_panes(
        &mut self,
        root_pane_id: PaneId,
        pane_count_in_stack: usize,
    ) -> Vec<PaneGeom> {
        StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
            .new_stack(root_pane_id, pane_count_in_stack)
    }
    fn is_connected(&self, client_id: &ClientId) -> bool {
        self.connected_clients.borrow().contains(client_id)
    }
    pub fn stacked_pane_ids_under_and_over_flexible_panes(
        &mut self,
    ) -> Result<(HashSet<PaneId>, HashSet<PaneId>)> {
        StackedPanes::new_from_btreemap(&mut self.panes, &self.panes_to_hide)
            .stacked_pane_ids_under_and_over_flexible_panes()
    }
    pub fn next_selectable_pane_id_above(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let include_panes_in_stack = true;
        pane_grid.next_selectable_pane_id_above(pane_id, include_panes_in_stack, false)
    }
    pub fn next_selectable_pane_id_below(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        let include_panes_in_stack = true;
        pane_grid.next_selectable_pane_id_below(pane_id, include_panes_in_stack, false)
    }
    pub fn next_selectable_pane_id_to_the_left(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        pane_grid.next_selectable_pane_id_to_the_left(pane_id)
    }
    pub fn next_selectable_pane_id_to_the_right(&mut self, pane_id: &PaneId) -> Option<PaneId> {
        let pane_grid = TiledPaneGrid::new(
            &mut self.panes,
            &self.panes_to_hide,
            display_area_viewport(&self.display_area),
            *self.viewport.borrow(),
        );
        pane_grid.next_selectable_pane_id_to_the_right(pane_id)
    }
}

/// What `apply_frame` needs to know about the tab as a whole. `set_pane_frames` used to
/// work it out inline; `reapply_pane_frames_in_stack_of` needs the same answers for a few
/// panes, so `TiledPanes::frame_context` gathers them once.
struct FrameContext {
    draws_full_frames: bool,
    draws_titles: bool,
    single_selectable_tiled_pane: bool,
    /// Both the viewport a pane is measured against and the boundary its content offset
    /// is computed from: `set_pane_frames` read the same field twice under two names.
    viewport: Viewport,
    no_ui_fullscreen_pane_id: Option<PaneId>,
    position_and_sizes_of_stacks: HashMap<usize, PaneGeom>,
    reserved_top_rows: HashMap<PaneId, usize>,
}

/// Give one pane the frame and content offset the tab's current style calls for.
///
/// The body of `set_pane_frames`'s loop, minus the pty resize: callers decide whom to
/// tell, because the point of `reapply_pane_frames_in_stack_of` is to tell a stack and
/// not a tab.
fn apply_frame(ctx: &FrameContext, pane: &mut Box<dyn Pane>) {
    pane.set_subtitle_enabled(ctx.draws_full_frames || ctx.draws_titles);
    if ctx.no_ui_fullscreen_pane_id == Some(pane.pid()) {
        pane.set_frame(false);
        pane.set_content_offset(Offset::default());
        return;
    }
    if !pane.borderless() {
        pane.set_frame(ctx.draws_full_frames);
    }

    let reserved_rows = ctx.reserved_top_rows.get(&pane.pid()).copied().unwrap_or(0);

    #[allow(clippy::if_same_then_else)]
    if reserved_rows > 0 {
        if ctx.draws_full_frames && !pane.borderless() {
            pane.set_content_offset(Offset {
                top: 1 + reserved_rows,
                bottom: 1,
                left: 1,
                right: 1,
            });
        } else {
            let position_and_size = pane.current_geom();
            let (pane_columns_offset, pane_rows_offset) =
                pane_content_offset(&position_and_size, &ctx.viewport);
            let visible_member_title_rows = if ctx.draws_full_frames { 1 } else { 0 };
            pane.set_content_offset(Offset {
                top: visible_member_title_rows + reserved_rows,
                bottom: pane_rows_offset,
                left: 0,
                right: pane_columns_offset,
            });
        }
    } else if ctx.draws_full_frames && !pane.borderless() {
        pane.set_content_offset(Offset::frame(1));
    } else if ctx.draws_full_frames && pane.borderless() {
        pane.set_content_offset(Offset::default());
    } else if !is_inside_viewport(&ctx.viewport, pane) {
        pane.set_content_offset(Offset::default());
    } else {
        let mut position_and_size = pane.current_geom();
        let is_stacked = position_and_size.is_stacked();
        let is_flexible = !position_and_size.rows.is_fixed();
        // A collapsed stack member is a single title row with no content of its
        // own, so it never reserves a title row. A fixed-size pane outside a stack
        // does: its title is drawn over its own top row.
        let is_one_liner_in_stack = is_stacked && !is_flexible;
        let pane_is_borderless = pane.borderless();
        if let Some(position_and_size_of_stack) = position_and_size
            .stacked
            .and_then(|s_id| ctx.position_and_sizes_of_stacks.get(&s_id))
        {
            position_and_size = *position_and_size_of_stack;
        };
        let (pane_columns_offset, pane_rows_offset) =
            pane_content_offset(&position_and_size, &ctx.viewport);
        let reserve_title_row = if ctx.draws_titles {
            !pane_is_borderless
                && (!ctx.single_selectable_tiled_pane || pane.subtitle().is_some())
                && !is_one_liner_in_stack
        } else {
            is_stacked && is_flexible
        };
        if reserve_title_row {
            pane.set_content_offset(Offset::shift_right_top_and_bottom(
                pane_columns_offset,
                1,
                pane_rows_offset,
            ));
        } else {
            pane.set_content_offset(Offset::shift(pane_rows_offset, pane_columns_offset));
        }
    }
}

#[allow(clippy::borrowed_box)]
pub fn is_inside_viewport(viewport: &Viewport, pane: &Box<dyn Pane>) -> bool {
    let pane_position_and_size = pane.current_geom();
    pane_position_and_size.y >= viewport.y
        && pane_position_and_size.y + pane_position_and_size.rows.as_usize()
            <= viewport.y + viewport.rows
}

pub fn pane_geom_is_inside_viewport(viewport: &Viewport, geom: &PaneGeom) -> bool {
    geom.y >= viewport.y
        && geom.y + geom.rows.as_usize() <= viewport.y + viewport.rows
        && geom.x >= viewport.x
        && geom.x + geom.cols.as_usize() <= viewport.x + viewport.cols
}

#[cfg(test)]
mod content_offset_tests {
    use super::pane_content_offset;
    use yinz_types::pane_size::{Dimension, PaneGeom, Viewport};

    fn geom(x: usize, cols: usize, y: usize, rows: usize) -> PaneGeom {
        PaneGeom {
            x,
            y,
            cols: Dimension::fixed(cols),
            rows: Dimension::fixed(rows),
            ..Default::default()
        }
    }

    #[test]
    fn offsets_are_reserved_relative_to_the_viewport_origin() {
        // a viewport that does not start at the screen origin
        let viewport = Viewport {
            x: 20,
            y: 0,
            cols: 80,
            rows: 40,
        };

        // a pane whose right edge lands mid-viewport still needs a boundary column, even though
        // its absolute right edge (85) is past `viewport.cols` (80)
        assert_eq!(pane_content_offset(&geom(20, 65, 0, 40), &viewport).0, 1);
        // a pane flush with the viewport's right edge does not
        assert_eq!(pane_content_offset(&geom(20, 80, 0, 40), &viewport).0, 0);
    }

    #[test]
    fn offsets_are_unchanged_for_an_origin_viewport() {
        let viewport = Viewport {
            x: 0,
            y: 0,
            cols: 100,
            rows: 40,
        };
        assert_eq!(pane_content_offset(&geom(0, 100, 0, 40), &viewport), (0, 0));
        assert_eq!(pane_content_offset(&geom(0, 50, 0, 20), &viewport), (1, 1));
    }
}

#[cfg(test)]
mod viewport_resize_tests {
    use super::resized_viewport_axis;

    // the rows of a 20-row screen with a one-row tab bar on top and a two-row status bar below
    const CHROME: (usize, usize, usize) = (1, 17, 20);

    #[test]
    fn a_screen_that_fits_the_insets_keeps_them() {
        let (start, extent, total) = CHROME;
        assert_eq!(resized_viewport_axis(start, extent, total, 10), (1, 7));
        assert_eq!(resized_viewport_axis(start, extent, total, 40), (1, 37));
        // no chrome at all: the viewport is the whole screen, before and after
        assert_eq!(resized_viewport_axis(0, 20, 20, 2), (0, 2));
    }

    #[test]
    fn a_resize_to_the_size_the_screen_already_is_is_the_identity() {
        // `Tab::relayout_tiled_panes` and `LayoutApplier::adjust_viewport` both resize to the
        // current display area, expecting only a pass through the constraint system
        let (start, extent, total) = CHROME;
        assert_eq!(resized_viewport_axis(start, extent, total, total), (1, 17));
        assert_eq!(resized_viewport_axis(0, 80, 80, 80), (0, 80));
    }

    #[test]
    fn a_screen_smaller_than_the_insets_empties_the_viewport() {
        let (start, extent, total) = CHROME;
        // this used to be `17 + (2 - 20)`, which wrapped around `usize`
        assert_eq!(resized_viewport_axis(start, extent, total, 2), (1, 0));
        // and here the screen is smaller than the leading inset on its own
        assert_eq!(resized_viewport_axis(start, extent, total, 0), (0, 0));
        for new_total in 0..25 {
            let (new_start, new_extent) = resized_viewport_axis(start, extent, total, new_total);
            assert!(
                new_start + new_extent <= new_total,
                "{new_start} + {new_extent} is outside a screen of {new_total}"
            );
        }
    }
}
