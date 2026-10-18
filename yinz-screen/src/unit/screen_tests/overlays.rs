#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const TERMINAL_3: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(3);
#[cfg(test)]
const TERMINAL_4: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(4);

use super::*;
use crate::panes::OverlayCarry;
use std::collections::BTreeMap;
use yinz_types::geometry::{Resize, ResizeStrategy};
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use {yinz_types::data::OverlayKind, yinz_types::data::OverlayPlacement};

#[test]
fn modal_plugin_configuration_requests_overlay() {
    let mut modal = yinz_runtime::plugin_location::run_plugin_from_url("yinz:quit-confirmation")
        .expect("test plugin");
    modal.configuration.insert("modal", "true");
    let modal = RunPluginOrAlias::RunPlugin(modal);
    assert!(modal.requests_overlay());
    assert!(modal.requests_modal());

    let plain = yinz_runtime::plugin_location::run_plugin_from_url("yinz:quit-confirmation")
        .expect("test plugin");
    let plain = RunPluginOrAlias::RunPlugin(plain);
    assert!(!plain.requests_overlay());
    assert!(!plain.requests_modal());
}

#[test]
fn resolved_modal_plugin_alias_requests_overlay() {
    let mut modal = yinz_runtime::plugin_location::run_plugin_from_url("yinz:quit-confirmation")
        .expect("test plugin");
    modal.configuration.insert("modal", "true");
    let alias = PluginAlias {
        name: "quit".to_owned(),
        run_plugin: Some(modal),
        ..Default::default()
    };

    let alias = RunPluginOrAlias::Alias(alias);
    assert!(alias.requests_overlay());
    assert!(alias.requests_modal());
}

#[test]
fn alt_f_opens_a_floating_pane_when_only_an_overlay_exists() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    new_tab(&mut screen, 1, TabId::new(0));

    let overlay = PaneId::Terminal(crate::TerminalId::new(2));
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.new_pane(
        overlay,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                ..Default::default()
            },
        ))),
        Some(crate::ClientId::new(1)),
        None,
    )
    .unwrap();
    tab.toggle_floating_panes(Some(crate::ClientId::new(1)), None, None)
        .unwrap();

    assert!(matches!(
        pty_receiver.try_recv().map(|(instruction, _)| instruction),
        Ok(PtyInstruction::SpawnTerminal(
            _,
            _,
            NewPanePlacement::Floating(None),
            _,
            ClientTabIndexOrPaneId::ClientId(CLIENT_1),
            _,
            _
        ))
    ));
    assert!(tab.pane_is_overlay(overlay));
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(overlay)
    );
}

#[test]
fn alt_f_hides_regular_floating_panes_while_an_overlay_exists() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    open_floating_pane(&mut screen, TabId::new(0), 3, "OVERLAY", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.are_floating_panes_visible());
    tab.toggle_floating_panes(Some(crate::ClientId::new(1)), None, None)
        .unwrap();

    assert!(!tab.are_floating_panes_visible());
    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(3))));
    assert!(pty_receiver.try_recv().is_err());
}

/// A pane's z-index has to be its place in the occlusion stack, which is not the same as its place
/// among the panes a given render pass happens to draw. Taking the index from the pass rather than
/// from the stack clips a pane against its own geometry, dropping every one of its chunks - the
/// popup renders as nothing at all and whatever sits behind it shows through.
#[test]
fn an_overlay_popup_still_renders_when_another_floating_pane_is_open() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    open_floating_pane(&mut screen, TabId::new(0), 3, "POPUP", true);

    let mut grid = grid_for_render_snapshot(size);
    let snapshot = render_frame_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert!(
        snapshot.contains("POPUP"),
        "the overlay popup must render above the other floating pane, got:\n{snapshot}"
    );
}

/// Where the plugin popups in these tests are placed. Fixed on both axes, so a drag is visible as
/// a difference in the resolved geometry.
fn popup_coordinates() -> FloatingPaneCoordinates {
    FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    }
}

/// The whole point of `awaits_placement`, through the door a keybinding launch actually comes in
/// by.
///
/// `overlay true` alone puts the pane on screen at `base_geom`'s half-size-centered default, and
/// its owner only moves it out of there a round trip later - which for agent's two corner panels
/// is a flash in the middle of the screen, on top of each other. With the flag the pane exists,
/// can be focused, and is simply not painted until somebody places it.
#[test]
fn an_overlay_that_awaits_placement_is_not_painted_until_it_is_placed() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_new_screen(size, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    let configuration = [
        ("overlay".to_owned(), "true".to_owned()),
        ("awaits_placement".to_owned(), "true".to_owned()),
    ]
    .into_iter()
    .collect();
    let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:/path/to/fake/popup",
        &Some(configuration),
        None,
        None,
    )
    .unwrap();
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let pane = PaneFactory::plugin(
        crate::PluginId::new(9),
        Default::default(),
        plugin_sender,
        "popup".to_owned(),
        "popup".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().iter().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin)),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    let pane_id = PaneId::Plugin(crate::PluginId::new(9));
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    // the door `LaunchOrFocusPlugin { floating true; overlay true }` comes in by: overlay-ness is
    // re-derived from the configuration, and so is this
    tab.add_floating_pane(
        pane,
        pane_id,
        Some(popup_coordinates()),
        true,
        Some(crate::ClientId::new(1)),
    )
    .expect("TEST");

    assert!(
        tab.pane_is_overlay(pane_id),
        "it is an overlay, and it exists"
    );
    assert!(
        tab.overlay_carry_for(pane_id).awaiting_placement,
        "and it is waiting to be told where it goes"
    );
    // `position_is_on_an_overlay` asks `visible_pane_id_at`, which derives from the same one list
    // the painting does - so this is "nothing is drawn there", not merely "nothing is clickable"
    let inside = Position::new(3, 3);
    assert!(
        !tab.position_is_on_an_overlay(&inside),
        "nothing is painted for it until somebody places it"
    );

    tab.change_overlay_coordinates_batch(vec![(pane_id, popup_coordinates())], false)
        .expect("TEST");
    assert!(
        !tab.overlay_carry_for(pane_id).awaiting_placement
            && tab.position_is_on_an_overlay(&inside),
        "the first placement is what draws it"
    );
}

#[test]
fn self_sizing_builtins_and_resolved_aliases_await_placement_without_new_configuration() {
    for url in ["yinz:new-agent", "yinz:quit-confirmation"] {
        let plugin = yinz_runtime::plugin_location::run_plugin_from_url(url).unwrap();
        assert!(RunPluginOrAlias::RunPlugin(plugin.clone()).awaits_placement());
        for name in ["custom-binding", "duplicate-agent", "fork-agent"] {
            assert!(RunPluginOrAlias::Alias(PluginAlias {
                name: name.to_owned(),
                run_plugin: Some(plugin.clone()),
                ..Default::default()
            })
            .awaits_placement());
        }
    }
    let mut ordinary =
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:configuration").unwrap();
    ordinary.configuration.insert("modal", "true");
    assert!(!RunPluginOrAlias::RunPlugin(ordinary.clone()).awaits_placement());
    ordinary.configuration.insert("awaits_placement", "true");
    assert!(RunPluginOrAlias::RunPlugin(ordinary).awaits_placement());
}

/// Drive the actual screen queue, with a render and a FIFO reply barrier between lifecycle
/// steps. No plugin thread or debounce timing decides whether the intermediate frame is tested.
#[test]
fn self_sizing_modals_never_paint_the_default_outline() {
    for columns in [80, 200] {
        for early in [false, true] {
            for url in ["yinz:new-agent", "yinz:quit-confirmation"] {
                let size = Size {
                    cols: columns,
                    rows: 40,
                };
                let mut mock = MockScreen::new(size);
                let server_rx = mock.server_receiver.take().unwrap();
                let thread = mock.run(None, vec![]);
                let pid = PluginId::new(9);
                let pane_id = PaneId::Plugin(pid);
                let to_screen = mock.to_screen.clone();
                let mut grid = grid_for_render_snapshot(size);
                let mut render_step = || {
                    to_screen.send(ScreenInstruction::RenderToClients).unwrap();
                    let (tx, rx) = crossbeam::channel::bounded(1);
                    to_screen
                        .send(ScreenInstruction::GetPaneInfo {
                            pane_id,
                            response_channel: tx,
                        })
                        .unwrap();
                    let info = rx
                        .recv_timeout(yinz_test_support::default_timeout())
                        .unwrap();
                    let frames: Vec<_> = server_rx
                        .try_iter()
                        .filter_map(|(event, _)| {
                            if let ServerInstruction::Render(Some(clients)) = event {
                                clients.get(&CLIENT_1).map(|bytes| {
                                    take_snapshot_and_cursor_coordinates(bytes, &mut grid).1
                                })
                            } else {
                                None
                            }
                        })
                        .collect();
                    (info, frames)
                };
                assert!(render_step().0.is_none());
                let width = if url.ends_with("new-agent") {
                    columns.div_ceil(2).min(60)
                } else {
                    columns * 40 / 100
                };
                let coordinates = FloatingPaneCoordinates {
                    width: Some(PercentOrFixed::Fixed(width)),
                    height: Some(PercentOrFixed::Fixed(8)),
                    ..Default::default()
                };
                let place = || {
                    // Two requests catch lost/reversed buffered coordinates.
                    for height in [6, 8] {
                        to_screen
                            .send(ScreenInstruction::ChangeOverlayCoordinates(
                                vec![(
                                    pane_id,
                                    FloatingPaneCoordinates {
                                        height: Some(PercentOrFixed::Fixed(height)),
                                        ..coordinates.clone()
                                    },
                                )],
                                true,
                                None,
                            ))
                            .unwrap();
                    }
                };
                if early {
                    place();
                    let (info, frames) = render_step();
                    assert!(info.is_none());
                    assert!(frames.iter().all(|frame| !frame.contains("MODAL")));
                }
                let mut plugin = yinz_runtime::plugin_location::run_plugin_from_url(url).unwrap();
                plugin.configuration.insert("modal", "true");
                to_screen
                    .send(ScreenInstruction::AddPlugin(
                        Some(true),
                        false,
                        false,
                        RunPluginOrAlias::RunPlugin(plugin),
                        Some("MODAL".to_owned()),
                        None,
                        pid,
                        None,
                        None,
                        false,
                        None,
                        Some(true),
                        Some(CLIENT_1),
                        None,
                        None,
                    ))
                    .unwrap();
                if !early {
                    let (info, frames) = render_step();
                    assert!(info.is_some(), "the modal exists before placement");
                    assert!(!frames.is_empty(), "force a frame while awaiting geometry");
                    assert!(
                        frames.iter().all(|frame| !frame.contains("MODAL")),
                        "{frames:?}"
                    );
                    place();
                }
                let (info, frames) = render_step();
                let info = info.unwrap();
                assert_eq!(info.pane_columns.get(), width);
                assert_eq!(info.pane_rows.get(), 8);
                assert!(
                    frames.iter().any(|frame| frame.contains("MODAL")),
                    "{frames:?}"
                );
                let final_border = format!("└{}┘", "─".repeat(width - 2));
                for frame in frames.iter().filter(|frame| frame.contains("MODAL")) {
                    assert!(frame.contains(&final_border), "{frame}");
                }
                mock.teardown(vec![thread]);
            }
        }
    }
}

#[test]
fn waiting_modals_recover_on_error_or_resize_and_ordinary_modals_open_immediately() {
    use crate::ui::loading_indication::LoadingIndication;
    for recovery in ["placed", "error", "resize", "ordinary"] {
        let size = Size { cols: 80, rows: 24 };
        let (mut screen, _, server_rx) =
            create_new_screen_with_capture(size, false, false, true, true);
        let (plugin_tx, _plugin_rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        new_tab(&mut screen, 1, TabId::new(0));
        let mut grid = grid_for_render_snapshot(size);
        render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
        let url = if recovery == "ordinary" {
            "yinz:configuration"
        } else {
            "yinz:new-agent"
        };
        let mut plugin = yinz_runtime::plugin_location::run_plugin_from_url(url).unwrap();
        plugin.configuration.insert("modal", "true");
        let run = RunPluginOrAlias::RunPlugin(plugin);
        let pid = PluginId::new(9);
        let pane_id = PaneId::Plugin(pid);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .new_pane(
                pane_id,
                Some("MODAL".to_owned()),
                Some(Run::Plugin(run.clone())),
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
                Some(CLIENT_1),
                None,
            )
            .unwrap();
        let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
        assert_eq!(frame.contains("MODAL"), recovery == "ordinary", "{frame}");
        match recovery {
            "placed" => {
                screen.change_overlay_coordinates(vec![(pane_id, popup_coordinates())], false);
            },
            "error" => {
                let mut loading = LoadingIndication::new("modal".to_owned());
                loading.indicate_loading_error("LOAD FAILED".to_owned());
                super::super::replay_pending_plugin_events(
                    &mut screen,
                    vec![ScreenInstruction::UpdatePluginLoadingStage(pid, loading)],
                )
                .unwrap();
            },
            "resize" => screen
                .resize_to_screen(Size { cols: 82, rows: 24 })
                .unwrap(),
            _ => {},
        }
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        assert!(!tab.overlay_carry_for(pane_id).awaiting_placement);
        // Reopening an existing modal must not restart the placement wait.
        tab.hide_overlay_pane(pane_id);
        assert!(screen
            .focus_plugin_pane(&run, true, true, false, CLIENT_1, &mut None)
            .unwrap());
        assert!(
            !screen.tabs[&TabId::new(0)]
                .overlay_carry_for(pane_id)
                .awaiting_placement
        );
        let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
        assert!(frame.contains("MODAL"), "{frame}");
        if recovery == "error" {
            assert!(frame.contains("LOAD FAILED"), "{frame}");
        }
        let placed_at = screen.tabs[&TabId::new(0)]
            .get_pane_with_id(pane_id)
            .unwrap()
            .position_and_size();
        new_tab(&mut screen, 2, TabId::new(1));
        assert!(screen
            .focus_plugin_pane(&run, true, true, false, CLIENT_1, &mut None)
            .unwrap());
        let moved_tab = &screen.tabs[&TabId::new(1)];
        assert!(!moved_tab.overlay_carry_for(pane_id).awaiting_placement);
        assert_eq!(
            moved_tab
                .get_pane_with_id(pane_id)
                .unwrap()
                .position_and_size(),
            placed_at
        );
    }
}

#[test]
fn initially_suppressed_modals_accept_geometry_before_or_after_reveal() {
    for reveal_before_placement in [false, true] {
        for move_to_another_tab in [false, true] {
            let size = Size { cols: 80, rows: 24 };
            let (mut screen, _, server_rx) =
                create_new_screen_with_capture(size, false, false, true, true);
            let (plugin_tx, _plugin_rx) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
            new_tab(&mut screen, 1, TabId::new(0));
            open_floating_pane(&mut screen, TabId::new(0), 2, "HIDDENFLOATING", false);
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .hide_floating_panes();
            let mut grid = grid_for_render_snapshot(size);
            render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
            let mut plugin =
                yinz_runtime::plugin_location::run_plugin_from_url("yinz:new-agent").unwrap();
            plugin.configuration.insert("modal", "true");
            let run = RunPluginOrAlias::RunPlugin(plugin);
            let pane_id = PaneId::Plugin(PluginId::new(9));
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .new_pane(
                    pane_id,
                    Some("MODAL".to_owned()),
                    Some(Run::Plugin(run.clone())),
                    true,
                    true,
                    NewPanePlacement::Floating(None),
                    Some(CLIENT_1),
                    None,
                )
                .unwrap();
            let info = screen.tabs[&TabId::new(0)].get_pane_info(pane_id).unwrap();
            assert!(info.is_suppressed);
            assert!(!info.is_focused);
            assert_ne!(screen.active_pane_id(CLIENT_1), Some(pane_id));
            let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
            assert!(!frame.contains("MODAL"), "{frame}");

            if !reveal_before_placement {
                screen.change_overlay_coordinates(vec![(pane_id, popup_coordinates())], false);
                let tab = &screen.tabs[&TabId::new(0)];
                let info = tab.get_pane_info(pane_id).unwrap();
                assert!(info.is_suppressed);
                assert_ne!(screen.active_pane_id(CLIENT_1), Some(pane_id));
                assert!(!tab.overlay_carry_for(pane_id).awaiting_placement);
                let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
                assert!(!frame.contains("MODAL"), "{frame}");
            }
            let destination = if move_to_another_tab {
                new_tab(&mut screen, 3, TabId::new(1));
                open_floating_pane(&mut screen, TabId::new(1), 4, "HIDDENFLOATING", false);
                screen
                    .tabs
                    .get_mut(&TabId::new(1))
                    .unwrap()
                    .hide_floating_panes();
                TabId::new(1)
            } else {
                TabId::new(0)
            };
            assert!(screen
                .focus_plugin_pane(&run, true, true, false, CLIENT_1, &mut None)
                .unwrap());
            assert_eq!(screen.active_pane_id(CLIENT_1), Some(pane_id));
            let tab = &screen.tabs[&destination];
            let info = tab.get_pane_info(pane_id).unwrap();
            assert!(!info.is_suppressed);
            assert_eq!(
                tab.overlay_carry_for(pane_id).awaiting_placement,
                reveal_before_placement
            );
            if reveal_before_placement {
                let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
                assert!(!frame.contains("MODAL"), "{frame}");
                screen.change_overlay_coordinates(vec![(pane_id, popup_coordinates())], false);
            }
            // One sizing request, with no resize or second request to rescue the reveal.
            let tab = &screen.tabs[&destination];
            let geom = tab.get_pane_with_id(pane_id).unwrap().position_and_size();
            assert_eq!(
                (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize()),
                (2, 2, 48, 4)
            );
            assert!(!tab.overlay_carry_for(pane_id).awaiting_placement);
            assert!(screen
                .tabs
                .values()
                .all(|tab| !tab.are_floating_panes_visible()));
            if move_to_another_tab {
                assert!(screen.tabs[&TabId::new(0)].get_pane_info(pane_id).is_none());
            }
            let frame = render_frame_into(&mut screen, &server_rx, CLIENT_1, &mut grid);
            assert!(frame.contains("MODAL"), "{frame}");
            assert!(frame.contains(&format!("└{}┘", "─".repeat(46))), "{frame}");
            assert!(!frame.contains("HIDDENFLOATING"), "{frame}");
        }
    }
}

/// Put a *plugin* overlay in `tab_id`, returning what `focus_plugin_pane` has to be asked for to
/// find it again and the pane's id.
///
/// A plugin rather than a terminal because `move_to_focused_tab` is a `LaunchOrFocusPlugin` thing:
/// the pane is looked up by its `RunPluginOrAlias`, which is why that comes back out.
fn open_overlay_plugin(
    screen: &mut Screen,
    plugin_id: yinz_types::internal_ids::PluginId,
    tab_id: TabId,
    coordinates: FloatingPaneCoordinates,
) -> (RunPluginOrAlias, PaneId) {
    let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        &format!("file:/path/to/fake/popup-{plugin_id}"),
        &None,
        None,
        None,
    )
    .unwrap();
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let popup = PaneFactory::plugin(
        plugin_id,
        Default::default(),
        plugin_sender,
        "popup".to_owned(),
        "popup".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().iter().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin.clone())),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    let popup_id = PaneId::Plugin(plugin_id);
    let tab = screen.tabs.get_mut(&tab_id).expect("TEST");
    tab.show_floating_panes();
    tab.add_overlay_pane(
        popup,
        popup_id,
        yinz_types::data::OverlayPlacement::dismissible(Some(coordinates)),
        OverlayCarry::default(),
        true,
        Some(crate::ClientId::new(1)),
    )
    .expect("TEST");
    (run_plugin, popup_id)
}

/// A popup bound with `move_to_focused_tab` follows the client from tab to tab. The tab it leaves
/// forgets the coordinates it was placed with, so unless they travel with it the popup lands in
/// the new tab as an ordinary floating pane, sized against the tab's viewport rather than against
/// the session surface.
#[test]
fn an_overlay_popup_stays_an_overlay_when_it_follows_the_client_to_another_tab() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    let coordinates = popup_coordinates();
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(0),
        coordinates.clone(),
    );
    let first_tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    assert!(first_tab.pane_is_overlay(popup_id));
    let placed_at = first_tab
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();

    // the client moves to the other tab and asks for the popup again
    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(1))
        .expect("TEST");
    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            false,
            true,
            false,
            crate::ClientId::new(1),
            &mut None
        )
        .expect("TEST"));

    let second_tab = screen.tabs.get(&TabId::new(1)).expect("TEST");
    assert!(
        second_tab.pane_is_overlay(popup_id),
        "the popup must still be an overlay in the tab it followed the client into"
    );
    let moved_to = second_tab
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (moved_to.x, moved_to.cols.as_usize()),
        (placed_at.x, placed_at.cols.as_usize()),
        "an overlay is sized against the whole surface in either tab"
    );
}

fn floating_geom(screen: &Screen, pane_id: u32) -> PaneGeom {
    screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(pane_id)))
        .expect("TEST")
        .position_and_size()
}

/// Where the overlays in these tests sit on the session surface. Just geometry: what makes a pane
/// an overlay is its placement, which is decided when it is born.
fn overlay_coordinates() -> FloatingPaneCoordinates {
    FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    }
}

/// Opening an ordinary floating pane shows the floating layer, and the whole of yinz reads that
/// flag as "the user has floating panes up": the status bar lights its `<f>` ribbon, the next new
/// pane joins the floating layer, splits and focus moves get swallowed. An overlay is not on that
/// layer, so opening one must leave it exactly as it was.
#[test]
fn opening_an_overlay_does_not_raise_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(
        !tab.are_floating_panes_visible(),
        "an overlay is not the floating layer"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the overlay has the keyboard"
    );
}

/// The layer only goes down because nothing but the overlay was on it. A real floating pane
/// behind the overlay is still the user's floating pane, and still visible.
#[test]
fn the_floating_layer_stays_up_when_a_real_floating_pane_is_behind_the_new_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    open_floating_pane(&mut screen, TabId::new(0), 3, "POPUP", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(3))));
    assert!(
        tab.are_floating_panes_visible(),
        "the floating pane behind the overlay is still up"
    );
}

/// Overlay status is permanent for the pane's lifetime, and coordinates cannot express it either
/// way. A move must not turn an overlay back into an ordinary floating pane on a hidden layer.
#[test]
fn changing_an_overlays_coordinates_does_not_remove_its_overlay_status() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(!tab.are_floating_panes_visible());

    tab.change_floating_pane_coordinates(
        &PaneId::Terminal(crate::TerminalId::new(2)),
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(4)),
            y: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    )
    .expect("TEST");

    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(
        !tab.are_floating_panes_visible(),
        "moving an overlay must not reveal the floating layer"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );
}

/// A plugin moving a hidden floating pane must not pop the layer open under the user.
#[test]
fn moving_an_unfocused_hidden_floating_pane_does_not_show_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.hide_floating_panes();
    assert!(!tab.are_floating_panes_visible());

    tab.change_floating_pane_coordinates(
        &PaneId::Terminal(crate::TerminalId::new(2)),
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(6)),
            y: Some(PercentOrFixed::Fixed(6)),
            ..Default::default()
        },
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "moving a hidden floating pane must not reveal the floating layer"
    );
}

/// A modal is the client's active pane while the floating layer is down. Reading that as "this
/// client is working on the floating layer" sends the next new pane there - and reveals every
/// hidden floating pane on the way.
#[test]
fn a_new_pane_opened_over_a_modal_is_tiled() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "MODAL", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "opening a pane over a modal must not reveal the floating layer"
    );
    assert!(
        !tab.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId::new(3))),
        "the new pane belongs on the tiled surface"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the modal gives the keyboard up to the pane the user asked for - a popup that closes \
         when it loses focus depends on this"
    );
}

/// A modal plugin overlay, focused, over the active tab - what `new-agent` is.
fn add_modal_plugin_overlay(
    screen: &mut Screen,
    plugin_id: yinz_types::internal_ids::PluginId,
) -> PaneId {
    let mut configuration = BTreeMap::new();
    configuration.insert("modal".to_owned(), "true".to_owned());
    let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:/path/to/fake/modal",
        &Some(configuration),
        None,
        None,
    )
    .unwrap();
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let modal = PaneFactory::plugin(
        plugin_id,
        Default::default(),
        plugin_sender,
        "modal".to_owned(),
        "modal".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().iter().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin)),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    let modal_id = PaneId::Plugin(plugin_id);
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.add_floating_pane(
        modal,
        modal_id,
        Some(overlay_coordinates()),
        true,
        Some(crate::ClientId::new(1)),
    )
    .expect("TEST");
    modal_id
}

/// A modal overlay (e.g. new-agent) only closes on Esc/submit/`Event::Visible(false)` - it
/// has no way to learn it lost focus, unlike a dismissible overlay that watches `PaneUpdate`.
/// Stealing its focus the way `a_new_pane_opened_over_a_modal_is_tiled` does for a plain overlay
/// would leave it stuck on screen while input goes to the pane underneath, so it must keep focus.
#[test]
fn a_new_pane_opened_over_a_modal_plugin_overlay_does_not_steal_its_focus() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(modal_id));
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id)
    );

    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "opening a pane over a modal must not reveal the floating layer"
    );
    assert!(
        !tab.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId::new(3))),
        "the new pane belongs on the tiled surface"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id),
        "a modal overlay has no way to learn it lost focus, so it must keep it"
    );
}

/// `Tab::pane_infos` has listed overlays since they got their own collection; the singular
/// lookup behind the `GetPaneInfo` plugin API has to answer for the same panes, or a plugin that
/// opens an overlay and asks about the ID it was handed back gets nothing.
#[test]
fn an_overlay_answers_the_singular_pane_info_lookup() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(modal_id));

    let pane_info = tab
        .get_pane_info(modal_id)
        .expect("an overlay is a pane the singular API knows about");
    assert_eq!(pane_info.id, modal_id);
    assert!(pane_info.is_plugin());
    assert!(
        pane_info.is_floating,
        "every consumer reads `is_floating` as `not tiled`, which an overlay is"
    );
    assert!(
        !pane_info.is_fullscreen,
        "a modal cannot be expanded over the screen"
    );
    assert!(!pane_info.is_suppressed);

    assert!(
        tab.pane_infos()
            .iter()
            .any(|info| info.id == PaneId::Plugin(crate::PluginId::new(7))),
        "and it agrees with the plural API it sits beside"
    );
}

/// The other half of the test above: keeping focus is not the end of the story, or the pane the
/// user asked for would stay unreachable. `new-agent` submits, focuses the agent pane it
/// just opened, and only then closes itself - and this is that hand-off. Focusing the tiled pane
/// while the overlay is still up moves nothing a client can see (`get_active_pane_id` prefers the
/// overlay), it moves the tiled focus *underneath*, which is what the close then lands on. Doing
/// it in the other order - close, then focus - would have nothing left to focus from.
#[test]
fn a_modal_overlay_hands_focus_to_the_pane_it_opened_when_it_closes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id)
    );

    tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(3)),
        false,
        false,
        crate::ClientId::new(1),
    )
    .expect("TEST");
    tab.close_pane(modal_id, false, None);

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the modal hands the keyboard to the pane it opened, not back to where the user came from"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "the floating layer stays down - only the modal was ever up"
    );
}

/// A left/right multipane of two dismissible plugin overlays in `tab_id`, the way agent composes
/// its panel from the agent list and the conversation, with the client's keyboard on the list.
fn panel_multipane(screen: &mut Screen, tab_id: TabId) -> [PaneId; 2] {
    use yinz_types::multipane::*;
    let children = [7, 8]
        .map(|id| open_overlay_plugin(screen, PluginId::new(id), tab_id, popup_coordinates()).1);
    screen
        .multipane_command(
            MultipaneOperation::Create {
                children,
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio: 5000,
                },
                placement: OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(3)),
                    y: Some(PercentOrFixed::Fixed(3)),
                    width: Some(PercentOrFixed::Fixed(80)),
                    height: Some(PercentOrFixed::Fixed(24)),
                    ..Default::default()
                })),
                focus: Some(0),
                min_cells: 0,
            },
            CLIENT_1,
        )
        .expect("TEST");
    assert_eq!(screen.active_pane_id(CLIENT_1), Some(children[0]));
    children
}

