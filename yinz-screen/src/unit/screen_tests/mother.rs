use super::*;
use yinz_bus::command_palette::AgentAction;
use yinz_types::data::OverlayPlacement;
use yinz_types::internal_ids::TabNumber;
use yinz_types::keys::{BareKey, KeyWithModifier};

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size { cols: 80, rows: 24 };
const SHELL: PaneId = PaneId::Terminal(TerminalId::new(1));
const MOTHER: PaneId = PaneId::Terminal(TerminalId::new(2));

fn screen() -> Screen {
    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
}

/// An overlay terminal on the client's tab, as the plugin host places a mother: unfocused.
fn overlay(screen: &mut Screen, pane: PaneId) {
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .new_pane(
            pane,
            None,
            None,
            false,
            false,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(CLIENT),
            None,
        )
        .unwrap();
}

fn screen_with_mother() -> Screen {
    let mut screen = screen();
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    screen
}

fn mother_tab(screen: &Screen) -> TabId {
    screen.tab_containing_overlay(MOTHER).unwrap().id
}

fn hidden(screen: &Screen) -> bool {
    screen
        .tab_containing_overlay(MOTHER)
        .unwrap()
        .overlay_carry_for(MOTHER)
        .hidden
}

fn reported_mother(screen: &Screen) -> Option<TerminalId> {
    screen.peer_sessions_cache[&screen.session_name].mother
}

fn hold(screen: &mut Screen, pane: PaneId) {
    screen.tab_containing_pane_mut(pane).unwrap().hold_pane(
        pane,
        Some(0),
        false,
        yinz_types::input::command::RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
    );
    screen.log_and_report_session_state().unwrap();
}

fn palette_key(screen: &mut Screen, key: BareKey) {
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(key)),
        &[],
        false,
        CLIENT
    ));
}

#[test]
fn toggle_without_a_mother_opens_the_palette_at_the_agents_to_launch() {
    let mut screen = screen();
    let (plugin_sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);

    screen.toggle_mother(CLIENT).unwrap();
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Launch mother"), "{frame}");
    assert!(
        frame.contains("claude") && frame.contains("codex"),
        "{frame}"
    );
    assert!(!frame.contains("New pane"), "{frame}");

    assert!(screen.command_palette_handles_input(&None, b"codex", false, CLIENT));
    palette_key(&mut screen, BareKey::Enter);
    let request = plugins
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::PaletteAgent(request) => Some(*request),
            _ => None,
        })
        .expect("the palette asked the plugin host to launch");
    assert!(matches!(&request.action, AgentAction::Mother { kind } if kind == "codex"));
    // A launch in flight cannot be submitted twice.
    palette_key(&mut screen, BareKey::Enter);
    assert!(!plugins
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    assert!(screen.mother.is_none());
}

