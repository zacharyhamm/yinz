use super::*;

pub(super) struct PendingPermission {
    identity: RunPlugin,
    pub permissions: Vec<PermissionType>,
    recipients: HashSet<(PluginId, PluginInstanceId, ClientId)>,
}

impl PendingPermission {
    pub(super) fn contains_plugin(&self, plugin: PluginId) -> bool {
        self.recipients.iter().any(|(id, _, _)| *id == plugin)
    }
}

impl PluginBridge {
    pub(crate) fn request_permissions(
        &mut self,
        plugin_id: PluginId,
        instance_id: PluginInstanceId,
        client_id: ClientId,
        permissions: Vec<PermissionType>,
        shutdown: Sender<()>,
    ) -> Result<()> {
        if self
            .plugin_map
            .lock()
            .unwrap()
            .permission_instance(plugin_id, instance_id)
            .is_none()
        {
            return Ok(());
        }
        let run = self
            .run_plugin_of_plugin_id(plugin_id)
            .context("permission request plugin disappeared")?;
        let run = self.loaded_plugin_identity(plugin_id, run);
        if let Some(pending) = self.permission_requests.values_mut().find(|pending| {
            pending.identity.location == run.location
                && pending.identity.configuration.without_caller_context()
                    == run.configuration.without_caller_context()
                && pending.permissions.iter().copied().collect::<HashSet<_>>()
                    == permissions.iter().copied().collect()
        }) {
            pending
                .recipients
                .insert((plugin_id, instance_id, client_id));
            self.plugin_callers.remove(&plugin_id);
            self.cache_plugin_events(plugin_id);
            return Ok(());
        }
        self.next_permission_request += 1;
        let request_id = self.next_permission_request;
        self.permission_requests.insert(
            request_id,
            PendingPermission {
                identity: run.clone(),
                permissions: permissions.clone(),
                recipients: HashSet::from([(plugin_id, instance_id, client_id)]),
            },
        );
        self.cache_plugin_events(plugin_id);
        let caller = self.plugin_callers.remove(&plugin_id);
        if PermissionCache::from_path_or_default(None)
            .check_permissions(run.location.to_string(), &permissions)
        {
            return self.answer_permissions(request_id, PermissionStatus::Granted, None, shutdown);
        }
        let mut prompt =
            yinz_types::data::PluginPermission::new(run.location.to_string(), permissions);
        prompt.request_id = request_id;
        prompt.client_id = Some(client_id);
        prompt.caller = caller;
        self.senders
            .send_to_screen(ScreenInstruction::RequestPluginPermissions(
                plugin_id, prompt,
            ))
            .context("show permission modal")
    }

    pub(crate) fn cancel_permission_requests(
        &mut self,
        plugin: PluginId,
        instance: Option<PluginInstanceId>,
    ) {
        let mut cancelled = vec![];
        self.permission_requests.retain(|token, pending| {
            pending.recipients.retain(|(id, current, _)| {
                *id != plugin || instance.is_some_and(|instance| instance != *current)
            });
            if pending.recipients.is_empty() {
                cancelled.push(*token);
                false
            } else {
                true
            }
        });
        for token in cancelled {
            self.senders
                .send_to_screen(ScreenInstruction::CancelPluginPermission(token))
                .non_fatal();
        }
        if !self
            .permission_requests
            .values()
            .any(|pending| pending.contains_plugin(plugin))
            && self
                .plugin_ids_waiting_for_permission_request
                .remove(&plugin)
        {
            if let Some(load_id) = self.load_ids.get(&plugin).copied() {
                self.senders
                    .send_to_plugin(PluginInstruction::ApplyCachedEvents {
                        load_id,
                        plugin_ids: vec![plugin],
                        done_receiving_permissions: true,
                    })
                    .context("replay events after permission cancellation")
                    .non_fatal();
            }
        }
    }