/// `new-agent` opened over the agent panel, from the agent list half. Cancelled, it used to
/// close onto the topmost overlay left - the conversation half - so the next key went into the
/// chat box while the list still drew its selection. Submitted, it named the new agent, but the
/// keyboard went to the conversation all the same, over the agent it had just started.
#[test]
fn a_modal_over_a_panel_hands_the_keyboard_back_to_its_half_or_to_the_pane_it_opened() {
    for submitted in [false, true] {
        let mut screen = create_non_mirrored_screen(Size {
            cols: 120,
            rows: 40,
        });
        new_tab(&mut screen, 1, TabId::new(0));
        let children = panel_multipane(&mut screen, TabId::new(0));
        let modal_id = add_modal_plugin_overlay(&mut screen, PluginId::new(9));
        let tab = screen.get_active_tab_mut(CLIENT_1).unwrap();
        assert_eq!(tab.get_active_pane_id(CLIENT_1), Some(modal_id));
        let agent = PaneId::Terminal(crate::TerminalId::new(3));
        if submitted {
            tab.new_pane(
                agent,
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(CLIENT_1),
                None,
            )
            .expect("TEST");
            tab.focus_pane_with_id(agent, false, false, CLIENT_1)
                .expect("TEST");
            assert_eq!(
                tab.get_active_pane_id(CLIENT_1),
                Some(modal_id),
                "the modal keeps the keyboard until it goes"
            );
        }

        tab.close_pane(modal_id, false, None);

        assert_eq!(
            tab.get_active_pane_id(CLIENT_1),
            Some(if submitted { agent } else { children[0] }),
            "submitted: {submitted}"
        );
    }
}

/// The overlay did not lose focus - the layer under it went away. Telling its terminal otherwise
/// is a lie, and one an application that tracks focus reporting will act on.
///
/// `FakeInputOutput::write_to_tty_stdin` is `unimplemented!()`, so the focus-out this used to
/// send would panic the test rather than merely being wrong. That is the assertion.
#[test]
fn an_overlay_terminal_is_not_told_it_lost_focus_when_the_layer_goes_down() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    // DECSET 1004: the pane asks to be told when it gains and loses focus
    tab.handle_pty_bytes(crate::TerminalId::new(2), b"\x1b[?1004h".to_vec())
        .expect("TEST");

    tab.change_overlay_coordinates_batch(
        vec![(
            PaneId::Terminal(crate::TerminalId::new(2)),
            overlay_coordinates(),
        )],
        false,
    )
    .expect("TEST");

    assert!(!tab.are_floating_panes_visible());
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the overlay is still the active pane"
    );
}

/// Open a popup the way every popup plugin in this fork arrives: born onto the session surface,
/// never on the tab's floating layer.
fn open_popup(screen: &mut Screen, pane_id: u32, contents: &str) {
    open_floating_pane(screen, TabId::new(0), pane_id, contents, true);
}

/// What the frame actually shows, which is the only thing the user can see. `are_floating_panes_visible`
/// is the flag; this is the consequence.
fn rendered_frame(screen: &mut Screen, size: Size) -> String {
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let mut grid = grid_for_render_snapshot(size);
    render_frame_into(screen, &server_receiver, crate::ClientId::new(1), &mut grid)
}

/// The reported bug. A modal is not a floating pane, so putting one on screen must not put the
/// user's hidden floating panes on screen with it.
///
/// This used to turn on the whole floating layer: the modal was opened as an ordinary floating
/// pane (which reveals the layer), and the corrective that ran when it became an overlay only
/// fired when nothing else was on the layer. One hidden floating pane was enough to defeat it.
#[test]
fn a_modal_opened_onto_a_hidden_layer_does_not_reveal_hidden_floating_panes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "HIDDENFLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();
    assert!(!screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .are_floating_panes_visible());

    open_popup(&mut screen, 3, "MODAL");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(3))));
    assert!(
        !tab.are_floating_panes_visible(),
        "showing a modal must not show the floating layer"
    );

    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("MODAL"),
        "the modal itself is on screen, got:\n{frame}"
    );
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

/// The same, with more than one pane behind it - "if any exist" was the shape of the bug, and one
/// pane is the case the old corrective happened to get right.
#[test]
fn a_modal_opened_onto_a_hidden_layer_with_several_floating_panes_behind_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FIRSTHIDDEN", false);
    open_floating_pane(&mut screen, TabId::new(0), 3, "SECONDHIDDEN", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    open_popup(&mut screen, 4, "MODAL");

    assert!(!screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .are_floating_panes_visible());
    let frame = rendered_frame(&mut screen, size);
    for hidden in ["FIRSTHIDDEN", "SECONDHIDDEN"] {
        assert!(
            !frame.contains(hidden),
            "{hidden} was hidden and must stay hidden, got:\n{frame}"
        );
    }
}

/// The other direction of "the layer is whatever the user left it as": a layer the user had up
/// stays up, and the floating pane on it stays on screen next to the modal.
#[test]
fn a_modal_opened_onto_a_visible_layer_leaves_it_up() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "VISIBLEFLOATING", false);

    open_popup(&mut screen, 3, "MODAL");

    assert!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .are_floating_panes_visible(),
        "the floating pane the user had up is still the user's floating pane"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("VISIBLEFLOATING"),
        "the visible floating pane is still visible, got:\n{frame}"
    );
}

/// Closing the modal gives back the tab it found, not one with the floating layer switched on.
#[test]
fn closing_a_modal_leaves_the_floating_layer_as_it_found_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "HIDDENFLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    open_popup(&mut screen, 3, "MODAL");
    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);

    assert!(
        !tab.are_floating_panes_visible(),
        "the layer was down before the modal and is down after it"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the hidden floating pane is still hidden once the modal is gone, got:\n{frame}"
    );
}

/// `create_non_mirrored_screen` leaves `to_plugin` unset, and a tab cannot build a `PluginPane`
/// without it. The receiver comes back so the channel outlives the screen.
fn screen_with_plugin_sender(
    size: Size,
) -> (
    Screen,
    channels::Receiver<(PluginInstruction, ErrorContext)>,
) {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut screen = create_non_mirrored_screen(size);
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    (screen, plugin_rx)
}

/// A plugin launched with `modal true` and nothing else - no coordinates, the way a keybinding
/// written before the flags existed reaches the tab.
fn modal_plugin_run() -> Run {
    let mut configuration = BTreeMap::new();
    configuration.insert("modal".to_owned(), "true".to_owned());
    Run::Plugin(
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/path/to/fake/modal",
            &Some(configuration),
            None,
            None,
        )
        .unwrap(),
    )
}

/// A modal plugin opened with no coordinates at all. `ScreenInstruction::AddPlugin` synthesizes
/// overlay coordinates from the plugin's configuration only when nobody passed any, so a caller
/// that passes its own (`pane_groups`) or a path that passes none reaches the tab with the
/// `modal` flag stripped. The pane itself still knows what it is, and must be placed as an
/// overlay on that basis - born one, never an ordinary floating pane even for a frame.
#[test]
fn a_modal_plugin_opened_without_overlay_coordinates_is_still_born_an_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "HIDDENFLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.new_pane(
        PaneId::Plugin(crate::PluginId::new(7)),
        Some("modal".to_owned()),
        Some(modal_plugin_run()),
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");

    assert!(
        tab.pane_is_overlay(PaneId::Plugin(crate::PluginId::new(7))),
        "the plugin's own configuration says it is an overlay"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and an overlay never raises the floating layer, not even for a frame"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

/// A modal that was parked in the suppressed panes and is asked for again comes back as a modal.
/// This path passed no coordinates and unconditionally raised the layer, so it both lost the
/// overlay placement and revealed everything the user had hidden.
#[test]
fn refocusing_a_suppressed_modal_does_not_reveal_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "HIDDENFLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let start_suppressed = true;
    tab.new_pane(
        PaneId::Plugin(crate::PluginId::new(7)),
        Some("modal".to_owned()),
        Some(modal_plugin_run()),
        start_suppressed,
        true,
        NewPanePlacement::Floating(None),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");

    let should_float = true;
    let should_be_in_place = false;
    tab.focus_pane_with_id(
        PaneId::Plugin(crate::PluginId::new(7)),
        should_float,
        should_be_in_place,
        crate::ClientId::new(1),
    )
    .expect("TEST");

    assert!(
        tab.pane_is_overlay(PaneId::Plugin(crate::PluginId::new(7))),
        "a modal taken back out of the suppressed panes is still a modal"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and still does not raise the floating layer"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

// ---------------------------------------------------------------------------
// Characterization tests added ahead of the modals/overlays refactor.
//
// These pin behavior that the move to a dedicated overlay collection must
// preserve. They assert what the code does *today*, so that the refactor is
// provably behavior-preserving; where today's behavior is itself wrong, that is
// called out in the test's doc comment and fixed in a later commit rather than
// baked in here.
// ---------------------------------------------------------------------------

/// An overlay is positioned against the whole session surface and re-resolves whenever that
/// surface changes, so a resize leaves it proportional to the new surface.
///
/// It used to keep its old width. `FloatingPanes::reapply_overlay_coordinates` ran from inside
/// `floating_panes.resize()`, and the shared `display_area` cell it reads is only updated by
/// `tiled_panes.resize()` - which runs after. The overlay was re-resolved against the surface the
/// terminal had *before* the resize. `OverlayPanes::reapply_coordinates` is called from
/// `Tab::resize_whole_tab_locally` once the surface is known.
#[test]
fn an_overlay_is_re_resolved_against_the_surface_on_resize() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);

    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    // `open_floating_pane`'s overlay coordinates ask for 60% of the surface
    assert_eq!(before.cols.as_usize(), 48);

    screen
        .resize_to_screen(Size {
            cols: 120,
            rows: 20,
        })
        .expect("TEST");

    let after = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(after.cols.as_usize(), 72, "60% of the new 120 columns");
    assert!(
        screen.tabs[&TabId::new(0)].pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))),
        "and it is still an overlay afterwards"
    );
}

/// An overlay renders and owns the mouse while the floating layer is down - that is the whole
/// point of it. `visible_pane_id_at` answers from the occlusion stack rather than from the pane
/// map, so the overlay is hit-tested even though no floating pane is visible.
#[test]
fn an_overlay_owns_the_mouse_where_it_is_painted_while_the_layer_is_hidden() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_popup(&mut screen, 3, "OVERLAY");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(!tab.are_floating_panes_visible());

    // `overlay_coordinates()` puts the overlay at (2, 2), 60% wide and 4 rows tall
    assert!(
        tab.position_is_on_an_overlay(&Position::new(3, 4)),
        "the overlay is hit-tested inside its own geometry with the layer down"
    );
    assert!(
        !tab.position_is_on_an_overlay(&Position::new(15, 70)),
        "and owns nothing outside it"
    );

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(3, 4)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "clicking it focuses it"
    );
    assert!(
        !screen.tabs[&TabId::new(0)].are_floating_panes_visible(),
        "and does not raise the floating layer"
    );
}

/// While a modal exists it masks whatever else a client is focused on: `get_active_pane_id`
/// prefers the overlay over the tiled pane for as long as the modal is in the occlusion stack.
/// That is what a modal is for - it must never silently lose the keyboard, because it has no way
/// to learn that it did.
///
/// The tiled focus underneath is still recorded, which is what lets
/// `a_modal_overlay_hands_focus_to_the_pane_it_opened_when_it_closes` work: it surfaces again the
/// moment the modal goes away. This pins both halves, for both clients of a non-mirrored session.
///
/// A *dismissible* overlay answers the other way and gets its own test below - masking is the
/// modal's rule, not every overlay's.
#[test]
fn a_modal_masks_each_client_s_tiled_focus_until_it_closes() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    open_overlay_of_kind(&mut screen, 3, "MODAL", OverlayKind::Modal);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(2)),
        false,
        false,
        crate::ClientId::new(2),
    )
    .expect("TEST");

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "client 1 is on the modal"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "and so is client 2 - the modal masks the tiled pane it just focused"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and neither client's focus raises the floating layer"
    );

    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "once the modal is gone, the tiled focus recorded underneath surfaces again"
    );
}

// ---------------------------------------------------------------------------
// Render snapshots.
//
// Every other rendering claim in this file is a `.contains()` on
// `rendered_frame`, which says a pane is somewhere on screen but nothing about
// what is drawn over what. Occlusion is exactly what a z-order refactor breaks,
// so these pin whole frames.
// ---------------------------------------------------------------------------

/// Relative z-order between the three things that can be on top of the tiled panes while the
/// floating layer is up: a plain floating pane, a pinned floating pane, and an overlay.
///
/// The overlay paints above both, pinned included. It used to be the other way round: overlays
/// shared the floating layer's `z_indices`, and `make_sure_pinned_panes_are_on_top` had already
/// lifted the pinned pane to the end of that list. Now `Tab::occlusion_stack` appends the overlay
/// layers after every floating layer, so an overlay outranks anything on the layer by
/// construction - which is what a modal should do, being the one pane the user has to answer.
///
/// The frame also shows the one place overlay-ness reaches the pane frame: the two plain floating
/// panes carry a `PIN` toggle and the overlay does not.
#[test]
fn frame_of_an_overlay_with_a_pinned_and_a_plain_floating_pane() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "PLAINFLOATING", false);
    open_floating_pane(&mut screen, TabId::new(0), 3, "PINNEDFLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .change_floating_pane_coordinates(
            &PaneId::Terminal(crate::TerminalId::new(3)),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(4)),
                y: Some(PercentOrFixed::Fixed(3)),
                width: Some(PercentOrFixed::Fixed(30)),
                height: Some(PercentOrFixed::Fixed(5)),
                pinned: Some(true),
                ..Default::default()
            },
        )
        .expect("TEST");
    open_popup(&mut screen, 4, "OVERLAY");

    assert_snapshot!(rendered_frame(&mut screen, size));
}

/// A modal over a tiled pane fullscreened across the whole UI. This combination had no test
/// before Phase 4 because the overlay was simply not drawn - which left the modal focused and
/// invisible.
///
/// It is also the sharpest test of the flat-list discipline: `Tab::occlusion_stack` and
/// `render_floating_panes_and_overlays` index into one list, and a fullscreen tiled pane is
/// the case where the floating half of that list is not drawn while the overlay half is. A pane
/// handed an index that is not its own is clipped against its own geometry and erases itself, so
/// an empty box here is the failure this pins.
#[test]
fn frame_of_an_overlay_over_a_ui_covering_fullscreen_tiled_pane() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .handle_pty_bytes(crate::TerminalId::new(1), b"TILED".to_vec())
        .expect("TEST");
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));
    assert!(screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .fullscreen_covers_ui());

    open_popup(&mut screen, 2, "OVERLAY");

    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("OVERLAY"),
        "the overlay must be drawn over the fullscreen pane, got:\n{frame}"
    );
    assert_snapshot!(frame);
}

// ---------------------------------------------------------------------------
// Structural invariants.
//
// The characterization tests above each pin one path. This drives a random
// sequence of operations instead and asserts `Tab::assert_invariants` after
// every step, so a path nobody thought to write a test for still has to hold
// the line. The two invariants that only become true once overlays have their
// own collection are listed in that helper and switched on with the refactor.
// ---------------------------------------------------------------------------

mod invariant_props {
    use super::*;
    use hegel::generators;
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};

    /// One thing a user (or a plugin) can do to a tab. Deliberately includes the operations that
    /// the overlay special-casing threads through - opening onto a hidden layer, toggling the
    /// layer underneath an overlay - because those are where the collections have historically
    /// fallen out of step.
    #[derive(Debug, Clone)]
    enum Op {
        NewTiledPane,
        NewFloatingPane,
        NewOverlay,
        NewModalPlugin,
        ClosePane,
        ShowFloatingLayer,
        HideFloatingLayer,
        FocusFirstTiledPane,
        ToggleFullscreen,
        ToggleNoUiFullscreen,
        Resize(u16),
        ComposeOverlays(MultipaneDirection),
    }

    #[hegel::composite]
    fn op(tc: &hegel::TestCase) -> Op {
        match tc.draw(generators::integers::<u8>().max_value(12)) {
            0 => Op::NewTiledPane,
            1 => Op::NewFloatingPane,
            2 => Op::NewOverlay,
            3 => Op::NewModalPlugin,
            4 => Op::ClosePane,
            5 => Op::ShowFloatingLayer,
            6 => Op::HideFloatingLayer,
            7 => Op::FocusFirstTiledPane,
            8 => Op::ToggleFullscreen,
            9 => Op::ToggleNoUiFullscreen,
            10 => Op::Resize(tc.draw(generators::integers::<u16>().min_value(40).max_value(160))),
            11 => Op::ComposeOverlays(MultipaneDirection::LeftRight),
            _ => Op::ComposeOverlays(MultipaneDirection::TopBottom),
        }
    }

    #[hegel::test]
    fn a_tab_holds_its_invariants_under_any_sequence_of_pane_operations(tc: hegel::TestCase) {
        let op_count = tc.draw(generators::integers::<usize>().max_value(12));
        let ops: Vec<Op> = (0..op_count).map(|_| tc.draw(op())).collect();
        assert_sequence(&ops);
    }

    #[test]
    fn multipane_lifecycle_holds_tab_invariants() {
        for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
            assert_sequence(&[
                Op::NewOverlay,
                Op::NewOverlay,
                Op::ComposeOverlays(direction),
                Op::ToggleFullscreen,
                Op::Resize(100),
                Op::ToggleNoUiFullscreen,
                Op::ToggleNoUiFullscreen,
                Op::NewOverlay,
                Op::ClosePane,
                Op::ClosePane,
            ]);
        }
    }

    fn assert_sequence(ops: &[Op]) {
        let size = Size { cols: 80, rows: 20 };
        let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
        new_tab(&mut screen, 1, TabId::new(0));
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .assert_invariants();

        // pane ids have to be unique across the whole run, including the plugin ones
        let mut next_id = 2u32;
        // the ids we opened, so `ClosePane` picks a real one
        let mut opened: Vec<PaneId> = vec![];

        for (index, op) in ops.iter().enumerate() {
            // Invariant 3, the transition half: opening or focusing an overlay must never change
            // whether the floating layer is up. That reading - "floating panes are visible" -
            // lights the status bar's <f>, sends the next new pane to the layer, and puts every
            // hidden floating pane back on screen under a modal.
            //
            // Nothing is exempt. The one operation that used to be - a floating pane promoting
            // itself to an overlay, which had to put back the reveal its own birth caused - does
            // not exist any more: a pane is born an overlay or is never one.
            let layer_was_visible = screen
                .get_active_tab_mut(crate::ClientId::new(1))
                .unwrap()
                .are_floating_panes_visible();
            let on_an_overlay = {
                let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
                tab.get_active_pane_id(crate::ClientId::new(1))
                    .is_some_and(|pane_id| tab.pane_is_overlay(pane_id))
            };
            let op_may_move_the_layer = matches!(
                op,
                Op::NewTiledPane
                    | Op::NewFloatingPane
                    | Op::ClosePane
                    | Op::ShowFloatingLayer
                    | Op::HideFloatingLayer
                    | Op::FocusFirstTiledPane
                    | Op::Resize(_)
            ) ||
                // A fullscreen keystroke aimed at a *tiled* pane may legitimately change this
                // reading: "the floating layer is visible" is `show_panes && !fullscreen_covers_ui`
                // (`FloatingPanes::all_panes_are_visible`), so collapsing a UI-covering tiled
                // fullscreen puts the layer back on screen without anything having raised it.
                //
                // Aimed at an *overlay* it may not, and that is not exempted: an overlay's
                // expansion is an override on its own panes and touches neither of those two
                // terms. The assertion below is what says so on every random sequence that reaches
                // the keystroke with an overlay focused.
                (matches!(op, Op::ToggleFullscreen | Op::ToggleNoUiFullscreen) && !on_an_overlay);
            match op {
                Op::NewTiledPane | Op::NewFloatingPane | Op::NewOverlay => {
                    let pane_id = PaneId::Terminal(crate::TerminalId::new(next_id));
                    next_id += 1;
                    let placement = match op {
                        Op::NewTiledPane => NewPanePlacement::default(),
                        Op::NewFloatingPane => NewPanePlacement::Floating(None),
                        _ => NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                            overlay_coordinates(),
                        ))),
                    };
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .new_pane(
                            pane_id,
                            None,
                            None,
                            false,
                            true,
                            placement,
                            Some(crate::ClientId::new(1)),
                            None,
                        )
                        .expect("TEST");
                    opened.push(pane_id);
                },
                Op::NewModalPlugin => {
                    let pane_id = PaneId::Plugin(crate::PluginId::new(next_id));
                    next_id += 1;
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .new_pane(
                            pane_id,
                            Some("modal".to_owned()),
                            Some(modal_plugin_run()),
                            false,
                            true,
                            NewPanePlacement::Floating(None),
                            Some(crate::ClientId::new(1)),
                            None,
                        )
                        .expect("TEST");
                    opened.push(pane_id);
                },
                Op::ClosePane => {
                    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
                    // Closing one multipane member closes its sibling too.
                    opened.retain(|id| tab.has_pane_with_pid(id));
                    if !opened.is_empty() {
                        let pane_id = opened.remove(0);
                        screen
                            .get_active_tab_mut(crate::ClientId::new(1))
                            .unwrap()
                            .close_pane(pane_id, false, None);
                    }
                },
                Op::ShowFloatingLayer => {
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .show_floating_panes();
                },
                Op::HideFloatingLayer => {
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .hide_floating_panes();
                },
                Op::FocusFirstTiledPane => {
                    let _ = screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .focus_pane_with_id(
                            PaneId::Terminal(crate::TerminalId::new(1)),
                            false,
                            false,
                            crate::ClientId::new(1),
                        );
                },
                Op::ToggleFullscreen => {
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
                },
                Op::ToggleNoUiFullscreen => {
                    screen
                        .get_active_tab_mut(crate::ClientId::new(1))
                        .unwrap()
                        .toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));
                },
                Op::Resize(cols) => {
                    screen
                        .resize_to_screen(Size {
                            cols: *cols as usize,
                            rows: 20,
                        })
                        .expect("TEST");
                },
                Op::ComposeOverlays(direction) => {
                    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
                    let children: Vec<_> = opened
                        .iter()
                        .copied()
                        .filter(|id| {
                            tab.multipane_info(*id).is_none()
                                && tab.overlay_placement_for(*id).is_some_and(|placement| {
                                    placement.kind == OverlayKind::Dismissible
                                })
                        })
                        .take(2)
                        .collect();
                    if let [first, second] = children.as_slice() {
                        tab.create_multipane(
                            [*first, *second],
                            MultipaneSplit {
                                direction: *direction,
                                ratio: 5000,
                            },
                            0,
                            OverlayPlacement::dismissible(Some(overlay_coordinates())),
                        )
                        .expect("TEST");
                    }
                },
            }

            let tab = screen
                .get_active_tab_mut(crate::ClientId::new(1))
                .unwrap_or_else(|_| panic!("the tab is gone after {op:?} (step {index})"));
            if !op_may_move_the_layer {
                assert_eq!(
                    tab.are_floating_panes_visible(),
                    layer_was_visible,
                    "{op:?} (step {index}) changed whether the floating layer is up"
                );
            }
            tab.assert_invariants();
        }
    }
}

// ---------------------------------------------------------------------------
// The new placement.
// ---------------------------------------------------------------------------

/// Modal-ness used to be a string in a plugin's own configuration map, which meant a terminal
/// pane could not be one: there was no configuration to read. `NewPanePlacement::Overlay` carries
/// the kind with the placement, so any pane can be placed as a modal.
#[test]
fn a_terminal_pane_can_be_placed_as_a_modal() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal = PaneId::Terminal(crate::TerminalId::new(2));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            modal,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::modal(Some(overlay_coordinates()))),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(
        tab.pane_is_overlay(modal),
        "a pane placed as a modal is an overlay"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and does not raise the floating layer"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(modal),
        "and holds focus"
    );
    tab.assert_invariants();
}

/// The same placement without coordinates. Nothing is dropped on the way through: the placement
/// itself says the pane belongs on the surface, so there is no coordinates field for a caller to
/// rebuild without the flag.
#[test]
fn an_overlay_placement_without_coordinates_is_still_an_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let overlay = PaneId::Terminal(crate::TerminalId::new(2));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            overlay,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(overlay));
    assert!(!tab.are_floating_panes_visible());
    tab.assert_invariants();
}

/// `--modal` on the CLI is `--overlay` plus "do not take focus from it". Both reach the tab as an
/// overlay placement rather than as a coordinates flag a caller could drop.
#[test]
fn the_cli_modal_and_overlay_flags_both_produce_an_overlay_placement() {
    use {yinz_types::data::NewPanePlacement, yinz_types::data::OverlayKind};

    let modal =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, None, Some(true));
    assert!(matches!(
        &modal,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Modal
    ));

    let overlay =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, Some(true), None);
    assert!(matches!(
        &overlay,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Dismissible
    ));

    // --modal wins over --overlay, being the stricter of the two
    let both = NewPanePlacement::floating_or_overlay(
        None,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        Some(true),
    );
    assert!(matches!(
        &both,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Modal
    ));

    let plain =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, None, None);
    assert!(matches!(plain, NewPanePlacement::Floating(_)));
}

/// The compatibility shim: a plugin (or keybinding, or layout) that still spells overlay-ness as
/// `overlay`/`modal` keys in the free-form plugin configuration map resolves to the same
/// placement. This is the only place that spelling is understood.
#[test]
fn plugin_configuration_keys_resolve_to_an_overlay_placement() {
    use yinz_types::data::OverlayKind;

    let mut modal = yinz_runtime::plugin_location::run_plugin_from_url("yinz:quit-confirmation")
        .expect("test plugin");
    modal.configuration.insert("modal", "true");
    let modal = RunPluginOrAlias::RunPlugin(modal);
    assert_eq!(
        modal.overlay_placement().map(|placement| placement.kind),
        Some(OverlayKind::Modal)
    );

    let mut dismissible = yinz_runtime::plugin_location::run_plugin_from_url("yinz:file-finder")
        .expect("test plugin");
    dismissible.configuration.insert("overlay", "true");
    let dismissible = RunPluginOrAlias::RunPlugin(dismissible);
    assert_eq!(
        dismissible
            .overlay_placement()
            .map(|placement| placement.kind),
        Some(OverlayKind::Dismissible)
    );

    let plain = yinz_runtime::plugin_location::run_plugin_from_url("yinz:file-finder")
        .expect("test plugin");
    assert_eq!(
        RunPluginOrAlias::RunPlugin(plain).overlay_placement(),
        None,
        "a plugin that asks for neither is not an overlay"
    );
}

/// A modal is opened for the client who asked for it, but it is drawn for everybody - and a client
/// that attaches afterwards was not in `connected_clients` when the overlay took focus. Without
/// this it sees the dialog over its whole surface and types into the pane behind it, and the modal
/// has no way to learn that anyone is doing so.
#[test]
fn a_client_joining_while_a_modal_is_open_lands_on_the_modal() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(2)).unwrap();
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(modal_id),
        "the client that just attached must be on the modal it can see, not on the pane under it"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id),
        "and the client that opened it is where it was"
    );
}

/// The same door, opened the way a user actually opens it: no reattach, just walking into a tab
/// that has a modal in it. `switch_active_tab` reaches `Tab::add_client` through
/// `move_clients_between_tabs`, so this is the path that matters in a live session.
#[test]
fn a_client_switching_into_a_tab_with_a_modal_lands_on_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(2))
        .expect("TEST");
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(2))
        .expect("TEST");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(2)).unwrap();
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(modal_id),
        "walking into a tab with a modal in it puts the keyboard on the modal"
    );
}

/// The other half of the rule. A dismissible overlay that gave its focus up - which is how
/// `file-finder` learns to close itself - stays given up: a joining client inherits what the other
/// clients actually have, and nobody has that overlay. Focusing it here would resurrect a dialog
/// the user already dismissed.
#[test]
fn a_joining_client_does_not_pick_up_a_dismissible_overlay_that_gave_up_focus() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_popup(&mut screen, 2, "POPUP");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the popup gave the keyboard up to the pane the user asked for"
    );

    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    let tab = screen.get_active_tab_mut(crate::ClientId::new(2)).unwrap();
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "a joining client inherits the tiled focus, not an overlay nobody is on"
    );
}

