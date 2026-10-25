use super::*;

fn texts(lines: &[MdLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

/// The escaper's whole contract in one pass: every character the parser acts on comes
/// back inert, an identifier comes back intact, and a newline cannot end the row the
/// value is sitting in. Round-tripped through [`render`] as well as compared, because
/// what matters is not the backslashes but that nothing survives them as markup.
#[test]
fn escape_inline_neutralises_every_character_the_renderer_acts_on() {
    assert_eq!(escape_inline("pane_id"), "pane_id");
    assert_eq!(escape_inline("a|b~~c~~"), r"a\|b\~\~c\~\~");
    assert_eq!(escape_inline("a `b` c"), r"a \`b\` c");
    assert_eq!(escape_inline("one\ntwo"), "one two");

    let hostile = "# **x** ``` | y <b> [z](w) ~~struck~~";
    let lines = render(&escape_inline(hostile), 200, Options::default());
    assert_eq!(texts(&lines), [hostile], "nothing survived as markup");
}

/// The whole dialect in one pass: a heading is bold, emphasis and code take colours,
/// a list gets bullets, a quote gets a bar, a link keeps its destination, and a rule
/// fills the width.
#[test]
fn markdown_renders_as_terminal_blocks() {
    let source = "# Heading\n\nA **bold** and *slanted* `value`.\n\n- one\n- two\n\n> quoted\n\n[site](https://example.com)\n\n---";
    let lines = render(source, 80, Options::default());
    let rendered = texts(&lines).join("\n");

    assert!(rendered.contains("Heading"), "{rendered}");
    assert!(!rendered.contains('#'), "{rendered}");
    assert!(rendered.contains("A bold and slanted value."), "{rendered}");
    assert!(!rendered.contains("**"), "{rendered}");
    assert!(rendered.contains("• one"), "{rendered}");
    assert!(rendered.contains("• two"), "{rendered}");
    assert!(rendered.contains("│ quoted"), "{rendered}");
    assert!(
        rendered.contains("site (https://example.com)"),
        "{rendered}"
    );
    assert!(rendered.contains(&"─".repeat(80)), "{rendered}");

    // and the styles are one per character of the line they belong to
    for line in &lines {
        if line.styles.is_empty() {
            continue;
        }
        assert_eq!(line.styles.len(), line.text.chars().count(), "{line:?}");
    }
}

/// The heading is bold and coloured, and the word inside `**` is bold while the prose
/// around it is not - the thing markdown is actually for.
#[test]
fn emphasis_lands_on_the_characters_it_covers() {
    let lines = render("# Title\n\nplain **loud** plain", 40, Options::default());
    let heading = lines.iter().find(|line| line.text == "Title").unwrap();
    assert!(heading.styles.iter().all(|style| style.bold));

    let body = lines
        .iter()
        .find(|line| line.text.starts_with("plain loud"))
        .unwrap();
    let bold: String = body
        .text
        .chars()
        .zip(&body.styles)
        .filter(|(_, style)| style.bold)
        .map(|(ch, _)| ch)
        .collect();
    assert_eq!(bold, "loud");
}

/// A small pane pays for a paragraph break in the only rows it has, so it asks for the
/// blocks hard against each other. The lines themselves are the same either way.
#[test]
fn compact_blocks_drops_the_air_between_them() {
    let source = "first para\n\nsecond para";
    let roomy = render(source, 40, Options::default());
    let tight = render(source, 40, Options::compact());

    assert_eq!(texts(&roomy), ["first para", "", "second para"]);
    assert_eq!(texts(&tight), ["first para", "second para"]);
}

/// Text that renders to nothing is still one line: a caller counting rows never has to
/// special-case the empty answer.
#[test]
fn empty_input_is_one_empty_line() {
    assert_eq!(render("", 40, Options::default()), vec![MdLine::blank()]);
}

/// An escape sequence in a model's reply must not reach the parser, let alone the pane.
#[test]
fn control_characters_never_reach_the_parser() {
    let lines = render("a\u{1b}[31mb", 40, Options::default());
    assert_eq!(texts(&lines), ["a[31mb"]);
}

/// Wrapping is by column rather than by character, and the emphasis spanning a break
/// survives it.
#[test]
fn a_wrapped_block_keeps_its_styles_on_both_rows() {
    let lines = render("**one two three four five six**", 10, Options::default());
    assert!(lines.len() > 1, "{lines:?}");
    for line in &lines {
        assert!(line.text.width() <= 10, "{line:?}");
        assert!(line.styles.iter().all(|style| style.bold), "{line:?}");
    }
}

/// The margin a caller adds is part of the string, so the vector has to grow with it or
/// every range after it would be off by one.
#[test]
fn indent_and_prepend_keep_the_styles_aligned() {
    let line = MdLine {
        text: "body".to_owned(),
        styles: vec![
            MdStyle {
                bold: true,
                ..MdStyle::default()
            };
            4
        ],
    }
    .indent(1)
    .prepend(
        "label: ",
        MdStyle {
            dim: true,
            ..MdStyle::default()
        },
    );

    assert_eq!(line.text, "label:  body");
    assert_eq!(line.styles.len(), line.text.chars().count());
    assert!(line.styles[0].dim, "the label");
    assert!(!line.styles[7].bold, "the margin");
    assert!(line.styles[8].bold, "the body");

    // and a line with no styles of its own does not inherit the prefix's
    let plain = MdLine::plain("body").prepend(
        "x",
        MdStyle {
            dim: true,
            ..MdStyle::default()
        },
    );
    assert_eq!(plain.styles.len(), 5);
    assert!(plain.styles[0].dim);
    assert!(!plain.styles[1].dim);
}
/// A table's source pipes are gone and its cells sit under their headers, separated by a
/// bar, with one rule under the header and none between the rows.
#[test]
fn a_table_becomes_aligned_columns() {
    let source = "| Pane | Command | Status |\n|---|---|---|\n| 1 | zsh | running |\n| 12 | cargo | exited |";
    let lines = render(source, 60, Options::default());
    assert_eq!(
        texts(&lines),
        [
            "Pane │ Command │ Status ",
            "─────┼─────────┼────────",
            "1    │ zsh     │ running",
            "12   │ cargo   │ exited ",
        ]
    );
    for line in &lines {
        assert_eq!(line.styles.len(), line.text.chars().count(), "{line:?}");
    }
}

/// The header is bold and every border character is dim - the same weight the quote bar
/// and the horizontal rule are drawn at.
#[test]
fn the_header_is_bold_and_the_borders_are_dim() {
    let lines = render("| a | b |\n|---|---|\n| 1 | 2 |", 40, Options::default());
    // Bold everywhere but the bar, padding included, so the row is one style run.
    let header = &lines[0];
    assert!(
        header
            .text
            .chars()
            .zip(&header.styles)
            .all(|(ch, style)| style.bold || ch == '│'),
        "{header:?}"
    );
    let rule = &lines[1];
    assert!(rule.styles.iter().all(|style| style.dim), "{rule:?}");

    let body = &lines[2];
    let bars: Vec<MdStyle> = body
        .text
        .chars()
        .zip(&body.styles)
        .filter(|(ch, _)| *ch == '│')
        .map(|(_, style)| *style)
        .collect();
    assert!(!bars.is_empty());
    assert!(
        bars.iter().all(|style| style.dim && !style.bold),
        "{body:?}"
    );
}

/// Too wide for the pane: the columns shrink and a long cell wraps inside its own column
/// rather than being cut off. Every row still ends at the same place.
#[test]
fn a_wide_table_shrinks_and_wraps_its_cells() {
    let source = "| id | what |\n|---|---|\n| 1 | a rather long description that will not fit |";
    let lines = render(source, 24, Options::default());
    assert!(lines.len() > 3, "{lines:?}");
    for line in &lines {
        assert!(line.text.width() <= 24, "{line:?}");
    }
    // Nothing is lost: every word of the long cell is somewhere in the output.
    let rendered = texts(&lines).join(" ");
    for word in "a rather long description that will not fit".split(' ') {
        assert!(rendered.contains(word), "{word} missing from {rendered}");
    }
    // The narrow column keeps what it asked for rather than being squeezed with the wide
    // one - which is the point of fitting the columns narrowest first.
    assert!(lines[0].text.starts_with("id │"), "{:?}", lines[0]);
}

/// The delimiter row's colons decide which side the padding falls on.
#[test]
fn alignment_puts_the_pad_where_the_delimiter_asked() {
    let lines = render(
        "| l | c | r |\n|:---|:---:|---:|\n| x | x | x |",
        40,
        Options::default(),
    );
    // Columns are four wide, from the header's `---:` row being three and the cells one.
    assert_eq!(texts(&lines)[2], "x │ x │ x");

    let lines = render(
        "| left | mid | right |\n|:---|:---:|---:|\n| x | x | x |",
        40,
        Options::default(),
    );
    assert_eq!(texts(&lines)[2], "x    │  x  │     x");
}

/// A row with fewer cells than the header keeps its separators, and one with more keeps
/// the extra cell: the cell is content, the header only a label.
#[test]
fn a_ragged_row_is_padded_rather_than_dropped() {
    let lines = render(
        "| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 |",
        40,
        Options::default(),
    );
    let short = texts(&lines)[2];
    assert_eq!(short.matches('│').count(), 2, "{short}");
    for line in &lines {
        assert_eq!(line.text.width(), lines[0].text.width(), "{line:?}");
    }
}

/// A header with no rows under it is still a table, and a table with nothing in it is no
/// block at all - an empty block would still cost the blank line between two of them.
#[test]
fn a_header_only_table_draws_and_an_empty_one_does_not() {
    let lines = render("| a | b |\n|---|---|", 40, Options::default());
    assert_eq!(texts(&lines), ["a │ b", "──┼──"]);

    // A delimiter row on its own parses as prose, not as an empty table.
    let lines = render("text\n\n|---|---|\n\ntext", 40, Options::default());
    assert!(
        texts(&lines).iter().all(|text| !text.contains('┼')),
        "{lines:?}"
    );
}

/// Narrower than one column per column: the grid is abandoned rather than overflowed, and
/// each row is read as a record of labelled cells. Nothing runs past the edge, no cell
/// text is lost, and every value still says which column it came from.
#[test]
fn a_table_too_narrow_for_its_columns_falls_back_to_records() {
    let lines = render(
        "| aaa | bbb | ccc |\n|---|---|---|\n| 111 | 222 | 333 |",
        8,
        Options::default(),
    );
    assert_eq!(
        texts(&lines),
        ["aaa: 111", "bbb: 222", "ccc: 333"],
        "{lines:?}"
    );

    // Two records are told apart by a blank line, and a value too wide for the width is
    // wrapped rather than truncated.
    let lines = render(
        "| aaa | bbb |\n|---|---|\n| 111 | 222 |\n| a long value | 333 |",
        4,
        Options::default(),
    );
    let rendered = texts(&lines).join(" ");
    for cell in ["aaa", "bbb", "111", "222", "long", "333"] {
        assert!(rendered.contains(cell), "{cell} missing from {rendered}");
    }
    assert!(texts(&lines).contains(&""), "{lines:?}");
    for line in &lines {
        assert!(line.text.width() <= 4, "{line:?}");
    }
}

/// A table inside a quote carries the bar on every row of it, and one inside a list item
/// takes that item's bullet once rather than leaving an empty marker above itself.
#[test]
fn a_table_inherits_the_prefix_of_where_it_sits() {
    let lines = render(
        "> | a | b |\n> |---|---|\n> | 1 | 2 |",
        40,
        Options::default(),
    );
    assert_eq!(texts(&lines), ["│ a │ b", "│ ──┼──", "│ 1 │ 2"]);

    let lines = render(
        "- | a | b |\n  |---|---|\n  | 1 | 2 |",
        40,
        Options::default(),
    );
    assert_eq!(texts(&lines), ["• a │ b", "  ──┼──", "  1 │ 2"]);

    // A table that is not the item's first block has no empty bullet to take over, but it
    // still belongs to the item: it sits under the text rather than back at column zero.
    let lines = render(
        "- text\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |",
        40,
        Options::default(),
    );
    assert_eq!(
        texts(&lines),
        ["• text", "  a │ b", "  ──┼──", "  1 │ 2"],
        "{lines:?}"
    );
}

/// A cell is prose: the markup inside it is read the same way it is anywhere else, and
/// the emphasis lands on the characters it covers rather than on the whole cell.
#[test]
fn markup_inside_a_cell_is_read_as_markup() {
    let lines = render(
        "| what | where |\n|---|---|\n| a **loud** word | [site](https://example.com) |",
        60,
        Options::default(),
    );
    let body = &lines[2];
    assert!(!body.text.contains("**"), "{body:?}");
    assert!(body.text.contains("site (https://example.com)"), "{body:?}");
    let bold: String = body
        .text
        .chars()
        .zip(&body.styles)
        .filter(|(_, style)| style.bold)
        .map(|(ch, _)| ch)
        .collect();
    assert_eq!(bold, "loud");
}

/// Columns are measured in cells, not characters, so a wide character does not skew every
/// column to its right.
#[test]
fn wide_characters_in_a_cell_do_not_skew_the_column() {
    let lines = render(
        "| a | b |\n|---|---|\n| 日本語 | x |",
        40,
        Options::default(),
    );
    for line in &lines {
        assert_eq!(line.text.width(), lines[0].text.width(), "{line:?}");
    }
}

/// A pipe with no delimiter row under it is prose, and stays prose. This is the guard on
/// turning the table extension on at all.
#[test]
fn pipes_outside_a_table_are_still_prose() {
    assert_eq!(texts(&render("a | b", 40, Options::default())), ["a | b"]);
}

/// Strikethrough marks the characters it covers and nothing around them, the way every
/// other span does.
#[test]
fn strikethrough_marks_the_characters_it_covers() {
    let lines = render("plain ~~gone~~ plain", 40, Options::default());
    let line = &lines[0];
    assert_eq!(line.text, "plain gone plain");
    let struck: String = line
        .text
        .chars()
        .zip(&line.styles)
        .filter(|(_, style)| style.strike)
        .map(|(ch, _)| ch)
        .collect();
    assert_eq!(struck, "gone");
}

/// A task list draws its boxes in the body, so the bullet and the hanging indent the list
/// already has are left alone.
#[test]
fn a_task_list_draws_its_boxes() {
    let lines = render("- [x] done\n- [ ] todo", 40, Options::default());
    assert_eq!(texts(&lines), ["• ☒ done", "• ☐ todo"]);
}

/// One of each shape a block can take, and several of them long enough that a head is a
/// small part of the whole: prose with a word too long for any row, a code block, a table
/// wide enough to shrink, the same table too narrow for a grid, a quote, a list and a rule.
fn every_shape() -> String {
    let prose = "word ".repeat(60) + &"x".repeat(90) + " tail";
    let code = (0..30)
        .map(|n| format!("line {n} of the fence"))
        .collect::<Vec<_>>();
    let rows = (0..20)
        .map(|n| format!("| {n} | {} |", "cell text ".repeat(n % 4 + 1)))
        .collect::<Vec<_>>();
    format!(
            "# Heading\n\n{prose}\n\n```\n{}\n```\n\n| id | description |\n|---|---|\n{}\n\n> quoted {prose}\n\n- one\n- {prose}\n\n---\n\nend",
            code.join("\n"),
            rows.join("\n"),
        )
}

/// A head is the same rows the whole block starts with, for every shape and at widths from
/// one too narrow for a table's grid to wider than any row - so a caller that draws only
/// the head draws exactly what it would have cut the whole down to.
#[test]
fn a_blocks_head_is_the_start_of_the_whole_block() {
    let blocks = parse(&every_shape());
    for cols in [1, 3, 7, 20, 41, 200] {
        for block in &blocks {
            let whole = render_block(block, cols);
            for max_rows in [0, 1, 2, 3, 5, 8, whole.len(), whole.len() + 3] {
                let head = render_block_head(block, cols, max_rows);
                assert_eq!(
                    head,
                    whole[..max_rows.min(whole.len())],
                    "{max_rows} rows of {:?} at {cols}",
                    whole.first().map(|line| &line.text)
                );
            }
        }
    }
}

/// The public wrappers still wrap the whole of what they are given.
#[test]
fn the_wrappers_still_wrap_everything() {
    let chars: Vec<StyledChar> = "ab cd\nef gh"
        .chars()
        .map(|ch| StyledChar {
            ch,
            style: MdStyle::default(),
        })
        .collect();
    let rows = |wrapped: Vec<Vec<StyledChar>>| -> Vec<String> {
        wrapped
            .iter()
            .map(|row| row.iter().map(|styled| styled.ch).collect())
            .collect()
    };
    assert_eq!(rows(word_wrap_styled(&chars, 2)), ["ab", "cd", "ef", "gh"]);
    assert_eq!(
        rows(hard_wrap_styled(&chars, 3)),
        ["ab ", "cd", "ef ", "gh"]
    );
}

/// A head of a text is the whole parse's first blocks, and says whether there was more.
#[test]
fn a_parsed_head_is_the_start_of_the_whole_parse() {
    let text = every_shape();
    let whole = parse(&text);
    let drawn = |blocks: &[MdBlock]| -> Vec<Vec<MdLine>> {
        blocks.iter().map(|block| render_block(block, 30)).collect()
    };
    for max_blocks in 0..=whole.len() + 1 {
        let (head, complete) = parse_head(&text, max_blocks, usize::MAX);
        let kept = max_blocks.min(whole.len());
        assert_eq!(drawn(&head), drawn(&whole[..kept]), "{max_blocks} blocks");
        assert_eq!(complete, max_blocks >= whole.len(), "{max_blocks} blocks");
    }
}

/// The byte cap bounds the parse of a text that is one enormous block, and a text that was
/// cut is never called complete - even though what was kept is a single block, as the
/// whole would have been.
#[test]
fn a_cut_text_is_parsed_only_as_far_as_the_cut() {
    let fence = format!("```\n{}\n```", "é".repeat(100_000));
    let (blocks, complete) = parse_head(&fence, 5, 1_001);
    assert!(!complete);
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].hard_wrap, "still a code block");
    // A cut in the middle of a two-byte character lands before it.
    assert!(blocks[0].chars.len() < 500);

    let (blocks, complete) = parse_head("short", 5, 1_001);
    assert!(complete);
    assert_eq!(blocks.len(), 1);
    // A cap at exactly the length cuts nothing.
    assert!(parse_head("short", 5, 5).1);
}

/// A reference link in the head whose definition the cut left behind draws as the whole
/// parse draws it - full, collapsed and shortcut references, a label matched regardless of
/// case and spacing, and a destination on the line after its label - while one defined
/// nowhere still draws as the brackets it was written as.
#[test]
fn a_head_resolves_references_defined_past_the_cut() {
    let filler = "filler ".repeat(200);
    for (reference, definition) in [
        ("[docs][1]", "[1]: https://example.com/one"),
        ("[docs][]", "[DOCS]: https://example.com/collapsed"),
        (
            "[The  Docs]",
            "  [the docs]:\n  <https://example.com/shortcut> \"a title\"",
        ),
        ("[nope][9]", "[8]: https://example.com/other"),
    ] {
        let text = format!("See {reference} here.\n\n{filler}\n\n{definition}\n");
        let (head, complete) = parse_head(&text, 5, 64);
        assert!(!complete);
        let whole = parse(&text);
        assert_eq!(
            render_block(&head[0], 80),
            render_block(&whole[0], 80),
            "{reference}"
        );
    }
    let (head, _) = parse_head("See [nope][9].\n\nfiller\n\n[8]: https://x", 5, 20);
    assert!(render_block(&head[0], 80)[0].text.contains("[nope][9]"));
}
