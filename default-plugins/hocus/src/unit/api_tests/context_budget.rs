use super::*;

fn batch(transcript: &mut Vec<Message>, id: &str, name: &str, args: Value, output: &str) {
    transcript.push(Message::tool_calls(vec![call(id, name, &args.to_string())]));
    transcript.push(Message::tool_result(id, output));
}

#[test]
fn authoring_replay_is_bounded_on_both_transports_and_measurably_cheaper() {
    for api in [Api::Chat, Api::Responses] {
        let mut cfg = test_config();
        cfg.api = api;
        cfg.tools = true;
        let extension = json!({"type":"function","function":{"name":"extension_observe", "parameters":{"type":"object","properties":{}}}});
        let mut transcript = vec![Message::user("Keep my instructions verbatim. 🦀")];
        let mut old_total = 0;
        let mut new_total = 0;
        let mut old_peak = 0;
        let mut new_peak = 0;
        for round in 0..24 {
            let file = format!("plugin-{round}.ts");
            let source = format!(
                "// draft {round}\n{}",
                "export const value = '🦀';\n".repeat(450)
            );
            for (step, tool, args, output) in [
                (
                    "docs",
                    "plugin_docs",
                    json!({"query":format!("api-{round}")}),
                    "documentation hit\n".repeat(700),
                ),
                (
                    "read",
                    "plugin_read",
                    json!({"name":file,"start_line":1,"line_count":300}),
                    source.clone(),
                ),
                (
                    "write",
                    "plugin_write",
                    json!({"name":file,"source":source}),
                    "written".into(),
                ),
                (
                    "edit",
                    "plugin_edit",
                    json!({"name":file,"edits":[{"old_string":source,"new_string":source.replace("draft", "final")}]}),
                    "edited".into(),
                ),
                (
                    "check",
                    "plugin_check",
                    json!({"name":file}),
                    "compiles".into(),
                ),
                (
                    "reload",
                    "plugin_reload",
                    json!({"name":file}),
                    "latest render\n".repeat(700),
                ),
                (
                    "extension",
                    "extension_observe",
                    json!({}),
                    "latest observation 🦀\n".repeat(700),
                ),
            ] {
                batch(
                    &mut transcript,
                    &format!("{round}-{step}"),
                    tool,
                    args,
                    &output,
                );
                // The pre-limit policy and serializers, with the same tools and prompt.
                let loaded = tools::loaded_families(transcript.iter().flat_map(|m| &m.tool_calls));
                let specs = tool_specs(&cfg, false, std::slice::from_ref(&extension), &loaded);
                let old = Elision::deduplicate(&transcript);
                let original = transcript.clone();
                let system = system_text(&cfg, None, false);
                let baseline = match api {
                    Api::Chat => chat_request_body(&cfg, &transcript, system, specs, &old),
                    Api::Responses => responses_request_body(
                        &cfg,
                        &transcript,
                        system,
                        specs.map(flatten_tools),
                        &old,
                    ),
                };
                let (body, stats) = request_body_with_stats(
                    &cfg,
                    &transcript,
                    None,
                    false,
                    std::slice::from_ref(&extension),
                );
                assert!(body.len() <= cfg.context_max_bytes);
                assert!(stats.result_bytes <= TOOL_RESULTS_BYTES);
                assert!(stats.transmitted_payload_bytes <= stats.original_payload_bytes);
                assert_eq!(transcript, original);
                let selected = Elision::plan(&transcript);
                for (i, m) in transcript
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.role == Role::Tool)
                {
                    let sent = selected.content(i).unwrap_or(&m.content);
                    assert!(sent.len() <= TOOL_RESULT_BYTES);
                    assert!(sent.len() <= m.content.len());
                }
                let latest = transcript.len() - 1;
                assert!(selected
                    .content(latest)
                    .unwrap_or(&output)
                    .starts_with(output.lines().next().unwrap()));
                assert_eq!(
                    body,
                    request_body_with_stats(
                        &cfg,
                        &transcript,
                        None,
                        false,
                        std::slice::from_ref(&extension)
                    )
                    .0,
                    "retry is deterministic"
                );
                old_total += baseline.len();
                new_total += body.len();
                old_peak = old_peak.max(baseline.len());
                new_peak = new_peak.max(body.len());
            }
        }
        eprintln!("{api:?}: peak bytes {old_peak} -> {new_peak}; cumulative bytes {old_total} -> {new_total}");
        assert!(
            new_total * 2 < old_total,
            "replay saves at least half on this fixture"
        );
        // Reopening reconstructs the policy entirely from the archived messages.
        let archive = serde_json::to_vec(&transcript).unwrap();
        let reopened: Vec<Message> = serde_json::from_slice(&archive).unwrap();
        assert_eq!(
            request_body(&cfg, &transcript),
            request_body(&cfg, &reopened)
        );
        assert_eq!(archive, serde_json::to_vec(&transcript).unwrap());
        let body = String::from_utf8(request_body(&cfg, &transcript)).unwrap();
        assert!(body.contains("Keep my instructions verbatim. 🦀"));
    }
}

