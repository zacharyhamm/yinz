use super::*;

// -- Config::resolve ----------------------------------------------------

#[test]
fn resolve_falls_back_to_defaults() {
    let cfg = Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "sk-1")])).unwrap();
    assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    assert_eq!(cfg.model, DEFAULT_MODEL);
    assert_eq!(cfg.api_key, "sk-1");
    assert_eq!(cfg.system_prompt, None);
}

#[test]
fn resolve_prefers_env_over_defaults() {
    let cfg = Config::resolve(
        &map(&[]),
        &map(&[
            ("OPENAI_API_KEY", "sk-1"),
            ("OPENAI_BASE_URL", "http://localhost:11434"),
            ("OPENAI_MODEL", "llama3"),
        ]),
    )
    .unwrap();
    assert_eq!(cfg.base_url, "http://localhost:11434");
    assert_eq!(cfg.model, "llama3");
    assert_eq!(cfg.url, "http://localhost:11434/v1/chat/completions");
}

#[test]
fn resolve_prefers_kdl_over_env() {
    let cfg = Config::resolve(
        &map(&[("base_url", "https://kdl/v1"), ("model", "from-kdl")]),
        &map(&[
            ("OPENAI_API_KEY", "sk-1"),
            ("OPENAI_BASE_URL", "https://env/v1"),
            ("OPENAI_MODEL", "from-env"),
        ]),
    )
    .unwrap();
    assert_eq!(cfg.base_url, "https://kdl/v1");
    assert_eq!(cfg.model, "from-kdl");
}

#[test]
fn resolve_redirects_the_key_lookup_via_api_key_env() {
    let cfg = Config::resolve(
        &map(&[("api_key_env", "MY_OTHER_KEY")]),
        &map(&[("OPENAI_API_KEY", "wrong"), ("MY_OTHER_KEY", "right")]),
    )
    .unwrap();
    assert_eq!(cfg.api_key, "right");
    assert_eq!(cfg.api_key_env, "MY_OTHER_KEY");
}

#[test]
fn resolve_errors_when_the_key_is_missing_and_names_the_variable() {
    let err = Config::resolve(&map(&[]), &map(&[])).unwrap_err();
    assert!(
        err.message.contains("OPENAI_API_KEY"),
        "message should name the variable: {}",
        err.message
    );
    // The banner still needs these.
    assert_eq!(err.base_url, DEFAULT_BASE_URL);
    assert_eq!(err.model, DEFAULT_MODEL);
}

#[test]
fn resolve_treats_an_empty_key_as_missing() {
    assert!(Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "   ")])).is_err());
}

#[test]
fn resolve_trims_the_key() {
    let cfg = Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "  sk-1\n")])).unwrap();
    assert_eq!(cfg.api_key, "sk-1");
}

#[test]
fn resolve_reads_the_system_prompt_from_kdl_only() {
    let cfg = Config::resolve(
        &map(&[("system_prompt", "be terse")]),
        &map(&[("OPENAI_API_KEY", "sk-1")]),
    )
    .unwrap();
    assert_eq!(cfg.system_prompt.as_deref(), Some("be terse"));
}

#[test]
fn resolve_reads_and_normalises_reasoning_controls() {
    let cfg = Config::resolve(
        &map(&[("thinking", "DISABLED"), ("reasoning_effort", "XHIGH")]),
        &map(&[("OPENAI_API_KEY", "sk-1")]),
    )
    .unwrap();
    assert_eq!(cfg.thinking.as_deref(), Some("disabled"));
    assert_eq!(cfg.reasoning_effort.as_deref(), Some("xhigh"));
}

#[test]
fn resolve_rejects_invalid_reasoning_controls() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    for (key, value) in [("thinking", "sometimes"), ("reasoning_effort", "huge")] {
        let error = Config::resolve(&map(&[(key, value)]), &env).unwrap_err();
        assert!(error.message.contains(key), "{}", error.message);
        assert!(
            error.message.contains("must be one of"),
            "{}",
            error.message
        );
    }
}

