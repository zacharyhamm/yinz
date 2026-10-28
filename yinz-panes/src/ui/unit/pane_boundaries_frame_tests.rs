use super::*;
use yinz_types::pane_size::{Offset, Viewport};
use yinz_types::style::Style;

fn pane_frame_with(mouse_scroll_resize: bool, is_floating: bool, cols: usize) -> PaneFrame {
    PaneFrame::new(
        Viewport {
            x: 0,
            y: 0,
            cols,
            rows: 10,
        },
        (0, 0),
        "pane".into(),
        FrameParams {
            focused_client: None,
            is_main_client: true,
            other_focused_clients: vec![],
            style: Style::default(),
            color: None,
            other_cursors_exist_in_session: false,
            pane_is_stacked_over: false,
            pane_is_stacked_under: false,
            pane_is_stacked: false,
            should_draw_pane_frames: true,
            pane_is_floating: is_floating,
            show_pinned_indication: true,
            show_scroll_indication: true,
            content_offset: Offset::default(),
            mouse_is_hovering_over_pane: false,
            pane_is_selectable: true,
            show_help_text: true,
            highlight_tooltip: None,
            omit_title: false,
            frame_geom_override: None,
            stack_list_entry: None,
            blank_title: false,
            same_worktree: false,
            mouse_scroll_resize,
            dimmed: false,
            guest_choice_indicator: None,
            opening_ripple: None,
        },
    )
}

#[test]
fn subtitle_moves_the_floating_pin_hit_target_with_the_title() {
    let mut frame = pane_frame_with(false, true, 80);
    frame.title = "pane".into();
    frame.content_offset = Offset::frame(1);
    frame.subtitle = Some(("review · running".into(), 1));
    assert!(frame.clicked_on_pinned(Position::new(-2, 75)));
    assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
}

#[test]
fn pin_hit_target_matches_the_visible_checkbox() {
    for cols in [10, 20, 40, 80] {
        for show_pin in [false, true] {
            for title in ["", "pane 界", "a title that takes all available space"] {
                let mut frame = pane_frame_with(false, true, cols);
                frame.title = title.into();
                frame.content_offset = Offset::frame(1);
                frame.scroll_position = (3, 20);
                frame.show_pinned_indication = show_pin;
                let rendered = characters_to_string(&frame.render_title().unwrap());
                let checkbox = rendered.rfind(" PIN [").map(|start| {
                    let column = rendered[..start].width() + 5 - frame.content_offset.left;
                    column..column + 3
                });
                for column in 0..cols {
                    assert_eq!(
                        frame.clicked_on_pinned(Position::new(-1, column as u16)),
                        checkbox
                            .as_ref()
                            .is_some_and(|range| range.contains(&column)),
                        "column {column} of {rendered:?}"
                    );
                }
            }
        }
    }
    let mut frame = pane_frame_with(false, true, 80);
    frame.title = "pane".into();
    frame.content_offset = Offset::frame(1);
    frame.is_selectable = false;
    assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
    frame.is_selectable = true;
    frame.omit_title = true;
    assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
}

#[test]
fn narrow_agent_pin_hit_target_matches_multiplayer_title_fallback() {
    let mut frame = pane_frame_with(false, true, 21);
    frame.title = "[1] x".into();
    frame.content_offset = Offset::frame(1);
    frame.other_focused_clients = vec![None];
    frame.agent_title.state = Some(yinz_types::agent::AgentState::Busy);
    let rendered = characters_to_string(&frame.render_title().unwrap());
    let pin = rendered.rfind(" PIN [").expect("pin must be visible");
    let start = rendered[..pin].width() + 5 - frame.content_offset.left;
    for column in 0..frame.geom.cols {
        assert_eq!(
            frame.clicked_on_pinned(Position::new(-1, column as u16)),
            (start..start + 3).contains(&column),
            "column {column} of {rendered:?}"
        );
    }
}

