use super::*;
use yinz_types::{input::layout::RunPluginLocation, pane_size::Constraint};

impl Tab {
    pub fn update_tab_groups(&mut self, enabled: bool) -> Result<()> {
        self.default_mode_info.tab_groups = Some(enabled);
        for mode in self.mode_info.borrow_mut().values_mut() {
            mode.tab_groups = Some(enabled);
        }
        self.sync_tab_bar_height()
    }

    pub fn tab_bar_serialization_geom(&self, id: PaneId, mut geom: PaneGeom) -> PaneGeom {
        let original_y = geom.y;
        for bar in &self.collapsed_tab_bars {
            if let Some(pane) = self.tiled_panes.get_pane(*bar) {
                let bar_geom = pane.position_and_size();
                if original_y >= bar_geom.y + bar_geom.rows.as_usize() {
                    geom.y += 1;
                }
                if *bar == id {
                    // Export the declared strip and shift its neighbours to keep the layout contiguous.
                    geom.rows = Dimension::fixed(2);
                }
            }
        }
        geom
    }

    pub(super) fn restore_tab_bar_height(&mut self) -> Result<()> {
        self.set_tab_bar_height(true)
    }

    pub(super) fn sync_tab_bar_height(&mut self) -> Result<()> {
        self.set_tab_bar_height(self.default_mode_info.tab_groups != Some(false))
    }

    fn set_tab_bar_height(&mut self, enabled: bool) -> Result<()> {
        if enabled && self.collapsed_tab_bars.is_empty() {
            return Ok(());
        }
        let changes: Vec<_> = self
            .tiled_panes
            .get_panes()
            .filter_map(|(id, pane)| {
                let mut geom = pane.position_and_size();
                let restore = self.collapsed_tab_bars.contains(id)
                    && geom.rows.constraint == Constraint::Fixed(1);
                let collapse = !enabled
                    && pane.borderless()
                    && geom.x == 0
                    && geom.cols.as_usize() == self.size.cols
                    && geom.rows.constraint == Constraint::Fixed(2)
                    && matches!(pane.invoked_with(), Some(Run::Plugin(plugin)) if plugin.get_run_plugin().is_some_and(|plugin|
                        matches!(plugin.location, RunPluginLocation::Yinz(tag) if tag.to_string() == "tab-bar")));
                if (enabled && restore) || collapse {
                    // Preserve cell coordinates until the solver has walked the existing neighbours.
                    geom.rows.constraint = Constraint::Fixed(if enabled { 2 } else { 1 });
                    Some((*id, geom))
                } else {
                    None
                }
            })
            .collect();
        self.collapsed_tab_bars
            .retain(|id| self.tiled_panes.panes_contain(id));
        if changes.is_empty() {
            return Ok(());
        }
        let fullscreen = self.tiled_panes.fullscreen_pane_id();
        let covers_ui = self.tiled_panes.fullscreen_covers_ui();
        if fullscreen.is_some() {
            self.tiled_panes.unset_fullscreen();
        }
        let floating_fullscreen = self.floating_panes.fullscreen_pane_id();
        let floating_covers_ui = self.floating_panes.fullscreen_covers_ui();
        if floating_fullscreen.is_some() {
            self.floating_panes.unset_fullscreen();
        }
        for (id, geom) in changes {
            self.tiled_panes.set_geom_for_pane_with_id(&id, geom);
            if enabled {
                self.collapsed_tab_bars.remove(&id);
            } else {
                self.collapsed_tab_bars.insert(id);
            }
        }
        self.tiled_panes.force_resize();
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            &mut self.tiled_panes,
            self.pane_frame_style,
        );
        if let Some(id) = fullscreen {
            if covers_ui {
                self.tiled_panes.toggle_pane_no_ui_fullscreen(id);
            } else {
                self.tiled_panes.toggle_pane_fullscreen(id);
            }
        }
        if let Some(id) = floating_fullscreen {
            if floating_covers_ui {
                self.floating_panes.toggle_pane_no_ui_fullscreen(id);
            } else {
                self.floating_panes.toggle_pane_fullscreen(id);
            }
        }
        self.refresh_overlay_expansion();
        self.resize_all_stack_list_hidden_members();
        self.set_should_clear_display_before_rendering();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .context("apply tab-bar height change")?;
        Ok(())
    }
}
