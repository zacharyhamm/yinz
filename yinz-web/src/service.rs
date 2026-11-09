//! Native HTTP service. Authentication remains fail-closed in this milestone.
use crate::{pages, policy, request, site, Backend, WebSettings};
use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use yinz_types::web::{WebReply, WebRequest, MAX_RESPONSE_BODY};
use zeroize::Zeroizing;

pub(crate) type Body = Full<Bytes>;
pub(crate) struct State {
    pub settings: WebSettings,
    pub backend: Arc<dyn Backend>,
    pub funnel_warning: std::sync::Mutex<Option<tokio::time::Instant>>,
}

fn response(
    status: StatusCode,
    body: impl Into<Bytes>,
    content_type: &'static str,
) -> Response<Body> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response
}
fn error(status: StatusCode, code: &str) -> Response<Body> {
    response(
        status,
        serde_json::json!({"error": {"code": code}}).to_string(),
        "application/json",
    )
}
fn retry(mut response: Response<Body>, seconds: u32) -> Response<Body> {
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
    response
}
fn finish(mut response: Response<Body>, head: bool, api: bool, close: bool) -> Response<Body> {
    let headers = response.headers_mut();
    for (name, value) in [
        ("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"),
        ("x-content-type-options", "nosniff"), ("referrer-policy", "no-referrer"), ("x-frame-options", "DENY"),
        ("cross-origin-opener-policy", "same-origin"), ("cross-origin-resource-policy", "same-origin"),
        ("permissions-policy", "camera=(), microphone=(), geolocation=()"),
    ] { headers.insert(name, HeaderValue::from_static(value)); }
    if !headers.contains_key("cache-control") {
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
    }
    if api {
        headers.insert("vary", HeaderValue::from_static("Authorization"));
    }
    if close {
        headers.insert("connection", HeaderValue::from_static("close"));
    }
    if head {
        use hyper::body::Body as _;
        let length = response.body().size_hint().exact().unwrap_or(0);
        if response.status() != StatusCode::NOT_MODIFIED
            && let Ok(value) = HeaderValue::from_str(&length.to_string())
        {
            response.headers_mut().insert("content-length", value);
        }
        *response.body_mut() = Full::new(Bytes::new());
    }
    response
}

pub(crate) async fn serve(
    req: Request<Incoming>,
    state: Arc<State>,
    peer: SocketAddr,
    admission: CancellationToken,
) -> Result<Response<Body>, Infallible> {
    let head = req.method() == Method::HEAD;
    let api = request::normalize(req.uri().path(), req.uri().query())
        .is_ok_and(|path| path.path.starts_with("/api/"));
    let timeout = if req.method() == Method::POST {
        state.settings.post_timeout
    } else {
        state.settings.get_timeout
    };
    let result = tokio::time::timeout(timeout, handle(req, &state, peer, admission)).await;
    let (response, close) =
        result.unwrap_or_else(|_| (error(StatusCode::GATEWAY_TIMEOUT, "timeout"), true));
    Ok(finish(response, head, api, close))
}

