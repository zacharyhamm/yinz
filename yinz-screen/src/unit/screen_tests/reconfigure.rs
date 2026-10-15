use super::*;
use std::path::PathBuf;
use yinz_types::colors::PaletteColor;
use yinz_types::data::{ContextMenuItem, InputMode};
use yinz_types::input::options::{
    Clipboard, HostNotificationProtocol, NestedSessionHandling, NotificationPosition,
};
use yinz_types::internal_ids::TabId;
use yinz_types::style::{default_palette, ThemeHue};
use yinz_types::units::ElapsedSeconds;

/// A styling that differs from every other shade's, and from the default palette.
fn styling(shade: u8) -> Styling {
    let mut styling: Styling = default_palette().into();
    styling.text_unselected.base = PaletteColor::EightBit(shade);
    styling
}

/// Every field of `ScreenConfig` set to something other than what `test_screen_config` holds.
fn every_field_changed() -> ScreenConfig {
    ScreenConfig {
        agent_theme_mapping: yinz_types::agent::theme::AgentThemeMapping::Off,
        terminal_theme: None,
        terminal_themes: Default::default(),
        theme: styling(1),
        themes: std::collections::BTreeMap::from([("custom".into(), styling(1))]),
        theme_name: Some("custom".into()),
        theme_dark_name: Some("night".into()),
        theme_light_name: Some("day".into()),
        host_theme_dark: Some(styling(2)),
        host_theme_light: Some(styling(3)),
        explicit_theme_hue: Some(ThemeHue::Light),
        rounded_corners: true,
        hide_session_name: true,
        pane_frame_style: PaneFrameStyle::Full,
        max_agent_width: 100,
        simplified_ui: true,
        pane_animations: false,
        styled_underlines: false,
        background_opacity: 40,
        osc8_hyperlinks: false,
        default_mode: Some(InputMode::Locked),
        default_shell: Some(PathBuf::from("/bin/fish")),
        default_cwd: Some(PathBuf::from("/srv")),
        default_editor: Some(PathBuf::from("/usr/bin/vi")),
        default_layout_name: Some("compact".into()),
        layout_dir: Some(PathBuf::from("/layouts")),
        auto_layout: false,
        stacked_resize: false,
        stacked_pane_list: true,
        agent_pane_min_size: Size { cols: 10, rows: 5 },
        agent_double_escape: true,
        advanced_mouse_actions: false,
        mouse_scroll_resize: false,
        mouse_hover_effects: false,
        focus_follows_mouse: true,
        mouse_click_through: true,
        context_menu_on_right_click: true,
        context_menu_override_agent_mouse: false,
        context_menu: ContextMenu::from_items(vec![ContextMenuItem::new(
            "Only this",
            vec![Action::CloseFocus],
        )]),
        osc133_command_selection: false,
        word_separators: ",;".into(),
        copy_command: Some("wl-copy".into()),
        copy_clipboard: Some(Clipboard::Primary),
        copy_on_select: false,
        support_kitty_keyboard_protocol: false,
        support_kitty_graphics_protocol: false,
        host_notification_protocol: HostNotificationProtocol::Osc99,
        nested_session_handling: NestedSessionHandling::Descend,
        dangerously_enable_paste_buffer_read: true,
        visual_bell: false,
        session_is_mirrored: true,
        session_serialization: false,
        serialize_pane_viewport: true,
        scrollback_lines_to_serialize: Some(500),
        notification_position: NotificationPosition::BottomLeft,
        notification_timeout_secs: ElapsedSeconds::new(3),
    }
}

fn screen_with_one_tab() -> Screen {
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    screen
}

fn active_pane_replies(screen: &mut Screen, bytes: &[u8]) -> Vec<Vec<u8>> {
    let client_id = crate::ClientId::new(1);
    let pane = screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .get_active_pane_mut(client_id)
        .unwrap();
    pane.handle_pty_bytes(bytes.to_vec());
    pane.drain_messages_to_pty()
}

