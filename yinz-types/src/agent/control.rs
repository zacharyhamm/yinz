//! Direct CLI control contracts and correlation of one submitted prompt with its hooks.

use super::{AgentEvent, AgentInfo, AgentSpawnSpec, AgentState, HookEvent, SpawnedAgent};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum_macros::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum NotReadyReason {
    PromptUnsupported,
    NotIdle,
    PendingSubmission,
    UnconsumedInput,
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
    #[serde(default)]
    pub not_ready_reason: Option<NotReadyReason>,
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
/// Codex requires provider turn IDs; Claude requires prompt IDs and transcript confirmation.
#[derive(Clone, Debug)]
pub struct PromptObservation {
    kind: String,
    conversation_id: String,
    prompt_sha256: String,
    claude_prompt_sha256: Option<String>,
    prompt_id: Option<String>,
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
            claude_prompt_sha256: (kind == "claude")
                .then(|| prompt_sha256(&claude_prompt_text(prompt))),
            prompt_id: None,
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
            // A repeat start of this conversation is not a replacement until a turn is accepted:
            // Codex defers its own SessionStart until the first turn begins, after Yinz announced
            // the conversation, and Claude repeats hooks once per hook registration.
            return self.acknowledged.then_some(AgentOutcome::TargetChanged);
        }
        if hook.hook_event_name == "UserPromptSubmit" {
            let fingerprint = hook
                .prompt_sha256
                .clone()
                .or_else(|| hook.prompt.as_deref().map(prompt_sha256));
            if fingerprint.as_deref() != Some(self.prompt_sha256.as_str())
                && !(self.kind == "claude" && fingerprint == self.claude_prompt_sha256)
            {
                return Some(AgentOutcome::Interrupted);
            }
            let turn_id = hook.turn_id.clone().filter(|id| !id.is_empty());
            let prompt_id = hook.prompt_id.clone().filter(|id| !id.is_empty());
            if (self.kind == "codex" && turn_id.is_none())
                || (self.kind == "claude" && prompt_id.is_none())
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
        if !self.acknowledged
            || (self.kind != "claude"
                && self
                    .turn_id
                    .as_deref()
                    .is_some_and(|turn| hook.turn_id.as_deref() != Some(turn)))
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

/// Commands do not produce the lifecycle acknowledgement required by prompt submission.
pub fn is_command_prompt(kind: &str, prompt: &str) -> bool {
    if kind == "claude" && prompt == "/clear" {
        return false;
    }
    let text = if kind == "claude" {
        claude_prompt_text(prompt)
    } else {
        Cow::Borrowed(prompt)
    };
    let text = text.trim_start();
    text.starts_with('/') || (kind == "claude" && text.starts_with('!'))
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
