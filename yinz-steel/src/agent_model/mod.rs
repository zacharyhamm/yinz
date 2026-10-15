//! Whitelisted pure shared models. No backend, host shim, or filesystem API is exposed.
mod agents;
mod archive;
mod history;
mod launch;
mod notes;
mod panel;
mod projects;
mod store;
mod todos;

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use steel::{
    rvals::SteelVal,
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};

struct SnapshotCache<T>(Mutex<Option<(SteelVal, Arc<T>)>>);
impl<T> Default for SnapshotCache<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}
impl<T> SnapshotCache<T> {
    fn get(&self, input: &SteelVal, decode: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let mut cached = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("model cache lock poisoned"))?;
        if let Some((_, value)) = cached.as_ref().filter(|(key, _)| same_input(key, input)) {
            return Ok(value.clone());
        }
        let value = Arc::new(decode()?);
        // Only immutable inputs can retain a validated result between model calls.
        *cached = immutable(input).then(|| (input.clone(), value.clone()));
        Ok(value)
    }
}
fn same_input(left: &SteelVal, right: &SteelVal) -> bool {
    match (left, right) {
        (SteelVal::ListV(left), SteelVal::ListV(right)) => {
            left.ptr_eq(right)
                || (left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| same_input(left, right)))
        },
        (SteelVal::HashMapV(_), SteelVal::HashMapV(_)) => {
            left.as_ptr_usize() == right.as_ptr_usize()
        },
        _ => false,
    }
}
fn immutable(value: &SteelVal) -> bool {
    match value {
        SteelVal::ListV(values) => values.iter().all(immutable),
        SteelVal::VectorV(values) => values.iter().all(immutable),
        SteelVal::HashMapV(values) => values
            .iter()
            .all(|(key, value)| immutable(key) && immutable(value)),
        SteelVal::ByteVector(_) => false,
        _ => true, // The bridge rejects all other mutable or unsupported values before caching.
    }
}
#[derive(Default)]
struct ModelCaches {
    providers: SnapshotCache<Vec<yinz_types::agent::provider::AgentProvider>>,
    pane_views: SnapshotCache<[Value; 3]>,
}

pub(crate) fn register(engine: &mut Engine) {
    archive::register(engine);
    history::register(engine);
    let cache = ModelCaches::default();
    engine.register_fn(
        "yinz-agent-model",
        move |namespace: String, op: String, args: SteelVal| -> steel::rvals::Result<SteelVal> {
            let value = yinz_types::agent::provider::with_snapshot(&[], || {
                run_scheme(&namespace, &op, &args, &cache)
            })
            .map_err(crate::scheme_error)?;
            crate::to_scheme(&value).map_err(crate::scheme_error)
        },
    );
}

fn argument(args: &SteelVal, index: usize) -> Result<&SteelVal> {
    match args {
        SteelVal::ListV(args) => args.iter().nth(index),
        SteelVal::VectorV(args) => args.get(index),
        _ => None,
    }
    .with_context(|| format!("missing model argument {index}"))
}
fn scalar<T: DeserializeOwned>(args: &SteelVal, index: usize) -> Result<T> {
    decode(crate::from_scheme(argument(args, index)?)?)
}
fn bytes(
    args: &SteelVal,
    index: usize,
    read: impl FnOnce(&[u8]) -> Result<Value>,
) -> Result<Value> {
    match argument(args, index)? {
        SteelVal::ByteVector(bytes) => bytes.with_bytes(|bytes| {
            anyhow::ensure!(
                bytes.len() <= crate::binary::MAX_BYTES,
                "model byte input exceeds 64 MiB"
            );
            read(bytes)
        }),
        value => read(&decode::<Vec<u8>>(crate::from_scheme(value)?)?),
    }
}
fn run_scheme(namespace: &str, op: &str, args: &SteelVal, cache: &ModelCaches) -> Result<Value> {
    match (namespace, op) {
        ("providers", "WithSnapshot") => {
            let input = argument(args, 0)?;
            let providers = cache
                .providers
                .get(input, || decode(crate::from_scheme(input)?))?;
            let namespace: String = scalar(args, 1)?;
            let op: String = scalar(args, 2)?;
            yinz_types::agent::provider::with_snapshot(&providers, || {
                run_scheme(&namespace, &op, argument(args, 3)?, cache)
            })
        },
        ("agents", "TerminalPanes" | "SuppressedPanes" | "FocusedPane") => {
            let views = cache.pane_views.get(args, || {
                agents::pane_views(crate::bridge::from_model(args)?)
            })?;
            Ok(views[match op {
                "TerminalPanes" => 0,
                "SuppressedPanes" => 1,
                _ => 2,
            }]
            .clone())
        },
        ("projects", "ParseLegacy") => bytes(args, 0, |bytes| {
            encode(agent_proto::projects::parse_legacy(bytes))
        }),
        ("agents", "ParseSummary") => bytes(args, 0, |body| {
            encode(agent_proto::agents::parse_summary(body, scalar(args, 1)?))
        }),
        ("agents", "CompletionParse") => bytes(args, 1, |body| {
            encode(agent_proto::completion::parse_completion(
                scalar(args, 0)?,
                body,
                &scalar::<String>(args, 2)?,
            ))
        }),
        ("agents", "GitProbe") => bytes(args, 1, |metadata| {
            bytes(args, 2, |config| {
                agents::git_probe(scalar(args, 0)?, metadata, config)
            })
        }),
        _ => run(namespace, op, crate::bridge::from_model(args)?),
    }
}

fn run(namespace: &str, op: &str, args: Value) -> Result<Value> {
    match namespace {
        "agents" => agents::run(op, args),
        "archive" => archive::run(op, args),
        "history" => history::run(op, args),
        "launch" => launch::run(op, args),
        "notes" => notes::run(op, args),
        "panel" => panel::run(op, args),
        "projects" => projects::run(op, args),
        "store" => store::run(op, args),
        "todos" => todos::run(op, args),
        _ => bail!("unknown agent model namespace {namespace:?}"),
    }
    .with_context(|| format!("agent model {namespace}.{op}"))
}

pub(super) fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("invalid model arguments")
}
pub(super) fn encode(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).context("serializing model result")
}
