use super::*;
use crate::testing::painter::Painter;

fn screen(paint: impl FnOnce(&mut Painter)) -> crate::emulation::terminal::TerminalState {
    Painter::state(3, 10, paint)
}

#[test]
fn a_selection_on_one_row_takes_the_cells_between_its_ends() {
    let state = screen(|p| p.text(0, 0, "hello world"));
    let selection = Selection::at(0, 2).extended_to(0, 6);
    assert_eq!(selection.text(state.screen()), "llo w");
}

#[test]
fn a_selection_dragged_backwards_reads_the_same_as_forwards() {
    let state = screen(|p| p.text(0, 0, "hello world"));
    assert_eq!(
        Selection::at(0, 6).extended_to(0, 2).text(state.screen()),
        "llo w"
    );
}

#[test]
fn a_selection_over_rows_flows_like_text_and_drops_trailing_blanks() {
    let state = screen(|p| {
        p.text(0, 0, "Tab #1  x");
        p.text(1, 0, "│ ls   │");
        p.text(2, 0, "status");
    });
    let selection = Selection::at(0, 4).extended_to(2, 2);
    assert_eq!(selection.text(state.screen()), "#1  x\n│ ls   │\nsta");
}

#[test]
fn the_right_half_of_a_wide_character_is_not_copied_twice() {
    let state = screen(|p| {
        p.text(0, 0, "a");
        p.wide(0, 1, '漢');
        p.text(0, 3, "b");
    });
    assert_eq!(
        Selection::at(0, 0).extended_to(0, 3).text(state.screen()),
        "a漢b"
    );
    assert_eq!(
        Selection::at(0, 2).extended_to(0, 3).text(state.screen()),
        "漢b"
    );
}

#[test]
fn a_selection_that_never_moved_covers_and_copies_nothing() {
    let state = screen(|p| p.text(0, 0, "hello"));
    let selection = Selection::at(0, 1);
    assert!(selection.is_empty());
    assert!(!selection.covers(0, 1));
    assert_eq!(selection.text(state.screen()), "");
}

#[test]
fn coverage_follows_the_flow_of_text() {
    let selection = Selection::at(0, 5).extended_to(2, 1);
    assert!(!selection.covers(0, 4));
    assert!(selection.covers(0, 5));
    assert!(selection.covers(0, 9));
    assert!(selection.covers(1, 0));
    assert!(selection.covers(2, 1));
    assert!(!selection.covers(2, 2));
    assert_eq!(selection.rows(), 0..=2);
}

#[test]
fn a_selection_beyond_a_shrunken_screen_reads_only_what_is_there() {
    let state = screen(|p| p.text(2, 0, "end"));
    assert_eq!(
        Selection::at(2, 0).extended_to(9, 40).text(state.screen()),
        "end"
    );
}

const FAMILY: [(&str, usize); 3] = [
    ("\u{1f468}\u{200d}", 2),
    ("\u{1f469}\u{200d}", 2),
    ("\u{1f467}", 2),
];
const FAMILY_TEXT: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";

#[test]
fn a_copied_cluster_keeps_every_code_point() {
    let state = screen(|p| {
        p.text(0, 0, "a");
        p.cluster(0, 1, "e\u{301}", 1);
        p.cluster(0, 2, "\u{26a0}\u{fe0f}", 1);
        p.text(0, 3, "b");
    });
    assert_eq!(
        Selection::at(0, 0).extended_to(0, 3).text(state.screen()),
        "ae\u{301}\u{26a0}\u{fe0f}b"
    );
}

#[test]
fn a_copied_group_is_its_whole_text_once() {
    let state = screen(|p| {
        p.text(0, 0, "a");
        p.group(0, 1, &FAMILY);
        p.text(0, 7, "b");
    });
    assert_eq!(
        Selection::at(0, 0).extended_to(0, 7).text(state.screen()),
        format!("a{}b", FAMILY_TEXT)
    );
}

