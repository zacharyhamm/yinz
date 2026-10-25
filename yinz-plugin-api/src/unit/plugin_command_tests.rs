use super::*;
use yinz_types::command::{CommandToRun, GitCheckout};
use yinz_types::data::PluginCommand;

#[test]
fn tab_commands_preserve_their_zero_and_one_based_values() {
    for value in [0, 1, u32::MAX] {
        for original in [
            PluginCommand::GoToTab(TabPosition::new(value as usize)),
            PluginCommand::SwitchTabTo(TabNumber::new(value)),
        ] {
            let proto = ProtobufPluginCommand::try_from(original.clone()).unwrap();
            match &proto.payload {
                Some(Payload::GoToTabPayload(index)) => assert_eq!(*index, value),
                Some(Payload::SwitchTabToPayload(payload)) => {
                    assert_eq!(payload.tab_index, value)
                },
                _ => panic!("the command changed its payload"),
            }
            assert_eq!(PluginCommand::try_from(proto).unwrap(), original);
        }
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn tab_commands_reject_positions_outside_the_wire_range() {
    let position = TabPosition::new(usize::MAX);
    for command in [
        PluginCommand::GoToTab(position),
        PluginCommand::CloseTabWithIndex(position),
        PluginCommand::RenameTab(position, "work".to_owned()),
        PluginCommand::SwitchSession(ConnectToSession {
            tab_position: Some(position),
            ..Default::default()
        }),
    ] {
        assert!(ProtobufPluginCommand::try_from(command).is_err());
    }
}

#[test]
fn default_keybindings_survive_the_wire() {
    let protobuf: ProtobufPluginCommand = PluginCommand::RegisterDefaultKeybindings(
        "keybinds { normal { bind \"x\" { Quit; }; }; }".to_owned(),
    )
    .try_into()
    .unwrap();
    let decoded: PluginCommand = protobuf.try_into().unwrap();
    assert!(
        matches!(decoded, PluginCommand::RegisterDefaultKeybindings(keybindings) if keybindings.contains("Quit"))
    );
}

#[test]
fn git_checkout_command_survives_the_wire() {
    {
        let command = PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
            CommandToRun::new("codex"),
            PathBuf::from("/work/project"),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        );
        let protobuf: ProtobufPluginCommand = command.try_into().unwrap();
        let decoded: PluginCommand = protobuf.try_into().unwrap();
        assert!(matches!(decoded,
                PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(_, source, GitCheckout::NewWorktree { branch, base, path })
                if source == *"/work/project" && branch == "topic" && base == "main" && path == *".worktrees/topic"));
    }
}

#[test]
fn spawn_agent_survives_the_wire() {
    let command = PluginCommand::SpawnAgent {
        spec: AgentSpawnSpec {
            kind: Some("claude".to_owned()),
            command: Some("claude".to_owned()),
            args: vec!["--verbose".to_owned()],
            model: Some("opus".to_owned()),
            effort: Some("high".to_owned()),
            prompt: Some("fix the parser".to_owned()),
            branch: Some("fix-parser".to_owned()),
            base: Some("main".to_owned()),
            worktree: Some(true),
            worktree_dir: Some(".worktrees".to_owned()),
            reuse: Some(true),
            reuse_prompt: Some("resume the parser fix".into()),
            allow_outside_worktree: Some(true),
            env: BTreeMap::from([("TOKEN".to_owned(), "t".to_owned())]),
            permission_mode: Some("skip".to_owned()),
            resume: Some("last".to_owned()),
            hooks: Some(false),
            bubblewrap: true,
        },
        source_directory: Some(PathBuf::from("/host")),
        cwd: Some(PathBuf::from("/tmp/work")),
        name: Some("fixer".to_owned()),
        context: BTreeMap::from([("step".to_owned(), "1".to_owned())]),
        start_suppressed: true,
    };
    let protobuf: ProtobufPluginCommand = command.clone().try_into().unwrap();
    let decoded: PluginCommand = protobuf.try_into().unwrap();
    assert_eq!(decoded, command);

    let empty = PluginCommand::SpawnAgent {
        spec: AgentSpawnSpec::default(),
        source_directory: None,
        cwd: None,
        name: None,
        context: BTreeMap::new(),
        start_suppressed: false,
    };
    let protobuf: ProtobufPluginCommand = empty.clone().try_into().unwrap();
    let decoded: PluginCommand = protobuf.try_into().unwrap();
    assert_eq!(decoded, empty);
}

#[test]
fn notifications_with_and_without_panes_survive_the_wire() {
    for pane_id in [
        None,
        Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            7,
        ))),
        Some(PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3))),
    ] {
        // The click rides on the pane payload, so it is only offered where there is one -
        // see the encoder, which drops a click that has no pane to land in.
        let click = pane_id.map(|_| NotifyClick {
            message: "agent_toast_click".to_owned(),
            payload: "42".to_owned(),
        });
        let protobuf: ProtobufPluginCommand =
            PluginCommand::Notify("done".to_owned(), pane_id, click.clone())
                .try_into()
                .expect("encode");
        assert_eq!(
            matches!(&protobuf.payload, Some(Payload::NotifyPayload(_))),
            pane_id.is_none(),
            "plain notifications keep the legacy string payload",
        );
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::Notify(text, decoded_pane_id, decoded_click) => {
                assert_eq!(text, "done");
                assert_eq!(decoded_pane_id, pane_id);
                assert_eq!(decoded_click, click);
            },
            other => panic!("expected Notify, got {:?}", other),
        }
    }
}

