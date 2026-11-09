//! Password login and bearer sessions, checked natively before any request reaches the plugin.
//!
//! Files live under `<data_dir>/web/`: `password` (the argon2id verifier) and `sessions.json`
//! (hashed bearer tokens), shared by every server on the data directory.

pub mod endpoints;
pub mod limiter;
pub mod password;
pub mod sessions;

use std::path::PathBuf;
use std::sync::Arc;

/// Wall-clock milliseconds, injectable so expiry can be tested.
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    /// Absolute session lifetime.
    pub session_days: u32,
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            session_days: yinz_types::input::web::DEFAULT_WEB_SESSION_DAYS,
        }
    }
}

/// Whether the server may serve: a password file must be present and valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordStateKind {
    Missing,
    Invalid,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No bearer token, or one that names no valid session: 401.
    Unauthenticated,
    /// The password is missing or invalid, so no session is valid: 503.
    PasswordNotReady,
}

/// A per-session API bucket ran dry: 429 with `Retry-After`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_s: u64,
}

/// The public view of an authenticated session. The token never leaves this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// 16 hex characters, safe to log by prefix and to show the plugin.
    pub id: String,
    pub label: String,
    pub expires_at_ms: i64,
}

/// A native auth endpoint's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthResponse {
    pub status: u16,
    pub json: serde_json::Value,
    pub retry_after_s: Option<u64>,
}

impl AuthResponse {
    fn error(status: u16, code: &str) -> Self {
        AuthResponse {
            status,
            json: serde_json::json!({"error": {"code": code}}),
            retry_after_s: None,
        }
    }
}

/// One server's authentication state. Denies everything until the auth workstream lands.
pub struct Auth {
    data_dir: PathBuf,
    config: AuthConfig,
    clock: Arc<dyn Clock>,
}

impl Auth {
    pub fn new(data_dir: PathBuf, cfg: AuthConfig, clock: Arc<dyn Clock>) -> Self {
        Auth {
            data_dir,
            config: cfg,
            clock,
        }
    }

    /// Missing, Invalid or Ready, from a fresh read; the listener polls off-runtime at most 500 ms apart.
    pub fn password_state(&self) -> PasswordStateKind {
        match password::read_state(&self.data_dir) {
            password::PasswordState::Missing => PasswordStateKind::Missing,
            password::PasswordState::Invalid(_) => PasswordStateKind::Invalid,
            password::PasswordState::Ready(_) => PasswordStateKind::Ready,
        }
    }

    pub fn authenticate(&self, bearer: Option<&str>) -> Result<SessionInfo, AuthError> {
        let _ = (bearer, &self.config, self.clock.now_ms());
        match self.password_state() {
            PasswordStateKind::Ready => Err(AuthError::Unauthenticated),
            PasswordStateKind::Missing | PasswordStateKind::Invalid => {
                Err(AuthError::PasswordNotReady)
            },
        }
    }

    pub async fn login(
        &self,
        body: zeroize::Zeroizing<Vec<u8>>,
        user_agent: Option<&str>,
        client_key: &str,
    ) -> AuthResponse {
        let _ = (body, user_agent, client_key);
        match self.password_state() {
            PasswordStateKind::Ready => AuthResponse::error(401, "invalid_credentials"),
            PasswordStateKind::Missing | PasswordStateKind::Invalid => {
                AuthResponse::error(503, "no_password")
            },
        }
    }

    pub fn logout(&self, s: &SessionInfo) -> AuthResponse {
        let _ = s;
        AuthResponse::error(401, "unauthenticated")
    }

    pub fn logout_all(&self) -> AuthResponse {
        AuthResponse::error(401, "unauthenticated")
    }

    pub fn session_json(&self, s: &SessionInfo) -> AuthResponse {
        let _ = s;
        AuthResponse::error(401, "unauthenticated")
    }

    /// Spend one request from the session's API bucket and note it as seen.
    pub fn touch(&self, s: &SessionInfo) -> Result<(), RateLimited> {
        let _ = s;
        Ok(())
    }
}