/// The forcing function: a field added to `ScreenConfig` has to be destructured here and
/// asserted against the screen, and `Screen::reconfigure` has to apply it, before this compiles
/// and passes. Eleven options used to be read at startup and never again.
#[test]
fn a_reload_applies_every_field_of_the_screen_config() {
    let mut screen = screen_with_one_tab();
    let client_id = crate::ClientId::new(1);
    let applied = every_field_changed();

    screen
        .reconfigure(applied.clone(), Keybinds::default(), client_id)
        .unwrap();

    let ScreenConfig {
        agent_theme_mapping,
        terminal_theme,
        terminal_themes,
        theme,
        themes,
        theme_name,
        theme_dark_name,
        theme_light_name,
        host_theme_dark,
        host_theme_light,
        explicit_theme_hue,
        rounded_corners,
        hide_session_name,
        pane_frame_style,
        max_agent_width,
        simplified_ui,
        pane_animations,
        styled_underlines,
        background_opacity,
        osc8_hyperlinks,
        default_mode,
        default_shell,
        default_cwd,
        default_editor,
        default_layout_name,
        layout_dir,
        auto_layout,
        stacked_resize,
        stacked_pane_list,
        agent_pane_min_size,
        agent_double_escape,
        advanced_mouse_actions,
        mouse_scroll_resize,
        mouse_hover_effects,
        focus_follows_mouse,
        mouse_click_through,
        context_menu_on_right_click,
        context_menu_override_agent_mouse,
        context_menu,
        osc133_command_selection,
        word_separators,
        copy_command,
        copy_clipboard,
        copy_on_select,
        support_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        host_notification_protocol,
        nested_session_handling,
        dangerously_enable_paste_buffer_read,
        visual_bell,
        session_is_mirrored,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        notification_position,
        notification_timeout_secs,
    } = applied;

    // an explicit light hue paints the pair's light theme over `theme`; the test below covers
    // `theme` on its own
    assert_eq!(Some(screen.style.colors), host_theme_light);
    assert_ne!(Some(theme), host_theme_light);
    assert_eq!(screen.themes, themes);
    assert_eq!(screen.theme_name, theme_name);
    assert_eq!(screen.theme_dark_name, theme_dark_name);
    assert_eq!(screen.theme_light_name, theme_light_name);
    assert_eq!(screen.style.terminal, terminal_theme);
    assert_eq!(screen.style.agent_theme_mapping, agent_theme_mapping);
    assert_eq!(screen.terminal_themes, terminal_themes);
    assert_eq!(screen.host_theme_dark_styling, host_theme_dark);
    assert_eq!(screen.host_theme_light_styling, host_theme_light);
    assert_eq!(screen.configured_explicit_theme_hue, explicit_theme_hue);
    assert_eq!(
        screen.default_mode_info.style.rounded_corners,
        rounded_corners
    );
    assert_eq!(
        screen.default_mode_info.style.hide_session_name,
        hide_session_name
    );
    assert_eq!(screen.pane_frame_style, pane_frame_style);
    assert_eq!(screen.max_agent_width, max_agent_width);
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().max_agent_width,
        max_agent_width
    );
    assert_eq!(screen.arrow_fonts, !simplified_ui);
    assert_eq!(screen.pane_animations, pane_animations);
    assert_eq!(screen.styled_underlines, styled_underlines);
    assert_eq!(screen.background_opacity, background_opacity);
    assert_eq!(screen.osc8_hyperlinks, osc8_hyperlinks);
    assert_eq!(screen.default_mode_info.base_mode, default_mode);
    assert_eq!(Some(screen.default_shell.clone()), default_shell);
    match &screen.default_shell_action {
        Some(TerminalAction::RunCommand(run_command)) => {
            assert_eq!(run_command.cwd, default_cwd);
        },
        other => panic!("the default shell action carries the cwd, got {other:?}"),
    }
    assert_eq!(screen.default_editor, default_editor);
    assert_eq!(screen.default_layout_name, default_layout_name);
    assert_eq!(screen.layout_dir, layout_dir);
    assert_eq!(screen.auto_layout, auto_layout);
    assert_eq!(*screen.stacked_resize.borrow(), stacked_resize);
    assert_eq!(*screen.stacked_pane_list.borrow(), stacked_pane_list);
    assert_eq!(screen.agent_pane_min_size, agent_pane_min_size);
    assert_eq!(screen.agent_double_escape, agent_double_escape);
    assert_eq!(screen.advanced_mouse_actions, advanced_mouse_actions);
    assert_eq!(screen.mouse_scroll_resize, mouse_scroll_resize);
    assert_eq!(screen.mouse_hover_effects, mouse_hover_effects);
    assert_eq!(screen.focus_follows_mouse, focus_follows_mouse);
    assert_eq!(screen.mouse_click_through, mouse_click_through);
    assert_eq!(
        screen.context_menu_on_right_click,
        context_menu_on_right_click
    );
    assert_eq!(
        screen.context_menu_override_agent_mouse,
        context_menu_override_agent_mouse
    );
    for tab in screen.tabs.values() {
        assert_eq!(
            tab.context_menu_override_agent_mouse,
            context_menu_override_agent_mouse
        );
    }
    assert_eq!(screen.context_menu.items(), context_menu.items());
    assert_eq!(screen.osc133_command_selection, osc133_command_selection);
    assert_eq!(screen.word_separators, word_separators);
    assert_eq!(screen.copy_options.command, copy_command);
    assert_eq!(Some(screen.copy_options.clipboard), copy_clipboard);
    assert_eq!(screen.copy_options.copy_on_select, copy_on_select);
    assert_eq!(
        screen.explicitly_disable_kitty_keyboard_protocol,
        !support_kitty_keyboard_protocol
    );
    assert_eq!(
        screen.support_kitty_graphics_protocol,
        support_kitty_graphics_protocol
    );
    assert_eq!(
        screen.host_notification_protocol,
        host_notification_protocol
    );
    assert_eq!(screen.nested_session_handling, nested_session_handling);
    assert_eq!(
        screen.paste_buffer_read_enabled,
        dangerously_enable_paste_buffer_read
    );
    assert_eq!(screen.visual_bell, visual_bell);
    assert_eq!(screen.session_is_mirrored, session_is_mirrored);
    assert_eq!(screen.session_serialization, session_serialization);
    assert_eq!(screen.serialize_pane_viewport, serialize_pane_viewport);
    assert_eq!(
        screen.scrollback_lines_to_serialize,
        scrollback_lines_to_serialize
    );
    assert_eq!(screen.notification_position, notification_position);
    assert_eq!(screen.notification_timeout_secs, notification_timeout_secs);
}

