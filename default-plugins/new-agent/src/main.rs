use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use agent_proto::location::Location;
use agent_proto::{
    agents::{self, AgentPane},
    launch,
    projects::{self, Registry},
    store::{self, Backend},
    wire::{self, PollReply},
};
use yinz_tile::prelude::*;
use yinz_tile::ui_components::forms::{
    viewport_window, Button, Checkbox, Combo, Dialog, Form, FormEvent, Rect, Select, TextInput,
};

const PROJECT: &str = "project";
const AGENT: &str = "agent";
const MODEL: &str = "model";
const EFFORT: &str = "effort";
const PERMISSION_MODE: &str = "permission_mode";
const AGENT_DEFAULT: &str = "Agent default";
const PROMPT: &str = "prompt";
const INITIAL_PROMPT: &str = "initial_prompt";
const CREATE_BRANCH: &str = "create_branch";
const BASE: &str = "base";
const BRANCH: &str = "branch";
const WORKTREE: &str = "worktree";
const PATH: &str = "path";
const REUSE: &str = "reuse";
/// Which question a `RunCommandResult` is answering, and which round it belongs
/// to. Both travel in the command's context, which comes back verbatim.
const GIT_PROBE: &str = "git_probe";
const GIT_PROBE_ID: &str = "git_probe_id";
const BRANCHES_PROBE: &str = "branches";
const WORKTREES_PROBE: &str = "worktrees";
const DUPLICATE_FOCUSED: &str = "duplicate_focused";
const FORK_FOCUSED: &str = "fork_focused";
const CALLER_TERMINAL_PANE_ID: &str = "caller_terminal_pane_id";

/// Neither action can use a pane whose agent the session does not know.
const NOT_AN_AGENT: &str = "the focused pane is not an agent";

/// Reported when an action has no terminal pane to use.
const NO_PANE_TO_DUPLICATE: &str = "duplicate needs a focused agent pane";
const NO_PANE_TO_FORK: &str = "fork needs a focused agent pane";

/// The project option that stands for "wherever the caller was", which is where
/// this modal created agents before it knew about projects at all.
///
/// Always the first option, whether or not a project owns the caller's cwd. A
/// project means its root, and a caller is very often somewhere else inside it -
/// a linked worktree, a vendored repository below it - so "here" has to stay
/// sayable, or the only way to start an agent in the folder you are looking at
/// would be to have no project at all.
///
/// Everything resolves it by *position*, never by this text: `projects::validate`
/// accepts any non-empty name without control characters, so a real project can
/// be called this, and a name lookup would hand it the sentinel's meaning.
const CURRENT_DIRECTORY: &str = "(current directory)";

/// Top and bottom border of the pane frame, which the plugin does not draw and
/// is not given rows for.
const FRAME_ROWS: usize = 2;

/// What this modal was opened to do.
///
/// Both jobs resolve the source from the daemon snapshot or an explicit lookup for
/// a hidden pane. Only `Fork` stays up once it has one, to ask its question.
enum Job {
    /// Fill the form in and create an agent. What the modal is without a caller pane.
    New,
    /// Start the focused pane's own agent again beside it, no questions asked.
    Duplicate { pane_id: u32 },
    /// The focused pane's context, a different agent: ask which, then start it.
    Fork { pane_id: u32, source: ForkSource },
}

/// What the snapshot said about the pane being forked.
enum ForkSource {
    /// No snapshot yet. `agent-service` may not have started; its `AGENT_HELLO` is what
    /// gets this subscribed when it does.
    Waiting,
    /// Boxed because it is much the largest thing a `Job` can hold and only one job at a
    /// time has it.
    Ready(Box<ForkContext>),
    /// A pane the snapshot has no agent for. There is nothing to fork, and the modal says
    /// so rather than starting an agent somewhere the user did not ask for.
    NotAnAgent,
}

/// Where the fork will run, and what is running there now.
struct ForkContext {
    pane: AgentPane,
    /// The kind `classify` gave the pane: what the picker defaults *away* from.
    kind: String,
}

struct State {
    duplicate: bool,
    dialog: Dialog,
    /// The caller's cwd: the fallback source directory, and what the default
    /// project is derived from.
    source_directory: PathBuf,
    projects: Registry,
    plugin_id: yinz_tile::prelude::PluginId,
    /// The panel on this modal's tab, retained until a manifest includes the modal again.
    panel_pane: Option<PaneId>,
    job: Job,
    worktree_dir_edited: bool,
    error: Option<String>,
    /// A standing remark for the status row: what the fork modal is forking, or why a
    /// fork is showing the New agent form instead. Outlived by the error,
    /// which takes the same row when there is one.
    note: Option<String>,
    /// The `(width, height, y)` `resize` last asked the host for, so it can decline to ask again.
    ///
    /// `TabUpdate` arrives for reasons that have nothing to do with this modal's size - including
    /// every step of a mouse drag of the modal itself - and each coordinate change re-resolves the
    /// pane. Sending only when the size actually changed keeps the drag quiet.
    requested_size: Option<(String, usize, usize)>,
    /// Which round of Git probes the suggestions on screen belong to.
    ///
    /// The project can change while a probe is in flight, and the answer that
    /// comes back is about the folder it was asked in - branches of the project
    /// the user has just left. Bumped by every refresh, so a late reply can be
    /// told apart from the current one and dropped.
    git_probe: u32,
}

impl Default for State {
    fn default() -> Self {
        let form = Form::new()
            // First, because it decides what every field below it means: the
            // base branch, the worktree path and the agent's cwd are all read
            // against this project's root.
            //
            // The one option here is a placeholder `load` replaces. Building it
            // empty is what keeps `Default` free of host calls, which is what
            // lets the tests construct a `State`.
            .select(PROJECT, "Project", Select::new([CURRENT_DIRECTORY]))
            // Built from the shared list rather than written out, so the agents
            // this offers and the ones `agent_spawn` advertises stay the same set.
            .select(
                AGENT,
                "Agent",
                Select::new(launch::offered().iter().map(|kind| capitalized(kind))),
            )
            .field(MODEL, "Model (optional)", TextInput::empty())
            .select(
                EFFORT,
                "Reasoning effort",
                Select::new(agent_options(launch::effort_levels(Some(
                    launch::OFFERED[0],
                )))),
            )
            .select(
                PERMISSION_MODE,
                "Approval mode",
                Select::new(agent_options(permission_modes(launch::OFFERED[0]))),
            )
            // What the agent is started with. The same argument `agent_spawn`
            // takes, so asking agent for an agent and filling this in are the
            // same request said two ways.
            .field(PROMPT, "Prompt (optional)", TextInput::empty())
            .checkbox(
                CREATE_BRANCH,
                "Create and switch branch",
                Checkbox::new(true),
            )
            // The three Git fields are combos rather than plain inputs: each one
            // is free text - a branch nobody has made yet is the common case -
            // that is very often one of the values already in the repository.
            // Their suggestions are empty here and filled by
            // `refresh_git_suggestions`, for the same reason the project select
            // starts on a placeholder: `Default` makes no host calls.
            .combo(BASE, "Base", Combo::empty().with_text(launch::DEFAULT_BASE))
            .combo(BRANCH, "Branch", Combo::empty())
            .checkbox(WORKTREE, "Use worktree", Checkbox::new(true))
            .combo(
                PATH,
                // Where the worktree is *placed*, never what it is called: a worktree
                // is named after its branch, here and everywhere else. Named "from
                // repo root" because a relative path is anchored to the repository's
                // primary checkout, not to the pane's cwd — which differs whenever
                // the agent asking sits in a worktree already.
                "Worktree directory (from repo root)",
                Combo::empty().with_text(launch::WORKTREE_PREFIX),
            )
            // Off, because joining work somebody else has open is a thing to ask for.
            // Unticked, a branch name already taken is cut afresh as `<branch>-2`.
            .checkbox(
                REUSE,
                "Reuse existing branch or worktree",
                Checkbox::new(false),
            )
            .button("create", Button::new("Create agent"));
        // Replaced every render by `help_for`, which says what Enter does in
        // whichever field has focus. Set here too so the dialog reserves the
        // row from the start.
        let help = help_for(&form);
        Self {
            duplicate: false,
            dialog: Dialog::new(Text::new("New agent").color_all(2), form)
                .help(Text::new(help))
                .status_row(),
            source_directory: PathBuf::from("."),
            projects: Registry::new(),
            plugin_id: yinz_tile::prelude::PluginId::new(0),
            panel_pane: None,
            job: Job::New,
            worktree_dir_edited: false,
            error: None,
            note: None,
            requested_size: None,
            git_probe: 0,
        }
    }
}

register_plugin!(State);

impl YinzPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        yinz_tile::shim::init_agent_providers();
        self.refresh_agent_options();
        self.source_directory = configuration
            .get("caller_cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.plugin_id = get_plugin_ids().plugin_id;
        let asked_for = |key: &str| configuration.get(key).map(String::as_str) == Some("true");
        self.duplicate = asked_for(DUPLICATE_FOCUSED);
        let caller_pane_id = configuration
            .get(CALLER_TERMINAL_PANE_ID)
            .and_then(|pane_id| pane_id.parse().ok());
        let (job, note) = requested_job(
            caller_pane_id,
            asked_for(DUPLICATE_FOCUSED),
            asked_for(FORK_FOCUSED),
        );
        self.job = job;
        self.note = note.map(str::to_owned);
        if self.duplicate {
            hide_self();
            if let Some(note) = note {
                notify(note);
                self.finish_picker(None);
                return;
            }
        } else {
            self.load_projects();
            self.fill_projects();
            self.fill_initial_prompt(&configuration);
        }
        if matches!(self.job, Job::Fork { .. }) {
            // Before `resize`, which sizes the overlay to whichever dialog is up.
            self.dialog = fork_dialog();
        }
        if !matches!(self.job, Job::New) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
        }
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::TabUpdate,
            EventType::PastedText,
            EventType::RunCommandResult,
            EventType::Visible,
            EventType::PaneUpdate,
        ]);
        if matches!(self.job, Job::New) {
            self.refresh_git_suggestions();
        }
        set_selectable(!self.duplicate);
        rename_plugin_pane(self.plugin_id, self.pane_title());
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::AgentProvidersChanged(_) => {
                self.refresh_agent_options();
                true
            },
            Event::PaneUpdate(manifest) => {
                if let Some(tab) = tab_of(&manifest, PaneId::Plugin(self.plugin_id)) {
                    self.panel_pane = panel_in_tab(&manifest, tab);
                }
                false
            },
            Event::TabUpdate(tabs) => {
                if let Some(tab) = tabs.iter().find(|tab| tab.active).or_else(|| tabs.first()) {
                    self.resize(tab.display_area_columns.get(), tab.viewport_rows.get());
                }
                false
            },
            Event::Key(key) => {
                let event = self.dialog.handle_key(key);
                self.handle(event)
            },
            Event::Mouse(mouse) => {
                let event = self.dialog.handle_mouse(mouse);
                self.handle(event)
            },
            Event::PastedText(text) => match self.dialog.form_mut().insert_pasted_text(&text) {
                Some(id) => {
                    self.changed(&id);
                    true
                },
                None => false,
            },
            Event::RunCommandResult(exit_code, stdout, _stderr, context) => {
                self.finish_git_probe(exit_code, &String::from_utf8_lossy(&stdout), &context)
            },
            Event::Visible(false) if !self.duplicate => {
                self.finish_picker(None);
                false
            },
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if self.duplicate {
            return;
        }
        let help = help_for(self.dialog.form());
        self.dialog.set_help(Some(Text::new(help)));
        let status = self.status(cols);
        self.dialog.set_status(status);
        self.dialog.render(Rect::new(0, 0, cols, rows));
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_HELLO {
            if !matches!(self.job, Job::New) {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            }
            return false;
        }
        if pipe_message.name != wire::AGENT_SNAPSHOT {
            return false;
        }
        let Some(reply) = pipe_message
            .payload
            .as_deref()
            .and_then(|payload| serde_json::from_str::<PollReply>(payload).ok())
        else {
            return false;
        };
        let pane_of = |pane_id| source_pane(&reply, pane_id, get_pane_info);
        // Taken, so a second snapshot cannot open a second pane or move a picker the user
        // is already looking at.
        match std::mem::replace(&mut self.job, Job::New) {
            Job::New => false,
            Job::Duplicate { pane_id } => {
                let command =
                    pane_of(pane_id).and_then(|pane| duplicate_command(&pane, &reply.manual_kinds));
                let Some(command) = command else {
                    notify(NOT_AN_AGENT);
                    self.finish_picker(None);
                    return false;
                };
                match start(command) {
                    Ok(new_pane_id) => {
                        self.finish_launch(new_pane_id);
                        false
                    },
                    Err(error) => {
                        notify(error);
                        self.finish_picker(None);
                        false
                    },
                }
            },
            Job::Fork {
                pane_id,
                source: ForkSource::Waiting,
            } => {
                let source = match pane_of(pane_id).and_then(|pane| {
                    let kind = agents::classify(&pane, &reply.manual_kinds)?.0.to_owned();
                    Some((pane, kind))
                }) {
                    Some((pane, kind)) => ForkSource::Ready(Box::new(ForkContext { pane, kind })),
                    None => ForkSource::NotAnAgent,
                };
                self.describe_fork(&source);
                self.job = Job::Fork { pane_id, source };
                true
            },
            // A fork that already has its answer; the picker is the user's now.
            answered => {
                self.job = answered;
                false
            },
        }
    }
}

