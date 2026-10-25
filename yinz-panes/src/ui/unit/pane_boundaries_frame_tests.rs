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
        String::new(),
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
