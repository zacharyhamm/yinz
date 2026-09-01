// false positive: thiserror's derive macro triggers unused_assignments on struct-style enum variant fields
#![allow(unused_assignments)]
//! Error context system based on a thread-local representation of the call stack, itself based on
//! the instructions that are sent between threads.
//!
//! # Help wanted
//!
//! There is an ongoing endeavor to improve the state of error handling in zellij. Currently, many
//! functions rely on [`unwrap`]ing [`Result`]s rather than returning and hence propagating
//! potential errors. If you're interested in helping to add error handling to zellij, don't
//! hesitate to get in touch with us. Additional information can be found in [the docs about error
//! handling](https://github.com/zellij-org/zellij/tree/main/docs/ERROR_HANDLING.md).

use anyhow::Context;
use colored::*;
#[allow(unused_imports)] // used in set_panic_handler; may appear unused under wasm target
use log::error;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Error, Formatter};
use std::path::PathBuf;

/// Re-exports of common error-handling code.
pub mod prelude {
    pub use super::FatalError;
    pub use super::LoggableError;
    #[cfg(not(target_family = "wasm"))]
    pub use super::ToAnyhow;
    pub use super::ZellijError;
    pub use anyhow::anyhow;
    pub use anyhow::bail;
    pub use anyhow::Context;
    pub use anyhow::Error as anyError;
    pub use anyhow::Result;
}

pub trait ErrorInstruction {
    fn error(err: String) -> Self;
}

/// Helper trait to easily log error types.
///
/// The `print_error` function takes a closure which takes a `&str` and fares with it as necessary
/// to log the error to some usable location. For convenience, logging to stdout, stderr and
/// `log::error!` is already implemented.
///
/// Note that the trait functions pass the error through unmodified, so they can be chained with
/// the usual handling of [`std::result::Result`] types.
pub trait LoggableError<T>: Sized {
    /// Gives a formatted error message derived from `self` to the closure `fun` for
    /// printing/logging as appropriate.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use anyhow;
    /// use zellij_utils::errors::LoggableError;
    ///
    /// let my_err: anyhow::Result<&str> = Err(anyhow::anyhow!("Test error"));
    /// my_err
    ///     .print_error(|msg| println!("{msg}"))
    ///     .unwrap();
    /// ```
    #[track_caller]
    fn print_error<F: Fn(&str)>(self, fun: F) -> Self;

    /// Convenienve function, calls `print_error` and logs the result as error.
    ///
    /// This is not a wrapper around `log::error!`, because the `log` crate uses a lot of compile
    /// time macros from `std` to determine caller locations/module names etc. Since these are
    /// resolved at compile time in the location they are written, they would always resolve to the
    /// location in this function where `log::error!` is called, masking the real caller location.
    /// Hence, we build the log message ourselves. This means that we lose the information about
    /// the calling module (Because it can only be resolved at compile time), however the callers
    /// file and line number are preserved.
    #[track_caller]
    fn to_log(self) -> Self {
        let caller = std::panic::Location::caller();
        self.print_error(|msg| {
            // Build the log entry manually
            // NOTE: The log entry has no module path associated with it. This is because `log`
            // gets the module path from the `std::module_path!()` macro, which is replaced at
            // compile time in the location it is written!
            log::logger().log(
                &log::Record::builder()
                    .level(log::Level::Error)
                    .args(format_args!("{}", msg))
                    .file(Some(caller.file()))
                    .line(Some(caller.line()))
                    .module_path(None)
                    .build(),
            );
        })
    }

    /// Convenienve function, calls `print_error` with the closure `|msg| eprintln!("{}", msg)`.
    fn to_stderr(self) -> Self {
        self.print_error(|msg| eprintln!("{}", msg))
    }

    /// Convenienve function, calls `print_error` with the closure `|msg| println!("{}", msg)`.
    fn to_stdout(self) -> Self {
        self.print_error(|msg| println!("{}", msg))
    }
}

impl<T> LoggableError<T> for anyhow::Result<T> {
    fn print_error<F: Fn(&str)>(self, fun: F) -> Self {
        if let Err(ref err) = self {
            fun(&format!("{:?}", err));
        }
        self
    }
}

