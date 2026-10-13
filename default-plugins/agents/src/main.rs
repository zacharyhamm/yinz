//! Agent panel, histories, and the four project views.
#[cfg(test)]
#[path = "unit/attention.rs"]
mod attention_tests;
mod overlay;
mod service;
use std::collections::BTreeMap;
use std::sync::Arc;

use agent_proto::agents::{self, now_ms, Strip, StripRow, Summaries};
use agent_proto::envelope;
#[cfg(not(test))]
use agent_proto::launch;
use agent_proto::markdown;
use agent_proto::panel_text::*;
use agent_proto::tabs;
use agent_proto::wire;
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::*;

#[derive(Default)]
struct State {
    mount_pending: bool,
    configuration: BTreeMap<String, String>,
    service_id: Option<PluginId>,
    service_seq: u64,
    snapshot: Option<wire::PollReply>,
    archived_runs: Vec<agents::AgentRunHeader>,
    archive_offset: u32,
    archive_loaded_through: Option<(i64, String)>,
    archive_more: bool,
    panel: overlay::Panel,
    pane_tabs: Option<PaneTabsId>,
    own_plugin_id: PluginId,
    own_client_id: ClientId,
    initial_cwd: std::path::PathBuf,
    bar_focused: Vec<ClientId>,
    tabs_focused: bool,
    age_minute: i64,
    /// The strip as agent last sent it.
    rows: Vec<StripRow>,
    /// What the daemon's model last made of each idle agent, as agent last sent it.
    ///
    /// The whole map rather than only the selected row's: the selection moves here, under `j`/`k`,
    /// and asking agent for the box on every keystroke would put a round trip between the cursor
    /// and what it is pointing at.
    summaries: Summaries,
    /// Full history for the selected row, fetched on demand by agent, held as its parse. See
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
    /// Which half the keys work. Not reset by anything agent pushes: a strip arriving while
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
    /// Whether the host last reported the panel shown, which is the edge `own_pane_changed`
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
    /// Every run this pane has asked agent to resume, in order.
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
    /// How many times this pane has asked agent for older runs. Test-only, for `ring_yields`'
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
    /// Whether any strip has arrived from agent yet. Before one has, the list is empty because
    /// nobody has said what is in it - which is not the same thing as a session with no agents,
    /// and must not be drawn as one.
    strip_seen: bool,
    /// Why agent could not fetch the history of the row it names, as a `selection_key`.
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
    /// The last pane agent reported focused, agent or not.
    ///
    /// Two jobs. The selection follows it when it *changes* onto an agent, which
    /// is what makes walking to a pane move the cursor here — comparing rather
    /// than assigning is what leaves `j`/`k` alone, since the strip is pushed on
    /// every snapshot and would otherwise drag the cursor back once a second.
    /// And the selected row is drawn live only while this names it, so a
    /// selection you moved off the pane you are in says so.
    ///
    /// A `None` from agent is ignored on both counts: it means the focus is on a
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
    /// (`"agent_resume_result"`): the user answered a confirmation, and an answered
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
    /// When a search sent and not yet answered stops being worth waiting for. agent answers
    /// synchronously, so a search still out long after it was sent was dropped - or went to a
    /// agent that is not there - and a box that goes on looking the same says neither.
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
    /// agent sends the selected run again each time it is asked and each time it grows, and a
    /// long run is a quarter of a megabyte of it. Compared before anything is decoded, so a run
    /// sent again unchanged costs one hash rather than a decode - without keeping the bytes to
    /// compare against.
    fingerprint: (usize, u64),
    /// The run, parsed and wrapped at the last width asked for - what a `history_cache` miss
    /// is rebuilt from.
    ///
    /// The cache misses on every step of a resize and on every minute, and agent sends the
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
    /// The strip - the summary agent pushes for the row, and the row itself:
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

/// How long a search is worth waiting on. agent answers from its store synchronously, so this
/// is long enough for a slow store and short enough that a dropped request is noticed.
const SEARCH_TIMEOUT_MS: i64 = 15_000;

/// Which half of the pane the keys work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    List,
    History,
}

