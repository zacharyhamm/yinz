use crate::input::actions::{Action, RunCommandAction};
use crate::input::command::RunCommand;
use crate::input::hooks::HookPoint;
use crate::input::keybinds::Keybinds;
use crate::input::layout::{
    FloatingPaneLayout, Run, RunPlugin, RunPluginLocation, RunPluginOrAlias, TiledPaneLayout,
};
pub use crate::input::options::PaneFrameStyle;
use crate::internal_ids::ContextMenuItemId;
use crate::internal_ids::ContextMenuRequestId;
use crate::internal_ids::DataWatchId;
use crate::internal_ids::HostId;
use crate::internal_ids::NotificationId;
use crate::internal_ids::ProcessId;
use crate::internal_ids::SessionName;
use crate::internal_ids::SupervisorId;
use crate::internal_ids::TabId;
use crate::internal_ids::TabNumber;
use crate::internal_ids::TabPosition;
use crate::position::{CellCoordinates, Column, ColumnCount, Row, RowCount};
use crate::units::{ElapsedMillis, FractionalSeconds, SignedUnixMillis, UnixMillis, UnixSeconds};
// The data store's types are part of the plugin API, so they belong in the prelude alongside
// everything else here. Their module contains portable values without database implementation details.
pub use crate::plugin_data::*;
use crate::position::Position;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::{self, FromStr};
use std::time::Duration;
use strum_macros::{Display, EnumDiscriminants, EnumIter, EnumString};
use unicode_width::UnicodeWidthChar;

pub use crate::internal_ids::ClientId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnblockCondition {
    /// Unblock only when exit status is 0 (success)
    OnExitSuccess,
    /// Unblock only when exit status is non-zero (failure)
    OnExitFailure,
    /// Unblock on any exit (success or failure)
    OnAnyExit,
}

impl UnblockCondition {
    /// Check if the condition is met for the given exit status
    pub fn is_met(&self, exit_status: i32) -> bool {
        match self {
            UnblockCondition::OnExitSuccess => exit_status == 0,
            UnblockCondition::OnExitFailure => exit_status != 0,
            UnblockCondition::OnAnyExit => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOrPlugin {
    Command(RunCommandAction),
    Plugin(RunPluginOrAlias),
    File(FileToOpen), // open file in configured editor
}

impl CommandOrPlugin {
    pub fn new_command(command: Vec<String>) -> Self {
        CommandOrPlugin::Command(RunCommandAction::new(command))
    }
}

pub fn single_client_color(colors: Palette) -> (PaletteColor, PaletteColor) {
    (colors.green, colors.black)
}

pub use crate::keys::{BareKey, KeyModifier, KeyWithModifier};

use crate::geometry::{Direction, Resize, ResizeStrategy};

/// A mouse event delivered to a plugin pane, with positions relative to the pane's content.
///
/// The host sends `LeftClick`, `Hold` and `Release` for the left button, and `RightClick` for a
/// right press on the focused pane's content, unless `context_menu_on_right_click` opens a menu
/// instead or alt is held (alt+right ungroups panes). `Hover` reports buttonless motion, also
/// over the pane's frame; when the pointer leaves the pane, the host sends `Hover(0, u16::MAX)`.
/// Each wheel direction has its own variant, carrying a count rather than a sign, because
/// horizontal scrolling exists alongside vertical. The middle button is not delivered.
///
/// The variants mirror `MouseEventName` in yinz-plugin-api's `event.proto` and the Scheme SDK event values.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mouse {
    ScrollUp(usize),   // number of lines
    ScrollDown(usize), // number of lines
    ScrollLeft(usize),
    ScrollRight(usize),
    LeftClick(isize, usize),  // line and column
    RightClick(isize, usize), // line and column
    Hold(isize, usize),       // line and column
    Release(isize, usize),    // line and column
    Hover(isize, usize),      // line and column
}

impl Mouse {
    pub fn position(&self) -> Option<(usize, usize)> {
        // (line, column)
        match self {
            Mouse::LeftClick(line, column) => Some((*line as usize, *column)),
            Mouse::RightClick(line, column) => Some((*line as usize, *column)),
            Mouse::Hold(line, column) => Some((*line as usize, *column)),
            Mouse::Release(line, column) => Some((*line as usize, *column)),
            Mouse::Hover(line, column) => Some((*line as usize, *column)),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileMetadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub len: u64,
}

/// One entry of an `FsList` answer: the entry's path as the plugin would name it, and its
/// metadata - `None` when the entry exists but could not be stat'ed, which is an answer and
/// not an error.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEntry {
    pub path: String,
    pub metadata: Option<FileMetadata>,
}

impl From<Metadata> for FileMetadata {
    fn from(metadata: Metadata) -> Self {
        FileMetadata {
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            is_symlink: metadata.is_symlink(),
            len: metadata.len(),
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledText {
    pub text: String,
    pub indices: Vec<Vec<usize>>,
}

/// These events can be subscribed to with subscribe method exported by `yinz-tile`.
/// Once subscribed to, they will trigger the `update` method of the `YinzPlugin` trait.
// `ModeUpdate` is the widest variant, but this enum is the plugin-facing event API and is
// mirrored in the protobuf contract: boxing a variant would break the `update()` match arm of
// every plugin built against it, in and out of this repo.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, EnumDiscriminants, Display, Serialize, Deserialize)]
#[strum_discriminants(derive(EnumString, EnumIter, Display, Hash, Serialize, Deserialize))]
#[strum_discriminants(name(EventType))]
#[non_exhaustive]
pub enum Event {
    NotificationReceived(NotificationData),
    NotificationRequested(NotificationData),
    NotificationResolved(NotificationResolution),
    ModeUpdate(ModeInfo),
    TabUpdate(Vec<TabInfo>),
    PaneUpdate(PaneManifest),
    PaneTabsUpdate(Vec<crate::pane_tabs::PaneTabsInfo>),
    /// A key was pressed while the user is focused on this plugin's pane
    Key(KeyWithModifier),
    /// A mouse event happened while the user is focused on this plugin's pane
    Mouse(Mouse),
    /// A timer expired set by the `set_timeout` method exported by `yinz-tile`. It carries the
    /// time the host timer slept, measured when it fired; delivery to the plugin can add more,
    /// so a plugin that needs elapsed wall time reads its own clock.
    Timer(FractionalSeconds),
    /// Text was copied to the clipboard anywhere in the app
    CopyToClipboard(CopyDestination),
    /// Failed to copy text to clipboard anywhere in the app
    SystemClipboardFailure,
    /// Input was received anywhere in the app
    InputReceived,
    /// This plugin became visible or invisible
    Visible(bool),
    /// A message from one of the plugin's workers
    CustomMessage(
        String, // message
        String, // payload
    ),
    /// A file was created somewhere in the Yinz CWD folder
    FileSystemCreate(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was accessed somewhere in the Yinz CWD folder
    FileSystemRead(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was modified somewhere in the Yinz CWD folder
    FileSystemUpdate(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was deleted somewhere in the Yinz CWD folder
    FileSystemDelete(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A Result of plugin permission request
    PermissionRequestResult(PermissionStatus),
    SessionUpdate(
        Vec<SessionInfo>,
        Vec<(SessionName, Duration)>, // resurrectable sessions
    ),
    PaneOutput {
        pane_id: u32,
        data: Vec<u8>,
        context: BTreeMap<String, String>,
    },
    RunCommandOutput {
        data: Vec<u8>,
        stderr: bool,
        context: BTreeMap<String, String>,
    },
    RunCommandResult(Option<i32>, Vec<u8>, Vec<u8>, BTreeMap<String, String>), // exit_code, STDOUT, STDERR,
    // context
    WebRequestResult(
        u16,
        BTreeMap<String, String>,
        Vec<u8>,
        BTreeMap<String, String>,
    ), // status,
    // headers,
    // body,
    // context
    /// A chunk of a streamed web request body (see `web_request_stream`). Chunks arrive in
    /// order after the leading `WebRequestResult`, and are always terminated by exactly one
    /// chunk with `done: true` - on clean EOF, on network error and on cancellation alike.
    WebRequestStreamChunk {
        stream_id: String,
        chunk: Vec<u8>,
        /// no further chunks will arrive for this stream_id
        done: bool,
        /// set only on the final chunk, when the stream was aborted rather than completed
        error: Option<String>,
    },
    CommandPaneOpened(crate::internal_ids::TerminalId, Context), // u32 - terminal_pane_id
    CommandPaneExited(crate::internal_ids::TerminalId, Option<i32>, Context), // u32 - terminal_pane_id, Option<i32> -
    // exit_code
    PaneClosed(PaneId),
    EditPaneOpened(crate::internal_ids::TerminalId, Context), // u32 - terminal_pane_id
    EditPaneExited(crate::internal_ids::TerminalId, Option<i32>, Context), // u32 - terminal_pane_id, Option<i32> - exit code
    CommandPaneReRun(crate::internal_ids::TerminalId, Context), // u32 - terminal_pane_id, Option<i32> -
    /// The config could not be written: the file that was meant to be written, when one was
    /// reached, and why, when the server knows.
    FailedToWriteConfigToDisk {
        file_path: Option<String>,
        reason: Option<String>,
    },
    ListClients(Vec<ClientInfo>),
    HostFolderChanged(PathBuf),               // PathBuf -> new host folder
    FailedToChangeHostFolder(Option<String>), // String -> the error we got when changing
    PastedText(String),
    ConfigWasWrittenToDisk,
    BeforeClose,
    /// The acting client's terminal cell geometry, or unknown before a valid report.
    CellPixelSizeChanged(Option<crate::pane_size::SizeInPixels>),
    InterceptedKeyPress(KeyWithModifier),
    /// An action was performed by the user (requires InterceptInput permission)
    UserAction(
        Action,
        ClientId,
        Option<crate::internal_ids::TerminalId>,
        Option<ClientId>,
    ), // Action, client_id, terminal_id, cli_client_id
    PaneRenderReport(HashMap<PaneId, PaneContents>),
    ActionComplete(Action, Option<PaneId>, BTreeMap<String, String>), // Action, pane_id, context
    CwdChanged(PaneId, PathBuf, Vec<ClientId>), // pane_id, cwd, focused_client_ids
    CommandChanged(PaneId, Vec<String>, bool, Vec<ClientId>), // pane_id, command, is_foreground, focused_client_ids
    AvailableLayoutInfo(Vec<LayoutInfo>, Vec<LayoutWithError>),
    PluginConfigurationChanged(BTreeMap<String, String>),
    HighlightClicked {
        pane_id: PaneId,
        pattern: String,
        matched_string: String,
        context: BTreeMap<String, String>,
    },
    /// A right-click asked for a context menu. The bound `context_menu` hook plugin answers with
    /// `set_context_menu`; anything else that receives this should ignore it.
    ContextMenuRequested(Box<ContextMenuContext>),
    /// An item the plugin supplied - one carrying `context` - was chosen.
    ///
    /// Only items with a non-empty `context` produce this. An item that is only a list of
    /// `Action`s is run by the server and the plugin hears nothing, the same way a keybinding's
    /// actions are not narrated back to anyone.
    ContextMenuItemSelected {
        label: String,
        /// The item's own `context`, echoed verbatim.
        item_context: BTreeMap<String, String>,
        /// The click the menu was opened by, so a stateless plugin still knows where it came from.
        context: Box<ContextMenuContext>,
    },
    /// Initial keybindings sent once on plugin load and on reconfiguration.
    /// Plugins that subscribe to this event signal they cache keybindings
    /// and can handle lightweight ModeUpdate events without keybindings.
    InitialKeybinds(KeybindsVec),
    /// The host terminal indicated its color palette theme mode (CSI 2031 / DSR 997).
    HostTerminalThemeChanged(HostTerminalThemeMode),
    HintText(BTreeMap<usize, StyledText>),
    ActivePaneScroll(Option<(usize, usize)>),
    /// The results of a watch registered with `data_watch` have changed.
    ///
    /// Sent only when the result set actually differs, so an idempotent write is silent. The rows
    /// are subject to the same limits as a fetch, and `truncated` says when they hit one.
    PluginDataChanged {
        watch_id: DataWatchId,
        rows: DataRows,
        truncated: bool,
    },
    /// A hook point with a headless shape was reached. Sent only to the plugin bound to `point`
    /// in `hooks { }`, which is loaded (without a pane) the first time it is needed and receives
    /// every later firing. The payload is typed per point - see [`HookPayload`].
    Hook {
        point: HookPoint,
        payload: HookPayload,
    },
    /// One event from a coding agent's own hooks - `yinz action agent-report --hook-json` -
    /// read once by the session and sent to every plugin subscribed to it, and streamed by
    /// `yinz events --type agent_event`. The pane's record moves in the same breath, on
    /// `PaneUpdate`; this carries what the record cannot, the prompt and the agent's last
    /// message above all. Needs `ReadApplicationState`.
    AgentEvent(crate::agent::AgentEvent),
    /// Complete provider registry snapshot, replayed to new subscribers.
    PickResult(crate::pick::PickResult),
    AgentProvidersChanged(Vec<crate::agent::provider::AgentProvider>),
    ProjectProvidersChanged(Vec<crate::project_provider::ProjectProvider>),
    /// Which modifier keys the client believes are held down right now, so a plugin can show
    /// something for as long as one is - the status bar swaps its `Alt + ...` hint in this way.
    ///
    /// `is_live` says which of two very different things this is. `true` means the terminal
    /// actually reported the modifier key going down, and a matching event with an empty set
    /// will report it coming back up. `false` means it was *inferred* from a chorded keystroke
    /// (`Alt+n` says Alt was down when it was pressed and nothing more), and no release will
    /// follow. A plugin must time out an inferred hold itself, and should time out a live one
    /// too: a terminal that loses focus mid-hold is not obliged to send the release.
    ///
    /// Needs `ReadApplicationState`.
    ModifiersHeld {
        modifiers: BTreeSet<KeyModifier>,
        is_live: bool,
    },
}

impl EventType {
    /// The events `yinz events` streams: the session's state as it is reported to plugins.
    /// Not here, on purpose: one client's input (`Key`, `Mouse`, `PastedText`), one plugin's
    /// replies (`RunCommandResult`, `WebRequestResult`, `CustomMessage`, `Timer`) and the
    /// render report, which has its own stream in `yinz subscribe`.
    pub const CLI_STREAMABLE: &'static [EventType] = &[
        EventType::ModeUpdate,
        EventType::TabUpdate,
        EventType::PaneUpdate,
        EventType::SessionUpdate,
        EventType::CommandPaneOpened,
        EventType::CommandPaneExited,
        EventType::CommandPaneReRun,
        EventType::EditPaneOpened,
        EventType::EditPaneExited,
        EventType::PaneClosed,
        EventType::CwdChanged,
        EventType::CommandChanged,
        EventType::HostFolderChanged,
        EventType::FileSystemCreate,
        EventType::FileSystemRead,
        EventType::FileSystemUpdate,
        EventType::FileSystemDelete,
        EventType::PluginConfigurationChanged,
        EventType::AgentEvent,
    ];

    pub fn is_cli_streamable(&self) -> bool {
        Self::CLI_STREAMABLE.contains(self)
    }

    /// A streamable event by either of its spellings, `PaneUpdate` or `pane_update`. `None` for
    /// a name that is not an event, or an event that is not streamed.
    pub fn from_cli_name(name: &str) -> Option<EventType> {
        let name = name.trim();
        let pascal: String = match name.contains('_') {
            true => name
                .split('_')
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                        None => String::new(),
                    }
                })
                .collect(),
            false => name.to_owned(),
        };
        pascal
            .parse::<EventType>()
            .ok()
            .filter(EventType::is_cli_streamable)
    }

    pub fn cli_streamable_names() -> Vec<String> {
        Self::CLI_STREAMABLE
            .iter()
            .map(EventType::to_string)
            .collect()
    }
}

pub use crate::style::HostTerminalThemeMode;

#[derive(
    Debug,
    PartialEq,
    Eq,
    Hash,
    Copy,
    Clone,
    EnumDiscriminants,
    Display,
    Serialize,
    Deserialize,
    PartialOrd,
    Ord,
)]
#[strum_discriminants(derive(
    EnumString,
    EnumIter,
    Hash,
    Serialize,
    Deserialize,
    Display,
    PartialOrd,
    Ord
))]
#[strum_discriminants(name(PermissionType))]
#[non_exhaustive]
pub enum Permission {
    ReadApplicationState,
    ChangeApplicationState,
    OpenFiles,
    RunCommands,
    OpenTerminalsOrPlugins,
    WriteToStdin,
    WebAccess,
    ReadCliPipes,
    MessageAndLaunchOtherPlugins,
    Reconfigure,
    FullHdAccess,
    InterceptInput,
    ReadPaneContents,
    RunActionsAsUser,
    WriteToClipboard,
    ReadSessionEnvironmentVariables,
    PluginData,
    SharedPluginData,
    ReadFiles,
    WriteFiles,
    /// Write to `/plugins`, the user's own plugin folder, and reload plugins by url. The
    /// authority a plugin that writes other plugins needs, and nothing wider: `WriteFiles`
    /// still covers the write itself, so a plugin that only reads `/plugins` needs `ReadFiles`.
    AuthorPlugins,
}

impl PermissionType {
    pub fn display_name(&self) -> String {
        match self {
            PermissionType::ReadApplicationState => {
                "Access Yinz state (Panes, Tabs and UI)".to_owned()
            },
            PermissionType::ChangeApplicationState => {
                "Change Yinz state (Panes, Tabs and UI)".to_owned()
            },
            PermissionType::OpenFiles => "Open files (eg. for editing)".to_owned(),
            PermissionType::RunCommands => "Run commands".to_owned(),
            PermissionType::OpenTerminalsOrPlugins => "Start new terminals and plugins".to_owned(),
            PermissionType::WriteToStdin => "Write to standard input (STDIN)".to_owned(),
            PermissionType::WebAccess => "Make web requests".to_owned(),
            PermissionType::ReadCliPipes => "Control command line pipes and output".to_owned(),
            PermissionType::MessageAndLaunchOtherPlugins => {
                "Send messages to and launch other plugins".to_owned()
            },
            PermissionType::Reconfigure => "Change Yinz runtime configuration".to_owned(),
            PermissionType::FullHdAccess => "Full access to the hard-drive".to_owned(),
            PermissionType::InterceptInput => "Intercept Input (keyboard & mouse)".to_owned(),
            PermissionType::ReadPaneContents => {
                "Read pane contents (viewport and selection)".to_owned()
            },
            PermissionType::RunActionsAsUser => "Execute actions as the user".to_owned(),
            PermissionType::WriteToClipboard => "Write to clipboard".to_owned(),
            PermissionType::ReadSessionEnvironmentVariables => {
                "Read environment variables present upon session creation".to_owned()
            },
            PermissionType::PluginData => "Store and query its own structured data".to_owned(),
            PermissionType::SharedPluginData => {
                "Store and query data shared with other plugins".to_owned()
            },
            PermissionType::ReadFiles => "Read files in its own folders".to_owned(),
            PermissionType::WriteFiles => "Write files in its own folders".to_owned(),
            PermissionType::AuthorPlugins => {
                "Write and reload plugins in your plugin folder".to_owned()
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginPermission {
    pub request_id: u64,
    pub client_id: Option<crate::internal_ids::ClientId>,
    pub caller: Option<crate::internal_ids::TerminalId>,
    pub name: String,
    pub permissions: Vec<PermissionType>,
}

impl PluginPermission {
    pub fn new(name: String, permissions: Vec<PermissionType>) -> Self {
        PluginPermission {
            name,
            permissions,
            request_id: 0,
            client_id: None,
            caller: None,
        }
    }
}

/// Describes the different input modes, which change the way that keystrokes will be interpreted.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Hash,
    Copy,
    Clone,
    EnumIter,
    Serialize,
    Deserialize,
    PartialOrd,
    Ord,
    Default,
)]
pub enum InputMode {
    /// In `Normal` mode, input is always written to the terminal, except for the shortcuts leading
    /// to other modes
    #[default]
    #[serde(alias = "normal")]
    Normal,
    /// In `Locked` mode, input is always written to the terminal and all shortcuts are disabled
    /// except the one leading back to normal mode
    #[serde(alias = "locked")]
    Locked,
    /// `Resize` mode allows resizing the different existing panes.
    #[serde(alias = "resize")]
    Resize,
    /// `Pane` mode allows creating and closing panes, as well as moving between them.
    #[serde(alias = "pane")]
    Pane,
    /// `Tab` mode allows creating and closing tabs, as well as moving between them.
    #[serde(alias = "tab")]
    Tab,
    /// `Scroll` mode allows scrolling up and down within a pane.
    #[serde(alias = "scroll")]
    Scroll,
    /// `EnterSearch` mode allows for typing in the needle for a search in the scroll buffer of a pane.
    #[serde(alias = "entersearch")]
    EnterSearch,
    /// `Search` mode allows for searching a term in a pane (superset of `Scroll`).
    #[serde(alias = "search")]
    Search,
    /// `RenameTab` mode allows assigning a new name to a tab.
    #[serde(alias = "renametab")]
    RenameTab,
    /// `RenamePane` mode allows assigning a new name to a pane.
    #[serde(alias = "renamepane")]
    RenamePane,
    /// `Session` mode allows detaching sessions
    #[serde(alias = "session")]
    Session,
    /// `Move` mode allows moving the different existing panes within a tab
    #[serde(alias = "move")]
    Move,
    /// `Prompt` mode allows interacting with active prompts.
    #[serde(alias = "prompt")]
    Prompt,
    /// `Tmux` mode allows for basic tmux keybindings functionality
    #[serde(alias = "tmux")]
    Tmux,
    /// `Agent` mode gathers coding-agent actions behind a single leader.
    #[serde(alias = "agent")]
    Agent,
    /// `Select` mode extends a text selection in the focused pane.
    #[serde(alias = "select")]
    Select,
    /// `Notifications` mode browses the session's notifications in place, where the toasts render.
    #[serde(alias = "notifications")]
    Notifications,
}

pub use crate::style::ThemeHue;

pub use crate::colors::PaletteColor;

/// Priority layer for plugin-supplied regex highlights.
/// Higher-priority layers take visual precedence over lower ones
/// when highlights overlap.  Built-in highlights (mouse selection,
/// search results) always take precedence over all plugin layers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub enum HighlightLayer {
    #[default]
    Hint, // lowest: pure pattern matching (paths, URLs, IPs)
    Tool,           // middle: backed by runtime domain knowledge (git, docker, k8s)
    ActionFeedback, // highest: result of an explicit user action (search, bookmarks)
}

/// Style for a plugin-supplied regex highlight.
/// Theme-based variants reference `style.colors.text_unselected.*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightStyle {
    None,      // no color override — use with bold/italic/underline for style-only highlights
    Emphasis0, // fg = emphasis_0, no bg override
    Emphasis1, // fg = emphasis_1, no bg override
    Emphasis2, // fg = emphasis_2, no bg override
    Emphasis3, // fg = emphasis_3, no bg override
    BackgroundEmphasis0, // bg = emphasis_0, fg = background
    BackgroundEmphasis1, // bg = emphasis_1, fg = background
    BackgroundEmphasis2, // bg = emphasis_2, fg = background
    BackgroundEmphasis3, // bg = emphasis_3, fg = background
    CustomRgb {
        fg: Option<(u8, u8, u8)>,
        bg: Option<(u8, u8, u8)>,
    },
    CustomIndex {
        fg: Option<u8>,
        bg: Option<u8>,
    },
}

/// One pattern + style pair sent by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegexHighlight {
    pub pattern: String, // key for upsert; also the regex source
    pub style: HighlightStyle,
    pub layer: HighlightLayer,
    pub context: BTreeMap<String, String>, // arbitrary data echoed back verbatim on click
    pub on_hover: bool, // if true, only rendered when the cursor overlaps this match
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub tooltip_text: Option<String>, // shown at bottom of pane frame when hovering over match
}

