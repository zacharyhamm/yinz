#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const TERMINAL_3: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(3);
#[cfg(test)]
const TERMINAL_41: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(41);

use super::*;
use crate::os_input_output::ServerOsApi;
use crate::plugins::PluginInstruction;
use crate::pty_host::remote::conn::RemoteGitProbe;
use crate::pty_host::remote::RemoteStatus;
use crate::thread_bus::Bus;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use yinz_runtime::channels::{self, SenderWithContext, Shutdown};
use yinz_types::agent::report::AgentReportState;
use yinz_types::agent::{AgentSource, AgentState, HOOK_BROADCAST_MAX_BYTES};
use yinz_types::command::PaneTarget;
use yinz_types::data::Event;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::style::Palette;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::ServerToClientMsg,
};

/// The plugin thread's end of the bus a test pty sends on.
type PluginReceiver = channels::Receiver<(PluginInstruction, ErrorContext)>;
/// The screen thread's end of the bus a test pty sends on.
type ScreenReceiver = channels::Receiver<(ScreenInstruction, ErrorContext)>;

#[derive(Clone)]
struct MockOsApi {
    cwds: Arc<Mutex<HashMap<ProcessId, PathBuf>>>,
    cmds: Arc<Mutex<HashMap<ProcessId, Vec<String>>>>,
    cmds_by_ppid: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Every pid-addressed signal, recorded so a test can assert one was never
    /// sent. This is the whole point for remote panes: the failure being guarded
    /// against is a signal that *is* delivered, to the wrong process.
    local_signals: Arc<Mutex<Vec<(ProcessId, &'static str)>>>,
    remote_panes: Arc<Mutex<std::collections::HashSet<TerminalId>>>,
    remote_signals: Arc<Mutex<Vec<(TerminalId, ChildSignal)>>>,
    remote_status: Arc<Mutex<HashMap<TerminalId, RemoteStatus>>>,
    remote_hosts: Arc<Mutex<HashMap<TerminalId, String>>>,
    /// What each remote pane's machine last said about its folder, as the
    /// broker's `GitProbe` arrives.
    remote_git_probes: Arc<Mutex<HashMap<TerminalId, RemoteGitProbe>>>,
    /// Panes this backend will refuse to warp, and why.
    ///
    /// Absent means yes. A real backend answers out of the maps the release
    /// would use; this one answers out of what a test put here.
    warp_refusals: Arc<Mutex<HashMap<TerminalId, String>>>,
    /// Panes whose release fails *after* the check said yes — the shape of a
    /// remote broker that will not confirm a handoff, and the only case where a
    /// pane really is cut loose.
    unreleasable: Arc<Mutex<std::collections::HashSet<TerminalId>>>,
    /// Every id the release was actually attempted for, so a test can assert it
    /// was never attempted at all.
    release_attempts: Arc<Mutex<Vec<TerminalId>>>,
    /// The budget each release was handed, so a test can assert the caller's
    /// deadline reached this far down rather than stopping at a constant.
    release_budgets: Arc<Mutex<Vec<std::time::Duration>>>,
    /// How long each release blocks for. Stands in for a remote pane's round
    /// trip to its broker, which is the only reason releasing a tab's panes one
    /// at a time was ever expensive.
    release_delay: Arc<Mutex<std::time::Duration>>,
    /// Every command handed to `re_run_command_in_terminal`, so a test can see what a respawn
    /// actually asked the OS to run - the `originating_plugin` it carried included.
    rerun_commands: Arc<Mutex<Vec<RunCommand>>>,
    rerun_failures: Arc<Mutex<std::collections::HashSet<TerminalId>>>,
    clear_failures: Arc<Mutex<std::collections::HashSet<TerminalId>>>,
    /// Every id handed to `clear_terminal_id`, the teardown funnel, so a test can assert a
    /// pane that was closed out from under a worker still went through it.
    cleared_terminal_ids: Arc<Mutex<Vec<TerminalId>>>,
    spawn_started: Arc<Barrier>,
    spawn_release: Arc<Barrier>,
    transport_started: Arc<Barrier>,
    transport_release: Arc<Barrier>,
    next_terminal_id: Arc<AtomicU32>,
    resolved_spawns: Arc<Mutex<Vec<TerminalId>>>,
    fail_local_spawns: Arc<Mutex<bool>>,
    /// How many local spawns still succeed before they start failing the way
    /// `fail_local_spawns` makes them fail, so a test can spawn a pane for real
    /// and then break the one after it.
    local_spawns_to_allow: Arc<Mutex<usize>>,
    adopted_command: Arc<Mutex<Option<RunCommand>>>,
}

impl MockOsApi {
    fn new() -> Self {
        MockOsApi {
            cwds: Arc::new(Mutex::new(HashMap::new())),
            cmds: Arc::new(Mutex::new(HashMap::new())),
            cmds_by_ppid: Arc::new(Mutex::new(HashMap::new())),
            local_signals: Arc::new(Mutex::new(Vec::new())),
            remote_panes: Arc::new(Mutex::new(std::collections::HashSet::new())),
            remote_signals: Arc::new(Mutex::new(Vec::new())),
            remote_status: Arc::new(Mutex::new(HashMap::new())),
            remote_hosts: Arc::new(Mutex::new(HashMap::new())),
            remote_git_probes: Arc::new(Mutex::new(HashMap::new())),
            warp_refusals: Arc::new(Mutex::new(HashMap::new())),
            unreleasable: Arc::new(Mutex::new(std::collections::HashSet::new())),
            release_attempts: Arc::new(Mutex::new(Vec::new())),
            release_budgets: Arc::new(Mutex::new(Vec::new())),
            release_delay: Arc::new(Mutex::new(std::time::Duration::ZERO)),
            rerun_commands: Arc::new(Mutex::new(Vec::new())),
            rerun_failures: Arc::new(Mutex::new(std::collections::HashSet::new())),
            clear_failures: Arc::new(Mutex::new(std::collections::HashSet::new())),
            cleared_terminal_ids: Arc::new(Mutex::new(Vec::new())),
            spawn_started: Arc::new(Barrier::new(1)),
            spawn_release: Arc::new(Barrier::new(1)),
            transport_started: Arc::new(Barrier::new(1)),
            transport_release: Arc::new(Barrier::new(1)),
            next_terminal_id: Arc::new(AtomicU32::new(0)),
            resolved_spawns: Arc::new(Mutex::new(vec![])),
            fail_local_spawns: Arc::new(Mutex::new(false)),
            local_spawns_to_allow: Arc::new(Mutex::new(0)),
            adopted_command: Arc::new(Mutex::new(None)),
        }
    }
    fn rerun_commands(&self) -> Vec<RunCommand> {
        self.rerun_commands.lock().unwrap().clone()
    }
    fn fail_rerun_of(&self, terminal_id: TerminalId) {
        self.rerun_failures.lock().unwrap().insert(terminal_id);
    }
    fn fail_clear_of(&self, terminal_id: TerminalId) {
        self.clear_failures.lock().unwrap().insert(terminal_id);
    }
    fn refuse_to_warp(&self, terminal_id: TerminalId, why: &str) {
        self.warp_refusals
            .lock()
            .unwrap()
            .insert(terminal_id, why.to_owned());
    }
    fn fail_the_release_of(&self, terminal_id: TerminalId) {
        self.unreleasable.lock().unwrap().insert(terminal_id);
    }
    fn release_attempts(&self) -> Vec<TerminalId> {
        self.release_attempts.lock().unwrap().clone()
    }
    fn release_budgets(&self) -> Vec<std::time::Duration> {
        self.release_budgets.lock().unwrap().clone()
    }
    /// Make every release take this long, the way a round trip to a broker does.
    fn make_each_release_take(&self, how_long: std::time::Duration) {
        *self.release_delay.lock().unwrap() = how_long;
    }
    fn set_remote_host(&self, terminal_id: TerminalId, host: &str) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_hosts
            .lock()
            .unwrap()
            .insert(terminal_id, host.to_owned());
    }
    fn set_remote_git_probe(&self, terminal_id: TerminalId, probe: RemoteGitProbe) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_git_probes
            .lock()
            .unwrap()
            .insert(terminal_id, probe);
    }
    fn set_remote_status(&self, terminal_id: TerminalId, status: RemoteStatus) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_status
            .lock()
            .unwrap()
            .insert(terminal_id, status);
    }
    fn mark_remote(&self, terminal_id: TerminalId) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
    }
    fn local_signals(&self) -> Vec<(ProcessId, &'static str)> {
        self.local_signals.lock().unwrap().clone()
    }
    fn remote_signals(&self) -> Vec<(TerminalId, ChildSignal)> {
        self.remote_signals.lock().unwrap().clone()
    }
    fn set_cwd(&self, pid: ProcessId, path: PathBuf) {
        self.cwds.lock().unwrap().insert(pid, path);
    }
    fn set_cmd(&self, pid: ProcessId, cmd: Vec<String>) {
        self.cmds.lock().unwrap().insert(pid, cmd);
    }
    fn set_foreground_cmd(&self, ppid: ProcessId, cmd: Vec<String>) {
        self.cmds_by_ppid
            .lock()
            .unwrap()
            .insert(ppid.to_string(), cmd);
    }
    fn clear_foreground_cmd(&self, ppid: ProcessId) {
        self.cmds_by_ppid.lock().unwrap().remove(&ppid.to_string());
    }
}

impl ServerOsApi for MockOsApi {
    fn set_terminal_size_using_terminal_id(
        &self,
        _: TerminalId,
        _: u16,
        _: u16,
        _: Option<u16>,
        _: Option<u16>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn spawn_terminal(
        &self,
        _: TerminalAction,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        let mut allowed = self.local_spawns_to_allow.lock().unwrap();
        if *allowed > 0 {
            *allowed -= 1;
            let terminal_id = self.next_terminal_id.fetch_add(1, Ordering::SeqCst);
            // The child pid is the terminal id offset into a range of its own, so
            // a test asserting on signals cannot confuse the two.
            return Ok((
                crate::TerminalId::new(terminal_id),
                Box::new(crate::os_input_output::NullAsyncReader),
                Some(ProcessId::new(1000 + terminal_id)),
            ));
        }
        if *self.fail_local_spawns.lock().unwrap() {
            return Err(anyhow!("injected local spawn failure"));
        }
        unimplemented!()
    }
    fn resolve_spawn(
        &self,
        terminal_id: TerminalId,
        terminal_action: TerminalAction,
        _default_editor: Option<PathBuf>,
        _report_progress: &mut dyn FnMut(&str),
    ) -> anyhow::Result<crate::os_input_output::ResolvedSpawn> {
        self.spawn_started.wait();
        self.spawn_release.wait();
        let TerminalAction::RunCommand(command) = terminal_action else {
            unreachable!()
        };
        let rerun_command = command.clone();
        Ok(crate::os_input_output::ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            endpoint: Some(crate::os_input_output::RemoteEndpoint {
                host: "resolved-host".to_owned(),
                ssh_options: Default::default(),
            }),
        })
    }
    fn spawn_resolved(
        &self,
        resolved: crate::os_input_output::ResolvedSpawn,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        self.transport_started.wait();
        self.transport_release.wait();
        self.resolved_spawns
            .lock()
            .unwrap()
            .push(resolved.terminal_id);
        Ok((
            resolved.terminal_id,
            Box::new(crate::os_input_output::NullAsyncReader),
            None,
        ))
    }
    fn resolve_rerun(
        &self,
        terminal_id: TerminalId,
        command: RunCommand,
        _report_progress: &mut dyn FnMut(&str),
    ) -> anyhow::Result<crate::os_input_output::ResolvedSpawn> {
        self.spawn_started.wait();
        self.spawn_release.wait();
        if self.rerun_failures.lock().unwrap().contains(&terminal_id) {
            return Err(anyhow!("injected rerun failure"));
        }
        let rerun_command = command.clone();
        Ok(crate::os_input_output::ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            endpoint: Some(crate::os_input_output::RemoteEndpoint {
                host: "resolved-host".to_owned(),
                ssh_options: Default::default(),
            }),
        })
    }
    fn reserve_terminal_id(&self) -> std::result::Result<TerminalId, anyhow::Error> {
        Ok(crate::TerminalId::new(
            self.next_terminal_id.fetch_add(1, Ordering::SeqCst),
        ))
    }
    fn rerun_resolved(
        &self,
        resolved: crate::os_input_output::ResolvedSpawn,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        self.transport_started.wait();
        self.transport_release.wait();
        self.resolved_spawns
            .lock()
            .unwrap()
            .push(resolved.terminal_id);
        Ok((Box::new(crate::os_input_output::NullAsyncReader), None))
    }
    fn write_to_tty_stdin(&self, _: TerminalId, buf: &[u8]) -> anyhow::Result<usize> {
        Ok(buf.len())
    }
    fn tcdrain(&self, _: TerminalId) -> anyhow::Result<()> {
        Ok(())
    }
    fn kill(&self, pid: ProcessId) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGHUP"));
        Ok(())
    }
    fn force_kill(&self, pid: ProcessId) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGKILL"));
        Ok(())
    }
    fn send_sigint(&self, pid: ProcessId) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGINT"));
        Ok(())
    }
    fn send_sigcont(&self, pid: ProcessId) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGCONT"));
        Ok(())
    }
    fn is_remote_pane(&self, terminal_id: TerminalId) -> bool {
        self.remote_panes.lock().unwrap().contains(&terminal_id)
    }
    fn signal_remote_child(
        &self,
        terminal_id: TerminalId,
        signal: ChildSignal,
    ) -> anyhow::Result<()> {
        self.remote_signals
            .lock()
            .unwrap()
            .push((terminal_id, signal));
        Ok(())
    }
    fn remote_pane_ids(&self) -> Vec<TerminalId> {
        self.remote_panes.lock().unwrap().iter().copied().collect()
    }
    fn remote_pane_status(&self, terminal_id: TerminalId) -> Option<RemoteStatus> {
        self.remote_status
            .lock()
            .unwrap()
            .get(&terminal_id)
            .cloned()
    }
    fn remote_pane_host(&self, terminal_id: TerminalId) -> Option<String> {
        self.remote_hosts.lock().unwrap().get(&terminal_id).cloned()
    }
    fn remote_pane_git_probe(
        &self,
        terminal_id: TerminalId,
        cwd: &str,
        _refresh_token: Option<&str>,
    ) -> Result<Option<RemoteGitProbe>, String> {
        self.remote_git_probes
            .lock()
            .unwrap()
            .get(&terminal_id)
            .cloned()
            .map(|probe| Some(probe).filter(|probe| probe.cwd == cwd))
            .ok_or_else(|| "pane is not remote".into())
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn send_to_client(&self, _: ClientId, _: ServerToClientMsg) -> anyhow::Result<()> {
        Ok(())
    }
    fn new_client(
        &mut self,
        _: ClientId,
        _: LocalSocketStream,
    ) -> anyhow::Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _: ClientId) -> anyhow::Result<()> {
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        Palette::default()
    }
    fn get_cwd(&self, pid: ProcessId) -> Option<PathBuf> {
        self.cwds.lock().unwrap().get(&pid).cloned()
    }
    fn get_cwds(
        &self,
        pids: Vec<ProcessId>,
    ) -> (HashMap<ProcessId, PathBuf>, HashMap<ProcessId, Vec<String>>) {
        let cwds_lock = self.cwds.lock().unwrap();
        let cmds_lock = self.cmds.lock().unwrap();
        let cwds = pids
            .iter()
            .filter_map(|pid| cwds_lock.get(pid).map(|cwd| (*pid, cwd.clone())))
            .collect();
        let cmds = pids
            .iter()
            .filter_map(|pid| cmds_lock.get(pid).map(|cmd| (*pid, cmd.clone())))
            .collect();
        (cwds, cmds)
    }
    fn get_all_cmds_by_ppid(&self, _: &Option<String>) -> HashMap<String, Vec<String>> {
        self.cmds_by_ppid.lock().unwrap().clone()
    }
    fn get_foreground_cmds(
        &self,
        panes: &[(TerminalId, ProcessId)],
        _: &Option<String>,
    ) -> HashMap<TerminalId, Vec<String>> {
        let cmds_by_ppid = self.cmds_by_ppid.lock().unwrap();
        panes
            .iter()
            .filter_map(|(terminal_id, shell_pid)| {
                cmds_by_ppid
                    .get(&shell_pid.to_string())
                    .map(|cmd| (*terminal_id, cmd.clone()))
            })
            .collect()
    }
    fn write_to_file(&mut self, _: String, _: Option<String>) -> anyhow::Result<()> {
        Ok(())
    }
    fn re_run_command_in_terminal(
        &self,
        terminal_id: TerminalId,
        run_command: RunCommand,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        let next_host = run_command.target.display();
        self.rerun_commands.lock().unwrap().push(run_command);
        if self.rerun_failures.lock().unwrap().contains(&terminal_id) {
            self.remote_panes.lock().unwrap().remove(&terminal_id);
            self.remote_hosts.lock().unwrap().remove(&terminal_id);
            return Err(anyhow::anyhow!("replacement failed"));
        }
        match next_host {
            Some(host) => {
                self.remote_panes.lock().unwrap().insert(terminal_id);
                self.remote_hosts.lock().unwrap().insert(terminal_id, host);
            },
            None => {
                self.remote_panes.lock().unwrap().remove(&terminal_id);
                self.remote_hosts.lock().unwrap().remove(&terminal_id);
            },
        }
        Ok((Box::new(crate::os_input_output::NullAsyncReader), None))
    }
    fn adopt_terminal(
        &self,
        _: &std::path::Path,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<crate::os_input_output::AdoptedTerminal> {
        Ok(crate::os_input_output::AdoptedTerminal {
            terminal_id: TerminalId::new(7),
            reader: Box::new(crate::os_input_output::NullAsyncReader),
            child_pid: Some(ProcessId::new(4242)),
            cmd: self.adopted_command.lock().unwrap().take().unwrap(),
            buffered: vec![],
            child_exit_status: None,
        })
    }

    fn clear_terminal_id(&self, terminal_id: TerminalId) -> anyhow::Result<()> {
        self.cleared_terminal_ids.lock().unwrap().push(terminal_id);
        (!self.clear_failures.lock().unwrap().contains(&terminal_id))
            .then_some(())
            .context("injected clear failure")
    }
    fn check_terminal_can_be_warped(&self, terminal_id: TerminalId) -> anyhow::Result<()> {
        match self.warp_refusals.lock().unwrap().get(&terminal_id) {
            Some(why) => Err(anyhow::anyhow!("{why}")),
            None => Ok(()),
        }
    }
    fn release_terminal_for_warp(
        &self,
        terminal_id: TerminalId,
        budget: std::time::Duration,
    ) -> anyhow::Result<ReleasedPane> {
        self.release_attempts.lock().unwrap().push(terminal_id);
        self.release_budgets.lock().unwrap().push(budget);
        let delay = *self.release_delay.lock().unwrap();
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if self.unreleasable.lock().unwrap().contains(&terminal_id) {
            return Err(anyhow::anyhow!(
                "the supervisor did not confirm the handoff"
            ));
        }
        Ok(ReleasedPane::Local(
            crate::pty_host::client::ReleasedPtyHost {
                host_id: SupervisorId::new(format!("host-{terminal_id}")),
                host_pid: ProcessId::new(1),
                control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
            },
        ))
    }
}

