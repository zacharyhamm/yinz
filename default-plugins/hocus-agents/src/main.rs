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
//! selected agent's idle summary under the list, and walks into a pane on Enter.
//! `AGENT_STRIP_HELLO` is the reload handshake — a freshly loaded instance
//! announces itself and hocus answers with the current rows.
//!
//! **The pane is two permanent halves.** The list is the top one; the events history
//! of whatever the cursor is on is the bottom one, with a divider between them naming
//! it. The history used to be a framed box pinned to the foot of the pane and drawn
//! only when there was something to put in it, which meant the list's height — and so
//! which row a click landed on — moved with what the selected agent happened to have
//! said. The halves are fixed now, and an empty history says it is empty - and says *why*,
//! since loading, a run with no events and a pane too narrow to wrap into are three different
//! things to do about it. `Tab` moves the keyboard between them and so does a click; the
//! divider is where the focus shows, coloured and marked `▸`, because the list's own
//! dim-against-colour already means something else. Only while the keys are really here: the
//! host giving them to another pane, or the panel's tab bar taking them, draws every marker
//! unfocused, while the stop itself is kept for the keys to come back to.
//!
//! **The two halves sit in a ring that goes on into the conversation.** `Tab` and `Shift+Tab`
//! walk five stops, not two: the panel's own tab bar (Agents, Todos, VMs, Swamp), this pane's
//! list and history halves, then the conversation's tab bar and its input box, and forward
//! from the box wraps back round onto the panel's tab bar. That bar is the one keyboard route
//! between the four tabs - `←`/`→` switch, `Enter`, `↓`, `j`, `Tab` and `Esc` drop into the
//! tab's list, and any other key goes to the list too, doing there what it always does. The
//! two crossings that leave this pane are requests - `wire::AGENT_STRIP_YIELD`, the same shape
//! as the Esc and Ctrl-o ones, because hocus holds this pane's plugin id and this pane holds
//! none of hocus's - and the ones that arrive land through `wire::AGENT_STRIP_TAKE`, which
//! names the stop but does not move the keyboard; hocus has already done that. The history
//! half is a stop only when it exists: an empty list, or a pane too short to split, has no
//! second half, and forward from the list goes straight on to the conversation.
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
//! with the keys in this half, `j`/`k` and the arrows walk an event at a time rather than a line
//! at a time, and `Enter` gives the event the whole pane and a scroll of its own until `Esc`. The
//! wheel and the page keys still move by lines, which is what reading in place wants, and the
//! cursor comes along with the window rather than the window snapping back to the cursor. Off
//! the tail, both hold their event while a working agent adds more below them. The idle summary
//! hocus pushes is not capped — there is one of it and nothing beside it competing for the room.
//!
//! **A prompt is not always something a person typed.** Claude Code wakes an agent whose
//! background task has finished by submitting a `<task-notification>` on its behalf, staples a
//! `<system-reminder>` onto real prompts, and spells a slash command as three elements. Drawn
//! as they arrive those spend all four rows on opening tags and a temp path. `hocus_proto::
//! envelope` reads such a prompt apart and the timeline draws that reading — status, short task
//! id, summary, and a row saying what is folded away — while the entry itself keeps the prompt
//! byte for byte. In the reader those parts become folds: `j`/`k` walk them the way they walk
//! events in the half below, `Enter` opens the one under the cursor, `o` opens the file it
//! names, `y` copies it, and `v` swaps the lot for the source - not `r`, which one `Esc` later
//! arms a resume on the same run, with `y` a keystroke from answering it. `o` works in the
//! history half too, on the event under the cursor, because a collapsed row has a cursor on an
//! event and none within one. It puts the panel away and opens the file focused in the floating
//! layer, since an editor left under an overlay is an editor nobody can see; where there is no
//! file to open, it says so rather than doing nothing.
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
//! draws it dim, marked `·` rather than `>`, which is the difference between
//! "you are here" and "here is where you were" - in a glyph as well as a colour,
//! for a terminal that draws neither dim nor colour.
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
//! **`r` resumes the archived run under the cursor**, behind a `y`/`n` the footer asks - the
//! footer rather than only the divider, because a divider cut to a narrow half loses its tail,
//! and a question's tail is its answer keys. A run
//! in the archive is named for the agent's own session id — `AgentRun.id` is
//! `"{kind}:{session id}"` — which is exactly what `claude --resume` and `codex resume` take,
//! so the one thing missing was a key. It asks first because it is the only key here that
//! starts a process, and it sits next to the ones the list is walked with. Archived rows
//! only: a live agent already has a pane, and a second process on its session id would be two
//! agents writing one conversation. Like everything else in this pane it names the row and no
//! more (`wire::AGENT_RESUME`) — which agent, which folder and which machine are hocus's to
//! know. Unlike everything else it is told when it is refused
//! (`wire::AGENT_RESUME_RESULT`), because an answered confirmation that does nothing at all
//! is worse than never having asked. One that works puts the panel away, the way `Enter` on an
//! agent does: hocus has walked the keyboard into the resumed pane, and an overlay left up over
//! it would take the next keys meant for the panel into the agent underneath. A resumed agent
//! keeps its session id and its hooks, so the daemon adopts the same run back: the row leaves
//! the archive rather than a second one appearing beside it.
//!
//! **`?` opens the key sheet** - every key the surface is answering right now, one to a row,
//! over the surface it describes. Each hint line is fitted to its row by dropping whole keys
//! from the end, and ends in `? keys` where the sheet has the rest of them: the Agents footer
//! gives up a trailing key or two to make room for it, but never its first key or the row's
//! own verb, and a line where `?` would type a question mark or shut the sheet carries no
//! marker at all. `Esc`, `?` or `q` shut it; `Tab`, `Shift+Tab` and Ctrl-o work through it;
//! any other key shuts it and then does what it always does. Not while a box is being typed
//! in, where `?` is a character. Every armed question stays armed under the sheet, which lists
//! its answer keys - the VMs tab's close or sync question too, whose list goes on waiting for
//! the answer under it exactly as it would without the sheet.
//!
//! **The two panes are separate plugins with one visibility.** The panel is one
//! thing to the user, so Esc and Enter here put the whole of it away. Esc steps back
//! out of whatever it is in first - the reader, a question, a search, the
//! previous-agents or waiting view, the tab bar, the key sheet - and only then does
//! it hide the panel: this pane hides itself and hocus is asked for the other half
//! (`wire::AGENT_STRIP_HIDE`), being the only one that knows both plugin ids.
//! `Ctrl e a` brings the whole panel back with the keyboard here, `Ctrl e c`
//! with the keyboard in the conversation; neither ever leaves one half alone.
//! Ctrl-o - `Ctrl e o` in the default keymap, which spends a bare Ctrl-o on
//! Session mode before any plugin hears it - cycles the panel, both halves and
//! the split intact, one corner clockwise; hocus does the arithmetic, since it
//! places both overlays, so that key is a request (`wire::AGENT_STRIP_REPOSITION`)
//! rather than a move.
//! Enter walks you to the selected agent, and puts the panel away on the way:
//! the panel is an overlay drawn over the pane you asked for, so leaving it up
//! would hand you the keyboard in a pane you cannot see. Esc at the top level is
//! the same dismissal without the walk. `say_hello_to_hocus` carries hocus's url,
//! because this pane is a
//! view over rows only hocus can compute and a list with no hocus to ask has
//! nothing to draw; the hocus it launches appears beside us but does **not**
//! take the focus - `wire::OPENED_BY` is what tells it so - because the user
//! asked for a list.

#[cfg(test)]
#[path = "unit/attention.rs"]
mod attention_tests;
mod swamp;
#[cfg(test)]
#[path = "../../hocus-todos/src/todos.rs"]
mod todos;
mod vms;
use std::collections::BTreeMap;
use std::sync::Arc;

use hocus_proto::agents::{self, now_ms, Strip, StripRow, Summaries};
use hocus_proto::envelope;
use hocus_proto::launch;
use hocus_proto::markdown;
use hocus_proto::panel_text::{divider_line, scroll_readout, wrap_words};
use hocus_proto::tabs::{self, TabCell};
use hocus_proto::wire;
use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    native_tabs: bool,
    mount_pending: bool,
    pane_tabs: Option<PaneTabsId>,
    own_plugin_id: PluginId,
    own_client_id: ClientId,
    initial_cwd: std::path::PathBuf,
    bar_focused: Vec<ClientId>,
    surface: Surface,
    tabs_focused: bool,
    tab_cells: Vec<TabCell<Surface>>,
    age_minute: i64,
    vms: vms::View,
    #[cfg(test)]
    todos: todos::View,
    swamp: swamp::View,
    /// The strip as hocus last sent it.
    rows: Vec<StripRow>,
    /// What the daemon's model last made of each idle agent, as hocus last sent it.
    ///
    /// The whole map rather than only the selected row's: the selection moves here, under `j`/`k`,
    /// and asking hocus for the box on every keystroke would put a round trip between the cursor
    /// and what it is pointing at.
    summaries: Summaries,
    /// Full history for the selected row, fetched on demand by hocus, held as its parse. See
    /// [`LoadedRun`].
    run: Option<LoadedRun>,
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
    /// Counted from the newest so that at the tail the cursor follows new events, which is what
    /// the tail is for. Anywhere else a count from the newest moves as the run grows, so the
    /// cursor also holds the event itself (`history_cursor_anchor`) and is put back on it when
    /// the run is drawn again. Drawn while the keys are in the half, and dimly beside a search
    /// hit, so the list can show which event matched.
    history_cursor: usize,
    /// The event under `history_cursor`, by `HistoryEvent::revision`, while the cursor is off
    /// the newest one: what a run growing under it must not move it off. `None` at the tail,
    /// which follows new events instead.
    history_cursor_anchor: Option<Option<u64>>,
    /// The event the bottom row of a wound-back window belongs to, and how far into it that row
    /// is. `history_scroll` counts from the newest end, so a run growing under a window the user
    /// wound back used to slide the text being read up by however much arrived; the window is
    /// put back on this row instead. `None` at the tail, which follows.
    history_bottom_anchor: Option<(Option<u64>, usize)>,
    /// The page keys or the wheel moved the history window since a key last moved the cursor.
    ///
    /// Which of the two gives way when they part: after a scroll the cursor comes to the window,
    /// and after anything else - a key on the cursor, a resize, a run growing - the window goes
    /// to the cursor. A window wound back to the cursor on every frame used to undo the page
    /// keys and the wheel before they were drawn, which is what made them look dead there.
    /// Settled in `render_history` for `Reading::reveal`'s reason: only it knows the width.
    history_scrolled: bool,
    /// The event being read in full, if any - the whole pane becomes it.
    ///
    /// Holds what the event *said* rather than a pointer into the timeline: the run behind it
    /// goes on growing while it is open, and a reader that re-resolved its event every frame
    /// would be reading a moving target - the entry under a given index is not the entry that
    /// was there when Enter was pressed.
    reading: Option<Reading>,
    /// The last wrapped body of `reading`, and the `(id, fold, width)` it was wrapped for.
    ///
    /// The same bargain `history_cache` strikes, for the same reason: the reader redraws on
    /// every keystroke and wrapping it is a markdown parse of the whole event, which is much
    /// the most expensive thing this pane does. `fold` is in the key for the same reason the
    /// width is — opening a section or showing the source is a different body, not a different
    /// view of one.
    reader_cache: Option<((u64, u64, usize), envelope::ReaderLines)>,
    /// The parse behind `reader_cache` for prose or raw source, keyed on the opening it was
    /// made for. Held apart because it depends on nothing but `Reading::text`, which an opening
    /// never changes: a resize, a fold or `v` with the reader open re-wraps an event rather
    /// than parsing it again.
    ///
    /// Shared: an event the timeline parsed whole arrives with the timeline's parse
    /// (`agents::ParsedRun::whole_blocks`), so opening it parses nothing at all.
    reader_blocks: Option<(u64, Arc<[markdown::MdBlock]>)>,
    /// The same for the folds, keyed on the opening too: each section's parse, beside the
    /// `open` it was made under. Opening or shutting a fold is the only thing that changes a
    /// section, so a resize re-wraps them all and a fold toggled parses that fold alone.
    reader_folds: Option<(u64, Vec<ParsedFold>)>,
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
    /// the user is reading the history must not throw them back into the list. Put back on the
    /// list when the panel is put away (`end_visit`), and by nothing short of that.
    focus: Focus,
    /// Whether the host last reported this pane as the one with the keyboard
    /// (`PaneInfo::is_focused`), or `None` before any manifest has named it.
    ///
    /// `None` is read as "yes": a pane that has been told nothing either way draws its focus
    /// markers as it always did, and the first manifest to arrive is not worth a redraw on its
    /// own (see `own_pane_changed`). Only what is *drawn* reads this - `focus` and
    /// `tabs_focused` stay where they are when the keyboard leaves, so that coming back lands
    /// where the user left - which is what [`State::keys_here`] is for.
    has_keyboard: Option<bool>,
    /// Whether the host last reported this pane shown, which is the edge `own_pane_changed`
    /// ends a visit on. `hide_the_panel` clears it too, having ended the visit itself.
    pane_shown: bool,
    /// How many rows the last Agents render gave the label naming a filtered view, above the
    /// list: `1` in a search, the previous-agents view or the waiting view, `0` otherwise - and
    /// a row more for each line of a failed search's reason under its box
    /// (`search_failure_lines`). A click is mapped through it onto the list, the same way
    /// `last_window_start` maps it onto a row.
    last_label_rows: usize,
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
    /// Every agent pane this pane has asked the host to focus, in order.
    ///
    /// Test-only, for `ring_yields`' reason: `focus_terminal_pane` is a no-op off wasm, so
    /// nothing else can tell a click that only moved the cursor from one that navigated.
    #[cfg(test)]
    activations: Vec<u32>,
    /// How many times this pane has put the panel away.
    ///
    /// Test-only, for `ring_yields`' reason: both halves of `hide_the_panel` are no-ops off
    /// wasm, so nothing else can tell an Enter that dismissed the panel from one that did not.
    #[cfg(test)]
    panel_hides: usize,
    /// Every file `o` has opened, in order. Test-only: the host call waits on a reply, which a
    /// native test cannot give it.
    #[cfg(test)]
    opened_files: Vec<String>,
    /// How many times this pane has asked hocus for older runs. Test-only, for `ring_yields`'
    /// reason.
    #[cfg(test)]
    archive_requests: usize,
    /// Every New agent modal this pane has launched, as the configuration it was launched with.
    /// Test-only, for `ring_yields`' reason.
    #[cfg(test)]
    new_agent_launches: Vec<BTreeMap<String, String>>,
    /// What the last `o` or `y` had to say, until the next key: why `o` opened nothing, or
    /// what `y` put on the clipboard. A key that silently does nothing reads as a key that is
    /// not bound, and a copy that says nothing leaves the user to guess which of two things it
    /// took.
    open_note: Option<String>,
    /// Whether any strip has arrived from hocus yet. Before one has, the list is empty because
    /// nobody has said what is in it - which is not the same thing as a session with no agents,
    /// and must not be drawn as one.
    strip_seen: bool,
    /// Why hocus could not fetch the history of the row it names, as a `selection_key`.
    run_error: Option<(String, String)>,
    /// The key sheet `?` opened over the surface, as how far it is scrolled, or `None` while it
    /// is shut.
    key_sheet_scroll: Option<usize>,
    /// How many rows the last key-sheet render had to show it in, and the furthest it could be
    /// scrolled - only `render` knows the pane's height.
    last_sheet_rows: usize,
    last_sheet_max_scroll: usize,
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
    /// Manual wheel position; None keeps the list centered on the selection.
    list_scroll: Option<usize>,
    /// A second click's press, waiting for its release on the same cell to do what `Enter`
    /// does there. See `on_click`.
    click_press: Option<ClickPress>,
    /// Latest pane-relative pointer position, including the negative hover-leave sentinel.
    mouse_position: Option<(isize, usize)>,
    last_rows: usize,
    last_cols: usize,
    last_footer_row: Option<usize>,
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
    /// Where the last history render's window began, as an index into its entries, and how
    /// many rows under the divider it gave to what does not scroll - the pinned header and a
    /// refusal - before the window. What maps a click in the half back onto an event.
    last_history_window_start: usize,
    last_history_body_top: usize,
    /// The largest `history_scroll` the last render could actually honour.
    ///
    /// Kept for the same reason `last_window_start` is: how many lines the history has
    /// depends on the width it is wrapped to, and only `render` is told the width. Without
    /// it a wheel against the top of the history would keep counting, and scrolling back down
    /// would cost as many notches as were wasted going up.
    last_history_max_scroll: usize,
    /// Bumped whenever anything the history half is drawn *from* changes, other than the
    /// loaded run - a strip carrying fresh summaries, a search, a list move. Counted rather
    /// than compared field by field: what `history_lines` reads is spread across several of
    /// them, and a counter cannot be half updated by someone adding another.
    generation: u64,
    /// Bumped whenever `run` is loaded with a run that changed, and by nothing else.
    ///
    /// Apart from `generation` because a run's timeline is drawn from the run alone, and
    /// `generation` moves on every strip push that changes anything on any pane. Keyed on
    /// that, the history half re-compared every entry of a long run and re-assembled its
    /// timeline each time another agent's summary changed.
    run_revision: u64,
    /// The last `history_lines`, and what it was computed from. See `cached_history_lines`.
    /// A miss on a run is rebuilt from `LoadedRun::parsed`, which only re-wraps.
    history_cache: Option<(HistoryKey, agents::HistoryLines)>,
    /// The row an `s` has asked the daemon about and not yet had an answer for.
    ///
    /// A hint on the divider rather than state anything is decided from. The round trip is a
    /// model's, so it is seconds long, and a key that reaches the network and shows nothing in
    /// the meantime reads as a key that did nothing. Held as the row's key rather than a flag
    /// so that walking off the row takes it with you. Nothing here ever hears that a request
    /// was refused - a refusal in the daemon is silent - so it ends on a summary stamped after
    /// the press, the only thing that answers it, and says so once it has waited longer than
    /// the daemon ever takes: a hint that ended on any change to the run read as "done" when
    /// the run had only moved on.
    summarizing: Option<SummaryAsk>,
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
    search_revisions: BTreeMap<String, u64>,
    // Consumed on the first render that can reveal the hit; refreshes retain manual scrolling.
    pending_history_revision: Option<u64>,
    predecessors: Option<Predecessors>,
    predecessor_request: u64,
    search_editing: bool,
    search_pending: bool,
    /// When a search sent and not yet answered stops being worth waiting for. hocus answers
    /// synchronously, so a search still out long after it was sent was dropped - or went to a
    /// hocus that is not there - and a box that goes on looking the same says neither.
    search_deadline_ms: i64,
    search_query: String,
    /// What was typed in the search box when `Tab` or `Shift+Tab` took the keys out of it, for
    /// the next `/` to bring back. The box cannot stay open behind the keys the way the Todos
    /// box does: it takes every key ahead of the tab bar, so the ring would never get past it.
    search_draft: Option<String>,
    applied_search_query: Option<String>,
    /// When the applied search ran. Its hits are not run again, so after a long hide the view
    /// says how old it is rather than passing for a list of what matches now.
    searched_at_ms: Option<i64>,
    search_request: u64,
    search_error: Option<String>,
    search_truncated: bool,
    search_base: Option<(Vec<StripRow>, Summaries)>,
    /// Latest unfiltered strip while the list shows only agents waiting for an answer.
    waiting_base: Option<Strip>,
}

/// Everything [`State::history_lines`] reads, as one comparable value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryKey {
    /// The row under the cursor, which is what the half is about.
    selection: Option<String>,
    /// The half's width, which is what its lines are wrapped to.
    width: usize,
    /// The state behind the selection.
    source: HistorySource,
    /// The minute the cache was filled in, `now_ms() / 60_000`. Nothing else in the key
    /// changes with the passage of time alone, so without this the "ago" label on each
    /// history row would freeze at whatever it read on the last selection change - this
    /// bounds the staleness to about a minute on any render that already happens to occur.
    minute: i64,
}

/// One fold of the reader, parsed (`envelope::section_blocks`), beside whether it was open
/// then. See `State::reader_folds`.
type ParsedFold = (bool, Vec<markdown::MdBlock>);

/// The selected run, as the pane keeps it: parsed, and the few facts about it the pane reads.
///
/// Not the decoded [`agents::AgentRun`]. A long run is a quarter of a megabyte of text, and
/// the parse keeps every drawn event's text for the reader already; holding the decoded run
/// and the JSON it came as beside it put the same run in the heap three times over. The run
/// is parsed when it arrives and dropped, so what is left is one copy of its text.
///
/// The parse goes with the run: a long run's parse is megabytes, kept for redrawing that run,
/// not for the pane's lifetime. It stays while the cursor merely leaves the run's row - `j`
/// then `k` would parse the whole run again - and it is handed on when another run replaces
/// this one, which `ParsedRun::update` turns over itself.
struct LoadedRun {
    id: String,
    pane_id: u32,
    /// Whether the run has an `ended_at_ms`: a live agent's row only shows its live run.
    ended: bool,
    /// The machine the run's folder is on - see `open_target`.
    host: Option<String>,
    cwd: Option<String>,
    /// Whether any entry, or the rolling summary, carries a stamp - `shows_an_age`'s answer
    /// for a run, worked out once rather than on every tick.
    stamped: bool,
    /// The JSON the run came as, as [`agents::run_payload_fingerprint`] reduces it.
    ///
    /// hocus sends the selected run again each time it is asked and each time it grows, and a
    /// long run is a quarter of a megabyte of it. Compared before anything is decoded, so a run
    /// sent again unchanged costs one hash rather than a decode - without keeping the bytes to
    /// compare against.
    fingerprint: (usize, u64),
    /// The run, parsed and wrapped at the last width asked for - what a `history_cache` miss
    /// is rebuilt from.
    ///
    /// The cache misses on every step of a resize and on every minute, and hocus sends the
    /// selected run again each time it grows, which is several times a second while its agent
    /// works. Each of those used to parse the whole run's markdown again; this keeps the parse
    /// per event, so a miss costs a re-wrap and a grown run costs its new entries.
    parsed: agents::ParsedRun,
}

/// What the history half is drawn from, and the counter that moves when it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistorySource {
    /// The loaded run, which is all a run's timeline reads: [`State::run_revision`].
    Run(u64),
    /// The strip - the summary hocus pushes for the row, and the row itself:
    /// [`State::generation`].
    Strip(u64),
}

/// One event of the timeline, filling the pane so it can be read whole.
///
/// The history half caps every event at [`agents::MAX_ENTRY_ROWS`] rows, which is only honest
/// because this exists: `Enter` on the cursor opens the event here, with the whole pane and a
/// scroll of its own to spend on it.
struct Reading {
    /// What the divider calls it - `prompt`, `result`, `summary`.
    kind: &'static str,
    /// When the event was recorded, or `None` for one made before there were stamps. Kept
    /// rather than the formatted age so the divider goes on ticking with the minute timer.
    created_at_ms: Option<i64>,
    /// The whole of what the event said, unwrapped. The event's own, shared rather than
    /// copied: a long one is hundreds of kilobytes, and nothing here ever edits it.
    text: Arc<str>,
    /// Lines scrolled from the top. The reader is anchored to the head rather than the tail:
    /// the history half is a timeline, where the newest end is the interesting one, and an
    /// event is a piece of prose, where the beginning is.
    scroll: usize,
    /// Which opening this is. See `State::reading_id`.
    id: u64,
    /// The folds this event breaks into, when it is a prompt carrying a hook envelope rather
    /// than prose. Empty for every other kind and for most prompts, which is what makes this
    /// reader two readers in one: a folding one, and the line-scrolling one it has always been.
    ///
    /// Computed on open rather than carried on `agents::HistoryEvent`: an event is built for
    /// every row of every run on every history render, and this is work only for the events
    /// somebody actually presses Enter on.
    sections: Vec<envelope::Section>,
    /// Show the event exactly as it arrived, tags and all — what `v` toggles. The folds are the
    /// reading; this is the record, and a reader that could not get back to it would be asking
    /// to be trusted.
    raw: bool,
    /// Which fold the keys act on. Meaningless, and undrawn, without `sections`.
    cursor: usize,
    /// Bumped by every fold and by the raw toggle. Part of `State::reader_cache`'s key.
    fold: u64,
    /// A key moved the cursor or changed the folds, and the rows it now owns have to be wound
    /// on screen. Deferred to the next render rather than done where it is asked for, because
    /// only `render` knows the width and so only `render` knows which row a fold begins on —
    /// and because a fold that has just opened is measured on rows the cache does not hold
    /// yet.
    reveal: bool,
    /// The envelope's own tag, where the event turned out to be one. On the divider beside the
    /// kind, because `prompt` alone does not say which of the shapes a harness writes this is —
    /// and for an envelope nobody registered, the tag is the only name it has.
    envelope: Option<String>,
}

/// A press that `State::on_click` found to be a second click, and what its release does.
///
/// Done on the release rather than the press, because the pane selects text with the mouse: a
/// press on the row or event already chosen is also how a drag that copies its text starts,
/// and doing the Enter on the press walked out of the panel, or opened the reader, under the
/// drag. A release on the cell that was pressed is a click; one anywhere else ends a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClickPress {
    /// Where it was pressed, in the content coordinates `State::content_mouse` sees.
    line: isize,
    column: usize,
    act: ClickAct,
}

/// What a second click does once it is released: see [`ClickPress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClickAct {
    /// Walk into the agent on this row of `rows`.
    Activate(usize),
    /// Read this event of the history half, counted as `history_cursor` counts it.
    Read(usize),
}

/// An `s` waiting on its answer. See `State::summarizing`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SummaryAsk {
    /// The row asked about, as a `selection_key`.
    row: String,
    /// When `s` was pressed: a summary stamped at or after this is the answer. The daemon
    /// stamps with the same clock, on the same machine.
    asked_at_ms: i64,
    /// Waited longer than the daemon ever takes: nothing is coming, or what came said what the
    /// last summary said.
    stale: bool,
}

/// How long an `s` is worth waiting on before the divider stops saying it is being worked on.
/// A little over the daemon's own in-flight window (twelve heartbeats, about a minute), so a
/// retry it offers is never one the daemon would still refuse as already asked.
const SUMMARY_WAIT_MS: i64 = 70_000;

/// The foot of every key sheet: the keys the whole panel answers, whichever tab or stop has
/// them, and which no tab's own sheet lists. The move is named as the default keymap spells
/// it - a bare Ctrl-o is Session mode there, and reaches the panel only while Locked.
const PANEL_KEYS: &[(&str, &str)] = &[
    ("Ctrl e o", "move the panel (Ctrl-o when locked)"),
    ("?", "shut this sheet"),
];

/// How long a search is worth waiting on. hocus answers from its store synchronously, so this
/// is long enough for a slow store and short enough that a dropped request is noticed.
const SEARCH_TIMEOUT_MS: i64 = 15_000;

/// Which half of the pane the keys work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    List,
    History,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Surface {
    #[default]
    Agents,
    Todos,
    Vms,
    Swamp,
}

impl Surface {
    fn subscriptions(self) -> Vec<EventType> {
        let mut events = vec![
            EventType::Key,
            EventType::Mouse,
            EventType::PastedText,
            EventType::PaneTabsUpdate,
            EventType::Timer,
            EventType::Visible,
            EventType::PaneUpdate,
        ];
        match self {
            Surface::Vms => events.push(EventType::SessionUpdate),
            Surface::Swamp => events.extend([
                EventType::PermissionRequestResult,
                EventType::RunCommandResult,
                EventType::WebRequestResult,
            ]),
            Surface::Agents | Surface::Todos => {},
        }
        events
    }

    fn step(self, delta: isize) -> Self {
        let index: usize = match self {
            Self::Agents => 0,
            Self::Todos => 1,
            Self::Vms => 2,
            Self::Swamp => 3,
        };
        match index.saturating_add_signed(delta).min(3) {
            0 => Self::Agents,
            1 => Self::Todos,
            2 => Self::Vms,
            _ => Self::Swamp,
        }
    }
}

/// VM polling cadence. Agent ages redraw only when their minute changes.
const POLL_TICK_SECS: f64 = 2.0;

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.native_tabs = true;
        self.surface = match configuration.get("view").map(String::as_str) {
            Some("vms") => Surface::Vms,
            Some("swamp") => Surface::Swamp,
            _ => Surface::Agents,
        };
        subscribe(&self.surface.subscriptions());
        set_timeout(zellij_tile::prelude::FractionalSeconds::new(POLL_TICK_SECS));
        set_selectable(true);
        set_self_mouse_selection_support(true);
        let ids = get_plugin_ids();
        self.own_plugin_id = ids.plugin_id;
        self.own_client_id = ids.client_id;
        self.initial_cwd = ids.initial_cwd;
        rename_plugin_pane(
            self.own_plugin_id,
            match self.surface {
                Surface::Vms => "VMs",
                Surface::Swamp => "Swamp",
                _ => "Agents",
            },
        );
        if !configuration.contains_key("view") {
            self.mount_pending = true;
            self.mount_composition();
        }
        if self.surface == Surface::Swamp {
            self.activate_swamp();
        }
        // Unconditional, and before there are any rows: what this first one carries
        // is the plugin id, which is how hocus tells "the agents pane is focused"
        // from "some other plugin pane is". The pane it names follows with the
        // strip, a moment later.
        if self.surface == Surface::Agents {
            self.send_selection();
            self.say_hello_to_hocus();
        }
    }

    fn update(&mut self, event: Event) -> bool {
        let is_key = matches!(event, Event::Key(_));
        let is_input = matches!(
            event,
            Event::Key(_) | Event::Mouse(_) | Event::PastedText(_)
        );
        if is_input && self.native_tabs {
            #[cfg(not(test))]
            let client = {
                let client = get_plugin_ids().client_id;
                self.own_client_id = client;
                client
            };
            #[cfg(test)]
            let client = self.own_client_id;
            self.tabs_focused = self.bar_focused.contains(&client);
        }
        let had_bar = self.tabs_focused;
        let selection = self.selection_key();
        let revision = self.selected_revision();
        let rendered = match event {
            Event::RunCommandResult(code, stdout, stderr, context) => {
                self.swamp_result(code, &stdout, &stderr, &context)
            },
            Event::WebRequestResult(status, headers, body, context) => {
                self.swamp_web_result(status, &headers, &body, &context)
            },
            Event::PermissionRequestResult(status) => self.swamp_permission(status),
            Event::Visible(visible) => {
                if !visible {
                    self.end_visit();
                }
                if self.surface == Surface::Vms {
                    self.vms.visible = visible;
                }
                self.surface == Surface::Swamp && self.swamp_visibility(visible)
            },
            Event::PaneTabsUpdate(tabs) => {
                if let Some(tabs) = tabs.iter().find(|tabs| {
                    tabs.tabs
                        .iter()
                        .any(|slot| slot.plugin_id == Some(self.own_plugin_id))
                }) {
                    self.pane_tabs = Some(tabs.id);
                    self.bar_focused = tabs.bar_focused.clone();
                    self.tabs_focused = self.bar_focused.contains(&self.own_client_id);
                } else {
                    self.pane_tabs = None;
                    self.bar_focused.clear();
                    self.tabs_focused = false;
                }
                false
            },
            Event::PaneUpdate(manifest) => {
                // Read here rather than in `swamp_panes`, which looks the pane up for its own
                // polling: whether this pane is shown and has the keys is the whole panel's
                // business, not one tab's. First, so a hide ends the visit before Swamp
                // reacts to it.
                let keys = own_pane(&manifest, self.own_plugin_id).is_some_and(|pane| {
                    self.own_pane_changed(!pane.is_suppressed, pane.is_focused)
                });
                self.pane_tabs = own_pane(&manifest, self.own_plugin_id)
                    .and_then(|pane| pane.pane_tabs.as_ref().map(|tabs| tabs.id))
                    .or(self.pane_tabs);
                if self.mount_pending && own_pane(&manifest, self.own_plugin_id).is_some() {
                    if self.pane_tabs.is_none() {
                        self.mount_composition();
                    } else {
                        self.mount_pending = false;
                    }
                }
                if self.native_tabs && self.surface == Surface::Vms {
                    self.vms.visible = own_pane(&manifest, self.own_plugin_id)
                        .is_some_and(|pane| !pane.is_suppressed);
                }
                (self.surface == Surface::Swamp && self.swamp_panes(&manifest)) || keys
            },
            Event::SessionUpdate(sessions, _) => self.vm_sessions(&sessions),
            #[cfg(test)]
            Event::PluginDataChanged { watch_id, rows, .. } => {
                self.todo_rows_changed(watch_id, rows)
            },
            Event::Key(key) => self.on_key(key),
            Event::PastedText(text) => self.on_paste(&text),
            Event::Mouse(mouse_event) => self.on_mouse(if self.native_tabs {
                match mouse_event {
                    Mouse::LeftClick(y, x) => Mouse::LeftClick(y + 1, x),
                    Mouse::RightClick(y, x) => Mouse::RightClick(y + 1, x),
                    Mouse::Hold(y, x) => Mouse::Hold(y + 1, x),
                    Mouse::Release(y, x) => Mouse::Release(y + 1, x),
                    Mouse::Hover(y, x) => Mouse::Hover(y + 1, x),
                    other => other,
                }
            } else {
                mouse_event
            }),
            // The ages in the history half are drawn from the clock, so nothing the pane
            // receives can tell it they have moved on. Re-armed whatever the answer, so the
            // column keeps ticking; a redraw only where an age is actually on screen, so a
            // pane showing nothing time-dependent still costs nothing to leave open.
            Event::Timer(_) => {
                set_timeout(zellij_tile::prelude::FractionalSeconds::new(POLL_TICK_SECS));
                let now = now_ms();
                let vm_changed = self.surface == Surface::Vms && self.poll_vms();
                // The other requests here with a deadline: a triage handed to hocus that was
                // never acknowledged, a search nobody answered and a summary nobody wrote all
                // stop claiming to be on their way.
                let triage_expired =
                    self.surface == Surface::Swamp && self.expire_swamp_triage(now);
                let search_expired = self.expire_search(now);
                let summary_expired = self.expire_summary_ask(now);
                let minute = now / 60_000;
                let changed = minute != self.age_minute;
                self.age_minute = minute;
                vm_changed
                    || triage_expired
                    || search_expired
                    || summary_expired
                    || (changed && self.shows_an_age())
            },
            _ => false,
        };
        // Where the cursor moved to is hocus's business, not only this pane's: it
        // draws the selected agent's summary. Compared rather than sent on every
        // key, so holding `j` at the end of the list says nothing.
        let moved = self.follow_selection(selection);
        if !moved && revision != self.selected_revision() {
            self.leave_row();
            self.pending_history_revision = self.selected_revision();
        }
        if self.surface == Surface::Agents && (moved || revision != self.selected_revision()) {
            self.send_selection();
        }
        if self.native_tabs && is_key && self.tabs_focused && !had_bar {
            if let Some(id) = self.pane_tabs {
                let _ = focus_pane_tabs(id, true);
            }
        }
        if self.native_tabs && is_key {
            self.bar_focused
                .retain(|client| *client != self.own_client_id);
            if self.tabs_focused {
                self.bar_focused.push(self.own_client_id);
            }
        }
        rendered
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::ASK_ACK {
            return self.swamp_ack(&pipe_message);
        }
        if let Some(changed) = self.vm_pipe(&pipe_message) {
            return changed;
        }
        if pipe_message.name == wire::AGENT_STRIP_OPEN {
            // hocus asking for this pane. If it is reaching us at all then we already exist and
            // the message opened nothing - but a hocus that reloaded has forgotten our id, and
            // `send_selection` on load is the only other thing that ever tells it. Answering here
            // is what stops a reloaded hocus being unable to place or hide the list beside it.
            if self.surface == Surface::Agents {
                self.send_selection();
            }
            return false;
        }
        if pipe_message.name == wire::AGENT_STRIP_TAKE {
            // The ring stepped into this pane from the conversation. hocus holds our plugin id
            // and has already moved the keyboard; all this carries is which of our two halves
            // the arriving keys belong to.
            return self
                .take_ring_stop(pipe_message.args.get(wire::PANEL_STOP).map(String::as_str));
        }
        if pipe_message.name == wire::AGENT_PREDECESSORS_RESULT {
            let Some(result) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::PredecessorsResult>(json).ok())
            else {
                return false;
            };
            let changed = self.apply_predecessors(result);
            if changed {
                self.send_selection();
            }
            return changed;
        }
        if pipe_message.name == wire::AGENT_SEARCH_RESULT {
            let Some(result) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::SearchResult>(json).ok())
            else {
                return false;
            };
            let succeeded = result.error.is_empty();
            let changed = self.apply_search_result(result);
            if changed && succeeded {
                self.send_selection();
            }
            return changed;
        }
        if pipe_message.name == wire::AGENT_RESUME_RESULT {
            let Some(result) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::ResumeResult>(json).ok())
            else {
                return false;
            };
            // Empty detail is success, and success puts the panel away wherever the cursor
            // is now. hocus has already walked the keyboard into the resumed pane, and an
            // overlay left drawn over it is the "keyboard in a pane you cannot see" that
            // `activate` hides for: the next `j`, `k` or `Esc` meant for the list would go to
            // the agent underneath. The hide comes after the focus moved, which is safe - the
            // overlay holds no keyboard by then to drop.
            if result.detail.is_empty() {
                self.resume_note = None;
                self.hide_the_panel();
                return true;
            }
            // A refusal is named by the run rather than by the cursor, and dropped when the two
            // disagree: the answer is about the row that was asked about, and a user who walked
            // on while it was out has ended the question themselves - the same lifetime every
            // other hint on this divider has. Keeping it would mean a stale refusal waiting to
            // reappear the next time the cursor came back to the row.
            let row = format!("run:{}", result.run_id);
            if self.selection_key() != Some(row.clone()) {
                return false;
            }
            let note = Some((row, result.detail));
            let changed = self.resume_note != note;
            self.resume_note = note;
            return changed;
        }
        if pipe_message.name == wire::AGENT_RUN_DETAIL {
            let revision = pipe_message
                .args
                .get("revision")
                .and_then(|value| value.parse::<u64>().ok());
            if revision != self.selected_revision() {
                return false;
            }
            let Some(payload) = pipe_message.payload else {
                // No run, and why: hocus could not fetch the selected archived run's history.
                // Said in the half, which would otherwise go on saying it is loading.
                let Some(error) = pipe_message.args.get("error") else {
                    return false;
                };
                // Named by the run it is about, the way a resume refusal is, and dropped when
                // that is not the row under the cursor: a reply to a selection the user has
                // walked off must not say another run's history is unavailable. hocus sends it
                // again on every snapshot, so only a changed reason is worth a redraw.
                let row = pipe_message
                    .args
                    .get("run_id")
                    .map(|run_id| format!("run:{run_id}"));
                if row.is_none() || row != self.selection_key() {
                    return false;
                }
                let note = row.map(|row| (row, error.clone()));
                let changed = self.run_error != note;
                self.run_error = note;
                return changed;
            };
            let fingerprint = agents::run_payload_fingerprint(&payload);
            if self
                .run
                .as_ref()
                .is_some_and(|run| run.fingerprint == fingerprint)
            {
                return false;
            }
            let Ok(run) = serde_json::from_str::<agents::AgentRun>(&payload) else {
                return false;
            };
            // Before the parse rather than after it: at the peak the heap holds the decoded run
            // and its parse, not the JSON as well.
            drop(payload);
            self.load_run(run, fingerprint);
            return true;
        }
        if (self.native_tabs && self.surface != Surface::Agents)
            || pipe_message.name != wire::AGENT_STRIP
        {
            return false;
        }
        let Some(strip) = pipe_message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<Strip>(json).ok())
        else {
            return false;
        };
        // The first strip is news even when it is empty: the list stops saying it is waiting
        // for hocus and starts saying what hocus said.
        let first = !std::mem::replace(&mut self.strip_seen, true);
        // The same comparison, for the other thing that moves the selection: a
        // push whose focus walked onto another agent.
        let selection = self.selection_key();
        let rendered = self.set_rows(strip);
        let answered = self.summary_in_the_strip();
        if self.follow_selection(selection) {
            self.send_selection();
        }
        rendered || first || answered
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if self.native_tabs {
            #[cfg(not(test))]
            let client = get_plugin_ids().client_id;
            #[cfg(test)]
            let client = self.own_client_id;
            self.tabs_focused = self.bar_focused.contains(&client);
        }
        CONTENT_OFFSET.with(|offset| offset.set(usize::from(!self.native_tabs)));
        if self.native_tabs {
            rename_plugin_pane(
                self.own_plugin_id,
                match self.surface {
                    Surface::Vms if self.vms.has_error() => "VMs!",
                    Surface::Vms => "VMs",
                    Surface::Swamp if self.swamp.has_error() => "Swamp!",
                    Surface::Swamp => "Swamp",
                    _ => "Agents",
                },
            );
        }
        self.last_rows = rows;
        self.last_cols = cols;
        self.last_footer_row = None;
        self.last_label_rows = 0;
        // Settled before anything is drawn, because the surface's share of the pane - what is
        // left under the tab bar and above the footer - is what the cursor is placed in.
        let footer = self.footer_hint();
        let body_rows =
            rows.saturating_sub(usize::from(!self.native_tabs) + usize::from(footer.is_some()));
        // Host commands must precede drawing; keep the dropdown output until
        // after the issue list so it paints over the rows beneath it. Nothing of the
        // surface's is drawn under the key sheet, so neither is its dropdown.
        let swamp_form = if self.surface == Surface::Swamp && self.key_sheet_scroll.is_none() {
            self.swamp_filter_buffer(body_rows, cols)
        } else {
            None
        };
        let form_cursor = swamp_form
            .as_ref()
            .and_then(|buffer| buffer.cursor.flatten());
        show_cursor(
            self.text_cursor(form_cursor, body_rows, cols)
                .map(|(x, y)| {
                    (
                        x,
                        if form_cursor.is_some() {
                            y
                        } else {
                            y.saturating_sub(usize::from(self.native_tabs))
                        },
                    )
                }),
        );
        if rows <= 1 || cols == 0 {
            // Forget the hidden geometry, but keep selections and scrolls for when it returns.
            self.last_window_start = 0;
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_body_rows = 0;
            self.last_history_max_scroll = 0;
            self.last_reader_rows = 0;
            self.last_reader_max_scroll = 0;
            self.vms.clear_dimensions();
            #[cfg(test)]
            self.todos.clear_dimensions();
            self.swamp.clear_dimensions();
        }
        if rows == 0 || cols == 0 {
            return;
        }
        // The tab bar is drawn on every surface and in every view. It used to give way to a
        // "Search: …" or "Previous agents: …" label, which took the panel's only map - and its
        // only click targets, and a ring stop - away from all four tabs over a state that
        // belongs to one of them. That label is the Agents surface's own first row now.
        let bar = self.bar_has_keys();
        self.tab_cells = tabs::cells(
            [
                (
                    "Agents".to_owned(),
                    self.surface == Surface::Agents,
                    bar && self.surface == Surface::Agents,
                    Surface::Agents,
                ),
                (
                    "Todos".to_owned(),
                    self.surface == Surface::Todos,
                    bar && self.surface == Surface::Todos,
                    Surface::Todos,
                ),
                (
                    if self.vms.has_error() { "VMs!" } else { "VMs" }.to_owned(),
                    self.surface == Surface::Vms,
                    bar && self.surface == Surface::Vms,
                    Surface::Vms,
                ),
                (
                    if self.swamp.has_error() {
                        "Swamp!"
                    } else {
                        "Swamp"
                    }
                    .to_owned(),
                    self.surface == Surface::Swamp,
                    bar && self.surface == Surface::Swamp,
                    Surface::Swamp,
                ),
            ],
            cols,
        );
        for cell in self.tab_cells.iter().filter(|_| !self.native_tabs) {
            print_text_with_coordinates(cell.styled(), cell.x, 0, Some(cell.width), None);
        }
        if let Some(hint) = footer {
            self.last_footer_row = Some(rows - 1);
            print_text_with_coordinates(
                Text::new(hint).dim_range(..),
                0,
                rows - 1,
                Some(cols),
                Some(1),
            );
        }
        let rows = body_rows;
        if rows == 0 {
            return;
        }
        // Over the surface rather than beside it: it is about the surface, and a pane of these
        // widths has no room to spare for both.
        if self.key_sheet_scroll.is_some() {
            self.render_key_sheet(rows, cols);
            return;
        }
        match self.surface {
            Surface::Swamp => {
                self.render_swamp(rows, cols);
                if let Some(buffer) = swamp_form {
                    print!("{}", buffer.output);
                }
                return;
            },
            #[cfg(test)]
            Surface::Todos => {
                self.render_todos(rows, cols);
                return;
            },
            Surface::Vms => {
                self.render_vms(rows, cols);
                return;
            },
            Surface::Agents => {},
            #[cfg(not(test))]
            Surface::Todos => {},
        }
        // The reader is the whole pane rather than a third region: an event long enough to
        // need it is long enough to want the room, and the list it covers is still exactly
        // where Esc puts it back.
        if self.reading.is_some() {
            self.render_reader(rows, cols);
            return;
        }
        // A filtered view says so above the list it filters - on this surface only, since it
        // is this surface's state - and the list, the divider and the history half all start a
        // row lower for it. Plain rather than dim: it is what the list below *is*, not a hint.
        let top = match self.view_label() {
            Some(label) => {
                paint(Text::new(label), 0, 0, Some(cols), None);
                // Why the search just typed failed, whole, under the box it failed in: the
                // footer is one row and would cut the reason to its first few words, and the
                // reason is what decides between a retry and going back. At most three rows,
                // and never so many that nothing is left of the list under them.
                let failure = self.search_failure_lines(cols);
                let shown = failure.len().min(3).min(rows.saturating_sub(2));
                for (index, line) in failure.into_iter().take(shown).enumerate() {
                    paint(
                        Text::new(line).error_color_range(..),
                        0,
                        1 + index,
                        Some(cols),
                        None,
                    );
                }
                1 + shown
            },
            None => 0,
        };
        self.last_label_rows = top;
        let rows = rows - top;
        if rows == 0 {
            self.last_window_start = 0;
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_body_rows = 0;
            self.last_history_max_scroll = 0;
            self.focus = Focus::List;
            return;
        }
        if self.rows.is_empty() {
            // Nothing to select and so nothing to have a history: the pane says so once
            // rather than splitting an empty list off an empty history.
            self.list_scroll = None;
            self.last_window_start = 0;
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_body_rows = 0;
            // No history half drawn, so the focus cannot be in one - the same correction the
            // too-short pane makes below, and what the Tab handler's ring relies on.
            self.focus = Focus::List;
            self.last_history_max_scroll = 0;
            // What the emptiness is, and the key out of it on a row of its own: the two on one
            // line are wider than the narrowest half, which would cut off the part that says
            // what to do.
            let (message, next) = self.empty_list_message();
            paint(Text::new(message).dim_range(..), 0, top, Some(cols), None);
            if let Some(next) = next.filter(|_| rows >= 2) {
                paint(Text::new(next).dim_range(..), 0, top + 1, Some(cols), None);
            }
            return;
        }
        let (list_rows, history_rows) = split(rows);
        // Keyboard navigation centers the selection; the wheel moves only the viewport.
        let start = self
            .list_scroll
            .unwrap_or_else(|| self.selected.saturating_sub(list_rows / 2))
            .min(self.rows.len().saturating_sub(list_rows));
        if self.list_scroll.is_some() {
            self.list_scroll = Some(start);
        }
        self.last_window_start = start;
        self.last_list_rows = list_rows;
        let live = self.live();
        // Measured over every archived row rather than the visible ones, so a column does not
        // change width under the cursor as the window scrolls past a longer cell. One clock
        // reading for the whole frame, for the same reason: two rows recorded a second apart
        // must not disagree about which of them is `now`.
        let columns = archive_columns(&self.rows, cols);
        let now = now_ms();
        for (index, row) in self.rows.iter().skip(start).take(list_rows).enumerate() {
            let selected = start + index == self.selected;
            let text = list_text(row, selected, live, &columns, now);
            paint(text, 0, top + index, Some(cols), None);
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
        self.render_history(top + list_rows, history_rows, cols);
    }
}

/// How much of a run row the marker and the indent take before any column starts: the `>` the
/// selection is drawn with, and the three spaces that nest the row under the `archive` header.
const RUN_GUTTER: usize = 4;
/// The gap between two of the table's columns. The age column carries its own trailing space
/// (`agents::AGE_WIDTH` counts it), so this is only paid after the kind and the worktree.
const COLUMN_GAP: usize = 2;
/// The widest a kind cell and a worktree cell are allowed to get, however long the longest one
/// in the archive is. A column exists to be read down, and one wide enough for the worst name
/// in the history would spend the pane on a folder nobody is looking for.
const MAX_KIND: usize = 8;
const MAX_WORKTREE: usize = 16;
/// The least room the prompt is worth drawing in. A column narrower than this says nothing
/// about what the run was for, and every other column is furniture around that one.
const MIN_TITLE: usize = 12;
/// What marks a run that continues an earlier one across a context clear.
const CONTINUATION: &str = "↳ ";

/// The widths the archive table's columns get on a pane this wide. A zero-width kind or
/// worktree is a column that is not drawn at all.
///
/// The prompt is the point of the row, so the columns around it are shed to keep it readable:
/// the kind first, because an agent that is nearly always the same one is the most guessable
/// cell on the row, then the worktree, then the age. Nothing wraps - a list is scanned down,
/// and a row that took two of them would break the column it is being scanned in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Columns {
    age: bool,
    kind: usize,
    worktree: usize,
    title: usize,
}

impl Columns {
    /// What everything but the prompt costs at these widths.
    fn furniture(&self) -> usize {
        let cell = |width: usize| match width {
            0 => 0,
            width => width + COLUMN_GAP,
        };
        (if self.age { agents::AGE_WIDTH } else { 0 }) + cell(self.kind) + cell(self.worktree)
    }

    fn shed_kind(&mut self) {
        self.kind = 0;
    }

    fn shed_worktree(&mut self) {
        self.worktree = 0;
    }

    fn shed_age(&mut self) {
        self.age = false;
    }
}

fn archive_columns(rows: &[StripRow], cols: usize) -> Columns {
    let mut columns = Columns::default();
    for row in rows {
        if let StripRow::Run { kind, worktree, .. } = row {
            columns.kind = columns.kind.max(kind.width().min(MAX_KIND));
            columns.worktree = columns.worktree.max(worktree.width().min(MAX_WORKTREE));
        }
    }
    columns.age = true;
    let body = cols.saturating_sub(RUN_GUTTER);
    for shed in [
        Columns::shed_kind,
        Columns::shed_worktree,
        Columns::shed_age,
    ] {
        if body.saturating_sub(columns.furniture()) >= MIN_TITLE {
            break;
        }
        shed(&mut columns);
    }
    columns.title = body.saturating_sub(columns.furniture());
    columns
}

/// One archive row, from the marker column onwards.
fn archive_row(
    columns: &Columns,
    now: i64,
    kind: &str,
    worktree: &str,
    title: &str,
    continuation: bool,
    started_at_ms: Option<i64>,
) -> String {
    let mut row = String::new();
    if columns.age {
        // Blank rather than absent for a row that cannot say when it started: the columns
        // after it are read down, and a missing gutter would step every one of them left.
        row.push_str(&agents::age_gutter(now, started_at_ms));
    }
    for (cell, width) in [(kind, columns.kind), (worktree, columns.worktree)] {
        if width == 0 {
            continue;
        }
        let cell = agents::cut_to_width(cell.to_owned(), width);
        row.push_str(&cell);
        row.push_str(&" ".repeat(width - cell.width() + COLUMN_GAP));
    }
    let title = match continuation {
        true => format!("{CONTINUATION}{title}"),
        false => title.to_owned(),
    };
    row.push_str(&agents::cut_to_width(title, columns.title));
    row
}

/// One row of the list as it is drawn: `selected` is whether the cursor is on it, and `live`
/// whether the selected row is the pane the user is in.
fn list_text(row: &StripRow, selected: bool, live: bool, columns: &Columns, now: i64) -> Text {
    let marker = if selected { ">" } else { " " };
    match row {
        // A header takes no marker column: nothing can land on it, and
        // the indentation it carries is what shows the nesting.
        StripRow::Project(text) => Text::new(text).color_range(0, ..),
        StripRow::Branch(text) => Text::new(text).color_range(3, ..),
        StripRow::Agent { text, .. } => {
            // The selection is a band either way - `selected()` paints the row's whole
            // width - so it stays findable when the user is somewhere else. Colour against
            // dim says which of those two it is, and so does the marker: `>` for the pane the
            // user is in, `·` for the one they were last in, since dim and colour are both
            // things a terminal can decline to draw.
            let marker = match (selected, live) {
                (true, true) => ">",
                (true, false) => "·",
                (false, _) => " ",
            };
            let line = Text::new(format!("{marker} {text}"));
            match (selected, live) {
                (true, true) => line.selected().color_range(2, ..),
                (true, false) => line.selected().dim_range(..),
                (false, _) => line.dim_range(..),
            }
        },
        StripRow::Run {
            kind,
            title,
            worktree,
            started_at_ms,
            continuation,
            ..
        } => {
            let line = Text::new(format!(
                "{marker}   {}",
                archive_row(
                    columns,
                    now,
                    kind,
                    worktree,
                    title,
                    *continuation,
                    *started_at_ms,
                )
            ));
            if selected {
                line.selected().color_range(4, ..)
            } else {
                line.dim_range(..)
            }
        },
        StripRow::MoreRuns => {
            let line = Text::new(format!("{marker}   load older runs…"));
            if selected {
                line.selected()
            } else {
                line.dim_range(..)
            }
        },
    }
}

// Every content coordinate is relative to the row below the permanent tab bar.
thread_local! { static CONTENT_OFFSET: std::cell::Cell<usize> = const { std::cell::Cell::new(1) }; }
fn paint(text: Text, x: usize, y: usize, width: Option<usize>, height: Option<usize>) {
    print_text_with_coordinates(
        text,
        x,
        y + CONTENT_OFFSET.with(|offset| offset.get()),
        width,
        height,
    );
}

/// The rule between the two halves: a label naming what is below it, the rest of the row ruled
/// off, and the scroll position at the right-hand end.
///
/// Where the focus shows. The list's own selection band is dim or coloured already, and that
/// says whether the selected agent is the pane the user is *in* - overloading it with "and the
/// keys are up here" would cost the pane the only thing it says about the agent.
///
/// `focused` is whether a key pressed now reaches what is below the rule, and callers pass
/// exactly that - not merely which half the stop is on - so a rule left coloured after the
/// keys went elsewhere cannot claim them.
fn render_divider(label: &str, above: usize, below: usize, focused: bool, y: usize, cols: usize) {
    paint(
        divider_text(label, above, below, focused, cols),
        0,
        y,
        Some(cols),
        None,
    );
}

/// The columns [`divider_text`] puts in front of a label to say whether the keys reach below
/// the rule - `▸ `, or two spaces - which a caller fitting its label to the row by whole
/// segments has to leave room for, or `divider_line` cuts the tail off mid-word.
const FOCUS_MARK_COLS: usize = 2;

/// [`render_divider`]'s row, styled: what a test can read the styles off.
///
/// The focus is a `▸` before the label as well as the rule's colour, since colour is a thing a
/// terminal can decline to draw; an unfocused label keeps the two columns, so it does not jump
/// sideways as the keys come and go.
fn divider_text(label: &str, above: usize, below: usize, focused: bool, cols: usize) -> Text {
    let label = match (focused, label.is_empty()) {
        (true, true) => "▸".to_owned(),
        (true, false) => format!("▸ {label}"),
        (false, true) => String::new(),
        (false, false) => format!("  {label}"),
    };
    let line = divider_line(&label, above, below, cols);
    // Found rather than counted: a label cut to fit may have lost the marker, and no kind can
    // contain one (`valid_kind` allows no punctuation).
    let marker = label
        .contains('!')
        .then(|| line.chars().position(|c| c == '!'))
        .flatten();
    let text = Text::new(line);
    // Dim wins over colour where both are asked for, so an unfocused rule is dimmed around
    // the marker rather than under it: a waiting `!` painted red under a dim range was drawn
    // dim, the one character on the row meant to stand out.
    let text = match (focused, marker) {
        (true, _) => text.color_range(2, ..),
        (false, Some(at)) => text.dim_range(..at).dim_range(at + 1..),
        (false, None) => text.dim_range(..),
    };
    match marker {
        Some(at) => text.error_color_range(at..at + 1),
        None => text,
    }
}

/// What the divider has to say about the row under the cursor beyond naming it, and how much
/// of the row it is worth.
///
/// The divider is one line, `fit_label` cuts it from the tail, and an agents pane is narrow -
/// so what shares the row with the label and what takes the whole of it is a decision, not a
/// detail. A hint about something still happening rides `Beside` the label; a question comes
/// `Before` it, because its tail is its answer keys and the tail is what a narrow row loses;
/// a refusal, which is longer than any of them and is the only one the user has not already
/// seen, takes the row `Instead`.
enum Hint {
    Beside(String),
    Before(String),
    Instead(String),
}

/// How the pane divides: `(list_rows, history_rows)`, with the divider on the row between
/// them. `history_rows` is `0` on a pane too short to be split, which is the one case the
/// halves are not permanent - a list squeezed to nothing shows neither.
///
/// Half each, near enough: the divider comes off the list's share, so an odd pane gives the
/// extra row to history.
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

struct Predecessors {
    base: Strip,
    pane_id: u32,
    title: String,
    result: Option<wire::PredecessorsResult>,
}

impl Predecessors {
    fn rows(&self) -> Vec<StripRow> {
        let live_end = self
            .base
            .rows
            .iter()
            .rposition(|row| matches!(row, StripRow::Agent { .. }))
            .map_or(0, |index| index + 1);
        let mut rows = self.base.rows[..live_end].to_vec();
        let label = match &self.result {
            None => "Loading predecessors…",
            Some(result) if !result.diagnostic.is_empty() => &result.diagnostic,
            Some(result) if result.runs.is_empty() => "No predecessors recorded",
            Some(_) => "Previous agents",
        };
        rows.push(StripRow::Project(label.to_owned()));
        if let Some(result) = &self.result {
            rows.extend(result.runs.iter().map(|run| StripRow::Run {
                run_id: run.id.clone(),
                kind: run.kind.clone(),
                title: run.title.clone(),
                worktree: agents::worktree_cell(run),
                started_at_ms: Some(run.started_at_ms),
                continuation: run.previous_run_id.is_some(),
            }));
        }
        rows
    }
}

impl State {
    fn composition_tabs(&self) -> Vec<PaneTabDescriptor> {
        [
            ("Agents", "hocus-agents", "agents"),
            ("Todos", "hocus-todos", ""),
            ("VMs", "hocus-agents", "vms"),
            ("Swamp", "hocus-agents", "swamp"),
        ]
        .into_iter()
        .map(|(label, plugin, view)| PaneTabDescriptor {
            label: label.into(),
            location: format!("zellij:{plugin}"),
            initial_cwd: Some(self.initial_cwd.clone()),
            configuration: if view.is_empty() {
                BTreeMap::new()
            } else {
                BTreeMap::from([("view".into(), view.into())])
            },
        })
        .collect()
    }

    fn mount_composition(&mut self) {
        let tabs = self.composition_tabs();
        self.pane_tabs = mount_pane_tabs(
            PaneId::Plugin(self.own_plugin_id),
            PaneTabsLayout { tabs, active: 0 },
            0,
        )
        .ok()
        .flatten()
        .map(|tabs| tabs.id);

        self.mount_pending = self.pane_tabs.is_none();
    }

    /// The key hints on the pane's bottom row, or `None` where there is no such row.
    ///
    /// Three owners. The key sheet, while it is up, says how to shut it, on every surface: it
    /// is drawn over whatever hint row the surface has. Otherwise only Agents has one - the
    /// other surfaces carry their hints on a row of their own, so a footer there would be the
    /// same keys twice, and that row is where they say how to work the tab bar while it has the
    /// keys, so their bodies keep the same number of rows either way - all but a quiet VMs
    /// list, whose row comes only with the bar's keys (`render_vms`), and an open todo box,
    /// whose keys spill onto a row under the list when the draft leaves its own row too little
    /// room for them, the bar's as much as the box's (`input_row`). On Agents, while the
    /// panel's own tab bar has the keys the row is the bar's: the bar answers nothing but its
    /// own keys, so the list's hints would be advertising keys that are not what the next press
    /// does.
    ///
    /// Built as segments and fitted by dropping whole ones from the end rather than cutting
    /// mid-word - `Enter fo` names no key at all. The Agents footer gives up a trailing key or
    /// two to end in `? keys`, which is what `?` opens, but never its first key or the row's own
    /// verb (`agents::fit_hints_marked`). A line where `?` is typed into the search box, or
    /// shuts the sheet, carries no marker (`agents::fit_keys`): there it opens nothing.
    fn footer_hint(&self) -> Option<String> {
        if self.last_rows < 3 || self.last_cols == 0 {
            return None;
        }
        if self.key_sheet_scroll.is_some() {
            // Measured against the rows the sheet is about to get - the pane less the tab bar
            // and this row - since the footer is settled before the sheet is drawn, and in the
            // screen rows its entries wrap to at this width, which is what it scrolls by.
            let scrolls = self.key_sheet_display(self.last_cols).len() > self.last_rows - 2;
            let segments: &[&str] = match scrolls {
                true => &["Esc/? close", "j/k scroll"],
                false => &["Esc/? close"],
            };
            return Some(agents::fit_keys(segments, self.last_cols));
        }
        if self.surface != Surface::Agents {
            return None;
        }
        // Not while the search box is being typed in, which takes the keys ahead of the bar
        // (see `on_key`): the hint follows the keys rather than the highlight.
        if self.bar_has_keys() && !self.box_ahead_of_the_bar() {
            let segments: Vec<&str> = tabs::BAR_HINT.split(" · ").collect();
            return Some(agents::fit_hints(&segments, self.last_cols));
        }
        let (segments, keep) = self.agents_hints();
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        Some(match self.typing() {
            true => agents::fit_keys(&segments, self.last_cols),
            false => agents::fit_hints_marked(&segments, self.last_cols, keep),
        })
    }

    /// The Agents footer's keys for what the pane is doing now, most useful first.
    ///
    /// Read off the row under the cursor as well as the view, the way `on_key` reads them: a
    /// hint offering `p` on a run, or `Enter focus` on an empty list, names keys that do
    /// nothing there, and one that is the same everywhere leaves out the keys that work. A
    /// question comes first and says its answer keys first, because the row is cut from the
    /// tail and a question without its `y` is not one the user can see they are being asked.
    ///
    /// Beside the keys, how many of them lead - the ones `agents::fit_hints_marked` keeps
    /// whatever it gives up to say `? keys`. On a row that is whatever stands ahead of the row's
    /// verb - a refusal, the way out of the view, somebody waiting - and the verb itself; in the
    /// history half, up to `Enter read`; in the reader, the way back. A question, the search box
    /// and an empty list keep all of theirs, which is `fit_hints` exactly: they have few keys,
    /// and each is the answer to something.
    fn agents_hints(&self) -> (Vec<String>, usize) {
        let owned =
            |keys: &[&str]| -> Vec<String> { keys.iter().map(|key| (*key).to_owned()).collect() };
        let all = |keys: Vec<String>| {
            let keep = keys.len();
            (keys, keep)
        };
        if let Some(reading) = &self.reading {
            let folding = !reading.sections.is_empty() && !reading.raw;
            // The keys nobody would guess come first. The `▸`/`▾` on each fold and a click
            // already suggest what `Enter` does to one; nothing on screen says that `y`, `o`
            // and `v` do anything at all.
            let mut keys = vec![
                "Esc back",
                if folding { "j/k fold" } else { "↑↓ scroll" },
                "y copy",
            ];
            if self.open_target().is_some() {
                keys.push("o file");
            }
            if !reading.sections.is_empty() {
                keys.push(if reading.raw { "v folds" } else { "v source" });
            }
            if folding {
                keys.push("Enter open");
            }
            keys.push("PgUp/PgDn page");
            return (owned(&keys), 1);
        }
        if self.search_editing {
            if self.search_pending {
                return all(owned(&["searching…", "Esc cancel"]));
            }
            // What went wrong is drawn whole under the box (`search_failure_lines`), where it
            // has rows to wrap into. It used to lead this row, where a narrow pane cut it to
            // its first few words and the keys that go on from it with them.
            if self.search_error.is_some() {
                return all(owned(&["Enter retry", "Esc back"]));
            }
            // The way out before the way to start again, so a narrow pane keeps the way out.
            let mut keys = owned(&["Enter apply", "Esc cancel"]);
            if !self.search_query.is_empty() {
                keys.push("Ctrl-u clear".to_owned());
            }
            return all(keys);
        }
        let selection = self.selection_key();
        if selection.is_some() && self.confirming_resume == selection {
            return all(owned(&["y resume", "n cancel"]));
        }
        let waiting = self.waiting_count();
        let toggle = format!("w waiting ({waiting})");
        let search = self.search_base.is_some();
        let mut keys = Vec::new();
        // A refusal is news, and its reason is what the user acts on. The divider says it too,
        // but a pane too short for a history half has no divider.
        if let Some((row, detail)) = &self.resume_note {
            if Some(row) == selection.as_ref() {
                keys.push(format!("resume failed: {detail}"));
            }
        }
        // Then the way out of the view the list is in. A search's `/ edit` waits its turn with
        // the other keys: straight after the way out it pushed the waiting count and the hit's
        // own verb off every pane narrower than about fifty columns.
        if self.waiting_base.is_some() {
            keys.push("Esc/w all".to_owned());
            keys.push(format!("{waiting} waiting"));
        } else if self.predecessors.is_some() || search {
            keys.push("Esc/c clear".to_owned());
        }
        if self.rows.is_empty() {
            // Nothing to act on, so no row keys, no history half to Tab into, and the way on
            // is a search or the conversation - or, from a search that found nothing, the
            // agents that are waiting, if any are, and another search.
            if search {
                if waiting > 0 {
                    keys.push(toggle);
                }
                keys.push("/ edit".to_owned());
            } else if self.waiting_base.is_none() {
                keys.push("/ search".to_owned());
                // Only once hocus has said anything. Before its first strip there may be no
                // conversation for a Tab to yield to, and a hint for a key that may land
                // nowhere is worse than none; the first strip redraws, so the hint arrives
                // with it. The key is the same either way.
                if self.strip_seen {
                    keys.push("Tab chat".to_owned());
                }
                keys.push("Esc hide".to_owned());
            }
            return all(keys);
        }
        let row = self.rows.get(self.selected);
        // Somebody waiting on an answer outranks the row's own keys: it is what the list is
        // opened for, more often than not, and the one thing here about every agent at once.
        if waiting > 0 && self.waiting_base.is_none() {
            keys.push(toggle.clone());
        }
        // The half's keys only while the keys are really in it: once another pane has them the
        // divider is drawn unfocused, and the footer does not claim otherwise.
        let history = self.focus == Focus::History && self.has_keyboard != Some(false);
        let keep = if history {
            keys.push("↑↓ event".to_owned());
            if self.cursor_event().is_some() {
                keys.push("Enter read".to_owned());
            }
            let keep = keys.len();
            if self.open_target().is_some() {
                keys.push("o file".to_owned());
            }
            keep
        } else {
            match row {
                Some(StripRow::Agent { .. }) => keys.push("Enter focus".to_owned()),
                Some(StripRow::Run { .. }) if self.last_history_rows > 0 => {
                    keys.push("Enter history".to_owned())
                },
                Some(StripRow::MoreRuns) => keys.push("Enter load older runs".to_owned()),
                _ => {},
            }
            keys.len()
        };
        match row {
            Some(StripRow::Agent { .. }) => keys.extend(owned(&["s summary", "p previous"])),
            Some(StripRow::Run { .. }) => keys.extend(owned(&["r resume", "s summary"])),
            _ => {},
        }
        if self.waiting_base.is_none() && waiting == 0 {
            keys.push(toggle);
        }
        keys.push(match search {
            true => "/ edit".to_owned(),
            false => "/ search".to_owned(),
        });
        if history {
            keys.extend(owned(&["PgUp/PgDn scroll", "Tab chat"]));
        } else if self.last_history_rows > 0 {
            keys.push("Tab history".to_owned());
        } else {
            keys.push("Tab chat".to_owned());
        }
        (keys, keep)
    }

    /// What the key sheet lists: every key whatever has the keys now answers, as
    /// `(keys, what they do)`, most useful first.
    fn key_sheet(&self) -> Vec<(String, String)> {
        // The panel's own tab bar is the panel's, whichever tab it is on.
        if self.bar_has_keys() {
            let mut rows = sheet(&[
                ("←/→", "switch tab"),
                ("Enter ↓ j", "into the tab's list"),
                ("Tab/Esc", "into the tab's list"),
                ("Shift+Tab", "to the conversation's box"),
                ("other keys", "go to the list, and act there"),
                ("click a tab", "switch to it"),
            ]);
            rows.extend(sheet(PANEL_KEYS));
            return rows;
        }
        let mut rows = match self.surface {
            Surface::Agents => return self.agents_key_sheet(),
            #[cfg(test)]
            Surface::Todos => self.todo_key_sheet(),
            Surface::Vms => self.vm_key_sheet(),
            Surface::Swamp => self.swamp_key_sheet(),
            #[cfg(not(test))]
            Surface::Todos => vec![],
        };
        rows.extend(sheet(PANEL_KEYS));
        rows
    }

    /// The Agents surface's key sheet, for the mode it is in: the reader, a question, or the
    /// list and history halves - with the keys that act on a row only where the row under the
    /// cursor is one they act on. Never the search box's: `?` typed there is a question mark
    /// (`typing`), so the box's keys are its footer's to name.
    fn agents_key_sheet(&self) -> Vec<(String, String)> {
        let tail = PANEL_KEYS;
        if let Some(reading) = &self.reading {
            let mut rows: Vec<(&str, &str)> = match (reading.sections.is_empty(), reading.raw) {
                (false, false) => vec![
                    ("j/k", "the next or previous fold"),
                    ("Enter", "open or shut the fold"),
                    ("click", "point at a fold; again opens it"),
                    ("↑/↓", "scroll a line"),
                    ("PgUp/PgDn", "scroll a page"),
                    ("y", "copy the fold"),
                    ("o", "open the file the fold names"),
                    ("v", "show the source"),
                ],
                (false, true) => vec![
                    ("j/k ↑/↓", "scroll a line"),
                    ("PgUp/PgDn", "scroll a page"),
                    ("y", "copy the whole event"),
                    ("o", "open the first file it names"),
                    ("v", "back to the folds"),
                ],
                (true, _) => vec![
                    ("j/k ↑/↓", "scroll a line"),
                    ("PgUp/PgDn", "scroll a page"),
                    ("y", "copy the event"),
                ],
            };
            rows.extend([
                ("wheel", "scroll"),
                ("Esc", "back to the history"),
                ("Tab", "to the conversation"),
                ("Shift+Tab", "to the tab bar"),
            ]);
            rows.extend(tail);
            return sheet(&rows);
        }
        let selection = self.selection_key();
        if selection.is_some() && self.confirming_resume == selection {
            let mut rows = vec![
                ("y", "resume the run in a new pane"),
                ("n/Esc", "cancel"),
                ("other keys", "cancel, then act as usual"),
            ];
            rows.extend(tail);
            return sheet(&rows);
        }
        let row = self.rows.get(self.selected);
        let history = self.focus == Focus::History;
        // An empty list has no row to walk, act on or click, and no history half to scroll:
        // the sheet lists the keys that lead out of it, the way the empty list's footer does.
        let empty = self.rows.is_empty();
        let mut rows: Vec<(&str, &str)> = Vec::new();
        if history {
            rows.extend([
                ("↑/↓ j/k", "the previous or next event"),
                ("Enter", "read the event in full"),
                ("o", "open the file the event names"),
                ("click", "point at an event; again reads it"),
                ("PgUp/PgDn", "scroll the history"),
                ("wheel", "scroll the half under it"),
            ]);
        } else if !empty {
            rows.push(("↑/↓ j/k", "choose an agent or a run"));
            rows.push(match row {
                Some(StripRow::Run { .. }) => ("Enter", "into the run's history"),
                Some(StripRow::MoreRuns) => ("Enter", "load older runs"),
                _ => ("Enter", "go to the agent; the panel goes"),
            });
            rows.push(match row {
                Some(StripRow::Agent { .. }) => ("click", "select; again goes to the agent"),
                Some(StripRow::MoreRuns) => ("click", "load older runs"),
                _ => ("click", "select the row"),
            });
        }
        match row {
            Some(StripRow::Agent { .. }) => rows.extend([
                ("s", "summarise the agent now"),
                ("p", "the agent's previous runs"),
                ("D / F", "duplicate / fork the agent"),
            ]),
            Some(StripRow::Run { .. }) => rows.extend([
                ("r", "resume the run (asks y/n)"),
                ("s", "summarise the run now"),
            ]),
            _ => {},
        }
        rows.push(match self.waiting_base.is_some() {
            true => ("w", "every agent again"),
            false => ("w", "only agents waiting for you"),
        });
        rows.push(match self.search_base.is_some() {
            true => ("/", "edit the search"),
            false => ("/", "search every run"),
        });
        if self.search_base.is_some() || self.predecessors.is_some() || self.waiting_base.is_some()
        {
            rows.push(("c", "back to the whole list"));
        }
        if !history && !empty {
            rows.push(("PgUp/PgDn", "scroll the history"));
            rows.push(("wheel", "scroll the half under it"));
        }
        // Not before hocus's first strip, for the reason the empty list's footer leaves out
        // `Tab chat`: there may be no conversation yet for the key to reach. The tab bar is
        // always there, so Shift+Tab stays.
        if !(empty && !self.strip_seen) {
            rows.push(match (history, self.last_history_rows > 0) {
                (true, _) => ("Tab", "to the conversation"),
                (false, true) => ("Tab", "into the history"),
                (false, false) => ("Tab", "to the conversation"),
            });
        }
        rows.push(match history {
            true => ("Shift+Tab", "back to the list"),
            false => ("Shift+Tab", "to the tab bar"),
        });
        rows.push(("Ctrl-r", "look at the branches again"));
        let filtered = self.search_base.is_some()
            || self.predecessors.is_some()
            || self.waiting_base.is_some();
        rows.push(match filtered {
            true => ("Esc", "back to the whole list"),
            false => ("Esc", "put the panel away"),
        });
        rows.extend(tail);
        sheet(&rows)
    }

    /// The key sheet over the surface's share of the pane: keys in one column, what they do in
    /// the next, scrolled where it is taller than the pane.
    ///
    /// Scrolled by screen row rather than by entry, since an entry wraps onto as many rows as
    /// its description needs: counted in entries, the last few rows of a sheet whose entries
    /// wrap could never be scrolled onto the screen.
    fn render_key_sheet(&mut self, rows: usize, cols: usize) {
        let lines = self.key_sheet_display(cols);
        self.last_sheet_rows = rows;
        let max_scroll = lines.len().saturating_sub(rows);
        self.last_sheet_max_scroll = max_scroll;
        let scroll = self.key_sheet_scroll.unwrap_or(0).min(max_scroll);
        self.key_sheet_scroll = Some(scroll);
        if lines.is_empty() {
            paint(
                Text::new(" no keys listed for this tab").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
            return;
        }
        for (index, (line, key_chars)) in lines.into_iter().skip(scroll).take(rows).enumerate() {
            paint(
                Text::new(line).color_range(2, ..key_chars),
                0,
                index,
                Some(cols),
                None,
            );
        }
    }

    /// The key sheet as it is drawn `cols` columns wide: one `(line, key characters)` per
    /// screen row, each entry wrapped onto as many rows as it needs (see `key_sheet_lines`).
    fn key_sheet_display(&self, cols: usize) -> Vec<(String, usize)> {
        let sheet = self.key_sheet();
        // As wide as the widest keys, up to half the row: past that the descriptions would have
        // nothing left to be read in.
        let key_width = sheet
            .iter()
            .map(|(keys, _)| keys.width())
            .max()
            .unwrap_or(0)
            .min(cols.saturating_sub(1) / 2);
        sheet
            .iter()
            .flat_map(|(keys, what)| key_sheet_lines(keys, what, key_width, cols))
            .collect()
    }

    /// What an empty Agents list says, and the key that leads on from it where there is one.
    fn empty_list_message(&self) -> (&'static str, Option<&'static str>) {
        if self.search_base.is_some() {
            ("no matches", None)
        } else if self.waiting_base.is_some() {
            ("no agents waiting for you", Some("w shows every agent"))
        } else if !self.strip_seen {
            // Nothing has said the list is empty yet: hocus may still be loading, or not be
            // there at all, and "no agents" would claim something nobody has reported.
            ("waiting for hocus…", None)
        } else {
            ("no agents", Some("Ctrl e n starts one"))
        }
    }

    /// How many agents are waiting on an answer - in the session, not the view. Counted in the
    /// live strip a filtered view keeps behind it: a search or the previous-agents view shows
    /// no live agents to count, and the waiting view may be holding on to one that has just
    /// been answered.
    fn waiting_count(&self) -> usize {
        let live = self
            .waiting_base
            .as_ref()
            .map(|strip| &strip.rows[..])
            .or_else(|| {
                self.predecessors
                    .as_ref()
                    .map(|previous| &previous.base.rows[..])
            })
            .or_else(|| self.search_base.as_ref().map(|(rows, _)| &rows[..]))
            .unwrap_or(&self.rows);
        live.iter().filter(|row| waiting_row(row)).count()
    }

    /// Whether a box that takes every key ahead of the tab bar is being typed in: an Agents
    /// search, or one of Swamp's. See `on_key`.
    fn box_ahead_of_the_bar(&self) -> bool {
        self.search_editing || (self.surface == Surface::Swamp && self.swamp.is_editing())
    }

    /// Whether a key typed now is text for a box rather than a command - which is when `?` is
    /// a question mark and not the key sheet.
    ///
    /// The todo box counts even under the tab bar: the bar hands a key it does not own to the
    /// surface, so `?` typed there would have reached the draft. Asked of the mode rather than
    /// of where the caret would go, which a pane too narrow to draw the box in has nowhere to
    /// put - and a box too narrow to see is still the box the next key is typed into.
    fn typing(&self) -> bool {
        #[cfg(not(test))]
        {
            self.box_ahead_of_the_bar()
        }
        #[cfg(test)]
        {
            self.box_ahead_of_the_bar()
                || (self.surface == Surface::Todos
                    && matches!(self.todos.mode, todos::Mode::Input { .. }))
        }
    }

    /// The Agents search box as it is drawn in `cols` columns, and the column its caret is in.
    ///
    /// Scrolled sideways rather than cut from the tail, with the prompt pinned: a box that
    /// loses its end loses the character being typed, and the caret with it.
    fn search_box(&self, cols: usize) -> (String, usize) {
        let prompt = "Search: ";
        let cursor = self.search_query.chars().count();
        zellij_tile::ui_components::forms::viewport(prompt, &self.search_query, cursor, cols)
    }

    /// What a filtered Agents view is called, drawn on the surface's first row above the list,
    /// or `None` for the plain list. While a search is being typed it is the search box.
    fn view_label(&self) -> Option<String> {
        if self.search_editing {
            return Some(self.search_box(self.last_cols).0);
        }
        if let Some(previous) = &self.predecessors {
            return Some(format!("Previous agents: {}", previous.title));
        }
        if self.search_base.is_some() {
            // Its age once it has one worth saying: the hits are not run again, so after a long
            // hide they are an old answer, and runs that matched since are not among them.
            let age = self
                .searched_at_ms
                .map(|at| (now_ms(), at))
                .filter(|(now, at)| now - at >= 60_000)
                .map(|(now, at)| format!(" · {} ago", agents::short_age(now, at)))
                .unwrap_or_default();
            let omitted = match self.search_truncated {
                true => "  more matches omitted",
                false => "",
            };
            return Some(format!("Search: {}{age}{omitted}", self.search_query));
        }
        // The one filtered view that used to have no label at all, only a dim footer: the tab
        // still said "Agents" over a list quietly missing most of them.
        self.waiting_base
            .is_some()
            .then(|| "Waiting for you".to_owned())
    }

    fn switch_surface(&mut self, surface: Surface) {
        if self.surface == surface {
            return;
        }
        self.swamp.cancel_filter();
        // A search being typed takes every key ahead of the surface, so leaving one open would
        // send the next tab's keys into a box on a tab no longer showing. Ended the way Esc
        // ends it. An armed question - a VM close or sync, a todo delete or complete, a resume -
        // is dropped for the reason `end_visit` drops it: it would otherwise take the first key
        // of the next visit to its tab, and a `y` then would act on a question asked about a
        // list since forgotten.
        if self.search_editing {
            self.cancel_search_edit();
        }
        match self.surface {
            Surface::Vms => self.cancel_vm_confirmation(),
            #[cfg(test)]
            Surface::Todos => self.cancel_todo_question(),
            Surface::Agents => self.confirming_resume = None,
            Surface::Swamp => {},
            #[cfg(not(test))]
            Surface::Todos => {},
        }
        self.surface = surface;
        if surface == Surface::Vms {
            self.poll_vms();
        }
        if surface == Surface::Swamp {
            self.activate_swamp();
        }
    }

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
        self.pending_history_revision = self.selected_revision();
        true
    }

    /// Everything about the history half that belonged to the row the cursor has just left.
    ///
    /// One place rather than three, because the list of what a row owns has grown: where the
    /// half was wound to, which event the cursor was on and the events both were holding,
    /// whether a summary was asked for, what `o` or a fetch had to say about the row, and
    /// whichever event was open in the reader, which is about an entry of a run the cursor is
    /// no longer pointing at.
    ///
    /// A summary asked for and not yet answered goes too: it is about the row, and a hint left
    /// on the next one would claim that row was being described.
    fn leave_row(&mut self) {
        self.pending_history_revision = None;
        self.list_scroll = None;
        self.history_scroll = 0;
        self.history_cursor = 0;
        self.history_cursor_anchor = None;
        self.history_bottom_anchor = None;
        self.history_scrolled = false;
        self.summarizing = None;
        self.confirming_resume = None;
        self.resume_note = None;
        self.open_note = None;
        self.run_error = None;
        self.close_reader();
    }

    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        // Cycle the whole panel one corner clockwise - the same gesture the conversation has.
        // Taken before everything else, because where the keys are inside the panel changes
        // nothing about what it means: not a box being typed in, not the tab bar, not the
        // reader, not an armed question - which stays armed and on screen, since moving the
        // panel answers nothing it asked - and not the key sheet, which stays up. hocus owns
        // both overlays' geometry, so this asks rather than moves. Nothing to redraw: the move
        // is the server's to paint.
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            return false;
        }
        let sheet_was_up = self.key_sheet_scroll.is_some();
        if let Some(answer) = self.sheet_key(&key) {
            return answer;
        }
        // A key the sheet let through has shut it, unless it is one that works through it, and
        // the sheet coming down is a redraw whatever the key goes on to do.
        let shut = sheet_was_up && self.key_sheet_scroll.is_none();
        self.dispatch_key(key) || shut
    }

    /// A key while the key sheet is up, or the `?` that puts it up. `Some` is the whole of the
    /// answer; `None` hands the key on - having shut the sheet, unless it is one of the keys
    /// that work through it.
    ///
    /// `?` is the sheet's only while nobody is typing, where it is a character (`typing`). Every
    /// armed question stays armed under it, the VMs tab's close and sync among them: the sheet
    /// says how to answer, and shutting it leaves the question where it was. A key the sheet
    /// lets through then reaches the question as it would have without the sheet - `y`
    /// answers it, and anything else cancels it and does its job.
    fn sheet_key(&mut self, key: &KeyWithModifier) -> Option<bool> {
        let plain = key.has_no_modifiers();
        // A terminal that reports the Shift most layouts spend on `?` is still typing one.
        let question = key.bare_key == BareKey::Char('?')
            && (plain || key.has_modifiers(&[KeyModifier::Shift]));
        if self.key_sheet_scroll.is_none() {
            if question && !self.typing() {
                self.key_sheet_scroll = Some(0);
                return Some(true);
            }
            return None;
        }
        let shuts =
            question || (plain && matches!(key.bare_key, BareKey::Esc | BareKey::Char('q')));
        if shuts {
            self.key_sheet_scroll = None;
            return Some(true);
        }
        let page = self.last_sheet_rows.saturating_sub(1).max(1) as isize;
        match key.bare_key {
            BareKey::Down => Some(self.scroll_key_sheet(1)),
            BareKey::Up => Some(self.scroll_key_sheet(-1)),
            BareKey::Char('j') if plain => Some(self.scroll_key_sheet(1)),
            BareKey::Char('k') if plain => Some(self.scroll_key_sheet(-1)),
            BareKey::PageDown => Some(self.scroll_key_sheet(page)),
            BareKey::PageUp => Some(self.scroll_key_sheet(-page)),
            // The ring works through the sheet, which then says what the stop the keys went to
            // answers - or stays up over a pane the keys have left, to be read from there.
            BareKey::Tab => None,
            _ => {
                self.key_sheet_scroll = None;
                None
            },
        }
    }

    /// Move the key sheet by `delta` rows, and say whether it moved.
    fn scroll_key_sheet(&mut self, delta: isize) -> bool {
        let Some(scroll) = self.key_sheet_scroll else {
            return false;
        };
        let target = scroll
            .saturating_add_signed(delta)
            .min(self.last_sheet_max_scroll);
        self.key_sheet_scroll = Some(target);
        target != scroll
    }

    /// Hand a key to whatever in the panel is answering keys now.
    fn dispatch_key(&mut self, key: KeyWithModifier) -> bool {
        if self.search_editing {
            return self.search_key(key);
        }
        if self.surface == Surface::Swamp && self.swamp.is_editing() {
            return self.swamp_key(key);
        }
        // A burst can already be queued in the child when it yields to the native bar.
        // Route those keys through the same host actions instead of changing this fixed view.
        if self.native_tabs && self.tabs_focused {
            match key.bare_key {
                BareKey::Left | BareKey::Right if key.has_no_modifiers() => {
                    run_action(
                        if key.bare_key == BareKey::Left {
                            actions::Action::PreviousPaneTab
                        } else {
                            actions::Action::NextPaneTab
                        },
                        BTreeMap::new(),
                    );
                    return true;
                },
                BareKey::Tab if key.has_modifiers(&[KeyModifier::Shift]) => {
                    self.tabs_focused = false;
                    if let Some(id) = self.pane_tabs {
                        let _ = focus_pane_tabs(id, false);
                    }
                    self.yield_to_chat(wire::STOP_INPUT);
                    return true;
                },
                _ => {
                    self.keys_to_the_list();
                    if let Some(id) = self.pane_tabs {
                        let _ = focus_pane_tabs(id, false);
                    }
                    if matches!(
                        key.bare_key,
                        BareKey::Tab
                            | BareKey::Esc
                            | BareKey::Enter
                            | BareKey::Down
                            | BareKey::Char('j')
                    ) {
                        return true;
                    }
                },
            }
        }
        // Whether this key took the keys off the bar, which is a redraw whatever the key goes on
        // to do: the bar's highlight and its footer come down.
        let mut left_bar = false;
        if self.tabs_focused {
            // The bar answers its own keys. Anything else is not the bar's, so rather than
            // being swallowed it hands the keys back to the surface and does its usual job
            // there: the list it lands in is drawn right under the bar, so the user sees where
            // the key went. The chat's bar swallows stray keys because its input box is hidden
            // behind it; nothing is hidden behind this one.
            let plain = key.has_no_modifiers();
            match key.bare_key {
                BareKey::Tab if key.has_modifiers(&[KeyModifier::Shift]) => {
                    // Back off the front of the ring. The flag goes with the keys, so the bar
                    // stops drawing itself focused while the conversation has them.
                    self.tabs_focused = false;
                    self.yield_to_chat(wire::STOP_INPUT);
                    return true;
                },
                // Esc steps back to the list, as the chat's bar steps back to its box: it used
                // to hide the whole panel, one level short of where every other Esc here stops.
                BareKey::Tab | BareKey::Esc => {
                    self.keys_to_the_list();
                    return true;
                },
                BareKey::Left if plain => {
                    self.switch_surface(self.surface.step(-1));
                    return true;
                },
                BareKey::Right if plain => {
                    self.switch_surface(self.surface.step(1));
                    return true;
                },
                BareKey::Enter | BareKey::Down | BareKey::Char('j') if plain => {
                    self.keys_to_the_list();
                    return true;
                },
                _ => {
                    self.tabs_focused = false;
                    left_bar = true;
                },
            }
        }
        let answered = match self.surface {
            #[cfg(not(test))]
            Surface::Todos => false,
            Surface::Swamp => self.swamp_key(key),
            #[cfg(test)]
            Surface::Todos => self.todo_key(key),
            Surface::Vms => self.vm_key(key),
            Surface::Agents => {
                // What the last `o` or `y` had to say is about that key, and the next one ends
                // it - taken before the key is answered, so an `o` pressed again on the same
                // nothing says so again.
                let noted = self.open_note.take().is_some();
                self.agents_key(key) || noted
            },
        };
        answered || left_bar
    }

    /// A key on the Agents surface.
    fn agents_key(&mut self, key: KeyWithModifier) -> bool {
        if self.reading.is_some() && key.bare_key == BareKey::Tab {
            if key.has_modifiers(&[KeyModifier::Shift]) {
                self.tabs_focused = true;
            } else {
                self.yield_to_chat(wire::STOP_TABS);
            }
            return true;
        }
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
        if key.bare_key == BareKey::Char('r') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            // the one part of a row no host event keeps fresh is the branch a
            // checkout is on; ask agentd to probe again
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_REFRESH));
            return false;
        }
        if key.bare_key == BareKey::Tab {
            // This pane's two halves are two of the panel's five stops - the panel's own tab
            // bar, the list, the history half, the conversation's tab bar and its input box -
            // and Tab walks the ring end to end. The crossings that leave this pane are asked
            // for, because the list knows only its own plugin id: `wire::AGENT_STRIP_YIELD`,
            // the same shape as the Esc and Ctrl-o requests. Backward off the list is the
            // panel's tab bar, which is this pane's to draw, so that one is a flag.
            //
            // Directional now, where this used to be modifier-agnostic: with two stops
            // shift-tab had nowhere to go that tab did not, and with five it does. A terminal
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
                    self.tabs_focused = true;
                    return true;
                },
            }
        }
        // The arrows follow the focus, the way `j`/`k` do and the way they do on VMs: in the
        // history half they walk its events. They used to steer the list from either half and
        // take the keys back with them, which made the half's `>` a cursor the arrows every
        // other list moves could not move - and dropped the user's place in it besides, since
        // a new row is a new history. `Shift+Tab` or a click is the way back to the list.
        if let Some(motion) = arrow(&key) {
            return match (self.focus, motion) {
                (Focus::History, Motion::Up) => self.step_history_cursor(1),
                (Focus::History, Motion::Down) => self.step_history_cursor(-1),
                (Focus::List, motion) => self.step_selection(motion),
            };
        }
        // Duplicate or fork the agent under the cursor. Capitals, because `d` and `f` are
        // delete and filter on the tabs beside this one, and a near-miss between two tabs is
        // the kind nobody notices until it has happened.
        if let BareKey::Char(letter @ ('D' | 'F')) = key.bare_key {
            if key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]) {
                return self.act_on_the_agent(letter == 'F');
            }
        }
        if key.has_no_modifiers() {
            match key.bare_key {
                BareKey::Char('w') => return self.toggle_waiting(),
                BareKey::Char('p') => return self.begin_predecessors(),
                // `c` leaves whichever filtered view the list is in, the waiting one as much
                // as the other two.
                BareKey::Char('c') if self.predecessors.is_some() => {
                    return self.clear_predecessors()
                },
                BareKey::Char('c') if self.waiting_base.is_some() => return self.clear_waiting(),
                BareKey::Char('/') => return self.begin_search(),
                BareKey::Char('c') if self.search_base.is_some() => return self.clear_search(),
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
                // Ask the daemon to describe the selected row now. Not gated on the focus:
                // it is about the row the cursor is on, which is the same row whichever
                // half the keys are in, and the answer arrives in the half below either
                // way.
                BareKey::Char('s') => return self.request_summary(),
                // Ask before starting a process. See `confirming_resume`.
                BareKey::Char('r') => return self.arm_resume(),
                // The file the event under the cursor names, without opening the event first.
                // The collapsed row has a cursor on an *event* and none within one, so one key
                // on the event is the whole of what can be offered here — and a notification
                // whose output file is what you actually want is the case this exists for.
                BareKey::Char('o') if self.focus == Focus::History => return self.open_the_file(),
                // The page keys are the history's from either half, since the list follows its
                // selection and has no page of its own; the wheel scrolls whichever half is
                // under the pointer.
                BareKey::PageUp => return self.scroll_history(self.page() as isize),
                BareKey::PageDown => return self.scroll_history(-(self.page() as isize)),
                _ => {},
            }
        }
        match key.bare_key {
            BareKey::Esc => {
                // One step back before the way out: a filtered view is a level the list went
                // into, and Esc used to dismiss the panel from inside one - bringing it back
                // with Ctrl e a still filtered. Guarded on a search that has results, so a plain
                // dismissal does not throw away a history fetch the search bookkeeping holds.
                if self.clear_predecessors()
                    || self.clear_waiting()
                    || (self.search_base.is_some() && self.clear_search())
                {
                    return true;
                }
                // The plain dismissal: the panel goes and the keyboard lands wherever the host
                // puts it. Enter dismisses too, but hands the keyboard on deliberately.
                self.hide_the_panel();
                false
            },
            BareKey::Enter => {
                // With the keys in the history half, Enter is about the event under the
                // cursor rather than the row above: the half caps every event at four rows,
                // and this is where the rest of one is. The two halves have separate cursors,
                // and this is the key that says so.
                if self.focus == Focus::History {
                    return self.open_reader();
                }
                if matches!(self.rows.get(self.selected), Some(StripRow::MoreRuns)) {
                    self.ask_for_older_runs();
                    return false;
                }
                // A run has no pane to walk into, so Enter walks into its history instead -
                // where a search has already put the cursor on the event that matched, and a
                // second Enter reads it. The panel stays up: what Enter found is in it.
                if matches!(self.rows.get(self.selected), Some(StripRow::Run { .. }))
                    && self.last_history_rows > 0
                {
                    self.focus = Focus::History;
                    return true;
                }
                if !matches!(self.rows.get(self.selected), Some(StripRow::Agent { .. })) {
                    return false;
                }
                // The only key in the pane that navigates: a click moves the cursor and no
                // more, so walking the list with the pointer reads an agent's history without
                // leaving for it. Walking in also puts the panel away - the agent is what the
                // user asked to look at, and the panel covers it.
                self.activate();
                false
            },
            _ => false,
        }
    }

    /// Ask hocus for the next page of the archive. The rows arrive with the next strip.
    fn ask_for_older_runs(&mut self) {
        #[cfg(test)]
        {
            self.archive_requests += 1;
        }
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_ARCHIVE_MORE));
    }

    /// Open the New agent modal on the agent under the cursor: a duplicate of it, or a `fork`
    /// of its conversation. Nothing on a run, whose `r` is the way back to it.
    ///
    /// `Ctrl e d` and `Ctrl e f` do the same from the agent's own pane, and from here used to
    /// open a blank New agent instead: the pane with the keyboard is this one, a plugin, and
    /// the modal had no agent to take. This names it.
    ///
    /// The panel stays up under the modal, as it does under the Todos tab's `n`. A modal that
    /// closes hands the keyboard back to the pane it took it from, so one put away with Esc
    /// hands the keys back to this list. One that starts an agent focuses the new pane, and the
    /// `wire::AGENT_STRIP_HIDE` the modal sends as it hands the focus over (its
    /// `hand_over_focus`) puts the panel away then. Hiding the panel before opening the modal,
    /// as `activate` does before a focus, turned that Esc into a trip to whatever was under
    /// the panel.
    fn act_on_the_agent(&mut self, fork: bool) -> bool {
        let Some(StripRow::Agent { pane_id, .. }) = self.rows.get(self.selected) else {
            return false;
        };
        let configuration = pane_action_configuration(*pane_id, fork);
        #[cfg(test)]
        self.new_agent_launches.push(configuration.clone());
        load_new_plugin("hocus-new-agent", configuration, false, false);
        false
    }

    fn begin_predecessors(&mut self) -> bool {
        let Some(StripRow::Agent { pane_id, text, .. }) = self.rows.get(self.selected) else {
            return false;
        };
        let (pane_id, title) = (*pane_id, text.clone());
        self.clear_waiting();
        self.clear_search();
        self.clear_predecessors();
        self.predecessor_request = self.predecessor_request.wrapping_add(1);
        let previous = Predecessors {
            base: Strip {
                rows: self.rows.clone(),
                focused: self.focused,
                summaries: self.summaries.clone(),
            },
            pane_id,
            title,
            result: None,
        };
        self.rows = previous.rows();
        self.predecessors = Some(previous);
        self.selected = position_of(&self.rows, pane_id).unwrap_or(0);
        self.generation += 1;
        let request = wire::PredecessorsRequest {
            request_id: self.predecessor_request.to_string(),
            pane_id,
        };
        if let Ok(payload) = serde_json::to_string(&request) {
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::AGENT_PREDECESSORS).with_payload(payload),
            );
        }
        true
    }

    fn apply_predecessors(&mut self, result: wire::PredecessorsResult) -> bool {
        let selection = self.selection_key();
        let Some(previous) = &mut self.predecessors else {
            return false;
        };
        if result.request_id != self.predecessor_request.to_string()
            || result.pane_id != previous.pane_id
            || previous.result.is_some()
        {
            return false;
        }
        if let Some(anchor) = &result.anchor {
            previous.title = anchor.title.clone();
        }
        previous.result = Some(result);
        self.rows = previous.rows();
        self.selected = self
            .rows
            .iter()
            .position(|row| matches!(row, StripRow::Run { .. }))
            .or_else(|| {
                selection
                    .as_deref()
                    .and_then(|key| position_of_key(&self.rows, key))
            })
            .or_else(|| self.rows.iter().position(selectable))
            .unwrap_or(0);
        self.generation += 1;
        self.follow_selection(selection);
        true
    }

    fn clear_predecessors(&mut self) -> bool {
        let Some(previous) = self.predecessors.take() else {
            return false;
        };
        self.set_rows(previous.base);
        true
    }

    fn apply_search_result(&mut self, result: wire::SearchResult) -> bool {
        if self.search_base.is_none()
            || !self.search_pending
            || result.request_id != self.search_request.to_string()
        {
            return false;
        }
        self.search_pending = false;
        self.search_error = (!result.error.is_empty()).then_some(result.error);
        // A failure leaves the box open with the query as typed, so `Enter` retries it and
        // `Esc` goes back to the search that produced the rows below - which are left alone,
        // and so is whether that search had more matches than it showed.
        // Closing the box put the new query's name over the old query's hits, and made a retry
        // a retype.
        if self.search_error.is_some() {
            return true;
        }
        self.search_truncated = result.truncated;
        self.search_editing = false;
        self.applied_search_query = Some(self.search_query.clone());
        self.searched_at_ms = Some(now_ms());
        let selection = self.selection_key();
        let revision = self.selected_revision();
        self.search_revisions = result
            .runs
            .iter()
            .map(|run| (run.run_id.clone(), run.revision))
            .collect();
        let query = self.search_query.clone();
        self.rows = result
            .runs
            .into_iter()
            .map(|run| {
                // The snippet is what the hit was, and it goes first, from the word that
                // matched: behind a title of up to forty characters it never reached a cell
                // of this width, and every hit read as its title and nothing else.
                let hit = hit_first(&run.snippet, &query);
                StripRow::Run {
                    run_id: run.run_id,
                    kind: run.kind,
                    title: match hit.is_empty() {
                        true => run.title,
                        false => format!("{hit} — {}", run.title),
                    },
                    worktree: run.worktree,
                    started_at_ms: Some(run.started_at_ms),
                    // A search result is a flat list of hits, not a chain: the run above it is
                    // whatever else matched, not the run it continues.
                    continuation: false,
                }
            })
            .collect();
        self.selected = self.rows.iter().position(selectable).unwrap_or(0);
        self.list_scroll = None;
        self.generation += 1;
        if !self.follow_selection(selection) && self.selected_revision() != revision {
            self.leave_row();
            self.pending_history_revision = self.selected_revision();
        }
        if self.pending_history_revision.is_some() {
            self.run = None;
        }
        true
    }

    /// Start typing a search. A fresh one starts empty; `/` over a search already made opens
    /// that search's query to be refined, which is what "edit" says it does - `Ctrl-u` clears
    /// it for a new one. Either way, what was being typed when `Tab` left the box comes back.
    fn begin_search(&mut self) -> bool {
        self.clear_waiting();
        self.clear_predecessors();
        self.search_request = self.search_request.wrapping_add(1);
        self.search_pending = false;
        if self.search_base.is_none() {
            self.search_base = Some((self.rows.clone(), self.summaries.clone()));
            self.search_query.clear();
        }
        if let Some(draft) = self.search_draft.take() {
            self.search_query = draft;
        }
        self.search_error = None;
        self.search_editing = true;
        true
    }

    fn search_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Esc {
            return self.cancel_search_edit();
        }
        if key.bare_key == BareKey::Tab {
            // The ring goes on from the box, which cannot stay open behind it: it takes every
            // key ahead of the tab bar, so a Tab that left it open would be the last key the
            // rest of the panel ever heard. So Tab ends the edit the way Esc does - back to the
            // search last applied, or out of it - keeps what was typed for the next `/`, and
            // then steps on from the list the box sits over: into the history half, on to the
            // conversation, or with Shift back to the tab bar. A redraw whichever: the box is
            // gone.
            let draft = std::mem::take(&mut self.search_query);
            self.cancel_search_edit();
            self.search_draft = (!draft.is_empty()).then_some(draft);
            self.focus = Focus::List;
            self.agents_key(key);
            return true;
        }
        if key.bare_key == BareKey::Enter {
            if self.search_query.trim().is_empty() {
                return true;
            }
            // Already out: hocus answers a search whole and in order, so a second one for the
            // same query would only queue behind the first and throw its answer away. The
            // footer says it is searching.
            if self.search_pending {
                return true;
            }
            self.search_request = self.search_request.wrapping_add(1);
            let request = wire::SearchRequest {
                request_id: self.search_request.to_string(),
                query: self.search_query.clone(),
            };
            let Ok(payload) = serde_json::to_string(&request) else {
                return false;
            };
            self.search_pending = true;
            self.search_deadline_ms = now_ms() + SEARCH_TIMEOUT_MS;
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SEARCH).with_payload(payload));
            self.search_error = None;
            return true;
        }
        if key.bare_key == BareKey::Char('u') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.query_edited();
            }
            return true;
        }
        if key.bare_key == BareKey::Backspace {
            if self.search_query.pop().is_some() {
                self.query_edited();
            }
            return true;
        }
        if key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]) {
            if let BareKey::Char(ch) = key.bare_key {
                self.search_query.push(ch);
                self.query_edited();
                return true;
            }
        }
        false
    }

    /// The query changed: no reply in flight is for it any more, and no failure is about it.
    fn query_edited(&mut self) {
        self.search_request = self.search_request.wrapping_add(1);
        self.search_pending = false;
        self.search_error = None;
    }

    /// Give up on a search hocus has not answered in `SEARCH_TIMEOUT_MS`, and say whether the
    /// box changed. The query stays, so `Enter` asks again; a reply that turns up late is
    /// ignored, the way any reply is once nothing is pending.
    fn expire_search(&mut self, now: i64) -> bool {
        if !self.search_editing || !self.search_pending || now < self.search_deadline_ms {
            return false;
        }
        self.search_pending = false;
        self.search_error = Some("hocus did not answer".to_owned());
        true
    }

    /// Why the search in the box failed, wrapped to `cols`: the rows `render` draws under the
    /// box while it is open on the failure, and nothing once it is not. The footer used to lead
    /// with it, cut to the row, which lost both the end of the reason and the keys after it.
    fn search_failure_lines(&self, cols: usize) -> Vec<String> {
        match (&self.search_error, self.search_editing) {
            (Some(error), true) => wrap_words(&format!("failed: {error}"), cols),
            _ => Vec::new(),
        }
    }

    /// Stop typing a search, the way Esc in the box does: back to the query last applied and
    /// its matches, or out of the search altogether when nothing was applied yet.
    fn cancel_search_edit(&mut self) -> bool {
        let Some(query) = self.applied_search_query.clone() else {
            return self.clear_search();
        };
        self.search_request = self.search_request.wrapping_add(1);
        self.search_pending = false;
        self.search_editing = false;
        self.search_query = query;
        self.search_error = None;
        true
    }

    /// A bracketed paste, delivered whole rather than key by key.
    ///
    /// Taken only by a box being typed in, routed the way `on_key` routes a key, so it lands
    /// in whatever the next keystroke would have. Everywhere else the pane is a list of
    /// single-letter commands, and a paste replayed as keys - or a paste that merely looked as
    /// if it went somewhere - would fire them; so it goes nowhere, and says so by drawing
    /// nothing new.
    fn on_paste(&mut self, text: &str) -> bool {
        if self.search_editing {
            let line = single_line(text);
            if line.is_empty() {
                return false;
            }
            // The same bookkeeping a typed character does: the query is not the one any
            // reply in flight was for any more.
            self.search_query.push_str(&line);
            self.query_edited();
            return true;
        }
        if self.surface == Surface::Swamp && self.swamp.is_editing() {
            return self.swamp_paste(text);
        }
        // A todo box stays open under the tab bar, and a key the bar does not own is typed
        // into it (`typing`), so a paste is too - it is typing, and takes the keys down off
        // the bar with it, as a typed letter does. `todo_paste` takes nothing but that box,
        // so the bar still turns away every other paste.
        #[cfg(test)]
        if self.surface == Surface::Todos && self.todo_paste(text) {
            self.tabs_focused = false;
            return true;
        }
        false
    }

    /// Leave the search altogether, and say whether there was one to leave. A draft `Tab` kept
    /// goes with a search that is put away - `c` and `Esc`, or `w` and `p` over one - but not
    /// when there is no search: `w` and `p` call this only to leave whatever view the list is
    /// in, and the draft is kept for the next `/` until a search is really put away.
    fn clear_search(&mut self) -> bool {
        self.search_revisions.clear();
        self.pending_history_revision = None;
        self.applied_search_query = None;
        self.searched_at_ms = None;
        self.search_request = self.search_request.wrapping_add(1);
        self.search_pending = false;
        self.search_editing = false;
        let Some((rows, summaries)) = self.search_base.take() else {
            return false;
        };
        // The cursor stays on its row where the list it goes back to has it: a box opened and
        // left without a search - Tab stepping on from it, into the history of the agent it
        // was over - never moved the rows at all. It used to go back to the top, and the Tab
        // stepped into the first agent's history instead.
        let keep = self.selection_key();
        self.search_draft = None;
        self.rows = rows;
        self.summaries = summaries;
        self.search_query.clear();
        self.search_error = None;
        self.search_truncated = false;
        self.selected = keep
            .as_deref()
            .and_then(|key| position_of_key(&self.rows, key))
            .or_else(|| self.rows.iter().position(selectable))
            .unwrap_or(0);
        self.generation += 1;
        true
    }

    fn toggle_waiting(&mut self) -> bool {
        self.focus = Focus::List;
        self.list_scroll = None;
        if !self.clear_waiting() {
            self.clear_search();
            self.clear_predecessors();
            let strip = Strip {
                rows: self.rows.clone(),
                focused: self.focused,
                summaries: self.summaries.clone(),
            };
            self.waiting_base = Some(strip.clone());
            let was = self.selection_key();
            // Nothing kept: the view opening is the queue as it stands, and an agent the
            // cursor happened to be on is not in it unless it is waiting.
            self.set_rows_keeping(strip, None);
            // Then the cursor starts at the top of the queue. A view opening is not a row
            // vanishing inside it, so the neighbour `set_rows_keeping` hands such a row's cursor
            // to does not apply: it was found by the cursor's place in the whole list, and
            // landed on an arbitrary waiting agent - most often the last.
            if was.is_none_or(|key| position_of_key(&self.rows, &key).is_none()) {
                self.selected = self.rows.iter().position(selectable).unwrap_or(0);
            }
        }
        true
    }

    fn clear_waiting(&mut self) -> bool {
        let Some(strip) = self.waiting_base.take() else {
            return false;
        };
        self.list_scroll = None;
        self.set_rows(strip);
        true
    }

    /// One page of the history half: what it can show, less a line of overlap so a page turn
    /// does not lose the sentence that was straddling the seam.
    fn page(&self) -> usize {
        self.last_history_body_rows.saturating_sub(1).max(1)
    }

    fn on_mouse(&mut self, mouse_event: Mouse) -> bool {
        match mouse_event {
            Mouse::Hover(line, column)
            | Mouse::LeftClick(line, column)
            | Mouse::RightClick(line, column)
            | Mouse::Hold(line, column)
            | Mouse::Release(line, column) => self.mouse_position = Some((line, column)),
            _ => {},
        }
        if self.last_rows == 0 || self.last_cols == 0 {
            return false;
        }
        if let Some((line, column)) = self.mouse_position {
            if line < 0 || line as usize >= self.last_rows || column >= self.last_cols {
                return false;
            }
            if self.last_footer_row == Some(line as usize) {
                return false;
            }
        }
        // A click anywhere shuts the key sheet: whatever it landed on is under the sheet, so it
        // does nothing else - except on a tab, which is drawn above the sheet and does what a
        // tab click always does. The wheel scrolls it.
        let sheet_up = self.key_sheet_scroll.is_some();
        if sheet_up {
            match mouse_event {
                Mouse::LeftClick(..) | Mouse::RightClick(..) => self.key_sheet_scroll = None,
                Mouse::ScrollUp(lines) => return self.scroll_key_sheet(-(lines.max(1) as isize)),
                Mouse::ScrollDown(lines) => return self.scroll_key_sheet(lines.max(1) as isize),
                _ => return false,
            }
        }
        if let Mouse::LeftClick(0, column) = mouse_event {
            if let Some(target) = self
                .tab_cells
                .iter()
                .find(|cell| column >= cell.x && column < cell.x + cell.width)
                .map(|cell| cell.target)
            {
                // A click on a tab is a click on that tab's list: the keys go to the content,
                // as they do for a tab click in the conversation. It used to leave them on the
                // bar, where every key but the bar's own was dropped under a surface still
                // advertising its keys.
                self.swamp.cancel_filter();
                self.switch_surface(target);
                self.keys_to_the_list();
                return true;
            }
        }
        if sheet_up {
            return true;
        }
        if self.surface == Surface::Swamp && self.swamp.has_filter_editor() {
            let mouse = if self.native_tabs {
                match mouse_event {
                    Mouse::LeftClick(y, x) => Mouse::LeftClick(y - 1, x),
                    Mouse::RightClick(y, x) => Mouse::RightClick(y - 1, x),
                    Mouse::Hold(y, x) => Mouse::Hold(y - 1, x),
                    Mouse::Release(y, x) => Mouse::Release(y - 1, x),
                    Mouse::Hover(y, x) => Mouse::Hover(y - 1, x),
                    other => other,
                }
            } else {
                mouse_event
            };
            return self.swamp_filter_mouse(mouse);
        }
        if self.last_rows <= 1 || self.mouse_position.is_some_and(|(line, _)| line == 0) {
            return false;
        }
        // Where a key pressed just before this click would have gone. A click that brings the
        // keys to what it lands on - off the tab bar, or into this pane from another - only
        // points: its second half, the one that walks into an agent or opens an event, is for
        // a click made where the keys already were, which is what makes a second click the
        // Enter. Asked before the click moves anything.
        //
        // Not while the search box is being typed in, which takes every key ahead of the list
        // and its history: a click there only points. Taken as the Enter, it opened the reader
        // over the box - drawing the reader's keys while every key went on into the hidden query
        // - or walked out of the panel on the first click.
        let keys_were_here = self.keys_here() && !self.search_editing;
        // Whether the click took the keys off the tab bar, which is a redraw whatever it lands
        // on: the bar's highlight and its footer come down. Only a press does - the wheel and
        // a hover leave the bar with the keys.
        let mut left_bar = false;
        let mouse_event = match mouse_event {
            Mouse::LeftClick(line, column) => {
                left_bar = std::mem::take(&mut self.tabs_focused);
                if left_bar {
                    self.bar_focused
                        .retain(|client| *client != self.own_client_id);
                    if let Some(id) = self.pane_tabs {
                        let _ = focus_pane_tabs(id, false);
                    }
                }
                Mouse::LeftClick(line - 1, column)
            },
            // The drag and the release that follow a press are shifted with it, so a surface
            // sees all three in one set of coordinates and can tell a click - press and release
            // on one cell - from a drag that only selects text.
            Mouse::Hold(line, column) => Mouse::Hold(line - 1, column),
            Mouse::Release(line, column) => Mouse::Release(line - 1, column),
            other => other,
        };
        self.content_mouse(mouse_event, keys_were_here) || left_bar
    }

    /// A mouse event on the surface under the tab bar, in its own coordinates - content row `0`
    /// is the pane's row `1`. `keys_were_here` is whether the keys were in the surface's content
    /// before the event, which a click that lands on what is already chosen reads to tell a
    /// second click from the one that brought the keys back.
    fn content_mouse(&mut self, mouse_event: Mouse, keys_were_here: bool) -> bool {
        match self.surface {
            Surface::Swamp => return self.swamp_mouse(mouse_event),
            #[cfg(test)]
            Surface::Todos => return self.todo_mouse(mouse_event),
            Surface::Vms => return self.vm_mouse(mouse_event, keys_were_here),
            Surface::Agents => {},
            #[cfg(not(test))]
            Surface::Todos => {},
        }
        // With the reader up there is one thing on screen and the wheel is its. A click never
        // closes it - a stray click losing the user's place in a long event is worse than a
        // click that does nothing - and is taken only by a fold row, the one target the reader
        // draws: it points the cursor there, and a second one opens or shuts the fold.
        if self.reading.is_some() {
            return match mouse_event {
                Mouse::ScrollUp(lines) => self.scroll_reader(-(lines.max(1) as isize)),
                Mouse::ScrollDown(lines) => self.scroll_reader(lines.max(1) as isize),
                // Row 0 is the reader's divider; the body starts under it.
                Mouse::LeftClick(line, _) if line >= 1 => {
                    self.click_in_the_reader(line as usize - 1, keys_were_here)
                },
                _ => false,
            };
        }
        match mouse_event {
            // The view label above the list names the view, and nothing on it is a target.
            Mouse::LeftClick(line, column) if line >= 0 => {
                self.click_press = None;
                let Some(content) = (line as usize).checked_sub(self.last_label_rows) else {
                    return false;
                };
                let (redraw, act) = self.point_at(content, keys_were_here);
                self.click_press = act.map(|act| ClickPress { line, column, act });
                redraw
            },
            // A second click does its Enter when it comes up where it went down: see
            // `ClickPress`.
            Mouse::Release(line, column) => match self.click_press.take() {
                Some(press) if press.line == line && press.column == column => {
                    self.click_act(press.act)
                },
                _ => false,
            },
            Mouse::ScrollUp(lines) => self.scroll_surface(-(lines.max(1) as isize)),
            Mouse::ScrollDown(lines) => self.scroll_surface(lines.max(1) as isize),
            _ => false,
        }
    }

    /// The server sends Hover with the wheel's coordinates before each wheel event.
    fn scroll_surface(&mut self, delta: isize) -> bool {
        // on_mouse has checked the pane bounds and excluded the tab header. A wheel over the
        // view label scrolls the list it names.
        let target = match self
            .mouse_position
            .map(|(line, _)| (line as usize - 1).saturating_sub(self.last_label_rows))
        {
            Some(line) if line < self.last_list_rows => Focus::List,
            Some(line)
                if self.last_history_rows > 0
                    && line <= self.last_list_rows + self.last_history_rows =>
            {
                Focus::History
            },
            Some(_) => return false,
            None => self.focus,
        };
        if target == Focus::History {
            return self.scroll_history(-delta);
        }
        if self.last_list_rows == 0 {
            return false;
        }
        let max = self.rows.len().saturating_sub(self.last_list_rows);
        let start = self.list_scroll.unwrap_or(self.last_window_start).min(max);
        let next = start.saturating_add_signed(delta).min(max);
        if next == start {
            return false;
        }
        self.list_scroll = Some(next);
        true
    }

    /// A click on content line `line`, below the tab header.
    ///
    /// The half it lands in takes the keys - the pointer saying where the user is looking is
    /// the whole reason for having two halves rather than one. The first click on something
    /// points at it: in the list it moves the cursor onto the row, so the history half fills
    /// with the agent it points at and nothing is navigated to; in the history half it moves
    /// the cursor onto the event. A second click on what the cursor is already on, in the half
    /// that already has the keys, does what `Enter` does there - walks into the agent, or reads
    /// the event. Clicking only ever pointed, which left the mouse able to choose everything
    /// and do nothing with it.
    ///
    /// "Already has the keys" is `keys_were_here`, not only the stop: a click that brings them
    /// back from the tab bar, or from another pane, is saying where the user is looking, the
    /// same as one that brings them over from the other half - and a user clicking to get the
    /// keys back is clicking on whatever the cursor was left on, most often.
    ///
    /// The press and the release in one, for a test with no mouse to release. The pointer does
    /// the Enter half on the release instead (`content_mouse`, [`ClickPress`]).
    #[cfg(test)]
    fn on_click(&mut self, line: usize, keys_were_here: bool) -> bool {
        match self.point_at(line, keys_were_here) {
            (_, Some(act)) => self.click_act(act),
            (redraw, None) => redraw,
        }
    }

    /// The press of a click on content line `line`: what it points at, whether that is worth a
    /// redraw, and - for a second click, on what the cursor is already on in the half that
    /// already has the keys - the Enter it does once released. See `on_click`.
    fn point_at(&mut self, line: usize, keys_were_here: bool) -> (bool, Option<ClickAct>) {
        if self.last_history_rows > 0 && line >= self.last_list_rows {
            let was_history = self.focus == Focus::History && keys_were_here;
            self.focus = Focus::History;
            // Under the divider and whatever is pinned below it, the timeline's rows.
            let event = line
                .checked_sub(self.last_list_rows + 1 + self.last_history_body_top)
                .filter(|row| *row < self.last_history_body_rows)
                .and_then(|row| self.event_at_row(self.last_history_window_start + row));
            let Some(event) = event else {
                // The divider, the header or an empty row: the focus is all the click does -
                // and it is worth a redraw, because the divider draws it.
                return (!was_history, None);
            };
            if was_history && event == self.history_cursor {
                return (false, Some(ClickAct::Read(event)));
            }
            let moved = event != self.history_cursor;
            self.history_cursor = event;
            self.history_cursor_anchor = self.cursor_anchor();
            self.history_scrolled = false;
            return (moved || !was_history, None);
        }
        let was_list = self.focus == Focus::List && keys_were_here;
        self.focus = Focus::List;
        let Some(index) = click_target(
            &self.rows,
            self.last_window_start,
            self.last_list_rows,
            line,
        ) else {
            return (!was_list, None);
        };
        let again = index == self.selected && was_list;
        let moved = index != self.selected;
        // The window stays where it is drawn, so the row clicked stays under the pointer. It
        // used to be handed back to the selection, which re-centred the list on the row and
        // moved it away from the pointer - and the second click of the pair then landed on
        // another row, so in a list longer than its window a double click walked into nothing.
        self.list_scroll = Some(self.last_window_start);
        self.selected = index;
        match self.rows.get(index) {
            // Nothing to preview on this row, so the first click is the Enter.
            Some(StripRow::MoreRuns) => {
                self.ask_for_older_runs();
                (true, None)
            },
            // Only with the keys already in the list: the click that brings them back from the
            // history half, the tab bar or another pane onto the selected row is a click saying
            // where the user is looking, not one walking them out of the panel.
            Some(StripRow::Agent { .. }) if again => (false, Some(ClickAct::Activate(index))),
            _ => (moved || !was_list, None),
        }
    }

    /// The Enter half of a second click, once released: only while what it was pressed on is
    /// still what the cursor is on, since a strip or a key may have moved it in between.
    fn click_act(&mut self, act: ClickAct) -> bool {
        match act {
            ClickAct::Activate(index) => {
                if index == self.selected
                    && matches!(self.rows.get(index), Some(StripRow::Agent { .. }))
                {
                    self.activate();
                }
                false
            },
            ClickAct::Read(event) => {
                self.focus == Focus::History
                    && self.history_cursor == event
                    && self.reading.is_none()
                    && self.open_reader()
            },
        }
    }

    /// A click on row `row` of the reader's body: onto a fold points the cursor at it, and a
    /// second one opens or shuts it - a second click made with the keys already here, for the
    /// reason `on_click` gives. Anything else is ignored - a prose or raw reader has nothing to
    /// point at.
    fn click_in_the_reader(&mut self, row: usize, keys_were_here: bool) -> bool {
        if row >= self.last_reader_rows {
            return false;
        }
        let Some(scroll) = self.reading.as_ref().map(|reading| reading.scroll) else {
            return false;
        };
        let line = scroll + row;
        if line >= self.reader_lines().len() {
            return false;
        }
        let Some(fold) = self.fold_at(line) else {
            return false;
        };
        // Only the fold's own row, the one drawn as its target. `fold_at` also answers for the
        // body of an open fold, and a click there - or the press a drag to copy from it starts
        // with - shut the fold being read and wound the reader back to its header.
        if self.reader_starts().get(fold) != Some(&line) {
            return false;
        }
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        if reading.cursor == fold && keys_were_here {
            return self.toggle_fold();
        }
        // No winding: the fold is on screen, since it was just clicked. A redraw even on the
        // fold the cursor was already on: the keys came back with the click, and the reader's
        // divider says so.
        reading.cursor = fold;
        true
    }

    /// Walk the selection one row, and say whether it went anywhere.
    fn step_selection(&mut self, motion: Motion) -> bool {
        let next = step(&self.rows, self.selected, motion);
        let moved = next != self.selected;
        self.selected = next;
        let recentered = self.list_scroll.take().is_some();
        moved || recentered
    }

    /// Ask the daemon to describe the selected row now, and say whether the divider changed.
    ///
    /// Both kinds of row are worth asking about, and the daemon answers them differently: a
    /// live agent has a pane to read and a run the daemon is already holding, an archived one
    /// has neither and has to be fetched from the store. This pane knows nothing about that
    /// difference and should not - it names the row and lets the daemon choose the path,
    /// which is the same division of labour that has hocus computing the rows drawn here.
    ///
    /// Nothing is expected back on this message. The answer arrives the way every other
    /// change to a run does, as a fresh `AGENT_RUN_DETAIL` - or, for a pane with no hooks,
    /// with the next strip - so a summary asked for here and one that happened on its own
    /// reach the half by one road. See `summarizing` for how the hint knows it has.
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
        let asked = self.selection_key().map(|row| SummaryAsk {
            row,
            asked_at_ms: now_ms(),
            stale: false,
        });
        // A refusal outranks `summarising…` on the divider and lives until the cursor leaves
        // the row, so leaving it up here would make `s` on a row that has just refused an `r`
        // look like a key that does nothing. The note is an account of a question that has
        // been overtaken - `arm_resume` takes it down for the same reason. A stale hint is
        // replaced by the fresh one, which is a change too.
        let changed = self.summarizing.as_ref().map(|ask| (&ask.row, ask.stale))
            != asked.as_ref().map(|ask| (&ask.row, false))
            || self.resume_note.is_some();
        self.resume_note = None;
        self.summarizing = asked;
        changed
    }

    /// Whether a strip just taken answered an `s` on a live agent, taking the hint down if so.
    ///
    /// A pane with no hooks is summarised from its screen, and that summary reaches this pane
    /// with the strip rather than as a run - the one road `load_run` never sees.
    fn summary_in_the_strip(&mut self) -> bool {
        let Some(ask) = self.summarizing.as_ref() else {
            return false;
        };
        let answered = self.summaries.iter().any(|(pane_id, summary)| {
            ask.row == format!("pane:{pane_id}")
                && summary
                    .created_at_ms
                    .is_some_and(|made| made >= ask.asked_at_ms)
        });
        if answered {
            self.summarizing = None;
        }
        answered
    }

    /// Stop saying a summary is on its way once it has been asked for longer than the daemon
    /// ever takes, and say whether the divider changed. Nothing is ever told of a refusal, so
    /// this is the only ending a hint that nothing will answer can have - and the wording is
    /// careful: a summary that came back saying what the last one said is not news either.
    fn expire_summary_ask(&mut self, now: i64) -> bool {
        match self.summarizing.as_mut() {
            Some(ask) if !ask.stale && now - ask.asked_at_ms >= SUMMARY_WAIT_MS => {
                ask.stale = true;
                true
            },
            _ => false,
        }
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
    /// Nothing is expected back except `wire::AGENT_RESUME_RESULT`. A refusal is drawn on the
    /// row; a resume that worked puts the panel away, since hocus has by then walked the
    /// keyboard into the new pane under it - and the row leaves the archive on the next strip.
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

    /// Put the whole panel away, not this pane: the list and the conversation are one thing to
    /// the user and have one visibility. This half goes on its own - it has its own id and no
    /// round trip is needed for it - and hocus is asked for the other, being the only half that
    /// knows both ids. Sent even when there may be no hocus listening: the message launches
    /// nobody, so it is free.
    ///
    /// Everything that dismisses comes through here: `Esc`, which is the whole of what it does,
    /// and everything that follows it with a focus somewhere the panel would cover - `Enter` on
    /// an agent, `o` on a file, and a resume that worked. Not `D` and `F`, whose modal is drawn
    /// over the panel and hands the keys back to it when it is cancelled.
    fn hide_the_panel(&mut self) {
        self.end_visit();
        self.swamp.cancel_filter();
        self.swamp.pane_visible = false;
        // Ended just now, so the manifest that reports the pane suppressed is not a second end.
        self.pane_shown = false;
        self.vms.visible = false;
        #[cfg(test)]
        {
            self.panel_hides += 1;
        }
        hide_self();
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HIDE));
    }

    /// The panel has been put away, by whichever half: this pane's side of it.
    ///
    /// The list half's counterpart of hocus resetting its own ring stop on a hide. The keyboard
    /// stop goes back to the list - `Ctrl e a` promises the panel back "with the keyboard on the
    /// agent list", and used to bring it back on the tab bar, where `j` did nothing - and every
    /// question left armed is dropped, so it cannot take the first keys of a visit that may be
    /// an hour later. Those are questions the user started and can ask again with one key; a
    /// draft being typed is kept, as the conversation keeps its own.
    ///
    /// What the user was looking at stays: the surface, each surface's selection and scroll,
    /// an open reader and any filtered view. So does anything the keyboard leaving *without*
    /// a hide would touch - that only changes how the markers are drawn (`keys_here`).
    ///
    /// Run from every panel-level hide: `hide_the_panel`, the manifest reporting this pane
    /// suppressed (`own_pane_changed`), and hocus's `vms::VISIBILITY` going false. Never from
    /// the tab-level `Event::Visible`, which fires in pairs when hocus carries the panel to
    /// another Zellij tab without it ever leaving the screen.
    fn end_visit(&mut self) {
        self.tabs_focused = false;
        self.focus = Focus::List;
        self.click_press = None;
        self.vms.details = false;
        self.confirming_resume = None;
        // The key sheet is about the moment it was asked for, like the questions are.
        self.key_sheet_scroll = None;
        #[cfg(test)]
        self.todos_end_visit();
        self.cancel_vm_confirmation();
    }

    /// What the host last said about this pane: shown or not, and holding the keyboard or not.
    /// Whether that is worth a redraw.
    ///
    /// The shown-to-hidden edge ends the visit (`end_visit`); nothing else resets a stop. In
    /// particular the keyboard leaving does not - it would race `AGENT_STRIP_TAKE`, which can
    /// arrive before the manifest reporting the focus change, and a yield into the conversation
    /// has to come back to where it left - it only changes what `keys_here` says.
    ///
    /// A redraw only on a real edge of the keyboard while shown: the first manifest to name
    /// this pane is not one, since the markers were drawn for "assume focused" and a pane
    /// reported unfocused on its first manifest has nothing drawn focused yet; and a hidden
    /// pane has nothing on screen to correct.
    fn own_pane_changed(&mut self, visible: bool, focused: bool) -> bool {
        let was_shown = std::mem::replace(&mut self.pane_shown, visible);
        if was_shown && !visible {
            self.end_visit();
        }
        let focused = visible && focused;
        let was = self.has_keyboard.replace(focused);
        visible && was.is_some_and(|was| was != focused)
    }

    /// Whether a key pressed now reaches this surface's own content - the list, its history or
    /// details half, a box being typed in - which is what a focus marker drawn there claims.
    ///
    /// False when the host has given the keyboard to another pane, and while the panel's tab
    /// bar has it: in both cases `focus` still names the half the keys will come back to, and
    /// is drawn as unfocused until they do.
    fn keys_here(&self) -> bool {
        self.has_keyboard != Some(false) && !self.tabs_focused
    }

    /// Whether a key pressed now reaches the panel's tab bar: what the bar's highlight and its
    /// hint line claim.
    fn bar_has_keys(&self) -> bool {
        self.has_keyboard != Some(false) && self.tabs_focused
    }

    /// Off the tab bar and onto the surface's list - not its history or details half. What
    /// `Tab`, `Enter`, `↓`, `j` and `Esc` on the bar do, and what a click on a tab does.
    fn keys_to_the_list(&mut self) {
        self.tabs_focused = false;
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
        match self.surface {
            Surface::Agents => self.focus = Focus::List,
            Surface::Vms => self.vms.details = false,
            Surface::Todos | Surface::Swamp => {},
        }
    }

    /// Where the terminal cursor goes this frame, in pane coordinates: in the box that takes
    /// the next key, or nowhere. `rows` and `cols` are the surface's share of the pane - under
    /// the tab bar, above any footer - which is what each surface lays its box out in.
    ///
    /// One answer per frame because the host has one cursor per pane, and before any drawing
    /// because a host command has to be: whichever surface is up asks its own box, in the
    /// order `on_key` hands the keys out. Nowhere while another pane has the keyboard, or with
    /// the key sheet drawn over the box.
    fn text_cursor(
        &self,
        form_cursor: Option<(usize, usize)>,
        rows: usize,
        cols: usize,
    ) -> Option<(usize, usize)> {
        if self.has_keyboard == Some(false) || self.key_sheet_scroll.is_some() {
            return None;
        }
        match self.surface {
            // Swamp's boxes take the keys ahead of the bar, so the bar does not hide them.
            Surface::Swamp => form_cursor.or_else(|| self.swamp_query_cursor(rows, cols)),
            #[cfg(test)]
            Surface::Todos if self.keys_here() => self.todo_input_cursor(rows, cols),
            // The search box is the Agents surface's first row - the pane's second, under the
            // tab bar - and it too takes the keys ahead of the bar.
            Surface::Agents if self.search_editing && self.reading.is_none() && rows > 0 => {
                Some((self.search_box(cols).1, 1))
            },
            _ => None,
        }
    }

    /// The whole of what `Enter` on a list row does: put the panel away and hand the keyboard
    /// to the selected agent's pane, if there is one.
    ///
    /// **The panel goes first.** It is an overlay, so it is drawn over whatever the agent's
    /// pane is, and staying up would leave the user looking at the list rather than at the
    /// thing they just asked for. Hiding a focused overlay drops the keyboard onto whatever is
    /// underneath, which is why the focus call comes after rather than before: both are host
    /// commands queued from this one call, so the server applies them in this order and the
    /// focus is the one that lands. hocus hides on its own message a moment later, by which
    /// time it holds no keyboard to lose.
    ///
    /// The next `Ctrl e a` brings the panel back with the selection still on this agent, so
    /// the history half is a keystroke away rather than gone. That holds in the waiting view
    /// too, answered or not: the queue keeps the row the cursor is on (see `waiting_rows`), and
    /// an answered agent leaves it once the cursor moves on to the next.
    fn activate(&mut self) {
        let Some(StripRow::Agent { pane_id, .. }) = self.rows.get(self.selected) else {
            return;
        };
        let pane_id = *pane_id;
        #[cfg(test)]
        self.activations.push(pane_id);
        self.hide_the_panel();
        // the host resolves which tab holds the pane, so this works across
        // tabs
        focus_terminal_pane(TerminalId::new(pane_id), false, false);
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
        let keep = self.selection_key();
        self.set_rows_keeping(strip, keep.as_deref())
    }

    /// [`Self::set_rows`], with the waiting view keeping the row `keep` names whether or not
    /// it is still waiting.
    ///
    /// The queue is re-filtered on every strip, and it used to drop the agent the cursor was on
    /// the moment it was answered: the cursor jumped to the top of the queue, and the reader
    /// and history the user had open on it closed under them. Kept until the cursor leaves it,
    /// the row goes on the next strip after that - and a row that is gone altogether, its pane
    /// closed, hands the cursor to its neighbour rather than to the top.
    fn set_rows_keeping(&mut self, mut strip: Strip, keep: Option<&str>) -> bool {
        let filtered = self.predecessors.is_some() || self.waiting_base.is_some();
        if let Some(base) = &mut self.waiting_base {
            *base = strip.clone();
            strip.rows = waiting_rows(&strip.rows, keep);
        }
        if let Some(previous) = &mut self.predecessors {
            previous.base = strip.clone();
            strip.rows = previous.rows();
        }
        let Strip {
            rows,
            focused,
            summaries,
        } = strip;
        if self.search_base.is_some() {
            // The search's rows stand still, but its footer counts the waiting agents in the
            // strip kept behind it, so a strip that changes that count is worth a redraw. The
            // once-a-second push that changes nothing is not.
            let was_waiting = self.waiting_count();
            let was_live = self.live();
            if let Some(base) = self.search_base.as_mut() {
                *base = (rows, summaries);
            }
            self.focused = focused.or(self.focused);
            return self.live() != was_live || self.waiting_count() != was_waiting;
        }
        let walked_to = focused
            .filter(|_| !filtered)
            .filter(|_| focused != self.focused)
            .and_then(|pane_id| position_of(&rows, pane_id));
        let was = self.rows.get(self.selected).and_then(row_key);
        let selected = walked_to
            .or_else(|| was.as_deref().and_then(|key| position_of_key(&rows, key)))
            // `load older runs…` answered by the last page is replaced by the runs it asked
            // for, and the cursor goes to the first of them rather than back to the top of the
            // list, which is a page per row away from where the user was reading.
            .or_else(|| {
                (was.as_deref() == Some("more"))
                    .then(|| self.first_run_loaded(&rows))
                    .flatten()
            })
            .or_else(|| {
                filtered
                    .then(|| nearest_selectable(&rows, self.selected))
                    .flatten()
            })
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
        if rows.get(selected).and_then(row_key) != was {
            self.list_scroll = None;
        }
        self.rows = rows;
        self.selected = selected;
        self.summaries = summaries;
        self.focused = focused;
        changed
    }

    /// Where the cursor goes when `load older runs…` has gone from `rows`: the first run after
    /// the last one it followed, which is the first of the page it loaded - or, if that run has
    /// gone too, the last row that can be selected.
    fn first_run_loaded(&self, rows: &[StripRow]) -> Option<usize> {
        let last_run = self.rows[..self.selected.min(self.rows.len())]
            .iter()
            .rev()
            .find(|row| matches!(row, StripRow::Run { .. }))
            .and_then(row_key);
        last_run
            .and_then(|key| position_of_key(rows, &key))
            .map(|at| step(rows, at, Motion::Down))
            .or_else(|| rows.iter().rposition(selectable))
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
    /// absence - one that says which kind of empty it is (`history_placeholder`), wrapped over
    /// as many of the half's rows as it needs, since the reason is its end.
    fn render_history(&mut self, top: usize, rows: usize, cols: usize) {
        self.fill_history_cache(cols);
        // `rows` is the body alone: `split` takes the divider off the list's share, and it is
        // drawn on the row above the body at `top`.
        //
        // Only two things are held above the timeline, and both are about the run rather than
        // about anything it said: when it began, and its whole session id. A summary is *not*
        // one of them - it is a row of the timeline, where in time it was made - so the events
        // remain one scrolling window and a talkative model costs the events beside it nothing
        // they cannot be scrolled back to. A note that has taken the divider's place is held
        // there too while it stands, whole - a refused resume, a summary nobody wrote: the
        // divider cuts it to a few words on a narrow half, and the rest is what to do about it.
        let header_rows = self.header_rows(rows);
        let refusal = self.refusal_lines(cols);
        let refusal_rows = refusal
            .len()
            .min(rows.saturating_sub(header_rows).saturating_sub(1));
        let room = rows.saturating_sub(header_rows + refusal_rows);
        let entries_len = self.history().entries.len();
        let events_len = self.history().events.len();
        // Nothing to measure against on a frame with no room or no events, so nothing is
        // settled on one either: the positions and the events they hold wait for a frame that
        // can show them.
        let measurable = room > 0 && events_len > 0;
        if measurable {
            self.reanchor_history();
        }
        // `history_scroll` counts backward from the newest entry, which is the end the half is
        // anchored to while it is at the tail: an entry arriving pushes the window along rather
        // than leaving the user reading a stale top. Wound back, it holds its event instead.
        let max_scroll = entries_len.saturating_sub(room);
        self.last_history_max_scroll = max_scroll;
        self.last_history_body_rows = room;
        self.history_scroll = self.history_scroll.min(max_scroll);
        let stepped = self.focus == Focus::History;
        // The cursor is walked by `step_history_cursor`, which knows how many events there are
        // but not how many of their rows fit: how many lines the half has depends on the width,
        // and only `render` is told the width. So keeping the cursor and the window together
        // happens here, the same place and for the same reason `last_history_max_scroll` is
        // recorded - and which of the two gives way depends on which one the user moved.
        let anchored = room > 0 && self.position_search_hit();
        if measurable {
            self.history_cursor = self.history_cursor.min(events_len - 1);
            if stepped && self.history_scrolled && !anchored {
                // The page keys or the wheel moved the window: the cursor comes to it, so what
                // `Enter` and `o` act on is on screen. Winding the window back to the cursor
                // instead was what made both do nothing here.
                let end = entries_len - self.history_scroll;
                if let Some(cursor) = self.cursor_into_window(end.saturating_sub(room), end) {
                    self.history_cursor = cursor;
                    self.history_cursor_anchor = self.cursor_anchor();
                }
            } else if stepped || anchored {
                // A key moved the cursor, or a resize or a growing run moved the rows under it:
                // the window goes to the cursor.
                if let Some(reveal) = self.reveal_cursor(room) {
                    self.history_scroll = reveal.min(max_scroll);
                }
            }
            if stepped {
                self.history_scrolled = false;
            }
        }
        let end = entries_len - self.history_scroll;
        let start = end.saturating_sub(room);
        if measurable {
            self.history_bottom_anchor = self.bottom_anchor(end);
        }
        self.last_history_window_start = start;
        self.last_history_body_top = header_rows + refusal_rows;
        // Drawn as holding the keys only while a key pressed now would reach it. The cursor is
        // drawn then, and dimly on a search hit's row, so the list says which event matched.
        let focused = stepped && self.keys_here();
        let marked = focused || self.selected_revision().is_some();
        let cursor = marked
            .then(|| self.cursor_event().map(|event| event.start))
            .flatten();
        let cursor_style = markdown::MdStyle {
            dim: !focused,
            ..Default::default()
        };
        let placeholder = self.placeholder_lines(cols);
        // A refused resume is drawn in the error colour. The other note that takes the
        // divider's place, a summary nobody wrote, is news rather than a failure, and is dim
        // like every other hint in the half.
        let refused = self
            .resume_note
            .as_ref()
            .is_some_and(|(row, _)| self.selection_key().as_ref() == Some(row));
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
                paint(text, 0, body + index, Some(cols), None);
            }
            // A plain rule, never coloured: `render_divider`'s colour is what says where the
            // keyboard is, and there is one focusable rule per half. This one only separates
            // what is fixed from what scrolls.
            paint(
                Text::new(divider_line("", 0, 0, cols)).dim_range(..),
                0,
                body + history.header.len(),
                Some(cols),
                None,
            );
            body += header_rows;
        }
        for (index, line) in refusal.iter().take(refusal_rows).enumerate() {
            let text = Text::new(format!(" {line}"));
            let text = match refused {
                true => text.error_color_range(..),
                false => text.dim_range(..),
            };
            paint(text, 0, body + index, Some(cols), None);
        }
        body += refusal_rows;
        if history.entries.is_empty() {
            // Over the rows the timeline would have had, and at least one: the reason a
            // history is unavailable is the end of its sentence, which one cut row loses.
            for (index, line) in placeholder.iter().take(room.max(1)).enumerate() {
                paint(
                    Text::new(format!("  {line}")).dim_range(..),
                    0,
                    body + index,
                    Some(cols),
                    None,
                );
            }
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
                true => line.clone().prepend(">", cursor_style),
                false => line.clone().indent(1),
            };
            // Nothing is dimmed by where it sits: every event reads at the same weight, and a
            // summary is told apart by its bold label rather than by the rest of the timeline
            // being pushed into the background. Whatever the markdown asked for still stands.
            let text = markdown::style_text(&line.text, &line.styles, false);
            paint(text, 0, body + index, Some(cols), None);
        }
    }

    /// What the history half says when it has no timeline to draw, which depends on why: a half
    /// too narrow to wrap into, a row that has no history of its own, one whose history is on
    /// its way or could not be fetched, and a run that recorded nothing are five different
    /// things to do about it, and used to be one sentence.
    fn history_placeholder(&self, cols: usize) -> String {
        if cols.saturating_sub(2) < agents::HISTORY_MIN_COLS {
            return "too narrow for history".to_owned();
        }
        match self.rows.get(self.selected) {
            Some(StripRow::MoreRuns) => "Enter loads older runs".to_owned(),
            Some(StripRow::Run { .. }) => {
                let selection = self.selection_key();
                if let Some((row, reason)) = self.run_error.as_ref() {
                    if Some(row) == selection.as_ref() {
                        return format!("history unavailable: {reason}");
                    }
                }
                match self
                    .run
                    .as_ref()
                    .is_some_and(|run| self.selected_run_matches(run))
                {
                    true => "no events recorded".to_owned(),
                    false => "loading…".to_owned(),
                }
            },
            Some(StripRow::Agent { .. }) => "nothing to show yet".to_owned(),
            _ => "select an agent".to_owned(),
        }
    }

    /// [`Self::history_placeholder`], wrapped to the half's margins: the rows the empty half
    /// draws. Wrapped rather than cut, so a reason hocus gives for a history being unavailable
    /// is read to its end however narrow the half, and needs no shortening to fit one.
    fn placeholder_lines(&self, cols: usize) -> Vec<String> {
        wrap_words(&self.history_placeholder(cols), cols.saturating_sub(2))
    }

    /// Whatever has taken the divider's place (`Hint::Instead`), wrapped to the half: what the
    /// divider starts to say and a narrow half cuts off. That is a refused resume on the row
    /// under the cursor, or a summary asked for that nobody wrote - whose tail is the key that
    /// asks again.
    fn refusal_lines(&self, cols: usize) -> Vec<String> {
        match self.divider_hint() {
            Some(Hint::Instead(note)) => wrap_words(&note, cols.saturating_sub(2)),
            _ => Vec::new(),
        }
    }

    /// Put the window and the cursor back on the events they were holding (see
    /// `history_bottom_anchor` and `history_cursor_anchor`), in a history that may have grown,
    /// lost its oldest rows or been re-wrapped since they were placed. An event that has gone
    /// lets its anchor go, and the position it was counted at stands.
    fn reanchor_history(&mut self) {
        let history = self.history();
        let entries_len = history.entries.len();
        let events = &history.events;
        let cursor = self.history_cursor_anchor.map(|revision| {
            events
                .iter()
                .rposition(|event| event.revision == revision)
                .map(|index| events.len() - index - 1)
        });
        let scroll = self.history_bottom_anchor.map(|(revision, offset)| {
            events
                .iter()
                .find(|event| event.revision == revision)
                .map(|event| {
                    let row = event.start + offset.min(event.len.saturating_sub(1));
                    entries_len.saturating_sub(row + 1)
                })
        });
        match cursor {
            Some(Some(cursor)) => self.history_cursor = cursor,
            Some(None) => self.history_cursor_anchor = None,
            None => {},
        }
        match scroll {
            Some(Some(scroll)) => self.history_scroll = scroll,
            Some(None) => self.history_bottom_anchor = None,
            None => {},
        }
    }

    /// What the window holds on to while it is wound back from the tail: the event its bottom
    /// row belongs to and how far into it that row is. Anchored at the bottom edge because that
    /// is the edge a resize grows the window from, so the text being read stays put.
    fn bottom_anchor(&self, end: usize) -> Option<(Option<u64>, usize)> {
        if self.history_scroll == 0 || end == 0 {
            return None;
        }
        let row = end - 1;
        let events = &self.history().events;
        let index = events.partition_point(|event| event.end() <= row);
        let event = events.get(index).filter(|event| event.start <= row)?;
        Some((event.revision, row - event.start))
    }

    /// What the cursor holds on to off the tail: its event. `None` on the newest, which follows
    /// the tail.
    fn cursor_anchor(&self) -> Option<Option<u64>> {
        if self.history_cursor == 0 {
            return None;
        }
        self.cursor_event().map(|event| event.revision)
    }

    /// The cursor, as a count back from the newest, moved onto an event wholly inside the rows
    /// `start..end` if its own is not: the one nearest the edge it fell off, or where no event
    /// fits whole, the one owning that edge row. `None` when it is on screen already.
    fn cursor_into_window(&self, start: usize, end: usize) -> Option<usize> {
        let events = &self.history().events;
        let index = events.len().checked_sub(self.history_cursor + 1)?;
        let event = events.get(index)?;
        if event.start >= start && event.end() <= end {
            return None;
        }
        // Events tile the rows oldest first, so the ones wholly inside are a run of them.
        let first_inside = events.partition_point(|event| event.start < start);
        let past_inside = events.partition_point(|event| event.end() <= end);
        let owner = |row: usize| {
            let at = events.partition_point(|event| event.end() <= row);
            events
                .get(at)
                .filter(|event| event.start <= row)
                .map(|_| at)
        };
        let target = if event.end() > end {
            (first_inside < past_inside)
                .then(|| past_inside - 1)
                .or_else(|| owner(end.checked_sub(1)?))
        } else {
            (first_inside < past_inside)
                .then_some(first_inside)
                .or_else(|| owner(start))
        }?;
        Some(events.len() - target - 1)
    }

    /// The event a click on row `row` of the history's entries lands on, as a count back from
    /// the newest, or `None` for a row no event owns.
    fn event_at_row(&self, row: usize) -> Option<usize> {
        let events = &self.history().events;
        let at = events.partition_point(|event| event.end() <= row);
        events
            .get(at)
            .filter(|event| event.start <= row)
            .map(|_| events.len() - at - 1)
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
        // Above the top edge: wind back until its first row is the oldest on screen. On the
        // oldest event, measured from the first row instead where that still leaves the event's
        // first row in the room. The rows above `events[0]` belong to no event - they are the
        // notice that older events were not loaded - and a cursor walked to the oldest event is
        // exactly the user who wants to know whether there is anything older.
        let oldest = self.history_cursor + 1 == self.history().events.len();
        let head = match oldest && event.start < room {
            true => 0,
            false => event.start,
        };
        if head + room < entries_len.saturating_sub(scroll) {
            scroll = entries_len.saturating_sub(head + room);
        }
        (scroll != self.history_scroll).then_some(scroll)
    }

    /// The reader: one event of the timeline with the whole pane to itself.
    ///
    /// A divider of the same shape the halves have - what this is, and how far through it you
    /// are - and the event's markdown below it. Focused whenever the keys are here at all,
    /// since there is nowhere else in the pane for them to be while this is up.
    fn render_reader(&mut self, rows: usize, cols: usize) {
        // A row for the divider, the rest for the event. Settled before the winding below,
        // which is measured in the rows this frame has to give.
        let room = rows.saturating_sub(1);
        self.last_reader_rows = room;
        self.fill_reader_cache(cols);
        // The winding a fold key asked for, on the rows the cache now holds. See
        // `Reading::reveal`.
        if self.reading.as_ref().is_some_and(|reading| reading.reveal) {
            self.reveal_fold();
            if let Some(reading) = self.reading.as_mut() {
                reading.reveal = false;
            }
        }
        let Some(reading) = self.reading.as_ref() else {
            return;
        };
        let lines = self.reader_lines();
        let max_scroll = lines.len().saturating_sub(room);
        let scroll = reading.scroll.min(max_scroll);
        let label = self.reader_label();
        let end = (scroll + room).min(lines.len());
        // The cursor's fold marks its first row, in the margin every other row leaves blank —
        // the same shape the list and the history half both use to say where their cursor is.
        // Absent for a prose event, which has no folds to point at.
        //
        // Or, where the scroll has passed that row, the first of the fold's rows still on
        // screen: a fold taller than the pane is still the fold the keys act on, and a marker
        // nobody can see would leave the reader unable to say so.
        let cursor = self
            .reader_starts()
            .get(reading.cursor)
            .map(|&start| start.max(scroll));
        render_divider(&label, scroll, lines.len() - end, self.keys_here(), 0, cols);
        for (index, line) in lines[scroll..end].iter().enumerate() {
            let line = match cursor == Some(scroll + index) {
                true => line.clone().prepend(">", markdown::MdStyle::default()),
                false => line.clone().indent(1),
            };
            let text = markdown::style_text(&line.text, &line.styles, false);
            paint(text, 0, 1 + index, Some(cols), None);
        }
        // Settled last, for the same reason the history half settles its own here: only this
        // knows the width the body was wrapped to, and so only this knows how many lines it
        // came to.
        self.last_reader_max_scroll = max_scroll;
        if let Some(reading) = self.reading.as_mut() {
            reading.scroll = scroll;
        }
    }

    /// What the reader's divider says about the event: its kind first, then whether the source
    /// is showing, the envelope's tag and its age.
    ///
    /// Built per frame rather than held in `Reading`, so the age goes on ticking with the minute
    /// timer the way every other age on this pane does.
    fn reader_label(&self) -> String {
        let Some(reading) = self.reading.as_ref() else {
            return String::new();
        };
        let mut label = reading.kind.to_owned();
        // Said rather than left to be inferred, and straight after the kind, where a narrow
        // divider still has it: with the folds replaced by the source, every key in here does
        // something different, and a reader that looked the same either way would be a reader
        // nobody could tell they were in. Called `source`, the word `v` and every hint about it
        // use for this view.
        if reading.raw {
            label.push_str(" · source");
        }
        if let Some(tag) = reading.envelope.as_deref() {
            label.push_str(&format!(" · {tag}"));
        }
        if let Some(then_ms) = reading.created_at_ms {
            label.push_str(&format!(" · {}", agents::short_age(now_ms(), then_ms)));
        }
        // What the last `o` or `y` had to say leads, until the next key: it answers the key just
        // pressed, and a narrow divider keeps its head.
        if let Some(note) = self.open_note.as_deref() {
            label = format!("{note} · {label}");
        }
        label
    }

    /// Wind the reader so the cursor's fold is on screen, the way `reveal_cursor` does for the
    /// history half. Asked for by [`Reading::reveal`] and answered from `render_reader`, which
    /// is the one place that knows the width the folds were wrapped to.
    ///
    /// Head first, then foot: a fold taller than the room shows its beginning, which is the row
    /// carrying its name.
    fn reveal_fold(&mut self) {
        let room = self.last_reader_rows;
        let starts = self.reader_starts();
        let Some(reading) = self.reading.as_ref() else {
            return;
        };
        let Some(&start) = starts.get(reading.cursor) else {
            return;
        };
        let end = starts
            .get(reading.cursor + 1)
            .copied()
            .unwrap_or(self.reader_lines().len());
        let mut scroll = reading.scroll;
        if end > scroll + room {
            scroll = end.saturating_sub(room);
        }
        if start < scroll {
            scroll = start;
        }
        if let Some(reading) = self.reading.as_mut() {
            reading.scroll = scroll;
        }
    }

    /// [`Self::reader_lines`], computed once per event, fold and width. See
    /// `fill_history_cache` for why anything on this pane is cached at all.
    fn fill_reader_cache(&mut self, width: usize) {
        let Some(reading) = self.reading.as_ref() else {
            return;
        };
        let key = (reading.id, reading.fold, width);
        if self.reader_cache.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        // The same margin the history half's body keeps, at both edges.
        let inner = width.saturating_sub(2);
        let rendered = match reading.sections.is_empty() || reading.raw {
            // Prose, or the source somebody asked for: one document, and no fold to put a
            // cursor on.
            true => {
                if self
                    .reader_blocks
                    .as_ref()
                    .is_none_or(|(id, _)| *id != reading.id)
                {
                    self.reader_blocks = Some((reading.id, markdown::parse(&reading.text).into()));
                }
                let blocks = self
                    .reader_blocks
                    .as_ref()
                    .map_or(&[][..], |(_, blocks)| blocks);
                envelope::ReaderLines {
                    lines: markdown::wrap(blocks, inner, markdown::Options::compact()),
                    starts: Vec::new(),
                }
            },
            false => {
                let folds = match self.reader_folds.take() {
                    Some((id, folds)) if id == reading.id => folds,
                    _ => Vec::new(),
                };
                let mut folds = folds.into_iter();
                let folds: Vec<ParsedFold> = reading
                    .sections
                    .iter()
                    .map(|section| match folds.next() {
                        Some((open, blocks)) if open == section.open => (open, blocks),
                        _ => (section.open, envelope::section_blocks(section)),
                    })
                    .collect();
                let rendered = envelope::wrap_sections(
                    folds.iter().map(|(_, blocks)| blocks.as_slice()),
                    inner,
                );
                self.reader_folds = Some((reading.id, folds));
                rendered
            },
        };
        self.reader_cache = Some((key, rendered));
    }

    /// Whatever [`Self::fill_reader_cache`] last put there.
    fn reader_lines(&self) -> &[markdown::MdLine] {
        self.reader_cache
            .as_ref()
            .map(|(_, rendered)| rendered.lines.as_slice())
            .unwrap_or_default()
    }

    /// Where each fold's rows begin, for the cursor. Empty where there are no folds.
    fn reader_starts(&self) -> &[usize] {
        self.reader_cache
            .as_ref()
            .map(|(_, rendered)| rendered.starts.as_slice())
            .unwrap_or_default()
    }

    /// What the divider says: what the half is about, and whatever is happening to it.
    ///
    /// The hint is part of the label rather than drawn anywhere of its own, because the
    /// divider is already the row that names the half - and `fit_label` cuts it to the room
    /// left over, so where the hint goes decides what a narrow pane loses (see `Hint`).
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
            // A selected agent with neither a run nor a summary yet has no label at all, and
            // the first thing it ever says should not open or close with a separator.
            Some(Hint::Before(hint) | Hint::Beside(hint)) if label.is_empty() => hint,
            Some(Hint::Before(hint)) => format!("{hint} · {label}"),
            Some(Hint::Beside(hint)) => format!("{label} · {hint}"),
        }
    }

    /// What the divider has to say about the row under the cursor beyond naming it, if
    /// anything.
    ///
    /// At most one, and the question comes before the answer: an armed confirmation is what
    /// the user is being asked *now*, and a note from a refused resume is about a question
    /// already answered. `arm_resume` clears the note anyway, so the two can only overlap
    /// for as long as it takes a `y` to come back refused, but the order says which wins.
    /// The question goes before the label, since its tail is its answer keys; so does what an
    /// `o` just found, which answers the key just pressed.
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
            return Some(Hint::Before("resume? y/n".to_owned()));
        }
        if let Some((row, detail)) = self.resume_note.as_ref() {
            if Some(row) == selection.as_ref() {
                return Some(Hint::Instead(format!("resume failed: {detail}")));
            }
        }
        if let Some(note) = self.open_note.as_ref() {
            return Some(Hint::Before(note.clone()));
        }
        if let Some(ask) = self.summarizing.as_ref() {
            if Some(&ask.row) == selection.as_ref() {
                // Neutral about why: a refusal and a summary that said what the last one did
                // both look like nothing arriving.
                return Some(match ask.stale {
                    true => Hint::Instead("no new summary · s to try again".to_owned()),
                    false => Hint::Beside("summarising…".to_owned()),
                });
            }
        }
        // The waiting view keeps the row the cursor is on after it stops waiting, so it says
        // why a row without a `!` is in the queue.
        let answered = matches!(
            self.rows.get(self.selected),
            Some(StripRow::Agent { waiting: false, .. })
        );
        if self.waiting_base.is_some() && answered {
            return Some(Hint::Beside("no longer waiting".to_owned()));
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
        // The same question `history_lines` asks first, so the key names what it will draw.
        let source = if self
            .run
            .as_ref()
            .is_some_and(|run| self.selected_run_matches(run))
        {
            HistorySource::Run(self.run_revision)
        } else {
            HistorySource::Strip(self.generation)
        };
        let now = now_ms();
        let key = HistoryKey {
            selection: self.selection_key(),
            width,
            source,
            minute: now / 60_000,
        };
        if self.history_cache.as_ref().is_some_and(|(c, _)| *c == key) {
            return;
        }
        // The same run at the same width a minute on: only the ages moved, so the rows already
        // here are restamped in place rather than the whole timeline cloned out of the parse
        // again. The parse is the one that drew them, since it only changes when a run is
        // loaded, which moves `run_revision` - and that is in the key.
        if let (Some((previous, mut lines)), HistorySource::Run(_)) =
            (self.history_cache.take(), source)
        {
            let only_the_clock = previous.selection == key.selection
                && previous.width == key.width
                && previous.source == key.source;
            let restamped = only_the_clock
                && self
                    .run
                    .as_mut()
                    .is_some_and(|run| run.parsed.restamp(&mut lines, now));
            if restamped {
                self.history_cache = Some((key, lines));
                return;
            }
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
    fn history_lines(&mut self, width: usize) -> agents::HistoryLines {
        if self
            .run
            .as_ref()
            .is_some_and(|run| self.selected_run_matches(run))
        {
            let offset = self.run_offset_secs;
            if let Some(run) = self.run.as_mut() {
                return run.parsed.lines(width, now_ms(), offset);
            }
        }
        let Some(pane_id) = self.selection() else {
            return agents::HistoryLines::default();
        };
        let Some(summary) = self.summaries.get(&pane_id) else {
            return agents::HistoryLines::default();
        };
        let Some(StripRow::Agent { kind, waiting, .. }) = self.rows.get(self.selected) else {
            return agents::HistoryLines::default();
        };
        // The row's own `!`, which has already asked whether the summary is still about the
        // screen: the stored summary goes on saying it is waiting after the user answered.
        agents::summary_history(pane_id, kind, summary, *waiting, width, now_ms())
    }

    /// Whether anything this pane draws is an age - the one thing on it that goes stale on
    /// its own.
    ///
    /// Asked of every source that draws one, in the order the pane draws them, so the answer
    /// cannot claim an age nothing on screen is showing.
    fn shows_an_age(&self) -> bool {
        if self.surface == Surface::Swamp {
            return self.swamp.shows_an_age();
        }
        // The reader is the whole pane, so what it says is the only thing that can go stale.
        if let Some(reading) = self.reading.as_ref() {
            return reading.created_at_ms.is_some();
        }
        // A search's own age, on its label - the one age an empty "no matches" still shows.
        if self.search_base.is_some() && self.searched_at_ms.is_some() && !self.search_editing {
            return true;
        }
        // The archive table's age column is in the *list* half, so it ticks whatever the
        // history half beside it happens to be showing - and it is usually the only age on a
        // pane whose selected run has no stamped entry to draw one from.
        if self.rows.iter().any(|row| {
            matches!(
                row,
                StripRow::Run {
                    started_at_ms: Some(_),
                    ..
                }
            )
        }) {
            return true;
        }
        if let Some(run) = self
            .run
            .as_ref()
            .filter(|run| self.selected_run_matches(run))
        {
            return run.stamped;
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
        if moved {
            // The window the user moved is the one to keep: its old anchor would put it back.
            self.history_scrolled = true;
            self.history_bottom_anchor = None;
        }
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
        // The window follows a cursor a key moved, and the cursor holds the event it landed on.
        self.history_scrolled = false;
        self.history_cursor_anchor = self.cursor_anchor();
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
        let (kind, created_at_ms, text) = (event.kind, event.created_at_ms, event.text.clone());
        // The timeline's parse, where it parsed the whole event: the reader would only make
        // the same one again.
        let blocks = self
            .run
            .as_ref()
            .and_then(|run| run.parsed.whole_blocks(&text));
        let sections = event_sections(kind, &text);
        // An event with no folds has no envelope to name on the divider either, so the parse
        // is asked for only where the folds were.
        let tag = (!sections.is_empty())
            .then(|| {
                envelope::parse(&text)
                    .envelopes
                    .first()
                    .map(|found| found.tag.to_owned())
            })
            .flatten();
        self.reading_id += 1;
        self.reader_blocks = blocks.map(|blocks| (self.reading_id, blocks));
        self.reading = Some(Reading {
            kind,
            created_at_ms,
            text,
            scroll: 0,
            id: self.reading_id,
            sections,
            raw: false,
            cursor: 0,
            fold: 0,
            reveal: false,
            envelope: tag,
        });
        true
    }

    /// Put the reader away, and say whether there was one.
    ///
    /// The caches go with it: they are keyed on an id that never comes round again, so keeping
    /// them would only hold a copy of the whole event for as long as the pane is open.
    fn close_reader(&mut self) -> bool {
        self.reader_cache = None;
        self.reader_blocks = None;
        self.reader_folds = None;
        self.reading.take().is_some()
    }

    /// A key with the reader up. Everything in here is about the one event on screen.
    ///
    /// `Esc` closes the reader rather than dismissing the panel: the pane's Esc puts the whole
    /// panel away, and with a reader open the thing being dismissed is the reader. Every other
    /// key is swallowed, because the list and the halves are not on screen and a key that
    /// steered them would move something the user cannot see.
    fn read_key(&mut self, key: KeyWithModifier) -> bool {
        // `j`/`k` walk a *fold* rather than a line when there are folds, which is not a new
        // idiom: it is the one the history half already uses, and for the reason `on_key`
        // spells out there — a line step would leave the cursor pointing between two of them
        // with nothing for `Enter` to open. The arrows stay line-wise in both halves, so
        // nothing that scrolls today stops scrolling.
        let folding = self
            .reading
            .as_ref()
            .is_some_and(|reading| !reading.sections.is_empty() && !reading.raw);
        if key.has_no_modifiers() {
            match key.bare_key {
                BareKey::Char('k') if folding => return self.step_fold(-1),
                BareKey::Char('j') if folding => return self.step_fold(1),
                BareKey::Enter if folding => return self.toggle_fold(),
                BareKey::Char('o') => return self.open_the_file(),
                // Not `r`: one `Esc` from here `r` arms a resume of this same run, and `y`
                // answers it - so the reader's own `r` then `y` repeated there would start one.
                BareKey::Char('v') => return self.toggle_raw(),
                BareKey::Char('y') => return self.copy_the_event(),
                _ => {},
            }
        }
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

    /// Move the reader's cursor by `delta` folds, and say whether it moved.
    fn step_fold(&mut self, delta: isize) -> bool {
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        let last = reading.sections.len().saturating_sub(1);
        let target = match delta >= 0 {
            true => reading
                .cursor
                .saturating_add(delta.unsigned_abs())
                .min(last),
            false => reading.cursor.saturating_sub(delta.unsigned_abs()),
        };
        if target == reading.cursor {
            return false;
        }
        reading.cursor = target;
        reading.reveal = true;
        true
    }

    /// Open or shut the fold under the cursor. A fold with nothing to hide — a one-line field
    /// — has nothing to toggle, and says so by doing nothing.
    fn toggle_fold(&mut self) -> bool {
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        let cursor = reading.cursor;
        let Some(section) = reading.sections.get_mut(cursor) else {
            return false;
        };
        if !section.foldable() {
            return false;
        }
        section.open = !section.open;
        reading.fold += 1;
        // A fold opened at the foot of the pane spends its rows below the last one on screen,
        // and one shut at the head takes the rows under the cursor away. Either way the fold
        // the keys are on has moved, and it is asked for here rather than wound now because
        // the rows it moved to do not exist until the next `fill_reader_cache`.
        reading.reveal = true;
        true
    }

    /// Swap the folds for the event exactly as it arrived, and back.
    fn toggle_raw(&mut self) -> bool {
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        if reading.sections.is_empty() {
            // Nothing to swap: a prose event is already its own source.
            return false;
        }
        reading.raw = !reading.raw;
        reading.fold += 1;
        // Both halves of the swap are a different document, so there is no row to hold the
        // place at and the reader starts at the head of it — which for the folded view means
        // the first fold, the cursor included. Keeping a cursor from the other view would put
        // it forty rows below a scroll of zero, where nothing draws it.
        reading.scroll = 0;
        reading.cursor = 0;
        true
    }

    /// The file the keys would open: the fold under the reader's cursor, or the event under the
    /// history half's.
    ///
    /// Split out from the key handler because `open_file_floating` has no answer worth
    /// asserting on, and the decision — *which* file, and whether there is one — is the whole
    /// of what this pane contributes.
    ///
    /// `None` for a run recorded on another machine. The path in the envelope names a file in
    /// that machine's temp folder, and opening the same path here would either fail or, worse,
    /// open something else entirely.
    fn open_target(&self) -> Option<String> {
        if self.run.as_ref().is_some_and(|run| run.host.is_some()) {
            return None;
        }
        let path = self.named_path()?;
        // A relative path is relative to the folder the agent was working in, which is the one
        // thing the run knows and the envelope does not.
        if path.starts_with('/') {
            return Some(path);
        }
        let cwd = self.run.as_ref()?.cwd.as_deref()?;
        Some(format!("{}/{path}", cwd.trim_end_matches('/')))
    }

    /// The path the keys are pointing at, as the event spells it, whichever machine it is on:
    /// the fold under the reader's cursor, or the first one the event names. Apart from
    /// `open_target` so that `o` on another machine's run can tell a file it cannot open from
    /// no file at all.
    fn named_path(&self) -> Option<String> {
        match self.reading.as_ref() {
            // With the source up there is no cursor to read — the same reason `copy_target`
            // falls back to the whole event there — so the offer is the first path the event
            // names, which is what the history half offers too.
            Some(reading) if reading.raw => reading
                .sections
                .iter()
                .find_map(|section| section.path.clone()),
            Some(reading) => reading
                .sections
                .get(reading.cursor)
                .and_then(|section| section.path.clone()),
            None => {
                let event = self.cursor_event()?;
                event_sections(event.kind, &event.text)
                    .into_iter()
                    .find_map(|section| section.path)
            },
        }
    }

    /// What `y` would put on the clipboard: in a folding reader the fold under the `>`, and
    /// otherwise - a prose event, or the source `v` put up - the whole event exactly as it
    /// arrived. So `v` then `y` is how a folded event is copied whole: the thing itself, not
    /// our reading of it. `y` says which of the two it took (`copy_the_event`).
    fn copy_target(&self) -> Option<String> {
        let reading = self.reading.as_ref()?;
        let fold = (!reading.raw)
            .then(|| reading.sections.get(reading.cursor))
            .flatten();
        Some(match fold {
            Some(section) => section.text.clone(),
            None => reading.text.to_string(),
        })
    }

    /// Open the file the cursor names, or say why there is none.
    ///
    /// Floating, because the agents pane is an overlay and a tiled open would rearrange the
    /// layout underneath it - and opened for this client, which shows the floating layer and
    /// focuses the editor. The near-plugin placement did neither: the editor started hidden and
    /// unfocused, so `o` looked like a key that did nothing and each press left another one
    /// behind. The panel goes first, as `activate`'s does, since the editor is under it.
    fn open_the_file(&mut self) -> bool {
        let Some(path) = self.open_target() else {
            // Nothing to open is still an answer: a key that silently does nothing is one the
            // user cannot tell from a key that is not bound. Where the file is, only when there
            // is one: a fold that names no file names none on any machine.
            let on_another_host = self
                .run
                .as_ref()
                .and_then(|run| run.host.clone())
                .filter(|_| self.named_path().is_some());
            self.open_note = Some(match on_another_host {
                Some(host) => format!("file is on {host}"),
                None => "no file named here".to_owned(),
            });
            return true;
        };
        self.hide_the_panel();
        #[cfg(test)]
        self.opened_files.push(path.clone());
        // Waits on the host's answer, which a native test has nobody to give.
        #[cfg(not(test))]
        open_file_floating(FileToOpen::new(path), None, BTreeMap::new());
        // The panel is gone and the server paints the editor; nothing here to redraw.
        false
    }

    /// Copy what `copy_target` names, and say on the divider which it was: the fold or the whole
    /// event are two different things to paste, and a copy is otherwise invisible until then.
    /// The note leads the label, which `fit_label` cuts from the tail, and goes with the next
    /// key, as `o`'s does.
    fn copy_the_event(&mut self) -> bool {
        let Some(text) = self.copy_target() else {
            return false;
        };
        let fold = self
            .reading
            .as_ref()
            .filter(|reading| !reading.raw)
            .and_then(|reading| reading.sections.get(reading.cursor))
            .map(|section| section.name.clone());
        self.open_note = Some(match fold {
            Some(name) => format!("copied {name} · v then y copies the whole event"),
            None => "copied the whole event".to_owned(),
        });
        copy_to_clipboard(text);
        true
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
        let Some(scroll) = self.reading.as_ref().map(|reading| reading.scroll) else {
            return false;
        };
        let target = match delta >= 0 {
            true => scroll.saturating_add(delta.unsigned_abs()).min(max),
            false => scroll.saturating_sub(delta.unsigned_abs()),
        };
        if target == scroll {
            return false;
        }
        // The cursor comes along, so that what `Enter`, `y` and `o` act on is always something
        // on screen. A line scroll is how a long fold is read, and a cursor left behind at the
        // head of the event would leave those three keys working on a fold the user walked
        // away from ten pages ago — and no `>` anywhere to say which one.
        let cursor = self.fold_at(target);
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        reading.scroll = target;
        if let Some(cursor) = cursor {
            reading.cursor = cursor;
        }
        true
    }

    /// The fold whose rows the line `row` falls in.
    ///
    /// `None` where there are no folds to point at: a prose event, or the source `v` put up,
    /// both of which `fill_reader_cache` renders as one document with no spans.
    fn fold_at(&self, row: usize) -> Option<usize> {
        let starts = self.reader_starts();
        (!starts.is_empty()).then(|| {
            starts
                .partition_point(|&start| start <= row)
                .saturating_sub(1)
        })
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

    fn position_search_hit(&mut self) -> bool {
        let Some(revision) = self.pending_history_revision else {
            return false;
        };
        let Some(index) = self
            .history()
            .events
            .iter()
            .position(|event| event.revision == Some(revision))
        else {
            return false;
        };
        self.history_cursor = self.history().events.len() - index - 1;
        // Held on the hit even at the tail: the match is what the user searched for, and a
        // working agent's newer events must not walk the cursor off it.
        self.history_cursor_anchor = Some(Some(revision));
        self.pending_history_revision = None;
        true
    }

    fn selected_revision(&self) -> Option<u64> {
        self.selected_run_id()
            .and_then(|id| self.search_revisions.get(&id).copied())
    }

    fn selection_key(&self) -> Option<String> {
        self.rows.get(self.selected).and_then(row_key)
    }

    /// Take `run` as the selected run, reduced to a [`LoadedRun`]: parsed here, on arrival,
    /// and then dropped, so the parse is the one copy of its text the pane keeps. The parse a
    /// previous run left is brought up to date rather than started again - a run sent again
    /// with one more entry costs that entry.
    fn load_run(&mut self, run: agents::AgentRun, fingerprint: (usize, u64)) {
        // Once per run, here rather than in the draw path. The offset is asked for the
        // instant the run began, not for now, so a run from before a daylight-saving change
        // is read back with the offset it started under.
        self.run_offset_secs = get_local_utc_offset(zellij_tile::prelude::SignedUnixMillis::new(
            run.started_at_ms,
        ))
        .unwrap_or(0);
        let parsed = match self.run.take() {
            Some(LoadedRun { mut parsed, .. }) => {
                parsed.update(&run);
                parsed
            },
            None => agents::ParsedRun::new(&run),
        };
        let stamped = run
            .entries
            .iter()
            .any(|entry| entry.created_at_ms.is_some())
            || run
                .summary
                .as_ref()
                .is_some_and(|summary| summary.created_at_ms.is_some());
        let answered = self
            .summarizing
            .as_ref()
            .is_some_and(|ask| answers(ask, &run));
        self.run = Some(LoadedRun {
            ended: run.ended_at_ms.is_some(),
            pane_id: run.pane_id,
            stamped,
            fingerprint,
            parsed,
            id: run.id,
            host: run.host,
            cwd: run.cwd,
        });
        self.run_revision += 1;
        self.run_error = None;
        // A summary asked for arrives inside one of these, as an entry on the timeline, and
        // only a summary stamped after the press answers it: clearing on any change used to
        // take the hint down on a busy agent's next message, which read as "done" when the
        // summary had not come - and might never.
        if answered {
            self.summarizing = None;
        }
    }

    /// [`Self::load_run`] with the fingerprint the run's own encoding gives it, as though
    /// hocus had sent it.
    #[cfg(test)]
    fn set_run(&mut self, run: agents::AgentRun) {
        let payload = serde_json::to_string(&run).expect("encoded");
        self.load_run(run, agents::run_payload_fingerprint(&payload));
    }

    fn selected_run_matches(&self, run: &LoadedRun) -> bool {
        match self.rows.get(self.selected) {
            Some(StripRow::Agent { pane_id, .. }) => *pane_id == run.pane_id && !run.ended,
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
        let tabs_focused = stop == Some(hocus_proto::vms::STOP_TABS);
        let tabs_changed = self.tabs_focused != tabs_focused;
        self.tabs_focused = tabs_focused;
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
        if tabs_focused {
            self.bar_focused.push(self.own_client_id);
        }
        if self.native_tabs {
            if let Some(id) = self.pane_tabs {
                let _ = focus_pane_tabs(id, tabs_focused);
            }
        }
        if self.surface == Surface::Vms {
            self.vms.details = stop == Some(wire::STOP_HISTORY) && self.vms.last_details_rows > 0;
            return true;
        }
        if matches!(self.surface, Surface::Todos | Surface::Swamp) {
            return true;
        }
        let focus = match stop {
            Some(wire::STOP_HISTORY) => Focus::History,
            _ => Focus::List,
        };
        let changed = tabs_changed || self.focus != focus;
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
            plugin_id: self.own_plugin_id,
            pane_id: self.selection(),
            run_id: self.selected_run_id(),
            revision: self.selected_revision(),
        };
        let Ok(payload) = serde_json::to_string(&selection) else {
            return;
        };
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SELECTION).with_payload(payload));
    }
}

/// The folds an event of this kind opens into, and empty for one that has none.
///
/// A prompt is the one kind a harness writes as well as a person, which is the same gate
/// `agents::run_history` puts on its own digest — and the two have to agree, or the timeline
/// draws an event one way and `Enter` reads it another. An agent's own message quoting a
/// `<system-reminder>` at the start of a line is prose *about* markup: the scanner admits that
/// shape knowingly (see `envelope`'s module doc), which costs a prompt one fold row and would
/// cost a `result:` the whole of what the reader was opened to read.
fn event_sections(kind: &str, text: &str) -> Vec<envelope::Section> {
    match kind == agents::RunEntryKind::Prompt.label() {
        true => envelope::sections(text),
        false => Vec::new(),
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

/// This pane, as a manifest describes it, if the manifest names it at all. A `PaneUpdate`
/// reaches the active tab only, and says nothing about a pane in any other.
fn own_pane(manifest: &PaneManifest, own_plugin_id: PluginId) -> Option<&PaneInfo> {
    manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == PaneId::Plugin(own_plugin_id))
}

/// A paste as one line of text, for a box that has only the one.
///
/// Every box in this pane is a single line, and a newline arriving in one must not read as the
/// Enter that submits it: each line break becomes one space, with the blank lines and the
/// whitespace either side of the break dropped - what a terminal copy leaves at the ends of
/// its lines. A paste with no line break keeps its spacing as it is. Any other control
/// character becomes a space too: an escape sequence in a paste must not reach a box that is
/// drawn back to the terminal.
fn single_line(text: &str) -> String {
    let pieces: Vec<&str> = text.split(['\n', '\r']).collect();
    let last = pieces.len() - 1;
    let mut line = String::with_capacity(text.len());
    for (index, piece) in pieces.into_iter().enumerate() {
        let piece: String = piece
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let piece = match (index == 0, index == last) {
            (true, true) => piece.as_str(),
            (true, false) => piece.trim_end(),
            (false, true) => piece.trim_start(),
            (false, false) => piece.trim(),
        };
        if piece.is_empty() {
            continue;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(piece);
    }
    line
}

/// Whether `run` answers the `s` `ask` is waiting on: it is the run of the row asked about, and
/// it carries a summary made at or after the press - an entry somebody asked for, or the
/// run's rolling one. Any other change to the run is the run moving on, not an answer.
fn answers(ask: &SummaryAsk, run: &agents::AgentRun) -> bool {
    let about = ask.row == format!("run:{}", run.id)
        || (ask.row == format!("pane:{}", run.pane_id) && run.ended_at_ms.is_none());
    let since = |made: Option<i64>| made.is_some_and(|made| made >= ask.asked_at_ms);
    about
        && (run
            .entries
            .iter()
            .any(|entry| entry.kind == agents::RunEntryKind::Summary && since(entry.created_at_ms))
            || run
                .summary
                .as_ref()
                .is_some_and(|summary| since(summary.created_at_ms)))
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

fn waiting_row(row: &StripRow) -> bool {
    matches!(row, StripRow::Agent { waiting: true, .. })
}

/// Keep the project and branch context of waiting agents, without empty headers or archive -
/// and the row `keep` names, waiting or not, which is the one the cursor is on: see
/// `State::set_rows_keeping`.
fn waiting_rows(rows: &[StripRow], keep: Option<&str>) -> Vec<StripRow> {
    let mut filtered = Vec::new();
    let mut headers = Vec::new();
    for row in rows {
        match row {
            StripRow::Project(_) => {
                headers.clear();
                headers.push(row.clone());
            },
            StripRow::Branch(_) => {
                if matches!(headers.last(), Some(StripRow::Branch(_))) {
                    headers.pop();
                }
                headers.push(row.clone());
            },
            row if waiting_row(row)
                || (keep.is_some()
                    && matches!(row, StripRow::Agent { .. })
                    && row_key(row).as_deref() == keep) =>
            {
                filtered.append(&mut headers);
                filtered.push(row.clone());
            },
            _ => {},
        }
    }
    filtered
}

/// The selectable row nearest `at` in `rows`: at it or after it first, the way a list closes
/// up over a row that has gone, and before it only at the end.
fn nearest_selectable(rows: &[StripRow], at: usize) -> Option<usize> {
    let at = at.min(rows.len());
    (at..rows.len())
        .find(|index| selectable(&rows[*index]))
        .or_else(|| (0..at).rev().find(|index| selectable(&rows[*index])))
}

/// A search snippet from the first word of `query` it contains, with a few characters of what
/// led up to it: the list's cell is narrow, and the part of a snippet that says why it matched
/// is the part a cell cut from the tail used never to reach. Matched without regard to case,
/// and counted in characters so a multi-byte one cannot split. A snippet the word is not in,
/// or a query with no word, is left as it was.
fn hit_first(snippet: &str, query: &str) -> String {
    /// How much of what comes before the match is kept, for the sense of where it is.
    const LEAD: usize = 8;
    let Some(word) = query
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .find(|word| !word.is_empty())
    else {
        return snippet.to_owned();
    };
    let folded = |ch: &char| ch.to_lowercase().collect::<String>();
    let haystack: Vec<char> = snippet.chars().collect();
    let needle: Vec<char> = word.chars().collect();
    let Some(at) = (0..haystack.len()).find(|&at| {
        haystack.len() - at >= needle.len()
            && haystack[at..at + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(a, b)| folded(a) == folded(b))
    }) else {
        return snippet.to_owned();
    };
    if at <= LEAD {
        return snippet.to_owned();
    }
    let kept: String = haystack[at - LEAD..].iter().collect();
    format!("…{}", kept.trim_start())
}

/// What `D` and `F` launch the New agent modal with: the agent to duplicate or fork, named the
/// way `Ctrl e d` and `Ctrl e f` name the focused pane, and the overlay the modal opens in.
fn pane_action_configuration(pane_id: u32, fork: bool) -> BTreeMap<String, String> {
    let action = match fork {
        true => "fork_focused",
        false => "duplicate_focused",
    };
    BTreeMap::from([
        ("caller_terminal_pane_id".to_owned(), pane_id.to_string()),
        (action.to_owned(), "true".to_owned()),
        ("overlay".to_owned(), "true".to_owned()),
        ("modal".to_owned(), "true".to_owned()),
    ])
}

/// A key sheet from borrowed rows.
fn sheet(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(keys, what)| ((*keys).to_owned(), (*what).to_owned()))
        .collect()
}

/// One entry of the key sheet, in `cols` columns: the keys in a column `key_width` wide and
/// what they do beside it, each wrapped between words onto as many rows as it needs, with the
/// descriptions lined up whatever the keys' widths. Beside each row, how many of its characters
/// are keys and their margin, which is the part drawn as keys - none on a row the keys have
/// already ended above.
///
/// Wrapped rather than cut: a sheet exists to say what every key does, and at the widths a
/// panel is drawn the description column is ten to twenty columns, where cutting stopped most
/// descriptions mid-word. Keys wider than their column, such as the Todos box's caret keys,
/// break at the spaces around their ` / `s. Each row is still cut to `cols`, for a pane too
/// narrow for the margins themselves.
fn key_sheet_lines(keys: &str, what: &str, key_width: usize, cols: usize) -> Vec<(String, usize)> {
    let keys = wrap_words(keys, key_width);
    let what = wrap_words(what, cols.saturating_sub(1 + key_width + 2));
    (0..keys.len().max(what.len()).max(1))
        .map(|row| {
            let piece = keys.get(row).map_or("", String::as_str);
            let mut line = format!(" {piece}");
            let key_chars = match piece.is_empty() {
                true => 0,
                false => line.chars().count(),
            };
            if let Some(what) = what.get(row) {
                line.push_str(&" ".repeat(key_width.saturating_sub(piece.width()) + 2));
                line.push_str(what);
            }
            (agents::cut_to_width(line, cols), key_chars)
        })
        .collect()
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
/// `j`/`k` are the vim spelling of these, matched in `agents_key` because they are reserved
/// there for unmodified presses. Both pairs follow the focus there: rows in the list, events in
/// the history half.
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

    #[test]
    fn pane_tabs_descriptors_keep_originating_cwd_on_retries() {
        let origin = std::path::PathBuf::from("/tmp/hocus-origin");
        assert_ne!(origin, std::env::current_dir().unwrap());
        let state = State {
            initial_cwd: origin.clone(),
            ..Default::default()
        };
        for _ in 0..2 {
            let tabs = state.composition_tabs();
            assert_eq!(tabs.len(), 4);
            assert!(tabs
                .iter()
                .all(|tab| tab.initial_cwd.as_ref() == Some(&origin)));
        }
    }

    #[test]
    fn fixed_views_subscribe_only_to_their_own_services() {
        for surface in [Surface::Agents, Surface::Vms, Surface::Swamp] {
            let subscriptions = surface.subscriptions();
            assert_eq!(
                subscriptions.contains(&EventType::SessionUpdate),
                surface == Surface::Vms
            );
            assert_eq!(
                subscriptions.contains(&EventType::RunCommandResult),
                surface == Surface::Swamp
            );
            assert_eq!(
                subscriptions.contains(&EventType::WebRequestResult),
                surface == Surface::Swamp
            );
            assert!(!subscriptions.contains(&EventType::PluginDataChanged));
            assert!(subscriptions.contains(&EventType::Visible));
            assert!(subscriptions.contains(&EventType::PastedText));
        }
    }

    /// The row an `s` is waiting on, if any.
    fn asked(state: &State) -> Option<&str> {
        state.summarizing.as_ref().map(|ask| ask.row.as_str())
    }

    fn search_reply(state: &mut State, ids: &[&str]) -> wire::SearchResult {
        state.begin_search();
        state.search_query = "history".to_owned();
        state.search_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.search_pending);
        wire::SearchResult {
            request_id: state.search_request.to_string(),
            runs: ids
                .iter()
                .map(|id| wire::SearchRun {
                    run_id: (*id).to_owned(),
                    kind: "claude".to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn predecessor_reply(state: &State, ids: &[&str]) -> wire::PredecessorsResult {
        let mut anchor = a_run().header();
        anchor.id = "anchor".into();
        wire::PredecessorsResult {
            request_id: state.predecessor_request.to_string(),
            pane_id: state.predecessors.as_ref().unwrap().pane_id,
            anchor: Some(anchor),
            runs: ids
                .iter()
                .map(|id| {
                    let mut header = a_run().header();
                    header.id = (*id).into();
                    header
                })
                .collect(),
            diagnostic: String::new(),
        }
    }

    #[test]
    fn predecessors_select_history_keep_live_updates_and_restore_latest_archive() {
        let mut state = State::default();
        let mut rows = strip();
        rows.extend(archive());
        state.set_rows(unfocused(rows));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('p'))));
        let reply = predecessor_reply(&state, &["nearest", "oldest"]);
        assert!(state.apply_predecessors(reply));
        assert_eq!(state.selected_run_id().as_deref(), Some("nearest"));
        assert_eq!(state.selected_revision(), None);
        assert!(!state
            .rows
            .iter()
            .any(|row| matches!(row, StripRow::MoreRuns)));
        let mut run = a_run();
        run.id = "nearest".into();
        assert!(state.pipe(PipeMessage::new(
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
            wire::AGENT_RUN_DETAIL,
            &Some(serde_json::to_string(&run).unwrap()),
            &None,
            false
        )));
        assert_eq!(state.run.as_ref().unwrap().id, "nearest");
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert_eq!(state.confirming_resume.as_deref(), Some("run:nearest"));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('n'))));
        assert!(state.step_selection(Motion::Down));
        assert_eq!(state.selected_run_id().as_deref(), Some("oldest"));
        let mut incoming = with_summary(strip(), 7, "new summary", false);
        incoming.rows[2] = agent(3, "new run after /clear");
        incoming.rows.push(agent(99, "new live agent"));
        incoming.rows.extend(archive());
        incoming.focused = Some(99);
        state.set_rows(incoming.clone());
        assert_eq!(state.selected_run_id().as_deref(), Some("oldest"));
        assert!(position_of(&state.rows, 99).is_some());
        assert_eq!(state.summaries, incoming.summaries);
        assert_eq!(
            state
                .predecessors
                .as_ref()
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .anchor
                .as_ref()
                .unwrap()
                .id,
            "anchor"
        );
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('c'))));
        assert_eq!(state.rows, incoming.rows);
        assert_eq!(state.summaries, incoming.summaries);
    }

    #[test]
    fn predecessors_replacement_clear_search_and_stale_replies() {
        for action in ["replace", "clear", "search"] {
            let mut state = State::default();
            state.set_rows(unfocused(strip()));
            state.begin_predecessors();
            let stale = predecessor_reply(&state, &["stale"]);
            match action {
                "replace" => {
                    state.selected = position_of(&state.rows, 7).unwrap();
                    assert!(state.begin_predecessors());
                },
                "clear" => {
                    state.clear_predecessors();
                },
                _ => {
                    state.begin_search();
                },
            }
            assert!(!state.apply_predecessors(stale));
            if action == "replace" {
                let reply = predecessor_reply(&state, &["replacement"]);
                assert!(state.apply_predecessors(reply));
                assert_eq!(state.selected_run_id().as_deref(), Some("replacement"));
            }
        }
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let stale = search_reply(&mut state, &["stale"]);
        assert!(state.begin_predecessors());
        assert!(state.search_base.is_none());
        assert!(!state.apply_search_result(stale));
        let empty = predecessor_reply(&state, &[]);
        assert!(state.apply_predecessors(empty.clone()));
        assert!(!state.apply_predecessors(empty));
        assert!(state.rows.iter().any(
            |row| matches!(row, StripRow::Project(label) if label == "No predecessors recorded")
        ));
        // Esc steps back out of the view first, and only then is the dismissal it always was.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(
            state.predecessors.is_none(),
            "the first Escape leaves the view"
        );
        assert_eq!(state.panel_hides, 0);
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert_eq!(state.panel_hides, 1, "and the second puts the panel away");
        state.begin_predecessors();
        let mut partial = predecessor_reply(&state, &["reachable"]);
        partial.diagnostic = "Incomplete chain: missing run lost".into();
        assert!(state.apply_predecessors(partial));
        assert_eq!(state.selected_run_id().as_deref(), Some("reachable"));
        assert!(state.rows.iter().any(
            |row| matches!(row, StripRow::Project(label) if label.contains("Incomplete chain"))
        ));
    }

    #[test]
    fn search_revisions_reposition_with_list_focus_and_refresh_preserves_scroll() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        assert_eq!(state.selected_revision(), None);
        let mut run = a_run();
        run.entries = (1..=600)
            .map(|revision| agents::RunEntry {
                revision,
                text: format!("message {revision}"),
                kind: agents::RunEntryKind::Message,
                created_at_ms: Some(revision as i64),
                event_id: None,
            })
            .collect();
        state.focus = Focus::List;
        for revision in [10, 20] {
            let mut reply = search_reply(&mut state, &[&run.id]);
            reply.runs[0].revision = revision;
            assert!(state.apply_search_result(reply));
            assert_eq!(state.selected_revision(), Some(revision));
            // A detail already in flight for the preceding hit must not settle this anchor.
            assert!(!state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &Some(serde_json::to_string(&run).unwrap()),
                &Some(BTreeMap::from([(
                    "revision".to_owned(),
                    (revision - 1).to_string()
                )])),
                false,
            )));
            state.render(40, 80);
            assert_eq!(state.pending_history_revision, Some(revision));
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &Some(serde_json::to_string(&run).unwrap()),
                &Some(BTreeMap::from([(
                    "revision".to_owned(),
                    revision.to_string(),
                )])),
                false,
            ));
            state.render_history(0, 0, 80);
            assert_eq!(state.pending_history_revision, Some(revision));
            state.render(40, 80);
            assert_eq!(state.cursor_event().unwrap().revision, Some(revision));
            let event = state.cursor_event().unwrap();
            let end = state.history().entries.len() - state.history_scroll;
            assert!(
                event.start < end && event.end() > end.saturating_sub(state.last_history_body_rows)
            );
            state.scroll_history(-3);
            let scroll = state.history_scroll;
            run.entries.last_mut().unwrap().text.push('!');
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &Some(serde_json::to_string(&run).unwrap()),
                &Some(BTreeMap::from([(
                    "revision".to_owned(),
                    revision.to_string(),
                )])),
                false,
            ));
            state.render(40, 80);
            assert_eq!(state.history_scroll, scroll);
        }
        state.clear_search();
        assert_eq!(state.selected_revision(), None);
        let selection: wire::Selection =
            serde_json::from_str(r#"{"plugin_id":0,"run_id":"old"}"#).unwrap();
        assert_eq!(selection.revision, None);
    }

    #[test]
    fn cancelling_an_initial_search_restores_live_rows_and_summaries() {
        for pending in [false, true] {
            let mut state = State::default();
            state.set_rows(unfocused(strip()));
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
            let reply = pending.then(|| search_reply(&mut state, &["stale"]));
            let mut incoming = with_summary(strip(), 7, "While editing", false);
            incoming.rows.push(agent(10, "new agent"));
            state.set_rows(incoming.clone());
            assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
            assert!(state.search_base.is_none());
            assert!(!state.search_editing);
            assert!(!state.search_pending);
            assert!(state.search_query.is_empty());
            assert_eq!(state.rows, incoming.rows);
            assert_eq!(state.summaries, incoming.summaries);
            if let Some(reply) = reply {
                assert!(!state.apply_search_result(reply));
            }
            incoming.rows.push(agent(11, "another agent"));
            incoming.summaries.get_mut(&7).unwrap().text = "After Escape".to_owned();
            assert!(state.set_rows(incoming.clone()));
            assert_eq!(state.rows, incoming.rows);
            assert_eq!(state.summaries, incoming.summaries);
        }
    }

    #[test]
    fn cancelling_edits_restores_the_applied_query_and_matches() {
        for pending in [false, true] {
            let mut state = State::default();
            state.set_rows(unfocused(strip()));
            let reply = search_reply(&mut state, &["match-1", "match-2"]);
            assert!(state.apply_search_result(reply));
            state.selected = 1;
            let matches = state.rows.clone();
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
            if pending {
                assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
            }
            let stale = wire::SearchResult {
                request_id: state.search_request.to_string(),
                ..Default::default()
            };
            assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
            assert_eq!(state.search_query, "history");
            assert_eq!(state.applied_search_query.as_deref(), Some("history"));
            assert_eq!(state.rows, matches);
            assert_eq!(state.selected, 1);
            assert!(!state.search_editing);
            assert!(!state.search_pending);
            assert!(!state.apply_search_result(stale));
            state.set_rows(unfocused(vec![agent(10, "new agent")]));
            assert_eq!(state.rows, matches);
            assert!(state.clear_search());
            assert!(state.applied_search_query.is_none());
        }
    }

    #[test]
    fn editing_a_pending_query_ignores_stale_replies_and_accepts_the_revised_query() {
        for key in [BareKey::Char('x'), BareKey::Backspace] {
            for error in ["", "stale error"] {
                let mut state = State::default();
                state.set_rows(unfocused(strip()));
                let original = state.rows.clone();
                let mut stale = search_reply(&mut state, &["match"]);
                stale.error = error.to_owned();
                assert!(state.on_key(KeyWithModifier::new(key)));
                let revised = state.search_query.clone();
                assert_eq!(
                    revised,
                    if key == BareKey::Backspace {
                        "histor"
                    } else {
                        "historyx"
                    }
                );
                assert!(!state.search_pending);
                assert!(!state.apply_search_result(stale.clone()));
                assert!(state.search_editing);
                assert_eq!(state.rows, original);
                assert!(state.search_error.is_none());
                assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
                assert!(!state.apply_search_result(stale.clone()));
                assert!(state.search_pending);
                stale.request_id = state.search_request.to_string();
                stale.error.clear();
                assert!(state.apply_search_result(stale));
                assert_eq!(state.search_query, revised);
                assert_eq!(
                    state.applied_search_query.as_deref(),
                    Some(revised.as_str())
                );
                assert_eq!(state.selection_key().as_deref(), Some("run:match"));
                assert!(!state.search_editing);
                assert!(state.search_error.is_none());
            }
        }
    }

    #[test]
    fn strips_during_search_save_the_latest_rows_without_replacing_matches() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let reply = search_reply(&mut state, &["match-1", "match-2"]);
        assert!(state.apply_search_result(reply));
        state.selected = 1;
        let matches = state.rows.clone();
        let mut incoming = with_summary(strip(), 7, "Updated summary", false);
        incoming.rows.push(agent(10, "new agent"));
        incoming.focused = Some(7);
        assert!(!state.set_rows(incoming.clone()));
        assert_eq!(state.rows, matches);
        assert_eq!(state.selected, 1);
        assert_eq!(state.focused, Some(7));
        state.set_rows(Strip {
            focused: None,
            ..incoming.clone()
        });
        assert_eq!(state.focused, Some(7));
        assert!(state.clear_search());
        assert_eq!(state.rows, incoming.rows);
        assert_eq!(state.summaries, incoming.summaries);
        assert!(state.search_base.is_none());
    }

    #[test]
    fn cancelled_and_replaced_search_replies_are_ignored() {
        for cancellation in ["escape", "clear", "replace", "submit"] {
            for error in ["", "stale error"] {
                let mut state = State::default();
                state.set_rows(one_run_selected());
                let mut reply = search_reply(&mut state, &["stale"]);
                reply.error = error.to_owned();
                match cancellation {
                    "escape" => {
                        state.search_key(KeyWithModifier::new(BareKey::Esc));
                    },
                    "clear" => {
                        state.clear_search();
                    },
                    "replace" => {
                        state.begin_search();
                    },
                    _ => {
                        search_reply(&mut state, &["new"]);
                    },
                }
                let rows = state.rows.clone();
                let editing = state.search_editing;
                let pending = state.search_pending;
                assert!(!state.apply_search_result(reply));
                assert_eq!(state.rows, rows);
                assert_eq!(state.search_editing, editing);
                assert_eq!(state.search_pending, pending);
                assert!(state.search_error.is_none());
            }
        }
    }

    #[test]
    fn search_results_follow_selection_and_preserve_state_for_the_same_run() {
        for id in ["claude:session-1", "another-run"] {
            let mut state = reading("An open event");
            state.history_scroll = 3;
            state.history_cursor = 2;
            state.summarizing = state.selection_key().map(|row| SummaryAsk {
                row,
                asked_at_ms: 0,
                stale: false,
            });
            state.confirming_resume = state.selection_key();
            state.resume_note = Some((state.selection_key().unwrap(), "refused".to_owned()));
            state
                .search_revisions
                .insert("claude:session-1".to_owned(), 0);
            let reply = search_reply(&mut state, &[id]);
            assert!(state.apply_search_result(reply));
            let same = id == "claude:session-1";
            assert_eq!(state.history_scroll, if same { 3 } else { 0 });
            assert_eq!(state.history_cursor, if same { 2 } else { 0 });
            assert_eq!(state.reading.is_some(), same);
            assert_eq!(state.reader_cache.is_some(), same);
            assert_eq!(state.summarizing.is_some(), same);
            assert_eq!(state.confirming_resume.is_some(), same);
            assert_eq!(state.resume_note.is_some(), same);
        }
    }

    #[test]
    fn empty_results_and_errors_finish_search_and_ignore_duplicate_replies() {
        for error in ["", "search failed"] {
            let mut state = reading("An open event");
            let original = state.rows.clone();
            let mut reply = search_reply(&mut state, &[]);
            reply.error = error.to_owned();
            reply.truncated = true;
            assert!(state.apply_search_result(reply.clone()));
            // A failure keeps the box open on the query, for Enter to retry and Esc to leave.
            assert_eq!(state.search_editing, !error.is_empty());
            assert!(!state.search_pending);
            assert!(state.search_base.is_some());
            // A failure says nothing about how many matches there were: the rows below are the
            // last search's, and so is whether it left some out.
            assert_eq!(state.search_truncated, error.is_empty());
            assert_eq!(
                state.search_error.as_deref(),
                (!error.is_empty()).then_some(error)
            );
            if error.is_empty() {
                assert!(state.rows.is_empty());
                assert_eq!(state.selected, 0);
                assert!(state.reading.is_none());
            } else {
                assert_eq!(state.rows, original);
                assert!(state.reading.is_some());
            }
            assert!(!state.apply_search_result(reply));
            assert!(state.clear_search());
            assert_eq!(state.rows, original);
            assert!(!state.search_truncated);
            assert!(state.search_error.is_none());
        }
    }

    /// A refinement that fails leaves the search it was refining exactly as it was, including
    /// the notice that it had more matches than it showed: `Esc` goes back to that search, and
    /// the rows under it are still its rows.
    #[test]
    fn a_failed_refinement_keeps_the_applied_searchs_omission_notice() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let mut reply = search_reply(&mut state, &["match-1", "match-2"]);
        reply.truncated = true;
        assert!(state.apply_search_result(reply));
        let label = state.view_label().expect("a search view");
        assert!(label.contains("more matches omitted"), "{label}");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        let failed = wire::SearchResult {
            request_id: state.search_request.to_string(),
            error: "store busy".into(),
            ..Default::default()
        };
        assert!(state.apply_search_result(failed));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert_eq!(state.view_label(), Some(label));
    }

    fn agent(pane_id: u32, text: &str) -> StripRow {
        StripRow::Agent {
            pane_id,
            text: text.to_owned(),
            waiting: false,
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
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
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
        state.set_run(a_run());
        state.set_rows(unfocused(vec![
            StripRow::Project("hocus".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        assert!(!state.shows_an_age(), "nothing in `a_run` is stamped");
        let mut stamped = a_run();
        stamped
            .entries
            .first_mut()
            .expect("a timeline")
            .created_at_ms = Some(1);
        state.set_run(stamped);
        assert!(state.shows_an_age());

        // And the archive's own age column, which is drawn in the *list* half: it has to tick
        // with nothing stamped in the history half beside it, which is the usual case for an
        // archived run that was never summarized.
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        assert!(
            !state.shows_an_age(),
            "a row that cannot say when it started draws a blank gutter, not an age"
        );
        let StripRow::Run { started_at_ms, .. } = &mut state.rows[1] else {
            panic!("not a run row");
        };
        *started_at_ms = Some(1);
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
        // `j`/`k` are answered in `agents_key`, not here; see `arrow`.
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
            run_row("claude:session-1".to_owned(), "hook history"),
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
        state.set_run(run);

        let (_, history_rows) = split(19);
        state.render(21, 50);
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
        cramped.render(10, 50);
        let (_, cramped_rows) = split(8);
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
        state.set_run(run);
        // What the host would have answered for that instant; a native test build gets `Err`
        // from the shim and falls back to UTC, so it is set here the way `pipe` would.
        state.run_offset_secs = -4 * 3600;

        state.render(21, 60);
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
        state.render(21, 60);
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
        state.render(6, 40);
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
    fn shift_tab_from_the_list_visits_tabs_before_the_input_box() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        assert_eq!(state.focus, Focus::List);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(state.ring_yields.is_empty());
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert_eq!(state.ring_yields, ["input"]);
    }

    /// A terminal that cannot report Shift+Tab sends a plain Tab, which reads as forward. The
    /// ring wraps, so nothing is out of reach - backward is a shortcut, never the only way.
    #[test]
    fn an_unreported_shift_tab_reads_as_a_forward_step() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);

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
        state.render(21, 40);

        assert!(state.pipe(take(wire::STOP_HISTORY)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.pipe(take(wire::STOP_LIST)));
        assert_eq!(state.focus, Focus::List);
        assert!(
            !state.pipe(take(wire::STOP_LIST)),
            "already there, so nothing to redraw"
        );
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
        state.render(6, 40);
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
        state.render(21, 40);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.focus, Focus::History);

        // The last agent exits, and the strip arrives with nothing in it.
        state.set_rows(unfocused(vec![]));
        state.render(21, 40);

        assert_eq!(state.last_history_rows, 0);
        assert_eq!(state.focus, Focus::List, "and the focus came back with it");
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()),
            "so backward reaches the tab bar"
        );
        assert!(state.tabs_focused);
        assert!(state.ring_yields.is_empty());
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
        assert_eq!(asked(&state), Some("pane:3"));
        assert_eq!(state.selected, 2, "and the cursor stays where it was");

        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
            StripRow::MoreRuns,
        ]));
        state.selected = 1;
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Char('s'))),
            "an archived run is summarised from what it recorded"
        );
        assert_eq!(asked(&state), Some("run:claude:session-1"));

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
            run_row("claude:session-1".to_owned(), "hook history"),
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

    /// Leaving the tab drops the resume question armed there, as it drops a VMs one: left
    /// armed, it answered the first key of the next visit, and a `y` then started a process
    /// for a question asked about a list since forgotten.
    #[test]
    fn a_tab_switch_drops_an_armed_resume() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert!(state.confirming_resume.is_some());
        state.switch_surface(Surface::Todos);
        state.switch_surface(Surface::Agents);
        assert!(state.confirming_resume.is_none());
    }

    /// While the search box has the keys, a click only points. Taken as the Enter, a click on
    /// the row already chosen walked out of the panel, and two on an event opened the reader
    /// over the box - drawing the reader's keys while every key went on into the hidden query.
    #[test]
    fn a_click_while_the_search_box_is_open_only_points() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(21, 40);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        state.render(21, 40);
        let row = (1 + state.last_label_rows + state.selected - state.last_window_start) as isize;
        for _ in 0..2 {
            state.on_mouse(Mouse::LeftClick(row, 0));
            state.on_mouse(Mouse::Release(row, 0));
        }
        assert!(state.activations.is_empty());
        assert_eq!(state.panel_hides, 0);
        let event =
            (1 + state.last_label_rows + state.last_list_rows + 1 + state.last_history_body_top)
                as isize;
        for _ in 0..2 {
            state.on_mouse(Mouse::LeftClick(event, 0));
            state.on_mouse(Mouse::Release(event, 0));
        }
        assert_eq!(state.focus, Focus::History, "pointed at");
        assert!(state.reading.is_none());
        assert!(state.search_editing, "and the box still has the keys");
    }

    /// Tab out of a search box that never searched steps into the history of the agent the
    /// cursor was on: leaving the box used to put the cursor back at the top of the list, and
    /// the Tab stepped into the first agent's history instead.
    #[test]
    fn tab_out_of_an_unapplied_search_keeps_the_row() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.selected = 5;
        assert_eq!(state.selection(), Some(9));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        for ch in "ab".chars() {
            state.on_key(KeyWithModifier::new(BareKey::Char(ch)));
        }
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert!(!state.search_editing);
        assert_eq!(state.selection(), Some(9));
        assert_eq!(state.focus, Focus::History);
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
        rows.insert(1, run_row("claude:session-2".to_owned(), "newer"));
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
        assert_eq!(state.history_label(), "resume? y/n");

        state.on_key(KeyWithModifier::new(BareKey::Char('y')));
        assert_eq!(state.history_label(), "", "the question has been answered");

        let refuse = |state: &mut State, run_id: &str, detail: &str| {
            let result = wire::ResumeResult {
                run_id: run_id.to_owned(),
                detail: detail.to_owned(),
            };
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
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
        assert_eq!(
            state.panel_hides, 0,
            "a refusal leaves the keyboard here, and the panel up to say why"
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
        assert_eq!(state.history_label(), "resume? y/n");
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
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
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

    /// The hint on the divider is a hint: it says a request is out, and it goes when a summary
    /// made after the press arrives, or the cursor moves off the row it was about. A run that
    /// merely changes is not an answer - a working agent adds a message while the summary is
    /// still being written - and a hint that ended on one read as "done" when nothing had come.
    #[test]
    fn the_divider_says_a_summary_was_asked_for_until_something_answers() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 2;
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "");

        state.on_key(KeyWithModifier::new(BareKey::Char('s')));
        assert_eq!(state.history_label(), "summarising…");

        let send = |state: &mut State, run: &agents::AgentRun| {
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &Some(serde_json::to_string(run).expect("encoded")),
                &None,
                false,
            ));
            state.fill_history_cache(48);
        };
        let mut run = a_run();
        run.pane_id = 3;
        run.ended_at_ms = None;
        run.outcome = agents::RunOutcome::Active;
        send(&mut state, &run);
        assert_eq!(
            state.history_label(),
            "claude:session-1 · summarising…",
            "the run moved on, and said nothing about the summary"
        );

        // A summary made after the press is the answer.
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Summary,
            text: "Tests pass.".to_owned(),
            created_at_ms: Some(now_ms() + 1_000),
        });
        send(&mut state, &run);
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

    /// A pane with no hooks is summarised from its screen, and its answer comes in the strip
    /// rather than in a run: a summary there stamped after the press ends the hint too, and one
    /// from before it does not.
    #[test]
    fn a_summary_in_the_strip_answers_an_s_on_a_live_agent() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 2;
        state.on_key(KeyWithModifier::new(BareKey::Char('s')));
        let push = |state: &mut State, created_at_ms: Option<i64>| {
            let mut strip = with_summary(strip(), 3, "Ran the tests.", false);
            strip.summaries.get_mut(&3).expect("pushed").created_at_ms = created_at_ms;
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_STRIP,
                &Some(serde_json::to_string(&strip).expect("encoded")),
                &None,
                false,
            ))
        };
        push(&mut state, Some(1));
        assert_eq!(
            asked(&state),
            Some("pane:3"),
            "an old summary is not the answer"
        );
        assert!(push(&mut state, Some(now_ms() + 1_000)));
        assert_eq!(asked(&state), None);
    }

    /// Nothing tells this pane an `s` was refused, so the hint cannot wait for ever: once it
    /// has waited longer than the daemon ever takes it says nothing came, and `s` asks again.
    #[test]
    fn a_summary_nobody_answers_says_so_and_s_asks_again() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 2;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        let asked_at = state.summarizing.as_ref().expect("asked").asked_at_ms;
        assert!(!state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS - 1));
        state.fill_history_cache(48);
        assert_eq!(state.history_label(), "summarising…");

        assert!(state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS));
        assert!(
            !state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS + 1),
            "said once"
        );
        assert_eq!(state.history_label(), "no new summary · s to try again");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        assert_eq!(state.history_label(), "summarising…");
        assert!(!state.summarizing.as_ref().expect("asked again").stale);
    }

    /// Every note that takes the divider's place is held whole under it, as a refused resume
    /// is: a narrow divider cut "no new summary · s to try again" before the key that asks
    /// again.
    #[test]
    fn a_summary_nobody_wrote_is_wrapped_whole_under_the_divider() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 2;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        let asked_at = state.summarizing.as_ref().expect("asked").asked_at_ms;
        assert!(state.expire_summary_ask(asked_at + SUMMARY_WAIT_MS));
        for cols in 22..=40 {
            let lines = state.refusal_lines(cols);
            assert!(
                lines.iter().all(|line| line.width() <= cols - 2),
                "{lines:?} at {cols}"
            );
            assert_eq!(lines.join(" "), "no new summary · s to try again", "{cols}");
        }

        state.render(21, 22);
        let lines = state.refusal_lines(22);
        let header = state.header_rows(state.last_history_rows);
        let held = lines.len().min(state.last_history_rows - header - 1);
        assert!(held > 0);
        assert_eq!(state.last_history_body_top, header + held);
        assert_eq!(
            state.last_history_body_rows,
            state.last_history_rows - header - held
        );

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        assert!(
            state.refusal_lines(22).is_empty(),
            "asked again, and waiting"
        );
    }

    #[test]
    fn an_archived_run_draws_its_summary_and_timeline() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        state.set_run(a_run());

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
        assert!(history
            .entries
            .iter()
            .any(|row| row.text.contains("result: Implemented")));
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Enter)));
    }

    #[test]
    fn tab_clicks_use_the_drawn_cells_and_keep_each_surface() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.selected = 3;
        state.history_scroll = 2;
        state.render(12, 40);
        let agents_state = (state.selected, state.history_scroll);
        assert_eq!(
            state
                .tab_cells
                .iter()
                .map(|cell| (cell.text.as_str(), cell.target))
                .collect::<Vec<_>>(),
            [
                (" Agents │", Surface::Agents),
                (" Todos │", Surface::Todos),
                (" VMs │", Surface::Vms),
                (" Swamp │", Surface::Swamp),
            ]
        );

        let vm_x = state.tab_cells[2].x;
        assert!(state.on_mouse(Mouse::LeftClick(0, vm_x)));
        assert_eq!(state.surface, Surface::Vms);
        assert_eq!((state.selected, state.history_scroll), agents_state);

        let agents_x = state.tab_cells[0].x;
        assert!(state.on_mouse(Mouse::LeftClick(0, agents_x)));
        assert_eq!(state.surface, Surface::Agents);
        assert_eq!((state.selected, state.history_scroll), agents_state);
        assert!(!state.on_mouse(Mouse::LeftClick(0, 39)));
    }

    /// Whole segments or none: at every width the hint is the first few of `expected`'s keys,
    /// perhaps with the `? keys` marker after them, and only a first key too wide for the row
    /// on its own is ever cut. At full width it is `expected` exactly.
    ///
    /// And the marker is there wherever keys were left out and the ones that lead
    /// (`agents_hints`) still fit beside it - except in the search box, where `?` is typed
    /// rather than opening anything, and which never has one.
    fn check_footer(state: &mut State, rows: usize, expected: &str) {
        let segments: Vec<&str> = expected.split(" · ").collect();
        for cols in 1..=expected.width() + 1 {
            state.render(rows, cols);
            let hint = state.footer_hint().unwrap();
            assert!(hint.width() <= cols, "{hint:?} in {cols} columns");
            assert!(!hint.contains('\n'));
            assert_eq!(state.last_footer_row, Some(rows - 1));
            let keys = hint.strip_suffix(" · ? keys").unwrap_or(&hint);
            let whole = (1..=segments.len()).any(|n| keys == segments[..n].join(" · "));
            let cut = segments[0].width() > cols && segments[0].starts_with(keys);
            assert!(whole || cut, "{hint:?} in {cols} columns");
            if cols >= expected.width() {
                assert_eq!(hint, expected);
            }
            if state.typing() {
                assert!(!hint.ends_with("? keys"), "{hint:?} in {cols} columns");
                continue;
            }
            let keep = state.agents_hints().1.clamp(1, segments.len());
            let leads = segments[..keep].join(" · ").width() + " · ? keys".width();
            if keys != expected && leads <= cols {
                assert!(hint.ends_with(" · ? keys"), "{hint:?} in {cols} columns");
            }
        }
    }

    #[test]
    fn footer_hints_follow_the_view_and_fit_one_line() {
        let mut state = State::default();
        // Nothing to act on: no row keys, no history half to Tab into - nor a conversation to
        // Tab to, as far as anybody has said, until hocus sends its first strip.
        check_footer(&mut state, 12, "/ search · Esc hide");
        state.strip_seen = true;
        check_footer(&mut state, 12, "/ search · Tab chat · Esc hide");
        state.set_rows(unfocused(strip()));
        check_footer(
            &mut state,
            12,
            "Enter focus · s summary · p previous · w waiting (0) · / search · Tab history",
        );
        assert!(state.begin_predecessors());
        check_footer(
            &mut state,
            12,
            "Esc/c clear · Enter focus · s summary · p previous · w waiting (0) · / search · \
             Tab history",
        );
        assert!(state.begin_search());
        check_footer(&mut state, 12, "Enter apply · Esc cancel");
        state.search_query = "a query".into();
        check_footer(&mut state, 12, "Enter apply · Esc cancel · Ctrl-u clear");
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        check_footer(&mut state, 12, "searching… · Esc cancel");
        let result = search_reply(&mut state, &[]);
        assert!(state.apply_search_result(result));
        check_footer(&mut state, 12, "Esc/c clear · / edit");
        state.search_truncated = true;
        check_footer(&mut state, 12, "Esc/c clear · / edit");
        // A failure keeps the box open, and says why under it - so the footer is the keys.
        assert!(state.begin_search());
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        let mut failed = wire::SearchResult {
            request_id: state.search_request.to_string(),
            ..Default::default()
        };
        failed.error = "store busy".into();
        assert!(state.apply_search_result(failed));
        check_footer(&mut state, 12, "Enter retry · Esc back");
        assert!(state.clear_search());
        check_footer(
            &mut state,
            12,
            "Enter focus · s summary · p previous · w waiting (0) · / search · Tab history",
        );
    }

    /// The footer is read off the row under the cursor as well as the view: keys that work
    /// there, and none that do not - no `p` on a run, no `s` on `load older runs…`, no `Enter`
    /// on an empty list - with a question's answer keys first, and somebody waiting ahead of
    /// the row's own keys.
    #[test]
    fn footer_hints_name_the_keys_for_the_row_under_the_cursor() {
        let mut state = State::default();
        let mut rows = strip();
        rows.extend(archive());
        state.set_rows(unfocused(rows));
        state.render(21, 120);
        let footer = |state: &mut State| {
            state.render(21, 200);
            state.footer_hint().unwrap()
        };
        state.selected = 7;
        assert_eq!(
            footer(&mut state),
            "Enter history · r resume · s summary · w waiting (0) · / search · Tab history"
        );
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        assert_eq!(footer(&mut state), "y resume · n cancel");
        state.on_key(KeyWithModifier::new(BareKey::Char('n')));
        state.resume_note = Some(("run:claude:session-1".into(), "no session id".into()));
        assert!(footer(&mut state).starts_with("resume failed: no session id · Enter history"));
        state.resume_note = None;

        state.selected = 8;
        assert_eq!(
            footer(&mut state),
            "Enter load older runs · w waiting (0) · / search · Tab history"
        );

        // The history half on a run: its events, then the run's own keys.
        state.selected = 7;
        state.set_run(a_run());
        state.focus = Focus::History;
        assert_eq!(
            footer(&mut state),
            "↑↓ event · Enter read · r resume · s summary · w waiting (0) · / search · \
             PgUp/PgDn scroll · Tab chat"
        );
        // Unless another pane has the keyboard, when the half does not claim them.
        state.has_keyboard = Some(false);
        assert!(footer(&mut state).starts_with("Enter history"));
        state.has_keyboard = None;
        state.focus = Focus::List;

        // Somebody waiting goes ahead of the row's keys.
        let mut waiting = strip();
        waiting[3] = StripRow::Agent {
            pane_id: 7,
            text: "  7  codex  asking".into(),
            kind: "codex".into(),
            waiting: true,
        };
        state.set_rows(unfocused(waiting));
        state.selected = 2;
        assert_eq!(
            footer(&mut state),
            "w waiting (1) · Enter focus · s summary · p previous · / search · Tab history"
        );
        assert!(state.toggle_waiting());
        assert_eq!(
            footer(&mut state),
            "Esc/w all · 1 waiting · Enter focus · s summary · p previous · / search · \
             Tab history"
        );
        assert!(state.clear_waiting());

        // The reader's own keys, by what kind of reader it is.
        let mut state = reading(&task_notification());
        let reader = |state: &mut State| {
            state.render(21, 120);
            state.footer_hint().unwrap()
        };
        // The keys nothing on screen suggests come before the `Enter` the fold glyphs do.
        assert_eq!(
            reader(&mut state),
            "Esc back · j/k fold · y copy · v source · Enter open · PgUp/PgDn page"
        );
        for _ in 0..3 {
            press(&mut state, BareKey::Char('j'));
        }
        assert_eq!(
            reader(&mut state),
            "Esc back · j/k fold · y copy · o file · v source · Enter open · PgUp/PgDn page",
            "on the fold that names a file"
        );
        press(&mut state, BareKey::Char('v'));
        assert_eq!(
            reader(&mut state),
            "Esc back · ↑↓ scroll · y copy · o file · v folds · PgUp/PgDn page"
        );
    }

    /// At the widths a panel is drawn the keys fill the footer to within a few columns, which
    /// left `? keys` no room at all. The footer gives up a trailing key or two to say it - but
    /// never the row's verb, nor anything ahead of it - and the search box, where `?` is typed,
    /// never says it.
    #[test]
    fn the_footer_names_the_key_sheet_at_the_default_width() {
        let footer = |state: &mut State, cols: usize| {
            state.render(12, cols);
            state.render(12, cols);
            state.footer_hint().unwrap()
        };
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert_eq!(footer(&mut state, 24), "Enter focus · ? keys");
        assert_eq!(footer(&mut state, 34), "Enter focus · s summary · ? keys");

        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        assert_eq!(footer(&mut state, 24), "Enter history · ? keys");

        let mut state = reading(&task_notification());
        assert_eq!(footer(&mut state, 24), "Esc back · ? keys");

        // The history half's verb and the key before it take the row with no room left over:
        // the marker gives way to them.
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.focus = Focus::History;
        assert_eq!(footer(&mut state, 24), "↑↓ event · Enter read");

        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.begin_search();
        for query in ["", "a query"] {
            state.search_query = query.into();
            for cols in 1..=45 {
                let hint = footer(&mut state, cols);
                assert!(!hint.contains("? keys"), "{hint:?} at {cols}");
            }
        }
    }

    #[test]
    fn the_footer_leaves_room_for_the_header_and_content_on_resize() {
        let mut state = State::default();
        state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
        for cols in [0, 1, 40] {
            for (rows, list, history, footer) in [
                (12, 4, 5, Some(11)),
                (0, 0, 0, None),
                (1, 0, 0, None),
                (2, 1, 0, None),
                (3, 1, 0, Some(2)),
                (4, 2, 0, Some(3)),
                (8, 6, 0, Some(7)),
                (9, 3, 3, Some(8)),
                (12, 4, 5, Some(11)),
            ] {
                state.render(rows, cols);
                let expected = if cols == 0 {
                    (0, 0, None)
                } else {
                    (list, history, footer)
                };
                assert_eq!(
                    (
                        state.last_list_rows,
                        state.last_history_rows,
                        state.last_footer_row,
                    ),
                    expected,
                    "{rows} rows, {cols} columns"
                );
                assert_eq!(state.footer_hint().is_some(), expected.2.is_some());
            }
        }
    }

    #[test]
    fn the_footer_ignores_clicks_and_wheel_events() {
        let mut state = reading(&"word ".repeat(1000));
        state.close_reader();
        state.rows.extend((0..20).map(|id| agent(id, "claude")));
        for rows in [3, 12] {
            state.render(rows, 50);
            state.list_scroll = Some(3);
            state.history_scroll = 1;
            for focus in [Focus::List, Focus::History] {
                state.focus = focus;
                state.tabs_focused = true;
                let selected = state.selected;
                let footer = state.last_footer_row.unwrap() as isize;
                for event in [
                    Mouse::Hover(footer, 49),
                    Mouse::ScrollUp(1),
                    Mouse::ScrollDown(1),
                    Mouse::LeftClick(footer, 0),
                    Mouse::RightClick(footer, 0),
                    Mouse::ScrollUp(1),
                    Mouse::ScrollDown(1),
                ] {
                    assert!(!state.on_mouse(event));
                    assert_eq!(state.selected, selected);
                    assert_eq!(state.list_scroll, Some(3));
                    assert_eq!(state.history_scroll, 1);
                    assert_eq!(state.focus, focus);
                    assert!(state.tabs_focused);
                }
            }
        }
    }

    /// The reader has a footer of its own, with its own keys - not the list's, which would be
    /// wrong over it - and the other tabs still have none: their hints are on a row of their
    /// own already. The footer's row is not the reader's, so the wheel over it does nothing.
    #[test]
    fn readers_have_their_own_footer_and_other_tabs_none() {
        let mut state = reading(&"word ".repeat(1000));
        state.render(12, 50);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Esc back · ↑↓ scroll · y copy · PgUp/PgDn page")
        );
        assert_eq!(state.last_footer_row, Some(11));
        assert_eq!(state.last_reader_rows, 9);
        state.on_mouse(Mouse::Hover(11, 0));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)), "the footer's row");
        state.on_mouse(Mouse::Hover(10, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        state.close_reader();
        state.render(12, 50);
        assert_eq!(state.last_footer_row, Some(11));
        for surface in [Surface::Todos, Surface::Vms, Surface::Swamp] {
            state.surface = surface;
            state.render(12, 50);
            assert!(state.footer_hint().is_none());
            assert_eq!(state.last_footer_row, None);
        }
    }

    /// While the tab bar has the keys the Agents footer says how to work the bar, over the list
    /// and over the reader - neither of which would otherwise have the right keys on show - and
    /// goes back to the list's or the reader's own hints once the keys leave it. The other tabs
    /// say it in the hint row they have anyway, so they grow no footer: their bodies keep the
    /// same number of rows whoever has the keys - but for a quiet VMs list, which has no such
    /// row and grows one of its own (`render_vms`).
    #[test]
    fn the_bar_hint_is_the_footer_wherever_the_bar_has_the_keys() {
        let mut state = reading(&"word ".repeat(1000));
        for reader in [true, false] {
            if !reader {
                state.close_reader();
                state.set_rows(unfocused(strip()));
            }
            state.tabs_focused = true;
            state.render(12, 60);
            assert_eq!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
            assert_eq!(state.last_footer_row, Some(11));
            assert!(state.tab_cells.iter().any(|cell| cell.focused));

            // Another pane took the keyboard: the stop is kept, but nothing claims it.
            state.has_keyboard = Some(false);
            state.render(12, 60);
            assert!(state.tabs_focused);
            assert!(state.tab_cells.iter().all(|cell| !cell.focused));
            assert_ne!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
            state.has_keyboard = None;

            state.tabs_focused = false;
            state.render(12, 60);
            assert_eq!(state.last_footer_row, Some(11));
            assert_ne!(state.footer_hint().as_deref(), Some(tabs::BAR_HINT));
        }
        for surface in [Surface::Todos, Surface::Vms, Surface::Swamp] {
            state.surface = surface;
            for bar in [true, false, true] {
                state.tabs_focused = bar;
                state.render(12, 60);
                assert_eq!(state.last_footer_row, None, "{surface:?}, bar {bar}");
                assert_eq!(state.footer_hint(), None, "{surface:?}, bar {bar}");
            }
            assert!(state.tab_cells.iter().any(|cell| cell.focused));
        }
        state.surface = Surface::Agents;
        // A box being typed in takes the keys ahead of the bar, so its hint does too.
        state.begin_search();
        state.tabs_focused = true;
        state.render(12, 60);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Enter apply · Esc cancel")
        );
    }

    /// Ctrl-o moves the panel from wherever the keys are in it, including every state that
    /// answers all the other keys itself - and changes nothing inside it on the way.
    #[test]
    fn ctrl_o_is_taken_ahead_of_every_state_that_answers_every_key() {
        let ctrl_o = || KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier();
        let mut state = reading("An event being read");
        assert!(!state.on_key(ctrl_o()));
        assert!(state.reading.is_some(), "the reader stays open");

        let mut state = State::default();
        let mut rows = strip();
        rows.extend(archive());
        state.set_rows(unfocused(rows));
        state.begin_search();
        state.search_query = "typed".into();
        assert!(!state.on_key(ctrl_o()));
        assert!(state.search_editing);
        assert_eq!(state.search_query, "typed", "nothing typed into the box");
        state.clear_search();

        state.tabs_focused = true;
        assert!(!state.on_key(ctrl_o()));
        assert!(state.tabs_focused, "the bar keeps the keys");
        state.tabs_focused = false;

        state.selected = 7;
        assert!(state.arm_resume());
        assert!(!state.on_key(ctrl_o()));
        assert_eq!(
            state.confirming_resume.as_deref(),
            Some("run:claude:session-1"),
            "the question stays armed and on screen"
        );
    }

    #[test]
    fn native_bar_focus_uses_the_sending_client() {
        let mut state = State {
            native_tabs: true,
            surface: Surface::Vms,
            own_client_id: ClientId::new(1),
            bar_focused: vec![ClientId::new(1)],
            ..Default::default()
        };
        let shift_tab = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
        state.update(Event::Key(shift_tab.clone()));
        assert_eq!(state.ring_yields, [wire::STOP_INPUT]);
        assert!(state.bar_focused.is_empty());

        state.bar_focused.push(ClientId::new(1));
        state.own_client_id = ClientId::new(2);
        state.update(Event::Key(shift_tab));
        assert_eq!(state.ring_yields, [wire::STOP_INPUT]);
        assert_eq!(state.bar_focused, [ClientId::new(1), ClientId::new(2)]);

        state.pane_tabs = Some(PaneTabsId(7));
        state.update(Event::PaneTabsUpdate(vec![]));
        assert_eq!(state.pane_tabs, None);
        assert!(state.bar_focused.is_empty());
    }

    /// The bar answers its own keys and hands every other one back to the surface, which then
    /// does with it what it always does - rather than the key vanishing under a surface still
    /// advertising it.
    #[test]
    fn a_key_the_bar_does_not_own_goes_to_the_list_below_it() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.selected = 3;
        for release in [BareKey::Enter, BareKey::Down, BareKey::Char('j')] {
            state.tabs_focused = true;
            assert!(state.on_key(KeyWithModifier::new(release)));
            assert!(!state.tabs_focused);
            assert_eq!(state.selected, 3, "{release:?} only leaves the bar");
        }
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert!(!state.tabs_focused);
        assert_eq!(state.selected, 2, "k walked the list");

        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('s'))));
        assert_eq!(asked(&state), Some("pane:3"));
        assert!(!state.tabs_focused);

        // Modified keys too: Ctrl-r is the list's refresh, not the bar's to drop. And a redraw,
        // like every key that leaves the bar, whether or not the list has anything to draw for
        // it: the bar's highlight and its footer are coming down.
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier()));
        assert!(!state.tabs_focused);
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
        assert!(!state.tabs_focused);

        for surface in [Surface::Todos, Surface::Vms, Surface::Swamp] {
            state.surface = surface;
            state.tabs_focused = true;
            assert!(
                state.on_key(KeyWithModifier::new(BareKey::Char('%'))),
                "{surface:?}"
            );
            assert!(!state.tabs_focused, "{surface:?}");
        }
    }

    /// A click takes the keys off the bar wherever it lands, and that is a redraw even on a
    /// row nothing can be chosen on.
    #[test]
    fn a_click_off_the_bar_is_a_redraw_wherever_it_lands() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.focus = Focus::List;
        state.tabs_focused = true;
        assert!(state.on_mouse(Mouse::LeftClick(1, 0)), "the project header");
        assert!(!state.tabs_focused);
        assert_eq!(state.selected, 2);
    }

    /// A click that brings the keys back - off the tab bar, off another stop of the ring, or
    /// into this pane from another - only points, even on the row already chosen: the click
    /// after it, with the keys here, is the Enter. Walking the user out of the panel on the
    /// click that was only meant to get the keys back made the mouse a trap.
    #[test]
    fn a_click_bringing_the_keys_back_only_points() {
        for away in ["shift-tab", "take", "another pane"] {
            let mut state = State::default();
            state.set_rows(unfocused(strip()));
            state.render(21, 40);
            match away {
                "shift-tab" => {
                    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
                    assert!(state.tabs_focused);
                },
                "take" => {
                    state.take_ring_stop(Some(hocus_proto::vms::STOP_TABS));
                    assert!(state.tabs_focused);
                },
                _ => state.has_keyboard = Some(false),
            }
            // The selected row, pane 3, under the tab bar.
            let row = 1 + state.selected as isize;
            state.on_mouse(Mouse::LeftClick(row, 0));
            assert!(state.activations.is_empty(), "{away}");
            assert_eq!(state.panel_hides, 0, "{away}");
            assert!(state.keys_here() || away == "another pane", "{away}");
            // What the manifest the click makes the host send says.
            state.has_keyboard = Some(true);
            state.on_mouse(Mouse::LeftClick(row, 0));
            assert!(state.activations.is_empty(), "{away}: not on the press");
            state.on_mouse(Mouse::Release(row, 0));
            assert_eq!(state.activations, [3], "{away}");
        }

        // In the history half too: the click that brings the keys back to the event under the
        // cursor points at it, and the next one reads it.
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(21, 40);
        state.focus = Focus::History;
        state.has_keyboard = Some(false);
        let event = (state.last_list_rows + 2) as isize;
        state.on_mouse(Mouse::LeftClick(event, 0));
        assert!(state.reading.is_none());
        state.has_keyboard = Some(true);
        state.on_mouse(Mouse::LeftClick(event, 0));
        assert!(state.reading.is_none(), "not on the press");
        assert!(state.on_mouse(Mouse::Release(event, 0)));
        assert!(state.reading.is_some(), "the idle summary is an event too");
    }

    /// Esc on the bar is a step back to the list, the way Esc on the conversation's bar is a
    /// step back to its box - not the whole panel put away.
    #[test]
    fn esc_on_the_tab_bar_returns_to_the_list() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.focus = Focus::History;
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(!state.tabs_focused);
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.panel_hides, 0);

        state.surface = Surface::Vms;
        state.vms.details = true;
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(!state.tabs_focused);
        assert!(!state.vms.details);
        assert_eq!(state.panel_hides, 0);
    }

    /// A click on the tab already showing is a click on its list, too: the keys go to the list
    /// and `j` walks it.
    #[test]
    fn clicking_the_agents_tab_then_j_walks_the_list() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.focus = Focus::History;
        assert!(state.on_mouse(Mouse::LeftClick(0, state.tab_cells[0].x)));
        assert!(!state.tabs_focused);
        assert_eq!(state.focus, Focus::List);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert_eq!(state.selected, 3);
    }

    /// The tab bar is the panel's only map, so a filtered Agents view does not take it away:
    /// the four tabs stay drawn and clickable on every surface, and the view's name is the
    /// Agents surface's own first row, with the list a row below it.
    #[test]
    fn the_tab_bar_stays_up_in_every_view_and_the_view_names_itself_above_the_list() {
        for view in ["search", "typing", "previous", "waiting"] {
            let mut state = State::default();
            let mut rows = strip();
            for (index, pane_id) in [(2, 3), (3, 7)] {
                rows[index] = StripRow::Agent {
                    pane_id,
                    text: format!("  {pane_id}  claude  asking"),
                    kind: "claude".into(),
                    waiting: true,
                };
            }
            state.set_rows(unfocused(rows));
            let label = match view {
                "search" => {
                    let result = search_reply(&mut state, &["claude:a", "claude:b"]);
                    assert!(state.apply_search_result(result));
                    "Search: history"
                },
                "typing" => {
                    state.begin_search();
                    state.search_query = "half".into();
                    "Search: half"
                },
                "previous" => {
                    state.selected = 2;
                    assert!(state.begin_predecessors());
                    "Previous agents:   3  claude  asking"
                },
                _ => {
                    assert!(state.toggle_waiting());
                    "Waiting for you"
                },
            };
            state.render(12, 40);
            assert_eq!(state.view_label().as_deref(), Some(label), "{view}");
            assert_eq!(state.last_label_rows, 1);
            // Row 1 is the label, and the list starts on row 2.
            assert!(!state.on_mouse(Mouse::LeftClick(1, 0)), "{view}");
            let first = state.rows.iter().position(selectable).unwrap();
            let last = state.rows.iter().rposition(selectable).unwrap();
            assert_ne!(first, last, "{view}");
            state.selected = last;
            assert!(state.on_mouse(Mouse::LeftClick(2 + first as isize, 0)));
            assert_eq!(state.selected, first, "{view}");
            for surface in [
                Surface::Agents,
                Surface::Todos,
                Surface::Vms,
                Surface::Swamp,
            ] {
                state.surface = surface;
                state.render(12, 40);
                assert_eq!(state.tab_cells.len(), 4, "{view} on {surface:?}");
                assert_eq!(
                    state
                        .tab_cells
                        .iter()
                        .find(|cell| cell.active)
                        .map(|cell| cell.target),
                    Some(surface)
                );
                if surface != Surface::Agents {
                    assert_eq!(
                        state.last_label_rows, 0,
                        "the label is the Agents surface's"
                    );
                }
            }
            state.surface = Surface::Agents;
            state.render(12, 40);
            let todos_x = state.tab_cells[1].x;
            assert!(state.on_mouse(Mouse::LeftClick(0, todos_x)));
            assert_eq!(state.surface, Surface::Todos, "{view}");
            assert!(
                !state.search_editing,
                "a search being typed ends with the switch, rather than taking Todos' keys"
            );
        }
    }

    /// A paste lands in the box being typed in, as the typed characters would have, and
    /// nowhere else: on the list it would have been a string of single-letter commands.
    #[test]
    fn a_paste_extends_a_search_being_typed_and_nothing_else() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let before = state.selected;
        assert!(!state.update(Event::PastedText("sjp/w".into())));
        assert_eq!(state.selected, before);
        assert!(state.summarizing.is_none());
        assert!(state.waiting_base.is_none() && state.predecessors.is_none());

        state.begin_search();
        state.search_query = "build".into();
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        assert!(state.search_pending);
        let request = state.search_request;
        assert!(state.update(Event::PastedText(" failing\r\n  on macOS\n".into())));
        assert_eq!(state.search_query, "build failing on macOS");
        assert!(
            !state.search_pending,
            "the reply in flight was for another query"
        );
        assert_ne!(state.search_request, request);
        assert!(!state.update(Event::PastedText("\n\n".into())));
    }

    #[test]
    fn a_paste_is_one_line_with_no_control_characters() {
        assert_eq!(single_line("one\ntwo\r\nthree\rfour"), "one two three four");
        assert_eq!(single_line("  padded  \n\n\n  lines "), "  padded lines ");
        assert_eq!(
            single_line(" one line, as it was "),
            " one line, as it was "
        );
        assert_eq!(single_line("copied line\n"), "copied line");
        assert_eq!(single_line("tab\there\u{1b}[31m"), "tab here [31m");
        assert_eq!(single_line("\n \n"), "");
    }

    /// Putting the panel away puts the keyboard stop back on the list and drops every question
    /// left armed, whichever half did the hiding. What the user was looking at stays.
    #[test]
    fn a_hide_ends_the_visit_but_keeps_the_view() {
        let pane = |shown: bool, focused: bool| {
            Event::PaneUpdate(PaneManifest {
                panes: std::collections::HashMap::from([(
                    TabPosition::new(0),
                    vec![PaneInfo {
                        id: PaneId::Plugin(PluginId::default()),
                        is_suppressed: !shown,
                        is_focused: focused,
                        ..Default::default()
                    }],
                )]),
            })
        };
        let mut rows = strip();
        rows.extend(archive());
        for hide in ["esc", "manifest"] {
            let mut state = State::default();
            state.set_rows(unfocused(rows.clone()));
            state.render(21, 40);
            state.update(pane(true, true));
            state.selected = 7;
            assert!(state.arm_resume());
            state.focus = Focus::History;
            state.tabs_focused = true;
            match hide {
                "esc" => state.hide_the_panel(),
                _ => assert!(!state.update(pane(false, false)), "nothing shown to redraw"),
            }
            assert!(!state.tabs_focused, "{hide}");
            assert_eq!(state.focus, Focus::List, "{hide}");
            assert!(state.confirming_resume.is_none(), "{hide}");
            assert_eq!(state.selected, 7, "the selection is kept");
            state.update(pane(true, true));
            state.on_key(KeyWithModifier::new(BareKey::Char('y')));
            assert!(
                state.resume_requests.is_empty(),
                "{hide}: y answers nothing now"
            );
        }

        // A surface other than Agents is kept too, and so is a filtered view.
        let mut state = State::default();
        state.set_rows(unfocused(rows.clone()));
        state.toggle_waiting();
        state.surface = Surface::Todos;
        state.tabs_focused = true;
        state.update(pane(true, true));
        state.update(pane(false, false));
        assert_eq!(state.surface, Surface::Todos);
        assert!(state.waiting_base.is_some());
        assert!(!state.tabs_focused);
    }

    /// The keyboard leaving this pane only changes what is drawn - the stops stay where they
    /// are, for the keys to come back to - and the first manifest is no edge at all.
    #[test]
    fn losing_the_keyboard_redraws_but_keeps_the_stop() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert!(
            state.keys_here(),
            "told nothing, the pane assumes the keys are here"
        );
        assert!(
            !state.own_pane_changed(true, false),
            "the first manifest without the focus does not redraw"
        );
        assert!(!state.keys_here());
        assert!(state.own_pane_changed(true, true));
        assert!(state.keys_here());
        assert!(!state.own_pane_changed(true, true), "no edge, no redraw");
        state.focus = Focus::History;
        assert!(state.own_pane_changed(true, false));
        assert_eq!(
            state.focus,
            Focus::History,
            "the stop is kept for the way back"
        );
        assert!(!state.keys_here());
        assert!(
            !state.own_pane_changed(false, false),
            "a hidden pane has nothing on screen to correct"
        );
        assert_eq!(state.focus, Focus::List, "but hiding it ends the visit");
    }

    #[test]
    fn the_wheel_scrolls_the_list_without_selecting_and_clicks_use_its_window() {
        let mut state = State::default();
        state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
        state.render(12, 50);
        let selection = state.selection_key();
        state.history_scroll = 2;
        state.focus = Focus::History;
        assert!(!state.on_mouse(Mouse::Hover(1, 0)));
        assert!(state.on_mouse(Mouse::ScrollDown(3)));
        state.on_mouse(Mouse::Hover(state.last_list_rows as isize, 49));
        assert!(
            state.on_mouse(Mouse::ScrollDown(3)),
            "wheel events can precede a render"
        );
        state.render(12, 50);
        assert_eq!(state.last_window_start, 6);
        assert_eq!(state.selection_key(), selection);
        assert_eq!(state.focus, Focus::History);
        // render clamps the empty history; the wheel itself must leave it alone.
        state.history_scroll = 2;
        assert!(state.on_mouse(Mouse::ScrollDown(100)));
        assert_eq!(state.history_scroll, 2);
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        state.render(12, 50);
        assert_eq!(state.last_window_start, 15);
        assert!(state.on_mouse(Mouse::LeftClick(2, 0)));
        assert_eq!(state.selected, 16);
        // The window stays where it is, so the row clicked stays under the pointer for a second
        // click; a key hands it back to the selection.
        assert_eq!(state.list_scroll, Some(15));
        state.render(12, 50);
        assert_eq!(state.last_window_start, 15);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert_eq!(state.list_scroll, None);
    }

    /// In a list longer than its window, the first click of a double click used to re-centre
    /// the list on the row, so the second one landed on another row and walked into nothing.
    #[test]
    fn a_double_click_walks_into_the_agent_anywhere_in_a_long_list() {
        let mut state = State::default();
        state.set_rows(unfocused((0..20).map(|id| agent(id, "claude")).collect()));
        state.selected = 10;
        state.render(12, 50);
        let start = state.last_window_start;
        assert!(start > 0 && start < 10, "a window centred on the cursor");
        // The window's first row, under the tab bar: not the cursor's, and not the centre.
        let line = 1;
        state.on_mouse(Mouse::LeftClick(line, 0));
        state.on_mouse(Mouse::Release(line, 0));
        assert_eq!(state.selected, start, "the first click points");
        assert!(state.activations.is_empty());
        state.render(12, 50);
        assert_eq!(
            state.last_window_start, start,
            "the row stayed under the pointer"
        );
        state.on_mouse(Mouse::LeftClick(line, 0));
        assert!(state.activations.is_empty(), "not on the press");
        state.on_mouse(Mouse::Release(line, 0));
        assert_eq!(state.activations, [start as u32]);
    }

    #[test]
    fn manual_list_scroll_survives_refreshes_and_clamps_to_the_available_rows() {
        let rows: Vec<_> = (0..20).map(|id| agent(id, "claude")).collect();
        let mut state = State::default();
        state.set_rows(unfocused(rows.clone()));
        state.render(12, 50);
        // Without a pointer position, the wheel uses keyboard focus.
        assert!(state.on_mouse(Mouse::ScrollDown(100)));
        assert!(!state.set_rows(unfocused(rows.clone())));
        state.render(16, 50);
        assert_eq!(state.last_window_start, 14);
        state.set_rows(unfocused(rows[..10].to_vec()));
        state.render(16, 50);
        assert_eq!(state.last_window_start, 4);
        assert_eq!(state.selected, 0);
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Up)),
            "reveal even at the first agent"
        );
        state.render(16, 50);
        assert_eq!(state.last_window_start, 0);
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        state.set_rows(focused_on(rows.clone(), 9));
        state.render(16, 50);
        assert_eq!(state.list_scroll, None);
        assert_eq!(state.selected, 9);
        assert_eq!(state.last_window_start, 6);

        state.set_rows(unfocused(Vec::new()));
        state.render(16, 50);
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        state.set_rows(unfocused(rows));
        state.render(2, 50);
        assert_eq!(state.last_history_rows, 0);
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        state.render(1, 50);
        assert_eq!(state.last_list_rows, 0);
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        state.render(0, 50);
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        state.render(12, 50);
        state.on_mouse(Mouse::Hover(-1, 0));
        assert!(
            !state.on_mouse(Mouse::ScrollDown(1)),
            "pointer left the pane"
        );
        state.on_mouse(Mouse::Hover(12, 0));
        assert!(
            !state.on_mouse(Mouse::ScrollDown(1)),
            "outside the rendered pane"
        );
    }

    #[test]
    fn header_and_empty_panes_preserve_list_history_and_reader_positions() {
        for reader in [false, true] {
            let mut state = reading(&"word ".repeat(1000));
            state.rows.extend((0..20).map(|id| agent(id, "claude")));
            state.list_scroll = Some(3);
            state.history_scroll = 1;
            if reader {
                assert!(state.scroll_reader(3));
            } else {
                state.close_reader();
            }
            state.render(12, 50);
            let position = |state: &State| {
                (
                    state.selected,
                    state.list_scroll,
                    state.history_scroll,
                    state.history_cursor,
                    state.reading.as_ref().map(|r| (r.scroll, r.cursor)),
                    state.focus,
                    state.tabs_focused,
                )
            };
            let before = position(&state);
            let dimensions = |state: &State| {
                [
                    state.last_window_start,
                    state.last_list_rows,
                    state.last_history_rows,
                    state.last_history_body_rows,
                    state.last_history_max_scroll,
                    state.last_reader_rows,
                    state.last_reader_max_scroll,
                ]
            };
            let visible = dimensions(&state);
            for (line, column) in [(0, 0), (-1, 0), (12, 0), (1, 50)] {
                state.on_mouse(Mouse::Hover(line, column));
                assert!(!state.on_mouse(Mouse::ScrollUp(1)));
                assert!(!state.on_mouse(Mouse::ScrollDown(1)));
                if line != 0 {
                    assert!(!state.on_mouse(Mouse::LeftClick(line, column)));
                }
                assert_eq!(position(&state), before);
            }
            for (rows, cols) in [(1, 50), (0, 50), (12, 0)] {
                state.render(rows, cols);
                assert_eq!((state.last_rows, state.last_cols), (rows, cols));
                assert_eq!(dimensions(&state), [0; 7]);
                state.mouse_position = None;
                assert!(!state.on_mouse(Mouse::ScrollUp(1)));
                assert!(!state.on_mouse(Mouse::ScrollDown(1)));
                state.on_mouse(Mouse::Hover(1, 0));
                assert!(!state.on_mouse(Mouse::ScrollUp(1)));
                assert!(!state.on_mouse(Mouse::ScrollDown(1)));
                assert!(!state.on_mouse(Mouse::LeftClick(1, 0)));
                assert_eq!(position(&state), before);

                let header_visible = rows > 0 && cols > 0;
                let vm_x = state
                    .tab_cells
                    .iter()
                    .find(|cell| cell.target == Surface::Vms)
                    .map(|cell| cell.x)
                    .unwrap_or(10);
                assert_eq!(state.on_mouse(Mouse::LeftClick(0, vm_x)), header_visible);
                assert_eq!(state.surface == Surface::Vms, header_visible);
                assert_eq!(state.on_mouse(Mouse::LeftClick(0, 0)), header_visible);
                assert_eq!(state.surface, Surface::Agents);
                // A tab click lands on the tab's list; put the keys back where they were to
                // go on.
                if header_visible {
                    assert_eq!(state.focus, Focus::List);
                    state.focus = before.5;
                }

                state.render(12, 50);
                assert_eq!(position(&state), before);
                // Only the active surface's geometry is restored.
                let restored = dimensions(&state);
                if reader {
                    assert_eq!(restored[5..], visible[5..]);
                } else {
                    assert_eq!(restored[..5], visible[..5]);
                }
                state.on_mouse(Mouse::Hover(1, 0));
                assert!(state.on_mouse(Mouse::ScrollDown(1)));
                assert!(state.on_mouse(Mouse::ScrollUp(1)));
                assert_eq!(position(&state), before);
            }
        }
    }

    #[test]
    fn the_wheel_scrolls_folded_and_raw_readers_without_touching_the_list() {
        for raw in [false, true] {
            let mut state = reading(&long_notification());
            if raw {
                press(&mut state, BareKey::Char('v'));
            } else {
                for _ in 0..5 {
                    press(&mut state, BareKey::Char('j'));
                }
                press(&mut state, BareKey::Enter);
            }
            state.on_mouse(Mouse::ScrollUp(1000));
            state.render(20, 50);
            let selected = state.selected;
            let history_scroll = state.history_scroll;
            state.on_mouse(Mouse::Hover(1, 0));
            assert!(state.on_mouse(Mouse::ScrollDown(1)));
            assert_eq!(state.reading.as_ref().unwrap().scroll, 1);
            assert!(state.on_mouse(Mouse::ScrollDown(1000)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            assert!(state.on_mouse(Mouse::ScrollUp(1)));
            assert_eq!(
                state.reading.as_ref().unwrap().scroll,
                state.last_reader_max_scroll - 1
            );
            assert_eq!(state.selected, selected);
            assert_eq!(state.history_scroll, history_scroll);
            assert_eq!(state.list_scroll, None);
        }
    }

    /// The wheel over history stops at both ends. Unclamped, a wheel at the top would count
    /// coming back down would cost as many notches as were wasted going up.
    #[test]
    fn the_wheel_scrolls_the_history_and_stops_at_both_ends() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        let mut run = a_run();
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Ship it?".to_owned(),
            created_at_ms: None,
        });
        state.set_run(run);
        // Four rows below the divider, against five timeline rows including the result.
        state.render(10, 50);
        assert_eq!(state.last_history_rows, 4);
        assert_eq!(state.last_history_max_scroll, 1);

        assert!(!state.on_mouse(Mouse::Hover((state.last_list_rows + 1) as isize, 0)));
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
        assert_eq!(
            state.focus,
            Focus::List,
            "scrolling history does not take the keys"
        );
        assert_eq!(state.list_scroll, None);
        // The last rendered row and the first row below the divider belong to history too.
        state.on_mouse(Mouse::Hover(8, 0));
        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        state.on_mouse(Mouse::Hover((state.last_list_rows + 2) as isize, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        state.mouse_position = None;
        state.focus = Focus::History;
        assert!(
            state.on_mouse(Mouse::ScrollUp(1)),
            "focus is the fallback without coordinates"
        );
    }

    /// The history half keeps its parse of the run across resizes and across the run growing
    /// (`LoadedRun::parsed`), and neither may change what it draws: after both, the half is exactly
    /// what the run drawn from scratch at the new width is.
    #[test]
    fn a_resized_half_over_a_grown_run_draws_what_a_fresh_one_does() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        let mut run = a_run();
        // Unstamped, so the ages cannot tick over between the two draws being compared.
        for entry in &mut run.entries {
            entry.created_at_ms = None;
        }
        state.set_run(run.clone());
        state.render(14, 60);
        state.render(14, 41);

        run.entries.push(agents::RunEntry {
            revision: 9,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Shall I **push** it?".to_owned(),
            created_at_ms: None,
        });
        state.set_run(run.clone());
        state.render(14, 41);

        let fresh = agents::run_history(&run, 41, now_ms(), state.run_offset_secs);
        assert_eq!(state.cached_history_lines(41), &fresh);
        assert!(fresh.entries.iter().any(|line| line.text.contains("push")));
    }

    /// The run's parse is touched only when a run is loaded - a resize step draws what is
    /// held, and so does a strip push, which moves `generation` on any pane's change and says
    /// nothing about this run. Read by poisoning the held parse with a shorter copy of the run,
    /// as `the_reader_is_wrapped_once_per_event_fold_and_width` poisons its cache.
    #[test]
    fn the_runs_parse_is_only_brought_up_to_date_by_a_loaded_run() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        for entry in &mut run.entries {
            entry.created_at_ms = None;
        }
        state.set_run(run.clone());
        state.render(14, 60);

        let mut poison = run.clone();
        poison.entries.truncate(1);
        state.run.as_mut().expect("a run").parsed = agents::ParsedRun::new(&poison);
        let fresh = |run: &agents::AgentRun, offset| agents::run_history(run, 41, now_ms(), offset);
        let offset = state.run_offset_secs;
        assert_eq!(
            state.cached_history_lines(41),
            &fresh(&poison, offset),
            "a resize re-wraps what is held"
        );

        state.generation += 1;
        assert_eq!(
            state.cached_history_lines(41),
            &fresh(&poison, offset),
            "a strip push leaves the run's timeline alone"
        );

        state.set_run(run.clone());
        assert_eq!(
            state.cached_history_lines(41),
            &fresh(&run, offset),
            "a loaded run brings it up to date"
        );
    }

    /// A cursor that leaves the run's row and comes back finds the parse where it left it: the
    /// run has not changed, and parsing a long one again is seconds. Poisoned as above, so a
    /// parse made again would show.
    #[test]
    fn stepping_off_the_run_and_back_keeps_its_parse() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
            run_row("claude:session-2".to_owned(), "another run"),
        ]));
        state.selected = 1;
        let mut run = a_run();
        for entry in &mut run.entries {
            entry.created_at_ms = None;
        }
        state.set_run(run.clone());
        state.render(14, 60);

        let mut poison = run;
        poison.entries.truncate(1);
        state.run.as_mut().expect("a run").parsed = agents::ParsedRun::new(&poison);
        let offset = state.run_offset_secs;

        state.selected = 2;
        state.render(14, 41);
        assert!(state.run.is_some(), "the run is still held");
        state.selected = 1;
        assert_eq!(
            state.cached_history_lines(41),
            &agents::run_history(&poison, 41, now_ms(), offset),
            "back on the run, the held parse is drawn"
        );
    }

    /// Only a run that says something new moves `run_revision`, and a run arriving does not
    /// move `generation`: hocus sends the selected run again each time it is asked, and the
    /// strip's counter is not the run's.
    #[test]
    fn only_a_changed_run_moves_the_run_revision() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        let generation = state.generation;
        let send = |state: &mut State, run: &agents::AgentRun| {
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &Some(serde_json::to_string(run).expect("encoded")),
                &None,
                false,
            ))
        };
        let mut run = a_run();
        assert!(send(&mut state, &run));
        assert_eq!(state.run_revision, 1);
        assert!(!send(&mut state, &run), "the same run says nothing");
        assert_eq!(state.run_revision, 1);

        run.entries[0].text.push_str(" - and more");
        assert!(send(&mut state, &run));
        assert_eq!(state.run_revision, 2);
        assert_eq!(state.generation, generation);

        // The fingerprint goes with the run, so a cleared one cannot make the same run
        // arriving again look like one already held.
        state.run = None;
        assert!(send(&mut state, &run), "a run cleared is taken again");
        assert_eq!(state.run_revision, 3);
    }

    /// The summary is a row of the timeline like any other, so a talkative one costs the
    /// events beside it nothing they cannot be scrolled back to: the whole half winds over it.
    #[test]
    fn a_long_summary_scrolls_off_the_top_like_any_other_event() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        let mut run = a_run();
        run.summary = Some(agents::Summary {
            // Four rows of it at this width - more than the half has to give, which under the
            // old pinned block would have left one row of timeline showing whatever the scroll.
            text: "word ".repeat(40),
            ..run.summary.take().unwrap()
        });
        state.set_run(run);

        // Three rows below the divider, and every one of them a line of timeline.
        state.render(9, 50);
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
        state.on_mouse(Mouse::Hover((state.last_list_rows + 1) as isize, 0));
        assert!(state.on_mouse(Mouse::ScrollUp(50)));
        state.render(9, 50);
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

    /// `Tab` and `Shift+Tab` walk the ring, and `j`/`k` and the arrows do whatever the half they
    /// are in does with a step - a row of the list, an event of the timeline.
    ///
    /// Directional now: forward off the history half leaves this pane for the conversation, so
    /// getting back to the list is `Shift+Tab` rather than another `Tab`.
    #[test]
    fn tab_walks_the_ring_and_jk_follow_the_focus() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
            run_row("claude:session-2".to_owned(), "another"),
        ]));
        let mut run = a_run();
        run.entries.push(agents::RunEntry {
            revision: 5,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Ship it?".to_owned(),
            created_at_ms: None,
        });
        state.set_run(run);
        // Three rows of timeline, with older events available above them.
        state.render(9, 50);
        assert_eq!(state.selected, 1);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        assert_eq!(
            state.history_cursor, 1,
            "k walks the timeline it is in, an event at a time"
        );
        assert_eq!(state.selected, 1, "and moves no selection");

        // The arrows follow the focus too: in the history half they walk its events, and leave
        // the list and the keys where they are.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert_eq!(
            state.history_cursor, 0,
            "Down is j, a step toward the newest"
        );
        assert!(state.on_key(KeyWithModifier::new(BareKey::Up)));
        assert_eq!(state.history_cursor, 1, "Up is k");
        assert_eq!((state.selected, state.focus), (1, Focus::History));

        // Shift+Tab is the way back to the list, where they walk the rows.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
        assert_eq!(state.focus, Focus::List);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Down)));
        assert_eq!(state.selected, 2);
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

        // An arrow with nowhere to go is nothing to redraw, in either half.
        state.on_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(state.selected, 2, "the last row there is");
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Down)));
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.focus, Focus::History);
        state.history_cursor = 0;
        assert!(
            !state.on_key(KeyWithModifier::new(BareKey::Down)),
            "already on the newest event"
        );
        assert_eq!((state.selected, state.focus), (2, Focus::History));
    }

    /// A strip of one archived run, the shape every history test below wants.
    fn one_run_selected() -> Strip {
        unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
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
        state.set_run(run);
        state.focus = Focus::History;
        state.render(21, 50);
        // Thirty one-line events against the six rows the pinned header leaves of the nine:
        // the half is at the newest end.
        assert_eq!(state.last_history_rows, 9);
        assert_eq!(state.last_history_body_rows, 6);
        assert_eq!(state.history_scroll, 0);

        // Walk the cursor past the top of the window and the half follows it back.
        for _ in 0..12 {
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
        }
        state.render(21, 50);
        assert_eq!(state.history_cursor, 12);
        assert_eq!(
            state.history_scroll, 7,
            "wound back just far enough to put the event on the oldest row"
        );

        // And back down again: the half winds forward only as far as it has to.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        state.render(21, 50);
        assert_eq!(state.history_cursor, 11);
        assert_eq!(state.history_scroll, 7, "still on screen, so nothing moved");

        // The cursor stops at the oldest event rather than running off the end.
        for _ in 0..50 {
            state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        }
        state.render(21, 50);
        assert_eq!(state.history_cursor, 29);
        assert_eq!(state.history_scroll, state.last_history_max_scroll);
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('k'))));
    }

    /// The oldest event is where anybody looking for something older ends up, and the notice
    /// that older events were not loaded is the row above it - a row no event owns, which
    /// winding the window to the event's own first row left just off the top.
    #[test]
    fn walking_to_the_oldest_event_shows_the_omitted_notice() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        run.omitted_entries = 3;
        run.entries = (0..30)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
                created_at_ms: None,
            })
            .collect();
        state.set_run(run);
        state.focus = Focus::History;
        state.render(21, 50);
        for _ in 0..50 {
            state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        }
        state.render(21, 50);
        assert_eq!(state.history_cursor, 29);
        assert_eq!(state.history_scroll, state.last_history_max_scroll);
        assert_eq!(state.last_history_window_start, 0);
        let notice = &state.history().entries[0].text;
        assert!(notice.contains("older events not loaded"), "{notice}");
    }

    /// `Enter` in the history half opens the event under the cursor, uncapped and with the
    /// whole pane to read it in; `Esc` puts the reader away and leaves the pane where it is.
    /// On a run, the list half's `Enter` is the way into that half: the run has no pane to walk
    /// to, and the two halves have separate cursors.
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
        state.set_run(run);
        state.render(21, 50);

        // Capped in the half, whole in the reader.
        assert_eq!(
            state.cached_history_lines(50).entries.len(),
            agents::MAX_ENTRY_ROWS
        );
        // In the list half a run has no pane to walk into, so Enter walks into its history -
        // and opens nothing yet, and leaves the panel up.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.reading.is_none());
        assert_eq!(state.panel_hides, 0);

        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        let reading = state.reading.as_ref().expect("the event under the cursor");
        assert_eq!(reading.kind, "prompt");
        assert_eq!(reading.created_at_ms, Some(1));
        assert_eq!(&*reading.text, "word ".repeat(400));

        state.render(21, 50);
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
        state.set_run(run);
        state.render(21, 50);
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
        state.render(21, 50);
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
    /// every keystroke - so it is wrapped once per event, fold and width, the way the history
    /// half below it is. Read by poisoning the cache, as `the_history_is_rendered_once_per_thing
    /// _it_depends_on` does.
    #[test]
    fn the_reader_is_wrapped_once_per_event_fold_and_width() {
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
        state.set_run(run);
        state.render(21, 50);
        state.focus = Focus::History;
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.render(21, 50);

        let poison = envelope::ReaderLines {
            lines: vec![markdown::MdLine::plain("poison")],
            starts: Vec::new(),
        };
        let key = state.reader_cache.as_ref().unwrap().0;
        state.reader_cache = Some((key, poison.clone()));
        state.fill_reader_cache(50);
        assert_eq!(state.reader_lines(), poison.lines, "same event, same width");
        state.fill_reader_cache(40);
        assert_ne!(state.reader_lines(), poison.lines, "a resize rewraps it");

        // And a second opening is a second event, however alike the two look.
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.fill_reader_cache(40);
        assert_ne!(state.reader_lines(), poison.lines);
    }

    /// The page keys are the history's from either half: the list has no page of its own,
    /// following the selection during keyboard navigation.
    #[test]
    fn the_page_keys_are_the_historys_from_either_half() {
        let mut state = State::default();
        state.set_rows(unfocused(vec![
            StripRow::Project("archive".to_owned()),
            run_row("claude:session-1".to_owned(), "hook history"),
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
        state.set_run(run);
        state.render(21, 50);

        assert_eq!(state.focus, Focus::List, "the page keys did not need it");
        // A frame between the presses, so a window wound back to the cursor on render would
        // show: in the history half the cursor has to come to the window instead.
        for focus in [Focus::List, Focus::History] {
            state.focus = focus;
            assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
            state.render(21, 50);
            assert_eq!(state.history_scroll, state.page(), "{focus:?}");
            assert!(state.on_key(KeyWithModifier::new(BareKey::PageDown)));
            state.render(21, 50);
            assert_eq!(state.history_scroll, 0, "{focus:?}");
            assert!(
                !state.on_key(KeyWithModifier::new(BareKey::PageDown)),
                "at the newest line already"
            );
        }
    }

    /// A click lands in the half it points at. In the list it moves the cursor and no more;
    /// below the divider it points at the event under it, and on the divider the focus is all
    /// it does.
    #[test]
    fn a_click_lands_in_the_half_it_points_at() {
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(21, 40);
        let (list_rows, history_rows) = split(19);
        assert_eq!(state.last_list_rows, list_rows);
        assert_eq!(state.last_history_rows, history_rows);

        assert!(
            state.on_mouse(Mouse::LeftClick((list_rows + 1) as isize, 0)),
            "the divider takes the focus"
        );
        assert_eq!(state.focus, Focus::History);
        assert_eq!(state.selected, 2, "and selects nothing");
        assert!(
            !state.on_mouse(Mouse::LeftClick((list_rows + 3) as isize, 0)),
            "already down here, and on no event"
        );
        // A click on the event the cursor is on, with the keys already down here, reads it -
        // once it comes up on the cell it went down on.
        let event = (list_rows + 2) as isize;
        state.on_mouse(Mouse::LeftClick(event, 0));
        assert!(state.reading.is_none(), "not on the press");
        assert!(state.on_mouse(Mouse::Release(event, 0)));
        assert!(state.reading.is_some(), "the idle summary is an event too");
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        // A press on it that ends as a drag - to copy its text - reads nothing.
        state.on_mouse(Mouse::LeftClick(event, 0));
        state.on_mouse(Mouse::Hold(event, 6));
        assert!(!state.on_mouse(Mouse::Release(event, 6)));
        assert!(state.reading.is_none(), "a drag is not a click");

        assert!(
            state.on_mouse(Mouse::LeftClick(4, 0)),
            "the list takes it back"
        );
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.selected, 3, "pane 7, the row that was pointed at");
        assert!(
            state.activations.is_empty(),
            "and pointed at it is all: a click does not walk into the agent"
        );

        // and a click on a header inside the list still hits no row, but the half is the
        // half the pointer is in
        state.focus = Focus::History;
        assert!(state.on_mouse(Mouse::LeftClick(1, 0)), "the Project header");
        assert_eq!(state.focus, Focus::List);
        assert_eq!(state.selected, 3, "still pane 7");
    }

    /// A click is an arrow key with a destination: it moves the cursor, the history half
    /// follows it there, and `Enter` is what actually walks into the agent. Clicking used to
    /// do both, which meant looking at an agent's events cost you the pane you were in.
    #[test]
    fn a_click_selects_and_enter_is_what_focuses() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        assert_eq!(state.selected, 2, "the first agent, pane 3");

        // wound into the history of the row the cursor starts on, so the reset that
        // belongs to leaving a row has something to undo
        state.history_scroll = 4;
        let was = state.selection_key();

        assert!(state.on_click(3, true), "the row under the pointer");
        assert_eq!(state.selected, 3, "pane 7");
        assert!(
            state.activations.is_empty(),
            "a click moved the cursor and nothing else"
        );
        assert_eq!(state.panel_hides, 0, "and left the panel up");
        // what `update` does after every event, and the whole of how the history half
        // learns which agent it is now about
        assert!(
            state.follow_selection(was),
            "the cursor left the row it was on"
        );
        assert_eq!(state.history_scroll, 0, "the half is about pane 7 now");

        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(state.activations, [7], "and Enter walks into it");
        assert_eq!(
            state.panel_hides, 1,
            "putting the panel away on the way, so the agent is not walked into behind it"
        );

        // A second click on the row the cursor is on, with the keys already in the list, is the
        // Enter - but not the click that brings the keys back from the history half onto it.
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 40);
        state.focus = Focus::History;
        assert!(state.on_click(2, true), "back to the list");
        assert!(state.activations.is_empty());
        assert!(
            !state.on_click(2, true),
            "the panel went, so nothing here to redraw"
        );
        assert_eq!(state.activations, [3]);
        assert_eq!(state.panel_hides, 1);

        // `load older runs…` has nothing to preview, so its first click is its Enter.
        let mut rows = strip();
        rows.push(StripRow::MoreRuns);
        let mut state = State::default();
        state.set_rows(unfocused(rows));
        state.render(21, 40);
        assert!(state.on_click(6, true));
        assert_eq!(state.archive_requests, 1);
        assert_eq!(state.panel_hides, 0);
    }

    /// Only the Enter that actually walks into an agent dismisses the panel. Every other Enter
    /// is about something inside the panel, and putting the panel away would take the answer
    /// off screen with it.
    #[test]
    fn only_an_enter_on_an_agent_puts_the_panel_away() {
        // `reading` is the history half's Enter: it opens the event under that half's own
        // cursor, which is a thing drawn in this pane.
        let mut state = reading("implement history");
        assert!(state.reading.is_some(), "the reader is up");
        assert_eq!(
            state.panel_hides, 0,
            "the reader is in this pane - hiding it would be closing what was just opened"
        );
        assert!(state.activations.is_empty(), "and nothing was walked into");

        // Esc backs out of the reader rather than falling through to the dismissal
        press(&mut state, BareKey::Esc);
        assert!(state.reading.is_none(), "back to the list");
        assert_eq!(state.panel_hides, 0, "one step back, not the way out");

        // a project header: Enter is inert on it, and an inert key hides nothing
        let mut state = State::default();
        state.set_rows(with_summary(strip(), 3, "Ran the tests.", false));
        state.render(21, 40);
        state.focus = Focus::List;
        state.selected = 0;
        assert!(
            matches!(state.rows.first(), Some(StripRow::Project { .. })),
            "the strip starts on a header"
        );
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(
            state.panel_hides, 0,
            "Enter on a header does nothing at all"
        );

        // `load older runs…` asks hocus for more of the archive, in this pane
        let mut rows = strip();
        rows.push(StripRow::MoreRuns);
        state.set_rows(unfocused(rows));
        state.selected = state.rows.len() - 1;
        assert!(matches!(
            state.rows.get(state.selected),
            Some(StripRow::MoreRuns)
        ));
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(
            state.panel_hides, 0,
            "the rows it asks for arrive here, so the list has to still be on screen"
        );
        assert!(state.activations.is_empty(), "and it is not an agent");

        // an archived run walks into its history, in this pane
        let mut rows = strip();
        rows.extend(archive());
        state.set_rows(unfocused(rows));
        state.render(21, 40);
        state.selected = 7;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        assert_eq!(state.focus, Focus::History);
        assert_eq!(state.panel_hides, 0, "what Enter found is in the panel");
        assert!(state.activations.is_empty());
        state.focus = Focus::List;

        // and the one that is
        state.selected = 2;
        assert!(matches!(
            state.rows.get(state.selected),
            Some(StripRow::Agent { .. })
        ));
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(state.activations, [3], "the first agent");
        assert_eq!(state.panel_hides, 1, "and this Enter is the one that hides");
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
                    waiting: true,
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
            run_row("claude:session-1".to_owned(), "hook history"),
        ]));
        state.set_run(a_run());
        state.render(21, 50);

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
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
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

    #[test]
    fn live_and_archived_continuations_render_in_a_narrow_panel() {
        for archived in [false, true] {
            let mut run = a_run();
            run.origin_run_id = Some("claude:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into());
            run.previous_run_id = Some("claude:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into());
            let row = if archived {
                run_row(run.id.clone(), "history")
            } else {
                run.ended_at_ms = None;
                run.outcome = agents::RunOutcome::Active;
                StripRow::Agent {
                    pane_id: 3,
                    text: "↳ claude · history".into(),
                    kind: "claude".into(),
                    waiting: false,
                }
            };
            let mut state = State::default();
            state.set_rows(unfocused(vec![row]));
            state.set_run(run);
            state.render(60, 32);
            let header = &state.history().header;
            assert!(header.iter().all(|row| row.text.chars().count() <= 32));
            for label in ["started", "session", "origin", "previous"] {
                assert!(
                    header.iter().any(|row| row.text.starts_with(label)),
                    "{label} / archived={archived}"
                );
            }
            assert!(
                header.iter().any(|row| row.text.starts_with("        ")),
                "long session IDs wrap"
            );
            assert!(!state.history().entries.is_empty());
        }
    }

    /// An archived run with every cell filled, for the table tests.
    fn cells(kind: &str, worktree: &str, title: &str, started_at_ms: Option<i64>) -> StripRow {
        StripRow::Run {
            run_id: format!("{kind}:{title}"),
            kind: kind.to_owned(),
            title: title.to_owned(),
            worktree: worktree.to_owned(),
            started_at_ms,
            continuation: false,
        }
    }

    /// What the table draws for one row, which is what a test can compare down a column.
    fn drawn(columns: &Columns, now: i64, row: &StripRow) -> String {
        let StripRow::Run {
            kind,
            title,
            worktree,
            started_at_ms,
            continuation,
            ..
        } = row
        else {
            panic!("not a run row");
        };
        archive_row(
            columns,
            now,
            kind,
            worktree,
            title,
            *continuation,
            *started_at_ms,
        )
    }

    /// The point of the table: a column is read down, so a cell's start cannot depend on how
    /// long the cell above it was.
    #[test]
    fn the_archive_table_lines_its_columns_up() {
        let now = 60_000;
        let rows = vec![
            cells("claude", "hocus", "fix the pane exit teardown", Some(now)),
            cells(
                "codex",
                "zellij",
                "add the archive table",
                Some(now - 7_200_000),
            ),
        ];
        let columns = archive_columns(&rows, 60);
        assert_eq!(
            (columns.age, columns.kind, columns.worktree),
            (true, 6, 6),
            "the widest cell in each column, and the age gutter",
        );
        let drawn: Vec<String> = rows.iter().map(|row| drawn(&columns, now, row)).collect();
        assert_eq!(
            drawn,
            [
                "now claude  hocus   fix the pane exit teardown",
                " 2h codex   zellij  add the archive table",
            ],
        );
        for row in &drawn {
            assert_eq!(
                row.find("fix").or_else(|| row.find("add")),
                Some(agents::AGE_WIDTH + 6 + COLUMN_GAP + 6 + COLUMN_GAP),
                "every prompt starts in the same column",
            );
        }
    }

    /// A pane too narrow for the lot sheds the agent first and the worktree next, because the
    /// prompt is what the row is for and the kind is the most guessable thing on it.
    #[test]
    fn a_narrow_pane_sheds_the_agent_then_the_worktree_then_the_age() {
        let now = 0;
        let rows = vec![cells(
            "claude",
            "hocus",
            "fix the pane exit teardown",
            Some(now),
        )];
        let widths = |cols: usize| {
            let columns = archive_columns(&rows, cols);
            (
                columns.age,
                columns.kind,
                columns.worktree,
                drawn(&columns, now, &rows[0]),
            )
        };
        assert_eq!(
            widths(48),
            (
                true,
                6,
                5,
                "now claude  hocus  fix the pane exit teardow".to_owned()
            ),
            "one column short of the whole prompt, so the cut is the tail and nothing else",
        );
        assert_eq!(
            widths(30),
            (true, 0, 5, "now hocus  fix the pane ex".to_owned()),
            "the agent goes first",
        );
        assert_eq!(
            widths(22),
            (true, 0, 0, "now fix the pane e".to_owned()),
            "then the worktree",
        );
        assert_eq!(
            widths(14),
            (false, 0, 0, "fix the pa".to_owned()),
            "then the age, and the prompt is cut to whatever is left",
        );
    }

    /// A row from before the archive recorded a start still has to hold its place in the
    /// columns, or every cell to the right of it steps left on that one line.
    #[test]
    fn a_run_that_cannot_say_when_it_started_keeps_the_columns_in_line() {
        let rows = vec![
            cells("claude", "hocus", "newer", Some(0)),
            cells("claude", "hocus", "older", None),
        ];
        let columns = archive_columns(&rows, 60);
        let drawn: Vec<String> = rows.iter().map(|row| drawn(&columns, 0, row)).collect();
        assert_eq!(
            drawn,
            ["now claude  hocus  newer", "    claude  hocus  older"]
        );
    }

    /// The chain marker lives in the prompt cell now - the kind has a column of its own, so
    /// there is nothing for it to elide.
    #[test]
    fn a_continuation_is_marked_inside_the_prompt_cell() {
        let mut row = cells(
            "claude",
            "hocus",
            "continued after a context clear",
            Some(0),
        );
        let StripRow::Run { continuation, .. } = &mut row else {
            panic!("not a run row");
        };
        *continuation = true;
        let columns = archive_columns(&[row.clone()], 60);
        assert_eq!(
            drawn(&columns, 0, &row),
            "now claude  hocus  ↳ continued after a context clear",
        );
    }

    /// A search hit is drawn in the archive's columns, so the hits can be read down them too -
    /// with what matched first, since the cell is narrow and why a row matched is its point.
    #[test]
    fn a_search_hit_fills_the_same_cells() {
        let mut state = State {
            search_base: Some((Vec::new(), BTreeMap::new())),
            search_pending: true,
            search_request: 0,
            ..State::default()
        };
        assert!(state.apply_search_result(wire::SearchResult {
            request_id: "0".to_owned(),
            runs: vec![wire::SearchRun {
                run_id: "claude:session-1".to_owned(),
                kind: "claude".to_owned(),
                title: "hook history".to_owned(),
                revision: 4,
                snippet: "the matching line".to_owned(),
                started_at_ms: 10,
                worktree: "hocus".to_owned(),
            }],
            truncated: false,
            error: String::new(),
        }));
        assert_eq!(
            state.rows,
            [StripRow::Run {
                run_id: "claude:session-1".to_owned(),
                kind: "claude".to_owned(),
                title: "the matching line — hook history".to_owned(),
                worktree: "hocus".to_owned(),
                started_at_ms: Some(10),
                continuation: false,
            }],
        );
    }

    /// An archived run row as hocus sends one: the cells a test does not name are the ones it
    /// is not about.
    fn run_row(run_id: impl Into<String>, title: &str) -> StripRow {
        StripRow::Run {
            run_id: run_id.into(),
            kind: "claude".to_owned(),
            title: title.to_owned(),
            worktree: String::new(),
            started_at_ms: None,
            continuation: false,
        }
    }

    /// One `AgentRun`, as hocus pipes it over for the selected row.
    fn a_run() -> agents::AgentRun {
        agents::AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 3,
            zellij_session_name: "zellij".to_owned().into(),
            host_id: zellij_tile::prelude::HostId::default(),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            worktree: None,
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
            omitted_entries: 0,
        }
    }
    // -- a prompt a harness wrote --------------------------------------------

    /// A notification of the shape Claude Code submits when a background task wakes an agent.
    fn task_notification() -> String {
        [
            "<task-notification>",
            "<task-id>a10fe836c6a9fa4ca</task-id>",
            "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
            "<status>completed</status>",
            "<summary>Agent \"/code-review\" finished</summary>",
            "<note>A task-notification fires each time this agent stops.",
            "The same task-id may notify more than once.</note>",
            "<result>The review found four issues.",
            "Two are in the scanner.</result>",
            "</task-notification>",
        ]
        .join("\n")
    }

    /// [`task_notification`] with a `note` and a `result` long enough to scroll through: what
    /// a real one carries, where the fold rows exist precisely because the body does not fit.
    fn long_notification() -> String {
        // Paragraphs rather than lines: `markdown::render` reflows a run of single-newline
        // lines into one, and forty of them come back as fourteen rows.
        let long = |what: &str| {
            (0..40)
                .map(|line| format!("{what} line {line}."))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        [
            "<task-notification>",
            "<task-id>a10fe836c6a9fa4ca</task-id>",
            "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
            "<status>completed</status>",
            "<summary>Agent \"/code-review\" finished</summary>",
            &format!("<note>{}</note>", long("Note")),
            &format!("<result>{}</result>", long("Result")),
            "</task-notification>",
        ]
        .join("\n")
    }

    /// A run holding one prompt, with the reader open on it.
    fn reading(prompt: &str) -> State {
        reading_entry(agents::RunEntryKind::Prompt, prompt)
    }

    #[test]
    fn reader_traverses_the_tab_bar_and_conversation_without_losing_its_event() {
        let mut state = reading("An event kept open while visiting VMs");
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        state.on_key(KeyWithModifier::new(BareKey::Right));
        assert_eq!(state.surface, Surface::Todos);
        state.on_key(KeyWithModifier::new(BareKey::Right));
        assert_eq!(state.surface, Surface::Vms);
        state.render(8, 20);
        state.on_key(KeyWithModifier::new(BareKey::Left));
        state.on_key(KeyWithModifier::new(BareKey::Left));
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert!(!state.tabs_focused);
        assert!(state.reading.is_some());
        assert!(rows(&state).join("\n").contains("An event kept open"));
    }

    /// [`reading`], for an event of some other kind.
    fn reading_entry(kind: agents::RunEntryKind, text: &str) -> State {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        let mut run = a_run();
        run.summary = None;
        // A `Result` reaches the timeline only on a run whose newest entry is past its last
        // summary - see `agents::run_history`. Every other kind is drawn either way.
        run.summarized_revision = 0;
        run.entries = vec![agents::RunEntry {
            revision: 1,
            event_id: None,
            kind,
            text: text.to_owned(),
            created_at_ms: None,
        }];
        state.set_run(run);
        state.render(21, 50);
        state.focus = Focus::History;
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.render(21, 50);
        state
    }

    fn rows(state: &State) -> Vec<String> {
        state
            .reader_lines()
            .iter()
            .map(|line| line.text.clone())
            .collect()
    }

    fn press(state: &mut State, key: BareKey) {
        state.on_key(KeyWithModifier::new(key));
        state.render(21, 50);
    }

    /// The four-row cap on the timeline is only honest because this exists. `Enter` opens the
    /// event into its parts, with the short ones showing and the long ones shut — and `Enter`
    /// again on one opens it.
    #[test]
    fn the_reader_folds_a_task_notification_and_enter_opens_a_section() {
        let mut state = reading(&task_notification());
        let folds = &state.reading.as_ref().expect("a reader").sections;
        assert_eq!(
            folds.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            [
                "status",
                "task-id",
                "summary",
                "output-file",
                "note",
                "result"
            ]
        );

        let shut = rows(&state).join("\n");
        assert!(shut.contains("completed"), "{shut}");
        assert!(shut.contains("a10fe836c6a9fa4ca"), "{shut}");
        assert!(shut.contains("▸ result"), "{shut}");
        assert!(!shut.contains("The review found four issues."), "{shut}");

        // Walk to `result` and open it.
        for _ in 0..5 {
            press(&mut state, BareKey::Char('j'));
        }
        assert_eq!(state.reading.as_ref().expect("a reader").cursor, 5);
        press(&mut state, BareKey::Enter);
        let open = rows(&state).join("\n");
        assert!(open.contains("▾ result"), "{open}");
        assert!(open.contains("The review found four issues."), "{open}");
        // And the fold beside it stayed shut.
        assert!(!open.contains("notify more than once"), "{open}");
    }

    /// `j`/`k` walk a fold where there are folds and a line where there are not — the same
    /// split the history half makes between an event and a line, and for the same reason.
    #[test]
    fn j_and_k_walk_the_folds_of_a_folding_reader_and_the_lines_of_every_other() {
        let mut state = reading(&task_notification());
        press(&mut state, BareKey::Char('j'));
        let folding = state.reading.as_ref().expect("a reader");
        assert_eq!(folding.cursor, 1);
        assert_eq!(folding.scroll, 0, "walking a fold is not scrolling");

        let mut prose = reading(&"word ".repeat(200));
        assert!(prose
            .reading
            .as_ref()
            .expect("a reader")
            .sections
            .is_empty());
        press(&mut prose, BareKey::Char('j'));
        assert_eq!(prose.reading.as_ref().expect("a reader").scroll, 1);
    }

    /// The folds are a reading of the event; `v` is how you get back to the event. A reader
    /// that could not would be asking to be trusted.
    #[test]
    fn v_shows_the_event_exactly_as_it_arrived() {
        let mut state = reading(&task_notification());
        press(&mut state, BareKey::Char('v'));
        let raw = rows(&state).join("\n");
        assert!(raw.contains("<task-notification>"), "{raw}");
        assert!(
            raw.contains("<task-id>a10fe836c6a9fa4ca</task-id>"),
            "{raw}"
        );

        press(&mut state, BareKey::Char('v'));
        let folded = rows(&state).join("\n");
        assert!(!folded.contains("<task-notification>"), "{folded}");
        assert!(folded.contains("▸ result"), "{folded}");
    }

    /// Opening a fold is a different body, not a different view of one, so it has to miss the
    /// cache the way a resize does.
    #[test]
    fn toggling_a_fold_rewraps_the_body() {
        let mut state = reading(&task_notification());
        let poison = envelope::ReaderLines {
            lines: vec![markdown::MdLine::plain("poison")],
            starts: vec![0],
        };
        let key = state.reader_cache.as_ref().expect("a cache").0;
        state.reader_cache = Some((key, poison.clone()));
        state.fill_reader_cache(50);
        assert_eq!(state.reader_lines(), poison.lines, "same event, same fold");

        // Onto `result`, which is the first fold with a body to hide. Walking there is not a
        // new body: the cursor is drawn from the spans, which the same rows already carry.
        for _ in 0..5 {
            state.on_key(KeyWithModifier::new(BareKey::Char('j')));
        }
        state.fill_reader_cache(50);
        assert_eq!(
            state.reader_lines(),
            poison.lines,
            "moving the cursor is not a new body"
        );

        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.fill_reader_cache(50);
        assert_ne!(state.reader_lines(), poison.lines, "a fold rewraps it");

        // And a one-line field has nothing to fold away, so Enter on one buys no rewrap.
        let key = state.reader_cache.as_ref().expect("a cache").0;
        state.reader_cache = Some((key, poison.clone()));
        for _ in 0..5 {
            state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        }
        state.on_key(KeyWithModifier::new(BareKey::Enter));
        state.fill_reader_cache(50);
        assert_eq!(state.reader_lines(), poison.lines, "nothing to fold");
    }

    /// The whole reason the path is on the row at all. From the reader it is the fold under the
    /// cursor; from the history half it is the event's, because a collapsed row has a cursor on
    /// an event and none within one.
    #[test]
    fn o_on_a_task_notification_names_the_file_it_would_open() {
        let mut state = reading(&task_notification());
        for _ in 0..3 {
            press(&mut state, BareKey::Char('j'));
        }
        assert_eq!(
            state.open_target().as_deref(),
            Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
        );

        // A fold that names no file offers none.
        press(&mut state, BareKey::Char('j'));
        assert_eq!(state.open_target(), None);

        // And from the history half, with no reader open at all.
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        assert!(state.reading.is_none());
        assert_eq!(
            state.open_target().as_deref(),
            Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
        );
    }

    /// The path names a file in another machine's temp folder. Opening it here would either
    /// fail or, worse, open something else entirely.
    #[test]
    fn a_run_on_another_host_offers_no_file_to_open() {
        let mut state = reading(&task_notification());
        for _ in 0..3 {
            press(&mut state, BareKey::Char('j'));
        }
        assert!(state.open_target().is_some(), "the local run offers one");

        state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
        assert_eq!(state.open_target(), None);
        // `o` says where the file is rather than doing nothing, and leaves the panel up.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        assert_eq!(state.panel_hides, 0);
        assert_eq!(state.open_note.as_deref(), Some("file is on buildbox"));
        assert!(state.reader_label().starts_with("file is on buildbox"));
    }

    /// Where a file is on another machine, `o` says which one - but only where there is a
    /// file: a fold or an event that names none names none on any machine.
    #[test]
    fn o_on_another_hosts_run_says_where_only_when_there_is_a_file() {
        let mut state = reading(&task_notification());
        state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        assert_eq!(state.open_note.as_deref(), Some("no file named here"));

        let mut state = reading("fix the flaky reconnect test");
        state.run.as_mut().expect("a run").host = Some("buildbox".to_owned());
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        assert_eq!(state.open_note.as_deref(), Some("no file named here"));
        assert_eq!(state.panel_hides, 0);
    }

    /// `y` copies the fold under the `>` in a folding reader, and the whole event exactly as it
    /// arrived from the source `v` puts up or from a prose event - so `v` then `y` is how a
    /// folded event is copied whole.
    #[test]
    fn y_copies_the_fold_under_the_cursor_and_the_whole_event_otherwise() {
        let mut state = reading(&task_notification());
        assert_eq!(state.copy_target().as_deref(), Some("completed"));

        for _ in 0..5 {
            press(&mut state, BareKey::Char('j'));
        }
        assert_eq!(
            state.copy_target().as_deref(),
            Some("The review found four issues.\nTwo are in the scanner.")
        );

        // Showing the source is asking for the source.
        press(&mut state, BareKey::Char('v'));
        assert_eq!(
            state.copy_target().as_deref(),
            Some(task_notification().as_str())
        );

        let prose = reading("fix the flaky reconnect test");
        assert_eq!(
            prose.copy_target().as_deref(),
            Some("fix the flaky reconnect test")
        );
    }

    /// A copy is invisible until it is pasted, and the fold and the whole event are two
    /// different things to paste: `y` says which it took, and how to get the other, on the
    /// divider until the next key.
    #[test]
    fn y_says_what_it_copied() {
        let mut state = reading(&task_notification());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
        assert!(
            state
                .reader_label()
                .starts_with("copied status · v then y copies the whole event · prompt"),
            "{}",
            state.reader_label()
        );
        press(&mut state, BareKey::Char('v'));
        assert!(state.open_note.is_none(), "the next key takes it down");
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('y'))));
        assert!(
            state
                .reader_label()
                .starts_with("copied the whole event · prompt"),
            "{}",
            state.reader_label()
        );
    }

    /// A line scroll is how a long fold is read, and the cursor comes along with it: what
    /// `Enter`, `y` and `o` act on is always something on screen. Left at the head of the
    /// event, the cursor pointed at a fold the user had walked ten pages away from, with no
    /// `>` anywhere to say which one - and `k` on it did nothing at all, having nowhere left
    /// to go.
    #[test]
    fn scrolling_the_reader_takes_the_cursor_with_it() {
        let mut state = reading(&long_notification());
        // Onto `result` and open it, so there is more body than the pane has room for.
        for _ in 0..5 {
            press(&mut state, BareKey::Char('j'));
        }
        press(&mut state, BareKey::Enter);
        // Back to the head of the event, which the cursor comes back to with it.
        for _ in 0..80 {
            press(&mut state, BareKey::Up);
        }
        let reader = state.reading.as_ref().expect("a reader");
        assert_eq!((reader.scroll, reader.cursor), (0, 0));

        for _ in 0..8 {
            press(&mut state, BareKey::Down);
        }
        let starts = state.reader_starts().to_vec();
        let reader = state.reading.as_ref().expect("a reader");
        assert_eq!(reader.scroll, 8, "eight lines down into the event");
        assert_ne!(
            reader.cursor, 0,
            "and off the fold the reader opened on, which is what `Enter` would have acted on"
        );
        assert_eq!(
            Some(reader.cursor),
            state.fold_at(reader.scroll),
            "the cursor is the fold owning the top row"
        );
        assert!(starts[reader.cursor] <= reader.scroll);
    }

    /// A fold taller than the pane is shown from its beginning - the row carrying its name.
    /// Without the winding it opened where it sat, four rows from the foot, and spent the rest
    /// of itself below the screen.
    #[test]
    fn opening_a_fold_shows_it_from_its_first_row() {
        let mut state = reading(&long_notification());
        for _ in 0..5 {
            press(&mut state, BareKey::Char('j'));
        }
        press(&mut state, BareKey::Enter);

        let starts = state.reader_starts().to_vec();
        let reader = state.reading.as_ref().expect("a reader");
        assert_eq!(reader.cursor, 5);
        assert_eq!(reader.scroll, starts[5], "the fold's name is the top row");
    }

    /// The reader's parse depends on the event's text alone, which an opening never changes,
    /// so leaving the source with `v` and coming back re-wraps the parse made the first time
    /// rather than parsing the whole event again. Read by poisoning that parse.
    #[test]
    fn toggling_raw_keeps_the_readers_parse() {
        let mut state = reading(&task_notification());
        press(&mut state, BareKey::Char('v'));
        let id = state.reading.as_ref().expect("a reader").id;
        assert_eq!(
            state.reader_blocks.as_ref().map(|(held, _)| *held),
            Some(id)
        );

        state.reader_blocks = Some((id, markdown::parse("poison").into()));
        press(&mut state, BareKey::Char('v'));
        press(&mut state, BareKey::Char('v'));
        let lines: Vec<&str> = state
            .reader_lines()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(lines, ["poison"]);
    }

    /// A fold's parse depends on the section and whether it is open, and nothing else: a
    /// resize re-wraps what is held, a fold toggled parses that fold alone. Read by poisoning
    /// one fold's parse, as `toggling_raw_keeps_the_readers_parse` does the prose one.
    #[test]
    fn each_fold_is_parsed_once_per_toggle_and_rewrapped_per_width() {
        let mut state = reading(&task_notification());
        let folds = &mut state.reader_folds.as_mut().expect("held folds").1;
        assert_eq!(folds.len(), 6);
        folds[4].1 = markdown::parse("poison");
        state.render(21, 44);
        let has_poison = |state: &State| rows(state).iter().any(|row| row.contains("poison"));
        assert!(has_poison(&state), "a resize re-wraps what is held");

        for _ in 0..5 {
            press(&mut state, BareKey::Char('j'));
        }
        press(&mut state, BareKey::Enter);
        assert!(
            rows(&state)
                .join("\n")
                .contains("The review found four issues."),
            "the fold toggled is parsed open"
        );
        assert!(has_poison(&state), "and the one beside it is left alone");

        press(&mut state, BareKey::Char('k'));
        press(&mut state, BareKey::Enter);
        assert!(!has_poison(&state), "until it is toggled itself");
        assert!(rows(&state).join("\n").contains("▾ note"));
    }

    /// An event the timeline parsed whole is opened on that parse rather than a new one, and
    /// the reader holds the event's own text rather than a copy. One it parsed only in part,
    /// or drew from a digest, is parsed when the reader first draws it.
    #[test]
    fn a_short_event_opens_on_the_timelines_parse() {
        let state = reading_entry(agents::RunEntryKind::Message, "Shall I **push** it?");
        let opened = state.reading.as_ref().expect("a reader");
        let parsed = &state.run.as_ref().expect("the run").parsed;
        let shared = parsed.whole_blocks(&opened.text).expect("parsed whole");
        let (_, held) = state.reader_blocks.as_ref().expect("the reader's parse");
        assert!(Arc::ptr_eq(held, &shared));
        assert!(Arc::ptr_eq(
            &opened.text,
            &state.cursor_event().expect("an event").text
        ));
        assert!(rows(&state).join("\n").contains("Shall I push it?"));

        for state in [
            reading_entry(agents::RunEntryKind::Message, &"word ".repeat(4_000)),
            reading(&task_notification()),
        ] {
            let opened = state.reading.as_ref().expect("a reader");
            let parsed = &state.run.as_ref().expect("the run").parsed;
            assert!(parsed.whole_blocks(&opened.text).is_none());
        }
    }

    /// `v` puts up a different document, so there is no row to hold the place at and the
    /// reader starts at the head of it. The cursor goes back with the scroll: left where it
    /// was, it pointed forty rows below a window anchored to zero, where nothing draws it and
    /// `Enter`, `y` and `o` would all have acted on a fold nobody could see.
    #[test]
    fn coming_back_from_raw_starts_at_the_top() {
        let mut state = reading(&long_notification());
        // Open `note`, which is long enough to push `result` off the foot, and put the cursor
        // on `result` under it.
        for _ in 0..4 {
            press(&mut state, BareKey::Char('j'));
        }
        press(&mut state, BareKey::Enter);
        press(&mut state, BareKey::Char('j'));
        let starts = state.reader_starts().to_vec();
        assert_eq!(state.reading.as_ref().expect("a reader").cursor, 5);
        assert!(
            starts[5] > state.last_reader_rows,
            "the fold is off the foot at a scroll of zero"
        );

        press(&mut state, BareKey::Char('v'));
        press(&mut state, BareKey::Char('v'));
        let reader = state.reading.as_ref().expect("a reader");
        assert_eq!((reader.scroll, reader.cursor), (0, 0));
    }

    /// A prompt is the one kind a harness writes as well as a person - the same gate the
    /// timeline puts on its digest. An agent writing *about* an envelope is writing prose, and
    /// folding it away would hide the message the reader was opened to read.
    #[test]
    fn a_result_that_quotes_an_envelope_opens_as_prose() {
        let text = format!(
            "I removed the handler. It was woken by this:\n\n{}",
            task_notification()
        );
        let state = reading_entry(agents::RunEntryKind::Result, &text);

        let reader = state.reading.as_ref().expect("a reader");
        assert!(
            reader.sections.is_empty(),
            "no folds on an agent's own words"
        );
        assert_eq!(reader.envelope, None, "and nothing to name on the divider");
        assert_eq!(
            rows(&state),
            markdown::render(&text, 48, markdown::Options::compact())
                .iter()
                .map(|line| line.text.clone())
                .collect::<Vec<_>>()
        );

        // And the prompt beside it is still read apart, which is the half of the gate that
        // must not have moved.
        let prompt = reading(&task_notification());
        assert!(!prompt
            .reading
            .as_ref()
            .expect("a reader")
            .sections
            .is_empty());
    }

    /// The regression guard: everything a person typed reads, scrolls and copies exactly as it
    /// did before any of this existed.
    #[test]
    fn a_prompt_with_no_envelope_reads_exactly_as_it_did_before() {
        let prose = "fix the flaky reconnect test\n\n- check the timeout\n- check the retry";
        let mut state = reading(prose);
        let reader = state.reading.as_ref().expect("a reader");
        assert!(reader.sections.is_empty());
        assert!(!reader.raw);
        assert_eq!(state.reader_starts(), &[] as &[usize]);
        assert_eq!(
            rows(&state),
            markdown::render(prose, 48, markdown::Options::compact())
                .iter()
                .map(|line| line.text.clone())
                .collect::<Vec<_>>()
        );

        // `v` and `Enter` have nothing to act on and say so by doing nothing.
        let before = rows(&state);
        press(&mut state, BareKey::Char('v'));
        press(&mut state, BareKey::Enter);
        assert_eq!(rows(&state), before);
    }

    // -- the keys a view names ------------------------------------------------

    /// A question's answer keys are its tail, and a narrow divider cuts the tail: the footer
    /// asks answer first at every width the panel's half comes in, the divider puts the
    /// question before the run's name, and a pane with no history half still asks.
    #[test]
    fn the_resume_question_keeps_its_answer_keys_at_every_width() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.set_run(a_run());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        for cols in 22..=40 {
            state.render(12, cols);
            let footer = state.footer_hint().unwrap();
            assert!(footer.starts_with("y resume · n"), "{footer:?} at {cols}");
        }
        state.render(8, 30);
        assert_eq!(state.last_history_rows, 0, "no divider to ask on");
        assert_eq!(state.footer_hint().as_deref(), Some("y resume · n cancel"));
        state.render(12, 24);
        let line = divider_line(&state.history_label(), 5, 0, 24);
        assert!(line.contains("y/n"), "{line}");
    }

    /// A refusal is wrapped whole under the divider, which a narrow half cuts to a few words
    /// of it, and the timeline keeps a row under it.
    #[test]
    fn a_refusal_is_wrapped_whole_under_the_divider() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.set_run(a_run());
        let detail = "that run is live again in pane 12, and resuming it would put two agents on \
                      one conversation";
        state.resume_note = Some(("run:claude:session-1".into(), detail.into()));
        let lines = state.refusal_lines(24);
        assert!(lines.len() > 2, "{lines:?}");
        assert!(lines.iter().all(|line| line.width() <= 22), "{lines:?}");
        assert_eq!(lines.join(" "), format!("resume failed: {detail}"));

        state.render(21, 24);
        let header = state.history().header.len() + 1;
        let held = lines.len().min(state.last_history_rows - header - 1);
        assert_eq!(state.last_history_body_top, header + held);
        assert_eq!(
            state.last_history_body_rows,
            state.last_history_rows - header - held
        );
        assert!(state
            .footer_hint()
            .unwrap()
            .starts_with("resume failed: that run"));
        assert_eq!(
            wrap_words("a veryveryverylongword b", 5),
            ["a", "veryv", "eryve", "rylon", "gword", "b"]
        );
    }

    /// A resume that worked has walked the keyboard into the new pane, under the panel: the
    /// panel goes, wherever the cursor has got to since the question was answered.
    #[test]
    fn a_successful_resume_puts_the_panel_away() {
        let succeeded = || {
            let result = wire::ResumeResult {
                run_id: "claude:session-1".to_owned(),
                detail: String::new(),
            };
            PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RESUME_RESULT,
                &Some(serde_json::to_string(&result).expect("encoded")),
                &None,
                false,
            )
        };
        for walked_on in [false, true] {
            let mut state = State::default();
            state.set_rows(unfocused(archive()));
            state.selected = 1;
            state.on_key(KeyWithModifier::new(BareKey::Char('r')));
            state.on_key(KeyWithModifier::new(BareKey::Char('y')));
            assert_eq!(state.resume_requests, ["claude:session-1"]);
            if walked_on {
                state.on_key(KeyWithModifier::new(BareKey::Down));
            }
            assert!(state.pipe(succeeded()));
            assert_eq!(state.panel_hides, 1, "walked on: {walked_on}");
        }
    }

    /// `o` opens the file for this client - floating, focused and shown - and puts the panel
    /// away first, since the editor is under it.
    #[test]
    fn o_opens_the_named_file_and_puts_the_panel_away() {
        let mut state = reading(&task_notification());
        for _ in 0..3 {
            press(&mut state, BareKey::Char('j'));
        }
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        assert_eq!(state.panel_hides, 1);
        assert_eq!(
            state.opened_files,
            ["/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output"]
        );
        assert!(
            state.reading.is_some(),
            "the event is still open when the panel comes back"
        );
    }

    /// Where there is no file, `o` says so - on the reader's divider or the history half's -
    /// and the next key takes the note down.
    #[test]
    fn o_on_a_fold_with_no_file_says_so() {
        let mut state = reading(&task_notification());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        assert_eq!(state.panel_hides, 0);
        assert!(state.opened_files.is_empty());
        assert!(state
            .reader_label()
            .starts_with("no file named here · prompt"));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert!(state.open_note.is_none());

        let mut state = reading("fix the flaky reconnect test");
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        assert_eq!(state.focus, Focus::History);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('o'))));
        state.fill_history_cache(50);
        assert!(state.history_label().starts_with("no file named here · "));
        assert!(
            state.on_key(KeyWithModifier::new(BareKey::Char('x'))),
            "a key that does nothing else still takes the note down"
        );
        assert!(state.open_note.is_none());
    }

    /// The reader's source toggle is `v`, so `r` in there neither shows the source nor arms a
    /// resume - one `Esc` from a resume `y` answers.
    #[test]
    fn reader_r_does_not_show_the_source_or_arm_a_resume() {
        let mut state = reading(&task_notification());
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('r'))));
        assert!(!state.reading.as_ref().expect("a reader").raw);
        assert!(state.confirming_resume.is_none());
        press(&mut state, BareKey::Char('v'));
        assert!(state.reader_label().starts_with("prompt · source"));
    }

    /// The search box scrolls sideways rather than losing its end, and the terminal's caret is
    /// in it - where the next character typed goes.
    #[test]
    fn the_search_box_keeps_its_end_and_its_caret_in_view() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.begin_search();
        state.search_query = "a query long enough to run off the end of the box".into();
        state.render(12, 24);
        let label = state.view_label().unwrap();
        assert!(label.starts_with("Search: "), "{label}");
        assert!(label.width() < 24, "{label}");
        assert!(label.ends_with("box"), "{label}");
        let (_, column) = state.search_box(24);
        assert!(column < 24);
        assert_eq!(state.text_cursor(None, 10, 24), Some((column, 1)));
        state.has_keyboard = Some(false);
        assert_eq!(state.text_cursor(None, 10, 24), None);
    }

    /// An empty list says whether anybody has told it so: before hocus's first strip it is
    /// waiting, and after one it says what to do about having no agents.
    #[test]
    fn an_empty_list_says_whether_hocus_has_said_anything_yet() {
        let mut state = State::default();
        assert_eq!(state.empty_list_message(), ("waiting for hocus…", None));
        state.render(12, 60);
        let footer = state.footer_hint().unwrap();
        assert!(
            !footer.contains("Tab chat"),
            "nobody said there is one: {footer}"
        );
        let empty = serde_json::to_string(&unfocused(Vec::new())).expect("encoded");
        assert!(
            state.pipe(PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_STRIP,
                &Some(empty),
                &None,
                false,
            )),
            "the first strip is news, empty or not"
        );
        assert_eq!(
            state.empty_list_message(),
            ("no agents", Some("Ctrl e n starts one"))
        );
        state.render(12, 60);
        let footer = state.footer_hint().unwrap();
        assert!(footer.contains("Tab chat"), "{footer}");
    }

    fn questions(count: u64) -> agents::AgentRun {
        let mut run = a_run();
        run.summary = None;
        run.entries = (0..count)
            .map(|index| agents::RunEntry {
                revision: index + 1,
                event_id: None,
                kind: agents::RunEntryKind::Question,
                text: format!("question {index}"),
                created_at_ms: None,
            })
            .collect();
        run
    }

    /// The page keys and the wheel move the focused half: the window stays where they put it,
    /// and the cursor comes to the window, so `Enter` and `o` act on something on screen.
    #[test]
    fn the_page_keys_scroll_the_focused_history_half() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(40));
        state.focus = Focus::History;
        state.render(21, 50);
        let on_screen = |state: &State| {
            let end = state.history().entries.len() - state.history_scroll;
            let start = end - state.last_history_body_rows;
            let event = state.cursor_event().expect("a cursor");
            event.start >= start && event.end() <= end
        };
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        state.render(21, 50);
        assert_eq!(state.history_scroll, state.page());
        assert!(on_screen(&state), "the cursor came with the window");
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        state.render(21, 50);
        assert_eq!(state.history_scroll, 2 * state.page());
        assert!(on_screen(&state));
        for _ in 0..2 {
            assert!(state.on_key(KeyWithModifier::new(BareKey::PageDown)));
            state.render(21, 50);
        }
        assert_eq!(state.history_scroll, 0);
        assert!(on_screen(&state));
    }

    #[test]
    fn the_wheel_scrolls_the_focused_history_half() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(40));
        state.render(21, 50);
        let below = (state.last_list_rows + 3) as isize;
        assert!(state.on_mouse(Mouse::LeftClick(below, 0)));
        assert_eq!(state.focus, Focus::History);
        state.on_mouse(Mouse::Hover(below, 0));
        assert!(state.on_mouse(Mouse::ScrollUp(3)));
        state.render(21, 50);
        assert_eq!(
            state.history_scroll, 3,
            "kept, not wound back to the cursor"
        );
    }

    /// Taking the half's keys after wheeling it keeps the window where the wheel left it, and
    /// puts the cursor on screen in it.
    #[test]
    fn taking_the_history_half_keeps_where_the_wheel_left_it() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(40));
        state.render(21, 50);
        state.on_mouse(Mouse::Hover((state.last_list_rows + 3) as isize, 0));
        assert!(state.on_mouse(Mouse::ScrollUp(5)));
        state.render(21, 50);
        assert_eq!(state.history_scroll, 5);
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        state.render(21, 50);
        assert_eq!(state.history_scroll, 5);
        let end = state.history().entries.len() - 5;
        let event = state.cursor_event().expect("a cursor");
        assert!(event.start >= end - state.last_history_body_rows && event.end() <= end);
    }

    /// The revision of the event the bottom row of the history's window belongs to.
    fn bottom_revision(state: &State) -> Option<u64> {
        let end = state.history().entries.len() - state.history_scroll;
        let from_newest = state.event_at_row(end - 1)?;
        let events = &state.history().events;
        events[events.len() - from_newest - 1].revision
    }

    /// Off the tail the cursor holds the event it was put on, as a working agent adds more -
    /// and as the oldest are trimmed off - while on the newest it follows the tail.
    #[test]
    fn the_cursor_holds_its_event_as_the_run_grows() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(30));
        state.focus = Focus::History;
        state.render(21, 50);
        for _ in 0..2 {
            state.on_key(KeyWithModifier::new(BareKey::Char('k')));
        }
        state.render(21, 50);
        assert_eq!(state.cursor_event().unwrap().revision, Some(28));
        state.set_run(questions(31));
        state.render(21, 50);
        assert_eq!(state.cursor_event().unwrap().revision, Some(28));
        // The oldest trimmed off and a newer one on: the same event still.
        let mut trimmed = questions(32);
        trimmed.entries.remove(0);
        state.set_run(trimmed);
        state.render(21, 50);
        assert_eq!(state.cursor_event().unwrap().revision, Some(28));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        assert_eq!(
            &*state.reading.as_ref().expect("a reader").text,
            "question 27"
        );

        // On the newest, the cursor follows the tail.
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(30));
        state.focus = Focus::History;
        state.render(21, 50);
        state.set_run(questions(31));
        state.render(21, 50);
        assert_eq!(state.cursor_event().unwrap().revision, Some(31));
    }

    /// A window wound back from the tail holds the text being read as the run grows under it,
    /// and as the pane is resized; at the tail it follows new events.
    #[test]
    fn a_wound_back_window_holds_still_as_the_run_grows() {
        let mut state = State::default();
        state.set_rows(one_run_selected());
        state.set_run(questions(30));
        state.render(21, 50);
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        state.render(21, 50);
        let before = bottom_revision(&state);
        assert!(before.is_some());
        state.set_run(questions(33));
        state.render(21, 50);
        assert_eq!(bottom_revision(&state), before);
        state.render(21, 41);
        assert_eq!(bottom_revision(&state), before, "a resize moves nothing");

        while state.on_key(KeyWithModifier::new(BareKey::PageDown)) {
            state.render(21, 50);
        }
        state.render(21, 50);
        assert_eq!(state.history_scroll, 0);
        state.set_run(questions(35));
        state.render(21, 50);
        assert_eq!(state.history_scroll, 0, "the tail follows");
        assert_eq!(bottom_revision(&state), Some(35));
    }

    /// A search sent and not answered says it is searching, is not sent again by a second
    /// Enter, and gives up after a while with the query still there to retry.
    #[test]
    fn a_search_nobody_answers_times_out_and_keeps_the_query() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.begin_search();
        state.search_query = "flaky".into();
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        let request = state.search_request;
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        assert_eq!(state.search_request, request, "nothing sent twice");
        let late = wire::SearchResult {
            request_id: request.to_string(),
            ..Default::default()
        };
        assert!(!state.expire_search(state.search_deadline_ms - 1));
        assert!(state.expire_search(state.search_deadline_ms));
        assert!(!state.search_pending);
        assert!(state.search_editing);
        assert_eq!(state.search_query, "flaky");
        assert_eq!(state.search_error.as_deref(), Some("hocus did not answer"));
        assert_eq!(
            state.search_failure_lines(24).join(" "),
            "failed: hocus did not answer",
            "said under the box, whole"
        );
        assert!(!state.apply_search_result(late), "a late reply is ignored");
        assert!(state.search_key(KeyWithModifier::new(BareKey::Enter)));
        assert!(state.search_pending, "Enter asks again");
        assert!(state.search_error.is_none());
    }

    /// A failed search stays open on the query as typed, over the rows the last search found,
    /// and `Esc` goes back to that search. `/` over a search edits its query, and `Ctrl-u`
    /// clears it.
    #[test]
    fn a_failed_search_stays_open_on_its_query_and_esc_goes_back_to_the_last_one() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let reply = search_reply(&mut state, &["match", "another match"]);
        assert!(state.apply_search_result(reply));
        let matches = state.rows.clone();
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('/'))));
        assert_eq!(
            state.search_query, "history",
            "/ edits the search it is over"
        );
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()));
        assert!(state.search_query.is_empty());
        for ch in "bar".chars() {
            state.on_key(KeyWithModifier::new(BareKey::Char(ch)));
        }
        assert!(state.on_key(KeyWithModifier::new(BareKey::Enter)));
        let failed = wire::SearchResult {
            request_id: state.search_request.to_string(),
            error: "store busy".into(),
            ..Default::default()
        };
        assert!(state.apply_search_result(failed));
        assert!(state.search_editing);
        assert_eq!(state.search_query, "bar");
        assert_eq!(state.rows, matches, "the last search's rows stay");
        // The reason is under the box, whole, and the footer is the keys that go on from it.
        for cols in [24, 28, 34, 40] {
            state.render(12, cols);
            assert_eq!(
                state.footer_hint().as_deref(),
                Some("Enter retry · Esc back")
            );
            let lines = state.search_failure_lines(cols);
            assert_eq!(lines.join(" "), "failed: store busy", "{cols}");
            assert_eq!(state.last_label_rows, 1 + lines.len(), "{cols}");
        }
        // Clicks land on the list under the reason, not on the reason.
        state.render(12, 24);
        state.selected = state.rows.len() - 1;
        let first = state.rows.iter().position(selectable).unwrap();
        assert!(state.on_mouse(Mouse::LeftClick(
            (1 + state.last_label_rows + first) as isize,
            0
        )));
        assert_eq!(state.selected, first);
        // A reason hocus gives while it is starting up, long enough to need every row it gets.
        state.search_error =
            Some("hocus has not registered this agent list yet - try again".to_owned());
        state.render(12, 24);
        let lines = state.search_failure_lines(24);
        assert!(lines.iter().all(|line| line.width() <= 24), "{lines:?}");
        assert_eq!(
            lines.join(" "),
            "failed: hocus has not registered this agent list yet - try again"
        );
        assert_eq!(state.last_label_rows, 1 + lines.len());
        state.search_error = Some("store busy".to_owned());

        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert_eq!(state.search_query, "history");
        assert!(state.search_error.is_none());
        assert!(!state.search_editing);
        state.render(12, 24);
        assert!(state.search_failure_lines(24).is_empty());
        assert_eq!(state.last_label_rows, 1, "nothing is left under the box");
    }

    /// The search box takes every key ahead of the tab bar, so it cannot stay open behind a
    /// `Tab`: the key closes it, keeps what was typed for the next `/`, and goes on round the
    /// ring from the list it sits over - into the history half, or with Shift to the tab bar.
    #[test]
    fn tab_in_the_search_box_keeps_the_draft_and_walks_the_ring() {
        let key = |ch: char| KeyWithModifier::new(BareKey::Char(ch));
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(12, 60);
        assert!(state.on_key(key('/')));
        for ch in "ab".chars() {
            assert!(state.on_key(key(ch)));
        }
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert!(!state.search_editing);
        assert!(
            state.search_base.is_none(),
            "nothing was applied to go back to"
        );
        assert!(
            state.ring_yields == [wire::STOP_TABS] || state.focus == Focus::History,
            "on round the ring from the list"
        );
        assert_eq!(state.search_draft.as_deref(), Some("ab"));
        // Neither `w` nor `p` puts a search away when there is none: they leave views, and the
        // draft waits for the next `/` through both.
        assert!(state.on_key(key('w')));
        assert!(state.waiting_base.is_some());
        assert!(state.on_key(key('w')));
        assert!(state.waiting_base.is_none());
        assert_eq!(state.search_draft.as_deref(), Some("ab"), "w, w keeps it");
        assert!(matches!(
            state.rows.get(state.selected),
            Some(StripRow::Agent { .. })
        ));
        state.on_key(key('p'));
        assert!(state.predecessors.is_some());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(state.predecessors.is_none());
        assert_eq!(state.search_draft.as_deref(), Some("ab"), "p, Esc keeps it");
        assert!(state.on_key(key('/')));
        assert_eq!(state.search_query, "ab", "the draft comes back");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()));
        assert!(!state.search_editing);
        assert!(state.tabs_focused, "Shift+Tab from the box is the tab bar");
        state.tabs_focused = false;

        // Esc cancels as it always did, and keeps nothing.
        assert!(state.on_key(key('/')));
        assert_eq!(state.search_query, "ab");
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));
        assert!(!state.search_editing);
        assert!(state.on_key(key('/')));
        assert!(state.search_query.is_empty(), "Esc keeps no draft");
        assert!(state.on_key(KeyWithModifier::new(BareKey::Esc)));

        // Over a search already applied, Tab goes back to that search, and `c` puts the draft
        // away with it.
        let reply = search_reply(&mut state, &["match"]);
        assert!(state.apply_search_result(reply));
        assert!(state.on_key(key('/')));
        assert!(state.on_key(key('x')));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert!(!state.search_editing);
        assert_eq!(
            state.search_query, "history",
            "the applied search, under the list"
        );
        assert_eq!(state.search_draft.as_deref(), Some("historyx"));
        assert!(state.on_key(key('c')));
        assert!(state.search_draft.is_none());
        assert!(state.on_key(key('/')));
        assert!(state.search_query.is_empty());
    }

    /// The hits are not run again, so once they are old enough to matter the view says how old.
    #[test]
    fn an_applied_search_says_how_old_it_is() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        let reply = search_reply(&mut state, &["match"]);
        assert!(state.apply_search_result(reply));
        assert_eq!(state.view_label().as_deref(), Some("Search: history"));
        state.searched_at_ms = Some(now_ms() - 42 * 60_000);
        assert_eq!(
            state.view_label().as_deref(),
            Some("Search: history · 42m ago")
        );
        assert!(state.shows_an_age());
        assert!(state.clear_search());
        assert!(state.searched_at_ms.is_none());
    }

    /// Enter on the last page of `load older runs…` puts the cursor on the first run it loaded,
    /// not back at the top of the list; a page that is not the last keeps it on the row.
    #[test]
    fn the_last_archive_page_leaves_the_cursor_on_the_first_run_it_loaded() {
        let archive_of = |runs: &[&str], more: bool| {
            let mut rows = strip();
            rows.push(StripRow::Project("archive".to_owned()));
            rows.extend(runs.iter().map(|id| run_row(*id, "a run")));
            if more {
                rows.push(StripRow::MoreRuns);
            }
            unfocused(rows)
        };
        let mut state = State::default();
        state.set_rows(archive_of(&["claude:1"], true));
        state.selected = state.rows.len() - 1;
        assert_eq!(state.history_placeholder(40), "Enter loads older runs");
        state.set_rows(archive_of(&["claude:1", "claude:2"], true));
        assert_eq!(state.selection_key().as_deref(), Some("more"));
        state.set_rows(archive_of(&["claude:1", "claude:2", "claude:3"], false));
        assert_eq!(state.selection_key().as_deref(), Some("run:claude:3"));
        assert_eq!(state.list_scroll, None);
    }

    /// The history half says which kind of empty it is.
    #[test]
    fn an_empty_history_says_which_kind_of_empty_it_is() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        assert_eq!(state.history_placeholder(40), "loading…");
        let mut empty = a_run();
        empty.summary = None;
        empty.entries.clear();
        state.set_run(empty);
        assert_eq!(state.history_placeholder(40), "no events recorded");
        state.run = None;
        let failed = |run_id: &str| {
            PipeMessage::new(
                PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
                wire::AGENT_RUN_DETAIL,
                &None,
                &Some(BTreeMap::from([
                    ("error".to_owned(), "store unavailable".to_owned()),
                    ("run_id".to_owned(), run_id.to_owned()),
                ])),
                false,
            )
        };
        // About a run the cursor is not on: another row's answer, and not this one's.
        assert!(!state.pipe(failed("claude:somewhere-else")));
        assert_eq!(state.history_placeholder(40), "loading…");
        assert!(state.pipe(failed("claude:session-1")));
        assert_eq!(
            state.history_placeholder(40),
            "history unavailable: store unavailable"
        );
        // hocus says it again on every snapshot, which is not news.
        assert!(!state.pipe(failed("claude:session-1")));
        assert_eq!(state.history_placeholder(12), "too narrow for history");
        state.set_rows(unfocused(strip()));
        assert_eq!(state.history_placeholder(40), "nothing to show yet");
    }

    /// Why a history is unavailable is the end of its sentence, and a half of the width a
    /// panel is drawn at cut it off there: the placeholder wraps over the half's rows instead.
    #[test]
    fn an_unavailable_history_is_wrapped_whole() {
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        assert!(state.pipe(PipeMessage::new(
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
            wire::AGENT_RUN_DETAIL,
            &None,
            &Some(BTreeMap::from([
                (
                    "error".to_owned(),
                    "the run is no longer in the archive".to_owned()
                ),
                ("run_id".to_owned(), "claude:session-1".to_owned()),
            ])),
            false,
        )));
        for (cols, said) in [
            (
                24,
                "history unavailable: the run is no longer in the archive",
            ),
            (
                34,
                "history unavailable: the run is no longer in the archive",
            ),
            (21, "too narrow for history"),
            (12, "too narrow for history"),
        ] {
            let lines = state.placeholder_lines(cols);
            assert!(
                lines.iter().all(|line| line.width() <= cols - 2),
                "{lines:?} at {cols}"
            );
            assert_eq!(lines.join(" "), said, "{cols}");
        }
        state.render(12, 24);
        assert!(state.last_history_body_rows >= state.placeholder_lines(24).len());
    }

    /// The focus is drawn as a glyph as well as a colour, and a dimmed rule keeps its waiting
    /// `!` in the error colour rather than dimming it with the rest.
    #[test]
    fn the_focus_markers_are_glyphs_as_well_as_colours() {
        let focused = divider_text("3 · claude !", 0, 0, true, 30);
        assert!(focused.content().starts_with("─ ▸ 3 · claude !"));
        let unfocused = divider_text("3 · claude !", 0, 0, false, 30);
        assert!(unfocused.content().starts_with("─   3 · claude !"));
        let at = unfocused
            .content()
            .chars()
            .position(|ch| ch == '!')
            .unwrap();
        // The levels `Text` keeps its dim and error ranges at.
        let (dim, error) = (&unfocused.indices()[4], &unfocused.indices()[6]);
        assert!(!dim.contains(&at));
        assert!(dim.contains(&(at - 1)) && dim.contains(&(at + 1)));
        assert!(error.contains(&at));

        let row = agent(3, "  3  claude  busy");
        let columns = Columns::default();
        assert!(list_text(&row, true, true, &columns, 0)
            .content()
            .starts_with('>'));
        assert!(list_text(&row, true, false, &columns, 0)
            .content()
            .starts_with('·'));
        assert!(list_text(&row, false, false, &columns, 0)
            .content()
            .starts_with(' '));
    }

    /// The `!` on the divider is the row's own, which knows the agent was answered - not the
    /// stored summary's, which goes on saying it is waiting.
    #[test]
    fn the_divider_marks_a_wait_only_while_the_row_does() {
        let mut state = State::default();
        state.set_rows(with_summary(
            strip(),
            3,
            "Waiting on a permission prompt.",
            true,
        ));
        let label = state.history_lines(38).label;
        assert!(!label.contains('!'), "{label}");
    }

    /// No label reaches the terminal carrying a character that moves its cursor.
    #[test]
    fn a_label_cannot_move_the_terminal_cursor() {
        let line = divider_line("edit: a\nb\u{1b}[2J", 0, 0, 40);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("edit: a b"), "{line:?}");
    }

    /// A hit's cell opens on the word that matched, where the narrow cell can show it.
    #[test]
    fn a_hit_opens_on_the_word_that_matched() {
        let cell = hit_first(
            "lorem ipsum dolor sit amet consectetur the needle is here",
            "needle",
        );
        assert!(cell.starts_with('…'), "{cell}");
        let at = cell.find("needle").unwrap();
        assert!(cell[..at].chars().count() <= 9, "{cell}");
        assert_eq!(
            hit_first("the needle up front", "needle"),
            "the needle up front"
        );
        assert_eq!(hit_first("nothing matches", "needle"), "nothing matches");
        let folded = hit_first("Case Folded And Then NEEDLE", "\"needle\"");
        assert!(
            folded.starts_with('…') && folded.ends_with("NEEDLE"),
            "{folded}"
        );
    }

    /// `D` and `F` open the New agent modal on the agent under the cursor - named, so the modal
    /// has an agent to copy - with the panel left up under it, so an Esc in the modal comes
    /// back to this list. Nothing on a run.
    #[test]
    fn d_and_f_open_the_new_agent_modal_on_the_agent_under_the_cursor() {
        let fork = pane_action_configuration(7, true);
        assert_eq!(
            fork.get("caller_terminal_pane_id").map(String::as_str),
            Some("7")
        );
        assert_eq!(fork.get("fork_focused").map(String::as_str), Some("true"));
        assert!(!fork.contains_key("duplicate_focused"));
        assert_eq!(
            pane_action_configuration(7, false)
                .get("duplicate_focused")
                .map(String::as_str),
            Some("true")
        );

        let mut state = State::default();
        let mut rows = strip();
        rows.extend(archive());
        state.set_rows(unfocused(rows));
        state.selected = 7;
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('D'))));
        assert!(state.new_agent_launches.is_empty(), "a run has `r`");
        assert_eq!(state.panel_hides, 0);
        state.selected = 3;
        state.on_key(KeyWithModifier::new(BareKey::Char('F')).with_shift_modifier());
        assert_eq!(
            state.new_agent_launches,
            [pane_action_configuration(7, true)]
        );
        assert_eq!(state.panel_hides, 0, "the panel stays up under the modal");
    }

    /// A click on a fold points the reader's cursor at it, and a second one opens it. A prose
    /// reader has nothing to point at and ignores clicks, as it always did.
    #[test]
    fn a_click_on_a_fold_points_at_it_and_a_second_opens_it() {
        let mut state = reading(&task_notification());
        let starts = state.reader_starts().to_vec();
        // In pane rows: the tab bar and the reader's divider are above the body.
        let line = (starts[5] + 2) as isize;
        assert!(state.on_mouse(Mouse::LeftClick(line, 3)));
        let reader = state.reading.as_ref().expect("a reader");
        assert_eq!(reader.cursor, 5);
        assert!(!reader.sections[5].open);
        assert!(state.on_mouse(Mouse::LeftClick(line, 3)));
        assert!(state.reading.as_ref().expect("a reader").sections[5].open);

        // The open fold's body is what is being read, not its target: a click there - or the
        // press a drag to copy from it starts with - used to shut it and wind the reader back.
        state.render(21, 50);
        let starts = state.reader_starts().to_vec();
        let end = starts.get(6).copied().unwrap_or(state.reader_lines().len());
        assert!(end > starts[5] + 1, "the open fold has a body");
        let scroll = state.reading.as_ref().expect("a reader").scroll;
        let body = (starts[5] + 1 - scroll + 2) as isize;
        assert!(!state.on_mouse(Mouse::LeftClick(body, 3)));
        assert!(state.reading.as_ref().expect("a reader").sections[5].open);

        let mut prose = reading(&"word ".repeat(200));
        assert!(!prose.on_mouse(Mouse::LeftClick(3, 3)));
    }

    // -- the key sheet ---------------------------------------------------------

    /// `?` puts the key sheet up, and `Esc`, `q` or `?` take it down again - `Esc` the sheet,
    /// not the panel.
    #[test]
    fn question_mark_opens_the_key_sheet_and_esc_q_or_question_mark_shut_it() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 60);
        for shut in [BareKey::Esc, BareKey::Char('q'), BareKey::Char('?')] {
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
            assert!(state.key_sheet_scroll.is_some());
            state.render(21, 60);
            assert_eq!(state.footer_hint().as_deref(), Some("Esc/? close"));
            assert!(state.on_key(KeyWithModifier::new(shut)));
            assert!(state.key_sheet_scroll.is_none(), "{shut:?}");
            assert_eq!(
                state.panel_hides, 0,
                "{shut:?} shut the sheet, not the panel"
            );
        }
        // Some terminals report the Shift the `?` took.
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?')).with_shift_modifier()));
        assert!(state.key_sheet_scroll.is_some());
        // On the bar, the sheet is the bar's.
        state.key_sheet_scroll = None;
        state.tabs_focused = true;
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        assert!(state.tabs_focused, "the bar keeps the keys under it");
        assert_eq!(state.key_sheet()[0].0, "←/→");
    }

    /// The sheet names every key the list answers - and none it does not on this row - and
    /// scrolls when it is taller than the pane, without steering anything under it.
    #[test]
    fn the_key_sheet_lists_what_the_list_answers_and_scrolls_when_it_is_taller() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 60);
        let keys: Vec<String> = state
            .key_sheet()
            .into_iter()
            .map(|(keys, _)| keys)
            .collect();
        for key in [
            "↑/↓ j/k",
            "Enter",
            "click",
            "s",
            "p",
            "D / F",
            "w",
            "/",
            "PgUp/PgDn",
            "wheel",
            "Tab",
            "Shift+Tab",
            "Ctrl-r",
            "Esc",
            "Ctrl e o",
            "?",
        ] {
            assert!(keys.iter().any(|held| held == key), "{key} in {keys:?}");
        }
        assert!(!keys.iter().any(|held| held == "r"), "r is a run's key");

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        state.render(8, 60);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Esc/? close · j/k scroll")
        );
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        state.render(8, 60);
        assert_eq!(state.key_sheet_scroll, Some(1));
        for _ in 0..50 {
            state.on_key(KeyWithModifier::new(BareKey::Down));
        }
        state.render(8, 60);
        assert_eq!(state.key_sheet_scroll, Some(state.last_sheet_max_scroll));
        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert!(state.on_key(KeyWithModifier::new(BareKey::PageUp)));
        assert_eq!(state.selected, 2, "and walked no list");
    }

    /// An empty list has no row to walk, act on or click and no history half to scroll, so
    /// its sheet lists the keys that lead out of it - the same ones its footer offers.
    #[test]
    fn the_key_sheet_on_an_empty_list_lists_no_row_or_history_keys() {
        for view in ["plain", "waiting", "no matches"] {
            // Every one of these is reached from a strip; before the first there is no `Tab`
            // row, which the next test covers.
            let mut state = State {
                strip_seen: true,
                ..Default::default()
            };
            match view {
                "plain" => {
                    state.set_rows(unfocused(Vec::new()));
                },
                "waiting" => {
                    state.set_rows(unfocused(strip()));
                    assert!(state.toggle_waiting());
                },
                _ => {
                    state.set_rows(unfocused(strip()));
                    let reply = search_reply(&mut state, &[]);
                    assert!(state.apply_search_result(reply));
                },
            }
            state.render(21, 60);
            assert!(state.rows.is_empty(), "{view}");
            let sheet = state.key_sheet();
            let keys: Vec<&str> = sheet.iter().map(|(keys, _)| keys.as_str()).collect();
            for key in [
                "↑/↓ j/k",
                "Enter",
                "click",
                "PgUp/PgDn",
                "wheel",
                "s",
                "p",
                "r",
                "D / F",
            ] {
                assert!(!keys.contains(&key), "{view}: {key} in {keys:?}");
            }
            for key in [
                "w",
                "/",
                "Tab",
                "Shift+Tab",
                "Ctrl-r",
                "Esc",
                "Ctrl e o",
                "?",
            ] {
                assert!(keys.contains(&key), "{view}: {key} in {keys:?}");
            }
            assert_eq!(keys.contains(&"c"), view != "plain", "{view}");
            assert_eq!(
                sheet
                    .iter()
                    .find(|(keys, _)| keys == "Tab")
                    .map(|(_, what)| what.as_str()),
                Some("to the conversation"),
                "{view}"
            );
            for key in [BareKey::Enter, BareKey::PageUp, BareKey::PageDown] {
                assert!(!state.on_key(KeyWithModifier::new(key)), "{view}: {key:?}");
            }
            assert_eq!(state.panel_hides, 0, "{view}");
        }
    }

    /// The sheet agrees with the empty list's footer about `Tab`: not before hocus's first
    /// strip, when there may be no conversation for it to reach, and from that strip on.
    #[test]
    fn the_key_sheet_offers_tab_to_the_conversation_only_once_hocus_has_answered() {
        let mut state = State::default();
        state.render(21, 60);
        assert!(!state.footer_hint().unwrap().contains("Tab chat"));
        let before = state.key_sheet();
        let names: Vec<&str> = before.iter().map(|(keys, _)| keys.as_str()).collect();
        assert!(!names.contains(&"Tab"), "{names:?}");
        for key in ["Shift+Tab", "/", "w", "Esc"] {
            assert!(names.contains(&key), "{key} in {names:?}");
        }
        let empty = serde_json::to_string(&unfocused(Vec::new())).expect("encoded");
        assert!(state.pipe(PipeMessage::new(
            PipeSource::Plugin(zellij_tile::prelude::PluginId::new(0)),
            wire::AGENT_STRIP,
            &Some(empty),
            &None,
            false,
        )));
        state.render(21, 60);
        assert!(state.footer_hint().unwrap().contains("Tab chat"));
        assert_eq!(
            state
                .key_sheet()
                .iter()
                .find(|(keys, _)| keys == "Tab")
                .map(|(_, what)| what.as_str()),
            Some("to the conversation")
        );
    }

    /// The ring works through the sheet, and the sheet then lists the stop the keys went to.
    /// Any key it has no use for shuts it and does what it always does, a click shuts it, and a
    /// `?` typed into a box is a character.
    #[test]
    fn keys_the_sheet_does_not_answer_shut_it_and_do_their_job() {
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        state.render(21, 60);
        state.on_key(KeyWithModifier::new(BareKey::Char('?')));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
        assert_eq!(state.focus, Focus::History);
        assert!(state.key_sheet_scroll.is_some(), "the sheet stays up");
        assert!(state
            .key_sheet()
            .iter()
            .any(|(keys, what)| keys == "Enter" && what.contains("read")));

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('w'))));
        assert!(state.key_sheet_scroll.is_none());
        assert!(state.waiting_base.is_some(), "and the w did its job");
        state.clear_waiting();

        state.begin_search();
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        assert_eq!(state.search_query, "?");
        assert!(state.key_sheet_scroll.is_none());
        state.clear_search();

        state.render(21, 60);
        state.on_key(KeyWithModifier::new(BareKey::Char('?')));
        assert!(state.on_mouse(Mouse::LeftClick(4, 0)));
        assert!(state.key_sheet_scroll.is_none());
        assert_eq!(state.selected, 2, "the click did nothing under the sheet");

        // An armed question stays armed under the sheet, which says how to answer it.
        let mut state = State::default();
        state.set_rows(unfocused(archive()));
        state.selected = 1;
        state.on_key(KeyWithModifier::new(BareKey::Char('r')));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        assert!(state.confirming_resume.is_some());
        assert_eq!(state.key_sheet()[0].0, "y");
        state.on_key(KeyWithModifier::new(BareKey::Esc));
        assert!(state.confirming_resume.is_some(), "Esc shut the sheet");

        // Every surface has a sheet of its own, even one with nothing listed on it yet - on
        // VMs with no hocus to ask, only the key that finds one.
        for (surface, key) in [
            (Surface::Todos, "p"),
            (Surface::Vms, "Ctrl e c"),
            (Surface::Swamp, "Enter"),
        ] {
            let mut state = State {
                surface,
                ..State::default()
            };
            assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
            state.render(60, 40);
            assert!(state.footer_hint().unwrap().starts_with("Esc/? close"));
            assert!(
                state.key_sheet().iter().any(|(keys, _)| keys == key),
                "{surface:?}: {key}"
            );
            state.hide_the_panel();
            assert!(state.key_sheet_scroll.is_none(), "a hide shuts it");
        }
    }

    /// What a key sheet row says after the keys' column, `key_width` wide: the description's
    /// piece on that row, if it has one.
    fn described(line: &str, key_width: usize) -> String {
        use unicode_width::UnicodeWidthChar;
        let mut columns = 0;
        line.chars()
            .skip_while(|ch| {
                let skip = columns < 1 + key_width + 2;
                columns += ch.width().unwrap_or(0);
                skip
            })
            .collect()
    }

    /// Whether `pieces` are `what` wrapped: every word of it, in order, with only the spaces
    /// between them moved - or, where a word was too wide for the column, only the word broken.
    fn rewraps(pieces: &[String], what: &str) -> bool {
        let words = what.split_whitespace().collect::<Vec<_>>().join(" ");
        let squeezed = |text: &str| text.split_whitespace().collect::<String>();
        pieces.join(" ") == words || squeezed(&pieces.concat()) == squeezed(what)
    }

    /// The sheet's descriptions line up whatever the keys' widths, and a narrow pane wraps
    /// both columns onto more rows rather than cutting either or overflowing.
    #[test]
    fn the_key_sheet_lines_its_descriptions_up() {
        let short = key_sheet_lines("j/k", "the next fold", 9, 40);
        let long = key_sheet_lines("Shift+Tab", "to the tab bar", 9, 40);
        assert_eq!((short.len(), long.len()), (1, 1));
        assert_eq!(short[0].0.find("the next"), long[0].0.find("to the"));
        assert_eq!(short[0].1, 4, "the margin and the keys");

        let narrow = key_sheet_lines("Shift+Tab", "to the tab bar", 5, 12);
        assert!(narrow.len() > 1, "{narrow:?}");
        assert!(
            narrow.iter().all(|(line, _)| line.width() <= 12),
            "{narrow:?}"
        );
        let pieces: Vec<String> = narrow
            .iter()
            .map(|(line, _)| described(line, 5))
            .filter(|piece| !piece.is_empty())
            .collect();
        assert_eq!(pieces.join(" "), "to the tab bar", "{narrow:?}");
        assert!(
            narrow.iter().skip(2).all(|(_, keys)| *keys == 0),
            "no keys drawn once the keys have ended: {narrow:?}"
        );
    }

    /// At the width a panel is usually drawn, every sheet wraps its descriptions under their
    /// column rather than cutting them: every word of every entry is on screen, and no row is
    /// wider than the pane.
    #[test]
    fn the_key_sheet_wraps_rather_than_cuts_at_the_default_width() {
        let mut agents = State::default();
        agents.set_rows(unfocused(strip()));
        // No VM, so no lease: every row this tab lists without one.
        let vms = State {
            surface: Surface::Vms,
            ..State::default()
        };
        let mut todos = State {
            surface: Surface::Todos,
            ..State::default()
        };
        todos.todos.mode = todos::Mode::Input {
            action: todos::InputAction::Add { parent_id: None },
            input: zellij_tile::ui_components::forms::TextInput::new(String::new()),
            refusal: None,
        };
        for mut state in [agents, vms, todos] {
            let surface = state.surface;
            state.key_sheet_scroll = Some(0);
            state.render(40, 24);
            let sheet = state.key_sheet();
            let key_width = sheet
                .iter()
                .map(|(keys, _)| keys.width())
                .max()
                .unwrap_or(0)
                .min(23 / 2);
            for (keys, what) in &sheet {
                let lines = key_sheet_lines(keys, what, key_width, 24);
                assert!(
                    lines.iter().all(|(line, _)| line.width() <= 24),
                    "{surface:?}: {lines:?}"
                );
                let pieces: Vec<String> = lines
                    .iter()
                    .map(|(line, _)| described(line, key_width))
                    .filter(|piece| !piece.is_empty())
                    .collect();
                assert!(rewraps(&pieces, what), "{surface:?} {keys}: {pieces:?}");
                if keys == "x" {
                    assert!(pieces.join(" ").ends_with("(asks first)"), "{pieces:?}");
                }
            }
            assert_eq!(
                state.key_sheet_display(24).len(),
                sheet
                    .iter()
                    .map(|(keys, what)| key_sheet_lines(keys, what, key_width, 24).len())
                    .sum::<usize>(),
                "{surface:?}"
            );
        }

        // The footer counts the sheet in the rows it wraps to, and so does its scroll.
        let mut state = State::default();
        state.set_rows(unfocused(strip()));
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('?'))));
        state.render(12, 24);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Esc/? close · j/k scroll")
        );
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('j'))));
        assert_eq!(state.key_sheet_scroll, Some(1));
        for _ in 0..100 {
            state.on_key(KeyWithModifier::new(BareKey::Down));
        }
        state.render(12, 24);
        assert_eq!(
            state.last_sheet_max_scroll,
            state.key_sheet_display(24).len() - 10,
            "the last wrapped row can be scrolled onto the screen"
        );
        assert_eq!(state.key_sheet_scroll, Some(state.last_sheet_max_scroll));

        // No `? keys` on a row where `?` shuts the sheet, at any width.
        for cols in 1..=30 {
            state.render(8, cols);
            let footer = state.footer_hint().unwrap();
            assert!(!footer.contains("? keys"), "{footer:?} at {cols}");
        }
        state.render(8, 22);
        assert_eq!(state.footer_hint().as_deref(), Some("Esc/? close"));
        state.render(8, 24);
        assert_eq!(
            state.footer_hint().as_deref(),
            Some("Esc/? close · j/k scroll")
        );
    }
}
