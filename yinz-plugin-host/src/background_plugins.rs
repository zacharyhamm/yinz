//! Session lifecycle operations reuse the loader and its retained retry metadata.
use super::*;
use yinz_types::cli::{PluginOperation, PluginRequest, PluginResponse, PluginState, PluginStatus};

impl PluginBridge {
    pub(super) fn normalize_plugin(&self, mut run: RunPlugin) -> RunPlugin {
        if let Some(location) = self.resolved_plugin_locations.get(&run.location) {
            run.location = location.clone();
            return run;
        }
        if let RunPluginLocation::File(path) = &run.location {
            // Identity lookup must not read source bytes on the plugin thread.
            let resolved = PluginConfig::from_run_plugin(&run).and_then(|config| {
                match std::fs::metadata(&config.path) {
                    Ok(metadata) => metadata.is_file().then_some(config.path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        self.user_plugin_dir.as_ref().and_then(|dir| {
                            let path =
                                dir.join(config.relative_path.as_ref().unwrap_or(&config.path));
                            path.is_file().then_some(path)
                        })
                    },
                    Err(_) => None,
                }
            });
            run.location = RunPluginLocation::File(yinz_runtime::common_path::source_path(
                resolved.as_deref().unwrap_or(path),
                &self.yinz_cwd,
            ));
        }
        run
    }

    pub(super) fn remember_plugin_location(&mut self, run: &RunPlugin) {
        let normalized = self.normalize_plugin(run.clone());
        self.resolved_plugin_locations
            .insert(run.location.clone(), normalized.location);
    }

    pub(super) fn refresh_loaded_plugin_location(&mut self, id: PluginId) {
        let map = self.plugin_map.lock().unwrap();
        // Failed loads retain metadata, but must not move the declaration's mapping.
        if map.get_running_plugin(id, None).is_none() {
            return;
        }
        if let Some(metadata) = map.cached_metadata(id)
            && let (RunPluginLocation::File(_), Some(source)) =
                (&metadata.config.location, &metadata.resolved_source_path)
        {
            // Refresh only this declaration, not every alias of its previous source.
            self.resolved_plugin_locations.insert(
                RunPluginLocation::File(metadata.config.path.clone()),
                RunPluginLocation::File(source.clone()),
            );
        }
    }

    fn identities(&self) -> BTreeMap<PluginId, RunPlugin> {
        let mut plugins = self.plugin_map.lock().unwrap().list_plugins();
        plugins.extend(self.loading_plugins.iter().cloned());
        plugins
    }

    pub(super) fn loaded_plugin_identity(&self, id: PluginId, run: RunPlugin) -> RunPlugin {
        if matches!(run.location, RunPluginLocation::File(_))
            && let Some(path) = self
                .plugin_map
                .lock()
                .unwrap()
                // Identity is fixed at load; the guest may be waiting for this thread's reply.
                .cached_metadata(id)
                .and_then(|metadata| metadata.resolved_source_path.clone())
        {
            // This belongs to this load, even if its declaration's symlink has moved.
            return RunPlugin {
                location: RunPluginLocation::File(path),
                ..run
            };
        }
        self.normalize_plugin(run)
    }

    fn matches_location(&self, id: PluginId, run: &RunPlugin, target: &RunPlugin) -> bool {
        let declaration = |run: &RunPlugin| match &run.location {
            RunPluginLocation::File(path) => Some(
                self.yinz_cwd.join(
                    run.declared_path
                        .as_ref()
                        .filter(|path| !path.as_os_str().is_empty())
                        .unwrap_or(path),
                ),
            ),
            _ => None,
        };
        run.location == target.location
            || declaration(run).is_some_and(|path| Some(path) == declaration(target))
            || self.loaded_plugin_identity(id, run.clone()).location
                == self.normalize_plugin(target.clone()).location
    }

    pub(super) fn same_identity(&self, left: &RunPlugin, right: &RunPlugin) -> bool {
        self.normalize_plugin(left.clone()).location
            == self.normalize_plugin(right.clone()).location
            && left.configuration.without_caller_context()
                == right.configuration.without_caller_context()
    }

