//! Read the local account's quota through Codex's supported app-server API.
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};

const RESPONSE_LIMIT: u64 = 1024 * 1024;

/// A one-shot process honors this server's environment instead of an existing daemon's account.
/// No conversation is opened, credentials are never returned, and cancellation kills the child.
pub async fn provider_usage(program: &Path) -> Result<ProviderUsage> {
    let mut child = tokio::process::Command::new(program)
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start Codex usage reader")?;
    let mut input = child.stdin.take().context("open Codex usage input")?;
    let mut output = BufReader::new(child.stdout.take().context("open Codex usage output")?);
    let result = tokio::time::timeout(Duration::from_secs(15), read_usage(&mut input, &mut output))
        .await
        .context("Codex usage request timed out")
        .and_then(|result| result);
    // Reap the one-shot process on success, error, or deadline expiry.
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    result
}

async fn request(
    input: &mut (impl AsyncWrite + Unpin),
    output: &mut (impl AsyncBufRead + Unpin),
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let message = json!({"id": id, "method": method, "params": params});
    input
        .write_all(format!("{message}\n").as_bytes())
        .await
        .context("send Codex usage request")?;
    input.flush().await.context("flush Codex usage request")?;
    loop {
        let mut line = Vec::new();
        let read = (&mut *output)
            .take(RESPONSE_LIMIT + 1)
            .read_until(b'\n', &mut line)
            .await
            .context("read Codex usage response")?;
        if read == 0 || read as u64 > RESPONSE_LIMIT {
            bail!("Codex usage response closed or exceeded its size limit");
        }
        let response: Value = serde_json::from_slice(&line)
            .map_err(|_| anyhow!("Codex usage response was malformed"))?;
        if response.get("method").is_some() {
            // This connection cannot supply external tokens or approve unrelated server requests.
            if response.get("id").is_some() {
                bail!("Codex usage requires additional authentication");
            }
            continue;
        }
        if response["id"] == id {
            if response.get("error").is_some() {
                // RPC errors can contain provider response bodies and must not reach the panel.
                bail!("Codex rejected the usage request");
            }
            return response
                .get("result")
                .cloned()
                .context("missing Codex usage result");
        }
    }
}

async fn read_usage(
    input: &mut (impl AsyncWrite + Unpin),
    output: &mut (impl AsyncBufRead + Unpin),
) -> Result<ProviderUsage> {
    request(
        input,
        output,
        0,
        "initialize",
        json!({
            "clientInfo": {"name": "yinz-usage", "version": env!("CARGO_PKG_VERSION")}
        }),
    )
    .await?
    .as_object()
    .context("malformed Codex initialization")?;
    input
        .write_all(b"{\"method\":\"initialized\"}\n")
        .await
        .context("initialize Codex usage reader")?;
    let account = request(
        input,
        output,
        1,
        "account/read",
        json!({"refreshToken": false}),
    )
    .await?;
    let account = account
        .get("account")
        .context("missing Codex account state")?;
    if account.is_null() {
        return Ok(unavailable(
            ProviderUsageStatus::Unavailable,
            "Sign in to Codex to view account limits.",
        ));
    }
    match account["type"].as_str() {
        Some("chatgpt") => {},
        Some(_) => {
            return Ok(unavailable(
                ProviderUsageStatus::Unsupported,
                "This Codex account does not expose ChatGPT subscription limits.",
            ))
        },
        None => bail!("malformed Codex account state"),
    }
    let limits = request(input, output, 2, "account/rateLimits/read", Value::Null).await?;
    normalize(&limits, account)
}

fn unavailable(status: ProviderUsageStatus, message: &str) -> ProviderUsage {
    ProviderUsage {
        provider: "codex".into(),
        status,
        observed_at_ms: None,
        plan: None,
        windows: vec![],
        message: Some(message.into()),
    }
}

fn normalize(response: &Value, account: &Value) -> Result<ProviderUsage> {
    let limits = response
        .get("rateLimits")
        .filter(|value| value.is_object())
        .context("Codex did not return quota windows")?;
    let mut windows = vec![];
    for (field, fallback) in [
        ("primary", "Primary limit"),
        ("secondary", "Secondary limit"),
    ] {
        let Some(window) = limits.get(field).filter(|window| !window.is_null()) else {
            continue;
        };
        let percent = window["usedPercent"]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .context("invalid Codex usage percentage")?;
        let label = match window["windowDurationMins"].as_u64() {
            Some(300) => "5-hour limit".into(),
            Some(1440) => "Daily limit".into(),
            Some(10080) => "Weekly limit".into(),
            Some(minutes) if minutes > 0 => format!("{minutes}-minute limit"),
            _ => fallback.into(),
        };
        windows.push(ProviderUsageWindow {
            label,
            used_percent: percent,
            resets_at_ms: window["resetsAt"]
                .as_u64()
                .and_then(|value| value.checked_mul(1000)),
        });
    }
    if windows.is_empty() {
        return Ok(unavailable(
            ProviderUsageStatus::Unavailable,
            "Codex returned no quota windows for this account.",
        ));
    }
    let plan = limits["planType"]
        .as_str()
        .or_else(|| account["planType"].as_str())
        .filter(|plan| {
            matches!(
                *plan,
                "free"
                    | "go"
                    | "plus"
                    | "pro"
                    | "team"
                    | "business"
                    | "enterprise"
                    | "edu"
                    | "education"
            )
        })
        .map(str::to_owned);
    Ok(ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
        plan,
        windows,
        message: None,
    })
}

#[cfg(test)]
#[path = "unit/usage.rs"]
mod tests;
