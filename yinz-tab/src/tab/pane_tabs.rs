use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use yinz_panes::pane_tabs::{bar_cells, PaneTabsTransfer, PaneTabsView};
use yinz_types::keys::BareKey;
use yinz_types::pane_tabs::*;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct PaneTabs {
    pub info: PaneTabsInfo,
    pub(super) views: HashMap<ClientId, PaneTabsView>,
    generations: Vec<u64>,
    visibility: HashMap<PluginId, bool>,
    last_geometry: Option<(PaneGeom, Offset, bool)>,
}

/// What `extract_pane` leaves on a pane with pane tabs, inside the pane layer's opaque
/// [`PaneTabsTransfer`].
pub(super) struct PaneTabsCarry {
    pub(super) tabs: PaneTabs,
    pub(super) children: SuppressedPanes,
}

pub(super) fn take_carry(pane: &mut Box<dyn Pane>) -> Option<PaneTabsCarry> {
    pane.take_pane_tabs_transfer()?.into_contents()
}

impl Tab {
    pub fn reserve_pane_tabs_id() -> PaneTabsId {
        PaneTabsId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn mark_overlay_tab_group(&mut self, id: PaneTabsId) -> Result<()> {
        self.pane_tabs
            .get_mut(&id)
            .context("unknown overlay pane tabs")?
            .info
            .overlay_group = true;
        self.refresh_pane_tabs(id)
    }

    pub(super) fn retain_pane_tabs_transfer(&mut self, pane: &mut Box<dyn Pane>) {
        if let Some(transfer) = take_carry(pane) {
            self.pane_tabs.insert(transfer.tabs.info.id, transfer.tabs);
            self.suppressed_panes.extend(transfer.children);
        }
    }

    pub(super) fn attach_pane_tabs_transfer(&mut self, pane: &mut Box<dyn Pane>) {
        if let Some(tabs) = self
            .pane_tabs_of(pane.pid())
            .and_then(|id| self.pane_tabs.remove(&id))
        {
            let children = tabs
                .info
                .tabs
                .iter()
                .filter_map(PaneTabInfo::pane_id)
                .filter_map(|id| self.suppressed_panes.remove(&id).map(|child| (id, child)))
                .collect();
            pane.set_pane_tabs_transfer(PaneTabsTransfer::new(PaneTabsCarry { tabs, children }));
        }
    }

    pub(super) fn update_pane_tab_label(&mut self, pane: PaneId) -> Result<()> {
        if matches!(pane, PaneId::Terminal(_)) {
            return Ok(());
        }
        let Some(id) = self.pane_tabs_of(pane) else {
            return Ok(());
        };
        if self
            .pane_tabs_info(id)
            .is_some_and(|info| info.overlay_group)
        {
            return Ok(());
        }
        let title = self
            .get_pane_with_id(pane)
            .context("pane tab disappeared")?
            .current_title();
        if let Some(slot) = self.pane_tabs.get_mut(&id).and_then(|tabs| {
            tabs.info
                .tabs
                .iter_mut()
                .find(|slot| slot.pane_id() == Some(pane))
        }) {
            slot.descriptor.label = title;
        }
        self.refresh_pane_tabs(id)
    }

    pub fn pane_tabs_info(&self, id: PaneTabsId) -> Option<PaneTabsInfo> {
        self.pane_tabs.get(&id).map(|tabs| tabs.info.clone())
    }

    pub fn pane_tabs_of(&self, pane: PaneId) -> Option<PaneTabsId> {
        self.pane_tabs.iter().find_map(|(id, tabs)| {
            (tabs.info.pane_id == pane
                || tabs
                    .info
                    .tabs
                    .iter()
                    .any(|slot| slot.pane_id() == Some(pane)))
            .then_some(*id)
        })
    }

    pub fn mount_pane_tabs(
        &mut self,
        pane: PaneId,
        layout: PaneTabsLayout,
        existing: usize,
        client: ClientId,
    ) -> Result<PaneTabsInfo> {
        layout.validate().map_err(|error| anyhow!(error))?;
        let plugin_id = match pane {
            PaneId::Plugin(id) => Some(id),
            _ => None,
        };
        let terminal_id = match pane {
            PaneId::Terminal(id) => Some(id),
            _ => None,
        };
        if existing >= layout.tabs.len()
            || self.pane_tabs_of(pane).is_some()
            || self.pane_is_stack_list_member(&pane)
            || self.stands_in_for_a_pane(pane).is_some()
        {
            return Err(anyhow!("invalid existing pane tab slot"));
        }
        self.get_pane_with_id(pane)
            .context("pane tabs mount pane not found")?;
        for descriptor in &layout.tabs {
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &descriptor.location,
                &Some(descriptor.configuration.clone()),
                None,
                descriptor.initial_cwd.clone(),
            )
            .map_err(|error| anyhow!(error))?;
        }
        let id = Self::reserve_pane_tabs_id();
        let info = PaneTabsInfo {
            overlay_group: false,
            id,
            pane_id: pane,
            active: existing,
            bar_focused: vec![],
            tabs: layout
                .tabs
                .into_iter()
                .enumerate()
                .map(|(index, descriptor)| PaneTabInfo {
                    descriptor,
                    plugin_id: (index == existing).then_some(plugin_id).flatten(),
                    terminal_id: (index == existing).then_some(terminal_id).flatten(),
                    state: if index == existing {
                        PaneTabLoadState::Ready
                    } else {
                        PaneTabLoadState::Unloaded
                    },
                })
                .collect(),
        };
        self.pane_tabs.insert(
            id,
            PaneTabs {
                views: HashMap::new(),
                last_geometry: None,
                visibility: HashMap::new(),
                generations: vec![0; info.tabs.len()],
                info,
            },
        );
        self.select_pane_tab(id, layout.active, client)?;
        self.pane_tabs_info(id)
            .context("mounted pane tabs disappeared")
    }

