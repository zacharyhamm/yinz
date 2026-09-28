//! The `swamp_issue_*` tools: the swamp-club Lab API, called as the host's
//! `swamp auth login` (or `SWAMP_API_KEY`).
//!
//! A call takes two host round trips. The login is read fresh each time
//! (`auth.json` through `sh`, the only thing that needs `RunCommands`), so a
//! login or logout since the last call is always honoured; then the request
//! goes out as a buffered `web_request`. Both replies carry the call's
//! `tool_call_id`, and `Status` says which of the two the head is waiting on.
//!
//! Writes are redacted exactly as the `swamp` CLI redacts them, before the
//! confirmation is drawn, so what the user approves is what is posted.

use crate::tools::ToolCall;
use crate::{State, Status};
use hocus_proto::swamp::swamp_lab::{
    self,
    api::{NewIssue, SearchQuery},
    redact, types as lab, Client, Credentials,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yinz_tile::prelude::*;

pub const SEARCH: &str = "swamp_issue_search";
pub const GET: &str = "swamp_issue_get";
pub const CREATE: &str = "swamp_issue_create";
pub const COMMENT: &str = "swamp_issue_comment";

/// The largest page a search asks for. The API allows 200; a hundred issue
/// summaries is already more than a model reads closely.
pub const MAX_LIMIT: u32 = 100;

/// A validated call, with anything it will post already redacted.
#[derive(Debug)]
pub enum Call {
    Search(SearchQuery),
    Get(u64),
    Create {
        issue: NewIssue,
        redacted: redact::Summary,
    },
    Comment {
        number: u64,
        body: String,
        redacted: redact::Summary,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    query: Option<String>,
    r#type: Option<String>,
    status: Option<String>,
    source: Option<String>,
    author: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    number: Option<u64>,
    title: Option<String>,
    body: Option<String>,
}

pub fn parse(name: &str, arguments: &str) -> Result<Call, String> {
    let args: Args = serde_json::from_str(if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    })
    .map_err(|e| e.to_string())?;
    let number = || {
        args.number
            .filter(|n| *n > 0)
            .ok_or_else(|| "number must be a positive integer".to_owned())
    };
    let text = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(source) = text(&args.source) {
        swamp_lab::api::valid_source(&source).map_err(|e| e.to_string())?;
    }
    match name {
        SEARCH => {
            let limit = args.limit.unwrap_or(MAX_LIMIT);
            if !(1..=MAX_LIMIT).contains(&limit) {
                return Err(format!("limit must be between 1 and {MAX_LIMIT}"));
            }
            let kind = match text(&args.r#type) {
                Some(kind) => Some(
                    lab::IssueType::parse(&kind)
                        .filter(|kind| *kind != lab::IssueType::Platform)
                        .ok_or_else(|| format!("invalid type: {kind}"))?,
                ),
                None => None,
            };
            let status = match text(&args.status) {
                Some(status) => Some(
                    lab::IssueStatus::parse(&status)
                        .ok_or_else(|| format!("invalid status: {status}"))?,
                ),
                None => None,
            };
            let query = text(&args.query);
            if query.as_deref().is_some_and(|q| q.contains('\0')) {
                return Err("query must not contain NUL".into());
            }
            Ok(Call::Search(SearchQuery {
                text: query,
                kind,
                status,
                source: text(&args.source),
                author: text(&args.author),
                shipped_since: None,
                limit,
                offset: args.offset.unwrap_or(0),
            }))
        },
        GET => Ok(Call::Get(number()?)),
        CREATE => {
            let kind = text(&args.r#type).ok_or("type is required: bug, feature or security")?;
            let kind = lab::IssueType::parse(&kind)
                .filter(|kind| *kind != lab::IssueType::Platform)
                .ok_or_else(|| format!("invalid type: {kind}"))?;
            let title = text(&args.title).ok_or("title is required")?;
            let body = text(&args.body).ok_or("body is required")?;
            let redacted = redact::redact_title_and_body(&title, &body);
            Ok(Call::Create {
                issue: NewIssue {
                    kind,
                    title: redacted.title.text,
                    body: redacted.body.text,
                    source: text(&args.source),
                },
                redacted: redacted.summary,
            })
        },
        COMMENT => {
            let number = number()?;
            let body = text(&args.body).ok_or("body is required")?;
            let redacted = redact::redact(&body);
            Ok(Call::Comment {
                number,
                body: redacted.text,
                redacted: redacted.summary,
            })
        },
        other => Err(format!("unknown Swamp tool: {other}")),
    }
}

impl Call {
    fn request(&self, credentials: &Credentials) -> Result<swamp_lab::Request, swamp_lab::Error> {
        let client = Client::new(credentials);
        match self {
            Call::Search(query) => client.search(query),
            Call::Get(number) => client.get(*number),
            Call::Create { issue, .. } => client.create(issue),
            Call::Comment { number, body, .. } => client.comment(*number, body),
        }
    }

    /// What the model is told: the reply reduced to what it can use, or why
    /// there is none.
    fn result(
        &self,
        credentials: &Credentials,
        status: u16,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> String {
        let outcome = match self {
            Call::Search(query) => swamp_lab::parse::<lab::SearchResponse>(status, headers, body)
                .map(|reply| search_json(credentials, query, &reply)),
            Call::Get(_) => swamp_lab::parse::<lab::IssueDetail>(status, headers, body)
                .map(|reply| detail_json(credentials, &reply)),
            Call::Create { redacted, .. } => {
                swamp_lab::parse::<lab::Created>(status, headers, body).map(|reply| {
                    json!({
                        "created": reply.issue.number,
                        "url": credentials.issue_url(reply.issue.number),
                        "type": reply.issue.kind,
                        "status": reply.issue.status,
                        "source": reply.issue.source,
                        "visibility": reply.audience.visibility,
                        "redacted": redaction_note(redacted),
                    })
                })
            },
            Call::Comment {
                number, redacted, ..
            } => swamp_lab::parse::<lab::Comment>(status, headers, body).map(|reply| {
                json!({
                    "commented": number,
                    "comment_id": reply.id,
                    "url": credentials.issue_url(*number),
                    "redacted": redaction_note(redacted),
                })
            }),
        };
        match outcome {
            Ok(value) => value.to_string(),
            Err(error) => error.to_string(),
        }
    }
}

fn redaction_note(summary: &redact::Summary) -> Value {
    if summary.is_empty() {
        Value::Null
    } else {
        summary.to_string().into()
    }
}

fn search_json(
    credentials: &Credentials,
    query: &SearchQuery,
    reply: &lab::SearchResponse,
) -> Value {
    json!({
        "total": reply.total,
        "offset": query.offset,
        "issues": reply.issues.iter().map(|issue| json!({
            "number": issue.number,
            "title": issue.title,
            "type": issue.kind,
            "status": issue.status,
            "author": issue.author_username,
            "source": issue.source,
            "url": credentials.issue_url(issue.number),
            "updatedAt": issue.updated_at,
        })).collect::<Vec<_>>(),
    })
}

fn detail_json(credentials: &Credentials, reply: &lab::IssueDetail) -> Value {
    let issue = &reply.issue;
    let mut value = json!({
        "number": issue.number,
        "title": issue.title,
        "type": issue.kind,
        "status": issue.status,
        "author": issue.author_username,
        "source": issue.source,
        "url": credentials.issue_url(issue.number),
        "visibility": reply.audience.visibility,
        "createdAt": issue.created_at,
        "updatedAt": issue.updated_at,
        "assignees": issue.assignees.iter().map(|a| &a.username).collect::<Vec<_>>(),
        "body": issue.body,
        "comments": reply.comments.iter().map(|comment| json!({
            "author": comment.author_username,
            "createdAt": comment.created_at,
            "body": comment.body,
        })).collect::<Vec<_>>(),
        "relationships": reply.relationships.iter().map(|r| json!({
            "type": r.kind,
            "direction": r.direction,
            "source": r.source_issue_number,
            "target": r.target_issue_number,
        })).collect::<Vec<_>>(),
    });
    for (key, field) in [
        ("prUrl", issue.github_pr_url.as_deref()),
        ("resolvedInVersion", issue.resolved_in_version.as_deref()),
        ("blockedReason", issue.blocked_reason.as_deref()),
    ] {
        if let Some(field) = field {
            value[key] = field.into();
        }
    }
    if issue.is_regression {
        value["regression"] = issue
            .regression_introduced_in
            .as_deref()
            .map_or(Value::Bool(true), Value::from);
    }
    value
}

/// The confirmation's red line: who will see what, under whose name.
pub fn consequence(call: &ToolCall) -> Option<String> {
    let parsed = parse(&call.name, &call.arguments);
    let redacted = |summary: &redact::Summary| {
        if summary.is_empty() {
            String::new()
        } else {
            format!("; redacted {summary} first")
        }
    };
    Some(match (call.name.as_str(), parsed) {
        (CREATE, Ok(Call::Create { issue, redacted: summary })) => {
            let audience = if issue.kind == lab::IssueType::Security {
                "visible to you and the swamp-club admins"
            } else {
                "publicly visible"
            };
            format!(
                "files a {} issue on the swamp-club Lab under your login, {audience}{}",
                issue.kind,
                redacted(&summary)
            )
        },
        (COMMENT, Ok(Call::Comment { number, redacted: summary, .. })) => format!(
            "posts a comment on swamp-club Lab issue #{number} under your login, visible to everyone who can see the issue{}",
            redacted(&summary)
        ),
        (CREATE, _) => "files an issue on the swamp-club Lab under your login".into(),
        (COMMENT, _) => "posts a comment on a swamp-club Lab issue under your login".into(),
        _ => return None,
    })
}

/// The arguments as they will be posted, redactions applied, in place of the
/// raw ones the model wrote. `None` leaves the generic rendering, which is
/// what a call that does not parse gets: it will be refused, not posted.
pub fn confirmation_fields(call: &ToolCall) -> Option<Vec<(String, String)>> {
    match parse(&call.name, &call.arguments).ok()? {
        Call::Create { issue, .. } => {
            let mut fields = vec![
                ("type".to_owned(), issue.kind.to_string()),
                ("title".to_owned(), issue.title),
            ];
            if let Some(source) = issue.source {
                fields.push(("source".to_owned(), source));
            }
            fields.push(("body".to_owned(), issue.body));
            Some(fields)
        },
        Call::Comment { number, body, .. } => Some(vec![
            ("number".to_owned(), number.to_string()),
            ("body".to_owned(), body),
        ]),
        _ => None,
    }
}

/// Context key marking the `auth.json` read that precedes a Lab call.
pub const LOGIN_CONTEXT: &str = "swamp_login";

impl State {
    /// Validate the call, then read the login it will run as.
    pub(crate) fn start_swamp_issue(&mut self, call: &ToolCall) -> bool {
        if let Err(error) = parse(&call.name, &call.arguments) {
            self.finish_head(error);
            return true;
        }
        let argv = swamp_lab::auth::auth_json_argv();
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command(
            &borrowed,
            BTreeMap::from([
                ("tool_call_id".to_owned(), call.id.clone()),
                (LOGIN_CONTEXT.to_owned(), "1".to_owned()),
            ]),
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(crate::FRAME_SECONDS);
        false
    }

    /// The login has been read: send the request, or finish with why not.
    pub(crate) fn send_swamp_issue(
        &mut self,
        head: &ToolCall,
        exit_code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
    ) -> bool {
        let prepared = swamp_lab::auth::auth_json_from_output(exit_code, stdout, stderr)
            .and_then(|file| Credentials::resolve(&self.env, file.as_deref()))
            .and_then(|credentials| {
                let call = parse(&head.name, &head.arguments)?;
                let request = call.request(&credentials).map_err(|e| e.to_string())?;
                Ok((credentials, request))
            });
        let (credentials, request) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.finish_head(error);
                self.pump_tools();
                return true;
            },
        };
        hocus_proto::swamp::send(
            request,
            BTreeMap::from([("tool_call_id".to_owned(), head.id.clone())]),
        );
        self.status = Status::AwaitingSwamp {
            server_url: credentials.server_url,
        };
        false
    }

    pub(crate) fn finish_swamp(
        &mut self,
        status: u16,
        headers: &BTreeMap<String, String>,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        let Status::AwaitingSwamp { server_url } = &self.status else {
            return false;
        };
        let Some(head) = self.pending.front().cloned() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }
        // Only the server is needed from here on, to write issue URLs.
        let credentials = Credentials {
            server_url: server_url.clone(),
            ..Credentials::anonymous()
        };
        let result = match parse(&head.name, &head.arguments) {
            Ok(call) => call.result(&credentials, status, headers, body),
            Err(error) => error,
        };
        self.status = Status::Idle;
        self.finish_head(result);
        self.pump_tools();
        true
    }
}

#[cfg(test)]
#[path = "unit/lab.rs"]
mod tests;