/// The help line for whatever has focus.
///
/// Enter means something different in every widget here, so one static line
/// cannot describe it. Every arm has to fit inside the narrowest modal's frame -
/// see `the_help_line_fits_the_narrowest_modal` - which is why the button and the
/// checkbox name `Tab` alone: `↓` moves on from them too, but the two keys and
/// the rest of the line are two columns wider than the frame leaves.
fn help_for(form: &Form) -> String {
    if form.focused_id() == Some(PERMISSION_MODE)
        && form
            .select_by_id(PERMISSION_MODE)
            .is_some_and(|select| !select.is_open())
    {
        if let Some(description) = yinz_tile::agent::provider::provider(
            &form.text(AGENT).to_lowercase(),
        )
        .and_then(|provider| {
            provider
                .launch
                .permission_descriptions
                .get(form.text(PERMISSION_MODE))
                .cloned()
        }) {
            return description;
        }
    }
    builtin_help_for(form).to_owned()
}

fn builtin_help_for(form: &Form) -> &'static str {
    let Some(field) = form.focused_id().and_then(|id| form.field_by_id(id)) else {
        // No field of that id: focus is on the button, or the form is empty.
        return "<Enter> create  <Tab> next  <Esc> back";
    };
    if let Some(select) = field.select() {
        if field.id() == PERMISSION_MODE && !select.is_open() {
            return match (form.text(AGENT), form.text(PERMISSION_MODE)) {
                ("Codex", "skip") => "Bypasses approvals AND sandbox",
                ("Claude", "skip" | "bypassPermissions") => "Bypasses all permission checks",
                ("Cursor", "skip" | "force") => "Allows commands without approval",
                ("Codex", "never") => "No prompts; keeps sandbox setting",
                ("Codex", "full-auto") => "Automatic mode with workspace sandbox",
                ("Codex", "untrusted") => "Asks before any untrusted command",
                ("Codex", "on-request") => "Agent decides when to ask for approval",
                ("Codex", "on-failure") => "Asks after a sandboxed command fails",
                ("Claude", "acceptEdits") => "Auto-approves file edits",
                ("Claude", "dontAsk") => "Denies actions that need approval",
                ("Claude", "plan") => "Plans without making changes",
                ("Claude", "auto") => "Automatic permission approval",
                ("Claude", "manual") => "Requests manual permission approvals",
                _ => "Uses the agent's permission settings",
            };
        }
        return if select.is_open() {
            "<Enter> choose  <↑↓> move  <Esc> close"
        } else {
            "<Enter> open  <↓/Tab> next  <Esc> back"
        };
    }
    if let Some(combo) = field.combo() {
        // Three states, not the select's two: a combo is a text field as well
        // as a list, and Enter means something different in each of them.
        return if combo.is_open() {
            "<↑↓> pick  <Enter> use  <Esc> close"
        } else if form.is_editing() {
            "<Enter> done  <↓> list  <Tab> next"
        } else {
            "<Enter> edit  <↓/Tab> next  <Esc> back"
        };
    }
    if field.checkbox().is_some() {
        return "<Enter> toggle  <Tab> next  <Esc> back";
    }
    if field.radio().is_some() {
        return if form.is_radio_active() {
            "<Arrows> choose  <Enter/Esc> done"
        } else {
            "<Enter> activate  <↓/Tab> next"
        };
    }
    if form.is_editing() {
        "<Enter> done  <Tab> next  <Esc> back"
    } else {
        "<Enter> edit  <↓/Tab> next  <Esc> back"
    }
}

/// What the alias asked this modal to do, and what the status row says when it cannot.
///
/// Without a caller pane there is nothing to duplicate or fork. The duplicate action
/// reports the note and closes; a fork leaves it on the New agent form. Duplicate wins
/// when both are requested.
fn requested_job(caller: Option<u32>, duplicate: bool, fork: bool) -> (Job, Option<&'static str>) {
    match (caller, duplicate, fork) {
        (Some(pane_id), true, _) => (Job::Duplicate { pane_id }, None),
        (Some(pane_id), false, true) => (
            Job::Fork {
                pane_id,
                source: ForkSource::Waiting,
            },
            None,
        ),
        (None, true, _) => (Job::New, Some(NO_PANE_TO_DUPLICATE)),
        (None, false, true) => (Job::New, Some(NO_PANE_TO_FORK)),
        (_, false, false) => (Job::New, None),
    }
}

/// The status row's account of a prompt the field cannot show: how much of it there is.
///
/// A count rather than a quote, because at its narrowest the row is 38 columns and a
/// count is what still means something there. Always short enough for that row - see
/// `the_prompt_summary_fits_the_narrowest_modal`.
fn prompt_summary(text: &str) -> String {
    let plural = |count: usize, noun: &str| match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    };
    format!(
        "prompt: {}, {}",
        plural(text.lines().count(), "line"),
        plural(text.chars().count(), "char")
    )
}

/// Whether the prompt field, in a pane `cols` wide, is keeping any of its prompt out of
/// sight: text scrolled off either end, or line breaks the row draws as spaces.
///
/// The field is one row that scrolls sideways, draws a newline as a space and opens on
/// the first column, so a todo's prompt - paragraphs of it - shows as its opening words
/// with nothing to say the rest exists.
///
/// Asks `viewport_window`, which is what draws the row, so the cursor's own scroll counts
/// too. The width it is handed mirrors the layout rather than reading it, because that is
/// not exposed: `Dialog::render_buffer` insets the form two columns each side, and a text
/// field draws `"<label>: "` before its value. A test draws the dialog to hold the two to
/// the same answer (`the_prompt_summary_appears_when_the_row_cuts_the_prompt`).
fn prompt_is_cut(form: &Form, cols: usize) -> bool {
    let Some(field) = form.field_by_id(PROMPT) else {
        return false;
    };
    let Some(input) = field.input() else {
        return false;
    };
    let text = input.get_text();
    if text.lines().count() > 1 {
        return true;
    }
    let inset = if cols > 4 { 2 } else { 0 };
    let label = format!("{}: ", field.label().content());
    let (line, _, start) = viewport_window(
        &label,
        text,
        input.get_cursor_position(),
        cols.saturating_sub(inset * 2),
    );
    start > 0 || line.chars().count() < label.chars().count() + text.chars().count()
}

/// The Fork agent modal: one question, and the button that answers it.
///
/// Deliberately not the New agent form with most of it hidden. A fork takes its project,
/// its branch and its folder from the pane it forks - there is nothing to fill in but the
/// agent, and a form showing the rest would be offering to change what this action does
/// not change.
fn fork_dialog() -> Dialog {
    let form = Form::new()
        // The same shared list the New agent modal offers, so the two cannot come to
        // disagree about which agents you may ask for.
        .select(
            AGENT,
            "Agent",
            Select::new(launch::offered().iter().map(|kind| capitalized(kind))),
        )
        .button("fork", Button::new("Fork agent"));
    let help = help_for(&form);
    Dialog::new(Text::new("Fork agent").color_all(2), form)
        .help(Text::new(help))
        .status_row()
}

/// The agent a fork opens on: the first offered kind that is not the one already running,
/// or nothing when there is no other to offer.
///
/// `OFFERED` leads with `codex, claude`, so the two agents a person is likeliest to be
/// swapping between default to each other, and anything else - the other offered agents, or
/// a kind `classify` knows and the picker does not - opens on the first.
fn default_fork_kind(current: &str) -> Option<&'static str> {
    launch::OFFERED
        .iter()
        .copied()
        .find(|kind| *kind != current)
}

/// The status line under the picker: what is being forked, and where the fork will land.
///
/// The folder's name rather than its path, and for the reason the row is clipped from the
/// right (`Dialog::render`): a modal is at most 60 columns, most cwds are longer than that,
/// and the half that would survive the clip is the half that says nothing. A host is named
/// because a fork onto another machine is the case worth being sure about.
fn fork_note(pane: &AgentPane, kind: &str) -> String {
    let cwd = agent_cwd(pane).unwrap_or_default();
    let folder = Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_owned());
    match (pane.host.as_deref(), folder.is_empty()) {
        (Some(host), false) => format!("forking {kind} in {folder} on {host}"),
        (Some(host), true) => format!("forking {kind} on {host}"),
        (None, false) => format!("forking {kind} in {folder}"),
        (None, true) => format!("forking {kind}"),
    }
}

fn modal_width(display_columns: usize) -> String {
    display_columns.div_ceil(2).min(60).to_string()
}

/// Rows of pane the dialog needs: its own rows plus the two the pane frame
/// takes. Derived rather than written down so that adding a field - or the
/// error row - cannot quietly start scrolling the form.
fn modal_height(dialog: &Dialog) -> usize {
    dialog.height() + FRAME_ROWS
}

impl State {
    fn resize(&mut self, display_columns: usize, available_rows: usize) {
        if self.duplicate {
            return;
        }
        // Floating geometry clips at the bottom instead of moving a tall dialog up.
        let height = modal_height(&self.dialog).min(available_rows);
        let y = (available_rows / 5).min(available_rows.saturating_sub(height));
        let size = (modal_width(display_columns), height, y);
        if self.requested_size.as_ref() == Some(&size) {
            // nothing about the modal's size changed, so there is nothing to ask for. A
            // `TabUpdate` is not evidence that this pane wants to be a different shape.
            return;
        }
        let (width, height, y) = size.clone();
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            None,
            Some(y.to_string()),
            Some(width),
            Some(height.to_string()),
            None,
            None,
        ) {
            // This API resizes either a floating pane or an overlay where it already is.
            // For overlays it preserves the kind and any position the user dragged it to.
            change_floating_panes_coordinates(vec![(PaneId::Plugin(self.plugin_id), coordinates)]);
            self.requested_size = Some(size);
        }
    }
}

impl State {
    /// Take the project registry as it stands.
    ///
    /// A plain fetch rather than a watch: this modal lives for as long as it
    /// takes to fill a form in, and a registry that changes underneath it has
    /// nothing to correct - the user is already looking at the choice they
    /// meant. A failure leaves the registry empty, which offers the caller's
    /// own folder and is exactly what this modal did before.
    fn load_projects(&mut self) {
        let backend = store::Host::shared();
        // Declared before reading, because a fetch against an entity nobody has
        // declared is an error and any of the agent plugins may start first.
        // The definitions come from one place so no two can disagree.
        if backend.define_all(&store::shared_defs()).is_err() {
            return;
        }
        if let Ok(rows) =
            backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
        {
            self.projects = projects::from_rows(&rows);
        }
    }

    /// Put the registry into the select, opened on the active project.
    fn fill_projects(&mut self) {
        let (options, selected) = project_options(&self.projects, &self.source_directory);
        if let Some(select) = self.dialog.form_mut().select_by_id_mut(PROJECT) {
            select.set_options(options);
            // By index, because the options are not unique by text: see
            // `CURRENT_DIRECTORY`.
            select.select_index(selected);
        }
        self.fill_worktree_default();
    }

