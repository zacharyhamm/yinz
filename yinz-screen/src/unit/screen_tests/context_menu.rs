//! Right-click context menus, from the click that opens one to the item that closes it.
//!
//! A menu is not a pane, so nothing here goes through the pane machinery: the state lives on
//! `Screen`, the geometry is a pure function of the click and the viewport, and the mouse and
//! keyboard are captured ahead of everything else. These check that, plus the two things that are
//! easy to get wrong and invisible when they are: that a menu still waiting on its hook plugin
//! draws nothing at all, and that an answer for a menu that has since closed is ignored.

use super::*;
use yinz_types::input::actions::Action;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::internal_ids::ContextMenuItemId;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::TabId;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_size::Viewport;
use {yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem};

/// A small configured menu, so these tests do not depend on what the shipped default happens to
/// contain. `ContextMenu::from_items` assigns the ids, exactly as parsing a config block does.
fn configured_menu() -> ContextMenu {
    ContextMenu::from_items(vec![
        ContextMenuItem::new("Copy", vec![Action::Copy]),
        ContextMenuItem::separator(),
        ContextMenuItem::new("Close pane", vec![Action::CloseFocus]),
    ])
}

/// The plugin id a hook's answer is attributed to. Any number will do; what matters is that a
/// selection is addressed to *this* one and not broadcast.
const HOOK_PLUGIN_ID: u32 = 7;

fn menu_items() -> Vec<ContextMenuItem> {
    configured_menu().items().to_vec()
}

/// A screen with one tab and the small menu above configured.
fn screen_with_menu_configured() -> Screen {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.context_menu = configured_menu();
    screen
}

/// The context a right-click hands up. `items` is deliberately empty: the tab that fires this
/// knows the pane and nothing about the configuration, and `Screen::open_context_menu` is what
/// fills them in.
fn click_context(pane_id: PaneId, click: (usize, usize)) -> ContextMenuContext {
    ContextMenuContext {
        pane_id,
        click,
        ..Default::default()
    }
}

/// A screen with one tab and a menu already open and resolved, anchored at `click`.
fn screen_with_open_menu(click: (usize, usize)) -> (Screen, ClientId, ContextMenuRequestId) {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), click),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    (screen, client_id, request_id)
}

fn viewport(_screen: &Screen) -> Viewport {
    Viewport {
        x: 0,
        y: 0,
        rows: 20,
        cols: 80,
    }
}

#[test]
fn a_menu_awaiting_its_plugin_draws_nothing() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    assert!(
        screen.context_menu_is_open(client_id),
        "the menu exists - it just has nothing to say yet"
    );
    assert!(
        screen
            .context_menu_layout_for(client_id, viewport(&screen))
            .is_none(),
        "an unresolved menu must paint no cells, or the list changes under the pointer"
    );
}

#[test]
fn the_timeout_falls_back_to_the_configured_items() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    screen.context_menu_timed_out(request_id, client_id);

    let layout = screen
        .context_menu_layout_for(client_id, viewport(&screen))
        .expect("the configured items are drawn once the plugin has had its chance");
    assert_eq!(layout.rows.len(), 3);
    assert_eq!(layout.rows[0].label, "Copy");
}

/// A menu that is drawn is a menu that is being clicked. An answer arriving after the fallback is
/// up would move the rows out from under a press the user has already made, so the release could
/// run an item they never aimed at - `Close pane` where they meant `Copy`.
#[test]
fn a_late_plugin_answer_is_ignored_once_the_fallback_is_visible() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    screen.context_menu_timed_out(request_id, client_id);
    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem::new("Filtered", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    let items = screen.context_menus[&client_id].items.visible();
    assert_eq!(
        items.len(),
        3,
        "the fallback the user is looking at stays: {items:?}"
    );
    assert_eq!(items[0].label, "Copy");
}

