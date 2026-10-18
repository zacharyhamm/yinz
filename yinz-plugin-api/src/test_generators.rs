//! Generators for the values the protobuf round-trip properties draw, written once so that
//! `Action`, `Event` and `PluginCommand` describe each leaf type in one place.
//!
//! Every generator is a plain `fn(&hegel::TestCase) -> T` that draws in sequence, so they compose
//! by calling each other. Where one is narrower than the type it builds, a comment quotes the
//! wire field that makes the rest of the domain unrepresentable - the narrowing is part of the
//! specification, not a way past a failing case.

use hegel::generators;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use strum::IntoEnumIterator;
use yinz_types::geometry::{PercentOrFixed, SplitDirection, SplitSize};
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::internal_ids::ContextMenuItemId;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::DataWatchId;
use yinz_types::internal_ids::QueueJobId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentSpawnSpec, AgentState},
    command::{CommandToRun, GitCheckout, PaneTarget},
    geometry::{Direction, FloatingPaneCoordinates, Resize, ResizeStrategy},
};
use {
    yinz_types::data::CommandOrPlugin, yinz_types::data::CommandType,
    yinz_types::data::ConnectToSession, yinz_types::data::ContextMenuItem,
    yinz_types::data::EntityDef, yinz_types::data::EventType, yinz_types::data::FetchRequest,
    yinz_types::data::FileMetadata, yinz_types::data::FileToOpen, yinz_types::data::HighlightLayer,
    yinz_types::data::HighlightStyle, yinz_types::data::HttpVerb, yinz_types::data::InputMode,
    yinz_types::data::LayoutInfo, yinz_types::data::LayoutMetadata,
    yinz_types::data::MessageToPlugin, yinz_types::data::Mutation,
    yinz_types::data::NewPanePlacement, yinz_types::data::NewPluginArgs,
    yinz_types::data::NotifyClick, yinz_types::data::OriginatingPlugin,
    yinz_types::data::OverlayKind, yinz_types::data::OverlayPlacement, yinz_types::data::PaneId,
    yinz_types::data::PaneMetadata, yinz_types::data::PermissionType,
    yinz_types::data::PluginCommand, yinz_types::data::PluginMessage, yinz_types::data::PluginTag,
    yinz_types::data::RegexHighlight, yinz_types::data::RemotePaneHandoff,
    yinz_types::data::TabMetadata, yinz_types::data::UnblockCondition,
    yinz_types::data::WarpPlacement, yinz_types::data::WarpedPane, yinz_types::data::WarpedTab,
    yinz_types::data::WatchMode,
};
use {
    yinz_types::input::actions::Action, yinz_types::input::actions::ActionKind,
    yinz_types::input::actions::SearchDirection, yinz_types::input::actions::SearchOption,
};
use {
    yinz_types::input::command::OpenFilePayload, yinz_types::input::command::RunCommand,
    yinz_types::input::command::RunCommandAction,
};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::LayoutConstraint,
    yinz_types::input::layout::PluginAlias, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::Run, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::layout::SwapFloatingLayout, yinz_types::input::layout::SwapTiledLayout,
    yinz_types::input::layout::TabLayoutInfo, yinz_types::input::layout::TiledPaneLayout,
};
use {yinz_types::position::Column, yinz_types::position::Line, yinz_types::position::Position};

pub(crate) fn strings(tc: &hegel::TestCase) -> String {
    tc.draw(generators::text().max_size(8))
}

pub(crate) fn optional_strings(tc: &hegel::TestCase) -> Option<String> {
    tc.draw(generators::optional(generators::text().max_size(8)))
}

pub(crate) fn booleans(tc: &hegel::TestCase) -> bool {
    tc.draw(generators::booleans())
}

pub(crate) fn optional_booleans(tc: &hegel::TestCase) -> Option<bool> {
    tc.draw(generators::optional(generators::booleans()))
}

pub(crate) fn agent_events(tc: &hegel::TestCase) -> yinz_types::agent::AgentEvent {
    yinz_types::agent::AgentEvent {
        pane_id: yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
        kind: strings(tc),
        hook: strings(tc),
        session_id: optional_strings(tc),
        state: tc.draw(generators::optional(generators::sampled_from(vec![
            AgentState::Busy,
            AgentState::Waiting,
            AgentState::Idle,
            AgentState::Exited,
        ]))),
        detail: optional_strings(tc),
        prompt: optional_strings(tc),
        last_message: optional_strings(tc),
        tool_name: optional_strings(tc),
        cwd: optional_strings(tc),
        raw: strings(tc),
    }
}

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

pub(crate) fn context(tc: &hegel::TestCase) -> BTreeMap<String, String> {
    tc.draw(
        generators::vecs(generators::tuples!(
            generators::text().max_size(8),
            generators::text().max_size(8)
        ))
        .max_size(4),
    )
    .into_iter()
    .collect()
}

/// `Option<BTreeMap>` fields cross the client-server wire as a bare `map<string, string>`
/// (`CliPipeAction.args`, `KeybindPipeAction.configuration`), which has no way to say "absent",
/// so `Some` of an empty map comes back as `None`.
pub(crate) fn optional_non_empty_context(tc: &hegel::TestCase) -> Option<BTreeMap<String, String>> {
    booleans(tc).then(|| {
        tc.draw(
            generators::vecs(generators::tuples!(
                generators::text().max_size(8),
                generators::text().max_size(8)
            ))
            .min_size(1)
            .max_size(4),
        )
        .into_iter()
        .collect()
    })
}

pub(crate) fn bytes(tc: &hegel::TestCase) -> Vec<u8> {
    tc.draw(generators::binary().max_size(16))
}

pub(crate) fn paths(tc: &hegel::TestCase) -> PathBuf {
    PathBuf::from(strings(tc))
}

pub(crate) fn optional_paths(tc: &hegel::TestCase) -> Option<PathBuf> {
    booleans(tc).then(|| paths(tc))
}

/// `usize` fields cross both wires as `uint32` (`optional uint32 tab_id`, `uint32 tab_index`,
/// `optional uint32 line_number`, `optional uint32 logical_position`, ...), so nothing wider can
/// come back.
pub(crate) fn usize_as_u32(tc: &hegel::TestCase) -> usize {
    tc.draw(generators::integers::<u32>()) as usize
}

pub(crate) fn optional_usize_as_u32(tc: &hegel::TestCase) -> Option<usize> {
    booleans(tc).then(|| usize_as_u32(tc))
}

/// `Option<Vec>` fields cross both wires as `repeated` (`NewTabPayload.swap_tiled_layouts`,
/// `NewTabAction.initial_panes`, ...), which cannot tell `Some(vec![])` from `None`.
fn optional_non_empty<T>(
    tc: &hegel::TestCase,
    draw: impl Fn(&hegel::TestCase) -> T,
) -> Option<Vec<T>> {
    booleans(tc).then(|| {
        let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(2));
        (0..count).map(|_| draw(tc)).collect()
    })
}

