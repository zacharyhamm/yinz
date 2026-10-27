use super::*;
use unicode_width::UnicodeWidthStr;
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    command::PaneTarget,
    data::{PaneId, PaneInfo},
    internal_ids::TerminalId,
};

fn pane(state: AgentState) -> PaneInfo {
    PaneInfo {
        id: PaneId::Terminal(TerminalId::new(42)),
        title: "**task**_[title] 日本語".into(),
        target: PaneTarget::ssh("user@雪").unwrap(),
        agent: Some(AgentInfo {
            kind: "co*dex◆".into(),
            state,
            source: AgentSource::Reported,
            detail: Some("`tool` [arg] **literal**".into()),
            waiting_on: Some("Which **option**_ [a]?".into()),
            wait_needs_a_person: true,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
        ..Default::default()
    }
}

fn palette(count: usize) -> CommandPalette {
    let mut palette = CommandPalette::default();
    let entries = (0..count)
        .map(|i| {
            let id = format!("agent:{i}");
            palette
                .agent_metadata
                .insert(id.clone(), Default::default());
            palette.agent_presentations.insert(
                id.clone(),
                AgentPresentation::from_pane(&pane(AgentState::Busy)).unwrap(),
            );
            Entry {
                id,
                label: format!("Search title {i}"),
                description: "search metadata".into(),
                shortcut: String::new(),
                aliases: vec!["alias".into()],
                kind: EntryKind::Agent,
            }
        })
        .collect();
    palette.set_entries(entries);
    palette.open_picker(EntryKind::Agent);
    palette
}

fn texts(lines: &[MdLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

#[test]
fn live_agent_activity_follows_state_and_falls_back_to_literal_title() {
    for (state, expected) in [
        (AgentState::Busy, "`tool` [arg] **literal**"),
        (AgentState::Waiting, "Which **option**_ [a]?"),
        (AgentState::Idle, "**task**_[title] 日本語"),
    ] {
        let mut pane = pane(state);
        let presentation = AgentPresentation::from_pane(&pane).unwrap();
        assert_eq!(presentation.activity, expected);
        assert_eq!(presentation.provider, "co*dex");
        assert_eq!(presentation.host.as_deref(), Some("user@雪"));
        assert_eq!(presentation.pane_id, "42");
        let agent = pane.agent.as_mut().unwrap();
        agent.detail = Some(" \r\n\t ".into());
        agent.waiting_on = None;
        assert_eq!(
            AgentPresentation::from_pane(&pane).unwrap().activity,
            pane.title
        );
        pane.target = PaneTarget::Local;
        assert_eq!(AgentPresentation::from_pane(&pane).unwrap().host, None);
        pane.agent = None;
        assert!(AgentPresentation::from_pane(&pane).is_none());
    }
}

#[test]
fn live_agent_rendering_matches_panel_emphasis_and_selected_theme() {
    let mut palette = palette(2);
    let mut style = Style::default();
    // Distinct selected colors catch accidentally using the unselected palette.
    style.colors.text_selected.emphasis_1 = style.colors.text_unselected.emphasis_2;
    style.colors.text_selected.emphasis_2 = style.colors.text_unselected.emphasis_3;
    style.colors.text_selected.emphasis_3 = style.colors.text_unselected.emphasis_0;
    for (state, glyph, status, color) in [
        (AgentState::Busy, '●', "working", Some(2)),
        (AgentState::Waiting, '!', "needs input", Some(1)),
        (AgentState::Idle, '○', "idle", None),
    ] {
        palette.agent_presentations.insert(
            "agent:0".into(),
            AgentPresentation::from_pane(&pane(state)).unwrap(),
        );
        for selected in [false, true] {
            palette.select_id(if selected { "agent:0" } else { "agent:1" });
            let layout = palette.layout(Viewport {
                cols: 80,
                rows: 40,
                ..Default::default()
            });
            let chunks = palette.chunks(&layout, &style);
            let rows = layout
                .result_rows
                .iter()
                .enumerate()
                .filter_map(|(row, result)| {
                    matches!(result, ResultRow::Entry(0, _)).then_some(
                        &chunks[layout.options_y + row - layout.rect.y].terminal_characters,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 2);
            let main = rows[0].iter().map(|ch| ch.character).collect::<String>();
            assert!(
                main.contains(&format!("{glyph} co*dex  {status} @user@雪")),
                "{main}"
            );
            assert!(main.trim_end_matches([' ', '│']).ends_with("[42]"));
            let activity = rows[1].iter().map(|ch| ch.character).collect::<String>();
            assert!(
                activity.contains(&format!(
                    "    {}",
                    AgentPresentation::from_pane(&pane(state)).unwrap().activity
                )),
                "{activity}"
            );
            let colors = if selected {
                style.colors.text_selected
            } else {
                style.colors.text_unselected
            };
            let tone = match color {
                Some(1) => colors.emphasis_1,
                Some(2) => colors.emphasis_2,
                _ => colors.base,
            };
            let glyph_cell = rows[0].iter().find(|ch| ch.character == glyph).unwrap();
            assert_eq!(glyph_cell.styles.foreground, Some(tone.into()));
            assert_eq!(glyph_cell.styles.bold, Some(AnsiCode::On));
            let provider = rows[0].iter().position(|ch| ch.character == 'c').unwrap();
            assert_eq!(rows[0][provider].styles.bold, Some(AnsiCode::On));
            assert_eq!(
                rows[0][provider].styles.foreground,
                Some(colors.base.into())
            );
            let status_start = main.find(status).unwrap();
            let status_index = main[..status_start].chars().count();
            assert_eq!(rows[0][status_index].styles.foreground, Some(tone.into()));
            for ch in rows[0]
                .iter()
                .filter(|ch| ['@', '[', ']'].contains(&ch.character))
            {
                assert_eq!(ch.styles.foreground, Some(colors.emphasis_3.into()));
            }
            let first_activity = rows[1].get(layout.border + 4).unwrap();
            assert_eq!(
                first_activity.styles.bold,
                if state == AgentState::Waiting {
                    Some(AnsiCode::On)
                } else {
                    Some(AnsiCode::Reset)
                }
            );
            assert_eq!(
                first_activity.styles.foreground,
                Some(
                    if state == AgentState::Waiting {
                        colors.emphasis_1
                    } else {
                        colors.base
                    }
                    .into()
                )
            );
            for row in rows {
                assert_eq!(
                    row[layout.border].styles.background,
                    if selected {
                        Some(colors.background.into())
                    } else {
                        Some(AnsiCode::Reset)
                    }
                );
            }
        }
    }
}

#[test]
fn live_agents_wrap_without_losing_words_and_cap_main_before_activity() {
    let mut palette = palette(1);
    let presentation = palette.agent_presentations.get_mut("agent:0").unwrap();
    presentation.provider = "codex".into();
    presentation.host = None;
    presentation.activity =
        "one two 三四 e\u{301} six seven eight nine ten eleven twelve thirteen fourteen".into();
    let lines = palette.entry_lines(0, 24, 4);
    assert_eq!(
        texts(&lines),
        [
            "● codex  working  [42]",
            "  one two 三四 e\u{301} six",
            "  seven eight nine ten",
            "  eleven twelve…"
        ]
    );
    assert!(lines[2..].iter().all(|line| line.text.starts_with("  ")));
    // A wrapped main row consumes the budget before activity does.
    let lines = palette.entry_lines(0, 13, 4);
    assert_eq!(
        texts(&lines),
        ["● codex", "  working", "  [42]", "  one two…"]
    );
    palette
        .agent_presentations
        .get_mut("agent:0")
        .unwrap()
        .activity = "三四 e\u{301} abcdefghijklmnop".into();
    let lines = palette.entry_lines(0, 22, 4);
    assert_eq!(
        texts(&lines),
        [
            "● codex  working",
            "  [42]",
            "  三四 e\u{301}",
            "  abcdefghijklmnop"
        ]
    );
    let presentation = palette.agent_presentations.get_mut("agent:0").unwrap();
    presentation.provider = "provider with a name longer than four whole display lines".into();
    let lines = palette.entry_lines(0, 13, 4);
    assert_eq!(lines.len(), 4);
    assert!(lines[3].text.ends_with('…'));
    assert!(!lines.iter().any(|line| line.text.contains("one")));
    for width in 0..30 {
        let lines = palette.entry_lines(0, width, 4);
        assert!(lines.len() <= 4);
        assert!(
            lines.iter().all(|line| line.text.width() <= width),
            "width={width}: {lines:?}"
        );
        assert!(lines
            .iter()
            .all(|line| line.styles.len() == line.text.chars().count()));
    }
}

#[test]
fn live_agent_continuations_share_click_navigation_scroll_and_refresh_identity() {
    for picker in [false, true] {
        let mut palette = palette(5);
        if !picker {
            palette.back();
            palette.set_query("alias".into());
        }
        for selected in 0..5 {
            palette.select_id(&format!("agent:{selected}"));
            for cols in 0..40 {
                for rows in [1, 4, 8, 12, 24, 40] {
                    let layout = palette.layout(Viewport {
                        cols,
                        rows,
                        ..Default::default()
                    });
                    let chunks = palette.chunks(&layout, &Style::default());
                    assert!(chunks.iter().all(|chunk| chunk.width() == layout.rect.cols));
                    if layout.rect.is_empty() {
                        continue;
                    }
                    if layout.options_rows > 0 {
                        assert!(
                            layout
                                .result_rows
                                .iter()
                                .any(|row| palette.row_selected(row)),
                            "picker={picker}, {cols}x{rows}"
                        );
                    }
                    for (row, result) in layout.result_rows.iter().enumerate() {
                        if let ResultRow::Entry(index, _) = result {
                            let painted =
                                &chunks[layout.options_y + row - layout.rect.y].terminal_characters;
                            if *index == selected
                                || layout.rect.cols.saturating_sub(2 * layout.border) >= 2
                            {
                                assert_eq!(
                                    painted[layout.border].styles.background,
                                    if *index == selected {
                                        Some(Style::default().colors.text_selected.background.into())
                                    } else {
                                        Some(AnsiCode::Reset)
                                    },
                                    "picker={picker}, {cols}x{rows}, selected={selected}, row={index}"
                                );
                            }
                            palette.click(
                                &layout,
                                Position::new(
                                    (layout.options_y + row) as i32,
                                    (layout.rect.x + layout.border) as u16,
                                ),
                            );
                            let id = format!("agent:{index}");
                            assert_eq!(palette.selected_entry().unwrap().id, id);
                            assert_eq!(
                                palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                                Outcome::Activate(id.clone())
                            );
                            assert_eq!(
                                palette.handle_key(
                                    KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier()
                                ),
                                Outcome::FocusAgent(id)
                            );
                        }
                    }
                    palette.select_id(&format!("agent:{selected}"));
                }
            }
        }
        palette.select_id("agent:2");
        let mut entries = palette.native_entries.clone();
        entries.swap(0, 2);
        palette
            .agent_presentations
            .get_mut("agent:2")
            .unwrap()
            .activity = "refreshed activity that wraps onto several display lines".into();
        palette.set_entries(entries);
        assert_eq!(palette.selected_entry().unwrap().id, "agent:2");
        let layout = palette.layout(Viewport {
            cols: 30,
            rows: 20,
            ..Default::default()
        });
        assert!(layout
            .result_rows
            .iter()
            .any(|row| palette.row_selected(row)));
        palette.handle_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(palette.selected_entry().unwrap().id, "agent:1");
        palette.handle_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(palette.selected_entry().unwrap().id, "agent:2");
        palette.handle_key(KeyWithModifier::new(BareKey::PageDown));
        assert_ne!(palette.selected_entry().unwrap().id, "agent:2");
    }
}
