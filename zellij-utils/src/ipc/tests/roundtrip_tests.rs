use super::test_framework::*;
use crate::data::{
    AgentInfo, AgentSource, AgentState, BareKey, CommandOrPlugin, ConnectToSession, Direction,
    FloatingPaneCoordinates, HostTerminalThemeMode, InputMode, KeyModifier, KeyWithModifier,
    LayoutInfo, LayoutMetadata, NewPanePlacement, OriginatingPlugin, PaneId, PluginTag,
    RemotePaneHandoff, Resize, UnblockCondition, WarpPlacement, WarpedPane, WarpedTab,
};
use crate::input::actions::{Action, SearchDirection, SearchOption};
use crate::input::cli_assets::CliAssets;
use crate::input::command::{OpenFilePayload, RunCommand, RunCommandAction};
use crate::input::layout::{
    FloatingPaneLayout, LayoutConstraint, PercentOrFixed, PluginAlias, PluginUserConfiguration,
    Run, RunPlugin, RunPluginLocation, RunPluginOrAlias, SplitDirection, SplitSize,
    TiledPaneLayout,
};
use crate::input::mouse::{MouseEvent, MouseEventType};
use crate::input::options::{
    Clipboard, HostNotificationProtocol, NestedSessionHandling, NotificationPosition, OnForceClose,
    Options, PaneFrameStyle,
};
use crate::ipc::{
    ClientToServerMsg, ColorRegister, ExitReason, PaneReference, PixelDimensions, ServerToClientMsg,
};
use crate::pane_size::{Size, SizeInPixels};
use crate::position::Position;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use strum::VariantNames;

const ROUNDTRIP_TEST_STACK_SIZE: usize = 32 * 1024 * 1024;

#[test]
fn server_client_contract() {
    std::thread::Builder::new()
        .stack_size(ROUNDTRIP_TEST_STACK_SIZE)
        .spawn(|| {
            test_client_messages();
            test_server_messages();

            // Coverage is asserted inside the same thread, and after both halves have run, so it
            // sees every invocation regardless of the order the test harness picks. The 374
            // invocations below are hand-written values; without this, adding a variant to
            // either enum and forgetting to test it is silent.
            let uncovered = uncovered_variants(Which::Client, ClientToServerMsg::VARIANTS);
            assert!(
                uncovered.is_empty(),
                "no test_client_roundtrip! invocation builds these ClientToServerMsg variants, \
                 so their protobuf conversion is untested: {uncovered:?}"
            );
            let uncovered = uncovered_variants(Which::Server, ServerToClientMsg::VARIANTS);
            assert!(
                uncovered.is_empty(),
                "no test_server_roundtrip! invocation builds these ServerToClientMsg variants, \
                 so their protobuf conversion is untested: {uncovered:?}"
            );
        })
        .expect("failed to spawn roundtrip test thread")
        .join()
        .expect("roundtrip test thread panicked");
}

/// `zellij plugin -- file:keys.lua` is parsed in the client and loaded in the server, so the name
/// as the user wrote it has to survive this hop - without it the server can only look for
/// `$cwd/keys.lua`, and the user's plugin folder is never searched.
///
/// Asserted on the field rather than through `test_client_roundtrip!`, because `RunPlugin`'s
/// `PartialEq` deliberately ignores `relative_path`: a whole-message comparison would pass just
/// as happily with the field dropped.
#[test]
fn a_relative_plugin_path_survives_the_client_server_contract() {
    use crate::client_server_contract::client_server_contract::RunPlugin as ProtoRunPlugin;

    let original = RunPlugin {
        location: RunPluginLocation::File(PathBuf::from("/somewhere/keys.lua")),
        relative_path: Some(PathBuf::from("keys.lua")),
        ..Default::default()
    };
    let protobuf: ProtoRunPlugin = original.clone().into();
    let decoded: RunPlugin = protobuf.try_into().expect("decode");

    assert_eq!(decoded.relative_path, original.relative_path);
}

#[test]
fn a_remote_handoff_without_a_command_arrives_as_no_remote_pane() {
    // Fail closed. A handoff with no command is not a handoff: the adopting
    // session would hold a running process it could not name, re-run on the
    // right machine, or title. Dropping the whole `remote` makes the adoption
    // fail with "no supervisor" and the warp roll back, which is recoverable;
    // arriving half there is not.
    use crate::client_server_contract::client_server_contract::{
        action::ActionType, Action as ProtoAction, AdoptPaneAction as ProtoAdoptPaneAction,
        RemotePaneHandoff as ProtoRemotePaneHandoff, WarpPlacement as ProtoWarpPlacement,
    };

    let adopt_pane = |cmd| ProtoAction {
        action_type: Some(ActionType::AdoptPane(ProtoAdoptPaneAction {
            host_id: String::new(),
            control_socket: String::new(),
            title: None,
            contents: None,
            borderless: false,
            placement: ProtoWarpPlacement::NewTab as i32,
            remote: Some(ProtoRemotePaneHandoff {
                host: "build-box".to_owned(),
                pane_token: "d6a7b8c9e0f1".to_owned(),
                received: 4096,
                cmd,
            }),
            agent: None,
        })),
    };

    match Action::try_from(adopt_pane(None)) {
        Ok(Action::AdoptPane { payload, .. }) => assert_eq!(
            payload.remote, None,
            "a handoff with no command was let through half-built",
        ),
        other => panic!("expected an AdoptPane, got {other:?}"),
    }

    // The same message with a command keeps its handoff, so the assertion above
    // is about the missing command rather than about this shape never surviving.
    let cmd = Some(RunCommandAction {
        command: PathBuf::from("/usr/bin/cargo"),
        args: vec!["watch".to_owned()],
        remote_host: Some("build-box".to_owned()),
        ..Default::default()
    })
    .map(|cmd| cmd.into());
    match Action::try_from(adopt_pane(cmd)) {
        Ok(Action::AdoptPane { payload, .. }) => {
            let remote = payload.remote.expect("the handoff should have survived");
            assert_eq!(remote.host, "build-box");
            assert_eq!(remote.cmd.remote_host.as_deref(), Some("build-box"));
        },
        other => panic!("expected an AdoptPane, got {other:?}"),
    }
}

