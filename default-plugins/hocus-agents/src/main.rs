//! hocus-agents — the agent list, an overlay pane of its own.
//!
//! This used to be the top third of the hocus pane, drawn by hocus itself with
//! an `agents_focused` flag deciding who owned the keys. Then it was the upper
//! half of a two-pane column. It is now simply its own pane, sitting beside the
//! conversation rather than above it, and opening, hiding and being read on its
//! own.
//!
//! **hocus computes, this pane draws.** Which panes are agents, which project
//! owns them and what branch they are on takes the agentd snapshot, the project
//! registry and the Git probes — all of which hocus already holds for its
//! tools. Deriving them again here would mean a second copy of that state to
//! keep in step, so hocus pipes the *finished rows* over (`wire::AGENT_STRIP`)
//! and this plugin is a view: it windows them, tracks a selection, draws the
//! selected agent's idle summary under the list, and focuses a pane on Enter.
//! `AGENT_STRIP_HELLO` is the reload handshake — a freshly loaded instance
//! announces itself and hocus answers with the current rows.
//!
//! Which pane the user is *in* comes over the same wire, and the cursor follows
//! it: walking into an agent by any route — the pane keys, a tab switch, a tool
//! — selects its row here, so the list shows where you are and not only where
//! you last pointed. Walking onto anything else leaves the row selected and
//! draws it dim, which is the difference between "you are here" and "here is
//! where you were".
//!
//! **The two panes are separate, with one exception.** Esc here hides this pane
//! and nothing else, and hocus's Esc hides hocus. Enter keeps that same line:
//! having sent you to an agent, the list puts itself away and leaves an open
//! conversation open. Neither opens the other for the user. The exception is
//! `say_hello_to_hocus`, which does carry hocus's url: this pane is a view over
//! rows only hocus can compute, so a list with no hocus to ask has nothing to
//! draw. The hocus that greeting launches starts **hidden** — `wire::OPENED_BY`
//! is what tells it so — because the user asked for a list, not for a
//! conversation.

use std::collections::BTreeMap;

use hocus_proto::agents::{self, Strip, StripRow, Summaries};
use hocus_proto::launch;
use hocus_proto::wire;
use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    /// The strip as hocus last sent it.
    rows: Vec<StripRow>,
    /// What the daemon's model last made of each idle agent, as hocus last sent it.
    ///
    /// The whole map rather than only the selected row's: the selection moves here, under `j`/`k`,
    /// and asking hocus for the box on every keystroke would put a round trip between the cursor
    /// and what it is pointing at.
    summaries: Summaries,
    /// Full history for the selected row, fetched on demand by hocus.
    run: Option<agents::AgentRun>,
    history_scroll: usize,
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
    /// How many rows the last render gave the list. The rest of the pane is the summary box,
    /// which is drawn under the list and points at nothing: a click down there has to be
    /// rejected rather than mapped through `last_window_start` onto a row that was never drawn.
    last_list_rows: usize,
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
        self.say_hello_to_hocus();
    }

    fn update(&mut self, event: Event) -> bool {
        let selection = self.selection_key();
        let rendered = match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse_event) => self.on_mouse(mouse_event),
            _ => false,
        };
        // Where the cursor moved to is hocus's business, not only this pane's: it
        // draws the selected agent's summary. Compared rather than sent on every
        // key, so holding `j` at the end of the list says nothing.
        if self.selection_key() != selection {
            self.history_scroll = 0;
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
        if pipe_message.name == wire::AGENT_RUN_DETAIL {
            let Some(run) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<agents::AgentRun>(json).ok())
            else {
                return false;
            };
            let changed = self.run.as_ref() != Some(&run);
            self.run = Some(run);
            return changed;
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
        let selection = self.selection_key();
        let rendered = self.set_rows(strip);
        if self.selection_key() != selection {
            self.history_scroll = 0;
            self.send_selection();
        }
        rendered
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if self.rows.is_empty() {
            self.last_list_rows = 0;
            print_text_with_coordinates(
                Text::new("no agents").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
            return;
        }
        // The box is built first because the list has to be told how many rows are left. It is
        // pinned to the foot of the pane, under the list rather than over it: the list is what
        // the keys work, and a box that covered rows the cursor can move onto would hide the
        // thing it is describing.
        let summary = self.summary_box(rows, cols);
        let list_rows = rows.saturating_sub(summary.len());
        // Window the list around the selection rather than tracking a scroll
        // offset: keeping it near the middle means it is always visible and the
        // list moves by one row per keypress in both directions.
        let start = self
            .selected
            .saturating_sub(list_rows / 2)
            .min(self.rows.len().saturating_sub(list_rows));
        self.last_window_start = start;
        self.last_list_rows = list_rows;
        let live = self.live();
        for (index, row) in self.rows.iter().skip(start).take(list_rows).enumerate() {
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
                StripRow::Run { text, .. } => {
                    let line = Text::new(format!("{}   {text}", if selected { ">" } else { " " }));
                    if selected {
                        line.selected().color_range(4, ..)
                    } else {
                        line.dim_range(..)
                    }
                },
                StripRow::MoreRuns => {
                    let line = Text::new(format!(
                        "{}   load older runs…",
                        if selected { ">" } else { " " }
                    ));
                    if selected {
                        line.selected()
                    } else {
                        line.dim_range(..)
                    }
                },
            };
            print_text_with_coordinates(text, 0, index, Some(cols), None);
        }
        render_summary(&summary, list_rows, cols);
    }
}