    /// Ask Git what this project already has, for the three combos to suggest.
    ///
    /// Asynchronous, through `RunCommandResult`, because a plugin has no way to
    /// run a process and wait: see `finish_git_probe` for the other half. Run in
    /// the same folder the submit path resolves - the suggestions have to be
    /// about the repository the agent would be created in, not the one the modal
    /// was opened from.
    ///
    /// Nothing here can fail loudly. A folder that is not a checkout answers
    /// non-zero and simply has nothing to offer; the fields are free text and
    /// stay usable with no list at all.
    fn refresh_git_suggestions(&mut self) {
        let cwd = source_directory(self.dialog.form(), &self.projects, &self.source_directory);
        self.git_probe += 1;
        let context = |kind: &str| {
            BTreeMap::from([
                (GIT_PROBE.to_owned(), kind.to_owned()),
                (GIT_PROBE_ID.to_owned(), self.git_probe.to_string()),
            ])
        };
        run_command_with_env_variables_and_cwd(
            &[
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                // Most recent first: the branch a base is taken from is nearly
                // always one of the last few touched, and the combo filters by
                // typing anyway.
                "--sort=-committerdate",
                "--count=200",
                "refs/heads",
            ],
            BTreeMap::new(),
            cwd.clone(),
            context(BRANCHES_PROBE),
        );
        run_command_with_env_variables_and_cwd(
            &["git", "worktree", "list", "--porcelain"],
            BTreeMap::new(),
            cwd,
            context(WORKTREES_PROBE),
        );
    }

    /// File one probe's answer into the combos it belongs to.
    fn finish_git_probe(
        &mut self,
        exit_code: Option<i32>,
        stdout: &str,
        context: &BTreeMap<String, String>,
    ) -> bool {
        let Some(kind) = context.get(GIT_PROBE) else {
            return false;
        };
        // A reply from a round before the current one is about another project's
        // folder, and would offer its branches for this one.
        if context.get(GIT_PROBE_ID).map(String::as_str) != Some(&self.git_probe.to_string()) {
            return false;
        }
        if exit_code.unwrap_or(0) != 0 {
            return false;
        }
        match kind.as_str() {
            BRANCHES_PROBE => {
                let branches = branch_suggestions(stdout);
                // Both, and for the same reason: a base is always an existing
                // branch, and a branch is one whenever the agent is meant to
                // carry on work that is already there.
                for id in [BASE, BRANCH] {
                    if let Some(combo) = self.dialog.form_mut().combo_by_id_mut(id) {
                        combo.set_suggestions(branches.clone());
                    }
                }
                true
            },
            WORKTREES_PROBE => {
                if !self.worktree_dir_edited
                    && selected_project(self.dialog.form(), &self.projects).is_none()
                {
                    if let Some(primary) = stdout
                        .lines()
                        .find_map(|line| line.strip_prefix("worktree "))
                    {
                        let dir = projects::for_cwd(
                            &self.projects,
                            &Location::local(self.source_directory.to_string_lossy()),
                        )
                        .or_else(|| projects::for_cwd(&self.projects, &Location::local(primary)))
                        .and_then(|project| project.worktree_dir.clone());
                        if let Some(dir) = dir {
                            self.set_text(PATH, &dir);
                        }
                    }
                }
                let paths = worktree_suggestions(stdout);
                if let Some(combo) = self.dialog.form_mut().combo_by_id_mut(PATH) {
                    combo.set_suggestions(paths);
                }
                true
            },
            _ => false,
        }
    }
}

/// One branch per line, as `for-each-ref` prints them.
fn branch_suggestions(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The directories this repository's linked worktrees already live in, written the
/// way the field is labelled.
///
/// The field names where a worktree is *placed*, never the worktree itself - that
/// is always the branch - so what is on offer is each existing worktree's parent,
/// deduplicated, and not the worktrees themselves.
///
/// `git worktree list --porcelain` opens each block with a `worktree <path>` line
/// and lists the primary checkout first. That first entry is dropped: it is the
/// repository itself, which is what "use worktree" unticked already means.
///
/// The rest are relativised against it, because the field is anchored to the
/// primary checkout ("from repo root") - and left absolute when they live
/// outside it, which is where a relative path could not reach them.
fn worktree_suggestions(porcelain: &str) -> Vec<String> {
    let mut paths = porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let Some(primary) = paths.next() else {
        return vec![];
    };
    let mut directories: Vec<String> = vec![];
    for path in paths {
        let directory = Path::new(path)
            .parent()
            .unwrap_or_else(|| Path::new(path))
            .to_path_buf();
        let written = directory
            .strip_prefix(primary)
            .map(|relative| relative.to_string_lossy().into_owned())
            .unwrap_or_else(|_| directory.to_string_lossy().into_owned());
        // A worktree sitting directly in the primary checkout relativises to
        // nothing, and an empty suggestion is not one.
        if written.is_empty() || directories.contains(&written) {
            continue;
        }
        directories.push(written);
    }
    directories
}

/// What the project select offers, and the index of the one that starts
/// selected.
///
/// `CURRENT_DIRECTORY` first, then every project; an index rather than a name,
/// because the sentinel is positional. The list is built from `values()` rather
/// than `keys()` so it is visibly the same order `source_directory` indexes back
/// into - both are sorted by name, a `Registry` being a `BTreeMap`.
///
/// The default is the deepest project whose root contains the caller's cwd, and
/// that is *containment only*. the Agents panel resolves its active project differently
/// (`projects::project_for_cwd`, default-plugins/agents/src/service.rs): it first requires
/// the folder to be a Git checkout, then prefers `projects::for_repository` - the
/// remote, or the common directory - and falls back to `for_cwd` only after that.
/// It can, because the daemon probes Git for it; this modal has no probe of its
/// own and no rows to read one out of, so it cannot follow. Two differences come
/// of that, both survivable because the first option is always there:
///
/// - a linked worktree living *outside* its project's root matches nothing here
///   and defaults to `CURRENT_DIRECTORY`, where Agents would recognise the project
///   by its remote;
/// - a folder that is not a checkout but sits below a project root the user named
///   defaults to that project, where Agents would report no project. Should that
///   root not be a checkout either, the host answers "git could not find the
///   repository root" - and the caller's own folder, which may well be a
///   repository, is one option away.
fn project_options(projects: &Registry, caller_cwd: &Path) -> (Vec<String>, usize) {
    // The caller is a pane in this session asking about its own folder, which
    // is on this machine by construction.
    let selected = projects::for_cwd(projects, &Location::local(caller_cwd.to_string_lossy()))
        .and_then(|active| {
            projects
                .values()
                .position(|project| project.name == active.name)
        })
        // Past the sentinel, which is what index 0 is.
        .map_or(0, |position| position + 1);
    let options = std::iter::once(CURRENT_DIRECTORY.to_owned())
        .chain(projects.values().map(|project| project.name.clone()))
        .collect();
    (options, selected)
}

/// Where the agent is created: the selected project's root, or - at option 0 -
/// the caller's own folder.
///
/// Everything downstream hangs off this - `prepare_git_checkout` resolves the
/// repository from it, so the base branch, the new branch and a relative
/// worktree path all belong to whichever project is selected here. An absolute
/// root reaches the host untouched; only `/host`, `/data`, `/cache`, `/tmp` and
/// relative paths are translated.
///
/// Read by index, so a project called `(current directory)` is that project and
/// not the sentinel.
fn source_directory(form: &Form, projects: &Registry, caller_cwd: &Path) -> PathBuf {
    selected_project(form, projects)
        .map(|project| PathBuf::from(&project.root))
        .unwrap_or_else(|| caller_cwd.to_path_buf())
}

fn selected_project<'a>(form: &Form, projects: &'a Registry) -> Option<&'a projects::Project> {
    form.select_by_id(PROJECT)
        .and_then(|select| select.selected_index())
        // Option 0 is the sentinel; each later option is the project at that
        // offset in the registry's own order, which is the order the options
        // were built in.
        .and_then(|index| index.checked_sub(1))
        .and_then(|nth| projects.values().nth(nth))
}

/// Visible snapshots omit a source hidden behind its exit hook. Look up only the
/// requested terminal, preserving the normal list's filtering and detection latch.
fn source_pane(
    reply: &PollReply,
    pane_id: u32,
    lookup: impl FnOnce(PaneId) -> Option<PaneInfo>,
) -> Option<AgentPane> {
    reply
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.pane_id == pane_id)
        .cloned()
        .or_else(|| {
            lookup(PaneId::Terminal(TerminalId::new(pane_id)))
                .map(|pane| agent_proto::panes::agent_pane(&pane, None))
        })
}

fn duplicate_command(
    pane: &AgentPane,
    manual_kinds: &BTreeMap<u32, String>,
) -> Option<CommandToRun> {
    let (kind, _) = agents::classify(pane, manual_kinds)?;
    command_in_context(pane, &kind).ok()
}

fn agent_cwd(pane: &AgentPane) -> Option<&str> {
    pane.agent
        .as_ref()
        .and_then(|agent| agent.cwd.as_deref())
        .filter(|cwd| !cwd.is_empty())
        .or(pane.cwd.as_deref())
}

/// The command that starts `kind` where `pane` is running: same machine, same
/// folder, and nothing of the old argv - a duplicate and a fork both start a fresh
/// conversation, they only differ over which agent has it.
fn command_in_context(pane: &AgentPane, kind: &str) -> Result<CommandToRun, String> {
    // An exited process loses its live cwd, but its agent record retains it.
    let target = match pane.bubblewrap.as_deref() {
        // The sandbox is part of where the pane runs, as much as its machine is.
        Some(root) => PaneTarget::bubblewrap(std::path::Path::new(root))?,
        None => PaneTarget::parse_display(pane.host.as_deref()),
    };
    launch::command_in_context(kind, agent_cwd(pane).map(Into::into), target)
}

fn start(command: CommandToRun) -> Result<PaneId, &'static str> {
    let context = BTreeMap::from([
        (AGENT_PANE_CONTEXT.to_owned(), "true".to_owned()),
        ("agent_agent_host_cwd".to_owned(), "true".to_owned()),
    ]);
    open_command_pane_near_plugin(command, context).ok_or("could not open a pane")
}

impl State {
    fn finish_picker(&mut self, pane: Option<PaneId>) {
        self.job = Job::New;
        if let Some(pane) = pane {
            hand_over_focus(pane, self.panel_pane);
        } else {
            close_self();
        }
    }

    fn finish_launch(&mut self, pane: PaneId) {
        self.finish_picker(Some(pane));
    }

    fn handle(&mut self, event: FormEvent) -> bool {
        if self.duplicate {
            return false;
        }
        match event {
            FormEvent::ButtonPressed(_) => {
                if matches!(self.job, Job::Fork { .. }) {
                    self.fork()
                } else {
                    self.submit()
                }
            },
            FormEvent::Cancel => {
                self.finish_picker(None);
                false
            },
            FormEvent::FieldChanged(id) => {
                self.changed(&id);
                true
            },
            other => other.should_render(),
        }
    }

    fn refresh_agent_options(&mut self) {
        let selected = self.dialog.form().text(AGENT).to_owned();
        if let Some(select) = self.dialog.form_mut().select_by_id_mut(AGENT) {
            select.set_options(
                launch::offered()
                    .iter()
                    .map(|kind| capitalized(kind))
                    .collect(),
            );
        }
        self.refresh_launch_options(self.dialog.form().text(AGENT) != selected);
    }

    fn refresh_launch_options(&mut self, reset: bool) {
        let kind = self.dialog.form().text(AGENT).to_lowercase();
        for (field, options) in [
            (EFFORT, agent_options(launch::effort_options(&kind))),
            (PERMISSION_MODE, agent_options(permission_modes(&kind))),
        ] {
            if let Some(select) = self.dialog.form_mut().select_by_id_mut(field) {
                select.set_options(options);
                if reset {
                    select.select_index(0);
                }
            }
        }
    }