#[test]
fn set_pane_frame_style_protobuf_round_trip() {
    use yinz_types::input::options::PaneFrameStyle;
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let original = PluginCommand::SetPaneFrameStyle(style);
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::SetPaneFrameStyle(decoded_style) => {
                assert_eq!(decoded_style, style)
            },
            other => panic!("expected SetPaneFrameStyle, got {:?}", other),
        }
    }
}

/// Both halves have to survive: a reveal that loses its pane id points at
/// nothing, and one that loses its label marks a pane without saying what
/// for.
#[test]
fn reveal_pane_with_id_protobuf_round_trip() {
    for (pane_id, label) in [
        (
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            Some("agent: send_input?".to_owned()),
        ),
        (
            PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3)),
            None,
        ),
    ] {
        let original = PluginCommand::RevealPaneWithId(pane_id, label.clone());
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::RevealPaneWithId(decoded_pane_id, decoded_label) => {
                assert_eq!(decoded_pane_id, pane_id);
                assert_eq!(decoded_label, label);
            },
            other => panic!("expected RevealPaneWithId, got {:?}", other),
        }
    }
}

/// `None` is a value here, not an absence: it is how the prefix is *dropped*,
/// so an encoding that lost the difference between "no prefix" and "clear the
/// prefix" would leave a stale checkout on a pane that is no longer an agent.
#[test]
fn set_pane_title_prefix_protobuf_round_trip() {
    for (pane_id, prefix) in [
        (
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            Some("yinz:topic".to_owned()),
        ),
        (
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            Some(String::new()),
        ),
        (
            PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3)),
            None,
        ),
    ] {
        let original = PluginCommand::SetPaneTitlePrefix {
            pane_id,
            prefix: prefix.clone(),
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::SetPaneTitlePrefix {
                pane_id: decoded_pane_id,
                prefix: decoded_prefix,
            } => {
                assert_eq!(decoded_pane_id, pane_id);
                assert_eq!(decoded_prefix, prefix);
            },
            other => panic!("expected SetPaneTitlePrefix, got {:?}", other),
        }
    }
}

#[test]
fn set_pane_subtitle_protobuf_round_trip() {
    for (pane_id, subtitle) in [
        (
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            Some("yinz:topic".to_owned()),
        ),
        (
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            Some(String::new()),
        ),
        (
            PaneId::Plugin(yinz_types::internal_ids::PluginId::new(3)),
            None,
        ),
    ] {
        let original = PluginCommand::SetPaneSubtitle {
            pane_id,
            subtitle: subtitle.clone(),
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::SetPaneSubtitle {
                pane_id: decoded_pane_id,
                subtitle: decoded_subtitle,
            } => {
                assert_eq!(decoded_pane_id, pane_id);
                assert_eq!(decoded_subtitle, subtitle);
            },
            other => panic!("expected SetPaneSubtitle, got {:?}", other),
        }
    }
}

#[test]
fn new_pane_protobuf_round_trip() {
    let original = PluginCommand::NewPane;
    let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    match decoded {
        PluginCommand::NewPane => {},
        other => panic!("expected NewPane, got {:?}", other),
    }
}

#[test]
fn toggle_floating_panes_focused_tab_protobuf_round_trip() {
    let original = PluginCommand::ToggleFloatingPanes { tab_id: None };
    let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    match decoded {
        PluginCommand::ToggleFloatingPanes { tab_id } => assert_eq!(tab_id, None),
        other => panic!("expected ToggleFloatingPanes, got {:?}", other),
    }
}

