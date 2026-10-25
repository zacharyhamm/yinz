use super::thread_main::PendingEvents;
use super::Screen;
use yinz_bus::pty::PtyInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::{internal_ids::ClientId, multipane::*, pane_id::PaneId};

impl Screen {
    pub(super) fn selection_has_multipane(&self, panes: &[PaneId]) -> bool {
        panes.iter().any(|id| {
            self.tab_containing_pane(*id)
                .is_some_and(|tab| tab.multipane_info(*id).is_some())
        })
    }

    fn multipane_member(&self, id: MultipaneId) -> Result<PaneId> {
        self.tabs
            .values()
            .flat_map(|tab| {
                tab.get_all_pane_ids()
                    .into_iter()
                    .filter_map(|pane| tab.multipane_info(pane))
            })
            .find(|info| info.id == id)
            .map(|info| info.children[0])
            .context("unknown multipane")
    }
    fn validate_multipane_children(&self, ids: &[PaneId]) -> Result<()> {
        let mut unique = std::collections::HashSet::new();
        let mut tab_id = None;
        for id in ids {
            if !unique.insert(id) {
                return Err(anyhow!("multipane children must be distinct"));
            }
            let tab = self
                .tab_containing_pane(*id)
                .context("multipane child does not exist")?;
            if tab_id.is_some_and(|previous| previous != tab.id) {
                return Err(anyhow!("multipane children must be on the same tab"));
            }
            tab_id = Some(tab.id);
            tab.validate_multipane_child(*id)?;
        }
        Ok(())
    }
    /// The plugin thread gives up `slot`, a pane it held for a plugin that will not have one
    /// there: a multipane child that declares no pane, or that failed before its declaration
    /// was known. A multipane member leaves its container, and the sibling stays as the
    /// container's only pane, focused for whoever was on the slot. A pane that is no longer a
    /// member (its container closed, or its sibling left first) closes as `ClosePane` closes
    /// it, except that no `pane_exit` hook fires: the slot's held command never ran.
    ///
    /// The pty then frees what the slot held: a held or running terminal's process and
    /// resources, or the plugin that was in it, which a `ReplacedPane::Close` replacement
    /// would have unloaded too. The plugin the slot was held for is never named here; the
    /// plugin thread unloads it, or keeps it running headless, on its own.
    pub(super) fn give_up_slot(&mut self, slot: PaneId, pending: &mut PendingEvents) -> Result<()> {
        let member = self
            .tab_containing_pane(slot)
            .is_some_and(|tab| tab.multipane_info(slot).is_some());
        if !member {
            self.close_pane_without_exit_hook(slot, None, None, None, pending)?;
        } else {
            self.repaint_tab_of_closing_overlay(slot);
            let removed = self
                .tab_containing_pane_mut(slot)
                .is_some_and(|tab| tab.remove_multipane_member(slot));
            if removed {
                self.cleanup_closed_pane(slot);
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(slot, None))
                    .context("failed to free a given-up multipane slot")
                    .non_fatal();
                let clients: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
                for client_id in clients {
                    self.sync_scroll_mode_on_focus(client_id)?;
                }
            } else {
                // It holds the place of a suppressed pane, which an ordinary close restores, or
                // belongs to pane tabs, whose child an ordinary close blanks in place.
                self.close_pane_without_exit_hook(slot, None, None, None, pending)?;
            }
        }
        self.log_and_report_session_state().non_fatal();
        self.retain_only_existing_panes_in_pane_groups();
        self.render()
    }
    pub(crate) fn multipane_command(
        &mut self,
        operation: MultipaneOperation,
        client: ClientId,
    ) -> Result<Option<MultipaneInfo>> {
        let member = match operation {
            MultipaneOperation::Validate(ids) => {
                self.validate_multipane_children(&ids)?;
                return Ok(None);
            },
            MultipaneOperation::Get(id) => self.multipane_member(id)?,
            MultipaneOperation::Create {
                children,
                placement,
                split,
                focus,
                min_cells,
            } => {
                split.validate().map_err(|error| anyhow!(error))?;
                if focus.is_some_and(|slot| slot >= 2) {
                    return Err(anyhow!("invalid multipane focus slot"));
                }
                self.validate_multipane_children(&children)?;
                let tab = self
                    .tab_containing_pane_mut(children[0])
                    .context("multipane tab disappeared")?;
                tab.create_multipane(children, split, min_cells, placement)?;
                if let Some(slot) = focus {
                    self.focus_pane_with_id(children[slot], false, false, client)?;
                }
                children[0]
            },
            MultipaneOperation::Update(request) => {
                yinz_types::data::validate_background_opacity(request.background_opacity)
                    .map_err(|error| anyhow!(error))?;
                if let Some(split) = request.split {
                    split.validate().map_err(|error| anyhow!(error))?;
                }
                let member = self.multipane_member(request.id)?;
                let tab = self
                    .tab_containing_pane_mut(member)
                    .context("multipane tab disappeared")?;
                if let Some(opacity) = request.background_opacity {
                    tab.set_overlay_background_opacity(member, opacity)?;
                }
                if let Some(coordinates) = request.coordinates {
                    tab.change_overlay_coordinates_batch(
                        vec![(member, coordinates)],
                        request.override_user_placement,
                    )?;
                }
                if let Some(split) = request.split {
                    tab.update_multipane_split(request.id, split)?;
                }
                member
            },
            MultipaneOperation::Replace { id, slot, child } => {
                if slot >= 2 {
                    return Err(anyhow!("invalid multipane child slot"));
                }
                let member = self.multipane_member(id)?;
                let info = self
                    .tab_containing_pane(member)
                    .and_then(|tab| tab.multipane_info(member))
                    .context("multipane disappeared")?;
                if info.children[slot] == child {
                    return Ok(Some(info));
                }
                self.validate_multipane_children(&[child])?;
                self.replace_pane_with_existing_pane(info.children[slot], child, false, None)?;
                self.multipane_member(id)?
            },
        };
        self.tab_containing_pane(member)
            .and_then(|tab| tab.multipane_info(member))
            .map(Some)
            .context("multipane operation did not retain its container")
    }
}