/// One row of a right-click context menu.
///
/// A menu is a list of these and nothing else: the server draws them, hit-tests them, and runs
/// `actions` against the pane the click landed in. There is no widget tree and no nesting - see
/// `yinz-panes/src/ui/context_menu.rs` for the drawing and `input::context_menu` for the
/// configured half of where they come from.
///
/// **`actions` does not cross the plugin boundary, and `id` is why it does not have to.** A good
/// many `Action` variants have no protobuf representation at all - `Action::Copy` among them - so
/// a wire format carrying actions would drop some items' behaviour in silence. Instead the server
/// keeps the action lists and sends the plugin an `id` per configured item; a plugin returning an
/// item with that `id` gets the actions re-attached on arrival. An item the plugin *invented* has
/// no `id`, no actions, and acts through its `context` instead - which reaches it as
/// `Event::ContextMenuItemSelected`, from where `run_action` can do anything an action could.
///
/// `separator` is a variant spelled as a flag rather than as an enum because a separator is a row
/// like any other everywhere it matters - layout, hit-testing, and the wire format - and an enum
/// would put a match in each of those for one uninteresting case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuItem {
    /// Identifies a configured item across the plugin boundary. `None` for an item a plugin made
    /// up, and for one a plugin returned without the `id` it was given - which is how a plugin
    /// deliberately strips a configured item's behaviour, if it ever wants to.
    #[serde(default)]
    pub id: Option<ContextMenuItemId>,
    /// Defaulted rather than required: a separator genuinely has no label, and making every
    /// plugin write `label = ""` for one would be a papercut with nothing behind it.
    #[serde(default)]
    pub label: String,
    /// Drawn right-aligned, dimmed. Purely a hint - nothing binds it.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Run in order, against the pane the menu was opened over. Never sent to a plugin; see the
    /// type's own documentation for why.
    #[serde(default, skip_serializing)]
    pub actions: Vec<Action>,
    /// Opaque plugin data echoed back to the hook plugin when this item is chosen, exactly as
    /// `RegexHighlight::context` is echoed on click. This is how a plugin-supplied item does
    /// something no `Action` can express.
    #[serde(default)]
    pub context: BTreeMap<String, String>,
    /// Drawn dimmed and not selectable.
    #[serde(default)]
    pub disabled: bool,
    /// A horizontal rule. `label`, `actions` and the rest are ignored.
    #[serde(default)]
    pub separator: bool,
}

impl ContextMenuItem {
    pub fn new<L: Into<String>>(label: L, actions: Vec<Action>) -> Self {
        ContextMenuItem {
            label: label.into(),
            actions,
            ..Default::default()
        }
    }
    pub fn separator() -> Self {
        ContextMenuItem {
            separator: true,
            ..Default::default()
        }
    }
    pub fn with_shortcut<S: Into<String>>(mut self, shortcut: S) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
    pub fn with_id(mut self, id: ContextMenuItemId) -> Self {
        self.id = Some(id);
        self
    }
    /// Whether the keyboard and mouse may land on this row.
    pub fn is_selectable(&self) -> bool {
        !self.separator && !self.disabled
    }
    /// This item as the plugin should see it: identified, but without the behaviour the server
    /// keeps to itself.
    pub fn without_actions(&self) -> Self {
        ContextMenuItem {
            actions: vec![],
            ..self.clone()
        }
    }
}

/// A `link`-plugin regex match sitting under the cursor when the menu was opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuLink {
    /// The plugin that registered the highlight, so an item can be aimed back at it.
    pub plugin_id: crate::internal_ids::PluginId,
    pub pattern: String,
    pub matched: String,
    pub context: BTreeMap<String, String>,
}

/// Everything the server could cheaply learn about a right-click, handed to the hook plugin so it
/// can decide what the menu should say.
///
/// "Cheaply" is the whole design rule. Two things a menu might plausibly want are absent for the
/// same reason - getting them would mean blocking the screen thread, which every mouse *motion*
/// also passes through:
///
/// - **cwd** lives in the pty thread (`PtyInstruction::GetPaneCwd`), reachable only through a
///   blocking `recv_timeout`. A plugin asks for it itself with `get_pane_cwd`.
/// - **scrollback** can be megabytes. `get_pane_scrollback` already exists for a plugin that
///   wants it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuContext {
    /// Correlates the plugin's answer with the menu that asked. An answer naming a menu that has
    /// since closed is dropped.
    pub request_id: ContextMenuRequestId,
    pub client_id: ClientId,
    pub pane_id: PaneId,
    pub tab_position: TabPosition,
    /// The clicked cell in surface coordinates - where the menu is anchored.
    pub click: (usize, usize),
    /// The same cell relative to the pane, which is what a plugin reasoning about pane content
    /// wants. Both are carried because neither can be derived from the other here.
    pub click_in_pane: (usize, usize),
    /// The click landed on the pane's frame rather than its content.
    pub on_frame: bool,
    pub pane_title: String,
    pub pane_command: Option<String>,
    pub pane_exited: bool,
    pub pane_exit_status: Option<i32>,
    pub pane_is_held: bool,
    pub pane_is_floating: bool,
    pub selected_text: Option<String>,
    pub word_under_cursor: Option<String>,
    pub line_under_cursor: Option<String>,
    pub link: Option<ContextMenuLink>,
    /// The items the configuration declared, in the order it declared them. The plugin filters and
    /// augments this rather than building a menu from nothing.
    pub items: Vec<ContextMenuItem>,
}

