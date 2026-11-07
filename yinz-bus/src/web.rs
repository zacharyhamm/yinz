//! Bus payloads of the mobile web interface.
//!
//! The HTTP server (`yinz-web`, wired by `yinz-server`) sends [`WebInstruction`]s to the plugin
//! thread inside `PluginInstruction::Web`; the plugin host's agent view asks the screen for
//! [`WebLiveAgent`] snapshots through `ScreenInstruction::WebLiveAgents`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::command_palette::AgentLabelSource;
use yinz_types::data::PaneInfo;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::{ClientId, PluginId, SessionName};
use yinz_types::web::{SiteHandle, WebReply, WebRequest};

/// Where the answer to one web request goes.
///
/// Shared so the first send wins on any clone; `PluginInstruction` is `Clone`, so a request may
/// be copied on its way to the plugin, and every copy holds the same one-shot sender.
#[derive(Clone)]
pub struct ReplySlot(Arc<Mutex<Option<tokio::sync::oneshot::Sender<WebReply>>>>);

impl ReplySlot {
    pub fn new() -> (Self, tokio::sync::oneshot::Receiver<WebReply>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (ReplySlot(Arc::new(Mutex::new(Some(sender)))), receiver)
    }

    /// Deliver `reply`. False when another clone already answered or the receiver is gone.
    pub fn send(&self, reply: WebReply) -> bool {
        let sender = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        sender.is_some_and(|sender| sender.send(reply).is_ok())
    }

    /// Whether nothing can be delivered any more: answered, or the HTTP side stopped waiting.
    pub fn is_closed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_none_or(|sender| sender.is_closed())
    }
}

impl std::fmt::Debug for ReplySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReplySlot")
    }
}

#[derive(Clone, Debug)]
pub enum WebInstruction {
    /// An authenticated request for the configured web plugin.
    Request {
        request: Box<WebRequest>,
        reply: ReplySlot,
    },
    /// The listener bound its port: load the web plugin in this session.
    Activate,
    /// A web plugin's `AgentView` command, run natively off the plugin executor.
    AgentView(Box<AgentViewCall>),
}

/// One `PluginCommand::AgentView`, with what is needed to route its reply back as
/// `CustomMessage("agent-view-response", {"id","reply"})`.
#[derive(Clone)]
pub struct AgentViewCall {
    pub plugin_id: PluginId,
    pub client: ClientId,
    pub generation: Option<u64>,
    pub id: String,
    /// The JSON operation (`{"op":"list"}`, …).
    pub request: String,
}

/// Prints the request's length only: it can carry prompt text.
impl std::fmt::Debug for AgentViewCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentViewCall")
            .field("plugin_id", &self.plugin_id)
            .field("client", &self.client)
            .field("generation", &self.generation)
            .field("id", &self.id)
            .field("request_len", &self.request.len())
            .finish()
    }
}

/// The plugin thread's web input. `Some` only when the session's `web { }` block enables it.
#[derive(Clone, Debug)]
pub struct WebPluginParams {
    pub plugin: RunPluginOrAlias,
    /// Shared with the HTTP server, which serves whatever the plugin froze into it.
    pub site: SiteHandle,
}

/// One live agent as the palette shows it, computed on the screen thread.
#[derive(Clone, Debug)]
pub struct WebLiveAgent {
    /// `LiveAgent::id()`.
    pub id: String,
    pub session: SessionName,
    pub current_session: bool,
    pub pane: PaneInfo,
    pub metadata: BTreeMap<String, String>,
    /// `LiveAgent::run()`.
    pub run: Option<String>,
    /// Exactly what the palette sends for labels.
    pub label_source: AgentLabelSource,
    /// `AgentPresentation::from_pane`.
    pub provider: String,
    pub activity: String,
    pub host: Option<String>,
    pub pane_label: String,
    /// `yinz_panes::ui::agent_title::status`.
    pub glyph: &'static str,
    pub status: &'static str,
    /// In the current session, and the active pane of a connected client.
    pub focused: bool,
    pub selectable: bool,
    pub suppressed: bool,
    pub usage_unavailable: Option<&'static str>,
    /// `LiveAgent::text()`.
    pub details: String,
    pub header: WebLiveHeader,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebLiveHeader {
    pub name: String,
    pub status: String,
    pub location: String,
    pub task: String,
    pub project: String,
    pub session_id: Option<String>,
}

/// The identity a browser holds for an agent: actions are refused when it no longer matches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAgentRef {
    pub id: String,
    pub kind: String,
    pub session_id: String,
}

impl WebLiveAgent {
    /// Same live-agent id, agent kind and agent session id, none of them empty.
    pub fn matches(&self, r: &WebAgentRef) -> bool {
        let Some(agent) = self.pane.agent.as_ref() else {
            return false;
        };
        !r.id.is_empty()
            && !r.kind.is_empty()
            && !r.session_id.is_empty()
            && self.id == r.id
            && agent.kind == r.kind
            && agent.session_id.as_deref() == Some(r.session_id.as_str())
    }
}

#[cfg(test)]
#[path = "unit/web_tests.rs"]
mod tests;
