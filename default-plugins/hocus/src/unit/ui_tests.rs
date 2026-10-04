use super::*;
use crate::Message;
use std::collections::BTreeSet;

/// Most of these care only about the wrapping, not about where each message
/// starts in it.
fn transcript_lines(messages: &[Message], cols: usize) -> Vec<VLine> {
    transcript(messages, cols).lines
}

// -- frames ---------------------------------------------------------------

fn plan_prompt() -> Confirmation {
    Confirmation {
        kind: PromptKind::Plan,
        title: "PLAN".to_owned(),
        detail: Vec::new(),
        keys: vec![("/execute", "run"), ("n", "refine"), ("Esc", "hide")],
    }
}

fn tool_prompt() -> Confirmation {
    Confirmation {
        kind: PromptKind::Tool,
        title: "send_input".to_owned(),
        detail: vec![
            ("types into pane 7".to_owned(), true),
            ("text: ls".to_owned(), false),
        ],
        keys: vec![("y", "run"), ("n", "skip")],
    }
}

/// Every row of a box is exactly as wide as the pane, borders included —
/// `print_text_with_coordinates` truncates, so a row a column too wide loses
/// its right-hand border rather than wrapping.
#[test]
fn a_frame_is_exactly_as_wide_as_the_pane() {
    let cols = 40;
    let inner = markdown_lines("ordinary words and 日本語 too", cols - FRAME_PAD);
    let lines = frame("PLAN", &key_runs(&[("n", "keep planning")]), inner, cols);

    for line in &lines {
        assert_eq!(
            line.text.width(),
            cols,
            "row {:?} is the wrong width",
            line.text
        );
        assert_eq!(
            line.styles.len(),
            line.text.chars().count(),
            "row {:?} has a style per character",
            line.text
        );
    }
}

/// The title sits on the top border and the keys on the bottom one, which is
/// what lets the plan be one box rather than a box and a prompt under it.
#[test]
fn a_frame_carries_its_title_and_keys_on_its_borders() {
    let lines = prompt_box(&plan_prompt(), markdown_lines("do the thing", 36), 40);

    assert!(lines[0].text.starts_with("┌ PLAN "), "{:?}", lines[0].text);
    assert!(lines[1].text.contains("do the thing"));
    let bottom = &lines.last().expect("a bottom border").text;
    assert!(
        bottom.starts_with("└ /execute run · n refine "),
        "{bottom:?}"
    );
    assert!(bottom.ends_with('┘'));
}

/// A pane too narrow for `│ … │` keeps the words and loses the box: the
/// content is what the user has to read, and the keys are what they have to
/// press.
#[test]
fn a_frame_gives_up_rather_than_clip_a_narrow_pane() {
    for cols in MIN_COLS..MIN_FRAME_COLS {
        let lines = prompt_box(&plan_prompt(), markdown_lines("do the thing", cols), cols);
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(lines.iter().all(|line| !line.text.contains('┌')));
        assert!(lines.iter().all(|line| line.text.width() <= cols));
        assert!(text.contains("do the thing"));
        for key in ["/execute", "n", "Esc"] {
            assert!(text.contains(key), "{key:?} was clipped at {cols} columns");
        }
    }
}

/// Keys wider than the border they would sit on move to their own line
/// instead of being cut in half by it.
#[test]
fn keys_too_wide_for_the_border_move_under_the_box() {
    for cols in MIN_FRAME_COLS..=35 {
        let lines = prompt_box(
            &plan_prompt(),
            markdown_lines("do the thing", cols - FRAME_PAD),
            cols,
        );
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            lines
                .iter()
                .any(|line| line.text.starts_with('└') && line.text.ends_with('┘')),
            "the bottom border is missing at {cols} columns"
        );
        assert!(lines.iter().all(|line| line.text.width() <= cols));
        for key in ["/execute", "n", "Esc"] {
            assert!(text.contains(key), "{key:?} was clipped at {cols} columns");
        }
    }
}

#[test]
fn an_unframed_tool_uses_the_whole_pane_width() {
    let cols = MIN_FRAME_COLS - 1;
    let prompt = Confirmation {
        detail: vec![("1234567890123456789012345678901".to_owned(), false)],
        ..tool_prompt()
    };
    let lines = prompt_box(&prompt, Vec::new(), cols);

    assert!(lines.iter().any(|line| line.text.len() == cols));
}

/// The point of the whole change: a confirmation is no longer a block of
/// red. Only the sentence saying what the call will do carries the error
/// colour, and the border stays dim.
#[test]
fn only_the_consequence_of_a_tool_call_is_red() {
    let lines = prompt_box(&tool_prompt(), Vec::new(), 40);

    assert!(
        lines.iter().all(|line| line.role != Role::Error),
        "no line is red as a whole"
    );
    assert!(lines[0].text.starts_with("┌ send_input "));
    let reddened: Vec<&str> = lines
        .iter()
        .filter(|line| line.styles.iter().any(|style| style.error))
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(
        reddened.len(),
        1,
        "exactly one line is reddened: {reddened:?}"
    );
    assert!(reddened[0].contains("types into pane 7"));
    // The border of that very line is still dim rather than red.
    let consequence = lines
        .iter()
        .find(|line| line.text.contains("types into pane 7"))
        .expect("present");
    assert!(consequence.styles[0].dim && !consequence.styles[0].error);
    assert!(lines
        .last()
        .expect("a bottom border")
        .text
        .contains("y run · n skip"));
}

// -- the input box's wrapping -------------------------------------------

/// The bug the trailing empty row exists for: a full row put the cursor at
/// column `cols`, one past the last column, and yinz hides a plugin cursor
/// that sits outside the pane. The wrapper itself lives in `hocus_proto::text`
/// now; the prompt it has to leave room for is still ours.
#[test]
fn the_cursor_stays_inside_the_pane_when_a_row_is_full() {
    let cols = 24;
    let full = format!("{PROMPT}{}", "a".repeat(cols - PROMPT.width()));
    let lines = wrap_input(&full, cols);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        cursor_cell(&lines, full.chars().count()),
        Some((0, 1)),
        "cursor lands at column 0 of the next row"
    );
}

#[test]
fn the_input_box_breaks_between_words() {
    let cols = 24;
    let text = format!("{PROMPT}some reasonably long words here");
    let rows: Vec<String> = wrap_input(&text, cols)
        .into_iter()
        .map(|line| line.text)
        .collect();
    assert_eq!(rows, vec!["> some reasonably long ", "words here"]);
}

/// The cursor has to follow the word the wrap moved down, which is why it is
/// read out of the box's own rows rather than from a wrap of the text before
/// it: "words" only left the first row because of what was typed after it.
#[test]
fn the_cursor_follows_a_word_the_wrap_moved_down() {
    let cols = 24;
    let text = format!("{PROMPT}some reasonably long words here");
    let lines = wrap_input(&text, cols);
    // Char index of "words", which is where the second row begins.
    let at_words = text[..text.find("words").expect("present")].chars().count();
    // "words" starts the second row, so the cursor just before it is at its
    // column 0 and the cursor after its first character is at column 1.
    assert_eq!(cursor_cell(&lines, at_words), Some((0, 1)));
    assert_eq!(cursor_cell(&lines, at_words + 1), Some((1, 1)));
}

