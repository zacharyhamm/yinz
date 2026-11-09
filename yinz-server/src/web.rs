//! Session-owned bridge; the web crate has no dependency on bus or plugin internals.
use crate::plugins::PluginInstruction;
use std::sync::{atomic::Ordering, Arc};
use yinz_bus::{
    thread_bus::ThreadSenders,
    web::{ReplySlot, WebInstruction, WebPluginParams},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::web::{Site, SiteHandle, WebRequest};
use yinz_web::{Backend, DispatchFuture, WebSettings};

pub(crate) struct PluginBackend {
    senders: ThreadSenders,
    site: SiteHandle,
}
impl PluginBackend {
    pub(crate) fn new(senders: ThreadSenders, site: SiteHandle) -> Self {
        Self { senders, site }
    }
}
impl Backend for PluginBackend {
    fn dispatch(&self, request: WebRequest) -> DispatchFuture {
        let (reply, receiver) = ReplySlot::new();
        let sent = self
            .senders
            .send_to_plugin(PluginInstruction::Web(WebInstruction::Request {
                request: Box::new(request),
                reply,
            }))
            .is_ok();
        Box::pin(async move {
            if sent {
                receiver.await.ok()
            } else {
                None
            }
        })
    }
    fn site(&self) -> Option<Arc<Site>> {
        self.site.current()
    }
    fn quiesced(&self) -> bool {
        self.senders.session_update_quiesced.load(Ordering::Acquire)
    }
    fn activate(&self) {
        self.senders
            .send_to_plugin(PluginInstruction::Web(WebInstruction::Activate))
            .context("activate web plugin")
            .non_fatal();
    }
    fn session_name(&self) -> String {
        self.senders.session.current().to_string()
    }
}

pub(crate) fn configure(
    config: &yinz_config::config::Config,
    data_dir: &std::path::Path,
) -> Result<Option<(WebSettings, WebPluginParams)>> {
    if !config.web.enabled() {
        return Ok(None);
    }
    let settings = WebSettings::from_config(&config.web, data_dir)?;
    let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        &config.web.plugin_or_default(),
        &None,
        Some(&config.plugins),
        None,
    )
    .map_err(|e| anyhow!(e))
    .context("resolve web plugin")?;
    Ok(Some((
        settings,
        WebPluginParams {
            plugin,
            site: SiteHandle::default(),
        },
    )))
}

#[cfg(test)]
#[path = "unit/web.rs"]
mod tests;