fn test_client_messages() {
    let empty_context = BTreeMap::new();
    let mut demo_context = BTreeMap::new();
    demo_context.insert("demo_key1".to_owned(), "demo_value1".to_owned());
    demo_context.insert("demo_key2".to_owned(), "demo_value2".to_owned());
    demo_context.insert("demo_key3".to_owned(), "demo_value3".to_owned());
    let mut swap_tiled_layouts_1 = BTreeMap::new();
    swap_tiled_layouts_1.insert(
        LayoutConstraint::MaxPanes(1),
        TiledPaneLayout {
            name: Some("max_panes_1".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_1.insert(
        LayoutConstraint::MinPanes(2),
        TiledPaneLayout {
            name: Some("min_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_1.insert(
        LayoutConstraint::ExactPanes(3),
        TiledPaneLayout {
            name: Some("exact_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_1.insert(
        LayoutConstraint::NoConstraint,
        TiledPaneLayout {
            name: Some("no_constraint".to_owned()),
            ..Default::default()
        },
    );
    let swap_tiled_layouts_1 = (
        swap_tiled_layouts_1,
        Some("swap_tiled_layouts_1".to_owned()),
    );
    let mut swap_tiled_layouts_2 = BTreeMap::new();
    swap_tiled_layouts_2.insert(
        LayoutConstraint::MaxPanes(1),
        TiledPaneLayout {
            name: Some("max_panes_1".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_2.insert(
        LayoutConstraint::MinPanes(2),
        TiledPaneLayout {
            name: Some("min_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_2.insert(
        LayoutConstraint::ExactPanes(3),
        TiledPaneLayout {
            name: Some("exact_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_2.insert(
        LayoutConstraint::NoConstraint,
        TiledPaneLayout {
            name: Some("no_constraint".to_owned()),
            ..Default::default()
        },
    );
    let swap_tiled_layouts_2 = (
        swap_tiled_layouts_2,
        Some("swap_tiled_layouts_2".to_owned()),
    );
    let mut swap_tiled_layouts_3 = BTreeMap::new();
    swap_tiled_layouts_3.insert(
        LayoutConstraint::MaxPanes(1),
        TiledPaneLayout {
            name: Some("max_panes_1".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_3.insert(
        LayoutConstraint::MinPanes(2),
        TiledPaneLayout {
            name: Some("min_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_3.insert(
        LayoutConstraint::ExactPanes(3),
        TiledPaneLayout {
            name: Some("exact_panes_2".to_owned()),
            ..Default::default()
        },
    );
    swap_tiled_layouts_3.insert(
        LayoutConstraint::NoConstraint,
        TiledPaneLayout {
            name: Some("no_constraint".to_owned()),
            ..Default::default()
        },
    );
    let swap_tiled_layouts_3 = (swap_tiled_layouts_3, None);

    let mut swap_floating_layouts_1 = BTreeMap::new();
    swap_floating_layouts_1.insert(
        LayoutConstraint::MaxPanes(1),
        vec![
            FloatingPaneLayout {
                name: Some("max_panes_1".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("max_panes_1_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_1.insert(
        LayoutConstraint::MinPanes(2),
        vec![
            FloatingPaneLayout {
                name: Some("min_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("min_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_1.insert(
        LayoutConstraint::ExactPanes(3),
        vec![
            FloatingPaneLayout {
                name: Some("exact_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("exact_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_1.insert(
        LayoutConstraint::NoConstraint,
        vec![
            FloatingPaneLayout {
                name: Some("no_constraint".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("no_constraint_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    let swap_floating_layouts_1 = (
        swap_floating_layouts_1,
        Some("swap_floating_layouts_1".to_owned()),
    );
    let mut swap_floating_layouts_2 = BTreeMap::new();
    swap_floating_layouts_2.insert(
        LayoutConstraint::MaxPanes(1),
        vec![
            FloatingPaneLayout {
                name: Some("max_panes_1".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("max_panes_1_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_2.insert(
        LayoutConstraint::MinPanes(2),
        vec![
            FloatingPaneLayout {
                name: Some("min_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("min_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_2.insert(
        LayoutConstraint::ExactPanes(3),
        vec![
            FloatingPaneLayout {
                name: Some("exact_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("exact_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_2.insert(
        LayoutConstraint::NoConstraint,
        vec![
            FloatingPaneLayout {
                name: Some("no_constraint".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("no_constraint_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    let swap_floating_layouts_2 = (
        swap_floating_layouts_2,
        Some("swap_floating_layouts_2".to_owned()),
    );
    let mut swap_floating_layouts_3 = BTreeMap::new();
    swap_floating_layouts_3.insert(
        LayoutConstraint::MaxPanes(1),
        vec![
            FloatingPaneLayout {
                name: Some("max_panes_1".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("max_panes_1_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_3.insert(
        LayoutConstraint::MinPanes(2),
        vec![
            FloatingPaneLayout {
                name: Some("min_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("min_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_3.insert(
        LayoutConstraint::ExactPanes(3),
        vec![
            FloatingPaneLayout {
                name: Some("exact_panes_2".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("exact_panes_2_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    swap_floating_layouts_3.insert(
        LayoutConstraint::NoConstraint,
        vec![
            FloatingPaneLayout {
                name: Some("no_constraint".to_owned()),
                ..Default::default()
            },
            FloatingPaneLayout {
                name: Some("no_constraint_1".to_owned()),
                ..Default::default()
            },
        ],
    );
    let swap_floating_layouts_3 = (swap_floating_layouts_3, None);
    let mut demo_modifiers_1 = BTreeSet::new();
    let mut demo_modifiers_2 = BTreeSet::new();
    let mut demo_modifiers_3 = BTreeSet::new();
    let mut demo_modifiers_4 = BTreeSet::new();
    demo_modifiers_1.insert(KeyModifier::Ctrl);
    demo_modifiers_2.insert(KeyModifier::Ctrl);
    demo_modifiers_3.insert(KeyModifier::Ctrl);
    demo_modifiers_4.insert(KeyModifier::Ctrl);
    demo_modifiers_2.insert(KeyModifier::Alt);
    demo_modifiers_3.insert(KeyModifier::Alt);
    demo_modifiers_4.insert(KeyModifier::Alt);
    demo_modifiers_3.insert(KeyModifier::Shift);
    demo_modifiers_4.insert(KeyModifier::Shift);
    demo_modifiers_4.insert(KeyModifier::Super);

    test_client_roundtrip!(ClientToServerMsg::DetachSession { client_ids: vec![] });
    test_client_roundtrip!(ClientToServerMsg::DetachSession {
        client_ids: vec![1],
    });
    test_client_roundtrip!(ClientToServerMsg::DetachSession {
        client_ids: vec![1, 2, 999],
    });
    test_client_roundtrip!(ClientToServerMsg::TerminalPixelDimensions {
        pixel_dimensions: PixelDimensions {
            text_area_size: None,
            character_cell_size: None,
        },
    });
    test_client_roundtrip!(ClientToServerMsg::TerminalPixelDimensions {
        pixel_dimensions: PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 600
            }),
            character_cell_size: Some(SizeInPixels {
                width: 10,
                height: 20
            }),
        },
    });
    test_client_roundtrip!(ClientToServerMsg::KittyGraphicsSupport { supported: true });
    test_client_roundtrip!(ClientToServerMsg::KittyGraphicsSupport { supported: false });
    test_client_roundtrip!(ClientToServerMsg::SixelSupport { supported: true });
    test_client_roundtrip!(ClientToServerMsg::SixelSupport { supported: false });
    test_client_roundtrip!(ClientToServerMsg::BackgroundColor {
        color: "red".to_string(),
    });
    test_client_roundtrip!(ClientToServerMsg::BackgroundColor {
        color: "#FF0000".to_string(),
    });
    test_client_roundtrip!(ClientToServerMsg::BackgroundColor {
        color: "".to_string(),
    });
    test_client_roundtrip!(ClientToServerMsg::ForegroundColor {
        color: "blue".to_string(),
    });
    test_client_roundtrip!(ClientToServerMsg::ColorRegisters {
        color_registers: vec![],
    });
    test_client_roundtrip!(ClientToServerMsg::ColorRegisters {
        color_registers: vec![
            ColorRegister {
                index: 0,
                color: "black".to_string()
            },
            ColorRegister {
                index: 1,
                color: "red".to_string()
            },
            ColorRegister {
                index: 255,
                color: "#FFFFFF".to_string()
            },
        ],
    });
    test_client_roundtrip!(ClientToServerMsg::TerminalResize {
        new_size: Size { cols: 80, rows: 24 },
    });
    test_client_roundtrip!(ClientToServerMsg::TerminalResize {
        new_size: Size {
            cols: 200,
            rows: 50
        },
    });
    test_client_roundtrip!(ClientToServerMsg::TerminalResize {
        new_size: Size { cols: 40, rows: 38 },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets::default(),
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets::default(),
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            config_file_path: Some(PathBuf::from("/path/to/config/file.kdl")),
            config_dir: Some(PathBuf::from("/path/to/config/dir")),
            should_ignore_config: true,
            configuration_options: None,
            layout: None,
            terminal_window_size: Size { rows: 80, cols: 42 },
            data_dir: Some(PathBuf::from("/path/to/data/dir")),
            is_debug: true,
            max_panes: Some(4),
            force_run_layout_commands: true,
            cwd: Some(PathBuf::from("/path/to/cwd")),
            host_terminal_env: [("TERM".to_owned(), "xterm-kitty".to_owned())]
                .into_iter()
                .collect(),
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            config_file_path: Some(PathBuf::from("/path/to/config/file.kdl")),
            config_dir: Some(PathBuf::from("/path/to/config/dir")),
            should_ignore_config: true,
            configuration_options: Some(Options::default()),
            layout: None,
            terminal_window_size: Size { rows: 80, cols: 42 },
            data_dir: Some(PathBuf::from("/path/to/data/dir")),
            is_debug: true,
            max_panes: Some(4),
            force_run_layout_commands: true,
            cwd: Some(PathBuf::from("/path/to/cwd")),
            host_terminal_env: [("TERM".to_owned(), "xterm-kitty".to_owned())]
                .into_iter()
                .collect(),
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            config_file_path: Some(PathBuf::from("/path/to/config/file.kdl")),
            config_dir: Some(PathBuf::from("/path/to/config/dir")),
            should_ignore_config: true,
            configuration_options: Some(Options {
                simplified_ui: Some(true),
                theme: Some("theme".to_owned()),
                theme_dark: Some("theme_dark".to_owned()),
                theme_light: Some("theme_light".to_owned()),
                explicit_theme_hue: Some(crate::data::ThemeHue::Light),
                default_mode: Some(InputMode::Normal),
                default_shell: Some(PathBuf::from("default_shell")),
                default_cwd: Some(PathBuf::from("default_cwd")),
                default_layout: Some(PathBuf::from("default_layout")),
                layout_dir: Some(PathBuf::from("layout_dir")),
                builtin_plugins_dir: Some(PathBuf::from("builtin_plugins_dir")),
                theme_dir: Some(PathBuf::from("theme_dir")),
                user_plugin_dir: Some(PathBuf::from("user_plugin_dir")),
                mouse_mode: Some(true),
                // Intentionally not carried over the wire; see the field's docs.
                detachable_panes: None,
                pane_frames: Some(true),
                pane_frame_style: Some(PaneFrameStyle::Full),
                notification_position: Some(NotificationPosition::BottomLeft),
                notification_timeout: Some(9),
                mirror_session: Some(true),
                on_force_close: Some(OnForceClose::Quit),
                scroll_buffer_size: Some(100000),
                copy_command: Some("copy_command".to_owned()),
                copy_clipboard: Some(Clipboard::System),
                copy_on_select: Some(true),
                osc8_hyperlinks: Some(true),
                scrollback_editor: Some(PathBuf::from("scrollback_editor")),
                session_name: Some("session_name".to_owned()),
                attach_to_session: Some(true),
                auto_layout: Some(true),
                session_serialization: Some(true),
                session_recording: Some(true),
                session_recording_input: Some(true),
                serialize_pane_viewport: Some(true),
                scrollback_lines_to_serialize: Some(10000),
                styled_underlines: Some(true),
                serialization_interval: Some(1),
                disable_session_metadata: Some(true),
                support_kitty_keyboard_protocol: Some(true),
                support_kitty_graphics_protocol: Some(false),
                stacked_resize: Some(true),
                stacked_pane_list: Some(true),
                show_startup_tips: Some(true),
                show_release_notes: Some(true),
                advanced_mouse_actions: Some(true),
                mouse_scroll_resize: Some(true),
                post_command_discovery_hook: Some("post_command_discovery_hook".to_owned()),
                mouse_hover_effects: Some(false),
                visual_bell: Some(true),
                focus_follows_mouse: Some(false),
                mouse_click_through: Some(false),
                context_menu_on_right_click: Some(true),
                osc133_command_selection: Some(false),
                word_separators: Some("[]{}<>():".to_owned()),
                host_notification_protocol: Some(HostNotificationProtocol::Osc99),
                nested_session_handling: Some(NestedSessionHandling::Fullscreen),
                dangerously_enable_paste_buffer_read: Some(true),
            }),
            layout: None,
            terminal_window_size: Size { rows: 80, cols: 42 },
            data_dir: Some(PathBuf::from("/path/to/data/dir")),
            is_debug: true,
            max_panes: Some(4),
            force_run_layout_commands: true,
            cwd: Some(PathBuf::from("/path/to/cwd")),
            host_terminal_env: [("TERM".to_owned(), "xterm-kitty".to_owned())]
                .into_iter()
                .collect(),
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Normal),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Locked),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Resize),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Pane),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Tab),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Scroll),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::EnterSearch),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Search),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::RenameTab),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::RenamePane),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Session),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Move),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Prompt),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                default_mode: Some(InputMode::Tmux),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                on_force_close: Some(OnForceClose::Detach),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                copy_clipboard: Some(Clipboard::Primary),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                nested_session_handling: Some(NestedSessionHandling::Ask),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                nested_session_handling: Some(NestedSessionHandling::Fullscreen),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                nested_session_handling: Some(NestedSessionHandling::Descend),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::FirstClientConnected {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                nested_session_handling: Some(NestedSessionHandling::Never),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_position_to_focus: None,
        pane_to_focus: None,
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_position_to_focus: Some(0),
        pane_to_focus: None,
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_position_to_focus: None,
        pane_to_focus: Some(PaneReference {
            pane_id: 0,
            is_plugin: false,
        }),
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_position_to_focus: None,
        pane_to_focus: Some(PaneReference {
            pane_id: 100,
            is_plugin: true,
        }),
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                nested_session_handling: Some(NestedSessionHandling::Descend),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        tab_position_to_focus: None,
        pane_to_focus: None,
    });
    // TODO: Action
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Quit,
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Quit,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    // Moving a pane between sessions. The placement enum and the optional pane id
    // are the parts a hand-written protobuf conversion gets wrong.
    for placement in [
        WarpPlacement::NewTab,
        WarpPlacement::FocusedTab,
        WarpPlacement::Floating,
    ] {
        test_client_roundtrip!(ClientToServerMsg::Action {
            action: Action::WarpPane {
                pane_id: None,
                to_session: "other".to_owned(),
                placement,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        });
        test_client_roundtrip!(ClientToServerMsg::Action {
            action: Action::WarpPane {
                pane_id: Some(PaneId::Terminal(3)),
                to_session: "other".to_owned(),
                placement,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        });
    }
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptPane {
            payload: Box::new(WarpedPane {
                host_id: "a3f1c2d4e5b6".to_owned(),
                control_socket: PathBuf::from("/run/user/1000/zellij/pty/a3f1c2d4e5b6.sock"),
                title: None,
                contents: None,
                borderless: false,
                remote: None,
                agent: Some(AgentInfo {
                    kind: "codex".to_owned(),
                    state: AgentState::Waiting,
                    source: AgentSource::Reported,
                    detail: None,
                    waiting_on: Some("approval".to_owned()),
                    cwd: Some("/work".to_owned()),
                    session_id: Some("agent-session".to_owned()),
                }),
            }),
            placement: WarpPlacement::NewTab,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    // Adopting a single pane whose process is on another machine. This is the
    // exact shape `zellij pty-host adopt --host <machine> <token>` builds, and
    // the only one where the pane's whole address is in `remote` — the two local
    // fields are empty because there is no supervisor here to name.
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptPane {
            payload: Box::new(WarpedPane {
                host_id: String::new(),
                control_socket: PathBuf::new(),
                title: Some("cargo watch".to_owned()),
                contents: None,
                borderless: false,
                remote: Some(RemotePaneHandoff {
                    host: "build-box".to_owned(),
                    pane_token: "d6a7b8c9e0f1".to_owned(),
                    received: 4_294_967_296,
                    cmd: RunCommand {
                        command: PathBuf::from("/usr/bin/cargo"),
                        args: vec!["watch".to_owned()],
                        cwd: Some(PathBuf::from("/home/build/project")),
                        remote_host: Some("build-box".to_owned()),
                        remote_default_shell: true,
                        ..Default::default()
                    },
                }),
                agent: None,
            }),
            placement: WarpPlacement::NewTab,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    // A warped pane carries its whole scrollback, so the payload is routinely
    // orders of magnitude larger than any other action on this wire.
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptPane {
            payload: Box::new(WarpedPane {
                host_id: "a3f1c2d4e5b6".to_owned(),
                control_socket: PathBuf::from("/run/user/1000/zellij/pty/a3f1c2d4e5b6.sock"),
                title: Some("vim src/main.rs".to_owned()),
                contents: Some("\u{1b}[38;5;9mred\u{1b}[m ".repeat(200_000)),
                borderless: true,
                remote: None,
                agent: None,
            }),
            placement: WarpPlacement::Floating,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    // Moving a whole tab. The two directions are small, but the tab itself
    // carries a nested layout and a pane list whose *order* is what places the
    // panes, so this is the one warp payload where a conversion that dropped or
    // reordered a repeated field would be silently wrong rather than obviously so.
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WarpTab {
            tab_index: None,
            to_session: "other".to_owned(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WarpTab {
            tab_index: Some(2),
            to_session: "other".to_owned(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WarpTabFrom {
            from_session: "other".to_owned(),
            tab_index: 2,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptTab {
            payload: Box::new(WarpedTab::default()),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptTab {
            payload: Box::new(WarpedTab {
                name: Some("build".to_owned()),
                tiled_layout: TiledPaneLayout {
                    children_split_direction: SplitDirection::Vertical,
                    children: vec![
                        TiledPaneLayout {
                            run: Some(Run::Command(RunCommand {
                                command: PathBuf::from("vim"),
                                args: vec!["src/main.rs".to_owned()],
                                ..Default::default()
                            })),
                            split_size: Some(SplitSize::Percent(60)),
                            ..Default::default()
                        },
                        TiledPaneLayout {
                            run: Some(Run::Command(RunCommand {
                                command: PathBuf::from("cargo"),
                                args: vec!["watch".to_owned()],
                                ..Default::default()
                            })),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                floating_layout: vec![FloatingPaneLayout {
                    name: Some("notes".to_owned()),
                    x: Some(PercentOrFixed::Percent(10)),
                    y: Some(PercentOrFixed::Percent(20)),
                    width: Some(PercentOrFixed::Percent(30)),
                    height: Some(PercentOrFixed::Percent(40)),
                    ..Default::default()
                }],
                tiled_panes: vec![
                    WarpedPane {
                        host_id: "a3f1c2d4e5b6".to_owned(),
                        control_socket: PathBuf::from(
                            "/run/user/1000/zellij/pty/a3f1c2d4e5b6.sock"
                        ),
                        title: Some("vim src/main.rs".to_owned()),
                        contents: Some("\u{1b}[38;5;9mred\u{1b}[m ".repeat(100_000)),
                        borderless: false,
                        remote: None,
                        agent: None,
                    },
                    // A pane whose process is on another machine, in a tab
                    // otherwise full of local ones. The mix is the point: a tab
                    // is warped whole, so both addressing shapes have to survive
                    // the same message.
                    WarpedPane {
                        host_id: String::new(),
                        control_socket: PathBuf::new(),
                        title: Some("cargo watch".to_owned()),
                        contents: None,
                        borderless: true,
                        remote: Some(RemotePaneHandoff {
                            host: "build-box".to_owned(),
                            pane_token: "d6a7b8c9e0f1".to_owned(),
                            received: 4_294_967_296,
                            cmd: RunCommand {
                                command: PathBuf::from("/usr/bin/cargo"),
                                args: vec!["watch".to_owned()],
                                cwd: Some(PathBuf::from("/home/build/project")),
                                hold_on_close: true,
                                remote_host: Some("build-box".to_owned()),
                                ..Default::default()
                            },
                        }),
                        agent: None,
                    },
                ],
                floating_panes: vec![WarpedPane {
                    host_id: "c5f3e4d6a7b8".to_owned(),
                    control_socket: PathBuf::from("/run/user/1000/zellij/pty/c5f3e4d6a7b8.sock"),
                    title: Some("notes".to_owned()),
                    contents: None,
                    borderless: false,
                    remote: None,
                    agent: None,
                }],
                hide_floating_panes: true,
            }),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Write {
            key_with_modifier: None,
            bytes: vec![],
            is_kitty_keyboard_protocol: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Write {
            key_with_modifier: Some(KeyWithModifier::new(BareKey::Char('a'))),
            bytes: "a".as_bytes().to_vec(),
            is_kitty_keyboard_protocol: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WriteChars {
            chars: "my chars".to_owned(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchToMode {
            input_mode: InputMode::Locked,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchModeForAllClients {
            input_mode: InputMode::Locked,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Increase,
            direction: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Left),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Right),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Up),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Down),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusNextPane,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPreviousPane,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchFocus,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveFocus {
            direction: Direction::Up,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveFocusOrTab {
            direction: Direction::Up,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePane { direction: None },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePane {
            direction: Some(Direction::Up),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneBackwards,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClearScreen,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: true,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: true,
            pane_id: Some(PaneId::Terminal(5)),
            ansi: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: None,
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpLayout,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollback { ansi: false },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUp,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpAt {
            position: Position::new(0, 0),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpAt {
            position: Position::new(-10, 0),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDown,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDownAt {
            position: Position::new(0, 0),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToBottom,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToTop,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToPreviousPrompt,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToNextPrompt,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SelectCommandAtScrollPosition,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CopyLastCommandOutput,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollUp,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollDown,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollUp,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollDown,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusFullscreen,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusNoUiFullscreen,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneFrames,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleActiveSyncTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewPane {
            direction: Some(Direction::Right),
            pane_name: Some("pane_name".to_owned()),
            start_suppressed: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: None,
                cwd: None,
                originating_plugin: None,
            },
            direction: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            start_suppressed: false,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: empty_context.clone(),
                })
            },
            direction: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            start_suppressed: false,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            start_suppressed: false,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: Some(Direction::Right),
            floating: true,
            in_place: true,
            close_replaced_pane: false,
            start_suppressed: true,
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                None,
                None,
                None,
                None,
                None,
                Some(false)
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: Some(Direction::Right),
            floating: true,
            in_place: true,
            close_replaced_pane: false,
            start_suppressed: true,
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("100%".to_owned()),
                None,
                None,
                None,
                None,
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: Some(Direction::Right),
            floating: true,
            in_place: true,
            close_replaced_pane: false,
            start_suppressed: true,
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("10".to_owned()),
                None,
                None,
                None,
                None,
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: Some(Direction::Right),
            floating: true,
            in_place: true,
            close_replaced_pane: false,
            start_suppressed: true,
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("10".to_owned()),
                Some("50%".to_owned()),
                Some("10".to_owned()),
                Some("20".to_owned()),
                Some(true),
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 11,
                    client_id: 22,
                    context: demo_context.clone(),
                })
            },
            direction: Some(Direction::Right),
            floating: true,
            in_place: true,
            close_replaced_pane: false,
            start_suppressed: true,
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                None,
                None,
                None,
                None,
                Some(false),
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPane {
            command: None,
            pane_name: None,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPane {
            command: Some(RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec![],
                cwd: None,
                direction: None,
                hold_on_close: false,
                hold_on_start: false,
                originating_plugin: None,
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("10".to_owned()),
                None,
                None,
                None,
                None,
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPane {
            command: Some(RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec!["arg1".to_owned(), "arg2".to_owned()],
                cwd: Some(PathBuf::from("/path/to/cwd")),
                direction: Some(Direction::Right),
                hold_on_close: true,
                hold_on_start: true,
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 1,
                    client_id: 2,
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("10".to_owned()),
                None,
                None,
                None,
                None,
                Some(false),
            )),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPane {
            command: None,
            direction: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPane {
            command: Some(RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec!["arg1".to_owned(), "arg2".to_owned()],
                cwd: Some(PathBuf::from("/path/to/cwd")),
                direction: Some(Direction::Right),
                hold_on_close: true,
                hold_on_start: true,
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 1,
                    client_id: 2,
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            }),
            direction: Some(Direction::Right),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            borderless: Some(true),
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePane {
            command: Some(RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec!["arg1".to_owned(), "arg2".to_owned()],
                cwd: Some(PathBuf::from("/path/to/cwd")),
                direction: Some(Direction::Right),
                hold_on_close: true,
                hold_on_start: true,
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 1,
                    client_id: 2,
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewStackedPane {
            command: Some(RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec!["arg1".to_owned(), "arg2".to_owned()],
                cwd: Some(PathBuf::from("/path/to/cwd")),
                direction: Some(Direction::Right),
                hold_on_close: true,
                hold_on_start: true,
                originating_plugin: Some(OriginatingPlugin {
                    plugin_id: 1,
                    client_id: 2,
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewStackedPane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneEmbedOrFloating,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFloatingPanes,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseFocus,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PaneNameInput {
            input: "name input".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenamePane,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "spark".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "x".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout::default()),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                children_split_direction: SplitDirection::Vertical,
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                children_split_direction: SplitDirection::Horizontal,
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                children_split_direction: SplitDirection::Horizontal,
                name: Some("tiled_layout_name".to_owned()),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        name: Some("first_item_in_vec".to_owned()),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        name: Some("second_item_in_vec".to_owned()),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        name: Some("third_item_in_vec".to_owned()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                split_size: Some(SplitSize::Fixed(1)),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                split_size: Some(SplitSize::Percent(50)),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                    RunPlugin::default()
                ))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    _allow_exec_host_cmd: true,
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    location: RunPluginLocation::File(PathBuf::from("/my/plugin/location")),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    location: RunPluginLocation::Zellij(PluginTag::new("my_plugin_tag")),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    location: RunPluginLocation::Remote("my_remote_url".to_owned()),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    configuration: PluginUserConfiguration::new(demo_context.clone()),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    configuration: PluginUserConfiguration::new(demo_context.clone()),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    configuration: PluginUserConfiguration::new(empty_context.clone()),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    initial_cwd: Some(PathBuf::from("/initial/cwd")),
                    ..Default::default()
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::default()))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias {
                    name: "alias name".to_owned(),
                    configuration: Some(PluginUserConfiguration::new(demo_context.clone())),
                    initial_cwd: Some(PathBuf::from("initial_cwd")),
                    run_plugin: Some(RunPlugin::default()),
                }))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/path/to/command"),
                    args: vec![],
                    cwd: None,
                    hold_on_close: false,
                    hold_on_start: false,
                    originating_plugin: None,
                    use_terminal_title: true,
                    remote_host: None,
                    remote_default_shell: false,
                })),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/path/to/command"),
                    args: vec!["arg1".to_owned(), "arg2".to_owned(), "arg3".to_owned()],
                    cwd: Some(PathBuf::from("/path/to/cwd")),
                    hold_on_close: true,
                    hold_on_start: true,
                    originating_plugin: Some(OriginatingPlugin {
                        plugin_id: 11,
                        client_id: 22,
                        context: empty_context.clone(),
                    }),
                    use_terminal_title: true,
                    remote_host: None,
                    remote_default_shell: false,
                })),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::EditFile(PathBuf::from("/path/to/file"), None, None)),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::EditFile(
                    PathBuf::from("/path/to/file"),
                    Some(10),
                    Some(PathBuf::from("/path"))
                )),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Cwd(PathBuf::from("/path/to/cwd"))),
                ..Default::default()
            }),
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![
                FloatingPaneLayout {
                    name: Some("first floating layout".to_owned()),
                    ..Default::default()
                },
                FloatingPaneLayout {
                    name: Some("third floating layout".to_owned()),
                    height: Some(PercentOrFixed::Percent(10)),
                    width: Some(PercentOrFixed::Fixed(20)),
                    x: Some(PercentOrFixed::Percent(30)),
                    y: Some(PercentOrFixed::Percent(40)),
                    pinned: Some(true),
                    borderless: None,
                    run: Some(Run::Cwd(PathBuf::from("/path/to/cwd"))),
                    focus: Some(true),
                    already_running: true,
                    pane_initial_contents: Some("pane_initial_contents".to_owned()),
                    logical_position: Some(15),
                    default_fg: None,
                    default_bg: None,
                },
                FloatingPaneLayout {
                    name: Some("third floating layout".to_owned()),
                    ..Default::default()
                },
                FloatingPaneLayout::default(),
            ],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: Some(vec![
                swap_tiled_layouts_1.clone(),
                swap_tiled_layouts_2.clone(),
                swap_tiled_layouts_3.clone()
            ]),
            swap_floating_layouts: Some(vec![
                swap_floating_layouts_1,
                swap_floating_layouts_2,
                swap_floating_layouts_3
            ]),
            tab_name: None,
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: Some("tab_name".to_owned()),
            should_change_focus_to_new_tab: false,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd: Some(PathBuf::from("relative/path/to/cwd")),
            initial_panes: None,
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd: None,
            initial_panes: Some(vec![
                CommandOrPlugin::Command(RunCommandAction {
                    command: PathBuf::from("/path/to/command"),
                    args: vec![],
                    cwd: None,
                    direction: None,
                    hold_on_close: false,
                    hold_on_start: false,
                    originating_plugin: None,
                    use_terminal_title: false,
                    remote_host: None,
                    remote_default_shell: false,
                }),
                CommandOrPlugin::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin::default())),
            ]),
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::OverrideLayout {
            tabs: vec![],
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            apply_only_to_active_tab: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NoOp,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToNextTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToPreviousTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTab { index: 0 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTabName {
            name: "tab_name".to_owned(),
            create: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTabName {
            name: "tab_name".to_owned(),
            create: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TabNameInput {
            input: "my cool tab_name".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenameTab,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveTab {
            direction: Direction::Right,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Run {
            command: RunCommandAction {
                command: PathBuf::from("/path/to/command"),
                args: vec![],
                cwd: None,
                direction: None,
                hold_on_close: false,
                hold_on_start: false,
                originating_plugin: None,
                use_terminal_title: false,
                remote_host: None,
                remote_default_shell: false,
            },
            near_current_pane: false,
            no_focus: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Detach,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetDarkTheme,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetLightTheme,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleTheme,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            should_float: true,
            move_to_focused_tab: true,
            should_open_in_place: true,
            close_replaced_pane: false,
            skip_cache: true,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
            should_float: false,
            move_to_focused_tab: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchPlugin {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            should_float: true,
            should_open_in_place: true,
            close_replaced_pane: false,
            skip_cache: true,
            cwd: None,
            no_focus: true,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchPlugin {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
            should_float: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            cwd: Some(PathBuf::from("/path/to/cwd")),
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MouseEvent {
            event: MouseEvent {
                event_type: MouseEventType::Motion,
                left: true,
                right: true,
                middle: true,
                wheel_up: true,
                wheel_down: true,
                wheel_left: true,
                wheel_right: true,
                shift: true,
                alt: true,
                ctrl: true,
                position: Position::new(-10, 0),
            }
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MouseEvent {
            event: MouseEvent {
                event_type: MouseEventType::Press,
                left: true,
                right: true,
                middle: true,
                wheel_up: true,
                wheel_down: true,
                wheel_left: true,
                wheel_right: true,
                shift: true,
                alt: true,
                ctrl: true,
                position: Position::new(-10, 0),
            }
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MouseEvent {
            event: MouseEvent {
                event_type: MouseEventType::Release,
                left: true,
                right: true,
                middle: true,
                wheel_up: true,
                wheel_down: true,
                wheel_left: true,
                wheel_right: true,
                shift: true,
                alt: true,
                ctrl: true,
                position: Position::new(10, 0),
            }
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Copy,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Confirm,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Deny,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SkipConfirm {
            action: Box::new(Action::Quit)
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchInput {
            input: "my_search_input".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Search {
            direction: SearchDirection::Up,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Search {
            direction: SearchDirection::Down,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::CaseSensitivity,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::WholeWord,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::Wrap,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleMouseMode,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PreviousSwapLayout,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NextSwapLayout,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::QueryTabNames,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPluginPane {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
            pane_name: Some("my_pane_name".to_owned()),
            skip_cache: false,
            cwd: Some(PathBuf::from("relative/path/to/cwd")),
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: Some("my_pane_name".to_owned()),
            skip_cache: true,
            cwd: Some(PathBuf::from("relative/path/to/cwd")),
            placement: NewPanePlacement::Floating(None),
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPluginPane {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
            pane_name: Some("my_pane_name".to_owned()),
            skip_cache: true,
            cwd: Some(PathBuf::from("relative/path/to/cwd")),
            placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                Some("10".to_owned()),
                Some("10%".to_owned()),
                None,
                None,
                Some(true),
                Some(false),
            )),
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePluginPane {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
            pane_name: Some("my_pane_name".to_owned()),
            skip_cache: true,
            close_replaced_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPane {
            direction: Some(Direction::Right),
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(3),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPane {
            command: None,
            pane_name: None,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(5),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewStackedPane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(2),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewBlockingPane {
            placement: NewPanePlacement::Tiled {
                direction: None,
                borderless: None
            },
            pane_name: None,
            command: None,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewBlockingPane {
            placement: NewPanePlacement::Tiled {
                direction: None,
                borderless: None
            },
            pane_name: None,
            command: None,
            unblock_condition: Some(UnblockCondition::OnExitFailure),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: None,
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: None,
                cwd: None,
                originating_plugin: None,
            },
            direction: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            start_suppressed: false,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(4),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane: false,
            tab_id: Some(7),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            should_float: true,
            move_to_focused_tab: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            tab_id: Some(2),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::LaunchPlugin {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            should_float: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            cwd: None,
            no_focus: false,
            tab_id: Some(6),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: None,
            skip_cache: false,
            cwd: None,
            no_focus: false,
            tab_id: Some(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: None,
            skip_cache: false,
            cwd: None,
            placement: NewPanePlacement::Floating(None),
            no_focus: false,
            tab_id: Some(3),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: None,
            skip_cache: false,
            close_replaced_pane: false,
            no_focus: false,
            tab_id: Some(2),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StartOrReloadPlugin {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseTerminalPane { pane_id: 11 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClosePluginPane { pane_id: 12 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusTerminalPaneWithId {
            pane_id: 12,
            should_float_if_hidden: false,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusTerminalPaneWithId {
            pane_id: 12,
            should_float_if_hidden: true,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPluginPaneWithId {
            pane_id: 12,
            should_float_if_hidden: false,
            should_be_in_place_if_hidden: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPluginPaneWithId {
            pane_id: 12,
            should_float_if_hidden: true,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameTerminalPane {
            pane_id: 12,
            name: "terminal_pane_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePluginPane {
            pane_id: 12,
            name: "plugin_pane_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameTab {
            tab_index: 11,
            name: "tab_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPane,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPaneRight,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPaneLeft,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameSession {
            name: "new_session_name".to_owned(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CliPipe {
            pipe_id: "pipe_id_name".to_owned(),
            name: None,
            payload: None,
            args: None,
            plugin: None,
            configuration: None,
            launch_new: false,
            skip_cache: false,
            floating: None,
            in_place: None,
            cwd: None,
            pane_title: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CliPipe {
            pipe_id: "pipe_id_name".to_owned(),
            name: Some("pipe_name".to_owned()),
            payload: Some("pipe_payload".to_owned()),
            args: Some(demo_context.clone()),
            plugin: Some("zellij:status-bare".to_owned()),
            configuration: Some(demo_context.to_owned()),
            launch_new: true,
            skip_cache: true,
            floating: Some(true),
            in_place: Some(false),
            cwd: Some(PathBuf::from("/path/to/cwd")),
            pane_title: Some("pane_title".to_owned()),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::KeybindPipe {
            plugin_id: None,
            name: None,
            payload: None,
            args: None,
            plugin: None,
            configuration: None,
            launch_new: false,
            skip_cache: false,
            floating: None,
            in_place: None,
            cwd: None,
            pane_title: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::KeybindPipe {
            plugin_id: Some(112),
            name: Some("pipe_name".to_owned()),
            payload: Some("pipe_payload".to_owned()),
            args: Some(demo_context.clone()),
            plugin: Some("zellij:status-bare".to_owned()),
            configuration: Some(demo_context.to_owned()),
            launch_new: true,
            skip_cache: true,
            floating: Some(true),
            in_place: Some(false),
            cwd: Some(PathBuf::from("/path/to/cwd")),
            pane_title: Some("pane_title".to_owned()),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ListClients,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePanePinned,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StackPanes { pane_ids: vec![] },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StackPanes {
            pane_ids: vec![
                PaneId::Terminal(0),
                PaneId::Plugin(1),
                PaneId::Terminal(2),
                PaneId::Plugin(3)
            ],
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ChangeFloatingPaneCoordinates {
            pane_id: PaneId::Terminal(0),
            coordinates: FloatingPaneCoordinates::new(
                None,
                None,
                None,
                Some("10%".to_owned()),
                Some(false),
                Some(false),
            )
            .unwrap(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ChangeFloatingPaneCoordinates {
            pane_id: PaneId::Terminal(0),
            coordinates: FloatingPaneCoordinates::new(
                None,
                None,
                None,
                Some("10%".to_owned()),
                Some(false),
                Some(true),
            )
            .unwrap(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneInGroup,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleGroupMarking,
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneAgent {
            pane_id: PaneId::Terminal(3),
            agent: Some(AgentInfo {
                kind: "claude".to_owned(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: Some("Bash cargo".to_owned()),
                waiting_on: Some("Bash cargo".to_owned()),
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
            }),
        },
        terminal_id: None,
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Reconfigure {
            config: "hooks {\n    agent_exit \"\"\n}\n".to_owned(),
            write_to_disk: true,
        },
        terminal_id: None,
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneAgent {
            pane_id: PaneId::Terminal(3),
            agent: None,
        },
        terminal_id: None,
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneColor {
            pane_id: PaneId::Terminal(0),
            fg: Some("#00e000".to_owned()),
            bg: Some("#001a3a".to_owned()),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneColor {
            pane_id: PaneId::Plugin(2),
            fg: None,
            bg: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageDown,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageDown,
            key_modifiers: demo_modifiers_1,
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageDown,
            key_modifiers: demo_modifiers_2,
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageDown,
            key_modifiers: demo_modifiers_3,
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageDown,
            key_modifiers: demo_modifiers_4,
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PageUp,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Down,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Up,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Right,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Home,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::End,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Backspace,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Delete,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Insert,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Tab,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Esc,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Enter,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::CapsLock,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::ScrollLock,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::NumLock,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::PrintScreen,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Pause,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Menu,
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(1),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(2),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(3),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(4),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(5),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(6),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(7),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(8),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(9),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(10),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(11),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::F(12),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Key {
        key: KeyWithModifier {
            bare_key: BareKey::Char('a'),
            key_modifiers: BTreeSet::new(),
        },
        raw_bytes: "raw_bytes".as_bytes().to_vec(),
        is_kitty_keyboard_protocol: false,
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToEvents {
        event_types: vec![],
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToEvents {
        event_types: vec!["PaneUpdate".to_owned(), "cwd_changed".to_owned()],
    });
    test_client_roundtrip!(ClientToServerMsg::ClientExited);
    test_client_roundtrip!(ClientToServerMsg::KillSession);
    test_client_roundtrip!(ClientToServerMsg::ConnStatus);
    test_client_roundtrip!(ClientToServerMsg::RequestSessionList);
    test_client_roundtrip!(ClientToServerMsg::HostTerminalFocusChanged { focused: true });
    test_client_roundtrip!(ClientToServerMsg::HostTerminalFocusChanged { focused: false });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![PaneId::Terminal(1), PaneId::Plugin(2)],
        scrollback: Some(100),
        ansi: false,
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![PaneId::Terminal(0)],
        scrollback: None,
        ansi: false,
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![PaneId::Terminal(1)],
        scrollback: Some(0),
        ansi: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpByPaneId {
            pane_id: PaneId::Plugin(5),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDownByPaneId {
            pane_id: PaneId::Terminal(2),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToTopByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToBottomByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollUpByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollDownByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollUpByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollDownByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ResizeByPaneId {
            pane_id: PaneId::Terminal(1),
            resize: Resize::Increase,
            direction: Some(Direction::Left),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ResizeByPaneId {
            pane_id: PaneId::Terminal(1),
            resize: Resize::Decrease,
            direction: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneByPaneId {
            pane_id: PaneId::Terminal(3),
            direction: Some(Direction::Up),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneByPaneId {
            pane_id: PaneId::Terminal(3),
            direction: None,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneBackwardsByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClearScreenByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollbackByPaneId {
            pane_id: PaneId::Terminal(1),
            ansi: false,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path".to_owned()),
            include_scrollback: true,
            pane_id: None,
            ansi: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollback { ansi: true },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollbackByPaneId {
            pane_id: PaneId::Terminal(1),
            ansi: true,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusFullscreenByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusNoUiFullscreenByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneEmbedOrFloatingByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseFocusByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: Some(PaneId::Terminal(1)),
            name: "test-name".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenamePaneByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePanePinnedByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPaneByPaneId {
            pane_id: PaneId::Terminal(1),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenameTabByTabId { id: 5 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleActiveSyncTabByTabId { id: 2 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFloatingPanesByTabId { id: 0 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PreviousSwapLayoutByTabId { id: 1 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NextSwapLayoutByTabId { id: 3 },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveTabByTabId {
            id: 1,
            direction: Direction::Left,
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::Full),
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::Titles),
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::None),
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::AttachWatcherClient {
        terminal_size: Size { rows: 24, cols: 80 },
    });
    test_client_roundtrip!(ClientToServerMsg::AttachWatcherClient {
        terminal_size: Size { rows: 0, cols: 0 },
    });
    test_client_roundtrip!(ClientToServerMsg::DesktopNotificationResponse { raw_bytes: vec![] });
    // as with ForwardedReplyFromHost below, the payload is raw terminal bytes and must survive
    // protobuf's string-vs-bytes distinction unchanged
    test_client_roundtrip!(ClientToServerMsg::DesktopNotificationResponse {
        raw_bytes: (0u8..=255u8).collect(),
    });
    test_client_roundtrip!(ClientToServerMsg::ForwardedReplyFromHost {
        token: 0,
        reply_bytes: vec![],
    });
    test_client_roundtrip!(ClientToServerMsg::ForwardedReplyFromHost {
        token: 42,
        reply_bytes: b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
    });
    test_client_roundtrip!(ClientToServerMsg::ForwardedReplyFromHost {
        token: u32::MAX,
        reply_bytes: (0u8..=255u8).collect(),
    });
    // Guard against protobuf string-vs-bytes encoding regressions: a
    // payload mixing NULs, ESC, and CSI framing must round-trip
    // byte-for-byte or the pane will receive corrupt terminal data.
    test_client_roundtrip!(ClientToServerMsg::ForwardedReplyFromHost {
        token: 17,
        reply_bytes: vec![
            0x00, 0x1b, b'[', b'?', b'6', b'2', b';', b'1', b';', b'6', b'c', 0x00, 0x1b, b']',
            b'1', b'1', b';', b'r', b'g', b'b', b':', b'f', b'f', b'f', b'f', b'/', b'f', b'f',
            b'f', b'f', b'/', b'f', b'f', b'f', b'f', 0x07, 0xff, 0x00,
        ],
    });
    test_client_roundtrip!(ClientToServerMsg::HostTerminalThemeChanged {
        mode: HostTerminalThemeMode::Dark,
    });
    test_client_roundtrip!(ClientToServerMsg::HostTerminalThemeChanged {
        mode: HostTerminalThemeMode::Light,
    });
    test_client_roundtrip!(ClientToServerMsg::NestedSessionFrameFromHost {
        payload_bytes: vec![],
    });
    test_client_roundtrip!(ClientToServerMsg::NestedSessionFrameFromHost {
        payload_bytes: (0u8..=255u8).collect(),
    });
}

fn test_server_messages() {
    test_server_roundtrip!(ServerToClientMsg::Render {
        content: "Hello, World!".to_string(),
    });
    test_server_roundtrip!(ServerToClientMsg::Render {
        content: "".to_string(),
    });
    test_server_roundtrip!(ServerToClientMsg::Render {
        content: "x".repeat(10000),
    });
    test_server_roundtrip!(ServerToClientMsg::UnblockInputThread);
    test_server_roundtrip!(ServerToClientMsg::Connected);
    test_server_roundtrip!(ServerToClientMsg::QueryTerminalSize);
    test_server_roundtrip!(ServerToClientMsg::ConfigFileUpdated);
    test_server_roundtrip!(ServerToClientMsg::RenamedSession {
        name: "my-session".to_string(),
    });
    test_server_roundtrip!(ServerToClientMsg::UnblockCliPipeInput {
        pipe_name: "stdout".to_string(),
    });
    test_server_roundtrip!(ServerToClientMsg::CliPipeOutput {
        pipe_name: "stderr".to_string(),
        output: "Error occurred\n".to_string(),
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Error("Something went wrong".to_string()),
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::NormalDetached,
    });
    // A blocking pane's status rides in the message payload: both signs, and zero, have to
    // survive the trip so `zellij run --block-until-exit` exits with what its command did.
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::CustomExitStatus(42),
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::CustomExitStatus(0),
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::CustomExitStatus(-1),
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::ForceDetached,
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::CannotAttach,
    });
    test_server_roundtrip!(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Disconnect,
    });
    test_server_roundtrip!(ServerToClientMsg::Log { lines: vec![] });
    test_server_roundtrip!(ServerToClientMsg::Log {
        lines: vec![
            "Starting server...".to_string(),
            "Server ready on port 8080".to_string(),
            "Accepting connections".to_string(),
        ],
    });
    test_server_roundtrip!(ServerToClientMsg::LogError { lines: vec![] });
    test_server_roundtrip!(ServerToClientMsg::LogError {
        lines: vec![
            "ERROR: Failed to bind socket".to_string(),
            "ERROR: Retrying in 5 seconds".to_string(),
        ],
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: ConnectToSession::default(),
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: ConnectToSession {
            name: Some("new_session_name".to_owned()),
            tab_position: Some(5),
            pane_id: Some((5, true)),
            layout: Some(LayoutInfo::BuiltIn("compact".to_owned())),
            cwd: Some(PathBuf::from("/path/to/cwd")),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: ConnectToSession {
            name: Some("new_session_name".to_owned()),
            tab_position: Some(5),
            pane_id: Some((5, true)),
            layout: Some(LayoutInfo::File(
                "/path/to/my/file.kdl".to_owned(),
                LayoutMetadata {
                    tabs: vec![],
                    creation_time: "0".to_owned(),
                    update_time: "0".to_owned()
                }
            )),
            cwd: Some(PathBuf::from("/path/to/cwd")),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: ConnectToSession {
            name: Some("new_session_name".to_owned()),
            tab_position: Some(5),
            pane_id: Some((5, true)),
            layout: Some(LayoutInfo::Url("https://example.com/layout.kdl".to_owned())),
            cwd: Some(PathBuf::from("/path/to/cwd")),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: ConnectToSession {
            name: Some("new_session_name".to_owned()),
            tab_position: Some(5),
            pane_id: Some((5, true)),
            layout: Some(LayoutInfo::Stringified(
                "layout { pane; pane; pane; }".to_owned()
            )),
            cwd: Some(PathBuf::from("/path/to/cwd")),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::EventUpdate {
        event_type: "CwdChanged".to_owned(),
        event_json: r#"{"CwdChanged":[{"Terminal":3},"/tmp",[1]]}"#.to_owned(),
    });
    test_server_roundtrip!(ServerToClientMsg::EventUpdate {
        event_type: String::new(),
        event_json: String::new(),
    });
    test_server_roundtrip!(ServerToClientMsg::PaneRenderUpdate {
        pane_id: PaneId::Terminal(1),
        viewport: vec!["hello".to_string(), "world".to_string()],
        scrollback: Some(vec!["line1".to_string()]),
        is_initial: true,
    });
    test_server_roundtrip!(ServerToClientMsg::PaneRenderUpdate {
        pane_id: PaneId::Plugin(2),
        viewport: vec!["viewport line".to_string()],
        scrollback: None,
        is_initial: false,
    });
    test_server_roundtrip!(ServerToClientMsg::PaneRenderUpdate {
        pane_id: PaneId::Terminal(0),
        viewport: vec![],
        scrollback: Some(vec![]),
        is_initial: true,
    });
    test_server_roundtrip!(ServerToClientMsg::SubscribedPaneClosed {
        pane_id: PaneId::Terminal(1),
    });
    test_server_roundtrip!(ServerToClientMsg::SubscribedPaneClosed {
        pane_id: PaneId::Plugin(3),
    });
    test_server_roundtrip!(ServerToClientMsg::ForwardQueryToHost {
        token: 0,
        query_bytes: vec![],
        resolve_async: false,
    });
    test_server_roundtrip!(ServerToClientMsg::ForwardQueryToHost {
        token: 7,
        query_bytes: b"\x1b[14t".to_vec(),
        resolve_async: false,
    });
    test_server_roundtrip!(ServerToClientMsg::ForwardQueryToHost {
        token: u32::MAX,
        query_bytes: (0u8..=255u8).collect(),
        resolve_async: false,
    });
    test_server_roundtrip!(ServerToClientMsg::ForwardQueryToHost {
        token: 12,
        query_bytes: b"\x1b]52;c;?\x1b\\".to_vec(),
        resolve_async: true,
    });
    test_server_roundtrip!(ServerToClientMsg::EmitNestedSessionFrame {
        payload_bytes: vec![],
    });
    test_server_roundtrip!(ServerToClientMsg::EmitNestedSessionFrame {
        payload_bytes: (0u8..=255u8).collect(),
    });
}

#[test]
fn set_pane_color_wire_roundtrip() {
    use prost::Message;

    let original = ClientToServerMsg::Action {
        action: Action::SetPaneColor {
            pane_id: PaneId::Terminal(0),
            fg: Some("#00e000".to_owned()),
            bg: Some("#001a3a".to_owned()),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    };

    let proto: crate::client_server_contract::client_server_contract::ClientToServerMsg = original
        .clone()
        .try_into()
        .expect("Failed to convert to protobuf");

    let bytes = proto.encode_to_vec();

    let decoded_proto =
        crate::client_server_contract::client_server_contract::ClientToServerMsg::decode(
            &bytes[..],
        )
        .expect("Failed to decode protobuf bytes");

    let roundtrip: ClientToServerMsg = decoded_proto
        .try_into()
        .expect("Failed to convert decoded protobuf back to Rust");

    assert_eq!(original, roundtrip);
}

/// Tests that RenamePaneByPaneId with `pane_id: None` (the active-pane CLI
/// rename case) survives a full wire-level round-trip
/// (Rust → proto struct → bytes → proto struct → Rust).
///
/// The struct-level round-trip in `test_client_roundtrip!` doesn't catch
/// issues with prost's `tags` attribute on oneof fields, because prost
/// only uses the tags list during byte decoding, not struct-to-struct
/// conversion.
#[test]
fn rename_active_pane_wire_roundtrip() {
    use prost::Message;

    let original = ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "x".as_bytes().to_vec(),
        },
        terminal_id: Some(1),
        client_id: Some(100),
        is_cli_client: true,
    };

    // Rust → proto struct
    let proto: crate::client_server_contract::client_server_contract::ClientToServerMsg = original
        .clone()
        .try_into()
        .expect("Failed to convert to protobuf");

    let mut buf = Vec::new();
    proto.encode(&mut buf).expect("Failed to encode protobuf");
    let decoded =
        crate::client_server_contract::client_server_contract::ClientToServerMsg::decode(&buf[..])
            .expect("Failed to decode protobuf bytes");

    let roundtrip: ClientToServerMsg = decoded
        .try_into()
        .expect("Failed to convert decoded protobuf back to Rust");

    assert_eq!(original, roundtrip);
}

/// The length prefix is read straight off the wire and used as an allocation
/// size, so without a bound a peer claiming four gigabytes gets four gigabytes.
/// Nothing legitimate comes close: the largest real message is a warped pane's
/// scrollback, which the sender caps far below this.
#[test]
fn an_oversized_frame_is_refused_without_allocating() {
    use crate::ipc::MAX_IPC_FRAME_LEN;
    use std::io::Read;

    /// Yields the length prefix and then refuses to produce a single byte more,
    /// so a reader that honours the limit returns before reading the body and one
    /// that does not fails here rather than by exhausting memory.
    struct LyingLength {
        prefix: std::io::Cursor<Vec<u8>>,
    }

    impl Read for LyingLength {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.prefix.read(buf)?;
            if n == 0 {
                panic!("the reader tried to read a body it had been told was oversized");
            }
            Ok(n)
        }
    }

    let claimed = (MAX_IPC_FRAME_LEN + 1) as u32;
    let mut reader = LyingLength {
        prefix: std::io::Cursor::new(claimed.to_le_bytes().to_vec()),
    };
    let result = crate::ipc::read_protobuf_message_for_test(&mut reader);
    assert!(
        result.is_err(),
        "a frame claiming {claimed} bytes should have been refused"
    );
}

/// Every variant, every case: the kinds are walked rather than sampled, so a variant the wire
/// mishandles cannot hide behind an unlucky draw. The generator has no wildcard arm, so a new
/// variant does not compile until it is covered here.
#[hegel::test]
fn every_action_survives_the_client_server_round_trip(tc: hegel::TestCase) {
    use crate::client_server_contract::client_server_contract::Action as ProtoAction;
    use crate::input::actions::ActionKind;
    use crate::plugin_api::test_generators::action_of_kind;
    use prost::Message;
    use strum::IntoEnumIterator;

    for kind in ActionKind::iter() {
        let action = action_of_kind(&tc, kind);
        let protobuf: ProtoAction = action
            .clone()
            .try_into()
            .unwrap_or_else(|e| panic!("{action:?} failed to encode: {e:?}"));
        let bytes = protobuf.encode_to_vec();
        let decoded: ProtoAction = Message::decode(bytes.as_slice())
            .unwrap_or_else(|e| panic!("{action:?} failed to decode: {e}"));
        let round_tripped: Action = decoded
            .try_into()
            .unwrap_or_else(|e| panic!("{action:?} failed to convert back: {e:?}"));
        assert_eq!(
            round_tripped, action,
            "the client-server wire changed the action"
        );
    }
}
