//! Local provider account observations, called only through AgentControl's RunCommands authority.
use isahc::{config::RedirectPolicy, prelude::*, HttpClient, Request};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::Read,
    path::Path,
    sync::{Arc, Condvar, LazyLock, Mutex},
    time::{Duration, Instant, SystemTime},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};

const DATA_LIMIT: u64 = 1024 * 1024;
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Default)]
struct FetchMetadata {
    throttled: bool,
    retry_after: Option<Duration>,
}

#[derive(Default)]
struct CacheEntry {
    result: Option<(ProviderUsage, Instant)>,
    last_success: Option<ProviderUsage>,
    pending: bool,
    throttle_streak: u32,
}

#[derive(Default)]
struct ProviderCache {
    entry: Mutex<CacheEntry>,
    completed: Condvar,
}

impl ProviderCache {
    fn collect(
        &self,
        provider: &str,
        now: impl Fn() -> Instant,
        fetch: impl FnOnce() -> (ProviderUsage, FetchMetadata),
    ) -> ProviderUsage {
        let mut entry = self.entry.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((result, deadline)) = &entry.result
                && now() < *deadline
            {
                return result.clone();
            }
            if !entry.pending {
                break;
            }
            entry = self
                .completed
                .wait(entry)
                .unwrap_or_else(|e| e.into_inner());
        }
        entry.pending = true;
        drop(entry);

        // A collector panic must not leave every later caller waiting forever.
        let (mut result, metadata) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fetch))
            .unwrap_or_else(|_| {
                (
                    failure(
                        provider,
                        ProviderUsageStatus::Error,
                        "Could not read provider limits.",
                    ),
                    FetchMetadata::default(),
                )
            });
        let mut entry = self.entry.lock().unwrap_or_else(|e| e.into_inner());
        match result.status {
            ProviderUsageStatus::Ok => entry.last_success = Some(result.clone()),
            ProviderUsageStatus::Error => {
                if let Some(previous) = &entry.last_success {
                    result.windows = previous.windows.clone();
                    result.observed_at_ms = previous.observed_at_ms;
                    result.plan = previous.plan.clone();
                }
            },
            // Drop retained quotas when authentication expires or reporting becomes unavailable.
            _ => entry.last_success = None,
        }
        let ttl = if metadata.throttled {
            entry.throttle_streak = entry.throttle_streak.saturating_add(1);
            let backoff = Duration::from_secs(
                (900 * (1u64 << entry.throttle_streak.saturating_sub(1).min(2))).min(3600),
            );
            backoff.max(metadata.retry_after.unwrap_or_default())
        } else {
            entry.throttle_streak = 0;
            Duration::from_secs(if provider == "claude" { 300 } else { 60 })
        };
        let completed = now();
        // Reject unrepresentable server deadlines rather than overflowing Instant.
        let deadline = completed
            .checked_add(ttl)
            .unwrap_or_else(|| completed + Duration::from_secs(3600));
        entry.result = Some((result.clone(), deadline));
        entry.pending = false;
        self.completed.notify_all();
        result
    }
}

static USAGE_CACHE: LazyLock<Mutex<HashMap<String, Arc<ProviderCache>>>> =
    LazyLock::new(Mutex::default);

pub(crate) fn collect(provider: &str) -> ProviderUsage {
    if !matches!(provider, "codex" | "claude") {
        return collect_uncached(provider, &mut FetchMetadata::default());
    }
    let cache = USAGE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(provider.to_owned())
        .or_default()
        .clone();
    cache.collect(provider, Instant::now, || {
        let mut metadata = FetchMetadata::default();
        let usage = collect_uncached(provider, &mut metadata);
        (usage, metadata)
    })
}