#[test]
fn toggle_floating_panes_with_tab_id_protobuf_round_trip() {
    let original = PluginCommand::ToggleFloatingPanes {
        tab_id: Some(TabId::new(2)),
    };
    let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    match decoded {
        PluginCommand::ToggleFloatingPanes { tab_id } => {
            assert_eq!(tab_id, Some(TabId::new(2)))
        },
        other => panic!("expected ToggleFloatingPanes, got {:?}", other),
    }
}

#[test]
fn web_request_protobuf_round_trip() {
    use yinz_types::data::HttpVerb;
    // None must survive as None: it is what keeps a plain web_request buffered,
    // and what keeps a request unproxied.
    for stream_id in [None, Some("stream-1".to_owned())] {
        for proxy in [None, Some("socks5://127.0.0.1:1080".to_owned())] {
            let original = PluginCommand::WebRequest(
                "https://example.com/v1/chat/completions".to_owned(),
                HttpVerb::Post,
                BTreeMap::from([("authorization".to_owned(), "Bearer sk-test".to_owned())]),
                b"{\"stream\":true}".to_vec(),
                BTreeMap::from([("request_id".to_owned(), "7".to_owned())]),
                stream_id.clone(),
                proxy.clone(),
            );
            let protobuf: ProtobufPluginCommand = original.clone().try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match (decoded, original) {
                (
                    PluginCommand::WebRequest(
                        url,
                        _,
                        headers,
                        body,
                        context,
                        decoded_stream_id,
                        decoded_proxy,
                    ),
                    PluginCommand::WebRequest(
                        expected_url,
                        _,
                        expected_headers,
                        expected_body,
                        expected_context,
                        _,
                        _,
                    ),
                ) => {
                    assert_eq!(url, expected_url);
                    assert_eq!(headers, expected_headers);
                    assert_eq!(body, expected_body);
                    assert_eq!(context, expected_context);
                    assert_eq!(decoded_stream_id, stream_id);
                    assert_eq!(decoded_proxy, proxy);
                },
                (other, _) => panic!("expected WebRequest, got {:?}", other),
            }
        }
    }
}

#[test]
fn list_notifications_protobuf_round_trip() {
    let protobuf: ProtobufPluginCommand =
        PluginCommand::ListNotifications.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    assert!(matches!(decoded, PluginCommand::ListNotifications));

    let notification = NotificationInfo {
        id: NotificationId::new(7),
        message: "build finished".to_owned(),
        received_at_unix_ms: yinz_types::units::UnixMillis::new(42),
        outcome: None,
    };
    let protobuf: ProtobufNotificationInfo = notification.clone().into();
    assert_eq!(NotificationInfo::from(protobuf), notification);
}

#[test]
fn cancel_web_request_protobuf_round_trip() {
    let original = PluginCommand::CancelWebRequest("stream-1".to_owned());
    let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    match decoded {
        PluginCommand::CancelWebRequest(stream_id) => assert_eq!(stream_id, "stream-1"),
        other => panic!("expected CancelWebRequest, got {:?}", other),
    }
}

#[test]
fn project_context_defaults_sources_and_round_trips() {
    for wire in [
        serde_json::json!({"SetProjectContext": [["notes"], []]}),
        serde_json::json!({"SetProjectContext": [["notes"], [], {"notes": "/home/雪/[notes]"}]}),
    ] {
        let original: PluginCommand = serde_json::from_value(wire.clone()).unwrap();
        let PluginCommand::SetProjectContext(_, _, sources) = &original else {
            panic!("project context")
        };
        assert_eq!(
            sources.get("notes").map(String::as_str),
            wire["SetProjectContext"][2]["notes"].as_str()
        );
        let protobuf: ProtobufPluginCommand = original.clone().try_into().unwrap();
        let decoded: PluginCommand = protobuf.try_into().unwrap();
        assert_eq!(decoded, original);
    }
}

#[test]
fn run_command_output_limit_protobuf_round_trip() {
    for max_output_bytes in [None, Some(40_000)] {
        let original = PluginCommand::RunCommand(
            vec!["git".to_owned(), "show".to_owned()],
            BTreeMap::new(),
            PathBuf::from("."),
            BTreeMap::new(),
            max_output_bytes,
            Some(true),
        );
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::RunCommand(_, _, _, _, decoded_limit, Some(true)) => {
                assert_eq!(decoded_limit, max_output_bytes)
            },
            other => panic!("expected RunCommand, got {:?}", other),
        }
    }
}

/// Every placement, every time: a mis-numbered proto enum is exactly the kind
/// of mistake a single-value test walks straight past.
const EVERY_PLACEMENT: [WarpPlacement; 3] = [
    WarpPlacement::NewTab,
    WarpPlacement::FocusedTab,
    WarpPlacement::Floating,
];

