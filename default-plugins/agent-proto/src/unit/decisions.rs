use super::*;
use serde_json::json;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn configuration_precedence_blanks_and_headers() {
    let env = map(&[
        ("OPENROUTER_API_KEY", " key "),
        (
            "OPENROUTER_DECISIONS_URL",
            " http://localhost:8080/decide?version=1 ",
        ),
        ("OPENROUTER_DECISIONS_MODEL", " env-model "),
    ]);
    let endpoint = Endpoint::resolve(&map(&[]), &env).unwrap().unwrap();
    assert_eq!(endpoint.url, "http://localhost:8080/decide?version=1");
    assert_eq!(endpoint.model, "env-model");
    assert_eq!(
        endpoint.headers(),
        map(&[
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("Authorization", "Bearer key"),
        ])
    );
    let kdl = map(&[
        ("decisions_url", " https://example.com/custom/ "),
        ("decisions_model", " kdl-model "),
    ]);
    let endpoint = Endpoint::resolve(&kdl, &env).unwrap().unwrap();
    assert_eq!(endpoint.url, "https://example.com/custom/");
    assert_eq!(endpoint.model, "kdl-model");
    let blank = map(&[
        ("decisions_url", " \t"),
        ("decisions_model", ""),
        ("decisions_api_key_env", " "),
    ]);
    let endpoint = Endpoint::resolve(&blank, &env).unwrap().unwrap();
    assert_eq!(endpoint.model, "env-model");
    assert_eq!(endpoint.url, "http://localhost:8080/decide?version=1");
    let env = map(&[
        ("OPENROUTER_API_KEY", "key"),
        ("OPENROUTER_DECISIONS_URL", " "),
        ("OPENROUTER_DECISIONS_MODEL", ""),
    ]);
    let endpoint = Endpoint::resolve(&blank, &env).unwrap().unwrap();
    assert_eq!(endpoint.url, DEFAULT_URL);
    assert_eq!(endpoint.model, DEFAULT_MODEL);
}

#[test]
fn credentials_and_chat_settings_are_independent() {
    let chat = map(&[
        ("base_url", "https://chat.example"),
        ("model", "chat"),
        ("summary_model", "summary"),
        ("api_key_env", "CHAT_KEY"),
        ("summaries", "false"),
        ("plugin_completions", "false"),
    ]);
    let mut env = map(&[
        ("OPENAI_API_KEY", "openai"),
        ("CHAT_KEY", "chat"),
        ("OPENAI_BASE_URL", "https://chat.example"),
        ("OPENAI_MODEL", "chat"),
    ]);
    assert!(Endpoint::resolve(&chat, &env).unwrap().is_none());
    env.insert(DEFAULT_API_KEY_ENV.into(), " ".into());
    assert!(Endpoint::resolve(&chat, &env).unwrap().is_none());
    env.insert(DEFAULT_API_KEY_ENV.into(), "router".into());
    let endpoint = Endpoint::resolve(&chat, &env).unwrap().unwrap();
    assert_eq!(endpoint.url, DEFAULT_URL);
    assert_eq!(endpoint.model, DEFAULT_MODEL);
    assert_eq!(endpoint.api_key, "router");
    let custom = map(&[("decisions_api_key_env", "CUSTOM")]);
    assert!(Endpoint::resolve(&custom, &env).unwrap().is_none());
    env.insert("CUSTOM".into(), "custom".into());
    assert_eq!(
        Endpoint::resolve(&custom, &env).unwrap().unwrap().api_key,
        "custom"
    );
    assert!(Endpoint::resolve(&map(&[]), &map(&[])).unwrap().is_none());
}

#[test]
fn endpoint_requires_http_or_https_without_chat_rewriting() {
    for invalid in [
        "relative/path",
        "http:example.org",
        "ftp://host/path",
        "file:///tmp/key",
        "https://",
        "://bad",
    ] {
        assert!(Endpoint::resolve(&map(&[("decisions_url", invalid)]), &map(&[])).is_err());
    }
    for url in [
        "http://localhost:8080",
        "https://example.org/v1/responses",
        "https://example.org/decisions/?x=1",
    ] {
        let endpoint = Endpoint::resolve(
            &map(&[("decisions_url", url)]),
            &map(&[(DEFAULT_API_KEY_ENV, "key")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(endpoint.url, url);
    }
}

fn mixed_request() -> Value {
    json!({
        "state": {"ticket": "Checkout is broken", "context": [true, 3, null]},
        "questions": {
            "team": {"type": "choice", "instructions": "Pick a team",
                "criteria": {"frontend": {"owns": "UI"}, "payments": null}},
            "urgency": {"type": "score", "instructions": ["Assess urgency"],
                "criteria": ["Can wait", {"action": "Fix today"}, ["Fix now"]]},
            "is_bug": {"type": "noul", "instructions": {"ask": "Is it a bug?"},
                "criteria": {"true": "Broken behavior", "false": ["Feature request"]}}
        },
        "user": "user-1", "session_id": "session-1"
    })
}

fn mixed_response() -> Value {
    json!({
        "id": "decision-1", "model": "typesafe/jev-1.13-20260917", "provider": "TypeSafe",
        "answers": {
            "team": {"type": "choice", "choice": "payments", "confidence": 0.9,
                "probabilities": {"frontend": 0.1, "payments": 0.9}},
            "urgency": {"type": "score", "score": 0.75, "confidence": 0.8,
                "probabilities": {"0": 0.1, "0.5": 0.3, "1": 0.6},
                "legend": {"0": "Can wait", "0.5": {"action": "Fix today"}, "1": ["Fix now"]}},
            "is_bug": {"type": "noul", "noul": 0.97}
        },
        "usage": {"input_tokens": 476, "output_tokens": 70, "cost": 0.000019992}
    })
}

#[test]
fn mixed_questions_and_answers_preserve_wire_fields() {
    let mut expected = mixed_request();
    let request: Request = serde_json::from_value(expected.clone()).unwrap();
    expected["model"] = json!(DEFAULT_MODEL);
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body(DEFAULT_MODEL).unwrap()).unwrap(),
        expected
    );
    let expected = mixed_response();
    let response = parse_response(200, &serde_json::to_vec(&expected).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), expected);
}

