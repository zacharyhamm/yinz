use super::*;
use yinz_types::pick::PickKind;

#[test]
fn request_fields_overlay_alias_arrays_without_changing_settings() {
    let alias = r#"{"kind":"file","roots":["/alias"],"extensions":["txt"],"hidden":true,"recursive":false}"#.to_owned();
    let request = PickOptions {
        kind: PickKind::File,
        roots: Some(vec!["/request".into()]),
        extensions: Some(vec!["pdf".into()]),
        hidden: None,
        recursive: None,
        timeout_ms: None,
    };
    let merged = merge_options(request, Some(&alias)).unwrap();
    assert_eq!(merged.roots, Some(vec!["/request".into()]));
    assert_eq!(merged.extensions, Some(vec!["pdf".into()]));
    assert_eq!(merged.hidden, Some(true));
    assert_eq!(merged.recursive, Some(false));
}

#[test]
fn host_rejects_invalid_listing_options() {
    for json in [
        r#"{"kind":"file","roots":[]}"#,
        r#"{"kind":"directory","extensions":[]}"#,
        r#"{"kind":"file","extensions":[".pdf"]}"#,
        r#"{"kind":"file","extensions":["p*f"]}"#,
        r#"{"kind":"file","roots":["a\u0000b"]}"#,
    ] {
        let options: PickOptions = serde_json::from_str(json).unwrap();
        assert!(options.validate().is_err(), "{json}");
    }
}