#[test]
fn toggle_focuses_then_hides_then_shows_mother_again() {
    let mut screen = screen_with_mother();
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));

    // Visible but without the keyboard: the toggle hands it over.
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert!(!hidden(&screen));

    screen.toggle_mother(CLIENT).unwrap();
    assert!(hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));

    screen.toggle_mother(CLIENT).unwrap();
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert!(!screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn a_visible_mother_follows_tab_switches_and_keeps_the_keyboard_it_had() {
    let mut screen = screen_with_mother();

    // Shown without focus: it comes along, and the new tab's own pane keeps the keyboard.
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert!(!hidden(&screen));
    assert_eq!(
        screen.get_active_pane_id(&CLIENT),
        Some(PaneId::Terminal(TerminalId::new(3)))
    );

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn a_hidden_mother_stays_behind_until_a_toggle_fetches_it() {
    let mut screen = screen_with_mother();
    screen.toggle_mother(CLIENT).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert!(hidden(&screen));

    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert!(hidden(&screen));

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_eq!(
        screen.active_tab_ids.get(&CLIENT),
        Some(&TabId::new(1)),
        "fetching mother must not move the client"
    );
}

#[test]
fn mother_survives_a_new_conversation_and_ends_with_its_process() {
    let mut screen = screen_with_mother();
    // `/clear` and a replaced conversation end the agent's lifecycle, not the pane.
    screen.invalidate_agent_lifecycle(MOTHER, "conversation changed");
    screen.log_and_report_session_state().unwrap();
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));
    assert_eq!(screen.live_mother(), Some(MOTHER));

    hold(&mut screen, MOTHER);
    assert_eq!(screen.live_mother(), None);
    assert_eq!(reported_mother(&screen), None);

    // The next toggle clears away the exited pane and offers a new mother.
    let (pty_sender, _pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    screen.toggle_mother(CLIENT).unwrap();
    assert!(screen.tab_containing_pane(MOTHER).is_none());
    assert!(screen.mother.is_none());
    assert!(screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn an_exited_mother_is_not_listed_as_mother_among_the_live_agents() {
    let mut screen = screen_with_mother();
    let agent = yinz_types::agent::AgentInfo {
        kind: "claude".into(),
        state: yinz_types::agent::AgentState::Idle,
        source: yinz_types::agent::AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: None,
        session_id: None,
        run_id: None,
    };
    let listed_as_mother = |screen: &Screen| {
        screen
            .live_palette_agents()
            .iter()
            .find(|agent| agent.pane.id == MOTHER)
            .map(|agent| agent.metadata["mother"].clone())
    };
    assert!(screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_agent(MOTHER, Some(agent.clone())));
    assert_eq!(listed_as_mother(&screen).as_deref(), Some("true"));

    // Held for a rerun: the pane and its record remain, mother does not.
    screen.tab_containing_pane_mut(MOTHER).unwrap().hold_pane(
        MOTHER,
        Some(0),
        true,
        yinz_types::input::command::RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
    );
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_agent(MOTHER, Some(agent));
    assert_eq!(screen.live_mother(), None);
    assert_eq!(listed_as_mother(&screen).as_deref(), Some("false"));
}

#[test]
fn mother_is_forgotten_when_its_pane_closes() {
    let mut screen = screen_with_mother();
    let (pty_sender, _pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    screen.close_pane_explicitly(MOTHER, None);
    screen.log_and_report_session_state().unwrap();
    assert!(screen.mother.is_none());
    assert_eq!(reported_mother(&screen), None);
}

#[test]
fn a_second_launch_never_replaces_a_live_mother() {
    let mut screen = screen_with_mother();
    let (pty_sender, _pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    let late = PaneId::Terminal(TerminalId::new(3));
    overlay(&mut screen, late);
    screen.mother_spawned(late).unwrap();
    assert_eq!(screen.mother, Some(TerminalId::new(2)));
    assert!(screen.tab_containing_pane(late).is_none());

    // An exited mother is replaced, and what it left on screen goes with it.
    hold(&mut screen, MOTHER);
    overlay(&mut screen, late);
    screen.mother_spawned(late).unwrap();
    assert_eq!(screen.mother, Some(TerminalId::new(3)));
    assert!(screen.tab_containing_pane(MOTHER).is_none());
}

#[test]
fn the_palette_offers_launch_or_toggle_and_refuses_a_second_mother() {
    let mut screen = screen();
    let (plugin_sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);

    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, b"mother", false, CLIENT));
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Launch mother…"), "{frame}");
    assert!(!frame.contains("Toggle mother"), "{frame}");
    // The picker's candidates were listed before mother appeared; choosing one is refused.
    palette_key(&mut screen, BareKey::Enter);
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Mother is already running"), "{frame}");
    assert!(!plugins
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    screen.close_command_palette(CLIENT);

    let (screen_sender, actions) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(screen_sender));
    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, b"mother", false, CLIENT));
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Toggle mother"), "{frame}");
    assert!(!frame.contains("Launch mother"), "{frame}");
    palette_key(&mut screen, BareKey::Enter);
    assert!(actions.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::ActivateCommandPaletteAction {
            action: Action::ToggleMother,
            ..
        }
    )));
}

const OTHER: ClientId = ClientId::new(2);

