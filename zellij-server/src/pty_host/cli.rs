//! `zellij pty-host list|kill` — the window into supervisors that outlived their
//! session.
//!
//! Not a nicety. A server that is `kill -9`'d (or OOM-killed) deliberately leaves
//! its supervisors running, and nothing else will ever reclaim them, so without
//! this the only evidence a pane survived is a stray process in `ps`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use zellij_utils::data::WarpedPane;
use zellij_utils::errors::prelude::*;

use super::client;
use super::protocol::{self, HostStatus};

/// A supervisor's socket, paired with whatever it had to say for itself.
struct Entry {
    path: PathBuf,
    status: HostStatus,
}

/// Collect every live supervisor, unlinking sockets whose owner is gone.
///
/// A stale socket is the normal residue of a `SIGKILL`'d supervisor, so garbage
/// collecting here rather than reporting it keeps the listing honest.
fn collect() -> Result<Vec<Entry>> {
    let dir = protocol::socket_dir();
    let read_dir = match std::fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", dir.display()));
        },
    };

    let mut entries = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        // Probe before querying, and only garbage collect on a refused connect.
        // That is the one error that proves nobody is listening; a timeout just
        // means the supervisor is busy, and unlinking then would put a live pane
        // permanently out of reach of this command.
        if client::socket_is_dead(&path) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        match client::query(&path) {
            Ok(status) => entries.push(Entry { path, status }),
            Err(e) => {
                eprintln!("Could not query {}: {e:#}", path.display());
            },
        }
    }
    entries.sort_by(|a, b| {
        a.status
            .session_name
            .cmp(&b.status.session_name)
            .then(a.status.terminal_id.cmp(&b.status.terminal_id))
    });
    Ok(entries)
}

pub fn list(as_json: bool) -> Result<()> {
    let entries = collect()?;

    if as_json {
        let statuses: Vec<&HostStatus> = entries.iter().map(|e| &e.status).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&statuses).context("failed to render JSON")?
        );
        return Ok(());
    }

    if entries.is_empty() {
        println!("No pane supervisors are running.");
        return Ok(());
    }

    let rows: Vec<[String; 6]> = entries
        .iter()
        .map(|entry| {
            let s = &entry.status;
            [
                s.host_id.clone(),
                s.session_name.clone().unwrap_or_else(|| "-".to_owned()),
                s.terminal_id.to_string(),
                s.child_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                describe_state(s),
                s.command.clone(),
            ]
        })
        .collect();

    let headers = ["ID", "SESSION", "PANE", "PID", "STATE", "COMMAND"];
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let render = |cells: &[String]| {
        let line: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                // Never pad the last column: it is free-form and padding it just
                // trails whitespace.
                if i + 1 == cells.len() {
                    cell.clone()
                } else {
                    format!("{:width$}", cell, width = widths[i])
                }
            })
            .collect();
        println!("{}", line.join("  "));
    };

    render(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>());
    for row in &rows {
        render(row);
    }
    Ok(())
}

fn describe_state(status: &HostStatus) -> String {
    let age = humanize_age(status.started_at_unix);
    match (status.attached, status.child_exit_status) {
        (_, Some(code)) => match code {
            Some(code) => format!("exited({code}) {age}"),
            None => format!("exited {age}"),
        },
        (true, None) => format!("attached {age}"),
        (false, None) => {
            // The interesting case: a pane whose session is gone.
            let buffered = status.buffered_bytes;
            if buffered > 0 {
                format!("orphaned {age} (+{} buffered)", human_bytes(buffered))
            } else {
                format!("orphaned {age}")
            }
        },
    }
}

fn humanize_age(started_at_unix: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs = now.saturating_sub(started_at_unix);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}K", bytes / 1024)
    } else {
        format!("{}M", bytes / (1024 * 1024))
    }
}

/// Resolve a host id, or any unique prefix of one, to the supervisor it names.
///
/// Shared by `kill` and `adopt` so the two agree on what counts as a match and
/// word the ambiguity the same way.
fn resolve(host_id: &str) -> Result<Entry> {
    let entries = collect()?;
    let mut matches: Vec<Entry> = entries
        .into_iter()
        .filter(|e| e.status.host_id == host_id || e.status.host_id.starts_with(host_id))
        .collect();

    match matches.len() {
        0 => Err(anyhow!("no pane supervisor matches '{host_id}'")),
        1 => Ok(matches.remove(0)),
        _ => {
            let ids: Vec<&str> = matches.iter().map(|e| e.status.host_id.as_str()).collect();
            Err(anyhow!(
                "'{host_id}' is ambiguous; it matches {}",
                ids.join(", ")
            ))
        },
    }
}

pub fn kill(host_id: &str) -> Result<()> {
    let entry = resolve(host_id)?;
    client::terminate_via_socket(&entry.path)
        .with_context(|| format!("failed to terminate {}", entry.status.host_id))?;
    println!(
        "Terminated {} ({}).",
        entry.status.host_id, entry.status.command
    );
    Ok(())
}

/// Where an `adopt` should send its `AdoptPane`, and the pane to put in it.
///
/// This runs as a plain CLI command with no session of its own, so it resolves
/// the supervisor here — failing fast and legibly if it is gone or already
/// spoken for — and leaves the actual adoption to the session, which is the only
/// thing that can give the pane a home.
pub fn adopt_request(host_id: &str, session: Option<String>) -> Result<(String, WarpedPane)> {
    let entry = resolve(host_id)?;
    if entry.status.attached {
        return Err(anyhow!(
            "{} is already attached to session '{}'; there is nothing to recover",
            entry.status.host_id,
            entry.status.session_name.as_deref().unwrap_or("<unknown>")
        ));
    }
    let session = match session.or_else(|| zellij_utils::envs::get_session_name().ok()) {
        Some(session) => session,
        None => {
            return Err(anyhow!(
                "not inside a zellij session; say which one to adopt into with --session"
            ));
        },
    };
    Ok((
        session,
        WarpedPane {
            host_id: entry.status.host_id.clone(),
            control_socket: entry.path,
            // A survivor has no screen to bring with it — the server that held
            // its scrollback is what died. Name it after the command instead, and
            // let the ring buffer supply whatever it wrote while orphaned.
            title: Some(entry.status.command.clone()),
            contents: None,
            borderless: false,
        },
    ))
}

pub fn kill_all() -> Result<()> {
    let entries = collect()?;
    if entries.is_empty() {
        println!("No pane supervisors are running.");
        return Ok(());
    }
    let mut killed = 0;
    for entry in &entries {
        match client::terminate_via_socket(&entry.path) {
            Ok(_) => killed += 1,
            Err(e) => {
                log::warn!("failed to terminate {}: {e:#}", entry.status.host_id);
                eprintln!("Could not terminate {}: {e:#}", entry.status.host_id);
            },
        }
    }
    println!("Terminated {killed} pane supervisor(s).");
    Ok(())
}

#[cfg(test)]
#[path = "../unit/pty_host_cli_tests.rs"]
mod pty_host_cli_tests;