/// The hit-test already knows the overlay is there - the write has to agree with it. Scroll
/// forwarding used to hand the position to `Tab::write_to_terminal_at`, which resolved it a second
/// time through the floating and tiled layers only: an overlay in alternate-screen mode had its
/// cursor keys delivered to the tiled pane underneath, so scrolling a dialog moved a shell behind
/// it.
#[test]
fn scrolling_a_terminal_overlay_in_alternate_mode_writes_to_the_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    // the tab copies the bus senders when it is created, so the capture goes in first
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    open_popup(&mut screen, 3, "OVERLAY");
    // DECSET 1049: the overlay is on the alternate screen, where scrolling is cursor keys
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .handle_pty_bytes(crate::TerminalId::new(3), b"\x1b[?1049h".to_vec())
        .expect("TEST");
    pty_writer_rx.try_iter().for_each(drop);

    // `overlay_coordinates()` puts the overlay at (2, 2), 60% wide and 4 rows tall
    screen.handle_mouse_event(
        MouseEvent::new_scroll_up_event(Position::new(3, 4)),
        crate::ClientId::new(1),
    );

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, terminal_id, _) => Some((terminal_id, bytes)),
            _ => None,
        })
        .collect();
    assert!(
        writes
            .iter()
            .all(|(terminal_id, _)| *terminal_id == crate::TerminalId::new(3)),
        "every write goes to the overlay, none to the tiled pane under it: {writes:?}"
    );
    assert_eq!(
        writes
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect::<Vec<_>>(),
        vec![b"\x1b[A".to_vec(); 3],
        "and the overlay got the cursor keys its alternate screen scrolls with"
    );
}

/// The same delivery, through the other branch: a terminal overlay that tracks the mouse itself
/// gets the SGR report rather than cursor keys, and it has to arrive at the overlay too.
#[test]
fn scrolling_a_terminal_overlay_that_tracks_the_mouse_writes_to_the_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    // the tab copies the bus senders when it is created, so the capture goes in first
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    open_popup(&mut screen, 3, "OVERLAY");
    // DECSET 1000 + 1006: report mouse buttons, in SGR encoding
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .handle_pty_bytes(
            crate::TerminalId::new(3),
            b"\x1b[?1000h\x1b[?1006h".to_vec(),
        )
        .expect("TEST");
    pty_writer_rx.try_iter().for_each(drop);

    screen.handle_mouse_event(
        MouseEvent::new_scroll_up_event(Position::new(3, 4)),
        crate::ClientId::new(1),
    );

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, terminal_id, _) => Some((terminal_id, bytes)),
            _ => None,
        })
        .collect();
    assert!(
        !writes.is_empty(),
        "the overlay tracks the mouse, so the scroll is reported to it"
    );
    assert!(
        writes
            .iter()
            .all(|(terminal_id, _)| *terminal_id == crate::TerminalId::new(3)),
        "and every report goes to the overlay, not to the tiled pane under it: {writes:?}"
    );
}

/// `Output` hides and cleans up the kitty image placements of any pane it is not told is visible.
/// An overlay is drawn for as long as it exists, so it belongs in that set - otherwise a terminal
/// overlay showing graphics has them wiped out from under it while it is plainly on screen.
#[test]
fn an_overlay_counts_as_visible_for_kitty_graphics() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(
        tab.kitty_visible_pane_ids()
            .contains(&PaneId::Terminal(crate::TerminalId::new(2))),
        "the overlay is on screen, so its kitty placements must not be cleaned up"
    );
}

// ---------------------------------------------------------------------------
// An overlay's geometry is a function of the surface and its coordinates, and of nothing else.
//
// `PaneGeom::adjust_coordinates` writes only the fields the coordinates name, so whatever the
// resolution starts from is what a coordinate that said nothing means. All three resolution
// points used to start from the pane's own geometry - and a pane built for this path arrives
// with `PaneGeom::default()`, whose 100%-constraint dimensions have an `inner` of 1. An overlay
// that named no size was born one cell square.
// ---------------------------------------------------------------------------

/// Open an overlay with exactly these coordinates, saying nothing the caller did not say.
fn open_overlay_with(screen: &mut Screen, pane_id: u32, coordinates: FloatingPaneCoordinates) {
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(pane_id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates))),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
}

/// `OverlayPlacement::coordinates` is documented as "`None` centres it at a default size", and
/// that is what every overlay born from plugin configuration asks for: `RunPlugin::overlay_placement`
/// returns `modal(None)` / `dismissible(None)` for the `modal`/`overlay` keys, and the fork's own
/// `new-agent` binding is one of them.
///
/// It used to be born 1x1 - a framed pane with no content area at all - and only recovered if the
/// plugin happened to re-assert its placement afterwards.
#[test]
fn an_overlay_that_named_no_size_gets_the_default_geometry() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 2, FloatingPaneCoordinates::default());

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 10),
        "half the 80x20 session surface, not one cell"
    );
    assert_eq!(
        (geom.x, geom.y),
        (20, 5),
        "centered on the surface, as a floating pane with no coordinates would be"
    );
}

/// Coordinates are resolved per axis: the one the caller named follows the coordinate, the one it
/// did not falls back to the surface default rather than to whatever the pane happened to carry.
#[test]
fn an_overlay_defaults_only_the_axis_its_coordinates_did_not_name() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            width: Some(PercentOrFixed::Percent(60)),
            ..Default::default()
        },
    );

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        geom.cols.as_usize(),
        48,
        "60% of the 80 columns it asked for"
    );
    assert_eq!(
        geom.rows.as_usize(),
        10,
        "and half the surface for the height it did not"
    );
}

/// The default is re-resolved against the surface like any other coordinate: an overlay has no
/// positional history, so a resize leaves it half of whatever the screen is *now* rather than
/// stranded at the absolute the old surface produced.
#[test]
fn an_overlay_that_named_no_size_follows_the_surface_across_a_resize() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 2, FloatingPaneCoordinates::default());

    screen
        .resize_to_screen(Size {
            cols: 120,
            rows: 20,
        })
        .expect("TEST");

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        geom.cols.as_usize(),
        60,
        "half of the new 120 columns, not the old 40"
    );
    assert_eq!(geom.x, 30, "and re-centered on the new surface");
}

/// `is_pinned` lives inside the `PaneGeom` rather than beside it, and `Tab::add_overlay_pane` sets
/// it on the pane before handing it over. Seeding the geometry from the surface without carrying
/// the flag across would drop something the caller explicitly asked for.
#[test]
fn an_overlay_that_named_no_size_keeps_the_pinned_flag_it_was_given() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            pinned: Some(true),
            ..Default::default()
        },
    );

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert!(
        geom.is_pinned,
        "the pinned flag survives the default geometry"
    );
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 10),
        "and it still got a usable size"
    );
}

/// An overlay is not in the tiled grid, so it cannot be the root of a stack - and saying so has to
/// happen *before* `Screen::stack_panes` extracts the members.
///
/// `has_room_for_stack` used to reject only a floating or suppressed root, and it measures the root
/// through `get_pane_with_id`, which resolves overlays: a tall enough overlay answered "yes, there
/// is room". Screen then pulled every member out of its tab, `TiledPanes::stack_panes` could not
/// find the root, and `Tab::stack_panes` returned - dropping the vec that was by then the only
/// thing holding those panes, while their processes went on running.
#[test]
fn an_overlay_cannot_be_the_root_of_a_stack_and_no_pane_is_lost_refusing() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    for pane_id in [2, 3] {
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .new_pane(
                PaneId::Terminal(crate::TerminalId::new(pane_id)),
                None,
                None,
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: Some(Direction::Down),
                    borderless: None,
                },
                Some(crate::ClientId::new(1)),
                None,
            )
            .expect("TEST");
    }
    let overlay = PaneId::Terminal(crate::TerminalId::new(4));
    open_overlay_with(&mut screen, 4, FloatingPaneCoordinates::default());

    // the size gate is not what refuses this: the overlay's default geometry is half the 20-row
    // surface, comfortably more than the `stack_size + MIN_TERMINAL_HEIGHT` the root is measured
    // against. Without the overlay arm in `has_room_for_stack` this stack would be accepted.
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(overlay)
            .expect("TEST")
            .position_and_size()
            .rows
            .as_usize()
            >= 3 + crate::tab::MIN_TERMINAL_HEIGHT
    );

    assert_eq!(
        screen.stack_panes(vec![
            overlay,
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Terminal(crate::TerminalId::new(3))
        ]),
        None,
        "an overlay root is refused"
    );

    // still *tiled*, not merely still reachable: `get_pane_with_id` would also find them parked
    // in the suppressed panes, and being parked is the salvage path, not the correct outcome
    let tiled: Vec<PaneId> = screen.tabs[&TabId::new(0)]
        .pane_infos()
        .into_iter()
        .filter(|info| !info.is_floating && !info.is_suppressed)
        .map(|info| info.id)
        .collect();
    for pane_id in [
        PaneId::Terminal(crate::TerminalId::new(2)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    ] {
        assert!(
            tiled.contains(&pane_id),
            "{pane_id:?} was never extracted, so it is still tiled in its tab - got {tiled:?}"
        );
    }
    assert!(
        screen.tabs[&TabId::new(0)].pane_is_overlay(overlay),
        "and the overlay is untouched"
    );
}

// --- Phase 7: the generic paths that still reached past the collection ------------------------

/// Open a *modal* terminal overlay in the given tab, by id.
///
/// A terminal rather than a plugin deliberately: a plugin's overlay-ness can be re-derived on the
/// far side of a move from its `modal` configuration key, so a plugin modal can survive a path
/// that drops the placement entirely. A terminal modal has no such second source and is the only
/// honest test of whether the placement travelled.
fn open_modal_terminal_in_tab(screen: &mut Screen, tab_id: TabId, pane_id: PaneId) {
    screen
        .tabs
        .get_mut(&tab_id)
        .expect("TEST")
        .new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::modal(Some(overlay_coordinates()))),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
}

/// Moving a pane between tabs is the one place overlay-ness can be lost without anybody
/// noticing: `extract_pane` drops the kind and coordinates on the way out, and the destination
/// has no way to re-derive them. `break_multiple_panes_to_tab_with_id` used to ask only
/// `pane_id_is_floating`, which an overlay answers `false` to - so a modal arrived as an
/// ordinary tiled pane, silently stripped of the one property it cannot do without.
#[test]
fn a_modal_broken_out_to_another_tab_arrives_as_a_modal() {
    let mut screen = screen_with_diverged_tab_ids();
    let modal = PaneId::Terminal(crate::TerminalId::new(42));
    open_modal_terminal_in_tab(&mut screen, TabId::new(2), modal);
    assert!(
        screen.tabs[&TabId::new(2)].pane_is_overlay(modal),
        "pre-condition"
    );

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(
            vec![modal],
            TabId::new(3),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.moved, vec![modal]);
    assert!(
        screen.tabs[&TabId::new(3)].pane_is_overlay(modal),
        "it is an overlay in the tab it landed in"
    );
    assert!(
        screen.tabs[&TabId::new(3)]
            .overlay_placement_for(modal)
            .expect("TEST")
            .is_modal(),
        "and a modal, not merely a dismissible overlay"
    );
    assert!(
        !screen.tabs[&TabId::new(3)].are_floating_panes_visible(),
        "an overlay arriving does not raise the destination's floating layer"
    );
}

/// The same reading of "not floating" as "tiled", on the path that makes a brand new tab.
#[test]
fn a_modal_broken_out_to_a_new_tab_arrives_as_a_modal() {
    let mut screen = screen_with_diverged_tab_ids();
    let modal = PaneId::Terminal(crate::TerminalId::new(42));
    open_modal_terminal_in_tab(&mut screen, TabId::new(2), modal);

    let new_tab_id = screen
        .break_multiple_panes_to_new_tab(vec![modal], None, false, None, crate::ClientId::new(1))
        .expect("TEST");

    assert!(
        screen.tabs[&new_tab_id].pane_is_overlay(modal),
        "an overlay broken out to a new tab is still an overlay there"
    );
    assert!(screen.tabs[&new_tab_id]
        .overlay_placement_for(modal)
        .expect("TEST")
        .is_modal());
    assert!(
        !screen.tabs[&TabId::new(2)].has_pane_with_pid(&modal),
        "and no longer in the tab it came from"
    );
}

/// `Tab::resize` was a two-way floating/tiled split, and an overlay answers `false` to the
/// floating half - so a resize keystroke with an overlay up dissolved the tab's stack lists and
/// resized whatever the overlay was covering. It now resizes the overlay itself; what must not
/// happen, then or now, is the keystroke reaching the panes behind it.
#[test]
fn a_resize_keystroke_on_an_overlay_leaves_the_pane_behind_it_alone() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .vertical_split(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .expect("TEST");
    open_floating_pane(&mut screen, TabId::new(0), 3, "OVERLAY", true);

    let before: Vec<PaneGeom> = [
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(2)),
    ]
    .iter()
    .map(|id| {
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(*id)
            .expect("TEST")
            .position_and_size()
    })
    .collect();
    assert_eq!(
        screen.tabs[&TabId::new(0)].get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "pre-condition: the overlay has the keyboard"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
        )
        .expect("TEST");

    let after: Vec<PaneGeom> = [
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(2)),
    ]
    .iter()
    .map(|id| {
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(*id)
            .expect("TEST")
            .position_and_size()
    })
    .collect();
    assert_eq!(
        before, after,
        "the tiled panes behind the overlay are untouched"
    );
}

/// The other half of the same keystroke: it reaches the overlay, rather than being swallowed the
/// way it was while overlays were unresizable.
#[test]
fn a_resize_keystroke_resizes_the_overlay_that_has_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);

    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .expect("TEST");

    let after = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert!(
        after.cols.as_usize() > before.cols.as_usize(),
        "the overlay grew: {before:?} -> {after:?}"
    );
}

/// The same keystroke with a fullscreen floating pane underneath. `Tab::resize` used to return at
/// its floating-fullscreen guard before ever reaching the overlay branch, so the overlay was
/// unresizable by keyboard in this one state - while ctrl-dragging its frame resized it fine.
#[test]
fn a_resize_keystroke_reaches_an_overlay_over_a_fullscreen_floating_pane() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert_eq!(
        screen.tabs[&TabId::new(0)].fullscreen_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the floating pane is the one that went fullscreen"
    );
    open_overlay_with(&mut screen, 3, overlay_coordinates());

    let floating_before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(3)))
        .expect("TEST")
        .position_and_size();

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .expect("TEST");

    let after = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(3)))
        .expect("TEST")
        .position_and_size();
    assert!(
        after.cols.as_usize() > before.cols.as_usize(),
        "the overlay grew: {before:?} -> {after:?}"
    );
    assert_eq!(
        floating_before,
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
            .expect("TEST")
            .position_and_size(),
        "the fullscreen floating pane behind it is untouched"
    );
}

/// The fullscreen keystroke, end to end, with an overlay holding the keyboard. It used to be
/// refused outright - and before that it fullscreened the tiled pane *behind* the overlay, which
/// is the pane the user was not looking at.
#[test]
fn a_fullscreen_keystroke_expands_the_overlay_that_has_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 2, overlay_coordinates());

    let tiled_before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("TEST")
        .position_and_size();
    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));

    let tab = &screen.tabs[&TabId::new(0)];
    assert!(
        tab.get_pane_info(PaneId::Terminal(crate::TerminalId::new(2)))
            .expect("TEST")
            .is_fullscreen,
        "the overlay is the pane the keystroke was about"
    );
    let expanded = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .current_geom();
    assert_eq!(
        (
            expanded.x,
            expanded.y,
            expanded.cols.as_usize(),
            expanded.rows.as_usize()
        ),
        (
            tiled_before.x,
            tiled_before.y,
            tiled_before.cols.as_usize(),
            tiled_before.rows.as_usize()
        ),
        "and it fills the viewport, which the lone tiled pane behind it also fills"
    );
    assert!(
        !tab.is_fullscreen_active(),
        "while the tab's own layout was never in question"
    );
    assert_eq!(
        tiled_before,
        tab.get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
            .expect("TEST")
            .position_and_size(),
        "and the pane behind it is untouched"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert_eq!(
        before,
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
            .expect("TEST")
            .position_and_size(),
        "and the second press puts it back where its coordinates said"
    );
}

/// A press naming an overlay that may not be expanded - a modal, here - still puts back the
/// expansion that was on screen, and what that screenful was covering has to be painted again.
///
/// The tab used to hang that repaint on the toggle answering `true`, which this press does not:
/// it collapses the expansion and *then* refuses. Nothing else asks a tiled pane that did not
/// change to draw itself, so what stayed on screen was a ghost of the overlay that had just been
/// put back.
#[test]
fn a_refused_fullscreen_press_repaints_what_the_expansion_covered() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .handle_pty_bytes(crate::TerminalId::new(1), b"TILED".to_vec())
        .expect("TEST");
    open_popup(&mut screen, 2, "OVERLAY");
    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(3));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(2)));

    // one grid across both renders, because the ghost is the frame the second render *fails* to
    // paint over - a fresh grid per render could not show it
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let mut grid = grid_for_render_snapshot(size);
    let expanded = render_frame_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert!(
        !expanded.contains("TILED"),
        "the expansion covers the tiled pane, got:\n{expanded}"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_pane_fullscreen(modal_id);

    assert!(
        screen.tabs[&TabId::new(0)].fullscreen_pane_id().is_none(),
        "a press naming a modal puts the expansion back, even though it may not expand the modal"
    );
    let collapsed = render_frame_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert!(
        collapsed.contains("TILED"),
        "and the pane the expansion was covering is painted again, got:\n{collapsed}"
    );
}

/// A tab whose two tiled panes are stacked one above the other, so that turning the lower one
/// into a ui pane - which is what `set_pane_selectable(false)` means: the status bar and the tab
/// bar are toggled exactly this way - takes rows off the viewport without emptying it.
fn new_tab_with_stacked_panes(screen: &mut Screen) {
    let client_id = ClientId::new(1);
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    screen
        .new_tab(TabId::new(0), (vec![], vec![]), None, Some(client_id))
        .expect("TEST");
    screen
        .apply_layout(
            layout,
            vec![],
            vec![
                (crate::TerminalId::new(1), None),
                (crate::TerminalId::new(2), None),
            ],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            client_id,
            None,
        )
        .expect("TEST");
}

/// The twin of `an_expanded_overlay_fills_the_viewport_a_resize_leaves`, for the other way the
/// viewport moves: a pane becoming chrome. `set_pane_selectable` recomputes the viewport out of
/// the ui panes on its edges, and an overlay expanded over the viewport - not over the whole
/// surface - is drawn against it, so it has to be re-derived here as well.
#[test]
fn an_expanded_overlay_follows_the_viewport_a_ui_pane_leaves() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab_with_stacked_panes(&mut screen);
    open_overlay_with(&mut screen, 3, overlay_coordinates());
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(3)));
    let lower = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert!(
        lower.y > 0,
        "the pane about to become chrome is the lower one"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(crate::TerminalId::new(2)), false);

    let tab = &screen.tabs[&TabId::new(0)];
    // the upper pane is what the viewport is now: the lower one is chrome, and the two of them
    // tile the whole surface between them
    let viewport = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("TEST")
        .position_and_size();
    let expanded = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(3)))
        .expect("TEST")
        .current_geom();
    assert_eq!(
        (
            expanded.x,
            expanded.y,
            expanded.cols.as_usize(),
            expanded.rows.as_usize()
        ),
        (
            viewport.x,
            viewport.y,
            viewport.cols.as_usize(),
            viewport.rows.as_usize()
        ),
        "the expansion stops at the chrome, rather than keeping the screenful it had before it"
    );
}

/// The same keystroke with a fullscreen floating pane underneath - the twin of
/// `a_resize_keystroke_reaches_an_overlay_over_a_fullscreen_floating_pane`, and the state where
/// the floating branch would happily have answered a keystroke that was never about it.
#[test]
fn a_fullscreen_keystroke_on_an_overlay_leaves_a_fullscreen_floating_pane_alone() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
    open_overlay_with(&mut screen, 3, overlay_coordinates());

    let floating_before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));

    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(PaneId::Terminal(crate::TerminalId::new(3)))
            .expect("TEST")
            .is_fullscreen,
        "the overlay is what the keystroke expanded"
    );
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(PaneId::Terminal(crate::TerminalId::new(2)))
            .expect("TEST")
            .is_fullscreen,
        "and the floating pane is still the floating layer's fullscreen"
    );
    assert_eq!(
        floating_before,
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
            .expect("TEST")
            .position_and_size(),
        "and it is untouched"
    );
}

/// The mouse half, end to end. `start_pane_resize_with_mouse` used to look in the floating and
/// tiled collections only, so an overlay id resolved to neither and errored the whole mouse event
/// - which is why the ctrl-press branch refused an overlay outright rather than being taught to
///   find one.
#[test]
fn a_ctrl_drag_on_an_overlay_frame_resizes_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 2, overlay_coordinates());

    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();

    // the bottom-right corner of the overlay's own frame, dragged 6 columns further right
    let row = (before.y + before.rows.as_usize() - 1) as i32;
    let from = (before.x + before.cols.as_usize() - 1) as u16;
    let mut press = MouseEvent::new_left_press_event(Position::new(row, from));
    press.ctrl = true;
    let mut motion = MouseEvent::new_left_motion_event(Position::new(row, from + 6));
    motion.ctrl = true;
    let mut release = MouseEvent::new_left_release_event(Position::new(row, from + 6));
    release.ctrl = true;
    screen.handle_mouse_event(press, crate::ClientId::new(1));
    screen.handle_mouse_event(motion, crate::ClientId::new(1));
    screen.handle_mouse_event(release, crate::ClientId::new(1));

    let after = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        after.cols.as_usize(),
        before.cols.as_usize() + 6,
        "the overlay followed the pointer: {before:?} -> {after:?}"
    );
    assert_eq!(
        (after.x, after.y),
        (before.x, before.y),
        "and grew from the corner that was grabbed"
    );
}

/// The other mouse gesture the overlay's frame advertises. `OverlayPanes::render` draws the frame
/// as a floating one, so the hint has always read `Ctrl <MouseScroll> or Ctrl <drag borders> to
/// resize` - but only the drag half was wired up, and `handle_resize_scroll` knew none of the
/// overlay collection.
#[test]
fn a_ctrl_scroll_on_an_overlay_resizes_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    // taller than `overlay_coordinates()`, whose 4 rows are already under `MIN_TERMINAL_HEIGHT`:
    // this test scrolls both ways, and a shrink that starts under the minimum cannot move
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            height: Some(PercentOrFixed::Fixed(8)),
            ..overlay_coordinates()
        },
    );

    let before = floating_geom(&screen, 2);
    // somewhere in the overlay's contents, well inside its frame
    let inside = Position::new(before.y as i32 + 1, before.x as u16 + 4);

    screen.handle_mouse_event(
        MouseEvent::new_ctrl_scroll_up_event(inside),
        crate::ClientId::new(1),
    );

    let grown = floating_geom(&screen, 2);
    // `direction: None` moves all four borders, and `geom_resized_within` halves a directionless
    // `change_by`, so one notch is 2 columns and 1 row on each side - the floating layer's step
    assert_eq!(
        (
            grown.x,
            grown.y,
            grown.cols.as_usize(),
            grown.rows.as_usize()
        ),
        (
            before.x - 2,
            before.y - 1,
            before.cols.as_usize() + 4,
            before.rows.as_usize() + 2
        ),
        "the overlay grew on all four sides: {before:?} -> {grown:?}"
    );

    screen.handle_mouse_event(
        MouseEvent::new_ctrl_scroll_down_event(inside),
        crate::ClientId::new(1),
    );
    assert_eq!(
        floating_geom(&screen, 2),
        before,
        "and a notch the other way puts it back"
    );
}

/// The regression the missing branch actually caused. An overlay is in neither the floating nor
/// the tiled collection, so the scroll fell through to the tiled branch and resized whatever the
/// client was focused on - which, since an overlay holds the keyboard, meant handing the tiled
/// collection a pane id it does not hold.
#[test]
fn a_ctrl_scroll_on_an_overlay_leaves_the_panes_behind_it_alone() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .vertical_split(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .expect("TEST");
    open_overlay_with(&mut screen, 3, overlay_coordinates());

    let tiled_before: Vec<PaneGeom> = [
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(2)),
    ]
    .iter()
    .map(|id| {
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(*id)
            .expect("TEST")
            .position_and_size()
    })
    .collect();
    assert_eq!(
        screen.tabs[&TabId::new(0)].get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "pre-condition: the overlay has the keyboard"
    );

    let overlay_before = floating_geom(&screen, 3);
    let inside = Position::new(overlay_before.y as i32 + 1, overlay_before.x as u16 + 4);
    screen.handle_mouse_event(
        MouseEvent::new_ctrl_scroll_up_event(inside),
        crate::ClientId::new(1),
    );

    assert_ne!(
        floating_geom(&screen, 3),
        overlay_before,
        "the overlay under the pointer is what resized"
    );
    let tiled_after: Vec<PaneGeom> = [
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(2)),
    ]
    .iter()
    .map(|id| {
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(*id)
            .expect("TEST")
            .position_and_size()
    })
    .collect();
    assert_eq!(
        tiled_before, tiled_after,
        "the tiled panes behind the overlay are untouched"
    );
}

/// A modal is sized to the question it is asking, so the scroll is nothing at all - and in
/// particular it does not fall through to the pane behind it, the way it used to.
#[test]
fn a_ctrl_scroll_on_a_modal_changes_nothing() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let modal = PaneId::Terminal(crate::TerminalId::new(2));
    open_modal_terminal_in_tab(&mut screen, TabId::new(0), modal);

    let before: Vec<PaneGeom> = [PaneId::Terminal(crate::TerminalId::new(1)), modal]
        .iter()
        .map(|id| {
            screen.tabs[&TabId::new(0)]
                .get_pane_with_id(*id)
                .expect("TEST")
                .position_and_size()
        })
        .collect();

    let modal_geom = before[1];
    let inside = Position::new(modal_geom.y as i32 + 1, modal_geom.x as u16 + 4);
    screen.handle_mouse_event(
        MouseEvent::new_ctrl_scroll_up_event(inside),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_ctrl_scroll_down_event(inside),
        crate::ClientId::new(1),
    );

    let after: Vec<PaneGeom> = [PaneId::Terminal(crate::TerminalId::new(1)), modal]
        .iter()
        .map(|id| {
            screen.tabs[&TabId::new(0)]
                .get_pane_with_id(*id)
                .expect("TEST")
                .position_and_size()
        })
        .collect();
    assert_eq!(
        before, after,
        "neither the modal nor the pane behind it moved"
    );
}

/// A modal keeps the size its plugin asked for, so the same keystroke changes nothing anywhere -
/// not the modal, and not the pane behind it.
#[test]
fn a_resize_keystroke_on_a_modal_changes_nothing() {
    let mut screen = screen_with_diverged_tab_ids();
    let modal = PaneId::Terminal(crate::TerminalId::new(42));
    open_modal_terminal_in_tab(&mut screen, TabId::new(2), modal);
    screen
        .tabs
        .get_mut(&TabId::new(2))
        .expect("TEST")
        .set_force_render();

    let before = screen.tabs[&TabId::new(2)]
        .get_pane_with_id(modal)
        .expect("TEST")
        .position_and_size();

    screen
        .tabs
        .get_mut(&TabId::new(2))
        .expect("TEST")
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .expect("TEST");

    assert_eq!(
        screen.tabs[&TabId::new(2)]
            .get_pane_with_id(modal)
            .expect("TEST")
            .position_and_size(),
        before,
        "a modal is sized by its plugin, not by the user"
    );
}

/// With `visual_bell` off, `check_and_consume_bells_without_visual_notification` is the *only*
/// thing that sees a bell - and its iterator chain was the visual one minus the overlay link.
/// A BEL from a modal was therefore neither forwarded to the client nor consumed off the pane.
#[test]
fn a_bell_in_an_overlay_is_heard_without_visual_notification() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);

    let tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), b"\x07".to_vec())
        .expect("TEST");

    assert!(
        tab.check_and_consume_bells_without_visual_notification(),
        "the bell rung inside the overlay is heard"
    );
    assert!(
        !tab.check_and_consume_bells_without_visual_notification(),
        "and consumed, so it does not ring forever"
    );
}

