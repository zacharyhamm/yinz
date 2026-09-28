use super::*;
use crate::panes::OverlayCarry;
use std::time::{Duration, Instant};
use yinz_types::animation::FRAME_PERIOD;
use yinz_types::data::OverlayKind;
use yinz_types::internal_ids::{PluginId, TabId};

fn panel_half(
    screen: &mut Screen,
    id: u32,
    name: &str,
    x: usize,
    awaiting: bool,
) -> (RunPluginOrAlias, PaneId) {
    let run = RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("yinz:{name}")).unwrap(),
    );
    let (tx, _rx) = channels::unbounded();
    let pane = PaneFactory::plugin(
        PluginId::new(id),
        Default::default(),
        SenderWithContext::new(tx),
        "panel─title".to_owned(),
        "panel─title".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        vec![ClientId::new(1)],
        screen.style,
        Some(Run::Plugin(run.clone())),
        false,
        true,
        true,
    );
    let pane_id = PaneId::Plugin(PluginId::new(id));
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.add_overlay_pane(
        pane,
        pane_id,
        OverlayKind::Dismissible,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(x)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(35)),
            height: Some(PercentOrFixed::Fixed(10)),
            ..Default::default()
        },
        OverlayCarry {
            awaiting_placement: awaiting,
            ..Default::default()
        },
        false,
        Some(ClientId::new(1)),
    )
    .unwrap();
    tab.get_pane_with_id_mut(pane_id)
        .unwrap()
        .handle_plugin_bytes(ClientId::new(1), b"\x1b[HCONTENTS".to_vec());
    (run, pane_id)
}

#[test]
fn opening_ripple_placement_cache_and_clock_shutdown() {
    let size = Size {
        cols: 100,
        rows: 24,
    };
    let (mut screen, jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    let (_, pane_id) = panel_half(&mut screen, 7, "hocus", 3, true);
    let (server_tx, server_rx) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    let mut grid = grid_for_render_snapshot(size);
    let start = Instant::now();
    screen.pinned_now = Some(start);
    let hidden = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
    assert!(!hidden.contains("CONTENTS"));
    assert!(!screen.animations_are_live(start));

    // Showing a permission prompt is not the plugin's first placement.
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .request_plugin_permissions(
            PluginId::new(7),
            Some(yinz_types::data::PluginPermission::new(
                "hocus".into(),
                vec![yinz_types::data::PermissionType::ReadApplicationState],
            )),
        );
    let prompt = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
    assert!(prompt.contains("Allow?"));
    assert!(!screen.animations_are_live(start));
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.request_plugin_permissions(PluginId::new(7), None);
    tab.get_pane_with_id_mut(pane_id)
        .unwrap()
        .handle_plugin_bytes(ClientId::new(1), b"\x1b[2J\x1b[HCONTENTS".to_vec());

    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    let coordinates = tab
        .overlay_placement_for(pane_id)
        .unwrap()
        .coordinates
        .unwrap();
    tab.change_overlay_coordinates_batch(vec![(pane_id, coordinates)], false)
        .unwrap();
    let initial = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
    assert!(initial.contains("CONTENTS"), "{initial}");
    assert!(initial.contains("panel─title"), "{initial}");
    assert_eq!(screen.animation_clock_period(start), Some(FRAME_PERIOD));
    assert!(
        drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(FRAME_PERIOD))
    );
    let original_geometry = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(pane_id)
        .unwrap()
        .position_and_size();

    screen.pinned_now = Some(start + Duration::from_millis(160));
    let middle = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
    assert_ne!(initial, middle, "the cached frame must be repainted");
    assert!(middle.contains('∿') || middle.contains('∽'), "{middle}");
    assert!(middle.contains("panel─title") && middle.contains("CONTENTS"));
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(pane_id)
            .unwrap()
            .position_and_size(),
        original_geometry
    );

    let end = start + Duration::from_millis(450);
    screen.on_animation_tick(end).unwrap();
    assert!(
        !drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock),
        "the straight border has not been painted yet"
    );
    screen.pinned_now = Some(end);
    let settled = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
    assert_eq!(settled, initial);
    assert!(!screen.animations_are_live(end));
    assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
}

