use super::{PipeStateChange, PluginId, PluginInstruction};
use crate::plugin_bridge::render_plugin_for_clients;
use crate::plugin_map::RunningPlugin;
use crate::{ClientId, PluginRenderAsset};
use std::collections::{HashMap, HashSet};
use yinz_bus::plugins::{PipeCompletion, PipeRequestId, PluginInstanceId};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{PipeMessage, PipeSource};

type Owner = (PluginId, PluginInstanceId);
pub type PipeOutcomes = Vec<(String, Result<(), String>)>;

/// The host owns completion. Every removal records why the recipient finished.
#[derive(Debug, Default)]
pub struct PendingPipes {
    next_request: PipeRequestId,
    requests: HashMap<PipeRequestId, PendingPipe>,
}

#[derive(Debug, Default)]
struct PendingPipe {
    stream: String,
    queued: HashSet<(PluginId, u64)>,
    callbacks: HashSet<Owner>,
    holds: HashSet<Owner>,
    failure: Option<String>,
}

impl PendingPipes {
    pub fn begin(&mut self, stream: &str) -> PipeRequestId {
        self.next_request += 1;
        self.requests.insert(
            self.next_request,
            PendingPipe {
                stream: stream.to_owned(),
                ..Default::default()
            },
        );
        self.next_request
    }

    pub fn processing(
        &mut self,
        request: PipeRequestId,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) {
        if let Some(pipe) = self.requests.get_mut(&request) {
            pipe.callbacks.insert((plugin, instance));
        }
    }

    pub fn cache(&mut self, request: PipeRequestId, plugin: PluginId, load: u64) {
        if let Some(pipe) = self.requests.get_mut(&request) {
            pipe.queued.insert((plugin, load));
        }
    }

    pub fn finish_cached(
        &mut self,
        request: PipeRequestId,
        plugin: PluginId,
        load: u64,
        result: Result<(), String>,
    ) {
        if let Some(pipe) = self.requests.get_mut(&request)
            && pipe.queued.remove(&(plugin, load))
            && let Err(error) = result
        {
            pipe.failure.get_or_insert(error);
        }
    }

    pub fn fail(&mut self, request: PipeRequestId, error: String) {
        if let Some(pipe) = self.requests.get_mut(&request) {
            pipe.failure.get_or_insert(error);
        }
    }

    pub fn complete(&mut self, completion: PipeCompletion) {
        let owner = (completion.plugin_id, completion.instance_id);
        for (request, change) in completion.changes {
            if let Some(pipe) = self.requests.get_mut(&request) {
                match change {
                    PipeStateChange::Block
                        if pipe.callbacks.contains(&owner) || pipe.holds.contains(&owner) =>
                    {
                        pipe.holds.insert(owner);
                    },
                    PipeStateChange::Unblock => {
                        pipe.holds.remove(&owner);
                    },
                    _ => {},
                }
            }
        }
        if let Some((request, result)) = completion.callback
            && let Some(pipe) = self.requests.get_mut(&request)
            && pipe.callbacks.remove(&owner)
            && let Err(error) = result
        {
            pipe.holds.remove(&owner);
            pipe.failure.get_or_insert(error);
        }
    }

    pub fn has_recipient(&self, plugin: PluginId) -> bool {
        self.requests.values().any(|pipe| {
            pipe.queued.iter().any(|(id, _)| *id == plugin)
                || pipe
                    .callbacks
                    .iter()
                    .chain(&pipe.holds)
                    .any(|(id, _)| *id == plugin)
        })
    }

    pub fn reject_cached(&mut self, plugin: PluginId, load: Option<u64>, error: &str) {
        for pipe in self.requests.values_mut() {
            let before = pipe.queued.len();
            pipe.queued
                .retain(|(id, attempt)| *id != plugin || load.is_some_and(|load| load != *attempt));
            if pipe.queued.len() != before {
                pipe.failure.get_or_insert_with(|| error.to_owned());
            }
        }
    }

    /// Handles an executor retirement report after the last callback and guest cleanup return.
    pub fn retire(&mut self, plugin: PluginId, instance: PluginInstanceId, error: &str) {
        let owner = (plugin, instance);
        for pipe in self.requests.values_mut() {
            let callback = pipe.callbacks.remove(&owner);
            let hold = pipe.holds.remove(&owner);
            if callback || hold {
                pipe.failure.get_or_insert_with(|| error.to_owned());
            }
        }
    }