/// An overlay's frame is drawn over whatever is under it, and it is the overlay's own frame: the
/// drag it starts is the overlay's. The mouse pipeline used to decide "start moving a floating
/// pane" from the *global* layer-visibility flag rather than from the clicked pane, and the
/// executor then threw the pane id away and re-resolved the position through `FloatingPanes`
/// alone. Pressing a modal's frame therefore picked up the ordinary floating pane whose own frame
/// lay beneath it and dragged that.
#[test]
fn a_press_on_an_overlay_frame_does_not_drag_the_floating_pane_beneath_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    assert!(
        screen.tabs[&TabId::new(0)].are_floating_panes_visible(),
        "pre-condition: the layer is up, which is what the buggy test consulted"
    );

    // an overlay whose top frame row lies exactly on the floating pane's own top frame row
    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );

    let row = floating.y as i32;
    let from = floating.x as u16 + 2;
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row, from)),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(row, from - 5)),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(row, from - 5)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        floating_geom(&screen, 2),
        floating,
        "the floating pane under the overlay did not move"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "and the press landed on the overlay, which is what was drawn there"
    );
}

/// The same thing with the pressed overlay *unfocused*, which is a separate door: the press used
/// to classify as an ordinary focus, and `execute_focus_pane` went on to call
/// `move_pane_with_mouse` anyway - which re-resolves the position through `FloatingPanes` and
/// finds the pane the overlay is covering. It now classifies as an overlay drag, whether or not
/// the overlay pressed is the focused one.
#[test]
fn dragging_one_overlay_by_its_frame_does_not_drag_the_floating_pane_beneath_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);

    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    // a second overlay, elsewhere, which takes the focus - so the press below lands on an
    // overlay that is *not* active and classifies as a focus rather than a selection
    open_overlay_with(
        &mut screen,
        4,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(0)),
            y: Some(PercentOrFixed::Fixed(0)),
            width: Some(PercentOrFixed::Fixed(10)),
            height: Some(PercentOrFixed::Fixed(3)),
            ..Default::default()
        },
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(4))),
        "pre-condition: the overlay about to be pressed does not have the focus"
    );

    let row = floating.y as i32;
    let from = floating.x as u16 + 2;
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row, from)),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(row, from - 5)),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(row, from - 5)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        floating_geom(&screen, 2),
        floating,
        "the floating pane under the overlay did not move"
    );
    let overlay = floating_geom(&screen, 3);
    assert_eq!(
        overlay.x,
        floating.x - 5,
        "the overlay the press landed on is the one that moved"
    );
}

/// `break_pane_to_new_tab` moves the *active* pane, which is the overlay whenever one is up, and
/// it chose between floating and tiled from `are_floating_panes_visible` alone. A modal broken
/// out this way arrived in the neighbouring tab as an ordinary pane.
#[test]
fn a_modal_broken_out_to_the_neighbouring_tab_arrives_as_a_modal() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .switch_tab_prev(None, false, crate::ClientId::new(1))
        .expect("TEST");

    let modal = PaneId::Terminal(crate::TerminalId::new(3));
    open_modal_terminal_in_tab(&mut screen, TabId::new(0), modal);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(modal),
        "pre-condition"
    );

    screen
        .break_pane_to_new_tab(Direction::Right, crate::ClientId::new(1))
        .expect("TEST");

    assert!(
        screen.tabs[&TabId::new(1)].pane_is_overlay(modal),
        "it is an overlay in the tab it landed in"
    );
    assert!(screen.tabs[&TabId::new(1)]
        .overlay_placement_for(modal)
        .expect("TEST")
        .is_modal());
    assert!(
        !screen.tabs[&TabId::new(1)].are_floating_panes_visible(),
        "and did not raise that tab's floating layer on the way in"
    );
}

/// `Screen::break_pane` - the plain "break the active pane out to its own new tab" action - was
/// the one break path that never asked `placement_for_extraction`: it extracted whatever was
/// active and handed it to `add_tiled_pane` unconditionally. With an overlay up the active pane
/// *is* the overlay, so a modal broken out this way arrived in the new tab as an ordinary tiled
/// pane, stripped of its kind and coordinates.
#[test]
fn a_modal_broken_out_to_its_own_new_tab_arrives_as_a_modal() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .vertical_split(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .expect("TEST");

    let modal = PaneId::Terminal(crate::TerminalId::new(3));
    open_modal_terminal_in_tab(&mut screen, TabId::new(0), modal);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(modal),
        "pre-condition"
    );

    let tabs_before: HashSet<TabId> = screen.tabs.keys().copied().collect();
    screen
        .break_pane(None, Box::new(Layout::default()), crate::ClientId::new(1))
        .expect("TEST");
    let new_tab_id = *screen
        .tabs
        .keys()
        .find(|tab_id| !tabs_before.contains(tab_id))
        .expect("the break was refused");

    assert!(
        screen.tabs[&new_tab_id].pane_is_overlay(modal),
        "it is an overlay in the tab it made"
    );
    assert!(
        screen.tabs[&new_tab_id]
            .overlay_placement_for(modal)
            .expect("TEST")
            .is_modal(),
        "and a modal, not merely a dismissible overlay"
    );
    assert!(
        !screen.tabs[&new_tab_id].are_floating_panes_visible(),
        "and did not raise the new tab's floating layer on the way in"
    );
    assert!(
        !screen.tabs[&TabId::new(0)].has_pane_with_pid(&modal),
        "and is no longer in the tab it came from"
    );
    assert_eq!(
        screen.tabs[&TabId::new(0)].get_selectable_tiled_panes_count(),
        2,
        "which kept both of the panes the overlay was covering"
    );
}

/// An overlay is not on a layer and does not tile, so taking it out cannot leave its tab without
/// a pane - but the guard counted only what was underneath it, so over a single tiled pane the
/// break was refused with "Cannot break single pane out!".
#[test]
fn an_overlay_can_be_broken_out_over_a_single_tiled_pane() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal = PaneId::Terminal(crate::TerminalId::new(2));
    open_modal_terminal_in_tab(&mut screen, TabId::new(0), modal);
    assert_eq!(
        screen.tabs[&TabId::new(0)].get_selectable_tiled_panes_count(),
        1,
        "pre-condition: nothing but the overlay and the pane it covers"
    );

    let tabs_before: HashSet<TabId> = screen.tabs.keys().copied().collect();
    screen
        .break_pane(None, Box::new(Layout::default()), crate::ClientId::new(1))
        .expect("TEST");
    let new_tab_id = *screen
        .tabs
        .keys()
        .find(|tab_id| !tabs_before.contains(tab_id))
        .expect("the break was refused");

    assert!(screen.tabs[&new_tab_id].pane_is_overlay(modal));
    assert!(
        screen.tabs[&TabId::new(0)].has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

/// An overlay does not tile, so the new tab still wants the layout's pane underneath it. The
/// tiled arm tells the layout "this pane already exists, do not spawn it" - doing that for an
/// overlay would leave a modal floating over an empty tab.
#[test]
fn breaking_an_overlay_out_still_spawns_the_new_tabs_own_pane() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let (plugin_tx, plugin_rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));

    open_modal_terminal_in_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Terminal(crate::TerminalId::new(2)),
    );
    screen
        .break_pane(None, Box::new(Layout::default()), crate::ClientId::new(1))
        .expect("TEST");

    let ignored = plugin_rx
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::NewTab(_, _, tiled_panes_layout, ..) => tiled_panes_layout,
            _ => None,
        })
        .expect("no new tab was requested of the plugin thread")
        .run_instructions_to_ignore;
    assert!(
        ignored.is_empty(),
        "the overlay must not stand in for the new tab's own pane: {ignored:?}"
    );
}

/// `execute_focus_on_hover` asked whether the hovered pane was on the *visible floating layer*,
/// and an overlay answers `false` - so with the layer up, focus-follows-mouse over a modal
/// reduced to a hover and left the keyboard where it was. An overlay is painted over everything
/// with no layer precondition, which is the whole reason it is not a floating pane.
#[test]
fn focus_follows_mouse_lands_on_an_overlay_over_a_visible_floating_layer() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "FLOATING", false);
    assert!(
        screen.tabs[&TabId::new(0)].are_floating_panes_visible(),
        "pre-condition"
    );

    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    // a second overlay elsewhere takes the focus, so there is somewhere for the hover to move it
    // *from*
    open_overlay_with(
        &mut screen,
        4,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(0)),
            y: Some(PercentOrFixed::Fixed(0)),
            width: Some(PercentOrFixed::Fixed(10)),
            height: Some(PercentOrFixed::Fixed(3)),
            ..Default::default()
        },
    );
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .update_focus_follows_mouse(true);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(4)))
    );

    // the body of the first overlay, not its frame
    screen.handle_mouse_event(
        MouseEvent::new_buttonless_motion(Position::new(
            floating.y as i32 + 2,
            floating.x as u16 + 2,
        )),
        crate::ClientId::new(1),
    );

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the hover focused the overlay under the cursor"
    );
}

// ---------------------------------------------------------------------------
// A live modal owns the keyboard and the top of the stack.
//
// A modal has no way to learn that it lost focus, so an overlay opened after it must not take
// either. Both halves matter: focus alone would leave the modal holding a keyboard the user
// cannot see it holding, buried under whatever opened last.
// ---------------------------------------------------------------------------

/// Open an overlay of an explicit kind, overlapping the region `open_popup` uses so that whichever
/// one is on top is the one that shows.
fn open_overlay_of_kind(screen: &mut Screen, pane_id: u32, contents: &str, kind: OverlayKind) {
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    };
    let placement = match kind {
        OverlayKind::Modal => OverlayPlacement::modal(Some(coordinates)),
        OverlayKind::Dismissible => OverlayPlacement::dismissible(Some(coordinates)),
    };
    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(placement),
        None, // This fixture opens an overlay for every client on the tab.
        None,
    )
    .expect("TEST");
    tab.handle_pty_bytes(
        crate::TerminalId::new(pane_id),
        contents.as_bytes().to_vec(),
    )
    .expect("TEST");
}

#[test]
fn an_overlay_opened_over_a_modal_takes_neither_the_keyboard_nor_the_top_of_the_stack() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "THEMODAL", OverlayKind::Modal);
    open_overlay_of_kind(&mut screen, 3, "LATEROVERLAY", OverlayKind::Dismissible);

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the modal keeps the keyboard"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("THEMODAL"),
        "and stays visible, rather than being buried under the overlay that opened after it, \
         got:\n{frame}"
    );
    assert!(
        !frame.contains("LATEROVERLAY"),
        "the later overlay is behind the modal, which covers it entirely, got:\n{frame}"
    );
    assert_snapshot!(frame);
}

#[test]
fn a_modal_opened_over_a_modal_takes_both_and_hands_them_back_when_it_closes() {
    // the deliberate exception: without it a quit-confirmation raised over an already open modal
    // would come up unfocused and unable to close itself
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "FIRSTMODAL", OverlayKind::Modal);
    open_overlay_of_kind(&mut screen, 3, "SECONDMODAL", OverlayKind::Modal);

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3)))
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("SECONDMODAL"),
        "the newer modal is the one on top, got:\n{frame}"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "closing it walks the keyboard back down to the modal underneath"
    );
}

/// The reveal side of that rule. A modal that was set aside before a newer one opened is not the
/// dialog the user is in, so unsuppressing it must put it back *underneath* the newer modal and
/// leave the keyboard alone - `show_pane`'s unconditional raise-and-focus used to do the opposite,
/// because for a modal the top of its own class is the top of the whole stack.
#[test]
fn an_unsuppressed_modal_stays_below_the_modal_that_came_up_while_it_was_away() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "FIRSTMODAL", OverlayKind::Modal);
    screen
        .tab_containing_overlay_mut(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .hide_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    open_overlay_of_kind(&mut screen, 3, "SECONDMODAL", OverlayKind::Modal);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3)))
    );

    screen
        .tab_containing_overlay_mut(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .show_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the newer modal keeps the keyboard - it has no way to learn otherwise"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("SECONDMODAL"),
        "and the top of the stack, got:\n{frame}"
    );
    assert!(
        !frame.contains("FIRSTMODAL"),
        "the revealed modal is back on screen, underneath it, got:\n{frame}"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "so the revealed modal is not stranded: closing the newer one walks focus down to it"
    );
}

/// An overlay outranks the floating layer in `get_active_pane_id`, so a dismissible overlay that
/// keeps its focus when a floating pane opens leaves that pane behind the popup, with the keyboard
/// still on the popup - and the popup, which learns it should close by seeing itself unfocused,
/// never finds out.
#[test]
fn a_focused_floating_pane_opened_over_a_dismissible_overlay_takes_the_keyboard() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "THEPOPUP", OverlayKind::Dismissible);
    open_floating_pane(&mut screen, TabId::new(0), 3, "THEFLOATER", false);

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the floating pane the user just asked for has the keyboard, rather than the popup \
         it opened over - an overlay that still held focus would win this lookup"
    );
}

/// The modal half of the same rule: a modal has no way to learn it lost focus, so a floating pane
/// opened over it comes up behind it rather than stranding it on screen with the input elsewhere.
#[test]
fn a_focused_floating_pane_opened_over_a_modal_leaves_the_keyboard_alone() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "THEMODAL", OverlayKind::Modal);
    open_floating_pane(&mut screen, TabId::new(0), 3, "THEFLOATER", false);

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the modal keeps the keyboard"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("THEMODAL"),
        "and stays visible over the floating pane that opened under it, got:\n{frame}"
    );
}

/// `set_pane_selectable(false)` moves clients off an overlay; a later `FocusPaneWithId` used to
/// walk straight past that and re-insert it as the active pane, routing input to a pane that had
/// explicitly asked for none.
#[test]
fn a_non_selectable_overlay_cannot_be_focused_again() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "THEPOPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId::new(2)), false);
    assert_ne!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "making it non-selectable moved the client off it"
    );

    tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(2)),
        false,
        false,
        crate::ClientId::new(1),
    )
    .expect_err("focusing a non-selectable pane must report an error");
    assert_ne!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "and focusing it by id does not put the client back onto a pane that wants no input - \
         an active overlay would outrank every other layer in this lookup"
    );
}

/// Whatever arms the display clear, every layer that is still logically on screen has to repaint
/// into the frame that carries it.
///
/// `set_should_clear_display_before_rendering` schedules a full `ESC[2J` that goes out during the
/// tiled leg of `Tab::render`, but the overlays draw later, from
/// `render_floating_panes_and_overlays`. The setter used to force-render only the floating
/// layer, so an overlay that was already on screen and clean contributed nothing to the frame that
/// wiped it: opening a tiled pane behind a modal erased the modal while it still held the keyboard,
/// which is a state the user cannot get out of.
#[test]
fn a_modal_survives_a_display_clear_armed_by_a_tiled_pane_behind_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_popup(&mut screen, 2, "MODAL");

    let first_frame = rendered_frame(&mut screen, size);
    assert!(
        first_frame.contains("MODAL"),
        "the modal is on screen to begin with, got:\n{first_frame}"
    );

    // opening a tiled pane arms the clear (see the `add_tiled_pane` path) and does not touch the
    // overlay, which is by now clean. The new pane deliberately does *not* take focus: a focus
    // change repaints the modal's frame on its own and would hide the bug, so this is the case
    // that isolates it.
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(3)),
            None,
            None,
            false,
            false,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");

    // a fresh grid, so this snapshot holds only what *this* frame emitted - exactly what the user
    // is left looking at after the clear
    let second_frame = rendered_frame(&mut screen, size);
    assert!(
        second_frame.contains("MODAL"),
        "the frame that clears the display has to redraw the modal it wiped, got:\n{second_frame}"
    );
}

// ---------------------------------------------------------------------------
// Phase 10: a lower layer taking focus, and what the overlay above it does about it.
//
// `Tab::get_active_pane_id` prefers an active overlay over every other layer, so overlay focus is
// not one opinion among three - it is the answer. A lower collection that took focus without
// saying so left a stale overlay entry masking it: input went on reaching the popup, and a
// dismiss-on-defocus popup never learned it had lost the keyboard. The rule these pin has three
// parts: a modal makes the move a no-op, a dismissible overlay gives the keyboard up, and a move
// that did not happen dismisses nothing.
// ---------------------------------------------------------------------------

/// A floating pane at an explicit place on the surface, so that a click can be aimed at it.
fn open_floating_pane_at(screen: &mut Screen, pane_id: u32, x: usize, y: usize) {
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(x)),
            y: Some(PercentOrFixed::Fixed(y)),
            width: Some(PercentOrFixed::Fixed(15)),
            height: Some(PercentOrFixed::Fixed(5)),
            ..Default::default()
        })),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
}

/// Two floating panes clear of the overlay's geometry - `open_overlay_of_kind` puts that at
/// (2, 2), 60% of the width and 4 rows tall - and an overlay of `kind` above them, holding the
/// keyboard. Floating pane 4 is the one with the floating layer's focus.
fn screen_with_floating_panes_under_an_overlay(size: Size, kind: OverlayKind) -> Screen {
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane_at(&mut screen, 2, 60, 10);
    open_floating_pane_at(&mut screen, 4, 5, 10);
    open_overlay_of_kind(&mut screen, 3, "OVERLAY", kind);
    assert!(
        screen.tabs[&TabId::new(0)].are_floating_panes_visible(),
        "the floating layer is up, which is what makes the panes underneath clickable"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "and the overlay holds the keyboard to begin with"
    );
    screen
}

/// Clicking a floating pane underneath a dismissible overlay takes the keyboard from it.
///
/// `MouseHandler::focus_pane_at` swallows a click that lands *on* an overlay, but a click below
/// one used to update `floating_panes.active_panes` alone. `get_active_pane_id` prefers the
/// overlay, so the click focused a pane that then received nothing: the keyboard stayed on the
/// popup, and the popup - which closes by seeing itself unfocused - was never told.
#[test]
fn clicking_a_floating_pane_under_a_dismissible_overlay_takes_the_keyboard_from_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_floating_panes_under_an_overlay(size, OverlayKind::Dismissible);

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(12, 65)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the click reaches the pane it landed on, keyboard and all - which it could not do while \
         the overlay was still holding focus, since that outranks every other layer"
    );
}

/// The same click under a modal moves nothing at all.
///
/// A modal owns the keyboard until it closes, so focusing the pane underneath would be a focus
/// change the user could neither see nor act on - and would leave the floating layer pointing
/// somewhere else for when the modal finally goes away.
#[test]
fn clicking_a_floating_pane_under_a_modal_moves_nothing() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_floating_panes_under_an_overlay(size, OverlayKind::Modal);

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(12, 65)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "the modal keeps the keyboard"
    );

    // and the floating layer's own focus was left where it was rather than moved out of sight,
    // which is what surfaces once the modal is gone
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(4))),
        "the click under the modal moved nothing underneath it either"
    );
}

/// Keyboard focus movement answers the same way a click does: the dismissible overlay gives the
/// keyboard up to the pane the move lands on.
#[test]
fn moving_focus_under_a_dismissible_overlay_takes_the_keyboard_from_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_floating_panes_under_an_overlay(size, OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    assert!(
        tab.move_focus_right(crate::ClientId::new(1)).expect("TEST"),
        "there is a floating pane to the right of the focused one"
    );

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "and the keyboard went with the move"
    );
}

/// Under a modal the same move is a no-op, in both halves: it reports that nothing moved, and
/// nothing did - not even the floating layer's own idea of where focus is.
#[test]
fn moving_focus_under_a_modal_is_a_no_op() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = screen_with_floating_panes_under_an_overlay(size, OverlayKind::Modal);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    assert!(
        !tab.move_focus_right(crate::ClientId::new(1)).expect("TEST"),
        "the modal owns the keyboard, so there is no move to report"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3)))
    );

    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(4))),
        "and nothing moved underneath it either, which is what surfaces once the modal is gone"
    );
}

/// A move that found nothing dismisses nothing.
///
/// The release happens after the move and only if the move happened. Releasing on the way in
/// would let a keypress with nowhere to go close the popup the user was reading - and drop the
/// keyboard onto the tiled pane behind it, which is not what pressing "focus right" against the
/// edge of the surface means.
#[test]
fn a_focus_move_that_goes_nowhere_leaves_a_dismissible_overlay_holding_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    assert!(
        !tab.move_focus_right(crate::ClientId::new(1)).expect("TEST"),
        "one tiled pane, so there is nothing to the right of it"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "and the popup still has the keyboard"
    );
}

/// The tiled focus cycle is the same rule again: `focus_next_pane` moves the keyboard, so a
/// dismissible overlay above it has to let go.
#[test]
fn cycling_focus_under_a_dismissible_overlay_takes_the_keyboard_from_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    open_overlay_of_kind(&mut screen, 3, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.focus_next_pane(crate::ClientId::new(1));

    assert!(
        !matches!(
            tab.get_active_pane_id(crate::ClientId::new(1)),
            Some(PaneId::Terminal(TERMINAL_3))
        ),
        "the keyboard left the popup for the tiled pane the cycle landed on, got {:?}",
        tab.get_active_pane_id(crate::ClientId::new(1))
    );
}

/// Focusing a pane by id is an explicit "focus *this* pane", so it takes the shape Phase 9 settled
/// on for opening a pane the user asked for: a dismissible overlay gives the keyboard up, and a
/// modal keeps it (`a_modal_masks_each_client_s_tiled_focus_until_it_closes` pins that half).
#[test]
fn focusing_a_pane_by_id_under_a_dismissible_overlay_takes_the_keyboard_from_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    open_overlay_of_kind(&mut screen, 3, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(2)),
        false,
        false,
        crate::ClientId::new(2),
    )
    .expect("TEST");

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "client 2 asked for the tiled pane and got it, keyboard and all"
    );
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "and the other client, which asked for nothing, is left on the popup"
    );
}

/// A focus move made against a modal must not escalate: the keypress is a no-op, not a hop to
/// the previous tab. `move_focus_left` answers `Ok(false)` while a modal holds the keyboard,
/// and `move_focus_left_or_previous_tab` used to read that as "at the edge" and switch tabs.
#[test]
fn a_focus_move_at_the_edge_under_a_modal_does_not_switch_tabs() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    open_overlay_of_kind(&mut screen, 3, "MODAL", OverlayKind::Modal);
    assert_eq!(
        screen
            .get_active_tab(crate::ClientId::new(1))
            .expect("TEST")
            .position,
        TabPosition::new(1)
    );

    screen
        .move_focus_left_or_previous_tab(crate::ClientId::new(1))
        .expect("TEST");

    assert_eq!(
        screen
            .get_active_tab(crate::ClientId::new(1))
            .expect("TEST")
            .position,
        TabPosition::new(1),
        "the modal owns the keyboard, so the keypress is a no-op rather than a tab switch"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "and the modal still has the keyboard"
    );
}

/// The same move in a single-tab session used to take the other escalation: with no previous tab
/// to switch to, the focus was bubbled out to the host session while the modal was still up.
#[test]
fn a_focus_move_at_the_edge_under_a_modal_does_not_bubble_to_the_host() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "MODAL", OverlayKind::Modal);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    screen
        .move_focus_left_or_previous_tab(crate::ClientId::new(1))
        .expect("TEST");

    while let Ok((instruction, _)) = server_receiver.try_recv() {
        assert!(
            !matches!(
                instruction,
                ServerInstruction::EmitNestedSessionFrameToClient(..)
            ),
            "nothing was handed to the host session"
        );
    }
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the modal still has the keyboard"
    );
}

/// The two halves of a left/right multipane sit side by side, so Alt h/l cross the divider
/// between them rather than moving the focus of the pane hidden under the panel - which, at the
/// edge of the tiled layout, used to switch tabs with the panel in tow.
#[test]
fn a_directional_move_crosses_a_left_right_multipane_to_its_sibling() {
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let children = panel_multipane(&mut screen, TabId::new(1));
    let position = |screen: &Screen| screen.get_active_tab(CLIENT_1).expect("TEST").position;
    assert_eq!(position(&screen), TabPosition::new(1));

    screen.move_focus_right_or_next_tab(CLIENT_1).expect("TEST");
    assert_eq!(screen.active_pane_id(CLIENT_1), Some(children[1]));
    assert_eq!(position(&screen), TabPosition::new(1), "and no tab switch");

    screen
        .move_focus_left_or_previous_tab(CLIENT_1)
        .expect("TEST");
    assert_eq!(screen.active_pane_id(CLIENT_1), Some(children[0]));

    screen
        .move_focus_in_direction_or_escalate(Direction::Right, CLIENT_1)
        .expect("TEST");
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(children[1]),
        "plain MoveFocus crosses the same way"
    );
    assert_eq!(position(&screen), TabPosition::new(1));
}

/// Off the panel's outer edge the move carries on from the pane hidden under it, and that tab has
/// one tiled pane, so nothing lies beyond the covered pane. While any overlay holds the keyboard
/// that is a no-op, as it is under a modal: switching tabs would have agent carry the panel into
/// the new tab.
#[test]
fn a_focus_move_or_tab_at_the_edge_under_a_dismissible_overlay_does_not_switch_tabs() {
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let children = panel_multipane(&mut screen, TabId::new(1));
    // down, as a real overlay leaves it, so the move looks among the tiled panes
    // (`a_focus_move_or_tab_past_the_panel_edge_moves_to_the_tiled_pane_beyond_the_covered_one`)
    screen
        .get_active_tab_mut(CLIENT_1)
        .expect("TEST")
        .hide_floating_panes();
    let position = |screen: &Screen| screen.get_active_tab(CLIENT_1).expect("TEST").position;

    screen
        .move_focus_left_or_previous_tab(CLIENT_1)
        .expect("TEST");
    assert_eq!(position(&screen), TabPosition::new(1));
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(children[0]),
        "the list keeps the keyboard"
    );

    screen.move_focus_right_or_next_tab(CLIENT_1).expect("TEST");
    screen.move_focus_right_or_next_tab(CLIENT_1).expect("TEST");
    assert_eq!(position(&screen), TabPosition::new(1));
    assert_eq!(screen.active_pane_id(CLIENT_1), Some(children[1]));
}

/// The same rule without a multipane: a plain dismissible popup holding the keyboard over a tab
/// with one tiled pane. Alt h/l find nothing either way, so neither hops to the other tab.
#[test]
fn a_focus_move_or_tab_at_the_edge_under_a_single_dismissible_overlay_does_not_switch_tabs() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    open_overlay_of_kind(&mut screen, 3, "POPUP", OverlayKind::Dismissible);
    let position = |screen: &Screen| screen.get_active_tab(CLIENT_1).expect("TEST").position;
    assert_eq!(position(&screen), TabPosition::new(1));

    screen.move_focus_right_or_next_tab(CLIENT_1).expect("TEST");
    assert_eq!(position(&screen), TabPosition::new(1), "no tab switch");
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(PaneId::Terminal(TERMINAL_3)),
        "and the popup still has the keyboard"
    );

    screen
        .move_focus_left_or_previous_tab(CLIENT_1)
        .expect("TEST");
    assert_eq!(position(&screen), TabPosition::new(1));
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(PaneId::Terminal(TERMINAL_3))
    );
}

/// Plain `MoveFocus` (Alt j/k, and h/l in pane mode) keeps to the same rule as `MoveFocusOrTab`.
/// At the edge of a single-tab session it used to bubble the focus out to the host session while
/// a dismissible popup still held the keyboard - only a modal stopped it.
#[test]
fn a_focus_move_at_the_edge_under_a_dismissible_overlay_does_not_bubble_to_the_host() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "POPUP", OverlayKind::Dismissible);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let assert_nothing_bubbled = |what: &str| {
        while let Ok((instruction, _)) = server_receiver.try_recv() {
            assert!(
                !matches!(
                    instruction,
                    ServerInstruction::EmitNestedSessionFrameToClient(..)
                ),
                "{what}: nothing was handed to the host session"
            );
        }
    };

    screen
        .move_focus_in_direction_or_escalate(Direction::Down, CLIENT_1)
        .expect("TEST");
    assert_nothing_bubbled("MoveFocus down");
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the popup still has the keyboard"
    );

    screen
        .move_focus_left_or_previous_tab(CLIENT_1)
        .expect("TEST");
    assert_nothing_bubbled("MoveFocusOrTab left");
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );
}