impl FromStr for InputMode {
    type Err = ConversionError;

    fn from_str(s: &str) -> Result<Self, ConversionError> {
        match s {
            "normal" | "Normal" => Ok(InputMode::Normal),
            "locked" | "Locked" => Ok(InputMode::Locked),
            "resize" | "Resize" => Ok(InputMode::Resize),
            "pane" | "Pane" => Ok(InputMode::Pane),
            "tab" | "Tab" => Ok(InputMode::Tab),
            "search" | "Search" => Ok(InputMode::Search),
            "scroll" | "Scroll" => Ok(InputMode::Scroll),
            "renametab" | "RenameTab" => Ok(InputMode::RenameTab),
            "renamepane" | "RenamePane" => Ok(InputMode::RenamePane),
            "session" | "Session" => Ok(InputMode::Session),
            "move" | "Move" => Ok(InputMode::Move),
            "prompt" | "Prompt" => Ok(InputMode::Prompt),
            "tmux" | "Tmux" => Ok(InputMode::Tmux),
            "agent" | "Agent" => Ok(InputMode::Agent),
            "select" | "Select" => Ok(InputMode::Select),
            "notifications" | "Notifications" => Ok(InputMode::Notifications),
            "entersearch" | "Entersearch" | "EnterSearch" => Ok(InputMode::EnterSearch),
            e => Err(ConversionError::UnknownInputMode(e.into())),
        }
    }
}

pub use crate::style::{
    Coloration, MultiplayerColors, Palette, PaletteSource, Style, StyleDeclaration, Styling,
    DEFAULT_STYLES,
};

/// Keybinds as an ordered list rather than a map: this crosses the plugin wire
/// (`ModeInfo.keybinds`, `Event::InitialKeybinds`) and the status bars render it in
/// order, so the order has to be stable and part of the type.
pub type KeybindsVec = Vec<(InputMode, Vec<(KeyWithModifier, Vec<Action>)>)>;

/// Provides information helpful in rendering the Yinz controls for UI bars
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeInfo {
    /// Missing values preserve the default enabled behavior for older plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_groups: Option<bool>,
    pub mode: InputMode,
    pub base_mode: Option<InputMode>,
    pub keybinds: KeybindsVec,
    pub style: Style,
    pub capabilities: PluginCapabilities,
    pub session_name: Option<SessionName>,
    pub editor: Option<PathBuf>,
    pub shell: Option<PathBuf>,
    pub currently_marking_pane_group: Option<bool>,
    pub pane_frame_style: Option<PaneFrameStyle>,
    pub session_dimmed: Option<bool>,
    pub session_ancestry: Vec<SessionName>,
    pub host_fullscreen: Option<bool>,
    pub nested_ascend_keys: Vec<KeyWithModifier>,
    pub session_ascended: Option<bool>,
    pub nested_descend_keys: Vec<KeyWithModifier>,
}

impl ModeInfo {
    pub fn get_mode_keybinds(&self) -> Vec<(KeyWithModifier, Vec<Action>)> {
        self.get_keybinds_for_mode(self.mode)
    }