fn collect_uncached(provider: &str, metadata: &mut FetchMetadata) -> ProviderUsage {
    match provider {
        "codex" => {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("start usage reader")
                .and_then(|runtime| {
                    runtime.block_on(yinz_codex::provider_usage(Path::new("codex")))
                });
            result.unwrap_or_else(|_| failure("codex", ProviderUsageStatus::Error,
                "Could not read Codex limits. Check that Codex is installed, supports app-server, and is signed in."))
        },
        "claude" => claude(metadata),
        _ => failure(
            provider,
            ProviderUsageStatus::Unsupported,
            "Usage reporting is not available for this provider.",
        ),
    }
}

fn failure(provider: &str, status: ProviderUsageStatus, message: &str) -> ProviderUsage {
    ProviderUsage {
        provider: provider.into(),
        status,
        observed_at_ms: None,
        plan: None,
        windows: vec![],
        message: Some(message.into()),
    }
}

fn now_ms() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

// Never derive Debug or Serialize: the credential must stay inside this collector.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCredentials {
    access_token: String,
    expires_at: Option<u64>,
    subscription_type: Option<String>,
}

fn credentials(raw: &[u8]) -> Result<ClaudeCredentials> {
    #[derive(Deserialize)]
    struct File {
        #[serde(rename = "claudeAiOauth")]
        oauth: ClaudeCredentials,
    }
    let parsed: File =
        serde_json::from_slice(raw).map_err(|_| anyhow!("Claude credentials are malformed"))?;
    if parsed.oauth.access_token.is_empty() {
        bail!("Claude credentials have no access token");
    }
    Ok(parsed.oauth)
}

fn bounded_read(reader: impl Read) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader
        .take(DATA_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|_| anyhow!("could not read provider data"))?;
    if data.len() as u64 > DATA_LIMIT {
        bail!("provider data exceeded its size limit");
    }
    Ok(data)
}

fn file_credentials(root: &Path) -> Result<Option<ClaudeCredentials>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(root.join(".credentials.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("could not open Claude credentials"),
    };
    if !file
        .metadata()
        .context("inspect Claude credential file")?
        .is_file()
    {
        bail!("Claude credentials are not a regular file");
    }
    credentials(&bounded_read(file)?).map(Some)
}

#[cfg(any(target_os = "macos", test))]
fn keychain_credentials(raw: &[u8]) -> Result<ClaudeCredentials> {
    if let Ok(credentials) = credentials(raw) {
        return Ok(credentials);
    }
    let text = std::str::from_utf8(raw)
        .context("invalid Keychain output")?
        .trim();
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid Keychain credentials");
    }
    let decoded = (0..text.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&text[offset..offset + 2], 16).context("invalid Keychain encoding")
        })
        .collect::<Result<Vec<_>>>()?;
    credentials(&decoded)
}

#[cfg(target_os = "macos")]
fn mac_credentials() -> Option<ClaudeCredentials> {
    use tokio::io::AsyncReadExt;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let username = nix::unistd::User::from_uid(nix::unistd::Uid::current())
        .ok()
        .flatten()
        .map(|user| user.name);
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        for account in username
            .as_deref()
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
        {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let mut command = tokio::process::Command::new("/usr/bin/security");
            command.args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ]);
            if let Some(account) = account {
                command.args(["-a", account]);
            }
            let mut child = command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .ok()?;
            let mut output = child.stdout.take()?.take(DATA_LIMIT + 1);
            let result = tokio::time::timeout_at(deadline, async {
                let mut data = Vec::new();
                output.read_to_end(&mut data).await.ok()?;
                if data.len() as u64 > DATA_LIMIT || !child.wait().await.ok()?.success() {
                    return None;
                }
                keychain_credentials(&data).ok()
            })
            .await
            .ok()
            .flatten();
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            if result.is_some() {
                return result;
            }
        }
        None
    })
}