#[test]
fn another_client_cannot_carry_or_hide_a_focused_mother() {
    for stand_in in [None, Some(PaneId::Terminal(TerminalId::new(4)))] {
        let mut screen = screen_with_mother();
        screen.add_client(OTHER).unwrap();
        screen.toggle_mother(OTHER).unwrap();
        if let Some(stand_in) = stand_in {
            screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(stand_in, MOTHER)
                .unwrap();
        }
        let visible = stand_in.unwrap_or(MOTHER);
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));

        // Sharing focus is allowed, hiding the other client's input target is not.
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));
        assert!(
            !screen
                .tab_containing_overlay(visible)
                .unwrap()
                .overlay_carry_for(visible)
                .hidden
        );

        new_tab(&mut screen, 3, TabId::new(1));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(0)
        );
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));
        assert_eq!(
            screen.get_active_pane_id(&CLIENT),
            Some(PaneId::Terminal(TerminalId::new(3)))
        );

        let (sender, writes) = channels::unbounded();
        screen
            .tab_containing_overlay_mut(visible)
            .unwrap()
            .senders
            .to_pty_writer = Some(SenderWithContext::new(sender));
        screen
            .write_character(
                None,
                b"still typing".to_vec(),
                false,
                OTHER,
                &HashMap::new(),
                None,
            )
            .unwrap();
        let PaneId::Terminal(terminal) = visible else {
            unreachable!()
        };
        assert!(writes.try_iter().any(|(instruction, _)| matches!(instruction,
            PtyWriteInstruction::Write(bytes, id, _) if id == terminal && bytes == b"still typing"
        )));

        screen
            .focus_pane_with_id(SHELL, false, false, OTHER)
            .unwrap();
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(1)
        );
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
    }
}

#[test]
fn clients_sharing_mother_let_go_one_at_a_time_and_the_last_one_hides_it() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(screen.get_active_pane_id(&OTHER), Some(MOTHER));
    assert!(!hidden(&screen));

    screen.toggle_mother(OTHER).unwrap();
    assert!(hidden(&screen));
    screen.toggle_mother(CLIENT).unwrap();
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn a_disconnected_client_does_not_pin_mother_to_its_tab() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.remove_client(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn mirrored_clients_leaving_together_do_not_block_mother() {
    let mut screen = create_new_screen(SIZE, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 3, TabId::new(1));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    assert_eq!(screen.active_tab_ids[&OTHER], TabId::new(1));
}

#[test]
fn invalid_mother_focus_is_non_fatal_and_later_toggles_work() {
    let mut screen = screen_with_mother();
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_selectable(MOTHER, false);
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_selectable(MOTHER, true);
    screen.active_tab_ids.insert(CLIENT, TabId::new(99));
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.active_tab_ids.remove(&CLIENT);
    screen.toggle_mother(CLIENT).unwrap();
    screen.active_tab_ids.insert(CLIENT, TabId::new(0));
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn mothers_replacements_toggle_travel_and_restore_as_overlays() {
    let editor = PaneId::Terminal(TerminalId::new(4));
    let plugin = PaneId::Plugin(yinz_types::internal_ids::PluginId::new(5));
    for replacements in [vec![editor], vec![plugin], vec![editor, plugin]] {
        let mut screen = screen_with_mother();
        let (sender, _plugins) = channels::unbounded();
        screen
            .tab_containing_pane_mut(MOTHER)
            .unwrap()
            .senders
            .to_plugin = Some(SenderWithContext::new(sender));
        screen.toggle_mother(CLIENT).unwrap();
        let mut previous = MOTHER;
        for replacement in &replacements {
            let tab = screen.tab_containing_pane_mut(previous).unwrap();
            if *replacement == editor {
                tab.replace_pane_with_editor_pane(*replacement, previous)
                    .unwrap();
            } else {
                tab.suppress_pane_and_replace_with_pid(
                    previous,
                    *replacement,
                    false,
                    None,
                    None,
                    None,
                )
                .unwrap();
            }
            previous = *replacement;
        }
        let visible = previous;
        assert_eq!(screen.live_mother(), Some(MOTHER));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.toggle_mother(CLIENT).unwrap();
        assert!(
            screen
                .tab_containing_overlay(visible)
                .unwrap()
                .overlay_carry_for(visible)
                .hidden
        );
        new_tab(&mut screen, 3, TabId::new(1));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(0)
        );
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        let tab = screen.tab_containing_pane(MOTHER).unwrap();
        assert!(!tab.pane_id_is_floating(&MOTHER));
        assert_eq!(tab.get_suppressed_panes().count(), replacements.len());
        for replacement in replacements.iter().rev() {
            screen
                .tab_containing_pane_mut(*replacement)
                .unwrap()
                .close_pane(*replacement, false, None);
        }
        assert_eq!(mother_tab(&screen), TabId::new(0));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
        assert_eq!(
            screen
                .tab_containing_pane(MOTHER)
                .unwrap()
                .get_suppressed_panes()
                .count(),
            0
        );
        screen.toggle_mother(CLIENT).unwrap();
        assert!(hidden(&screen));
        screen.toggle_mother(CLIENT).unwrap();
        screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
        assert_eq!(mother_tab(&screen), TabId::new(1));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    }
}

#[test]
fn a_toggle_fetches_a_parked_mother_like_any_parked_pane() {
    let mut screen = screen_with_mother();
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .suppress_pane(MOTHER, Some(CLIENT));
    screen.toggle_mother(CLIENT).unwrap();
    let tab = screen.tab_containing_pane(MOTHER).unwrap();
    assert_eq!(tab.get_suppressed_panes().count(), 0);
    assert!(tab.has_non_suppressed_pane_with_pid(&MOTHER));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_eq!(screen.live_mother(), Some(MOTHER));
}

#[test]
fn toggling_an_unselectable_mother_keeps_the_screen_thread_running() {
    let mut mock = MockScreen::new(SIZE);
    let received = Watched::new(vec![]);
    let server = mock.server_receiver.take().unwrap();
    let logger = log_actions_in_thread!(received, ServerInstruction::KillSession, server);
    let screen_thread = mock.run(None, vec![]);
    let client = mock.main_client_id;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.is_mother = true;
    for instruction in [
        ScreenInstruction::NewPane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::ClientId(client),
            Some(completion),
            false,
        ),
        ScreenInstruction::SetSelectable(MOTHER, false),
        ScreenInstruction::ToggleMother(client, None),
        ScreenInstruction::SetSelectable(MOTHER, true),
        ScreenInstruction::ToggleMother(client, None),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetFocusedPaneInfo {
            client_id: client,
            response_channel: tx,
        })
        .unwrap();
    let response = rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    mock.teardown(vec![screen_thread, logger]);
    assert!(matches!(
        response,
        yinz_types::data::GetFocusedPaneInfoResponse::Ok {
            pane_id: MOTHER,
            ..
        }
    ));
}