fn make_pty_with_plugin_receiver(mock: MockOsApi) -> (Pty, PluginReceiver) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let plugin_sender = SenderWithContext::new(plugin_tx);
    let mut bus: Bus<PtyInstruction> = Bus::empty().should_silently_fail();
    bus.os_input = Some(Box::new(mock));
    bus.senders.to_plugin = Some(plugin_sender);
    let pty = Pty::new(bus, false, None, None);
    (pty, plugin_rx)
}

fn make_pty_with_screen_receiver(mock: MockOsApi) -> (Pty, PluginReceiver, ScreenReceiver) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let (screen_tx, screen_rx) = channels::unbounded();
    let mut bus: Bus<PtyInstruction> = Bus::empty().should_silently_fail();
    bus.os_input = Some(Box::new(mock));
    bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    bus.senders.to_screen = Some(SenderWithContext::new(screen_tx));
    let pty = Pty::new(bus, false, None, None);
    (pty, plugin_rx, screen_rx)
}

fn collect_pane_host_instructions(rx: &ScreenReceiver) -> Vec<(TerminalId, Option<String>)> {
    let mut updates = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let ScreenInstruction::UpdatePaneHost(terminal_id, host) = instruction {
            updates.push((terminal_id, host));
        }
    }
    updates
}

fn collect_pane_cwd_instructions(rx: &ScreenReceiver) -> Vec<(TerminalId, Option<PathBuf>)> {
    let mut updates = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let ScreenInstruction::UpdatePaneCwd(terminal_id, cwd) = instruction {
            updates.push((terminal_id, cwd));
        }
    }
    updates
}

fn set_active_terminal(pty: &mut Pty, terminal_id: TerminalId, child_pid: ProcessId) {
    let flag = Arc::new(AtomicBool::new(true));
    pty.id_to_child_pid.insert(terminal_id, child_pid);
    pty.pane_activity_flags.insert(terminal_id, flag);
}

fn collect_cwd_changed_events(rx: &PluginReceiver) -> Vec<(PaneId, PathBuf)> {
    let mut events = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                if let Event::CwdChanged(pane_id, cwd, _) = event {
                    events.push((pane_id, cwd));
                }
            }
        }
    }
    events
}

fn collect_command_changed_events(rx: &PluginReceiver) -> Vec<(PaneId, Vec<String>, bool)> {
    let mut events = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                if let Event::CommandChanged(pane_id, cmd, is_foreground, _) = event {
                    events.push((pane_id, cmd, is_foreground));
                }
            }
        }
    }
    events
}

#[test]
fn screen_gets_effective_program_changes_even_without_foreground_argv() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    let terminal_id = TerminalId::new(1);
    let (mut pty, plugins, screen) = make_pty_with_screen_receiver(mock.clone());
    for (shell, foreground, expected) in [
        (vec![], vec![], None),
        (vec!["/bin/zsh"], vec![], Some("/bin/zsh")),
        (vec!["-bash"], vec![], Some("-bash")),
        (
            vec!["-bash"],
            vec!["/usr/bin/vim", "main.rs"],
            Some("/usr/bin/vim"),
        ),
        (vec!["-bash"], vec![], Some("-bash")),
    ] {
        mock.set_cmd(child_pid, shell.into_iter().map(str::to_owned).collect());
        let foreground: Vec<String> = foreground.into_iter().map(str::to_owned).collect();
        mock.set_foreground_cmd(child_pid, foreground.clone());
        set_active_terminal(&mut pty, terminal_id, child_pid);
        pty.update_and_report_cwds();
        let updates: Vec<_> = screen
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ScreenInstruction::UpdatePaneForegroundCommand(id, argv, program) => {
                    Some((id, argv, program))
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            updates,
            vec![(terminal_id, foreground, expected.map(str::to_owned))]
        );
        assert_eq!(collect_command_changed_events(&plugins).len(), 1);
    }
    pty.forget_terminal_foreground(terminal_id);
    assert!(matches!(screen.recv().unwrap().0,
        ScreenInstruction::UpdatePaneForegroundCommand(_, argv, None) if argv.is_empty()));
}

#[test]
fn foreground_command_emitted_with_is_foreground_true() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_foreground_cmd(child_pid, vec!["vim".into(), "file.rs".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, vec!["vim", "file.rs"]);
    assert!(events[0].2, "expected is_foreground=true");
}

#[test]
fn empty_foreground_falls_back_to_shell_command() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cmd(child_pid, vec!["/bin/bash".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, vec!["/bin/bash"]);
    assert!(!events[0].2, "expected is_foreground=false");
}

#[test]
fn foreground_clearing_emits_shell_fallback() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cmd(child_pid, vec!["/bin/zsh".into()]);
    mock.set_foreground_cmd(child_pid, vec!["cargo".into(), "build".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock.clone());
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert!(events[0].2, "first event should be foreground");
    assert_eq!(events[0].1, vec!["cargo", "build"]);

    mock.clear_foreground_cmd(child_pid);
    pty.pane_activity_flags
        .get(&TerminalId::new(1))
        .unwrap()
        .store(true, Ordering::Relaxed);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, vec!["/bin/zsh"]);
    assert!(
        !events[0].2,
        "after clearing foreground, should fall back to shell"
    );
}

#[test]
fn no_event_when_foreground_unchanged() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_foreground_cmd(child_pid, vec!["htop".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();
    let _ = collect_command_changed_events(&rx);

    pty.pane_activity_flags
        .get(&TerminalId::new(1))
        .unwrap()
        .store(true, Ordering::Relaxed);
    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert!(
        events.is_empty(),
        "no event expected when command unchanged"
    );
}

#[test]
fn no_event_for_inactive_terminal() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);
    pty.pane_activity_flags
        .get(&TerminalId::new(1))
        .unwrap()
        .store(false, Ordering::Relaxed);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert!(
        events.is_empty(),
        "inactive terminal should produce no events"
    );
}

#[test]
fn foreground_change_between_two_commands() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock.clone());
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events[0].1, vec!["vim"]);
    assert!(events[0].2);

    mock.set_foreground_cmd(child_pid, vec!["cargo".into(), "test".into()]);
    pty.pane_activity_flags
        .get(&TerminalId::new(1))
        .unwrap()
        .store(true, Ordering::Relaxed);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, vec!["cargo", "test"]);
    assert!(events[0].2);
}

// --- Activity flag gating ---

#[test]
fn activity_flag_reset_after_poll() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    let (mut pty, _rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);
    assert!(pty
        .pane_activity_flags
        .get(&TerminalId::new(1))
        .unwrap()
        .load(Ordering::Relaxed));

    pty.update_and_report_cwds();

    assert!(
        !pty.pane_activity_flags
            .get(&TerminalId::new(1))
            .unwrap()
            .load(Ordering::Relaxed),
        "activity flag should be reset to false after poll"
    );
}

#[test]
fn multiple_terminals_only_active_ones_polled() {
    let mock = MockOsApi::new();
    let pid_active = ProcessId::new(100);
    let pid_inactive = ProcessId::new(200);
    mock.set_cwd(pid_active, PathBuf::from("/active"));
    mock.set_cwd(pid_inactive, PathBuf::from("/inactive"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), pid_active);
    set_active_terminal(&mut pty, crate::TerminalId::new(2), pid_inactive);
    pty.pane_activity_flags
        .get(&TerminalId::new(2))
        .unwrap()
        .store(false, Ordering::Relaxed);

    pty.update_and_report_cwds();

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, PathBuf::from("/active"));
}

// --- CWD change events ---

#[test]
fn cwd_changed_event_emitted_on_change() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/home/user"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.update_and_report_cwds();

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, PathBuf::from("/home/user"));
}

#[test]
fn no_cwd_event_when_unchanged() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/home/user"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);
    pty.terminal_cwds
        .insert(crate::TerminalId::new(1), PathBuf::from("/home/user"));

    pty.update_and_report_cwds();

    let events = collect_cwd_changed_events(&rx);
    assert!(events.is_empty(), "no event expected when cwd unchanged");
}

// --- initial CWD capture ---

/// The regression. The initial folder used to be recorded silently, and the
/// ticker above only reports a folder that *differs* from the recorded one — so
/// a pane whose process never left the folder it started in told nobody where it
/// was, ever.
#[test]
fn a_pane_that_never_leaves_its_launch_folder_still_reports_it() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/work/agent"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);

    pty.capture_initial_cwd(crate::TerminalId::new(1), child_pid, None);

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, PathBuf::from("/work/agent"));
}

/// Why it was a race rather than a certainty: with the child not yet forked far
/// enough to be asked, nothing was recorded and the ticker reported it after
/// all. The caller has already resolved the folder it asked for, so use it.
#[test]
fn an_initial_cwd_the_os_cannot_answer_falls_back_to_the_requested_one() {
    let (mut pty, rx) = make_pty_with_plugin_receiver(MockOsApi::new());

    pty.capture_initial_cwd(
        crate::TerminalId::new(1),
        ProcessId::new(100),
        Some(PathBuf::from("/work/agent")),
    );

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, PathBuf::from("/work/agent"));
    assert_eq!(
        pty.terminal_cwds.get(&TerminalId::new(1)),
        Some(&PathBuf::from("/work/agent"))
    );
}

#[test]
fn an_initial_cwd_with_nothing_to_go_on_reports_nothing() {
    let (mut pty, rx) = make_pty_with_plugin_receiver(MockOsApi::new());

    pty.capture_initial_cwd(crate::TerminalId::new(1), ProcessId::new(100), None);

    assert!(collect_cwd_changed_events(&rx).is_empty());
    assert!(pty.terminal_cwds.is_empty());
}

/// Both writers go through one place, so the ticker must not say again what the
/// capture already said.
#[test]
fn an_initial_cwd_reports_once_and_the_ticker_stays_quiet() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/work/agent"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.capture_initial_cwd(crate::TerminalId::new(1), child_pid, None);
    pty.update_and_report_cwds();

    assert_eq!(collect_cwd_changed_events(&rx).len(), 1);
}

/// Screen holds the folder too, so that every pane manifest can carry it and a
/// plugin that missed the event above is put right by the next update.
#[test]
fn an_initial_cwd_reaches_screen() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/work/agent"));
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.capture_initial_cwd(crate::TerminalId::new(1), child_pid, None);

    assert_eq!(
        collect_pane_cwd_instructions(&screen_rx),
        vec![(TerminalId::new(1), Some(PathBuf::from("/work/agent")))]
    );
}

/// A terminal id is handed out again after its pane closes, so a folder left
/// behind on screen is the wrong answer for whichever pane inherits the id.
#[test]
fn a_closed_pane_takes_its_folder_with_it() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/work/agent"));
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.capture_initial_cwd(crate::TerminalId::new(1), child_pid, None);
    let _ = collect_pane_cwd_instructions(&screen_rx);

    pty.forget_terminal_cwd(crate::TerminalId::new(1));

    assert_eq!(
        collect_pane_cwd_instructions(&screen_rx),
        vec![(TerminalId::new(1), None)]
    );
    assert!(pty.terminal_cwds.is_empty());
}

// --- OSC7 CWD notification ---

#[test]
fn osc7_emits_cwd_changed() {
    let mock = MockOsApi::new();
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    pty.id_to_child_pid
        .insert(crate::TerminalId::new(1), ProcessId::new(100));

    pty.notify_cwd_from_osc7(crate::TerminalId::new(1), PathBuf::from("/tmp/new"));

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(events[0].1, PathBuf::from("/tmp/new"));
    assert_eq!(
        pty.terminal_cwds.get(&TerminalId::new(1)),
        Some(&PathBuf::from("/tmp/new")),
        "cache should be updated"
    );
}

#[test]
fn osc7_no_event_when_unchanged() {
    let mock = MockOsApi::new();
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    pty.id_to_child_pid
        .insert(crate::TerminalId::new(1), ProcessId::new(100));
    pty.terminal_cwds
        .insert(crate::TerminalId::new(1), PathBuf::from("/same"));

    pty.notify_cwd_from_osc7(crate::TerminalId::new(1), PathBuf::from("/same"));

    let events = collect_cwd_changed_events(&rx);
    assert!(events.is_empty(), "no event when osc7 path matches cache");
}

#[test]
fn osc7_clears_activity_flag() {
    let mock = MockOsApi::new();
    let (mut pty, _rx) = make_pty_with_plugin_receiver(mock);
    let flag = Arc::new(AtomicBool::new(true));
    pty.id_to_child_pid
        .insert(crate::TerminalId::new(1), ProcessId::new(100));
    pty.pane_activity_flags
        .insert(crate::TerminalId::new(1), flag.clone());

    pty.notify_cwd_from_osc7(crate::TerminalId::new(1), PathBuf::from("/new"));

    assert!(
        !flag.load(Ordering::Relaxed),
        "osc7 should clear the activity flag"
    );
}

#[test]
fn osc7_then_poll_skips_terminal() {
    let mock = MockOsApi::new();
    let child_pid = ProcessId::new(100);
    mock.set_cwd(child_pid, PathBuf::from("/from-proc"));
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, crate::TerminalId::new(1), child_pid);

    pty.notify_cwd_from_osc7(crate::TerminalId::new(1), PathBuf::from("/from-osc7"));
    let osc7_events = collect_cwd_changed_events(&rx);
    assert_eq!(osc7_events.len(), 1);

    pty.update_and_report_cwds();
    let cwd_events = collect_cwd_changed_events(&rx);
    let cmd_events = collect_command_changed_events(&rx);
    assert!(
        cwd_events.is_empty() && cmd_events.is_empty(),
        "poll after osc7 should skip terminal since flag was cleared"
    );
}

