//! Strict credential decoding and bounded off-runtime verification.
use super::{password, Auth, AuthResponse, RateLimited};
use serde::Deserialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use zeroize::Zeroizing;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    #[serde(deserialize_with = "secret")]
    password: Zeroizing<String>,
    #[serde(default, deserialize_with = "optional_label")]
    label: Option<String>,
    #[serde(default)]
    remember: bool,
}
fn optional_label<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
fn secret<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}
fn label(text: &str) -> Option<String> {
    let s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.chars().any(char::is_control) && s.len() <= 128).then_some(s)
}
struct Live(Arc<AtomicBool>);
impl Drop for Live {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl AuthResponse {
    pub(crate) fn limited(r: RateLimited) -> Self {
        let mut response = Self::error(429, "rate_limited");
        response.retry_after_s = Some(r.retry_after_s);
        response
    }
}
impl Auth {
    pub async fn login(
        self: &Arc<Self>,
        body: Zeroizing<Vec<u8>>,
        user_agent: Option<&str>,
        client_key: &str,
    ) -> AuthResponse {
        self.login_guarded(body, user_agent, client_key, Arc::new(|| true))
            .await
    }
    pub(crate) async fn login_guarded(
        self: &Arc<Self>,
        body: Zeroizing<Vec<u8>>,
        user_agent: Option<&str>,
        client_key: &str,
        live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> AuthResponse {
        let Ok(dto) = serde_json::from_slice::<Login>(&body) else {
            return AuthResponse::error(400, "invalid_request");
        };
        if dto.password.len() > 1024 || dto.password.contains('\0') {
            return AuthResponse::error(400, "invalid_request");
        }
        let supplied = match dto.label {
            Some(s) if s.len() <= 128 => match label(&s) {
                Some(s) => Some(s),
                None => return AuthResponse::error(400, "invalid_request"),
            },
            Some(_) => return AuthResponse::error(400, "invalid_request"),
            None => None,
        };
        let label = supplied.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            let agent = user_agent
                .unwrap_or("Browser")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let mut s = if agent.chars().any(char::is_control) {
                "Browser".into()
            } else {
                agent
            };
            while s.len() > 128 {
                s.pop();
            }
            if s.is_empty() {
                s = "Browser".into();
            }
            s
        });
        if let Err(r) = self
            .limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(client_key, self.clock.now_ms())
        {
            return AuthResponse::limited(r);
        }
        let Ok(admission) = self.admission.clone().try_acquire_owned() else {
            return AuthResponse::limited(RateLimited { retry_after_s: 1 });
        };
        let Ok(active) = self.verification.clone().acquire_owned().await else {
            return AuthResponse::error(503, "unavailable");
        };
        if let Err(r) = self
            .limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(client_key, self.clock.now_ms())
        {
            return AuthResponse::limited(r);
        }
        if !live() {
            return AuthResponse::error(503, "not_serving");
        }
        let auth = self.clone();
        let key = client_key.to_owned();
        let verified = tokio::task::spawn_blocking(move || {
            // Owned permits stay with Argon2 even if the HTTP future is canceled.

            let password::PasswordState::Ready(v) = password::read_state(&auth.data_dir) else {
                return None;
            };
            #[cfg(test)]
            if let Some(hook) = auth
                .verify_hook
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                hook();
            }
            let ok = password::verify(&v, dto.password.as_bytes());
            let mut limits = auth.limits.lock().unwrap_or_else(|e| e.into_inner());
            if ok {
                limits.success(&key, auth.clock.now_ms());
            } else {
                limits.failure(&key, auth.clock.now_ms());
            }
            Some((v, ok, admission, active))
        })
        .await;
        let Ok(Some((verifier, valid, admission, active))) = verified else {
            return AuthResponse::error(503, "no_password");
        };
        if !valid {
            return AuthResponse::error(401, "invalid_credentials");
        }
        let cancellation = Live(Arc::new(AtomicBool::new(true)));
        let check = cancellation.0.clone();
        let auth = self.clone();
        let remember = dto.remember;
        let result = tokio::task::spawn_blocking(move || {
            let (_admission, _active) = (admission, active);
            let mut store = auth.store.lock().unwrap_or_else(|e| e.into_inner());
            #[cfg(test)]
            if let Some(hook) = auth
                .commit_hook
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                hook();
            }
            let Ok(_lock) = password::lock(&auth.data_dir.join("web")) else {
                return AuthResponse::error(503, "session_store_unavailable");
            };
            let password::PasswordState::Ready(current) = password::read_state(&auth.data_dir)
            else {
                return AuthResponse::error(503, "no_password");
            };
            if current.verifier_id != verifier.verifier_id {
                return AuthResponse::error(401, "invalid_credentials");
            }
            if !check.load(Ordering::Acquire) || !live() {
                return AuthResponse::error(503, "unavailable");
            }
            let issued = store.issue(
                &auth.data_dir,
                &current.verifier_id,
                label,
                remember,
                auth.config.session_days,
                auth.clock.now_ms(),
            );
            match issued {
                Ok((token, s)) if check.load(Ordering::Acquire) && live() => AuthResponse {
                    status: 200,
                    json: serde_json::json!({"token":token,"session":s}),
                    retry_after_s: None,
                },
                Ok((_, s)) => {
                    // Cancellation during publication cannot leave an abandoned issuance.
                    // The password lock is still held; no concurrent edit can be lost.
                    store
                        .discard_issued(&auth.data_dir, &current.verifier_id, &s.id)
                        .map(|()| AuthResponse::error(503, "unavailable"))
                        .unwrap_or_else(|_| AuthResponse::error(503, "session_store_unavailable"))
                },
                Err(_) => AuthResponse::error(503, "session_store_unavailable"),
            }
        })
        .await
        .unwrap_or_else(|_| AuthResponse::error(503, "session_store_unavailable"));
        drop(cancellation);
        result
    }
}
#[cfg(test)]
#[path = "../unit/endpoints.rs"]
mod tests;
