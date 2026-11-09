//! Steel HTTP commands. All callers pass through the RunCommands dispatcher.
use crate::plugin_map::PluginEnv;
use yinz_runtime::errors::prelude::*;
use yinz_types::web::{WebReply, WebResponse, MAX_RESPONSE_BODY};

pub(super) const RESERVED: &str = "reserved for the configured web plugin";
pub(super) fn is_web_plugin(env: &PluginEnv) -> bool {
    env.plugin_config.is_builtin() || env.web.is_target(env.plugin_id)
}
pub(super) fn respond(env: &mut PluginEnv, request_id: u64, response: String) -> Result<()> {
    let slot = env
        .web_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request_id);
    let Some(slot) = slot.filter(|slot| !slot.is_closed()) else {
        return Ok(());
    };
    let reply = match validate_response(&response) {
        Ok(response) => WebReply::Response(response),
        Err(_) => {
            log::warn!("invalid HTTP response from plugin {}", env.plugin_id);
            WebReply::Failed("invalid HTTP response".into())
        },
    };
    slot.send(reply);
    Ok(())
}
fn validate_response(response: &str) -> Result<WebResponse> {
    let value: serde_json::Value =
        serde_json::from_str(response).context("decode HTTP response")?;
    let status = value
        .get("status")
        .and_then(|value| value.as_u64())
        .context("invalid status")?;
    anyhow::ensure!((200..=599).contains(&status), "invalid status");
    let body = value
        .get("body")
        .and_then(|value| value.as_str())
        .context("invalid body")?
        .to_owned();
    anyhow::ensure!(
        body.len() <= MAX_RESPONSE_BODY && (status != 304 || body.is_empty()),
        "invalid body size"
    );
    let headers = value
        .get("headers")
        .and_then(|value| value.as_object())
        .context("invalid headers")?;
    let mut result = Vec::new();
    for (name, value) in headers {
        let name =
            http::header::HeaderName::from_bytes(name.as_bytes()).context("invalid header name")?;
        let value = value.as_str().context("invalid header value")?;
        http::HeaderValue::from_str(value).context("invalid header value")?;
        let name = name.as_str();
        anyhow::ensure!(
            matches!(
                name,
                "content-type" | "etag" | "cache-control" | "vary" | "retry-after" | "location"
            ) || (name.starts_with("x-yinz-") && name.len() > 7),
            "disallowed header"
        );
        if name == "location" {
            anyhow::ensure!(
                (300..400).contains(&status) && safe_location(value),
                "invalid redirect"
            );
        }
        result.push((name.to_owned(), value.to_owned()));
    }
    anyhow::ensure!(
        body.is_empty() || result.iter().any(|(name, _)| name == "content-type"),
        "body needs content-type"
    );
    Ok(WebResponse {
        status: status as u16,
        body,
        headers: result,
    })
}
fn safe_location(value: &str) -> bool {
    let Ok(uri) = value.parse::<http::Uri>() else {
        return false;
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || uri.scheme().is_some()
        || uri.authority().is_some()
    {
        return false;
    }
    // Reject escapes that a browser or intermediary can normalize into authority/control syntax.
    let lower = value.to_ascii_lowercase();
    !value
        .bytes()
        .any(|byte| byte == b'\\' || byte.is_ascii_control())
        && !lower.as_bytes().windows(3).any(|part| {
            part[0] == b'%'
                && std::str::from_utf8(&part[1..])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    .is_some_and(|byte| byte.is_ascii_control() || matches!(byte, b'/' | b'\\'))
        })
}
pub(super) fn assets(env: &mut PluginEnv, prefix: String, directory: String) -> Result<()> {
    let source = env
        .resolved_source_path
        .clone()
        .context("plugin has no resolved source")?;
    let data_dir = env.yinz_data_dir.clone();
    let generation = env.generation.context("plugin has no generation")?;
    let task = yinz_bus::global_async_runtime::get_tokio_runtime()
        .spawn_blocking(move || crate::web_assets::load_directory(&data_dir, &source, &directory));
    // This callback remains part of the pinned executor's tracked job through completion.
    let files = yinz_bus::global_async_runtime::get_tokio_runtime()
        .block_on(task)
        .context("join asset freeze")??;
    env.web
        .publish_assets(env.plugin_id, generation, prefix, files)
}
pub(super) fn publish(
    env: &mut PluginEnv,
    path: String,
    content_type: String,
    body: String,
) -> Result<()> {
    let content_type = match content_type.as_str() {
        "text/html; charset=utf-8" => "text/html; charset=utf-8",
        "application/manifest+json" => "application/manifest+json",
        "text/plain; charset=utf-8" => "text/plain; charset=utf-8",
        _ => bail!("invalid page content type"),
    };
    anyhow::ensure!(body.len() <= 512 * 1024, "page exceeds 512 KiB");
    env.web.publish_page(
        env.plugin_id,
        env.generation.context("plugin has no generation")?,
        path,
        crate::web_assets::freeze(body.into_bytes(), content_type),
    )
}
#[cfg(test)]
#[path = "unit/web.rs"]
mod tests;