#[test]
fn opening_ripple_link_reveal_move_resize_and_disable() {
    let size = Size {
        cols: 100,
        rows: 24,
    };
    let (mut screen, _jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    let (run, left) = panel_half(&mut screen, 7, "hocus", 3, false);
    let start = Instant::now();
    screen.pinned_now = Some(start);
    screen.render_to_clients().unwrap();
    let (_, right) = panel_half(&mut screen, 8, "hocus-agents", 38, false);
    let (_, unrelated) = panel_half(&mut screen, 9, "file-finder", 73, false);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .create_multipane([left, right], panel_split(), 0, panel_placement())
        .unwrap();
    let later = start + Duration::from_millis(120);
    screen.pinned_now = Some(later);
    screen.render_to_clients().unwrap();
    for id in [left, right] {
        assert_eq!(
            screen.tabs[&TabId::new(0)]
                .overlay_carry_for(id)
                .opening_started,
            Some(later)
        );
    }
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .overlay_carry_for(unrelated)
            .opening_started,
        None
    );

    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    assert!(screen
        .focus_plugin_pane(&run, true, true, false, ClientId::new(1), &mut None)
        .unwrap());
    let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    tab.resize_whole_tab_to(Size {
        cols: 110,
        rows: 26,
    })
    .unwrap();
    tab.sample_overlay_ripples(later, true);
    for id in [left, right] {
        assert_eq!(
            tab.overlay_carry_for(id).opening_started,
            Some(later),
            "moving, focusing and resizing do not replay"
        );
    }
    assert!(tab.hide_overlay_pane(left));
    assert!(!tab.overlay_ripple_is_live());
    assert!(tab.show_overlay_pane(right));
    tab.sample_overlay_ripples(later, true);
    for id in [left, right] {
        assert_eq!(tab.overlay_carry_for(id).opening_started, Some(later));
    }

    screen.set_pane_animations(false);
    assert_eq!(
        screen.tabs[&TabId::new(1)]
            .overlay_carry_for(left)
            .opening_started,
        None,
        "live reconfiguration settles immediately"
    );
    assert!(
        screen.animations_are_live(later),
        "the clock stays until straight borders are painted"
    );
    screen.render_to_clients().unwrap();
    assert!(!screen.animations_are_live(later));
    screen.set_pane_animations(true);
    screen.render_to_clients().unwrap();
    assert!(
        !screen.animations_are_live(later),
        "enabling effects does not replay a visible panel"
    );
    let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    tab.hide_overlay_pane(left);
    tab.show_overlay_pane(left);
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.sample_overlay_ripples(later, true);
    assert!(
        tab.overlay_ripple_is_live(),
        "regular overlays still have full borders in the default layout"
    );
    tab.set_pane_frames(PaneFrameStyle::Full);
    tab.sample_overlay_ripples(later + Duration::from_millis(10), true);
    assert_eq!(
        tab.overlay_carry_for(left).opening_started,
        Some(later),
        "changing frame style does not replay"
    );
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.toggle_pane_fullscreen(left);
    tab.sample_overlay_ripples(later + Duration::from_millis(20), true);
    assert!(
        tab.overlay_ripple_is_live(),
        "regular fullscreen keeps the borders and their effect under title-only settings"
    );
    tab.toggle_pane_no_ui_fullscreen(left);
    tab.sample_overlay_ripples(later + Duration::from_millis(30), true);
    assert!(
        !tab.overlay_ripple_is_live(),
        "no-UI fullscreen removes the borders and settles the effect"
    );
    tab.toggle_pane_no_ui_fullscreen(left);
    tab.sample_overlay_ripples(later + Duration::from_millis(40), true);
    assert!(
        !tab.overlay_ripple_is_live(),
        "restoring borders does not replay"
    );
}

