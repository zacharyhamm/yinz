use super::*;
use yinz_types::input::options::PaneFrameStyle;

#[test]
fn overlay_opacity_is_optional_and_validated_at_the_plugin_boundary() {
    for opacity in [None, Some(0), Some(75), Some(100)] {
        let wire = ProtobufOverlayPlacement {
            background_opacity: opacity,
            ..Default::default()
        };
        let placement = OverlayPlacement::try_from(wire).unwrap();
        assert_eq!(placement.background_opacity.map(u32::from), opacity);
    }
    for opacity in [101, 255, 256, u32::MAX] {
        assert!(OverlayPlacement::try_from(ProtobufOverlayPlacement {
            background_opacity: Some(opacity),
            ..Default::default()
        })
        .is_err());
    }
}

#[test]
fn pane_tabs_layout_decoding_validates_nested_and_floating_children() {
    for json in [
        r#"{"tabs":[],"active":0}"#,
        r#"{"tabs":[{"label":"First","location":"yinz:strider"}],"active":1}"#,
        r#"{"tabs":[{"label":" ","location":"yinz:strider"}],"active":0}"#,
        r#"{"tabs":[{"label":"First","location":" "}],"active":0}"#,
    ] {
        let mut tiled = ProtobufTiledPaneLayout::try_from(TiledPaneLayout::default()).unwrap();
        tiled.pane_tabs_json = Some(json.into());
        assert!(TiledPaneLayout::try_from(tiled.clone()).is_err());
        let mut parent = ProtobufTiledPaneLayout::try_from(TiledPaneLayout::default()).unwrap();
        parent.children = vec![tiled];
        assert!(TiledPaneLayout::try_from(parent).is_err());
        let mut floating =
            ProtobufFloatingPaneLayout::try_from(FloatingPaneLayout::default()).unwrap();
        floating.pane_tabs_json = Some(json.into());
        assert!(FloatingPaneLayout::try_from(floating).is_err());
    }
    let json = r#"{"tabs":[{"label":"First","location":"yinz:strider"}],"active":0}"#;
    let mut tiled = ProtobufTiledPaneLayout::try_from(TiledPaneLayout::default()).unwrap();
    tiled.pane_tabs_json = Some(json.into());
    assert!(TiledPaneLayout::try_from(tiled).is_ok());
    let mut floating = ProtobufFloatingPaneLayout::try_from(FloatingPaneLayout::default()).unwrap();
    floating.pane_tabs_json = Some(json.into());
    assert!(FloatingPaneLayout::try_from(floating).is_ok());
}

#[test]
fn pane_ids_preserve_protobuf_bytes() {
    use prost::Message;
    use yinz_types::internal_ids::{PluginId, TerminalId};
    use {crate::event, crate::plugin_command};

    for (pane, oneof_bytes, enum_bytes) in [
        (PaneId::Terminal(TerminalId::new(0)), vec![8, 0], vec![]),
        (PaneId::Plugin(PluginId::new(0)), vec![16, 0], vec![8, 1]),
        (
            PaneId::Terminal(TerminalId::new(u32::MAX)),
            vec![8, 255, 255, 255, 255, 15],
            vec![16, 255, 255, 255, 255, 15],
        ),
        (
            PaneId::Plugin(PluginId::new(u32::MAX)),
            vec![16, 255, 255, 255, 255, 15],
            vec![8, 1, 16, 255, 255, 255, 255, 15],
        ),
    ] {
        let action: ProtobufPaneId = pane.try_into().unwrap();
        assert_eq!(action.encode_to_vec(), oneof_bytes);
        assert_eq!(
            PaneId::try_from(ProtobufPaneId::decode(oneof_bytes.as_slice()).unwrap()).unwrap(),
            pane
        );
        let event: event::ProtobufPaneId = pane.try_into().unwrap();
        assert_eq!(event.encode_to_vec(), enum_bytes);
        assert_eq!(
            PaneId::try_from(event::ProtobufPaneId::decode(enum_bytes.as_slice()).unwrap())
                .unwrap(),
            pane
        );
        let command: plugin_command::ProtobufPaneId = pane.try_into().unwrap();
        assert_eq!(command.encode_to_vec(), enum_bytes);
        assert_eq!(
            PaneId::try_from(
                plugin_command::ProtobufPaneId::decode(enum_bytes.as_slice()).unwrap()
            )
            .unwrap(),
            pane
        );
    }
}

