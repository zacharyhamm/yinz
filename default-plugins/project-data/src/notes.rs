//! Persistent notes, beside the todo lists.
//!
//! A note is a titled piece of Markdown that belongs to a project — keyed on
//! the project's root folder, for the same reason a todo list is — or to no
//! project at all, in which case its `root` is null and it is global. It carries
//! who wrote it, whether it has been archived, and the store's own timestamps.
//!
//! Notes and todos reference each other. A note's root is its tie to a list; a
//! `note_link` row ties it to one todo item. Both directions are answered from
//! the same rows: the notes of a list are the notes with its root plus the notes
//! linked to any item in it, and the notes of an item are its links.
//!
//! Tags are rows too (`note_tag`), one per tag per note, so a listing can be
//! filtered to a tag by the store rather than in memory.
//!
//! Everything here is either pure or goes through [`Backend`], so it all tests
//! on the host against the real engine.

use crate::store::{fetch_all, Backend};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use yinz_tile::prelude::{DataRevision, DataRowId};
use yinz_tile::prelude::{DataRows, DataValue, FetchRequest, Mutation, Pred, SortDir};

/// The most notes one scope holds, live or archived. Equal to the most rows a
/// fetch returns, so a scope's listing is always one fetch.
pub const MAX_NOTES_PER_SCOPE: usize = 1000;
/// The most characters in a title. A title is one line; a longer thought is a body.
pub const MAX_TITLE_CHARS: usize = 200;
/// The most UTF-8 bytes in a cleaned body (8 MiB), inclusive.
/// Searches remain bounded by the store's fetch budget.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// The most tags on one note.
pub const MAX_TAGS: usize = 16;
/// The most characters in one tag.
pub const MAX_TAG_CHARS: usize = 32;
/// The most matches one search answers with.
pub const MAX_MATCHES: usize = 50;
/// The most characters of a body a search result shows.
pub const SNIPPET_CHARS: usize = 160;

/// The most ids one `IN` predicate names, for side tables and for the note
/// fetch of a narrowed listing alike. Well under SQLite's variable cap, and
/// small enough that a side table's rows for a chunk fit a handful of pages.
const IN_CHUNK: usize = 500;

/// The columns a listing reads. Not `body`: a thousand notes at the body cap
/// would blow the byte limit of one fetch, and a listing does not show it.
///
/// `rev` is here because naming any column at all drops the ones the store adds
/// unasked, and a `Note` whose `rev` came back zero is indistinguishable from one
/// with no row to guard — so a listing that omitted it would hand every reader a
/// note that [`plan_edit`] then writes unguarded.
pub const LIST_COLUMNS: &[&str] = &[
    "id",
    "root",
    "title",
    "author",
    "archived",
    "created_at",
    "updated_at",
    "rev",
];

/// One note, with its tags and the todo items it references.
///
/// `body` is empty when the note came from a listing, which does not read it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    /// The normalized project root, or `None` for a global note.
    pub root: Option<String>,
    pub title: String,
    pub body: String,
    pub author: String,
    pub archived: bool,
    /// Wall-clock milliseconds, as the store stamps them.
    pub created_at: i64,
    pub updated_at: i64,
    /// The store's revision of this row when it was read, and what an edit guards on. Zero on a
    /// note that did not come from a store row.
    pub rev: DataRevision,
    pub tags: Vec<String>,
    pub todo_ids: Vec<u64>,
}

/// Which notes a read is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope<'a> {
    /// The notes of one project, by normalized root.
    Project(&'a str),
    /// The notes tied to no project.
    Global,
    /// Every note.
    All,
}

impl Scope<'_> {
    /// The predicate that selects this scope, or none for every note.
    pub fn pred(self) -> Option<Pred> {
        match self {
            Scope::Project(root) => Some(Pred::eq("root", root)),
            Scope::Global => Some(Pred::is_null("root")),
            Scope::All => None,
        }
    }
}

/// The scope a call resolved to, owning its root so it can outlive the call's
/// arguments. What the tools and the pipe both build before asking for a
/// [`Scope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    /// One project's notes, and its name for the reply if it still has one.
    Project {
        root: String,
        name: Option<String>,
    },
    Global,
    All,
}

impl Where {
    pub fn scope(&self) -> Scope<'_> {
        match self {
            Where::Project { root, .. } => Scope::Project(root),
            Where::Global => Scope::Global,
            Where::All => Scope::All,
        }
    }

    /// The root a note written here gets: none for a global note.
    pub fn root(&self) -> Option<&str> {
        match self {
            Where::Project { root, .. } => Some(root),
            Where::Global | Where::All => None,
        }
    }

    /// What a reply calls the scope: the project's name, its root if the
    /// project is gone, or the word for the other two.
    pub fn label(&self) -> String {
        match self {
            Where::Project { root, name } => name.clone().unwrap_or_else(|| root.clone()),
            Where::Global => "global".to_owned(),
            Where::All => "all".to_owned(),
        }
    }
}

/// What a listing is narrowed to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filter<'a> {
    pub scope: Option<Scope<'a>>,
    pub include_archived: bool,
    /// Only notes carrying this tag.
    pub tag: Option<&'a str>,
    /// Only notes linked to this todo item.
    pub todo_id: Option<u64>,
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

/// A title as stored: one line of visible text, capped.
pub fn clean_title(title: &str) -> Result<String, String> {
    let title =
        crate::todos::clean_text(title).map_err(|_| "title must contain visible characters")?;
    let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "title is longer than {MAX_TITLE_CHARS} characters; put the rest in the body"
        ));
    }
    Ok(title)
}

/// A body as stored: Markdown with its newlines and indentation, control
/// characters dropped, capped.
pub fn clean_body(body: &str) -> Result<String, String> {
    let body =
        crate::todos::clean_text(body).map_err(|_| "body must contain visible characters")?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "body is longer than 8 MiB ({MAX_BODY_BYTES} bytes); split it into more than one note"
        ));
    }
    Ok(body)
}

/// A tag as stored: lowercase, trimmed, `[a-z0-9_-]` only.
pub fn normalize_tag(tag: &str) -> Result<String, String> {
    let tag = tag.trim().trim_start_matches('#').to_lowercase();
    if tag.is_empty() {
        return Err("a tag must contain visible characters".to_owned());
    }
    if tag.chars().count() > MAX_TAG_CHARS {
        return Err(format!(
            "tag `{tag}` is longer than {MAX_TAG_CHARS} characters"
        ));
    }
    if !tag
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "tag `{tag}` may only contain letters, digits, `_` and `-`"
        ));
    }
    Ok(tag)
}

/// A set of tags as stored: each normalized, deduplicated, sorted, capped.
pub fn normalize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let set: BTreeSet<String> = tags
        .iter()
        .map(|tag| normalize_tag(tag))
        .collect::<Result<_, _>>()?;
    if set.len() > MAX_TAGS {
        return Err(format!("a note carries {MAX_TAGS} tags at most"));
    }
    Ok(set.into_iter().collect())
}

/// An author as stored. The same alphabet as a tag: `user`, `agent`, or the
/// kind of an agent, none of which needs more.
pub fn normalize_author(author: &str) -> Result<String, String> {
    normalize_tag(author).map_err(|_| {
        "author must be a short lowercase word such as `user`, `agent` or an agent kind".to_owned()
    })
}

/// The uid of a link row, so a repeated link upserts rather than piling up.
pub fn link_uid(note_id: u64, todo_id: u64) -> String {
    format!("{note_id}:{todo_id}")
}

