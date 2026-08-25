//! hocus-agents — the agent list as a real pane in the hocus overlay column.
//!
//! This used to be the top third of the hocus pane, drawn by hocus itself with
//! an `agents_focused` flag deciding who owned the keys. Now it is its own pane
//! — the overlay directly above the chat in the same bottom-left column — so the
//! ordinary pane-movement keys walk between the list and the conversation and
//! each one keeps its own focus, scrollback and cursor.
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
//! Which pane the user is *in* comes over the same wire, and the cursor follows
//! it: walking into an agent by any route — the pane keys, a tab switch, a tool
//! — selects its row here, so the list shows where you are and not only where
//! you last pointed. Walking onto anything else leaves the row selected and
//! draws it dim, which is the difference between "you are here" and "here is
//! where you were".
//!
//! Either half of the column opens the other, through a `MessageToPlugin`
//! carrying the other's url: that is launch-or-reuse, so hocus opens this pane
//! on load and this pane opens a hocus when a keybinding got here first. A
//! binding that opens the list directly therefore gets a whole column rather
//! than a view with nothing feeding it.
//!
//! **Hiding, though, does not work in both directions.** An overlay is hidden by
//! name, where the old global strip hid by edge and so went away as one unit, and
//! this pane does not know the chat's id. So Esc here sends `AGENT_STRIP_HIDE` to
//! hocus, which holds both ids and puts the column away as one — rather than
//! `hide_self`, which would leave the conversation on screen with its list
//! missing.

use std::collections::BTreeMap;

