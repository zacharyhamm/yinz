use super::*;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// The body OpenAI turns a request away with when the per-minute token budget is spent.
fn rate_limited(hint: &str) -> Vec<u8> {
    format!(
        r#"{{"error":{{"message":"Rate limit reached for gpt-5.6-luna in organization \
org-PY8t1WwcVzrAcPUs8vCKH02p on tokens per min (TPM): Limit 200000, Used 164171, Requested \
47713. Please try again in {hint}. Visit https://platform.openai.com/account/rate-limits to \
learn more."}}}}"#
    )
    .into_bytes()
}

#[test]
fn streamed_errors_retry_only_rate_limits_and_h2_protocol_errors() {
    for message in [
            "h2 protocol error",
            "Together: H2 PROTOCOL ERROR",
            "Upstream error from Together: Stream error: h2 protocol error: error reading a body from connection",
        ] {
            for (attempt, seconds) in [1.0, 2.0, 4.0, 8.0].into_iter().enumerate() {
                assert_eq!(streamed_retry_delay(message, attempt as u32), Some(seconds));
            }
        }
    for message in [
        "Rate limit reached",
        "tokens per min",
        "requests per min",
        "Too Many Requests",
    ] {
        assert_eq!(streamed_retry_delay(message, 0), Some(1.0));
    }
    assert_eq!(
        streamed_retry_delay("Rate limit reached. Please try again in 3.565s", 0),
        Some(3.565)
    );
    for message in [
        "connection reset",
        "cancelled",
        "invalid tool schema",
        "context length exceeded",
        "protocol error",
    ] {
        assert_eq!(streamed_retry_delay(message, 0), None);
    }
}

#[test]
fn retry_hint_reads_every_duration_openai_writes() {
    for (hint, want) in [
        ("3.565s", 3.565),
        ("1m2s", 62.0),
        ("2m", 120.0),
        ("20ms", 0.02),
        ("1h", 3600.0),
        ("1h1m1s", 3661.0),
    ] {
        let message = String::from_utf8(rate_limited(hint)).expect("utf8");
        assert_eq!(retry_hint(&message), Some(want), "hint {hint}");
    }
}

/// Nothing that is not a duration is read as one, and a message with no marker at all
/// leaves the caller on its own backoff rather than on a number invented here.
#[test]
fn retry_hint_declines_what_is_not_a_duration() {
    for message in [
        "HTTP 429: Rate limit reached. Limit 200000, Used 164171.",
        "HTTP 429: please try again in a little while",
        "HTTP 429: try again in 5 minutes",
    ] {
        assert_eq!(retry_hint(message), None, "{message}");
    }
}

#[test]
fn retry_delay_reads_the_wait_openai_names() {
    assert_eq!(retry_delay(429, &rate_limited("3.565s"), 0), Some(3.565));
    assert_eq!(
        retry_delay(429, &rate_limited("20ms"), 0),
        Some(0.5),
        "held at the floor rather than spun on"
    );
}

/// Only the delay is read. Every other number in that message - the limit, the tokens used,
/// the tokens requested, the URL - sits *before* the marker and must not be mistaken for
/// one, which is what a bare "find a number" would do with `200000`.
#[test]
fn retry_delay_ignores_the_other_numbers_in_the_message() {
    assert_eq!(retry_delay(429, &rate_limited("4s"), 0), Some(4.0));
}

/// No hint, so the fallback: `2^attempt`, still clamped.
#[test]
fn retry_delay_backs_off_when_the_body_names_no_wait() {
    let body = br#"{"error":{"message":"Rate limit reached"}}"#;
    let waits: Vec<_> = (0..4).map(|n| retry_delay(429, body, n)).collect();
    assert_eq!(
        waits,
        vec![Some(1.0), Some(2.0), Some(4.0), Some(8.0)],
        "the fallback doubles"
    );
}

/// The status set is the whole of the policy: a resend has to be able to succeed where the
/// first attempt did not, which a 400 or a 401 never will.
#[test]
fn retry_delay_declines_every_status_a_resend_would_only_repeat() {
    let body = br#"{"error":{"message":"nope"}}"#;
    for status in [400, 401, 403, 404, 413, 422, 200] {
        assert_eq!(retry_delay(status, body, 0), None, "status {status}");
    }
    for status in [429, 500, 502, 503, 504, 529] {
        assert!(retry_delay(status, body, 0).is_some(), "status {status}");
    }
}

/// A hint of zero would spin, and one of ten minutes would be indistinguishable from a
/// hang, so both ends are held.
#[test]
fn retry_delay_clamps_both_ends() {
    assert_eq!(retry_delay(429, &rate_limited("0s"), 0), Some(0.5));
    assert_eq!(retry_delay(429, &rate_limited("10m"), 0), Some(60.0));
    assert_eq!(
        retry_delay(500, br"", 30),
        Some(60.0),
        "the shift is bounded"
    );
}

