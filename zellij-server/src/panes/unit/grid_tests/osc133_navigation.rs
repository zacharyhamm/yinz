use super::*;

/// `count` shell commands the way a shell with OSC 133 integration writes them: a marked
/// prompt (`A`), the typed command (`B`), its output (`C`) and the end of the command (`D`),
/// each on two rows: `$ cmdNN` and `outNN`.
fn marked_commands(count: usize) -> String {
    (0..count)
        .map(|command_index| {
            format!(
                "\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}cmd{command_index:02}\u{1b}]133;C\u{7}\r\nout{command_index:02}\u{1b}]133;D;0\u{7}\r\n"
            )
        })
        .collect()
}

/// The same commands from a shell that marks only the command and its output (`B`, `C`, `D`)
/// and never the prompt itself.
fn commands_marked_without_prompts(count: usize) -> String {
    (0..count)
        .map(|command_index| {
            format!(
                "$ \u{1b}]133;B\u{7}cmd{command_index:02}\u{1b}]133;C\u{7}\r\nout{command_index:02}\u{1b}]133;D;0\u{7}\r\n"
            )
        })
        .collect()
}

fn six_row_grid_with(content: &str) -> Grid {
    create_grid_with_size_and_raw(6, 40, content.as_bytes())
}

fn is_scrolled(grid: &Grid) -> bool {
    !grid.lines_below.is_empty()
}

fn top_viewport_line(grid: &Grid) -> String {
    viewport_texts(grid)
        .first()
        .map(|line| line.trim_end().to_owned())
        .unwrap_or_default()
}

#[test]
fn scrolling_to_the_previous_prompt_puts_it_at_the_top_of_the_viewport() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    assert_eq!(
        top_viewport_line(&grid),
        "out09",
        "pre-condition: at the bottom"
    );

    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd09");

    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd08");
}

#[test]
fn scrolling_to_the_next_prompt_moves_back_down_one_prompt_at_a_time() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    assert!(grid.scroll_to_previous_prompt());
    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd08");

    assert!(grid.scroll_to_next_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd09");
}

#[test]
fn the_next_prompt_jump_stops_at_the_bottom_of_the_buffer() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    assert!(grid.scroll_to_previous_prompt());
    assert!(grid.scroll_to_next_prompt(), "back to the bottom");
    assert!(!is_scrolled(&grid));

    assert!(
        !grid.scroll_to_next_prompt(),
        "nothing below the bottom to jump to"
    );
    assert!(!is_scrolled(&grid));
}

#[test]
fn prompt_jumps_do_nothing_in_a_pane_without_osc133_markers() {
    let plain_commands: String = (0..12)
        .map(|command_index| format!("$ cmd{command_index:02}\r\nout{command_index:02}\r\n"))
        .collect();
    let mut grid = six_row_grid_with(&plain_commands);

    assert!(!grid.scroll_to_previous_prompt());
    assert!(!is_scrolled(&grid), "an unmarked pane is left where it was");
    assert!(!grid.scroll_to_next_prompt());
}

#[test]
fn an_input_marker_stands_in_for_a_prompt_the_shell_never_marked() {
    let mut grid = six_row_grid_with(&commands_marked_without_prompts(12));

    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd09");
    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd08");
}

#[test]
fn selecting_the_command_at_the_scroll_position_selects_the_command_at_the_top() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    assert!(grid.scroll_to_previous_prompt());
    assert_eq!(top_viewport_line(&grid), "$ cmd09");

    assert!(grid.select_command_at_scroll_position());
    assert_eq!(grid.get_selected_text().as_deref(), Some("cmd09\nout09"));
}

#[test]
fn selecting_the_command_at_the_scroll_position_needs_a_marked_command_in_view() {
    let mut grid = six_row_grid_with("$ hello\r\n$ world\r\n");
    assert!(!grid.select_command_at_scroll_position());
    assert_eq!(grid.get_selected_text(), None);
}

#[test]
fn the_last_completed_command_output_is_the_most_recent_finished_one() {
    let grid = six_row_grid_with(&marked_commands(12));

    let (output, start, end) = grid
        .last_completed_command_output()
        .expect("twelve completed commands");
    assert_eq!(output, "out11");
    assert!(
        start.line.0 < end.line.0,
        "the output spans from the C marker down to the D marker"
    );
}

#[test]
fn a_command_still_running_has_no_completed_output_yet() {
    let grid =
        six_row_grid_with("\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}sleep 100\u{1b}]133;C\u{7}\r\n");
    assert_eq!(grid.last_completed_command_output(), None);
}

#[test]
fn the_command_output_flash_is_shown_until_it_is_cleared() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    assert!(!grid.command_output_flash_is_shown());
    assert!(!grid.clear_command_output_flash(), "nothing to clear yet");

    let (_, start, end) = grid.last_completed_command_output().unwrap();
    grid.set_command_output_flash(start, end);
    assert!(grid.command_output_flash_is_shown());

    assert!(grid.clear_command_output_flash());
    assert!(!grid.command_output_flash_is_shown());
}

#[test]
fn the_command_output_flash_follows_its_rows_into_the_scrollback() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    let (_, start, end) = grid.last_completed_command_output().unwrap();
    grid.set_command_output_flash(start, end);
    let before = grid.command_output_flash.unwrap();

    // a background job prints while the flash is up: the flashed rows move up one line
    feed_bytes(&mut grid, b"late output\r\n");

    let after = grid.command_output_flash.unwrap();
    assert_eq!(after.start.line.0, before.start.line.0 - 1);
    assert_eq!(after.end.line.0, before.end.line.0 - 1);
}

#[test]
fn the_command_output_flash_is_dropped_by_a_resize() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    let (_, start, end) = grid.last_completed_command_output().unwrap();
    grid.set_command_output_flash(start, end);
    assert!(grid.command_output_flash_is_shown());

    grid.change_size(6, 30);
    assert!(!grid.command_output_flash_is_shown());
}

#[test]
fn selecting_the_command_at_the_scroll_position_ignores_the_triple_click_opt_out() {
    let mut grid = six_row_grid_with(&marked_commands(12));
    grid.set_selection_options(false, "");
    assert!(grid.scroll_to_previous_prompt());
    assert!(
        grid.select_command_at_scroll_position(),
        "`osc133_command_selection false` only opts out of triple-click selection"
    );
}

#[test]
fn text_in_range_reads_across_the_scrollback_boundary() {
    let grid = six_row_grid_with(&marked_commands(12));
    let scrollback_rows = grid.lines_above.len() as i32;
    assert!(scrollback_rows > 0, "pre-condition: there is scrollback");

    // from the last scrollback row into the first viewport row
    let text = grid
        .text_in_range(Position::new(-1, 0), Position::new(0, 5))
        .expect("a non-empty range");
    assert_eq!(text, "$ cmd09\nout09");
}
