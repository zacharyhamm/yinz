use super::GridSnapshot;

fn snapshot(text: &str) -> GridSnapshot {
    GridSnapshot {
        text: text.to_owned(),
        cursor: None,
        styles: vec![],
    }
}

#[test]
fn tab_bar_detection_accepts_current_titles_ids_and_layout_positions() {
    for text in [
        " Yinz (test-0)  [0] Tab #1  + \n$ ",
        " Yinz (test-0)  [17] survivor  + \n$ ",
        " Yinz (test-0)  renamed tab  + \n$ ",
        "$ \n Yinz (test-0)  [0] Tab #1  + ",
    ] {
        assert!(snapshot(text).tab_bar_appears(), "{text}");
    }
}

#[test]
fn tab_bar_detection_does_not_confuse_pane_titles_or_terminal_text_with_chrome() {
    for text in [
        " [1] Pane #2\n$ ",
        " Tab #1\n$ ",
        " Yinz terminal output\n unrelated ribbon ",
        " output about Yinz (test-0)  a tab ",
    ] {
        assert!(!snapshot(text).tab_bar_appears(), "{text}");
    }
}
