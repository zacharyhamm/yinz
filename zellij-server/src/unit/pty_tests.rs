use super::*;
use crate::os_input_output::ServerOsApi;
use crate::plugins::PluginInstruction;
use crate::pty_host::remote::conn::RemoteGitProbe;
use crate::pty_host::remote::RemoteStatus;
use crate::thread_bus::Bus;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use zellij_utils::channels::{self, SenderWithContext};
use zellij_utils::data::{Event, Palette};
use zellij_utils::errors::ErrorContext;
use zellij_utils::input::command::{PaneTarget, RunCommand};
use zellij_utils::ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg};

#[derive(Clone)]
struct MockOsApi {
    cwds: Arc<Mutex<HashMap<u32, PathBuf>>>,
    cmds: Arc<Mutex<HashMap<u32, Vec<String>>>>,
    cmds_by_ppid: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Every pid-addressed signal, recorded so a test can assert one was never
    /// sent. This is the whole point for remote panes: the failure being guarded
    /// against is a signal that *is* delivered, to the wrong process.
    local_signals: Arc<Mutex<Vec<(u32, &'static str)>>>,
    remote_panes: Arc<Mutex<std::collections::HashSet<u32>>>,
    remote_signals: Arc<Mutex<Vec<(u32, ChildSignal)>>>,
    remote_status: Arc<Mutex<HashMap<u32, RemoteStatus>>>,
    remote_hosts: Arc<Mutex<HashMap<u32, String>>>,
    /// What each remote pane's machine last said about its folder, as the
    /// broker's `GitProbe` arrives.
    remote_git_probes: Arc<Mutex<HashMap<u32, RemoteGitProbe>>>,
    /// Panes this backend will refuse to warp, and why.
    ///
    /// Absent means yes. A real backend answers out of the maps the release
    /// would use; this one answers out of what a test put here.
    warp_refusals: Arc<Mutex<HashMap<u32, String>>>,
    /// Panes whose release fails *after* the check said yes — the shape of a
    /// remote broker that will not confirm a handoff, and the only case where a
    /// pane really is cut loose.
    unreleasable: Arc<Mutex<std::collections::HashSet<u32>>>,
    /// Every id the release was actually attempted for, so a test can assert it
    /// was never attempted at all.
    release_attempts: Arc<Mutex<Vec<u32>>>,
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
    rerun_failures: Arc<Mutex<std::collections::HashSet<u32>>>,
    clear_failures: Arc<Mutex<std::collections::HashSet<u32>>>,
    /// Every id handed to `clear_terminal_id`, the teardown funnel, so a test can assert a
    /// pane that was closed out from under a worker still went through it.
    cleared_terminal_ids: Arc<Mutex<Vec<u32>>>,
    firecracker_started: Arc<Barrier>,
    firecracker_release: Arc<Barrier>,
    firecracker_transport_started: Arc<Barrier>,
    firecracker_transport_release: Arc<Barrier>,
    firecracker_terminal_id: Arc<Mutex<Option<u32>>>,
    resolved_firecracker_spawns: Arc<Mutex<Vec<u32>>>,
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
            firecracker_started: Arc::new(Barrier::new(1)),
            firecracker_release: Arc::new(Barrier::new(1)),
            firecracker_transport_started: Arc::new(Barrier::new(1)),
            firecracker_transport_release: Arc::new(Barrier::new(1)),
            firecracker_terminal_id: Arc::new(Mutex::new(None)),
            resolved_firecracker_spawns: Arc::new(Mutex::new(vec![])),
        }
    }
    fn rerun_commands(&self) -> Vec<RunCommand> {
        self.rerun_commands.lock().unwrap().clone()
    }
    fn fail_rerun_of(&self, terminal_id: u32) {
        self.rerun_failures.lock().unwrap().insert(terminal_id);
    }
    fn fail_clear_of(&self, terminal_id: u32) {
        self.clear_failures.lock().unwrap().insert(terminal_id);
    }
    fn refuse_to_warp(&self, terminal_id: u32, why: &str) {
        self.warp_refusals
            .lock()
            .unwrap()
            .insert(terminal_id, why.to_owned());
    }
    fn fail_the_release_of(&self, terminal_id: u32) {
        self.unreleasable.lock().unwrap().insert(terminal_id);
    }
    fn release_attempts(&self) -> Vec<u32> {
        self.release_attempts.lock().unwrap().clone()
    }
    fn release_budgets(&self) -> Vec<std::time::Duration> {
        self.release_budgets.lock().unwrap().clone()
    }
    /// Make every release take this long, the way a round trip to a broker does.
    fn make_each_release_take(&self, how_long: std::time::Duration) {
        *self.release_delay.lock().unwrap() = how_long;
    }
    fn set_remote_host(&self, terminal_id: u32, host: &str) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_hosts
            .lock()
            .unwrap()
            .insert(terminal_id, host.to_owned());
    }
    fn set_remote_git_probe(&self, terminal_id: u32, probe: RemoteGitProbe) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_git_probes
            .lock()
            .unwrap()
            .insert(terminal_id, probe);
    }
    fn set_remote_status(&self, terminal_id: u32, status: RemoteStatus) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
        self.remote_status
            .lock()
            .unwrap()
            .insert(terminal_id, status);
    }
    fn mark_remote(&self, terminal_id: u32) {
        self.remote_panes.lock().unwrap().insert(terminal_id);
    }
    fn local_signals(&self) -> Vec<(u32, &'static str)> {
        self.local_signals.lock().unwrap().clone()
    }
    fn remote_signals(&self) -> Vec<(u32, ChildSignal)> {
        self.remote_signals.lock().unwrap().clone()
    }
    fn set_cwd(&self, pid: u32, path: PathBuf) {
        self.cwds.lock().unwrap().insert(pid, path);
    }
    fn set_cmd(&self, pid: u32, cmd: Vec<String>) {
        self.cmds.lock().unwrap().insert(pid, cmd);
    }
    fn set_foreground_cmd(&self, ppid: u32, cmd: Vec<String>) {
        self.cmds_by_ppid
            .lock()
            .unwrap()
            .insert(ppid.to_string(), cmd);
    }
    fn clear_foreground_cmd(&self, ppid: u32) {
        self.cmds_by_ppid.lock().unwrap().remove(&ppid.to_string());
    }
}

