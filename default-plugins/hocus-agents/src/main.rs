//! hocus-agents — the agent list as a real pane in the hocus sidebar strip.
//!
//! This used to be the top third of the hocus pane, drawn by hocus itself with
//! an `agents_focused` flag deciding who owned the keys. Now it is its own
//! global pane, split off above the chat in the same strip, so the ordinary
//! pane-movement keys walk between the list and the conversation and each one
//! keeps its own focus, scrollback and cursor.
//!
//! **hocus computes, this pane draws.** Which panes are agents, which project
//! owns them and what branch they are on takes the agentd snapshot, the project
//! registry and the Git probes — all of which hocus already holds for its
//! tools. Deriving them again here would mean a second copy of that state to
//! keep in step, so hocus pipes the *finished rows* over (`wire::AGENT_STRIP`)
//! and this plugin is a view: it windows them, tracks a selection, and focuses
//! a pane on Enter. `AGENT_STRIP_HELLO` is the reload handshake — a freshly
//! loaded instance announces itself and hocus answers with the current rows.
//!
//! The pane is spawned by hocus on load via `launch_or_focus_global_pane_split`,
//! so the `Ctrl+`` keybinding keeps working unchanged: revealing hocus reveals
//! the whole strip, and `hide_self()` here hides it likewise.

use std::collections::BTreeMap;

use hocus_proto::agents::StripRow;
use hocus_proto::wire;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    /// The strip as hocus last sent it.
    rows: Vec<StripRow>,
    /// Index into `rows`. Always on an `Agent` row when one exists.
    selected: usize,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key, EventType::PermissionRequestResult]);
        set_selectable(true);
        rename_plugin_pane(get_plugin_ids().plugin_id, "agents");
        // hocus pushes the strip on every change; this asks for the current one
        // so a reload (or a session restore) does not sit empty until the next
        // agentd snapshot.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HELLO));
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.on_key(key),
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name != wire::AGENT_STRIP {
            return false;
        }
        let Some(rows) = pipe_message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<StripRow>>(json).ok())
        else {
            return false;
        };
        self.set_rows(rows)
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if self.rows.is_empty() {
            print_text_with_coordinates(
                Text::new("no agents").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
            return;
        }
        // Window the list around the selection rather than tracking a scroll
        // offset: keeping it near the middle means it is always visible and the
        // list moves by one row per keypress in both directions.
        let start = self
            .selected
            .saturating_sub(rows / 2)
            .min(self.rows.len().saturating_sub(rows));
        for (index, row) in self.rows.iter().skip(start).take(rows).enumerate() {
            let selected = start + index == self.selected;
            let text = match row {
                // A header takes no marker column: nothing can land on it, and
                // the indentation it carries is what shows the nesting.
                StripRow::Project(text) => Text::new(text).color_range(0, ..),
                StripRow::Branch(text) => Text::new(text).color_range(3, ..),
                StripRow::Agent { text, .. } => {
                    let line = Text::new(format!("{} {}", if selected { ">" } else { " " }, text));
                    match selected {
                        true => line.color_range(2, ..),
                        false => line.dim_range(..),
                    }
                },
            };
            print_text_with_coordinates(text, 0, index, Some(cols), None);
        }
    }
}

