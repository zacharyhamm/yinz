//! Pure todo decoding and guarded mutation planning shared with the original plugins.
use agent_proto::todos;
use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

fn arg<T: DeserializeOwned>(args: &Value, index: usize) -> Result<T> {
    serde_json::from_value(args.get(index).cloned().unwrap_or(Value::Null))
        .with_context(|| format!("invalid todo argument {index}"))
}

pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    anyhow::ensure!(args.is_array(), "todo arguments must be an array");
    Ok(match op {
        "FromRows" => json!(todos::from_rows(&arg(&args, 0)?)),
        "ItemsFromRows" => json!(todos::items_from_rows(&arg(&args, 0)?)),
        "NormalizeRoot" => json!(todos::normalize_root(&arg::<String>(&args, 0)?)),
        "Resolve" => json!(todos::resolve(&arg(&args, 0)?, &arg::<String>(&args, 1)?)),
        "ListJson" => todos::list_json(&arg(&args, 0)?, &arg::<String>(&args, 1)?, arg(&args, 2)?),
        "CleanText" => json!(todos::clean_text(&arg::<String>(&args, 0)?)),
        "Counts" => json!(todos::counts(&arg(&args, 0)?)),
        "Item" => json!(todos::item_of(&arg(&args, 0)?, arg(&args, 1)?)),
        "Trail" => json!(todos::trail_of(&arg::<todos::List>(&args, 0)?.items, arg(&args, 1)?)),
        "PlanAdd" => json!(todos::plan_add(&arg(&args, 0)?, &arg::<String>(&args, 1)?, &arg::<String>(&args, 2)?, arg(&args, 3)?)),
        "PlanEdit" => json!(todos::plan_edit(&arg(&args, 0)?, arg(&args, 1)?, &arg::<String>(&args, 2)?, arg(&args, 3)?)),
        "PlanDelete" => json!(todos::plan_delete(&arg(&args, 0)?, &arg::<String>(&args, 1)?, arg(&args, 2)?, arg(&args, 3)?)),
        "PlanMove" => json!(todos::plan_move(&arg(&args, 0)?, &arg::<String>(&args, 1)?, arg(&args, 2)?, arg(&args, 3)?, arg(&args, 4)?)),
        "PlanMoveRoot" => json!(todos::plan_move_root(&arg::<String>(&args, 0)?, &arg::<String>(&args, 1)?)),
        "PlanSetDone" | "PlanDone" => json!(todos::plan_set_done(&arg(&args, 0)?, &arg::<String>(&args, 1)?, arg(&args, 2)?, arg(&args, 3)?)
            .map(|(plan, change)| (plan, json!({"id":change.id,"text":change.text,"done":change.done,"cascaded":change.cascaded,"reopened":change.reopened})))),
        "ParseLegacy" => json!(todos::parse_legacy(arg::<String>(&args, 0)?.as_bytes())),
        "ImportUid" => json!(todos::import_uid(&arg(&args, 0)?, &arg::<String>(&args, 1)?, arg(&args, 2)?)),
        "ImportMutations" => {
            #[derive(serde::Deserialize)]
            struct Pending {
                old_id: u64,
                old_parent: Option<u64>,
                text: String,
                done: bool,
            }
            let level: Vec<Pending> = arg(&args, 2)?;
            let level: Vec<_> = level.into_iter().map(|item| todos::Pending {
                old_id: item.old_id, old_parent: item.old_parent, text: item.text, done: item.done,
            }).collect();
            json!(todos::import_mutations(&arg(&args, 0)?, &arg::<String>(&args, 1)?, &level, &arg(&args, 3)?, &arg(&args, 4)?))
        },
        "LevelsForImport" => json!(todos::levels_for_import(&arg(&args, 0)?).into_iter().map(|level| level.into_iter().map(|item| json!({"old_id":item.old_id,"old_parent":item.old_parent,"text":item.text,"done":item.done})).collect::<Vec<_>>()).collect::<Vec<_>>()),
        _ => bail!("unknown todo operation {op}"),
    })
}