/// Special trait to mark fatal/non-fatal errors.
///
/// This works in tandem with `LoggableError` above and is meant to make reading code easier with
/// regard to whether an error is fatal or not (i.e. can be ignored, or at least doesn't make the
/// application crash).
///
/// This essentially degrades any `std::result::Result<(), _>` to a simple `()`.
pub trait FatalError<T> {
    /// Mark results as being non-fatal.
    ///
    /// If the result is an `Err` variant, this will [print the error to the log][`to_log`].
    /// Discards the result type afterwards.
    ///
    /// [`to_log`]: LoggableError::to_log
    #[track_caller]
    fn non_fatal(self);

    /// Mark results as being fatal.
    ///
    /// If the result is an `Err` variant, this will unwrap the error and panic the application.
    /// If the result is an `Ok` variant, the inner value is unwrapped and returned instead.
    ///
    /// # Panics
    ///
    /// If the given result is an `Err` variant.
    #[track_caller]
    fn fatal(self) -> T;
}

/// Helper function to silence `#[warn(unused_must_use)]` cargo warnings. Used exclusively in
/// `FatalError::non_fatal`!
fn discard_result<T>(_arg: anyhow::Result<T>) {}

impl<T> FatalError<T> for anyhow::Result<T> {
    fn non_fatal(self) {
        if self.is_err() {
            discard_result(self.context("a non-fatal error occurred").to_log());
        }
    }

    fn fatal(self) -> T {
        if let Ok(val) = self {
            val
        } else {
            self.context("a fatal error occurred")
                .expect("Program terminates")
        }
    }
}

/// Different types of calls that form an [`ErrorContext`] call stack.
///
/// Complex variants store a variant of a related enum, whose variants can be built from
/// the corresponding Zellij MSPC instruction enum variants ([`ScreenInstruction`],
/// [`PtyInstruction`], [`ClientInstruction`], etc).
#[derive(Copy, Clone, PartialEq, Serialize, Deserialize, Debug)]
pub enum ContextType {
    /// A screen-related call.
    Screen(ScreenContext),
    /// A PTY-related call.
    Pty(PtyContext),
    /// A plugin-related call.
    Plugin(PluginContext),
    /// An app-related call.
    Client(ClientContext),
    /// A server-related call.
    IPCServer(ServerContext),
    StdinHandler,
    AsyncTask,
    PtyWrite(PtyWriteContext),
    BackgroundJob(BackgroundJobContext),
    /// An empty, placeholder call. This should be thought of as representing no call at all.
    /// A call stack representation filled with these is the representation of an empty call stack.
    Empty,
}

impl Display for ContextType {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        if let Some((left, right)) = match *self {
            ContextType::Screen(c) => Some(("screen_thread:", format!("{:?}", c))),
            ContextType::Pty(c) => Some(("pty_thread:", format!("{:?}", c))),
            ContextType::Plugin(c) => Some(("plugin_thread:", format!("{:?}", c))),
            ContextType::Client(c) => Some(("main_thread:", format!("{:?}", c))),
            ContextType::IPCServer(c) => Some(("ipc_server:", format!("{:?}", c))),
            ContextType::StdinHandler => Some(("stdin_handler_thread:", "AcceptInput".to_string())),
            ContextType::AsyncTask => Some(("stream_terminal_bytes:", "AsyncTask".to_string())),
            ContextType::PtyWrite(c) => Some(("pty_writer_thread:", format!("{:?}", c))),
            ContextType::BackgroundJob(c) => Some(("background_jobs_thread:", format!("{:?}", c))),
            ContextType::Empty => None,
        } {
            write!(f, "{} {}", left.purple(), right.green())
        } else {
            write!(f, "")
        }
    }
}

