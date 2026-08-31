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
//! **The pane is two permanent halves.** The list is the top one; the events history
//! of whatever the cursor is on is the bottom one, with a divider between them naming
//! it. The history used to be a framed box pinned to the foot of the pane and drawn
//! only when there was something to put in it, which meant the list's height — and so
//! which row a click landed on — moved with what the selected agent happened to have
//! said. The halves are fixed now, and an empty history says it is empty. `Tab` moves
//! the keyboard between them and so does a click; the divider is where the focus shows,
//! because the list's own dim-against-colour already means something else.
//!
//! Which pane the user is *in* comes over the same wire, and the cursor follows
//! it: walking into an agent by any route — the pane keys, a tab switch, a tool
//! — selects its row here, so the list shows where you are and not only where
//! you last pointed. Walking onto anything else leaves the row selected and
//! draws it dim, which is the difference between "you are here" and "here is
//! where you were".
//!
//! **The two panes are separate, with one exception.** Esc here hides this pane
//! and nothing else, and hocus's Esc hides hocus. Ctrl-o moves this pane out of
//! the way of a tiled pane and leaves the conversation where it is — hocus does
//! the arithmetic, since it places both overlays, so the key is a request
//! (`wire::AGENT_STRIP_REPOSITION`) rather than a move. Enter keeps that same
//! line: having sent you to an agent, the list puts itself away and leaves an
//! open conversation open. Neither opens the other for the user. The exception is
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
    /// Lines the history half is wound back from its newest entry.
    history_scroll: usize,
    /// Which half the keys work. Not reset by anything hocus pushes: a strip arriving while
    /// the user is reading the history must not throw them back into the list.
    focus: Focus,
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
    /// How many rows the last render gave the list. Below it are the divider and the history
    /// half, which point at nothing: a click down there has to be rejected rather than mapped
    /// through `last_window_start` onto a row that was never drawn.
    last_list_rows: usize,
    /// How many rows the last render gave the history half, and `0` on a pane too short to
    /// split. A page key and a focus switch both need to know whether there is a half there.
    last_history_rows: usize,
    /// The largest `history_scroll` the last render could actually honour.
    ///
    /// Kept for the same reason `last_window_start` is: how many lines the history has
    /// depends on the width it is wrapped to, and only `render` is told the width. Without
    /// it a wheel against the top of the history would keep counting, and scrolling back down
    /// would cost as many notches as were wasted going up.
    last_history_max_scroll: usize,
}

/// Which half of the pane the keys work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    List,
    History,
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
            // Nothing to select and so nothing to have a history: the pane says so once
            // rather than splitting an empty list off an empty history.
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_max_scroll = 0;
            print_text_with_coordinates(
                Text::new("no agents").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
            return;
        }
        let (list_rows, history_rows) = split(rows);
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
        self.last_history_rows = history_rows;
        if history_rows == 0 {
            // A pane too short to hold both halves is the list, and the focus cannot be
            // anywhere else.
            self.focus = Focus::List;
            self.last_history_max_scroll = 0;
            return;
        }
        self.render_history(list_rows, history_rows, cols);
    }
}

/// The rule between the two halves: a label naming what is below it, the rest of the row ruled
/// off, and the scroll position at the right-hand end.
///
/// Where the focus shows. The list's own selection band is dim or coloured already, and that
/// says whether the selected agent is the pane the user is *in* - overloading it with "and the
/// keys are up here" would cost the pane the only thing it says about the agent.
fn render_divider(label: &str, above: usize, below: usize, focused: bool, y: usize, cols: usize) {
    let line = divider_line(label, above, below, cols);
    // Found rather than counted: a label cut to fit may have lost the marker, and no kind can
    // contain one (`valid_kind` allows no punctuation).
    let marker = label
        .contains('!')
        .then(|| line.chars().position(|c| c == '!'))
        .flatten();
    let text = Text::new(line);
    let text = match focused {
        true => text.color_range(2, ..),
        false => text.dim_range(..),
    };
    let text = match marker {
        Some(at) => text.error_color_range(at..at + 1),
        None => text,
    };
    print_text_with_coordinates(text, 0, y, Some(cols), None);
}

