//! Session launch coordination and durable, secret-free receipts. Runs on host workers.
use super::{
    agent_control::{
        finish_spawn, prepare_automation_agent, prepare_new_project_agent, NewProjectProgress,
    },
    agent_launch::{spawn_agent_prepared_with_gate, AgentSpawnError, OpeningPromptPolicy},
};
use std::{cell::RefCell, sync::Arc};
use yinz_bus::{
    agent_launch::{server_instance, LaunchGate},
    screen::ScreenInstruction,
    thread_bus::ThreadSenders,
};
use yinz_storage::data_access::DataAccess;
use yinz_types::{
    agent::{
        control::{validate_submission_id, AgentOutcome, AgentResponse},
        launch::*,
    },
    internal_ids::DataRevision,
    plugin_data::*,
};

const NS: &str = "shared:yinz_launch";
const ENTITY: &str = "receipt";
struct Journal<'a> {
    data: &'a DataAccess,
    host: String,
    session: String,
    uid: String,
}
impl<'a> Journal<'a> {
    fn new(data: &'a DataAccess, senders: &ThreadSenders, id: &str) -> Result<Self, String> {
        let host = senders.launches.host_id.to_string();
        let session = senders.session.current().to_string();
        let uid = serde_json::to_string(&(&host, &session, id)).map_err(|e| e.to_string())?;
        Ok(Self {
            data,
            host,
            session,
            uid,
        })
    }
    fn ensure(&self) -> Result<(), String> {
        self.data
            .plugin_data()
            .define_entity(
                NS,
                "yinz_launch",
                &EntityDef::new(ENTITY, 1)
                    .attr(AttrDef::text("host_id").required())
                    .attr(AttrDef::text("yinz_session_name").required())
                    .attr(AttrDef::text("receipt").required()),
                false,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn read(&self) -> Result<Option<(LaunchReceipt, DataRevision)>, String> {
        let rows = self
            .data
            .plugin_data()
            .fetch(
                NS,
                &FetchRequest::entity(ENTITY)
                    .filter(
                        Pred::eq("uid", self.uid.as_str())
                            .and(Pred::eq("host_id", self.host.as_str()))
                            .and(Pred::eq("yinz_session_name", self.session.as_str())),
                    )
                    .select(["receipt", "rev"])
                    .limit(1),
            )
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.rows.first() else {
            return Ok(None);
        };
        let Some(DataValue::Text(json)) = rows.get(row, "receipt") else {
            return Err("invalid launch history".into());
        };
        let Some(DataValue::Int(rev)) = rows.get(row, "rev") else {
            return Err("missing launch history revision".into());
        };
        Ok(Some((
            serde_json::from_str(json).map_err(|e| e.to_string())?,
            DataRevision::new(*rev),
        )))
    }
    fn write(&self, receipt: &LaunchReceipt, rev: &mut DataRevision) -> Result<(), String> {
        let json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;
        self.data
            .plugin_data()
            .apply(
                NS,
                &[Mutation::put(ENTITY)
                    .uid(&self.uid)
                    .set("host_id", self.host.as_str())
                    .set("yinz_session_name", self.session.as_str())
                    .set("receipt", json)
                    .expect_rev(*rev)
                    .build()],
            )
            .map_err(|e| e.to_string())?;
        *rev = DataRevision::new(rev.get() + 1);
        Ok(())
    }
}

fn fresh(id: &str, digest: String) -> LaunchReceipt {
    LaunchReceipt {
        version: 2,
        launch_id: id.into(),
        request_digest: digest,
        server_instance: server_instance().into(),
        outcome: LaunchOutcome::Pending,
        phase: LaunchPhase::Admits,
        reason: None,
        process: ProcessState::NotStarted,
        resources: Default::default(),
        spawned: None,
        disposition: WorkerDisposition::Reserved,
    }
}
fn answer(receipt: LaunchReceipt, message: Option<String>) -> AgentResponse {
    let outcome = match receipt.outcome {
        LaunchOutcome::Started if receipt.disposition == WorkerDisposition::Available => {
            AgentOutcome::Ok
        },
        LaunchOutcome::Rejected | LaunchOutcome::Conflict => AgentOutcome::Rejected,
        LaunchOutcome::Failed | LaunchOutcome::Cancelled => AgentOutcome::Failed,
        _ => AgentOutcome::StatusUnavailable,
    };
    let mut response = AgentResponse::new(outcome);
    response.spawned = receipt.spawned.clone();
    response.launch_resources = receipt.resources.checkout.clone();
    response.launch = Some(receipt);
    response.message = message;
    response
}
fn unavailable(id: &str, digest: String, error: String) -> AgentResponse {
    let mut receipt = fresh(id, digest);
    receipt.outcome = LaunchOutcome::Unknown;
    receipt.reason = Some(LaunchReason::HistoryUnavailable);
    receipt.process = ProcessState::Unknown;
    receipt.disposition = WorkerDisposition::Unknown;
    answer(receipt, Some(error))
}

/// Observation never creates a terminal or materializes a checkout. A missing tombstone is
/// unknown; an old server's pending/starting record is never replayed.
pub(super) fn status(id: &str, senders: &ThreadSenders, data: &DataAccess) -> AgentResponse {
    let stored = senders
        .launches
        .operations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned();
    let mut receipt = match stored.map(Ok).unwrap_or_else(|| {
        Journal::new(data, senders, id)
            .and_then(|j| j.read())
            .and_then(|r| {
                r.map(|(r, _)| r)
                    .ok_or_else(|| "launch history unavailable; do not spawn again".into())
            })
    }) {
        Ok(receipt) => receipt,
        Err(error) => return unavailable(id, String::new(), error),
    };
    if receipt.server_instance != server_instance() {
        receipt.disposition = WorkerDisposition::Unknown;
        if receipt.outcome == LaunchOutcome::Pending {
            receipt.outcome = LaunchOutcome::Unknown;
            receipt.process = ProcessState::Unknown;
        }
        receipt.reason = Some(LaunchReason::HistoryUnavailable);
    } else {
        let gate = senders
            .launches
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|gate| gate.launch_id == id)
            .cloned();
        if let Some(gate) = gate {
            let state = gate.snapshot();
            receipt.resources.slot = state.slot.clone();
            receipt.resources.terminal_allocated = state.allocated.is_some();
            receipt.resources.placeholder_owned = state.slot.is_some();
            receipt.resources.process_owned = state.claimed;
            if state.superseded {
                // A deliberate ordinary rerun belongs to a different process identity. It
                // cannot turn the historical launch into a newly available worker.
                receipt.disposition = WorkerDisposition::Unknown;
            }
            if state.claimed {
                receipt.process = if state.running {
                    ProcessState::Running
                } else {
                    ProcessState::Starting
                };
            }
            if receipt.outcome == LaunchOutcome::Started && !state.finished {
                receipt.outcome = LaunchOutcome::Pending;
                receipt.phase = LaunchPhase::Registers;
            }
            if state.layout_released || state.cleanup_confirmed || state.exited {
                receipt.disposition = WorkerDisposition::Closed;
                receipt.process = if !state.claimed || state.non_execution_proven {
                    ProcessState::NotStarted
                } else if state.cleanup_confirmed || state.exited {
                    ProcessState::Exited
                } else {
                    ProcessState::Unknown
                };
                receipt.resources.cleanup_pending =
                    !(state.cleanup_confirmed && state.layout_released);
            }
            if state.cleanup_unknown {
                receipt.process = if !state.claimed || state.non_execution_proven {
                    ProcessState::NotStarted
                } else {
                    ProcessState::Unknown
                };
                receipt.disposition = if state.layout_released {
                    WorkerDisposition::Closed
                } else {
                    WorkerDisposition::Unknown
                };
                receipt.resources.cleanup_pending = true;
                if receipt.outcome == LaunchOutcome::Pending {
                    receipt.outcome = LaunchOutcome::Unknown;
                }
            }
            if state.revoked.is_some()
                && !state.finished
                && receipt.outcome == LaunchOutcome::Pending
            {
                receipt.reason = receipt.reason.or(state.revoked);
                if state.cleanup_confirmed && state.layout_released {
                    receipt.outcome = match receipt.reason {
                        Some(
                            LaunchReason::PrepareFailed
                            | LaunchReason::ExecFailed
                            | LaunchReason::RegistrationFailed,
                        ) => LaunchOutcome::Failed,
                        Some(
                            LaunchReason::InsufficientRoom
                            | LaunchReason::PaneLimit
                            | LaunchReason::InvalidRequest,
                        ) => LaunchOutcome::Rejected,
                        _ => LaunchOutcome::Cancelled,
                    };
                    receipt.resources.cleanup_pending = false;
                    receipt.phase = LaunchPhase::Finished;
                } else {
                    receipt.phase = LaunchPhase::Aborts;
                    receipt.resources.cleanup_pending = true;
                }
            }
            if state.departed {
                receipt.disposition = WorkerDisposition::Unknown;
                receipt.process = ProcessState::Unknown;
                receipt.resources.placeholder_owned = false;
                receipt.resources.process_owned = false;
                receipt.resources.cleanup_pending = false;
            }
        }
    }

    answer(receipt, None)
}

pub(super) fn launch(
    id: String,
    request: LaunchRequest,
    client: crate::ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> AgentResponse {
    launch_with_prepare(
        id,
        request,
        client,
        senders,
        data,
        OpeningPromptPolicy::Reject,
        None,
        None,
        None,
    )
}

type Preparation<'a> = Box<
    dyn FnOnce(
            &mut yinz_types::agent::AgentSpawnSpec,
        ) -> Result<super::agent_launch::PreparedAgent, AgentSpawnError>
        + 'a,
>;
pub(super) fn launch_with_prepare(
    id: String,
    request: LaunchRequest,
    client: crate::ClientId,
    senders: &ThreadSenders,
    data: &DataAccess,
    opening_prompt: OpeningPromptPolicy,
    origin: Option<yinz_types::data::OriginatingPlugin>,
    title: Option<String>,
    prepare: Option<Preparation<'_>>,
) -> AgentResponse {
    let digest = match request.digest() {
        Ok(digest) => digest,
        Err(e) => return unavailable(&id, String::new(), e.to_string()),
    };
    if let Err(error) = validate_submission_id(&id) {
        let mut receipt = fresh(&id, digest);
        receipt.outcome = LaunchOutcome::Rejected;
        receipt.reason = Some(LaunchReason::InvalidRequest);
        return answer(receipt, Some(error.into()));
    }
    let journal = match Journal::new(data, senders, &id) {
        Ok(journal) => journal,
        Err(error) => return unavailable(&id, digest, error),
    };
    if let Err(error) = journal.ensure() {
        return unavailable(&id, digest, error);
    }
    let initial = fresh(&id, digest.clone());
    let mut revision = REV_ABSENT;
    {
        let _lock = senders
            .launches
            .intent_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let existing = senders
            .launches
            .operations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned();
        if let Some(mut original) = existing {
            if original.request_digest != digest {
                original.outcome = LaunchOutcome::Conflict;
                original.reason = Some(LaunchReason::RequestConflict);
                return answer(
                    original,
                    Some("launch ID already belongs to a different request".into()),
                );
            }
            return status(&id, senders, data);
        }
        match journal.read() {
            Ok(Some((mut original, _))) => {
                if original.request_digest != digest {
                    original.outcome = LaunchOutcome::Conflict;
                    original.reason = Some(LaunchReason::RequestConflict);
                    return answer(
                        original,
                        Some("launch ID already belongs to a different request".into()),
                    );
                }
                return status(&id, senders, data);
            },
            Err(error) => return unavailable(&id, digest, error),
            Ok(None) => {},
        }
        // IDs are caller-chosen but incarnation-qualified. Losing the journal or restarting
        // cannot turn an old ID's absence into permission to execute it again.
        if !id.starts_with(&format!("{}.", server_instance())) {
            return unavailable(&id, digest, "launch ID is not qualified by this server incarnation; query launch-support before recording a new intent".into());
        }
        if let Err(error) = journal.write(&initial, &mut revision) {
            return unavailable(&id, digest, error);
        }
        senders
            .launches
            .operations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), initial.clone());
    }
    let new_project = request.new_project.is_some();
    let gate = Arc::new(LaunchGate::new(
        id.clone(),
        matches!(
            request.placement,
            yinz_types::agent::control::AgentPlacement::Suppressed(_)
        ),
    ));
    let progress = RefCell::new((initial, revision));
    let publish = |phase,
                   resources: Option<yinz_types::agent::AgentLaunchResources>|
     -> Result<(), AgentSpawnError> {
        let mut progress = progress.borrow_mut();
        let (receipt, rev) = &mut *progress;
        receipt.phase = phase;
        receipt.resources.slot = gate.snapshot().slot;
        receipt.resources.terminal_allocated = receipt.resources.slot.is_some();
        receipt.resources.placeholder_owned = receipt.resources.slot.is_some();
        if let Some(resources) = resources {
            receipt.resources.created_tab = resources.created_tab_id.is_some();
            receipt.resources.checkout = Some(resources);
        }
        journal.write(receipt, rev).map_err(AgentSpawnError::from)?;
        senders
            .launches
            .operations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), receipt.clone());
        Ok(())
    };
    let result = spawn_agent_prepared_with_gate(
        request.placement,
        request.spec,
        client,
        senders,
        gate.clone(),
        opening_prompt,
        origin,
        title,
        |spec| {
            publish(LaunchPhase::Prepares, None)?;
            if let Some(prepare) = prepare {
                return prepare(spec);
            }
            if let Some((name, destination)) = request.new_project {
                let mut project_progress = NewProjectProgress::default();
                return prepare_new_project_agent(
                    spec,
                    &name,
                    &destination,
                    senders,
                    data,
                    &mut project_progress,
                )
                .map_err(|mut error| {
                    if let Some(root) = project_progress.retained {
                        error
                            .0
                            .launch_resources
                            .get_or_insert_with(Default::default)
                            .cwd = Some(root);
                    }
                    error
                });
            }
            prepare_automation_agent(
                spec,
                request.cwd,
                request.project,
                request.remote_host,
                data,
                &senders.session.current(),
            )
        },
        |resources, git| {
            progress.borrow_mut().0.resources.git = git.to_vec();
            publish(LaunchPhase::Starts, Some(resources.clone()))
        },
    );
    let mut response = finish_spawn(result, senders);
    let (mut receipt, mut rev) = progress.into_inner();
    let state = gate.snapshot();
    receipt.resources.slot = state.slot.clone();
    receipt.spawned = response.spawned.clone();
    receipt.resources.checkout = response
        .launch_resources
        .clone()
        .or(receipt.resources.checkout);
    receipt.resources.terminal_allocated = state.allocated.is_some();
    receipt.resources.placeholder_owned = state.slot.is_some();
    receipt.resources.created_tab = state.destination.as_ref().is_some_and(|d| d.created);
    if receipt.resources.git.is_empty()
        && let Some(resources) = &receipt.resources.checkout
        && let Some(path) = &resources.cwd
    {
        receipt.resources.git.push(GitResource {
            kind: if new_project {
                GitResourceKind::ProjectDirectory
            } else {
                GitResourceKind::Worktree
            },
            ownership: ResourceOwnership::RetainedAttempt,
            path: path.clone(),
            branch: resources.branch.clone(),
        });
    }
    receipt.resources.process_owned = state.claimed;
    receipt.reason = state.revoked;
    if state.registered && state.running && !state.exited && state.revoked.is_none() {
        receipt.outcome = LaunchOutcome::Started;
        receipt.phase = LaunchPhase::Finished;
        receipt.process = ProcessState::Running;
        receipt.disposition = WorkerDisposition::Available;
    } else {
        if let Some(terminal) = state.allocated {
            gate.revoke(LaunchReason::PlacementRevoked);
            let (reply, receiver) = std::sync::mpsc::channel();
            let layout_released = senders
                .send_to_screen(ScreenInstruction::ReleaseAgentLaunch {
                    terminal_id: terminal,
                    reply,
                })
                .is_ok()
                && receiver
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap_or(false);
            let _ = senders.send_to_pty(yinz_bus::pty::PtyInstruction::ClosePane(
                yinz_types::pane_id::PaneId::Terminal(terminal),
                None,
            ));
            if layout_released {
                gate.wait_cleanup(std::time::Duration::from_secs(5));
            }
        }
        let state = gate.snapshot();
        if let Some(destination) = &state.destination {
            let resources = receipt
                .resources
                .checkout
                .get_or_insert_with(Default::default);
            resources.tab_id = Some(destination.tab_id);
            resources.created_tab_id = destination.created.then_some(destination.tab_id);
            if state.cleanup_confirmed {
                let cleanup =
                    super::agent_launch::cleanup_agent_tab(senders, Some(destination), resources);
                if !cleanup.is_empty() {
                    response
                        .message
                        .get_or_insert_with(String::new)
                        .push_str(&cleanup);
                }
            }
        }
        receipt.process = if !state.claimed || state.non_execution_proven {
            ProcessState::NotStarted
        } else if state.cleanup_confirmed && (state.running || state.exited) {
            ProcessState::Exited
        } else {
            ProcessState::Unknown
        };
        receipt.reason = receipt
            .reason
            .or(Some(if receipt.phase == LaunchPhase::Admits {
                LaunchReason::InvalidRequest
            } else if receipt.phase == LaunchPhase::Prepares {
                LaunchReason::PrepareFailed
            } else {
                LaunchReason::ExecFailed
            }));
        receipt.resources.cleanup_pending = state.cleanup_unknown
            || (state.allocated.is_some() && !(state.cleanup_confirmed && state.layout_released));
        receipt.outcome = if state.cleanup_unknown
            || (state.claimed
                && receipt.process == ProcessState::Unknown
                && !receipt.resources.cleanup_pending)
        {
            LaunchOutcome::Unknown
        } else if receipt.resources.cleanup_pending {
            LaunchOutcome::Pending
        } else if receipt.phase == LaunchPhase::Admits {
            LaunchOutcome::Rejected
        } else if matches!(
            receipt.reason,
            Some(
                LaunchReason::ControllerMoved
                    | LaunchReason::ControllerClosed
                    | LaunchReason::DestinationClosed
                    | LaunchReason::PlacementRevoked
            )
        ) {
            LaunchOutcome::Cancelled
        } else {
            LaunchOutcome::Failed
        };
        receipt.phase = if receipt.resources.cleanup_pending {
            LaunchPhase::Aborts
        } else {
            LaunchPhase::Finished
        };
        receipt.disposition = if receipt.resources.cleanup_pending {
            WorkerDisposition::Unknown
        } else {
            WorkerDisposition::Closed
        };
    }
    let candidate = receipt.outcome == LaunchOutcome::Started;
    if candidate {
        receipt.outcome = LaunchOutcome::Pending;
        receipt.phase = LaunchPhase::Registers;
    }
    let durable = match journal.write(&receipt, &mut rev) {
        Ok(()) => true,
        Err(error) => {
            gate.revoke(LaunchReason::HistoryUnavailable);
            if let Some(slot) = &state.slot {
                let _ = senders.send_to_screen(ScreenInstruction::ClosePane(
                    slot.pane_id,
                    None,
                    None,
                    None,
                ));
            }
            receipt.outcome = LaunchOutcome::Unknown;
            receipt.reason = Some(LaunchReason::HistoryUnavailable);
            response.message = Some(format!(
                "launch receipt could not be committed: {error}; observe the same launch ID"
            ));
            false
        },
    };
    if candidate && durable {
        let (reply, receiver) = std::sync::mpsc::channel();
        let committed = state.allocated.is_some_and(|terminal_id| {
            senders
                .send_to_screen(ScreenInstruction::CommitAgentLaunch { terminal_id, reply })
                .is_ok()
                && receiver
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap_or(false)
        }) || gate.snapshot().finished;
        if committed {
            receipt.outcome = LaunchOutcome::Started;
            receipt.phase = LaunchPhase::Finished;
        } else {
            receipt.outcome = LaunchOutcome::Pending;
            receipt.phase = LaunchPhase::Aborts;
            receipt.reason = gate
                .snapshot()
                .revoked
                .or(Some(LaunchReason::PlacementRevoked));
            receipt.resources.cleanup_pending = true;
            gate.revoke(LaunchReason::PlacementRevoked);
            if let Some(slot) = &state.slot {
                let _ = senders.send_to_screen(ScreenInstruction::ClosePane(
                    slot.pane_id,
                    None,
                    None,
                    None,
                ));
            }
        }
        if let Err(error) = journal.write(&receipt, &mut rev) {
            receipt.outcome = LaunchOutcome::Unknown;
            receipt.reason = Some(LaunchReason::HistoryUnavailable);
            response.message = Some(format!(
                "launch final receipt unavailable: {error}; observe the original launch"
            ));
        }
    }
    senders
        .launches
        .operations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), receipt);
    // Durable handoff and its observation are separate. If the worker already exited or
    // closed while the final receipt was written, return its current disposition as well.
    let mut observed = status(&id, senders, data);
    observed.message = response.message;
    observed
}

#[cfg(test)]
#[path = "unit/launch_transaction.rs"]
pub(crate) mod tests;
