//! Pane and agent lifecycle hooks.
use super::agent_hook_edges;
use super::Screen;
use crate::panes::PaneId;
use crate::plugins::HookFiring;
use crate::plugins::PluginInstruction;
use crate::ClientId;
use std::collections::BTreeMap;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::AgentInfo;
use yinz_types::data::HookPayload;
use yinz_types::data::AGENT_PANE_CONTEXT;
use yinz_types::input::command::RunCommand;
use yinz_types::input::hooks::HookPoint;

impl Screen {
    /// Ask the plugin thread to run the plugin bound to `point`.
    ///
    /// Fired unconditionally by every hook site - whether a hook is bound is the plugin
    /// thread's question, so that the answer lives in one place.
    pub(super) fn fire_hook(
        &self,
        point: HookPoint,
        pane_id: Option<PaneId>,
        payload: HookFiring,
        client_id: ClientId,
    ) {
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                client_id,
            })
            .non_fatal();
    }

    /// The client a hook about a pane is run for: the one that spawned the pane, or the first
    /// one where nobody did. `None` when there is no client at all, which is no one to run a
    /// hook for.
    pub(super) fn hook_client_id(&self, run_command: Option<&RunCommand>) -> Option<ClientId> {
        run_command
            .and_then(|run_command| run_command.originating_plugin.as_ref())
            .map(|plugin| plugin.client_id)
            .or_else(|| self.get_first_client_id())
    }

    /// Hand an exited agent pane to the `agent_exit` hook.
    ///
    /// This is what used to be a branch in the terminal emulator: `TerminalPane::hold` painted a
    /// fixed question over the pane and `adjust_input_to_terminal` implemented its two answers.
    /// Now the pane just holds, and the plugin bound to the hook point is opened in its place.
    ///
    /// An agent pane is one the pane itself vouches for (`pane_is_agent`: a record, however it
    /// got there - the spawn path, a plugin, or the agent's own hooks - or the memory of one),
    /// or - for a pane spawned by an older plugin - one whose originating context carries the
    /// marker.
    ///
    /// The memory rather than the live record, because a clean exit clears the record first:
    /// Claude Code's `SessionEnd` hook runs before the process does, and `hook_outcome` reads
    /// it as the end of the record. The context marker used to cover that for a pane agent
    /// spawned, and only for one - a pane warped in from another session has none, because the
    /// supervisor drops an `originating_plugin` that names a plugin in the session it left.
    pub(super) fn fire_agent_exit_hook(
        &self,
        pane_id: PaneId,
        exit_status: Option<i32>,
        run_command: &RunCommand,
        pane_is_agent: bool,
    ) {
        let originating_plugin = run_command.originating_plugin.as_ref();
        let context_says_agent = originating_plugin
            .is_some_and(|plugin| plugin.context.contains_key(AGENT_PANE_CONTEXT));
        if !(pane_is_agent || context_says_agent) {
            return;
        }
        let Some(client_id) = self.hook_client_id(Some(run_command)) else {
            return;
        };
        let PaneId::Terminal(terminal_pane_id) = pane_id else {
            return;
        };
        // Strings, because a plugin's configuration is a `BTreeMap<String, String>`.
        // RestartAgentPane reads the structured command from the held pane.
        //
        // Every key is prefixed. `PluginUserConfiguration::new` silently drops a reserved word -
        // an unprefixed `cwd` arrives as nothing at all - and a payload key blanking itself
        // because the host grew a configuration option of the same name would be a bad way to
        // find that out.
        let tab = self.tab_containing_pane(pane_id);
        let pane = tab.and_then(|tab| tab.get_pane_with_id(pane_id));
        let mut payload = BTreeMap::new();
        payload.insert("agent_pane_id".to_owned(), terminal_pane_id.to_string());
        if let Some(pane) = pane {
            payload.insert("agent_pane_title".to_owned(), pane.display_title());
        }
        if let Some(exit_status) = exit_status {
            payload.insert("agent_exit_code".to_owned(), exit_status.to_string());
        }
        payload.insert(
            "agent_cwd".to_owned(),
            run_command
                .cwd
                .as_ref()
                .map(|cwd| cwd.display().to_string())
                .unwrap_or_default(),
        );
        if let Some(cwd) = tab.and_then(|tab| tab.pane_last_agent_cwd(pane_id)) {
            payload.insert("agent_cwd".to_owned(), cwd);
        }
        if let Some(session_id) = pane.and_then(|pane| pane.last_agent_session_id()) {
            payload.insert("agent_session_id".to_owned(), session_id.to_owned());
        }
        payload.insert("agent_command".to_owned(), run_command.to_string());
        let kind = tab
            .and_then(|tab| tab.pane_last_agent_kind(pane_id))
            .or_else(|| {
                originating_plugin
                    .and_then(|plugin| plugin.context.get(AGENT_PANE_CONTEXT).cloned())
            });
        if let Some(kind) = kind {
            payload.insert("agent_kind".to_owned(), kind);
        }
        payload.insert(
            "agent_target".to_owned(),
            match &run_command.target {
                yinz_types::command::PaneTarget::Local => "local",
                yinz_types::command::PaneTarget::Ssh { .. } => "ssh",
                yinz_types::command::PaneTarget::Bubblewrap { .. } => "bubblewrap",
            }
            .to_owned(),
        );
        self.fire_hook(
            HookPoint::AgentExit,
            Some(pane_id),
            HookFiring::ReplacePane(payload),
            client_id,
        );
    }

    /// Hand any command pane's exit to the `pane_exit` hook, held or not.
    ///
    /// Fired before `agent_exit` for a pane that is both, so the observer hears of the exit
    /// before the pane is replaced. The keys carry an `exit_` prefix for the same reason
    /// `agent_exit`'s carry `agent_`: one vocabulary across the hook payloads.
    pub(super) fn fire_pane_exit_hook(
        &self,
        pane_id: PaneId,
        exit_status: Option<i32>,
        run_command: &RunCommand,
        held: bool,
    ) {
        let Some(client_id) = self.hook_client_id(Some(run_command)) else {
            return;
        };
        let PaneId::Terminal(terminal_pane_id) = pane_id else {
            return;
        };
        let payload = HookPayload::PaneExit {
            pane_id: terminal_pane_id,
            exit_code: exit_status,
            cwd: run_command.cwd.clone(),
            command: Some(run_command.to_string()),
            // Whether the pane is still there to be re-run, or is closing with its command.
            held,
        };
        self.fire_hook(
            HookPoint::PaneExit,
            Some(pane_id),
            HookFiring::Headless(payload),
            client_id,
        );
    }

    /// Fire the hooks an agent record's change calls for: `agent_start` and `agent_waiting`,
    /// as [`agent_hook_edges`] reads them off the record before and after.
    pub(super) fn fire_agent_state_hooks(
        &self,
        pane_id: PaneId,
        previous: Option<&AgentInfo>,
        current: Option<&AgentInfo>,
    ) {
        let edges = agent_hook_edges(previous, current);
        if edges.is_empty() {
            return;
        }
        let Some(current) = current else {
            return;
        };
        let PaneId::Terminal(terminal_pane_id) = pane_id else {
            return;
        };
        let run_command = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_run_command(pane_id));
        let Some(client_id) = self.hook_client_id(run_command.as_ref()) else {
            return;
        };
        // The record as it stands, with the pane's folder filled in where the agent has not
        // said where it works.
        let mut agent = current.clone();
        if agent.cwd.is_none() {
            agent.cwd = run_command
                .as_ref()
                .and_then(|run_command| run_command.cwd.as_ref())
                .map(|cwd| cwd.display().to_string());
        }
        let payload = HookPayload::Agent {
            pane_id: terminal_pane_id,
            agent,
            previous_state: previous.map(|previous| previous.state),
            command: run_command
                .as_ref()
                .map(|run_command| run_command.to_string()),
        };
        for point in edges {
            self.fire_hook(
                point,
                Some(pane_id),
                HookFiring::Headless(payload.clone()),
                client_id,
            );
        }
    }

    /// A pane that was born with an agent record - the spawn path wrote it - has started.
    pub(super) fn fire_agent_start_for_new_pane(&self, pane_id: PaneId) {
        let agent = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id));
        if let Some(agent) = agent {
            self.fire_agent_state_hooks(pane_id, None, Some(&agent));
        }
    }
}