    pub fn select_pane_tab(
        &mut self,
        id: PaneTabsId,
        index: usize,
        client: ClientId,
    ) -> Result<()> {
        let info = self.pane_tabs_info(id).context("unknown pane tabs")?;
        if index != info.active
            && info
                .tabs
                .get(info.active)
                .is_some_and(|slot| slot.terminal_id.is_some())
            && self.connected_clients.borrow().iter().any(|other| {
                *other != client && self.get_active_pane_id(*other) == Some(info.pane_id)
            })
        {
            return Ok(());
        }
        let tabs = self.pane_tabs.get_mut(&id).context("unknown pane tabs")?;
        let slot = tabs
            .info
            .tabs
            .get(index)
            .context("invalid pane tab index")?;
        let target = slot.pane_id();
        tabs.info.active = index;
        // Activation is shared; other clients keep their own highlight and overflow window.
        if let Some(view) = tabs.views.get_mut(&client) {
            view.reveal = None;
        }
        if let Some(target) = target {
            self.reveal_pane_tab(id, target)?;
        } else if !matches!(slot.state, PaneTabLoadState::Loading) {
            self.load_pane_tab(id, index, client)?;
        }
        self.refresh_pane_tabs(id)?;
        Ok(())
    }

    fn load_pane_tab(&mut self, id: PaneTabsId, index: usize, client: ClientId) -> Result<()> {
        let tabs = self.pane_tabs.get_mut(&id).context("unknown pane tabs")?;
        let slot = tabs
            .info
            .tabs
            .get_mut(index)
            .context("invalid pane tab index")?;
        tabs.generations[index] += 1;
        slot.state = PaneTabLoadState::Loading;
        let generation = tabs.generations[index];
        let descriptor = slot.descriptor.clone();
        let pane = tabs.info.pane_id;
        let size = self
            .get_pane_with_id(pane)
            .map(|pane| Size {
                rows: pane.get_content_rows(),
                cols: pane.get_content_columns(),
            })
            .unwrap_or(Size { rows: 1, cols: 1 });
        self.senders
            .send_to_plugin(PluginInstruction::LoadPaneTab {
                skip_cache: false,
                id,
                index,
                generation,
                descriptor,
                client_id: client,
                tab_id: self.id,
                size,
            })
            .context("failed to load pane tab")
    }