/// The same rule from the other side: the first answer wins, and a second one for the same menu
/// changes nothing either.
#[test]
fn a_second_plugin_answer_is_ignored() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem::new("Filtered", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem::new("Stale", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    assert_eq!(
        screen.context_menus[&client_id].items.visible()[0].label,
        "Filtered"
    );
}

/// The plugin is handed items with ids and no actions, so an item it hands back has to have its
/// behaviour put back on. Without this a menu draws perfectly and does nothing when clicked.
#[test]
fn an_item_the_plugin_returns_by_id_gets_its_actions_back() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    // What a plugin actually returns: ids and labels, no actions - and one it invented.
    let from_plugin = vec![
        ContextMenuItem {
            id: Some(ContextMenuItemId::new(2)),
            label: "Close this pane".to_owned(),
            ..Default::default()
        },
        ContextMenuItem {
            label: "Open https://example.com".to_owned(),
            context: [("url".to_owned(), "https://example.com".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    ];
    screen.set_context_menu_items(
        request_id,
        from_plugin,
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    let items = screen.context_menus[&client_id].items.visible().to_vec();
    assert_eq!(items[0].label, "Close this pane", "the plugin's label wins");
    assert_eq!(
        items[0].actions,
        vec![Action::CloseFocus],
        "but the behaviour came back"
    );
    assert!(
        items[1].actions.is_empty(),
        "an item the plugin invented has no configured actions to inherit"
    );
    assert_eq!(
        items[1].context.get("url").map(|s| s.as_str()),
        Some("https://example.com"),
        "it acts through its context instead"
    );
}

#[test]
fn a_plugins_answer_replaces_what_the_menu_says() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem::new("Open https://example.com", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    let layout = screen
        .context_menu_layout_for(client_id, viewport(&screen))
        .expect("the plugin's items are drawn");
    assert_eq!(layout.rows.len(), 1);
    assert_eq!(layout.rows[0].label, "Open https://example.com");
}

/// A plugin answering a menu the user has already dismissed must not resurrect it, and one
/// answering a menu that has been *replaced* must not overwrite the newer one.
#[test]
fn an_answer_naming_a_menu_that_is_gone_is_dropped() {
    let (mut screen, client_id, stale_request_id) = screen_with_open_menu((10, 5));
    screen.close_context_menu(client_id);

    screen.set_context_menu_items(
        stale_request_id,
        vec![ContextMenuItem::new("Too late", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    assert!(!screen.context_menu_is_open(client_id));

    // ...and again, with a newer menu in the way
    let newer_request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (20, 8)),
        client_id,
    );
    assert_ne!(newer_request_id, stale_request_id, "ids are monotonic");
    screen.set_context_menu_items(
        stale_request_id,
        vec![ContextMenuItem::new("Too late", vec![])],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    assert!(
        screen
            .context_menu_layout_for(client_id, viewport(&screen))
            .is_none(),
        "the newer menu is still awaiting its own answer"
    );
}

#[test]
fn a_plugin_answering_with_nothing_closes_the_menu() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    screen.set_context_menu_items(
        request_id,
        vec![],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    assert!(
        !screen.context_menu_is_open(client_id),
        "an empty answer means 'no menu here', not an empty box"
    );
}

#[test]
fn opening_a_second_menu_replaces_the_first() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let second = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (30, 9)),
        client_id,
    );
    screen.set_context_menu_items(
        second,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    assert_eq!(screen.context_menus.len(), 1);
    let layout = screen
        .context_menu_layout_for(client_id, viewport(&screen))
        .expect("drawn");
    assert_eq!(layout.x, 30, "at the newer click, not the older one");
}

#[test]
fn esc_closes_the_menu() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let esc = Some(KeyWithModifier::new(BareKey::Esc));

    assert!(screen.context_menu_handles_key(&esc, client_id));
    assert!(!screen.context_menu_is_open(client_id));
}

/// Esc has to work before the plugin has answered too, or a wedged hook leaves the user stuck
/// with an invisible thing eating their keys.
#[test]
fn esc_closes_a_menu_that_is_still_waiting() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );

    assert!(screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Esc)), client_id));
    assert!(!screen.context_menu_is_open(client_id));
}

