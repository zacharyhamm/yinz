//! Who is calling, and which server they are calling.
//!
//! Resolution follows the `swamp` CLI's `AuthRepository.load` exactly, so a
//! `swamp auth login` on this machine serves agent too and neither can see a
//! different server from the other:
//!
//! 1. `SWAMP_API_KEY` wins. The server is `SWAMP_CLUB_URL`, or the default. The
//!    username is borrowed from `auth.json` only when that file cached it for
//!    the same key (by [`key_fingerprint`]) and the same server.
//! 2. Otherwise `auth.json` (`serverUrl`, `apiKey`, `username`), with the
//!    retired `swamp.club` domain read as its successor. The CLI rewrites the
//!    file when it sees the old domain; this only reads it.
//! 3. Otherwise nobody: an anonymous caller, who can read public issues and
//!    nothing else. `SWAMP_CLUB_URL` still picks the server, so a test or a
//!    local swamp-club works without a login.
//!
//! A plugin cannot open `auth.json` itself, so [`auth_json_argv`] is the
//! command that prints it, and [`auth_json_from_output`] reads what came back.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

pub const DEFAULT_SERVER_URL: &str = "https://swamp-club.com";
/// The domain swamp-club moved away from; stored credentials may still name it.
pub const LEGACY_SERVER_URL: &str = "https://swamp.club";
pub const API_KEY_ENV: &str = "SWAMP_API_KEY";
pub const SERVER_URL_ENV: &str = "SWAMP_CLUB_URL";

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Scheme and host, with any trailing `/` removed.
    pub server_url: String,
    /// `None` for an anonymous caller.
    pub api_key: Option<String>,
    pub username: Option<String>,
}

/// Never print the key: credentials end up in logs through `{:?}`.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("server_url", &self.server_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("username", &self.username)
            .finish()
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct AuthFile {
    server_url: Option<String>,
    api_key: Option<String>,
    username: Option<String>,
    api_key_fingerprint: Option<String>,
}

impl Credentials {
    /// An anonymous caller of the default server.
    pub fn anonymous() -> Self {
        Credentials {
            server_url: DEFAULT_SERVER_URL.into(),
            api_key: None,
            username: None,
        }
    }

    /// Resolve from the session environment and the text of `auth.json`, if
    /// there is one. An unreadable file or a bad server URL is an error rather
    /// than a quiet fall back to anonymous: a caller who logged in should hear
    /// why they are not being treated as logged in.
    pub fn resolve(
        env: &BTreeMap<String, String>,
        auth_json: Option<&str>,
    ) -> Result<Self, String> {
        let file = match auth_json.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => Some(
                serde_json::from_str::<AuthFile>(text)
                    .map_err(|e| format!("swamp auth.json is not valid: {e}"))?,
            ),
            None => None,
        };
        let env_server = non_empty(env, SERVER_URL_ENV);
        if let Some(api_key) = non_empty(env, API_KEY_ENV) {
            let server_url = normalize_url(env_server.as_deref().unwrap_or(DEFAULT_SERVER_URL))?;
            let username = file.and_then(|cached| {
                let same_key =
                    cached.api_key_fingerprint.as_deref() == Some(&key_fingerprint(&api_key));
                let same_server = cached
                    .server_url
                    .as_deref()
                    .is_none_or(|url| normalize_url(url).ok().as_ref() == Some(&server_url));
                (same_key && same_server)
                    .then_some(cached.username)
                    .flatten()
                    .filter(|name| !name.is_empty())
            });
            return Ok(Credentials {
                server_url,
                api_key: Some(api_key),
                username,
            });
        }
        if let Some(file) = file {
            if let Some(api_key) = file.api_key.filter(|key| !key.trim().is_empty()) {
                let stored = file.server_url.as_deref().unwrap_or(DEFAULT_SERVER_URL);
                let stored = if stored.trim_end_matches('/') == LEGACY_SERVER_URL {
                    DEFAULT_SERVER_URL
                } else {
                    stored
                };
                return Ok(Credentials {
                    server_url: normalize_url(stored)?,
                    api_key: Some(api_key.trim().to_owned()),
                    username: file.username.filter(|name| !name.is_empty()),
                });
            }
        }
        Ok(Credentials {
            server_url: normalize_url(env_server.as_deref().unwrap_or(DEFAULT_SERVER_URL))?,
            api_key: None,
            username: None,
        })
    }

    pub fn is_anonymous(&self) -> bool {
        self.api_key.is_none()
    }

    /// The issue's page on the web, for a person to open or a model to cite.
    pub fn issue_url(&self, number: u64) -> String {
        format!("{}/lab/{number}", self.server_url)
    }
}

/// The CLI's key fingerprint: the first 12 characters, which is how it tells
/// whether the identity cached in `auth.json` belongs to the key in use.
pub fn key_fingerprint(api_key: &str) -> String {
    api_key.chars().take(12).collect()
}

/// A command that prints `auth.json`, or nothing when there is none.
///
/// The directory is the CLI's `getSwampConfigDir`: `$SWAMP_HOME/config`, then
/// `$XDG_CONFIG_HOME/swamp`, then `~/.config/swamp`. A missing file exits 0
/// with no output; a file that is there and cannot be read exits non-zero, so
/// that a permissions problem is not mistaken for being logged out.
pub fn auth_json_argv() -> Vec<String> {
    let script = r#"if [ -n "$SWAMP_HOME" ]; then dir="$SWAMP_HOME/config"
elif [ -n "$XDG_CONFIG_HOME" ]; then dir="$XDG_CONFIG_HOME/swamp"
else dir="$HOME/.config/swamp"; fi
file="$dir/auth.json"
[ -e "$file" ] || exit 0
cat -- "$file""#;
    vec!["sh".into(), "-c".into(), script.into()]
}

/// Read the result of running [`auth_json_argv`].
pub fn auth_json_from_output(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Option<String>, String> {
    if exit_code != Some(0) {
        let detail = String::from_utf8_lossy(stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            "could not read swamp auth.json".into()
        } else {
            format!("could not read swamp auth.json: {detail}")
        });
    }
    let text = String::from_utf8_lossy(stdout).trim().to_owned();
    Ok((!text.is_empty()).then_some(text))
}

fn non_empty(map: &BTreeMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// An absolute HTTP(S) origin, kept as written apart from a trailing `/`.
fn normalize_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let parsed =
        url::Url::parse(trimmed).map_err(|e| format!("invalid swamp-club URL {trimmed:?}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!(
            "invalid swamp-club URL {trimmed:?}: it must be an absolute http(s) URL"
        ));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
#[path = "unit/auth.rs"]
mod tests;
