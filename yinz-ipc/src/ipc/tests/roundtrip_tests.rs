use super::test_framework::*;
use crate::ipc::{
    ClientToServerMsg, ColorRegister, ExitReason, PixelDimensions, ServerToClientMsg,
};
use prost::Message;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use strum::VariantNames;
use yinz_types::command::PaneTarget;
use yinz_types::data::ClientId;
use yinz_types::geometry::{PercentOrFixed, SplitDirection, SplitSize};
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::{Size, SizeInPixels};
use yinz_types::position::Position;
use yinz_types::style::HostTerminalThemeMode;
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    geometry::{Direction, FloatingPaneCoordinates, Resize},
};
use {
    yinz_types::data::CommandOrPlugin, yinz_types::data::InputMode, yinz_types::data::LayoutInfo,
    yinz_types::data::LayoutMetadata, yinz_types::data::NewPanePlacement,
    yinz_types::data::OriginatingPlugin, yinz_types::data::PaneId, yinz_types::data::PluginTag,
    yinz_types::data::RemotePaneHandoff, yinz_types::data::SessionConnection,
    yinz_types::data::UnblockCondition, yinz_types::data::WarpPlacement,
    yinz_types::data::WarpedPane, yinz_types::data::WarpedTab,
};
use {
    yinz_types::input::actions::Action, yinz_types::input::actions::SearchDirection,
    yinz_types::input::actions::SearchOption,
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
    yinz_types::input::layout::TiledPaneLayout,
};
use {
    yinz_types::input::options::Clipboard, yinz_types::input::options::HostNotificationProtocol,
    yinz_types::input::options::NestedSessionHandling,
    yinz_types::input::options::NotificationPosition, yinz_types::input::options::OnForceClose,
    yinz_types::input::options::Options, yinz_types::input::options::PaneFrameStyle,
};

const ROUNDTRIP_TEST_STACK_SIZE: usize = 32 * 1024 * 1024;

#[test]
fn client_ids_preserve_detach_bytes_and_reject_narrowing() {
    use crate::client_server_contract::client_server_contract::{
        client_to_server_msg, ClientToServerMsg as ProtoClientToServerMsg, DetachSessionMsg,
    };
    use prost::Message;

    let bytes = [0x0a, 0x06, 0x0a, 0x04, 0x00, 0xff, 0xff, 0x03];
    let proto = ProtoClientToServerMsg::decode(bytes.as_slice()).unwrap();
    let message = ClientToServerMsg::try_from(proto).unwrap();
    assert_eq!(
        message,
        ClientToServerMsg::DetachSession {
            client_ids: vec![ClientId::new(0), ClientId::new(u16::MAX)],
        }
    );
    assert_eq!(
        ProtoClientToServerMsg::try_from(message)
            .unwrap()
            .encode_to_vec(),
        bytes
    );

    for id in [65536, u32::MAX] {
        let proto = ProtoClientToServerMsg {
            message: Some(client_to_server_msg::Message::DetachSession(
                DetachSessionMsg {
                    client_ids: vec![id],
                },
            )),
        };
        assert!(ClientToServerMsg::try_from(proto).is_err());
    }
}

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

/// `yinz plugin -- file:keys.ts` is parsed in the client and loaded in the server, so the name
/// as the user wrote it has to survive this hop - without it the server can only look for
/// `$cwd/keys.ts`, and the user's plugin folder is never searched.
///
/// Asserted on the field rather than through `test_client_roundtrip!`, because `RunPlugin`'s
/// `PartialEq` deliberately ignores `relative_path`: a whole-message comparison would pass just
/// as happily with the field dropped.
#[test]
fn git_plugins_survive_the_client_server_wire() {
    use crate::client_server_contract::client_server_contract::RunPluginLocationData;
    let original = RunPluginLocation::Git(yinz_types::input::git_plugin::GitPlugin {
        repo: "https://example.com/plugin.git".into(),
        entrypoint: "src/main.ts".into(),
        revision: Some(yinz_types::input::git_plugin::GitRevision::Branch(
            "main".into(),
        )),
    });
    let protobuf: RunPluginLocationData = original.clone().into();
    assert_eq!(RunPluginLocation::try_from(protobuf).unwrap(), original);
}

#[test]
fn a_relative_plugin_path_survives_the_client_server_contract() {
    use crate::client_server_contract::client_server_contract::RunPlugin as ProtoRunPlugin;

    let original = RunPlugin {
        location: RunPluginLocation::File(PathBuf::from("/somewhere/keys.ts")),
        relative_path: Some(PathBuf::from("keys.ts")),
        declared_path: Some(PathBuf::from("/somewhere/current.ts")),
        ..Default::default()
    };
    let protobuf: ProtoRunPlugin = original.clone().into();
    let decoded: RunPlugin = protobuf.try_into().expect("decode");

    assert_eq!(decoded.relative_path, original.relative_path);
    assert_eq!(decoded.declared_path, original.declared_path);
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
            agent_theme_json: None,
            bracketed_paste: false,
            agent_input: None,
            held: None,
            group: None,
            host_id: String::new(),
            control_socket: String::new(),
            title: None,
            caption: None,
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
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    })
    .map(|cmd| cmd.into());
    match Action::try_from(adopt_pane(cmd)) {
        Ok(Action::AdoptPane { payload, .. }) => {
            let remote = payload.remote.expect("the handoff should have survived");
            assert_eq!(remote.host, "build-box");
            assert_eq!(remote.cmd.target.ssh_host(), Some("build-box"));
        },
        other => panic!("expected an AdoptPane, got {other:?}"),
    }
}