#[test]
fn the_arrow_keys_move_the_selection_and_skip_the_separator() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let down = Some(KeyWithModifier::new(BareKey::Down));

    assert!(screen.context_menu_handles_key(&down, client_id));
    assert_eq!(screen.context_menus[&client_id].selected, Some(0));

    assert!(screen.context_menu_handles_key(&down, client_id));
    assert_eq!(
        screen.context_menus[&client_id].selected,
        Some(2),
        "index 1 is a separator"
    );
}

/// The menu takes only the keys it uses. The screen thread then closes it, because whatever the
/// key does next is about a moment the menu has stopped describing - that half is asserted at the
/// instruction level, where it lives.
#[test]
fn a_key_the_menu_does_not_use_falls_through() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    assert!(!screen
        .context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Char('x'))), client_id));
}

/// Esc dismisses the menu no matter what modifiers ride along with it - it's the universal
/// "get me out of here" key.
#[test]
fn esc_with_a_modifier_still_closes_the_menu() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let ctrl_esc = Some(KeyWithModifier::new(BareKey::Esc).with_ctrl_modifier());

    assert!(screen.context_menu_handles_key(&ctrl_esc, client_id));
    assert!(!screen.context_menu_is_open(client_id));
}

/// Ctrl-j, Ctrl-Down, and Ctrl-Enter are distinct keys from the keybinding system's point of
/// view, not the plain navigation/activation keys with an incidental modifier - so the menu must
/// not treat them as such.
#[test]
fn a_modified_navigation_key_falls_through() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let ctrl_down = Some(KeyWithModifier::new(BareKey::Down).with_ctrl_modifier());
    let ctrl_j = Some(KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier());

    assert!(!screen.context_menu_handles_key(&ctrl_down, client_id));
    assert!(!screen.context_menu_handles_key(&ctrl_j, client_id));
    assert_eq!(screen.context_menus[&client_id].selected, None);
}

#[test]
fn a_modified_enter_falls_through_rather_than_activating() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Down)), client_id);
    let ctrl_enter = Some(KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier());

    assert!(!screen.context_menu_handles_key(&ctrl_enter, client_id));
    assert!(screen.context_menu_is_open(client_id));
}

#[test]
fn enter_with_nothing_highlighted_closes_rather_than_guessing() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    assert!(screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Enter)), client_id));
    assert!(!screen.context_menu_is_open(client_id));
}

#[test]
fn enter_on_a_highlighted_item_runs_it_and_closes() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Down)), client_id);
    assert!(screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Enter)), client_id));
    assert!(!screen.context_menu_is_open(client_id));
}

#[test]
fn hovering_highlights_the_row_under_the_pointer_and_clears_off_it() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let view = viewport(&screen);
    let layout = screen
        .context_menu_layout_for(client_id, view)
        .expect("drawn");
    let row = layout.row_of_item(2).expect("the third item is on screen");

    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_buttonless_motion(Position::new(row as i32, (layout.x + 2) as u16)),
        view,
        client_id,
    );
    assert_eq!(screen.context_menus[&client_id].selected, Some(2));

    // off the menu entirely
    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_buttonless_motion(Position::new(0, 0)),
        view,
        client_id,
    );
    assert_eq!(
        screen.context_menus[&client_id].selected, None,
        "a highlight the pointer has left looks like an item about to run"
    );
}

#[test]
fn a_press_outside_the_menu_dismisses_it() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let view = viewport(&screen);
    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(0, 0)),
        view,
        client_id,
    );
    assert!(!screen.context_menu_is_open(client_id));
}

/// The press that lands on a row does nothing; the release is what activates. Dragging onto an
/// item and off again must leave the menu exactly as it was.
#[test]
fn an_item_activates_on_the_release_not_the_press() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let view = viewport(&screen);
    let layout = screen
        .context_menu_layout_for(client_id, view)
        .expect("drawn");
    let row = layout.row_of_item(0).expect("the first item is on screen");
    let on_item = Position::new(row as i32, (layout.x + 2) as u16);

    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_left_press_event(on_item),
        view,
        client_id,
    );
    assert!(
        screen.context_menu_is_open(client_id),
        "the press alone does not act"
    );

    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_left_release_event(on_item),
        view,
        client_id,
    );
    assert!(!screen.context_menu_is_open(client_id));
}