/// Stack call representations corresponding to the different types of [`ScreenInstruction`]s.
///
/// Almost every variant is named after the `ScreenInstruction` variant it stands for, and
/// `From<&ScreenInstruction>` relies on that: it resolves the common case through `EnumString`
/// and only spells out the handful of pairs whose names differ.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum ScreenContext {
    HandlePtyBytes,
    PluginBytes,
    Render,
    RenderToClients,
    NewPane,
    OpenInPlaceEditor,
    ToggleFloatingPanes,
    ShowFloatingPanes,
    HideFloatingPanes,
    AreFloatingPanesVisible,
    ContextMenuHandleKeyIfOpen,
    TogglePaneEmbedOrFloating,
    WriteCharacter,
    ResizeIncreaseAll,
    ResizeIncreaseLeft,
    ResizeIncreaseDown,
    ResizeIncreaseUp,
    ResizeIncreaseRight,
    ResizeDecreaseAll,
    ResizeDecreaseLeft,
    ResizeDecreaseDown,
    ResizeDecreaseUp,
    ResizeDecreaseRight,
    SwitchFocus,
    FocusNextPane,
    FocusPreviousPane,
    FocusLastPane,
    MoveFocusLeft,
    MoveFocusLeftOrPreviousTab,
    MoveFocusDown,
    MoveFocusUp,
    MoveFocusRight,
    MoveFocusRightOrNextTab,
    MovePane,
    MovePaneBackwards,
    MovePaneDown,
    MovePaneUp,
    MovePaneRight,
    MovePaneLeft,
    Exit,
    ClearScreen,
    DumpScreen,
    DumpLayout,
    SaveSession,
    EditScrollback,
    GetPaneScrollback,
    ScrollUp,
    ScrollUpAt,
    ScrollDown,
    ScrollDownAt,
    ScrollToBottom,
    ScrollToTop,
    ScrollToPreviousPrompt,
    ScrollToNextPrompt,
    SelectCommandAtScrollPosition,
    CopyLastCommandOutput,
    MoveTextSelection,
    SwapTextSelectionEnds,
    ClearCommandOutputFlash,
    PageScrollUp,
    PageScrollDown,
    HalfPageScrollUp,
    HalfPageScrollDown,
    ClearScroll,
    CloseFocusedPane,
    ToggleActiveSyncTab,
    ToggleActiveTerminalFullscreen,
    ToggleActiveTerminalNoUiFullscreen,
    TogglePaneFrames,
    SetPaneFrameStyle,
    SetSelectable,
    ShowPluginCursor,
    ClosePane,
    HoldPane,
    SetPaneSuspended,
    UpdatePaneName,
    UpdatePaneCwd,
    UpdatePaneHost,
    SetPaneTitlePrefix,
    UndoRenamePane,
    NewTab,
    ApplyLayout,
    SwitchTabNext,
    SwitchTabPrev,
    CloseTab,
    GoToTab,
    GoToTabName,
    UpdateTabName,
    UndoRenameTab,
    MoveTabLeft,
    MoveTabRight,
    GoToTabWithId,
    CloseTabWithId,
    RenameTabWithId,
    SetTabProjects,
    BreakPanesToTabWithId,
    RecomputeTabSize,
    TerminalPixelDimensions,
    TerminalBackgroundColor,
    TerminalForegroundColor,
    TerminalColorRegisters,
    SetKittyGraphicsSupport,
    SetSixelSupport,
    ForwardHostQuery,
    NestedSessionMessageFromPane,
    NestedGuestPingTick,
    NestedSessionMessageFromHost,
    GuestModalChoice,
    ForwardedReplyFromHost,
    ResumePaneAfterForward,
    HostTerminalThemeChanged,
    SetDarkTheme,
    SetLightTheme,
    ToggleTheme,
    ChangeMode,
    ChangeModeForAllClients,
    MouseEvent,
    Copy,
    ToggleTab,
    AddClient,
    RemoveClient,
    UpdateSearch,
    SearchDown,
    SearchUp,
    SearchToggleCaseSensitivity,
    SearchToggleWholeWord,
    SearchToggleWrap,
    AddRedPaneFrameColorOverride,
    ClearPaneFrameColorOverride,
    SetTabBellFlash,
    HostTerminalFocusChanged,
    SetClientHostTerminalEnv,
    ForwardDesktopNotifications,
    PreviousSwapLayout,
    NextSwapLayout,
    OverrideLayout,
    OverrideLayoutComplete,
    QueryTabNames,
    NewTiledPluginPane,
    StartOrReloadPluginPane,
    NewFloatingPluginPane,
    AddPlugin,
    UpdatePluginLoadingStage,
    StartPluginLoadingAnimation,
    StopPluginLoadingAnimation,
    StartPluginLoadingIndication,
    RequestStateUpdateForPlugins,
    LaunchOrFocusPlugin,
    LaunchPlugin,
    SuppressPane,
    UnsuppressPane,
    UnsuppressOrExpandPane,
    FocusPaneWithId,
    RenamePane,
    RenameActivePane,
    RenameTab,
    RequestPluginPermissions,
    BreakPane,
    BreakPaneRight,
    BreakPaneLeft,
    WarpPane,
    PaneReleasedForWarp,
    AdoptedPaneReady,
    WarpTab,
    TabReleasedForWarp,
    AdoptedTabReady,
    UpdateSessionInfos,
    UpdateAvailableLayouts,
    ReplacePane,
    NewInPlacePluginPane,
    SerializeLayoutForResurrection,
    RenameSession,
    Notify,
    DismissNotification,
    AnimationTick,
    ContextMenuTimedOut,
    SetContextMenu,
    ActivateContextMenuItem,
    CloseContextMenu,
    DumpLayoutToPlugin,
    GetFocusedPaneInfo,
    GetPaneInfo,
    GetTabInfo,
    ListClientsMetadata,
    ListPanes,
    ListNotifications,
    ListTabs,
    GetCurrentTabInfo,
    Reconfigure,
    RerunCommandPane,
    RunCommandInPane,
    ResizePaneWithId,
    EditScrollbackForPaneWithId,
    WriteToPaneId,
    Paste,
    SetPaneColor,
    SetPaneAgent,
    SetPaneAgentRun,
    SetQuitNotice,
    ConfirmQuit,
    WriteKeyToPaneId,
    CopyTextToClipboard,
    MovePaneWithPaneId,
    MovePaneWithPaneIdInDirection,
    ClearScreenForPaneId,
    ScrollUpInPaneId,
    ScrollDownInPaneId,
    ScrollToTopInPaneId,
    ScrollToBottomInPaneId,
    PageScrollUpInPaneId,
    PageScrollDownInPaneId,
    TogglePaneIdFullscreen,
    TogglePaneEmbedOrEjectForPaneId,
    CloseTabWithIndex,
    BreakPanesToNewTab,
    BreakPanesToTabWithIndex,
    ListClientsToPlugin,
    TogglePanePinned,
    SetFloatingPanePinned,
    StackPanes,
    ChangeFloatingPanesCoordinates,
    ChangeOverlayCoordinates,
    TogglePaneBorderless,
    SetPaneBorderless,
    AddHighlightPaneFrameColorOverride,
    FlashPaneFrameColor,
    GroupAndUngroupPanes,
    HighlightAndUnhighlightPanes,
    RevealPaneWithId,
    FloatMultiplePanes,
    EmbedMultiplePanes,
    TogglePaneInGroup,
    ToggleGroupMarking,
    SetMouseSelectionSupport,
    InterceptKeyPresses,
    ClearKeyPressesIntercepts,
    ReplacePaneWithExistingPane,
    AddWatcherClient,
    RemoveWatcherClient,
    SetFollowedClient,
    WatcherTerminalResize,
    ClearMouseHelpText,
    SetPluginRegexHighlights,
    ClearPluginHighlights,
    DesktopNotificationResponse,
    SubscribeToPaneRenders,
    NotifyPaneClosedToSubscribers,
    SetCliEventSubscribersPresent,
    // Pane-targeting CLI variants
    ScrollUpWithPaneId,
    ScrollDownWithPaneId,
    ScrollToTopWithPaneId,
    ScrollToBottomWithPaneId,
    PageScrollUpWithPaneId,
    PageScrollDownWithPaneId,
    HalfPageScrollUpWithPaneId,
    HalfPageScrollDownWithPaneId,
    ResizeWithPaneId,
    MovePaneWithPaneIdCli,
    MovePaneBackwardsWithPaneId,
    ClearScreenWithPaneId,
    EditScrollbackWithPaneId,
    ToggleFullscreenWithPaneId,
    ToggleNoUiFullscreenWithPaneId,
    TogglePaneEmbedOrFloatingWithPaneId,
    CloseFocusWithPaneId,
    RenamePaneWithPaneId,
    UndoRenamePaneWithPaneId,
    TogglePanePinnedWithPaneId,
    // Tab-targeting CLI variants
    UndoRenameTabWithTabId,
    ToggleActiveSyncTabWithTabId,
    ToggleFloatingPanesWithTabId,
    PreviousSwapLayoutWithTabId,
    NextSwapLayoutWithTabId,
    MoveTabWithTabId,
    UpdateBackgroundPluginSubscriptions,
    ClearHintTextCache,
    BroadcastModeUpdate,
    FocusHostSession,
    FocusGuestSession,
    ToggleHostFullscreen,
}

