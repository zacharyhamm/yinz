//! The OS API the route-thread tests hand to `route_thread_main`.

use crate::os_input_output::{AsyncReader, ServerOsApi};
use crate::panes::PaneId;
use crate::ClientId;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::collections::HashMap;
use std::path::PathBuf;
use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_test_support::Watched;
use yinz_types::data::Palette;
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::internal_ids::{ProcessId, TerminalId};

/// Every message the route thread sent to each client, shared with the `CapturingOs` that
/// records them.
type CapturedClientMessages = Watched<HashMap<ClientId, Vec<ServerToClientMsg>>>;

/// An OS API that records what a route thread sends its client. Clones share the record, so a
/// test keeps one and hands the route thread another. Nothing else is reachable from the route
/// threads these tests run, and reaching anything else is the failure.
#[derive(Clone, Default)]
pub(crate) struct CapturingOs {
    pub(crate) server_to_client_messages: CapturedClientMessages,
}

impl ServerOsApi for CapturingOs {
    fn set_terminal_size_using_terminal_id(
        &self,
        _: TerminalId,
        _: u16,
        _: u16,
        _: Option<u16>,
        _: Option<u16>,
    ) -> Result<()> {
        unimplemented!()
    }
    fn spawn_terminal(
        &self,
        _: TerminalAction,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, _: TerminalId, _: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn kill(&self, _: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigcont(&self, _: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new(self.clone())
    }
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()> {
        self.server_to_client_messages
            .lock()
            .entry(client_id)
            .or_default()
            .push(msg);
        Ok(())
    }
    fn new_client(
        &mut self,
        _: ClientId,
        _: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _: ClientId) -> Result<()> {
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        unimplemented!()
    }
    fn get_cwd(&self, _: ProcessId) -> Option<PathBuf> {
        unimplemented!()
    }
    fn write_to_file(&mut self, _: String, _: Option<String>) -> Result<()> {
        unimplemented!()
    }
    fn re_run_command_in_terminal(
        &self,
        _: TerminalId,
        _: RunCommand,
        _: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _: TerminalId) -> Result<()> {
        unimplemented!()
    }
}