#[test]
fn subtitle_fits_unicode_columns_without_touching_content_or_borders() {
    for full in [false, true] {
        for floating in [false, true] {
            for width in 1..12 {
                let mut frame = pane_frame_with(false, floating, width + 2);
                frame.should_draw_pane_frames = full;
                frame.content_offset = if full {
                    Offset::frame(1)
                } else {
                    Offset::shift_right_top_and_bottom(0, 1, 0)
                };
                frame.subtitle = Some(("界a\u{301} · 手順/検証 · running".into(), 1));
                let (chunks, _) = frame.render().unwrap();
                let subtitle = chunks.last().unwrap();
                assert_eq!(subtitle.x, frame.content_offset.left);
                assert_eq!(subtitle.y, 1);
                assert_eq!(
                    subtitle
                        .terminal_characters
                        .iter()
                        .map(|c| c.width())
                        .sum::<usize>(),
                    frame.geom.cols - frame.content_offset.left - frame.content_offset.right
                );
                frame.stack_list_entry = Some(StackListEntry {
                    width,
                    label: "collapsed".into(),
                    is_selected: false,
                    is_emphasized: false,
                    stack_is_focused: false,
                    border_color: None,
                });
                assert_eq!(
                    frame.render().unwrap().0.len(),
                    1,
                    "collapsed entries only render their title"
                );
            }
        }
    }
}

fn characters_to_string(chars: &[TerminalCharacter]) -> String {
    chars.iter().map(|c| c.character).collect()
}

fn right_side_indicators(show_pin: bool, show_scroll: bool) -> String {
    let mut frame = pane_frame_with(false, true, 80);
    frame.scroll_position = (1, 2);
    frame.show_pinned_indication = show_pin;
    frame.show_scroll_indication = show_scroll;
    frame
        .render_title_right_side(80)
        .map(|(characters, _)| characters_to_string(&characters))
        .unwrap_or_default()
}

#[test]
fn overlay_and_modal_frame_indicators() {
    let floating = right_side_indicators(true, true);
    assert!(floating.contains("PIN"));
    assert!(floating.contains("SCROLL"));

    let overlay = right_side_indicators(false, true);
    assert!(!overlay.contains("PIN"));
    assert!(overlay.contains("SCROLL"));

    assert!(right_side_indicators(false, false).is_empty());

    let mut modal_title = pane_frame_with(false, true, 80);
    modal_title.scroll_position = (1, 2);
    modal_title.show_scroll_indication = false;
    assert!(modal_title.bracketed_scroll_indicator(80).is_none());
}

#[test]
fn a_pane_without_scroll_or_pin_indications_has_no_right_title() {
    let mut frame = pane_frame_with(false, false, 80);
    frame.show_scroll_indication = false;
    frame.show_pinned_indication = false;
    assert!(frame.render_title_right_side(80).is_none());
}

#[test]
fn help_text_full_includes_ctrl_scroll_when_enabled_for_tiled_pane() {
    let frame = pane_frame_with(true, false, 80);
    let (chars, _) = frame.help_text_version_full(80).unwrap();
    let text = characters_to_string(&chars);
    assert!(text.contains("Ctrl <MouseScroll>"));
    assert!(text.contains("<drag borders>"));
}

#[test]
fn help_text_full_omits_ctrl_scroll_when_disabled_for_tiled_pane() {
    let frame = pane_frame_with(false, false, 80);
    let (chars, _) = frame.help_text_version_full(80).unwrap();
    let text = characters_to_string(&chars);
    assert!(!text.contains("MouseScroll"));
    assert!(text.contains("<drag borders> to resize"));
}

#[test]
fn help_text_full_includes_ctrl_scroll_when_enabled_for_floating_pane() {
    let frame = pane_frame_with(true, true, 80);
    let (chars, _) = frame.help_text_version_full(80).unwrap();
    let text = characters_to_string(&chars);
    assert!(text.contains("Ctrl <MouseScroll>"));
    assert!(text.contains("Ctrl <drag borders>"));
}

