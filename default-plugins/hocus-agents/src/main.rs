//! hocus-agents — the agent list, an overlay pane of its own.
//!
//! This used to be the top third of the hocus pane, drawn by hocus itself with
//! an `agents_focused` flag deciding who owned the keys. Then it was the upper
//! half of a two-pane column. It is now its own pane: the left half of one
//! panel whose right half is the conversation, the two butted together with a
//! divider between them that the mouse can drag. It opens, hides and is read on
//! its own; only its geometry is shared, and that is hocus's to compute.
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
//! **The two halves are the front of a ring that goes on into the conversation.** `Tab` and
//! `Shift+Tab` walk four stops, not two: this pane's list and history halves, then the
//! conversation's tab bar and its input box, wrapping back round. The two crossings that leave
//! this pane are requests - `wire::AGENT_STRIP_YIELD`, the same shape as the Esc and Ctrl-o
//! ones, because hocus holds this pane's plugin id and this pane holds none of hocus's - and
//! the two that arrive land through `wire::AGENT_STRIP_TAKE`, which names the half but does not
//! move the keyboard; hocus has already done that. The history half is a stop only when it
//! exists: an empty list, or a pane too short to split, has no second half, and forward from
//! the list goes straight on to the conversation.
//!
//! **A summary is one more event.** The run's own is the timeline's newest row and the
//! ones somebody asked for sit where they were asked for; nothing is held above the timeline,
//! so the whole half is one scrolling window and a talkative model costs the events beside it
//! nothing they cannot be scrolled back to. A `summary:` label is bold where the other labels
//! are dim, which is what tells the row apart now that its position no longer does.
//!
//! **An event gets four rows of the timeline and no more.** A talkative `result:` used to fill
//! the half on its own and push whatever happened either side of it out of view, which is the
//! opposite of what a timeline is for; the fourth row of a long one now says how many were left
//! out (`agents::MAX_ENTRY_ROWS`). That is only honest because the rest is a keystroke away:
//! with the keys in this half, `j`/`k` walk an event at a time rather than a line at a time, and
//! `Enter` gives the event the whole pane and a scroll of its own until `Esc`. The wheel and the
//! page keys still move by lines, which is what reading in place wants. The idle summary hocus
//! pushes is not capped — there is one of it and nothing beside it competing for the room.
//!
//! **The history is drawn as markdown**, by the same renderer the conversation uses
//! (`hocus_proto::markdown`) — everything in the half is markdown by the time it arrives, a
//! prompt being what the user typed and a result a model's last message, so the asterisks were
//! never what any of it meant. Compacted: the half has eight rows on a normal pane and cannot
//! spend one on a paragraph break. Parsing it is much the most expensive thing this pane does,
//! so `cached_history_lines` does it once per thing it depends on rather than once a frame.
//!
//! Which pane the user is *in* comes over the same wire, and the cursor follows
//! it: walking into an agent by any route — the pane keys, a tab switch, a tool
//! — selects its row here, so the list shows where you are and not only where
//! you last pointed. Walking onto anything else leaves the row selected and
//! draws it dim, which is the difference between "you are here" and "here is
//! where you were".
//!
//! **`s` asks for a summary of the row under the cursor.** It is the only summary anybody
//! asks for: the daemon buys exactly one of its own accord, when a run closes, and reads no
//! screens at all. So this key is what describes an agent that is still working, and the
//! answer arrives as an entry on the timeline, at the point it was asked for. It works on an
//! archived run too, which has no pane to read and is summarised from what it recorded.
//! This pane sends the row and
//! nothing else (`wire::AGENT_SUMMARIZE`): which of the two kinds of work that means is the
//! daemon's to know, the same way the rows themselves are hocus's to compute.
//!
//! **`r` resumes the archived run under the cursor**, behind a `y`/`n` on the divider. A run
//! in the archive is named for the agent's own session id — `AgentRun.id` is
//! `"{kind}:{session id}"` — which is exactly what `claude --resume` and `codex resume` take,
//! so the one thing missing was a key. It asks first because it is the only key here that
//! starts a process, and it sits next to the ones the list is walked with. Archived rows
//! only: a live agent already has a pane, and a second process on its session id would be two
//! agents writing one conversation. Like everything else in this pane it names the row and no
//! more (`wire::AGENT_RESUME`) — which agent, which folder and which machine are hocus's to
//! know. Unlike everything else it is told when it is refused
//! (`wire::AGENT_RESUME_RESULT`), because an answered confirmation that does nothing at all
//! is worse than never having asked. A resumed agent keeps its session id and its hooks, so
//! the daemon adopts the same run back: the row leaves the archive rather than a second one
//! appearing beside it.
//!
//! **The two panes are separate plugins with one visibility.** The panel is one
//! thing to the user, so Esc here puts the whole of it away: this pane hides
//! itself and hocus is asked for the other half
//! (`wire::AGENT_STRIP_HIDE`), being the only one that knows both plugin ids.
//! `Ctrl e a` brings the whole panel back with the keyboard here, `Ctrl e c`
//! with the keyboard in the conversation; neither ever leaves one half alone.
//! Ctrl-o moves the panel - both halves, split intact - clear of a tiled pane;
//! hocus does the arithmetic, since it places both overlays, so that key is a
//! request (`wire::AGENT_STRIP_REPOSITION`) rather than a move. Enter walks you
//! to the selected agent and leaves the panel exactly where it is: the history
//! half is worth reading beside the pane it is about, and dismissing is what
//! Esc is for. `say_hello_to_hocus` carries hocus's url, because this pane is a
//! view over rows only hocus can compute and a list with no hocus to ask has
//! nothing to draw; the hocus it launches appears beside us but does **not**
//! take the focus - `wire::OPENED_BY` is what tells it so - because the user
//! asked for a list.

use std::collections::BTreeMap;

use hocus_proto::agents::{self, now_ms, Strip, StripRow, Summaries};
use hocus_proto::launch;
use hocus_proto::markdown;
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
    /// Seconds east of UTC where `run` began, for the absolute start time in the history
    /// header. Asked of the host once per run, when the run arrives, rather than in `render`:
    /// a host round trip in the draw path would be paid on every keystroke, and `history_lines`
    /// is what the tests drive directly.
    ///
    /// `0` - UTC - where there is no run, or where the host could not say. A wrong-by-hours
    /// timestamp would be worse than one that admits which zone it is in, which is why
    /// `wall_clock` always names the offset it used.
    run_offset_secs: i32,
    /// Lines the history half is wound back from its newest entry.
    history_scroll: usize,
    /// Which event the history half's cursor is on, counted back from the newest - `0` is the
    /// newest event, the end the half is anchored to.
    ///
    /// Counted from the newest for the same reason `history_scroll` is: an entry arriving must
    /// not drag the cursor off what the user was pointing at. Only drawn while
    /// `focus == Focus::History` - the half has no cursor when the keys are not in it, the way
    /// the list's own selection is a band whether or not it has them.
    history_cursor: usize,
    /// The event being read in full, if any - the whole pane becomes it.
    ///
    /// Holds what the event *said* rather than a pointer into the timeline: the run behind it
    /// goes on growing while it is open, and a reader that re-resolved its event every frame
    /// would be reading a moving target - the entry under a given index is not the entry that
    /// was there when Enter was pressed.
    reading: Option<Reading>,
    /// The last wrapped body of `reading`, and the `(id, width)` it was wrapped for.
    ///
    /// The same bargain `history_cache` strikes, for the same reason: the reader redraws on
    /// every keystroke and wrapping it is a markdown parse of the whole event, which is much
    /// the most expensive thing this pane does.
    reader_cache: Option<((u64, usize), Vec<markdown::MdLine>)>,
    /// The largest `Reading::scroll` the last reader render could honour - the counterpart of
    /// `last_history_max_scroll`, and kept for the same reason: only `render` knows the width
    /// the body was wrapped to, and so only `render` knows how many lines it came to.
    last_reader_max_scroll: usize,
    /// How many rows the last reader render had for its body.
    last_reader_rows: usize,
    /// Bumped on every open, and what `reader_cache` is keyed on. A counter rather than the
    /// event's text, so two openings of the same event are still two readers and a very long
    /// event is not compared character by character on the way to every frame.
    reading_id: u64,
    /// Which half the keys work. Not reset by anything hocus pushes: a strip arriving while
    /// the user is reading the history must not throw them back into the list.
    focus: Focus,
    /// Every ring step that left this pane, as the stop the conversation was sent to.
    ///
    /// Test-only: `pipe_message_to_plugin` is a no-op off wasm, so nothing else can tell a
    /// yield that was sent from one this pane declined to make.
    #[cfg(test)]
    ring_yields: Vec<&'static str>,
    /// Every run this pane has asked hocus to resume, in order.
    ///
    /// Test-only, for `ring_yields`' reason: off wasm the message goes nowhere, and a
    /// confirmation that starts nothing looks exactly like one that was declined.
    #[cfg(test)]
    resume_requests: Vec<String>,
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
    /// How many of those rows the timeline actually scrolls in, which is the rest of them once
    /// the pinned header has taken its share. What a page turn is worth - `last_history_rows`
    /// still answers the two questions it always did, whether there is a history half at all
    /// and where a click lands.
    last_history_body_rows: usize,
    /// The largest `history_scroll` the last render could actually honour.
    ///
    /// Kept for the same reason `last_window_start` is: how many lines the history has
    /// depends on the width it is wrapped to, and only `render` is told the width. Without
    /// it a wheel against the top of the history would keep counting, and scrolling back down
    /// would cost as many notches as were wasted going up.
    last_history_max_scroll: usize,
    /// Bumped whenever anything the history half is drawn *from* changes - the loaded run, or
    /// a strip carrying fresh summaries. Counted rather than compared field by field: what
    /// `history_lines` reads is spread across four of them, and a counter cannot be half
    /// updated by someone adding a fifth.
    generation: u64,
    /// The last `history_lines`, and what it was computed from. See `cached_history_lines`.
    history_cache: Option<(HistoryKey, agents::HistoryLines)>,
    /// The row an `s` has asked the daemon about and not yet had an answer for, as a
    /// `selection_key`.
    ///
    /// A hint on the divider rather than state anything is decided from. The round trip is a
    /// model's, so it is seconds long, and a key that reaches the network and shows nothing in
    /// the meantime reads as a key that did nothing. Held as the key rather than a flag so
    /// that walking off the row takes it with you: nothing here ever hears that a request
    /// failed - a refusal in the daemon is silent by design - so the only honest lifetime is
    /// one the user can end themselves.
    summarizing: Option<String>,
    /// The row an `r` has armed a resume confirmation on, as a `selection_key`.
    ///
    /// A confirmation because this is the one key here that starts a process. Everything
    /// else the pane does is a move, a fetch or a redraw, all of them free to get wrong;
    /// `r` next to `j` and `k` is a keystroke away from the keys the list is walked with.
    ///
    /// Held as the key rather than a flag for `summarizing`'s reason and one more: an armed
    /// confirmation names the row it was armed on, so a strip that arrives and moves the
    /// selection cannot leave a `y` pointing at a run the user never looked at.
    confirming_resume: Option<String>,
    /// Why the last resume was refused, and which row it was about.
    ///
    /// Unlike `summarizing`, this one *is* told when the request fails
    /// (`wire::AGENT_RESUME_RESULT`): the user answered a confirmation, and an answered
    /// confirmation that silently does nothing is worse than no confirmation at all. It
    /// still ends when the cursor walks away, because it is an account of a row.
    resume_note: Option<(String, String)>,
}

