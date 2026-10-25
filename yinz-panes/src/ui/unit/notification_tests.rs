use super::*;

fn viewport() -> Viewport {
    Viewport {
        x: 10,
        y: 5,
        rows: 30,
        cols: 80,
    }
}

fn rows(chunks: &[CharacterChunk]) -> Vec<String> {
    chunks
        .iter()
        .map(|chunk| {
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect()
        })
        .collect()
}

#[test]
fn notifications_stack_away_from_the_edge_they_are_anchored_to() {
    let viewport = viewport();
    let width = 20;
    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
        NotificationPosition::BottomLeft,
        NotificationPosition::BottomRight,
    ] {
        let origins = notification_origins(viewport, position, &[3, 3, 3], width);
        assert_eq!(origins.len(), 3, "{:?}", position);

        let expected_x = if position.is_left() {
            viewport.x + EDGE_MARGIN
        } else {
            viewport.x + viewport.cols - width - EDGE_MARGIN
        };
        assert!(
            origins.iter().all(|(x, _)| *x == expected_x),
            "{:?} should pin every toast to the same column",
            position
        );

        if position.is_top() {
            assert_eq!(origins[0].1, viewport.y, "{:?}", position);
            assert!(
                origins[0].1 < origins[1].1 && origins[1].1 < origins[2].1,
                "{:?} should grow downwards",
                position
            );
        } else {
            assert_eq!(
                origins[0].1,
                viewport.y + viewport.rows - 3,
                "{:?}",
                position
            );
            assert!(
                origins[0].1 > origins[1].1 && origins[1].1 > origins[2].1,
                "{:?} should grow upwards",
                position
            );
        }
    }
}

#[test]
fn notifications_that_do_not_fit_are_dropped_rather_than_wrapping_around() {
    let short = Viewport {
        x: 0,
        y: 0,
        rows: 7, // room for 2 toasts, not 5
        cols: 80,
    };
    let origins = notification_origins(short, NotificationPosition::TopLeft, &[3; 5], 20);
    assert_eq!(origins.len(), 2);

    let narrow = Viewport {
        x: 0,
        y: 0,
        rows: 30,
        cols: 10,
    };
    assert!(notification_origins(narrow, NotificationPosition::TopLeft, &[3], 20).is_empty());
}

/// Heights differ now, so the stack is an accumulation rather than a multiplication.
#[test]
fn toasts_of_different_heights_stack_without_overlapping_or_gaps() {
    let viewport = viewport();
    let heights = [5, 3, 4];

    let top = notification_origins(viewport, NotificationPosition::TopLeft, &heights, 20);
    assert_eq!(top.len(), 3);
    for (index, height) in heights.iter().enumerate().take(2) {
        assert_eq!(
            top[index].1 + height,
            top[index + 1].1,
            "toast {index} should end exactly where the next one starts"
        );
    }

    let bottom = notification_origins(viewport, NotificationPosition::BottomLeft, &heights, 20);
    assert_eq!(bottom.len(), 3);
    for (index, _) in heights.iter().enumerate().take(2) {
        assert_eq!(
            bottom[index + 1].1 + heights[index + 1],
            bottom[index].1,
            "toast {index} should start exactly where the next one ends"
        );
    }
}

/// The easy one to get wrong: a taller toast at a bottom corner grows upwards, so its own
/// height moves its origin rather than leaving it hanging off the bottom.
#[test]
fn a_bottom_anchored_toast_is_positioned_by_its_bottom_edge() {
    let viewport = viewport();
    for height in [3, 5, 8] {
        let origins =
            notification_origins(viewport, NotificationPosition::BottomRight, &[height], 20);
        assert_eq!(
            origins[0].1 + height,
            viewport.y + viewport.rows,
            "a {height}-row toast should end on the last viewport row"
        );
    }
}

#[test]
fn a_toast_taller_than_the_viewport_is_dropped() {
    let short = Viewport {
        x: 0,
        y: 0,
        rows: 6,
        cols: 80,
    };
    assert!(notification_origins(short, NotificationPosition::TopLeft, &[8], 20).is_empty());
}

