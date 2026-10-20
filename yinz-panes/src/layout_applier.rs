use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::multiplayer::AttachedClients;

use crate::pane::{get_next_terminal_position, HoldForCommand, Pane};
use crate::resize_pty;

use crate::{
    os_input_output::ServerOsApi,
    panes::kitty_graphics::KittyImageStore,
    panes::sixel::SixelImageStore,
    panes::{FloatingPanes, TiledPanes},
    panes::{LinkHandler, PaneId, PluginPane, TerminalPane},
    plugins::PluginInstruction,
    pty::PtyInstruction,
    thread_bus::ThreadSenders,
    ClientId, NotificationEnd,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use yinz_types::pane_size::{PaneGeom, Size, SizeInPixels, Viewport};
use yinz_types::style::{Palette, Style};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::layout::TiledPaneLayout,
    yinz_types::input::options::PaneFrameStyle,
};

pub struct LayoutApplier<'a> {
    viewport: Rc<RefCell<Viewport>>, // includes all non-UI panes
    senders: ThreadSenders,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    link_handler: Rc<RefCell<LinkHandler>>,
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    connected_clients: Rc<RefCell<AttachedClients>>,
    style: Style,
    display_area: Rc<RefCell<Size>>, // includes all panes (including eg. the status bar and tab bar in the default layout)
    tiled_panes: &'a mut TiledPanes,
    floating_panes: &'a mut FloatingPanes,
    pane_frame_style: PaneFrameStyle,
    focus_pane_id: &'a mut Option<PaneId>,
    os_api: Box<dyn ServerOsApi>,
    debug: bool,
    arrow_fonts: bool,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    explicitly_disable_kitty_keyboard_protocol: bool,
    blocking_terminal: Option<(TerminalId, NotificationEnd)>,
}

struct PreparedLayout {
    tiled: Vec<(TiledPaneLayout, PaneGeom)>,
    floating: Vec<PaneGeom>,
    retained_floating: Vec<(PaneId, PaneGeom)>,
}

#[derive(Clone)]
struct LayoutPane {
    id: PaneId,
    geom: PaneGeom,
    run: Option<Run>,
    selectable: bool,
    borderless: bool,
}

impl LayoutPane {
    fn existing(pane: &dyn Pane) -> Self {
        Self {
            id: pane.pid(),
            geom: pane.position_and_size(),
            run: pane.invoked_with().clone(),
            selectable: pane.selectable(),
            borderless: pane.borderless(),
        }
    }
}

