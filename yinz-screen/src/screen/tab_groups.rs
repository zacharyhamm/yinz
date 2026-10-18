use super::Screen;
use crate::{plugins::PluginInstruction, route::NotificationEnd, ClientId};
use std::collections::{BTreeSet, HashSet};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    input::actions::Action,
    internal_ids::{TabId, TabPosition},
    tab_bar::TabGroupInfo,
};

#[derive(serde::Serialize)]
struct TabGroupRecord {
    id: u32,
    name: String,
    active: bool,
    tab_ids: Vec<TabId>,
}

pub(super) struct TabGroup {
    pub name: String,
    pub preferred: Option<TabId>,
    restored_preferred: Option<TabId>,
}

impl Screen {
    pub(super) fn tab_groups_enabled(&self) -> bool {
        self.default_mode_info.tab_groups != Some(false)
    }

    pub(super) fn navigation_tabs(&self, tab: TabId) -> Vec<TabId> {
        if self.tab_groups_enabled() {
            return self
                .tab_group_ids
                .get(&tab)
                .map(|id| self.group_tabs(*id))
                .unwrap_or_default();
        }
        let mut tabs: Vec<_> = self.tabs.values().collect();
        tabs.sort_by_key(|tab| tab.position);
        tabs.into_iter().map(|tab| tab.id).collect()
    }

    pub(super) fn sync_tab_bar_rows(&mut self) -> Result<()> {
        let count = self.tab_groups.len();
        for tab in self.tabs.values_mut().filter(|tab| !tab.is_pending()) {
            tab.set_tab_group_count(count)
                .context("sync tab bar height")?;
        }
        Ok(())
    }

    pub(super) fn tab_group_info(&self, tab: TabId) -> Option<TabGroupInfo> {
        let id = *self.tab_group_ids.get(&tab)?;
        Some(TabGroupInfo {
            id,
            name: self.tab_group_name(id)?,
        })
    }