#[test]
fn width_is_clamped_to_the_viewport_and_the_maximum() {
    assert_eq!(notification_width("hello", 80), Some(9));
    assert_eq!(
        notification_width(&"x".repeat(200), 80),
        Some(MAX_NOTIFICATION_COLS)
    );
    // a viewport narrower than the maximum clamps to what it can hold
    assert_eq!(notification_width(&"x".repeat(200), 20), Some(18));
    assert_eq!(notification_width("hello", 3), None);
}

/// Measuring the whole string would ask for a box as wide as every line laid end to end.
#[test]
fn width_is_the_widest_explicit_line_rather_than_the_whole_string() {
    assert_eq!(notification_width("hello\nhi\nthere", 80), Some(9));
}

#[test]
fn a_short_single_line_message_wraps_to_exactly_one_row() {
    assert_eq!(
        notification_lines("build finished", 40),
        vec!["build finished"]
    );
}

#[test]
fn explicit_newlines_start_a_new_row() {
    assert_eq!(
        notification_lines("agent needs confirmation\nsend_input", 40),
        vec!["agent needs confirmation", "send_input"]
    );
}

#[test]
fn a_blank_line_between_paragraphs_is_kept_as_a_blank_row() {
    assert_eq!(notification_lines("one\n\ntwo", 40), vec!["one", "", "two"]);
}

#[test]
fn words_wrap_on_whitespace_rather_than_mid_word() {
    // 14 columns of text: "the quick" fits, "brown" does not.
    assert_eq!(
        notification_lines("the quick brown fox", 18),
        vec!["the quick", "brown fox"]
    );
}

#[test]
fn leading_indentation_survives_wrapping() {
    let lines = notification_lines("  text: git push --force --all", 18);
    assert_eq!(lines, vec!["  text: git", "  push --force", "  --all"]);
}

#[test]
fn a_word_longer_than_a_row_is_hard_broken() {
    assert_eq!(
        notification_lines(&"x".repeat(20), 12),
        vec!["xxxxxxxx", "xxxxxxxx", "xxxx"]
    );
}

/// A double-width glyph that straddled the border would paint over the box's own frame.
#[test]
fn wide_characters_never_straddle_the_box_border() {
    // 7 columns of text, glyphs 2 columns each: three fit, the fourth starts a row.
    let lines = notification_lines(&"日".repeat(4), 11);
    assert_eq!(lines, vec!["日日日", "日"]);
    for line in &lines {
        assert!(line.width() <= 7, "{line:?} overflows the row");
    }
}

#[test]
fn text_past_the_row_cap_is_truncated_with_an_ellipsis_on_the_last_row() {
    let many = (0..MAX_NOTIFICATION_TEXT_ROWS + 4)
        .map(|index| format!("row {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    let lines = notification_lines(&many, 20);

    assert_eq!(lines.len(), MAX_NOTIFICATION_TEXT_ROWS);
    assert!(
        lines
            .last()
            .expect("capped at a non-zero cap")
            .ends_with('…'),
        "got {:?}",
        lines.last()
    );
}

/// Truncate-then-append is what makes the narrowest possible row work without a special case.
#[test]
fn the_ellipsis_still_fits_a_one_column_row() {
    let many = "a\n".repeat(MAX_NOTIFICATION_TEXT_ROWS + 2);
    let lines = notification_lines(&many, CHROME_COLS + 1);
    assert_eq!(lines.last().map(String::as_str), Some("…"));
}

#[test]
fn control_characters_are_dropped_so_a_message_cannot_repaint_the_session() {
    assert_eq!(
        notification_lines("safe\u{1b}[2Jtext\rmore", 40),
        vec!["safe[2Jtextmore"]
    );
}

#[test]
fn a_single_line_toast_is_still_three_rows_of_exactly_the_requested_width() {
    let style = Style::default();
    let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
    assert_eq!(chunks.len(), notification_height(1));
    for (offset, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.x, 4);
        assert_eq!(chunk.y, 9 + offset);
        assert_eq!(
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.width())
                .sum::<usize>(),
            20,
            "row {} should span the full width",
            offset
        );
    }
}

#[test]
fn every_row_of_a_multi_line_toast_spans_the_full_width() {
    let style = Style::default();
    let lines = notification_lines("one\ntwo three\n  four", 20);
    let chunks = notification_chunks(&lines, None, 0, 0, 20, &style, 1.0);

    assert_eq!(chunks.len(), notification_height(lines.len()));
    for (offset, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.y, offset, "rows should be contiguous");
        assert_eq!(
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.width())
                .sum::<usize>(),
            20,
            "row {offset} should span the full width"
        );
    }
}