impl ServerOsApi for MockOsApi {
    fn set_terminal_size_using_terminal_id(
        &self,
        _: u32,
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
    ) -> anyhow::Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn resolve_firecracker_spawn(
        &self,
        terminal_action: TerminalAction,
        _default_editor: Option<PathBuf>,
    ) -> anyhow::Result<crate::os_input_output::ResolvedFirecracker> {
        self.firecracker_started.wait();
        self.firecracker_release.wait();
        let terminal_id = self
            .firecracker_terminal_id
            .lock()
            .unwrap()
            .context("injected provider result")?;
        let TerminalAction::RunCommand(command) = terminal_action else {
            unreachable!()
        };
        let rerun_command = command.clone();
        Ok(crate::os_input_output::ResolvedFirecracker {
            terminal_id,
            command,
            rerun_command,
            endpoint: crate::os_input_output::RemoteEndpoint {
                host: "resolved-vm".to_owned(),
                ssh_options: Default::default(),
            },
        })
    }
    fn spawn_resolved_firecracker(
        &self,
        resolved: crate::os_input_output::ResolvedFirecracker,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        self.firecracker_transport_started.wait();
        self.firecracker_transport_release.wait();
        self.resolved_firecracker_spawns
            .lock()
            .unwrap()
            .push(resolved.terminal_id);
        Ok((
            resolved.terminal_id,
            Box::new(crate::os_input_output::NullAsyncReader),
            None,
        ))
    }
    fn resolve_firecracker_rerun(
        &self,
        terminal_id: u32,
        command: RunCommand,
    ) -> anyhow::Result<crate::os_input_output::ResolvedFirecracker> {
        self.firecracker_started.wait();
        self.firecracker_release.wait();
        let rerun_command = command.clone();
        Ok(crate::os_input_output::ResolvedFirecracker {
            terminal_id,
            command,
            rerun_command,
            endpoint: crate::os_input_output::RemoteEndpoint {
                host: "resolved-vm".to_owned(),
                ssh_options: Default::default(),
            },
        })
    }
    fn rerun_resolved_firecracker(
        &self,
        resolved: crate::os_input_output::ResolvedFirecracker,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(Box<dyn AsyncReader>, Option<u32>)> {
        self.firecracker_transport_started.wait();
        self.firecracker_transport_release.wait();
        self.resolved_firecracker_spawns
            .lock()
            .unwrap()
            .push(resolved.terminal_id);
        Ok((Box::new(crate::os_input_output::NullAsyncReader), None))
    }
    fn write_to_tty_stdin(&self, _: u32, buf: &[u8]) -> anyhow::Result<usize> {
        Ok(buf.len())
    }
    fn tcdrain(&self, _: u32) -> anyhow::Result<()> {
        Ok(())
    }
    fn kill(&self, pid: u32) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGHUP"));
        Ok(())
    }
    fn force_kill(&self, pid: u32) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGKILL"));
        Ok(())
    }
    fn send_sigint(&self, pid: u32) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGINT"));
        Ok(())
    }
    fn send_sigcont(&self, pid: u32) -> anyhow::Result<()> {
        self.local_signals.lock().unwrap().push((pid, "SIGCONT"));
        Ok(())
    }
    fn is_remote_pane(&self, terminal_id: u32) -> bool {
        self.remote_panes.lock().unwrap().contains(&terminal_id)
    }
    fn signal_remote_child(&self, terminal_id: u32, signal: ChildSignal) -> anyhow::Result<()> {
        self.remote_signals
            .lock()
            .unwrap()
            .push((terminal_id, signal));
        Ok(())
    }
    fn remote_pane_ids(&self) -> Vec<u32> {
        self.remote_panes.lock().unwrap().iter().copied().collect()
    }
    fn remote_pane_status(&self, terminal_id: u32) -> Option<RemoteStatus> {
        self.remote_status
            .lock()
            .unwrap()
            .get(&terminal_id)
            .cloned()
    }
    fn remote_pane_host(&self, terminal_id: u32) -> Option<String> {
        self.remote_hosts.lock().unwrap().get(&terminal_id).cloned()
    }
    fn remote_pane_git_probe(&self, terminal_id: u32) -> Option<RemoteGitProbe> {
        self.remote_git_probes
            .lock()
            .unwrap()
            .get(&terminal_id)
            .cloned()
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
    fn get_cwd(&self, pid: u32) -> Option<PathBuf> {
        self.cwds.lock().unwrap().get(&pid).cloned()
    }
    fn get_cwds(&self, pids: Vec<u32>) -> (HashMap<u32, PathBuf>, HashMap<u32, Vec<String>>) {
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
        panes: &[(u32, u32)],
        _: &Option<String>,
    ) -> HashMap<u32, Vec<String>> {
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
        terminal_id: u32,
        run_command: RunCommand,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(Box<dyn AsyncReader>, Option<u32>)> {
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
    fn clear_terminal_id(&self, terminal_id: u32) -> anyhow::Result<()> {
        self.cleared_terminal_ids.lock().unwrap().push(terminal_id);
        (!self.clear_failures.lock().unwrap().contains(&terminal_id))
            .then_some(())
            .context("injected clear failure")
    }
    fn check_terminal_can_be_warped(&self, terminal_id: u32) -> anyhow::Result<()> {
        match self.warp_refusals.lock().unwrap().get(&terminal_id) {
            Some(why) => Err(anyhow::anyhow!("{why}")),
            None => Ok(()),
        }
    }
    fn release_terminal_for_warp(
        &self,
        terminal_id: u32,
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
                host_id: format!("host-{terminal_id}"),
                host_pid: 1,
                control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
            },
        ))
    }
}

fn make_pty_with_plugin_receiver(
    mock: MockOsApi,
) -> (Pty, channels::Receiver<(PluginInstruction, ErrorContext)>) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let plugin_sender = SenderWithContext::new(plugin_tx);
    let mut bus: Bus<PtyInstruction> = Bus::empty().should_silently_fail();
    bus.os_input = Some(Box::new(mock));
    bus.senders.to_plugin = Some(plugin_sender);
    let pty = Pty::new(bus, false, None, None);
    (pty, plugin_rx)
}

