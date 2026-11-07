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
        let client = bridge.get_first_client_id().unwrap_or(client);
        self.last_start = Some(Instant::now());
        let started = bridge
            .start_background_plugin(params.plugin.clone(), client, true)
            .context("start the web plugin");
        if let Ok((id, _owner)) = &started {
            self.target = Some(*id);
            self.host.set_target(Some(*id));
        }
        started.non_fatal();
    }

    /// Hand one request to the web plugin, or answer it as unavailable.
    pub fn request(
        &mut self,
        bridge: &mut PluginBridge,
        request: Box<WebRequest>,
        reply: ReplySlot,
        client: ClientId,
        _shutdown: tokio::sync::mpsc::Sender<()>,
    ) {
        if self.target.is_none() {
            self.activate(bridge, client);
            reply.send(WebReply::Unavailable {
                reason: "starting".into(),
                retry_after_s: 1,
            });
            return;
        }
        log::debug!("web request {} cannot reach the web plugin yet", request.id);
        drop(request);
        reply.send(WebReply::Unavailable {
            reason: "web plugin unavailable".into(),
            retry_after_s: 5,
        });
    }
}