#[test]
fn a_selection_edge_inside_a_group_takes_the_whole_group() {
    let state = screen(|p| {
        p.text(0, 0, "a");
        p.group(0, 1, &FAMILY);
        p.text(0, 7, "b");
    });
    assert_eq!(
        Selection::at(0, 4).extended_to(0, 7).text(state.screen()),
        format!("{}b", FAMILY_TEXT)
    );
    assert_eq!(
        Selection::at(0, 0).extended_to(0, 3).text(state.screen()),
        format!("a{}", FAMILY_TEXT)
    );
}

#[test]
fn a_mode_2027_cluster_cell_is_copied_once_with_its_tail_skipped() {
    let state = screen(|p| {
        p.cluster(0, 0, FAMILY_TEXT, 2);
        p.text(0, 2, "x");
    });
    assert_eq!(
        Selection::at(0, 0).extended_to(0, 2).text(state.screen()),
        format!("{}x", FAMILY_TEXT)
    );
}

#[test]
fn a_width_three_cell_is_copied_once() {
    let state = screen(|p| {
        p.cluster(0, 0, "\u{17d8}", 3);
        p.text(0, 3, "y");
    });
    assert_eq!(
        Selection::at(0, 2).extended_to(0, 3).text(state.screen()),
        "\u{17d8}y"
    );
}

fn wrapped_frame(rows: usize, cols: usize, text: &str) -> Vec<u8> {
    assert!(text.is_ascii());
    Painter::frame(rows, cols, |p| {
        p.full_repaint();
        for (row, chunk) in text.as_bytes().chunks(cols).enumerate() {
            assert!(row < rows);
            p.text(row, 0, std::str::from_utf8(chunk).unwrap());
        }
    })
}

fn selected(state: &crate::emulation::terminal::TerminalState, range: Selection) -> LocalSelection {
    let mut local = LocalSelection::default();
    local.start(range.anchor.0, range.anchor.1);
    local.extend(range.head.0, range.head.1);
    assert_eq!(local.text(state.screen()), Some(range.text(state.screen())));
    local
}

#[test]
fn resize_repaint_reanchors_right_edge_selection_after_an_old_grid_frame() {
    let text = format!("{}distinctive ABCD suffix", "x".repeat(58));
    let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
    state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
    // The exact review reproduction: zero-based columns 70 through 73.
    assert_eq!(&text[70..74], "ABCD");
    let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
    local.resizing(&state, 3, 60);
    local
        .apply_frame(&mut state, &wrapped_frame(3, 80, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(0, 70).extended_to(0, 73)));
    local
        .apply_frame(&mut state, &wrapped_frame(3, 60, &text))
        .unwrap();
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    assert_eq!(local.range(), Some(Selection::at(1, 10).extended_to(1, 13)));
    local.resizing(&state, 4, 80);
    local
        .apply_frame(&mut state, &wrapped_frame(4, 80, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(0, 70).extended_to(0, 73)));
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
}

#[test]
fn wrap_boundary_reverse_drag_spaces_and_original_newlines_survive() {
    let text = format!("{}AB  CD suffix", "x".repeat(58));
    for reverse in [false, true] {
        let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
        state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
        let range = if reverse {
            Selection::at(0, 63).extended_to(0, 58)
        } else {
            Selection::at(0, 58).extended_to(0, 63)
        };
        let mut local = selected(&state, range);
        local.resizing(&state, 3, 60);
        local
            .apply_frame(&mut state, &wrapped_frame(3, 60, &text))
            .unwrap();
        let forward = Selection::at(0, 58).extended_to(1, 3);
        assert_eq!(
            local.range(),
            Some(if reverse {
                Selection {
                    anchor: forward.head,
                    head: forward.anchor,
                }
            } else {
                forward
            })
        );
        assert_eq!(local.text(state.screen()).as_deref(), Some("AB  CD"));
        local.resizing(&state, 3, 80);
        local
            .apply_frame(&mut state, &wrapped_frame(3, 80, &text))
            .unwrap();
        assert_eq!(local.text(state.screen()).as_deref(), Some("AB  CD"));
    }
    let mut state = Painter::state(3, 10, |p| {
        p.text(0, 0, "0123456789");
        p.text(1, 0, "abcdefghij");
    });
    let mut local = selected(&state, Selection::at(0, 8).extended_to(1, 2));
    local.resizing(&state, 5, 5);
    local
        .apply_frame(&mut state, &wrapped_frame(5, 5, "0123456789abcdefghij"))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(1, 3).extended_to(2, 2)));
    assert_eq!(local.text(state.screen()).as_deref(), Some("89\nabc"));
}

