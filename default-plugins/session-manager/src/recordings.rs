use zellij_tile::prelude::ElapsedMicros;
use zellij_tile::prelude::RecordingId;
use zellij_tile::prelude::SessionName;
#[cfg(test)]
use zellij_tile::prelude::TerminalId;
#[cfg(test)]
use zellij_tile::prelude::UnixMicros;
use zellij_tile::prelude::{RecordingSummary, SearchHit};
use zellij_tile::shim::*;

#[derive(Debug, Default)]
pub struct RecordingRow {
    pub id: RecordingId,
    pub session_name: SessionName,
    pub tab_name: String,
    pub pane_title: String,
    pub duration_us: ElapsedMicros,
    pub elapsed_us: ElapsedMicros,
    pub text: Option<String>,
}

impl From<RecordingSummary> for RecordingRow {
    fn from(row: RecordingSummary) -> Self {
        Self {
            id: row.id,
            session_name: row.session_name,
            tab_name: row.tab_name,
            pane_title: row.pane_title,
            duration_us: row.duration_us,
            ..Default::default()
        }
    }
}

impl From<SearchHit> for RecordingRow {
    fn from(hit: SearchHit) -> Self {
        Self {
            id: hit.recording_id,
            session_name: hit.session_name,
            tab_name: hit.tab_name,
            pane_title: hit.pane_title,
            elapsed_us: hit.elapsed_us,
            text: Some(hit.text),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct Recordings {
    pub search_term: String,
    rows: Vec<RecordingRow>,
    selected_index: Option<usize>,
}

impl Recordings {
    pub fn refresh(&mut self) {
        self.query();
    }
    pub fn handle_character(&mut self, character: char) {
        self.search_term.push(character);
        self.query();
    }
    pub fn handle_backspace(&mut self) {
        self.search_term.pop();
        self.query();
    }
    pub fn clear_search_term(&mut self) -> bool {
        if self.search_term.is_empty() {
            return false;
        }
        self.search_term.clear();
        self.query();
        true
    }
    fn query(&mut self) {
        self.rows = if self.search_term.is_empty() {
            list_recordings().into_iter().map(Into::into).collect()
        } else {
            search_recordings(&self.search_term, 50)
                .into_iter()
                .map(Into::into)
                .collect()
        };
        self.selected_index = if self.rows.is_empty() { None } else { Some(0) };
    }
    pub fn render(&self, rows: usize, columns: usize, x: usize, y: usize) {
        let search_indication =
            Text::new(format!("Search: {}_", self.search_term)).color_range(2, ..7);
        print_text_with_coordinates(search_indication, x.saturating_sub(1), y + 2, None, None);
        let table_rows = rows.saturating_sub(5); // search row, toggle row and some padding
        if self.rows.is_empty() {
            let message = if self.search_term.is_empty() {
                "No recordings."
            } else {
                "No matches."
            };
            print_text_with_coordinates(Text::new(message), x, y + 4, None, None);
            return;
        }
        print_table_with_coordinates(
            self.render_table(table_rows, columns),
            x,
            y + 3,
            Some(columns),
            Some(table_rows),
        );
    }
    fn render_table(&self, table_rows: usize, table_columns: usize) -> Table {
        let mut table = Table::new().add_row(vec![" ", " ", " ", " "]); // skip the title row
        let (first_row_index_to_render, last_row_index_to_render) =
            self.range_to_render(table_rows, self.rows.len(), self.selected_index);
        // the other three columns and the padding between them
        let room_for_text = table_columns.saturating_sub(48);
        for i in first_row_index_to_render..last_row_index_to_render {
            if let Some(row) = self.rows.get(i) {
                let is_selected = Some(i) == self.selected_index;
                let mut table_cells = vec![
                    Text::new(&row.session_name).color_range(0, ..),
                    Text::new(pane_label(row)).color_range(2, ..),
                    Text::new(time_label(row)).color_range(1, ..),
                    Text::new(truncate(
                        row.text.as_deref().unwrap_or(" ").trim(),
                        room_for_text,
                    )),
                ];
                if is_selected {
                    table_cells = table_cells.drain(..).map(|t| t.selected()).collect();
                }
                table = table.add_styled_row(table_cells);
            }
        }
        table
    }
    fn range_to_render(
        &self,
        table_rows: usize,
        results_len: usize,
        selected_index: Option<usize>,
    ) -> (usize, usize) {
        if table_rows <= results_len {
            let row_count_to_render = table_rows.saturating_sub(1); // 1 for the title
            let first_row_index_to_render = selected_index
                .unwrap_or(0)
                .saturating_sub(row_count_to_render / 2)
                // keep the viewport full when the selection is near the end of the list
                .min(results_len - row_count_to_render);
            let last_row_index_to_render =
                (first_row_index_to_render + row_count_to_render).min(results_len);
            (first_row_index_to_render, last_row_index_to_render)
        } else {
            (0, results_len)
        }
    }
    pub fn move_selection_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected_index = Some(match self.selected_index {
            Some(i) if i + 1 < self.rows.len() => i + 1,
            Some(_) => 0,
            None => 0,
        });
    }
    pub fn move_selection_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected_index = Some(match self.selected_index {
            Some(0) | None => self.rows.len().saturating_sub(1),
            Some(i) => i - 1,
        });
    }
    /// The `recording_id` under the cursor, for `zellij recordings export` (and, later, replay).
    #[allow(dead_code)] // the hook replay will hang off
    pub fn selected_recording_id(&self) -> Option<RecordingId> {
        self.selected_index
            .and_then(|i| self.rows.get(i))
            .map(|r| r.id)
    }
}

fn pane_label(row: &RecordingRow) -> String {
    let pane = if row.pane_title.is_empty() {
        format!("#{}", row.id)
    } else {
        row.pane_title.clone()
    };
    if row.tab_name.is_empty() {
        pane
    } else {
        format!("{}/{}", row.tab_name, pane)
    }
}

/// The offset of the matched line for search hits, the total length for list rows.
fn time_label(row: &RecordingRow) -> String {
    let micros = if row.text.is_some() {
        row.elapsed_us
    } else {
        row.duration_us
    };
    let total_seconds = micros.get().max(0) / 1_000_000;
    let (hours, minutes, seconds) = (
        total_seconds / 3600,
        (total_seconds % 3600) / 60,
        total_seconds % 60,
    );
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{}:{:02}", minutes, seconds)
    }
}

