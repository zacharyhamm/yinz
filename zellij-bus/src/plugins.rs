use std::str::FromStr;
use zellij_types::internal_ids::QueueJobId;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TerminalId;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use crate::route::NotificationEnd;
use crate::session_layout_metadata::SessionLayoutMetadata;
use crate::ClientId;
use crate::PaneId;
use zellij_types::data::PaneRenderReport;
use zellij_types::input::context_menu::ContextMenu;
use zellij_types::input::layout::TabLayoutInfo;
pub use zellij_types::internal_ids::PluginId;
use {zellij_types::input::hooks::HookPoint, zellij_types::input::hooks::Hooks};

use zellij_runtime::errors::prelude::*;
use zellij_types::diagnostics::PluginContext;
use zellij_types::geometry::FloatingPaneCoordinates;
use zellij_types::pane_size::Size;
use {
    zellij_types::data::CommandOrPlugin, zellij_types::data::ContextMenuContext,
    zellij_types::data::Event, zellij_types::data::EventType, zellij_types::data::HookPayload,
    zellij_types::data::InputMode, zellij_types::data::LayoutInfo,
    zellij_types::data::LayoutWithError, zellij_types::data::MessageToPlugin,
    zellij_types::data::OverlayPlacement, zellij_types::data::PermissionStatus,
    zellij_types::data::PermissionType, zellij_types::input::actions::Action,
    zellij_types::input::command::TerminalAction, zellij_types::input::keybinds::Keybinds,
    zellij_types::input::layout::FloatingPaneLayout, zellij_types::input::layout::RunPluginOrAlias,
    zellij_types::input::layout::TiledPaneLayout, zellij_types::input::plugins::PluginAliases,
};

#[derive(Clone, Debug)]
pub struct DumpSessionLayoutResponse {
    pub layout_result: Result<String, String>,
    pub metadata: Option<zellij_types::data::LayoutMetadata>,
}

/// The plugin id a state report is addressed to when no plugin wants it but a `zellij events`
/// client does. No plugin ever has this id, so the plugin thread loads nothing for it and the
/// report reaches the CLI subscribers alone. See `WasmBridge::forward_events_to_cli_subscribers`.
pub const CLI_EVENTS_ONLY_TARGET: PluginId = PluginId::new(u32::MAX);

