use crate::{to_scheme, Runtime};
use agent_proto::{agents, markdown};
use serde_json::{json, Value};

fn runtime() -> Runtime {
    Runtime::new(|_| anyhow::bail!("document models must not call the host")).unwrap()
}
fn run() -> agents::AgentRun {
    serde_json::from_value(json!({
        "id":"claude:recorded-session", "pane_id":7, "kind":"claude", "started_at_ms":1_000,
        "entries":[{"revision":1,"kind":"prompt","text":"**Hello** 日本語","created_at_ms":2_000},
                   {"revision":2,"kind":"tool_result","text":"**literal**\n\u{1b}[31mred","created_at_ms":3_000}]
    })).unwrap()
}
fn lines(lines: &[markdown::MdLine]) -> Value {
    json!(lines.iter().map(|line| json!({"text":line.text, "levels":markdown::style_text(&line.text,&line.styles,false).indices()})).collect::<Vec<_>>())
}

#[test]
fn history_fingerprint_ignores_json_map_order_and_default_field_omission() {
    let mut runtime = runtime();
    let value = serde_json::to_value(run()).unwrap();
    runtime
        .engine
        .register_value("run", to_scheme(&value).unwrap());
    runtime.eval("(define before (yinz-agent-model \"history\" \"Fingerprint\" run)) (define after (yinz-agent-model \"history\" \"Fingerprint\" (hash-insert run \"omitted_entries\" 0)))").unwrap();
    assert_eq!(
        runtime.value("before").unwrap(),
        runtime.value("after").unwrap()
    );
    assert_eq!(
        runtime.value("before").unwrap(),
        json!(agents::run_payload_fingerprint(
            &serde_json::to_string(&run()).unwrap()
        ))
    );
}

#[test]
fn parsed_history_keeps_shared_markdown_styles_across_updates_and_resizes() {
    let mut runtime = runtime();
    let mut run = run();
    runtime.engine.register_value(
        "run",
        to_scheme(&serde_json::to_value(&run).unwrap()).unwrap(),
    );
    runtime.eval("(define document (yinz-history-create run)) (define (render-history width) (yinz-history-call document \"Lines\" (list width 90000 0)))").unwrap();
    let mut original = agents::ParsedRun::new(&run);
    for width in [0, 4, 20, 80] {
        runtime
            .eval(&format!("(define rendered (render-history {width}))"))
            .unwrap();
        let actual = runtime.value("rendered").unwrap();
        let expected = original.lines(width, 90_000, 0);
        assert_eq!(actual["entries"], lines(&expected.entries));
        assert_eq!(actual["header"], lines(&expected.header));
        assert_eq!(actual["label"], expected.label);
        assert_eq!(
            actual["events"].as_array().unwrap().len(),
            expected.events.len()
        );
    }
    run.entries.push(
        serde_json::from_value(
            json!({"revision":3,"kind":"result","text":"new **result**","created_at_ms":80_000}),
        )
        .unwrap(),
    );
    runtime.engine.register_value(
        "next",
        to_scheme(&serde_json::to_value(&run).unwrap()).unwrap(),
    );
    runtime
        .eval("(yinz-history-call document \"Update\" next) (define rendered (render-history 30))")
        .unwrap();
    original.update(&run);
    assert_eq!(
        runtime.value("rendered").unwrap()["entries"],
        lines(&original.lines(30, 90_000, 0).entries)
    );
    assert!(runtime.eval("(render-history 1000000)").is_err());
    assert!(runtime.eval("(yinz-command document)").is_err());
}

#[test]
fn parsed_reader_folds_and_literal_tools_match_existing_document_models() {
    let mut runtime = runtime();
    let text = "<task-notification><task-id>task-7</task-id><status>completed</status><summary>Finished</summary><result>first line\nsecond **line**</result></task-notification>";
    runtime
        .engine
        .register_value("source", to_scheme(&json!(text)).unwrap());
    runtime.eval("(define document (yinz-document-create \"prompt\" source)) (define sections (yinz-document-call document \"Sections\" json-null))").unwrap();
    let sections = runtime.value("sections").unwrap();
    assert!(!sections.as_array().unwrap().is_empty());
    for open in [false, true] {
        let mut originals = agent_proto::envelope::sections(text);
        let flags: Vec<_> = originals
            .iter_mut()
            .map(|section| {
                section.open = open;
                open
            })
            .collect();
        runtime
            .engine
            .register_value("flags", to_scheme(&json!(flags)).unwrap());
        runtime
            .eval("(define rendered (yinz-document-call document \"Wrap\" (list 30 #f flags)))")
            .unwrap();
        let blocks: Vec<_> = originals
            .iter()
            .map(agent_proto::envelope::section_blocks)
            .collect();
        let expected = agent_proto::envelope::wrap_sections(blocks.iter().map(Vec::as_slice), 30);
        let actual = runtime.value("rendered").unwrap();
        assert_eq!(actual["lines"], lines(&expected.lines));
        assert_eq!(actual["starts"], json!(expected.starts));
    }
    runtime.eval("(define literal (yinz-document-create \"tool_result\" \"**literal**\")) (define rendered (yinz-document-call literal \"Wrap\" '(30 #t ())))").unwrap();
    assert_eq!(
        runtime.value("rendered").unwrap()["lines"][0]["text"],
        "**literal**"
    );
    assert!(runtime
        .eval("(yinz-document-call document \"Wrap\" '(30 #f ()))")
        .is_err());
}
