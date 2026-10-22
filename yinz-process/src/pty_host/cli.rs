//! `yinz pty-host list|kill` — the window into supervisors that outlived their
//! session.
//!
//! Not a nicety. A server that is `kill -9`'d (or OOM-killed) deliberately leaves
//! its supervisors running. Only two things reclaim them: these commands, and
//! `yinz kill-session <name>`, which sweeps the unattached supervisors labelled
//! with the session it kills (`terminate_orphans_of_session`). Without those the
//! only evidence a pane survived is a stray process in `ps`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use yinz_types::internal_ids::{SessionName, SupervisorId};
use yinz_types::units::UnixSeconds;

use yinz_runtime::errors::prelude::*;
use yinz_types::input::command::RunCommand;
use {yinz_types::data::RemotePaneHandoff, yinz_types::data::WarpedPane};

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
    collect_from(&protocol::socket_dir())
}

/// The directory is a parameter so this is reachable from a test without
/// enumerating — and garbage collecting — the sockets of whatever sessions the
/// developer running the suite happens to have open.
fn collect_from(dir: &Path) -> Result<Vec<Entry>> {
    collect_from_checked(dir, false)
}

fn collect_from_checked(dir: &Path, strict: bool) -> Result<Vec<Entry>> {
    let read_dir = match std::fs::read_dir(dir) {
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
            Err(e) if strict => return Err(e),
            Ok(status) => entries.push(Entry { path, status }),
            Err(e) => {
                // Logged rather than printed: this also runs inside the server,
                // on behalf of a plugin, where a stray stderr write would land
                // in whatever pty the server happens to be holding.
                log::warn!("could not query {}: {e:#}", path.display());
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

/// Every live supervisor's status, for callers that want the data rather than
/// the table `list` prints.
pub fn statuses() -> Result<Vec<HostStatus>> {
    Ok(collect()?.into_iter().map(|entry| entry.status).collect())
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
                s.host_id.to_string(),
                s.session_name
                    .clone()
                    .unwrap_or_else(|| "-".to_owned().into())
                    .to_string(),
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
    let mut table = vec![headers.map(str::to_owned).to_vec()];
    table.extend(rows.into_iter().map(|row| row.to_vec()));
    print_table(&table);
    Ok(())
}

/// Print rows in aligned columns, the first row being the header.
///
/// Shared by the local and remote listings so the two read as one command with
/// two subjects rather than as two commands.
fn print_table(rows: &[Vec<String>]) {
    let columns = rows.first().map(|row| row.len()).unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                // Never pad the last column: it is free-form and padding it just
                // trails whitespace.
                if i + 1 == row.len() {
                    cell.clone()
                } else {
                    format!("{:width$}", cell, width = widths[i])
                }
            })
            .collect();
        println!("{}", line.join("  "));
    }
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

fn humanize_age(started_at_unix: UnixSeconds) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs = now.saturating_sub((started_at_unix).get());
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
        .filter(|e| e.status.host_id.as_str().starts_with(host_id))
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
    let payload = adopt_payload(host_id)?;
    let session = match crate::pane_context::target(session.map(Into::into))?
        .0
        .map(|name| name.into_string())
    {
        Some(session) => session,
        None => {
            return Err(anyhow!(
                "not inside a yinz session; say which one to adopt into with --session"
            ));
        },
    };
    Ok((session, payload))
}

/// The pane a host id names, ready to be adopted.
///
/// Split out of `adopt_request` for callers that already know which session the
/// pane is going to — a session adopting into itself has no `--session` to
/// resolve and no environment to read it from.
pub fn adopt_payload(host_id: &str) -> Result<WarpedPane> {
    let entry = resolve(host_id)?;
    if entry.status.attached {
        return Err(anyhow!(
            "{} is already attached to session '{}'; there is nothing to recover",
            entry.status.host_id,
            entry
                .status
                .session_name
                .as_ref()
                .map(|name| name.as_str())
                .unwrap_or("<unknown>")
        ));
    }
    Ok(WarpedPane {
        agent_theme: None,
        bracketed_paste: false,
        agent_input: None,
        held: None,
        group: None,
        host_id: entry.status.host_id.clone(),
        control_socket: entry.path,
        // A survivor has no screen to bring with it — the server that held its
        // scrollback is what died. Caption it with the command instead, and let
        // the ring buffer supply whatever it wrote while orphaned. A caption and
        // not a name: nobody chose it, so it must not be lent to the tab.
        title: None,
        caption: Some(entry.status.command.clone()),
        contents: None,
        borderless: false,
        // A local supervisor, by construction: this walks sockets in this
        // machine's runtime directory. Remote survivors are found by asking the
        // broker holding them — see `remote_adopt_payload`.
        remote: None,
        // Adopting an orphan is not a warp: there is no session on the other side to have
        // read the pane's record off, and a supervisor keeps none - it clears
        // `originating_plugin` on attach, so even the spawn path cannot re-derive one. The
        // process comes back; what it was reported to be doing does not, and the agent has to
        // say so again on its next hook.
        agent: None,
    })
}