#[test]
fn warp_pane_to_session_protobuf_round_trip() {
    for placement in EVERY_PLACEMENT {
        let original = PluginCommand::WarpPaneToSession {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            to_session: "other".to_owned().into(),
            placement,
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::WarpPaneToSession {
                pane_id,
                to_session,
                placement: decoded_placement,
            } => {
                assert_eq!(
                    pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
                );
                assert_eq!(to_session, "other".into());
                assert_eq!(decoded_placement, placement);
            },
            other => panic!("expected WarpPaneToSession, got {:?}", other),
        }
    }
}

#[test]
fn warp_pane_from_session_protobuf_round_trip() {
    for placement in EVERY_PLACEMENT {
        let original = PluginCommand::WarpPaneFromSession {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(11)),
            from_session: "other".to_owned().into(),
            placement,
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::WarpPaneFromSession {
                pane_id,
                from_session,
                placement: decoded_placement,
            } => {
                assert_eq!(
                    pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(11))
                );
                assert_eq!(from_session, "other".into());
                assert_eq!(decoded_placement, placement);
            },
            other => panic!("expected WarpPaneFromSession, got {:?}", other),
        }
    }
}

#[test]
fn list_orphaned_panes_protobuf_round_trip() {
    let original = PluginCommand::ListOrphanedPanes;
    let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
    let decoded: PluginCommand = protobuf.try_into().expect("decode");
    match decoded {
        PluginCommand::ListOrphanedPanes => {},
        other => panic!("expected ListOrphanedPanes, got {:?}", other),
    }
}

#[test]
fn adopt_orphaned_pane_protobuf_round_trip() {
    for placement in EVERY_PLACEMENT {
        let original = PluginCommand::AdoptOrphanedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            placement,
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::AdoptOrphanedPane {
                host_id,
                placement: decoded_placement,
            } => {
                assert_eq!(host_id, SupervisorId::new("abc123"));
                assert_eq!(decoded_placement, placement);
            },
            other => panic!("expected AdoptOrphanedPane, got {:?}", other),
        }
    }
}

#[test]
fn a_warp_placement_from_a_newer_api_is_rejected_rather_than_defaulted() {
    // Defaulting an unknown placement to NewTab would put the pane somewhere
    // the caller did not ask for, with no way for it to find out.
    assert!(warp_placement_from_i32(99).is_err());
}

#[test]
fn warp_pane_response_protobuf_round_trip() {
    for response in [
        WarpPaneResponse::Ok("Moved pane to session 'other'".to_owned()),
        WarpPaneResponse::Err("this is the session's only pane".to_owned()),
    ] {
        let protobuf: ProtobufWarpPaneResponse = response.clone().into();
        assert_eq!(
            WarpPaneResponse::try_from(protobuf).expect("decode"),
            response
        );
    }
}

/// The three exit states an orphan can be in. Flattening `Option<Option<i32>>`
/// into a flag plus a code is the one place in this conversion where
/// information could silently go missing.
#[test]
fn an_orphaned_panes_exit_state_survives_the_wire() {
    for exit_status in [None, Some(None), Some(Some(1))] {
        let pane = OrphanedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            session_name: Some("gone".to_owned().into()),
            command: "top".to_owned(),
            cwd: Some(PathBuf::from("/tmp")),
            child_pid: Some(ProcessId::new(4242)),
            started_at_unix: yinz_types::units::UnixSeconds::new(1_700_000_000),
            exit_status,
            buffered_bytes: 4096,
        };
        let protobuf: ProtobufOrphanedPane = pane.clone().into();
        assert_eq!(OrphanedPane::from(protobuf), pane);
    }
}

#[test]
fn list_orphaned_panes_response_protobuf_round_trip() {
    for response in [
        ListOrphanedPanesResponse::Ok(vec![]),
        ListOrphanedPanesResponse::Ok(vec![OrphanedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            command: "top".to_owned(),
            ..Default::default()
        }]),
        ListOrphanedPanesResponse::Err("not supported here".to_owned()),
    ] {
        let protobuf: ProtobufListOrphanedPanesResponse = response.clone().into();
        assert_eq!(
            ListOrphanedPanesResponse::try_from(protobuf).expect("decode"),
            response
        );
    }
}

mod props {
    use super::*;
    use crate::action::tests::props::as_the_plugin_wire_carries_it as action_as_the_plugin_wire_carries_it;
    use crate::test_generators::plugin_command_of_kind;
    use strum::IntoEnumIterator;
    use yinz_types::geometry::ResizeStrategy;
    use {yinz_types::data::CommandType, yinz_types::data::ContextMenuItem};