use hocus_proto::agents::{Strip, StripRow};
use hocus_proto::launch;
use hocus_proto::wire;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    /// The strip as hocus last sent it.
    rows: Vec<StripRow>,
    /// Index into `rows`. Always on an `Agent` row when one exists.
    selected: usize,
    /// The last pane hocus reported focused, agent or not.
    ///
    /// Two jobs. The selection follows it when it *changes* onto an agent, which
    /// is what makes walking to a pane move the cursor here — comparing rather
    /// than assigning is what leaves `j`/`k` alone, since the strip is pushed on
    /// every snapshot and would otherwise drag the cursor back once a second.
    /// And the selected row is drawn live only while this names it, so a
    /// selection you moved off the pane you are in says so.
    ///
    /// A `None` from hocus is ignored on both counts: it means the focus is on a
    /// plugin pane — this one, most often — and coming here to work the list is
    /// not leaving the agent you were in.
    focused: Option<u32>,
    /// Where the last render's window into `rows` began — needed to map a mouse
    /// click's on-screen line back to an index into `rows`.
    last_window_start: usize,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::PermissionRequestResult,
        ]);
        set_selectable(true);
        rename_plugin_pane(get_plugin_ids().plugin_id, "agents");
        // Unconditional, and before there are any rows: what this first one carries
        // is the plugin id, which is how hocus tells "the agents pane is focused"
        // from "some other plugin pane is". The pane it names follows with the
        // strip, a moment later.
        self.send_selection();
        // One message doing two jobs. It asks hocus for the current strip, so a
        // reload (or a session restore) does not sit empty until the next agentd
        // snapshot — and, by naming hocus's url, it opens the chat if there is no
        // hocus to ask. The chat is the other half of this column and the half
        // that computes the rows drawn here, so a list with nothing feeding it is
        // not worth having: usually hocus opened *this* pane and the message
        // simply reaches it, but a keybinding can get here first.
        //
        // `MessageToPlugin` with a `plugin_url` is launch-or-reuse — an instance
        // with the same (location, configuration) is handed the message rather
        // than a second one being spawned — which is the same idempotence hocus
        // relies on in the other direction.
        //
        // No coordinates: this pane does not know the column's proportions. hocus
        // places both halves itself, on load and on every resize.
        pipe_message_to_plugin(
            MessageToPlugin::new(wire::AGENT_STRIP_HELLO)
                .with_plugin_url(launch::HOCUS_ALIAS)
                .with_plugin_config(launch::overlay_configuration()),
        );
    }

    fn update(&mut self, event: Event) -> bool {
        let selection = self.selection();
        let rendered = match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse_event) => self.on_mouse(mouse_event),
            _ => false,
        };
        // Where the cursor moved to is hocus's business, not only this pane's: it
        // draws the selected agent's summary. Compared rather than sent on every
        // key, so holding `j` at the end of the list says nothing.
        if self.selection() != selection {
            self.send_selection();
        }
        rendered
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_STRIP_OPEN {
            // hocus asking for this pane. If it is reaching us at all then we already exist and
            // the message opened nothing - but a hocus that reloaded has forgotten our id, and
            // `send_selection` on load is the only other thing that ever tells it. Answering here
            // is what stops a reloaded hocus being unable to place or hide the list beside it.
            self.send_selection();
            return false;
        }
        if pipe_message.name != wire::AGENT_STRIP {
            return false;
        }
        let Some(strip) = pipe_message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<Strip>(json).ok())
        else {
            return false;
        };
        // The same comparison, for the other thing that moves the selection: a
        // push whose focus walked onto another agent.
        let selection = self.selection();
        let rendered = self.set_rows(strip);
        if self.selection() != selection {
            self.send_selection();
        }
        rendered
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
        self.last_window_start = start;
        let live = self.live();
        for (index, row) in self.rows.iter().skip(start).take(rows).enumerate() {
            let selected = start + index == self.selected;
            let text = match row {
                // A header takes no marker column: nothing can land on it, and
                // the indentation it carries is what shows the nesting.
                StripRow::Project(text) => Text::new(text).color_range(0, ..),
                StripRow::Branch(text) => Text::new(text).color_range(3, ..),
                StripRow::Agent { text, .. } => {
                    let line = Text::new(format!("{} {}", if selected { ">" } else { " " }, text));
                    match (selected, live) {
                        // The selection is a band either way - `selected()` paints
                        // the row's whole width - so it stays findable when the
                        // user is somewhere else. Colour against dim is what says
                        // which of those two it is.
                        (true, true) => line.selected().color_range(2, ..),
                        (true, false) => line.selected().dim_range(..),
                        (false, _) => line.dim_range(..),
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
            // puts the whole column away, chat included - the same gesture that
            // revealed it. Routed through hocus: see the module doc.
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HIDE));
            return false;
        }
        if key.bare_key == BareKey::Char('r') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // the one part of a row no host event keeps fresh is the branch a
            // checkout is on; ask agentd to probe again
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_REFRESH));
            return false;
        }
        if let Some(motion) = motion(&key) {
            let next = step(&self.rows, self.selected, motion);
            let moved = next != self.selected;
            self.selected = next;
            return moved;
        }
        match key.bare_key {
            BareKey::Esc => {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HIDE));
                false
            },
            BareKey::Enter => {
                self.activate();
                false
            },
            _ => false,
        }
    }

    fn on_mouse(&mut self, mouse_event: Mouse) -> bool {
        match mouse_event {
            Mouse::LeftClick(line, _) if line >= 0 => self.on_click(line as usize),
            _ => false,
        }
    }

    /// A click on screen line `line`: select the row it lands on, if any, and
    /// activate it exactly as `Enter` would - clicking an agent row is meant to
    /// feel like navigating to it and pressing Enter, whether or not it was
    /// already selected.
    fn on_click(&mut self, line: usize) -> bool {
        let Some(index) = click_target(&self.rows, self.last_window_start, line) else {
            return false;
        };
        let moved = index != self.selected;
        self.selected = index;
        self.activate();
        moved
    }

    /// What `Enter` does, and now what a click on the selected row does too:
    /// focus the selected agent's pane, if there is one.
    fn activate(&self) {
        if let Some(StripRow::Agent { pane_id, .. }) = self.rows.get(self.selected) {
            // the host resolves which tab holds the pane, so this works across
            // tabs
            focus_terminal_pane(*pane_id, false, false);
        }
    }

    /// Take a fresh strip, staying on the agent the selection was on. The
    /// selection follows the pane rather than the row number, so a project
    /// appearing above it does not move what Enter would focus.
    ///
    /// Focus moving onto an agent overrides that: the pane the user just walked
    /// into is where the cursor belongs, whatever it was on before. Focus landing
    /// anywhere else - a shell, an editor - moves nothing and only dims what is
    /// selected, because the answer to "which agent" has not changed, only
    /// whether you are in it.
    fn set_rows(&mut self, strip: Strip) -> bool {
        let Strip { rows, focused } = strip;
        let walked_to = focused
            .filter(|_| focused != self.focused)
            .and_then(|pane_id| position_of(&rows, pane_id));
        let was = match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => Some(*pane_id),
            _ => None,
        };
        let selected = walked_to
            .or_else(|| was.and_then(|pane_id| position_of(&rows, pane_id)))
            .or_else(|| rows.iter().position(selectable))
            .unwrap_or(0);
        // Only a real pane replaces the focus: see the field's doc.
        let focused = focused.or(self.focused);
        let changed = self.rows != rows
            || self.selected != selected
            || self.live() != live(&rows, selected, focused);
        self.rows = rows;
        self.selected = selected;
        self.focused = focused;
        changed
    }

    /// Whether the selected row is the pane the user is actually in.
    fn live(&self) -> bool {
        live(&self.rows, self.selected, self.focused)
    }

    /// The agent the cursor is on, which is what hocus draws a summary for.
    ///
    /// `None` on a strip with nothing selectable in it. A header cannot be selected,
    /// so this is `Some` whenever there is an agent at all.
    fn selection(&self) -> Option<u32> {
        match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => Some(*pane_id),
            _ => None,
        }
    }

    /// Tell hocus where the cursor is.
    ///
    /// Called from the plugin entry points rather than from `on_key` and `set_rows`,
    /// which the tests drive directly and which stay free of host calls for that
    /// reason. The plugin id goes along every time: it is how hocus knows that this
    /// pane, and not the chat below it, is the one the daemon reports focused.
    fn send_selection(&self) {
        let selection = wire::Selection {
            plugin_id: get_plugin_ids().plugin_id,
            pane_id: self.selection(),
        };
        let Ok(payload) = serde_json::to_string(&selection) else {
            return;
        };
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SELECTION).with_payload(payload));
    }
}

