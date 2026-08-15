use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use interprocess::local_socket::Stream as LocalSocketStream;
use zellij_server::os_input_output::{AdoptedTerminal, AsyncReader, ServerOsApi};
use zellij_server::panes::PaneId;
use zellij_server::pty_host::client::ReleasedPtyHost;
use zellij_server::ClientId;
use zellij_utils::data::Palette;
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg,
};
use zellij_utils::shared::default_palette;

use crate::fake_pty::{SharedPtys, FAKE_PID_BASE};

#[derive(Clone)]
struct NonBlockingClientSender {
    buffer_tx: crossbeam::channel::Sender<ServerToClientMsg>,
}

impl NonBlockingClientSender {
    fn new(mut ipc_sender: IpcSenderWithContext<ServerToClientMsg>) -> Self {
        let (buffer_tx, buffer_rx) = crossbeam::channel::bounded::<ServerToClientMsg>(5000);
        std::thread::Builder::new()
            .name("non_blocking_client_sender".to_string())
            .spawn(move || {
                for msg in buffer_rx.iter() {
                    if ipc_sender.send_server_msg(msg).is_err() {
                        break;
                    }
                }
            })
            .unwrap();
        NonBlockingClientSender { buffer_tx }
    }
}

/// The last resize each fake pty was told about, mirroring the real server's cache.
type CachedResizes = Arc<Mutex<Option<BTreeMap<u32, (u16, u16)>>>>;

#[derive(Clone, Default)]
pub struct FakeServerOsApi {
    pub shared_ptys: SharedPtys,
    non_blocking_client_senders: Arc<Mutex<HashMap<ClientId, NonBlockingClientSender>>>,
    fake_filesystem: Arc<Mutex<HashMap<String, String>>>,
    // mirrors the resize caching of the real ServerOsInputOutput, so that tests see the same
    // resizes a real pane's pty would
    cached_resizes: CachedResizes,
    /// The folder each fake pane was spawned in, keyed by its child pid, so that
    /// `get_cwd` answers the way a real one does. Without it every pane looks
    /// like it is nowhere, and anything that reads a pane's folder — project
    /// detection above all — cannot be tested end to end.
    cwds: Arc<Mutex<HashMap<u32, PathBuf>>>,
}

impl FakeServerOsApi {
    pub fn written_files(&self) -> HashMap<String, String> {
        self.fake_filesystem.lock().unwrap().clone()
    }

    pub fn connected_client_count(&self) -> usize {
        self.non_blocking_client_senders.lock().unwrap().len()
    }
}