/// The shape this whole change exists for: agent's confirmation block, rendered as written.
#[test]
fn the_agent_confirmation_block_renders_as_written() {
    let style = Style::default();
    let text = "agent needs confirmation\nsend_input\n  pane_id: 3\n  text: git push";
    let width = notification_width(text, 80).expect("wide enough");
    let lines = notification_lines(text, width);

    assert_eq!(
        lines,
        vec![
            "agent needs confirmation",
            "send_input",
            "  pane_id: 3",
            "  text: git push",
        ]
    );

    let painted = rows(&notification_chunks(&lines, None, 0, 0, width, &style, 1.0));
    assert_eq!(painted.len(), notification_height(4));
    assert!(painted[0].starts_with('╭'), "got {:?}", painted[0]);
    assert_eq!(painted[1], "│ agent needs confirmation │");
    assert_eq!(painted[3], "│   pane_id: 3             │");
    assert!(painted[5].starts_with('╰'), "got {:?}", painted[5]);
}

#[test]
fn a_fully_hidden_reveal_paints_nothing() {
    let style = Style::default();
    let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
    ] {
        let (revealed, _) = notification_reveal(&chunks, 4, 20, position, 0.0);
        for chunk in &revealed {
            assert!(
                chunk.terminal_characters.is_empty(),
                "{:?} at progress 0.0 should paint nothing",
                position
            );
        }
    }
}

#[test]
fn a_fully_shown_reveal_is_identical_to_the_unanimated_chunks() {
    let style = Style::default();
    let lines = notification_lines("one\ntwo three\n  four", 20);
    let chunks = notification_chunks(&lines, None, 4, 9, 20, &style, 1.0);
    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
    ] {
        let (revealed, x) = notification_reveal(&chunks, 4, 20, position, 1.0);
        assert_eq!(x, 4);
        assert_eq!(revealed.len(), chunks.len(), "{:?}", position);
        for (revealed_chunk, chunk) in revealed.iter().zip(&chunks) {
            assert_eq!(revealed_chunk.x, chunk.x, "{:?}", position);
            assert_eq!(revealed_chunk.y, chunk.y, "{:?}", position);
            assert_eq!(
                rows(std::slice::from_ref(revealed_chunk)),
                rows(std::slice::from_ref(chunk)),
                "{:?}",
                position
            );
        }
    }
}

#[test]
fn a_left_anchored_reveal_keeps_x_fixed_and_grows_rightward() {
    let style = Style::default();
    let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
    for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let (revealed, x) =
            notification_reveal(&chunks, 4, 20, NotificationPosition::TopLeft, progress);
        assert_eq!(
            x, 4,
            "left-anchored x should never move, progress {progress}"
        );
        for chunk in &revealed {
            let painted_width: usize = chunk.terminal_characters.iter().map(|c| c.width()).sum();
            assert!(painted_width <= 20);
        }
    }
}

#[test]
fn a_right_anchored_reveal_keeps_the_right_edge_fixed_and_grows_leftward() {
    let style = Style::default();
    let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
    let mut previous_x = None;
    for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let (revealed, x) =
            notification_reveal(&chunks, 4, 20, NotificationPosition::TopRight, progress);
        let painted_width: usize = revealed[0]
            .terminal_characters
            .iter()
            .map(|c| c.width())
            .sum();
        assert_eq!(
            x + painted_width,
            24,
            "the right edge should stay fixed at x + width, progress {progress}"
        );
        if let Some(previous_x) = previous_x {
            assert!(
                x <= previous_x,
                "x should shrink towards 4 as progress increases"
            );
        }
        previous_x = Some(x);
    }
    assert_eq!(previous_x, Some(4));
}