fn make_pty_with_screen_receiver(
    mock: MockOsApi,
) -> (
    Pty,
    channels::Receiver<(PluginInstruction, ErrorContext)>,
    channels::Receiver<(ScreenInstruction, ErrorContext)>,
) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let (screen_tx, screen_rx) = channels::unbounded();
    let mut bus: Bus<PtyInstruction> = Bus::empty().should_silently_fail();
    bus.os_input = Some(Box::new(mock));
    bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    bus.senders.to_screen = Some(SenderWithContext::new(screen_tx));
    let pty = Pty::new(bus, false, None, None);
    (pty, plugin_rx, screen_rx)
}

fn collect_pane_host_instructions(
    rx: &channels::Receiver<(ScreenInstruction, ErrorContext)>,
) -> Vec<(u32, Option<String>)> {
    let mut updates = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let ScreenInstruction::UpdatePaneHost(terminal_id, host) = instruction {
            updates.push((terminal_id, host));
        }
    }
    updates
}

fn collect_pane_cwd_instructions(
    rx: &channels::Receiver<(ScreenInstruction, ErrorContext)>,
) -> Vec<(u32, Option<PathBuf>)> {
    let mut updates = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let ScreenInstruction::UpdatePaneCwd(terminal_id, cwd) = instruction {
            updates.push((terminal_id, cwd));
        }
    }
    updates
}

fn set_active_terminal(pty: &mut Pty, terminal_id: u32, child_pid: u32) {
    let flag = Arc::new(AtomicBool::new(true));
    pty.id_to_child_pid.insert(terminal_id, child_pid);
    pty.pane_activity_flags.insert(terminal_id, flag);
}

fn collect_cwd_changed_events(
    rx: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(PaneId, PathBuf)> {
    let mut events = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                if let Event::CwdChanged(pane_id, cwd, _) = event {
                    events.push((pane_id.into(), cwd));
                }
            }
        }
    }
    events
}

fn collect_command_changed_events(
    rx: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(PaneId, Vec<String>, bool)> {
    let mut events = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                if let Event::CommandChanged(pane_id, cmd, is_foreground, _) = event {
                    events.push((pane_id.into(), cmd, is_foreground));
                }
            }
        }
    }
    events
}

#[test]
fn foreground_command_emitted_with_is_foreground_true() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_foreground_cmd(child_pid, vec!["vim".into(), "file.rs".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, vec!["vim", "file.rs"]);
    assert!(events[0].2, "expected is_foreground=true");
}

#[test]
fn empty_foreground_falls_back_to_shell_command() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cmd(child_pid, vec!["/bin/bash".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, vec!["/bin/bash"]);
    assert!(!events[0].2, "expected is_foreground=false");
}

#[test]
fn foreground_clearing_emits_shell_fallback() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cmd(child_pid, vec!["/bin/zsh".into()]);
    mock.set_foreground_cmd(child_pid, vec!["cargo".into(), "build".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock.clone());
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert!(events[0].2, "first event should be foreground");
    assert_eq!(events[0].1, vec!["cargo", "build"]);

    mock.clear_foreground_cmd(child_pid);
    pty.pane_activity_flags
        .get(&1)
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
    let child_pid = 100;
    mock.set_foreground_cmd(child_pid, vec!["htop".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();
    let _ = collect_command_changed_events(&rx);

    pty.pane_activity_flags
        .get(&1)
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
    let child_pid = 100;
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);
    pty.pane_activity_flags
        .get(&1)
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
    let child_pid = 100;
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock.clone());
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();
    let events = collect_command_changed_events(&rx);
    assert_eq!(events[0].1, vec!["vim"]);
    assert!(events[0].2);

    mock.set_foreground_cmd(child_pid, vec!["cargo".into(), "test".into()]);
    pty.pane_activity_flags
        .get(&1)
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
    let child_pid = 100;
    let (mut pty, _rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);
    assert!(pty
        .pane_activity_flags
        .get(&1)
        .unwrap()
        .load(Ordering::Relaxed));

    pty.update_and_report_cwds();

    assert!(
        !pty.pane_activity_flags
            .get(&1)
            .unwrap()
            .load(Ordering::Relaxed),
        "activity flag should be reset to false after poll"
    );
}

#[test]
fn multiple_terminals_only_active_ones_polled() {
    let mock = MockOsApi::new();
    let pid_active = 100;
    let pid_inactive = 200;
    mock.set_cwd(pid_active, PathBuf::from("/active"));
    mock.set_cwd(pid_inactive, PathBuf::from("/inactive"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, pid_active);
    set_active_terminal(&mut pty, 2, pid_inactive);
    pty.pane_activity_flags
        .get(&2)
        .unwrap()
        .store(false, Ordering::Relaxed);

    pty.update_and_report_cwds();

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, PathBuf::from("/active"));
}

// --- CWD change events ---

#[test]
fn cwd_changed_event_emitted_on_change() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/home/user"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.update_and_report_cwds();

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, PathBuf::from("/home/user"));
}

#[test]
fn no_cwd_event_when_unchanged() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/home/user"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);
    pty.terminal_cwds.insert(1, PathBuf::from("/home/user"));

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
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/work/hocus"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);

    pty.capture_initial_cwd(1, child_pid, None);

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, PathBuf::from("/work/hocus"));
}

/// Why it was a race rather than a certainty: with the child not yet forked far
/// enough to be asked, nothing was recorded and the ticker reported it after
/// all. The caller has already resolved the folder it asked for, so use it.
#[test]
fn an_initial_cwd_the_os_cannot_answer_falls_back_to_the_requested_one() {
    let (mut pty, rx) = make_pty_with_plugin_receiver(MockOsApi::new());

    pty.capture_initial_cwd(1, 100, Some(PathBuf::from("/work/hocus")));

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, PathBuf::from("/work/hocus"));
    assert_eq!(
        pty.terminal_cwds.get(&1),
        Some(&PathBuf::from("/work/hocus"))
    );
}

