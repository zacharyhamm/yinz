//! The pane ledger: whether each plugin instance owns a pane, and the one place that decides it.
//!
//! A plugin owns a pane only if its entry file declares one (`(yinz-declare-pane)`, or
//! `(yinz-declare-pane-unless …)` when its configuration does not match). The declaration is
//! learned by a static scan before the load is queued, so the ledger's transitions run on the
//! thread that prepares the load. An undeclared plugin never gets a pane record: no
//! `ScreenInstruction::AddPlugin` is ever produced for it, and anything the screen holds for its
//! id is detached with `ScreenInstruction::PluginPaneless`.
//!
//! A launch's completion travels to the screen inside the instruction that tells it what became
//! of the plugin, and the screen answers it: the launcher's next action reaches the screen on
//! another channel, so only the screen can make sure it hears about the plugin first.
//!
//! Every transition is pure: it updates the ledger and returns the [`PaneEffect`]s that carry it
//! out. The caller sends them with [`send_effects`] while holding the ledger lock, so concurrent
//! preparations cannot publish transitions out of order. Never hold this lock with the plugin map.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yinz_bus::background_jobs::BackgroundJob;
use yinz_bus::loading_indication::LoadingIndication;
use yinz_bus::plugins::PluginInstruction;
use yinz_bus::pty::ReplacedPane;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::OverlayPlacement;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::{ClientId, PluginId, TabId};
use yinz_types::pane_id::PaneId;

/// Whether a running plugin instance has a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    Pane,
    Headless,
}

/// What the launcher of a plugin asked for, before the plugin's declaration is known.
#[derive(Debug)]
pub(crate) enum PaneRequest {
    /// A background start: `load_plugins`, `plugins start`, `LoadBackgroundPlugin`, headless
    /// hooks, the notification policy, the picker driver and host-required providers. A declared
    /// plugin gets a parked (suppressed) pane record; an undeclared one gets nothing.
    Parked {
        run: RunPluginOrAlias,
        cwd: Option<PathBuf>,
        client_id: ClientId,
    },
    /// A request for a pane: the `Load` instruction, a reload that starts the plugin instead,
    /// `OpenPluginPaneInPlaceOfPaneId`, the `ReplacePane` hook, cold pipes and `LoadIntoSlot`.
    Show(Box<ShowPane>),
    /// The screen owns, or will own, the pane's slot: layout plugins of `NewTab` and
    /// `OverrideLayout` (resurrection included), and `LoadPaneTab`.
    Placed { run: RunPluginOrAlias },
}

impl PaneRequest {
    /// The launcher's client, replaced by the one the load actually runs for: the pane goes to a
    /// connected client rather than a CLI connection. A request made for no client keeps none.
    pub(crate) fn for_client(mut self, resolved: ClientId) -> Self {
        match &mut self {
            PaneRequest::Parked { client_id, .. } => *client_id = resolved,
            PaneRequest::Show(show) => {
                if show.client_id.is_some() {
                    show.client_id = Some(resolved);
                }
            },
            PaneRequest::Placed { .. } => {},
        }
        self
    }

    /// Answer the request's completion with `error`: the load failed before the ledger saw it.
    pub(crate) fn reject(self, error: &str) {
        if let PaneRequest::Show(show) = self
            && let Some(completion) = show.completion
        {
            refuse(completion, error.to_owned());
        }
    }
}

/// Answer a launch's completion with an error, which a CLI launcher exits with status 1 for.
fn refuse(mut completion: NotificationEnd, error: String) {
    completion.set_exit_status(1);
    completion.set_error_message(error);
}

/// The payload of a `ScreenInstruction::AddPlugin` for a [`PaneRequest::Show`], short of the
/// plugin id (unknown until the load starts) and `start_suppressed` (always false here).
#[derive(Debug)]
pub(crate) struct ShowPane {
    pub should_float: Option<bool>,
    pub in_place: bool,
    pub replaced: ReplacedPane,
    pub run: RunPluginOrAlias,
    pub title: Option<String>,
    pub tab: Option<TabId>,
    pub pane_id_to_replace: Option<PaneId>,
    pub cwd: Option<PathBuf>,
    pub coordinates: Option<FloatingPaneCoordinates>,
    pub should_focus: Option<bool>,
    pub client_id: Option<ClientId>,
    pub completion: Option<NotificationEnd>,
    pub overlay: Option<OverlayPlacement>,
    /// `Some(slot)` only from `LoadIntoSlot`: the slot is held for this plugin, so an undeclared
    /// plugin closes it and is unloaded instead of running headless.
    pub reserved_slot: Option<PaneId>,
}