    fn changed(&mut self, id: &str) {
        self.error = None;
        // A fork with nothing to fork stays refused. The message is about the pane the
        // modal was opened from, not about anything the user has just done to the picker,
        // so clearing it here would hide a standing refusal until the button was pressed.
        if matches!(
            self.job,
            Job::Fork {
                source: ForkSource::NotAnAgent,
                ..
            }
        ) {
            self.error = Some(NOT_AN_AGENT.to_owned());
        }
        // The branches and worktrees on offer are the selected project's, so a
        // different project means a different list.
        if id == PATH {
            self.worktree_dir_edited = true;
        }
        if id == AGENT {
            // Shared option names need not promise the same behavior across agents.
            self.refresh_launch_options(true);
        }

        if id == PROJECT {
            self.fill_worktree_default();
            self.refresh_git_suggestions();
        }
        if id == CREATE_BRANCH && !self.dialog.form().checked(CREATE_BRANCH) {
            self.set_checked(WORKTREE, false);
        }
        if id == WORKTREE && self.dialog.form().checked(WORKTREE) {
            self.set_checked(CREATE_BRANCH, true);
        }
    }

    fn set_text(&mut self, id: &str, text: &str) {
        if let Some(input) = self
            .dialog
            .form_mut()
            .field_by_id_mut(id)
            .and_then(|field| field.input_mut())
        {
            input.set_text(text.to_owned());
        }
    }

    fn fill_initial_prompt(&mut self, configuration: &BTreeMap<String, String>) {
        if let Some(prompt) = configuration.get(INITIAL_PROMPT) {
            if let Some(input) = self
                .dialog
                .form_mut()
                .field_by_id_mut(PROMPT)
                .and_then(|field| field.input_mut())
            {
                input.set_text(prompt.clone());
                input.set_cursor_position(0);
            }
        }
    }

    fn set_checked(&mut self, id: &str, checked: bool) {
        if let Some(checkbox) = self.dialog.form_mut().checkbox_by_id_mut(id) {
            checkbox.set_checked(checked);
        }
    }

    fn fill_worktree_default(&mut self) {
        self.worktree_dir_edited = false;
        let dir = selected_project(self.dialog.form(), &self.projects)
            .or_else(|| {
                projects::for_cwd(
                    &self.projects,
                    &Location::local(self.source_directory.to_string_lossy()),
                )
            })
            .and_then(|project| project.worktree_dir.clone())
            .unwrap_or_else(|| launch::WORKTREE_PREFIX.to_owned());
        self.set_text(PATH, &dir);
    }

    /// What the status row says in a pane `cols` wide.
    ///
    /// The error wins the row: it is the thing the user has to act on. The note says what
    /// is being forked, or why this is the New agent form, and can wait for the error to
    /// be cleared. Field guidance and then the prompt's size come last, worked out every render
    /// rather than kept as a note, so an edit or a resize is reflected at once and a
    /// prompt that comes to fit takes it away.
    fn status(&self, cols: usize) -> Option<Text> {
        match (&self.error, &self.note) {
            (Some(error), _) => Some(Text::new(error).color_all(3)),
            (None, Some(note)) => Some(Text::new(note)),
            (None, None) => {
                let form = self.dialog.form();
                prompt_is_cut(form, cols).then(|| Text::new(prompt_summary(form.text(PROMPT))))
            },
        }
    }

    fn pane_title(&self) -> &'static str {
        match self.job {
            Job::Fork { .. } => "fork agent",
            _ => "new agent",
        }
    }

    /// Point the picker at an agent other than the one running, and say what is being
    /// forked - or say that there is nothing to fork.
    fn describe_fork(&mut self, source: &ForkSource) {
        match source {
            // The snapshot has not answered yet; the picker keeps its opening option and
            // the row stays empty.
            ForkSource::Waiting => {},
            ForkSource::Ready(context) => {
                if let Some(kind) = default_fork_kind(&context.kind) {
                    let option = capitalized(kind);
                    if let Some(select) = self.dialog.form_mut().select_by_id_mut(AGENT) {
                        select.select_option(&option);
                    }
                }
                self.note = Some(fork_note(&context.pane, &context.kind));
            },
            ForkSource::NotAnAgent => self.error = Some(NOT_AN_AGENT.to_owned()),
        }
    }

    /// Start the chosen agent where the forked pane is running.
    fn fork(&mut self) -> bool {
        // Read before the job is borrowed. The select's labels are the shared kinds,
        // capitalised; `binary_for` wants them as they are written in `OFFERED`.
        let kind = self.dialog.form().text(AGENT).to_lowercase();
        let Job::Fork { source, .. } = &self.job else {
            return false;
        };
        let context = match source {
            // The daemon has not answered. Saying so beats starting an agent in whatever
            // folder this modal happens to have been opened from.
            ForkSource::Waiting => {
                self.error = Some("still reading the focused pane".to_owned());
                return true;
            },
            ForkSource::NotAnAgent => {
                self.error = Some(NOT_AN_AGENT.to_owned());
                return true;
            },
            ForkSource::Ready(context) => context,
        };
        match command_in_context(&context.pane, &kind)
            .and_then(|command| start(command).map_err(str::to_owned))
        {
            Ok(pane_id) => {
                self.finish_launch(pane_id);
                false
            },
            Err(error) => {
                self.error = Some(error.to_owned());
                true
            },
        }
    }

    fn submit(&mut self) -> bool {
        // `request` for the one check that is the modal's own; `launch::open` plans
        // it again and does the work, which is the same call `agent_spawn` makes.
        if let Err(error) = request(self.dialog.form()) {
            self.error = Some(error);
            return true;
        }
        let source = source_directory(self.dialog.form(), &self.projects, &self.source_directory);
        let mut spec = spec(self.dialog.form());
        // The primary-checkout probe may still be in flight. Let the host resolve
        // the current directory's saved default unless the user edited it.
        if !self.worktree_dir_edited
            && selected_project(self.dialog.form(), &self.projects).is_none()
        {
            spec.worktree_dir = None;
        }
        match launch::open(&spec, source) {
            Ok(agent) => {
                self.finish_launch(agent.pane_id);
                false
            },
            Err(error) => {
                self.error = Some(error);
                true
            },
        }
    }
}

/// Close the modal onto the pane it just created, keyboard and all.
///
/// Neither half of this happens on its own. A pane opened near a plugin is
/// anchored to a pane rather than to a client, so the host has no client to
/// focus it for and does not try (`ScreenInstruction::NewPane`, the
/// `ClientTabIndexOrPaneId::PaneId` arm). And a modal overlay is never robbed
/// of focus by a pane opening behind it, because it has no way to learn it lost
/// it and would sit there taking no input while the user typed underneath. Both
/// are right; what neither can do is decide that *this* submit is over. Only
/// the modal knows that, so handing the keyboard on is its job.
///
/// Focus first, then close. While this modal is up it keeps the keyboard whatever
/// the layers under it say, so the focus call moves the tiled focus *underneath* -
/// and it also changes where the keys go when the modal closes. A closing modal
/// hands them back to the pane it took them from, which is all a cancel
/// (`close_self` alone) wants. A focus call made while the modal holds them
/// repoints that at the lower layer, so the close gives the keys to the pane just
/// focused rather than to whatever the user came from, an overlay included. The
/// other order has nothing left to focus from: the close would already have handed
/// the keys back, and this plugin is gone by then.
///
/// Hide the panel on the screen queue before focusing the terminal and closing the modal.
/// An asynchronous broadcast hide can outlive the modal and undo a later panel reveal.
fn hand_over_focus(pane_id: PaneId, panel: Option<PaneId>) {
    if let Some(panel) = panel {
        hide_pane_with_id(panel);
    }
    focus_pane_with_id(pane_id, false, false);
    close_self();
}

fn panel_in_tab(manifest: &PaneManifest, tab: TabPosition) -> Option<PaneId> {
    manifest.panes.get(&tab)?.iter().find_map(|pane| {
        pane.pane_tabs
            .as_ref()
            .filter(|tabs| {
                tabs.tabs
                    .iter()
                    .any(|slot| slot.descriptor.location == "yinz:agents")
            })
            .map(|tabs| tabs.pane_id)
            .or_else(|| (pane.plugin_url.as_deref() == Some("yinz:agents")).then_some(pane.id))
    })
}

/// The tab `me` is on in `manifest`, or `None` when the manifest does not list it.
fn tab_of(manifest: &PaneManifest, me: PaneId) -> Option<TabPosition> {
    manifest
        .panes
        .iter()
        .find_map(|(tab, panes)| panes.iter().any(|pane| pane.id == me).then_some(*tab))
}

/// What the form is asking for, in the terms agent asks for the same thing.
///
/// Reading only — every default and every rule about what pairs with what lives in
/// `launch`, which is also what `agent_spawn` calls. The checkbox gets a valid
/// placeholder here; the host replaces it after Git resolves the checkout.
fn spec(form: &Form) -> launch::Spec {
    let branch = form
        .checked(CREATE_BRANCH)
        .then(|| form.text(BRANCH).to_owned());
    launch::Spec {
        // The select's labels are the shared kinds, capitalised; `launch` lowercases.
        kind: Some(form.text(AGENT).to_owned()),
        command: None,
        args: Vec::new(),
        model: Some(form.text(MODEL).to_owned()),
        effort: selected_agent_option(form, EFFORT),
        permission_mode: selected_agent_option(form, PERMISSION_MODE),
        prompt: Some(form.text(PROMPT).to_owned()),

        // No branch is how `launch` is told to leave the repository alone, which is
        // exactly what an unchecked box means.
        branch,
        base: Some(form.text(BASE).to_owned()),
        // Only read while a branch is being created: the checkbox stays checked
        // underneath an unchecked `create_branch`, and reporting it there would ask
        // `launch` for a worktree with nothing to put in it.
        worktree: form.checked(CREATE_BRANCH).then(|| form.checked(WORKTREE)),
        // Only alongside a worktree, for the same reason: `launch` refuses a directory
        // to put a worktree in on a spec that will not cut one.
        worktree_dir: (form.checked(CREATE_BRANCH) && form.checked(WORKTREE))
            .then(|| form.text(PATH).to_owned()),
        reuse: Some(form.checked(REUSE)),
        // The modal's exemption from the guard `agent_spawn` and `yinz.agent.spawn` are
        // held to. Unticking "Use worktree" is a person looking straight at the box and
        // saying so, which is the whole of what the override asks a caller to do; making
        // them tick a second box beside it would say nothing the first did not.
        allow_outside_worktree: Some(!form.checked(CREATE_BRANCH) || !form.checked(WORKTREE)),
        ..Default::default()
    }
}

fn permission_modes(kind: &str) -> Vec<String> {
    launch::permission_modes(kind)
}

fn agent_options(values: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    std::iter::once(AGENT_DEFAULT.to_owned())
        .chain(values.into_iter().map(|value| value.as_ref().to_owned()))
        .collect()
}

fn selected_agent_option(form: &Form, id: &str) -> Option<String> {
    form.select_by_id(id)
        .filter(|select| select.selected_index().is_some_and(|index| index > 0))
        .and_then(|select| select.selected_option())
        .map(str::to_owned)
}

/// The form as the pair Yinz wants. Kept as its own function because the tests
/// below check the translation, and because it is the one place the modal could
/// still say something `launch` cannot.
fn request(form: &Form) -> Result<(CommandToRun, GitCheckout), String> {
    // `launch` defaults an omitted base and an omitted worktree path, because a tool
    // call that leaves a key out means "the usual one". A field the user cleared by
    // hand means something else — they are looking straight at it — so the modal
    // refuses rather than filling it back in behind them. That difference is only
    // knowable here, which is why these three checks did not move.
    if form.checked(CREATE_BRANCH) {
        if form.text(BRANCH).trim().is_empty() || form.text(BASE).trim().is_empty() {
            return Err("branch and base are required".to_owned());
        }
        if form.checked(WORKTREE) && form.text(PATH).trim().is_empty() {
            return Err("worktree directory is required".to_owned());
        }
    }
    launch::plan(&spec(form))
}

