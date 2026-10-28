//! Overlays own their complete contents. A multipane is one placement and one layer,
//! with stable child pane identities for input, output and plugin messages.

use crate::ui::pane_boundaries_frame::OpeningRipple;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;
use yinz_types::multiplayer::AttachedClients;

use yinz_runtime::errors::prelude::*;
use yinz_types::position::{Column, ColumnCount, Position, Row, RowCount};
use yinz_types::style::{Style, Styling};
use yinz_types::{
    geometry::{FloatingPaneCoordinates, PercentOrFixed, ResizeStrategy},
    pane_size::{Offset, PaneGeom, Size, SizeInPixels, Viewport},
};
use {
    yinz_types::data::ModeInfo,
    yinz_types::data::OverlayKind,
    yinz_types::data::OverlayPlacement,
    yinz_types::data::PaneInfo,
    yinz_types::input::layout::{Run, RunPluginOrAlias},
};

use crate::pane::{pane_info_for_pane, Pane};
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::resize_pty;
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

use std::sync::atomic::{AtomicU64, Ordering};
use yinz_types::multipane::{MultipaneDirection, MultipaneId, MultipaneInfo, MultipaneSplit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct OverlayId(u64);
static NEXT_OVERLAY_ID: AtomicU64 = AtomicU64::new(1);

type Child = (PaneId, Box<dyn Pane>);
enum Children {
    Single(Child),
    Multipane {
        children: [Child; 2],
        split: MultipaneSplit,
        /// The fewest cells either child keeps along the split (`MultipaneSplit::floor`).
        min_cells: u16,
    },
}
impl Children {
    fn as_slice(&self) -> &[Child] {
        match self {
            Self::Single(child) => std::slice::from_ref(child),
            Self::Multipane { children, .. } => children,
        }
    }
    fn as_mut_slice(&mut self) -> &mut [Child] {
        match self {
            Self::Single(child) => std::slice::from_mut(child),
            Self::Multipane { children, .. } => children,
        }
    }
}

/// An owned transfer unit. Moving it cannot separate its children or lose placement claims.
pub struct OverlayPane {
    id: OverlayId,
    children: Children,
    kind: OverlayKind,
    background_opacity: Option<u8>,
    coordinates: FloatingPaneCoordinates,
    geom: PaneGeom,
    fullscreen: Option<bool>,
    carry: OverlayCarry,
    ripple: Option<OpeningRipple>,
    arrival_focus: HashMap<ClientId, PaneId>,
    retains_tab: bool,
}
impl OverlayPane {
    fn constrained_geometry(
        &self,
        mut outer: PaneGeom,
    ) -> (PaneGeom, Option<([PaneGeom; 2], PaneGeom)>) {
        let Children::Multipane {
            children,
            split,
            min_cells,
        } = &self.children
        else {
            let mut displayed = self.children.as_slice()[0].1.constrain_content_width(outer);
            if self.carry.width_is_anchored && self.fullscreen.is_none() {
                displayed.x = outer.x;
            }
            return (displayed, None);
        };
        let limits = children
            .each_ref()
            .map(|(_, pane)| match pane.max_content_width() {
                0 => usize::MAX,
                limit => limit,
            });
        let horizontal = split.direction == MultipaneDirection::LeftRight;
        let frame = 2 * usize::from(self.framed());
        let limit = if horizontal {
            limits[0].saturating_add(limits[1]).saturating_add(1)
        } else {
            limits[0].min(limits[1])
        }
        .saturating_add(frame);
        let spare = outer.cols.as_usize().saturating_sub(limit);
        if !self.carry.width_is_anchored || self.fullscreen.is_some() {
            outer.x += spare / 2;
        }
        outer.cols.decrease_inner(spare);
        let (mut geoms, mut divider) = split.geometry(outer, self.framed(), *min_cells);
        if horizontal {
            let available = geoms[0].cols.as_usize() + geoms[1].cols.as_usize();
            let first = geoms[0].cols.as_usize().clamp(
                available.saturating_sub(limits[1]),
                limits[0].min(available),
            );
            geoms[0].cols.set_inner(first);
            divider.x = geoms[0].x + first;
            geoms[1].x = divider.x + divider.cols.as_usize();
            geoms[1].cols.set_inner(available - first);
        }
        (outer, Some((geoms, divider)))
    }
    fn borderless(&self) -> bool {
        self.coordinates
            .borderless
            .unwrap_or_else(|| match &self.children {
                Children::Single((_, pane)) => pane.borderless(),
                Children::Multipane { .. } => false,
            })
    }
    fn framed(&self) -> bool {
        !self.borderless() && self.fullscreen != Some(true)
    }
    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.children.as_slice().iter().map(|(id, _)| *id)
    }
    pub fn reveal(&mut self) {
        self.carry.hidden = false;
    }
    pub fn into_panes(self) -> Vec<Box<dyn Pane>> {
        match self.children {
            Children::Single((_, pane)) => vec![pane],
            Children::Multipane { children, .. } => {
                children.into_iter().map(|(_, pane)| pane).collect()
            },
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayCarry {
    pub hidden: bool,
    pub position_is_user_stated: bool,
    pub width_is_anchored: bool,
    pub size_is_user_stated: bool,
    pub awaiting_placement: bool,
    pub opening_seen: bool,
    pub opening_awaits_placement: bool,
    pub opening_started: Option<Instant>,
}
impl PaneLookup for BTreeMap<OverlayId, OverlayPane> {
    fn pane(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.values()
            .flat_map(|overlay| overlay.children.as_slice())
            .find(|(id, _)| id == pane_id)
            .map(|(_, pane)| pane)
    }
}
pub struct OverlayPanes {
    panes: BTreeMap<OverlayId, OverlayPane>,
    z_indices: Vec<OverlayId>,
    active_panes: ActivePanes,
    pane_being_moved_with_mouse: Option<(PaneId, Position, Position)>,
    seam_being_dragged: Option<(PaneId, PaneId)>,
    display_area: Rc<RefCell<Size>>,
    viewport: Rc<RefCell<Viewport>>,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    connected_clients_in_app: Rc<RefCell<AttachedClients>>,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    style: Style,
    session_is_mirrored: bool,
    default_mode_info: ModeInfo,
    senders: ThreadSenders,
    window_title: Option<String>,
    dimmed_clients: HashSet<ClientId>,
    /// Where each client's keyboard goes back to when a modal container it is on goes away:
    /// the overlay it was on when the modal took the keyboard, or `None` for the layers
    /// beneath. Without it the keyboard fell to whichever overlay was topmost, so a modal
    /// opened from one half of a panel closed onto the other half. See `modal_return_for`.
    modal_return: HashMap<(ClientId, OverlayId), Option<PaneId>>,
}

#[allow(clippy::borrowed_box)]
#[allow(clippy::too_many_arguments)]
impl OverlayPanes {
    pub fn new(
        display_area: Rc<RefCell<Size>>,
        viewport: Rc<RefCell<Viewport>>,
        connected_clients: Rc<RefCell<HashSet<ClientId>>>,
        connected_clients_in_app: Rc<RefCell<AttachedClients>>,
        mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
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
            window_title: None,
            dimmed_clients: HashSet::new(),
            pane_being_moved_with_mouse: None,
            seam_being_dragged: None,
            modal_return: HashMap::new(),
        }
    }
    fn surface(&self) -> Viewport {
        let display_area = *self.display_area.borrow();
        Viewport {
            x: 0,
            y: 0,
            cols: display_area.cols,
            rows: display_area.rows,
        }
    }
    fn base_geom(surface: &Viewport, current: PaneGeom) -> PaneGeom {
        let mut geom = half_size_middle_geom(surface, 0);
        geom.is_pinned = current.is_pinned;
        geom.logical_position = current.logical_position;
        geom
    }
    fn resolved(
        surface: &Viewport,
        current: PaneGeom,
        coordinates: &FloatingPaneCoordinates,
        width_is_anchored: bool,
    ) -> PaneGeom {
        let mut geom = Self::base_geom(surface, current);
        let mut coordinates = coordinates.clone();
        let anchored_x = if width_is_anchored {
            coordinates.x.take()
        } else {
            None
        };
        geom.adjust_coordinates(coordinates, *surface);
        if let Some(PercentOrFixed::Fixed(x)) = anchored_x {
            // Fit the requested width before applying the displayed position.
            geom.x = x.min((surface.x + surface.cols).saturating_sub(geom.cols.as_usize()));
        }
        geom
    }

    fn container_id(&self, pane_id: &PaneId) -> Option<OverlayId> {
        self.panes
            .iter()
            .find(|(_, overlay)| overlay.pane_ids().any(|id| id == *pane_id))
            .map(|(id, _)| *id)
    }
    fn overlay(&self, pane_id: &PaneId) -> Option<&OverlayPane> {
        self.container_id(pane_id)
            .and_then(|id| self.panes.get(&id))
    }
    fn overlay_mut(&mut self, pane_id: &PaneId) -> Option<&mut OverlayPane> {
        let id = self.container_id(pane_id)?;
        self.panes.get_mut(&id)
    }
    pub fn multipane_info(&self, pane_id: &PaneId) -> Option<MultipaneInfo> {
        let overlay = self.overlay(pane_id)?;
        let Children::Multipane {
            children, split, ..
        } = &overlay.children
        else {
            return None;
        };
        Some(MultipaneInfo {
            id: MultipaneId(overlay.id.0),
            children: [children[0].0, children[1].0],
            x: overlay.geom.x,
            y: overlay.geom.y,
            rows: overlay.geom.rows.as_usize(),
            columns: overlay.geom.cols.as_usize(),
            split: *split,
        })
    }
    pub fn members(&self, pane_id: &PaneId) -> Vec<PaneId> {
        self.overlay(pane_id)
            .map(|overlay| overlay.pane_ids().collect())
            .unwrap_or_default()
    }
    pub fn has_focused_member(&self, pane_id: &PaneId) -> bool {
        self.members(pane_id)
            .iter()
            .any(|member| self.active_panes.pane_id_is_focused(member))
    }
    pub fn add_pane(
        &mut self,
        pane_id: PaneId,
        pane: Box<dyn Pane>,
        kind: OverlayKind,
        coordinates: FloatingPaneCoordinates,
        mut carry: OverlayCarry,
    ) {
        carry.opening_awaits_placement |= carry.awaiting_placement;
        let geom = Self::resolved(
            &self.surface(),
            pane.position_and_size(),
            &coordinates,
            carry.width_is_anchored,
        );
        let overlay = OverlayPane {
            id: OverlayId(NEXT_OVERLAY_ID.fetch_add(1, Ordering::Relaxed)),
            children: Children::Single((pane_id, pane)),
            kind,
            background_opacity: None,
            coordinates,
            geom,
            fullscreen: None,
            carry,
            ripple: None,
            arrival_focus: HashMap::new(),
            retains_tab: false,
        };
        self.insert_container(overlay);
    }
    pub fn insert_container(&mut self, mut overlay: OverlayPane) {
        overlay.geom = Self::resolved(
            &self.surface(),
            overlay.geom,
            &overlay.coordinates,
            overlay.carry.width_is_anchored,
        );
        let first = overlay.children.as_slice()[0].0;
        for (_, pane) in overlay.children.as_mut_slice() {
            pane.reset_logical_position();
        }
        let arrival_focus = std::mem::take(&mut overlay.arrival_focus);
        self.panes.insert(overlay.id, overlay);
        self.raise_within_class(first);
        for (client, pane) in arrival_focus {
            self.focus_pane(pane, client);
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
    }
    /// Validate both children before moving ownership. No partial adoption on error.
    pub fn create_multipane(
        &mut self,
        children: [PaneId; 2],
        split: MultipaneSplit,
        min_cells: u16,
        placement: OverlayPlacement,
    ) -> Result<MultipaneId> {
        placement.validate().map_err(|error| anyhow!(error))?;
        split.validate().map_err(|error| anyhow!(error))?;
        if children[0] == children[1] {
            return Err(anyhow!("multipane children must be distinct"));
        }
        let ids: Vec<_> = children
            .iter()
            .map(|id| {
                self.container_id(id)
                    .context("multipane child is not an overlay")
            })
            .collect::<Result<_>>()?;
        for id in &ids {
            let overlay = &self.panes[id];
            if !matches!(overlay.children, Children::Single(_)) {
                return Err(anyhow!("nested multipanes are not supported"));
            }
            if overlay.kind != placement.kind {
                return Err(anyhow!(
                    "multipane children must have the same overlay kind"
                ));
            }
        }
        // Focus belongs to the child, so leave the active-pane map in place during adoption.
        let mut first = self.panes.remove(&ids[0]).context("missing first child")?;
        let second = self.panes.remove(&ids[1]).context("missing second child")?;
        let Children::Single(a) = first.children else {
            unreachable!()
        };
        let Children::Single(b) = second.children else {
            unreachable!()
        };
        self.z_indices.retain(|id| !ids.contains(id));
        self.modal_return
            .retain(|(_, container), _| !ids.contains(container));
        first.id = OverlayId(NEXT_OVERLAY_ID.fetch_add(1, Ordering::Relaxed));
        let id = MultipaneId(first.id.0);
        first.children = Children::Multipane {
            children: [a, b],
            split,
            min_cells,
        };
        first.coordinates = placement.coordinates.unwrap_or_default();
        first.background_opacity = placement.background_opacity;
        first.carry.hidden |= second.carry.hidden;
        first.carry.awaiting_placement = false;
        first.carry.opening_awaits_placement = false;
        first.carry.opening_seen = false;
        first.carry.opening_started = None;
        first.fullscreen = first.fullscreen.or(second.fullscreen);
        self.seam_being_dragged = None;
        self.pane_being_moved_with_mouse = None;
        self.insert_container(first);
        Ok(id)
    }
    pub fn retain_tab_for(&mut self, child: PaneId) {
        if let Some(overlay) = self.overlay_mut(&child) {
            overlay.retains_tab = true;
        }
    }
    pub fn selectable_tab_content_pane_id(&self) -> Option<PaneId> {
        self.selectable_tab_content_pane_ids().min()
    }
    pub fn selectable_tab_content_pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.panes
            .values()
            .filter(|overlay| overlay.retains_tab)
            .flat_map(|overlay| overlay.children.as_slice())
            .filter(|(_, pane)| pane.selectable())
            .map(|(id, _)| *id)
    }
    pub fn update_split(&mut self, id: MultipaneId, split: MultipaneSplit) -> Result<()> {
        split.validate().map_err(|error| anyhow!(error))?;
        let overlay = self
            .panes
            .get_mut(&OverlayId(id.0))
            .context("unknown multipane")?;
        let Children::Multipane { split: current, .. } = &mut overlay.children else {
            return Err(anyhow!("not a multipane"));
        };
        *current = split;
        self.set_pane_frames()?;
        self.set_force_render();
        Ok(())
    }
    pub fn take_container(&mut self, pane_id: PaneId) -> Option<OverlayPane> {
        let id = self.container_id(&pane_id)?;
        let members = self.members(&pane_id);
        // Hide before relocating focus, so no client can fall onto the sibling.
        let carry = self.carry_of(&pane_id);
        let fullscreen = self.panes[&id].fullscreen;
        // The keyboard travels only with a client still on this tab. One that has already
        // switched away left its entry behind - leaving a tab does not clear it - and carrying
        // that would take the keys back from the tab the client went to: agent carries its panel
        // after a tab switch or `Ctrl t n` (`follow_tab`), and it lands drawn but unfocused, so
        // the new tab's pane keeps them. A break or a tab close done while the client is still
        // here keeps its focus, and an explicit focus once the container lands wins either way.
        let arrival_focus = {
            let connected_clients = self.connected_clients.borrow();
            self.active_panes
                .iter()
                .filter(|(client, id)| members.contains(id) && connected_clients.contains(*client))
                .map(|(client, id)| (*client, *id))
                .collect()
        };
        self.hide_pane(pane_id);
        self.z_indices.retain(|other| *other != id);
        let mut overlay = self.panes.remove(&id)?;
        // Extraction itself does not change visibility or expansion.
        overlay.carry = carry;
        overlay.fullscreen = fullscreen;
        overlay.arrival_focus = arrival_focus;
        for (_, pane) in overlay.children.as_mut_slice() {
            pane.reset_size_and_position_override();
            pane.reset_logical_position();
        }
        // The hide above has already handed back whatever keyboard this container held. What
        // is left names a container no longer here, or a way back to a pane no longer here.
        self.modal_return.retain(|(_, container), back| {
            *container != id && !back.is_some_and(|back| members.contains(&back))
        });
        for member in members {
            self.release_seam_drag_of(member);
        }
        Some(overlay)
    }
    pub fn remove_pane(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        if self.multipane_info(&pane_id).is_some() {
            return None;
        }
        self.take_container(pane_id)?.into_panes().pop()
    }
    /// Take one child out of its multipane and leave the other as the container's only pane, in
    /// the same placement and layer. Clients on the removed child move to its sibling, which is
    /// the one pane of the composition still there. `None`, with nothing changed, when
    /// `pane_id` is not a multipane member.
    ///
    /// Only for a slot that was never the user's, such as one held for a child that will not
    /// come: every structural move takes the whole container (`take_container`).
    pub fn remove_multipane_member(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        let (first, second) = self.multipane_children(&pane_id)?;
        let kept = if first == pane_id { second } else { first };
        self.move_clients_between_panes(pane_id, kept);
        for back in self.modal_return.values_mut() {
            if *back == Some(pane_id) {
                *back = Some(kept);
            }
        }
        self.release_seam_drag_of(pane_id);
        if self
            .pane_being_moved_with_mouse
            .is_some_and(|(moved, ..)| moved == pane_id)
        {
            self.pane_being_moved_with_mouse = None;
        }
        let id = self.container_id(&pane_id)?;
        let mut overlay = self.panes.remove(&id)?;
        let [a, b] = match overlay.children {
            Children::Multipane { children, .. } => children,
            single @ Children::Single(_) => {
                overlay.children = single;
                self.panes.insert(id, overlay);
                return None;
            },
        };
        let ((_, mut removed), (kept_id, mut kept_pane)) =
            if a.0 == pane_id { (a, b) } else { (b, a) };
        removed.reset_size_and_position_override();
        // The divider's share of the container is the kept pane's again.
        kept_pane.reset_size_and_position_override();
        overlay.children = Children::Single((kept_id, kept_pane));
        self.panes.insert(id, overlay);
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        Some(removed)
    }
    pub fn replace_pane(
        &mut self,
        pane_id: PaneId,
        with_pane: Box<dyn Pane>,
    ) -> Result<Box<dyn Pane>> {
        let with_id = with_pane.pid();
        if with_id != pane_id && self.panes_contain(&with_id) {
            return Err(anyhow!("replacement pane already belongs to an overlay"));
        }
        let overlay = self
            .overlay_mut(&pane_id)
            .context("overlay pane not found")?;
        let (id, pane) = overlay
            .children
            .as_mut_slice()
            .iter_mut()
            .find(|(id, _)| *id == pane_id)
            .context("overlay child not found")?;
        *id = with_id;
        let mut old = std::mem::replace(pane, with_pane);
        old.reset_size_and_position_override();
        self.move_clients_between_panes(pane_id, with_id);
        for back in self.modal_return.values_mut() {
            if *back == Some(pane_id) {
                *back = Some(with_id);
            }
        }
        self.set_pane_frames()?;
        self.set_force_render();
        Ok(old)
    }
    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes
            .values()
            .flat_map(|overlay| overlay.children.as_slice().iter().map(|(id, _)| id))
    }
    pub fn panes_contain(&self, pane_id: &PaneId) -> bool {
        self.container_id(pane_id).is_some()
    }
    pub fn get(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.panes.pane(pane_id)
    }
    pub fn get_mut(&mut self, pane_id: &PaneId) -> Option<&mut Box<dyn Pane>> {
        self.overlay_mut(pane_id)?
            .children
            .as_mut_slice()
            .iter_mut()
            .find(|(id, _)| id == pane_id)
            .map(|(_, pane)| pane)
    }
    pub fn get_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.panes.values().flat_map(|overlay| {
            overlay
                .children
                .as_slice()
                .iter()
                .map(|(id, pane)| (id, pane))
        })
    }
    pub fn get_panes_mut(&mut self) -> impl Iterator<Item = (&PaneId, &mut Box<dyn Pane>)> {
        self.panes.values_mut().flat_map(|overlay| {
            overlay
                .children
                .as_mut_slice()
                .iter_mut()
                .map(|(id, pane)| (&*id, pane))
        })
    }
    pub fn len(&self) -> usize {
        self.pane_ids().count()
    }
    pub fn get_plugin_pane_id(&self, run: &RunPluginOrAlias) -> Option<PaneId> {
        self.get_panes()
            .find(|(_, pane)| run.is_equivalent_to_run(pane.invoked_with()))
            .map(|(id, _)| *id)
    }
    pub fn has_panes(&self) -> bool {
        self.panes.values().any(|overlay| !overlay.carry.hidden)
    }
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
    pub fn is_hidden(&self, pane_id: &PaneId) -> bool {
        self.carry_of(pane_id).hidden
    }
    pub fn carry_of(&self, pane_id: &PaneId) -> OverlayCarry {
        self.overlay(pane_id)
            .map(|overlay| overlay.carry)
            .unwrap_or_default()
    }
    pub fn finish_placement(&mut self, pane_id: PaneId) {
        if let Some(overlay) = self.overlay_mut(&pane_id) {
            overlay.carry.awaiting_placement = false;
        }
    }
    pub fn get_pane(&self, pane_id: PaneId) -> Option<&Box<dyn Pane>> {
        self.get(&pane_id)
    }
    pub fn get_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.get_mut(&pane_id)
    }
    pub fn placement_of(&self, pane_id: &PaneId) -> Option<OverlayPlacement> {
        self.overlay(pane_id).map(|overlay| OverlayPlacement {
            kind: overlay.kind,
            background_opacity: overlay.background_opacity,
            coordinates: Some(overlay.coordinates.clone()),
        })
    }
    pub fn set_background_opacity(&mut self, pane: PaneId, opacity: u8) -> Result<()> {
        yinz_types::data::validate_background_opacity(Some(opacity))
            .map_err(|error| anyhow!(error))?;
        self.overlay_mut(&pane)
            .context("missing overlay")?
            .background_opacity = Some(opacity);
        self.set_force_render();
        Ok(())
    }
    pub fn translucent_layers(&self, offset: usize, opacity: u8) -> Vec<(usize, CellRect, u8)> {
        self.visible_containers()
            .iter()
            .enumerate()
            .filter(|_| opacity < 100)
            .filter_map(|(index, id)| {
                let overlay = &self.panes[id];
                overlay
                    .background_opacity
                    .filter(|opacity| *opacity < 100)?;
                let geom = self.current_geom(overlay);
                Some((
                    offset + index + 1,
                    CellRect {
                        x: geom.x,
                        y: geom.y,
                        cols: geom.cols.as_usize(),
                        rows: geom.rows.as_usize(),
                    },
                    opacity,
                ))
            })
            .collect()
    }
    pub fn kind_of(&self, pane_id: &PaneId) -> Option<OverlayKind> {
        self.overlay(pane_id).map(|overlay| overlay.kind)
    }
    pub fn is_modal(&self, pane_id: &PaneId) -> bool {
        self.kind_of(pane_id).is_some_and(|kind| kind.is_modal())
    }
    pub fn is_resizable(&self, pane_id: &PaneId) -> bool {
        self.kind_of(pane_id)
            .is_some_and(|kind| !matches!(kind, OverlayKind::Modal))
            && !self.pane_is_fullscreen(pane_id)
    }
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

    pub fn pane_info(&self, current_pane_group: &HashMap<ClientId, Vec<PaneId>>) -> Vec<PaneInfo> {
        self.pane_ids()
            .filter_map(|id| self.pane_info_of(id, current_pane_group))
            .collect()
    }
    pub fn pane_info_of(
        &self,
        id: &PaneId,
        group: &HashMap<ClientId, Vec<PaneId>>,
    ) -> Option<PaneInfo> {
        let overlay = self.overlay(id)?;
        let pane = self.get(id)?;
        let mut info = pane_info_for_pane(id, pane.as_ref(), group);
        info.is_floating = true;
        info.is_suppressed = overlay.carry.hidden;
        info.is_focused = !overlay.carry.hidden && self.active_panes.pane_id_is_focused(id);
        info.is_fullscreen = overlay.fullscreen.is_some();
        info.multipane = self.multipane_info(id);
        if info.is_fullscreen {
            let geom = pane.position_and_size();
            let offset = if matches!(overlay.children, Children::Single(_)) && !pane.borderless() {
                Offset::frame(1)
            } else {
                Offset::default()
            };
            info.pane_x = Column::new(geom.x);
            info.pane_y = Row::new(geom.y);
            info.pane_columns = ColumnCount::new(geom.cols.as_usize());
            info.pane_rows = RowCount::new(geom.rows.as_usize());
            info.pane_content_x = Column::new(geom.x + offset.left);
            info.pane_content_y = Row::new(geom.y + offset.top);
            info.pane_content_columns = ColumnCount::new(
                geom.cols
                    .as_usize()
                    .saturating_sub(offset.left + offset.right),
            );
            info.pane_content_rows = RowCount::new(
                geom.rows
                    .as_usize()
                    .saturating_sub(offset.top + offset.bottom),
            );
        }
        Some(info)
    }
    fn visible_containers(&self) -> Vec<OverlayId> {
        self.z_indices
            .iter()
            .filter(|id| {
                self.panes.get(id).is_some_and(|overlay| {
                    !overlay.carry.hidden && !overlay.carry.awaiting_placement
                })
            })
            .copied()
            .collect()
    }
    pub fn stack_layer_ids(&self) -> Vec<PaneId> {
        self.visible_containers()
            .iter()
            .flat_map(|id| self.panes[id].pane_ids())
            .collect()
    }
    pub fn rendered_pane_ids(&self) -> Vec<PaneId> {
        self.stack_layer_ids()
    }
    pub fn get_pane_z_index(&self, pane_id: PaneId) -> Option<usize> {
        let id = self.container_id(&pane_id)?;
        self.visible_containers()
            .iter()
            .position(|other| *other == id)
    }
    fn current_geom(&self, overlay: &OverlayPane) -> PaneGeom {
        let requested = overlay
            .fullscreen
            .map(|covers_ui| viewport_geom(self.fullscreen_area(covers_ui)))
            .unwrap_or(overlay.geom);
        let mut displayed = overlay.constrained_geometry(requested).0;
        if overlay.carry.width_is_anchored && overlay.fullscreen.is_none() {
            if let Some(PercentOrFixed::Fixed(x)) = overlay.coordinates.x {
                displayed.x = x;
            }
            geom_moved_within(displayed, self.surface(), 0, 0)
        } else {
            displayed
        }
    }
    pub fn outer_geom(&self, pane_id: PaneId) -> Option<PaneGeom> {
        self.overlay(&pane_id)
            .map(|overlay| self.current_geom(overlay))
    }
    pub fn layers(&self) -> Vec<PaneGeom> {
        self.visible_containers()
            .iter()
            .map(|id| {
                let overlay = &self.panes[id];
                overlay
                    .fullscreen
                    .map(|ui| viewport_geom(self.fullscreen_area(ui)))
                    .unwrap_or_else(|| self.current_geom(overlay))
            })
            .collect()
    }
    pub fn agent_widths(&self) -> HashMap<PaneId, (usize, usize)> {
        self.panes
            .values()
            .flat_map(|overlay| {
                let outer = self.current_geom(overlay);
                let (_, split) = overlay.constrained_geometry(outer);
                overlay
                    .children
                    .as_slice()
                    .iter()
                    .enumerate()
                    .map(move |(index, (id, _))| {
                        let geom = split.map(|(geoms, _)| geoms[index]).unwrap_or(outer);
                        (*id, (geom.x, geom.cols.as_usize()))
                    })
            })
            .collect()
    }
    fn raise_within_class(&mut self, pane_id: PaneId) {
        let Some(id) = self.container_id(&pane_id) else {
            return;
        };
        self.z_indices.retain(|other| *other != id);
        let lowest_modal = if self.is_modal(&pane_id) {
            None
        } else {
            self.z_indices
                .iter()
                .position(|id| self.panes[id].kind.is_modal())
        };
        match lowest_modal {
            Some(index) => self.z_indices.insert(index, id),
            None => self.z_indices.push(id),
        }
    }
    pub fn hide_pane(&mut self, pane_id: PaneId) -> bool {
        let members = self.members(&pane_id);
        let Some(overlay) = self.overlay_mut(&pane_id) else {
            return false;
        };
        if overlay.carry.hidden {
            return false;
        }
        overlay.carry.hidden = true;
        overlay.fullscreen = None;
        overlay.carry.opening_seen = false;
        overlay.carry.opening_started = None;
        overlay.ripple = None;
        if self
            .pane_being_moved_with_mouse
            .is_some_and(|(id, ..)| members.contains(&id))
        {
            self.pane_being_moved_with_mouse = None;
        }
        for member in members {
            self.release_seam_drag_of(member);
            if self.is_modal(&member) {
                self.move_clients_out_of_modal(member);
            } else {
                self.move_clients_out_of_pane(member);
            }
        }
        self.set_force_render();
        true
    }
    pub fn show_pane(&mut self, pane_id: PaneId) -> bool {
        match self.overlay_mut(&pane_id) {
            Some(overlay) if overlay.carry.hidden => overlay.carry.hidden = false,
            _ => return false,
        }
        let outranked = self.is_modal(&pane_id)
            && self.stack_layer_ids().iter().any(|other| {
                self.container_id(other) != self.container_id(&pane_id)
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
    pub fn topmost_modal_pane_id(&self) -> Option<PaneId> {
        self.stack_layer_ids().into_iter().rev().find(|pane_id| {
            self.is_modal(pane_id) && self.get(pane_id).is_some_and(|pane| pane.selectable())
        })
    }
    pub fn first_active_pane_id(&self) -> Option<PaneId> {
        self.active_panes.values().next().copied()
    }
    pub fn focus_pane_for_joining_client(&mut self, pane_id: PaneId, client_id: ClientId) {
        if self.active_panes.contains_key(&client_id) || !self.may_be_focused(&pane_id) {
            return;
        }
        self.note_modal_return(client_id, None, pane_id);
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.set_force_render();
    }
    pub fn focus_pane_for_all_clients(&mut self, pane_id: PaneId) {
        if !self.may_be_focused(&pane_id) {
            return;
        }
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            self.note_modal_return(client_id, self.active_pane_id(client_id), pane_id);
            self.active_panes.insert(client_id, pane_id, &self.panes);
            self.set_pane_active_at(pane_id);
        }
        self.raise_within_class(pane_id);
        self.set_force_render();
    }
    pub fn may_be_focused(&self, pane_id: &PaneId) -> bool {
        if self.is_hidden(pane_id) {
            return false;
        }
        if !self.get(pane_id).is_some_and(|pane| pane.selectable()) {
            return false;
        }
        self.is_modal(pane_id) || self.topmost_modal_pane_id().is_none()
    }
    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        if !self.may_be_focused(&pane_id) {
            return;
        }
        let previously_active = self.active_pane_id(client_id);
        if let Some(previously_active) = previously_active {
            self.active_panes
                .set_last_pane(client_id, previously_active);
        }
        self.note_modal_return(client_id, previously_active, pane_id);
        self.active_panes.insert(client_id, pane_id, &self.panes);
        self.set_pane_active_at(pane_id);
        self.raise_within_class(pane_id);
        self.set_force_render();
    }
    pub fn focus_pane_if_exists(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        if self.panes_contain(&pane_id) {
            self.show_pane(pane_id);
            self.focus_pane(pane_id, client_id);
            Ok(())
        } else {
            Err(anyhow!("Pane not found"))
        }
    }
    pub fn defocus_pane(&mut self, client_id: ClientId) {
        self.active_panes.remove(&client_id, &self.panes);
        self.set_force_render();
    }
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
    pub fn a_modal_holds_focus(&self, client_id: ClientId) -> bool {
        self.active_pane_id(client_id)
            .is_some_and(|pane_id| self.is_modal(&pane_id))
    }
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
    pub fn forget_client(&mut self, client_id: ClientId) {
        self.active_panes.forget_client(&client_id);
        self.modal_return
            .retain(|(client, _), _| *client != client_id);
    }
    /// A modal has just taken `client_id`'s keyboard from `from` (`None`: the layers beneath).
    /// Note it, so the keyboard can go back there when the modal goes (`modal_return_for`). A
    /// move between the halves of one modal multipane is not a modal taking the keyboard, and
    /// keeps the container's way back as it was.
    fn note_modal_return(&mut self, client_id: ClientId, from: Option<PaneId>, to: PaneId) {
        if !self.is_modal(&to) {
            return;
        }
        let Some(container) = self.container_id(&to) else {
            return;
        };
        if from.is_some_and(|from| self.container_id(&from) == Some(container)) {
            return;
        }
        self.modal_return.insert((client_id, container), from);
    }
    /// `client_id` asked for `pane_id` while a modal holds its keyboard. The modal keeps the
    /// keyboard (`may_be_focused`), but the named pane is where the user now means to be, so it
    /// is where the modal hands the keyboard when it goes: `new-agent` names the agent it
    /// started and then closes itself. `pane_id` is one of ours or a pane on the layers beneath;
    /// a modal named is focused in its own right and changes nothing here.
    pub fn return_from_modal_to(&mut self, client_id: ClientId, pane_id: PaneId) {
        let Some(container) = self
            .active_pane_id(client_id)
            .filter(|active| self.is_modal(active))
            .and_then(|active| self.container_id(&active))
        else {
            return;
        };
        if self.is_modal(&pane_id) {
            return;
        }
        let back = self.panes_contain(&pane_id).then_some(pane_id);
        self.modal_return.insert((client_id, container), back);
    }
    /// Where `client_id`'s keyboard goes as `leaving` stops holding it, when a modal noted a way
    /// back (`note_modal_return`, `return_from_modal_to`) and that way is still open: `Some(pane)`
    /// for an overlay, `Some(None)` for the layers beneath. `None` leaves the choice to the
    /// caller's own search.
    ///
    /// The way back is taken only when the modal's whole container is leaving the client - a
    /// multipane half that stays on screen keeps the keyboard in the container - and only to a
    /// place no modal still on screen would outrank: while another modal is up, the keyboard
    /// stays in modal hands, as it always has.
    fn modal_return_for(&mut self, client_id: ClientId, leaving: PaneId) -> Option<Option<PaneId>> {
        let key = (client_id, self.container_id(&leaving)?);
        let back = *self.modal_return.get(&key)?;
        let stack = self.stack_layer_ids();
        let can_take = |pane_id: &PaneId| {
            *pane_id != leaving
                && stack.contains(pane_id)
                && self.get(pane_id).is_some_and(|pane| pane.selectable())
        };
        if self.members(&leaving).iter().any(can_take) {
            return None;
        }
        let another_modal_is_up = stack
            .iter()
            .any(|pane_id| can_take(pane_id) && self.is_modal(pane_id));
        let open = match back {
            Some(pane_id) => {
                can_take(&pane_id) && (self.is_modal(&pane_id) || !another_modal_is_up)
            },
            None => !another_modal_is_up,
        };
        if !open {
            return None;
        }
        self.modal_return.remove(&key);
        Some(back)
    }
    /// Move every client on `pane_id` elsewhere, as it closes or can no longer be focused: a
    /// modal hands each client back where it took the keyboard from (`modal_return_for`), and
    /// anything else falls to the topmost overlay still on screen, or to the layers beneath.
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
            match self.modal_return_for(client_id, pane_id).unwrap_or(next) {
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
    /// As `move_clients_out_of_pane`, for a modal being set aside rather than closed. Without a
    /// way back only another modal may take the keyboard: the overlays under a modal were not
    /// focusable a moment ago, and hiding it does not make an arbitrary one of them so.
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
            match self.modal_return_for(client_id, pane_id).unwrap_or(next) {
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

    pub fn change_pane_coordinates_batch(
        &mut self,
        batch: Vec<(PaneId, FloatingPaneCoordinates)>,
        override_user_placement: bool,
    ) -> Result<()> {
        let mut placements = BTreeMap::new();
        for (pane_id, coordinates) in batch {
            let id = self.container_id(&pane_id).context("overlay not found")?;
            if let Some((_, previous)) = placements.get(&id) {
                if previous != &coordinates {
                    return Err(anyhow!("conflicting placements for one overlay container"));
                }
            } else {
                placements.insert(id, (pane_id, coordinates));
            }
        }
        for (_, (pane_id, coordinates)) in placements {
            self.change_pane_coordinates(pane_id, coordinates, override_user_placement)?;
        }
        Ok(())
    }
    pub fn change_pane_coordinates(
        &mut self,
        pane_id: PaneId,
        new_coordinates: FloatingPaneCoordinates,
        override_user_placement: bool,
    ) -> Result<()> {
        let err_context = || format!("failed to change coordinates of overlay {pane_id:?}");
        let surface = self.surface();
        let overlay = self
            .overlay_mut(&pane_id)
            .with_context(|| format!("no overlay pane with ID {pane_id:?}"))
            .with_context(err_context)?;

        let mut new_coordinates = new_coordinates;
        new_coordinates.borderless = new_coordinates
            .borderless
            .or(overlay.coordinates.borderless);
        if override_user_placement {
            overlay.carry.position_is_user_stated = false;
            overlay.carry.width_is_anchored = false;
        }
        if overlay.carry.position_is_user_stated {
            new_coordinates.x = overlay.coordinates.x;
            new_coordinates.y = overlay.coordinates.y;
        }
        if overlay.carry.size_is_user_stated {
            new_coordinates.width = overlay.coordinates.width;
            new_coordinates.height = overlay.coordinates.height;
        }
        let geom = Self::resolved(
            &surface,
            overlay.geom,
            &new_coordinates,
            overlay.carry.width_is_anchored,
        );
        overlay.geom = geom;
        overlay.coordinates = new_coordinates;
        overlay.carry.awaiting_placement = false;
        overlay.carry.opening_awaits_placement = false;

        self.set_pane_frames().non_fatal();
        Ok(())
    }
    pub fn set_borderless(&mut self, pane_id: &PaneId, borderless: bool) -> bool {
        let Some(overlay) = self.overlay_mut(pane_id) else {
            return false;
        };
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

    pub fn reapply_coordinates(&mut self) {
        let surface = self.surface();
        for overlay in self.panes.values_mut() {
            overlay.geom = Self::resolved(
                &surface,
                overlay.geom,
                &overlay.coordinates,
                overlay.carry.width_is_anchored,
            );
            overlay.carry.awaiting_placement = false;
            overlay.carry.opening_awaits_placement = false;
        }
        self.set_pane_frames().non_fatal();
        self.set_force_render();
    }
    const RESIZE_INCREMENT_WIDTH: usize = 5;
    const RESIZE_INCREMENT_HEIGHT: usize = 2;
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

    pub fn resize_pane_with_strategies(
        &mut self,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> Result<bool> {
        if !self.is_resizable(&pane_id) {
            return Ok(false);
        }
        let surface = self.surface();
        let overlay = self.overlay_mut(&pane_id).context("overlay not found")?;
        let before = overlay.geom;
        let mut geom = before;
        for strategy in strategies {
            geom = geom_resized_within(geom, surface, strategy, change_by);
        }
        if geom == before {
            return Ok(false);
        }
        Self::claim_geom(overlay, geom, geom.x != before.x || geom.y != before.y);
        self.set_pane_frames()?;
        self.set_force_render();
        Ok(true)
    }
    fn claim_geom(overlay: &mut OverlayPane, geom: PaneGeom, moved: bool) {
        overlay.geom = geom;
        overlay.coordinates.width = Some(PercentOrFixed::Fixed(geom.cols.as_usize()));
        overlay.coordinates.height = Some(PercentOrFixed::Fixed(geom.rows.as_usize()));
        overlay.carry.size_is_user_stated = true;
        if moved {
            overlay.coordinates.x = Some(PercentOrFixed::Fixed(geom.x));
            overlay.coordinates.y = Some(PercentOrFixed::Fixed(geom.y));
            overlay.carry.position_is_user_stated = true;
        }
    }

    pub fn fullscreen_is_active(&self) -> bool {
        self.panes
            .values()
            .any(|overlay| overlay.fullscreen.is_some())
    }
    pub fn fullscreen_pane_id(&self) -> Option<PaneId> {
        self.panes
            .values()
            .find(|overlay| overlay.fullscreen.is_some())
            .map(|overlay| overlay.children.as_slice()[0].0)
    }
    pub fn fullscreen_covers_ui(&self) -> bool {
        self.panes
            .values()
            .any(|overlay| overlay.fullscreen == Some(true))
    }
    pub fn pane_is_fullscreen(&self, pane_id: &PaneId) -> bool {
        self.overlay(pane_id)
            .is_some_and(|overlay| overlay.fullscreen.is_some())
    }
    pub fn fullscreen_pane_ids(&self) -> Vec<PaneId> {
        self.panes
            .values()
            .filter(|overlay| overlay.fullscreen.is_some())
            .flat_map(OverlayPane::pane_ids)
            .collect()
    }
    fn may_be_fullscreen(&self, pane_id: &PaneId) -> bool {
        self.overlay(pane_id).is_some_and(|overlay| {
            !overlay.kind.is_modal() && !overlay.carry.hidden && !overlay.carry.awaiting_placement
        })
    }
    fn set_fullscreen(&mut self, pane_id: PaneId, covers_ui: bool) -> bool {
        if !self.may_be_fullscreen(&pane_id) {
            return false;
        }
        if let Some(overlay) = self.overlay_mut(&pane_id) {
            overlay.fullscreen = Some(covers_ui);
        }
        self.pane_being_moved_with_mouse = None;
        self.seam_being_dragged = None;
        self.raise_within_class(pane_id);
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        true
    }
    pub fn unset_fullscreen(&mut self) -> bool {
        let changed = self.fullscreen_is_active();
        for overlay in self.panes.values_mut() {
            overlay.fullscreen = None;
        }
        self.seam_being_dragged = None;
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        changed
    }
    fn toggle_fullscreen_of(&mut self, pane_id: PaneId, covers_ui: bool) -> bool {
        let same = self
            .overlay(&pane_id)
            .is_some_and(|overlay| overlay.fullscreen == Some(covers_ui));
        if same {
            return self.unset_fullscreen();
        }
        if !self.may_be_fullscreen(&pane_id) {
            self.unset_fullscreen();
            return false;
        }
        for overlay in self.panes.values_mut() {
            overlay.fullscreen = None;
        }
        self.set_fullscreen(pane_id, covers_ui)
    }
    fn fullscreen_area(&self, covers_ui: bool) -> Viewport {
        if covers_ui {
            self.surface()
        } else {
            *self.viewport.borrow()
        }
    }
    pub fn toggle_pane_fullscreen(&mut self, pane_id: PaneId) -> bool {
        self.toggle_fullscreen_of(pane_id, false)
    }
    pub fn toggle_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) -> bool {
        self.toggle_fullscreen_of(pane_id, true)
    }
    pub fn toggle_active_pane_fullscreen(&mut self, client_id: ClientId) -> bool {
        match self.fullscreen_target(client_id) {
            Some(pane_id) => self.toggle_pane_fullscreen(pane_id),
            None => false,
        }
    }
    pub fn toggle_active_pane_no_ui_fullscreen(&mut self, client_id: ClientId) -> bool {
        match self.fullscreen_target(client_id) {
            Some(pane_id) => self.toggle_pane_no_ui_fullscreen(pane_id),
            None => false,
        }
    }
    fn fullscreen_target(&self, client_id: ClientId) -> Option<PaneId> {
        self.fullscreen_pane_id()
            .or_else(|| self.active_pane_id(client_id))
    }
    pub fn set_pane_being_moved_with_mouse(&mut self, pane_id: PaneId, last_position: Position) {
        if let Some((last_pane_id, initial_position)) = self
            .pane_being_moved_with_mouse
            .map(|(pane_id, initial_position, _)| (pane_id, initial_position))
            && last_pane_id == pane_id
        {
            self.pane_being_moved_with_mouse = Some((pane_id, initial_position, last_position));
            return;
        }
        self.pane_being_moved_with_mouse = Some((pane_id, last_position, last_position));
    }
    pub fn pane_is_being_moved_with_mouse(&self) -> bool {
        self.pane_being_moved_with_mouse.is_some()
    }
    pub fn stop_moving_pane_with_mouse(&mut self, position: Position) -> bool {
        let mut never_moved = false;
        if let Some((pane_id, initial_position, last_position)) =
            self.pane_being_moved_with_mouse.take()
        {
            if initial_position == position {
                never_moved = true;
            } else {
                self.pane_being_moved_with_mouse = Some((pane_id, initial_position, last_position));
                self.move_pane_to_position(&position);
                self.pane_being_moved_with_mouse = None;
            }
            self.set_force_render();
        }
        self.pane_being_moved_with_mouse = None;
        never_moved
    }

    pub fn move_pane(&mut self, pane_id: PaneId, direction: yinz_types::geometry::Direction) {
        if self.pane_is_fullscreen(&pane_id) {
            return;
        }
        let (dx, dy) = match direction {
            yinz_types::geometry::Direction::Left => (-10, 0),
            yinz_types::geometry::Direction::Right => (10, 0),
            yinz_types::geometry::Direction::Up => (0, -5),
            yinz_types::geometry::Direction::Down => (0, 5),
        };
        self.move_pane_by(pane_id, dx, dy);
    }
    fn move_pane_by(&mut self, pane_id: PaneId, dx: isize, dy: isize) {
        let Some(displayed) = self.outer_geom(pane_id) else {
            return;
        };
        let moved = geom_moved_within(displayed, self.surface(), dx, dy);
        let Some(overlay) = self.overlay_mut(&pane_id) else {
            return;
        };
        // Movement owns the displayed origin; requested dimensions remain available when uncapped.
        overlay.geom.x = moved.x;
        overlay.geom.y = moved.y;
        overlay.coordinates.x = Some(PercentOrFixed::Fixed(moved.x));
        overlay.coordinates.y = Some(PercentOrFixed::Fixed(moved.y));
        overlay.carry.position_is_user_stated = true;
        overlay.carry.width_is_anchored = true;
        self.set_pane_frames().non_fatal();
        self.set_force_render();
    }
    pub fn move_pane_to_position(&mut self, position: &Position) -> bool {
        let Some((pane_id, _, previous)) = self.pane_being_moved_with_mouse else {
            return false;
        };
        if self.pane_is_fullscreen(&pane_id) || *position == previous {
            return false;
        }
        self.move_pane_by(
            pane_id,
            position.column() as isize - previous.column() as isize,
            position.line() - previous.line(),
        );
        self.set_pane_being_moved_with_mouse(pane_id, *position);
        true
    }
    pub fn position_is_on_frame(&self, pane_id: PaneId, position: &Position) -> bool {
        let Some(overlay) = self.overlay(&pane_id) else {
            return false;
        };
        if !overlay.framed() {
            return false;
        }
        let geom = if overlay.children.as_slice().len() == 1 {
            overlay.children.as_slice()[0].1.render_geom()
        } else {
            self.current_geom(overlay)
        };
        let x = position.column();
        let y = position.line();
        contains(geom, position)
            && (x == geom.x
                || x + 1 == geom.x + geom.cols.as_usize()
                || y == geom.y as isize
                || y + 1 == (geom.y + geom.rows.as_usize()) as isize)
    }
    pub fn move_pane_with_mouse(&mut self, position: Position) -> bool {
        if self.pane_is_being_moved_with_mouse() {
            return self.move_pane_to_position(&position);
        }
        let Some(id) = self.visible_pane_id_at(&position) else {
            return false;
        };
        if self.pane_is_fullscreen(&id) || !self.position_is_on_frame(id, &position) {
            return false;
        }
        self.set_pane_being_moved_with_mouse(id, position);
        true
    }
    /// The other half of `pane_id`'s multipane when it lies in `direction` from `pane_id`: right
    /// of (or below) the first child, left of (or above) the second. `None` for a single overlay,
    /// a direction across the split rather than along it, or one pointing out of the container.
    pub fn sibling_toward(
        &self,
        pane_id: &PaneId,
        direction: yinz_types::geometry::Direction,
    ) -> Option<PaneId> {
        use yinz_types::geometry::Direction;
        let Children::Multipane {
            children: [(first, _), (second, _)],
            split,
            ..
        } = &self.overlay(pane_id)?.children
        else {
            return None;
        };
        let (toward_second, toward_first) = match split.direction {
            MultipaneDirection::LeftRight => (Direction::Right, Direction::Left),
            MultipaneDirection::TopBottom => (Direction::Down, Direction::Up),
        };
        if pane_id == first && direction == toward_second {
            Some(*second)
        } else if pane_id == second && direction == toward_first {
            Some(*first)
        } else {
            None
        }
    }
    pub fn multipane_children(&self, pane_id: &PaneId) -> Option<(PaneId, PaneId)> {
        self.multipane_info(pane_id)
            .map(|info| (info.children[0], info.children[1]))
    }
    pub fn seam_at(&self, position: &Position) -> Option<(PaneId, PaneId)> {
        for id in self.visible_containers().iter().rev() {
            let overlay = &self.panes[id];
            let outer = self.current_geom(overlay);
            if !contains(outer, position) {
                continue;
            }
            let Children::Multipane { children, .. } = &overlay.children else {
                return None;
            };
            let (_, divider) = overlay.constrained_geometry(outer).1?;
            return contains(divider, position).then_some((children[0].0, children[1].0));
        }
        None
    }
    pub fn start_seam_drag(&mut self, left: PaneId, right: PaneId, _position: Position) {
        if self.multipane_children(&left) == Some((left, right)) {
            self.seam_being_dragged = Some((left, right));
        }
    }
    pub fn seam_is_being_dragged(&self) -> bool {
        self.seam_being_dragged.is_some()
    }
    pub fn drag_seam_to(&mut self, position: &Position) -> bool {
        let Some((first, _)) = self.seam_being_dragged else {
            return false;
        };
        let Some(outer) = self.outer_geom(first) else {
            return false;
        };
        let Some(overlay) = self.overlay_mut(&first) else {
            return false;
        };
        let Some((_, divider)) = overlay.constrained_geometry(outer).1 else {
            return false;
        };
        let framed = overlay.framed();
        let Children::Multipane {
            children,
            split,
            min_cells,
        } = &mut overlay.children
        else {
            return false;
        };
        let horizontal = split.direction == MultipaneDirection::LeftRight;
        let length = if horizontal {
            outer.cols.as_usize()
        } else {
            outer.rows.as_usize()
        };
        let available = length.saturating_sub(2 * usize::from(framed) + 1);
        if available < 2 {
            return false;
        }
        let start = if horizontal { outer.x } else { outer.y };
        let pointer = if horizontal {
            position.column() as isize
        } else {
            position.line()
        };
        let drawn = if horizontal { divider.x } else { divider.y };
        // Preserve the requested ratio when a cap or minimum size holds the divider in place.
        if pointer == drawn as isize {
            return false;
        }
        // the same floor `MultipaneSplit::geometry` keeps, so a drag past it stops where the
        // divider is drawn rather than storing a ratio the geometry would only clamp back
        let floor = MultipaneSplit::floor(available, *min_cells);
        let mut first_size = (pointer - (start + usize::from(framed)) as isize)
            .clamp(floor as isize, (available - floor) as isize)
            as usize;
        if horizontal {
            let limits = children
                .each_ref()
                .map(|(_, pane)| match pane.max_content_width() {
                    0 => usize::MAX,
                    limit => limit,
                });
            first_size = first_size.clamp(
                available.saturating_sub(limits[1]),
                limits[0].min(available),
            );
        }
        if first_size == drawn.saturating_sub(start + usize::from(framed)) {
            return false;
        }
        let ratio = (first_size * 10_000).div_ceil(available).clamp(1, 9999) as u16;
        if split.ratio == ratio {
            return false;
        }
        split.ratio = ratio;
        self.set_pane_frames().non_fatal();
        self.set_force_render();
        true
    }
    fn release_seam_drag_of(&mut self, pane_id: PaneId) {
        if matches!(self.seam_being_dragged, Some((left, right)) if left == pane_id || right == pane_id)
        {
            self.seam_being_dragged = None;
        }
    }
    pub fn stop_seam_drag(&mut self, position: Position) {
        self.drag_seam_to(&position);
        self.seam_being_dragged = None;
    }

    pub fn geoms_outside_viewport(&self) -> HashMap<PaneId, PaneGeom> {
        let viewport = *self.viewport.borrow();
        self.visible_containers()
            .into_iter()
            .filter_map(|id| {
                let overlay = &self.panes[&id];
                let geom = self.current_geom(overlay);
                geom_is_outside_viewport(&geom, &viewport)
                    .then_some((overlay.children.as_slice()[0].0, geom))
            })
            .collect()
    }
    pub fn has_panes_outside_viewport(&self) -> bool {
        !self.geoms_outside_viewport().is_empty()
    }
    pub fn update_agent_theme_mapping(
        &mut self,
        mapping: yinz_types::agent::theme::AgentThemeMapping,
    ) {
        self.style.agent_theme_mapping = mapping;
        for pane in self.get_panes_mut().map(|(_, pane)| pane) {
            pane.update_agent_theme_mapping(mapping);
        }
    }
    pub fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>) {
        self.style.terminal = theme;
        for (_, pane) in self.get_panes_mut() {
            pane.update_terminal_theme(theme);
        }
    }
    pub fn update_pane_themes(&mut self, theme: Styling) {
        self.style.colors = theme;
        for (_, pane) in self.get_panes_mut() {
            pane.update_theme(theme);
        }
    }
    pub fn update_pane_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        for (_, pane) in self.get_panes_mut() {
            pane.update_arrow_fonts(should_support_arrow_fonts);
        }
    }
    pub fn update_pane_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool) {
        for (_, pane) in self.get_panes_mut() {
            pane.update_render_options(styled_underlines, osc8_hyperlinks);
        }
    }
    pub fn update_pane_kitty_keyboard_protocol_disabled(&mut self, disabled: bool) {
        for (_, pane) in self.get_panes_mut() {
            pane.update_kitty_keyboard_protocol_disabled(disabled);
        }
    }
    pub fn set_session_is_mirrored(&mut self, session_is_mirrored: bool) {
        self.session_is_mirrored = session_is_mirrored;
    }
    pub fn update_pane_kitty_host_support(&mut self, supported: KittyHostSupport) {
        for (_, pane) in self.get_panes_mut() {
            pane.update_kitty_host_support(supported);
        }
    }
    pub fn update_pane_sixel_host_support(&mut self, supported: bool) {
        for (_, pane) in self.get_panes_mut() {
            pane.update_sixel_host_support(supported);
        }
    }
    pub fn update_pane_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        for (_, pane) in self.get_panes_mut() {
            pane.update_rounded_corners(rounded_corners);
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
    pub fn set_force_render(&mut self) {
        for (_, pane) in self.get_panes_mut() {
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
    }
    pub fn force_render_panes_intersecting(&mut self, damage: &[CellRect]) {
        for (_, pane) in self.get_panes_mut() {
            let geom = pane.render_geom();
            if !damage.iter().any(|rect| rect.intersects_geom(&geom)) {
                continue;
            }
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            pane.render_full_viewport();
        }
    }
    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        let senders = self.senders.clone();
        let character_cell_size = self.character_cell_size.clone();
        for (_, pane) in self.get_panes_mut() {
            if pane.current_geom().cols.as_usize() == 0 || pane.current_geom().rows.as_usize() == 0
            {
                continue;
            }
            pane.character_cell_size_possibly_changed();
            resize_pty!(pane, os_api, senders, character_cell_size)
                .with_context(|| format!("failed to resize PTY in overlay {:?}", pane.pid()))?;
        }
        Ok(())
    }

    pub fn set_pane_frames(&mut self) -> Result<()> {
        let surface = self.surface();
        let viewport = *self.viewport.borrow();
        for overlay in self.panes.values_mut() {
            let borderless = overlay.borderless();
            let framed = overlay.framed();
            let outer = overlay
                .fullscreen
                .map(|ui| viewport_geom(if ui { surface } else { viewport }))
                .unwrap_or(overlay.geom);
            let split = match overlay.children {
                Children::Multipane {
                    split, min_cells, ..
                } => Some((split, min_cells)),
                _ => None,
            };
            let base = split
                .map(|(split, min_cells)| split.geometry(overlay.geom, !borderless, min_cells).0);
            let displayed =
                split.map(|(split, min_cells)| split.geometry(outer, framed, min_cells).0);
            for (index, (_, pane)) in overlay.children.as_mut_slice().iter_mut().enumerate() {
                let geom = base.map(|geoms| geoms[index]).unwrap_or(overlay.geom);
                let geom_override = overlay
                    .fullscreen
                    .map(|_| displayed.map(|geoms| geoms[index]).unwrap_or(outer));
                pane.set_layout(
                    geom,
                    geom_override,
                    if split.is_some() { !framed } else { borderless },
                    if framed && split.is_none() {
                        Offset::frame(1)
                    } else {
                        Offset::default()
                    },
                );
                pane.set_frame(framed && split.is_none());
                let current = pane.current_geom();
                if current.cols.as_usize() > 0 && current.rows.as_usize() > 0 {
                    resize_pty!(pane, os_api, self.senders, self.character_cell_size)
                        .context("failed to resize overlay child")?;
                }
            }
        }
        Ok(())
    }
    pub fn visible_pane_id_at(&self, position: &Position) -> Option<PaneId> {
        for id in self.visible_containers().iter().rev() {
            let overlay = &self.panes[id];
            let hit_area = overlay
                .fullscreen
                .map(|ui| viewport_geom(self.fullscreen_area(ui)))
                .unwrap_or_else(|| self.current_geom(overlay));
            if !contains(hit_area, position) {
                continue;
            }
            return Some(
                overlay
                    .children
                    .as_slice()
                    .iter()
                    .find(|(_, pane)| pane.contains(position))
                    .map(|(id, _)| *id)
                    .unwrap_or(overlay.children.as_slice()[0].0),
            );
        }
        None
    }
    pub fn get_pane_id_at(&self, position: &Position) -> Option<PaneId> {
        self.visible_pane_id_at(position)
    }

    pub fn sample_opening_ripples(&mut self, now: Instant, enabled: bool) {
        for id in self.visible_containers() {
            let geom = self.current_geom(&self.panes[&id]);
            let overlay = self.panes.get_mut(&id).expect("visible container");
            if overlay.carry.opening_awaits_placement {
                continue;
            }
            let eligible = enabled
                && overlay.framed()
                && overlay.children.as_slice().iter().any(|(_, pane)| {
                    matches!(pane.invoked_with(), Some(Run::Plugin(plugin))
                        if plugin.presentation().opening_ripple)
                });
            if !eligible {
                overlay.carry.opening_started = None;
            } else if !overlay.carry.opening_seen {
                overlay.carry.opening_started = Some(now);
            }
            overlay.carry.opening_seen = true;
            overlay.ripple = overlay
                .carry
                .opening_started
                .and_then(|start| OpeningRipple::sample(start, now, geom.x, geom.cols.as_usize()));
        }
    }
    pub fn settle_opening_ripples(&mut self) {
        for overlay in self.panes.values_mut() {
            overlay.carry.opening_started = None;
        }
    }
    pub fn opening_ripple_is_live(&self) -> bool {
        self.panes.values().any(|overlay| overlay.ripple.is_some())
    }
    pub fn render(
        &mut self,
        output: &mut Output,
        mouse_hover_pane_id: &HashMap<ClientId, PaneId>,
        current_pane_group: HashMap<ClientId, Vec<PaneId>>,
        client_id_override: Option<ClientId>,
        help_text_visible: &HashMap<ClientId, bool>,
        mouse_scroll_resize: bool,
        z_index_offset: usize,
        same_worktree_panes: &HashMap<ClientId, HashSet<PaneId>>,
    ) -> Result<()> {
        let err_context = || "failed to render overlays";

        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }
        let connected_clients: Vec<ClientId> = connected_clients.into_iter().collect();

        let mut active_panes = self.active_panes.clone_active_panes();

        for (pane_id, pane) in self.get_panes().filter(|(id, _)| {
            self.overlay(id)
                .is_some_and(|overlay| !overlay.carry.hidden && !overlay.carry.awaiting_placement)
        }) {
            match pane_id {
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

        let multiplayer = self.connected_clients_in_app.borrow().snapshot();
        let style = self.style;
        let session_is_mirrored = self.session_is_mirrored;
        let dimmed_clients = self.dimmed_clients.clone();
        let default_mode_info = self.default_mode_info.clone();
        let client_modes: HashMap<ClientId, _> = connected_clients
            .iter()
            .map(|client_id| {
                let mode_info = self.mode_info.borrow();
                let mode = mode_info.get(client_id).unwrap_or(&default_mode_info);
                (*client_id, (mode.mode, mode.session_name.clone()))
            })
            .collect();
        active_panes.retain(|c_id, _| self.connected_clients.borrow().contains(c_id));

        let fullscreen: HashSet<PaneId> = self.fullscreen_pane_ids().into_iter().collect();
        for container_id in self.visible_containers() {
            let outer = self.current_geom(&self.panes[&container_id]);
            let index = self
                .visible_containers()
                .iter()
                .position(|id| *id == container_id)
                .unwrap_or(0);
            let overlay = self
                .panes
                .get_mut(&container_id)
                .context("missing overlay")?;
            let kind = overlay.kind;
            let multipane = matches!(overlay.children, Children::Multipane { .. });
            let first_id = overlay.children.as_slice()[0].0;
            let framed = overlay.framed();
            let divider = match overlay.children {
                Children::Multipane { split, .. } => overlay
                    .constrained_geometry(outer)
                    .1
                    .map(|(_, divider)| (divider, split.direction)),
                _ => None,
            };
            let overlay_ids: Vec<_> = overlay.pane_ids().collect();
            for (pane_id, pane) in overlay.children.as_mut_slice() {
                let pane_id = *pane_id;
                let geom = pane.current_geom();
                let has_content = geom.cols.as_usize() > 0 && geom.rows.as_usize() > 0;
                let pane_is_selectable = pane.selectable();
                let pane_has_guest_modal = pane.has_guest_modal_for_any_client();
                let z_index = Some(z_index_offset + index + 1);
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
                    multiplayer,
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
                pane_contents_and_ui.set_opening_ripple(overlay.ripple);
                for client_id in &connected_clients {
                    pane_contents_and_ui.set_same_worktree(
                        same_worktree_panes
                            .get(client_id)
                            .is_some_and(|panes| panes.contains(&pane_id)),
                    );
                    let (client_mode, session_name) = client_modes
                        .get(client_id)
                        .map(|(mode, session)| (*mode, session.as_ref()))
                        .unwrap_or((
                            default_mode_info.mode,
                            default_mode_info.session_name.as_ref(),
                        ));
                    let is_floating = true;
                    if outer.cols.as_usize() > 0
                        && outer.rows.as_usize() > 0
                        && (!multipane
                            || (framed
                                && active_panes
                                    .get(client_id)
                                    .copied()
                                    .filter(|id| overlay_ids.contains(id))
                                    .unwrap_or(first_id)
                                    == pane_id))
                    {
                        pane_contents_and_ui.set_frame_geom_override(multipane.then_some(outer));
                        pane_contents_and_ui
                            .render_pane_frame(
                                *client_id,
                                client_mode,
                                session_is_mirrored,
                                is_floating,
                                pane_is_selectable,
                                false,
                                !kind.is_modal(),
                            )
                            .with_context(err_context)?;
                    }
                    if !has_content {
                        continue;
                    }
                    if let PaneId::Plugin(..) = pane_id {
                        pane_contents_and_ui
                            .render_pane_contents_for_client(*client_id)
                            .with_context(err_context)?;
                    }
                    pane_contents_and_ui.render_terminal_title_if_needed(
                        *client_id,
                        client_mode,
                        session_name,
                        &mut self.window_title,
                    );
                    pane_contents_and_ui
                        .render_fake_cursor_if_needed(*client_id)
                        .with_context(err_context)?;
                }
                if !has_content {
                    continue;
                }
                if let PaneId::Terminal(..) = pane_id {
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
            if let Some((divider, direction)) = divider {
                use crate::output::CharacterChunk;
                use crate::ui::boundaries::{boundary_type, BoundarySymbol};
                let symbol = BoundarySymbol::new(if direction == MultipaneDirection::LeftRight {
                    boundary_type::VERTICAL
                } else {
                    boundary_type::HORIZONTAL
                })
                .color(None, style.chrome_colors())
                .as_terminal_character()?;
                for y in divider.y..divider.y + divider.rows.as_usize() {
                    let chunk = CharacterChunk::new(
                        vec![symbol.clone(); divider.cols.as_usize()],
                        divider.x,
                        y,
                    );
                    output.add_character_chunks_to_multiple_clients(
                        vec![chunk],
                        connected_clients.iter().copied(),
                        Some(z_index_offset + index + 1),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn contains(geom: PaneGeom, position: &Position) -> bool {
    position.line() >= geom.y as isize
        && position.line() < (geom.y + geom.rows.as_usize()) as isize
        && position.column() >= geom.x
        && position.column() < geom.x + geom.cols.as_usize()
}
#[cfg(test)]
#[path = "./unit/overlay_panes_tests.rs"]
mod overlay_panes_tests;

fn viewport_geom(viewport: Viewport) -> PaneGeom {
    let mut geom = PaneGeom::from(&Size {
        rows: viewport.rows,
        cols: viewport.cols,
    });
    geom.x = viewport.x;
    geom.y = viewport.y;
    geom
}
