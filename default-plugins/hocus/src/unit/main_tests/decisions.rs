use super::*;
use crate::decisions;
use hocus_proto::decisions::{DEFAULT_MODEL, DEFAULT_URL};
use serde_json::{json, Value};

fn arguments() -> Value {
    json!({
        "state": {"ticket": "Checkout is broken", "attempts": 2},
        "questions": {
            "team": {"type": "choice", "instructions": "Choose an owner", "criteria": {
                "payments": "Checkout problems", "frontend": null
            }},
            "urgency": {"type": "score", "instructions": ["Assess urgency"], "criteria": [
                "Can wait", {"urgent": "Blocks checkout"}
            ]},
            "is_bug": {"type": "noul", "instructions": "Is this a defect?", "criteria": {
                "true": "Reports a failure", "false": "Requests a feature"
            }}
        },
        "user": "test-user",
        "session_id": "test-session"
    })
}

fn response() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": DEFAULT_MODEL,
        "answers": {
            "team": {"type": "choice", "choice": "payments", "probabilities": {"payments": 0.9, "frontend": 0.1}, "confidence": 0.9},
            "urgency": {"type": "score", "score": 0.8, "legend": {"0": "Can wait", "1": "Blocks checkout"}},
            "is_bug": {"type": "noul", "noul": 0.95}
        },
        "usage": {"input_tokens": 42, "output_tokens": 8, "cost": 0.001},
        "id": "decision-request", "provider": "test-provider"
    }))
    .unwrap()
}

fn context(id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("tool_call_id".into(), id.into())])
}

fn decision_state() -> State {
    let mut state = ready_state();
    settle(&mut state);
    state
        .env
        .insert("OPENROUTER_API_KEY".into(), "test-key".into());
    let call = ToolCall {
        id: "decision-1".into(),
        name: "openrouter_decisions".into(),
        arguments: arguments().to_string(),
    };
    state.messages.push(Message::tool_calls(vec![call.clone()]));
    state.pending.push_back(call);
    state
}

#[test]
fn decisions_serializes_mixed_questions_and_resolves_each_calls_settings() {
    let mut state = decision_state();
    let args = arguments();
    let (endpoint, body) = decisions::prepare(&args.to_string(), &state.kdl, &state.env).unwrap();
    assert_eq!(endpoint.url, DEFAULT_URL);
    assert_eq!(endpoint.headers()["Authorization"], "Bearer test-key");
    let mut expected = args.clone();
    expected["model"] = json!(DEFAULT_MODEL);
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);

    state
        .env
        .insert("OPENROUTER_DECISIONS_MODEL".into(), "env-model".into());
    state
        .kdl
        .insert("decisions_model".into(), "configured-model".into());
    state.kdl.insert(
        "decisions_url".into(),
        "https://example.test/decisions".into(),
    );
    state
        .kdl
        .insert("decisions_api_key_env".into(), "DECISION_KEY".into());
    state.env.insert("DECISION_KEY".into(), "new-key".into());
    for override_model in [None, Some("per-call-model")] {
        let mut args = args.clone();
        if let Some(model) = override_model {
            args["model"] = json!(model);
        }
        let (endpoint, body) =
            decisions::prepare(&args.to_string(), &state.kdl, &state.env).unwrap();
        assert_eq!(endpoint.url, "https://example.test/decisions");
        assert_eq!(endpoint.headers()["Authorization"], "Bearer new-key");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["model"],
            override_model.unwrap_or("configured-model")
        );
    }
}

#[test]
fn decisions_configuration_errors_answer_the_tool_without_waiting() {
    for (kdl, expected) in [
        (BTreeMap::new(), "OPENROUTER_API_KEY"),
        (
            BTreeMap::from([("decisions_url".into(), "file:///tmp/decisions".into())]),
            "decisions_url",
        ),
    ] {
        let mut state = decision_state();
        state.kdl = kdl;
        state.env = BTreeMap::from([("OPENAI_API_KEY".into(), "not-a-fallback".into())]);
        let call = state.pending[0].clone();
        assert!(state.run_head(&call));
        assert!(matches!(state.status, Status::Idle));
        assert!(state.pending.is_empty());
        assert!(tool_results(&state)[0].content.contains(expected));
    }
}