#[test]
fn contextual_repeats_map_but_ambiguous_or_missing_content_only_keeps_copy() {
    let text = "left-one SAME right-one left-two SAME right-two";
    let mut state = Painter::state(3, 80, |p| p.text(0, 0, text));
    let mut local = selected(&state, Selection::at(0, 9).extended_to(0, 12));
    local.resizing(&state, 3, 20);
    local
        .apply_frame(&mut state, &wrapped_frame(3, 20, text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(0, 9).extended_to(0, 12)));
    for replacement in ["x".repeat(120), "removed".to_owned()] {
        let mut state = Painter::state(3, 80, |p| p.text(0, 0, &"x".repeat(80)));
        let mut local = selected(&state, Selection::at(0, 30).extended_to(0, 33));
        local.resizing(&state, 3, 60);
        local
            .apply_frame(&mut state, &wrapped_frame(3, 60, &replacement))
            .unwrap();
        assert_eq!(local.range(), None);
        assert_eq!(local.text(state.screen()).as_deref(), Some("xxxx"));
    }
}

#[test]
fn unicode_units_are_relocated_whole_with_copy_bytes_intact() {
    let mut state = Painter::state(3, 40, |p| {
        p.text(0, 18, "prefix ");
        p.wide(0, 25, '漢');
        p.cluster(0, 27, "e\u{301}", 1);
        p.group(0, 28, &FAMILY);
        p.text(0, 34, " end");
    });
    let mut local = selected(&state, Selection::at(0, 26).extended_to(0, 31));
    let expected = format!("漢e\u{301}{FAMILY_TEXT}");
    local.resizing(&state, 4, 25);
    let frame = Painter::frame(4, 25, |p| {
        p.full_repaint();
        p.text(0, 18, "prefix ");
        p.wide(1, 0, '漢');
        p.cluster(1, 2, "e\u{301}", 1);
        p.group(1, 3, &FAMILY);
        p.text(1, 9, " end");
    });
    local.apply_frame(&mut state, &frame).unwrap();
    assert_eq!(local.text(state.screen()), Some(expected));
    assert_eq!(local.range(), Some(Selection::at(1, 0).extended_to(1, 8)));
}

#[test]
fn coalesced_resize_keeps_capture_and_rejected_frame_does_not_commit_mapping() {
    let text = format!("{}ABCD suffix", "x".repeat(70));
    let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
    state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
    let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
    local.resizing(&state, 3, 70);
    local.resizing(&state, 4, 60);
    let malformed = Painter::frame(4, 60, |p| p.text(1, 10, "wrong"));
    assert!(local.apply_frame(&mut state, &malformed).is_err());
    assert_eq!(state.size().cols, 80);
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    assert_eq!(local.range(), Some(Selection::at(0, 70).extended_to(0, 73)));
    local
        .apply_frame(&mut state, &wrapped_frame(3, 70, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(1, 0).extended_to(1, 3)));
    local
        .apply_frame(&mut state, &wrapped_frame(4, 60, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(1, 10).extended_to(1, 13)));
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
}

