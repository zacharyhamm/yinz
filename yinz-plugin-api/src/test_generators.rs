//! Generators for plugin wire contract tests.
use hegel::generators;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;
use strum::IntoEnumIterator;
use yinz_types::agent::AgentSpawnSpec;
use yinz_types::command::CommandToRun;
use yinz_types::command::GitCheckout;
use yinz_types::data::CommandType;
use yinz_types::data::ConnectToSession;
use yinz_types::data::ContextMenuItem;
use yinz_types::data::EntityDef;
use yinz_types::data::EventType;
use yinz_types::data::FetchRequest;
use yinz_types::data::FileMetadata;
use yinz_types::data::HighlightLayer;
use yinz_types::data::HighlightStyle;
use yinz_types::data::HttpVerb;
use yinz_types::data::InputMode;
use yinz_types::data::MessageToPlugin;
use yinz_types::data::Mutation;
use yinz_types::data::NewPluginArgs;
use yinz_types::data::NotifyClick;
use yinz_types::data::PaneId;
use yinz_types::data::PermissionType;
use yinz_types::data::PluginCommand;
use yinz_types::data::PluginMessage;
use yinz_types::data::RegexHighlight;
use yinz_types::data::WatchMode;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::geometry::ResizeStrategy;
use yinz_types::input::actions::Action;
use yinz_types::internal_ids::ContextMenuItemId;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::DataWatchId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::keys::KeyWithModifier;
pub use yinz_types::test_generators::action_of_kind;
pub(crate) use yinz_types::test_generators::{
    agent_infos, any_action, booleans, bytes, context, directions, files_to_open,
    floating_pane_coordinates, input_modes, keys_with_modifier, layout_infos, optional_agent_infos,
    optional_booleans, optional_directions, optional_floating_pane_coordinates, optional_paths,
    optional_strings, optional_usize_as_u32, overlay_placements, pane_frame_styles, pane_id,
    pane_ids, pane_targets, paths, resizes, strings, u32s, up_to, usize_as_u32, warp_placements,
};

pub(crate) fn agent_spawn_specs(tc: &hegel::TestCase) -> AgentSpawnSpec {
    AgentSpawnSpec {
        kind: optional_strings(tc),
        command: optional_strings(tc),
        args: up_to(tc, 3, strings),
        model: optional_strings(tc),
        effort: optional_strings(tc),
        prompt: optional_strings(tc),
        branch: optional_strings(tc),
        base: optional_strings(tc),
        worktree: optional_booleans(tc),
        worktree_dir: optional_strings(tc),
        env: context(tc),
        permission_mode: optional_strings(tc),
        resume: optional_strings(tc),
        hooks: optional_booleans(tc),

        reuse: optional_booleans(tc),
        reuse_prompt: optional_strings(tc),
        allow_outside_worktree: optional_booleans(tc),
        bubblewrap: booleans(tc),
    }
}

pub(crate) fn paths_with_metadata(tc: &hegel::TestCase) -> Vec<(PathBuf, Option<FileMetadata>)> {
    up_to(tc, 3, |tc| {
        let path = paths(tc);
        let metadata = booleans(tc).then(|| FileMetadata {
            is_dir: booleans(tc),
            is_file: booleans(tc),
            is_symlink: booleans(tc),
            len: tc.draw(generators::integers::<u64>()),
        });
        (path, metadata)
    })
}

pub(crate) fn u64s(tc: &hegel::TestCase) -> u64 {
    tc.draw(generators::integers::<u64>())
}

/// `SetTimeoutPayload.seconds` is a `double`, which holds any `f64`; NaN is left out only because
/// no round trip of it can be asserted with `==`.
pub(crate) fn seconds(tc: &hegel::TestCase) -> f64 {
    tc.draw(generators::floats::<f64>().allow_nan(false))
}

pub(crate) fn event_types(tc: &hegel::TestCase) -> HashSet<EventType> {
    up_to(tc, 4, |tc| {
        tc.draw(generators::sampled_from(
            EventType::iter().collect::<Vec<_>>(),
        ))
    })
    .into_iter()
    .collect()
}

pub(crate) fn permission_types(tc: &hegel::TestCase) -> Vec<PermissionType> {
    up_to(tc, 4, |tc| {
        tc.draw(generators::sampled_from(
            PermissionType::iter().collect::<Vec<_>>(),
        ))
    })
}

pub(crate) fn commands_to_run(tc: &hegel::TestCase) -> CommandToRun {
    CommandToRun {
        path: paths(tc),
        args: up_to(tc, 3, strings),
        cwd: optional_paths(tc),
        target: pane_targets(tc),
        remote_default_shell: booleans(tc),
    }
}

pub(crate) fn git_checkouts(tc: &hegel::TestCase) -> GitCheckout {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => GitCheckout::Current,
        1 => GitCheckout::NewBranch {
            branch: strings(tc),
            base: strings(tc),
        },
        _ => GitCheckout::NewWorktree {
            branch: strings(tc),
            base: strings(tc),
            path: paths(tc),
        },
    }
}

pub(crate) fn plugin_messages(tc: &hegel::TestCase) -> PluginMessage {
    PluginMessage {
        name: strings(tc),
        payload: strings(tc),
        worker_name: optional_strings(tc),
    }
}

/// `SwitchSessionPayload.tab_position` is an `optional uint32`.
pub(crate) fn connect_to_sessions(tc: &hegel::TestCase) -> ConnectToSession {
    ConnectToSession {
        name: optional_strings(tc).map(Into::into),
        tab_position: optional_usize_as_u32(tc).map(TabPosition::new),
        pane_id: booleans(tc).then(|| PaneId::from_legacy(u32s(tc), booleans(tc))),
        layout: booleans(tc).then(|| layout_infos(tc)),
        cwd: optional_paths(tc),
    }
}

pub(crate) fn new_plugin_args(tc: &hegel::TestCase) -> NewPluginArgs {
    NewPluginArgs {
        should_float: optional_booleans(tc),
        pane_id_to_replace: booleans(tc).then(|| pane_id(tc)),
        pane_title: optional_strings(tc),
        cwd: optional_paths(tc),
        skip_cache: booleans(tc),
        should_focus: optional_booleans(tc),
    }
}

pub(crate) fn messages_to_plugin(tc: &hegel::TestCase) -> MessageToPlugin {
    MessageToPlugin {
        plugin_url: optional_strings(tc),
        destination_plugin_id: booleans(tc)
            .then(|| u32s(tc))
            .map(yinz_types::internal_ids::PluginId::new),
        plugin_config: context(tc),
        message_name: strings(tc),
        message_payload: optional_strings(tc),
        message_args: context(tc),
        new_plugin_args: booleans(tc).then(|| new_plugin_args(tc)),
        floating_pane_coordinates: optional_floating_pane_coordinates(tc),
    }
}

