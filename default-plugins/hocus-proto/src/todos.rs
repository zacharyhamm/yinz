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
use std::fs;

const FILE: &str = "/cache/todos.json";

/// The most levels a list holds. A top item is level 1.
pub const MAX_DEPTH: usize = 5;
/// The most characters in the text of one item.
pub const MAX_TEXT: usize = 500;
/// The most open items in one list. An item that is crossed off does not count.
pub const MAX_OPEN: usize = 200;
/// The most items of any state in one list. This bounds the file.
///
/// ponytail: there is no tool that removes an item, so a list at this cap needs
/// the file edited by hand.
pub const MAX_ITEMS: usize = 1000;
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Item>,
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

pub fn load() -> Store {
    parse(&fs::read(FILE).unwrap_or_default())
}

pub fn json(store: &Store) -> Option<String> {
    serde_json::to_string(store).ok()
}

pub fn write(json: &[u8]) {
    let _ = fs::write(FILE, json);
}

/// Move a project's list when its preferred checkout changes.
pub fn move_root(store: &mut Store, old: &str, new: &str) {
    let (Ok(old), Ok(new)) = (normalize_root(old), normalize_root(new)) else {
        return;
    };
    if old == new {
        return;
    }
    let Some(mut moved) = store.remove(&old) else {
        return;
    };
    let destination = store.entry(new).or_default();
    destination.next_id = destination.next_id.max(moved.next_id);
    destination.items.append(&mut moved.items);
    *destination = repair(std::mem::take(destination));
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
fn parse(json: &[u8]) -> Store {
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
    let mut kept = 0;
    let mut items = clean(list.items, 1, &mut next, &mut seen, &mut kept);
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
fn clean(
    items: Vec<Item>,
    depth: usize,
    next: &mut u64,
    seen: &mut BTreeSet<u64>,
    kept: &mut usize,
) -> Vec<Item> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in items {
        if *kept >= MAX_ITEMS {
            break;
        }
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
        *kept += 1;
        let children = clean(item.children, depth + 1, next, seen, kept);
        out.push(Item {
            id,
            text,
            done: item.done,
            children,
        });
    }
    out
}

/// The text of an item, or an error naming why it cannot be one.
fn clean_text(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err("todo text must contain visible characters".to_owned());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("todo text must be 1 to {MAX_TEXT} characters"));
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

/// The items of the given lists whose text holds the query.
///
/// The match ignores case. An item that is crossed off matches as well: a user
/// who asks whether the list already holds an item about a subject wants to
/// hear about a finished one too, and each match says which it is.
pub fn search(store: &Store, roots: &[String], query: &str) -> Vec<Match> {
    let needle = query.trim().to_lowercase();
    let mut matches = Vec::new();
    if needle.is_empty() {
        return matches;
    }
    for root in roots {
        let Some(list) = store.get(root) else {
            continue;
        };
        search_into(&list.items, root, &needle, &mut Vec::new(), &mut matches);
    }
    matches.truncate(MAX_MATCHES);
    matches
}

fn search_into(
    items: &[Item],
    root: &str,
    needle: &str,
    trail: &mut Vec<String>,
    matches: &mut Vec<Match>,
) {
    for item in items {
        trail.push(item.text.clone());
        if item.text.to_lowercase().contains(needle) {
            matches.push(Match {
                root: root.to_owned(),
                id: item.id,
                text: item.text.clone(),
                done: item.done,
                trail: trail.clone(),
            });
        }
        search_into(&item.children, root, needle, trail, matches);
        trail.pop();
    }
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
    let (open, all) = counts(list);
    if open >= MAX_OPEN {
        return Err(format!(
            "this project has {MAX_OPEN} open todo items; cross some off first"
        ));
    }
    if all >= MAX_ITEMS {
        return Err(format!(
            "this project has {MAX_ITEMS} todo items, the most a list holds"
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
    let Some(text) = find_text(&list.items, id) else {
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

fn find_text(items: &[Item], id: u64) -> Option<String> {
    for item in items {
        if item.id == id {
            return Some(item.text.clone());
        }
        if let Some(found) = find_text(&item.children, id) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, text: &str, children: Vec<Item>) -> Item {
        Item {
            id,
            text: text.to_owned(),
            done: false,
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

    #[test]
    fn moving_a_root_keeps_both_lists_and_repairs_duplicate_ids() {
        let mut store = Store::new();
        add(&mut store, "/linked", "linked", None).unwrap();
        add(&mut store, "/primary", "primary", None).unwrap();

        move_root(&mut store, "/linked", "/primary");

        assert!(!store.contains_key("/linked"));
        assert_eq!(store["/primary"].items.len(), 2);
        assert_ne!(store["/primary"].items[0].id, store["/primary"].items[1].id);
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
        let saved = parse(
            br#"{"work/hocus":{"items":[{"id":1,"text":"a"}]},
                 "/work/good":{"items":[{"id":1,"text":"b"}]}}"#,
        );
        assert_eq!(saved.keys().collect::<Vec<_>>(), vec!["/work/good"]);
    }

    #[test]
    fn a_repeated_id_and_a_missing_one_each_get_a_fresh_id() {
        let saved = parse(
            br#"{"/work/hocus":{"items":[
                {"id":1,"text":"first"},{"id":1,"text":"second"},{"text":"third"}]}}"#,
        );
        let ids: Vec<u64> = saved["/work/hocus"].items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(saved["/work/hocus"].next_id, 4);
    }

    #[test]
    fn a_repair_never_gives_out_an_id_another_item_holds() {
        // The fresh id for the second item must clear id 7, which comes later.
        let saved = parse(
            br#"{"/work/hocus":{"items":[
                {"id":1,"text":"first"},{"id":1,"text":"second"},{"id":7,"text":"third"}]}}"#,
        );
        let ids: Vec<u64> = saved["/work/hocus"].items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 8, 7]);
        assert_eq!(saved["/work/hocus"].next_id, 9);
    }

    #[test]
    fn next_id_ends_above_every_id_the_list_holds() {
        let saved = parse(br#"{"/work/hocus":{"next_id":2,"items":[{"id":30,"text":"a"}]}}"#);
        assert_eq!(saved["/work/hocus"].next_id, 31);
    }

    #[test]
    fn an_item_with_bad_text_is_dropped_with_its_sub_items() {
        let saved = parse(
            br#"{"/work/hocus":{"items":[
                {"id":1,"text":"  ","children":[{"id":2,"text":"hidden"}]},
                {"id":3,"text":"kept"}]}}"#,
        );
        assert_eq!(saved["/work/hocus"].items.len(), 1);
        assert_eq!(saved["/work/hocus"].items[0].text, "kept");
    }

    #[test]
    fn an_item_below_the_depth_cap_is_dropped() {
        let mut deep = item(6, "level 6", Vec::new());
        for level in (1..=5).rev() {
            deep = item(level, &format!("level {level}"), vec![deep]);
        }
        let saved = parse(json(&store("/work/hocus", vec![deep])).unwrap().as_bytes());

        let names = texts(&list_json(&saved, "/work/hocus", false));
        assert_eq!(
            names,
            vec!["level 1", "level 2", "level 3", "level 4", "level 5"],
        );
    }

    #[test]
    fn a_list_is_cut_at_the_item_cap() {
        let items: Vec<Item> = (1..=MAX_ITEMS as u64 + 10)
            .map(|id| item(id, &format!("item {id}"), Vec::new()))
            .collect();
        let saved = parse(json(&store("/work/hocus", items)).unwrap().as_bytes());

        assert_eq!(counts(&saved["/work/hocus"]).1, MAX_ITEMS);
    }

    #[test]
    fn a_parent_with_an_open_sub_item_is_opened() {
        let saved = parse(
            br#"{"/work/hocus":{"items":[
                {"id":1,"text":"parent","done":true,"children":[{"id":2,"text":"child"}]}]}}"#,
        );
        assert!(!saved["/work/hocus"].items[0].done);
    }

    #[test]
    fn a_done_parent_with_a_done_sub_item_stays_done() {
        let saved = parse(
            br#"{"/work/hocus":{"items":[
                {"id":1,"text":"parent","done":true,
                 "children":[{"id":2,"text":"child","done":true}]}]}}"#,
        );
        assert!(saved["/work/hocus"].items[0].done);
    }

    #[test]
    fn a_list_with_no_items_leaves_no_row() {
        assert!(parse(br#"{"/work/hocus":{"next_id":9,"items":[]}}"#).is_empty());
    }

    #[test]
    fn two_keys_that_name_one_folder_join_into_one_list() {
        let saved = parse(
            br#"{"/work/hocus":{"items":[{"id":1,"text":"first"}]},
                 "/work/hocus/":{"items":[{"id":1,"text":"second"}]}}"#,
        );
        assert_eq!(saved.len(), 1);
        assert_eq!(saved["/work/hocus"].items.len(), 2);
    }

    #[test]
    fn a_store_round_trips_through_json_and_parse() {
        let saved = store(
            "/work/hocus",
            vec![
                item(
                    1,
                    "ship the parser",
                    vec![done(item(2, "write the test", vec![]))],
                ),
                item(3, "answer the review", Vec::new()),
            ],
        );
        assert_eq!(parse(json(&saved).unwrap().as_bytes()), saved);
    }

    #[test]
    fn nothing_saved_is_an_empty_store() {
        assert!(parse(b"").is_empty());
        assert!(parse(b"not json").is_empty());
    }

    // -- adding -------------------------------------------------------------

    #[test]
    fn ids_start_at_one_and_never_come_back() {
        let mut saved = Store::new();
        assert_eq!(add(&mut saved, "/work/hocus", "first", None), Ok(1));
        assert_eq!(add(&mut saved, "/work/hocus", "second", None), Ok(2));
        saved.get_mut("/work/hocus").unwrap().items.clear();
        assert_eq!(add(&mut saved, "/work/hocus", "third", None), Ok(3));
    }

    #[test]
    fn an_item_lands_under_its_parent() {
        let mut saved = Store::new();
        let parent = add(&mut saved, "/work/hocus", "parent", None).unwrap();
        let child = add(&mut saved, "/work/hocus", "child", Some(parent)).unwrap();

        let items = &saved["/work/hocus"].items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children[0].id, child);
    }

    #[test]
    fn a_parent_id_from_another_project_is_refused() {
        let mut saved = Store::new();
        add(&mut saved, "/work/other", "elsewhere", None).unwrap();
        let error = add(&mut saved, "/work/hocus", "child", Some(1)).unwrap_err();

        assert!(
            error.starts_with("no todo item 1 in this project"),
            "{error}"
        );
        assert!(!saved.contains_key("/work/hocus"), "no empty list was left");
    }

    #[test]
    fn text_that_is_empty_or_too_long_is_refused() {
        let mut saved = Store::new();
        assert!(add(&mut saved, "/work/hocus", "   ", None).is_err());
        let long = "x".repeat(MAX_TEXT + 1);
        assert!(add(&mut saved, "/work/hocus", &long, None).is_err());
        assert!(saved.is_empty());
    }

    #[test]
    fn a_parent_at_the_depth_cap_is_refused() {
        let mut saved = Store::new();
        let mut parent = add(&mut saved, "/work/hocus", "level 1", None).unwrap();
        for level in 2..=MAX_DEPTH {
            parent = add(
                &mut saved,
                "/work/hocus",
                &format!("level {level}"),
                Some(parent),
            )
            .unwrap();
        }
        let error = add(&mut saved, "/work/hocus", "too deep", Some(parent)).unwrap_err();
        assert_eq!(
            error,
            format!("todo items nest {MAX_DEPTH} levels deep at most")
        );
    }

    #[test]
    fn a_list_at_the_open_item_cap_is_refused() {
        let mut saved = Store::new();
        for index in 0..MAX_OPEN {
            add(&mut saved, "/work/hocus", &format!("item {index}"), None).unwrap();
        }
        let error = add(&mut saved, "/work/hocus", "one more", None).unwrap_err();
        assert!(error.contains("cross some off first"), "{error}");
    }

    #[test]
    fn adding_under_a_done_parent_opens_it() {
        let mut saved = store("/work/hocus", vec![done(item(1, "parent", Vec::new()))]);
        add(&mut saved, "/work/hocus", "more work", Some(1)).unwrap();

        assert!(!saved["/work/hocus"].items[0].done);
    }

    // -- reading ------------------------------------------------------------

    #[test]
    fn a_list_hides_done_items_unless_they_are_asked_for() {
        let saved = store(
            "/work/hocus",
            vec![
                item(1, "open", Vec::new()),
                done(item(2, "finished", Vec::new())),
            ],
        );
        assert_eq!(
            texts(&list_json(&saved, "/work/hocus", false)),
            vec!["open"]
        );
        assert_eq!(
            texts(&list_json(&saved, "/work/hocus", true)),
            vec!["open", "finished"],
        );
    }

    #[test]
    fn a_list_hides_the_sub_items_of_a_done_item() {
        let saved = store(
            "/work/hocus",
            vec![done(item(
                1,
                "parent",
                vec![done(item(2, "child", Vec::new()))],
            ))],
        );
        assert!(texts(&list_json(&saved, "/work/hocus", false)).is_empty());
        assert_eq!(
            texts(&list_json(&saved, "/work/hocus", true)),
            vec!["parent", "child"],
        );
    }

    #[test]
    fn a_list_gives_every_item_an_id_at_every_depth() {
        let saved = store(
            "/work/hocus",
            vec![item(1, "parent", vec![item(2, "child", Vec::new())])],
        );
        let value = list_json(&saved, "/work/hocus", false);

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
        let value = list_json(&Store::new(), "/work/hocus", false);
        assert_eq!(value["open"], 0);
        assert!(value["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn counts_separate_open_items_from_done_items() {
        let list = &store(
            "/work/hocus",
            vec![item(1, "open", vec![done(item(2, "child", Vec::new()))])],
        )["/work/hocus"]
            .clone();
        assert_eq!(counts(list), (1, 2));
    }

    // -- searching ----------------------------------------------------------

    #[test]
    fn a_search_ignores_case_and_matches_part_of_the_text() {
        let saved = store("/work/hocus", vec![item(1, "Ship the Parser", Vec::new())]);
        assert_eq!(search(&saved, &roots(&["/work/hocus"]), "parse").len(), 1);
        assert!(search(&saved, &roots(&["/work/hocus"]), "compiler").is_empty());
    }

    #[test]
    fn a_search_gives_the_items_above_the_match() {
        let saved = store(
            "/work/hocus",
            vec![item(
                1,
                "release 2",
                vec![item(2, "ship the parser", Vec::new())],
            )],
        );
        let found = search(&saved, &roots(&["/work/hocus"]), "parser");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, 2);
        assert_eq!(found[0].root, "/work/hocus");
        assert_eq!(found[0].trail, vec!["release 2", "ship the parser"]);
    }

    #[test]
    fn a_search_finds_an_item_that_is_crossed_off() {
        let saved = store("/work/hocus", vec![done(item(1, "ship it", Vec::new()))]);
        let found = search(&saved, &roots(&["/work/hocus"]), "ship");

        assert_eq!(found.len(), 1);
        assert!(found[0].done);
    }

    #[test]
    fn a_search_over_every_list_answers_in_folder_order() {
        let mut saved = Store::new();
        add(&mut saved, "/work/pocus", "ship it", None).unwrap();
        add(&mut saved, "/work/hocus", "ship it", None).unwrap();
        let found = search(&saved, &roots(&["/work/hocus", "/work/pocus"]), "ship");

        let where_found: Vec<&str> = found.iter().map(|found| found.root.as_str()).collect();
        assert_eq!(where_found, vec!["/work/hocus", "/work/pocus"]);
    }

    #[test]
    fn a_search_is_cut_at_the_match_cap() {
        let items: Vec<Item> = (1..=MAX_MATCHES as u64 + 5)
            .map(|id| item(id, &format!("ship {id}"), Vec::new()))
            .collect();
        let saved = store("/work/hocus", items);

        assert_eq!(
            search(&saved, &roots(&["/work/hocus"]), "ship").len(),
            MAX_MATCHES
        );
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let saved = store("/work/hocus", vec![item(1, "ship it", Vec::new())]);
        assert!(search(&saved, &roots(&["/work/hocus"]), "  ").is_empty());
    }

    // -- crossing off -------------------------------------------------------

    #[test]
    fn crossing_an_item_off_crosses_off_the_items_below_it() {
        let mut saved = store(
            "/work/hocus",
            vec![item(
                1,
                "parent",
                vec![item(2, "child", vec![item(3, "grandchild", Vec::new())])],
            )],
        );
        let change = set_done(&mut saved, "/work/hocus", 1, true).unwrap();

        assert_eq!(change.cascaded, vec![2, 3]);
        assert!(change.reopened.is_empty());
        assert_eq!(counts(&saved["/work/hocus"]), (0, 3));
    }

    #[test]
    fn an_undo_opens_the_items_above_and_leaves_the_items_below_done() {
        let mut saved = store(
            "/work/hocus",
            vec![item(
                1,
                "parent",
                vec![item(2, "child", vec![item(3, "leaf", vec![])])],
            )],
        );
        set_done(&mut saved, "/work/hocus", 1, true).unwrap();
        let change = set_done(&mut saved, "/work/hocus", 3, false).unwrap();

        assert_eq!(change.reopened, vec![1, 2]);
        assert!(change.cascaded.is_empty());
        // Only the leaf and the items above it came back.
        assert_eq!(counts(&saved["/work/hocus"]), (3, 3));
    }

    #[test]
    fn an_undo_only_reports_the_items_it_changed() {
        let mut saved = store(
            "/work/hocus",
            vec![item(1, "parent", vec![done(item(2, "child", Vec::new()))])],
        );
        let change = set_done(&mut saved, "/work/hocus", 2, false).unwrap();

        assert!(change.reopened.is_empty(), "the parent was open already");
    }

    #[test]
    fn crossing_off_an_item_that_is_already_done_is_not_an_error() {
        let mut saved = store("/work/hocus", vec![done(item(1, "ship it", Vec::new()))]);
        let change = set_done(&mut saved, "/work/hocus", 1, true).unwrap();

        assert!(change.done);
        assert!(change.cascaded.is_empty());
    }

    #[test]
    fn an_id_that_is_not_in_the_list_is_reported() {
        let mut saved = store("/work/hocus", vec![item(1, "ship it", Vec::new())]);
        assert_eq!(
            set_done(&mut saved, "/work/hocus", 9, true),
            Err("no todo item 9 in this project; call todo_list".to_owned()),
        );
        assert!(set_done(&mut saved, "/work/other", 1, true).is_err());
    }
}