#[test]
fn opening_ripple_fullscreen_bounds_and_restore() {
    for linked in [false, true] {
        let size = Size {
            cols: 100,
            rows: 24,
        };
        let (mut screen, _jobs) = screen_with_background_jobs(size);
        new_tab(&mut screen, 1, TabId::new(0));
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .set_pane_frames(PaneFrameStyle::Full);
        let (_, left) = panel_half(&mut screen, 7, "hocus", 3, false);
        let mut panes = vec![left];
        if linked {
            let (_, right) = panel_half(&mut screen, 8, "hocus-agents", 38, false);
            panes.push(right);
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .create_multipane([panes[0], panes[1]], panel_split(), 0, panel_placement())
                .unwrap();
        }
        let original_geoms: Vec<_> = panes
            .iter()
            .map(|id| {
                screen.tabs[&TabId::new(0)]
                    .get_pane_with_id(*id)
                    .unwrap()
                    .current_geom()
            })
            .collect();
        let (server_tx, server_rx) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
        let mut grid = grid_for_render_snapshot(size);
        let start = Instant::now();
        screen.pinned_now = Some(start);
        render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .toggle_pane_fullscreen(left);

        // The wave crosses both halves of the displayed 100-column frame.
        // Sampling the left half also catches a partner using its unexpanded bounds.
        for (ms, column) in [(160, 35), (360, 85)] {
            screen.pinned_now = Some(start + Duration::from_millis(ms));
            let frame = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
            let bottom = frame.lines().last().unwrap().split_once(": ").unwrap().1;
            assert!(
                matches!(bottom.chars().nth(column), Some('∿' | '∽')),
                "linked={linked}, elapsed={ms}:\n{frame}"
            );
        }

        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .toggle_pane_fullscreen(left);
        let restored = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
        assert!(
            restored.contains('∿') || restored.contains('∽'),
            "{restored}"
        );
        let tab = &screen.tabs[&TabId::new(0)];
        for (id, geom) in panes.iter().zip(original_geoms) {
            assert_eq!(tab.get_pane_with_id(*id).unwrap().current_geom(), geom);
            assert_eq!(tab.overlay_carry_for(*id).opening_started, Some(start));
        }
        let end = start + Duration::from_millis(450);
        screen.pinned_now = Some(end);
        let settled = render_frame_into(&mut screen, &server_rx, ClientId::new(1), &mut grid);
        assert!(
            !settled.contains('∿') && !settled.contains('∽'),
            "{settled}"
        );
        assert!(!screen.animations_are_live(end));
    }
}

#[test]
fn opening_ripple_alone_and_reopened_beside_a_settled_half() {
    let (mut screen, _jobs) = screen_with_background_jobs(Size {
        cols: 100,
        rows: 24,
    });
    new_tab(&mut screen, 1, TabId::new(0));
    let (_, agents) = panel_half(&mut screen, 7, "hocus-agents", 3, false);
    let start = Instant::now();
    screen.pinned_now = Some(start);
    screen.render_to_clients().unwrap();
    assert!(screen.animations_are_live(start), "agents can open alone");
    let (_, conversation) = panel_half(&mut screen, 8, "hocus", 38, false);
    let later = start + Duration::from_secs(1);
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.create_multipane([agents, conversation], panel_split(), 0, panel_placement())
        .unwrap();
    tab.sample_overlay_ripples(later, true);
    assert_eq!(
        tab.overlay_carry_for(conversation).opening_started,
        Some(later),
        "no live ripple to join"
    );
    tab.sample_overlay_ripples(later + Duration::from_secs(1), true);
    tab.hide_overlay_pane(agents);
    tab.show_overlay_pane(agents);
    tab.sample_overlay_ripples(later + Duration::from_secs(1), true);
    assert!(
        tab.overlay_ripple_is_live(),
        "a reopened half gets a fresh ripple"
    );
    tab.set_pane_borderless(&agents, true).unwrap();
    tab.sample_overlay_ripples(later + Duration::from_secs(1), true);
    assert!(!tab.overlay_ripple_is_live());
    tab.set_pane_borderless(&agents, false).unwrap();
    tab.sample_overlay_ripples(later + Duration::from_secs(1), true);
    assert!(
        !tab.overlay_ripple_is_live(),
        "restoring borders does not replay"
    );
}

fn panel_split() -> yinz_types::multipane::MultipaneSplit {
    yinz_types::multipane::MultipaneSplit {
        direction: yinz_types::multipane::MultipaneDirection::LeftRight,
        ratio: 5000,
    }
}
fn panel_placement() -> yinz_types::data::OverlayPlacement {
    yinz_types::data::OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(3)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Fixed(70)),
        height: Some(PercentOrFixed::Fixed(10)),
        ..Default::default()
    }))
}