#[test]
fn a_long_input_window_follows_the_cursor() {
    let mut state = State::default();
    state.input.set_text(
        (0..10)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let tail = chat_layout(&state, 8, 24, full_chrome());
    assert_eq!(tail.input_lines.first().unwrap().text, "line 5");
    assert!(cursor_cell(
        &tail.input_lines,
        PROMPT.chars().count() + state.input.get_cursor_position()
    )
    .is_some());

    state.input.move_to_start();
    let head = chat_layout(&state, 8, 24, full_chrome());
    assert_eq!(head.input_lines.first().unwrap().text, "> line 0");
    assert_eq!(
        cursor_cell(&head.input_lines, PROMPT.chars().count()),
        Some((2, 0))
    );
}

#[test]
fn the_input_rules_fit_inside_the_chat_layout() {
    let layout = chat_layout(&State::default(), 4, 24, full_chrome());

    assert_eq!(layout.transcript_rows + layout.input_rows(), 4);
}

// -- degrading into a small pane ----------------------------------------
//
// The pane hocus is given is as small as the terminal is (see
// `overlay::MIN_PANE_ROWS`), so "pane too small" in place of the
// conversation is a refusal the user can do nothing about. What is drawn
// gets thinner instead.

/// The layout a pane with rows to spare gets, which is every one of these
/// tests' starting point.
fn full_chrome() -> Chrome {
    chrome_for(MIN_ROWS)
}

#[test]
fn chrome_is_given_up_in_order_of_what_matters_least() {
    let all = Chrome {
        title: true,
        transcript: true,
        help: true,
        rules: true,
    };
    assert_eq!(chrome_for(MIN_ROWS), all, "the whole layout at {MIN_ROWS}");
    assert_eq!(chrome_for(40), all, "and at any height above it");
    assert_eq!(
        chrome_for(5),
        Chrome {
            rules: false,
            ..all
        },
        "the rules go first"
    );
    assert_eq!(
        chrome_for(4),
        Chrome {
            rules: false,
            help: false,
            ..all
        },
        "then the help line"
    );
    assert_eq!(
        chrome_for(3),
        Chrome {
            title: false,
            transcript: true,
            help: false,
            rules: false
        },
        "then the title bar"
    );
    assert_eq!(
        chrome_for(2),
        Chrome {
            title: false,
            transcript: true,
            help: false,
            rules: false
        },
        "a transcript row and the input"
    );
    assert_eq!(
        chrome_for(1),
        Chrome {
            title: false,
            transcript: false,
            help: false,
            rules: false
        },
        "the input alone"
    );
    assert_eq!(chrome_for(0), chrome_for(1), "and nothing at all");
}

/// Whatever is drawn is drawn *inside* the pane: the input box is the one
/// thing never given up, so the row it lands on is the assertion that
/// matters. It is also where the cursor goes.
#[test]
fn a_short_pane_drops_chrome_rather_than_refusing() {
    let mut state = State {
        perms: true,
        ..Default::default()
    };
    state.input.set_text("typing".to_owned());
    for rows in 1..=(MIN_ROWS + 2) {
        for cols in [MIN_INPUT_COLS, 12, 40] {
            let chrome = chrome_for(rows);
            let (body_y, body_rows) = body_of(chrome, rows);
            let layout = chat_layout(&state, body_rows, cols, chrome);
            let at = format!("{rows}x{cols}");

            assert!(!layout.input_lines.is_empty(), "nowhere to type at {at}");
            assert_eq!(
                layout.transcript_rows + layout.input_rows(),
                body_rows,
                "the body is spent exactly at {at}"
            );
            assert!(
                layout.input_y(body_y) + layout.input_lines.len() <= rows,
                "the input box runs past the last row at {at}"
            );
            assert!(
                chrome.transcript || layout.transcript_rows == 0,
                "a transcript row in a pane with no room for one at {at}"
            );
            // and it renders, with a picker over it and without: a pane
            // this small used to say "too small" and draw nothing at all
            super::render(&mut state, rows, cols);
            let picker = state.picker.take();
            state.picker = Some(Picker::new(
                vec![crate::PickerRow::item("a model")],
                0,
                PickerKind::Models,
            ));
            super::render(&mut state, rows, cols);
            state.picker = picker;
        }
    }
}

#[test]
fn a_one_row_pane_is_the_input_box() {
    let chrome = chrome_for(1);
    let (body_y, body_rows) = body_of(chrome, 1);
    let layout = chat_layout(&State::default(), body_rows, 24, chrome);

    assert_eq!((body_y, body_rows), (0, 1), "no title, no help line");
    assert_eq!(layout.transcript_rows, 0);
    assert_eq!(layout.input_lines.len(), 1);
    assert_eq!(layout.input_y(body_y), 0, "the one row there is");
}

// -- transcript_lines ---------------------------------------------------

#[test]
fn transcript_right_aligns_user_turns() {
    // Wrapped to two thirds of 12, then padded out to the right edge.
    let lines = transcript_lines(&[Message::user("aaaa bbbb")], 12);
    assert_eq!(
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["        aaaa", "        bbbb"]
    );
    assert!(lines.iter().all(|l| l.text.width() == 12));
}

#[test]
fn transcript_leaves_replies_flush_left() {
    let lines = transcript_lines(&[Message::assistant("aaaa bbbb")], 6);
    assert_eq!(
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["aaaa", "bbbb"]
    );
}

#[test]
fn assistant_markdown_renders_as_terminal_blocks() {
    let markdown = "# Heading\n\nA **bold** and *slanted* `value`.\n\n- one\n- two\n\n> quoted\n\n[site](https://example.com)\n\n---";
    let lines = transcript_lines(&[Message::assistant(markdown)], 80);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Heading",
            "",
            "A bold and slanted value.",
            "",
            "• one",
            "• two",
            "",
            "│ quoted",
            "",
            "site (https://example.com)",
            "",
            &"─".repeat(80),
        ]
    );
}

#[test]
fn fenced_code_preserves_layout_and_hard_wraps() {
    let lines = transcript_lines(&[Message::assistant("```rust\n  abcdef\nxy\n```")], 5);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["  abc", "def", "xy"]
    );
}

#[test]
fn markdown_is_not_applied_to_non_reply_messages_or_tool_summaries() {
    let mut tool_summary = Message::assistant("**literal**");
    tool_summary.tool_calls.push(crate::ToolCall {
        id: "call-1".to_owned(),
        name: "list_session".to_owned(),
        arguments: "{}".to_owned(),
    });
    let lines = transcript_lines(
        &[
            Message::user("**user**"),
            tool_summary,
            Message::tool_result("call-1", "**tool**"),
            Message::error("**error**"),
        ],
        80,
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| !line.text.is_empty())
            .map(|line| line.text.trim_start())
            .collect::<Vec<_>>(),
        vec!["**user**", "**literal**", "fn:  **tool**", "err: **error**"]
    );
}

#[test]
fn a_long_tool_result_is_elided_to_a_few_lines() {
    let content = (1..=10)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let lines = transcript_lines(&[Message::tool_result("call-1", content)], 80);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.trim())
            .collect::<Vec<_>>(),
        vec!["fn:  1", "2", "3", "… 7 more lines"]
    );
}

/// A presented result is drawn as markdown, with the heading styled and no
/// `fn:` prefix competing with the renderer's own indents.
#[test]
fn a_presented_result_is_rendered_as_markdown() {
    let message =
        Message::tool_result_shown("manual-1", "{}", Some("## agents\n\n- one\n- two".into()));
    let lines = transcript_lines(&[message], 80);
    let texts: Vec<&str> = lines.iter().map(|line| line.text.trim_end()).collect();
    assert_eq!(texts, vec!["agents", "", "• one", "• two"]);
    // The heading is bold; nothing carries the `fn:` prefix.
    assert!(lines[0].styles.iter().all(|style| style.bold));
    assert!(!texts.iter().any(|text| text.starts_with("fn:")));
}