#[test]
fn completions_url_accepts_every_shape_of_base_url() {
    for (base, want) in [
        (
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/chat/completions",
        ),
        (
            "https://api.openai.com/v1/",
            "https://api.openai.com/v1/chat/completions",
        ),
        (
            "https://host/v1/chat/completions",
            "https://host/v1/chat/completions",
        ),
        // A base written as the *other* endpoint still names the same root, or
        // selecting `api "chat"` against it would reach /v1/responses/v1/chat/completions.
        (
            "https://host/v1/responses",
            "https://host/v1/chat/completions",
        ),
        (
            "http://localhost:11434",
            "http://localhost:11434/v1/chat/completions",
        ),
    ] {
        assert_eq!(completions_url(base), want, "{base}");
    }
}

/// Only these accept a `reasoning` object; everything else 400s on it, which is why
/// agent's default effort is gated on this rather than on the host alone.
#[test]
fn only_the_reasoning_families_are_recognised() {
    for model in [
        "gpt-5",
        "gpt-5.1-mini",
        "o1",
        "o3-mini",
        "o4-mini",
        "  O3  ",
    ] {
        assert!(is_reasoning_model(model), "{model}");
    }
    for model in ["gpt-4o-mini", "gpt-4.1", "deepseek-chat", "llama3", ""] {
        assert!(!is_reasoning_model(model), "{model}");
    }
}

#[test]
fn models_url_reaches_the_list_beside_the_same_endpoint() {
    for (base, want) in [
        (
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/models",
        ),
        (
            "https://api.openai.com/v1/",
            "https://api.openai.com/v1/models",
        ),
        ("https://host/v1/chat/completions", "https://host/v1/models"),
        ("http://localhost:11434", "http://localhost:11434/v1/models"),
    ] {
        assert_eq!(models_url(base), want, "{base}");
    }
}

#[test]
fn an_endpoint_needs_a_key_and_nothing_else() {
    let endpoint = Endpoint::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "sk-1")]))
        .expect("a key is all it takes");
    assert_eq!(endpoint.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(endpoint.model, DEFAULT_MODEL);
    assert_eq!(endpoint.api_key, "sk-1");

    assert!(Endpoint::resolve(&map(&[]), &map(&[])).is_none());
    // Exported but empty is the same as absent, or the Authorization header would go
    // out with nothing after "Bearer ".
    assert!(Endpoint::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "  ")])).is_none());
}

#[test]
fn kdl_wins_over_env_and_env_over_the_default() {
    let endpoint = Endpoint::resolve(
        &map(&[("base_url", "http://localhost:11434"), ("model", "llama3")]),
        &map(&[
            ("OPENAI_API_KEY", "sk-1"),
            ("OPENAI_BASE_URL", "https://ignored"),
            ("OPENAI_MODEL", "ignored"),
        ]),
    )
    .expect("resolved");
    assert_eq!(endpoint.url, "http://localhost:11434/v1/chat/completions");
    assert_eq!(endpoint.model, "llama3");

    let from_env = Endpoint::resolve(
        &map(&[]),
        &map(&[
            ("OPENAI_API_KEY", "sk-1"),
            ("OPENAI_BASE_URL", "http://localhost:8080"),
            ("OPENAI_MODEL", "mistral"),
        ]),
    )
    .expect("resolved");
    assert_eq!(from_env.url, "http://localhost:8080/v1/chat/completions");
    assert_eq!(from_env.model, "mistral");
}

/// Configuring one model configures both; naming `summary_model` sends the cheap work
/// somewhere else without touching the conversation.
#[test]
fn the_summary_model_falls_back_to_the_chat_model() {
    let shared = Endpoint::resolve(
        &map(&[("model", "gpt-5")]),
        &map(&[("OPENAI_API_KEY", "k")]),
    )
    .expect("resolved");
    assert_eq!(shared.model, "gpt-5");

    let split = Endpoint::resolve(
        &map(&[("model", "gpt-5"), ("summary_model", "gpt-4o-mini")]),
        &map(&[("OPENAI_API_KEY", "k")]),
    )
    .expect("resolved");
    assert_eq!(split.model, "gpt-4o-mini");
}

#[test]
fn a_different_api_key_variable_can_be_named() {
    let endpoint = Endpoint::resolve(
        &map(&[("api_key_env", "MY_KEY")]),
        &map(&[("MY_KEY", "sk-2"), ("OPENAI_API_KEY", "sk-1")]),
    )
    .expect("resolved");
    assert_eq!(endpoint.api_key, "sk-2");
}