fn test_client_messages() {
    use yinz_types::agent::control::AgentRequest;
    use {yinz_types::agent::report::AgentReport, yinz_types::agent::report::AgentReportState};
    let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7));
    for expected_session_id in [None, Some("conversation-7".to_owned())] {
        for request in [
            AgentRequest::List { project: None },
            AgentRequest::Status {
                pane_id,
                expected_session_id: expected_session_id.clone(),
            },
            AgentRequest::Spawn {
                placement: Default::default(),

                spec: yinz_types::agent::AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some("review-auth".into()),
                    ..Default::default()
                },
                cwd: Some("/repo".into()),
                project: None,
                remote_host: None,
            },
            AgentRequest::Prompt {
                preserve_draft: false,
                submission_id: None,
                pane_id,
                expected_session_id: expected_session_id.clone(),
                prompt: "Review this diff:\n+added line".into(),
                wait: true,
                timeout_ms: 600_000,
            },
            AgentRequest::Wait {
                pane_id,
                expected_session_id: expected_session_id.clone(),
                until: AgentState::Idle,
                timeout_ms: 30_000,
            },
            AgentRequest::WaitTurn {
                pane_id,
                expected_session_id: expected_session_id.clone(),
                submission_id: "submission-7".into(),
                timeout_ms: 30_000,
            },
            AgentRequest::Read {
                pane_id,
                expected_session_id: expected_session_id.clone(),
                lines: 80,
            },
            AgentRequest::ClearDraft {
                pane_id,
                expected_session_id: "conversation-7".into(),
                input_revision: 42,
                confirm_discard: true,
                interrupted: true,
                timeout_ms: 30_000,
            },
            AgentRequest::Recover {
                pane_id,
                expected_session_id: "conversation-7".into(),
                confirm_empty: true,
                interrupted: true,
            },
            AgentRequest::SendKeys {
                pane_id,
                expected_session_id: expected_session_id.clone(),
                keys: vec![KeyWithModifier::new(BareKey::Esc)],
            },
        ] {
            test_client_roundtrip!(ClientToServerMsg::AgentControl(request));
        }
    }
    for state in [
        AgentReportState::Unchanged,
        AgentReportState::Clear,
        AgentReportState::Set(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
    ] {
        let hook: yinz_types::agent::HookEvent =
            serde_json::from_str(r#"{"hook_event_name":"PreToolUse"}"#).unwrap();
        for event in [
            None,
            Some(yinz_types::agent::AgentEvent::from_hook(
                "codex",
                yinz_types::internal_ids::TerminalId::new(7),
                &hook,
                "{}".into(),
            )),
        ] {
            test_client_roundtrip!(ClientToServerMsg::AgentReport(AgentReport {
                report_id: "00000000-0000-4000-8000-000000000001".into(),
                pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
                state: state.clone(),
                event,
                trace: true,
                process_id: ProcessId::new(42),
                kind: "claude".into(),
            }));
        }
    }

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
        client_ids: vec![ClientId::new(1)],
    });
    test_client_roundtrip!(ClientToServerMsg::DetachSession {
        client_ids: vec![ClientId::new(1), ClientId::new(2), ClientId::new(999)],
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
    test_client_roundtrip!(ClientToServerMsg::SynchronizedOutputSupport { supported: true });
    test_client_roundtrip!(ClientToServerMsg::SynchronizedOutputSupport { supported: false });
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
                simplified_ui: Some(true),
                theme: Some("theme".to_owned()),
                theme_dark: Some("theme_dark".to_owned()),
                theme_light: Some("theme_light".to_owned()),
                explicit_theme_hue: Some(yinz_types::data::ThemeHue::Light),
                default_mode: Some(InputMode::Normal),
                default_shell: Some(PathBuf::from("default_shell")),
                default_cwd: Some(PathBuf::from("default_cwd")),
                default_layout: Some(PathBuf::from("default_layout")),
                layout_dir: Some(PathBuf::from("layout_dir")),
                builtin_plugins_dir: Some(PathBuf::from("builtin_plugins_dir")),
                theme_dir: Some(PathBuf::from("theme_dir")),
                user_plugin_dir: Some(PathBuf::from("user_plugin_dir")),
                mouse_mode: Some(true),
                agent_double_escape: Some(true),
                // Intentionally not carried over the wire; see the field's docs.
                detachable_panes: None,
                host_id: None,

                pane_frames: Some(true),
                pane_animations: Some(false),
                pane_frame_style: Some(PaneFrameStyle::Full),
                notification_position: Some(NotificationPosition::BottomLeft),
                notification_timeout: Some(yinz_types::units::ElapsedSeconds::new(9)),
                mirror_session: Some(true),
                on_force_close: Some(OnForceClose::Quit),
                scroll_buffer_size: Some(100000),
                max_agent_width: Some(100),
                copy_command: Some("copy_command".to_owned()),
                copy_clipboard: Some(Clipboard::System),
                copy_on_select: Some(true),
                osc8_hyperlinks: Some(true),
                scrollback_editor: Some(PathBuf::from("scrollback_editor")),
                session_name: Some("session_name".to_owned().into()),
                attach_to_session: Some(true),
                auto_layout: Some(true),
                session_serialization: Some(true),
                serialize_pane_viewport: Some(true),
                scrollback_lines_to_serialize: Some(10000),
                styled_underlines: Some(true),
                background_opacity: Some(85),
                serialization_interval: Some(yinz_types::units::ElapsedSeconds::new(1)),
                locked_escape_timeout_ms: Some(yinz_types::units::ElapsedMillis::new(500)),
                disable_session_metadata: Some(true),
                support_kitty_keyboard_protocol: Some(true),
                report_modifier_keys: Some(true),
                support_kitty_graphics_protocol: Some(false),
                stacked_resize: Some(true),
                agent_pane_min_columns: Some(100),
                agent_pane_min_rows: Some(30),
                stacked_pane_list: Some(true),
                advanced_mouse_actions: Some(true),
                mouse_scroll_resize: Some(true),
                post_command_discovery_hook: Some("post_command_discovery_hook".to_owned()),
                mouse_hover_effects: Some(false),
                visual_bell: Some(true),
                focus_follows_mouse: Some(false),
                mouse_click_through: Some(false),
                context_menu_on_right_click: Some(true),
                context_menu_override_agent_mouse: Some(true),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
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
        tab_number_to_focus: None,
        pane_to_focus: None,
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_number_to_focus: Some(TabNumber::new(0)),
        pane_to_focus: None,
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_number_to_focus: None,
        pane_to_focus: Some(PaneId::from_legacy(0, false)),
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets::default(),
        tab_number_to_focus: None,
        pane_to_focus: Some(PaneId::from_legacy(100, true)),
    });
    test_client_roundtrip!(ClientToServerMsg::AttachClient {
        cli_assets: CliAssets {
            configuration_options: Some(Options {
                agent_theme_mapping: Some(yinz_types::agent::theme::AgentThemeMapping::Off),
                nested_session_handling: Some(NestedSessionHandling::Descend),
                ..Default::default()
            }),
            host_terminal_env: [("TERM".to_owned(), "xterm-256color".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        tab_number_to_focus: None,
        pane_to_focus: None,
    });
    for first_run in [true, false] {
        let pane = WarpedPane {
            agent_theme: Some(yinz_types::agent::theme::AgentThemeHandoff {
                version: 1,
                snapshots: vec![yinz_types::agent::theme::AgentThemeSnapshot {
                    version: 1,
                    provider: "claude".into(),
                    provider_version: "fixture".into(),
                    theme: "dark".into(),
                    provenance: vec![],
                    availability: yinz_types::agent::theme::Availability::Available,
                    definitions: vec![],
                    collisions: vec![],
                }],
                associations: vec![yinz_types::agent::theme::PaletteAssociation {
                    offset: 0,
                    palette: Some(0),
                }],
                active: Some(yinz_types::agent::theme::HandoffLaunch {
                    launch_id: "launch".into(),
                    palette: 0,
                }),
            }),
            held: Some(yinz_types::data::HeldTerminal {
                command: yinz_types::input::command::RunCommand {
                    command: "/usr/bin/true".into(),
                    cwd: Some("/tmp".into()),
                    ..Default::default()
                },
                first_run,
                exit_status: (!first_run).then_some(17),
            }),
            title: Some("saved task".into()),
            contents: Some("saved scrollback".into()),
            ..Default::default()
        };
        test_client_roundtrip!(ClientToServerMsg::Action {
            action: Action::AdoptPane {
                payload: Box::new(pane.clone()),
                placement: WarpPlacement::FocusedTab
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        });
        test_client_roundtrip!(ClientToServerMsg::Action {
            action: Action::AdoptTab {
                payload: Box::new(WarpedTab {
                    tiled_panes: vec![pane],
                    ..Default::default()
                })
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        });
    }
    // --- Action ---
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Quit,
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Quit,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                to_session: "other".to_owned().into(),
                placement,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        });
        test_client_roundtrip!(ClientToServerMsg::Action {
            action: Action::WarpPane {
                pane_id: Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                    3
                ))),
                to_session: "other".to_owned().into(),
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
                agent_theme: None,
                bracketed_paste: true,
                agent_input: Some(yinz_types::data::WarpedAgentInput {
                    kind: "codex".into(),
                    conversation: "conversation".into(),
                    consumed: true
                }),
                held: None,
                group: None,
                host_id: SupervisorId::new("a3f1c2d4e5b6".to_owned()),
                control_socket: PathBuf::from("/run/user/1000/yinz/pty/a3f1c2d4e5b6.sock"),
                title: None,
                caption: None,
                contents: None,
                borderless: false,
                remote: None,
                agent: Some(AgentInfo {
                    kind: "codex".to_owned(),
                    state: AgentState::Waiting,
                    source: AgentSource::Reported,
                    detail: None,
                    waiting_on: Some("approval".to_owned()),
                    wait_needs_a_person: true,
                    cwd: Some("/work".to_owned()),
                    session_id: Some("agent-session".to_owned()),
                    // The whole point of the field: a warp is where a run id has to survive
                    // the wire, or the adopting session cannot find the run it names.
                    run_id: Some("codex:agent-session".to_owned()),
                }),
            }),
            placement: WarpPlacement::NewTab,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    // Adopting a single pane whose process is on another machine. This is the
    // exact shape `yinz pty-host adopt --host <machine> <token>` builds, and
    // the only one where the pane's whole address is in `remote` — the two local
    // fields are empty because there is no supervisor here to name.
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::AdoptPane {
            payload: Box::new(WarpedPane {
                agent_theme: None,
                bracketed_paste: true,
                agent_input: Some(yinz_types::data::WarpedAgentInput {
                    kind: "codex".into(),
                    conversation: "conversation".into(),
                    consumed: true
                }),
                held: None,
                group: None,
                host_id: SupervisorId::new(String::new()),
                control_socket: PathBuf::new(),
                title: Some("cargo watch".to_owned()),
                caption: None,
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
                        target: PaneTarget::Ssh {
                            host: "build-box".to_owned()
                        },
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
                agent_theme: None,
                bracketed_paste: true,
                agent_input: Some(yinz_types::data::WarpedAgentInput {
                    kind: "codex".into(),
                    conversation: "conversation".into(),
                    consumed: true
                }),
                held: None,
                group: None,
                host_id: SupervisorId::new("a3f1c2d4e5b6".to_owned()),
                control_socket: PathBuf::from("/run/user/1000/yinz/pty/a3f1c2d4e5b6.sock"),
                title: Some("vim src/main.rs".to_owned()),
                caption: None,
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
            tab_id: None,
            to_session: "other".to_owned().into(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WarpTab {
            tab_id: Some(TabId::new(2)),
            to_session: "other".to_owned().into(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WarpTabFrom {
            from_session: "other".to_owned().into(),
            tab_id: TabId::new(2),
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
                relationships: None,
                name: Some("build".to_owned()),
                user_named: true,
                tiled_layout: TiledPaneLayout {
                    children_split_direction: SplitDirection::Vertical,
                    children: vec![
                        TiledPaneLayout {
                            run: Some(Run::Command(RunCommand {
                                agent_theme: None,
                                command: PathBuf::from("vim"),
                                args: vec!["src/main.rs".to_owned()],
                                ..Default::default()
                            })),
                            split_size: Some(SplitSize::Percent(60)),
                            ..Default::default()
                        },
                        TiledPaneLayout {
                            run: Some(Run::Command(RunCommand {
                                agent_theme: None,
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
                        agent_theme: None,
                        bracketed_paste: true,
                        agent_input: Some(yinz_types::data::WarpedAgentInput {
                            kind: "codex".into(),
                            conversation: "conversation".into(),
                            consumed: true
                        }),
                        held: None,
                        group: None,
                        host_id: SupervisorId::new("a3f1c2d4e5b6".to_owned()),
                        control_socket: PathBuf::from("/run/user/1000/yinz/pty/a3f1c2d4e5b6.sock"),
                        title: Some("vim src/main.rs".to_owned()),
                        caption: None,
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
                        agent_theme: None,
                        bracketed_paste: true,
                        agent_input: Some(yinz_types::data::WarpedAgentInput {
                            kind: "codex".into(),
                            conversation: "conversation".into(),
                            consumed: true
                        }),
                        held: None,
                        group: None,
                        host_id: SupervisorId::new(String::new()),
                        control_socket: PathBuf::new(),
                        title: Some("cargo watch".to_owned()),
                        caption: None,
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
                                target: PaneTarget::Ssh {
                                    host: "build-box".to_owned()
                                },
                                ..Default::default()
                            },
                        }),
                        agent: None,
                    },
                ],
                floating_panes: vec![WarpedPane {
                    agent_theme: None,
                    bracketed_paste: true,
                    agent_input: Some(yinz_types::data::WarpedAgentInput {
                        kind: "codex".into(),
                        conversation: "conversation".into(),
                        consumed: true
                    }),
                    held: None,
                    group: None,
                    host_id: SupervisorId::new("c5f3e4d6a7b8".to_owned()),
                    control_socket: PathBuf::from("/run/user/1000/yinz/pty/c5f3e4d6a7b8.sock"),
                    title: Some("notes".to_owned()),
                    caption: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Write {
            key_with_modifier: Some(KeyWithModifier::new(BareKey::Char('a'))),
            bytes: "a".as_bytes().to_vec(),
            is_kitty_keyboard_protocol: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::WriteChars {
            chars: "my chars".to_owned(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchToMode {
            input_mode: InputMode::Locked,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchModeForAllClients {
            input_mode: InputMode::Locked,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Increase,
            direction: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Left),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Right),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Up),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Resize {
            resize: Resize::Decrease,
            direction: Some(Direction::Down),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusNextPane,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPreviousPane,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SwitchFocus,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveFocus {
            direction: Direction::Up,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveFocusOrTab {
            direction: Direction::Up,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePane { direction: None },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePane {
            direction: Some(Direction::Up),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneBackwards,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClearScreen,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: true,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path/to/file".to_owned()),
            include_scrollback: true,
            pane_id: Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                5
            ))),
            ansi: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: None,
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpLayout,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollback { ansi: false },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUp,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpAt {
            position: Position::new(0, 0),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpAt {
            position: Position::new(-10, 0),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDown,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDownAt {
            position: Position::new(0, 0),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToBottom,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToTop,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToPreviousPrompt,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToNextPrompt,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SelectCommandAtScrollPosition,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CopyLastCommandOutput,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollUp,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollDown,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollUp,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollDown,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusFullscreen,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusNoUiFullscreen,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleCommandPalette,
        terminal_id: None,
        client_id: Some(ClientId::new(100)),
        is_cli_client: false,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneFrames,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleActiveSyncTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewPane {
            direction: Some(Direction::Right),
            pane_name: Some("pane_name".to_owned()),
            start_suppressed: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditFile {
            payload: OpenFilePayload {
                path: PathBuf::from("/file/path"),
                line_number: Some(1),
                cwd: Some(PathBuf::from("/my/cwd")),
                originating_plugin: Some(OriginatingPlugin {
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(11),
                    client_id: ClientId::new(22),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                target: Default::default(),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(1),
                    client_id: ClientId::new(2),
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                target: Default::default(),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(1),
                    client_id: ClientId::new(2),
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                target: Default::default(),
                remote_default_shell: false,
            }),
            direction: Some(Direction::Right),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            borderless: Some(true),
            tab_id: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(1),
                    client_id: ClientId::new(2),
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                target: Default::default(),
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane: false,
            tab_id: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                    deno_generation: Some(123),
                    plugin_id: yinz_types::internal_ids::PluginId::new(1),
                    client_id: ClientId::new(2),
                    context: demo_context.clone(),
                }),
                use_terminal_title: false,
                target: Default::default(),
                remote_default_shell: false,
            }),
            pane_name: Some("my_pane_name".to_owned()),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneEmbedOrFloating,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFloatingPanes,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseFocus,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PaneNameInput {
            input: "name input".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenamePane,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "spark".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "x".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: Some(PathBuf::from("custom-layout")),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    location: RunPluginLocation::Yinz(PluginTag::new("my_plugin_tag")),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    agent_theme: None,
                    command: PathBuf::from("/path/to/command"),
                    args: vec![],
                    cwd: None,
                    hold_on_close: false,
                    hold_on_start: false,
                    originating_plugin: None,
                    use_terminal_title: true,
                    target: Default::default(),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
            tiled_layout: Some(TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    agent_theme: None,
                    command: PathBuf::from("/path/to/command"),
                    args: vec!["arg1".to_owned(), "arg2".to_owned(), "arg3".to_owned()],
                    cwd: Some(PathBuf::from("/path/to/cwd")),
                    hold_on_close: true,
                    hold_on_start: true,
                    originating_plugin: Some(OriginatingPlugin {
                        deno_generation: Some(123),
                        plugin_id: yinz_types::internal_ids::PluginId::new(11),
                        client_id: ClientId::new(22),
                        context: empty_context.clone(),
                    }),
                    use_terminal_title: true,
                    target: Default::default(),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
            tiled_layout: None,
            floating_layouts: vec![
                FloatingPaneLayout {
                    pane_tabs: None,
                    name: Some("first floating layout".to_owned()),
                    ..Default::default()
                },
                FloatingPaneLayout {
                    pane_tabs: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTab {
            layout_source: None,
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
                    target: Default::default(),
                    remote_default_shell: false,
                }),
                CommandOrPlugin::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin::default())),
            ]),
            first_pane_unblock_condition: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::OverrideLayout {
            tabs: vec![],
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            apply_only_to_active_tab: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NoOp,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToNextTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToPreviousTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTab {
            index: TabNumber::new(0)
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTabName {
            name: "tab_name".to_owned(),
            create: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::GoToTabName {
            name: "tab_name".to_owned(),
            create: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TabNameInput {
            input: "my cool tab_name".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenameTab,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveTab {
            direction: Direction::Right,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
                target: Default::default(),
                remote_default_shell: false,
            },
            near_current_pane: false,
            no_focus: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Detach,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetDarkTheme,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetLightTheme,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleTheme,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Copy,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Confirm,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Deny,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SkipConfirm {
            action: Box::new(Action::Quit)
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchInput {
            input: "my_search_input".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Search {
            direction: SearchDirection::Up,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Search {
            direction: SearchDirection::Down,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::CaseSensitivity,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::WholeWord,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SearchToggleOption {
            option: SearchOption::Wrap,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleMouseMode,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PreviousSwapLayout,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NextSwapLayout,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::QueryTabNames,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(3)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewFloatingPane {
            command: None,
            pane_name: None,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(TabId::new(5)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewStackedPane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: Some(TabId::new(2)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(4)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(7)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(2)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(6)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewTiledPluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: None,
            skip_cache: false,
            cwd: None,
            no_focus: false,
            tab_id: Some(TabId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
            tab_id: Some(TabId::new(3)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NewInPlacePluginPane {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            pane_name: None,
            skip_cache: false,
            close_replaced_pane: false,
            no_focus: false,
            tab_id: Some(TabId::new(2)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StartOrReloadPlugin {
            plugin: RunPluginOrAlias::Alias(PluginAlias::default()),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseTerminalPane {
            pane_id: yinz_types::internal_ids::TerminalId::new(11)
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClosePluginPane {
            pane_id: yinz_types::internal_ids::PluginId::new(12)
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusTerminalPaneWithId {
            pane_id: yinz_types::internal_ids::TerminalId::new(12),
            should_float_if_hidden: false,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusTerminalPaneWithId {
            pane_id: yinz_types::internal_ids::TerminalId::new(12),
            should_float_if_hidden: true,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPluginPaneWithId {
            pane_id: yinz_types::internal_ids::PluginId::new(12),
            should_float_if_hidden: false,
            should_be_in_place_if_hidden: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPluginPaneWithId {
            pane_id: yinz_types::internal_ids::PluginId::new(12),
            should_float_if_hidden: true,
            should_be_in_place_if_hidden: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameTerminalPane {
            pane_id: yinz_types::internal_ids::TerminalId::new(12),
            name: "terminal_pane_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePluginPane {
            pane_id: yinz_types::internal_ids::PluginId::new(12),
            name: "plugin_pane_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameTab {
            tab_index: TabNumber::new(11),
            name: "tab_new_name".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPane,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPaneRight,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::BreakPaneLeft,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenameSession {
            name: "new_session_name".to_owned().into(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CliPipe {
            focus_plugin: false,
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CliPipe {
            focus_plugin: true,
            pipe_id: "pipe_id_name".to_owned(),
            name: Some("pipe_name".to_owned()),
            payload: Some("pipe_payload".to_owned()),
            args: Some(demo_context.clone()),
            plugin: Some("yinz:status-bare".to_owned()),
            configuration: Some(demo_context.to_owned()),
            launch_new: true,
            skip_cache: true,
            floating: Some(true),
            in_place: Some(false),
            cwd: Some(PathBuf::from("/path/to/cwd")),
            pane_title: Some("pane_title".to_owned()),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::KeybindPipe {
            plugin_id: Some(yinz_types::internal_ids::PluginId::new(112)),
            name: Some("pipe_name".to_owned()),
            payload: Some("pipe_payload".to_owned()),
            args: Some(demo_context.clone()),
            plugin: Some("yinz:status-bare".to_owned()),
            configuration: Some(demo_context.to_owned()),
            launch_new: true,
            skip_cache: true,
            floating: Some(true),
            in_place: Some(false),
            cwd: Some(PathBuf::from("/path/to/cwd")),
            pane_title: Some("pane_title".to_owned()),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ListClients,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePanePinned,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StackPanes { pane_ids: vec![] },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::StackPanes {
            pane_ids: vec![
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
                PaneId::Plugin(yinz_types::internal_ids::PluginId::new(1)),
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(2)),
                PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3))
            ],
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ChangeFloatingPaneCoordinates {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ChangeFloatingPaneCoordinates {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
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
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneInGroup,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleGroupMarking,
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneAgent {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3)),
            agent: Some(AgentInfo {
                kind: "claude".to_owned(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: Some("Bash cargo".to_owned()),
                waiting_on: Some("Bash cargo".to_owned()),
                wait_needs_a_person: true,
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
                run_id: Some("claude:s1".to_owned()),
            }),
        },
        terminal_id: None,
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::Reconfigure {
            config: "hooks {\n    agent_exit \"\"\n}\n".to_owned(),
            write_to_disk: true,
        },
        terminal_id: None,
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneAgent {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3)),
            agent: None,
        },
        terminal_id: None,
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneColor {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
            fg: Some("#00e000".to_owned()),
            bg: Some("#001a3a".to_owned()),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneColor {
            pane_id: PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2)),
            fg: None,
            bg: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Paste("café\n東京\t🦀".into()));
    test_client_roundtrip!(ClientToServerMsg::Paste(String::new()));
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
        event_types: vec!["agent_lifecycle".into(), "PaneUpdate".into()],
        options: crate::ipc::EventSubscription {
            panes: vec!["terminal_7".into()],
            projects: vec!["yinz".into()],
            kinds: vec!["codex".into(), "claude".into()],
            lifecycle_types: vec!["turn_completed".into()],
            cursor: Some("saved".into()),
        },
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToEvents {
        options: Default::default(),
        event_types: vec![],
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToEvents {
        options: Default::default(),
        event_types: vec!["PaneUpdate".to_owned(), "cwd_changed".to_owned()],
    });
    test_client_roundtrip!(ClientToServerMsg::PluginRequest {
        request_id: "lifecycle".into(),
        request: yinz_types::cli::PluginRequest {
            operation: yinz_types::cli::PluginOperation::Reload,
            target: Some("plugin_10".into()),
            configuration: Some(BTreeMap::from([("shared".into(), "true".into())])),
            fallback: None,
            candidates: vec![],
            caller: Some(yinz_types::internal_ids::TerminalId::new(7)),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::PluginResponse {
        request_id: "lifecycle".into(),
        result: Ok(yinz_types::cli::PluginResponse { plugins: vec![] })
    });
    test_server_roundtrip!(ServerToClientMsg::PluginResponse {
        request_id: "lifecycle".into(),
        result: Err("not loaded; use plugins start".into())
    });
    test_client_roundtrip!(ClientToServerMsg::DataRequest {
        request_id: "catalog".into(),
        request: yinz_types::cli::DataRequest::Catalog { namespace: None }
    });
    test_client_roundtrip!(ClientToServerMsg::DataRequest {
        request_id: "fetch".into(),
        request: yinz_types::cli::DataRequest::Fetch(
            yinz_types::plugin_data::FetchRequest::entity("item")
                .in_namespace("plugin:file:/test.ts")
        )
    });
    test_client_roundtrip!(ClientToServerMsg::DataRequest {
        request_id: "count".into(),
        request: yinz_types::cli::DataRequest::Count(
            yinz_types::plugin_data::FetchRequest::entity("item").in_namespace("shared:fixture")
        )
    });
    test_client_roundtrip!(ClientToServerMsg::DataRequest {
        request_id: "apply".into(),
        request: yinz_types::cli::DataRequest::Apply {
            namespace: "shared:fixture".into(),
            mutations: vec![yinz_types::plugin_data::Mutation::put("item")
                .set("text", "hello\nworld")
                .build()]
        }
    });
    test_server_roundtrip!(ServerToClientMsg::DataResponse {
        request_id: "reply".into(),
        result: Ok("{\"rows\":[]}".into())
    });
    test_server_roundtrip!(ServerToClientMsg::DataResponse {
        request_id: "reply".into(),
        result: Err("conflict".into())
    });
    test_client_roundtrip!(ClientToServerMsg::ClientExited);
    test_client_roundtrip!(ClientToServerMsg::KillSession);
    test_client_roundtrip!(ClientToServerMsg::KillSessionWithCompletion);
    test_client_roundtrip!(ClientToServerMsg::ConnStatus);
    test_client_roundtrip!(ClientToServerMsg::RequestSessionList);
    test_client_roundtrip!(ClientToServerMsg::HostTerminalFocusChanged { focused: true });
    test_client_roundtrip!(ClientToServerMsg::HostTerminalFocusChanged { focused: false });
    test_client_roundtrip!(ClientToServerMsg::ModifiersHeld {
        modifiers: BTreeSet::new(),
    });
    test_client_roundtrip!(ClientToServerMsg::ModifiersHeld {
        modifiers: [KeyModifier::Alt, KeyModifier::Ctrl].into_iter().collect(),
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2))
        ],
        scrollback: Some(100),
        ansi: false,
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            0
        ))],
        scrollback: None,
        ansi: false,
    });
    test_client_roundtrip!(ClientToServerMsg::SubscribeToPaneRenders {
        pane_ids: vec![PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            1
        ))],
        scrollback: Some(0),
        ansi: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollUpByPaneId {
            pane_id: PaneId::Plugin(yinz_types::internal_ids::PluginId::new(5)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollDownByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(2)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToTopByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ScrollToBottomByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollUpByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PageScrollDownByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollUpByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::HalfPageScrollDownByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ResizeByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            resize: Resize::Increase,
            direction: Some(Direction::Left),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ResizeByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            resize: Resize::Decrease,
            direction: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3)),
            direction: Some(Direction::Up),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3)),
            direction: None,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MovePaneBackwardsByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ClearScreenByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollbackByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            ansi: false,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::DumpScreen {
            file_path: Some("/path".to_owned()),
            include_scrollback: true,
            pane_id: None,
            ansi: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollback { ansi: true },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::EditScrollbackByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            ansi: true,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusFullscreenByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFocusNoUiFullscreenByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePaneEmbedOrFloatingByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::CloseFocusByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                1
            ))),
            name: "test-name".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenamePaneByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::TogglePanePinnedByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::FocusPaneByPaneId {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::UndoRenameTabByTabId { id: TabId::new(5) },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleActiveSyncTabByTabId { id: TabId::new(2) },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::ToggleFloatingPanesByTabId { id: TabId::new(0) },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::PreviousSwapLayoutByTabId { id: TabId::new(1) },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::NextSwapLayoutByTabId { id: TabId::new(3) },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::MoveTabByTabId {
            id: TabId::new(1),
            direction: Direction::Left,
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::Full),
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::Titles),
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
        is_cli_client: true,
    });
    test_client_roundtrip!(ClientToServerMsg::Action {
        action: Action::SetPaneFrameStyle(PaneFrameStyle::None),
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
    use yinz_types::agent::control::{AgentOutcome, AgentResponse, AgentStatus};
    let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7));
    let status = AgentStatus {
        pane_id,
        host_id: "host".into(),
        tab_id: None,
        target: Default::default(),
        session: "development".into(),
        agent: AgentInfo {
            kind: "codex".into(),
            state: AgentState::Waiting,
            source: AgentSource::Reported,
            detail: Some("approval".into()),
            waiting_on: Some("run command".into()),
            wait_needs_a_person: true,
            cwd: Some("/repo/review-auth".into()),
            session_id: Some("conversation-7".into()),
            run_id: Some("run-7".into()),
        },
        cwd: Some("/repo/review-auth".into()),
        branch: Some("review-auth".into()),
        ready: false,
        not_ready_reason: Some(yinz_types::agent::control::NotReadyReason::NotIdle),
        status_available: true,
        prompt_supported: true,
        readiness: None,
        input_revision: None,
        clear_draft_supported: false,
        background_tasks: vec!["bj30txqod".into()],
    };
    for outcome in [
        AgentOutcome::Ok,
        AgentOutcome::Submitted,
        AgentOutcome::Completed,
        AgentOutcome::NeedsInput,
        AgentOutcome::Failed,
        AgentOutcome::Interrupted,
        AgentOutcome::TimedOut,
        AgentOutcome::TargetChanged,
        AgentOutcome::TargetClosed,
        AgentOutcome::StatusUnavailable,
        AgentOutcome::Rejected,
        AgentOutcome::TargetNotFound,
        AgentOutcome::NotAgent,
        AgentOutcome::TargetExited,
    ] {
        test_server_roundtrip!(ServerToClientMsg::AgentControl(AgentResponse::new(outcome)));
        test_server_roundtrip!(ServerToClientMsg::AgentControl(AgentResponse {
            outcome,
            agents: vec![status.clone()],
            spawned: Some(yinz_types::agent::SpawnedAgent {
                pane_id,
                branch: Some("review-auth-2".into()),
                cwd: Some("/repo/review-auth-2".into()),
                requested_branch: Some("review-auth".into()),
                reused: true,
                occupied_by: vec![pane_id],
                tab_id: Some(TabId::new(7)),
                created_tab_id: Some(TabId::new(7)),
            }),
            output: Some("Ready\n>".into()),
            launch_resources: None,
            message: Some("approval required".into()),
            acknowledged: true,
            turn_id: Some("turn-7".into()),
            submission_id: Some("submission-7".into()),
            background_tasks: vec!["bj30txqod".into()],
        }));
    }
    for reason in [
        None,
        Some(yinz_types::agent::control::NotReadyReason::PromptUnsupported),
        Some(yinz_types::agent::control::NotReadyReason::NotIdle),
        Some(yinz_types::agent::control::NotReadyReason::PendingSubmission),
        Some(yinz_types::agent::control::NotReadyReason::UnconsumedInput),
    ] {
        test_server_roundtrip!(ServerToClientMsg::AgentControl(AgentResponse {
            agents: vec![AgentStatus {
                not_ready_reason: reason,
                ..status.clone()
            }],
            ..AgentResponse::default()
        }));
    }
    test_server_roundtrip!(ServerToClientMsg::KillSessionComplete { error: None });
    test_server_roundtrip!(ServerToClientMsg::KillSessionComplete {
        error: Some("cleanup failed".into())
    });
    test_server_roundtrip!(ServerToClientMsg::AgentReportAccepted {
        report_id: "00000000-0000-4000-8000-000000000001".into(),
    });
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
    test_server_roundtrip!(ServerToClientMsg::Connected {
        warp_companions: true,
        warp_held_terminals: false,
    });
    test_server_roundtrip!(ServerToClientMsg::Connected {
        warp_companions: true,
        warp_held_terminals: true
    });
    test_server_roundtrip!(ServerToClientMsg::QueryTerminalSize);
    test_server_roundtrip!(ServerToClientMsg::ConfigFileUpdated);
    test_server_roundtrip!(ServerToClientMsg::RenamedSession {
        name: "my-session".to_string().into(),
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
    // survive the trip so `yinz run --block-until-exit` exits with what its command did.
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
        connect_to_session: SessionConnection::default(),
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: SessionConnection {
            name: Some("new_session_name".to_owned().into()),
            tab_position: Some(TabNumber::new(5)),
            pane_id: Some(PaneId::from_legacy(5, true)),
            layout: Some(LayoutInfo::BuiltIn("compact".to_owned())),
            cwd: Some(PathBuf::from("/path/to/cwd")),
        }
    });
    test_server_roundtrip!(ServerToClientMsg::SwitchSession {
        connect_to_session: SessionConnection {
            name: Some("new_session_name".to_owned().into()),
            tab_position: Some(TabNumber::new(5)),
            pane_id: Some(PaneId::from_legacy(5, true)),
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
        connect_to_session: SessionConnection {
            name: Some("new_session_name".to_owned().into()),
            tab_position: Some(TabNumber::new(5)),
            pane_id: Some(PaneId::from_legacy(5, true)),
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
        pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        viewport: vec!["hello".to_string(), "world".to_string()],
        scrollback: Some(vec!["line1".to_string()]),
        is_initial: true,
    });
    test_server_roundtrip!(ServerToClientMsg::PaneRenderUpdate {
        pane_id: PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2)),
        viewport: vec!["viewport line".to_string()],
        scrollback: None,
        is_initial: false,
    });
    test_server_roundtrip!(ServerToClientMsg::PaneRenderUpdate {
        pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
        viewport: vec![],
        scrollback: Some(vec![]),
        is_initial: true,
    });
    test_server_roundtrip!(ServerToClientMsg::SubscribedPaneClosed {
        pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
    });
    test_server_roundtrip!(ServerToClientMsg::SubscribedPaneClosed {
        pane_id: PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3)),
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
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
            fg: Some("#00e000".to_owned()),
            bg: Some("#001a3a".to_owned()),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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
/// Missing prost oneof tags only fail during byte decoding. Keep this focused
/// regression alongside the wire round-trips in `test_client_roundtrip!`.
#[test]
fn rename_active_pane_wire_roundtrip() {
    use prost::Message;

    let original = ClientToServerMsg::Action {
        action: Action::RenamePaneByPaneId {
            pane_id: None,
            name: "x".as_bytes().to_vec(),
        },
        terminal_id: Some(yinz_types::internal_ids::TerminalId::new(1)),
        client_id: Some(ClientId::new(100)),
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

/// The server answers a CLI pipe with an error instead of a frame that the thread writing to
/// the client would refuse and only log. Both must draw the line at the same byte, so the frame
/// is measured by the encoder the writer uses, not by a copy of the wire layout.
#[test]
fn a_cli_pipe_reply_is_refused_exactly_where_its_frame_would_be() {
    use crate::client_server_contract::client_server_contract::ServerToClientMsg as ProtoMsg;
    use crate::ipc::MAX_IPC_FRAME_LEN;

    let pipe_name = "pipe".to_owned();
    // What the frame adds around the output. 2 MiB takes the same 4-byte length prefixes as
    // 64 MiB; the frame length asserted below would catch it if it did not.
    let sample = "x".repeat(1 << 21);
    let envelope = ProtoMsg::from(ServerToClientMsg::CliPipeOutput {
        pipe_name: pipe_name.clone(),
        output: sample.clone(),
    })
    .encoded_len()
        - sample.len();

    // Never `{:?}` a reply here: it would print 64 MiB.
    let output = "x".repeat(MAX_IPC_FRAME_LEN - envelope);
    let reply = ServerToClientMsg::cli_pipe_output(pipe_name.clone(), output)
        .unwrap_or_else(|error| panic!("a frame of exactly the limit should be sent: {error}"));
    let frame = ProtoMsg::from(reply);
    assert_eq!(frame.encoded_len(), MAX_IPC_FRAME_LEN);
    let Ok(ServerToClientMsg::CliPipeOutput {
        pipe_name: sent_to,
        mut output,
    }) = ServerToClientMsg::try_from(frame)
    else {
        panic!("the reply should be the pipe's output");
    };
    assert_eq!(sent_to, pipe_name);
    assert_eq!(output.len(), MAX_IPC_FRAME_LEN - envelope);

    output.push('x');
    let Err(error) = ServerToClientMsg::cli_pipe_output(pipe_name, output) else {
        panic!("a frame one byte over the limit should be refused");
    };
    assert!(
        error.to_string().contains(&MAX_IPC_FRAME_LEN.to_string()),
        "{error}"
    );
}

/// Every variant, every case: the kinds are walked rather than sampled, so a variant the wire
/// mishandles cannot hide behind an unlucky draw. The generator has no wildcard arm, so a new
/// variant does not compile until it is covered here.
#[hegel::test]
fn every_action_survives_the_client_server_round_trip(tc: hegel::TestCase) {
    use crate::client_server_contract::client_server_contract::Action as ProtoAction;
    use prost::Message;
    use strum::IntoEnumIterator;
    use yinz_plugin_api::test_generators::action_of_kind;
    use yinz_types::input::actions::ActionKind;

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

#[test]
fn extracted_size_preserves_fixed_protobuf_bytes() {
    use crate::client_server_contract::client_server_contract::Size as ProtoSize;
    use prost::Message;

    for (size, bytes) in [
        (Size { rows: 0, cols: 0 }, vec![]),
        (Size { rows: 24, cols: 80 }, vec![0x08, 80, 0x10, 24]),
        (
            Size {
                rows: u32::MAX as usize,
                cols: u32::MAX as usize,
            },
            vec![
                0x08, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x10, 0xff, 0xff, 0xff, 0xff, 0x0f,
            ],
        ),
    ] {
        assert_eq!(ProtoSize::from(size).encode_to_vec(), bytes);
        let decoded = ProtoSize::decode(bytes.as_slice()).unwrap();
        assert_eq!(Size::try_from(decoded).unwrap(), size);
    }
}

#[test]
fn pane_ids_preserve_ipc_bytes() {
    use crate::client_server_contract::client_server_contract::PaneId as IpcPaneId;
    use yinz_types::{
        internal_ids::{PluginId, TerminalId},
        pane_id::PaneId,
    };
    for (pane, bytes) in [
        (PaneId::Terminal(TerminalId::new(0)), vec![8, 0]),
        (
            PaneId::Terminal(TerminalId::new(u32::MAX)),
            vec![8, 255, 255, 255, 255, 15],
        ),
        (PaneId::Plugin(PluginId::new(0)), vec![16, 0]),
        (
            PaneId::Plugin(PluginId::new(u32::MAX)),
            vec![16, 255, 255, 255, 255, 15],
        ),
    ] {
        let wire: IpcPaneId = pane.into();
        assert_eq!(wire.encode_to_vec(), bytes);
        assert_eq!(
            PaneId::try_from(IpcPaneId::decode(bytes.as_slice()).unwrap()).unwrap(),
            pane
        );
    }
}

#[test]
fn attach_pane_reference_keeps_its_json_object() {
    for (is_plugin, id) in [(false, 0), (true, u32::MAX)] {
        let pane = PaneId::from_legacy(id, is_plugin);
        let message = ClientToServerMsg::AttachClient {
            cli_assets: CliAssets::default(),
            tab_number_to_focus: None,
            pane_to_focus: Some(pane),
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(
            json["AttachClient"]["pane_to_focus"],
            serde_json::json!({
                "pane_id": id, "is_plugin": is_plugin,
            })
        );
        match serde_json::from_value::<ClientToServerMsg>(json).unwrap() {
            ClientToServerMsg::AttachClient { pane_to_focus, .. } => {
                assert_eq!(pane_to_focus, Some(pane))
            },
            _ => unreachable!(),
        }
    }
}

#[test]
fn override_layout_positions_survive_ipc_and_plugin_action_round_trips() {
    use crate::client_server_contract::client_server_contract::Action as IpcAction;
    use yinz_plugin_api::action::ProtobufAction as PluginAction;
    use yinz_types::input::layout::TabLayoutInfo;
    use yinz_types::internal_ids::TabPosition;

    let tabs: Vec<_> = [0, 7, u32::MAX as usize]
        .into_iter()
        .map(|position| TabLayoutInfo {
            tab_index: TabPosition::new(position),
            tab_name: Some(format!("tab-{position}")),
            tiled_layout: TiledPaneLayout {
                borderless: Some(false),
                ..Default::default()
            },
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
        })
        .collect();
    let action = Action::OverrideLayout {
        tabs: tabs.clone(),
        retain_existing_terminal_panes: true,
        retain_existing_plugin_panes: true,
        apply_only_to_active_tab: false,
    };
    let ipc: IpcAction = action.clone().try_into().unwrap();
    let ipc: Action = IpcAction::decode(ipc.encode_to_vec().as_slice())
        .unwrap()
        .try_into()
        .unwrap();
    let plugin: PluginAction = action.try_into().unwrap();
    let plugin: Action = PluginAction::decode(plugin.encode_to_vec().as_slice())
        .unwrap()
        .try_into()
        .unwrap();
    for decoded in [ipc, plugin] {
        // Action equality does not compare OverrideLayout's payload.
        match decoded {
            Action::OverrideLayout {
                tabs: decoded_tabs,
                retain_existing_terminal_panes: true,
                retain_existing_plugin_panes: true,
                apply_only_to_active_tab: false,
            } => assert_eq!(decoded_tabs, tabs),
            other => panic!("unexpected override action: {other:?}"),
        }
    }
    let json = serde_json::to_value(&tabs).unwrap();
    assert_eq!(json[1]["tab_index"], 7);
    assert_eq!(
        serde_json::from_value::<Vec<TabLayoutInfo>>(json).unwrap(),
        tabs
    );
}

#[test]
fn position_wire_preserves_scrollback_and_rejects_line_overflow() {
    use crate::client_server_contract::client_server_contract::Position as ProtoPosition;
    use yinz_types::position::{Column, Line};
    let position = Position {
        line: Line::new(-1),
        column: Column::new(7),
    };
    let proto = ProtoPosition::try_from(position).unwrap();
    assert_eq!(
        proto.encode_to_vec(),
        [8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1, 16, 7]
    );
    assert_eq!(Position::try_from(proto).unwrap(), position);
    let maximum_column = Position {
        line: Line::new(0),
        column: Column::new(usize::MAX),
    };
    assert_eq!(
        Position::try_from(ProtoPosition::try_from(maximum_column).unwrap()).unwrap(),
        maximum_column
    );
    if isize::BITS > 32 {
        assert!(ProtoPosition::try_from(Position {
            line: Line::new(isize::MAX),
            column: Column::new(0)
        })
        .is_err());
        assert!(ProtoPosition::try_from(Position {
            line: Line::new(isize::MIN),
            column: Column::new(0)
        })
        .is_err());
    }
}

#[test]
fn agent_double_escape_ipc_round_trip() {
    use crate::client_server_contract::client_server_contract::Options as ProtoOptions;
    for value in [None, Some(false), Some(true)] {
        let options = Options {
            agent_double_escape: value,
            ..Options::default()
        };
        let wire = ProtoOptions::from(options.clone()).encode_to_vec();
        let decoded = ProtoOptions::decode(wire.as_slice()).unwrap();
        assert_eq!(Options::try_from(decoded).unwrap(), options);
    }
}

#[test]
fn warp_companion_relationships_roundtrip_and_reject_malformed_groups() {
    use yinz_types::data::{WarpHiddenPane, WarpRelationships, WarpStackList};
    let relationships = WarpRelationships {
        hidden: vec![
            WarpHiddenPane {
                pane: 1,
                restored_by: None,
                is_scrollback_editor: false,
            },
            WarpHiddenPane {
                pane: 2,
                restored_by: Some(1),
                is_scrollback_editor: true,
            },
        ],
        stacks: vec![WarpStackList {
            members: vec![1, 0],
            selected: 0,
        }],
    };
    let payload = WarpedTab {
        relationships: Some(relationships.clone()),
        floating_layout: vec![FloatingPaneLayout::default(); 2],
        tiled_panes: vec![WarpedPane::default()],
        floating_panes: vec![WarpedPane::default(); 2],
        ..Default::default()
    };
    let message = |payload| ClientToServerMsg::Action {
        action: Action::AdoptPane {
            payload: Box::new(WarpedPane {
                agent_theme: None,
                held: None,
                group: Some(Box::new(payload)),
                ..Default::default()
            }),
            placement: WarpPlacement::FocusedTab,
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: true,
    };
    test_client_roundtrip!(message(payload.clone()));
    for case in [
        "cycle",
        "out of range",
        "duplicate hidden",
        "duplicate cover",
        "duplicate member",
        "invalid selection",
        "missing terminal",
        "invalid shape",
    ] {
        let mut malformed = payload.clone();
        let relationships = malformed.relationships.as_mut().unwrap();
        match case {
            "cycle" => relationships.hidden[0].restored_by = Some(2),
            "out of range" => relationships.hidden[0].restored_by = Some(3),
            "duplicate hidden" => relationships.hidden.push(relationships.hidden[0].clone()),
            "duplicate cover" => relationships.hidden[0].restored_by = Some(1),
            "duplicate member" => relationships.stacks[0].members.push(1),
            "invalid selection" => relationships.stacks[0].selected = 2,
            "missing terminal" => {
                malformed.floating_panes.pop();
            },
            "invalid shape" => {
                relationships.hidden.pop();
            },
            _ => unreachable!(),
        }
        use crate::client_server_contract::client_server_contract::ClientToServerMsg as ProtoMessage;
        let wire: ProtoMessage = message(malformed)
            .try_into()
            .expect("encode malformed fixture");
        let decoded =
            ProtoMessage::decode(wire.encode_to_vec().as_slice()).expect("protobuf decode");
        assert!(
            ClientToServerMsg::try_from(decoded).is_err(),
            "accepted {case}"
        );
    }
}

#[test]
fn originating_plugin_generation_roundtrips_and_defaults_for_legacy_records() {
    use crate::client_server_contract::client_server_contract::OriginatingPlugin as WireOrigin;
    use prost::Message;
    let legacy = OriginatingPlugin::new(
        yinz_types::internal_ids::PluginId::new(11),
        ClientId::new(22),
        Default::default(),
    );
    let bytes = [8, 11, 16, 22];
    assert_eq!(
        OriginatingPlugin::try_from(WireOrigin::decode(bytes.as_slice()).unwrap()).unwrap(),
        legacy
    );
    assert_eq!(WireOrigin::from(legacy.clone()).encode_to_vec(), bytes);
    assert_eq!(
        serde_json::from_str::<OriginatingPlugin>(
            r#"{"plugin_id":11,"client_id":22,"context":{}}"#
        )
        .unwrap(),
        legacy
    );
    let mut generated = legacy;
    generated.deno_generation = Some(u64::MAX);
    let bytes = WireOrigin::from(generated.clone()).encode_to_vec();
    assert_eq!(
        OriginatingPlugin::try_from(WireOrigin::decode(bytes.as_slice()).unwrap()).unwrap(),
        generated
    );
    assert_eq!(
        serde_json::from_str::<OriginatingPlugin>(&serde_json::to_string(&generated).unwrap())
            .unwrap(),
        generated
    );
}

#[test]
fn pane_tabs_layout_decoding_validates_nested_and_floating_children() {
    use crate::client_server_contract::client_server_contract as wire;
    for json in [
        r#"{"tabs":[],"active":0}"#,
        r#"{"tabs":[{"label":"First","location":"yinz:strider"}],"active":1}"#,
        r#"{"tabs":[{"label":" ","location":"yinz:strider"}],"active":0}"#,
        r#"{"tabs":[{"label":"First","location":" "}],"active":0}"#,
    ] {
        let mut tiled =
            wire::TiledPaneLayout::from(yinz_types::input::layout::TiledPaneLayout::default());
        tiled.pane_tabs_json = Some(json.into());
        assert!(yinz_types::input::layout::TiledPaneLayout::try_from(tiled.clone()).is_err());
        let mut parent =
            wire::TiledPaneLayout::from(yinz_types::input::layout::TiledPaneLayout::default());
        parent.children = vec![tiled];
        assert!(yinz_types::input::layout::TiledPaneLayout::try_from(parent).is_err());
        let mut floating = wire::FloatingPaneLayout::from(
            yinz_types::input::layout::FloatingPaneLayout::default(),
        );
        floating.pane_tabs_json = Some(json.into());
        assert!(yinz_types::input::layout::FloatingPaneLayout::try_from(floating).is_err());
    }
    let json = r#"{"tabs":[{"label":"First","location":"yinz:strider"}],"active":0}"#;
    let mut tiled =
        wire::TiledPaneLayout::from(yinz_types::input::layout::TiledPaneLayout::default());
    tiled.pane_tabs_json = Some(json.into());
    assert!(yinz_types::input::layout::TiledPaneLayout::try_from(tiled).is_ok());
    let mut floating =
        wire::FloatingPaneLayout::from(yinz_types::input::layout::FloatingPaneLayout::default());
    floating.pane_tabs_json = Some(json.into());
    assert!(yinz_types::input::layout::FloatingPaneLayout::try_from(floating).is_ok());
}

#[test]
fn warp_held_capability_defaults_to_false_for_older_destinations() {
    use crate::client_server_contract::client_server_contract::ConnectedMsg;
    use prost::Message;
    // An older server sends only field 1 (companion support).
    let connected = ConnectedMsg::decode(&b"\x08\x01"[..]).unwrap();
    assert!(connected.warp_companions);
    assert!(!connected.warp_held_terminals);
}

#[test]
fn overlay_opacity_is_optional_and_validated_at_the_ipc_boundary() {
    use crate::client_server_contract::client_server_contract::{
        new_pane_placement::PlacementType, NewPanePlacement as WirePlacement,
        OverlayPlacement as WireOverlay,
    };
    for opacity in [
        None,
        Some(0),
        Some(75),
        Some(100),
        Some(101),
        Some(256),
        Some(u32::MAX),
    ] {
        let wire = WirePlacement {
            placement_type: Some(PlacementType::Overlay(WireOverlay {
                background_opacity: opacity,
                ..Default::default()
            })),
        };
        let result = NewPanePlacement::try_from(wire);
        if opacity.is_some_and(|opacity| opacity > 100) {
            assert!(result.is_err());
        } else {
            let NewPanePlacement::Overlay(placement) = result.unwrap() else {
                panic!("overlay")
            };
            assert_eq!(placement.background_opacity.map(u32::from), opacity);
            let wire: WirePlacement = NewPanePlacement::Overlay(placement).into();
            let Some(PlacementType::Overlay(overlay)) = wire.placement_type else {
                panic!("overlay")
            };
            assert_eq!(overlay.background_opacity, opacity);
        }
    }
}

#[test]
fn global_background_opacity_wire_range_and_roundtrip() {
    use crate::client_server_contract::client_server_contract::Options as WireOptions;
    for opacity in [None, Some(0), Some(85), Some(100)] {
        let options = Options {
            background_opacity: opacity,
            ..Default::default()
        };
        let wire = WireOptions::from(options.clone());
        assert_eq!(wire.background_opacity, opacity.map(u32::from));
        assert_eq!(Options::try_from(wire).unwrap(), options);
    }
    for opacity in [101, 256, u32::MAX] {
        assert!(Options::try_from(WireOptions {
            background_opacity: Some(opacity),
            ..Default::default()
        })
        .is_err());
    }
}

#[test]
fn agent_spawn_placement_and_failure_resources_roundtrip() {
    use yinz_types::agent::control::{AgentOutcome, AgentPlacement, AgentRequest, AgentResponse};
    use yinz_types::internal_ids::TabId;
    for placement in [
        AgentPlacement::Automatic,
        AgentPlacement::Tab(TabId::new(0)),
        AgentPlacement::Tab(TabId::new(7)),
        AgentPlacement::NewTab(None),
        AgentPlacement::NewTab(Some("review".into())),
    ] {
        test_client_roundtrip!(ClientToServerMsg::AgentControl(AgentRequest::Spawn {
            placement,
            spec: Default::default(),
            cwd: None,
            project: None,
            remote_host: None,
        }));
    }
    test_client_roundtrip!(ClientToServerMsg::AgentControl(
        AgentRequest::SpawnPlacementSupport
    ));
    test_client_roundtrip!(ClientToServerMsg::AgentControl(
        AgentRequest::SpawnBubblewrapSupport
    ));
    test_server_roundtrip!(ServerToClientMsg::AgentControl(AgentResponse {
        outcome: AgentOutcome::Failed,
        launch_resources: Some(yinz_types::agent::AgentLaunchResources {
            tab_id: Some(TabId::new(7)),
            created_tab_id: Some(TabId::new(7)),
            branch: Some("review".into()),
            cwd: Some("/repo/review".into()),
            ..Default::default()
        }),
        ..Default::default()
    }));
}

#[test]
fn legacy_warp_messages_default_to_no_paste_or_input_authority() {
    use crate::client_server_contract::client_server_contract::{
        AdoptPaneAction, WarpedPane as ProtoWarpedPane,
    };
    use prost::Message;
    let pane = ProtoWarpedPane::decode(&b""[..]).unwrap();
    assert!(!pane.bracketed_paste);
    assert!(pane.agent_input.is_none());
    let pane = AdoptPaneAction::decode(&b""[..]).unwrap();
    assert!(!pane.bracketed_paste);
    assert!(pane.agent_input.is_none());
}

#[test]
fn cli_pipe_proto_without_focus_field_defaults_to_false() {
    use crate::client_server_contract::client_server_contract::{
        action::ActionType, Action as ProtoAction, CliPipeAction,
    };
    // Old sender: only tag 1 (pipe_id), no tag 13.
    let old = CliPipeAction::decode(&b"\x0a\x04pipe"[..]).unwrap();
    assert!(!old.focus_plugin);
    let action: Action = ProtoAction {
        action_type: Some(ActionType::CliPipe(old)),
    }
    .try_into()
    .unwrap();
    assert!(matches!(
        action,
        Action::CliPipe {
            focus_plugin: false,
            ..
        }
    ));
}

#[test]
fn agent_mouse_override_options_round_trip() {
    use crate::client_server_contract::client_server_contract::Options as WireOptions;
    use yinz_types::input::options::Options;
    for value in [None, Some(false), Some(true)] {
        let options = Options {
            context_menu_override_agent_mouse: value,
            ..Default::default()
        };
        let wire = WireOptions::from(options.clone()).encode_to_vec();
        assert_eq!(
            Options::try_from(WireOptions::decode(wire.as_slice()).unwrap()).unwrap(),
            options
        );
    }
}
