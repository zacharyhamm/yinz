//! Plugin permission policy and permission-request lifecycle.

use crate::plugin_map::PluginEnv;
use crate::PluginInstruction;
use std::collections::HashSet;
use std::sync::atomic;
use yinz_config::permission::PermissionCache;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{CommandType, PermissionStatus, PermissionType, PluginCommand};

/// What a layout file may do once loaded. A file is trusted without inspecting it - it can change
/// after any check, and its templates and swap layouts apply later - so applying or writing one
/// needs every permission a layout's panes can use.
pub(super) const LAYOUT_FILE_PERMISSIONS: [PermissionType; 3] = [
    PermissionType::OpenFiles,
    PermissionType::RunCommands,
    PermissionType::OpenTerminalsOrPlugins,
];

/// Grant another plugin permissions ahead of its prompt. Refused for anything but a builtin:
/// `check_command_permission` already waves a builtin through, so the gate has to be here, and
/// a non-builtin reaching it would be a plugin handing out authority it was never given.
pub(super) fn grant_plugin_permissions(
    env: &PluginEnv,
    plugin_url: String,
    permissions: Vec<PermissionType>,
) -> Result<()> {
    if !env.plugin_config.is_builtin() {
        return Err(anyhow!(
            "only a builtin plugin may grant permissions to another plugin; {} is not one",
            env.name()
        ));
    }
    env.senders
        .send_to_plugin(PluginInstruction::GrantPermissionsToLocation {
            plugin_url,
            permissions,
        })
        .with_context(|| format!("failed to grant permissions from {}", env.name()))
}

pub(super) fn request_permission(env: &PluginEnv, permissions: Vec<PermissionType>) -> Result<()> {
    // Before either branch: from here on this plugin is waiting for an answer, and a denial it
    // meets in the meantime is a timing problem rather than a missing request. See
    // [`permission_answer_pending`].
    env.permissions_requested
        .store(true, atomic::Ordering::SeqCst);
    // Cached grants take effect synchronously; every answer still carries its runtime identity.
    use yinz_types::input::layout::RunPluginLocation;
    let location = match (&env.plugin_config.location, &env.resolved_source_path) {
        (RunPluginLocation::File(_), Some(path)) => RunPluginLocation::File(
            yinz_runtime::common_path::source_path(path, &env.plugin_cwd),
        ),
        (location, _) => location.clone(),
    };
    if PermissionCache::from_path_or_default(None)
        .check_permissions(location.to_string(), &permissions)
    {
        env.permissions
            .lock()
            .unwrap()
            .get_or_insert_with(HashSet::new)
            .extend(permissions.iter().copied());
    }
    env.senders
        .send_to_plugin(PluginInstruction::RequestPermissions {
            plugin_id: env.plugin_id,
            instance_id: env.instance_id,
            client_id: env.client_id,
            permissions,
        })
}