/// The option reaches a pane that is already open, and undoes what the app negotiated before
/// the reload.
#[test]
fn a_reload_that_turns_the_kitty_keyboard_protocol_off_reaches_an_open_pane() {
    let mut screen = screen_with_one_tab();
    let client_id = crate::ClientId::new(1);
    active_pane_replies(&mut screen, b"\x1b[>1u");
    assert_eq!(
        active_pane_replies(&mut screen, b"\x1b[?u"),
        vec![b"\x1b[?1u".to_vec()],
        "the app negotiated the protocol"
    );
    active_pane_replies(&mut screen, b"\x1b[?1049h");

    let mut config = test_screen_config();
    config.support_kitty_keyboard_protocol = false;
    screen
        .reconfigure(config, Keybinds::default(), client_id)
        .unwrap();
    active_pane_replies(&mut screen, b"\x1b[?1049l");

    assert_eq!(
        active_pane_replies(&mut screen, b"\x1b[?u"),
        vec![b"\x1b[?0u".to_vec()],
        "the reload turned it off in the open pane"
    );
    let pane = screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .get_active_pane_mut(client_id)
        .unwrap();
    let key = yinz_types::keys::KeyWithModifier::new(yinz_types::keys::BareKey::Enter);
    assert!(matches!(
        pane.adjust_input_to_terminal(&Some(key), vec![b'\r'], false, Some(client_id)),
        Some(crate::tab::AdjustedInput::WriteBytesToTerminal(bytes)) if bytes == b"\r"
    ));
}

