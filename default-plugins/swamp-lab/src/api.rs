//! One builder per Lab endpoint.
//!
//! Each returns the [`Request`] to send; decode the reply with
//! [`crate::http::parse`] into the type the builder names, or
//! [`crate::http::parse_empty`] for a 204. Checks here are the cheap ones the
//! server would also make (a positive issue number, a well-formed `source`,
//! something to post), so a bad call fails before it costs a round trip. The
//! server stays the authority on everything else, permissions included: an
//! admin-only builder is not refused here for a caller who is not one.

use crate::auth::Credentials;
use crate::http::{Error, Method, Request, API_KEY_HEADER};
use crate::types::{IssueStatus, IssueType, RelationshipType};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Where every Lab path starts.
pub const LAB_PATH: &str = "/api/v1/lab";
/// A list without `limit` returns every match, so a search always sends one.
pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;
pub const MAX_TITLE_CHARS: usize = 256;
pub const MAX_BODY_CHARS: usize = 65_536;
pub const MAX_QUERY_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// Case-insensitive substring of the title or body.
    pub text: Option<String>,
    pub kind: Option<IssueType>,
    pub status: Option<IssueStatus>,
    pub source: Option<String>,
    /// An exact username, case-insensitively.
    pub author: Option<String>,
    /// An ISO date, or a swamp version whose leading `YYYYMMDD` is the date.
    pub shipped_since: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            text: None,
            kind: None,
            status: None,
            source: None,
            author: None,
            shipped_since: None,
            limit: DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIssue {
    /// Only an admin may file a `platform` issue.
    pub kind: IssueType,
    pub title: String,
    pub body: String,
    /// Which product the issue is about; the server defaults it to `swamp-club`.
    pub source: Option<String>,
}

/// A partial update: a field left `None` is not sent and so not changed.
/// A non-admin author may send only `title`, `body`, `kind`, and a `status`
/// of `open` or `closed`; anything else is a 403.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<IssueStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<IssueType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_locked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `Some(None)` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_in_version: Option<Option<String>>,
    /// User ids, replacing the whole set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

/// A triage pipeline step recorded on an issue. Admin only.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleInput {
    pub step: String,
    pub target_status: IssueStatus,
    pub summary: String,
    pub emoji: String,
    /// A `classified` step whose payload carries `isRegression` also sets the
    /// issue's regression flag.
    pub payload: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_verbose: Option<bool>,
}

/// Find or create the Lab issue mirroring a GitHub issue. Admin only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsureGithub {
    /// `owner/repo`.
    pub github_repo_full_name: String,
    pub github_issue_number: u64,
    pub title: String,
    pub body: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<IssueType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_issue_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_author_login: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Client<'a> {
    credentials: &'a Credentials,
}

impl<'a> Client<'a> {
    pub fn new(credentials: &'a Credentials) -> Self {
        Client { credentials }
    }