/// `yinz pty-host list --host <machine>` — what a broker over there is holding.
///
/// A remote pane's supervisor is on the far machine, so it leaves nothing in
/// this one's runtime directory for `collect` to find. The broker's registry is
/// the only record, and asking it is the only way to see a pane that a failed
/// handoff left with no server.
pub fn remote_list(host: &str, as_json: bool) -> Result<()> {
    let entries = remote_entries(host)?;

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).context("failed to render JSON")?
        );
        return Ok(());
    }

    if entries.is_empty() {
        println!("No panes are running on {host}.");
        return Ok(());
    }

    let mut rows: Vec<Vec<String>> = vec![vec![
        "TOKEN".to_owned(),
        "STATE".to_owned(),
        "COMMAND".to_owned(),
    ]];
    rows.extend(entries.iter().map(|entry| {
        vec![
            entry.pane_token.clone(),
            match entry.unattached_secs {
                // The one that matters: a pane in this state is running with
                // nobody watching it, and is what `adopt --host` is for.
                Some(secs) => format!("unattached {}", human_duration(secs)),
                None if entry.attached => "attached".to_owned(),
                None => "unattached".to_owned(),
            },
            entry.command.clone(),
        ]
    }));
    print_table(&rows);
    Ok(())
}

/// Roughly how long, in as few characters as say it.
fn human_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

/// The panes a host's broker is holding.
///
/// Builds a `RemoteHosts` of its own rather than borrowing a session's: this
/// runs as a plain CLI command, and the whole point is to work when the session
/// that had the pane is gone.
fn remote_entries(host: &str) -> Result<Vec<super::remote::protocol::RemotePaneEntry>> {
    use super::remote::ssh::RemoteHosts;
    use std::sync::Arc;

    Arc::new(RemoteHosts::new())
        .list_panes(host)
        .with_context(|| format!("failed to ask {host} what panes it is holding"))
}

/// `yinz pty-host adopt --host <machine> <token>` — recover a stranded remote
/// pane.
///
/// The remote twin of [`adopt_request`], and resolved here for the same reason:
/// a token that names nothing, or a pane someone else is already watching,
/// should fail before a session is involved.
pub fn remote_adopt_request(
    host: &str,
    pane_token: &str,
    session: Option<String>,
) -> Result<(String, WarpedPane)> {
    let payload = remote_adopt_payload(host, pane_token)?;
    let session = match crate::pane_context::target(session.map(Into::into))?
        .0
        .map(|name| name.into_string())
    {
        Some(session) => session,
        None => {
            return Err(anyhow!(
                "not inside a yinz session; say which one to adopt into with --session"
            ));
        },
    };
    Ok((session, payload))
}

/// The remote pane a token names, ready to be adopted.
pub fn remote_adopt_payload(host: &str, pane_token: &str) -> Result<WarpedPane> {
    let entry = remote_entries(host)?
        .into_iter()
        .find(|entry| entry.pane_token == pane_token)
        .ok_or_else(|| {
            anyhow!(
                "{host} is not holding a pane called '{pane_token}'. \
                 `yinz pty-host list --host {host}` says what it does have"
            )
        })?;
    warped_pane_from_entry(host, entry)
}

