//! The Linear GraphQL client: the endpoint, the hand-written query documents,
//! and the narrow response types they decode into. Read-only.
//!
//! Nothing here calls the Zellij host — `tool_loop` issues the request and hands
//! the reply back — so the whole module is unit-testable on the host triple.
//!
//! Three things about Linear's API shape the code and are easy to get wrong:
//!
//! - **The key goes in `Authorization` raw, with no `Bearer` prefix.** Every
//!   other endpoint hocus talks to takes `Bearer`; this one does not, and the
//!   failure mode is a 401 whose body does not say why.
//! - **Failures arrive inside a 200.** Linear returns `{"data":null,"errors":[…]}`
//!   for a validation failure with an HTTP 200, and the *same* body shape with a
//!   400 or 401. So [`format`] parses first and branches on `errors`; branching
//!   on the status would report success on a failed query.
//! - **User values are never interpolated into a document.** Every document here
//!   is a constant and everything variable rides in `variables`, which is the
//!   whole of the escaping story.
//!
//! There is deliberately no pagination. A tool result is re-sent on every later
//! turn of the conversation, so handing the model a cursor invites it to page
//! through a hundred issues and pay for all of them on every turn thereafter.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::Args;

pub const ENDPOINT: &str = "https://api.linear.app/graphql";

/// The default *name* of the variable holding the key — never the key itself.
pub const DEFAULT_API_KEY_ENV: &str = "LINEAR_API_KEY";

/// Rows per listing. Every one is re-sent on every later turn of the
/// conversation, the same reason `api::SEARCH_RESULTS` is 8.
pub const DEFAULT_LIMIT: usize = 15;
const MAX_LIMIT: usize = 50;

/// Caps on a single issue's detail. `finish_head` does not truncate — the
/// `MAX_TOOL_OUTPUT` cap applies only to the `RunCommandResult` path — so these
/// are the only thing standing between a long issue and the whole transcript.
const MAX_DESCRIPTION: usize = 4_000;
const MAX_COMMENTS: usize = 10;
const MAX_COMMENT: usize = 800;
/// Cap on the text of a GraphQL error, which can carry a whole query back.
const MAX_ERROR: usize = 500;

/// The states `linear_issues` accepts, matching Linear's `WorkflowState.type`
/// plus our own `open`. Closed here as well as in the schema: a model that
/// ignores the enum and sends a state *name* ("In Progress") would otherwise get
/// an empty list and no explanation.
pub const STATES: [&str; 7] = [
    "triage",
    "backlog",
    "unstarted",
    "started",
    "completed",
    "canceled",
    "open",
];

// ---------------------------------------------------------------------------
// Query documents
// ---------------------------------------------------------------------------
//
// Field selections are inlined rather than shared through a GraphQL fragment.
// A fragment would have to be declared `on Issue`, and Linear's search payload
// returns its own node type — whether a fragment spreads onto it is exactly the
// kind of thing that fails at runtime against a schema we cannot introspect
// from here. Repeating six field names is cheaper than that risk.

const ROW_FIELDS: &str = "identifier title priority updatedAt state { name type } \
                          assignee { displayName } team { key }";

/// Full-text search.
///
/// This is the substring form: `title`/`description` matched with
/// `containsIgnoreCase`, which is plain `issues(filter:)` and therefore valid
/// against any version of the schema. Linear also has a ranked full-text entry
/// point — `searchIssues(term:)` in current docs, `issueSearch(query:)` in older
/// ones — which is better but which has changed spelling more than once. Swap
/// this constant for the ranked form once it has been confirmed against the live
/// schema; the rest of the module does not care, because [`rows_from`] finds the
/// nodes wherever the document put them:
///
/// ```graphql
/// query Search($term: String!, $first: Int!) {
///   searchIssues(term: $term, first: $first) { nodes { … } }
/// }
/// ```
fn search_document() -> String {
    format!(
        "query Search($term: String!, $first: Int!) {{ \
           issues(first: $first, orderBy: updatedAt, filter: {{ or: [ \
             {{ title: {{ containsIgnoreCase: $term }} }}, \
             {{ description: {{ containsIgnoreCase: $term }} }} \
           ] }}) {{ nodes {{ {ROW_FIELDS} }} }} \
         }}"
    )
}

