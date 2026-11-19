use super::*;
#[test]
fn safe_markdown_handles_all_active_paths() {
    let rendered = render_safe("<script>x</script>\n\n[bad](JaVaScRiPt:bad) [bad](jav&#97;script:bad) [good](https://example.org)\n\n![*alt* <x>](https://example.org/image)\n\n```html\n<b>\n```");
    assert!(!rendered.contains("<script>"));
    assert!(!rendered.contains("<img"));
    assert!(!rendered.contains("href=\"JaVa"));
    assert!(!rendered.contains("href=\"java"));
    assert!(rendered.contains("rel=\"noopener noreferrer\""));
    assert!(rendered.contains("&lt;b&gt;"));
    assert!(rendered.contains("alt"));
}
#[test]
fn links_refuse_browser_normalization_tricks() {
    for url in [
        "//evil",
        "\\evil",
        "javascript:x",
        "data:x",
        "vbscript:x",
        "java\nscript:x",
        " https://example.org",
        "jav&colon;x",
    ] {
        assert!(!safe_link(url), "{url}");
    }
    for url in [
        "/safe",
        "#part",
        "../relative",
        "https://example.org",
        "http://example.org",
        "mailto:x@example.org",
        "a/b?x=1&y=2",
    ] {
        assert!(safe_link(url), "{url}");
    }
}

