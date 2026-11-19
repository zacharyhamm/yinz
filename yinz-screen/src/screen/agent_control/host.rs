use std::time::Instant;
use yinz_types::{
    agent::{
        control::{AgentRestartSession, RestartedAgent},
        AgentInfo,
    },
    data::{OriginatingPlugin, PaneContents, PaneInfo},
    input::command::RunCommand,
    internal_ids::{TabId, TerminalId},
    pane_id::PaneId,
};

pub(in crate::screen) struct PaneInput {
    pub suspended: bool,
    pub sync_active: bool,
    pub excluded_from_sync: bool,
    pub command_present: bool,
    pub command_kind: Option<String>,
    pub bracketed_paste: bool,
}

pub(in crate::screen) struct PreparedRestart {
    pub command: RunCommand,
    pub previous: Option<AgentInfo>,
    pub current: AgentInfo,
    pub fresh: bool,
    pub result: RestartedAgent,
}

pub(in crate::screen) type RestoredObserverRegistration = (
    crate::RestoredAgentObserver,
    yinz_runtime::channels::SenderWithContext<yinz_bus::screen::ScreenInstruction>,
);

/// Observations and terminal mutations supplied by the owner of the pane layout.
pub(in crate::screen) trait AgentHost {
    fn pane_info(&self, pane: PaneId) -> Option<PaneInfo>;
    fn session_name(&self) -> &str;
    fn tab_for_pane(&self, pane: PaneId) -> Option<TabId>;
    fn terminal_cwd(&self, pane: PaneId) -> Option<String>;
    fn foreground_command(&self, id: TerminalId) -> Option<&[String]>;
    fn last_output(&self, pane: PaneId) -> Option<Instant>;
    fn pane_input(&self, pane: PaneId) -> Option<PaneInput>;
    fn viewport(&self, pane: PaneId, ansi: bool) -> Option<(usize, PaneContents)>;
    fn agent_input_revision(&self, pane: PaneId) -> u64;
    fn agent_input_at(&self, pane: PaneId) -> Option<Instant>;
    fn prepare_restart(
        &self,
        pane_id: PaneId,
        prompt: Option<&str>,
        session: AgentRestartSession,
        originating_plugin: Option<OriginatingPlugin>,
    ) -> Result<PreparedRestart, String>;
    fn apply_restart(
        &mut self,
        terminal: TerminalId,
        command: RunCommand,
        fresh: bool,
        current: &AgentInfo,
    ) -> Result<(), String>;
    fn pane_ids(&self) -> Vec<PaneId>;
    fn pane_agent(&self, pane: PaneId) -> Option<AgentInfo>;
    fn pane_was_destroyed(&self, pane: PaneId) -> bool;
    fn send_control_reply(
        &self,
        client: yinz_types::internal_ids::ClientId,
        response: yinz_types::agent::control::AgentResponse,
    );
    fn remove_event_subscriber(&self, client: yinz_types::internal_ids::ClientId);
    fn resolve_event_projects(
        &self,
        version: u64,
        agents: Vec<yinz_types::agent::control::AgentStatus>,
        projects: Vec<String>,
    );
    fn state_hooks(&self, pane: PaneId, previous: Option<&AgentInfo>, current: Option<&AgentInfo>);
    fn explicit_close_is_single_pane(&self, pane: PaneId) -> bool;
    fn dump_screen(&mut self, pane: PaneId) -> Option<String>;
    fn event_transport_available(&self) -> bool;
    fn send_event(&self, client: yinz_types::internal_ids::ClientId, event: String) -> bool;
    fn subscription_error(&self, client: yinz_types::internal_ids::ClientId, message: String);
    fn agent_process_matches(&self, pane: PaneId, kind: &str) -> bool {
        let kind = yinz_types::agent::kind_of(kind);
        kind.is_some()
            && self
                .pane_input(pane)
                .is_some_and(|input| input.command_kind == kind)
    }
    fn agent_input_enabled(&self, pane: PaneId) -> bool {
        self.pane_input(pane).is_some_and(|input| {
            !input.suspended && !(input.sync_active && input.excluded_from_sync)
        })
    }
    fn local_agent_directories(&self) -> std::collections::HashMap<PaneId, std::path::PathBuf>;
    fn set_worktree_roots(&mut self, roots: std::collections::HashMap<PaneId, std::path::PathBuf>);
    fn worktree_sender(
        &self,
    ) -> Option<yinz_runtime::channels::SenderWithContext<yinz_bus::screen::ScreenInstruction>>;
    fn set_agent(&mut self, pane: PaneId, agent: Option<AgentInfo>) -> bool;
    fn write_key(
        &mut self,
        pane: PaneId,
        key: yinz_types::keys::KeyWithModifier,
        completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()>;
    fn write_bytes(
        &mut self,
        pane: PaneId,
        bytes: Vec<u8>,
        completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()>;
    fn paste(
        &mut self,
        pane: PaneId,
        bytes: Vec<u8>,
        completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()>;
    fn write_command(
        &mut self,
        pane: PaneId,
        prefix: Vec<u8>,
        paste: Vec<u8>,
        completion: yinz_bus::route::NotificationEnd,
    ) -> yinz_runtime::errors::prelude::Result<()>;
    fn instance_id(&self) -> uuid::Uuid;
    fn held_agent_conversation(&self, pane: PaneId) -> Option<Option<&str>>;
    fn pane_is_agent(&self, pane: PaneId) -> bool;
    fn set_suppressed(&mut self, pane: PaneId, suppress: bool) -> Result<(), &'static str>;
    fn agent_enter_revision(&self, pane: PaneId) -> u64;
    fn restore_terminal_state(&mut self, pane: PaneId, state: &str);
    fn restored_observer(&self) -> Option<RestoredObserverRegistration>;
}