#[test]
fn an_initial_cwd_with_nothing_to_go_on_reports_nothing() {
    let (mut pty, rx) = make_pty_with_plugin_receiver(MockOsApi::new());

    pty.capture_initial_cwd(1, 100, None);

    assert!(collect_cwd_changed_events(&rx).is_empty());
    assert!(pty.terminal_cwds.is_empty());
}

/// Both writers go through one place, so the ticker must not say again what the
/// capture already said.
#[test]
fn an_initial_cwd_reports_once_and_the_ticker_stays_quiet() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/work/hocus"));
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.capture_initial_cwd(1, child_pid, None);
    pty.update_and_report_cwds();

    assert_eq!(collect_cwd_changed_events(&rx).len(), 1);
}

/// Screen holds the folder too, so that every pane manifest can carry it and a
/// plugin that missed the event above is put right by the next update.
#[test]
fn an_initial_cwd_reaches_screen() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/work/hocus"));
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.capture_initial_cwd(1, child_pid, None);

    assert_eq!(
        collect_pane_cwd_instructions(&screen_rx),
        vec![(1, Some(PathBuf::from("/work/hocus")))]
    );
}

/// A terminal id is handed out again after its pane closes, so a folder left
/// behind on screen is the wrong answer for whichever pane inherits the id.
#[test]
fn a_closed_pane_takes_its_folder_with_it() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/work/hocus"));
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.capture_initial_cwd(1, child_pid, None);
    let _ = collect_pane_cwd_instructions(&screen_rx);

    pty.forget_terminal_cwd(1);

    assert_eq!(collect_pane_cwd_instructions(&screen_rx), vec![(1, None)]);
    assert!(pty.terminal_cwds.is_empty());
}

// --- OSC7 CWD notification ---

#[test]
fn osc7_emits_cwd_changed() {
    let mock = MockOsApi::new();
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    pty.id_to_child_pid.insert(1, 100);

    pty.notify_cwd_from_osc7(1, PathBuf::from("/tmp/new"));

    let events = collect_cwd_changed_events(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(1));
    assert_eq!(events[0].1, PathBuf::from("/tmp/new"));
    assert_eq!(
        pty.terminal_cwds.get(&1),
        Some(&PathBuf::from("/tmp/new")),
        "cache should be updated"
    );
}

#[test]
fn osc7_no_event_when_unchanged() {
    let mock = MockOsApi::new();
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    pty.id_to_child_pid.insert(1, 100);
    pty.terminal_cwds.insert(1, PathBuf::from("/same"));

    pty.notify_cwd_from_osc7(1, PathBuf::from("/same"));

    let events = collect_cwd_changed_events(&rx);
    assert!(events.is_empty(), "no event when osc7 path matches cache");
}

#[test]
fn osc7_clears_activity_flag() {
    let mock = MockOsApi::new();
    let (mut pty, _rx) = make_pty_with_plugin_receiver(mock);
    let flag = Arc::new(AtomicBool::new(true));
    pty.id_to_child_pid.insert(1, 100);
    pty.pane_activity_flags.insert(1, flag.clone());

    pty.notify_cwd_from_osc7(1, PathBuf::from("/new"));

    assert!(
        !flag.load(Ordering::Relaxed),
        "osc7 should clear the activity flag"
    );
}

#[test]
fn osc7_then_poll_skips_terminal() {
    let mock = MockOsApi::new();
    let child_pid = 100;
    mock.set_cwd(child_pid, PathBuf::from("/from-proc"));
    mock.set_foreground_cmd(child_pid, vec!["vim".into()]);
    let (mut pty, rx) = make_pty_with_plugin_receiver(mock);
    set_active_terminal(&mut pty, 1, child_pid);

    pty.notify_cwd_from_osc7(1, PathBuf::from("/from-osc7"));
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
fn pty_with_remote_pane(terminal_id: u32, bystander_pid: u32) -> (Pty, MockOsApi) {
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
    let (mut pty, os_api) = pty_with_remote_pane(7, 4242);
    let _ = pty.close_pane(PaneId::Terminal(7));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's close signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(os_api.remote_signals(), vec![(7, ChildSignal::Hup)]);
}

#[test]
fn a_failed_remote_termination_does_not_fail_the_close() {
    let (mut pty, os_api) = pty_with_remote_pane(7, 4242);
    os_api.fail_clear_of(7);

    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
}

#[test]
fn a_firecracker_preflight_does_not_block_other_pty_work() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_started = Arc::new(Barrier::new(2));
    os_api.firecracker_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    let pending = PendingTerminalSpawn {
        terminal_action: Some(TerminalAction::RunCommand(command.clone())),
        run_command: Some(command.clone()),
        pane_title: None,
        open_file_payload: None,
        invoked_with: Some(Run::Command(command)),
        new_pane_placement: NewPanePlacement::default(),
        start_suppressed: false,
        client_or_tab_index: ClientTabIndexOrPaneId::ClientId(1),
        completion_tx: None,
        set_blocking: false,
    };

    pty.defer_firecracker_spawn(pending).unwrap();
    os_api.firecracker_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    os_api.firecracker_release.wait();
}

#[test]
fn a_firecracker_layout_preflight_does_not_block_pty_work_or_apply_layout() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_started = Arc::new(Barrier::new(2));
    os_api.firecracker_release = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_started = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_release = Arc::new(Barrier::new(2));
    *os_api.firecracker_terminal_id.lock().unwrap() = Some(41);
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    let layout = TiledPaneLayout {
        run: Some(Run::Command(command.clone())),
        ..Default::default()
    };

    pty.spawn_terminals_for_layout(
        None,
        layout,
        vec![],
        Some(TerminalAction::RunCommand(command)),
        HashMap::new(),
        None,
        0,
        false,
        true,
        1,
        None,
    )
    .unwrap();
    os_api.firecracker_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    os_api.firecracker_release.wait();
    os_api.firecracker_transport_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    assert!(!screen_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ScreenInstruction::ApplyLayout(..))));
    os_api.firecracker_transport_release.wait();

    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::FirecrackerLayoutSpawned(pending, spawns) = instruction else {
        panic!("expected spawned Firecracker layout")
    };
    pty.finish_layout_spawn(*pending, spawns.take().unwrap())
        .unwrap();

    assert_eq!(
        *os_api.resolved_firecracker_spawns.lock().unwrap(),
        vec![41]
    );
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::ApplyLayout(_, _, tiled, _, _, _, _, _, _, _, _)
            if tiled == vec![(41, None)]
    )));
}

