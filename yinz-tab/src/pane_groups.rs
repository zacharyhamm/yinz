use std::collections::{HashMap, HashSet};
use yinz_types::internal_ids::TabId;
#[cfg(test)]
use yinz_types::internal_ids::{PluginId, TerminalId};

use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::geometry::PercentOrFixed;
use yinz_types::pane_size::Size;

use crate::{panes::PaneId, pty::PtyInstruction, thread_bus::ThreadSenders, ClientId};

pub struct PaneGroups {
    panes_in_group: HashMap<ClientId, Vec<PaneId>>,
    senders: ThreadSenders,
}

impl std::fmt::Debug for PaneGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneGroups")
            .field("panes_in_group", &self.panes_in_group)
            .finish_non_exhaustive()
    }
}

impl PaneGroups {
    pub fn new(senders: ThreadSenders) -> Self {
        PaneGroups {
            panes_in_group: HashMap::new(),
            senders,
        }
    }
    pub fn clone_inner(&self) -> HashMap<ClientId, Vec<PaneId>> {
        self.panes_in_group.clone()
    }
    pub fn get_client_pane_group(&self, client_id: &ClientId) -> HashSet<PaneId> {
        self.panes_in_group
            .get(client_id)
            .map(|p| p.iter().copied().collect())
            .unwrap_or_default()
    }
    pub fn clear_pane_group(&mut self, client_id: &ClientId) {
        if let Some(p) = self.panes_in_group.get_mut(client_id) {
            p.clear()
        }
    }
    /// Forget a client that has gone, group and all.
    pub fn remove_client(&mut self, client_id: &ClientId) {
        self.panes_in_group.remove(client_id);
    }
    pub fn toggle_pane_id_in_group(
        &mut self,
        pane_id: PaneId,
        screen_size: Size,
        client_id: &ClientId,
    ) {
        let previous_groups = self.clone_inner();
        let client_pane_group = self.panes_in_group.entry(*client_id).or_default();
        if client_pane_group.contains(&pane_id) {
            client_pane_group.retain(|p| p != &pane_id);
        } else {
            client_pane_group.push(pane_id);
        };
        if self.should_launch_plugin(&previous_groups, client_id) {
            self.launch_plugin(screen_size, client_id);
        }
    }
    pub fn add_pane_id_to_group(
        &mut self,
        pane_id: PaneId,
        screen_size: Size,
        client_id: &ClientId,
    ) {
        let previous_groups = self.clone_inner();
        let client_pane_group = self.panes_in_group.entry(*client_id).or_default();
        if !client_pane_group.contains(&pane_id) {
            client_pane_group.push(pane_id);
        }
        if self.should_launch_plugin(&previous_groups, client_id) {
            self.launch_plugin(screen_size, client_id);
        }
    }
    pub fn group_and_ungroup_panes(
        &mut self,
        pane_ids_to_group: Vec<PaneId>,
        pane_ids_to_ungroup: Vec<PaneId>,
        screen_size: Size,
        client_id: &ClientId,
    ) {
        let previous_groups = self.clone_inner();
        let client_pane_group = self.panes_in_group.entry(*client_id).or_default();
        for pane_id in pane_ids_to_group {
            if !client_pane_group.contains(&pane_id) {
                client_pane_group.push(pane_id);
            }
        }
        client_pane_group.retain(|p| !pane_ids_to_ungroup.contains(p));
        if self.should_launch_plugin(&previous_groups, client_id) {
            self.launch_plugin(screen_size, client_id);
        }
    }
    pub fn group_and_ungroup_panes_for_all_clients(
        &mut self,
        pane_ids_to_group: Vec<PaneId>,
        pane_ids_to_ungroup: Vec<PaneId>,
        screen_size: Size,
    ) {
        let previous_groups = self.clone_inner();
        let mut should_launch = false;
        let all_connected_clients: Vec<ClientId> = self.panes_in_group.keys().copied().collect();

        for client_id in &all_connected_clients {
            let client_pane_group = self.panes_in_group.entry(*client_id).or_default();
            for pane_id in &pane_ids_to_group {
                if !client_pane_group.contains(pane_id) {
                    client_pane_group.push(*pane_id);
                }
            }
            client_pane_group.retain(|p| !pane_ids_to_ungroup.contains(p));

            if self.should_launch_plugin(&previous_groups, client_id) {
                should_launch = true;
            }
        }

        if should_launch && let Some(first_client) = all_connected_clients.first() {
            self.launch_plugin(screen_size, first_client);
        }
    }
    pub fn override_groups_with(&mut self, new_pane_groups: HashMap<ClientId, Vec<PaneId>>) {
        self.panes_in_group = new_pane_groups;
    }
    fn should_launch_plugin(
        &self,
        previous_groups: &HashMap<ClientId, Vec<PaneId>>,
        client_id: &ClientId,
    ) -> bool {
        let mut should_launch = false;
        for (client_id, previous_panes) in previous_groups {
            let previous_panes_has_panes = !previous_panes.is_empty();
            let current_panes_has_panes = self
                .panes_in_group
                .get(client_id)
                .map(|g| !g.is_empty())
                .unwrap_or(false);
            if !previous_panes_has_panes && current_panes_has_panes {
                should_launch = true;
            }
        }
        should_launch || previous_groups.get(client_id).is_none()
    }
    fn launch_plugin(&self, screen_size: Size, client_id: &ClientId) {
        if let Ok(run_plugin) = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "yinz:multiple-select",
            &None,
            None,
            None,
        ) {
            let tab_index = 1;
            let size = Size::default();
            let should_float = Some(true);
            let should_be_opened_in_place = false;
            let pane_title = None;
            let skip_cache = false;
            let cwd = None;
            let should_focus_plugin = Some(false);
            let width_30_percent = (screen_size.cols as f64 * 0.3) as usize;
            let height_30_percent = (screen_size.rows as f64 * 0.3) as usize;
            let width = std::cmp::max(width_30_percent, 48);
            let height = std::cmp::max(height_30_percent, 10);
            let y_position = screen_size.rows.saturating_sub(height + 2);
            let floating_pane_coordinates = FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(2)),
                y: Some(PercentOrFixed::Fixed(y_position)),
                width: Some(PercentOrFixed::Fixed(width)),
                height: Some(PercentOrFixed::Fixed(height)),
                pinned: Some(true),
                borderless: Some(false),
            };
            let _ = self.senders.send_to_pty(PtyInstruction::FillPluginCwd(
                should_float,
                should_be_opened_in_place,
                false, // close_replaced_pane
                pane_title,
                run_plugin,
                TabId::new(tab_index),
                None,
                *client_id,
                size,
                skip_cache,
                cwd,
                should_focus_plugin,
                Some(floating_pane_coordinates),
                None,
                None,
            ));
        }
    }
}

#[cfg(test)]
#[path = "unit/pane_groups_tests.rs"]
mod tests;