/// `styled` dims a whole tool line, which would flatten every colour a
/// presented result asked for — so it must not, when there is styling.
#[test]
fn a_presented_result_keeps_its_own_styling() {
    let plain = VLine {
        text: "raw".to_owned(),
        role: Role::Tool,
        styles: Vec::new(),
    };
    let presented = VLine {
        text: "shown".to_owned(),
        role: Role::Tool,
        styles: vec![MdStyle::default(); 5],
    };
    // A Text carries its styling opaquely, so compare against the two
    // renderings we know are meant to differ.
    assert_eq!(
        format!("{:?}", styled(&plain)),
        format!("{:?}", Text::new("raw").dim_range(..))
    );
    assert_ne!(
        format!("{:?}", styled(&presented)),
        format!("{:?}", Text::new("shown").dim_range(..))
    );
}

/// An empty `Role::Assistant` line at the tail is how the renderer spots the
/// streaming placeholder. A presented result must never be mistaken for one.
#[test]
fn a_presented_result_is_not_mistaken_for_the_streaming_placeholder() {
    let lines = transcript_lines(
        &[Message::tool_result_shown(
            "manual-1",
            "{}",
            Some(String::new()),
        )],
        80,
    );
    assert!(!ends_with_empty_reply(&lines));
}

/// The output of a tool the user typed is the reason they typed it, so none
/// of it is hidden.
#[test]
fn a_typed_tools_result_is_rendered_whole() {
    let content = (1..=10)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let lines = transcript_lines(&[Message::tool_result_full("manual-1", content)], 80);
    let mut expected = vec!["fn:  1".to_owned()];
    expected.extend((2..=10).map(|n| n.to_string()));
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.trim())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn markdown_styles_survive_marker_removal() {
    let lines = transcript_lines(&[Message::assistant("plain **bold** *slanted* `code`")], 80);
    let line = &lines[0];
    let style_at = |needle: &str| {
        let byte = line.text.find(needle).unwrap();
        line.styles[line.text[..byte].chars().count()]
    };
    assert_eq!(style_at("plain"), MdStyle::default());
    assert!(style_at("bold").bold);
    assert_eq!(style_at("slanted").color, Some(1));
    assert_eq!(style_at("code").color, Some(3));
}

#[test]
fn lists_keep_markers_and_hanging_indents_inside_the_pane() {
    let lines = transcript_lines(
        &[Message::assistant("3. abcdefgh\n4. 日本語\n   - nested")],
        8,
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "3. abcde",
            "   fgh",
            "4. 日本",
            "   語",
            "  • nest",
            "    ed"
        ]
    );
    assert!(lines.iter().all(|line| line.text.width() <= 8));
}

/// A model's pipe table reaches the pane as columns, not as pipes. The renderer does the
/// layout; this is the end-to-end check that the conversation asks it to.
#[test]
fn a_reply_containing_a_table_renders_as_columns() {
    let lines = transcript_lines(
        &[Message::assistant(
            "| pane | status |\n|---|---|\n| 1 | running |\n| 2 | idle |",
        )],
        40,
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "pane │ status ",
            "─────┼────────",
            "1    │ running",
            "2    │ idle   ",
        ]
    );
}

#[test]
fn links_images_html_and_tables_have_readable_fallbacks() {
    let lines = transcript_lines(
        &[Message::assistant(
            "[site](https://example.com) ![logo](logo.png) <kbd>x</kbd>\n\n| a | b |",
        )],
        100,
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "site (https://example.com) logo (logo.png) <kbd>x</kbd>",
            "",
            "| a | b |",
        ]
    );
}

#[test]
fn raw_html_blocks_remain_separate_literal_blocks() {
    let lines = transcript_lines(&[Message::assistant("<div>\nraw\n</div>\n\nafter")], 80);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["<div>", "raw", "</div>", "", "after"]
    );
}

#[test]
fn markdown_sanitizes_control_characters_before_parsing() {
    let lines = transcript_lines(
        &[Message::assistant("[safe](https://exa\u{1b}mple.com)\u{7}")],
        80,
    );
    assert_eq!(lines[0].text, "safe (https://example.com)");
    assert!(!lines[0].text.chars().any(char::is_control));
}

#[test]
fn incomplete_streaming_markdown_stays_renderable() {
    let lines = transcript_lines(
        &[Message::assistant(
            "An **unfinished emphasis\n\n```\npartial",
        )],
        80,
    );
    assert!(!lines.is_empty());
    assert!(lines.iter().any(|line| line.text.contains("unfinished")));
    assert!(lines.iter().any(|line| line.text.contains("partial")));
}

#[test]
fn transcript_still_labels_the_asides() {
    let lines = transcript_lines(&[Message::error("boom")], 40);
    assert_eq!(lines[0].text, "err: boom");
}

#[test]
fn transcript_puts_a_blank_line_between_messages() {
    let lines = transcript_lines(&[Message::user("hi"), Message::assistant("yo")], 40);
    assert_eq!(
        lines
            .iter()
            .map(|l| l.text.trim_start())
            .collect::<Vec<_>>(),
        vec!["hi", "", "yo"]
    );
}

#[test]
fn transcript_of_nothing_is_nothing() {
    assert!(transcript_lines(&[], 40).is_empty());
}

#[test]
fn transcript_survives_a_pane_too_narrow_to_wrap_into() {
    // body_width would underflow to 0 without the saturating_sub/max guard.
    let lines = transcript_lines(&[Message::user("hello"), Message::error("boom")], 3);
    assert!(!lines.is_empty());
}

// -- the transcript cache -----------------------------------------------

/// The one case the message count can't catch: a streamed reply grows in
/// place, so the key has to notice the last message getting longer.
#[test]
fn the_transcript_cache_notices_text_appended_to_the_last_message() {
    let mut state = State::default();
    state.messages.push(Message::assistant("Hel"));
    assert_eq!(cached_transcript(&mut state, 40).lines[0].text, "Hel");

    state.messages.last_mut().unwrap().content.push_str("lo");
    assert_eq!(cached_transcript(&mut state, 40).lines[0].text, "Hello");
}

#[test]
fn the_transcript_cache_rewraps_when_the_pane_is_resized() {
    let mut state = State::default();
    state.messages.push(Message::user("aaaa bbbb"));
    assert_eq!(cached_transcript(&mut state, 40).lines.len(), 1);
    assert_eq!(cached_transcript(&mut state, 9).lines.len(), 2);
}

/// A few turns, ending on the assistant message a reply would be growing.
fn a_conversation() -> State {
    let mut state = State::default();
    for n in 0..6 {
        state.messages.push(match n % 2 {
            0 => Message::user(format!("question {n}")),
            _ => Message::assistant(format!("## reply {n}\n\nwith a paragraph under it")),
        });
    }
    state
}

/// The reason the cache is per-message at all. A streaming reply used to
/// move the one key the whole transcript was memoised on, so every frame
/// re-parsed every message — work proportional to the whole conversation,
/// ten times a second, queued ahead of whatever the user was typing.
#[test]
fn a_streaming_frame_redraws_only_the_message_that_grew() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    assert_eq!(rebuilds(), 6, "the first frame draws all of them");

    state.messages.last_mut().unwrap().content.push_str(" more");
    cached_transcript(&mut state, 40);
    assert_eq!(rebuilds(), 1, "and the next one draws only the reply");
}

#[test]
fn a_frame_with_nothing_new_redraws_nothing() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    rebuilds();
    cached_transcript(&mut state, 40);
    assert_eq!(rebuilds(), 0);
}

#[test]
fn a_resize_redraws_every_message() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    rebuilds();
    cached_transcript(&mut state, 60);
    assert_eq!(rebuilds(), 6);
}