/// Check request deadlines; agent ages redraw only when their minute changes.
const POLL_TICK_SECS: f64 = 2.0;

register_plugin!(State);

impl YinzPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.configuration = configuration.clone();
        init_agent_providers();
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::PastedText,
            EventType::PaneTabsUpdate,
            EventType::Timer,
            EventType::Visible,
            EventType::PaneUpdate,
            EventType::SessionUpdate,
            EventType::TabUpdate,
            EventType::PluginConfigurationChanged,
        ]);
        set_timeout(FractionalSeconds::new(POLL_TICK_SECS));
        set_selectable(true);
        set_self_mouse_selection_support(true);
        let ids = get_plugin_ids();
        self.own_plugin_id = ids.plugin_id;
        self.own_client_id = ids.client_id;
        self.initial_cwd = ids.initial_cwd;
        rename_plugin_pane(self.own_plugin_id, "Agents");
        if !configuration.contains_key("view") {
            self.mount_pending = true;
            self.mount_composition();
        }
        self.send_selection();
        self.subscribe_to_service();
        self.load_more_agent_runs();
    }

    fn update(&mut self, event: Event) -> bool {
        let is_key = matches!(event, Event::Key(_));
        let is_input = matches!(
            event,
            Event::Key(_) | Event::Mouse(_) | Event::PastedText(_)
        );
        if is_input {
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
            Event::PluginConfigurationChanged(configuration) => {
                self.configuration = configuration;
                self.place_panel(false);
                true
            },
            Event::TabUpdate(tabs) => {
                #[cfg(not(test))]
                {
                    self.own_client_id = get_plugin_ids().client_id;
                }
                self.panel_tabs_changed(tabs);
                false
            },
            Event::SessionUpdate(sessions, _) => {
                self.discover_service(&sessions);
                false
            },
            Event::Visible(visible) => {
                if !visible {
                    self.end_visit();
                }
                !visible
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
                self.panel_panes_changed(&manifest);
                let keys = panel_pane(&manifest, self.own_plugin_id).is_some_and(|pane| {
                    let focused =
                        own_pane(&manifest, self.own_plugin_id).is_some_and(|own| own.is_focused);
                    self.own_pane_changed(!pane.is_suppressed, focused)
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
                keys
            },
            Event::Key(key) => self.on_key(key),
            Event::PastedText(text) => self.on_paste(&text),
            Event::Mouse(mouse_event) => self.on_mouse(mouse_event),
            // The ages in the history half are drawn from the clock, so nothing the pane
            // receives can tell it they have moved on. Re-armed whatever the answer, so the
            // column keeps ticking; a redraw only where an age is actually on screen, so a
            // pane showing nothing time-dependent still costs nothing to leave open.
            Event::Timer(_) => {
                set_timeout(yinz_tile::prelude::FractionalSeconds::new(POLL_TICK_SECS));
                let now = now_ms();
                let search_expired = self.expire_search(now);
                let summary_expired = self.expire_summary_ask(now);
                let minute = now / 60_000;
                let changed = minute != self.age_minute;
                self.age_minute = minute;
                search_expired || summary_expired || (changed && self.shows_an_age())
            },
            _ => false,
        };
        // Where the cursor moved to is agent's business, not only this pane's: it
        // draws the selected agent's summary. Compared rather than sent on every
        // key, so holding `j` at the end of the list says nothing.
        let moved = self.follow_selection(selection);
        if !moved && revision != self.selected_revision() {
            self.leave_row();
            self.pending_history_revision = self.selected_revision();
        }
        if moved || revision != self.selected_revision() {
            self.send_selection();
        }
        if is_key && self.tabs_focused && !had_bar {
            if let Some(id) = self.pane_tabs {
                let _ = focus_pane_tabs(id, true);
            }
        }
        if is_key {
            self.bar_focused
                .retain(|client| *client != self.own_client_id);
            if self.tabs_focused {
                self.bar_focused.push(self.own_client_id);
            }
        }
        rendered
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_HELLO {
            if self
                .service_id
                .is_some_and(|id| pipe_message.source == PipeSource::Plugin(id))
            {
                self.service_seq = 0;
                self.subscribe_to_service();
            }
            return false;
        }
        if pipe_message.name == wire::AGENT_SNAPSHOT {
            return self.receive_snapshot(&pipe_message);
        }
        if pipe_message.name == wire::AGENT_STRIP_REPOSITION {
            self.reposition();
            return false;
        }
        if pipe_message.name == wire::AGENT_STRIP_YIELD {
            return self.take_ring_stop(Some(wire::STOP_TABS));
        }
        #[cfg(not(test))]
        {
            self.own_client_id = get_plugin_ids().client_id;
        }
        if pipe_message.name == wire::AGENT_STRIP_TAKE {
            // The ring stepped into this pane from the conversation. agent holds our plugin id
            // and has already moved the keyboard; all this carries is which of our two halves
            // the arriving keys belong to.
            return self
                .take_ring_stop(pipe_message.args.get(wire::PANEL_STOP).map(String::as_str));
        }
        false
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.draw(rows, cols);
    }
}

#[cfg(test)]
impl State {
    fn fixture_pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if matches!(
            pipe_message.name.as_str(),
            wire::AGENT_HELLO
                | wire::AGENT_SNAPSHOT
                | wire::AGENT_STRIP_REPOSITION
                | wire::AGENT_STRIP_YIELD
                | wire::AGENT_STRIP_TAKE
        ) {
            return self.pipe(pipe_message);
        }
        if pipe_message.name == "agent_predecessors_result" {
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
        if pipe_message.name == "agent_search_result" {
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
        if pipe_message.name == "agent_resume_result" {
            let Some(result) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::ResumeResult>(json).ok())
            else {
                return false;
            };
            // Empty detail is success, and success puts the panel away wherever the cursor
            // is now. agent has already walked the keyboard into the resumed pane, and an
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
        if pipe_message.name == "agent_run_detail" {
            let revision = pipe_message
                .args
                .get("revision")
                .and_then(|value| value.parse::<u64>().ok());
            if revision != self.selected_revision() {
                return false;
            }
            let Some(payload) = pipe_message.payload else {
                // No run, and why: agent could not fetch the selected archived run's history.
                // Said in the half, which would otherwise go on saying it is loading.
                let Some(error) = pipe_message.args.get("error") else {
                    return false;
                };
                // Named by the run it is about, the way a resume refusal is, and dropped when
                // that is not the row under the cursor: a reply to a selection the user has
                // walked off must not say another run's history is unavailable. agent sends it
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
        if pipe_message.name != "agent_strip" {
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
        // for agent and starts saying what agent said.
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
}

impl State {
    fn draw(&mut self, rows: usize, cols: usize) {
        {
            #[cfg(not(test))]
            let client = get_plugin_ids().client_id;
            #[cfg(test)]
            let client = self.own_client_id;
            self.tabs_focused = self.bar_focused.contains(&client);
        }
        self.last_rows = rows;
        self.last_cols = cols;
        self.last_footer_row = None;
        self.last_label_rows = 0;
        // Settled before anything is drawn, because the surface's share of the pane - what is
        // left under the tab bar and above the footer - is what the cursor is placed in.
        let footer = self.footer_hint();
        let body_rows = rows.saturating_sub(usize::from(footer.is_some()));
        show_cursor(self.text_cursor(body_rows, cols));
        if rows == 0 || cols == 0 {
            // Forget the hidden geometry, but keep selections and scrolls for when it returns.
            self.last_window_start = 0;
            self.last_list_rows = 0;
            self.last_history_rows = 0;
            self.last_history_body_rows = 0;
            self.last_history_max_scroll = 0;
            self.last_reader_rows = 0;
            self.last_reader_max_scroll = 0;
        }
        if rows == 0 || cols == 0 {
            return;
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
        [("Agents", "agents", "agents"), ("Todos", "todos", "")]
            .into_iter()
            .map(|(label, plugin, view)| PaneTabDescriptor {
                label: label.into(),
                location: format!("yinz:{plugin}"),
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
    /// Shows the host bar's keys while it has focus, otherwise the current Agents mode.
    fn footer_hint(&self) -> Option<String> {
        if self.last_rows < 2 || self.last_cols == 0 {
            return None;
        }
        if self.bar_has_keys() {
            let segments: Vec<&str> = tabs::BAR_HINT.split(" · ").collect();
            return Some(agents::fit_keys(&segments, self.last_cols));
        }
        if self.key_sheet_scroll.is_some() {
            // Measured against the rows the sheet is about to get - the pane less the tab bar
            // and this row - since the footer is settled before the sheet is drawn, and in the
            // screen rows its entries wrap to at this width, which is what it scrolls by.
            let scrolls = self.key_sheet_display(self.last_cols).len() > self.last_rows - 1;
            let segments: &[&str] = match scrolls {
                true => &["Esc/? close", "j/k scroll"],
                false => &["Esc/? close"],
            };
            return Some(agents::fit_keys(segments, self.last_cols));
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
                // Only once agent has said anything. Before its first strip there may be no
                // conversation for a Tab to yield to, and a hint for a key that may land
                // nowhere is worse than none; the first strip redraws, so the hint arrives
                // with it. The key is the same either way.
                if self.strip_seen {
                    keys.push("Tab bar".to_owned());
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
            if self.cursor_event().is_some() {
                keys.push("y copy".to_owned());
            }
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
            keys.extend(owned(&["PgUp/PgDn scroll", "Tab bar"]));
        } else if self.last_history_rows > 0 {
            keys.push("Tab history".to_owned());
        } else {
            keys.push("Tab bar".to_owned());
        }
        (keys, keep)
    }

    /// What the key sheet lists: every key whatever has the keys now answers, as
    /// `(keys, what they do)`, most useful first.
    fn key_sheet(&self) -> Vec<(String, String)> {
        // The panel's own tab bar is the panel's, whichever tab it is on.
        if self.bar_has_keys() {
            return sheet(tabs::BAR_KEYS);
        }
        self.agents_key_sheet()
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
                ("Tab", "to the panel tab bar"),
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
                ("y", "copy the whole event"),
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
        // Not before agent's first strip, for the reason the empty list's footer leaves out
        // `Tab bar`: there may be no conversation yet for the key to reach. The tab bar is
        // always there, so Shift+Tab stays.
        if !(empty && !self.strip_seen) {
            rows.push(match (history, self.last_history_rows > 0) {
                (true, _) => ("Tab", "to the panel tab bar"),
                (false, true) => ("Tab", "into the history"),
                (false, false) => ("Tab", "to the panel tab bar"),
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
            // Nothing has said the list is empty yet: agent may still be loading, or not be
            // there at all, and "no agents" would claim something nobody has reported.
            ("waiting for agent…", None)
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

    /// Whether a key typed now is text for a box rather than a command - which is when `?` is
    /// a question mark and not the key sheet.
    ///
    /// The todo box counts even under the tab bar: the bar hands a key it does not own to the
    /// surface, so `?` typed there would have reached the draft. Asked of the mode rather than
    /// of where the caret would go, which a pane too narrow to draw the box in has nowhere to
    /// put - and a box too narrow to see is still the box the next key is typed into.
    fn typing(&self) -> bool {
        self.search_editing
    }

    /// The Agents search box as it is drawn in `cols` columns, and the column its caret is in.
    ///
    /// Scrolled sideways rather than cut from the tail, with the prompt pinned: a box that
    /// loses its end loses the character being typed, and the caret with it.
    fn search_box(&self, cols: usize) -> (String, usize) {
        let prompt = "Search: ";
        let cursor = self.search_query.chars().count();
        yinz_tile::ui_components::forms::viewport(prompt, &self.search_query, cursor, cols)
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
        if self.tabs_focused {
            return self.dispatch_key(key);
        }
        // Cycle the whole panel one corner clockwise - the same gesture the conversation has.
        // Taken before the content handlers, because where the keys are inside the panel changes
        // nothing about what it means: not a box being typed in, not the
        // reader, not an armed question - which stays armed and on screen, since moving the
        // panel answers nothing it asked - and not the key sheet, which stays up. agent owns
        // both overlays' geometry, so this asks rather than moves. Nothing to redraw: the move
        // is the server's to paint.
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            self.reposition();
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
    /// armed question stays armed under it: the sheet
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
        if self.tabs_focused {
            if key.bare_key == BareKey::Tab && key.has_modifiers(&[KeyModifier::Shift]) {
                self.keys_to_the_list();
                if let Some(id) = self.pane_tabs {
                    let _ = focus_pane_tabs(id, false);
                }
                self.take_ring_stop(Some(wire::STOP_HISTORY));
            } else {
                if key.has_no_modifiers()
                    && matches!(key.bare_key, BareKey::Enter | BareKey::Tab | BareKey::Esc)
                {
                    self.keys_to_the_list();
                }
                tabs::forward_bar_key(key);
            }
            return true;
        }
        if self.search_editing {
            return self.search_key(key);
        }
        let noted = self.open_note.take().is_some();
        self.agents_key(key) || noted
    }

    /// A key on the Agents surface.
    fn agents_key(&mut self, key: KeyWithModifier) -> bool {
        if self.reading.is_some() && key.bare_key == BareKey::Tab {
            if key.has_modifiers(&[KeyModifier::Shift]) {
                self.tabs_focused = true;
            } else {
                self.focus_panel_bar(wire::STOP_TABS);
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
            // Tab cycles through the tab bar, list and history; Shift+Tab reverses it.
            // The history half is a stop only when there is one. `last_history_rows` is zero
            // for an empty list and for a pane too short to split, and `render` forces the
            // focus back to `List` in both - so the stop does not merely look empty, it is not
            // there, and forward from the list goes straight on to the panel tab bar. That also
            // makes the vanishing-stop case self-healing: a pane resized short while the keys
            // were in the history half puts them back in the list on the next frame.
            let back = key.has_modifiers(&[KeyModifier::Shift]);
            match (self.focus, back) {
                (Focus::List, false) if self.last_history_rows > 0 => {
                    self.focus = Focus::History;
                    return true;
                },
                (Focus::List, false) => {
                    self.focus_panel_bar(wire::STOP_TABS);
                    return false;
                },
                (Focus::History, true) => {
                    self.focus = Focus::List;
                    return true;
                },
                // Off the end of this pane, either way. Nothing to redraw: the keyboard is
                // going to the other half, and agent paints that.
                (Focus::History, false) => {
                    self.focus_panel_bar(wire::STOP_TABS);
                    return false;
                },
                (Focus::List, true) => {
                    self.tabs_focused = true;
                    return true;
                },
            }
        }
        // The arrows follow the focus, the way `j`/`k` do and the way they do on Todos: in the
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
                BareKey::Char('y') if self.focus == Focus::History => return self.copy_the_event(),
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
                    return self.ask_for_older_runs();
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

    /// Load the next archive page and report whether its rows need a redraw.
    fn ask_for_older_runs(&mut self) -> bool {
        #[cfg(test)]
        {
            self.archive_requests += 1;
        }
        self.load_more_agent_runs();
        self.refresh_strip()
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
    /// modal hides the panel directly before handing focus over. Hiding the panel before opening the modal,
    /// as `activate` does before a focus, turned that Esc into a trip to whatever was under
    /// the panel.
    fn act_on_the_agent(&mut self, fork: bool) -> bool {
        let Some(StripRow::Agent { pane_id, .. }) = self.rows.get(self.selected) else {
            return false;
        };
        let configuration = pane_action_configuration(*pane_id, fork);
        #[cfg(test)]
        self.new_agent_launches.push(configuration.clone());
        load_new_plugin("new-agent", configuration, false, false);
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
        #[cfg(not(test))]
        {
            let result = self.agent_predecessors(request);
            self.apply_predecessors(result);
        }
        #[cfg(test)]
        let _ = request;
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
            // Already out: agent answers a search whole and in order, so a second one for the
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
            let _ = payload;
            self.search_error = None;
            #[cfg(not(test))]
            {
                let result = self.agent_search_result(request, true);
                self.apply_search_result(result);
            }
            #[cfg(test)]
            let _ = request;
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

    /// Give up on a search agent has not answered in `SEARCH_TIMEOUT_MS`, and say whether the
    /// box changed. The query stays, so `Enter` asks again; a reply that turns up late is
    /// ignored, the way any reply is once nothing is pending.
    fn expire_search(&mut self, now: i64) -> bool {
        if !self.search_editing || !self.search_pending || now < self.search_deadline_ms {
            return false;
        }
        self.search_pending = false;
        self.search_error = Some("agent did not answer".to_owned());
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
        if self.tabs_focused {
            return false;
        }
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
        if sheet_up {
            return true;
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
                Mouse::LeftClick(line, column)
            },
            // The drag and the release that follow a press are shifted with it, so a surface
            // sees all three in one set of coordinates and can tell a click - press and release
            // on one cell - from a drag that only selects text.
            Mouse::Hold(line, column) => Mouse::Hold(line, column),
            Mouse::Release(line, column) => Mouse::Release(line, column),
            other => other,
        };
        self.content_mouse(mouse_event, keys_were_here) || left_bar
    }

    /// A mouse event on the surface under the tab bar, in its own coordinates - content row `0`
    /// is the pane's row `1`. `keys_were_here` is whether the keys were in the surface's content
    /// before the event, which a click that lands on what is already chosen reads to tell a
    /// second click from the one that brought the keys back.
    fn content_mouse(&mut self, mouse_event: Mouse, keys_were_here: bool) -> bool {
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
            .map(|(line, _)| (line as usize).saturating_sub(self.last_label_rows))
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
    /// which is the same division of labour that has agent computing the rows drawn here.
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

    /// Name the archived run under the cursor to agent, which knows what resuming one means.
    ///
    /// Nothing is expected back except `"agent_resume_result"`. A refusal is drawn on the
    /// row; a resume that worked puts the panel away, since agent has by then walked the
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
        let _ = payload;
        #[cfg(not(test))]
        self.resume_run(&request.run_id);
    }

    /// Put the four-tab panel away, preserving its selection and drafts.
    ///
    /// Everything that dismisses comes through here: `Esc`, which is the whole of what it does,
    /// and everything that follows it with a focus somewhere the panel would cover - `Enter` on
    /// an agent, `o` on a file, and a resume that worked. Not `D` and `F`, whose modal is drawn
    /// over the panel and hands the keys back to it when it is cancelled.
    fn hide_the_panel(&mut self) {
        self.end_visit();
        // Ended just now, so the manifest that reports the pane suppressed is not a second end.
        self.pane_shown = false;
        #[cfg(test)]
        {
            self.panel_hides += 1;
        }
        hide_pane_with_id(
            self.panel
                .pane_id
                .unwrap_or(PaneId::Plugin(self.own_plugin_id)),
        );
    }

    /// The panel has been put away, by whichever half: this pane's side of it.
    ///
    /// The list half's counterpart of agent resetting its own ring stop on a hide. The keyboard
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
    /// Run when this child is hidden, whether by the host tab switch, the manifest reporting
    /// suppression, or `hide_the_panel`. Drafts and selections survive each of these paths.
    fn end_visit(&mut self) {
        self.tabs_focused = false;
        self.bar_focused.clear();
        self.focus = Focus::List;
        self.click_press = None;
        self.confirming_resume = None;
        // The key sheet is about the moment it was asked for, like the questions are.
        self.key_sheet_scroll = None;
    }

    /// Whether the panel is shown and this Agents child holds the keyboard.
    /// Whether that is worth a redraw.
    ///
    /// The shown-to-hidden edge ends the visit (`end_visit`); nothing else resets a stop. In
    /// particular the keyboard leaving does not - it would race `AGENT_STRIP_TAKE`, which can
    /// arrive before the manifest reporting the focus change, and a yield into the panel tab bar
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
    /// `Tab`, `Enter` and `Esc` on the bar do, and what a click on a tab does.
    fn keys_to_the_list(&mut self) {
        self.tabs_focused = false;
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
        self.focus = Focus::List;
    }

    /// Where the terminal cursor goes this frame, in pane coordinates: in the box that takes
    /// the next key, or nowhere. `rows` and `cols` are the surface's share of the pane - under
    /// the tab bar, above any footer - which is what each surface lays its box out in.
    ///
    /// One answer per frame because the host has one cursor per pane, and before any drawing
    /// because a host command has to be: whichever surface is up asks its own box, in the
    /// order `on_key` hands the keys out. Nowhere while another pane has the keyboard, or with
    /// the key sheet drawn over the box.
    fn text_cursor(&self, rows: usize, cols: usize) -> Option<(usize, usize)> {
        if self.has_keyboard == Some(false)
            || self.key_sheet_scroll.is_some()
            || !self.search_editing
            || self.reading.is_some()
            || rows == 0
            || cols == 0
        {
            None
        } else {
            Some((self.search_box(cols).1, 0))
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
    /// focus is the one that lands. agent hides on its own message a moment later, by which
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
    /// draws. Wrapped rather than cut, so a reason agent gives for a history being unavailable
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
                    self.reader_blocks = Some((
                        reading.id,
                        agents::entry_blocks(reading.kind, &reading.text).into(),
                    ));
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
    /// The half is redrawn on every keystroke and on every strip agent pushes, about once a
    /// second, and rendering it is now a markdown parse plus a style per displayed character -
    /// which `agent/benches/transcript.rs` calls by far the most expensive thing a render does,
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
    /// A loaded run first - the whole point of asking agent for one - then the idle summary
    /// agent pushes with every strip, then nothing. Empty is a real answer here rather than a
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
    /// otherwise - a selected history event, a prose event, or the source `v` put up - the
    /// whole event exactly as it arrived. So `v` then `y` copies a folded event whole, not
    /// our reading of it. `y` says which of the two it took (`copy_the_event`).
    fn copy_target(&self) -> Option<String> {
        let Some(reading) = self.reading.as_ref() else {
            return self.cursor_event().map(|event| event.text.to_string());
        };
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
        self.run_offset_secs =
            get_local_utc_offset(yinz_tile::prelude::SignedUnixMillis::new(run.started_at_ms))
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
    /// agent had sent it.
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

    fn focus_panel_bar(&mut self, _stop: &'static str) {
        #[cfg(test)]
        self.ring_yields.push(_stop);
        self.take_ring_stop(Some(wire::STOP_TABS));
    }

    /// `AGENT_STRIP_TAKE`: the ring arrived at this view, on `stop`.
    fn take_ring_stop(&mut self, stop: Option<&str>) -> bool {
        let tabs_focused =
            stop == Some(agent_proto::tabs::STOP_TABS) || stop == Some(wire::STOP_TABS);
        let tabs_changed = self.tabs_focused != tabs_focused;
        self.tabs_focused = tabs_focused;
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
        if tabs_focused {
            self.bar_focused.push(self.own_client_id);
        }
        if let Some(id) = self.pane_tabs {
            let _ = focus_pane_tabs(id, tabs_focused);
        }
        let focus = match stop {
            Some(wire::STOP_HISTORY) if self.last_history_rows > 0 => Focus::History,
            Some(wire::STOP_TABS) | Some(agent_proto::tabs::STOP_TABS) => self.focus,
            _ => Focus::List,
        };
        let changed = tabs_changed || self.focus != focus;
        self.focus = focus;
        changed
    }

    fn send_selection(&mut self) {
        self.load_selected_history();
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

/// The active child carries the panel's geometry and visibility; Agents may be parked.
fn panel_pane(manifest: &PaneManifest, own_plugin_id: PluginId) -> Option<&PaneInfo> {
    let panes = || manifest.panes.values().flatten();
    let active = panes().find_map(|pane| {
        pane.pane_tabs
            .as_ref()
            .filter(|tabs| {
                tabs.tabs
                    .iter()
                    .any(|slot| slot.plugin_id == Some(own_plugin_id))
            })
            .map(|tabs| tabs.pane_id)
    });
    match active {
        Some(id) => panes().find(|pane| pane.id == id),
        None => own_pane(manifest, own_plugin_id),
    }
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
mod unit;
