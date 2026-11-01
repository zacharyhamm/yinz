use yinz_types::pick::PickOptions;

#[test]
fn host_rejects_invalid_browse_options() {
    for json in [
        r#"{"kind":"file","roots":[]}"#,
        r#"{"kind":"file","roots":["/a","/b"]}"#,
        r#"{"kind":"directory","extensions":[]}"#,
        r#"{"kind":"file","extensions":[".pdf"]}"#,
        r#"{"kind":"file","extensions":["p*f"]}"#,
        r#"{"kind":"file","roots":["a\u0000b"]}"#,
    ] {
        let options: PickOptions = serde_json::from_str(json).unwrap();
        assert!(options.validate().is_err(), "{json}");
    }
    let error =
        serde_json::from_str::<PickOptions>(r#"{"kind":"file","recursive":false}"#).unwrap_err();
    assert!(error.to_string().contains("recursive"));
}