#[test]
fn safe_markdown_renders_tables_with_csp_safe_alignment_classes() {
    let source = "\
| Default | Left | Center | Right |
| --- | :--- | :---: | ---: |
| **bold** | `code` | ~~old~~ | 42 |
| [ok](https://example.org) | a \\| b | *em* | |
";
    let rendered = render_safe(source);
    assert!(!rendered.contains("style="), "{rendered}");
    assert!(
        rendered.starts_with("<div class=\"table-wrap\"><table>\n"),
        "{rendered}"
    );
    assert!(rendered.ends_with("</table></div>\n"), "{rendered}");
    assert!(
        rendered.contains(
            "<thead><tr>\
             <th scope=\"col\">Default</th>\
             <th scope=\"col\" class=\"align-left\">Left</th>\
             <th scope=\"col\" class=\"align-center\">Center</th>\
             <th scope=\"col\" class=\"align-right\">Right</th>\
             </tr></thead>\n"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "<tbody>\n\
             <tr>\
             <td><strong>bold</strong></td>\
             <td class=\"align-left\"><code>code</code></td>\
             <td class=\"align-center\"><del>old</del></td>\
             <td class=\"align-right\">42</td>\
             </tr>\n"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "<tr>\
             <td><a href=\"https://example.org\" rel=\"noopener noreferrer\" title=\"\">ok</a></td>\
             <td class=\"align-left\">a | b</td>\
             <td class=\"align-center\"><em>em</em></td>\
             <td class=\"align-right\"></td>\
             </tr>\n\
             </tbody>\n"
        ),
        "{rendered}"
    );
}

#[test]
fn safe_markdown_header_only_table_omits_empty_tbody() {
    let rendered = render_safe("| A | B |\n| --- | ---: |\n");
    assert!(rendered.contains("<thead><tr><th scope=\"col\">A</th><th scope=\"col\" class=\"align-right\">B</th></tr></thead>"));
    assert!(!rendered.contains("<tbody>"), "{rendered}");
    assert!(!rendered.contains("style="), "{rendered}");
}

#[test]
fn safe_markdown_sanitizes_hostile_content_inside_table_cells() {
    let source = "\
| Col 1 | Col 2 | Col 3 |
| --- | --- | --- |
| <script>alert(1)</script> | [bad](javascript:alert(1)) | ![*alt* <x>](https://example.org/img.png) |
| </td><script>x</script> | <img src=x onerror=alert(1)> | ok |
";
    let rendered = render_safe(source);
    assert!(!rendered.contains("<script>"), "{rendered}");
    assert!(!rendered.contains("<img"), "{rendered}");
    assert!(!rendered.contains("href="), "{rendered}");
    assert!(!rendered.contains("style="), "{rendered}");
    assert!(
        rendered.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{rendered}"
    );
    assert!(
        rendered.contains("&lt;/td&gt;&lt;script&gt;x&lt;/script&gt;"),
        "{rendered}"
    );
    assert!(rendered.contains("<td>bad</td>"), "{rendered}");
    assert!(rendered.contains("<td>alt &lt;x&gt;</td>"), "{rendered}");
}

#[test]
fn safe_markdown_renders_tables_nested_in_blockquotes_and_lists() {
    let source = "\
> | Q | A |
> | --- | ---: |
> | 1 | 2 |

- | K | V |
  | :--- | --- |
  | x | y |
";
    let rendered = render_safe(source);
    assert!(
        rendered.contains("<blockquote>\n<div class=\"table-wrap\"><table>\n"),
        "{rendered}"
    );
    assert!(
        rendered.contains("<li><div class=\"table-wrap\"><table>\n"),
        "{rendered}"
    );
    assert!(!rendered.contains("style="), "{rendered}");
}

fn wide_table_block(cols: usize, rows: usize) -> String {
    let mut table = String::new();
    for _ in 0..cols {
        table.push_str("|a");
    }
    table.push_str("|\n");
    for _ in 0..cols {
        table.push_str("|:-");
    }
    table.push_str("|\n");
    for _ in 0..rows {
        table.push_str("x\n");
    }
    table
}

#[test]
fn render_safe_bounded_enforces_exact_limit_and_rejects_table_amplification() {
    let simple = "| A | B |\n| --- | ---: |\n| 1 | 2 |\n# After\n";
    let expected = render_safe(simple);
    assert_eq!(
        render_safe_bounded(simple, expected.len()),
        Some(expected.clone())
    );
    assert_eq!(render_safe_bounded(simple, expected.len() - 1), None);

    // Non-table expansion (`<` -> `&lt;`) stops at the byte budget.
    assert_eq!(render_safe_bounded(&"<".repeat(200), 256), None);

    // Single ~3.6 KiB wide table expands past 4 MiB (~7.3 MiB of aligned cells).
    let one_table = wide_table_block(512, 512);
    assert!(one_table.len() < 4096, "len = {}", one_table.len());
    assert_eq!(render_safe_bounded(&one_table, 4 * 1024 * 1024), None);

    // Multi-table < 1 MiB bomb (including an HTML comment + code-fence decoy)
    // is rejected before pulldown-cmark's first-pass table auto-completion.
    let mut bomb = String::from("<!--\n```\n-->\n");
    for _ in 0..200 {
        bomb.push_str(&one_table);
        bomb.push('\n');
    }
    assert!(bomb.len() < 1024 * 1024, "len = {}", bomb.len());
    assert_eq!(render_safe_bounded(&bomb, 4 * 1024 * 1024), None);
}

#[test]
fn render_safe_bounded_allows_non_table_delimiter_lines() {
    // Delimiter line not preceded by a piped header line is ordinary prose, not a table.
    let mut source = String::from("plain text without pipes\n");
    for _ in 0..512 {
        source.push_str("|:-");
    }
    source.push_str("|\n");
    for _ in 0..600 {
        source.push_str("x\n");
    }
    let rendered = render_safe_bounded(&source, 4 * 1024 * 1024)
        .expect("non-table prose should stay within budget");
    assert!(!rendered.contains("<table>"), "{rendered}");
}

#[test]
fn render_safe_bounded_handles_bare_cr_line_endings() {
    let small_cr = "| A | B |\r| --- | ---: |\r| 1 | 2 |\r";
    let rendered = render_safe_bounded(small_cr, 4096).expect("small CR table fits in budget");
    assert!(
        rendered.contains("<td class=\"align-right\">2</td>"),
        "{rendered}"
    );

    let one_cr_table = wide_table_block(512, 512).replace('\n', "\r");
    assert!(!table_html_within_budget(&one_cr_table, 4 * 1024 * 1024));
    assert_eq!(render_safe_bounded(&one_cr_table, 4 * 1024 * 1024), None);

    let mut cr_bomb = String::new();
    for _ in 0..200 {
        cr_bomb.push_str(&one_cr_table);
        cr_bomb.push('\r');
    }
    assert!(cr_bomb.len() < 1024 * 1024, "len = {}", cr_bomb.len());
    assert!(!table_html_within_budget(&cr_bomb, 4 * 1024 * 1024));
    assert_eq!(render_safe_bounded(&cr_bomb, 4 * 1024 * 1024), None);
}

#[test]
fn render_safe_bounded_respects_fenced_code_and_block_boundaries() {
    // Fenced code containing a wide table-shaped example must not be charged as
    // rendered table cells; content that fits a 2 KiB budget must still render.
    let mut fenced = String::from("```markdown\n");
    fenced.push_str(&wide_table_block(200, 120));
    fenced.push_str("```\n");
    let expected_fenced = render_safe(&fenced);
    assert!(
        expected_fenced.len() < 2048,
        "fenced HTML len = {}",
        expected_fenced.len()
    );
    assert_eq!(
        render_safe_bounded(&fenced, 2048),
        Some(expected_fenced.clone())
    );
    assert!(!expected_fenced.contains("<table>"), "{expected_fenced}");

    // Tilde-fenced and indented code blocks also never count as tables.
    let tilde_fenced = format!("~~~\n{}~~~\n", wide_table_block(200, 120));
    let expected_tilde = render_safe(&tilde_fenced);
    assert!(expected_tilde.len() < 2048);
    assert_eq!(
        render_safe_bounded(&tilde_fenced, 2048),
        Some(expected_tilde)
    );

    let indented: String = wide_table_block(200, 120)
        .lines()
        .map(|line| format!("    {line}\n"))
        .collect();
    let expected_indented = render_safe(&indented);
    assert!(expected_indented.len() < 4096);
    assert_eq!(
        render_safe_bounded(&indented, 4096),
        Some(expected_indented)
    );

    // Real tables interrupted without a blank line by another block must stop
    // charging rows at the block boundary.
    let small_table = format!("{}|\n{}|\n", "|a".repeat(25), "|:-".repeat(25));
    let tail_lines = "x\n".repeat(150);
    for (label, separator, closing) in [
        ("atx_heading", "# After\n", ""),
        ("code_fence", "```\n", "```\n"),
        ("thematic_break", "---\n", ""),
        ("blockquote", "> quote\n", ""),
        ("unordered_list", "- item\n", ""),
        ("ordered_list", "2. item\n", ""),
        ("html_block", "<div>\n", "</div>\n"),
    ] {
        let source = format!("{small_table}{separator}{tail_lines}{closing}");
        let expected = render_safe(&source);
        assert!(
            expected.len() < 2048,
            "{label}: expected HTML len = {}",
            expected.len()
        );
        assert_eq!(
            render_safe_bounded(&source, 2048),
            Some(expected),
            "{label} should fit within 2 KiB budget"
        );
    }

    // Blockquote table ended by lazy drop of `>` must not charge following lines.
    let quoted_table = format!("> {}|\n> {}|\n", "|a".repeat(25), "|:-".repeat(25));
    let lazy_source = format!("{quoted_table}{tail_lines}");
    let expected_lazy = render_safe(&lazy_source);
    assert!(
        expected_lazy.len() < 2048,
        "lazy HTML len = {}",
        expected_lazy.len()
    );
    assert_eq!(render_safe_bounded(&lazy_source, 2048), Some(expected_lazy));
}

#[test]
fn render_safe_bounded_still_rejects_smuggled_table_bombs() {
    let one_table = wide_table_block(512, 512);

    // Unclosed code fences inside blockquotes or lists end with their container,
    // so a table bomb immediately after them is a real table and must be rejected.
    for prefix in ["> ```\n", "- ```\n", "<!--\n```\n-->\n", "```foo`bar\n"] {
        let mut bomb = String::from(prefix);
        for _ in 0..200 {
            bomb.push_str(&one_table);
            bomb.push('\n');
        }
        assert!(bomb.len() < 1024 * 1024);
        assert!(
            !table_html_within_budget(&bomb, 4 * 1024 * 1024),
            "prefix {prefix:?} must not hide a table bomb"
        );
        assert_eq!(render_safe_bounded(&bomb, 4 * 1024 * 1024), None);
    }

    // Table bomb inside a tight list item must be caught.
    let tight_item_table: String = one_table
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!("- {line}\n")
            } else {
                format!("  {line}\n")
            }
        })
        .collect();
    assert!(!table_html_within_budget(
        &tight_item_table,
        4 * 1024 * 1024
    ));
    assert_eq!(
        render_safe_bounded(&tight_item_table, 4 * 1024 * 1024),
        None
    );

    // `===` after a table stays a table row (becomes setext H1 without ENABLE_TABLES),
    // so a table bomb terminated by `===` must still be rejected.
    let setext_bomb = format!("{one_table}===\n");
    assert!(!table_html_within_budget(&setext_bomb, 4 * 1024 * 1024));
    assert_eq!(render_safe_bounded(&setext_bomb, 4 * 1024 * 1024), None);

    // Four-space indented `2. x` lines do not start a list and stay table rows.
    let mut indented_rows_bomb = format!("{}|\n{}|\n", "|a".repeat(512), "|:-".repeat(512));
    for _ in 0..512 {
        indented_rows_bomb.push_str("    2. x\n");
    }
    assert!(!table_html_within_budget(
        &indented_rows_bomb,
        4 * 1024 * 1024
    ));
    assert_eq!(
        render_safe_bounded(&indented_rows_bomb, 4 * 1024 * 1024),
        None
    );
}
