//! The plugin thread's side of the web interface: loading the web plugin when the listener
//! binds, and handing each request to it.
//!
//! Owned by the plugin-thread loop as a local value, like its update pause; never global.

use std::time::Instant;

use crate::plugin_bridge::PluginBridge;
use crate::web_host::WebHost;
use yinz_bus::web::{ReplySlot, WebPluginParams};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::ClientId;
use yinz_types::web::WebRequest;

pub(crate) struct WebRequests {
    params: Option<WebPluginParams>,
    host: WebHost,
}

impl WebRequests {
    pub fn new(mut params: Option<WebPluginParams>, host: &WebHost) -> Self {
        if let Some(params) = &mut params {
            if let Some(run) = params.plugin.get_run_plugin() {
                params.plugin = yinz_types::input::layout::RunPluginOrAlias::RunPlugin(run);
            }
            host.configure(params);
        }
        Self {
            params,
            host: host.clone(),
        }
    }

    /// One bounded listener pulse. Binding and polling share one host budget.
    pub fn activate(&mut self, bridge: &mut PluginBridge, client: ClientId) {
        self.activate_at(bridge, client, Instant::now());
    }
    fn activate_at(&mut self, bridge: &mut PluginBridge, client: ClientId, now: Instant) {
        use crate::web_startup::Action;
        use yinz_types::web::ApplicationFailure as Failure;
        let Some(params) = &self.params else {
            return;
        };
        if let Some(id) = self.host.target() {
            self.host.permission(id, bridge.web_permission_pending(id));
        }
        match self.host.pulse(now) {
            Action::None => return,
            Action::Expire => {
                if let Some(id) = self.host.target() {
                    self.host.retire_plugin(id);
                    bridge.cancel_web_load(id);
                }
                return;
            },
            Action::Start => {},
        }
        if params.plugin.get_run_plugin().is_none() {
            self.host.start_failed(Failure::Configuration);
            return;
        }
        if let Some(id) = self.host.target() {
            // A restored healthy/loading identity already owns its admission. Do not initialize
            // it again just because listener binding happened later.
            if (self.host.selected(id).is_some() || bridge.web_loading(id))
                && self.host.status().is_some_and(|s| s.attempts == 0)
            {
                return;
            }
            if let Err(error) = bridge.unload_plugin_for_web_retry(id) {
                self.host.start_failed(Failure::Restart);
                Err::<(), _>(error)
                    .context("unload unavailable web plugin before restart")
                    .non_fatal();
                return;
            }
            self.host.set_target(None);
        }
        let client = bridge.get_first_client_id().unwrap_or(client);
        let attempts = self.host.status().map_or(0, |s| s.attempts);
        match bridge
            .start_background_plugin(params.plugin.clone(), client, true)
            .context("start the web plugin")
        {
            Ok((id, _)) => {
                // Retained failed metadata is an identity, not a live initializer. Its first
                // activation enters recovery; a subsequent eligible pulse unloads it normally.
                if !bridge.web_loading(id) && self.host.selected(id).is_none() {
                    self.host.start_failed(Failure::Initialization);
                }
            },
            Err(error) => {
                if self.host.status().is_some_and(|s| s.attempts == attempts) {
                    self.host.start_failed(Failure::Configuration);
                } else if let Some(id) = self.host.target() {
                    self.host.failed(id, None, None, Failure::Initialization);
                }
                Err::<(), _>(error).non_fatal();
            },
        }
    }

    pub fn request(
        &mut self,
        bridge: &mut PluginBridge,
        request: Box<WebRequest>,
        reply: ReplySlot,
        _client: ClientId,
        shutdown: tokio::sync::mpsc::Sender<()>,
    ) {
        if let Some(id) = self.host.target()
            && self.host.status().is_some_and(|s| s.ready())
            && let Some(generation) = self.host.selected(id)
        {
            bridge.plugin_web(id, generation, request, reply, shutdown);
        } else {
            reply.send(crate::web_host::unavailable());
        }
    }
}

/// Build the guest contract explicitly; the raw DTO contains ordered pairs instead of hashes.
pub(crate) fn guest_request(
    request: &WebRequest,
    client: Option<ClientId>,
    session: &crate::ServerSession,
) -> serde_json::Value {
    let query: serde_json::Map<String, serde_json::Value> = request
        .query
        .iter()
        .map(|(key, value)| (key.clone(), value.clone().into()))
        .collect();
    let headers: serde_json::Map<String, serde_json::Value> = request
        .headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "accept" | "accept-language" | "content-type" | "if-none-match" | "user-agent"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone().into()))
        .collect();
    serde_json::json!({ "id": request.id, "method": request.method, "path": request.path,
        "segments": request.segments, "query": query, "query_pairs": request.query,
        "headers": headers, "body": request.body, "auth": request.auth,
        "client": client, "attached": client.is_some(), "server": { "session": session.current() },
        "deadline_ms": request.deadline_ms })
}
#[cfg(test)]
#[path = "unit/web_requests.rs"]
mod tests;
