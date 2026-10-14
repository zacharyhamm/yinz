use std::collections::HashMap;
use std::str::FromStr;

use crate::route::NotificationEnd;
use crate::{ClientId, PaneId, PluginId};
use yinz_config::config::Config;
use yinz_runtime::errors::ErrorInstruction;
use yinz_types::data::{InputMode, SessionConnection};
use yinz_types::diagnostics::ServerContext;
use yinz_types::geometry::Direction;
use yinz_types::input::actions::Action;
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::internal_ids::TabNumber;
use yinz_types::keys::KeyWithModifier;
use yinz_types::pane_size::Size;

/// Instructions related to server-side application
#[derive(Debug, Clone, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum ServerInstruction {
    FirstClientConnected(CliAssets, ClientId),
    Render(Option<HashMap<ClientId, String>>),
    UnblockInputThread,
    ClientExit(ClientId, Option<NotificationEnd>),
    RemoveClient(ClientId),
    /// The router has queued its last instruction; its connection ID can be reused.
    ClientRouterFinished(ClientId),
    Error(String),
    KillSession,
    KillSessionWithCompletion(ClientId),
    DetachSession(Vec<ClientId>, Option<NotificationEnd>),
    AttachClient(
        CliAssets,
        Option<TabNumber>, // tab number to focus
        Option<PaneId>,    // pane to focus
        ClientId,
    ),
    AttachWatcherClient(ClientId, Size),
    ConnStatus(ClientId),
    Log(Vec<String>, ClientId, Option<NotificationEnd>),
    LogError(Vec<String>, ClientId, Option<NotificationEnd>),
    SwitchSession(SessionConnection, ClientId, Option<NotificationEnd>),
    UnblockCliPipeInput(String),   // String -> Pipe name
    CliPipeOutput(String, String), // String -> Pipe name, String -> Output
    CliPipeError(String, String),  // String -> Pipe name, String -> Error
    /// One event for a `yinz events` client: its type by name, and the event as JSON.
    EventUpdateToClient(ClientId, String, String),
    AssociatePipeWithClient {
        pipe_id: String,
        client_id: ClientId,
    },
    DisconnectAllClientsExcept(ClientId),
    GetDisconnectTargets(ClientId, crossbeam::channel::Sender<Vec<ClientId>>),
    ChangeMode(ClientId, InputMode, Option<NotificationEnd>),
    SetCommandPaletteOpen(ClientId, bool),
    ConfigurationEditor {
        theme: Option<String>,
        edit: Option<yinz_types::configuration::Edit>,
        reply: crossbeam::channel::Sender<Result<serde_json::Value, String>>,
    },
    SavePaletteTheme {
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        client: ClientId,
        request: uuid::Uuid,
        name: String,
        permanent: bool,
    },
    ResolveLockedEscape(ClientId, uuid::Uuid),
    ChangeModeForAllClients(InputMode),
    Reconfigure {
        client_id: ClientId,
        config: String,
        write_config_to_disk: bool,
    },
    /// `yinz action reconfigure`: the same fragment, for every attached client rather than
    /// the one that sent it, which is a CLI client about to exit.
    ReconfigureAllClients {
        config: String,
        write_config_to_disk: bool,
        completion: Option<NotificationEnd>,
    },
    ConfigWrittenToDisk(Config),
    RebindKeys {
        client_id: ClientId,
        keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
        keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
        write_config_to_disk: bool,
        plugin_id: Option<PluginId>,
    },
    ClearMouseHelpText(ClientId),
    ClearCommandOutputFlash(PaneId),
    /// Relay a forwarded-query dispatch from Screen to the server main
    /// loop. The main loop writes `ServerToClientMsg::ForwardQueryToHost`
    ForwardQueryToHost(u32, Vec<u8>, bool),
    KeyPassthroughChanged(ClientId, PaneId, PaneId, bool, Option<Direction>, bool),
    EmitNestedSessionFrameToClient(ClientId, Vec<u8>),
    /// Preserve geometry received before Screen exists, without waiting for another input.
    TerminalPixelDimensions {
        client_id: ClientId,
        pixel_dimensions: yinz_ipc::ipc::PixelDimensions,
    },
    /// Keep the startup graphics answer ordered after client registration and before removal,
    /// just like `SynchronizedOutputSupport`.
    KittyGraphicsSupport {
        client_id: ClientId,
        supported: bool,
    },
    /// Whether this client's host terminal presents each frame atomically (DECSET 2026).
    ///
    /// It comes the long way round, through this loop rather than straight from the router to
    /// Screen, so that it cannot overtake the client's own `AttachClient` nor be overtaken by its
    /// `RemoveClient` - client ids are recycled, and an answer landing on the wrong side of
    /// either would be attributed to the wrong terminal. Nothing re-sends it.
    SynchronizedOutputSupport {
        client_id: ClientId,
        supported: bool,
    },
}

impl From<&ServerInstruction> for ServerContext {
    fn from(server_instruction: &ServerInstruction) -> Self {
        // The two enums are kept name-for-name; `instruction_context_tests` fails the test run if
        // a variant is added without either a matching `ServerContext` or an arm here.
        match server_instruction {
            ServerInstruction::FirstClientConnected(..) => ServerContext::NewClient,
            ServerInstruction::ClientRouterFinished(..) => ServerContext::RemoveClient,
            ServerInstruction::AttachWatcherClient(..) => ServerContext::AttachClient,
            ServerInstruction::ResolveLockedEscape(..) => ServerContext::ChangeMode,
            ServerInstruction::ConfigurationEditor { .. } => ServerContext::Reconfigure,
            ServerInstruction::SavePaletteTheme { .. } => ServerContext::Reconfigure,
            ServerInstruction::SetCommandPaletteOpen(..) => ServerContext::ChangeMode,
            ServerInstruction::KillSessionWithCompletion(..) => ServerContext::KillSession,
            ServerInstruction::CliPipeError(..) => ServerContext::LogError,
            other => {
                let name: &'static str = other.into();
                ServerContext::from_str(name).unwrap_or_else(|_| {
                    debug_assert!(false, "no ServerContext variant named {name}");
                    ServerContext::Error
                })
            },
        }
    }
}

impl ErrorInstruction for ServerInstruction {
    fn error(err: String) -> Self {
        ServerInstruction::Error(err)
    }
}