#[test]
fn a_release_on_a_separator_does_nothing() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let view = viewport(&screen);
    let layout = screen
        .context_menu_layout_for(client_id, view)
        .expect("drawn");
    let separator_row = layout.y + 2; // top border, first item, then the separator
    screen.handle_context_menu_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(
            separator_row as i32,
            (layout.x + 2) as u16,
        )),
        view,
        client_id,
    );
    assert!(screen.context_menu_is_open(client_id));
}

/// A menu is drawn above every pane and above the toasts, so it occludes the cursor exactly as a
/// floating pane does. Without this the cursor is put back on its own cell - correctly - and then
/// blinks through the box painted over it.
#[test]
fn a_menu_covering_the_cursor_hides_it_until_it_closes() {
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_menu_configured();
    let server_receiver = screen_wired_to_server(&mut screen);

    // park the pane's cursor where the menu is about to be, rather than aiming the menu at the
    // cursor: where a menu goes is the anchor's business, and this is about the two coinciding
    screen
        .get_active_tab_mut(client_id)
        .expect("TEST")
        .handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[10;20H".to_vec())
        .expect("TEST");
    let expected = pane_cursor(&mut screen, client_id);
    assert_eq!(expected, (19, 9), "the escape put the cursor here");

    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (18, 5)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    let layout = screen
        .context_menu_layout_for(client_id, viewport(&screen))
        .expect("the menu is drawn");
    assert!(
        (layout.x..layout.x + layout.width).contains(&expected.0)
            && (layout.y..layout.y + layout.height()).contains(&expected.1),
        "the menu has to cover the cursor or this asserts nothing: {layout:?} vs {expected:?}"
    );

    let mut grid = grid_for_render_snapshot(size);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &server_receiver, client_id, &mut grid);
    assert_eq!(
        cursor, None,
        "a menu painted over the cursor's cell must hide it:\n{snapshot}"
    );

    // ...and it comes back the moment the menu is gone
    screen.close_context_menu(client_id);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &server_receiver, client_id, &mut grid);
    assert_eq!(
        cursor,
        Some(expected),
        "the cursor must come back once the menu uncovers it:\n{snapshot}"
    );
}

/// The other client is looking at the same cells with nothing drawn over them, so one client's
/// menu must not blank their cursor - which is what a session-wide rect list would have done.
#[test]
fn one_clients_menu_does_not_hide_another_clients_cursor() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_menu_configured();
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    let server_receiver = screen_wired_to_server(&mut screen);

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[10;20H".to_vec())
        .expect("TEST");
    let expected = pane_cursor(&mut screen, crate::ClientId::new(2));

    // client 1's menu, over the cell both clients' cursors sit on
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (18, 5)),
        crate::ClientId::new(1),
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        crate::ClientId::new(1),
    );

    let mut grid = grid_for_render_snapshot(size);
    let (cursor, snapshot) = render_frame_and_cursor_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(2),
        &mut grid,
    );
    assert_eq!(
        cursor,
        Some(expected),
        "client 2 sees no menu, so nothing hides their cursor:\n{snapshot}"
    );
}

/// Nothing under a menu is dirty when it goes, so its cells would keep its glyphs.
#[test]
fn closing_a_menu_forces_what_was_under_it_to_repaint() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    for tab in screen.tabs.values_mut() {
        if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
            pane.set_should_render(false);
        }
    }

    screen.close_context_menu(client_id);

    let repainting = screen
        .tabs
        .values_mut()
        .filter_map(|tab| tab.get_pane_with_id_mut(pane_id))
        .all(|pane| pane.should_render());
    assert!(
        repainting,
        "the pane underneath has to be told to repaint, or it keeps the menu's glyphs"
    );
}

/// `ScreenInstruction::CloseContextMenu` - sent for every key bound to something other than
/// writing to the pane - renders only when there was a menu to take away.
#[test]
fn closing_a_menu_a_client_does_not_have_does_nothing() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    assert!(screen.close_context_menu(client_id));
    assert!(!screen.close_context_menu(client_id));
    assert!(!screen.close_context_menu(ClientId::new(client_id.get() + 1)));
}