/// A filtered listing. The filter is built in Rust and passed whole, because
/// Linear's `IssueFilter` does not reliably read a null member as "no filter" —
/// `{assignee: null}` can mean "unassigned" instead.
fn issues_document() -> String {
    format!(
        "query Issues($filter: IssueFilter, $first: Int!) {{ \
           issues(first: $first, orderBy: updatedAt, filter: $filter) {{ nodes {{ {ROW_FIELDS} }} }} \
         }}"
    )
}

/// The same listing for "my issues". `viewer.assignedIssues` cannot be wrong
/// about who "me" is, which an `assignee` filter member could be.
fn mine_document() -> String {
    format!(
        "query Mine($filter: IssueFilter, $first: Int!) {{ \
           viewer {{ assignedIssues(first: $first, orderBy: updatedAt, filter: $filter) \
             {{ nodes {{ {ROW_FIELDS} }} }} }} \
         }}"
    )
}

fn detail_fields(comments: bool) -> String {
    let comments = if comments {
        format!("comments(first: {MAX_COMMENTS}) {{ nodes {{ createdAt body user {{ displayName }} }} }}")
    } else {
        String::new()
    };
    format!(
        "identifier title url priority estimate createdAt updatedAt \
         state {{ name type }} assignee {{ displayName }} creator {{ displayName }} \
         team {{ key name }} project {{ name }} parent {{ identifier title }} \
         labels {{ nodes {{ name }} }} description {comments}"
    )
}

/// One issue by team key and number — `ENG-123`. Spelled as a filtered listing
/// rather than `issue(id:)` because `issue(id:)` taking a human identifier is
/// documented but unverified, while this form is unambiguously valid.
fn issue_by_key_document(comments: bool) -> String {
    format!(
        "query IssueByKey($key: String!, $number: Float!) {{ \
           issues(first: 1, filter: {{ team: {{ key: {{ eq: $key }} }}, \
                                       number: {{ eq: $number }} }}) \
             {{ nodes {{ {} }} }} \
         }}",
        detail_fields(comments)
    )
}

/// One issue by UUID.
fn issue_by_id_document(comments: bool) -> String {
    format!(
        "query IssueById($id: String!) {{ issue(id: $id) {{ {} }} }}",
        detail_fields(comments)
    )
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Personal API keys go in `Authorization` **raw**. No `Bearer` prefix — that is
/// the OAuth form, and sending it with a personal key is a 401.
pub fn headers(api_key: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Accept".to_owned(), "application/json".to_owned()),
        ("Authorization".to_owned(), api_key.to_owned()),
    ])
}

fn body(document: String, variables: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "query": document, "variables": variables }))
        .expect("serializing a Linear request cannot fail")
}

/// The POST body for one call, or the sentence explaining why there isn't one.
pub fn request_for(tool: &str, args: &Args) -> Result<Vec<u8>, String> {
    let first = clamp_limit(args.limit) as u64;
    match tool {
        "linear_search" => {
            let term = args
                .query
                .as_deref()
                .map(str::trim)
                .filter(|term| !term.is_empty())
                .ok_or("query is required and must not be empty")?;
            Ok(body(
                search_document(),
                json!({ "term": term, "first": first }),
            ))
        },
        "linear_issues" => {
            let filter = build_filter(args)?;
            // No assignee, or explicitly "me": ask as the viewer, which cannot
            // be wrong about who that is.
            let mine = args
                .assignee
                .as_deref()
                .map(str::trim)
                .is_none_or(|who| who.is_empty() || who.eq_ignore_ascii_case("me"));
            let document = if mine {
                mine_document()
            } else {
                issues_document()
            };
            Ok(body(document, json!({ "filter": filter, "first": first })))
        },
        "linear_issue" => {
            let raw = args
                .identifier
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or("identifier is required, e.g. ENG-123")?;
            // Absent means yes: a bare `linear_issue` should read the whole
            // thing, and the model opts out when it only wants the summary.
            let comments = args.comments.unwrap_or(true);
            if let Some((key, number)) = parse_identifier(raw) {
                Ok(body(
                    issue_by_key_document(comments),
                    json!({ "key": key, "number": number }),
                ))
            } else if is_uuid(raw) {
                Ok(body(issue_by_id_document(comments), json!({ "id": raw })))
            } else {
                Err(format!(
                    "{raw:?} is not an issue identifier; use the form ENG-123, or the issue's UUID"
                ))
            }
        },
        other => Err(format!("no such tool: {other}")),
    }
}