    pub fn retire_plugin(&mut self, plugin: PluginId, error: &str) {
        for pipe in self.requests.values_mut() {
            let before = pipe.callbacks.len() + pipe.holds.len();
            pipe.callbacks.retain(|(id, _)| *id != plugin);
            pipe.holds.retain(|(id, _)| *id != plugin);
            if pipe.callbacks.len() + pipe.holds.len() != before {
                pipe.failure.get_or_insert_with(|| error.to_owned());
            }
        }
    }

    /// Call only after all recipients in a dispatch/replay batch have been admitted.
    pub fn take_ready(&mut self) -> PipeOutcomes {
        let mut ready = vec![];
        self.requests.retain(|_, pipe| {
            if pipe.queued.is_empty() && pipe.callbacks.is_empty() && pipe.holds.is_empty() {
                ready.push((pipe.stream.clone(), pipe.failure.take().map_or(Ok(()), Err)));
                false
            } else {
                true
            }
        });
        ready
    }
}

/// Drain guest commands with the request IDs this runtime actually received. A late result
/// cannot complete another input on the same stream, or work belonging to a replacement.
pub fn report_pipe_changes(
    plugin: &mut RunningPlugin,
    mut changes: HashMap<String, PipeStateChange>,
    callback: Option<(PipeRequestId, Result<(), String>)>,
    senders: &ThreadSenders,
) {
    changes.extend(pipes_to_block_or_unblock(plugin));
    let mut resolved = HashMap::new();
    for (stream, change) in changes {
        if let Some(request) = plugin.pipe_requests.get(&stream).copied() {
            if matches!(change, PipeStateChange::Unblock) {
                plugin.pipe_requests.remove(&stream);
            }
            resolved.insert(request, change);
        }
    }
    if let Some((request, result)) = &callback
        && (result.is_err() || !matches!(resolved.get(request), Some(PipeStateChange::Block)))
    {
        plugin.pipe_requests.retain(|_, id| id != request);
    }
    if callback.is_some() || !resolved.is_empty() {
        senders
            .send_to_plugin(PluginInstruction::PipeCompleted(PipeCompletion {
                plugin_id: plugin.env().plugin_id,
                instance_id: plugin.instance_id,
                changes: resolved,
                callback,
            }))
            .context("failed to report pipe completion")
            .non_fatal();
    }
}

pub fn apply_pipe_message_to_plugin(
    plugin_id: PluginId,
    client_id: ClientId,
    render_clients: &[ClientId],
    running_plugin: &mut RunningPlugin,
    pipe_message: &PipeMessage,
    request: Option<PipeRequestId>,
    plugin_render_assets: &mut Vec<PluginRenderAsset>,
    senders: &ThreadSenders,
) -> Result<()> {
    if let (Some(request), PipeSource::Cli(stream)) = (request, &pipe_message.source) {
        running_plugin.pipe_requests.insert(stream.clone(), request);
    }
    let unavailable = running_plugin.is_unavailable();
    let result = (|| {
        anyhow::ensure!(
            !unavailable,
            "Plugin {plugin_id} is unavailable; explicitly reload it before sending another pipe"
        );
        running_plugin.env_mut().client_id = client_id;
        let render = running_plugin
            .pipe(pipe_message)
            .with_context(|| format!("Failed to apply pipe to plugin {plugin_id}"))?;
        if render == Some(true) && running_plugin.has_area() {
            plugin_render_assets.extend(render_plugin_for_clients(
                plugin_id,
                running_plugin,
                render_clients,
            )?);
        }
        Ok(())
    })();
    if result.is_err() && !unavailable {
        running_plugin.mark_failed();
    }
    report_pipe_changes(
        running_plugin,
        HashMap::new(),
        request.map(|request| {
            (
                request,
                result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| format!("{error:#}")),
            )
        }),
        senders,
    );
    result
}

pub fn pipes_to_block_or_unblock(
    running_plugin: &mut RunningPlugin,
) -> HashMap<String, PipeStateChange> {
    let mut pipe_state_changes = HashMap::new();
    let mut input_pipes_to_unblock: HashSet<String> = running_plugin
        .env()
        .input_pipes_to_unblock
        .lock()
        .unwrap()
        .drain()
        .collect();
    let mut input_pipes_to_block: HashSet<String> = running_plugin
        .env()
        .input_pipes_to_block
        .lock()
        .unwrap()
        .drain()
        .collect();
    for pipe in input_pipes_to_block.drain() {
        pipe_state_changes.insert(pipe, PipeStateChange::Block);
    }
    for pipe in input_pipes_to_unblock.drain() {
        // unblock has priority over block if they happened simultaneously
        pipe_state_changes.insert(pipe, PipeStateChange::Unblock);
    }
    pipe_state_changes
}

#[cfg(test)]
#[path = "unit/pipes.rs"]
mod tests;