// ---- remote panes must never signal a local process ----
//
// pid spaces are per-machine. A remote pane's child pid, used locally, does not
// fail — it names whatever local process happens to hold that number, so a
// `SIGKILL` aimed at a pane lands on something unrelated. These pin that the
// remote check happens before the pid lookup, at every site that signals.

/// Build a pty holding a pane whose recorded pid is one that must never be
/// signalled, and mark that pane remote.
fn pty_with_remote_pane(terminal_id: TerminalId, bystander_pid: ProcessId) -> (Pty, MockOsApi) {
    let os_api = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    // A stale pid in the map is exactly the hazard: something has to be there
    // for the wrong branch to find.
    pty.id_to_child_pid.insert(terminal_id, bystander_pid);
    os_api.mark_remote(terminal_id);
    (pty, os_api)
}

#[test]
fn closing_a_remote_pane_does_not_signal_a_local_process() {
    let (mut pty, os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    let _ = pty.close_pane(PaneId::Terminal(crate::TerminalId::new(7)));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's close signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(
        os_api.remote_signals(),
        vec![(TerminalId::new(7), ChildSignal::Hup)]
    );
}

#[test]
fn a_failed_remote_termination_does_not_fail_the_close() {
    let (mut pty, os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    os_api.fail_clear_of(crate::TerminalId::new(7));

    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(7)))
        .is_ok());
}

#[test]
fn layout_override_completion_reports_failed_tabs_alongside_successes() {
    for successful_tabs in [0, 1] {
        let os_api = MockOsApi::new();
        *os_api.local_spawns_to_allow.lock().unwrap() = successful_tabs;
        *os_api.fail_local_spawns.lock().unwrap() = true;
        let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api);
        pty.finish_layout_override(PendingLayoutOverride {
            cwd: None,
            default_shell: Some(TerminalAction::RunCommand(RunCommand::default())),
            tabs_to_close: vec![TabId::new(9)],
            tabs: (0..2)
                .map(|id| {
                    (
                        TabId::new(id + 7),
                        TabLayoutInfo {
                            tab_index: TabPosition::new(id),
                            tab_name: None,
                            tiled_layout: TiledPaneLayout::default(),
                            floating_layouts: vec![],
                            swap_tiled_layouts: None,
                            swap_floating_layouts: None,
                        },
                        HashMap::new(),
                    )
                })
                .collect(),
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            client_id: CLIENT_1,
            completion_tx: None,
        })
        .unwrap();
        let (tabs, failed_tabs, tabs_to_close) = screen_rx
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                ScreenInstruction::OverrideLayoutComplete(
                    tabs,
                    failed_tabs,
                    tabs_to_close,
                    false,
                    false,
                    CLIENT_1,
                    _,
                ) => Some((tabs, failed_tabs, tabs_to_close)),
                _ => None,
            })
            .expect("completion is sent even when every tab fails");
        assert_eq!(tabs.len(), successful_tabs);
        assert_eq!(tabs_to_close, vec![TabId::new(9)]);
        if successful_tabs == 1 {
            assert_eq!(tabs[0].tab_index, TabId::new(7));
            assert_eq!(tabs[0].new_terminal_pids, vec![(TerminalId::new(0), None)]);
        }
        assert_eq!(
            failed_tabs,
            (successful_tabs..2)
                .map(|id| TabId::new(id + 7))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn remote_loading_details_cannot_write_terminal_control_sequences() {
    let command = RunCommand {
        target: PaneTarget::Ssh {
            host: "host\x1b[2J".to_owned(),
        },
        ..Default::default()
    };
    let screen = String::from_utf8(remote_loading_screen(&command, "connecting\x1b[H")).unwrap();

    assert!(!screen.contains("host\x1b[2J"));
    assert!(screen.contains(r"host\u{1b}[2J"));
    assert!(screen.contains(r"connecting\u{1b}[H"));
}

/// Take the one `SpawnResolved(Rerun)` a worker sent back.
fn resolved_rerun(
    pty_rx: &channels::Receiver<(PtyInstruction, ErrorContext)>,
) -> (PendingRerun, OneShot<Result<SpawnedRerun>>) {
    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::SpawnResolved(PendingSpawn::Rerun(pending, spawned)) = instruction else {
        panic!("expected a resolved rerun, got {instruction:?}")
    };
    (*pending, spawned)
}

#[test]
fn suspended_bubblewrap_creation_only_reserves_the_pane() {
    let os_api = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let command = RunCommand {
        command: "claude".into(),
        target: PaneTarget::Bubblewrap {
            root: "/repo".into(),
        },
        hold_on_start: true,
        ..Default::default()
    };
    let (_, held) = pty
        .spawn_prepared_terminal(
            TerminalAction::RunCommand(command),
            ClientTabIndexOrPaneId::ClientId(CLIENT_1),
        )
        .unwrap();
    assert!(held);
    assert!(os_api.resolved_spawns.lock().unwrap().is_empty());
}

#[test]
fn suspended_sandbox_and_remote_reruns_use_the_worker() {
    for target in [
        PaneTarget::Bubblewrap {
            root: "/repo".into(),
        },
        ssh_command().target,
    ] {
        let mut os_api = MockOsApi::new();
        os_api.spawn_started = Arc::new(Barrier::new(2));
        os_api.spawn_release = Arc::new(Barrier::new(2));
        let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
        let (pty_tx, pty_rx) = channels::unbounded();
        pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        let command = RunCommand {
            target,
            hold_on_start: true,
            ..ssh_command()
        };
        assert!(
            !command.defers_spawn(),
            "initial suspended creation must not launch"
        );
        assert!(pty
            .request_rerun(PaneId::Terminal(TerminalId::new(41)), command, None)
            .unwrap()
            .is_none());
        os_api.spawn_started.wait();
        assert!(pty.close_pane(PaneId::Terminal(TerminalId::new(7))).is_ok());
        assert!(pty_rx.try_recv().is_err());
        os_api.spawn_release.wait();
        let (pending, spawned) = resolved_rerun(&pty_rx);
        assert!(!pending.run_command.hold_on_start);
        pty.handle_rerun_spawned(pending, spawned).unwrap();
    }
}

/// The worker has already registered the remote transport and started the guest command by
/// the time its result is sent back, so a pane closed in the meantime still has to go through
/// `clear_terminal_id` when that message lands — otherwise the guest command and the transport
/// entry outlive the pane. A rerun queued behind it goes with the pane too.
#[test]
fn closing_a_pane_mid_transport_tears_down_the_late_spawn() {
    let mut os_api = MockOsApi::new();
    os_api.transport_started = Arc::new(Barrier::new(2));
    os_api.transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.transport_started.wait();
    // A rerun asked for while the spawn is off-thread is parked behind it; it must go
    // with the pane rather than being started against a closed one.
    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    assert_eq!(pty.reruns[&TerminalId::new(41)].queued.len(), 1);
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(41)))
        .is_ok());
    let closed_at = os_api.cleared_terminal_ids.lock().unwrap().len();
    os_api.transport_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    pty.handle_rerun_spawned(pending, spawned).unwrap();

    assert_eq!(
        os_api.cleared_terminal_ids.lock().unwrap()[closed_at..],
        [TerminalId::new(41)],
        "the late spawn must be torn down through the funnel"
    );
    assert!(!pty.reruns.contains_key(&TerminalId::new(41)));
    assert!(
        pty_rx.try_recv().is_err(),
        "no rerun may be started for a closed pane"
    );
}

/// The same race, but the spawn that comes back failed. There is no pane left to hold the
/// failure and no waiter to report it to, and the pty thread takes its errors fatally — so a
/// stale failure must be logged and dropped rather than propagated out of the handler.
#[test]
fn a_failed_spawn_for_a_closed_pane_does_not_fail_the_pty_thread() {
    let mut os_api = MockOsApi::new();
    os_api.spawn_started = Arc::new(Barrier::new(2));
    os_api.spawn_release = Arc::new(Barrier::new(2));
    os_api.fail_rerun_of(crate::TerminalId::new(41));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.spawn_started.wait();
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(41)))
        .is_ok());
    let closed_at = os_api.cleared_terminal_ids.lock().unwrap().len();
    os_api.spawn_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    pty.handle_rerun_spawned(pending, spawned)
        .expect("a stale failure must not reach the pty thread");

    assert!(
        os_api.cleared_terminal_ids.lock().unwrap()[closed_at..].is_empty(),
        "nothing started, so there is nothing to tear down"
    );
    assert!(!pty.reruns.contains_key(&TerminalId::new(41)));
    assert!(
        pty_rx.try_recv().is_err(),
        "no rerun may be started for a closed pane"
    );
}

/// The other way a stale completion can fail: the slot it rides in is empty. `take` is
/// fallible on its own, and its error used to travel out of the handler and through
/// `pty_thread_main`, which takes it fatally — for a pane that is already gone.
#[test]
fn an_empty_spawn_slot_for_a_closed_pane_does_not_fail_the_pty_thread() {
    let mut os_api = MockOsApi::new();
    os_api.transport_started = Arc::new(Barrier::new(2));
    os_api.transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.transport_started.wait();
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(41)))
        .is_ok());
    let closed_at = os_api.cleared_terminal_ids.lock().unwrap().len();
    os_api.transport_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    // Drain the slot behind the handler's back, the way a double delivery would.
    assert!(spawned.clone().take().is_ok());
    pty.handle_rerun_spawned(pending, spawned)
        .expect("an empty slot must not reach the pty thread");
    assert!(
        os_api.cleared_terminal_ids.lock().unwrap()[closed_at..].is_empty(),
        "there is no reader to tear down when the slot is empty"
    );
}

/// A local rerun (a plugin's `run_command_in_pane`, or the user dropping to a
/// shell) asked for while a remote one is off-thread waits its turn: running
/// it at once would put a second reader and a second process on the pane, and
/// the late remote result would then land on top of the new local one.
#[test]
fn a_local_rerun_during_a_deferred_one_is_queued_not_run_twice() {
    let mut os_api = MockOsApi::new();
    os_api.spawn_started = Arc::new(Barrier::new(2));
    os_api.spawn_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    os_api.mark_remote(crate::TerminalId::new(41));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.spawn_started.wait();
    let local = RunCommand {
        command: PathBuf::from("sh"),
        ..Default::default()
    };
    assert!(
        pty.request_rerun(PaneId::Terminal(crate::TerminalId::new(41)), local, None)
            .unwrap()
            .is_none(),
        "a local rerun of a pane with one in flight is queued, not handed back"
    );
    assert!(os_api.rerun_commands().is_empty());
    os_api.spawn_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    pty.handle_rerun_spawned(pending, spawned).unwrap();

    let reruns = os_api.rerun_commands();
    assert_eq!(reruns.len(), 1, "the queued local rerun runs once, after");
    assert_eq!(reruns[0].command, PathBuf::from("sh"));
    assert!(!pty.reruns.contains_key(&TerminalId::new(41)));
    assert!(os_api.cleared_terminal_ids.lock().unwrap().is_empty());
}

/// A second remote rerun while the first is resolving does not discard the
/// first (its VM boot, its reserved id and its waiter); it runs after it.
#[test]
fn a_second_deferred_rerun_is_queued_and_the_first_result_is_kept() {
    let mut os_api = MockOsApi::new();
    os_api.spawn_started = Arc::new(Barrier::new(2));
    os_api.spawn_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.spawn_started.wait();
    let mut second = ssh_command();
    second.args = vec!["--second".to_owned()];
    assert!(pty
        .request_rerun(PaneId::Terminal(crate::TerminalId::new(41)), second, None)
        .unwrap()
        .is_none());
    os_api.spawn_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    assert!(
        pending.run_command.args.is_empty(),
        "the first request lands first"
    );
    pty.handle_rerun_spawned(pending, spawned).unwrap();
    assert!(
        pty.reruns.contains_key(&TerminalId::new(41)),
        "the second is off-thread now"
    );

    os_api.spawn_started.wait();
    os_api.spawn_release.wait();
    let (pending, spawned) = resolved_rerun(&pty_rx);
    assert_eq!(pending.run_command.args, vec!["--second".to_owned()]);
    pty.handle_rerun_spawned(pending, spawned).unwrap();

    assert!(!pty.reruns.contains_key(&TerminalId::new(41)));
    assert_eq!(
        *os_api.resolved_spawns.lock().unwrap(),
        vec![TerminalId::new(41), TerminalId::new(41)]
    );
    assert!(
        os_api.cleared_terminal_ids.lock().unwrap().is_empty(),
        "neither result was thrown away"
    );
}

/// An ssh pane's connect and bootstrap take as long as the network does, and
/// they now run where an SSH connection does: on a worker, with the pty thread free.
#[test]
fn an_ssh_spawn_does_not_block_the_pty_thread() {
    let mut os_api = MockOsApi::new();
    os_api.spawn_started = Arc::new(Barrier::new(2));
    os_api.spawn_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let command = RunCommand {
        command: PathBuf::from("htop"),
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    assert!(command.defers_spawn());
    pty.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
        terminal_id: crate::TerminalId::new(41),
        terminal_action: TerminalAction::RunCommand(command),
    }))
    .unwrap();
    os_api.spawn_started.wait();
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(7)))
        .is_ok());
    assert!(pty_rx.try_recv().is_err());
    os_api.spawn_release.wait();

    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        instruction,
        PtyInstruction::SpawnResolved(PendingSpawn::Terminal(..))
    ));
    assert_eq!(
        *os_api.resolved_spawns.lock().unwrap(),
        vec![TerminalId::new(41)]
    );
}

#[test]
fn layout_handoffs_after_screen_shutdown_close_panes_without_starting_remote_commands() {
    for override_layout in [false, true] {
        let os_api = MockOsApi::new();
        *os_api.local_spawns_to_allow.lock().unwrap() = 1;
        let (mut pty, plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
        let shutdown = Shutdown::default();
        let (screen_tx, screen_rx) = channels::unbounded();
        let (pty_tx, _pty_rx) = channels::unbounded();
        pty.bus.senders.to_screen = Some(shutdown.sender(screen_tx));
        pty.bus.senders.to_pty = Some(shutdown.sender(pty_tx));
        pty.bus.senders.should_silently_fail = false;
        shutdown.begin();
        drop(screen_rx);

        // Ordinary updates still tolerate an exited screen.
        pty.bus
            .senders
            .send_to_screen(ScreenInstruction::Render)
            .unwrap();
        let local = RunCommand {
            command: PathBuf::from("sh"),
            ..Default::default()
        };
        let layout = TiledPaneLayout {
            run: Some(Run::Command(local.clone())),
            ..Default::default()
        };
        let floating_layouts = vec![FloatingPaneLayout {
            run: Some(Run::Command(RunCommand {
                command: PathBuf::from("sh"),
                target: PaneTarget::Ssh {
                    host: "remote-host".to_owned(),
                },
                ..Default::default()
            })),
            ..Default::default()
        }];
        let default_shell = Some(TerminalAction::RunCommand(local));
        let plugin_ids = HashMap::from([(
            RunPluginOrAlias::default(),
            vec![PluginId::new(100), PluginId::new(101)],
        )]);
        let result = if override_layout {
            pty.finish_layout_override(PendingLayoutOverride {
                cwd: None,
                default_shell,
                tabs_to_close: vec![],
                tabs: vec![(
                    TabId::new(0),
                    TabLayoutInfo {
                        tab_index: TabPosition::new(0),
                        tab_name: None,
                        tiled_layout: layout,
                        floating_layouts,
                        swap_tiled_layouts: None,
                        swap_floating_layouts: None,
                    },
                    plugin_ids,
                )],
                retain_existing_terminal_panes: false,
                retain_existing_plugin_panes: false,
                client_id: CLIENT_1,
                completion_tx: None,
            })
        } else {
            pty.spawn_terminals_for_layout(
                None,
                layout,
                floating_layouts,
                default_shell,
                plugin_ids,
                None,
                TabId::new(0),
                false,
                true,
                CLIENT_1,
                None,
            )
        };

        let error = result.expect_err("the layout handoff must fail after the screen exits");
        assert!(format!("{error:#}").contains("failed to send message to screen"));
        let mut cleared = os_api.cleared_terminal_ids.lock().unwrap().clone();
        cleared.sort_unstable();
        assert_eq!(cleared, vec![TerminalId::new(0), TerminalId::new(1)]);
        assert_eq!(
            os_api.local_signals(),
            vec![(ProcessId::new(1000), "SIGHUP")]
        );
        assert!(pty.id_to_child_pid.is_empty());
        assert!(pty.task_handles.is_empty());
        assert!(pty.pending_remote_spawns.is_empty());
        assert!(pty.spawn_workers.is_empty());
        assert!(os_api.resolved_spawns.lock().unwrap().is_empty());
        let unloaded: Vec<_> = plugin_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::Unload(plugin_id) => Some(plugin_id),
                _ => None,
            })
            .collect();
        assert_eq!(unloaded, vec![PluginId::new(100), PluginId::new(101)]);
    }
}

