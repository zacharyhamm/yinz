//! `yinz_config::kdl::action::to_kdl`'s contract, drawn from the plugin API's action
//! generator: every action either has no config spelling and says so, or reads back from the
//! line `to_kdl` writes as the very same value. It lives here rather than beside the serializer
//! because `yinz-config` may not depend on `yinz-plugin-api`, even for its tests.
use hegel::generators;
use kdl::KdlDocument;
use strum::IntoEnumIterator;
use yinz_config::kdl::action::{from_kdl, to_kdl, NotWritable, NOT_READ_BACK};
use yinz_types::input::actions::{Action, ActionKind};
use yinz_types::input::options::Options;

/// `to_kdl`'s contract, checked for one action: `Ok` reads back as the very same value,
/// and a refusal came from an arm that names the shape rather than from the round-trip
/// backstop - except for the three variants that carry a plugin reference, whose
/// loadability is the URL parser's to decide.
fn check_written_form(kind: ActionKind, action: &Action) {
    let node = match to_kdl(action) {
        Ok(node) => node,
        Err(NotWritable::Shape { why, .. }) if why == NOT_READ_BACK => {
            let plugin_reference = matches!(
                kind,
                ActionKind::LaunchOrFocusPlugin
                    | ActionKind::LaunchPlugin
                    | ActionKind::KeybindPipe
            );
            assert!(
                plugin_reference,
                "{action:?} was refused by the backstop; name its shape in `spell`"
            );
            return;
        },
        Err(_) => return,
    };
    let text = node.to_string();
    let document: KdlDocument = text.parse().unwrap_or_else(|e| panic!("{text}\n{e}"));
    let reread = from_kdl((&document.nodes()[0], &Options::default()))
        .unwrap_or_else(|e| panic!("{text}\n{e:?}"));
    assert_eq!(&reread, action, "{text}");
}

/// Every action either has no config spelling and says so, or reads back from the line
/// `to_kdl` writes as the very same value.
#[hegel::test(test_cases = 200)]
fn written_actions_round_trip_or_are_refused(tc: hegel::TestCase) {
    // `NewTab` and `SwitchSession` are read back through a layout loaded from disk, so what
    // they read back is a property of the disk; `Config::write_config_to_disk` compares
    // those against the source instead.
    let kinds: Vec<ActionKind> = ActionKind::iter()
        .filter(|kind| !matches!(kind, ActionKind::NewTab | ActionKind::SwitchSession))
        .collect();
    for _ in 0..10 {
        let kind = tc.draw(generators::sampled_from(kinds.clone()));
        let action = yinz_plugin_api::test_generators::action_of_kind(&tc, kind);
        check_written_form(kind, &action);
    }
}

/// The fields a config line can never carry - pane and tab ids, focus flags, a plugin's
/// identity - taken out, so that the draw lands on the shapes the deeper arms decide:
/// direction agreement, targets, coordinates, configuration keys, plugin references.
fn without_wire_only_fields(action: Action) -> Action {
    use yinz_types::input::command::RunCommandAction;
    fn command(command: Option<RunCommandAction>) -> Option<RunCommandAction> {
        command.map(|mut command| {
            command.originating_plugin = None;
            command.use_terminal_title = false;
            command.remote_default_shell = false;
            command
        })
    }
    match action {
        Action::NewTiledPane {
            direction,
            command: run,
            pane_name,
            ..
        } => Action::NewTiledPane {
            direction,
            command: command(run).map(|mut run| {
                run.direction = direction;
                run
            }),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        },
        Action::NewFloatingPane {
            command: run,
            pane_name,
            placement,
            ..
        } => Action::NewFloatingPane {
            command: command(run),
            pane_name,
            placement,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        Action::NewInPlacePane {
            command: run,
            pane_name,
            close_replaced_pane,
            ..
        } => Action::NewInPlacePane {
            command: command(run),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane,
            tab_id: None,
        },
        Action::NewStackedPane {
            command: run,
            pane_name,
            ..
        } => Action::NewStackedPane {
            command: command(run),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        },
        Action::LaunchOrFocusPlugin {
            plugin,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            ..
        } => Action::LaunchOrFocusPlugin {
            plugin,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            tab_id: None,
        },
        Action::LaunchPlugin {
            plugin,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            ..
        } => Action::LaunchPlugin {
            plugin,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            cwd: None,
            no_focus: false,
            tab_id: None,
        },
        Action::KeybindPipe {
            name,
            payload,
            plugin,
            plugin_id,
            configuration,
            launch_new,
            skip_cache,
            floating,
            cwd,
            pane_title,
            ..
        } => Action::KeybindPipe {
            name,
            payload,
            args: None,
            plugin,
            plugin_id,
            configuration,
            launch_new,
            skip_cache,
            floating,
            in_place: None,
            cwd,
            pane_title,
        },
        Action::Write { bytes, .. } => Action::Write {
            key_with_modifier: None,
            bytes,
            is_kitty_keyboard_protocol: false,
        },
        Action::NewPane { direction, .. } => Action::NewPane {
            direction,
            pane_name: None,
            start_suppressed: false,
        },
        Action::DumpScreen { file_path, .. } => Action::DumpScreen {
            file_path,
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        },
        other => other,
    }
}

/// The same contract, drawn from the variants with structured payloads, with the fields no
/// config line carries taken out first - otherwise almost every draw is refused for one of
/// those before the arms this property is about get a look.
#[hegel::test(test_cases = 300)]
fn written_pane_and_plugin_actions_read_back_or_name_their_shape(tc: hegel::TestCase) {
    let kinds = vec![
        ActionKind::NewTiledPane,
        ActionKind::NewFloatingPane,
        ActionKind::NewInPlacePane,
        ActionKind::NewStackedPane,
        ActionKind::LaunchOrFocusPlugin,
        ActionKind::LaunchPlugin,
        ActionKind::KeybindPipe,
        ActionKind::Write,
        ActionKind::NewPane,
        ActionKind::DumpScreen,
        ActionKind::MoveTab,
        ActionKind::GoToTab,
        ActionKind::ShowFloatingPanes,
        ActionKind::HideFloatingPanes,
        ActionKind::EditScrollback,
        ActionKind::RenameSession,
        ActionKind::Notify,
        ActionKind::WriteChars,
        ActionKind::SwitchToMode,
        ActionKind::Resize,
        ActionKind::SearchToggleOption,
        ActionKind::SetPaneFrameStyle,
        ActionKind::OverrideLayout,
    ];
    for _ in 0..10 {
        let kind = tc.draw(generators::sampled_from(kinds.clone()));
        let action =
            without_wire_only_fields(yinz_plugin_api::test_generators::action_of_kind(&tc, kind));
        check_written_form(kind, &action);
    }
}
