//! Pane process, working-directory and remote-host metadata.

use super::Pty;
use crate::plugins::PluginInstruction;
use crate::route::NotificationEnd;
use crate::screen::ScreenInstruction;
use crate::session_layout_metadata::SessionLayoutMetadata;
use crate::ClientId;
use std::collections::HashMap;
use std::path::PathBuf;
use yinz_bus::pty::get_default_shell;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{
    Event, GetPaneCwdResponse, GetPaneGitProbeResponse, GetPanePidResponse,
    GetPaneRunningCommandResponse, GetPanesMetadataResponse, OverlayPlacement, PaneGitProbe,
    PaneProcessMetadata,
};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::{ProcessId, TabId, TerminalId};
use yinz_types::pane_id::PaneId;
use yinz_types::pane_size::Size;

impl Pty {
    /// Which machine a pane's cwd would be a path on, if it is a terminal pane.
    ///
    /// `None` means this one. Used to keep a cwd from being inherited across a
    /// machine boundary, where it names a directory that need not exist.
    pub(super) fn host_of_pane(&self, pane_id: &PaneId) -> Option<String> {
        match pane_id {
            PaneId::Terminal(id) => self
                .bus
                .os_input
                .as_ref()
                .and_then(|input| input.remote_pane_host(*id)),
            PaneId::Plugin(_) => None,
        }
    }

    /// Whether a cwd from `source` can be handed to `terminal_action`.
    ///
    /// Only if both run on the same machine. Inheriting a path across a machine
    /// boundary is inheriting a path that is not there — and for a pane that
    /// asked for a login shell, "not there" is exactly the wrong answer: with no
    /// cwd it lands in the remote home directory, which is where ssh puts you.
    fn cwd_may_be_inherited(&self, terminal_action: &TerminalAction, source: &PaneId) -> bool {
        let target_host = match terminal_action {
            TerminalAction::RunCommand(run_command) => run_command.display_host(),
            TerminalAction::OpenFile(_) => None,
        };
        target_host.as_deref() == self.host_of_pane(source).as_deref()
    }

    pub(super) fn fill_cwd(&self, terminal_action: &mut TerminalAction, client_id: ClientId) {
        if let Some(active_pane) = self.active_panes.get(&client_id)
            && !self.cwd_may_be_inherited(terminal_action, active_pane)
        {
            return;
        }
        let cwd = match terminal_action {
            TerminalAction::RunCommand(run_command) => &mut run_command.cwd,
            TerminalAction::OpenFile(payload) => &mut payload.cwd,
        };
        if cwd.is_none() {
            *cwd = self
                .active_panes
                .get(&client_id)
                .and_then(|pane| match pane {
                    PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
                    PaneId::Terminal(id) => {
                        // Try to get CWD from OS, fall back to cached value
                        self.id_to_child_pid
                            .get(id)
                            .and_then(|&pid| {
                                self.bus
                                    .os_input
                                    .as_ref()
                                    .and_then(|input| input.get_cwd(pid))
                            })
                            .or_else(|| self.terminal_cwds.get(id).cloned())
                    },
                })
        };
    }

    pub(super) fn fill_cwd_from_pane_id(
        &self,
        terminal_action: &mut TerminalAction,
        pane_id: &PaneId,
    ) {
        if !self.cwd_may_be_inherited(terminal_action, pane_id) {
            return;
        }
        let cwd = match terminal_action {
            TerminalAction::RunCommand(run_command) => &mut run_command.cwd,
            TerminalAction::OpenFile(payload) => &mut payload.cwd,
        };
        if cwd.is_none() {
            *cwd = match pane_id {
                PaneId::Terminal(terminal_pane_id) => {
                    // Try to get CWD from OS, fall back to cached value
                    self.id_to_child_pid
                        .get(terminal_pane_id)
                        .and_then(|&pid| {
                            self.bus
                                .os_input
                                .as_ref()
                                .and_then(|input| input.get_cwd(pid))
                        })
                        .or_else(|| self.terminal_cwds.get(terminal_pane_id).cloned())
                },
                PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
            };
        };
    }

