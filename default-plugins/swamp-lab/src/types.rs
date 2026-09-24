//! The Lab's data, mirroring swamp-club's `lib/domain/lab/*` as it crosses the
//! wire (camelCase, dates as ISO strings).
//!
//! Decoding is lenient on purpose. An enum the server grows a value for lands in
//! `Other` instead of failing the whole list, every optional field defaults, and
//! fields this crate does not know are ignored. What the Lab calls a type is
//! `kind` here, because `type` is a keyword.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

macro_rules! wire_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant,)+
            /// A value this crate does not know yet.
            #[serde(untagged)]
            Other(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $($name::$variant => $wire,)+
                    $name::Other(value) => value,
                }
            }

            /// Every value this crate knows, in the server's order.
            pub const KNOWN: &'static [$name] = &[$($name::$variant),+];

            pub fn parse(value: &str) -> Option<Self> {
                Self::KNOWN.iter().find(|known| known.as_str() == value).cloned()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

wire_enum!(IssueType {
    Feature => "feature",
    Bug => "bug",
    Security => "security",
    Platform => "platform",
});

wire_enum!(IssueStatus {
    Open => "open",
    Triaged => "triaged",
    InProgress => "in_progress",
    Shipped => "shipped",
    Closed => "closed",
});

wire_enum!(RelationshipType {
    ParentOf => "parent_of",
    BlockedBy => "blocked_by",
    RelatedTo => "related_to",
});

wire_enum!(Visibility {
    Public => "public",
    Collective => "collective",
    AuthorAndAdmins => "author_and_admins",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignee {
    pub user_id: String,
    pub username: String,
}

/// `LabIssueData`. A list returns these without `body`, which then reads as
/// empty; [`Issue::body`] is only meaningful from a detail or a write.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    #[serde(default)]
    pub id: String,
    pub number: u64,
    #[serde(rename = "type")]
    pub kind: IssueType,
    pub status: IssueStatus,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author_id: String,
    #[serde(default)]
    pub author_username: String,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_collective_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_collective_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_collective_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_issue_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_repo_full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_author_login: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<Assignee>,
    #[serde(default)]
    pub comment_locked: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub blocked_manually: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_at: Option<String>,
    #[serde(default)]
    pub is_regression: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regression_introduced_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_in_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped_at: Option<String>,
    /// ISO 8601. Anything that is not a string reads as unknown rather than
    /// failing the issue, and with it the list it came in.
    #[serde(default, deserialize_with = "lenient_string")]
    pub created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub updated_at: Option<String>,
}

fn lenient_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        _ => None,
    })
}

/// `EffectiveAudience`: who can see an issue, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audience {
    pub visibility: Visibility,
    #[serde(default)]
    pub reasons: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collective: Option<Collective>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collective {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `LabCommentData`, with the server-rendered HTML the API adds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub issue_id: String,
    #[serde(default)]
    pub author_id: String,
    #[serde(default)]
    pub author_username: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_comment_id: Option<u64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
}

/// `LabLifecycleEntry`: one step of the triage pipeline's record on an issue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEntry {
    pub id: String,
    #[serde(default)]
    pub issue_id: String,
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub actor_username: String,
    pub target_status: IssueStatus,
    pub step: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub payload: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub is_verbose: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// This issue is the relationship's source (it is the parent, or it is blocked).
    Outgoing,
    Incoming,
}

/// `IssueRelationshipData`. `direction` is present when the server read the
/// relationship from one issue's point of view (a detail, or a listing).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: RelationshipType,
    #[serde(default)]
    pub source_issue_id: String,
    pub source_issue_number: u64,
    #[serde(default)]
    pub target_issue_id: String,
    pub target_issue_number: u64,
    #[serde(default)]
    pub created_by_user_id: String,
    #[serde(default)]
    pub created_by_username: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
}

/// `IssueRelationshipSummary`, the counts a list shows beside each issue.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RelationshipSummary {
    pub parent_issue_number: Option<u64>,
    pub child_count: u64,
    pub sibling_count: u64,
    pub blocked_by_count: u64,
    pub blocks_count: u64,
}

/// `LabEventData`: a change recorded on an issue's timeline.
///
/// The fields every event shares are here; the rest of each variant is kept
/// as it came in `detail`, since what differs is only ever displayed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub issue_id: String,
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub actor_username: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(flatten)]
    pub detail: BTreeMap<String, Value>,
}

/// `GET /issues`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub issues: Vec<Issue>,
    /// The count before the server dropped the issues this caller cannot see,
    /// so it can exceed what paging through will ever return.
    pub total: u64,
    #[serde(default)]
    pub tier_by_username: BTreeMap<String, Value>,
    /// Keyed by issue id, not number.
    #[serde(default)]
    pub relationship_summaries: BTreeMap<String, RelationshipSummary>,
}

/// `GET /issues/{n}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub issue: Issue,
    pub audience: Audience,
    #[serde(default)]
    pub tier_by_username: BTreeMap<String, Value>,
    #[serde(default)]
    pub body_html: String,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub lifecycle_entries: Vec<LifecycleEntry>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub can_edit: bool,
    /// Empty for an anonymous caller.
    #[serde(default)]
    pub current_user_id: String,
}

/// `POST /issues`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Created {
    pub issue: Issue,
    pub audience: Audience,
}

/// `PATCH /issues/{n}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Updated {
    pub issue: Issue,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
}

/// `GET /issues/{n}/relationships`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationships {
    pub relationships: Vec<Relationship>,
}

/// `POST /issues/{n}/relationships`. Relating two issues that are already
/// related succeeds with no events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Related {
    pub relationship: Relationship,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// `PATCH /issues/{n}/audience`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudienceChanged {
    pub audience: Audience,
    pub issue: Issue,
}

/// `POST /issues/ensure`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ensured {
    pub issue: Issue,
    /// False when the GitHub issue already had a Lab issue.
    pub created: bool,
}

/// `GET /assignees`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignees {
    pub assignees: Vec<Assignee>,
}
