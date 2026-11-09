//! Native HTTP admission: transport policy, authentication, limits, then plugin dispatch.
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
    pub auth: Arc<crate::auth::Auth>,
    pub request_id: std::sync::atomic::AtomicU64,
    pub io_work: Arc<tokio::sync::Semaphore>,
    pub backend: Arc<dyn Backend>,
    pub funnel_warning: std::sync::Mutex<Option<tokio::time::Instant>>,
}

// Admission waits are cancellable futures; a canceled running file operation retains its
// permit until completion. Together with bounded flock waits this caps abandoned I/O work.
async fn file_work<T: Send + 'static>(
    state: &State,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ()> {
    let permit = state
        .io_work
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| ())
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
fn retry(mut response: Response<Body>, seconds: u64) -> Response<Body> {
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
    response
}
fn auth_response(r: crate::auth::AuthResponse) -> Response<Body> {
    let mut out = if r.status == 204 {
        response(StatusCode::NO_CONTENT, Bytes::new(), "application/json")
    } else {
        response(
            StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            r.json.to_string(),
            "application/json",
        )
    };
    if r.status == 401 {
        out.headers_mut()
            .insert("www-authenticate", HeaderValue::from_static("Bearer"));
    }
    if let Some(seconds) = r.retry_after_s {
        out = retry(out, seconds);
    }
    out
}
fn unauthenticated() -> Response<Body> {
    let mut r = error(StatusCode::UNAUTHORIZED, "unauthenticated");
    r.headers_mut()
        .insert("www-authenticate", HeaderValue::from_static("Bearer"));
    r
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
    use crate::auth::Clock as _;
    let deadline_ms = crate::auth::SystemClock
        .now_ms()
        .saturating_add(timeout.as_millis() as i64);
    let lifetime = CancellationToken::new();
    let _request_guard = lifetime.clone().drop_guard();
    let result = tokio::time::timeout(timeout, async {
        tokio::select! {biased; _=admission.cancelled()=> (error(StatusCode::SERVICE_UNAVAILABLE,"not_serving"),true),
            result=handle(req,&state,peer,admission.clone(),lifetime,deadline_ms)=>result}
    }).await;
    let (response, close) =
        result.unwrap_or_else(|_| (error(StatusCode::GATEWAY_TIMEOUT, "timeout"), true));
    Ok(finish(response, head, api, close))
}

