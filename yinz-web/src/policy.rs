//! Transport-bound Host, Origin and identity checks. No forwarded field grants trust.
use crate::WebSettings;
use http::{HeaderMap, Method, StatusCode};
use std::net::{IpAddr, SocketAddr};

pub(crate) fn single<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, StatusCode> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|v| v.to_str().map_err(|_| StatusCode::BAD_REQUEST))
        .transpose()?;
    if values.next().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(value)
}

#[derive(Debug)]
struct Authority {
    text: String,
    host: String,
}

fn authority(text: &str) -> Result<Authority, StatusCode> {
    if text.is_empty()
        || text.bytes().any(|b| {
            b.is_ascii_whitespace() || matches!(b, b'@' | b'/' | b'\\' | b'?' | b'#' | b',' | b'%')
        })
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let parsed: http::uri::Authority = text.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    let host = parsed.host().to_ascii_lowercase();
    let host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host)
        .to_owned();
    if text.starts_with('[') {
        if !host.parse::<std::net::Ipv6Addr>().is_ok() {
            return Err(StatusCode::BAD_REQUEST);
        }
        let end = text.find(']').ok_or(StatusCode::BAD_REQUEST)?;
        if text.len() > end + 1 {
            validate_port(&text[end + 1..])?;
        }
    } else {
        if text.matches(':').count() > 1
            || host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if let Some((_, port)) = text.split_once(':') {
            validate_port(&format!(":{port}"))?;
        }
        if host.starts_with('.')
            || host.ends_with('.')
            || host
                .split('.')
                .any(|s| s.is_empty() || s.starts_with('-') || s.ends_with('-'))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    Ok(Authority {
        text: text.to_ascii_lowercase(),
        host,
    })
}
fn validate_port(suffix: &str) -> Result<(), StatusCode> {
    let port = suffix.strip_prefix(':').ok_or(StatusCode::BAD_REQUEST)?;
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err()
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}
fn loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn origin(text: &str) -> Option<String> {
    let (scheme, rest) = text.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    Some(format!("{scheme}://{}", authority(rest).ok()?.text))
}

pub(crate) fn check(
    method: &Method,
    headers: &HeaderMap,
    peer: SocketAddr,
    settings: &WebSettings,
) -> Result<(), StatusCode> {
    if !matches!(*method, Method::GET | Method::HEAD | Method::POST) {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    if headers.contains_key("tailscale-funnel-request") {
        return Err(StatusCode::FORBIDDEN);
    }
    let host = authority(single(headers, "host")?.ok_or(StatusCode::BAD_REQUEST)?)?;
    if !loopback(&host.host)
        && !host.host.ends_with(".ts.net")
        && !settings
            .allowed_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&host.host))
    {
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    if !settings.tailscale_users.is_empty()
        && (!peer.ip().is_loopback()
            || !single(headers, "tailscale-user-login")?
                .is_some_and(|login| settings.tailscale_users.iter().any(|u| u == login)))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if *method == Method::POST {
        if single(headers, "sec-fetch-site")?.is_some_and(|s| s.eq_ignore_ascii_case("cross-site"))
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let supplied = single(headers, "origin")?
            .and_then(origin)
            .ok_or(StatusCode::FORBIDDEN)?;
        let expected = format!(
            "{}://{}",
            if loopback(&host.host) {
                "http"
            } else {
                "https"
            },
            host.text
        );
        let forwarded = if peer.ip().is_loopback() {
            single(headers, "x-forwarded-host")?
                .map(authority)
                .transpose()?
                .map(|h| format!("https://{}", h.text))
        } else {
            None
        };
        if supplied != expected
            && forwarded.as_ref() != Some(&supplied)
            && !settings
                .allowed_origins
                .iter()
                .filter_map(|o| origin(o))
                .any(|o| o == supplied)
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let content_type =
            single(headers, "content-type")?.ok_or(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
        if !content_type
            .split(';')
            .next()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
    }
    Ok(())
}

/// Serve strips identity headers; XFF trust is deliberately not assumed.
pub(crate) fn client_key(headers: &HeaderMap, peer: SocketAddr) -> String {
    if peer.ip().is_loopback()
        && let Ok(Some(login)) = single(headers, "tailscale-user-login")
        && !login.is_empty()
        && login.len() <= 256
        && !login.chars().any(char::is_control)
    {
        return format!("user:{login}");
    }
    format!("peer:{}", peer.ip())
}

#[cfg(test)]
#[path = "unit/policy.rs"]
mod tests;
