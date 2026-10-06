//! Persistent per-project todo lists, keyed on the project root folder.
//!
//! The key is a path, not a project name. A name moves: `project_assign`
//! renames a project a poll found, and a shallower root absorbs the roots below
//! it. A list must stay with the folder.
//!
//! One invariant holds everywhere: if an item is open, every item above it is
//! open. So an item that is crossed off has a subtree that is fully crossed
//! off, and a list can hide that subtree without hiding open work.
//!
//! Every failure here is swallowed, as in the other `/cache` modules.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use yinz_tile::prelude::DataRevision;
use yinz_tile::prelude::DataRowId;
use yinz_tile::prelude::{DataRows, DataValue, FetchRequest, HostId, Mutation, Pred};

/// The file the lists lived in before the store. Read once, by the import.
pub const FILE: &str = "/cache/todos.json";

/// The most levels a list holds. A top item is level 1.
pub const MAX_DEPTH: usize = 5;
/// The most open items in one list. An item that is crossed off does not count.
pub const MAX_OPEN: usize = 200;
/// The most matches one search answers with.
pub const MAX_MATCHES: usize = 50;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// A saved item with no id, or with an id another item holds, gets a fresh
    /// one at load. So an id of 0 here means the file gave none.
    #[serde(default)]
    pub id: u64,
    pub text: String,
    /// A saved item from before this field existed has no key, so it loads as
    /// open. That is the safe direction: a load never crosses an item off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    /// The store's revision of this row when it was read, and what a write guards on so that it
    /// cannot silently replace an edit somebody else made in between. Zero on an item that came
    /// from the legacy file rather than the store, which has no row to guard.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rev: DataRevision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Item>,
}

