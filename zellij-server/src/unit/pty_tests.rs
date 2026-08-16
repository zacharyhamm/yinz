use super::*;
use crate::os_input_output::ServerOsApi;
use crate::plugins::PluginInstruction;
use crate::thread_bus::Bus;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zellij_utils::channels::{self, SenderWithContext};
use zellij_utils::data::{Event, Palette};
use zellij_utils::errors::ErrorContext;
use zellij_utils::input::command::RunCommand;
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
        }
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
        _: u32,
        _: RunCommand,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> anyhow::Result<(Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _: u32) -> anyhow::Result<()> {
        Ok(())
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