/// Everything [`State::history_lines`] reads, as one comparable value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryKey {
    /// The row under the cursor, which is what the half is about.
    selection: Option<String>,
    /// The half's width, which is what its lines are wrapped to.
    width: usize,
    /// [`State::generation`] - the state behind the selection.
    generation: u64,
    /// The minute the cache was filled in, `now_ms() / 60_000`. Nothing else in the key
    /// changes with the passage of time alone, so without this the "ago" label on each
    /// history row would freeze at whatever it read on the last selection change - this
    /// bounds the staleness to about a minute on any render that already happens to occur.
    minute: i64,
}

/// One event of the timeline, filling the pane so it can be read whole.
///
/// The history half caps every event at [`agents::MAX_ENTRY_ROWS`] rows, which is only honest
/// because this exists: `Enter` on the cursor opens the event here, with the whole pane and a
/// scroll of its own to spend on it.
struct Reading {
    /// What the divider calls it - `prompt`, `result`, `summary`.
    kind: String,
    /// When the event was recorded, or `None` for one made before there were stamps. Kept
    /// rather than the formatted age so the divider goes on ticking with the minute timer.
    created_at_ms: Option<i64>,
    /// The whole of what the event said, unwrapped.
    text: String,
    /// Lines scrolled from the top. The reader is anchored to the head rather than the tail:
    /// the history half is a timeline, where the newest end is the interesting one, and an
    /// event is a piece of prose, where the beginning is.
    scroll: usize,
    /// Which opening this is. See `State::reading_id`.
    id: u64,
}

/// Which half of the pane the keys work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    List,
    History,
}