    /// `GET /issues` → [`crate::types::SearchResponse`]. Anonymous callers see public issues only.
    pub fn search(&self, query: &SearchQuery) -> Result<Request, Error> {
        if query.limit == 0 || query.limit > MAX_LIMIT {
            return Err(Error::Invalid(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        let mut params = url::form_urlencoded::Serializer::new(String::new());
        if let Some(text) = present(&query.text) {
            if js_len(text) > MAX_QUERY_CHARS {
                return Err(Error::Invalid(format!(
                    "the search text is longer than {MAX_QUERY_CHARS} characters"
                )));
            }
            params.append_pair("q", text);
        }
        if let Some(kind) = &query.kind {
            params.append_pair("type", kind.as_str());
        }
        if let Some(status) = &query.status {
            params.append_pair("status", status.as_str());
        }
        if let Some(source) = present(&query.source) {
            params.append_pair("source", valid_source(source)?);
        }
        if let Some(author) = present(&query.author) {
            params.append_pair("author", author);
        }
        if let Some(since) = present(&query.shipped_since) {
            params.append_pair("shippedSince", since);
        }
        params.append_pair("limit", &query.limit.to_string());
        if query.offset > 0 {
            params.append_pair("offset", &query.offset.to_string());
        }
        self.request(
            Method::Get,
            &format!("/issues?{}", params.finish()),
            None,
            false,
        )
    }

    /// `GET /issues/{n}` → [`crate::types::IssueDetail`]. An issue the caller
    /// cannot see is a 404, exactly as one that does not exist.
    pub fn get(&self, number: u64) -> Result<Request, Error> {
        self.request(Method::Get, &issue_path(number)?, None, false)
    }

    /// `POST /issues` → [`crate::types::Created`]. The server may reclassify
    /// the issue, so read the type and source back from the reply.
    pub fn create(&self, issue: &NewIssue) -> Result<Request, Error> {
        let mut body = json!({
            "type": issue.kind.as_str(),
            "title": valid_title(&issue.title)?,
            "body": valid_body(&issue.body)?,
        });
        if let Some(source) = present(&issue.source) {
            body["source"] = valid_source(source)?.into();
        }
        self.request(Method::Post, "/issues", Some(body), true)
    }

    /// `PATCH /issues/{n}` → [`crate::types::Updated`].
    pub fn update(&self, number: u64, update: &IssueUpdate) -> Result<Request, Error> {
        if let Some(title) = &update.title {
            valid_title(title)?;
        }
        if let Some(body) = &update.body {
            valid_body(body)?;
        }
        if let Some(source) = &update.source {
            valid_source(source)?;
        }
        let body = serde_json::to_value(update).map_err(|e| Error::Invalid(e.to_string()))?;
        if body.as_object().is_some_and(Map::is_empty) {
            return Err(Error::Invalid("the update changes nothing".into()));
        }
        self.request(Method::Patch, &issue_path(number)?, Some(body), true)
    }

    /// `DELETE /issues/{n}` → 204. Admin only; takes the issue's comments and
    /// relationships with it.
    pub fn delete(&self, number: u64) -> Result<Request, Error> {
        self.request(Method::Delete, &issue_path(number)?, None, true)
    }

    /// `POST /issues/{n}/comments` → [`crate::types::Comment`].
    pub fn comment(&self, number: u64, body: &str) -> Result<Request, Error> {
        let body = json!({ "body": valid_body(body)? });
        let path = format!("{}/comments", issue_path(number)?);
        self.request(Method::Post, &path, Some(body), true)
    }

    /// `PATCH /issues/{n}/comments?commentId=` → [`crate::types::Comment`].
    pub fn edit_comment(
        &self,
        number: u64,
        comment_id: &str,
        body: &str,
    ) -> Result<Request, Error> {
        let body = json!({ "body": valid_body(body)? });
        self.request(
            Method::Patch,
            &comment_path(number, comment_id)?,
            Some(body),
            true,
        )
    }

    /// `DELETE /issues/{n}/comments?commentId=` → 204.
    pub fn delete_comment(&self, number: u64, comment_id: &str) -> Result<Request, Error> {
        self.request(
            Method::Delete,
            &comment_path(number, comment_id)?,
            None,
            true,
        )
    }

    /// `POST /issues/{n}/lifecycle` → [`crate::types::LifecycleEntry`].
    pub fn lifecycle(&self, number: u64, entry: &LifecycleInput) -> Result<Request, Error> {
        if entry.payload.keys().any(|key| key.starts_with('$')) {
            return Err(Error::Invalid(
                "a lifecycle payload key may not start with `$`".into(),
            ));
        }
        let body = serde_json::to_value(entry).map_err(|e| Error::Invalid(e.to_string()))?;
        let path = format!("{}/lifecycle", issue_path(number)?);
        self.request(Method::Post, &path, Some(body), true)
    }

    /// `GET /issues/{n}/relationships` → [`crate::types::Relationships`].
    pub fn relationships(&self, number: u64) -> Result<Request, Error> {
        let path = format!("{}/relationships", issue_path(number)?);
        self.request(Method::Get, &path, None, false)
    }

    /// `POST /issues/{n}/relationships` → [`crate::types::Related`]. Issue `n`
    /// is the source: it is the parent of, is blocked by, or relates to `target`.
    pub fn relate(
        &self,
        number: u64,
        kind: &RelationshipType,
        target: u64,
    ) -> Result<Request, Error> {
        if target == 0 {
            return Err(Error::Invalid(
                "the target issue number must be positive".into(),
            ));
        }
        if target == number {
            return Err(Error::Invalid(
                "an issue cannot be related to itself".into(),
            ));
        }
        let body = json!({ "type": kind.as_str(), "targetIssueNumber": target });
        let path = format!("{}/relationships", issue_path(number)?);
        self.request(Method::Post, &path, Some(body), true)
    }

    /// `DELETE /issues/{n}/relationships` → 204. The id travels in the body,
    /// not the query.
    pub fn unrelate(&self, number: u64, relationship_id: &str) -> Result<Request, Error> {
        let id = relationship_id.trim();
        if id.is_empty() {
            return Err(Error::Invalid("the relationship id is empty".into()));
        }
        let body = json!({ "relationshipId": id });
        let path = format!("{}/relationships", issue_path(number)?);
        self.request(Method::Delete, &path, Some(body), true)
    }

    /// `PATCH /issues/{n}/audience` → [`crate::types::AudienceChanged`].
    /// `None` makes the issue public; a collective id limits it to that
    /// collective's members.
    pub fn set_audience(&self, number: u64, collective_id: Option<&str>) -> Result<Request, Error> {
        let collective = collective_id.map(str::trim).filter(|id| !id.is_empty());
        let body = json!({ "collectiveId": collective });
        let path = format!("{}/audience", issue_path(number)?);
        self.request(Method::Patch, &path, Some(body), true)
    }

    /// `POST /issues/ensure` → [`crate::types::Ensured`], as a 201 when it
    /// created the issue and a 200 when one already existed.
    pub fn ensure(&self, github: &EnsureGithub) -> Result<Request, Error> {
        let repo = github.github_repo_full_name.trim();
        if repo.split('/').filter(|part| !part.is_empty()).count() != 2 {
            return Err(Error::Invalid(
                "the GitHub repository must be written owner/repo".into(),
            ));
        }
        if github.title.trim().is_empty() || github.body.is_empty() {
            return Err(Error::Invalid("a title and a body are required".into()));
        }
        let body = serde_json::to_value(github).map_err(|e| Error::Invalid(e.to_string()))?;
        self.request(Method::Post, "/issues/ensure", Some(body), true)
    }

    /// `GET /assignees` → [`crate::types::Assignees`]. Admin only.
    pub fn assignees(&self) -> Result<Request, Error> {
        self.request(Method::Get, "/assignees", None, true)
    }

    /// `GET /stream`: a server-sent event stream, read with [`crate::sse`].
    /// Needs no credentials, and carries only issue numbers and ids; fetch
    /// the issue to see what changed.
    pub fn stream(&self) -> Result<Request, Error> {
        let mut request = self.request(Method::Get, "/stream", None, false)?;
        request
            .headers
            .insert("Accept".into(), "text/event-stream".into());
        Ok(request)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        needs_login: bool,
    ) -> Result<Request, Error> {
        if needs_login && self.credentials.is_anonymous() {
            return Err(Error::NotLoggedIn);
        }
        let mut headers = BTreeMap::from([
            ("Accept".to_owned(), "application/json".to_owned()),
            (
                "User-Agent".to_owned(),
                concat!("swamp-lab/", env!("CARGO_PKG_VERSION")).to_owned(),
            ),
        ]);
        if let Some(key) = &self.credentials.api_key {
            headers.insert(API_KEY_HEADER.to_owned(), key.clone());
        }
        let body = match body {
            Some(body) => {
                headers.insert("Content-Type".into(), "application/json".into());
                serde_json::to_vec(&body).map_err(|e| Error::Invalid(e.to_string()))?
            },
            None => Vec::new(),
        };
        Ok(Request {
            method,
            url: format!("{}{LAB_PATH}{path}", self.credentials.server_url),
            headers,
            body,
        })
    }
}

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn issue_path(number: u64) -> Result<String, Error> {
    if number == 0 {
        return Err(Error::Invalid("issue numbers start at 1".into()));
    }
    Ok(format!("/issues/{number}"))
}

fn comment_path(number: u64, comment_id: &str) -> Result<String, Error> {
    let id = comment_id.trim();
    if id.is_empty() {
        return Err(Error::Invalid("the comment id is empty".into()));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("commentId", id)
        .finish();
    Ok(format!("{}/comments?{query}", issue_path(number)?))
}

/// `^[A-Za-z0-9_-]{1,64}$`, the server's rule for a source tag.
pub fn valid_source(source: &str) -> Result<&str, Error> {
    let ok = (1..=64).contains(&source.len())
        && source
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(source)
    } else {
        Err(Error::Invalid(format!(
            "source {source:?} must be 1-64 letters, digits, `_` or `-`"
        )))
    }
}

/// Lengths as the server measures them: JavaScript counts UTF-16 units.
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn valid_title(title: &str) -> Result<&str, Error> {
    let title = title.trim();
    match js_len(title) {
        0 => Err(Error::Invalid("the title is empty".into())),
        n if n > MAX_TITLE_CHARS => Err(Error::Invalid(format!(
            "the title is longer than {MAX_TITLE_CHARS} characters"
        ))),
        _ => Ok(title),
    }
}

fn valid_body(body: &str) -> Result<&str, Error> {
    let body = body.trim();
    match js_len(body) {
        0 => Err(Error::Invalid("the body is empty".into())),
        n if n > MAX_BODY_CHARS => Err(Error::Invalid(format!(
            "the body is longer than {MAX_BODY_CHARS} characters"
        ))),
        _ => Ok(body),
    }
}

#[cfg(test)]
#[path = "unit/api.rs"]
mod tests;