fn is_zero(n: &DataRevision) -> bool {
    *n == DataRevision::new(0)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct List {
    /// The next id to give out. An id is unique in one list and never comes
    /// back after the item that held it went away.
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub items: Vec<Item>,
}

/// Project root to list.
pub type Store = BTreeMap<String, List>;

fn is_false(value: &bool) -> bool {
    !*value
}

/// An absolute path with no `.` or `..`, as the store keys it.
///
/// The same rule `projects::validate` puts on a project root, so a root that
/// the registry accepts always keys a list. One folder must give one key, or a
/// list goes missing.
pub fn normalize_root(root: &str) -> Result<String, String> {
    crate::paths::normalize(root)
        .map_err(|_| "project root must be an absolute path without . or ..".to_owned())
}

/// The list a `project` selector means, as `(root, name)`.
///
/// A list is keyed on the root folder, so a rename of the project keeps the
/// list. The selector takes an exact name, or an absolute root for a list whose
/// project is gone — in which case the name comes back as `None`, and is only
/// there to put in the answer.
pub fn resolve(
    projects: &crate::projects::Registry,
    selector: &str,
) -> Result<(String, Option<String>), String> {
    if selector.starts_with('/') {
        let root = normalize_root(selector)?;
        let name = projects
            .values()
            .find(|project| normalize_root(&project.root) == Ok(root.clone()))
            .map(|project| project.name.clone());
        return Ok((root, name));
    }
    match projects.get(selector) {
        Some(project) => Ok((normalize_root(&project.root)?, Some(project.name.clone()))),
        None => Err(format!("unknown project {selector}; call project_list")),
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Read the saved lists, and repair each one.
///
/// A file that a person edited can hold a repeated id, a missing id, text that
/// no list should show, or more items than a list holds. Each list is put back
/// into a good state rather than dropped, because the items are the user's
/// work. A key that is not a clean absolute path is dropped: nothing can find
/// that list again.
///
/// Named for what it is now: the reader of the file the lists used to live in,
/// kept for the one-time import. All the repairs stay, because that file is the
/// one that has been accumulating hand edits and half-writes, and the import is
/// the last time anything will look at it.
pub fn parse_legacy(json: &[u8]) -> Store {
    let saved: Store = serde_json::from_slice(json).unwrap_or_default();
    let mut store = Store::new();
    for (key, list) in saved {
        let Ok(root) = normalize_root(&key) else {
            continue;
        };
        // Two keys can normalize to one root, so the later list joins the
        // earlier one rather than replacing it.
        let entry = store.entry(root).or_default();
        entry.next_id = entry.next_id.max(list.next_id);
        entry.items.extend(list.items);
    }
    store
        .into_iter()
        .map(|(root, list)| (root, repair(list)))
        .filter(|(_, list)| !list.items.is_empty())
        .collect()
}

/// Put one saved list into a state the rest of this module can trust.
fn repair(list: List) -> List {
    let mut seen = BTreeSet::new();
    highest_id(&list.items, &mut seen);
    // A fresh id starts above every id the file holds, so a repair can never
    // give out an id that another item still has.
    let mut next = list
        .next_id
        .max(seen.iter().next_back().map_or(0, |id| id + 1))
        .max(1);
    let mut items = clean(list.items, 1, &mut next, &mut seen);
    open_parents(&mut items);
    let next_id = items
        .iter()
        .flat_map(all_ids)
        .max()
        .map_or(1, |highest| highest + 1);
    List { next_id, items }
}

/// Every id in a tree, so a repair knows which ones are taken.
fn highest_id(items: &[Item], seen: &mut BTreeSet<u64>) {
    for item in items {
        seen.insert(item.id);
        highest_id(&item.children, seen);
    }
}

fn all_ids(item: &Item) -> Vec<u64> {
    let mut ids = vec![item.id];
    for child in &item.children {
        ids.extend(all_ids(child));
    }
    ids
}

/// Drop what a list must not hold, and give a good id to what stays.
///
/// A bad item takes its own subtree with it. The alternative is to guess where
/// the children of a dropped item belong, and a guess about the user's work is
/// worse than a clean loss the file itself shows.
fn clean(items: Vec<Item>, depth: usize, next: &mut u64, seen: &mut BTreeSet<u64>) -> Vec<Item> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in items {
        let Ok(text) = clean_text(&item.text) else {
            continue;
        };
        // An id of 0 is the serde default, so it means the file had no id.
        let id = if item.id == 0 || !seen.remove(&item.id) {
            let id = *next;
            *next += 1;
            id
        } else {
            item.id
        };
        let children = clean(item.children, depth + 1, next, seen);
        out.push(Item {
            id,
            text,
            done: item.done,
            // Cleaning a list from the legacy file, which has no store rows behind it.
            rev: item.rev,
            children,
        });
    }
    out
}

/// The text of an item, or an error naming why it cannot be one.
///
/// Public for the notes, whose title and body are cleaned the same way.
pub fn clean_text(text: &str) -> Result<String, String> {
    let text = yinz_tile::text::sanitize(text);
    let text = text.trim();
    if text.is_empty() {
        return Err("todo text must contain visible characters".to_owned());
    }
    Ok(text.to_owned())
}

/// Open every item that has an open item below it.
fn open_parents(items: &mut [Item]) -> bool {
    let mut any_open = false;
    for item in items {
        if open_parents(&mut item.children) {
            item.done = false;
        }
        any_open |= !item.done;
    }
    any_open
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// The open items and all the items of a list.
pub fn counts(list: &List) -> (usize, usize) {
    let mut open = 0;
    let mut all = 0;
    count_into(&list.items, &mut open, &mut all);
    (open, all)
}

fn count_into(items: &[Item], open: &mut usize, all: &mut usize) {
    for item in items {
        *all += 1;
        if !item.done {
            *open += 1;
        }
        count_into(&item.children, open, all);
    }
}

/// One list as a tree the model reads back to the user.
///
/// Every item carries its id at every depth, so a call that crosses an item off
/// can name it from what the model just read.
pub fn list_json(store: &Store, root: &str, include_done: bool) -> serde_json::Value {
    let empty = List::default();
    let list = store.get(root).unwrap_or(&empty);
    let (open, all) = counts(list);
    serde_json::json!({
        "root": root,
        "open": open,
        "done": all - open,
        "include_done": include_done,
        "items": items_json(&list.items, include_done),
    })
}

fn items_json(items: &[Item], include_done: bool) -> Vec<serde_json::Value> {
    items
        .iter()
        // An item that is crossed off takes its subtree out of the answer. The
        // invariant makes that safe: nothing open is below it.
        .filter(|item| include_done || !item.done)
        .map(|item| {
            let mut value = serde_json::json!({ "id": item.id, "text": item.text });
            // The revision this text was read at, so an edit composed from it can say what it
            // was composed from and be refused if somebody has since rewritten the item. Left
            // out when there is none — an item from the legacy file has no row to guard.
            if item.rev > DataRevision::new(0) {
                value["rev"] = serde_json::json!(item.rev);
            }
            if include_done {
                value["done"] = serde_json::json!(item.done);
            }
            let children = items_json(&item.children, include_done);
            if !children.is_empty() {
                value["children"] = serde_json::json!(children);
            }
            value
        })
        .collect()
}

/// One item a search found.
pub struct Match {
    pub root: String,
    pub id: u64,
    pub text: String,
    pub done: bool,
    /// The texts of the items above this one, from the top down, this one last.
    pub trail: Vec<String>,
}

/// Fetch every page before building the tree, so parent edges can cross pages.
pub fn fetch_list(backend: &impl crate::store::Backend, root: &str) -> Result<List, String> {
    let rows = crate::store::fetch_all(
        backend,
        FetchRequest::entity("todo").filter(Pred::eq("root", root)),
    )?;
    Ok(from_rows(&rows).remove(root).unwrap_or_default())
}

/// The items whose text matches the query, newest-relevant first.
///
/// Two passes, because they mean different things. Full-text matches whole words
/// and ranks by relevance, which is what someone typing a word wants. But the
/// scan this replaced matched substrings, so `reba` used to find `rebase` — and
/// would silently stop. Hence the fallback, which is only paid for when the
/// index found nothing at all.
///
/// An item that is crossed off matches too: someone asking whether a list
/// already holds an item about a subject wants to hear about a finished one,
/// and each match says which it is.
///
/// `scope` of `None` searches every project.
pub fn search(
    backend: &impl crate::store::Backend,
    scope: Option<&str>,
    query: &str,
) -> Result<Vec<Match>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let scoped = |request: FetchRequest| match scope {
        Some(root) => request.filter(Pred::eq("root", root)),
        None => request,
    };
    let limit = MAX_MATCHES as u32;
    let mut hits = backend.fetch(&scoped(
        FetchRequest::entity("todo").search(query).limit(limit),
    ))?;
    if hits.is_empty() {
        hits = backend.fetch(&scoped(
            FetchRequest::entity("todo")
                .filter(Pred::contains("text", query))
                .limit(limit),
        ))?;
    }
    // Flat, not a tree: a matched parent and child are two results, and nesting
    // one inside the other would report only the parent.
    let found = items_from_rows(&hits);

    // A trail is the texts of the items above a match, which the rows do not
    // carry. So the lists the matches landed in are materialized — only those,
    // and never more of them than there were matches. Usually one.
    let mut lists: BTreeMap<String, List> = BTreeMap::new();
    for (root, _) in &found {
        if !lists.contains_key(root) {
            lists.insert(root.clone(), fetch_list(backend, root)?);
        }
    }

    Ok(found
        .into_iter()
        .map(|(root, item)| {
            let trail = lists
                .get(&root)
                .map(|list| trail_of(&list.items, item.id))
                .unwrap_or_default();
            Match {
                root,
                id: item.id,
                text: item.text,
                done: item.done,
                trail,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Add an item to a list, and answer with the id it got.
///
/// A parent is looked for in this list only. So a parent id that names an item
/// of another project is an error, and the message says why.
pub fn add(
    store: &mut Store,
    root: &str,
    text: &str,
    parent_id: Option<u64>,
) -> Result<u64, String> {
    let text = clean_text(text)?;
    // Everything is checked against the list as it is, before the store gets an
    // entry. So a call that fails leaves no empty list behind.
    let empty = List::default();
    let list = store.get(root).unwrap_or(&empty);
    let (open, _) = counts(list);
    if open >= MAX_OPEN {
        return Err(format!(
            "this project has {MAX_OPEN} open todo items; cross some off first"
        ));
    }
    if let Some(parent_id) = parent_id {
        let Some(depth) = depth_of(&list.items, parent_id, 1) else {
            return Err(format!(
                "no todo item {parent_id} in this project; parent_id must name an item in the \
                 same project — call todo_list"
            ));
        };
        if depth >= MAX_DEPTH {
            return Err(format!("todo items nest {MAX_DEPTH} levels deep at most"));
        }
    }

    let list = store.entry(root.to_owned()).or_default();
    let id = list.next_id.max(1);
    list.next_id = id + 1;
    let item = Item {
        id,
        text,
        done: false,
        // Not from a store row: this is the legacy file being read in.
        rev: DataRevision::new(0),
        children: Vec::new(),
    };
    match parent_id {
        Some(parent_id) => {
            push_under(&mut list.items, parent_id, item);
            // The new item is open, so nothing above it can stay crossed off.
            open_parents(&mut list.items);
        },
        None => list.items.push(item),
    }
    Ok(id)
}

fn depth_of(items: &[Item], id: u64, depth: usize) -> Option<usize> {
    for item in items {
        if item.id == id {
            return Some(depth);
        }
        if let Some(found) = depth_of(&item.children, id, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn push_under(items: &mut [Item], parent_id: u64, item: Item) -> Option<Item> {
    let mut item = item;
    for candidate in items {
        if candidate.id == parent_id {
            candidate.children.push(item);
            return None;
        }
        item = push_under(&mut candidate.children, parent_id, item)?;
    }
    Some(item)
}

/// What one call to [`set_done`] changed.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub id: u64,
    pub text: String,
    pub done: bool,
    /// The items below this one that the call also crossed off.
    pub cascaded: Vec<u64>,
    /// The items above this one that the call also opened.
    pub reopened: Vec<u64>,
}

/// Cross an item off, or open it again.
///
/// Crossing an item off crosses off the items below it: work is not finished
/// while a part of it is open. Opening an item again does not open the items
/// below it, because an item that comes back says nothing about work already
/// finished. It does open the items above it, which the invariant needs.
pub fn set_done(store: &mut Store, root: &str, id: u64, done: bool) -> Result<Change, String> {
    let Some(list) = store.get_mut(root) else {
        return Err(format!("no todo item {id} in this project; call todo_list"));
    };
    let Some(text) = find_item(&list.items, id).map(|item| item.text.clone()) else {
        return Err(format!("no todo item {id} in this project; call todo_list"));
    };

    let mut cascaded = Vec::new();
    let mut reopened = Vec::new();
    apply(&mut list.items, id, done, &mut cascaded, &mut reopened);
    // The recursion finds the closest item above first, so turn it around to
    // read from the top down.
    reopened.reverse();
    Ok(Change {
        id,
        text,
        done,
        cascaded,
        reopened,
    })
}

/// One item, by id, wherever it sits in the tree. For reporting what an item says now — and at
/// what revision — after a guarded edit was refused.
pub fn item_of(list: &List, id: u64) -> Option<&Item> {
    find_item(&list.items, id)
}

/// Just the text of [`item_of`], for a caller that has nothing to say about revisions.
pub fn text_of(list: &List, id: u64) -> Option<String> {
    item_of(list, id).map(|item| item.text.clone())
}

fn find_item(items: &[Item], id: u64) -> Option<&Item> {
    for item in items {
        if item.id == id {
            return Some(item);
        }
        if let Some(found) = find_item(&item.children, id) {
            return Some(found);
        }
    }
    None
}

/// Find the item and change it. Answers whether this branch held it, so the
/// items above it can be opened as the recursion unwinds.
fn apply(
    items: &mut [Item],
    id: u64,
    done: bool,
    cascaded: &mut Vec<u64>,
    reopened: &mut Vec<u64>,
) -> bool {
    for item in items {
        if item.id == id {
            item.done = done;
            if done {
                cascade(&mut item.children, cascaded);
            }
            return true;
        }
        if apply(&mut item.children, id, done, cascaded, reopened) {
            // Only an item that was crossed off is a change worth reporting.
            if !done && item.done {
                item.done = false;
                reopened.push(item.id);
            }
            return true;
        }
    }
    false
}

fn cascade(items: &mut [Item], cascaded: &mut Vec<u64>) {
    for item in items {
        if !item.done {
            item.done = true;
            cascaded.push(item.id);
        }
        cascade(&mut item.children, cascaded);
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------
//
// The store keeps one row per item and holds the tree as `parent_id` edges. The
// nested `List` above survives as an in-memory materialization of one project's
// rows, and every function that carries the open-above-done invariant keeps
// working on it unchanged.
//
// Writes go the other way: plan against the materialized list using the
// existing tree functions, then translate what they did into mutations. The
// cascade and the open-ancestors rule are not reimplemented against predicates,
// because there would then be two copies of the invariant to keep in step.

/// The columns a materialization needs. Ordering is `id` ascending, which is
/// insertion order, which is sibling order.
pub const ROW_COLUMNS: &[&str] = &["id", "root", "parent_id", "text", "done"];

/// One item waiting to be inserted, still carrying the ids of the tree it came
/// from. See [`levels_for_import`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub old_id: u64,
    pub old_parent: Option<u64>,
    pub text: String,
    pub done: bool,
}

fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<String> {
    match rows.get(row, column) {
        Some(DataValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<u64> {
    match rows.get(row, column) {
        Some(DataValue::Int(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn bool_at(rows: &DataRows, row: &[DataValue], column: &str) -> bool {
    matches!(rows.get(row, column), Some(DataValue::Bool(true)))
}

/// The items these rows describe, flat, each with the root it belongs to.
///
/// For a set of rows that is not a whole list — search hits, say. Building a
/// tree out of those would be wrong twice over: an item whose parent did not
/// match would look top-level, and a matched parent and child would nest, hiding
/// one of the two results inside the other.
pub fn items_from_rows(rows: &DataRows) -> Vec<(String, Item)> {
    rows.rows
        .iter()
        .filter_map(|row| {
            let (Some(id), Some(root), Some(text)) = (
                int_at(rows, row, "id"),
                text_at(rows, row, "root"),
                text_at(rows, row, "text"),
            ) else {
                return None;
            };
            Some((
                root,
                Item {
                    id,
                    text,
                    done: bool_at(rows, row, "done"),
                    rev: int_at(rows, row, "rev")
                        .and_then(|rev| DataRevision::try_from(rev).ok())
                        .unwrap_or_default(),
                    children: Vec::new(),
                },
            ))
        })
        .collect()
}

/// Build the lists these rows describe, grouped by project root.
///
/// An item whose `parent_id` names no row in the set becomes a top-level item
/// rather than disappearing. Two reasons: a fetch scoped to one root sees a
/// parent in another root as absent, and a row left behind by a future delete
/// would otherwise take its subtree with it. Losing the nesting is recoverable;
/// losing the user's item is not.
pub fn from_rows(rows: &DataRows) -> Store {
    let mut store = Store::new();
    // Flat first, then linked, because a child can appear before its parent
    // only if something reordered the fetch — but nothing here should depend on
    // that not happening.
    let mut roots: BTreeMap<u64, String> = BTreeMap::new();
    let mut items: BTreeMap<u64, Item> = BTreeMap::new();
    let mut parents: BTreeMap<u64, u64> = BTreeMap::new();
    let mut order: Vec<u64> = Vec::new();

    for row in &rows.rows {
        let (Some(id), Some(root), Some(text)) = (
            int_at(rows, row, "id"),
            text_at(rows, row, "root"),
            text_at(rows, row, "text"),
        ) else {
            continue;
        };
        if let Some(parent) = int_at(rows, row, "parent_id") {
            parents.insert(id, parent);
        }
        roots.insert(id, root);
        items.insert(
            id,
            Item {
                id,
                text,
                done: bool_at(rows, row, "done"),
                rev: int_at(rows, row, "rev")
                    .and_then(|rev| DataRevision::try_from(rev).ok())
                    .unwrap_or_default(),
                children: Vec::new(),
            },
        );
        order.push(id);
    }

    // A parent in another project is not a parent. Resolving this before the
    // walk keeps the walk from having to know about roots at all.
    parents.retain(|id, parent| roots.get(id) == roots.get(parent));

    for id in &order {
        if is_cyclic(*id, &parents) {
            parents.remove(id);
        }
    }

    // Deepest first, so a child is finished before it is moved into its parent.
    let mut by_depth: Vec<u64> = order.clone();
    by_depth.sort_by_key(|id| std::cmp::Reverse(depth_via(*id, &parents)));
    for id in by_depth {
        let Some(parent) = parents.get(&id).copied() else {
            continue;
        };
        let Some(child) = items.remove(&id) else {
            continue;
        };
        if let Some(parent) = items.get_mut(&parent) {
            parent.children.push(child);
        }
    }

    // Siblings were pushed deepest-first, which says nothing about their order
    // among themselves; `id` does.
    for id in &order {
        if let Some(item) = items.get_mut(id) {
            sort_children(item);
        }
    }

    for id in order {
        let (Some(item), Some(root)) = (items.remove(&id), roots.get(&id)) else {
            continue;
        };
        store.entry(root.clone()).or_default().items.push(item);
    }
    store
}

fn sort_children(item: &mut Item) {
    item.children.sort_by_key(|child| child.id);
    for child in &mut item.children {
        sort_children(child);
    }
}

/// How far below a top-level item this one sits, for ordering the walk.
fn depth_via(id: u64, parents: &BTreeMap<u64, u64>) -> usize {
    let mut depth = 0;
    let mut at = id;
    // Bounded by the number of edges: `is_cyclic` has already run, but this must
    // terminate even if it had not.
    while let Some(parent) = parents.get(&at) {
        depth += 1;
        at = *parent;
        if depth > parents.len() {
            break;
        }
    }
    depth
}

fn is_cyclic(id: u64, parents: &BTreeMap<u64, u64>) -> bool {
    let mut seen = BTreeSet::new();
    let mut at = id;
    while let Some(parent) = parents.get(&at) {
        if !seen.insert(at) {
            return true;
        }
        at = *parent;
    }
    false
}

/// The ancestors of `id` that are crossed off, closest first.
///
/// Adding an open item below a finished one has to reopen everything above it,
/// or the invariant that a crossed-off item has no open work under it breaks.
pub fn ancestors_to_open(items: &[Item], id: u64) -> Vec<u64> {
    let mut trail = Vec::new();
    if !collect_trail(items, id, &mut trail) {
        return Vec::new();
    }
    // `collect_trail` includes the item itself, which is not an ancestor.
    trail.pop();
    trail
        .into_iter()
        .rev()
        .filter(|(_, done)| *done)
        .map(|(id, _)| id)
        .collect()
}

/// Walk to `id`, recording `(id, done)` from the top down. Answers whether this
/// branch held it.
fn collect_trail(items: &[Item], id: u64, trail: &mut Vec<(u64, bool)>) -> bool {
    for item in items {
        trail.push((item.id, item.done));
        if item.id == id || collect_trail(&item.children, id, trail) {
            return true;
        }
        trail.pop();
    }
    false
}

/// The texts of the items above `id`, from the top down, `id` last.
///
/// This is what a search result carries. Rebuilt from a materialized list rather
/// than stored, because a trail is a fact about the tree and would go stale the
/// moment an item above was renamed.
pub fn trail_of(items: &[Item], id: u64) -> Vec<String> {
    let mut trail = Vec::new();
    if !collect_text_trail(items, id, &mut trail) {
        return Vec::new();
    }
    trail
}

fn collect_text_trail(items: &[Item], id: u64, trail: &mut Vec<String>) -> bool {
    for item in items {
        trail.push(item.text.clone());
        if item.id == id || collect_text_trail(&item.children, id, trail) {
            return true;
        }
        trail.pop();
    }
    false
}

/// Guard the exact project list used to validate a topology or completion change.
fn assert_snapshot(list: &List, root: &str) -> Mutation {
    fn collect(items: &[Item], rows: &mut Vec<(DataRowId, DataRevision)>) {
        for item in items {
            rows.push((DataRowId::new(item.id as i64), item.rev));
            collect(&item.children, rows);
        }
    }
    let mut rows = Vec::new();
    collect(&list.items, &mut rows);
    Mutation::AssertRows {
        entity: "todo".into(),
        filter: Pred::eq("root", root),
        rows,
    }
}

/// Todo snapshot conflicts require fresh validation, never an automatic retry.
pub fn topology_error(error: yinz_tile::prelude::WriteError) -> String {
    if error.is_conflict() {
        "the todo list changed; reload and retry".into()
    } else {
        error.into()
    }
}

/// Plan adding an item, answering with the writes that do it.
///
/// The checks are the ones [`add`] makes, against the same materialized list, so
/// a call that fails writes nothing. The new item's id is not known here — the
/// store allocates it, and `ApplyReport::ids` hands it back.
pub fn plan_add(
    list: &List,
    root: &str,
    text: &str,
    parent_id: Option<u64>,
) -> Result<(Vec<Mutation>, String), String> {
    let text = clean_text(text)?;
    let (open, _) = counts(list);
    if open >= MAX_OPEN {
        return Err(format!(
            "this project has {MAX_OPEN} open todo items; cross some off first"
        ));
    }
    if let Some(parent_id) = parent_id {
        let Some(depth) = depth_of(&list.items, parent_id, 1) else {
            return Err(format!(
                "no todo item {parent_id} in this project; parent_id must name an item in the \
                 same project — call todo_list"
            ));
        };
        if depth >= MAX_DEPTH {
            return Err(format!("todo items nest {MAX_DEPTH} levels deep at most"));
        }
    }

    let mut put = Mutation::put("todo")
        .set("root", root)
        .set("text", text.clone())
        .set("done", false);
    if let Some(parent_id) = parent_id {
        put = put.set("parent_id", parent_id);
    }
    let mut mutations = vec![assert_snapshot(list, root), put.build()];

    // The new item is open, so nothing above it may stay crossed off.
    if let Some(parent_id) = parent_id {
        let reopen = ancestors_to_open(&list.items, parent_id);
        let reopen: Vec<u64> = std::iter::once(parent_id)
            .filter(|_| is_done(&list.items, parent_id))
            .chain(reopen)
            .collect();
        if !reopen.is_empty() {
            mutations.push(open_mutation(&reopen));
        }
    }
    Ok((mutations, text))
}

fn is_done(items: &[Item], id: u64) -> bool {
    for item in items {
        if item.id == id {
            return item.done;
        }
        if is_done(&item.children, id) {
            return true;
        }
    }
    false
}

fn open_mutation(ids: &[u64]) -> Mutation {
    Mutation::update("todo", Pred::is_in("id", ids.iter().copied()))
        .set("done", false)
        .build()
}

/// Plan crossing an item off, or opening it again.
///
/// [`set_done`] does the thinking, on a copy of the list, and what it reports
/// having changed becomes the writes. One batch, so the item, the subtree it
/// cascades to and the ancestors it reopens all move together or not at all.
/// The project snapshot guards those decisions, including against unrelated edits
/// and agreeing flag writes that advance a revision.
pub fn plan_set_done(
    list: &List,
    root: &str,
    id: u64,
    done: bool,
) -> Result<(Vec<Mutation>, Change), String> {
    let mut store = Store::new();
    store.insert(root.to_owned(), list.clone());
    let change = set_done(&mut store, root, id, done)?;

    let mut mutations = vec![assert_snapshot(list, root)];
    // The item itself and everything the cascade reached take the same value.
    let mut same: Vec<u64> = vec![change.id];
    same.extend(change.cascaded.iter().copied());
    mutations.push(
        Mutation::update("todo", Pred::is_in("id", same.iter().copied()))
            .set("done", done)
            .build(),
    );
    if !change.reopened.is_empty() {
        mutations.push(open_mutation(&change.reopened));
    }
    Ok((mutations, change))
}

/// Plan changing an item's text.
///
/// One update over the single row the id names — ids are unique in a list, so no
/// root scoping is needed beyond confirming the item is in this list at all.
///
/// `expect_rev` is the revision the *author of the text* read, where the caller can say so.
/// It takes precedence over the item's own, and that is the point: `list` was fetched
/// microseconds ago by the writer itself, while the wording on offer was composed from a
/// reading that may be several turns old. See [`crate::store::guarded_rev`].
pub fn plan_edit(
    list: &List,
    id: u64,
    text: &str,
    expect_rev: Option<DataRevision>,
) -> Result<(Vec<Mutation>, String), String> {
    let text = clean_text(text)?;
    let Some(item) = find_item(&list.items, id) else {
        return Err(format!("no todo item {id} in this project; call todo_list"));
    };
    // Guarded on the revision the text was composed from. An edit replaces the whole of an
    // item's text with something composed from what the caller saw, so a write that lands on a
    // row somebody else has since edited destroys their wording outright — and the store is
    // shared by every session, and with Postgres by every machine, pointed at it.
    //
    // Completion instead guards the whole project snapshot: its cascade and reopening
    // decisions depend on the tree, even when two writers agree on the flag itself.
    //
    // A `rev` of 0 means the item did not come from a store row — the legacy file — so there is
    // nothing to guard against and an unguarded write is the only one that can work. It must
    // never come to mean a fetch that projected the column away.
    let mut update = Mutation::update("todo", Pred::eq("id", id)).set("text", text.clone());
    if let Some(rev) = crate::store::guarded_rev(expect_rev, item.rev) {
        update = update.expect_rev(rev);
    }
    Ok((vec![update.build()], text))
}

/// Delete the selected subtree and its links in one batch, leaving notes intact.
pub fn plan_delete(
    list: &List,
    root: &str,
    id: u64,
    recursive: bool,
) -> Result<(Vec<Mutation>, Vec<u64>), String> {
    let item = item_of(list, id)
        .ok_or_else(|| format!("no todo item {id} in this project; call todo_list"))?;
    if !recursive && !item.children.is_empty() {
        return Err(format!(
            "todo item {id} has subtasks; use recursive to delete the subtree"
        ));
    }
    fn collect(item: &Item, ids: &mut Vec<u64>) {
        ids.push(item.id);
        for child in &item.children {
            collect(child, ids);
        }
    }
    let mut ids = Vec::new();
    collect(item, &mut ids);
    Ok((
        vec![
            assert_snapshot(list, root),
            Mutation::delete("note_link", Pred::is_in("todo_id", ids.iter().copied())),
            Mutation::delete("todo", Pred::is_in("id", ids.iter().copied())),
        ],
        ids,
    ))
}

/// Reparent only the subtree root. Descendants keep their IDs, state and links.
pub fn plan_move(
    list: &List,
    root: &str,
    id: u64,
    parent_id: Option<u64>,
    expect_rev: Option<DataRevision>,
) -> Result<(Vec<Mutation>, Vec<u64>), String> {
    let item = item_of(list, id)
        .ok_or_else(|| format!("no todo item {id} in this project; call todo_list"))?;
    fn height(item: &Item) -> usize {
        1 + item.children.iter().map(height).max().unwrap_or(0)
    }
    let mut reopened = Vec::new();
    if let Some(parent_id) = parent_id {
        let parent_depth = depth_of(&list.items, parent_id, 1)
            .ok_or_else(|| format!("no todo item {parent_id} in this project; parent_id must name an item in the same project"))?;
        if id == parent_id || find_item(&item.children, parent_id).is_some() {
            return Err("a todo cannot move under itself or its descendants".into());
        }
        if parent_depth + height(item) > MAX_DEPTH {
            return Err(format!("todo items nest {MAX_DEPTH} levels deep at most"));
        }
        if !item.done {
            if is_done(&list.items, parent_id) {
                reopened.push(parent_id);
            }
            reopened.extend(ancestors_to_open(&list.items, parent_id));
        }
    }
    let mut update = Mutation::update("todo", Pred::eq("id", id)).set(
        "parent_id",
        parent_id.map(DataValue::from).unwrap_or(DataValue::Null),
    );
    if let Some(rev) = crate::store::guarded_rev(expect_rev, item.rev) {
        update = update.expect_rev(rev);
    }
    let mut mutations = vec![assert_snapshot(list, root), update.build()];
    if !reopened.is_empty() {
        mutations.push(open_mutation(&reopened));
    }
    Ok((mutations, reopened))
}

/// Plan moving a project's list to a new root.
///
/// One update over every row of the old root. The old file-backed version had to
/// merge two lists and renumber the ids that collided; nothing collides now,
/// because the ids belong to the store rather than to the list.
pub fn plan_move_root(old: &str, new: &str) -> Result<Vec<Mutation>, String> {
    let old = normalize_root(old)?;
    let new = normalize_root(new)?;
    if old == new {
        return Ok(Vec::new());
    }
    Ok(vec![Mutation::update("todo", Pred::eq("root", old))
        .set("root", new)
        .build()])
}

/// A whole list broken into the levels an import inserts it in, shallowest
/// first.
///
/// A child's `parent_id` is the store id of its parent, which is only known once
/// the parent has been written — so an import is one apply per level, threading
/// the ids it gets back. Five levels at most, and only ever on the one-time
/// import.
pub fn levels_for_import(list: &List) -> Vec<Vec<Pending>> {
    let mut levels: Vec<Vec<Pending>> = Vec::new();
    collect_level(&list.items, None, 0, &mut levels);
    levels
}

fn collect_level(
    items: &[Item],
    parent: Option<u64>,
    depth: usize,
    levels: &mut Vec<Vec<Pending>>,
) {
    if items.is_empty() {
        return;
    }
    if levels.len() <= depth {
        levels.push(Vec::new());
    }
    for item in items {
        levels[depth].push(Pending {
            old_id: item.id,
            old_parent: parent,
            text: item.text.clone(),
            done: item.done,
        });
    }
    // Breadth first, so every item of a level is recorded before any of the next
    // — which is what makes one apply per level correct.
    for item in items {
        collect_level(&item.children, Some(item.id), depth + 1, levels);
    }
}

/// Stable identity for an imported row, scoped to its source host and list.
pub fn import_uid(host_id: &HostId, root: &str, old_id: u64) -> String {
    serde_json::json!([host_id, root, old_id]).to_string()
}

/// Insert one legacy level against a fresh list, refusing missing or moved parents.
pub fn import_mutations(
    list: &List,
    root: &str,
    level: &[Pending],
    new_ids: &BTreeMap<u64, DataRowId>,
    host_id: &HostId,
) -> Result<Vec<Mutation>, String> {
    let mut mutations = vec![assert_snapshot(list, root)];
    let mut reopened = Vec::new();
    for pending in level {
        let mut put = Mutation::put("todo")
            .uid(import_uid(host_id, root, pending.old_id))
            .expect_absent()
            .set("root", root)
            .set("text", pending.text.clone())
            .set("done", pending.done);
        if let Some(old_parent) = pending.old_parent {
            let parent = new_ids
                .get(&old_parent)
                .ok_or("the imported todo parent is missing; reload and retry")?;
            let parent_id = parent.get() as u64;
            let depth = depth_of(&list.items, parent_id, 1)
                .ok_or("the imported todo parent is missing; reload and retry")?;
            if depth >= MAX_DEPTH {
                return Err(format!("todo items nest {MAX_DEPTH} levels deep at most"));
            }
            put = put.set("parent_id", *parent);
            // A committed ancestor may have been completed before this level resumed.
            if !pending.done {
                if is_done(&list.items, parent_id) {
                    reopened.push(parent_id);
                }
                reopened.extend(ancestors_to_open(&list.items, parent_id));
            }
        }
        mutations.push(put.build());
    }
    if !reopened.is_empty() {
        reopened.sort_unstable();
        reopened.dedup();
        mutations.push(open_mutation(&reopened));
    }
    Ok(mutations)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, text: &str, children: Vec<Item>) -> Item {
        Item {
            id,
            text: text.to_owned(),
            done: false,
            rev: DataRevision::new(0),
            children,
        }
    }

    fn done(item: Item) -> Item {
        Item { done: true, ..item }
    }

    fn store(root: &str, items: Vec<Item>) -> Store {
        let next_id = items.iter().flat_map(all_ids).max().unwrap_or(0) + 1;
        Store::from([(root.to_owned(), List { next_id, items })])
    }

    fn roots(roots: &[&str]) -> Vec<String> {
        roots.iter().map(|root| (*root).to_owned()).collect()
    }

    /// Serialize a store the way the old file did, so `parse_legacy` can be
    /// tested against input in the shape it will actually meet. Only the tests
    /// write this format now.
    fn json(store: &Store) -> Option<String> {
        serde_json::to_string(store).ok()
    }

    // ----------------------------------------------------------------- rows

    use crate::store::{self, Backend};
    use yinz_tile::prelude::FetchRequest;

    /// Rows as the store would hand them back, without a store.
    fn rows(columns: &[&str], values: Vec<Vec<DataValue>>) -> DataRows {
        DataRows {
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: values,
            truncated: false,
            next_cursor: None,
        }
    }

    fn row(id: i64, root: &str, parent: Option<i64>, text: &str, done: bool) -> Vec<DataValue> {
        vec![
            DataValue::Int(id),
            DataValue::Text(root.to_owned()),
            parent.map_or(DataValue::Null, DataValue::Int),
            DataValue::Text(text.to_owned()),
            DataValue::Bool(done),
        ]
    }

    /// A backend with the todo entity declared, for the round trips below.
    fn backend() -> store::Local {
        let backend = store::Local::shared("project_store");
        backend
            .define_all(&store::shared_defs())
            .expect("declared the shared entities");
        backend
    }

    /// Read a whole project's list back out of the store.
    fn materialize(backend: &impl Backend, root: &str) -> List {
        fetch_list(backend, root).expect("fetched")
    }

    /// Write a whole list in, level by level, as the import will.
    fn import(backend: &impl Backend, root: &str, list: &List) {
        let mut new_ids: BTreeMap<u64, DataRowId> = BTreeMap::new();
        for level in levels_for_import(list) {
            let mutations = import_mutations(
                &fetch_list(backend, root).unwrap(),
                root,
                &level,
                &new_ids,
                &HostId::new("test-host"),
            )
            .unwrap();
            let report = backend.apply(mutations).expect("imported a level");
            for (pending, id) in level.iter().zip(report.ids) {
                new_ids.insert(pending.old_id, id);
            }
        }
    }

    /// Ignore the ids, which the store reassigns, and compare the shape.
    /// (title, done, children as (title, done, grandchildren)) - the grandchildren are only
    /// counted, hence the unit.
    type Shape = Vec<(String, bool, Vec<(String, bool, Vec<()>)>)>;

    fn shape(items: &[Item]) -> Shape {
        items
            .iter()
            .map(|item| {
                (
                    item.text.clone(),
                    item.done,
                    item.children
                        .iter()
                        .map(|child| {
                            (
                                child.text.clone(),
                                child.done,
                                child.children.iter().map(|_| ()).collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn rows_become_the_tree_their_edges_describe() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![
                row(1, "/work", None, "top", false),
                row(2, "/work", Some(1), "under top", false),
                row(3, "/work", Some(2), "deeper", false),
                row(4, "/work", None, "another top", false),
            ],
        ));
        let list = built.get("/work").expect("the project's list");
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[0].text, "top");
        assert_eq!(list.items[0].children[0].text, "under top");
        assert_eq!(list.items[0].children[0].children[0].text, "deeper");
        assert_eq!(list.items[1].text, "another top");
    }

    #[test]
    fn rows_of_several_projects_become_several_lists() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![
                row(1, "/work", None, "work item", false),
                row(2, "/play", None, "play item", false),
            ],
        ));
        assert_eq!(built.len(), 2);
        assert_eq!(built["/work"].items[0].text, "work item");
        assert_eq!(built["/play"].items[0].text, "play item");
    }

    /// Sibling order is `id` order, and nothing stores it separately — so a
    /// fetch that came back in another order must still rebuild it.
    #[test]
    fn siblings_come_back_in_id_order_whatever_order_the_rows_arrived_in() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![
                row(3, "/work", Some(1), "third", false),
                row(1, "/work", None, "top", false),
                row(2, "/work", Some(1), "second", false),
            ],
        ));
        let children = &built["/work"].items[0].children;
        assert_eq!(
            children.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["second", "third"]
        );
    }

    /// A parent in another project is not a parent. A fetch scoped to one root
    /// sees exactly this, and the item must still appear.
    #[test]
    fn an_item_whose_parent_is_absent_becomes_a_top_level_item() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![row(2, "/work", Some(99), "orphan", false)],
        ));
        assert_eq!(built["/work"].items.len(), 1);
        assert_eq!(built["/work"].items[0].text, "orphan");
    }

    #[test]
    fn a_parent_in_another_project_is_ignored() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![
                row(1, "/work", None, "work top", false),
                row(2, "/play", Some(1), "play item", false),
            ],
        ));
        assert_eq!(built["/work"].items.len(), 1);
        assert!(built["/work"].items[0].children.is_empty());
        assert_eq!(built["/play"].items.len(), 1);
    }

    /// Nothing in the API can make a cycle, but a hand-edited database could,
    /// and materializing one must terminate and keep every item.
    ///
    /// Where the cycle gets broken is not worth pinning down — one of the edges
    /// survives and the items end up nested under each other. What matters is
    /// that both are still there.
    #[test]
    fn a_cycle_does_not_hang_and_loses_no_items() {
        let built = from_rows(&rows(
            ROW_COLUMNS,
            vec![
                row(1, "/work", Some(2), "one", false),
                row(2, "/work", Some(1), "two", false),
            ],
        ));
        let (_, all) = counts(&built["/work"]);
        assert_eq!(all, 2, "both items should survive a broken cycle");
    }

    #[test]
    fn ancestors_to_open_names_only_the_finished_ones_closest_first() {
        let items = vec![done(item(
            1,
            "top",
            vec![item(2, "open middle", vec![done(item(3, "deep", vec![]))])],
        ))];
        assert_eq!(ancestors_to_open(&items, 3), vec![1]);
    }

    #[test]
    fn a_trail_reads_from_the_top_down() {
        let items = vec![item(
            1,
            "top",
            vec![item(2, "middle", vec![item(3, "leaf", vec![])])],
        )];
        assert_eq!(trail_of(&items, 3), roots(&["top", "middle", "leaf"]));
    }

    // ------------------------------------------------------- write planning

    #[test]
    fn planning_an_add_writes_one_row() {
        let list = List::default();
        let (mutations, text) = plan_add(&list, "/work", "ship it", None).expect("planned");
        assert_eq!(text, "ship it");
        assert_eq!(
            mutations,
            vec![
                assert_snapshot(&list, "/work"),
                Mutation::put("todo")
                    .set("root", "/work")
                    .set("text", "ship it")
                    .set("done", false)
                    .build()
            ]
        );
    }

    /// The invariant, as a write: an open item under a finished one reopens it
    /// and everything above, in the same batch.
    #[test]
    fn planning_an_add_under_a_finished_item_reopens_the_items_above() {
        let list = List {
            next_id: 3,
            items: vec![done(item(1, "top", vec![done(item(2, "middle", vec![]))]))],
        };
        let (mutations, _) = plan_add(&list, "/work", "more work", Some(2)).expect("planned");
        assert_eq!(mutations.len(), 3);
        assert_eq!(
            mutations[2],
            Mutation::update("todo", Pred::is_in("id", [2u64, 1]))
                .set("done", false)
                .build()
        );
    }

    #[test]
    fn planning_an_add_under_an_open_item_reopens_nothing() {
        let list = List {
            next_id: 2,
            items: vec![item(1, "top", vec![])],
        };
        let (mutations, _) = plan_add(&list, "/work", "more", Some(1)).expect("planned");
        assert_eq!(mutations.len(), 2);
    }

    #[test]
    fn planning_an_add_rejects_a_parent_from_another_project() {
        let list = List::default();
        let error = plan_add(&list, "/work", "text", Some(7)).expect_err("no such parent");
        assert!(error.contains("no todo item 7 in this project"), "{error}");
    }

    #[test]
    fn crossing_an_item_off_cascades_to_its_subtree_in_one_write() {
        let list = List {
            next_id: 4,
            items: vec![item(
                1,
                "top",
                vec![item(2, "middle", vec![item(3, "leaf", vec![])])],
            )],
        };
        let (mutations, change) = plan_set_done(&list, "/work", 1, true).expect("planned");
        assert_eq!(change.cascaded, vec![2, 3]);
        assert_eq!(
            mutations,
            vec![
                assert_snapshot(&list, "/work"),
                Mutation::update("todo", Pred::is_in("id", [1u64, 2, 3]))
                    .set("done", true)
                    .build()
            ]
        );
    }

    #[test]
    fn opening_an_item_again_opens_the_items_above_it() {
        let list = List {
            next_id: 3,
            items: vec![done(item(1, "top", vec![done(item(2, "middle", vec![]))]))],
        };
        let (mutations, change) = plan_set_done(&list, "/work", 2, false).expect("planned");
        assert_eq!(change.reopened, vec![1]);
        assert_eq!(mutations.len(), 3);
        assert_eq!(mutations[0], assert_snapshot(&list, "/work"));
        assert_eq!(
            mutations[2],
            Mutation::update("todo", Pred::is_in("id", [1u64]))
                .set("done", false)
                .build()
        );
    }

    #[test]
    fn planning_an_edit_writes_one_row() {
        let list = List {
            next_id: 2,
            items: vec![item(1, "old text", vec![])],
        };
        let (mutations, text) = plan_edit(&list, 1, "new text", None).expect("planned");
        assert_eq!(text, "new text");
        assert_eq!(
            mutations,
            vec![Mutation::update("todo", Pred::eq("id", 1u64))
                .set("text", "new text")
                .build()]
        );
    }

    /// Two writers editing one item. The second write was composed from wording the first has
    /// already replaced, so it is refused rather than landing on top.
    #[test]
    fn an_edit_planned_from_a_stale_read_is_refused() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        local
            .apply(
                plan_add(&List::default(), "/work", "ship it", None)
                    .unwrap()
                    .0,
            )
            .unwrap();

        // Both read the same list.
        let mine = fetch_list(&local, "/work").unwrap();
        let theirs = mine.clone();
        let id = mine.items[0].id;

        // They write first.
        local
            .apply(plan_edit(&theirs, id, "ship it on Friday", None).unwrap().0)
            .unwrap();

        // My edit was composed from what the item said before, so it is refused whole.
        let error = local
            .apply(plan_edit(&mine, id, "ship it on Monday", None).unwrap().0)
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            text_of(&fetch_list(&local, "/work").unwrap(), id).as_deref(),
            Some("ship it on Friday"),
            "the refused write should have changed nothing"
        );

        // Reading again is all it takes.
        let fresh = fetch_list(&local, "/work").unwrap();
        local
            .apply(plan_edit(&fresh, id, "ship it on Monday", None).unwrap().0)
            .unwrap();
        assert_eq!(
            text_of(&fetch_list(&local, "/work").unwrap(), id).as_deref(),
            Some("ship it on Monday")
        );
    }

    /// The window the guard has to span is the caller's, not the writer's.
    ///
    /// The test above reads once and plans twice, which is not how an edit actually arrives: a
    /// model reads a list in one turn, composes new wording several turns later, and the code
    /// that writes it re-reads the list microseconds before writing. Guarding on *that* read
    /// catches nothing — the row already carries the other writer's revision. The revision the
    /// text was composed from has to travel with the text, which is what `expect_rev` is.
    #[test]
    fn an_edit_guards_on_the_revision_its_author_read_not_the_writers() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agents");
        local.define_all(&store::shared_defs()).unwrap();
        local
            .apply(
                plan_add(&List::default(), "/work", "ship it", None)
                    .unwrap()
                    .0,
            )
            .unwrap();

        // What the model was shown, and the revision it was shown beside the text.
        let read = fetch_list(&local, "/work").unwrap();
        let id = read.items[0].id;
        let saw = read.items[0].rev;
        assert!(
            saw > DataRevision::new(0),
            "a listing carries a revision to guard on"
        );
        assert_eq!(
            items_json(&read.items, false)[0]["rev"],
            serde_json::json!(saw),
            "and the model is shown it"
        );

        // Somebody else rewrites the item in the meantime.
        local
            .apply(plan_edit(&read, id, "ship it on Friday", None).unwrap().0)
            .unwrap();

        // The write re-reads first, as every caller does, so the row's own revision is now the
        // other writer's and guarding on it would let this through.
        let fresh = fetch_list(&local, "/work").unwrap();
        let error = local
            .apply(
                plan_edit(&fresh, id, "ship it on Monday", Some(saw))
                    .unwrap()
                    .0,
            )
            .unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            text_of(&fetch_list(&local, "/work").unwrap(), id).as_deref(),
            Some("ship it on Friday"),
            "the refused write should have changed nothing"
        );

        // Reading again and carrying the new revision is all it takes.
        let fresh = fetch_list(&local, "/work").unwrap();
        let now = item_of(&fresh, id).unwrap().rev;
        local
            .apply(
                plan_edit(&fresh, id, "ship it on Monday", Some(now))
                    .unwrap()
                    .0,
            )
            .unwrap();
        assert_eq!(
            text_of(&fetch_list(&local, "/work").unwrap(), id).as_deref(),
            Some("ship it on Monday")
        );
    }

    /// Completion validates the entire project, even if only unrelated text changed.
    #[test]
    fn crossing_an_item_off_conflicts_with_text_edits_and_agreeing_flag_writes() {
        use crate::store::{self, Backend};

        for edit in [true, false] {
            let local = store::Local::shared("agents");
            local.define_all(&store::shared_defs()).unwrap();
            let id = add_stored(&local, None);
            let unrelated = add_stored(&local, None);
            let stale = fetch_list(&local, "/work").unwrap();
            let first = if edit {
                plan_edit(&stale, unrelated, "ship it soon", None)
                    .unwrap()
                    .0
            } else {
                plan_set_done(&stale, "/work", id, true).unwrap().0
            };
            let completion = plan_set_done(&stale, "/work", id, true).unwrap().0;
            commit_competing(&local, first, completion);
        }
    }

    #[test]
    fn planning_an_edit_rejects_an_unknown_id() {
        let list = List::default();
        let error = plan_edit(&list, 7, "text", None).expect_err("no such item");
        assert!(error.contains("no todo item 7 in this project"), "{error}");
    }

    #[test]
    fn moving_a_root_is_one_update() {
        let mutations = plan_move_root("/old", "/new").expect("planned");
        assert_eq!(
            mutations,
            vec![Mutation::update("todo", Pred::eq("root", "/old"))
                .set("root", "/new")
                .build()]
        );
    }

    #[test]
    fn moving_a_root_to_itself_writes_nothing() {
        assert!(plan_move_root("/same", "/same")
            .expect("planned")
            .is_empty());
    }

    // --------------------------------------------------- against the engine

    /// The whole layer end to end: a tree goes in through the import, comes back
    /// out through a fetch, and is the same tree.
    #[test]
    fn a_list_survives_a_round_trip_through_the_store() {
        let backend = backend();
        let original = List {
            next_id: 6,
            items: vec![
                item(
                    1,
                    "first",
                    vec![
                        done(item(2, "done child", vec![])),
                        item(3, "open child", vec![item(4, "grandchild", vec![])]),
                    ],
                ),
                item(5, "second", vec![]),
            ],
        };
        import(&backend, "/work", &original);
        assert_eq!(
            shape(&materialize(&backend, "/work").items),
            shape(&original.items)
        );
    }

    /// Adding through the planner and reading back agrees with what the old
    /// in-memory `add` would have produced.
    #[test]
    fn adding_through_the_store_builds_the_same_tree_as_adding_in_memory() {
        let backend = backend();
        let mut expected = Store::new();

        let mut parent = None;
        for text in ["top", "under top", "deeper"] {
            let list = materialize(&backend, "/work");
            let (mutations, _) = plan_add(&list, "/work", text, parent).expect("planned");
            let report = backend.apply(mutations).expect("applied");
            parent = Some(u64::try_from(report.ids[0].get()).unwrap());
            let under = expected_parent(&expected);
            add(&mut expected, "/work", text, under).expect("added");
        }

        assert_eq!(
            shape(&materialize(&backend, "/work").items),
            shape(&expected["/work"].items)
        );
    }

    /// The id of the deepest item of the expected tree, to nest the next one
    /// under — the in-memory twin of threading `ApplyReport::ids`.
    fn expected_parent(store: &Store) -> Option<u64> {
        let mut at = store.get("/work")?.items.first()?;
        while let Some(child) = at.children.first() {
            at = child;
        }
        Some(at.id)
    }

    #[test]
    fn crossing_off_through_the_store_cascades_to_the_whole_subtree() {
        let backend = backend();
        import(
            &backend,
            "/work",
            &List {
                next_id: 4,
                items: vec![item(
                    1,
                    "top",
                    vec![item(2, "middle", vec![item(3, "leaf", vec![])])],
                )],
            },
        );

        let list = materialize(&backend, "/work");
        let top = list.items[0].id;
        let (mutations, _) = plan_set_done(&list, "/work", top, true).expect("planned");
        backend.apply(mutations).expect("applied");

        let after = materialize(&backend, "/work");
        assert!(after.items[0].done);
        assert!(after.items[0].children[0].done);
        assert!(after.items[0].children[0].children[0].done);
    }

    /// The invariant survives a real write: reopening the leaf reopens the two
    /// items above it.
    #[test]
    fn opening_a_leaf_through_the_store_opens_the_items_above_it() {
        let backend = backend();
        import(
            &backend,
            "/work",
            &List {
                next_id: 4,
                items: vec![done(item(
                    1,
                    "top",
                    vec![done(item(2, "middle", vec![done(item(3, "leaf", vec![]))]))],
                ))],
            },
        );

        let list = materialize(&backend, "/work");
        let leaf = list.items[0].children[0].children[0].id;
        let (mutations, _) = plan_set_done(&list, "/work", leaf, false).expect("planned");
        backend.apply(mutations).expect("applied");

        let after = materialize(&backend, "/work");
        assert!(!after.items[0].done, "the top should have reopened");
        assert!(
            !after.items[0].children[0].done,
            "the middle should have reopened"
        );
        assert!(!after.items[0].children[0].children[0].done);
    }

    #[test]
    fn moving_a_root_through_the_store_carries_the_whole_list() {
        let backend = backend();
        import(
            &backend,
            "/old",
            &List {
                next_id: 3,
                items: vec![item(1, "top", vec![item(2, "child", vec![])])],
            },
        );

        let mutations = plan_move_root("/old", "/new").expect("planned");
        backend.apply(mutations).expect("applied");

        assert!(materialize(&backend, "/old").items.is_empty());
        let moved = materialize(&backend, "/new");
        assert_eq!(moved.items[0].text, "top");
        assert_eq!(moved.items[0].children[0].text, "child");
    }

    /// Search is the store's, not a scan of ours. The fallback for a partial
    /// word is what keeps the old substring behaviour.
    #[test]
    fn full_text_search_finds_a_whole_word_and_the_fallback_finds_a_fragment() {
        let backend = backend();
        import(
            &backend,
            "/work",
            &List {
                next_id: 3,
                items: vec![
                    item(1, "fixing a rebase conflict", vec![]),
                    item(2, "writing the changelog", vec![]),
                ],
            },
        );

        let hits = backend
            .fetch(&FetchRequest::entity("todo").search("rebase"))
            .expect("searched");
        assert_eq!(hits.len(), 1);

        // FTS5 tokenizes, so a fragment matches nothing...
        let fragment = backend
            .fetch(&FetchRequest::entity("todo").search("reba"))
            .expect("searched");
        assert!(fragment.is_empty(), "a fragment should not match a token");

        // ...which is why the substring fallback has to exist.
        let fallback = backend
            .fetch(&FetchRequest::entity("todo").filter(Pred::contains("text", "reba")))
            .expect("searched");
        assert_eq!(fallback.len(), 1);
    }

    fn texts(value: &serde_json::Value) -> Vec<String> {
        fn walk(items: &serde_json::Value, out: &mut Vec<String>) {
            for item in items.as_array().unwrap() {
                out.push(item["text"].as_str().unwrap().to_owned());
                if let Some(children) = item.get("children") {
                    walk(children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&value["items"], &mut out);
        out
    }

    // -- loading ------------------------------------------------------------

    #[test]
    fn a_root_that_is_not_a_clean_absolute_path_is_dropped() {
        let saved = parse_legacy(
            br#"{"work/agent":{"items":[{"id":1,"text":"a"}]},
                 "/work/good":{"items":[{"id":1,"text":"b"}]}}"#,
        );
        assert_eq!(saved.keys().collect::<Vec<_>>(), vec!["/work/good"]);
    }

    #[test]
    fn a_repeated_id_and_a_missing_one_each_get_a_fresh_id() {
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[
                {"id":1,"text":"first"},{"id":1,"text":"second"},{"text":"third"}]}}"#,
        );
        let ids: Vec<u64> = saved["/work/agent"].items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(saved["/work/agent"].next_id, 4);
    }

    #[test]
    fn a_repair_never_gives_out_an_id_another_item_holds() {
        // The fresh id for the second item must clear id 7, which comes later.
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[
                {"id":1,"text":"first"},{"id":1,"text":"second"},{"id":7,"text":"third"}]}}"#,
        );
        let ids: Vec<u64> = saved["/work/agent"].items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 8, 7]);
        assert_eq!(saved["/work/agent"].next_id, 9);
    }

    #[test]
    fn next_id_ends_above_every_id_the_list_holds() {
        let saved =
            parse_legacy(br#"{"/work/agent":{"next_id":2,"items":[{"id":30,"text":"a"}]}}"#);
        assert_eq!(saved["/work/agent"].next_id, 31);
    }

    #[test]
    fn an_item_with_bad_text_is_dropped_with_its_sub_items() {
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[
                {"id":1,"text":"  ","children":[{"id":2,"text":"hidden"}]},
                {"id":3,"text":"kept"}]}}"#,
        );
        assert_eq!(saved["/work/agent"].items.len(), 1);
        assert_eq!(saved["/work/agent"].items[0].text, "kept");
    }

    #[test]
    fn an_item_below_the_depth_cap_is_dropped() {
        let mut deep = item(6, "level 6", Vec::new());
        for level in (1..=5).rev() {
            deep = item(level, &format!("level {level}"), vec![deep]);
        }
        let saved = parse_legacy(json(&store("/work/agent", vec![deep])).unwrap().as_bytes());

        let names = texts(&list_json(&saved, "/work/agent", false));
        assert_eq!(
            names,
            vec!["level 1", "level 2", "level 3", "level 4", "level 5"],
        );
    }

    #[test]
    fn legacy_lists_preserve_more_than_one_page() {
        let items: Vec<Item> = (1..=1200)
            .map(|id| item(id, &format!("item {id}"), Vec::new()))
            .collect();
        let saved = parse_legacy(json(&store("/work/agent", items)).unwrap().as_bytes());

        assert_eq!(counts(&saved["/work/agent"]).1, 1200);
        let backend = backend();
        import(&backend, "/work/agent", &saved["/work/agent"]);
        assert_eq!(
            counts(&fetch_list(&backend, "/work/agent").unwrap()).1,
            1200
        );
    }

    #[test]
    fn a_parent_with_an_open_sub_item_is_opened() {
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[
                {"id":1,"text":"parent","done":true,"children":[{"id":2,"text":"child"}]}]}}"#,
        );
        assert!(!saved["/work/agent"].items[0].done);
    }

    #[test]
    fn a_done_parent_with_a_done_sub_item_stays_done() {
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[
                {"id":1,"text":"parent","done":true,
                 "children":[{"id":2,"text":"child","done":true}]}]}}"#,
        );
        assert!(saved["/work/agent"].items[0].done);
    }

    #[test]
    fn a_list_with_no_items_leaves_no_row() {
        assert!(parse_legacy(br#"{"/work/agent":{"next_id":9,"items":[]}}"#).is_empty());
    }

    #[test]
    fn two_keys_that_name_one_folder_join_into_one_list() {
        let saved = parse_legacy(
            br#"{"/work/agent":{"items":[{"id":1,"text":"first"}]},
                 "/work/agent/":{"items":[{"id":1,"text":"second"}]}}"#,
        );
        assert_eq!(saved.len(), 1);
        assert_eq!(saved["/work/agent"].items.len(), 2);
    }

    #[test]
    fn a_store_round_trips_through_json_and_parse() {
        let saved = store(
            "/work/agent",
            vec![
                item(
                    1,
                    "ship the parser",
                    vec![done(item(2, "write the test", vec![]))],
                ),
                item(3, "answer the review", Vec::new()),
            ],
        );
        assert_eq!(parse_legacy(json(&saved).unwrap().as_bytes()), saved);
    }

    #[test]
    fn nothing_saved_is_an_empty_store() {
        assert!(parse_legacy(b"").is_empty());
        assert!(parse_legacy(b"not json").is_empty());
    }

    // -- adding -------------------------------------------------------------

    #[test]
    fn ids_start_at_one_and_never_come_back() {
        let mut saved = Store::new();
        assert_eq!(add(&mut saved, "/work/agent", "first", None), Ok(1));
        assert_eq!(add(&mut saved, "/work/agent", "second", None), Ok(2));
        saved.get_mut("/work/agent").unwrap().items.clear();
        assert_eq!(add(&mut saved, "/work/agent", "third", None), Ok(3));
    }

    #[test]
    fn an_item_lands_under_its_parent() {
        let mut saved = Store::new();
        let parent = add(&mut saved, "/work/agent", "parent", None).unwrap();
        let child = add(&mut saved, "/work/agent", "child", Some(parent)).unwrap();

        let items = &saved["/work/agent"].items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children[0].id, child);
    }

    #[test]
    fn a_parent_id_from_another_project_is_refused() {
        let mut saved = Store::new();
        add(&mut saved, "/work/other", "elsewhere", None).unwrap();
        let error = add(&mut saved, "/work/agent", "child", Some(1)).unwrap_err();

        assert!(
            error.starts_with("no todo item 1 in this project"),
            "{error}"
        );
        assert!(!saved.contains_key("/work/agent"), "no empty list was left");
    }

    #[test]
    fn text_that_is_empty_is_refused() {
        let mut saved = Store::new();
        assert!(add(&mut saved, "/work/agent", "   ", None).is_err());
        assert!(add(&mut saved, "/work/agent", "\n\t\u{1b}\u{7}", None).is_err());
        assert!(saved.is_empty());
    }

    #[test]
    fn markdown_is_preserved_while_control_characters_are_removed() {
        assert_eq!(
            clean_text(" \n**ship it**\n\t- 日本語\u{1b}\u{7} "),
            Ok("**ship it**\n    - 日本語".to_owned())
        );
    }

    #[test]
    fn a_parent_at_the_depth_cap_is_refused() {
        let mut saved = Store::new();
        let mut parent = add(&mut saved, "/work/agent", "level 1", None).unwrap();
        for level in 2..=MAX_DEPTH {
            parent = add(
                &mut saved,
                "/work/agent",
                &format!("level {level}"),
                Some(parent),
            )
            .unwrap();
        }
        let error = add(&mut saved, "/work/agent", "too deep", Some(parent)).unwrap_err();
        assert_eq!(
            error,
            format!("todo items nest {MAX_DEPTH} levels deep at most")
        );
    }

    #[test]
    fn a_list_at_the_open_item_cap_is_refused() {
        let mut saved = Store::new();
        for index in 0..MAX_OPEN {
            add(&mut saved, "/work/agent", &format!("item {index}"), None).unwrap();
        }
        let error = add(&mut saved, "/work/agent", "one more", None).unwrap_err();
        assert!(error.contains("cross some off first"), "{error}");
    }

    #[test]
    fn adding_under_a_done_parent_opens_it() {
        let mut saved = store("/work/agent", vec![done(item(1, "parent", Vec::new()))]);
        add(&mut saved, "/work/agent", "more work", Some(1)).unwrap();

        assert!(!saved["/work/agent"].items[0].done);
    }

    // -- reading ------------------------------------------------------------

    #[test]
    fn a_list_hides_done_items_unless_they_are_asked_for() {
        let saved = store(
            "/work/agent",
            vec![
                item(1, "open", Vec::new()),
                done(item(2, "finished", Vec::new())),
            ],
        );
        assert_eq!(
            texts(&list_json(&saved, "/work/agent", false)),
            vec!["open"]
        );
        assert_eq!(
            texts(&list_json(&saved, "/work/agent", true)),
            vec!["open", "finished"],
        );
    }

    #[test]
    fn a_list_hides_the_sub_items_of_a_done_item() {
        let saved = store(
            "/work/agent",
            vec![done(item(
                1,
                "parent",
                vec![done(item(2, "child", Vec::new()))],
            ))],
        );
        assert!(texts(&list_json(&saved, "/work/agent", false)).is_empty());
        assert_eq!(
            texts(&list_json(&saved, "/work/agent", true)),
            vec!["parent", "child"],
        );
    }

    #[test]
    fn a_list_gives_every_item_an_id_at_every_depth() {
        let saved = store(
            "/work/agent",
            vec![item(1, "parent", vec![item(2, "child", Vec::new())])],
        );
        let value = list_json(&saved, "/work/agent", false);

        assert_eq!(value["open"], 2);
        assert_eq!(value["done"], 0);
        assert_eq!(value["items"][0]["id"], 1);
        assert_eq!(value["items"][0]["children"][0]["id"], 2);
        assert!(
            value["items"][0].get("done").is_none(),
            "open is the default"
        );
    }

    #[test]
    fn a_list_that_was_never_started_is_empty_and_not_an_error() {
        let value = list_json(&Store::new(), "/work/agent", false);
        assert_eq!(value["open"], 0);
        assert!(value["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn counts_separate_open_items_from_done_items() {
        let list = &store(
            "/work/agent",
            vec![item(1, "open", vec![done(item(2, "child", Vec::new()))])],
        )["/work/agent"]
            .clone();
        assert_eq!(counts(list), (1, 2));
    }

    // -- searching ----------------------------------------------------------

    /// Whole words go through the index; a fragment falls back to a substring
    /// match, which is what the recursive scan used to do for everything.
    #[test]
    fn a_search_ignores_case_and_matches_part_of_the_text() {
        let backend = backend();
        import(
            &backend,
            "/work/agent",
            &List {
                next_id: 2,
                items: vec![item(1, "Ship the Parser", Vec::new())],
            },
        );
        assert_eq!(search(&backend, None, "parse").expect("searched").len(), 1);
        assert_eq!(search(&backend, None, "PARSER").expect("searched").len(), 1);
        assert!(search(&backend, None, "compiler")
            .expect("searched")
            .is_empty());
    }

    #[test]
    fn a_search_gives_the_items_above_the_match() {
        let backend = backend();
        import(
            &backend,
            "/work/agent",
            &List {
                next_id: 3,
                items: vec![item(
                    1,
                    "release 2",
                    vec![item(2, "ship the parser", Vec::new())],
                )],
            },
        );
        let found = search(&backend, None, "parser").expect("searched");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, "/work/agent");
        assert_eq!(found[0].trail, vec!["release 2", "ship the parser"]);
    }

    #[test]
    fn a_search_finds_an_item_that_is_crossed_off() {
        let backend = backend();
        import(
            &backend,
            "/work/agent",
            &List {
                next_id: 2,
                items: vec![done(item(1, "ship it", Vec::new()))],
            },
        );
        let found = search(&backend, None, "ship").expect("searched");

        assert_eq!(found.len(), 1);
        assert!(found[0].done);
    }

    /// An unscoped search covers every project.
    ///
    /// It no longer answers in folder order — the index ranks by relevance, and
    /// for two equally good matches that order is the index's to choose. Nothing
    /// depended on the old order; both matches being present is the property.
    #[test]
    fn a_search_over_every_list_covers_every_project() {
        let backend = backend();
        for root in ["/work/pocus", "/work/agent"] {
            import(
                &backend,
                root,
                &List {
                    next_id: 2,
                    items: vec![item(1, "ship it", Vec::new())],
                },
            );
        }
        let found = search(&backend, None, "ship").expect("searched");

        let mut where_found: Vec<&str> = found.iter().map(|found| found.root.as_str()).collect();
        where_found.sort_unstable();
        assert_eq!(where_found, vec!["/work/agent", "/work/pocus"]);
    }

    #[test]
    fn a_search_can_be_scoped_to_one_project() {
        let backend = backend();
        for root in ["/work/pocus", "/work/agent"] {
            import(
                &backend,
                root,
                &List {
                    next_id: 2,
                    items: vec![item(1, "ship it", Vec::new())],
                },
            );
        }
        let found = search(&backend, Some("/work/agent"), "ship").expect("searched");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, "/work/agent");
    }

    #[test]
    fn a_search_is_cut_at_the_match_cap() {
        let backend = backend();
        let items: Vec<Item> = (1..=MAX_MATCHES as u64 + 5)
            .map(|id| item(id, &format!("ship {id}"), Vec::new()))
            .collect();
        import(
            &backend,
            "/work/agent",
            &List {
                next_id: MAX_MATCHES as u64 + 6,
                items,
            },
        );

        assert_eq!(
            search(&backend, None, "ship").expect("searched").len(),
            MAX_MATCHES
        );
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let backend = backend();
        import(
            &backend,
            "/work/agent",
            &List {
                next_id: 2,
                items: vec![item(1, "ship it", Vec::new())],
            },
        );
        assert!(search(&backend, None, "  ").expect("searched").is_empty());
    }

    // -- crossing off -------------------------------------------------------

    #[test]
    fn crossing_an_item_off_crosses_off_the_items_below_it() {
        let mut saved = store(
            "/work/agent",
            vec![item(
                1,
                "parent",
                vec![item(2, "child", vec![item(3, "grandchild", Vec::new())])],
            )],
        );
        let change = set_done(&mut saved, "/work/agent", 1, true).unwrap();

        assert_eq!(change.cascaded, vec![2, 3]);
        assert!(change.reopened.is_empty());
        assert_eq!(counts(&saved["/work/agent"]), (0, 3));
    }

    #[test]
    fn an_undo_opens_the_items_above_and_leaves_the_items_below_done() {
        let mut saved = store(
            "/work/agent",
            vec![item(
                1,
                "parent",
                vec![item(2, "child", vec![item(3, "leaf", vec![])])],
            )],
        );
        set_done(&mut saved, "/work/agent", 1, true).unwrap();
        let change = set_done(&mut saved, "/work/agent", 3, false).unwrap();

        assert_eq!(change.reopened, vec![1, 2]);
        assert!(change.cascaded.is_empty());
        // Only the leaf and the items above it came back.
        assert_eq!(counts(&saved["/work/agent"]), (3, 3));
    }

    #[test]
    fn an_undo_only_reports_the_items_it_changed() {
        let mut saved = store(
            "/work/agent",
            vec![item(1, "parent", vec![done(item(2, "child", Vec::new()))])],
        );
        let change = set_done(&mut saved, "/work/agent", 2, false).unwrap();

        assert!(change.reopened.is_empty(), "the parent was open already");
    }

    #[test]
    fn crossing_off_an_item_that_is_already_done_is_not_an_error() {
        let mut saved = store("/work/agent", vec![done(item(1, "ship it", Vec::new()))]);
        let change = set_done(&mut saved, "/work/agent", 1, true).unwrap();

        assert!(change.done);
        assert!(change.cascaded.is_empty());
    }

    #[test]
    fn an_id_that_is_not_in_the_list_is_reported() {
        let mut saved = store("/work/agent", vec![item(1, "ship it", Vec::new())]);
        assert_eq!(
            set_done(&mut saved, "/work/agent", 9, true),
            Err("no todo item 9 in this project; call todo_list".to_owned()),
        );
        assert!(set_done(&mut saved, "/work/other", 1, true).is_err());
    }
    fn add_stored(backend: &impl Backend, parent: Option<u64>) -> u64 {
        let list = fetch_list(backend, "/work").unwrap();
        let (mutations, _) = plan_add(&list, "/work", "item", parent).unwrap();
        backend.apply(mutations).unwrap().ids[0].get() as u64
    }

    /// Inspect raw edges: materializing a list repairs cycles and orphans for display.
    fn assert_raw_tree(backend: &impl Backend) {
        let rows = backend.fetch(&FetchRequest::entity("todo")).unwrap();
        let ids: BTreeSet<_> = rows
            .rows
            .iter()
            .map(|row| int_at(&rows, row, "id").unwrap())
            .collect();
        let parents: BTreeMap<_, _> = rows
            .rows
            .iter()
            .filter_map(|row| {
                int_at(&rows, row, "parent_id")
                    .map(|parent| (int_at(&rows, row, "id").unwrap(), parent))
            })
            .collect();
        let completed: BTreeSet<_> = rows
            .rows
            .iter()
            .filter(|row| matches!(rows.get(row, "done"), Some(DataValue::Bool(true))))
            .map(|row| int_at(&rows, row, "id").unwrap())
            .collect();
        for (id, parent) in &parents {
            assert!(ids.contains(parent), "orphan {id} -> {parent}");
            assert!(!is_cyclic(*id, &parents), "cycle at {id}");
            assert!(depth_via(*id, &parents) < MAX_DEPTH);
            assert!(
                !completed.contains(parent) || completed.contains(id),
                "completed parent {parent} hides open child {id}"
            );
        }
    }

    fn commit_competing(backend: &store::Local, first: Vec<Mutation>, stale: Vec<Mutation>) {
        let other = backend.joined(backend.namespace(), "other");
        other.apply(first).unwrap();
        let before = backend.fetch(&FetchRequest::entity("todo")).unwrap();
        let links = backend.fetch(&FetchRequest::entity("note_link")).unwrap();
        let error = backend.apply(stale).unwrap_err();
        assert!(error.is_conflict(), "{error:?}");
        assert_eq!(
            backend.fetch(&FetchRequest::entity("todo")).unwrap(),
            before
        );
        assert_eq!(
            backend.fetch(&FetchRequest::entity("note_link")).unwrap(),
            links
        );
        assert_raw_tree(backend);
    }

    #[test]
    fn reciprocal_moves_from_one_snapshot_cannot_create_a_cycle() {
        for reverse in [false, true] {
            let backend = backend();
            let a = add_stored(&backend, None);
            let b = add_stored(&backend, None);
            let list = fetch_list(&backend, "/work").unwrap();
            let a_under_b = plan_move(&list, "/work", a, Some(b), None).unwrap().0;
            let b_under_a = plan_move(&list, "/work", b, Some(a), None).unwrap().0;
            let (first, stale) = if reverse {
                (b_under_a, a_under_b)
            } else {
                (a_under_b, b_under_a)
            };
            commit_competing(&backend, first, stale);
        }
    }

    #[test]
    fn undo_and_move_under_a_completed_parent_conflict_in_both_orders() {
        for undo_first in [false, true] {
            let backend = backend();
            let parent = add_stored(&backend, None);
            let child = add_stored(&backend, Some(parent));
            let destination = add_stored(&backend, None);
            for id in [parent, destination] {
                let list = fetch_list(&backend, "/work").unwrap();
                backend
                    .apply(plan_set_done(&list, "/work", id, true).unwrap().0)
                    .unwrap();
            }
            let list = fetch_list(&backend, "/work").unwrap();
            let undo = plan_set_done(&list, "/work", child, false).unwrap().0;
            let moved = plan_move(&list, "/work", child, Some(destination), None)
                .unwrap()
                .0;
            let (first, stale) = if undo_first {
                (undo, moved)
            } else {
                (moved, undo)
            };
            commit_competing(&backend, first, stale);
        }
    }

    #[test]
    fn completion_and_child_insertion_or_deletion_conflict_in_both_orders() {
        for delete in [false, true] {
            for completion_first in [false, true] {
                let backend = backend();
                let parent = add_stored(&backend, None);
                let child = add_stored(&backend, Some(parent));
                let list = fetch_list(&backend, "/work").unwrap();
                let completion = plan_set_done(&list, "/work", parent, true).unwrap().0;
                let change = if delete {
                    plan_delete(&list, "/work", child, false).unwrap().0
                } else {
                    plan_add(&list, "/work", "new child", Some(parent))
                        .unwrap()
                        .0
                };
                let (first, stale) = if completion_first {
                    (completion, change)
                } else {
                    (change, completion)
                };
                commit_competing(&backend, first, stale);
            }
        }
    }

    #[test]
    fn insertion_and_deletion_conflict_in_both_orders_without_losing_links() {
        for recursive in [false, true] {
            for delete_first in [false, true] {
                let backend = backend();
                let parent = add_stored(&backend, None);
                if recursive {
                    add_stored(&backend, Some(parent));
                }
                backend
                    .apply(vec![Mutation::put("note_link")
                        .set("todo_id", parent)
                        .set("note_id", 99u64)
                        .build()])
                    .unwrap();
                let list = fetch_list(&backend, "/work").unwrap();
                let insert = plan_add(&list, "/work", "new child", Some(parent))
                    .unwrap()
                    .0;
                let delete = plan_delete(&list, "/work", parent, recursive).unwrap().0;
                let (first, stale) = if delete_first {
                    (delete, insert)
                } else {
                    (insert, delete)
                };
                commit_competing(&backend, first, stale);
            }
        }
    }

    #[test]
    fn destination_deletion_and_move_conflict_in_both_orders() {
        for delete_first in [false, true] {
            let backend = backend();
            let source = add_stored(&backend, None);
            let destination = add_stored(&backend, None);
            let list = fetch_list(&backend, "/work").unwrap();
            let moved = plan_move(&list, "/work", source, Some(destination), None)
                .unwrap()
                .0;
            let deleted = plan_delete(&list, "/work", destination, false).unwrap().0;
            let (first, stale) = if delete_first {
                (deleted, moved)
            } else {
                (moved, deleted)
            };
            commit_competing(&backend, first, stale);
        }
    }

    #[test]
    fn deeper_ancestor_moves_cannot_create_a_cycle() {
        let backend = backend();
        let a = add_stored(&backend, None);
        let child = add_stored(&backend, Some(a));
        let b = add_stored(&backend, None);
        let list = fetch_list(&backend, "/work").unwrap();
        let a_under_b = plan_move(&list, "/work", a, Some(b), None).unwrap().0;
        let b_under_child = plan_move(&list, "/work", b, Some(child), None).unwrap().0;
        commit_competing(&backend, a_under_b, b_under_child);
    }

    #[test]
    fn subtree_growth_and_ancestor_moves_cannot_exceed_depth_limit() {
        for grow_subtree in [false, true] {
            for reverse in [false, true] {
                let backend = backend();
                let source = add_stored(&backend, None);
                let extra = add_stored(&backend, None);
                let destination_top = add_stored(&backend, None);
                let mut destination = destination_top;
                for _ in 1..MAX_DEPTH - 1 {
                    destination = add_stored(&backend, Some(destination));
                }
                let list = fetch_list(&backend, "/work").unwrap();
                let moved = plan_move(&list, "/work", source, Some(destination), None)
                    .unwrap()
                    .0;
                let changed = if grow_subtree {
                    plan_add(&list, "/work", "child", Some(source)).unwrap().0
                } else {
                    plan_move(&list, "/work", destination_top, Some(extra), None)
                        .unwrap()
                        .0
                };
                let (first, stale) = if reverse {
                    (moved, changed)
                } else {
                    (changed, moved)
                };
                commit_competing(&backend, first, stale);
            }
        }
    }

    #[test]
    fn legacy_children_require_a_live_mapped_parent_and_guard_the_insertion() {
        for delete_first in [false, true] {
            let backend = backend();
            let parent = add_stored(&backend, None);
            let level = vec![Pending {
                old_id: 2,
                old_parent: Some(1),
                text: "child".into(),
                done: false,
            }];
            let ids = BTreeMap::from([(1, DataRowId::new(parent as i64))]);
            let list = fetch_list(&backend, "/work").unwrap();
            assert!(import_mutations(
                &list,
                "/work",
                &level,
                &BTreeMap::new(),
                &HostId::new("test-host")
            )
            .is_err());
            let insert =
                import_mutations(&list, "/work", &level, &ids, &HostId::new("test-host")).unwrap();
            let delete = plan_delete(&list, "/work", parent, true).unwrap().0;
            let (first, stale) = if delete_first {
                (delete, insert)
            } else {
                (insert, delete)
            };
            commit_competing(&backend, first, stale);
            if delete_first {
                let fresh = fetch_list(&backend, "/work").unwrap();
                assert!(
                    import_mutations(&fresh, "/work", &level, &ids, &HostId::new("test-host"))
                        .is_err()
                );
            }
        }
    }
}
