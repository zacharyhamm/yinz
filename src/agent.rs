//! Direct agent control requests. The server owns target checks, submission and observation.
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use yinz_cli::args::{Agent, CliArgs};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{
    AgentOutcome, AgentRequest, AgentResponse, MAX_PROMPT_BYTES, MAX_TIMEOUT_MS,
};
use yinz_types::agent::AgentSpawnSpec;

pub(crate) fn run(mut command: Agent, opts: &CliArgs) -> Result<i32> {
    match &command {
        Agent::Claude { command, args } => return launch_with_theme("claude", command, args),
        Agent::CodexUntracked { command, args } => {
            return launch_with_theme("codex", command, args)
        },
        Agent::Codex { command, args } => return yinz_codex::launch(command, args),
        Agent::Muse { command, args } => return yinz_muse::launch(command, args),
        Agent::CodexReport => {
            yinz_codex::report(&std::env::current_exe().context("find this yinz binary")?)?;
            return Ok(0);
        },
        Agent::MuseReport => {
            yinz_muse::report(&std::env::current_exe().context("find this yinz binary")?)?;
            return Ok(0);
        },
        _ => {},
    }
    if let Agent::Watch { filters, json } = command {
        crate::commands::stream_session_events(
            yinz_cli::args::EventsCli {
                event_type: vec!["agent_lifecycle".into()],
                filters,
                text: !json,
            },
            opts.session.clone(),
            None,
        );
        return Ok(0);
    }
    let json = command.json();
    let submission_id = submission_id(&mut command);
    match run_inner(command, opts) {
        Ok(code) => Ok(code),
        Err(error) if json => {
            let (outcome, error) = match error.downcast::<RequestFailed>() {
                Ok(RequestFailed(error)) => (AgentOutcome::Failed, error),
                Err(error) => (AgentOutcome::Rejected, error),
            };
            let mut response = AgentResponse::new(outcome);
            if outcome == AgentOutcome::Failed {
                response.submission_id = submission_id;
            }
            response.message = Some(format!("{error:#}"));
            print_response(&response, true, &mut std::io::stdout().lock())?;
            Ok(1)
        },
        Err(error) => Err(match error.downcast::<RequestFailed>() {
            Ok(RequestFailed(error)) => match submission_id {
                Some(id) => error.context(format!(
                    "submission {id}; outcome may be unknown, inspect the pane and retrieve this submission before retrying"
                )),
                None => error,
            },
            Err(error) => error,
        }),
    }
}

/// Establish an identity before any IPC so a lost response does not lose the submission.
fn submission_id(command: &mut Agent) -> Option<String> {
    match command {
        Agent::Prompt { submission, .. } => Some(
            submission
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone(),
        ),
        Agent::Wait { submission, .. } => submission.clone(),
        _ => None,
    }
}

/// An error after the request left for the session, as opposed to one found before sending.
/// With `--json`, the first is `failed` and the second `rejected`.
#[derive(Debug)]
struct RequestFailed(anyhow::Error);

impl std::fmt::Display for RequestFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for RequestFailed {}