#[test]
fn a_reveal_never_splits_a_wide_glyph() {
    let style = Style::default();
    // 4 double-width glyphs, 8 columns of text, box is CHROME_COLS(4) + 8 = 12 wide
    let lines = notification_lines(&"日".repeat(4), 12);
    let chunks = notification_chunks(&lines, None, 0, 0, 12, &style, 1.0);
    for progress in 0..=12 {
        let progress = progress as f32 / 12.0;
        let (revealed, _) =
            notification_reveal(&chunks, 0, 12, NotificationPosition::TopLeft, progress);
        for chunk in &revealed {
            for character in &chunk.terminal_characters {
                assert!(
                    character.width() == 1 || character.character == '日',
                    "a glyph must never be kept partially, got {:?}",
                    character.character
                );
            }
            let painted_width: usize = chunk.terminal_characters.iter().map(|c| c.width()).sum();
            assert!(painted_width <= 12);
        }
    }
}

/// The property the whole no-jump design rests on: for every survivor of a stack change, the
/// row it was settled at equals the row it is settled at now, plus the delta. If that holds,
/// adding the delta to the toast's displacement leaves the painted row exactly where it was.
#[test]
fn a_settled_row_delta_accounts_for_exactly_where_a_toast_moved_to() {
    let viewport = viewport();
    let width = 20;
    // ids 1..=3 at differing heights, then the newest (id 4) arrives at the corner and id 2 is
    // dismissed - so the survivors are re-ordered *and* pushed by a toast of a different size
    let before: Vec<(NotificationId, usize)> = vec![
        (NotificationId::new(3), 3),
        (NotificationId::new(2), 5),
        (NotificationId::new(1), 4),
    ];
    let after: Vec<(NotificationId, usize)> = vec![
        (NotificationId::new(4), 6),
        (NotificationId::new(3), 3),
        (NotificationId::new(1), 4),
    ];

    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
        NotificationPosition::BottomLeft,
        NotificationPosition::BottomRight,
    ] {
        let heights = |stack: &[(NotificationId, usize)]| -> Vec<usize> {
            stack.iter().map(|(_, height)| *height).collect()
        };
        let origins_before = notification_origins(viewport, position, &heights(&before), width);
        let origins_after = notification_origins(viewport, position, &heights(&after), width);

        for (index, (id, _)) in after.iter().enumerate() {
            let Some(before_index) = before.iter().position(|(candidate, _)| candidate == id)
            else {
                // a new arrival has nowhere to have moved from
                assert_eq!(settled_row_delta(position, &before, &after, *id), None);
                continue;
            };
            let delta =
                settled_row_delta(position, &before, &after, *id).expect("a survivor has a delta");
            assert_eq!(
                origins_before[before_index].1 as isize + delta,
                origins_after[index].1 as isize,
                "{position:?}: toast {id} should land exactly on its new row"
            );
        }
    }
}

#[test]
fn a_settled_row_delta_is_zero_when_nothing_moved() {
    let stack: Vec<(NotificationId, usize)> =
        vec![(NotificationId::new(1), 3), (NotificationId::new(2), 4)];
    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::BottomRight,
    ] {
        assert_eq!(
            settled_row_delta(position, &stack, &stack, NotificationId::new(1)),
            Some(0)
        );
        assert_eq!(
            settled_row_delta(position, &stack, &stack, NotificationId::new(2)),
            Some(0)
        );
    }
}

#[test]
fn a_settled_row_delta_has_no_answer_for_a_toast_outside_either_stack() {
    let before: Vec<(NotificationId, usize)> = vec![(NotificationId::new(1), 3)];
    let after: Vec<(NotificationId, usize)> = vec![(NotificationId::new(2), 3)];
    let position = NotificationPosition::TopRight;
    assert_eq!(
        settled_row_delta(position, &before, &after, NotificationId::new(1)),
        None
    );
    assert_eq!(
        settled_row_delta(position, &before, &after, NotificationId::new(2)),
        None
    );
    assert_eq!(
        settled_row_delta(position, &before, &after, NotificationId::new(9)),
        None
    );
}

