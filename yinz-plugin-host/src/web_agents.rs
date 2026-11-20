//! Native, bounded adapter over the terminal palette's live data and archive readers.
//! This runs on the counted, pause-aware AgentView worker owned by the plugin thread.
use super::{agent_control, palette_agents};
use palette_agents::history;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use yinz_bus::{
    command_palette::{self as palette, AgentAction, AgentReply, HistoryAction, HistoryReply},
    screen::ScreenInstruction,
    thread_bus::ThreadSenders,
    web::{MoveDestination, WebAgentRef, WebFocusError, WebLiveAgent, WebManageAction},
};
use yinz_storage::data_access::DataAccess;
use yinz_types::{
    agent::{
        control::{
            valid_web_identity, AgentOutcome, AgentRequest, AgentResponse, ProviderUsage,
            ProviderUsageStatus, WebAgentAction, WebTerminalInput, PEER_DETAIL_PREFIX,
        },
        history::{Cursor, Range},
        LiveAgentOrder,
    },
    internal_ids::ClientId,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    List {},
    LaunchChoices {
        project: Option<String>,
    },
    Launch {
        submission_id: String,
        kind: String,
        project: String,
        checkout: Checkout,
    },
    LaunchStatus {
        submission_id: String,
        agent: WebAgentRef,
    },
    Destinations {
        agent: WebAgentRef,
    },
    Move {
        agent: WebAgentRef,
        destination: MoveDestination,
    },
    Close {
        agent: WebAgentRef,
    },
    History {
        agent: Option<String>,
        run: String,
        from: Option<u64>,
        #[serde(default)]
        query: String,
        tz: Option<String>,
        worker: Option<String>,
        before: Option<Cursor>,
        after_revision: Option<u64>,
    },
    Hit {
        run: String,
        revision: u64,
        tz: Option<String>,
    },
    Event {
        run: String,
        revision: u64,
        tz: Option<String>,
    },
    Tools {
        run: String,
        revisions: Option<Vec<u64>>,
        range: Option<Range>,
        before: Option<Cursor>,
        tz: Option<String>,
    },
    Usage {
        agent: String,
    },
    Prompts {},
    Prompt {
        agent: WebAgentRef,
        submission_id: String,
        text: String,
    },
    Focus {
        agent: WebAgentRef,
    },
    Terminal {
        agent: WebAgentRef,
        after: Option<String>,
    },
    TerminalInput {
        agent: WebAgentRef,
        data: WebTerminalInput,
    },
    Screen {
        agent: WebAgentRef,
        #[serde(default = "screen_lines")]
        lines: usize,
    },
    Keys {
        agent: WebAgentRef,
        keys: Vec<String>,
    },
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Checkout {
    Existing { path: std::path::PathBuf },
    New { branch: String },
}

fn screen_lines() -> usize {
    80
}

#[derive(Debug)]
struct Error {
    code: &'static str,
    message: String,
}
type Result<T> = std::result::Result<T, Error>;
fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}
fn invalid(message: impl Into<String>) -> Error {
    error("invalid", message)
}
fn identifier(value: &str) -> Result<()> {
    if valid_web_identity(value) {
        Ok(())
    } else {
        Err(invalid(
            "identity must be nonblank and contain no control characters",
        ))
    }
}
fn revision(value: u64, zero: bool) -> Result<()> {
    // Leave room for the archive's next-revision watermark and signed SQLite predicates.
    if value < i64::MAX as u64 && (zero || value > 0) {
        Ok(())
    } else {
        Err(invalid("revision must fit the signed archive cursor range"))
    }
}
fn cursor(value: &Cursor) -> Result<()> {
    revision(value.revision, false)?;
    if value
        .created_at_ms
        .is_some_and(|at| !(-377705116800000..=253402300799999).contains(&at))
    {
        return Err(invalid("cursor timestamp is out of range"));
    }
    Ok(())
}
fn range(value: &Range) -> Result<()> {
    if let Some(start) = &value.start {
        cursor(start)?;
    }
    if let Some(end) = &value.end {
        cursor(end)?;
    }
    if value
        .start
        .zip(value.end)
        .is_some_and(|(start, end)| start >= end)
    {
        return Err(invalid("range endpoints must be ordered"));
    }
    if let Some(worker) = &value.worker {
        identifier(worker)?;
    }
    Ok(())
}
fn agent_ref(agent: &WebAgentRef) -> Result<()> {
    for value in [&agent.id, &agent.kind, &agent.session_id] {
        identifier(value)?;
    }
    Ok(())
}
impl Operation {
    fn parse(input: &str) -> Result<Self> {
        if input.len() > 64 * 1024 {
            return Err(invalid("request exceeds 64 KiB"));
        }
        let op: Self = serde_json::from_str(input).map_err(|e| invalid(e.to_string()))?;
        match &op {
            Self::History {
                agent,
                run,
                from,
                query,
                worker,
                before,
                after_revision,
                ..
            } => {
                identifier(run)?;
                if let Some(agent) = agent {
                    identifier(agent)?;
                }
                if let Some(worker) = worker {
                    identifier(worker)?;
                }
                if let Some(from) = from {
                    revision(*from, true)?;
                }
                if let Some(after) = after_revision {
                    revision(*after, true)?;
                }
                if let Some(before) = before {
                    cursor(before)?;
                }
                if (before.is_some() && after_revision.is_some())
                    || (from.is_some()
                        && (before.is_some() || after_revision.is_some() || worker.is_some()))
                    || (!query.is_empty()
                        && (before.is_some() || after_revision.is_some() || worker.is_some()))
                {
                    return Err(invalid(
                        "search, chronological and delivery cursors are mutually exclusive",
                    ));
                }
            },
            Self::Hit {
                run, revision: rev, ..
            }
            | Self::Event {
                run, revision: rev, ..
            } => {
                identifier(run)?;
                revision(*rev, false)?;
            },
            Self::Tools {
                run,
                revisions,
                range: scope,
                before,
                ..
            } => {
                identifier(run)?;
                match (revisions, scope) {
                    (Some(revisions), None)
                        if !revisions.is_empty() && revisions.len() <= 30 && before.is_none() =>
                    {
                        for rev in revisions {
                            revision(*rev, false)?;
                        }
                    },
                    (None, Some(scope)) => {
                        range(scope)?;
                        if let Some(before) = before {
                            cursor(before)?;
                        }
                    },
                    _ => return Err(invalid(
                        "tools requires either 1 to 30 revisions or a range with optional before",
                    )),
                }
            },
            Self::Usage { agent } => identifier(agent)?,
            Self::LaunchChoices { project } => {
                if let Some(project) = project {
                    identifier(project)?;
                }
            },
            Self::Launch {
                submission_id,
                kind,
                project,
                checkout,
            } => {
                yinz_types::agent::control::validate_submission_id(submission_id)
                    .map_err(invalid)?;
                identifier(project)?;
                identifier(kind)?;
                match checkout {
                    Checkout::Existing { path } if !path.is_absolute() => {
                        return Err(invalid("Choose an absolute checkout path"))
                    },
                    Checkout::New { branch } => identifier(branch)?,
                    _ => {},
                }
            },
            Self::LaunchStatus {
                submission_id,
                agent,
            } => {
                yinz_types::agent::control::validate_submission_id(submission_id)
                    .map_err(invalid)?;
                agent_ref(agent)?;
            },
            Self::Move { agent, destination } => {
                agent_ref(agent)?;
                let names = match destination {
                    MoveDestination::NewTab { name, .. } => [name.as_ref(), None],
                    MoveDestination::NewGroup { name, tab_name } => {
                        [name.as_ref(), tab_name.as_ref()]
                    },
                    MoveDestination::Tab { incarnation, .. } => {
                        identifier(incarnation)?;
                        [None, None]
                    },
                };
                for name in names.into_iter().flatten() {
                    if name.len() > 512 || name.chars().any(char::is_control) {
                        return Err(invalid("Invalid destination name"));
                    }
                }
            },
            Self::Destinations { agent } | Self::Close { agent } | Self::Focus { agent } => {
                agent_ref(agent)?
            },
            Self::Prompt {
                agent,
                submission_id,
                text,
            } => {
                agent_ref(agent)?;
                WebAgentAction::Prompt {
                    text: text.clone(),
                    submission_id: submission_id.clone(),
                }
                .validate()
                .map_err(invalid)?;
            },
            Self::Terminal { agent, after } => {
                agent_ref(agent)?;
                WebAgentAction::Terminal {
                    after: after.clone(),
                }
                .validate()
                .map_err(invalid)?;
            },
            Self::TerminalInput { agent, data } => {
                agent_ref(agent)?;
                WebAgentAction::TerminalInput { data: data.clone() }
                    .validate()
                    .map_err(invalid)?;
            },
            Self::Screen { agent, lines } => {
                agent_ref(agent)?;
                WebAgentAction::Read { lines: *lines }
                    .validate()
                    .map_err(invalid)?;
            },
            Self::Keys { agent, keys } => {
                agent_ref(agent)?;
                WebAgentAction::SendKeys { keys: keys.clone() }
                    .validate()
                    .map_err(invalid)?;
            },
            Self::List {} | Self::Prompts {} => {},
        }
        Ok(op)
    }
}