#[test]
fn a_menu_over_a_pane_closes_when_that_pane_does() {
    let (mut screen, client_id, _) = screen_with_open_menu((10, 5));
    screen.close_context_menus_for_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    assert!(
        screen.context_menu_is_open(client_id),
        "a different pane closing is not this menu's business"
    );

    screen.close_context_menus_for_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(!screen.context_menu_is_open(client_id));
}

#[test]
fn each_client_gets_its_own_menu() {
    let mut screen = screen_with_menu_configured();
    let first = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        crate::ClientId::new(1),
    );
    let second = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (40, 12)),
        crate::ClientId::new(2),
    );
    screen.set_context_menu_items(
        first,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        crate::ClientId::new(1),
    );
    screen.set_context_menu_items(
        second,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        crate::ClientId::new(2),
    );

    let view = viewport(&screen);
    assert_eq!(
        screen
            .context_menu_layout_for(crate::ClientId::new(1), view)
            .unwrap()
            .x,
        10
    );
    assert_eq!(
        screen
            .context_menu_layout_for(crate::ClientId::new(2), view)
            .unwrap()
            .x,
        40
    );

    screen.close_context_menu(crate::ClientId::new(1));
    assert!(!screen.context_menu_is_open(crate::ClientId::new(1)));
    assert!(
        screen.context_menu_is_open(crate::ClientId::new(2)),
        "one client dismissing does not dismiss the other's"
    );
}

/// The whole fire path, from a right-press on a pane to the plugin thread being asked what the
/// menu should say. Everything above this file's other tests is state; this is the one that says
/// a right-click actually reaches it.
#[test]
fn a_right_press_asks_the_plugin_thread_what_the_menu_should_say() {
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 20 };
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.os_input = Some(Box::new(FakeInputOutput::default()));
    let (to_plugin, plugin_receiver) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));

    let mut screen = create_non_mirrored_screen(size);
    screen.bus.senders.to_plugin = bus.senders.to_plugin.clone();
    new_tab(&mut screen, 1, TabId::new(0));

    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(3, 6)),
        client_id,
    );

    let asked: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RequestContextMenu { context, .. } => Some(context),
            _ => None,
        })
        .collect();
    assert_eq!(asked.len(), 1, "exactly one question per right-click");
    let context = &asked[0];
    assert_eq!(
        context.click,
        (6, 3),
        "the absolute click, which is what anchors the menu - not the pane-relative one"
    );
    assert_eq!(context.client_id, client_id);
    assert!(screen.context_menu_is_open(client_id));
}

/// The regression guard for the feature as a whole: with the option off, right-click is exactly
/// what it was.
#[test]
fn a_right_press_with_the_option_off_asks_nobody_anything() {
    let client_id = ClientId::new(1);
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.os_input = Some(Box::new(FakeInputOutput::default()));
    let (to_plugin, plugin_receiver) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));

    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    screen.bus.senders.to_plugin = bus.senders.to_plugin.clone();
    new_tab(&mut screen, 1, TabId::new(0));

    screen.context_menu_on_right_click = false;
    for tab in screen.tabs.values_mut() {
        tab.update_context_menu_on_right_click(false);
    }

    screen.handle_mouse_event(
        MouseEvent::new_right_press_event(Position::new(3, 6)),
        client_id,
    );

    assert!(!screen.context_menu_is_open(client_id));
    assert!(
        !plugin_receiver.try_iter().any(|(instruction, _)| matches!(
            instruction,
            PluginInstruction::RequestContextMenu { .. }
        )),
        "nothing was asked"
    );
}

/// An item's `context` is an opaque payload the hook plugin invented, and the plugin that answered
/// this menu is the only one it means anything to. Broadcasting it would hand one hook's private
/// data to every plugin that happens to subscribe.
#[test]
fn a_selected_items_context_reaches_only_the_plugin_that_offered_it() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.context_menu = configured_menu();
    let client_id = ClientId::new(1);

    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        vec![ContextMenuItem {
            label: "Open https://example.com".to_owned(),
            context: [("url".to_owned(), "https://example.com".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        }],
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    capture.drain_plugin_updates();

    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Down)), client_id);
    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Enter)), client_id);

    let selections: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter(|(_, _, event)| matches!(event, Event::ContextMenuItemSelected { .. }))
        .collect();
    assert_eq!(selections.len(), 1, "got: {selections:?}");
    assert_eq!(
        selections[0].0,
        Some(yinz_types::internal_ids::PluginId::new(HOOK_PLUGIN_ID)),
        "addressed to the hook, not broadcast"
    );
    assert_eq!(selections[0].1, Some(client_id));
}