/// Stack call representations corresponding to the different types of [`PtyInstruction`]s.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum PtyContext {
    SpawnTerminal,
    FirecrackerSpawnResolved,
    FirecrackerInPlaceSpawnResolved,
    FirecrackerLayoutResolved,
    OpenInPlaceEditor,
    UpdateActivePane,
    GoToTab,
    NewTab,
    OverrideLayout,
    ClosePane,
    CloseTab,
    ReleaseTerminalForWarp,
    AdoptTerminal,
    ReleaseTerminalsForWarp,
    StrandPaneAfterWarp,
    AdoptTab,
    ClaimAdoptedTab,
    ReRunCommandInPane,
    FirecrackerReRunResolved,
    DropToShellInPane,
    SpawnInPlaceTerminal,
    DumpLayout,
    LogLayoutToHd,
    SaveSessionToDisk,
    FillPluginCwd,
    DumpLayoutToPlugin,
    ListClientsMetadata,
    Reconfigure,
    ListClientsToPlugin,
    ReportPluginCwd,
    SendSigintToPaneId,
    SendSigkillToPaneId,
    ResumeSuspendedPane,
    GetPanePid,
    GetPaneRunningCommand,
    GetPaneCwd,
    GetPaneGitProbe,
    UpdateAndReportCwds,
    NotifyCwdFromOsc7,
    Exit,
}

