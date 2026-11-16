//! Web Push subscriptions shared by all local sessions using this data directory.
//! Push is independent of terminal clients and never runs on the plugin thread.
use crate::auth::{password, sessions, Clock, SessionInfo, SystemClock};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use isahc::config::Configurable;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};
use yinz_runtime::errors::prelude::*;
use yinz_types::notifications::NotificationResolution;

const MAX_STORE: u64 = 256 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Subscription {
    owner: String,
    verifier: String,
    endpoint: String,
    p256dh: String,
    auth: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    private_key: String,
    subscriptions: Vec<Subscription>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    endpoint: String,
    #[serde(default)]
    keys: Option<Keys>,
    #[serde(default, rename = "expirationTime")]
    _expiration_time: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keys {
    p256dh: String,
    auth: String,
}

fn read(data: &Path) -> Result<Store> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(data.join("web/push.json"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(e).context("open push subscriptions"),
    };
    password::validate_private(&file)?;
    let mut bytes = Vec::new();
    file.take(MAX_STORE + 1)
        .read_to_end(&mut bytes)
        .context("read push subscriptions")?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_STORE, "push store too large");
    serde_json::from_slice(&bytes).context("decode push subscriptions")
}
fn write(data: &Path, store: &Store) -> Result<()> {
    let bytes = serde_json::to_vec(store).context("encode push subscriptions")?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_STORE, "push store too large");
    password::atomic_write(&data.join("web"), "push.json", &bytes)
}
fn verifier(data: &Path) -> Result<String> {
    match password::read_state(data) {
        password::PasswordState::Ready(v) => Ok(v.verifier_id),
        _ => bail!("web password unavailable"),
    }
}
fn active(data: &Path, verifier: &str, owner: &str, now: i64) -> Result<bool> {
    Ok(sessions::read(data, verifier)?
        .sessions
        .iter()
        .any(|row| row.id == owner && row.valid(now)))
}
fn valid_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.fragment().is_none()
        && endpoint.len() <= 2048
        && url.host_str().is_some_and(|host| {
            host == "fcm.googleapis.com"
                || host == "updates.push.services.mozilla.com"
                || host.ends_with(".push.apple.com")
                || host.ends_with(".notify.windows.com")
        })
}
fn valid_keys(keys: &Keys) -> bool {
    URL_SAFE_NO_PAD
        .decode(&keys.p256dh)
        .is_ok_and(|key| key.len() == 65 && key[0] == 4)
        && URL_SAFE_NO_PAD
            .decode(&keys.auth)
            .is_ok_and(|key| key.len() == 16)
}

/// Native authenticated API. Hold the same cross-process lock as login/revocation.
/// A missing keys object removes this sign-in's subscription at the endpoint.
pub(crate) fn api(
    data: &Path,
    session: &SessionInfo,
    body: Option<&[u8]>,
) -> Result<serde_json::Value> {
    let dir = password::private_dir(data)?;
    let _lock = password::lock(&dir)?;
    let verifier = verifier(data)?;
    anyhow::ensure!(
        active(data, &verifier, &session.id, SystemClock.now_ms())?,
        "sign-in revoked"
    );
    let mut store = read(data)?;
    let changed = store.private_key.is_empty() || body.is_some();
    if store.private_key.is_empty() {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| anyhow!("generate push signing key"))?;
        store.private_key = URL_SAFE_NO_PAD.encode(bytes);
    }
    let signature = VapidSignatureBuilder::from_base64_no_sub(&store.private_key)
        .context("read push signing key")?;
    if let Some(body) = body {
        let update: Update = serde_json::from_slice(body).context("invalid push subscription")?;
        anyhow::ensure!(valid_endpoint(&update.endpoint), "invalid push endpoint");
        if let Some(keys) = &update.keys {
            anyhow::ensure!(valid_keys(keys), "invalid push encryption keys");
        }
        store.subscriptions.retain(|row| {
            row.verifier == verifier
                && !(row.endpoint == update.endpoint
                    && (update.keys.is_some() || row.owner == session.id))
        });
        if let Some(keys) = update.keys {
            anyhow::ensure!(
                store.subscriptions.len() < 64,
                "too many push subscriptions"
            );
            store.subscriptions.push(Subscription {
                owner: session.id.clone(),
                verifier,
                endpoint: update.endpoint,
                p256dh: keys.p256dh,
                auth: keys.auth,
            });
        }
    }
    if changed {
        write(data, &store)?;
    }
    Ok(serde_json::json!({"public_key": URL_SAFE_NO_PAD.encode(signature.get_public_key())}))
}

fn targets(data: &Path) -> Result<(String, Vec<Subscription>)> {
    targets_at(data, SystemClock.now_ms())
}

