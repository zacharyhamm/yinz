use super::*;
#[test]
fn safe_markdown_handles_all_active_paths() {
    let rendered = render_safe("<script>x</script>\n\n[bad](JaVaScRiPt:bad) [bad](jav&#97;script:bad) [good](https://example.org)\n\n![*alt* <x>](https://example.org/image)\n\n```html\n<b>\n```");
    assert!(!rendered.contains("<script>"));
    assert!(!rendered.contains("<img"));
    assert!(!rendered.contains("href=\"JaVa"));
    assert!(!rendered.contains("href=\"java"));
    assert!(rendered.contains("rel=\"noopener noreferrer\""));
    assert!(rendered.contains("&lt;b&gt;"));
    assert!(rendered.contains("alt"));
}
#[test]
fn links_refuse_browser_normalization_tricks() {
    for url in [
        "//evil",
        "\\evil",
        "javascript:x",
        "data:x",
        "vbscript:x",
        "java\nscript:x",
        " https://example.org",
        "jav&colon;x",
    ] {
        assert!(!safe_link(url), "{url}");
    }
    for url in [
        "/safe",
        "#part",
        "../relative",
        "https://example.org",
        "http://example.org",
        "mailto:x@example.org",
        "a/b?x=1&y=2",
    ] {
        assert!(safe_link(url), "{url}");
    }
}