#[test]
fn a_reload_enabling_mirroring_reconciles_tiled_focus_beneath_floating_panes() {
    for reload_client in [ClientId::new(2), ClientId::new(99)] {
        let clients = [ClientId::new(1), ClientId::new(2)];
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.add_client(clients[1]).unwrap();
        let tab = screen.get_active_tab_mut(clients[1]).unwrap();
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(clients[1]),
            None,
        )
        .unwrap();
        assert_ne!(
            tab.get_active_pane_id(clients[0]),
            tab.get_active_pane_id(clients[1])
        );
        let reference_client = if reload_client == clients[1] {
            clients[1]
        } else {
            clients[0]
        };
        let expected_tiled_pane = tab.get_active_pane_id(reference_client).unwrap();
        let floating_pane = PaneId::Terminal(TerminalId::new(3));
        tab.new_pane(
            floating_pane,
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            None,
            None,
        )
        .unwrap();
        assert!(tab.are_floating_panes_visible());
        for client in clients {
            assert_eq!(tab.get_active_pane_id(client), Some(floating_pane));
        }

        let mut config = test_screen_config();
        config.session_is_mirrored = true;
        screen
            .reconfigure(config, Keybinds::default(), reload_client)
            .unwrap();

        let tab = screen.get_active_tab_mut(clients[0]).unwrap();
        assert!(tab.are_floating_panes_visible());
        for client in clients {
            assert_eq!(tab.get_active_pane_id(client), Some(floating_pane));
        }
        tab.hide_floating_panes();
        for client in clients {
            assert_eq!(tab.get_active_pane_id(client), Some(expected_tiled_pane));
        }
    }
}

#[test]
fn a_reload_enabling_mirroring_reconciles_client_tabs_and_pane_focus() {
    // A connected caller supplies the focus; a detached caller falls back to the lowest ID.
    for reload_client in [ClientId::new(2), ClientId::new(99)] {
        let clients = [ClientId::new(1), ClientId::new(2), ClientId::new(3)];
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        screen.add_client(clients[1]).unwrap();
        screen.add_client(clients[2]).unwrap();
        screen
            .get_active_tab_mut(clients[2])
            .unwrap()
            .new_pane(
                PaneId::Terminal(TerminalId::new(3)),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(clients[2]),
                None,
            )
            .unwrap();
        screen
            .switch_active_tab(TabPosition::new(0), None, true, clients[0])
            .unwrap();
        assert_ne!(
            screen.get_active_pane_id(&clients[1]),
            screen.get_active_pane_id(&clients[2]),
            "clients in the same tab start on different panes"
        );
        let reference_client = if reload_client == clients[1] {
            clients[1]
        } else {
            clients[0]
        };
        let expected_tab = screen.active_tab_ids[&reference_client];
        let expected_pane = screen.get_active_pane_id(&reference_client);
        let mut config = test_screen_config();
        config.session_is_mirrored = true;
        screen
            .reconfigure(config.clone(), Keybinds::default(), reload_client)
            .unwrap();

        for client in clients {
            assert_eq!(screen.active_tab_ids[&client], expected_tab);
            assert_eq!(screen.get_active_pane_id(&client), expected_pane);
            for (tab_id, tab) in &screen.tabs {
                assert_eq!(tab.has_connected_client(&client), *tab_id == expected_tab);
            }
        }
        let tab_history = screen.tab_history.clone();
        screen
            .reconfigure(config, Keybinds::default(), clients[2])
            .unwrap();
        assert_eq!(screen.tab_history, tab_history);

        let next_tab = screen
            .tabs
            .values()
            .find(|tab| tab.id != expected_tab)
            .unwrap();
        let (next_id, next_position) = (next_tab.id, next_tab.position);
        screen
            .switch_active_tab(next_position, None, true, clients[2])
            .unwrap();
        for client in clients {
            assert_eq!(screen.active_tab_ids[&client], next_id);
            assert_eq!(
                screen.get_active_pane_id(&client),
                screen.get_active_pane_id(&clients[2])
            );
            for (tab_id, tab) in &screen.tabs {
                assert_eq!(tab.has_connected_client(&client), *tab_id == next_id);
            }
        }
    }
}