/// The row the divider draws, never wider than `cols`.
///
/// Split out from the drawing because it is the one line in the pane that has to fit three
/// things onto a row rather than one, and a string is what a test can measure.
fn divider_line(label: &str, above: usize, below: usize, cols: usize) -> String {
    // Both ends of it, so a half wound back into the past says how far and how much is still
    // ahead of it. No scrollbar: nothing in the tree draws one.
    let position = match (above, below) {
        (0, 0) => String::new(),
        (above, 0) => format!(" ↑{above} "),
        (0, below) => format!(" ↓{below} "),
        (above, below) => format!(" ↑{above} ↓{below} "),
    };
    // The readout shares the row with the label, so it is taken out of the label's room rather
    // than out of the rule: how far back the history is wound is worth more than the last few
    // characters of a kind, and this is the only place that knows both.
    let label = agents::fit_label(label.to_owned(), cols.saturating_sub(position.width()));
    let mut line = match label.is_empty() {
        true => "──".to_owned(),
        false => format!("─ {label} "),
    };
    let rule = cols
        .saturating_sub(line.width())
        .saturating_sub(position.width());
    line.push_str(&"─".repeat(rule));
    line.push_str(&position);
    line
}

/// How the pane divides: `(list_rows, history_rows)`, with the divider on the row between
/// them. `history_rows` is `0` on a pane too short to be split, which is the one case the
/// halves are not permanent - a list squeezed to nothing shows neither.
///
/// Half each, near enough: the divider comes off the list's share, so an odd pane gives the
/// extra row to the half that scrolls rather than to the one that windows itself.
fn split(rows: usize) -> (usize, usize) {
    let history_rows = rows / 2;
    let list_rows = rows.saturating_sub(history_rows + 1);
    // Measured rather than thresholded, so the two minimums stay the thing that decides and
    // there is no second expression of the arithmetic to keep in step with this one.
    if list_rows < MIN_LIST_ROWS || history_rows < MIN_HISTORY_ROWS {
        return (rows, 0);
    }
    (list_rows, history_rows)
}

/// The fewest rows worth giving the list: a row either side of the selection.
const MIN_LIST_ROWS: usize = 3;
/// The fewest rows worth giving the history: a pinned summary line and a line of timeline
/// under it, or a placeholder.
const MIN_HISTORY_ROWS: usize = 2;

