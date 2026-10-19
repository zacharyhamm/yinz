//! Pure note validation and mutation planning, without database or host access.
use super::{decode as args, encode as value};
use agent_proto::{notes, projects};
use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use yinz_types::{data::DataRows, internal_ids::DataRevision};

pub(super) fn run(op: &str, input: Value) -> Result<Value> {
    match op {
        "FromRows" => {
            let (rows,): (DataRows,) = args(input)?;
            value(notes::from_rows(&rows))
        },
        "NoteJson" => {
            let (note, now): (notes::Note, i64) = args(input)?;
            Ok(notes::note_json(&note, now))
        },
        "ProjectName" => {
            let (registry, root): (projects::Registry, Option<String>) = args(input)?;
            value(notes::project_name(&registry, root.as_deref()))
        },
        "PlanAdd" => {
            let (count, root, title, body, author, tags, ids): (
                usize,
                Option<String>,
                String,
                String,
                String,
                Vec<String>,
                Vec<u64>,
            ) = args(input)?;
            value(notes::plan_add(
                count,
                root.as_deref(),
                &title,
                &body,
                &author,
                &tags,
                &ids,
            ))
        },
        "PlanEdit" => {
            let (note, title, body, revision): (
                notes::Note,
                Option<String>,
                Option<String>,
                Option<DataRevision>,
            ) = args(input)?;
            value(notes::plan_edit(
                &note,
                title.as_deref(),
                body.as_deref(),
                revision,
            ))
        },
        "PlanAttach" => {
            let (note, tags, ids): (notes::Note, Vec<String>, Vec<u64>) = args(input)?;
            value(notes::plan_attach_to_note(&note, &tags, &ids))
        },
        "PlanSetTags" => {
            let (note, tags, revision, guard): (
                notes::Note,
                Vec<String>,
                Option<DataRevision>,
                bool,
            ) = args(input)?;
            value(notes::plan_set_tags(&note, &tags, revision, guard))
        },
        "PlanArchive" => {
            let (id, archived): (u64, bool) = args(input)?;
            value(notes::plan_archive(id, archived))
        },
        "PlanDelete" => {
            let (id,): (u64,) = args(input)?;
            value(notes::plan_delete(id))
        },
        "PlanUnlink" => {
            let (note, todo): (u64, u64) = args(input)?;
            value(notes::plan_unlink(note, todo))
        },
        "PlanMoveRoot" => {
            let (old, new): (String, String) = args(input)?;
            value(notes::plan_move_root(&old, &new))
        },
        "TagUid" => {
            let (id, tag): (u64, String) = args(input)?;
            value(notes::tag_uid(id, &tag))
        },
        "LinkUid" => {
            let (note, todo): (u64, u64) = args(input)?;
            value(notes::link_uid(note, todo))
        },
        "TodoIdsIn" => {
            let (listing,): (Value,) = args(input)?;
            value(notes::todo_ids_in(&listing))
        },
        "AnnotateTodoList" => {
            let (mut listing, links): (Value, BTreeMap<u64, Vec<u64>>) = args(input)?;
            notes::annotate_todo_list(&mut listing, &links);
            Ok(listing)
        },
        "Snippet" => {
            let (body,): (String,) = args(input)?;
            value(notes::snippet(&body))
        },
        "NormalizeTag" => {
            let (tag,): (String,) = args(input)?;
            value(notes::normalize_tag(&tag))
        },
        "ListColumns" => value(notes::LIST_COLUMNS),
        _ => bail!("unknown note model operation {op}"),
    }
}
