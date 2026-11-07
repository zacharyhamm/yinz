//! Handlers for the Steel-only web commands: `HttpRespond`, `HttpAssets` and `HttpPublish`.
//!
//! `AgentView` is dispatched in `yinz_exports.rs` itself: it only forwards to the plugin
//! thread, which runs the native agent view off the plugin executor.

use crate::plugin_map::PluginEnv;
use yinz_runtime::errors::prelude::*;

/// The error a guest sees when it uses a command reserved for the configured web plugin.
pub(super) const RESERVED: &str = "reserved for the configured web plugin";

/// Whether this instance may publish the site and call the agent view: a builtin plugin, or
/// the plugin the session's `web { }` block configured.
pub(super) fn is_web_plugin(env: &PluginEnv) -> bool {
    env.plugin_config.is_builtin()
}

/// Answer one pending web request delivered to this instance.
pub(super) fn respond(_env: &mut PluginEnv, request_id: u64, _response: String) -> Result<()> {
    log::debug!("web request {request_id} has no pending reply slot; ignoring the response");
    Ok(())
}

/// Freeze a directory of static assets under a URL prefix.
pub(super) fn assets(_env: &mut PluginEnv, prefix: String, _directory: String) -> Result<()> {
    Err(anyhow!(
        "web assets for {prefix} cannot be published by this build"
    ))
}

/// Publish a public, pre-rendered page.
pub(super) fn publish(
    _env: &mut PluginEnv,
    path: String,
    _content_type: String,
    _body: String,
) -> Result<()> {
    Err(anyhow!("web page {path} cannot be published by this build"))
}