/// A layout that fails part-way through must not leave the panes it already
/// started running with nobody to close them: Screen never hears of a layout
/// that does not reach `ApplyLayout`.
#[test]
fn a_failing_layout_closes_the_panes_it_already_spawned() {
    let os_api = MockOsApi::new();
    let (mut pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, _pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let (server_tx, server_rx) = channels::unbounded();
    pty.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    let layout = failing_three_slot_layout();
    // The first local pane starts for real, the second one fails.
    *os_api.local_spawns_to_allow.lock().unwrap() = 1;
    *os_api.fail_local_spawns.lock().unwrap() = true;

    let result = pty.spawn_terminals_for_layout(
        None,
        layout,
        vec![],
        Some(TerminalAction::RunCommand(RunCommand::default())),
        HashMap::from([(
            RunPluginOrAlias::default(),
            vec![PluginId::new(100), PluginId::new(101)],
        )]),
        None,
        TabId::new(0),
        false,
        true,
        crate::ClientId::new(1),
        Some(NotificationEnd::new(completion_tx)),
    );

    assert!(result.is_err(), "the local failure is still reported");
    let completion = completion_rx
        .try_recv()
        .expect("failed spawn releases its caller");
    assert_eq!(completion.exit_status, Some(1));
    assert!(completion
        .error_message
        .as_ref()
        .unwrap()
        .contains("Failed to spawn layout"));
    assert!(server_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ServerInstruction::LogError(messages, CLIENT_1, None)
            if messages == vec![completion.error_message.clone().unwrap()]
    )));
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::LayoutSpawnFailed(id, _) if id == TabId::new(0)
    )));
    let mut cleared = os_api.cleared_terminal_ids.lock().unwrap().clone();
    cleared.sort_unstable();
    assert_eq!(
        cleared,
        vec![TerminalId::new(0), TerminalId::new(1)],
        "both the running local pane and the reserved remote one are released"
    );
    assert_eq!(
        os_api.local_signals(),
        vec![(ProcessId::new(1000), "SIGHUP")],
        "the local child that was already running is hung up"
    );
    assert!(pty.id_to_child_pid.is_empty());
    assert!(pty.task_handles.is_empty());
    let unloaded: Vec<_> = plugin_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::Unload(plugin_id) => Some(plugin_id),
            _ => None,
        })
        .collect();
    assert_eq!(unloaded, vec![PluginId::new(100), PluginId::new(101)]);
}

/// The same, for the layout-override path - whose caller logs the failure and
/// carries on, so nothing else would ever notice the orphans.
#[test]
fn a_failing_layout_override_closes_the_panes_it_already_spawned() {
    let os_api = MockOsApi::new();
    let (mut pty, plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, _pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let layout = failing_three_slot_layout();
    *os_api.local_spawns_to_allow.lock().unwrap() = 1;
    *os_api.fail_local_spawns.lock().unwrap() = true;

    let result = pty.spawn_terminals_for_layout_override(
        None,
        layout,
        vec![],
        Some(TerminalAction::RunCommand(RunCommand::default())),
        HashMap::from([(
            RunPluginOrAlias::default(),
            vec![PluginId::new(100), PluginId::new(101)],
        )]),
        TabId::new(0),
        None,
        crate::ClientId::new(1),
        None,
        None,
    );

    assert!(result.is_err(), "the local failure is still reported");
    let mut cleared = os_api.cleared_terminal_ids.lock().unwrap().clone();
    cleared.sort_unstable();
    assert_eq!(cleared, vec![TerminalId::new(0), TerminalId::new(1)]);
    assert_eq!(
        os_api.local_signals(),
        vec![(ProcessId::new(1000), "SIGHUP")]
    );
    assert!(pty.id_to_child_pid.is_empty());
    assert!(pty.task_handles.is_empty());
    let unloaded: Vec<_> = plugin_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::Unload(plugin_id) => Some(plugin_id),
            _ => None,
        })
        .collect();
    assert_eq!(unloaded, vec![PluginId::new(100), PluginId::new(101)]);
}

/// An ssh pane's process is the point: it survives the session on its broker,
/// where `yinz pty-host adopt --host` can find it, so teardown leaves it be.
#[test]
fn pty_teardown_leaves_ssh_panes_adoptable() {
    let os_api = MockOsApi::new();
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    os_api.mark_remote(crate::TerminalId::new(7));

    drop(pty);

    assert!(os_api.remote_signals().is_empty());
    assert!(os_api.cleared_terminal_ids.lock().unwrap().is_empty());
}

/// Closing a remote pane aborts its reader and moves on. The wait for the
/// reader to actually stop belongs to the warp path, which reuses the socket;
/// a close does not, and the pty thread must not spend the reader's shutdown
/// timeout on every remote pane of a closing tab.
#[test]
fn closing_a_remote_pane_does_not_wait_for_its_reader() {
    let (mut pty, os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    let (started_tx, started_rx) = channels::bounded(1);
    let (release_tx, release_rx) = channels::bounded::<()>(1);
    // A reader that never reaches an await point until released, so an abort
    // alone cannot end it within the shutdown timeout.
    pty.task_handles.insert(
        crate::TerminalId::new(7),
        async_runtime().spawn(async move {
            let _ = started_tx.send(());
            let _ = tokio::task::spawn_blocking(move || release_rx.recv()).await;
        }),
    );
    started_rx.recv().unwrap();

    let start = std::time::Instant::now();
    pty.close_pane(PaneId::Terminal(crate::TerminalId::new(7)))
        .unwrap();
    let took = start.elapsed();
    let _ = release_tx.send(());

    assert!(
        took < READER_SHUTDOWN_TIMEOUT,
        "close_pane waited {took:?} for a reader it only had to abort"
    );
    assert_eq!(
        os_api.remote_signals(),
        vec![(TerminalId::new(7), ChildSignal::Hup)]
    );
}

#[test]
fn sigint_to_a_remote_pane_does_not_signal_a_local_process() {
    let (pty, os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    pty.send_sigint_to_pane(PaneId::Terminal(crate::TerminalId::new(7)));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's SIGINT signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(
        os_api.remote_signals(),
        vec![(TerminalId::new(7), ChildSignal::Int)]
    );
}

#[test]
fn sigkill_to_a_remote_pane_does_not_signal_a_local_process() {
    let (pty, os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    pty.send_sigkill_to_pane(PaneId::Terminal(crate::TerminalId::new(7)));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's SIGKILL signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(
        os_api.remote_signals(),
        vec![(TerminalId::new(7), ChildSignal::Kill)]
    );
}

/// The same refactor must leave local panes exactly as they were.
#[test]
fn a_local_pane_is_still_signalled_by_pid() {
    let os_api = MockOsApi::new();
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let mut pty = pty;
    pty.id_to_child_pid
        .insert(crate::TerminalId::new(3), ProcessId::new(909));

    pty.send_sigint_to_pane(PaneId::Terminal(crate::TerminalId::new(3)));
    pty.send_sigkill_to_pane(PaneId::Terminal(crate::TerminalId::new(3)));
    let _ = pty.close_pane(PaneId::Terminal(crate::TerminalId::new(3)));

    assert_eq!(
        os_api.local_signals(),
        vec![
            (ProcessId::new(909), "SIGINT"),
            (ProcessId::new(909), "SIGKILL"),
            (ProcessId::new(909), "SIGHUP")
        ]
    );
    assert!(os_api.remote_signals().is_empty());
}

/// Handing a remote pid to a plugin invites the same mistake one layer out.
#[test]
fn a_remote_panes_pid_is_not_handed_to_plugins() {
    let (pty, _os_api) = pty_with_remote_pane(
        crate::TerminalId::new(7),
        yinz_types::internal_ids::ProcessId::new(4242),
    );
    match pty.get_pane_pid(PaneId::Terminal(crate::TerminalId::new(7))) {
        GetPanePidResponse::Err(msg) => {
            assert!(
                msg.contains("another machine"),
                "the refusal should say why, got: {msg}"
            );
        },
        GetPanePidResponse::Ok(pid) => {
            panic!("a remote pane handed out local pid {pid}");
        },
    }
}

// ---- remote panes report from the broker, not from this machine ----

/// A pane whose process is on another machine, holding the status its broker
/// last reported.
fn remote_pane(
    terminal_id: TerminalId,
    status: RemoteStatus,
) -> (Pty, MockOsApi, PluginReceiver, ScreenReceiver) {
    let mock = MockOsApi::new();
    mock.set_remote_status(terminal_id, status);
    let (pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    (pty, mock, plugin_rx, screen_rx)
}

#[test]
fn remote_agent_hooks_queue_state_and_event_together() {
    let (pty, _, plugin_rx, screen_rx) = remote_pane(TerminalId::new(7), RemoteStatus::default());
    for kind in ["claude", "codex"] {
        for (hook, expected_state) in [
            ("SessionStart", Some(AgentState::Idle)),
            ("UserPromptSubmit", Some(AgentState::Busy)),
            (
                "Stop",
                Some(if kind == "claude" {
                    AgentState::Busy
                } else {
                    AgentState::Idle
                }),
            ),
            ("SessionEnd", None),
            ("UnknownHook", None),
        ] {
            let json = serde_json::to_string_pretty(&serde_json::json!({
                "hook_event_name": hook, "session_id": "parent", "source": "startup",
                "cwd": "/remote/work", "prompt": "review", "extra": {"preserved": true},
            }))
            .unwrap();
            let received_at = Instant::now();
            handle_remote_agent_report(&pty.bus.senders, TerminalId::new(7), kind, json.clone())
                .unwrap();
            let (ScreenInstruction::AgentReport(report, accepted_at), _) =
                screen_rx.try_recv().unwrap()
            else {
                panic!("expected a queued agent report");
            };
            assert!(received_at <= accepted_at && accepted_at <= Instant::now());
            yinz_runtime::agent::report::validate_report(&report).unwrap();
            assert_eq!(report.pane_id, PaneId::Terminal(TerminalId::new(7)));
            assert_eq!(report.kind, kind);
            assert_eq!(report.process_id, ProcessId::new(std::process::id()));
            if let Some(state) = expected_state {
                let AgentReportState::Set(agent) = report.state else {
                    panic!("expected state from {hook}");
                };
                assert_eq!(agent.state, state);
                assert_eq!(agent.kind, kind);
                assert_eq!(agent.source, AgentSource::Reported);
                assert_eq!(agent.session_id.as_deref(), Some("parent"));
                assert_eq!(agent.cwd.as_deref(), Some("/remote/work"));
            } else {
                assert_eq!(
                    report.state,
                    if hook == "SessionEnd" {
                        AgentReportState::Clear
                    } else {
                        AgentReportState::Unchanged
                    }
                );
            }
            let event = report.event.unwrap();
            assert_eq!(event.pane_id, TerminalId::new(7));
            assert_eq!(event.state, expected_state);
            assert_eq!(event.hook, hook);
            assert_eq!(event.kind, kind);
            assert_eq!(event.raw, json);
            assert!(screen_rx.try_recv().is_err(), "only one report is queued");
            assert!(
                plugin_rx.try_recv().is_err(),
                "screen must broadcast the event"
            );
        }
    }
}

#[test]
fn remote_subagent_hooks_queue_events_without_writing_parent_state() {
    let (pty, _, plugin_rx, screen_rx) = remote_pane(TerminalId::new(7), RemoteStatus::default());
    for kind in ["claude", "codex"] {
        for hook in ["SubagentStart", "SubagentStop"] {
            let json = serde_json::json!({
                "hook_event_name": hook, "session_id": "parent", "agent_id": "recap",
                "last_assistant_message": "Recap completed",
            })
            .to_string();
            handle_remote_agent_report(&pty.bus.senders, TerminalId::new(7), kind, json.clone())
                .unwrap();
            let (ScreenInstruction::AgentReport(report, _), _) = screen_rx.try_recv().unwrap()
            else {
                panic!("expected a queued agent report");
            };
            assert_eq!(report.pane_id, PaneId::Terminal(TerminalId::new(7)));
            assert_eq!(report.state, AgentReportState::Unchanged);
            let event = report.event.unwrap();
            assert_eq!(event.state, None);
            assert_eq!(event.hook, hook);
            assert_eq!(event.kind, kind);
            assert_eq!(event.raw, json);
            assert!(screen_rx.try_recv().is_err(), "only one report is queued");
            assert!(
                plugin_rx.try_recv().is_err(),
                "screen must broadcast the event"
            );
        }
    }
}

#[test]
fn invalid_remote_agent_reports_are_rejected_without_queueing() {
    let (pty, _, plugin_rx, screen_rx) = remote_pane(TerminalId::new(7), RemoteStatus::default());
    let oversized = serde_json::json!({
        "hook_event_name": "SessionStart", "session_id": "parent", "source": "startup",
        "prompt": "x".repeat(HOOK_BROADCAST_MAX_BYTES),
    })
    .to_string();
    for (json, message) in [
        ("{".to_owned(), "invalid remote agent report"),
        (oversized, "agent hook event exceeds the broadcast limit"),
    ] {
        let error = handle_remote_agent_report(&pty.bus.senders, TerminalId::new(7), "codex", json)
            .unwrap_err();
        assert_eq!(error.to_string(), message);
        assert!(screen_rx.try_recv().is_err());
        assert!(plugin_rx.try_recv().is_err());
    }
}

/// The whole point of Phase 3: argv the broker read on its own machine reaches
/// the same event a local pane's argv would. Agent classification is pure string
/// matching over that argv, so this is what makes a remote agent detectable at
/// all — with nothing in the detection layer aware of the difference.
#[test]
fn a_remote_panes_foreground_command_is_reported_like_any_other() {
    let (mut pty, _mock, plugin_rx, _screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned(), "--resume".to_owned()]),
        },
    );

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&plugin_rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(crate::TerminalId::new(7)));
    assert_eq!(events[0].1, vec!["claude", "--resume"]);
    assert!(
        events[0].2,
        "a foreground command must be flagged as one, or it reads as the shell"
    );
}

/// The process holding a sandboxed pane's pty is this binary's launcher and then `bwrap`.
/// Reporting either would unlatch a live agent, so the pane reports the command inside -
/// and only a pane launched into a sandbox does: a shell that runs `bwrap` by hand is
/// reported as it is.
#[test]
fn a_bubblewrap_pane_reports_the_command_inside_its_sandbox() {
    let sandboxed = TerminalId::new(1);
    let shell = TerminalId::new(2);
    let bwrap: Vec<String> = [
        "bwrap",
        "--unshare-all",
        "--chdir",
        "/repo",
        "--",
        "env",
        "YINZ_AGENT_PANE=1",
        "claude",
    ]
    .map(str::to_owned)
    .to_vec();
    let mock = MockOsApi::new();
    let (mut pty, plugin_rx) = make_pty_with_plugin_receiver(mock.clone());
    for (terminal_id, pid) in [(sandboxed, 100), (shell, 200)] {
        set_active_terminal(&mut pty, terminal_id, ProcessId::new(pid));
        mock.cmds
            .lock()
            .unwrap()
            .insert(ProcessId::new(pid), bwrap.clone());
    }
    pty.track_launch(
        sandboxed,
        &RunCommand {
            command: "env".into(),
            target: PaneTarget::Bubblewrap {
                root: "/repo".into(),
            },
            ..Default::default()
        },
    );
    pty.track_launch(shell, &RunCommand::default());

    pty.update_and_report_cwds();

    let mut events = collect_command_changed_events(&plugin_rx);
    events.sort_by_key(|(pane_id, _, _)| *pane_id);
    assert_eq!(
        events,
        vec![
            (
                PaneId::Terminal(sandboxed),
                vec![
                    "env".to_owned(),
                    "YINZ_AGENT_PANE=1".to_owned(),
                    "claude".to_owned()
                ],
                false
            ),
            (PaneId::Terminal(shell), bwrap.clone(), false),
        ]
    );
    assert_eq!(
        pty.get_pane_running_command(PaneId::Terminal(sandboxed)),
        GetPaneRunningCommandResponse::Ok(vec![
            "env".to_owned(),
            "YINZ_AGENT_PANE=1".to_owned(),
            "claude".to_owned()
        ])
    );
    // A rerun as a plain command stops reading through.
    pty.track_launch(sandboxed, &RunCommand::default());
    assert_eq!(
        pty.get_pane_running_command(PaneId::Terminal(sandboxed)),
        GetPaneRunningCommandResponse::Ok(bwrap)
    );
}

