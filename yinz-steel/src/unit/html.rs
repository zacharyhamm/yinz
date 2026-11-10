use crate::Runtime;
use serde_json::Value;
fn runtime() -> Runtime {
    Runtime::new(|_| Ok(Value::Null)).unwrap()
}
#[test]
fn opaque_html_escapes_and_builds_documents() {
    let mut r = runtime();
    r.eval(r#"(define out (yinz-html-document (yinz-h "title" (hash) "<&") (yinz-h "p" (hash "title" "\"'") (list "<b>" 12 #f json-null))))"#).unwrap();
    r.eval("(define rendered (yinz-html out))").unwrap();
    r.eval("(define big (yinz-html 18446744073709551615))")
        .unwrap();
    assert_eq!(r.value("big").unwrap(), "18446744073709551615");
    assert_eq!(r.value("rendered").unwrap(), "<!doctype html><html lang=\"en\"><head><title>&lt;&amp;</title></head><body><p title=\"&quot;&#39;\">&lt;b&gt;12</p></body></html>");
    r.eval(r#"(define boot (yinz-html (yinz-html-json-script "config" (hash "text" "</script>&\u{2028}\u{2029}"))))"#).unwrap();
    let boot = r.value("boot").unwrap();
    let boot = boot.as_str().unwrap();
    assert!(boot.contains("\\u003c/script\\u003e\\u0026"), "{boot}");
    assert!(!boot.contains("</script>&"));
}
#[test]
fn html_rejects_active_markup_and_non_native_children() {
    let mut r = runtime();
    for expression in [
        r#"(yinz-h "p" (hash "onclick" "bad"))"#,
        r#"(yinz-h "p" (hash "style" "color:red"))"#,
        r#"(yinz-h "iframe" (hash "srcdoc" "bad"))"#,
        r#"(yinz-h "a" (hash "href" "JaVaScRiPt:alert(1)"))"#,
        r#"(yinz-h "a" (hash "href" "jav&#97;script:bad"))"#,
        r#"(yinz-h "a" (hash "href" #t))"#,
        r#"(yinz-h "img" (hash "src" "//evil"))"#,
        r#"(yinz-h "input" (hash) "child")"#,
        r#"(yinz-h "script" (hash "src" "/x.js") "bad")"#,
        r#"(yinz-h "script" (hash "type" "application/json" "src" "/x.js"))"#,
        r#"(yinz-h "meta" (hash "http-equiv" "refresh"))"#,
        r#"(yinz-h "link" (hash "href" "/x" "rel" "import"))"#,
        r#"(yinz-h "form" (hash))"#,
        r#"(yinz-html-form (hash "action" "//evil"))"#,
        r#"(yinz-html-form (hash "action" "/login" "formaction" "/other"))"#,
        r#"(yinz-h "p" (hash) (hash "raw" "<script>"))"#,
        r#"(yinz-h "p" (hash) #t)"#,
    ] {
        assert!(r.eval(expression).is_err(), "accepted {expression}");
    }
    r.eval(r#"(define safe (yinz-html (yinz-html-form (hash "action" "/api/login") (yinz-h "input" (hash "type" "password" "required" #t)))))"#).unwrap();
    assert!(r
        .value("safe")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("method=\"post\""));
}
#[test]
fn html_bounds_raw_markdown_fragments_and_native_graphs() {
    let mut r = runtime();
    r.eval(r#"(define (nest n value) (if (= n 0) value (nest (- n 1) (list value)))) (define (nodes n value) (if (= n 0) value (nodes (- n 1) (yinz-h "div" (hash) value))))"#).unwrap();
    assert!(r.eval("(yinz-html (nest 66 \"x\"))").is_err());
    assert!(r.eval("(nodes 66 \"x\")").is_err());
    assert!(r
        .eval("(yinz-html-raw (make-string 4194305 #\\x))")
        .is_err());
    assert!(r.eval("(yinz-html (make-string 1048577 #\\<))").is_err());
    assert!(r
        .eval("(yinz-markdown->html (make-string 1048577 #\\x))")
        .is_err());
    r.eval("(define raw-limit (yinz-html (yinz-html-raw (make-string 4194304 #\\x))))")
        .unwrap();
    assert_eq!(
        r.value("raw-limit").unwrap().as_str().unwrap().len(),
        4194304
    );
    r.eval(r#"(define md (yinz-html (yinz-markdown->html "<script>alert(1)</script>\n\n[bad](javascript:bad) ![alt](https://example.org/x)")))"#).unwrap();
    let md = r.value("md").unwrap();
    let md = md.as_str().unwrap();
    assert!(!md.contains("<script>"));
    assert!(!md.contains("href="));
    assert!(!md.contains("<img"));
}

#[test]
fn bootstrap_unicode_and_closing_script_text_are_inert_and_budgeted() {
    let mut r = runtime();
    let expression = "(define bootstrap (yinz-html (yinz-html-json-script \"config\" \"</script>\u{2028}\u{2029}\")))";
    r.eval(expression).unwrap();
    let rendered = r.value("bootstrap").unwrap();
    let rendered = rendered.as_str().unwrap();
    assert!(
        rendered.contains("\\u003c/script\\u003e\\u2028\\u2029"),
        "{rendered}"
    );
    assert!(r
        .eval("(yinz-html-json-script \"config\" (make-string 4194305 #\\x))")
        .is_err());
    assert!(r
        .eval("(yinz-html-json-script \"config\" (make-string 1000000 #\\<))")
        .is_err());
}