async fn handle(
    req: Request<Incoming>,
    state: &State,
    peer: SocketAddr,
    admission: CancellationToken,
    lifetime: CancellationToken,
    deadline_ms: i64,
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
    // A direct loopback health probe is a local diagnostic, not a browser/API bypass.
    // Proxy requests still require the configured Tailscale identity on every route.
    let mut probe_settings;
    let policy_settings = if parts.uri.path() == "/healthz"
        && parts.method == Method::GET
        && peer.ip().is_loopback()
        && !parts
            .headers
            .keys()
            .any(|k| k.as_str().starts_with("x-forwarded-") || k.as_str().starts_with("tailscale-"))
    {
        probe_settings = state.settings.clone();
        probe_settings.tailscale_users.clear();
        &probe_settings
    } else {
        &state.settings
    };
    if let Err(status) = policy::check(&parts.method, &parts.headers, peer, policy_settings) {
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
    let native = match normalized.path.as_str() {
        "/api/login" | "/api/logout" | "/api/logout-all" => Some("POST"),
        "/api/session" => Some("GET"),
        _ => None,
    };
    if let Some(expected) = native
        && parts.method.as_str() != expected
    {
        let mut r = error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        r.headers_mut()
            .insert("allow", HeaderValue::from_static(expected));
        return (r, true);
    }
    if admission.is_cancelled() || state.backend.quiesced() {
        return (
            retry(error(StatusCode::SERVICE_UNAVAILABLE, "updating"), 1),
            true,
        );
    }
    let session = if normalized.path.starts_with("/api/") && normalized.path != "/api/login" {
        let bearer = policy::single(&parts.headers, "authorization")
            .ok()
            .flatten()
            .and_then(|s| s.split_once(' '))
            .filter(|(scheme, token)| {
                scheme.eq_ignore_ascii_case("Bearer") && crate::auth::sessions::valid_token(token)
            })
            .map(|(_, token)| token.to_owned());
        let auth = state.auth.clone();
        let result = file_work(state, move || auth.authenticate(bearer.as_deref())).await;
        let s = match result {
            Ok(Ok(s)) => s,
            Ok(Err(crate::auth::AuthError::PasswordNotReady)) => {
                return (error(StatusCode::SERVICE_UNAVAILABLE, "no_password"), true)
            },
            Ok(Err(crate::auth::AuthError::StoreUnavailable)) | Err(_) => {
                return (
                    error(StatusCode::SERVICE_UNAVAILABLE, "session_store_unavailable"),
                    true,
                )
            },
            _ => return (unauthenticated(), true),
        };
        if let Err(r) = state.auth.touch(&s) {
            return (auth_response(crate::auth::AuthResponse::limited(r)), true);
        }
        Some(s)
    } else {
        None
    };
    if (matches!(normalized.path.as_str(), "/" | "/login")
        || normalized.path.starts_with("/static/"))
        && let Err(r) = state.auth.public()
    {
        return (auth_response(crate::auth::AuthResponse::limited(r)), true);
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
    let ready = file_work(state, move || {
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
    if normalized.path == "/api/login" {
        let client_key = policy::client_key(&parts.headers, peer);
        let backend = state.backend.clone();
        let live = Arc::new(move || {
            !lifetime.is_cancelled() && !admission.is_cancelled() && !backend.quiesced()
        });
        return (
            auth_response(
                state
                    .auth
                    .login_guarded(
                        bytes,
                        policy::single(&parts.headers, "user-agent").ok().flatten(),
                        &client_key,
                        live,
                    )
                    .await,
            ),
            false,
        );
    }
    if let Some(session) = session {
        if admission.is_cancelled() || state.backend.quiesced() {
            return (error(StatusCode::SERVICE_UNAVAILABLE, "updating"), true);
        }
        if matches!(
            normalized.path.as_str(),
            "/api/session" | "/api/logout" | "/api/logout-all"
        ) {
            let auth = state.auth.clone();
            let path = normalized.path;
            let backend = state.backend.clone();
            let live = move || {
                !lifetime.is_cancelled() && !admission.is_cancelled() && !backend.quiesced()
            };
            let answer = file_work(state, move || match path.as_str() {
                "/api/session" => auth.session_json(&session),
                "/api/logout" => auth.logout_if(&session, live),
                _ => auth.logout_all_if(live),
            })
            .await
            .unwrap_or_else(|_| crate::auth::AuthResponse::error(503, "session_store_unavailable"));
            return (auth_response(answer), false);
        }
        let timeout = if post {
            state.settings.post_timeout
        } else {
            state.settings.get_timeout
        };
        let headers = parts
            .headers
            .iter()
            .filter(|(k, _)| {
                matches!(
                    k.as_str(),
                    "accept" | "accept-language" | "content-type" | "if-none-match" | "user-agent"
                )
            })
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
            .collect();
        let request = WebRequest {
            id: state
                .request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            method: if post {
                yinz_types::web::WebMethod::Post
            } else if parts.method == Method::HEAD {
                yinz_types::web::WebMethod::Head
            } else {
                yinz_types::web::WebMethod::Get
            },
            path: normalized.path,
            segments: normalized.segments,
            query: normalized.query,
            headers,
            body: post.then(|| String::from_utf8_lossy(&bytes).into_owned()),
            auth: yinz_types::web::WebAuth {
                session: session.id,
                label: session.label,
            },
            deadline_ms,
        };
        return (
            dispatch(state.backend.as_ref(), request, timeout).await,
            false,
        );
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
            u64::from(retry_after_s),
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