/// A menu takes no focus, so `Copy` and `CloseFocus` normally mean the pane that was right-clicked
/// without anyone having to arrange it. Nothing *stops* a plugin focusing elsewhere while the menu
/// is up, though, and an item that then ran against whatever had focus would close the wrong pane.
#[test]
fn an_item_runs_against_the_clicked_pane_even_if_focus_moved_meanwhile() {
    let client_id = ClientId::new(1);
    let (to_screen, screen_receiver) = channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let mut screen = screen_with_menu_configured();
    {
        let tab = screen.get_active_tab_mut(client_id).unwrap();
        tab.new_floating_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .expect("TEST");
    }
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    // Something else - the hook, a background plugin - moves the focus while the menu is open.
    screen
        .focus_pane_with_id(
            PaneId::Terminal(crate::TerminalId::new(2)),
            false,
            false,
            client_id,
        )
        .expect("TEST");
    assert_eq!(
        screen.active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    // "Copy" is a focus-relative action, and this stands in for the screen thread draining the
    // instruction the activation enqueued - see `activating_an_item_enqueues_rather_than_acting_inline`.
    screen.dispatch_context_menu_actions(
        PaneId::Terminal(crate::TerminalId::new(1)),
        vec![Action::Copy],
        client_id,
    );

    // The focus assertion is sent by the same thread that sends the action, immediately before it,
    // so nothing the screen was already holding can be processed between the two.
    let focus = screen_receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the focus assertion reaches the screen queue");
    match focus.0 {
        ScreenInstruction::FocusPaneWithId(pane_id, _, _, for_client, _) => {
            assert_eq!(
                pane_id,
                PaneId::Terminal(crate::TerminalId::new(1)),
                "the pane the menu was over"
            );
            assert_eq!(for_client, client_id);
        },
        other => panic!("expected the focus to be asserted first, got {other:?}"),
    }
    let copy = screen_receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the action reaches the screen queue");
    assert!(
        matches!(copy.0, ScreenInstruction::Copy(..)),
        "got {:?}",
        copy.0
    );
}

/// An item's action is something the user performed, so a plugin holding `InterceptInput` and
/// watching `Event::UserAction` has to see it - the key and CLI paths both announce it before
/// routing, and a menu is only a third way of asking for the same action.
#[test]
fn a_menu_item_announces_its_actions_as_user_input() {
    let client_id = ClientId::new(1);
    let (to_screen, _screen_receiver) = channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let (to_plugin, plugin_receiver) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (mut screen, _, _) = screen_with_open_menu((10, 5));
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));

    screen.dispatch_context_menu_actions(
        PaneId::Terminal(crate::TerminalId::new(1)),
        vec![Action::Copy],
        client_id,
    );

    let announced = std::iter::from_fn(|| {
        plugin_receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok()
            .map(|(instruction, _)| instruction)
    })
    .find_map(|instruction| match instruction {
        PluginInstruction::UserInput {
            client_id,
            action,
            terminal_id,
            cli_client_id,
        } => Some((client_id, action, terminal_id, cli_client_id)),
        _ => None,
    })
    .expect("the action reaches the plugin thread as user input");
    assert_eq!(announced.0, client_id);
    assert_eq!(announced.1, Action::Copy);
    assert_eq!(
        announced.2,
        Some(TerminalId::new(1)),
        "the terminal the menu was opened over"
    );
    assert_eq!(announced.3, None, "nothing about this came from the CLI");
}