/// A toast growing taller pushes what is behind it away from the corner, in whichever
/// direction that corner implies. This is the case a wide arrival creates: the shared width
/// grows, everything re-wraps, and heights change under toasts that did not themselves move
/// in the stack.
#[test]
fn a_height_change_displaces_what_is_stacked_behind_it() {
    let before: Vec<(NotificationId, usize)> =
        vec![(NotificationId::new(1), 3), (NotificationId::new(2), 4)];
    let after: Vec<(NotificationId, usize)> =
        vec![(NotificationId::new(1), 5), (NotificationId::new(2), 4)];
    assert_eq!(
        settled_row_delta(
            NotificationPosition::TopLeft,
            &before,
            &after,
            NotificationId::new(2)
        ),
        Some(2),
        "a top-anchored stack grows downwards, so the one behind moves down"
    );
    assert_eq!(
        settled_row_delta(
            NotificationPosition::BottomLeft,
            &before,
            &after,
            NotificationId::new(2)
        ),
        Some(-2),
        "a bottom-anchored stack grows upwards, so the one behind moves up"
    );
    assert_eq!(
        settled_row_delta(
            NotificationPosition::TopLeft,
            &before,
            &after,
            NotificationId::new(1)
        ),
        Some(0),
        "the toast in the corner does not move, however tall it gets"
    );
}

/// The colours a chunk's characters actually carry, as `(foreground, background)` pairs.
fn colors(chunk: &CharacterChunk) -> Vec<(Option<AnsiCode>, Option<AnsiCode>)> {
    chunk
        .terminal_characters
        .iter()
        .map(|character| (character.styles.foreground, character.styles.background))
        .collect()
}

/// The guarantee that makes the fade invisible at rest: a fully-arrived toast carries the
/// exact colours the theme declared, `PaletteColor` variant included, rather than an RGB
/// resolution of them. A terminal with a customised 16-colour palette therefore only ever sees
/// an approximation during the frames a toast is actually moving.
#[test]
fn a_fully_faded_in_toast_carries_the_declared_colours_exactly() {
    let style = Style::default();
    let declaration = style.colors.text_unselected;
    let background = Some(AnsiCode::from(declaration.background));
    let border = Some(AnsiCode::from(declaration.emphasis_0));
    let text = Some(AnsiCode::from(declaration.base));

    let lines = notification_lines("one\ntwo three", 20);
    let chunks = notification_chunks(&lines, None, 4, 9, 20, &style, 1.0);
    let (border_rows, text_rows) = chunks.split_at(1);

    for (foreground, chunk_background) in colors(&border_rows[0]) {
        assert_eq!(foreground, border, "the top border is the declared colour");
        assert_eq!(chunk_background, background);
    }
    // the last chunk is the bottom border; everything between is text framed by border pipes
    let (text_rows, bottom) = text_rows.split_at(text_rows.len() - 1);
    for (foreground, _) in colors(&bottom[0]) {
        assert_eq!(foreground, border);
    }
    for chunk in text_rows {
        let row = colors(chunk);
        assert_eq!(row.first().map(|(fg, _)| *fg), Some(border), "left pipe");
        assert_eq!(row.last().map(|(fg, _)| *fg), Some(border), "right pipe");
        for (foreground, chunk_background) in &row[1..row.len() - 1] {
            assert_eq!(*foreground, text, "the text is the declared colour");
            assert_eq!(*chunk_background, background);
        }
    }
}

#[test]
fn a_toast_at_zero_fade_is_a_solid_slab_of_its_own_background() {
    let style = Style::default();
    let declaration = style.colors.text_unselected;
    let background = Some(AnsiCode::from(declaration.background));
    let lines = notification_lines("hidden", 20);
    for chunk in notification_chunks(&lines, None, 4, 9, 20, &style, 0.0) {
        for (foreground, chunk_background) in colors(&chunk) {
            assert_eq!(
                foreground, background,
                "the border and text have not resolved out of the slab yet"
            );
            assert_eq!(chunk_background, background);
        }
    }
}

#[test]
fn a_partly_faded_toast_never_leaves_the_segment_between_its_endpoints() {
    let style = Style::default();
    let declaration = style.colors.text_unselected;
    let (from_r, from_g, from_b) = rgb_of(declaration.background);
    let lines = notification_lines("resolving", 20);
    for step in 1..10 {
        let fade = step as f32 / 10.0;
        for chunk in notification_chunks(&lines, None, 4, 9, 20, &style, fade) {
            for (foreground, _) in colors(&chunk) {
                let Some(AnsiCode::RgbCode((r, g, b))) = foreground else {
                    panic!("a mid-fade colour is resolved rgb, got {foreground:?}");
                };
                // every channel is between the background it started at and one of the two
                // endpoints it can be heading for
                for (channel, start, ends) in [
                    (
                        r,
                        from_r,
                        [rgb_of(declaration.base).0, rgb_of(declaration.emphasis_0).0],
                    ),
                    (
                        g,
                        from_g,
                        [rgb_of(declaration.base).1, rgb_of(declaration.emphasis_0).1],
                    ),
                    (
                        b,
                        from_b,
                        [rgb_of(declaration.base).2, rgb_of(declaration.emphasis_0).2],
                    ),
                ] {
                    assert!(
                        ends.iter().any(|end| {
                            channel >= start.min(*end) && channel <= start.max(*end)
                        }),
                        "channel {channel} outside [{start}, {ends:?}] at fade {fade}"
                    );
                }
            }
        }
    }
}

