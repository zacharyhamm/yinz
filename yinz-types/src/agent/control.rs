//! Direct CLI control contracts and correlation of one submitted prompt with its hooks.

use super::{AgentEvent, AgentInfo, AgentSpawnSpec, AgentState, HookEvent, SpawnedAgent};
use crate::{keys::KeyWithModifier, pane_id::PaneId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
pub const MAX_TIMEOUT_MS: u64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgentRequest {
    List {
        project: Option<String>,
    },
    Status {
        pane_id: PaneId,
        expected_session_id: Option<String>,
    },
    Spawn {
        spec: AgentSpawnSpec,
        cwd: Option<PathBuf>,
        project: Option<String>,
        remote_host: Option<String>,
        #[serde(default)]
        automatic_firecracker_vm: bool,
    },
    Prompt {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        prompt: String,
        wait: bool,
        timeout_ms: u64,
    },
    Wait {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        until: AgentState,
        timeout_ms: u64,
    },
    Read {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        lines: usize,
    },
    Recover {
        pane_id: PaneId,
        expected_session_id: String,
        confirm_empty: bool,
    },
    SendKeys {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        keys: Vec<KeyWithModifier>,
    },
}

/// Outcomes describe the observed operation, independently of the agent's current state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    #[default]
    Ok,
    Submitted,
    Completed,
    NeedsInput,
    Failed,
    Interrupted,
    TimedOut,
    TargetChanged,
    TargetClosed,
    StatusUnavailable,
    Rejected,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentResponse {
    pub outcome: AgentOutcome,
    pub agents: Vec<AgentStatus>,
    pub spawned: Option<SpawnedAgent>,
    pub output: Option<String>,
    pub message: Option<String>,
    pub acknowledged: bool,
    pub turn_id: Option<String>,
}

impl AgentResponse {
    pub fn new(outcome: AgentOutcome) -> Self {
        Self {
            outcome,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    #[serde(with = "pane_string")]
    pub pane_id: PaneId,
    pub session: String,
    pub host_id: crate::internal_ids::HostId,
    pub tab_id: Option<crate::internal_ids::TabId>,
    pub target: crate::command::PaneTarget,
    pub agent: AgentInfo,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub ready: bool,
    pub status_available: bool,
    pub prompt_supported: bool,
}

mod pane_string {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        pane: &PaneId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(pane)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PaneId, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The caller registers this before writing and verifies pane identity and input ownership.
/// Codex requires provider turn IDs. Claude's hooks can omit them, so its fallback depends on
/// serialized hooks for the same conversation and cancellation on any competing input.
#[derive(Clone, Debug)]
pub struct PromptObservation {
    kind: String,
    conversation_id: String,
    prompt_sha256: String,
    acknowledged: bool,
    turn_id: Option<String>,
    finished: bool,
}

impl PromptObservation {
    pub fn new(kind: &str, conversation_id: &str, prompt: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            conversation_id: conversation_id.to_owned(),
            prompt_sha256: prompt_sha256(prompt),
            acknowledged: false,
            turn_id: None,
            finished: false,
        }
    }

    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    /// `Submitted` means a matching prompt hook arrived, never merely a successful PTY write.
    /// Completion is emitted at most once. Uncertainty keeps observing valid late hooks.
    /// An old completion cannot accept a prompt.
    pub fn observe(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> Option<AgentOutcome> {
        if self.finished {
            return None;
        }
        let outcome = self.observe_event(event, hook);
        if outcome.is_some_and(|outcome| {
            matches!(
                outcome,
                AgentOutcome::Completed | AgentOutcome::Failed | AgentOutcome::TargetChanged
            )
        }) {
            self.finished = true;
        }
        outcome
    }

    fn observe_event(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> Option<AgentOutcome> {
        let Some(hook) = hook else {
            return Some(AgentOutcome::StatusUnavailable);
        };
        if hook.is_worker() {
            return (event.kind == self.kind
                && hook.session_id == self.conversation_id
                && event.session_id.as_deref() == Some(self.conversation_id.as_str())
                && super::hook_state(hook) == Some(AgentState::Waiting)
                && super::waiting_needs_a_person(hook))
            .then_some(AgentOutcome::NeedsInput);
        }
        if hook.session_id.is_empty() || event.session_id.as_deref().is_none_or(str::is_empty) {
            return Some(AgentOutcome::StatusUnavailable);
        }
        if event.kind != self.kind
            || hook.session_id != self.conversation_id
            || event.session_id.as_deref() != Some(self.conversation_id.as_str())
            || hook.hook_event_name == "SessionEnd"
        {
            return Some(AgentOutcome::TargetChanged);
        }
        if hook.hook_event_name == "SessionStart" && hook.source.as_deref() != Some("compact") {
            return Some(AgentOutcome::TargetChanged);
        }
        if hook.hook_event_name == "UserPromptSubmit" {
            let fingerprint = hook
                .prompt_sha256
                .clone()
                .or_else(|| hook.prompt.as_deref().map(prompt_sha256));
            if fingerprint.as_deref() != Some(self.prompt_sha256.as_str()) {
                return Some(AgentOutcome::Interrupted);
            }
            let turn_id = hook.turn_id.clone().filter(|id| !id.is_empty());
            if self.kind == "codex" && turn_id.is_none() {
                return Some(AgentOutcome::StatusUnavailable);
            }
            if self.acknowledged {
                return (turn_id != self.turn_id).then_some(AgentOutcome::Interrupted);
            }
            self.acknowledged = true;
            self.turn_id = turn_id;
            return Some(AgentOutcome::Submitted);
        }
        if !self.acknowledged
            || self
                .turn_id
                .as_deref()
                .is_some_and(|turn| hook.turn_id.as_deref() != Some(turn))
        {
            return None;
        }
        match hook.hook_event_name.as_str() {
            "Stop" => Some(AgentOutcome::Completed),
            "StopFailure" => Some(AgentOutcome::Failed),
            "Interrupt" => Some(AgentOutcome::Interrupted),
            _ if super::hook_state(hook) == Some(AgentState::Waiting)
                && super::waiting_needs_a_person(hook) =>
            {
                Some(AgentOutcome::NeedsInput)
            },
            _ => None,
        }
    }
}

pub fn prompt_sha256(prompt: &str) -> String {
    Sha256::digest(prompt.replace("\r\n", "\n").trim().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "unit/control_tests.rs"]
mod tests;