/// Whether the row at `selected` names the focused pane - "you are here", as
/// against "here is where you were", which is the whole difference between the
/// two ways a selection is drawn.
fn live(rows: &[StripRow], selected: usize, focused: Option<u32>) -> bool {
    matches!(
        (rows.get(selected), focused),
        (Some(StripRow::Agent { pane_id, .. }), Some(id)) if *pane_id == id
    )
}

/// Where a pane sits in the strip, if it is an agent at all.
fn position_of(rows: &[StripRow], pane_id: u32) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, StripRow::Agent { pane_id: id, .. } if *id == pane_id))
}

fn selectable(row: &StripRow) -> bool {
    matches!(row, StripRow::Agent { .. })
}

/// The row a click on screen line `line` resolves to, if any - `None` for a
/// header row or a click past the last row.
fn click_target(rows: &[StripRow], window_start: usize, line: usize) -> Option<usize> {
    let index = window_start + line;
    rows.get(index).filter(|row| selectable(row)).map(|_| index)
}

/// Which way a keypress moves the selection.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Motion {
    Up,
    Down,
}

/// The motion a key asks for, if any. `j`/`k` are the vim spelling of the
/// arrows: this pane is a list and never a text field, so the letters are free
/// to take. They want the letter on its own, leaving `Ctrl j` and friends to
/// whatever binds them; the arrows stay modifier-agnostic as they always were.
fn motion(key: &KeyWithModifier) -> Option<Motion> {
    match key.bare_key {
        BareKey::Up => Some(Motion::Up),
        BareKey::Down => Some(Motion::Down),
        BareKey::Char('k') if key.has_no_modifiers() => Some(Motion::Up),
        BareKey::Char('j') if key.has_no_modifiers() => Some(Motion::Down),
        _ => None,
    }
}