/// The uid of a tag row, for the same reason.
pub fn tag_uid(note_id: u64, tag: &str) -> String {
    format!("{note_id}:{tag}")
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<String> {
    match rows.get(row, column) {
        Some(DataValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<i64> {
    match rows.get(row, column) {
        Some(DataValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn id_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<u64> {
    int_at(rows, row, column).and_then(|value| u64::try_from(value).ok())
}

fn bool_at(rows: &DataRows, row: &[DataValue], column: &str) -> bool {
    matches!(rows.get(row, column), Some(DataValue::Bool(true)))
}

/// The notes these rows describe, without tags or links, in row order.
pub fn from_rows(rows: &DataRows) -> Vec<Note> {
    rows.rows
        .iter()
        .filter_map(|row| {
            let (Some(id), Some(title)) = (id_at(rows, row, "id"), text_at(rows, row, "title"))
            else {
                return None;
            };
            Some(Note {
                id,
                root: text_at(rows, row, "root"),
                title,
                body: text_at(rows, row, "body").unwrap_or_default(),
                author: text_at(rows, row, "author").unwrap_or_else(|| "user".to_owned()),
                archived: bool_at(rows, row, "archived"),
                created_at: int_at(rows, row, "created_at").unwrap_or_default(),
                updated_at: int_at(rows, row, "updated_at").unwrap_or_default(),
                rev: int_at(rows, row, "rev")
                    .map(DataRevision::new)
                    .unwrap_or_default(),
                tags: Vec::new(),
                todo_ids: Vec::new(),
            })
        })
        .collect()
}

/// The rows of a side table for a set of note ids, chunked so no `IN` grows
/// past what one query should carry. An empty set is no rows, not an error.
fn side_rows(
    backend: &impl Backend,
    entity: &str,
    key: &str,
    ids: &[u64],
) -> Result<Vec<DataRows>, String> {
    ids.chunks(IN_CHUNK)
        .map(|chunk| {
            fetch_all(
                backend,
                FetchRequest::entity(entity).filter(Pred::is_in(key, chunk.iter().copied())),
            )
        })
        .collect()
}

/// Read the tags and links of these notes onto them.
fn attach(backend: &impl Backend, notes: &mut [Note]) -> Result<(), String> {
    if notes.is_empty() {
        return Ok(());
    }
    let ids: Vec<u64> = notes.iter().map(|note| note.id).collect();
    let mut tags: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for rows in side_rows(backend, "note_tag", "note_id", &ids)? {
        for row in &rows.rows {
            if let (Some(note_id), Some(tag)) =
                (id_at(&rows, row, "note_id"), text_at(&rows, row, "tag"))
            {
                tags.entry(note_id).or_default().push(tag);
            }
        }
    }
    let mut links: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for rows in side_rows(backend, "note_link", "note_id", &ids)? {
        for row in &rows.rows {
            if let (Some(note_id), Some(todo_id)) =
                (id_at(&rows, row, "note_id"), id_at(&rows, row, "todo_id"))
            {
                links.entry(note_id).or_default().push(todo_id);
            }
        }
    }
    for note in notes {
        if let Some(mut found) = tags.remove(&note.id) {
            found.sort();
            note.tags = found;
        }
        if let Some(mut found) = links.remove(&note.id) {
            found.sort_unstable();
            note.todo_ids = found;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// How many notes a scope holds, archived ones included.
pub fn count_in_scope(backend: &impl Backend, scope: Scope) -> Result<usize, String> {
    let mut request = FetchRequest::entity("note");
    if let Some(pred) = scope.pred() {
        request = request.filter(pred);
    }
    backend
        .count(&request)
        .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
}

/// One note, whole, or none if the id names nothing.
pub fn fetch_note(backend: &impl Backend, id: u64) -> Result<Option<Note>, String> {
    let rows = backend.fetch(
        &FetchRequest::entity("note")
            .filter(Pred::eq("id", id))
            .limit(1),
    )?;
    let mut notes = from_rows(&rows);
    attach(backend, &mut notes)?;
    Ok(notes.into_iter().next())
}

/// The note ids a side table names for one key value.
fn ids_where(
    backend: &impl Backend,
    entity: &str,
    key: &str,
    value: impl Into<DataValue>,
) -> Result<BTreeSet<u64>, String> {
    let rows = fetch_all(
        backend,
        FetchRequest::entity(entity).filter(Pred::eq(key, value)),
    )?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| id_at(&rows, row, "note_id"))
        .collect())
}

/// The notes a filter selects, newest change first, without bodies.
///
/// A tag or a todo item narrows the set through its side table first, so the
/// note fetch names ids rather than reading a whole scope to throw most of it
/// away. Both given means both must hold.
pub fn list_notes(backend: &impl Backend, filter: Filter) -> Result<Vec<Note>, String> {
    let mut ids: Option<BTreeSet<u64>> = None;
    if let Some(tag) = filter.tag {
        let tag = normalize_tag(tag)?;
        ids = Some(ids_where(backend, "note_tag", "tag", tag)?);
    }
    if let Some(todo_id) = filter.todo_id {
        let linked = ids_where(backend, "note_link", "todo_id", todo_id)?;
        ids = Some(match ids {
            Some(ids) => ids.intersection(&linked).copied().collect(),
            None => linked,
        });
    }
    if matches!(&ids, Some(ids) if ids.is_empty()) {
        return Ok(Vec::new());
    }

    let mut request = FetchRequest::entity("note")
        .select(LIST_COLUMNS.iter().copied())
        .sort_by("updated_at", SortDir::Desc)
        .sort_by("id", SortDir::Desc)
        .limit(crate::store::LIST_FETCH_LIMIT);
    if let Some(pred) = filter.scope.and_then(Scope::pred) {
        request = request.filter(pred);
    }
    if !filter.include_archived {
        request = request.filter(Pred::eq("archived", false));
    }
    let mut notes = match ids {
        None => from_rows(&backend.fetch(&request)?),
        Some(ids) => {
            // A tag shared across scopes can name more ids than one `IN`
            // should carry, so fetch a chunk at a time. Each chunk comes back
            // sorted and capped on its own, so the merge sorts and caps again
            // to give what one unchunked fetch would have.
            let ids: Vec<u64> = ids.into_iter().collect();
            let mut notes = Vec::new();
            for chunk in ids.chunks(IN_CHUNK) {
                let request = request
                    .clone()
                    .filter(Pred::is_in("id", chunk.iter().copied()));
                notes.extend(from_rows(&backend.fetch(&request)?));
            }
            notes.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
            notes.truncate(crate::store::LIST_FETCH_LIMIT as usize);
            notes
        },
    };
    attach(backend, &mut notes)?;
    Ok(notes)
}

/// The ids of the notes linked to one todo item.
pub fn notes_for_todo(backend: &impl Backend, todo_id: u64) -> Result<Vec<u64>, String> {
    Ok(ids_where(backend, "note_link", "todo_id", todo_id)?
        .into_iter()
        .collect())
}

/// Todo item id to the ids of the notes linked to it, for every item that has
/// any. What annotates a todo listing.
pub fn links_for_todos(
    backend: &impl Backend,
    todo_ids: &[u64],
) -> Result<BTreeMap<u64, Vec<u64>>, String> {
    let mut links: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for rows in side_rows(backend, "note_link", "todo_id", todo_ids)? {
        for row in &rows.rows {
            if let (Some(todo_id), Some(note_id)) =
                (id_at(&rows, row, "todo_id"), id_at(&rows, row, "note_id"))
            {
                links.entry(todo_id).or_default().push(note_id);
            }
        }
    }
    for ids in links.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    Ok(links)
}

/// Every item id in a todo listing as `todos::list_json` shapes it.
pub fn todo_ids_in(value: &serde_json::Value) -> Vec<u64> {
    let mut ids = Vec::new();
    collect_todo_ids(value.get("items"), &mut ids);
    ids
}

fn collect_todo_ids(items: Option<&serde_json::Value>, ids: &mut Vec<u64>) {
    let Some(items) = items.and_then(serde_json::Value::as_array) else {
        return;
    };
    for item in items {
        if let Some(id) = item.get("id").and_then(serde_json::Value::as_u64) {
            ids.push(id);
        }
        collect_todo_ids(item.get("children"), ids);
    }
}

/// Put a `notes` array on every item of a todo listing that has linked notes.
/// Items with none are left as they were, so a listing without notes is
/// byte-for-byte what it was before notes existed.
pub fn annotate_todo_list(value: &mut serde_json::Value, links: &BTreeMap<u64, Vec<u64>>) {
    if links.is_empty() {
        return;
    }
    if let Some(items) = value.get_mut("items") {
        annotate_items(items, links);
    }
}

fn annotate_items(items: &mut serde_json::Value, links: &BTreeMap<u64, Vec<u64>>) {
    let Some(items) = items.as_array_mut() else {
        return;
    };
    for item in items {
        if let Some(notes) = item
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| links.get(&id))
        {
            item["notes"] = serde_json::json!(notes);
        }
        if let Some(children) = item.get_mut("children") {
            annotate_items(children, links);
        }
    }
}

/// Confirm every id names a todo item, in any project, and guard those rows.
///
/// A link is by id, and an id that names nothing would be a link to nothing.
/// Any project rather than the note's own: a note about one project may well
/// point at work in another.
pub fn require_todos(backend: &impl Backend, todo_ids: &[u64]) -> Result<Vec<Mutation>, String> {
    let mut found: BTreeSet<u64> = BTreeSet::new();
    let mut assertions = Vec::new();
    for chunk in todo_ids.chunks(IN_CHUNK) {
        let filter = Pred::is_in("id", chunk.iter().copied());
        let rows = fetch_all(
            backend,
            FetchRequest::entity("todo")
                .select(["id", "rev"])
                .filter(filter.clone()),
        )?;
        let revisions = rows
            .rows
            .iter()
            .filter_map(|row| {
                let id = id_at(&rows, row, "id")?;
                let rev = int_at(&rows, row, "rev")?;
                found.insert(id);
                Some((DataRowId::new(id as i64), DataRevision::new(rev)))
            })
            .collect();
        assertions.push(Mutation::AssertRows {
            entity: "todo".into(),
            filter,
            rows: revisions,
        });
    }
    if let Some(missing) = todo_ids.iter().find(|id| !found.contains(id)) {
        return Err(format!("no todo item {missing}; call todo_list"));
    }
    Ok(assertions)
}

/// A bounded search and whether more matches may exist.
#[derive(Debug, Default)]
pub struct SearchResult {
    pub notes: Vec<Note>,
    pub truncated: bool,
}

/// The notes whose title or body matches the query, most relevant first.
///
/// Two passes, as `todos::search`: full-text for whole words ranked by
/// relevance, then a substring pass only when the index found nothing, so a
/// fragment still finds the word it is part of. Archived notes match too: a
/// search is the way back to one.
pub fn search(backend: &impl Backend, scope: Scope, query: &str) -> Result<SearchResult, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(SearchResult::default());
    }
    let scoped = |request: FetchRequest| match scope.pred() {
        Some(pred) => request.filter(pred),
        None => request,
    };
    let limit = MAX_MATCHES as u32;
    let mut hits = backend.fetch(&scoped(
        FetchRequest::entity("note").search(query).limit(limit),
    ))?;
    if hits.is_empty() {
        hits = backend.fetch(&scoped(
            FetchRequest::entity("note")
                .filter(Pred::any([
                    Pred::contains("title", query),
                    Pred::contains("body", query),
                ]))
                .sort_by("updated_at", SortDir::Desc)
                .limit(limit),
        ))?;
    }
    let mut notes = from_rows(&hits);
    attach(backend, &mut notes)?;
    Ok(SearchResult {
        truncated: hits.truncated || notes.len() >= MAX_MATCHES,
        notes,
    })
}

/// The start of a body, on one line, for a search result.
pub fn snippet(body: &str) -> String {
    let one_line: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= SNIPPET_CHARS {
        return one_line;
    }
    let cut: String = one_line.chars().take(SNIPPET_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// The name of the project rooted at a note's root, for a reply. `None` for a
/// global note, and for a root whose project has been forgotten.
pub fn project_name(projects: &crate::projects::Registry, root: Option<&str>) -> Option<String> {
    let root = root?;
    projects
        .values()
        .find(|project| crate::todos::normalize_root(&project.root).ok().as_deref() == Some(root))
        .map(|project| project.name.clone())
}

/// A note as the model and the CLI read it.
///
/// Timestamps go out as the milliseconds the store keeps, plus how long ago the
/// last change was, which is what a reader usually wants and what the
/// presenter shows. `project` is the caller's to fill in: the rows hold roots.
pub fn note_json(note: &Note, now_ms: i64) -> serde_json::Value {
    let age_seconds = (now_ms - note.updated_at).max(0) / 1000;
    let mut value = serde_json::json!({
        "id": note.id,
        "root": note.root,
        "global": note.root.is_none(),
        "title": note.title,
        "author": note.author,
        "archived": note.archived,
        "tags": note.tags,
        "todo_ids": note.todo_ids,
        "created_at": note.created_at,
        "updated_at": note.updated_at,
        "age_seconds": age_seconds,
    });
    // The revision this note was read at, so an edit composed from it can say so and be
    // refused if somebody has since rewritten it. Left out when there is none.
    if note.rev > DataRevision::new(0) {
        value["rev"] = serde_json::json!(note.rev);
    }
    if !note.body.is_empty() {
        value["body"] = serde_json::json!(note.body);
    }
    value
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// A note as it will be written, after cleaning.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub root: Option<String>,
    pub title: String,
    pub body: String,
    pub author: String,
    pub tags: Vec<String>,
    pub todo_ids: Vec<u64>,
}

/// Plan adding a note: the one put that makes it, and the draft it was cleaned to.
///
/// Tags and links are not in the batch, because they need the note's id and the
/// store allocates it. [`plan_attach`] takes the id the apply reports. A call
/// that fails here writes nothing; a call whose attach fails leaves a bare note,
/// which the caller should say.
pub fn plan_add(
    count_in_scope: usize,
    root: Option<&str>,
    title: &str,
    body: &str,
    author: &str,
    tags: &[String],
    todo_ids: &[u64],
) -> Result<(Vec<Mutation>, Draft), String> {
    let title = clean_title(title)?;
    let body = clean_body(body)?;
    let author = normalize_author(author)?;
    let tags = normalize_tags(tags)?;
    let root = root.map(crate::todos::normalize_root).transpose()?;
    if count_in_scope >= MAX_NOTES_PER_SCOPE {
        return Err(format!(
            "this scope holds {MAX_NOTES_PER_SCOPE} notes, the most it can; delete some first"
        ));
    }
    let mut todo_ids: Vec<u64> = todo_ids.to_vec();
    todo_ids.sort_unstable();
    todo_ids.dedup();

    let mut put = Mutation::put("note")
        .set("title", title.clone())
        .set("body", body.clone())
        .set("author", author.clone())
        .set("archived", false);
    if let Some(root) = &root {
        put = put.set("root", root.clone());
    }
    Ok((
        vec![put.build()],
        Draft {
            root,
            title,
            body,
            author,
            tags,
            todo_ids,
        },
    ))
}

/// The tag and link rows for a note that now has an id. Empty for a note with
/// neither, so a caller can skip the second apply.
/// Assert both the note and todo revisions before writing side rows, so deletion
/// cannot leave dangling links or partial tags.
pub fn plan_attach(
    backend: &impl Backend,
    note_id: u64,
    tags: &[String],
    todo_ids: &[u64],
) -> Result<Vec<Mutation>, String> {
    if tags.is_empty() && todo_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = backend.fetch(
        &FetchRequest::entity("note")
            .select(LIST_COLUMNS.iter().copied())
            .filter(Pred::eq("id", note_id))
            .limit(1),
    )?;
    let note = from_rows(&rows)
        .into_iter()
        .next()
        .ok_or_else(|| format!("no note {note_id}; call note_list"))?;
    let mut mutations = require_todos(backend, todo_ids)?;
    mutations.extend(plan_attach_to_note(&note, tags, todo_ids));
    Ok(mutations)
}

/// The pure attachment plan shared by native callers and the Scheme store.
/// Callers prepend [`require_todos`]'s assertions for the linked todo rows.
pub fn plan_attach_to_note(note: &Note, tags: &[String], todo_ids: &[u64]) -> Vec<Mutation> {
    if tags.is_empty() && todo_ids.is_empty() {
        return Vec::new();
    }
    let note_id = note.id;
    let mut mutations = vec![Mutation::AssertRows {
        entity: "note".into(),
        filter: Pred::eq("id", note_id),
        rows: vec![(DataRowId::new(note_id as i64), note.rev)],
    }];
    mutations.extend(
        tags.iter().map(|tag| tag_mutation(note_id, tag)).chain(
            todo_ids
                .iter()
                .map(|todo_id| link_mutation(note_id, *todo_id)),
        ),
    );
    mutations
}

fn tag_mutation(note_id: u64, tag: &str) -> Mutation {
    Mutation::put("note_tag")
        .uid(tag_uid(note_id, tag))
        .set("note_id", note_id)
        .set("tag", tag)
        .build()
}

fn link_mutation(note_id: u64, todo_id: u64) -> Mutation {
    Mutation::put("note_link")
        .uid(link_uid(note_id, todo_id))
        .set("note_id", note_id)
        .set("todo_id", todo_id)
        .build()
}

/// Plan changing a note's title, body, or both, answering with the note as it
/// will read. Neither given is an error rather than a no-op write.
///
/// `expect_rev` is the revision the *author of the text* read, where the caller can say. It
/// takes precedence over the note's own `rev`, and the difference is the whole point: a note
/// fetched here is a note fetched microseconds ago, and the race worth catching is the one
/// where the wording on offer was composed from a reading several turns old. See
/// [`crate::todos::plan_edit`], which says the same at greater length.
pub fn plan_edit(
    note: &Note,
    title: Option<&str>,
    body: Option<&str>,
    expect_rev: Option<DataRevision>,
) -> Result<(Vec<Mutation>, Note), String> {
    if title.is_none() && body.is_none() {
        return Err("give a title, a body, or both".to_owned());
    }
    let mut after = note.clone();
    let mut update = Mutation::update("note", Pred::eq("id", note.id));
    // Guarded on the revision the note was read at, for the reason `todos::plan_edit` is: an
    // edit replaces a whole title or body with something composed from what the caller saw, so
    // landing it on a row somebody else has since edited destroys their wording. Archiving is
    // left unguarded — a flag two writers set the same way is not a disagreement.
    //
    // Zero means there is no row to guard: a note that came from the legacy file rather than
    // the store. It must never be allowed to mean "a projection dropped the column" — see
    // [`LIST_COLUMNS`], which reads `rev` for exactly that reason.
    if let Some(rev) = crate::store::guarded_rev(expect_rev, note.rev) {
        update = update.expect_rev(rev);
    }
    if let Some(title) = title {
        after.title = clean_title(title)?;
        update = update.set("title", after.title.clone());
    }
    if let Some(body) = body {
        after.body = clean_body(body)?;
        update = update.set("body", after.body.clone());
    }
    Ok((vec![update.build()], after))
}

/// Plan replacing a note's tags: every old row goes, every new one is put. One
/// batch, so a reader never sees the note untagged in between.
///
/// Guarded for [`plan_edit`]'s reason, and it is the same kind of write: the caller is not
/// adding a tag, it is replacing the whole set with one composed from what it read, so landing
/// that on a note somebody has since retagged silently drops their tags. `expect_rev` is the
/// revision the author of the set read.
///
/// The guard needs somewhere to live. The tag rows are a separate entity, a `Delete` cannot
/// carry `expect_rev`, and this batch would otherwise not touch the `note` row at all — so it
/// adds one: an `Update` of the note guarded on that revision. An `Update` that sets no
/// attributes is refused, and `updated_at` is a column the store assigns itself, so the carrier
/// is the title, rewritten to exactly what the author read. That is the claim the guard is
/// making, and it bumps `rev`, which is right: the tags are part of what a reader of a note is
/// shown, so changing them should stale anyone else's reading of it.
///
/// `guard` is the caller's promise that nothing else in the batch already guards this note.
/// Pass `false` when the same call also edits the title or body: [`plan_edit`] has emitted a
/// guarded update at that revision already, a second one would find the revision the first just
/// bumped, and the batch would refuse itself.
pub fn plan_set_tags(
    note: &Note,
    tags: &[String],
    expect_rev: Option<DataRevision>,
    guard: bool,
) -> Result<(Vec<Mutation>, Vec<String>), String> {
    let tags = normalize_tags(tags)?;
    let mut mutations = Vec::new();
    if guard {
        if let Some(rev) = crate::store::guarded_rev(expect_rev, note.rev) {
            mutations.push(
                Mutation::update("note", Pred::eq("id", note.id))
                    .expect_rev(rev)
                    .set("title", note.title.clone())
                    .build(),
            );
        }
    }
    mutations.push(Mutation::delete("note_tag", Pred::eq("note_id", note.id)));
    mutations.extend(tags.iter().map(|tag| tag_mutation(note.id, tag)));
    Ok((mutations, tags))
}

/// Plan archiving a note, or bringing it back.
pub fn plan_archive(note_id: u64, archived: bool) -> Vec<Mutation> {
    vec![Mutation::update("note", Pred::eq("id", note_id))
        .set("archived", archived)
        .build()]
}

/// Plan deleting a note with its tags and links, in one batch, so nothing is
/// left pointing at a row that is gone.
pub fn plan_delete(note_id: u64) -> Vec<Mutation> {
    vec![
        Mutation::delete("note_link", Pred::eq("note_id", note_id)),
        Mutation::delete("note_tag", Pred::eq("note_id", note_id)),
        Mutation::delete("note", Pred::eq("id", note_id)),
    ]
}

/// Plan linking a note to a todo item. An upsert, so linking twice is once.
pub fn plan_link(
    backend: &impl Backend,
    note_id: u64,
    todo_id: u64,
) -> Result<Vec<Mutation>, String> {
    plan_attach(backend, note_id, &[], &[todo_id])
}

/// Plan removing that link. Unlinking what was never linked deletes nothing.
pub fn plan_unlink(note_id: u64, todo_id: u64) -> Vec<Mutation> {
    vec![Mutation::delete(
        "note_link",
        Pred::all([Pred::eq("note_id", note_id), Pred::eq("todo_id", todo_id)]),
    )]
}

/// Plan moving a project's notes to a new root, as `todos::plan_move_root`
/// moves its list. Links are by id and need nothing.
pub fn plan_move_root(old: &str, new: &str) -> Result<Vec<Mutation>, String> {
    let old = crate::todos::normalize_root(old)?;
    let new = crate::todos::normalize_root(new)?;
    if old == new {
        return Ok(Vec::new());
    }
    Ok(vec![Mutation::update("note", Pred::eq("root", old))
        .set("root", new)
        .build()])
}

// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "unit/notes_attachments.rs"]
mod attachment_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    fn backend() -> store::Local {
        let backend = store::Local::shared("project_store");
        backend
            .define_all(&store::shared_defs())
            .expect("declared the shared entities");
        backend
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Write a note with its tags and links the way the tools do, and answer
    /// with its id.
    fn add(
        backend: &impl Backend,
        root: Option<&str>,
        title: &str,
        body: &str,
        tags: &[&str],
        todo_ids: &[u64],
    ) -> u64 {
        let scope = root.map_or(Scope::Global, Scope::Project);
        let count = count_in_scope(backend, scope).expect("counted");
        let (mutations, draft) =
            plan_add(count, root, title, body, "user", &strings(tags), todo_ids).expect("planned");
        let report = backend.apply(mutations).expect("wrote the note");
        let id = u64::try_from(report.ids[0].get()).unwrap();
        let attach = plan_attach(backend, id, &draft.tags, &draft.todo_ids).unwrap();
        if !attach.is_empty() {
            backend.apply(attach).expect("attached");
        }
        id
    }

    fn todo(backend: &impl Backend, root: &str, text: &str) -> u64 {
        let report = backend
            .apply(vec![Mutation::put("todo")
                .set("root", root)
                .set("text", text)
                .set("done", false)
                .build()])
            .expect("wrote a todo");
        u64::try_from(report.ids[0].get()).unwrap()
    }

    fn titles(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|note| note.title.as_str()).collect()
    }

    // ------------------------------------------------------------- cleaning

    #[test]
    fn a_title_is_one_line_and_a_body_keeps_its_lines() {
        assert_eq!(
            clean_title("  why\n\twe  did it \u{1b}").unwrap(),
            "why we did it"
        );
        assert_eq!(
            clean_body(" first\n\t- second\n").unwrap(),
            "first\n    - second"
        );
        assert!(clean_title(" \n ").is_err());
        assert!(clean_body("\u{1b}").is_err());
    }

    #[test]
    fn a_title_and_a_body_are_capped() {
        let long_title = "x".repeat(MAX_TITLE_CHARS + 1);
        assert!(clean_title(&long_title).unwrap_err().contains("characters"));
        let long_body = "y".repeat(MAX_BODY_BYTES + 1);
        assert!(clean_body(&long_body).unwrap_err().contains("bytes"));
    }

    #[test]
    fn body_limit_counts_cleaned_utf8_bytes() {
        for body in ["x".repeat(MAX_BODY_BYTES), "é".repeat(MAX_BODY_BYTES / 2)] {
            assert_eq!(clean_body(&body).unwrap(), body);
            assert_eq!(clean_body(&format!(" \u{1b}{body}\u{7}\n")).unwrap(), body);
            assert!(clean_body(&format!("{body}x"))
                .unwrap_err()
                .contains("8 MiB"));
        }
        // Tabs expand to four spaces before the size check.
        let body = format!("x\t{}", "y".repeat(MAX_BODY_BYTES - 5));
        assert_eq!(clean_body(&body).unwrap().len(), MAX_BODY_BYTES);
        assert!(clean_body(&format!("{body}y")).is_err());
    }

    #[test]
    fn large_notes_round_trip_and_edits_remain_guarded() {
        let backend = backend();
        let body = "é".repeat(MAX_BODY_BYTES / 2);
        let id = add(&backend, None, "large", &body, &[], &[]);
        let before = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(before.body, body);
        assert!(plan_edit(&before, None, Some(&format!("{body}x")), None).is_err());
        assert_eq!(fetch_note(&backend, id).unwrap().unwrap(), before);
        let replacement = "y".repeat(MAX_BODY_BYTES);
        backend
            .apply(
                plan_edit(&before, None, Some(&replacement), Some(before.rev))
                    .unwrap()
                    .0,
            )
            .unwrap();
        let after = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(after.body, replacement);
        assert!(after.rev > before.rev);
        assert!(backend
            .apply(
                plan_edit(&before, None, Some("stale"), Some(before.rev))
                    .unwrap()
                    .0
            )
            .unwrap_err()
            .is_conflict());
        assert_eq!(fetch_note(&backend, id).unwrap().unwrap(), after);
    }

    #[test]
    fn search_discloses_byte_and_match_limits() {
        let backend = backend();
        let body = format!("rebase {}", "x".repeat(MAX_BODY_BYTES - 7));
        for _ in 0..2 {
            add(&backend, None, "large", &body, &[], &[]);
        }
        for query in ["rebase", "rebas"] {
            let found = search(&backend, Scope::Global, query).unwrap();
            assert_eq!(found.notes.len(), 1);
            assert!(found.truncated, "{query}");
        }
        for _ in 0..MAX_MATCHES {
            add(&backend, Some("/small"), "small", "needle", &[], &[]);
        }
        let found = search(&backend, Scope::Project("/small"), "needle").unwrap();
        assert_eq!(found.notes.len(), MAX_MATCHES);
        assert!(found.truncated);
    }

    #[test]
    fn tags_are_lowercased_deduplicated_sorted_and_checked() {
        assert_eq!(
            normalize_tags(&strings(&["#Rust", "design", "rust", " Design "])).unwrap(),
            strings(&["design", "rust"])
        );
        assert!(normalize_tag("two words").is_err());
        assert!(normalize_tag("").is_err());
        let too_many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert!(normalize_tags(&too_many).unwrap_err().contains("at most"));
    }

    /// Two writers editing one note. The second edit was composed from a body the first has
    /// already replaced, so it is refused rather than overwriting it.
    #[test]
    fn an_edit_planned_from_a_stale_read_is_refused() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        let (mutations, _) = plan_add(0, None, "Why", "because", "user", &[], &[]).unwrap();
        let id = u64::try_from(local.apply(mutations).unwrap().ids[0].get()).unwrap();

        // Both read the same note.
        let mine = fetch_note(&local, id).unwrap().expect("stored");
        let theirs = mine.clone();

        local
            .apply(
                plan_edit(&theirs, None, Some("because of the deadline"), None)
                    .unwrap()
                    .0,
            )
            .unwrap();

        let error = local
            .apply(
                plan_edit(&mine, None, Some("because of the budget"), None)
                    .unwrap()
                    .0,
            )
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            fetch_note(&local, id).unwrap().unwrap().body,
            "because of the deadline",
            "the refused write should have changed nothing"
        );

        // Reading again is all it takes.
        let fresh = fetch_note(&local, id).unwrap().unwrap();
        local
            .apply(
                plan_edit(&fresh, None, Some("because of the budget"), None)
                    .unwrap()
                    .0,
            )
            .unwrap();
        assert_eq!(
            fetch_note(&local, id).unwrap().unwrap().body,
            "because of the budget"
        );
    }

    /// The window the guard has to span is the caller's, not the writer's — the note half of
    /// `todos::an_edit_guards_on_the_revision_its_author_read_not_the_writers`, and see that
    /// test for the reasoning.
    #[test]
    fn an_edit_guards_on_the_revision_its_author_read_not_the_writers() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        let (mutations, _) = plan_add(0, None, "Why", "because", "user", &[], &[]).unwrap();
        let id = u64::try_from(local.apply(mutations).unwrap().ids[0].get()).unwrap();

        // What the reader was shown, and the revision it was shown beside the body.
        let read = fetch_note(&local, id).unwrap().unwrap();
        let saw = read.rev;
        assert!(saw > DataRevision::new(0));
        assert_eq!(
            note_json(&read, 0)["rev"],
            serde_json::json!(saw),
            "and the reader is shown it"
        );

        // Somebody else rewrites it in the meantime.
        local
            .apply(
                plan_edit(&read, None, Some("because of the deadline"), None)
                    .unwrap()
                    .0,
            )
            .unwrap();

        // The write re-reads first, as every caller does, so guarding on the row's own
        // revision would let this through.
        let fresh = fetch_note(&local, id).unwrap().unwrap();
        let error = local
            .apply(
                plan_edit(&fresh, None, Some("because of the budget"), Some(saw))
                    .unwrap()
                    .0,
            )
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            fetch_note(&local, id).unwrap().unwrap().body,
            "because of the deadline",
            "the refused write should have changed nothing"
        );
    }

    /// A listing reads a named set of columns, and naming any drops the ones the store adds
    /// unasked. `rev` has to be among them: a note that came back with none is indistinguishable
    /// from one with no row to guard, and [`plan_edit`] would write it unguarded.
    #[test]
    fn a_listed_note_carries_a_revision_to_guard_on() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        let (mutations, _) = plan_add(0, None, "Why", "because", "user", &[], &[]).unwrap();
        let id = u64::try_from(local.apply(mutations).unwrap().ids[0].get()).unwrap();

        let listed = list_notes(&local, Filter::default()).unwrap();
        let listed = listed.iter().find(|note| note.id == id).expect("listed");
        assert_eq!(listed.rev, fetch_note(&local, id).unwrap().unwrap().rev);
        assert!(listed.rev > DataRevision::new(0));

        // And an edit planned from the listing is guarded, rather than quietly not being.
        local
            .apply(plan_edit(listed, Some("Why not"), None, None).unwrap().0)
            .unwrap();
        let error = local
            .apply(plan_edit(listed, Some("Why ever"), None, None).unwrap().0)
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
    }

    /// Archiving is deliberately not guarded, for the reason crossing a todo item off is not: a
    /// flag two writers set the same way is not a disagreement.
    #[test]
    fn archiving_is_not_guarded() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        let (mutations, _) = plan_add(0, None, "Why", "because", "user", &[], &[]).unwrap();
        let id = u64::try_from(local.apply(mutations).unwrap().ids[0].get()).unwrap();

        // Somebody edits the body, moving the revision on.
        let fresh = fetch_note(&local, id).unwrap().unwrap();
        local
            .apply(
                plan_edit(&fresh, None, Some("because it is late"), None)
                    .unwrap()
                    .0,
            )
            .unwrap();

        local.apply(plan_archive(id, true)).unwrap();
        let after = fetch_note(&local, id).unwrap().unwrap();
        assert!(after.archived);
        assert_eq!(after.body, "because it is late");
    }

    // ------------------------------------------------------------- planners

    #[test]
    fn planning_an_add_is_one_put_and_the_attach_is_the_rest() {
        let (mutations, draft) = plan_add(
            0,
            Some("/work/agent/"),
            "Why",
            "because",
            "agents",
            &strings(&["b", "a"]),
            &[7, 3, 7],
        )
        .unwrap();
        assert_eq!(
            mutations,
            vec![Mutation::put("note")
                .set("title", "Why")
                .set("body", "because")
                .set("author", "agents")
                .set("archived", false)
                .set("root", "/work/agent")
                .build()]
        );
        assert_eq!(draft.root.as_deref(), Some("/work/agent"));
        assert_eq!(draft.tags, strings(&["a", "b"]));
        assert_eq!(draft.todo_ids, vec![3, 7]);
        let backend = backend();
        let a = todo(&backend, "/work", "a");
        let b = todo(&backend, "/other", "b");
        let id = add(&backend, None, "note", "body", &[], &[]);
        let attach = plan_attach(&backend, id, &draft.tags, &[a, b]).unwrap();
        assert_eq!(attach.len(), 6);
        assert!(matches!(attach[0], Mutation::AssertRows { .. }));
        assert!(matches!(attach[1], Mutation::AssertRows { .. }));
        assert_eq!(attach[2].entity(), "note_tag");
        assert_eq!(attach[5].entity(), "note_link");
        assert!(plan_attach(&backend, id, &[], &[]).unwrap().is_empty());
        assert_eq!(
            plan_attach(&backend, id, &draft.tags, &[]).unwrap()[1..],
            vec![tag_mutation(id, "a"), tag_mutation(id, "b")]
        );
    }

    #[test]
    fn a_global_note_sets_no_root() {
        let (mutations, draft) = plan_add(0, None, "t", "b", "user", &[], &[]).unwrap();
        assert!(!format!("{mutations:?}").contains("root"));
        assert_eq!(draft.root, None);
    }

    #[test]
    fn a_full_scope_refuses_another_note() {
        let error = plan_add(MAX_NOTES_PER_SCOPE, None, "t", "b", "user", &[], &[]).unwrap_err();
        assert!(error.contains("the most"), "got: {error}");
    }

    #[test]
    fn deleting_a_note_takes_its_links_and_tags_in_one_batch() {
        let mutations = plan_delete(4);
        let entities: Vec<&str> = mutations.iter().map(Mutation::entity).collect();
        assert_eq!(entities, vec!["note_link", "note_tag", "note"]);
    }

    #[test]
    fn editing_nothing_is_refused() {
        let note = Note {
            id: 1,
            ..Default::default()
        };
        assert!(plan_edit(&note, None, None, None).is_err());
        let (mutations, after) = plan_edit(&note, Some(" New "), None, None).unwrap();
        assert_eq!(mutations.len(), 1);
        assert_eq!(after.title, "New");
    }

    #[test]
    fn moving_a_root_is_one_update_and_a_no_op_for_the_same_root() {
        assert!(plan_move_root("/a", "/a/").unwrap().is_empty());
        assert_eq!(
            plan_move_root("/a/b", "/a").unwrap(),
            vec![Mutation::update("note", Pred::eq("root", "/a/b"))
                .set("root", "/a")
                .build()]
        );
    }

    // --------------------------------------------------- through the store

    #[test]
    fn a_note_survives_a_round_trip_with_its_tags_and_links() {
        let backend = backend();
        let item = todo(&backend, "/work", "ship it");
        let id = add(
            &backend,
            Some("/work"),
            "Why",
            "# because\n\nreasons",
            &["Design", "rust"],
            &[item],
        );
        let note = fetch_note(&backend, id).unwrap().expect("found");
        assert_eq!(note.title, "Why");
        assert_eq!(note.body, "# because\n\nreasons");
        assert_eq!(note.root.as_deref(), Some("/work"));
        assert_eq!(note.author, "user");
        assert_eq!(note.tags, strings(&["design", "rust"]));
        assert_eq!(note.todo_ids, vec![item]);
        assert!(note.created_at > 0 && note.updated_at >= note.created_at);
        assert_eq!(fetch_note(&backend, id + 100).unwrap(), None);
    }

    #[test]
    fn a_listing_is_scoped_hides_archived_notes_and_omits_bodies() {
        let backend = backend();
        let project = add(&backend, Some("/work"), "in work", "b", &[], &[]);
        add(&backend, Some("/other"), "in other", "b", &[], &[]);
        let global = add(&backend, None, "global", "b", &[], &[]);
        backend.apply(plan_archive(global, true)).unwrap();

        let work = list_notes(
            &backend,
            Filter {
                scope: Some(Scope::Project("/work")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(titles(&work), vec!["in work"]);
        assert_eq!(work[0].body, "", "a listing does not read bodies");
        assert_eq!(work[0].id, project);

        let global_live = list_notes(
            &backend,
            Filter {
                scope: Some(Scope::Global),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            global_live.is_empty(),
            "archived notes are hidden by default"
        );
        let global_all = list_notes(
            &backend,
            Filter {
                scope: Some(Scope::Global),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(titles(&global_all), vec!["global"]);
        assert!(global_all[0].archived);

        let everything = list_notes(
            &backend,
            Filter {
                scope: Some(Scope::All),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(everything.len(), 3);
    }

    #[test]
    fn a_listing_narrows_by_tag_and_by_todo_item() {
        let backend = backend();
        let item = todo(&backend, "/work", "ship it");
        add(
            &backend,
            Some("/work"),
            "tagged and linked",
            "b",
            &["rust"],
            &[item],
        );
        add(&backend, Some("/work"), "tagged", "b", &["rust"], &[]);
        add(&backend, Some("/work"), "linked", "b", &[], &[item]);
        add(&backend, Some("/work"), "neither", "b", &[], &[]);

        let by_tag = list_notes(
            &backend,
            Filter {
                tag: Some("Rust"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_tag.len(), 2);
        let by_item = list_notes(
            &backend,
            Filter {
                todo_id: Some(item),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_item.len(), 2);
        let both = list_notes(
            &backend,
            Filter {
                tag: Some("rust"),
                todo_id: Some(item),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(titles(&both), vec!["tagged and linked"]);
        let none = list_notes(
            &backend,
            Filter {
                tag: Some("missing"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn search_finds_a_word_in_the_body_and_falls_back_to_a_fragment() {
        let backend = backend();
        add(
            &backend,
            Some("/work"),
            "Plan",
            "we should rebase before merging",
            &[],
            &[],
        );
        add(&backend, Some("/other"), "Rebase", "elsewhere", &[], &[]);
        let word = search(&backend, Scope::All, "rebase").unwrap();
        assert_eq!(word.notes.len(), 2, "title and body both match");
        assert!(!word.truncated);
        assert!(
            word.notes.iter().all(|note| !note.body.is_empty()),
            "search reads bodies"
        );
        let scoped = search(&backend, Scope::Project("/work"), "rebase").unwrap();
        assert_eq!(titles(&scoped.notes), vec!["Plan"]);
        let fragment = search(&backend, Scope::All, "merg").unwrap();
        assert_eq!(titles(&fragment.notes), vec!["Plan"]);
        assert!(search(&backend, Scope::All, "  ").unwrap().notes.is_empty());
    }

    #[test]
    fn deleting_a_note_removes_its_side_rows_and_unlinking_removes_one() {
        let backend = backend();
        let a = todo(&backend, "/work", "a");
        let b = todo(&backend, "/work", "b");
        let id = add(&backend, Some("/work"), "n", "b", &["t"], &[a, b]);

        backend.apply(plan_unlink(id, a)).unwrap();
        assert_eq!(fetch_note(&backend, id).unwrap().unwrap().todo_ids, vec![b]);
        backend.apply(plan_link(&backend, id, a).unwrap()).unwrap();
        backend.apply(plan_link(&backend, id, a).unwrap()).unwrap();
        assert_eq!(
            fetch_note(&backend, id).unwrap().unwrap().todo_ids,
            vec![a, b]
        );

        backend.apply(plan_delete(id)).unwrap();
        assert_eq!(fetch_note(&backend, id).unwrap(), None);
        assert!(notes_for_todo(&backend, a).unwrap().is_empty());
        assert!(backend
            .fetch(&FetchRequest::entity("note_tag").filter(Pred::eq("note_id", id)))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn setting_tags_replaces_them() {
        let backend = backend();
        let id = add(&backend, None, "n", "b", &["old"], &[]);
        let note = fetch_note(&backend, id).unwrap().unwrap();
        let (mutations, tags) =
            plan_set_tags(&note, &strings(&["new", "also"]), None, true).unwrap();
        assert_eq!(tags, strings(&["also", "new"]));
        backend.apply(mutations).unwrap();
        assert_eq!(
            fetch_note(&backend, id).unwrap().unwrap().tags,
            strings(&["also", "new"])
        );
    }

    /// A tag set is replaced whole, so it is the kind of write the guard is for: the set on
    /// offer was composed from a reading, and landing it on a note somebody has since retagged
    /// drops their tags without saying so. The batch carries no `note` row of its own, which is
    /// exactly why `plan_set_tags` has to add one.
    #[test]
    fn a_tag_replacement_guards_on_the_revision_its_author_read() {
        let backend = backend();
        let id = add(&backend, None, "n", "b", &["old"], &[]);
        let read = fetch_note(&backend, id).unwrap().unwrap();
        let saw = read.rev;
        assert!(saw > DataRevision::new(0));

        // Somebody else retags it in the meantime.
        backend
            .apply(
                plan_set_tags(&read, &strings(&["theirs"]), None, true)
                    .unwrap()
                    .0,
            )
            .unwrap();

        // The write re-reads first, as every caller does, so guarding on the row's own revision
        // would let this through.
        let fresh = fetch_note(&backend, id).unwrap().unwrap();
        let error = backend
            .apply(
                plan_set_tags(&fresh, &strings(&["mine"]), Some(saw), true)
                    .unwrap()
                    .0,
            )
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        let now = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(
            now.tags,
            strings(&["theirs"]),
            "the refused write should have changed nothing"
        );

        // And naming what it actually read gets it through.
        backend
            .apply(
                plan_set_tags(&now, &strings(&["mine"]), Some(now.rev), true)
                    .unwrap()
                    .0,
            )
            .unwrap();
        assert_eq!(
            fetch_note(&backend, id).unwrap().unwrap().tags,
            strings(&["mine"])
        );
    }

    /// One call that edits the text *and* the tags guards once, not twice. Two guarded updates
    /// of the same row in one batch is the second finding the revision the first just bumped,
    /// and the batch refusing itself — which would leave `note_edit` reporting a conflict
    /// nobody caused.
    #[test]
    fn a_text_edit_and_a_tag_replacement_in_one_batch_do_not_refuse_each_other() {
        let backend = backend();
        let id = add(&backend, None, "n", "b", &["old"], &[]);
        let read = fetch_note(&backend, id).unwrap().unwrap();

        let mut mutations = plan_edit(&read, Some("renamed"), None, Some(read.rev))
            .unwrap()
            .0;
        mutations.extend(
            plan_set_tags(&read, &strings(&["new"]), Some(read.rev), false)
                .unwrap()
                .0,
        );
        backend.apply(mutations).unwrap();

        let now = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(now.title, "renamed");
        assert_eq!(now.tags, strings(&["new"]));

        // The text edit's own guard still covers the batch: replaying it is refused, and the
        // tags do not move either.
        let error = backend
            .apply({
                let mut mutations = plan_edit(&read, Some("again"), None, Some(read.rev))
                    .unwrap()
                    .0;
                mutations.extend(
                    plan_set_tags(&read, &strings(&["later"]), Some(read.rev), false)
                        .unwrap()
                        .0,
                );
                mutations
            })
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        let now = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(now.title, "renamed");
        assert_eq!(now.tags, strings(&["new"]));
    }

    #[test]
    fn a_root_move_carries_only_that_roots_notes_and_keeps_links() {
        let backend = backend();
        let item = todo(&backend, "/work/sub", "x");
        let moved = add(&backend, Some("/work/sub"), "moved", "b", &[], &[item]);
        add(&backend, Some("/work"), "stays", "b", &[], &[]);
        backend
            .apply(plan_move_root("/work/sub", "/work").unwrap())
            .unwrap();
        let notes = list_notes(
            &backend,
            Filter {
                scope: Some(Scope::Project("/work")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes.len(), 2);
        assert!(list_notes(
            &backend,
            Filter {
                scope: Some(Scope::Project("/work/sub")),
                ..Default::default()
            }
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            fetch_note(&backend, moved).unwrap().unwrap().todo_ids,
            vec![item]
        );
    }

    #[test]
    fn todo_links_are_gathered_per_item_and_annotate_a_listing() {
        let backend = backend();
        let a = todo(&backend, "/work", "a");
        let b = todo(&backend, "/work", "b");
        let c = todo(&backend, "/work", "c");
        let n1 = add(&backend, Some("/work"), "n1", "b", &[], &[a, b]);
        let n2 = add(&backend, Some("/work"), "n2", "b", &[], &[a]);

        let links = links_for_todos(&backend, &[a, b, c]).unwrap();
        assert_eq!(links.get(&a), Some(&vec![n1, n2]));
        assert_eq!(links.get(&b), Some(&vec![n1]));
        assert_eq!(links.get(&c), None);
        assert!(links_for_todos(&backend, &[]).unwrap().is_empty());

        let mut listing = serde_json::json!({
            "items": [
                { "id": a, "text": "a", "children": [ { "id": b, "text": "b" } ] },
                { "id": c, "text": "c" },
            ]
        });
        assert_eq!(todo_ids_in(&listing), vec![a, b, c]);
        annotate_todo_list(&mut listing, &links);
        assert_eq!(listing["items"][0]["notes"], serde_json::json!([n1, n2]));
        assert_eq!(
            listing["items"][0]["children"][0]["notes"],
            serde_json::json!([n1])
        );
        assert!(listing["items"][1].get("notes").is_none());
    }

    #[test]
    fn a_link_to_a_missing_todo_is_refused() {
        let backend = backend();
        let item = todo(&backend, "/work", "a");
        assert!(require_todos(&backend, &[item]).is_ok());
        assert!(require_todos(&backend, &[]).is_ok());
        let error = require_todos(&backend, &[item, item + 50]).unwrap_err();
        assert!(
            error.contains(&format!("no todo item {}", item + 50)),
            "got: {error}"
        );
        assert!(plan_link(&backend, 1, item + 50).is_err());
        assert!(plan_attach(&backend, 1, &strings(&["tag"]), &[item + 50]).is_err());
    }

    #[test]
    fn linking_and_attachment_cannot_leave_links_to_deleted_todos() {
        use crate::todos;
        for attach in [false, true] {
            for recursive in [false, true] {
                for delete_first in [false, true] {
                    let backend = backend();
                    let other = backend.joined(backend.namespace(), "other");
                    let parent = todo(&backend, "/work", "parent");
                    let target = if recursive {
                        let list = todos::fetch_list(&backend, "/work").unwrap();
                        backend
                            .apply(
                                todos::plan_add(&list, "/work", "child", Some(parent))
                                    .unwrap()
                                    .0,
                            )
                            .unwrap()
                            .ids[0]
                            .get() as u64
                    } else {
                        parent
                    };
                    let survivor = todo(&backend, "/other", "keep");
                    let id = add(&backend, None, "note", "body", &[], &[]);
                    let link = if attach {
                        plan_attach(&backend, id, &strings(&["tag"]), &[survivor, target]).unwrap()
                    } else {
                        plan_link(&backend, id, target).unwrap()
                    };
                    let list = todos::fetch_list(&other, "/work").unwrap();
                    let delete = todos::plan_delete(&list, "/work", parent, recursive)
                        .unwrap()
                        .0;
                    if delete_first {
                        other.apply(delete).unwrap();
                        let error = backend.apply(link).unwrap_err();
                        assert!(error.is_conflict(), "{error:?}");
                        assert!(backend
                            .fetch(&FetchRequest::entity("note_tag"))
                            .unwrap()
                            .is_empty());
                        assert!(backend
                            .fetch(&FetchRequest::entity("note_link"))
                            .unwrap()
                            .is_empty());
                    } else {
                        backend.apply(link).unwrap();
                        other.apply(delete).unwrap();
                    }
                    assert!(
                        fetch_note(&backend, id).unwrap().is_some(),
                        "the created note survives"
                    );
                    // Read stored links directly: no join or display repair may hide a dangling ID.
                    let links = backend.fetch(&FetchRequest::entity("note_link")).unwrap();
                    for row in &links.rows {
                        assert_eq!(id_at(&links, row, "todo_id"), Some(survivor));
                    }
                    assert_eq!(links.rows.len(), usize::from(attach && !delete_first));
                }
            }
        }
    }

    #[test]
    fn attachment_assertions_cover_chunked_duplicate_ids_across_projects() {
        let backend = backend();
        let ids: Vec<u64> = backend
            .apply(
                (0..=IN_CHUNK)
                    .map(|n| {
                        Mutation::put("todo")
                            .set("root", if n % 2 == 0 { "/work" } else { "/other" })
                            .set("text", "item")
                            .build()
                    })
                    .collect(),
            )
            .unwrap()
            .ids
            .iter()
            .map(|id| id.get() as u64)
            .collect();
        let unrelated = todo(&backend, "/work", "unrelated");
        let id = add(&backend, Some("/notes"), "note", "body", &[], &[]);
        let mut duplicates = ids.clone();
        duplicates.extend_from_slice(&[ids[0], ids[0]]);
        let mutations = plan_attach(&backend, id, &strings(&["tag"]), &duplicates).unwrap();
        assert!(mutations[..2]
            .iter()
            .all(|m| matches!(m, Mutation::AssertRows { .. })));
        let other = backend.joined(backend.namespace(), "other");
        other
            .apply(vec![Mutation::update("todo", Pred::eq("id", unrelated))
                .set("text", "edited")
                .build()])
            .unwrap();
        backend.apply(mutations).unwrap();
        assert_eq!(fetch_note(&backend, id).unwrap().unwrap().todo_ids, ids);

        // Edits in either chunk invalidate the whole attachment, including its tags.
        for target in [ids[0], ids[IN_CHUNK]] {
            let pending = plan_attach(&backend, id, &strings(&["new"]), &duplicates).unwrap();
            other
                .apply(vec![Mutation::update("todo", Pred::eq("id", target))
                    .set("text", "edited")
                    .build()])
                .unwrap();
            let before = fetch_note(&backend, id).unwrap();
            assert!(backend.apply(pending).unwrap_err().is_conflict());
            assert_eq!(fetch_note(&backend, id).unwrap(), before);
        }
        let mut missing = ids;
        missing.push(unrelated + 100);
        assert!(require_todos(&backend, &missing)
            .unwrap_err()
            .contains("no todo item"));
    }

    #[test]
    fn side_rows_page_past_one_fetch() {
        let backend = backend();
        let id = add(&backend, None, "n", "b", &[], &[]);
        // More link rows than one fetch returns, all on one note.
        let mutations: Vec<Mutation> = (1..=(crate::store::LIST_FETCH_LIMIT as u64 + 5))
            .map(|todo_id| link_mutation(id, todo_id))
            .collect();
        backend.apply(mutations).unwrap();
        let note = fetch_note(&backend, id).unwrap().unwrap();
        assert_eq!(
            note.todo_ids.len(),
            crate::store::LIST_FETCH_LIMIT as usize + 5
        );
    }

    #[test]
    fn list_by_tag_chunks_past_one_in_predicate() {
        let backend = backend();
        // More tagged notes than one `IN` carries, spread over two scopes so
        // the per-scope cap does not bound the set.
        let total = IN_CHUNK + 5;
        for i in 0..total {
            let root = if i % 2 == 0 { Some("/a") } else { Some("/b") };
            add(&backend, root, &format!("n{i}"), "b", &["shared"], &[]);
        }
        let notes = list_notes(
            &backend,
            Filter {
                tag: Some("shared"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes.len(), total);
        assert!(
            notes
                .windows(2)
                .all(|pair| (pair[0].updated_at, pair[0].id) >= (pair[1].updated_at, pair[1].id)),
            "a narrowed listing is still newest first"
        );
    }

    #[test]
    fn the_json_carries_both_timestamps_and_an_age() {
        let note = Note {
            id: 3,
            title: "t".to_owned(),
            body: "b".to_owned(),
            author: "agents".to_owned(),
            created_at: 1_000,
            updated_at: 4_000,
            tags: strings(&["x"]),
            todo_ids: vec![9],
            ..Default::default()
        };
        let value = note_json(&note, 10_000);
        assert_eq!(value["age_seconds"], 6);
        assert_eq!(value["created_at"], 1_000);
        assert_eq!(value["updated_at"], 4_000);
        assert_eq!(value["global"], true);
        assert_eq!(value["body"], "b");
        let listed = note_json(
            &Note {
                body: String::new(),
                ..note
            },
            10_000,
        );
        assert!(
            listed.get("body").is_none(),
            "a listing has no body to show"
        );
    }

    #[test]
    fn a_snippet_is_one_line_and_cut() {
        assert_eq!(snippet("a\n  b\tc"), "a b c");
        let long = "word ".repeat(100);
        let cut = snippet(&long);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= SNIPPET_CHARS + 1);
    }

    #[test]
    fn a_scope_listing_still_fits_in_one_fetch() {
        assert!(MAX_NOTES_PER_SCOPE as u32 <= crate::store::LIST_FETCH_LIMIT);
    }
}