#[test]
fn dropping_pty_does_not_wait_for_a_firecracker_worker() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_started = Arc::new(Barrier::new(2));
    os_api.firecracker_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    let pending = PendingTerminalSpawn {
        terminal_action: Some(TerminalAction::RunCommand(command.clone())),
        run_command: Some(command.clone()),
        pane_title: None,
        open_file_payload: None,
        invoked_with: Some(Run::Command(command)),
        new_pane_placement: NewPanePlacement::default(),
        start_suppressed: false,
        client_or_tab_index: ClientTabIndexOrPaneId::ClientId(1),
        completion_tx: None,
        set_blocking: false,
    };
    pty.defer_firecracker_spawn(pending).unwrap();
    os_api.firecracker_started.wait();

    let (dropped_tx, dropped_rx) = channels::bounded(1);
    let dropper = std::thread::spawn(move || {
        drop(pty);
        dropped_tx.send(()).unwrap();
    });
    let returned = dropped_rx
        .recv_timeout(std::time::Duration::from_millis(100))
        .is_ok();
    os_api.firecracker_release.wait();
    dropper.join().unwrap();
    assert!(returned, "Pty::drop waited for the Firecracker worker");
}

#[test]
fn a_firecracker_layout_override_waits_off_thread_for_transport() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_started = Arc::new(Barrier::new(2));
    os_api.firecracker_release = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_started = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_release = Arc::new(Barrier::new(2));
    *os_api.firecracker_terminal_id.lock().unwrap() = Some(41);
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    let pending = PendingLayoutOverride {
        cwd: None,
        default_shell: None,
        tabs: vec![(
            TabLayoutInfo {
                tab_index: 0,
                tab_name: None,
                tiled_layout: TiledPaneLayout {
                    run: Some(Run::Command(command)),
                    ..Default::default()
                },
                floating_layouts: vec![],
                swap_tiled_layouts: None,
                swap_floating_layouts: None,
            },
            HashMap::new(),
        )],
        retain_existing_terminal_panes: false,
        retain_existing_plugin_panes: false,
        client_id: 1,
        completion_tx: None,
    };

    pty.defer_firecracker_layout_override(pending).unwrap();
    os_api.firecracker_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    os_api.firecracker_release.wait();
    os_api.firecracker_transport_started.wait();
    assert!(!screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::OverrideLayoutComplete(..)
    )));
    os_api.firecracker_transport_release.wait();

    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::FirecrackerLayoutOverrideSpawned(pending, spawns) = instruction else {
        panic!("expected spawned Firecracker layout override")
    };
    pty.finish_layout_override(*pending, spawns.take().unwrap())
        .unwrap();
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::OverrideLayoutComplete(tabs, false, false, 1, _)
            if tabs.len() == 1 && tabs[0].new_terminal_pids == vec![(41, None)]
    )));
}

#[test]
fn a_firecracker_rerun_does_not_block_the_pty_during_transport() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_started = Arc::new(Barrier::new(2));
    os_api.firecracker_release = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_started = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };

    pty.defer_firecracker_rerun(PaneId::Terminal(41), command, None)
        .unwrap();
    os_api.firecracker_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    os_api.firecracker_release.wait();
    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::FirecrackerReRunResolved(
        pane_id,
        command,
        completion_tx,
        generation,
        resolution,
    ) = instruction
    else {
        panic!("expected resolved Firecracker rerun")
    };
    pty.firecracker_reruns_in_transport.insert(41);
    pty.defer_resolved_firecracker_rerun(
        pane_id,
        command,
        completion_tx,
        generation,
        resolution.unwrap(),
    )
    .unwrap();
    os_api.firecracker_transport_started.wait();
    assert!(pty.close_pane(PaneId::Terminal(7)).is_ok());
    assert!(pty_rx.try_recv().is_err());
    os_api.firecracker_transport_release.wait();
    assert!(matches!(
        pty_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .0,
        PtyInstruction::FirecrackerReRunSpawned(..)
    ));
}

/// The worker has already registered the remote transport and started the guest command by
/// the time `FirecrackerReRunSpawned` is sent, so a pane closed in the meantime still has to
/// go through `clear_terminal_id` when that message lands — otherwise the guest command and
/// the transport entry outlive the pane, and the id stays marked as in transport forever.
#[test]
fn closing_a_pane_mid_transport_tears_down_the_late_spawn() {
    let mut os_api = MockOsApi::new();
    os_api.firecracker_transport_started = Arc::new(Barrier::new(2));
    os_api.firecracker_transport_release = Arc::new(Barrier::new(2));
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let (pty_tx, pty_rx) = channels::unbounded();
    pty.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };

    pty.defer_firecracker_rerun(PaneId::Terminal(41), command, None)
        .unwrap();
    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::FirecrackerReRunResolved(
        pane_id,
        command,
        completion_tx,
        generation,
        resolution,
    ) = instruction
    else {
        panic!("expected resolved Firecracker rerun")
    };
    pty.firecracker_reruns_in_transport.insert(41);
    pty.defer_resolved_firecracker_rerun(
        pane_id,
        command,
        completion_tx,
        generation,
        resolution.unwrap(),
    )
    .unwrap();
    os_api.firecracker_transport_started.wait();
    // A rerun asked for while the spawn is in transport is parked behind it; it must go
    // with the pane rather than being started against a closed one.
    pty.queued_firecracker_reruns.insert(
        41,
        (
            RunCommand {
                command: PathBuf::from("claude"),
                target: PaneTarget::Firecracker {
                    vm: "review".to_owned(),
                    image: None,
                    worktree: None,
                },
                ..Default::default()
            },
            None,
        ),
    );
    assert!(pty.close_pane(PaneId::Terminal(41)).is_ok());
    let closed_at = os_api.cleared_terminal_ids.lock().unwrap().len();
    os_api.firecracker_transport_release.wait();

    let (instruction, _) = pty_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let PtyInstruction::FirecrackerReRunSpawned(
        pane_id,
        command,
        _completion_tx,
        generation,
        previous_host,
        requested_cwd,
        spawned,
    ) = instruction
    else {
        panic!("expected spawned Firecracker rerun")
    };
    pty.handle_firecracker_rerun_spawned(
        pane_id,
        command,
        generation,
        previous_host,
        requested_cwd,
        spawned,
    )
    .unwrap();

    assert_eq!(
        os_api.cleared_terminal_ids.lock().unwrap()[closed_at..],
        [41],
        "the late spawn must be torn down through the funnel"
    );
    assert!(!pty.firecracker_reruns_in_transport.contains(&41));
    assert!(!pty.queued_firecracker_reruns.contains_key(&41));
    assert!(!pty.pending_firecracker_reruns.contains_key(&41));
    assert!(
        pty_rx.try_recv().is_err(),
        "no rerun may be started for a closed pane"
    );
}