#[test]
fn help_text_full_omits_ctrl_scroll_when_disabled_for_floating_pane() {
    let frame = pane_frame_with(false, true, 80);
    let (chars, _) = frame.help_text_version_full(80).unwrap();
    let text = characters_to_string(&chars);
    assert!(!text.contains("MouseScroll"));
    assert!(text.contains("Ctrl <drag borders> to resize"));
}

#[test]
fn help_text_medium_omits_ctrl_scroll_when_disabled() {
    let frame = pane_frame_with(false, false, 40);
    let (chars, _) = frame.help_text_version_medium(40).unwrap();
    let text = characters_to_string(&chars);
    assert!(!text.contains("MouseScroll"));
    assert!(text.contains("<drag borders> resize"));
}

#[test]
fn help_text_short_omits_ctrl_scroll_when_disabled() {
    let frame = pane_frame_with(false, false, 20);
    let (chars, _) = frame.help_text_version_short(20).unwrap();
    let text = characters_to_string(&chars);
    assert!(!text.contains("MouseScroll"));
    assert!(text.contains("<drag borders>"));
}
#[test]
fn opening_ripple_only_changes_border_strokes_and_handles_narrow_frames() {
    let start = std::time::Instant::now();
    for cols in [2, 3, 5, 12, 80, 120] {
        let mut frame = pane_frame_with(true, true, cols);
        frame.title = "title─∿".into();
        frame.highlight_tooltip = Some("hint─text".into());
        let normal = frame.render().unwrap().0;
        for ms in [0, 80, 160, 320, 449, 450, 900] {
            frame.opening_ripple =
                OpeningRipple::sample(start, start + std::time::Duration::from_millis(ms), 0, cols);
            let animated = frame.render().unwrap().0;
            assert_eq!(normal.len(), animated.len());
            for (plain, wave) in normal.iter().zip(&animated) {
                assert_eq!((plain.x, plain.y), (wave.x, wave.y));
                assert_eq!(
                    plain.terminal_characters.len(),
                    wave.terminal_characters.len()
                );
                for (before, after) in plain
                    .terminal_characters
                    .iter()
                    .zip(&wave.terminal_characters)
                {
                    assert_eq!(before.width(), after.width());
                    if before.character != '─'
                        || (plain.y != 0 && plain.y != 9)
                        || ms == 0
                        || ms >= 450
                    {
                        assert_eq!(before, after);
                    }
                }
            }
            if cols >= 80 {
                assert!(characters_to_string(&animated[0].terminal_characters).contains("title─∿"));
                assert!(animated
                    .iter()
                    .any(|chunk| characters_to_string(&chunk.terminal_characters)
                        .contains("hint─text")));
            }
        }
    }
    for glyph in ['∿', '∽'] {
        assert_eq!(glyph.width(), Some(1));
    }
}

#[test]
fn opening_ripple_halves_sample_the_same_travelling_wave() {
    let start = std::time::Instant::now();
    let sample = |ms| {
        OpeningRipple::sample(start, start + std::time::Duration::from_millis(ms), 3, 70).unwrap()
    };
    let first = sample(80);
    let later = sample(300);
    let active_columns = |ripple: OpeningRipple| {
        (3..73)
            .filter(|x| ripple.stroke(*x) != "─")
            .collect::<Vec<_>>()
    };
    assert!(active_columns(first).last().unwrap() < active_columns(later).first().unwrap());
    let mut left = pane_frame_with(false, true, 35);
    left.geom.x = 3;
    let mut right = pane_frame_with(false, true, 35);
    right.geom.x = 38;
    left.opening_ripple = Some(later);
    right.opening_ripple = Some(later);
    for x in 3..73 {
        assert_eq!(left.horizontal(x), right.horizontal(x));
    }
    assert!(
        OpeningRipple::sample(start, start + std::time::Duration::from_millis(450), 3, 70)
            .is_none()
    );
    assert_eq!(
        OpeningRipple::sample(start, start, 0, 0).unwrap().stroke(0),
        "─"
    );
}