/// The other half of `a_vanished_pane_drops_its_items_actions`: an item that does nothing must not
/// tell the plugins it did something either.
#[test]
fn a_vanished_pane_announces_nothing() {
    let client_id = ClientId::new(1);
    let (to_plugin, plugin_receiver) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (mut screen, _, _) = screen_with_open_menu((10, 5));
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));

    screen.dispatch_context_menu_actions(
        PaneId::Terminal(crate::TerminalId::new(999)),
        vec![Action::CloseFocus],
        client_id,
    );

    assert!(
        plugin_receiver
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "an item that acted on nothing announces nothing"
    );
}

/// `Screen` holds the configured items rather than re-reading them per click, so a config reload
/// that does not hand the new list over leaves the session showing the menu it started with -
/// while the option that turns the feature on and off updates, which is the confusing half.
#[test]
fn a_reload_replaces_the_configured_menu() {
    let mut screen = screen_with_menu_configured();
    let reloaded = ContextMenu::from_items(vec![ContextMenuItem::new(
        "Only this",
        vec![Action::CloseFocus],
    )]);

    let mut config = test_screen_config();
    config.theme = Styling::default();
    config.simplified_ui = true;
    config.copy_on_select = false;
    config.hide_session_name = true;
    config.context_menu_on_right_click = true;
    config.context_menu = reloaded.clone();
    screen
        .reconfigure(config, Keybinds::default(), crate::ClientId::new(1))
        .unwrap();

    assert_eq!(
        screen.context_menu.items(),
        reloaded.items(),
        "the next right-click offers what the reloaded config says"
    );
}

/// A menu whose pane is taken by a closing *tab*. `close_tab_by_id` removes the panes without
/// emitting a `ClosePane` for any of them, so the menu would otherwise outlive what it is offering
/// to act on - and its focus-relative items would then mean whichever pane the client was moved to.
#[test]
fn a_menu_dies_with_the_tab_holding_its_pane() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    new_tab(&mut screen, 2, TabId::new(1));
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(2)), (10, 5)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );
    assert!(screen.context_menu_is_open(client_id));

    screen.close_tab_by_id(TabId::new(1)).unwrap();

    assert!(
        !screen.context_menu_is_open(client_id),
        "the pane the menu was opened over went with the tab"
    );
}

/// A menu over a pane in a *different* tab is none of a tab closure's business.
#[test]
fn closing_a_tab_leaves_other_tabs_menus_alone() {
    let client_id = ClientId::new(1);
    let mut screen = screen_with_menu_configured();
    new_tab(&mut screen, 2, TabId::new(1));
    let request_id = screen.open_context_menu(
        click_context(PaneId::Terminal(crate::TerminalId::new(1)), (10, 5)),
        client_id,
    );
    screen.set_context_menu_items(
        request_id,
        menu_items(),
        Some(crate::PluginId::new(HOOK_PLUGIN_ID)),
        client_id,
    );

    screen.close_tab_by_id(TabId::new(1)).unwrap();

    assert!(screen.context_menu_is_open(client_id));
}

/// Activating an item does not restore the focus inline. It enqueues one instruction that will do
/// both the restore and the dispatch, so that a focus change already sitting in the queue behind
/// the click cannot land between them.
#[test]
fn activating_an_item_enqueues_rather_than_acting_inline() {
    let client_id = ClientId::new(1);
    let (to_screen, screen_receiver) = channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let (mut screen, _, _) = screen_with_open_menu((10, 5));
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));

    // Down highlights "Copy" - the first selectable item - and Enter runs it.
    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Down)), client_id);
    screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Enter)), client_id);

    let enqueued: Vec<_> = screen_receiver.try_iter().map(|(i, _)| i).collect();
    assert_eq!(
        enqueued.len(),
        1,
        "one instruction, carrying both halves: {enqueued:?}"
    );
    match &enqueued[0] {
        ScreenInstruction::ActivateContextMenuItem {
            pane_id,
            actions,
            client_id: for_client,
        } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(crate::TerminalId::new(1)),
                "the pane the menu was over"
            );
            assert_eq!(actions, &vec![Action::Copy]);
            assert_eq!(*for_client, client_id);
        },
        other => panic!("expected an ActivateContextMenuItem, got {other:?}"),
    }
    assert!(!screen.context_menu_is_open(client_id), "the menu is done");
}

