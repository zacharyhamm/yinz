use super::*;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn test_config() -> Config {
    Config {
        url: "https://api.openai.com/v1/chat/completions".into(),
        api_key: "sk-test".into(),
        model: "gpt-4o-mini".into(),
        base_url: DEFAULT_BASE_URL.into(),
        api_key_env: DEFAULT_API_KEY_ENV.into(),
        search_url: DEFAULT_SEARCH_URL.into(),
        dobbs: DEFAULT_DOBBS.into(),
        dobbs_addr: None,
        dobbs_token: None,
        system_prompt: None,
        // Off in most fixtures so the assertions can compare whole bodies.
        tools: false,
        max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
        // On, as it resolves by default: the fixtures that carry no reasoning
        // are then also the proof that the field stays off the wire anyway.
        reasoning_echo: true,
    }
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

// -- request_body -------------------------------------------------------

fn body_value(cfg: &Config, transcript: &[Message]) -> serde_json::Value {
    serde_json::from_slice(&request_body(cfg, transcript, true)).unwrap()
}

// -- SseParser ----------------------------------------------------------

fn deltas(items: Vec<SseItem>) -> Vec<String> {
    items
        .into_iter()
        .filter_map(|item| match item {
            SseItem::Delta(text) => Some(text),
            _ => None,
        })
        .collect()
}

const FRAME: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";

#[test]
fn parses_a_single_complete_frame() {
    let mut parser = SseParser::default();
    assert_eq!(deltas(parser.feed(FRAME)), vec!["Hello"]);
}

// -- ToolCallAccumulator ------------------------------------------------

/// Drive a whole stream through both stages, the way `on_chunk` does.
fn accumulate(chunks: &[&[u8]]) -> Vec<ToolCall> {
    let mut parser = SseParser::default();
    let mut accumulator = ToolCallAccumulator::default();
    for chunk in chunks {
        for item in parser.feed(chunk) {
            accumulator.feed(&item);
        }
    }
    accumulator.take()
}

mod config;
mod errors;
mod request_body;
mod search;
mod streaming;
mod tool_calls;
mod urls;
