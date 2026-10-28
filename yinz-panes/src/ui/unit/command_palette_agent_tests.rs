use super::*;
use unicode_width::UnicodeWidthStr;

fn header() -> HistoryHeader {
    HistoryHeader {
        name: "Agent".into(),
        status: "codex · idle".into(),
        location: "session · /work".into(),
        started: "2026-10-09 10:00 UTC".into(),
        details: "literal **metadata**\nsecond line".into(),
        task: "Agent task".into(),
        session_id: "session abc123".into(),
        ..Default::default()
    }
}
fn events(count: u64) -> Vec<HistoryEvent> {
    (1..=count)
        .map(|revision| HistoryEvent {
            revision,
            revisions: vec![revision],
            tool_calls: 0,
            created_at_ms: None,
            kind: "message".into(),
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
    assert!(shown.lines.iter().any(|l| l.text.trim() == "… more"));
    assert!(shown.lines.iter().any(|l| l.text.trim() == "Event 3"
        && l.text
            .chars()
            .zip(&l.styles)
            .filter(|(ch, _)| !ch.is_whitespace())
            .all(|(_, s)| s.bold)));
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
    assert_eq!(shown.lines[0].text.trim(), "… more");
}

#[test]
fn pinned_details_and_quota_leave_room_for_chat_at_small_sizes() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    let mut h = header();
    h.project = "yinz · command-palette".into();
    h.usage = Some(ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: Some(agent_proto::agents::now_ms() as u64),
        plan: None,
        message: None,
        windows: vec![ProviderUsageWindow {
            label: "Weekly limit".into(),
            used_percent: 25.0,
            resets_at_ms: None,
        }],
    });
    let mut view = view(1);
    view.update(h, events(1), vec![], String::new());
    let shown = layout(&mut view, 90, 30);
    let text = shown
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Task: Agent task",
        "session abc123",
        "yinz · command-palette",
        "Weekly · 75%",
        "Event 1",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(
        shown
            .lines
            .iter()
            .position(|l| l.text.contains("session abc123"))
            .unwrap()
            < shown
                .lines
                .iter()
                .position(|l| l.text.contains("Event 1"))
                .unwrap()
    );
    for width in [1, 12, 28, 60, 90] {
        for height in [1, 4, 8, 12, 30] {
            let shown = layout(&mut view, width, height);
            assert!(shown.lines.len() <= height);
            assert!(header_lines(&view.header, width, height / 2)
                .iter()
                .all(|l| l.text.width() <= width));
            assert!(view.page > 0);
        }
    }
    view.header.usage.as_mut().unwrap().status = ProviderUsageStatus::Error;
    assert!(header_lines(&view.header, 90, 8)
        .iter()
        .any(|l| l.text.contains("stale")));
}

#[test]
fn tools_collapse_and_prompt_envelopes_open_with_panel_folds() {
    let mut view = view(0);
    let mut tools = events(1).remove(0);
    tools.kind = "tool_call".into();
    tools.revisions = vec![1, 2, 3];
    tools.tool_calls = 2;
    tools.markdown = "secret tool output".into();
    view.update(header(), vec![tools], vec![], String::new());
    let shown = layout(&mut view, 80, 25);
    assert!(shown.lines.iter().any(|l| l.text.contains("tool call x2")));
    assert!(!shown
        .lines
        .iter()
        .any(|l| l.text.contains("secret tool output")));
    view.select_revision(3);
    assert_eq!(view.selected, 0);
    // The same envelope parser and folding renderer used by the Agents panel.
    let text = "<task-notification>\n<task-id>task-1</task-id>\n<summary>done</summary>\n<result>first\nsecond\nthird</result>\n</task-notification>";
    view.reader("prompt".into(), text.into(), String::new());
    layout(&mut view, 80, 25);
    let reader = view.reader.as_ref().unwrap();
    assert!(!reader.sections.is_empty());
    let target = reader.sections.iter().position(|s| s.foldable()).unwrap();
    view.reader.as_mut().unwrap().fold = target;
    let before = view.reader.as_ref().unwrap().sections[target].open;
    key(&mut view, BareKey::Enter);
    assert_ne!(view.reader.as_ref().unwrap().sections[target].open, before);
    key(&mut view, BareKey::Char('v'));
    assert!(view.reader.as_ref().unwrap().raw);
}