fn up_to<T>(tc: &hegel::TestCase, max: usize, draw: impl Fn(&hegel::TestCase) -> T) -> Vec<T> {
    let count = tc.draw(generators::integers::<usize>().max_value(max));
    (0..count).map(|_| draw(tc)).collect()
}

pub(crate) fn pane_id(tc: &hegel::TestCase) -> PaneId {
    let n = tc.draw(generators::integers::<u32>());
    if booleans(tc) {
        PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n))
    } else {
        PaneId::Plugin(yinz_types::internal_ids::PluginId::new(n))
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

pub(crate) fn directions(tc: &hegel::TestCase) -> Direction {
    tc.draw(generators::sampled_from(vec![
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ]))
}

pub(crate) fn optional_directions(tc: &hegel::TestCase) -> Option<Direction> {
    booleans(tc).then(|| directions(tc))
}

pub(crate) fn input_modes(tc: &hegel::TestCase) -> InputMode {
    tc.draw(generators::sampled_from(
        InputMode::iter().collect::<Vec<_>>(),
    ))
}

pub(crate) fn resizes(tc: &hegel::TestCase) -> Resize {
    tc.draw(generators::sampled_from(vec![
        Resize::Increase,
        Resize::Decrease,
    ]))
}

pub(crate) fn search_directions(tc: &hegel::TestCase) -> SearchDirection {
    tc.draw(generators::sampled_from(vec![
        SearchDirection::Down,
        SearchDirection::Up,
    ]))
}

pub(crate) fn search_options(tc: &hegel::TestCase) -> SearchOption {
    tc.draw(generators::sampled_from(vec![
        SearchOption::CaseSensitivity,
        SearchOption::WholeWord,
        SearchOption::Wrap,
    ]))
}

pub(crate) fn pane_frame_styles(tc: &hegel::TestCase) -> PaneFrameStyle {
    tc.draw(generators::sampled_from(vec![
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ]))
}

pub(crate) fn unblock_conditions(tc: &hegel::TestCase) -> UnblockCondition {
    tc.draw(generators::sampled_from(vec![
        UnblockCondition::OnExitSuccess,
        UnblockCondition::OnExitFailure,
        UnblockCondition::OnAnyExit,
    ]))
}

pub(crate) fn warp_placements(tc: &hegel::TestCase) -> WarpPlacement {
    tc.draw(generators::sampled_from(vec![
        WarpPlacement::NewTab,
        WarpPlacement::FocusedTab,
        WarpPlacement::Floating,
    ]))
}

pub(crate) fn key_modifiers(tc: &hegel::TestCase) -> BTreeSet<KeyModifier> {
    tc.draw(
        generators::vecs(generators::sampled_from(vec![
            KeyModifier::Ctrl,
            KeyModifier::Alt,
            KeyModifier::Shift,
            KeyModifier::Super,
        ]))
        .max_size(4),
    )
    .into_iter()
    .collect()
}

pub(crate) fn bare_keys(tc: &hegel::TestCase) -> BareKey {
    match tc.draw(generators::integers::<u8>().max_value(22)) {
        0 => BareKey::PageDown,
        1 => BareKey::PageUp,
        2 => BareKey::Left,
        3 => BareKey::Down,
        4 => BareKey::Up,
        5 => BareKey::Right,
        6 => BareKey::Home,
        7 => BareKey::End,
        8 => BareKey::Backspace,
        9 => BareKey::Delete,
        10 => BareKey::Insert,
        // Both `BareKey` enums stop at `BARE_KEY_F12`; there is no value for F13 and up.
        11 => BareKey::F(tc.draw(generators::integers::<u8>().min_value(1).max_value(12))),
        12 => BareKey::Char(
            tc.draw(generators::text().min_size(1).max_size(1))
                .chars()
                .next()
                .expect("a one-character string"),
        ),
        13 => BareKey::Tab,
        14 => BareKey::Esc,
        15 => BareKey::Enter,
        16 => BareKey::CapsLock,
        17 => BareKey::ScrollLock,
        18 => BareKey::NumLock,
        19 => BareKey::PrintScreen,
        20 => BareKey::Pause,
        _ => BareKey::Menu,
    }
}

pub(crate) fn keys_with_modifier(tc: &hegel::TestCase) -> KeyWithModifier {
    KeyWithModifier {
        bare_key: bare_keys(tc),
        key_modifiers: key_modifiers(tc),
    }
}

/// The line crosses the client-server wire as `int32 line`; the column is `uint64` there and
/// `int64` on the plugin wire, so generate values representable by both.
pub(crate) fn positions(tc: &hegel::TestCase) -> Position {
    Position {
        line: Line::new(tc.draw(generators::integers::<i32>()) as isize),
        column: Column::new(tc.draw(generators::integers::<usize>()) & (i64::MAX as usize)),
    }
}

pub(crate) fn mouse_events(tc: &hegel::TestCase) -> MouseEvent {
    MouseEvent {
        event_type: tc.draw(generators::sampled_from(vec![
            MouseEventType::Press,
            MouseEventType::Release,
            MouseEventType::Motion,
        ])),
        left: booleans(tc),
        right: booleans(tc),
        middle: booleans(tc),
        wheel_up: booleans(tc),
        wheel_down: booleans(tc),
        wheel_left: booleans(tc),
        wheel_right: booleans(tc),
        shift: booleans(tc),
        alt: booleans(tc),
        ctrl: booleans(tc),
        position: positions(tc),
    }
}

/// A percentage crosses the client-server wire as `FloatingCoordinate { float percent }` and
/// is documented there as 1 to 100, which a `float` holds exactly; a fixed size is `uint32` on
/// both wires.
pub(crate) fn percents_or_fixed(tc: &hegel::TestCase) -> PercentOrFixed {
    if booleans(tc) {
        PercentOrFixed::Percent(tc.draw(generators::integers::<u8>().max_value(100)) as usize)
    } else {
        PercentOrFixed::Fixed(usize_as_u32(tc))
    }
}

pub(crate) fn optional_percents_or_fixed(tc: &hegel::TestCase) -> Option<PercentOrFixed> {
    booleans(tc).then(|| percents_or_fixed(tc))
}

pub(crate) fn split_sizes(tc: &hegel::TestCase) -> SplitSize {
    percents_or_fixed(tc).into()
}

pub(crate) fn floating_pane_coordinates(tc: &hegel::TestCase) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates {
        x: optional_percents_or_fixed(tc),
        y: optional_percents_or_fixed(tc),
        width: optional_percents_or_fixed(tc),
        height: optional_percents_or_fixed(tc),
        pinned: optional_booleans(tc),
        borderless: optional_booleans(tc),
    }
}