#[test]
fn focus_emoji_is_part_of_the_title_and_fits_available_columns() {
    let mut frame = pane_frame_with(false, false, 80);
    assert_eq!(frame.title, "👇 pane");
    for full in [false, true] {
        frame.should_draw_pane_frames = full;
        for cols in 7..80 {
            frame.geom.cols = cols;
            let title = frame.render_one_line_title().unwrap();
            assert_eq!(title.iter().map(|c| c.width()).sum::<usize>(), cols);
        }
    }
    frame.stack_list_entry = Some(StackListEntry {
        width: 10,
        label: "pane".into(),
        is_selected: true,
        is_emphasized: false,
        stack_is_focused: true,
        border_color: None,
    });
    for focused in [false, true] {
        frame.is_main_client = focused;
        for width in 0..20 {
            let indicator = frame.bracketed_focus_indicator(width);
            assert_eq!(indicator.is_some(), focused && width >= 4);
            if let Some((characters, length)) = indicator {
                assert_eq!(characters_to_string(&characters), " 👇 ");
                assert_eq!(length, characters.iter().map(|c| c.width()).sum::<usize>());
            }
        }
    }
}

#[test]
fn agent_title_marker_and_flash_colors_leave_the_border_unchanged() {
    use crate::ui::agent_title::AgentTitle;
    use yinz_types::agent::AgentState;
    for floating in [false, true] {
        for focused in [false, true] {
            for state in [
                AgentState::Busy,
                AgentState::Idle,
                AgentState::Waiting,
                AgentState::Exited,
            ] {
                let mut frame = pane_frame_with(false, floating, 80);
                frame.focused_client = focused.then_some(ClientId::new(1));
                frame.title = "🔔 👯 [7] work [!]".into();
                frame.color = Some(PaletteColor::EightBit(99));
                frame.agent_title = AgentTitle {
                    state: Some(state),
                    hook_flash: false,
                };
                let colors = frame.style.colors.text_unselected;
                let (glyph, _, emphasis) = crate::ui::agent_title::status(state);
                let before = frame.render_title().unwrap();
                let marker = before
                    .iter()
                    .find(|ch| ch.character.to_string() == glyph)
                    .unwrap();
                assert_eq!(
                    marker.styles.foreground,
                    Some(crate::ui::agent_title::color(colors, emphasis).into())
                );
                let saved_title = frame.title.clone();
                let text = characters_to_string(&before);
                assert!(text.contains(&saved_title));
                frame.agent_title.hook_flash = true;
                let after = frame.render_title().unwrap();
                assert_eq!(frame.title, saved_title);
                assert_eq!(before.first(), after.first());
                assert_eq!(before.last(), after.last());
                let highlight = Some(colors.emphasis_0.into());
                for ch in after
                    .iter()
                    .filter(|ch| ch.character.to_string() == glyph || ch.character == 'w')
                {
                    assert_eq!(ch.styles.foreground, highlight);
                }
                frame.agent_title.state = None;
                let cleared = characters_to_string(&frame.render_title().unwrap());
                assert_eq!(
                    cleared.matches(glyph).count(),
                    saved_title.matches(glyph).count()
                );
                assert!(cleared.contains("work"));
            }
        }
    }
}

