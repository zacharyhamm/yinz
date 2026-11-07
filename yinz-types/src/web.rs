//! The native-to-Steel seam of the mobile web interface.
//!
//! `yinz-web` turns an authenticated HTTP request into a [`WebRequest`], the plugin thread hands
//! it to the configured web plugin, and the plugin answers with a [`WebReply`]. Public pages and
//! static assets never reach the plugin: the plugin freezes them into a [`Site`] when it loads,
//! and native code serves that snapshot through the shared [`SiteHandle`].
//!
//! Only `std` and `serde`: the HTTP server, the bus and the plugin host all name these types.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;

/// The largest request body forwarded to the plugin.
pub const MAX_REQUEST_BODY: usize = 64 * 1024;
/// The largest response body a plugin may answer with.
pub const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;

/// The methods that can reach a plugin. Everything else is refused natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WebMethod {
    Get,
    Head,
    Post,
}

/// The authenticated browser session a request belongs to. The bearer token itself never
/// leaves native code; the plugin sees only the session's public id and label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WebAuth {
    pub session: String,
    pub label: String,
}

/// One authenticated request, normalized and bounded by `yinz-web` before it is forwarded.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct WebRequest {
    pub id: u64,
    pub method: WebMethod,
    pub path: String,
    /// The percent-decoded path segments, without empty segments.
    pub segments: Vec<String>,
    /// Decoded query pairs, in request order.
    pub query: Vec<(String, String)>,
    /// Lowercase allowlist: accept, accept-language, content-type, if-none-match, user-agent.
    pub headers: Vec<(String, String)>,
    /// UTF-8 body of a POST, at most [`MAX_REQUEST_BODY`] bytes.
    pub body: Option<String>,
    pub auth: WebAuth,
    /// Unix milliseconds after which native code stops waiting for the reply.
    pub deadline_ms: i64,
}

/// Bodies and queries may carry prompts and other user text, so they are never logged.
impl std::fmt::Debug for WebRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("body_len", &self.body.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

/// What the plugin thread answers for one [`WebRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebReply {
    /// A validated plugin response.
    Response(WebResponse),
    /// The plugin cannot answer now; native code replies 503 with `Retry-After`.
    Unavailable { reason: String, retry_after_s: u32 },
    /// The plugin failed or answered something invalid; native code replies 502.
    Failed(String),
}

/// A plugin response after host validation (status range, header allowlist, size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebResponse {
    pub status: u16,
    /// Lowercase header names from the host allowlist.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// One frozen file of a [`Site`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticFile {
    pub bytes: Arc<[u8]>,
    pub content_type: &'static str,
    /// A strong ETag, quotes included.
    pub etag: String,
}

/// The public pages and static assets a web plugin froze when it loaded.
///
/// Built by the plugin host from `HttpAssets` and `HttpPublish`, served by `yinz-web`. A new
/// plugin generation starts a fresh `Site`, so files from two generations are never mixed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub plugin_id: Option<crate::internal_ids::PluginId>,
    pub generation: u64,
    /// URL prefix (for example `/static/`, with both slashes) to files keyed by their path
    /// relative to that prefix.
    pub assets: BTreeMap<String, BTreeMap<String, StaticFile>>,
    /// Exact paths (for example `/` and `/login`) to pre-rendered pages.
    pub pages: BTreeMap<String, StaticFile>,
}

impl Site {
    /// The file served at `path`: an exact page first, then the asset under the longest
    /// matching prefix.
    pub fn lookup(&self, path: &str) -> Option<&StaticFile> {
        if let Some(page) = self.pages.get(path) {
            return Some(page);
        }
        self.assets
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .and_then(|(prefix, files)| files.get(&path[prefix.len()..]))
    }
}

/// The current [`Site`], shared between the plugin host (writer) and the HTTP server (reader).
#[derive(Clone, Default)]
pub struct SiteHandle(Arc<RwLock<Option<Arc<Site>>>>);

impl SiteHandle {
    pub fn current(&self) -> Option<Arc<Site>> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set(&self, site: Option<Arc<Site>>) {
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = site;
    }
}

impl std::fmt::Debug for SiteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let current = self.current();
        f.debug_struct("SiteHandle")
            .field("generation", &current.as_ref().map(|site| site.generation))
            .field(
                "plugin_id",
                &current.as_ref().and_then(|site| site.plugin_id),
            )
            .finish()
    }
}

#[cfg(test)]
#[path = "unit/web_tests.rs"]
mod tests;