/// How often the pane wakes to redraw its ages, in seconds.
///
/// A minute, because that is the finest bucket `agents::short_age` has: a faster tick would
/// redraw for a number that cannot have changed, and a slower one would leave the column
/// reading a minute behind.
const AGE_TICK_SECS: f64 = 60.0;

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::PermissionRequestResult,
            EventType::Timer,
        ]);
        set_timeout(AGE_TICK_SECS);
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
            // The ages in the history half are drawn from the clock, so nothing the pane
            // receives can tell it they have moved on. Re-armed whatever the answer, so the
            // column keeps ticking; a redraw only where an age is actually on screen, so a
            // pane showing nothing time-dependent still costs nothing to leave open.
            Event::Timer(_) => {
                set_timeout(AGE_TICK_SECS);
                self.shows_an_age()
            },
            _ => false,
        };
        // Where the cursor moved to is hocus's business, not only this pane's: it
        // draws the selected agent's summary. Compared rather than sent on every
        // key, so holding `j` at the end of the list says nothing.
        if self.follow_selection(selection) {
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
        if pipe_message.name == wire::AGENT_STRIP_TAKE {
            // The ring stepped into this pane from the conversation. hocus holds our plugin id
            // and has already moved the keyboard; all this carries is which of our two halves
            // the arriving keys belong to.
            return self
                .take_ring_stop(pipe_message.args.get(wire::PANEL_STOP).map(String::as_str));
        }
        if pipe_message.name == wire::AGENT_RESUME_RESULT {
            let Some(result) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::ResumeResult>(json).ok())
            else {
                return false;
            };
            // Named by the run rather than by the cursor, and dropped when the two disagree:
            // the answer is about the row that was asked about, and a user who walked on
            // while it was out has ended the question themselves - the same lifetime every
            // other hint on this divider has. Keeping it would mean a stale refusal waiting
            // to reappear the next time the cursor came back to the row.
            let row = format!("run:{}", result.run_id);
            if self.selection_key() != Some(row.clone()) {
                return false;
            }
            // Empty detail is success. The pane has the focus by now and the next strip
            // moves the row out of the archive, so there is nothing left for the divider to
            // say - only a note from an earlier attempt to take down.
            let note = (!result.detail.is_empty()).then_some((row, result.detail));
            let changed = self.resume_note != note;
            self.resume_note = note;
            return changed;
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
            if changed {
                // Once per run, here rather than in the draw path. The offset is asked for
                // the instant the run began, not for now, so a run from before a
                // daylight-saving change is read back with the offset it started under.
                self.run_offset_secs = get_local_utc_offset(run.started_at_ms).unwrap_or(0);
            }
            self.run = Some(run);
            if changed {
                self.generation += 1;
                // A run that changed is the only answer an `s` ever gets: the summary it
                // asked for arrives inside one of these, as an entry on the timeline. Cleared
                // on any change rather than on one that demonstrably carries a summary,
                // because a run that grew a prompt while the request was out has moved on
                // from the state the hint was describing either way.
                self.summarizing = None;
            }
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
        if self.follow_selection(selection) {
            self.send_selection();
        }
        rendered
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // The reader is the whole pane rather than a third region: an event long enough to
        // need it is long enough to want the room, and the list it covers is still exactly
        // where Esc puts it back.
        if self.reading.is_some() {
            self.render_reader(rows, cols);
            return;
        }
        if self.rows.is_empty() {
            // Nothing to select and so nothing to have a history: the pane says so once
            // rather than splitting an empty list off an empty history.
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_body_rows = 0;
            // No history half drawn, so the focus cannot be in one - the same correction the
            // too-short pane makes below, and what the Tab handler's ring relies on.
            self.focus = Focus::List;
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
            self.last_history_body_rows = 0;
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

/// What the divider has to say about the row under the cursor beyond naming it, and how much
/// of the row it is worth.
///
/// The divider is one line, `fit_label` cuts it from the tail, and an agents pane is narrow -
/// so what shares the row with the label and what takes the whole of it is a decision, not a
/// detail. A hint about something still happening rides `Beside` the label; a refusal, which
/// is longer than either and is the only one the user has not already seen, takes the row
/// `Instead`.
enum Hint {
    Beside(String),
    Instead(String),
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
/// The fewest rows worth giving the history: a line of timeline and one to scroll to, or a
/// placeholder.
const MIN_HISTORY_ROWS: usize = 2;

impl State {
    /// Whether the cursor has left the row `was` named, dropping everything that belonged to
    /// it if so.
    ///
    /// Split from the `send_selection` that follows it at both call sites for the reason given
    /// there: the host calls live in the entry points, and the tests drive everything below
    /// them directly.
    fn follow_selection(&mut self, was: Option<String>) -> bool {
        if self.selection_key() == was {
            return false;
        }
        self.leave_row();
        true
    }

    /// Everything about the history half that belonged to the row the cursor has just left.
    ///
    /// One place rather than three, because the list of what a row owns has grown: where the
    /// half was wound to, which event the cursor was on, whether a summary was asked for, and
    /// the newest of them: whichever event was open in the reader, which is about an entry of a
    /// run the cursor is no longer pointing at.
    ///
    /// Nothing ever tells this pane that a summary request was refused, so the only honest
    /// lifetime for that hint is one the user can end themselves by walking off the row.
    fn leave_row(&mut self) {
        self.history_scroll = 0;
        self.history_cursor = 0;
        self.summarizing = None;
        self.confirming_resume = None;
        self.resume_note = None;
        self.close_reader();
    }

    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        // The reader is the whole pane, so it answers every key: there is no list to steer and
        // no half to move the focus between while it is up, and a key that fell through to them
        // would act on a pane the user cannot see. That includes Esc, which closes the reader
        // rather than dismissing the panel - one step back before the way out.
        if self.reading.is_some() {
            return self.read_key(key);
        }
        // An armed confirmation answers the next key, whichever it is. Not a modal - the
        // list is still drawn and still says which row is being asked about - but the
        // question is answered before anything else happens, so a `j` typed at it cannot
        // both cancel and move in a way that leaves the user unsure which they did.
        if self.confirming_resume.is_some() {
            return self.answer_resume(key);
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
        if key.bare_key == BareKey::Tab {
            // The panel is one ring of four stops - this pane's two halves, then the
            // conversation's tab bar and its input box - and Tab walks it end to end. Two of
            // those crossings leave this pane, and the list knows only its own plugin id, so
            // they are asked for: `wire::AGENT_STRIP_YIELD`, the same shape as the Esc and
            // Ctrl-o requests below and above.
            //
            // Directional now, where this used to be modifier-agnostic: with two stops
            // shift-tab had nowhere to go that tab did not, and with four it does. A terminal
            // that cannot report shift-tab, or spells it a way that does not reach us, lands in
            // the forward arm - which costs nothing, because the ring wraps: every stop is
            // reachable by pressing Tab alone.
            //
            // The history half is a stop only when there is one. `last_history_rows` is zero
            // for an empty list and for a pane too short to split, and `render` forces the
            // focus back to `List` in both - so the stop does not merely look empty, it is not
            // there, and forward from the list goes straight on to the conversation. That also
            // makes the vanishing-stop case self-healing: a pane resized short while the keys
            // were in the history half puts them back in the list on the next frame.
            let back = key.has_modifiers(&[KeyModifier::Shift]);
            match (self.focus, back) {
                (Focus::List, false) if self.last_history_rows > 0 => {
                    self.focus = Focus::History;
                    return true;
                },
                (Focus::List, false) => {
                    self.yield_to_chat(wire::STOP_TABS);
                    return false;
                },
                (Focus::History, true) => {
                    self.focus = Focus::List;
                    return true;
                },
                // Off the end of this pane, either way. Nothing to redraw: the keyboard is
                // going to the other half, and hocus paints that.
                (Focus::History, false) => {
                    self.yield_to_chat(wire::STOP_TABS);
                    return false;
                },
                (Focus::List, true) => {
                    self.yield_to_chat(wire::STOP_INPUT);
                    return false;
                },
            }
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
                //
                // In the history half that is an event rather than a line: the half caps every
                // event at four rows, so stepping one at a time is what walks the timeline, and
                // a line step would leave the cursor pointing between two of them with nothing
                // for `Enter` to open. The wheel and the page keys still move by lines, which
                // is what reading a long event in place wants.
                BareKey::Char('k') => {
                    return match self.focus {
                        Focus::List => self.step_selection(Motion::Up),
                        Focus::History => self.step_history_cursor(1),
                    };
                },
                BareKey::Char('j') => {
                    return match self.focus {
                        Focus::List => self.step_selection(Motion::Down),
                        Focus::History => self.step_history_cursor(-1),
                    };
                },
                // The page keys are the history's whichever half has the focus, for the
                // same reason the arrows are the list's: the list has no page of its own,
                // being windowed around the selection rather than scrolled.
                // Ask the daemon to describe the selected row now. Not gated on the focus:
                // it is about the row the cursor is on, which is the same row whichever
                // half the keys are in, and the answer arrives in the half below either
                // way.
                BareKey::Char('s') => return self.request_summary(),
                // Ask before starting a process. See `confirming_resume`.
                BareKey::Char('r') => return self.arm_resume(),
                BareKey::PageUp => return self.scroll_history(self.page() as isize),
                BareKey::PageDown => return self.scroll_history(-(self.page() as isize)),
                _ => {},
            }
        }
        match key.bare_key {
            BareKey::Esc => {
                // Put the whole panel away, not this pane: the list and the conversation are one
                // thing to the user and have one visibility. This half goes on its own - it has
                // its own id and no round trip is needed for it - and hocus is asked for the
                // other, being the only half that knows both ids. Sent even when there may be no
                // hocus listening: the message launches nobody, so it is free.
                hide_self();
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HIDE));
                false
            },
            BareKey::Enter => {
                // With the keys in the history half, Enter is about the event under the
                // cursor rather than the row above: the half caps every event at four rows,
                // and this is where the rest of one is. The list half's Enter is untouched -
                // the two halves have separate cursors, and this is the key that says so.
                if self.focus == Focus::History {
                    return self.open_reader();
                }
                if matches!(self.rows.get(self.selected), Some(StripRow::MoreRuns)) {
                    pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_ARCHIVE_MORE));
                    return false;
                }
                if !matches!(self.rows.get(self.selected), Some(StripRow::Agent { .. })) {
                    return false;
                }
                // Exactly what a click on the row does, and for the same reason it
                // leaves the panel up: the history half under the list is about the
                // agent you just walked into, so putting the panel away is the one
                // thing Enter must not do. Esc is the dismissal.
                self.activate();
                false
            },
            _ => false,
        }
    }

    /// One page of the history half: what it can show, less a line of overlap so a page turn
    /// does not lose the sentence that was straddling the seam.
    fn page(&self) -> usize {
        self.last_history_body_rows.saturating_sub(1).max(1)
    }

    fn on_mouse(&mut self, mouse_event: Mouse) -> bool {
        // With the reader up there is one thing on screen and the wheel is its. A click is
        // ignored rather than closing it: nothing under the pointer is a target, and a stray
        // click losing the user's place in a long event is worse than a click that does
        // nothing.
        if self.reading.is_some() {
            return match mouse_event {
                Mouse::ScrollUp(lines) => self.scroll_reader(-(lines.max(1) as isize)),
                Mouse::ScrollDown(lines) => self.scroll_reader(lines.max(1) as isize),
                _ => false,
            };
        }
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

    /// The whole of what `Enter` and a click on a list row do: focus the selected
    /// agent's pane, if there is one. Neither of them puts the panel away - the
    /// pointer is already where the next click would go, and the keys are already
    /// where the next Tab into the history would want them.
    /// Ask the daemon to describe the selected row now, and say whether the divider changed.
    ///
    /// Both kinds of row are worth asking about, and the daemon answers them differently: a
    /// live agent has a pane to read and a run the daemon is already holding, an archived one
    /// has neither and has to be fetched from the store. This pane knows nothing about that
    /// difference and should not - it names the row and lets the daemon choose the path,
    /// which is the same division of labour that has hocus computing the rows drawn here.
    ///
    /// Nothing is expected back on this message. The answer arrives the way every other
    /// change to a run does, as a fresh `AGENT_RUN_DETAIL`, so a summary asked for here and
    /// one that happened on its own reach the half by one road.
    fn request_summary(&mut self) -> bool {
        let request = match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => wire::SummarizeRequest {
                pane_id: Some(*pane_id),
                run_id: None,
            },
            Some(StripRow::Run { run_id, .. }) => wire::SummarizeRequest {
                pane_id: None,
                run_id: Some(run_id.clone()),
            },
            // A project or branch header, or `load older runs…`. Nothing there is an agent.
            _ => return false,
        };
        let Ok(payload) = serde_json::to_string(&request) else {
            return false;
        };
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUMMARIZE).with_payload(payload));
        let asked = self.selection_key();
        // A refusal outranks `summarising…` on the divider and lives until the cursor leaves
        // the row, so leaving it up here would make `s` on a row that has just refused an `r`
        // look like a key that does nothing. The note is an account of a question that has
        // been overtaken - `arm_resume` takes it down for the same reason.
        let changed = self.summarizing != asked || self.resume_note.is_some();
        self.resume_note = None;
        self.summarizing = asked;
        changed
    }

    /// Ask whether to resume the archived run under the cursor, and say whether the divider
    /// changed.
    ///
    /// Archived rows only. A live agent already has its pane - `Enter` walks to it - and
    /// handing its session id to a second process would put two agents on one conversation,
    /// each unaware of what the other wrote. So `r` is inert on every other kind of row, the
    /// same way `Enter` is on a project header.
    fn arm_resume(&mut self) -> bool {
        if !matches!(self.rows.get(self.selected), Some(StripRow::Run { .. })) {
            return false;
        }
        // A previous refusal is about a question that has just been asked again.
        self.resume_note = None;
        self.confirming_resume = self.selection_key();
        self.confirming_resume.is_some()
    }

    /// Take the answer to an armed resume confirmation.
    ///
    /// `y` starts it; `n` and `Esc` drop it and stop there - `Esc` deliberately does *not*
    /// fall through to hiding the panel, which is the reader's rule too: one step back before
    /// the way out. Anything else drops the question and then means what it always means, so
    /// a user who typed `r` by mistake and carried on walking the list is not made to
    /// acknowledge it first.
    fn answer_resume(&mut self, key: KeyWithModifier) -> bool {
        let armed = self.confirming_resume.take();
        if key.has_no_modifiers() && key.bare_key == BareKey::Char('y') {
            // Against the selection rather than blindly: a strip that arrived between the
            // question and the answer can have moved the cursor, and a `y` must never start
            // a run the user was not looking at when they were asked.
            if armed.is_some() && armed == self.selection_key() {
                self.request_resume();
            }
            return true;
        }
        if key.bare_key == BareKey::Esc
            || (key.has_no_modifiers() && key.bare_key == BareKey::Char('n'))
        {
            return true;
        }
        // Cleared above, so this cannot come back here. The answer is always a redraw,
        // whatever the key itself decides: the question has come off the divider.
        self.on_key(key);
        true
    }

    /// Name the archived run under the cursor to hocus, which knows what resuming one means.
    ///
    /// Nothing is expected back except `wire::AGENT_RESUME_RESULT`, which is only ever read
    /// for its refusal: a resume that worked shows up as a pane taking the focus, and then as
    /// the row leaving the archive on the next strip.
    fn request_resume(&mut self) {
        let Some(StripRow::Run { run_id, .. }) = self.rows.get(self.selected) else {
            return;
        };
        let request = wire::ResumeRequest {
            run_id: run_id.clone(),
        };
        let Ok(payload) = serde_json::to_string(&request) else {
            return;
        };
        #[cfg(test)]
        self.resume_requests.push(request.run_id.clone());
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_RESUME).with_payload(payload));
    }

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
        // The rows carry the kind the divider names and the summaries carry the text below it,
        // so a push worth a redraw is a push the history cache cannot survive. Tied to
        // `changed` rather than to the summaries alone: the once-a-second push that moves
        // nothing is the one that must not throw the cache away.
        if changed {
            self.generation += 1;
        }
        self.rows = rows;
        self.selected = selected;
        self.summaries = summaries;
        self.focused = focused;
        changed
    }

    /// How many of the half's `rows` the pinned header takes - its lines plus the rule under
    /// them - or `0` where it is not drawn.
    ///
    /// Dropped whole rather than squeezed on a half with no room for both it and a timeline
    /// worth the name: the header says two things about the run and the timeline says what the
    /// run did, and where only one of them fits the second is worth more. What "worth the
    /// name" means is [`MIN_HISTORY_ROWS`] - the same number `split` uses to decide whether to
    /// make a history half at all, rather than a second opinion about the same question.
    fn header_rows(&self, rows: usize) -> usize {
        let wanted = match self.history().header.len() {
            0 => return 0,
            lines => lines + 1,
        };
        match rows >= wanted + MIN_HISTORY_ROWS {
            true => wanted,
            false => 0,
        }
    }

    /// The history half: a divider naming what is below it, the run's header, then the timeline
    /// of whatever the cursor is on - summaries among the events rather than above them.
    ///
    /// Drawn even when there is nothing to say. The half is permanent so the list's height
    /// never moves under the pointer, and an empty history is a sentence rather than an
    /// absence.
    fn render_history(&mut self, top: usize, rows: usize, cols: usize) {
        self.fill_history_cache(cols);
        // `rows` is the body alone: `split` takes the divider off the list's share, and it is
        // drawn on the row above the body at `top`.
        //
        // Only two things are held above the timeline, and both are about the run rather than
        // about anything it said: when it began, and its whole session id. A summary is *not*
        // one of them - it is a row of the timeline, where in time it was made - so the events
        // remain one scrolling window and a talkative model costs the events beside it nothing
        // they cannot be scrolled back to.
        let header_rows = self.header_rows(rows);
        let room = rows.saturating_sub(header_rows);
        let entries_len = self.history().entries.len();
        // `history_scroll` counts backward from the newest entry, which is the end the half is
        // anchored to: an entry arriving pushes the window along rather than leaving the user
        // reading a stale top.
        let max_scroll = entries_len.saturating_sub(room);
        self.last_history_max_scroll = max_scroll;
        self.last_history_body_rows = room;
        self.history_scroll = self.history_scroll.min(max_scroll);
        let focused = self.focus == Focus::History;
        // The cursor is walked by `step_history_cursor`, which knows how many events there are
        // but not how many of their rows fit: how many lines the half has depends on the width,
        // and only `render` is told the width. So the winding that keeps the cursor on screen
        // happens here, the same place and for the same reason `last_history_max_scroll` is
        // recorded.
        if focused {
            self.history_cursor = self
                .history_cursor
                .min(self.history().events.len().saturating_sub(1));
            if let Some(reveal) = self.reveal_cursor(room) {
                self.history_scroll = reveal.min(max_scroll);
            }
        }
        let end = entries_len - self.history_scroll;
        let start = end.saturating_sub(room);
        let cursor = focused
            .then(|| self.cursor_event().map(|event| event.start))
            .flatten();
        let placeholder = match self.selection_key() {
            Some(_) => "  nothing to show yet",
            None => "  select an agent",
        };
        // Everything the half is drawn from is settled now, so the lines can be borrowed
        // rather than copied.
        // Built before `history` is borrowed below, because it borrows `self` too.
        let label = self.history_label();
        let history = self.history();

        render_divider(&label, start, entries_len - end, focused, top, cols);
        let mut body = top + 1;
        if header_rows > 0 {
            for (index, line) in history.header.iter().enumerate() {
                // The same margin every row of the timeline takes, and taken the same way -
                // part of the string, so the styles shift along with it. The column it leaves
                // is the cursor's, and nothing in the header can hold a cursor.
                let line = line.clone().indent(1);
                let text = markdown::style_text(&line.text, &line.styles, false);
                print_text_with_coordinates(text, 0, body + index, Some(cols), None);
            }
            // A plain rule, never coloured: `render_divider`'s colour is what says where the
            // keyboard is, and there is one focusable rule per half. This one only separates
            // what is fixed from what scrolls.
            print_text_with_coordinates(
                Text::new(divider_line("", 0, 0, cols)).dim_range(..),
                0,
                body + history.header.len(),
                Some(cols),
                None,
            );
            body += header_rows;
        }
        if history.entries.is_empty() {
            print_text_with_coordinates(
                Text::new(placeholder).dim_range(..),
                0,
                body,
                Some(cols),
                None,
            );
            return;
        }
        for (index, line) in history.entries[start..end].iter().enumerate() {
            // The margin is part of the string rather than an `x` offset, so a style range
            // covers the row as it reads rather than the text alone - which is why the styles
            // are shifted along with it. Nothing has to be padded out to the pane's width: the
            // host deletes the viewport before every render, so there is no frame underneath
            // for a short line to leave showing. Only the rows on their way to the screen are
            // copied: the cache behind them holds every line of the run.
            //
            // The cursor takes that margin on the event's first row, which is the row its
            // label is on: a marker in the column every other row leaves blank, the same
            // shape the list above uses to say where its own cursor is.
            let line = match cursor == Some(start + index) {
                true => line.clone().prepend(">", markdown::MdStyle::default()),
                false => line.clone().indent(1),
            };
            // Nothing is dimmed by where it sits: every event reads at the same weight, and a
            // summary is told apart by its bold label rather than by the rest of the timeline
            // being pushed into the background. Whatever the markdown asked for still stands.
            let text = markdown::style_text(&line.text, &line.styles, false);
            print_text_with_coordinates(text, 0, body + index, Some(cols), None);
        }
    }

    /// Where the history half has to be wound for the cursor's event to be on screen, or `None`
    /// when it already is.
    ///
    /// In `history_scroll`'s units - lines back from the newest - and unclamped: the caller
    /// holds the `max_scroll` this has to fit inside.
    ///
    /// The two edges are answered in order, foot then head, so an event taller than the room
    /// shows its head: it is capped at four rows, so that only happens on a pane with two or
    /// three rows to give, and there the beginning of an event is what identifies it.
    fn reveal_cursor(&self, room: usize) -> Option<usize> {
        let event = self.cursor_event()?;
        let entries_len = self.history().entries.len();
        let mut scroll = self.history_scroll;
        // Below the bottom edge: wind forward until its last row is the newest on screen.
        if entries_len.saturating_sub(scroll) < event.end() {
            scroll = entries_len.saturating_sub(event.end());
        }
        // Above the top edge: wind back until its first row is the oldest on screen.
        if event.start + room < entries_len.saturating_sub(scroll) {
            scroll = entries_len.saturating_sub(event.start + room);
        }
        (scroll != self.history_scroll).then_some(scroll)
    }

    /// The reader: one event of the timeline with the whole pane to itself.
    ///
    /// A divider of the same shape the halves have - what this is, and how far through it you
    /// are - and the event's markdown below it. `focused` is unconditionally true: there is
    /// nowhere else for the keys to be while this is up.
    fn render_reader(&mut self, rows: usize, cols: usize) {
        self.fill_reader_cache(cols);
        let Some(reading) = self.reading.as_ref() else {
            return;
        };
        // A row for the divider, the rest for the event.
        let room = rows.saturating_sub(1);
        self.last_reader_rows = room;
        let lines = self.reader_lines();
        let max_scroll = lines.len().saturating_sub(room);
        let scroll = reading.scroll.min(max_scroll);
        // The label is built here rather than held in `Reading` so the age goes on ticking with
        // the minute timer, the way every other age on this pane does.
        let mut label = reading.kind.clone();
        if let Some(then_ms) = reading.created_at_ms {
            label.push_str(&format!(" · {}", agents::short_age(now_ms(), then_ms)));
        }
        let end = (scroll + room).min(lines.len());
        render_divider(&label, scroll, lines.len() - end, true, 0, cols);
        for (index, line) in lines[scroll..end].iter().enumerate() {
            let line = line.clone().indent(1);
            let text = markdown::style_text(&line.text, &line.styles, false);
            print_text_with_coordinates(text, 0, 1 + index, Some(cols), None);
        }
        // Settled last, for the same reason the history half settles its own here: only this
        // knows the width the body was wrapped to, and so only this knows how many lines it
        // came to.
        self.last_reader_max_scroll = max_scroll;
        if let Some(reading) = self.reading.as_mut() {
            reading.scroll = scroll;
        }
    }

    /// [`Self::reader_lines`], computed once per event and width. See `fill_history_cache` for
    /// why anything on this pane is cached at all.
    fn fill_reader_cache(&mut self, width: usize) {
        let Some(reading) = self.reading.as_ref() else {
            return;
        };
        let key = (reading.id, width);
        if self.reader_cache.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        // The same margin the history half's body keeps, at both edges.
        let lines = markdown::render(
            &reading.text,
            width.saturating_sub(2),
            markdown::Options::compact(),
        );
        self.reader_cache = Some((key, lines));
    }

    /// Whatever [`Self::fill_reader_cache`] last put there.
    fn reader_lines(&self) -> &[markdown::MdLine] {
        self.reader_cache
            .as_ref()
            .map(|(_, lines)| lines.as_slice())
            .unwrap_or_default()
    }

    /// What the divider says: what the half is about, and whether a summary was asked for
    /// and has not come back.
    ///
    /// The hint is appended to the label rather than drawn anywhere of its own, because the
    /// divider is already the row that names the half - and `fit_label` cuts it to the room
    /// left over, so a narrow pane loses the hint rather than the rule.
    ///
    /// Compared against the selection rather than read as a flag: `summarizing` is cleared
    /// when the cursor moves, so this can only differ from it in the moment between a strip
    /// arriving and the selection being re-sent. Belt and braces on a hint that must never
    /// claim the row under the cursor is doing something it is not.
    fn history_label(&self) -> String {
        let label = &self.history().label;
        match self.divider_hint() {
            None => label.clone(),
            // A refusal is the longest thing the divider ever says and the only one that is
            // news, and `fit_label` cuts from the tail: sharing the row with a label is what
            // loses the half of it that says what went wrong.
            Some(Hint::Instead(hint)) => hint,
            Some(Hint::Beside(hint)) => match label.is_empty() {
                // A selected agent with neither a run nor a summary yet has no label at all,
                // and the first thing it ever says should not open with a separator.
                true => hint,
                false => format!("{label} · {hint}"),
            },
        }
    }

    /// What the divider has to say about the row under the cursor beyond naming it, if
    /// anything.
    ///
    /// At most one, and the question comes before the answer: an armed confirmation is what
    /// the user is being asked *now*, and a note from a refused resume is about a question
    /// already answered. `arm_resume` clears the note anyway, so the two can only overlap
    /// for as long as it takes a `y` to come back refused, but the order says which wins.
    ///
    /// Every arm is compared against the selection rather than read as a flag: `summarizing`
    /// and both resume fields are cleared when the cursor moves, so this can only differ from
    /// them in the moment between a strip arriving and the selection being re-sent. Belt and
    /// braces on a hint that must never claim the row under the cursor is doing something it
    /// is not.
    fn divider_hint(&self) -> Option<Hint> {
        let selection = self.selection_key();
        selection.as_ref()?;
        if self.confirming_resume == selection {
            return Some(Hint::Beside("resume this run? y/n".to_owned()));
        }
        if let Some((row, detail)) = self.resume_note.as_ref() {
            if Some(row) == selection.as_ref() {
                return Some(Hint::Instead(format!("resume failed: {detail}")));
            }
        }
        if self.summarizing == selection {
            return Some(Hint::Beside("summarising…".to_owned()));
        }
        None
    }

    /// [`Self::history_lines`], computed once per thing it depends on.
    ///
    /// The half is redrawn on every keystroke and on every strip hocus pushes, about once a
    /// second, and rendering it is now a markdown parse plus a style per displayed character -
    /// which `hocus/benches/transcript.rs` calls by far the most expensive thing a render does,
    /// and which the conversation needed its own caches to afford. Nothing here holds still
    /// except what the key names, so a hit is exact rather than approximate: walking the list,
    /// resizing the pane, or a run gaining an entry all miss, and scrolling the half - the one
    /// gesture that redraws fastest - always hits.
    ///
    /// Nothing is copied on a hit. What is cached is every rendered line of the run rather than
    /// the page being drawn - each carrying a style per character of it - so handing it back by
    /// value would have put a deep copy of the whole timeline on the path this exists to make
    /// cheap. The drawing loop clones the rows it actually prints, one at a time.
    fn fill_history_cache(&mut self, width: usize) {
        let key = HistoryKey {
            selection: self.selection_key(),
            width,
            generation: self.generation,
            minute: now_ms() / 60_000,
        };
        if self.history_cache.as_ref().is_some_and(|(c, _)| *c == key) {
            return;
        }
        let lines = self.history_lines(width);
        self.history_cache = Some((key, lines));
    }

    /// Whatever [`Self::fill_history_cache`] last put there, or nothing to draw.
    fn history(&self) -> &agents::HistoryLines {
        self.history_cache
            .as_ref()
            .map(|(_, lines)| lines)
            .unwrap_or(&agents::NO_HISTORY)
    }

    /// [`Self::fill_history_cache`] and then [`Self::history`] in one call. `render_history`
    /// has the scroll to settle in between and so does the two itself; this is for the tests
    /// that only want the answer.
    #[cfg(test)]
    fn cached_history_lines(&mut self, width: usize) -> &agents::HistoryLines {
        self.fill_history_cache(width);
        self.history()
    }

    /// What the history half has to draw for the row under the cursor, for a half `width`
    /// columns wide. The margin inside that is `agents`' to take: the body keeps one, the
    /// divider wants the whole row.
    ///
    /// A loaded run first - the whole point of asking hocus for one - then the idle summary
    /// hocus pushes with every strip, then nothing. Empty is a real answer here rather than a
    /// reason not to draw: see `render_history`.
    ///
    /// Not called from `render`: `cached_history_lines` is, for what it costs.
    fn history_lines(&self, width: usize) -> agents::HistoryLines {
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| self.selected_run_matches(run))
        {
            return agents::run_history(run, width, now_ms(), self.run_offset_secs);
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
        agents::summary_history(pane_id, kind, summary, width, now_ms())
    }

    /// Whether anything the history half draws is an age - the one thing on this pane that
    /// goes stale on its own.
    ///
    /// Asked of the same two sources `history_lines` draws from, in the same order, so the
    /// answer cannot claim an age the half is not showing.
    fn shows_an_age(&self) -> bool {
        // The reader is the whole pane, so what it says is the only thing that can go stale.
        if let Some(reading) = self.reading.as_ref() {
            return reading.created_at_ms.is_some();
        }
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| self.selected_run_matches(run))
        {
            return run
                .entries
                .iter()
                .any(|entry| entry.created_at_ms.is_some())
                || run
                    .summary
                    .as_ref()
                    .is_some_and(|summary| summary.created_at_ms.is_some());
        }
        self.selection()
            .and_then(|pane_id| self.summaries.get(&pane_id))
            .is_some_and(|summary| summary.created_at_ms.is_some())
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

    /// Move the history half's cursor by `delta` events, positive being back into the past, and
    /// say whether it went anywhere.
    ///
    /// The counterpart of `scroll_history`, and the one place `history_cursor` changes, so the
    /// clamp cannot be forgotten at a call site. Clamped against the events the *cache* holds
    /// rather than the run's own entries: what a cursor can point at is what the half drew, and
    /// the two differ - a result is filtered out of a live run's timeline, and the run's
    /// summary is a row the run does not have an entry for.
    fn step_history_cursor(&mut self, delta: isize) -> bool {
        let last = self.history().events.len().saturating_sub(1);
        let target = match delta >= 0 {
            true => self
                .history_cursor
                .saturating_add(delta.unsigned_abs())
                .min(last),
            false => self.history_cursor.saturating_sub(delta.unsigned_abs()),
        };
        let moved = target != self.history_cursor;
        self.history_cursor = target;
        moved
    }

    /// The event under the history half's cursor, if there is one.
    ///
    /// `history_cursor` counts back from the newest - see the field - so the index into the
    /// oldest-first `events` is measured from the far end.
    fn cursor_event(&self) -> Option<&agents::HistoryEvent> {
        let events = &self.history().events;
        let index = events.len().checked_sub(self.history_cursor + 1)?;
        events.get(index)
    }

    /// Open the event under the cursor in the reader, and say whether anything changed.
    ///
    /// Everything the reader needs is copied out of the event rather than referred back to:
    /// see [`Reading`]. Nothing to open is not a failure - a half showing a placeholder has no
    /// event under its cursor - and Enter simply does nothing there.
    fn open_reader(&mut self) -> bool {
        let Some(event) = self.cursor_event() else {
            return false;
        };
        let (kind, created_at_ms, text) =
            (event.kind.clone(), event.created_at_ms, event.text.clone());
        self.reading_id += 1;
        self.reading = Some(Reading {
            kind,
            created_at_ms,
            text,
            scroll: 0,
            id: self.reading_id,
        });
        true
    }

    /// Put the reader away, and say whether there was one.
    ///
    /// The cache goes with it: it is keyed on an id that never comes round again, so keeping it
    /// would only hold a copy of the whole event for as long as the pane is open.
    fn close_reader(&mut self) -> bool {
        self.reader_cache = None;
        self.reading.take().is_some()
    }

    /// A key with the reader up. Everything in here is about the one event on screen.
    ///
    /// `Esc` closes the reader rather than dismissing the panel: the pane's Esc puts the whole
    /// panel away, and with a reader open the thing being dismissed is the reader. Every other
    /// key is swallowed, because the list and the halves are not on screen and a key that
    /// steered them would move something the user cannot see.
    fn read_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => self.close_reader(),
            BareKey::Up => self.scroll_reader(-1),
            BareKey::Down => self.scroll_reader(1),
            BareKey::Char('k') if key.has_no_modifiers() => self.scroll_reader(-1),
            BareKey::Char('j') if key.has_no_modifiers() => self.scroll_reader(1),
            BareKey::PageUp => self.scroll_reader(-(self.reader_page() as isize)),
            BareKey::PageDown => self.scroll_reader(self.reader_page() as isize),
            _ => false,
        }
    }

    /// One page of the reader: what it can show, less a line of overlap, the same bargain
    /// `page` strikes for the history half.
    fn reader_page(&self) -> usize {
        self.last_reader_rows.saturating_sub(1).max(1)
    }

    /// Move the reader by `delta` lines, positive being *down* into the event, and say whether
    /// it moved.
    ///
    /// The opposite sign to `scroll_history`, and deliberately: the half is a timeline anchored
    /// to its newest end, where winding back means going up; the reader is a piece of prose
    /// anchored to its head, where the scroll is a line number.
    fn scroll_reader(&mut self, delta: isize) -> bool {
        let max = self.last_reader_max_scroll;
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        let target = match delta >= 0 {
            true => reading.scroll.saturating_add(delta.unsigned_abs()).min(max),
            false => reading.scroll.saturating_sub(delta.unsigned_abs()),
        };
        let moved = target != reading.scroll;
        reading.scroll = target;
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
    /// **The hocus it opens that way appears**, unfocused, beside this pane - the panel is both
    /// halves or it is nothing. It does not take the keyboard, because the user asked for a *list*;
    /// `wire::OPENED_BY` is the marker that says which of the two they asked for, and
    /// `should_focus: false` is what acts on it. (This paragraph used to claim the launched hocus
    /// started *hidden*. Nothing ever made it so - `State::note_greeting_from_agents` only records
    /// that a list exists - and `a36cf7830` settled the question the other way.)
    ///
    /// `MessageToPlugin` with a `plugin_url` is launch-or-reuse - an instance with the same
    /// (location, configuration) is handed the message rather than a second one being spawned -
    /// which is the same idempotence hocus relies on in the other direction. **Reuse is by
    /// configuration, so a hocus the user put in their layout as `plugin location="hocus"` is not
    /// reused**: this launches an overlay one beside the list, which is right, because a tiled
    /// hocus is not a panel and cannot be the list's other half (`State::is_panel`). What must
    /// never happen is a list being opened beside a tiled hocus that nobody asked for one from -
    /// and it cannot, because that hocus never launches a list in the first place.
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
                // jumping to the conversation. It still *appears* - the panel is both halves
                // or it is nothing - it simply does not take the focus.
                .new_plugin_instance_should_not_be_focused(),
        );
    }

    /// Hand the keyboard to the conversation, landing on `stop`.
    ///
    /// The asymmetric half of the ring's two crossings. hocus can focus this pane itself,
    /// holding our plugin id off `send_selection`; we hold nothing of its, so stepping the
    /// other way is a request - the same shape as the `AGENT_STRIP_HIDE` an Esc sends, and
    /// carrying no `plugin_url` for the same reason: a Tab must never launch a conversation.
    fn yield_to_chat(&mut self, stop: &'static str) {
        #[cfg(test)]
        self.ring_yields.push(stop);
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_YIELD).with_args(
            BTreeMap::from([(wire::PANEL_STOP.to_owned(), stop.to_owned())]),
        ));
    }

    /// `AGENT_STRIP_TAKE`: the ring arrived from the conversation, on `stop`.
    fn take_ring_stop(&mut self, stop: Option<&str>) -> bool {
        let focus = match stop {
            Some(wire::STOP_HISTORY) => Focus::History,
            _ => Focus::List,
        };
        let changed = self.focus != focus;
        self.focus = focus;
        changed
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

    /// An `AGENT_STRIP_TAKE` naming `stop`, as hocus sends it when the ring steps in.
    fn take(stop: &str) -> PipeMessage {
        PipeMessage::new(
            PipeSource::Plugin(0),
            wire::AGENT_STRIP_TAKE,
            &None,
            &Some(BTreeMap::from([(
                wire::PANEL_STOP.to_owned(),
                stop.to_owned(),
            )])),
            false,
        )
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
                    created_at_ms: None,
                },
            )]),
        }
    }

    /// The ages are drawn from the clock, so the minute tick is the only thing that can
    /// refresh them - and it redraws only where there is an age on screen, so a pane showing
    /// nothing time-dependent still costs nothing to leave open.
    #[test]
    fn the_minute_tick_redraws_only_where_an_age_is_shown() {
        let mut state = State::default();
        state.set_rows(with_summary(
            strip(),
            3,
            "Waiting on a permission prompt.",
            true,
        ));
        assert!(
            !state.shows_an_age(),
            "a summary made before there were stamps has no age to move"
        );

        let mut stamped = with_summary(strip(), 3, "Waiting on a permission prompt.", true);
        stamped
            .summaries
            .get_mut(&3)
            .expect("the summary just pushed")
            .created_at_ms = Some(1);
        state.set_rows(stamped);
        assert!(state.shows_an_age());

        // And a run, whose entries wear their ages in a column of their own.
        state.run = Some(a_run());
        state.set_rows(unfocused(vec![
            StripRow::Project("hocus".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ]));
        assert!(!state.shows_an_age(), "nothing in `a_run` is stamped");
        state
            .run
            .as_mut()
            .expect("loaded")
            .entries
            .first_mut()
            .expect("a timeline")
            .created_at_ms = Some(1);
        assert!(state.shows_an_age());
    }

    /// The once-a-second push that moves nothing must cost nothing: `generation` is what
    /// throws the history cache away, and the cache exists to keep a settled pane from
    /// re-parsing a summary's markdown every second.
    ///
    /// A summary carries the time it was made now, and `Summary` derives `PartialEq`, so a
    /// daemon that re-stamped an unchanged summary would move `generation` on every poll.
    /// `agents::stamp_summary` is what stops that; this is the pane's half of the same claim.
    #[test]
    fn a_summary_that_says_the_same_thing_does_not_bounce_the_history_cache() {
        let mut state = State::default();
        let push = || with_summary(strip(), 3, "Waiting on a permission prompt.", true);
        assert!(state.set_rows(push()));
        state.fill_history_cache(48);
        let generation = state.generation;
        let cached = state.history().clone();

        assert!(
            !state.set_rows(push()),
            "the same rows and the same summary said nothing"
        );
        assert_eq!(state.generation, generation);
        state.fill_history_cache(48);
        assert_eq!(state.history(), &cached, "and the cache survived it");
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

    /// The history's share is body and the pinned header, and nothing else: `split` has already
    /// taken the divider off the list's, so taking it off again here is what left the last row
    /// of the pane blank.
    ///
    /// Nothing observable is printed by a unit test, so the height is read off the room the
    /// scroll is clamped against - which is the share less the header's two rows and the rule
    /// under them, and nothing else.
    #[test]
    fn the_history_body_is_its_share_less_the_pinned_header() {
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
        // No summary, so the thirty questions are the whole timeline.
        run.summary = None;
        run.entries = (0..30)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
                created_at_ms: None,
            })
            .collect();
        state.run = Some(run);

        let (_, history_rows) = split(20);
        state.render(20, 50);
        assert_eq!(state.last_history_rows, history_rows);
        // Two header rows and the rule under them.
        let body_rows = history_rows - 3;
        assert_eq!(
            state.last_history_max_scroll,
            30 - body_rows,
            "every row of the share the header did not take holds a line of timeline"
        );
        assert_eq!(state.page(), body_rows - 1, "less a line of overlap");

        // A half with no room for the header and a line of timeline both drops the header
        // whole rather than squeezing the timeline: the run's two facts are worth less than
        // what it did.
        let mut cramped = State::default();
        std::mem::swap(&mut cramped, &mut state);
        cramped.render(9, 50);
        let (_, cramped_rows) = split(9);
        assert_eq!(cramped_rows, 4, "three rows of header and one of timeline");
        assert_eq!(
            cramped.last_history_max_scroll,
            30 - cramped_rows,
            "the header is dropped whole on a half with no room for it and a timeline both"
        );
    }

    /// The offset the host gave for the run's start reaches the row that draws it, and the
    /// header is not part of the timeline: scrolling the half moves the entries under it and
    /// leaves it where it is.
    #[test]
    fn the_pinned_header_carries_the_offset_and_does_not_scroll() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();
        // 2026-09-04 18:32:07 UTC.
        run.started_at_ms = 1_788_546_727_000;
        run.entries = (0..30)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
                created_at_ms: None,
            })
            .collect();
        state.run = Some(run);
        // What the host would have answered for that instant; a native test build gets `Err`
        // from the shim and falls back to UTC, so it is set here the way `pipe` would.
        state.run_offset_secs = -4 * 3600;

        state.render(20, 60);
        let header: Vec<String> = state
            .history()
            .header
            .iter()
            .map(|line| line.text.clone())
            .collect();
        assert_eq!(
            header,
            vec![
                "started 2026-09-04 14:32:07 -04:00".to_owned(),
                "session 9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned(),
            ]
        );

        assert!(state.scroll_history(5));
        state.render(20, 60);
        let after: Vec<String> = state
            .history()
            .header
            .iter()
            .map(|line| line.text.clone())
            .collect();
        assert_eq!(header, after, "the header is not part of what scrolls");
    }

    /// With no history half there is no history stop: forward from the list steps straight on
    /// to the conversation, and the ring stays four stops only while the pane has four to give.
    #[test]
    fn a_pane_with_no_history_half_rings_straight_on_to_the_conversation() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(5, 40);
        assert_eq!(state.last_history_rows, 0);

        assert!(!state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.ring_yields, ["tabs"]);
        assert_eq!(
            state.focus,
            Focus::List,
            "and never claimed a half it has not got"
        );
    }

    /// Backward off the first stop leaves the pane the other way, landing in the box.
    #[test]
    fn shift_tab_from_the_list_hands_the_keyboard_to_the_input_box() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(20, 40);
        assert_eq!(state.focus, Focus::List);

        assert!(!state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
        assert_eq!(state.ring_yields, ["input"]);
    }

    /// A terminal that cannot report Shift+Tab sends a plain Tab, which reads as forward. The
    /// ring wraps, so nothing is out of reach - backward is a shortcut, never the only way.
    #[test]
    fn an_unreported_shift_tab_reads_as_a_forward_step() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(20, 40);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(
            state.focus,
            Focus::History,
            "forward, as a bare Tab always is"
        );
    }

    /// The ring arriving from the conversation names the half, and hocus has already moved the
    /// keyboard: all this does is say where in the pane the arriving keys belong.
    #[test]
    fn a_take_lands_the_keys_on_the_half_it_names() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(20, 40);

        assert!(state.pipe(take(wire::STOP_HISTORY)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.pipe(take(wire::STOP_LIST)));
        assert_eq!(state.focus, Focus::List);
        assert!(
            !state.pipe(take(wire::STOP_LIST)),
            "already there, so nothing to redraw"
        );
    }

    /// The reader is the whole pane while it is open, and a ring step out of something you are
    /// reading is wrong: Esc closes it first. The same relationship the conversation's picker
    /// has to the ring.
    #[test]
    fn the_reader_keeps_tab_to_itself() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.entries = vec![agents::RunEntry {
            revision: 1,
            event_id: None,
            kind: agents::RunEntryKind::Prompt,
            text: "implement history".to_owned(),
            created_at_ms: None,
        }];
        state.run = Some(run);
        state.render(20, 50);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.focus, Focus::History);
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.reading.is_some(), "the reader is open");

        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert!(state.ring_yields.is_empty(), "the reader took the key");
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
            "no other half here, so the keys carry on to the conversation"
        );
        assert_eq!(state.focus, Focus::List);
    }

    /// The list emptying takes the history half with it, so the keys cannot stay in one: a
    /// stop that is not drawn would swallow a Tab moving to an invisible half.
    #[test]
    fn an_empty_list_puts_the_keys_back_in_the_list_half() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(20, 40);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.focus, Focus::History);

        // The last agent exits, and the strip arrives with nothing in it.
        state.set_rows(unfocused(vec![]));
        state.render(20, 40);

        assert_eq!(state.last_history_rows, 0);
        assert_eq!(state.focus, Focus::List, "and the focus came back with it");
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()),
            "so backward leaves the pane"
        );
        assert_eq!(state.ring_yields, ["input"]);
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
            history
                .entries
                .iter()
                .any(|row| row.text.contains("all green")),
            "{history:?}"
        );

        // and it follows the cursor off that row
        state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        assert!(state.history_lines(38).is_empty(), "pane 7 has no summary");
    }

    /// `s` is taken on both kinds of row that name an agent, and on neither kind that does
    /// not. What it sends is not asserted here - the message reaches the host - but which
    /// rows take the key is this pane's whole share of the decision.
    #[test]
    fn s_is_taken_on_a_row_that_names_an_agent_and_nowhere_else() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));

        state.selected = 0;
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
            "a project header names no agent"
        );
        assert!(state.summarizing.is_none());

        state.selected = 2;
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
            "a live agent does, and the divider says so"
        );
        assert_eq!(state.summarizing.as_deref(), Some("pane:3"));
        assert_eq!(state.selected, 2, "and the cursor stays where it was");

        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
            StripRow::MoreRuns,
        ]));
        state.selected = 1;
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
            "an archived run is summarised from what it recorded"
        );
        assert_eq!(state.summarizing.as_deref(), Some("run:claude:session-1"));

        state.selected = 2;
        state.summarizing = None;
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
            "`load older runs…` is Enter's, and names nothing to describe"
        );
        assert!(state.summarizing.is_none());
    }

    /// One archive row, for the resume tests: the shape hocus pushes for a completed run.
    fn archive() -> Vec<StripRow> {
        vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
            StripRow::MoreRuns,
        ]
    }

    /// `r` is the one key here that starts a process, so it is taken on exactly the rows that
    /// name something resumable and nowhere else. A live agent is deliberately not one of
    /// them: `Enter` already walks to its pane, and a second process on its session id would
    /// be two agents writing one conversation.
    #[test]
    fn r_arms_a_confirmation_on_an_archived_run_and_nowhere_else() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));

        state.selected = 0;
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert!(state.confirming_resume.is_none(), "a project header");

        state.selected = 2;
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert!(
            state.confirming_resume.is_none(),
            "a live agent has its pane"
        );

        state.set_rows(unfocused(archive()));
        state.selected = 2;
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert!(state.confirming_resume.is_none(), "`load older runs…`");

        state.selected = 1;
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Char('r'))),
            "an archived run does, and the divider says so"
        );
        assert_eq!(
            state.confirming_resume.as_deref(),
            Some("run:claude:session-1")
        );
        assert!(
            state.resume_requests.is_empty(),
            "and nothing has been started yet - that is the whole point of asking"
        );
    }

    /// `r` is a letter, so a modified one belongs to whatever the modifiers are bound to.
    /// `Ctrl-r` in particular is already this pane's refresh.
    #[test]
    fn a_modified_r_is_left_alone() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier()));
        assert!(state.confirming_resume.is_none());
    }

    /// `y` is the only key that starts it. Everything else drops the question, and the two
    /// that mean "no" stop there rather than also doing whatever they normally do.
    #[test]
    fn only_y_answers_the_resume_confirmation() {
        let arm = |key: KeyWithModifier| {
            let mut state = State::default();
            state.set_rows(unfocused(archive()));
            state.selected = 1;
            state.on_key(KeyWithModifier::new(BareKey::Char('r')));
            state.on_key(key);
            state
        };

        let state = arm(KeyWithModifier::new(BareKey::Char('y')));
        assert_eq!(state.resume_requests, ["claude:session-1"]);
        assert!(state.confirming_resume.is_none(), "asked and answered");

        for key in [
            KeyWithModifier::new(BareKey::Char('n')),
            KeyWithModifier::new(BareKey::Esc),
            KeyWithModifier::new(BareKey::Char('Y')).with_shift_modifier(),
        ] {
            let state = arm(key.clone());
            assert!(state.resume_requests.is_empty(), "{key} started something");
            assert!(
                state.confirming_resume.is_none(),
                "{key} left the question up"
            );
        }
    }

    /// `Esc` at the question is one step back, not the way out: the panel stays up, exactly
    /// as it does for an `Esc` that closes the reader.
    #[test]
    fn esc_at_the_question_drops_it_rather_than_the_panel() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Esc)),
            "the divider changed, so the pane redraws"
        );
        assert!(state.confirming_resume.is_none());

        // and a second Esc, with nothing to take back, is the dismissal it always was
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Esc)));
    }

    /// A key that is neither yes nor no drops the question and then means what it always
    /// means. Someone who typed `r` by mistake and carried on walking the list should not
    /// have to acknowledge it first - and the cursor moving is itself what ends the question,
    /// so the two cannot disagree about which row is being asked about.
    #[test]
    fn walking_off_the_question_drops_it_and_still_walks() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));

        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert!(state.confirming_resume.is_none());
        assert_eq!(state.selected, 2, "and the arrow moved");
        assert!(state.resume_requests.is_empty());

        // A `y` now is a `y` at no question at all, and starts nothing.
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert!(state.resume_requests.is_empty());
    }

    /// A strip arriving between the question and the answer can move the cursor. The
    /// confirmation names the row it was armed on, so a `y` cannot start a run the user was
    /// not looking at when they were asked.
    #[test]
    fn a_y_after_the_cursor_moved_under_it_starts_nothing() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));

        // Same shape, different run: the archive gained a newer row above this one.
        let mut rows = archive();
        rows.insert(
            1,
            StripRow::Run {
                run_id: "claude:session-2".to_owned(),
                text: "claude · newer".to_owned(),
                kind: "claude".to_owned(),
            },
        );
        state.rows = rows;

        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert!(
            state.resume_requests.is_empty(),
            "the row under the cursor is not the row that was asked about"
        );
    }

    /// The divider carries the question, and then whatever came back. A refusal is shown
    /// because the user answered a confirmation: a `y` that silently does nothing is worse
    /// than never having been asked. Like every other hint here it ends when the cursor
    /// walks off the row it is about.
    #[test]
    fn the_divider_asks_and_then_says_why_it_was_refused() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "");

        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        assert_eq!(state.history_label(), "resume this run? y/n");

        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert_eq!(state.history_label(), "", "the question has been answered");

        let refuse = |state: &mut State, run_id: &str, detail: &str| {
            let result = wire::ResumeResult {
                run_id: run_id.to_owned(),
                detail: detail.to_owned(),
            };
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(0),
                wire::AGENT_RESUME_RESULT,
                &Some(serde_json::to_string(&result).expect("encoded")),
                &None,
                false,
            ))
        };

        assert!(refuse(
            &mut state,
            "claude:session-1",
            "this run recorded no session id to resume"
        ));
        assert_eq!(
            state.history_label(),
            "resume failed: this run recorded no session id to resume"
        );

        // An answer about a row the cursor is not on says nothing about the row it is on.
        state.on_key(KeyWithModifier::new(BareKey::Down));
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "", "walking off ends it");
        assert!(!refuse(&mut state, "claude:session-1", "gone"));
        assert_eq!(state.history_label(), "");
    }

    /// Asking again clears the last refusal: the note is about a question that has just been
    /// asked a second time, and leaving it up beside the new one would read as an answer to
    /// it.
    #[test]
    fn asking_again_clears_the_last_refusal() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.resume_note = Some((
            "run:claude:session-1".to_owned(),
            "the pane could not be opened".to_owned(),
        ));
        state.fill_history_cache(48);
        assert!(state.history_label().starts_with("resume failed"));

        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        assert!(state.resume_note.is_none());
        assert_eq!(state.history_label(), "resume this run? y/n");
    }

    /// A refusal takes the whole divider rather than sharing it with the label: it is the
    /// longest thing this row ever says, `fit_label` cuts from the tail, and the half that
    /// would be lost is the half that says what went wrong.
    ///
    /// And it comes down for the next request the row makes, `s` as much as `r`. It outranks
    /// `summarising…` and outlives it, so leaving it up would make `s` on a row that has just
    /// refused an `r` look like a key that does nothing at all.
    #[test]
    fn a_refusal_takes_the_whole_divider_until_the_row_asks_for_something_else() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.pipe(PipeMessage::new(
            PipeSource::Plugin(0),
            wire::AGENT_RUN_DETAIL,
            &Some(serde_json::to_string(&a_run()).expect("encoded")),
            &None,
            false,
        ));
        state.fill_history_cache(48);
        let label = state.history_label();
        assert!(!label.is_empty(), "the row names the run it is about");

        state.resume_note = Some((
            "run:claude:session-1".to_owned(),
            "hocus cannot open a pane under /tmp: the plugin mounts claim it".to_owned(),
        ));
        assert_eq!(
            state.history_label(),
            "resume failed: hocus cannot open a pane under /tmp: the plugin mounts claim it"
        );

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        assert!(state.resume_note.is_none());
        assert_eq!(state.history_label(), format!("{label} · summarising…"));
    }

    /// `s` is a letter, so it must not be taken from whatever the modifiers are bound to,
    /// and it must not be mistaken for the history scroll `j`/`k` do.
    #[test]
    fn a_modified_s_is_left_alone() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));

        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Char('s')).with_ctrl_modifier()),
            "ctrl-s is flow control in a great many terminals and is nobody's here"
        );
        assert!(state.summarizing.is_none());
    }

    /// The hint on the divider is a hint: it says a request is out, and it goes as soon as
    /// there is any answer or the cursor moves off the row it was about. Nothing ever tells
    /// this pane that a request was refused, so a hint with no ending of its own would be a
    /// lie the user could not clear.
    #[test]
    fn the_divider_says_a_summary_was_asked_for_until_something_answers() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 2;
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "");

        state.on_key(KeyWithModifier::new(BareKey::Char('s')));
        assert_eq!(state.history_label(), "summarising…");

        // A run arriving is the only answer an `s` ever gets.
        let mut run = a_run();
        run.pane_id = 3;
        run.ended_at_ms = None;
        run.outcome = agents::RunOutcome::Active;
        state.pipe(PipeMessage::new(
            PipeSource::Plugin(0),
            wire::AGENT_RUN_DETAIL,
            &Some(serde_json::to_string(&run).expect("encoded")),
            &None,
            false,
        ));
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "claude:session-1");

        // And walking away from an answer that never came takes the hint with it. `update`
        // is what clears it, and reaches the host to re-send the selection, so what is
        // driven here is the guard that makes the clearing safe rather than the clearing:
        // a hint left standing by any route still cannot claim the row it is not about.
        state.on_key(KeyWithModifier::new(BareKey::Char('s')));
        assert_eq!(state.history_label(), "claude:session-1 · summarising…");
        state.on_key(KeyWithModifier::new(BareKey::Down));
        state.fill_history_cache(48);
        assert_eq!(
            state.history_label(),
            "",
            "the cursor is on another agent now, and that one was not asked about"
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
        state.run = Some(a_run());

        let history = state.history_lines(48);
        assert_eq!(history.label, "claude:session-1");
        assert!(history
            .entries
            .iter()
            .any(|row| row.text.contains("summary: History")));
        assert!(history
            .entries
            .iter()
            .any(|row| row.text.contains("question: Keep")));
        assert!(!history
            .entries
            .iter()
            .any(|row| row.text.contains("Bash cargo")));
        assert!(!history
            .entries
            .iter()
            .any(|row| row.text.contains("result: Implemented")));
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
            created_at_ms: None,
        });
        state.run = Some(run);
        // Three rows of history below the divider, against four lines of timeline - the three
        // entries the filter lets through and the run's summary under them - so exactly one
        // line to wind back over.
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

    /// The summary is a row of the timeline like any other, so a talkative one costs the
    /// events beside it nothing they cannot be scrolled back to: the whole half winds over it.
    #[test]
    fn a_long_summary_scrolls_off_the_top_like_any_other_event() {
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
        run.summary = Some(agents::Summary {
            // Four rows of it at this width - more than the half has to give, which under the
            // old pinned block would have left one row of timeline showing whatever the scroll.
            text: "word ".repeat(40),
            ..run.summary.take().unwrap()
        });
        state.run = Some(run);

        // Three rows below the divider, and every one of them a line of timeline.
        state.render(7, 50);
        assert_eq!(state.last_history_rows, 3);
        let lines = state.cached_history_lines(50).entries.len();
        assert!(lines > 5, "a summary longer than the half: {lines} lines");
        assert_eq!(state.last_history_max_scroll, lines - 3);

        // At rest the half is against the newest end, and the events above the summary have
        // been wound past - under the old pinned block the top of it was always on screen.
        let lines = state.cached_history_lines(50).entries.len();
        assert!(!state.cached_history_lines(50).entries[lines - 3..]
            .iter()
            .any(|row| row.text.contains("prompt:")),);

        // Wound all the way back, the oldest event is at the top of the half and the tail of
        // the summary has scrolled off the bottom.
        assert!(state.on_mouse(Mouse::ScrollUp(50)));
        state.render(7, 50);
        assert_eq!(state.history_scroll, state.last_history_max_scroll);
        let history = state.cached_history_lines(50);
        assert!(history.entries[0]
            .text
            .contains("prompt: implement history"));
        assert!(
            !history.entries[..3]
                .iter()
                .any(|row| row.text.ends_with("word") && !row.text.contains("summary:")),
            "only the first line of the summary is still on screen"
        );
    }

    /// `Tab` and `Shift+Tab` walk the ring, `j`/`k` do whatever the half they are in does with
    /// a step - a row of the list, an event of the timeline - and the arrows walk the list from
    /// either half.
    ///
    /// Directional now: forward off the history half leaves this pane for the conversation, so
    /// getting back to the list is `Shift+Tab` rather than another `Tab`.
    #[test]
    fn tab_walks_the_ring_and_jk_follow_the_focus() {
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
            created_at_ms: None,
        });
        state.run = Some(run);
        // two lines of timeline against three entries: exactly one line to scroll
        state.render(7, 50);
        assert_eq!(state.selected, 1);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(
            state.history_cursor, 1,
            "k walks the timeline it is in, an event at a time"
        );
        assert_eq!(state.selected, 1, "and moves no selection");

        // the arrows steer the list from either half, and take the keys back with them
        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert_eq!(state.selected, 2);
        assert_eq!(state.focus, Focus::List);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(state.selected, 1, "k walks the list it is back in");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        // Forward off the history half is off the end of this pane: the conversation's tab bar
        // is the next stop, and the keys go with a request rather than a redraw.
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.ring_yields, ["tabs"]);
        assert_eq!(
            state.focus,
            Focus::History,
            "the sub-focus stays where it was left - the pane frame is what says the keys are \
             gone, and coming back by a click should land where the user was"
        );
        // Backward is the way back inside the pane.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
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

    /// A strip of one archived run, the shape every history test below wants.
    fn one_run_selected() -> Strip {
        unfocused(vec![
            StripRow::Project("archive".to_owned()),
            StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                text: "claude · hook history".to_owned(),
                kind: "claude".to_owned(),
            },
        ])
    }

    /// Walking the cursor onto an event the half is not showing winds the half to it. The key
    /// handler cannot do this itself: how many lines the half has depends on the width, and
    /// only `render` is told the width.
    #[test]
    fn the_cursor_winds_the_half_back_to_the_event_it_lands_on() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.entries = (0..30)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
                created_at_ms: None,
            })
            .collect();
        state.run = Some(run);
        state.focus = Focus::History;
        state.render(20, 50);
        // Thirty one-line events against the seven rows the pinned header leaves of the ten:
        // the half is at the newest end.
        assert_eq!(state.last_history_rows, 10);
        assert_eq!(state.last_history_body_rows, 7);
        assert_eq!(state.history_scroll, 0);

        // Walk the cursor past the top of the window and the half follows it back.
        for _ in 0..12 {
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        }
        state.render(20, 50);
        assert_eq!(state.history_cursor, 12);
        assert_eq!(
            state.history_scroll, 6,
            "wound back just far enough to put the event on the oldest row"
        );

        // And back down again: the half winds forward only as far as it has to.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        state.render(20, 50);
        assert_eq!(state.history_cursor, 11);
        assert_eq!(state.history_scroll, 6, "still on screen, so nothing moved");

        // The cursor stops at the oldest event rather than running off the end.
        for _ in 0..50 {
            state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        }
        state.render(20, 50);
        assert_eq!(state.history_cursor, 29);
        assert_eq!(state.history_scroll, state.last_history_max_scroll);
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    }

    /// `Enter` in the history half opens the event under the cursor, uncapped and with the
    /// whole pane to read it in; `Esc` puts the reader away and leaves the pane where it is.
    /// The list half's `Enter` is untouched - the two halves have separate cursors.
    #[test]
    fn enter_in_the_history_half_opens_the_event_and_esc_comes_back() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.entries = vec![agents::RunEntry {
            revision: 1,
            event_id: None,
            kind: agents::RunEntryKind::Prompt,
            text: "word ".repeat(400),
            created_at_ms: Some(1),
        }];
        state.run = Some(run);
        state.render(20, 50);

        // Capped in the half, whole in the reader.
        assert_eq!(
            state.cached_history_lines(50).entries.len(),
            agents::MAX_ENTRY_ROWS
        );
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Enter)),
            "the keys are in the list half, where Enter is the list's - and this row is an \
             archived run, which has no pane to walk to"
        );
        assert!(state.reading.is_none());

        state.focus = Focus::History;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        let reading = state.reading.as_ref().expect("the event under the cursor");
        assert_eq!(reading.kind, "prompt");
        assert_eq!(reading.created_at_ms, Some(1));
        assert_eq!(reading.text, "word ".repeat(400));

        state.render(20, 50);
        assert!(
            state.reader_lines().len() > agents::MAX_ENTRY_ROWS,
            "the whole of it, which is the point of opening it"
        );
        assert!(state.last_reader_max_scroll > 0);

        // The reader has the keys while it is up, and scrolls from its head.
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert_eq!(state.reading.as_ref().unwrap().scroll, 1);
        assert_eq!(state.selected, 1, "and steers nothing behind it");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(state.reading.is_none(), "Esc closed the reader");
        assert!(state.reader_cache.is_none(), "and let go of its copy");
    }

    /// A summary somebody asked for arrives as an entry of the run, so the reader's divider
    /// says what it is and how old it is - and the minute tick has to keep it honest.
    #[test]
    fn the_reader_is_an_age_that_goes_on_ticking() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.entries = vec![agents::RunEntry {
            revision: 1,
            event_id: None,
            kind: agents::RunEntryKind::Prompt,
            text: "implement history".to_owned(),
            created_at_ms: None,
        }];
        state.run = Some(run);
        state.render(20, 50);
        state.focus = Focus::History;
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.reading.is_some());
        assert!(!state.shows_an_age(), "an unstamped event has none to tick");

        state.reading.as_mut().unwrap().created_at_ms = Some(1);
        assert!(state.shows_an_age());
    }

    /// The reader is about an event of the row the cursor was on, so walking off that row
    /// takes it with you: a strip whose focus lands on another agent moves the selection, and
    /// what is on screen has to follow.
    #[test]
    fn a_strip_that_moves_the_selection_closes_the_reader() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(20, 50);
        state.focus = Focus::History;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        assert!(state.reading.is_some(), "the idle summary is an event too");

        // What `pipe` does with a strip, less the host call it ends with.
        let was = state.selection_key();
        state.set_rows(focused_on(strip(), 7));
        assert!(state.follow_selection(was));
        assert_eq!(state.selection(), Some(7), "the focus walked onto pane 7");
        assert!(state.reading.is_none());
    }

    /// The reader's body is a markdown parse of the whole event, on a pane that redraws on
    /// every keystroke - so it is wrapped once per event and width, the way the history half
    /// below it is. Read by poisoning the cache, as `the_history_is_rendered_once_per_thing_it
    /// _depends_on` does.
    #[test]
    fn the_reader_is_wrapped_once_per_event_and_width() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.entries = vec![agents::RunEntry {
            revision: 1,
            event_id: None,
            kind: agents::RunEntryKind::Prompt,
            text: "word ".repeat(50),
            created_at_ms: None,
        }];
        state.run = Some(run);
        state.render(20, 50);
        state.focus = Focus::History;
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.render(20, 50);

        let poison = vec![markdown::MdLine::plain("poison")];
        let key = state.reader_cache.as_ref().unwrap().0;
        state.reader_cache = Some((key, poison.clone()));
        state.fill_reader_cache(50);
        assert_eq!(state.reader_lines(), poison, "same event, same width");
        state.fill_reader_cache(40);
        assert_ne!(state.reader_lines(), poison, "a resize rewraps it");

        // And a second opening is a second event, however alike the two look.
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.fill_reader_cache(40);
        assert_ne!(state.reader_lines(), poison);
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
                created_at_ms: None,
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

    /// Rendering the half is a markdown parse per entry, and the half is redrawn on every
    /// keystroke, so it is computed once per thing it depends on rather than once per frame.
    ///
    /// Read by poisoning the cache: a second render that draws the poison never recomputed,
    /// which is the only observable difference between a hit and a very fast miss.
    #[test]
    fn the_history_is_rendered_once_per_thing_it_depends_on() {
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
        state.render(20, 50);

        let poison = agents::HistoryLines {
            label: "poison".to_owned(),
            ..agents::HistoryLines::default()
        };
        let key = state.history_cache.as_ref().unwrap().0.clone();
        state.history_cache = Some((key, poison.clone()));

        assert_eq!(
            state.cached_history_lines(50),
            &poison,
            "same row, same width, same run - nothing to parse again"
        );
        assert_ne!(
            state.cached_history_lines(40),
            &poison,
            "a resize rewraps every line"
        );

        // and a run gaining an entry is a new history, at any width
        state.history_cache = Some((state.history_cache.take().unwrap().0, poison.clone()));
        let mut run = a_run();
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Ship it?".to_owned(),
            created_at_ms: None,
        });
        state.pipe(PipeMessage::new(
            PipeSource::Plugin(0),
            wire::AGENT_RUN_DETAIL,
            &Some(serde_json::to_string(&run).unwrap()),
            &None,
            false,
        ));
        assert_ne!(state.cached_history_lines(40), &poison);
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
            host: None,
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
                created_at_ms: None,
            }),
            entries: vec![
                agents::RunEntry {
                    revision: 1,
                    event_id: None,
                    kind: agents::RunEntryKind::Prompt,
                    text: "implement history".to_owned(),
                    created_at_ms: None,
                },
                agents::RunEntry {
                    revision: 2,
                    event_id: None,
                    kind: agents::RunEntryKind::Question,
                    text: "Keep old runs?".to_owned(),
                    created_at_ms: None,
                },
                agents::RunEntry {
                    revision: 3,
                    event_id: None,
                    kind: agents::RunEntryKind::Step,
                    text: "Bash cargo test".to_owned(),
                    created_at_ms: None,
                },
                agents::RunEntry {
                    revision: 4,
                    event_id: None,
                    kind: agents::RunEntryKind::Result,
                    text: "Implemented and tested.".to_owned(),
                    created_at_ms: None,
                },
            ],
        }
    }
}
