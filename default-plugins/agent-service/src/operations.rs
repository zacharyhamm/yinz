use super::*;
use serde_json::{json, Value};

impl State {
    pub(crate) fn prune_closed_tab_assignments(
        &mut self,
        tabs: &[TabInfo],
        backend: &impl Backend,
    ) -> Result<(), String> {
        // Only observed closures count: restored and newly opened assignments may
        // still be waiting for their first tab event.
        let closed: Vec<_> = self
            .tabs
            .iter()
            .filter(|old| !tabs.iter().any(|tab| tab.tab_id == old.tab_id))
            .map(|tab| tab.tab_id)
            .collect();
        if !closed.is_empty() {
            let owner = self.owner();
            backend.apply(vec![Mutation::delete(
                "agent_assignment",
                Pred::eq("host_id", owner.host_id)
                    .and(Pred::eq("yinz_session_name", owner.yinz_session_name))
                    .and(Pred::is_in(
                        "tab_id",
                        closed.iter().map(|id| id.get() as i64),
                    )),
            )])?;
            self.tab_projects.retain(|id, _| !closed.contains(id));
        }
        Ok(())
    }

    pub(crate) fn restore_assignments(&mut self) {
        if let Err(error) = self.restore_assignments_from(&store::Host::shared()) {
            eprintln!("agent-service: could not restore metadata: {error}");
        }
    }

    fn restore_assignments_from(&mut self, backend: &impl Backend) -> Result<(), String> {
        let latches = backend.fetch(
            &agents::rows::owned_rows_request(agents::rows::LATCH_ENTITY, &self.owner())
                .limit(store::LIST_FETCH_LIMIT),
        )?;
        let assignments = backend.fetch(
            &agents::rows::owned_rows_request("agent_assignment", &self.owner())
                .limit(store::LIST_FETCH_LIMIT),
        )?;
        let projects =
            backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?;
        if latches.truncated || assignments.truncated || projects.truncated {
            return Err("metadata is incomplete; refusing to replace the current snapshot".into());
        }
        let registry = projects::from_rows(&projects);
        let ids = projects::row_ids(&projects);
        let mut tabs = BTreeMap::new();
        for row in &assignments.rows {
            let (Some(DataValue::Int(tab)), Some(DataValue::Int(project_id))) = (
                assignments.get(row, "tab_id"),
                assignments.get(row, "project_id"),
            ) else {
                continue;
            };
            let project_id = DataRowId::new(*project_id);
            let mut matches = registry.values().filter(|p| {
                ids.get(&p.name) == Some(&project_id) || p.replaced_ids.contains(&project_id)
            });
            if let (Some(project), None, Ok(tab)) =
                (matches.next(), matches.next(), usize::try_from(*tab))
            {
                tabs.insert(TabId::new(tab), project.name.clone());
            }
        }
        self.manual_kinds = agents::rows::latches_from_rows(&latches);
        self.tab_projects = tabs;
        Ok(())
    }

