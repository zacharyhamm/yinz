//! The request a caller sends, and how a reply becomes a value or an [`Error`].

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

/// One HTTP request, ready to hand to whatever sends it.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// Never print the key: requests end up in logs through `{:?}`.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: BTreeMap<_, _> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let secret = k.eq_ignore_ascii_case(API_KEY_HEADER);
                (k.as_str(), if secret { "<redacted>" } else { v.as_str() })
            })
            .collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body", &String::from_utf8_lossy(&self.body))
            .finish()
    }
}

/// The server reads this before `Authorization` when both are sent.
pub const API_KEY_HEADER: &str = "x-api-key";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Refused before sending: the server would have refused it too.
    Invalid(String),
    /// A write was attempted without credentials.
    NotLoggedIn,
    /// No reply at all: the connection failed or the body could not be read.
    Transport(String),
    /// 401: the key is unknown, revoked or expired.
    Unauthorized(String),
    /// 403: the key is fine, but this caller may not do this.
    Forbidden(String),
    /// 404. The server says this for an issue the caller cannot see, too.
    NotFound(String),
    /// 429, with `Retry-After` in seconds when the server sent it.
    RateLimited {
        retry_after: Option<u64>,
        scope: Option<String>,
    },
    /// Any other refusal. `flagged` lists the words a 422 profanity check
    /// objected to.
    Rejected {
        status: u16,
        message: String,
        flagged: Vec<String>,
    },
    /// A success whose body did not have the documented shape.
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(message) => write!(f, "{message}"),
            Error::NotLoggedIn => write!(
                f,
                "not logged in to swamp-club: run `swamp auth login`, or set SWAMP_API_KEY"
            ),
            Error::Transport(message) => write!(f, "could not reach swamp-club: {message}"),
            Error::Unauthorized(message) => write!(
                f,
                "swamp-club rejected the API key ({message}): run `swamp auth login` again"
            ),
            Error::Forbidden(message) => write!(f, "not allowed: {message}"),
            Error::NotFound(message) => write!(f, "{message}"),
            Error::RateLimited { retry_after, .. } => match retry_after {
                Some(seconds) => write!(f, "rate limited by swamp-club; retry in {seconds}s"),
                None => write!(f, "rate limited by swamp-club"),
            },
            Error::Rejected {
                status,
                message,
                flagged,
            } if !flagged.is_empty() => {
                write!(
                    f,
                    "HTTP {status}: {message} (flagged: {})",
                    flagged.join(", ")
                )
            },
            Error::Rejected {
                status, message, ..
            } => write!(f, "HTTP {status}: {message}"),
            Error::Decode(message) => write!(f, "unexpected reply from swamp-club: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Every Lab error body is `{"error": "..."}`; a 422 adds `flagged`, and a
/// 429 adds the limiter's `scope`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    error: Option<String>,
    flagged: Vec<String>,
    scope: Option<String>,
}

/// Decode a successful reply as `T`, or classify a failed one.
pub fn parse<T: DeserializeOwned>(
    status: u16,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<T, Error> {
    check(status, headers, body)?;
    serde_json::from_slice(body).map_err(|e| Error::Decode(e.to_string()))
}

/// For the endpoints that answer 204 with no body.
pub fn parse_empty(
    status: u16,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<(), Error> {
    check(status, headers, body)
}

fn check(status: u16, headers: &BTreeMap<String, String>, body: &[u8]) -> Result<(), Error> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    // yinz reports a request that got no response as a 400 with no headers
    // and the error text as the body; a real server always sends headers.
    if headers.is_empty() && status == 400 {
        return Err(Error::Transport(
            String::from_utf8_lossy(body).trim().to_owned(),
        ));
    }
    let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
    let message = parsed.error.unwrap_or_else(|| {
        let text = String::from_utf8_lossy(body);
        let text: String = text.trim().chars().take(500).collect();
        if text.is_empty() {
            format!("HTTP {status}")
        } else {
            text
        }
    });
    Err(match status {
        401 => Error::Unauthorized(message),
        403 => Error::Forbidden(message),
        404 => Error::NotFound(message),
        429 => Error::RateLimited {
            retry_after: header(headers, "retry-after").and_then(|v| v.trim().parse().ok()),
            scope: parsed.scope,
        },
        _ => Error::Rejected {
            status,
            message,
            flagged: parsed.flagged,
        },
    })
}

/// Header names are case-insensitive, and nothing promises the sender kept
/// the server's spelling.
pub fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
#[path = "unit/http.rs"]
mod tests;