fn targets_at(data: &Path, now: i64) -> Result<(String, Vec<Subscription>)> {
    // Avoid creating web state for sessions whose owner has never enabled push.
    if !data.join("web/push.json").exists() {
        return Ok((String::new(), vec![]));
    }
    let dir = password::private_dir(data)?;
    let _lock = password::lock(&dir)?;
    let verifier = verifier(data)?;
    let mut store = read(data)?;
    let sessions = sessions::read(data, &verifier)?;
    let previous = store.subscriptions.len();
    store.subscriptions.retain(|row| {
        row.verifier == verifier
            && valid_endpoint(&row.endpoint)
            && sessions
                .sessions
                .iter()
                .any(|s| s.id == row.owner && s.valid(now))
    });
    if store.subscriptions.len() != previous {
        write(data, &store)?;
    }
    Ok((store.private_key, store.subscriptions))
}
fn authorized(data: &Path, target: &Subscription) -> Result<bool> {
    let dir = password::private_dir(data)?;
    let _lock = password::lock(&dir)?;
    Ok(verifier(data)? == target.verifier
        && active(data, &target.verifier, &target.owner, SystemClock.now_ms())?
        && read(data)?.subscriptions.contains(target))
}
fn remove(data: &Path, target: &Subscription) -> Result<()> {
    let dir = password::private_dir(data)?;
    let _lock = password::lock(&dir)?;
    let mut store = read(data)?;
    store.subscriptions.retain(|s| {
        !(s.owner == target.owner
            && s.endpoint == target.endpoint
            && s.verifier == target.verifier
            && s.auth == target.auth
            && s.p256dh == target.p256dh)
    });
    write(data, &store)
}
fn truncate(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn payload(session: &str, notification: &NotificationResolution) -> String {
    let mut body = truncate(&notification.body, 1500);
    let session_tag = URL_SAFE_NO_PAD.encode(&sha2::Sha256::digest(session.as_bytes())[..12]);
    loop {
        let payload = serde_json::json!({
            "title": format!("{} · {}", truncate(session, 128), truncate(&notification.title, 128)),
            "body": body,
            "tag": format!("{}:{}:{:?}", session_tag, notification.original.received_at_unix_ms, notification.original.id),
        }).to_string();
        // Stay below the encrypted record limit even when JSON escapes expand the text.
        if payload.len() <= 3000 {
            return payload;
        }
        body = truncate(body, body.len() / 2);
    }
}

fn message(
    key: &str,
    subscription: &Subscription,
    payload: &str,
) -> Result<web_push::WebPushMessage> {
    let info = SubscriptionInfo::new(
        &subscription.endpoint,
        &subscription.p256dh,
        &subscription.auth,
    );
    let mut signature =
        VapidSignatureBuilder::from_base64(key, &info).context("prepare push signature")?;
    signature.add_claim("sub", "https://github.com/zacharyhamm/yinz");
    let mut message = WebPushMessageBuilder::new(&info);
    message.set_payload(ContentEncoding::Aes128Gcm, payload.as_bytes());
    message.set_vapid_signature(signature.build().context("sign push")?);
    message.set_ttl(3600);
    message.build().context("encrypt push")
}

/// Bounded per-session queue; dropping it cancels delivery on session shutdown.
pub struct Dispatcher {
    queue: tokio::sync::mpsc::Sender<(String, NotificationResolution)>,
    task: tokio::task::JoinHandle<()>,
}
impl Dispatcher {
    pub fn start(data: PathBuf, runtime: &tokio::runtime::Handle) -> Self {
        let (queue, mut receiver) =
            tokio::sync::mpsc::channel::<(String, NotificationResolution)>(128);
        let task = runtime.spawn(async move {
            let mut client = None;
            while let Some((session, notification)) = receiver.recv().await {
                let data_clone = data.clone();
                let Ok(Ok((key, subscriptions))) =
                    tokio::task::spawn_blocking(move || targets(&data_clone)).await
                else {
                    log::warn!("cannot read active Web Push subscriptions");
                    continue;
                };
                if subscriptions.is_empty() {
                    continue;
                }
                if client.is_none() {
                    match isahc::HttpClient::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .redirect_policy(isahc::config::RedirectPolicy::None)
                        .build()
                    {
                        Ok(built) => client = Some(built),
                        Err(_) => {
                            log::error!("cannot initialize Web Push client");
                            continue;
                        },
                    }
                }
                let Some(client) = client.as_ref() else {
                    continue;
                };
                let payload = payload(&session, &notification);
                for subscription in subscriptions {
                    let data_clone = data.clone();
                    let target = subscription.clone();
                    if !matches!(
                        tokio::task::spawn_blocking(move || authorized(&data_clone, &target)).await,
                        Ok(Ok(true))
                    ) {
                        continue;
                    }
                    let Ok(message) = message(&key, &subscription, &payload) else {
                        log::warn!("cannot encrypt Web Push notification");
                        continue;
                    };
                    let request =
                        web_push::request_builder::build_request::<isahc::AsyncBody>(message);
                    match client.send_async(request).await {
                        Ok(response) if matches!(response.status().as_u16(), 404 | 410) => {
                            let data_clone = data.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                remove(&data_clone, &subscription)
                            })
                            .await;
                        },
                        Ok(response) if response.status().is_success() => {},
                        _ => log::warn!("Web Push delivery failed"),
                    }
                }
            }
        });
        Self { queue, task }
    }
    pub fn notify(&self, session: String, notification: &NotificationResolution) {
        if !notification.suppressed
            && self
                .queue
                .try_send((session, notification.clone()))
                .is_err()
        {
            log::warn!("Web Push queue full or unavailable");
        }
    }
}
impl Drop for Dispatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
#[path = "unit/push.rs"]
mod tests;