impl State {
    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Char('`') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // puts this pane away - the same gesture that revealed it. This pane and
            // nothing else: the conversation beside it, if there is one, is the user's
            // to put away with its own Esc.
            hide_self();
            return false;
        }
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // Move this pane out of the way of whatever it is covering - the same gesture the
            // conversation has, and the same walk: clear of a tiled pane, then round the corners.
            // hocus owns both overlays' geometry, so this asks rather than moves. Nothing to
            // redraw: the move is the server's to paint.
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            return false;
        }
        if key.bare_key == BareKey::Char('r') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // the one part of a row no host event keeps fresh is the branch a
            // checkout is on; ask agentd to probe again
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_REFRESH));
            return false;
        }
        if key.bare_key == BareKey::Tab && self.last_history_rows > 0 {
            // Modifier-agnostic: there are two halves, so shift-tab has nowhere to go that
            // tab does not, and a terminal that spells shift-tab as a modified Tab and one
            // that spells it its own way both land here.
            self.focus = match self.focus {
                Focus::List => Focus::History,
                Focus::History => Focus::List,
            };
            return true;
        }
        // The arrows always walk the list, whichever half has the keys: they are how the
        // pane is steered, and a user reading the history still wants to point at the next
        // agent without going back for the focus first.
        if let Some(motion) = arrow(&key) {
            let focus_moved = self.focus != Focus::List;
            self.focus = Focus::List;
            // The keys moving is worth a redraw on its own - the divider draws them - even
            // when the selection they came back for was already at the end of the list.
            return self.step_selection(motion) || focus_moved;
        }
        if key.has_no_modifiers() {
            match key.bare_key {
                // `j`/`k` follow the focus - the vim spelling of whatever the focused half
                // does with a line.
                BareKey::Char('k') => {
                    return match self.focus {
                        Focus::List => self.step_selection(Motion::Up),
                        Focus::History => self.scroll_history(1),
                    };
                },
                BareKey::Char('j') => {
                    return match self.focus {
                        Focus::List => self.step_selection(Motion::Down),
                        Focus::History => self.scroll_history(-1),
                    };
                },
                // The page keys are the history's whichever half has the focus, for the
                // same reason the arrows are the list's: the list has no page of its own,
                // being windowed around the selection rather than scrolled.
                BareKey::PageUp => return self.scroll_history(self.page() as isize),
                BareKey::PageDown => return self.scroll_history(-(self.page() as isize)),
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

    /// One page of the history half: what it can show, less a line of overlap so a page turn
    /// does not lose the sentence that was straddling the seam.
    fn page(&self) -> usize {
        self.last_history_rows.saturating_sub(1).max(1)
    }

    fn on_mouse(&mut self, mouse_event: Mouse) -> bool {
        match mouse_event {
            Mouse::LeftClick(line, _) if line >= 0 => self.on_click(line as usize),
            // The wheel carries no coordinates, so it cannot be routed by where the pointer
            // is. It is the history's unconditionally rather than the focused half's: the
            // list windows itself around the selection and has no scroll to give it.
            Mouse::ScrollUp(lines) => self.scroll_history(lines.max(1) as isize),
            Mouse::ScrollDown(lines) => self.scroll_history(-(lines.max(1) as isize)),
            _ => false,
        }
    }

    /// A click on screen line `line`.
    ///
    /// The half it lands in takes the keys - the pointer saying where the user is looking is
    /// the whole reason for having two halves rather than one. In the list it also selects the
    /// row it lands on and focuses it the way `Enter` does: clicking an agent row is meant to
    /// feel like navigating to it and pressing Enter, whether or not it was already selected.
    /// Short of the dismissal: see `activate`.
    fn on_click(&mut self, line: usize) -> bool {
        if self.last_history_rows > 0 && line >= self.last_list_rows {
            // The divider and everything under it. Nothing down there is selectable, so the
            // focus is all the click does - and it is worth a redraw, because the divider
            // draws it.
            let moved = self.focus != Focus::History;
            self.focus = Focus::History;
            return moved;
        }
        let focus_moved = self.focus != Focus::List;
        self.focus = Focus::List;
        let Some(index) = click_target(
            &self.rows,
            self.last_window_start,
            self.last_list_rows,
            line,
        ) else {
            return focus_moved;
        };
        let moved = index != self.selected;
        self.selected = index;
        self.activate();
        moved || focus_moved
    }

    /// Walk the selection one row, and say whether it went anywhere.
    fn step_selection(&mut self, motion: Motion) -> bool {
        let next = step(&self.rows, self.selected, motion);
        let moved = next != self.selected;
        self.selected = next;
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

    /// The history half: a divider naming what is below it, then the summary and timeline of
    /// whatever the cursor is on.
    ///
    /// Drawn even when there is nothing to say. The half is permanent so the list's height
    /// never moves under the pointer, and an empty history is a sentence rather than an
    /// absence.
    fn render_history(&mut self, top: usize, rows: usize, cols: usize) {
        let history = self.history_lines(cols);
        // `rows` is the body alone: `split` takes the divider off the list's share, and it is
        // drawn on the row above the body at `top`.
        //
        // The summary is pinned above the timeline, and never takes more than half the half:
        // a talkative model must not leave the timeline it is summarising with no room.
        let body_rows = rows;
        let mut pinned = history.pinned;
        pinned.truncate(body_rows / 2);
        let room = body_rows - pinned.len();
        // `history_scroll` counts backward from the newest entry, which is the end the half is
        // anchored to: an entry arriving pushes the window along rather than leaving the user
        // reading a stale top.
        let max_scroll = history.entries.len().saturating_sub(room);
        self.last_history_max_scroll = max_scroll;
        self.history_scroll = self.history_scroll.min(max_scroll);
        let end = history.entries.len() - self.history_scroll;
        let start = end.saturating_sub(room);

        render_divider(
            &history.label,
            start,
            history.entries.len() - end,
            self.focus == Focus::History,
            top,
            cols,
        );
        let body = top + 1;
        if pinned.is_empty() && history.entries.is_empty() {
            let placeholder = match self.selection_key() {
                Some(_) => "  nothing to show yet",
                None => "  select an agent",
            };
            print_text_with_coordinates(
                Text::new(placeholder).dim_range(..),
                0,
                body,
                Some(cols),
                None,
            );
            return;
        }
        for (index, line) in pinned
            .iter()
            .chain(&history.entries[start..end])
            .enumerate()
        {
            // The margin is part of the string rather than an `x` offset, so a style range
            // covers the row as it reads rather than the text alone. Nothing has to be padded
            // out to the pane's width: the host deletes the viewport before every render, so
            // there is no frame underneath for a short line to leave showing.
            let text = Text::new(format!(" {line}"));
            // The pinned summary is the model's conclusion and the timeline is the record;
            // dimming the record is what keeps the eye on the first.
            let text = match index < pinned.len() {
                true => text,
                false => text.dim_range(..),
            };
            print_text_with_coordinates(text, 0, body + index, Some(cols), None);
        }
    }

    /// What the history half has to draw for the row under the cursor, for a half `width`
    /// columns wide. The margin inside that is `agents`' to take: the body keeps one, the
    /// divider wants the whole row.
    ///
    /// A loaded run first - the whole point of asking hocus for one - then the idle summary
    /// hocus pushes with every strip, then nothing. Empty is a real answer here rather than a
    /// reason not to draw: see `render_history`.
    fn history_lines(&self, width: usize) -> agents::HistoryLines {
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| self.selected_run_matches(run))
        {
            return agents::run_history(run, width);
        }
        let Some(pane_id) = self.selection() else {
            return agents::HistoryLines::default();
        };
        let Some(summary) = self.summaries.get(&pane_id) else {
            return agents::HistoryLines::default();
        };
        let Some(StripRow::Agent { kind, .. }) = self.rows.get(self.selected) else {
            return agents::HistoryLines::default();
        };
        agents::summary_history(pane_id, kind, summary, width)
    }

    /// Move the history half by `delta` lines, positive being backward into the past, and say
    /// whether anything actually moved.
    ///
    /// The one place `history_scroll` changes, so the clamp cannot be forgotten at one of the
    /// call sites that want it. Without it a wheel against the top of the half would keep
    /// counting, and coming back down would cost as many notches as were wasted going up.
    fn scroll_history(&mut self, delta: isize) -> bool {
        let target = match delta >= 0 {
            true => self
                .history_scroll
                .saturating_add(delta.unsigned_abs())
                .min(self.last_history_max_scroll),
            false => self.history_scroll.saturating_sub(delta.unsigned_abs()),
        };
        let moved = target != self.history_scroll;
        self.history_scroll = target;
        moved
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

/// The motion an arrow asks for, if any. Modifier-agnostic, as it always was.
///
/// `j`/`k` are the vim spelling of these and used to be answered here too - this pane is a
/// list and never a text field, so the letters are free to take. They are dispatched in
/// `on_key` now instead, because they follow the focus and the arrows do not: the arrows are
/// how the *list* is steered from either half.
fn arrow(key: &KeyWithModifier) -> Option<Motion> {
    match key.bare_key {
        BareKey::Up => Some(Motion::Up),
        BareKey::Down => Some(Motion::Down),
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
    fn the_arrows_are_a_motion_whatever_is_held_with_them() {
        assert_eq!(
            arrow(&KeyWithModifier::new(BareKey::Down)),
            Some(Motion::Down)
        );
        assert_eq!(arrow(&KeyWithModifier::new(BareKey::Up)), Some(Motion::Up));
        assert_eq!(
            arrow(&KeyWithModifier::new(BareKey::Up).with_shift_modifier()),
            Some(Motion::Up)
        );
        // `j`/`k` are answered in `on_key` now, because they follow the focus.
        assert_eq!(arrow(&KeyWithModifier::new(BareKey::Char('j'))), None);
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

    /// `j`/`k` want the letter on its own, leaving `Ctrl j` and friends to whatever binds
    /// them.
    #[test]
    fn a_modified_or_unrelated_letter_moves_nothing() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        for key in [
            KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('k')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('x')),
        ] {
            assert!(!state.on_key(key));
            assert_eq!(state.selected, 2);
        }
    }

    /// Ctrl-o is hocus's to answer - the list has no geometry of its own - so all this pane does
    /// is ask, and take the key off whatever would have typed it.
    #[test]
    fn ctrl_o_is_taken_and_moves_the_selection_nowhere() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.focus = Focus::History;

        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier()),
            "nothing to redraw here - the move is the server's to paint"
        );

        assert_eq!(state.selected, 2, "the cursor stays where it was");
        assert_eq!(state.focus, Focus::History, "and so does the focus");
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

    // -- the history half ---------------------------------------------------

    /// The pane is two halves and a rule between them, and the list's share does not depend
    /// on what the history happens to have to say - which is the whole reason for the split:
    /// a click has to keep landing on the row it points at.
    #[test]
    fn the_split_is_permanent() {
        let (list_rows, history_rows) = split(20);
        assert_eq!((list_rows, history_rows), (9, 10));
        assert_eq!(list_rows + 1 + history_rows, 20, "the divider is the rest");

        // and it is the same split whether or not there is a history to draw
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert_eq!(split(20).0, list_rows, "no summary, no run");
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        assert_eq!(split(20).0, list_rows, "a summary");
    }

    /// The history's share is body and nothing else: `split` has already taken the divider off
    /// the list's, so taking it off again here is what left the last row of the pane blank.
    ///
    /// Nothing observable is printed by a unit test, so the height is read off the room the
    /// scroll is clamped against - which is the body's, less anything pinned above it.
    #[test]
    fn the_history_body_is_the_whole_of_its_share() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        let mut run = a_run();
        // No summary, so nothing is pinned and the timeline has the whole body to itself.
        run.summary = None;
        run.entries = (0..30)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
            })
            .collect();
        state.run = Some(run);

        let (_, history_rows) = split(20);
        state.render(20, 50);
        assert_eq!(state.last_history_rows, history_rows);
        assert_eq!(
            state.last_history_max_scroll,
            30 - history_rows,
            "every row of the share holds a line of timeline"
        );
        assert_eq!(state.page(), history_rows - 1, "less a line of overlap");
    }

    /// The label, the rule and the scroll readout share the divider's row, and the readout is
    /// the only thing in the pane that says how far back the history is wound. A kind long
    /// enough to fill the row is cut for it rather than pushing it off the end.
    #[test]
    fn a_long_label_does_not_push_the_scroll_readout_off_the_divider() {
        let kind = "a-very-long-agent-kind-indeed-xx";
        let label = format!("7 · {kind} !");
        for cols in [22, 26, 40, 80] {
            let line = divider_line(&label, 12, 34, cols);
            assert!(line.width() <= cols, "{cols}: {line}");
            assert!(line.ends_with(" ↑12 ↓34 "), "{cols}: {line}");
        }

        // and a divider with nothing wound back has no readout to make room for
        assert_eq!(divider_line("", 0, 0, 6), "──────");
    }

    /// A pane with room for one half is the list. Two halves of nothing is worse than one
    /// list, and the focus has nowhere else to be.
    #[test]
    fn a_pane_too_short_to_split_is_all_list() {
        assert_eq!(split(6), (6, 0), "a list of three is not worth halving");
        assert_eq!(split(7), (3, 3), "just enough for both");

        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(5, 40);
        assert_eq!(state.last_history_rows, 0);
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Tab)),
            "no other half to go to"
        );
        assert_eq!(state.focus, Focus::List);
    }

    /// The history is about the row under the cursor, and the divider names it: the pane id
    /// and the kind, which is why `StripRow::Agent` carries the kind at all.
    #[test]
    fn the_history_names_the_selected_agent() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests, all green.", false));
        let history = state.history_lines(38);
        assert_eq!(history.label, "3 · claude");
        assert!(
            history.entries.iter().any(|row| row.contains("all green")),
            "{history:?}"
        );

        // and it follows the cursor off that row
        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        assert!(state.history_lines(38).is_empty(), "pane 7 has no summary");
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
        state.run = Some(a_run());

        let history = state.history_lines(48);
        assert_eq!(history.label, "claude · archived");
        assert!(history
            .pinned
            .iter()
            .any(|row| row.contains("summary: History")));
        assert!(history
            .entries
            .iter()
            .any(|row| row.contains("question: Keep")));
        assert!(!history.entries.iter().any(|row| row.contains("Bash cargo")));
        assert!(!history
            .entries
            .iter()
            .any(|row| row.contains("result: Implemented")));
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Enter)));
    }

    /// The wheel is the history's wherever the pointer is - it carries no coordinates - and
    /// it stops at both ends. Unclamped, a wheel against the top would keep counting and
    /// coming back down would cost as many notches as were wasted going up.
    #[test]
    fn the_wheel_scrolls_the_history_and_stops_at_both_ends() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        let mut run = a_run();
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Ship it?".to_owned(),
        });
        state.run = Some(run);
        // Three rows of history below the divider: one pinned summary line and two of
        // timeline, against the three timeline entries the filter lets through - so exactly
        // one line to wind back over.
        state.render(7, 50);
        assert_eq!(state.last_history_rows, 3);
        assert_eq!(state.last_history_max_scroll, 1);

        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        assert_eq!(state.history_scroll, 1);
        assert!(
            !state.on_mouse(Mouse::ScrollUp(50)),
            "already at the oldest line, and nothing to redraw"
        );
        assert_eq!(state.history_scroll, 1, "the clamp, not the count");
        assert!(state.on_mouse(Mouse::ScrollDown(1)), "one notch back down");
        assert_eq!(state.history_scroll, 0);
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    }

    /// `Tab` moves the keys, `j`/`k` do whatever the half they are in does with a line, and
    /// the arrows walk the list from either half.
    #[test]
    fn tab_moves_the_focus_and_jk_follow_it() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
            StripRow::Run {
                run_id: "claude:session-2".to_owned(),
                text: "claude · another".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        let mut run = a_run();
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Ship it?".to_owned(),
        });
        state.run = Some(run);
        // two lines of timeline against three entries: exactly one line to scroll
        state.render(7, 50);
        assert_eq!(state.selected, 1);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(state.history_scroll, 1, "k scrolls the history it is in");
        assert_eq!(state.selected, 1, "and moves no selection");

        // the arrows steer the list from either half, and take the keys back with them
        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert_eq!(state.selected, 2);
        assert_eq!(state.focus, Focus::List);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(state.selected, 1, "k walks the list it is back in");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::List);

        // An arrow that finds nowhere to go still took the keys back, and the divider draws
        // them: without the redraw it would go on claiming the focus is down there.
        state.on_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(state.selected, 2, "the last row there is");
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.focus, Focus::History);
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Down)),
            "the focus moved even though the selection could not"
        );
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.selected, 2);
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Down)),
            "and now nothing moved at all"
        );
    }

    /// The page keys are the history's from either half: the list has no page of its own,
    /// being windowed around the selection rather than scrolled.
    #[test]
    fn the_page_keys_are_the_historys_from_either_half() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        let mut run = a_run();
        run.entries = (0..40)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
            })
            .collect();
        state.run = Some(run);
        state.render(20, 50);

        assert_eq!(state.focus, Focus::List, "the page keys did not need it");
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        assert_eq!(state.history_scroll, state.page());
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageDown)));
        assert_eq!(state.history_scroll, 0);
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::PageDown)),
            "at the newest line already"
        );
    }

    /// A click lands in the half it points at. In the list that still selects and activates;
    /// below the divider the focus is all it does, because nothing down there is a row.
    #[test]
    fn a_click_lands_in_the_half_it_points_at() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(20, 40);
        let (list_rows, history_rows) = split(20);
        assert_eq!(state.last_list_rows, list_rows);
        assert_eq!(state.last_history_rows, history_rows);

        assert!(state.on_click(list_rows), "the divider takes the focus");
        assert_eq!(state.focus, Focus::History);
        assert_eq!(state.selected, 2, "and selects nothing");
        assert!(!state.on_click(list_rows + 2), "already down here");

        assert!(state.on_click(3), "the list takes it back");
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.selected, 3, "pane 7, the row that was pointed at");

        // and a click on a header inside the list still hits no row, but the half is the
        // half the pointer is in
        state.focus = Focus::History;
        assert!(state.on_click(0), "the Project header");
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.selected, 3, "still pane 7");
    }

    /// The list windows above the divider, so a click maps to the row under the pointer
    /// rather than the one the history is covering.
    #[test]
    fn the_list_windows_above_the_divider() {
        let mut state = State::default();
        // a strip longer than the window, so the rows the history covers are real rows
        let mut rows = strip();
        for pane_id in 20..30 {
            rows.push(agent(pane_id, &format!("  {pane_id}  claude  idle")));
        }
        state.set_rows(with_summary(rows, 3, "Ran the tests, all green.", false));

        let (list_rows, history_rows) = split(12);
        assert_eq!((list_rows, history_rows), (5, 6));
        assert!(
            state.rows.len() > list_rows,
            "and the strip outruns what is left"
        );
        assert_eq!(
            click_target(&state.rows, 0, list_rows, list_rows),
            None,
            "the divider is not a row"
        );
        assert_eq!(
            click_target(&state.rows, 1, list_rows, list_rows - 1),
            Some(5),
            "the last row of the list is still clickable"
        );
    }

    /// `render_divider` colours the `!` in a label by *searching* the line for the character
    /// — the label is cut to fit, so its position cannot be counted. That makes every glyph
    /// the label can carry a hazard: one containing a `!` would light up the wrong column.
    /// The hook marker is the newest of them, so it is worth pinning here rather than
    /// trusting that nobody ever picks an exclamation mark for one.
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
        let label = state.history_lines(38).label;
        // The one `!` in the label is the waiting marker, and it is the last of it rather
        // than anywhere inside the kind.
        assert!(label.contains("3 · claude◆"), "{label}");
        assert_eq!(label.matches('!').count(), 1, "{label}");
        assert!(
            label.find('!').unwrap() > label.find('◆').unwrap(),
            "{label}"
        );
    }

    /// Better nothing than a column of single words: a half too narrow to wrap into says so
    /// instead, and the divider is still drawn.
    #[test]
    fn a_half_too_narrow_to_wrap_into_draws_nothing() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        assert!(state.history_lines(12).is_empty());
        assert!(!state.history_lines(38).is_empty());
    }

    /// One `AgentRun`, as hocus pipes it over for the selected row.
    fn a_run() -> agents::AgentRun {
        agents::AgentRun {
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
        }
    }
}