/// A message going away has to take its lines and its separator with it, or
/// the cached prefix no longer lines up with `starts`.
#[test]
fn popping_a_message_drops_its_lines_and_keeps_the_rest() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    state.messages.pop();
    rebuilds();

    let popped = cached_transcript(&mut state, 40).lines.clone();
    assert_eq!(state.transcript_cache.as_ref().unwrap().len(), 5);
    assert_eq!(rebuilds(), 0, "the survivors were already drawn");
    assert_eq!(popped, transcript(&state.messages, 40).lines);
}

/// Expanding a tool result changes how many of its lines are drawn without
/// changing a byte of its content, so the key has to carry the flag.
#[test]
fn expanding_a_tool_result_redraws_it_and_what_follows() {
    let mut state = State::default();
    state.messages.push(Message::user("run it"));
    let mut result = Message::tool_result("call_1", "a line\n".repeat(40));
    result.full_output = false;
    state.messages.push(result);
    state.messages.push(Message::assistant("done"));
    let elided = cached_transcript(&mut state, 40).lines.len();
    rebuilds();

    state.messages[1].full_output = true;
    let full = cached_transcript(&mut state, 40).lines.clone();
    assert!(full.len() > elided, "every line is drawn now");
    assert_eq!(rebuilds(), 2, "the result and the message after it");
    assert_eq!(full, transcript(&state.messages, 40).lines);
}

/// The property that matters most: however the cache got there, what it
/// holds is what a from-scratch render would have produced.
#[test]
fn the_incremental_transcript_matches_a_full_one() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    for token in ["\n\n- one", ", two", "\n- three", "\n\n```\ncode"] {
        state.messages.last_mut().unwrap().content.push_str(token);
        let incremental = cached_transcript(&mut state, 40);
        let (lines, starts) = (incremental.lines.clone(), incremental.starts.clone());
        let full = transcript(&state.messages, 40);
        assert_eq!(lines, full.lines, "after {token:?}");
        assert_eq!(starts, full.starts, "after {token:?}");
    }
}

/// The pane title is asserted to the host with `rename_plugin_pane`, which
/// costs a screen repaint and a session-state report fanned out to every
/// plugin. `last_pane_title` is what stops a render restating an unchanged
/// one — ten times a second while streaming, and once per keystroke.
#[test]
fn the_pane_title_is_only_asserted_when_it_changes() {
    let mut state = State::default();
    assert_eq!(state.last_pane_title, None, "nothing asserted yet");

    render_title(&mut state, RENDER_COLS);
    let asserted = state.last_pane_title.clone();
    assert!(asserted.is_some(), "the first render names the pane");

    render_title(&mut state, RENDER_COLS);
    assert_eq!(
        state.last_pane_title, asserted,
        "and the next one has nothing new to say"
    );

    // Plan mode is in the title, so turning it on has to reach the host.
    state.plan = Plan::Planning;
    render_title(&mut state, RENDER_COLS);
    assert_ne!(state.last_pane_title, asserted, "the title moved");
}

/// `on_chunk` swaps the streamed prose onto a tool-calls message, which can
/// leave the count and every length where they were. Nothing but the
/// explicit invalidation can catch that.
#[test]
fn an_invalidated_message_is_redrawn_even_when_its_key_did_not_move() {
    let mut state = a_conversation();
    cached_transcript(&mut state, 40);
    // Same length, different text — invisible to the key by construction.
    let last = state.messages.last_mut().unwrap();
    last.content = "x".repeat(last.content.len());
    rebuilds();

    state.transcript_cache.as_mut().unwrap().invalidate_from(5);
    let redrawn = cached_transcript(&mut state, 40).lines.clone();
    assert_eq!(rebuilds(), 1);
    assert_eq!(redrawn, transcript(&state.messages, 40).lines);
}

#[test]
fn the_plan_cache_reuses_only_the_same_render_key() {
    let mut state = State::default();
    state.messages.push(Message::assistant("the original plan"));
    cached_plan(&mut state, 40, 0);
    state.plan_cache.as_mut().unwrap().1[0].text = "cached".to_owned();

    assert_eq!(cached_plan(&mut state, 40, 0)[0].text, "cached");
    assert_ne!(cached_plan(&mut state, 30, 0)[0].text, "cached");

    state.plan_cache.as_mut().unwrap().1[0].text = "cached".to_owned();
    state.messages.push(Message::notice("x"));
    assert_ne!(cached_plan(&mut state, 30, 0)[0].text, "cached");

    state
        .messages
        .push(Message::assistant("a replacement plan"));
    assert_eq!(cached_plan(&mut state, 30, 2)[0].text, "a replacement plan");
}

#[test]
fn a_notice_after_a_plan_does_not_empty_the_plan_box() {
    let state = State {
        plan: crate::Plan::AwaitingApproval,
        messages: vec![
            Message::assistant("the plan body"),
            Message::notice("the help notice"),
        ],
        ..Default::default()
    };
    let prompt = state.confirmation_prompt().expect("the prompt is up");
    let at = plan_at(&state, Some(&prompt)).expect("the plan is still selected");
    let transcript = transcript(&state.messages, 40);

    assert_eq!(at, 0);
    assert_eq!(
        transcript.lines[transcript.starts[at]].text,
        "the plan body"
    );
    assert!(transcript.lines[transcript.starts[at + 1]..]
        .iter()
        .any(|line| line.text.contains("the help notice")));
}

#[test]
fn confirmation_help_carries_the_answers_without_the_box() {
    let mut state = State {
        status: Status::ConfirmingTool,
        ..Default::default()
    };
    let help = help_line(&state);
    assert!(help.starts_with("<y> yes  <n> no"));

    state.plan = crate::Plan::AwaitingApproval;
    let help = help_line(&state);
    assert!(help.starts_with("</execute> run  </execute --clear> fresh  <n> refine"));
}

#[test]
fn idle_help_names_the_reposition_key() {
    let state = State {
        status: Status::Idle,
        ..Default::default()
    };
    assert!(help_line(&state).contains("<Ctrl e o> move"));
}

/// Bare Up and Down belong to the box's prompt history in every one of these, so the keys
/// the line offers for the transcript are the page keys - as the confirmation's line and
/// `/keys` say.
#[test]
fn busy_help_names_the_page_keys_not_the_arrows() {
    let busy = [
        Status::AwaitingCommand,
        Status::AwaitingSearch,
        Status::AwaitingDecisions,
        Status::AwaitingSwamp {
            server_url: "https://lab.example.test".into(),
        },
        Status::AwaitingExtension,
        Status::AwaitingPluginTest,
        Status::AwaitingPluginLoad,
        Status::AwaitingRetry,
    ];
    let streaming = State {
        stream: Some(crate::StreamState {
            id: "hocus-1".into(),
            finishing: false,
            usage: None,
        }),
        ..Default::default()
    };
    let states = busy
        .into_iter()
        .map(|status| State {
            status,
            ..Default::default()
        })
        .chain([streaming]);
    for state in states {
        let help = help_line(&state);
        assert!(help.contains("<PgUp/PgDn> scroll"), "{help}");
        assert!(!help.contains("<↑↓>"), "{help}");
    }
}

#[test]
fn transcript_keeps_the_role_on_continuation_lines() {
    let lines = transcript_lines(&[Message::error("aaaa bbbb")], 9);
    assert!(lines.iter().all(|l| l.role == Role::Error));
}

// -- where each message starts ------------------------------------------