/// Draw the box `summary_box` built, at the foot of the pane.
///
/// Every row is printed across the whole pane width with the margin as part of the string: a
/// print only paints the columns it is given, and a row of the list showing through beside the
/// box would read as part of it.
fn render_summary(rows: &[String], top: usize, cols: usize) {
    let Some(last) = rows.len().checked_sub(1) else {
        return;
    };
    let width = rows[0].width();
    for (index, row) in rows.iter().enumerate() {
        let padded = format!(" {row}{}", " ".repeat(cols.saturating_sub(width + 1)));
        let text = Text::new(&padded);
        let text = match index == 0 || index == last {
            // Border rows are frame all the way across.
            true => text.dim_range(1..width + 1),
            // Body rows are frame at each end and the model's words between.
            false => text.dim_range(1..2).dim_range(width..width + 1),
        };
        // Found rather than counted: a label cut to fit may have lost the marker,
        // and no kind can contain one (`valid_kind` allows no punctuation). The
        // one is the margin column every row is printed behind.
        let text = match index == 0 {
            true => match row.chars().position(|c| c == '!') {
                Some(at) => text.error_color_range(at + 1..at + 2),
                None => text,
            },
            false => text,
        };
        print_text_with_coordinates(text, 0, top + index, Some(cols), None);
    }
}

