use std::str::FromStr;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use crate::route::NotificationEnd;
use crate::session_layout_metadata::SessionLayoutMetadata;
use crate::ClientId;
use crate::PaneId;
use yinz_types::data::PaneRenderReport;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::layout::TabLayoutInfo;
pub use yinz_types::internal_ids::PluginId;
use {yinz_types::input::hooks::HookPoint, yinz_types::input::hooks::Hooks};

use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::PluginContext;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::pane_size::Size;
use {
    yinz_types::data::CommandOrPlugin, yinz_types::data::ContextMenuContext,
    yinz_types::data::Event, yinz_types::data::EventType, yinz_types::data::HookPayload,
    yinz_types::data::InputMode, yinz_types::data::LayoutInfo, yinz_types::data::LayoutWithError,
    yinz_types::data::MessageToPlugin, yinz_types::data::OverlayPlacement,
    yinz_types::data::PermissionStatus, yinz_types::data::PermissionType,
    yinz_types::input::actions::Action, yinz_types::input::command::TerminalAction,
    yinz_types::input::keybinds::Keybinds, yinz_types::input::layout::FloatingPaneLayout,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::layout::TiledPaneLayout,
    yinz_types::input::plugins::PluginAliases,
};

#[derive(Clone, Debug)]
pub struct DumpSessionLayoutResponse {
    pub layout_result: Result<String, String>,
    pub metadata: Option<yinz_types::data::LayoutMetadata>,
}

/// Bounded progress retained by the synchronous caller even when a downstream thread stalls.
#[derive(Clone, Debug)]
pub struct LayoutQueryTrace {
    started: std::time::Instant,
    stages: std::sync::Arc<
        std::sync::Mutex<std::collections::VecDeque<(&'static str, std::time::Duration)>>,
    >,
}

impl Default for LayoutQueryTrace {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            stages: Default::default(),
        }
    }
}

impl LayoutQueryTrace {
    pub fn mark(&self, stage: &'static str) {
        let mut stages = self
            .stages
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if stages.len() == 16 {
            // Keep the six routing stages when a layout contains many plugin lookups.
            stages.remove(6);
        }
        stages.push_back((stage, self.started.elapsed()));
    }
}

#[cfg(test)]
#[path = "unit/layout_query_trace_tests.rs"]
mod layout_query_trace_tests;

/// The plugin id a state report is addressed to when no plugin wants it but a `yinz events`
/// client does. No plugin ever has this id, so the plugin thread loads nothing for it and the
/// report reaches the CLI subscribers alone. See `PluginBridge::forward_events_to_cli_subscribers`.
pub const CLI_EVENTS_ONLY_TARGET: PluginId = PluginId::new(u32::MAX);

