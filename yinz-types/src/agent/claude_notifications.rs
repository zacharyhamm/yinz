//! Internal, bounded positive facts for the pinned local Claude root-notification adapter.
use serde::{Deserialize, Serialize};

pub const MAX_FACTS: usize = 64;
pub const MAX_ID_BYTES: usize = 256;

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:./".contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub pid: u32,
    pub started: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Bash,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub task: String,
    pub tool: String,
    pub prompt: String,
    pub kind: TaskKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub task: String,
    pub tool: String,
    pub run: String,
    pub prompt: String,
    pub row: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancellation {
    pub task: String,
    pub prompt: String,
    pub tool: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub kind: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub launches: Vec<Launch>,
    #[serde(default)]
    pub notice: Option<Notice>,
    #[serde(default)]
    pub cancellation: Option<Cancellation>,
    #[serde(default)]
    pub tasks: Vec<TaskSnapshot>,
    /// Scoped by the containing hook's prompt, or source-wide if it has no prompt.
    #[serde(default)]
    pub unavailable: bool,
    /// Positively detected unsupported resumed task; never a new root obligation.
    #[serde(default)]
    pub unsupported_tasks: Vec<String>,
}

impl Evidence {
    pub fn bounded(&self) -> Self {
        let valid = self.launches.len() <= MAX_FACTS
            && self.tasks.len() <= MAX_FACTS
            && self.unsupported_tasks.len() <= MAX_FACTS
            && self.launches.iter().all(|l| {
                [&l.task, &l.tool, &l.prompt]
                    .into_iter()
                    .all(|s| valid_id(s))
            })
            && self
                .tasks
                .iter()
                .all(|t| [&t.id, &t.kind, &t.status].into_iter().all(|s| valid_id(s)))
            && self.unsupported_tasks.iter().all(|s| valid_id(s))
            && self.notice.as_ref().is_none_or(|n| {
                [&n.task, &n.tool, &n.run, &n.prompt, &n.row]
                    .into_iter()
                    .all(|s| valid_id(s))
            })
            && self.cancellation.as_ref().is_none_or(|c| {
                [&c.task, &c.prompt, &c.tool]
                    .into_iter()
                    .all(|s| valid_id(s))
            });
        if valid {
            self.clone()
        } else {
            Self {
                source: self.source.clone(),
                unavailable: true,
                ..Self::default()
            }
        }
    }
}

/// Positive facts survive screen transfer. No unresolved identity is evicted to make room.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub source: Option<Source>,
    pub launches: Vec<Obligation>,
    pub notices: Vec<Notice>,
    #[serde(default)]
    pub cancellations: Vec<Cancellation>,
    pub answers: Vec<Answer>,
    pub disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    pub launch: Launch,
    pub answer_prompt: Option<String>,
    pub remaining_ms: u64,
    pub settled: bool,
    pub unavailable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub prompt: String,
    pub sequence: u64,
    pub text: Option<String>,
    /// An accepted notification may confirm before its structural notice arrives.
    /// Preserve that dependency even after the active notification lifetime ends.
    #[serde(default)]
    pub notification: bool,
}

impl Pending {
    pub fn active(&self) -> bool {
        self.launches.iter().any(|l| !l.settled && !l.unavailable)
    }
    pub fn elapse(&mut self, ms: u64) {
        for launch in &mut self.launches {
            launch.remaining_ms = launch.remaining_ms.saturating_sub(ms);
            if launch.remaining_ms == 0 && !launch.settled {
                launch.unavailable = true;
            }
        }
    }
}
