use super::*;
use unicode_width::UnicodeWidthStr;

fn header() -> HistoryHeader {
    HistoryHeader {
        name: "Agent".into(),
        status: "codex · idle".into(),
        location: "session · /work".into(),
        started: "2026-10-09 10:00 UTC".into(),
        details: "literal **metadata**\nsecond line".into(),
    }
}
fn events(count: u64) -> Vec<HistoryEvent> {
    (1..=count)
        .map(|revision| HistoryEvent {
            revision,
            kind: "Message".into(),
            time: "10:00".into(),
            markdown: format!("**Event {revision}**\n\n- first\n- second\n- third\n- fourth"),
            truncated: false,
        })
        .collect()
}
fn action(id: &str) -> Entry {
    Entry {
        id: id.into(),
        label: if id == "history-details" {
            "Details".into()
        } else {
            id.into()
        },
        description: String::new(),
        shortcut: String::new(),
        aliases: vec![],
        kind: EntryKind::Plugin,
    }
}
fn view(count: u64) -> AgentView {
    let mut view = AgentView::new(header());
    view.update(
        header(),
        events(count),
        vec![action("Focus"), action("history-details")],
        String::new(),
    );
    view
}
fn layout(view: &mut AgentView, cols: usize, rows: usize) -> AgentLayout {
    view.layout(
        CellRect {
            x: 0,
            y: 0,
            cols,
            rows,
        },
        0,
        &TextInput::default(),
        None,
        false,
    )
}
fn key(view: &mut AgentView, key: BareKey) -> Outcome {
    view.key(&KeyWithModifier::new(key), 42)
        .unwrap_or(Outcome::None)
}

#[test]
fn previews_wrap_limit_and_keep_styles_and_mouse_geometry() {
    let mut view = view(3);
    let shown = layout(&mut view, 32, 30);
    assert_eq!(view.selected, 2);
    assert!(shown.lines.iter().any(|l| l.text == "… more"));
    assert!(shown
        .lines
        .iter()
        .any(|l| l.text == "Event 3" && l.styles.iter().all(|s| s.bold)));
    assert!(!shown.lines.iter().any(|l| l.text.contains("fourth")));
    for (row, hit) in shown.hits.iter().enumerate() {
        if let Some(Hit::Event(i)) = hit {
            view.click(&shown, row);
            assert_eq!(view.selected, *i);
            assert_eq!(
                key(&mut view, BareKey::Enter),
                Outcome::Activate(format!("history:42:event:{}", i + 1))
            );
        }
    }
}

#[test]
fn markdown_shapes_unicode_and_literal_tool_details() {
    let mut view = view(0);
    view.reader("Message".into(), "# Title\n\n**bold** and ~~gone~~\n\n- item\n\n```rust\nlet x = 1;\n```\n\n| key | value |\n|---|---|\n| 日 | 世界 |\n\nabcdefghijklmnopqrstuvwxyz".into(), "{\"literal\": \"**not bold**\"}".into());
    layout(&mut view, 40, 100);
    let reader = view.reader.as_mut().unwrap();
    let lines = reader.document.lines(38);
    let text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("• item"));
    assert!(text.contains("let x = 1;"));
    assert!(text.contains("│"));
    assert!(text.contains("**not bold**"));
    assert!(lines.iter().flat_map(|l| &l.styles).any(|s| s.bold));
    assert!(lines.iter().flat_map(|l| &l.styles).any(|s| s.strike));
    for width in [0, 1, 2, 3, 7, 19] {
        for line in reader.document.lines(width) {
            if width >= 7 {
                assert!(line.text.width() <= width, "{width}: {line:?}");
            }
        }
    }
}

#[test]
fn scroll_anchors_survive_updates_resize_and_details() {
    let mut view = view(30);
    layout(&mut view, 35, 20);
    assert!(view.following);
    view.scroll(false, 9);
    let anchor = view.anchor;
    let selected = view.selected;
    view.update(
        header(),
        events(31),
        vec![action("Focus"), action("history-details")],
        String::new(),
    );
    layout(&mut view, 25, 20);
    assert_eq!(view.anchor, anchor);
    assert_eq!(view.selected, selected);
    key(&mut view, BareKey::Tab);
    key(&mut view, BareKey::Down);
    key(&mut view, BareKey::Enter);
    let details = layout(&mut view, 40, 20);
    assert!(details
        .lines
        .iter()
        .any(|l| l.text.contains("**metadata**")));
    assert!(view.back());
    layout(&mut view, 25, 20);
    assert_eq!(view.anchor, anchor);
    view.page_request(true);
    view.update(header(), events(32), vec![], String::new());
    layout(&mut view, 25, 20);
    assert!(view.following);
    assert_eq!(view.selected, 31);
    assert_eq!(view.offset, view.total - view.page);
}

