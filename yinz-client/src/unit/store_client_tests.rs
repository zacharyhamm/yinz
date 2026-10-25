use super::*;
#[test]
fn only_structured_success_is_success() {
    assert_eq!(
        decode_reply(r#"{"ok":true,"data":{"id":7}}"#).unwrap()["id"],
        7
    );
    assert_eq!(
        decode_reply(r#"{"ok":true,"data":"remembered p"}"#).unwrap(),
        "remembered p"
    );
    assert!(decode_reply(r#"{"ok":false,"error":"conflict"}"#)
        .unwrap_err()
        .to_string()
        .contains("conflict"));
    for bad in ["raw error", r#"{"id":7}"#, r#"{"ok":true}"#] {
        assert!(decode_reply(bad).is_err());
    }
}
