//! Where the OpenAI-compatible endpoint is, and how its answers are shaped.
//!
//! Split out of hocus's `api.rs` when `hocus-agentd` grew a reason to call the same
//! endpoint: the daemon summarises an agent's session when it goes idle, and doing that
//! from a second copy of the URL-shaping rules would mean two places to get
//! `http://localhost:11434` right.
//!
//! Only the *endpoint* lives here — where to send, with what credentials, as which model.
//! The conversation half (the transcript mapping, the tool specs, the SSE framing) stays in
//! hocus, which is the only thing that has a conversation.
//!
//! Nothing here calls the Zellij host, so all of it runs on the host triple.

use std::collections::BTreeMap;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// Look up a key, treating whitespace-only values as absent. An exported-but-empty
/// `OPENAI_API_KEY` should produce hocus's config banner, not an `Authorization: Bearer `
/// header that fails confusingly at the API.
pub fn non_empty(map: &BTreeMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Accept whatever shape of base URL the user has lying around.
///
/// ```text
/// https://api.openai.com/v1         -> https://api.openai.com/v1/chat/completions
/// https://api.openai.com/v1/        -> https://api.openai.com/v1/chat/completions
/// https://host/v1/chat/completions  -> unchanged
/// http://localhost:11434            -> http://localhost:11434/v1/chat/completions
/// ```
pub fn completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// The Responses endpoint, for the same `base_url` and the same four shapes.
///
/// ```text
/// https://api.openai.com/v1         -> https://api.openai.com/v1/responses
/// https://api.openai.com/v1/        -> https://api.openai.com/v1/responses
/// https://host/v1/chat/completions  -> https://host/v1/responses
/// http://localhost:11434            -> http://localhost:11434/v1/responses
/// ```
///
/// A base already pointing at one of the two endpoints is re-pointed rather than left
/// alone: `base_url` names the API root for everything hocus does, and switching
/// transports must not depend on the user having spelled it one particular way.
pub fn responses_url(base: &str) -> String {
    format!("{}/responses", api_root(base))
}

/// The model list that sits beside the completions endpoint. Same four shapes of base URL
/// as [`completions_url`], since it is the same `base_url` setting.
///
/// ```text
/// https://api.openai.com/v1         -> https://api.openai.com/v1/models
/// https://api.openai.com/v1/        -> https://api.openai.com/v1/models
/// https://host/v1/chat/completions  -> https://host/v1/models
/// https://host/v1/responses         -> https://host/v1/models
/// http://localhost:11434            -> http://localhost:11434/v1/models
/// ```
pub fn models_url(base: &str) -> String {
    format!("{}/models", api_root(base))
}

/// The `/v1` root a configured `base_url` names, whichever endpoint it was written as.
fn api_root(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let base = base
        .strip_suffix("/chat/completions")
        .or_else(|| base.strip_suffix("/responses"))
        .unwrap_or(base);
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

/// Best-effort human-readable message from a non-2xx response body.
pub fn api_error_message(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().chars().take(500).collect());

    if detail.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    }
}

/// A resolved place to send one non-streaming request.
///
/// This is what a caller needs who has no conversation and no UI to explain itself to —
/// `hocus-agentd`. hocus keeps its own richer `api::Config` for the chat turn; both resolve
/// from the same keys through [`Endpoint::resolve`], so they cannot point at different
/// endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Fully resolved completions endpoint, derived from the configured base URL.
    pub url: String,
    pub api_key: String,
    /// The model summaries are asked of: `summary_model` when set, otherwise `model`.
    pub model: String,
}

impl Endpoint {
    /// Resolve from a plugin KDL block and the session environment, with the same
    /// precedence hocus uses: KDL wins over env, env wins over the built-in default.
    ///
    /// `None` when there is no API key, or when summaries are switched off. Both are
    /// ordinary states for a background caller with nowhere to put a banner: it simply
    /// does nothing, which is what an unconfigured session should cost.
    ///
    /// There is deliberately no `api_key` KDL key, matching hocus: that would put a secret
    /// in a config file for no gain over pointing `api_key_env` at a different variable.
    pub fn resolve(kdl: &BTreeMap<String, String>, env: &BTreeMap<String, String>) -> Option<Self> {
        if !summaries_enabled(kdl) {
            return None;
        }
        let base_url = non_empty(kdl, "base_url")
            .or_else(|| non_empty(env, "OPENAI_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        // The summary model falls back to the chat model rather than to the built-in
        // default, so configuring one model configures both.
        let model = non_empty(kdl, "summary_model")
            .or_else(|| non_empty(kdl, "model"))
            .or_else(|| non_empty(env, "OPENAI_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let api_key_env =
            non_empty(kdl, "api_key_env").unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_owned());
        let api_key = non_empty(env, &api_key_env)?;
        Some(Endpoint {
            url: completions_url(&base_url),
            api_key,
            model,
        })
    }

    /// Headers for a buffered, non-streaming completion. `Accept: application/json` rather
    /// than hocus's `text/event-stream`: there is no stream to frame here, and an endpoint
    /// told to expect SSE may well send it.
    pub fn headers(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
            (
                "Authorization".to_owned(),
                format!("Bearer {}", self.api_key),
            ),
        ])
    }
}

/// Whether the KDL block leaves idle summaries on.
///
/// Same stance as hocus's `tools` key: anything but an explicit `false` leaves them on, so
/// a typo does not silently disable the feature.
///
/// Public because [`Endpoint::resolve`] answers `None` for two different reasons and one
/// caller has to tell them apart. "No API key" means *I have no model*, and a pane whose
/// agent reports for itself can still be summarised from its own words. `summaries
/// "false"` means *do not do this*, and that has to switch off the local fallback too —
/// see `on_turn_end_with` in `hocus-agentd`.
pub fn summaries_enabled(kdl: &BTreeMap<String, String>) -> bool {
    !non_empty(kdl, "summaries").is_some_and(|value| value.eq_ignore_ascii_case("false"))
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
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
            (
                "http://localhost:11434",
                "http://localhost:11434/v1/chat/completions",
            ),
        ] {
            assert_eq!(completions_url(base), want, "{base}");
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
}