const LONG: &str = "one two three four five six seven eight nine ten eleven twelve thirteen \
        fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two \
        twenty-three twenty-four twenty-five twenty-six twenty-seven twenty-eight";

#[test]
fn a_collapsed_browser_entry_is_cut_to_two_rows_and_elided() {
    let layout = notification_browser_layout(
        viewport(),
        NotificationPosition::TopRight,
        &[LONG, "short"],
        1,
        false,
        0,
    )
    .expect("the viewport holds the browser");
    let long = &layout.entries[0];
    assert_eq!(long.lines.len(), BROWSER_COLLAPSED_TEXT_ROWS);
    assert!(
        long.lines.last().unwrap().ends_with('…'),
        "{:?}",
        long.lines
    );
    assert_eq!(layout.entries[1].lines, vec!["short".to_owned()]);
}

#[test]
fn the_expanded_browser_entry_shows_its_whole_text() {
    let layout = notification_browser_layout(
        viewport(),
        NotificationPosition::TopRight,
        &[LONG, "short"],
        0,
        true,
        0,
    )
    .expect("the viewport holds the browser");
    let expanded = &layout.entries[0];
    assert!(expanded.lines.len() > MAX_NOTIFICATION_TEXT_ROWS.min(3));
    assert!(!expanded.lines.iter().any(|line| line.ends_with('…')));
    let text: Vec<&str> = expanded
        .lines
        .iter()
        .flat_map(|line| line.split_whitespace())
        .collect();
    assert_eq!(text, LONG.split_whitespace().collect::<Vec<_>>());
}

#[test]
fn the_browser_scrolls_only_as_far_as_the_selection_needs() {
    // every entry is one text row, so three rows tall; ten rows hold three of them
    let viewport = Viewport {
        x: 0,
        y: 0,
        rows: 10,
        cols: 80,
    };
    let texts = ["a", "b", "c", "d", "e", "f"];
    let shown = |selected: usize, first: usize| {
        let layout = notification_browser_layout(
            viewport,
            NotificationPosition::TopRight,
            &texts,
            selected,
            false,
            first,
        )
        .unwrap();
        let indices: Vec<usize> = layout.entries.iter().map(|entry| entry.index).collect();
        (layout.first, indices)
    };
    assert_eq!(shown(2, 0), (0, vec![0, 1, 2]));
    // moving past the bottom scrolls by one
    assert_eq!(shown(3, 0), (1, vec![1, 2, 3]));
    // moving back up inside the window keeps it where it is
    assert_eq!(shown(2, 1), (1, vec![1, 2, 3]));
    // moving above the window scrolls back to the selection
    assert_eq!(shown(0, 1), (0, vec![0, 1, 2]));
}

#[test]
fn the_browser_stacks_from_the_configured_corner() {
    let viewport = viewport();
    for position in [
        NotificationPosition::TopLeft,
        NotificationPosition::BottomRight,
    ] {
        let layout =
            notification_browser_layout(viewport, position, &["a", "b"], 0, false, 0).unwrap();
        let (newest, older) = (&layout.entries[0], &layout.entries[1]);
        if position.is_top() {
            assert_eq!(newest.y, viewport.y);
            assert!(older.y > newest.y);
        } else {
            assert_eq!(newest.y, viewport.y + viewport.rows - 3);
            assert!(older.y < newest.y);
        }
    }
}

fn rgb_of(color: yinz_types::colors::PaletteColor) -> (u8, u8, u8) {
    match color {
        yinz_types::colors::PaletteColor::Rgb(rgb) => rgb,
        yinz_types::colors::PaletteColor::EightBit(index) => {
            yinz_types::colors::eightbit_to_rgb(index)
        },
    }
}