#[test]
fn agent_title_marker_survives_narrow_frames_and_participates_in_cache_equality() {
    use crate::ui::agent_title::AgentTitle;
    use yinz_types::agent::AgentState;
    for full in [false, true] {
        for width in 1..40 {
            let mut frame = pane_frame_with(false, false, width + 2);
            frame.should_draw_pane_frames = full;
            frame.title = "long title with 界 and a tail".into();
            frame.agent_title = AgentTitle {
                state: Some(AgentState::Busy),
                hook_flash: false,
            };
            let (chars, length) = if full {
                frame.render_title_left_side(width)
            } else {
                frame.bracketed_pane_title(width)
            }
            .unwrap();
            assert!(length <= width);
            assert_eq!(
                chars.iter().map(TerminalCharacter::width).sum::<usize>(),
                length
            );
            assert!(characters_to_string(&chars).contains('●'));
        }
    }
    for width in 1..40 {
        let mut frame = pane_frame_with(false, false, width + 2);
        frame.agent_title.state = Some(AgentState::Exited);
        frame.add_exit_status(Some(1));
        frame.pane_is_stacked_under = true;
        assert!(characters_to_string(&frame.render_title().unwrap()).contains('×'));
        let (characters, length) = frame.bracketed_pane_title(width).unwrap();
        assert!(length <= width);
        assert!(characters_to_string(&characters).contains('×'));
    }
    let frame = pane_frame_with(false, false, 80);
    let mut changed = pane_frame_with(false, false, 80);
    assert!(frame == changed);
    changed.agent_title.state = Some(AgentState::Idle);
    assert!(frame != changed);
    changed.agent_title.state = None;
    changed.agent_title.hook_flash = true;
    assert!(frame != changed);
}

#[test]
fn agent_title_stack_list_marker_survives_truncation_and_flash_preserves_borders() {
    use crate::ui::agent_title::AgentTitle;
    use yinz_types::agent::AgentState;
    let mut frame = pane_frame_with(false, false, 40);
    frame.agent_title = AgentTitle {
        state: Some(AgentState::Busy),
        hook_flash: false,
    };
    let mut entry = StackListEntry {
        width: 24,
        label: "agent with a long title".into(),
        is_selected: false,
        is_emphasized: false,
        stack_is_focused: false,
        border_color: Some(PaletteColor::EightBit(99)),
    };
    for width in 1..30 {
        entry.width = width;
        frame.agent_title.hook_flash = false;
        let before = frame.render_stack_list_entry(&entry);
        assert_eq!(
            before.iter().map(TerminalCharacter::width).sum::<usize>(),
            40
        );
        assert!(characters_to_string(&before).contains('●'));
        frame.agent_title.hook_flash = true;
        let after = frame.render_stack_list_entry(&entry);
        assert_eq!(characters_to_string(&before), characters_to_string(&after));
        for (old, new) in before.iter().zip(&after) {
            if new.character == '│' {
                assert_eq!(old, new);
            }
            if new.character == '●' || new.character == 'a' {
                assert_eq!(
                    new.styles.foreground,
                    Some(frame.style.colors.text_unselected.emphasis_0.into())
                );
            }
        }
    }
}

#[test]
fn focused_idle_agent_title_marker_uses_normal_background_palette() {
    for (name, source) in [
        (
            "ansi",
            include_str!("../../../../yinz-config/assets/themes/ansi.kdl"),
        ),
        (
            "catppuccin-macchiato",
            include_str!("../../../../yinz-config/assets/themes/catppuccin-macchiato.kdl"),
        ),
    ] {
        let themes = yinz_config::theme::Themes::from_string(source, false).unwrap();
        let theme = themes.get_theme(name).unwrap();
        for focused in [false, true] {
            for full in [false, true] {
                let mut frame = pane_frame_with(false, false, 40);
                frame.style.colors = theme.palette;
                frame.focused_client = focused.then_some(ClientId::new(1));
                frame.should_draw_pane_frames = full;
                frame.agent_title.state = Some(yinz_types::agent::AgentState::Idle);
                let characters = frame.render_one_line_title().unwrap();
                let marker = characters.iter().find(|ch| ch.character == '○').unwrap();
                assert_eq!(
                    marker.styles.foreground,
                    Some(frame.style.colors.text_unselected.base.into())
                );
                assert_ne!(
                    marker.styles.foreground,
                    Some(frame.style.colors.text_unselected.background.into())
                );
            }
        }
    }
}
