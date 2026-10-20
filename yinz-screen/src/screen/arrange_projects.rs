use super::{Screen, ScreenInstruction};
use crate::{panes::PaneId, plugins::PluginInstruction, ClientId, TerminalId};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    data::ProjectPaneGroup,
    input::layout::{Run, TiledPaneLayout},
    internal_ids::TabId,
};

// No panes move until every destination has its configured content viewport.
pub(super) struct PendingArrangement {
    groups: Vec<ProjectPaneGroup>,
    client_id: ClientId,
    pub destinations: HashMap<String, TabId>,
}

impl Screen {
    pub(super) fn arrangement_notice(&self, message: String) {
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::Notify(message, None, None, None))
            .non_fatal();
    }

    pub(super) fn arrange_project_panes(
        &mut self,
        groups: Vec<ProjectPaneGroup>,
        client_id: ClientId,
    ) -> Result<()> {
        if self.pending_arrangement.is_some() {
            self.arrangement_notice("Arrange panes: destination tabs are still loading".into());
            return Ok(());
        }
        self.arrange_project_panes_in_tabs(groups, client_id, HashMap::new())
    }

    pub(super) fn resume_pending_arrangement(&mut self) -> Result<()> {
        let Some(pending) = &self.pending_arrangement else {
            return Ok(());
        };
        if pending
            .destinations
            .values()
            .any(|id| self.tabs.get(id).is_some_and(|tab| tab.is_pending()))
        {
            return Ok(());
        }
        let pending = self
            .pending_arrangement
            .take()
            .context("missing pending arrangement")?;
        self.arrange_project_panes_in_tabs(pending.groups, pending.client_id, pending.destinations)
    }

    fn arrange_project_panes_in_tabs(
        &mut self,
        groups: Vec<ProjectPaneGroup>,
        client_id: ClientId,
        destinations: HashMap<String, TabId>,
    ) -> Result<()> {
        // Validate the entire request before touching any pane. A delayed snapshot may
        // name a pane that has since closed or become suppressed.
        let eligible: HashMap<PaneId, TabId> = self
            .tabs
            .values()
            .flat_map(|tab| {
                tab.tiled_and_floating_pane_ids()
                    .into_iter()
                    .chain(tab.suppressed_stack_list_members().map(|(id, _)| *id))
                    .filter(|id| matches!(id, PaneId::Terminal(_)))
                    .filter(|id| {
                        !tab.get_tiled_pane_ids().contains(id)
                            || tab.arrangement_pane_ids().contains(id)
                    })
                    .map(|id| (id, tab.id))
            })
            .collect();
        let original_stacks: Vec<_> = self
            .tabs
            .values()
            .flat_map(|tab| tab.arrangement_stacks())
            .collect();
        let mut seen = HashSet::new();
        let mut identities = HashSet::new();
        for group in &groups {
            if group.identity.is_empty()
                || group.name.trim().is_empty()
                || group.name.chars().any(char::is_control)
                || !identities.insert(&group.identity)
                || group.pane_ids.iter().any(|id| !seen.insert(*id))
            {
                self.arrangement_notice(
                    "Arrange panes: invalid or duplicate project/pane identity".into(),
                );
                return Ok(());
            }
        }
        let focused: Vec<_> = self
            .connected_clients
            .borrow()
            .iter()
            .filter_map(|client| self.get_active_pane_id(client).map(|pane| (*client, pane)))
            .collect();
        let mut reserved = HashSet::new();
        let mut selected = vec![];
        for mut group in groups {
            let before = group.pane_ids.len();
            group
                .pane_ids
                .retain(|id| eligible.contains_key(&PaneId::Terminal(TerminalId::new(*id))));
            if group.pane_ids.len() != before {
                self.arrangement_notice(format!(
                    "Arrange panes: {} changed panes left untouched",
                    before - group.pane_ids.len()
                ));
            }
            if group.pane_ids.is_empty() {
                continue;
            }
            let panes: Vec<_> = group
                .pane_ids
                .iter()
                .map(|id| PaneId::Terminal(TerminalId::new(*id)))
                .collect();
            let mut candidates: Vec<_> = self
                .tabs
                .values()
                .filter(|tab| !reserved.contains(&tab.id))
                .collect();
            candidates.sort_by_key(|tab| {
                let count = panes
                    .iter()
                    .filter(|pane| eligible.get(pane) == Some(&tab.id))
                    .count();
                let matching = self.arrangement_projects.get(&tab.id) == Some(&group.identity)
                    || (count > 0
                        && eligible
                            .iter()
                            .filter(|(_, id)| **id == tab.id)
                            .all(|(pane, _)| panes.contains(pane)));
                (
                    std::cmp::Reverse(matching),
                    std::cmp::Reverse(count),
                    tab.position,
                )
            });
            let destination = destinations.get(&group.identity).copied().or_else(|| {
                candidates
                    .first()
                    .filter(|tab| {
                        self.arrangement_projects.get(&tab.id) == Some(&group.identity)
                            || panes.iter().any(|pane| eligible.get(pane) == Some(&tab.id))
                    })
                    .map(|tab| tab.id)
            });
            if let Some(id) = destination {
                reserved.insert(id);
            }
            selected.push((group, destination, panes));
        }
        let mut loaded_projects = vec![];
        for (group, destination, panes) in selected {
            let loaded = group
                .configuration_root
                .as_ref()
                .map(|root| {
                    if !Path::new(root).is_absolute() {
                        return Err("configuration root must be absolute".to_owned());
                    }
                    let path = Path::new(root).join(".yinz/layout.kdl");
                    match std::fs::read_to_string(&path) {
                        Ok(source) => yinz_config::project_layout::parse(&source).map(Some),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(error) => Err(format!("{}: {error}", path.display())),
                    }
                })
                .unwrap_or(Ok(None));
            let layout = match loaded {
                Ok(layout) => layout.unwrap_or_else(TiledPaneLayout::default),
                Err(error) => {
                    self.arrangement_notice(format!("Arrange {}: {error}", group.name));
                    continue;
                },
            };
            loaded_projects.push((group, destination, panes, layout));
        }
        let mut selected = loaded_projects;
        // Membership is computed only after every destination is known. Rejection can
        // leave residents behind, so retry the remaining projects until none fail.
        let mut prepared;
        loop {
            let outgoing: HashSet<_> = selected
                .iter()
                .flat_map(|(_, destination, panes, _)| {
                    panes
                        .iter()
                        .copied()
                        .filter(|pane| eligible.get(pane) != destination.as_ref())
                })
                .collect();
            prepared = vec![];
            let mut rejected = HashSet::new();
            for (index, (group, destination, panes, layout)) in selected.iter().enumerate() {
                let tab = destination.and_then(|id| self.tabs.get(&id));
                let size = tab
                    .map(|tab| {
                        let viewport = tab.get_viewport();
                        yinz_types::pane_size::Size {
                            rows: viewport.rows,
                            cols: viewport.cols,
                        }
                    })
                    .unwrap_or_else(|| self.size_for_client(None));
                let mut order: Vec<_> = panes
                    .iter()
                    .copied()
                    .filter(|pane| {
                        self.tabs
                            .get(&eligible[pane])
                            .is_some_and(|tab| tab.arrangement_pane_ids().contains(pane))
                    })
                    .collect();
                let mut resident = tab
                    .map(|tab| tab.arrangement_pane_ids())
                    .unwrap_or_default();
                resident.sort_by_key(|pane| {
                    tab.and_then(|tab| tab.get_pane_with_id(*pane))
                        .map(|p| (p.position_and_size().logical_position, p.y(), p.x(), *pane))
                });
                for pane in resident {
                    if !outgoing.contains(&pane) && !order.contains(&pane) {
                        order.push(pane);
                    }
                }
                let mut stacks = vec![];
                for original in &original_stacks {
                    // Project members and unknown residents are separate partitions,
                    // even when this project stays in the original tab.
                    for project_members in [true, false] {
                        let members: Vec<_> = order
                            .iter()
                            .copied()
                            .filter(|id| {
                                original.members.contains(id)
                                    && panes.contains(id) == project_members
                            })
                            .collect();
                        if !members.is_empty() {
                            let visible = if members.contains(&original.visible) {
                                original.visible
                            } else {
                                members[0]
                            };
                            stacks.push(crate::tab::StackList { members, visible });
                        }
                    }
                }
                let mut slots = vec![];
                for pane in order {
                    let representative = stacks
                        .iter()
                        .find(|stack| stack.members.contains(&pane))
                        .map(|stack| stack.visible)
                        .unwrap_or(pane);
                    if !slots.contains(&representative) {
                        slots.push(representative);
                    }
                }
                // Existing lists take priority over file splits, which describe
                // individual panes and cannot express these preserved partitions.
                let layout = if !stacks.iter().any(|stack| stack.members.len() > 1) {
                    layout.clone()
                } else {
                    TiledPaneLayout::default()
                };
                let prepared_layout =
                    crate::tab::arrangement_layout(layout, &size, slots.len(), None).and_then(
                        |layout| {
                            let positions = layout
                                .position_panes_in_space(
                                    &yinz_types::pane_size::PaneGeom::from(&size),
                                    Some(slots.len()),
                                    false,
                                    false,
                                )
                                .context("failed to position panes in layout")?;
                            for (pane, (_, geom)) in slots.iter().zip(positions) {
                                if let Some(stack) = stacks
                                    .iter()
                                    .find(|stack| stack.visible == *pane && stack.members.len() > 1)
                                    && geom.rows.as_usize()
                                        < stack.members.len() + 1 + crate::tab::MIN_TERMINAL_HEIGHT
                                {
                                    return Err(anyhow!(
                                        "project stack headers do not fit this tab"
                                    ));
                                }
                            }
                            Ok(layout)
                        },
                    );
                match prepared_layout {
                    Ok(layout) => prepared.push((
                        group.clone(),
                        *destination,
                        panes.clone(),
                        layout,
                        slots,
                        stacks,
                    )),
                    Err(error) => {
                        self.arrangement_notice(format!("Arrange {}: {error}", group.name));
                        rejected.insert(index);
                    },
                }
            }
            if rejected.is_empty() {
                break;
            }
            selected = selected
                .into_iter()
                .enumerate()
                .filter_map(|(index, project)| (!rejected.contains(&index)).then_some(project))
                .collect();
        }
        if prepared
            .iter()
            .any(|(_, destination, ..)| destination.is_none())
        {
            let mut destinations = HashMap::new();
            let mut groups = vec![];
            for (group, destination, ..) in &prepared {
                let id = match destination {
                    Some(id) => *id,
                    None => {
                        let id = self.get_new_tab_id();
                        self.new_tab(
                            id,
                            (
                                self.default_layout.swap_tiled_layouts.clone(),
                                self.default_layout.swap_floating_layouts.clone(),
                            ),
                            Some(group.name.clone()),
                            None,
                        )?;
                        self.assign_tab_group(id, Some(client_id));
                        let (mut layout, _) = self.default_layout.new_tab();
                        // Keep plugin slots and their properties, but start no new processes.
                        for run in layout.extract_run_instructions() {
                            if !matches!(run, Some(Run::Plugin(_))) {
                                layout.ignore_run_instruction(run);
                            }
                        }
                        self.bus
                            .senders
                            .send_to_plugin(PluginInstruction::NewTab(
                                None,
                                None,
                                Some(layout),
                                Some(vec![]),
                                id,
                                None,
                                false,
                                false,
                                client_id,
                                None,
                            ))
                            .context("failed to load arrangement tab")?;
                        id
                    },
                };
                destinations.insert(group.identity.clone(), id);
                groups.push(group.clone());
            }
            self.pending_arrangement = Some(PendingArrangement {
                groups,
                client_id,
                destinations,
            });
            return Ok(());
        }
        reserved.clear();
        let mut ready = vec![];
        for (group, destination, panes, layout, order, stacks) in prepared {
            let destination = destination.context("arrangement destination was not initialized")?;
            reserved.insert(destination);
            ready.push((group, destination, panes, layout, order, stacks));
        }
        let prepared = ready;
        let sources: HashSet<_> = prepared
            .iter()
            .flat_map(|(_, _, panes, _, _, _)| {
                panes.iter().filter_map(|pane| eligible.get(pane).copied())
            })
            .collect();
        // All filesystem reads and geometry checks precede extraction.
        for tab in self.tabs.values_mut() {
            if sources.contains(&tab.id) || reserved.contains(&tab.id) {
                tab.prepare_arrangement();
            }
        }
        for (group, destination, panes, _, _, _) in &prepared {
            let outcome = self.move_project_or_existing_panes(
                panes.clone(),
                *destination,
                false,
                client_id,
                true,
            )?;
            if !outcome.returned_to_source.is_empty() || !outcome.not_found.is_empty() {
                self.arrangement_notice(format!(
                    "Arrange {}: some panes could not move and remain in their source tabs",
                    group.name
                ));
            }
            self.arrangement_projects
                .insert(*destination, group.identity.clone());
        }
        for (_, destination, _, layout, order, stacks) in prepared {
            if let Some(tab) = self.tabs.get_mut(&destination) {
                tab.restore_arrangement_stacks(&stacks);
                tab.arrange_existing_panes(&layout, &order)?;
            }
        }
        for tab in self.tabs.values_mut() {
            if sources.contains(&tab.id) && !reserved.contains(&tab.id) {
                tab.restore_arrangement_stack_visibility(&original_stacks);
            }
        }
        for (client, pane) in focused {
            self.focus_pane_with_id(pane, false, false, client)
                .non_fatal();
        }
        let empty: Vec<_> = self
            .tabs
            .values()
            .filter(|tab| {
                sources.contains(&tab.id) && !reserved.contains(&tab.id) && !tab.should_retain_tab()
            })
            .map(|tab| tab.id)
            .collect();
        for id in empty {
            self.close_tab_by_id(id)?;
        }
        self.arrangement_projects
            .retain(|id, _| self.tabs.contains_key(id));
        self.log_and_report_session_state()?;
        Ok(())
    }
}