/// The row one motion lands on: the next selectable one in that direction,
/// stepping over the headers between. Staying put when there is none makes
/// `Up` on the first agent a no-op rather than a jump onto a header.
fn step(rows: &[StripRow], selected: usize, motion: Motion) -> usize {
    let next = match motion {
        Motion::Up => (0..selected).rev().find(|at| selectable(&rows[*at])),
        Motion::Down => (selected.saturating_add(1)..rows.len()).find(|at| selectable(&rows[*at])),
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

    /// A push from hocus with nobody in a terminal pane - what the hocus column
    /// having the focus looks like on the wire.
    fn unfocused(rows: Vec<StripRow>) -> Strip {
        Strip {
            rows,
            focused: None,
        }
    }

    /// A push from hocus with `pane_id` focused.
    fn focused_on(rows: Vec<StripRow>, pane_id: u32) -> Strip {
        Strip {
            rows,
            focused: Some(pane_id),
        }
    }

    #[test]
    fn a_fresh_strip_lands_on_the_first_agent_not_the_header() {
        let mut state = State::default();
        assert!(state.set_rows(unfocused(strip())));
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn stepping_skips_headers_in_both_directions() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = step(&state.rows, state.selected, Motion::Down);
        assert_eq!(state.selected, 3);
        state.selected = step(&state.rows, state.selected, Motion::Down);
        assert_eq!(state.selected, 5, "down from 7 steps over the header to 9");
        state.selected = step(&state.rows, state.selected, Motion::Up);
        assert_eq!(state.selected, 3);
        state.selected = step(&state.rows, state.selected, Motion::Up);
        state.selected = step(&state.rows, state.selected, Motion::Up);
        assert_eq!(state.selected, 2, "up from the first agent stays put");
    }

    /// What hocus is told, and the reason the keys above are worth a message:
    /// the selection names an agent, and hocus draws that agent's summary.
    #[test]
    fn the_selection_is_the_agent_under_the_cursor() {
        let mut state = State::default();
        // Nothing on the list at all, and nothing to name.
        assert_eq!(state.selection(), None);

        state.set_rows(unfocused(strip()));
        assert_eq!(state.selection(), Some(3));
        state.selected = step(&state.rows, state.selected, Motion::Down);
        assert_eq!(state.selection(), Some(7));

        // A header cannot be walked onto, but a strip of nothing else can arrive.
        state.set_rows(unfocused(vec![StripRow::Project("hocus".to_owned())]));
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn j_and_k_are_the_arrows() {
        assert_eq!(
            motion(&KeyWithModifier::new(BareKey::Down)),
            Some(Motion::Down)
        );
        assert_eq!(motion(&KeyWithModifier::new(BareKey::Up)), Some(Motion::Up));
        assert_eq!(
            motion(&KeyWithModifier::new(BareKey::Char('j'))),
            Some(Motion::Down)
        );
        assert_eq!(
            motion(&KeyWithModifier::new(BareKey::Char('k'))),
            Some(Motion::Up)
        );
    }

    #[test]
    fn a_click_resolves_to_the_agent_row_under_it() {
        let rows = strip();
        // line 3 is "  7  codex  idle", offset by a window that starts at 1
        assert_eq!(click_target(&rows, 1, 2), Some(3));
    }

    #[test]
    fn a_click_on_a_header_row_hits_nothing() {
        let rows = strip();
        assert_eq!(click_target(&rows, 0, 0), None, "the Project header");
        assert_eq!(click_target(&rows, 0, 1), None, "the Branch header");
    }

    #[test]
    fn a_click_past_the_last_row_hits_nothing() {
        let rows = strip();
        assert_eq!(click_target(&rows, 0, rows.len()), None);
    }

    #[test]
    fn a_modified_or_unrelated_letter_is_not_a_motion() {
        assert_eq!(
            motion(&KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier()),
            None
        );
        assert_eq!(
            motion(&KeyWithModifier::new(BareKey::Char('k')).with_alt_modifier()),
            None
        );
        assert_eq!(motion(&KeyWithModifier::new(BareKey::Char('x'))), None);
    }

    #[test]
    fn jk_walk_the_selection_the_way_the_arrows_do() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert_eq!(state.selected, 3);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(state.selected, 2);
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Char('k'))),
            "no move means nothing to redraw"
        );
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn a_refresh_follows_the_selected_pane_not_the_row_number() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 3; // pane 7
        let mut rows = strip();
        // a new project appears above, shifting every row down
        rows.insert(0, StripRow::Project("aardvark".to_owned()));
        state.set_rows(unfocused(rows));
        assert!(matches!(
            state.rows.get(state.selected),
            Some(StripRow::Agent { pane_id: 7, .. })
        ));
    }

    #[test]
    fn a_refresh_that_drops_the_selected_pane_falls_back_to_the_first_agent() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 5; // pane 9
        let rows = vec![
            StripRow::Project("hocus".to_owned()),
            agent(3, "  3  claude"),
        ];
        state.set_rows(unfocused(rows));
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn walking_into_an_agent_pane_moves_the_selection_onto_it() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert_eq!(state.selected, 2, "pane 3, for want of anything better");

        assert!(state.set_rows(focused_on(strip(), 9)));
        assert_eq!(state.selected, 5);
        assert!(state.live(), "the selected agent is the one we are in");
    }

    #[test]
    fn focus_on_something_that_is_not_an_agent_leaves_the_selection_standing() {
        let mut state = State::default();
        state.set_rows(focused_on(strip(), 7));
        assert_eq!(state.selected, 3);

        // pane 4 is a shell, an editor, anything at all - it is in no row
        assert!(state.set_rows(focused_on(strip(), 4)));
        assert_eq!(state.selected, 3, "still the agent we were last in");
        assert!(!state.live(), "but we are no longer in it");
    }

    #[test]
    fn focus_on_a_plugin_pane_changes_nothing_at_all() {
        let mut state = State::default();
        state.set_rows(focused_on(strip(), 7));

        // hocus sends `None` when a plugin pane has the focus, which is what
        // coming to this very list to work it looks like
        assert!(!state.set_rows(unfocused(strip())));
        assert_eq!(state.selected, 3);
        assert!(state.live(), "walking into the list is not leaving pane 7");
    }

    #[test]
    fn a_push_that_does_not_move_the_focus_leaves_a_jk_selection_alone() {
        let mut state = State::default();
        state.set_rows(focused_on(strip(), 3));
        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        assert_eq!(state.selected, 3, "pane 7");

        // the snapshot pushes that keep arriving must not drag the cursor back
        state.set_rows(focused_on(strip(), 3));
        assert_eq!(state.selected, 3);
        assert!(!state.live(), "the cursor left the pane we are in");
    }

    #[test]
    fn stepping_off_the_focused_agent_and_back_takes_the_liveness_with_it() {
        let mut state = State::default();
        state.set_rows(focused_on(strip(), 7));
        assert!(state.live());

        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        assert!(!state.live(), "selected pane 9, sitting in pane 7");
        state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        assert!(state.live());
    }

    /// A redraw is owed for liveness alone: the rows and the selection can both
    /// be identical while the band goes from coloured to dim.
    #[test]
    fn losing_the_focus_is_worth_a_redraw() {
        let mut state = State::default();
        state.set_rows(focused_on(strip(), 3));
        assert!(state.live());

        assert!(state.set_rows(focused_on(strip(), 4)));
        assert_eq!(state.selected, 2);
        assert!(!state.live());
    }
}
