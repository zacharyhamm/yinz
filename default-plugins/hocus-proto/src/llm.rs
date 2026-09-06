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
/// https://host/v1/responses         -> https://host/v1/chat/completions
/// http://localhost:11434            -> http://localhost:11434/v1/chat/completions
/// ```
///
/// A base already pointing at the *other* endpoint is re-pointed for the same reason
/// [`responses_url`] re-points one written this way: `base_url` names the API root for
/// everything hocus does, so switching transports must not depend on which of the two the
/// user happened to spell.
pub fn completions_url(base: &str) -> String {
    format!("{}/chat/completions", api_root(base))
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

/// Whether `base_url` names OpenAI's own API, as opposed to a compatible proxy, a
/// local model server, or another provider. Only OpenAI's endpoint understands
/// `reasoning_effort` and `/v1/responses`, so this gates their defaults in hocus.
pub fn is_openai(base_url: &str) -> bool {
    api_root(base_url)
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
}

/// Whether `model` names one of OpenAI's reasoning models, which are the only ones that
/// accept a `reasoning` object at all - every other model 400s on the field.
///
/// A prefix list rather than a lookup: nothing on the wire answers the question, and the
/// families are named consistently enough that a prefix is as good as it gets. An
/// unrecognised name simply gets no default effort, which is the safe direction to be
/// wrong in: a reasoning model without one thinks at OpenAI's own default anyway, while a
/// non-reasoning model with one cannot make the request at all.
pub fn is_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    ["gpt-5", "o1", "o3", "o4"]
        .iter()
        .any(|family| model.starts_with(family))
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

/// How long to wait before resending a request the endpoint turned away, or `None` when the
/// status is not one worth resending.
///
/// The transient set is deliberately narrow. A 429 is the one this exists for — a per-minute
/// token budget refills on its own, and the request that hit it is still exactly the right
/// request to send. 500/502/503/504/529 are the same shape of "nothing about the body was
/// wrong"; every other status describes something a resend would only repeat.
///
/// The plugin host hands the caller `(status, body)` and no headers, so `Retry-After` is out of
/// reach. OpenAI writes the wait into the message instead — "Please try again in 3.565s", or
/// "in 1m2s" for a longer one — and that hint is the endpoint's own arithmetic on its own
/// budget, so it beats anything guessed here. `attempt` (0 for the first resend) supplies the
/// `2^attempt` fallback when there is no hint, and the result is clamped either way: a hint of
/// zero would spin, and one of ten minutes would look like a hang.
pub fn retry_delay(status: u16, body: &[u8], attempt: u32) -> Option<f64> {
    if !matches!(status, 429 | 500 | 502 | 503 | 504 | 529) {
        return None;
    }
    let hinted = retry_hint(&api_error_message(status, body));
    let seconds = hinted.unwrap_or_else(|| f64::from(1u32 << attempt.min(6)));
    Some(seconds.clamp(MIN_RETRY_SECONDS, MAX_RETRY_SECONDS))
}

/// The same question for an error that arrived *inside* a stream, where there is a message and
/// no status to go with it.
///
/// Stricter than the status form on purpose. A status of 429 is the endpoint saying, in the one
/// place it cannot be mistaken, that nothing about the request was wrong; a mid-stream error is
/// free text, and most of what arrives that way - a refusal, a context-length complaint, a
/// malformed tool call - is a thing a resend would only repeat. So this asks the message to
/// name a rate limit before it will wait on one.
pub fn streamed_retry_delay(message: &str, attempt: u32) -> Option<f64> {
    let lowered = message.to_ascii_lowercase();
    let rate_limited = [
        "rate limit",
        "tokens per min",
        "requests per min",
        "too many requests",
    ]
    .iter()
    .any(|marker| lowered.contains(marker));
    if !rate_limited {
        return None;
    }
    let seconds = retry_hint(message).unwrap_or_else(|| f64::from(1u32 << attempt.min(6)));
    Some(seconds.clamp(MIN_RETRY_SECONDS, MAX_RETRY_SECONDS))
}

/// Never resend faster than this. A hint of `0s` is a busy loop, and the endpoint is no worse
/// off for half a second of patience.
const MIN_RETRY_SECONDS: f64 = 0.5;
/// Never wait longer than this. Past a minute a retry is indistinguishable from a hang, and
/// the person watching would rather be told and decide for themselves.
const MAX_RETRY_SECONDS: f64 = 60.0;

/// The wait an error message names, in seconds.
///
/// Read from the first `try again in ` in the message, so a number appearing anywhere else in
/// the prose - the limit, the tokens used, the tokens requested - is never mistaken for a
/// delay. What follows is a Go-style duration: one or more `<number><unit>` pairs, units `ms`,
/// `s`, `m` and `h`. OpenAI writes all of `20ms`, `3.565s` and `1m2s`, so all of them are read,
/// and scanning stops at the first byte that cannot continue one.
fn retry_hint(message: &str) -> Option<f64> {
    const MARKER: &str = "try again in ";
    let mut rest = message.split_once(MARKER).map(|(_, rest)| rest)?;
    let mut total = 0.0;
    let mut read_any = false;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let Ok(value) = rest[..digits].parse::<f64>() else {
            break;
        };
        rest = &rest[digits..];
        // Longest unit first: `ms` must not be read as `m` with a stray `s` behind it.
        let (unit, scale) = match rest {
            _ if rest.starts_with("ms") => ("ms", 0.001),
            _ if rest.starts_with('s') => ("s", 1.0),
            _ if rest.starts_with('m') => ("m", 60.0),
            _ if rest.starts_with('h') => ("h", 3600.0),
            _ => break,
        };
        total += value * scale;
        read_any = true;
        rest = &rest[unit.len()..];
    }
    read_any.then_some(total)
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
    /// The model to ask, chosen for the lane that resolved this: `summary_model` falling
    /// back to `model` for a summary, `model` alone for a completion.
    pub model: String,
}

