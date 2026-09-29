//! Direct agent control requests. The server owns target checks, submission and observation.
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use yinz_cli::args::{Agent, CliArgs};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse};
use yinz_types::agent::AgentSpawnSpec;

pub(crate) fn run(command: Agent, opts: &CliArgs) -> Result<i32> {
    let (request, json, timeout) = request(command, &mut std::io::stdin())?;
    let explicit = opts.session.clone().or_else(|| {
        yinz_runtime::envs::get_session_name()
            .ok()
            .map(|name| name.into_string())
    });
    let session = match explicit {
        Some(session) => session,
        None => crate::commands::resolve_firecracker_session(
            None,
            None,
            yinz_cli::sessions::get_active_session(),
        )?,
    };
    yinz_session::sessions::validate_session_name(&session).map_err(|error| anyhow!(error))?;
    let response = exchange(&session, request, timeout)?;
    print_response(&response, json, &mut std::io::stdout().lock())?;
    Ok(
        if matches!(
            response.outcome,
            AgentOutcome::Ok | AgentOutcome::Submitted | AgentOutcome::Completed
        ) {
            0
        } else {
            1
        },
    )
}

fn request(command: Agent, stdin: &mut impl Read) -> Result<(AgentRequest, bool, Duration)> {
    let mut timeout = Duration::from_secs(30);
    let (request, json) = match command {
        Agent::List { json } => (AgentRequest::List, json),
        Agent::Status {
            target,
            conversation,
            json,
        } => (
            AgentRequest::Status {
                pane_id: target,
                expected_session_id: conversation,
            },
            json,
        ),
        Agent::Spawn {
            kind,
            cwd,
            branch,
            base,
            worktree_dir,
            model,
            effort,
            permission_mode,
            resume,
            reuse,
            allow_outside_worktree,
            json,
        } => {
            let cwd = cwd
                .canonicalize()
                .context("cannot resolve agent working directory")?;
            if !cwd.is_dir() {
                bail!(
                    "agent working directory is not a directory: {}",
                    cwd.display()
                );
            }
            let spec = AgentSpawnSpec {
                kind: Some(kind),
                branch,
                base,
                worktree_dir,
                model,
                effort,
                permission_mode,
                resume,
                reuse: Some(reuse),
                allow_outside_worktree: Some(allow_outside_worktree),
                ..Default::default()
            };
            yinz_runtime::agent::launch::plan(&spec).map_err(|error| anyhow!(error))?;
            timeout = Duration::from_secs(120);
            (AgentRequest::Spawn { spec, cwd }, json)
        },
        Agent::Prompt {
            target,
            text,
            file,
            conversation,
            wait,
            timeout: requested,
            json,
        } => {
            timeout = requested.unwrap_or(Duration::from_secs(if wait { 600 } else { 30 }));
            let prompt = read_prompt(text, file.as_deref(), stdin)?;
            (
                AgentRequest::Prompt {
                    pane_id: target,
                    prompt,
                    wait,
                    timeout_ms: timeout_millis(timeout)?,
                    expected_session_id: conversation,
                },
                json,
            )
        },
        Agent::Wait {
            target,
            conversation,
            until,
            timeout: requested,
            json,
        } => {
            timeout = requested;
            (
                AgentRequest::Wait {
                    pane_id: target,
                    until,
                    timeout_ms: timeout_millis(timeout)?,
                    expected_session_id: conversation,
                },
                json,
            )
        },
        Agent::Read {
            target,
            conversation,
            lines,
            json,
        } => (
            AgentRequest::Read {
                pane_id: target,
                lines: lines as usize,
                expected_session_id: conversation,
            },
            json,
        ),
        Agent::SendKeys {
            target,
            keys,
            conversation,
            json,
        } => (
            AgentRequest::SendKeys {
                pane_id: target,
                keys: keys
                    .iter()
                    .map(|key| {
                        key.parse()
                            .map_err(|error| anyhow!("invalid key {key:?}: {error}"))
                    })
                    .collect::<Result<_>>()?,
                expected_session_id: conversation,
            },
            json,
        ),
    };
    Ok((request, json, timeout))
}

fn timeout_millis(timeout: Duration) -> Result<u64> {
    let millis = timeout
        .as_millis()
        .try_into()
        .context("agent timeout is too large")?;
    if !(1..=86_400_000).contains(&millis) {
        bail!("agent timeout must be between 1ms and 24h");
    }
    Ok(millis)
}