/// With nothing in the foreground, the pane's own command is reported and
/// flagged as not-foreground — exactly as for a local pane.
#[test]
fn a_remote_pane_with_no_foreground_falls_back_to_its_own_command() {
    let (mut pty, _mock, plugin_rx, _screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: None,
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: None,
        },
    );

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&plugin_rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, vec!["/bin/zsh"]);
    assert!(!events[0].2);
}

/// A remote pane's cwd is a path on its own machine, and travels unchanged.
#[test]
fn a_remote_panes_cwd_is_reported_unchanged() {
    let (mut pty, _mock, _plugin_rx, screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/srv/build/project".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: None,
        },
    );

    pty.update_and_report_cwds();

    let cwds = collect_pane_cwd_instructions(&screen_rx);
    assert_eq!(
        cwds,
        vec![(
            TerminalId::new(7),
            Some(PathBuf::from("/srv/build/project"))
        )],
        "a remote cwd must be reported as-is"
    );
}

/// The activity flag exists because inspecting a *local* pane costs a `sysinfo`
/// refresh. A remote pane's status is already cached locally, so it is read
/// every tick — otherwise an agent sitting silently at its prompt, which is
/// precisely the state worth seeing, would never be re-polled.
#[test]
fn a_silent_remote_pane_is_still_polled() {
    let (mut pty, mock, plugin_rx, _screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned()]),
        },
    );

    // No activity flag is ever set: this pane has produced no output at all.
    pty.update_and_report_cwds();
    let first = collect_command_changed_events(&plugin_rx);
    assert_eq!(first.len(), 1, "a silent remote pane was skipped entirely");

    mock.set_remote_status(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["vim".to_owned()]),
        },
    );
    pty.update_and_report_cwds();
    let second = collect_command_changed_events(&plugin_rx);
    assert_eq!(second.len(), 1, "a change on a silent pane went unreported");
    assert_eq!(second[0].1, vec!["vim"]);
}

/// Unchanged status must not re-emit, or every tick would wake every plugin.
#[test]
fn an_unchanged_remote_pane_emits_nothing_further() {
    let (mut pty, _mock, plugin_rx, _screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned()]),
        },
    );

    pty.update_and_report_cwds();
    assert_eq!(collect_command_changed_events(&plugin_rx).len(), 1);
    pty.update_and_report_cwds();
    assert!(
        collect_command_changed_events(&plugin_rx).is_empty(),
        "an unchanged pane should stay quiet"
    );
}

/// Plugins ask directly when the poll missed something (`agent-service` repairs
/// panes this way). A remote pane has to answer from its cache rather than
/// reporting itself as not running.
#[test]
fn a_remote_pane_answers_the_plugin_facing_accessors() {
    let (pty, _mock, _plugin_rx, _screen_rx) = remote_pane(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/srv/build/project".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned()]),
        },
    );

    match pty.get_pane_running_command(PaneId::Terminal(crate::TerminalId::new(7))) {
        GetPaneRunningCommandResponse::Ok(argv) => assert_eq!(argv, vec!["claude".to_owned()]),
        other => panic!("expected the running command, got {other:?}"),
    }
    match pty.get_pane_cwd(PaneId::Terminal(crate::TerminalId::new(7))) {
        GetPaneCwdResponse::Ok(cwd) => assert_eq!(cwd, PathBuf::from("/srv/build/project")),
        other => panic!("expected the cwd, got {other:?}"),
    }
}

/// The far machine ran Git in the pane's folder, and the answer is handed to the
/// plugins raw. Raw is the point: a repository with no commits writes its
/// identity to stdout and *then* exits 128, so a parse that gated on the exit
/// code would record a real checkout as an ordinary folder — and the parser that
/// knows this already exists in `agent-service`, for local panes.
#[test]
fn a_remote_pane_hands_its_git_probe_to_the_plugins() {
    let mock = MockOsApi::new();
    mock.set_remote_git_probe(
        crate::TerminalId::new(7),
        RemoteGitProbe {
            refresh_token: None,
            host: "build-box".to_owned(),
            cwd: "/srv/build/project".to_owned(),
            exit_code: Some(128),
            metadata: b"/srv/build/project\n/srv/build/project/.git\n".to_vec(),
            config: b"remote.origin.url\ngit@example.com:me/project\0".to_vec(),
        },
    );
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);

    assert_eq!(
        pty.get_pane_git_probe(PaneId::Terminal(crate::TerminalId::new(7)), "/agent", None),
        GetPaneGitProbeResponse::Pending
    );
    match pty.get_pane_git_probe(
        PaneId::Terminal(crate::TerminalId::new(7)),
        "/srv/build/project",
        None,
    ) {
        GetPaneGitProbeResponse::Ok(probe) => {
            assert_eq!(
                probe.host, "build-box",
                "without the machine, two checkouts at the same path would merge"
            );
            assert_eq!(probe.cwd, "/srv/build/project");
            assert_eq!(probe.exit_code, Some(128));
            assert!(String::from_utf8_lossy(&probe.metadata).contains("/srv/build/project/.git"));
            assert!(probe.config.contains(&0), "git's --null output, unmangled");
        },
        other => panic!("expected the git probe, got {other:?}"),
    }
}

/// A local pane has no cached probe, and the pty thread will not spawn Git to
/// make one — every pane in the session is waiting on that thread. Saying so is
/// not the same as answering "not a repository", which is what an empty probe
/// would read as.
#[test]
fn a_local_pane_has_no_git_probe_to_give() {
    let mock = MockOsApi::new();
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);

    match pty.get_pane_git_probe(
        PaneId::Terminal(crate::TerminalId::new(7)),
        "/srv/build/project",
        None,
    ) {
        GetPaneGitProbeResponse::Err(message) => assert!(
            message.contains("not remote"),
            "the reason has to be readable: {message}"
        ),
        other => panic!("expected an error for a local pane, got {other:?}"),
    }
}

/// A pane's host reaches Screen, which is what puts it on `PaneInfo` for the
/// plugins that decide which project a pane belongs to.
#[test]
fn a_remote_panes_host_is_announced_to_screen() {
    let mock = MockOsApi::new();
    mock.set_remote_status(
        crate::TerminalId::new(7),
        RemoteStatus {
            child_group_terminated: false,
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: None,
        },
    );
    mock.set_remote_host(crate::TerminalId::new(7), "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.report_terminal_host(crate::TerminalId::new(7));

    let hosts = collect_pane_host_instructions(&screen_rx);
    assert_eq!(
        hosts,
        vec![(TerminalId::new(7), Some("build-box".to_owned()))]
    );
}

/// Local panes are the overwhelming majority, and must stay silent — an
/// announcement per pane would be noise, and a `Some` on a local pane would make
/// every consumer treat it as elsewhere.
#[test]
fn a_local_pane_announces_no_host() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.report_terminal_host(crate::TerminalId::new(3));

    assert!(
        collect_pane_host_instructions(&screen_rx).is_empty(),
        "a local pane should say nothing about a host"
    );
}

/// A respawn is the one moment a pane's host can change, so unlike the first report it has to
/// be able to say `None` — a workflow step that moves from a remote host back to this machine
/// would otherwise leave the pane attributed to the host it left.
#[test]
fn a_respawn_back_onto_this_machine_retracts_the_panes_host() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(7)),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        collect_pane_host_instructions(&screen_rx),
        vec![(TerminalId::new(7), None)],
        "the pane's host should be retracted, exactly once"
    );
}

/// And a respawn that stays put says so in a single update. A `forget` followed by a `report`
/// would be two, and a plugin reading the pair would see the pane briefly claim to be local.
#[test]
fn a_respawn_on_the_same_host_reports_it_once_and_never_blips() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(7), "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(7)),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            target: PaneTarget::Ssh {
                host: "build-box".to_owned(),
            },
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        collect_pane_host_instructions(&screen_rx),
        vec![(TerminalId::new(7), Some("build-box".to_owned()))],
        "one update naming the host, with no host-less one before it"
    );
}

#[test]
fn a_respawn_onto_another_host_clears_the_old_hosts_cwd() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(7), "old-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(crate::TerminalId::new(7), PathBuf::from("/srv/on-old-box"));

    pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(7)),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            target: PaneTarget::Ssh {
                host: "new-box".to_owned(),
            },
            ..Default::default()
        },
    )
    .unwrap();

    assert!(pty.terminal_cwds.is_empty());
    assert_eq!(
        collect_pane_cwd_instructions(&screen_rx),
        vec![(TerminalId::new(7), None)]
    );
}

#[test]
fn a_same_host_respawn_keeps_its_cwd_until_the_host_reports_again() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(7), "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds.insert(
        crate::TerminalId::new(7),
        PathBuf::from("/srv/on-build-box"),
    );

    pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(7)),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            target: PaneTarget::Ssh {
                host: "build-box".to_owned(),
            },
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        pty.terminal_cwds.get(&TerminalId::new(7)),
        Some(&PathBuf::from("/srv/on-build-box"))
    );
    assert!(collect_pane_cwd_instructions(&screen_rx).is_empty());
}

#[test]
fn a_failed_host_change_retracts_the_old_host_and_cwd() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(7), "old-box");
    mock.fail_rerun_of(crate::TerminalId::new(7));
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(crate::TerminalId::new(7), PathBuf::from("/srv/on-old-box"));

    let result = pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(7)),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            target: PaneTarget::Ssh {
                host: "new-box".to_owned(),
            },
            ..Default::default()
        },
    );

    assert!(result.is_err());
    assert!(pty.terminal_cwds.is_empty());
    let mut cwd_updates = Vec::new();
    let mut host_updates = Vec::new();
    for (instruction, _) in screen_rx.try_iter() {
        match instruction {
            ScreenInstruction::UpdatePaneCwd(id, cwd) => cwd_updates.push((id, cwd)),
            ScreenInstruction::UpdatePaneHost(id, host) => host_updates.push((id, host)),
            _ => {},
        }
    }
    assert_eq!(cwd_updates, vec![(TerminalId::new(7), None)]);
    assert_eq!(host_updates, vec![(TerminalId::new(7), None)]);
}

/// A cwd is a path on one machine. Handing a local pane's cwd to a pane that
/// runs elsewhere names a directory that need not exist there — and for a pane
/// that asked for a login shell it is worse than useless: an unset cwd is
/// precisely what lands it in the remote home directory, the way ssh does.
#[test]
fn a_remote_pane_does_not_inherit_a_local_panes_cwd() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds.insert(
        crate::TerminalId::new(3),
        PathBuf::from("/home/me/on-this-machine"),
    );

    let mut action = TerminalAction::RunCommand(RunCommand {
        remote_default_shell: true,
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(crate::TerminalId::new(3)));

    match action {
        TerminalAction::RunCommand(command) => assert_eq!(command.cwd, None),
        other => panic!("expected a RunCommand, got {other:?}"),
    }
}

/// A pane on the *same* machine is a different matter: its cwd is a path the new
/// pane can actually use, so opening a pane from a remote pane keeps landing in
/// that pane's directory.
#[test]
fn a_remote_pane_inherits_the_cwd_of_a_pane_on_its_own_machine() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(3), "build-box");
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds.insert(
        crate::TerminalId::new(3),
        PathBuf::from("/srv/on-the-far-machine"),
    );

    let mut action = TerminalAction::RunCommand(RunCommand {
        remote_default_shell: true,
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(crate::TerminalId::new(3)));

    match action {
        TerminalAction::RunCommand(command) => {
            assert_eq!(command.cwd, Some(PathBuf::from("/srv/on-the-far-machine")))
        },
        other => panic!("expected a RunCommand, got {other:?}"),
    }
}

/// The mirror image, and the one that keeps this from being a regression: a
/// local pane still inherits a local pane's cwd.
#[test]
fn a_local_pane_still_inherits_a_local_panes_cwd() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds.insert(
        crate::TerminalId::new(3),
        PathBuf::from("/home/me/on-this-machine"),
    );

    let mut action = TerminalAction::RunCommand(RunCommand {
        command: PathBuf::from("/bin/sh"),
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(crate::TerminalId::new(3)));

    match action {
        TerminalAction::RunCommand(command) => {
            assert_eq!(command.cwd, Some(PathBuf::from("/home/me/on-this-machine")))
        },
        other => panic!("expected a RunCommand, got {other:?}"),
    }
}

/// Closing a pane retracts it, or Screen would keep attributing a recycled
/// terminal id to a machine it has nothing to do with.
#[test]
fn closing_a_pane_retracts_its_host() {
    let mock = MockOsApi::new();
    mock.set_remote_host(crate::TerminalId::new(7), "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    let _ = pty.close_pane(PaneId::Terminal(crate::TerminalId::new(7)));

    let hosts = collect_pane_host_instructions(&screen_rx);
    assert!(
        hosts.contains(&(crate::TerminalId::new(7), None)),
        "closing a pane should retract its host, got {hosts:?}"
    );
}

/// Which failures the pty thread can survive is decided here and nowhere else:
/// anything this does not recognise reaches `.fatal()` and takes the session —
/// every local pane in it included — down with the pane that could not start.
#[test]
fn a_pane_that_could_not_start_names_its_terminal_and_its_reason() {
    let not_found: anyhow::Error = YinzError::CommandNotFound {
        terminal_id: yinz_types::internal_ids::TerminalId::new(3),
        command: "htop".to_owned(),
    }
    .into();
    let (terminal_id, message) =
        pane_start_failure(&not_found).expect("a missing command must stay survivable");
    assert_eq!(terminal_id, crate::TerminalId::new(3));
    assert!(
        message.contains("htop"),
        "the pane should say what was missing, got: {message}"
    );

    let unreachable: anyhow::Error = YinzError::PaneStartFailed {
        terminal_id: yinz_types::internal_ids::TerminalId::new(4),
        message: "could not open a pane on build-box: the broker closed the connection".to_owned(),
    }
    .into();
    let (terminal_id, message) =
        pane_start_failure(&unreachable).expect("an unreachable host must not end the session");
    assert_eq!(terminal_id, crate::TerminalId::new(4));
    assert!(
        message.contains("build-box"),
        "the pane should name the machine, got: {message}"
    );

    // Context wrapping is what the error picks up on its way through
    // `spawn_terminal`, so the match has to survive it.
    let wrapped = unreachable.context("failed to apply run instruction");
    assert!(
        pane_start_failure(&wrapped).is_some(),
        "a wrapped failure is the same failure"
    );

    assert!(
        pane_start_failure(&anyhow!("the pty thread has no pane to blame this on")).is_none(),
        "an unrelated failure must stay fatal"
    );
}

/// The pane a warp test moves: enough bookkeeping for the release to have
/// something to take apart, and a reader task standing in for the real one so a
/// test can see whether it survived.
fn set_warpable_terminal(pty: &mut Pty, terminal_id: TerminalId) {
    pty.terminal_cmds
        .insert(terminal_id, vec!["/bin/sh".to_owned()]);
    pty.id_to_child_pid
        .insert(terminal_id, ProcessId::new(100 + terminal_id.get()));
    pty.task_handles
        .insert(terminal_id, async_runtime().spawn(async {}));
}

fn drain_screen(rx: &ScreenReceiver) -> Vec<ScreenInstruction> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(instruction, _)| instruction)
        .collect()
}

/// What a pane was told about itself, by terminal id.
fn pane_messages(instructions: &[ScreenInstruction]) -> HashMap<TerminalId, String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PtyBytes(terminal_id, bytes) => {
                Some((*terminal_id, String::from_utf8_lossy(bytes).to_string()))
            },
            _ => None,
        })
        .collect()
}

/// A deadline no test will reach, so a release under test is never cut short by
/// one. What the deadline *does* to a release is exercised on its own, by the
/// tests that pass one already in the past.
fn test_warp_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(30)
}

