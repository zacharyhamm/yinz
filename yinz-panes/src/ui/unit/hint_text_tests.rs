use super::*;

#[test]
fn disabled_resize_hint_variants_omit_mouse_scroll_at_every_tier() {
    for is_floating in [false, true] {
        for hint in resize_hint_variants(is_floating, false).into_values() {
            assert!(!hint.text.contains("MouseScroll"));
            assert!(hint.text.contains("drag borders"));
        }
    }
}

#[test]
fn enabled_resize_hint_variants_include_mouse_scroll_at_every_tier() {
    for is_floating in [false, true] {
        for hint in resize_hint_variants(is_floating, true).into_values() {
            assert!(hint.text.contains("MouseScroll"));
            assert!(hint.text.contains("drag borders"));
        }
    }
}
