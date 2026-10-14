use super::*;
#[test]
fn quoted_filters_roundtrip_and_match_only_available_fields() {
    let q = AgentQuery::parse("fix café state:waiting directory:\"/a b\"", LIVE_FIELDS).unwrap();
    assert_eq!(AgentQuery::parse(&q.tokens(), LIVE_FIELDS).unwrap(), q);
    assert!(q.matches(&BTreeMap::from([
        ("state".into(), "waiting".into()),
        ("directory".into(), "/a b/c".into())
    ])));
    assert!(!q.matches(&BTreeMap::new()));
    for input in ["state:done", "state:idle state:busy", "title:\"oops"] {
        assert!(AgentQuery::parse(input, LIVE_FIELDS).is_err());
    }
}

#[test]
fn literal_free_text_roundtrips_alongside_filters() {
    for input in [
        r#"say \"hello world\" provider:codex"#,
        r#"unmatched \"quote provider:codex"#,
        r#"C:\\work\\file directory:"C:\\work tree" provider:codex"#,
    ] {
        let query = AgentQuery::parse(input, LIVE_FIELDS).unwrap();
        assert_eq!(
            AgentQuery::parse(&query.tokens(), LIVE_FIELDS).unwrap(),
            query
        );
    }
}

#[test]
fn quoted_or_escaped_filter_syntax_is_literal() {
    for fields in [LIVE_FIELDS, ARCHIVE_FIELDS] {
        for (input, text) in [
            (r#""hello provider:codex""#, "hello provider:codex"),
            (r#""provider:codex""#, "provider:codex"),
            (r#""provider":codex"#, "provider:codex"),
            (r#"provider\:codex"#, "provider:codex"),
            (r#"\provider:codex"#, "provider:codex"),
            (r#""provider:""#, "provider:"),
        ] {
            let query = AgentQuery::parse(&format!("{input} provider:\"claude\""), fields).unwrap();
            assert_eq!(query.text, text);
            assert_eq!(
                query.filters,
                BTreeMap::from([("provider".into(), "claude".into())])
            );
            assert_eq!(AgentQuery::parse(&query.tokens(), fields).unwrap(), query);
        }
        for text in [
            "",
            "provider:codex",
            "hello provider:codex",
            "session:Main provider:",
            "  café\t日本語\n  ",
            "say \"hello\" C:\\work\\",
        ] {
            let query = AgentQuery {
                text: text.into(),
                filters: BTreeMap::from([("provider".into(), "claude".into())]),
            };
            assert_eq!(AgentQuery::parse(&query.tokens(), fields).unwrap(), query);
        }
    }
}