pub(crate) fn http_verbs(tc: &hegel::TestCase) -> HttpVerb {
    tc.draw(generators::sampled_from(vec![
        HttpVerb::Get,
        HttpVerb::Post,
        HttpVerb::Put,
        HttpVerb::Delete,
        HttpVerb::Patch,
    ]))
}

pub(crate) fn resize_strategies(tc: &hegel::TestCase) -> ResizeStrategy {
    ResizeStrategy {
        resize: resizes(tc),
        direction: optional_directions(tc),
        invert_on_boundaries: booleans(tc),
    }
}

pub(crate) fn highlight_styles(tc: &hegel::TestCase) -> HighlightStyle {
    let rgb = |tc: &hegel::TestCase| {
        booleans(tc).then(|| {
            (
                tc.draw(generators::integers::<u8>()),
                tc.draw(generators::integers::<u8>()),
                tc.draw(generators::integers::<u8>()),
            )
        })
    };
    match tc.draw(generators::integers::<u8>().max_value(10)) {
        0 => HighlightStyle::None,
        1 => HighlightStyle::Emphasis0,
        2 => HighlightStyle::Emphasis1,
        3 => HighlightStyle::Emphasis2,
        4 => HighlightStyle::Emphasis3,
        5 => HighlightStyle::BackgroundEmphasis0,
        6 => HighlightStyle::BackgroundEmphasis1,
        7 => HighlightStyle::BackgroundEmphasis2,
        8 => HighlightStyle::BackgroundEmphasis3,
        9 => HighlightStyle::CustomRgb {
            fg: rgb(tc),
            bg: rgb(tc),
        },
        _ => HighlightStyle::CustomIndex {
            fg: booleans(tc).then(|| tc.draw(generators::integers::<u8>())),
            bg: booleans(tc).then(|| tc.draw(generators::integers::<u8>())),
        },
    }
}

pub(crate) fn highlight_layers(tc: &hegel::TestCase) -> HighlightLayer {
    tc.draw(generators::sampled_from(vec![
        HighlightLayer::Hint,
        HighlightLayer::Tool,
        HighlightLayer::ActionFeedback,
    ]))
}

pub(crate) fn regex_highlights(tc: &hegel::TestCase) -> RegexHighlight {
    RegexHighlight {
        pattern: strings(tc),
        style: highlight_styles(tc),
        layer: highlight_layers(tc),
        context: context(tc),
        on_hover: booleans(tc),
        bold: booleans(tc),
        italic: booleans(tc),
        underline: booleans(tc),
        tooltip_text: optional_strings(tc),
    }
}

/// `actions` is drawn even though `ContextMenuItem` on the wire has no field for it: that the
/// wire drops it is part of the type's contract, and the property asserts the drop.
pub(crate) fn context_menu_items(tc: &hegel::TestCase) -> ContextMenuItem {
    ContextMenuItem {
        id: booleans(tc).then(|| u64s(tc)).map(ContextMenuItemId::new),
        label: strings(tc),
        shortcut: optional_strings(tc),
        actions: up_to(tc, 2, any_action),
        context: context(tc),
        disabled: booleans(tc),
        separator: booleans(tc),
    }
}

pub(crate) fn watch_modes(tc: &hegel::TestCase) -> WatchMode {
    tc.draw(generators::sampled_from(vec![
        WatchMode::Rows,
        WatchMode::Ids,
    ]))
}

pub(crate) fn entity_defs(tc: &hegel::TestCase) -> EntityDef {
    tc.draw(crate::plugin_data::tests::any_entity_def())
}

pub(crate) fn fetch_requests(tc: &hegel::TestCase) -> FetchRequest {
    tc.draw(crate::plugin_data::tests::any_fetch_request())
}

pub(crate) fn mutations(tc: &hegel::TestCase) -> Mutation {
    tc.draw(crate::plugin_data::tests::any_mutation())
}

fn pane_ids_with_coordinates(tc: &hegel::TestCase) -> Vec<(PaneId, FloatingPaneCoordinates)> {
    up_to(tc, 3, |tc| (pane_id(tc), floating_pane_coordinates(tc)))
}

fn keys_to_rebind(tc: &hegel::TestCase) -> Vec<(InputMode, KeyWithModifier, Vec<Action>)> {
    up_to(tc, 2, |tc| {
        (
            input_modes(tc),
            keys_with_modifier(tc),
            up_to(tc, 2, any_action),
        )
    })
}

fn keys_to_unbind(tc: &hegel::TestCase) -> Vec<(InputMode, KeyWithModifier)> {
    up_to(tc, 2, |tc| (input_modes(tc), keys_with_modifier(tc)))
}

fn tab_projects(tc: &hegel::TestCase) -> BTreeMap<TabId, String> {
    up_to(tc, 3, |tc| {
        (
            TabId::new(tc.draw(generators::integers::<usize>())),
            strings(tc),
        )
    })
    .into_iter()
    .collect()
}

