//! Shell archive queries share the same readers and bounds as Hocus.
use hocus_proto::{
    agents, projects, run_history,
    store::{self, Backend},
};
use serde_json::{json, Value};
use yinz_tile::prelude::{FetchRequest, HostId, SessionName};

pub fn query(backend: &impl Backend, op: &str, args: &Value) -> Result<String, String> {
    let text = |key| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let number = |key, default| {
        usize::try_from(args.get(key).and_then(Value::as_u64).unwrap_or(default))
            .unwrap_or(usize::MAX)
    };
    let limit = number("limit", if op == "agent_history_search" { 25 } else { 20 });
    let rows = backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?;
    if rows.truncated {
        return Err("project registry is incomplete".into());
    }
    let registry = projects::from_rows(&rows);
    let filter = run_history::Filter {
        yinz_session_name: text("run_session").map(SessionName::new),
        host_id: text("host_id").map(HostId::new),
        root: if let Some(name) = text("project") {
            let project = registry
                .get(name)
                .ok_or_else(|| format!("unknown project {name}; use project-list"))?;
            Some(run_history::RootScope::new(
                &project.root,
                HostId::new(
                    text("local_host_id")
                        .ok_or("project history requires the current host identity")?,
                ),
            ))
        } else {
            None
        },
        kind: text("kind").map(str::to_owned),
        since_ms: args.get("since_ms").and_then(Value::as_i64),
        active: args.get("active").and_then(Value::as_bool),
    };
    let now = agents::now_ms();
    let scope = format!(
        "{}{}{}",
        text("run_session")
            .map(|s| format!("session {s}"))
            .unwrap_or_else(|| "all sessions".into()),
        text("host_id")
            .map(|s| format!(" · host {s}"))
            .unwrap_or_default(),
        text("project")
            .map(|s| format!(" · {s}"))
            .unwrap_or_default()
    );
    let result = match op {
        "agent_runs" => {
            let offset = number("offset", 0);
            let page = run_history::list_runs(backend, &filter, limit, offset)?;
            let runs: Vec<_> = page
                .headers
                .iter()
                .map(|header| run_history::header_reply(header, &registry, now))
                .collect();
            json!({"runs":runs,"scope":scope,"count":runs.len(),"offset":offset,"more":page.more,"next_offset":page.more.then(|| offset.saturating_add(page.scanned))})
        },
        "agent_run" | "agent_predecessors" => {
            let id = text("run_id").ok_or("run_id is required")?;
            let header =
                run_history::run_header(backend, id)?.ok_or_else(|| format!("no such run {id}"))?;
            if op == "agent_predecessors" {
                let (runs, diagnostic) =
                    run_history::predecessors(&header, |id| run_history::run_header(backend, id));
                json!({"anchor":header,"runs":runs,"diagnostic":diagnostic})
            } else {
                let from = args
                    .get("from_revision")
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        args.get("revision")
                            .and_then(Value::as_u64)
                            .map(run_history::anchored_from)
                    });
                let page = run_history::entry_page(backend, id, from, limit)?;
                let entries: Vec<_> = page.entries.iter().map(|entry| json!({"revision":entry.revision,"kind":entry.kind.label(),"created_at_ms":entry.created_at_ms,"age":entry.created_at_ms.map(|at| agents::short_age(now,at)),"text":entry.text})).collect();
                let mut reply = run_history::header_reply(&header, &registry, now);
                reply["shown"] = json!(entries.len());
                reply["entries"] = json!(entries);
                reply["entry_count"] = json!(page.total);
                reply["earlier"] = json!(page.earlier);
                reply["next_revision"] = json!(page.next_revision);
                reply["more"] = json!(page
                    .next_revision
                    .map(|next| format!("agent_run run_id={id} from_revision={next}")));
                reply
            }
        },
        "agent_history_search" => {
            let found = run_history::search(
                backend,
                &filter,
                text("run_id"),
                text("query").ok_or("query is required")?,
                limit,
            )?;
            let hits: Vec<_> = found.hits.iter().map(|hit| {
                let run = found.runs.get(&hit.run_id);
                json!({"run_id":hit.run_id,"revision":hit.revision,"kind":hit.kind.label(),"created_at_ms":hit.created_at_ms,"age":hit.created_at_ms.map(|at|agents::short_age(now,at)),"snippet":hit.snippet,"run_title":run.map(|r|&r.title),"agent":run.map(|r|&r.kind)})
            }).collect();
            json!({"query":text("query"),"scope":scope,"count":hits.len(),"matches":hits,"truncated":found.truncated})
        },
        _ => return Err(format!("unknown archive operation {op}")),
    };
    Ok(result.to_string())
}
