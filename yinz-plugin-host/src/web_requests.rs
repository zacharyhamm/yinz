//! The plugin thread's side of the web interface: loading the web plugin when the listener
//! binds, and handing each request to it.
//!
//! Owned by the plugin-thread loop as a local value, like its update pause; never global.

use std::time::Instant;

use crate::plugin_bridge::PluginBridge;
use crate::web_host::WebHost;
use yinz_bus::web::{ReplySlot, WebPluginParams};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::{ClientId, PluginId};
use yinz_types::web::{WebReply, WebRequest};

pub(crate) struct WebRequests {
    params: Option<WebPluginParams>,
    host: WebHost,
    target: Option<PluginId>,
    last_start: Option<Instant>,
}

impl WebRequests {
    pub fn new(params: Option<WebPluginParams>, host: &WebHost) -> Self {
        WebRequests {
            params,
            host: host.clone(),
            target: None,
            last_start: None,
        }
    }

    /// Load the configured web plugin; nothing when the session's web interface is off.
    pub fn activate(&mut self, bridge: &mut PluginBridge, client: ClientId) {
        let Some(params) = &self.params else {
            return;
        };
        if self
            .target
            .is_some_and(|id| self.host.selected(id).is_some() || bridge.web_loading(id))
            || !self.start_due()
        {
            return;
        }
        if let Some(id) = self.target.take()
            && let Err(error) = bridge.unload_plugin(id)
        {
            self.target = Some(id);
            Err::<(), _>(error)
                .context("unload unavailable web plugin before restart")
                .non_fatal();
            return;
        }
        let client = bridge.get_first_client_id().unwrap_or(client);
        self.last_start = Some(Instant::now());
        // The target is recorded before the guest initializes, so the web plugin can publish
        // its site from `load`.
        let host = &self.host;
        let started = bridge
            .start_background_plugin_then(params.plugin.clone(), client, true, |id| {
                host.set_target(Some(id));
            })
            .context("start the web plugin");
        match started {
            Ok((id, _owner)) => self.target = Some(id),
            Err(error) => {
                self.target = None;
                self.host.set_target(None);
                Err::<(), _>(error).non_fatal();
            },
        }
    }

    fn start_due(&self) -> bool {
        self.last_start
            .is_none_or(|started| started.elapsed() >= std::time::Duration::from_secs(30))
    }

    /// Hand one request to the web plugin, or answer it as unavailable.
    pub fn request(
        &mut self,
        bridge: &mut PluginBridge,
        request: Box<WebRequest>,
        reply: ReplySlot,
        client: ClientId,
        shutdown: tokio::sync::mpsc::Sender<()>,
    ) {
        if self.params.is_none() {
            drop(request);
            reply.send(WebReply::Unavailable {
                reason: "web plugin unavailable".into(),
                retry_after_s: 5,
            });
            return;
        }
        if self.target.is_none() {
            if !self.start_due() {
                reply.send(crate::web_host::unavailable());
                return;
            }
            self.activate(bridge, client);
            reply.send(WebReply::Unavailable {
                reason: "starting".into(),
                retry_after_s: 1,
            });
            return;
        }
        let id = self.target.expect("target checked above");
        if let Some(generation) = self.host.selected(id) {
            bridge.plugin_web(id, generation, request, reply, shutdown);
        } else if bridge.web_loading(id) {
            reply.send(WebReply::Unavailable {
                reason: "starting".into(),
                retry_after_s: 1,
            });
        } else {
            if self.start_due() {
                self.activate(bridge, client);
            }
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
