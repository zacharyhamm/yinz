use super::Screen;
use crate::{plugins::PluginInstruction, route::NotificationEnd, ClientId};
use std::collections::HashSet;
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    input::actions::Action,
    internal_ids::{TabId, TabPosition},
    tab_bar::TabGroupInfo,
};

pub(super) struct TabGroup {
    pub name: String,
    pub preferred: Option<TabId>,
    restored_preferred: Option<TabId>,
}

impl Screen {
    pub(super) fn tab_group_info(&self, tab: TabId) -> Option<TabGroupInfo> {
        let id = *self.tab_group_ids.get(&tab)?;
        Some(TabGroupInfo {
            id,
            name: self.tab_groups.get(&id)?.name.clone(),
        })
    }

    pub(super) fn group_for_client(&self, client: Option<ClientId>) -> u32 {
        client
            .and_then(|c| self.active_tab_ids.get(&c))
            .or_else(|| self.active_tab_ids.values().next())
            .and_then(|tab| self.tab_group_ids.get(tab))
            .copied()
            .or_else(|| self.tab_groups.keys().next().copied())
            .unwrap_or(0)
    }

    pub(super) fn assign_tab_group(&mut self, tab: TabId, client: Option<ClientId>) {
        let id = self.group_for_client(client);
        self.tab_groups.entry(id).or_insert_with(|| TabGroup {
            name: "Default".into(),
            preferred: None,
            restored_preferred: None,
        });
        self.tab_group_ids.insert(tab, id);
    }

    pub(super) fn restore_tab_group(
        &mut self,
        tab: TabId,
        layout: &yinz_types::input::layout::TiledPaneLayout,
    ) {
        if let Some(group) = &layout.tab_group {
            let restored = self.tab_groups.entry(group.id).or_insert_with(|| TabGroup {
                name: group.name.clone(),
                preferred: None,
                restored_preferred: None,
            });
            restored.name = yinz_types::text::clean_string_from_control_and_linebreak(&group.name);
            if layout.group_focused {
                restored.preferred = Some(tab);
                restored.restored_preferred = Some(tab);
            }
            self.tab_group_ids.insert(tab, group.id);
            self.next_tab_group_id = self.next_tab_group_id.max(u64::from(group.id) + 1);
            self.prune_tab_groups();
        }
    }

    pub(super) fn finish_group_restore(&mut self) {
        if self.has_pending_tabs() {
            return;
        }
        for (group_id, group) in &mut self.tab_groups {
            let Some(tab) = group
                .restored_preferred
                .take()
                .filter(|tab| self.tabs.contains_key(tab))
            else {
                continue;
            };
            group.preferred = Some(tab);
            // Initial layout focus is provisional until all restored tabs have loaded.
            for (client, active) in &self.active_tab_ids {
                if self.tab_group_ids.get(active) != Some(group_id) {
                    let history = self.tab_history.entry(*client).or_default();
                    history.retain(|id| *id != tab);
                    history.push(tab);
                }
            }
        }
    }

    pub(super) fn prune_tab_groups(&mut self) {
        self.tab_group_ids
            .retain(|tab, _| self.tabs.contains_key(tab));
        for history in self.tab_history.values_mut() {
            history.retain(|tab| self.tabs.contains_key(tab));
        }
        self.tab_groups
            .retain(|id, _| self.tab_group_ids.values().any(|group| group == id));
    }

    pub(super) fn group_tabs(&self, group: u32) -> Vec<TabId> {
        let mut tabs: Vec<_> = self
            .tabs
            .values()
            .filter(|tab| self.tab_group_ids.get(&tab.id) == Some(&group))
            .collect();
        tabs.sort_by_key(|tab| tab.position);
        tabs.into_iter().map(|tab| tab.id).collect()
    }

    pub(super) fn preferred_group_tab(
        &self,
        group: u32,
        client: ClientId,
        require_selectable: bool,
    ) -> Option<TabId> {
        let valid = |id: &TabId| {
            self.tab_group_ids.get(id) == Some(&group)
                && self.tabs.get(id).is_some_and(|tab| {
                    !tab.is_pending() && (!require_selectable || tab.has_selectable_panes())
                })
        };
        self.active_tab_ids
            .get(&client)
            .filter(|id| valid(id))
            .copied()
            .or_else(|| {
                self.tab_history
                    .get(&client)?
                    .iter()
                    .rev()
                    .find(|id| valid(id))
                    .copied()
            })
            .or_else(|| self.tab_groups.get(&group)?.preferred.filter(valid))
            .or_else(|| self.group_tabs(group).into_iter().find(valid))
    }

    pub(super) fn adjacent_group_tab(&self, tab: TabId, forward: bool) -> Option<TabPosition> {
        let group = *self.tab_group_ids.get(&tab)?;
        let tabs = self.group_tabs(group);
        let index = tabs.iter().position(|id| *id == tab)?;
        let next = if forward {
            (index + 1) % tabs.len()
        } else {
            (index + tabs.len() - 1) % tabs.len()
        };
        self.tabs.get(&tabs[next]).map(|tab| tab.position)
    }

    pub(super) fn closed_tab_group_destination(&self, client: ClientId) -> Option<TabId> {
        let group = self.group_for_client(Some(client));
        self.preferred_group_tab(group, client, true).or_else(|| {
            self.tab_groups
                .range((std::ops::Bound::Excluded(group), std::ops::Bound::Unbounded))
                .chain(self.tab_groups.range(..group).rev())
                .find_map(|(id, _)| self.preferred_group_tab(*id, client, true))
        })
    }

