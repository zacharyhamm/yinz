use super::*;
use yinz_types::pane_tabs::*;

impl Screen {
    pub(crate) fn report_pane_tabs(&mut self) -> Result<()> {
        let tabs: Vec<_> = self.tabs.values().flat_map(Tab::pane_tabs_infos).collect();
        self.overlay_tab_completions
            .retain_mut(|(id, index, completion)| {
                let slot = tabs
                    .iter()
                    .find(|info| info.id == *id)
                    .and_then(|info| info.tabs.get(*index));
                match slot {
                    Some(PaneTabInfo {
                        plugin_id: Some(plugin),
                        ..
                    }) => {
                        completion.set_affected_pane_id(PaneId::Plugin(*plugin));
                    },
                    Some(slot) if matches!(slot.state, PaneTabLoadState::Loading) => return true,
                    Some(PaneTabInfo {
                        state: PaneTabLoadState::Failed(error),
                        ..
                    }) => {
                        completion.set_exit_status(1);
                        completion.set_error_message(error.clone());
                    },
                    _ => {
                        completion.set_exit_status(1);
                        completion.set_error_message("Overlay tab closed before loading".into());
                    },
                }
                false
            });
        if tabs != self.reported_pane_tabs {
            self.reported_pane_tabs = tabs.clone();
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(vec![(
                    None,
                    None,
                    Event::PaneTabsUpdate(tabs),
                )]))
                .context("failed to report pane tabs")?;
        }
        Ok(())
    }

    pub(crate) fn pane_tabs_action(&mut self, step: i8, client: ClientId) -> Result<()> {
        let Some(pane) = self.get_active_pane_id(&client) else {
            return Ok(());
        };
        let Some(tab) = self.tab_containing_pane_mut(pane) else {
            return Ok(());
        };
        let Some(id) = tab.pane_tabs_of(pane) else {
            return Ok(());
        };
        if step == 0 {
            tab.focus_pane_tabs(id, true, client)?;
        } else if let Some(info) = tab.pane_tabs_info(id) {
            let index = info
                .active
                .saturating_add_signed(isize::from(step))
                .min(info.tabs.len() - 1);
            tab.select_pane_tab(id, index, client)?;
        }
        self.log_and_report_session_state()
            .context("failed to report pane tab action")
    }

    pub(crate) fn pane_tabs_command(
        &mut self,
        command: PaneTabsCommand,
        client: ClientId,
    ) -> Result<Option<PaneTabsInfo>> {
        let id = match command {
            PaneTabsCommand::Mount {
                pane_id,
                layout,
                existing,
            } => {
                self.tab_containing_pane_mut(pane_id)
                    .context("pane tabs mount pane not found")?
                    .mount_pane_tabs(pane_id, layout, existing, client)?
                    .id
            },
            PaneTabsCommand::Select { id, index } => {
                self.pane_tabs_tab_mut(id)?
                    .select_pane_tab(id, index, client)?;
                id
            },
            PaneTabsCommand::Focus { id, bar } => {
                self.pane_tabs_tab_mut(id)?
                    .focus_pane_tabs(id, bar, client)?;
                id
            },
            PaneTabsCommand::Retry { id, index } => {
                let tab = self.pane_tabs_tab_mut(id)?;
                tab.close_pane_tab_child(id, index)?;
                tab.select_pane_tab(id, index, client)?;
                id
            },
            PaneTabsCommand::CloseChild { id, index } => {
                self.pane_tabs_tab_mut(id)?
                    .close_pane_tab_child(id, index)?;
                id
            },
            PaneTabsCommand::Close { id } => {
                self.pane_tabs_tab_mut(id)?.close_pane_tabs(id)?;
                return Ok(None);
            },
        };
        let info = self.pane_tabs_tab_mut(id)?.pane_tabs_info(id);
        self.log_and_report_session_state()
            .context("failed to report pane tabs state")?;
        Ok(info)
    }

    fn pane_tabs_tab_mut(&mut self, id: PaneTabsId) -> Result<&mut Tab> {
        self.tabs
            .values_mut()
            .find(|tab| tab.pane_tabs_info(id).is_some())
            .context("unknown pane tabs")
    }

    pub(crate) fn pane_tab_loaded(
        &mut self,
        id: PaneTabsId,
        index: usize,
        generation: u64,
        result: std::result::Result<(PluginId, RunPluginOrAlias), String>,
    ) -> Result<()> {
        if self.pending_overlay_tab_loaded(id, &result)? {
            return Ok(());
        }
        if let Ok(tab) = self.pane_tabs_tab_mut(id) {
            tab.pane_tab_loaded(id, index, generation, result)?;
            self.report_pane_tabs()?;
            self.log_and_report_session_state()
                .context("failed to report loaded pane tab")?;
        } else if let Ok((plugin_id, _)) = result {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Unload(plugin_id))
                .context("failed to unload closed pane tabs child")?;
        }
        Ok(())
    }
}