/// Past the panel's outer edge Alt h/l carry on from the pane hidden under it, as they would
/// without the panel. Here there is a tiled pane that way, so the keyboard goes to it - the
/// panel lets go as it does for a click on that pane - and the tab stays where it is.
#[test]
fn a_focus_move_or_tab_past_the_panel_edge_moves_to_the_tiled_pane_beyond_the_covered_one() {
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let left = PaneId::Terminal(crate::TerminalId::new(2));
    let right = PaneId::Terminal(TERMINAL_3);
    screen
        .get_active_tab_mut(CLIENT_1)
        .expect("TEST")
        .vertical_split(right, None, None, CLIENT_1, None, None)
        .expect("TEST");
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(right),
        "pre-condition: the right-hand tiled pane is focused"
    );
    let children = panel_multipane(&mut screen, TabId::new(1));
    // `open_overlay_plugin` raises the floating layer, which a real overlay leaves as it was
    // (`opening_an_overlay_does_not_raise_the_floating_layer`), and a raised layer with nothing
    // on it is where the move would look instead of among the tiled panes
    screen
        .get_active_tab_mut(CLIENT_1)
        .expect("TEST")
        .hide_floating_panes();
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(children[0]),
        "pre-condition: the list has the keyboard"
    );
    let position = |screen: &Screen| screen.get_active_tab(CLIENT_1).expect("TEST").position;

    screen
        .move_focus_left_or_previous_tab(CLIENT_1)
        .expect("TEST");

    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(left),
        "the list has no half to its left, so the move went on from the covered pane"
    );
    assert_eq!(position(&screen), TabPosition::new(1), "and no tab switch");
}

/// agent carries its panel into the tab its client just switched to (`follow_tab`, after a tab
/// switch or `Ctrl t n`). By then the client is no longer on the panel's tab, so the panel lands
/// drawn but unfocused and the pane the client went to keeps the keyboard. A break done while the
/// client is still on the panel's tab is the counterpart: the list keeps it.
#[test]
fn a_panel_carried_after_its_client_switched_tabs_arrives_unfocused() {
    for client_left_first in [true, false] {
        let mut screen = create_non_mirrored_screen(Size {
            cols: 120,
            rows: 40,
        });
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        let children = panel_multipane(&mut screen, TabId::new(1));
        if client_left_first {
            screen
                .switch_active_tab(TabPosition::new(0), None, true, CLIENT_1)
                .expect("TEST");
        }

        screen
            .break_multiple_panes_to_tab_with_id(
                vec![children[0]],
                TabId::new(0),
                !client_left_first,
                CLIENT_1,
            )
            .expect("TEST");

        let destination = &screen.tabs[&TabId::new(0)];
        for child in children {
            assert!(
                destination.pane_is_overlay(child),
                "client left first: {client_left_first}: both halves travel as an overlay"
            );
        }
        assert_eq!(
            screen.get_active_tab(CLIENT_1).expect("TEST").id,
            TabId::new(0)
        );
        if client_left_first {
            assert_eq!(
                screen.active_pane_id(CLIENT_1),
                Some(PaneId::Terminal(crate::TerminalId::new(1))),
                "the tab the client switched to keeps the keyboard"
            );
            assert!(!destination.overlay_holds_focus(CLIENT_1));
        } else {
            assert_eq!(
                screen.active_pane_id(CLIENT_1),
                Some(children[0]),
                "a break made from the panel's own tab keeps the list focused"
            );
        }
    }
}

/// Switching panes while fullscreen is active is a focus move like any other, so a dismissible
/// overlay above it has to let go. The fullscreen branch used to return before the release.
#[test]
fn switching_panes_in_fullscreen_releases_a_dismissible_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
    open_overlay_of_kind(&mut screen, 3, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    // the split orientation decides which direction leads back to pane 1; the failed attempt
    // moves nothing and so must not release either
    let moved = tab.move_focus_left(crate::ClientId::new(1)).expect("TEST")
        || tab.move_focus_up(crate::ClientId::new(1)).expect("TEST");

    assert!(moved, "one of the two directions leads to the other pane");
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "the keyboard left the popup for the pane the fullscreen switch landed on"
    );
}

/// Moving within a stack list is a focus move like any other, so a dismissible overlay above it
/// has to let go. The stack-list branch used to return before the release.
#[test]
fn moving_within_a_stack_list_releases_a_dismissible_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    *screen.stacked_pane_list.borrow_mut() = true;
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    for pane_id in [
        PaneId::Terminal(crate::TerminalId::new(2)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    ] {
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    }
    assert!(tab.has_stack_lists(), "the fixture formed a stack list");
    open_overlay_of_kind(&mut screen, 4, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    assert!(
        tab.move_focus_up(crate::ClientId::new(1)).expect("TEST"),
        "the visible member is the last in the stack, so up moves within the list"
    );
    assert!(
        !matches!(
            tab.get_active_pane_id(crate::ClientId::new(1)),
            Some(PaneId::Terminal(TERMINAL_4))
        ),
        "the keyboard left the popup for the stack member the move landed on, got {:?}",
        tab.get_active_pane_id(crate::ClientId::new(1))
    );
}

/// A cycle with nowhere to go dismisses nothing: with a single selectable pane the cycle lands
/// where it started, and releasing on the way in would let that dead keypress close the popup.
#[test]
fn cycling_focus_with_a_single_pane_leaves_a_dismissible_overlay_holding_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.focus_next_pane(crate::ClientId::new(1));
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the cycle landed where it started, so the popup still has the keyboard"
    );
    tab.focus_previous_pane(crate::ClientId::new(1));
    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "and the same going the other way"
    );
}

/// An edge move that goes nowhere dismisses nothing either.
#[test]
fn focus_pane_on_edge_that_goes_nowhere_leaves_a_dismissible_overlay_holding_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.focus_pane_on_edge(Direction::Left, crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the only pane is already on every edge, so nothing moved and the popup keeps the keyboard"
    );
}

/// And the positive half: an edge move that does land somewhere releases the popup.
#[test]
fn focus_pane_on_edge_that_moves_releases_a_dismissible_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    open_overlay_of_kind(&mut screen, 3, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    // the direction is the direction of travel - entering rightwards lands on the leftmost pane -
    // and the split orientation decides which call moves; the focus ends up on pane 1 either way,
    // and the call that moves nothing must not release
    tab.focus_pane_on_edge(Direction::Right, crate::ClientId::new(1));
    tab.focus_pane_on_edge(Direction::Down, crate::ClientId::new(1));

    assert!(
        !matches!(
            tab.get_active_pane_id(crate::ClientId::new(1)),
            Some(PaneId::Terminal(TERMINAL_3))
        ),
        "the keyboard left the popup for the pane on the edge, got {:?}",
        tab.get_active_pane_id(crate::ClientId::new(1))
    );
}

/// "Focus the last pane" with no last pane recorded moves nothing, and so dismisses nothing.
#[test]
fn focus_last_pane_with_no_previous_pane_leaves_a_dismissible_overlay_holding_the_keyboard() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "POPUP", OverlayKind::Dismissible);

    let tab = screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST");
    tab.focus_last_pane(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "with nowhere to go back to, the popup still has the keyboard"
    );
}

/// A nested guest handing focus back with a direction while a modal is up is a no-op, not a
/// hand-off to the host session. A pin rather than a regression test: `clients_with_pane_focused`
/// already answers with no clients while a modal holds the keyboard, and the handler itself now
/// refuses the hand-off too - this is the first coverage of the FocusHost direction path at all.
#[test]
fn a_guest_hand_back_under_a_modal_does_not_bubble_to_the_host() {
    use yinz_ipc::nested_session::NestedSessionMessage;

    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "MODAL", OverlayKind::Modal);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    screen.handle_nested_session_message_from_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        NestedSessionMessage::FocusHost {
            direction: Some(Direction::Left),
        },
    );

    while let Ok((instruction, _)) = server_receiver.try_recv() {
        assert!(
            !matches!(
                instruction,
                ServerInstruction::EmitNestedSessionFrameToClient(..)
            ),
            "nothing was handed to the host session"
        );
    }
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the modal still has the keyboard"
    );
}

// ---------------------------------------------------------------------------
// Dragging an overlay with the mouse.
//
// An overlay is drawn over whatever it is covering, so the user's only way to see what is
// underneath is to move it. It drags by its frame, exactly as a floating pane does - but through
// its own collection, because the floating path re-resolves the pointer position from scratch and
// would pick up the pane the overlay is drawn over.
// ---------------------------------------------------------------------------

/// Press the overlay's top-left frame cell, drag it by `(columns, lines)`, release.
fn drag_overlay_by(screen: &mut Screen, pane_id: u32, columns: i32, lines: i32) {
    let geom = floating_geom(screen, pane_id);
    let from = Position::new(geom.y as i32, geom.x as u16);
    let to = Position::new(
        geom.y as i32 + lines,
        (geom.x as i32 + columns).max(0) as u16,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(from),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(to),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(to),
        crate::ClientId::new(1),
    );
}

/// The Phase 7 bug from the other side. Pressing a modal's frame used to be routed through
/// `FloatingPanes`, which threw the pane id away and re-resolved the position - so the drag moved
/// whatever the modal was covering. Now it moves the modal, and only the modal.
#[test]
fn dragging_a_modal_moves_the_modal_and_leaves_what_it_covers_alone() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 3, "FLOATING", false);
    open_overlay_of_kind(&mut screen, 2, "THEMODAL", OverlayKind::Modal);

    let tiled_before = floating_geom(&screen, 1);
    let floating_before = floating_geom(&screen, 3);
    let modal_before = floating_geom(&screen, 2);

    drag_overlay_by(&mut screen, 2, 6, 3);

    let modal_after = floating_geom(&screen, 2);
    assert_eq!(
        (modal_after.x, modal_after.y),
        (modal_before.x + 6, modal_before.y + 3),
        "the modal follows the pointer"
    );
    let tiled_after = floating_geom(&screen, 1);
    let floating_after = floating_geom(&screen, 3);
    assert_eq!(
        (tiled_after.x, tiled_after.y),
        (tiled_before.x, tiled_before.y),
        "the tiled pane behind it must not have moved"
    );
    assert_eq!(
        (floating_after.x, floating_after.y),
        (floating_before.x, floating_before.y),
        "and neither must the floating pane behind it"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "a drag never moves the keyboard, so the modal still holds it"
    );
}

/// A press that never left its cell is a click. It has to still focus the overlay, or an overlay
/// under a live one could be pressed and nothing at all would happen.
#[test]
fn a_frame_press_that_never_moved_still_focuses_the_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_of_kind(&mut screen, 2, "LOWER", OverlayKind::Dismissible);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(40)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "pre-condition: the later overlay has the keyboard"
    );

    let geom = floating_geom(&screen, 2);
    let corner = Position::new(geom.y as i32, geom.x as u16);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(corner),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(corner),
        crate::ClientId::new(1),
    );

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the press that never moved is a click, and a click on an overlay focuses it"
    );
    let after = floating_geom(&screen, 2);
    assert_eq!((after.x, after.y), (geom.x, geom.y), "and nothing moved");
}

/// The drag rewrites the overlay's *coordinates*, not just its geometry, so where the user put it
/// is re-resolved on the next surface change the same way where it was opened would have been.
/// Without that the overlay is re-centred by the first resize and walks out from under the pointer.
#[test]
fn a_dragged_overlay_stays_where_it_was_dropped_across_a_resize() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.render_to_clients().expect("TEST");
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(30)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    drag_overlay_by(&mut screen, 2, -28, 1);
    let dropped_at = floating_geom(&screen, 2);

    screen
        .resize_to_screen(Size { cols: 90, rows: 14 })
        .expect("TEST");

    let after = floating_geom(&screen, 2);
    assert_eq!(
        (after.x, after.y),
        (dropped_at.x, dropped_at.y),
        "the overlay stays where it was dropped instead of being re-centred"
    );
}

/// Where an overlay was dropped and how large the user made it are *in-memory* state on the
/// wrapper `extract_pane` throws away, so both have to be carried across a tab move by hand -
/// exactly as suppression is, and now in the same `OverlayCarry`.
///
/// Without that, the position survives the move (it is in the coordinates, which do travel) and is
/// then overwritten by the plugin's next placement, because the destination no longer knows the
/// user had a say.
#[test]
fn a_moved_and_resized_overlay_keeps_its_geometry_across_a_tab_move() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");
    let opened_with = overlay_coordinates();
    open_overlay_with(&mut screen, 3, opened_with);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .expect("TEST");
    drag_overlay_by(&mut screen, 3, 20, 4);
    let before = floating_geom(&screen, 3);

    screen
        .break_multiple_panes_to_tab_with_id(
            vec![PaneId::Terminal(crate::TerminalId::new(3))],
            TabId::new(1),
            false,
            crate::ClientId::new(1),
        )
        .expect("TEST");

    let after = screen.tabs[&TabId::new(1)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(3)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (
            after.x,
            after.y,
            after.cols.as_usize(),
            after.rows.as_usize(),
        ),
        (
            before.x,
            before.y,
            before.cols.as_usize(),
            before.rows.as_usize(),
        ),
        "the move carries the exact position and size without a plugin restatement"
    );
}

#[test]
fn replayed_overlay_geometry_preserves_the_user_placement_override() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 3, overlay_coordinates());
    drag_overlay_by(&mut screen, 3, 20, 4);
    let dropped_at = floating_geom(&screen, 3);
    for override_user_placement in [false, true] {
        super::super::replay_pending_plugin_events(
            &mut screen,
            vec![ScreenInstruction::ChangeOverlayCoordinates(
                vec![(PaneId::Terminal(TERMINAL_3), overlay_coordinates())],
                override_user_placement,
                None,
            )],
        )
        .unwrap();
        let placed_at = floating_geom(&screen, 3);
        let expected = if override_user_placement {
            (2, 2)
        } else {
            (dropped_at.x, dropped_at.y)
        };
        assert_eq!((placed_at.x, placed_at.y), expected);
    }
}

/// A plugin relaying a move key outranks a drag, but not the independent size the user chose.
#[test]
fn a_dragged_and_resized_overlay_is_moved_without_being_resized() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 3, overlay_coordinates());
    drag_overlay_by(&mut screen, 3, 20, 4);
    let dropped_at = floating_geom(&screen, 3);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .resize(
            crate::ClientId::new(1),
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .expect("TEST");
    let resized_to = floating_geom(&screen, 3);

    let asked_for = FloatingPaneCoordinates::new(
        Some("10".to_owned()),
        Some("2".to_owned()),
        Some("30".to_owned()),
        Some("8".to_owned()),
        None,
        None,
    )
    .expect("TEST");
    screen.change_overlay_coordinates(
        vec![(PaneId::Terminal(crate::TerminalId::new(3)), asked_for)],
        true,
    );

    let after = floating_geom(&screen, 3);
    assert_ne!((after.x, after.y), (dropped_at.x, dropped_at.y));
    assert_eq!(
        (after.x, after.y),
        (10, 2),
        "exactly the position the key asked for"
    );
    assert_eq!(
        (after.cols.as_usize(), after.rows.as_usize()),
        (resized_to.cols.as_usize(), resized_to.rows.as_usize()),
        "and exactly the size it had before the move"
    );
}

/// The same carry on the reveal path, which is the one the `a` and `c` keybindings take.
///
/// `LaunchOrFocusPlugin { move_to_focused_tab }` overrides exactly one bit of what it carries -
/// naming a plugin is the user asking for it back, so a hidden overlay arrives shown. It must not
/// quietly override the other: a reveal is not a request to undo the drag, and the plugin
/// re-placing itself on arrival is what would do that.
#[test]
fn a_dragged_overlay_revealed_onto_another_tab_arrives_shown_and_still_dragged() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");
    let coordinates = popup_coordinates();
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(0),
        coordinates.clone(),
    );

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();
    let from = Position::new(geom.y as i32, geom.x as u16);
    let to = Position::new(geom.y as i32 + 5, geom.x as u16 + 12);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(from),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(to),
        crate::ClientId::new(1),
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(to),
        crate::ClientId::new(1),
    );
    let dropped_at = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();

    // Esc puts the column away, and the user asks for it back from the other tab
    assert!(screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .hide_overlay_pane(popup_id));
    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(1))
        .expect("TEST");
    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            false,
            true,
            false,
            crate::ClientId::new(1),
            &mut None
        )
        .expect("TEST"));

    let second_tab = screen.tabs.get(&TabId::new(1)).expect("TEST");
    assert!(
        second_tab.overlays_are_rendered(),
        "the reveal half of the move still happens - a pane asked for by name comes back shown"
    );
    // the plugin restating its placement, as agent does the instant it hears where it landed
    screen.change_overlay_coordinates(vec![(popup_id, coordinates)], false);
    let after = screen.tabs[&TabId::new(1)]
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (after.x, after.y),
        (dropped_at.x, dropped_at.y),
        "and the drag came with it"
    );
}

// ---------------------------------------------------------------------------
// Per-pane suppression: an overlay set aside without being unloaded.
//
// agent draws itself as an overlay column in the bottom-left corner, and Esc has to put it away
// while keeping the conversation, the parked chats and anything still streaming. That is what
// `SuppressPane` on an overlay now means. These pin the instruction routing; the collection's own
// rules are pinned in `overlay_panes/unit/overlay_panes_tests.rs`.
// ---------------------------------------------------------------------------

/// `SuppressPane` on an overlay hides it in place rather than extracting it.
///
/// The extraction is the thing to avoid: `Tab::suppress_pane` moves a pane into `suppressed_panes`,
/// which drops the kind and coordinates that make it an overlay - the same loss
/// `placement_for_extraction` exists to prevent on a cross-tab move. Before this arm existed the
/// instruction fell through to the tab loop and did exactly that.
#[test]
fn suppressing_an_overlay_hides_it_in_place() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);
    let before = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();

    assert!(
        screen.tabs[&TabId::new(0)].pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2)))
    );
    screen
        .tab_containing_overlay_mut(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .hide_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert!(
        screen.tabs[&TabId::new(0)].pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))),
        "still an overlay, and still loaded - hiding is not closing"
    );
    assert!(
        !screen.tabs[&TabId::new(0)].overlays_are_rendered(),
        "but nothing is drawn for it"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("OVERLAY"),
        "and the cells it vacated are repainted, got:\n{frame}"
    );

    screen
        .tab_containing_overlay_mut(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .show_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    let after = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (
            before.x,
            before.y,
            before.cols.as_usize(),
            before.rows.as_usize()
        ),
        (
            after.x,
            after.y,
            after.cols.as_usize(),
            after.rows.as_usize()
        ),
        "and it comes back exactly where it was"
    );
}

#[test]
fn overlay_permission_prompts_restore_visibility_and_preserve_placement() {
    use yinz_types::data::{PermissionStatus, PermissionType, PluginPermission};

    for kind in [OverlayKind::Dismissible, OverlayKind::Modal] {
        for hidden in [false, true] {
            for awaiting_placement in [false, true] {
                let size = Size {
                    cols: 100,
                    rows: 30,
                };
                let mut screen = create_non_mirrored_screen(size);
                let (plugin_tx, plugin_rx) = channels::unbounded();
                let plugin_sender = SenderWithContext::new(plugin_tx);
                screen.bus.senders.to_plugin = Some(plugin_sender.clone());
                new_tab(&mut screen, 1, TabId::new(0));
                let pid = PluginId::new(9);
                let pane_id = PaneId::Plugin(pid);
                let pane = PaneFactory::plugin(
                    pid,
                    Default::default(),
                    plugin_sender,
                    "overlay".to_owned(),
                    "overlay".to_owned(),
                    screen.sixel_image_store.clone(),
                    screen.kitty_image_store.clone(),
                    screen.terminal_emulator_colors.clone(),
                    screen.terminal_emulator_color_codes.clone(),
                    Rc::new(RefCell::new(LinkHandler::new())),
                    screen.character_cell_size.clone(),
                    screen.connected_clients.borrow().iter().copied().collect(),
                    screen.style,
                    None,
                    screen.debug,
                    screen.arrow_fonts,
                    screen.styled_underlines,
                );
                let coordinates = FloatingPaneCoordinates {
                    height: Some(PercentOrFixed::Fixed(12)),
                    ..overlay_coordinates()
                };
                let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
                tab.add_overlay_pane(
                    pane,
                    pane_id,
                    yinz_types::data::OverlayPlacement::new(kind, Some(coordinates)),
                    OverlayCarry {
                        hidden,
                        awaiting_placement,
                        ..Default::default()
                    },
                    true,
                    Some(CLIENT_1),
                )
                .unwrap();
                let placement = tab.overlay_placement_for(pane_id).unwrap();
                let geometry = tab.get_pane_with_id(pane_id).unwrap().position_and_size();
                let permissions = vec![PermissionType::ReadApplicationState];
                let focus = tab.get_active_pane_id(CLIENT_1);
                for granted in [true, false] {
                    for _ in 0..2 {
                        screen.open_permission_modal(
                            pid,
                            PluginPermission::new("overlay".into(), permissions.clone()),
                        );
                    }
                    let frame = rendered_frame(&mut screen, size);
                    assert!(frame.contains("Plugin permissions"), "{frame}");
                    if granted {
                        screen.permission_modal_input(
                            &Some(yinz_types::keys::KeyWithModifier::new(
                                yinz_types::keys::BareKey::Tab,
                            )),
                            CLIENT_1,
                        );
                    }
                    screen.permission_modal_input(
                        &Some(yinz_types::keys::KeyWithModifier::new(
                            yinz_types::keys::BareKey::Enter,
                        )),
                        CLIENT_1,
                    );
                    let expected = if granted {
                        PermissionStatus::Granted
                    } else {
                        PermissionStatus::Denied
                    };
                    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::PermissionAnswer { status, .. } if status == expected)));
                    let tab = screen.tabs.get(&TabId::new(0)).unwrap();
                    assert_eq!(tab.get_active_pane_id(CLIENT_1), focus);
                    assert_eq!(tab.overlay_carry_for(pane_id).hidden, hidden);
                    assert_eq!(
                        tab.overlay_carry_for(pane_id).awaiting_placement,
                        awaiting_placement
                    );
                    assert_eq!(tab.overlay_placement_for(pane_id).unwrap(), placement);
                    assert_eq!(
                        tab.get_pane_with_id(pane_id).unwrap().position_and_size(),
                        geometry
                    );
                    let frame = rendered_frame(&mut screen, size);
                    assert!(!frame.contains("Plugin permissions"), "{frame}");
                }
            }
        }
    }
}

/// A hidden overlay is re-resolved while hidden, so it comes back sized for the screen as it is
/// now rather than as it was when it went away.
#[test]
fn a_hidden_overlay_survives_a_resize_and_returns_proportional() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .hide_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    let bigger = Size {
        cols: 160,
        rows: 40,
    };
    screen.resize_to_screen(bigger).expect("TEST");
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .show_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    let geom = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        geom.cols.as_usize(),
        96,
        "60% of the surface as it is now, not the 48 it had when it was hidden"
    );
}

/// A tab whose only overlay is hidden must behave exactly like a tab with no overlay at all -
/// otherwise `get_active_pane_id` keeps preferring the overlay collection and the occlusion stack
/// keeps a layer for a pane nothing draws.
#[test]
fn a_tab_holding_only_a_hidden_overlay_has_no_overlays_to_render() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);
    assert!(screen.tabs[&TabId::new(0)].overlays_are_rendered());

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .hide_overlay_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    assert!(!screen.tabs[&TabId::new(0)].overlays_are_rendered());
    assert_ne!(
        screen.tabs[&TabId::new(0)].get_active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "and the keyboard is not left on a pane nobody can see"
    );
}

/// An overlay in a tab nobody is looking at still hears broadcast events.
///
/// This is what lets agent follow tab switches at all: it moves itself when it sees the
/// `TabUpdate`, and the overlay it moves is by definition sitting in the tab the user just left.
/// The active-tab walk in `targeted_plugin_ids` would never reach it.
#[test]
fn an_overlay_in_a_background_tab_is_still_a_broadcast_target() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let overlay = PaneFactory::plugin(
        crate::PluginId::new(9),
        Default::default(),
        plugin_sender,
        "overlay".to_owned(),
        "overlay".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().iter().copied().collect(),
        screen.style,
        None,
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .add_overlay_pane(
            overlay,
            PaneId::Plugin(crate::PluginId::new(9)),
            yinz_types::data::OverlayPlacement::dismissible(Some(
                FloatingPaneCoordinates::default(),
            )),
            OverlayCarry::default(),
            true,
            Some(crate::ClientId::new(1)),
        )
        .expect("TEST");

    // client 1 is looking at tab 1, so the overlay in tab 0 is in nobody's active tab
    screen
        .go_to_tab(TabNumber::new(2), crate::ClientId::new(1))
        .expect("TEST");
    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)),
        Some(&TabId::new(1))
    );

    let targets = screen.targeted_plugin_ids(crate::ClientId::new(1), EventType::TabUpdate);
    assert!(
        targets.contains(&PluginId::new(9)),
        "an overlay left behind on another tab must still hear the event it moves on, got {targets:?}"
    );
    let targets = screen.targeted_plugin_ids(crate::ClientId::new(1), EventType::PaneUpdate);
    assert!(
        targets.contains(&PluginId::new(9)),
        "and the other state event, which is how it learns it was hidden or revealed, got {targets:?}"
    );
    // ...but only those two. An `InputReceived` is one client's keystroke on one tab, and an
    // overlay sitting somewhere else would answer it by clearing UI the user never typed at.
    let targets = screen.targeted_plugin_ids(crate::ClientId::new(1), EventType::InputReceived);
    assert!(
        !targets.contains(&PluginId::new(9)),
        "input performed on another tab is not this overlay's business, got {targets:?}"
    );
}

/// Why `SuppressPane`'s overlay arm has to come *before* the tab loop.
///
/// An overlay is matched by `has_non_suppressed_pane_with_pid` like any other pane, so without an
/// arm of its own the instruction reaches `Tab::suppress_pane`, which extracts the pane into
/// `suppressed_panes` and loses the kind and coordinates that make it an overlay. This pins the
/// precondition rather than the arm: if this assertion ever goes false the arm is no longer load
/// bearing, and if it stays true the arm must stay ahead of the loop.
#[test]
fn an_overlay_would_be_caught_by_the_tab_suppression_loop() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_floating_pane(&mut screen, TabId::new(0), 2, "OVERLAY", true);

    assert!(
        screen.tabs[&TabId::new(0)]
            .has_non_suppressed_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(2))),
        "the tab loop would claim this overlay, so the overlay arm must be reached first"
    );
    assert!(
        screen
            .tab_containing_overlay(PaneId::Terminal(crate::TerminalId::new(2)))
            .is_some(),
        "and the overlay arm's own lookup finds it"
    );
}

/// A plugin's `show_pane_with_id(.., should_focus_pane = false)` arrives as
/// `ScreenInstruction::UnsuppressOrExpandPane`, whose overlay arm is `Tab::show_overlay_pane` -
/// the "put it back on screen, don't move the keyboard" path agent uses to restore its agent list.
///
/// For a *modal* that separation does not exist. The modal lock is re-armed by this very
/// statement, so a modal restored without its keyboard is a dialog nobody can type into that
/// nothing else may be focused past. The plugin-facing bool decides whether an ordinary overlay
/// takes focus; it does not get to decide whether a modal is a modal.
#[test]
fn unsuppressing_a_modal_overlay_gives_it_the_keyboard_back() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(7));
    assert_eq!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id)
    );

    screen
        .tab_containing_overlay_mut(modal_id)
        .expect("TEST")
        .hide_overlay_pane(modal_id);
    assert_ne!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id),
        "a modal that is not drawn does not hold the keyboard either"
    );

    screen
        .tab_containing_overlay_mut(modal_id)
        .expect("TEST")
        .show_overlay_pane(modal_id);
    assert_eq!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .get_active_pane_id(crate::ClientId::new(1)),
        Some(modal_id),
        "and one that is drawn again does"
    );
}

