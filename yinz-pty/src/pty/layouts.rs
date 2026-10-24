//! Layout spawn preparation, placement and rollback.

use super::{pane_start_failure, send_pane_error_to_screen, Pty, SpawnJob};
use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use crate::os_input_output::AsyncReader;
use crate::plugins::{PluginId, PluginInstruction};
use crate::route::NotificationEnd;
use crate::screen::{ScreenInstruction, TabOverrideResult};
use crate::terminal_bytes::TerminalBytes;
use crate::{ClientId, ServerInstruction};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use yinz_bus::pty::{
    terminal_exit_instruction, AdoptedTabTerminal, AdoptedTabTerminals, PendingRemoteSpawn,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{CommandOrPlugin, OriginatingPlugin};
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::input::layout::{
    FloatingPaneLayout, Run, RunPluginOrAlias, SwapFloatingLayout, SwapTiledLayout, TabLayoutInfo,
    TiledPaneLayout,
};
use yinz_types::internal_ids::{TabId, TerminalId};
use yinz_types::pane_id::PaneId;

#[derive(Clone, Debug)]
struct PendingLayoutSpawn {
    layout: TiledPaneLayout,
    floating_panes_layout: Vec<FloatingPaneLayout>,
    default_shell: TerminalAction,
    plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
    adopted_panes: Option<AdoptedTabTerminals>,
    run_instructions: Vec<Option<Run>>,
    floating_run_instructions: Vec<Option<Run>>,
    tab_index: TabId,
    block_on_first_terminal: bool,
    should_change_focus_to_new_tab: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
}

#[derive(Clone, Debug)]
pub(super) struct PendingLayoutOverride {
    pub(super) cwd: Option<PathBuf>,
    pub(super) default_shell: Option<TerminalAction>,
    pub(super) tabs_to_close: Vec<TabId>,
    pub(super) tabs: Vec<(
        TabId,
        TabLayoutInfo,
        HashMap<RunPluginOrAlias, Vec<PluginId>>,
    )>,
    pub(super) retain_existing_terminal_panes: bool,
    pub(super) retain_existing_plugin_panes: bool,
    pub(super) client_id: ClientId,
    pub(super) completion_tx: Option<NotificationEnd>,
}

/// A pane a layout has spawned but not yet handed to Screen: its terminal id,
/// whether it starts held, the command it holds for, and the reader its bytes
/// will arrive on (`None` for a pane whose spawn was deferred).
pub(super) type SpawnedPane = (
    TerminalId,
    bool,
    Option<RunCommand>,
    Option<Result<Box<dyn AsyncReader>>>,
);

/// The panes one layout has spawned but not yet handed to Screen.
#[derive(Default)]
struct SpawnedLayoutPanes {
    tiled: Vec<SpawnedPane>,
    floating: Vec<SpawnedPane>,
    /// Remote panes whose spawn goes to a worker once Screen knows their ids.
    deferred: Vec<PendingRemoteSpawn>,
    /// Command panes a plugin asked for, told once their readers are running.
    originating_plugins: Vec<(TerminalId, OriginatingPlugin)>,
}

impl SpawnedLayoutPanes {
    pub(super) fn terminal_ids(&self) -> impl Iterator<Item = TerminalId> + '_ {
        self.tiled
            .iter()
            .chain(&self.floating)
            .map(|(terminal_id, ..)| *terminal_id)
    }

    /// The ids Screen places, tiled then floating, each with the command it
    /// holds for - `Some` only for a pane that starts held.
    fn placement_ids(&self) -> (PlacedPanes, PlacedPanes) {
        (held_ids(&self.tiled), held_ids(&self.floating))
    }
}

/// What Screen is told about a layout's panes: each id with the command it
/// holds for.
type PlacedPanes = Vec<(TerminalId, Option<RunCommand>)>;

fn held_ids(panes: &[SpawnedPane]) -> PlacedPanes {
    panes
        .iter()
        .map(|(terminal_id, starts_held, run_command, _)| {
            let hold_for_command = starts_held.then(|| run_command.clone()).flatten();
            (*terminal_id, hold_for_command)
        })
        .collect()
}