#[test]
fn original_results_and_arguments_page_exactly_with_unicode_and_escaping() {
    let text = "🦀é中\n\t\"\\\u{0001}".repeat(3000);
    let mut transcript = Vec::new();
    batch(
        &mut transcript,
        "a",
        "plugin_write",
        json!({"name":"a.ts","source":text}),
        &text,
    );
    for (part, original) in [
        ("result", text.as_str()),
        ("arguments", transcript[0].tool_calls[0].arguments.as_str()),
    ] {
        let mut offset = 0;
        let mut restored = String::new();
        loop {
            let args = tools::Args::parse(
                &json!({"tool_call_id":"a","part":part,"offset":offset}).to_string(),
            )
            .unwrap();
            let output = tool_result_read(&transcript, &args).unwrap();
            assert!(output.len() <= TOOL_RESULT_BYTES);
            let page: Value = serde_json::from_str(&output).unwrap();
            let text = page["text"].as_str().unwrap();
            assert!(text.chars().count() <= 4000);
            restored.push_str(text);
            if page["next_offset"].is_null() {
                break;
            }
            let next = page["next_offset"].as_u64().unwrap() as usize;
            assert_eq!(next, offset + text.chars().count());
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(restored, original);
    }
    for args in [
        json!({"tool_call_id":"missing","part":"result"}),
        json!({"tool_call_id":"a","part":"other"}),
        json!({"tool_call_id":"a","part":"result","offset":999999}),
    ] {
        assert!(
            tool_result_read(&transcript, &tools::Args::parse(&args.to_string()).unwrap()).is_err()
        );
    }
    assert!(tools::Args::parse(r#"{"offset":-1}"#).is_err());
    let end = tools::Args::parse(
        &json!({"tool_call_id":"a","part":"result","offset":text.chars().count()}).to_string(),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&tool_result_read(&transcript, &end).unwrap()).unwrap(),
        json!({"text":"","next_offset":null})
    );
}

#[test]
fn old_completed_batches_mask_source_strings_but_preserve_pending_calls_and_reasoning() {
    let source = "export const a = 1;\n".repeat(1000);
    let mut first = Message::tool_calls(vec![
        call("write", "plugin_write", &json!({"name":"a.ts","source":source}).to_string()),
        call("edit", "plugin_edit", &json!({"name":"b.ts","edits":[{"old_string":source,"new_string":source,"replace_all":true}]}).to_string()),
        call("pending", "plugin_check", &json!({"name":"c.ts","source":source}).to_string()),
    ]);
    first.content = "Keep the assistant explanation".into();
    first.reasoning = Some(Reasoning {
        key: ReasoningKey::ReasoningContent,
        text: "required reasoning".into(),
    });
    first.reasoning_items = vec![ReasoningItem {
        id: "rs_1".into(),
        encrypted_content: "sealed".into(),
        before: 0,
    }];
    let mut transcript = vec![
        first,
        Message::tool_result("write", "ok"),
        Message::tool_result("edit", "ok"),
    ];
    for i in 0..2 {
        batch(
            &mut transcript,
            &i.to_string(),
            "plugin_check",
            json!({"name":format!("recent-{i}.ts"),"source":source}),
            "compiles",
        );
    }
    let selected = Elision::plan(&transcript);
    for at in 0..2 {
        let args: Value = serde_json::from_str(selected.arguments(0, at).unwrap()).unwrap();
        assert!(args.to_string().contains("tool_result_read"));
        assert_eq!(args["name"], if at == 0 { "a.ts" } else { "b.ts" });
        if at == 1 {
            assert_eq!(args["edits"][0]["replace_all"], true);
        }
    }
    assert!(selected.arguments(0, 2).is_none());
    assert!(selected.arguments(3, 0).is_none());
    assert!(selected.arguments(5, 0).is_none());
    for api in [Api::Chat, Api::Responses] {
        let cfg = Config {
            api,
            ..test_config()
        };
        let body = String::from_utf8(request_body(&cfg, &transcript)).unwrap();
        assert!(body.contains("Keep the assistant explanation"));
        assert!(body.contains(if api == Api::Chat {
            "required reasoning"
        } else {
            "sealed"
        }));
    }
}

#[test]
fn logged_sizes_and_breakdown_follow_the_selected_wire_format() {
    for api in [Api::Chat, Api::Responses] {
        let cfg = Config {
            api,
            tools: true,
            ..test_config()
        };
        let mut transcript = Vec::new();
        batch(
            &mut transcript,
            "external",
            "extension_observe",
            json!({}),
            &"🦀\n".repeat(10000),
        );
        let (body, stats) = request_body_with_stats(&cfg, &transcript, None, false, &[]);
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.bytes, value["tools"].to_string().len());
        assert_eq!(stats.omitted_results, 1);
        assert_eq!(
            stats.result_bytes,
            Elision::plan(&transcript).content(1).unwrap().len()
        );
        assert!(wire_breakdown(&body, 20)
            .iter()
            .any(|(label, _)| label == "result:extension_observe"));
        assert!(wire_breakdown(&body, 20)
            .iter()
            .any(|(label, _)| label == "tools"));
    }
}
