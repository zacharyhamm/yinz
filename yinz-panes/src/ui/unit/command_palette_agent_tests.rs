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
    view.focus = Focus::Messages;
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
        true,
    )
}
fn key(view: &mut AgentView, key: BareKey) -> Outcome {
    view.key(&KeyWithModifier::new(key), 42, &mut TextInput::default())
}

#[test]
fn previews_wrap_limit_and_keep_styles_and_mouse_geometry() {
    let mut view = view(3);
    let shown = layout(&mut view, 32, 30);
    assert_eq!(view.selected, 2);
    assert!(shown
        .lines
        .iter()
        .any(|l| l.text.trim_start_matches(['┃', ' ']) == "… more"));
    assert!(shown
        .lines
        .iter()
        .any(|l| l.text.trim_start_matches(['┃', ' ']) == "Event 3"
            && l.text
                .chars()
                .zip(&l.styles)
                .filter(|(ch, _)| !ch.is_whitespace() && *ch != '┃')
                .all(|(_, s)| s.bold)));
    assert!(!shown.lines.iter().any(|l| l.text.contains("fourth")));
    for (row, hit) in shown.hits.iter().enumerate() {
        if let Some(Hit::Event(i)) = hit {
            view.click(&shown, 0, row, &mut TextInput::default());
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
    assert!(view.back(&mut TextInput::default()));
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
    assert_eq!(view.focus, Focus::Action(1));
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
fn wheel_scrolls_history_and_readers_independently_of_focus() {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Agent".into());
    palette.set_agent_history(header(), events(30), vec![action("Focus")], String::new());
    let surface = Viewport {
        cols: 60,
        rows: 25,
        ..Default::default()
    };
    palette.layout(surface);
    palette.agent.as_ref().unwrap().borrow_mut().focus = Focus::Search;
    palette.scroll(false);
    let history_anchor = palette.agent.as_ref().unwrap().borrow().anchor;
    assert!(!palette.agent_following());
    for title in ["Message", "tool_call", "tool_result", "Details"] {
        palette.open_agent_reader(title.into(), "line\n\n".repeat(100), String::new());
        palette.layout(surface);
        palette.scroll(true);
        let agent = palette.agent.as_ref().unwrap().borrow();
        assert_eq!(agent.reader.as_ref().unwrap().offset, 3);
        assert_eq!(agent.anchor, history_anchor);
        drop(agent);
        palette.scroll(false);
        assert_eq!(
            palette
                .agent
                .as_ref()
                .unwrap()
                .borrow()
                .reader
                .as_ref()
                .unwrap()
                .offset,
            0
        );
    }
    palette.agent.as_ref().unwrap().borrow_mut().reader = None;
    palette.layout(surface);
    palette.scroll(true);
    assert!(palette.agent_following());
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
    palette.scroll(false);
    let before = palette.agent.as_ref().unwrap().borrow().anchor;
    palette.push_view(43, "Event".into());
    palette.open_agent_reader("Message".into(), "paragraph\n\n".repeat(100), String::new());
    palette.layout(surface);
    palette.scroll(true);
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
    view.search_expanded = true;
    view.focus = Focus::Search;
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
        true,
    );
    let query = layout
        .lines
        .iter()
        .find(|l| l.text.starts_with("Search: [ "))
        .unwrap();
    assert!(query.text.contains("tail"));
    assert!(query.text.width() <= 28);
}

#[test]
fn a_single_available_row_shows_event_content_instead_of_a_separator() {
    let mut view = view(1);
    let shown = layout(&mut view, 30, 1);
    assert_eq!(shown.lines.len(), 1);
    assert_eq!(shown.lines[0].text.trim_start_matches(['┃', ' ']), "… more");
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
        "Session: abc123",
        "yinz · command-palette",
        "Weekly",
        "75%",
        "Event 1",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(
        shown
            .lines
            .iter()
            .position(|l| l.text.contains("Session: abc123"))
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
            assert!(
                agent_header(&view.header, width, height / 2, width >= 44 && height >= 10)
                    .lines
                    .iter()
                    .all(|l| l.text.width() <= width)
            );
            assert!(view.page > 0);
        }
    }
    view.header.usage.as_mut().unwrap().status = ProviderUsageStatus::Error;
    assert!(agent_header(&view.header, 90, 8, true)
        .lines
        .iter()
        .any(|l| l.text.contains("stale")));
}

#[test]
fn tools_and_prompt_envelopes_use_panel_presentation() {
    let mut view = view(0);
    let mut tools = events(1).remove(0);
    tools.kind = "tools".into();
    tools.revisions = vec![1, 2, 3];
    tools.tool_calls = 2;
    tools.markdown = "secret tool output".into();
    view.update(header(), vec![tools], vec![], String::new());
    let shown = layout(&mut view, 80, 25);
    assert!(shown.lines.iter().any(|l| l.text.contains("tools x2")));
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
fn tool_groups_stay_compact_and_preserve_member_selection_on_refresh() {
    let mut view = view(3);
    view.select_revision(2);
    let mut group = events(1).remove(0);
    group.kind = "tools".into();
    group.revisions = vec![1, 2, 3];
    group.tool_calls = 1;
    view.update(header(), vec![group.clone()], vec![], String::new());
    assert_eq!(view.selected, 0);
    assert!(!view.following);
    assert!(
        matches!(key(&mut view, BareKey::Enter), Outcome::Activate(id) if id == "history:42:event:1")
    );
    group.revisions.push(4);
    group.tool_calls = 0;
    view.update(header(), vec![group], vec![], String::new());
    let shown = layout(&mut view, 80, 25);
    assert!(shown
        .lines
        .iter()
        .any(|line| line.text.contains("tools x4")));
    assert_eq!(view.spans[0].len(), 1);
    layout(&mut view, 6, 25);
    assert_eq!(view.spans[0].len(), 1);
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
    view.click(&shown, 0, click, &mut TextInput::default());
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

#[test]
fn live_header_deduplicates_details_and_stays_above_scrolled_history() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    use yinz_types::data::PaneInfo;
    for state in [AgentState::Busy, AgentState::Waiting, AgentState::Idle] {
        let mut view = view(30);
        view.header.project = "project".into();
        view.header.branch = "branch".into();
        view.header.live = Some(PaneInfo {
            title: "task".into(),
            target: yinz_types::command::PaneTarget::ssh("user@host").unwrap(),
            agent: Some(AgentInfo {
                kind: "codex".into(),
                state,
                source: AgentSource::Reported,
                detail: Some("doing work".into()),
                waiting_on: Some("permission".into()),
                wait_needs_a_person: true,
                cwd: None,
                session_id: None,
                run_id: None,
            }),
            ..Default::default()
        });
        view.header.status = format!(
            "[0] · codex · {}",
            match state {
                AgentState::Busy => "working",
                AgentState::Waiting => "needs input",
                _ => "idle",
            }
        );
        let expected = agent_header(&view.header, 88, 20, true).lines;
        let first = layout(&mut view, 90, 30);
        for (actual, expected) in first.lines.iter().zip(&expected) {
            assert_eq!(actual, expected);
        }
        assert!(first.selected[..expected.len()]
            .iter()
            .all(|selected| !selected));
        let text = first
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for label in ["project", "branch", "codex", "[0]", "@user@host"] {
            assert_eq!(text.matches(label).count(), 1, "{label}: {text}");
        }
        assert_eq!(text.matches("Task:").count(), 1, "{text}");
        for label in ["session · /work", "Started", "Session: abc123", "Event 30"] {
            assert!(text.contains(label), "{label}: {text}");
        }

        let mut palette = CommandPalette::default();
        palette.set_agent_history(view.header.clone(), events(1), vec![], String::new());
        let palette_layout = palette.layout(Viewport {
            cols: 90,
            rows: 30,
            ..Default::default()
        });
        let style = Style::default();
        let painted = palette.chunks(&palette_layout, &style);
        for row in painted
            .iter()
            .skip(palette_layout.border)
            .take(expected.len())
        {
            assert!(row.terminal_characters.iter().all(|cell| {
                cell.styles.background != Some(style.colors.text_selected.background.into())
            }));
        }
        view.scroll(false, 12);
        let scrolled = layout(&mut view, 90, 30);
        assert_eq!(
            &first.lines[..expected.len()],
            &scrolled.lines[..expected.len()]
        );
        assert!(scrolled.lines.iter().any(|l| l.text.contains("@user@host")));
    }
}

#[test]
fn constrained_live_header_keeps_provider_state_and_pane_id() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    use yinz_types::command::PaneTarget;
    use yinz_types::data::{PaneId, PaneInfo};
    use yinz_types::internal_ids::TerminalId;

    for (state, status) in [
        (AgentState::Busy, "working"),
        (AgentState::Waiting, "needs input"),
        (AgentState::Idle, "idle"),
    ] {
        let mut palette = CommandPalette::default();
        palette.set_agent_history(
            HistoryHeader {
                project: "project".into(),
                branch: "branch".into(),
                live: Some(PaneInfo {
                    id: PaneId::Terminal(TerminalId::new(42)),
                    title: "agent activity".into(),
                    agent: Some(AgentInfo {
                        kind: "codex".into(),
                        state,
                        source: AgentSource::Reported,
                        detail: None,
                        waiting_on: None,
                        wait_needs_a_person: false,
                        cwd: None,
                        session_id: None,
                        run_id: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            events(1),
            vec![],
            String::new(),
        );
        let layout = palette.layout(Viewport {
            cols: 90,
            rows: 11,
            ..Default::default()
        });
        let identity = &layout.agent.as_ref().unwrap().lines[0];
        for label in ["codex", status, "[42]"] {
            assert!(identity.text.contains(label), "{label}: {identity:?}");
        }
        for host in [None, Some("user@雪-very-long-ssh-host.example.com")] {
            let view = palette.agent.as_mut().unwrap().get_mut();
            let pane = view.header.live.as_mut().unwrap();
            pane.agent.as_mut().unwrap().kind = "claude".into();
            pane.target = host
                .map(|host| PaneTarget::ssh(host).unwrap())
                .unwrap_or(PaneTarget::Local);
            let layout = palette.layout(Viewport {
                cols: 48,
                rows: 24,
                ..Default::default()
            });
            let agent = layout.agent.as_ref().unwrap();
            let identity = &agent.lines[0];
            assert!(identity.text.contains("claude"), "{identity:?}");
            assert!(identity.text.contains("[42]"), "{identity:?}");
            for line in &agent.lines {
                assert!(
                    line.text.width() <= layout.rect.cols - 2 * layout.border,
                    "{line:?}"
                );
                assert!(line.styles.is_empty() || line.styles.len() == line.text.chars().count());
            }
            let id_start = identity.text.find("[42]").unwrap();
            let id_index = identity.text[..id_start].chars().count();
            assert!(identity.styles[id_index..id_index + 4]
                .iter()
                .all(|style| style.color == Some(3) && !style.bold));
        }
    }
}

#[test]
fn quota_placement_respects_spacious_threshold_and_keeps_text_fallback() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    let mut palette = CommandPalette::default();
    let mut header = header();
    header.usage = Some(ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: None,
        plan: None,
        message: None,
        windows: vec![ProviderUsageWindow {
            label: "7d".into(),
            used_percent: 75.0,
            resets_at_ms: None,
        }],
    });
    palette.set_agent_history(header, events(2), vec![], String::new());
    for (cols, rows, image) in [
        (48, 24, true),
        (47, 24, false),
        (90, 14, false),
        (90, 30, true),
    ] {
        let mut layout = palette.layout(Viewport {
            x: 0,
            y: 0,
            cols,
            rows,
        });
        assert_eq!(layout.quota_image().is_some(), image, "{cols}x{rows}");
        assert!(layout
            .agent
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .any(|l| l.text.contains("25%")));
        if let Some(quota) = layout.quota_image() {
            assert_eq!((quota.rect.cols, quota.rect.rows), (18, 2));
            assert!(quota.rect.x + 18 <= layout.rect.x + layout.rect.cols);
            layout.use_quota_image();
            assert!(!layout
                .agent
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .any(|l| l.text.contains("25%")));
        }
    }
}

#[test]
fn quota_custom_windows_remain_visible_in_compact_and_graphical_palette_headers() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    for weekly in [false, true] {
        let mut header = header();
        let mut windows = vec![
            ProviderUsageWindow {
                label: "Daily limit".into(),
                used_percent: 30.0,
                resets_at_ms: Some(0),
            },
            ProviderUsageWindow {
                label: "Weekly Fable".into(),
                used_percent: 40.0,
                resets_at_ms: None,
            },
        ];
        if weekly {
            windows.push(ProviderUsageWindow {
                label: "7d".into(),
                used_percent: 75.0,
                resets_at_ms: None,
            });
        }
        header.usage = Some(ProviderUsage {
            provider: "codex".into(),
            status: ProviderUsageStatus::Error,
            observed_at_ms: Some(0),
            plan: None,
            message: None,
            windows,
        });
        let mut palette = CommandPalette::default();
        palette.set_agent_history(header, events(2), vec![], String::new());
        for rows in [14, 40] {
            let mut layout = palette.layout(Viewport {
                x: 0,
                y: 0,
                cols: 90,
                rows,
            });
            assert_eq!(layout.quota_image().is_some(), weekly && rows == 40);
            layout.use_quota_image();
            let text = layout
                .agent
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for label in ["Daily limit · 70%", "Fable · 60%", "stale", "reset due"] {
                assert!(text.contains(label), "{label}: {text}");
            }
            assert_eq!(
                text.contains(if rows == 40 { "Weekly" } else { "Weekly ·" }),
                weekly || rows == 40,
                "{text}"
            );
            let line = layout
                .agent
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .find(|line| line.text.contains("Fable"))
                .unwrap();
            let width =
                layout.rect.cols - layout.border * 2 - layout.agent.as_ref().unwrap().padding * 2;
            assert_eq!(line.text.width(), width, "{text}");
            if weekly && rows == 40 {
                assert!(line.text.ends_with("Fable · 60%"), "{text}");
            }
        }
    }
}

#[test]
fn custom_quota_percentages_survive_graphical_header_clipping() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    for (label, expected) in [
        ("Weekly Sonnet", "Weekly Sonn · 100%"),
        (
            "Weekly Fable with a very long quota label",
            "Weekly Fabl · 100%",
        ),
        ("Weekly 界界界界 quota label", "Weekly 界界 · 100%"),
    ] {
        let mut header = header();
        header.usage = Some(ProviderUsage {
            provider: "claude".into(),
            status: ProviderUsageStatus::Ok,
            observed_at_ms: None,
            plan: None,
            message: None,
            windows: vec![
                ProviderUsageWindow {
                    label: "7d".into(),
                    used_percent: 25.0,
                    resets_at_ms: None,
                },
                ProviderUsageWindow {
                    label: label.into(),
                    used_percent: 0.0,
                    resets_at_ms: None,
                },
            ],
        });
        let mut palette = CommandPalette::default();
        palette.set_agent_history(header, events(2), vec![], String::new());
        let mut layout = palette.layout(Viewport {
            cols: 90,
            rows: 40,
            ..Default::default()
        });
        assert!(layout.quota_image().is_some());
        layout.use_quota_image();
        let agent = layout.agent.as_ref().unwrap();
        let custom = agent
            .lines
            .iter()
            .find(|line| line.text.ends_with(expected))
            .unwrap_or_else(|| {
                panic!(
                    "{label}: {:?}",
                    agent
                        .lines
                        .iter()
                        .map(|line| &line.text)
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            custom.text.width(),
            layout.rect.cols - layout.border * 2 - agent.padding * 2
        );
        assert_eq!(custom.styles.len(), custom.text.chars().count());
    }
}

#[test]
fn long_custom_quota_labels_stay_within_the_header() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    let mut header = header();
    header.usage = Some(ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: None,
        plan: None,
        message: None,
        windows: vec![
            ProviderUsageWindow {
                label: "7d".into(),
                used_percent: 25.0,
                resets_at_ms: None,
            },
            ProviderUsageWindow {
                label: "Weekly Fable with a very long 界 quota label".into(),
                used_percent: 40.0,
                resets_at_ms: None,
            },
        ],
    });
    for width in [1, 12, 28, 44, 90] {
        for full in [false, true] {
            let lines = agent_header(&header, width, 10, full && width >= 44).lines;
            assert!(lines.iter().all(|line| line.text.width() <= width));
            assert!(lines
                .iter()
                .all(|line| line.styles.len() == line.text.chars().count()));
            if full && width >= 44 {
                let custom = lines
                    .iter()
                    .find(|line| line.text.contains("Weekly Fable"))
                    .unwrap();
                assert_eq!(custom.text.width(), width);
                assert!(custom.text.contains(" │ "));
                assert!(custom.text.ends_with(" · 60%"), "{custom:?}");
            }
        }
    }
}

fn controls_palette(prompt: bool) -> CommandPalette {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Agent".into());
    let mut actions = vec![
        action("Focus"),
        action("Latest / Refresh"),
        action("history-details"),
    ];
    if prompt {
        actions.push(action("history-prompt"));
    }
    palette.set_agent_history(header(), events(3), actions, String::new());
    palette
}

fn focus(palette: &CommandPalette) -> Focus {
    palette.agent.as_ref().unwrap().borrow().focus
}

fn press(palette: &mut CommandPalette, key: BareKey) -> Outcome {
    palette.handle_key(KeyWithModifier::new(key))
}

#[test]
fn search_expands_by_keyboard_or_click_and_escape_clears_before_back() {
    for activation in [BareKey::Enter, BareKey::Char(' ')] {
        let mut palette = controls_palette(true);
        assert_eq!(focus(&palette), Focus::Search);
        palette.insert_text("ignored");
        assert_eq!(palette.query(), "");
        press(&mut palette, activation);
        assert!(palette.agent.as_ref().unwrap().borrow().search_expanded);
        for ch in "jk".chars() {
            press(&mut palette, BareKey::Char(ch));
        }
        palette.insert_text(" needle");
        assert_eq!(palette.query(), "jk needle");
        // History filtering uses the ordinary query, even after Tab leaves the field.
        press(&mut palette, BareKey::Tab);
        assert_eq!(focus(&palette), Focus::Messages);
        palette.set_agent_history(
            header(),
            events(1),
            vec![action("history-prompt")],
            "Matching retained events".into(),
        );
        assert_eq!(palette.query(), "jk needle");
        assert_eq!(
            press(&mut palette, BareKey::Enter),
            Outcome::Activate("history:42:event:1".into())
        );
        assert_eq!(press(&mut palette, BareKey::Esc), Outcome::None);
        assert_eq!(palette.query(), "");
        assert_eq!(focus(&palette), Focus::Search);
        assert!(!palette.agent.as_ref().unwrap().borrow().search_expanded);
        assert_eq!(press(&mut palette, BareKey::Esc), Outcome::None);
        assert!(palette.is_root());
    }
    let mut palette = controls_palette(true);
    let layout = palette.layout(Viewport {
        x: 3,
        y: 2,
        cols: 80,
        rows: 28,
    });
    let widget = &layout.agent.as_ref().unwrap().widgets[0];
    let position = Position::new(
        (layout.rect.y + layout.border + widget.rect.y) as i32,
        (layout.rect.x + layout.border + layout.agent.as_ref().unwrap().padding + widget.rect.x)
            as u16,
    );
    assert_eq!(palette.click(&layout, position), Outcome::None);
    assert!(palette.agent.as_ref().unwrap().borrow().search_expanded);
    assert!(palette
        .cursor(&palette.layout(Viewport {
            cols: 80,
            rows: 28,
            ..Default::default()
        }))
        .is_some());
}

#[test]
fn automatic_run_retarget_preserves_search_and_focus() {
    for tabs in 0..=2 {
        let mut palette = controls_palette(true);
        press(&mut palette, BareKey::Enter);
        palette.insert_text("needle");
        for _ in 0..tabs {
            press(&mut palette, BareKey::Tab);
        }
        let old_focus = focus(&palette);
        palette.agent_retarget_run();
        palette.set_agent_history(
            header(),
            events(1),
            vec![action("history-prompt")],
            String::new(),
        );
        assert_eq!(focus(&palette), old_focus);
        assert_eq!(palette.query(), "needle");
        assert!(palette.agent.as_ref().unwrap().borrow().search_expanded);
        assert_eq!(press(&mut palette, BareKey::Esc), Outcome::None);
        assert_eq!(palette.query(), "");
        assert_eq!(palette.view, 42);
        assert_eq!(focus(&palette), Focus::Search);
        press(&mut palette, BareKey::Esc);
        assert!(palette.is_root());
    }
}

#[test]
fn tab_order_visits_every_control_and_reverses_with_unavailable_prompt_skipped() {
    for prompt in [true, false] {
        let mut palette = controls_palette(prompt);
        let mut targets = vec![Focus::Search, Focus::Messages];
        if prompt {
            targets.push(Focus::Prompt);
        }
        targets.extend([Focus::Action(0), Focus::Action(1), Focus::Action(2)]);
        for expected in targets.iter().cycle().skip(1).take(targets.len()) {
            press(&mut palette, BareKey::Tab);
            assert_eq!(focus(&palette), *expected);
        }
        for expected in targets.iter().rev() {
            palette.handle_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
            assert_eq!(focus(&palette), *expected);
        }
    }
}

#[test]
fn message_navigation_aliases_and_text_inputs_do_not_interfere() {
    for (up, down) in [
        (
            KeyWithModifier::new(BareKey::Up),
            KeyWithModifier::new(BareKey::Down),
        ),
        (
            KeyWithModifier::new(BareKey::Char('k')),
            KeyWithModifier::new(BareKey::Char('j')),
        ),
        (
            KeyWithModifier::new(BareKey::Char('p')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier(),
        ),
    ] {
        let mut palette = controls_palette(true);
        press(&mut palette, BareKey::Enter);
        palette.insert_text("search");
        press(&mut palette, BareKey::Char('j'));
        press(&mut palette, BareKey::Char('k'));
        assert_eq!(palette.query(), "searchjk");
        press(&mut palette, BareKey::Tab);
        palette.handle_key(up);
        assert_eq!(
            press(&mut palette, BareKey::Enter),
            Outcome::Activate("history:42:event:2".into())
        );
        palette.insert_text("ignored");
        press(&mut palette, BareKey::Char('x'));
        palette.handle_key(down);
        assert_eq!(
            press(&mut palette, BareKey::Enter),
            Outcome::Activate("history:42:event:3".into())
        );
        press(&mut palette, BareKey::Tab);
        press(&mut palette, BareKey::Char('j'));
        press(&mut palette, BareKey::Char('k'));
        palette.insert_text(" prompt\nline");
        assert_eq!(palette.query(), "searchjk");
        let expected = Outcome::Submit {
            id: "history-prompt".into(),
            values: vec!["jk prompt line".into()],
        };
        assert_eq!(press(&mut palette, BareKey::Enter), expected);
        palette.set_error("Send failed".into());
        assert_eq!(press(&mut palette, BareKey::Enter), expected);
        assert!(palette.agent_prompt_sent());
        assert_eq!(press(&mut palette, BareKey::Enter), Outcome::None);
        assert!(palette.error.is_none());
        assert_eq!(palette.view, 42);
    }
}

#[test]
fn horizontal_buttons_have_separate_hits_and_focus_styles() {
    let mut palette = controls_palette(true);
    let surface = Viewport {
        x: 5,
        y: 4,
        cols: 80,
        rows: 28,
    };
    let layout = palette.layout(surface);
    let agent = layout.agent.as_ref().unwrap();
    let buttons: Vec<_> = agent
        .widgets
        .iter()
        .filter(|w| matches!(w.hit, Hit::Action(_)))
        .collect();
    assert_eq!(buttons.len(), 3);
    assert_eq!(buttons[0].rect.y, buttons[1].rect.y);
    for (index, button) in buttons[..2].iter().enumerate() {
        let x = layout.rect.x + layout.border + agent.padding + button.rect.x;
        let y = layout.rect.y + layout.border + button.rect.y;
        assert_eq!(
            palette.click(&layout, Position::new(y as i32, x as u16)),
            Outcome::Activate(["Focus", "Latest / Refresh"][index].into())
        );
        let painted = palette.chunks(&palette.layout(surface), &Style::default());
        let row = painted.iter().find(|row| row.y == y).unwrap();
        assert_eq!(
            row.terminal_characters[x - layout.rect.x].styles.background,
            Some(Style::default().colors.ribbon_selected.background.into())
        );
        let other = buttons[1 - index];
        assert_ne!(
            row.terminal_characters[layout.border + agent.padding + other.rect.x]
                .styles
                .background,
            Some(Style::default().colors.ribbon_selected.background.into())
        );
    }
    press(&mut palette, BareKey::Left);
    assert_eq!(focus(&palette), Focus::Action(0));
    assert_eq!(
        press(&mut palette, BareKey::Char(' ')),
        Outcome::Activate("Focus".into())
    );
    press(&mut palette, BareKey::Right);
    assert_eq!(
        press(&mut palette, BareKey::Enter),
        Outcome::Activate("Latest / Refresh".into())
    );
    // A gap between adjacent buttons is not a hit on either button.
    assert_eq!(
        palette.click(
            &layout,
            Position::new(
                (layout.rect.y + layout.border + buttons[0].rect.y) as i32,
                (layout.rect.x + layout.border + agent.padding + buttons[0].rect.cols) as u16
            )
        ),
        Outcome::None
    );
}

#[test]
fn palette_messages_match_panel_text_spacing_and_styles() {
    use agent_proto::agents::{AgentRun, ParsedRun, RunEntry, RunEntryKind};
    let now = agent_proto::agents::now_ms();
    let entries: Vec<_> = [
        RunEntryKind::Prompt,
        RunEntryKind::Message,
        RunEntryKind::ToolCall,
        RunEntryKind::ToolResult,
        RunEntryKind::Summary,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| RunEntry {
        kind,
        revision: i as u64 + 1,
        created_at_ms: Some(now - 120_000),
        text: "**hello** 界\n\n- first\n- second".into(),
        event_id: None,
        tool: None,
    })
    .collect();
    let mut run = AgentRun {
        id: "codex:test".into(),
        pane_id: 1,
        kind: "codex".into(),
        started_at_ms: 0,
        yinz_session_name: Default::default(),
        host_id: Default::default(),
        session_id: "test".into(),
        origin_run_id: None,
        previous_run_id: None,
        cwd: None,
        host: None,
        bubblewrap: None,
        ended_at_ms: None,
        outcome: Default::default(),
        revision: 5,
        summarized_revision: 0,
        summary: None,
        entries: vec![],
        omitted_entries: 0,
    };
    run.entries = entries.clone();
    for width in [30, 60, 90] {
        let expected = ParsedRun::new(&run).lines(width + 1, now, 0, None);
        let mut view = view(0);
        view.update(
            header(),
            entries
                .iter()
                .map(|entry| HistoryEvent {
                    revision: entry.revision,
                    revisions: vec![entry.revision],
                    tool_calls: 0,
                    kind: entry.kind.label().into(),
                    created_at_ms: entry.created_at_ms,
                    markdown: entry.text.clone(),
                    time: String::new(),
                    truncated: false,
                })
                .collect(),
            vec![],
            String::new(),
        );
        view.focus = Focus::Search;
        // Layout's content width is width, with one column for the panel selection marker.
        let shown = layout(&mut view, width + 2, 80);
        let actual: Vec<_> = shown
            .lines
            .iter()
            .zip(&shown.hits)
            .filter(|(_, hit)| matches!(hit, Some(Hit::Event(_))))
            .map(|(line, _)| line.clone())
            .collect();
        let expected: Vec<_> = expected
            .entries
            .into_iter()
            .map(|line| line.prepend(" ", MdStyle::default()))
            .collect();
        assert_eq!(actual, expected, "width {width}");
    }
}

#[test]
fn agent_info_control_render_snapshots() {
    for (name, tabs, expanded, cols) in [
        ("collapsed", 0, false, 70),
        ("expanded", 0, true, 70),
        ("messages", 1, false, 70),
        ("prompt", 2, false, 70),
        ("buttons", 4, false, 70),
        ("narrow", 4, false, 30),
    ] {
        let mut palette = controls_palette(true);
        if expanded {
            press(&mut palette, BareKey::Enter);
            palette.insert_text("controls");
        }
        for _ in 0..tabs {
            press(&mut palette, BareKey::Tab);
        }
        let layout = palette.layout(Viewport {
            cols,
            rows: 28,
            ..Default::default()
        });
        let text = palette
            .chunks(&layout, &Style::default())
            .iter()
            .map(|row| {
                row.terminal_characters
                    .iter()
                    .map(|c| c.character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(format!("agent_info_{name}"), text);
    }
}

#[test]
fn input_cursors_and_wrapped_buttons_stay_inside_constrained_windows() {
    for cols in [1, 4, 8, 12, 20, 30, 50, 80] {
        for rows in [1, 4, 8, 12, 20] {
            let mut palette = controls_palette(true);
            palette.agent.as_mut().unwrap().get_mut().header.live =
                Some(yinz_types::data::PaneInfo {
                    title: "task".into(),
                    agent: Some(yinz_types::agent::AgentInfo {
                        kind: "codex".into(),
                        state: yinz_types::agent::AgentState::Idle,
                        source: yinz_types::agent::AgentSource::Reported,
                        detail: None,
                        waiting_on: None,
                        wait_needs_a_person: false,
                        cwd: None,
                        session_id: None,
                        run_id: None,
                    }),
                    ..Default::default()
                });
            let surface = Viewport {
                x: 3,
                y: 2,
                cols,
                rows,
            };
            press(&mut palette, BareKey::Enter);
            palette.insert_text(&"界jk ".repeat(30));
            for _ in 0..6 {
                let layout = palette.layout(surface);
                let agent = layout.agent.as_ref().unwrap();
                assert!(
                    agent.lines.len() <= rows.saturating_sub(layout.border * 2),
                    "{cols}x{rows}"
                );
                if rows >= 8 {
                    assert!(palette.agent.as_ref().unwrap().borrow().page > 0);
                }
                if let Some((x, y)) = palette.cursor(&layout) {
                    assert!(layout.rect.contains(x, y));
                }
                for chunk in palette.chunks(&layout, &Style::default()) {
                    let text: String = chunk
                        .terminal_characters
                        .iter()
                        .map(|c| c.character)
                        .collect();
                    assert!(text.width() <= cols);
                }
                press(&mut palette, BareKey::Tab);
                palette.insert_text("jk");
            }
        }
    }
}

#[test]
fn balanced_header_keeps_usage_in_one_column_with_no_duplicate_values() {
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    let mut data = header();
    data.project = "yinz".into();
    data.branch = "command-palette".into();
    data.usage = Some(ProviderUsage {
        provider: "codex".into(),
        status: ProviderUsageStatus::Ok,
        observed_at_ms: None,
        plan: None,
        message: None,
        windows: vec![
            ProviderUsageWindow {
                label: "7d".into(),
                used_percent: 58.0,
                resets_at_ms: Some(0),
            },
            ProviderUsageWindow {
                label: "5h".into(),
                used_percent: 82.0,
                resets_at_ms: None,
            },
        ],
    });
    let grid = agent_header(&data, 78, 12, true);
    assert_eq!(grid.lines.len(), 6);
    let graphical = grid.graphical.as_ref().unwrap();
    assert_eq!(graphical.len(), 6);
    for lines in [&grid.lines, graphical] {
        for (row, line) in lines.iter().enumerate() {
            let (left, right) = line.text.split_once(" │ ").unwrap();
            assert_eq!(left.width(), 57);
            assert_eq!(right.width(), 18);
            assert!(!left.trim().is_empty());
            assert_eq!(line.styles.len(), line.text.chars().count());
            assert_eq!(
                right.trim().is_empty(),
                if std::ptr::eq(lines, graphical) {
                    (1..=2).contains(&row) || row >= 5
                } else {
                    row >= 4
                }
            );
        }
    }
    let text = grid
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for value in [
        "codex",
        "Agent task",
        "command-palette",
        "abc123",
        "42%",
        "18%",
        "reset due",
    ] {
        assert_eq!(text.matches(value).count(), 1, "{value}: {text}");
    }
    assert!(!text.contains("Updated"));
    assert!(grid.lines[2].text.ends_with("5hr · 18%"));
    assert!(graphical[3].text.ends_with("5hr · 18%"));
    insta::assert_snapshot!("agent_header_balanced_text", text);
    insta::assert_snapshot!(
        "agent_header_balanced_graphic_cells",
        graphical
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn balanced_header_missing_fields_and_unicode_stay_bounded() {
    for data in [
        HistoryHeader::default(),
        HistoryHeader {
            task: "界🦀 long task ".repeat(20),
            location: "/界/🦀/very-long-directory".repeat(20),
            session_id: "run archived-123".into(),
            ..header()
        },
    ] {
        for width in [0, 1, 12, 43, 44, 78] {
            for budget in [0, 1, 3, 6, 10] {
                let grid = agent_header(&data, width, budget, true);
                assert!(grid.lines.len() <= budget);
                assert!(grid.quota.is_none());
                for line in &grid.lines {
                    assert!(line.text.width() <= width, "{line:?}");
                    assert_eq!(line.styles.len(), line.text.chars().count());
                }
                if width >= 44 && budget >= 6 {
                    assert_eq!(grid.lines.len(), 6);
                    assert!(grid.lines[0].text.contains("Usage unavailable"));
                    assert!(grid.lines.iter().all(|l| l.text.contains(" │ ")));
                    if data.session_id.starts_with("run ") {
                        assert!(grid.lines[5].text.contains("Run: archived-123"));
                        assert!(grid.lines[1]
                            .text
                            .split_once(" │ ")
                            .unwrap()
                            .0
                            .trim_end()
                            .ends_with('…'));
                    }
                }
            }
        }
    }
}

#[test]
fn ribbon_buttons_follow_font_mode_and_theme_with_unicode_labels() {
    for arrow_fonts in [false, true] {
        let mut palette = controls_palette(true);
        let view = palette.agent.as_mut().unwrap().get_mut();
        view.actions = vec![action("界 Focus"), action("Details")];
        view.focus = Focus::Action(0);
        let surface = Viewport {
            cols: 80,
            rows: 28,
            ..Default::default()
        };
        let layout = palette.layout_with_arrow_fonts(surface, arrow_fonts);
        let agent = layout.agent.as_ref().unwrap();
        let style = Style::default();
        let chunks = palette.chunks(&layout, &style);
        for widget in agent
            .widgets
            .iter()
            .filter(|w| matches!(w.hit, Hit::Action(_)))
        {
            let row = &chunks[layout.border + widget.rect.y];
            let start = layout.border + agent.padding + widget.rect.x;
            let mut column = 0;
            let cells: Vec<_> = row
                .terminal_characters
                .iter()
                .filter(|c| {
                    let inside = (start..start + widget.rect.cols).contains(&column);
                    column += c.character.width().unwrap_or(0);
                    inside
                })
                .collect();
            let label = if widget.focused {
                "界 Focus"
            } else {
                "Details"
            };
            let expected = if arrow_fonts {
                format!(" {label} ")
            } else {
                format!(" {label} ")
            };
            assert_eq!(
                cells.iter().map(|c| c.character).collect::<String>(),
                expected
            );
            assert_eq!(widget.rect.cols, expected.width());
            let declaration = if widget.focused {
                style.colors.ribbon_selected
            } else {
                style.colors.ribbon_unselected
            };
            let label_cell = cells[if arrow_fonts { 2 } else { 1 }];
            assert_eq!(label_cell.styles.foreground, Some(declaration.base.into()));
            assert_eq!(
                label_cell.styles.background,
                Some(declaration.background.into())
            );
            if arrow_fonts {
                assert_eq!(
                    cells[0].styles.foreground,
                    Some(style.colors.text_unselected.background.into())
                );
                assert_eq!(
                    cells.last().unwrap().styles.foreground,
                    Some(declaration.background.into())
                );
                assert_eq!(
                    cells.last().unwrap().styles.background,
                    Some(style.colors.text_unselected.background.into())
                );
            }
            let index = match widget.hit {
                Hit::Action(index) => index,
                _ => unreachable!(),
            };
            for x in 0..widget.rect.cols {
                assert_eq!(
                    palette.click(
                        &layout,
                        Position::new(
                            (layout.rect.y + layout.border + widget.rect.y) as i32,
                            (layout.rect.x + start + x) as u16
                        )
                    ),
                    Outcome::Activate(["界 Focus", "Details"][index].into())
                );
            }
        }
    }
}

#[test]
fn backfill_preserves_tail_and_manual_anchor() {
    let mut view = view(0);
    view.update(
        header(),
        events(90).into_iter().skip(60).collect(),
        vec![],
        String::new(),
    );
    layout(&mut view, 80, 25);
    assert_eq!(view.offset, view.total.saturating_sub(view.page));
    view.scroll(false, 10);
    let anchor = view.anchor;
    view.update(header(), events(100), vec![], String::new());
    layout(&mut view, 80, 25);
    assert_eq!(view.anchor, anchor);
    assert!(!view.following);
    view.scroll(true, usize::MAX);
    view.update(header(), events(101), vec![], String::new());
    layout(&mut view, 60, 20);
    assert!(view.following);
    assert_eq!(view.offset, view.total.saturating_sub(view.page));
}

#[test]
fn mouse_selection_survives_background_history_updates() {
    let mut view = view(0);
    view.update(
        header(),
        events(4).into_iter().skip(1).collect(),
        vec![],
        String::new(),
    );
    let shown = layout(&mut view, 80, 40);
    assert!(view.following);
    let row = shown
        .hits
        .iter()
        .position(|hit| matches!(hit, Some(Hit::Event(1))))
        .unwrap();
    view.click(&shown, 20, row, &mut TextInput::default());
    assert!(!view.following);
    for count in [4, 5, 5] {
        view.update(header(), events(count), vec![], String::new());
        layout(&mut view, 80, 40);
        assert_eq!(
            key(&mut view, BareKey::Enter),
            Outcome::Activate("history:42:event:3".into())
        );
    }
}