    pub fn pane_tab_load_state(&mut self, plugin: PluginId, state: PaneTabLoadState) -> Result<()> {
        if let Some(id) = self.pane_tabs_of(PaneId::Plugin(plugin)) {
            let tabs = self
                .pane_tabs
                .get_mut(&id)
                .context("pane tabs disappeared")?;
            if let Some(slot) = tabs
                .info
                .tabs
                .iter_mut()
                .find(|slot| slot.plugin_id == Some(plugin))
                && slot.state != state
            {
                slot.state = state;
                self.refresh_pane_tabs(id)?;
            }
        }
        Ok(())
    }

    pub fn pane_tab_loaded(
        &mut self,
        id: PaneTabsId,
        index: usize,
        generation: u64,
        result: std::result::Result<(PluginId, RunPluginOrAlias), String>,
    ) -> Result<()> {
        let valid = self
            .pane_tabs
            .get(&id)
            .is_some_and(|tabs| tabs.generations.get(index) == Some(&generation));
        if !valid {
            if let Ok((plugin_id, _)) = result {
                self.senders
                    .send_to_plugin(PluginInstruction::Unload(plugin_id))
                    .context("failed to unload stale pane tab")?;
            }
            return Ok(());
        }
        match result {
            Ok((plugin_id, run)) => {
                let pane = self.build_pane(
                    PaneId::Plugin(plugin_id),
                    None,
                    Some(Run::Plugin(run)),
                    None,
                )?;
                self.park_pane(pane);
                let tabs = self
                    .pane_tabs
                    .get_mut(&id)
                    .context("pane tabs disappeared")?;
                tabs.info.tabs[index].plugin_id = Some(plugin_id);
                if tabs.info.active == index {
                    self.reveal_pane_tab(id, PaneId::Plugin(plugin_id))?;
                }
            },
            Err(error) => {
                self.pane_tabs
                    .get_mut(&id)
                    .context("pane tabs disappeared")?
                    .info
                    .tabs[index]
                    .state = PaneTabLoadState::Failed(error);
            },
        }
        self.refresh_pane_tabs(id)
    }

    fn reveal_pane_tab(&mut self, id: PaneTabsId, target: PaneId) -> Result<()> {
        let current = self
            .pane_tabs
            .get(&id)
            .context("unknown pane tabs")?
            .info
            .pane_id;
        if current == target {
            return Ok(());
        }
        let (geom, offset, borderless) = self
            .get_pane_with_id(current)
            .map(|pane| {
                (
                    pane.current_geom(),
                    pane.get_content_offset(),
                    pane.borderless(),
                )
            })
            .context("pane tabs content disappeared")?;
        let mut child = self
            .suppressed_panes
            .remove(&target)
            .context("pane tab is not retained")?
            .pane;
        child.set_geom(geom);
        child.set_borderless_with_content_offset(borderless, offset);
        let previous = if self.overlay_panes.panes_contain(&current) {
            self.overlay_panes
                .replace_pane(current, child)
                .context("failed to replace overlay tab")?
        } else if self.floating_panes.panes_contain(&current) {
            self.floating_panes
                .replace_pane(current, child)
                .context("failed to replace floating tab")?
        } else if self.suppressed_panes.contains_key(&current) {
            self.suppressed_panes
                .insert(target, SuppressedPane::parked(child));
            self.suppressed_panes
                .remove(&current)
                .context("hidden pane tabs disappeared")?
                .pane
        } else {
            self.tiled_panes
                .replace_pane(current, child)
                .context("failed to replace tiled tab")?
        };
        if self.pane_tabs[&id]
            .info
            .tabs
            .iter()
            .any(|slot| slot.pane_id() == Some(current))
        {
            self.park_pane(previous);
        }
        self.pane_tabs
            .get_mut(&id)
            .context("pane tabs disappeared")?
            .info
            .pane_id = target;
        self.set_force_render();
        Ok(())
    }

