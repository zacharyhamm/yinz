use super::*;

// -- completions_url ----------------------------------------------------

#[test]
fn completions_url_appends_to_a_v1_base() {
    assert_eq!(
        completions_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/chat/completions"
    );
}

#[test]
fn completions_url_ignores_a_trailing_slash() {
    assert_eq!(
        completions_url("https://api.openai.com/v1/"),
        "https://api.openai.com/v1/chat/completions"
    );
}

#[test]
fn completions_url_leaves_a_full_endpoint_alone() {
    assert_eq!(
        completions_url("https://host/v1/chat/completions"),
        "https://host/v1/chat/completions"
    );
}

#[test]
fn completions_url_adds_v1_to_a_bare_host() {
    assert_eq!(
        completions_url("http://localhost:11434"),
        "http://localhost:11434/v1/chat/completions"
    );
}

// -- models -------------------------------------------------------------

/// Every shape `completions_url` accepts has to resolve here too — it is the
/// same `base_url` setting, so a base that chats must also list.
#[test]
fn models_url_normalises_every_shape_of_base() {
    for base in [
        "https://api.openai.com/v1",
        "https://api.openai.com/v1/",
        "https://api.openai.com/v1/chat/completions",
        "  https://api.openai.com/v1/chat/completions/  ",
    ] {
        assert_eq!(
            models_url(base),
            "https://api.openai.com/v1/models",
            "{base}"
        );
    }
}

#[test]
fn models_url_adds_v1_to_a_bare_host() {
    assert_eq!(
        models_url("http://localhost:11434"),
        "http://localhost:11434/v1/models"
    );
}

#[test]
fn model_ids_are_sorted_and_deduplicated() {
    let body = br#"{"object":"list","data":[
        {"id":"gpt-4o","object":"model","created":1},
        {"id":"o3-mini","owned_by":"openai"},
        {"id":"gpt-4o"}
    ]}"#;
    assert_eq!(model_ids(body), ["gpt-4o", "o3-mini"]);
}

/// A nameless entry is not something the picker can switch to, and an id is
/// the one field every provider agrees on.
#[test]
fn model_ids_drops_entries_with_no_usable_id() {
    let body = br#"{"data":[{"id":"  "},{"object":"model"},{"id":" gpt-4o "}]}"#;
    assert_eq!(model_ids(body), ["gpt-4o"]);
}

/// An endpoint that answers /v1/models with HTML, or with a shape nobody
/// predicted, must read as "nothing to show" rather than panic.
#[test]
fn model_ids_of_an_unparseable_body_is_empty() {
    assert!(model_ids(b"<html>404</html>").is_empty());
    assert!(model_ids(b"{}").is_empty());
}
