use super::*;
#[test]
fn etag_lists_include_quoted_commas_and_weak_tags() {
    for value in ["*", "W/\"a,b\"", "\"other\", W/\"a,b\""] {
        assert!(matches(value, "\"a,b\""));
    }
    for value in [
        "\"a\",\"b\"",
        "\"a,b\",junk",
        "w/\"a,b\"",
        "\"a,b\" ,",
        "\"a,b",
    ] {
        assert!(!matches(value, "\"a,b\""));
    }
}
