use super::*;

fn file(body: &str) -> StaticFile {
    StaticFile {
        bytes: Arc::from(body.as_bytes()),
        content_type: "text/plain; charset=utf-8",
        etag: format!("\"{body}\""),
    }
}

#[test]
fn web_site_lookup_prefers_pages_then_the_longest_asset_prefix() {
    let mut site = Site::default();
    site.pages.insert("/".into(), file("index"));
    site.assets
        .entry("/static/".into())
        .or_default()
        .insert("app.js".into(), file("app"));
    site.assets
        .entry("/static/vendor/".into())
        .or_default()
        .insert("lib.js".into(), file("lib"));

    assert_eq!(site.lookup("/").map(|f| f.etag.as_str()), Some("\"index\""));
    assert_eq!(
        site.lookup("/static/app.js").map(|f| f.etag.as_str()),
        Some("\"app\"")
    );
    assert_eq!(
        site.lookup("/static/vendor/lib.js")
            .map(|f| f.etag.as_str()),
        Some("\"lib\"")
    );
    assert!(site.lookup("/static/missing.js").is_none());
    assert!(site.lookup("/login").is_none());
}

#[test]
fn web_site_handle_is_shared_between_clones() {
    let handle = SiteHandle::default();
    let reader = handle.clone();
    assert!(reader.current().is_none());
    handle.set(Some(Arc::new(Site {
        generation: 3,
        ..Default::default()
    })));
    assert_eq!(reader.current().map(|site| site.generation), Some(3));
    handle.set(None);
    assert!(reader.current().is_none());
}

#[test]
fn web_request_debug_omits_body_and_query() {
    let request = WebRequest {
        id: 7,
        method: WebMethod::Post,
        path: "/api/agent/prompt".into(),
        segments: vec!["api".into(), "agent".into(), "prompt".into()],
        query: vec![("secret".into(), "query".into())],
        headers: vec![],
        body: Some("private prompt".into()),
        auth: WebAuth {
            session: "abc".into(),
            label: "phone".into(),
        },
        deadline_ms: 0,
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains("private prompt"));
    assert!(!debug.contains("secret"));
    assert!(debug.contains("body_len"));
    let json = serde_json::to_value(&request).expect("serialize");
    assert_eq!(json["method"], "POST");
    assert_eq!(json["query"][0][1], "query");
}