fn read_prompt(text: Option<String>, file: Option<&Path>, stdin: &mut impl Read) -> Result<String> {
    let prompt = match (text, file) {
        (Some(text), None) => text,
        (None, Some(file)) if file == Path::new("-") => {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .context("cannot read prompt from standard input")?;
            text
        },
        (None, Some(file)) => std::fs::read_to_string(file)
            .with_context(|| format!("cannot read prompt file {}", file.display()))?,
        _ => bail!("provide prompt text or --file PATH"),
    };
    let prompt = prompt
        .replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_owned();
    if prompt.trim().is_empty() {
        bail!("prompt must not be empty");
    }
    if prompt.len() > 1024 * 1024 {
        bail!("prompt must be at most 1 MiB");
    }
    if prompt
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        bail!("prompt contains a terminal control character; use send-keys for deliberate terminal input");
    }
    Ok(prompt)
}

fn exchange(session: &str, request: AgentRequest, timeout: Duration) -> Result<AgentResponse> {
    let socket = yinz_ipc::consts::ipc_connect(&yinz_runtime::consts::YINZ_SOCK_DIR.join(session))
        .with_context(|| format!("cannot connect to session {session}"))?;
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender
        .get_receiver::<ServerToClientMsg>()
        .context("cannot set up agent reply")?;
    // The server's deadline determines the outcome. Allow its timeout result to arrive.
    let deadline = Instant::now()
        .checked_add(timeout)
        .and_then(|deadline| deadline.checked_add(Duration::from_secs(2)))
        .context("agent timeout is too large")?;
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(request))
        .context("cannot send agent request")?;
    let result = (|| loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("session did not answer the agent request before the deadline; the request was not retried");
        }
        receiver
            .set_recv_timeout(Some(remaining))
            .context("cannot bound agent reply wait")?;
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => return Ok(response),
            IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => bail!("{}", lines.join("\n")),
            IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => bail!("session exited before answering the agent request: {exit_reason:?}"),
            IpcRecv::Eof => bail!("session disconnected before answering the agent request; the request was not retried"),
            IpcRecv::Malformed => bail!("invalid agent reply from session"),
            IpcRecv::Idle => bail!("session did not answer the agent request before the deadline; the request was not retried"),
            _ => {},
        }
    })();
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .context("disconnecting agent client")
        .non_fatal();
    result
}

fn print_response(response: &AgentResponse, json: bool, out: &mut impl Write) -> Result<()> {
    if json {
        let mut value = serde_json::to_value(response).context("cannot encode agent response")?;
        if let Some(spawned) = &response.spawned {
            value["spawned"]["pane_id"] = spawned.pane_id.to_string().into();
        }
        serde_json::to_writer(&mut *out, &value).context("cannot write agent response")?;
        writeln!(out).context("cannot finish agent response")?;
        return Ok(());
    }
    if let Some(output) = &response.output {
        write!(out, "{output}").context("cannot write agent output")?;
        if !output.is_empty() && !output.ends_with('\n') {
            writeln!(out).context("cannot finish agent output")?;
        }
        if response.outcome == AgentOutcome::Ok {
            return Ok(());
        }
    }
    if let Some(spawned) = &response.spawned {
        writeln!(
            out,
            "{}\t{}\t{}",
            spawned.pane_id,
            spawned.branch.as_deref().unwrap_or("-"),
            spawned
                .cwd
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".into())
        )
        .context("cannot write spawned agent")?;
    }
    for status in &response.agents {
        writeln!(
            out,
            "{}  {}  {}  {}  {}",
            status.pane_id,
            status.agent.kind,
            status.agent.state.to_string().to_lowercase(),
            status.agent.source.to_string().to_lowercase(),
            status.session
        )
        .context("cannot write agent status")?;
        writeln!(
            out,
            "  ready: {}  status_available: {}  prompt_supported: {}",
            status.ready, status.status_available, status.prompt_supported
        )
        .context("cannot write agent readiness")?;
        for (label, value) in [
            ("cwd", status.cwd.as_deref()),
            ("branch", status.branch.as_deref()),
            ("conversation", status.agent.session_id.as_deref()),
            ("run", status.agent.run_id.as_deref()),
            ("waiting", status.agent.waiting_on.as_deref()),
        ] {
            if let Some(value) = value {
                writeln!(out, "  {label}: {value}").context("cannot write agent details")?;
            }
        }
        if status.agent.state == yinz_types::agent::AgentState::Waiting {
            writeln!(out, "  needs_person: {}", status.agent.wait_needs_a_person)
                .context("cannot write agent waiting reason")?;
        }
    }
    if response.outcome != AgentOutcome::Ok {
        let outcome =
            serde_json::to_value(response.outcome).context("cannot encode agent outcome")?;
        writeln!(
            out,
            "{} (acknowledged: {})",
            outcome.as_str().unwrap_or("unknown"),
            response.acknowledged
        )
        .context("cannot write agent outcome")?;
    }
    if let Some(message) = &response.message {
        writeln!(out, "{message}").context("cannot write agent message")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/agent.rs"]
mod tests;