pub(crate) fn optional_floating_pane_coordinates(
    tc: &hegel::TestCase,
) -> Option<FloatingPaneCoordinates> {
    booleans(tc).then(|| floating_pane_coordinates(tc))
}

pub(crate) fn overlay_placements(tc: &hegel::TestCase) -> OverlayPlacement {
    OverlayPlacement {
        kind: tc.draw(generators::sampled_from(vec![
            OverlayKind::Dismissible,
            OverlayKind::Modal,
        ])),
        coordinates: optional_floating_pane_coordinates(tc),
        background_opacity: booleans(tc)
            .then(|| tc.draw(generators::integers::<u8>().max_value(100))),
    }
}

pub(crate) fn new_pane_placements(tc: &hegel::TestCase) -> NewPanePlacement {
    match tc.draw(generators::integers::<u8>().max_value(5)) {
        0 => NewPanePlacement::NoPreference {
            borderless: optional_booleans(tc),
        },
        1 => NewPanePlacement::Tiled {
            direction: optional_directions(tc),
            borderless: optional_booleans(tc),
        },
        // `NewPanePlacement.floating` is a bare `FloatingPaneCoordinates` message on the
        // client-server wire, so `Floating(Some(FloatingPaneCoordinates::default()))` is the same
        // bytes as `Floating(None)`.
        2 => NewPanePlacement::Floating(
            optional_floating_pane_coordinates(tc)
                .filter(|coordinates| *coordinates != FloatingPaneCoordinates::default()),
        ),
        3 => NewPanePlacement::InPlace {
            pane_id_to_replace: booleans(tc).then(|| pane_id(tc)),
            close_replaced_pane: booleans(tc),
            borderless: optional_booleans(tc),
        },
        4 => NewPanePlacement::Stacked {
            pane_id_to_stack_under: booleans(tc).then(|| pane_id(tc)),
            borderless: optional_booleans(tc),
        },
        _ => NewPanePlacement::Overlay(overlay_placements(tc)),
    }
}

pub(crate) fn originating_plugins(tc: &hegel::TestCase) -> OriginatingPlugin {
    OriginatingPlugin {
        generation: booleans(tc).then(|| tc.draw(generators::integers::<u64>())),
        plugin_id: yinz_types::internal_ids::PluginId::new(tc.draw(generators::integers::<u32>())),
        client_id: yinz_types::data::ClientId::new(tc.draw(generators::integers::<u32>())),
        context: context(tc),
    }
}

pub(crate) fn optional_originating_plugins(tc: &hegel::TestCase) -> Option<OriginatingPlugin> {
    booleans(tc).then(|| originating_plugins(tc))
}

pub(crate) fn run_command_actions(tc: &hegel::TestCase) -> RunCommandAction {
    RunCommandAction {
        command: paths(tc),
        args: up_to(tc, 3, strings),
        cwd: optional_paths(tc),
        direction: optional_directions(tc),
        hold_on_close: booleans(tc),
        hold_on_start: booleans(tc),
        originating_plugin: optional_originating_plugins(tc),
        use_terminal_title: booleans(tc),
        target: pane_targets(tc),
        remote_default_shell: booleans(tc),
    }
}

pub(crate) fn pane_targets(tc: &hegel::TestCase) -> PaneTarget {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => PaneTarget::Local,
        1 => PaneTarget::Ssh { host: strings(tc) },
        // Absolute, as the wire refuses anything else.
        _ => PaneTarget::Bubblewrap {
            root: std::path::Path::new("/").join(strings(tc).trim_start_matches('/')),
        },
    }
}

pub(crate) fn optional_run_command_actions(tc: &hegel::TestCase) -> Option<RunCommandAction> {
    booleans(tc).then(|| run_command_actions(tc))
}

pub(crate) fn run_commands(tc: &hegel::TestCase) -> RunCommand {
    RunCommand {
        agent_theme: None,
        command: paths(tc),
        args: up_to(tc, 3, strings),
        cwd: optional_paths(tc),
        hold_on_close: booleans(tc),
        hold_on_start: booleans(tc),
        originating_plugin: optional_originating_plugins(tc),
        use_terminal_title: booleans(tc),
        target: pane_targets(tc),
        remote_default_shell: booleans(tc),
    }
}

pub(crate) fn open_file_payloads(tc: &hegel::TestCase) -> OpenFilePayload {
    OpenFilePayload {
        path: paths(tc),
        line_number: optional_usize_as_u32(tc),
        cwd: optional_paths(tc),
        originating_plugin: optional_originating_plugins(tc),
    }
}

/// `CommandOrPluginFile.line_number` is an `optional int32` on both wires.
pub(crate) fn files_to_open(tc: &hegel::TestCase) -> FileToOpen {
    FileToOpen {
        path: paths(tc),
        line_number: booleans(tc)
            .then(|| tc.draw(generators::integers::<i32>().min_value(0)) as usize),
        cwd: optional_paths(tc),
    }
}

/// Built through `PluginUserConfiguration::new`, which strips the reserved keys (`cwd`,
/// `name`, ...) - a map containing one is not a configuration that can exist.
pub(crate) fn plugin_user_configurations(tc: &hegel::TestCase) -> PluginUserConfiguration {
    PluginUserConfiguration::new(context(tc))
}

/// The plugin wire carries some plugin locations as a URL string (`string plugin_url`,
/// `string start_or_reload_plugin_payload`) that comes back through `Url::parse`, which
/// percent-decodes the path, cuts it at `?` and `#`, collapses dot segments and normalises a
/// host. So paths and tags here use only characters a URL leaves alone, and a remote URL is one
/// `Url::parse` already considers normal. A segment is never empty: `//a` is a host to a URL.
fn url_safe_segments(tc: &hegel::TestCase) -> String {
    tc.draw(
        generators::vecs(generators::sampled_from(vec![
            'a', 'b', 'z', 'A', 'Z', '0', '9', '_', '-',
        ]))
        .min_size(1)
        .max_size(6),
    )
    .into_iter()
    .collect()
}

pub(crate) fn run_plugin_locations(tc: &hegel::TestCase) -> RunPluginLocation {
    if booleans(tc) {
        let segments = up_to(tc, 3, url_safe_segments).join("/");
        let root = if booleans(tc) { "/" } else { "" };
        RunPluginLocation::File(PathBuf::from(format!("{root}{segments}")))
    } else {
        RunPluginLocation::Yinz(PluginTag::new(url_safe_segments(tc)))
    }
}

/// `relative_path` is never drawn: `RunPlugin`'s `PartialEq` ignores it, so no property here
/// could see it change. The example tests assert it on its own.
pub(crate) fn run_plugins(tc: &hegel::TestCase) -> RunPlugin {
    RunPlugin {
        _allow_exec_host_cmd: booleans(tc),
        location: run_plugin_locations(tc),
        configuration: plugin_user_configurations(tc),
        initial_cwd: optional_paths(tc),
        relative_path: None,
        declared_path: None,
    }
}