impl ShowPane {
    /// The screen-side half of a `ReplacePane` hook: a pane opened over `pane_id_to_replace`,
    /// which is suppressed behind it rather than closed.
    ///
    /// Suppressed rather than closed so that the pane's scrollback survives and it can be re-run -
    /// which is exactly what the shipped `agent_exit` screen offers to do. `should_focus` is false:
    /// an agent exiting in a tab the user is not looking at must not pull them out of the pane they
    /// are working in. That was true of the banner this replaces, and is the reason a hook is a
    /// pane rather than a session-wide modal.
    pub(crate) fn in_place_of_hooked_pane(
        run: RunPluginOrAlias,
        pane_id_to_replace: PaneId,
        title: Option<String>,
        cwd: Option<PathBuf>,
        client_id: ClientId,
    ) -> Self {
        Self {
            should_float: Some(false),
            in_place: true,
            // Not lent: the hook screen and the pane it reports on are one thing, so the user
            // closing the screen closes that pane too.
            replaced: ReplacedPane::Suppress,
            run,
            title,
            tab: None,
            pane_id_to_replace: Some(pane_id_to_replace),
            cwd,
            coordinates: None,
            should_focus: Some(false),
            client_id: Some(client_id),
            completion: None,
            overlay: None,
            reserved_slot: None,
        }
    }

    /// A multipane child: the plugin opens floating and unfocused in place of the slot held for
    /// it, which it closes. An undeclared plugin closes the slot and is unloaded.
    pub(crate) fn into_slot(
        run: RunPluginOrAlias,
        slot: PaneId,
        client_id: ClientId,
        cwd: PathBuf,
    ) -> Self {
        Self {
            should_float: Some(true),
            in_place: true,
            replaced: ReplacedPane::Close,
            run,
            title: None,
            tab: None,
            pane_id_to_replace: Some(slot),
            cwd: Some(cwd),
            coordinates: None,
            should_focus: Some(false),
            client_id: Some(client_id),
            completion: None,
            overlay: None,
            reserved_slot: Some(slot),
        }
    }

    fn into_add_plugin(self, plugin_id: PluginId) -> ScreenInstruction {
        let start_suppressed = false;
        ScreenInstruction::AddPlugin(
            self.should_float,
            self.in_place,
            self.replaced,
            self.run,
            self.title,
            self.tab,
            plugin_id,
            self.pane_id_to_replace,
            self.cwd,
            start_suppressed,
            self.coordinates,
            self.should_focus,
            self.client_id,
            self.completion,
            self.overlay,
        )
    }
}

/// The screen-side half of a declared background plugin: a suppressed pane that holds no place in
/// any layout, so the plugin has a `PaneId` to be closed by but nothing on screen.
fn parked_add_plugin(
    run_plugin_or_alias: RunPluginOrAlias,
    plugin_id: PluginId,
    cwd: Option<PathBuf>,
    client_id: ClientId,
) -> ScreenInstruction {
    let should_float = None;
    let should_be_open_in_place = false;
    let pane_title = None;
    let pane_id_to_replace = None;
    let start_suppressed = true;
    ScreenInstruction::AddPlugin(
        should_float,
        should_be_open_in_place,
        ReplacedPane::Suppress,
        run_plugin_or_alias,
        pane_title,
        None,
        plugin_id,
        pane_id_to_replace,
        cwd,
        start_suppressed,
        None,
        None,
        Some(client_id),
        None,
        None,
    )
}

/// How a plugin was first asked for, kept after its request is answered: a reload that changes
/// the declaration decides from it what the plugin gets.
#[derive(Debug, Clone)]
enum Origin {
    Parked {
        run: RunPluginOrAlias,
        cwd: Option<PathBuf>,
        client_id: ClientId,
    },
    Show {
        run: RunPluginOrAlias,
        cwd: Option<PathBuf>,
        client_id: Option<ClientId>,
        tab: Option<TabId>,
    },
    Placed {
        run: RunPluginOrAlias,
    },
}

impl Origin {
    fn of(request: &PaneRequest) -> Self {
        match request {
            PaneRequest::Parked {
                run,
                cwd,
                client_id,
            } => Origin::Parked {
                run: run.clone(),
                cwd: cwd.clone(),
                client_id: *client_id,
            },
            PaneRequest::Show(show) => Origin::Show {
                run: show.run.clone(),
                cwd: show.cwd.clone(),
                client_id: show.client_id,
                tab: show.tab,
            },
            PaneRequest::Placed { run } => Origin::Placed { run: run.clone() },
        }
    }

    fn run(&self) -> &RunPluginOrAlias {
        match self {
            Origin::Parked { run, .. } | Origin::Show { run, .. } | Origin::Placed { run } => run,
        }
    }
}