#[derive(Clone, Debug, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum PluginInstruction {
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
    DenoTick(PluginId, ClientId, u64),
    DenoEvent(PluginId, ClientId, u64, Event),
    DenoDispose(PluginId, u64),
    Unload(PluginId),                                         // plugin_id
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
    AddClient(ClientId),
    RemoveClient(ClientId),
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
        Vec<(TabId, TabLayoutInfo)>, // resolved ID and layout for each tab
        bool,                        // retain_existing_terminal_panes
        bool,                        // retain_existing_plugin_panes
        ClientId,
        Option<NotificationEnd>,
    ),
    ApplyCachedEvents {
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
    PluginSubscribedToEvents(PluginId, ClientId, HashSet<EventType>, bool), // is_background
    /// A `zellij events` client asking for the session's state events, by name.
    SubscribeCliClientToEvents {
        client_id: ClientId,
        event_types: Vec<String>,
    },
    PermissionRequestResult(
        PluginId,
        Option<ClientId>,
        Vec<PermissionType>,
        PermissionStatus,
        Option<PathBuf>,
    ),
    DumpLayout(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    ListClientsMetadata(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    DumpLayoutToPlugin {
        session_layout_metadata: SessionLayoutMetadata,
        plugin_id: PluginId,
        response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
    },
    LogLayoutToHd(SessionLayoutMetadata),
    CliPipe {
        pipe_id: String,
        name: String,
        payload: Option<String>,
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
    UnblockCliPipes(Vec<PluginRenderAsset>),
    Reconfigure {
        client_id: ClientId,
        keybinds: Option<Keybinds>,
        default_mode: Option<InputMode>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
        builtin_plugins_dir: Option<PathBuf>,
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
    },
    WatchFilesystem,
    ListClientsToPlugin(SessionLayoutMetadata, PluginId, ClientId),
    ChangePluginHostDir(PathBuf, PluginId, ClientId),
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
    GetLastSessionSaveTime {
        response_channel: crossbeam::channel::Sender<Option<u64>>,
    },
    DetectPluginConfigChanges(PluginAliases),
    /// Something was put on `queue` (by a plugin, straight into the shared `Queues`): start
    /// whatever it now has room for.
    QueuePump {
        queue: String,
    },
    QueueConfigure {
        queue: String,
        concurrency: usize,
    },
    QueueCancelJob {
        job_id: QueueJobId,
    },
    /// A job (`plugin_id`) reported its outcome and wants to be unloaded.
    QueueFinishJob {
        plugin_id: PluginId,
        success: bool,
        payload: Option<String>,
    },
    /// Grant permissions to every plugin at a location, running or not, as if the user had
    /// answered its prompt. Sent only on behalf of a builtin - see `grant_plugin_permissions`
    /// in `zellij_exports`.
    GrantPermissionsToLocation {
        plugin_url: String,
        permissions: Vec<PermissionType>,
    },
    /// A plugin failed to load or trapped. Deno reports are validated against their instance
    /// generation before painting errors or disposing it. Failed jobs also release their slot.
    PluginFailed {
        plugin_id: PluginId,
        /// None for legacy runtimes and load failures that precede instance creation.
        deno_generation: Option<u64>,
        error: String,
        /// True when the load itself failed, so the loader has already run and given up; false
        /// when a plugin that was loaded trapped. The two need different cleanup - see
        /// `WasmBridge::queue_plugin_failed`.
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
    /// Async completions belong to the Deno instance that started the operation.
    /// Wasm and Lua retain their existing event delivery protocol.
    pub fn for_plugin_event(
        plugin_id: PluginId,
        client_id: ClientId,
        deno_generation: Option<u64>,
        event: Event,
    ) -> Self {
        match deno_generation {
            Some(generation) => Self::DenoEvent(plugin_id, client_id, generation, event),
            None => Self::Update(vec![(Some(plugin_id), Some(client_id), event)]),
        }
    }
}

impl From<&PluginInstruction> for PluginContext {
    fn from(plugin_instruction: &PluginInstruction) -> Self {
        // The two enums are kept name-for-name; `instruction_context_tests` fails the test run if
        // a variant is added without either a matching `PluginContext` or an arm here.
        match plugin_instruction {
            PluginInstruction::DenoTick(..) => PluginContext::Update,
            PluginInstruction::DenoEvent(..) => PluginContext::Update,
            PluginInstruction::DenoDispose(..) => PluginContext::Unload,
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

#[derive(Debug, Clone)]
pub struct PluginRenderAsset {
    // TODO: naming
    pub client_id: ClientId,
    pub plugin_id: PluginId,
    pub bytes: Vec<u8>,
    pub cli_pipes: HashMap<String, PipeStateChange>,
}

impl Default for PluginRenderAsset {
    fn default() -> Self {
        Self {
            client_id: ClientId::new(0),
            plugin_id: PluginId::new(0),
            bytes: Vec::new(),
            cli_pipes: HashMap::new(),
        }
    }
}

impl PluginRenderAsset {
    pub fn new(plugin_id: PluginId, client_id: ClientId, bytes: Vec<u8>) -> Self {
        PluginRenderAsset {
            client_id,
            plugin_id,
            bytes,
            ..Default::default()
        }
    }
    pub fn with_pipes(mut self, cli_pipes: HashMap<String, PipeStateChange>) -> Self {
        self.cli_pipes = cli_pipes;
        self
    }
}

#[derive(Debug, Clone)]
pub enum PipeStateChange {
    NoChange,
    Block,
    Unblock,
}