impl<'a> LayoutApplier<'a> {
    pub fn new(
        viewport: &Rc<RefCell<Viewport>>,
        senders: &ThreadSenders,
        sixel_image_store: &Rc<RefCell<SixelImageStore>>,
        kitty_image_store: &Rc<RefCell<KittyImageStore>>,
        link_handler: &Rc<RefCell<LinkHandler>>,
        terminal_emulator_colors: &Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: &Rc<RefCell<HashMap<usize, String>>>,
        character_cell_size: &Rc<RefCell<Option<SizeInPixels>>>,
        connected_clients: &Rc<RefCell<AttachedClients>>,
        style: &Style,
        display_area: &Rc<RefCell<Size>>, // includes all panes (including eg. the status bar and tab bar in the default layout)
        tiled_panes: &'a mut TiledPanes,
        floating_panes: &'a mut FloatingPanes,
        pane_frame_style: PaneFrameStyle,
        focus_pane_id: &'a mut Option<PaneId>,
        os_api: &dyn ServerOsApi,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        explicitly_disable_kitty_keyboard_protocol: bool,
        blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Self {
        let viewport = viewport.clone();
        let senders = senders.clone();
        let sixel_image_store = sixel_image_store.clone();
        let kitty_image_store = kitty_image_store.clone();
        let link_handler = link_handler.clone();
        let terminal_emulator_colors = terminal_emulator_colors.clone();
        let terminal_emulator_color_codes = terminal_emulator_color_codes.clone();
        let character_cell_size = character_cell_size.clone();
        let connected_clients = connected_clients.clone();
        let style = *style;
        let display_area = display_area.clone();
        let os_api = os_api.box_clone();
        LayoutApplier {
            viewport,
            senders,
            sixel_image_store,
            kitty_image_store,
            link_handler,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            character_cell_size,
            connected_clients,
            style,
            display_area,
            tiled_panes,
            floating_panes,
            pane_frame_style,
            focus_pane_id,
            os_api,
            debug,
            arrow_fonts,
            styled_underlines,
            osc8_hyperlinks,
            explicitly_disable_kitty_keyboard_protocol,
            blocking_terminal,
        }
    }
    /// Check the complete layout before its caller changes focus or hands away panes.
    pub fn validate_layout(
        &self,
        layout: &TiledPaneLayout,
        floating: &[FloatingPaneLayout],
        terminals: &[(TerminalId, HoldForCommand)],
        floating_terminals: &[(TerminalId, HoldForCommand)],
        plugins: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
    ) -> Result<()> {
        self.prepare_layout(
            Some(layout),
            Some(floating),
            terminals,
            floating_terminals,
            plugins,
            None,
            None,
        )
        .map(|_| ())
    }

    pub fn validate_override_layout(
        &self,
        layout: &TiledPaneLayout,
        floating: &[FloatingPaneLayout],
        terminals: &[(TerminalId, HoldForCommand)],
        floating_terminals: &[(TerminalId, HoldForCommand)],
        plugins: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_terminals: bool,
        retain_plugins: bool,
        client_id: ClientId,
    ) -> Result<()> {
        self.prepare_layout(
            Some(layout),
            Some(floating),
            terminals,
            floating_terminals,
            plugins,
            Some((retain_terminals, retain_plugins)),
            Some(client_id),
        )
        .map(|_| ())
    }

    fn prepare_layout(
        &self,
        layout: Option<&TiledPaneLayout>,
        floating: Option<&[FloatingPaneLayout]>,
        terminals: &[(TerminalId, HoldForCommand)],
        floating_terminals: &[(TerminalId, HoldForCommand)],
        plugins: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retaining: Option<(bool, bool)>,
        client_id: Option<ClientId>,
    ) -> Result<PreparedLayout> {
        let mut plugins = plugins.clone();
        let mut allocate = |run: &Option<Run>,
                            terminals: &mut std::slice::Iter<'_, (TerminalId, HoldForCommand)>|
         -> Result<Option<PaneId>> {
            if let Some(Run::Plugin(run)) = run {
                self.senders
                    .to_plugin
                    .as_ref()
                    .context("missing plugin sender for layout")?;
                let id = plugins.get_mut(run).and_then(Vec::pop).with_context(|| {
                    format!("missing plugin for layout: {}", run.location_string())
                })?;
                Ok(Some(PaneId::Plugin(id)))
            } else {
                Ok(terminals.next().map(|(id, _)| PaneId::Terminal(*id)))
            }
        };
        let mut tiled: Vec<_> = self
            .tiled_panes
            .get_panes()
            .map(|(_, pane)| LayoutPane::existing(pane.as_ref()))
            .collect();
        let mut positions = vec![];
        let mut viewport = *self.viewport.borrow();
        if let Some(layout) = layout {
            positions = self.flatten_layout(layout, false)?;
            let mut remaining_positions = positions.clone();
            let mut terminals = terminals.iter();
            let mut retained = vec![];
            if retaining.is_some() {
                tiled.sort_by_key(|pane| (pane.geom.logical_position, pane.id));
                let mut existing = std::mem::take(&mut tiled);
                let mut unmatched = vec![];
                for (layout, geom) in remaining_positions {
                    if let Some(index) = existing.iter().position(|pane| pane.run == layout.run) {
                        let mut pane = existing.remove(index);
                        pane.geom = geom;
                        pane.borderless = layout.borderless.unwrap_or(pane.borderless);
                        tiled.push(pane);
                    } else {
                        unmatched.push((layout, geom));
                    }
                }
                remaining_positions = unmatched;
                let (retain_terminals, retain_plugins) = retaining.unwrap_or_default();
                retained.extend(existing.into_iter().filter(|pane| match pane.id {
                    PaneId::Terminal(_) => retain_terminals,
                    PaneId::Plugin(_) => retain_plugins,
                }));
            } else {
                for run in &layout.run_instructions_to_ignore {
                    let index = remaining_positions
                        .iter()
                        .position(|(slot, _)| &slot.run == run)
                        .or_else(|| {
                            remaining_positions
                                .iter()
                                .position(|(slot, _)| slot.run.is_none())
                        });
                    if let Some(index) = index {
                        let (slot, geom) = remaining_positions.remove(index);
                        if let Some(pane) = tiled.iter_mut().find(|pane| &pane.run == run) {
                            pane.geom = geom;
                            pane.borderless = slot.borderless.unwrap_or(pane.borderless);
                        }
                    }
                }
            }
            for (layout, geom) in remaining_positions {
                if let Some(id) = allocate(&layout.run, &mut terminals)? {
                    tiled.push(LayoutPane {
                        id,
                        geom,
                        run: layout.run,
                        selectable: true,
                        borderless: layout.borderless.unwrap_or(false),
                    });
                }
            }
            let mut logical_position = positions.last().and_then(|(_, geom)| geom.logical_position);
            for pane in &mut retained {
                pane.geom.logical_position = logical_position;
                if let Some(position) = logical_position.as_mut() {
                    *position += 1;
                }
            }
            if !retained.is_empty() {
                let positions = self.tiled_panes.prepare_insertions(
                    tiled
                        .iter()
                        .map(|pane| (pane.id, self.geometry_pane(pane)))
                        .collect(),
                    retained
                        .iter()
                        .map(|pane| self.geometry_pane(pane))
                        .collect(),
                    client_id,
                    self.os_api.box_clone(),
                )?;
                tiled.extend(retained);
                for pane in &mut tiled {
                    if let Some(geom) = positions.get(&pane.id) {
                        pane.geom = *geom;
                    }
                }
            }
            tiled.sort_by_key(|pane| pane.id);
            viewport = (*self.display_area.borrow()).into();
            let ui_pane_geoms: Vec<_> = tiled
                .iter()
                .filter(|pane| {
                    !pane.selectable || (pane.borderless && matches!(pane.id, PaneId::Plugin(_)))
                })
                .map(|pane| pane.geom.into())
                .collect();
            offset_viewport_for_ui_panes(&mut viewport, &ui_pane_geoms);
        }
        let mut floating_positions = vec![];
        let mut retained_floating = vec![];
        if let Some(floating) = floating {
            let mut existing: Vec<_> = self
                .floating_panes
                .get_panes()
                .map(|(_, pane)| LayoutPane::existing(pane.as_ref()))
                .collect();
            let mut occupied = vec![];
            let mut order = vec![];
            if retaining.is_some() {
                existing.sort_by_key(|pane| (pane.geom.logical_position, pane.id));
                let mut unmatched = vec![];
                for (index, layout) in floating.iter().enumerate() {
                    if let Some(pane_index) =
                        existing.iter().position(|pane| pane.run == layout.run)
                    {
                        order.push((index, Some(existing.remove(pane_index).id)));
                    } else {
                        unmatched.push((index, None));
                    }
                }
                order.extend(unmatched);
            } else {
                occupied.extend(existing.iter().map(|pane| (pane.id, pane.geom)));
                order.extend(floating.iter().enumerate().map(|(index, layout)| {
                    (
                        index,
                        layout
                            .already_running
                            .then(|| {
                                existing
                                    .iter()
                                    .find(|pane| pane.run == layout.run)
                                    .map(|pane| pane.id)
                            })
                            .flatten(),
                    )
                }));
            }
            floating_positions.resize(floating.len(), PaneGeom::default());
            let mut terminals = floating_terminals.iter();
            for (index, matched) in order {
                let layout = &floating[index];
                let geoms: Vec<_> = occupied.iter().map(|(_, geom)| *geom).collect();
                let mut geom =
                    FloatingPanes::position_layout_in_viewport(viewport, layout, &geoms)?;
                geom.logical_position = Some(index);
                floating_positions[index] = geom;
                let id = if matched.is_some() || (retaining.is_none() && layout.already_running) {
                    matched
                } else {
                    allocate(&layout.run, &mut terminals)?
                };
                if let Some(id) = id {
                    occupied.retain(|(existing_id, _)| *existing_id != id);
                    occupied.push((id, geom));
                }
            }
            if let Some((retain_terminals, retain_plugins)) = retaining {
                for pane in existing.into_iter().filter(|pane| match pane.id {
                    PaneId::Terminal(_) => retain_terminals,
                    PaneId::Plugin(_) => retain_plugins,
                }) {
                    let geoms: Vec<_> = occupied.iter().map(|(_, geom)| *geom).collect();
                    let mut geom = FloatingPanes::find_room_in_viewport(viewport, &geoms)
                        .context("not enough room for retained floating pane")?;
                    geom.logical_position = Some(floating.len());
                    retained_floating.push((pane.id, geom));
                    occupied.push((pane.id, geom));
                }
            }
        }
        Ok(PreparedLayout {
            tiled: positions,
            floating: floating_positions,
            retained_floating,
        })
    }

    fn geometry_pane(&self, pane: &LayoutPane) -> Box<dyn Pane> {
        let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
        let kitty_image_store = Rc::new(RefCell::new(KittyImageStore::default()));
        let mut blank: Box<dyn Pane> = match pane.id {
            PaneId::Terminal(id) => Box::new(TerminalPane::new(
                id,
                pane.geom,
                self.style,
                0,
                String::new(),
                self.link_handler.clone(),
                self.character_cell_size.clone(),
                sixel_image_store,
                kitty_image_store,
                self.terminal_emulator_colors.clone(),
                self.terminal_emulator_color_codes.clone(),
                None,
                pane.run.clone(),
                false,
                self.arrow_fonts,
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.explicitly_disable_kitty_keyboard_protocol,
                None,
            )),
            PaneId::Plugin(id) => {
                let (sender, _) = yinz_runtime::channels::unbounded();
                Box::new(PluginPane::new(
                    id,
                    pane.geom,
                    yinz_runtime::channels::SenderWithContext::new(sender),
                    String::new(),
                    String::new(),
                    sixel_image_store,
                    kitty_image_store,
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    vec![],
                    self.style,
                    pane.run.clone(),
                    false,
                    self.arrow_fonts,
                    self.styled_underlines,
                ))
            },
        };
        blank.set_selectable(pane.selectable);
        blank.set_borderless(pane.borderless);
        blank
    }

    pub fn apply_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        mut new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        client_id: ClientId,
    ) -> Result<bool> {
        // true => should_show_floating_panes
        let hide_floating_panes = layout.hide_floating_panes;
        let prepared = self.prepare_layout(
            Some(&layout),
            Some(&floating_panes_layout),
            &new_terminal_ids,
            &new_floating_terminal_ids,
            &new_plugin_ids,
            None,
            None,
        )?;
        self.apply_tiled_panes_layout(
            layout,
            new_terminal_ids,
            &mut new_plugin_ids,
            client_id,
            prepared.tiled,
        );
        let layout_has_floating_panes = self.apply_floating_panes_layout(
            floating_panes_layout,
            new_floating_terminal_ids,
            &mut new_plugin_ids,
            prepared.floating,
        );
        let should_show_floating_panes = layout_has_floating_panes && !hide_floating_panes;
        Ok(should_show_floating_panes)
    }
    pub fn override_layout(
        &mut self,
        tiled_panes_layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        mut new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
    ) -> Result<bool> {
        // true => should_show_floating_panes
        let hide_floating_panes = tiled_panes_layout.hide_floating_panes;
        let prepared = self.prepare_layout(
            Some(&tiled_panes_layout),
            Some(&floating_panes_layout),
            &new_terminal_ids,
            &new_floating_terminal_ids,
            &new_plugin_ids,
            Some((retain_existing_terminal_panes, retain_existing_plugin_panes)),
            Some(client_id),
        )?;
        self.override_tiled_panes(
            prepared.tiled,
            new_terminal_ids,
            &mut new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
        );

        let layout_has_floating_panes = self.override_floating_panes(
            &floating_panes_layout,
            new_floating_terminal_ids,
            &mut new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            prepared.floating,
            prepared.retained_floating,
        );
        let should_show_floating_panes = layout_has_floating_panes && !hide_floating_panes;
        Ok(should_show_floating_panes)
    }
    pub fn apply_tiled_panes_layout_to_existing_panes(
        &mut self,
        layout: &TiledPaneLayout,
    ) -> Result<()> {
        self.apply_tiled_panes_layout_in_order(layout, None)
    }
    pub fn apply_tiled_panes_layout_in_order(
        &mut self,
        layout: &TiledPaneLayout,
        order: Option<&[PaneId]>,
    ) -> Result<()> {
        let positions_in_layout = if let Some(order) = order {
            let viewport = *self.viewport.borrow();
            let size = Size {
                rows: viewport.rows,
                cols: viewport.cols,
            };
            let layout = arrangement_layout(layout.clone(), &size, order.len(), None)?;
            let mut positions = layout
                .position_panes_in_space(&PaneGeom::from(&size), Some(order.len()), false, false)
                .context("failed to position panes in layout")?;
            for (index, (_, geom)) in positions.iter_mut().enumerate() {
                geom.x += viewport.x;
                geom.y += viewport.y;
                geom.logical_position = Some(index);
            }
            positions
        } else {
            self.flatten_layout(layout, true)?
        };

        let mut panes: BTreeMap<_, _> = self
            .tiled_panes
            .get_panes()
            .map(|(id, pane)| {
                (
                    *id,
                    self.geometry_pane(&LayoutPane::existing(pane.as_ref())),
                )
            })
            .collect();
        let mut preserved = BTreeMap::new();
        if let Some(order) = order {
            let ids: Vec<_> = panes
                .keys()
                .copied()
                .filter(|id| !order.contains(id))
                .collect();
            for id in ids {
                if let Some(pane) = panes.remove(&id) {
                    preserved.insert(id, pane);
                }
            }
        }
        let mut existing_tab_state = ExistingTabState::new(panes);
        existing_tab_state.order = order.map(|ids| ids.to_vec());
        let mut placements = vec![];
        let mut positions_left_without_exact_matches = vec![];

        // look for exact matches (eg. panes that expect a specific command or plugin to run in them)
        for (layout, position_and_size) in positions_in_layout {
            match existing_tab_state
                .find_and_extract_exact_match_pane(&layout.run, position_and_size.logical_position)
            {
                Some(pane) => {
                    placements.push((pane, position_and_size, layout));
                },
                None => {
                    positions_left_without_exact_matches.push((layout, position_and_size));
                },
            }
        }

        // look for matches according to the logical position in the layout
        let mut positions_left = vec![];
        for (layout, position_and_size) in positions_left_without_exact_matches {
            match existing_tab_state.find_and_extract_pane_with_same_logical_position(
                position_and_size.logical_position,
            ) {
                Some(pane) => {
                    placements.push((pane, position_and_size, layout));
                },
                _ => {
                    positions_left.push((layout, position_and_size));
                },
            }
        }

        // fill the remaining panes by order of their logical position
        for (layout, position_and_size) in positions_left {
            // now let's try to find panes on a best-effort basis
            if let Some(pane) =
                existing_tab_state.find_and_extract_pane(position_and_size.logical_position)
            {
                placements.push((pane, position_and_size, layout));
            }
        }

        let remaining_pane_ids = existing_tab_state.pane_ids();
        let mut prepared = vec![];
        for (mut pane, geom, layout) in placements {
            pane.set_configured_rows(geom.rows);
            pane.set_geom(geom);
            if let Some(borderless) = layout.borderless {
                pane.set_borderless(borderless);
            }
            prepared.push((pane.pid(), geom, layout));
            preserved.insert(pane.pid(), pane);
        }
        let remaining = remaining_pane_ids
            .iter()
            .filter_map(|id| existing_tab_state.remove_pane(id))
            .collect();
        self.tiled_panes
            .prepare_insertions(preserved, remaining, None, self.os_api.box_clone())?;

        let mut panes = self.tiled_panes.drain();
        if let Some(order) = order {
            let ids: Vec<_> = panes
                .keys()
                .copied()
                .filter(|id| !order.contains(id))
                .collect();
            for id in ids {
                if let Some(pane) = panes.remove(&id) {
                    self.tiled_panes.add_pane_with_existing_geom(id, pane);
                }
            }
        }
        let mut existing_tab_state = ExistingTabState::new(panes);
        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );
        for (id, geom, layout) in prepared {
            if let Some(pane) = existing_tab_state.remove_pane(&id) {
                pane_applier.apply_position_and_size_to_tiled_pane(pane, geom, layout);
            }
        }
        pane_applier.handle_remaining_tiled_pane_ids(
            remaining_pane_ids,
            existing_tab_state,
            None,
            None,
        );
        pane_applier.finalize_tiled_state();

        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            self.tiled_panes,
            self.pane_frame_style,
        );
        Ok(())
    }
    pub fn override_tiled_panes_layout_for_existing_panes(
        &mut self,
        tiled_panes_layout: &TiledPaneLayout,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let prepared = self.prepare_layout(
            Some(tiled_panes_layout),
            None,
            &new_terminal_ids,
            &[],
            new_plugin_ids,
            Some((retain_existing_terminal_panes, retain_existing_plugin_panes)),
            Some(client_id),
        )?;
        self.override_tiled_panes(
            prepared.tiled,
            new_terminal_ids,
            new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
        );
        Ok(())
    }

    fn override_tiled_panes(
        &mut self,
        positions_in_layout: Vec<(TiledPaneLayout, PaneGeom)>,
        mut new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
    ) {
        let mut existing_tab_state = ExistingTabState::new(self.tiled_panes.drain());

        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );
        let mut positions_left_without_exact_matches = vec![];

        // look for exact matches (eg. panes that expect a specific command or plugin to run in them)
        let mut last_logical_position = None;
        for (layout, position_and_size) in positions_in_layout {
            last_logical_position = position_and_size.logical_position;
            match existing_tab_state.find_and_extract_exact_pane_with_same_run(&layout.run) {
                Some(pane) => {
                    pane_applier.apply_position_and_size_to_tiled_pane(
                        pane,
                        position_and_size,
                        layout,
                    );
                },
                None => {
                    positions_left_without_exact_matches.push((layout, position_and_size));
                },
            }
        }

        let remaining_pane_ids: Vec<PaneId> = existing_tab_state.pane_ids();
        self.close_non_retained_panes(
            &mut existing_tab_state,
            &remaining_pane_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
        );

        let (focus_pane_id, pane_ids_expanded_in_stack) = self.position_new_panes(
            &mut new_terminal_ids,
            new_plugin_ids,
            &mut positions_left_without_exact_matches,
        );

        // we do this because we have to add the remaining tiled pane ids ONLY AFTER positioning
        // the new panes, otherwise the layout might get borked
        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );
        // Preparation ran these same insertions with private geometry and disconnected senders.
        if retain_existing_terminal_panes || retain_existing_plugin_panes {
            pane_applier.handle_remaining_tiled_pane_ids(
                remaining_pane_ids,
                existing_tab_state,
                last_logical_position,
                Some(client_id),
            );
        }
        pane_applier.finalize_tiled_state();

        if let Some(pane_id) = focus_pane_id {
            *self.focus_pane_id = Some(pane_id);
            self.tiled_panes.focus_pane(pane_id, client_id);
        }

        // in case focused panes were closed by the override
        self.tiled_panes.move_client_focus_to_existing_panes();

        for pane_id in &pane_ids_expanded_in_stack {
            self.tiled_panes.expand_pane_in_stack(*pane_id);
        }

        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            self.tiled_panes,
            self.pane_frame_style,
        );
    }
    fn close_non_retained_panes(
        &self,
        existing_tab_state: &mut ExistingTabState,
        remaining_pane_ids: &[PaneId],
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
    ) {
        for pane_id in remaining_pane_ids {
            if retain_existing_terminal_panes && matches!(pane_id, PaneId::Terminal(_)) {
                continue;
            }
            if retain_existing_plugin_panes && matches!(pane_id, PaneId::Plugin(_)) {
                continue;
            }
            existing_tab_state.remove_pane(pane_id);
            match pane_id {
                PaneId::Terminal(_) => {
                    let _ = self
                        .senders
                        .send_to_pty(PtyInstruction::ClosePane(*pane_id, None));
                },
                PaneId::Plugin(plugin_id) => {
                    let _ = self
                        .senders
                        .send_to_plugin(PluginInstruction::Unload(*plugin_id));
                },
            }
        }
    }
    fn flatten_layout(
        &self,
        layout: &TiledPaneLayout,
        has_existing_panes: bool,
    ) -> Result<Vec<(TiledPaneLayout, PaneGeom)>> {
        let free_space = self.total_space_for_tiled_panes();
        let tiled_panes_count = if has_existing_panes {
            Some(self.tiled_panes.visible_panes_count())
        } else {
            None
        };
        let focus_layout_if_not_focused = !has_existing_panes;
        let mut positions_in_layout = layout
            .position_panes_in_space(
                &free_space,
                tiled_panes_count,
                false,
                focus_layout_if_not_focused,
            )
            .or_else(|_e| {
                // in the error branch, we try to recover by positioning the panes in the space but
                // ignoring the percentage sizes (passing true as the third argument), this is a hack
                // around some issues with the constraint system that should be addressed in a systemic
                // manner
                layout.position_panes_in_space(
                    &free_space,
                    tiled_panes_count,
                    true,
                    focus_layout_if_not_focused,
                )
            })
            .context("failed to position panes in layout")?;
        for (logical_position, (_layout, position_and_size)) in
            positions_in_layout.iter_mut().enumerate()
        {
            position_and_size.logical_position = Some(logical_position);
        }
        Ok(positions_in_layout)
    }
    fn apply_tiled_panes_layout(
        &mut self,
        layout: TiledPaneLayout,
        mut new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        client_id: ClientId,
        mut positions_in_layout: Vec<(TiledPaneLayout, PaneGeom)>,
    ) {
        let run_instructions_without_a_location = self.position_run_instructions_to_ignore(
            &layout.run_instructions_to_ignore,
            &mut positions_in_layout,
        );
        let (focus_pane_id, pane_ids_expanded_in_stack) = self.position_new_panes(
            &mut new_terminal_ids,
            new_plugin_ids,
            &mut positions_in_layout,
        );
        self.handle_run_instructions_without_a_location(
            run_instructions_without_a_location,
            &mut new_terminal_ids,
        );

        for pane_id in &pane_ids_expanded_in_stack {
            self.tiled_panes.expand_pane_in_stack(*pane_id);
        }

        self.adjust_viewport();
        self.set_focused_tiled_pane(focus_pane_id, client_id);
    }
    fn position_run_instructions_to_ignore(
        &mut self,
        run_instructions_to_ignore: &[Option<Run>],
        positions_in_layout: &mut Vec<(TiledPaneLayout, PaneGeom)>,
    ) -> Vec<Option<Run>> {
        // here we try to find rooms for the panes that are already running (represented by
        // run_instructions_to_ignore), we try to either find an explicit position (the new
        // layout has a pane with the exact run instruction) or an otherwise free position
        // (the new layout has a pane with None as its run instruction, eg. just `pane` in the
        // layout)
        let mut run_instructions_without_a_location = vec![];
        for run_instruction in run_instructions_to_ignore.iter().cloned() {
            if self
                .place_running_pane_in_exact_match_location(&run_instruction, positions_in_layout)
            {
                // found exact match
            } else if self
                .place_running_pane_in_empty_location(&run_instruction, positions_in_layout)
            {
                // found empty location
            } else {
                // no room! we'll add it below after we place everything else
                run_instructions_without_a_location.push(run_instruction);
            }
        }
        run_instructions_without_a_location
    }
    fn position_new_panes(
        &mut self,
        new_terminal_ids: &mut Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        positions_in_layout: &mut Vec<(TiledPaneLayout, PaneGeom)>,
    ) -> (Option<PaneId>, Vec<PaneId>) {
        // returns: optional pane id to focus, pane ids
        // expanded in stack
        // here we open new panes for each run instruction in the layout with the details
        // we got from the plugin thread and pty thread
        let mut pane_ids_expanded_in_stack = vec![];
        let mut focus_pane_id: Option<PaneId> = None;
        let mut set_focus_pane_id = |layout: &TiledPaneLayout, pane_id: PaneId| {
            if layout.focus.unwrap_or(false) && focus_pane_id.is_none() {
                focus_pane_id = Some(pane_id);
            }
        };
        for (layout, position_and_size) in positions_in_layout {
            if let Some(Run::Plugin(run)) = layout.run.clone() {
                let pid = match self.new_tiled_plugin_pane(
                    run,
                    new_plugin_ids,
                    position_and_size,
                    layout,
                ) {
                    Ok(pid) => pid,
                    Err(error) => {
                        Err::<(), _>(error).non_fatal();
                        continue;
                    },
                };
                if layout.is_expanded_in_stack {
                    pane_ids_expanded_in_stack.push(PaneId::Plugin(pid));
                }
                set_focus_pane_id(layout, PaneId::Plugin(pid));
            } else if !new_terminal_ids.is_empty() {
                // there are still panes left to fill, use the pids we received in this method
                let (pid, hold_for_command) = new_terminal_ids.remove(0);
                self.new_terminal_pane(pid, &hold_for_command, position_and_size, layout)
                    .non_fatal();
                if layout.is_expanded_in_stack {
                    pane_ids_expanded_in_stack.push(PaneId::Terminal(pid));
                }
                set_focus_pane_id(layout, PaneId::Terminal(pid));
            }
        }
        (focus_pane_id, pane_ids_expanded_in_stack)
    }
    fn handle_run_instructions_without_a_location(
        &mut self,
        run_instructions_without_a_location: Vec<Option<Run>>,
        new_terminal_ids: &mut Vec<(TerminalId, HoldForCommand)>,
    ) {
        for run_instruction in run_instructions_without_a_location {
            self.tiled_panes
                .assign_geom_for_pane_with_run(run_instruction);
        }
        for (unused_pid, _) in new_terminal_ids {
            let _ = self.senders.send_to_pty(PtyInstruction::ClosePane(
                PaneId::Terminal(*unused_pid),
                None,
            ));
        }
    }
    fn new_tiled_plugin_pane(
        &mut self,
        run: RunPluginOrAlias,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        position_and_size: &PaneGeom,
        layout: &TiledPaneLayout,
    ) -> Result<PluginId> {
        let err_context = || "Failed to start new plugin pane".to_string();
        let pane_title = run.location_string();
        let pid = new_plugin_ids
            .get_mut(&run)
            .and_then(|ids| ids.pop())
            .with_context(err_context)?;
        let mut new_plugin = PluginPane::new(
            pid,
            *position_and_size,
            self.senders
                .to_plugin
                .as_ref()
                .with_context(err_context)?
                .clone(),
            pane_title,
            layout.name.clone().unwrap_or_default(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.connected_clients.borrow().iter().copied().collect(),
            self.style,
            layout.run.clone(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
        );
        new_plugin.pending_pane_tabs = layout.pane_tabs.clone();
        if let Some(pane_initial_contents) = &layout.pane_initial_contents {
            new_plugin.handle_pty_bytes(pane_initial_contents.as_bytes().into());
            new_plugin.handle_pty_bytes("\n\r".as_bytes().into());
        }

        new_plugin.set_borderless(layout.borderless.unwrap_or(false));
        if let Some(exclude_from_sync) = layout.exclude_from_sync {
            new_plugin.set_exclude_from_sync(exclude_from_sync);
        }
        self.tiled_panes
            .add_pane_with_existing_geom(PaneId::Plugin(pid), Box::new(new_plugin));
        Ok(pid)
    }
    fn new_floating_plugin_pane(
        &mut self,
        run: RunPluginOrAlias,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        position_and_size: PaneGeom,
        floating_pane_layout: &FloatingPaneLayout,
    ) -> Result<Option<PaneId>> {
        let mut pid_to_focus = None;
        let err_context = || "Failed to create new floating plugin pane".to_string();
        let pane_title = run.location_string();
        let pid = new_plugin_ids
            .get_mut(&run)
            .and_then(|ids| ids.pop())
            .with_context(err_context)?;
        let mut new_pane = PluginPane::new(
            pid,
            position_and_size,
            self.senders
                .to_plugin
                .as_ref()
                .with_context(err_context)?
                .clone(),
            pane_title,
            floating_pane_layout.name.clone().unwrap_or_default(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.connected_clients.borrow().iter().copied().collect(),
            self.style,
            floating_pane_layout.run.clone(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
        );
        new_pane.pending_pane_tabs = floating_pane_layout.pane_tabs.clone();
        if let Some(pane_initial_contents) = &floating_pane_layout.pane_initial_contents {
            new_pane.handle_pty_bytes(pane_initial_contents.as_bytes().into());
            new_pane.handle_pty_bytes("\n\r".as_bytes().into());
        }
        if floating_pane_layout.borderless.unwrap_or(false) {
            new_pane.set_borderless(true);
        } else {
            new_pane.set_borderless(false);
        }
        resize_pty!(
            new_pane,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .non_fatal();
        self.floating_panes
            .add_pane(PaneId::Plugin(pid), Box::new(new_pane));
        if floating_pane_layout.focus.unwrap_or(false) {
            pid_to_focus = Some(PaneId::Plugin(pid));
        }
        Ok(pid_to_focus)
    }
    fn new_floating_terminal_pane(
        &mut self,
        pid: &TerminalId,
        hold_for_command: &HoldForCommand,
        position_and_size: PaneGeom,
        floating_pane_layout: &FloatingPaneLayout,
    ) -> Result<Option<PaneId>> {
        let mut pane_id_to_focus = None;
        let next_terminal_position =
            get_next_terminal_position(self.tiled_panes, self.floating_panes);
        let initial_title = match &floating_pane_layout.run {
            Some(Run::Command(run_command)) => Some(run_command.to_string()),
            _ => None,
        };
        let mut new_pane = TerminalPane::new(
            *pid,
            position_and_size,
            self.style,
            next_terminal_position,
            floating_pane_layout.name.clone().unwrap_or_default(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            initial_title,
            floating_pane_layout.run.clone(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            None,
        );
        if let Some(pane_initial_contents) = &floating_pane_layout.pane_initial_contents {
            new_pane.handle_pty_bytes(pane_initial_contents.as_bytes().into());
            new_pane.handle_pty_bytes("\n\r".as_bytes().into());
        }
        if floating_pane_layout.borderless.unwrap_or(false) {
            new_pane.set_borderless(true);
        } else {
            new_pane.set_borderless(false);
        }
        if floating_pane_layout.default_fg.is_some() || floating_pane_layout.default_bg.is_some() {
            new_pane.set_pane_default_colors(
                floating_pane_layout.default_fg.clone(),
                floating_pane_layout.default_bg.clone(),
            );
        }
        if let Some(held_command) = hold_for_command {
            new_pane.hold(None, true, held_command.clone());
        }
        resize_pty!(
            new_pane,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .non_fatal();
        self.floating_panes
            .add_pane(PaneId::Terminal(*pid), Box::new(new_pane));
        if floating_pane_layout.focus.unwrap_or(false) {
            pane_id_to_focus = Some(PaneId::Terminal(*pid));
        }
        Ok(pane_id_to_focus)
    }
    fn new_terminal_pane(
        &mut self,
        pid: TerminalId,
        hold_for_command: &HoldForCommand,
        position_and_size: &PaneGeom,
        layout: &TiledPaneLayout,
    ) -> Result<()> {
        let next_terminal_position =
            get_next_terminal_position(self.tiled_panes, self.floating_panes);
        let initial_title = match &layout.run {
            Some(Run::Command(run_command)) => Some(run_command.to_string()),
            _ => None,
        };

        // Check if this terminal should receive the blocking completion_tx
        let notification_end = if let Some((blocking_pid, _)) = &self.blocking_terminal {
            if *blocking_pid == pid {
                self.blocking_terminal.take().map(|(_, tx)| tx)
            } else {
                None
            }
        } else {
            None
        };

        let mut new_pane = TerminalPane::new(
            pid,
            *position_and_size,
            self.style,
            next_terminal_position,
            layout.name.clone().unwrap_or_default(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            initial_title,
            layout.run.clone(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            notification_end,
        );
        if let Some(pane_initial_contents) = &layout.pane_initial_contents {
            new_pane.handle_pty_bytes(pane_initial_contents.as_bytes().into());
            new_pane.handle_pty_bytes("\n\r".as_bytes().into());
        }
        new_pane.set_borderless(layout.borderless.unwrap_or(false));
        if let Some(exclude_from_sync) = layout.exclude_from_sync {
            new_pane.set_exclude_from_sync(exclude_from_sync);
        }
        if layout.default_fg.is_some() || layout.default_bg.is_some() {
            new_pane.set_pane_default_colors(layout.default_fg.clone(), layout.default_bg.clone());
        }
        if let Some(held_command) = hold_for_command {
            new_pane.hold(None, true, held_command.clone());
        }
        self.tiled_panes
            .add_pane_with_existing_geom(PaneId::Terminal(pid), Box::new(new_pane));
        Ok(())
    }
    fn place_running_pane_in_exact_match_location(
        &mut self,
        run_instruction: &Option<Run>,
        positions_in_layout: &mut Vec<(TiledPaneLayout, PaneGeom)>,
    ) -> bool {
        let mut found_exact_match = false;

        if let Some(position) = positions_in_layout
            .iter()
            .position(|(layout, _position_and_size)| &layout.run == run_instruction)
        {
            let (layout, position_and_size) = positions_in_layout.remove(position);
            self.tiled_panes.set_geom_for_pane_with_run(
                layout.run,
                position_and_size,
                layout.borderless,
            );
            found_exact_match = true;
        }
        found_exact_match
    }
    fn place_running_pane_in_empty_location(
        &mut self,
        run_instruction: &Option<Run>,
        positions_in_layout: &mut Vec<(TiledPaneLayout, PaneGeom)>,
    ) -> bool {
        let mut found_empty_location = false;
        if let Some(position) = positions_in_layout
            .iter()
            .position(|(layout, _position_and_size)| layout.run.is_none())
        {
            let (layout, position_and_size) = positions_in_layout.remove(position);
            self.tiled_panes.set_geom_for_pane_with_run(
                run_instruction.clone(),
                position_and_size,
                layout.borderless,
            );
            found_empty_location = true;
        }
        found_empty_location
    }
    fn apply_floating_panes_layout(
        &mut self,
        mut floating_panes_layout: Vec<FloatingPaneLayout>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        positions: Vec<PaneGeom>,
    ) -> bool {
        let layout_has_floating_panes = !floating_panes_layout.is_empty();

        for (logical_position, floating_pane_layout) in floating_panes_layout.iter_mut().enumerate()
        {
            floating_pane_layout.logical_position = Some(logical_position);
        }

        let floating_panes_layout = floating_panes_layout.iter();
        let mut focused_floating_pane = None;
        let mut new_floating_terminal_ids = new_floating_terminal_ids.iter();
        for (floating_pane_layout, position_and_size) in floating_panes_layout.zip(positions) {
            let pid_to_focus = if floating_pane_layout.already_running {
                self.floating_panes.set_geom_for_pane_with_run(
                    floating_pane_layout.run.clone(),
                    position_and_size,
                );
                None
            } else if let Some(Run::Plugin(run)) = floating_pane_layout.run.clone() {
                self.new_floating_plugin_pane(
                    run,
                    new_plugin_ids,
                    position_and_size,
                    floating_pane_layout,
                )
                .unwrap_or_else(|error| {
                    Err::<(), _>(error).non_fatal();
                    None
                })
            } else if let Some((pid, hold_for_command)) = new_floating_terminal_ids.next() {
                self.new_floating_terminal_pane(
                    pid,
                    hold_for_command,
                    position_and_size,
                    floating_pane_layout,
                )
                .unwrap_or_else(|error| {
                    Err::<(), _>(error).non_fatal();
                    None
                })
            } else {
                None
            };
            if let Some(pid_to_focus) = pid_to_focus {
                focused_floating_pane = Some(pid_to_focus);
            }
        }
        if let Some(focused_floating_pane) = focused_floating_pane {
            self.floating_panes
                .focus_pane_for_all_clients(focused_floating_pane);
        }
        layout_has_floating_panes
    }
    pub fn apply_floating_panes_layout_to_existing_panes(
        &mut self,
        floating_panes_layout: &[FloatingPaneLayout],
    ) -> Result<bool> {
        let layout_has_floating_panes = self.floating_panes.has_panes();
        let mut positions_in_layout = floating_panes_layout.to_vec();
        let mut logical_position = 0;
        for floating_pane_layout in positions_in_layout.iter_mut() {
            floating_pane_layout.logical_position = Some(logical_position);
            logical_position += 1;
        }
        let mut existing_tab_state = ExistingTabState::new(
            self.floating_panes
                .get_panes()
                .map(|(id, pane)| {
                    (
                        *id,
                        self.geometry_pane(&LayoutPane::existing(pane.as_ref())),
                    )
                })
                .collect(),
        );
        let mut panes_to_apply = vec![];
        let mut positions_left = vec![];

        // look for exact matches, first by pane contents and then by logical position
        for floating_pane_layout in positions_in_layout {
            match existing_tab_state
                .find_and_extract_exact_match_pane(
                    &floating_pane_layout.run,
                    floating_pane_layout.logical_position,
                )
                .or_else(|| {
                    existing_tab_state.find_and_extract_pane_with_same_logical_position(
                        floating_pane_layout.logical_position,
                    )
                }) {
                Some(pane) => {
                    panes_to_apply.push((pane, floating_pane_layout));
                },
                None => {
                    positions_left.push(floating_pane_layout);
                },
            }
        }

        // fill the remaining panes by order of their logical position
        for floating_pane_layout in positions_left {
            if let Some(pane) =
                existing_tab_state.find_and_extract_pane(floating_pane_layout.logical_position)
            {
                panes_to_apply.push((pane, floating_pane_layout));
            }
        }

        let viewport = *self.viewport.borrow();
        let mut occupied = vec![];
        let mut placements = vec![];
        for (pane, layout) in panes_to_apply {
            let geom = FloatingPanes::position_layout_in_viewport(viewport, &layout, &occupied)?;
            occupied.push(geom);
            placements.push((pane.pid(), layout, geom));
        }
        let mut retained = vec![];
        for id in existing_tab_state.pane_ids() {
            let mut geom = FloatingPanes::find_room_in_viewport(viewport, &occupied)
                .context("not enough room for retained floating pane")?;
            geom.logical_position = Some(logical_position);
            occupied.push(geom);
            retained.push((id, geom));
        }
        let mut real_panes = ExistingTabState::new(self.floating_panes.drain());
        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );
        for (id, layout, geom) in placements {
            if let Some(pane) = real_panes.remove_pane(&id) {
                pane_applier.apply_prepared_floating_pane(pane, layout, geom);
            }
        }
        for (id, geom) in retained {
            if let Some(pane) = real_panes.remove_pane(&id) {
                pane_applier.apply_position_and_size_to_floating_pane(pane, geom);
            }
        }
        pane_applier.finalize_floating_panes_state();

        if layout_has_floating_panes {
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn override_floating_panes_layout_for_existing_panes(
        &mut self,
        floating_panes_layout: &[FloatingPaneLayout],
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
    ) -> Result<bool> {
        let prepared = self.prepare_layout(
            None,
            Some(floating_panes_layout),
            &[],
            &new_terminal_ids,
            new_plugin_ids,
            Some((retain_existing_terminal_panes, retain_existing_plugin_panes)),
            None,
        )?;
        Ok(self.override_floating_panes(
            floating_panes_layout,
            new_terminal_ids,
            new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            prepared.floating,
            prepared.retained_floating,
        ))
    }

    fn override_floating_panes(
        &mut self,
        floating_panes_layout: &[FloatingPaneLayout],
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: &mut HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        positions: Vec<PaneGeom>,
        retained_positions: Vec<(PaneId, PaneGeom)>,
    ) -> bool {
        let layout_has_floating_panes = !floating_panes_layout.is_empty();
        let mut positions_in_layout = floating_panes_layout.to_vec();
        for (logical_position, floating_pane_layout) in positions_in_layout.iter_mut().enumerate() {
            floating_pane_layout.logical_position = Some(logical_position);
        }
        let mut existing_tab_state = ExistingTabState::new(self.floating_panes.drain());
        let mut positions_left = vec![];

        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );

        // find already running exact matches
        for (floating_pane_layout, geom) in positions_in_layout.into_iter().zip(positions) {
            match existing_tab_state
                .find_and_extract_exact_pane_with_same_run(&floating_pane_layout.run)
            {
                Some(pane) => {
                    pane_applier.apply_prepared_floating_pane(pane, floating_pane_layout, geom);
                },
                None => {
                    positions_left.push((floating_pane_layout, geom));
                },
            }
        }

        let remaining_pane_ids: Vec<PaneId> = existing_tab_state.pane_ids();
        self.close_non_retained_panes(
            &mut existing_tab_state,
            &remaining_pane_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
        );

        // open new panes
        let mut focused_floating_pane = None;
        let mut new_floating_terminal_ids = new_terminal_ids.iter();
        for (floating_pane_layout, position_and_size) in positions_left {
            let pid_to_focus = if let Some(Run::Plugin(run)) = floating_pane_layout.run.clone() {
                self.new_floating_plugin_pane(
                    run,
                    new_plugin_ids,
                    position_and_size,
                    &floating_pane_layout,
                )
                .unwrap_or_else(|error| {
                    Err::<(), _>(error).non_fatal();
                    None
                })
            } else if let Some((pid, hold_for_command)) = new_floating_terminal_ids.next() {
                self.new_floating_terminal_pane(
                    pid,
                    hold_for_command,
                    position_and_size,
                    &floating_pane_layout,
                )
                .unwrap_or_else(|error| {
                    Err::<(), _>(error).non_fatal();
                    None
                })
            } else {
                None
            };
            if let Some(pid_to_focus) = pid_to_focus {
                focused_floating_pane = Some(pid_to_focus);
            }
        }

        // we do this because we have to add the remaining tiled pane ids ONLY AFTER positioning
        // the new panes, otherwise the layout might get borked
        let mut pane_applier = PaneApplier::new(
            self.tiled_panes,
            self.floating_panes,
            &self.senders,
            &self.character_cell_size,
        );
        for (id, geom) in retained_positions {
            if let Some(pane) = existing_tab_state.remove_pane(&id) {
                pane_applier.apply_position_and_size_to_floating_pane(pane, geom);
            }
        }

        if let Some(focused_floating_pane) = focused_floating_pane {
            self.floating_panes
                .focus_pane_for_all_clients(focused_floating_pane);
        } else {
            // we do this in case there is no explicitly focused pane in the new layout, so as to
            // make sure any panes that are already focused have a higher z_index than the ones
            // added above
            self.floating_panes.reapply_pane_focus();
        }

        // in case focused panes were closed by the override
        self.floating_panes.move_client_focus_to_existing_panes();

        layout_has_floating_panes
    }
    fn resize_whole_tab(&mut self, new_screen_size: Size) {
        let err_context = || {
            format!(
                "failed to resize whole tab to new screen size {:?}",
                new_screen_size
            )
        };
        self.floating_panes.resize(new_screen_size);
        // we need to do this explicitly because floating_panes.resize does not do this
        self.floating_panes
            .resize_pty_all_panes(&mut self.os_api)
            .with_context(err_context)
            .non_fatal();
        self.tiled_panes.resize(new_screen_size);
    }
    pub fn offset_viewport(
        viewport: Rc<RefCell<Viewport>>,
        display_area: Rc<RefCell<Size>>,
        tiled_panes: &mut TiledPanes,
        pane_frame_style: PaneFrameStyle,
    ) {
        {
            // reset viewport before reapplying offset
            let mut viewport = viewport.borrow_mut();
            *viewport = (*display_area.borrow()).into();
        }
        let boundary_geoms = tiled_panes.non_selectable_pane_geoms_inside_viewport();
        {
            // curly braces here is so that we free viewport immediately when we're done
            let mut viewport = viewport.borrow_mut();
            offset_viewport_for_ui_panes(&mut viewport, &boundary_geoms);
        }
        tiled_panes.set_pane_frames(pane_frame_style);
    }
    fn adjust_viewport(&mut self) {
        // here we offset the viewport after applying a tiled panes layout
        // from borderless panes that are on the edges of the
        // screen, this is so that when we don't have pane boundaries (eg. when they were
        // disabled by the user) boundaries won't be drawn around these panes
        // geometrically, we can only do this with panes that are on the edges of the
        // screen - so it's mostly a best-effort thing
        let display_area = {
            let display_area = self.display_area.borrow();
            *display_area
        };
        self.resize_whole_tab(display_area);
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            self.tiled_panes,
            self.pane_frame_style,
        );
    }
    fn set_focused_tiled_pane(&mut self, focus_pane_id: Option<PaneId>, client_id: ClientId) {
        if let Some(pane_id) = focus_pane_id {
            *self.focus_pane_id = Some(pane_id);
            self.tiled_panes.focus_pane(pane_id, client_id);
        } else {
            let next_selectable_pane_id = self.tiled_panes.first_selectable_pane_id();
            match next_selectable_pane_id {
                Some(active_pane_id) => {
                    self.tiled_panes.focus_pane(active_pane_id, client_id);
                },
                None => {
                    self.tiled_panes.clear_active_panes();
                },
            }
        }
    }
    fn total_space_for_tiled_panes(&self) -> PaneGeom {
        // for tiled panes we need to take the display area rather than the viewport because the
        // viewport can potentially also be changed
        let (display_area_cols, display_area_rows) = {
            let display_area = self.display_area.borrow();
            (display_area.cols, display_area.rows)
        };

        let mut free_space = PaneGeom::default();
        free_space.cols.set_inner(display_area_cols);
        free_space.rows.set_inner(display_area_rows);
        free_space
    }
}

fn offset_viewport_for_ui_panes(viewport: &mut Viewport, ui_pane_geoms: &[Viewport]) {
    for geom in ui_pane_geoms {
        let row_strips = ui_pane_geoms.iter().map(|g| ((g.y, g.rows), (g.x, g.cols)));
        if let Some(remaining_rows) = shrink_by_edge_strip(
            (viewport.y, viewport.rows),
            (viewport.x, viewport.cols),
            (geom.y, geom.rows),
            row_strips,
        ) {
            (viewport.y, viewport.rows) = remaining_rows;
        }
        let col_strips = ui_pane_geoms.iter().map(|g| ((g.x, g.cols), (g.y, g.rows)));
        if let Some(remaining_cols) = shrink_by_edge_strip(
            (viewport.x, viewport.cols),
            (viewport.y, viewport.rows),
            (geom.x, geom.cols),
            col_strips,
        ) {
            (viewport.x, viewport.cols) = remaining_cols;
        }
    }
}

fn shrink_by_edge_strip(
    (start, len): (usize, usize),
    (cross_start, cross_len): (usize, usize),
    (strip_start, strip_len): (usize, usize),
    geoms: impl Iterator<Item = ((usize, usize), (usize, usize))>,
) -> Option<(usize, usize)> {
    let is_at_start = strip_start == start;
    let is_at_end = strip_start + strip_len == start + len;
    if strip_len == 0 || strip_len >= len || !(is_at_start || is_at_end) {
        return None;
    }
    let mut cross_spans: Vec<(usize, usize)> = geoms
        .filter(|(span, _)| *span == (strip_start, strip_len))
        .map(|(_, cross_span)| cross_span)
        .collect();
    cross_spans.sort_unstable();
    let mut covered_until = cross_start;
    for (cross_span_start, cross_span_len) in cross_spans {
        if cross_span_start > covered_until {
            break;
        }
        covered_until = covered_until.max(cross_span_start + cross_span_len);
    }
    if covered_until < cross_start + cross_len {
        return None;
    }
    let remaining_start = if is_at_start {
        start + strip_len
    } else {
        start
    };
    Some((remaining_start, len - strip_len))
}

struct ExistingTabState {
    order: Option<Vec<PaneId>>,
    existing_panes: BTreeMap<PaneId, Box<dyn Pane>>,
}

impl ExistingTabState {
    pub fn new(existing_panes: BTreeMap<PaneId, Box<dyn Pane>>) -> Self {
        ExistingTabState {
            existing_panes,
            order: None,
        }
    }
    pub fn find_and_extract_exact_match_pane(
        &mut self,
        run: &Option<Run>,
        logical_position: Option<usize>,
    ) -> Option<Box<dyn Pane>> {
        let candidates = self.pane_candidates();
        if self.order.is_some() {
            let id = candidates.first().map(|(id, _)| **id)?;
            return self.existing_panes.remove(&id);
        }
        if let Some(current_pane_id_with_same_contents) =
            self.find_pane_id_with_same_contents(&candidates, run, logical_position)
        {
            return self
                .existing_panes
                .remove(&current_pane_id_with_same_contents);
        }
        None
    }
    pub fn find_and_extract_exact_pane_with_same_run(
        &mut self,
        run: &Option<Run>,
    ) -> Option<Box<dyn Pane>> {
        let candidates = self.pane_candidates();
        let pane_id = candidates.iter().find_map(|(_pid, p)| {
            if p.invoked_with() == run {
                Some(p.pid())
            } else {
                None
            }
        });
        pane_id.and_then(|p| self.existing_panes.remove(&p))
    }
    pub fn find_and_extract_pane_with_same_logical_position(
        &mut self,
        logical_position: Option<usize>,
    ) -> Option<Box<dyn Pane>> {
        let candidates = self.pane_candidates();
        if let Some(current_pane_id_with_same_logical_position) =
            self.find_pane_id_with_same_logical_position(&candidates, logical_position)
        {
            self.existing_panes
                .remove(&current_pane_id_with_same_logical_position)
        } else {
            None
        }
    }
    pub fn find_and_extract_pane(
        &mut self,
        logical_position: Option<usize>,
    ) -> Option<Box<dyn Pane>> {
        let candidates = self.pane_candidates();
        if let Some(current_pane_id_with_same_logical_position) =
            self.find_pane_id_with_same_logical_position(&candidates, logical_position)
        {
            self.existing_panes
                .remove(&current_pane_id_with_same_logical_position)
        } else {
            match candidates.first().map(|(pid, _p)| *pid).copied() {
                Some(first_candidate) => self.existing_panes.remove(&first_candidate),
                None => None,
            }
        }
    }
    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.pane_candidates()
            .into_iter()
            .map(|(id, _)| *id)
            .collect()
    }
    pub fn remove_pane(&mut self, pane_id: &PaneId) -> Option<Box<dyn Pane>> {
        self.existing_panes.remove(pane_id)
    }
    fn pane_candidates(&self) -> Vec<(&PaneId, &dyn Pane)> {
        let mut candidates: Vec<_> = self
            .existing_panes
            .iter()
            .map(|(id, pane)| (id, pane.as_ref()))
            .collect();
        candidates.sort_by(|(a_id, a), (b_id, b)| {
            if let Some(order) = &self.order {
                let rank = |id: &PaneId| {
                    order
                        .iter()
                        .position(|candidate| candidate == id)
                        .unwrap_or(usize::MAX)
                };
                let ordering = rank(a_id).cmp(&rank(b_id));
                if !ordering.is_eq() {
                    return ordering;
                }
            }
            let a_logical_position = a.position_and_size().logical_position;
            let b_logical_position = b.position_and_size().logical_position;
            if a_logical_position != b_logical_position {
                a_logical_position.cmp(&b_logical_position)
            } else {
                a_id.cmp(b_id)
            }
        });
        candidates
    }
    fn find_pane_id_with_same_contents(
        &self,
        candidates: &[(&PaneId, &dyn Pane)],
        run: &Option<Run>,
        pane_logical_position: Option<usize>,
    ) -> Option<PaneId> {
        if run.is_none() {
            return None;
        }
        let panes_with_same_contents = candidates
            .iter()
            .filter(|(_pid, p)| p.invoked_with() == run)
            .collect::<Vec<_>>();
        if panes_with_same_contents.len() > 1 {
            panes_with_same_contents
                .iter()
                .find(|(_pid, p)| p.position_and_size().logical_position == pane_logical_position)
                .map(|(pid, _p)| *pid)
                .copied()
                .or_else(|| {
                    panes_with_same_contents
                        .first()
                        .map(|(pid, _p)| *pid)
                        .copied()
                })
        } else {
            panes_with_same_contents
                .first()
                .map(|(pid, _p)| *pid)
                .copied()
        }
    }
    fn find_pane_id_with_same_logical_position(
        &self,
        candidates: &[(&PaneId, &dyn Pane)],
        logical_position: Option<usize>,
    ) -> Option<PaneId> {
        candidates
            .iter()
            .find(|(_pid, p)| p.position_and_size().logical_position == logical_position)
            .map(|(pid, _p)| *pid)
            .copied()
    }
}

struct PaneApplier<'a> {
    new_focused_pane_id: Option<PaneId>,
    pane_ids_expanded_in_stack: Vec<PaneId>,
    tiled_panes: &'a mut TiledPanes,
    floating_panes: &'a mut FloatingPanes,
    senders: ThreadSenders,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
}

impl<'a> PaneApplier<'a> {
    pub fn new(
        tiled_panes: &'a mut TiledPanes,
        floating_panes: &'a mut FloatingPanes,
        senders: &ThreadSenders,
        character_cell_size: &Rc<RefCell<Option<SizeInPixels>>>,
    ) -> Self {
        PaneApplier {
            new_focused_pane_id: None,
            pane_ids_expanded_in_stack: vec![],
            tiled_panes,
            floating_panes,
            senders: senders.clone(),
            character_cell_size: character_cell_size.clone(),
        }
    }
    pub fn apply_position_and_size_to_tiled_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        position_and_size: PaneGeom,
        layout: TiledPaneLayout,
    ) {
        self.apply_layout_properties_to_pane(&mut pane, &layout, Some(position_and_size));
        if layout.focus.unwrap_or(false) {
            self.new_focused_pane_id = Some(pane.pid());
        }
        if layout.is_expanded_in_stack {
            self.pane_ids_expanded_in_stack.push(pane.pid());
        }
        let _ = resize_pty!(pane, self.os_api, self.senders, self.character_cell_size);
        self.tiled_panes
            .add_pane_with_existing_geom(pane.pid(), pane);
    }
    fn apply_prepared_floating_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        floating_panes_layout: FloatingPaneLayout,
        position_and_size: PaneGeom,
    ) {
        if let Some(pane_title) = floating_panes_layout.name.as_ref() {
            pane.set_title(pane_title.into());
        }
        if floating_panes_layout.focus.unwrap_or(false) {
            self.new_focused_pane_id = Some(pane.pid());
        }
        if let Some(should_be_borderless) = floating_panes_layout.borderless {
            pane.set_borderless(should_be_borderless);
        }
        self.apply_position_and_size_to_floating_pane(pane, position_and_size);
    }
    pub fn apply_position_and_size_to_floating_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        position_and_size: PaneGeom,
    ) {
        pane.set_geom(position_and_size);
        let _ = resize_pty!(pane, self.os_api, self.senders, self.character_cell_size);
        self.floating_panes.add_pane(pane.pid(), pane);
    }

    pub fn handle_remaining_tiled_pane_ids(
        &mut self,
        remaining_pane_ids: Vec<PaneId>,
        mut existing_tab_state: ExistingTabState,
        mut last_logical_position: Option<usize>,
        client_id: Option<ClientId>,
    ) {
        for pane_id in remaining_pane_ids {
            if let Some(pane) = existing_tab_state.remove_pane(&pane_id) {
                if let Some(pane) = self.tiled_panes.insert_pane(pane.pid(), pane, client_id) {
                    // A pane the tab already holds, for which the layout being
                    // applied has no room. It cannot be parked from here -
                    // `LayoutApplier` is handed the two pane collections and not
                    // the tab's suppressed ones - so this is the one place a
                    // pane is still dropped. Said out loud rather than silently,
                    // because the process it was running outlives it.
                    log::error!(
                        "No room in the layout for pane {:?}, dropping it",
                        pane.pid()
                    );
                }
                if let Some(l) = last_logical_position.take() {
                    self.tiled_panes.set_pane_logical_position(pane_id, l);
                    last_logical_position = Some(l + 1);
                }
            }
        }
    }
    pub fn finalize_tiled_state(&mut self) {
        // do some housekeeping to apply various layout properties to panes
        for pane_id in &self.pane_ids_expanded_in_stack {
            self.tiled_panes.expand_pane_in_stack(*pane_id);
        }
        if let Some(pane_id) = self.new_focused_pane_id {
            self.tiled_panes.focus_pane_for_all_clients(pane_id);
        }
        self.tiled_panes.reapply_pane_focus();
    }
    pub fn finalize_floating_panes_state(&mut self) {
        // do some housekeeping to apply various layout properties to panes
        if let Some(pane_id) = self.new_focused_pane_id {
            self.floating_panes.focus_pane_for_all_clients(pane_id);
        }
        self.floating_panes.reapply_pane_focus();
    }
    fn apply_layout_properties_to_pane(
        &self,
        pane: &mut Box<dyn Pane>,
        layout: &TiledPaneLayout,
        position_and_size: Option<PaneGeom>,
    ) {
        if let Some(position_and_size) = position_and_size {
            pane.set_configured_rows(position_and_size.rows);
            pane.set_geom(position_and_size);
        }
        if let Some(should_be_borderless) = layout.borderless {
            pane.set_borderless(should_be_borderless);
        }
        if let Some(pane_title) = layout.name.as_ref() {
            pane.set_title(pane_title.into());
        }
    }
}

/// Expand overflow with the same largest-pane selection used by ordinary insertion.
/// Every slot is resolved before draining any live pane collection.
pub fn arrangement_layout(
    mut layout: TiledPaneLayout,
    size: &Size,
    count: usize,
    ratio: Option<usize>,
) -> Result<TiledPaneLayout> {
    fn split_leaf(
        layout: &mut TiledPaneLayout,
        wanted: usize,
        cursor: &mut usize,
        direction: yinz_types::geometry::SplitDirection,
    ) -> bool {
        if layout.children.is_empty() {
            let index = *cursor;
            *cursor += 1;
            if index != wanted {
                return false;
            }
            layout.children_split_direction = direction;
            layout.children = vec![TiledPaneLayout::default(); 2];
            true
        } else {
            layout
                .children
                .iter_mut()
                .any(|child| split_leaf(child, wanted, cursor, direction))
        }
    }
    fn grow_stack(layout: &mut TiledPaneLayout, wanted: usize, cursor: &mut usize) -> bool {
        let start = *cursor;
        if layout.children_are_stacked && (start..start + layout.pane_count()).contains(&wanted) {
            layout.children.push(TiledPaneLayout::default());
            return true;
        }
        if layout.children.is_empty() {
            *cursor += 1;
            return false;
        }
        layout
            .children
            .iter_mut()
            .any(|child| grow_stack(child, wanted, cursor))
    }
    while layout.pane_count() < count {
        let positions = layout
            .position_panes_in_space(&PaneGeom::from(size), None, false, false)
            .context("failed to position panes in layout")?;
        let split = crate::panes::room_in_geometries(
            positions.iter().enumerate().map(|(index, (_, geom))| {
                (
                    index,
                    *geom,
                    crate::pane::MIN_TERMINAL_WIDTH,
                    crate::pane::MIN_TERMINAL_HEIGHT,
                )
            }),
            ratio,
        );
        if let Some((index, direction)) = split {
            if !split_leaf(&mut layout, index, &mut 0, direction) {
                return Err(anyhow!("could not expand project layout"));
            }
        } else {
            let index = positions
                .iter()
                .position(|(_, geom)| {
                    geom.is_stacked()
                        && !geom.rows.is_fixed()
                        && geom.rows.as_usize() > crate::pane::MIN_TERMINAL_HEIGHT
                })
                .context("no room for excess panes in project layout")?;
            if !grow_stack(&mut layout, index, &mut 0) {
                return Err(anyhow!("could not expand project stack"));
            }
        }
    }
    let positions = layout
        .position_panes_in_space(&PaneGeom::from(size), Some(count), false, false)
        .context("failed to position panes in layout")?;
    if positions.iter().any(|(_, geom)| {
        geom.cols.as_usize() < crate::pane::MIN_TERMINAL_WIDTH
            || ((!geom.is_stacked() || !geom.rows.is_fixed())
                && geom.rows.as_usize() < crate::pane::MIN_TERMINAL_HEIGHT)
    }) {
        return Err(anyhow!("project layout does not fit this tab"));
    }
    Ok(layout)
}

#[cfg(test)]
#[path = "unit/layout_applier_viewport_tests.rs"]
mod viewport_tests;