fn run_inner(command: Agent, opts: &CliArgs) -> Result<i32> {
    let (explicit, caller) =
        yinz_process::pane_context::target(opts.session.clone().map(Into::into))?;
    let explicit = explicit.map(|session| session.into_string());
    let session = match explicit {
        Some(session) => session,
        None => crate::commands::resolve_session(yinz_cli::sessions::get_active_session())
            .map_err(|error| {
                if yinz_cli::sessions::sessions_denied() {
                    anyhow!(yinz_cli::sessions::no_active_session_message())
                } else {
                    error
                }
            })?,
    };
    yinz_session::sessions::validate_session_name(&session).map_err(|error| anyhow!(error))?;
    let action = match &command {
        Agent::Archive(archive) => Some(archive.action().map_err(anyhow::Error::msg)?),
        Agent::Register { pane_id, kind, .. } => Some(yinz_cli::notes_todos::pipe_action(
            "agent_action",
            serde_json::json!({"op":"agent_register","args":{"pane_id":pane_id,"kind":kind},"structured_reply":true}),
        )),
        _ => None,
    };
    if let Some(action) = action {
        let reply = yinz_client::store_client::request(&session.into(), action, caller)
            .map_err(|error| anyhow!(RequestFailed(error)))?;
        allow_closed_stdout(
            writeln!(std::io::stdout().lock(), "{reply}").context("cannot write agent reply"),
        )?;
        return Ok(0);
    }
    let (request, json, timeout) = request(command, &mut std::io::stdin())?;
    let response =
        exchange(&session, request, timeout).map_err(|error| anyhow!(RequestFailed(error)))?;
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

fn request(mut command: Agent, stdin: &mut impl Read) -> Result<(AgentRequest, bool, Duration)> {
    submission_id(&mut command);
    let mut timeout = Duration::from_secs(30);
    let (request, json) = match command {
        Agent::List { project, json } => (AgentRequest::List { project }, json),
        Agent::Claude { .. }
        | Agent::CodexUntracked { .. }
        | Agent::Codex { .. }
        | Agent::CodexReport
        | Agent::Muse { .. }
        | Agent::MuseReport
        | Agent::Watch { .. }
        | Agent::Archive(_)
        | Agent::Register { .. } => {
            bail!("archive and registration use background services")
        },
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
            tab,
            new_tab,
            kind,
            file,
            project,
            remote_host,

            worktree,
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
            bubblewrap,
            allow_yinz,
            json,
        } => {
            let mut input = if let Some(file) = file {
                let mut source = String::new();
                if file == Path::new("-") {
                    stdin
                        .read_to_string(&mut source)
                        .context("read spawn configuration")?;
                } else {
                    source = std::fs::read_to_string(file).context("read spawn configuration")?;
                }
                serde_json::from_str::<serde_json::Value>(&source)
                    .context("parse spawn configuration")?
            } else {
                serde_json::json!({})
            };
            let fields = input
                .as_object_mut()
                .context("spawn configuration must be an object")?;
            if fields.contains_key("prompt") || fields.contains_key("reuse_prompt") {
                bail!("spawn does not accept prompt or reuse_prompt; spawn, wait, then prompt");
            }
            let overrides = serde_json::json!({"kind":kind,"project":project,"remote_host":remote_host,
                "worktree":worktree,
                "cwd":cwd,"branch":branch,"base":base,"worktree_dir":worktree_dir,"model":model,
                "effort":effort,"permission_mode":permission_mode,"resume":resume});
            fields.extend(
                overrides
                    .as_object()
                    .context("spawn overrides")?
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            if bubblewrap {
                fields.insert("bubblewrap".into(), true.into());
            }
            if reuse {
                fields.insert("reuse".into(), true.into());
            }
            if allow_outside_worktree {
                fields.insert("allow_outside_worktree".into(), true.into());
            }
            if tab.is_some() || new_tab.is_some() {
                fields.remove("tab");
                fields.remove("new_tab");
                if let Some(tab) = tab {
                    fields.insert("tab".into(), tab.into());
                }
                if let Some(name) = new_tab {
                    fields.insert("new_tab".into(), name.map_or(true.into(), Into::into));
                }
            }
            let mut spec_fields = fields.clone();
            for field in ["tab", "new_tab", "project", "remote_host", "cwd"] {
                spec_fields.remove(field);
            }
            let placement = yinz_types::agent::control::AgentPlacement::from_launch_fields(
                &input["tab"],
                &input["new_tab"],
            )
            .map_err(|error| anyhow!(error))?;
            let mut spec: AgentSpawnSpec =
                serde_json::from_value(spec_fields.into()).context("parse launch options")?;
            if allow_yinz {
                allow_sandboxed_yinz(&mut spec)?;
            }
            let project: Option<String> =
                serde_json::from_value(input["project"].clone()).context("parse project")?;
            let remote_host: Option<String> =
                serde_json::from_value(input["remote_host"].clone()).context("parse SSH host")?;
            let mut cwd: Option<std::path::PathBuf> =
                serde_json::from_value(input["cwd"].clone()).context("parse cwd")?;
            if remote_host.is_none() {
                if let Some(path) = cwd {
                    cwd = Some(std::path::absolute(path).context("resolve cwd")?);
                } else if project.is_none() {
                    cwd = Some(std::env::current_dir().context("read cwd")?);
                }
            }
            timeout = Duration::from_secs(120);
            (
                AgentRequest::Spawn {
                    placement,

                    spec,
                    cwd,
                    project,
                    remote_host,
                },
                json,
            )
        },
        Agent::Prompt {
            target,
            text,
            file,
            conversation,
            submission,
            preserve_draft: _,
            wait,
            timeout: requested,
            json,
        } => {
            timeout = requested.unwrap_or(Duration::from_secs(if wait { 600 } else { 30 }));
            let prompt = read_prompt(text, file.as_deref(), stdin)?;
            if let Some(id) = &submission {
                yinz_types::agent::control::validate_submission_id(id)
                    .map_err(anyhow::Error::msg)?;
            }
            (
                AgentRequest::Prompt {
                    pane_id: target,
                    prompt,
                    wait,
                    timeout_ms: timeout_millis(timeout)?,
                    expected_session_id: conversation,
                    submission_id: submission,
                    preserve_draft: false,
                },
                json,
            )
        },
        Agent::Wait {
            target,
            conversation,
            submission,
            until,
            timeout: requested,
            json,
        } => {
            timeout = requested;
            let timeout_ms = timeout_millis(timeout)?;
            let request = if let Some(submission_id) = submission {
                yinz_types::agent::control::validate_submission_id(&submission_id)
                    .map_err(anyhow::Error::msg)?;
                AgentRequest::WaitTurn {
                    pane_id: target,
                    expected_session_id: conversation,
                    submission_id,
                    timeout_ms,
                }
            } else {
                AgentRequest::Wait {
                    pane_id: target,
                    until,
                    timeout_ms,
                    expected_session_id: conversation,
                }
            };
            (request, json)
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
        Agent::ClearDraft {
            target,
            conversation,
            input_revision,
            confirm_discard,
            interrupted,
            timeout: requested,
            json,
        } => {
            timeout = requested;
            (
                AgentRequest::ClearDraft {
                    pane_id: target,
                    expected_session_id: conversation,
                    input_revision,
                    confirm_discard,
                    interrupted,
                    timeout_ms: timeout_millis(timeout)?,
                },
                json,
            )
        },
        Agent::Recover {
            target,
            conversation,
            confirm_empty,
            interrupted,
            json,
        } => (
            AgentRequest::Recover {
                pane_id: target,
                expected_session_id: conversation,
                confirm_empty,
                interrupted,
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
    if !(1..=MAX_TIMEOUT_MS).contains(&millis) {
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
        (Some(_), Some(_)) => bail!("pass prompt TEXT or --file PATH, not both"),
        (None, None) => {
            bail!("provide prompt TEXT or --file PATH (use --file - for standard input)")
        },
    };
    let prompt = prompt
        .replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_owned();
    if prompt.trim().is_empty() {
        bail!("prompt must not be empty");
    }
    if prompt.len() > MAX_PROMPT_BYTES {
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

/// Codex's Linux sandbox blocks `connect()` on every socket, the session's included, unless
/// the sandbox has network access. Neither writable roots nor a Unix socket allowlist lifts it.
fn allow_sandboxed_yinz(spec: &mut AgentSpawnSpec) -> Result<()> {
    yinz_runtime::agent::launch::allow_sandboxed_yinz(spec).map_err(anyhow::Error::msg)
}

fn exchange(session: &str, request: AgentRequest, timeout: Duration) -> Result<AgentResponse> {
    let socket =
        match yinz_ipc::consts::ipc_connect(&yinz_runtime::consts::YINZ_SOCK_DIR.join(session)) {
            Ok(socket) => socket,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                bail!(yinz_cli::sessions::denied_session_message(session))
            },
            Err(error) => {
                return Err(error).with_context(|| format!("cannot connect to session {session}"))
            },
        };
    exchange_on_socket(socket, request, timeout)
}

fn exchange_on_socket(
    socket: interprocess::local_socket::Stream,
    request: AgentRequest,
    timeout: Duration,
) -> Result<AgentResponse> {
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender
        .get_receiver::<ServerToClientMsg>()
        .context("cannot set up agent reply")?;
    if let AgentRequest::Spawn {
        placement, spec, ..
    } = &request
    {
        for (required, probe, feature, token) in [
            (
                *placement != yinz_types::agent::control::AgentPlacement::Automatic,
                AgentRequest::SpawnPlacementSupport,
                "tab placement",
                "agent-tab-placement-v1",
            ),
            (
                spec.bubblewrap,
                AgentRequest::SpawnBubblewrapSupport,
                "Bubblewrap",
                "agent-bubblewrap-v1",
            ),
        ] {
            if !required {
                continue;
            }
            let guidance = format!("server did not confirm {feature} support; restart the session with this Yinz version (spawn was not sent)");
            sender
                .send_client_msg(ClientToServerMsg::AgentControl(probe))
                .context(guidance.clone())?;
            let response = receive_response(&mut receiver, Instant::now() + Duration::from_secs(5))
                .context(guidance.clone())?;
            if response.outcome != AgentOutcome::Ok || response.output.as_deref() != Some(token) {
                bail!(guidance);
            }
        }
    }
    // The server's deadline determines the outcome. Allow its timeout result to arrive.
    let deadline = Instant::now()
        .checked_add(timeout)
        .and_then(|deadline| deadline.checked_add(Duration::from_secs(2)))
        .context("agent timeout is too large")?;
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(request))
        .context("cannot send agent request")?;
    let result = receive_response(&mut receiver, deadline);
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .context("disconnecting agent client")
        .non_fatal();
    result
}

// The router can acknowledge a request before the screen thread answers it.
// Ignore those acknowledgements without extending the original deadline.
fn receive_response(
    receiver: &mut IpcReceiverWithContext<ServerToClientMsg>,
    deadline: Instant,
) -> Result<AgentResponse> {
    loop {
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
    }
}

fn print_response(response: &AgentResponse, json: bool, out: &mut impl Write) -> Result<()> {
    allow_closed_stdout(
        write_response(response, json, out)
            .and_then(|()| out.flush().context("cannot flush agent response")),
    )
}

pub(crate) fn allow_closed_stdout(result: Result<()>) -> Result<()> {
    match result {
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        },
        result => result,
    }
}

fn write_response(response: &AgentResponse, json: bool, out: &mut impl Write) -> Result<()> {
    if json {
        let mut value = serde_json::to_value(response).context("cannot encode agent response")?;
        if let Some(spawned) = &response.spawned {
            value["spawned"]["pane_id"] = spawned.pane_id.to_string().into();
            if !spawned.occupied_by.is_empty() {
                value["spawned"]["occupied_by"] = spawned
                    .occupied_by
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .into();
            }
        }
        writeln!(out, "{value}").context("cannot write agent response")?;
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
        if let Some(tab) = spawned.tab_id {
            writeln!(
                out,
                "Tab: {tab}{}",
                if spawned.created_tab_id == Some(tab) {
                    " (created)"
                } else {
                    ""
                }
            )
            .context("cannot write spawned tab")?;
        }
    }
    if let Some(resources) = &response.launch_resources {
        writeln!(
            out,
            "Launch resources: branch={} cwd={} reused={}",
            resources.branch.as_deref().unwrap_or("unknown"),
            resources
                .cwd
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".into()),
            resources.reused,
        )
        .context("cannot write partial launch resources")?;
        if let Some(tab) = resources.tab_id {
            writeln!(out, "Requested tab: {tab}").context("cannot write requested tab")?;
        }
        if let Some(tab) = resources.created_tab_id {
            writeln!(out, "Created tab retained or uncertain: {tab}")
                .context("cannot write retained tab")?;
        }
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
        writeln!(
            out,
            "  input_revision: {}  clear_draft_supported: {}",
            status
                .input_revision
                .map(|revision| revision.to_string())
                .unwrap_or_else(|| "unknown".into()),
            status.clear_draft_supported
        )
        .context("cannot write draft clearing status")?;
        if let Some(reason) = status.not_ready_reason {
            writeln!(out, "  not_ready_reason: {reason}")
                .context("cannot write readiness reason")?;
        }
        if let Some(readiness) = &status.readiness {
            let failures = readiness.failures();
            if !failures.is_empty() {
                writeln!(out, "  readiness_failures: {}", failures.join(", "))
                    .context("cannot write readiness failures")?;
            }
        }
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
        if !status.background_tasks.is_empty() {
            writeln!(
                out,
                "  background_tasks: {}",
                status.background_tasks.join(", ")
            )
            .context("cannot write background tasks")?;
        }
    }
    if let Some(id) = &response.submission_id {
        writeln!(out, "submission: {id}").context("cannot write submission identity")?;
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
    if let Some(status) = response
        .agents
        .first()
        .filter(|_| !response.background_tasks.is_empty())
    {
        writeln!(
            out,
            "Background work is still running ({}). `yinz agent wait {} --until idle` returns its final answer.",
            response.background_tasks.join(", "),
            status.pane_id
        )
        .context("cannot write background work guidance")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/agent.rs"]
mod tests;

fn launch_with_theme(kind: &str, program: &Path, args: &[String]) -> Result<i32> {
    yinz_process::agent_theme::launch(kind, program, args)
}

pub(crate) fn mother(command: yinz_cli::args::Mother, opts: &CliArgs) -> Result<i32> {
    use yinz_cli::args::Mother;
    use yinz_types::agent::mother::MotherRequest;
    let request = match command {
        Mother::Status => MotherRequest::Status,
        Mother::Overview => MotherRequest::Overview,
        Mother::Journal {
            after,
            through,
            limit,
        } => MotherRequest::Journal {
            after,
            through,
            limit,
        },
        Mother::Resolve {
            submission_id,
            expect_rev,
        } => MotherRequest::Resolve {
            submission_id,
            expect_rev,
        },
        Mother::Inbox => MotherRequest::Inbox,
        Mother::Ack { batch } => MotherRequest::Ack { batch },
        Mother::Pause => MotherRequest::Pause,
        Mother::Resume => MotherRequest::Resume,
        Mother::Checkpoint { expect_rev, file } => MotherRequest::Checkpoint {
            expect_rev,
            value: file
                .map(|path| -> Result<_> {
                    let bytes = std::fs::read(path).context("read Mother checkpoint input")?;
                    if bytes.len() > 1024 * 1024 {
                        bail!("checkpoint exceeds 1 MiB");
                    }
                    serde_json::from_slice(&bytes).context("parse Mother checkpoint input")
                })
                .transpose()?,
        },
        Mother::Configure { filters } => {
            if filters.cursor.is_some() {
                bail!("Mother owns its stream cursor; configure accepts filters only");
            }
            MotherRequest::Configure {
                panes: filters.pane.iter().map(ToString::to_string).collect(),
                projects: filters.project,
                kinds: filters.kind,
                lifecycle_types: filters.lifecycle_type,
            }
        },
    };
    let (explicit, _) = yinz_process::pane_context::target(opts.session.clone().map(Into::into))?;
    let session = match explicit {
        Some(session) => session.into_string(),
        None => crate::commands::resolve_session(yinz_cli::sessions::get_active_session())?,
    };
    let response = exchange(
        &session,
        AgentRequest::Mother(request),
        Duration::from_secs(30),
    )?;
    if let Some(output) = response.output {
        println!("{output}");
    } else {
        println!(
            "{}",
            serde_json::to_string(&response).context("encode Mother response")?
        );
    }
    Ok(if response.outcome == AgentOutcome::Ok {
        0
    } else {
        1
    })
}
