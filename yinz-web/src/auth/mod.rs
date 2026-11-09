//! Password login and bearer sessions, checked natively before any request reaches the plugin.
//!
//! Files live under `<data_dir>/web/`: `password` (the argon2id verifier) and `sessions.json`
//! (hashed bearer tokens), shared by every server on the data directory.

pub mod endpoints;
pub mod limiter;
pub mod password;
pub mod sessions;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

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
    StoreUnavailable,
}

/// A per-session API bucket ran dry: 429 with `Retry-After`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_s: u64,
}

/// The public view of an authenticated session. The token never leaves this module.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SessionInfo {
    /// 16 hex characters, safe to log by prefix and to show the plugin.
    pub id: String,
    pub label: String,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
    pub idle_expires_at_ms: i64,
}

/// A native auth endpoint's answer.
#[derive(Clone, PartialEq)]
pub struct AuthResponse {
    pub status: u16,
    pub json: serde_json::Value,
    pub retry_after_s: Option<u64>,
}

impl AuthResponse {
    pub(crate) fn error(status: u16, code: &str) -> Self {
        AuthResponse {
            status,
            json: serde_json::json!({"error": {"code": code}}),
            retry_after_s: None,
        }
    }
}

impl std::fmt::Debug for AuthResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
/// One server's authentication state.
pub struct Auth {
    data_dir: PathBuf,
    config: AuthConfig,
    clock: Arc<dyn Clock>,
    store: Mutex<sessions::Store>,
    limits: Mutex<limiter::Limiter>,
    admission: Arc<Semaphore>,
    verification: Arc<Semaphore>,
    #[cfg(test)]
    pub(crate) verify_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    pub(crate) commit_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Auth {
    pub fn new(data_dir: PathBuf, cfg: AuthConfig, clock: Arc<dyn Clock>) -> Self {
        Auth {
            data_dir,
            config: cfg,
            clock,
            store: Mutex::default(),
            limits: Mutex::default(),
            #[cfg(test)]
            verify_hook: Mutex::default(),
            #[cfg(test)]
            commit_hook: Mutex::default(),
            admission: Arc::new(Semaphore::new(3)),
            verification: Arc::new(Semaphore::new(1)),
        }
    }

    #[cfg(test)]
    pub(crate) fn login_slots(&self) -> usize {
        self.admission.available_permits()
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
        let password::PasswordState::Ready(v) = password::read_state(&self.data_dir) else {
            return Err(AuthError::PasswordNotReady);
        };
        let token = bearer
            .filter(|t| sessions::valid_token(t))
            .ok_or(AuthError::Unauthenticated)?;
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let info = store
            .authenticate(&self.data_dir, &v.verifier_id, token, self.clock.now_ms())
            .map_err(|_| AuthError::StoreUnavailable)?;
        let snapshot = store
            .snapshot(&self.data_dir, &v.verifier_id)
            .map_err(|_| AuthError::StoreUnavailable)?;
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain_sessions(snapshot.sessions.iter().map(|r| r.id.as_str()));
        info.ok_or(AuthError::Unauthenticated)
    }

    pub fn logout(&self, s: &SessionInfo) -> AuthResponse {
        self.logout_if(s, || true)
    }
    pub(crate) fn logout_if(&self, s: &SessionInfo, live: impl FnOnce() -> bool) -> AuthResponse {
        match self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .logout_guarded(&self.data_dir, &s.id, self.clock.now_ms(), live)
        {
            Ok(()) => {
                self.limits
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&s.id);
                AuthResponse::error(204, "")
            },
            Err(_) => AuthResponse::error(503, "session_store_unavailable"),
        }
    }
    pub fn logout_all(&self) -> AuthResponse {
        self.logout_all_if(|| true)
    }
    pub(crate) fn logout_all_if(&self, live: impl FnOnce() -> bool) -> AuthResponse {
        use yinz_runtime::errors::prelude::*;
        let result = (|| -> Result<()> {
            let dir = self.data_dir.join("web");
            let _lock = password::lock(&dir)?;
            let password::PasswordState::Ready(mut v) = password::read_state(&self.data_dir) else {
                bail!("no password");
            };
            if !live() {
                bail!("request canceled");
            }
            v.verifier_id = password::random_id()?;
            v.updated_at_ms = self.clock.now_ms();
            password::publish(&dir, v)
        })();
        match result {
            Ok(()) => AuthResponse::error(204, ""),
            Err(_) => AuthResponse::error(503, "session_store_unavailable"),
        }
    }
    pub fn session_json(&self, s: &SessionInfo) -> AuthResponse {
        AuthResponse {
            status: 200,
            json: serde_json::json!({"session":s}),
            retry_after_s: None,
        }
    }
    pub fn touch(&self, s: &SessionInfo) -> Result<(), RateLimited> {
        self.limits.lock().unwrap_or_else(|e| e.into_inner()).api(
            &s.id,
            s.idle_expires_at_ms,
            self.clock.now_ms(),
        )
    }
    pub fn public(&self) -> Result<(), RateLimited> {
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .public(self.clock.now_ms())
    }
}
