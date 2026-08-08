use std::collections::BTreeMap;

use serde::Deserialize;
use zellij_tile::shim::*;

/// The context key we tag our `run_command` calls with, so we can tell our own results apart
/// from anyone else's and drop replies to queries the user has already typed past.
const CONTEXT_KEY: &str = "recordings_query";

/// A row of `zellij recordings list --json` or `zellij recordings search --json`. The two
/// share enough fields to be one struct: search hits carry `recording_id` and the matched
/// `text`, list rows carry `id` and a `duration_us`.
#[derive(Debug, Default, Deserialize)]
pub struct RecordingRow {
    #[serde(alias = "recording_id")]
    pub id: i64,
    #[serde(default)]
    pub session_name: String,
    #[serde(default)]
    pub tab_name: String,
    #[serde(default)]
    pub pane_title: String,
    #[serde(default)]
    pub duration_us: i64,
    #[serde(default)]
    pub elapsed_us: i64,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Default)]
pub struct Recordings {
    pub search_term: String,
    rows: Vec<RecordingRow>,
    selected_index: Option<usize>,
    error: Option<String>,
    /// A query is running. Only ever one at a time - the next one is queued in `pending` and
    /// fired when this one returns, which doubles as the debounce for typing.
    in_flight: bool,
    pending: Option<String>,
    has_queried: bool,
}

impl Recordings {
    /// Fetch the list if we never have. Called when the screen is entered.
    pub fn ensure_queried(&mut self) {
        if !self.has_queried {
            self.has_queried = true;
            self.query();
        }
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
        let term = self.search_term.clone();
        if self.in_flight {
            self.pending = Some(term);
            return;
        }
        self.in_flight = true;
        let mut context = BTreeMap::new();
        context.insert(CONTEXT_KEY.to_owned(), term.clone());
        // ponytail: shells out to `zellij` on PATH with the default data dir - a server started
        // with --data-dir will point us at the wrong database. Plumb a PluginCommand if that
        // ever matters.
        if term.is_empty() {
            run_command(&["zellij", "recordings", "list", "--json"], context);
        } else {
            run_command(
                &["zellij", "recordings", "search", &term, "--json"],
                context,
            );
        }
    }
    /// Returns whether this result was ours (and so whether a re-render is needed).
    pub fn handle_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        context: BTreeMap<String, String>,
    ) -> bool {
        let Some(term) = context.get(CONTEXT_KEY) else {
            return false;
        };
        let is_stale = *term != self.search_term;
        self.in_flight = false;
        if let Some(pending) = self.pending.take() {
            self.search_term = pending;
            self.query();
        }
        if is_stale {
            // a newer query is on its way, don't flash intermediate results
            return false;
        }
        if exit_code.unwrap_or(0) != 0 {
            self.error = Some(String::from_utf8_lossy(&stderr).trim().to_owned());
            self.rows.clear();
        } else {
            match serde_json::from_slice::<Vec<RecordingRow>>(&stdout) {
                Ok(rows) => {
                    self.error = None;
                    self.rows = rows;
                },
                Err(e) => {
                    self.error = Some(format!("Failed to parse recordings: {}", e));
                    self.rows.clear();
                },
            }
        }
        self.selected_index = if self.rows.is_empty() { None } else { Some(0) };
        true
    }
    pub fn render(&self, rows: usize, columns: usize, x: usize, y: usize) {
        let search_indication =
            Text::new(format!("Search: {}_", self.search_term)).color_range(2, ..7);
        print_text_with_coordinates(search_indication, x.saturating_sub(1), y + 2, None, None);
        let table_rows = rows.saturating_sub(5); // search row, toggle row and some padding
        if let Some(error) = self.error.as_ref() {
            print_text_with_coordinates(Text::new(error).color_range(3, ..), x, y + 4, None, None);
            return;
        }
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
                .saturating_sub(row_count_to_render / 2);
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
    pub fn selected_recording_id(&self) -> Option<i64> {
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
    let total_seconds = micros.max(0) / 1_000_000;
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

    fn list_json() -> Vec<u8> {
        br#"[{"id":7,"session_name":"s","tab_name":"t","terminal_id":1,"pane_title":"zsh",
             "started_at_us":0,"duration_us":65000000,"is_running":false,"capture_input":false,
             "error":null}]"#
            .to_vec()
    }

    fn search_json() -> Vec<u8> {
        br#"[{"recording_id":9,"session_name":"s","tab_name":"t","pane_title":"zsh",
             "elapsed_us":3661000000,"started_at_us":0,"text":"error: boom"}]"#
            .to_vec()
    }

    fn context(term: &str) -> BTreeMap<String, String> {
        let mut context = BTreeMap::new();
        context.insert(CONTEXT_KEY.to_owned(), term.to_owned());
        context
    }

    #[test]
    fn parses_both_shapes_and_labels_them() {
        let mut recordings = Recordings::default();
        recordings.in_flight = true;
        assert!(recordings.handle_command_result(Some(0), list_json(), vec![], context("")));
        assert_eq!(recordings.selected_recording_id(), Some(7));
        assert_eq!(time_label(&recordings.rows[0]), "1:05"); // duration, no text
        assert_eq!(pane_label(&recordings.rows[0]), "t/zsh");

        recordings.search_term = "boom".to_owned();
        recordings.in_flight = true;
        assert!(recordings.handle_command_result(Some(0), search_json(), vec![], context("boom")));
        assert_eq!(recordings.selected_recording_id(), Some(9));
        assert_eq!(time_label(&recordings.rows[0]), "1:01:01"); // hit offset
    }

    #[test]
    fn stale_results_are_dropped_and_the_pending_query_wins() {
        let mut recordings = Recordings::default();
        recordings.search_term = "boom".to_owned();
        recordings.in_flight = true;
        // a reply to the older "bo" query lands after the user typed on
        assert!(!recordings.handle_command_result(Some(0), list_json(), vec![], context("bo")));
        assert!(recordings.rows.is_empty());
    }

    #[test]
    fn other_plugins_command_results_are_ignored() {
        let mut recordings = Recordings::default();
        assert!(!recordings.handle_command_result(Some(0), list_json(), vec![], BTreeMap::new()));
    }

    #[test]
    fn a_failed_command_surfaces_stderr() {
        let mut recordings = Recordings::default();
        recordings.in_flight = true;
        assert!(recordings.handle_command_result(
            Some(1),
            vec![],
            b"no such database\n".to_vec(),
            context("")
        ));
        assert_eq!(recordings.error.as_deref(), Some("no such database"));
        assert_eq!(recordings.selected_recording_id(), None);
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut recordings = Recordings::default();
        recordings.in_flight = true;
        recordings.handle_command_result(Some(0), list_json(), vec![], context(""));
        recordings.move_selection_down(); // one row, wraps back onto itself
        assert_eq!(recordings.selected_recording_id(), Some(7));
        recordings.move_selection_up();
        assert_eq!(recordings.selected_recording_id(), Some(7));
    }
}
