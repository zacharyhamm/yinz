use super::*;
use crate::command::CommandToRun;
use crate::data::{FileToOpen, PermissionType};
use crate::internal_ids::{TabId, TabPosition, TerminalId};

const ME: PluginId = PluginId::new(7);
const OTHER_PLUGIN: PluginId = PluginId::new(8);

fn coordinates() -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::default()
}

/// Every command that addresses a pane by id, aimed at `target`.
fn targeted(target: PaneId) -> Vec<PluginCommand> {
    vec![
        PluginCommand::HidePaneWithId(target),
        PluginCommand::ShowPaneWithId(target, true, true),
        PluginCommand::SetFloatingPanePinned(target, true),
        PluginCommand::ChangeFloatingPanesCoordinates(vec![
            (PaneId::Terminal(TerminalId::new(1)), coordinates()),
            (target, coordinates()),
        ]),
        PluginCommand::ChangeOverlayCoordinates {
            placements: vec![(target, coordinates())],
            override_user_placement: false,
        },
        PluginCommand::BreakPanesToNewTab(vec![target], None, false),
        PluginCommand::BreakPanesToTabWithIndex(vec![target], TabPosition::new(0), false),
        PluginCommand::BreakPanesToTabWithId(vec![target], TabId::new(0), false),
        PluginCommand::SetPaneTitlePrefix {
            pane_id: target,
            prefix: Some("x".to_owned()),
        },
        PluginCommand::SetPaneSubtitle {
            pane_id: target,
            subtitle: None,
        },
        PluginCommand::HighlightAndUnhighlightPanes(vec![target], vec![]),
        PluginCommand::HighlightAndUnhighlightPanes(vec![], vec![target]),
    ]
}

#[test]
fn own_pane_commands_that_always_mean_the_calling_plugin() {
    let always = [
        (PluginCommand::HideSelf, "HideSelf"),
        (PluginCommand::ShowSelf(true), "ShowSelf"),
        (PluginCommand::SetSelectable(false), "SetSelectable"),
        (PluginCommand::ShowCursor(None), "ShowCursor"),
        (
            PluginCommand::SetSelfMouseSelectionSupport(true),
            "SetSelfMouseSelectionSupport",
        ),
        (
            PluginCommand::OpenTerminalInPlaceOfPlugin(FileToOpen::default(), false),
            "OpenTerminalInPlaceOfPlugin",
        ),
        (
            PluginCommand::OpenFileInPlaceOfPlugin(FileToOpen::default(), true, Default::default()),
            "OpenFileInPlaceOfPlugin",
        ),
        (
            PluginCommand::OpenCommandPaneInPlaceOfPlugin(
                CommandToRun::default(),
                false,
                Default::default(),
            ),
            "OpenCommandPaneInPlaceOfPlugin",
        ),
    ];
    for (command, name) in &always {
        assert_eq!(own_pane_command(command, ME), Some(*name), "{command:?}");
        // Whoever the caller is: these never depend on a target.
        assert_eq!(
            own_pane_command(command, OTHER_PLUGIN),
            Some(*name),
            "{command:?}"
        );
    }
    // The published list names exactly these commands, so a check that matches names (as
    // `plugins check` does) flags each command the host refuses whatever its arguments.
    let mut named: Vec<_> = always.iter().map(|(_, name)| *name).collect();
    let mut listed = UNCONDITIONAL_OWN_PANE_COMMANDS.to_vec();
    named.sort_unstable();
    listed.sort_unstable();
    assert_eq!(named, listed);
}

#[test]
fn the_refusal_names_the_command_the_plugin_and_the_fix() {
    assert_eq!(
        own_pane_refusal("HideSelf", "clock"),
        "'HideSelf' needs the plugin's own pane, but clock declares no pane; add \
         (yinz-declare-pane) at the top level of its entry file"
    );
}

#[test]
fn own_pane_commands_by_id_are_gated_only_when_they_target_the_caller() {
    for command in targeted(PaneId::Plugin(ME)) {
        assert!(
            own_pane_command(&command, ME).is_some(),
            "{command:?} targets the caller's pane"
        );
    }
    for target in [
        PaneId::Plugin(OTHER_PLUGIN),
        PaneId::Terminal(TerminalId::new(ME.get())),
    ] {
        for command in targeted(target) {
            assert_eq!(own_pane_command(&command, ME), None, "{command:?}");
        }
    }
    assert_eq!(
        own_pane_command(&PluginCommand::RenamePluginPane(ME, "x".to_owned()), ME),
        Some("RenamePluginPane")
    );
    assert_eq!(
        own_pane_command(&PluginCommand::FocusPluginPane(ME, true, false), ME),
        Some("FocusPluginPane")
    );
    assert_eq!(
        own_pane_command(
            &PluginCommand::RenamePluginPane(OTHER_PLUGIN, "x".to_owned()),
            ME
        ),
        None
    );
    assert_eq!(
        own_pane_command(
            &PluginCommand::FocusPluginPane(OTHER_PLUGIN, true, false),
            ME
        ),
        None
    );
}

#[test]
fn own_pane_classifier_leaves_commands_a_headless_plugin_can_use() {
    let headless_safe = [
        PluginCommand::CloseSelf,
        PluginCommand::ClosePluginPane(ME),
        PluginCommand::InterceptKeyPresses,
        PluginCommand::ClearKeyPressesIntercepts,
        PluginCommand::Notify("hello".to_owned(), None, None),
        PluginCommand::RequestPluginPermissions(vec![PermissionType::ReadApplicationState]),
    ];
    for command in headless_safe {
        assert_eq!(own_pane_command(&command, ME), None, "{command:?}");
    }
}