async fn handle(
    req: Request<Incoming>,
    state: &State,
    peer: SocketAddr,
    admission: CancellationToken,
) -> (Response<Body>, bool) {
    let (parts, body) = req.into_parts();
    if parts.headers.contains_key("tailscale-funnel-request") {
        let mut warning = state
            .funnel_warning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if warning.is_none_or(|last| last.elapsed() >= Duration::from_secs(60)) {
            log::warn!("web request rejected: Tailscale Funnel is not permitted");
            *warning = Some(tokio::time::Instant::now());
        }
    }
    if parts.uri.authority().is_some() || parts.uri.scheme().is_some() {
        return (
            error(StatusCode::BAD_REQUEST, "invalid_request_target"),
            true,
        );
    }
    if let Err(status) = policy::check(&parts.method, &parts.headers, peer, &state.settings) {
        let mut response = error(status, "request_rejected");
        if status == StatusCode::METHOD_NOT_ALLOWED {
            response
                .headers_mut()
                .insert("allow", HeaderValue::from_static("GET, HEAD, POST"));
        }
        return (response, true);
    }
    let normalized = match request::normalize(parts.uri.path(), parts.uri.query()) {
        Ok(path) => path,
        Err(status) => return (error(status, "invalid_path"), true),
    };
    let post = parts.method == Method::POST;
    let limit = if normalized.path == "/api/login" {
        4096
    } else {
        yinz_types::web::MAX_REQUEST_BODY
    };
    let length = match policy::single(&parts.headers, "content-length") {
        Ok(Some(value)) => match value.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return (error(StatusCode::BAD_REQUEST, "invalid_length"), true),
        },
        Ok(None) => 0,
        Err(status) => return (error(status, "invalid_length"), true),
    };
    if length > limit as u64 {
        return (error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"), true);
    }
    if !post && (length != 0 || parts.headers.contains_key("transfer-encoding")) {
        return (error(StatusCode::BAD_REQUEST, "unexpected_body"), true);
    }
    let bytes =
        match tokio::time::timeout(Duration::from_secs(10), Limited::new(body, limit).collect())
            .await
        {
            Err(_) => return (error(StatusCode::REQUEST_TIMEOUT, "body_timeout"), true),
            Ok(Err(e)) => {
                let status = if e.is::<http_body_util::LengthLimitError>() {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                };
                return (error(status, "invalid_body"), true);
            },
            Ok(Ok(collected)) => Zeroizing::new(collected.to_bytes().to_vec()),
        };
    if std::str::from_utf8(&bytes).is_err() || (!post && !bytes.is_empty()) {
        return (error(StatusCode::BAD_REQUEST, "invalid_body"), false);
    }
    if state.backend.quiesced() && post {
        return (
            retry(error(StatusCode::SERVICE_UNAVAILABLE, "updating"), 5),
            true,
        );
    }
    if admission.is_cancelled() {
        return (
            retry(error(StatusCode::SERVICE_UNAVAILABLE, "not_serving"), 1),
            true,
        );
    }
    // The accept loop refreshes readiness off-runtime. A fresh per-request read closes
    // the keep-alive admission race without caching missing/invalid state for another second.
    let data_dir = state.settings.data_dir.clone();
    let ready = tokio::task::spawn_blocking(move || {
        matches!(
            crate::auth::password::read_state(&data_dir),
            crate::auth::password::PasswordState::Ready(_)
        )
    })
    .await
    .unwrap_or(false);
    if !ready || state.backend.quiesced() {
        return (
            retry(error(StatusCode::SERVICE_UNAVAILABLE, "no_password"), 1),
            true,
        );
    }
    if normalized.path.starts_with("/api/") {
        // No credential can authenticate until bearer sessions land. Never dispatch or
        // perform an expensive password check from this HTTP path.
        let mut response = error(
            StatusCode::UNAUTHORIZED,
            if normalized.path == "/api/login" {
                "invalid_credentials"
            } else {
                "unauthenticated"
            },
        );
        response
            .headers_mut()
            .insert("www-authenticate", HeaderValue::from_static("Bearer"));
        return (response, false);
    }
    if post {
        let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        response
            .headers_mut()
            .insert("allow", HeaderValue::from_static("GET, HEAD"));
        return (response, false);
    }
    if normalized.path == "/healthz" {
        let mut json = serde_json::json!({"ok": true});
        if peer.ip().is_loopback() && !parts.headers.contains_key("x-forwarded-for") {
            json["session"] = state.backend.session_name().into();
        }
        return (
            response(StatusCode::OK, json.to_string(), "application/json"),
            false,
        );
    }
    let Some(site) = state.backend.site() else {
        let body = pages::page(
            "Yinz web is starting",
            &format!(
                "Session {} is waiting for its web application.",
                state.backend.session_name()
            ),
        );
        return (
            retry(
                response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    body,
                    "text/html; charset=utf-8",
                ),
                1,
            ),
            false,
        );
    };
    let Some(file) = site.lookup(&normalized.path) else {
        return (
            response(
                StatusCode::NOT_FOUND,
                pages::page("Not found", &normalized.path),
                "text/html; charset=utf-8",
            ),
            false,
        );
    };
    if file.bytes.len() > MAX_RESPONSE_BODY {
        return (error(StatusCode::BAD_GATEWAY, "response_too_large"), false);
    }
    if !site::strong_etag(&file.etag) || HeaderValue::from_str(file.content_type).is_err() {
        return (
            error(StatusCode::BAD_GATEWAY, "invalid_site_metadata"),
            false,
        );
    }
    let Ok(etag) = HeaderValue::from_str(&file.etag) else {
        return (error(StatusCode::BAD_GATEWAY, "invalid_etag"), false);
    };
    let modified = !site::not_modified(&parts.headers, &file.etag);
    let mut response = response(
        if modified {
            StatusCode::OK
        } else {
            StatusCode::NOT_MODIFIED
        },
        if modified {
            Bytes::copy_from_slice(&file.bytes)
        } else {
            Bytes::new()
        },
        file.content_type,
    );
    response.headers_mut().insert("etag", etag);
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-cache"));
    (response, false)
}

// The dispatch contract is implemented without accepting synthetic credentials on HTTP.
// It will be called only after a real Auth::authenticate success in the auth milestone.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "authenticated dispatch is deliberately unreachable until bearer sessions land"
    )
)]
pub(crate) async fn dispatch(
    backend: &dyn Backend,
    request: WebRequest,
    timeout: Duration,
) -> Response<Body> {
    match tokio::time::timeout(timeout, backend.dispatch(request)).await {
        Err(_) => error(StatusCode::GATEWAY_TIMEOUT, "timeout"),
        Ok(None) => retry(error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"), 1),
        Ok(Some(WebReply::Unavailable { retry_after_s, .. })) => retry(
            error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            retry_after_s,
        ),
        Ok(Some(WebReply::Failed(_))) => error(StatusCode::BAD_GATEWAY, "plugin_failed"),
        Ok(Some(WebReply::Response(reply))) => {
            let Ok(status) = StatusCode::from_u16(reply.status) else {
                return error(StatusCode::BAD_GATEWAY, "invalid_response");
            };
            if !(200..=599).contains(&reply.status) || reply.body.len() > MAX_RESPONSE_BODY {
                return error(StatusCode::BAD_GATEWAY, "invalid_response");
            }
            let mut response = response(status, reply.body, "application/json");
            for (name, value) in reply.headers {
                if matches!(name.as_str(), "content-type" | "retry-after" | "etag")
                    && let (Ok(name), Ok(value)) = (
                        http::header::HeaderName::from_bytes(name.as_bytes()),
                        HeaderValue::from_str(&value),
                    )
                {
                    response.headers_mut().insert(name, value);
                }
            }
            response
        },
    }
}

#[cfg(test)]
#[path = "unit/service.rs"]
mod tests;
