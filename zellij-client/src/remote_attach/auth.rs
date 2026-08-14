use super::http_client::HttpClientWithCookies;
use crate::web_protocol::endpoints::{LOGIN, SESSION};
use crate::web_protocol::http::{CreateClientIdResponse, LoginRequest};
use crate::RemoteClientError;
use isahc::{AsyncReadResponseExt, Request};

/// Identifies this client to the web server. Sent on every request.
const USER_AGENT: &str = "http-terminal-client/1.0";

fn session_url(server_base_url: &str, session_name: &str) -> String {
    if session_name.is_empty() {
        format!("{}{}", server_base_url, SESSION)
    } else {
        format!(
            "{}{}?session={}",
            server_base_url,
            SESSION,
            urlencoding::encode(session_name)
        )
    }
}

/// POST `body` as JSON and check the status.
///
/// `unauthorized` is the error a 401 maps to, which is the only thing that
/// differs between the two ways of reaching `/session`: a rejected login
/// means bad credentials, while a rejected session token means it expired.
async fn post_json(
    http_client: &HttpClientWithCookies,
    url: String,
    body: Vec<u8>,
    unauthorized: RemoteClientError,
) -> Result<isahc::Response<isahc::AsyncBody>, RemoteClientError> {
    let request = Request::post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .body(body)?;

    let response = http_client
        .send_with_cookies(request)
        .await
        .map_err(|e| RemoteClientError::ConnectionFailed(e.to_string()))?;

    match response.status().as_u16() {
        401 => Err(unauthorized),
        status if !response.status().is_success() => Err(RemoteClientError::ConnectionFailed(
            format!("Server returned status {}", status),
        )),
        _ => Ok(response),
    }
}

/// Ask `/session` for the web client id this connection will use.
async fn fetch_web_client_id(
    http_client: &HttpClientWithCookies,
    server_base_url: &str,
    session_name: &str,
    unauthorized: RemoteClientError,
) -> Result<String, RemoteClientError> {
    let mut response = post_json(
        http_client,
        session_url(server_base_url, session_name),
        b"{}".to_vec(),
        unauthorized,
    )
    .await?;

    let body = response.text().await?;
    let session_data: CreateClientIdResponse = serde_json::from_str(&body)?;
    Ok(session_data.web_client_id)
}

/// Exchange an auth token for a session, returning the web client id, the
/// cookie-bearing client to keep using, and the session token when the caller
/// asked to be remembered.
pub async fn authenticate(
    server_base_url: &str,
    auth_token: &str,
    remember_me: bool,
    session_name: &str,
    ca_cert: Option<&std::path::Path>,
    insecure: bool,
) -> Result<(String, HttpClientWithCookies, Option<String>), RemoteClientError> {
    let http_client = HttpClientWithCookies::new(ca_cert, insecure)?;

    let login_request = LoginRequest {
        auth_token: auth_token.to_string(),
        remember_me: Some(remember_me),
    };
    post_json(
        &http_client,
        format!("{}{}", server_base_url, LOGIN),
        serde_json::to_vec(&login_request)?,
        RemoteClientError::InvalidAuthToken,
    )
    .await?;

    let web_client_id = fetch_web_client_id(
        &http_client,
        server_base_url,
        session_name,
        RemoteClientError::Unauthorized,
    )
    .await?;

    // The login response set this cookie; it is only worth saving if the
    // caller asked to be remembered.
    let session_token = if remember_me {
        http_client.get_cookie("session_token")
    } else {
        None
    };

    Ok((web_client_id, http_client, session_token))
}

/// Re-use a remembered session token, skipping `/login` entirely.
pub async fn validate_session_token(
    server_base_url: &str,
    session_token: &str,
    session_name: &str,
    ca_cert: Option<&std::path::Path>,
    insecure: bool,
) -> Result<(String, HttpClientWithCookies), RemoteClientError> {
    let http_client = HttpClientWithCookies::new(ca_cert, insecure)?;
    http_client.set_cookie("session_token".to_string(), session_token.to_string());

    let web_client_id = fetch_web_client_id(
        &http_client,
        server_base_url,
        session_name,
        RemoteClientError::SessionTokenExpired,
    )
    .await?;

    Ok((web_client_id, http_client))
}