/// The probe answer outlives the option: turning the protocol off masks it, turning the
/// protocol back on recovers it.
#[test]
fn a_reload_can_turn_kitty_graphics_off_and_back_on_for_a_probed_client() {
    let mut screen = screen_with_one_tab();
    let client_id = crate::ClientId::new(1);
    screen.update_kitty_graphics_support(client_id, true);
    let query = b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\";
    let ok = vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()];
    assert_eq!(active_pane_replies(&mut screen, query), ok);

    let mut off = test_screen_config();
    off.support_kitty_graphics_protocol = false;
    screen
        .reconfigure(off, Keybinds::default(), client_id)
        .unwrap();
    assert!(
        active_pane_replies(&mut screen, query).is_empty(),
        "a disabled protocol does not reply to queries at all"
    );

    screen
        .reconfigure(test_screen_config(), Keybinds::default(), client_id)
        .unwrap();
    assert_eq!(
        active_pane_replies(&mut screen, query),
        ok,
        "the client's probe answer was kept across the reload that turned the protocol off"
    );
}

#[test]
fn a_reload_with_no_theme_pair_paints_the_theme() {
    let mut screen = screen_with_one_tab();
    let mut config = test_screen_config();
    config.theme = styling(1);

    screen
        .reconfigure(config, Keybinds::default(), crate::ClientId::new(1))
        .unwrap();

    assert_eq!(screen.style.colors, styling(1));
}

/// Reloads recolor retained cells, and no pane output follows to paint them.
#[test]
fn a_reload_that_changes_the_theme_or_mapping_repaints_mapped_cells() {
    use base64::Engine;
    use yinz_types::agent::theme::*;
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    let renders = screen_wired_to_server(&mut screen);
    let event = AgentThemeEvent {
        version: EVENT_VERSION,
        launch_id: "mapped".into(),
        snapshot: Some(AgentThemeSnapshot {
            version: SNAPSHOT_VERSION,
            provider: "claude".into(),
            provider_version: "fixture".into(),
            theme: "fixture".into(),
            provenance: vec![],
            availability: Availability::Available,
            definitions: vec![Definition {
                token: "status".into(),
                channel: Channel::Foreground,
                color: SourceColor::Rgb(18, 52, 86),
                pair: None,
                target: Some(Role::Success.into()),
            }],
            collisions: vec![],
        }),
    };
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap());
    active_pane_replies(
        &mut screen,
        format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\\x1b[38;2;18;52;86mMAPPED\x1b[0m")
            .as_bytes(),
    );
    let frame = |screen: &mut Screen| {
        screen.render_to_clients().unwrap();
        renders
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::Render(Some(output)) => Some(output),
                _ => None,
            })
            .flat_map(|output| output.into_values())
            .collect::<String>()
    };
    let source_color = "38;2;18;52;86";
    let mapped = frame(&mut screen);
    assert!(mapped.contains("MAPPED") && !mapped.contains(source_color));
    drain_background_jobs(&background_jobs);

    assert!(screen.host_terminal_theme_mode.is_none());
    let mut theme = test_screen_config();
    theme.theme.exit_code_success.base = PaletteColor::Rgb((9, 8, 7));
    screen
        .reconfigure(theme, Keybinds::default(), crate::ClientId::new(1))
        .unwrap();
    assert!(
        drain_background_jobs(&background_jobs).contains(&BackgroundJob::RenderToClients),
        "a static theme reload must request a frame without host theme reports or pane output"
    );
    assert!(frame(&mut screen).contains("38;2;9;8;7"));
    drain_background_jobs(&background_jobs);

    let mut off = test_screen_config();
    off.agent_theme_mapping = AgentThemeMapping::Off;
    screen
        .reconfigure(off, Keybinds::default(), crate::ClientId::new(1))
        .unwrap();

    assert!(
        drain_background_jobs(&background_jobs).contains(&BackgroundJob::RenderToClients),
        "the reload asks for the frame that paints the source color back"
    );
    assert!(frame(&mut screen).contains(source_color));
}