#[test]
fn clearing_run_search_restores_conversation_position() {
    for grouped in [false, true] {
        let mut view = view(30);
        layout(&mut view, 70, 25);
        view.scroll(false, 12);
        let before = (view.anchor, view.offset, view.following);
        view.search_mode(true);
        view.update(header(), events(2), vec![], String::new());
        layout(&mut view, 70, 25);
        view.search_mode(false);
        let mut restored = events(31);
        if grouped {
            restored[28].revisions.push(30);
            restored.remove(29);
        }
        view.update(header(), restored, vec![], String::new());
        layout(&mut view, 70, 25);
        assert_eq!((view.anchor, view.offset, view.following), before);
        assert_eq!(
            view.events[view.selected].data.revision,
            if grouped { 29 } else { 30 }
        );
    }
}

#[test]
fn clearing_search_reveals_fallback_when_original_page_is_gone() {
    let mut view = view(30);
    view.select_revision(20);
    layout(&mut view, 70, 25);
    assert!(view.offset > 0);
    view.search_mode(true);
    view.update(header(), events(2), vec![], String::new());
    layout(&mut view, 70, 25);
    view.search_mode(false);
    view.update(
        header(),
        events(90).into_iter().skip(60).collect(),
        vec![],
        String::new(),
    );
    assert_eq!(view.anchor, None);
    assert!(view.reveal);
    let shown = layout(&mut view, 70, 25);
    assert_eq!(view.selected, 0);
    assert!(shown
        .hits
        .iter()
        .zip(&shown.selected)
        .any(|(hit, selected)| { matches!(hit, Some(Hit::Event(0))) && *selected }));
    assert_eq!(
        key(&mut view, BareKey::Enter),
        Outcome::Activate("history:42:event:61".into())
    );
}

#[test]
fn clearing_search_resumes_following_new_messages() {
    let mut view = view(10);
    view.search_mode(true);
    view.update(header(), events(2), vec![], String::new());
    view.search_mode(false);
    view.update(header(), events(11), vec![], String::new());
    assert!(view.following);
    assert_eq!(view.selected, 10);
}

#[test]
fn explicit_page_navigation_overrides_saved_search_position() {
    for following in [false, true] {
        let mut view = view(30);
        layout(&mut view, 70, 25);
        view.scroll(false, 12);
        view.search_mode(true);
        view.update(header(), events(2), vec![], String::new());
        layout(&mut view, 70, 25);
        view.page_request(following);
        view.search_mode(false);
        let page = if following {
            events(31)
        } else {
            events(30).into_iter().skip(10).collect()
        };
        view.update(header(), page, vec![], String::new());
        let shown = layout(&mut view, 70, 25);
        assert_eq!(view.following, following);
        assert!(view.saved_position.is_none());
        if following {
            assert_eq!(view.selected, 30);
            assert_eq!(view.offset, view.total - view.page);
        } else {
            assert_eq!(view.selected, 0);
            assert_eq!(view.offset, 0);
            assert!(shown.hits.iter().any(|h| matches!(h, Some(Hit::Event(0)))));
        }
    }
}

#[test]
fn tool_search_results_show_distinct_snippets() {
    let mut view = view(0);
    let mut matches = events(3);
    for (event, (kind, snippet)) in matches.iter_mut().zip([
        ("tool_call", "first matching [command]"),
        ("tool_result", "matching C:\\logs\\output *literal* `code`"),
        ("tool_call", "second matching [ERROR]"),
    ]) {
        event.kind = kind.into();
        event.markdown = snippet.into();
    }
    view.search_mode(true);
    view.update(header(), matches, vec![], String::new());
    let shown = layout(&mut view, 80, 40);
    for snippet in [
        "first matching [command]",
        "matching C:\\logs\\output *literal* `code`",
        "second matching [ERROR]",
    ] {
        assert!(shown.lines.iter().any(|l| l.text.contains(snippet)));
    }
    assert!(!shown.lines.iter().any(|l| l.text.contains("tool call x")));
}

