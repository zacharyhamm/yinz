//! Direct CLI control contracts and correlation of one submitted prompt with its hooks.

use super::{
    provider::AgentInput, AgentEvent, AgentInfo, AgentSpawnSpec, AgentState, HookEvent,
    SpawnedAgent,
};
use crate::{keys::KeyWithModifier, pane_id::PaneId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    path::PathBuf,
};

pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
pub const MAX_TIMEOUT_MS: u64 = 86_400_000;

pub fn validate_submission_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err("submission ID must be 1 to 128 ASCII letters, digits, '.', '_' or '-'")
    } else {
        Ok(())
    }
}

/// Initial placement only; project selection and later pane moves remain independent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlacement {
    #[default]
    Automatic,
    Tab(crate::internal_ids::TabId),
    NewTab(Option<String>),
    Suppressed(Option<crate::internal_ids::TabId>),
}

impl AgentPlacement {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::NewTab(Some(name)) = self
            && (name.trim().is_empty() || name.chars().any(char::is_control))
        {
            return Err("new tab name must be nonblank and contain no control characters");
        }
        Ok(())
    }

    /// Normalize the flat launch-file fields before sending the typed request.
    pub fn from_launch_fields(
        tab: &serde_json::Value,
        new_tab: &serde_json::Value,
        suppressed: &serde_json::Value,
    ) -> Result<Self, &'static str> {
        let existing =
            if tab.is_null() {
                None
            } else {
                Some(tab.as_u64().and_then(|id| usize::try_from(id).ok()).ok_or(
                    "tab must be a nonnegative stable numeric ID (names are not supported)",
                )?)
            };
        let new = match new_tab {
            serde_json::Value::Null | serde_json::Value::Bool(false) => None,
            serde_json::Value::Bool(true) => Some(None),
            serde_json::Value::String(name) => Some(Some(name.clone())),
            _ => return Err("new_tab must be true, false, null, or a tab name"),
        };
        let suppressed = match suppressed {
            serde_json::Value::Null | serde_json::Value::Bool(false) => false,
            serde_json::Value::Bool(true) => true,
            _ => return Err("suppressed must be true, false, or null"),
        };
        if suppressed {
            if new.is_some() {
                return Err("suppressed and new_tab are mutually exclusive");
            }
            return Ok(Self::Suppressed(
                existing.map(crate::internal_ids::TabId::new),
            ));
        }
        let placement = match (existing, new) {
            (Some(_), Some(_)) => return Err("tab and new_tab are mutually exclusive"),
            (Some(id), None) => Self::Tab(crate::internal_ids::TabId::new(id)),
            (None, Some(name)) => Self::NewTab(name),
            (None, None) => Self::Automatic,
        };
        placement.validate()?;
        Ok(placement)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum_macros::EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AgentRestartSession {
    Resume,
    Fresh,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RestartedAgent {
    pub pane_id: PaneId,
    pub session_id: Option<String>,
    pub command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgentRequest {
    /// Read-only feature negotiation before sending placement to an older server.
    SpawnPlacementSupport,
    /// Read-only negotiation before sending a suppressed launch to an older server.
    SpawnSuppressedSupport,
    /// Read-only feature negotiation before sending a sandboxed spawn to an older server.
    SpawnBubblewrapSupport,
    List {
        project: Option<String>,
    },
    /// Account usage reported by the provider on this session's host.
    Usage {
        provider: String,
    },
    Status {
        pane_id: PaneId,
        expected_session_id: Option<String>,
    },
    Show {
        pane_id: PaneId,
        expected_session_id: Option<String>,
    },
    Hide {
        pane_id: PaneId,
        expected_session_id: Option<String>,
    },
    Spawn {
        #[serde(default)]
        placement: AgentPlacement,
        spec: AgentSpawnSpec,
        cwd: Option<PathBuf>,
        project: Option<String>,
        remote_host: Option<String>,
    },
    /// Distinct wire variant so older servers cannot silently launch in another checkout.
    SpawnNewProject {
        name: String,
        destination: PathBuf,
        spec: AgentSpawnSpec,
        #[serde(default)]
        placement: AgentPlacement,
    },
    Restart {
        pane_id: PaneId,
        prompt: Option<String>,
        session: AgentRestartSession,
    },
    Prompt {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        /// Caller-chosen retry identity, or a server-generated UUID when omitted.
        #[serde(default)]
        submission_id: Option<String>,
        /// Ignored compatibility field: explicit prompts always replace drafts.
        #[serde(default)]
        preserve_draft: bool,
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
    WaitTurn {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        submission_id: String,
        timeout_ms: u64,
    },
    Read {
        pane_id: PaneId,
        expected_session_id: Option<String>,
        lines: usize,
    },
    ClearDraft {
        pane_id: PaneId,
        expected_session_id: String,
        input_revision: u64,
        confirm_discard: bool,
        #[serde(default)]
        interrupted: bool,
        timeout_ms: u64,
    },
    Recover {
        pane_id: PaneId,
        expected_session_id: String,
        confirm_empty: bool,
        /// The caller saw the agent stop a turn without reporting it (an Esc interrupt), so a
        /// busy or waiting record may be reset to idle.
        #[serde(default)]
        interrupted: bool,
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
    /// The requested pane was absent before this operation began.
    TargetNotFound,
    /// The requested pane has no agent record.
    NotAgent,
    /// The requested agent's process has already exited.
    TargetExited,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub status: ProviderUsageStatus,
    pub observed_at_ms: Option<u64>,
    pub plan: Option<String>,
    pub windows: Vec<ProviderUsageWindow>,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUsageStatus {
    Ok,
    Unavailable,
    Unsupported,
    Expired,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsageWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentResponse {
    pub outcome: AgentOutcome,
    pub agents: Vec<AgentStatus>,
    pub spawned: Option<SpawnedAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted: Option<RestartedAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_resources: Option<super::AgentLaunchResources>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ProviderUsage>,
    pub output: Option<String>,
    pub message: Option<String>,
    pub acknowledged: bool,
    pub turn_id: Option<String>,
    /// Stable Yinz submission identity, including providers without a turn ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    /// Claude background work the completed turn launched. Its final answer arrives in a later
    /// turn; `agent wait --until idle` returns it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background_tasks: Vec<String>,
}

impl AgentResponse {
    pub fn new(outcome: AgentOutcome) -> Self {
        Self {
            outcome,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum_macros::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum NotReadyReason {
    ClearingDraft,
    PromptUnsupported,
    NotIdle,
    PendingSubmission,
    /// An acknowledged turn still owns the input until completion or explicit recovery.
    UnresolvedTurn,
    UnconsumedInput,
    /// An agent detected in the pane's foreground that Yinz did not launch. It cannot be
    /// prompted; relaunch it with `agent spawn`.
    Unmanaged,
}

/// The capability checks sampled together by the screen thread. These explain
/// `prompt_unsupported`; idle state and input consumption are separate checks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentReadiness {
    pub kind_supported: bool,
    pub reported: bool,
    pub lifecycle_kind: Option<String>,
    pub lifecycle_session_id: Option<String>,
    pub lifecycle_matches: bool,
    pub provisional_lifecycle: bool,
    /// Includes worker/stateless hooks; age alone cannot establish readiness.
    pub last_report_age_ms: Option<u64>,
    pub held: bool,
    pub exited: bool,
    pub agent_exited: bool,
    pub suspended: bool,
    pub sync_active: bool,
    pub excluded_from_sync: bool,
    pub command_present: bool,
    pub command_kind: Option<String>,
    pub process_matches: bool,
    pub bracketed_paste: bool,
}

impl AgentReadiness {
    pub fn status_available(&self) -> bool {
        self.reported && (self.lifecycle_matches || self.provisional_lifecycle)
    }

    pub fn prompt_supported(&self) -> bool {
        self.kind_supported
            && self.status_available()
            && !self.held
            && !self.exited
            && !self.agent_exited
            && !self.suspended
            && !(self.sync_active && self.excluded_from_sync)
            && self.process_matches
            && self.bracketed_paste
    }

    /// Return every failed capability check, including failures masked by another.
    pub fn failures(&self) -> Vec<&'static str> {
        [
            (!self.kind_supported, "unsupported_kind"),
            (!self.reported, "not_reported"),
            (
                !self.lifecycle_matches && !self.provisional_lifecycle,
                if self.lifecycle_kind.is_none() {
                    "lifecycle_missing"
                } else {
                    "lifecycle_mismatch"
                },
            ),
            (self.held, "held"),
            (self.exited, "pane_exited"),
            (self.agent_exited, "agent_exited"),
            (self.suspended, "suspended"),
            (self.sync_active && self.excluded_from_sync, "sync_excluded"),
            (
                !self.process_matches,
                if self.command_present {
                    "command_mismatch"
                } else {
                    "unmanaged"
                },
            ),
            (!self.bracketed_paste, "bracketed_paste_disabled"),
        ]
        .into_iter()
        .filter_map(|(failed, reason)| failed.then_some(reason))
        .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    #[serde(with = "pane_string")]
    pub pane_id: PaneId,
    pub session: String,
    pub host_id: crate::internal_ids::HostId,
    pub tab_id: Option<crate::internal_ids::TabId>,
    #[serde(default)]
    pub is_suppressed: bool,
    pub target: crate::command::PaneTarget,
    pub agent: AgentInfo,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub ready: bool,
    #[serde(default)]
    pub not_ready_reason: Option<NotReadyReason>,
    pub status_available: bool,
    pub prompt_supported: bool,
    /// Absent in responses from older servers.
    #[serde(default)]
    pub readiness: Option<AgentReadiness>,
    #[serde(default)]
    pub input_revision: Option<u64>,
    #[serde(default)]
    pub clear_draft_supported: bool,
    /// Claude background tasks whose `<task-notification>` turn has not finished.
    #[serde(default)]
    pub background_tasks: Vec<String>,
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
/// Codex and Muse require provider turn IDs; Claude requires prompt IDs and transcript
/// confirmation.
#[derive(Clone, Debug)]
pub struct PromptObservation {
    kind: String,
    conversation_id: String,
    prompt_sha256: String,
    claude_prompt_sha256: Option<String>,
    provider_prompt_sha256: Option<String>,
    supported: bool,
    prompt_id: Option<String>,
    acknowledged: bool,
    waiting_before_submit: bool,
    turn_id: Option<String>,
    finished: bool,
}

impl PromptObservation {
    pub fn new(kind: &str, conversation_id: &str, prompt: &str) -> Self {
        let provider = super::provider::provider(kind);
        let input = provider
            .as_ref()
            .and_then(|provider| provider.input.as_ref());
        Self::with_input(kind, conversation_id, prompt, input)
    }

    /// Observe using the same provider snapshot that prepared the terminal input.
    pub fn with_input(
        kind: &str,
        conversation_id: &str,
        prompt: &str,
        input: Option<&AgentInput>,
    ) -> Self {
        Self {
            kind: kind.to_owned(),
            conversation_id: conversation_id.to_owned(),
            prompt_sha256: if kind == "pi" {
                // Pi's editor replaces each pasted tab with four spaces.
                prompt_sha256(&pi_prompt_text(prompt).replace('\t', "    "))
            } else {
                prompt_sha256(prompt)
            },
            claude_prompt_sha256: (kind == "claude")
                .then(|| prompt_sha256(&claude_prompt_text(prompt))),
            provider_prompt_sha256: input.map(|input| {
                prompt_sha256(&format!(
                    "{}{}",
                    input.paste_prefix,
                    prompt.replace('\n', &input.newline),
                ))
            }),
            supported: builtin_prompt_supported(kind) || input.is_some(),
            prompt_id: None,
            acknowledged: false,
            waiting_before_submit: false,
            turn_id: None,
            finished: false,
        }
    }

    /// Observe a provider-acknowledged turn, including human and background turns.
    /// No prompt text is retained by lifecycle subscribers.
    pub fn from_accepted_hook(kind: &str, hook: &HookEvent) -> Self {
        let mut observation = Self::with_input(
            kind,
            &hook.session_id,
            hook.prompt.as_deref().unwrap_or_default(),
            None,
        );
        if let Some(fingerprint) = &hook.prompt_sha256 {
            observation.prompt_sha256 = fingerprint.clone();
        }
        observation.supported = true;
        observation.acknowledged = true;
        observation.turn_id = hook.turn_id.clone().filter(|id| !id.is_empty());
        observation.prompt_id = hook.prompt_id.clone().filter(|id| !id.is_empty());
        observation
    }

    /// Whether this is the exact provider turn accepted by this observation.
    pub fn acknowledges(&self, hook: &HookEvent) -> bool {
        self.acknowledged
            && self.conversation_id == hook.session_id
            && self.turn_id.as_deref() == hook.turn_id.as_deref().filter(|id| !id.is_empty())
            && self.prompt_id.as_deref() == hook.prompt_id.as_deref().filter(|id| !id.is_empty())
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    /// A verified Pi dialog shown before its submit handler has assigned a turn ID.
    pub fn waiting_before_submit(&self) -> bool {
        self.waiting_before_submit
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    /// A caller may submit literal notification XML as ordinary task text. A matching
    /// fingerprint takes precedence over recognizing a provider-generated notification.
    pub fn matches_submission(&self, hook: &HookEvent) -> bool {
        hook.hook_event_name == "UserPromptSubmit"
            && (self.conversation_id.is_empty() || hook.session_id == self.conversation_id)
            && submit_matches(
                hook,
                &self.kind,
                &self.prompt_sha256,
                &self.claude_prompt_sha256,
                &self.provider_prompt_sha256,
            )
    }

    /// `Submitted` means a matching prompt hook arrived, never merely a successful PTY write.
    /// Completion is emitted at most once. Uncertainty keeps observing valid late hooks.
    /// An old completion cannot accept a prompt.
    pub fn observe(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> Option<AgentOutcome> {
        if hook.is_some_and(|hook| hook.archive_only) {
            return None;
        }
        self.waiting_before_submit = false;
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
        if !self.supported {
            return Some(AgentOutcome::StatusUnavailable);
        }
        let Some(hook) = hook else {
            return Some(AgentOutcome::StatusUnavailable);
        };
        if hook.is_worker() {
            return (event.kind == self.kind
                && hook.session_id == self.conversation_id
                && event.session_id.as_deref() == Some(self.conversation_id.as_str())
                && super::hook_state_for(&event.kind, hook) == Some(AgentState::Waiting)
                && super::waiting_needs_a_person(hook))
            .then_some(AgentOutcome::NeedsInput);
        }
        if self.kind == "agy"
            && hook.agy_turn_complete.is_some()
            && (hook.session_id.is_empty()
                || hook.session_id != self.conversation_id
                || hook.prompt_id.as_deref().is_none_or(str::is_empty)
                || hook.prompt_id != self.prompt_id)
        {
            return None;
        }
        if hook.session_id.is_empty() || event.session_id.as_deref().is_none_or(str::is_empty) {
            return Some(AgentOutcome::StatusUnavailable);
        }
        // A Muse pane verified at startup has no conversation until its first turn
        // begins: the bootstrap that proved it idle carries no session id, because
        // the provider only speaks one once someone prompts it. The first submit
        // binds the observation the way the screen binds the lifecycle; a start
        // heard before it is that same turn arriving, not a replacement.
        if self.kind == "muse"
            && self.conversation_id.is_empty()
            && !self.acknowledged
            && event.kind == self.kind
            && event.session_id.as_deref() == Some(hook.session_id.as_str())
        {
            if hook.hook_event_name == "SessionStart" && hook.source.as_deref() != Some("compact") {
                return None;
            }
            if hook.hook_event_name == "UserPromptSubmit" {
                if !submit_matches(
                    hook,
                    self.kind.as_str(),
                    &self.prompt_sha256,
                    &self.claude_prompt_sha256,
                    &self.provider_prompt_sha256,
                ) {
                    return Some(AgentOutcome::Interrupted);
                }
                if hook.turn_id.clone().filter(|id| !id.is_empty()).is_none() {
                    return Some(AgentOutcome::StatusUnavailable);
                }
                self.conversation_id = hook.session_id.clone();
            }
        }
        if event.kind != self.kind
            || hook.session_id != self.conversation_id
            || event.session_id.as_deref() != Some(self.conversation_id.as_str())
            || hook.hook_event_name == "SessionEnd"
        {
            return Some(AgentOutcome::TargetChanged);
        }
        if hook.hook_event_name == "SessionStart" && hook.source.as_deref() != Some("compact") {
            // A repeat start of this conversation is not a replacement until a turn is accepted:
            // Codex defers its own SessionStart until the first turn begins, after Yinz announced
            // the conversation, and Claude repeats hooks once per hook registration.
            return self.acknowledged.then_some(AgentOutcome::TargetChanged);
        }
        if is_claude_task_notification(&self.kind, hook) && !self.matches_submission(hook) {
            // Claude starts these turns itself when background work finishes, possibly before
            // this prompt's own submission or confirmation arrives. They carry no caller input.
            return None;
        }
        if hook.hook_event_name == "UserPromptSubmit" {
            if !submit_matches(
                hook,
                self.kind.as_str(),
                &self.prompt_sha256,
                &self.claude_prompt_sha256,
                &self.provider_prompt_sha256,
            ) {
                return Some(AgentOutcome::Interrupted);
            }
            let turn_id = hook.turn_id.clone().filter(|id| !id.is_empty());
            let prompt_id = hook.prompt_id.clone().filter(|id| !id.is_empty());
            if (!matches!(self.kind.as_str(), "claude" | "agy") && turn_id.is_none())
                || (matches!(self.kind.as_str(), "claude" | "agy") && prompt_id.is_none())
            {
                return Some(AgentOutcome::StatusUnavailable);
            }
            if self.acknowledged {
                return (turn_id != self.turn_id || prompt_id != self.prompt_id)
                    .then_some(AgentOutcome::Interrupted);
            }
            self.acknowledged = true;
            self.turn_id = turn_id;
            self.prompt_id = prompt_id;
            return Some(AgentOutcome::Submitted);
        }
        if !self.acknowledged {
            self.waiting_before_submit = self.kind == "pi"
                && hook.hook_event_name == "Elicitation"
                && hook.turn_id.is_none();
            return self
                .waiting_before_submit
                .then_some(AgentOutcome::NeedsInput);
        }
        if self.kind == "pi" && hook.turn_id != self.turn_id {
            return None;
        }
        // A human wait in this conversation ends the wait even without turn
        // identity: Muse's approval notification is session-scoped, and a pane
        // blocked on a person is blocked whatever turn the provider names.
        // Completions below still require the acknowledged turn.
        if super::hook_state_for(&event.kind, hook) == Some(AgentState::Waiting)
            && super::waiting_needs_a_person(hook)
        {
            return Some(AgentOutcome::NeedsInput);
        }
        if !matches!(self.kind.as_str(), "claude" | "agy")
            && self
                .turn_id
                .as_deref()
                .is_some_and(|turn| hook.turn_id.as_deref() != Some(turn))
        {
            return None;
        }
        match hook.hook_event_name.as_str() {
            "Stop" if self.kind == "claude" => {
                if hook.prompt_id != self.prompt_id {
                    return None;
                }
                hook.claude_turn_complete.map(|complete| {
                    if complete {
                        AgentOutcome::Completed
                    } else {
                        AgentOutcome::StatusUnavailable
                    }
                })
            },
            "Stop" if self.kind == "agy" => {
                if hook.prompt_id != self.prompt_id {
                    return None;
                }
                hook.agy_turn_complete.map(|complete| {
                    if complete {
                        AgentOutcome::Completed
                    } else {
                        AgentOutcome::StatusUnavailable
                    }
                })
            },
            "Stop" => Some(AgentOutcome::Completed),
            "StopFailure" => Some(AgentOutcome::Failed),
            "Interrupt" => Some(AgentOutcome::Interrupted),
            _ => None,
        }
    }
}

/// Whether a submit hook carries the prompt under observation: its fingerprint, or the
/// prompt itself hashed the same way, matches what was sent. One reading for the pinned
/// acknowledgement and the provisional Muse bind, so the two cannot disagree.
fn submit_matches(
    hook: &HookEvent,
    kind: &str,
    expected: &str,
    claude_expected: &Option<String>,
    provider_expected: &Option<String>,
) -> bool {
    let fingerprint = hook
        .prompt_sha256
        .clone()
        .or_else(|| hook.prompt.as_deref().map(prompt_sha256));
    fingerprint.as_deref() == Some(expected)
        || (kind == "claude" && fingerprint == *claude_expected)
        || (provider_expected.is_some() && fingerprint == *provider_expected)
}

/// Fingerprint of a prompt as the agent received it: CRLF normalized, then
/// [`submitted_prompt`].
pub fn prompt_sha256(prompt: &str) -> String {
    let prompt = prompt.replace("\r\n", "\n");
    Sha256::digest(submitted_prompt(&prompt).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Claude 2.1.285 strips these format controls; emoji joiners remain significant.
pub fn claude_prompt_text(text: &str) -> Cow<'_, str> {
    if text.contains(['\u{200b}', '\u{202e}']) {
        Cow::Owned(text.replace(['\u{200b}', '\u{202e}'], ""))
    } else {
        Cow::Borrowed(text)
    }
}

/// Pi submits JavaScript-trimmed text: unlike Rust, it trims BOM but not NEXT LINE.
pub fn pi_prompt_text(text: &str) -> &str {
    text.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{0085}'))
}

/// Whether `prompt` is exactly the command that starts a new conversation in place: Claude's
/// `/clear` or Codex's `/new`. Its lifecycle carries the submission to the new conversation.
pub fn is_conversation_reset(kind: &str, prompt: &str) -> bool {
    matches!(
        (kind, prompt),
        ("claude", "/clear") | ("codex" | "pi", "/new")
    )
}

/// Whether the provider declares the input and hook correlation needed for safe prompting.
pub fn supports_prompt(kind: &str) -> bool {
    builtin_prompt_supported(kind)
        || super::provider::provider(kind).is_some_and(|provider| provider.input.is_some())
}

fn builtin_prompt_supported(kind: &str) -> bool {
    matches!(kind, "claude" | "codex" | "muse" | "agy" | "pi")
}

/// Commands do not produce the lifecycle acknowledgement required by prompt submission.
pub fn is_command_prompt(kind: &str, prompt: &str) -> bool {
    if is_conversation_reset(kind, prompt) {
        return false;
    }
    let text = if kind == "claude" {
        claude_prompt_text(prompt)
    } else {
        Cow::Borrowed(prompt)
    };
    let text = if kind == "pi" {
        pi_prompt_text(&text)
    } else {
        text.trim_start()
    };
    if let Some(provider) = super::provider::provider(kind)
        && let Some(input) = &provider.input
    {
        return input.is_command_prompt(text);
    }
    text.starts_with('/') || (matches!(kind, "claude" | "pi") && text.starts_with('!'))
}

const TASK_NOTIFICATION_OPEN: &str = "<task-notification>";

/// The task IDs of a turn Claude starts by itself when background work finishes. Its
/// `UserPromptSubmit.prompt` is one or more `<task-notification>` elements, each naming the
/// `<task-id>` of the Bash command or subagent that finished. `None` for any other prompt.
pub fn claude_task_notification_ids(prompt: &str) -> Option<Vec<String>> {
    let text = prompt.trim_start();
    if !text.starts_with(TASK_NOTIFICATION_OPEN) {
        return None;
    }
    let ids = text
        .split(TASK_NOTIFICATION_OPEN)
        .filter_map(|element| {
            let id = element
                .split_once("<task-id>")?
                .1
                .split_once("</task-id>")?
                .0;
            let id = id.trim();
            (!id.is_empty()).then(|| id.to_owned())
        })
        .collect();
    Some(ids)
}

/// Whether a hook is the submission of a Claude `<task-notification>` turn.
pub fn is_claude_task_notification(kind: &str, hook: &HookEvent) -> bool {
    kind == "claude"
        && hook.hook_event_name == "UserPromptSubmit"
        && hook
            .prompt
            .as_deref()
            .is_some_and(|prompt| claude_task_notification_ids(prompt).is_some())
}

/// Typed ahead of an ordinary paste Claude may collapse, to identify its body as instructions.
pub const PASTE_LEAD_IN: &str = "Follow the instructions in the pasted text:";

/// Whether a prompt is typed behind [`PASTE_LEAD_IN`]. Command prefixes must stay at the start
/// of the input. The cutoff is loose on purpose: an inline paste still reads correctly.
pub fn needs_paste_lead_in(kind: &str, prompt: &str) -> bool {
    kind == "claude"
        && !prompt.starts_with(['/', '!', '#'])
        && (prompt.trim().contains('\n') || prompt.len() >= 500)
}

/// The prompt a submission carried, without pasted-content wrappers, the lead-in typed ahead of
/// them, or outer whitespace.
pub fn submitted_prompt(text: &str) -> Cow<'_, str> {
    fn strip(text: &str) -> &str {
        let text = text.trim();
        text.strip_prefix(PASTE_LEAD_IN).map_or(text, str::trim)
    }
    match unwrap_pasted_content(text) {
        Cow::Borrowed(text) => Cow::Borrowed(strip(text)),
        Cow::Owned(text) => Cow::Owned(strip(&text).to_owned()),
    }
}

/// The part of a pasted-content tag both the opener and the closer spell.
const PASTED_CONTENT_NAME: &str = "pasted_content id=\"";

/// One `<pasted_content id="X">` or `</pasted_content id="X">` in a prompt.
struct PastedContentTag<'a> {
    start: usize,
    end: usize,
    id: &'a str,
    close: bool,
}

/// Every well-formed pasted-content tag in `text`, in order. One pass: an id ends at the first
/// quote, angle bracket or newline, so no two tags' scans overlap.
fn pasted_content_tags(text: &str) -> Vec<PastedContentTag<'_>> {
    let mut tags = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(PASTED_CONTENT_NAME) {
        let name = from + found;
        from = name + PASTED_CONTENT_NAME.len();
        let (start, close) = if text[..name].ends_with("</") {
            (name - 2, true)
        } else if text[..name].ends_with('<') {
            (name - 1, false)
        } else {
            continue;
        };
        let rest = &text[from..];
        let Some(id_len) = rest.find(['"', '<', '>', '\n']) else {
            continue;
        };
        if id_len == 0 || !rest[id_len..].starts_with("\">") {
            continue;
        }
        tags.push(PastedContentTag {
            start,
            end: from + id_len + 2,
            id: &rest[..id_len],
            close,
        });
    }
    tags
}

/// Claude collapses a long paste into a chip and reports it in `UserPromptSubmit.prompt` as
/// `<pasted_content id="X">\nBODY\n</pasted_content id="X">`. This replaces each such element
/// with its body so the prompt reads as what was pasted. An opener pairs with the first closer
/// with the same id; one without a closer, or whose element would cross its enclosing one, is
/// left as written.
///
/// Elements inside a body are unwrapped too. Claude's wrapper and one the user typed look the
/// same, so only a canonical form — every element unwrapped — fingerprints a typed prompt and
/// the hooked copy Claude wrapped around it alike. Linear in the text: it runs on the screen
/// thread for every prompt hook.
pub fn unwrap_pasted_content(text: &str) -> Cow<'_, str> {
    if !text.contains(PASTED_CONTENT_NAME) {
        return Cow::Borrowed(text);
    }
    let tags = pasted_content_tags(text);
    let mut closers: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for (index, tag) in tags.iter().enumerate().filter(|(_, tag)| tag.close) {
        closers.entry(tag.id).or_default().push_back(index);
    }
    // (closer index, output length where the body starts) per open element.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut unwrapped = String::new();
    let mut cursor = 0;
    for (index, tag) in tags.iter().enumerate() {
        if tag.close {
            let Some((_, body_start)) = open.pop_if(|&mut (closer, _)| closer == index) else {
                continue;
            };
            unwrapped.push_str(&text[cursor..tag.start]);
            if unwrapped.len() > body_start && unwrapped.ends_with('\n') {
                unwrapped.pop();
            }
            cursor = tag.end;
            continue;
        }
        let Some(queue) = closers.get_mut(tag.id) else {
            continue;
        };
        while queue.front().is_some_and(|closer| *closer < index) {
            queue.pop_front();
        }
        let Some(&closer) = queue.front() else {
            continue;
        };
        if open.last().is_some_and(|(outer, _)| closer >= *outer) {
            continue;
        }
        unwrapped.push_str(&text[cursor..tag.start]);
        cursor = tag.end;
        if text[cursor..].starts_with('\n') {
            cursor += 1;
        }
        open.push((closer, unwrapped.len()));
    }
    if cursor == 0 {
        return Cow::Borrowed(text);
    }
    unwrapped.push_str(&text[cursor..]);
    Cow::Owned(unwrapped)
}

#[cfg(test)]
#[path = "unit/control_tests.rs"]
mod tests;