fn claude(metadata: &mut FetchMetadata) -> ProviderUsage {
    let fetch = |token: &str| {
        let (status, body, retry_after) = fetch_claude(token)?;
        metadata.throttled = status == 429;
        metadata.retry_after = retry_after;
        Ok((status, body))
    };
    let config_override = std::env::var_os("CLAUDE_CONFIG_DIR");
    let root = yinz_runtime::agent::paths::config_root(
        "claude",
        config_override.clone(),
        None,
        None,
        None,
        || {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(std::path::PathBuf::from)
                .context("home directory is unavailable")
        },
    );
    let Ok(root) = root else {
        return failure(
            "claude",
            ProviderUsageStatus::Unavailable,
            "The local Claude configuration directory is unavailable.",
        );
    };
    // A custom directory can belong to another account; never fall back to the default Keychain.
    #[cfg(target_os = "macos")]
    if config_override.as_ref().is_none_or(|root| root.is_empty()) {
        if let Some(credentials) = mac_credentials() {
            return claude_with(credentials, now_ms(), fetch);
        }
    }
    match file_credentials(&root) {
        Ok(Some(credentials)) => claude_with(credentials, now_ms(), fetch),
        Ok(None) => failure(
            "claude",
            ProviderUsageStatus::Unavailable,
            "No local Claude OAuth credentials are available. Sign in to Claude to view limits.",
        ),
        Err(_) => failure(
            "claude",
            ProviderUsageStatus::Error,
            "Could not read local Claude OAuth credentials.",
        ),
    }
}

fn claude_request(token: &str) -> Result<Request<()>> {
    Request::get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", concat!("yinz/", env!("CARGO_PKG_VERSION")))
        .body(())
        .map_err(|_| anyhow!("could not construct Claude usage request"))
}

fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let deadline: SystemTime = date.with_timezone(&chrono::Utc).into();
    Some(deadline.duration_since(now).unwrap_or_default())
}

fn fetch_claude(token: &str) -> Result<(u16, Vec<u8>, Option<Duration>)> {
    // Plugins only see a generic failure; keep the transport cause diagnosable.
    // These errors never contain the token, headers or a response body.
    // Every step's context names the Claude usage check, so the chain needs no prefix.
    send_claude(token).inspect_err(|error| log::warn!("{error:#}"))
}

fn send_claude(token: &str) -> Result<(u16, Vec<u8>, Option<Duration>)> {
    let client = HttpClient::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        // Credentials must never follow an upstream redirect to another origin.
        .redirect_policy(RedirectPolicy::None)
        .build()
        .context("create Claude usage client")?;
    let mut response = client
        .send(claude_request(token).context("build Claude usage request")?)
        .map_err(transport_error)?;
    let status = response.status().as_u16();
    let retry = response
        .headers()
        .get("Retry-After")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after(value, SystemTime::now()));
    // Error bodies can contain credentials or account details; do not read or return them.
    let body = if response.status().is_success() {
        bounded_read(response.body_mut()).context("read Claude usage response")?
    } else {
        vec![]
    };
    Ok((status, body, retry))
}

/// Describe a transport failure with the peer, keeping isahc's kind and source chain.
///
/// isahc's `Display` omits its source, which carries the I/O or curl cause.
fn transport_error(error: isahc::Error) -> anyhow::Error {
    let peer = error
        .remote_addr()
        .map(|addr| format!(" (remote {addr})"))
        .unwrap_or_default();
    anyhow::Error::new(error).context(format!("Claude usage request failed{peer}"))
}