pub(super) fn ensure_plugin_permissions(
    env: &PluginEnv,
    permissions: impl IntoIterator<Item = PermissionType>,
) -> Result<()> {
    let missing: Vec<_> = permissions
        .into_iter()
        .filter(|permission| !has_permission(env, *permission))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "layout needs permissions this plugin has not been granted: {}",
            missing
                .into_iter()
                .map(|permission| permission.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Whether the plugin holds `permission`. Built-in plugins hold everything, as they do in
/// [`check_command_permission`].
pub(super) fn has_permission(env: &PluginEnv, permission: PermissionType) -> bool {
    if env.plugin_config.is_builtin() {
        return true;
    }
    env.permissions
        .lock()
        .unwrap()
        .as_ref()
        .map(|granted| granted.contains(&permission))
        .unwrap_or(false)
}

/// Whether this plugin has asked for permissions and is still waiting to hear back.
///
/// The window is real and every plugin passes through it: the grant, cached or freshly prompted
/// for, arrives as a `PermissionRequestResult` event, and `load` runs before any event does, so
/// a plugin that asks in `load` is refused everything for the rest of it however long ago the
/// permission was granted. A denial made in that window has a different fix from one made after
/// (wait for the answer, rather than ask), and a different fix again from the one a plugin that
/// never asked at all needs, which is why the request is tracked separately from its answer.
pub(crate) fn permission_answer_pending(plugin_env: &PluginEnv) -> bool {
    !plugin_env.plugin_config.is_builtin()
        && plugin_env
            .permissions_requested
            .load(atomic::Ordering::SeqCst)
        && plugin_env.permissions.lock().unwrap().is_none()
}

/// Whether `plugin_env` may run `command`, and which permission it is short of if not.
///
/// The runtime checks before dispatching so denied operations raise guest errors.
pub(crate) fn check_command_permission(
    plugin_env: &PluginEnv,
    command: &PluginCommand,
) -> (PermissionStatus, Option<PermissionType>) {
    if matches!(command, PluginCommand::Pick(command) if !matches!(command, yinz_types::pick::PickCommand::Start { .. }))
    {
        return (PermissionStatus::Granted, None);
    }
    if matches!(command, PluginCommand::GetPluginInfo(id) if id.is_none_or(|id| id == plugin_env.plugin_id))
    {
        return (PermissionStatus::Granted, None);
    }
    if plugin_env.plugin_config.is_builtin() {
        // built-in plugins can do all the things because they're part of the application and
        // there's no use to deny them anything
        return (PermissionStatus::Granted, None);
    }
    if matches!(
        command,
        PluginCommand::PaneTabs(yinz_types::pane_tabs::PaneTabsCommand::Mount { .. })
    ) && !plugin_env
        .permissions
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|permissions| permissions.contains(&PermissionType::OpenTerminalsOrPlugins))
    {
        return (
            PermissionStatus::Denied,
            Some(PermissionType::OpenTerminalsOrPlugins),
        );
    }
    // A saved or renamed layout file runs as a `LayoutInfo::File` when it is next loaded - as
    // `default.kdl`, by the next plain `yinz` start, with no plugin involved - so writing one
    // needs what applying one does, on top of `ChangeApplicationState` below.
    if matches!(
        command,
        PluginCommand::SaveLayout { .. } | PluginCommand::RenameLayout { .. }
    ) && let Some(permission) = LAYOUT_FILE_PERMISSIONS
        .into_iter()
        .find(|permission| !has_permission(plugin_env, *permission))
    {
        return (PermissionStatus::Denied, Some(permission));
    }
    let children: &[yinz_types::multipane::MultipaneChild] = match command {
        PluginCommand::CreateMultipane(request) => &request.children,
        PluginCommand::ReplaceMultipaneChild(request) => std::slice::from_ref(&request.child),
        _ => &[],
    };
    for child in children {
        let permission = match child {
            yinz_types::multipane::MultipaneChild::Existing(_) => continue,
            yinz_types::multipane::MultipaneChild::Terminal(_) => PermissionType::RunCommands,
            yinz_types::multipane::MultipaneChild::Plugin { .. } => {
                PermissionType::OpenTerminalsOrPlugins
            },
        };
        if !plugin_env
            .permissions
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|permissions| permissions.contains(&permission))
        {
            return (PermissionStatus::Denied, Some(permission));
        }
    }
    // A syntax check of an inline source reads no file - the handler follows imports only in
    // `Semantic` mode - so it needs no permission.
    if let PluginCommand::CheckPlugin {
        mode: yinz_types::authoring::PluginCheckMode::Syntax,
        ..
    } = command
    {
        return (PermissionStatus::Granted, None);
    }
    // `None` is an explicit verdict from an exhaustive match — the command needs no permission —
    // never "not yet classified": a new variant fails to compile in `command_permission`.
    let Some(permission) = command_permission(CommandType::from(command)) else {
        return (PermissionStatus::Granted, None);
    };

    if let Some(permissions) = plugin_env.permissions.lock().unwrap().as_ref()
        && permissions.contains(&permission)
    {
        return (PermissionStatus::Granted, None);
    }

    (PermissionStatus::Denied, Some(permission))
}