#[test]
fn cancellation_and_new_selection_cannot_be_resurrected_by_a_pending_repaint() {
    for new_selection in [false, true] {
        let mut state = Painter::state(3, 80, |p| p.text(0, 70, "ABCD"));
        let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
        local.resizing(&state, 3, 60);
        // The production clear is shared by Escape, typing and session switch.
        local.clear();
        if new_selection {
            local.start(0, 0);
            local.extend(0, 3);
        }
        local
            .apply_frame(&mut state, &wrapped_frame(3, 60, "new! other content"))
            .unwrap();
        assert_eq!(
            local.text(state.screen()).as_deref(),
            new_selection.then_some("new!")
        );
        assert_eq!(
            local.range(),
            new_selection.then_some(Selection::at(0, 0).extended_to(0, 3))
        );
    }
}

#[test]
fn unchanged_grid_does_not_freeze_an_ordinary_selection() {
    let mut state = Painter::state(3, 80, |p| p.text(0, 0, "old!"));
    let mut local = selected(&state, Selection::at(0, 0).extended_to(0, 3));
    local.resizing(&state, 3, 80);
    local
        .apply_frame(&mut state, &wrapped_frame(3, 80, "new!"))
        .unwrap();
    assert_eq!(local.text(state.screen()).as_deref(), Some("new!"));
    assert!(local.captured.is_none());
}

#[test]
fn pane_topology_chrome_and_cross_pane_ranges_do_not_guess_highlights() {
    use yinz_types::structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE};
    for changed_topology in [false, true] {
        let pane = PaneRect {
            cols: 80,
            rows: 3,
            flags: PANE_SELECTABLE,
            ..Default::default()
        };
        let mut state = Painter::state(3, 80, |p| {
            p.text(0, 70, "ABCD");
            p.geometry(&GeometryRecord { panes: vec![pane] });
        });
        let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
        local.resizing(&state, 3, 60);
        let resized = PaneRect { cols: 60, ..pane };
        let frame = Painter::frame(3, 60, |p| {
            p.full_repaint();
            p.text(1, 10, "ABCD");
            p.geometry(&GeometryRecord {
                panes: if changed_topology {
                    vec![resized, resized]
                } else {
                    vec![resized]
                },
            });
        });
        local.apply_frame(&mut state, &frame).unwrap();
        // The original context is blank, and this fixture preserves it.
        assert_eq!(local.range().is_none(), changed_topology);
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    }
    for panes in [
        vec![
            PaneRect {
                cols: 40,
                rows: 3,
                ..Default::default()
            };
            2
        ],
        vec![PaneRect {
            cols: 80,
            rows: 3,
            top: 1,
            ..Default::default()
        }],
    ] {
        let mut state = Painter::state(3, 80, |p| {
            p.text(0, 0, "chrome");
            p.geometry(&GeometryRecord { panes });
        });
        let mut local = selected(&state, Selection::at(0, 0).extended_to(0, 3));
        local.resizing(&state, 3, 60);
        local
            .apply_frame(&mut state, &wrapped_frame(3, 60, "chrome"))
            .unwrap();
        assert_eq!(local.range(), None);
        assert_eq!(local.text(state.screen()).as_deref(), Some("chro"));
    }
}

#[test]
fn returning_to_original_target_still_handles_late_intermediate_resize_frames() {
    let text = format!("{}ABCD end", "x".repeat(70));
    let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
    state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
    let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
    local.resizing(&state, 3, 60);
    local.resizing(&state, 3, 80);
    for cols in [80, 60, 80] {
        local
            .apply_frame(&mut state, &wrapped_frame(3, cols, &text))
            .unwrap();
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
        assert_eq!(
            local.range(),
            Some(Selection::at(70 / cols, 70 % cols).extended_to(73 / cols, 73 % cols))
        );
    }
}

