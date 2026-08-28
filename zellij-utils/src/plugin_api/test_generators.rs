//! Generators for the values the protobuf round-trip properties draw, written once so that
//! `Action`, `Event` and `PluginCommand` describe each leaf type in one place.
//!
//! Every generator is a plain `fn(&hegel::TestCase) -> T` that draws in sequence, so they compose
//! by calling each other. Where one is narrower than the type it builds, a comment quotes the
//! wire field that makes the rest of the domain unrepresentable - the narrowing is part of the
//! specification, not a way past a failing case.

use crate::data::{
    BareKey, CommandOrPlugin, Direction, FileMetadata, FileToOpen, FloatingPaneCoordinates,
    InputMode, KeyModifier, KeyWithModifier, LayoutInfo, LayoutMetadata, NewPanePlacement,
    OriginatingPlugin, OverlayKind, OverlayPlacement, PaneId, PaneMetadata, PluginTag,
    RemotePaneHandoff, Resize, TabMetadata, UnblockCondition, WarpPlacement, WarpedPane, WarpedTab,
};
use crate::input::actions::{Action, ActionKind, SearchDirection, SearchOption};
use crate::input::command::{OpenFilePayload, RunCommand, RunCommandAction};
use crate::input::layout::{
    FloatingPaneLayout, LayoutConstraint, PercentOrFixed, PluginAlias, PluginUserConfiguration,
    Run, RunPlugin, RunPluginLocation, RunPluginOrAlias, SplitDirection, SplitSize,
    SwapFloatingLayout, SwapTiledLayout, TabLayoutInfo, TiledPaneLayout,
};
use crate::input::mouse::{MouseEvent, MouseEventType};
use crate::input::options::PaneFrameStyle;
use crate::position::{Column, Line, Position};
use hegel::generators;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use strum::IntoEnumIterator;

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
        PaneId::Terminal(n)
    } else {
        PaneId::Plugin(n)
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
    tc.draw(generators::sampled_from(vec![
        InputMode::Normal,
        InputMode::Locked,
        InputMode::Resize,
        InputMode::Pane,
        InputMode::Tab,
        InputMode::Scroll,
        InputMode::EnterSearch,
        InputMode::Search,
        InputMode::RenameTab,
        InputMode::RenamePane,
        InputMode::Session,
        InputMode::Move,
        InputMode::Prompt,
        InputMode::Tmux,
        InputMode::Agent,
    ]))
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
/// `int64` on the plugin wire, both of which hold any `usize`.
pub(crate) fn positions(tc: &hegel::TestCase) -> Position {
    Position {
        line: Line(tc.draw(generators::integers::<i32>()) as isize),
        column: Column(tc.draw(generators::integers::<usize>())),
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
        plugin_id: tc.draw(generators::integers::<u32>()),
        client_id: tc.draw(generators::integers::<u16>()),
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
        remote_host: optional_strings(tc),
        remote_default_shell: booleans(tc),
    }
}

pub(crate) fn optional_run_command_actions(tc: &hegel::TestCase) -> Option<RunCommandAction> {
    booleans(tc).then(|| run_command_actions(tc))
}

pub(crate) fn run_commands(tc: &hegel::TestCase) -> RunCommand {
    RunCommand {
        command: paths(tc),
        args: up_to(tc, 3, strings),
        cwd: optional_paths(tc),
        hold_on_close: booleans(tc),
        hold_on_start: booleans(tc),
        originating_plugin: optional_originating_plugins(tc),
        use_terminal_title: booleans(tc),
        remote_host: optional_strings(tc),
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
/// `Url::parse` already considers normal.
fn url_safe_segments(tc: &hegel::TestCase) -> String {
    tc.draw(
        generators::vecs(generators::sampled_from(vec![
            'a', 'b', 'z', 'A', 'Z', '0', '9', '_', '-',
        ]))
        .max_size(6),
    )
    .into_iter()
    .collect()
}

pub(crate) fn run_plugin_locations(tc: &hegel::TestCase) -> RunPluginLocation {
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => {
            let segments = up_to(tc, 3, url_safe_segments).join("/");
            let root = if booleans(tc) { "/" } else { "" };
            RunPluginLocation::File(PathBuf::from(format!("{root}{segments}")))
        },
        1 => RunPluginLocation::Zellij(PluginTag::new(url_safe_segments(tc))),
        _ => RunPluginLocation::Remote(format!("https://example.com/{}", url_safe_segments(tc))),
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
    }
}

/// An alias is whatever `RunPluginOrAlias::from_url` could not parse as a plugin URL, and the
/// plugin wire sends one back through that same parse as `string plugin_url` - so a name that
/// reads as a `zellij:`, `file:` or `http(s):` URL would come back as that plugin instead. The
/// prefix keeps the scheme, if the text forms one, from being any of those.
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
    match tc.draw(generators::integers::<u8>().max_value(3)) {
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
    match tc.draw(generators::integers::<u8>().max_value(3)) {
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
        pane_initial_contents: optional_strings(tc),
        default_fg: optional_strings(tc),
        default_bg: optional_strings(tc),
    }
}

pub(crate) fn floating_pane_layouts(tc: &hegel::TestCase) -> FloatingPaneLayout {
    FloatingPaneLayout {
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
        tab_index: usize_as_u32(tc),
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
    match tc.draw(generators::integers::<u8>().max_value(3)) {
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
        2 => LayoutInfo::Url(strings(tc)),
        _ => LayoutInfo::Stringified(strings(tc)),
    }
}

pub(crate) fn warped_panes(tc: &hegel::TestCase) -> WarpedPane {
    WarpedPane {
        host_id: strings(tc),
        control_socket: paths(tc),
        title: optional_strings(tc),
        contents: optional_strings(tc),
        borderless: booleans(tc),
        remote: booleans(tc).then(|| RemotePaneHandoff {
            host: strings(tc),
            pane_token: strings(tc),
            received: tc.draw(generators::integers::<u64>()),
            cmd: run_commands(tc),
        }),
    }
}

pub(crate) fn warped_tabs(tc: &hegel::TestCase) -> WarpedTab {
    WarpedTab {
        name: optional_strings(tc),
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
pub(crate) fn action_of_kind(tc: &hegel::TestCase, kind: ActionKind) -> Action {
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
        ActionKind::PageScrollUp => Action::PageScrollUp,
        ActionKind::PageScrollDown => Action::PageScrollDown,
        ActionKind::HalfPageScrollUp => Action::HalfPageScrollUp,
        ActionKind::HalfPageScrollDown => Action::HalfPageScrollDown,
        ActionKind::ToggleFocusFullscreen => Action::ToggleFocusFullscreen,
        ActionKind::ToggleFocusNoUiFullscreen => Action::ToggleFocusNoUiFullscreen,
        ActionKind::TogglePaneFrames => Action::TogglePaneFrames,
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
            tab_id: optional_usize_as_u32(tc),
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
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewFloatingPane => Action::NewFloatingPane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            placement: new_pane_placements(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewTiledPane => Action::NewTiledPane {
            direction: optional_directions(tc),
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            borderless: optional_booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewInPlacePane => Action::NewInPlacePane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            pane_id_to_replace: booleans(tc).then(|| pane_id(tc)),
            close_replaced_pane: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewStackedPane => Action::NewStackedPane {
            command: optional_run_command_actions(tc),
            pane_name: optional_strings(tc),
            near_current_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::TogglePaneEmbedOrFloating => Action::TogglePaneEmbedOrFloating,
        ActionKind::ToggleFloatingPanes => Action::ToggleFloatingPanes,
        ActionKind::ShowFloatingPanes => Action::ShowFloatingPanes {
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::HideFloatingPanes => Action::HideFloatingPanes {
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::AreFloatingPanesVisible => Action::AreFloatingPanesVisible {
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::CloseFocus => Action::CloseFocus,
        ActionKind::PaneNameInput => Action::PaneNameInput { input: bytes(tc) },
        ActionKind::UndoRenamePane => Action::UndoRenamePane,
        ActionKind::NewTab => Action::NewTab {
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
        ActionKind::GoToTab => Action::GoToTab {
            index: tc.draw(generators::integers::<u32>()),
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
            name: strings(tc),
            tab_position: optional_usize_as_u32(tc),
            pane_id: booleans(tc).then(|| (tc.draw(generators::integers::<u32>()), booleans(tc))),
            layout: booleans(tc).then(|| layout_infos(tc)),
            cwd: optional_paths(tc),
        },
        ActionKind::WarpPane => Action::WarpPane {
            pane_id: booleans(tc).then(|| pane_id(tc)),
            to_session: strings(tc),
            placement: warp_placements(tc),
        },
        ActionKind::AdoptPane => Action::AdoptPane {
            payload: Box::new(warped_panes(tc)),
            placement: warp_placements(tc),
        },
        ActionKind::WarpTab => Action::WarpTab {
            tab_index: optional_usize_as_u32(tc),
            to_session: strings(tc),
        },
        ActionKind::AdoptTab => Action::AdoptTab {
            payload: Box::new(warped_tabs(tc)),
        },
        ActionKind::WarpTabFrom => Action::WarpTabFrom {
            from_session: strings(tc),
            tab_index: usize_as_u32(tc),
        },
        ActionKind::LaunchOrFocusPlugin => Action::LaunchOrFocusPlugin {
            plugin: run_plugins_or_aliases(tc),
            should_float: booleans(tc),
            move_to_focused_tab: booleans(tc),
            should_open_in_place: booleans(tc),
            close_replaced_pane: booleans(tc),
            skip_cache: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::LaunchPlugin => Action::LaunchPlugin {
            plugin: run_plugins_or_aliases(tc),
            should_float: booleans(tc),
            should_open_in_place: booleans(tc),
            close_replaced_pane: booleans(tc),
            skip_cache: booleans(tc),
            cwd: optional_paths(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
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
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewFloatingPluginPane => Action::NewFloatingPluginPane {
            plugin: run_plugins_or_aliases(tc),
            pane_name: optional_strings(tc),
            skip_cache: booleans(tc),
            cwd: optional_paths(tc),
            placement: new_pane_placements(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::NewInPlacePluginPane => Action::NewInPlacePluginPane {
            plugin: run_plugins_or_aliases(tc),
            pane_name: optional_strings(tc),
            skip_cache: booleans(tc),
            close_replaced_pane: booleans(tc),
            no_focus: booleans(tc),
            tab_id: optional_usize_as_u32(tc),
        },
        ActionKind::StartOrReloadPlugin => Action::StartOrReloadPlugin {
            plugin: run_plugins_or_aliases(tc),
        },
        ActionKind::CloseTerminalPane => Action::CloseTerminalPane {
            pane_id: tc.draw(generators::integers::<u32>()),
        },
        ActionKind::ClosePluginPane => Action::ClosePluginPane {
            pane_id: tc.draw(generators::integers::<u32>()),
        },
        ActionKind::FocusTerminalPaneWithId => Action::FocusTerminalPaneWithId {
            pane_id: tc.draw(generators::integers::<u32>()),
            should_float_if_hidden: booleans(tc),
            should_be_in_place_if_hidden: booleans(tc),
        },
        ActionKind::FocusPluginPaneWithId => Action::FocusPluginPaneWithId {
            pane_id: tc.draw(generators::integers::<u32>()),
            should_float_if_hidden: booleans(tc),
            should_be_in_place_if_hidden: booleans(tc),
        },
        ActionKind::RenameTerminalPane => Action::RenameTerminalPane {
            pane_id: tc.draw(generators::integers::<u32>()),
            name: bytes(tc),
        },
        ActionKind::RenamePluginPane => Action::RenamePluginPane {
            pane_id: tc.draw(generators::integers::<u32>()),
            name: bytes(tc),
        },
        ActionKind::RenameTab => Action::RenameTab {
            tab_index: tc.draw(generators::integers::<u32>()),
            name: bytes(tc),
        },
        ActionKind::GoToTabById => Action::GoToTabById {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::CloseTabById => Action::CloseTabById {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::RenameTabById => Action::RenameTabById {
            id: tc.draw(generators::integers::<u64>()),
            name: strings(tc),
        },
        ActionKind::BreakPane => Action::BreakPane,
        ActionKind::BreakPaneRight => Action::BreakPaneRight,
        ActionKind::BreakPaneLeft => Action::BreakPaneLeft,
        ActionKind::FocusHostSession => Action::FocusHostSession,
        ActionKind::FocusGuestSession => Action::FocusGuestSession,
        ActionKind::ToggleHostFullscreen => Action::ToggleHostFullscreen,
        ActionKind::RenameSession => Action::RenameSession { name: strings(tc) },
        ActionKind::Notify => Action::Notify(strings(tc)),
        ActionKind::CliPipe => Action::CliPipe {
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
            plugin_id: tc.draw(generators::optional(generators::integers::<u32>())),
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
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::ToggleActiveSyncTabByTabId => Action::ToggleActiveSyncTabByTabId {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::ToggleFloatingPanesByTabId => Action::ToggleFloatingPanesByTabId {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::PreviousSwapLayoutByTabId => Action::PreviousSwapLayoutByTabId {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::NextSwapLayoutByTabId => Action::NextSwapLayoutByTabId {
            id: tc.draw(generators::integers::<u64>()),
        },
        ActionKind::MoveTabByTabId => Action::MoveTabByTabId {
            id: tc.draw(generators::integers::<u64>()),
            direction: directions(tc),
        },
    }
}
