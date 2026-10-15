//! Picker ownership lives on the plugin thread; screen acknowledges focus restoration.
use super::*;
use prost::Message;
use std::path::Path;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pick::{PickCommand, PickKind, PickOptions, PickRequest, PickResult};

pub(super) struct PendingPick {
    caller: PluginId,
    caller_instance: PluginInstanceId,
    generation: Option<u64>,
    client: ClientId,
    request_id: String,
    pub(super) request: PickRequest,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) opening: Arc<AtomicBool>,
    finder_instance: Option<PluginInstanceId>,
    pane: Option<TerminalId>,
    result: Option<Result<Option<String>, String>>,
}

pub(crate) fn dispatch(env: &mut PluginEnv, mut command: PickCommand) -> Result<()> {
    let result = (|| -> Result<Option<PickRequest>> {
        if command == PickCommand::Request {
            return Ok(env.pick_request.clone());
        }
        let mut cwd = env.plugin_cwd.clone();
        let mut focused_plugin = None;
        if let PickCommand::Start { options, .. } = &mut command {
            options.validate().map_err(|error| anyhow!(error))?;
            if options.kind == PickKind::Directory
                && matches!(&env.plugin_config.location, RunPluginLocation::Yinz(tag) if tag.to_string() == "session-palette")
            {
                if let Some(roots) = &mut options.roots {
                    for root in roots {
                        *root = crate::yinz_exports::resolve_host_directory(env, root)?
                            .to_string_lossy()
                            .into_owned();
                    }
                }
            }
            let (response_channel, receive) = crossbeam::channel::bounded(1);
            env.senders
                .send_to_screen(ScreenInstruction::GetFocusedPaneInfo {
                    client_id: env.client_id,
                    response_channel,
                })
                .context("capture picker focus")?;
            let pane_id = match receive
                .recv_timeout(Duration::from_secs(10))
                .context("receive picker focus")?
            {
                yinz_types::data::GetFocusedPaneInfoResponse::Ok { pane_id, .. } => pane_id,
                yinz_types::data::GetFocusedPaneInfoResponse::Err(error) => {
                    return Err(anyhow!(error))
                },
            };
            if let PaneId::Plugin(plugin) = pane_id {
                focused_plugin = Some(plugin);
            } else {
                let (response_channel, receive) = crossbeam::channel::bounded(1);
                env.senders
                    .send_to_pty(yinz_bus::pty::PtyInstruction::GetPaneCwd {
                        pane_id,
                        response_channel,
                    })
                    .context("capture picker cwd")?;
                cwd = match receive
                    .recv_timeout(Duration::from_secs(10))
                    .context("receive picker cwd")?
                {
                    yinz_types::data::GetPaneCwdResponse::Ok(cwd) => cwd,
                    yinz_types::data::GetPaneCwdResponse::Err(_)
                        if options.roots.as_ref().is_some_and(|roots| {
                            roots.iter().all(|root| Path::new(root).is_absolute())
                        }) =>
                    {
                        PathBuf::from("/")
                    },
                    yinz_types::data::GetPaneCwdResponse::Err(error) => return Err(anyhow!(error)),
                };
            }
        }
        send_command(env, command, cwd, focused_plugin)
    })()
    .map_err(|error| format!("{error:#}"));
    let response = yinz_plugin_api::plugin_command::ProtobufPickResponse {
        json: serde_json::to_string(&result).context("encode picker reply")?,
    };
    crate::yinz_exports::write_response(env, &response.encode_to_vec())
        .context("write picker reply")
}

fn send_command(
    env: &PluginEnv,
    command: PickCommand,
    cwd: PathBuf,
    focused_plugin: Option<PluginId>,
) -> Result<Option<PickRequest>> {
    let asynchronous = matches!(
        command,
        PickCommand::Start { .. } | PickCommand::Cancel { .. }
    );
    let (response, receive) = crossbeam::channel::bounded(1);
    env.senders
        .send_to_plugin(PluginInstruction::Pick {
            plugin_id: env.plugin_id,
            instance_id: env.instance_id,
            generation: env.generation,
            client_id: env.client_id,
            cwd,
            focused_plugin,
            command,
            response,
        })
        .context("send picker command")?;
    if asynchronous {
        return Ok(None);
    }
    receive
        .recv_timeout(Duration::from_secs(10))
        .context("receive picker command")?
        .map_err(|error| anyhow!(error))
}

pub(crate) fn attach_opened_pane(env: &PluginEnv, pane: Option<PaneId>) -> Result<()> {
    if env.pick_request.is_none() {
        return Ok(());
    }
    let command = match pane {
        Some(PaneId::Terminal(pane_id)) => PickCommand::Attach { pane_id },
        _ => PickCommand::Complete {
            result: Err("failed to open picker terminal".into()),
        },
    };
    let result = send_command(env, command, env.plugin_cwd.clone(), None);
    if let Some(opening) = &env.pick_opening {
        opening.store(false, Ordering::SeqCst);
    }
    if result.is_err() {
        if let Some(pane) = pane {
            env.senders
                .send_to_screen(ScreenInstruction::ClosePane(
                    pane,
                    Some(env.client_id),
                    None,
                    None,
                ))
                .context("close rejected picker pane")
                .non_fatal();
        }
    }
    result.map(|_| ())
}