    pub(super) fn tab_group_name(&self, id: u32) -> Option<String> {
        let group = self.tab_groups.get(&id)?;
        let mut seen = HashSet::new();
        let projects: Vec<_> = self
            .group_tabs(id)
            .into_iter()
            .flat_map(|tab| {
                self.tabs[&tab]
                    .get_all_pane_ids()
                    .into_iter()
                    .filter_map(|pane| self.pane_projects.get(&pane))
                    .map(|name| yinz_types::text::clean_string_from_control_and_linebreak(name))
                    .collect::<BTreeSet<_>>()
            })
            .filter(|name| !name.trim().is_empty() && seen.insert(name.clone()))
            .collect();
        Some(if projects.is_empty() {
            group.name.clone()
        } else {
            projects.join(" + ")
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
        let tabs = self.navigation_tabs(tab);
        let index = tabs.iter().position(|id| *id == tab)?;
        let next = if forward {
            (index + 1) % tabs.len()
        } else {
            (index + tabs.len() - 1) % tabs.len()
        };
        self.tabs.get(&tabs[next]).map(|tab| tab.position)
    }

    pub(super) fn closed_tab_group_destination(&self, client: ClientId) -> Option<TabId> {
        if !self.tab_groups_enabled() {
            let valid = |id: &TabId| {
                self.tabs
                    .get(id)
                    .is_some_and(|tab| !tab.is_pending() && tab.has_selectable_panes())
            };
            return self
                .tab_history
                .get(&client)
                .and_then(|history| history.iter().rev().find(|id| valid(id)).copied())
                .or_else(|| {
                    self.tabs
                        .values()
                        .filter(|tab| valid(&tab.id))
                        .min_by_key(|tab| tab.position)
                        .map(|tab| tab.id)
                });
        }
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
        mut completion: Option<NotificationEnd>,
        tab_switches: &mut HashSet<(TabId, ClientId)>,
    ) -> Result<()> {
        let result = self.tab_group_action_inner(action, client, &mut completion, tab_switches);
        if let Err(error) = &result {
            if let Some(completion) = completion.as_mut() {
                completion.set_error_message(format!("{error:#}"));
            }
        }
        result
    }

    fn tab_group_action_inner(
        &mut self,
        action: Action,
        client: ClientId,
        completion: &mut Option<NotificationEnd>,
        tab_switches: &mut HashSet<(TabId, ClientId)>,
    ) -> Result<()> {
        if !self.tab_groups_enabled()
            && !matches!(
                action,
                Action::ListTabGroups { .. }
                    | Action::CurrentTabGroupInfo { .. }
                    | Action::GoToTabInGroup { .. }
            )
        {
            return Ok(());
        }
        let client = self.client_id(client).unwrap_or(client);
        let action = match action {
            Action::RenameCurrentTabGroup { name } => Action::RenameTabGroup {
                id: self.group_for_client(Some(client)),
                name,
            },
            Action::CloseCurrentTabGroup => Action::CloseTabGroup {
                id: self.group_for_client(Some(client)),
            },
            Action::GoToTabGroupName { name, create } => {
                let id = self
                    .tab_groups
                    .keys()
                    .copied()
                    .find(|id| self.tab_group_name(*id).as_deref() == Some(name.as_str()));
                if let Some(id) = id {
                    Action::SwitchTabGroup { id }
                } else if create {
                    Action::NewNamedTabGroup { name: Some(name) }
                } else {
                    return Err(anyhow!("tab group not found: {name}"));
                }
            },
            action => action,
        };
        match action {
            Action::ListTabGroups { output_json } | Action::CurrentTabGroupInfo { output_json } => {
                let current_only = matches!(action, Action::CurrentTabGroupInfo { .. });
                let active = self.group_for_client(Some(client));
                let groups: Vec<_> = self
                    .tab_groups
                    .keys()
                    .copied()
                    .filter(|id| !current_only || *id == active)
                    .map(|id| {
                        Ok(TabGroupRecord {
                            id,
                            name: self.tab_group_name(id).context("tab group missing")?,
                            active: id == active,
                            tab_ids: self.group_tabs(id),
                        })
                    })
                    .collect::<Result<_>>()?;
                let output = if current_only {
                    let group = groups.first().context("no current tab group")?;
                    if output_json {
                        serde_json::to_string_pretty(group)
                            .context("serialize current tab group")?
                    } else {
                        format!("name: {}\nid: {}", group.name, group.id)
                    }
                } else if output_json {
                    serde_json::to_string_pretty(&groups).context("serialize tab groups")?
                } else {
                    let mut table = vec![vec![
                        "ID".into(),
                        "NAME".into(),
                        "ACTIVE".into(),
                        "TAB_IDS".into(),
                    ]];
                    for group in groups {
                        table.push(vec![
                            group.id.to_string(),
                            group.name,
                            group.active.to_string(),
                            group
                                .tab_ids
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(","),
                        ]);
                    }
                    table
                        .iter()
                        .map(|row| row.join("  "))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if let Some(completion) = completion.as_mut() {
                    completion.set_stdout_message(output);
                }
                return Ok(());
            },
            Action::NewTabGroup | Action::NewNamedTabGroup { .. } => {
                let (name, report_id) = match action {
                    Action::NewNamedTabGroup { name } => (name, true),
                    _ => (None, false),
                };
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
                        name: name
                            .map(|name| {
                                yinz_types::text::clean_string_from_control_and_linebreak(&name)
                            })
                            .unwrap_or_else(|| format!("Group {number}")),
                        preferred: Some(tab),
                        restored_preferred: None,
                    },
                );
                self.tab_group_ids.insert(tab, id);
                self.prune_tab_groups();
                if let Some(completion) = completion.as_mut() {
                    completion.wait_for_placement();
                    if report_id {
                        completion.set_stdout_message(id.to_string());
                    }
                }
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
                    completion.take(),
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
                    self.confirm_quit(client, true, completion.take())
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
            Action::MovePaneToNextTabGroup | Action::MovePaneToPreviousTabGroup => {
                let pane = self
                    .get_active_tab(client)?
                    .get_active_pane_id(client)
                    .context("no active pane to move to group")?;
                let groups: Vec<_> = self.tab_groups.keys().copied().collect();
                let active = self.group_for_client(Some(client));
                let index = groups.iter().position(|id| *id == active).unwrap_or(0);
                let forward = action == Action::MovePaneToNextTabGroup;
                let destination = (1..groups.len()).find_map(|offset| {
                    let index = if forward {
                        (index + offset) % groups.len()
                    } else {
                        (index + groups.len() - offset) % groups.len()
                    };
                    self.preferred_group_tab(groups[index], client, false)
                });
                let error = if let Some(destination) = destination {
                    let outcome = self
                        .break_multiple_panes_to_tab_with_id(vec![pane], destination, false, client)
                        .context("move pane to adjacent group")?;
                    if outcome.moved.contains(&pane) {
                        self.switch_active_tab_with_pane_focus(
                            self.tabs[&destination].position,
                            None,
                            true,
                            client,
                            Some(pane),
                        )
                        .context("follow pane to adjacent group")?;
                        None
                    } else {
                        if outcome.returned_to_source.contains(&pane) {
                            self.get_active_tab_mut(client)?
                                .focus_pane_with_id(pane, false, false, client)
                                .context("restore focus after refused group move")?;
                        }
                        Some("Could not move pane to tab group!")
                    }
                } else {
                    Some("No other tab groups to add pane to!")
                };
                if let Some(error) = error {
                    self.bus
                        .senders
                        .send_to_background_jobs(
                            crate::background_jobs::BackgroundJob::DisplayPaneError(
                                vec![pane],
                                error.into(),
                            ),
                        )
                        .context("report pane group move failure")?;
                }
            },
            Action::GoToTabInGroup { index } => {
                let tabs = self
                    .active_tab_ids
                    .get(&client)
                    .map(|tab| self.navigation_tabs(*tab))
                    .unwrap_or_default();
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