/// Stack call representations corresponding to the different types of [`PluginInstruction`]s.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum PluginContext {
    Load,
    LoadBackgroundPlugin,
    RunHook,
    RequestContextMenu,
    Update,
    Unload,
    Reload,
    ReloadPluginWithId,
    Resize,
    Exit,
    AddClient,
    RemoveClient,
    NewTab,
    OverrideLayout,
    ApplyCachedEvents,
    ApplyCachedWorkerMessages,
    PostMessageToPluginWorker,
    PostMessageToPlugin,
    PluginSubscribedToEvents,
    SubscribeCliClientToEvents,
    ReloadPluginByUrl,
    PermissionRequestResult,
    DumpLayout,
    LogLayoutToHd,
    CliPipe,
    CachePluginEvents,
    MessageFromPlugin,
    UnblockCliPipes,
    WatchFilesystem,
    KeybindPipe,
    DumpLayoutToPlugin,
    ListClientsMetadata,
    Reconfigure,
    FailedToWriteConfigToDisk,
    ListClientsToPlugin,
    ChangePluginHostDir,
    PaneRenderReport,
    UserInput,
    LayoutListUpdate,
    RequestStateUpdateForPlugin,
    UpdateSessionSaveTime,
    GetLastSessionSaveTime,
    DetectPluginConfigChanges,
    HighlightClicked,
    QueuePump,
    QueueConfigure,
    QueueCancelJob,
    QueueFinishJob,
    GrantPermissionsToLocation,
    PluginFailed,
}

/// Stack call representations corresponding to the different types of [`ClientInstruction`]s.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ClientContext {
    Exit,
    Error,
    UnblockInputThread,
    Render,
    ServerError,
    SwitchToMode,
    Connected,
    Log,
    LogError,
    OwnClientId,
    SwitchSession,
    SetSynchronisedOutput,
    UnblockCliPipeInput,
    CliPipeOutput,
    QueryTerminalSize,
    WriteConfigToDisk,
    RenamedSession,
    ConfigFileUpdated,
    ForwardQueryToHost,
    EmitNestedSessionFrame,
}