#[test]
fn every_message_gets_a_start_line() {
    let transcript = transcript(
        &[
            Message::user("aaaa bbbb cccc"),
            Message::assistant("dddd"),
            Message::error("eeee"),
        ],
        9,
    );
    assert_eq!(transcript.starts.len(), 3);
    assert_eq!(transcript.starts[0], 0);
    assert!(transcript.starts.windows(2).all(|pair| pair[0] < pair[1]));
}

/// The anchor puts this line on the top row, so it has to be the reply
/// itself rather than the blank line separating it from the question.
#[test]
fn a_start_line_is_the_first_content_line_not_the_separator() {
    let transcript = transcript(&[Message::user("hi"), Message::assistant("yo")], 40);
    let start = transcript.starts[1];
    assert_eq!(transcript.lines[start].text, "yo");
    assert!(transcript.lines[start - 1].text.is_empty());
}

/// The placeholder a reply streams into is empty, and the spinner renders on
/// it. Its start is that line, which is where the first token lands.
#[test]
fn the_streaming_placeholder_still_gets_a_start_line() {
    let transcript = transcript(&[Message::user("hi"), Message::assistant("")], 40);
    assert_eq!(transcript.starts.len(), 2);
    assert_eq!(transcript.starts[1], transcript.lines.len() - 1);
}

// -- the transcript window ----------------------------------------------

#[test]
fn without_an_anchor_the_window_is_the_requested_offset_clamped() {
    assert_eq!(transcript_window(100, 10, 0, None).offset, 0);
    assert_eq!(transcript_window(100, 10, 25, None).offset, 25);
    assert_eq!(transcript_window(100, 10, 999, None).offset, 90);
    assert_eq!(transcript_window(4, 10, 3, None).offset, 0);
}

/// Until the reply outgrows the pane there is nothing to protect, so the
/// viewport goes on following the tail exactly as it always did.
#[test]
fn a_reply_that_fits_the_pane_still_follows_the_tail() {
    // 30 lines, a 10-row pane, the reply starting at line 25: five lines of
    // it so far, so the whole thing is on screen with room to spare.
    let window = transcript_window(30, 10, 0, Some(25));
    assert_eq!(window.offset, 0);
    assert_eq!(window.start, 20);
    assert!(!window.frozen);
}

/// The frame the reply fills the pane exactly: its first line has just
/// reached the top row, and the next line is what would push it off.
#[test]
fn a_reply_exactly_as_tall_as_the_pane_is_pinned_and_frozen() {
    let window = transcript_window(30, 10, 0, Some(20));
    assert_eq!(window.offset, 0);
    assert_eq!(window.start, 20);
    assert!(window.frozen);
}

/// The whole point: the reply goes on growing and its opening line does not
/// move off the top row.
#[test]
fn a_growing_reply_stops_at_the_line_it_started_on() {
    for grown in 0..40 {
        let window = transcript_window(30 + grown, 10, 0, Some(20));
        assert_eq!(window.start, 20, "reply grown by {grown} lines");
        assert_eq!(window.end - window.start, 10);
        assert_eq!(window.offset, grown);
        assert!(window.frozen);
    }
}

/// The offset that would hold the anchor depends on the height, so a stored
/// one goes stale the moment the picker opens or the input box grows. The
/// anchor is re-resolved instead, which is why `requested` is ignored.
#[test]
fn the_anchor_survives_the_viewport_changing_height() {
    assert_eq!(transcript_window(60, 10, 0, Some(20)).start, 20);
    let shrunk = transcript_window(60, 5, 30, Some(20));
    assert_eq!(shrunk.start, 20);
    assert_eq!(transcript_window(60, 10, shrunk.offset, Some(20)).start, 20);
}

/// A transcript that is all reply: nothing to hide above it, everything to
/// hide below.
#[test]
fn an_anchor_on_the_first_line_hides_only_what_is_below() {
    let window = transcript_window(100, 10, 0, Some(0));
    assert_eq!(window.start, 0);
    assert_eq!(window.offset, 90);
    assert!(window.frozen);
}

#[test]
fn an_anchor_past_the_end_cannot_scroll_past_the_bottom() {
    let window = transcript_window(30, 10, 0, Some(999));
    assert_eq!(window.offset, 0);
    assert_eq!(window.end, 30);
    assert!(!window.frozen);
}

/// A pane too short to hold the transcript's first line still has to draw
/// something rather than underflow.
#[test]
fn a_viewport_taller_than_the_transcript_shows_all_of_it() {
    let window = transcript_window(4, 10, 0, Some(2));
    assert_eq!((window.start, window.end, window.offset), (0, 4, 0));
}

/// The two halves together, over a reply arriving a word at a time: the
/// anchored message index resolves through `starts` to a line that stays on
/// the top row however much more text lands under it.
#[test]
fn a_streamed_reply_keeps_its_first_line_on_the_top_row() {
    const HEIGHT: usize = 6;
    let mut messages = vec![Message::user("count"), Message::assistant("")];
    let mut tops = Vec::new();

    for word in 1..20 {
        messages[1].content.push_str(&format!("line {word}\n\n"));
        let transcript = transcript(&messages, 40);
        let window = transcript_window(
            transcript.lines.len(),
            HEIGHT,
            0,
            Some(transcript.starts[1]),
        );
        tops.push(transcript.lines[window.start].text.clone());
    }

    // Followed the tail while the reply was short — the question was still
    // on screen above it — then stopped dead on the reply's first line
    // rather than scrolling it away.
    assert!(tops[0].trim_end().ends_with("count"), "{:?}", tops[0]);
    assert_eq!(tops.last().unwrap(), "line 1");
    assert!(tops.iter().rev().take(12).all(|top| top == "line 1"));
}

// -- the empty-chat wand -------------------------------------------------

/// Big enough for the image and a prompt line under it.
const WAND_HEIGHT: usize = wand::MIN_ROWS + 2;
const WAND_COLS: usize = wand::MIN_COLS + 6;

fn empty_state(kitty_graphics: bool) -> State {
    let mut state = State::default();
    if kitty_graphics {
        state.kitty_graphics.insert(state.client);
    }
    state
}

#[test]
fn an_empty_chat_draws_the_wand_and_keeps_its_own_clock_running() {
    let mut state = empty_state(true);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(
        state.wand_drawing,
        "the splash has to ask for the next frame"
    );
    // The pixels went over once, for this client. Every later frame is placeholder cells.
    assert!(state.wand_transmitted.contains(&state.client));
    assert_eq!(
        wand_height(&state, WAND_HEIGHT, WAND_COLS),
        wand::WAND_ROWS + 1
    );
}

#[test]
fn the_frames_are_transmitted_once_per_client_not_once_per_render() {
    let mut state = empty_state(true);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    let after_first = state.wand_transmitted.clone();
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert_eq!(state.wand_transmitted, after_first);
}

#[test]
fn client_grid_reset_invalidates_only_its_wand_after_batched_capabilities() {
    let mut state = empty_state(false);
    let first = ClientId::new(1);
    let other = ClientId::new(2);
    for client in [first, other] {
        state.client = client;
        state.kitty_graphics.insert(client);
        render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    }
    state.client = first;
    // Reconnection can batch the initial false capability and the probe's true answer
    // before either causes a render. Neither update invalidates the transmitted pixels.
    for supported in [false, true] {
        let mut mode = yinz_tile::prelude::ModeInfo::default();
        mode.capabilities.kitty_graphics = supported;
        state.handle(Event::ModeUpdate(mode));
    }
    assert_eq!(state.wand_transmitted, BTreeSet::from([first, other]));
    // Pending-plugin replay may run under a different acting client; use the payload.
    state.client = other;
    assert!(state.handle(Event::ClientGridReset(first)));
    assert_eq!(state.wand_transmitted, BTreeSet::from([other]));
    assert_eq!(state.kitty_graphics, BTreeSet::from([first, other]));
    state.client = first;
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert_eq!(state.wand_transmitted, BTreeSet::from([first, other]));
}

