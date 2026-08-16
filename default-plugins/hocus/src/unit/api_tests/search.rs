use super::*;

// -- search -------------------------------------------------------------

#[test]
fn search_query_url_normalises_every_shape_of_base() {
    for base in [
        "http://localhost:8888",
        "http://localhost:8888/",
        "http://localhost:8888/search",
        "  http://localhost:8888/search/  ",
    ] {
        assert_eq!(
            search_query_url(base, "zellij"),
            "http://localhost:8888/search?q=zellij&format=json",
            "{base}"
        );
    }
}

#[test]
fn search_query_url_escapes_everything_outside_the_unreserved_set() {
    assert_eq!(
        search_query_url("http://h:8888", "rust & wasm?"),
        "http://h:8888/search?q=rust%20%26%20wasm%3F&format=json"
    );
}

#[test]
fn search_query_url_escapes_multibyte_characters_per_utf8_byte() {
    assert_eq!(
        search_query_url("http://h:8888", "café"),
        "http://h:8888/search?q=caf%C3%A9&format=json"
    );
}

#[test]
fn search_results_renders_title_url_and_snippet() {
    let body = br#"{"results":[
        {"title":"Zellij","url":"https://zellij.dev","content":"A terminal workspace.","score":9},
        {"title":"Docs","url":"https://zellij.dev/docs","content":"","engine":"google"}
    ]}"#;
    assert_eq!(
        search_results(body, 8),
        "Zellij\nhttps://zellij.dev\nA terminal workspace.\n\nDocs\nhttps://zellij.dev/docs"
    );
}

#[test]
fn search_results_stops_at_the_limit() {
    let body = br#"{"results":[
        {"title":"a","url":"http://a","content":""},
        {"title":"b","url":"http://b","content":""},
        {"title":"c","url":"http://c","content":""}
    ]}"#;
    assert_eq!(search_results(body, 2), "a\nhttp://a\n\nb\nhttp://b");
}

/// An instant answer is the whole point of asking a search engine a
/// question, so it goes above the links rather than after them.
#[test]
fn search_results_put_instant_answers_first() {
    let body = br#"{"answers":[{"answer":"42","url":"http://x"}],
                    "results":[{"title":"a","url":"http://a","content":""}]}"#;
    assert_eq!(search_results(body, 8), "42\n\na\nhttp://a");
}

#[test]
fn search_results_accept_a_bare_string_answer() {
    let body = br#"{"answers":["42"],"results":[]}"#;
    assert_eq!(search_results(body, 8), "42");
}

#[test]
fn search_results_report_an_empty_page_rather_than_nothing() {
    assert_eq!(search_results(br#"{"results":[]}"#, 8), "no results");
}

/// The 403-serving-HTML case: `format=json` is off in the instance's config.
#[test]
fn search_results_name_the_likely_misconfiguration_on_a_non_json_body() {
    let message = search_results(b"<html>Forbidden</html>", 8);
    assert!(message.contains("search.formats"), "{message}");
}