fn truncate(text: &str, max_len: usize) -> String {
    if max_len == 0 || text.chars().count() <= max_len {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_len.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    /// `(table_rows, results_len, selected_index)` with the selection always in range,
    /// as `render_table` guarantees.
    #[hegel::composite]
    fn render_inputs(tc: &hegel::TestCase) -> (usize, usize, Option<usize>) {
        let table_rows = tc.draw(generators::integers::<usize>().max_value(500));
        let results_len = tc.draw(generators::integers::<usize>().max_value(500));
        let selected_index = if results_len == 0 {
            None
        } else {
            tc.draw(generators::optional(
                generators::integers::<usize>().max_value(results_len - 1),
            ))
        };
        (table_rows, results_len, selected_index)
    }

    fn row(id: i64) -> RecordingRow {
        RecordingRow {
            id: RecordingId::new(id),
            session_name: "s".to_string().into(),
            tab_name: "t".to_string(),
            pane_title: "zsh".to_string(),
            duration_us: ElapsedMicros::new(65_000_000),
            ..Default::default()
        }
    }

    #[test]
    fn converts_typed_rows_and_labels_them() {
        let list_row = RecordingRow::from(RecordingSummary {
            id: RecordingId::new(7),
            session_name: "s".to_string().into(),
            tab_name: "t".to_string(),
            terminal_id: TerminalId::new(1),
            pane_title: "zsh".to_string(),
            started_at_us: UnixMicros::new(0),
            duration_us: ElapsedMicros::new(65_000_000),
            is_running: false,
            capture_input: false,
            error: None,
        });
        assert_eq!(time_label(&list_row), "1:05");
        assert_eq!(pane_label(&list_row), "t/zsh");

        let search_row = RecordingRow::from(SearchHit {
            recording_id: RecordingId::new(9),
            session_name: "s".to_string().into(),
            tab_name: "t".to_string(),
            pane_title: "zsh".to_string(),
            elapsed_us: ElapsedMicros::new(3_661_000_000),
            started_at_us: UnixMicros::new(0),
            text: "error: boom".to_string(),
        });
        assert_eq!(search_row.id, RecordingId::new(9));
        assert_eq!(time_label(&search_row), "1:01:01");
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut recordings = Recordings {
            rows: vec![row(7)],
            selected_index: Some(0),
            ..Default::default()
        };
        recordings.move_selection_down();
        assert_eq!(
            recordings.selected_recording_id(),
            Some(RecordingId::new(7))
        );
        recordings.move_selection_up();
        assert_eq!(
            recordings.selected_recording_id(),
            Some(RecordingId::new(7))
        );
    }

    #[hegel::test]
    fn range_stays_within_the_results_and_the_viewport(tc: hegel::TestCase) {
        let (table_rows, results_len, selected_index) = tc.draw(render_inputs());
        let (first, last) =
            Recordings::default().range_to_render(table_rows, results_len, selected_index);
        assert!(first <= last, "inverted range {}..{}", first, last);
        assert!(
            last <= results_len,
            "{} rows past the end of {}",
            last,
            results_len
        );
        assert!(
            last - first <= table_rows.saturating_sub(1),
            "{}..{} does not fit in {} rows (1 goes to the title)",
            first,
            last,
            table_rows
        );
    }

    #[hegel::test]
    fn selection_is_always_on_screen(tc: hegel::TestCase) {
        let (table_rows, results_len, selected_index) = tc.draw(render_inputs());
        // fewer than 2 rows leaves no room for anything but the title
        tc.assume(table_rows >= 2);
        let Some(selected) = selected_index else {
            return;
        };
        let (first, last) =
            Recordings::default().range_to_render(table_rows, results_len, selected_index);
        assert!(
            first <= selected && selected < last,
            "selection {} outside rendered range {}..{}",
            selected,
            first,
            last
        );
    }

    #[hegel::test]
    fn everything_is_rendered_when_it_fits(tc: hegel::TestCase) {
        let (table_rows, results_len, selected_index) = tc.draw(render_inputs());
        tc.assume(results_len < table_rows);
        assert_eq!(
            Recordings::default().range_to_render(table_rows, results_len, selected_index),
            (0, results_len)
        );
    }

    #[hegel::test]
    fn viewport_is_filled_when_there_are_enough_results(tc: hegel::TestCase) {
        let (table_rows, results_len, selected_index) = tc.draw(render_inputs());
        tc.assume(table_rows >= 2 && results_len >= table_rows - 1);
        let (first, last) =
            Recordings::default().range_to_render(table_rows, results_len, selected_index);
        assert_eq!(
            last - first,
            table_rows - 1,
            "{} results but only {}..{} rendered in {} rows",
            results_len,
            first,
            last,
            table_rows
        );
    }
}