#[test]
fn resolve_leaves_tools_on_unless_kdl_explicitly_says_false() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert!(Config::resolve(&map(&[]), &env).unwrap().tools);
    assert!(
        Config::resolve(&map(&[("tools", "true")]), &env)
            .unwrap()
            .tools
    );
    assert!(
        !Config::resolve(&map(&[("tools", "false")]), &env)
            .unwrap()
            .tools
    );
    assert!(
        !Config::resolve(&map(&[("tools", "FALSE")]), &env)
            .unwrap()
            .tools
    );
    // A typo must not silently disable them.
    assert!(
        Config::resolve(&map(&[("tools", "flase")]), &env)
            .unwrap()
            .tools
    );
}

#[test]
fn resolve_leaves_reasoning_echo_on_unless_kdl_explicitly_says_false() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert!(Config::resolve(&map(&[]), &env).unwrap().reasoning_echo);
    assert!(
        !Config::resolve(&map(&[("reasoning_echo", "false")]), &env)
            .unwrap()
            .reasoning_echo
    );
    assert!(
        !Config::resolve(&map(&[("reasoning_echo", "FALSE")]), &env)
            .unwrap()
            .reasoning_echo
    );
    // A typo here would break the tool loop against a thinking model rather
    // than merely degrade it, so it must leave the default alone.
    assert!(
        Config::resolve(&map(&[("reasoning_echo", "flase")]), &env)
            .unwrap()
            .reasoning_echo
    );
}

#[test]
fn resolve_leaves_stream_usage_on_unless_kdl_explicitly_says_false() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert!(Config::resolve(&map(&[]), &env).unwrap().stream_usage);
    assert!(
        !Config::resolve(&map(&[("stream_usage", "false")]), &env)
            .unwrap()
            .stream_usage
    );
    assert!(
        !Config::resolve(&map(&[("stream_usage", "FALSE")]), &env)
            .unwrap()
            .stream_usage
    );
    // A typo costs an exact count, never a 400 on every request.
    assert!(
        Config::resolve(&map(&[("stream_usage", "flase")]), &env)
            .unwrap()
            .stream_usage
    );
}

#[test]
fn resolve_reads_the_tool_budget_from_kdl_only() {
    let env = map(&[("OPENAI_API_KEY", "sk-1")]);
    assert_eq!(
        Config::resolve(&map(&[("max_tool_rounds", "8")]), &env)
            .unwrap()
            .max_tool_rounds,
        8
    );
    // Absent, blank, unparseable, or zero: all fall back rather than leave
    // the tool loop uncapped or capped at nothing.
    for kdl in [
        map(&[]),
        map(&[("max_tool_rounds", "  ")]),
        map(&[("max_tool_rounds", "lots")]),
        map(&[("max_tool_rounds", "-1")]),
        map(&[("max_tool_rounds", "0")]),
    ] {
        assert_eq!(
            Config::resolve(&kdl, &env).unwrap().max_tool_rounds,
            DEFAULT_MAX_TOOL_ROUNDS,
            "{kdl:?}"
        );
    }
}

#[test]
fn resolve_prefers_kdl_search_url_over_env_over_the_default() {
    let key = ("OPENAI_API_KEY", "sk-1");
    assert_eq!(
        Config::resolve(&map(&[]), &map(&[key])).unwrap().search_url,
        DEFAULT_SEARCH_URL
    );
    assert_eq!(
        Config::resolve(&map(&[]), &map(&[key, ("SEARXNG_URL", "http://env:8888")]))
            .unwrap()
            .search_url,
        "http://env:8888"
    );
    assert_eq!(
        Config::resolve(
            &map(&[("search_url", "http://kdl:8888")]),
            &map(&[key, ("SEARXNG_URL", "http://env:8888")]),
        )
        .unwrap()
        .search_url,
        "http://kdl:8888"
    );
}

#[test]
fn resolve_prefers_kdl_search_proxy_over_env_and_defaults_to_unset() {
    let key = ("OPENAI_API_KEY", "sk-1");
    assert_eq!(
        Config::resolve(&map(&[]), &map(&[key]))
            .unwrap()
            .search_proxy,
        None
    );
    assert_eq!(
        Config::resolve(
            &map(&[]),
            &map(&[key, ("SEARXNG_PROXY", "socks5://env:1080")]),
        )
        .unwrap()
        .search_proxy,
        Some("socks5://env:1080".to_owned())
    );
    assert_eq!(
        Config::resolve(
            &map(&[("search_proxy", "socks5://kdl:1080")]),
            &map(&[key, ("SEARXNG_PROXY", "socks5://env:1080")]),
        )
        .unwrap()
        .search_proxy,
        Some("socks5://kdl:1080".to_owned())
    );
}

