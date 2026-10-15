//! Parsed documents are interpreter-owned values; selection, scrolling and host I/O stay in Scheme.
use super::{decode, encode};
use agent_proto::{agents, envelope, markdown};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use steel::{
    rvals::{Custom, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};

#[derive(Clone)]
struct ParsedHistory(Arc<Mutex<agents::ParsedRun>>);
impl Custom for ParsedHistory {}

#[derive(Clone)]
struct ParsedDocument(Arc<Mutex<Document>>);
impl Custom for ParsedDocument {}
struct Document {
    blocks: Vec<markdown::MdBlock>,
    sections: Vec<envelope::Section>,
    folds: Vec<Option<(bool, Vec<markdown::MdBlock>)>>,
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn(
        "yinz-history-create",
        |value: SteelVal| -> steel::rvals::Result<ParsedHistory> {
            let run = decode(crate::bridge::from_model(&value).map_err(crate::scheme_error)?)
                .map_err(crate::scheme_error)?;
            Ok(ParsedHistory(Arc::new(Mutex::new(agents::ParsedRun::new(
                &run,
            )))))
        },
    );
    engine.register_fn(
        "yinz-history-call",
        |native: &ParsedHistory, op: String, value: SteelVal| -> steel::rvals::Result<SteelVal> {
            let result = (|| -> Result<Value> {
                let args = crate::bridge::from_model(&value)?;
                let mut parsed = native
                    .0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("parsed history lock poisoned"))?;
                match op.as_str() {
                    "Update" => {
                        parsed.update(&decode(args)?);
                        Ok(Value::Null)
                    },
                    "Lines" => {
                        let (width, now, offset) = decode(args)?;
                        check_width(width)?;
                        Ok(history(&parsed.lines(width, now, offset)))
                    },
                    _ => bail!("unknown parsed history operation {op}"),
                }
            })()
            .map_err(crate::scheme_error)?;
            crate::to_scheme(&result).map_err(crate::scheme_error)
        },
    );
    engine.register_fn(
        "yinz-document-create",
        |kind: String, text: String| -> ParsedDocument {
            let sections = if kind == "prompt" {
                envelope::sections(&text)
            } else {
                vec![]
            };
            let folds = (0..sections.len()).map(|_| None).collect();
            ParsedDocument(Arc::new(Mutex::new(Document {
                blocks: agents::entry_blocks(&kind, &text),
                sections,
                folds,
            })))
        },
    );
    engine.register_fn("yinz-document-call", |native: &ParsedDocument, op: String, value: SteelVal| -> steel::rvals::Result<SteelVal> {
        let result = (|| -> Result<Value> {
            let args = crate::from_scheme(&value)?;
            let mut doc = native.0.lock().map_err(|_| anyhow::anyhow!("parsed document lock poisoned"))?;
            match op.as_str() {
                "Sections" => Ok(Value::Array(doc.sections.iter().map(section).collect())),
                "Wrap" => {
                    let (width, raw, opened): (usize, bool, Vec<bool>) = decode(args)?;
                    check_width(width)?;
                    if raw || doc.sections.is_empty() {
                        let lines = markdown::wrap(&doc.blocks, width, markdown::Options::compact());
                        Ok(json!({"lines": lines.iter().map(line).collect::<Vec<_>>(), "starts": []}))
                    } else {
                        anyhow::ensure!(opened.len() == doc.sections.len(), "section flags must match the document");
                        for (index, open) in opened.into_iter().enumerate() {
                            if doc.folds[index].as_ref().is_none_or(|(old, _)| *old != open) {
                                doc.sections[index].open = open;
                                doc.folds[index] = Some((open, envelope::section_blocks(&doc.sections[index])));
                            }
                        }
                        let result = envelope::wrap_sections(doc.folds.iter().filter_map(|f| f.as_ref().map(|(_, blocks)| blocks.as_slice())), width);
                        Ok(json!({"lines": result.lines.iter().map(line).collect::<Vec<_>>(), "starts": result.starts}))
                    }
                },
                _ => bail!("unknown parsed document operation {op}"),
            }
        })().map_err(crate::scheme_error)?;
        crate::to_scheme(&result).map_err(crate::scheme_error)
    });
}

fn check_width(width: usize) -> Result<()> {
    anyhow::ensure!(
        width <= crate::MAX_OUTPUT_BYTES / 256,
        "history width exceeds transport budget"
    );
    Ok(())
}
fn line(value: &markdown::MdLine) -> Value {
    let styled = markdown::style_text(&value.text, &value.styles, false);
    json!({"text": value.text, "levels": styled.indices()})
}
fn history(value: &agents::HistoryLines) -> Value {
    json!({
        "label": value.label,
        "entries": value.entries.iter().map(line).collect::<Vec<_>>(),
        "header": value.header.iter().map(line).collect::<Vec<_>>(),
        "events": value.events.iter().map(|event| json!({"revision": event.revision, "start": event.start, "len": event.len,
            "kind": event.kind, "created_at_ms": event.created_at_ms, "text": event.text.as_ref()})).collect::<Vec<_>>(),
    })
}
fn section(value: &envelope::Section) -> Value {
    json!({"name": value.name, "text": value.text, "open": value.open, "path": value.path,
        "lines": value.lines, "truncated": value.truncated})
}
pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    match op {
        "Summary" => {
            let (pane, kind, summary, waiting, width, now): (
                u32,
                String,
                agents::Summary,
                bool,
                usize,
                i64,
            ) = decode(args)?;
            check_width(width)?;
            Ok(history(&agents::summary_history(
                pane, &kind, &summary, waiting, width, now,
            )))
        },
        "EnvelopeTag" => {
            let text: String = decode(args)?;
            encode(
                envelope::parse(&text)
                    .envelopes
                    .first()
                    .map(|value| value.tag),
            )
        },
        "Fingerprint" => {
            let run: agents::AgentRun =
                serde_json::from_value(args).context("decoding history run for fingerprint")?;
            let payload = serde_json::to_string(&run).context("encoding history payload")?;
            encode(agents::run_payload_fingerprint(&payload))
        },
        _ => bail!("unknown history operation {op}"),
    }
}