#[test]
fn a_pane_that_cannot_be_warped_is_refused_before_its_reader_is_touched() {
    // The invariant the whole feature rests on. Aborting the reader closes this
    // server's dup of the pty master and there is no second copy to reopen, so a
    // refusal arriving after it would cost the user a working pane. Asked first,
    // it costs an error message.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock.clone());
    set_warpable_terminal(&mut pty, crate::TerminalId::new(3));
    mock.refuse_to_warp(
        crate::TerminalId::new(3),
        "pane 3 has no pty-host supervisor",
    );

    let error = format!(
        "{:#}",
        pty.release_terminal_for_warp(crate::TerminalId::new(3), test_warp_deadline())
            .expect_err("an unwarpable pane must be refused")
    );

    assert!(
        error.contains("no pty-host supervisor"),
        "the refusal should reach the caller intact, got: {error}"
    );
    assert!(
        mock.release_attempts().is_empty(),
        "nothing should have been released"
    );
    assert!(
        pty.task_handles.contains_key(&TerminalId::new(3)),
        "the pane's reader must still be running — this is the whole point of asking first"
    );
    assert!(
        pty.terminal_cmds.contains_key(&TerminalId::new(3)),
        "a refused pane keeps its bookkeeping"
    );
}

#[test]
fn a_refused_pane_is_not_told_it_was_cut_loose() {
    // The other half of the same rule: a refusal costs the user an error and
    // nothing else, so the pane must not be held and must not have a failure
    // written into its scrollback.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    set_warpable_terminal(&mut pty, crate::TerminalId::new(3));
    mock.refuse_to_warp(
        crate::TerminalId::new(3),
        "pane 3 has no pty-host supervisor",
    );

    let released = release_pane_for_warp(
        &mut pty,
        crate::TerminalId::new(3),
        &yinz_types::internal_ids::SessionName::new("elsewhere"),
        test_warp_deadline(),
    );

    assert!(released.is_err(), "the warp should have been refused");
    let instructions = drain_screen(&screen_rx);
    assert!(
        !instructions
            .iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::HoldPane(..))),
        "a pane that was never released is still working and must not be held"
    );
    assert!(
        pane_messages(&instructions).is_empty(),
        "nothing happened to this pane, so nothing should be written into it"
    );
}

#[test]
fn a_release_that_fails_after_the_check_says_so_in_the_pane() {
    // The residual case: a supervisor that agrees to be asked and then does not
    // confirm. The pane cannot be put back, so the one thing left to get right
    // is that it stops looking alive.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    set_warpable_terminal(&mut pty, crate::TerminalId::new(3));
    mock.fail_the_release_of(crate::TerminalId::new(3));

    let released = release_pane_for_warp(
        &mut pty,
        crate::TerminalId::new(3),
        &yinz_types::internal_ids::SessionName::new("elsewhere"),
        test_warp_deadline(),
    );

    assert!(released.is_err(), "the release should have failed");
    let instructions = drain_screen(&screen_rx);
    let message = pane_messages(&instructions)
        .remove(&TerminalId::new(3))
        .expect("the pane should have been told what happened to it");
    assert!(
        message.contains("could not move this pane to 'elsewhere'"),
        "the pane's own account should name the session it was going to, got: {message}"
    );
    assert!(
        instructions.iter().any(|instruction| matches!(
            instruction,
            ScreenInstruction::HoldPane(PaneId::Terminal(TERMINAL_3), ..)
        )),
        "a pane with nothing on the other end must be held rather than left looking live"
    );
}

#[test]
fn a_tabs_panes_are_released_together_rather_than_one_after_another() {
    // The bug this is here for: a tab with two remote panes could not be warped
    // at all. Each remote release is a round trip to a broker over ssh, they
    // were done in turn, and two of them spent the whole of `WARP_TIMEOUT`
    // before the first word came back — so the caller reported "timed out
    // preparing the tab to be moved" while this thread was still waiting on
    // pane one.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock.clone());
    let terminal_ids = vec![
        TerminalId::new(1),
        TerminalId::new(2),
        TerminalId::new(3),
        TerminalId::new(4),
    ];
    for terminal_id in &terminal_ids {
        set_warpable_terminal(&mut pty, *terminal_id);
    }
    let each = std::time::Duration::from_millis(300);
    mock.make_each_release_take(each);

    let started_at = std::time::Instant::now();
    let released = release_tab_for_warp(&mut pty, terminal_ids.clone(), test_warp_deadline());
    let took = started_at.elapsed();

    assert!(
        released.iter().all(|(_, result)| result.is_ok()),
        "every pane should have been released: {released:?}"
    );
    assert_eq!(
        mock.release_attempts().len(),
        terminal_ids.len(),
        "every pane is still released exactly once"
    );
    // Four panes at 300ms each is 1.2s in turn and one 300ms round trip
    // together. Halfway between the two is a threshold no amount of scheduler
    // noise crosses, and it fails outright on the sequential version.
    assert!(
        took < each * 2,
        "a tab's releases must overlap: four panes at {each:?} each took {took:?}"
    );
}

#[test]
fn a_release_gets_what_is_left_of_the_callers_deadline_and_no_more() {
    // The budget used to be a constant with no relation to how long the caller
    // was actually prepared to wait, which is how a release could still be
    // running — and then hand a tab to nobody — after the caller had given up.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock.clone());
    for terminal_id in [1, 2] {
        set_warpable_terminal(&mut pty, crate::TerminalId::new(terminal_id));
    }
    let remaining = std::time::Duration::from_millis(400);

    release_tab_for_warp(
        &mut pty,
        vec![crate::TerminalId::new(1), TerminalId::new(2)],
        std::time::Instant::now() + remaining,
    );

    let budgets = mock.release_budgets();
    assert_eq!(budgets.len(), 2, "both panes were released");
    for budget in budgets {
        assert!(
            budget <= remaining,
            "a release may not be given longer than the caller will wait: {budget:?} > \
             {remaining:?}"
        );
    }
}

/// A deadline the caller has already reached. What a release does with one is
/// the whole question, so no sleeping is needed to ask it.
fn expired_warp_deadline() -> std::time::Instant {
    std::time::Instant::now() - std::time::Duration::from_secs(1)
}

#[test]
fn a_pane_whose_caller_has_already_given_up_is_refused_before_its_reader_is_touched() {
    // Screen sends a deadline that has already passed when the caller left it
    // nothing to spend, and the pty thread can dequeue late on its own. Either
    // way, that used to reach the backend as a budget of zero — *after* the
    // reader was aborted — so a remote pane's socket refused the bound and the
    // pane was stranded to answer a caller who was gone. Asked first, the pane
    // stays exactly as it was.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    set_warpable_terminal(&mut pty, crate::TerminalId::new(3));

    let error = format!(
        "{:#}",
        pty.release_terminal_for_warp(crate::TerminalId::new(3), expired_warp_deadline())
            .expect_err("a release nobody is waiting for must be refused")
    );

    assert!(
        error.contains("stopped waiting"),
        "the refusal should say why, got: {error}"
    );
    assert!(
        mock.release_attempts().is_empty(),
        "nothing should have been released"
    );
    assert!(
        pty.task_handles.contains_key(&TerminalId::new(3)),
        "the pane's reader must still be running — refusing after the abort is what stranded it"
    );
    assert!(
        pane_messages(&drain_screen(&screen_rx)).is_empty(),
        "a pane that was never touched has nothing to tell the user"
    );
}

#[test]
fn a_tab_whose_caller_has_already_given_up_is_refused_while_it_is_still_whole() {
    // The tab-sized version, and the one with more at stake: a tab release
    // stops every reader before it asks any backend, so a zero budget found
    // afterwards stranded every pane in the tab at once.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    for terminal_id in [1, 2] {
        set_warpable_terminal(&mut pty, crate::TerminalId::new(terminal_id));
    }

    let released = release_tab_for_warp(
        &mut pty,
        vec![crate::TerminalId::new(1), TerminalId::new(2)],
        expired_warp_deadline(),
    );

    assert_eq!(released.len(), 2, "every pane is answered");
    for (terminal_id, result) in &released {
        let error = result
            .as_ref()
            .expect_err("a release nobody is waiting for must be refused");
        assert!(
            error.contains("stopped waiting"),
            "pane {terminal_id}'s refusal should say why, got: {error}"
        );
        assert!(
            pty.task_handles.contains_key(terminal_id),
            "pane {terminal_id}'s reader must still be running"
        );
    }
    assert!(
        mock.release_attempts().is_empty(),
        "nothing should have been released"
    );
    assert!(
        pane_messages(&drain_screen(&screen_rx)).is_empty(),
        "panes that were never touched have nothing to tell the user"
    );
}

#[test]
fn one_unwarpable_pane_stops_a_whole_tab_being_released() {
    // Releasing panes one at a time and stopping at the first refusal would
    // answer "no" with the panes ahead of it already handed away and nothing to
    // give them back to.
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    for terminal_id in [1, 2, 3] {
        set_warpable_terminal(&mut pty, crate::TerminalId::new(terminal_id));
    }
    mock.refuse_to_warp(
        crate::TerminalId::new(3),
        "pane 3 has no pty-host supervisor",
    );

    let released = release_tab_for_warp(
        &mut pty,
        vec![
            crate::TerminalId::new(1),
            crate::TerminalId::new(2),
            crate::TerminalId::new(3),
        ],
        test_warp_deadline(),
    );

    assert!(
        mock.release_attempts().is_empty(),
        "not one pane of a tab that cannot be moved should have been released, got: {:?}",
        mock.release_attempts()
    );
    assert_eq!(
        released.len(),
        3,
        "screen hears about every pane it asked about"
    );
    assert!(
        released.iter().all(|(_, result)| result.is_err()),
        "the whole tab is refused for the one pane that cannot go"
    );
    assert!(
        !drain_screen(&screen_rx)
            .iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::HoldPane(..))),
        "a tab that was never released is still working and must not be held"
    );
}

#[test]
fn a_tab_release_that_fails_part_way_tells_every_pane_in_it() {
    // Pane 1 was handed over before pane 3 failed, so it is orphaned and pane 3
    // is severed. Neither has a process on the other end any more, and both have
    // to say so — including the one whose own release "worked".
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    for terminal_id in [1, 3] {
        set_warpable_terminal(&mut pty, crate::TerminalId::new(terminal_id));
    }
    mock.fail_the_release_of(crate::TerminalId::new(3));

    release_tab_for_warp(
        &mut pty,
        vec![crate::TerminalId::new(1), TerminalId::new(3)],
        test_warp_deadline(),
    );

    let messages = pane_messages(&drain_screen(&screen_rx));
    let orphaned = messages
        .get(&TerminalId::new(1))
        .expect("the pane handed over before the failure must say so");
    assert!(
        orphaned.contains("had already been handed over")
            && orphaned.contains("yinz pty-host adopt host-1"),
        "an orphaned pane's only way back is the recovery command, got: {orphaned}"
    );
    assert!(
        messages
            .get(&TerminalId::new(3))
            .is_some_and(|message| message.contains("this tab could not be moved")),
        "the pane whose release failed must say so too, got: {messages:?}"
    );
}

// A pane workflow's steps each carry their own context, and `rerun_command_in_pane` is where a
// step's context either survives to its `CommandPaneExited` or is clobbered by the pane's
// original one. The rule: an incoming `originating_plugin` wins and becomes the pane's record;
// the stored record is only a fallback for a command that carries none (a user's ENTER re-run).

fn step_origin(step: &str) -> yinz_types::data::OriginatingPlugin {
    yinz_types::data::OriginatingPlugin::new(
        yinz_types::internal_ids::PluginId::new(2),
        crate::ClientId::new(1),
        std::collections::BTreeMap::from([("__step".to_owned(), step.to_owned())]),
    )
}

fn collect_command_pane_rerun_steps(rx: &PluginReceiver) -> Vec<String> {
    let mut steps = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                if let Event::CommandPaneReRun(_, context) = event {
                    steps.push(context.get("__step").cloned().unwrap_or_default());
                }
            }
        }
    }
    steps
}

#[test]
fn a_respawn_commands_own_context_wins_over_the_panes_original() {
    let mock = MockOsApi::new();
    let (mut pty, plugin_rx) = make_pty_with_plugin_receiver(mock.clone());
    pty.originating_plugins
        .insert(crate::TerminalId::new(1), step_origin("1"));

    let replacement = RunCommand {
        command: PathBuf::from("step2"),
        hold_on_close: true,
        originating_plugin: Some(step_origin("2")),
        ..Default::default()
    };
    pty.rerun_command_in_pane(PaneId::Terminal(crate::TerminalId::new(1)), replacement)
        .unwrap();

    let ran = mock.rerun_commands();
    assert_eq!(
        ran[0]
            .originating_plugin
            .as_ref()
            .and_then(|origin| origin.context.get("__step"))
            .map(String::as_str),
        Some("2"),
        "the spawned command keeps the context it came with"
    );
    assert_eq!(
        pty.originating_plugins
            .get(&TerminalId::new(1))
            .and_then(|origin| origin.context.get("__step"))
            .map(String::as_str),
        Some("2"),
        "the pane's record is updated, so later re-runs answer with the newest context"
    );
    assert_eq!(
        collect_command_pane_rerun_steps(&plugin_rx),
        vec!["2".to_owned()],
        "the re-run event names the step that is actually starting"
    );
}

#[test]
fn a_respawn_without_a_context_falls_back_to_the_panes_original() {
    let mock = MockOsApi::new();
    let (mut pty, plugin_rx) = make_pty_with_plugin_receiver(mock.clone());
    pty.originating_plugins
        .insert(crate::TerminalId::new(1), step_origin("1"));

    let same_command_again = RunCommand {
        command: PathBuf::from("step1"),
        hold_on_close: true,
        ..Default::default()
    };
    pty.rerun_command_in_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        same_command_again,
    )
    .unwrap();

    assert_eq!(
        mock.rerun_commands()[0]
            .originating_plugin
            .as_ref()
            .and_then(|origin| origin.context.get("__step"))
            .map(String::as_str),
        Some("1"),
        "a command carrying no origin inherits the pane's stored one"
    );
    assert_eq!(
        collect_command_pane_rerun_steps(&plugin_rx),
        vec!["1".to_owned()]
    );
}

/// A layout whose second local pane fails to start: shared by the new-tab and
/// the override path, so the two can be held to the same cleanup.
fn failing_three_slot_layout() -> TiledPaneLayout {
    let local = || RunCommand {
        command: PathBuf::from("sh"),
        ..Default::default()
    };
    TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                run: Some(Run::Command(local())),
                ..Default::default()
            },
            TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    target: PaneTarget::ssh("build-box").unwrap(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            TiledPaneLayout {
                run: Some(Run::Command(local())),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// Every event the plugins were sent, in order.
fn collect_plugin_events(rx: &PluginReceiver) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            events.extend(updates.into_iter().map(|(_, _, event)| event));
        }
    }
    events
}

/// A pane spawned to take over another one, as `SpawnInPlaceTerminal` asks.
fn in_place_spawn(
    run_command: Option<RunCommand>,
    open_file_payload: Option<OpenFilePayload>,
) -> PendingPaneSpawn {
    PendingPaneSpawn {
        run_command,
        open_file_payload,
        pane_title: None,
        invoked_with: None,
        client_or_tab_index: ClientTabIndexOrPaneId::ClientId(crate::ClientId::new(1)),
        completion_tx: None,
        placement: PanePlacement::InPlace {
            replaced: ReplacedPane::Close,
        },
    }
}