    pub(super) fn move_group_tab(&mut self, tab: TabId, forward: bool) -> Result<()> {
        let group = *self.tab_group_ids.get(&tab).context("tab group missing")?;
        let tabs = self.group_tabs(group);
        let index = tabs
            .iter()
            .position(|id| *id == tab)
            .context("tab missing")?;
        let positions: Vec<_> = tabs.iter().map(|id| self.tabs[id].position).collect();
        let mut reordered = tabs;
        if forward && index + 1 == reordered.len() {
            reordered.rotate_right(1);
        } else if !forward && index == 0 {
            reordered.rotate_left(1);
        } else {
            reordered.swap(index, if forward { index + 1 } else { index - 1 });
        }
        for (id, position) in reordered.into_iter().zip(positions) {
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.position = position;
            }
        }
        self.log_and_report_session_state()
            .context("move tab within group")
    }

    pub(super) fn tab_group_action(
        &mut self,
        action: Action,
        client: ClientId,
        completion: Option<NotificationEnd>,
        tab_switches: &mut HashSet<(TabId, ClientId)>,
    ) -> Result<()> {
        let client = self.client_id(client).unwrap_or(client);
        match action {
            Action::NewTabGroup => {
                let id =
                    u32::try_from(self.next_tab_group_id).context("tab group IDs exhausted")?;
                self.next_tab_group_id += 1;
                let mut number = 1u64;
                while self
                    .tab_groups
                    .values()
                    .any(|group| group.name == format!("Group {number}"))
                {
                    number += 1;
                }
                let tab = self.get_new_tab_id();
                self.new_tab(
                    tab,
                    (
                        self.default_layout.swap_tiled_layouts.clone(),
                        self.default_layout.swap_floating_layouts.clone(),
                    ),
                    None,
                    Some(client),
                )
                .context("create first tab in group")?;
                self.tab_groups.insert(
                    id,
                    TabGroup {
                        name: format!("Group {number}"),
                        preferred: Some(tab),
                        restored_preferred: None,
                    },
                );
                self.tab_group_ids.insert(tab, id);
                self.prune_tab_groups();
                if let Err(error) = self.bus.senders.send_to_plugin(PluginInstruction::NewTab(
                    None,
                    self.default_shell_action.clone(),
                    None,
                    None,
                    tab,
                    None,
                    false,
                    true,
                    client,
                    completion,
                )) {
                    self.close_tab_by_id(tab)
                        .context("remove undispatched group tab")
                        .non_fatal();
                    return Err(error).context("spawn first tab in group");
                }
                return Ok(());
            },
            Action::RenameTabGroup { id, name } => {
                self.tab_groups
                    .get_mut(&id)
                    .context("tab group no longer exists")?
                    .name = yinz_types::text::clean_string_from_control_and_linebreak(&name);
            },
            Action::CloseTabGroup { id } => {
                if !self.tab_groups.contains_key(&id) {
                    return Err(anyhow!("tab group no longer exists"));
                }
                let has_destination = self.tabs.values().any(|tab| {
                    self.tab_group_ids.get(&tab.id) != Some(&id)
                        && !tab.is_pending()
                        && tab.has_selectable_panes()
                });
                if !has_destination && self.quit_is_guarded() {
                    self.confirm_quit(client, true, completion)
                        .context("confirm closing last tab group")?;
                    return Ok(());
                }
                for tab in self.group_tabs(id) {
                    self.close_tab_by_id(tab).context("close tab group")?;
                }
                let clients: Vec<_> = self.active_tab_ids.keys().copied().collect();
                for client in clients {
                    self.sync_scroll_mode_on_focus(client)
                        .context("sync scroll mode after closing tab group")?;
                }
            },
            Action::SwitchTabGroup { id } => {
                let tab = self
                    .preferred_group_tab(id, client, false)
                    .context("tab group has no loaded tab")?;
                self.navigate_to_tab(self.tabs[&tab].position, None, true, client)
                    .context("switch tab group")?;
            },
            Action::GoToNextTabGroup | Action::GoToPreviousTabGroup => {
                let groups: Vec<_> = self.tab_groups.keys().copied().collect();
                if groups.is_empty() {
                    return Ok(());
                }
                let active = self.group_for_client(Some(client));
                let index = groups.iter().position(|id| *id == active).unwrap_or(0);
                let step = if action == Action::GoToNextTabGroup {
                    1
                } else {
                    groups.len() - 1
                };
                for offset in 1..=groups.len() {
                    let id = groups[(index + step * offset) % groups.len()];
                    if let Some(tab) = self.preferred_group_tab(id, client, false) {
                        self.navigate_to_tab(self.tabs[&tab].position, None, true, client)
                            .context("switch adjacent group")?;
                        break;
                    }
                }
            },
            Action::GoToTabInGroup { index } => {
                let tabs = self.group_tabs(self.group_for_client(Some(client)));
                if let Some(tab) = tabs
                    .get(index.position().get())
                    .and_then(|id| self.tabs.get(id))
                {
                    if self.has_pending_tabs() {
                        tab_switches.insert((tab.id, client));
                    } else {
                        self.navigate_to_tab(tab.position, None, true, client)
                            .context("switch group tab")?;
                    }
                }
            },
            _ => return Err(anyhow!("invalid tab group action")),
        }
        self.log_and_report_session_state()
            .context("report tab groups")
    }
}