#[test]
fn decisions_invalid_arguments_are_tool_results() {
    let mut bad = vec!["not json".into(), "[]".into(), "{}".into()];
    for (key, value) in [
        ("state", json!(true)),
        ("questions", json!({})),
        (
            "questions",
            json!({" ": {"type": "noul", "instructions": "?"}}),
        ),
        (
            "questions",
            json!({"q": {"type": "boolean", "instructions": "?"}}),
        ),
        (
            "questions",
            json!({"q": {"type": "choice", "instructions": "?", "criteria": {}}}),
        ),
        (
            "questions",
            json!({"q": {"type": "score", "instructions": "?", "criteria": []}}),
        ),
        (
            "questions",
            json!({"q": {"type": "noul", "instructions": "?", "criteria": {"true": "yes"}}}),
        ),
        ("model", json!(7)),
        ("model", json!("  ")),
        ("user", json!("x".repeat(257))),
        ("session_id", json!(false)),
        ("url", json!("https://example.test")),
        ("api_key", json!("not-an-argument")),
    ] {
        let mut args = arguments();
        args[key] = value;
        bad.push(args.to_string());
    }
    for arguments in bad {
        let mut state = decision_state();
        state.pending[0].arguments = arguments.clone();
        let call = state.pending[0].clone();
        assert!(state.run_head(&call), "{arguments}");
        assert!(matches!(state.status, Status::Idle));
        assert_eq!(tool_results(&state).len(), 1);
        assert!(!tool_results(&state)[0].content.is_empty());
    }
}

#[test]
fn decisions_waits_then_preserves_answers_and_resumes_the_queue() {
    let mut state = decision_state();
    let next = ToolCall {
        id: "next".into(),
        name: "tools_load".into(),
        arguments: r#"{"family":"decisions"}"#.into(),
    };
    state.messages.push(Message::tool_calls(vec![next.clone()]));
    state.pending.push_back(next);
    state.owes_reply = true;
    state.usage = Some(TokenUsage {
        input_tokens: 100,
        output_tokens: 20,
        cost_nanos: Some(2000),
    });
    let usage = state.usage;
    state.pump_tools();
    assert!(matches!(state.status, Status::AwaitingDecisions));
    assert!(state.is_loading());
    assert!(state.loader().unwrap().contains("openrouter_decisions"));
    assert_eq!(state.pending.len(), 2);
    assert!(tool_results(&state).is_empty());

    assert!(state.on_response(200, &response(), &context("decision-1")));
    let results = tool_results(&state);
    assert_eq!(results.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&results[0].content).unwrap(),
        serde_json::from_slice::<Value>(&response()).unwrap()
    );
    assert!(results[1].content.contains("decisions loaded"));
    assert!(state.pending.is_empty());
    assert!(
        state.is_receiving(),
        "the model receives a follow-up request"
    );
    assert_eq!(
        state.usage, usage,
        "Decisions usage stays in the tool result"
    );
}

#[test]
fn decisions_http_and_decoding_failures_finish_without_retry() {
    for (status, body, expected) in [
        (
            429,
            br#"{"error":{"message":"rate limited"}}"#.as_slice(),
            "rate limited",
        ),
        (502, b"bad gateway".as_slice(), "502"),
        (200, b"not json".as_slice(), "invalid decisions response"),
        (200, b"{}".as_slice(), "invalid decisions response"),
    ] {
        let mut state = decision_state();
        state.pump_tools();
        assert!(state.on_response(status, body, &context("decision-1")));
        assert!(tool_results(&state)[0].content.contains(expected));
        assert!(state.pending.is_empty());
        assert!(matches!(state.status, Status::Idle));
        assert!(state.retry.is_none());
    }
}