/// The permission a command needs, or `None` when it needs none.
///
/// The match is exhaustive on purpose: `check_command_permission` grants a `None`, so a catch-all
/// arm would make every variant added later fail open. Each `None` says why below.
pub(super) fn command_permission(kind: CommandType) -> Option<PermissionType> {
    Some(match kind {
        // Bootstrap and the plugin's own pane: nothing here reaches outside the plugin.
        CommandType::ResolveNotification
        | CommandType::RequestPluginPermissions
        | CommandType::Subscribe
        | CommandType::Unsubscribe
        | CommandType::SetSelectable
        | CommandType::SetSelfMouseSelectionSupport
        | CommandType::ShowCursor
        | CommandType::HideSelf
        | CommandType::ShowSelf
        | CommandType::CloseSelf
        | CommandType::SetTimeout
        | CommandType::ReportPanic
        | CommandType::GetCellPixelSize
        | CommandType::GetPluginIds
        | CommandType::GetYinzVersion => return None,
        // A message to the plugin's own worker, and the worker's reply to the plugin: both stay
        // inside the plugin (`post_message_to_plugin` always addresses `env.plugin_id`).
        CommandType::PostMessageTo | CommandType::PostMessageToPlugin => return None,
        // Listing and watching the assigned host root are part of the baseline capability.
        // Changing that root requires `ChangeHostFolder`, below.
        CommandType::ScanHostFolder | CommandType::WatchFilesystem => return None,
        CommandType::OpenFile
        | CommandType::OpenFileFloating
        | CommandType::OpenFileNearPlugin
        | CommandType::OpenFileFloatingNearPlugin
        | CommandType::OpenFileInPlaceOfPlugin
        | CommandType::OpenFileInPlace
        | CommandType::OpenEditPaneInPlaceOfPaneId => PermissionType::OpenFiles,
        CommandType::OpenTerminal
        | CommandType::OpenTerminalNearPlugin
        | CommandType::StartOrReloadPlugin
        | CommandType::OpenTerminalFloating
        | CommandType::OpenTerminalFloatingNearPlugin
        | CommandType::OpenTerminalInPlace
        | CommandType::OpenTerminalInPlaceOfPlugin
        | CommandType::OpenPluginPaneInNewTab
        | CommandType::OpenPluginPaneFloating
        | CommandType::LoadNewPlugin
        | CommandType::NewTiledPaneInTab
        | CommandType::OpenTerminalPaneInPlaceOfPaneId
        | CommandType::OpenPluginPaneInPlaceOfPaneId => PermissionType::OpenTerminalsOrPlugins,
        CommandType::OpenCommandPane
        | CommandType::OpenCommandPaneNearPlugin
        | CommandType::OpenCommandPaneInGitCheckoutNearPlugin
        | CommandType::OpenCommandPaneFloating
        | CommandType::OpenCommandPaneOverlay
        | CommandType::OpenCommandPaneFloatingNearPlugin
        | CommandType::OpenCommandPaneInPlace
        | CommandType::OpenCommandPaneInPlaceOfPlugin
        | CommandType::OpenCommandPaneBackground
        | CommandType::OpenCommandPaneInPlaceOfPaneId
        | CommandType::RunCommand
        | CommandType::AgentControl
        | CommandType::RegisterProjectProvider
        | CommandType::GetProjectProviders
        | CommandType::ValidateProjectMatch
        | CommandType::PublishProjectLocations
        | CommandType::RegisterAgentProvider
        | CommandType::GetAgentProviders
        | CommandType::RestartAgentPane
        | CommandType::RunCommandInPane
        | CommandType::SpawnAgent
        | CommandType::ExecCmd => PermissionType::RunCommands,
        CommandType::WebRequest | CommandType::CancelWebRequest => PermissionType::WebAccess,
        CommandType::Write
        | CommandType::WriteChars
        | CommandType::WriteToPaneId
        | CommandType::WriteCharsToPaneId => PermissionType::WriteToStdin,
        CommandType::CopyToClipboard => PermissionType::WriteToClipboard,
        CommandType::SwitchTabTo
        | CommandType::SwitchToMode
        | CommandType::NewTabsWithLayout
        | CommandType::NewTabsWithLayoutInfo
        | CommandType::NewTab
        | CommandType::NewTabUnfocused
        | CommandType::GoToNextTab
        | CommandType::GoToPreviousTab
        | CommandType::Resize
        | CommandType::ResizeWithDirection
        | CommandType::FocusNextPane
        | CommandType::FocusPreviousPane
        | CommandType::FocusLastPane
        | CommandType::MoveFocus
        | CommandType::MoveFocusOrTab
        | CommandType::Detach
        | CommandType::EditScrollback
        | CommandType::EditScrollbackForPaneWithId
        | CommandType::ToggleTab
        | CommandType::MovePane
        | CommandType::MovePaneWithDirection
        | CommandType::MovePaneWithPaneId
        | CommandType::MovePaneWithPaneIdInDirection
        | CommandType::ClearScreen
        | CommandType::ClearScreenForPaneId
        | CommandType::ScrollUp
        | CommandType::ScrollUpInPaneId
        | CommandType::ScrollDown
        | CommandType::ScrollDownInPaneId
        | CommandType::ScrollToTop
        | CommandType::ScrollToTopInPaneId
        | CommandType::ScrollToBottom
        | CommandType::ScrollToBottomInPaneId
        | CommandType::PageScrollUp
        | CommandType::PageScrollUpInPaneId
        | CommandType::PageScrollDown
        | CommandType::PageScrollDownInPaneId
        | CommandType::ToggleFocusFullscreen
        | CommandType::ToggleFocusNoUiFullscreen
        | CommandType::FocusHostSession
        | CommandType::TogglePaneIdFullscreen
        | CommandType::TogglePaneFrames
        | CommandType::SetPaneFrameStyle
        | CommandType::ToggleFloatingPanes
        | CommandType::NewPane
        | CommandType::TogglePaneEmbedOrEject
        | CommandType::TogglePaneEmbedOrEjectForPaneId
        | CommandType::UndoRenamePane
        | CommandType::CloseFocus
        | CommandType::ToggleActiveTabSync
        | CommandType::CloseFocusedTab
        | CommandType::UndoRenameTab
        | CommandType::QuitYinz
        | CommandType::PreviousSwapLayout
        | CommandType::NextSwapLayout
        | CommandType::GoToTabName
        | CommandType::FocusOrCreateTab
        | CommandType::GoToTab
        | CommandType::CloseTerminalPane
        | CommandType::ClosePluginPane
        | CommandType::FocusTerminalPane
        | CommandType::FocusPluginPane
        | CommandType::RenameTerminalPane
        | CommandType::RenamePluginPane
        | CommandType::SetPaneTitlePrefix
        | CommandType::SetPaneSubtitle
        | CommandType::SwitchSessionDestination
        | CommandType::SwitchSession
        | CommandType::DeleteDeadSession
        | CommandType::DeleteAllDeadSessions
        | CommandType::RenameSession
        | CommandType::Notify
        | CommandType::RenameTab
        | CommandType::DisconnectOtherClients
        | CommandType::ShowPaneWithId
        | CommandType::HidePaneWithId
        | CommandType::RerunCommandPane
        | CommandType::ResizePaneIdWithDirection
        | CommandType::CloseTabWithIndex
        | CommandType::BreakPanesToNewTab
        | CommandType::BreakPanesToTabWithIndex
        | CommandType::SwitchTabToId
        | CommandType::GoToTabWithId
        | CommandType::CloseTabWithId
        | CommandType::RenameTabWithId
        | CommandType::ArrangeProjectPanes
        | CommandType::SetProjectContext
        | CommandType::SetTabProjects
        | CommandType::BreakPanesToTabWithId
        | CommandType::ReloadPlugin
        | CommandType::SetFloatingPanePinned
        | CommandType::StackPanes
        | CommandType::ChangeFloatingPanesCoordinates
        | CommandType::PaneTabs
        | CommandType::CreateMultipane
        | CommandType::UpdateMultipane
        | CommandType::ReplaceMultipaneChild
        | CommandType::ChangeOverlayCoordinates
        | CommandType::TogglePaneBorderless
        | CommandType::SetPaneBorderless
        | CommandType::SetPaneColor
        | CommandType::SetPaneAgent
        | CommandType::SetPaneAgentRun
        | CommandType::SetQuitNotice
        | CommandType::GroupAndUngroupPanes
        | CommandType::HighlightAndUnhighlightPanes
        | CommandType::RevealPaneWithId
        | CommandType::CloseMultiplePanes
        | CommandType::FloatMultiplePanes
        | CommandType::EmbedMultiplePanes
        | CommandType::ReplacePaneWithExistingPane
        | CommandType::KillSessions
        | CommandType::KillSessionsAndReply
        | CommandType::DeleteDeadSessionAndReply
        | CommandType::DeleteAllDeadSessionsAndReply
        | CommandType::SendSigintToPaneId
        | CommandType::SendSigkillToPaneId
        | CommandType::OverrideLayout
        | CommandType::SaveLayout
        | CommandType::DeleteLayout
        | CommandType::RenameLayout
        | CommandType::EditLayout
        | CommandType::ShowFloatingPanes
        | CommandType::HideFloatingPanes
        | CommandType::SetPaneRegexHighlights
        | CommandType::ClearPaneHighlights
        | CommandType::WarpPaneToSession
        | CommandType::WarpPaneFromSession
        | CommandType::WarpTabToSession
        | CommandType::WarpTabFromSession
        | CommandType::AdoptOrphanedPane => PermissionType::ChangeApplicationState,
        CommandType::UnblockCliPipeInput
        | CommandType::BlockCliPipeInput
        | CommandType::CliPipeOutput => PermissionType::ReadCliPipes,
        CommandType::MessageToPlugin => PermissionType::MessageAndLaunchOtherPlugins,
        CommandType::ListClients
        | CommandType::DumpSessionLayout
        | CommandType::GetPanePid
        | CommandType::GetPaneRunningCommand
        | CommandType::GetPaneCwd
        | CommandType::GetPaneGitProbe
        | CommandType::GenerateRandomName
        | CommandType::GetLayoutDir
        | CommandType::GetUserPluginDir
        | CommandType::GetFocusedPaneInfo
        | CommandType::DumpLayout
        | CommandType::ParseLayout
        | CommandType::SaveSession
        | CommandType::CurrentSessionLastSavedTime
        | CommandType::GetPluginInfo
        | CommandType::GetPaneInfo
        | CommandType::GetTabInfo
        | CommandType::GetDisconnectTargets
        | CommandType::GetSessionInventory
        | CommandType::GetSessionList
        | CommandType::ListNotifications
        | CommandType::ListOrphanedPanes
        | CommandType::CommandPalette
        | CommandType::ResolveProject => PermissionType::ReadApplicationState,
        // One permission covers reading and writing the plugin's *own* namespace. Reaching a
        // `shared:` namespace additionally needs `SharedPluginData`, which is checked in the
        // handlers — this match sees only the command, not the namespace it names.
        CommandType::DataDefineEntity
        | CommandType::DataApply
        | CommandType::DataFetch
        | CommandType::DataCount
        | CommandType::DataWatch
        | CommandType::DataUnwatch
        | CommandType::DataCatalog => PermissionType::PluginData,
        CommandType::RebindKeys
        | CommandType::RegisterDefaultKeybindings
        | CommandType::InspectConfiguration
        | CommandType::EditConfiguration
        | CommandType::Reconfigure => PermissionType::Reconfigure,
        CommandType::ChangeHostFolder => PermissionType::FullHdAccess,
        // Nominal: the handler refuses anything that is not a builtin, and a builtin is never
        // asked. Listed so that the command is not permissionless on paper.
        CommandType::GrantPluginPermissions => PermissionType::ChangeApplicationState,
        CommandType::TestPlugin | CommandType::CancelPluginTest => PermissionType::AuthorPlugins,
        CommandType::ProjectConfigRead
        | CommandType::CheckPlugin
        | CommandType::FsRead
        | CommandType::FsList
        | CommandType::FsMetadata => PermissionType::ReadFiles,
        CommandType::ProjectConfigWrite
        | CommandType::FsWrite
        | CommandType::FsRemove
        | CommandType::FsCreateDir
        | CommandType::FsRename => PermissionType::WriteFiles,
        CommandType::InterceptKeyPresses | CommandType::ClearKeyPressesIntercepts => {
            PermissionType::InterceptInput
        },
        CommandType::GetPaneScrollback | CommandType::SubscribePaneOutput => {
            PermissionType::ReadPaneContents
        },
        // The same gate `RunAction` sits behind, and for the same reason: a menu item is a list
        // of actions the server will run as the user, whichever way round it was assembled.
        CommandType::RunAction | CommandType::SetContextMenu => PermissionType::RunActionsAsUser,
        CommandType::GetSessionEnvironmentVariables => {
            PermissionType::ReadSessionEnvironmentVariables
        },
        // Another plugin's configuration is session state, and is read the same way the
        // session list and the pane manifest are.
        CommandType::GetPluginAliasConfiguration
        | CommandType::GetPluginIdsForUrl
        | CommandType::GetPluginIdsForUrlWithConfiguration => PermissionType::ReadApplicationState,
        CommandType::OpenCommandPaneInNewTab => PermissionType::RunCommands,
        CommandType::OpenEditorPaneInNewTab => PermissionType::OpenFiles,
        CommandType::Pick => PermissionType::MessageAndLaunchOtherPlugins,
        // A clock. It says the machine's offset from UTC and nothing about the session,
        // which is less than `GetYinzVersion` already says, and a permission prompt in
        // front of drawing a timestamp would be answered blind every time.
        CommandType::GetLocalUtcOffset | CommandType::GetLocalTime => return None,
        // Reloading a plugin someone wrote is the second half of writing it.
        CommandType::ReloadPluginByUrl => PermissionType::AuthorPlugins,
        // Reading a plugin's log lines is the authoring loop's "what did it say" step, the same
        // act as `yinz plugins log`; it is the log of the whole server, so not `ReadFiles`.
        CommandType::ReadPluginLog => PermissionType::AuthorPlugins,
    })
}