/// Stack call representations corresponding to the different types of [`ServerInstruction`]s.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum ServerContext {
    NewClient,
    Render,
    UnblockInputThread,
    ClientExit,
    RemoveClient,
    Error,
    KillSession,
    DetachSession,
    AttachClient,
    ConnStatus,
    Log,
    LogError,
    SwitchSession,
    UnblockCliPipeInput,
    CliPipeOutput,
    EventUpdateToClient,
    AssociatePipeWithClient,
    DisconnectAllClientsExcept,
    ChangeMode,
    ChangeModeForAllClients,
    Reconfigure,
    ReconfigureAllClients,
    ConfigWrittenToDisk,
    FailedToWriteConfigToDisk,
    RebindKeys,
    ClearMouseHelpText,
    ClearCommandOutputFlash,
    ForwardQueryToHost,
    KeyPassthroughChanged,
    EmitNestedSessionFrameToClient,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum PtyWriteContext {
    Write,
    ResizePty,
    StartCachingResizes,
    ApplyCachedResizes,
    Exit,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::VariantNames,
)]
pub enum BackgroundJobContext {
    DisplayPaneError,
    AnimatePluginLoading,
    StopPluginLoadingAnimation,
    ReportSessionInfo,
    ReportLayoutInfo,
    RunCommand,
    WebRequest,
    ReportPluginList,
    RenderToClients,
    HighlightPanesWithMessage,
    DismissNotification,
    StartAnimationClock,
    StopAnimationClock,
    ContextMenuTimeout,
    ClearHelpText,
    ClearCommandOutputFlash,
    FlashPaneBell,
    StopFlashPaneBell,
    FlashTabBell,
    StopFlashTabBell,
    StartNestedGuestPing,
    StopNestedGuestPing,
    Exit,
}

use thiserror::Error;
#[derive(Debug, Error)]
pub enum ZellijError {
    #[error("could not find command '{command}' for terminal {terminal_id}")]
    CommandNotFound { terminal_id: u32, command: String },

    /// A pane that could not be started, for a reason the user should read
    /// rather than a reason that should end the session.
    ///
    /// Carries the reserved terminal id so the failure can be shown *inside* the
    /// pane it was meant to be, the way a missing command already is. Opening a
    /// pane on another machine fails in a dozen ways no local pane can — an ssh
    /// that will not connect, a broker that is not there — and none of them are
    /// worth taking every other pane in the session down with.
    #[error("could not start a pane in terminal {terminal_id}: {message}")]
    PaneStartFailed { terminal_id: u32, message: String },

    #[error("could not determine default editor")]
    NoEditorFound,

    #[error("failed to allocate another terminal id")]
    NoMoreTerminalIds,

    #[error("failed to start PTY")]
    FailedToStartPty,

    #[error(
        "Zellij loads the core plugins from disk, but this plugin wasn't found:

    Plugin name: '{plugin_path}'
    Plugin directory: '{plugin_dir}'

If you built zellij from source:
    `cargo x install <destination>` installs the builtin plugins next to the data
    dir, under 'builtins/<version>'. In a checkout, `cargo x build -p` builds them
    where a debug binary looks. Do not set CARGO_TARGET_DIR: the checkout paths
    are baked in at compile time.

If you're a user of a packaged zellij:
    Please report this error to the distributor of your current zellij version.

Otherwise, place the builtin plugin '.wasm' files in the plugin directory shown
above, in the 'plugins' folder of the system data directory, or point the
'builtin_plugins_dir' config option at the directory holding them. Every
location zellij looks in is listed by `zellij setup --check`.
"
    )]
    BuiltinPluginMissing {
        plugin_path: PathBuf,
        plugin_dir: PathBuf,
        #[source]
        source: anyhow::Error,
    },

    #[error(
        "It seems you tried to load the following builtin plugin:

    Plugin name: '{plugin_path}'

This is not a builtin plugin known to this version of zellij. If you were using
a custom layout, please refer to the layout documentation at:

    https://zellij.dev/documentation/creating-a-layout.html#plugin

If you think this is a bug and the plugin is indeed an internal plugin, please
open an issue on GitHub:

    https://github.com/zellij-org/zellij/issues
"
    )]
    BuiltinPluginNonexistent {
        plugin_path: PathBuf,
        #[source]
        source: anyhow::Error,
    },

    // this is a temporary hack until we're able to merge custom errors from within the various
    // crates themselves without having to move their payload types here
    #[error("Cannot resize fixed panes")]
    CantResizeFixedPanes { pane_ids: Vec<(u32, bool)> }, // bool: 0 => terminal_pane, 1 =>
    // plugin_pane
    #[error("Pane size remains unchanged")]
    PaneSizeUnchanged,

    #[error("an error occurred")]
    GenericError { source: anyhow::Error },

    #[error("Client {client_id} is too slow to handle incoming messages")]
    ClientTooSlow { client_id: u16 },

    #[error("The plugin does not exist")]
    PluginDoesNotExist,

    #[error("Ran out of room for spans")]
    RanOutOfRoomForSpans,
}