/// An alias is whatever `yinz_plugin_location::run_plugin_or_alias_from_url` could not parse as a plugin URL, and the
/// plugin wire sends one back through that same parse as `string plugin_url` - so a name that
/// reads as a `yinz:` or `file:` URL would come back as that plugin instead, and an `http(s):`
/// one would be refused. The prefix keeps the scheme, if the text forms one, from being any of those.
pub(crate) fn plugin_aliases(tc: &hegel::TestCase) -> PluginAlias {
    PluginAlias {
        name: format!("alias-{}", strings(tc)),
        configuration: booleans(tc).then(|| plugin_user_configurations(tc)),
        initial_cwd: optional_paths(tc),
        run_plugin: booleans(tc).then(|| run_plugins(tc)),
    }
}

pub(crate) fn run_plugins_or_aliases(tc: &hegel::TestCase) -> RunPluginOrAlias {
    if booleans(tc) {
        RunPluginOrAlias::RunPlugin(run_plugins(tc))
    } else {
        RunPluginOrAlias::Alias(plugin_aliases(tc))
    }
}

pub(crate) fn runs(tc: &hegel::TestCase) -> Run {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => Run::Plugin(run_plugins_or_aliases(tc)),
        1 => Run::Command(run_commands(tc)),
        2 => Run::EditFile(paths(tc), optional_usize_as_u32(tc), optional_paths(tc)),
        _ => Run::Cwd(paths(tc)),
    }
}

pub(crate) fn optional_runs(tc: &hegel::TestCase) -> Option<Run> {
    booleans(tc).then(|| runs(tc))
}

pub(crate) fn layout_constraints(tc: &hegel::TestCase) -> LayoutConstraint {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => LayoutConstraint::MaxPanes(usize_as_u32(tc)),
        1 => LayoutConstraint::MinPanes(usize_as_u32(tc)),
        2 => LayoutConstraint::ExactPanes(usize_as_u32(tc)),
        _ => LayoutConstraint::NoConstraint,
    }
}

/// `run_instructions_to_ignore` is never drawn: it is server-internal and, as both proto files
/// say, not represented on either wire.
pub(crate) fn tiled_pane_layouts(tc: &hegel::TestCase) -> TiledPaneLayout {
    tiled_pane_layouts_of_depth(tc, 0)
}

fn tiled_pane_layouts_of_depth(tc: &hegel::TestCase, depth: usize) -> TiledPaneLayout {
    TiledPaneLayout {
        pane_tabs: None,
        children_split_direction: tc.draw(generators::sampled_from(vec![
            SplitDirection::Horizontal,
            SplitDirection::Vertical,
        ])),
        name: optional_strings(tc),
        children: if depth < 2 {
            up_to(tc, 2, |tc| tiled_pane_layouts_of_depth(tc, depth + 1))
        } else {
            vec![]
        },
        split_size: booleans(tc).then(|| split_sizes(tc)),
        run: optional_runs(tc),
        borderless: optional_booleans(tc),
        focus: optional_booleans(tc),
        external_children_index: optional_usize_as_u32(tc),
        children_are_stacked: booleans(tc),
        is_expanded_in_stack: booleans(tc),
        exclude_from_sync: optional_booleans(tc),
        run_instructions_to_ignore: vec![],
        hide_floating_panes: booleans(tc),
        user_named: booleans(tc),
        tab_group: booleans(tc).then(|| yinz_types::tab_bar::TabGroupInfo {
            id: u32s(tc),
            name: strings(tc),
        }),
        group_focused: booleans(tc),
        pane_initial_contents: optional_strings(tc),
        default_fg: optional_strings(tc),
        default_bg: optional_strings(tc),
    }
}

pub(crate) fn floating_pane_layouts(tc: &hegel::TestCase) -> FloatingPaneLayout {
    FloatingPaneLayout {
        pane_tabs: None,
        name: optional_strings(tc),
        height: optional_percents_or_fixed(tc),
        width: optional_percents_or_fixed(tc),
        x: optional_percents_or_fixed(tc),
        y: optional_percents_or_fixed(tc),
        pinned: optional_booleans(tc),
        borderless: optional_booleans(tc),
        run: optional_runs(tc),
        focus: optional_booleans(tc),
        already_running: booleans(tc),
        pane_initial_contents: optional_strings(tc),
        logical_position: optional_usize_as_u32(tc),
        default_fg: optional_strings(tc),
        default_bg: optional_strings(tc),
    }
}

pub(crate) fn swap_tiled_layouts(tc: &hegel::TestCase) -> SwapTiledLayout {
    (
        up_to(tc, 2, |tc| (layout_constraints(tc), tiled_pane_layouts(tc)))
            .into_iter()
            .collect(),
        optional_strings(tc),
    )
}

pub(crate) fn swap_floating_layouts(tc: &hegel::TestCase) -> SwapFloatingLayout {
    (
        up_to(tc, 2, |tc| {
            (layout_constraints(tc), up_to(tc, 2, floating_pane_layouts))
        })
        .into_iter()
        .collect(),
        optional_strings(tc),
    )
}

/// `tab_name` is never drawn empty: both decoders read `Some("")` as `None`, on the grounds
/// that a tab with an empty name has no name.
pub(crate) fn tab_layout_infos(tc: &hegel::TestCase) -> TabLayoutInfo {
    TabLayoutInfo {
        tab_index: TabPosition::new(usize_as_u32(tc)),
        tab_name: booleans(tc).then(|| tc.draw(generators::text().min_size(1).max_size(8))),
        tiled_layout: tiled_pane_layouts(tc),
        floating_layouts: up_to(tc, 2, floating_pane_layouts),
        swap_tiled_layouts: optional_non_empty(tc, swap_tiled_layouts),
        swap_floating_layouts: optional_non_empty(tc, swap_floating_layouts),
    }
}

pub(crate) fn commands_or_plugins(tc: &hegel::TestCase) -> CommandOrPlugin {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => CommandOrPlugin::Command(run_command_actions(tc)),
        1 => CommandOrPlugin::Plugin(run_plugins_or_aliases(tc)),
        _ => CommandOrPlugin::File(files_to_open(tc)),
    }
}

pub(crate) fn layout_infos(tc: &hegel::TestCase) -> LayoutInfo {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => LayoutInfo::BuiltIn(strings(tc)),
        1 => LayoutInfo::File(
            strings(tc),
            LayoutMetadata {
                tabs: up_to(tc, 2, |tc| TabMetadata {
                    panes: up_to(tc, 2, |tc| PaneMetadata {
                        name: optional_strings(tc),
                        is_plugin: booleans(tc),
                        is_builtin_plugin: booleans(tc),
                    }),
                    name: optional_strings(tc),
                }),
                creation_time: strings(tc),
                update_time: strings(tc),
            },
        ),
        _ => LayoutInfo::Stringified(strings(tc)),
    }
}