impl Pty {
    fn discard_deferred_spawns(&mut self, pending: &[PendingRemoteSpawn]) {
        for spawn in pending {
            self.close_pane(PaneId::Terminal(spawn.terminal_id))
                .with_context(|| {
                    format!(
                        "failed to close reserved pane {} of a failed layout",
                        spawn.terminal_id
                    )
                })
                .non_fatal();
        }
    }

    /// Release the panes a layout spawned but never handed to Screen.
    ///
    /// Screen only learns of a layout's panes through `ApplyLayout` or
    /// `OverrideLayoutComplete`. A layout that fails before one of those is sent
    /// still owns every child it started, and no `ClosePane` will ever arrive for
    /// them, so they must be closed here or they outlive the layout that asked
    /// for them.
    fn discard_layout_panes(&mut self, terminal_ids: impl IntoIterator<Item = TerminalId>) {
        for terminal_id in terminal_ids {
            self.close_pane(PaneId::Terminal(terminal_id))
                .with_context(|| format!("failed to close pane {terminal_id} of a failed layout"))
                .non_fatal();
        }
    }

    /// These ids belong to new instances loaded for this request, including on an override.
    fn discard_layout_plugins(&self, plugin_ids: impl IntoIterator<Item = PluginId>) {
        for plugin_id in plugin_ids {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Unload(plugin_id))
                .with_context(|| format!("failed to unload plugin {plugin_id} of a failed layout"))
                .non_fatal();
        }
    }

    /// Give up on a layout: close every pane it spawned, deferred or already
    /// running, so a failure part-way through leaves nothing behind.
    fn abandon_layout(&mut self, panes: &SpawnedLayoutPanes) {
        self.close_unplaced_layout_panes(panes.terminal_ids(), &panes.deferred);
    }

    /// Close the panes a layout spawned but never handed to Screen. A deferred
    /// pane holds a slot in both lists - it is closed once, through the deferred
    /// path, which names it in its own error context.
    fn close_unplaced_layout_panes(
        &mut self,
        spawned: impl IntoIterator<Item = TerminalId>,
        deferred: &[PendingRemoteSpawn],
    ) {
        let deferred_ids: HashSet<TerminalId> =
            deferred.iter().map(|spawn| spawn.terminal_id).collect();
        let spawned: Vec<TerminalId> = spawned
            .into_iter()
            .filter(|terminal_id| !deferred_ids.contains(terminal_id))
            .collect();
        self.discard_layout_panes(spawned);
        self.discard_deferred_spawns(deferred);
    }

    pub(crate) fn spawn_terminals_for_layout(
        &mut self,
        cwd: Option<PathBuf>,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        default_shell: Option<TerminalAction>,
        plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        initial_panes: Option<Vec<CommandOrPlugin>>,
        tab_index: TabId,
        block_on_first_terminal: bool,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        log::info!("startup: PTY NewTab tab={tab_index:?} client={client_id} begin");
        let mut default_shell =
            default_shell.unwrap_or_else(|| self.get_default_terminal(cwd, None));
        self.fill_cwd(&mut default_shell, client_id);

        // Match initial_panes commands to empty slots in the layout
        let mut layout = layout;
        if let Some(ref initial_panes_vec) = initial_panes {
            for initial_pane in initial_panes_vec.iter() {
                if let CommandOrPlugin::Command(run_command_action) = initial_pane {
                    let run_command: RunCommand = run_command_action.clone().into();
                    if !layout.replace_next_empty_slot_with_run(Run::Command(run_command)) {
                        log::warn!("More initial_panes provided than empty slots available");
                        break;
                    }
                } else if let CommandOrPlugin::File(file_to_open) = initial_pane
                    && !layout.replace_next_empty_slot_with_run(Run::EditFile(
                        file_to_open.path.clone(),
                        file_to_open.line_number,
                        file_to_open.cwd.clone(),
                    ))
                {
                    log::warn!("More initial_panes provided than empty slots available");
                    break;
                }
                // Skip CommandOrPlugin::Plugin entries (already handled by plugin thread)
            }
        }

        // A tab warped in from another session brings its panes with it. Their
        // supervisors are already attached and already have reader tasks, so
        // there is nothing here to spawn: an empty instruction list is what says
        // so, and the ids come from the queue instead, further down.
        //
        // Nothing else about the layout changes. Its plugin nodes were resolved
        // into `plugin_ids` by the plugin thread on the way here, exactly as they
        // are for any other new tab — which is how a warped tab gets its plugin
        // panes back.
        let adopted_panes = self.adopted_tabs.remove(&tab_index);

        let extracted_run_instructions = match &adopted_panes {
            Some(_) => vec![],
            None => layout.extract_run_instructions(),
        };
        let extracted_floating_run_instructions: Vec<Option<Run>> = match &adopted_panes {
            Some(_) => vec![],
            None => floating_panes_layout
                .iter()
                .filter(|f| !f.already_running)
                .map(|f| f.run.clone())
                .collect(),
        };
        let pending = PendingLayoutSpawn {
            layout,
            floating_panes_layout,
            default_shell,
            plugin_ids,
            adopted_panes,
            run_instructions: extracted_run_instructions,
            floating_run_instructions: extracted_floating_run_instructions,
            tab_index,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        };
        self.finish_layout_spawn(pending)
    }

    fn finish_layout_spawn(&mut self, pending: PendingLayoutSpawn) -> Result<()> {
        let startup_started = std::time::Instant::now();
        let err_context = || "failed to spawn terminals for layout for".to_string();
        let PendingLayoutSpawn {
            layout,
            floating_panes_layout,
            default_shell,
            plugin_ids,
            adopted_panes,
            run_instructions,
            floating_run_instructions,
            tab_index,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            mut completion_tx,
        } = pending;
        let panes = match self.spawn_layout_panes(
            run_instructions,
            floating_run_instructions,
            &default_shell,
            &plugin_ids,
        ) {
            Ok(panes) => panes,
            Err(error) => {
                let message = format!("Failed to spawn layout for tab {tab_index}: {error:#}");
                if let Some(completion) = completion_tx.as_mut() {
                    completion.set_exit_status(1);
                    completion.set_error_message(message.clone());
                }
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::LogError(vec![message], client_id, None))
                    .context("failed to report layout spawn failure")
                    .non_fatal();
                // Screen may already own panes moved into this pending tab.
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::LayoutSpawnFailed(
                        tab_index,
                        completion_tx.as_ref().and_then(|c| c.agent_tab.clone()),
                    ))
                    .context("failed to recover a tab whose layout could not spawn")
                    .non_fatal();
                return Err(error);
            },
        };

        let adopted_ids = |terminals: &[AdoptedTabTerminal]| {
            terminals
                .iter()
                .map(|terminal| (terminal.terminal_id, terminal.hold_for_command()))
                .collect::<Vec<_>>()
        };
        let (new_tab_pane_ids, new_tab_floating_pane_ids) = match &adopted_panes {
            Some((tiled, floating)) => (adopted_ids(tiled), adopted_ids(floating)),
            None => panes.placement_ids(),
        };
        let adopted_terminals = match &adopted_panes {
            Some((tiled, floating)) => tiled
                .iter()
                .chain(floating)
                .map(|terminal| {
                    (
                        terminal.terminal_id,
                        terminal.cmd.clone(),
                        terminal.agent.clone(),
                    )
                })
                .collect(),
            None => vec![],
        };

        // Track the first terminal_id if blocking is requested
        let first_initial_pane_terminal_id = if block_on_first_terminal {
            panes.tiled.first().map(|(terminal_id, ..)| *terminal_id)
        } else {
            None
        };

        log::info!(
            "startup: PTY NewTab tab={tab_index:?} client={client_id} sending ApplyLayout at {:?}",
            startup_started.elapsed()
        );
        // Prepare blocking_terminal for ApplyLayout
        let (direct_completion_tx, blocking_terminal) =
            if let Some(terminal_id) = first_initial_pane_terminal_id {
                (None, completion_tx.map(|tx| (terminal_id, tx)))
            } else {
                (completion_tx, None)
            };

        log::info!(
            "spawn_terminals_for_layout: {} tiled + {} floating panes created, sending ApplyLayout",
            new_tab_pane_ids.len(),
            new_tab_floating_pane_ids.len()
        );
        let plugin_ids_to_close: Vec<_> = plugin_ids.values().flatten().copied().collect();
        if let Err(error) = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::ApplyLayout(
                layout,
                floating_panes_layout,
                new_tab_pane_ids.clone(),
                new_tab_floating_pane_ids.clone(),
                adopted_terminals,
                plugin_ids,
                tab_index,
                should_change_focus_to_new_tab,
                client_id,
                direct_completion_tx,
                blocking_terminal,
            ))
        {
            self.abandon_layout(&panes);
            self.discard_layout_plugins(plugin_ids_to_close);
            return Err(error).with_context(err_context);
        }

        // Behind the `ApplyLayout` on the same channel, so the panes exist by
        // the time these land. An adopted pane whose child is already gone is
        // then held, showing the status it exited with, or closed if it would
        // not have been held in its old session either.
        if let Some((tiled, floating)) = adopted_panes {
            for terminal in tiled.into_iter().chain(floating) {
                if !terminal.replay.is_empty() {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::PtyBytes(
                            terminal.terminal_id,
                            terminal.replay,
                        ))
                        .with_context(err_context)?;
                }
                if terminal.child_exit_status.is_none()
                    && terminal.held.is_none()
                    && let Some(input) = terminal.agent_input
                {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::RestoreWarpedAgentInput(
                            PaneId::Terminal(terminal.terminal_id),
                            input,
                        ))
                        .with_context(err_context)?;
                }
                if let Some(caption) = terminal.caption {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::SetPaneCaption(
                            PaneId::Terminal(terminal.terminal_id),
                            caption,
                        ))
                        .with_context(err_context)?;
                }
                if let Some(exit_status) = terminal.child_exit_status {
                    let pane_id = PaneId::Terminal(terminal.terminal_id);
                    let hold_on_close = terminal.cmd.hold_on_close;
                    let instruction = if let Some(held) = &terminal.held {
                        ScreenInstruction::RestoreHeldPane(
                            pane_id,
                            yinz_types::data::HeldTerminal {
                                command: terminal.cmd,
                                ..held.clone()
                            },
                        )
                    } else {
                        terminal_exit_instruction(pane_id, exit_status, terminal.cmd, hold_on_close)
                    };
                    self.bus
                        .senders
                        .send_to_screen(instruction)
                        .with_context(err_context)?;
                }
                if terminal.held.is_none() && terminal.child_exit_status.is_none() {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::RequestPaneRedraw(terminal.terminal_id))
                        .context("failed to request redraw for adopted tab pane")
                        .non_fatal();
                }
                if terminal.held.is_some() {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::SetPaneAgent(
                            PaneId::Terminal(terminal.terminal_id),
                            terminal.agent,
                            None,
                        ))
                        .with_context(err_context)?;
                }
            }
        }

        for deferred in self.start_layout_panes(panes)? {
            self.defer_spawn(SpawnJob::Terminal(deferred))?;
        }
        Ok(())
    }

    /// Spawn the panes a layout asks for, one slot at a time. The first slot
    /// that cannot be spawned abandons everything spawned so far: Screen has
    /// not heard of any of it yet, so nothing else would ever close them.
    fn spawn_layout_panes(
        &mut self,
        tiled: Vec<Option<Run>>,
        floating: Vec<Option<Run>>,
        default_shell: &TerminalAction,
        plugin_ids: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
    ) -> Result<SpawnedLayoutPanes> {
        let mut panes = SpawnedLayoutPanes::default();
        for (instructions, is_floating) in [(tiled, false), (floating, true)] {
            if let Err(error) =
                self.spawn_layout_slots(instructions, default_shell, &mut panes, is_floating)
            {
                self.abandon_layout(&panes);
                self.discard_layout_plugins(plugin_ids.values().flatten().copied());
                return Err(error);
            }
        }
        Ok(panes)
    }

    fn spawn_layout_slots(
        &mut self,
        instructions: Vec<Option<Run>>,
        default_shell: &TerminalAction,
        panes: &mut SpawnedLayoutPanes,
        is_floating: bool,
    ) -> Result<()> {
        for run_instruction in instructions {
            // Taken before `apply_run_instruction` consumes the instruction.
            let originating_plugin = match &run_instruction {
                Some(Run::Command(run_command)) => run_command.originating_plugin.clone(),
                _ => None,
            };
            let Some(spawned) = self.apply_run_instruction(
                run_instruction,
                default_shell.clone(),
                &mut panes.deferred,
            )?
            else {
                continue;
            };
            if let Some(originating_plugin) = originating_plugin {
                panes
                    .originating_plugins
                    .push((spawned.0, originating_plugin));
            }
            if is_floating {
                panes.floating.push(spawned);
            } else {
                panes.tiled.push(spawned);
            }
        }
        Ok(())
    }

    /// Start listening to the panes a layout spawned, tiled then floating: a
    /// pane that starts held is left alone, one whose spawn was deferred has no
    /// reader yet, and one that could not start is shown its failure or closed.
    /// Then the plugins that asked for command panes are told. Returns the
    /// deferred spawns for the caller to hand to workers at its own moment.
    fn start_layout_panes(&mut self, panes: SpawnedLayoutPanes) -> Result<Vec<PendingRemoteSpawn>> {
        let err_context = || "failed to spawn terminals for layout for".to_string();
        let SpawnedLayoutPanes {
            tiled,
            floating,
            deferred,
            originating_plugins,
        } = panes;
        for (terminal_id, starts_held, run_command, reader_result) in
            tiled.into_iter().chain(floating)
        {
            if starts_held {
                // we do not run a command or start listening for bytes on held panes
                continue;
            }
            let Some(reader_result) = reader_result else {
                continue;
            };
            match reader_result {
                Ok(reader) => {
                    let activity_flag =
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let terminal_bytes = async_runtime().spawn({
                        let senders = self.bus.senders.clone();
                        let debug_to_file = self.debug_to_file;
                        let activity_flag = activity_flag.clone();
                        async move {
                            TerminalBytes::new(
                                terminal_id,
                                reader,
                                senders,
                                debug_to_file,
                                activity_flag,
                            )
                            .listen()
                            .await
                            .context("failed to spawn terminals for layout")
                            .fatal();
                        }
                    });
                    self.task_handles.insert(terminal_id, terminal_bytes);
                    self.pane_activity_flags.insert(terminal_id, activity_flag);
                    self.report_terminal_host(terminal_id);
                },
                Err(err) => match run_command {
                    Some(run_command) if run_command.shows_start_failure() => {
                        let message = pane_start_failure(&err)
                            .map(|(_, message)| message)
                            .unwrap_or_else(|| format!("{err:#}"));
                        send_pane_error_to_screen(
                            self.bus.senders.clone(),
                            terminal_id,
                            message,
                            run_command.clone(),
                        )
                        .with_context(err_context)?;
                    },
                    _ => {
                        self.close_pane(PaneId::Terminal(terminal_id))
                            .with_context(err_context)?;
                    },
                },
            }
        }
        for (terminal_id, originating_plugin) in originating_plugins {
            let _ = self.inform_originating_plugin_of_open(terminal_id, originating_plugin);
        }
        Ok(deferred)
    }

    pub(super) fn finish_layout_override(&mut self, pending: PendingLayoutOverride) -> Result<()> {
        let err_context = || "failed to override layout";
        let mut all_tab_results = vec![];
        let mut failed_tabs = vec![];
        let mut deferred_spawns = vec![];
        for (tab_index, tab, plugin_ids) in pending.tabs {
            match self.spawn_terminals_for_layout_override(
                pending.cwd.clone(),
                tab.tiled_layout,
                tab.floating_layouts,
                pending.default_shell.clone(),
                plugin_ids,
                tab_index,
                tab.tab_name,
                pending.client_id,
                tab.swap_tiled_layouts,
                tab.swap_floating_layouts,
            ) {
                Ok((tab_result, mut tab_deferred_spawns)) => {
                    all_tab_results.push(tab_result);
                    deferred_spawns.append(&mut tab_deferred_spawns);
                },
                Err(error) => {
                    failed_tabs.push(tab_index);
                    Err::<(), _>(error)
                        .with_context(|| format!("failed to spawn terminals for tab {tab_index}"))
                        .non_fatal();
                },
            }
        }
        // The results carry the panes away with them, so note the ids while they
        // are still here: if the send fails, Screen never hears of these panes and
        // this is the last chance to close them.
        let spawned_ids: Vec<TerminalId> = all_tab_results
            .iter()
            .flat_map(|tab_result| {
                tab_result
                    .new_terminal_pids
                    .iter()
                    .chain(&tab_result.new_floating_pane_pids)
            })
            .map(|(terminal_id, _)| *terminal_id)
            .collect();
        let plugin_ids_to_close: Vec<_> = all_tab_results
            .iter()
            .flat_map(|tab_result| tab_result.plugin_ids.values().flatten().copied())
            .collect();
        if let Err(error) =
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::OverrideLayoutComplete(
                    all_tab_results,
                    failed_tabs,
                    pending.tabs_to_close,
                    pending.retain_existing_terminal_panes,
                    pending.retain_existing_plugin_panes,
                    pending.client_id,
                    pending.completion_tx,
                ))
        {
            self.close_unplaced_layout_panes(spawned_ids, &deferred_spawns);
            self.discard_layout_plugins(plugin_ids_to_close);
            return Err(error).with_context(err_context);
        }
        for deferred in deferred_spawns {
            self.defer_spawn(SpawnJob::Terminal(deferred))?;
        }
        Ok(())
    }

    pub(super) fn spawn_terminals_for_layout_override(
        &mut self,
        cwd: Option<PathBuf>,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        default_shell: Option<TerminalAction>,
        plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        tab_index: TabId,
        tab_name: Option<String>,
        client_id: ClientId,
        swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
    ) -> Result<(TabOverrideResult, Vec<PendingRemoteSpawn>)> {
        let mut default_shell =
            default_shell.unwrap_or_else(|| self.get_default_terminal(cwd, None));
        self.fill_cwd(&mut default_shell, client_id);

        let run_instructions = layout.extract_run_instructions();
        let floating_run_instructions: Vec<Option<Run>> = floating_panes_layout
            .iter()
            .filter(|f| !f.already_running)
            .map(|f| f.run.clone())
            .collect();
        let panes = self.spawn_layout_panes(
            run_instructions,
            floating_run_instructions,
            &default_shell,
            &plugin_ids,
        )?;
        let (new_terminal_pids, new_floating_pane_pids) = panes.placement_ids();
        let tab_result = TabOverrideResult {
            tab_index,
            tab_name,
            tiled_layout: layout,
            floating_layouts: floating_panes_layout,
            swap_tiled_layouts,
            swap_floating_layouts,
            new_terminal_pids,
            new_floating_pane_pids,
            plugin_ids,
        };
        let spawned_ids: Vec<_> = panes.terminal_ids().collect();
        let deferred = match self.start_layout_panes(panes) {
            Ok(deferred) => deferred,
            Err(error) => {
                self.discard_layout_panes(spawned_ids);
                self.discard_layout_plugins(tab_result.plugin_ids.values().flatten().copied());
                return Err(error);
            },
        };
        Ok((tab_result, deferred))
    }
}