#[test]
fn surviving_unique_terminal_pane_reanchors_beside_plugin_chrome() {
    use yinz_types::structured_render::{
        GeometryRecord, PaneRect, PANE_FOCUSED, PANE_PLUGIN_UI, PANE_SELECTABLE,
    };
    let terminal = PaneRect {
        y: 1,
        cols: 80,
        rows: 3,
        flags: PANE_SELECTABLE | PANE_FOCUSED,
        ..Default::default()
    };
    let chrome = PaneRect {
        cols: 80,
        rows: 1,
        flags: PANE_PLUGIN_UI,
        ..Default::default()
    };
    let mut state = Painter::state(4, 80, |p| {
        p.text(1, 70, "ABCD");
        p.geometry(&GeometryRecord {
            panes: vec![chrome, terminal],
        });
    });
    let mut local = selected(&state, Selection::at(1, 70).extended_to(1, 73));
    local.resizing(&state, 4, 60);
    let frame = Painter::frame(4, 60, |p| {
        p.full_repaint();
        p.text(2, 10, "ABCD");
        p.geometry(&GeometryRecord {
            panes: vec![
                PaneRect { cols: 60, ..chrome },
                PaneRect {
                    cols: 60,
                    ..terminal
                },
            ],
        });
    });
    local.apply_frame(&mut state, &frame).unwrap();
    assert_eq!(local.range(), Some(Selection::at(2, 10).extended_to(2, 13)));
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    // Location proof uses the resized pane geometry rather than the original
    // 80-column geometry, and harmless output must retain it.
    local.apply_frame(&mut state, &frame).unwrap();
    assert_eq!(local.range(), Some(Selection::at(2, 10).extended_to(2, 13)));
    let changed_layout = Painter::frame(4, 60, |p| {
        p.geometry(&GeometryRecord {
            panes: vec![PaneRect {
                cols: 60,
                ..terminal
            }],
        })
    });
    local.apply_frame(&mut state, &changed_layout).unwrap();
    assert_eq!(local.range(), None);
    local.apply_frame(&mut state, &frame).unwrap();
    assert_eq!(local.range(), None);
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
}

#[test]
fn selection_scrolled_out_of_a_shorter_viewport_keeps_copy_without_highlight() {
    let mut state = Painter::state(3, 80, |p| p.text(2, 70, "ABCD"));
    let mut local = selected(&state, Selection::at(2, 70).extended_to(2, 73));
    local.resizing(&state, 1, 60);
    local
        .apply_frame(&mut state, &wrapped_frame(1, 60, "surviving row"))
        .unwrap();
    assert_eq!(local.range(), None);
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
}

fn reanchored() -> (
    crate::emulation::terminal::TerminalState,
    LocalSelection,
    String,
) {
    let text = format!("{}distinctive ABCD suffix", "x".repeat(58));
    let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
    state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
    let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
    local.resizing(&state, 3, 60);
    local
        .apply_frame(&mut state, &wrapped_frame(3, 80, &text))
        .unwrap();
    local
        .apply_frame(&mut state, &wrapped_frame(3, 60, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(1, 10).extended_to(1, 13)));
    assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    (state, local, text)
}

#[test]
fn captured_highlight_disappears_after_delta_full_clear_or_scroll_output() {
    use yinz_types::structured_render::{FrameBuilder, ShiftEntry, ShiftRecord};
    for update in ["delta", "full", "clear", "scroll"] {
        let (mut state, mut local, text) = reanchored();
        let frame = match update {
            "delta" => Painter::frame(3, 60, |p| p.text(1, 10, "WXYZ")),
            "full" => wrapped_frame(3, 60, &text.replace("ABCD", "WXYZ")),
            "clear" => {
                let mut builder = FrameBuilder::new(60, 3, 1);
                builder.set_clear(true);
                builder.finish()
            },
            _ => {
                let mut builder = FrameBuilder::new(60, 3, 1);
                builder.push_shifts(&ShiftRecord {
                    entries: vec![ShiftEntry {
                        x: 0,
                        y: 0,
                        cols: 60,
                        rows: 3,
                        lines: 1,
                    }],
                });
                builder.finish()
            },
        };
        local.apply_frame(&mut state, &frame).unwrap();
        let current = Selection::at(1, 10).extended_to(1, 13).text(state.screen());
        if matches!(update, "delta" | "full") {
            assert_eq!(current, "WXYZ");
        } else {
            assert!(current.is_empty());
        }
        assert_eq!(local.range(), None, "{update}");
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
        // Same-grid output cannot resurrect the highlight, even if the entire
        // original pattern returns. A real resize can uniquely locate it again.
        local
            .apply_frame(&mut state, &wrapped_frame(3, 60, &text))
            .unwrap();
        assert_eq!(local.range(), None);
        local.resizing(&state, 3, 80);
        local
            .apply_frame(&mut state, &wrapped_frame(3, 80, &text))
            .unwrap();
        assert_eq!(local.range(), Some(Selection::at(0, 70).extended_to(0, 73)));
        local.clear();
        assert_eq!(local.text(state.screen()), None);
        local.start(0, 70);
        local.extend(0, 73);
        local
            .apply_frame(
                &mut state,
                &Painter::frame(3, 80, |p| p.text(0, 70, "WXYZ")),
            )
            .unwrap();
        assert_eq!(local.text(state.screen()).as_deref(), Some("WXYZ"));
    }
}