#[test]
fn originating_pane_replies_preserve_generation_on_open_rerun_exit_and_failure() {
    for generation in [None, Some(42)] {
        let (mut pty, plugin_rx, _screen_rx) = make_pty_with_screen_receiver(MockOsApi::new());
        let mut origin = step_origin("worker");
        origin.generation = generation;
        let command = RunCommand {
            hold_on_close: true,
            originating_plugin: Some(origin.clone()),
            ..Default::default()
        };
        let terminal = TerminalId::new(41);
        let pane = PaneId::Terminal(terminal);
        pty.announce_pane_opened(terminal, Some(&command), None)
            .unwrap();
        // A user rerun with no new origin inherits the complete stored identity.
        let (rerun, _) = pty
            .request_rerun(pane, RunCommand::default(), None)
            .unwrap()
            .unwrap();
        assert_eq!(rerun.originating_plugin, Some(origin.clone()));
        pty.rerun_command_in_pane(pane, rerun).unwrap();
        let callback = terminal_quit_callback(
            pty.bus.senders.clone(),
            &TerminalAction::RunCommand(command.clone()),
        );
        callback(pane, Some(0), command.clone());
        finish_pane_spawn(
            &mut pty,
            in_place_spawn(Some(command.clone()), None),
            Err(YinzError::CommandNotFound {
                terminal_id: terminal,
                command: "missing".into(),
            }
            .into()),
        )
        .unwrap();
        finish_command_rerun(
            &mut pty,
            pane,
            command,
            Err(YinzError::CommandNotFound {
                terminal_id: terminal,
                command: "missing".into(),
            }
            .into()),
        )
        .unwrap();
        let payload = OpenFilePayload::new(PathBuf::from("notes.md"), None, None)
            .with_originating_plugin(origin.clone());
        pty.announce_pane_opened(terminal, None, Some(&payload))
            .unwrap();
        terminal_quit_callback(pty.bus.senders.clone(), &TerminalAction::OpenFile(payload))(
            pane,
            Some(1),
            RunCommand::default(),
        );
        let expected = [
            Event::CommandPaneOpened(terminal, origin.context.clone()),
            Event::CommandPaneReRun(terminal, origin.context.clone()),
            Event::CommandPaneExited(terminal, Some(0), origin.context.clone()),
            Event::CommandPaneExited(terminal, Some(2), origin.context.clone()),
            Event::CommandPaneExited(terminal, Some(2), origin.context.clone()),
            Event::EditPaneOpened(terminal, origin.context.clone()),
            Event::EditPaneExited(terminal, Some(1), origin.context.clone()),
        ];
        for expected in expected {
            let (instruction, _) = plugin_rx.try_recv().unwrap();
            match (generation, instruction) {
                (
                    Some(generation),
                    PluginInstruction::PluginEvent(plugin, client, actual_generation, event),
                ) => {
                    assert_eq!(
                        (plugin, client, actual_generation),
                        (origin.plugin_id, origin.client_id, generation)
                    );
                    assert_eq!(event, expected);
                },
                (None, PluginInstruction::Update(updates)) => assert_eq!(
                    updates,
                    vec![(Some(origin.plugin_id), Some(origin.client_id), expected)]
                ),
                (_, other) => panic!("unexpected pane reply: {other:?}"),
            }
        }
        assert!(plugin_rx.try_recv().is_err());
    }
}

#[test]
fn an_in_place_command_pane_tells_its_plugin_it_opened() {
    let (mut pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(MockOsApi::new());
    let command = RunCommand {
        command: PathBuf::from("htop"),
        originating_plugin: Some(step_origin("x")),
        ..Default::default()
    };
    finish_pane_spawn(
        &mut pty,
        in_place_spawn(Some(command), None),
        Ok((TerminalId::new(0), false)),
    )
    .unwrap();

    let events = collect_plugin_events(&plugin_rx);
    assert!(
        matches!(
            &events[..],
            [Event::CommandPaneOpened(id, context)]
                if *id == TerminalId::new(0) && context.get("__step").map(String::as_str) == Some("x")
        ),
        "the plugin that asked for the pane is told it opened, got: {events:?}"
    );
    assert_eq!(
        pty.originating_plugins
            .get(&TerminalId::new(0))
            .and_then(|plugin| plugin.context.get("__step").cloned()),
        Some("x".to_owned()),
        "the pane is registered as the plugin's, so a rerun keeps its context"
    );
    let screen = drain_screen(&screen_rx);
    assert!(
        matches!(
            &screen[..],
            [ScreenInstruction::ReplacePane(PaneId::Terminal(id), None, _, _, ReplacedPane::Close, _, _)]
                if *id == TerminalId::new(0)
        ),
        "the pane takes over the one it was asked to, got: {screen:?}"
    );
}

#[test]
fn an_in_place_edit_pane_tells_its_plugin_it_opened() {
    let (mut pty, plugin_rx, _screen_rx) = make_pty_with_screen_receiver(MockOsApi::new());
    let payload = OpenFilePayload::new(PathBuf::from("notes.md"), None, None)
        .with_originating_plugin(step_origin("e"));
    finish_pane_spawn(
        &mut pty,
        in_place_spawn(None, Some(payload)),
        Ok((TerminalId::new(0), false)),
    )
    .unwrap();

    let events = collect_plugin_events(&plugin_rx);
    assert!(
        matches!(
            &events[..],
            [Event::EditPaneOpened(id, context)]
                if *id == TerminalId::new(0) && context.get("__step").map(String::as_str) == Some("e")
        ),
        "the plugin that asked for the editor is told it opened, got: {events:?}"
    );
    assert!(
        pty.originating_plugins.is_empty(),
        "an edit pane is not a command pane to rerun"
    );
}

#[test]
fn an_in_place_command_pane_that_cannot_start_tells_its_plugin_it_exited() {
    let not_found = || -> anyhow::Error {
        YinzError::CommandNotFound {
            terminal_id: TerminalId::new(41),
            command: "htop".to_owned(),
        }
        .into()
    };
    let shown = RunCommand {
        command: PathBuf::from("htop"),
        hold_on_close: true,
        originating_plugin: Some(step_origin("x")),
        ..Default::default()
    };

    let os_api = MockOsApi::new();
    let (mut pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    finish_pane_spawn(
        &mut pty,
        in_place_spawn(Some(shown.clone()), None),
        Err(not_found()),
    )
    .unwrap();

    let screen = drain_screen(&screen_rx);
    assert!(
        matches!(
            &screen[..],
            [
                ScreenInstruction::ReplacePane(PaneId::Terminal(placeholder), None, ..),
                ScreenInstruction::PtyBytes(reason_for, reason),
                ScreenInstruction::HoldPane(PaneId::Terminal(held), Some(2), _),
            ] if *placeholder == TerminalId::new(41)
                && *reason_for == TerminalId::new(41)
                && *held == TerminalId::new(41)
                && String::from_utf8_lossy(reason).contains("htop")
        ),
        "the pane is put up, told why, and held, got: {screen:?}"
    );
    let events = collect_plugin_events(&plugin_rx);
    assert!(
        matches!(&events[..], [Event::CommandPaneExited(id, Some(2), _)] if *id == TerminalId::new(41)),
        "the plugin is told the command exited, got: {events:?}"
    );
    assert!(
        os_api.cleared_terminal_ids.lock().unwrap().is_empty(),
        "the pane stays up to show the reason"
    );

    // Without `hold_on_close` nothing shows the failure: the reserved pane is
    // released and nobody is told.
    let silent = RunCommand {
        hold_on_close: false,
        ..shown
    };
    let os_api = MockOsApi::new();
    let (mut pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    finish_pane_spawn(
        &mut pty,
        in_place_spawn(Some(silent), None),
        Err(not_found()),
    )
    .unwrap();
    assert_eq!(
        *os_api.cleared_terminal_ids.lock().unwrap(),
        vec![TerminalId::new(41)]
    );
    let screen = drain_screen(&screen_rx);
    assert!(
        !screen.iter().any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::ReplacePane(..)
                    | ScreenInstruction::PtyBytes(..)
                    | ScreenInstruction::HoldPane(..)
            )
        }),
        "nothing shows the failure; only the closed pane's bookkeeping remains, got: {screen:?}"
    );
    assert!(collect_plugin_events(&plugin_rx).is_empty());
}

#[test]
fn a_layout_marks_all_adopted_terminals_even_without_agent_records() {
    for state in ["live", "held", "exited"] {
        let os_api = MockOsApi::new();
        let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
        let command = |name: &str| RunCommand {
            command: PathBuf::from(name),
            args: vec!["--resume".into()],
            cwd: Some(PathBuf::from("/work")),
            hold_on_close: true,
            ..Default::default()
        };
        let tiled_command = command("codex");
        let floating_command = command("claude");
        let adopted = |terminal_id, cmd: RunCommand| AdoptedTabTerminal {
            agent_input: None,
            terminal_id,
            held: (state == "held").then(|| yinz_types::data::HeldTerminal {
                command: cmd.clone(),
                first_run: true,
                exit_status: None,
            }),
            cmd,
            agent: None,
            child_exit_status: (state == "exited").then_some(Some(0)),
            replay: b"replayed screen".to_vec(),
            caption: None,
        };
        pty.adopted_tabs.insert(
            TabId::new(7),
            (
                vec![adopted(TERMINAL_3, tiled_command.clone())],
                vec![adopted(TERMINAL_41, floating_command.clone())],
            ),
        );

        pty.spawn_terminals_for_layout(
            None,
            TiledPaneLayout::default(),
            vec![FloatingPaneLayout::default()],
            Some(TerminalAction::RunCommand(RunCommand::default())),
            HashMap::new(),
            None,
            TabId::new(7),
            false,
            true,
            CLIENT_1,
            None,
        )
        .unwrap();

        let instructions = drain_screen(&screen_rx);
        if state == "live" {
            assert!(matches!(
                instructions.as_slice(),
                [
                    ScreenInstruction::ApplyLayout(..),
                    ScreenInstruction::PtyBytes(TERMINAL_3, _),
                    ScreenInstruction::RequestPaneRedraw(TERMINAL_3),
                    ScreenInstruction::PtyBytes(TERMINAL_41, _),
                    ScreenInstruction::RequestPaneRedraw(TERMINAL_41),
                ]
            ));
        } else {
            assert!(!instructions
                .iter()
                .any(|instruction| matches!(instruction, ScreenInstruction::RequestPaneRedraw(_))));
        }
        let (tiled, floating, adopted) = instructions
            .into_iter()
            .find_map(|instruction| match instruction {
                ScreenInstruction::ApplyLayout(_, _, tiled, floating, adopted, ..) => {
                    Some((tiled, floating, adopted))
                },
                _ => None,
            })
            .expect("adopted terminals are handed to Screen");
        assert_eq!(
            tiled,
            vec![(TERMINAL_3, (state != "live").then(|| tiled_command.clone()))]
        );
        assert_eq!(
            floating,
            vec![(
                TERMINAL_41,
                (state != "live").then(|| floating_command.clone())
            )]
        );
        assert_eq!(
            adopted,
            vec![
                (TERMINAL_3, tiled_command, None),
                (TERMINAL_41, floating_command, None),
            ]
        );
        assert!(os_api.cleared_terminal_ids.lock().unwrap().is_empty());
    }
}

/// The two layout paths spawn the same panes for the same layout.
#[test]
fn a_layout_override_places_the_same_panes_as_a_new_tab() {
    let local = || RunCommand {
        command: PathBuf::from("sh"),
        ..Default::default()
    };
    let held = || RunCommand {
        command: PathBuf::from("make"),
        hold_on_start: true,
        ..Default::default()
    };
    let layout = || TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                run: Some(Run::Command(local())),
                ..Default::default()
            },
            TiledPaneLayout {
                run: Some(Run::Command(held())),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let floating = || {
        vec![FloatingPaneLayout {
            run: Some(Run::Command(local())),
            ..Default::default()
        }]
    };
    let default_shell = Some(TerminalAction::RunCommand(RunCommand::default()));

    let os_api = MockOsApi::new();
    *os_api.local_spawns_to_allow.lock().unwrap() = 2;
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    pty.spawn_terminals_for_layout(
        None,
        layout(),
        floating(),
        default_shell.clone(),
        HashMap::new(),
        None,
        TabId::new(0),
        false,
        true,
        crate::ClientId::new(1),
        None,
    )
    .unwrap();
    let new_tab = drain_screen(&screen_rx)
        .into_iter()
        .find_map(|instruction| match instruction {
            ScreenInstruction::ApplyLayout(_, _, tiled, floating, ..) => Some((tiled, floating)),
            _ => None,
        })
        .expect("a new tab is applied");
    let new_tab_cleared = os_api.cleared_terminal_ids.lock().unwrap().clone();
    let new_tab_readers = pty.task_handles.len();

    let os_api = MockOsApi::new();
    *os_api.local_spawns_to_allow.lock().unwrap() = 2;
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (tab_result, deferred) = pty
        .spawn_terminals_for_layout_override(
            None,
            layout(),
            floating(),
            default_shell,
            HashMap::new(),
            TabId::new(0),
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        (
            tab_result.new_terminal_pids,
            tab_result.new_floating_pane_pids
        ),
        new_tab,
        "the override places exactly what a new tab would"
    );
    assert_eq!(
        new_tab.0[1]
            .1
            .as_ref()
            .map(|command| command.command.clone()),
        Some(PathBuf::from("make")),
        "the held slot carries its command"
    );
    assert!(deferred.is_empty());
    assert_eq!(
        *os_api.cleared_terminal_ids.lock().unwrap(),
        new_tab_cleared
    );
    assert_eq!(pty.task_handles.len(), new_tab_readers);
}

/// The two layout paths give up on a failing layout the same way.
#[test]
fn a_layout_override_and_a_new_tab_abandon_the_same_panes() {
    let run = |override_layout: bool| {
        let os_api = MockOsApi::new();
        let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
        let (pty_tx, _pty_rx) = channels::unbounded();
        pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        *os_api.local_spawns_to_allow.lock().unwrap() = 1;
        *os_api.fail_local_spawns.lock().unwrap() = true;
        let default_shell = Some(TerminalAction::RunCommand(RunCommand::default()));
        let failed = if override_layout {
            pty.spawn_terminals_for_layout_override(
                None,
                failing_three_slot_layout(),
                vec![],
                default_shell,
                HashMap::new(),
                TabId::new(0),
                None,
                crate::ClientId::new(1),
                None,
                None,
            )
            .is_err()
        } else {
            pty.spawn_terminals_for_layout(
                None,
                failing_three_slot_layout(),
                vec![],
                default_shell,
                HashMap::new(),
                None,
                TabId::new(0),
                false,
                true,
                crate::ClientId::new(1),
                None,
            )
            .is_err()
        };
        assert!(failed, "the local failure is reported on both paths");
        let mut cleared = os_api.cleared_terminal_ids.lock().unwrap().clone();
        cleared.sort_unstable();
        (cleared, os_api.local_signals())
    };
    let new_tab = run(false);
    let override_layout = run(true);
    assert_eq!(new_tab, override_layout);
    assert_eq!(new_tab.0, vec![TerminalId::new(0), TerminalId::new(1)]);
}

#[test]
fn warp_held_release_drains_final_output_before_acknowledging() {
    for batch in [false, true] {
        let mock = MockOsApi::new();
        let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
        let live = TerminalId::new(3);
        let held_id = TerminalId::new(8);
        set_warpable_terminal(&mut pty, live);
        let held = yinz_types::data::HeldTerminal {
            command: RunCommand::default(),
            first_run: false,
            exit_status: Some(0),
        };
        let (ready_tx, ready_rx) = crossbeam::channel::bounded(1);
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let senders = pty.bus.senders.clone();
        let reader = async_runtime().spawn(async move {
            ready_tx.send(()).unwrap();
            finish_rx.await.unwrap();
            senders
                .send_to_screen(ScreenInstruction::PtyBytes(
                    held_id,
                    b"final output".to_vec(),
                ))
                .unwrap();
        });
        let reader_abort = reader.abort_handle();
        pty.task_handles.insert(held_id, reader);
        ready_rx
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();

        let (to_pty, pty_rx) = channels::unbounded();
        let to_pty = SenderWithContext::new(to_pty);
        let mut bus = Bus::new(
            vec![pty_rx],
            None,
            None,
            None,
            None,
            None,
            None,
            pty.bus.os_input.take(),
        );
        bus.senders = pty.bus.senders.clone();
        pty.bus = bus;
        let thread = std::thread::spawn(move || pty_thread_main(pty, Box::default()).unwrap());
        let request = |deadline| {
            if batch {
                PtyInstruction::ReleaseTerminalsForWarp {
                    tab_index: TabId::new(0),
                    terminal_ids: vec![live, held_id],
                    held: HashMap::from([(held_id, held.clone())]),
                    deadline,
                }
            } else {
                PtyInstruction::ReleaseTerminalForWarp {
                    terminal_id: held_id,
                    held: Some(held.clone()),
                    to_session: "elsewhere".into(),
                    placement: yinz_types::data::WarpPlacement::FocusedTab,
                    client_id: CLIENT_1,
                    deadline,
                }
            }
        };
        // Keep the reader gated through two attempts. A second refusal proves the
        // first timeout retained its handle rather than detaching the unfinished task.
        for _ in 0..2 {
            to_pty
                .send(request(Instant::now() + Duration::from_secs(1)))
                .unwrap();
            let (reply, _) = screen_rx
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            let results = match reply {
                ScreenInstruction::PaneReleasedForWarp { released, .. } => {
                    vec![(held_id, released)]
                },
                ScreenInstruction::TabReleasedForWarp { released, .. } => released,
                other => panic!("unexpected instruction: {other:?}"),
            };
            assert!(results
                .iter()
                .all(|(_, r)| r.as_ref().unwrap_err().contains("output is still draining")));
            assert!(
                !reader_abort.is_finished(),
                "timeout must not abort the reader"
            );
            assert!(
                mock.release_attempts().is_empty(),
                "live peers must remain attached"
            );
        }
        to_pty.send(request(test_warp_deadline())).unwrap();
        finish_tx.send(()).unwrap();
        let (bytes, _) = screen_rx
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        assert!(matches!(bytes, ScreenInstruction::PtyBytes(id, bytes)
            if id == held_id && bytes == b"final output"));
        let reply = loop {
            let (instruction, _) = screen_rx
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            if !matches!(instruction, ScreenInstruction::UpdatePaneHost(..)) {
                break instruction;
            }
        };
        match reply {
            ScreenInstruction::PaneReleasedForWarp { released, .. } => assert!(released.is_ok()),
            ScreenInstruction::TabReleasedForWarp { released, .. } => {
                assert!(released.iter().all(|(_, r)| r.is_ok()));
            },
            other => panic!("unexpected instruction: {other:?}"),
        }
        assert!(reader_abort.is_finished());
        assert_eq!(
            mock.release_attempts(),
            if batch { vec![live] } else { vec![] }
        );
        to_pty.send(PtyInstruction::Exit).unwrap();
        thread.join().unwrap();
    }
}