#[test]
fn kitty_graphics_support_is_scoped_to_the_rendering_client() {
    let mut state = State::default();
    state.kitty_graphics.insert(ClientId::new(1));

    state.client = ClientId::new(1);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_transmitted.contains(&ClientId::new(1)));

    state.client = ClientId::new(2);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert_eq!(wand_form(&state, WAND_HEIGHT, WAND_COLS), Some(Wand::Text));
    assert!(!state.wand_transmitted.contains(&ClientId::new(2)));
}

#[test]
fn a_terminal_without_kitty_graphics_gets_the_text_wand_instead() {
    let mut state = empty_state(false);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_drawing);
    // Nothing was transmitted, because nothing could have been displayed.
    assert!(state.wand_transmitted.is_empty());
    assert_eq!(
        wand_height(&state, WAND_HEIGHT, WAND_COLS),
        wand::fallback_lines(0).len() + 1
    );
}

/// The capability arrives after the first render and can go either way - a client attaching
/// over mosh onto a recycled id is told `true` and then corrected. The image has to be
/// released when that happens, or `wand_transmitted` stops meaning "clients currently showing
/// the image" and a later flip back would print placeholders that refer to nothing.
#[test]
fn a_capability_that_turns_off_mid_session_releases_the_image() {
    let mut state = empty_state(true);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_transmitted.contains(&state.client));

    state.kitty_graphics.remove(&state.client);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert_eq!(wand_form(&state, WAND_HEIGHT, WAND_COLS), Some(Wand::Text));
    assert!(state.wand_transmitted.is_empty());

    // And turning back on re-transmits rather than referring to images it no longer holds.
    state.kitty_graphics.insert(state.client);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_transmitted.contains(&state.client));
}

#[test]
fn a_pane_that_shrinks_below_the_image_releases_it() {
    let mut state = empty_state(true);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_transmitted.contains(&state.client));

    render_transcript(&mut state, 0, wand::MIN_ROWS - 1, WAND_COLS);
    assert!(state.wand_transmitted.is_empty());
}

#[test]
fn a_pane_too_short_for_the_image_falls_back_to_the_text_wand() {
    let mut state = empty_state(true);
    let short = wand::MIN_ROWS - 1;
    render_transcript(&mut state, 0, short, WAND_COLS);
    assert_eq!(wand_form(&state, short, WAND_COLS), Some(Wand::Text));
    assert!(state.wand_drawing);
    // Nothing was transmitted: the image was never going to be drawn.
    assert!(state.wand_transmitted.is_empty());
}

#[test]
fn a_pane_too_small_for_either_wand_is_as_bare_as_it_ever_was() {
    let mut state = empty_state(true);
    render_transcript(&mut state, 0, 3, WAND_COLS);
    assert_eq!(wand_form(&state, 3, WAND_COLS), None);
    assert!(!state.wand_drawing, "nothing drawn, so nothing to animate");
    assert!(state.wand_transmitted.is_empty());
}

#[test]
fn an_early_render_return_clears_a_stale_wand_clock() {
    let mut state = State {
        wand_drawing: true,
        ..Default::default()
    };
    super::render(&mut state, 1, 1);
    assert!(!state.wand_drawing);

    state.wand_drawing = true;
    state.perms = true;
    super::render(&mut state, WAND_HEIGHT, WAND_COLS);
    assert!(
        !state.wand_drawing,
        "an invalid configuration renders its banner instead of the wand"
    );
}

#[test]
fn the_first_message_stops_the_wand_and_releases_each_clients_images() {
    let mut state = empty_state(true);
    state.client = ClientId::new(1);
    state.kitty_graphics.insert(ClientId::new(1));
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    state.client = ClientId::new(2);
    state.kitty_graphics.insert(ClientId::new(2));
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_drawing);
    assert_eq!(
        state.wand_transmitted,
        BTreeSet::from([ClientId::new(1), ClientId::new(2)])
    );

    state.messages.push(Message::user("hello"));
    state.client = ClientId::new(1);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(!state.wand_drawing);
    assert_eq!(state.wand_transmitted, BTreeSet::from([ClientId::new(2)]));

    state.client = ClientId::new(2);
    render_transcript(&mut state, 0, WAND_HEIGHT, WAND_COLS);
    assert!(state.wand_transmitted.is_empty());
}

#[test]
fn the_text_wand_actually_animates() {
    let frames: std::collections::BTreeSet<Vec<String>> =
        (0..12).map(wand::fallback_lines).collect();
    assert!(frames.len() > 1, "every frame drew the same thing");
    // Every frame is the same shape, so the block does not jump about as it twinkles.
    let widths: std::collections::BTreeSet<usize> = frames
        .iter()
        .flat_map(|frame| frame.iter().map(|line| line.chars().count()))
        .collect();
    assert_eq!(widths, BTreeSet::from([10]));
}

// -- rendering the window -----------------------------------------------

/// `render_transcript` only ever prints, so a test can drive the real thing
/// and read the viewport it settled on back off the state.
const RENDER_HEIGHT: usize = 6;
const RENDER_COLS: usize = 40;

/// A question, then a reply four times taller than the pane.
fn state_mid_reply() -> State {
    let mut state = State {
        messages: vec![
            Message::user("count"),
            Message::assistant(
                (1..=12)
                    .map(|n| format!("line {n}\n\n"))
                    .collect::<String>(),
            ),
        ],
        ..Default::default()
    };
    state.view.anchor = Some(1);
    state
}

/// Where the reply starts, which is where the anchor should hold the pane.
fn reply_start(state: &State) -> usize {
    transcript(&state.messages, RENDER_COLS).starts[1]
}

fn render(state: &mut State) {
    render_transcript(state, 0, RENDER_HEIGHT, RENDER_COLS);
}

/// The offset the render settled on is the one the scroll keys then work
/// from. Nothing else knows it: the anchor and the clamp are applied here.
#[test]
fn rendering_records_the_viewport_it_settled_on() {
    let mut state = state_mid_reply();
    let total = transcript(&state.messages, RENDER_COLS).lines.len();

    render(&mut state);

    assert_eq!(
        total - state.scroll_offset() - RENDER_HEIGHT,
        reply_start(&state),
        "the reply's first line is on the top row"
    );
}

/// The render measures the transcript; once the client is at the top, more
/// notches change nothing and the plugin says so rather than re-rendering
/// the same frame for each of them.
#[test]
fn scrolling_up_past_the_top_does_not_ask_for_a_render() {
    let mut state = state_mid_reply();
    state.view.anchor = None;
    render(&mut state);
    let total = transcript(&state.messages, RENDER_COLS).lines.len();

    assert!(state.scroll_up(100), "the first scroll moves");
    assert_eq!(
        state.scroll_offset(),
        total - RENDER_HEIGHT,
        "clamped to the top"
    );
    assert!(!state.scroll_up(1), "already at the top");
    assert!(!state.on_mouse(Mouse::ScrollUp(3)), "the wheel too");
    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::PageUp)),
        "and PageUp"
    );

    render(&mut state);
    assert_eq!(
        state.scroll_offset(),
        total - RENDER_HEIGHT,
        "the render agrees"
    );
}

/// A transcript that grows re-renders, and the render moves the top.
#[test]
fn a_grown_transcript_lets_you_scroll_further() {
    let mut state = state_mid_reply();
    state.view.anchor = None;
    render(&mut state);
    assert!(state.scroll_up(100));
    assert!(!state.scroll_up(1), "at the top");

    state.messages.push(Message::user("and then?"));
    render(&mut state);

    assert!(state.scroll_up(1), "there is more above now");
}