#[cfg(not(target_family = "wasm"))]
pub use not_wasm::*;

#[cfg(not(target_family = "wasm"))]
mod not_wasm {
    use super::*;
    use crate::channels::{SenderWithContext, ASYNCOPENCALLS, OPENCALLS};
    use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, Report};
    use std::panic::PanicHookInfo;
    use thiserror::Error as ThisError;

    /// The maximum amount of calls an [`ErrorContext`] will keep track
    /// of in its stack representation. This is a per-thread maximum.
    const MAX_THREAD_CALL_STACK: usize = 6;

    #[derive(Debug, ThisError, Diagnostic)]
    #[error("{0}{backtrace}", backtrace = self.show_backtrace())]
    #[diagnostic(help("{}", self.show_help()))]
    struct Panic(String);

    impl Panic {
        // We already capture a backtrace with `anyhow` using the `backtrace` crate in the background.
        // The advantage is that this is the backtrace of the real errors source (i.e. where we first
        // encountered the error and turned it into an `anyhow::Error`), whereas the backtrace recorded
        // here is the backtrace leading to the call to any `panic`ing function. Since now we propagate
        // errors up before `unwrap`ing them (e.g. in `zellij_server::screen::screen_thread_main`), the
        // former is what we really want to diagnose.
        // We still keep the second one around just in case the first backtrace isn't meaningful or
        // non-existent in the first place (Which really shouldn't happen, but you never know).
        fn show_backtrace(&self) -> String {
            if let Ok(var) = std::env::var("RUST_BACKTRACE") {
                if !var.is_empty() && var != "0" {
                    return format!("\n\nPanic backtrace:\n{:?}", backtrace::Backtrace::new());
                }
            }
            "".into()
        }

        fn show_help(&self) -> String {
            format!(
                "If you are seeing this message, it means that something went wrong.

-> To get additional information, check the log at: {}
-> To see a backtrace next time, reproduce the error with: RUST_BACKTRACE=1 zellij [...]
-> To help us fix this, please open an issue: https://github.com/zellij-org/zellij/issues

",
                crate::consts::ZELLIJ_TMP_LOG_FILE.display()
            )
        }
    }

    /// Custom panic handler/hook. Prints the [`ErrorContext`].
    pub fn handle_panic<T>(info: &PanicHookInfo<'_>, sender: Option<&SenderWithContext<T>>)
    where
        T: ErrorInstruction + Clone,
    {
        use std::{process, thread};
        let thread = thread::current();
        let thread = thread.name().unwrap_or("unnamed");

        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => Some(*s),
            None => info.payload().downcast_ref::<String>().map(|s| &**s),
        }
        .unwrap_or("An unexpected error occurred!");

        let err_ctx = OPENCALLS.with(|ctx| *ctx.borrow());

        let mut report: Report = Panic(format!("\u{1b}[0;31m{}\u{1b}[0;0m", msg)).into();

        let mut location_string = String::new();
        if let Some(location) = info.location() {
            location_string = format!(
                "At {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
            report = report.wrap_err(location_string.clone());
        }

        if !err_ctx.is_empty() {
            report = report.wrap_err(format!("{}", err_ctx));
        }

        report = report.wrap_err(format!(
            "Thread '\u{1b}[0;31m{}\u{1b}[0;0m' panicked.",
            thread
        ));

        error!(
            "Panic occurred:
             thread: {}
             location: {}
             message: {}",
            thread, location_string, msg
        );

        if thread == "main" || sender.is_none() {
            // here we only show the first line because the backtrace is not readable otherwise
            // a better solution would be to escape raw mode before we do this, but it's not trivial
            // to get os_input here
            println!("\u{1b}[2J{}", fmt_report(report));
            process::exit(1);
        } else if let Some(sender) = sender {
            let _ = sender.send(T::error(fmt_report(report)));
        }
    }

    pub fn get_current_ctx() -> ErrorContext {
        ASYNCOPENCALLS
            .try_with(|ctx| *ctx.borrow())
            .unwrap_or_else(|_| OPENCALLS.with(|ctx| *ctx.borrow()))
    }

    fn fmt_report(diag: Report) -> String {
        let mut out = String::new();
        GraphicalReportHandler::new_themed(GraphicalTheme::unicode())
            .render_report(&mut out, diag.as_ref())
            .unwrap();
        out
    }

    /// A representation of the call stack.
    #[derive(Clone, Copy, Serialize, Deserialize, Debug)]
    pub struct ErrorContext {
        calls: [ContextType; MAX_THREAD_CALL_STACK],
    }

    impl ErrorContext {
        /// Returns a new, blank [`ErrorContext`] containing only [`Empty`](ContextType::Empty)
        /// calls.
        pub fn new() -> Self {
            Self {
                calls: [ContextType::Empty; MAX_THREAD_CALL_STACK],
            }
        }

        /// Returns `true` if the calls has all [`Empty`](ContextType::Empty) calls.
        pub fn is_empty(&self) -> bool {
            self.calls.iter().all(|c| c == &ContextType::Empty)
        }

        /// Adds a call to this [`ErrorContext`]'s call stack representation.
        pub fn add_call(&mut self, call: ContextType) {
            for ctx in &mut self.calls {
                if let ContextType::Empty = ctx {
                    *ctx = call;
                    break;
                }
            }
            self.update_thread_ctx()
        }

        /// Updates the thread local [`ErrorContext`].
        pub fn update_thread_ctx(&self) {
            ASYNCOPENCALLS
                .try_with(|ctx| *ctx.borrow_mut() = *self)
                .unwrap_or_else(|_| OPENCALLS.with(|ctx| *ctx.borrow_mut() = *self));
        }
    }

    impl Default for ErrorContext {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Display for ErrorContext {
        fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
            writeln!(f, "Originating Thread(s)")?;
            for (index, ctx) in self.calls.iter().enumerate() {
                if *ctx == ContextType::Empty {
                    break;
                }
                writeln!(f, "\t\u{1b}[0;0m{}. {}", index + 1, ctx)?;
            }
            Ok(())
        }
    }

    /// Helper trait to convert error types that don't satisfy `anyhow`s trait requirements to
    /// anyhow errors.
    pub trait ToAnyhow<U> {
        fn to_anyhow(self) -> anyhow::Result<U>;
    }

    /// `SendError` doesn't satisfy `anyhow`s trait requirements due to `T` possibly being a
    /// `PluginInstruction` type, which wraps an `mpsc::Send` and isn't `Sync`. Due to this, in turn,
    /// the whole error type isn't `Sync` and doesn't work with `anyhow` (or pretty much any other
    /// error handling crate).
    ///
    /// Takes the `SendError` and creates an `anyhow` error type with the message that was sent
    /// (formatted as string), attaching the [`ErrorContext`] as anyhow context to it.
    impl<T: std::fmt::Debug, U> ToAnyhow<U>
        for Result<U, crate::channels::SendError<(T, ErrorContext)>>
    {
        fn to_anyhow(self) -> anyhow::Result<U> {
            match self {
                Ok(val) => anyhow::Ok(val),
                Err(e) => {
                    let (msg, context) = e.into_inner();
                    if *crate::consts::DEBUG_MODE.get().unwrap_or(&true) {
                        Err(anyhow::anyhow!(
                            "failed to send message to channel: {:#?}",
                            msg
                        ))
                        .with_context(|| context.to_string())
                    } else {
                        Err(anyhow::anyhow!("failed to send message to channel"))
                            .with_context(|| context.to_string())
                    }
                },
            }
        }
    }

    impl<U> ToAnyhow<U> for Result<U, std::sync::PoisonError<U>> {
        fn to_anyhow(self) -> anyhow::Result<U> {
            match self {
                Ok(val) => anyhow::Ok(val),
                Err(e) => {
                    if *crate::consts::DEBUG_MODE.get().unwrap_or(&true) {
                        Err(anyhow::anyhow!("cannot acquire poisoned lock for {e:#?}"))
                    } else {
                        Err(anyhow::anyhow!("cannot acquire poisoned lock"))
                    }
                },
            }
        }
    }
}
