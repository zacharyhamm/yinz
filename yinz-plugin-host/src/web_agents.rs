//! Native, bounded adapter over the terminal palette's live data and archive readers.
//! This runs on the counted, pause-aware AgentView worker owned by the plugin thread.
use super::{agent_control, palette_agents};
use palette_agents::history;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use yinz_bus::{
    command_palette::{self as palette, AgentAction, AgentReply, HistoryAction, HistoryReply},
    screen::ScreenInstruction,
    thread_bus::ThreadSenders,
    web::{WebAgentRef, WebFocusError, WebLiveAgent},
};
use yinz_storage::data_access::DataAccess;
use yinz_types::{
    agent::{
        control::{
            valid_web_identity, AgentOutcome, AgentRequest, AgentResponse, ProviderUsage,
            ProviderUsageStatus, WebAgentAction,
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
            Self::Focus { agent } => agent_ref(agent)?,
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
fn order<'a>(
    a: &'a WebLiveAgent,
    labels: &'a BTreeMap<String, palette::AgentLabels>,
) -> LiveAgentOrder<'a> {
    let label = labels.get(&a.id);
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
    let identity = WebAgentRef {
        id: live.id.clone(),
        kind: kind.into(),
        session_id: session_id.into(),
    };
    json!({"id":live.id,"ref":identity,"session":live.session,"current_session":live.current_session,
        "pane_id":live.pane.id.to_string(),"pane":live.pane_label,"provider":live.provider,
        "state":live.metadata.get("state"),"glyph":live.glyph,"status":live.status,"host":live.host,
        "title":live.pane.title,"activity":live.activity,"project":label.map(|l| l.project.as_str()).unwrap_or_default(),
        "branch":label.map(|l| l.branch.as_str()).unwrap_or_default(),"last_reply":label.and_then(|l| l.last_reply.as_ref())
            .map(|r| json!({"text":r.text,"truncated":r.truncated})),
        "last_hook_at_ms":live.pane.last_hook_at_ms,"focused":live.focused,"selectable":live.selectable,
        "suppressed":live.suppressed,"directory":live.metadata.get("directory"),"run":live.run,
        "wait_needs_a_person":agent.is_some_and(|a| a.wait_needs_a_person),"source":live.metadata.get("source"),
        "can_prompt":live.matches(&identity) && valid_web_identity(kind) && valid_web_identity(session_id),
        "prompt_unavailable":(!valid_web_identity(session_id)).then_some("Agent has no conversation ID"),
        "usage_unavailable":live.usage_unavailable})
}
fn target(senders: &ThreadSenders, agent: &WebAgentRef) -> Result<WebLiveAgent> {
    snapshot(senders)?
        .into_iter()
        .find(|a| a.matches(agent))
        .ok_or_else(|| error("target_changed", "Agent disappeared or changed identity"))
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
    let request = AgentRequest::WebGuarded {
        pane_id: live.pane.id,
        expected_kind: agent.kind,
        expected_session_id: agent.session_id,
        action,
    };
    let mut response = if live.current_session {
        agent_control::control_agent(request, client, senders, data)
    } else {
        // Only a session obtained from this fresh snapshot can become a route.
        palette_agents::control_peer(
            &live.session,
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
    agent_gone: bool,
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
                .filter_map(|r| navigation(&r.action).map(|n| (r.id.clone(), n)))
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
                json!({"run":run,"retargeted":retargeted,"agent_gone":agent_gone,"live":live.map(|a| row(a,label)),
                "header":{"name":header.name,"status":header.status,"location":header.location,"started":header.started,
                    "task":header.task,"project":header.project,"branch":header.branch,"session_id":header.session_id,"details":header.details},
                "events":events,"earlier":action("earlier").is_some(),"next_from":next_from,
                "previous_run":run_id("previous"),"origin_run":run_id("origin"),"resumable":false,
                "activity_ranges":activity_ranges,"navigation":requests,"status":status}),
            )
        },
        HistoryReply::Rows { .. } => Err(error("failed", "Unexpected history navigation")),
    }
}

fn execute(
    op: Operation,
    client: ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<Value> {
    match op {
        Operation::List {} => {
            let mut live = snapshot(senders)?;
            let labels = labels(&live, data)?;
            live.sort_by(|a, b| order(a, &labels).cmp(&order(b, &labels)));
            let count = |state: &str| {
                live.iter()
                    .filter(|a| a.metadata.get("state").is_some_and(|s| s == state))
                    .count()
            };
            Ok(
                json!({"session":senders.session.current(),"now_ms":agent_proto::agents::now_ms(),
                "counts":{"waiting":count("waiting"),"busy":count("busy"),"idle":count("idle")},
                "agents":live.iter().map(|a| row(a, labels.get(&a.id))).collect::<Vec<_>>()}),
            )
        },
        Operation::Prompts {} => Ok(
            json!({"prompts":history::prompt_library(data).map_err(|e| error("failed",e))?
            .into_iter().take(500).map(|p| json!({"text":p.text,"description":p.description})).collect::<Vec<_>>()}),
        ),
        Operation::Usage { agent } => {
            let live = snapshot(senders)?
                .into_iter()
                .find(|a| a.id == agent)
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
            target(senders, &agent)?;
            let (reply, rx) = crossbeam::channel::bounded(1);
            senders
                .send_to_screen(ScreenInstruction::WebFocusAgent { agent, reply })
                .map_err(|e| error("failed", e.to_string()))?;
            rx.recv_timeout(Duration::from_secs(5))
                .map_err(|_| error("unavailable", "Focus unavailable"))?
                .map_err(|e| match e {
                    WebFocusError::TargetChanged => {
                        error("target_changed", "Agent disappeared or changed identity")
                    },
                    WebFocusError::NoTerminal => error("unavailable", "No terminal attached"),
                    WebFocusError::Unavailable(message) => error("unavailable", message),
                    WebFocusError::Failed(message) => error("failed", message),
                })?;
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
                false,
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
                false,
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
                snapshot(senders)?.into_iter().find(|a| &a.id == id)
            } else {
                None
            };
            let agent_gone = agent.is_some() && live.is_none();
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
                agent_gone,
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
                    false,
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