#[derive(Debug)]
enum PaneState {
    /// The declaration is not known yet. `request` is `None` once a load failure consumed it.
    Pending {
        origin: Origin,
        request: Option<PaneRequest>,
        /// Where a pane requested by a `Show` that failed before its declaration was known goes
        /// if a reload finds one: the placement asked for, without its completion (answered by
        /// the failure). `None` for any other request, and for one in place of another pane.
        retry: Option<Box<ShowPane>>,
        /// The screen was told the plugin is loading (`PluginResolving`), so it must hear if the
        /// load fails before the declaration is known.
        resolving: bool,
    },
    /// The plugin declares a pane, and the screen has (or is placing) its record.
    Owned { origin: Origin },
    /// The plugin declares no pane and runs without one.
    Headless {
        origin: Origin,
        /// The failure last toasted, so a failing plugin toasts once per load and once per
        /// generation rather than once per failure report.
        toasted: Option<Failure>,
    },
}

#[derive(Debug)]
struct Entry {
    background: bool,
    /// The raw scan result for the current load configuration: the eval check compares the
    /// evaluated module's declaration against it.
    scanned: Option<bool>,
    /// The plugin's latest load or reload. A declaration or failure of any earlier one is stale:
    /// that load is cancelled, and must not decide the plugin's pane.
    load_id: u64,
    state: PaneState,
}

/// Whether the screen should receive the plugin's pane, and with what.
#[derive(Debug, Default)]
pub(crate) struct PaneLedger {
    entries: HashMap<PluginId, Entry>,
    /// Locations already toasted for losing a slot, so a layout repeated across tabs explains
    /// itself once.
    retired_slot_notices: HashSet<String>,
}

pub(crate) type PluginPanes = Arc<Mutex<PaneLedger>>;

/// One message a ledger transition needs sent.
#[derive(Debug)]
pub(crate) enum PaneEffect {
    Screen(ScreenInstruction),
    Background(BackgroundJob),
    /// Send `PluginInstruction::Unload(id)` to the plugin thread, which runs it after the current
    /// call returns.
    Unload(PluginId),
}

/// Where a reload that finds a declaration puts a plugin that had no pane.
enum Place {
    /// The screen still holds the slot the plugin was placed into.
    SlotExists,
    /// The placement first asked for, by a launch that failed before its declaration was known.
    AsRequested(Box<ShowPane>),
    /// A parked record for a background plugin; otherwise floating and unfocused.
    Floating,
}

/// What the load does after the declaration is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prepared {
    Proceed(Surface),
    /// Do not queue the load: the plugin is being unloaded, or its load already failed.
    Abort,
}

/// Which failure [`PaneLedger::failure_effects`] reports, and of which attempt. A headless
/// plugin toasts each attempt once: load ids and generations are separate counters, so a load
/// and a crash never share a key however their numbers fall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The load failed to evaluate the plugin or to run `load`.
    Load { load_id: Option<u64> },
    /// A running instance of the generation crashed.
    Crash { generation: Option<u64> },
}

/// Send `effects` in order, holding the ledger lock until publication finishes.
pub(crate) fn send_effects(senders: &ThreadSenders, effects: Vec<PaneEffect>) {
    for effect in effects {
        match effect {
            PaneEffect::Screen(instruction) => senders
                .send_to_screen(instruction)
                .context("send a plugin pane instruction to the screen")
                .non_fatal(),
            PaneEffect::Background(job) => senders
                .send_to_background_jobs(job)
                .context("send a plugin loading animation job")
                .non_fatal(),
            PaneEffect::Unload(plugin_id) => senders
                .send_to_plugin(PluginInstruction::Unload(plugin_id))
                .with_context(|| format!("unload plugin {plugin_id}, which has no place"))
                .non_fatal(),
        }
    }
}

/// One line of `error` for a notification, saying what went wrong rather than what was being
/// attempted: the message under a Steel diagnostic's carets, else the first line of the root
/// cause of an `anyhow` chain (`Caused by:`), else the first line. Crash reports end their
/// lines with `\n\r` for the pane they were written for, so carriage returns are not text.
fn summary(error: &str) -> &str {
    let lines: Vec<&str> = error.lines().map(|line| line.trim_matches('\r')).collect();
    let first = lines.first().copied().unwrap_or_default();
    // A caret line is a diagnostic's gutter and carets, never text with a `^` in it (a regex in
    // an error message, say).
    let diagnostic = lines.iter().rev().find_map(|line| {
        let (gutter, after) = line.split_once('^')?;
        if !gutter
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '│' | '|'))
        {
            return None;
        }
        let message = after.trim_start_matches('^').trim();
        (!message.is_empty()).then_some(message)
    });
    if let Some(message) = diagnostic {
        return message;
    }
    let Some(caused_by) = lines.iter().position(|line| line.trim() == "Caused by:") else {
        return first;
    };
    let causes: Vec<&str> = lines[caused_by + 1..]
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect();
    // Each cause starts `    N: `; a single one is not numbered.
    causes
        .iter()
        .rev()
        .find_map(|line| {
            let (index, cause) = line.trim_start().split_once(": ")?;
            index.chars().all(|c| c.is_ascii_digit()).then_some(cause)
        })
        .or_else(|| causes.first().map(|line| line.trim()))
        .unwrap_or(first)
}