/// The `IssueFilter` as a JSON value. Members are omitted rather than nulled:
/// Linear reads a null member as a filter on absence, not as "no filter".
///
/// The assignee is deliberately not represented. "me" is served by the
/// `viewer.assignedIssues` document instead, and a named assignee is the only
/// case that lands here.
fn build_filter(args: &Args) -> Result<Value, String> {
    let mut filter = serde_json::Map::new();

    if let Some(who) = trimmed(&args.assignee) {
        if !who.eq_ignore_ascii_case("me") {
            filter.insert(
                "assignee".to_owned(),
                json!({ "displayName": { "eqIgnoreCase": who } }),
            );
        }
    }
    if let Some(team) = trimmed(&args.team) {
        filter.insert("team".to_owned(), json!({ "key": { "eq": team } }));
    }
    if let Some(project) = trimmed(&args.project) {
        filter.insert(
            "project".to_owned(),
            json!({ "name": { "containsIgnoreCase": project } }),
        );
    }
    if let Some(label) = trimmed(&args.label) {
        filter.insert(
            "labels".to_owned(),
            json!({ "name": { "eqIgnoreCase": label } }),
        );
    }
    if let Some(since) = trimmed(&args.updated_since) {
        if !is_iso_date(since) {
            return Err(format!(
                "updated_since must be an ISO 8601 date such as 2026-08-01, not {since:?}"
            ));
        }
        filter.insert("updatedAt".to_owned(), json!({ "gte": since }));
    }

    // Absent means open: the question "what am I working on" is the one this
    // tool exists for, and answering it with three years of closed issues is
    // both wrong and expensive.
    let state = trimmed(&args.state).unwrap_or("open");
    if !STATES.contains(&state) {
        return Err(format!(
            "unknown state {state:?}; use one of {}",
            STATES.join(", ")
        ));
    }
    let state_filter = if state == "open" {
        json!({ "type": { "nin": ["completed", "canceled"] } })
    } else {
        json!({ "type": { "eq": state } })
    };
    filter.insert("state".to_owned(), state_filter);

    Ok(Value::Object(filter))
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Rows per call: the model's ask, bounded at both ends. Zero would be a call
/// that cannot answer anything, so it is raised rather than refused.
pub fn clamp_limit(limit: Option<usize>) -> usize {
    match limit {
        None => DEFAULT_LIMIT,
        Some(limit) => limit.clamp(1, MAX_LIMIT),
    }
}

/// `"ENG-123"` -> `("ENG", 123)`. `None` for anything else, including a UUID.
///
/// Split on the *last* hyphen and then insist the key is alphanumeric, which is
/// what rejects `A-B-1`: Linear team keys never contain a hyphen, so a leftover
/// one means this was never an identifier.
pub fn parse_identifier(raw: &str) -> Option<(String, u64)> {
    let raw = raw.trim();
    let (key, number) = raw.rsplit_once('-')?;
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((key.to_ascii_uppercase(), number.parse().ok()?))
}

/// The 8-4-4-4-12 hex form. Linear's own ids are UUIDs, and the model will
/// sometimes have one from an earlier result.
pub fn is_uuid(raw: &str) -> bool {
    let raw = raw.trim();
    let groups: Vec<&str> = raw.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Enough of a check to catch a model sending "last week". Anything that starts
/// with a plausible `YYYY-MM-DD` passes; Linear rejects the rest with a message
/// of its own, which is better than a regex here pretending to be a date parser.
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Envelope {
    data: Value,
    errors: Vec<GqlError>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GqlError {
    message: String,
    extensions: Extensions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Extensions {
    code: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Row {
    identifier: String,
    title: String,
    priority: Option<f64>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    state: Option<Named>,
    assignee: Option<Person>,
    team: Option<TeamRef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Detail {
    identifier: String,
    title: String,
    url: Option<String>,
    priority: Option<f64>,
    estimate: Option<f64>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    state: Option<Named>,
    assignee: Option<Person>,
    creator: Option<Person>,
    team: Option<TeamRef>,
    project: Option<NameOnly>,
    parent: Option<ParentRef>,
    labels: Option<LabelNodes>,
    description: Option<String>,
    comments: Option<CommentNodes>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Person {
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TeamRef {
    key: String,
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NameOnly {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParentRef {
    identifier: String,
    title: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LabelNodes {
    nodes: Vec<NameOnly>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CommentNodes {
    nodes: Vec<Comment>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Comment {
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    body: Option<String>,
    user: Option<Person>,
}

/// The reply, as the sentence or listing the model sees.
///
/// Takes the status rather than letting the caller branch on 2xx: a Linear
/// failure can arrive with a 200, so that decision has to live in one place and
/// this is it.
pub fn format(tool: &str, status: u16, body: &[u8], api_key_env: &str) -> String {
    let Ok(envelope) = serde_json::from_slice::<Envelope>(body) else {
        // Not a GraphQL body at all — an HTML error page from a proxy, most
        // likely. `api_error_message` renders that far better than we could.
        let message = crate::api::api_error_message(status, body);
        return match status {
            401 | 403 => format!(
                "{message}\n\nLinear rejected the API key. Check ${api_key_env} in the \
                 environment the Zellij session was started from."
            ),
            429 => format!("{message}\n\nRate limited by Linear; try again shortly."),
            _ => message,
        };
    };

    if !envelope.errors.is_empty() {
        return format_errors(&envelope.errors, api_key_env);
    }

    match tool {
        "linear_issue" => match detail_from(&envelope.data) {
            Some(detail) => format_detail(&detail),
            None => "no such issue".to_owned(),
        },
        _ => match rows_from(&envelope.data) {
            Some(rows) if !rows.is_empty() => {
                rows.iter().map(format_row).collect::<Vec<_>>().join("\n")
            },
            // Never an empty string: to the model that reads as a broken tool,
            // and it retries the same call.
            Some(_) => "no issues match".to_owned(),
            None => "could not read Linear's response".to_owned(),
        },
    }
}

fn format_errors(errors: &[GqlError], api_key_env: &str) -> String {
    let joined: Vec<String> = errors
        .iter()
        .map(|error| match &error.extensions.code {
            Some(code) if !code.is_empty() => format!("{} ({code})", error.message.trim()),
            _ => error.message.trim().to_owned(),
        })
        .collect();
    let mut message = format!("Linear error: {}", truncate(&joined.join("; "), MAX_ERROR));

    // Each of these implies a different thing for the user to do, so name it
    // rather than leaving them to read the raw message.
    let codes: Vec<&str> = errors
        .iter()
        .filter_map(|error| error.extensions.code.as_deref())
        .collect();
    if codes.iter().any(|code| code.contains("AUTHENTICATION")) {
        message.push_str(&format!(
            "\n\nCheck ${api_key_env} in the environment the Zellij session was started from."
        ));
    } else if codes.iter().any(|code| code.contains("RATELIMIT")) {
        message.push_str("\n\nRate limited by Linear; try again shortly.");
    }
    message
}

/// The nodes, wherever the document put them. Listed rather than hardcoded so
/// that swapping the search document for the ranked form needs no change here.
fn rows_from(data: &Value) -> Option<Vec<Row>> {
    for pointer in [
        "/issues/nodes",
        "/searchIssues/nodes",
        "/issueSearch/nodes",
        "/viewer/assignedIssues/nodes",
    ] {
        if let Some(nodes) = data.pointer(pointer) {
            if let Ok(rows) = serde_json::from_value::<Vec<Row>>(nodes.clone()) {
                return Some(rows);
            }
        }
    }
    None
}

fn detail_from(data: &Value) -> Option<Detail> {
    if let Some(issue) = data.pointer("/issue").filter(|issue| !issue.is_null()) {
        return serde_json::from_value(issue.clone()).ok();
    }
    let first = data.pointer("/issues/nodes")?.as_array()?.first()?;
    serde_json::from_value(first.clone()).ok()
}

/// One issue as one line. No URL: at ~60 characters each, a listing of them is
/// most of a page of tokens re-sent every turn, and the identifier is what both
/// a human and `linear_issue` actually need.
fn format_row(row: &Row) -> String {
    let mut line = row.identifier.clone();
    if let Some(team) = row.team.as_ref().filter(|team| !team.key.is_empty()) {
        line.push_str(&format!(" [{}]", team.key));
    }
    if let Some(state) = row.state.as_ref().filter(|state| !state.name.is_empty()) {
        line.push_str(&format!(" {}", state.name));
    }
    if let Some(priority) = priority_label(row.priority) {
        line.push_str(&format!(" {priority}"));
    }
    if let Some(who) = row
        .assignee
        .as_ref()
        .filter(|who| !who.display_name.is_empty())
    {
        line.push_str(&format!(" {}", who.display_name));
    }
    if let Some(day) = row.updated_at.as_deref().and_then(day) {
        line.push_str(&format!(" {day}"));
    }
    format!("{line} — {}", row.title.trim())
}

fn format_detail(detail: &Detail) -> String {
    let mut out = format!("{} {}", detail.identifier, detail.title.trim());

    let mut facts: Vec<String> = Vec::new();
    if let Some(state) = detail.state.as_ref().filter(|s| !s.name.is_empty()) {
        facts.push(format!("state: {}", state.name));
    }
    if let Some(team) = detail.team.as_ref().filter(|t| !t.key.is_empty()) {
        facts.push(format!("team: {} ({})", team.key, team.name));
    }
    if let Some(who) = detail
        .assignee
        .as_ref()
        .filter(|w| !w.display_name.is_empty())
    {
        facts.push(format!("assignee: {}", who.display_name));
    }
    if let Some(who) = detail
        .creator
        .as_ref()
        .filter(|w| !w.display_name.is_empty())
    {
        facts.push(format!("creator: {}", who.display_name));
    }
    if let Some(priority) = priority_label(detail.priority) {
        facts.push(format!("priority: {priority}"));
    }
    if let Some(estimate) = detail.estimate {
        facts.push(format!("estimate: {estimate}"));
    }
    if let Some(project) = detail.project.as_ref().filter(|p| !p.name.is_empty()) {
        facts.push(format!("project: {}", project.name));
    }
    if let Some(parent) = detail.parent.as_ref().filter(|p| !p.identifier.is_empty()) {
        facts.push(format!("parent: {} {}", parent.identifier, parent.title));
    }
    let labels: Vec<&str> = detail
        .labels
        .iter()
        .flat_map(|labels| labels.nodes.iter())
        .map(|label| label.name.as_str())
        .filter(|name| !name.is_empty())
        .collect();
    if !labels.is_empty() {
        facts.push(format!("labels: {}", labels.join(", ")));
    }
    if let Some(day) = detail.created_at.as_deref().and_then(day) {
        facts.push(format!("created: {day}"));
    }
    if let Some(day) = detail.updated_at.as_deref().and_then(day) {
        facts.push(format!("updated: {day}"));
    }
    if let Some(url) = detail.url.as_deref().filter(|url| !url.is_empty()) {
        facts.push(format!("url: {url}"));
    }
    if !facts.is_empty() {
        out.push('\n');
        out.push_str(&facts.join("\n"));
    }

    if let Some(description) = detail
        .description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        out.push_str("\n\n");
        out.push_str(&truncate(description, MAX_DESCRIPTION));
    }

    let comments: Vec<&Comment> = detail
        .comments
        .iter()
        .flat_map(|comments| comments.nodes.iter())
        .collect();
    if !comments.is_empty() {
        out.push_str("\n\ncomments:");
        for comment in comments.iter().take(MAX_COMMENTS) {
            let who = comment
                .user
                .as_ref()
                .map(|user| user.display_name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("someone");
            let when = comment
                .created_at
                .as_deref()
                .and_then(day)
                .unwrap_or_default();
            let text = comment.body.as_deref().map(str::trim).unwrap_or_default();
            out.push_str(&format!(
                "\n\n{who} {when}\n{}",
                truncate(text, MAX_COMMENT)
            ));
        }
        if comments.len() > MAX_COMMENTS {
            out.push_str(&format!(
                "\n\n… {} more comments not shown",
                comments.len() - MAX_COMMENTS
            ));
        }
    }

    out
}

/// Linear's priority is 0-4, and 0 means "none" rather than "most urgent" —
/// rendering the bare number would read as the opposite of what it means.
fn priority_label(priority: Option<f64>) -> Option<&'static str> {
    match priority? as i64 {
        1 => Some("Urgent"),
        2 => Some("High"),
        3 => Some("Medium"),
        4 => Some("Low"),
        _ => None,
    }
}

/// The date out of an ISO timestamp. The time of day is never what is being
/// asked and costs a token on every row.
fn day(timestamp: &str) -> Option<&str> {
    is_iso_date(timestamp).then(|| &timestamp[..10])
}

/// Says so when it cuts, so the model knows it is not seeing everything.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}… (truncated)")
}

#[path = "./unit/linear_tests/mod.rs"]
#[cfg(test)]
mod linear_tests;
