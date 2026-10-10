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