/// The pane can still go between the click that activated an item and the instruction that runs
/// it. An item offering to act on a pane that no longer exists has to do nothing at all - running
/// its focus-relative actions anyway would aim them at whatever took that pane's place.
#[test]
fn a_vanished_pane_drops_its_items_actions() {
    let client_id = ClientId::new(1);
    let (to_screen, screen_receiver) = channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let (mut screen, _, _) = screen_with_open_menu((10, 5));
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));

    screen.dispatch_context_menu_actions(
        PaneId::Terminal(crate::TerminalId::new(999)),
        vec![Action::CloseFocus],
        client_id,
    );
    assert!(
        screen_receiver
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "nothing may be routed for a pane that is gone"
    );

    // ...and the same call for a pane that *is* there does route it, so the assertion above is
    // about the missing pane rather than about the plumbing being dead.
    screen.dispatch_context_menu_actions(
        PaneId::Terminal(crate::TerminalId::new(1)),
        vec![Action::CloseFocus],
        client_id,
    );
    // The focus assertion comes first - see
    // `an_item_runs_against_the_clicked_pane_even_if_focus_moved_meanwhile`.
    let routed: Vec<_> = std::iter::from_fn(|| {
        screen_receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok()
            .map(|(instruction, _)| instruction)
    })
    .take(2)
    .collect();
    assert!(
        routed
            .iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::CloseFocusedPane(..))),
        "got {routed:?}"
    );
}

#[test]
fn a_watcher_menu_only_repaint_restores_the_cursor_after_the_menu() {
    let (mut screen, followed, _) = screen_with_open_menu((18, 5));
    let watcher = ClientId::new(2);
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, Size { cols: 80, rows: 20 });
    screen.set_followed_client(followed).unwrap();
    // Keep regular-client menu output from forcing the watcher panes to repaint.
    screen.connected_clients.borrow_mut().remove(&followed);
    screen
        .connected_clients
        .borrow_mut()
        .insert(ClientId::new(99));
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(Size { cols: 80, rows: 20 });
    let expected = pane_cursor(&mut screen, followed);
    for _ in 0..2 {
        let (cursor, snapshot) =
            render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
        assert!(snapshot.contains("Copy"), "{snapshot}");
        assert_eq!(cursor, Some(expected), "{snapshot}");
    }
}

#[test]
fn menu_selection_repaints_only_the_menu_for_a_healthy_watcher() {
    let (mut screen, followed, _) = screen_with_open_menu((18, 5));
    let watcher = ClientId::new(2);
    screen
        .get_active_tab_mut(followed)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"unchanged-pane-content".to_vec())
        .unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, Size { cols: 80, rows: 20 });
    screen.set_followed_client(followed).unwrap();
    screen.render_to_clients().unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let fake_os = FakeInputOutput::default();
    *fake_os.render_backlog.lock().unwrap() = vec![
        (followed, RenderBacklog { queued_renders: 1 }),
        (watcher, RenderBacklog::default()),
    ];
    screen.bus.os_input = Some(Box::new(fake_os));
    assert!(
        screen.deferred_tabs().is_empty(),
        "the followed client's open menu can render for its healthy watcher"
    );
    assert!(screen.context_menu_handles_key(&Some(KeyWithModifier::new(BareKey::Down)), followed));
    assert!(!screen.watcher_clients[&watcher].should_force_render());
    assert!(
        screen.deferred_tabs().is_empty(),
        "selection changed without pane output"
    );
    screen.render_to_clients().unwrap();
    let watcher_frame: String = receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::Render(Some(output)) => output.get(&watcher).cloned(),
            _ => None,
        })
        .collect();
    assert!(watcher_frame.contains("Copy"), "{watcher_frame:?}");
    assert!(
        !watcher_frame.contains("unchanged-pane-content"),
        "menu selection must not repaint pane contents: {watcher_frame:?}"
    );
    assert!(screen.close_context_menu(followed));
    screen.render_to_clients().unwrap();
    assert!(screen.deferred_tabs().is_empty());
}