pub(crate) fn warped_panes(tc: &hegel::TestCase) -> WarpedPane {
    WarpedPane {
        agent_theme: None,
        bracketed_paste: false,
        agent_input: None,
        held: None,
        group: None,
        host_id: SupervisorId::new(strings(tc)),
        control_socket: paths(tc),
        title: optional_strings(tc),
        caption: optional_strings(tc),
        contents: optional_strings(tc),
        borderless: booleans(tc),
        remote: booleans(tc).then(|| RemotePaneHandoff {
            host: strings(tc),
            pane_token: strings(tc),
            received: tc.draw(generators::integers::<u64>()),
            cmd: run_commands(tc),
        }),
        agent: optional_agent_infos(tc),
    }
}

pub(crate) fn warped_tabs(tc: &hegel::TestCase) -> WarpedTab {
    WarpedTab {
        relationships: None,
        name: optional_strings(tc),
        user_named: booleans(tc),
        tiled_layout: tiled_pane_layouts(tc),
        floating_layout: up_to(tc, 2, floating_pane_layouts),
        tiled_panes: up_to(tc, 2, warped_panes),
        floating_panes: up_to(tc, 2, warped_panes),
        hide_floating_panes: booleans(tc),
    }
}

/// Any action but `SkipConfirm`, so that a `SkipConfirm` wraps something that does not wrap in
/// turn. `action_of_kind` is the exhaustive one.
pub(crate) fn any_action(tc: &hegel::TestCase) -> Action {
    let kind = tc.draw(generators::sampled_from(
        ActionKind::iter()
            .filter(|kind| *kind != ActionKind::SkipConfirm)
            .collect::<Vec<_>>(),
    ));
    action_of_kind(tc, kind)
}

