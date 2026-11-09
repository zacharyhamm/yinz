//! Strictly local, read-only web diagnostics, plus guarded dead-record cleanup.
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    path::Path,
    process::Stdio,
    time::Duration,
};
use yinz_cli::args::CliArgs;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::web::{WebConfig, DEFAULT_WEB_HTTPS_PORT_HINT};
use yinz_web::auth::{password, Clock, SystemClock};

// No shell, bounded output and wall time. kill_on_drop plus explicit kill/wait on every
// failure ensures a timed-out diagnostic cannot leave a child or a pipe reader running.
async fn diagnostic(program: &Path, args: &[&str]) -> Option<Value> {
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 256 * 1024 {
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    })
    .await
    .ok()
    .flatten();
    if child.id().is_some() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
#[derive(Debug, PartialEq, Eq)]
struct ProxyBackend {
    address: SocketAddr,
    localhost: bool,
}
fn backend(proxy: &str) -> Option<ProxyBackend> {
    let url = url::Url::parse(proxy).ok()?;
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.trim_matches(['[', ']']);
    let ip = if host.eq_ignore_ascii_case("localhost") {
        "127.0.0.1".parse().ok()?
    } else {
        host.parse().ok()?
    };
    Some(ProxyBackend {
        address: SocketAddr::new(ip, url.port_or_known_default()?),
        localhost: host.eq_ignore_ascii_case("localhost"),
    })
}
fn same_backend(proxy: &ProxyBackend, target: SocketAddr) -> bool {
    let a = proxy.address;
    a.port() == target.port()
        && (a.ip() == target.ip()
            || (proxy.localhost
                && (target.ip() == std::net::Ipv4Addr::LOCALHOST
                    || target.ip() == std::net::Ipv6Addr::LOCALHOST))
            || (target.ip().is_unspecified()
                && (proxy.localhost || a.ip().is_loopback())
                && a.is_ipv4() == target.is_ipv4()))
}
#[derive(Debug, PartialEq, Eq)]
enum Exposure {
    Unknown,
    NoMatch,
    Exposed,
    PossiblyExposed,
}
fn exposure(config: &Value, targets: &[SocketAddr]) -> Exposure {
    fn collect(c: &Value, web: &mut Vec<(String, String)>, funnel: &mut Vec<String>) -> Option<()> {
        let obj = c.as_object()?;
        if let Some(w) = obj.get("Web").filter(|v| !v.is_null()) {
            for (host, site) in w.as_object()? {
                for handler in site.get("Handlers")?.as_object()?.values() {
                    if let Some(p) = handler.get("Proxy") {
                        web.push((host.to_ascii_lowercase(), p.as_str()?.into()));
                    }
                }
            }
        }
        if let Some(f) = obj.get("AllowFunnel").filter(|v| !v.is_null()) {
            for (host, allow) in f.as_object()? {
                if allow.as_bool()? {
                    funnel.push(host.to_ascii_lowercase());
                }
            }
        }
        if let Some(f) = obj.get("Foreground").filter(|v| !v.is_null()) {
            for child in f.as_object()?.values() {
                collect(child, web, funnel)?;
            }
        }
        if obj
            .get("Services")
            .is_some_and(|v| v.as_object().is_none_or(|m| !m.is_empty()))
        {
            return None;
        }
        if let Some(tcp) = obj.get("TCP").filter(|v| !v.is_null()) {
            for (port, handler) in tcp.as_object()? {
                if port.parse::<std::num::NonZeroU16>().is_err() {
                    return None;
                }
                for (key, value) in handler.as_object()? {
                    match key.as_str() {
                        "HTTPS" | "HTTP" => {
                            value.as_bool()?;
                        },
                        "TCPForward" => {
                            if !value.as_str()?.is_empty() {
                                return None;
                            }
                        },
                        "TerminateTLS" => {
                            value.as_str()?;
                        },
                        _ => return None,
                    }
                }
            }
        }
        // Unknown top-level fields might describe a new forwarding scope. Never claim privacy.
        if obj.keys().any(|k| {
            !matches!(
                k.as_str(),
                "TCP" | "Web" | "AllowFunnel" | "Foreground" | "Services" | "ETag"
            )
        }) {
            return None;
        }
        Some(())
    }
    let (mut web, mut funnel) = (vec![], vec![]);
    let valid = collect(config, &mut web, &mut funnel).is_some();
    if web.iter().any(|(h, p)| {
        funnel.contains(h)
            && backend(p).is_some_and(|a| targets.iter().any(|b| same_backend(&a, *b)))
    }) {
        return Exposure::Exposed;
    }
    // The record does not establish IPV6_V6ONLY. IPv4 loopback may reach an IPv6
    // wildcard on the same port; do not affirmatively classify it as unrelated.
    if web.iter().any(|(h, p)| {
        funnel.contains(h)
            && backend(p).is_some_and(|a| {
                targets.iter().any(|b| {
                    a.address.port() == b.port()
                        && b.ip() == std::net::Ipv6Addr::UNSPECIFIED
                        && a.address.is_ipv4()
                        && a.address.ip().is_loopback()
                })
            })
    }) {
        return Exposure::PossiblyExposed;
    }
    if !valid {
        Exposure::Unknown
    } else {
        Exposure::NoMatch
    }
}
fn describe_config(c: &WebConfig) -> String {
    format!("enabled={} bind={} plugin={} session_days={} allowed_hosts={:?} allowed_origins={:?} tailscale_users={:?}",
        c.enabled(),c.bind_addr().map(|a|a.to_string()).unwrap_or_else(|_|"invalid".into()),c.plugin_or_default(),c.session_days_or_default(),c.allowed_hosts,c.allowed_origins,c.tailscale_users)
}
/// Display only local store/config selectors. Single quotes keep shell syntax literal.
pub(crate) fn suggested_command(opts: &CliArgs, data: &Path, action: &[&str]) -> String {
    fn quote(path: &Path) -> Option<String> {
        let text = path.to_str()?;
        if text.chars().any(char::is_control) {
            return None;
        }
        Some(format!("'{}'", text.replace('\'', "'\\''")))
    }
    let mut command = String::from("yinz");
    for (flag, path) in [
        ("--data-dir", Some(data)),
        ("--config", opts.config.as_deref()),
        ("--config-dir", opts.config_dir.as_deref()),
    ] {
        if let Some(path) = path {
            let Some(quoted) = quote(path) else {
                return format!("manually run yinz {} with --data-dir={data:?}, --config={:?}, --config-dir={:?} (paths cannot be represented as a shell command)", action.join(" "), opts.config, opts.config_dir);
            };
            command.push_str(&format!(" {flag} {quoted}"));
        }
    }
    command.push(' ');
    command.push_str(&action.join(" "));
    command
}
pub(crate) fn run(opts: &CliArgs, https_port: Option<u16>) -> Result<i32> {
    let data = opts
        .data_dir
        .clone()
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
    let config = match yinz_cli::setup::load_config(opts) {
        Ok(c) => c.web,
        Err(_) => {
            println!(
                "Warning: configuration could not be loaded; configured values below are defaults."
            );
            WebConfig::default()
        },
    };
    println!(
        "Configured (applies at session start): {}",
        describe_config(&config)
    );
    println!("Data directory: {}", data.display());
    let provision = suggested_command(opts, &data, &["set-password"]);
    let revoke = suggested_command(opts, &data, &["set-password", "--revoke-sessions"]);
    match password::read_state(&data) {
        password::PasswordState::Missing => {
            println!("Password: Missing; enabled listeners wait for {provision}")
        },
        password::PasswordState::Invalid(_) => {
            println!("Password: Invalid; repair with {provision}")
        },
        password::PasswordState::Ready(v) => {
            println!("Password: Ready; updated_at_ms={}", v.updated_at_ms)
        },
    }
    match yinz_web::auth::sessions::summaries(&data, SystemClock.now_ms()) {
        Ok(rows) => {
            println!("Active browser sessions: {}", rows.len());
            for s in rows {
                println!(
                    "  {} {:?} created={} last_seen={} expires={} idle_expires={}",
                    s.id,
                    s.label,
                    s.created_at_ms,
                    s.last_seen_ms,
                    s.expires_at_ms,
                    s.idle_expires_at_ms
                );
            }
        },
        Err(_) => println!(
            "Browser sessions: unavailable (unsafe or invalid store; recover with {revoke})"
        ),
    }
    let listeners = yinz_web::listener_file::inspect(&data).unwrap_or_else(|_| {
        println!("Listener records: unavailable; effective setup unknown");
        vec![]
    });
    let mut targets = vec![];
    for observed in listeners {
        let verified = observed.is_current();
        let r = observed.record;
        println!(
            "Listener {}: recorded session {:?}; responding session {:?}",
            r.address, r.session, observed.responding_session
        );
        if verified {
            if !targets.contains(&r.address) {
                targets.push(r.address);
            }
            match r.startup_config {
                Some(c) => println!("  Effective startup config: {}", describe_config(&c)),
                None => println!("  Effective policy: unknown (legacy record)"),
            }
        } else {
            println!("  Effective policy: unverified; record may describe an older listener");
        }
    }
    if targets.is_empty() {
        println!(
            "No verified active listener; runtime state unknown. {}",
            if config.enabled() {
                "Enabled sessions may be waiting for a password or port."
            } else {
                "Configured web is disabled."
            }
        );
    }
    let port = https_port
        .or(config.https_port_hint)
        .unwrap_or(DEFAULT_WEB_HTTPS_PORT_HINT);
    let configured = config.bind_addr().ok();
    if targets.is_empty()
        && let Some(a) = configured
    {
        if a.port() != 0 {
            targets.push(a);
            println!("Guidance target is configured-only, not a verified listener.");
        } else {
            println!("Configured ephemeral port 0: start a session to discover its actual Serve backend.");
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start diagnostics runtime")?;
    let (status, serve) = rt.block_on(async {
        tokio::join!(
            diagnostic(Path::new("tailscale"), &["status", "--json"]),
            diagnostic(Path::new("tailscale"), &["serve", "status", "--json"])
        )
    });
    for target in &targets {
        let mut target = *target;
        if target.ip().is_unspecified() {
            target.set_ip(if target.is_ipv4() {
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
            });
        }
        println!("Suggested: tailscale serve --bg --https={port} http://{target}");
    }
    if let Some(dns) = status
        .as_ref()
        .and_then(|v| v.pointer("/Self/DNSName"))
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('.'))
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        })
    {
        println!("Expected HTTPS origin: https://{dns}:{port}");
    } else {
        println!("Tailscale DNS name unavailable; expected HTTPS origin unknown.");
    }
    let exposure = serve
        .as_ref()
        .map(|v| exposure(v, &targets))
        .unwrap_or(Exposure::Unknown);
    let message=match exposure {
        Exposure::Exposed=>"WARNING: Funnel exposes this web backend publicly. Disable that Funnel mapping before use.",
        Exposure::PossiblyExposed=>"WARNING: Reported Funnel mapping may reach this IPv6 wildcard listener over dual-stack IPv4; reachability is unverified. Inspect or disable that mapping before relying on privacy.",
        Exposure::Unknown=>"Funnel exposure unknown: Tailscale diagnostics unavailable or unsupported.",
        Exposure::NoMatch=>"No matching Funnel mapping in reported device Serve configuration; this does not verify all network exposure.",
    };
    println!("{message}");
    println!("Serve proxy/header and phone smoke checks remain operator checks; status never changes Tailscale configuration.");
    Ok(
        if matches!(exposure, Exposure::Exposed | Exposure::PossiblyExposed) {
            1
        } else {
            0
        },
    )
}
#[cfg(test)]
#[path = "unit/web_cli.rs"]
mod tests;
