use super::{PipeStateChange, PluginId, PluginInstruction};
use crate::plugin_map::RunningPlugin;
use crate::wasm_bridge::render_plugin_for_clients;
use crate::PluginRenderAsset;
use std::collections::{HashMap, HashSet};
use {zellij_types::data::PipeMessage, zellij_types::data::PipeSource};

use zellij_runtime::errors::prelude::*;

use crate::ClientId;
use zellij_bus::thread_bus::ThreadSenders;

#[derive(Debug, Clone, Default)]
pub struct PendingPipes {
    pipes: HashMap<String, PendingPipeInfo>,
}

impl PendingPipes {
    pub fn mark_being_processed(
        &mut self,
        pipe_id: &str,
        plugin_id: &PluginId,
        client_id: &ClientId,
    ) {
        if self.pipes.contains_key(pipe_id) {
            if let Some(pending_pipe_info) = self.pipes.get_mut(pipe_id) {
                pending_pipe_info.add_processing_plugin(plugin_id, client_id)
            }
        } else {
            self.pipes.insert(
                pipe_id.to_owned(),
                PendingPipeInfo::new(plugin_id, client_id),
            );
        }
    }
    // returns a list of pipes that are no longer pending and should be unblocked
    pub fn update_pipe_state_change(
        &mut self,
        cli_pipe_name: &str,
        pipe_state_change: PipeStateChange,
        plugin_id: &PluginId,
        client_id: &ClientId,
    ) -> Vec<String> {
        let mut pipe_names_to_unblock = vec![];
        match self.pipes.get_mut(cli_pipe_name) {
            Some(pending_pipe_info) => {
                let should_unblock_this_pipe =
                    pending_pipe_info.update_state_change(pipe_state_change, plugin_id, client_id);
                if should_unblock_this_pipe {
                    pipe_names_to_unblock.push(cli_pipe_name.to_owned());
                }
            },
            None => {
                // state somehow corrupted, let's recover...
                pipe_names_to_unblock.push(cli_pipe_name.to_owned());
            },
        }
        for pipe_name in &pipe_names_to_unblock {
            self.pipes.remove(pipe_name);
        }
        pipe_names_to_unblock
    }
    // returns a list of pipes that are no longer pending and should be unblocked
    pub fn unload_plugin(&mut self, plugin_id: &PluginId) -> Vec<String> {
        let mut pipe_names_to_unblock = vec![];
        for (pipe_name, pending_pipe_info) in self.pipes.iter_mut() {
            let should_unblock_this_pipe = pending_pipe_info.unload_plugin(plugin_id);
            if should_unblock_this_pipe {
                pipe_names_to_unblock.push(pipe_name.to_owned());
            }
        }
        for pipe_name in &pipe_names_to_unblock {
            self.pipes.remove(pipe_name);
        }
        pipe_names_to_unblock
    }
}

#[derive(Debug, Clone, Default)]
pub struct PendingPipeInfo {
    is_explicitly_blocked: bool,
    currently_being_processed_by: HashSet<(PluginId, ClientId)>,
}

impl PendingPipeInfo {
    pub fn new(plugin_id: &PluginId, client_id: &ClientId) -> Self {
        let mut currently_being_processed_by = HashSet::new();
        currently_being_processed_by.insert((*plugin_id, *client_id));
        PendingPipeInfo {
            currently_being_processed_by,
            ..Default::default()
        }
    }
    pub fn add_processing_plugin(&mut self, plugin_id: &PluginId, client_id: &ClientId) {
        self.currently_being_processed_by
            .insert((*plugin_id, *client_id));
    }
    // returns true if this pipe should be unblocked
    pub fn update_state_change(
        &mut self,
        pipe_state_change: PipeStateChange,
        plugin_id: &PluginId,
        client_id: &ClientId,
    ) -> bool {
        match pipe_state_change {
            PipeStateChange::Block => {
                self.is_explicitly_blocked = true;
            },
            PipeStateChange::Unblock => {
                self.is_explicitly_blocked = false;
            },
            _ => {},
        };
        self.currently_being_processed_by
            .remove(&(*plugin_id, *client_id));
        self.currently_being_processed_by.is_empty() && !self.is_explicitly_blocked
    }
    // returns true if this pipe should be unblocked
    pub fn unload_plugin(&mut self, plugin_id_to_unload: &PluginId) -> bool {
        self.currently_being_processed_by
            .retain(|(plugin_id, _)| plugin_id != plugin_id_to_unload);
        self.currently_being_processed_by.is_empty() && !self.is_explicitly_blocked
    }
}