fn assert_input_goes_to(screen: &mut Screen, client: ClientId, pane: PaneId) {
    let (sender, writes) = channels::unbounded();
    screen
        .tab_containing_pane_mut(pane)
        .unwrap()
        .senders
        .to_pty_writer = Some(SenderWithContext::new(sender));
    screen
        .write_character(
            None,
            b"input".to_vec(),
            false,
            client,
            &HashMap::new(),
            None,
        )
        .unwrap();
    assert!(writes.try_iter().any(|(instruction, _)| matches!(instruction,
        PtyWriteInstruction::Write(bytes, id, _) if PaneId::Terminal(id) == pane && bytes == b"input"
    )));
}

#[test]
fn mirrored_transitions_restore_each_clients_mother_focus_after_layout_and_edge_focus() {
    for stand_in in [false, true] {
        for other_focused in [false, true] {
            for transition in ["ordinary", "directional", "layout"] {
                let mut screen = create_new_screen(SIZE, false, false);
                new_tab(&mut screen, 1, TabId::new(0));
                if transition != "layout" {
                    new_tab(&mut screen, 3, TabId::new(1));
                    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
                }
                overlay(&mut screen, MOTHER);
                screen.mother_spawned(MOTHER).unwrap();
                screen.add_client(OTHER).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
                if other_focused {
                    screen.toggle_mother(OTHER).unwrap();
                }
                let visible = if stand_in {
                    let editor = PaneId::Terminal(TerminalId::new(4));
                    screen
                        .tab_containing_pane_mut(MOTHER)
                        .unwrap()
                        .replace_pane_with_editor_pane(editor, MOTHER)
                        .unwrap();
                    editor
                } else {
                    MOTHER
                };
                match transition {
                    "layout" => new_tab(&mut screen, 3, TabId::new(1)),
                    "directional" => screen
                        .switch_active_tab(
                            yinz_types::internal_ids::TabPosition::new(1),
                            Some(Direction::Left),
                            true,
                            CLIENT,
                        )
                        .unwrap(),
                    _ => screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap(),
                }
                assert_eq!(
                    screen.tab_containing_overlay(visible).unwrap().id,
                    TabId::new(1)
                );
                let destination = PaneId::Terminal(TerminalId::new(3));
                for (client, expected) in [
                    (CLIENT, visible),
                    (OTHER, if other_focused { visible } else { destination }),
                ] {
                    assert_eq!(screen.active_tab_ids[&client], TabId::new(1));
                    assert_eq!(
                        screen.get_active_pane_id(&client),
                        Some(expected),
                        "{transition} {stand_in} {other_focused}"
                    );
                    assert_input_goes_to(&mut screen, client, expected);
                }
            }
        }
    }
}