#[test]
fn structured_inputs_and_optional_fields() {
    for state in [json!("text"), json!({}), json!([null, 1, false])] {
        let request: Request = serde_json::from_value(json!({"state": state,
            "questions": {"q": {"type": "noul", "instructions": "Evaluate"}}}))
        .unwrap();
        let body: Value = serde_json::from_slice(&request.body(DEFAULT_MODEL).unwrap()).unwrap();
        assert_eq!(body["state"], state);
        assert!(body.get("user").is_none());
        assert!(body.get("session_id").is_none());
        assert!(body["questions"]["q"].get("criteria").is_none());
    }
    let response = json!({"model": "m", "answers": {
        "c": {"type": "choice", "choice": "x"},
        "s": {"type": "score", "score": 0.5}, "n": {"type": "noul", "noul": 0.2}
    }, "usage": {"input_tokens": 1, "output_tokens": 2}});
    assert_eq!(
        serde_json::to_value(parse_response(201, &serde_json::to_vec(&response).unwrap()).unwrap())
            .unwrap(),
        response
    );
}

#[test]
fn invalid_request_inputs_fail() {
    let mut request: Request = serde_json::from_value(mixed_request()).unwrap();
    assert!(request.body(" ").is_err());
    request.user = Some("é".repeat(256));
    assert!(request.body(DEFAULT_MODEL).is_ok());
    request.user = Some("é".repeat(257));
    assert!(request.body(DEFAULT_MODEL).is_err());
    request.user = None;
    request.session_id = Some("a".repeat(257));
    assert!(request.body(DEFAULT_MODEL).is_err());
    request.session_id = None;
    request.questions.clear();
    assert!(request.body(DEFAULT_MODEL).is_err());
    for q in [
        Question::Score {
            instructions: Input::String("score".into()),
            criteria: vec![],
        },
        Question::Choice {
            instructions: Input::String("choose".into()),
            criteria: BTreeMap::new(),
        },
    ] {
        request.questions.insert("q".into(), q);
        assert!(request.body(DEFAULT_MODEL).is_err());
    }
    request.questions = BTreeMap::from([(
        " ".into(),
        Question::Noul {
            instructions: Input::String("ask".into()),
            criteria: None,
        },
    )]);
    assert!(request.body(DEFAULT_MODEL).is_err());
    for invalid in [Value::Null, json!(true), json!(1)] {
        assert!(serde_json::from_value::<Input>(invalid).is_err());
    }
    for q in [
        json!({"type": "noul", "instructions": true}),
        json!({"type": "noul", "instructions": "ask", "criteria": {"true": "yes"}}),
        json!({"type": "score", "instructions": "ask", "criteria": [null]}),
        json!({"type": "choice", "instructions": "ask", "criteria": {"x": 1}}),
        json!({"type": "unknown", "instructions": "ask"}),
    ] {
        assert!(serde_json::from_value::<Question>(q).is_err());
    }
}

#[test]
fn malformed_successes_and_http_errors_fail_explicitly() {
    for body in [b"not JSON".as_slice(), b"{}", b"null", b"", b"\xff"] {
        assert!(parse_response(200, body)
            .unwrap_err()
            .starts_with("invalid decisions response:"));
    }
    for field in ["model", "answers", "usage"] {
        let mut response = mixed_response();
        response.as_object_mut().unwrap().remove(field);
        assert!(parse_response(200, &serde_json::to_vec(&response).unwrap()).is_err());
    }
    for (name, field) in [
        ("team", "choice"),
        ("urgency", "score"),
        ("is_bug", "noul"),
        ("team", "type"),
    ] {
        let mut response = mixed_response();
        response["answers"][name]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(parse_response(200, &serde_json::to_vec(&response).unwrap()).is_err());
    }
    for field in ["input_tokens", "output_tokens"] {
        let mut response = mixed_response();
        response["usage"].as_object_mut().unwrap().remove(field);
        assert!(parse_response(200, &serde_json::to_vec(&response).unwrap()).is_err());
    }
    assert_eq!(
        parse_response(429, br#"{"error":{"message":"slow down"}}"#).unwrap_err(),
        "HTTP 429: slow down"
    );
    assert_eq!(
        parse_response(502, b"bad gateway").unwrap_err(),
        "HTTP 502: bad gateway"
    );
    assert_eq!(parse_response(500, b"").unwrap_err(), "HTTP 500");
}