#[test]
fn replacement_selection_is_visible_after_search_results_change() {
    for empty_first in [false, true] {
        let mut view = view(30);
        layout(&mut view, 35, 20);
        view.scroll(false, 9);
        layout(&mut view, 35, 20);
        assert!(view.offset > 0);
        assert!(!view.following);
        if empty_first {
            view.update(header(), vec![], vec![], String::new());
            layout(&mut view, 35, 20);
            assert_eq!(key(&mut view, BareKey::Enter), Outcome::None);
        }
        view.update(header(), events(29), vec![], String::new());
        let shown = layout(&mut view, 35, 20);
        assert_eq!(view.selected, 0);
        assert!(shown.hits.iter().any(|h| matches!(h, Some(Hit::Event(0)))));
        assert_eq!(
            key(&mut view, BareKey::Enter),
            Outcome::Activate("history:42:event:1".into())
        );
    }
}

#[test]
fn search_hit_reveals_event_and_older_pages_do_not_follow() {
    let mut view = view(30);
    view.select_revision(10);
    let shown = layout(&mut view, 32, 20);
    assert!(!view.following);
    assert!(shown.hits.iter().any(|h| matches!(h, Some(Hit::Event(9)))));
    let before = view.offset;
    key(&mut view, BareKey::PageDown);
    assert_eq!(view.offset, before + view.page);
    view.newest = false;
    key(&mut view, BareKey::End);
    assert!(!view.following);
}

#[test]
fn empty_timeline_focus_boundaries_and_small_layouts() {
    let mut view = view(0);
    assert_eq!(key(&mut view, BareKey::Enter), Outcome::None);
    key(&mut view, BareKey::Tab);
    key(&mut view, BareKey::Up);
    assert_eq!(
        key(&mut view, BareKey::Enter),
        Outcome::Activate("Focus".into())
    );
    key(&mut view, BareKey::Down);
    key(&mut view, BareKey::Down);
    assert_eq!(view.action, 1);
    key(&mut view, BareKey::Tab);
    assert_eq!(key(&mut view, BareKey::Char('x')), Outcome::None);
    for cols in 0..12 {
        for rows in 0..16 {
            let shown = layout(&mut view, cols, rows);
            assert!(
                shown.lines.len() <= rows,
                "{cols}x{rows}: {}",
                shown.lines.len()
            );
        }
    }
}

#[test]
fn reader_back_restores_palette_timeline_and_render_is_bounded() {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Agent".into());
    palette.set_agent_history(header(), events(12), vec![action("Focus")], String::new());
    let surface = Viewport {
        cols: 60,
        rows: 25,
        ..Default::default()
    };
    palette.layout(surface);
    palette.scroll_agent(false);
    let before = palette.agent.as_ref().unwrap().borrow().anchor;
    palette.push_view(43, "Event".into());
    palette.open_agent_reader("Message".into(), "paragraph\n\n".repeat(100), String::new());
    palette.layout(surface);
    palette.handle_key(KeyWithModifier::new(BareKey::End));
    assert!(
        palette
            .agent
            .as_ref()
            .unwrap()
            .borrow()
            .reader
            .as_ref()
            .unwrap()
            .offset
            > 0
    );
    assert_eq!(palette.back(), Outcome::None);
    palette.layout(surface);
    assert_eq!(palette.agent.as_ref().unwrap().borrow().anchor, before);
    palette.layout(Viewport::default());
    let restored = palette.layout(surface);
    assert_eq!(palette.agent.as_ref().unwrap().borrow().anchor, before);
    assert!(palette
        .chunks(&restored, &Style::default())
        .iter()
        .flat_map(|chunk| &chunk.terminal_characters)
        .any(|c| c.styles.bold == Some(AnsiCode::On)));
    for cols in 0..12 {
        for rows in 0..12 {
            let layout = palette.layout(Viewport {
                cols,
                rows,
                ..Default::default()
            });
            for chunk in palette.chunks(&layout, &Style::default()) {
                let text: String = chunk
                    .terminal_characters
                    .iter()
                    .map(|c| c.character)
                    .collect();
                assert!(text.width() <= cols);
                assert!(chunk.y < rows);
            }
        }
    }
}

#[test]
fn agent_timeline_render_snapshot() {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Agent".into());
    palette.set_agent_history(
        header(),
        events(3),
        vec![
            action("Focus"),
            action("Latest / Refresh"),
            action("history-details"),
        ],
        String::new(),
    );
    let layout = palette.layout(Viewport {
        cols: 70,
        rows: 28,
        ..Default::default()
    });
    let text = palette
        .chunks(&layout, &Style::default())
        .iter()
        .map(|chunk| {
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(text);
}

#[test]
fn agent_search_shows_the_cursor_end_of_a_long_unicode_query() {
    let mut view = view(1);
    let mut input = TextInput::default();
    input.insert_text(&format!("{} tail", "世界 ".repeat(30)));
    let layout = view.layout(
        CellRect {
            x: 0,
            y: 0,
            cols: 30,
            rows: 20,
        },
        0,
        &input,
        None,
        false,
    );
    let query = layout
        .lines
        .iter()
        .find(|l| l.text.starts_with("> "))
        .unwrap();
    assert!(query.text.contains("tail"));
    assert!(query.text.width() <= 28);
}

#[test]
fn a_single_available_row_shows_event_content_instead_of_a_separator() {
    let mut view = view(1);
    let shown = layout(&mut view, 30, 1);
    assert_eq!(shown.lines.len(), 1);
    assert_eq!(shown.lines[0].text, "… more");
}