impl ServerOsApi for FakeServerOsApi {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: u32,
        cols: u16,
        rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        if let Some(cached_resizes) = self.cached_resizes.lock().unwrap().as_mut() {
            cached_resizes.insert(id, (cols, rows));
            return Ok(());
        }
        self.shared_ptys.set_size(id, cols, rows);
        Ok(())
    }
    fn cache_resizes(&mut self) {
        let mut cached_resizes = self.cached_resizes.lock().unwrap();
        if cached_resizes.is_none() {
            *cached_resizes = Some(BTreeMap::new());
        }
    }
    fn apply_cached_resizes(&mut self) -> Vec<(u32, u16, u16)> {
        let cached_resizes = self.cached_resizes.lock().unwrap().take();
        let cached_resizes = cached_resizes.unwrap_or_default();
        for (terminal_id, (cols, rows)) in &cached_resizes {
            self.shared_ptys.set_size(*terminal_id, *cols, *rows);
        }
        cached_resizes
            .into_iter()
            .map(|(terminal_id, (cols, rows))| (terminal_id, cols, rows))
            .collect()
    }
    fn spawn_terminal(
        &self,
        terminal_action: TerminalAction,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        let terminal_id = self.shared_ptys.next_terminal_id();
        let opened_file_contents = match &terminal_action {
            TerminalAction::OpenFile(payload) => self
                .fake_filesystem
                .lock()
                .unwrap()
                .get(&payload.path.to_string_lossy().to_string())
                .cloned(),
            _ => None,
        };
        let requested_cwd = match &terminal_action {
            TerminalAction::RunCommand(run_command) => run_command.cwd.clone(),
            TerminalAction::OpenFile(payload) => payload.cwd.clone(),
        };
        if let Some(cwd) = requested_cwd {
            self.cwds
                .lock()
                .unwrap()
                .insert(FAKE_PID_BASE + terminal_id, cwd);
        }
        let fake_async_reader =
            self.shared_ptys
                .register(terminal_id, Some(terminal_action), Some(quit_cb));
        if let Some(contents) = opened_file_contents {
            let contents_with_carriage_returns =
                contents.replace("\r\n", "\n").replace('\n', "\r\n");
            self.shared_ptys
                .write_output(terminal_id, contents_with_carriage_returns.as_bytes());
        }
        Ok((
            terminal_id,
            fake_async_reader,
            Some(FAKE_PID_BASE + terminal_id),
        ))
    }
    fn reserve_terminal_id(&self) -> Result<u32> {
        let terminal_id = self.shared_ptys.next_terminal_id();
        let _ = self.shared_ptys.register(terminal_id, None, None);
        Ok(terminal_id)
    }
    fn write_to_tty_stdin(&self, terminal_id: u32, buf: &[u8]) -> Result<usize> {
        if self.shared_ptys.append_stdin(terminal_id, buf) {
            Ok(buf.len())
        } else {
            Err(anyhow!("no fake pty for terminal id {terminal_id}"))
        }
    }
    fn tcdrain(&self, _terminal_id: u32) -> Result<()> {
        Ok(())
    }
    fn kill(&self, pid: u32) -> Result<()> {
        self.shared_ptys
            .exit_terminal(pid.saturating_sub(FAKE_PID_BASE), None);
        Ok(())
    }
    fn force_kill(&self, pid: u32) -> Result<()> {
        self.shared_ptys
            .exit_terminal(pid.saturating_sub(FAKE_PID_BASE), None);
        Ok(())
    }
    fn send_sigint(&self, pid: u32) -> Result<()> {
        self.shared_ptys
            .exit_terminal(pid.saturating_sub(FAKE_PID_BASE), None);
        Ok(())
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new(self.clone())
    }
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()> {
        if let Some(non_blocking_client_sender) = self
            .non_blocking_client_senders
            .lock()
            .unwrap()
            .get(&client_id)
        {
            non_blocking_client_sender
                .buffer_tx
                .try_send(msg)
                .with_context(|| format!("failed to buffer message for client {client_id}"))?;
        }
        Ok(())
    }
    fn new_client(
        &mut self,
        client_id: ClientId,
        stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        let ipc_receiver = IpcReceiverWithContext::new(stream);
        let non_blocking_client_sender = NonBlockingClientSender::new(ipc_receiver.get_sender());
        self.non_blocking_client_senders
            .lock()
            .unwrap()
            .insert(client_id, non_blocking_client_sender);
        Ok(ipc_receiver)
    }
    fn remove_client(&mut self, client_id: ClientId) -> Result<()> {
        self.non_blocking_client_senders
            .lock()
            .unwrap()
            .remove(&client_id);
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        default_palette()
    }
    fn get_cwd(&self, pid: u32) -> Option<PathBuf> {
        self.cwds.lock().unwrap().get(&pid).cloned()
    }
    fn get_cwds(&self, pids: Vec<u32>) -> (HashMap<u32, PathBuf>, HashMap<u32, Vec<String>>) {
        // A fake pane runs no process of its own, so both halves are answered from
        // what it was spawned with rather than from the OS. The command half used
        // to be left empty, which left every pane looking like it ran nothing —
        // and agent detection reads exactly that.
        let process_commands = self.shared_ptys.process_commands();
        let cwds = self.cwds.lock().unwrap();
        let mut found_cwds = HashMap::new();
        let mut found_cmds = HashMap::new();
        for pid in pids {
            if let Some(cwd) = cwds.get(&pid) {
                found_cwds.insert(pid, cwd.clone());
            }
            if let Some(command) = pid
                .checked_sub(FAKE_PID_BASE)
                .and_then(|terminal_id| process_commands.get(&terminal_id))
            {
                found_cmds.insert(pid, command.clone());
            }
        }
        (found_cwds, found_cmds)
    }

    /// What holds each pane's tty foreground. Keyed by terminal id, as the real
    /// one is — it reaches the foreground process group through `tcgetpgrp`, not
    /// through the pane's own pid.
    fn get_foreground_cmds(
        &self,
        panes: &[(u32, u32)],
        _post_hook: &Option<String>,
    ) -> HashMap<u32, Vec<String>> {
        let foreground_commands = self.shared_ptys.foreground_commands();
        panes
            .iter()
            .filter_map(|(terminal_id, _shell_pid)| {
                Some((*terminal_id, foreground_commands.get(terminal_id)?.clone()))
            })
            .collect()
    }
    fn write_to_file(&mut self, buf: String, file: Option<String>) -> Result<()> {
        if let Some(file) = file {
            self.fake_filesystem.lock().unwrap().insert(file, buf);
        }
        Ok(())
    }
    fn re_run_command_in_terminal(
        &self,
        terminal_id: u32,
        _run_command: RunCommand,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        let fake_async_reader = self.shared_ptys.rerun(terminal_id, quit_cb);
        Ok((fake_async_reader, Some(FAKE_PID_BASE + terminal_id)))
    }
    fn clear_terminal_id(&self, terminal_id: u32) -> Result<()> {
        self.shared_ptys.remove(terminal_id);
        Ok(())
    }
    /// Hand a pane's supervisor over without killing it.
    ///
    /// A real one detaches a process the server never owned in the first place;
    /// the fake moves its pty state somewhere neither session's registry can see
    /// until the other one asks for it. `host_pid` is not modelled — nothing here
    /// has a supervisor process — but the control socket is, because that is the
    /// only thing the receiving session is told and the only thing it can get
    /// wrong.
    fn release_terminal_for_warp(&self, terminal_id: u32) -> Result<ReleasedPtyHost> {
        let control_socket = self
            .shared_ptys
            .release_for_warp(terminal_id)
            .with_context(|| format!("no fake pty for terminal id {terminal_id}"))?;
        Ok(ReleasedPtyHost {
            host_id: format!("fake-pty-host-{terminal_id}"),
            host_pid: FAKE_PID_BASE + terminal_id,
            control_socket: Some(control_socket),
        })
    }
    fn adopt_terminal(
        &self,
        control_socket: &Path,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let adopted = self
            .shared_ptys
            .adopt_for_warp(control_socket, quit_cb)
            .with_context(|| format!("no supervisor is waiting at {}", control_socket.display()))?;
        // The pane's folder follows it: `get_cwd` is answered out of a map keyed
        // by child pid, and this session has never seen this pane before.
        if let Some(cwd) = adopted.run_command.cwd.clone() {
            self.cwds
                .lock()
                .unwrap()
                .insert(FAKE_PID_BASE + adopted.terminal_id, cwd);
        }
        Ok(AdoptedTerminal {
            terminal_id: adopted.terminal_id,
            reader: adopted.reader,
            // Renumbered with the terminal id, as every other fake pid is. A real
            // warp keeps the child's pid, which is the evidence that the process
            // survived; here the evidence is that the pty *state* travelled -
            // its stdin, its sizes, the command it was spawned with - and that it
            // was never told to exit.
            child_pid: FAKE_PID_BASE + adopted.terminal_id,
            cmd: adopted.run_command,
            // Not modelled: a real supervisor buffers what the child wrote while
            // nobody was attached. The fake has no way to be written to while it
            // is detached, so there is never anything to replay.
            buffered: Vec::new(),
            child_exit_status: adopted.child_exit_status,
        })
    }
}