#[test]
fn folded_reader_reuses_parsing_and_wrapping_and_keeps_hit_targets() {
    let mut view = view(0);
    let text = format!(
        "<task-notification>\n<task-id>task-1</task-id>\n<summary>finished collecting history metadata and preparing detailed retained results</summary>\n<result>{}</result>\n</task-notification>",
        "first line with enough words to wrap when the reader gets narrower\n".repeat(20)
    );
    view.reader("prompt".into(), text.clone(), String::new());
    let shown = layout(&mut view, 80, 20);
    let cache = |view: &AgentView| {
        let reader = view.reader.as_ref().unwrap();
        (
            reader
                .section_blocks
                .iter()
                .map(|b| b.as_ptr())
                .collect::<Vec<_>>(),
            reader.lines.as_ptr(),
            reader.starts.as_ptr(),
        )
    };
    let assert_hits = |view: &AgentView, shown: &AgentLayout| {
        let reader = view.reader.as_ref().unwrap();
        for (row, hit) in shown.hits.iter().enumerate() {
            if let Some(Hit::Fold(i)) = hit {
                // The header and separator occupy two rows at this height.
                let line = row - 2 + reader.offset;
                assert_eq!(shown.lines[row], reader.lines[line]);
                assert_eq!(
                    *i,
                    reader.starts.partition_point(|start| *start <= line) - 1
                );
                assert_eq!(
                    shown.selected[row],
                    *i == reader.fold && line == reader.starts[*i]
                );
            }
        }
    };
    assert_hits(&view, &shown);
    let initial = cache(&view);
    for bare in [BareKey::Down, BareKey::Char('j'), BareKey::Char('k')] {
        key(&mut view, bare);
        let shown = layout(&mut view, 80, 20);
        assert_eq!(cache(&view), initial);
        assert_hits(&view, &shown);
    }
    let target = view
        .reader
        .as_ref()
        .unwrap()
        .sections
        .iter()
        .position(|s| s.foldable())
        .unwrap();
    view.reader.as_mut().unwrap().fold = target;
    key(&mut view, BareKey::Enter);
    assert_eq!(view.reader.as_ref().unwrap().width, None);
    let toggled = cache(&view);
    for (i, (before, after)) in initial.0.iter().zip(&toggled.0).enumerate() {
        assert_eq!(before == after, i != target);
    }
    let shown = layout(&mut view, 80, 20);
    assert_hits(&view, &shown);
    let open = cache(&view);
    assert_ne!(open.1, initial.1);
    assert_ne!(open.2, initial.2);
    view.scroll(true, 4);
    let shown = layout(&mut view, 80, 20);
    assert_eq!(cache(&view), open);
    assert_hits(&view, &shown);
    let starts = view.reader.as_ref().unwrap().starts.clone();
    let shown = layout(&mut view, 24, 20);
    let resized = cache(&view);
    assert_eq!(resized.0, open.0);
    assert_ne!(resized.1, open.1);
    assert_hits(&view, &shown);
    let reader = view.reader.as_ref().unwrap();
    assert_eq!(reader.lines.len(), view.total);
    assert_ne!(starts, reader.starts);
    let click = shown
        .hits
        .iter()
        .position(|hit| matches!(hit, Some(Hit::Fold(i)) if *i == target))
        .unwrap();
    view.click(&shown, click);
    assert_eq!(view.reader.as_ref().unwrap().width, None);
    let shown = layout(&mut view, 24, 20);
    assert_hits(&view, &shown);
    assert!(!view.reader.as_ref().unwrap().sections[target].open);

    key(&mut view, BareKey::Char('v'));
    let raw = layout(&mut view, 24, 20);
    assert!(!raw.hits.iter().any(|h| matches!(h, Some(Hit::Fold(_)))));
    let raw_cache = view.reader.as_ref().unwrap().document.lines.as_ptr();
    key(&mut view, BareKey::Down);
    layout(&mut view, 24, 20);
    assert_eq!(
        view.reader.as_ref().unwrap().document.lines.as_ptr(),
        raw_cache
    );
    key(&mut view, BareKey::Char('v'));
    layout(&mut view, 24, 20);
    key(&mut view, BareKey::Char('v'));
    layout(&mut view, 24, 20);
    assert_eq!(
        view.reader.as_ref().unwrap().document.lines.as_ptr(),
        raw_cache
    );

    view.reader("prompt".into(), text, String::new());
    let reader = view.reader.as_ref().unwrap();
    assert_eq!(reader.width, None);
    assert!(reader.lines.is_empty());
    assert!(reader.starts.is_empty());
    assert!(!reader.raw);
    assert_ne!(cache(&view).0, resized.0);
}