/// A `PluginCommand` of the given kind with a freshly drawn payload.
///
/// As with `action_of_kind`, there is no wildcard arm: a new `PluginCommand` variant does not
/// compile until it has a generator here.
pub(crate) fn plugin_command_of_kind(tc: &hegel::TestCase, kind: CommandType) -> PluginCommand {
    match kind {
        CommandType::Subscribe => PluginCommand::Subscribe(event_types(tc)),
        CommandType::Unsubscribe => PluginCommand::Unsubscribe(event_types(tc)),
        CommandType::SetSelectable => PluginCommand::SetSelectable(booleans(tc)),
        // The whole `usize` is drawn on purpose: `CursorPosition` is a pair of `uint32`, and the
        // property pins what the wire does to the rest.
        CommandType::ShowCursor => PluginCommand::ShowCursor(booleans(tc).then(|| {
            (
                tc.draw(generators::integers::<usize>()),
                tc.draw(generators::integers::<usize>()),
            )
        })),
        CommandType::GetClientTimezone => PluginCommand::GetClientTimezone {
            client_id: tc
                .draw(generators::optional(generators::integers::<u16>()))
                .map(|id| yinz_types::internal_ids::ClientId::new(id as u32)),
        },
        CommandType::GetCellPixelSize => PluginCommand::GetCellPixelSize,
        CommandType::GetPluginIds => PluginCommand::GetPluginIds,
        CommandType::GetYinzVersion => PluginCommand::GetYinzVersion,
        CommandType::TestPlugin => PluginCommand::TestPlugin {
            name: "fixture.scm".into(),
            options: Default::default(),
            request_id: "test-1".into(),
        },
        CommandType::CancelPluginTest => PluginCommand::CancelPluginTest("test-1".into()),
        CommandType::CheckPlugin => PluginCommand::CheckPlugin {
            name: strings(tc),
            source: strings(tc),
            mode: match (booleans(tc), booleans(tc)) {
                (false, false) => yinz_types::authoring::PluginCheckMode::Syntax,
                (false, true) => yinz_types::authoring::PluginCheckMode::Semantic,
                (true, false) => yinz_types::authoring::PluginCheckMode::Format,
                (true, true) => yinz_types::authoring::PluginCheckMode::Prepare,
            },
        },
        CommandType::GetLocalTime => PluginCommand::GetLocalTime {
            epoch_ms: yinz_types::units::SignedUnixMillis::new(
                tc.draw(generators::integers::<i64>()),
            ),
        },
        CommandType::GetLocalUtcOffset => PluginCommand::GetLocalUtcOffset {
            client_id: tc
                .draw(generators::optional(generators::integers::<u16>()))
                .map(|id| yinz_types::internal_ids::ClientId::new(id as u32)),
            timezone: optional_strings(tc),
            epoch_ms: yinz_types::units::SignedUnixMillis::new(
                tc.draw(generators::integers::<i64>()),
            ),
        },
        CommandType::SetPaneAgent => PluginCommand::SetPaneAgent {
            pane_id: pane_id(tc),
            agent: optional_agent_infos(tc),
        },
        CommandType::SetPaneAgentRun => PluginCommand::SetPaneAgentRun {
            pane_id: pane_id(tc),
            run_id: optional_strings(tc),
        },
        CommandType::SetPaneAgentActivity => PluginCommand::SetPaneAgentActivity {
            pane_id: pane_id(tc),
            expected: agent_infos(tc),
            state: agent_infos(tc).state,
        },
        CommandType::InspectConfiguration => {
            PluginCommand::InspectConfiguration(Some("theme".into()))
        },
        CommandType::EditConfiguration => {
            PluginCommand::EditConfiguration(yinz_types::configuration::Edit {
                revision: strings(tc),
                destination: yinz_types::configuration::Destination::Session,
                patches: vec![],
                reset: vec![],
            })
        },
        CommandType::ResolveProject => PluginCommand::ResolveProject {
            directory: strings(tc).into(),
        },
        CommandType::ProjectConfigRead => PluginCommand::ProjectConfigRead {
            directory: strings(tc).into(),
        },
        CommandType::OpenPluginPaneInPlaceOfPaneId => {
            PluginCommand::OpenPluginPaneInPlaceOfPaneId {
                pane_id: pane_id(tc),
                plugin_url: strings(tc),
                configuration: context(tc),
                context: context(tc),
            }
        },
        CommandType::RestartAgentPane => PluginCommand::RestartAgentPane {
            pane_id: pane_id(tc),
            prompt: optional_strings(tc),
            session: yinz_types::agent::control::AgentRestartSession::Resume,
            context: context(tc),
        },
        CommandType::AgentControl => PluginCommand::AgentControl {
            id: strings(tc),
            request: strings(tc),
        },
        CommandType::HttpRespond => PluginCommand::HttpRespond {
            request_id: u64s(tc),
            response: strings(tc),
        },
        CommandType::HttpAssets => PluginCommand::HttpAssets {
            prefix: strings(tc),
            directory: strings(tc),
        },
        CommandType::HttpPublish => PluginCommand::HttpPublish {
            path: strings(tc),
            content_type: strings(tc),
            body: strings(tc),
        },
        CommandType::AgentView => PluginCommand::AgentView {
            id: strings(tc),
            request: strings(tc),
        },
        CommandType::ProjectConfigWrite => PluginCommand::ProjectConfigWrite {
            directory: strings(tc).into(),
            source: strings(tc),
            expected: optional_strings(tc),
        },
        CommandType::SpawnAgent => PluginCommand::SpawnAgent {
            spec: agent_spawn_specs(tc),
            source_directory: optional_paths(tc),
            cwd: optional_paths(tc),
            name: optional_strings(tc),
            context: context(tc),

            start_suppressed: booleans(tc),
        },
        CommandType::SetQuitNotice => PluginCommand::SetQuitNotice {
            notice: optional_strings(tc),
            guard: booleans(tc),
        },
        CommandType::GetPluginIdsForUrlWithConfiguration => {
            PluginCommand::GetPluginIdsForUrlWithConfiguration {
                url: strings(tc),
                configuration: BTreeMap::new(),
            }
        },
        CommandType::Pick => PluginCommand::Pick(yinz_types::pick::PickCommand::Request),
        CommandType::GetPluginInfo => PluginCommand::GetPluginInfo(None),
        CommandType::GetPluginIdsForUrl => PluginCommand::GetPluginIdsForUrl(strings(tc)),
        CommandType::ReloadPluginByUrl => PluginCommand::ReloadPluginByUrl(strings(tc)),
        CommandType::ReadPluginLog => PluginCommand::ReadPluginLog {
            name: strings(tc),
            lines: tc.draw(generators::integers::<usize>()),
        },
        CommandType::OpenFile => PluginCommand::OpenFile(files_to_open(tc), context(tc)),
        CommandType::OpenFileFloating => PluginCommand::OpenFileFloating(
            files_to_open(tc),
            optional_floating_pane_coordinates(tc),
            context(tc),
        ),
        CommandType::OpenTerminal => PluginCommand::OpenTerminal(files_to_open(tc)),
        CommandType::OpenTerminalFloating => PluginCommand::OpenTerminalFloating(
            files_to_open(tc),
            optional_floating_pane_coordinates(tc),
        ),
        CommandType::OpenCommandPane => {
            PluginCommand::OpenCommandPane(commands_to_run(tc), context(tc))
        },
        CommandType::OpenCommandPaneFloating => PluginCommand::OpenCommandPaneFloating(
            commands_to_run(tc),
            optional_floating_pane_coordinates(tc),
            context(tc),
        ),
        CommandType::OpenCommandPaneOverlay => PluginCommand::OpenCommandPaneOverlay(
            commands_to_run(tc),
            overlay_placements(tc),
            context(tc),
        ),
        CommandType::SwitchTabTo => PluginCommand::SwitchTabTo(TabNumber::new(u32s(tc))),
        CommandType::SetTimeout => {
            PluginCommand::SetTimeout(yinz_types::units::FractionalSeconds::new(seconds(tc)))
        },
        CommandType::ExecCmd => PluginCommand::ExecCmd(up_to(tc, 3, strings)),
        CommandType::PostMessageTo => PluginCommand::PostMessageTo(plugin_messages(tc)),
        CommandType::PostMessageToPlugin => PluginCommand::PostMessageToPlugin(plugin_messages(tc)),
        CommandType::HideSelf => PluginCommand::HideSelf,
        CommandType::ShowSelf => PluginCommand::ShowSelf(booleans(tc)),
        CommandType::SwitchToMode => PluginCommand::SwitchToMode(input_modes(tc)),
        CommandType::NewTabsWithLayout => PluginCommand::NewTabsWithLayout(strings(tc)),
        CommandType::NewTab => PluginCommand::NewTab {
            name: optional_strings(tc),
            cwd: optional_strings(tc),
        },
        CommandType::NewTabUnfocused => PluginCommand::NewTabUnfocused {
            name: optional_strings(tc),
            cwd: optional_strings(tc),
        },
        // `NewTiledPaneInTabPayload.tab_id` is a `uint32`.
        CommandType::NewTiledPaneInTab => PluginCommand::NewTiledPaneInTab {
            tab_id: TabId::new(usize_as_u32(tc)),
        },
        CommandType::ToggleFloatingPanes => PluginCommand::ToggleFloatingPanes {
            tab_id: booleans(tc)
                .then(|| u64s(tc))
                .map(TabId::try_from)
                .transpose()
                .unwrap(),
        },
        CommandType::NewPane => PluginCommand::NewPane,
        CommandType::GoToNextTab => PluginCommand::GoToNextTab,
        CommandType::GoToPreviousTab => PluginCommand::GoToPreviousTab,
        CommandType::Resize => PluginCommand::Resize(resizes(tc)),
        CommandType::ResizeWithDirection => {
            PluginCommand::ResizeWithDirection(resize_strategies(tc))
        },
        CommandType::FocusNextPane => PluginCommand::FocusNextPane,
        CommandType::FocusPreviousPane => PluginCommand::FocusPreviousPane,
        CommandType::FocusLastPane => PluginCommand::FocusLastPane,
        CommandType::MoveFocus => PluginCommand::MoveFocus(directions(tc)),
        CommandType::MoveFocusOrTab => PluginCommand::MoveFocusOrTab(directions(tc)),
        CommandType::Detach => PluginCommand::Detach,
        CommandType::EditScrollback => PluginCommand::EditScrollback,
        CommandType::Write => PluginCommand::Write(bytes(tc)),
        CommandType::WriteChars => PluginCommand::WriteChars(strings(tc)),
        CommandType::ToggleTab => PluginCommand::ToggleTab,
        CommandType::MovePane => PluginCommand::MovePane,
        CommandType::MovePaneWithDirection => PluginCommand::MovePaneWithDirection(directions(tc)),
        CommandType::ClearScreen => PluginCommand::ClearScreen,
        CommandType::ScrollUp => PluginCommand::ScrollUp,
        CommandType::ScrollDown => PluginCommand::ScrollDown,
        CommandType::ScrollToTop => PluginCommand::ScrollToTop,
        CommandType::ScrollToBottom => PluginCommand::ScrollToBottom,
        CommandType::PageScrollUp => PluginCommand::PageScrollUp,
        CommandType::PageScrollDown => PluginCommand::PageScrollDown,
        CommandType::ToggleFocusFullscreen => PluginCommand::ToggleFocusFullscreen,
        CommandType::ToggleFocusNoUiFullscreen => PluginCommand::ToggleFocusNoUiFullscreen,
        CommandType::TogglePaneFrames => PluginCommand::TogglePaneFrames,
        CommandType::SetPaneFrameStyle => PluginCommand::SetPaneFrameStyle(pane_frame_styles(tc)),
        CommandType::TogglePaneEmbedOrEject => PluginCommand::TogglePaneEmbedOrEject,
        CommandType::UndoRenamePane => PluginCommand::UndoRenamePane,
        CommandType::CloseFocus => PluginCommand::CloseFocus,
        CommandType::ToggleActiveTabSync => PluginCommand::ToggleActiveTabSync,
        CommandType::CloseFocusedTab => PluginCommand::CloseFocusedTab,
        CommandType::UndoRenameTab => PluginCommand::UndoRenameTab,
        CommandType::QuitYinz => PluginCommand::QuitYinz,
        CommandType::PreviousSwapLayout => PluginCommand::PreviousSwapLayout,
        CommandType::NextSwapLayout => PluginCommand::NextSwapLayout,
        CommandType::GoToTabName => PluginCommand::GoToTabName(strings(tc)),
        CommandType::FocusOrCreateTab => PluginCommand::FocusOrCreateTab(strings(tc)),
        CommandType::GoToTab => PluginCommand::GoToTab(TabPosition::new(u32s(tc) as usize)),
        CommandType::StartOrReloadPlugin => PluginCommand::StartOrReloadPlugin(strings(tc)),
        CommandType::CloseTerminalPane => {
            PluginCommand::CloseTerminalPane(yinz_types::internal_ids::TerminalId::new(u32s(tc)))
        },
        CommandType::ClosePluginPane => {
            PluginCommand::ClosePluginPane(yinz_types::internal_ids::PluginId::new(u32s(tc)))
        },
        CommandType::FocusTerminalPane => PluginCommand::FocusTerminalPane(
            yinz_types::internal_ids::TerminalId::new(u32s(tc)),
            booleans(tc),
            booleans(tc),
        ),
        CommandType::FocusPluginPane => PluginCommand::FocusPluginPane(
            yinz_types::internal_ids::PluginId::new(u32s(tc)),
            booleans(tc),
            booleans(tc),
        ),
        CommandType::RenameTerminalPane => PluginCommand::RenameTerminalPane(
            yinz_types::internal_ids::TerminalId::new(u32s(tc)),
            strings(tc),
        ),
        CommandType::RenamePluginPane => PluginCommand::RenamePluginPane(
            yinz_types::internal_ids::PluginId::new(u32s(tc)),
            strings(tc),
        ),
        CommandType::RenameTab => {
            PluginCommand::RenameTab(TabPosition::new(u32s(tc) as usize), strings(tc))
        },
        CommandType::ReportPanic => PluginCommand::ReportPanic(strings(tc)),
        CommandType::RequestPluginPermissions => {
            PluginCommand::RequestPluginPermissions(permission_types(tc))
        },
        CommandType::SwitchSession => PluginCommand::SwitchSession(connect_to_sessions(tc)),
        CommandType::DeleteDeadSession => PluginCommand::DeleteDeadSession(strings(tc).into()),
        CommandType::DeleteAllDeadSessions => PluginCommand::DeleteAllDeadSessions,
        CommandType::OpenTerminalInPlace => PluginCommand::OpenTerminalInPlace(files_to_open(tc)),
        CommandType::OpenFileInPlace => {
            PluginCommand::OpenFileInPlace(files_to_open(tc), context(tc))
        },
        CommandType::OpenCommandPaneInPlace => {
            PluginCommand::OpenCommandPaneInPlace(commands_to_run(tc), context(tc))
        },
        CommandType::RunCommand => PluginCommand::RunCommand(
            up_to(tc, 3, strings),
            context(tc),
            paths(tc),
            context(tc),
            optional_usize_as_u32(tc),
            None,
        ),
        CommandType::WebRequest => PluginCommand::WebRequest(
            strings(tc),
            http_verbs(tc),
            context(tc),
            bytes(tc),
            context(tc),
            optional_strings(tc),
            optional_strings(tc),
        ),
        CommandType::CancelWebRequest => PluginCommand::CancelWebRequest(strings(tc)),
        CommandType::RenameSession => PluginCommand::RenameSession(strings(tc).into()),
        CommandType::Notify => {
            // A click is only carried by the pane-bearing payload, so generating one without a
            // pane would ask the round-trip to preserve something the wire cannot hold.
            let pane = booleans(tc).then(|| pane_id(tc));
            let click = pane.and_then(|_| {
                booleans(tc).then(|| NotifyClick {
                    message: strings(tc),
                    payload: strings(tc),
                })
            });
            PluginCommand::Notify(strings(tc), pane, click)
        },
        CommandType::UnblockCliPipeInput => PluginCommand::UnblockCliPipeInput(strings(tc)),
        CommandType::BlockCliPipeInput => PluginCommand::BlockCliPipeInput(strings(tc)),
        CommandType::CliPipeOutput => PluginCommand::CliPipeOutput(strings(tc), strings(tc)),
        CommandType::MessageToPlugin => PluginCommand::MessageToPlugin(messages_to_plugin(tc)),
        CommandType::DisconnectOtherClients => PluginCommand::DisconnectOtherClients,
        CommandType::KillSessions => {
            PluginCommand::KillSessions(up_to(tc, 3, strings).into_iter().map(Into::into).collect())
        },
        CommandType::ScanHostFolder => PluginCommand::ScanHostFolder(paths(tc)),
        CommandType::WatchFilesystem => PluginCommand::WatchFilesystem,
        CommandType::FsRead => PluginCommand::FsRead {
            path: paths(tc),
            max_bytes: optional_usize_as_u32(tc),
            offset: booleans(tc).then(|| tc.draw(generators::integers::<u64>())),
        },
        CommandType::FsList => PluginCommand::FsList { path: paths(tc) },
        CommandType::FsMetadata => PluginCommand::FsMetadata {
            path: paths(tc),
            follow_symlinks: booleans(tc),
        },
        CommandType::FsWrite => PluginCommand::FsWrite {
            path: paths(tc),
            contents: bytes(tc),
            append: booleans(tc),
            create_dirs: booleans(tc),
        },
        CommandType::FsRemove => PluginCommand::FsRemove {
            path: paths(tc),
            recursive: booleans(tc),
        },
        CommandType::FsCreateDir => PluginCommand::FsCreateDir {
            path: paths(tc),
            all: booleans(tc),
        },
        CommandType::FsRename => PluginCommand::FsRename {
            from: paths(tc),
            to: paths(tc),
        },
        CommandType::GrantPluginPermissions => PluginCommand::GrantPluginPermissions {
            plugin_url: strings(tc),
            permissions: permission_types(tc),
        },
        // `DumpSessionLayoutPayload.tab_index` is an `optional uint32`.
        CommandType::DumpSessionLayout => PluginCommand::DumpSessionLayout {
            tab_index: optional_usize_as_u32(tc).map(TabId::new),
        },
        CommandType::CloseSelf => PluginCommand::CloseSelf,
        CommandType::NewTabsWithLayoutInfo => {
            PluginCommand::NewTabsWithLayoutInfo(layout_infos(tc))
        },
        CommandType::Reconfigure => PluginCommand::Reconfigure(strings(tc), booleans(tc)),
        CommandType::HidePaneWithId => PluginCommand::HidePaneWithId(pane_id(tc)),
        CommandType::ShowPaneWithId => {
            PluginCommand::ShowPaneWithId(pane_id(tc), booleans(tc), booleans(tc))
        },
        CommandType::OpenCommandPaneBackground => {
            PluginCommand::OpenCommandPaneBackground(commands_to_run(tc), context(tc))
        },
        CommandType::RerunCommandPane => {
            PluginCommand::RerunCommandPane(yinz_types::internal_ids::TerminalId::new(u32s(tc)))
        },
        CommandType::RunCommandInPane => PluginCommand::RunCommandInPane {
            pane_id: pane_id(tc),
            command: commands_to_run(tc),
            context: context(tc),
            keep_scrollback: booleans(tc),
        },
        CommandType::ResizePaneIdWithDirection => {
            PluginCommand::ResizePaneIdWithDirection(resize_strategies(tc), pane_id(tc))
        },
        CommandType::EditScrollbackForPaneWithId => {
            PluginCommand::EditScrollbackForPaneWithId(pane_id(tc))
        },
        CommandType::GetPaneScrollback => PluginCommand::GetPaneScrollback {
            pane_id: pane_id(tc),
            get_full_scrollback: booleans(tc),
        },
        CommandType::WriteToPaneId => PluginCommand::WriteToPaneId(bytes(tc), pane_id(tc)),
        CommandType::WriteCharsToPaneId => {
            PluginCommand::WriteCharsToPaneId(strings(tc), pane_id(tc))
        },
        CommandType::SendSigintToPaneId => PluginCommand::SendSigintToPaneId(pane_id(tc)),
        CommandType::SendSigkillToPaneId => PluginCommand::SendSigkillToPaneId(pane_id(tc)),
        CommandType::GetPanePid => PluginCommand::GetPanePid {
            pane_id: pane_id(tc),
        },
        CommandType::GetPaneRunningCommand => PluginCommand::GetPaneRunningCommand {
            pane_id: pane_id(tc),
        },
        CommandType::GetPaneCwd => PluginCommand::GetPaneCwd {
            pane_id: pane_id(tc),
        },
        CommandType::GetPaneGitProbe => PluginCommand::GetPaneGitProbe {
            refresh_token: Some("refresh-test".into()),
            pane_id: pane_id(tc),
            cwd: "/work/project".into(),
        },
        CommandType::MovePaneWithPaneId => PluginCommand::MovePaneWithPaneId(pane_id(tc)),
        CommandType::MovePaneWithPaneIdInDirection => {
            PluginCommand::MovePaneWithPaneIdInDirection(pane_id(tc), directions(tc))
        },
        CommandType::ClearScreenForPaneId => PluginCommand::ClearScreenForPaneId(pane_id(tc)),
        CommandType::ScrollUpInPaneId => PluginCommand::ScrollUpInPaneId(pane_id(tc)),
        CommandType::ScrollDownInPaneId => PluginCommand::ScrollDownInPaneId(pane_id(tc)),
        CommandType::ScrollToTopInPaneId => PluginCommand::ScrollToTopInPaneId(pane_id(tc)),
        CommandType::ScrollToBottomInPaneId => PluginCommand::ScrollToBottomInPaneId(pane_id(tc)),
        CommandType::PageScrollUpInPaneId => PluginCommand::PageScrollUpInPaneId(pane_id(tc)),
        CommandType::PageScrollDownInPaneId => PluginCommand::PageScrollDownInPaneId(pane_id(tc)),
        CommandType::TogglePaneIdFullscreen => PluginCommand::TogglePaneIdFullscreen(pane_id(tc)),
        CommandType::TogglePaneEmbedOrEjectForPaneId => {
            PluginCommand::TogglePaneEmbedOrEjectForPaneId(pane_id(tc))
        },
        // `CloseTabWithIndexPayload.tab_index` is a `uint32`.
        CommandType::CloseTabWithIndex => {
            PluginCommand::CloseTabWithIndex(TabPosition::new(usize_as_u32(tc)))
        },
        CommandType::BreakPanesToNewTab => {
            PluginCommand::BreakPanesToNewTab(pane_ids(tc), optional_strings(tc), booleans(tc))
        },
        // `BreakPanesToTabWithIndexPayload.tab_index` is a `uint32`.
        CommandType::BreakPanesToTabWithIndex => PluginCommand::BreakPanesToTabWithIndex(
            pane_ids(tc),
            TabPosition::new(usize_as_u32(tc)),
            booleans(tc),
        ),
        CommandType::SwitchTabToId => {
            PluginCommand::SwitchTabToId(TabId::try_from(u64s(tc)).unwrap())
        },
        CommandType::GoToTabWithId => {
            PluginCommand::GoToTabWithId(TabId::try_from(u64s(tc)).unwrap())
        },
        CommandType::CloseTabWithId => {
            PluginCommand::CloseTabWithId(TabId::try_from(u64s(tc)).unwrap())
        },
        CommandType::RenameTabWithId => {
            PluginCommand::RenameTabWithId(TabId::try_from(u64s(tc)).unwrap(), strings(tc))
        },
        CommandType::SetProjectContext => PluginCommand::SetProjectContext(
            vec![strings(tc)],
            vec![(pane_id(tc), strings(tc))],
            [(strings(tc), strings(tc))].into(),
        ),
        CommandType::SetTabProjects => PluginCommand::SetTabProjects(tab_projects(tc)),
        CommandType::BreakPanesToTabWithId => PluginCommand::BreakPanesToTabWithId(
            pane_ids(tc),
            TabId::try_from(u64s(tc)).unwrap(),
            booleans(tc),
        ),
        CommandType::ReloadPlugin => {
            PluginCommand::ReloadPlugin(yinz_types::internal_ids::PluginId::new(u32s(tc)))
        },
        CommandType::LoadNewPlugin => PluginCommand::LoadNewPlugin {
            url: strings(tc),
            config: context(tc),
            load_in_background: booleans(tc),
            skip_plugin_cache: booleans(tc),
            cwd: optional_paths(tc),
        },
        CommandType::RebindKeys => PluginCommand::RebindKeys {
            keys_to_rebind: keys_to_rebind(tc),
            keys_to_unbind: keys_to_unbind(tc),
            write_config_to_disk: booleans(tc),
        },
        CommandType::CommandPalette => {
            PluginCommand::CommandPalette(yinz_types::command_palette::Command::Unregister {
                registration: "test".into(),
            })
        },
        CommandType::RegisterAgentProvider => {
            PluginCommand::RegisterAgentProvider(yinz_types::agent::provider::AgentProvider {
                kind: "fixture-agent".into(),
                executable: "fixture-agent".into(),
                ..Default::default()
            })
        },
        CommandType::GetAgentProviders => PluginCommand::GetAgentProviders,
        CommandType::GetProjectProviders => PluginCommand::GetProjectProviders,
        CommandType::RegisterProjectProvider => {
            PluginCommand::RegisterProjectProvider(Default::default())
        },
        CommandType::ValidateProjectMatch => {
            PluginCommand::ValidateProjectMatch(serde_json::json!({}))
        },
        CommandType::PublishProjectLocations => {
            PluginCommand::PublishProjectLocations(serde_json::json!({}))
        },
        CommandType::RegisterDefaultKeybindings => {
            PluginCommand::RegisterDefaultKeybindings(strings(tc))
        },
        CommandType::ListClients => PluginCommand::ListClients,
        CommandType::ChangeHostFolder => PluginCommand::ChangeHostFolder(paths(tc)),
        CommandType::SetFloatingPanePinned => {
            PluginCommand::SetFloatingPanePinned(pane_id(tc), booleans(tc))
        },
        CommandType::StackPanes => PluginCommand::StackPanes(pane_ids(tc)),
        CommandType::ChangeFloatingPanesCoordinates => {
            PluginCommand::ChangeFloatingPanesCoordinates(pane_ids_with_coordinates(tc))
        },
        CommandType::PaneTabs => {
            PluginCommand::PaneTabs(yinz_types::pane_tabs::PaneTabsCommand::Select {
                id: yinz_types::pane_tabs::PaneTabsId(tc.draw(generators::integers::<u64>())),
                index: 0,
            })
        },
        CommandType::CreateMultipane => {
            PluginCommand::CreateMultipane(yinz_types::multipane::CreateMultipane {
                children: [multipane_child(tc), multipane_child(tc)],
                placement: overlay_placements(tc),
                split: multipane_split(tc),
                focus: booleans(tc).then(|| usize::from(booleans(tc))),
                min_cells: tc.draw(generators::integers::<u16>()),
            })
        },
        CommandType::UpdateMultipane => {
            PluginCommand::UpdateMultipane(yinz_types::multipane::UpdateMultipane {
                id: yinz_types::multipane::MultipaneId(tc.draw(generators::integers::<u64>())),
                coordinates: booleans(tc).then(|| floating_pane_coordinates(tc)),
                split: booleans(tc).then(|| multipane_split(tc)),
                override_user_placement: booleans(tc),
                background_opacity: booleans(tc)
                    .then(|| tc.draw(generators::integers::<u8>().max_value(100))),
            })
        },
        CommandType::ReplaceMultipaneChild => {
            PluginCommand::ReplaceMultipaneChild(yinz_types::multipane::ReplaceMultipaneChild {
                id: yinz_types::multipane::MultipaneId(tc.draw(generators::integers::<u64>())),
                slot: usize::from(booleans(tc)),
                child: multipane_child(tc),
            })
        },
        CommandType::ChangeOverlayCoordinates => PluginCommand::ChangeOverlayCoordinates {
            placements: pane_ids_with_coordinates(tc),
            override_user_placement: booleans(tc),
        },
        CommandType::TogglePaneBorderless => PluginCommand::TogglePaneBorderless(pane_id(tc)),
        CommandType::SetPaneBorderless => {
            PluginCommand::SetPaneBorderless(pane_id(tc), booleans(tc))
        },
        CommandType::OpenCommandPaneNearPlugin => {
            PluginCommand::OpenCommandPaneNearPlugin(commands_to_run(tc), context(tc))
        },
        CommandType::OpenCommandPaneInGitCheckoutNearPlugin => {
            PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
                commands_to_run(tc),
                paths(tc),
                git_checkouts(tc),
            )
        },
        CommandType::OpenTerminalNearPlugin => {
            PluginCommand::OpenTerminalNearPlugin(files_to_open(tc))
        },
        CommandType::OpenTerminalFloatingNearPlugin => {
            PluginCommand::OpenTerminalFloatingNearPlugin(
                files_to_open(tc),
                optional_floating_pane_coordinates(tc),
            )
        },
        CommandType::OpenTerminalInPlaceOfPlugin => {
            PluginCommand::OpenTerminalInPlaceOfPlugin(files_to_open(tc), booleans(tc))
        },
        CommandType::OpenCommandPaneFloatingNearPlugin => {
            PluginCommand::OpenCommandPaneFloatingNearPlugin(
                commands_to_run(tc),
                optional_floating_pane_coordinates(tc),
                context(tc),
            )
        },
        CommandType::OpenCommandPaneInPlaceOfPlugin => {
            PluginCommand::OpenCommandPaneInPlaceOfPlugin(
                commands_to_run(tc),
                booleans(tc),
                context(tc),
            )
        },
        CommandType::OpenFileNearPlugin => {
            PluginCommand::OpenFileNearPlugin(files_to_open(tc), context(tc))
        },
        CommandType::OpenFileFloatingNearPlugin => PluginCommand::OpenFileFloatingNearPlugin(
            files_to_open(tc),
            optional_floating_pane_coordinates(tc),
            context(tc),
        ),
        CommandType::OpenFileInPlaceOfPlugin => {
            PluginCommand::OpenFileInPlaceOfPlugin(files_to_open(tc), booleans(tc), context(tc))
        },
        CommandType::GroupAndUngroupPanes => {
            PluginCommand::GroupAndUngroupPanes(pane_ids(tc), pane_ids(tc), booleans(tc))
        },
        CommandType::HighlightAndUnhighlightPanes => {
            PluginCommand::HighlightAndUnhighlightPanes(pane_ids(tc), pane_ids(tc))
        },
        CommandType::RevealPaneWithId => {
            PluginCommand::RevealPaneWithId(pane_id(tc), optional_strings(tc))
        },
        CommandType::CloseMultiplePanes => PluginCommand::CloseMultiplePanes(pane_ids(tc)),
        CommandType::FloatMultiplePanes => PluginCommand::FloatMultiplePanes(pane_ids(tc)),
        CommandType::EmbedMultiplePanes => PluginCommand::EmbedMultiplePanes(pane_ids(tc)),
        CommandType::SetSelfMouseSelectionSupport => {
            PluginCommand::SetSelfMouseSelectionSupport(booleans(tc))
        },
        CommandType::InterceptKeyPresses => PluginCommand::InterceptKeyPresses,
        CommandType::ClearKeyPressesIntercepts => PluginCommand::ClearKeyPressesIntercepts,
        CommandType::ReplacePaneWithExistingPane => {
            PluginCommand::ReplacePaneWithExistingPane(pane_id(tc), pane_id(tc), booleans(tc))
        },
        CommandType::RunAction => PluginCommand::RunAction(any_action(tc), context(tc)),
        CommandType::CopyToClipboard => PluginCommand::CopyToClipboard(strings(tc)),
        CommandType::OverrideLayout => PluginCommand::OverrideLayout(
            layout_infos(tc),
            booleans(tc),
            booleans(tc),
            booleans(tc),
            context(tc),
        ),
        CommandType::SaveLayout => PluginCommand::SaveLayout {
            layout_name: strings(tc),
            layout_kdl: strings(tc),
            overwrite: booleans(tc),
        },
        CommandType::DeleteLayout => PluginCommand::DeleteLayout {
            layout_name: strings(tc),
        },
        CommandType::RenameLayout => PluginCommand::RenameLayout {
            old_layout_name: strings(tc),
            new_layout_name: strings(tc),
        },
        CommandType::EditLayout => PluginCommand::EditLayout {
            layout_name: strings(tc),
            context: context(tc),
        },
        CommandType::GenerateRandomName => PluginCommand::GenerateRandomName,
        CommandType::DumpLayout => PluginCommand::DumpLayout(strings(tc)),
        CommandType::ParseLayout => PluginCommand::ParseLayout(strings(tc)),
        CommandType::GetLayoutDir => PluginCommand::GetLayoutDir,
        CommandType::GetUserPluginDir => PluginCommand::GetUserPluginDir,
        CommandType::GetFocusedPaneInfo => PluginCommand::GetFocusedPaneInfo,
        CommandType::SaveSession => PluginCommand::SaveSession,
        CommandType::CurrentSessionLastSavedTime => PluginCommand::CurrentSessionLastSavedTime,
        CommandType::SubscribePaneOutput => PluginCommand::SubscribePaneOutput(Some(context(tc))),
        CommandType::GetPaneInfo => PluginCommand::GetPaneInfo(pane_id(tc)),
        // `GetTabInfoPayload.tab_id` is a `uint64`, which holds any `usize`.
        CommandType::GetTabInfo => {
            PluginCommand::GetTabInfo(TabId::new(tc.draw(generators::integers::<usize>())))
        },
        CommandType::GetSessionEnvironmentVariables => {
            PluginCommand::GetSessionEnvironmentVariables
        },
        CommandType::GetPluginAliasConfiguration => {
            PluginCommand::GetPluginAliasConfiguration(strings(tc))
        },
        CommandType::OpenCommandPaneInNewTab => {
            PluginCommand::OpenCommandPaneInNewTab(commands_to_run(tc), context(tc))
        },
        CommandType::OpenPluginPaneInNewTab => PluginCommand::OpenPluginPaneInNewTab {
            plugin_url: strings(tc),
            configuration: context(tc),
            context: context(tc),
        },
        CommandType::OpenEditorPaneInNewTab => {
            PluginCommand::OpenEditorPaneInNewTab(files_to_open(tc), context(tc))
        },
        CommandType::OpenCommandPaneInPlaceOfPaneId => {
            PluginCommand::OpenCommandPaneInPlaceOfPaneId(
                pane_id(tc),
                commands_to_run(tc),
                booleans(tc),
                context(tc),
            )
        },
        CommandType::OpenTerminalPaneInPlaceOfPaneId => {
            PluginCommand::OpenTerminalPaneInPlaceOfPaneId(
                pane_id(tc),
                files_to_open(tc),
                booleans(tc),
            )
        },
        CommandType::OpenEditPaneInPlaceOfPaneId => PluginCommand::OpenEditPaneInPlaceOfPaneId(
            pane_id(tc),
            files_to_open(tc),
            booleans(tc),
            context(tc),
        ),
        // `HideFloatingPanesPayload.tab_id` and `ShowFloatingPanesPayload.tab_id` are
        // `optional uint32`.
        CommandType::HideFloatingPanes => PluginCommand::HideFloatingPanes {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        CommandType::ShowFloatingPanes => PluginCommand::ShowFloatingPanes {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        CommandType::SetPaneColor => {
            PluginCommand::SetPaneColor(pane_id(tc), optional_strings(tc), optional_strings(tc))
        },
        CommandType::SetPaneRegexHighlights => {
            PluginCommand::SetPaneRegexHighlights(pane_id(tc), up_to(tc, 3, regex_highlights))
        },
        CommandType::SetContextMenu => PluginCommand::SetContextMenu {
            request_id: ContextMenuRequestId::new(u64s(tc)),
            items: up_to(tc, 3, context_menu_items),
        },
        CommandType::ClearPaneHighlights => PluginCommand::ClearPaneHighlights(pane_id(tc)),
        CommandType::OpenPluginPaneFloating => PluginCommand::OpenPluginPaneFloating {
            plugin_url: strings(tc),
            configuration: context(tc),
            floating_pane_coordinates: optional_floating_pane_coordinates(tc),
            context: context(tc),
        },
        CommandType::GetDisconnectTargets => PluginCommand::GetDisconnectTargets,
        CommandType::GetSessionInventory => PluginCommand::GetSessionInventory("inventory".into()),
        CommandType::SwitchSessionDestination => PluginCommand::SwitchSessionDestination(
            yinz_types::session_destination::SwitchRequest::Status("switch".into()),
        ),
        CommandType::UpdateYinz => PluginCommand::UpdateYinz,
        CommandType::UpdateYinzPrerelease => PluginCommand::UpdateYinzPrerelease,
        CommandType::UpdateYinzFromSource => PluginCommand::UpdateYinzFromSource,
        CommandType::YinzUpdateStatus => PluginCommand::YinzUpdateStatus,
        CommandType::GetSessionList => PluginCommand::GetSessionList,
        CommandType::KillSessionsAndReply => PluginCommand::KillSessionsAndReply(
            up_to(tc, 3, strings).into_iter().map(Into::into).collect(),
        ),
        CommandType::DeleteDeadSessionAndReply => {
            PluginCommand::DeleteDeadSessionAndReply(strings(tc).into())
        },
        CommandType::DeleteAllDeadSessionsAndReply => PluginCommand::DeleteAllDeadSessionsAndReply,
        CommandType::FocusHostSession => PluginCommand::FocusHostSession,
        CommandType::ResolveNotification => PluginCommand::ResolveNotification {
            id: yinz_types::internal_ids::NotificationId::new(1),
            decision: yinz_types::notifications::NotificationDecision::Pass,
        },
        CommandType::ListNotifications => PluginCommand::ListNotifications,
        CommandType::WarpPaneToSession => PluginCommand::WarpPaneToSession {
            pane_id: pane_id(tc),
            to_session: strings(tc).into(),
            placement: warp_placements(tc),
        },
        CommandType::WarpPaneFromSession => PluginCommand::WarpPaneFromSession {
            pane_id: pane_id(tc),
            from_session: strings(tc).into(),
            placement: warp_placements(tc),
        },
        // `WarpTabToSessionPayload.tab_id` and `WarpTabFromSessionPayload.tab_id` are
        // `uint32`.
        CommandType::WarpTabToSession => PluginCommand::WarpTabToSession {
            tab_id: TabId::new(usize_as_u32(tc)),
            to_session: strings(tc).into(),
        },
        CommandType::WarpTabFromSession => PluginCommand::WarpTabFromSession {
            tab_id: TabId::new(usize_as_u32(tc)),
            from_session: strings(tc).into(),
        },
        CommandType::ListOrphanedPanes => PluginCommand::ListOrphanedPanes,
        CommandType::AdoptOrphanedPane => PluginCommand::AdoptOrphanedPane {
            host_id: SupervisorId::new(strings(tc)),
            placement: warp_placements(tc),
        },
        CommandType::DataDefineEntity => PluginCommand::DataDefineEntity {
            namespace: optional_strings(tc),
            def: entity_defs(tc),
            allow_destructive: booleans(tc),
        },
        CommandType::DataApply => PluginCommand::DataApply {
            namespace: optional_strings(tc),
            mutations: up_to(tc, 3, mutations),
        },
        CommandType::DataFetch => PluginCommand::DataFetch {
            request: fetch_requests(tc),
        },
        CommandType::DataCount => PluginCommand::DataCount {
            request: fetch_requests(tc),
        },
        CommandType::DataWatch => PluginCommand::DataWatch {
            request: fetch_requests(tc),
            mode: watch_modes(tc),
        },
        CommandType::DataUnwatch => PluginCommand::DataUnwatch {
            watch_id: DataWatchId::new(u64s(tc)),
        },
        CommandType::DataCatalog => PluginCommand::DataCatalog {
            namespace: optional_strings(tc),
        },
        CommandType::SetPaneTitlePrefix => PluginCommand::SetPaneTitlePrefix {
            pane_id: pane_id(tc),
            prefix: optional_strings(tc),
        },
        CommandType::SetPaneSubtitle => PluginCommand::SetPaneSubtitle {
            pane_id: pane_id(tc),
            subtitle: optional_strings(tc),
        },
    }
}

fn multipane_split(tc: &hegel::TestCase) -> yinz_types::multipane::MultipaneSplit {
    use yinz_types::multipane::*;
    MultipaneSplit {
        direction: if booleans(tc) {
            MultipaneDirection::LeftRight
        } else {
            MultipaneDirection::TopBottom
        },
        ratio: tc.draw(generators::integers::<u16>()) % 9999 + 1,
    }
}

fn multipane_child(tc: &hegel::TestCase) -> yinz_types::multipane::MultipaneChild {
    use yinz_types::multipane::MultipaneChild;
    if booleans(tc) {
        MultipaneChild::Existing(pane_id(tc))
    } else if booleans(tc) {
        MultipaneChild::Terminal(commands_to_run(tc))
    } else {
        MultipaneChild::Plugin {
            url: strings(tc),
            configuration: context(tc),
        }
    }
}
