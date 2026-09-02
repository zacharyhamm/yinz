use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use hocus_proto::location::Location;
use hocus_proto::{
    agents::{self, AgentPane},
    launch,
    projects::{self, Registry},
    store::{self, Backend},
    wire::{self, PollReply},
};
use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{
    Button, Checkbox, Combo, Dialog, Form, FormEvent, Rect, Select, TextInput,
};

const PROJECT: &str = "project";
const AGENT: &str = "agent";
const MODEL: &str = "model";
const PROMPT: &str = "prompt";
const FIRECRACKER: &str = "firecracker";
const FIRECRACKER_IMAGE: &str = "firecracker_image";
const CREATE_BRANCH: &str = "create_branch";
const BASE: &str = "base";
const BRANCH: &str = "branch";
const WORKTREE: &str = "worktree";
const PATH: &str = "path";
/// Which question a `RunCommandResult` is answering, and which round it belongs
/// to. Both travel in the command's context, which comes back verbatim.
const GIT_PROBE: &str = "git_probe";
const GIT_PROBE_ID: &str = "git_probe_id";
const BRANCHES_PROBE: &str = "branches";
const WORKTREES_PROBE: &str = "worktrees";
const DUPLICATE_FOCUSED: &str = "duplicate_focused";
const CALLER_TERMINAL_PANE_ID: &str = "caller_terminal_pane_id";

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

