//! The Lab's live change feed, `GET /api/v1/lab/stream`.
//!
//! Events carry ids only: re-fetch the issue to learn what changed. The feed
//! is fanned out from one server process's memory, so behind a load balancer
//! a subscriber hears only about the writes that landed on its replica. Treat
//! it as a hint to refresh sooner, never as the only way to notice a change.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    IssueCreated {
        issue_number: u64,
    },
    IssueUpdated {
        issue_number: u64,
    },
    IssueDeleted {
        issue_number: u64,
    },
    CommentAdded {
        issue_number: u64,
        comment_id: String,
    },
    CommentUpdated {
        issue_number: u64,
        comment_id: String,
    },
    CommentDeleted {
        issue_number: u64,
        comment_id: String,
    },
    LifecycleAdded {
        issue_number: u64,
        entry_id: String,
    },
    /// An event type this crate does not know, with its issue when it named one.
    Other {
        kind: String,
        issue_number: Option<u64>,
    },
}

impl StreamEvent {
    pub fn issue_number(&self) -> Option<u64> {
        match self {
            StreamEvent::IssueCreated { issue_number }
            | StreamEvent::IssueUpdated { issue_number }
            | StreamEvent::IssueDeleted { issue_number }
            | StreamEvent::CommentAdded { issue_number, .. }
            | StreamEvent::CommentUpdated { issue_number, .. }
            | StreamEvent::CommentDeleted { issue_number, .. }
            | StreamEvent::LifecycleAdded { issue_number, .. } => Some(*issue_number),
            StreamEvent::Other { issue_number, .. } => *issue_number,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    #[serde(rename = "type")]
    kind: String,
    issue_number: Option<u64>,
    comment_id: Option<String>,
    entry_id: Option<String>,
}

/// Reassembles events from chunks that split wherever the transport did.
#[derive(Default)]
pub struct Parser {
    pending: Vec<u8>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk; get back every event it completed. Heartbeats, comments
    /// and frames that are not JSON are dropped.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, separator)) = frame_end(&self.pending) {
            let frame: Vec<u8> = self.pending.drain(..end + separator).take(end).collect();
            if let Some(event) = decode_frame(&String::from_utf8_lossy(&frame)) {
                events.push(event);
            }
        }
        events
    }
}

/// Where the first frame ends, and how long its blank-line separator is.
fn frame_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find(buf, b"\n\n").map(|i| (i, 2));
    let crlf = find(buf, b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn decode_frame(frame: &str) -> Option<StreamEvent> {
    // Multiple `data:` lines in one frame are one payload joined by newlines.
    let data: Vec<&str> = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
        .collect();
    if data.is_empty() {
        return None;
    }
    let wire: Wire = serde_json::from_str(&data.join("\n")).ok()?;
    let issue_number = wire.issue_number;
    let with_issue = |make: fn(u64) -> StreamEvent| issue_number.map(make);
    match wire.kind.as_str() {
        "ping" => None,
        "issue_created" => with_issue(|issue_number| StreamEvent::IssueCreated { issue_number }),
        "issue_updated" => with_issue(|issue_number| StreamEvent::IssueUpdated { issue_number }),
        "issue_deleted" => with_issue(|issue_number| StreamEvent::IssueDeleted { issue_number }),
        "comment_added" | "comment_updated" | "comment_deleted" => {
            let (issue_number, comment_id) = (issue_number?, wire.comment_id?);
            Some(match wire.kind.as_str() {
                "comment_added" => StreamEvent::CommentAdded {
                    issue_number,
                    comment_id,
                },
                "comment_updated" => StreamEvent::CommentUpdated {
                    issue_number,
                    comment_id,
                },
                _ => StreamEvent::CommentDeleted {
                    issue_number,
                    comment_id,
                },
            })
        },
        "lifecycle_added" => Some(StreamEvent::LifecycleAdded {
            issue_number: issue_number?,
            entry_id: wire.entry_id?,
        }),
        _ => Some(StreamEvent::Other {
            kind: wire.kind,
            issue_number,
        }),
    }
}

#[cfg(test)]
#[path = "unit/sse.rs"]
mod tests;