/// The reply goes on arriving and the pane does not move.
#[test]
fn a_reply_growing_under_a_frozen_pane_does_not_move_it() {
    let mut state = state_mid_reply();
    render(&mut state);
    let before = state.scroll_offset();

    state.messages[1].content.push_str("and another\n\n");
    render(&mut state);

    let total = transcript(&state.messages, RENDER_COLS).lines.len();
    assert!(state.scroll_offset() > before, "more is hidden below");
    assert_eq!(
        total - state.scroll_offset() - RENDER_HEIGHT,
        reply_start(&state),
        "but the top row is the same line it was"
    );
}

/// What the write-back is for. Without it the client reads as pinned to the
/// bottom, and this PageDown would saturate at 0 and drop them at the end of
/// the reply — the trip back up the anchor exists to save.
#[test]
fn the_first_page_down_after_a_freeze_moves_one_page() {
    let mut state = state_mid_reply();
    render(&mut state);
    let frozen = state.scroll_offset();
    assert!(frozen > 0, "the pane is frozen to begin with");

    state.scroll_down(RENDER_HEIGHT);

    assert_eq!(state.scroll_offset(), frozen - RENDER_HEIGHT);
}

/// Scrolling is how you say you don't want the anchor. It does not come back
/// and take the pane again on the next frame.
#[test]
fn a_client_that_scrolled_is_not_dragged_back_by_the_anchor() {
    let mut state = state_mid_reply();
    state.scroll_up(3);

    render(&mut state);

    assert_eq!(state.scroll_offset(), 3);
}

/// The `run? …` prompt renders below the transcript. Freezing an older line
/// on the top row would leave it off the bottom of the pane, and the tool
/// waits on a keypress nothing on screen has asked for.
#[test]
fn a_pending_confirmation_is_never_frozen_off_screen() {
    let mut state = state_mid_reply();
    state.status = Status::ConfirmingTool;
    state.pending = vec![crate::ToolCall {
        id: "a".into(),
        name: "close_pane".into(),
        arguments: "{}".into(),
    }]
    .into();
    assert!(state.confirmation_prompt().is_some(), "the prompt is up");

    render(&mut state);

    assert_eq!(state.scroll_offset(), 0, "pinned to the bottom instead");

    state.on_key(KeyWithModifier::new(BareKey::PageUp));
    render(&mut state);
    assert!(
        state.scroll_offset() > 0,
        "the client may read older output"
    );
    assert!(help_line(&state).contains("<y> yes  <n> no"));

    state.on_mouse(Mouse::ScrollUp(1));
    render(&mut state);
    assert!(
        state.scroll_offset() > 0,
        "mouse scrolling behaves the same way"
    );
    assert!(help_line(&state).contains("<y> yes  <n> no"));
}

#[test]
fn a_tall_confirmation_scrolls_through_the_transcript_viewport() {
    let source = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut state = State {
        status: Status::ConfirmingTool,
        ..Default::default()
    };
    state.pending.push_back(crate::ToolCall {
        id: "a".into(),
        name: "plugin_write".into(),
        arguments: serde_json::json!({ "name": "keys", "source": source }).to_string(),
    });

    render(&mut state);
    assert_eq!(state.scroll_offset(), 0, "the footer starts in view");
    assert!(state.scroll_up(usize::MAX));
    render(&mut state);
    assert!(!state.scroll_up(1), "the full prompt has a reachable top");
    assert!(state.scroll_offset() > 0);

    state.scroll_down(usize::MAX);
    render(&mut state);
    assert_eq!(
        state.scroll_offset(),
        0,
        "and its footer is reachable again"
    );
    assert!(help_line(&state).contains("<PgUp/PgDn> scroll"));
}

/// Every other client goes on being held by it.
#[test]
fn one_client_scrolling_does_not_release_another() {
    let mut state = state_mid_reply();
    state.client = ClientId::new(1);
    state.scroll_up(3);
    render(&mut state);
    assert_eq!(state.scroll_offset(), 3);

    state.client = ClientId::new(2);
    render(&mut state);

    let total = transcript(&state.messages, RENDER_COLS).lines.len();
    assert_eq!(
        total - state.scroll_offset() - RENDER_HEIGHT,
        reply_start(&state)
    );
}

/// The pane the anchor was measured against is gone, and the offset that
/// held it is wrong for the new one. Re-resolving the message index is what
/// makes that not matter.
#[test]
fn the_anchor_survives_the_pane_being_resized_under_it() {
    let mut state = state_mid_reply();
    render(&mut state);

    render_transcript(&mut state, 0, RENDER_HEIGHT * 2, RENDER_COLS);

    let total = transcript(&state.messages, RENDER_COLS).lines.len();
    assert_eq!(
        total - state.scroll_offset() - RENDER_HEIGHT * 2,
        reply_start(&state)
    );
}

/// A reply still short enough to fit is nothing to protect, and the pane
/// goes on following the tail exactly as it always did.
#[test]
fn an_anchored_reply_that_fits_still_follows_the_tail() {
    let mut state = State {
        messages: vec![Message::user("hi"), Message::assistant("yo")],
        ..Default::default()
    };
    state.view.anchor = Some(1);

    render(&mut state);

    assert_eq!(state.scroll_offset(), 0);
}

// -- the waiting indicator ----------------------------------------------

#[test]
fn the_spinner_walks_every_frame_and_wraps() {
    let frames: Vec<char> = (0..SPINNER.len() as u64).map(spinner).collect();
    assert_eq!(frames, SPINNER);
    assert_eq!(spinner(SPINNER.len() as u64), SPINNER[0]);
}

/// Every frame must be one column wide or the text beside it jitters.
#[test]
fn every_spinner_frame_is_one_column_wide() {
    for frame in SPINNER {
        assert_eq!(frame.width(), Some(1), "{frame:?} is not one column");
    }
}

#[test]
fn a_phrase_holds_until_its_ticks_run_out() {
    let first = phrase(0, 0);
    assert_eq!(phrase(TICKS_PER_PHRASE - 1, 0), first, "still the same");
    assert_ne!(phrase(TICKS_PER_PHRASE, 0), first, "now the next one");
}

#[test]
fn phrases_cycle_through_the_whole_pool_and_wrap() {
    let seen: Vec<&str> = (0..PHRASES.len() as u64)
        .map(|step| phrase(step * TICKS_PER_PHRASE, 0))
        .collect();
    assert_eq!(seen, PHRASES, "in order, no repeats");
    assert_eq!(
        phrase(PHRASES.len() as u64 * TICKS_PER_PHRASE, 0),
        PHRASES[0]
    );
}

#[test]
fn consecutive_requests_open_on_different_phrases() {
    assert_ne!(phrase(0, 1), phrase(0, 2));
}

/// Control characters are stripped during wrapping, so a phrase containing
/// one would silently lose it.
#[test]
fn no_phrase_contains_a_control_character() {
    for text in PHRASES {
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }
}

/// The loader is appended after wrapping, so it is truncated rather than
/// wrapped if it overruns the pane. This is the width budget that keeps it
/// whole in a 40-column pane: the spinner and its space.
#[test]
fn every_phrase_fits_beside_the_spinner() {
    let budget = 40 - 2;
    for text in PHRASES {
        assert!(text.width() <= budget, "{text:?} is {} wide", text.width());
    }
}

// -- loader placement ---------------------------------------------------

#[test]
fn the_empty_placeholder_is_recognised_as_the_loaders_home() {
    let lines = transcript_lines(&[Message::user("hi"), Message::assistant("")], 40);
    assert!(ends_with_empty_reply(&lines));
}