/// `RunPlugin`'s `PartialEq` ignores `relative_path`, so an `assert_eq!` on a round-tripped
/// plugin cannot see the field dropped - and dropping it silently costs a plugin named by a
/// relative `file:` URL its lookup in the user's plugin folder. Assert the field itself.
#[test]
fn git_plugins_survive_the_plugin_wire() {
    let original = RunPluginLocation::Git(yinz_types::input::git_plugin::GitPlugin {
        repo: "git@example.com:team/plugin.git".into(),
        entrypoint: "src/panel.scm".into(),
        revision: Some(yinz_types::input::git_plugin::GitRevision::Tag("v1".into())),
    });
    let protobuf: ProtobufRunPluginLocationData = original.clone().try_into().unwrap();
    assert_eq!(RunPluginLocation::try_from(protobuf).unwrap(), original);
}

#[test]
fn a_relative_plugin_path_survives_the_plugin_api_round_trip() {
    let original = RunPlugin {
        location: RunPluginLocation::File(PathBuf::from("/somewhere/keys.scm")),
        relative_path: Some(PathBuf::from("keys.scm")),
        declared_path: Some(PathBuf::from("/somewhere/current.scm")),
        ..Default::default()
    };
    let protobuf: ProtobufRunPlugin = original.clone().try_into().expect("encode");
    let decoded: RunPlugin = protobuf.try_into().expect("decode");
    assert_eq!(decoded.relative_path, original.relative_path);
    assert_eq!(decoded.declared_path, original.declared_path);
}

#[test]
fn plugin_url_actions_preserve_declared_paths_and_reserved_characters() {
    let path = PathBuf::from("/somewhere/current # %20?.scm");
    let action = Action::StartOrReloadPlugin {
        plugin: RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(PathBuf::from("/somewhere/v1.scm")),
            declared_path: Some(path.clone()),
            ..Default::default()
        }),
    };
    let protobuf: ProtobufAction = action.try_into().unwrap();
    let Action::StartOrReloadPlugin { plugin } = Action::try_from(protobuf).unwrap() else {
        panic!("expected reload action");
    };
    assert_eq!(plugin.get_run_plugin().unwrap().declared_path, Some(path));
}

#[test]
fn new_tab_layout_source_is_optional_on_the_plugin_wire() {
    for layout_source in [None, Some("custom".to_owned())] {
        let protobuf = ProtobufAction {
            name: ProtobufActionName::NewTab as i32,
            optional_payload: Some(OptionalPayload::NewTabPayload(NewTabPayload {
                layout_source: layout_source.clone(),
                ..Default::default()
            })),
        };
        let action: Action = protobuf.try_into().unwrap();
        assert!(
            matches!(&action, Action::NewTab { layout_source: source, .. }
                if *source == layout_source.as_ref().map(PathBuf::from))
        );
        let encoded: ProtobufAction = action.try_into().unwrap();
        assert!(matches!(encoded.optional_payload,
                Some(OptionalPayload::NewTabPayload(payload)) if payload.layout_source == layout_source));
    }
    let legacy = ProtobufAction {
        name: ProtobufActionName::NewTab as i32,
        optional_payload: None,
    };
    assert!(matches!(
        Action::try_from(legacy).unwrap(),
        Action::NewTab {
            layout_source: None,
            ..
        }
    ));
}

#[test]
fn set_pane_frame_style_action_protobuf_round_trip() {
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let original = Action::SetPaneFrameStyle(style);
        let protobuf: ProtobufAction = original.clone().try_into().expect("encode");
        let decoded: Action = protobuf.try_into().expect("decode");
        assert_eq!(original, decoded);
    }
}