fn toast(plugin_id: PluginId, text: String) -> PaneEffect {
    PaneEffect::Screen(ScreenInstruction::PluginNotify {
        text,
        pane_id: None,
        on_click: None,
        plugin_id,
        completion: None,
    })
}

/// The name a pane's loading indication shows: the resolved location, as the loader names it.
fn loading_name(run: &RunPluginOrAlias) -> String {
    match run.get_run_plugin() {
        Some(run_plugin) => run_plugin.location.to_string(),
        None => run.location_string(),
    }
}

/// Today's painted failure: the loading animation stops and the error shows in the pane.
fn painted_failure(plugin_id: PluginId, name: String, error: &str) -> Vec<PaneEffect> {
    let mut loading_indication = LoadingIndication::new(name);
    loading_indication.indicate_loading_error(error.to_owned());
    vec![
        PaneEffect::Background(BackgroundJob::StopPluginLoadingAnimation(plugin_id)),
        PaneEffect::Screen(ScreenInstruction::UpdatePluginLoadingStage(
            plugin_id,
            loading_indication,
        )),
    ]
}

fn paneless(
    plugin_id: PluginId,
    run: &RunPluginOrAlias,
    completion: Option<NotificationEnd>,
) -> PaneEffect {
    PaneEffect::Screen(ScreenInstruction::PluginPaneless {
        plugin_id,
        run: Box::new(run.clone()),
        completion,
    })
}

/// Fills in the answer to a launch whose plugin runs headless. The screen sends it, once it has
/// recorded the plugin as headless.
fn answered_headless(
    plugin_id: PluginId,
    location: &str,
    mut completion: NotificationEnd,
) -> NotificationEnd {
    completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
    completion.set_stdout_message(format!("{location} runs headless: it declares no pane"));
    completion
}

impl PaneLedger {
    /// Record what the launcher asked for. Called by `load_plugin` before the load is prepared.
    pub(crate) fn request(&mut self, plugin_id: PluginId, load_id: u64, request: PaneRequest) {
        let entry = Entry {
            background: matches!(request, PaneRequest::Parked { .. }),
            scanned: None,
            load_id,
            state: PaneState::Pending {
                origin: Origin::of(&request),
                request: Some(request),
                retry: None,
                resolving: false,
            },
        };
        if let Some(Entry {
            state:
                PaneState::Pending {
                    request: Some(PaneRequest::Show(show)),
                    ..
                },
            ..
        }) = self.entries.insert(plugin_id, entry)
            && let Some(completion) = show.completion
        {
            refuse(
                completion,
                format!("plugin {plugin_id} was loaded again before its first load finished"),
            );
        }
    }

    /// A reload of the plugin began as `load_id`. Only its declaration counts from now on: a
    /// reload still preparing (a git plugin's runs on its own repository thread) is cancelled,
    /// whichever finishes first.
    pub(crate) fn reload(&mut self, plugin_id: PluginId, load_id: u64) {
        if let Some(entry) = self.entries.get_mut(&plugin_id) {
            entry.load_id = load_id;
        }
    }