impl State {
    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Char('`') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // hides the whole sidebar strip, chat included - the same gesture
            // that revealed it
            hide_self();
            return false;
        }
        if key.bare_key == BareKey::Char('r') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // the one part of a row no host event keeps fresh is the branch a
            // checkout is on; ask agentd to probe again
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_REFRESH));
            return false;
        }
        match key.bare_key {
            BareKey::Esc => {
                hide_self();
                false
            },
            BareKey::Enter => {
                if let Some(StripRow::Agent { pane_id, .. }) = self.rows.get(self.selected) {
                    // the host resolves which tab holds the pane, so this works
                    // across tabs
                    focus_terminal_pane(*pane_id, false, false);
                }
                false
            },
            BareKey::Up | BareKey::Down => {
                let next = step(&self.rows, self.selected, key.bare_key);
                let moved = next != self.selected;
                self.selected = next;
                moved
            },
            _ => false,
        }
    }

    /// Take a fresh strip, staying on the agent the selection was on. The
    /// selection follows the pane rather than the row number, so a project
    /// appearing above it does not move what Enter would focus.
    fn set_rows(&mut self, rows: Vec<StripRow>) -> bool {
        let was = match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => Some(*pane_id),
            _ => None,
        };
        let selected = was
            .and_then(|pane_id| {
                rows.iter().position(
                    |row| matches!(row, StripRow::Agent { pane_id: id, .. } if *id == pane_id),
                )
            })
            .or_else(|| rows.iter().position(selectable))
            .unwrap_or(0);
        let changed = self.rows != rows || self.selected != selected;
        self.rows = rows;
        self.selected = selected;
        changed
    }
}

fn selectable(row: &StripRow) -> bool {
    matches!(row, StripRow::Agent { .. })
}

/// The row one arrow key lands on: the next selectable one in that direction,
/// stepping over the headers between. Staying put when there is none makes
/// `Up` on the first agent a no-op rather than a jump onto a header.
fn step(rows: &[StripRow], selected: usize, direction: BareKey) -> usize {
    let next = match direction {
        BareKey::Up => (0..selected).rev().find(|at| selectable(&rows[*at])),
        _ => (selected.saturating_add(1)..rows.len()).find(|at| selectable(&rows[*at])),
    };
    next.unwrap_or(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pane_id: u32, text: &str) -> StripRow {
        StripRow::Agent {
            pane_id,
            text: text.to_owned(),
        }
    }

    fn strip() -> Vec<StripRow> {
        vec![
            StripRow::Project("hocus".to_owned()),
            StripRow::Branch("  main".to_owned()),
            agent(3, "  3  claude  busy"),
            agent(7, "  7  codex  idle"),
            StripRow::Project("no project".to_owned()),
            agent(9, "  9  claude  held"),
        ]
    }

    #[test]
    fn a_fresh_strip_lands_on_the_first_agent_not_the_header() {
        let mut state = State::default();
        assert!(state.set_rows(strip()));
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn stepping_skips_headers_in_both_directions() {
        let mut state = State::default();
        state.set_rows(strip());
        state.selected = step(&state.rows, state.selected, BareKey::Down);
        assert_eq!(state.selected, 3);
        state.selected = step(&state.rows, state.selected, BareKey::Down);
        assert_eq!(state.selected, 5, "down from 7 steps over the header to 9");
        state.selected = step(&state.rows, state.selected, BareKey::Up);
        assert_eq!(state.selected, 3);
        state.selected = step(&state.rows, state.selected, BareKey::Up);
        state.selected = step(&state.rows, state.selected, BareKey::Up);
        assert_eq!(state.selected, 2, "up from the first agent stays put");
    }

    #[test]
    fn a_refresh_follows_the_selected_pane_not_the_row_number() {
        let mut state = State::default();
        state.set_rows(strip());
        state.selected = 3; // pane 7
        let mut rows = strip();
        // a new project appears above, shifting every row down
        rows.insert(0, StripRow::Project("aardvark".to_owned()));
        state.set_rows(rows);
        assert!(matches!(
            state.rows.get(state.selected),
            Some(StripRow::Agent { pane_id: 7, .. })
        ));
    }

    #[test]
    fn a_refresh_that_drops_the_selected_pane_falls_back_to_the_first_agent() {
        let mut state = State::default();
        state.set_rows(strip());
        state.selected = 5; // pane 9
        let rows = vec![
            StripRow::Project("hocus".to_owned()),
            agent(3, "  3  claude"),
        ];
        state.set_rows(rows);
        assert_eq!(state.selected, 1);
    }
}