/// `Ctrl e c` on a agent that was put away with Esc. `LaunchOrFocusPlugin` is the user asking for
/// the pane back, so it has to come back *and* take the keyboard - a pane that reappears while
/// typing still goes to whatever is underneath is worse than one that never reappeared.
#[test]
fn launch_or_focus_plugin_focuses_a_hidden_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(2),
        TabId::new(0),
        popup_coordinates(),
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id)
    );

    // Esc in agent: `hide_self` -> `ScreenInstruction::SuppressPane` -> `hide_overlay_pane`
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hide_overlay_pane(popup_id);
    assert_ne!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id),
        "a hidden overlay does not hold the keyboard"
    );

    let mut completion = None;
    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            true,
            true,
            false,
            crate::ClientId::new(1),
            &mut completion
        )
        .expect("TEST"));
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id),
        "the revealed overlay takes the keyboard"
    );
}

/// Three tabs with the first closed, so ids and positions no longer agree: tab id 1 now sits at
/// position 0 and tab id 2 at position 1. Every session looks like this after one tab is closed -
/// `close_tab_by_id` renumbers positions and leaves ids alone - and `MoveTab` does the same by
/// swapping positions under fixed ids.
///
/// The client is left on tab id 1, which is the arrangement that tells a position lookup apart
/// from an id one: reading id 1 as a position lands on tab id 2, a real other tab, so the mistake
/// shows up as a visible switch rather than as a lookup that finds nothing and quietly does
/// nothing.
fn screen_with_diverging_tab_ids(size: Size) -> Screen {
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    screen.close_tab_by_id(TabId::new(0)).expect("TEST");
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");
    assert_eq!(
        screen.get_tab_position_by_id(TabId::new(1)),
        Some(TabPosition::new(0)),
        "TEST"
    );
    assert_eq!(
        screen.get_tab_position_by_id(TabId::new(2)),
        Some(TabPosition::new(1)),
        "TEST"
    );
    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)).copied(),
        Some(TabId::new(1)),
        "TEST"
    );
    screen
}

/// `Ctrl e c` on a agent that is already in the tab the user is standing in - which is where it
/// almost always is, since every shipped `LaunchOrFocusPlugin` binding carries
/// `move_to_focused_tab` and so keeps the pane with the client.
///
/// A reveal must not move the client. The tab holding the plugin is found by `self.tabs` key,
/// which is a stable *id*, while `go_to_tab` names a tab by its *display position*; the two agree
/// only until a tab is closed or moved. Reading the id as a position walked the client to
/// whichever tab now sits at that number and then unhid agent on the one it had just left - a tab
/// switch, and no agent.
#[test]
fn revealing_a_plugin_overlay_in_the_focused_tab_does_not_switch_tabs() {
    let mut screen = screen_with_diverging_tab_ids(Size { cols: 80, rows: 12 });
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(1),
        popup_coordinates(),
    );
    // Esc in agent
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .hide_overlay_pane(popup_id);

    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            true,
            true,
            false,
            crate::ClientId::new(1),
            &mut None
        )
        .expect("TEST"));

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)).copied(),
        Some(TabId::new(1)),
        "a reveal leaves the client on the tab it is looking at - tab id 1 is at position 0, and \
         reading its id as a position walks the client over to tab id 2"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id),
        "and the overlay that was asked for is the pane holding the keyboard, in that tab"
    );
}

/// The other half of the same arm: without `move_to_focused_tab` the client goes to the plugin
/// rather than the plugin coming to the client. That is still a tab named by position, and the
/// plugin's tab id here runs past the end of the tab bar - the case where the old code switched to
/// no tab at all and focused the pane in the background.
#[test]
fn focusing_a_plugin_without_moving_it_goes_to_the_tab_that_holds_it() {
    let mut screen = screen_with_diverging_tab_ids(Size { cols: 80, rows: 12 });
    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(1))
        .expect("TEST"); // tab id 2
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(2),
        popup_coordinates(),
    );
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST"); // back to tab id 1

    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            false,
            false,
            false,
            crate::ClientId::new(1),
            &mut None
        )
        .expect("TEST"));

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)).copied(),
        Some(TabId::new(2)),
        "the client is taken to the tab that holds the plugin, named by position - id 2 is past \
         the end of the tab bar and used to resolve to nothing at all"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id)
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "and nothing moved - the pane is where it was"
    );
}

/// The move arm, which compares ids and is right to. It is the one that would break if the id and
/// position confusion below it were "fixed" by turning this comparison into a positional one, so
/// it is pinned with ids and positions disagreeing.
#[test]
fn a_plugin_overlay_still_moves_to_the_focused_tab_when_ids_and_positions_disagree() {
    let mut screen = screen_with_diverging_tab_ids(Size { cols: 80, rows: 12 });
    screen
        .switch_active_tab(TabPosition::new(1), None, true, crate::ClientId::new(1))
        .expect("TEST"); // tab id 2
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(2),
        popup_coordinates(),
    );
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST"); // back to tab id 1

    assert!(screen
        .focus_plugin_pane(
            &run_plugin,
            true,
            true,
            false,
            crate::ClientId::new(1),
            &mut None
        )
        .expect("TEST"));

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(1)).copied(),
        Some(TabId::new(1)),
        "the pane comes to the client, so the client does not go anywhere"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(1))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "and it arrives as an overlay on the tab the client is on"
    );
    assert!(
        !screen
            .tabs
            .get(&TabId::new(2))
            .expect("TEST")
            .has_pane_with_pid(&popup_id),
        "leaving nothing behind on the tab it came from"
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id)
    );
}

/// A client with no tab of its own has nowhere for a `move_to_focused_tab` to move a pane *to*.
/// The extraction happens before the destination is looked up, so taking the pane out for such a
/// client would drop it on the way out of the function - a plugin pane gone with no `PaneClosed`
/// and no `Unload`. Nothing in production reaches this (`resolve_tab_and_client` only hands over a
/// client that has an active tab), which is exactly why the guard needs a test.
#[test]
fn a_plugin_pane_is_not_extracted_for_a_client_with_no_tab() {
    let mut screen = screen_with_diverging_tab_ids(Size { cols: 80, rows: 12 });
    let (run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(2),
        popup_coordinates(),
    );
    screen.active_tab_ids.remove(&ClientId::new(1));

    let found = screen
        .focus_plugin_pane(
            &run_plugin,
            true,
            true,
            false,
            crate::ClientId::new(1),
            &mut None,
        )
        .expect("TEST");

    assert!(found, "the pane is still found - it is only not moved");
    assert!(
        screen
            .tabs
            .get(&TabId::new(2))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "and it is still in the tab it started in, rather than dropped in transit"
    );
}

/// `Ctrl e c` with no agent running. `focus_plugin_pane` finds nothing, so `LaunchOrFocusPlugin`
/// creates the pane instead - the `AddPlugin` path, which is what this mirrors: `should_focus`
/// defaults to true and the placement is an overlay.
#[test]
fn a_newly_created_overlay_plugin_takes_the_keyboard() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let popup_id = PaneId::Plugin(crate::PluginId::new(2));
    let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:/path/to/fake/popup",
        &None,
        None,
        None,
    )
    .unwrap();
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            popup_id,
            Some("popup".to_owned()),
            Some(Run::Plugin(run_plugin)),
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(popup_coordinates()))),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");

    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(popup_id)
    );
}

/// The arrangement agent actually runs: two overlay plugins side by side, the conversation and
/// the agent list. Esc in the conversation hands the keyboard to the list (`move_clients_out_of_pane`
/// walks focus down the stack rather than off it), so `Ctrl e c` has to take it back off a
/// *sibling overlay* rather than off a tiled pane - a different arm of `focus_pane_with_id`.
#[test]
fn launch_or_focus_plugin_takes_the_keyboard_back_from_a_sibling_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));

    let coordinates = |x: usize| FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(x)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Fixed(20)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    };
    let (_, agents_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(3),
        TabId::new(0),
        coordinates(24),
    );
    let (chat_run, chat_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(2),
        TabId::new(0),
        coordinates(2),
    );
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(chat_id)
    );

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hide_overlay_pane(chat_id);
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(agents_id),
        "hiding the conversation drops the client onto the list beside it"
    );

    let mut completion = None;
    assert!(screen
        .focus_plugin_pane(
            &chat_run,
            true,
            true,
            false,
            crate::ClientId::new(1),
            &mut completion
        )
        .expect("TEST"));
    assert_eq!(
        screen.active_pane_id(crate::ClientId::new(1)),
        Some(chat_id),
        "the revealed conversation takes the keyboard back from the list"
    );
}

/// Summoning either half must carry the link before any plugin can reconcile its placement.
#[test]
fn summoning_a_linked_overlay_moves_and_reveals_the_whole_panel() {
    let client_id = crate::ClientId::new(1);
    let geom = |screen: &Screen, tab_id, pane_id| {
        screen.tabs[&tab_id]
            .get_pane_with_id(pane_id)
            .expect("TEST")
            .position_and_size()
    };
    let drag = |screen: &mut Screen, pane_id, tab_id| {
        let before = screen.tabs[&tab_id].multipane_info(pane_id).unwrap();
        let from = Position::new(before.y as i32, (before.x + 5) as u16);
        let to = Position::new(before.y as i32 + 2, (before.x + 8) as u16);
        screen.handle_mouse_event(MouseEvent::new_left_press_event(from), client_id);
        screen.handle_mouse_event(MouseEvent::new_left_motion_event(to), client_id);
        screen.handle_mouse_event(MouseEvent::new_left_release_event(to), client_id);
    };
    for requested in 0..2 {
        for hidden in [[false, false], [true, false], [false, true], [true, true]] {
            for previously_placed in [false, true] {
                let mut screen = create_non_mirrored_screen(Size {
                    cols: 120,
                    rows: 40,
                });
                new_tab(&mut screen, 1, TabId::new(0));
                let coordinates = |x| FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(x)),
                    y: Some(PercentOrFixed::Fixed(5)),
                    width: Some(PercentOrFixed::Fixed(25)),
                    height: Some(PercentOrFixed::Fixed(12)),
                    ..Default::default()
                };
                let pair = [
                    open_overlay_plugin(
                        &mut screen,
                        PluginId::new(7),
                        TabId::new(0),
                        coordinates(5),
                    ),
                    open_overlay_plugin(
                        &mut screen,
                        PluginId::new(8),
                        TabId::new(0),
                        coordinates(30),
                    ),
                ];
                screen
                    .tabs
                    .get_mut(&TabId::new(0))
                    .unwrap()
                    .create_multipane(
                        [pair[0].1, pair[1].1],
                        yinz_types::multipane::MultipaneSplit {
                            direction: yinz_types::multipane::MultipaneDirection::LeftRight,
                            ratio: 5000,
                        },
                        0,
                        OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                            width: Some(PercentOrFixed::Fixed(50)),
                            ..coordinates(5)
                        })),
                    )
                    .unwrap();
                if previously_placed {
                    let before = geom(&screen, TabId::new(0), pair[1].1);
                    screen
                        .tabs
                        .get_mut(&TabId::new(0))
                        .unwrap()
                        .resize(
                            client_id,
                            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
                        )
                        .expect("TEST");
                    assert!(
                        geom(&screen, TabId::new(0), pair[1].1).cols.as_usize()
                            > before.cols.as_usize()
                    );
                    let before_drag = geom(&screen, TabId::new(0), pair[1].1);
                    drag(&mut screen, pair[0].1, TabId::new(0));
                    assert_eq!(geom(&screen, TabId::new(0), pair[1].1).x, before_drag.x + 3);
                }
                let before = pair
                    .each_ref()
                    .map(|(_, id)| geom(&screen, TabId::new(0), *id));
                for (index, (_, id)) in pair.iter().enumerate() {
                    if hidden[index] {
                        screen
                            .tabs
                            .get_mut(&TabId::new(0))
                            .unwrap()
                            .hide_overlay_pane(*id);
                    }
                }
                let (_, unrelated) = open_overlay_plugin(
                    &mut screen,
                    PluginId::new(9),
                    TabId::new(0),
                    coordinates(85),
                );
                let unrelated_before = geom(&screen, TabId::new(0), unrelated);
                new_tab(&mut screen, 2, TabId::new(1));
                let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
                let mut completion = Some(crate::NotificationEnd::new(completion_tx));
                assert!(screen
                    .focus_plugin_pane(
                        &pair[requested].0,
                        true,
                        true,
                        false,
                        client_id,
                        &mut completion,
                    )
                    .expect("TEST"));
                drop(completion);
                assert_eq!(
                    completion_rx.try_recv().expect("TEST").affected_pane_id,
                    Some(pair[requested].1)
                );
                assert_eq!(screen.active_tab_ids[&client_id], TabId::new(1));
                assert_eq!(screen.active_pane_id(client_id), Some(pair[requested].1));
                for (index, (_, id)) in pair.iter().enumerate() {
                    assert!(
                        !screen.tabs[&TabId::new(0)].has_pane_with_pid(id),
                        "both halves must leave"
                    );
                    assert!(
                        screen.tabs[&TabId::new(1)].pane_is_overlay(*id),
                        "both halves must arrive"
                    );
                    assert!(
                        !screen.tabs[&TabId::new(1)].overlay_carry_for(*id).hidden,
                        "both halves are revealed"
                    );
                    let after = geom(&screen, TabId::new(1), *id);
                    assert_eq!(
                        (
                            after.x,
                            after.y,
                            after.cols.as_usize(),
                            after.rows.as_usize()
                        ),
                        (
                            before[index].x,
                            before[index].y,
                            before[index].cols.as_usize(),
                            before[index].rows.as_usize()
                        ),
                    );
                }
                for (_, dragged) in &pair {
                    let before = pair
                        .each_ref()
                        .map(|(_, id)| geom(&screen, TabId::new(1), *id));
                    drag(&mut screen, *dragged, TabId::new(1));
                    for (index, (_, id)) in pair.iter().enumerate() {
                        let after = geom(&screen, TabId::new(1), *id);
                        assert_eq!(
                            (after.x, after.y),
                            (before[index].x + 3, before[index].y + 2)
                        );
                        assert_eq!(
                            (after.cols, after.rows),
                            (before[index].cols, before[index].rows)
                        );
                    }
                }
                assert_eq!(geom(&screen, TabId::new(0), unrelated), unrelated_before);
                assert!(!screen.tabs[&TabId::new(1)].has_pane_with_pid(&unrelated));
            }
        }
    }
}

/// Collect every pane the pty thread was told to tear down, from either instruction that does it.
fn panes_torn_down_by_pty(
    receiver: &channels::Receiver<(PtyInstruction, ErrorContext)>,
) -> Vec<PaneId> {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .flat_map(|(instruction, _)| match instruction {
            PtyInstruction::CloseTab(pane_ids) => pane_ids,
            PtyInstruction::ClosePane(pane_id, _) => vec![pane_id],
            _ => vec![],
        })
        .collect()
}

/// Collect every pane the plugins were told had closed.
fn panes_reported_closed_to_plugins(
    receiver: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<yinz_types::data::PaneId> {
    std::iter::from_fn(|| receiver.try_recv().ok())
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .filter_map(|(_, _, event)| match event {
                    Event::PaneClosed(pane_id) => Some(pane_id),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect()
}

/// Two tabs, the overlay and the client on the second one.
fn screen_with_an_overlay_on_the_second_tab(size: Size) -> (Screen, PaneId) {
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let (_run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(1),
        popup_coordinates(),
    );
    (screen, popup_id)
}

/// An overlay belongs to a `Tab` but is drawn over the whole session surface, and agent keeps a
/// session's worth of conversation in one. Closing the tab it happens to be sitting on used to
/// take it with the tab - `get_all_pane_ids` reports overlays, so the overlay was named in the
/// `PtyInstruction::CloseTab` that ends in `PluginInstruction::Unload`.
#[test]
fn a_dismissible_overlay_survives_its_tab_closing() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, popup_id) = screen_with_an_overlay_on_the_second_tab(size);
    assert!(screen
        .tabs
        .get(&TabId::new(1))
        .expect("TEST")
        .pane_is_overlay(popup_id));

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    assert_eq!(screen.tabs.len(), 1, "the tab itself is gone");
    assert!(
        screen
            .tabs
            .get(&TabId::new(0))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "the overlay must move to the surviving tab, and must still be an overlay there"
    );
}

/// The load-bearing half of the fix. Surviving the close means more than appearing in another
/// tab's collection: neither the pty thread nor the plugins may be told the pane closed, because
/// the pty thread answers a plugin pane with `PluginInstruction::Unload` and there is no
/// conversation left after that.
#[test]
fn an_overlay_that_outlives_its_tab_is_not_unloaded() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, popup_id) = screen_with_an_overlay_on_the_second_tab(size);

    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    let (plugin_sender, plugin_receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    let torn_down = panes_torn_down_by_pty(&pty_receiver);
    assert!(
        torn_down.contains(&PaneId::Terminal(crate::TerminalId::new(2))),
        "the tab's own pane is still torn down: {torn_down:?}"
    );
    assert!(
        !torn_down.contains(&popup_id),
        "the overlay must never reach the pty thread, which would unload its plugin: {torn_down:?}"
    );

    let reported_closed = panes_reported_closed_to_plugins(&plugin_receiver);
    assert!(
        !reported_closed.contains(&popup_id),
        "a pane that moved tabs did not close, and plugins must not be told it did: {reported_closed:?}"
    );
}

/// Where they land is where the user lands. The client's own destination is its most recently
/// visited tab, which is not the lowest surviving tab id - the rule suppressed panes use - so the
/// two answers can be told apart.
#[test]
fn an_overlay_follows_its_tab_s_client_when_the_tab_closes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    let (_run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(2),
        popup_coordinates(),
    );

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    let client_landed_on = *screen.active_tab_ids.get(&ClientId::new(1)).expect("TEST");
    assert_eq!(
        client_landed_on,
        TabId::new(1),
        "the client goes to the tab it came from, not to the lowest id"
    );
    assert!(
        screen
            .tabs
            .get(&client_landed_on)
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "the overlay follows the client rather than landing on some other surviving tab"
    );
    assert!(
        !screen
            .tabs
            .get(&TabId::new(0))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "and specifically not on the lowest-id tab"
    );
}

/// A background tab closed by `CloseTabWithIndex` or a cli action has no client to follow, so the
/// overlay falls back to the lowest surviving tab id - what
/// `move_suppressed_panes_from_closed_tab` does in the same situation.
#[test]
fn an_overlay_on_a_tab_nobody_is_on_falls_back_to_the_first_tab() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    let (_run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(2),
        popup_coordinates(),
    );
    // the client leaves the overlay behind on a tab of its own
    screen
        .switch_active_tab(TabPosition::new(0), None, true, crate::ClientId::new(1))
        .expect("TEST");

    screen.close_tab_by_id(TabId::new(2)).expect("TEST");

    assert!(
        screen
            .tabs
            .get(&TabId::new(0))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "with no client to follow, the overlay lands on the lowest surviving tab id"
    );
}

/// agent and its agent list are two panes drawn as one panel. They are carried between tabs
/// together everywhere else, and a tab closing must not be the one thing that parts them.
#[test]
fn both_halves_of_a_panel_land_on_the_same_tab_when_their_tab_closes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let (_, conversation_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(1),
        popup_coordinates(),
    );
    let (_, agents_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(8),
        TabId::new(1),
        popup_coordinates(),
    );

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    let surviving_tab = screen.tabs.get(&TabId::new(0)).expect("TEST");
    assert!(
        surviving_tab.pane_is_overlay(conversation_id) && surviving_tab.pane_is_overlay(agents_id),
        "both halves move, and to the same tab"
    );
}

/// A modal is a dialog *about* the tab that is going away, and one re-homed unfocused is an input
/// deadlock: `may_be_focused` refuses every other overlay the keyboard while a modal is in the
/// stack, and a modal has no channel to learn it lost focus. So it closes with its tab while the
/// dismissible overlay beside it does not.
#[test]
fn a_modal_overlay_closes_with_its_tab_while_a_dismissible_one_survives() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let (_, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(1),
        popup_coordinates(),
    );
    let modal_id =
        add_modal_plugin_overlay(&mut screen, yinz_types::internal_ids::PluginId::new(8));

    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    let surviving_tab = screen.tabs.get(&TabId::new(0)).expect("TEST");
    assert!(
        surviving_tab.pane_is_overlay(popup_id),
        "the dismissible overlay survives"
    );
    assert!(
        !surviving_tab.pane_is_overlay(modal_id),
        "the modal must not be re-homed"
    );
    let torn_down = panes_torn_down_by_pty(&pty_receiver);
    assert!(
        torn_down.contains(&modal_id),
        "the modal is torn down with its tab: {torn_down:?}"
    );
}

/// An overlay the user set aside is still an overlay, and still holds its plugin. Moving it must
/// not bring it back on screen - `OverlayCarry` is what carries "set aside" across the move, along
/// with whether the user has stated its position.
#[test]
fn an_overlay_set_aside_stays_set_aside_when_its_tab_closes() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, popup_id) = screen_with_an_overlay_on_the_second_tab(size);
    assert!(screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .hide_overlay_pane(popup_id));

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    let surviving_tab = screen.tabs.get(&TabId::new(0)).expect("TEST");
    assert!(surviving_tab.pane_is_overlay(popup_id));
    assert!(
        surviving_tab.overlay_carry_for(popup_id).hidden,
        "a hidden overlay must arrive hidden, not revealed by the move"
    );
}

/// The last tab closing ends the session, and an overlay is not a reason to keep it open - there
/// is nowhere to re-home one to.
#[test]
fn an_overlay_does_not_keep_the_session_alive_when_the_last_tab_closes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let (_run_plugin, popup_id) = open_overlay_plugin(
        &mut screen,
        yinz_types::internal_ids::PluginId::new(7),
        TabId::new(0),
        popup_coordinates(),
    );
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));

    screen.close_tab(crate::ClientId::new(1)).expect("TEST");

    assert!(screen.tabs.is_empty(), "no tab is left");
    assert!(
        std::iter::from_fn(|| server_receiver.try_recv().ok())
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))),
        "expected ServerInstruction::Render(None) to exit the session"
    );
    let torn_down = panes_torn_down_by_pty(&pty_receiver);
    assert!(
        torn_down.contains(&popup_id),
        "with nowhere to go the overlay is torn down like everything else: {torn_down:?}"
    );
}

/// An overlay follows the client to a surviving tab when its last terminal leaves.
#[test]
fn an_overlay_moves_to_a_surviving_tab_when_the_last_terminal_leaves() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, popup_id) = screen_with_an_overlay_on_the_second_tab(size);
    let (server_sender, _server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    // the user exits the shell in the tab's only pane
    assert!(screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .extract_pane(PaneId::Terminal(crate::TerminalId::new(2)), true)
        .is_some());
    screen.settle_tabs().expect("TEST");

    assert!(
        !screen.tabs.contains_key(&TabId::new(1)),
        "an overlay alone does not keep its source tab alive"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(0))
            .expect("TEST")
            .pane_is_overlay(popup_id),
        "the overlay moves to the surviving tab"
    );
}

/// Overlays alone do not keep the session alive after all its terminals exit.
#[test]
fn settling_closes_every_tab_when_only_overlays_remain() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, popup_id) = screen_with_an_overlay_on_the_second_tab(size);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));

    // both shells exit before a single render tick
    for (tab_id, terminal_id) in [(TabId::new(0), 1), (TabId::new(1), 2)] {
        assert!(screen
            .tabs
            .get_mut(&tab_id)
            .expect("TEST")
            .extract_pane(PaneId::Terminal(crate::TerminalId::new(terminal_id)), true)
            .is_some());
    }
    screen.settle_tabs().expect("TEST");

    assert!(screen.tabs.is_empty());
    assert!(std::iter::from_fn(|| server_receiver.try_recv().ok())
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))));
    let torn_down = panes_torn_down_by_pty(&pty_receiver);
    assert!(
        torn_down.contains(&popup_id),
        "the overlay is torn down with the session: {torn_down:?}"
    );
}

/// An overlay is expanded over the *tab*, so the clients that did not ask for it are looking at a
/// screenful of someone else's pane and the keystroke that gets it off their screen has to exist.
/// What must not is a bystander re-sizing an expansion they never asked for - so the press puts it
/// back and goes no further, and the next one is about the pane this client is actually on.
#[test]
fn a_client_that_is_not_on_an_overlay_may_only_put_the_expansion_back() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    // a second tiled pane, so that the tiled collection has a fullscreen to give client 2 at all
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    open_overlay_with(&mut screen, 3, overlay_coordinates());
    let _ = screen
        .get_active_tab_mut(crate::ClientId::new(2))
        .unwrap()
        .focus_pane_with_id(
            PaneId::Terminal(crate::TerminalId::new(1)),
            false,
            false,
            crate::ClientId::new(2),
        );

    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(PaneId::Terminal(crate::TerminalId::new(3)))
            .expect("TEST")
            .is_fullscreen,
        "the client that is on the overlay expanded it"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(2))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(2));
    assert!(
        !screen.tabs[&TabId::new(0)]
            .get_pane_info(PaneId::Terminal(crate::TerminalId::new(3)))
            .expect("TEST")
            .is_fullscreen,
        "the other client's press put it back"
    );
    assert!(
        !screen.tabs[&TabId::new(0)].is_fullscreen_active(),
        "and went no further than that - the tiled pane it is on is untouched"
    );

    screen
        .get_active_tab_mut(crate::ClientId::new(2))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(2));
    assert_eq!(
        screen.tabs[&TabId::new(0)].fullscreen_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "now that the screen is its own again, the next press is about its own pane"
    );
}

/// `Tab::resize_whole_tab_locally` re-derives the expansion from the tab's viewport, and
/// `LayoutApplier::offset_viewport` recomputes that viewport later in the same function - so an
/// overlay expanded over the viewport, rather than over the whole surface, kept the rectangle the
/// chrome left before the resize.
#[test]
fn an_expanded_overlay_fills_the_viewport_a_resize_leaves() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    open_overlay_with(&mut screen, 2, overlay_coordinates());
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .toggle_active_pane_fullscreen(crate::ClientId::new(1));

    screen
        .resize_to_screen(Size {
            cols: 120,
            rows: 30,
        })
        .expect("TEST");

    let tab = &screen.tabs[&TabId::new(0)];
    // the lone tiled pane fills the viewport, so it is what the expansion should have grown to
    let viewport = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("TEST")
        .position_and_size();
    let expanded = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .current_geom();
    assert_eq!(
        (
            expanded.x,
            expanded.y,
            expanded.cols.as_usize(),
            expanded.rows.as_usize()
        ),
        (
            viewport.x,
            viewport.y,
            viewport.cols.as_usize(),
            viewport.rows.as_usize()
        ),
        "it fills the viewport as it is now, not as it was before the resize"
    );
    assert_eq!(expanded.cols.as_usize(), 120, "which is the wider screen");
}