/// An `Action` of the given kind with a freshly drawn payload.
///
/// There is deliberately no wildcard arm: a new `Action` variant does not compile until it
/// has a generator here, which is what makes the round-trip properties exhaustive.
pub fn action_of_kind(tc: &hegel::TestCase, kind: ActionKind) -> Action {
    match kind {
        ActionKind::Quit => Action::Quit,
        ActionKind::Write => Action::Write {
            key_with_modifier: booleans(tc).then(|| keys_with_modifier(tc)),
            bytes: bytes(tc),
            is_kitty_keyboard_protocol: booleans(tc),
        },
        ActionKind::WriteChars => Action::WriteChars { chars: strings(tc) },
        ActionKind::WriteToPaneId => Action::WriteToPaneId {
            bytes: bytes(tc),
            pane_id: pane_id(tc),
        },
        ActionKind::WriteCharsToPaneId => Action::WriteCharsToPaneId {
            chars: strings(tc),
            pane_id: pane_id(tc),
        },
        ActionKind::Paste => Action::Paste {
            chars: strings(tc),
            pane_id: booleans(tc).then(|| pane_id(tc)),
        },
        ActionKind::SwitchToMode => Action::SwitchToMode {
            input_mode: input_modes(tc),
        },
        ActionKind::SwitchModeForAllClients => Action::SwitchModeForAllClients {
            input_mode: input_modes(tc),
        },
        ActionKind::Resize => Action::Resize {
            resize: resizes(tc),
            direction: optional_directions(tc),
        },
        ActionKind::FocusNextPane => Action::FocusNextPane,
        ActionKind::FocusPreviousPane => Action::FocusPreviousPane,
        ActionKind::FocusPaneTabs => Action::FocusPaneTabs,
        ActionKind::PreviousPaneTab => Action::PreviousPaneTab,
        ActionKind::NextPaneTab => Action::NextPaneTab,
        ActionKind::FocusLastPane => Action::FocusLastPane,
        ActionKind::SwitchFocus => Action::SwitchFocus,
        ActionKind::MoveFocus => Action::MoveFocus {
            direction: directions(tc),
        },
        ActionKind::MoveFocusOrTab => Action::MoveFocusOrTab {
            direction: directions(tc),
        },
        ActionKind::MovePane => Action::MovePane {
            direction: optional_directions(tc),
        },
        ActionKind::MovePaneBackwards => Action::MovePaneBackwards,
        ActionKind::ClearScreen => Action::ClearScreen,
        ActionKind::DumpScreen => Action::DumpScreen {
            file_path: optional_strings(tc),
            include_scrollback: booleans(tc),
            pane_id: booleans(tc).then(|| pane_id(tc)),
            ansi: booleans(tc),
        },
        ActionKind::DumpLayout => Action::DumpLayout,
        ActionKind::SaveSession => Action::SaveSession,
        ActionKind::EditScrollback => Action::EditScrollback { ansi: booleans(tc) },
        ActionKind::ScrollUp => Action::ScrollUp,
        ActionKind::ScrollUpAt => Action::ScrollUpAt {
            position: positions(tc),
        },
        ActionKind::ScrollDown => Action::ScrollDown,
        ActionKind::ScrollDownAt => Action::ScrollDownAt {
            position: positions(tc),
        },
        ActionKind::ScrollToBottom => Action::ScrollToBottom,
        ActionKind::ScrollToTop => Action::ScrollToTop,
        ActionKind::ScrollToPreviousPrompt => Action::ScrollToPreviousPrompt,
        ActionKind::ScrollToNextPrompt => Action::ScrollToNextPrompt,
        ActionKind::SelectCommandAtScrollPosition => Action::SelectCommandAtScrollPosition,
        ActionKind::CopyLastCommandOutput => Action::CopyLastCommandOutput,
        ActionKind::MoveTextSelection => Action::MoveTextSelection {
            direction: directions(tc),
        },
        ActionKind::SwapTextSelectionEnds => Action::SwapTextSelectionEnds,
        ActionKind::MoveNotificationSelection => Action::MoveNotificationSelection {
            direction: directions(tc),
        },
        ActionKind::ActivateNotification => Action::ActivateNotification,
        ActionKind::ActivateLastNotification => Action::ActivateLastNotification,
        ActionKind::ToggleNotificationDetail => Action::ToggleNotificationDetail,
        ActionKind::PageScrollUp => Action::PageScrollUp,
        ActionKind::PageScrollDown => Action::PageScrollDown,
        ActionKind::HalfPageScrollUp => Action::HalfPageScrollUp,
        ActionKind::HalfPageScrollDown => Action::HalfPageScrollDown,
        ActionKind::ToggleFocusFullscreen => Action::ToggleFocusFullscreen,
        ActionKind::ToggleFocusNoUiFullscreen => Action::ToggleFocusNoUiFullscreen,
        ActionKind::TogglePaneFrames => Action::TogglePaneFrames,
        ActionKind::ToggleCommandPalette => Action::ToggleCommandPalette,
        ActionKind::SetPaneFrameStyle => Action::SetPaneFrameStyle(pane_frame_styles(tc)),
        ActionKind::ToggleActiveSyncTab => Action::ToggleActiveSyncTab,
        ActionKind::NewPane => Action::NewPane {
            direction: optional_directions(tc),
            pane_name: optional_strings(tc),
            start_suppressed: booleans(tc),
        },
        ActionKind::NewBlockingPane => Action::NewBlockingPane {
            placement: new_pane_placements(tc),
            pane_name: optional_strings(tc),
            command: optional_run_command_actions(tc),
            unblock_condition: booleans(tc).then(|| unblock_conditions(tc)),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::EditFile => Action::EditFile {
            payload: open_file_payloads(tc),
            direction: optional_directions(tc),
            floating: booleans(tc),
            in_place: booleans(tc),
            close_replaced_pane: booleans(tc),
            start_suppressed: booleans(tc),
            placement: new_pane_placements(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewFloatingPane => Action::NewFloatingPane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            placement: new_pane_placements(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewTiledPane => Action::NewTiledPane {
            direction: optional_directions(tc),
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            borderless: optional_booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewInPlacePane => Action::NewInPlacePane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            pane_id_to_replace: booleans(tc).then(|| pane_id(tc)),
            close_replaced_pane: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewStackedPane => Action::NewStackedPane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::TogglePaneEmbedOrFloating => Action::TogglePaneEmbedOrFloating,
        ActionKind::ToggleFloatingPanes => Action::ToggleFloatingPanes,
        ActionKind::ShowFloatingPanes => Action::ShowFloatingPanes {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::HideFloatingPanes => Action::HideFloatingPanes {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::AreFloatingPanesVisible => Action::AreFloatingPanesVisible {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::CloseFocus => Action::CloseFocus,
        ActionKind::PaneNameInput => Action::PaneNameInput { input: bytes(tc) },
        ActionKind::UndoRenamePane => Action::UndoRenamePane,
        ActionKind::NewTab => Action::NewTab {
            layout_source: optional_paths(tc),
            tiled_layout: booleans(tc).then(|| tiled_pane_layouts(tc)),
            floating_layouts: up_to(tc, 2, floating_pane_layouts),
            swap_tiled_layouts: optional_non_empty(tc, swap_tiled_layouts),
            swap_floating_layouts: optional_non_empty(tc, swap_floating_layouts),
            tab_name: optional_strings(tc),
            should_change_focus_to_new_tab: booleans(tc),
            cwd: optional_paths(tc),
            initial_panes: optional_non_empty(tc, commands_or_plugins),
            first_pane_unblock_condition: booleans(tc).then(|| unblock_conditions(tc)),
        },
        ActionKind::NoOp => Action::NoOp,
        ActionKind::GoToNextTab => Action::GoToNextTab,
        ActionKind::GoToPreviousTab => Action::GoToPreviousTab,
        ActionKind::CloseTab => Action::CloseTab,
        ActionKind::NewTabGroup => Action::NewTabGroup,
        ActionKind::GoToNextTabGroup => Action::GoToNextTabGroup,
        ActionKind::MovePaneToNextTabGroup => Action::MovePaneToNextTabGroup,
        ActionKind::MovePaneToPreviousTabGroup => Action::MovePaneToPreviousTabGroup,
        ActionKind::GoToPreviousTabGroup => Action::GoToPreviousTabGroup,
        ActionKind::GoToTabInGroup => Action::GoToTabInGroup {
            index: TabNumber::new(u32s(tc)),
        },
        ActionKind::SwitchTabGroup => Action::SwitchTabGroup { id: u32s(tc) },
        ActionKind::RenameTabGroup => Action::RenameTabGroup {
            id: u32s(tc),
            name: strings(tc),
        },
        ActionKind::CloseTabGroup => Action::CloseTabGroup { id: u32s(tc) },
        ActionKind::GoToTab => Action::GoToTab {
            index: TabNumber::new(tc.draw(generators::integers::<u32>())),
        },
        ActionKind::GoToTabName => Action::GoToTabName {
            name: strings(tc),
            create: booleans(tc),
        },
        ActionKind::ToggleTab => Action::ToggleTab,
        ActionKind::TabNameInput => Action::TabNameInput { input: bytes(tc) },
        ActionKind::UndoRenameTab => Action::UndoRenameTab,
        ActionKind::MoveTab => Action::MoveTab {
            direction: directions(tc),
        },
        ActionKind::Run => Action::Run {
            command: run_command_actions(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
        },
        ActionKind::SetPaneAgent => Action::SetPaneAgent {
            pane_id: pane_id(tc),
            agent: optional_agent_infos(tc),
        },
        ActionKind::AgentEvent => Action::AgentEvent(agent_events(tc)),
        ActionKind::Reconfigure => Action::Reconfigure {
            config: strings(tc),
            write_to_disk: booleans(tc),
        },
        ActionKind::SetPaneColor => Action::SetPaneColor {
            pane_id: pane_id(tc),
            fg: optional_strings(tc),
            bg: optional_strings(tc),
        },
        ActionKind::Detach => Action::Detach,
        ActionKind::SetDarkTheme => Action::SetDarkTheme,
        ActionKind::SetLightTheme => Action::SetLightTheme,
        ActionKind::ToggleTheme => Action::ToggleTheme,
        ActionKind::SwitchSession => Action::SwitchSession {
            name: strings(tc).into(),
            tab_position: optional_usize_as_u32(tc)
                .map(|p| u32::try_from(p).map(TabNumber::new))
                .transpose()
                .unwrap(),
            pane_id: booleans(tc)
                .then(|| PaneId::from_legacy(tc.draw(generators::integers::<u32>()), booleans(tc))),
            layout: booleans(tc).then(|| layout_infos(tc)),
            cwd: optional_paths(tc),
        },
        ActionKind::WarpPane => Action::WarpPane {
            pane_id: booleans(tc).then(|| pane_id(tc)),
            to_session: strings(tc).into(),
            placement: warp_placements(tc),
        },
        ActionKind::AdoptPane => Action::AdoptPane {
            payload: Box::new(warped_panes(tc)),
            placement: warp_placements(tc),
        },
        ActionKind::WarpTab => Action::WarpTab {
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
            to_session: strings(tc).into(),
        },
        ActionKind::AdoptTab => Action::AdoptTab {
            payload: Box::new(warped_tabs(tc)),
        },
        ActionKind::WarpTabFrom => Action::WarpTabFrom {
            from_session: strings(tc).into(),
            tab_id: TabId::new(usize_as_u32(tc)),
        },
        ActionKind::LaunchOrFocusPlugin => Action::LaunchOrFocusPlugin {
            plugin: run_plugins_or_aliases(tc),
            should_float: booleans(tc),
            move_to_focused_tab: booleans(tc),
            should_open_in_place: booleans(tc),
            close_replaced_pane: booleans(tc),
            skip_cache: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::LaunchPlugin => Action::LaunchPlugin {
            plugin: run_plugins_or_aliases(tc),
            should_float: booleans(tc),
            should_open_in_place: booleans(tc),
            close_replaced_pane: booleans(tc),
            skip_cache: booleans(tc),
            cwd: optional_paths(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::MouseEvent => Action::MouseEvent {
            event: mouse_events(tc),
        },
        ActionKind::Copy => Action::Copy,
        ActionKind::Confirm => Action::Confirm,
        ActionKind::Deny => Action::Deny,
        ActionKind::SkipConfirm => Action::SkipConfirm {
            action: Box::new(any_action(tc)),
        },
        ActionKind::SearchInput => Action::SearchInput { input: bytes(tc) },
        ActionKind::Search => Action::Search {
            direction: search_directions(tc),
        },
        ActionKind::SearchToggleOption => Action::SearchToggleOption {
            option: search_options(tc),
        },
        ActionKind::ToggleMouseMode => Action::ToggleMouseMode,
        ActionKind::PreviousSwapLayout => Action::PreviousSwapLayout,
        ActionKind::NextSwapLayout => Action::NextSwapLayout,
        ActionKind::OverrideLayout => Action::OverrideLayout {
            tabs: up_to(tc, 2, tab_layout_infos),
            retain_existing_terminal_panes: booleans(tc),
            retain_existing_plugin_panes: booleans(tc),
            apply_only_to_active_tab: booleans(tc),
        },
        ActionKind::QueryTabNames => Action::QueryTabNames,
        ActionKind::NewTiledPluginPane => Action::NewTiledPluginPane {
            plugin: run_plugins_or_aliases(tc),
            pane_name: optional_strings(tc),
            skip_cache: booleans(tc),
            cwd: optional_paths(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewFloatingPluginPane => Action::NewFloatingPluginPane {
            plugin: run_plugins_or_aliases(tc),
            pane_name: optional_strings(tc),
            skip_cache: booleans(tc),
            cwd: optional_paths(tc),
            placement: new_pane_placements(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::NewInPlacePluginPane => Action::NewInPlacePluginPane {
            plugin: run_plugins_or_aliases(tc),
            pane_name: optional_strings(tc),
            skip_cache: booleans(tc),
            close_replaced_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc).map(TabId::new),
        },
        ActionKind::StartOrReloadPlugin => Action::StartOrReloadPlugin {
            plugin: run_plugins_or_aliases(tc),
        },
        ActionKind::CloseTerminalPane => Action::CloseTerminalPane {
            pane_id: yinz_types::internal_ids::TerminalId::new(
                tc.draw(generators::integers::<u32>()),
            ),
        },
        ActionKind::ClosePluginPane => Action::ClosePluginPane {
            pane_id: yinz_types::internal_ids::PluginId::new(
                tc.draw(generators::integers::<u32>()),
            ),
        },
        ActionKind::FocusTerminalPaneWithId => Action::FocusTerminalPaneWithId {
            pane_id: yinz_types::internal_ids::TerminalId::new(
                tc.draw(generators::integers::<u32>()),
            ),
            should_float_if_hidden: booleans(tc),
            should_be_in_place_if_hidden: booleans(tc),
        },
        ActionKind::FocusPluginPaneWithId => Action::FocusPluginPaneWithId {
            pane_id: yinz_types::internal_ids::PluginId::new(
                tc.draw(generators::integers::<u32>()),
            ),
            should_float_if_hidden: booleans(tc),
            should_be_in_place_if_hidden: booleans(tc),
        },
        ActionKind::RenameTerminalPane => Action::RenameTerminalPane {
            pane_id: yinz_types::internal_ids::TerminalId::new(
                tc.draw(generators::integers::<u32>()),
            ),
            name: bytes(tc),
        },
        ActionKind::RenamePluginPane => Action::RenamePluginPane {
            pane_id: yinz_types::internal_ids::PluginId::new(
                tc.draw(generators::integers::<u32>()),
            ),
            name: bytes(tc),
        },
        ActionKind::RenameTab => Action::RenameTab {
            tab_index: TabNumber::new(tc.draw(generators::integers::<u32>())),
            name: bytes(tc),
        },
        ActionKind::GoToTabById => Action::GoToTabById {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::CloseTabById => Action::CloseTabById {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::RenameTabById => Action::RenameTabById {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
            name: strings(tc),
        },
        ActionKind::ArrangePanesByProject => Action::ArrangePanesByProject,
        ActionKind::BreakPane => Action::BreakPane,
        ActionKind::BreakPaneRight => Action::BreakPaneRight,
        ActionKind::BreakPaneLeft => Action::BreakPaneLeft,
        ActionKind::FocusHostSession => Action::FocusHostSession,
        ActionKind::FocusGuestSession => Action::FocusGuestSession,
        ActionKind::ToggleHostFullscreen => Action::ToggleHostFullscreen,
        ActionKind::RenameSession => Action::RenameSession {
            name: strings(tc).into(),
        },
        ActionKind::Notify => Action::Notify(strings(tc)),
        ActionKind::CliPipe => Action::CliPipe {
            focus_plugin: false,
            pipe_id: strings(tc),
            name: optional_strings(tc),
            payload: optional_strings(tc),
            args: optional_non_empty_context(tc),
            plugin: optional_strings(tc),
            configuration: optional_non_empty_context(tc),
            launch_new: booleans(tc),
            skip_cache: booleans(tc),
            floating: optional_booleans(tc),
            in_place: optional_booleans(tc),
            cwd: optional_paths(tc),
            pane_title: optional_strings(tc),
        },
        ActionKind::KeybindPipe => Action::KeybindPipe {
            name: optional_strings(tc),
            payload: optional_strings(tc),
            args: optional_non_empty_context(tc),
            plugin: optional_strings(tc),
            plugin_id: tc
                .draw(generators::optional(generators::integers::<u32>()))
                .map(yinz_types::internal_ids::PluginId::new),
            configuration: optional_non_empty_context(tc),
            launch_new: booleans(tc),
            skip_cache: booleans(tc),
            floating: optional_booleans(tc),
            in_place: optional_booleans(tc),
            cwd: optional_paths(tc),
            pane_title: optional_strings(tc),
        },
        ActionKind::ListClients => Action::ListClients,
        ActionKind::ListPanes => Action::ListPanes {
            show_tab: booleans(tc),
            show_command: booleans(tc),
            show_state: booleans(tc),
            show_geometry: booleans(tc),
            show_all: booleans(tc),
            output_json: booleans(tc),
        },
        ActionKind::ListTabs => Action::ListTabs {
            show_state: booleans(tc),
            show_dimensions: booleans(tc),
            show_panes: booleans(tc),
            show_layout: booleans(tc),
            show_all: booleans(tc),
            output_json: booleans(tc),
        },
        ActionKind::CurrentTabInfo => Action::CurrentTabInfo {
            output_json: booleans(tc),
        },
        ActionKind::TogglePanePinned => Action::TogglePanePinned,
        ActionKind::StackPanes => Action::StackPanes {
            pane_ids: up_to(tc, 3, pane_id),
        },
        ActionKind::ChangeFloatingPaneCoordinates => Action::ChangeFloatingPaneCoordinates {
            pane_id: pane_id(tc),
            coordinates: floating_pane_coordinates(tc),
        },
        ActionKind::TogglePaneBorderless => Action::TogglePaneBorderless {
            pane_id: pane_id(tc),
        },
        ActionKind::SetPaneBorderless => Action::SetPaneBorderless {
            pane_id: pane_id(tc),
            borderless: booleans(tc),
        },
        ActionKind::TogglePaneInGroup => Action::TogglePaneInGroup,
        ActionKind::ToggleGroupMarking => Action::ToggleGroupMarking,
        ActionKind::ScrollUpByPaneId => Action::ScrollUpByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ScrollDownByPaneId => Action::ScrollDownByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ScrollToTopByPaneId => Action::ScrollToTopByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ScrollToBottomByPaneId => Action::ScrollToBottomByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::PageScrollUpByPaneId => Action::PageScrollUpByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::PageScrollDownByPaneId => Action::PageScrollDownByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::HalfPageScrollUpByPaneId => Action::HalfPageScrollUpByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::HalfPageScrollDownByPaneId => Action::HalfPageScrollDownByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ResizeByPaneId => Action::ResizeByPaneId {
            pane_id: pane_id(tc),
            resize: resizes(tc),
            direction: optional_directions(tc),
        },
        ActionKind::MovePaneByPaneId => Action::MovePaneByPaneId {
            pane_id: pane_id(tc),
            direction: optional_directions(tc),
        },
        ActionKind::MovePaneBackwardsByPaneId => Action::MovePaneBackwardsByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ClearScreenByPaneId => Action::ClearScreenByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::EditScrollbackByPaneId => Action::EditScrollbackByPaneId {
            pane_id: pane_id(tc),
            ansi: booleans(tc),
        },
        ActionKind::ToggleFocusFullscreenByPaneId => Action::ToggleFocusFullscreenByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::ToggleFocusNoUiFullscreenByPaneId => {
            Action::ToggleFocusNoUiFullscreenByPaneId {
                pane_id: pane_id(tc),
            }
        },
        ActionKind::TogglePaneEmbedOrFloatingByPaneId => {
            Action::TogglePaneEmbedOrFloatingByPaneId {
                pane_id: pane_id(tc),
            }
        },
        ActionKind::CloseFocusByPaneId => Action::CloseFocusByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::RenamePaneByPaneId => Action::RenamePaneByPaneId {
            pane_id: booleans(tc).then(|| pane_id(tc)),
            name: bytes(tc),
        },
        ActionKind::UndoRenamePaneByPaneId => Action::UndoRenamePaneByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::TogglePanePinnedByPaneId => Action::TogglePanePinnedByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::FocusPaneByPaneId => Action::FocusPaneByPaneId {
            pane_id: pane_id(tc),
        },
        ActionKind::UndoRenameTabByTabId => Action::UndoRenameTabByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::ToggleActiveSyncTabByTabId => Action::ToggleActiveSyncTabByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::ToggleFloatingPanesByTabId => Action::ToggleFloatingPanesByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::PreviousSwapLayoutByTabId => Action::PreviousSwapLayoutByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::NextSwapLayoutByTabId => Action::NextSwapLayoutByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
        },
        ActionKind::MoveTabByTabId => Action::MoveTabByTabId {
            id: TabId::try_from(tc.draw(generators::integers::<u64>())).unwrap(),
            direction: directions(tc),
        },
    }
}

// ------------------------------------------------------------ PluginCommand leaves

pub(crate) fn u32s(tc: &hegel::TestCase) -> u32 {
    tc.draw(generators::integers::<u32>())
}

pub(crate) fn agent_infos(tc: &hegel::TestCase) -> AgentInfo {
    AgentInfo {
        kind: strings(tc),
        state: tc.draw(generators::sampled_from(vec![
            AgentState::Busy,
            AgentState::Waiting,
            AgentState::Idle,
            AgentState::Exited,
        ])),
        source: tc.draw(generators::sampled_from(vec![
            AgentSource::Spawned,
            AgentSource::Registered,
            AgentSource::Reported,
            AgentSource::Detected,
        ])),
        detail: optional_strings(tc),
        waiting_on: optional_strings(tc),
        wait_needs_a_person: booleans(tc),
        cwd: optional_strings(tc),
        session_id: optional_strings(tc),
        run_id: optional_strings(tc),
    }
}

fn optional_agent_infos(tc: &hegel::TestCase) -> Option<AgentInfo> {
    booleans(tc).then(|| agent_infos(tc))
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

fn pane_ids(tc: &hegel::TestCase) -> Vec<PaneId> {
    up_to(tc, 3, pane_id)
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
        CommandType::GetLocalUtcOffset => PluginCommand::GetLocalUtcOffset {
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
        CommandType::ProjectConfigRead => PluginCommand::ProjectConfigRead {
            directory: strings(tc).into(),
        },
        CommandType::AgentControl => PluginCommand::AgentControl {
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
        CommandType::ArrangeProjectPanes => {
            PluginCommand::ArrangeProjectPanes(vec![yinz_types::data::ProjectPaneGroup {
                identity: strings(tc),
                name: strings(tc),
                configuration_root: Some(strings(tc)),
                pane_ids: vec![u32s(tc)],
            }])
        },
        CommandType::SetProjectContext => {
            PluginCommand::SetProjectContext(vec![strings(tc)], vec![(pane_id(tc), strings(tc))])
        },
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
        CommandType::SetRelatedPanes => PluginCommand::SetRelatedPanes(pane_ids(tc)),
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
        CommandType::EnqueueQueueJob => PluginCommand::EnqueueQueueJob {
            queue: strings(tc),
            plugin_url: strings(tc),
            config: context(tc),
            context: context(tc),
        },
        // `ConfigureQueuePayload.concurrency` is a `uint32`.
        CommandType::ConfigureQueue => PluginCommand::ConfigureQueue {
            queue: strings(tc),
            concurrency: usize_as_u32(tc),
        },
        CommandType::CancelQueueJob => PluginCommand::CancelQueueJob {
            job_id: QueueJobId::new(u64s(tc)),
        },
        CommandType::QueueStatus => PluginCommand::QueueStatus {
            queue: optional_strings(tc),
        },
        CommandType::FinishQueueJob => PluginCommand::FinishQueueJob {
            success: booleans(tc),
            payload: optional_strings(tc),
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
