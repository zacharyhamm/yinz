//! The swamp-club Lab, reached from a plugin.
//!
//! [`swamp_lab`] builds the requests and reads the replies; this is the one
//! place that hands them to the host. The caller needs `WebAccess` and routes
//! the `WebRequestResult` carrying its `context` back to `swamp_lab::parse`.
//! Credentials come from [`swamp_lab::auth`]: the session environment plus
//! the output of `auth_json_argv`, which needs `RunCommands`.

pub use swamp_lab;

use std::collections::BTreeMap;
use swamp_lab::Method;
use zellij_tile::prelude::HttpVerb;

pub fn verb(method: Method) -> HttpVerb {
    match method {
        Method::Get => HttpVerb::Get,
        Method::Post => HttpVerb::Post,
        Method::Patch => HttpVerb::Patch,
        Method::Delete => HttpVerb::Delete,
    }
}

/// Send `request` through the host; the reply arrives as a `WebRequestResult`
/// carrying `context`.
pub fn send(request: swamp_lab::Request, context: BTreeMap<String, String>) {
    zellij_tile::prelude::web_request(
        request.url,
        verb(request.method),
        request.headers,
        request.body,
        context,
        None,
    );
}
