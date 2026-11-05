//! Pane-owned agent tracking transferred between sessions. Live requests never travel.
use super::control::{AgentOutcome, PromptDelivery, PromptObservation};
use super::{AgentInfo, SpawnedAgent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A snapshot taken on the screen thread immediately before removing a pane.
/// Revisions are relative to `input_revision`; monotonic clocks restart on receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTrackingHandoff {
    pub agent: AgentInfo,
    pub input_revision: u64,
    /// Consumption can be unknown even when the source composer revision is zero.
    pub input_consumed: Option<bool>,
    pub lifecycle: Option<LifecycleState>,
    pub dialog_input: Option<DialogInput>,
    pub worker_waits: Option<WorkerWaits>,
    pub permission_wait: Option<PermissionWait>,
    pub launch: Option<SpawnedAgent>,
    pub submissions: VecDeque<Submission>,
    pub events: EventTrackingHandoff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerWaits {
    pub parent: AgentInfo,
    pub waits: std::collections::BTreeMap<String, Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionWait {
    pub conversation: String,
    pub input_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleState {
    pub kind: String,
    pub conversation: String,
    pub claude: ClaudeActivity,
    pub input_revision: Option<u64>,
    /// The provider turn last reported as interrupted or failed, until the next prompt. Codex
    /// finishes a cancelled turn's tool call after its `Interrupt`, and that hook must not make
    /// the idle agent busy again. `Stop` is not an end here: a blocking Stop hook continues
    /// the same turn.
    pub ended_turn: Option<String>,
    pub claude_prompt_id: Option<String>,
    pub claude_confirmation: Option<bool>,
    pub claude_previous_prompt_id: Option<String>,
    pub agy_confirmation: Option<bool>,
    /// Claude background tasks launched by confirmed turns whose `<task-notification>` turn
    /// has not been confirmed yet.
    pub background_tasks: BTreeSet<String>,
    /// Claude `<task-notification>` turns in flight, by prompt ID. They run beside the
    /// caller's prompt tracking, which they never replace.
    pub claude_notifications: BTreeMap<String, ClaudeNotification>,
    /// The final assistant text of the last completed turn, until the next caller prompt.
    pub last_message: Option<String>,
}

/// Parent presentation, workers, and retained turn confirmations have separate lifetimes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClaudeActivity {
    pub parent: Option<AgentInfo>,
    pub workers: BTreeSet<String>,
    pub stopped_workers: BTreeSet<String>,
    /// Raw starts for known workers cannot distinguish a duplicate from an unobserved
    /// resumption. Only explicit recovery or conversation replacement clears this evidence.
    #[serde(default)]
    pub provisional_workers: BTreeSet<String>,
    /// Tool identity of each worker's visible input wait. Missing identity never permits
    /// an unrelated tool event to release the wait, including after execution completion.
    #[serde(default)]
    pub worker_wait_tools: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub uncorrelated_worker_waits: BTreeSet<String>,
    /// Released tool calls suppress reordered waiting hooks across prompts and transfers.
    #[serde(default)]
    pub released_worker_tools: BTreeMap<String, BTreeSet<String>>,
    pub turns: BTreeMap<String, ClaudeTurn>,
    pub completed_tasks: BTreeSet<String>,
    /// Executions indexed by task ID and causal tool call. Completion tombstones
    /// survive ordinary prompts and pane transfer, until conversation replacement.
    #[serde(default)]
    pub executions: BTreeMap<String, BTreeMap<String, Option<String>>>,
    /// Explicit recovery abandons executions without claiming successful completion.
    #[serde(default)]
    pub abandoned_executions: BTreeSet<String>,
    pub sequence: u64,
    pub message_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeTurn {
    pub confirmation: Option<bool>,
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeNotification {
    /// The tasks the notification reports finished.
    pub tasks: Vec<String>,
    pub waiting: Option<AgentInfo>,
    pub permission_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogInput {
    pub kind: String,
    pub conversation: Option<String>,
    pub input_revision: u64,
    pub startup: bool,
    pub fresh_waiting: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    #[serde(default)]
    pub prompt_delivery: Option<PromptDelivery>,
    pub id: String,
    pub kind: String,
    pub conversation: Option<String>,
    pub acknowledged: bool,
    pub turn_id: Option<String>,
    pub result: Option<SubmissionResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionResult {
    pub outcome: AgentOutcome,
    pub message: Option<String>,
    pub background_tasks: Vec<String>,
}

/// Correlation for the destination's lifecycle feed. Subscribers, cursors, and the
/// source session's event journal remain local to the source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTrackingHandoff {
    pub turns: VecDeque<ObservedAgentTurn>,
    pub uncertain: Vec<(Option<String>, AgentOutcome)>,
    pub submission_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedAgentTurn {
    pub identity: (Option<String>, Option<String>),
    pub observation: PromptObservation,
    pub submission: Option<String>,
    pub last: Option<AgentOutcome>,
    pub ended: bool,
}