impl PluginBridge {
    pub(crate) fn pick_cwd(
        &self,
        focused_plugin: Option<PluginId>,
        client: ClientId,
        cwd: PathBuf,
    ) -> Result<PathBuf, String> {
        match focused_plugin {
            Some(plugin) => self
                .plugin_map
                .lock()
                .unwrap()
                .instance_cwd(
                    plugin,
                    (!self.shared_plugin_ids.contains(&plugin)).then_some(client),
                )
                .ok_or_else(|| "focused plugin disappeared before picking".into()),
            None => Ok(cwd),
        }
    }

    pub(crate) fn pick_command(
        &mut self,
        plugin: PluginId,
        instance: PluginInstanceId,
        generation: Option<u64>,
        client: ClientId,
        cwd: PathBuf,
        command: PickCommand,
    ) -> Result<Option<PickRequest>, String> {
        match command {
            PickCommand::Start {
                request_id,
                options,
            } => {
                if self
                    .plugin_map
                    .lock()
                    .unwrap()
                    .permission_instance(plugin, instance)
                    .is_none()
                {
                    return Err("picker caller was unloaded or replaced".into());
                }
                if !self.connected_clients.lock().unwrap().contains(&client) {
                    return Err("picker requires a connected client".into());
                }
                if self.picks.values().any(|pick| pick.client == client) {
                    return Err("picker is busy for this client".into());
                }
                if request_id.is_empty() {
                    return Err("picker request ID is empty".into());
                }
                let aliases = self.plugin_aliases.lock().unwrap();
                let location = if aliases.aliases.contains_key("file-finder") {
                    "file-finder"
                } else {
                    "yinz:file-finder"
                };
                let mut run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                    location,
                    &None,
                    Some(&aliases),
                    Some(cwd.clone()),
                )?
                .get_run_plugin()
                .ok_or("file-finder alias is unresolved")?;
                drop(aliases);
                if run.location
                    != RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("file-finder"))
                {
                    return Err("z.pick requires the builtin yinz:file-finder alias".into());
                }
                let options =
                    merge_options(options, run.configuration.inner().get("listing_json"))?;
                let finder = PluginId::new(self.next_plugin_id);
                self.picks.insert(
                    finder,
                    PendingPick {
                        caller: plugin,
                        caller_instance: instance,
                        generation,
                        client,
                        request_id,
                        request: PickRequest {
                            options,
                            cwd: cwd.clone(),
                        },
                        cancelled: Arc::default(),
                        opening: Arc::default(),
                        finder_instance: None,
                        pane: None,
                        result: None,
                    },
                );
                self.plugin_roles.lock().unwrap().insert(finder, true);
                let mut configuration = run.configuration.inner().clone();
                configuration.insert("shared".into(), "true".into());
                run.configuration = PluginUserConfiguration::new(configuration);
                run.initial_cwd = Some(cwd.clone());
                if let Err(error) = self.load_plugin(
                    &Some(run.clone()),
                    None,
                    Size::default(),
                    Some(cwd),
                    false,
                    Some(client),
                ) {
                    self.picks.remove(&finder);
                    self.plugin_roles.lock().unwrap().remove(&finder);
                    return Err(format!("failed to load picker: {error:#}"));
                }
                self.senders
                    .send_to_screen(crate::add_suppressed_plugin_pane(
                        RunPluginOrAlias::RunPlugin(run.clone()),
                        finder,
                        run.initial_cwd,
                        client,
                    ))
                    .map_err(|error| format!("create picker driver: {error:#}"))?;
                Ok(None)
            },
            PickCommand::Cancel {
                request_id,
                timeout,
            } => {
                let finder = self
                    .picks
                    .iter()
                    .find(|(_, pick)| {
                        pick.caller == plugin
                            && pick.caller_instance == instance
                            && pick.request_id == request_id
                    })
                    .map(|(finder, _)| *finder);
                if let Some(finder) = finder {
                    self.retire_pick(
                        finder,
                        Err(if timeout {
                            "TimeoutError"
                        } else {
                            "AbortError"
                        }
                        .into()),
                    );
                }
                Ok(None)
            },
            PickCommand::Request => {
                let Some(pick) = self.picks.get_mut(&plugin) else {
                    return Ok(None);
                };
                if pick.result.is_some() {
                    return Err("picker was cancelled".into());
                }
                match pick.finder_instance {
                    None => pick.finder_instance = Some(instance),
                    Some(owner) if owner != instance => return Err("stale picker instance".into()),
                    Some(_) => {},
                }
                Ok(Some(pick.request.clone()))
            },
            PickCommand::Attach { pane_id } => {
                let pick = self
                    .picks
                    .get_mut(&plugin)
                    .ok_or("picker is no longer active")?;
                if pick.finder_instance.is_some_and(|owner| owner != instance) {
                    return Err("only the assigned picker instance can attach".into());
                }
                pick.finder_instance = Some(instance);
                if pick.pane.is_some() {
                    return Err("picker already has a terminal".into());
                }
                pick.pane = Some(pane_id);
                if pick.result.is_some() {
                    self.senders
                        .send_to_screen(ScreenInstruction::RetirePicker {
                            finder: plugin,
                            pane: Some(pane_id),
                            client: pick.client,
                        })
                        .map_err(|error| format!("close cancelled picker: {error:#}"))?;
                }
                Ok(None)
            },
            PickCommand::Complete { result } => {
                let Some(pick) = self.picks.get_mut(&plugin) else {
                    return Ok(None);
                };
                if pick.finder_instance.is_some_and(|owner| owner != instance) {
                    return Err("only the assigned picker instance can complete".into());
                }
                pick.finder_instance = Some(instance);
                let cancelled_open = pick.result.is_some()
                    && pick.pane.is_none()
                    && pick.opening.load(Ordering::SeqCst);
                pick.opening.store(false, Ordering::SeqCst);
                let result = result.and_then(|path| {
                    if path.as_ref().is_some_and(|path| {
                        !std::path::Path::new(path).is_absolute() || path.contains('\0')
                    }) {
                        Err("picker returned an invalid path".into())
                    } else {
                        Ok(path)
                    }
                });
                self.retire_pick(plugin, result);
                if let Some(pick) = self.picks.get(&plugin).filter(|_| cancelled_open) {
                    self.senders
                        .send_to_screen(ScreenInstruction::RetirePicker {
                            finder: plugin,
                            pane: None,
                            client: pick.client,
                        })
                        .map_err(|error| format!("retire failed picker startup: {error:#}"))?;
                }
                Ok(None)
            },
        }
    }

    fn retire_pick(&mut self, finder: PluginId, result: Result<Option<String>, String>) {
        let Some(pick) = self.picks.get_mut(&finder) else {
            return;
        };
        if pick.result.is_some() {
            return;
        }
        pick.cancelled.store(true, Ordering::SeqCst);
        pick.result = Some(result);
        // An in-flight open must attach or fail before screen retirement can finish.
        if pick.pane.is_some() || !pick.opening.load(Ordering::SeqCst) {
            self.senders
                .send_to_screen(ScreenInstruction::RetirePicker {
                    finder,
                    pane: pick.pane,
                    client: pick.client,
                })
                .context("retire picker overlay")
                .non_fatal();
        }
    }

    pub(crate) fn pick_retired(&mut self, finder: PluginId) {
        let Some(pick) = self.picks.remove(&finder) else {
            return;
        };
        self.senders
            .send_to_screen(ScreenInstruction::ClosePane(
                PaneId::Plugin(finder),
                Some(pick.client),
                None,
                None,
            ))
            .context("close picker driver")
            .non_fatal();
        if (self.shared_plugin_ids.contains(&pick.caller)
            || self
                .connected_clients
                .lock()
                .unwrap()
                .contains(&pick.client))
            && self
                .plugin_map
                .lock()
                .unwrap()
                .permission_instance(pick.caller, pick.caller_instance)
                .is_some()
        {
            self.senders
                .send_to_plugin(PluginInstruction::for_plugin_event(
                    pick.caller,
                    pick.client,
                    pick.generation,
                    Event::PickResult(PickResult {
                        request_id: pick.request_id,
                        result: pick
                            .result
                            .unwrap_or_else(|| Err("picker retirement failed".into())),
                    }),
                ))
                .context("deliver picker result")
                .non_fatal();
        }
    }

    pub(crate) fn cancel_plugin_picks(&mut self, plugin: PluginId) {
        let finders: Vec<_> = self
            .picks
            .iter()
            .filter(|(finder, pick)| **finder == plugin || pick.caller == plugin)
            .map(|(finder, _)| *finder)
            .collect();
        for finder in finders {
            self.retire_pick(
                finder,
                Err("picker or caller was unloaded or reloaded".into()),
            );
        }
    }

    pub(crate) fn cancel_client_picks(&mut self, client: ClientId) {
        let finders: Vec<_> = self
            .picks
            .iter()
            .filter(|(_, pick)| pick.client == client)
            .map(|(finder, _)| *finder)
            .collect();
        for finder in finders {
            self.retire_pick(finder, Err("picker client disconnected".into()));
        }
    }
}

fn merge_options(options: PickOptions, alias: Option<&String>) -> Result<PickOptions, String> {
    let mut merged: serde_json::Map<String, serde_json::Value> =
        match alias.filter(|json| !json.trim().is_empty()) {
            Some(json) => serde_json::from_str(json)
                .map_err(|error| format!("file-finder listing_json: {error}"))?,
            None => Default::default(),
        };
    let request = serde_json::to_value(options).map_err(|error| error.to_string())?;
    if let serde_json::Value::Object(request) = request {
        merged.extend(request.into_iter().filter(|(_, value)| !value.is_null()));
    }
    let options: PickOptions = serde_json::from_value(serde_json::Value::Object(merged))
        .map_err(|error| error.to_string())?;
    options.validate()?;
    Ok(options)
}

#[cfg(test)]
#[path = "unit/picker.rs"]
mod tests;