#[test]
fn decisions_ignores_wrong_ids_and_late_responses_after_interrupt() {
    let mut state = decision_state();
    state.pump_tools();
    for context in [
        context("other"),
        BTreeMap::new(),
        stream_context("hocus-old"),
    ] {
        assert!(!state.on_response(200, &response(), &context));
        assert_eq!(state.pending.len(), 1);
        assert!(tool_results(&state).is_empty());
    }
    assert!(state.on_key(KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier()));
    assert!(state.pending.is_empty());
    assert!(tool_results(&state)[0].content.contains("aborted"));
    let messages = state.messages.len();
    assert!(!state.on_response(200, &response(), &context("decision-1")));
    assert_eq!(state.messages.len(), messages);
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn decisions_response_follows_a_parked_chat() {
    let mut state = decision_state();
    state.pump_tools();
    state.chat_id = Some(4);
    state.new_chat();
    assert_eq!(state.parked.len(), 1);
    assert!(state.parked[0].is_live());
    assert!(state.handle(Event::WebRequestResult(
        200,
        BTreeMap::new(),
        response(),
        context("decision-1")
    )));
    assert!(state.messages.is_empty());
    assert!(state.pending.is_empty());
    let parked = &state.parked[0];
    assert!(parked.pending.is_empty());
    assert!(!parked.is_live());
    let result = parked
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert_eq!(result.tool_call_id.as_deref(), Some("decision-1"));
    assert_eq!(
        serde_json::from_str::<Value>(&result.content).unwrap()["id"],
        "decision-request"
    );
}

#[test]
fn decisions_is_deferred_discoverable_and_runs_in_planning_and_manual_modes() {
    let family = tools::family_of("openrouter_decisions").unwrap();
    assert_eq!(family.name, "decisions");
    assert!(family.deferred);
    assert!(tools::load_family(Some("decisions"), true).contains("openrouter_decisions"));
    let offered = |specs: Value| {
        specs
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["function"]["name"] == "openrouter_decisions")
    };
    assert!(!offered(tools::specs_for(&BTreeSet::new())));
    assert!(offered(tools::read_only_specs_for(&BTreeSet::from([
        "decisions"
    ]))));
    let parameters = decisions::parameters();
    assert_eq!(parameters["required"], json!(["state", "questions"]));
    assert!(parameters["properties"]["url"].is_null());
    assert!(parameters["properties"]["api_key"].is_null());
    let questions = &parameters["properties"]["questions"]["additionalProperties"]["anyOf"];
    // OpenAI requires array item schemas; nested input values remain unrestricted.
    for input in [
        &parameters["properties"]["state"],
        &questions[0]["properties"]["instructions"],
        &questions[1]["properties"]["instructions"],
        &questions[2]["properties"]["instructions"],
        &questions[0]["properties"]["criteria"]["additionalProperties"]["anyOf"][0],
        &questions[1]["properties"]["criteria"]["items"],
        &questions[2]["properties"]["criteria"]["properties"]["true"],
        &questions[2]["properties"]["criteria"]["properties"]["false"],
    ] {
        assert_eq!(input["items"], json!({}), "input: {input}");
    }

    let mut state = decision_state();
    state.plan = Plan::Planning;
    assert!(!state.call_needs_confirmation(&state.pending[0]));
    state.pump_tools();
    assert!(matches!(state.status, Status::AwaitingDecisions));

    let mut state = decision_state();
    state.pending.clear();
    state.messages.clear();
    assert!(state.run_manual(&format!("/openrouter_decisions {}", arguments())));
    assert!(matches!(state.status, Status::AwaitingDecisions));
    assert!(state.manual_head);
    let id = state.pending[0].id.clone();
    assert!(state.on_response(200, &response(), &context(&id)));
    assert!(tool_results(&state)[0].display.is_some());
    assert!(
        !state.is_receiving(),
        "manual calls do not ask the chat model"
    );
}