#[test]
fn a_failed_firecracker_rerun_is_shown_and_held_without_hold_on_close() {
    let os_api = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(os_api);
    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        hold_on_close: false,
        ..Default::default()
    };
    let error = ZellijError::PaneStartFailed {
        terminal_id: 41,
        message: "provider failed".to_owned(),
    }
    .into();

    finish_command_rerun(&mut pty, PaneId::Terminal(41), command, Err(error)).unwrap();

    let instructions = screen_rx
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        ScreenInstruction::PtyBytes(41, message) if message == b"provider failed"
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        ScreenInstruction::HoldPane(PaneId::Terminal(41), Some(2), _)
    )));
}

#[test]
fn pty_teardown_closes_remote_panes_without_local_pids() {
    let os_api = MockOsApi::new();
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    os_api.mark_remote(7);

    drop(pty);

    assert_eq!(os_api.remote_signals(), vec![(7, ChildSignal::Hup)]);
}

#[test]
fn sigint_to_a_remote_pane_does_not_signal_a_local_process() {
    let (pty, os_api) = pty_with_remote_pane(7, 4242);
    pty.send_sigint_to_pane(PaneId::Terminal(7));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's SIGINT signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(os_api.remote_signals(), vec![(7, ChildSignal::Int)]);
}

#[test]
fn sigkill_to_a_remote_pane_does_not_signal_a_local_process() {
    let (pty, os_api) = pty_with_remote_pane(7, 4242);
    pty.send_sigkill_to_pane(PaneId::Terminal(7));

    assert!(
        os_api.local_signals().is_empty(),
        "a remote pane's SIGKILL signalled local pids {:?}",
        os_api.local_signals()
    );
    assert_eq!(os_api.remote_signals(), vec![(7, ChildSignal::Kill)]);
}

/// The same refactor must leave local panes exactly as they were.
#[test]
fn a_local_pane_is_still_signalled_by_pid() {
    let os_api = MockOsApi::new();
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(os_api.clone());
    let mut pty = pty;
    pty.id_to_child_pid.insert(3, 909);

    pty.send_sigint_to_pane(PaneId::Terminal(3));
    pty.send_sigkill_to_pane(PaneId::Terminal(3));
    let _ = pty.close_pane(PaneId::Terminal(3));

    assert_eq!(
        os_api.local_signals(),
        vec![(909, "SIGINT"), (909, "SIGKILL"), (909, "SIGHUP")]
    );
    assert!(os_api.remote_signals().is_empty());
}

/// Handing a remote pid to a plugin invites the same mistake one layer out.
#[test]
fn a_remote_panes_pid_is_not_handed_to_plugins() {
    let (pty, _os_api) = pty_with_remote_pane(7, 4242);
    match pty.get_pane_pid(PaneId::Terminal(7)) {
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
    terminal_id: u32,
    status: RemoteStatus,
) -> (
    Pty,
    MockOsApi,
    channels::Receiver<(PluginInstruction, ErrorContext)>,
    channels::Receiver<(ScreenInstruction, ErrorContext)>,
) {
    let mock = MockOsApi::new();
    mock.set_remote_status(terminal_id, status);
    let (pty, plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock.clone());
    (pty, mock, plugin_rx, screen_rx)
}

/// The whole point of Phase 3: argv the broker read on its own machine reaches
/// the same event a local pane's argv would. Agent classification is pure string
/// matching over that argv, so this is what makes a remote agent detectable at
/// all — with nothing in the detection layer aware of the difference.
#[test]
fn a_remote_panes_foreground_command_is_reported_like_any_other() {
    let (mut pty, _mock, plugin_rx, _screen_rx) = remote_pane(
        7,
        RemoteStatus {
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned(), "--resume".to_owned()]),
        },
    );

    pty.update_and_report_cwds();

    let events = collect_command_changed_events(&plugin_rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, PaneId::Terminal(7));
    assert_eq!(events[0].1, vec!["claude", "--resume"]);
    assert!(
        events[0].2,
        "a foreground command must be flagged as one, or it reads as the shell"
    );
}