#[test]
fn multipane_structural_commands_and_transfers_keep_mixed_children_contained() {
    use yinz_types::multipane::*;
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        for selected in 0..2 {
            let mut screen = create_non_mirrored_screen(Size {
                cols: 120,
                rows: 40,
            });
            new_tab(&mut screen, 1, TabId::new(0));
            new_tab(&mut screen, 2, TabId::new(1));
            let plugin = open_overlay_plugin(
                &mut screen,
                PluginId::new(7),
                TabId::new(0),
                popup_coordinates(),
            )
            .1;
            let terminal = PaneId::Terminal(crate::TerminalId::new(1));
            let children = [plugin, terminal];
            let info = screen
                .multipane_command(
                    MultipaneOperation::Create {
                        children,
                        split: MultipaneSplit {
                            direction,
                            ratio: 4300,
                        },
                        placement: OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                            x: Some(PercentOrFixed::Fixed(3)),
                            y: Some(PercentOrFixed::Fixed(3)),
                            width: Some(PercentOrFixed::Fixed(80)),
                            height: Some(PercentOrFixed::Fixed(24)),
                            ..Default::default()
                        })),
                        focus: Some(selected),
                        min_cells: 0,
                    },
                    CLIENT_1,
                )
                .unwrap()
                .unwrap();
            screen.settle_tabs().unwrap();
            let assert_contained = |screen: &Screen| {
                let tab = screen
                    .tabs
                    .values()
                    .find(|tab| tab.has_pane_with_pid(&plugin))
                    .unwrap();
                for child in children {
                    assert_eq!(tab.multipane_info(child).unwrap().id, info.id);
                }
                assert_eq!(tab.multipane_info(terminal).unwrap().split.ratio, 4300);
            };
            assert_contained(&screen);
            assert!(screen.selection_has_multipane(&[
                children[selected],
                PaneId::Terminal(crate::TerminalId::new(2))
            ]));
            let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
            assert!(tab.extract_pane(children[selected], true).is_none());
            assert!(tab
                .toggle_pane_embed_or_floating_for_pane_id(children[selected], None)
                .is_err());
            assert!(screen
                .stack_panes(vec![
                    children[selected],
                    PaneId::Terminal(crate::TerminalId::new(2))
                ])
                .is_none());
            assert_contained(&screen);
            let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
            tab.new_stacked_pane(
                PaneId::Terminal(crate::TerminalId::new(99)),
                None,
                None,
                false,
                true,
                Some(children[selected]),
                Some(CLIENT_1),
                None,
                None,
            )
            .unwrap();
            assert!(!tab.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(99))));
            let before = tab.multipane_info(plugin).unwrap();
            tab.move_pane_right(children[selected]);
            assert_eq!(tab.multipane_info(plugin).unwrap().x, before.x + 10);
            let before = tab.multipane_info(plugin).unwrap();
            screen.change_floating_panes_coordinates(vec![
                (plugin, popup_coordinates()),
                (terminal, FloatingPaneCoordinates::default()),
            ]);
            assert_eq!(
                screen.tabs[&TabId::new(0)].multipane_info(plugin).unwrap(),
                before
            );
            assert_contained(&screen);
            let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
            tab.hide_overlay_pane(children[selected]);
            assert!(children.iter().all(|id| tab.overlay_carry_for(*id).hidden));
            tab.show_overlay_pane(children[1 - selected]);
            tab.toggle_fullscreen_by_pane_id(children[selected]);
            assert_contained(&screen);
            screen
                .break_multiple_panes_to_tab_with_id(
                    vec![
                        children[selected],
                        children[1 - selected],
                        children[selected],
                    ],
                    TabId::new(1),
                    false,
                    CLIENT_1,
                )
                .unwrap();
            assert_contained(&screen);
            assert!(screen.tabs[&TabId::new(1)].has_pane_with_pid(&plugin));
            // No plugin event or repair runs between any operation and these assertions.
            screen.close_pane_explicitly(children[selected], None);
            assert!(screen
                .tabs
                .values()
                .all(|tab| children.iter().all(|id| !tab.has_pane_with_pid(id))));
        }
    }
}

#[test]
fn multipane_creation_focus_switches_to_the_children_tab_only_when_requested() {
    use yinz_types::multipane::*;
    for focus in [Some(0), Some(1), None] {
        let mut screen = create_non_mirrored_screen(Size {
            cols: 120,
            rows: 40,
        });
        new_tab(&mut screen, 1, TabId::new(0));
        let children = [7, 8].map(|id| {
            open_overlay_plugin(
                &mut screen,
                PluginId::new(id),
                TabId::new(0),
                popup_coordinates(),
            )
            .1
        });
        new_tab(&mut screen, 2, TabId::new(1));
        let previous_pane = screen.active_pane_id(CLIENT_1);
        assert_eq!(screen.active_tab_ids[&CLIENT_1], TabId::new(1));
        screen
            .multipane_command(
                MultipaneOperation::Create {
                    children,
                    split: MultipaneSplit {
                        direction: MultipaneDirection::LeftRight,
                        ratio: 5000,
                    },
                    placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                    focus,
                    min_cells: 0,
                },
                CLIENT_1,
            )
            .unwrap();
        assert_eq!(
            screen.active_tab_ids[&CLIENT_1],
            TabId::new(if focus.is_some() { 0 } else { 1 })
        );
        assert_eq!(
            screen.active_pane_id(CLIENT_1),
            focus.map(|slot| children[slot]).or(previous_pane)
        );
    }
}

#[test]
fn multipane_close_self_restores_a_replaced_child_and_only_closes_the_temporary_plugin() {
    use yinz_types::multipane::*;
    let (mut screen, _events) = screen_with_plugin_sender(Size {
        cols: 120,
        rows: 40,
    });
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    new_tab(&mut screen, 1, TabId::new(0));
    let children = [7, 8].map(|id| {
        open_overlay_plugin(
            &mut screen,
            PluginId::new(id),
            TabId::new(0),
            popup_coordinates(),
        )
        .1
    });
    let info = screen
        .multipane_command(
            MultipaneOperation::Create {
                children,
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio: 4300,
                },
                placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                focus: Some(0),
                min_cells: 0,
            },
            CLIENT_1,
        )
        .unwrap()
        .unwrap();
    let temporary = PaneId::Plugin(PluginId::new(9));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .suppress_pane_and_replace_with_pid(children[0], temporary, false, None, None, None)
        .unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)].stands_in_for_a_pane(temporary),
        Some(children[0])
    );
    assert!(panes_torn_down_by_pty(&pty_receiver).is_empty());

    crate::screen::replay_pending_plugin_events(
        &mut screen,
        vec![ScreenInstruction::CloseSelf(PluginId::new(9), CLIENT_1)],
    )
    .unwrap();

    let tab = &screen.tabs[&TabId::new(0)];
    assert!(!tab.has_pane_with_pid(&temporary));
    for child in children {
        assert_eq!(tab.multipane_info(child), Some(info.clone()));
    }
    assert_eq!(screen.active_pane_id(CLIENT_1), Some(children[0]));
    assert_eq!(panes_torn_down_by_pty(&pty_receiver), vec![temporary]);
}

#[test]
fn multipane_scrollback_editor_exit_restores_the_child_while_ordinary_exits_hold() {
    use yinz_types::multipane::*;
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    let original = PaneId::Terminal(crate::TerminalId::new(1));
    let sibling = open_overlay_plugin(
        &mut screen,
        PluginId::new(7),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let info = screen
        .multipane_command(
            MultipaneOperation::Create {
                children: [original, sibling],
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio: 4300,
                },
                placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                focus: Some(0),
                min_cells: 0,
            },
            CLIENT_1,
        )
        .unwrap()
        .unwrap();
    let editor = PaneId::Terminal(crate::TerminalId::new(9));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .replace_pane_with_editor_pane(editor, original)
        .unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .multipane_info(editor)
            .unwrap()
            .children,
        [editor, sibling]
    );

    crate::screen::replay_pending_plugin_events(
        &mut screen,
        vec![ScreenInstruction::ClosePane(editor, None, None, Some(0))],
    )
    .unwrap();
    let tab = &screen.tabs[&TabId::new(0)];
    assert!(
        !tab.has_pane_with_pid(&editor),
        "the exited editor must leave its slot"
    );
    assert_eq!(tab.multipane_info(original).unwrap(), info);
    assert_eq!(tab.multipane_info(sibling).unwrap(), info);
    assert!(!tab.get_pane_with_id(original).unwrap().is_held());

    crate::screen::replay_pending_plugin_events(
        &mut screen,
        vec![ScreenInstruction::ClosePane(original, None, None, Some(17))],
    )
    .unwrap();
    let tab = &screen.tabs[&TabId::new(0)];
    assert_eq!(tab.multipane_info(original).unwrap(), info);
    assert!(tab.get_pane_with_id(original).unwrap().is_held());
}

#[test]
fn multipane_replacing_a_child_with_the_last_working_pane_retains_the_tab() {
    use yinz_types::multipane::*;
    for floating in [false, true] {
        // The structural command and both variants of the generic replacement share this policy.
        for replacement_path in 0..3 {
            let mut screen = create_non_mirrored_screen(Size {
                cols: 120,
                rows: 40,
            });
            new_tab(&mut screen, 1, TabId::new(0));
            new_tab(&mut screen, 2, TabId::new(1));
            screen
                .switch_active_tab(TabPosition::new(0), None, true, CLIENT_1)
                .unwrap();
            let terminal = PaneId::Terminal(crate::TerminalId::new(1));
            if floating {
                let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
                let pane = tab.extract_pane(terminal, true).unwrap();
                tab.add_floating_pane(pane, terminal, None, true, Some(CLIENT_1))
                    .unwrap();
                assert!(tab.pane_id_is_floating(&terminal));
            }
            let children = [7, 8].map(|id| {
                open_overlay_plugin(
                    &mut screen,
                    PluginId::new(id),
                    TabId::new(0),
                    popup_coordinates(),
                )
                .1
            });
            let info = screen
                .multipane_command(
                    MultipaneOperation::Create {
                        children,
                        split: MultipaneSplit {
                            direction: MultipaneDirection::LeftRight,
                            ratio: 4300,
                        },
                        placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                        focus: Some(0),
                        min_cells: 0,
                    },
                    CLIENT_1,
                )
                .unwrap()
                .unwrap();
            if replacement_path == 0 {
                screen
                    .multipane_command(
                        MultipaneOperation::Replace {
                            id: info.id,
                            slot: 0,
                            child: terminal,
                        },
                        CLIENT_1,
                    )
                    .unwrap();
            } else {
                screen.replace_pane_with_existing_pane(
                    children[0],
                    terminal,
                    replacement_path == 2,
                    None,
                );
            }
            screen.settle_tabs().unwrap();
            assert_eq!(
                screen.active_tab_ids[&CLIENT_1],
                TabId::new(0),
                "replacement must not move the client to the other usable tab"
            );
            let tab = &screen.tabs[&TabId::new(0)];
            assert!(tab.has_selectable_panes());
            let updated = tab.multipane_info(terminal).unwrap();
            assert_eq!(updated.id, info.id);
            assert_eq!(updated.children, [terminal, children[1]]);
            screen
                .switch_active_tab(TabPosition::new(1), None, true, CLIENT_1)
                .unwrap();
            screen.settle_tabs().unwrap();
            screen
                .switch_active_tab(TabPosition::new(0), None, true, CLIENT_1)
                .unwrap();
            screen.settle_tabs().unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT_1], TabId::new(0));

            // Replacing with an overlay later must preserve the container's retention.
            let overlay = open_overlay_plugin(
                &mut screen,
                PluginId::new(9),
                TabId::new(0),
                popup_coordinates(),
            )
            .1;
            screen.replace_pane_with_existing_pane(terminal, overlay, false, None);
            screen.settle_tabs().unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT_1], TabId::new(0));
            assert_eq!(
                screen.tabs[&TabId::new(0)]
                    .multipane_info(overlay)
                    .unwrap()
                    .id,
                info.id
            );
        }
    }
}

#[test]
fn multipane_rejected_adoption_changes_nothing_and_child_replacement_retains_the_slot() {
    use yinz_types::multipane::*;
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    let first = open_overlay_plugin(
        &mut screen,
        PluginId::new(7),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let second = open_overlay_plugin(
        &mut screen,
        PluginId::new(8),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let create = |children, ratio| MultipaneOperation::Create {
        children,
        split: MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio,
        },
        placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
        focus: None,
        min_cells: 0,
    };
    for operation in [
        create([first, first], 5000),
        create([first, second], 0),
        create([first, PaneId::Plugin(PluginId::new(99))], 5000),
    ] {
        assert!(screen.multipane_command(operation, CLIENT_1).is_err());
        for child in [first, second] {
            assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&child));
            assert!(screen.tabs[&TabId::new(0)].multipane_info(child).is_none());
        }
    }
    let info = screen
        .multipane_command(create([first, second], 5000), CLIENT_1)
        .unwrap()
        .unwrap();
    let terminal = PaneId::Terminal(crate::TerminalId::new(1));
    let updated = screen
        .multipane_command(
            MultipaneOperation::Replace {
                id: info.id,
                slot: 0,
                child: terminal,
            },
            CLIENT_1,
        )
        .unwrap()
        .unwrap();
    assert_eq!(updated.children, [terminal, second]);
    assert_eq!(updated.id, info.id);
    assert!(screen.tabs[&TabId::new(0)].multipane_info(first).is_none());
    assert!(screen
        .multipane_command(create([terminal, second], 5000), CLIENT_1)
        .is_err());
    let command = screen.replacement_exit_command(terminal).unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hold_pane(terminal, Some(17), false, command);
    assert_eq!(
        screen.tabs[&TabId::new(0)].multipane_info(second).unwrap(),
        updated
    );
    assert!(screen.tabs[&TabId::new(0)]
        .get_pane_with_id(terminal)
        .unwrap()
        .is_held());
}

#[test]
fn multipane_transfer_carries_replacements_and_tab_closure_keeps_the_container() {
    use yinz_types::multipane::*;
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let first = open_overlay_plugin(
        &mut screen,
        PluginId::new(7),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let second = open_overlay_plugin(
        &mut screen,
        PluginId::new(8),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let info = screen
        .multipane_command(
            MultipaneOperation::Create {
                children: [first, second],
                split: MultipaneSplit {
                    direction: MultipaneDirection::TopBottom,
                    ratio: 3500,
                },
                placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                focus: Some(1),
                min_cells: 0,
            },
            CLIENT_1,
        )
        .unwrap()
        .unwrap();
    let editor = PaneId::Terminal(crate::TerminalId::new(9));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .replace_pane_with_editor_pane(editor, first)
        .unwrap();
    screen
        .break_multiple_panes_to_tab_with_id(vec![editor, second], TabId::new(1), false, CLIENT_1)
        .unwrap();
    assert!(!screen.tabs[&TabId::new(0)].has_pane_with_pid(&first));
    let destination = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    assert!(destination.has_pane_with_pid(&first));
    assert_eq!(
        destination.multipane_info(second).unwrap().children,
        [editor, second]
    );
    destination
        .replace_pane_with_suppressed_pane(editor)
        .unwrap();
    assert_eq!(destination.multipane_info(first).unwrap().id, info.id);
    assert!(destination.warp_pane_ids(Some(first)).is_err());
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)].multipane_info(first).unwrap(),
        info
    );
    let close = screen.close_self_instruction(PluginId::new(8), CLIENT_1);
    assert!(matches!(&close, ScreenInstruction::ClosePane(id, Some(_), _, _) if *id == second));
    crate::screen::replay_pending_plugin_events(&mut screen, vec![close]).unwrap();
    assert!(screen
        .tabs
        .values()
        .all(|tab| !tab.has_pane_with_pid(&first) && !tab.has_pane_with_pid(&second)));
}

#[test]
fn closing_a_multipane_repaints_the_idle_terminal_underneath() {
    use yinz_types::multipane::*;
    for selected in 0..2 {
        let size = Size {
            cols: 120,
            rows: 40,
        };
        let mut screen = create_non_mirrored_screen(size);
        new_tab(&mut screen, 1, TabId::new(0));
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(size);
        let underlying = PaneId::Terminal(crate::TerminalId::new(1));
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(1), b"IDLE TERMINAL".to_vec())
            .unwrap();
        open_popup(&mut screen, 2, "LEFT CHILD");
        open_popup(&mut screen, 3, "RIGHT CHILD");
        let children = [2, 3].map(|id| PaneId::Terminal(crate::TerminalId::new(id)));
        screen
            .multipane_command(
                MultipaneOperation::Create {
                    children,
                    split: MultipaneSplit {
                        direction: MultipaneDirection::LeftRight,
                        ratio: 5000,
                    },
                    placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                    focus: None,
                    min_cells: 0,
                },
                CLIENT_1,
            )
            .unwrap();
        screen
            .focus_pane_with_id(underlying, false, false, CLIENT_1)
            .unwrap();
        // Drain synchronous incremental renders so no earlier invalidation can mask the close.
        for _ in 0..3 {
            render_frame_into(&mut screen, &receiver, CLIENT_1, &mut grid);
        }
        screen.close_pane_explicitly(children[selected], None);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT_1, &mut grid);
        assert!(frame.contains("IDLE TERMINAL"), "{frame}");
        assert!(
            !frame.contains("LEFT CHILD") && !frame.contains("RIGHT CHILD"),
            "{frame}"
        );
        assert_eq!(screen.active_pane_id(CLIENT_1), Some(underlying));
    }
}

#[test]
fn retained_multipane_repairs_focus_when_the_last_working_pane_exits() {
    use yinz_types::multipane::*;
    for (hidden, first_selectable) in [(false, true), (true, true), (false, false)] {
        let mut screen = create_non_mirrored_screen(Size {
            cols: 120,
            rows: 40,
        });
        let (writer, writes) = channels::unbounded();
        screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(writer));
        new_tab(&mut screen, 1, TabId::new(0));
        open_floating_pane(&mut screen, TabId::new(0), 2, "FIRST", false);
        open_floating_pane(&mut screen, TabId::new(0), 3, "SECOND", false);
        let children = [2, 3].map(|id| PaneId::Terminal(crate::TerminalId::new(id)));
        let underlying = PaneId::Terminal(crate::TerminalId::new(1));
        screen
            .focus_pane_with_id(underlying, false, false, CLIENT_1)
            .unwrap();
        screen
            .multipane_command(
                MultipaneOperation::Create {
                    children,
                    split: MultipaneSplit {
                        direction: MultipaneDirection::LeftRight,
                        ratio: 5000,
                    },
                    placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                    focus: None,
                    min_cells: 0,
                },
                CLIENT_1,
            )
            .unwrap();
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.set_pane_selectable(children[0], first_selectable);
        if hidden {
            assert!(tab.hide_overlay_pane(children[0]));
        }
        assert_eq!(tab.get_active_pane_id(CLIENT_1), Some(underlying));
        tab.close_pane(underlying, false, Some(0));
        screen.settle_tabs().unwrap();
        assert!(screen.tabs.contains_key(&TabId::new(0)));
        let expected = children[usize::from(!first_selectable)];
        assert_eq!(screen.active_pane_id(CLIENT_1), Some(expected));
        writes.try_iter().for_each(drop);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .write_to_active_terminal(&None, b"x".to_vec(), false, CLIENT_1, None)
            .unwrap();
        assert!(writes.try_iter().any(|(instruction, _)| matches!(instruction,
            PtyWriteInstruction::Write(bytes, id, _) if PaneId::Terminal(id) == expected && bytes == b"x"
        )));
    }
}

#[test]
fn multipane_children_receive_visibility_after_transfer_hide_and_reveal() {
    use yinz_types::multipane::*;
    let (mut screen, events) = screen_with_plugin_sender(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let children = [7, 8].map(|id| {
        open_overlay_plugin(
            &mut screen,
            PluginId::new(id),
            TabId::new(0),
            popup_coordinates(),
        )
        .1
    });
    screen
        .multipane_command(
            MultipaneOperation::Create {
                children,
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio: 5000,
                },
                placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                focus: None,
                min_cells: 0,
            },
            CLIENT_1,
        )
        .unwrap();
    // Both tabs retain their visibility during the move: only container adoption can notify
    // the children that they have arrived on a visible tab.
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .visible(false)
        .unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .visible(true)
        .unwrap();
    events.try_iter().for_each(drop);
    let visibility_events = || {
        events
            .try_iter()
            .flat_map(|(event, _)| match event {
                PluginInstruction::Update(updates) => updates
                    .into_iter()
                    .filter_map(|(id, _, event)| match event {
                        Event::Visible(value) => Some((id, value)),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect::<Vec<_>>()
    };
    let assert_visible = |visible| {
        assert_eq!(
            visibility_events(),
            vec![
                (Some(PluginId::new(7)), visible),
                (Some(PluginId::new(8)), visible)
            ]
        );
    };
    screen
        .break_multiple_panes_to_tab_with_id(vec![children[0]], TabId::new(1), false, CLIENT_1)
        .unwrap();
    assert_visible(true);
    let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    assert!(tab.hide_overlay_pane(children[0]));
    assert_visible(false);
    assert!(tab.show_overlay_pane(children[1]));
    assert_visible(true);
    screen
        .switch_active_tab(TabPosition::new(1), None, true, CLIENT_1)
        .unwrap();
    events.try_iter().for_each(drop);
    for child in children {
        for launch in [false, true] {
            let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
            assert!(tab.hide_overlay_pane(child));
            assert_visible(false);
            let run_plugin = match tab.get_pane_with_id(child).unwrap().invoked_with() {
                Some(Run::Plugin(run)) => run.clone(),
                _ => panic!("expected plugin child"),
            };
            for attempt in 0..2 {
                if launch {
                    assert!(screen
                        .focus_plugin_pane(&run_plugin, true, true, false, CLIENT_1, &mut None)
                        .unwrap());
                } else {
                    // ShowSelf and FocusPaneWithId both reach this screen entry point.
                    screen
                        .focus_pane_with_id(child, true, false, CLIENT_1)
                        .unwrap();
                }
                assert!(!screen.tabs[&TabId::new(1)].overlay_carry_for(child).hidden);
                assert_eq!(screen.active_pane_id(CLIENT_1), Some(child));
                if attempt == 0 {
                    assert_visible(true);
                } else {
                    assert!(visibility_events().is_empty());
                }
            }
        }
    }
    screen
        .break_multiple_panes_to_tab_with_id(vec![children[1]], TabId::new(0), false, CLIENT_1)
        .unwrap();
    assert_visible(false);
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    assert!(tab.hide_overlay_pane(children[0]));
    assert_visible(false);
    tab.focus_pane_with_id(children[1], false, false, CLIENT_1)
        .unwrap();
    assert_visible(false);
}

#[test]
fn pane_tabs_lazy_children_receive_only_owning_tab_input_and_one_mode_update() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};

    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    let (sender, receiver) = channels::unbounded();
    screen
        .bus
        .senders
        .replace_to_plugin(SenderWithContext::new(sender));
    let client = ClientId::new(1);
    let owner = TabId::new(0);
    new_tab(&mut screen, 1, owner);
    let first = PluginId::new(100);
    let child = PluginId::new(101);
    let tab = screen.tabs.get_mut(&owner).unwrap();
    tab.new_pane(
        PaneId::Plugin(first),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    let layout = PaneTabsLayout {
        active: 0,
        tabs: ["First", "Child"]
            .map(|label| PaneTabDescriptor {
                label: label.into(),
                location: "yinz:strider".into(),
                initial_cwd: None,
                configuration: BTreeMap::new(),
            })
            .to_vec(),
    };
    let info = tab
        .mount_pane_tabs(PaneId::Plugin(first), layout, 0, client)
        .unwrap();
    tab.select_pane_tab(info.id, 1, client).unwrap();
    assert!(receiver.try_iter().any(|(instruction, _)| matches!(instruction, PluginInstruction::LoadPaneTab { tab_id, .. } if tab_id == owner)));
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:strider",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(info.id, 1, 1, Ok((child, run)))
        .unwrap();
    new_tab(&mut screen, 2, TabId::new(1));
    let targets = screen.targeted_plugin_ids(client, EventType::InputReceived);
    assert!(!targets.contains(&child));
    assert!(!targets.contains(&first));
    screen.go_to_tab(TabNumber::new(1), client).unwrap();
    let targets = screen.targeted_plugin_ids(client, EventType::InputReceived);
    assert!(targets.contains(&child));
    receiver.try_iter().for_each(drop);
    screen.change_mode(InputMode::Locked, None, client).unwrap();
    let targets = receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(events) => events,
            _ => vec![],
        })
        .filter_map(|(plugin, target_client, event)| {
            (target_client == Some(client) && matches!(event, Event::ModeUpdate(_)))
                .then_some(plugin)
                .flatten()
        })
        .collect::<Vec<_>>();
    assert_eq!(targets.iter().filter(|target| **target == child).count(), 1);
}

#[test]
fn disconnected_plugin_grids_are_removed_from_inactive_suppressed_and_overlay_panes() {
    use yinz_bus::plugins::PluginRenderAsset;
    for ansi in [false, true] {
        let low = ClientId::new(1);
        let high = ClientId::new(2);
        let subscriber = ClientId::new(3);
        let (mut screen, _plugins) = screen_with_plugin_sender(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.add_client(high).unwrap();
        for (id, suppressed) in [(70, false), (71, true)] {
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .new_pane(
                    PaneId::Plugin(PluginId::new(id)),
                    None,
                    None,
                    suppressed,
                    false,
                    NewPanePlacement::default(),
                    Some(low),
                    None,
                )
                .unwrap();
        }
        open_overlay_plugin(
            &mut screen,
            PluginId::new(72),
            TabId::new(0),
            popup_coordinates(),
        );
        let panes: Vec<_> = (70..73)
            .map(|id| PaneId::Plugin(PluginId::new(id)))
            .collect();
        for client in [high, low] {
            screen
                .handle_plugin_render_assets(
                    (70..73)
                        .map(|id| {
                            PluginRenderAsset::new(
                                PluginId::new(id),
                                client,
                                if client == low {
                                    b"\x1b[31mlow-grid".to_vec()
                                } else {
                                    b"high-grid".to_vec()
                                },
                            )
                        })
                        .collect(),
                )
                .unwrap();
        }
        new_tab(&mut screen, 2, TabId::new(1));
        // The low grid survives a tab switch, even though high is still viewing tab zero.
        let os = FakeInputOutput::default();
        let messages = os.server_to_client_messages.clone();
        screen.bus.os_input = Some(Box::new(os));
        screen.subscribe_to_pane_renders(subscriber, panes.clone(), None, ansi);
        assert!(messages.lock().unwrap()[&subscriber].iter().all(|msg| matches!(msg,
            ServerToClientMsg::PaneRenderUpdate { viewport, .. } if viewport.concat().contains("low-grid"))));
        messages.lock().unwrap().clear();
        screen.remove_client(low).unwrap();
        screen
            .handle_plugin_render_assets(
                (70..73)
                    .map(|id| {
                        PluginRenderAsset::new(
                            PluginId::new(id),
                            low,
                            b"late-disconnected-output".to_vec(),
                        )
                    })
                    .collect(),
            )
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(messages.lock().unwrap()[&subscriber].len(), 3);
        assert!(messages.lock().unwrap()[&subscriber]
            .iter()
            .all(|msg| matches!(msg,
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
                if viewport.concat().contains("high-grid"))));
        messages.lock().unwrap().clear();
        screen.remove_client(high).unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(messages.lock().unwrap()[&subscriber].len(), 3);
        assert!(messages.lock().unwrap()[&subscriber].iter().all(|msg| matches!(msg,
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. } if viewport.is_empty())));
        screen.add_client(low).unwrap();
        screen
            .handle_plugin_render_assets(
                (70..73)
                    .map(|id| {
                        PluginRenderAsset::new(PluginId::new(id), low, b"reused-client".to_vec())
                    })
                    .collect(),
            )
            .unwrap();
        for pane in panes {
            assert!(screen.tabs[&TabId::new(0)]
                .get_pane_with_id(pane)
                .unwrap()
                .pane_contents(Some(low), false, None)
                .viewport
                .concat()
                .contains("reused-client"));
        }
    }
}

#[test]
fn suppressed_plugins_are_active_tab_broadcast_targets_without_duplicates() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    let (sender, receiver) = channels::unbounded();
    screen
        .bus
        .senders
        .replace_to_plugin(SenderWithContext::new(sender));
    new_tab(&mut screen, 1, TabId::new(0));
    let client = ClientId::new(1);
    let plugin = PluginId::new(9);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .new_pane(
            PaneId::Plugin(plugin),
            None,
            None,
            true,
            true,
            yinz_types::data::NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
    for event_type in [
        EventType::ModeUpdate,
        EventType::TabUpdate,
        EventType::PaneUpdate,
        EventType::InputReceived,
    ] {
        assert_eq!(screen.targeted_plugin_ids(client, event_type), vec![plugin]);
        screen
            .background_plugin_subscriptions
            .entry((plugin, client))
            .or_default()
            .insert(event_type);
        assert_eq!(screen.targeted_plugin_ids(client, event_type), vec![plugin]);
    }
    receiver.try_iter().for_each(drop);
    screen
        .broadcast_mode_update(ModeInfo::default(), Some(client))
        .unwrap();
    screen.generate_and_report_tab_state().unwrap();
    screen.generate_and_report_pane_state().unwrap();
    screen.notify_plugins_of_input(client);
    let updates: Vec<_> = receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .collect();
    assert_eq!(updates.len(), 4);
    assert!(updates
        .iter()
        .all(|(pid, cid, _)| *pid == Some(plugin) && *cid == Some(client)));
    assert!(matches!(updates[0].2, Event::ModeUpdate(_)));
    assert!(matches!(updates[1].2, Event::TabUpdate(_)));
    assert!(matches!(updates[2].2, Event::PaneUpdate(_)));
    assert!(matches!(updates[3].2, Event::InputReceived));
    screen.background_plugin_subscriptions.clear();
    new_tab(&mut screen, 2, TabId::new(1));
    screen.go_to_tab(TabNumber::new(2), client).unwrap();
    assert!(!screen
        .targeted_plugin_ids(client, EventType::InputReceived)
        .contains(&plugin));
}

#[expect(
    clippy::type_complexity,
    reason = "fixture returns the screen and its plugin and pty instruction receivers"
)]
fn configured_overlay_screen() -> (
    Screen,
    channels::Receiver<(PluginInstruction, ErrorContext)>,
    channels::Receiver<(PtyInstruction, ErrorContext)>,
) {
    let (mut screen, plugins) = screen_with_plugin_sender(Size {
        cols: 120,
        rows: 40,
    });
    let (tx, ptys) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(tx));
    (screen, plugins, ptys)
}