#[test]
fn osc133_actions_protobuf_round_trip() {
    for original in [
        Action::ScrollToPreviousPrompt,
        Action::ScrollToNextPrompt,
        Action::SelectCommandAtScrollPosition,
        Action::CopyLastCommandOutput,
    ] {
        let protobuf: ProtobufAction = original.clone().try_into().expect("encode");
        let decoded: Action = protobuf.try_into().expect("decode");
        assert_eq!(original, decoded);
    }
}

pub(crate) mod props {
    use super::*;
    use crate::test_generators::action_of_kind;
    use strum::IntoEnumIterator;
    use yinz_types::input::actions::ActionKind;
    use yinz_types::input::command::RunCommand;

    /// `RunCommandAction` on the plugin wire (`message RunCommandAction`) has `command`,
    /// `args`, `cwd`, `direction`, `hold_on_close` and `hold_on_start`, and nothing else.
    fn run_command_action(command: RunCommandAction) -> RunCommandAction {
        RunCommandAction {
            originating_plugin: None,
            use_terminal_title: false,
            remote_default_shell: false,
            ..command
        }
    }

    /// A layout's `Run::Command` crosses as the same `RunCommandAction` message.
    fn run(run: Run) -> Run {
        match run {
            Run::Command(command) => Run::Command(RunCommand {
                originating_plugin: None,
                use_terminal_title: false,
                remote_default_shell: false,
                ..command
            }),
            other => other,
        }
    }

    /// `message TiledPaneLayout` has `bool borderless`, not an optional one, and no
    /// `default_fg` / `default_bg`.
    fn tiled_pane_layout(layout: TiledPaneLayout) -> TiledPaneLayout {
        TiledPaneLayout {
            children: layout.children.into_iter().map(tiled_pane_layout).collect(),
            run: layout.run.map(run),
            borderless: Some(layout.borderless.unwrap_or(false)),
            default_fg: None,
            default_bg: None,
            ..layout
        }
    }

    /// `message FloatingPaneLayout` has no `default_fg` / `default_bg`.
    fn floating_pane_layout(layout: FloatingPaneLayout) -> FloatingPaneLayout {
        FloatingPaneLayout {
            run: layout.run.map(run),
            default_fg: None,
            default_bg: None,
            ..layout
        }
    }

    fn floating_pane_layouts(layouts: Vec<FloatingPaneLayout>) -> Vec<FloatingPaneLayout> {
        layouts.into_iter().map(floating_pane_layout).collect()
    }

    fn swap_tiled_layout((layouts, name): SwapTiledLayout) -> SwapTiledLayout {
        (
            layouts
                .into_iter()
                .map(|(constraint, layout)| (constraint, tiled_pane_layout(layout)))
                .collect(),
            name,
        )
    }

    fn swap_floating_layout((layouts, name): SwapFloatingLayout) -> SwapFloatingLayout {
        (
            layouts
                .into_iter()
                .map(|(constraint, layouts)| (constraint, floating_pane_layouts(layouts)))
                .collect(),
            name,
        )
    }

    fn command_or_plugin(pane: CommandOrPlugin) -> CommandOrPlugin {
        match pane {
            CommandOrPlugin::Command(command) => {
                CommandOrPlugin::Command(run_command_action(command))
            },
            other => other,
        }
    }