#[test]
fn resolve_prefers_kdl_dobbs_path_over_env_over_the_default() {
    let key = ("OPENAI_API_KEY", "sk-1");
    assert_eq!(
        Config::resolve(&map(&[]), &map(&[key])).unwrap().dobbs,
        DEFAULT_DOBBS
    );
    assert_eq!(
        Config::resolve(&map(&[]), &map(&[key, ("DOBBS_BIN", "/env/dobbs")]))
            .unwrap()
            .dobbs,
        "/env/dobbs"
    );
    assert_eq!(
        Config::resolve(
            &map(&[("dobbs_path", "/kdl/dobbs")]),
            &map(&[key, ("DOBBS_BIN", "/env/dobbs")]),
        )
        .unwrap()
        .dobbs,
        "/kdl/dobbs"
    );
}

/// A local daemon needs no environment at all; a remote one is only reachable
/// with both halves, so neither may be dropped on the way to the CLI.
/// Nothing downstream expands a tilde — Yinz passes argv straight to exec
/// — so a `~/…` binary that got through would fail naming nothing.
#[test]
fn resolve_expands_a_tilde_in_the_dobbs_path() {
    let env = map(&[("OPENAI_API_KEY", "sk-1"), ("HOME", "/Users/zach")]);
    assert_eq!(
        Config::resolve(&map(&[("dobbs_path", "~/dobbs/dobbs")]), &env)
            .unwrap()
            .dobbs,
        "/Users/zach/dobbs/dobbs"
    );
    // Only a leading "~/", and only with a HOME to expand against.
    assert_eq!(
        Config::resolve(&map(&[("dobbs_path", "/opt/~/dobbs")]), &env)
            .unwrap()
            .dobbs,
        "/opt/~/dobbs"
    );
    assert_eq!(
        Config::resolve(
            &map(&[("dobbs_path", "~/dobbs/dobbs")]),
            &map(&[("OPENAI_API_KEY", "sk-1")]),
        )
        .unwrap()
        .dobbs,
        "~/dobbs/dobbs"
    );
}

#[test]
fn dobbs_env_carries_a_remote_daemon_and_nothing_otherwise() {
    let key = ("OPENAI_API_KEY", "sk-1");
    let cfg = Config::resolve(&map(&[]), &map(&[key])).unwrap();
    assert!(cfg.dobbs_env().is_empty());

    let cfg = Config::resolve(
        &map(&[("dobbs_addr", "10.0.0.5:7355"), ("dobbs_token", "foobar")]),
        &map(&[key]),
    )
    .unwrap();
    assert_eq!(
        cfg.dobbs_env(),
        BTreeMap::from([
            ("DOBBS_ADDR".to_owned(), "10.0.0.5:7355".to_owned()),
            ("DOBBS_TOKEN".to_owned(), "foobar".to_owned()),
        ])
    );

    // The session environment is the fallback, as for every other setting.
    let cfg = Config::resolve(
        &map(&[("dobbs_addr", "kdl:7355")]),
        &map(&[key, ("DOBBS_ADDR", "env:7355"), ("DOBBS_TOKEN", "t")]),
    )
    .unwrap();
    assert_eq!(cfg.dobbs_addr.as_deref(), Some("kdl:7355"));
    assert_eq!(cfg.dobbs_token.as_deref(), Some("t"));
}

#[test]
fn context_bytes_default_and_override_are_positive() {
    let env = map(&[("OPENAI_API_KEY", "test")]);
    for (setting, expected) in [
        ("", CONTEXT_MAX_BYTES),
        ("0", CONTEXT_MAX_BYTES),
        ("bad", CONTEXT_MAX_BYTES),
        ("65536", 65536),
    ] {
        assert_eq!(
            Config::resolve(&map(&[("context_max_bytes", setting)]), &env)
                .unwrap()
                .context_max_bytes,
            expected
        );
    }
}