fn configured_overlay_run(location: &str) -> RunPluginOrAlias {
    yinz_runtime::plugin_location::run_plugin_or_alias_from_url(location, &None, None, None)
        .unwrap()
}

fn launch_configured_overlay(screen: &mut Screen, location: &str, client: ClientId) {
    screen
        .launch_or_focus_plugin(
            configured_overlay_run(location),
            false,
            false,
            false,
            false,
            None,
            false,
            client,
            None,
            None,
        )
        .unwrap();
}

fn next_overlay_load(
    screen: &mut Screen,
    ptys: &channels::Receiver<(PtyInstruction, ErrorContext)>,
    events: &channels::Receiver<(PluginInstruction, ErrorContext)>,
) -> (
    yinz_types::pane_tabs::PaneTabsId,
    usize,
    u64,
    yinz_types::pane_tabs::PaneTabDescriptor,
) {
    for (event, _) in ptys.try_iter() {
        if let PtyInstruction::ResolveOverlayTabCwd { id, cwd, .. } = event {
            screen
                .overlay_tab_cwd(
                    id,
                    cwd.or_else(|| Some(std::path::PathBuf::from("/focused"))),
                    Some(std::path::PathBuf::from("/focused")),
                    Some(yinz_types::internal_ids::TerminalId::new(7)),
                )
                .unwrap();
        }
    }
    events
        .try_iter()
        .find_map(|(event, _)| match event {
            PluginInstruction::LoadPaneTab {
                id,
                index,
                generation,
                descriptor,
                ..
            } => Some((id, index, generation, descriptor)),
            _ => None,
        })
        .expect("a plugin tab load")
}

#[test]
fn configured_overlay_coalesces_launches_selects_unloaded_tabs_and_survives_reload() {
    let (mut screen, events, ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    let config = Config::from_kdl(
        r#"
        plugins { custom location="file:overlay-custom.scm" { option "value"; cwd "sub"; }; }
        overlay_tab_plugins { custom label="First"; "file:overlay-second.scm" label="Second"; }
    "#,
        None,
    )
    .unwrap();
    screen
        .reconfigure(
            ScreenConfig::from_config(&config),
            Keybinds::default(),
            CLIENT_1,
        )
        .unwrap();
    let absolute_custom = configured_overlay_run("file:overlay-custom.scm")
        .get_run_plugin()
        .unwrap()
        .location
        .display();
    let absolute_second = configured_overlay_run("file:overlay-second.scm")
        .get_run_plugin()
        .unwrap()
        .location
        .display();
    launch_configured_overlay(&mut screen, &absolute_custom, CLIENT_1);
    launch_configured_overlay(&mut screen, "custom", CLIENT_1);
    let (id, first, generation, descriptor) = next_overlay_load(&mut screen, &ptys, &events);
    assert_eq!(descriptor.configuration["caller_cwd"], "/focused");
    assert_eq!(descriptor.configuration["caller_terminal_pane_id"], "7");
    assert_eq!(
        descriptor.initial_cwd.as_deref(),
        Some(std::path::Path::new("/focused/sub"))
    );
    assert_eq!(
        descriptor.configuration.get("option").map(String::as_str),
        Some("value")
    );
    launch_configured_overlay(&mut screen, &absolute_second, CLIENT_1);
    launch_configured_overlay(&mut screen, "file:overlay-second.scm", CLIENT_1);
    assert!(!events
        .try_iter()
        .any(|(event, _)| matches!(event, PluginInstruction::LoadPaneTab { .. })));
    screen
        .pane_tab_loaded(
            id,
            first,
            generation,
            Ok((
                PluginId::new(100),
                configured_overlay_run("file:overlay-custom.scm"),
            )),
        )
        .unwrap();
    let (id, second, generation, descriptor) = next_overlay_load(&mut screen, &ptys, &events);
    assert_eq!(
        descriptor.initial_cwd.as_deref(),
        Some(std::path::Path::new("/focused"))
    );
    let info = screen.tabs[&TabId::new(0)].pane_tabs_info(id).unwrap();
    assert!(info.overlay_group);
    assert_eq!(info.active, 1);
    screen
        .pane_tab_loaded(
            id,
            second,
            generation,
            Ok((
                PluginId::new(101),
                configured_overlay_run("file:overlay-second.scm"),
            )),
        )
        .unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .close_pane_tab_child(id, 0)
        .unwrap();
    launch_configured_overlay(&mut screen, &absolute_custom, CLIENT_1);
    let (retry_id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    assert_eq!((retry_id, index), (id, 0));
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((
                PluginId::new(102),
                configured_overlay_run("file:overlay-custom.scm"),
            )),
        )
        .unwrap();
    launch_configured_overlay(&mut screen, &absolute_second, CLIENT_1);
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .pane_tabs_info(id)
            .unwrap()
            .active,
        1
    );
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hide_overlay_pane(PaneId::Plugin(PluginId::new(101)));
    let empty = Config::from_kdl("overlay_tab_plugins {}", None).unwrap();
    screen
        .reconfigure(
            ScreenConfig::from_config(&empty),
            Keybinds::default(),
            CLIENT_1,
        )
        .unwrap();
    launch_configured_overlay(&mut screen, "file:overlay-custom.scm", CLIENT_1);
    let tab = &screen.tabs[&TabId::new(0)];
    let info = tab.pane_tabs_info(id).unwrap();
    assert_eq!(info.active, 0);
    assert_eq!(info.tabs[0].descriptor.label, "First");
    assert!(!tab.get_pane_info(info.pane_id).unwrap().is_suppressed);
    assert_eq!(tab.pane_tabs_infos().count(), 1);
    assert!(!events
        .try_iter()
        .any(|(event, _)| matches!(event, PluginInstruction::LoadPaneTab { .. })));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .close_pane_tabs(id)
        .unwrap();
    launch_configured_overlay(&mut screen, "file:overlay-custom.scm", CLIENT_1);
    assert!(screen.pending_overlay_tabs.is_none());
}

#[test]
fn configured_overlay_load_failure_releases_pending_launch_and_retry_reuses_slots() {
    let (mut screen, events, ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    launch_configured_overlay(&mut screen, "yinz:todos", CLIENT_1);
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    screen
        .pane_tab_loaded(id, index, generation, Err("load failed".into()))
        .unwrap();
    assert!(screen.pending_overlay_tabs.is_none());
    launch_configured_overlay(&mut screen, "yinz:todos", CLIENT_1);
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(100), configured_overlay_run("yinz:todos"))),
        )
        .unwrap();
    launch_configured_overlay(&mut screen, "yinz:agents", CLIENT_1);
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    screen
        .pane_tab_loaded(id, index, generation, Err("permission denied".into()))
        .unwrap();
    assert!(matches!(
        screen.tabs[&TabId::new(0)].pane_tabs_info(id).unwrap().tabs[index].state,
        yinz_types::pane_tabs::PaneTabLoadState::Failed(_)
    ));
    launch_configured_overlay(&mut screen, "yinz:agents", CLIENT_1);
    let (retry_id, retry_index, retry_generation, _) =
        next_overlay_load(&mut screen, &ptys, &events);
    assert_eq!((retry_id, retry_index), (id, index));
    assert!(retry_generation > generation);
}

#[test]
fn configured_overlay_follows_tabs_without_agents_and_preserves_geometry() {
    for mirrored in [false, true] {
        let (mut screen, events, ptys) = configured_overlay_screen();
        screen.session_is_mirrored = mirrored;
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        screen.overlay_tab_plugins =
            Config::from_kdl("overlay_tab_plugins { \"file:/custom.scm\"; }", None)
                .unwrap()
                .resolved_overlay_tab_plugins()
                .unwrap();
        launch_configured_overlay(&mut screen, "file:/custom.scm", CLIENT_1);
        let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
        screen
            .pane_tab_loaded(
                id,
                index,
                generation,
                Ok((
                    PluginId::new(100),
                    configured_overlay_run("file:/custom.scm"),
                )),
            )
            .unwrap();
        let pane = PaneId::Plugin(PluginId::new(100));
        screen.change_overlay_coordinates(vec![(pane, popup_coordinates())], true);
        let source = *screen.active_tab_ids.get(&CLIENT_1).unwrap();
        let target = if source == TabId::new(0) {
            TabId::new(1)
        } else {
            TabId::new(0)
        };
        let destination_focus = screen.tabs[&target].get_active_pane_id(CLIENT_1);
        assert!(matches!(destination_focus, Some(PaneId::Terminal(_))));
        assert_eq!(screen.get_active_pane_id(&CLIENT_1), Some(pane));
        let placement = screen.tabs[&source].overlay_placement_for(pane);
        screen
            .switch_active_tab(screen.tabs[&target].position, None, true, CLIENT_1)
            .unwrap();
        assert!(screen.tabs[&target].pane_is_overlay(pane));
        assert_eq!(screen.tabs[&target].overlay_placement_for(pane), placement);
        assert!(!screen.tabs[&source].pane_is_overlay(pane));
        assert_eq!(screen.get_active_pane_id(&CLIENT_1), destination_focus);
        if mirrored {
            assert_eq!(screen.active_tab_ids[&other], target);
            assert_eq!(screen.get_active_pane_id(&other), destination_focus);
        } else {
            assert_eq!(screen.active_tab_ids[&other], source);
        }
        launch_configured_overlay(&mut screen, "file:/custom.scm", CLIENT_1);
        assert_eq!(screen.get_active_pane_id(&CLIENT_1), Some(pane));
    }
}

#[test]
fn configured_overlay_skips_explicit_placement_and_background_launches() {
    let (mut screen, events, _ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    for (in_place, replacement, explicit_tab, no_focus) in [
        (
            true,
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            None,
            false,
        ),
        (false, None, Some(TabId::new(0)), false),
        (false, None, None, true),
    ] {
        screen
            .launch_plugin(
                configured_overlay_run("yinz:agents"),
                false,
                in_place,
                false,
                replacement,
                false,
                None,
                CLIENT_1,
                None,
                explicit_tab,
                no_focus,
                None,
            )
            .unwrap();
        assert!(screen.pending_overlay_tabs.is_none());
    }
    assert!(!events
        .try_iter()
        .any(|(event, _)| matches!(event, PluginInstruction::LoadPaneTab { .. })));
}

#[test]
fn configured_overlay_plugin_launch_uses_acting_clients_tab_and_new_groups_use_reloaded_list() {
    let (mut screen, events, ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen
        .switch_active_tab(screen.tabs[&TabId::new(0)].position, None, true, CLIENT_1)
        .unwrap();
    launch_configured_overlay(&mut screen, "yinz:todos", other);
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(100), configured_overlay_run("yinz:todos"))),
        )
        .unwrap();
    let pane = PaneId::Plugin(PluginId::new(100));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .hide_overlay_pane(pane);
    // Plugin actions carry their source pane even when they are not replacing it.
    screen
        .launch_plugin(
            configured_overlay_run("yinz:todos"),
            false,
            false,
            false,
            Some(PaneId::Plugin(PluginId::new(200))),
            false,
            None,
            CLIENT_1,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    assert!(screen.tabs[&TabId::new(0)].pane_is_overlay(pane));
    assert_eq!(screen.active_tab_ids[&other], TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT_1), Some(pane));
    let info = screen.tabs[&TabId::new(0)]
        .pane_tabs_infos()
        .next()
        .unwrap();
    let config = Config::from_kdl(
        "overlay_tab_plugins { \"yinz:todos\" label=\"Only\"; }",
        None,
    )
    .unwrap();
    screen
        .reconfigure(
            ScreenConfig::from_config(&config),
            Keybinds::default(),
            CLIENT_1,
        )
        .unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .pane_tabs_info(info.id)
            .unwrap()
            .tabs
            .len(),
        2
    );
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .close_pane_tabs(info.id)
        .unwrap();
    screen
        .new_floating_plugin_pane(
            configured_overlay_run("yinz:todos"),
            None,
            false,
            None,
            NewPanePlacement::Floating(None),
            CLIENT_1,
            None,
            None,
            false,
        )
        .unwrap();
    let (id, index, generation, descriptor) = next_overlay_load(&mut screen, &ptys, &events);
    assert_eq!(descriptor.label, "Only");
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(101), configured_overlay_run("yinz:todos"))),
        )
        .unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .pane_tabs_infos()
            .next()
            .unwrap()
            .tabs
            .len(),
        1
    );
}

fn launch_overlay_with_completion(
    screen: &mut Screen,
    location: &str,
) -> tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    screen
        .launch_or_focus_plugin(
            configured_overlay_run(location),
            false,
            false,
            false,
            false,
            None,
            false,
            CLIENT_1,
            Some(crate::NotificationEnd::new(tx)),
            None,
        )
        .unwrap();
    rx
}

#[test]
fn configured_overlay_lazy_launch_completion_follows_requested_slot() {
    let (mut screen, events, ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    let mut first = launch_overlay_with_completion(&mut screen, "yinz:agents");
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    assert!(first.try_recv().is_err());
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(100), configured_overlay_run("yinz:agents"))),
        )
        .unwrap();
    assert_eq!(
        first.try_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(PluginId::new(100)))
    );

    let (tx, mut second) = tokio::sync::oneshot::channel();
    screen
        .launch_plugin(
            configured_overlay_run("yinz:todos"),
            false,
            false,
            false,
            None,
            false,
            None,
            CLIENT_1,
            Some(crate::NotificationEnd::new(tx)),
            None,
            false,
            None,
        )
        .unwrap();
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    assert!(second.try_recv().is_err());
    let mut duplicate = launch_overlay_with_completion(&mut screen, "yinz:todos");
    let mut loaded = launch_overlay_with_completion(&mut screen, "yinz:agents");
    assert_eq!(
        loaded.try_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(PluginId::new(100)))
    );
    screen
        .pane_tab_loaded(
            id,
            index,
            generation - 1,
            Ok((PluginId::new(101), configured_overlay_run("yinz:todos"))),
        )
        .unwrap();
    assert!(second.try_recv().is_err());
    assert!(duplicate.try_recv().is_err());
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(102), configured_overlay_run("yinz:todos"))),
        )
        .unwrap();
    for completion in [&mut second, &mut duplicate] {
        assert_eq!(
            completion.try_recv().unwrap().affected_pane_id,
            Some(PaneId::Plugin(PluginId::new(102)))
        );
    }
    assert_eq!(
        screen.active_pane_id(CLIENT_1),
        Some(PaneId::Plugin(PluginId::new(100)))
    );
}

#[test]
fn configured_overlay_coalesced_completions_load_each_requested_slot() {
    let (mut screen, events, ptys) = configured_overlay_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .overlay_tab_plugins
        .push(yinz_types::pane_tabs::PaneTabDescriptor {
            label: "Third".into(),
            location: "yinz:about".into(),
            configuration: Default::default(),
            initial_cwd: None,
        });
    let mut first = launch_overlay_with_completion(&mut screen, "yinz:agents");
    let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
    let mut second = launch_overlay_with_completion(&mut screen, "yinz:todos");
    let mut third = launch_overlay_with_completion(&mut screen, "yinz:about");
    screen
        .pane_tab_loaded(
            id,
            index,
            generation,
            Ok((PluginId::new(100), configured_overlay_run("yinz:agents"))),
        )
        .unwrap();
    assert_eq!(
        first.try_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(PluginId::new(100)))
    );
    assert!(second.try_recv().is_err());
    assert!(third.try_recv().is_err());
    let id = screen.tabs[&TabId::new(0)]
        .pane_tabs_infos()
        .next()
        .unwrap()
        .id;
    let loads: Vec<_> = events
        .try_iter()
        .filter_map(|(event, _)| match event {
            PluginInstruction::LoadPaneTab {
                index,
                generation,
                descriptor,
                ..
            } => Some((index, generation, descriptor)),
            _ => None,
        })
        .collect();
    assert_eq!(loads.len(), 2);
    for (index, generation, descriptor) in loads {
        screen
            .pane_tab_loaded(
                id,
                index,
                generation,
                Ok((
                    PluginId::new(100 + index as u32),
                    configured_overlay_run(&descriptor.location),
                )),
            )
            .unwrap();
    }
    assert_eq!(
        second.try_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(PluginId::new(101)))
    );
    assert_eq!(
        third.try_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(PluginId::new(102)))
    );
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .pane_tabs_info(id)
            .unwrap()
            .active,
        2
    );
}

#[test]
fn configured_overlay_launch_completion_reports_failure_and_closure() {
    for close in [false, true] {
        let (mut screen, events, ptys) = configured_overlay_screen();
        new_tab(&mut screen, 1, TabId::new(0));
        launch_configured_overlay(&mut screen, "yinz:agents", CLIENT_1);
        let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
        screen
            .pane_tab_loaded(
                id,
                index,
                generation,
                Ok((PluginId::new(100), configured_overlay_run("yinz:agents"))),
            )
            .unwrap();
        let mut completion = launch_overlay_with_completion(&mut screen, "yinz:todos");
        let (id, index, generation, _) = next_overlay_load(&mut screen, &ptys, &events);
        if close {
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .close_pane_tabs(id)
                .unwrap();
            screen.render().unwrap();
        } else {
            screen
                .pane_tab_loaded(id, index, generation, Err("load failed".into()))
                .unwrap();
        }
        let result = completion.try_recv().unwrap();
        assert_eq!(result.exit_status, Some(1));
        assert!(result.error_message.is_some());
        assert_eq!(result.affected_pane_id, None);
    }
}

#[test]
fn configured_overlay_initial_configuration_and_caller_metadata_survive_loading() {
    for location in ["custom", "file:overlay-custom.scm"] {
        let (mut screen, events, ptys) = configured_overlay_screen();
        new_tab(&mut screen, 1, TabId::new(0));
        let config = Config::from_kdl(r#"
            plugins { custom location="file:overlay-custom.scm" { width "50%"; unchanged "yes"; cwd "/plugin"; }; }
            overlay_tab_plugins { custom; "yinz:todos"; }
        "#, None).unwrap();
        screen
            .reconfigure(
                ScreenConfig::from_config(&config),
                Keybinds::default(),
                CLIENT_1,
            )
            .unwrap();
        let configuration = std::collections::BTreeMap::from([
            ("width".into(), "80%".into()),
            ("caller_cwd".into(), "/explicit-caller".into()),
            ("caller_terminal_pane_id".into(), "42".into()),
        ]);
        let request = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            location,
            &Some(configuration),
            None,
            None,
        )
        .unwrap();
        assert!(screen
            .launch_overlay_tab(&request, None, false, CLIENT_1, &mut None)
            .unwrap());
        let (id, index, generation, descriptor) = next_overlay_load(&mut screen, &ptys, &events);
        assert_eq!(descriptor.configuration["width"], "80%");
        assert_eq!(descriptor.configuration["unchanged"], "yes");
        assert_eq!(descriptor.configuration["caller_cwd"], "/explicit-caller");
        assert_eq!(descriptor.configuration["caller_terminal_pane_id"], "42");
        assert_eq!(descriptor.initial_cwd, Some("/plugin".into()));
        screen
            .pane_tab_loaded(id, index, generation, Ok((PluginId::new(100), request)))
            .unwrap();
        launch_configured_overlay(&mut screen, "custom", CLIENT_1);
        let id = screen.tabs[&TabId::new(0)]
            .pane_tabs_infos()
            .next()
            .unwrap()
            .id;
        assert_eq!(
            screen.tabs[&TabId::new(0)].pane_tabs_info(id).unwrap().tabs[index].descriptor,
            descriptor
        );
        launch_configured_overlay(&mut screen, "yinz:todos", CLIENT_1);
        let (_, _, _, companion) = next_overlay_load(&mut screen, &ptys, &events);
        assert_eq!(companion.configuration["caller_cwd"], "/focused");
        assert_eq!(companion.configuration["caller_terminal_pane_id"], "7");
        assert!(!companion.configuration.contains_key("width"));
    }
}

#[test]
fn configured_overlay_explicit_cwd_overrides_alias_default() {
    for location in ["custom", "file:overlay-custom.scm"] {
        for (cwd, expected) in [
            (None, "/plugin"),
            (Some("/override"), "/override"),
            (Some("relative"), "/focused/relative"),
        ] {
            let (mut screen, events, ptys) = configured_overlay_screen();
            new_tab(&mut screen, 1, TabId::new(0));
            let config = Config::from_kdl(
                r#"
                plugins { custom location="file:overlay-custom.scm" { cwd "/plugin"; }; }
                overlay_tab_plugins { custom; "yinz:todos"; }
                "#,
                None,
            )
            .unwrap();
            screen
                .reconfigure(
                    ScreenConfig::from_config(&config),
                    Keybinds::default(),
                    CLIENT_1,
                )
                .unwrap();
            let request = configured_overlay_run(location).with_initial_cwd(cwd.map(Into::into));
            screen
                .launch_or_focus_plugin(
                    request.clone(),
                    false,
                    false,
                    false,
                    false,
                    None,
                    false,
                    CLIENT_1,
                    None,
                    None,
                )
                .unwrap();
            let (id, index, generation, descriptor) =
                next_overlay_load(&mut screen, &ptys, &events);
            assert_eq!(descriptor.initial_cwd, Some(expected.into()));
            assert_eq!(descriptor.configuration["caller_cwd"], "/focused");
            screen
                .pane_tab_loaded(id, index, generation, Ok((PluginId::new(100), request)))
                .unwrap();
            launch_configured_overlay(&mut screen, "yinz:todos", CLIENT_1);
            let (_, _, _, sibling) = next_overlay_load(&mut screen, &ptys, &events);
            assert_eq!(sibling.initial_cwd, Some("/focused".into()));
        }
    }
}

#[test]
fn configured_overlay_coalesced_loads_keep_originating_clients() {
    for with_completions in [false, true] {
        for queue_before_cwd in [false, true] {
            let (mut screen, events, ptys) = configured_overlay_screen();
            new_tab(&mut screen, 1, TabId::new(0));
            new_tab(&mut screen, 2, TabId::new(1));
            let other = ClientId::new(2);
            screen.add_client(other).unwrap();
            screen
                .switch_active_tab(screen.tabs[&TabId::new(0)].position, None, true, CLIENT_1)
                .unwrap();
            screen
                .overlay_tab_plugins
                .push(yinz_types::pane_tabs::PaneTabDescriptor {
                    label: "Third".into(),
                    location: "yinz:about".into(),
                    configuration: Default::default(),
                    initial_cwd: None,
                });
            let mut completions = vec![];
            let mut pending_id = None;
            for (index, location, client) in [
                (0, "yinz:agents", CLIENT_1),
                (1, "yinz:todos", other),
                (2, "yinz:about", CLIENT_1),
                (2, "yinz:about", other),
            ] {
                let mut completion = if with_completions {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    completions.push((index, rx));
                    Some(crate::NotificationEnd::new(tx))
                } else {
                    None
                };
                assert!(screen
                    .launch_overlay_tab(
                        &configured_overlay_run(location),
                        None,
                        false,
                        client,
                        &mut completion
                    )
                    .unwrap());
                if index == 0 {
                    let (id, client_id) = ptys
                        .try_iter()
                        .find_map(|(event, _)| match event {
                            PtyInstruction::ResolveOverlayTabCwd { id, client_id, .. } => {
                                Some((id, client_id))
                            },
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(client_id, CLIENT_1);
                    pending_id = Some(id);
                    if !queue_before_cwd {
                        screen
                            .overlay_tab_cwd(
                                id,
                                Some("/client-a".into()),
                                Some("/client-a".into()),
                                Some(TerminalId::new(7)),
                            )
                            .unwrap();
                    }
                }
            }
            if queue_before_cwd {
                screen
                    .overlay_tab_cwd(
                        pending_id.unwrap(),
                        Some("/client-a".into()),
                        Some("/client-a".into()),
                        Some(TerminalId::new(7)),
                    )
                    .unwrap();
            }
            for expected in [vec![(0, CLIENT_1)], vec![(1, other), (2, CLIENT_1)]] {
                let loads: Vec<_> = events
                    .try_iter()
                    .filter_map(|(event, _)| match event {
                        PluginInstruction::LoadPaneTab {
                            id,
                            index,
                            generation,
                            descriptor,
                            client_id,
                            ..
                        } => Some((id, index, generation, descriptor, client_id)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    loads
                        .iter()
                        .map(|(_, index, _, _, client)| (*index, *client))
                        .collect::<Vec<_>>(),
                    expected
                );
                for (id, index, generation, descriptor, _) in loads {
                    assert_eq!(descriptor.configuration["caller_cwd"], "/client-a");
                    assert_eq!(descriptor.configuration["caller_terminal_pane_id"], "7");
                    screen
                        .pane_tab_loaded(
                            id,
                            index,
                            generation,
                            Ok((
                                PluginId::new(100 + index as u32),
                                configured_overlay_run(&descriptor.location),
                            )),
                        )
                        .unwrap();
                }
            }
            let tab = &screen.tabs[&TabId::new(1)];
            let info = tab
                .pane_tabs_infos()
                .find(|info| info.overlay_group)
                .unwrap();
            assert_eq!(info.active, 2);
            assert_eq!(tab.get_active_pane_id(other), Some(info.pane_id));
            assert!(!screen.tabs[&TabId::new(0)]
                .pane_tabs_infos()
                .any(|info| info.overlay_group));
            for (index, mut completion) in completions {
                assert_eq!(
                    completion.try_recv().unwrap().affected_pane_id,
                    Some(PaneId::Plugin(PluginId::new(100 + index)))
                );
            }
        }
    }
}

#[test]
fn move_pane_to_tab_group_carries_overlay_container_and_focus() {
    use yinz_types::multipane::*;
    let mut screen = create_non_mirrored_screen(Size {
        cols: 120,
        rows: 40,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let first = open_overlay_plugin(
        &mut screen,
        PluginId::new(7),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let second = open_overlay_plugin(
        &mut screen,
        PluginId::new(8),
        TabId::new(0),
        popup_coordinates(),
    )
    .1;
    let info = screen
        .multipane_command(
            MultipaneOperation::Create {
                children: [first, second],
                split: MultipaneSplit {
                    direction: MultipaneDirection::TopBottom,
                    ratio: 3500,
                },
                placement: OverlayPlacement::dismissible(Some(popup_coordinates())),
                focus: Some(1),
                min_cells: 0,
            },
            CLIENT_1,
        )
        .unwrap()
        .unwrap();

    screen.restore_tab_group(
        TabId::new(1),
        &TiledPaneLayout {
            tab_group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 1,
                name: "Other".into(),
            }),
            ..Default::default()
        },
    );
    screen
        .switch_active_tab(TabPosition::new(0), None, true, CLIENT_1)
        .unwrap();
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .focus_pane_with_id(second, false, false, CLIENT_1)
        .unwrap();
    screen
        .tab_group_action(
            yinz_types::input::actions::Action::MovePaneToNextTabGroup,
            CLIENT_1,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT_1], TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT_1), Some(second));
    let destination = &screen.tabs[&TabId::new(1)];
    assert_eq!(destination.multipane_info(second).unwrap().id, info.id);
    for pane in [first, second] {
        assert!(destination.pane_is_overlay(pane));
        assert!(!screen.tabs[&TabId::new(0)].has_pane_with_pid(&pane));
    }
}