struct State {
    dialog: Dialog,
    /// The caller's cwd: the fallback source directory, and what the default
    /// project is derived from.
    source_directory: PathBuf,
    projects: Registry,
    plugin_id: u32,
    duplicate_pane_id: Option<u32>,
    path_edited: bool,
    firecracker_image_edited: bool,
    error: Option<String>,
    /// The `(width, height)` `resize` last asked the host for, so it can decline to ask again.
    ///
    /// `TabUpdate` arrives for reasons that have nothing to do with this modal's size - including
    /// every step of a mouse drag of the modal itself - and each coordinate change re-resolves the
    /// pane. Sending only when the size actually changed keeps the drag quiet.
    requested_size: Option<(String, String)>,
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
                Select::new(launch::OFFERED.iter().copied().map(capitalized)),
            )
            .field(MODEL, "Model (optional)", TextInput::empty())
            // What the agent is started with. The same argument `agent_spawn`
            // takes, so asking hocus for an agent and filling this in are the
            // same request said two ways.
            .field(PROMPT, "Prompt (optional)", TextInput::empty())
            .field(FIRECRACKER, "Firecracker VM (optional)", TextInput::empty())
            .field(
                FIRECRACKER_IMAGE,
                "Firecracker image (optional)",
                TextInput::empty(),
            )
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
                // Named because a relative path is anchored to the repository's
                // primary checkout, not to the pane's cwd — which differs whenever
                // the agent asking sits in a worktree already.
                "Worktree path (from repo root)",
                Combo::empty().with_text(launch::WORKTREE_PREFIX),
            )
            .button("create", Button::new("Create agent"))
            // Half a form filled in and an Enter pressed out of habit used to
            // be enough to start an agent. Nothing but the button starts one.
            .enter_never_submits();
        // Replaced every render by `help_for`, which says what Enter does in
        // whichever field has focus. Set here too so the dialog reserves the
        // row from the start.
        let help = help_for(&form);
        Self {
            dialog: Dialog::new(Text::new("New agent").color_all(2), form)
                .help(Text::new(help))
                .status_row(),
            source_directory: PathBuf::from("."),
            projects: Registry::new(),
            plugin_id: 0,
            duplicate_pane_id: None,
            path_edited: false,
            firecracker_image_edited: false,
            error: None,
            requested_size: None,
            git_probe: 0,
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.source_directory = configuration
            .get("caller_cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.load_projects();
        self.fill_projects();
        self.plugin_id = get_plugin_ids().plugin_id;
        if configuration.get(DUPLICATE_FOCUSED).map(String::as_str) == Some("true") {
            self.duplicate_pane_id = configuration
                .get(CALLER_TERMINAL_PANE_ID)
                .and_then(|pane_id| pane_id.parse().ok());
            if self.duplicate_pane_id.is_some() {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            }
        }
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::TabUpdate,
            EventType::PastedText,
            EventType::RunCommandResult,
            EventType::Visible,
        ]);
        self.refresh_git_suggestions();
        set_selectable(true);
        rename_plugin_pane(self.plugin_id, "new agent");
        self.resize(None);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::TabUpdate(tabs) => {
                if let Some(tab) = tabs.first() {
                    self.resize(Some(tab.display_area_columns));
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
            Event::Visible(false) => {
                close_self();
                false
            },
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let help = help_for(self.dialog.form());
        self.dialog.set_help(Some(Text::new(help)));
        self.dialog.set_status(
            self.error
                .as_ref()
                .map(|error| Text::new(error).color_all(3)),
        );
        self.dialog.render(Rect::new(0, 0, cols, rows));
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_HELLO && self.duplicate_pane_id.is_some() {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
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
        let Some(pane_id) = self.duplicate_pane_id.take() else {
            return false;
        };
        let command = reply
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .and_then(|pane| duplicate_command(pane, &reply.manual_kinds));
        if let Some(new_pane_id) = command.and_then(|command| {
            open_command_pane_near_plugin(
                command,
                BTreeMap::from([(HOCUS_AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
            )
        }) {
            hand_over_focus(new_pane_id);
        }
        false
    }
}

/// The help line for whatever has focus.
///
/// Enter means something different in every widget here, so one static line
/// cannot describe it. Every arm has to fit the narrowest modal - see
/// `the_help_line_fits_the_narrowest_modal`.
fn help_for(form: &Form) -> &'static str {
    let Some(field) = form.focused_id().and_then(|id| form.field_by_id(id)) else {
        // No field of that id: focus is on the button, or the form is empty.
        return "<Enter> create  <Tab> next  <Esc> cancel";
    };
    if let Some(select) = field.select() {
        return if select.is_open() {
            "<Enter> choose  <↑↓> move  <Esc> close"
        } else {
            "<Enter> open  <Tab> next  <Esc> cancel"
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
            "<Enter> edit  <Tab> next  <Esc> cancel"
        };
    }
    if field.checkbox().is_some() {
        return "<Enter> toggle  <Tab> next  <Esc> cancel";
    }
    if field.radio().is_some() {
        return "<Enter> change  <Tab> next  <Esc> cancel";
    }
    if form.is_editing() {
        "<Enter> done  <Tab> next  <Esc> back"
    } else {
        "<Enter> edit  <Tab> next  <Esc> cancel"
    }
}

fn modal_width(display_columns: Option<usize>) -> String {
    display_columns
        .map(|columns| columns.div_ceil(2).min(60).to_string())
        .unwrap_or_else(|| "50%".to_owned())
}

/// Rows of pane the dialog needs: its own rows plus the two the pane frame
/// takes. Derived rather than written down so that adding a field - or the
/// error row - cannot quietly start scrolling the form.
fn modal_height(dialog: &Dialog) -> String {
    (dialog.height() + FRAME_ROWS).to_string()
}

impl State {
    fn resize(&mut self, display_columns: Option<usize>) {
        let size = (modal_width(display_columns), modal_height(&self.dialog));
        if self.requested_size.as_ref() == Some(&size) {
            // nothing about the modal's size changed, so there is nothing to ask for. A
            // `TabUpdate` is not evidence that this pane wants to be a different shape.
            return;
        }
        let (width, height) = size.clone();
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            None,
            Some("20%".into()),
            Some(width),
            Some(height),
            None,
            None,
        ) {
            // Already an overlay by the time this runs - the keybinding opens it as a modal. This
            // resizes it where it is; `OverlayPanes` keeps the kind, and keeps the position too
            // once the user has dragged the modal - which is what makes the `None` x and the 20% y
            // an *initial* placement rather than one re-imposed for the life of the pane.
            change_overlay_coordinates(vec![(PaneId::Plugin(self.plugin_id), coordinates)]);
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
        // declared is an error and any of the hocus plugins may start first.
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

/// The repository's linked worktrees, written the way the field is labelled.
///
/// `git worktree list --porcelain` opens each block with a `worktree <path>`
/// line and lists the primary checkout first. That first entry is dropped: it is
/// the repository itself, which is what "use worktree" unticked already means,
/// and offering it as a worktree path would be offering to work in the main
/// checkout under another name.
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
    paths
        .map(|path| {
            Path::new(path)
                .strip_prefix(primary)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_owned())
        })
        .collect()
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
/// that is *containment only*. hocus resolves its own active project differently
/// (`project_for_cwd`, default-plugins/hocus/src/tool_loop.rs): it first requires
/// the folder to be a Git checkout, then prefers `projects::for_repository` - the
/// remote, or the common directory - and falls back to `for_cwd` only after that.
/// It can, because the daemon probes Git for it; this modal has no probe of its
/// own and no rows to read one out of, so it cannot follow. Two differences come
/// of that, both survivable because the first option is always there:
///
/// - a linked worktree living *outside* its project's root matches nothing here
///   and defaults to `CURRENT_DIRECTORY`, where hocus would recognise the project
///   by its remote;
/// - a folder that is not a checkout but sits below a project root the user named
///   defaults to that project, where hocus would report no project. Should that
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

fn duplicate_command(
    pane: &AgentPane,
    manual_kinds: &BTreeMap<u32, String>,
) -> Option<CommandToRun> {
    let (kind, _) = agents::classify(pane, manual_kinds)?;
    // A pane on another machine, ssh or Firecracker, already reports a cwd on
    // that machine, and the server hands a remote cwd over untranslated. Only a
    // local pane gets `/host`, which the server maps to the caller's cwd.
    let mut target = PaneTarget::parse_display(pane.host.as_deref());
    if let PaneTarget::Firecracker {
        image, worktree, ..
    } = &mut target
    {
        image.clone_from(&pane.firecracker_image);
        *worktree = pane.firecracker_worktree.clone().map(Into::into);
    }
    let cwd = if target.is_local() {
        Some("/host".to_owned())
    } else {
        pane.cwd.clone()
    };
    Some(CommandToRun {
        path: launch::binary_for(kind).into(),
        cwd: cwd.map(Into::into),
        target,
        ..Default::default()
    })
}

impl State {
    fn handle(&mut self, event: FormEvent) -> bool {
        match event {
            // Only the button. `FormEvent::Submit` cannot reach here - the form
            // is built with `enter_never_submits` - and saying so in the match
            // is what keeps a stray Enter from ever creating an agent again.
            FormEvent::ButtonPressed(_) => self.submit(),
            FormEvent::Cancel => {
                close_self();
                false
            },
            FormEvent::FieldChanged(id) => {
                self.changed(&id);
                true
            },
            other => other.should_render(),
        }
    }

    fn changed(&mut self, id: &str) {
        self.error = None;
        // The branches and worktrees on offer are the selected project's, so a
        // different project means a different list.
        if id == PROJECT {
            self.refresh_git_suggestions();
            self.fill_firecracker_image_default();
        }
        if id == FIRECRACKER {
            self.fill_firecracker_image_default();
        }
        if id == FIRECRACKER_IMAGE {
            self.firecracker_image_edited = true;
        }
        if id == CREATE_BRANCH && !self.dialog.form().checked(CREATE_BRANCH) {
            self.set_checked(WORKTREE, false);
        }
        if id == WORKTREE && self.dialog.form().checked(WORKTREE) {
            self.set_checked(CREATE_BRANCH, true);
        }
        if id == PATH {
            self.path_edited = true;
        }
        if id == BRANCH && !self.path_edited {
            let branch = self.dialog.form().text(BRANCH).to_owned();
            if let Some(combo) = self.dialog.form_mut().combo_by_id_mut(PATH) {
                combo.set_text(launch::worktree_path_for(&branch));
            }
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

    fn set_checked(&mut self, id: &str, checked: bool) {
        if let Some(checkbox) = self.dialog.form_mut().checkbox_by_id_mut(id) {
            checkbox.set_checked(checked);
        }
    }

    fn fill_firecracker_image_default(&mut self) {
        if self.firecracker_image_edited {
            return;
        }
        let image = selected_project(self.dialog.form(), &self.projects)
            .filter(|_| !self.dialog.form().text(FIRECRACKER).trim().is_empty())
            .and_then(|project| project.firecracker_image.clone())
            .unwrap_or_default();
        self.set_text(FIRECRACKER_IMAGE, &image);
    }

    fn submit(&mut self) -> bool {
        // `request` for the one check that is the modal's own; `launch::open` plans
        // it again and does the work, which is the same call `agent_spawn` makes.
        if let Err(error) = request(self.dialog.form()) {
            self.error = Some(error);
            return true;
        }
        let source = source_directory(self.dialog.form(), &self.projects, &self.source_directory);
        match launch::open(&spec(self.dialog.form()), source) {
            Ok(pane_id) => {
                hand_over_focus(pane_id);
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
/// Focus first, then close. While this pane is still up it goes on reading as
/// the focused one whatever the tiled layer says, so the focus call moves the
/// tiled focus *underneath* - and closing the last floating pane hands that
/// tiled focus back out. The other order has nothing left to focus from: the
/// close would restore whichever pane the user came from, and this plugin is
/// gone by then.
fn hand_over_focus(pane_id: PaneId) {
    focus_pane_with_id(pane_id, false, false);
    close_self();
}

/// What the form is asking for, in the terms hocus asks for the same thing.
///
/// Reading only — every default and every rule about what pairs with what lives in
/// `launch`, which is also what `agent_spawn` calls. A blank field is left blank
/// here and defaulted there, so a field the user cleared and a key the model omitted
/// come out the same.
fn spec(form: &Form) -> launch::Spec {
    launch::Spec {
        // The select's labels are the shared kinds, capitalised; `launch` lowercases.
        kind: Some(form.text(AGENT).to_owned()),
        command: None,
        args: Vec::new(),
        model: Some(form.text(MODEL).to_owned()),
        prompt: Some(form.text(PROMPT).to_owned()),
        firecracker: Some(form.text(FIRECRACKER).to_owned()),
        firecracker_image: Some(form.text(FIRECRACKER_IMAGE).to_owned()),
        // No branch is how `launch` is told to leave the repository alone, which is
        // exactly what an unchecked box means.
        branch: form
            .checked(CREATE_BRANCH)
            .then(|| form.text(BRANCH).to_owned()),
        base: Some(form.text(BASE).to_owned()),
        // Only read while a branch is being created: the checkbox stays checked
        // underneath an unchecked `create_branch`, and reporting it there would ask
        // `launch` for a worktree with nothing to put in it.
        worktree: form.checked(CREATE_BRANCH).then(|| form.checked(WORKTREE)),
        worktree_path: Some(form.text(PATH).to_owned()),
        ..Default::default()
    }
}

/// The form as the pair Zellij wants. Kept as its own function because the tests
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
            return Err("worktree path is required".to_owned());
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
mod tests {
    use super::*;
    use hocus_proto::projects::Project;

    fn project(name: &str, root: &str) -> (String, Project) {
        (
            name.to_owned(),
            Project {
                name: name.to_owned(),
                root: root.to_owned(),
                firecracker_image: None,
                remote: None,
                worktree_status: None,
                common_dir: None,
                discovered: false,
            },
        )
    }

    fn registry() -> Registry {
        Registry::from([
            project("zellij", "/work/zellij"),
            project("dobbs", "/work/dobbs"),
        ])
    }

    #[test]
    fn modal_is_half_the_display_up_to_sixty_columns() {
        assert_eq!(modal_width(Some(80)), "40");
        assert_eq!(modal_width(Some(200)), "60");
    }

    #[test]
    fn the_modal_is_tall_enough_for_the_whole_dialog() {
        let state = State::default();
        assert_eq!(modal_height(&state.dialog), "19");
        assert_eq!(state.dialog.height() + FRAME_ROWS, 19);
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
        assert_eq!(command.path, PathBuf::from("codex"));
        assert!(
            matches!(checkout, GitCheckout::NewWorktree { branch, base, path }
            if branch == "topic" && base == "main" && path == *".worktrees/topic")
        );
    }

    #[test]
    fn choosing_a_firecracker_vm_keeps_host_worktree_setup() {
        let mut state = State::default();
        state.set_text(FIRECRACKER, "review");
        state.changed(FIRECRACKER);
        assert!(state.dialog.form().checked(CREATE_BRANCH));
        assert!(state.dialog.form().checked(WORKTREE));
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        let (command, checkout) = request(state.dialog.form()).unwrap();
        assert_eq!(command.target.firecracker_vm(), Some("review"));
        assert!(matches!(checkout, GitCheckout::NewWorktree { .. }));
    }

    /// The two checks `launch::plan` makes of the Firecracker fields, shown in
    /// the status row like every other refusal rather than reaching the host.
    #[test]
    fn an_image_without_a_vm_is_refused_on_submit() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        state.set_text(FIRECRACKER_IMAGE, "rust");
        state.changed(FIRECRACKER_IMAGE);
        assert!(state.submit());
        assert_eq!(
            state.error.as_deref(),
            Some("firecracker_image and firecracker_worktree need a firecracker_vm")
        );
    }

    #[test]
    fn an_invalid_vm_name_is_refused_on_submit() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        state.set_text(FIRECRACKER, "no spaces");
        state.changed(FIRECRACKER);
        assert!(state.submit());
        let error = state.error.clone().expect("the name was refused");
        assert!(error.contains("invalid Firecracker VM name"), "{error}");
    }

    /// The point of the shared module: the modal's form and hocus's `agent_spawn`
    /// arguments are two spellings of one request, and must resolve alike.
    #[test]
    fn the_form_and_a_bare_spec_ask_for_the_same_thing() {
        let mut state = State::default();
        state.set_text(BRANCH, "topic");
        state.changed(BRANCH);
        state.set_text(MODEL, "opus");
        state.set_text(PROMPT, "fix the parser");
        let (from_form, checkout) = request(state.dialog.form()).unwrap();
        let (from_spec, spec_checkout) = launch::plan(&launch::Spec {
            kind: Some("codex".to_owned()),
            model: Some("opus".to_owned()),
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
        assert_eq!(command.args, ["fix the parser"]);
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
            "worktree path is required"
        );
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
        assert_eq!(command.path, PathBuf::from("claude"));
        assert_eq!(&command.args[..2], ["--model", "opus"]);
        // A claude pane the modal starts reports its own state, like one hocus starts.
        assert_eq!(command.args[2], "--settings");
        assert_eq!(command.args.len(), 4);
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
        // terminal worth drawing this in.
        let narrowest: usize = modal_width(Some(80)).parse().unwrap();
        let mut state = State::default();
        let mut lines = vec![];
        for id in [PROJECT, AGENT, MODEL, CREATE_BRANCH, BASE, "create"] {
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
    fn the_project_field_is_first_and_defaults_to_the_active_project() {
        // A worktree below the project root is still the project's.
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/work/zellij/.worktrees/topic"),
            ..Default::default()
        };
        state.fill_projects();

        assert_eq!(state.dialog.form().fields()[0].id(), PROJECT);
        let select = state.dialog.form().select_by_id(PROJECT).unwrap();
        assert_eq!(select.options(), [CURRENT_DIRECTORY, "dobbs", "zellij"]);
        assert_eq!(select.selected_option(), Some("zellij"));
    }

    /// The finding this fixes: the caller's own folder had no option at all once
    /// a project owned it, and `git switch -c` for a branch without a worktree
    /// then ran in the project's primary checkout instead of the worktree the
    /// modal was opened from.
    #[test]
    fn the_current_directory_is_offered_from_inside_a_project() {
        let worktree = PathBuf::from("/work/zellij/.worktrees/topic");
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
        assert_eq!(select.options(), [CURRENT_DIRECTORY, "dobbs", "zellij"]);
        assert_eq!(select.selected_option(), Some(CURRENT_DIRECTORY));

        let (options, selected) = project_options(&Registry::new(), Path::new("/elsewhere"));
        assert_eq!(options, [CURRENT_DIRECTORY]);
        assert_eq!(selected, 0);
    }

    #[test]
    fn the_agent_is_created_in_the_selected_projects_root() {
        let mut state = State {
            projects: registry(),
            source_directory: PathBuf::from("/work/zellij"),
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

    #[test]
    fn firecracker_image_defaults_follow_the_project_until_edited() {
        let mut projects = registry();
        projects.get_mut("zellij").unwrap().firecracker_image = Some("rust".to_owned());
        projects.get_mut("dobbs").unwrap().firecracker_image = Some("ruby".to_owned());
        let mut state = State {
            projects,
            source_directory: PathBuf::from("/work/zellij"),
            ..Default::default()
        };
        state.fill_projects();
        state.changed(PROJECT);
        assert_eq!(state.dialog.form().text(FIRECRACKER_IMAGE), "");

        state.set_text(FIRECRACKER, "review");
        state.changed(FIRECRACKER);
        assert_eq!(state.dialog.form().text(FIRECRACKER_IMAGE), "rust");

        state.set_text(FIRECRACKER, "");
        state.changed(FIRECRACKER);
        assert_eq!(state.dialog.form().text(FIRECRACKER_IMAGE), "");

        state.set_text(FIRECRACKER, "review");
        state.changed(FIRECRACKER);

        state
            .dialog
            .form_mut()
            .select_by_id_mut(PROJECT)
            .unwrap()
            .select_option("dobbs");
        state.changed(PROJECT);
        assert_eq!(state.dialog.form().text(FIRECRACKER_IMAGE), "ruby");

        state.set_text(FIRECRACKER_IMAGE, "");
        state.changed(FIRECRACKER_IMAGE);
        state
            .dialog
            .form_mut()
            .select_by_id_mut(PROJECT)
            .unwrap()
            .select_option("zellij");
        state.changed(PROJECT);
        assert_eq!(state.dialog.form().text(FIRECRACKER_IMAGE), "");
    }

    /// The primary checkout is not a worktree path anyone can ask for - leaving
    /// the "use worktree" box unticked is what running there means - and the
    /// rest are written the way the field is labelled.
    #[test]
    fn the_worktree_suggestions_are_the_linked_checkouts_relative_to_the_root() {
        let porcelain = "worktree /work/zellij\nHEAD abcd\nbranch refs/heads/main\n\
             \nworktree /work/zellij/.worktrees/topic\nHEAD ef01\nbranch refs/heads/topic\n\
             \nworktree /elsewhere/detached\nHEAD 2345\ndetached\n";
        assert_eq!(
            worktree_suggestions(porcelain),
            // The one outside the root cannot be written relative to it.
            [".worktrees/topic", "/elsewhere/detached"]
        );
        // A repository with no linked worktrees has nothing to suggest, and a
        // folder that is not a checkout answers with nothing at all.
        assert!(worktree_suggestions("worktree /work/zellij\nbranch refs/heads/main\n").is_empty());
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
            "worktree /work/zellij\n\nworktree /work/zellij/.worktrees/topic\n",
            &context(WORKTREES_PROBE, 3),
        ));
        assert_eq!(
            state.dialog.form().combo_by_id(PATH).unwrap().suggestions(),
            [".worktrees/topic"]
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

    #[test]
    fn duplicate_uses_the_agent_kind_and_exact_location_without_old_args() {
        let pane = AgentPane {
            pane_id: 7,
            title: "agent".to_owned(),
            command: Some("git status".to_owned()),
            cwd: Some("/tmp/repo".to_owned()),
            host: Some("builder".to_owned()),
            firecracker_image: None,
            firecracker_worktree: None,
            status: agents::AgentStatus::Running,
            tab: Some((1, "work".to_owned())),
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
        assert_eq!(command.cwd, Some("/host".into()));
        assert!(command.target.is_local());

        let firecracker_pane = AgentPane {
            host: Some("firecracker:review".to_owned()),
            firecracker_image: Some("rust-dev".to_owned()),
            firecracker_worktree: Some("/host/repo".to_owned()),
            ..pane.clone()
        };
        let command = duplicate_command(&firecracker_pane, &BTreeMap::new()).unwrap();
        assert_eq!(command.cwd, Some("/tmp/repo".into()));
        assert_eq!(
            command.target,
            PaneTarget::Firecracker {
                vm: "review".to_owned(),
                image: Some("rust-dev".to_owned()),
                worktree: Some("/host/repo".into()),
            }
        );

        let ordinary_pane = AgentPane {
            command: Some("bash".to_owned()),
            detected: None,
            ..pane
        };
        assert!(duplicate_command(&ordinary_pane, &BTreeMap::new()).is_none());
    }
}