    pub(crate) fn operation(
        &mut self,
        op: &str,
        args: &Value,
        caller: Option<PaneId>,
    ) -> Result<Value, String> {
        let session = get_session_list()?
            .live_sessions
            .into_iter()
            .find(|s| s.is_current_session)
            .ok_or("current session is unavailable")?;
        if self.needs_bootstrap() {
            self.bootstrap();
        }
        // The event stream is newer than the session scanner's cached snapshot.
        let mut session = session;
        session.panes = self.manifest.clone();
        session.tabs = self.tabs.clone();
        let backend = store::Host::shared();
        let project_rows =
            backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?;
        if project_rows.truncated {
            return Err("project registry is incomplete".into());
        }
        let registry = projects::from_rows(&project_rows);
        let text = |key| {
            args.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{key} is required"))
        };
        let id = || {
            args.get("pane_id")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| "pane_id is required".to_owned())
        };
        let mut panes = self.panes.clone().unwrap_or_default();
        panes.extend(self.suppressed_panes.iter().cloned());
        // A CLI can address a freshly spawned pane before its PaneUpdate reaches us.
        // Confirm explicit targets with the host; detection history still belongs here.
        let selected = args
            .get("pane_id")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .or_else(|| match caller {
                Some(PaneId::Terminal(id)) if op == "context" => Some(id.get()),
                _ => None,
            });
        if let Some(id) = selected {
            let info = get_pane_info(PaneId::Terminal(TerminalId::new(id)))
                .ok_or("terminal pane is unavailable")?;
            let previous = panes.iter().position(|p| p.pane_id == id);
            let tab = previous.and_then(|i| panes[i].tab.clone());
            let mut live = agent_proto::panes::agent_pane(&info, tab);
            live.detected = self.latched.get(&id).cloned();
            if let Some(index) = previous {
                panes[index] = live;
            } else {
                panes.push(live);
            }
        }
        let owners: BTreeMap<_, _> = panes
            .iter()
            .filter_map(|pane| {
                let assigned = pane
                    .tab
                    .as_ref()
                    .and_then(|(tab, _)| self.tab_projects.get(tab))
                    .cloned();
                assigned
                    .or_else(|| {
                        let location = pane.checkout_location()?;
                        projects::label_for_cwd(&self.repositories, &registry, &location)
                    })
                    .map(|name| (pane.pane_id, name))
            })
            .collect();
        match op {
            "context" => {
                let selected = args
                    .get("pane_id")
                    .and_then(Value::as_u64)
                    .and_then(|id| u32::try_from(id).ok())
                    .or_else(|| match caller {
                        Some(PaneId::Terminal(id)) => Some(id.get()),
                        _ => None,
                    });
                let pane = selected.and_then(|id| panes.iter().find(|pane| pane.pane_id == id));
                if selected.is_some() && pane.is_none() {
                    return Err("selected caller pane does not exist".into());
                }
                let tab = pane.and_then(|pane| pane.tab.as_ref().map(|(id, _)| *id));
                let project = tab
                    .and_then(|tab| self.tab_projects.get(&tab))
                    .or_else(|| pane.and_then(|pane| owners.get(&pane.pane_id)));
                let services: BTreeMap<_, _> = ["agent-service", "project-store"]
                    .into_iter()
                    .map(|name| {
                        let ids: Vec<_> = session
                            .plugins
                            .iter()
                            .filter_map(|(id, info)| {
                                (info.location == format!("yinz:{name}")).then_some(id)
                            })
                            .collect();
                        (name, ids)
                    })
                    .collect();
                let mut missing = Vec::new();
                if pane.is_none() {
                    missing.push("caller_pane");
                }
                if project.is_none() {
                    missing.push("project");
                }
                missing.extend(
                    services
                        .iter()
                        .filter_map(|(name, ids)| ids.is_empty().then_some(*name)),
                );
                let ambiguous: Vec<_> = services
                    .iter()
                    .filter_map(|(name, ids)| (ids.len() > 1).then_some(*name))
                    .collect();
                let pane = pane.cloned().map(agent_proto::panes::displayed);
                let panes: Vec<_> = panes
                    .iter()
                    .cloned()
                    .map(agent_proto::panes::displayed)
                    .collect();
                Ok(
                    json!({"host_id":self.host_id,"session":session.name,"pane":pane,"tab_id":tab,"project":project,"root":project.and_then(|name| registry.get(name)).map(|p| &p.root),"services":services,"missing":missing,"ambiguous":ambiguous,"pane_candidates":panes,"projects":registry.values().collect::<Vec<_>>()}),
                )
            },
            "agent_register" => {
                let id = id()?;
                let kind = text("kind")?;
                if !agents::valid_kind(kind) {
                    return Err("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into());
                }
                let pane = panes
                    .iter()
                    .find(|pane| pane.pane_id == id)
                    .ok_or("terminal pane is unavailable")?;
                backend
                    .apply(vec![agents::rows::latch_mutation(&self.owner(), id, kind)])
                    .map_err(|error| error.to_string())?;
                self.manual_kinds.insert(id, kind.into());
                set_pane_agent(
                    PaneId::Terminal(TerminalId::new(id)),
                    Some(agents::registered_agent(kind, pane.cwd.clone())),
                );
                self.publish_forced();
                Ok(json!({"pane_id":id,"kind":kind}))
            },
            "project_assign" | "project_open" => {
                let project = registry
                    .get(text("project")?)
                    .ok_or("unknown project; use project-remember first")?;
                let tab = if op == "project_open" {
                    new_tab(
                        Some(
                            args.get("tab_name")
                                .and_then(Value::as_str)
                                .unwrap_or(&project.name),
                        ),
                        Some(&project.root),
                    )
                    .ok_or("could not open project tab")?
                } else {
                    let id = args
                        .get("tab_id")
                        .and_then(Value::as_u64)
                        .and_then(|id| u32::try_from(id).ok())
                        .map(|id| TabId::new(id as usize))
                        .ok_or("tab_id is required")?;
                    if !self.tabs.iter().any(|tab| tab.tab_id == id) {
                        return Err("tab is unavailable".into());
                    }
                    id
                };
                let uid = json!([self.host_id, session.name, tab]).to_string();
                backend
                    .apply(vec![Mutation::put("agent_assignment")
                        .uid(uid)
                        .set("host_id", &self.host_id)
                        .set("yinz_session_name", &session.name)
                        .set("tab_id", tab.get() as i64)
                        .set("project", project.name.clone())
                        .set(
                            "project_id",
                            projects::row_ids(&project_rows)
                                .get(&project.name)
                                .ok_or("project disappeared")?
                                .get(),
                        )
                        .build()])
                    .map_err(|error| error.to_string())?;
                self.tab_projects.insert(tab, project.name.clone());
                self.publish_forced();
                Ok(json!({"tab_id":tab,"project":project.name}))
            },
            "project_config_read" | "project_config_write" => {
                let project = registry
                    .get(text("project")?)
                    .ok_or("unknown project; use project-list")?;
                let directory = std::path::PathBuf::from(&project.root);
                let snapshot = if op == "project_config_read" {
                    project_config_read(directory)?
                } else {
                    let expected = args
                        .get("expected")
                        .ok_or("expected source is required (null for an absent file)")?;
                    if !expected.is_null() && !expected.is_string() {
                        return Err("expected must be a string or null".into());
                    }
                    project_config_write(
                        directory,
                        text("source")?.to_owned(),
                        expected.as_str().map(str::to_owned),
                    )?
                };
                serde_json::to_value(snapshot).map_err(|error| error.to_string())
            },
            "plugin_reload" => {
                let url = text("url")?;
                let running = get_plugin_ids_for_url(url)?;
                if running.is_empty() {
                    load_new_plugin(url, BTreeMap::new(), true, true);
                } else {
                    for id in &running {
                        reload_plugin_with_id(*id);
                    }
                }
                Ok(json!({"url":url,"reloaded":running,"dispatched":true}))
            },
            "pane_rerun" => {
                let id = id()?;
                let pane_id = PaneId::Terminal(TerminalId::new(id));
                let info = get_pane_info(pane_id).ok_or("terminal pane is unavailable")?;
                if !info.is_held {
                    return Err("pane is not a held command".into());
                }
                if let Some(reason) = info.restart_error {
                    return Err(reason);
                }
                rerun_command_pane(TerminalId::new(id));
                Ok(json!({"pane_id":id,"dispatched":true}))
            },
            "pane_move_to_tab" => {
                let pane: PaneId = text("pane_id")?
                    .parse()
                    .map_err(|e| format!("invalid pane: {e}"))?;
                if !session.panes.panes.values().flatten().any(|p| p.id == pane) {
                    return Err("pane is unavailable".into());
                }
                let tab = args
                    .get("tab_id")
                    .and_then(Value::as_u64)
                    .and_then(|id| TabId::try_from(id).ok())
                    .ok_or("tab_id is required")?;
                if !session.tabs.iter().any(|t| t.tab_id == tab) {
                    return Err("tab is unavailable".into());
                }
                let result = break_panes_to_tab_with_id(&[pane], tab, false)
                    .ok_or("could not move pane to tab")?;
                Ok(json!({"pane_id":pane,"tab_id":result}))
            },
            "pane_swap" => {
                use agent_proto::pane_operations::{plan_swap, PaneDirection, PanePosition};
                let first: PaneId = text("first")?
                    .parse()
                    .map_err(|e| format!("invalid first pane: {e}"))?;
                let second: PaneId = text("second")?
                    .parse()
                    .map_err(|e| format!("invalid second pane: {e}"))?;
                let positions: Vec<_> = session
                    .tabs
                    .iter()
                    .flat_map(|tab| {
                        session
                            .panes
                            .panes
                            .get(&tab.position)
                            .into_iter()
                            .flatten()
                            .map(move |pane| PanePosition {
                                id: pane.id,
                                tab_position: Some(tab.position),
                                x: pane.pane_x.get(),
                                y: pane.pane_y.get(),
                                columns: pane.pane_columns.get(),
                                rows: pane.pane_rows.get(),
                                visible: !pane.is_suppressed,
                                selectable: pane.is_selectable,
                                floating: pane.is_floating,
                                fullscreen: tab.is_fullscreen_active,
                                floating_layer_visible: tab.are_floating_panes_visible,
                            })
                    })
                    .collect();
                let direction = match plan_swap(&positions, first, second)? {
                    PaneDirection::Left => Direction::Left,
                    PaneDirection::Right => Direction::Right,
                    PaneDirection::Up => Direction::Up,
                    PaneDirection::Down => Direction::Down,
                };
                move_pane_with_pane_id_in_direction(first, direction);
                Ok(json!({"first":first,"second":second,"dispatched":true}))
            },
            "pane_pin" | "pane_unpin" => {
                let pane: PaneId = text("pane_id")?
                    .parse()
                    .map_err(|error| format!("invalid pane ID: {error}"))?;
                let info = session
                    .panes
                    .panes
                    .values()
                    .flatten()
                    .find(|info| info.id == pane)
                    .ok_or("pane is unavailable")?;
                if !info.is_floating {
                    return Err("only floating panes can be pinned".into());
                }
                set_floating_pane_pinned(pane, op == "pane_pin");
                Ok(json!({"pane_id":pane,"dispatched":true}))
            },
            _ => Err(format!("unknown agent operation {op}")),
        }
    }
}

#[cfg(test)]
#[path = "unit/operations.rs"]
mod tests;