    // URL decoding resolves file identities against the receiving process cwd.
    fn plugin_by_url(mut plugin: RunPluginOrAlias) -> RunPluginOrAlias {
        if let RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(path),
            ..
        }) = &mut plugin
        {
            *path = std::env::current_dir().unwrap().join(&*path);
            *path = path.canonicalize().unwrap_or_else(|_| path.clone());
        }
        plugin
    }

    /// A plugin named only by `string plugin_url` (`NewPluginPanePayload`,
    /// `start_or_reload_plugin_payload`) comes back through `yinz_plugin_location::run_plugin_or_alias_from_url`
    /// with no configuration at all.
    fn plugin_by_url_alone(plugin: RunPluginOrAlias) -> RunPluginOrAlias {
        match plugin_by_url(plugin) {
            RunPluginOrAlias::RunPlugin(run_plugin) => RunPluginOrAlias::RunPlugin(RunPlugin {
                configuration: PluginUserConfiguration::default(),
                ..run_plugin
            }),
            RunPluginOrAlias::Alias(alias) => RunPluginOrAlias::Alias(PluginAlias {
                configuration: None,
                ..alias
            }),
        }
    }

    /// What the plugin wire hands back for `action`, or `None` where `action.proto` has no
    /// encoding for it at all.
    ///
    /// Written against the payload messages, not against the conversion: every field
    /// zeroed here is one its payload has no room for, so the round trip is exact
    /// everywhere else, and a field the payload *does* carry going missing still fails.
    pub(crate) fn as_the_plugin_wire_carries_it(action: Action) -> Option<Action> {
        Some(match action {
            carried @ (Action::NewTabGroup
            | Action::MovePaneToNextTabGroup
            | Action::MovePaneToPreviousTabGroup
            | Action::GoToNextTabGroup
            | Action::GoToPreviousTabGroup
            | Action::GoToTabInGroup { .. }
            | Action::SwitchTabGroup { .. }
            | Action::RenameTabGroup { .. }
            | Action::CloseTabGroup { .. }
            | Action::Quit
            | Action::Write { .. }
            | Action::WriteChars { .. }
            | Action::SwitchToMode { .. }
            | Action::SwitchModeForAllClients { .. }
            | Action::Resize { .. }
            | Action::FocusNextPane
            | Action::FocusPreviousPane
            | Action::FocusPaneTabs
            | Action::PreviousPaneTab
            | Action::NextPaneTab
            | Action::FocusLastPane
            | Action::SwitchFocus
            | Action::MoveFocus { .. }
            | Action::MoveFocusOrTab { .. }
            | Action::MovePane { .. }
            | Action::MovePaneBackwards
            | Action::ClearScreen
            | Action::DumpScreen { .. }
            | Action::ScrollUp
            | Action::ScrollUpAt { .. }
            | Action::ScrollDown
            | Action::ScrollDownAt { .. }
            | Action::ScrollToBottom
            | Action::ScrollToTop
            | Action::ScrollToPreviousPrompt
            | Action::ScrollToNextPrompt
            | Action::SelectCommandAtScrollPosition
            | Action::CopyLastCommandOutput
            | Action::MoveTextSelection { .. }
            | Action::SwapTextSelectionEnds
            | Action::MoveNotificationSelection { .. }
            | Action::ActivateNotification
            | Action::ActivateLastNotification
            | Action::ToggleNotificationDetail
            | Action::PageScrollUp
            | Action::PageScrollDown
            | Action::HalfPageScrollUp
            | Action::HalfPageScrollDown
            | Action::ToggleFocusFullscreen
            | Action::ToggleFocusNoUiFullscreen
            | Action::TogglePaneFrames
            | Action::ToggleCommandPalette
            | Action::OpenAgents
            | Action::SetPaneFrameStyle(_)
            | Action::ToggleActiveSyncTab
            | Action::TogglePaneEmbedOrFloating
            | Action::ToggleFloatingPanes
            | Action::ShowFloatingPanes { .. }
            | Action::HideFloatingPanes { .. }
            | Action::AreFloatingPanesVisible { .. }
            | Action::CloseFocus
            | Action::PaneNameInput { .. }
            | Action::UndoRenamePane
            | Action::NoOp
            | Action::Copy
            | Action::GoToNextTab
            | Action::GoToPreviousTab
            | Action::CloseTab
            | Action::GoToTab { .. }
            | Action::GoToTabName { .. }
            | Action::ToggleTab
            | Action::TabNameInput { .. }
            | Action::UndoRenameTab
            | Action::MoveTab {
                direction: Direction::Left | Direction::Right,
            }
            | Action::Detach
            | Action::SetDarkTheme
            | Action::SetLightTheme
            | Action::ToggleTheme
            | Action::MouseEvent { .. }
            | Action::SearchInput { .. }
            | Action::Search { .. }
            | Action::SearchToggleOption { .. }
            | Action::ToggleMouseMode
            | Action::PreviousSwapLayout
            | Action::NextSwapLayout
            | Action::QueryTabNames
            | Action::CloseTerminalPane { .. }
            | Action::ClosePluginPane { .. }
            | Action::FocusTerminalPaneWithId { .. }
            | Action::FocusPluginPaneWithId { .. }
            | Action::RenameTerminalPane { .. }
            | Action::RenamePluginPane { .. }
            | Action::RenameTab { .. }
            | Action::GoToTabById { .. }
            | Action::CloseTabById { .. }
            | Action::RenameTabById { .. }
            | Action::ArrangePanesByProject
            | Action::BreakPane
            | Action::BreakPaneRight
            | Action::BreakPaneLeft
            | Action::KeybindPipe { .. }
            | Action::FocusHostSession
            | Action::FocusGuestSession
            | Action::ToggleHostFullscreen
            | Action::RenameSession { .. }
            | Action::Notify(_)
            | Action::TogglePanePinned
            | Action::SetPaneBorderless { .. }
            | Action::TogglePaneInGroup
            | Action::ToggleGroupMarking) => carried,
            // `MoveTabDirection` is `Left` or `Right`; the encoder refuses the rest.
            Action::MoveTab { .. } => return None,
            // `EditScrollback` has no payload.
            Action::EditScrollback { .. } => Action::EditScrollback { ansi: false },
            // `NewPanePayload` has `direction` and `pane_name`.
            Action::NewPane {
                direction,
                pane_name,
                ..
            } => Action::NewPane {
                direction,
                pane_name,
                start_suppressed: false,
            },
            // `NewBlockingPanePayload` has no `no_focus` and no `tab_id`.
            Action::NewBlockingPane {
                placement,
                pane_name,
                command,
                unblock_condition,
                near_current_pane,
                ..
            } => Action::NewBlockingPane {
                placement,
                pane_name,
                command: command.map(run_command_action),
                unblock_condition,
                near_current_pane,
                no_focus: false,
                tab_id: None,
            },
            // `EditFilePayload` has the file, `line_number`, `cwd`, `direction`,
            // `should_float` and `near_current_pane`.
            Action::EditFile {
                payload,
                direction,
                floating,
                near_current_pane,
                ..
            } => Action::EditFile {
                payload: OpenFilePayload {
                    originating_plugin: None,
                    ..payload
                },
                direction,
                floating,
                in_place: false,
                close_replaced_pane: false,
                start_suppressed: false,
                placement: NewPanePlacement::Floating(None),
                near_current_pane,
                no_focus: false,
                tab_id: None,
            },
            // `NewFloatingPanePayload` has `command` and `near_current_pane`; the pane name
            // rides inside the command, so without one there is nowhere to put it.
            Action::NewFloatingPane {
                command,
                pane_name,
                near_current_pane,
                ..
            } => Action::NewFloatingPane {
                pane_name: pane_name.filter(|_| command.is_some()),
                command: command.map(run_command_action),
                placement: NewPanePlacement::Floating(None),
                near_current_pane,
                no_focus: false,
                tab_id: None,
            },
            // `NewTiledPanePayload` adds `direction` and `borderless` to the same shape.
            Action::NewTiledPane {
                direction,
                command,
                pane_name,
                near_current_pane,
                borderless,
                ..
            } => Action::NewTiledPane {
                direction,
                pane_name: pane_name.filter(|_| command.is_some()),
                command: command.map(run_command_action),
                near_current_pane,
                no_focus: false,
                borderless,
                tab_id: None,
            },
            // `NewInPlacePanePayload` has no `no_focus` and no `tab_id`.
            Action::NewInPlacePane {
                command,
                pane_name,
                near_current_pane,
                pane_id_to_replace,
                close_replaced_pane,
                ..
            } => Action::NewInPlacePane {
                command: command.map(run_command_action),
                pane_name,
                near_current_pane,
                no_focus: false,
                pane_id_to_replace,
                close_replaced_pane,
                tab_id: None,
            },
            // `NewStackedPanePayload` is empty (its one field is reserved).
            Action::NewStackedPane { .. } => Action::NewStackedPane {
                command: None,
                pane_name: None,
                near_current_pane: false,
                no_focus: false,
                tab_id: None,
            },
            Action::NewTab {
                layout_source,
                tiled_layout,
                floating_layouts,
                swap_tiled_layouts,
                swap_floating_layouts,
                tab_name,
                should_change_focus_to_new_tab,
                cwd,
                initial_panes,
                first_pane_unblock_condition,
            } => Action::NewTab {
                layout_source,
                tiled_layout: tiled_layout.map(tiled_pane_layout),
                floating_layouts: floating_pane_layouts(floating_layouts),
                swap_tiled_layouts: swap_tiled_layouts
                    .map(|layouts| layouts.into_iter().map(swap_tiled_layout).collect()),
                swap_floating_layouts: swap_floating_layouts
                    .map(|layouts| layouts.into_iter().map(swap_floating_layout).collect()),
                tab_name,
                should_change_focus_to_new_tab,
                cwd,
                initial_panes: initial_panes
                    .map(|panes| panes.into_iter().map(command_or_plugin).collect()),
                first_pane_unblock_condition,
            },
            // `run_payload` is a bare `RunCommandAction`.
            Action::Run { command, .. } => Action::Run {
                command: run_command_action(command),
                near_current_pane: false,
                no_focus: false,
            },
            // `LaunchOrFocusPluginPayload` has no `close_replaced_pane`, `cwd`, `no_focus`
            // or `tab_id`.
            Action::LaunchOrFocusPlugin {
                plugin,
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                skip_cache,
                ..
            } => Action::LaunchOrFocusPlugin {
                plugin: plugin_by_url(plugin),
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane: false,
                skip_cache,
                tab_id: None,
            },
            Action::LaunchPlugin {
                plugin,
                should_float,
                should_open_in_place,
                skip_cache,
                ..
            } => Action::LaunchPlugin {
                plugin: plugin_by_url(plugin),
                should_float,
                should_open_in_place,
                close_replaced_pane: false,
                skip_cache,
                cwd: None,
                no_focus: false,
                tab_id: None,
            },
            Action::OverrideLayout {
                tabs,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
            } => Action::OverrideLayout {
                tabs: tabs
                    .into_iter()
                    .map(|tab| TabLayoutInfo {
                        tiled_layout: tiled_pane_layout(tab.tiled_layout),
                        floating_layouts: floating_pane_layouts(tab.floating_layouts),
                        swap_tiled_layouts: tab
                            .swap_tiled_layouts
                            .map(|layouts| layouts.into_iter().map(swap_tiled_layout).collect()),
                        swap_floating_layouts: tab
                            .swap_floating_layouts
                            .map(|layouts| layouts.into_iter().map(swap_floating_layout).collect()),
                        ..tab
                    })
                    .collect(),
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
            },
            // `NewPluginPanePayload` has `plugin_url`, `pane_name` and `skip_plugin_cache`.
            Action::NewTiledPluginPane {
                plugin,
                pane_name,
                skip_cache,
                ..
            } => Action::NewTiledPluginPane {
                plugin: plugin_by_url_alone(plugin),
                pane_name,
                skip_cache,
                cwd: None,
                no_focus: false,
                tab_id: None,
            },
            Action::NewFloatingPluginPane {
                plugin,
                pane_name,
                skip_cache,
                ..
            } => Action::NewFloatingPluginPane {
                plugin: plugin_by_url_alone(plugin),
                pane_name,
                skip_cache,
                cwd: None,
                placement: NewPanePlacement::Floating(None),
                no_focus: false,
                tab_id: None,
            },
            Action::StartOrReloadPlugin { plugin } => Action::StartOrReloadPlugin {
                plugin: plugin_by_url_alone(plugin),
            },
            // No `ActionName` at all (`CliPipe` has one, but `CliPipePayload` cannot carry
            // a pipe id), so the encoder refuses these.
            Action::WriteToPaneId { .. }
            | Action::WriteCharsToPaneId { .. }
            | Action::Paste { .. }
            | Action::DumpLayout
            | Action::SaveSession
            | Action::SetPaneColor { .. }
            | Action::SetPaneAgent { .. }
            | Action::AgentEvent(_)
            | Action::Reconfigure { .. }
            | Action::SwitchSession { .. }
            | Action::WarpPane { .. }
            | Action::AdoptPane { .. }
            | Action::WarpTab { .. }
            | Action::AdoptTab { .. }
            | Action::WarpTabFrom { .. }
            | Action::Confirm
            | Action::Deny
            | Action::SkipConfirm { .. }
            | Action::NewInPlacePluginPane { .. }
            | Action::CliPipe { .. }
            | Action::ListClients
            | Action::ListPanes { .. }
            | Action::NewNamedTabGroup { .. }
            | Action::GoToTabGroupName { .. }
            | Action::RenameCurrentTabGroup { .. }
            | Action::CloseCurrentTabGroup
            | Action::ListTabGroups { .. }
            | Action::CurrentTabGroupInfo { .. }
            | Action::ListTabs { .. }
            | Action::CurrentTabInfo { .. }
            | Action::StackPanes { .. }
            | Action::ChangeFloatingPaneCoordinates { .. }
            | Action::TogglePaneBorderless { .. }
            | Action::ScrollUpByPaneId { .. }
            | Action::ScrollDownByPaneId { .. }
            | Action::ScrollToTopByPaneId { .. }
            | Action::ScrollToBottomByPaneId { .. }
            | Action::PageScrollUpByPaneId { .. }
            | Action::PageScrollDownByPaneId { .. }
            | Action::HalfPageScrollUpByPaneId { .. }
            | Action::HalfPageScrollDownByPaneId { .. }
            | Action::ResizeByPaneId { .. }
            | Action::MovePaneByPaneId { .. }
            | Action::MovePaneBackwardsByPaneId { .. }
            | Action::ClearScreenByPaneId { .. }
            | Action::EditScrollbackByPaneId { .. }
            | Action::ToggleFocusFullscreenByPaneId { .. }
            | Action::ToggleFocusNoUiFullscreenByPaneId { .. }
            | Action::TogglePaneEmbedOrFloatingByPaneId { .. }
            | Action::CloseFocusByPaneId { .. }
            | Action::RenamePaneByPaneId { .. }
            | Action::UndoRenamePaneByPaneId { .. }
            | Action::TogglePanePinnedByPaneId { .. }
            | Action::FocusPaneByPaneId { .. }
            | Action::UndoRenameTabByTabId { .. }
            | Action::ToggleActiveSyncTabByTabId { .. }
            | Action::ToggleFloatingPanesByTabId { .. }
            | Action::PreviousSwapLayoutByTabId { .. }
            | Action::NextSwapLayoutByTabId { .. }
            | Action::MoveTabByTabId { .. } => return None,
        })
    }

    /// Every variant, every case: the kinds are walked rather than sampled, so a variant
    /// the wire mishandles cannot hide behind an unlucky draw.
    #[hegel::test]
    fn every_action_survives_the_plugin_api_round_trip(tc: hegel::TestCase) {
        use prost::Message;
        for kind in ActionKind::iter() {
            let action = action_of_kind(&tc, kind);
            let expected = as_the_plugin_wire_carries_it(action.clone());
            let (protobuf, expected) = match (ProtobufAction::try_from(action.clone()), expected) {
                (Err(_), None) => continue,
                (Err(e), Some(_)) => panic!("{action:?} failed to encode: {e}"),
                (Ok(_), None) => {
                    panic!("{action:?} encoded, but this test says action.proto cannot carry it")
                },
                (Ok(protobuf), Some(expected)) => (protobuf, expected),
            };
            let bytes = protobuf.encode_to_vec();
            let decoded: ProtobufAction = Message::decode(bytes.as_slice())
                .unwrap_or_else(|e| panic!("{action:?} failed to decode: {e}"));
            let round_tripped: Action = decoded
                .try_into()
                .unwrap_or_else(|e| panic!("{action:?} failed to convert back: {e}"));
            assert_eq!(
                round_tripped, expected,
                "the plugin wire changed {action:?}"
            );
        }
    }
}