    pub fn focus_pane_tabs(&mut self, id: PaneTabsId, bar: bool, client: ClientId) -> Result<()> {
        let tabs = self.pane_tabs.get_mut(&id).context("unknown pane tabs")?;
        tabs.info.bar_focused.retain(|c| *c != client);
        if bar {
            tabs.views.insert(
                client,
                PaneTabsView {
                    highlight: tabs.info.active,
                    reveal: None,
                },
            );
            tabs.info.bar_focused.push(client);
            tabs.info.bar_focused.sort();
        }
        let pane = tabs.info.pane_id;
        self.focus_pane_with_id(pane, false, false, client)
            .context("failed to focus pane tabs")?;
        self.refresh_pane_tabs(id)
    }

    pub fn close_pane_tab_child(&mut self, id: PaneTabsId, index: usize) -> Result<()> {
        self.release_pane_tab_child(id, index, true)
    }

    /// Takes `plugin` out of the pane tab that shows it, as closing that pane tab does, but
    /// leaves the plugin running: it runs headless from now on. Answers whether a pane tab
    /// held it.
    pub fn detach_pane_tab_plugin(&mut self, plugin: PluginId) -> bool {
        let pane_id = PaneId::Plugin(plugin);
        let Some((id, index)) = self.pane_tabs.iter().find_map(|(id, tabs)| {
            tabs.info
                .tabs
                .iter()
                .position(|slot| slot.pane_id() == Some(pane_id))
                .map(|index| (*id, index))
        }) else {
            return false;
        };
        self.release_pane_tab_child(id, index, false)
            .context("failed to take a headless plugin out of its pane tab")
            .non_fatal();
        true
    }

    /// Empties pane tab `index`, unloading the plugin it showed when `unload` says so.
    fn release_pane_tab_child(&mut self, id: PaneTabsId, index: usize, unload: bool) -> Result<()> {
        let tabs = self.pane_tabs.get_mut(&id).context("unknown pane tabs")?;
        let slot = tabs
            .info
            .tabs
            .get_mut(index)
            .context("invalid pane tab index")?;
        if slot.terminal_id.is_some() {
            bail!("close the containing pane to close its terminal tab");
        }
        tabs.generations[index] += 1;
        slot.state = PaneTabLoadState::Unloaded;
        if let Some(plugin) = slot.plugin_id.take() {
            // Keep the displayed pane as a blank slot until retry replaces it. It owns no guest.
            if PaneId::Plugin(plugin) != tabs.info.pane_id {
                self.suppressed_panes.remove(&PaneId::Plugin(plugin));
            }
            if unload {
                self.senders
                    .send_to_plugin(PluginInstruction::Unload(plugin))
                    .context("failed to unload pane tab")?;
            }
        }
        self.refresh_pane_tabs(id)
    }

    pub fn close_pane_tabs(&mut self, id: PaneTabsId) -> Result<()> {
        let pane = self
            .pane_tabs_info(id)
            .context("unknown pane tabs")?
            .pane_id;
        self.close_pane_explicitly(pane, None);
        Ok(())
    }