impl State {
    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Char('`') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // puts this pane away - the same gesture that revealed it. This pane and
            // nothing else: the conversation beside it, if there is one, is the user's
            // to put away with its own Esc.
            hide_self();
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
        if key.has_no_modifiers() {
            match key.bare_key {
                BareKey::PageUp if self.selected_run_is_loaded() => {
                    self.history_scroll = self.history_scroll.saturating_add(3);
                    return true;
                },
                BareKey::PageDown if self.selected_run_is_loaded() => {
                    self.history_scroll = self.history_scroll.saturating_sub(3);
                    return true;
                },
                _ => {},
            }
        }
        match key.bare_key {
            BareKey::Esc => {
                hide_self();
                false
            },
            BareKey::Enter => {
                if matches!(self.rows.get(self.selected), Some(StripRow::MoreRuns)) {
                    pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_ARCHIVE_MORE));
                    return false;
                }
                if !matches!(self.rows.get(self.selected), Some(StripRow::Agent { .. })) {
                    return false;
                }
                self.activate();
                // Walking to an agent is the end of what this list is for, so
                // put it away - the same gesture as Esc, and this pane only: a
                // conversation open beside it stays open. Focus first, then
                // hide, so the focus call lands while this pane is still up.
                hide_self();
                false
            },
            _ => false,
        }
    }

    fn on_mouse(&mut self, mouse_event: Mouse) -> bool {
        match mouse_event {
            Mouse::LeftClick(line, _) if line >= 0 => self.on_click(line as usize),
            Mouse::ScrollUp(lines) if self.selected_run_is_loaded() => {
                self.history_scroll = self.history_scroll.saturating_add(lines.max(1));
                true
            },
            Mouse::ScrollDown(lines) if self.selected_run_is_loaded() => {
                self.history_scroll = self.history_scroll.saturating_sub(lines.max(1));
                true
            },
            _ => false,
        }
    }

    /// A click on screen line `line`: select the row it lands on, if any, and
    /// focus it the way `Enter` does - clicking an agent row is meant to feel
    /// like navigating to it and pressing Enter, whether or not it was already
    /// selected. Short of the dismissal: see `activate`.
    fn on_click(&mut self, line: usize) -> bool {
        let Some(index) = click_target(
            &self.rows,
            self.last_window_start,
            self.last_list_rows,
            line,
        ) else {
            return false;
        };
        let moved = index != self.selected;
        self.selected = index;
        self.activate();
        moved
    }

    /// The half of `Enter` a click on the selected row shares: focus the
    /// selected agent's pane, if there is one. The dismissal that follows a
    /// keypress is not part of it - a click leaves the list up, because the
    /// pointer is already where the next click would go.
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
        let Strip {
            rows,
            focused,
            summaries,
        } = strip;
        let walked_to = focused
            .filter(|_| focused != self.focused)
            .and_then(|pane_id| position_of(&rows, pane_id));
        let was = self.rows.get(self.selected).and_then(row_key);
        let selected = walked_to
            .or_else(|| was.as_deref().and_then(|key| position_of_key(&rows, key)))
            .or_else(|| rows.iter().position(selectable))
            .unwrap_or(0);
        // Only a real pane replaces the focus: see the field's doc.
        let focused = focused.or(self.focused);
        let changed = self.rows != rows
            || self.selected != selected
            || self.summaries != summaries
            || self.live() != live(&rows, selected, focused);
        self.rows = rows;
        self.selected = selected;
        self.summaries = summaries;
        self.focused = focused;
        changed
    }

    /// The idle summary for the row under the cursor, as the rows of a framed box - empty when
    /// there is nothing to say or nowhere to put it.
    ///
    /// The box is given at most half the pane so the list it is about is never reduced to a
    /// couple of rows by a talkative model, and `agents::summary_box` caps it again at its own
    /// ceiling. No focus gate, unlike the inset this replaced: that one was drawn in the pane
    /// *beside* the list and had to say which pane it was about; this one is in the list, under
    /// the row it names.
    fn summary_box(&self, rows: usize, cols: usize) -> Vec<String> {
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| self.selected_run_matches(run))
        {
            let width = cols.saturating_sub(2).min(agents::SUMMARY_BOX_MAX_COLS);
            return agents::run_box(run, width, rows / 2, self.history_scroll).unwrap_or_default();
        }
        let Some(pane_id) = self.selection() else {
            return Vec::new();
        };
        let Some(summary) = self.summaries.get(&pane_id) else {
            return Vec::new();
        };
        let Some(StripRow::Agent { kind, .. }) = self.rows.get(self.selected) else {
            return Vec::new();
        };
        // Room for the frame, a space either side of the text, and a margin column at each edge
        // of the pane.
        let width = cols.saturating_sub(2).min(agents::SUMMARY_BOX_MAX_COLS);
        agents::summary_box(pane_id, kind, summary, width, rows / 2).unwrap_or_default()
    }

    /// Whether the selected row is the pane the user is actually in.
    fn live(&self) -> bool {
        live(&self.rows, self.selected, self.focused)
    }

    /// The agent the cursor is on: the row `Enter` focuses, and the one the summary box
    /// under the list is about.
    ///
    /// `None` on a strip with nothing selectable in it. A header cannot be selected,
    /// so this is `Some` whenever there is an agent at all.
    fn selection(&self) -> Option<u32> {
        match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => Some(*pane_id),
            _ => None,
        }
    }

    fn selected_run_id(&self) -> Option<String> {
        match self.rows.get(self.selected) {
            Some(StripRow::Run { run_id, .. }) => Some(run_id.clone()),
            _ => None,
        }
    }

    fn selection_key(&self) -> Option<String> {
        self.rows.get(self.selected).and_then(row_key)
    }

    fn selected_run_matches(&self, run: &agents::AgentRun) -> bool {
        match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => {
                *pane_id == run.pane_id && run.ended_at_ms.is_none()
            },
            Some(StripRow::Run { run_id, .. }) => run_id == &run.id,
            _ => false,
        }
    }

    fn selected_run_is_loaded(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| self.selected_run_matches(run))
    }

    /// Ask hocus for the current strip - and open one if there is none to ask.
    ///
    /// One message doing two jobs. It asks hocus for the current strip, so a reload (or a session
    /// restore) does not sit empty until the next agentd snapshot - and, by naming hocus's url, it
    /// opens a hocus if there is none. hocus is what computes the rows drawn here, so a list with
    /// nothing feeding it is not worth having.
    ///
    /// The hocus it opens that way starts **hidden**, which is what keeps this from being the
    /// coupling the two panes no longer have: the user asked for a list. `wire::OPENED_BY` is
    /// the marker that says so, and `should_focus: false` keeps the keyboard here in the meantime.
    ///
    /// `MessageToPlugin` with a `plugin_url` is launch-or-reuse - an instance with the same
    /// (location, configuration) is handed the message rather than a second one being spawned -
    /// which is the same idempotence hocus relies on in the other direction.
    ///
    /// No coordinates: this pane does not know the panel's proportions. hocus places both panes
    /// itself, on load and on every resize.
    fn say_hello_to_hocus(&self) {
        pipe_message_to_plugin(
            MessageToPlugin::new(wire::AGENT_STRIP_HELLO)
                .with_plugin_url(launch::HOCUS_ALIAS)
                .with_plugin_config(launch::overlay_configuration())
                .with_args(BTreeMap::from([(
                    wire::OPENED_BY.to_owned(),
                    wire::OPENED_BY_AGENTS.to_owned(),
                )]))
                // Only read if a hocus actually has to be launched, which is the case this is
                // for: the user asked for a list, so the keyboard stays here rather than
                // jumping to a conversation they did not ask to see - and which, being opened
                // this way, hides itself the moment it has a pane to hide.
                .new_plugin_instance_should_not_be_focused(),
        );
    }

    /// Tell hocus where the cursor is, and - the part hocus actually needs - who we are.
    ///
    /// Called from the plugin entry points rather than from `on_key` and `set_rows`,
    /// which the tests drive directly and which stay free of host calls for that
    /// reason. The plugin id goes along every time: it is the only word hocus gets of
    /// this pane's id, and without it hocus cannot place the list beside itself.
    fn send_selection(&self) {
        let selection = wire::Selection {
            plugin_id: get_plugin_ids().plugin_id,
            pane_id: self.selection(),
            run_id: self.selected_run_id(),
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

fn row_key(row: &StripRow) -> Option<String> {
    match row {
        StripRow::Agent { pane_id, .. } => Some(format!("pane:{pane_id}")),
        StripRow::Run { run_id, .. } => Some(format!("run:{run_id}")),
        StripRow::MoreRuns => Some("more".to_owned()),
        _ => None,
    }
}

fn position_of_key(rows: &[StripRow], key: &str) -> Option<usize> {
    rows.iter()
        .position(|row| row_key(row).as_deref() == Some(key))
}

fn selectable(row: &StripRow) -> bool {
    matches!(
        row,
        StripRow::Agent { .. } | StripRow::Run { .. } | StripRow::MoreRuns
    )
}

/// The row a click on screen line `line` resolves to, if any - `None` for a
/// header row, a click past the last row, or a click on the summary box under
/// the list, which `list_rows` is the height of the list above.
fn click_target(
    rows: &[StripRow],
    window_start: usize,
    list_rows: usize,
    line: usize,
) -> Option<usize> {
    if line >= list_rows {
        return None;
    }
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
            // the second field of the line, the way hocus spells it
            kind: text
                .split_whitespace()
                .nth(1)
                .unwrap_or("claude")
                .to_owned(),
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

    /// A push from hocus with nobody in a terminal pane - what one of the hocus
    /// panes having the focus looks like on the wire.
    fn unfocused(rows: Vec<StripRow>) -> Strip {
        Strip {
            rows,
            focused: None,
            summaries: Summaries::new(),
        }
    }

    /// A push from hocus with `pane_id` focused.
    fn focused_on(rows: Vec<StripRow>, pane_id: u32) -> Strip {
        Strip {
            rows,
            focused: Some(pane_id),
            summaries: Summaries::new(),
        }
    }

    /// A push carrying one summary, for the box under the list.
    fn with_summary(rows: Vec<StripRow>, pane_id: u32, text: &str, awaiting: bool) -> Strip {
        Strip {
            rows,
            focused: None,
            summaries: Summaries::from([(
                pane_id,
                agents::Summary {
                    text: text.to_owned(),
                    title: "running the tests".to_owned(),
                    awaiting,
                    hash: 1,
                },
            )]),
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
        assert_eq!(click_target(&rows, 1, rows.len(), 2), Some(3));
    }

    #[test]
    fn a_click_on_a_header_row_hits_nothing() {
        let rows = strip();
        assert_eq!(
            click_target(&rows, 0, rows.len(), 0),
            None,
            "the Project header"
        );
        assert_eq!(
            click_target(&rows, 0, rows.len(), 1),
            None,
            "the Branch header"
        );
    }

    #[test]
    fn a_click_past_the_last_row_hits_nothing() {
        let rows = strip();
        assert_eq!(click_target(&rows, 0, rows.len(), rows.len()), None);
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

    // -- the summary box ----------------------------------------------------

    /// The box is about the row under the cursor, and it names it: the pane id and the kind,
    /// which is why `StripRow::Agent` carries the kind at all.
    #[test]
    fn the_box_names_the_selected_agent() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests, all green.", false));
        let rows = state.summary_box(20, 40);
        assert!(rows[0].starts_with("┌─ 3 · claude ─"), "{}", rows[0]);
        assert!(rows.iter().any(|row| row.contains("all green")), "{rows:?}");

        // and it follows the cursor off that row
        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        assert!(
            state.summary_box(20, 40).is_empty(),
            "pane 7 has no summary"
        );
    }

    #[test]
    fn an_archived_run_draws_its_summary_and_timeline() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        state.run = Some(agents::AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 3,
            zellij_session_name: "zellij".to_owned(),
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: Some("/work".to_owned()),
            started_at_ms: 10,
            ended_at_ms: Some(20),
            outcome: agents::RunOutcome::Completed,
            revision: 4,
            summarized_revision: 4,
            summary: Some(agents::Summary {
                text: "History is stored and rendered.".to_owned(),
                title: "hook history".to_owned(),
                awaiting: false,
                hash: 0,
            }),
            entries: vec![
                agents::RunEntry {
                    revision: 1,
                    event_id: None,
                    kind: agents::RunEntryKind::Prompt,
                    text: "implement history".to_owned(),
                },
                agents::RunEntry {
                    revision: 2,
                    event_id: None,
                    kind: agents::RunEntryKind::Question,
                    text: "Keep old runs?".to_owned(),
                },
                agents::RunEntry {
                    revision: 3,
                    event_id: None,
                    kind: agents::RunEntryKind::Step,
                    text: "Bash cargo test".to_owned(),
                },
                agents::RunEntry {
                    revision: 4,
                    event_id: None,
                    kind: agents::RunEntryKind::Result,
                    text: "Implemented and tested.".to_owned(),
                },
            ],
        });

        let rows = state.summary_box(20, 50);
        assert!(rows[0].contains("claude · archived"), "{}", rows[0]);
        assert!(rows.iter().any(|row| row.contains("summary: History")));
        assert!(rows.iter().any(|row| row.contains("question: Keep")));
        assert!(!rows.iter().any(|row| row.contains("Bash cargo")));
        assert!(!rows.iter().any(|row| row.contains("result: Implemented")));
        assert!(state.on_mouse(Mouse::ScrollUp(2)));
        assert_eq!(state.history_scroll, 2);
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        assert_eq!(state.history_scroll, 1);
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Enter)));
    }

    /// `render_summary` colours the `!` in a box header by *searching* the line for the
    /// character — the label is cut to fit, so its position cannot be counted. That makes
    /// every glyph the header can carry a hazard: one containing a `!` would light up the
    /// wrong column. The hook marker is the newest of them, so it is worth pinning here
    /// rather than trusting that nobody ever picks an exclamation mark for one.
    #[test]
    fn the_hook_marker_cannot_be_mistaken_for_the_waiting_one() {
        assert!(!agents::HOOK_MARKER.contains('!'));

        let mut state = State::default();
        state.set_rows(with_summary(
            vec![
                StripRow::Project("hocus".to_owned()),
                StripRow::Agent {
                    pane_id: 3,
                    text: "  3  claude◆  waiting".to_owned(),
                    kind: "claude◆".to_owned(),
                },
            ],
            3,
            "Waiting on a permission prompt.",
            true,
        ));
        let rows = state.summary_box(20, 40);
        // The one `!` in the header is the waiting marker, and it is the last of the
        // label rather than anywhere inside the kind.
        let header = &rows[0];
        assert!(header.contains("3 · claude◆"), "{header}");
        assert_eq!(header.matches('!').count(), 1, "{header}");
        assert!(
            header.find('!').unwrap() > header.find('◆').unwrap(),
            "{header}"
        );
    }

    /// A pane with room for the list and nothing else draws the list. `summary_box` is given
    /// half the pane, and `agents::summary_box` refuses anything it cannot frame.
    #[test]
    fn there_is_no_box_where_there_is_no_room_for_one() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        assert!(state.summary_box(5, 40).is_empty(), "too few rows");
        assert!(state.summary_box(20, 12).is_empty(), "too narrow");
    }

    /// The box takes its rows off the bottom of the pane, so the window into the list has to
    /// shrink by exactly as many - otherwise a click maps to the row above or below the one
    /// under the pointer.
    #[test]
    fn the_list_windows_above_the_box() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests, all green.", false));
        let rows = 10;
        let list_rows = rows - state.summary_box(rows, 40).len();
        assert!(list_rows < rows, "the box took rows off the list");

        // the whole strip still fits above the box here, so the window starts at the top and a
        // click lands on the row it points at
        assert!(state.rows.len() <= list_rows);
        assert_eq!(click_target(&state.rows, 0, list_rows, 3), Some(3));
    }

    /// And a click *on* the box hits nothing. Without the height the line would be mapped
    /// through the window start onto a row the box is covering, so pointing at the model's
    /// words would focus an agent the user never pointed at.
    #[test]
    fn a_click_on_the_summary_box_hits_nothing() {
        let mut state = State::default();
        // a strip longer than the window, so the rows the box covers are real rows
        let mut rows = strip();
        for pane_id in 20..30 {
            rows.push(agent(pane_id, &format!("  {pane_id}  claude  idle")));
        }
        state.set_rows(with_summary(rows, 3, "Ran the tests, all green.", false));

        let pane_rows = 12;
        let list_rows = pane_rows - state.summary_box(pane_rows, 40).len();
        assert!(list_rows < pane_rows, "the box took rows off the list");
        assert!(
            state.rows.len() > list_rows,
            "and the strip outruns what is left"
        );

        assert_eq!(click_target(&state.rows, 0, list_rows, list_rows), None);
        assert!(
            click_target(&state.rows, 0, list_rows, list_rows - 1).is_some(),
            "the last row of the list is still clickable"
        );
    }
}