#[test]
fn warp_held_reader_joins_remove_completed_handles_and_refuse_failed_tasks() {
    for failure in ["none", "cancelled", "panicked"] {
        let mock = MockOsApi::new();
        let (mut pty, _, _) = make_pty_with_screen_receiver(mock.clone());
        let held_id = TerminalId::new(8);
        let reader = async_runtime().spawn(async move {
            match failure {
                "cancelled" => std::future::pending::<()>().await,
                "panicked" => panic!("injected reader failure"),
                _ => {},
            }
        });
        if failure == "cancelled" {
            reader.abort();
        }
        pty.task_handles.insert(held_id, reader);
        let result = pty.drain_held_reader_for_warp(held_id, test_warp_deadline());
        if failure == "none" {
            result.unwrap();
            pty.drain_held_reader_for_warp(held_id, test_warp_deadline())
                .unwrap();
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("output reader failed"));
        }
        assert!(!pty.task_handles.contains_key(&held_id));
        assert!(mock.release_attempts().is_empty());
    }
}

#[test]
fn warp_revalidates_pending_restarts_before_releasing_any_pane() {
    let mock = MockOsApi::new();
    let (mut pty, _, _) = make_pty_with_screen_receiver(mock.clone());
    let live = TerminalId::new(3);
    let held_id = TerminalId::new(8);
    set_warpable_terminal(&mut pty, live);
    pty.reruns.insert(
        held_id,
        RerunState {
            generation: 1,
            queued: VecDeque::new(),
        },
    );
    let held = yinz_types::data::HeldTerminal {
        command: RunCommand::default(),
        first_run: true,
        exit_status: None,
    };
    let released = release_tab_with_held_for_warp(
        &mut pty,
        vec![live, held_id],
        HashMap::from([(held_id, held)]),
        test_warp_deadline(),
    );
    assert!(released.iter().all(|(_, result)| result
        .as_ref()
        .unwrap_err()
        .contains("terminal_8 is restarting")));
    assert!(mock.release_attempts().is_empty());
    assert!(pty.task_handles.contains_key(&live));
}

#[test]
fn warp_adoption_failure_cleans_up_held_reservations() {
    let mock = MockOsApi::new();
    let (mut pty, _, _) = make_pty_with_screen_receiver(mock.clone());
    let held = yinz_types::data::HeldTerminal {
        command: RunCommand::default(),
        first_run: true,
        exit_status: None,
    };
    let payload = WarpedTab {
        tiled_panes: vec![
            WarpedPane {
                agent_theme: None,
                held: Some(held.clone()),
                ..Default::default()
            },
            // An invalid handoff fails after the first reservation was acquired.
            WarpedPane {
                agent_theme: None,
                held: Some(held),
                control_socket: "/invalid".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(pty.adopt_tab(&payload).is_err());
    assert_eq!(mock.cleared_terminal_ids.lock().unwrap().len(), 1);
    assert!(mock.release_attempts().is_empty());
    assert!(mock.rerun_commands().is_empty());
}

fn pending_control_agent(command: RunCommand, completion: NotificationEnd) -> PendingPaneSpawn {
    PendingPaneSpawn {
        run_command: Some(command.clone()),
        pane_title: None,
        open_file_payload: None,
        invoked_with: Some(Run::Command(command)),
        client_or_tab_index: ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(7)),
        completion_tx: Some(completion),
        placement: PanePlacement::New {
            new_pane_placement: NewPanePlacement::default(),
            start_suppressed: false,
            set_blocking: false,
        },
    }
}

#[test]
fn direct_agent_remote_startup_waits_before_placement_and_late_cancelled_starts_are_closed() {
    for target in [
        PaneTarget::Ssh {
            host: "host".into(),
        },
        // Its launch checks run in the same worker, so the spawner waits on them too.
        PaneTarget::Bubblewrap {
            root: "/repo".into(),
        },
    ] {
        for cancel in [false, true] {
            let mut os_api = MockOsApi::new();
            os_api.spawn_started = Arc::new(Barrier::new(2));
            os_api.spawn_release = Arc::new(Barrier::new(2));
            let (mut pty, _plugins, screen) = make_pty_with_screen_receiver(os_api.clone());
            let (pty_tx, pty_rx) = channels::unbounded();
            pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
            let command = RunCommand {
                command: "claude".into(),
                target: target.clone(),
                ..Default::default()
            };
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            let mut completion = NotificationEnd::new(tx);
            completion.wait_for_agent_start = true;
            let lifetime = Arc::new(());
            completion.agent_tab = Some(yinz_bus::route::AgentTabPlacement {
                tab_id: TabId::new(7),
                lifetime: Arc::downgrade(&lifetime),
                created: false,
            });
            pty.spawn_pane(
                Some(TerminalAction::RunCommand(command.clone())),
                pending_control_agent(command, completion),
            )
            .unwrap();
            os_api.spawn_started.wait();
            assert!(matches!(
                rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            assert!(!screen
                .try_iter()
                .any(|(instruction, _)| matches!(instruction, ScreenInstruction::NewPane(..))));
            if cancel {
                pty.close_pane(PaneId::Terminal(TerminalId::new(0)))
                    .unwrap();
            }
            os_api.spawn_release.wait();
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnResolved(PendingSpawn::Terminal(pending, spawned)) =
                instruction
            else {
                panic!("expected resolved spawn");
            };
            pty.handle_remote_spawned(pending, spawned).unwrap();
            let mut placed = false;
            for (instruction, _) in screen.try_iter() {
                if let ScreenInstruction::NewPane(
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    destination,
                    Some(mut completion),
                    _,
                ) = instruction
                {
                    assert!(!cancel);
                    assert_eq!(
                        destination,
                        ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(7))
                    );
                    assert!(completion
                        .agent_tab
                        .as_ref()
                        .unwrap()
                        .lifetime
                        .ptr_eq(&Arc::downgrade(&lifetime)));
                    completion.complete_pane_placement(TabId::new(7));
                    placed = true;
                }
            }
            assert_eq!(placed, !cancel);
            let result = rx.blocking_recv().unwrap();
            assert_eq!(result.error_message.is_some(), cancel);
            assert_eq!(result.affected_tab_id, (!cancel).then_some(TabId::new(7)));
            if cancel {
                assert!(os_api
                    .cleared_terminal_ids
                    .lock()
                    .unwrap()
                    .contains(&TerminalId::new(0)));
            }
            assert!(pty.pending_agent_spawns.is_empty());
        }
    }
}

#[test]
fn agent_process_start_failure_is_not_success_even_when_a_placeholder_is_placed() {
    let (mut pty, _, screen) = make_pty_with_screen_receiver(MockOsApi::new());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut completion = NotificationEnd::new(tx);
    completion.wait_for_agent_start = true;
    let command = RunCommand {
        command: "missing-agent".into(),
        hold_on_close: true,
        ..Default::default()
    };
    finish_pane_spawn(
        &mut pty,
        pending_control_agent(command, completion),
        Err(YinzError::CommandNotFound {
            terminal_id: TerminalId::new(41),
            command: "missing-agent".into(),
        }
        .into()),
    )
    .unwrap();
    for (instruction, _) in screen.try_iter() {
        if let ScreenInstruction::NewPane(_, _, _, _, _, _, _, Some(mut completion), _) =
            instruction
        {
            completion.wait_for_placement();
            completion.complete_pane_placement(TabId::new(7));
        }
    }
    let result = rx.blocking_recv().unwrap();
    assert!(result.error_message.unwrap().contains("failed to start"));
    assert_eq!(
        result.affected_pane_id,
        Some(PaneId::Terminal(TerminalId::new(41)))
    );
    assert_eq!(result.affected_tab_id, Some(TabId::new(7)));
}

#[test]
fn closing_a_new_remote_pane_while_it_connects_tears_down_the_late_transport() {
    let mut os_api = MockOsApi::new();
    os_api.transport_started = Arc::new(Barrier::new(2));
    os_api.transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    pty.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
        terminal_id: crate::TerminalId::new(41),
        terminal_action: TerminalAction::RunCommand(ssh_command()),
    }))
    .unwrap();
    os_api.transport_started.wait();
    pty.close_pane(PaneId::Terminal(crate::TerminalId::new(41)))
        .unwrap();
    let closed_at = os_api.cleared_terminal_ids.lock().unwrap().len();
    os_api.transport_release.wait();

    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::SpawnResolved(PendingSpawn::Terminal(pending, spawned)) = instruction
    else {
        panic!("expected a resolved remote pane")
    };
    pty.handle_remote_spawned(pending, spawned).unwrap();

    assert_eq!(
        os_api.cleared_terminal_ids.lock().unwrap()[closed_at..],
        [TerminalId::new(41)]
    );
}

#[test]
fn a_remote_spawn_after_the_pty_exits_clears_its_terminal_id() {
    let mut os_api = MockOsApi::new();
    os_api.transport_started = Arc::new(Barrier::new(2));
    os_api.transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let shutdown = Shutdown::default();
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(shutdown.sender(pty_tx));
    pty.bus.senders.should_silently_fail = false;

    pty.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
        terminal_id: TERMINAL_41,
        terminal_action: TerminalAction::RunCommand(ssh_command()),
    }))
    .unwrap();
    os_api.transport_started.wait();
    // The worker must observe the failed handback and clean up the transport itself.
    shutdown.begin();
    drop(pty_rx);
    os_api.transport_release.wait();
    pty.spawn_workers.pop().unwrap().join().unwrap();

    assert_eq!(*os_api.resolved_spawns.lock().unwrap(), [TERMINAL_41]);
    assert_eq!(*os_api.cleared_terminal_ids.lock().unwrap(), [TERMINAL_41]);
}

#[test]
fn a_ssh_rerun_does_not_block_the_pty_during_transport() {
    let mut os_api = MockOsApi::new();
    os_api.spawn_started = Arc::new(Barrier::new(2));
    os_api.spawn_release = Arc::new(Barrier::new(2));
    os_api.transport_started = Arc::new(Barrier::new(2));
    os_api.transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));

    assert!(pty
        .request_rerun(
            PaneId::Terminal(crate::TerminalId::new(41)),
            ssh_command(),
            None
        )
        .unwrap()
        .is_none());
    os_api.spawn_started.wait();
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(7)))
        .is_ok());
    os_api.spawn_release.wait();
    os_api.transport_started.wait();
    assert!(pty
        .close_pane(PaneId::Terminal(crate::TerminalId::new(7)))
        .is_ok());
    assert!(pty_rx.try_recv().is_err());
    os_api.transport_release.wait();

    let (pending, spawned) = resolved_rerun(&pty_rx);
    pty.handle_rerun_spawned(pending, spawned).unwrap();
    assert!(!pty.reruns.contains_key(&TerminalId::new(41)));
    assert_eq!(
        *os_api.resolved_spawns.lock().unwrap(),
        vec![TerminalId::new(41)]
    );
}

#[test]
fn a_failing_layout_pane_clears_its_reserved_remote_panes() {
    let os_api = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, _pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let local = RunCommand {
        command: PathBuf::from("sh"),
        ..Default::default()
    };
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                run: Some(Run::Command(ssh_command())),
                ..Default::default()
            },
            TiledPaneLayout {
                run: Some(Run::Command(local)),
                ..Default::default()
            },
            TiledPaneLayout {
                run: Some(Run::Command(ssh_command())),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    *os_api.fail_local_spawns.lock().unwrap() = true;

    let result = pty.spawn_terminals_for_layout(
        None,
        layout,
        vec![],
        Some(TerminalAction::RunCommand(RunCommand::default())),
        HashMap::new(),
        None,
        TabId::new(0),
        false,
        true,
        crate::ClientId::new(1),
        None,
    );

    assert!(result.is_err(), "the local failure is still reported");
    assert_eq!(
        *os_api.cleared_terminal_ids.lock().unwrap(),
        vec![TerminalId::new(0)],
        "the reservation made before the local failure is released"
    );
    assert!(pty.task_handles.is_empty());
}

fn ssh_command() -> RunCommand {
    RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::ssh("build-box").unwrap(),
        ..Default::default()
    }
}

#[test]
fn a_departed_client_is_focused_on_no_pane() {
    let (mut pty, _plugin_rx) = make_pty_with_plugin_receiver(MockOsApi::new());
    let pane = PaneId::Terminal(TerminalId::new(7));
    let (departed, attached) = (ClientId::new(1), ClientId::new(2));
    pty.set_active_pane(Some(pane), departed);
    pty.set_active_pane(Some(pane), attached);
    pty.remove_client(departed);
    assert_eq!(pty.focused_clients(pane), vec![attached]);
    assert!(!pty.active_panes.contains_key(&departed));
}

#[test]
fn overlay_launch_resolves_caller_metadata_separately_from_plugin_cwd() {
    let (mut pty, _, screen) = make_pty_with_screen_receiver(MockOsApi::new());
    pty.active_panes
        .insert(CLIENT_1, PaneId::Terminal(TERMINAL_41));
    pty.terminal_cwds
        .insert(TERMINAL_41, PathBuf::from("/caller"));
    for explicit_cwd in [None, Some(PathBuf::from("/plugin"))] {
        pty.resolve_overlay_tab_cwd(
            yinz_types::pane_tabs::PaneTabsId(1),
            CLIENT_1,
            explicit_cwd.clone(),
        )
        .unwrap();
        let (
            ScreenInstruction::OverlayTabCwd {
                cwd,
                caller_cwd,
                caller_terminal_pane_id,
                ..
            },
            _,
        ) = screen.try_recv().unwrap()
        else {
            panic!("expected overlay launch context");
        };
        assert_eq!(cwd, explicit_cwd.or_else(|| Some(PathBuf::from("/caller"))));
        assert_eq!(caller_cwd, Some(PathBuf::from("/caller")));
        assert_eq!(caller_terminal_pane_id, Some(TERMINAL_41));
    }
    let plugin = yinz_types::internal_ids::PluginId::new(8);
    pty.active_panes.insert(CLIENT_1, PaneId::Plugin(plugin));
    pty.plugin_cwds
        .insert(plugin, PathBuf::from("/caller-plugin"));
    pty.resolve_overlay_tab_cwd(yinz_types::pane_tabs::PaneTabsId(1), CLIENT_1, None)
        .unwrap();
    let (
        ScreenInstruction::OverlayTabCwd {
            caller_cwd,
            caller_terminal_pane_id,
            ..
        },
        _,
    ) = screen.try_recv().unwrap()
    else {
        panic!("expected overlay launch context");
    };
    assert_eq!(caller_cwd, Some(PathBuf::from("/caller-plugin")));
    assert_eq!(caller_terminal_pane_id, None);
}