#[derive(Clone, Debug, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum PluginInstruction {
    PluginRequest {
        request: yinz_types::cli::PluginRequest,
        client_id: ClientId,
        reply: std::sync::mpsc::Sender<Result<yinz_types::cli::PluginResponse, String>>,
    },
    ReconcileBackgroundPlugins {
        plugins: std::collections::BTreeSet<RunPluginOrAlias>,
        client_id: ClientId,
    },
    DataRequest {
        request: yinz_types::cli::DataRequest,
        reply: std::sync::mpsc::Sender<Result<String, String>>,
    },
    RefreshDataWatches {
        namespace: String,
        entities: std::collections::BTreeSet<String>,
    },
    LoadPaneTab {
        skip_cache: bool,
        id: yinz_types::pane_tabs::PaneTabsId,
        index: usize,
        generation: u64,
        descriptor: yinz_types::pane_tabs::PaneTabDescriptor,
        client_id: ClientId,
        tab_id: TabId,
        size: Size,
    },
    Load(
        Option<bool>,   // should float
        bool,           // should be opened in place
        bool,           // close_replaced_pane
        Option<String>, // pane title
        RunPluginOrAlias,
        Option<TabId>,  // tab index
        Option<PaneId>, // pane id to replace if this is to be opened "in-place"
        ClientId,
        Size,
        Option<PathBuf>,  // cwd
        Option<PluginId>, // the focused plugin id if relevant
        bool,             // skip cache
        Option<bool>,     // should focus plugin
        Option<FloatingPaneCoordinates>,
        Option<NotificationEnd>, // completion signal
        // the overlay placement the launcher stated outright, rather than the one derived from
        // the plugin's own `overlay`/`modal` configuration
        Option<OverlayPlacement>,
    ),
    LoadBackgroundPlugin(RunPluginOrAlias, ClientId),
    /// A hook point was reached. What happens to the plugin bound to it - if any - is the
    /// point's [`HookShape`]: a `ReplacePane` point opens it in place of `pane_id`, which the
    /// screen has already put in a state it can be replaced from; a `Headless` point hands the
    /// payload to a long-lived instance as `Event::Hook`.
    ///
    /// Sent unconditionally by whoever reaches the hook point, so that the config lives in one
    /// place: an unbound point is a no-op here rather than a check at every call site.
    RunHook {
        point: HookPoint,
        /// The pane the hook is about - and for a `ReplacePane` point, the pane it replaces.
        /// `None` for a point that is not about a pane, such as `session_start`.
        pane_id: Option<PaneId>,
        /// What the plugin is handed, in the point's shape.
        payload: HookFiring,
        client_id: ClientId,
    },
    /// A right-click asked for a context menu. The plugin bound to the `context_menu` hook point
    /// is sent `Event::ContextMenuRequested` and answers with `set_context_menu`.
    ///
    /// Unlike `RunHook` this opens nothing: the menu is drawn by the screen and the plugin only
    /// says what is in it. And unlike `RunHook` it always answers, even with no hook bound - the
    /// screen is holding an empty menu open waiting to hear, so silence here would be a menu that
    /// only the timeout resolves.
    RequestContextMenu {
        context: Box<ContextMenuContext>,
        client_id: ClientId,
    },
    Update(Vec<(Option<PluginId>, Option<ClientId>, Event)>), // Focused plugin / broadcast, client_id, event data
    InputBarrier(PluginId, NotificationEnd),
    PluginTick(PluginId, ClientId, u64),
    PluginPalette(crate::command_palette::Delivery),
    PaletteAgent(Box<crate::command_palette::AgentRequest>),
    PluginEvent(PluginId, ClientId, u64, Event),
    PluginReply(
        PluginId,
        ClientId,
        u64,
        Event,
        crate::thread_bus::PluginReplyId,
    ),
    PluginDispose(PluginId, u64),
    Unload(PluginId), // plugin_id
    Reload(
        Option<bool>,   // should float
        Option<String>, // pane title
        RunPluginOrAlias,
        TabId, // tab index
        Size,
        Option<NotificationEnd>,
    ),
    ReloadPluginWithId(PluginId),
    /// Reload every running instance at a location in place; nothing if none is running.
    ReloadPluginByUrl(RunPluginOrAlias),
    Resize(PluginId, usize, usize), // plugin_id, columns, rows
    /// Register an attaching client's attachment token before anything can be loaded for it:
    /// the server sends it before the client counts as attached, so that a `Load` for the
    /// client always comes after it. Its `RemoveClient` cancels and forgets the token.
    ReserveClient(ClientId),
    AddClient(ClientId),
    RemoveClient(ClientId),
    CliPipesDisconnected(Vec<String>),
    NewTab(
        Option<PathBuf>,
        Option<TerminalAction>,
        Option<TiledPaneLayout>,
        Option<Vec<FloatingPaneLayout>>, // None inherits defaults; Some(empty) disables them
        TabId,                           // tab_id
        Option<Vec<CommandOrPlugin>>,    // initial_panes
        bool,                            // block_on_first_terminal
        bool,                            // should change focus to new tab
        ClientId,
        Option<NotificationEnd>, // completion signal
    ),
    OverrideLayout(
        Option<PathBuf>,             // cwd
        Option<TerminalAction>,      // default_shell
        Vec<TabId>,                  // surplus tabs to close after successful application
        Vec<(TabId, TabLayoutInfo)>, // resolved ID and layout for each tab
        bool,                        // retain_existing_terminal_panes
        bool,                        // retain_existing_plugin_panes
        ClientId,
        Option<NotificationEnd>,
    ),
    ApplyCachedEvents {
        load_id: u64,
        plugin_ids: Vec<PluginId>,
        done_receiving_permissions: bool,
    },
    ApplyCachedWorkerMessages(PluginId),
    PostMessagesToPluginWorker(
        PluginId,
        ClientId,
        String, // worker name
        Vec<(
            String, // serialized message name
            String, // serialized payload
        )>,
    ),
    PostMessageToPlugin(
        PluginId,
        ClientId,
        String, // serialized message
        String, // serialized payload
    ),
    /// Somebody clicked a toast that asked to hear about it. Two clients, because the one id
    /// `PostMessageToPlugin` carries cannot answer both halves of this - see
    /// [`client_for_notification_click`].
    NotificationClicked {
        plugin_id: PluginId,
        /// The client whose instance raised the toast.
        raiser: ClientId,
        /// The client who clicked it. The same client, nearly always.
        clicker: ClientId,
        message: String,
        payload: String,
    },
    /// Newly requested events and, for background plugins, their complete current subscriptions.
    PluginSubscribedToEvents(
        PluginId,
        ClientId,
        HashSet<EventType>,
        Option<HashSet<EventType>>,
    ),
    /// A `yinz events` client asking for the session's state events, by name.
    ResolveAgentEventProjects {
        version: u64,
        agents: Vec<yinz_types::agent::control::AgentStatus>,
        projects: Vec<String>,
    },
    SubscribeCliClientToEvents {
        client_id: ClientId,
        event_types: Vec<String>,
    },
    RequestPermissions {
        plugin_id: PluginId,
        instance_id: PluginInstanceId,
        client_id: ClientId,
        permissions: Vec<PermissionType>,
    },
    PermissionAnswer {
        request_id: u64,
        status: PermissionStatus,
        cache_path: Option<PathBuf>,
    },
    DumpLayout(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    ListClientsMetadata(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    DumpLayoutToPlugin {
        session_layout_metadata: SessionLayoutMetadata,
        plugin_id: PluginId,
        response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
        trace: LayoutQueryTrace,
    },
    LogLayoutToHd(SessionLayoutMetadata),
    CliPipe {
        focus_plugin: bool,
        pipe_id: String,
        name: String,
        payload: Option<String>,
        /// A payload-less message on a pipe already used by this CLI connection.
        is_eof: bool,
        plugin: Option<String>,
        args: Option<BTreeMap<String, String>>,
        configuration: Option<BTreeMap<String, String>>,
        floating: Option<bool>,
        pane_id_to_replace: Option<PaneId>,
        pane_title: Option<String>,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        cli_client_id: ClientId,
        /// The pane the pipe was run in, if the CLI client knew it.
        source_pane_id: Option<PaneId>,
    },
    ArrangePanesByProject,
    KeybindPipe {
        name: String,
        payload: Option<String>,
        plugin: Option<String>,
        args: Option<BTreeMap<String, String>>,
        configuration: Option<BTreeMap<String, String>>,
        floating: Option<bool>,
        /// Replace the client's focused pane when `pane_id_to_replace` names none. A keybind
        /// is routed without a pane id, so this is how its `in_place` reaches the host.
        should_open_in_place: bool,
        pane_id_to_replace: Option<PaneId>,
        pane_title: Option<String>,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        cli_client_id: ClientId,
        plugin_and_client_id: Option<(PluginId, ClientId)>,
        notification_end: Option<NotificationEnd>,
    },
    CachePluginEvents {
        plugin_id: PluginId,
    },
    MessageFromPlugin {
        source_plugin_id: PluginId,
        message: MessageToPlugin,
    },
    PipeCompleted(PipeCompletion),
    ReleasePluginInstancePipes(PluginId, PluginInstanceId),
    Reconfigure {
        plugin_aliases: yinz_types::input::plugins::PluginAliases,
        client_id: ClientId,
        keybinds: Option<Keybinds>,
        default_mode: Option<InputMode>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
        /// The hook bindings the new configuration declares. Held by this thread rather than
        /// re-read per hook, so a reload has to hand them over or every hook point keeps whatever
        /// the session started with.
        hooks: Hooks,
        /// Likewise the configured context menu items, which `request_context_menu` answers from
        /// when no hook is bound or the hook is not yet loaded.
        context_menu: ContextMenu,
        was_written_to_disk: bool,
    },
    FailedToWriteConfigToDisk {
        file_path: Option<PathBuf>,
        /// Why, when the server knows: the serializer refused an action, the written form
        /// differed from the source, or the filesystem said no.
        reason: Option<String>,
    },
    ListClientsToPlugin(SessionLayoutMetadata, PluginId, ClientId, Option<u64>),
    ChangePluginHostDir(PathBuf, PluginId, ClientId, Option<u64>),
    PaneRenderReport(PaneRenderReport),
    UserInput {
        client_id: ClientId,
        action: Action,
        terminal_id: Option<TerminalId>,
        cli_client_id: Option<ClientId>,
    },
    LayoutListUpdate(Vec<LayoutInfo>, Vec<LayoutWithError>),
    RequestStateUpdateForPlugin(PluginId),
    UpdateSessionSaveTime(u64), // u64 = milliseconds since UNIX epoch
    Pick {
        plugin_id: PluginId,
        instance_id: PluginInstanceId,
        generation: Option<u64>,
        client_id: ClientId,
        cwd: std::path::PathBuf,
        focused_plugin: Option<PluginId>,
        command: yinz_types::pick::PickCommand,
        response: crossbeam::channel::Sender<Result<Option<yinz_types::pick::PickRequest>, String>>,
    },
    PickRetired(PluginId),
    /// Load `plugin_url` in place of `pane_id`, which the screen has found holding a slot in
    /// `tab_id`, lending the new pane that slot. Refused when an instance with this url and
    /// configuration already exists, rather than quietly doing nothing with it; the reason,
    /// or the new pane, goes to `completion`.
    OpenPluginPaneInPlaceOfPaneId {
        pane_id: PaneId,
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        cwd: PathBuf,
        tab_id: TabId,
        client_id: ClientId,
        completion: NotificationEnd,
    },
    GetPluginIdsForUrl {
        url: String,
        configuration: Option<std::collections::BTreeMap<String, String>>,
        response_channel: crossbeam::channel::Sender<Result<Vec<PluginId>, String>>,
    },
    GetLastSessionSaveTime {
        response_channel: crossbeam::channel::Sender<Option<u64>>,
    },
    DetectPluginConfigChanges(PluginAliases),
    /// Grant permissions to every plugin at a location, running or not, as if the user had
    /// answered its prompt. Sent only on behalf of a builtin - see `grant_plugin_permissions`
    /// in `yinz_exports`.
    GrantPermissionsToLocation {
        plugin_url: String,
        permissions: Vec<PermissionType>,
    },
    /// A plugin failed to load or trapped. Runtime reports are validated against their instance
    /// generation before painting errors or disposing it.
    PluginFailed {
        load_id: Option<u64>,
        plugin_id: PluginId,
        /// None for legacy runtimes and load failures that precede instance creation.
        generation: Option<u64>,
        instance_id: Option<PluginInstanceId>,
        loading_client: Option<ClientId>,
        error: String,
        /// True when the load itself failed, so the loader has already run and given up; false
        /// when a plugin that was loaded trapped. The two need different cleanup - see
        /// `PluginBridge::plugin_failed`.
        failed_to_load: bool,
    },
    HighlightClicked {
        plugin_id: PluginId,
        client_id: ClientId,
        pane_id: PaneId,
        pattern: String,
        matched_string: String,
        context: BTreeMap<String, String>,
    },
    Exit,
}

impl PluginInstruction {
    /// Async completions belong to the runtime instance that started the operation.
    pub fn for_plugin_event(
        plugin_id: PluginId,
        client_id: ClientId,
        generation: Option<u64>,
        event: Event,
    ) -> Self {
        match generation {
            Some(generation) => Self::PluginEvent(plugin_id, client_id, generation, event),
            None => Self::Update(vec![(Some(plugin_id), Some(client_id), event)]),
        }
    }
}

impl From<&PluginInstruction> for PluginContext {
    fn from(plugin_instruction: &PluginInstruction) -> Self {
        // The two enums are kept name-for-name; `instruction_context_tests` fails the test run if
        // a variant is added without either a matching `PluginContext` or an arm here.
        match plugin_instruction {
            PluginInstruction::DataRequest { .. }
            | PluginInstruction::RefreshDataWatches { .. } => PluginContext::Update,
            PluginInstruction::InputBarrier(..) => PluginContext::Update,
            PluginInstruction::PaletteAgent(..) | PluginInstruction::PluginPalette(..) => {
                PluginContext::Update
            },
            PluginInstruction::PluginTick(..) => PluginContext::Update,
            PluginInstruction::PluginEvent(..) | PluginInstruction::PluginReply(..) => {
                PluginContext::Update
            },
            PluginInstruction::PluginDispose(..) => PluginContext::Unload,
            PluginInstruction::PipeCompleted(..) => PluginContext::UnblockCliPipes,
            PluginInstruction::ReleasePluginInstancePipes(..) => PluginContext::UnblockCliPipes,
            PluginInstruction::PostMessagesToPluginWorker(..) => {
                PluginContext::PostMessageToPluginWorker
            },
            other => {
                let name: &'static str = other.into();
                PluginContext::from_str(name).unwrap_or_else(|_| {
                    debug_assert!(false, "no PluginContext variant named {name}");
                    PluginContext::Exit
                })
            },
        }
    }
}

/// What a hook point hands its plugin, in the point's shape: a `ReplacePane` plugin receives
/// a string map as its configuration, which is what `load(config)` receives; a `Headless` one
/// receives a typed [`HookPayload`] as `Event::Hook { payload }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookFiring {
    ReplacePane(BTreeMap<String, String>),
    Headless(HookPayload),
}

/// Runtime identity, independent of the client an instance is currently keyed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginInstanceId(u64);

impl Default for PluginInstanceId {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone)]
pub struct PluginRenderAsset {
    pub client_id: ClientId,
    pub plugin_id: PluginId,
    pub bytes: Vec<u8>,
}

impl Default for PluginRenderAsset {
    fn default() -> Self {
        Self {
            client_id: ClientId::NONE,
            plugin_id: PluginId::new(0),
            bytes: Vec::new(),
        }
    }
}

impl PluginRenderAsset {
    pub fn new(plugin_id: PluginId, client_id: ClientId, bytes: Vec<u8>) -> Self {
        PluginRenderAsset {
            client_id,
            plugin_id,
            bytes,
        }
    }
}

/// One input message, independent of the CLI stream it arrived on.
pub type PipeRequestId = u64;

#[derive(Debug, Clone)]
pub struct PipeCompletion {
    pub plugin_id: PluginId,
    pub instance_id: PluginInstanceId,
    pub changes: HashMap<PipeRequestId, PipeStateChange>,
    pub callback: Option<(PipeRequestId, Result<(), String>)>,
}

#[derive(Debug, Clone)]
pub enum PipeStateChange {
    NoChange,
    Block,
    Unblock,
}