/// Turn what a broker said about a pane into a pane a session can adopt.
///
/// Split from the lookup so the decision has no broker in it: everything here
/// is a judgement about one entry, and the only way to reach it otherwise is to
/// have a machine on the far end of an ssh.
fn warped_pane_from_entry(
    host: &str,
    entry: super::remote::protocol::RemotePaneEntry,
) -> Result<WarpedPane> {
    let pane_token = entry.pane_token.clone();
    if entry.attached {
        return Err(anyhow!(
            "{pane_token} already has a session attached to it on {host}; there is nothing to \
             recover"
        ));
    }
    Ok(WarpedPane {
        agent_theme: None,
        bracketed_paste: false,
        agent_input: None,
        held: None,
        group: None,
        // No local supervisor to name. See `WarpedPane::remote`.
        host_id: SupervisorId::default(),
        control_socket: PathBuf::new(),
        // A survivor brings no screen with it — the server that held its
        // scrollback is what died, or never took delivery. Caption it with the
        // command, and let the broker's buffer supply what it wrote meanwhile.
        title: None,
        caption: Some(entry.command.clone()),
        contents: None,
        borderless: false,
        remote: Some(RemotePaneHandoff {
            host: host.to_owned(),
            pane_token: entry.pane_token,
            // From the beginning of what the broker still has: this side has
            // taken delivery of nothing. `replay_from` clamps to the ring, and
            // reports whatever it could not keep as dropped.
            received: 0,
            // Reconstructed from what the broker reports rather than carried:
            // the session that knew the full `RunCommand` is the one that is
            // gone. The host is the part that has to be right, because it is
            // what a later re-run reads.
            cmd: RunCommand {
                command: PathBuf::from(entry.command.split(' ').next().unwrap_or_default()),
                args: entry
                    .command
                    .split(' ')
                    .skip(1)
                    .map(str::to_owned)
                    .collect(),
                target: yinz_types::command::PaneTarget::Ssh {
                    host: host.to_owned(),
                },
                ..Default::default()
            },
        }),
        // Adopting an orphan is not a warp: there is no session on the other side to have
        // read the pane's record off, and a supervisor keeps none - it clears
        // `originating_plugin` on attach, so even the spawn path cannot re-derive one. The
        // process comes back; what it was reported to be doing does not, and the agent has to
        // say so again on its next hook.
        agent: None,
    })
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

/// Relabel this server's supervisors and the old session's orphans as one batch.
/// The backend holds its ownership lock until it publishes the new session name.
pub fn relabel_session_in(
    dir: &Path,
    owned: &[PathBuf],
    old: &SessionName,
    new: &SessionName,
) -> Result<()> {
    let mut entries = collect_from_checked(dir, true)?;
    // Owned supervisors need not live in the discovery directory. Missing or
    // unqueryable owned sockets must fail preflight rather than skip a pane.
    for path in owned {
        if !entries.iter().any(|entry| &entry.path == path) {
            entries.push(Entry {
                path: path.clone(),
                status: client::query(path)?,
            });
        }
    }
    entries.retain(|entry| {
        owned.contains(&entry.path)
            || (!entry.status.attached && entry.status.session_name.as_ref() == Some(old))
    });
    for entry in &entries {
        if !entry.status.supports_relabel {
            bail!(
                "supervisor {} does not support session relabeling",
                entry.status.host_id
            );
        }
        if entry.status.session_name.as_ref() != Some(old) {
            bail!(
                "supervisor {} no longer belongs to {old}",
                entry.status.host_id
            );
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        if let Err(error) =
            client::relabel_via_socket(&entry.path, old, new, !owned.contains(&entry.path))
        {
            let mut message = format!(
                "failed to relabel supervisor {}: {error:#}",
                entry.status.host_id
            );
            for changed in entries[..index].iter().rev() {
                if let Err(rollback) = client::relabel_via_socket(
                    &changed.path,
                    new,
                    old,
                    !owned.contains(&changed.path),
                ) {
                    message.push_str(&format!(
                        "; rollback failed for supervisor {}: {rollback:#}",
                        changed.status.host_id
                    ));
                }
            }
            bail!(message);
        }
    }
    Ok(())
}

/// Terminate every unattached supervisor that labels itself with `session`.
///
/// This is what `kill-session` reaches for after its own panes are closed: a
/// server that died without shutting down left its supervisors detached, and
/// killing the session by name is the user saying those panes are done too. The
/// `attached` check is the whole safety of it — a supervisor another live server
/// holds is never touched, whatever it is called.
pub fn terminate_orphans_of_session(session: &str) -> Result<Vec<HostStatus>> {
    terminate_orphans_of_session_in(&protocol::socket_dir(), session)
}

pub fn terminate_orphans_of_session_with_progress(
    session: &str,
    progress: &mut dyn FnMut(),
) -> Result<Vec<HostStatus>> {
    terminate_orphans_of_session_in_with_progress(&protocol::socket_dir(), session, progress)
}

pub(crate) fn terminate_orphans_of_session_in(
    dir: &Path,
    session: &str,
) -> Result<Vec<HostStatus>> {
    terminate_orphans_of_session_in_with_progress(dir, session, &mut || {})
}

fn terminate_orphans_of_session_in_with_progress(
    dir: &Path,
    session: &str,
    progress: &mut dyn FnMut(),
) -> Result<Vec<HostStatus>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start orphan cleanup runtime")?;
    runtime.block_on(async {
        use futures_util::{stream, StreamExt};
        let timeout = std::time::Duration::from_secs(5);
        let mut terminated = Vec::new();
        let mut errors = Vec::new();
        let mut paths = Vec::new();
        progress();
        let discover = async {
            let mut dir = match tokio::fs::read_dir(dir).await {
                Ok(dir) => dir,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e).context("failed to discover supervisors"),
            };
            while let Some(entry) = dir
                .next_entry()
                .await
                .context("incomplete supervisor discovery")?
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("sock") {
                    paths.push(path);
                }
            }
            Ok::<(), anyhow::Error>(())
        };
        match tokio::time::timeout(timeout, discover).await {
            Ok(Ok(())) => {},
            Ok(Err(e)) => errors.push(format!("{e:#}")),
            Err(_) => errors.push(format!(
                "five-second supervisor discovery deadline exceeded for {}",
                dir.display()
            )),
        }
        progress();
        let mut pending = stream::iter(paths.into_iter().map(|path| async move {
            // Queued supervisors get their full query-and-termination budget when polled.
            tokio::time::timeout(timeout, sweep_socket(&path, session))
                .await
                .context("five-second supervisor deadline exceeded")
                .and_then(|result| result)
                .with_context(|| format!("unresolved supervisor {}", path.display()))
        }))
        .buffer_unordered(32);
        while let Some(result) = pending.next().await {
            match result {
                Ok(Some(status)) => terminated.push(status),
                Ok(None) => {},
                Err(e) => errors.push(format!("{e:#}")),
            }
            progress();
        }
        if !errors.is_empty() {
            bail!("incomplete orphan cleanup: {}", errors.join("; "));
        }
        Ok(terminated)
    })
}