/// Which of the two configured models an endpoint is resolved for.
///
/// A type rather than a bool because the two lanes are asymmetric: the summary lane falls
/// back from `summary_model` to `model`, and the completion lane must not fall the other
/// way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Model {
    Summary,
    Chat,
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
        Self::resolve_for(kdl, env, Model::Summary)
    }

    /// The same endpoint, for a caller that is not asking for a summary.
    ///
    /// Not gated on `summaries`: that key means *do not describe my agents*, which is a
    /// statement about one feature rather than about whether the session has a model at
    /// all. [`plugin_completions_enabled`] is the key that governs this lane, and the
    /// caller checks it — the two `None`s here stay the ones about configuration.
    ///
    /// Resolves `model` and never `summary_model`. A plugin asking a real question must
    /// not silently land on whatever cheap model the user picked for one-line pane
    /// summaries, which is exactly what [`Endpoint::resolve`]'s precedence would do.
    pub fn resolve_for_completion(
        kdl: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
    ) -> Option<Self> {
        Self::resolve_for(kdl, env, Model::Chat)
    }

    /// The shared body of both resolvers: everything but which model key wins.
    fn resolve_for(
        kdl: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
        model: Model,
    ) -> Option<Self> {
        let base_url = non_empty(kdl, "base_url")
            .or_else(|| non_empty(env, "OPENAI_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        // The summary model falls back to the chat model rather than to the built-in
        // default, so configuring one model configures both.
        let model = match model {
            Model::Summary => non_empty(kdl, "summary_model").or_else(|| non_empty(kdl, "model")),
            Model::Chat => non_empty(kdl, "model"),
        }
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
/// see `send_run_summary` in `hocus-agentd`.
pub fn summaries_enabled(kdl: &BTreeMap<String, String>) -> bool {
    !non_empty(kdl, "summaries").is_some_and(|value| value.eq_ignore_ascii_case("false"))
}

/// Whether the KDL block lets other plugins spend the user's API key.
///
/// Same stance as [`summaries_enabled`] and hocus's `tools` key: anything but an explicit
/// `false` leaves it on, so a typo does not silently disable the feature.
///
/// What keeps that default honest is not the flag but the shape of what is on offer. A
/// caller names messages, tools and at most a model — never a URL, a key, a header or a
/// provider — so the worst a hostile plugin can do is spend tokens at the user's own
/// endpoint rather than carry the key anywhere. The rest of the guard is structural: the
/// caps in `completion::validate` and the daemon's concurrency limit.
pub fn plugin_completions_enabled(kdl: &BTreeMap<String, String>) -> bool {
    !non_empty(kdl, "plugin_completions").is_some_and(|value| value.eq_ignore_ascii_case("false"))
}

/// A model the user pinned for plugin completions, which overrides any a caller names.
///
/// For the session that wants the feature on but not at a caller's choice of price. Unset
/// is the ordinary case, and then the caller's model wins over the configured one — a
/// plugin that knows it needs a reasoning model should be able to say so.
pub fn pinned_completion_model(kdl: &BTreeMap<String, String>) -> Option<String> {
    non_empty(kdl, "plugin_completion_model")
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
    /// hocus's default effort is gated on this rather than on the host alone.
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
            Endpoint::resolve_for_completion(&map(&[]), &map(&[("OPENAI_API_KEY", "  ")]))
                .is_none()
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
}