#[test]
fn a_reply_with_text_is_not_the_loaders_home() {
    let lines = transcript_lines(&[Message::user("hi"), Message::assistant("word")], 40);
    assert!(!ends_with_empty_reply(&lines));
}

/// What the transcript looks like while a tool runs: the last message is the
/// assistant's tool-call summary, so the loader needs its own line.
#[test]
fn a_tool_call_summary_is_not_the_loaders_home() {
    let lines = transcript_lines(&[Message::assistant("list_session {}")], 40);
    assert!(!ends_with_empty_reply(&lines));
}

// -- the tab bar ----------------------------------------------------------

fn chat_named(first: &str) -> crate::Chat {
    crate::Chat {
        messages: vec![Message::user(first)],
        ..Default::default()
    }
}

/// `n` open chats titled by number, the first on screen.
fn tabs_state(n: usize) -> State {
    let mut state = State {
        messages: vec![Message::user("chat 0")],
        ..Default::default()
    };
    for i in 1..n {
        state.parked.push(chat_named(&format!("chat {i}")));
    }
    state
}

fn texts(cells: &[TabCell]) -> Vec<&str> {
    cells.iter().map(|cell| cell.text.as_str()).collect()
}

fn job(id: u64) -> crate::message::Job {
    crate::message::Job {
        id,
        every: 60.0,
        due_in: 60.0,
        prompt: "check".to_owned(),
        show: false,
        auto_confirm: false,
    }
}

/// A job in another tab is work happening off-screen, and the note says so
/// rather than counting only what the foreground armed.
#[test]
fn the_title_note_sees_jobs_in_every_tab() {
    let mut state = tabs_state(2);
    assert_eq!(title_note(&state), None, "nothing happening anywhere");

    state.jobs.push(job(1));
    assert_eq!(title_note(&state).as_deref(), Some("⏲ 1"));

    state.parked[0].jobs.push(job(2));
    assert_eq!(
        title_note(&state).as_deref(),
        Some("+1"),
        "the other tab is live, and counted"
    );
    assert!(chat_tabs(&state, 80)[1].text.starts_with(" ● "));
}

/// The stream the foreground is running is the help line's to report, beside
/// the key that cancels it. The top row says only what the bottom cannot see:
/// the other chat that is also working.
#[test]
fn the_title_note_leaves_streaming_to_the_help_line() {
    let mut state = tabs_state(2);
    state.stream = Some(crate::StreamState {
        id: "hocus-1".into(),
        finishing: false,
        usage: None,
    });
    assert_eq!(title_note(&state), None, "nothing else is working");
    assert!(help_line(&state).starts_with("streaming…"));

    state.parked[0].jobs.push(job(2));
    assert_eq!(title_note(&state).as_deref(), Some("+1"));
}

#[test]
fn usage_distinguishes_last_request_from_totals_without_spending_tab_width() {
    let mut state = tabs_state(2);
    assert_eq!(usage_note(&state), "");
    state.last_request_input = Some(0);
    assert_eq!(usage_note(&state), "last in 0");
    state.last_request_input = Some(400);
    assert_eq!(usage_note(&state), "last in 400");
    state.usage = Some(crate::TokenUsage {
        input_tokens: 1200,
        output_tokens: 34,
        cost_nanos: Some(12_300_000),
    });
    assert_eq!(
        usage_note(&state),
        "last in 400 · 1.2k in · 34 out · $0.0123"
    );
    assert!(usage_note(&state).width() < 80);
    state.last_request_input = None;
    assert_eq!(usage_note(&state), "1.2k in · 34 out · $0.0123");
    state.usage.as_mut().unwrap().cost_nanos = None;
    assert_eq!(usage_note(&state), "1.2k in · 34 out");
    state.usage.as_mut().unwrap().cost_nanos = Some(0);
    assert_eq!(usage_note(&state), "1.2k in · 34 out · $0");
    render_title(&mut state, 80);
    assert_eq!(state.chat_tab_width, 80);
}

#[test]
fn a_single_chat_is_one_active_cell() {
    let state = State::default();
    let cells = chat_tabs(&state, 40);
    assert_eq!(texts(&cells), [" new chat │"], "an empty chat is named");
    assert!(cells[0].active);
    assert!(!cells[0].focused);
    assert_eq!(cells[0].x, 0);
}

#[test]
fn cells_carry_a_glyph_for_what_the_chat_is_doing() {
    let mut state = tabs_state(3);
    state.stream = Some(crate::StreamState {
        id: "hocus-1".into(),
        finishing: false,
        usage: None,
    });
    state.parked[1].status = Status::ConfirmingTool;
    let cells = chat_tabs(&state, 60);
    assert_eq!(
        texts(&cells),
        [" ● chat 0 │", " chat 1 │", " ? chat 2 │"],
        "streaming, idle, waiting on y/n"
    );
}

#[test]
fn cells_sit_side_by_side_and_never_pass_the_width() {
    let state = tabs_state(4);
    let cells = chat_tabs(&state, 80);
    let mut x = 0;
    for cell in &cells {
        assert_eq!(cell.x, x);
        assert_eq!(cell.width, cell.text.width());
        x += cell.width;
    }
    assert!(x <= 80);

    for width in 0..80 {
        let cells = chat_tabs(&state, width);
        let end = cells.last().map_or(0, |cell| cell.x + cell.width);
        assert!(end <= width, "{width} columns, drawn to {end}");
    }
}

#[test]
fn a_long_title_is_clipped_with_an_ellipsis() {
    let state = State {
        messages: vec![Message::user(
            "a question that goes on for a very long time indeed",
        )],
        ..Default::default()
    };
    let cells = chat_tabs(&state, 80);
    assert_eq!(texts(&cells), [" a question that go… │"]);
    assert!(cells[0].width <= TAB_TITLE_COLS + 4);
}

#[test]
fn the_window_keeps_the_highlighted_tab_on_screen() {
    // Every cell is " chat N │" = 9 columns; room for two and an arrow.
    let mut state = tabs_state(5);
    let cells = chat_tabs(&state, 20);
    assert_eq!(
        texts(&cells),
        [" chat 0 │", " chat 1 │", "→│"],
        "the active tab first"
    );
    assert_eq!(cells[2].target, TabTarget::Reveal(2));

    state.chat_tab_focus = Some(4);
    let cells = chat_tabs(&state, 20);
    assert_eq!(texts(&cells), ["←│", " chat 3 │", " chat 4 │"]);
    assert_eq!(cells[0].target, TabTarget::Reveal(2));
    assert!(cells[2].focused);
    assert!(!cells[1].active);

    state.chat_tab_focus = Some(2);
    let cells = chat_tabs(&state, 22);
    assert_eq!(
        texts(&cells),
        ["←│", " chat 2 │", " chat 3 │", "→│"],
        "hidden tabs on both sides, growing rightward first"
    );
    let cells = chat_tabs(&state, 21);
    assert_eq!(
        texts(&cells),
        ["←│", " chat 2 │", "→│"],
        "and with room for only one"
    );
}

#[test]
fn the_active_cell_is_marked_and_the_highlight_can_be_elsewhere() {
    let mut state = tabs_state(3);
    state.chat_tab_focus = Some(2);
    let cells = chat_tabs(&state, 60);
    assert!(cells[0].active && !cells[0].focused);
    assert!(!cells[2].active && cells[2].focused);
}

#[test]
fn bar_focus_has_its_own_help_line() {
    let state = State {
        chat_tab_focus: Some(0),
        ..Default::default()
    };
    assert!(help_line(&state).starts_with("<←→> choose"));
}