    pub(crate) fn start_background_plugin(
        &mut self,
        mut declaration: RunPluginOrAlias,
        client: ClientId,
        independent: bool,
    ) -> Result<(PluginId, ClientId)> {
        declaration.populate_run_plugin_if_needed(&self.plugin_aliases.lock().unwrap());
        let run = declaration
            .get_run_plugin()
            .context("unknown plugin alias")?;
        self.remember_plugin_location(&run);
        PluginConfig::from_run_plugin(&run)
            .with_context(|| format!("Failed to resolve plugin: {}", run.location))?;
        let existing = self.identities().into_iter().find(|(id, other)| {
            self.matches_location(*id, other, &run)
                && other.configuration.without_caller_context()
                    == run.configuration.without_caller_context()
        });
        let (id, owner) = if let Some((id, _)) = existing {
            (id, self.get_first_client_id().unwrap_or(client))
        } else {
            // A background start: a plugin that declares a pane gets a parked record, which the
            // ledger records before the executor can enter guest initialization.
            let (id, owner) = self.load_plugin(
                &Some(run.clone()),
                None,
                Size::default(),
                run.initial_cwd.clone(),
                false,
                Some(client),
                PaneRequest::Parked {
                    run: RunPluginOrAlias::RunPlugin(run.clone()),
                    cwd: run.initial_cwd.clone(),
                    client_id: client,
                },
            )?;
            if !independent {
                self.independent_plugin_ids.remove(&id);
            }
            (id, owner)
        };
        if independent {
            self.independent_plugin_ids.insert(id);
        }
        Ok((id, owner))
    }

    pub(crate) fn reconcile_background_plugins(
        &mut self,
        declarations: std::collections::BTreeSet<RunPluginOrAlias>,
        client: ClientId,
    ) -> Result<()> {
        let aliases = self.plugin_aliases.lock().unwrap().clone();
        let desired: Vec<_> = declarations
            .into_iter()
            .filter_map(|mut declaration| {
                declaration.populate_run_plugin_if_needed(&aliases);
                match declaration.get_run_plugin() {
                    Some(run) => {
                        self.remember_plugin_location(&run);
                        Some(run)
                    },
                    None => {
                        log::error!("Unknown background plugin alias: {declaration:?}");
                        None
                    },
                }
            })
            .collect();
        let removed: Vec<_> = self
            .identities()
            .into_iter()
            .filter(|(id, run)| {
                !self.independent_plugin_ids.contains(id)
                    && self
                        .configured_plugins
                        .iter()
                        .any(|old| self.same_identity(old, run))
                    && !desired.iter().any(|new| self.same_identity(new, run))
            })
            .map(|(id, _)| id)
            .collect();
        for id in removed {
            self.stop_plugin(id)?;
        }
        let mut reconciled = vec![];
        for run in desired {
            if !self
                .configured_plugins
                .iter()
                .any(|old| self.same_identity(old, &run))
                && let Err(error) = self.start_background_plugin(
                    RunPluginOrAlias::RunPlugin(run.clone()),
                    client,
                    false,
                )
            {
                log::error!("Failed to start background plugin {run:?}: {error:#}");
                continue;
            }
            reconciled.push(run);
        }
        self.configured_plugins = reconciled;
        Ok(())
    }

    fn stop_plugin(&mut self, id: PluginId) -> Result<()> {
        self.unload_plugin(id)
    }

    fn plugin_status(&self, id: PluginId, run: &RunPlugin) -> PluginStatus {
        let mut clients = self
            .plugin_map
            .lock()
            .unwrap()
            .owner_client_ids_of_plugin_id(id);
        clients.sort();
        let latest_error = self.latest_plugin_errors.get(&id).cloned();
        let state = if latest_error.is_some() {
            PluginState::Failed
        } else if self.plugin_ids_waiting_for_permission_request.contains(&id) {
            PluginState::AwaitingPermission
        } else if clients.is_empty() || self.loading_plugins.iter().any(|(p, _)| *p == id) {
            PluginState::Loading
        } else {
            PluginState::Running
        };
        let (background, pane) = {
            let panes = self.panes.lock().unwrap();
            (panes.background(id), panes.presence(id))
        };
        PluginStatus {
            id,
            location: match &self.loaded_plugin_identity(id, run.clone()).location {
                RunPluginLocation::File(path) => Url::from_file_path(path)
                    .map(String::from)
                    .unwrap_or_else(|_| path.display().to_string()),
                location => location.display(),
            },
            configuration: run.configuration.without_caller_context(),
            background,
            shared: self.plugin_is_shared(&id),
            clients,
            state,
            pending_permissions: self
                .permission_requests
                .values()
                .filter(|pending| pending.contains_plugin(id))
                .flat_map(|pending| pending.permissions.iter().copied())
                .collect(),
            latest_error,
            pane,
        }
    }