async fn supervisor_request(
    path: &Path,
    request: &protocol::ServerToHostMsg,
) -> Result<protocol::HostToServerMsg> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::UnixStream::connect(path)
        .await
        .context("failed to connect")?;
    let mut request_bytes = Vec::new();
    protocol::write_msg(&mut request_bytes, request)?;
    stream
        .write_all(&request_bytes)
        .await
        .context("failed to send supervisor request")?;
    let len = stream
        .read_u32_le()
        .await
        .context("supervisor did not confirm operation (EOF or unsupported protocol)")?;
    if len > protocol::MAX_FRAME_LEN {
        bail!("oversized supervisor reply");
    }
    let mut body = vec![0; len as usize];
    stream
        .read_exact(&mut body)
        .await
        .context("incomplete supervisor reply")?;
    serde_json::from_slice(&body).context("unsupported supervisor reply")
}

async fn sweep_socket(path: &Path, session: &str) -> Result<Option<HostStatus>> {
    use protocol::{HostToServerMsg, ServerToHostMsg};
    let HostToServerMsg::Status(status) = supervisor_request(path, &ServerToHostMsg::Query).await?
    else {
        bail!("supervisor did not answer discovery query");
    };
    if status.attached || status.session_name.as_ref().map(SessionName::as_str) != Some(session) {
        return Ok(None);
    }
    match supervisor_request(
        path,
        &ServerToHostMsg::TerminateDetached {
            expected_session: session.to_owned().into(),
        },
    )
    .await?
    {
        HostToServerMsg::TerminationComplete {
            error: Some(error), ..
        } => bail!(error),
        HostToServerMsg::TerminationComplete {
            terminated,
            error: None,
        } => Ok(terminated.then_some(status)),
        _ => bail!("supervisor cannot confirm conditional termination; cleanup incomplete"),
    }
}

#[cfg(test)]
#[path = "../unit/pty_host_cli_tests.rs"]
mod pty_host_cli_tests;
