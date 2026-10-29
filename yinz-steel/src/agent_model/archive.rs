//! Pure archive models and page accounting; storage reads remain in the Scheme guest.
use super::{decode, encode};
use agent_proto::{
    agents::{AgentRunHeader, RunEntry},
    projects, run_history,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use steel::{
    rvals::{Custom, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};
use yinz_types::data::DataRows;

type PatternMatcher = dyn Fn(&str) -> Option<String> + Send + Sync;

#[derive(Clone)]
struct Pattern(Arc<PatternMatcher>);
impl Custom for Pattern {}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn(
        "yinz-archive-pattern",
        |query: String| -> steel::rvals::Result<Pattern> {
            let pattern = run_history::search_pattern(&query).map_err(crate::scheme_error)?;
            Ok(Pattern(Arc::new(move |text| {
                pattern
                    .find(text)
                    .map(|found| run_history::regex_snippet(text, found.start()))
            })))
        },
    );
    engine.register_fn(
        "yinz-archive-match",
        |pattern: &Pattern, text: String| -> steel::rvals::Result<SteelVal> {
            crate::to_scheme(&json!((pattern.0)(&text))).map_err(crate::scheme_error)
        },
    );
}

pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    match op {
        "ListRunsRequest" => {
            let (filter, limit, offset): (run_history::Filter, usize, usize) = decode(args)?;
            encode(run_history::list_runs_request(&filter, limit, offset))
        },
        "ListRunsPage" => {
            let (filter, limit, rows): (run_history::Filter, usize, DataRows) = decode(args)?;
            encode(run_history::list_runs_page(&filter, limit, &rows))
        },
        "Filter" => {
            let (filter,): (run_history::Filter,) = decode(args)?;
            Ok(
                json!({"filter":filter,"predicate":filter.pred(),"narrows":filter.narrows_in_guest(),"empty":filter == run_history::Filter::default()}),
            )
        },
        "Keeps" => {
            let (filter, header): (run_history::Filter, AgentRunHeader) = decode(args)?;
            encode(filter.keeps(&header))
        },
        "HeaderReply" => {
            let (header, registry, now): (AgentRunHeader, projects::Registry, i64) = decode(args)?;
            encode(run_history::header_reply(&header, &registry, now))
        },
        "EntryReply" => {
            let (entry, now): (RunEntry, i64) = decode(args)?;
            encode(run_history::entry_reply(&entry, now))
        },
        "Budget" => {
            let (mut entries, newest, exact): (Vec<RunEntry>, bool, bool) = decode(args)?;
            let mut truncated: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    !exact && entry.text.chars().count() > run_history::ENTRY_TEXT_CHARS
                })
                .map(|entry| entry.revision)
                .collect();
            run_history::budget(&mut entries, newest, exact);
            truncated.retain(|revision| entries.iter().any(|entry| entry.revision == *revision));
            Ok(json!({"entries":entries,"truncated_revisions":truncated}))
        },
        "HitsFromRows" => {
            let (rows, query, tools): (DataRows, String, bool) = decode(args)?;
            encode(run_history::hits_from_rows(&rows, &query, tools))
        },
        "SortHits" => {
            let (mut hits,): (Vec<run_history::Hit>,) = decode(args)?;
            hits.sort_by(|a, b| {
                b.created_at_ms
                    .cmp(&a.created_at_ms)
                    .then_with(|| b.revision.cmp(&a.revision))
            });
            encode(hits)
        },
        "KindLabel" => {
            let (kind,): (agent_proto::agents::RunEntryKind,) = decode(args)?;
            encode(kind.label())
        },
        "KindFromLabel" => {
            let (label,): (String,) = decode(args)?;
            encode(agent_proto::agents::RunEntryKind::from_label(&label))
        },
        "ShortAge" => {
            let (now, at): (i64, i64) = decode(args)?;
            encode(agent_proto::agents::short_age(now, at))
        },
        _ => bail!("unknown archive model operation {op}"),
    }
}