/// A shared kind as the select shows it. Presentation, so it stays here.
fn capitalized(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "unit/provider_refresh.rs"]
mod provider_refresh;

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::projects::Project;

    fn project(name: &str, root: &str) -> (String, Project) {
        (
            name.to_owned(),
            Project {
                name: name.to_owned(),
                root: root.to_owned(),

                worktree_dir: None,
                remote: None,
                worktree_status: None,
                common_dir: None,
                discovered: false,
                replaced_ids: Vec::new(),
            },
        )
    }

    /// A local agent pane, as the daemon reports one.
    fn agent_pane(kind: &str) -> AgentPane {
        AgentPane {
            pane_id: 7,
            title: format!("{kind} agent"),
            command: Some(kind.to_owned()),
            cwd: Some("/work/yinz".to_owned()),
            host: None,
            bubblewrap: None,
            status: agents::AgentStatus::Running,
            tab: Some((yinz_tile::prelude::TabId::new(1), "work".to_owned())),
            detected: Some(kind.to_owned()),
            agent: None,
        }
    }

    /// A fork modal whose snapshot has already answered.
    fn forking(kind: &str) -> State {
        let source = ForkSource::Ready(Box::new(ForkContext {
            pane: agent_pane(kind),
            kind: kind.to_owned(),
        }));
        let mut state = State {
            dialog: fork_dialog(),
            ..Default::default()
        };
        state.describe_fork(&source);
        state.job = Job::Fork { pane_id: 7, source };
        state
    }

    fn registry() -> Registry {
        Registry::from([
            project("yinz", "/work/yinz"),
            project("dobbs", "/work/dobbs"),
        ])
    }

    fn register_opencode_fixture() {
        yinz_tile::agent::provider::register(0, serde_json::from_value(serde_json::json!({
            "kind":"opencode", "executable":"opencode", "launch": {
                "model_args":["--model","{value}"], "prompt_args":["--prompt","{value}"],
                "permissions":{"auto":["--auto"],"skip":["--auto"]},
                "permission_descriptions":{"auto":"Auto-approves; explicit denials stay","skip":"Auto-approves; explicit denials stay"}
            }
        })).unwrap()).unwrap();
    }

    #[test]
    fn modal_is_half_the_display_up_to_sixty_columns() {
        assert_eq!(modal_width(80), "40");
        assert_eq!(modal_width(200), "60");
    }

    #[test]
    fn new_and_fork_wait_for_display_dimensions_before_requesting_geometry() {
        for columns in [80, 200] {
            for dialog in [State::default().dialog, fork_dialog()] {
                let mut state = State {
                    dialog,
                    ..Default::default()
                };
                state.update(Event::TabUpdate(vec![]));
                assert_eq!(state.requested_size, None);
                let tab = TabInfo {
                    display_area_columns: ColumnCount::new(columns),
                    viewport_rows: RowCount::new(40),
                    ..Default::default()
                };
                state.update(Event::TabUpdate(vec![tab.clone()]));
                let expected = Some((modal_width(columns), modal_height(&state.dialog), 8));
                assert_eq!(state.requested_size, expected);
                state.update(Event::TabUpdate(vec![tab]));
                assert_eq!(state.requested_size, expected);
            }
        }
    }

    #[test]
    fn the_modal_is_tall_enough_for_the_whole_dialog() {
        let state = State::default();
        assert_eq!(modal_height(&state.dialog), 20);
        assert_eq!(state.dialog.height() + FRAME_ROWS, 20);
    }

    #[test]
    fn the_modal_moves_up_to_keep_its_footer_inside_short_viewports() {
        let mut state = State::default();
        for (rows, y) in [(40, 8), (24, 4), (20, 0), (18, 0), (20, 0), (40, 8)] {
            state.resize(80, rows);
            assert_eq!(state.requested_size, Some(("40".into(), rows.min(20), y)));
            assert!(y + 20.min(rows) <= rows);
        }
    }

    #[test]
    fn project_worktree_defaults_reset_on_selection_and_allow_overrides() {
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/work/yinz"),
            ..Default::default()
        };
        state.projects.get_mut("yinz").unwrap().worktree_dir = Some("../yinz trees".into());
        state.projects.get_mut("dobbs").unwrap().worktree_dir = Some("/tmp/dobbs trees".into());
        state.fill_projects();
        assert_eq!(state.dialog.form().text(PATH), "../yinz trees");
        state.set_text(PATH, "/manual");
        assert_eq!(
            spec(state.dialog.form()).worktree_dir.as_deref(),
            Some("/manual")
        );
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PROJECT)
            .unwrap()
            .select_index(1);
        state.changed(PROJECT);
        assert_eq!(state.dialog.form().text(PATH), "/tmp/dobbs trees");
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PROJECT)
            .unwrap()
            .select_index(0);
        state.changed(PROJECT);
        assert_eq!(state.dialog.form().text(PATH), "../yinz trees");
    }

    #[test]
    fn external_worktree_probe_fills_project_default_without_replacing_manual_input() {
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/outside/linked"),
            ..Default::default()
        };
        state.projects.get_mut("yinz").unwrap().worktree_dir = Some("../saved trees".into());
        state.fill_projects();
        let context = BTreeMap::from([
            (GIT_PROBE.to_owned(), WORKTREES_PROBE.to_owned()),
            (GIT_PROBE_ID.to_owned(), state.git_probe.to_string()),
        ]);
        let output = "worktree /work/yinz\n\nworktree /outside/linked\n";
        assert!(state.finish_git_probe(Some(0), output, &context));
        assert_eq!(state.dialog.form().text(PATH), "../saved trees");
        state.set_text(PATH, "/manual");
        state.changed(PATH);
        state.finish_git_probe(Some(0), output, &context);
        assert_eq!(state.dialog.form().text(PATH), "/manual");
    }

    #[test]
    fn defaults_create_a_codex_worktree_from_main() {
        let mut state = State::default();
        state
            .dialog
            .form_mut()
            .field_by_id_mut(BRANCH)
            .unwrap()
            .input_mut()
            .unwrap()
            .set_text("topic".into());
        state.changed(BRANCH);
        let (command, checkout) = request(state.dialog.form()).unwrap();
        assert_eq!(command.path, PathBuf::from("yinz"));
        assert_eq!(command.args, ["agent", "codex", "--command", "codex", "--"]);
        assert!(
            matches!(checkout, GitCheckout::NewWorktree { branch, base, path }
            if branch == "topic" && base == "main" && path == *".worktrees/topic")
        );
    }

    /// The point of the shared module: the modal's form and agent's `agent_spawn`
    /// arguments are two spellings of one request, and must resolve alike.
    #[test]
    fn the_form_and_a_bare_spec_ask_for_the_same_thing() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        state.set_text(MODEL, "gpt-6-luna");
        state.set_text(PROMPT, "fix the parser");
        let (from_form, checkout) = request(state.dialog.form()).unwrap();
        let (from_spec, spec_checkout) = launch::plan(&launch::Spec {
            kind: Some("codex".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            prompt: Some("fix the parser".to_owned()),
            branch: Some("topic".to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(from_form.path, from_spec.path);
        assert_eq!(from_form.args, from_spec.args);
        assert_eq!(checkout, spec_checkout);
    }

    #[test]
    fn the_prompt_is_handed_to_the_agent_as_it_starts() {
        let mut state = State::default();
        state.set_checked(CREATE_BRANCH, false);
        state.set_text(PROMPT, "fix the parser");
        let (command, _) = request(state.dialog.form()).unwrap();
        assert_eq!(
            command.args,
            [
                "agent",
                "codex",
                "--command",
                "codex",
                "--",
                "fix the parser"
            ]
        );
    }

    #[test]
    fn a_todo_prefills_an_editable_prompt_without_changing_its_text() {
        let mut state = State::default();
        let prompt = "Implement todo #4: fix `parser`\nCheck $(literal) and 日本語.\nRun tests.";
        state.fill_initial_prompt(&BTreeMap::from([(
            INITIAL_PROMPT.to_owned(),
            prompt.to_owned(),
        )]));
        state.set_checked(CREATE_BRANCH, false);
        assert_eq!(state.dialog.form().text(PROMPT), prompt);
        assert_eq!(
            state
                .dialog
                .form()
                .field_by_id(PROMPT)
                .unwrap()
                .input()
                .unwrap()
                .get_cursor_position(),
            0
        );
        assert_eq!(
            request(state.dialog.form()).unwrap().0.args,
            ["agent", "codex", "--command", "codex", "--", prompt]
        );
        state.set_text(PROMPT, "the edited task");
        assert_eq!(
            spec(state.dialog.form()).prompt.as_deref(),
            Some("the edited task")
        );
        assert_eq!(State::default().dialog.form().text(PROMPT), "");
    }

    /// The prompt `n` on a todo fills in: parent context, open subtasks, instructions.
    const TODO_PROMPT: &str = "Todo #2: Handle `quotes` and $(commands)\nKeep each argument intact\n\nParent context (for reference):\n- Improve the build\n\nOpen subtasks:\n- [ ] Cover multiline prompts\n  - [ ] Check newlines\n\nImplement the task, run relevant checks, and summarize the changes and anything still outstanding.";

    /// Columns inside the narrowest modal: `modal_width` less the frame's two.
    fn narrowest_inside() -> usize {
        modal_width(80).parse::<usize>().unwrap() - 2
    }

    /// A prompt opened the way a todo opens it: cursor on the first column.
    fn prefilled(prompt: &str) -> State {
        let mut state = State::default();
        state.fill_initial_prompt(&BTreeMap::from([(
            INITIAL_PROMPT.to_owned(),
            prompt.to_owned(),
        )]));
        state
    }

    fn status(state: &State, cols: usize) -> Option<String> {
        state.status(cols).map(|text| text.content().to_owned())
    }

    #[test]
    fn a_prompt_the_row_cannot_show_is_counted_on_the_status_row() {
        let narrowest = narrowest_inside();
        let state = prefilled(TODO_PROMPT);
        let summary = status(&state, narrowest).unwrap();
        assert_eq!(summary, "prompt: 11 lines, 288 chars");
        assert!(
            summary.chars().count() <= narrowest,
            "{summary:?} does not fit {narrowest} columns"
        );
        // Line breaks are drawn as spaces, so even a prompt narrow enough for the row
        // hides something.
        assert_eq!(
            status(&prefilled("fix\nthe parser"), 60).as_deref(),
            Some("prompt: 2 lines, 14 chars")
        );
    }

    #[test]
    fn the_prompt_summary_fits_the_narrowest_modal() {
        let narrowest = narrowest_inside();
        // Far past any prompt a todo writes, and still inside the row.
        let summary = prompt_summary(&"x\n".repeat(99_990));
        assert_eq!(summary, "prompt: 99990 lines, 199980 chars");
        assert!(summary.chars().count() <= narrowest);
    }

    #[test]
    fn the_prompt_summary_goes_when_the_prompt_is_all_on_screen() {
        let mut state = prefilled(TODO_PROMPT);
        state.set_text(PROMPT, "");
        assert_eq!(status(&state, narrowest_inside()), None);
        let short = prefilled("fix the parser");
        assert_eq!(status(&short, narrowest_inside()), None);
        // And the fork modal, which has no prompt at all, never shows one.
        let fork = State {
            dialog: fork_dialog(),
            ..Default::default()
        };
        assert_eq!(status(&fork, narrowest_inside()), None);
    }

    #[test]
    fn an_error_or_a_note_takes_the_row_over_the_prompt_summary() {
        let mut state = prefilled(TODO_PROMPT);
        state.note = Some(NO_PANE_TO_FORK.to_owned());
        assert_eq!(
            status(&state, narrowest_inside()).as_deref(),
            Some(NO_PANE_TO_FORK)
        );
        state.error = Some("branch and base are required".to_owned());
        assert_eq!(
            status(&state, narrowest_inside()).as_deref(),
            Some("branch and base are required")
        );
        state.error = None;
        state.note = None;
        assert_eq!(
            status(&state, narrowest_inside()).as_deref(),
            Some("prompt: 11 lines, 288 chars")
        );
    }

    /// `prompt_is_cut` works out the field's width rather than being told it, so this
    /// draws the dialog and holds the two to the same answer at the modal's narrowest and
    /// widest: 15 and 35 columns of text beside the label.
    #[test]
    fn the_prompt_summary_appears_when_the_row_cuts_the_prompt() {
        // A drawn row's text is its bytes, comma-separated, after its colour indices.
        fn drawn(state: &mut State, cols: usize, text: &str) -> bool {
            let bytes: Vec<String> = format!("Prompt (optional): {text}")
                .bytes()
                .map(|byte| byte.to_string())
                .collect();
            let rows = state.dialog.height();
            state
                .dialog
                .render_buffer(Rect::new(0, 0, cols, rows))
                .output
                .contains(&format!("${}\u{1b}\\", bytes.join(",")))
        }
        for (cols, field) in [(narrowest_inside(), 15), (58, 35)] {
            // Opened on the first column, the field shows all of a prompt its own width.
            let mut state = prefilled(&"x".repeat(field));
            assert!(drawn(&mut state, cols, &"x".repeat(field)));
            assert_eq!(status(&state, cols), None);
            let mut state = prefilled(&"x".repeat(field + 1));
            assert!(!drawn(&mut state, cols, &"x".repeat(field + 1)));
            assert!(status(&state, cols).is_some());
            // With the cursor after the end, one column goes to the cursor, and the
            // first character scrolls off to make room.
            state.set_text(PROMPT, &"x".repeat(field));
            assert!(!drawn(&mut state, cols, &"x".repeat(field)));
            assert!(status(&state, cols).is_some());
            state.set_text(PROMPT, &"x".repeat(field - 1));
            assert!(drawn(&mut state, cols, &"x".repeat(field - 1)));
            assert_eq!(status(&state, cols), None);
        }
    }

    #[test]
    fn registered_agent_choices_refresh_and_use_provider_launch_options() {
        use yinz_tile::agent::provider::{self, AgentProvider};
        let owner = 901_611;
        let mut state = State::default();
        state.set_checked(CREATE_BRANCH, false);
        let descriptor: AgentProvider = serde_json::from_value(serde_json::json!({
            "kind":"fixture-chooser", "executable":"fixture-chooser-bin", "launch": {
                "model_args":["--engine","{value}"], "prompt_args":["--ask","{value}"],
                "permissions":{"ask":["--approval","ask"]},
                "permission_descriptions":{"ask":"Ask before each tool"},
                "efforts":{"deep":["--think","deep"]}
            }
        }))
        .unwrap();
        provider::register(owner, descriptor).unwrap();
        assert!(state.update(Event::AgentProvidersChanged(provider::providers())));
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .select_option("Fixture-chooser");
        state.changed(AGENT);
        assert_eq!(
            state.dialog.form().select_by_id(EFFORT).unwrap().options(),
            agent_options(["deep"])
        );
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PERMISSION_MODE)
            .unwrap()
            .select_option("ask");
        state.dialog.form_mut().focus_on(PERMISSION_MODE);
        assert_eq!(help_for(state.dialog.form()), "Ask before each tool");
        state.set_text(MODEL, "chosen");
        state.set_text(PROMPT, "work here");
        let (command, _) = launch::plan(&spec(state.dialog.form())).unwrap();
        assert_eq!(command.path, PathBuf::from("fixture-chooser-bin"));
        assert_eq!(
            command.args,
            [
                "--engine",
                "chosen",
                "--approval",
                "ask",
                "--ask",
                "work here"
            ]
        );
        assert!(provider::unregister_owner(owner));
        state.update(Event::AgentProvidersChanged(provider::providers()));
        assert!(!state
            .dialog
            .form()
            .select_by_id(AGENT)
            .unwrap()
            .options()
            .contains(&"Fixture-chooser".to_owned()));
    }

    #[test]
    fn launch_options_follow_the_agent_and_default_to_its_configuration() {
        register_opencode_fixture();
        let mut state = State::default();
        state.set_checked(CREATE_BRANCH, false);
        state.set_text(MODEL, "example");
        for kind in launch::OFFERED {
            state
                .dialog
                .form_mut()
                .select_by_id_mut(AGENT)
                .unwrap()
                .select_option(&capitalized(kind));
            state.changed(AGENT);
            let form = state.dialog.form();
            assert_eq!(spec(form).effort, None);
            assert_eq!(spec(form).permission_mode, None);
            assert_eq!(
                form.select_by_id(EFFORT).unwrap().options(),
                agent_options(launch::effort_levels(Some(kind)))
            );
            assert_eq!(
                form.select_by_id(PERMISSION_MODE).unwrap().options(),
                agent_options(permission_modes(kind))
            );
            for mode in permission_modes(kind) {
                state
                    .dialog
                    .form_mut()
                    .select_by_id_mut(PERMISSION_MODE)
                    .unwrap()
                    .select_option(&mode);
                assert_eq!(
                    spec(state.dialog.form()).permission_mode.as_deref(),
                    Some(mode.as_str())
                );
                assert!(request(state.dialog.form()).is_ok(), "{kind}: {mode}");
            }
        }
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .select_option("Codex");
        state.changed(AGENT);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(EFFORT)
            .unwrap()
            .select_option("xhigh");
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PERMISSION_MODE)
            .unwrap()
            .select_option("full-auto");
        let command = request(state.dialog.form()).unwrap().0;
        assert_eq!(
            command.args,
            [
                "agent",
                "codex",
                "--command",
                "codex",
                "--",
                "-c",
                "model_reasoning_effort=\"xhigh\"",
                "--model",
                "example",
                "--full-auto"
            ]
        );
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .select_option("Opencode");
        state.changed(AGENT);
        assert_eq!(
            spec(state.dialog.form()).effort,
            None,
            "unsupported effort is reset"
        );
        assert_eq!(
            spec(state.dialog.form()).permission_mode,
            None,
            "permission choices never carry across agents"
        );
    }

    /// A field the user cleared is not a field they left alone: `launch` would
    /// default both of these, and the modal deliberately will not.
    #[test]
    fn a_field_emptied_by_hand_is_refused_rather_than_defaulted() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        state.set_text(BASE, "");
        assert_eq!(
            request(state.dialog.form()).unwrap_err(),
            "branch and base are required"
        );
        state.set_text(BASE, "main");
        state.set_text(PATH, "");
        assert_eq!(
            request(state.dialog.form()).unwrap_err(),
            "worktree directory is required"
        );
    }

    /// The modal's exemption from the guard every other caller is held to: unticking
    /// a box is the deliberate act the override asks for, so the spec carries it. With
    /// the box ticked there is a worktree to land in and nothing to excuse.
    #[test]
    fn leaving_the_worktree_box_unticked_carries_the_override_itself() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        assert_eq!(
            spec(state.dialog.form()).allow_outside_worktree,
            Some(false)
        );

        state.set_checked(WORKTREE, false);
        assert_eq!(spec(state.dialog.form()).allow_outside_worktree, Some(true));
        // And a directory to put a worktree in is not reported when none is cut:
        // `launch` refuses that pairing.
        assert_eq!(spec(state.dialog.form()).worktree_dir, None);

        state.set_checked(CREATE_BRANCH, false);
        assert_eq!(spec(state.dialog.form()).allow_outside_worktree, Some(true));
    }

    #[test]
    fn model_is_passed_directly_to_the_selected_agent() {
        let mut state = State::default();
        state.set_checked(CREATE_BRANCH, false);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .select_option("Claude");
        state
            .dialog
            .form_mut()
            .field_by_id_mut(MODEL)
            .unwrap()
            .input_mut()
            .unwrap()
            .set_text("opus".into());
        let (command, checkout) = request(state.dialog.form()).unwrap();
        assert_eq!(command.path, PathBuf::from("env"));
        assert_eq!(
            &command.args[..9],
            [
                "YINZ_CLAUDE_INLINE_HOOKS=1",
                "yinz",
                "agent",
                "claude",
                "--command",
                "claude",
                "--",
                "--model",
                "opus"
            ]
        );
        // A claude pane the modal starts reports its own state, like one agent starts.
        assert_eq!(command.args[9], "--settings");
        assert_eq!(command.args.len(), 11);
        assert_eq!(checkout, GitCheckout::Current);
    }

    /// The bug this dialog had: a stray Enter created a half-configured agent.
    #[test]
    fn enter_in_the_first_field_opens_its_list_instead_of_creating() {
        let mut state = State::default();
        // Not `handle`, which reaches host functions: the point is what the
        // form did with the key, and that it was not a submit.
        let event = state
            .dialog
            .handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(event, FormEvent::FocusMoved);
        // The first field is the project select now; the assertion is about
        // where the Enter went, not about which list it opened.
        assert!(state.dialog.form().select_by_id(PROJECT).unwrap().is_open());
    }

    #[test]
    fn the_help_line_fits_the_narrowest_modal() {
        // `modal_width` is half the display, so 80 columns is the narrowest
        // terminal worth drawing this in - and the line is drawn inside the frame.
        let narrowest = narrowest_inside();
        let mut state = State::default();
        let mut lines = vec![];
        for id in [
            PROJECT,
            AGENT,
            MODEL,
            EFFORT,
            PERMISSION_MODE,
            CREATE_BRANCH,
            BASE,
            "create",
        ] {
            state.dialog.form_mut().focus_on(id);
            lines.push(help_for(state.dialog.form()));
        }
        // The two states a field has beyond being focused.
        state.dialog.form_mut().focus_on(AGENT);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .open();
        lines.push(help_for(state.dialog.form()));
        state.dialog.form_mut().focus_on(MODEL);
        state
            .dialog
            .handle_key(KeyWithModifier::new(BareKey::Enter));
        lines.push(help_for(state.dialog.form()));
        // And the three a combo has: closed, editing, and with its list open.
        state.dialog.form_mut().focus_on(BASE);
        state
            .dialog
            .handle_key(KeyWithModifier::new(BareKey::Enter));
        lines.push(help_for(state.dialog.form()));
        state
            .dialog
            .form_mut()
            .combo_by_id_mut(BASE)
            .unwrap()
            .open();
        lines.push(help_for(state.dialog.form()));

        for line in lines {
            assert!(
                line.chars().count() <= narrowest,
                "{line:?} does not fit {narrowest} columns"
            );
        }
    }

    #[test]
    fn approval_help_explains_bypasses_without_changing_the_dropdown_controls() {
        register_opencode_fixture();
        let mut state = State::default();
        for (agent, mode, expected) in [
            ("Codex", "skip", "Bypasses approvals AND sandbox"),
            ("Codex", "never", "No prompts; keeps sandbox setting"),
            ("Claude", "skip", "Bypasses all permission checks"),
            (
                "Claude",
                "bypassPermissions",
                "Bypasses all permission checks",
            ),
            ("Cursor", "force", "Allows commands without approval"),
            ("Opencode", "skip", "Auto-approves; explicit denials stay"),
        ] {
            state
                .dialog
                .form_mut()
                .select_by_id_mut(AGENT)
                .unwrap()
                .select_option(agent);
            state.changed(AGENT);
            state.dialog.form_mut().focus_on(PERMISSION_MODE);
            state
                .dialog
                .form_mut()
                .select_by_id_mut(PERMISSION_MODE)
                .unwrap()
                .select_option(mode);
            assert_eq!(help_for(state.dialog.form()), expected);
            assert!(expected.chars().count() <= 40);
            state
                .dialog
                .form_mut()
                .select_by_id_mut(PERMISSION_MODE)
                .unwrap()
                .open();
            assert_eq!(
                help_for(state.dialog.form()),
                "<Enter> choose  <↑↓> move  <Esc> close"
            );
            state.dialog.handle_key(KeyWithModifier::new(BareKey::Esc));
        }
    }

    #[test]
    fn the_project_field_is_first_and_defaults_to_the_active_project() {
        // A worktree below the project root is still the project's.
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/work/yinz/.worktrees/topic"),
            ..Default::default()
        };
        state.fill_projects();

        assert_eq!(state.dialog.form().fields()[0].id(), PROJECT);
        let select = state.dialog.form().select_by_id(PROJECT).unwrap();
        assert_eq!(select.options(), [CURRENT_DIRECTORY, "dobbs", "yinz"]);
        assert_eq!(select.selected_option(), Some("yinz"));
    }

    /// The finding this fixes: the caller's own folder had no option at all once
    /// a project owned it, and `git switch -c` for a branch without a worktree
    /// then ran in the project's primary checkout instead of the worktree the
    /// modal was opened from.
    #[test]
    fn the_current_directory_is_offered_from_inside_a_project() {
        let worktree = PathBuf::from("/work/yinz/.worktrees/topic");
        let mut state = State {
            projects: registry(),
            source_directory: worktree.clone(),
            ..Default::default()
        };
        state.fill_projects();

        let select = state.dialog.form_mut().select_by_id_mut(PROJECT).unwrap();
        assert_eq!(select.options()[0], CURRENT_DIRECTORY);
        assert!(select.select_index(0));
        assert_eq!(
            source_directory(
                state.dialog.form(),
                &state.projects,
                &state.source_directory
            ),
            worktree
        );
    }

    /// Nothing stops a project being named exactly like the sentinel, so the
    /// sentinel cannot be a name.
    #[test]
    fn a_project_named_like_the_sentinel_does_not_shadow_it() {
        let mut state = State {
            projects: Registry::from([project(CURRENT_DIRECTORY, "/work/oddly-named")]),
            source_directory: PathBuf::from("/elsewhere"),
            ..Default::default()
        };
        state.fill_projects();

        let select = state.dialog.form_mut().select_by_id_mut(PROJECT).unwrap();
        assert_eq!(select.options(), [CURRENT_DIRECTORY, CURRENT_DIRECTORY]);
        assert_eq!(select.selected_index(), Some(0));
        assert_eq!(
            source_directory(
                state.dialog.form(),
                &state.projects,
                &state.source_directory
            ),
            PathBuf::from("/elsewhere")
        );

        let select = state.dialog.form_mut().select_by_id_mut(PROJECT).unwrap();
        assert!(select.select_index(1));
        assert_eq!(
            source_directory(
                state.dialog.form(),
                &state.projects,
                &state.source_directory
            ),
            PathBuf::from("/work/oddly-named")
        );
    }

    /// The registry is the only source of projects, so a folder outside every
    /// one of them - and a session whose registry is still empty - has to keep
    /// the behaviour this modal had before the field existed.
    #[test]
    fn a_cwd_in_no_project_offers_the_current_directory() {
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/elsewhere"),
            ..Default::default()
        };
        state.fill_projects();

        let select = state.dialog.form().select_by_id(PROJECT).unwrap();
        assert_eq!(select.options(), [CURRENT_DIRECTORY, "dobbs", "yinz"]);
        assert_eq!(select.selected_option(), Some(CURRENT_DIRECTORY));

        let (options, selected) = project_options(&Registry::new(), Path::new("/elsewhere"));
        assert_eq!(options, [CURRENT_DIRECTORY]);
        assert_eq!(selected, 0);
    }

    #[test]
    fn the_agent_is_created_in_the_selected_projects_root() {
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/work/yinz"),
            ..Default::default()
        };
        state.fill_projects();
        // The point of the field: an agent for a project the caller is not in.
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PROJECT)
            .unwrap()
            .select_option("dobbs");
        assert_eq!(
            source_directory(
                state.dialog.form(),
                &state.projects,
                &state.source_directory
            ),
            PathBuf::from("/work/dobbs")
        );

        state.source_directory = PathBuf::from("/elsewhere");
        state.fill_projects();
        assert_eq!(
            source_directory(
                state.dialog.form(),
                &state.projects,
                &state.source_directory
            ),
            PathBuf::from("/elsewhere")
        );
    }

    /// The field says where a worktree is *placed*, so what is offered is the
    /// directories the linked ones already live in, each once. The primary checkout
    /// is not among them - leaving the "use worktree" box unticked is what running
    /// there means - and the rest are written the way the field is labelled.
    #[test]
    fn the_worktree_suggestions_are_the_directories_the_linked_checkouts_live_in() {
        let porcelain = "worktree /work/yinz\nHEAD abcd\nbranch refs/heads/main\n\
             \nworktree /work/yinz/.worktrees/topic\nHEAD ef01\nbranch refs/heads/topic\n\
             \nworktree /work/yinz/.worktrees/other\nHEAD ef02\nbranch refs/heads/other\n\
             \nworktree /elsewhere/detached\nHEAD 2345\ndetached\n";
        assert_eq!(
            worktree_suggestions(porcelain),
            // Two worktrees in one directory are one suggestion, and the one outside
            // the root cannot be written relative to it.
            [".worktrees", "/elsewhere"]
        );
        // A repository with no linked worktrees has nothing to suggest, and a
        // folder that is not a checkout answers with nothing at all.
        assert!(worktree_suggestions("worktree /work/yinz\nbranch refs/heads/main\n").is_empty());
        assert!(worktree_suggestions("").is_empty());
    }

    #[test]
    fn a_finished_probe_fills_the_combos_it_is_about() {
        let mut state = State {
            git_probe: 3,
            ..Default::default()
        };
        let context = |kind: &str, id: u32| {
            BTreeMap::from([
                (GIT_PROBE.to_owned(), kind.to_owned()),
                (GIT_PROBE_ID.to_owned(), id.to_string()),
            ])
        };

        assert!(state.finish_git_probe(Some(0), "topic\nmain\n\n", &context(BRANCHES_PROBE, 3)));
        // A base is an existing branch; so is a branch whenever the agent is
        // meant to carry on work that is already there.
        for id in [BASE, BRANCH] {
            assert_eq!(
                state.dialog.form().combo_by_id(id).unwrap().suggestions(),
                ["topic", "main"],
                "{id}"
            );
        }
        assert!(state.finish_git_probe(
            Some(0),
            "worktree /work/yinz\n\nworktree /work/yinz/.worktrees/topic\n",
            &context(WORKTREES_PROBE, 3),
        ));
        assert_eq!(
            state.dialog.form().combo_by_id(PATH).unwrap().suggestions(),
            [".worktrees"]
        );
        // The text the field started with is the value; a suggestion list never
        // touches it.
        assert_eq!(state.dialog.form().text(BASE), "main");
        assert_eq!(state.dialog.form().text(PATH), ".worktrees/");
    }

    /// A reply about the folder the user has just left would offer another
    /// project's branches for this one, and a probe that failed - a folder that
    /// is not a checkout - has nothing to say either.
    #[test]
    fn a_stale_or_failed_probe_leaves_the_suggestions_alone() {
        let mut state = State {
            git_probe: 2,
            ..Default::default()
        };
        let probe = |id: u32| {
            BTreeMap::from([
                (GIT_PROBE.to_owned(), BRANCHES_PROBE.to_owned()),
                (GIT_PROBE_ID.to_owned(), id.to_string()),
            ])
        };
        assert!(!state.finish_git_probe(Some(0), "old-project-branch\n", &probe(1)));
        assert!(!state.finish_git_probe(Some(128), "\n", &probe(2)));
        assert!(state
            .dialog
            .form()
            .combo_by_id(BASE)
            .unwrap()
            .suggestions()
            .is_empty());
    }

    /// The alternative would be starting an agent in whatever folder the modal was opened
    /// from, which is not what "fork this pane" asked for.
    #[test]
    fn a_pane_that_is_not_an_agent_is_refused_rather_than_forked() {
        let mut state = State {
            dialog: fork_dialog(),
            job: Job::Fork {
                pane_id: 7,
                source: ForkSource::NotAnAgent,
            },
            ..Default::default()
        };
        state.describe_fork(&ForkSource::NotAnAgent);
        assert_eq!(state.error.as_deref(), Some(NOT_AN_AGENT));
        // And it stays said: moving the picker is not an answer to it.
        state.changed(AGENT);
        assert_eq!(state.error.as_deref(), Some(NOT_AN_AGENT));
        // Refused before any host call, which is what makes this testable at all.
        assert!(state.fork());
        assert_eq!(state.error.as_deref(), Some(NOT_AN_AGENT));

        // And the same before the daemon has answered, for the same reason.
        let mut waiting = State {
            dialog: fork_dialog(),
            job: Job::Fork {
                pane_id: 7,
                source: ForkSource::Waiting,
            },
            ..Default::default()
        };
        assert!(waiting.fork());
        assert_eq!(
            waiting.error.as_deref(),
            Some("still reading the focused pane")
        );
    }

    #[test]
    fn the_fork_modal_is_only_as_tall_as_its_one_question() {
        let dialog = fork_dialog();
        assert_eq!(modal_height(&dialog), dialog.height() + FRAME_ROWS);
        // Much shorter than the New agent form, which is the point of a second dialog.
        assert!(dialog.height() < State::default().dialog.height());
    }

    #[test]
    fn the_fork_help_line_fits_the_narrowest_modal() {
        let narrowest: usize = modal_width(80).parse().unwrap();
        let mut state = State {
            dialog: fork_dialog(),
            ..Default::default()
        };
        let mut lines = vec![];
        for id in [AGENT, "fork"] {
            state.dialog.form_mut().focus_on(id);
            lines.push(help_for(state.dialog.form()));
        }
        state.dialog.form_mut().focus_on(AGENT);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(AGENT)
            .unwrap()
            .open();
        lines.push(help_for(state.dialog.form()));

        for line in lines {
            assert!(
                line.chars().count() <= narrowest,
                "{line:?} does not fit {narrowest} columns"
            );
        }
    }

    /// The row is clipped from the right, so a note built from a whole path would say
    /// nothing by the time it was drawn.
    #[test]
    fn the_fork_note_names_the_folder_and_the_machine() {
        let pane = agent_pane("codex");
        assert_eq!(fork_note(&pane, "codex"), "forking codex in yinz");
        assert_eq!(
            fork_note(
                &AgentPane {
                    host: Some("builder".to_owned()),
                    ..pane.clone()
                },
                "codex"
            ),
            "forking codex in yinz on builder"
        );
        // A pane the daemon has no cwd for still says what it is forking.
        assert_eq!(
            fork_note(&AgentPane { cwd: None, ..pane }, "codex"),
            "forking codex"
        );
    }

    #[test]
    fn duplicate_or_fork_with_no_pane_has_no_job_and_says_why() {
        assert!(matches!(
            requested_job(None, true, false),
            (Job::New, Some(NO_PANE_TO_DUPLICATE))
        ));
        assert!(matches!(
            requested_job(None, false, true),
            (Job::New, Some(NO_PANE_TO_FORK))
        ));
        // Duplicate wins a request for both, with or without a pane.
        assert!(matches!(
            requested_job(None, true, true),
            (Job::New, Some(NO_PANE_TO_DUPLICATE))
        ));
        assert!(matches!(
            requested_job(Some(7), true, true),
            (Job::Duplicate { pane_id: 7 }, None)
        ));
        assert!(matches!(
            requested_job(Some(7), false, true),
            (
                Job::Fork {
                    pane_id: 7,
                    source: ForkSource::Waiting
                },
                None
            )
        ));
        // A plain New agent, with or without a pane, has nothing to explain.
        for caller in [None, Some(7)] {
            assert!(matches!(
                requested_job(caller, false, false),
                (Job::New, None)
            ));
        }
    }

    #[test]
    fn the_new_agent_fallback_notes_fit_the_narrowest_modal() {
        let narrowest = narrowest_inside();
        for note in [NO_PANE_TO_DUPLICATE, NO_PANE_TO_FORK, NOT_AN_AGENT] {
            assert!(
                note.chars().count() <= narrowest,
                "{note:?} does not fit {narrowest} columns"
            );
        }
    }

    #[test]
    fn duplicate_stays_hidden_and_finishes_without_a_form_when_the_source_is_not_an_agent() {
        let mut state = State {
            duplicate: true,
            job: Job::Duplicate { pane_id: 7 },
            ..Default::default()
        };
        state.update(Event::Visible(false));
        assert!(
            matches!(state.job, Job::Duplicate { .. }),
            "hiding must not cancel duplication"
        );
        state.resize(120, 40);
        assert!(state.requested_size.is_none());
        assert!(!state.handle(FormEvent::ButtonPressed("create".to_owned())));
        assert!(state.error.is_none());
        let pane = AgentPane {
            title: "shell".to_owned(),
            command: Some("/bin/sh".to_owned()),
            detected: None,
            ..agent_pane("claude")
        };
        assert!(duplicate_command(&pane, &BTreeMap::new()).is_none());
        state.pipe(PipeMessage::new(
            PipeSource::Plugin(PluginId::new(1)),
            wire::AGENT_SNAPSHOT,
            &Some(serde_json::json!({"panes": [pane], "activity": {}}).to_string()),
            &None,
            false,
        ));
        assert!(matches!(state.job, Job::New));
        assert!(state.note.is_none());
        assert!(!state.handle(FormEvent::ButtonPressed("create".to_owned())));
        assert!(state.error.is_none());
    }

    #[test]
    fn launcher_detects_only_standalone_agents_on_its_own_tab() {
        let me = PaneId::Plugin(PluginId::new(12));
        let panel = PaneId::Plugin(PluginId::new(13));
        let tab = TabPosition::new(1);
        for location in ["yinz:agents", "yinz:todos", "yinz:chat"] {
            for panel_tab in [TabPosition::new(0), tab] {
                let mut manifest = PaneManifest::default();
                manifest.panes.insert(
                    tab,
                    vec![PaneInfo {
                        id: me,
                        ..Default::default()
                    }],
                );
                manifest.panes.entry(panel_tab).or_default().push(PaneInfo {
                    id: panel,
                    plugin_url: Some(location.into()),
                    pane_tabs: None,
                    ..Default::default()
                });
                let mut state = State {
                    plugin_id: PluginId::new(12),
                    ..Default::default()
                };
                state.update(Event::PaneUpdate(manifest));
                assert_eq!(
                    state.panel_pane,
                    (location == "yinz:agents" && panel_tab == tab).then_some(panel),
                    "{location} on tab {panel_tab}"
                );
            }
        }
    }

    #[test]
    fn launcher_hides_only_the_panel_on_its_own_tab() {
        let me = PaneId::Plugin(PluginId::new(12));
        let panel = PaneId::Plugin(PluginId::new(13));
        let tab = TabPosition::new(1);
        let mut manifest = PaneManifest {
            panes: std::collections::HashMap::from([
                (
                    tab,
                    vec![PaneInfo {
                        id: me,
                        ..Default::default()
                    }],
                ),
                (
                    TabPosition::new(0),
                    vec![PaneInfo {
                        id: panel,
                        pane_tabs: Some(PaneTabsInfo {
                            id: PaneTabsId(1),
                            pane_id: panel,
                            active: 1,
                            bar_focused: vec![],
                            tabs: ["agents", "todos"]
                                .into_iter()
                                .enumerate()
                                .map(|(index, name)| PaneTabInfo {
                                    descriptor: PaneTabDescriptor {
                                        label: name.into(),
                                        location: format!("yinz:{name}"),
                                        configuration: Default::default(),
                                        initial_cwd: None,
                                    },
                                    plugin_id: Some(PluginId::new(14 + index as u32)),
                                    terminal_id: None,
                                    state: PaneTabLoadState::Ready,
                                })
                                .collect(),
                        }),
                        ..Default::default()
                    }],
                ),
            ]),
        };
        let mut state = State {
            plugin_id: PluginId::new(12),
            ..Default::default()
        };
        assert_eq!(tab_of(&manifest, me), Some(tab));
        state.update(Event::PaneUpdate(manifest.clone()));
        assert_eq!(state.panel_pane, None, "another tab's panel stays open");
        let panels = manifest.panes.remove(&TabPosition::new(0)).unwrap();
        manifest.panes.get_mut(&tab).unwrap().extend(panels);
        state.update(Event::PaneUpdate(manifest));
        assert_eq!(
            state.panel_pane,
            Some(panel),
            "use the container, including when Todos is active"
        );
        state.update(Event::PaneUpdate(PaneManifest::default()));
        assert_eq!(state.panel_pane, Some(panel));
    }

    #[test]
    fn the_fork_default_is_an_agent_other_than_the_one_running() {
        // The two agents a person swaps between default to each other.
        assert_eq!(default_fork_kind("codex"), Some("claude"));
        assert_eq!(default_fork_kind("claude"), Some("codex"));
        // Anything else - the third offered agent, or a kind `classify` knows and the
        // picker does not - opens on the first offered one.
        assert_eq!(default_fork_kind("cursor"), Some("codex"));
        assert_eq!(default_fork_kind("gemini"), Some("codex"));
    }

    #[test]
    fn the_picker_opens_on_the_other_agent_and_still_offers_this_one() {
        let state = forking("codex");

        let select = state.dialog.form().select_by_id(AGENT).unwrap();
        assert_eq!(select.selected_option(), Some("Claude"));
        // Forking onto the same agent is a duplicate, and stays one keypress away.
        assert!(select.options().iter().any(|option| option == "Codex"));
        assert_eq!(state.note.as_deref(), Some("forking codex in yinz"));
        assert!(state.error.is_none());

        assert_eq!(
            forking("claude").dialog.form().text(AGENT),
            "Codex",
            "the default swaps both ways"
        );
    }

    #[test]
    fn forking_keeps_the_context_and_applies_the_new_agents_launch_defaults() {
        let pane = AgentPane {
            host: Some("builder".to_owned()),
            cwd: Some("/tmp/repo".to_owned()),
            ..agent_pane("codex")
        };
        let command = command_in_context(&pane, "claude").unwrap();
        assert_eq!(command.path, PathBuf::from("env"));
        assert!(
            command.args
                == [
                    "YINZ_CLAUDE_INLINE_HOOKS=0",
                    "yinz",
                    "agent",
                    "claude",
                    "--command",
                    "claude",
                    "--"
                ],
            "SSH forks preserve installed broker-aware hooks"
        );
        // The cwd of an ssh pane is a path on `builder`, so it is sent as-is.
        assert_eq!(command.cwd, Some("/tmp/repo".into()));
        assert_eq!(command.target.ssh_host(), Some("builder"));

        // The one offered kind whose program is not its name.
        assert_eq!(
            command_in_context(&pane, "cursor").unwrap().path,
            PathBuf::from("cursor-agent")
        );

        let local = AgentPane {
            host: None,
            ..pane.clone()
        };
        let local_command = command_in_context(&local, "claude").unwrap();
        assert_eq!(
            local_command.args,
            launch::plan(&launch::Spec {
                kind: Some("claude".into()),
                ..Default::default()
            })
            .unwrap()
            .0
            .args
        );
        assert_eq!(
            local_command.args[7], "--settings",
            "local forks keep automatic reporting hooks"
        );
        assert_eq!(local_command.cwd, Some("/tmp/repo".into()));
        assert!(local_command.target.is_local());

        for (pane, fork) in [(&pane, &command), (&local, &local_command)] {
            let duplicate =
                duplicate_command(pane, &BTreeMap::from([(7, "claude".into())])).unwrap();
            assert_eq!(duplicate.path, fork.path);
            assert_eq!(
                duplicate.args, fork.args,
                "duplicates use the same reporting hooks as forks"
            );
            assert_eq!(duplicate.cwd, fork.cwd);
            assert_eq!(duplicate.target, fork.target);
        }
    }

    #[test]
    fn explicit_hidden_sources_keep_their_agent_and_launch_context() {
        let reply: PollReply =
            serde_json::from_value(serde_json::json!({"panes": [], "activity": {}})).unwrap();
        for target in [
            PaneTarget::Local,
            PaneTarget::parse_display(Some("builder")),
        ] {
            let info = PaneInfo {
                id: PaneId::Terminal(TerminalId::new(7)),
                is_suppressed: true,
                exited: true,
                is_held: true,
                host: target.display(),
                target: target.clone(),
                agent: Some(agents::registered_agent(
                    "claude",
                    Some("/work/repo".to_owned()),
                )),
                ..Default::default()
            };
            let mut manifest = PaneManifest::default();
            manifest
                .panes
                .insert(TabPosition::new(0), vec![info.clone()]);
            assert!(agent_proto::panes::terminal_panes(&manifest, &[]).is_empty());
            let pane = source_pane(&reply, 7, |id| {
                assert_eq!(id, info.id);
                Some(info.clone())
            })
            .unwrap();
            assert_eq!(pane.agent, info.agent);
            assert_eq!(pane.status, agents::AgentStatus::Held);
            let duplicate = duplicate_command(&pane, &reply.manual_kinds).unwrap();
            assert_eq!(duplicate.path, PathBuf::from("env"));
            assert_eq!(duplicate.cwd, Some("/work/repo".into()));
            assert_eq!(duplicate.target, target);
            let fork = command_in_context(&pane, "codex").unwrap();
            if target.is_local() {
                assert_eq!(fork.path, PathBuf::from("yinz"));
                assert_eq!(fork.args, ["agent", "codex", "--command", "codex", "--"]);
            } else {
                assert_eq!(fork.path, PathBuf::from("yinz"));
                assert_eq!(
                    fork.args,
                    ["agent", "codex-untracked", "--command", "codex", "--"]
                );
            }
            assert_eq!(
                yinz_tile::agent::agent_kind_of(&fork.path, &fork.args).as_deref(),
                Some("codex")
            );
            assert_eq!(fork.cwd, duplicate.cwd);
            assert_eq!(fork.target, target);
            let codex = AgentPane {
                agent: Some(agents::registered_agent("codex", Some("/work/repo".into()))),
                command: Some("codex resume old-thread old-prompt".into()),
                ..pane
            };
            assert_eq!(
                duplicate_command(&codex, &reply.manual_kinds).unwrap(),
                fork
            );
        }
        assert!(source_pane(&reply, 7, |_| None).is_none());
        let visible = agent_pane("codex");
        let reply = PollReply {
            suppressed_panes: Vec::new(),
            panes: Some(vec![visible.clone()]),
            ..reply
        };
        assert_eq!(
            source_pane(&reply, visible.pane_id, |_| panic!(
                "visible source needs no lookup"
            )),
            Some(visible)
        );
    }

    #[test]
    fn duplicate_uses_the_agent_kind_and_exact_location_without_old_args() {
        let pane = AgentPane {
            pane_id: 7,
            title: "agent".to_owned(),
            command: Some("git status".to_owned()),
            cwd: Some("/tmp/repo".to_owned()),
            host: Some("builder".to_owned()),
            bubblewrap: None,
            status: agents::AgentStatus::Running,
            tab: Some((yinz_tile::prelude::TabId::new(1), "work".to_owned())),
            detected: Some("cursor".to_owned()),
            agent: None,
        };
        let command = duplicate_command(&pane, &BTreeMap::new()).unwrap();

        assert_eq!(command.path, PathBuf::from("cursor-agent"));
        assert!(command.args.is_empty());
        // The cwd of an ssh pane is a path on `builder`, so it is sent as-is.
        assert_eq!(command.cwd, Some("/tmp/repo".into()));
        assert_eq!(command.target.ssh_host(), Some("builder"));

        let local_pane = AgentPane {
            host: None,
            ..pane.clone()
        };
        let command = duplicate_command(&local_pane, &BTreeMap::new()).unwrap();
        assert_eq!(command.cwd, Some("/tmp/repo".into()));
        assert!(command.target.is_local());

        // A sandboxed pane's duplicate is sandboxed in the same environment.
        let sandboxed = AgentPane {
            bubblewrap: Some("/repo".to_owned()),
            ..local_pane.clone()
        };
        let command = duplicate_command(&sandboxed, &BTreeMap::new()).unwrap();
        assert_eq!(command.path, PathBuf::from("cursor-agent"));
        assert_eq!(
            command.target.bubblewrap_root(),
            Some(std::path::Path::new("/repo"))
        );

        let exited = AgentPane {
            cwd: None,
            agent: Some(agents::registered_agent(
                "cursor",
                Some("/tmp/repo".to_owned()),
            )),
            ..local_pane
        };
        assert_eq!(
            duplicate_command(&exited, &BTreeMap::new()).unwrap().cwd,
            Some("/tmp/repo".into())
        );
        assert_eq!(fork_note(&exited, "cursor"), "forking cursor in repo");

        let ordinary_pane = AgentPane {
            command: Some("bash".to_owned()),
            detected: None,
            ..pane
        };
        assert!(duplicate_command(&ordinary_pane, &BTreeMap::new()).is_none());
    }
}