fn claude_with(
    credentials: ClaudeCredentials,
    now: Option<u64>,
    fetch: impl FnOnce(&str) -> Result<(u16, Vec<u8>)>,
) -> ProviderUsage {
    if credentials
        .expires_at
        .zip(now)
        .is_some_and(|(expiry, now)| expiry <= now)
    {
        return failure(
            "claude",
            ProviderUsageStatus::Expired,
            "Claude credentials expired. Sign in again to refresh account limits.",
        );
    }
    let (status, body) = match fetch(&credentials.access_token) {
        Ok(response) => response,
        Err(_) => {
            return failure(
                "claude",
                ProviderUsageStatus::Error,
                "Could not fetch Claude account limits.",
            )
        },
    };
    // Plugins see one message for several statuses; the code alone stays
    // diagnosable without logging the response body.
    if !(200..=299).contains(&status) {
        log::warn!("Claude usage check returned HTTP {status}");
    }
    match status {
        401 => {
            return failure(
                "claude",
                ProviderUsageStatus::Expired,
                "Claude authentication expired. Sign in again to refresh account limits.",
            )
        },
        429 => {
            return failure(
                "claude",
                ProviderUsageStatus::Error,
                "Claude throttled this usage check. This does not mean your quota is exhausted.",
            )
        },
        200..=299 => {},
        _ => {
            return failure(
                "claude",
                ProviderUsageStatus::Error,
                "Claude could not return account limits.",
            )
        },
    }
    let windows = serde_json::from_slice::<Value>(&body)
        .map_err(|_| anyhow!("malformed Claude usage response"))
        .and_then(|value| claude_windows(&value));
    match windows {
        Ok(windows) if !windows.is_empty() => ProviderUsage {
            provider: "claude".into(),
            status: ProviderUsageStatus::Ok,
            observed_at_ms: now_ms(),
            plan: credentials.subscription_type.filter(|plan| {
                matches!(
                    plan.as_str(),
                    "free" | "pro" | "max" | "team" | "enterprise"
                )
            }),
            windows,
            message: None,
        },
        Ok(_) => failure(
            "claude",
            ProviderUsageStatus::Unavailable,
            "Claude returned no quota windows for this account.",
        ),
        Err(_) => failure(
            "claude",
            ProviderUsageStatus::Error,
            "Claude returned an invalid usage response.",
        ),
    }
}

fn claude_window(
    raw: &Value,
    label: String,
    percent_field: &str,
) -> Result<Option<ProviderUsageWindow>> {
    if raw.is_null() {
        return Ok(None);
    }
    if !raw.is_object() {
        bail!("invalid Claude quota window");
    }
    if raw.get(percent_field).is_none_or(Value::is_null) {
        return Ok(None);
    }
    let percent = raw[percent_field]
        .as_f64()
        .filter(|percent| percent.is_finite() && *percent >= 0.0)
        .context("invalid Claude usage percentage")?;
    Ok(Some(ProviderUsageWindow {
        label,
        used_percent: percent,
        resets_at_ms: raw["resets_at"]
            .as_str()
            .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
            .and_then(|reset| u64::try_from(reset.timestamp_millis()).ok()),
    }))
}

fn claude_windows(value: &Value) -> Result<Vec<ProviderUsageWindow>> {
    let fields = [
        ("five_hour", "5-hour limit"),
        ("seven_day", "Weekly limit"),
        ("seven_day_sonnet", "Weekly Sonnet"),
        ("seven_day_opus", "Weekly Opus"),
    ];
    if !value.is_object()
        || (!fields.iter().any(|(field, _)| value.get(*field).is_some())
            && value.get("limits").is_none())
    {
        bail!("missing Claude quota windows");
    }
    let mut windows = vec![];
    for (field, label) in fields {
        if let Some(window) = claude_window(&value[field], label.into(), "utilization")? {
            windows.push(window);
        }
    }
    if !value["limits"].is_null() && !value["limits"].is_array() {
        bail!("invalid Claude model quotas");
    }
    if let Some(limits) = value["limits"].as_array() {
        for limit in limits.iter().take(32) {
            if limit["kind"] != "weekly_scoped" {
                continue;
            }
            let Some(model) = limit["scope"]["model"]["display_name"].as_str() else {
                continue;
            };
            let model: String = model
                .chars()
                .filter(|character| !character.is_control())
                .take(80)
                .collect();
            if model.trim().is_empty() {
                continue;
            }
            let label = format!("Weekly {}", model.trim());
            if windows
                .iter()
                .any(|window| window.label.eq_ignore_ascii_case(&label))
            {
                continue;
            }
            if let Some(window) = claude_window(limit, label, "percent")? {
                windows.push(window);
            }
        }
    }
    Ok(windows)
}

#[cfg(test)]
#[path = "unit/provider_usage.rs"]
mod tests;