    pub(crate) fn answer_permissions(
        &mut self,
        token: u64,
        status: PermissionStatus,
        cache_path: Option<PathBuf>,
        shutdown: Sender<()>,
    ) -> Result<()> {
        let initializing = self.permission_requests.get(&token).and_then(|pending| {
            self.loading_plugins
                .iter()
                .find_map(|(id, _)| pending.contains_plugin(*id).then_some(*id))
        });
        if let Some(plugin) = initializing {
            // Let every initializing client join the request before consuming its answer.
            let cache_path = cache_path.clone();
            if self.plugin_executor.execute_for_plugin_tagged(
                plugin,
                "permission answer",
                false,
                move |senders, _, _| {
                    senders
                        .send_to_plugin(PluginInstruction::PermissionAnswer {
                            request_id: token,
                            status,
                            cache_path,
                        })
                        .context("defer permission answer until initialization finishes")
                        .non_fatal();
                },
            ) {
                return Ok(());
            }
        }
        let Some(pending) = self.permission_requests.remove(&token) else {
            return Ok(());
        };
        self.senders
            .send_to_screen(ScreenInstruction::CancelPluginPermission(token))
            .non_fatal();
        let plugins: HashSet<_> = pending.recipients.iter().map(|(id, _, _)| *id).collect();
        let mut answered = HashSet::new();
        let mut granted = false;
        for (plugin_id, instance_id, client) in pending.recipients {
            let runtime = self
                .plugin_map
                .lock()
                .unwrap()
                .permission_instance(plugin_id, instance_id);
            let Some((owner, runtime)) = runtime else {
                continue;
            };
            let mut instance = runtime.lock().unwrap();
            let env = instance.env_mut();
            if answered.insert((plugin_id, instance_id)) {
                // Denial answers the FIFO head, preserving all earlier grants.
                let mut permissions = env.permissions.lock().unwrap().clone().unwrap_or_default();
                if status == PermissionStatus::Granted {
                    if !granted {
                        // Save before callbacks can issue synchronous queries to this thread.
                        let mut cache = PermissionCache::from_path_or_default(cache_path.clone());
                        cache.grant(
                            pending.identity.location.to_string(),
                            pending.permissions.clone(),
                        );
                        cache
                            .write_to_file()
                            .context("save plugin permission grant")
                            .non_fatal();
                        granted = true;
                    }
                    permissions.extend(pending.permissions.iter().copied());
                }
                env.set_permissions(permissions);
                if let Some(workers) = &env.workers {
                    workers.permission_answer(env, &pending.permissions, status);
                }
            }
            let shared = env.plugin_config.is_shared();
            drop(instance);
            let shutdown = shutdown.clone();
            self.plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                "permissions",
                false,
                move |senders, _, clients| {
                    let _shutdown = shutdown;
                    let clients = if shared {
                        clients.lock().unwrap().clone()
                    } else {
                        vec![owner]
                    };
                    let mut runtime = runtime.lock().unwrap();
                    if runtime.is_unavailable() {
                        return;
                    }
                    let generation = runtime.generation();
                    let subscriptions = runtime.env().subscriptions.lock().unwrap().clone();
                    if generation.is_none()
                        && !subscriptions.contains(&EventType::PermissionRequestResult)
                    {
                        return;
                    }
                    let mut assets = vec![];
                    let result = apply_events_to_plugin(
                        plugin_id,
                        &clients,
                        &mut runtime,
                        &[(client, Event::PermissionRequestResult(status))],
                        &mut assets,
                        senders.clone(),
                        &subscriptions,
                    );
                    drop(runtime);
                    match result {
                        Ok(()) if !assets.is_empty() => senders
                            .send_to_screen(ScreenInstruction::PluginBytes(assets))
                            .context("render plugin permission result")
                            .non_fatal(),
                        Ok(()) => {},
                        Err(error) => handle_plugin_crash(
                            plugin_id,
                            generation,
                            Some(instance_id),
                            format!("{error:#}"),
                            senders,
                        ),
                    }
                },
            );
        }
        let ready = plugins
            .into_iter()
            .filter(|plugin| {
                !self
                    .permission_requests
                    .values()
                    .any(|pending| pending.contains_plugin(*plugin))
            })
            .collect();
        self.apply_cached_events(ready, true, shutdown)?;
        Ok(())
    }
}