    /// `message Resize` has `resize_action` and `direction`, and no room for
    /// `invert_on_boundaries`; the decoder fills in `false`.
    fn resize_strategy(strategy: ResizeStrategy) -> ResizeStrategy {
        ResizeStrategy {
            invert_on_boundaries: false,
            ..strategy
        }
    }

    /// `message ContextMenuItem` carries no actions by design: the server re-attaches a
    /// configured item's actions by `id` (see `data::ContextMenuItem`).
    fn context_menu_item(item: ContextMenuItem) -> ContextMenuItem {
        ContextMenuItem {
            actions: vec![],
            ..item
        }
    }

    /// What the plugin wire hands back for `command`, or `None` where the encoder refuses
    /// it.
    ///
    /// Written against `plugin_command.proto`, not against the conversion: every field
    /// changed here is one its payload has no room for, so the round trip is exact
    /// everywhere else, and a field the payload *does* carry going missing still fails.
    fn as_the_plugin_wire_carries_it(command: PluginCommand) -> Option<PluginCommand> {
        Some(match command {
            PluginCommand::CommandPalette(..) => return None,
            carried @ (PluginCommand::Subscribe(_)
            | PluginCommand::PaneTabs(..)
            | PluginCommand::CreateMultipane(..)
            | PluginCommand::UpdateMultipane(..)
            | PluginCommand::ReplaceMultipaneChild(..)
            | PluginCommand::CheckPlugin { .. }
            | PluginCommand::TestPlugin { .. }
            | PluginCommand::CancelPluginTest(_)
            | PluginCommand::SetPaneAgent { .. }
            | PluginCommand::SetPaneAgentRun { .. }
            | PluginCommand::SpawnAgent { .. }
            | PluginCommand::AgentControl { .. }
            | PluginCommand::RestartAgentPane { .. }
            | PluginCommand::OpenPluginPaneInPlaceOfPaneId { .. }
            | PluginCommand::InspectConfiguration(..)
            | PluginCommand::EditConfiguration(..)
            | PluginCommand::ResolveProject { .. }
            | PluginCommand::ProjectConfigRead { .. }
            | PluginCommand::ProjectConfigWrite { .. }
            | PluginCommand::SetQuitNotice { .. }
            | PluginCommand::ReloadPluginByUrl(_)
            | PluginCommand::ReadPluginLog { .. }
            | PluginCommand::GetLocalUtcOffset { .. }
            | PluginCommand::Unsubscribe(_)
            | PluginCommand::SetSelectable(_)
            | PluginCommand::GetCellPixelSize
            | PluginCommand::GetPluginIds
            | PluginCommand::GetYinzVersion
            | PluginCommand::OpenFile(..)
            | PluginCommand::OpenFileFloating(..)
            | PluginCommand::OpenTerminal(_)
            | PluginCommand::OpenTerminalFloating(..)
            | PluginCommand::OpenCommandPane(..)
            | PluginCommand::OpenCommandPaneFloating(..)
            | PluginCommand::OpenCommandPaneOverlay(..)
            | PluginCommand::SwitchTabTo(_)
            | PluginCommand::SetTimeout(_)
            | PluginCommand::ExecCmd(_)
            | PluginCommand::PostMessageTo(_)
            | PluginCommand::PostMessageToPlugin(_)
            | PluginCommand::HideSelf
            | PluginCommand::ShowSelf(_)
            | PluginCommand::SwitchToMode(_)
            | PluginCommand::NewTabsWithLayout(_)
            | PluginCommand::NewTab { .. }
            | PluginCommand::NewTabUnfocused { .. }
            | PluginCommand::NewTiledPaneInTab { .. }
            | PluginCommand::ToggleFloatingPanes { .. }
            | PluginCommand::NewPane
            | PluginCommand::GoToNextTab
            | PluginCommand::GoToPreviousTab
            | PluginCommand::Resize(_)
            | PluginCommand::FocusNextPane
            | PluginCommand::FocusPreviousPane
            | PluginCommand::FocusLastPane
            | PluginCommand::MoveFocus(_)
            | PluginCommand::MoveFocusOrTab(_)
            | PluginCommand::Detach
            | PluginCommand::EditScrollback
            | PluginCommand::Write(_)
            | PluginCommand::WriteChars(_)
            | PluginCommand::ToggleTab
            | PluginCommand::MovePane
            | PluginCommand::MovePaneWithDirection(_)
            | PluginCommand::ClearScreen
            | PluginCommand::ScrollUp
            | PluginCommand::ScrollDown
            | PluginCommand::ScrollToTop
            | PluginCommand::ScrollToBottom
            | PluginCommand::PageScrollUp
            | PluginCommand::PageScrollDown
            | PluginCommand::ToggleFocusFullscreen
            | PluginCommand::ToggleFocusNoUiFullscreen
            | PluginCommand::TogglePaneFrames
            | PluginCommand::SetPaneFrameStyle(_)
            | PluginCommand::TogglePaneEmbedOrEject
            | PluginCommand::UndoRenamePane
            | PluginCommand::CloseFocus
            | PluginCommand::ToggleActiveTabSync
            | PluginCommand::CloseFocusedTab
            | PluginCommand::UndoRenameTab
            | PluginCommand::QuitYinz
            | PluginCommand::PreviousSwapLayout
            | PluginCommand::NextSwapLayout
            | PluginCommand::GoToTabName(_)
            | PluginCommand::FocusOrCreateTab(_)
            | PluginCommand::GoToTab(_)
            | PluginCommand::StartOrReloadPlugin(_)
            | PluginCommand::CloseTerminalPane(_)
            | PluginCommand::ClosePluginPane(_)
            | PluginCommand::FocusTerminalPane(..)
            | PluginCommand::FocusPluginPane(..)
            | PluginCommand::RenameTerminalPane(..)
            | PluginCommand::RenamePluginPane(..)
            | PluginCommand::RenameTab(..)
            | PluginCommand::ReportPanic(_)
            | PluginCommand::RequestPluginPermissions(_)
            | PluginCommand::SwitchSession(_)
            | PluginCommand::DeleteDeadSession(_)
            | PluginCommand::DeleteAllDeadSessions
            | PluginCommand::OpenTerminalInPlace(_)
            | PluginCommand::OpenFileInPlace(..)
            | PluginCommand::OpenCommandPaneInPlace(..)
            | PluginCommand::RunCommand(..)
            | PluginCommand::WebRequest(..)
            | PluginCommand::CancelWebRequest(_)
            | PluginCommand::RenameSession(_)
            | PluginCommand::Notify(..)
            | PluginCommand::UnblockCliPipeInput(_)
            | PluginCommand::BlockCliPipeInput(_)
            | PluginCommand::CliPipeOutput(..)
            | PluginCommand::MessageToPlugin(_)
            | PluginCommand::DisconnectOtherClients
            | PluginCommand::KillSessions(_)
            | PluginCommand::ScanHostFolder(_)
            | PluginCommand::WatchFilesystem
            | PluginCommand::FsRead { .. }
            | PluginCommand::FsList { .. }
            | PluginCommand::FsMetadata { .. }
            | PluginCommand::FsWrite { .. }
            | PluginCommand::FsRemove { .. }
            | PluginCommand::FsCreateDir { .. }
            | PluginCommand::FsRename { .. }
            | PluginCommand::GrantPluginPermissions { .. }
            | PluginCommand::DumpSessionLayout { .. }
            | PluginCommand::CloseSelf
            | PluginCommand::NewTabsWithLayoutInfo(_)
            | PluginCommand::Reconfigure(..)
            | PluginCommand::HidePaneWithId(_)
            | PluginCommand::ShowPaneWithId(..)
            | PluginCommand::OpenCommandPaneBackground(..)
            | PluginCommand::RerunCommandPane(_)
            | PluginCommand::RunCommandInPane { .. }
            | PluginCommand::EditScrollbackForPaneWithId(_)
            | PluginCommand::GetPaneScrollback { .. }
            | PluginCommand::WriteToPaneId(..)
            | PluginCommand::WriteCharsToPaneId(..)
            | PluginCommand::SendSigintToPaneId(_)
            | PluginCommand::SendSigkillToPaneId(_)
            | PluginCommand::GetPanePid { .. }
            | PluginCommand::GetPaneRunningCommand { .. }
            | PluginCommand::GetPaneCwd { .. }
            | PluginCommand::GetPaneGitProbe { .. }
            | PluginCommand::MovePaneWithPaneId(_)
            | PluginCommand::MovePaneWithPaneIdInDirection(..)
            | PluginCommand::ClearScreenForPaneId(_)
            | PluginCommand::ScrollUpInPaneId(_)
            | PluginCommand::ScrollDownInPaneId(_)
            | PluginCommand::ScrollToTopInPaneId(_)
            | PluginCommand::ScrollToBottomInPaneId(_)
            | PluginCommand::PageScrollUpInPaneId(_)
            | PluginCommand::PageScrollDownInPaneId(_)
            | PluginCommand::TogglePaneIdFullscreen(_)
            | PluginCommand::TogglePaneEmbedOrEjectForPaneId(_)
            | PluginCommand::CloseTabWithIndex(_)
            | PluginCommand::BreakPanesToNewTab(..)
            | PluginCommand::BreakPanesToTabWithIndex(..)
            | PluginCommand::SwitchTabToId(_)
            | PluginCommand::GoToTabWithId(_)
            | PluginCommand::CloseTabWithId(_)
            | PluginCommand::RenameTabWithId(..)
            | PluginCommand::ArrangeProjectPanes(_)
            | PluginCommand::SetProjectContext(..)
            | PluginCommand::SetTabProjects(_)
            | PluginCommand::BreakPanesToTabWithId(..)
            | PluginCommand::ReloadPlugin(_)
            | PluginCommand::LoadNewPlugin { .. }
            | PluginCommand::RegisterDefaultKeybindings(_)
            | PluginCommand::RegisterAgentProvider(_)
            | PluginCommand::RegisterProjectProvider(_)
            | PluginCommand::GetProjectProviders
            | PluginCommand::ValidateProjectMatch(_)
            | PluginCommand::PublishProjectLocations(_)
            | PluginCommand::GetAgentProviders
            | PluginCommand::ListClients
            | PluginCommand::ChangeHostFolder(_)
            | PluginCommand::SetFloatingPanePinned(..)
            | PluginCommand::StackPanes(_)
            | PluginCommand::ChangeFloatingPanesCoordinates(_)
            | PluginCommand::ChangeOverlayCoordinates { .. }
            | PluginCommand::TogglePaneBorderless(_)
            | PluginCommand::SetPaneBorderless(..)
            | PluginCommand::OpenCommandPaneNearPlugin(..)
            | PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(..)
            | PluginCommand::OpenTerminalNearPlugin(_)
            | PluginCommand::OpenTerminalFloatingNearPlugin(..)
            | PluginCommand::OpenTerminalInPlaceOfPlugin(..)
            | PluginCommand::OpenCommandPaneFloatingNearPlugin(..)
            | PluginCommand::OpenCommandPaneInPlaceOfPlugin(..)
            | PluginCommand::OpenFileNearPlugin(..)
            | PluginCommand::OpenFileFloatingNearPlugin(..)
            | PluginCommand::OpenFileInPlaceOfPlugin(..)
            | PluginCommand::GroupAndUngroupPanes(..)
            | PluginCommand::HighlightAndUnhighlightPanes(..)
            | PluginCommand::RevealPaneWithId(..)
            | PluginCommand::CloseMultiplePanes(_)
            | PluginCommand::FloatMultiplePanes(_)
            | PluginCommand::EmbedMultiplePanes(_)
            | PluginCommand::SetSelfMouseSelectionSupport(_)
            | PluginCommand::InterceptKeyPresses
            | PluginCommand::ClearKeyPressesIntercepts
            | PluginCommand::ReplacePaneWithExistingPane(..)
            | PluginCommand::CopyToClipboard(_)
            | PluginCommand::OverrideLayout(..)
            | PluginCommand::SaveLayout { .. }
            | PluginCommand::DeleteLayout { .. }
            | PluginCommand::RenameLayout { .. }
            | PluginCommand::EditLayout { .. }
            | PluginCommand::GenerateRandomName
            | PluginCommand::DumpLayout(_)
            | PluginCommand::ParseLayout(_)
            | PluginCommand::GetLayoutDir
            | PluginCommand::GetUserPluginDir
            | PluginCommand::GetFocusedPaneInfo
            | PluginCommand::SaveSession
            | PluginCommand::CurrentSessionLastSavedTime
            | PluginCommand::SubscribePaneOutput(_)
            | PluginCommand::GetPaneInfo(_)
            | PluginCommand::GetTabInfo(_)
            | PluginCommand::GetSessionEnvironmentVariables
            | PluginCommand::GetPluginIdsForUrl(_)
            | PluginCommand::GetPluginInfo(_)
            | PluginCommand::Pick(_)
            | PluginCommand::GetPluginIdsForUrlWithConfiguration { .. }
            | PluginCommand::GetPluginAliasConfiguration(_)
            | PluginCommand::OpenCommandPaneInNewTab(..)
            | PluginCommand::OpenPluginPaneInNewTab { .. }
            | PluginCommand::OpenEditorPaneInNewTab(..)
            | PluginCommand::OpenCommandPaneInPlaceOfPaneId(..)
            | PluginCommand::OpenTerminalPaneInPlaceOfPaneId(..)
            | PluginCommand::OpenEditPaneInPlaceOfPaneId(..)
            | PluginCommand::HideFloatingPanes { .. }
            | PluginCommand::ShowFloatingPanes { .. }
            | PluginCommand::SetPaneColor(..)
            | PluginCommand::SetPaneRegexHighlights(..)
            | PluginCommand::ClearPaneHighlights(_)
            | PluginCommand::OpenPluginPaneFloating { .. }
            | PluginCommand::GetSessionInventory(_)
            | PluginCommand::SwitchSessionDestination(_)
            | PluginCommand::GetSessionList
            | PluginCommand::GetDisconnectTargets
            | PluginCommand::KillSessionsAndReply(_)
            | PluginCommand::DeleteDeadSessionAndReply(_)
            | PluginCommand::DeleteAllDeadSessionsAndReply
            | PluginCommand::FocusHostSession
            | PluginCommand::ResolveNotification { .. }
            | PluginCommand::ListNotifications
            | PluginCommand::WarpPaneToSession { .. }
            | PluginCommand::WarpPaneFromSession { .. }
            | PluginCommand::WarpTabToSession { .. }
            | PluginCommand::WarpTabFromSession { .. }
            | PluginCommand::ListOrphanedPanes
            | PluginCommand::AdoptOrphanedPane { .. }
            | PluginCommand::DataDefineEntity { .. }
            | PluginCommand::DataApply { .. }
            | PluginCommand::DataFetch { .. }
            | PluginCommand::DataCount { .. }
            | PluginCommand::DataWatch { .. }
            | PluginCommand::DataUnwatch { .. }
            | PluginCommand::DataCatalog { .. }
            | PluginCommand::SetPaneTitlePrefix { .. }
            | PluginCommand::SetPaneSubtitle { .. }) => carried,
            // `CursorPosition` is a pair of `uint32`, so both coordinates are cut to 32 bits.
            PluginCommand::ShowCursor(position) => PluginCommand::ShowCursor(
                position.map(|(x, y)| (x as u32 as usize, y as u32 as usize)),
            ),
            PluginCommand::ResizeWithDirection(strategy) => {
                PluginCommand::ResizeWithDirection(resize_strategy(strategy))
            },
            PluginCommand::ResizePaneIdWithDirection(strategy, pane_id) => {
                PluginCommand::ResizePaneIdWithDirection(resize_strategy(strategy), pane_id)
            },
            // `KeyToRebind.actions` is `repeated Action`: an action `action.proto` cannot
            // carry is dropped, and the rest cross as the `Action` property says they do.
            PluginCommand::RebindKeys {
                keys_to_rebind,
                keys_to_unbind,
                write_config_to_disk,
            } => PluginCommand::RebindKeys {
                keys_to_rebind: keys_to_rebind
                    .into_iter()
                    .map(|(mode, key, actions)| {
                        (
                            mode,
                            key,
                            actions
                                .into_iter()
                                .filter_map(action_as_the_plugin_wire_carries_it)
                                .collect(),
                        )
                    })
                    .collect(),
                keys_to_unbind,
                write_config_to_disk,
            },
            // `RunActionPayload.action` is one `Action`, and the encoder refuses the
            // command when `action.proto` cannot carry it.
            PluginCommand::RunAction(action, context) => {
                PluginCommand::RunAction(action_as_the_plugin_wire_carries_it(action)?, context)
            },
            PluginCommand::SetContextMenu { request_id, items } => PluginCommand::SetContextMenu {
                request_id,
                items: items.into_iter().map(context_menu_item).collect(),
            },
        })
    }