#[test]
fn harmless_output_preserves_captured_highlight_and_copy() {
    use crate::emulation::screen_buffer::CursorShape;
    use yinz_types::structured_render::ATTR_BOLD;
    for update in [
        "other-row",
        "other-cell",
        "cursor",
        "style",
        "identical-full",
    ] {
        let (mut state, mut local, text) = reanchored();
        let expected = local.range();
        let frame = match update {
            "other-row" => Painter::frame(3, 60, |p| p.text(2, 0, "unrelated")),
            "other-cell" => Painter::frame(3, 60, |p| p.text(1, 50, "unrelated")),
            "cursor" => Painter::frame(3, 60, |p| p.cursor(1, 12, CursorShape::Beam)),
            "style" => Painter::frame(3, 60, |p| {
                p.styled(1, 10, "ABCD", |c| {
                    c.attrs |= ATTR_BOLD;
                    c.fg = yinz_types::structured_render::WireColor::Rgb(12, 34, 56).pack();
                })
            }),
            _ => wrapped_frame(3, 60, &text),
        };
        local.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(local.range(), expected, "{update}");
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    }
}

#[test]
fn context_and_geometry_changes_hide_highlights_without_same_grid_resurrection() {
    use yinz_types::structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE};
    for geometry in [false, true] {
        let (mut state, mut local, text) = reanchored();
        let frame = Painter::frame(3, 60, |p| {
            if geometry {
                p.geometry(&GeometryRecord {
                    panes: vec![PaneRect {
                        cols: 60,
                        rows: 3,
                        flags: PANE_SELECTABLE,
                        ..Default::default()
                    }],
                });
            } else {
                p.text(1, 9, "!");
            }
        });
        local.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(
            Selection::at(1, 10).extended_to(1, 13).text(state.screen()),
            "ABCD"
        );
        assert_eq!(local.range(), None);
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
        let restored = Painter::frame(3, 60, |p| {
            p.geometry(&GeometryRecord::default());
            p.text(1, 0, &text[60..]);
        });
        local.apply_frame(&mut state, &restored).unwrap();
        assert_eq!(local.range(), None);
    }
}