    pub fn get_keybinds_for_mode(&self, mode: InputMode) -> Vec<(KeyWithModifier, Vec<Action>)> {
        for (vec_mode, map) in &self.keybinds {
            if mode == *vec_mode {
                return map.to_vec();
            }
        }
        vec![]
    }
    pub fn update_keybinds(&mut self, keybinds: Keybinds) {
        self.keybinds = keybinds.to_keybinds_vec();
    }
    pub fn update_default_mode(&mut self, new_default_mode: InputMode) {
        self.base_mode = Some(new_default_mode);
    }
    pub fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
    }
    pub fn update_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
    }
    pub fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        // it is honestly quite baffling to me how "arrow_fonts: false" can mean "I support arrow
        // fonts", but since this is a public API... ¯\_(ツ)_/¯
        self.capabilities.arrow_fonts = !should_support_arrow_fonts;
    }
    pub fn update_hide_session_name(&mut self, hide_session_name: bool) {
        self.style.hide_session_name = hide_session_name;
    }
    pub fn change_to_default_mode(&mut self) {
        if let Some(base_mode) = self.base_mode {
            self.mode = base_mode;
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SessionInfo {
    pub name: SessionName,
    pub tabs: Vec<TabInfo>,
    pub panes: PaneManifest,
    pub connected_clients: usize,
    pub is_current_session: bool,
    pub available_layouts: Vec<LayoutInfo>,
    pub plugins: BTreeMap<crate::internal_ids::PluginId, PluginInfo>,
    pub tab_history: BTreeMap<ClientId, Vec<TabId>>,
    pub pane_history: BTreeMap<ClientId, Vec<PaneId>>,
    pub creation_time: Duration,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PluginInfo {
    pub location: String,
    pub configuration: BTreeMap<String, String>,
}

impl From<RunPlugin> for PluginInfo {
    fn from(run_plugin: RunPlugin) -> Self {
        PluginInfo {
            location: run_plugin.location.display(),
            configuration: run_plugin.configuration.inner().clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum LayoutInfo {
    BuiltIn(String),
    File(String, LayoutMetadata),
    Stringified(String),
}

impl AsRef<LayoutInfo> for LayoutInfo {
    fn as_ref(&self) -> &LayoutInfo {
        self
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LayoutMetadata {
    pub tabs: Vec<TabMetadata>,
    pub creation_time: String,
    pub update_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TabMetadata {
    pub panes: Vec<PaneMetadata>,
    pub name: Option<String>,
}

impl
    From<&(
        Option<String>,
        crate::input::layout::TiledPaneLayout,
        Vec<crate::input::layout::FloatingPaneLayout>,
    )> for TabMetadata
{
    fn from(
        tab: &(
            Option<String>,
            crate::input::layout::TiledPaneLayout,
            Vec<crate::input::layout::FloatingPaneLayout>,
        ),
    ) -> Self {
        let (tab_name, tiled_pane_layout, floating_panes) = tab;

        // Collect panes from tiled layout (only leaf nodes are real panes)
        let mut panes = Vec::new();
        collect_leaf_panes(tiled_pane_layout, &mut panes);

        // Collect panes from floating panes
        for floating_pane in floating_panes {
            panes.push(PaneMetadata::from(floating_pane));
        }

        TabMetadata {
            panes,
            name: tab_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneMetadata {
    pub name: Option<String>,
    pub is_plugin: bool,
    pub is_builtin_plugin: bool,
}

impl From<&crate::input::layout::TiledPaneLayout> for PaneMetadata {
    fn from(pane: &crate::input::layout::TiledPaneLayout) -> Self {
        let mut is_plugin = false;
        let mut is_builtin_plugin = false;

        // Try to get the name from the pane's name field first
        let name = if let Some(ref name) = pane.name {
            Some(name.clone())
        } else if let Some(ref run) = pane.run {
            // If no explicit name, glean it from the run configuration
            match run {
                Run::Command(cmd) => {
                    // Use the command name
                    Some(cmd.command.to_string_lossy().to_string())
                },
                Run::EditFile(path, _line, _cwd) => {
                    // Use the file name
                    path.file_name().map(|n| n.to_string_lossy().to_string())
                },
                Run::Plugin(plugin) => {
                    is_plugin = true;
                    is_builtin_plugin = plugin.is_builtin_plugin();
                    Some(plugin.location_string())
                },
                Run::Cwd(_) => None,
            }
        } else {
            None
        };

        PaneMetadata {
            name,
            is_plugin,
            is_builtin_plugin,
        }
    }
}

impl From<&crate::input::layout::FloatingPaneLayout> for PaneMetadata {
    fn from(pane: &crate::input::layout::FloatingPaneLayout) -> Self {
        let mut is_plugin = false;
        let mut is_builtin_plugin = false;

        // Try to get the name from the pane's name field first
        let name = if let Some(ref name) = pane.name {
            Some(name.clone())
        } else if let Some(ref run) = pane.run {
            // If no explicit name, glean it from the run configuration
            match run {
                Run::Command(cmd) => {
                    // Use the command name
                    Some(cmd.command.to_string_lossy().to_string())
                },
                Run::EditFile(path, _line, _cwd) => {
                    // Use the file name
                    path.file_name().map(|n| n.to_string_lossy().to_string())
                },
                Run::Plugin(plugin) => {
                    is_plugin = true;
                    is_builtin_plugin = match plugin {
                        crate::input::layout::RunPluginOrAlias::RunPlugin(run_plugin) => {
                            matches!(run_plugin.location, RunPluginLocation::Yinz(_))
                        },
                        crate::input::layout::RunPluginOrAlias::Alias(_) => false,
                    };
                    // Use the plugin location string
                    Some(plugin.location_string())
                },
                Run::Cwd(_) => None,
            }
        } else {
            None
        };

        PaneMetadata {
            name,
            is_plugin,
            is_builtin_plugin,
        }
    }
}

// Helper function to recursively collect leaf panes from TiledPaneLayout
fn collect_leaf_panes(
    pane: &crate::input::layout::TiledPaneLayout,
    result: &mut Vec<PaneMetadata>,
) {
    if pane.children.is_empty() {
        // This is a leaf node (actual pane)
        result.push(PaneMetadata::from(pane));
    } else {
        // This is a container, recurse into children
        for child in &pane.children {
            collect_leaf_panes(child, result);
        }
    }
}

impl LayoutInfo {
    pub fn name(&self) -> &str {
        match self {
            LayoutInfo::BuiltIn(name) => name,
            LayoutInfo::File(name, _) => name,
            LayoutInfo::Stringified(layout) => layout,
        }
    }
    pub fn is_builtin(&self) -> bool {
        match self {
            LayoutInfo::BuiltIn(_name) => true,
            LayoutInfo::File(_name, _) => false,
            LayoutInfo::Stringified(_stringified) => false,
        }
    }
}

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for SessionInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl SessionInfo {
    pub fn new(name: SessionName) -> Self {
        SessionInfo {
            name,
            ..Default::default()
        }
    }
    pub fn update_tab_info(&mut self, new_tab_info: Vec<TabInfo>) {
        self.tabs = new_tab_info;
    }
    pub fn update_pane_info(&mut self, new_pane_info: PaneManifest) {
        self.panes = new_pane_info;
    }
    pub fn update_connected_clients(&mut self, new_connected_clients: usize) {
        self.connected_clients = new_connected_clients;
    }
    pub fn populate_plugin_list(
        &mut self,
        plugins: BTreeMap<crate::internal_ids::PluginId, RunPlugin>,
    ) {
        // u32 - plugin_id
        let mut plugin_list = BTreeMap::new();
        for (plugin_id, run_plugin) in plugins {
            plugin_list.insert(plugin_id, run_plugin.into());
        }
        self.plugins = plugin_list;
    }
}

use crate::agent::{AgentInfo, AgentState};

/// Contains all the information for a currently opened tab.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct TabInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<crate::tab_bar::TabGroupInfo>,
    /// The Tab's 0 indexed position
    pub position: TabPosition,
    /// The name of the tab as it appears in the UI (if there's enough room for it)
    pub name: String,
    /// Whether this tab is focused
    pub active: bool,
    /// The number of suppressed panes this tab has
    pub panes_to_hide: usize,
    /// Whether there's one pane taking up the whole display area on this tab
    pub is_fullscreen_active: bool,
    /// Whether input sent to this tab will be synced to all panes in it
    pub is_sync_panes_active: bool,
    pub are_floating_panes_visible: bool,
    pub other_focused_clients: Vec<ClientId>,
    /// The multiplayer color slot of each of `other_focused_clients`, in the same order: `None`
    /// for a client past the tenth. Client ids grow without bound, so a plugin colors a client by
    /// its slot (`player_<slot>`), not its id. Empty from a host that does not send it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_focused_client_slots: Vec<Option<crate::multiplayer::ColorSlot>>,
    pub active_swap_layout_name: Option<String>,
    /// Whether the user manually changed the layout, moving out of the swap layout scheme
    pub is_swap_layout_dirty: bool,
    /// Row count in the viewport (including all non-ui panes, eg. will exclude the status bar)
    pub viewport_rows: RowCount,
    /// Column count in the viewport (including all non-ui panes, eg. will exclude the status bar)
    pub viewport_columns: ColumnCount,
    /// Row count in the display area (including all panes, will typically be larger than the
    /// viewport)
    pub display_area_rows: RowCount,
    /// Column count in the display area (including all panes, will typically be larger than the
    /// viewport)
    pub display_area_columns: ColumnCount,
    /// The number of selectable (eg. not the UI bars) tiled panes currently in this tab
    pub selectable_tiled_panes_count: usize,
    /// The number of selectable (eg. not the UI bars) floating panes currently in this tab
    pub selectable_floating_panes_count: usize,
    /// The stable identifier for this tab
    pub tab_id: TabId,
    /// Whether this tab has an active (persistent) bell notification
    pub has_bell_notification: bool,
    /// Whether this tab is currently flashing its bell (transient 400ms state)
    pub is_flashing_bell: bool,
}

/// The `PaneManifest` contains a dictionary of panes, indexed by the tab position (0 indexed).
/// Panes include all panes in the relevant tab, including `tiled` panes, `floating` panes and
/// `suppressed` panes.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneManifest {
    pub panes: HashMap<TabPosition, Vec<PaneInfo>>, // keyed by display position
}

/// Contains all the information for a currently open pane
///
/// # Difference between coordinates/size and content coordinates/size
///
/// The pane basic coordinates and size (eg. `pane_x` or `pane_columns`) are the entire space taken
/// up by this pane - including its frame and title if it has a border.
///
/// The pane content coordinates and size (eg. `pane_content_x` or `pane_content_columns`)
/// represent the area taken by the pane's content, excluding its frame and title if it has a
/// border.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PaneInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_tabs: Option<crate::pane_tabs::PaneTabsInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipane: Option<crate::multipane::MultipaneInfo>,
    /// The pane identity, including whether it is a terminal or a plugin.
    #[serde(flatten, with = "crate::pane_id::legacy_fields")]
    pub id: PaneId,
    /// Whether the pane is focused in its layer (tiled or floating)
    pub is_focused: bool,
    pub is_fullscreen: bool,
    /// Whether a pane is floating or tiled (embedded)
    pub is_floating: bool,
    /// Whether a pane is suppressed - suppressed panes are not visible to the user, but still run
    /// in the background
    pub is_suppressed: bool,
    /// Membership reported by the owning tab, including hidden list entries.
    #[serde(default)]
    pub is_stack_list_member: bool,
    /// The raw title, used for renaming and saved layouts.
    pub title: String,
    /// The full display title, without the pane ID or transient frame indicators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    /// Whether a pane exited or not, note that most panes close themselves before setting this
    /// flag, so this is only relevant to command panes
    pub exited: bool,
    /// The exit status of a pane if it did exit and is still in the UI
    pub exit_status: Option<i32>,
    /// A "held" pane is a paused pane that is waiting for user input (eg. a command pane that
    /// exited and is waiting to be re-run or closed)
    pub is_held: bool,
    /// Why this held agent command cannot safely restart, if applicable.
    #[serde(default)]
    pub restart_error: Option<String>,
    pub pane_x: Column,
    pub pane_content_x: Column,
    pub pane_y: Row,
    pub pane_content_y: Row,
    pub pane_rows: RowCount,
    pub pane_content_rows: RowCount,
    pub pane_columns: ColumnCount,
    pub pane_content_columns: ColumnCount,
    /// The coordinates of the cursor - if this pane is focused - relative to the pane's
    /// coordinates
    pub cursor_coordinates_in_pane: Option<CellCoordinates>, // x, y if cursor is visible
    /// If this is a command pane, this will show the stringified version of the command and its
    /// arguments
    pub terminal_command: Option<String>,
    /// The URL from which this plugin was loaded (eg. `yinz:strider` for the built-in `strider`
    /// plugin or `file:/path/to/my/plugin.scm` for a local plugin)
    pub plugin_url: Option<String>,
    /// Unselectable panes are often used for UI elements that do not have direct user interaction
    /// (eg. the default `status-bar` or `tab-bar`).
    pub is_selectable: bool,
    /// Grouped panes (usually through an explicit user action) that are staged for a bulk action
    /// the index is kept track of in order to preserve the pane group order
    pub index_in_pane_group: BTreeMap<ClientId, usize>,
    /// The default foreground color of this pane, if set (e.g. "#00e000")
    pub default_fg: Option<String>,
    /// The default background color of this pane, if set (e.g. "#001a3a")
    pub default_bg: Option<String>,
    /// The folder this pane's process is in right now, for a terminal pane.
    ///
    /// Carried on every manifest rather than only announced by `CwdChanged`: an
    /// announcement is heard once, so a plugin that loaded after it — or missed
    /// it — had no second way to find out and stayed wrong about the pane for as
    /// long as it lived. `None` for a plugin pane, which is not a process in a
    /// folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The machine this pane's process runs on, if it is not this one.
    ///
    /// `None` means local, which is every pane unless it was opened with
    /// `--remote-host`. It is here so a pane's *identity* is complete: two panes
    /// sitting in `/home/me/work` on different machines are in different folders,
    /// and anything keying on the path alone would quietly merge them.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub target: PaneTarget,
    /// The agent running in this pane, if the session knows of one. See [`AgentInfo`].
    #[serde(default)]
    pub agent: Option<AgentInfo>,
}

impl PaneInfo {
    pub fn is_plugin(&self) -> bool {
        matches!(self.id, PaneId::Plugin(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneListEntry {
    #[serde(flatten)]
    pub pane_info: PaneInfo,
    pub tab_id: TabId,
    pub tab_position: TabPosition,
    pub tab_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_cwd: Option<String>,
}

pub type ListPanesResponse = Vec<PaneListEntry>;
pub type ListTabsResponse = Vec<TabInfo>;

pub use crate::notifications::{
    NotificationData, NotificationDecision, NotificationDelivery, NotificationResolution,
    NotificationSource,
};

/// A notification outcome retained by the current live session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct NotificationInfo {
    pub id: NotificationId,
    pub message: String,
    pub received_at_unix_ms: UnixMillis,
    #[serde(default)]
    pub outcome: Option<NotificationResolution>,
}

impl NotificationInfo {
    pub fn browser_message(&self) -> String {
        match &self.outcome {
            Some(outcome) => format!(
                "[{:?}{}] {}",
                outcome.original.source,
                if outcome.suppressed {
                    ", suppressed"
                } else {
                    ""
                },
                self.message
            ),
            None => self.message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientInfo {
    pub client_id: ClientId,
    pub pane_id: PaneId,
    pub running_command: String,
    pub is_current_client: bool,
}

impl ClientInfo {
    pub fn new(
        client_id: ClientId,
        pane_id: PaneId,
        running_command: String,
        is_current_client: bool,
    ) -> Self {
        ClientInfo {
            client_id,
            pane_id,
            running_command,
            is_current_client,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneRenderReport {
    pub all_pane_contents: HashMap<ClientId, HashMap<PaneId, PaneContents>>,
}

impl PaneRenderReport {
    pub fn add_pane_contents(
        &mut self,
        client_ids: &[ClientId],
        pane_id: PaneId,
        pane_contents: PaneContents,
    ) {
        for client_id in client_ids {
            let p = self.all_pane_contents.entry(*client_id).or_default();
            p.insert(pane_id, pane_contents.clone());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneContents {
    // NOTE: both lines_above_viewport and lines_below_viewport are only populated if explicitly
    // requested (eg. with get_full_scrollback true in the plugin command) this is for performance
    // reasons
    pub lines_above_viewport: Vec<String>,
    pub lines_below_viewport: Vec<String>,
    pub viewport: Vec<String>,
    pub selected_text: Option<SelectedText>,
    pub cursor: Option<CellCoordinates>,
    /// Milliseconds since this pane's process last wrote to its pty, if it ever did.
    ///
    /// The only signal that separates "the program produced something" from "the
    /// picture moved": scrolling, reflowing and re-rendering all change what a pane
    /// shows without a single byte arriving from the process.
    #[serde(default)]
    pub last_output_ms_ago: Option<ElapsedMillis>,
    /// Whether the user has scrolled this pane up out of its live tail.
    ///
    /// A scrolled pane shows old output, so its viewport says nothing about what the
    /// process is doing right now.
    #[serde(default)]
    pub is_scrolled: bool,
}

/// Extract text from a line between two column positions, accounting for wide characters
fn extract_text_by_columns(line: &str, start_col: usize, end_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();
    let mut capturing = false;

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        // Start capturing when we reach start_col
        if current_col >= start_col && !capturing {
            capturing = true;
        }

        // Stop if we've reached or passed end_col
        if current_col >= end_col {
            break;
        }

        // Capture character if we're in the range
        if capturing {
            result.push(ch);
        }

        current_col += char_width;
    }

    result
}

/// Extract text from a line starting at a column position, accounting for wide characters
fn extract_text_from_column(line: &str, start_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();
    let mut capturing = false;

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        if current_col >= start_col {
            capturing = true;
        }

        if capturing {
            result.push(ch);
        }

        current_col += char_width;
    }

    result
}

/// Extract text from a line up to a column position, accounting for wide characters
fn extract_text_to_column(line: &str, end_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        if current_col >= end_col {
            break;
        }

        result.push(ch);
        current_col += char_width;
    }

    result
}

impl PaneContents {
    pub fn new(viewport: Vec<String>, selection_start: Position, selection_end: Position) -> Self {
        PaneContents {
            viewport,
            selected_text: SelectedText::from_positions(selection_start, selection_end),
            ..Default::default()
        }
    }
    pub fn new_with_scrollback(
        viewport: Vec<String>,
        selection_start: Position,
        selection_end: Position,
        lines_above_viewport: Vec<String>,
        lines_below_viewport: Vec<String>,
    ) -> Self {
        PaneContents {
            viewport,
            selected_text: SelectedText::from_positions(selection_start, selection_end),
            lines_above_viewport,
            lines_below_viewport,
            ..Default::default()
        }
    }

    /// Returns the actual text content of the selection, if any exists.
    /// Selection only occurs within the viewport.
    pub fn get_selected_text(&self) -> Option<String> {
        let selected_text = self.selected_text?;

        let start_line = selected_text.start.line() as usize;
        let start_col = selected_text.start.column();
        let end_line = selected_text.end.line() as usize;
        let end_col = selected_text.end.column();

        // Handle out of bounds
        if start_line >= self.viewport.len() || end_line >= self.viewport.len() {
            return None;
        }

        if start_line == end_line {
            // Single line selection
            let line = &self.viewport[start_line];
            Some(extract_text_by_columns(line, start_col, end_col))
        } else {
            // Multi-line selection
            let mut result = String::new();

            // First line - from start column to end of line
            let first_line = &self.viewport[start_line];
            result.push_str(&extract_text_from_column(first_line, start_col));
            result.push('\n');

            // Middle lines - complete lines
            for i in (start_line + 1)..end_line {
                result.push_str(&self.viewport[i]);
                result.push('\n');
            }

            // Last line - from start to end column
            let last_line = &self.viewport[end_line];
            result.push_str(&extract_text_to_column(last_line, end_col));

            Some(result)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaneScrollbackResponse {
    Ok(PaneContents),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPanePidResponse {
    Ok(#[serde(with = "crate::internal_ids::process_id_legacy_signed")] ProcessId),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneRunningCommandResponse {
    Ok(Vec<String>),
    Err(String),
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionListSnapshot {
    pub live_sessions: Vec<SessionInfo>,
    pub resurrectable_sessions: Vec<(SessionName, Duration)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetSessionListResponse {
    Ok(SessionListSnapshot),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KillSessionsResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeleteDeadSessionResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeleteAllDeadSessionsResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneCwdResponse {
    Ok(PathBuf),
    Err(String),
}

/// What Git said about a remote pane's working directory, unparsed.
///
/// Raw on purpose. `agent-service`'s `poll::repository` already turns exactly
/// these three things into a checkout identity, including the awkward parts — a
/// repository with no commits writes its identity to stdout and *then* exits
/// 128, so the exit code cannot gate the parse. Shipping the bytes means that
/// logic stays in one place and keeps working for remote panes without knowing
/// they exist.
///
/// A projection of the server's `RemoteGitProbe`, which lives in `yinz-server`
/// and is not part of the plugin wire API.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PaneGitProbe {
    /// The machine this was run on, so two checkouts at the same path on
    /// different hosts stay distinct.
    pub host: String,
    /// The folder the probe was run in, which is a path on `host`.
    ///
    /// Not necessarily the pane's folder *now*: the far machine probes on its
    /// own clock, so a pane that has just moved reports the folder it was in.
    /// Whoever files the answer keys it by this, not by where the pane is.
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub metadata: Vec<u8>,
    pub config: Vec<u8>,
}

/// The answer to `get_pane_git_probe`.
///
/// A local pane is an `Err`: nothing on this machine has a cached probe to give,
/// and a plugin wanting one runs Git itself. That is not a failure worth
/// special-casing — callers branch on `PaneInfo::host` before asking — but it is
/// worth saying rather than returning an empty answer that reads as "not a
/// repository".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneGitProbeResponse {
    Ok(PaneGitProbe),
    Pending,
    Err(String),
}

/// The outcome of moving a pane between sessions, in either direction.
///
/// Both variants carry prose meant to be shown to whoever asked. The failures in
/// particular are not codes to be matched on: when a warp fails after the pane's
/// supervisor has been released, the message is the only place the recovery
/// instructions exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WarpPaneResponse {
    Ok(String),
    Err(String),
}

/// A pane whose session died but whose process is still running.
///
/// A projection of the server's own `HostStatus` rather than that type itself,
/// which lives in `yinz-server` and carries a host pid and a socket path a plugin has no use for.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OrphanedPane {
    /// The id `adopt_orphaned_pane` takes, and that `yinz pty-host adopt` on
    /// the command line takes too.
    pub host_id: SupervisorId,
    /// The session it belonged to, if the supervisor was ever told one.
    pub session_name: Option<SessionName>,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub child_pid: Option<ProcessId>,
    pub started_at_unix: UnixSeconds,
    /// `None` while the child is still running; `Some(None)` once it has been
    /// killed by a signal; `Some(Some(code))` once it has exited.
    pub exit_status: Option<Option<i32>>,
    /// Output the supervisor caught while nobody was reading, which the adopting
    /// session will replay.
    pub buffered_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ListOrphanedPanesResponse {
    Ok(Vec<OrphanedPane>),
    Err(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetFocusedPaneInfoResponse {
    Ok { tab_id: TabId, pane_id: PaneId },
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeleteLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenameLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedText {
    pub start: Position,
    pub end: Position,
}

impl SelectedText {
    pub fn new(start: Position, end: Position) -> Self {
        // Normalize: ensure start <= end
        let (normalized_start, normalized_end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        // Normalize negative line values to 0
        // (column is already usize so can't be negative)
        let normalized_start = Position::new(
            normalized_start.line().max(0) as i32,
            normalized_start.column() as u16,
        );
        let normalized_end = Position::new(
            normalized_end.line().max(0) as i32,
            normalized_end.column() as u16,
        );

        SelectedText {
            start: normalized_start,
            end: normalized_end,
        }
    }

    pub fn from_positions(start: Position, end: Position) -> Option<Self> {
        if start == end {
            None
        } else {
            Some(Self::new(start, end))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginIds {
    pub plugin_id: crate::internal_ids::PluginId,
    pub yinz_pid: ProcessId,
    pub initial_cwd: PathBuf,
    pub client_id: ClientId,
    /// Short hostname of the machine running this session's server, empty if unavailable
    pub hostname: String,
    /// A stable, opaque id for the installation running this session's server.
    ///
    /// Unlike `hostname` this is meant to be *stored*, not shown: it is what lets a plugin
    /// writing to a data store shared between machines say which machine a row came from, and
    /// tell its own rows from another server's. Two servers sharing a data dir share this id,
    /// because they also share the pane ids and session names it qualifies.
    ///
    /// See `yinz_runtime::host_identity`.
    pub host_id: HostId,
    /// Maximum session-name bytes after reserving the host's socket directory and terminator.
    /// Absent when the host predates this field.
    #[serde(default)]
    pub max_session_name_bytes: Option<usize>,
}

/// Tag used to identify the plugin in layout and config kdl files
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd, Ord)]
pub struct PluginTag(String);

impl PluginTag {
    pub fn new(url: impl Into<String>) -> Self {
        PluginTag(url.into())
    }
}

impl From<PluginTag> for String {
    fn from(tag: PluginTag) -> Self {
        tag.0
    }
}

impl fmt::Display for PluginTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginCapabilities {
    pub arrow_fonts: bool,
    /// Whether the host terminal can display kitty graphics, so a plugin knows whether drawing an
    /// image is worth doing or whether it should fall back to text. Unlike `arrow_fonts` this is
    /// not known at startup: it is learned from the client's probe reply and pushed out in a later
    /// `ModeUpdate`, so it starts `false` and turns on.
    pub kitty_graphics: bool,
}

impl Default for PluginCapabilities {
    fn default() -> PluginCapabilities {
        PluginCapabilities {
            arrow_fonts: true,
            kitty_graphics: false,
        }
    }
}

/// Represents a Clipboard type
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum CopyDestination {
    Command,
    Primary,
    System,
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum PermissionStatus {
    Granted,
    Denied,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileToOpen {
    pub path: PathBuf,
    pub line_number: Option<usize>,
    pub cwd: Option<PathBuf>,
}

impl FileToOpen {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        FileToOpen {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        }
    }
    pub fn with_line_number(mut self, line_number: usize) -> Self {
        self.line_number = Some(line_number);
        self
    }
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }
}

use crate::command::{CommandToRun, PaneTarget};

/// What a headless hook point hands the plugin bound to it, typed per point.
///
/// Untagged on the wire a plugin reads (`yinz events`, a TypeScript plugin's `event.payload`), so a
/// hook reads `payload.pane_id` and `payload.agent.state` rather than a variant name; the
/// variants are told apart by the fields only one of them has. The `agent_exit` point is the
/// exception to all of this: its plugin is opened *in place of* the pane, and its payload is
/// the plugin's configuration - a string map, documented with that point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookPayload {
    /// `agent_start`, `agent_waiting` and `agent_idle`: the record as it now stands, and the
    /// state it moved from - absent for a record that just appeared.
    Agent {
        pane_id: crate::internal_ids::TerminalId,
        agent: AgentInfo,
        previous_state: Option<AgentState>,
        /// The command the pane runs, for display, when it is a command pane.
        command: Option<String>,
    },
    /// `pane_exit`: any command pane's command exited. `held` says whether the pane is still
    /// there to be re-run, or is closing with its command.
    PaneExit {
        pane_id: crate::internal_ids::TerminalId,
        /// Absent when the command was killed by a signal.
        exit_code: Option<i32>,
        cwd: Option<PathBuf>,
        command: Option<String>,
        held: bool,
    },
    /// `session_start`: the session's first client connected.
    SessionStart {
        session_name: SessionName,
        client_id: ClientId,
    },
}

pub use crate::agent::{AgentSpawnSpec, SpawnedAgent};

use crate::command::GitCheckout;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageToPlugin {
    pub plugin_url: Option<String>,
    pub destination_plugin_id: Option<crate::internal_ids::PluginId>,
    pub plugin_config: BTreeMap<String, String>,
    pub message_name: String,
    pub message_payload: Option<String>,
    pub message_args: BTreeMap<String, String>,
    /// these will only be used in case we need to launch a new plugin to send this message to,
    /// since none are running
    pub new_plugin_args: Option<NewPluginArgs>,
    pub floating_pane_coordinates: Option<FloatingPaneCoordinates>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewPluginArgs {
    pub should_float: Option<bool>,
    pub pane_id_to_replace: Option<PaneId>,
    pub pane_title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub skip_cache: bool,
    pub should_focus: Option<bool>,
}

pub use crate::pane_id::PaneId;

impl MessageToPlugin {
    pub fn new(message_name: impl Into<String>) -> Self {
        MessageToPlugin {
            message_name: message_name.into(),
            ..Default::default()
        }
    }
    pub fn with_plugin_url(mut self, url: impl Into<String>) -> Self {
        self.plugin_url = Some(url.into());
        self
    }
    pub fn with_destination_plugin_id(
        mut self,
        destination_plugin_id: crate::internal_ids::PluginId,
    ) -> Self {
        self.destination_plugin_id = Some(destination_plugin_id);
        self
    }
    pub fn with_plugin_config(mut self, plugin_config: BTreeMap<String, String>) -> Self {
        self.plugin_config = plugin_config;
        self
    }
    pub fn with_payload(mut self, payload: impl Into<String>) -> Self {
        self.message_payload = Some(payload.into());
        self
    }
    pub fn with_args(mut self, args: BTreeMap<String, String>) -> Self {
        self.message_args = args;
        self
    }
    pub fn with_floating_pane_coordinates(
        mut self,
        floating_pane_coordinates: FloatingPaneCoordinates,
    ) -> Self {
        self.floating_pane_coordinates = Some(floating_pane_coordinates);
        self
    }
    pub fn new_plugin_instance_should_float(mut self, should_float: bool) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_float = Some(should_float);
        self
    }
    pub fn new_plugin_instance_should_replace_pane(mut self, pane_id: PaneId) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.pane_id_to_replace = Some(pane_id);
        self
    }
    pub fn new_plugin_instance_should_have_pane_title(
        mut self,
        pane_title: impl Into<String>,
    ) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.pane_title = Some(pane_title.into());
        self
    }
    pub fn new_plugin_instance_should_have_cwd(mut self, cwd: PathBuf) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.cwd = Some(cwd);
        self
    }
    pub fn new_plugin_instance_should_skip_cache(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.skip_cache = true;
        self
    }
    pub fn new_plugin_instance_should_be_focused(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_focus = Some(true);
        self
    }
    /// The other half of `new_plugin_instance_should_be_focused`, for a message that opens a
    /// plugin the sender wants running rather than looked at. Distinct from leaving
    /// `should_focus` unset, which lets the launch decide.
    pub fn new_plugin_instance_should_not_be_focused(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_focus = Some(false);
        self
    }
    pub fn has_cwd(&self) -> bool {
        self.new_plugin_args
            .as_ref()
            .map(|n| n.cwd.is_some())
            .unwrap_or(false)
    }
}

/// A plugin session-switch request. The requested tab position is zero-based.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectToSession {
    pub name: Option<SessionName>,
    pub tab_position: Option<TabPosition>,
    #[serde(default, with = "crate::pane_id::legacy_tuple")]
    pub pane_id: Option<PaneId>,
    pub layout: Option<LayoutInfo>,
    pub cwd: Option<PathBuf>,
}

/// A client reconnect target. Tab numbers are one-based on the client/server protocol.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionConnection {
    pub name: Option<SessionName>,
    pub tab_position: Option<TabNumber>,
    #[serde(default, with = "crate::pane_id::legacy_tuple")]
    pub pane_id: Option<PaneId>,
    pub layout: Option<LayoutInfo>,
    pub cwd: Option<PathBuf>,
}

impl SessionConnection {
    pub fn apply_layout_dir(&mut self, layout_dir: &Path) {
        if let Some(LayoutInfo::File(file_path, _layout_metadata)) = self.layout.as_mut() {
            *file_path = Path::join(layout_dir, &file_path)
                .to_string_lossy()
                .to_string();
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMessage {
    pub name: String,
    pub payload: String,
    pub worker_name: Option<String>,
}

impl PluginMessage {
    pub fn new_to_worker(worker_name: &str, message: &str, payload: &str) -> Self {
        PluginMessage {
            name: message.to_owned(),
            payload: payload.to_owned(),
            worker_name: Some(worker_name.to_owned()),
        }
    }
    pub fn new_to_plugin(message: &str, payload: &str) -> Self {
        PluginMessage {
            name: message.to_owned(),
            payload: payload.to_owned(),
            worker_name: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PipeSource {
    Cli(String), // String is the pipe_id of the CLI pipe (used for blocking/unblocking)
    Plugin(crate::internal_ids::PluginId), // the sending plugin
    Keybind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipeMessage {
    pub source: PipeSource,
    pub name: String,
    pub payload: Option<String>,
    pub args: BTreeMap<String, String>,
    pub is_private: bool,
    /// The pane `yinz pipe` was run in, when the source is a CLI pipe and the caller was
    /// inside one: the client reads `YINZ_PANE_ID` and the server carries it here, so no
    /// integration has to hand-thread a `pane_id=` argument. Convenience, not authentication —
    /// the variable is as trustworthy as the process that set it.
    #[serde(default)]
    pub pane_id: Option<PaneId>,
}

impl PipeMessage {
    pub fn new(
        source: PipeSource,
        name: impl Into<String>,
        payload: &Option<String>,
        args: &Option<BTreeMap<String, String>>,
        is_private: bool,
    ) -> Self {
        PipeMessage {
            source,
            name: name.into(),
            payload: payload.clone(),
            args: args.clone().unwrap_or_default(),
            is_private,
            pane_id: None,
        }
    }
}

/// Where a warped or adopted pane should land in the session receiving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum WarpPlacement {
    /// A tab of its own. The default, because it is the only placement that
    /// cannot disturb a layout the receiving session already has.
    #[default]
    NewTab,
    /// A tiled split in whatever tab the receiving session has focused.
    FocusedTab,
    Floating,
}

/// Everything that has to travel with a pane moving between sessions, beyond what
/// the pane's own supervisor can already report.
///
/// Deliberately small. The running process, its command and its exit status are
/// all held by the pty-host, and the adopting server learns them from the
/// `AttachAck` — so this carries what lives in the *terminal emulator* plus the
/// server-side agent record attached to that process.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct WarpedPane {
    #[serde(default)]
    pub agent_theme: Option<crate::agent::theme::AgentThemeHandoff>,
    /// The emulator mode, restored after the ANSI dump and before buffered output.
    #[serde(default)]
    pub bracketed_paste: bool,
    #[serde(default)]
    pub agent_input: Option<WarpedAgentInput>,
    /// A terminal waiting for explicit user action; no process is started on arrival.
    #[serde(default)]
    pub held: Option<HeldTerminal>,
    /// A visible pane and its companions, using the same handoff as a tab.
    #[serde(default)]
    pub group: Option<Box<WarpedTab>>,
    /// Identifies the supervisor, for error messages and `pty-host adopt`.
    /// Empty for a remote pane, whose supervisor is not on this machine.
    pub host_id: SupervisorId,
    /// Where to attach. A supervisor that failed to bind a socket has no address
    /// to hand out, which is the one way a pane can be detachable but not
    /// transferable — the source refuses the warp rather than sending this empty.
    /// Empty for a remote pane; `remote` is its address instead.
    pub control_socket: PathBuf,
    /// The pane's title when the user or its program gave it one, so it does not arrive
    /// renamed. It arrives as the pane's name, which is explicit.
    pub title: Option<String>,
    /// The default caption ("Pane #3", or the command) a pane with no `title` was wearing. It
    /// arrives as a caption rather than a name, so it stays a default there - and a default is
    /// not lent to the tab, where it would outrank the project.
    #[serde(default)]
    pub caption: Option<String>,
    /// The screen and scrollback as an ANSI dump, replayed into the new pane the
    /// same way session resurrection restores one.
    pub contents: Option<String>,
    pub borderless: bool,
    /// `Some` when the pane's process is on another machine, and then the only
    /// address that means anything: the two fields above name a local
    /// supervisor socket, and there is none.
    pub remote: Option<RemotePaneHandoff>,
    /// The agent record belongs to the running process, so it moves with it.
    #[serde(default)]
    pub agent: Option<AgentInfo>,
}

/// Input evidence from a managed Codex pane, awaiting a fresh parent report.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WarpedAgentInput {
    pub kind: String,
    pub conversation: String,
    pub consumed: bool,
}

/// Saved state for a terminal that is not running.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct HeldTerminal {
    pub command: RunCommand,
    pub first_run: bool,
    pub exit_status: Option<i32>,
}

/// How to pick a remote pane back up from the broker that has been holding it.
///
/// The remote counterpart of `WarpedPane`'s `control_socket`, and the reason it
/// cannot simply *be* one: a broker is reached by ssh, not by a path, and the
/// adopting server builds its own forward rather than inheriting the releasing
/// server's.
///
/// Carries the command, which the local path does not have to: the adopting
/// server learns a local pane's command from its supervisor's `AttachAck`, but
/// the broker's answer to a resume carries none. Without it the pane would
/// arrive unable to say what it runs, unable to re-run on the right machine when
/// held, and unable to title itself.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct RemotePaneHandoff {
    /// The ssh destination the pane was opened against.
    pub host: String,
    /// How the broker on that host names this pane. Meaningful only there.
    pub pane_token: String,
    /// Output bytes the releasing session took delivery of. The broker replays
    /// from exactly here, which is what stops the pane's serialized screen and
    /// the replay from overlapping or leaving a gap between them.
    pub received: u64,
    pub cmd: RunCommand,
}

/// Everything that has to travel with a whole tab moving between sessions.
///
/// The arrangement travels as a layout rather than as pane geometry, which is
/// what lets a tab land in a session of a different size: a layout is
/// proportional, and the receiving session positions it against its own
/// viewport. This is the same representation session resurrection uses to
/// survive a resize.
///
/// The panes are split into two lists for the same reason `apply_layout` takes
/// two: tiled and floating slots are filled from separate queues, in separate
/// orders.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct WarpedTab {
    #[serde(default)]
    pub relationships: Option<WarpRelationships>,
    /// The tab's name, so it does not arrive renamed.
    pub name: Option<String>,
    /// Whether the user named the tab in the session it left. Only then does the name hold
    /// off the project and pane titles the receiving session would otherwise show.
    #[serde(default)]
    pub user_named: bool,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layout: Vec<FloatingPaneLayout>,
    /// One per released supervisor, ordered to match the layout's terminal slots
    /// — that is, `tiled_layout.extract_run_instructions()` order, which is what
    /// the receiving side's `position_new_panes` consumes ids in. Plugin panes
    /// are not here: they have no process, and are recreated from their
    /// `Run::Plugin` node instead.
    ///
    /// Only the parts of a `WarpedPane` the layout cannot express are filled in:
    /// which supervisor to attach to, and the pane's screen. Titles and borders
    /// travel as layout nodes, because that is where the receiving session looks
    /// for them when it builds the tab.
    pub tiled_panes: Vec<WarpedPane>,
    /// Ordered to match `floating_layout`, minus its plugin panes.
    pub floating_panes: Vec<WarpedPane>,
    pub hide_floating_panes: bool,
}

impl WarpedTab {
    pub fn validate_relationships(&self) -> Result<(), String> {
        let Some(relationships) = &self.relationships else {
            return Ok(());
        };
        let tiled = self.tiled_layout.extract_run_instructions();
        let terminal = |run: &Option<Run>| !matches!(run, Some(Run::Plugin(_)));
        if tiled.iter().filter(|run| terminal(run)).count() != self.tiled_panes.len()
            || self
                .floating_layout
                .iter()
                .filter(|pane| terminal(&pane.run))
                .count()
                != self.floating_panes.len()
            || self
                .tiled_panes
                .iter()
                .chain(&self.floating_panes)
                .any(|pane| pane.group.is_some())
        {
            return Err("warp terminal payloads do not match their layout slots".into());
        }
        relationships.validate(tiled.len() + self.floating_layout.len())
    }

    pub fn validate_pane_group(&self) -> Result<(), String> {
        self.validate_relationships()?;
        let relationships = self
            .relationships
            .as_ref()
            .ok_or_else(|| "pane group is missing relationships".to_owned())?;
        if self.tiled_layout.extract_run_instructions().len() != 1
            || relationships.hidden.len() != self.floating_layout.len()
            || relationships.hidden.iter().any(|pane| pane.pane == 0)
        {
            return Err("a pane group must have one visible root and hidden companions".into());
        }
        Ok(())
    }
}

/// References index all layout slots: tiled first, then floating, including plugins.
/// Hidden companions occupy extra floating slots until relationships are restored.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct WarpRelationships {
    pub hidden: Vec<WarpHiddenPane>,
    pub stacks: Vec<WarpStackList>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WarpHiddenPane {
    pub pane: u32,
    pub restored_by: Option<u32>,
    pub is_scrollback_editor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WarpStackList {
    pub members: Vec<u32>,
    pub selected: u32,
}

impl WarpRelationships {
    pub fn validate(&self, slots: usize) -> Result<(), String> {
        use std::collections::{HashMap, HashSet};
        let valid = |id: u32| (id as usize) < slots;
        let mut hidden = HashMap::new();
        let mut covers = HashSet::new();
        for pane in &self.hidden {
            if !valid(pane.pane) || hidden.insert(pane.pane, pane.restored_by).is_some() {
                return Err("invalid or duplicate hidden warp pane".into());
            }
            if let Some(cover) = pane.restored_by {
                if !valid(cover) || !covers.insert(cover) {
                    return Err("invalid or duplicate warp cover".into());
                }
            } else if pane.is_scrollback_editor {
                return Err("a parked pane cannot have an editor flag".into());
            }
        }
        for pane in hidden.keys() {
            let mut seen = HashSet::new();
            let mut next = Some(*pane);
            while let Some(id) = next {
                if !seen.insert(id) {
                    return Err("cyclic warp replacement chain".into());
                }
                next = hidden.get(&id).copied().flatten();
            }
        }
        let mut members = HashSet::new();
        for stack in &self.stacks {
            if stack.members.len() < 2
                || !stack.members.contains(&stack.selected)
                || hidden.contains_key(&stack.selected)
            {
                return Err("invalid selected warp stack member".into());
            }
            for id in &stack.members {
                if !valid(*id)
                    || !members.insert(*id)
                    || (*id != stack.selected && hidden.get(id) != Some(&None))
                {
                    return Err("invalid or duplicate warp stack member".into());
                }
            }
        }
        Ok(())
    }
}

use crate::geometry::FloatingPaneCoordinates;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginatingPlugin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub plugin_id: crate::internal_ids::PluginId,
    pub client_id: ClientId,
    pub context: Context,
}

/// Marks command panes opened by the new/duplicate agent actions.
pub const AGENT_PANE_CONTEXT: &str = "agent_pane";

impl OriginatingPlugin {
    pub fn new(
        plugin_id: crate::internal_ids::PluginId,
        client_id: ClientId,
        context: Context,
    ) -> Self {
        OriginatingPlugin {
            generation: None,
            plugin_id,
            client_id,
            context,
        }
    }
}

/// What a pane drawn over the whole session surface does when the user's attention moves
/// elsewhere.
///
/// The distinction used to be a string in the plugin's own configuration map, read back out of
/// `Pane::invoked_with()` at every decision point - which meant only a plugin pane could ever be
/// modal, and that a caller which rebuilt a pane's coordinates silently dropped it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverlayKind {
    /// Watches `PaneUpdate` and closes itself once it sees it is no longer focused, so focus may
    /// be taken from it freely - `file-finder`.
    #[default]
    Dismissible,
    /// Closes only on Esc, on submit, or on `Event::Visible(false)`. It has no way to learn that
    /// it lost focus, so taking focus from it strands it on screen while the keyboard goes
    /// somewhere else - `new-agent`, `quit-confirmation`.
    Modal,
}

impl OverlayKind {
    pub fn is_modal(&self) -> bool {
        matches!(self, OverlayKind::Modal)
    }
}

impl FromStr for OverlayKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dismissible" | "Dismissible" | "overlay" => Ok(OverlayKind::Dismissible),
            "modal" | "Modal" => Ok(OverlayKind::Modal),
            _ => Err(format!("No such overlay kind: {s}")),
        }
    }
}

/// A request to place a pane on the session surface rather than on a tab's floating layer.
///
/// The placement is the thing that says what the pane is, so
/// that overlay-ness cannot be lost by a caller that rebuilds the coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPlacement {
    pub kind: OverlayKind,
    /// Where on the surface. `None` centres it at a default size.
    #[serde(default)]
    pub coordinates: Option<FloatingPaneCoordinates>,
    /// Values below 100 opt into global background_opacity; the requested percentage
    /// does not override it. Unspecified or explicit 100 keeps the overlay opaque.
    #[serde(default)]
    pub background_opacity: Option<u8>,
}

impl OverlayPlacement {
    pub fn new(kind: OverlayKind, coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement {
            kind,
            coordinates,
            background_opacity: None,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_background_opacity(self.background_opacity)
    }
    pub fn modal(coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement::new(OverlayKind::Modal, coordinates)
    }
    pub fn dismissible(coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement::new(OverlayKind::Dismissible, coordinates)
    }
    pub fn is_modal(&self) -> bool {
        self.kind.is_modal()
    }
    /// Where on the surface this overlay sits, defaulted if it did not say.
    ///
    /// Only the geometry: what makes it an overlay is the placement itself, and which kind of
    /// overlay is `kind`. Coordinates cannot express either.
    pub fn as_floating_pane_coordinates(&self) -> FloatingPaneCoordinates {
        self.coordinates.clone().unwrap_or_default()
    }
}

pub fn validate_background_opacity(opacity: Option<u8>) -> Result<(), &'static str> {
    if opacity.is_some_and(|opacity| opacity > 100) {
        Err("background opacity must be between 0 and 100")
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NewPanePlacement {
    NoPreference {
        borderless: Option<bool>,
    },
    Tiled {
        direction: Option<Direction>,
        borderless: Option<bool>,
    },
    Floating(Option<FloatingPaneCoordinates>),
    InPlace {
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
        borderless: Option<bool>,
    },
    Stacked {
        pane_id_to_stack_under: Option<PaneId>,
        borderless: Option<bool>,
    },
    /// Over the whole session surface - not on the tab's floating layer.
    Overlay(OverlayPlacement),
}

impl Default for NewPanePlacement {
    fn default() -> Self {
        NewPanePlacement::NoPreference { borderless: None }
    }
}

impl NewPanePlacement {
    /// The placement a `--floating` CLI invocation asks for.
    ///
    /// `--modal` and `--overlay` both put the pane on the session surface rather than on the
    /// tab's floating layer; they differ only in whether focus may be taken from it. `--modal`
    /// wins if both are given, being the stricter of the two.
    #[allow(clippy::too_many_arguments)]
    pub fn floating_or_overlay(
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        pinned: Option<bool>,
        borderless: Option<bool>,
        overlay: Option<bool>,
        modal: Option<bool>,
    ) -> Self {
        let is_modal = modal.unwrap_or(false);
        let is_overlay = is_modal || overlay.unwrap_or(false);
        let coordinates = FloatingPaneCoordinates::new(x, y, width, height, pinned, borderless);
        if is_overlay {
            NewPanePlacement::Overlay(OverlayPlacement {
                kind: if is_modal {
                    OverlayKind::Modal
                } else {
                    OverlayKind::Dismissible
                },
                coordinates,
                background_opacity: None,
            })
        } else {
            NewPanePlacement::Floating(coordinates)
        }
    }
    pub fn with_floating_pane_coordinates(
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    ) -> Self {
        NewPanePlacement::Floating(floating_pane_coordinates)
    }
    pub fn with_should_be_in_place(
        self,
        should_be_in_place: bool,
        close_replaced_pane: bool,
    ) -> Self {
        if should_be_in_place {
            NewPanePlacement::InPlace {
                pane_id_to_replace: None,
                close_replaced_pane,
                borderless: None,
            }
        } else {
            self
        }
    }
    pub fn with_pane_id_to_replace(
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
    ) -> Self {
        NewPanePlacement::InPlace {
            pane_id_to_replace,
            close_replaced_pane,
            borderless: None,
        }
    }
    pub fn should_float(&self) -> Option<bool> {
        match self {
            NewPanePlacement::Floating(_) => Some(true),
            NewPanePlacement::Tiled { .. } => Some(false),
            _ => None,
        }
    }
    /// Where on a surface this pane sits, for the two placements that have coordinates at all:
    /// the tab's floating layer and the session surface an overlay is drawn on.
    pub fn floating_pane_coordinates(&self) -> Option<FloatingPaneCoordinates> {
        match self {
            NewPanePlacement::Floating(floating_pane_coordinates) => {
                floating_pane_coordinates.clone()
            },
            NewPanePlacement::Overlay(overlay) => overlay.coordinates.clone(),
            _ => None,
        }
    }
    pub fn should_stack(&self) -> bool {
        matches!(self, NewPanePlacement::Stacked { .. })
    }
    pub fn id_of_stack_root(&self) -> Option<PaneId> {
        match self {
            NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                ..
            } => *pane_id_to_stack_under,
            _ => None,
        }
    }
    pub fn get_borderless(&self) -> Option<bool> {
        match self {
            NewPanePlacement::NoPreference { borderless } => *borderless,
            NewPanePlacement::Tiled { borderless, .. } => *borderless,
            NewPanePlacement::Floating(coords) => coords.as_ref().and_then(|c| c.borderless),
            NewPanePlacement::InPlace { borderless, .. } => *borderless,
            NewPanePlacement::Stacked { borderless, .. } => *borderless,
            NewPanePlacement::Overlay(overlay) => {
                overlay.coordinates.as_ref().and_then(|c| c.borderless)
            },
        }
    }
}

type Context = BTreeMap<String, String>;

/// Bytes that may be written as a string.
///
/// Plugin commands accept text as UTF-8 bytes and lists of integer byte values.
/// This preserves convenient text writes alongside exact binary writes.
mod bytes_or_string {
    use serde::de::{Deserializer, Error, SeqAccess, Visitor};
    use serde::Serializer;
    use std::fmt;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct BytesOrString;

        impl<'de> Visitor<'de> for BytesOrString {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or an array of byte values")
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(value.as_bytes().to_vec())
            }

            fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(value.to_vec())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(byte) = seq.next_element::<u8>()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(BytesOrString)
    }
}

/// What a notification's own plugin is told when somebody clicks its toast.
///
/// A toast is drawn over whatever pane the user is actually in, so a click on one is the only
/// gesture that says "the thing this toast was about, now". Focusing the toast's `PaneId` says
/// *where*; this says *what*, and is delivered to the plugin that raised the notification as an
/// `Event::CustomMessage(message, payload)`. Without it a plugin cannot tell a click on its toast
/// from any other route into its pane - a keybinding, another plugin's focus - and the two mean
/// different things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyClick {
    pub message: String,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, EnumDiscriminants, Display, Serialize, Deserialize)]
#[strum_discriminants(derive(EnumString, EnumIter, Display, Hash, Serialize, Deserialize))]
#[strum_discriminants(name(CommandType))]
#[allow(clippy::large_enum_variant)]
pub enum PluginCommand {
    InspectConfiguration(Option<String>),
    EditConfiguration(crate::configuration::Edit),
    /// Private command-palette transport between the runtime and host.
    CommandPalette(crate::command_palette::Command),
    PaneTabs(crate::pane_tabs::PaneTabsCommand),
    CreateMultipane(crate::multipane::CreateMultipane),
    UpdateMultipane(crate::multipane::UpdateMultipane),
    ReplaceMultipaneChild(crate::multipane::ReplaceMultipaneChild),
    Subscribe(HashSet<EventType>),
    Unsubscribe(HashSet<EventType>),
    SetSelectable(bool),
    ShowCursor(Option<(usize, usize)>),
    GetPluginIds,
    GetCellPixelSize,
    GetYinzVersion,
    OpenFile(FileToOpen, Context),
    OpenFileFloating(FileToOpen, Option<FloatingPaneCoordinates>, Context),
    OpenTerminal(FileToOpen), // only used for the path as cwd
    OpenTerminalFloating(FileToOpen, Option<FloatingPaneCoordinates>), // only used for the path as cwd
    OpenCommandPane(CommandToRun, Context),
    OpenCommandPaneFloating(CommandToRun, Option<FloatingPaneCoordinates>, Context),
    /// Open a command pane directly onto the session surface, above the tab's own panes. A terminal
    /// pane carries no plugin configuration, so the placement is the only thing that can say it
    /// is an overlay - and which kind.
    OpenCommandPaneOverlay(CommandToRun, OverlayPlacement, Context),
    SwitchTabTo(TabNumber),        // tab index
    SetTimeout(FractionalSeconds), // seconds
    ExecCmd(Vec<String>),
    PostMessageTo(PluginMessage),
    PostMessageToPlugin(PluginMessage),
    HideSelf,
    ShowSelf(bool), // bool - should float if hidden
    SwitchToMode(InputMode),
    NewTabsWithLayout(String), // raw kdl layout
    NewTab {
        name: Option<String>,
        cwd: Option<String>,
    },
    NewTabUnfocused {
        name: Option<String>,
        cwd: Option<String>,
    },
    NewTiledPaneInTab {
        tab_id: TabId,
    },
    ToggleFloatingPanes {
        tab_id: Option<TabId>,
    },
    NewPane,
    GoToNextTab,
    GoToPreviousTab,
    Resize(Resize),
    ResizeWithDirection(ResizeStrategy),
    FocusNextPane,
    FocusPreviousPane,
    FocusLastPane,
    MoveFocus(Direction),
    MoveFocusOrTab(Direction),
    Detach,
    EditScrollback,
    Write(Vec<u8>), // bytes
    WriteChars(String),
    ToggleTab,
    MovePane,
    MovePaneWithDirection(Direction),
    ClearScreen,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    PageScrollUp,
    PageScrollDown,
    ToggleFocusFullscreen,
    ToggleFocusNoUiFullscreen,
    TogglePaneFrames,
    SetPaneFrameStyle(PaneFrameStyle),
    TogglePaneEmbedOrEject,
    UndoRenamePane,
    CloseFocus,
    ToggleActiveTabSync,
    CloseFocusedTab,
    UndoRenameTab,
    QuitYinz,
    PreviousSwapLayout,
    NextSwapLayout,
    GoToTabName(String),
    FocusOrCreateTab(String),
    GoToTab(TabPosition),                                           // tab index
    StartOrReloadPlugin(String), // plugin url (eg. file:/path/to/plugin.scm)
    CloseTerminalPane(crate::internal_ids::TerminalId), // terminal pane id
    ClosePluginPane(crate::internal_ids::PluginId), // plugin pane id
    FocusTerminalPane(crate::internal_ids::TerminalId, bool, bool), // terminal pane id, should_float_if_hidden, should_be_in_place_if_hidden
    FocusPluginPane(crate::internal_ids::PluginId, bool, bool), // plugin pane id, should_float_if_hidden, should_be_in_place_if_hidden
    RenameTerminalPane(crate::internal_ids::TerminalId, String), // terminal pane id, new name
    RenamePluginPane(crate::internal_ids::PluginId, String),    // plugin pane id, new name
    RenameTab(TabPosition, String),                             // tab index, new name
    ReportPanic(String),                                        // stringified panic
    RequestPluginPermissions(Vec<PermissionType>),
    SwitchSession(ConnectToSession),
    DeleteDeadSession(SessionName),  // String -> session name
    DeleteAllDeadSessions,           // String -> session name
    OpenTerminalInPlace(FileToOpen), // only used for the path as cwd
    OpenFileInPlace(FileToOpen, Context),
    OpenCommandPaneInPlace(CommandToRun, Context),
    SubscribePaneOutput(Option<BTreeMap<String, String>>),
    /// Guarded agent control. Requires RunCommands; replies use correlated CustomMessage events.
    AgentControl {
        id: String,
        request: String,
    },
    RunCommand(
        Vec<String>,              // command
        BTreeMap<String, String>, // env_variables
        PathBuf,                  // cwd
        BTreeMap<String, String>, // context
        Option<usize>,            // max bytes returned for each output stream
        Option<bool>,             // opt-in chunk events
    ),
    WebRequest(
        String, // url
        HttpVerb,
        BTreeMap<String, String>, // headers
        Vec<u8>,                  // body
        BTreeMap<String, String>, // context
        Option<String>,           // stream_id: Some(_) => stream the response body back as
        // WebRequestStreamChunk events under this id
        Option<String>, // proxy: Some(_) => send this request through the given
                        // proxy URL (e.g. "socks5://host:port") instead of direct
    ),
    CancelWebRequest(String),   // String -> stream_id
    RenameSession(SessionName), // String -> new session name
    Notify(String, Option<PaneId>, Option<NotifyClick>), // message, pane to focus on click, what
    // to tell the raising plugin when it is clicked
    UnblockCliPipeInput(String),   // String => pipe name
    BlockCliPipeInput(String),     // String => pipe name
    CliPipeOutput(String, String), // String => pipe name, String => output
    MessageToPlugin(MessageToPlugin),
    DisconnectOtherClients,
    KillSessions(Vec<SessionName>), // one or more session names
    ScanHostFolder(PathBuf),
    WatchFilesystem,
    /// Read a file, answering with its bytes.
    ///
    /// Paths use virtual roots such as `/host`, `/data`, `/cache` and `/tmp`.
    /// The host confines each path to its root. Access outside the roots requires
    /// `FullHdAccess` in addition to the operation's read or write permission.
    ///
    /// Answers synchronously, so that a plugin can read a config file in one expression rather
    /// than through a subscription. `max_bytes` caps the read; the host applies its own ceiling
    /// regardless, so this can only ask for less. Without `offset`, oversized files fail rather
    /// than return a truncated file. With `offset`, return a bounded range starting at that byte,
    /// possibly empty at EOF. Each call opens the file afresh; ranges are not a snapshot.
    FsRead {
        path: PathBuf,
        max_bytes: Option<usize>,
        offset: Option<u64>,
    },
    /// List a directory. Answers with one entry per child, its path spelled in the same virtual
    /// root the request used, so a plugin can feed the answer straight back in.
    FsList {
        path: PathBuf,
    },
    /// Metadata for one path, or nothing if it does not exist.
    FsMetadata {
        path: PathBuf,
        /// Follow the final symlink, while retaining the same root and permission checks.
        #[serde(default)]
        follow_symlinks: bool,
    },
    /// Write a file, replacing it or appending to it.
    FsWrite {
        path: PathBuf,
        #[serde(with = "bytes_or_string")]
        contents: Vec<u8>,
        append: bool,
        create_dirs: bool,
    },
    /// Remove a file, or a directory when `recursive`.
    FsRemove {
        path: PathBuf,
        recursive: bool,
    },
    /// Create a directory. `all` creates missing parents and succeeds if it already exists.
    FsCreateDir {
        path: PathBuf,
        all: bool,
    },
    /// Rename a path. Both endpoints are resolved and confined independently.
    FsRename {
        from: PathBuf,
        to: PathBuf,
    },
    /// Grant `permissions` to the plugin at `plugin_url` ahead of it asking, as if the user had
    /// answered its prompt. Builtins only: this is how a plugin that authors other plugins
    /// carries the user's approval of the source over to the permissions that source asks for.
    GrantPluginPermissions {
        plugin_url: String,
        permissions: Vec<PermissionType>,
    },
    DumpSessionLayout {
        tab_index: Option<TabId>,
    },
    CloseSelf,
    NewTabsWithLayoutInfo(LayoutInfo),
    Reconfigure(String, bool), // String -> stringified configuration, bool -> save configuration
    // file to disk
    HidePaneWithId(PaneId),
    ShowPaneWithId(PaneId, bool, bool), // bools -> should_float_if_hidden, should_focus_pane
    OpenCommandPaneBackground(CommandToRun, Context),
    RerunCommandPane(crate::internal_ids::TerminalId), // u32  - terminal pane id
    /// Run a different command in an existing (held) command pane, reusing its pane id. The
    /// `context` round-trips on the resulting `CommandPaneExited`, so each step of a sequence can
    /// carry its own correlation keys. When the pane's current command is still running, the
    /// replacement starts when it exits (last write wins).
    RunCommandInPane {
        pane_id: PaneId,
        command: CommandToRun,
        context: Context,
        #[serde(default)]
        keep_scrollback: bool,
    },
    ResizePaneIdWithDirection(ResizeStrategy, PaneId),
    EditScrollbackForPaneWithId(PaneId),
    GetPaneScrollback {
        pane_id: PaneId,
        get_full_scrollback: bool,
    },
    WriteToPaneId(Vec<u8>, PaneId),
    WriteCharsToPaneId(String, PaneId),
    SendSigintToPaneId(PaneId),
    SendSigkillToPaneId(PaneId),
    GetPanePid {
        pane_id: PaneId,
    },
    GetPaneRunningCommand {
        pane_id: PaneId,
    },
    GetPaneCwd {
        pane_id: PaneId,
    },
    /// What Git says about a *remote* pane's folder, as its own machine sees it.
    GetPaneGitProbe {
        pane_id: PaneId,
        cwd: String,
        refresh_token: Option<String>,
    },
    MovePaneWithPaneId(PaneId),
    MovePaneWithPaneIdInDirection(PaneId, Direction),
    ClearScreenForPaneId(PaneId),
    ScrollUpInPaneId(PaneId),
    ScrollDownInPaneId(PaneId),
    ScrollToTopInPaneId(PaneId),
    ScrollToBottomInPaneId(PaneId),
    PageScrollUpInPaneId(PaneId),
    PageScrollDownInPaneId(PaneId),
    TogglePaneIdFullscreen(PaneId),
    TogglePaneEmbedOrEjectForPaneId(PaneId),
    CloseTabWithIndex(TabPosition), // usize - tab_index
    BreakPanesToNewTab(Vec<PaneId>, Option<String>, bool), // bool -
    // should_change_focus_to_new_tab,
    // Option<String> - optional name for
    // the new tab
    BreakPanesToTabWithIndex(Vec<PaneId>, TabPosition, bool), // usize - tab_index, bool -
    // should_change_focus_to_new_tab
    SwitchTabToId(TabId),           // stable tab id
    GoToTabWithId(TabId),           // stable tab id
    CloseTabWithId(TabId),          // stable tab id
    RenameTabWithId(TabId, String), // stable tab id, String - new name
    /// The project each tab is working on, keyed by tab id.
    ///
    /// Advisory rather than a rename: the server shows these to fill in tabs the
    /// user has not named, and never writes them to the tab. Sent as the whole
    /// map, so a tab left out of it has no project.
    ArrangeProjectPanes(Vec<ProjectPaneGroup>),
    SetTabProjects(BTreeMap<TabId, String>),
    SetProjectContext(Vec<String>, Vec<(PaneId, String)>),
    BreakPanesToTabWithId(Vec<PaneId>, TabId, bool), // stable tab id, bool -
    // should_change_focus_to_target_tab
    ReloadPlugin(crate::internal_ids::PluginId), // u32 - plugin pane id
    LoadNewPlugin {
        url: String,
        config: BTreeMap<String, String>,
        load_in_background: bool,
        skip_plugin_cache: bool,
    },
    RebindKeys {
        keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
        keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
        write_config_to_disk: bool,
    },
    RegisterDefaultKeybindings(String),
    /// Register a provider for this plugin instance. Requires RunCommands.
    RegisterAgentProvider(crate::agent::provider::AgentProvider),
    /// Read all registered agent provider definitions. Requires RunCommands.
    GetAgentProviders,
    RegisterProjectProvider(crate::project_provider::ProjectProvider),
    GetProjectProviders,
    ValidateProjectMatch(serde_json::Value),
    PublishProjectLocations(serde_json::Value),
    ListClients,
    ChangeHostFolder(PathBuf),
    SetFloatingPanePinned(PaneId, bool), // bool -> should be pinned
    StackPanes(Vec<PaneId>),
    ChangeFloatingPanesCoordinates(Vec<(PaneId, FloatingPaneCoordinates)>),
    /// Move or resize an overlay on the session surface. The overlay's kind was decided when it
    /// was born and is not touched here.
    ChangeOverlayCoordinates {
        placements: Vec<(PaneId, FloatingPaneCoordinates)>,
        /// The placement is a user gesture made through the plugin, so it may reclaim a position
        /// the user dragged (`OverlayCarry::position_is_user_stated` on the server) rather than
        /// being overruled by it. A size the user resized (`size_is_user_stated`) is kept even
        /// so: the overlay moves where the placement says, at the size the user gave it.
        override_user_placement: bool,
    },
    TogglePaneBorderless(PaneId),
    SetPaneBorderless(PaneId, bool),
    OpenCommandPaneNearPlugin(CommandToRun, Context),
    OpenCommandPaneInGitCheckoutNearPlugin(CommandToRun, PathBuf, GitCheckout),
    OpenTerminalNearPlugin(FileToOpen),
    OpenTerminalFloatingNearPlugin(FileToOpen, Option<FloatingPaneCoordinates>),
    OpenTerminalInPlaceOfPlugin(FileToOpen, bool), // bool -> close_plugin_after_replace
    OpenCommandPaneFloatingNearPlugin(CommandToRun, Option<FloatingPaneCoordinates>, Context),
    OpenCommandPaneInPlaceOfPlugin(CommandToRun, bool, Context), // bool ->
    // close_plugin_after_replace
    OpenFileNearPlugin(FileToOpen, Context),
    OpenFileFloatingNearPlugin(FileToOpen, Option<FloatingPaneCoordinates>, Context),
    OpenFileInPlaceOfPlugin(FileToOpen, bool, Context), // bool -> close_plugin_after_replace
    GroupAndUngroupPanes(Vec<PaneId>, Vec<PaneId>, bool), // panes to group, panes to ungroup,
    // bool -> for all clients
    HighlightAndUnhighlightPanes(Vec<PaneId>, Vec<PaneId>), // panes to highlight, panes to
    // unhighlight
    /// The panes that are kin of the focused one - agents sharing its checkout, say - to be
    /// drawn with a gentle tint until this is sent again.
    ///
    /// Replaces the whole set rather than marking and unmarking, because the sender is
    /// answering "which panes are related *now*" and has no way to know what the session
    /// still believes. A plugin that reloads re-asserts; a dropped message costs one stale
    /// frame rather than a mark nothing will ever take off.
    SetRelatedPanes(Vec<PaneId>),
    /// Bring a pane into view for this client and highlight it, without taking
    /// keyboard focus. The `String` is a label drawn in place of the pane's
    /// title for as long as the highlight is up.
    RevealPaneWithId(PaneId, Option<String>),
    CloseMultiplePanes(Vec<PaneId>),
    FloatMultiplePanes(Vec<PaneId>),
    EmbedMultiplePanes(Vec<PaneId>),
    SetSelfMouseSelectionSupport(bool),
    InterceptKeyPresses,
    ClearKeyPressesIntercepts,
    ReplacePaneWithExistingPane(PaneId, PaneId, bool), // (pane id to replace, pane id of existing,
    // suppress_replaced_pane)
    RunAction(Action, BTreeMap<String, String>),
    CopyToClipboard(String), // text to copy
    OverrideLayout(
        LayoutInfo,
        bool,                     // retain_existing_terminal_panes
        bool,                     // retain_existing_plugin_panes
        bool,                     // apply_only_to_active_tab,
        BTreeMap<String, String>, // context
    ),
    SaveLayout {
        layout_name: String,
        layout_kdl: String,
        overwrite: bool,
    },
    DeleteLayout {
        layout_name: String,
    },
    RenameLayout {
        old_layout_name: String,
        new_layout_name: String,
    },
    EditLayout {
        layout_name: String,
        context: Context,
    },
    GenerateRandomName,
    DumpLayout(String),
    ParseLayout(String), // String contains raw KDL layout
    GetLayoutDir,
    /// The absolute folder holding the user's own plugins, available through `/plugins`.
    GetUserPluginDir,
    GetFocusedPaneInfo,
    SaveSession,
    CurrentSessionLastSavedTime,
    GetPaneInfo(PaneId),
    GetTabInfo(TabId), // tab_id
    GetSessionEnvironmentVariables,
    /// The KDL configuration block declared for one plugin alias, by alias name.
    ///
    /// A plugin's own configuration already reaches it through `load`; this is for
    /// reading a *different* plugin's, which nothing else answers. `SessionInfo.plugins`
    /// comes closest but is keyed on running plugins, so it cannot speak for an alias
    /// whose plugin has never been launched.
    GetPluginAliasConfiguration(String), // alias name
    /// Running plugin IDs matching a URL or alias through canonical source identity.
    GetPluginIdsForUrl(String),
    GetPluginInfo(Option<crate::internal_ids::PluginId>),
    Pick(crate::pick::PickCommand),
    GetPluginIdsForUrlWithConfiguration {
        url: String,
        configuration: BTreeMap<String, String>,
    },
    OpenCommandPaneInNewTab(CommandToRun, Context),
    OpenPluginPaneInNewTab {
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        context: Context,
    },
    OpenEditorPaneInNewTab(FileToOpen, Context),
    OpenCommandPaneInPlaceOfPaneId(PaneId, CommandToRun, bool, Context), // bool = close_replaced_pane
    OpenTerminalPaneInPlaceOfPaneId(PaneId, FileToOpen, bool),
    OpenEditPaneInPlaceOfPaneId(PaneId, FileToOpen, bool, Context),
    HideFloatingPanes {
        tab_id: Option<TabId>,
    },
    ShowFloatingPanes {
        tab_id: Option<TabId>,
    },
    SetPaneColor(PaneId, Option<String>, Option<String>), // (pane_id, fg, bg)
    SetPaneRegexHighlights(PaneId, Vec<RegexHighlight>),
    /// Answer a `Event::ContextMenuRequested`: this is what the menu should say.
    ///
    /// `request_id` is echoed from the request. Answering a menu that has since been dismissed or
    /// replaced is harmless - the server drops it - so a plugin never has to track whether its
    /// answer is still wanted.
    SetContextMenu {
        request_id: ContextMenuRequestId,
        items: Vec<ContextMenuItem>,
    },
    ClearPaneHighlights(PaneId),
    OpenPluginPaneFloating {
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        context: BTreeMap<String, String>,
    },
    GetSessionList,
    GetDisconnectTargets,
    KillSessionsAndReply(Vec<SessionName>), // one or more session names; sends a response back
    DeleteDeadSessionAndReply(SessionName), // session name; sends a response back
    DeleteAllDeadSessionsAndReply,          // no payload; sends a response back
    FocusHostSession,
    ListNotifications,
    ResolveNotification {
        id: NotificationId,
        decision: NotificationDecision,
    },
    /// Move a pane out of this session and into another one. Sends a response
    /// back, and blocks until the pane has landed or the move has been undone.
    WarpPaneToSession {
        pane_id: PaneId,
        to_session: SessionName,
        placement: WarpPlacement,
    },
    /// Ask another session for one of its panes. Sends a response back.
    WarpPaneFromSession {
        pane_id: PaneId,
        from_session: SessionName,
        placement: WarpPlacement,
    },
    /// Move a whole tab out of this session and into another one. Sends a
    /// response back, and blocks until the tab has landed or the move has been
    /// undone.
    WarpTabToSession {
        tab_id: TabId,
        to_session: SessionName,
    },
    /// Ask another session for one of its tabs. Sends a response back.
    WarpTabFromSession {
        tab_id: TabId,
        from_session: SessionName,
    },
    /// Panes whose session died but whose process is still running. Sends a
    /// response back.
    ListOrphanedPanes,
    /// Take one of those panes into this session. Sends a response back.
    AdoptOrphanedPane {
        host_id: SupervisorId,
        placement: WarpPlacement,
    },
    /// Declare an entity in the data store, creating or migrating its storage to match. Sends a
    /// response back.
    DataDefineEntity {
        namespace: Option<String>,
        def: EntityDef,
        allow_destructive: bool,
    },
    /// Apply a batch of writes in one transaction, all or nothing. Sends a response back.
    DataApply {
        namespace: Option<String>,
        mutations: Vec<Mutation>,
    },
    /// Read rows from the data store. Sends a response back.
    DataFetch {
        request: FetchRequest,
    },
    /// Count the rows a fetch would match. Sends a response back.
    DataCount {
        request: FetchRequest,
    },
    /// Register a standing fetch, and be sent an [`Event::PluginDataChanged`] whenever its results
    /// change. Sends the initial rows back.
    DataWatch {
        request: FetchRequest,
        mode: WatchMode,
    },
    /// Stop a watch registered with `DataWatch`. Sends a response back.
    DataUnwatch {
        watch_id: DataWatchId,
    },
    /// What entities exist, and what their columns are. Sends a response back.
    ///
    /// Unlike every other `Data*` command, `None` means *every namespace this plugin may see* —
    /// its own, plus the `shared:` ones if it holds `SharedPluginData` — rather than its own
    /// alone. A catalog of only what the caller declared itself would have nothing to tell it,
    /// and discovering another plugin's published entities is the whole point. `Some(namespace)`
    /// names one `shared:` namespace, on the same terms a fetch does.
    DataCatalog {
        namespace: Option<String>,
    },
    /// Reserve a frame row below the title. `None` or empty text clears it.
    SetPaneSubtitle {
        pane_id: PaneId,
        subtitle: Option<String>,
    },
    /// Compose a decoration in front of a pane's title, or drop it with `None`.
    ///
    /// Not a rename: the pane's name, its own OSC title and the title a layout gave
    /// it are all untouched, and so is the `title` every plugin reads off the pane
    /// manifest. The prefix only shows where a human looks at the pane — its frame
    /// and its row in a stack list — and survives every rename, which is the point:
    /// it carries a fact about the pane that the pane's name cannot be trusted to
    /// keep. Terminal panes only.
    SetPaneTitlePrefix {
        pane_id: PaneId,
        prefix: Option<String>,
    },
    /// Compile `source` as a TypeScript plugin without running it, and answer with the compile error if
    /// there is one. `name` is what the error names, as the loader's own error would
    /// (`keys.ts:3: ...`). Nothing is written or loaded: this is the check a writer runs before
    /// a reload, so a plugin that cannot parse is refused where the writer can see it rather than
    /// reported in a pane nobody is looking at.
    TestPlugin {
        name: String,
        options: crate::authoring::PluginTestOptions,
        request_id: String,
    },
    CancelPluginTest(String),
    CheckPlugin {
        name: String,
        source: String,
        mode: crate::authoring::PluginCheckMode,
    },
    /// Set what agent runs in `pane_id` and what it is doing, or clear it with `None`. The
    /// record replaces the pane's whole one; every reader of the manifest sees it next report.
    SetPaneAgent {
        pane_id: PaneId,
        agent: Option<AgentInfo>,
    },
    /// Set the id of the run record this plugin is keeping for `pane_id`'s agent, or clear it
    /// with `None`.
    ///
    /// Additive where [`PluginCommand::SetPaneAgent`] is wholesale: this touches one field and
    /// leaves the rest of the record as the agent last reported it, which is what a plugin
    /// holding an agent's history needs — it is not the authority on what the agent is doing,
    /// only on which run it is doing it in. A run id for a pane with no record is dropped:
    /// it is bookkeeping about a record, not a record.
    SetPaneAgentRun {
        pane_id: PaneId,
        run_id: Option<String>,
    },
    /// Read validated agent-exit configuration at the Git worktree root.
    ProjectConfigRead {
        directory: PathBuf,
    },
    /// Validate and atomically replace configuration if the previous source still matches.
    ProjectConfigWrite {
        directory: PathBuf,
        source: String,
        expected: Option<String>,
    },
    /// Start a coding agent in a command pane near this plugin, the way the CLI
    /// and the `new-agent` modal do: plan its argv with `agent::launch::plan` (per-kind
    /// flags, permission mode, resume, the inline Claude Code hooks), make the branch or
    /// worktree `spec` asks for, open the pane there and record it as an agent pane
    /// (`AgentSource::Spawned`). Answers with the pane id, or the error text.
    ///
    /// `source_directory` is the repository a branch or worktree is made in, as a plugin path
    /// (`/host`, a relative path, or an absolute one); absent, the plugin's own folder. `cwd`
    /// is where the pane runs when `spec` names no branch, and is ignored when a checkout is
    /// made. With neither, a spec with no branch runs in `source_directory` as it stands.
    /// `name` titles the pane; `context` round-trips on `command_pane_exited` beside the agent
    /// marker the host adds.
    ///
    SpawnAgent {
        spec: AgentSpawnSpec,
        #[serde(default)]
        source_directory: Option<PathBuf>,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context: BTreeMap<String, String>,
        /// Start parked, without opening a split or selecting an agent tab.
        #[serde(default)]
        start_suppressed: bool,
    },
    /// What this plugin wants said, and decided, when the session is about to end.
    ///
    /// The `notice` is one line the quit confirmation shows, or `None` to withdraw whatever this
    /// plugin last said. `guard` is whether closing the session's *last* pane should raise that
    /// confirmation instead of ending the session there and then.
    ///
    /// The two are independent: a plugin can report unfinished work without blocking a quit.
    ///
    /// Held per plugin and dropped when its pane closes, so a plugin that goes away cannot leave
    /// a guard behind that nothing can lift.
    SetQuitNotice {
        notice: Option<String>,
        guard: bool,
    },
    /// Reload every running instance of the plugin at `url` in place, opening nothing. For a
    /// plugin that is not running this does nothing; `StartOrReloadPlugin` is the one that opens
    /// a pane. Fires `BeforeClose` on the old instance first, for state worth keeping.
    ReloadPluginByUrl(String),
    /// The last `lines` lines of yinz's own log about the plugin `name`: what its
    /// `yinz-log` wrote and any loader error, matched by the source identity marker.
    /// `name` is a plugin name as `plugin_write` spells it (`keys`, `agent/weather`), resolved in
    /// the user's plugin folder the way `yinz plugins log` resolves it, or a builtin's tag.
    ///
    /// Answers synchronously with the lines, or the reason there are none. The log file is
    /// outside every root a plugin can read, and the server holds the only copy of its path,
    /// which is why this is a command and not a file a plugin greps for itself.
    ReadPluginLog {
        name: String,
        lines: usize,
    },
    /// Seconds east of UTC where this session is running, at the instant `epoch_ms`.
    ///
    /// Plugins have no ambient timezone database or process environment, so the host
    /// supplies its local UTC offset for formatting absolute timestamps.
    ///
    /// The instant is asked for rather than assumed to be now, because a run that started
    /// before a daylight-saving change is read back long after it: formatting it with today's
    /// offset would put it an hour out for exactly as long as anyone still cares.
    GetLocalUtcOffset {
        epoch_ms: SignedUnixMillis,
    },
}

/// The outcome of declaring an entity in the data store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataDefineResponse {
    Ok(DataSchemaInfo),
    Err(String),
}

/// The outcome of a batch of writes to the data store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataApplyResponse {
    Ok(ApplyReport),
    /// A write guarded with `expect_rev` found the row at another revision, or gone. The whole
    /// batch was rolled back; re-read and decide again.
    ///
    /// Its own variant rather than an `Err` a caller has to read the message of, because the
    /// answer is different in kind: a conflict is a race worth retrying, and everything else is
    /// a request that will not work however often it is sent.
    Conflict(String),
    Err(String),
}

/// Rows read from the data store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataFetchResponse {
    Ok(DataRows),
    Err(String),
}

/// How many rows a fetch matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataCountResponse {
    Ok(u64),
    Err(String),
}

/// The entities a plugin may see, with their definitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataCatalogResponse {
    Ok(Vec<DataEntityInfo>),
    Err(String),
}

/// A registered watch, and the rows at the moment it was registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataWatchResponse {
    Ok(DataWatchHandle),
    Err(String),
}

/// The outcome of cancelling a watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataUnwatchResponse {
    Ok,
    Err(String),
}

// Response type for plugin API methods that open a pane in a new tab
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenPaneInNewTabResponse {
    pub tab_id: Option<TabId>,
    pub pane_id: Option<PaneId>,
}

// Response types for plugin API methods that create tabs
pub type NewTabResponse = Option<TabId>;
pub type NewTabUnfocusedResponse = Option<TabId>;
pub type NewTabsResponse = Vec<TabId>;
pub type FocusOrCreateTabResponse = Option<TabId>;
pub type BreakPanesToNewTabResponse = Option<TabId>;
pub type BreakPanesToTabWithIndexResponse = Option<TabId>;
pub type BreakPanesToTabWithIdResponse = Option<TabId>;

// Response types for plugin API methods that create panes
pub type OpenFileResponse = Option<PaneId>;
pub type OpenFileFloatingResponse = Option<PaneId>;
pub type OpenFileInPlaceResponse = Option<PaneId>;
pub type OpenFileNearPluginResponse = Option<PaneId>;
pub type OpenFileFloatingNearPluginResponse = Option<PaneId>;
pub type OpenFileInPlaceOfPluginResponse = Option<PaneId>;

pub type OpenTerminalResponse = Option<PaneId>;
pub type OpenTerminalFloatingResponse = Option<PaneId>;
pub type OpenTerminalInPlaceResponse = Option<PaneId>;
pub type OpenTerminalNearPluginResponse = Option<PaneId>;
pub type OpenTerminalFloatingNearPluginResponse = Option<PaneId>;
pub type OpenTerminalInPlaceOfPluginResponse = Option<PaneId>;
pub type NewTiledPaneInTabResponse = Option<PaneId>;

pub type OpenCommandPaneResponse = Option<PaneId>;
pub type OpenCommandPaneFloatingResponse = Option<PaneId>;
pub type OpenCommandPaneOverlayResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceResponse = Option<PaneId>;
pub type OpenCommandPaneNearPluginResponse = Option<PaneId>;
pub type OpenCommandPaneInGitCheckoutResponse = Result<PaneId, String>;
pub type SpawnAgentResponse = Result<SpawnedAgent, String>;
pub type OpenCommandPaneFloatingNearPluginResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceOfPluginResponse = Option<PaneId>;
pub type OpenCommandPaneBackgroundResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenTerminalPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenEditPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenPluginPaneFloatingResponse = Option<PaneId>;

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("{0}")]
    UnknownInputMode(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LayoutWithError {
    pub layout_name: String,
    pub error: LayoutParsingError,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum LayoutParsingError {
    KdlError {
        // boxed to keep this variant from making every `Result<_, LayoutParsingError>` wide
        kdl_error: Box<LayoutDiagnostic>,
        file_name: String,
        source_code: String,
    },
    SyntaxError,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LayoutDiagnostic {
    pub error_message: String,
    pub offset: Option<usize>,
    pub len: Option<usize>,
    pub help_message: Option<String>,
}

/// A project identity and its terminal panes, in checkout traversal order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPaneGroup {
    pub identity: String,
    pub name: String,
    pub configuration_root: Option<String>,
    pub pane_ids: Vec<u32>,
}

/// Launch provenance, independent of the pane's current visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstanceInfo {
    pub plugin_id: crate::internal_ids::PluginId,
    pub background: bool,
}