    /// Every variant, every case: the kinds are walked rather than sampled, so a variant
    /// the wire mishandles cannot hide behind an unlucky draw.
    #[hegel::test]
    fn every_plugin_command_survives_a_protobuf_round_trip(tc: hegel::TestCase) {
        use prost::Message;
        for kind in CommandType::iter() {
            let command = plugin_command_of_kind(&tc, kind);
            let expected = as_the_plugin_wire_carries_it(command.clone());
            let (protobuf, expected) =
                match (ProtobufPluginCommand::try_from(command.clone()), expected) {
                    (Err(_), None) => continue,
                    (Err(e), Some(_)) => panic!("{command:?} failed to encode: {e}"),
                    (Ok(_), None) => panic!(
                    "{command:?} encoded, but this test says plugin_command.proto cannot carry it"
                ),
                    (Ok(protobuf), Some(expected)) => (protobuf, expected),
                };
            let bytes = protobuf.encode_to_vec();
            let decoded: ProtobufPluginCommand = Message::decode(bytes.as_slice())
                .unwrap_or_else(|e| panic!("{command:?} failed to decode: {e}"));
            let round_tripped: PluginCommand = decoded
                .try_into()
                .unwrap_or_else(|e| panic!("{command:?} failed to convert back: {e}"));
            assert_eq!(
                round_tripped, expected,
                "the plugin wire changed {command:?}"
            );
        }
    }
}