#[test]
fn generic_focus_fetches_hidden_mother_and_its_replacement_without_toggling_or_switching_tabs() {
    for stand_in in [false, true] {
        let mut screen = screen_with_mother();
        screen.toggle_mother(CLIENT).unwrap();
        let visible = if stand_in {
            let editor = PaneId::Terminal(TerminalId::new(4));
            screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(editor, MOTHER)
                .unwrap();
            editor
        } else {
            MOTHER
        };
        screen.toggle_mother(CLIENT).unwrap();
        new_tab(&mut screen, 3, TabId::new(1));
        for requested in [MOTHER, visible, MOTHER] {
            screen
                .focus_pane_with_id(requested, true, true, CLIENT)
                .unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
            assert!(
                !screen
                    .tab_containing_overlay(visible)
                    .unwrap()
                    .overlay_carry_for(visible)
                    .hidden
            );
            assert_input_goes_to(&mut screen, CLIENT, visible);
        }
        assert_eq!(
            screen
                .tab_containing_pane(MOTHER)
                .unwrap()
                .get_suppressed_panes()
                .count(),
            usize::from(stand_in)
        );
    }
}

#[test]
fn generic_mother_focus_respects_other_clients_and_invalid_tabs_and_fetches_a_parked_mother() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    screen
        .focus_pane_with_id(MOTHER, true, false, CLIENT)
        .unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    assert_eq!(
        screen.get_active_pane_id(&CLIENT),
        Some(PaneId::Terminal(TerminalId::new(3)))
    );
    assert_input_goes_to(&mut screen, OTHER, MOTHER);
    screen.active_tab_ids.insert(CLIENT, TabId::new(99));
    screen
        .focus_pane_with_id(MOTHER, true, false, CLIENT)
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(99));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.active_tab_ids.insert(CLIENT, TabId::new(1));
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .suppress_pane(MOTHER, Some(OTHER));
    // Parked with nothing standing in for it, mother answers to the caller's flags.
    screen
        .focus_pane_with_id(MOTHER, true, true, CLIENT)
        .unwrap();
    assert!(screen
        .tab_containing_pane(MOTHER)
        .unwrap()
        .has_non_suppressed_pane_with_pid(&MOTHER));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn saved_mother_matches_reported_live_hidden_replaced_held_and_missing_metadata() {
    for state in ["live", "hidden", "replaced", "held", "exited", "missing"] {
        let mut screen = screen_with_mother();
        match state {
            "hidden" => {
                screen.toggle_mother(CLIENT).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
            },
            "replaced" => screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(PaneId::Terminal(TerminalId::new(4)), MOTHER)
                .unwrap(),
            "held" | "exited" => {
                screen.tab_containing_pane_mut(MOTHER).unwrap().hold_pane(
                    MOTHER,
                    Some(0),
                    state == "held",
                    RunCommand {
                        command: "/usr/bin/true".into(),
                        ..Default::default()
                    },
                );
            },
            "missing" => {
                screen.mother = Some(TerminalId::new(99));
            },
            _ => {},
        }
        let expected =
            matches!(state, "live" | "hidden" | "replaced").then_some(TerminalId::new(2));
        let (sender, messages) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
        screen.save_session(None).unwrap();
        let saved = messages
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PtyInstruction::SaveSessionToDisk { session_info, .. } => Some(session_info),
                _ => None,
            })
            .unwrap();
        assert_eq!(saved.mother, expected, "{state}");
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["mother"].is_null(),
            expected.is_none()
        );
        screen.log_and_report_session_state().unwrap();
        assert_eq!(saved.mother, reported_mother(&screen), "{state}");
        if matches!(state, "held" | "exited") {
            assert_eq!(screen.mother, Some(TerminalId::new(2)));
        }
    }
}