pub(crate) fn handle(
    input: &str,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Value {
    match Operation::parse(input).and_then(|op| execute(op, client, senders, data)) {
        Ok(value) => json!({"ok":value}),
        Err(e) => json!({"error":{"code":e.code,"message":e.message}}),
    }
}
fn snapshot(senders: &ThreadSenders) -> Result<Vec<WebLiveAgent>> {
    let (reply, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_screen(ScreenInstruction::WebLiveAgents { reply })
        .map_err(|e| error("failed", format!("Cannot request live agents: {e}")))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| error("unavailable", "Live agent snapshot unavailable"))
}
fn labels(
    live: &[WebLiveAgent],
    data: &DataAccess,
) -> Result<BTreeMap<String, palette::AgentLabels>> {
    palette_agents::compute_labels(live.iter().map(|a| a.label_source.clone()).collect(), data)
        .map_err(|e| error("failed", e))
}
fn order<'a>(a: &'a WebLiveAgent, label: Option<&'a palette::AgentLabels>) -> LiveAgentOrder<'a> {
    LiveAgentOrder::new(
        a.pane.agent.as_ref().map(|a| a.state),
        a.pane.last_hook_at_ms,
        label.map(|l| l.project.as_str()),
        label.map(|l| l.branch.as_str()),
        a.pane.id,
        a.session.as_str(),
    )
}
fn row(live: &WebLiveAgent, label: Option<&palette::AgentLabels>) -> Value {
    let agent = live.pane.agent.as_ref();
    let kind = agent.map(|a| a.kind.as_str()).unwrap_or_default();
    let session_id = agent
        .and_then(|a| a.session_id.as_deref())
        .unwrap_or_default();
    let detail_id = live.peer_detail_id();
    let identity = WebAgentRef {
        id: if live.current_session {
            live.id.clone()
        } else {
            detail_id.clone().unwrap_or_default()
        },
        kind: kind.into(),
        session_id: session_id.into(),
    };
    let mut row = json!({"id":live.id,"ref":identity,"session":live.session,"current_session":live.current_session,
        "pane_id":live.pane.id.to_string(),"pane":live.pane_label,"provider":live.provider,
        "state":live.metadata.get("state"),"glyph":live.glyph,"status":live.status,"host":live.host,
        "title":live.pane.title,"activity":live.activity,"project":label.map(|l| l.project.as_str()).unwrap_or_default(),
        "branch":label.map(|l| l.branch.as_str()).unwrap_or_default(),"last_reply":label.and_then(|l| l.last_reply.as_ref())
            .map(|r| json!({"text":r.text,"truncated":r.truncated})),
        "last_hook_at_ms":live.pane.last_hook_at_ms,"focused":live.focused,"selectable":live.selectable,
        "suppressed":live.suppressed,"directory":live.metadata.get("directory"),"run":live.run,
        "wait_needs_a_person":agent.is_some_and(|a| a.wait_needs_a_person),"source":live.metadata.get("source"),
        "can_prompt":live.matches(&identity),
        "prompt_unavailable":if !live.current_session && detail_id.is_none() {
            Some("Live controls are unavailable until the peer publishes its identity. Refresh Agents and reopen the card.")
        } else if !valid_web_identity(session_id) {
            Some("Agent has no conversation ID")
        } else {
            None
        },
        "usage_unavailable":live.usage_unavailable});
    if let Some(tab) = &live.local_tab {
        row["location"] = json!({"group":live.tab_group,"tab":{"id":tab.id,"name":tab.name}});
    }
    if let Some(id) = detail_id {
        row["detail_id"] = json!(id);
    }
    row
}

fn unique_live<T>(mut matches: impl Iterator<Item = T>) -> Option<T> {
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn detail_target(mut live: Vec<WebLiveAgent>, id: &str) -> Option<WebLiveAgent> {
    let scoped = id.starts_with(PEER_DETAIL_PREFIX);
    let index = unique_live(live.iter().enumerate().filter_map(|(index, row)| {
        let matched = if scoped {
            row.peer_detail_id().as_deref() == Some(id)
        } else {
            row.current_session && row.id == id
        };
        matched.then_some(index)
    }))?;
    Some(live.swap_remove(index))
}

fn target(senders: &ThreadSenders, agent: &WebAgentRef) -> Result<WebLiveAgent> {
    unique_live(snapshot(senders)?.into_iter().filter(|a| a.matches(agent)))
        .ok_or_else(|| error("target_changed", "Agent disappeared or changed identity"))
}

fn verify_peer_focus(live: &WebLiveAgent, agent: &WebAgentRef, deadline: Instant) -> Result<()> {
    let unavailable = || {
        error(
            "unavailable",
            "Peer Focus is unavailable. Refresh Agents and try again.",
        )
    };
    if live.tab_group_session != live.session {
        return Err(error(
            "unavailable",
            "Focus is temporarily unavailable while the peer publishes its renamed session. Refresh Agents and reopen the card.",
        ));
    }
    let expected_instance = live
        .peer_instance
        .clone()
        .ok_or_else(|| error("target_changed", "Peer identity unavailable"))?;
    let request = AgentRequest::WebPeerGuarded {
        expected_instance,
        pane_id: live.pane.id,
        expected_kind: agent.kind.clone(),
        expected_session_id: agent.session_id.clone(),
        action: WebAgentAction::VerifyFocus,
    };
    let response = palette_agents::control_peer(&live.tab_group_session, request, deadline)
        .map_err(|_| unavailable())?;
    match response.outcome {
        AgentOutcome::Ok if Instant::now() < deadline => Ok(()),
        AgentOutcome::TargetChanged => Err(error(
            "target_changed",
            "Agent disappeared or changed identity",
        )),
        _ => Err(unavailable()),
    }
}

fn control(
    agent: WebAgentRef,
    action: WebAgentAction,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<Value> {
    let live = target(senders, &agent)?;
    let submission_id = action.submission_id().map(str::to_owned);
    let mut response = if live.current_session {
        let request = AgentRequest::WebGuarded {
            pane_id: live.pane.id,
            expected_kind: agent.kind,
            expected_session_id: agent.session_id,
            action,
        };
        agent_control::control_agent(request, client, senders, data)
    } else {
        let expected_instance = live
            .peer_instance
            .ok_or_else(|| error("target_changed", "Peer identity unavailable"))?;
        let request = AgentRequest::WebPeerGuarded {
            expected_instance,
            pane_id: live.pane.id,
            expected_kind: agent.kind,
            expected_session_id: agent.session_id,
            action,
        };
        // Only a session obtained from this fresh snapshot can become a route.
        palette_agents::control_peer(
            &live.tab_group_session,
            request,
            Instant::now() + Duration::from_secs(32),
        )
        .unwrap_or_else(|message| {
            let mut response = AgentResponse::new(AgentOutcome::Failed);
            response.message = Some(message);
            response.submission_id = submission_id;
            response
        })
    };
    if let Some(output) = &mut response.output {
        output.retain(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'));
    }
    Ok(json!({"response":response}))
}
fn read(
    action: HistoryAction,
    tz: Option<&str>,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<HistoryReply> {
    let request = palette::AgentRequest {
        timezone: yinz_runtime::client_time::ClientTimeZone::from_client(tz),
        client,
        opening: 0,
        step: 0,
        source: Default::default(),
        action: AgentAction::History(action),
        values: vec![],
        active: Arc::new(AtomicBool::new(true)),
    };
    match palette_agents::execute(&request, senders, data).map_err(|e| error("failed", e))? {
        AgentReply::History(reply) => Ok(reply),
        _ => Err(error("failed", "Unexpected native history reply")),
    }
}
fn event(data: &DataAccess, run: &str, rev: u64) -> Result<HistoryReply> {
    history::read_event(data, run, rev).map_err(|e| match e {
        history::RetainedEventError::NotFound => error("not_found", e.to_string()),
        history::RetainedEventError::Failed(message) => error("failed", message),
    })
}

/// Navigation is a closed adapter, never serialized native actions (in particular Resume).
fn navigation(action: &HistoryAction) -> Option<Value> {
    Some(match action {
        HistoryAction::Run { id, from, query } => {
            json!({"op":"history","run":id,"from":from,"query":query})
        },
        HistoryAction::Conversation {
            id,
            worker,
            before,
            after_revision,
        } => {
            json!({"op":"history","run":id,"worker":worker,"before":before,"after_revision":after_revision})
        },
        HistoryAction::Hit { id, revision } => json!({"op":"hit","run":id,"revision":revision}),
        HistoryAction::Event { id, revision } => json!({"op":"event","run":id,"revision":revision}),
        HistoryAction::Tools { id, range, before } => {
            json!({"op":"tools","run":id,"range":range,"before":before})
        },
        _ => return None,
    })
}
fn adapt(
    reply: HistoryReply,
    run: &str,
    live: Option<&WebLiveAgent>,
    label: Option<&palette::AgentLabels>,
    retargeted: bool,
    requested_agent: Option<&str>,
    searching: bool,
) -> Result<Value> {
    match reply {
        HistoryReply::Reader {
            header,
            markdown,
            tool,
        } => Ok(json!({"header":header,"markdown":markdown,"tool":tool})),
        HistoryReply::Run {
            mut header,
            suppressed_result_revisions,
            activity_ranges,
            events,
            rows,
            status,
        } => {
            if let Some(live) = live {
                header.name.clone_from(&live.header.name);
                header.status.clone_from(&live.header.status);
                header.location.clone_from(&live.header.location);
                header.task.clone_from(&live.header.task);
                header.details = format!("{}\n{}", live.details, header.details);
                if let Some(label) = label {
                    header.project.clone_from(&label.project);
                    header.branch.clone_from(&label.branch);
                }
                if header.project.is_empty() {
                    header.project.clone_from(&live.header.project);
                }
                if let Some(session_id) = &live.header.session_id {
                    header.session_id.clone_from(session_id);
                }
            }
            let action = |id: &str| rows.iter().find(|r| r.id == id).map(|r| &r.action);
            let run_id = |id| match action(id) {
                Some(HistoryAction::Run { id, .. }) => Some(id),
                _ => None,
            };
            let next_from = match action("continue") {
                Some(HistoryAction::Run { from, .. }) => *from,
                _ => None,
            };
            let requests: BTreeMap<_, _> = rows
                .iter()
                .filter_map(|r| {
                    let mut request = navigation(&r.action)?;
                    // Keep current-run browsing live-bound even while its agent is gone.
                    // Lineage links deliberately inspect stored runs, including self-links.
                    if !matches!(r.id.as_str(), "previous" | "origin")
                        && matches!(&r.action,
                            HistoryAction::Run { id, .. } | HistoryAction::Conversation { id, .. }
                            if id == run)
                        && let Some(agent) = requested_agent
                    {
                        request["agent"] = json!(agent);
                    }
                    Some((r.id.clone(), request))
                })
                .collect();
            let events: Vec<_> = events.into_iter().map(|e| {
                let action = if searching { HistoryAction::Hit { id: run.into(), revision: e.revision } }
                    else if let Some(range) = &e.range { HistoryAction::Tools { id: run.into(), range: range.clone(), before: None } }
                    else { HistoryAction::Event { id: run.into(), revision: e.revision } };
                json!({"revision":e.revision,"revisions":if e.range.is_none() {vec![e.revision]} else {vec![]},
                    "range":e.range,"kind":e.kind,"created_at_ms":e.created_at_ms,"time":e.time,
                    "truncated":e.truncated,"tool_calls":e.tool_calls,"markdown":e.markdown,"request":navigation(&action)})
            }).collect();
            Ok(
                json!({"run":run,"retargeted":retargeted,"agent_gone":requested_agent.is_some() && live.is_none(),"live":live.map(|a| row(a,label)),
                "header":{"name":header.name,"status":header.status,"location":header.location,"started":header.started,
                    "task":header.task,"project":header.project,"branch":header.branch,"session_id":header.session_id,"details":header.details},
                "events":events,"earlier":action("earlier").is_some(),"next_from":next_from,
                "previous_run":run_id("previous"),"origin_run":run_id("origin"),"resumable":false,
                "suppressed_result_revisions":suppressed_result_revisions,
                "activity_ranges":activity_ranges,"navigation":requests,"status":status}),
            )
        },
        HistoryReply::Rows { .. } => Err(error("failed", "Unexpected history navigation")),
    }
}

fn web_error(e: WebFocusError) -> Error {
    match e {
        WebFocusError::TargetChanged => {
            error("target_changed", "Agent disappeared or changed identity")
        },
        WebFocusError::NoTerminal => error("unavailable", "No terminal attached"),
        WebFocusError::Unavailable(message) => error("unavailable", message),
        WebFocusError::Failed(message) => error("failed", message),
    }
}

fn manage(
    agent: WebAgentRef,
    action: WebManageAction,
    client: ClientId,
    senders: &ThreadSenders,
) -> Result<Value> {
    let (reply, rx) = crossbeam::channel::bounded(1);
    let deadline = Instant::now() + Duration::from_secs(5);
    senders
        .send_to_screen(ScreenInstruction::WebManageAgent {
            agent,
            action,
            client,
            deadline,
            reply,
        })
        .map_err(|e| error("failed", e.to_string()))?;
    match rx
        .recv_deadline(deadline)
        .map_err(|_| {
            error(
                "unavailable",
                "Request timed out. Refresh Agents to check the result.",
            )
        })?
        .map_err(web_error)?
    {
        Some(inventory) => Ok(json!(inventory)),
        None => Ok(json!({"ok":true})),
    }
}

fn launch(
    id: String,
    kind: String,
    project: String,
    checkout: Checkout,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<Value> {
    use super::{
        agent_launch::{prepare_agent_with_plan, OpeningPromptPolicy},
        launch_transaction::LaunchPreflight,
    };
    use yinz_runtime::agent::launch::OfferedProvider;
    use yinz_types::agent::{
        control::AgentPlacement,
        launch::{LaunchOutcome, LaunchRequest},
        AgentSpawnSpec,
    };

    let (path, branch) = match checkout {
        Checkout::Existing { path } => (Some(path), None),
        Checkout::New { branch } => (None, Some(branch)),
    };
    // Hash only the submitted intent. Project settings and available branches
    // can change while a browser is recovering the original receipt.
    let request = LaunchRequest {
        placement: AgentPlacement::Automatic,
        spec: AgentSpawnSpec {
            kind: Some(kind.clone()),
            branch: branch.clone(),
            ..Default::default()
        },
        project: Some(project.clone()),
        cwd: path.clone(),
        remote_host: None,
        new_project: None,
    };
    let project = palette::AgentProject::Registered(project);
    let resolve =
        || palette_agents::project_launch(&kind, &project, path.as_deref(), branch.clone(), data);
    let response = super::launch_transaction::launch_with_preflight(
        id,
        request,
        client,
        senders,
        data,
        OpeningPromptPolicy::Reject,
        None,
        Some(kind.clone()),
        |spec| {
            let provider = OfferedProvider::resolve(&kind).ok_or("Choose an offered provider")?;
            let (placeholder, _) = provider.plan(spec)?;
            let (_, cwd) = resolve()?;
            let original = CheckoutIdentity::open(&cwd)?;
            Ok(LaunchPreflight {
                placeholder: Some(placeholder),
                prepare: Some(Box::new(move |spec| {
                    let (current, cwd) = resolve()?;
                    original.verify(&cwd)?;
                    *spec = current;
                    spec.env
                        .insert(yinz_types::agent::AGENT_PANE_ENV.into(), "1".into());
                    prepare_agent_with_plan(
                        spec,
                        cwd,
                        None,
                        data,
                        &senders.session.current(),
                        |spec| provider.plan(spec),
                    )
                })),
            })
        },
    );
    let receipt = response
        .launch
        .ok_or_else(|| error("unavailable", "Launch receipt unavailable"))?;
    match receipt.outcome {
        LaunchOutcome::Started if response.outcome != AgentOutcome::Ok => Err(error(
            "launch_failed",
            "The agent started, but its original pane is no longer available.",
        )),
        LaunchOutcome::Started => {
            let spawned = receipt
                .spawned
                .ok_or_else(|| error("unavailable", "Launch receipt has no pane"))?;
            Ok(
                json!({"session":senders.session.current(), "pane_id":spawned.pane_id.to_string(), "kind":kind}),
            )
        },
        LaunchOutcome::Pending | LaunchOutcome::Unknown => Err(error(
            "unavailable",
            response
                .message
                .unwrap_or_else(|| "Launch outcome is pending. Check this launch again.".into()),
        )),
        outcome => {
            let code = match outcome {
                LaunchOutcome::Rejected => "invalid",
                LaunchOutcome::Conflict => "launch_conflict",
                _ => "launch_failed",
            };
            Err(error(
                code,
                response
                    .message
                    .unwrap_or_else(|| format!("Launch {outcome:?} ({:?})", receipt.reason)),
            ))
        },
    }
}

fn launch_status(
    id: &str,
    agent: &WebAgentRef,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<Value> {
    use yinz_types::agent::launch::{LaunchOutcome, WorkerDisposition};

    let instance = yinz_bus::agent_launch::server_instance();
    if !id.starts_with(&format!("{instance}.")) {
        return Err(error(
            "target_changed",
            "The server restarted after this launch.",
        ));
    }
    let live = target(senders, agent)?;
    let changed = || {
        error(
            "target_changed",
            "The original launched agent is no longer available.",
        )
    };
    let session = senders.session.current();
    let yinz_types::pane_id::PaneId::Terminal(terminal) = live.pane.id else {
        return Err(changed());
    };
    if !live.current_session
        || live.session != session
        || senders
            .launches
            .gate(terminal)
            .is_none_or(|gate| gate.launch_id != id)
    {
        return Err(changed());
    }
    // Observe after the live snapshot so a worker superseded during that read fails verification.
    let receipt = super::launch_transaction::status(id, senders, data)
        .launch
        .ok_or_else(changed)?;
    if receipt.server_instance != instance
        || receipt.launch_id != id
        || receipt.outcome != LaunchOutcome::Started
        || receipt.disposition != WorkerDisposition::Available
        || receipt
            .spawned
            .is_none_or(|spawned| spawned.pane_id != live.pane.id)
    {
        return Err(changed());
    }
    Ok(json!({"session":session, "pane_id":live.pane.id.to_string(), "agent":agent}))
}

// Keep the original Git directory open across reservation. Even if the entire
// project is replaced at the same pathname its inode cannot be recycled.
struct CheckoutIdentity(std::fs::File);

impl CheckoutIdentity {
    fn open(cwd: &std::path::Path) -> std::result::Result<Self, String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .map_err(|e| format!("Identify checkout: {e}"))?;
        if !output.status.success() {
            return Err("Cannot identify selected checkout".into());
        }
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::OsStr::from_bytes(
            output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout),
        );
        std::fs::File::open(path)
            .map(Self)
            .map_err(|e| format!("Open selected Git directory: {e}"))
    }

    fn verify(&self, cwd: &std::path::Path) -> std::result::Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let identity = |file: &std::fs::File| {
            file.metadata()
                .map(|m| (m.dev(), m.ino()))
                .map_err(|e| format!("Identify Git directory: {e}"))
        };
        if identity(&self.0)? != identity(&Self::open(cwd)?.0)? {
            return Err("Selected checkout changed during pane reservation".into());
        }
        Ok(())
    }
}

fn execute(
    op: Operation,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<Value> {
    match op {
        Operation::LaunchChoices { project } => {
            let registry = project_data::projects::from_rows(
                &agent_control::projects(data).map_err(|e| error("failed", e))?,
            );
            let checkouts = if let Some(name) = project {
                let project = registry
                    .get(&name)
                    .ok_or_else(|| invalid("Project no longer exists"))?;
                super::agent_launch::available_worktrees(std::path::Path::new(&project.root))
                    .map_err(invalid)?
                    .into_iter()
                    .filter_map(|w| Some(json!({"path":w.path.to_str()?, "branch":w.branch})))
                    .collect()
            } else {
                Vec::new()
            };
            Ok(
                json!({"server_instance":yinz_bus::agent_launch::server_instance(),"providers":yinz_runtime::agent::launch::offered(), "projects":registry.keys().collect::<Vec<_>>(), "worktrees":checkouts}),
            )
        },
        Operation::Launch {
            submission_id,
            kind,
            project,
            checkout,
        } => launch(
            submission_id,
            kind,
            project,
            checkout,
            client,
            senders,
            data,
        ),
        Operation::LaunchStatus {
            submission_id,
            agent,
        } => launch_status(&submission_id, &agent, senders, data),
        Operation::Destinations { agent } => {
            manage(agent, WebManageAction::Destinations, client, senders)
        },
        Operation::Move { agent, destination } => {
            manage(agent, WebManageAction::Move(destination), client, senders)
        },
        Operation::Close { agent } => manage(agent, WebManageAction::Close, client, senders),
        Operation::List {} => {
            let live = snapshot(senders)?;
            let sources = live
                .iter()
                .enumerate()
                .map(|(slot, agent)| {
                    let mut source = agent.label_source.clone();
                    source.id = slot.to_string();
                    source
                })
                .collect();
            let mut labels =
                palette_agents::compute_labels(sources, data).map_err(|e| error("failed", e))?;
            let mut live: Vec<_> = live
                .into_iter()
                .enumerate()
                .map(|(slot, row)| (row, labels.remove(&slot.to_string())))
                .collect();
            live.sort_by(|(a, a_label), (b, b_label)| {
                order(a, a_label.as_ref()).cmp(&order(b, b_label.as_ref()))
            });
            let count = |state: &str| {
                live.iter()
                    .filter(|(a, _)| a.metadata.get("state").is_some_and(|s| s == state))
                    .count()
            };
            let mut seen = HashSet::new();
            let mut ambiguous = HashSet::new();
            for (agent, _) in &live {
                let id = if agent.current_session {
                    Some(agent.id.clone())
                } else {
                    agent.peer_detail_id()
                };
                if let Some(id) = id
                    && !seen.insert(id.clone())
                {
                    ambiguous.insert(id);
                }
            }
            let agents: Vec<_> = live
                .iter()
                .map(|(a, label)| {
                    let mut row = row(a, label.as_ref());
                    row["tab_group_session"] = json!(a.tab_group_session);
                    row["tab_group"] = json!(a.tab_group);
                    if row["ref"]["id"]
                        .as_str()
                        .is_some_and(|id| ambiguous.contains(id))
                    {
                        row.as_object_mut().map(|fields| fields.remove("detail_id"));
                        row["ref"]["session_id"] = json!("");
                        row["can_prompt"] = json!(false);
                        row["prompt_unavailable"] = json!(
                            "Agent identity is ambiguous. Refresh Agents and reopen the card."
                        );
                    }
                    row
                })
                .collect();
            Ok(
                json!({"session":senders.session.current(),"now_ms":agent_proto::agents::now_ms(),
                "counts":{"waiting":count("waiting"),"busy":count("busy"),"idle":count("idle")},
                "agents":agents}),
            )
        },
        Operation::Prompts {} => Ok(
            json!({"prompts":history::prompt_library(data).map_err(|e| error("failed",e))?
            .into_iter().take(500).map(|p| json!({"text":p.text,"description":p.description})).collect::<Vec<_>>()}),
        ),
        Operation::Usage { agent } => {
            let live = detail_target(snapshot(senders)?, &agent)
                .ok_or_else(|| error("not_found", "Agent not found"))?;
            let provider = live
                .pane
                .agent
                .as_ref()
                .map(|a| a.kind.replace('◆', ""))
                .unwrap_or_default();
            let usage = if let Some(reason) = live.usage_unavailable {
                ProviderUsage {
                    provider,
                    status: ProviderUsageStatus::Unavailable,
                    observed_at_ms: None,
                    plan: None,
                    windows: vec![],
                    message: Some(reason.into()),
                }
            } else {
                crate::provider_usage::collect(&provider)
            };
            Ok(json!({"usage":usage}))
        },
        Operation::Focus { agent } => {
            let live = target(senders, &agent)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            if !live.current_session {
                verify_peer_focus(&live, &agent, deadline)?;
            }
            let (reply, rx) = crossbeam::channel::bounded(1);
            senders
                .send_to_screen(ScreenInstruction::WebFocusAgent {
                    agent,
                    owner: live.tab_group_session,
                    peer_instance: live.peer_instance,
                    deadline,
                    reply,
                })
                .map_err(|e| error("failed", e.to_string()))?;
            rx.recv_deadline(deadline)
                .map_err(|_| {
                    error(
                        "unavailable",
                        "Focus unavailable. Refresh Agents and try again.",
                    )
                })?
                .map_err(web_error)?;
            Ok(json!(true))
        },
        Operation::Prompt {
            agent,
            submission_id,
            text,
        } => control(
            agent,
            WebAgentAction::Prompt {
                text,
                submission_id,
            },
            client,
            senders,
            data,
        ),
        Operation::Terminal { agent, after } => control(
            agent,
            WebAgentAction::Terminal { after },
            client,
            senders,
            data,
        ),
        Operation::TerminalInput { agent, data: input } => control(
            agent,
            WebAgentAction::TerminalInput { data: input },
            client,
            senders,
            data,
        ),
        Operation::Screen { agent, lines } => {
            control(agent, WebAgentAction::Read { lines }, client, senders, data)
        },
        Operation::Keys { agent, keys } => control(
            agent,
            WebAgentAction::SendKeys { keys },
            client,
            senders,
            data,
        ),
        Operation::Event { run, revision, tz } => {
            let _ = tz;
            adapt(
                event(data, &run, revision)?,
                &run,
                None,
                None,
                false,
                None,
                false,
            )
        },
        Operation::Hit { run, revision, tz } => {
            event(data, &run, revision)?;
            adapt(
                read(
                    HistoryAction::Hit {
                        id: run.clone(),
                        revision,
                    },
                    tz.as_deref(),
                    client,
                    senders,
                    data,
                )?,
                &run,
                None,
                None,
                false,
                None,
                false,
            )
        },
        Operation::History {
            agent,
            mut run,
            mut from,
            query,
            tz,
            mut worker,
            mut before,
            mut after_revision,
        } => {
            let live = if let Some(id) = &agent {
                detail_target(snapshot(senders)?, id)
            } else {
                None
            };
            let retargeted = live
                .as_ref()
                .and_then(|a| a.run.as_ref())
                .is_some_and(|id| id != &run);
            if retargeted {
                if let Some(new_run) = live.as_ref().and_then(|a| a.run.as_ref()) {
                    run.clone_from(new_run);
                }
                from = None;
                before = None;
                after_revision = None;
                worker = None;
            }
            let labels = if let Some(a) = &live {
                labels(std::slice::from_ref(a), data)?
            } else {
                BTreeMap::new()
            };
            let action = if query.is_empty() {
                HistoryAction::Conversation {
                    id: run.clone(),
                    worker,
                    before,
                    after_revision: after_revision.or(from),
                }
            } else {
                HistoryAction::Run {
                    id: run.clone(),
                    from,
                    query: query.clone(),
                }
            };
            adapt(
                read(action, tz.as_deref(), client, senders, data)?,
                &run,
                live.as_ref(),
                live.as_ref().and_then(|a| labels.get(&a.id)),
                retargeted,
                agent.as_deref(),
                !query.is_empty(),
            )
        },
        Operation::Tools {
            run,
            revisions,
            range,
            before,
            tz,
        } => {
            if let Some(revisions) = revisions {
                let mut markdown = Vec::new();
                let mut tools = Vec::new();
                for rev in revisions {
                    if let HistoryReply::Reader {
                        markdown: text,
                        tool,
                        ..
                    } = event(data, &run, rev)?
                    {
                        markdown.push(text);
                        if !tool.is_empty() {
                            tools.push(tool);
                        }
                    }
                }
                Ok(
                    json!({"header":"Retained tools","markdown":markdown.join("\n\n"),"tool":tools.join("\n\n")}),
                )
            } else if let Some(range) = range {
                let page = adapt(
                    read(
                        HistoryAction::Tools {
                            id: run.clone(),
                            range,
                            before,
                        },
                        tz.as_deref(),
                        client,
                        senders,
                        data,
                    )?,
                    &run,
                    None,
                    None,
                    false,
                    None,
                    false,
                )?;
                let markdown = page["events"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| {
                        format!(
                            "{} · {}\n{}{}",
                            e["time"].as_str().unwrap_or_default(),
                            e["kind"].as_str().unwrap_or_default(),
                            e["markdown"].as_str().unwrap_or_default(),
                            if e["truncated"] == true {
                                "\n[truncated]"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(json!({"header":"Retained tools","markdown":markdown,"tool":"","page":page}))
            } else {
                Err(invalid("tools requires a scope"))
            }
        },
    }
}

#[cfg(test)]
#[path = "unit/web_agents.rs"]
mod tests;