    /// The first load of the plugin waits for its repository before its declaration can be read
    /// (a git plugin): tell the screen it is loading, so that a launch of the same plugin finds
    /// it rather than loading another, and answer the launch with the plugin's id now rather
    /// than after the clone. Its pane, if it declares one, comes later without a completion.
    pub(crate) fn resolving(&mut self, plugin_id: PluginId, load_id: u64) -> Vec<PaneEffect> {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return vec![];
        };
        if entry.load_id != load_id {
            return vec![];
        }
        let PaneState::Pending {
            origin,
            request: Some(request),
            resolving,
            ..
        } = &mut entry.state
        else {
            return vec![];
        };
        let completion = match request {
            PaneRequest::Show(show) => show.completion.take().map(|mut completion| {
                completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
                completion
            }),
            PaneRequest::Parked { .. } => None,
            // The screen already holds the slot.
            PaneRequest::Placed { .. } => return vec![],
        };
        *resolving = true;
        vec![PaneEffect::Screen(ScreenInstruction::PluginResolving {
            plugin_id,
            run: Box::new(origin.run().clone()),
            completion,
        })]
    }

    /// The prepare step learned the plugin's declaration (`scanned`, already evaluated against
    /// its configuration). Decides whether it gets a pane and returns what to send.
    ///
    /// A `load_id` that is not the pending one changes nothing; the loader's own currency checks
    /// cancel that load.
    pub(crate) fn declare(
        &mut self,
        plugin_id: PluginId,
        load_id: u64,
        scanned: bool,
    ) -> (Prepared, Vec<PaneEffect>) {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return (Prepared::Proceed(Surface::Pane), vec![]);
        };
        if !matches!(entry.state, PaneState::Pending { .. }) {
            return (Prepared::Proceed(Self::surface_of(&entry.state)), vec![]);
        }
        if entry.load_id != load_id {
            return (Prepared::Proceed(Surface::Pane), vec![]);
        }
        entry.scanned = Some(scanned);
        self.resolve_pending(plugin_id, scanned)
    }

    /// A reload's prepare step learned the plugin's declaration, which may have changed.
    ///
    /// As with [`PaneLedger::declare`], a `load_id` that is not the latest changes nothing: two
    /// reloads preparing at once may read two versions of the source, and only the latest one's
    /// declaration is the plugin's.
    pub(crate) fn redeclare(
        &mut self,
        plugin_id: PluginId,
        load_id: u64,
        scanned: bool,
    ) -> (Prepared, Vec<PaneEffect>) {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return (Prepared::Proceed(Surface::Pane), vec![]);
        };
        if entry.load_id != load_id {
            return (Prepared::Proceed(Self::surface_of(&entry.state)), vec![]);
        }
        entry.scanned = Some(scanned);
        match &entry.state {
            PaneState::Pending {
                request: Some(_), ..
            } => self.resolve_pending(plugin_id, scanned),
            // The first load failed before its declaration was known, so the plugin has no pane
            // (a layout slot shows the error). A reload starts over: it is not a load that already
            // failed, and aborting it would leave the plugin unloadable until it is restarted.
            PaneState::Pending {
                request: None,
                origin,
                ..
            } => {
                let origin = origin.clone();
                if scanned {
                    let retry = match &mut entry.state {
                        PaneState::Pending { retry, .. } => retry.take(),
                        _ => None,
                    };
                    // The slot of a placed plugin is still there, showing the failure.
                    let place = match &origin {
                        Origin::Placed { .. } => Place::SlotExists,
                        _ => retry.map_or(Place::Floating, Place::AsRequested),
                    };
                    self.become_owned(plugin_id, origin, place)
                } else {
                    self.become_headless(plugin_id, origin)
                }
            },
            PaneState::Owned { origin } if !scanned => {
                let origin = origin.clone();
                self.become_headless(plugin_id, origin)
            },
            PaneState::Headless { origin, .. } if scanned => {
                let origin = origin.clone();
                self.become_owned(plugin_id, origin, Place::Floating)
            },
            // Still headless. The reload runs it again, so the screen stops counting it as
            // failed, should it have: a launch finds it running from now on.
            PaneState::Headless { origin, .. } => {
                let effects = match origin {
                    Origin::Placed { .. } => vec![],
                    _ => vec![paneless(plugin_id, origin.run(), None)],
                };
                (Prepared::Proceed(Surface::Headless), effects)
            },
            unchanged => (Prepared::Proceed(Self::surface_of(unchanged)), vec![]),
        }
    }

    /// A reload found no declaration for a plugin that had, or was to have, a pane.
    fn become_headless(
        &mut self,
        plugin_id: PluginId,
        origin: Origin,
    ) -> (Prepared, Vec<PaneEffect>) {
        let placed = matches!(origin, Origin::Placed { .. });
        let run = origin.run().clone();
        if let Some(entry) = self.entries.get_mut(&plugin_id) {
            entry.state = PaneState::Headless {
                origin,
                toasted: None,
            };
        }
        if placed {
            // A layout slot is no place for a headless plugin, as on a fresh load.
            let effects = self.retire_slot(plugin_id, &run, "layout pane");
            return (Prepared::Abort, effects);
        }
        (
            Prepared::Proceed(Surface::Headless),
            vec![paneless(plugin_id, &run, None)],
        )
    }

    /// A reload found a declaration for a plugin that had no pane, which goes to `place`.
    fn become_owned(
        &mut self,
        plugin_id: PluginId,
        origin: Origin,
        place: Place,
    ) -> (Prepared, Vec<PaneEffect>) {
        let add_plugin = match place {
            // Only animated: the screen still holds the slot the plugin was placed into.
            Place::SlotExists => None,
            Place::AsRequested(show) => Some(show.into_add_plugin(plugin_id)),
            Place::Floating => Some(match &origin {
                Origin::Parked {
                    run,
                    cwd,
                    client_id,
                } => parked_add_plugin(run.clone(), plugin_id, cwd.clone(), *client_id),
                // A plugin that ran headless is shown floating: the placement it was first asked
                // for (in place of a pane, in a slot) has moved on since.
                Origin::Show {
                    run,
                    cwd,
                    client_id,
                    tab,
                } => Self::floating(run.clone(), cwd.clone(), *client_id, *tab, plugin_id),
                Origin::Placed { run } => Self::floating(run.clone(), None, None, None, plugin_id),
            }),
        };
        if let Some(entry) = self.entries.get_mut(&plugin_id) {
            entry.state = PaneState::Owned { origin };
        }
        let effects = add_plugin
            .map(PaneEffect::Screen)
            .into_iter()
            .chain([PaneEffect::Background(BackgroundJob::AnimatePluginLoading(
                plugin_id,
            ))])
            .collect();
        (Prepared::Proceed(Surface::Pane), effects)
    }

    /// The load failed before its declaration was known: an unreadable source, an unresolved
    /// alias, a failed git resolution or a declaration the scan rejects. No pane is created; the
    /// failure goes where the request can see it.
    ///
    /// After the declaration is known this is [`PaneLedger::failure_effects`] for a load.
    pub(crate) fn fail(
        &mut self,
        plugin_id: PluginId,
        load_id: u64,
        error: &str,
    ) -> Vec<PaneEffect> {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return vec![];
        };
        if entry.load_id != load_id {
            return vec![];
        }
        let PaneState::Pending {
            origin,
            request,
            retry,
            resolving,
        } = &mut entry.state
        else {
            return self.failure_effects(
                plugin_id,
                error,
                Failure::Load {
                    load_id: Some(load_id),
                },
                true,
            );
        };
        // The screen knows it as loading, and must stop: it is not running.
        let stopped = std::mem::take(resolving).then_some(PaneEffect::Screen(
            ScreenInstruction::PluginFailed(plugin_id),
        ));
        let mut effects: Vec<PaneEffect> = stopped.into_iter().collect();
        effects.extend(match (request.take(), origin) {
            // A placed slot survives the request's failure and still needs reload errors.
            (None, Origin::Placed { run }) => painted_failure(plugin_id, loading_name(run), error),
            (request, _) => Self::failed_request(plugin_id, request, retry, error),
        });
        effects
    }

    /// What a load that failed before its declaration was known tells whoever asked for it.
    fn failed_request(
        plugin_id: PluginId,
        request: Option<PaneRequest>,
        retry: &mut Option<Box<ShowPane>>,
        error: &str,
    ) -> Vec<PaneEffect> {
        match request {
            Some(PaneRequest::Show(mut show)) => {
                let location = show.run.location_string();
                let completion = show.completion.take();
                let client_id = show.client_id;
                let reserved_slot = show.reserved_slot;
                // Nothing happened in the placement asked for, so a reload that finds the plugin
                // declares a pane places it there - unless it was in place of another pane, which
                // may be anything by then, or a slot, which is closed below.
                if !show.in_place && reserved_slot.is_none() {
                    // Not focused: the reload, not the user, is what brings the pane up.
                    show.should_focus = Some(false);
                    *retry = Some(show);
                }
                if let Some(completion) = completion {
                    refuse(completion, error.to_owned());
                }
                // A slot held for the pane that will not come is closed.
                let mut effects: Vec<PaneEffect> = reserved_slot
                    .map(|slot| PaneEffect::Screen(ScreenInstruction::GiveUpSlot(slot)))
                    .into_iter()
                    .collect();
                if client_id.is_some() {
                    effects.push(toast(
                        plugin_id,
                        format!(
                            "{location} failed to load: {} (yinz plugins log plugin_{plugin_id})",
                            summary(error)
                        ),
                    ));
                }
                effects
            },
            Some(PaneRequest::Placed { run }) => {
                painted_failure(plugin_id, loading_name(&run), error)
            },
            Some(PaneRequest::Parked { .. }) | None => vec![],
        }
    }

    /// A load or a running instance failed after the declaration was known. A pane shows the
    /// error as it always has; a headless plugin started on request toasts once per generation; a
    /// headless background plugin is reported only through the log and `plugins list`.
    ///
    /// `stopped` says no instance of the plugin runs any more (a load failure, or a crash of its
    /// last instance). The screen then hears that a headless plugin has stopped, so a launch of
    /// it starts another instead of finding this one.
    pub(crate) fn failure_effects(
        &mut self,
        plugin_id: PluginId,
        error: &str,
        failure: Failure,
        stopped: bool,
    ) -> Vec<PaneEffect> {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return vec![];
        };
        let headless_stopped = (stopped && matches!(entry.state, PaneState::Headless { .. }))
            .then_some(PaneEffect::Screen(ScreenInstruction::PluginFailed(
                plugin_id,
            )));
        let mut effects: Vec<PaneEffect> = headless_stopped.into_iter().collect();
        effects.extend(self.reported_failure(plugin_id, error, failure));
        effects
    }

    /// Where `failure_effects` shows a failure.
    fn reported_failure(
        &mut self,
        plugin_id: PluginId,
        error: &str,
        failure: Failure,
    ) -> Vec<PaneEffect> {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return vec![];
        };
        match &mut entry.state {
            PaneState::Pending { .. } => {
                let load_id = entry.load_id;
                self.fail(plugin_id, load_id, error)
            },
            PaneState::Owned { origin } => match failure {
                Failure::Load { .. } => {
                    painted_failure(plugin_id, loading_name(origin.run()), error)
                },
                // A running instance has no loading animation to stop, and stopping one could
                // cut short the animation of a reload under way.
                Failure::Crash { .. } => {
                    let mut loading_indication = LoadingIndication::new("Panic!".to_owned());
                    loading_indication.indicate_loading_error(error.to_owned());
                    vec![PaneEffect::Screen(
                        ScreenInstruction::UpdatePluginLoadingStage(plugin_id, loading_indication),
                    )]
                },
            },
            PaneState::Headless {
                origin: Origin::Show { run, .. },
                toasted,
            } => {
                if *toasted == Some(failure) {
                    return vec![];
                }
                *toasted = Some(failure);
                let what = match failure {
                    Failure::Load { .. } => "failed to load",
                    Failure::Crash { .. } => "crashed",
                };
                vec![toast(
                    plugin_id,
                    format!(
                        "{} {what}: {} (yinz plugins log plugin_{plugin_id})",
                        run.location_string(),
                        summary(error)
                    ),
                )]
            },
            PaneState::Headless { .. } => vec![],
        }
    }

    /// The plugin was unloaded. Its pending completion, if any, is answered with an error, and
    /// the screen forgets the plugin - except at shutdown, when the panes it still holds are
    /// about to be serialized for resurrection.
    pub(crate) fn retire(&mut self, plugin_id: PluginId, shutting_down: bool) -> Vec<PaneEffect> {
        let mut reserved_slot = None;
        if let Some(Entry {
            state:
                PaneState::Pending {
                    request: Some(PaneRequest::Show(show)),
                    ..
                },
            ..
        }) = self.entries.remove(&plugin_id)
        {
            let show = *show;
            reserved_slot = show.reserved_slot;
            if let Some(completion) = show.completion {
                refuse(
                    completion,
                    format!("plugin {plugin_id} was unloaded before it loaded"),
                );
            }
        }
        if shutting_down {
            return vec![];
        }
        let mut effects = vec![PaneEffect::Screen(ScreenInstruction::PluginUnloaded(
            plugin_id,
        ))];
        // A slot held for the pane that will not come is closed.
        if let Some(slot) = reserved_slot {
            effects.push(PaneEffect::Screen(ScreenInstruction::GiveUpSlot(slot)));
        }
        effects
    }

    /// `Some(Pane)` for a plugin that owns a pane, `Some(Headless)` for one that runs without,
    /// `None` while its declaration is unknown or for an unknown plugin.
    pub(crate) fn surface(&self, plugin_id: PluginId) -> Option<Surface> {
        match &self.entries.get(&plugin_id)?.state {
            PaneState::Pending { .. } => None,
            state => Some(Self::surface_of(state)),
        }
    }

    /// Whether the plugin has a pane, for `plugins list` and `GetPluginInfo`: `None` while
    /// undecided.
    pub(crate) fn presence(&self, plugin_id: PluginId) -> Option<bool> {
        self.surface(plugin_id)
            .map(|surface| surface == Surface::Pane)
    }

    /// Whether the ledger knows the plugin: it was loaded and not unloaded since.
    pub(crate) fn contains(&self, plugin_id: PluginId) -> bool {
        self.entries.contains_key(&plugin_id)
    }

    /// Whether the plugin's declaration is still unknown: its first load has not read it yet (a
    /// git plugin still resolving its repository) or failed before it could.
    pub(crate) fn pending(&self, plugin_id: PluginId) -> bool {
        self.entries
            .get(&plugin_id)
            .is_some_and(|entry| matches!(entry.state, PaneState::Pending { .. }))
    }

    /// Whether the plugin was started in the background.
    pub(crate) fn background(&self, plugin_id: PluginId) -> bool {
        self.entries
            .get(&plugin_id)
            .is_some_and(|entry| entry.background)
    }

    /// The raw scan result of the plugin's current load, for the eval check.
    pub(crate) fn scanned(&self, plugin_id: PluginId) -> Option<bool> {
        self.entries.get(&plugin_id)?.scanned
    }

    fn surface_of(state: &PaneState) -> Surface {
        match state {
            PaneState::Owned { .. } | PaneState::Pending { .. } => Surface::Pane,
            PaneState::Headless { .. } => Surface::Headless,
        }
    }

    fn floating(
        run: RunPluginOrAlias,
        cwd: Option<PathBuf>,
        client_id: Option<ClientId>,
        tab: Option<TabId>,
        plugin_id: PluginId,
    ) -> ScreenInstruction {
        ShowPane {
            should_float: Some(true),
            in_place: false,
            replaced: ReplacedPane::Suppress,
            run,
            title: None,
            tab,
            pane_id_to_replace: None,
            cwd,
            coordinates: None,
            should_focus: Some(false),
            client_id,
            completion: None,
            overlay: None,
            reserved_slot: None,
        }
        .into_add_plugin(plugin_id)
    }

    /// Detach an undeclared plugin from a slot it was placed into, explain it once per location,
    /// and unload it: a layout must not leak one headless instance per tab.
    fn retire_slot(
        &mut self,
        plugin_id: PluginId,
        run: &RunPluginOrAlias,
        what: &str,
    ) -> Vec<PaneEffect> {
        let location = run.location_string();
        let mut effects = vec![paneless(plugin_id, run, None)];
        if self.retired_slot_notices.insert(location.clone()) {
            effects.push(toast(
                plugin_id,
                format!("{location} declares no pane; its {what} was removed"),
            ));
        }
        effects.push(PaneEffect::Unload(plugin_id));
        effects
    }

    /// Answer the pending request of `plugin_id` now that its declaration is known.
    fn resolve_pending(
        &mut self,
        plugin_id: PluginId,
        scanned: bool,
    ) -> (Prepared, Vec<PaneEffect>) {
        let Some(entry) = self.entries.get_mut(&plugin_id) else {
            return (Prepared::Proceed(Surface::Pane), vec![]);
        };
        let PaneState::Pending {
            origin, request, ..
        } = &mut entry.state
        else {
            return (Prepared::Proceed(Self::surface_of(&entry.state)), vec![]);
        };
        let origin = origin.clone();
        // A failure already answered this load.
        let Some(request) = request.take() else {
            return (Prepared::Abort, vec![]);
        };
        let animate = PaneEffect::Background(BackgroundJob::AnimatePluginLoading(plugin_id));
        if scanned {
            let add_plugin = match request {
                PaneRequest::Show(show) => Some(show.into_add_plugin(plugin_id)),
                PaneRequest::Parked {
                    run,
                    cwd,
                    client_id,
                } => Some(parked_add_plugin(run, plugin_id, cwd, client_id)),
                // The screen already holds the slot.
                PaneRequest::Placed { .. } => None,
            };
            entry.state = PaneState::Owned { origin };
            let effects = add_plugin
                .map(PaneEffect::Screen)
                .into_iter()
                .chain([animate])
                .collect();
            return (Prepared::Proceed(Surface::Pane), effects);
        }
        entry.state = PaneState::Headless {
            origin,
            toasted: None,
        };
        match request {
            PaneRequest::Show(show) => {
                let show = *show;
                let location = show.run.location_string();
                match show.reserved_slot {
                    Some(slot) => {
                        let mut effects = self.retire_slot(plugin_id, &show.run, "pane");
                        effects.insert(1, PaneEffect::Screen(ScreenInstruction::GiveUpSlot(slot)));
                        // A refusal: nothing the launcher does next depends on the screen.
                        if let Some(completion) = show.completion {
                            refuse(completion, format!("{location} declares no pane"));
                        }
                        (Prepared::Abort, effects)
                    },
                    None => {
                        // Answered by the screen once it has recorded the plugin as headless, so
                        // a launch that follows this one finds it running.
                        let completion = show
                            .completion
                            .map(|completion| answered_headless(plugin_id, &location, completion));
                        (
                            Prepared::Proceed(Surface::Headless),
                            vec![paneless(plugin_id, &show.run, completion)],
                        )
                    },
                }
            },
            PaneRequest::Parked { run, .. } => (
                Prepared::Proceed(Surface::Headless),
                vec![paneless(plugin_id, &run, None)],
            ),
            PaneRequest::Placed { run } => (
                Prepared::Abort,
                self.retire_slot(plugin_id, &run, "layout pane"),
            ),
        }
    }
}

#[cfg(test)]
#[path = "unit/plugin_panes.rs"]
mod tests;