/// With nothing in the foreground, the pane's own command is reported and
/// flagged as not-foreground — exactly as for a local pane.
#[test]
fn a_remote_pane_with_no_foreground_falls_back_to_its_own_command() {
    let (mut pty, _mock, plugin_rx, _screen_rx) = remote_pane(
        7,
        RemoteStatus {
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
        7,
        RemoteStatus {
            cwd: Some("/srv/build/project".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: None,
        },
    );

    pty.update_and_report_cwds();

    let cwds = collect_pane_cwd_instructions(&screen_rx);
    assert_eq!(
        cwds,
        vec![(7, Some(PathBuf::from("/srv/build/project")))],
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
        7,
        RemoteStatus {
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
        7,
        RemoteStatus {
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
        7,
        RemoteStatus {
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

/// Plugins ask directly when the poll missed something (`hocus-agentd` repairs
/// panes this way). A remote pane has to answer from its cache rather than
/// reporting itself as not running.
#[test]
fn a_remote_pane_answers_the_plugin_facing_accessors() {
    let (pty, _mock, _plugin_rx, _screen_rx) = remote_pane(
        7,
        RemoteStatus {
            cwd: Some("/srv/build/project".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: Some(vec!["claude".to_owned()]),
        },
    );

    match pty.get_pane_running_command(PaneId::Terminal(7)) {
        GetPaneRunningCommandResponse::Ok(argv) => assert_eq!(argv, vec!["claude".to_owned()]),
        other => panic!("expected the running command, got {other:?}"),
    }
    match pty.get_pane_cwd(PaneId::Terminal(7)) {
        GetPaneCwdResponse::Ok(cwd) => assert_eq!(cwd, PathBuf::from("/srv/build/project")),
        other => panic!("expected the cwd, got {other:?}"),
    }
}

/// The far machine ran Git in the pane's folder, and the answer is handed to the
/// plugins raw. Raw is the point: a repository with no commits writes its
/// identity to stdout and *then* exits 128, so a parse that gated on the exit
/// code would record a real checkout as an ordinary folder — and the parser that
/// knows this already exists in `hocus-agentd`, for local panes.
#[test]
fn a_remote_pane_hands_its_git_probe_to_the_plugins() {
    let mock = MockOsApi::new();
    mock.set_remote_git_probe(
        7,
        RemoteGitProbe {
            host: "build-box".to_owned(),
            cwd: "/srv/build/project".to_owned(),
            exit_code: Some(128),
            metadata: b"/srv/build/project\n/srv/build/project/.git\n".to_vec(),
            config: b"remote.origin.url\ngit@example.com:me/project\0".to_vec(),
        },
    );
    let (pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);

    match pty.get_pane_git_probe(PaneId::Terminal(7)) {
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

    match pty.get_pane_git_probe(PaneId::Terminal(7)) {
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
        7,
        RemoteStatus {
            cwd: Some("/home/me/work".to_owned()),
            shell_argv: Some(vec!["/bin/zsh".to_owned()]),
            foreground_argv: None,
        },
    );
    mock.set_remote_host(7, "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.report_terminal_host(7);

    let hosts = collect_pane_host_instructions(&screen_rx);
    assert_eq!(hosts, vec![(7, Some("build-box".to_owned()))]);
}

/// Local panes are the overwhelming majority, and must stay silent — an
/// announcement per pane would be noise, and a `Some` on a local pane would make
/// every consumer treat it as elsewhere.
#[test]
fn a_local_pane_announces_no_host() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.report_terminal_host(3);

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
        PaneId::Terminal(7),
        RunCommand {
            command: PathBuf::from("step2"),
            hold_on_close: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        collect_pane_host_instructions(&screen_rx),
        vec![(7, None)],
        "the pane's host should be retracted, exactly once"
    );
}

/// And a respawn that stays put says so in a single update. A `forget` followed by a `report`
/// would be two, and a plugin reading the pair would see the pane briefly claim to be local.
#[test]
fn a_respawn_on_the_same_host_reports_it_once_and_never_blips() {
    let mock = MockOsApi::new();
    mock.set_remote_host(7, "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    pty.rerun_command_in_pane(
        PaneId::Terminal(7),
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
        vec![(7, Some("build-box".to_owned()))],
        "one update naming the host, with no host-less one before it"
    );
}

#[test]
fn a_respawn_onto_another_host_clears_the_old_hosts_cwd() {
    let mock = MockOsApi::new();
    mock.set_remote_host(7, "old-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(7, PathBuf::from("/srv/on-old-box"));

    pty.rerun_command_in_pane(
        PaneId::Terminal(7),
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
    assert_eq!(collect_pane_cwd_instructions(&screen_rx), vec![(7, None)]);
}

#[test]
fn a_same_host_respawn_keeps_its_cwd_until_the_host_reports_again() {
    let mock = MockOsApi::new();
    mock.set_remote_host(7, "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(7, PathBuf::from("/srv/on-build-box"));

    pty.rerun_command_in_pane(
        PaneId::Terminal(7),
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
        pty.terminal_cwds.get(&7),
        Some(&PathBuf::from("/srv/on-build-box"))
    );
    assert!(collect_pane_cwd_instructions(&screen_rx).is_empty());
}

#[test]
fn a_failed_host_change_retracts_the_old_host_and_cwd() {
    let mock = MockOsApi::new();
    mock.set_remote_host(7, "old-box");
    mock.fail_rerun_of(7);
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(7, PathBuf::from("/srv/on-old-box"));

    let result = pty.rerun_command_in_pane(
        PaneId::Terminal(7),
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
    assert_eq!(cwd_updates, vec![(7, None)]);
    assert_eq!(host_updates, vec![(7, None)]);
}

/// A cwd is a path on one machine. Handing a local pane's cwd to a pane that
/// runs elsewhere names a directory that need not exist there — and for a pane
/// that asked for a login shell it is worse than useless: an unset cwd is
/// precisely what lands it in the remote home directory, the way ssh does.
#[test]
fn a_remote_pane_does_not_inherit_a_local_panes_cwd() {
    let mock = MockOsApi::new();
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(3, PathBuf::from("/home/me/on-this-machine"));

    let mut action = TerminalAction::RunCommand(RunCommand {
        remote_default_shell: true,
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(3));

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
    mock.set_remote_host(3, "build-box");
    let (mut pty, _plugin_rx, _screen_rx) = make_pty_with_screen_receiver(mock);
    pty.terminal_cwds
        .insert(3, PathBuf::from("/srv/on-the-far-machine"));

    let mut action = TerminalAction::RunCommand(RunCommand {
        remote_default_shell: true,
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(3));

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
    pty.terminal_cwds
        .insert(3, PathBuf::from("/home/me/on-this-machine"));

    let mut action = TerminalAction::RunCommand(RunCommand {
        command: PathBuf::from("/bin/sh"),
        ..Default::default()
    });
    pty.fill_cwd_from_pane_id(&mut action, &PaneId::Terminal(3));

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
    mock.set_remote_host(7, "build-box");
    let (mut pty, _plugin_rx, screen_rx) = make_pty_with_screen_receiver(mock);

    let _ = pty.close_pane(PaneId::Terminal(7));

    let hosts = collect_pane_host_instructions(&screen_rx);
    assert!(
        hosts.contains(&(7, None)),
        "closing a pane should retract its host, got {hosts:?}"
    );
}

/// Which failures the pty thread can survive is decided here and nowhere else:
/// anything this does not recognise reaches `.fatal()` and takes the session —
/// every local pane in it included — down with the pane that could not start.
#[test]
fn a_pane_that_could_not_start_names_its_terminal_and_its_reason() {
    let not_found: anyhow::Error = ZellijError::CommandNotFound {
        terminal_id: 3,
        command: "htop".to_owned(),
    }
    .into();
    let (terminal_id, message) =
        pane_start_failure(&not_found).expect("a missing command must stay survivable");
    assert_eq!(terminal_id, 3);
    assert!(
        message.contains("htop"),
        "the pane should say what was missing, got: {message}"
    );

    let unreachable: anyhow::Error = ZellijError::PaneStartFailed {
        terminal_id: 4,
        message: "could not open a pane on build-box: the broker closed the connection".to_owned(),
    }
    .into();
    let (terminal_id, message) =
        pane_start_failure(&unreachable).expect("an unreachable host must not end the session");
    assert_eq!(terminal_id, 4);
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
fn set_warpable_terminal(pty: &mut Pty, terminal_id: u32) {
    pty.terminal_cmds
        .insert(terminal_id, vec!["/bin/sh".to_owned()]);
    pty.id_to_child_pid.insert(terminal_id, 100 + terminal_id);
    pty.task_handles
        .insert(terminal_id, async_runtime().spawn(async {}));
}

fn drain_screen(
    rx: &channels::Receiver<(ScreenInstruction, ErrorContext)>,
) -> Vec<ScreenInstruction> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(instruction, _)| instruction)
        .collect()
}

/// What a pane was told about itself, by terminal id.
fn pane_messages(instructions: &[ScreenInstruction]) -> HashMap<u32, String> {
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
    set_warpable_terminal(&mut pty, 3);
    mock.refuse_to_warp(3, "pane 3 has no pty-host supervisor");

    let error = format!(
        "{:#}",
        pty.release_terminal_for_warp(3, test_warp_deadline())
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
        pty.task_handles.contains_key(&3),
        "the pane's reader must still be running — this is the whole point of asking first"
    );
    assert!(
        pty.terminal_cmds.contains_key(&3),
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
    set_warpable_terminal(&mut pty, 3);
    mock.refuse_to_warp(3, "pane 3 has no pty-host supervisor");

    let released = release_pane_for_warp(&mut pty, 3, "elsewhere", test_warp_deadline());

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
    set_warpable_terminal(&mut pty, 3);
    mock.fail_the_release_of(3);

    let released = release_pane_for_warp(&mut pty, 3, "elsewhere", test_warp_deadline());

    assert!(released.is_err(), "the release should have failed");
    let instructions = drain_screen(&screen_rx);
    let message = pane_messages(&instructions)
        .remove(&3)
        .expect("the pane should have been told what happened to it");
    assert!(
        message.contains("could not move this pane to 'elsewhere'"),
        "the pane's own account should name the session it was going to, got: {message}"
    );
    assert!(
        instructions.iter().any(|instruction| matches!(
            instruction,
            ScreenInstruction::HoldPane(PaneId::Terminal(3), ..)
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
    let terminal_ids = vec![1, 2, 3, 4];
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
        set_warpable_terminal(&mut pty, terminal_id);
    }
    let remaining = std::time::Duration::from_millis(400);

    release_tab_for_warp(&mut pty, vec![1, 2], std::time::Instant::now() + remaining);

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
    set_warpable_terminal(&mut pty, 3);

    let error = format!(
        "{:#}",
        pty.release_terminal_for_warp(3, expired_warp_deadline())
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
        pty.task_handles.contains_key(&3),
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
        set_warpable_terminal(&mut pty, terminal_id);
    }

    let released = release_tab_for_warp(&mut pty, vec![1, 2], expired_warp_deadline());

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
        set_warpable_terminal(&mut pty, terminal_id);
    }
    mock.refuse_to_warp(3, "pane 3 has no pty-host supervisor");

    let released = release_tab_for_warp(&mut pty, vec![1, 2, 3], test_warp_deadline());

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
        set_warpable_terminal(&mut pty, terminal_id);
    }
    mock.fail_the_release_of(3);

    release_tab_for_warp(&mut pty, vec![1, 3], test_warp_deadline());

    let messages = pane_messages(&drain_screen(&screen_rx));
    let orphaned = messages
        .get(&1)
        .expect("the pane handed over before the failure must say so");
    assert!(
        orphaned.contains("had already been handed over")
            && orphaned.contains("zellij pty-host adopt host-1"),
        "an orphaned pane's only way back is the recovery command, got: {orphaned}"
    );
    assert!(
        messages
            .get(&3)
            .is_some_and(|message| message.contains("this tab could not be moved")),
        "the pane whose release failed must say so too, got: {messages:?}"
    );
}

// A pane workflow's steps each carry their own context, and `rerun_command_in_pane` is where a
// step's context either survives to its `CommandPaneExited` or is clobbered by the pane's
// original one. The rule: an incoming `originating_plugin` wins and becomes the pane's record;
// the stored record is only a fallback for a command that carries none (a user's ENTER re-run).

fn step_origin(step: &str) -> zellij_utils::data::OriginatingPlugin {
    zellij_utils::data::OriginatingPlugin::new(
        2,
        1,
        std::collections::BTreeMap::from([("__step".to_owned(), step.to_owned())]),
    )
}

fn collect_command_pane_rerun_steps(
    rx: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<String> {
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
    pty.originating_plugins.insert(1, step_origin("1"));

    let replacement = RunCommand {
        command: PathBuf::from("step2"),
        hold_on_close: true,
        originating_plugin: Some(step_origin("2")),
        ..Default::default()
    };
    pty.rerun_command_in_pane(PaneId::Terminal(1), replacement)
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
            .get(&1)
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
    pty.originating_plugins.insert(1, step_origin("1"));

    let same_command_again = RunCommand {
        command: PathBuf::from("step1"),
        hold_on_close: true,
        ..Default::default()
    };
    pty.rerun_command_in_pane(PaneId::Terminal(1), same_command_again)
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