/// Hands `pipe_message` to the plugin, and makes sure the CLI pipe it came from (if any) is
/// released whatever happens: every success path below unblocks it, and a plugin that traps while
/// handling the message is not unloaded (`handle_plugin_crash` only paints the error), so without
/// the release here the `zellij pipe` client would stay blocked until the plugin is unloaded or
/// the session ends.
pub fn apply_pipe_message_to_plugin(
    plugin_id: PluginId,
    client_id: ClientId,
    render_clients: &[ClientId],
    running_plugin: &mut RunningPlugin,
    pipe_message: &PipeMessage,
    plugin_render_assets: &mut Vec<PluginRenderAsset>,
    senders: &ThreadSenders,
) -> Result<()> {
    let result = apply_pipe_message_to_plugin_inner(
        plugin_id,
        client_id,
        render_clients,
        running_plugin,
        pipe_message,
        plugin_render_assets,
        senders,
    );
    if result.is_err() {
        release_pipe_of_crashed_plugin(plugin_id, running_plugin, pipe_message, senders);
    }
    result
}

fn release_pipe_of_crashed_plugin(
    plugin_id: PluginId,
    running_plugin: &RunningPlugin,
    pipe_message: &PipeMessage,
    senders: &ThreadSenders,
) {
    if let PipeSource::Cli(pipe_id) = &pipe_message.source {
        let mut pipe_state_changes = HashMap::new();
        pipe_state_changes.insert(pipe_id.to_owned(), PipeStateChange::NoChange);
        // the pipe was marked pending under the instance's owner, which is also what the success
        // paths release it under
        let plugin_render_asset =
            PluginRenderAsset::new(plugin_id, running_plugin.env().owner_client_id, vec![])
                .with_pipes(pipe_state_changes);
        senders
            .send_to_plugin(PluginInstruction::UnblockCliPipes(vec![
                plugin_render_asset,
            ]))
            .context("failed to unblock input pipe of crashed plugin")
            .non_fatal();
    }
}

fn apply_pipe_message_to_plugin_inner(
    plugin_id: PluginId,
    client_id: ClientId,
    render_clients: &[ClientId],
    running_plugin: &mut RunningPlugin,
    pipe_message: &PipeMessage,
    plugin_render_assets: &mut Vec<PluginRenderAsset>,
    senders: &ThreadSenders,
) -> Result<()> {
    // host commands issued while handling this message act on behalf of the client it was directed
    // at (this only differs from the instance's owner for shared plugins)
    running_plugin.env_mut().client_id = client_id;
    let rows = running_plugin.rows;
    let columns = running_plugin.columns;

    let err_context = || format!("Failed to apply event to plugin {plugin_id}");
    match running_plugin
        .pipe(pipe_message)
        .with_context(err_context)?
    {
        Some(should_render) => {
            if rows > 0 && columns > 0 && should_render {
                plugin_render_assets.append(&mut render_plugin_for_clients(
                    plugin_id,
                    running_plugin,
                    render_clients,
                    Some(&pipe_message.source),
                )?);
            } else {
                let pipes_to_block_or_unblock =
                    pipes_to_block_or_unblock(running_plugin, Some(&pipe_message.source));
                let plugin_render_asset =
                    PluginRenderAsset::new(plugin_id, running_plugin.env().owner_client_id, vec![])
                        .with_pipes(pipes_to_block_or_unblock);
                let _ = senders
                    .send_to_plugin(PluginInstruction::UnblockCliPipes(vec![
                        plugin_render_asset,
                    ]))
                    .context("failed to unblock input pipe");
            }
        },
        None => {
            // no-op, this is probably an old plugin that does not have this interface
            // we don't log this error because if we do the logs will be super crowded
            let pipes_to_block_or_unblock =
                pipes_to_block_or_unblock(running_plugin, Some(&pipe_message.source));
            let plugin_render_asset = PluginRenderAsset::new(
                plugin_id,
                running_plugin.env().owner_client_id,
                vec![], // nothing to render
            )
            .with_pipes(pipes_to_block_or_unblock);
            let _ = senders
                .send_to_plugin(PluginInstruction::UnblockCliPipes(vec![
                    plugin_render_asset,
                ]))
                .context("failed to unblock input pipe");
        },
    }
    Ok(())
}

pub fn pipes_to_block_or_unblock(
    running_plugin: &mut RunningPlugin,
    current_pipe: Option<&PipeSource>,
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
    if let Some(PipeSource::Cli(current_pipe)) = current_pipe {
        pipe_state_changes.insert(current_pipe.to_owned(), PipeStateChange::NoChange);
    }
    for pipe in input_pipes_to_block.drain() {
        pipe_state_changes.insert(pipe, PipeStateChange::Block);
    }
    for pipe in input_pipes_to_unblock.drain() {
        // unblock has priority over block if they happened simultaneously
        pipe_state_changes.insert(pipe, PipeStateChange::Unblock);
    }
    pipe_state_changes
}