    pub(crate) fn plugin_request(
        &mut self,
        request: PluginRequest,
        client: ClientId,
    ) -> Result<PluginResponse> {
        let existing = self.identities();
        if request.operation == PluginOperation::List {
            return Ok(PluginResponse {
                plugins: existing
                    .iter()
                    .map(|(id, run)| self.plugin_status(*id, run))
                    .collect(),
            });
        }
        let target = request
            .target
            .as_deref()
            .context("plugin target is required")?;
        let by_id = target
            .strip_prefix("plugin_")
            .and_then(|id| id.parse::<u32>().ok())
            .map(PluginId::new);
        let fallback = request
            .fallback
            .inspect(|run| self.remember_plugin_location(run));
        let matches: Vec<_> = existing
            .iter()
            .filter(|(id, run)| {
                let location_matches = by_id.map(|wanted| **id == wanted).unwrap_or_else(|| {
                    fallback
                        .as_ref()
                        .is_some_and(|fallback| self.matches_location(**id, run, fallback))
                });
                location_matches
                    && request.configuration.as_ref().is_none_or(|configuration| {
                        *configuration == run.configuration.without_caller_context()
                    })
            })
            .collect();
        let mut configurations: Vec<_> = matches
            .iter()
            .map(|(_, run)| run.configuration.without_caller_context())
            .collect();
        configurations.sort();
        configurations.dedup();
        if configurations.len() > 1 {
            anyhow::bail!(
                "ambiguous plugin {target}; specify --configuration-json: {}",
                matches
                    .iter()
                    .map(|(id, run)| format!(
                        "plugin_{id} {}",
                        serde_json::to_string(run.configuration.inner()).unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let run = if let Some((_, run)) = matches.first() {
            (*run).clone()
        } else {
            if request.operation != PluginOperation::Start {
                if request.operation == PluginOperation::Inspect {
                    return Ok(PluginResponse { plugins: vec![] });
                }
                anyhow::bail!("{target} not loaded; use plugins start");
            }
            let mut fallback =
                fallback.context("cannot start a missing plugin ID; use a location")?;
            if let Some(configuration) = &request.configuration {
                fallback.configuration = PluginUserConfiguration::new(configuration.clone());
            } else {
                let mut candidates: Vec<_> = request
                    .candidates
                    .into_iter()
                    .chain(self.configured_plugins.clone())
                    .filter(|run| {
                        self.normalize_plugin(run.clone()).location
                            == self.normalize_plugin(fallback.clone()).location
                    })
                    .collect();
                candidates.sort_by_key(|run| run.configuration.without_caller_context());
                candidates.dedup_by(|a, b| self.same_identity(a, b));
                if candidates.len() > 1 {
                    anyhow::bail!(
                        "ambiguous plugin {target}; specify --configuration-json: {}",
                        serde_json::to_string(
                            &candidates
                                .iter()
                                .map(|run| run.configuration.inner())
                                .collect::<Vec<_>>()
                        )
                        .context("encode configurations")?
                    );
                }
                if let Some(candidate) = candidates.pop() {
                    fallback = candidate;
                }
            }
            fallback
        };
        let ids: Vec<_> = matches.iter().map(|(id, _)| **id).collect();
        let mut statuses = vec![];
        if request.operation == PluginOperation::Start {
            let (id, _) = self.start_background_plugin(
                RunPluginOrAlias::RunPlugin(run.clone()),
                client,
                true,
            )?;
            if let Some(caller) = request.caller {
                self.plugin_callers.insert(id, caller);
            }
            statuses.push(self.plugin_status(id, &run));
        } else {
            for id in ids {
                if request.operation == PluginOperation::Reload
                    && let Some(caller) = request.caller
                {
                    self.plugin_callers.insert(id, caller);
                }
                match request.operation {
                    PluginOperation::Reload => {
                        if self.loading_plugins.iter().any(|(p, _)| *p == id) {
                            self.pending_plugin_id_reloads.insert(id);
                        } else {
                            self.reload_plugin_with_id(id)?;
                        }
                    },
                    PluginOperation::Stop => {
                        let mut status = self.plugin_status(id, &run);
                        self.stop_plugin(id)?;
                        status.state = PluginState::Stopped;
                        status.clients.clear();
                        status.pending_permissions.clear();
                        statuses.push(status);
                        continue;
                    },
                    _ => {},
                }
                statuses.push(self.plugin_status(id, &run));
            }
        }
        Ok(PluginResponse { plugins: statuses })
    }
}