fn place_mother(
    screen: &mut Screen,
    id: u32,
    set_blocking: bool,
) -> tokio::sync::oneshot::Receiver<crate::route::ActionCompletionResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.is_mother = true;
    completion.wait_for_agent_start = true;
    screen
        .place_new_pane(
            PaneId::Terminal(TerminalId::new(id)),
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::ClientIdNoFocus(CLIENT),
            Some(completion),
            set_blocking,
            &mut Default::default(),
        )
        .unwrap();
    rx
}

#[test]
fn mother_registration_survives_late_placement_and_refuses_duplicate_arrivals() {
    let mut screen = screen();
    let (sender, messages) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    // The launcher gave up before its pane arrived; registration belongs to the screen.
    drop(place_mother(&mut screen, 2, false));
    assert_eq!(screen.live_mother(), Some(MOTHER));

    let late = PaneId::Terminal(TerminalId::new(3));
    let reply = place_mother(&mut screen, 3, false).blocking_recv().unwrap();
    assert_eq!(
        reply.error_message.as_deref(),
        Some("Mother is already running")
    );
    // The pane was never shown, so the launcher is not told about one.
    assert_eq!(reply.affected_pane_id, None);
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert!(screen.tab_containing_pane(late).is_none());
    assert_eq!(
        messages
            .try_iter()
            .filter(|(instruction, _)| matches!(instruction,
                PtyInstruction::ClosePane(pane, _) if *pane == late))
            .count(),
        1
    );
}

#[test]
fn a_blocking_mother_launch_is_still_registered() {
    let mut screen = screen();
    let _reply = place_mother(&mut screen, 2, true);
    assert_eq!(screen.live_mother(), Some(MOTHER));
}

#[test]
fn mother_placement_registers_retained_panes_on_error_but_never_missing_panes() {
    for retained in [false, true] {
        let mut screen = screen();
        let (sender, _messages) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
        if retained {
            let tab = screen.get_active_tab_mut(CLIENT).unwrap();
            tab.senders.should_silently_fail = false;
            tab.senders.to_pty_writer = None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut completion = crate::route::NotificationEnd::new(tx);
        completion.is_mother = true;
        completion.wait_for_agent_start = true;
        let target = if retained {
            TabId::new(0)
        } else {
            TabId::new(99)
        };
        let result = screen.place_new_pane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::TabIndexNoFocus(target),
            Some(completion),
            false,
            &mut Default::default(),
        );
        assert_eq!(result.is_err(), retained);
        let reply = rx.blocking_recv().unwrap();
        assert!(reply.error_message.is_some());
        assert_eq!(reply.affected_pane_id, Some(MOTHER));
        assert_eq!(screen.live_mother(), retained.then_some(MOTHER));
        assert_eq!(screen.tab_containing_pane(MOTHER).is_some(), retained);
    }
}

#[test]
fn mother_is_registered_before_pending_layout_completes_placement() {
    let mut screen = screen();
    let tab = TabId::new(1);
    screen
        .new_tab(tab, (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.is_mother = true;
    completion.wait_for_agent_start = true;
    screen
        .place_new_pane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::TabIndexNoFocus(tab),
            Some(completion),
            false,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    // The caller gives up before the layout arrives; registration already belongs to the screen.
    drop(rx);
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(3), None)],
            vec![],
            HashMap::new(),
            tab,
            false,
            CLIENT,
            None,
        )
        .unwrap();
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert_eq!(mother_tab(&screen), tab);
}