/// Only an explicit `false` switches summaries off — a typo must not silently disable
/// them.
#[test]
fn summaries_are_on_unless_switched_off_in_so_many_words() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert!(Endpoint::resolve(&map(&[("summaries", "false")]), &env).is_none());
    assert!(Endpoint::resolve(&map(&[("summaries", "FALSE")]), &env).is_none());
    assert!(Endpoint::resolve(&map(&[("summaries", "true")]), &env).is_some());
    assert!(Endpoint::resolve(&map(&[("summaries", "flase")]), &env).is_some());
    assert!(Endpoint::resolve(&map(&[]), &env).is_some());
}

/// `summaries "false"` is a statement about describing agents, not about whether the
/// session has a model — a plugin asking its own question is unaffected.
#[test]
fn a_completion_endpoint_resolves_with_summaries_switched_off() {
    let kdl = map(&[("summaries", "false")]);
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert!(Endpoint::resolve(&kdl, &env).is_none());
    let endpoint = Endpoint::resolve_for_completion(&kdl, &env).expect("resolved");
    assert_eq!(endpoint.api_key, "sk-1");
    assert_eq!(endpoint.url, "https://api.openai.com/v1/chat/completions");
}

/// The whole reason the completion lane has a resolver of its own: a real question must
/// not land on whatever cheap model was picked for one-line pane summaries.
#[test]
fn a_completion_endpoint_prefers_the_chat_model_over_the_summary_model() {
    let kdl = map(&[("model", "gpt-5"), ("summary_model", "gpt-4o-mini")]);
    let env = map(&[("OPENAI_API_KEY", "k")]);
    assert_eq!(Endpoint::resolve(&kdl, &env).unwrap().model, "gpt-4o-mini");
    assert_eq!(
        Endpoint::resolve_for_completion(&kdl, &env).unwrap().model,
        "gpt-5"
    );
}

/// With no `model` at all it still falls through env and then the built-in default,
/// rather than reaching sideways for `summary_model`.
#[test]
fn a_completion_endpoint_falls_through_env_to_the_default_model() {
    let env = map(&[("OPENAI_API_KEY", "k")]);
    assert_eq!(
        Endpoint::resolve_for_completion(&map(&[("summary_model", "cheap")]), &env)
            .unwrap()
            .model,
        DEFAULT_MODEL
    );
    assert_eq!(
        Endpoint::resolve_for_completion(
            &map(&[("summary_model", "cheap")]),
            &map(&[("OPENAI_API_KEY", "k"), ("OPENAI_MODEL", "from-env")]),
        )
        .unwrap()
        .model,
        "from-env"
    );
}

#[test]
fn a_completion_endpoint_is_none_without_a_key() {
    assert!(Endpoint::resolve_for_completion(&map(&[]), &map(&[])).is_none());
    assert!(
        Endpoint::resolve_for_completion(&map(&[]), &map(&[("OPENAI_API_KEY", "  ")])).is_none()
    );
}

/// Only an explicit `false`, for the reason summaries take the same stance.
#[test]
fn plugin_completions_are_on_unless_switched_off_in_so_many_words() {
    assert!(!plugin_completions_enabled(&map(&[(
        "plugin_completions",
        "false"
    )])));
    assert!(!plugin_completions_enabled(&map(&[(
        "plugin_completions",
        "FALSE"
    )])));
    assert!(plugin_completions_enabled(&map(&[(
        "plugin_completions",
        "flase"
    )])));
    assert!(plugin_completions_enabled(&map(&[(
        "plugin_completions",
        "true"
    )])));
    assert!(plugin_completions_enabled(&map(&[])));
    // The two gates are independent: switching summaries off says nothing about this.
    assert!(plugin_completions_enabled(&map(&[("summaries", "false")])));
}

#[test]
fn a_pinned_completion_model_is_read_and_whitespace_is_not_one() {
    assert_eq!(
        pinned_completion_model(&map(&[("plugin_completion_model", "gpt-5")])).as_deref(),
        Some("gpt-5")
    );
    assert_eq!(
        pinned_completion_model(&map(&[("plugin_completion_model", "   ")])),
        None
    );
    assert_eq!(pinned_completion_model(&map(&[("model", "gpt-5")])), None);
}

#[test]
fn headers_carry_the_key_and_ask_for_json_rather_than_a_stream() {
    let endpoint = Endpoint::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "sk-1")])).unwrap();
    let headers = endpoint.headers();
    assert_eq!(headers["Authorization"], "Bearer sk-1");
    assert_eq!(headers["Accept"], "application/json");
    assert_eq!(headers["Content-Type"], "application/json");
}

#[test]
fn api_error_message_prefers_the_providers_own_words() {
    assert_eq!(
        api_error_message(429, br#"{"error":{"message":"slow down"}}"#),
        "HTTP 429: slow down",
    );
    assert_eq!(api_error_message(500, b"  "), "HTTP 500");
    assert_eq!(
        api_error_message(502, b"bad gateway"),
        "HTTP 502: bad gateway"
    );
}