#[test]
fn pending_and_coalesced_resize_output_validates_the_current_location() {
    let text = format!("{}distinctive ABCD suffix", "x".repeat(58));
    let mut state = crate::emulation::terminal::TerminalState::new(3, 80);
    state.apply_frame(&wrapped_frame(3, 80, &text)).unwrap();
    let mut local = selected(&state, Selection::at(0, 70).extended_to(0, 73));
    local.resizing(&state, 3, 70);
    local.resizing(&state, 3, 60);
    local
        .apply_frame(
            &mut state,
            &Painter::frame(3, 80, |p| p.text(0, 70, "WXYZ")),
        )
        .unwrap();
    assert_eq!(local.range(), None);
    assert_eq!(local.pending, Some((3, 60)));
    local
        .apply_frame(&mut state, &wrapped_frame(3, 70, &text))
        .unwrap();
    assert_eq!(local.range(), Some(Selection::at(1, 0).extended_to(1, 3)));
    local
        .apply_frame(&mut state, &Painter::frame(3, 70, |p| p.text(1, 0, "WXYZ")))
        .unwrap();
    assert_eq!(local.range(), None);
    assert_eq!(local.pending, Some((3, 60)));
    local
        .apply_frame(&mut state, &wrapped_frame(3, 60, &text))
        .unwrap();
    let expected = local.range();
    assert_eq!(expected, Some(Selection::at(1, 10).extended_to(1, 13)));
    for frame in [
        b"invalid".to_vec(),
        Painter::frame(4, 50, |p| p.text(0, 0, "invalid reshape")),
    ] {
        assert!(local.apply_frame(&mut state, &frame).is_err());
        assert_eq!(local.range(), expected);
        assert_eq!(local.text(state.screen()).as_deref(), Some("ABCD"));
    }
    local
        .apply_frame(
            &mut state,
            &Painter::frame(3, 60, |p| p.text(1, 10, "WXYZ")),
        )
        .unwrap();
    assert_eq!(local.range(), None);
}

#[test]
fn wrapped_reverse_capture_keeps_original_newlines_on_identical_output() {
    for reverse in [false, true] {
        let mut state = Painter::state(3, 10, |p| {
            p.text(0, 0, "0123456789");
            p.text(1, 0, "abcdefghij");
        });
        let range = if reverse {
            Selection::at(1, 2).extended_to(0, 8)
        } else {
            Selection::at(0, 8).extended_to(1, 2)
        };
        let mut local = selected(&state, range);
        local.resizing(&state, 5, 5);
        let frame = wrapped_frame(5, 5, "0123456789abcdefghij");
        local.apply_frame(&mut state, &frame).unwrap();
        let mapped = local.range();
        assert!(mapped.is_some());
        local.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(local.range(), mapped);
        assert_eq!(local.text(state.screen()).as_deref(), Some("89\nabc"));
        local
            .apply_frame(&mut state, &Painter::frame(5, 5, |p| p.text(2, 0, "XYZ")))
            .unwrap();
        assert_eq!(local.range(), None);
        assert_eq!(local.text(state.screen()).as_deref(), Some("89\nabc"));
    }
}

#[test]
fn captured_unicode_support_requires_exact_text_and_display_unit_boundaries() {
    for update in ["combining", "width", "group"] {
        let paint = |p: &mut Painter| {
            p.wide(0, 5, '漢');
            p.cluster(0, 7, "e\u{301}", 1);
            p.group(0, 8, &FAMILY);
        };
        let mut state = Painter::state(3, 30, paint);
        let mut local = selected(&state, Selection::at(0, 6).extended_to(0, 11));
        let expected = local.text(state.screen());
        local.resizing(&state, 4, 30);
        local
            .apply_frame(
                &mut state,
                &Painter::frame(4, 30, |p| {
                    p.full_repaint();
                    paint(p);
                }),
            )
            .unwrap();
        let range = local.range();
        assert!(range.is_some());
        local
            .apply_frame(&mut state, &Painter::frame(4, 30, paint))
            .unwrap();
        assert_eq!(local.range(), range);
        let frame = Painter::frame(4, 30, |p| match update {
            "combining" => p.cluster(0, 7, "e\u{300}", 1),
            // Same text and total occupied cells, different display-unit span.
            "width" => {
                p.cluster(0, 5, "漢", 1);
                p.text(0, 6, " ");
            },
            _ => {
                let mut col = 8;
                for (text, width) in FAMILY {
                    p.cluster(0, col, text, width);
                    col += width;
                }
            },
        });
        local.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(local.range(), None, "{update}");
        assert_eq!(local.text(state.screen()), expected);
    }
}
