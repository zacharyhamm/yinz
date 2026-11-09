use super::*;
#[test]
fn strict_paths_and_queries() {
    for path in [
        "", "abc", "/a//b", "/a/../b", "/%2e", "/%2E%2e", "/%2f", "/%5C", "/%00", "/%ff", "/%",
        "/%g0", "/a\\b",
    ] {
        assert!(normalize(path, None).is_err(), "{path}");
    }
    assert!(normalize(&format!("/{}", "a".repeat(2048)), None).is_err());
    assert!(normalize(&format!("/{}", vec!["a"; 33].join("/")), None).is_err());
    assert!(normalize("/", Some(&vec!["a=b"; 33].join("&"))).is_err());
    for query in ["a=%ff", "a=%", "a=%00"] {
        assert!(normalize("/", Some(query)).is_err());
    }
    let got = normalize("/caf%C3%A9/%252f/", Some("q=hello+world&q=%252f")).unwrap();
    assert_eq!(got.path, "/café/%2f/");
    assert_eq!(
        got.query,
        [
            ("q".into(), "hello world".into()),
            ("q".into(), "%2f".into())
        ]
    );
}