    pub fn pane_tabs_infos(&self) -> impl Iterator<Item = PaneTabsInfo> + '_ {
        self.pane_tabs.values().map(|tabs| tabs.info.clone())
    }

    pub(super) fn sync_pane_tabs(&mut self) -> Result<()> {
        let ids: Vec<_> = self
            .pane_tabs
            .iter()
            .filter_map(|(id, tabs)| {
                self.pane_tabs_geometry(tabs.info.pane_id)
                    .filter(|geometry| Some(*geometry) != tabs.last_geometry)
                    .map(|_| *id)
            })
            .collect();
        for id in ids {
            self.refresh_pane_tabs(id)?;
        }
        Ok(())
    }

    fn pane_tabs_geometry(&self, pane_id: PaneId) -> Option<(PaneGeom, Offset, bool)> {
        let pane = self.get_pane_with_id(pane_id)?;
        let visible = self.tab_visible
            && !self.suppressed_panes.contains_key(&pane_id)
            && (!self.floating_panes.panes_contain(&pane_id)
                || self.floating_panes.panes_are_visible())
            && self
                .get_pane_info(pane_id)
                .is_some_and(|pane| !pane.is_suppressed);
        Some((pane.current_geom(), pane.get_content_offset(), visible))
    }

    pub fn refresh_pane_tabs(&mut self, id: PaneTabsId) -> Result<()> {
        let info = self.pane_tabs_info(id).context("unknown pane tabs")?;
        let mut members: Vec<_> = info.tabs.iter().filter_map(|tab| tab.pane_id()).collect();
        if !members.contains(&info.pane_id) {
            members.push(info.pane_id);
        }
        let (geom, offset, visible) = self
            .pane_tabs_geometry(info.pane_id)
            .context("pane tabs content disappeared")?;
        self.pane_tabs
            .get_mut(&id)
            .context("pane tabs disappeared")?
            .last_geometry = Some((geom, offset, visible));
        let views = self.pane_tabs[&id].views.clone();
        for member in members {
            if let Some(pane) = self.get_pane_with_id_mut(member) {
                if pane.current_geom() != geom {
                    pane.set_geom(geom);
                }
                if pane.get_content_offset() != offset {
                    pane.set_content_offset(offset);
                }
                pane.set_pane_tabs(Some(info.clone()));
                pane.set_pane_tabs_views(views.clone());
            }
            if let Some(pane) = self.get_pane_with_id(member) {
                resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                    .context("failed to resize pane tab")?;
                if let PaneId::Plugin(plugin) = member {
                    let effective_visibility = visible
                        && member == info.pane_id
                        && info.tabs[info.active].plugin_id == Some(plugin);
                    let changed = self
                        .pane_tabs
                        .get_mut(&id)
                        .context("pane tabs disappeared")?
                        .visibility
                        .insert(plugin, effective_visibility)
                        != Some(effective_visibility);
                    if changed {
                        self.senders
                            .send_to_plugin(PluginInstruction::Update(vec![(
                                Some(plugin),
                                None,
                                Event::Visible(effective_visibility),
                            )]))
                            .context("failed to update pane tab visibility")?;
                    }
                }
            }
        }
        self.set_force_render();
        Ok(())
    }

    pub(super) fn pane_tabs_input(
        &mut self,
        pane: PaneId,
        key: &Option<KeyWithModifier>,
        client: Option<ClientId>,
    ) -> Result<bool> {
        // Guarded host input addresses the retained terminal, bypassing the tab's keyboard focus.
        if client.is_none() && matches!(pane, PaneId::Terminal(_)) {
            return Ok(false);
        }
        let Some(id) = self.pane_tabs_of(pane) else {
            return Ok(false);
        };
        let info = self.pane_tabs_info(id).context("pane tabs disappeared")?;
        let selected = &info.tabs[info.active];
        if pane != info.pane_id {
            return Ok(true);
        }
        let content_available = selected.pane_id() == Some(pane)
            && !matches!(selected.state, PaneTabLoadState::Failed(_));
        let Some(client) =
            client.filter(|client| info.bar_focused.contains(client) || !content_available)
        else {
            return Ok(!content_available);
        };
        // Participating children use reverse Tab to leave their enclosing focus ring.
        // Keep the bar focus in the event metadata so the child can distinguish this stop.
        if content_available
            && key.as_ref().is_some_and(|key| {
                key.bare_key == BareKey::Tab
                    && key.has_modifiers(&[yinz_types::keys::KeyModifier::Shift])
            })
        {
            return Ok(false);
        }
        if let Some(key) = key.as_ref().filter(|key| key.has_no_modifiers()) {
            match key.bare_key {
                BareKey::Left | BareKey::Right => {
                    let tabs = self
                        .pane_tabs
                        .get_mut(&id)
                        .context("pane tabs disappeared")?;
                    let view = tabs.views.entry(client).or_insert(PaneTabsView {
                        highlight: info.active,
                        reveal: None,
                    });
                    if !info.bar_focused.contains(&client) {
                        view.highlight = info.active;
                    }
                    view.highlight = if key.bare_key == BareKey::Left {
                        view.highlight.saturating_sub(1)
                    } else {
                        (view.highlight + 1).min(info.tabs.len() - 1)
                    };
                    view.reveal = None;
                    if !tabs.info.bar_focused.contains(&client) {
                        tabs.info.bar_focused.push(client);
                        tabs.info.bar_focused.sort();
                    }
                    self.refresh_pane_tabs(id)?;
                },
                BareKey::Enter => {
                    let index = self.pane_tabs[&id]
                        .views
                        .get(&client)
                        .filter(|_| info.bar_focused.contains(&client))
                        .map_or(info.active, |view| view.highlight);
                    self.select_pane_tab(id, index, client)?;
                    self.focus_pane_tabs(id, false, client)?;
                },
                BareKey::Esc | BareKey::Tab => self.focus_pane_tabs(id, false, client)?,
                BareKey::Char('r')
                    if matches!(
                        selected.state,
                        PaneTabLoadState::Failed(_) | PaneTabLoadState::Unloaded
                    ) =>
                {
                    self.close_pane_tab_child(id, info.active)?;
                    self.select_pane_tab(id, info.active, client)?;
                },
                _ => {},
            }
        }
        Ok(true)
    }

    pub(super) fn pane_tabs_mouse(&mut self, event: &MouseEvent, client: ClientId) -> Result<bool> {
        let Some(pane_id) =
            MouseHandler::get_pane_at(self, &event.position, false)?.map(|pane| pane.pid())
        else {
            return Ok(false);
        };
        let Some(id) = self.pane_tabs_of(pane_id) else {
            return Ok(false);
        };
        let pane = self
            .get_pane_with_id(pane_id)
            .context("pane tabs content disappeared")?;
        let y = pane.get_content_y().saturating_sub(1);
        let info = self.pane_tabs_info(id).context("pane tabs disappeared")?;
        if event.position.line() != y as isize {
            let selected = &info.tabs[info.active];
            let content_available = selected.pane_id() == Some(pane_id)
                && !matches!(selected.state, PaneTabLoadState::Failed(_));
            if content_available
                && event.event_type == yinz_types::mouse::MouseEventType::Press
                && (event.left || event.right || event.middle)
                && !pane.position_is_on_frame(&event.position)
                && info.bar_focused.contains(&client)
            {
                self.focus_pane_tabs(id, false, client)?;
            }
            return Ok(!content_available);
        }
        let Some(col) = event.position.column().checked_sub(pane.get_content_x()) else {
            return Ok(false);
        };
        if col >= pane.get_content_columns() {
            return Ok(false);
        }
        let view = self.pane_tabs[&id]
            .views
            .get(&client)
            .copied()
            .unwrap_or_default();
        let cells = bar_cells(
            &info,
            view,
            info.bar_focused.contains(&client),
            pane.get_content_columns(),
        );
        let target = cells
            .iter()
            .find(|cell| col >= cell.x && col < cell.x + cell.width)
            .map(|cell| cell.target);
        if event.left && event.event_type == yinz_types::mouse::MouseEventType::Press {
            match target {
                Some(yinz_types::tab_bar::TabTarget::Tab(index)) => {
                    self.select_pane_tab(id, index, client)?;
                    self.focus_pane_tabs(id, false, client)?;
                },
                Some(yinz_types::tab_bar::TabTarget::Reveal(index)) => {
                    self.pane_tabs
                        .get_mut(&id)
                        .context("pane tabs disappeared")?
                        .views
                        .entry(client)
                        .or_default()
                        .reveal = Some(index);
                    self.focus_pane_tabs(id, false, client)?;
                },
                None => {},
            }
        }
        Ok(true)
    }
}