    /// The argv to report for a pane's own process: the command inside the sandbox for a
    /// Bubblewrap pane, and the process's own otherwise.
    fn pane_argv(&self, terminal_id: TerminalId, argv: Vec<String>) -> Vec<String> {
        if !self
            .terminal_launch_commands
            .get(&terminal_id)
            .is_some_and(|command| command.target.is_bubblewrap())
        {
            return argv;
        }
        match yinz_types::agent::bubblewrap_inner_argv(&argv) {
            Some(inner) => inner.to_vec(),
            None => argv,
        }
    }

    pub(crate) fn populate_session_layout_metadata(
        &mut self,
        session_layout_metadata: &mut SessionLayoutMetadata,
    ) {
        let terminal_ids = session_layout_metadata.all_terminal_ids();
        let mut terminal_ids_to_commands: HashMap<TerminalId, Vec<String>> = HashMap::new();
        let mut terminal_ids_to_cwds: HashMap<TerminalId, PathBuf> = HashMap::new();

        let pids: Vec<_> = terminal_ids
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id))
            .copied()
            .collect();
        let (pids_to_cwds, pids_to_cmds) = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_cwds(pids))
            .unwrap_or_default();
        let panes: Vec<(TerminalId, ProcessId)> = terminal_ids
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id).map(|pid| (*id, *pid)))
            .collect();
        let foreground_cmds = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_foreground_cmds(&panes, &self.post_command_discovery_hook))
            .unwrap_or_default();

        for terminal_id in terminal_ids {
            let process_id = self.id_to_child_pid.get(&terminal_id);
            let cwd = process_id.and_then(|pid| pids_to_cwds.get(pid));
            let cmd_sysinfo = process_id.and_then(|pid| pids_to_cmds.get(pid));
            let cmd_foreground = foreground_cmds.get(&terminal_id);
            if let Some(cmd) = cmd_foreground {
                terminal_ids_to_commands.insert(terminal_id, cmd.clone());
            } else if let Some(cmd) = cmd_sysinfo {
                terminal_ids_to_commands.insert(terminal_id, cmd.clone());
            }
            if let Some(cwd) = cwd {
                terminal_ids_to_cwds.insert(terminal_id, cwd.clone());
            }
        }
        session_layout_metadata.update_default_shell(get_default_shell());
        session_layout_metadata.update_terminal_commands(terminal_ids_to_commands);
        session_layout_metadata.update_terminal_cwds(terminal_ids_to_cwds);
        session_layout_metadata.update_default_editor(&self.default_editor);
        session_layout_metadata.detect_editor_panes();
    }

    fn focused_plugin_launch_cwd(&self, client_id: ClientId) -> Option<PathBuf> {
        self.active_panes
            .get(&client_id)
            .and_then(|pane| match pane {
                PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
                PaneId::Terminal(id) => {
                    // Try to get CWD from OS, fall back to cached value
                    self.id_to_child_pid
                        .get(id)
                        .and_then(|&pid| {
                            self.bus
                                .os_input
                                .as_ref()
                                .and_then(|input| input.get_cwd(pid))
                        })
                        .or_else(|| self.terminal_cwds.get(id).cloned())
                },
            })
    }

    fn focused_terminal_pane_id(&self, client_id: ClientId) -> Option<TerminalId> {
        match self.active_panes.get(&client_id) {
            Some(PaneId::Terminal(id)) => Some(*id),
            _ => None,
        }
    }

    pub(super) fn resolve_overlay_tab_cwd(
        &self,
        id: yinz_types::pane_tabs::PaneTabsId,
        client_id: ClientId,
        cwd: Option<PathBuf>,
    ) -> Result<()> {
        let caller_cwd = self.focused_plugin_launch_cwd(client_id);
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::OverlayTabCwd {
                id,
                cwd: cwd.or_else(|| caller_cwd.clone()),
                caller_cwd,
                caller_terminal_pane_id: self.focused_terminal_pane_id(client_id),
            })
            .context("failed to resolve overlay tab cwd")
    }

    pub(crate) fn fill_plugin_cwd(
        &self,
        should_float: Option<bool>,
        should_open_in_place: bool, // should be opened in place
        close_replaced_pane: bool,  // close_replaced_pane
        pane_title: Option<String>, // pane title
        mut run: RunPluginOrAlias,
        tab_index: TabId,                   // tab index
        pane_id_to_replace: Option<PaneId>, // pane id to replace if this is to be opened "in-place"
        client_id: ClientId,
        size: Size,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        should_focus_plugin: Option<bool>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        completion_tx: Option<NotificationEnd>,
        overlay_placement: Option<OverlayPlacement>,
    ) -> Result<()> {
        let cwd = cwd.or_else(|| self.focused_plugin_launch_cwd(client_id));
        let focused_plugin_id = self
            .active_panes
            .get(&client_id)
            .and_then(|pane| match pane {
                PaneId::Plugin(plugin_id) => Some(*plugin_id),
                _ => None,
            });

        if let RunPluginOrAlias::Alias(alias) = &mut run {
            let cwd = self.focused_plugin_launch_cwd(client_id);
            alias.set_caller_cwd_if_not_set(cwd);
            let pane_id = self.focused_terminal_pane_id(client_id);
            alias.set_caller_terminal_pane_id_if_not_set(pane_id);
        }
        self.bus.senders.send_to_plugin(PluginInstruction::Load(
            should_float,
            should_open_in_place,
            close_replaced_pane,
            pane_title,
            run,
            Some(tab_index),
            pane_id_to_replace,
            client_id,
            size,
            cwd,
            focused_plugin_id,
            skip_cache,
            should_focus_plugin,
            floating_pane_coordinates,
            completion_tx,
            overlay_placement,
        ))?;
        Ok(())
    }

    /// The folder an action asks to start in, taken before it is spawned and
    /// consumed.
    pub(super) fn action_cwd(terminal_action: &TerminalAction) -> Option<PathBuf> {
        match terminal_action {
            TerminalAction::RunCommand(run_command) => run_command.cwd.clone(),
            TerminalAction::OpenFile(payload) => payload.cwd.clone(),
        }
    }

    /// The clients this pane is the active one for, who a cwd change concerns.
    pub(super) fn focused_clients(&self, pane_id: PaneId) -> Vec<ClientId> {
        self.active_panes
            .iter()
            .filter(|(_, active_pane)| *active_pane == &pane_id)
            .map(|(client_id, _)| *client_id)
            .collect()
    }

    /// Record where a pane is, and say so when it has moved.
    ///
    /// The single writer of `terminal_cwds`. Everything that learns a pane's
    /// folder — the initial capture, the ticker, OSC 7 — goes through here, so
    /// there is one place that decides a change has happened and one place that
    /// announces it.
    fn set_terminal_cwd(&mut self, terminal_id: TerminalId, cwd: PathBuf) {
        if self.terminal_cwds.get(&terminal_id) == Some(&cwd) {
            return;
        }
        let pane_id = PaneId::Terminal(terminal_id);
        let focused_client_ids = self.focused_clients(pane_id);
        let _ = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::CwdChanged(
                    pane_id,
                    cwd.clone(),
                    focused_client_ids.into_iter().collect(),
                ),
            )]));
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneCwd(
                terminal_id,
                Some(cwd.clone()),
            ));
        self.terminal_cwds.insert(terminal_id, cwd);
    }

    /// Drop what we knew about a pane that has gone.
    ///
    /// Terminal ids are handed out again after a pane closes, so a folder left
    /// behind here is not merely wasted space — it is the wrong answer for
    /// whichever pane inherits the id.
    /// Tell Screen which machine a pane runs on, so it can reach the plugins
    /// that decide what a pane *is* — two panes in the same folder on different
    /// machines are in different folders, and anything keying on the path alone
    /// would merge them.
    pub(super) fn report_terminal_host(&mut self, terminal_id: TerminalId) {
        let host = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.remote_pane_host(terminal_id));
        if host.is_some() {
            let _ = self
                .bus
                .senders
                .send_to_screen(ScreenInstruction::UpdatePaneHost(terminal_id, host));
        }
    }

    /// The respawn counterpart of `report_terminal_host`. A pane's host is fixed for the life
    /// of one command, but a workflow step can respawn the pane on a different machine — or on
    /// none — so unlike the first report this one has to send `None` as well, or a
    /// remote-to-local rerun leaves the pane's reported host stale. One update rather than a
    /// `forget` followed by a `report`, so a plugin watching pane hosts never sees a spurious
    /// host-less pane in between.
    pub(super) fn re_report_terminal_host(&mut self, terminal_id: TerminalId) {
        let host = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.remote_pane_host(terminal_id));
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneHost(terminal_id, host));
    }

    pub(super) fn forget_terminal_host(&mut self, terminal_id: TerminalId) {
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneHost(terminal_id, None));
    }

    pub(super) fn forget_terminal_foreground(&mut self, terminal_id: TerminalId) {
        if self
            .terminal_reported_commands
            .remove(&terminal_id)
            .is_some()
        {
            let _ =
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::UpdatePaneForegroundCommand(
                        terminal_id,
                        Vec::new(),
                        None,
                    ));
        }
    }

    pub(super) fn forget_terminal_cwd(&mut self, terminal_id: TerminalId) {
        if self.terminal_cwds.remove(&terminal_id).is_some() {
            let _ = self
                .bus
                .senders
                .send_to_screen(ScreenInstruction::UpdatePaneCwd(terminal_id, None));
        }
    }

    /// The folder a pane starts in.
    ///
    /// This used to be recorded silently, and the ticker below only reports a
    /// cwd that *differs* from the recorded one — so a pane whose process never
    /// left the folder it started in announced its folder to nobody, ever. It
    /// was a race, not a certainty: when the child had not forked yet there was
    /// nothing to record, and the ticker then reported it after all. That is why
    /// the requested cwd is a fallback here rather than an afterthought — the
    /// caller has already resolved it, and it is better than nothing to go on.
    pub(super) fn capture_initial_cwd(
        &mut self,
        terminal_id: TerminalId,
        child_pid: ProcessId,
        requested_cwd: Option<PathBuf>,
    ) {
        let cwd = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.get_cwd(child_pid))
            .or(requested_cwd);
        if let Some(cwd) = cwd {
            self.set_terminal_cwd(terminal_id, cwd);
        }
    }

    pub(crate) fn update_and_report_cwds(&mut self) {
        use std::sync::atomic::Ordering;

        // Local panes are gated on having produced output, because inspecting
        // them costs a `sysinfo` refresh per pane. Remote panes are not: their
        // information is pushed up by the broker on its own ticker and is
        // already sitting in a local cache, so re-reading it costs nothing —
        // which also means a silent remote agent, one sitting at its prompt
        // producing nothing, stays visible where a local one would go stale.
        let active_local: Vec<TerminalId> = self
            .id_to_child_pid
            .keys()
            .copied()
            .filter(|id| {
                self.pane_activity_flags
                    .get(id)
                    .map(|f| f.swap(false, Ordering::Relaxed))
                    .unwrap_or(false)
            })
            .collect();
        let remote_ids: Vec<TerminalId> = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.remote_pane_ids())
            .unwrap_or_default();

        if active_local.is_empty() && remote_ids.is_empty() {
            return;
        }

        let pids: Vec<_> = active_local
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id))
            .copied()
            .collect();

        let (pids_to_cwds, pids_to_cmds) = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_cwds(pids))
            .unwrap_or_default();

        for terminal_id in &active_local {
            let process_id = self.id_to_child_pid.get(terminal_id).copied();
            let cwd = process_id.and_then(|pid| pids_to_cwds.get(&pid).cloned());
            let cmd = process_id.and_then(|pid| pids_to_cmds.get(&pid).cloned());

            if let Some(cwd) = cwd {
                self.set_terminal_cwd(*terminal_id, cwd);
            }
            if let Some(cmd) = cmd {
                let cmd = self.pane_argv(*terminal_id, cmd);
                self.terminal_cmds.insert(*terminal_id, cmd);
            }
        }

        let panes: Vec<(TerminalId, ProcessId)> = active_local
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id).map(|pid| (*id, *pid)))
            .collect();
        let mut foreground_cmds = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_foreground_cmds(&panes, &self.post_command_discovery_hook))
            .unwrap_or_default();

        // The same three facts for a remote pane, from the broker's report
        // rather than from this machine's process table. Everything downstream
        // is identical, which is what lets agent detection — pure string
        // matching over argv — work on a remote pane with no changes at all.
        for terminal_id in &remote_ids {
            let status = match self
                .bus
                .os_input
                .as_ref()
                .and_then(|os_input| os_input.remote_pane_status(*terminal_id))
            {
                Some(status) => status,
                None => continue,
            };
            if let Some(cwd) = status.cwd {
                self.set_terminal_cwd(*terminal_id, PathBuf::from(cwd));
            }
            if let Some(shell_argv) = status.shell_argv {
                self.terminal_cmds.insert(*terminal_id, shell_argv);
            }
            if let Some(foreground) = status.foreground_argv {
                foreground_cmds.insert(*terminal_id, foreground);
            }
        }

        let active_terminal_ids: Vec<TerminalId> = active_local
            .iter()
            .copied()
            .chain(remote_ids.iter().copied())
            .collect();

        for terminal_id in &active_terminal_ids {
            let foreground_cmd: Vec<String> = foreground_cmds
                .get(terminal_id)
                .cloned()
                .unwrap_or_default();

            let (command, is_foreground) = if foreground_cmd.is_empty() {
                let shell_cmd = self
                    .terminal_cmds
                    .get(terminal_id)
                    .cloned()
                    .unwrap_or_default();
                (shell_cmd, false)
            } else {
                (foreground_cmd.clone(), true)
            };
            let reported = (foreground_cmd.clone(), command.clone());
            if self.terminal_reported_commands.get(terminal_id) != Some(&reported) {
                let pane_id = PaneId::Terminal(*terminal_id);
                let focused_client_ids: Vec<ClientId> = self
                    .active_panes
                    .iter()
                    .filter(|(_, active_pane)| *active_pane == &pane_id)
                    .map(|(client_id, _)| *client_id)
                    .collect();
                let _ = self
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        None,
                        None,
                        Event::CommandChanged(
                            pane_id,
                            command.clone(),
                            is_foreground,
                            focused_client_ids.into_iter().collect(),
                        ),
                    )]));
                let _ = self.bus.senders.send_to_screen(
                    ScreenInstruction::UpdatePaneForegroundCommand(
                        *terminal_id,
                        foreground_cmd,
                        command.first().cloned(),
                    ),
                );
                self.terminal_reported_commands
                    .insert(*terminal_id, reported);
            }
        }
    }

    pub(crate) fn notify_cwd_from_osc7(&mut self, terminal_id: TerminalId, path: PathBuf) {
        use std::sync::atomic::Ordering;

        self.set_terminal_cwd(terminal_id, path);
        if let Some(flag) = self.pane_activity_flags.get(&terminal_id) {
            flag.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn get_pane_pid(&self, pane_id: PaneId) -> GetPanePidResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // A remote pane's pid is meaningful only on its own machine.
                // Handing it to a plugin invites exactly the mistake this whole
                // boundary exists to prevent — the plugin would `kill` or `stat`
                // whatever local process happens to hold that number.
                if self
                    .bus
                    .os_input
                    .as_ref()
                    .is_some_and(|os_input| os_input.is_remote_pane(terminal_id))
                {
                    return GetPanePidResponse::Err(format!(
                        "Terminal pane {terminal_id} runs on another machine, so it has no pid \
                         meaningful on this one"
                    ));
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    GetPanePidResponse::Ok(child_pid)
                } else {
                    GetPanePidResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => {
                GetPanePidResponse::Err(format!("Cannot get PID for plugin pane {}", plugin_id))
            },
        }
    }

    pub(crate) fn get_pane_running_command(
        &self,
        pane_id: PaneId,
    ) -> GetPaneRunningCommandResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // `agent-service` calls this directly to repair panes the
                // activity-flag poll missed, so a remote pane has to answer it
                // too — otherwise a remote agent would only ever be classified
                // from its title.
                if let Some(status) = self
                    .bus
                    .os_input
                    .as_ref()
                    .and_then(|os_input| os_input.remote_pane_status(terminal_id))
                {
                    return match status.foreground_argv.or(status.shell_argv) {
                        Some(argv) => GetPaneRunningCommandResponse::Ok(argv),
                        None => GetPaneRunningCommandResponse::Err(format!(
                            "the machine hosting terminal pane {terminal_id} has not reported \
                             its running command yet"
                        )),
                    };
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    // Query OS for current running command
                    if let Some(os_input) = self.bus.os_input.as_ref() {
                        // First, try to get the foreground command (e.g., nvim running in bash)
                        let foreground_cmds = os_input.get_foreground_cmds(
                            &[(terminal_id, child_pid)],
                            &self.post_command_discovery_hook,
                        );
                        let cmd_foreground = foreground_cmds.get(&terminal_id);

                        // If no foreground command, fall back to the shell itself
                        let (_cwds, cmds) = os_input.get_cwds(vec![child_pid]);
                        let cmd_sysinfo = cmds.get(&child_pid);

                        if let Some(command_args) =
                            self.local_running_command(terminal_id, cmd_foreground, cmd_sysinfo)
                        {
                            GetPaneRunningCommandResponse::Ok(command_args)
                        } else {
                            GetPaneRunningCommandResponse::Err(format!(
                                "Could not retrieve running command for terminal pane {}",
                                terminal_id
                            ))
                        }
                    } else {
                        GetPaneRunningCommandResponse::Err("OS input not available".to_string())
                    }
                } else {
                    GetPaneRunningCommandResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => GetPaneRunningCommandResponse::Err(format!(
                "Cannot get running command for plugin pane {}",
                plugin_id
            )),
        }
    }

    pub(crate) fn get_pane_cwd(&self, pane_id: PaneId) -> GetPaneCwdResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // A remote pane's cwd is a path on *its* machine. Reported as-is:
                // rewriting it would be a lie, and the plugins that consume it
                // pair it with the host when deciding which project it belongs
                // to.
                if let Some(status) = self
                    .bus
                    .os_input
                    .as_ref()
                    .and_then(|os_input| os_input.remote_pane_status(terminal_id))
                {
                    return match status.cwd {
                        Some(cwd) => GetPaneCwdResponse::Ok(PathBuf::from(cwd)),
                        None => GetPaneCwdResponse::Err(format!(
                            "the machine hosting terminal pane {terminal_id} has not reported \
                             its working directory yet"
                        )),
                    };
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    // Query OS for current working directory
                    if let Some(os_input) = self.bus.os_input.as_ref() {
                        let (cwds, _cmds) = os_input.get_cwds(vec![child_pid]);
                        if let Some(cwd) = cwds.get(&child_pid) {
                            GetPaneCwdResponse::Ok(cwd.clone())
                        } else {
                            GetPaneCwdResponse::Err(format!(
                                "Could not retrieve CWD for terminal pane {}",
                                terminal_id
                            ))
                        }
                    } else {
                        GetPaneCwdResponse::Err("OS input not available".to_string())
                    }
                } else {
                    GetPaneCwdResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => {
                GetPaneCwdResponse::Err(format!("Cannot get CWD for plugin pane {}", plugin_id))
            },
        }
    }

    /// A local pane's running command: what is in the foreground, else the
    /// pane's own process.
    fn local_running_command(
        &self,
        terminal_id: TerminalId,
        foreground: Option<&Vec<String>>,
        own: Option<&Vec<String>>,
    ) -> Option<Vec<String>> {
        foreground
            .cloned()
            .or_else(|| own.map(|argv| self.pane_argv(terminal_id, argv.clone())))
    }

    /// [`Self::get_pane_running_command`] and [`Self::get_pane_cwd`] for many
    /// panes, with one process-table refresh for all the local ones instead of
    /// three per pane.
    pub(crate) fn get_panes_metadata(&self, pane_ids: &[PaneId]) -> GetPanesMetadataResponse {
        let mut metadata = GetPanesMetadataResponse::new();
        let Some(os_input) = self.bus.os_input.as_ref() else {
            return metadata;
        };
        let mut local: Vec<(TerminalId, ProcessId)> = Vec::new();
        for pane_id in pane_ids {
            let PaneId::Terminal(terminal_id) = *pane_id else {
                continue;
            };
            if let Some(status) = os_input.remote_pane_status(terminal_id) {
                // A remote pane that has reported nothing yet is absent, as a local one is.
                let running_command = status.foreground_argv.or(status.shell_argv);
                let cwd = status.cwd.map(PathBuf::from);
                if running_command.is_some() || cwd.is_some() {
                    metadata.insert(
                        *pane_id,
                        PaneProcessMetadata {
                            running_command,
                            cwd,
                        },
                    );
                }
            } else if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                local.push((terminal_id, child_pid));
            }
        }
        if local.is_empty() {
            return metadata;
        }

        let foreground_cmds =
            os_input.get_foreground_cmds(&local, &self.post_command_discovery_hook);
        let (cwds, cmds) = os_input.get_cwds(local.iter().map(|(_, pid)| *pid).collect());
        for (terminal_id, child_pid) in local {
            let running_command = self.local_running_command(
                terminal_id,
                foreground_cmds.get(&terminal_id),
                cmds.get(&child_pid),
            );
            let cwd = cwds.get(&child_pid).cloned();
            if running_command.is_some() || cwd.is_some() {
                metadata.insert(
                    PaneId::Terminal(terminal_id),
                    PaneProcessMetadata {
                        running_command,
                        cwd,
                    },
                );
            }
        }
        metadata
    }

    /// What Git said about a remote pane's folder, as the far machine sees it.
    ///
    /// Answered from the cache the broker pushes up, never by running Git here:
    /// the folder is a path on another machine, and this is the pty thread,
    /// which every pane in the session is waiting on. A local pane is an `Err`
    /// for the same reason — there is nothing cached to give, and spawning two
    /// `git` processes to make one would block that thread. Plugins branch on
    /// `PaneInfo::host` and run Git themselves for a local pane.
    pub(crate) fn get_pane_git_probe(
        &self,
        pane_id: PaneId,
        cwd: &str,
        refresh_token: Option<&str>,
    ) -> GetPaneGitProbeResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                match self
                    .bus
                    .os_input
                    .as_ref()
                    .ok_or_else(|| "PTY backend unavailable".to_owned())
                    .and_then(|os_input| {
                        os_input.remote_pane_git_probe(terminal_id, cwd, refresh_token)
                    }) {
                    Ok(Some(probe)) => GetPaneGitProbeResponse::Ok(PaneGitProbe {
                        host: probe.host,
                        cwd: probe.cwd,
                        exit_code: probe.exit_code,
                        metadata: probe.metadata,
                        config: probe.config,
                    }),
                    Ok(None) => GetPaneGitProbeResponse::Pending,
                    Err(err) => GetPaneGitProbeResponse::Err(err),
                }
            },
            PaneId::Plugin(plugin_id) => GetPaneGitProbeResponse::Err(format!(
                "cannot get a git probe for plugin pane {plugin_id}"
            )),
        }
    }
}
