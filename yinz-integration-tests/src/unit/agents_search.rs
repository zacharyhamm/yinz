use crate::{keys, test_env, LayoutInfo, Size, TestRunner};
use agent_proto::{agents, store};
use serde_json::json;
use yinz_storage::data_access::DataAccess;

#[test]
fn agents_prompt_search_through_default_key_bindings() {
    let data = DataAccess::sqlite(&test_env::init().join("data"));
    for definition in store::shared_defs() {
        data.plugin_data()
            .define_entity(store::SHARED, "test", &definition, false)
            .unwrap();
    }
    for (id, kind, text) in [
        ("prompt-run", "prompt", "history PROMPT-HIT"),
        ("assistant-run", "message", "history ASSISTANT-HIT"),
    ] {
        let run: agents::AgentRun = serde_json::from_value(json!({
            "id":id,"pane_id":999,"kind":"claude","session_id":id,
            "started_at_ms":1000,"ended_at_ms":2000,"outcome":"completed","revision":1,
            "entries":[{"revision":1,"kind":kind,"text":text,"created_at_ms":1000}]
        }))
        .unwrap();
        data.plugin_data()
            .apply(store::SHARED, &agents::rows::run_mutations(&run))
            .unwrap();
    }
    let mut yinz = TestRunner::new(Size {
        rows: 40,
        cols: 160,
    })
    .with_config(
        r#"
plugins {
  agents location="yinz:agents" { shared true; width "90%"; height "80%"; }
}
"#,
    )
    .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
    .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"search routing ready");
    yinz.wait_until("initial layout rendered", |frame| {
        frame.contains("search routing ready")
    });
    yinz.send_stdin(&keys::ctrl('e'));
    yinz.send_stdin(b"a");
    yinz.wait_until("Agents panel loaded", |frame| {
        frame.contains("Agents") && frame.contains("/ search")
    });
    yinz.send_stdin(b"/prompt: history");
    yinz.wait_until("prefixed query entered through stdin", |frame| {
        frame.contains("Search [User prompts]: prompt: history") && frame.contains("Enter apply")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("only the user prompt matches", |frame| {
        frame.contains("Search [User prompts]: prompt: history")
            && frame.contains("PROMPT-HIT")
            && !frame.contains("ASSISTANT-HIT")
            && !frame.contains("Enter apply")
    });
    yinz.send_stdin(b"/");
    yinz.send_stdin(&keys::ctrl('u'));
    yinz.send_stdin(b"history");
    yinz.wait_until("unprefixed query entered through stdin", |frame| {
        frame.contains("Search [All messages]: history") && frame.contains("Enter apply")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("both prompt and assistant messages match", |frame| {
        frame.contains("Search [All messages]: history")
            && frame.contains("PROMPT-HIT")
            && frame.contains("ASSISTANT-HIT")
            && !frame.contains("Enter apply")
    });
    yinz.quit();
}
