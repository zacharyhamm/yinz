use super::*;
use crate::panes::terminal_character::AnsiCode;
use crate::screen::apply_pane_agent_report;
use std::time::{Duration, Instant};
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::animation::FRAME_PERIOD;
use yinz_types::data::{PaletteColor, WarpHiddenPane, WarpRelationships, WarpStackList};
use yinz_types::geometry::Direction;

const CLIENT: ClientId = ClientId::new(1);
const TAB: TabId = TabId::new(0);
const WAITING: PaneId = PaneId::Terminal(TerminalId::new(1));
const FOCUSED: PaneId = PaneId::Terminal(TerminalId::new(2));
const PERIOD: Duration = Duration::from_millis(50);
const SIZE: Size = Size {
    cols: 100,
    rows: 24,
};

fn waiting() -> AgentInfo {
    AgentInfo {
        kind: "claude".into(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: Some("approval".into()),
        wait_needs_a_person: true,
        cwd: None,
        session_id: None,
        run_id: None,
    }
}

fn waiting_since(screen: &Screen) -> Option<Instant> {
    screen.tabs[&TAB]
        .get_pane_with_id(WAITING)
        .unwrap()
        .waiting_pulse_started_at()
}

fn pair(
    floating: bool,
    stacked: bool,
    list: bool,
    style: PaneFrameStyle,
) -> (Screen, Receiver<(BackgroundJob, ErrorContext)>) {
    let (mut screen, jobs) = screen_with_background_jobs(SIZE);
    new_tab(&mut screen, if floating { 0 } else { 1 }, TAB);
    *screen.stacked_pane_list.borrow_mut() = list;
    screen.terminal_emulator_colors.borrow_mut().fg = PaletteColor::Rgb((100, 150, 200));
    let tab = screen.tabs.get_mut(&TAB).unwrap();
    tab.set_pane_frames(style);
    for id in if floating { 1..=2 } else { 2..=2 } {
        let placement = if floating {
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(if id == 1 { 3 } else { 53 })),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Fixed(40)),
                height: Some(PercentOrFixed::Fixed(18)),
                ..Default::default()
            }))
        } else if stacked {
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            }
        } else {
            NewPanePlacement::default()
        };
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(id)),
            None,
            None,
            false,
            true,
            placement,
            Some(CLIENT),
            None,
        )
        .unwrap();
    }
    if style == PaneFrameStyle::None && !stacked {
        // A shared boundary with the focused pane keeps focus precedence. Leave another
        // unfocused neighbor so the waiting pane also has a boundary of its own to animate.
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(3)),
            None,
            None,
            false,
            false,
            NewPanePlacement::default(),
            Some(CLIENT),
            None,
        )
        .unwrap();
    }
    screen.render_to_clients().unwrap();
    assert_eq!(screen.tabs[&TAB].get_active_pane_id(CLIENT), Some(FOCUSED));
    drain_background_jobs(&jobs);
    (screen, jobs)
}

#[test]
fn waiting_pulse_repaints_idle_frames_boundaries_and_both_stack_headers() {
    for (floating, stacked, list, style) in [
        (false, false, false, PaneFrameStyle::Full),
        (false, false, false, PaneFrameStyle::None),
        (true, false, false, PaneFrameStyle::Full),
        (false, true, false, PaneFrameStyle::Full),
        (false, true, true, PaneFrameStyle::Full),
        (false, true, false, PaneFrameStyle::None),
        (false, true, true, PaneFrameStyle::None),
    ] {
        let (mut screen, jobs) = pair(floating, stacked, list, style);
        apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
        let start = waiting_since(&screen).unwrap();
        let (server_tx, server_rx) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
        let mut grid = grid_for_render_snapshot(SIZE);
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        screen.pinned_now = Some(start);
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));
        let base = grid.as_character_lines();

        let peak = start + Duration::from_millis(900);
        screen.pinned_now = Some(peak);
        screen.on_animation_tick(peak).unwrap();
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::RenderToClients));
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert!(
            grid.as_character_lines().iter().flatten().any(|cell| {
                matches!(cell.character, '─' | '│' | '┌' | '└' | '┐' | '┘')
                    && cell.styles.foreground == Some(AnsiCode::RgbCode((127, 142, 157)))
            }),
            "floating={floating}, stacked={stacked}, list={list}, style={style:?}"
        );

        screen.pinned_now = Some(start + Duration::from_millis(1800));
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert!(
            grid.as_character_lines() == base,
            "a full cycle returns to the base frame: floating={floating}, stacked={stacked}, list={list}, style={style:?}"
        );
        assert!(screen.tabs[&TAB]
            .get_pane_with_id(WAITING)
            .unwrap()
            .stack_list_entry_label(false)
            .contains('🔔'));
    }
}

#[test]
fn waiting_pulse_shared_boundary_loses_to_focus_and_stops_after_removing_frames() {
    for direction in [Direction::Right, Direction::Down] {
        for waiting_pane in [WAITING, FOCUSED] {
            for initial_style in [
                PaneFrameStyle::None,
                PaneFrameStyle::Full,
                PaneFrameStyle::Titles,
            ] {
                let (mut screen, jobs) = screen_with_background_jobs(SIZE);
                new_tab(&mut screen, 1, TAB);
                let tab = screen.tabs.get_mut(&TAB).unwrap();
                tab.set_pane_frames(initial_style);
                tab.new_pane(
                    FOCUSED,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: Some(direction),
                        borderless: None,
                    },
                    Some(CLIENT),
                    None,
                )
                .unwrap();
                let focused = if waiting_pane == WAITING {
                    FOCUSED
                } else {
                    WAITING
                };
                tab.focus_pane_with_id(focused, false, false, CLIENT)
                    .unwrap();
                screen.render_to_clients().unwrap();
                drain_background_jobs(&jobs);
                apply_pane_agent_report(&mut screen, waiting_pane, Some(waiting())).unwrap();
                let start = screen.tabs[&TAB]
                    .get_pane_with_id(waiting_pane)
                    .unwrap()
                    .waiting_pulse_started_at()
                    .unwrap();
                let (server_tx, server_rx) = channels::unbounded();
                screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
                let mut grid = grid_for_render_snapshot(SIZE);
                screen.tabs.get_mut(&TAB).unwrap().set_force_render();
                screen.pinned_now = Some(start);
                render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
                let base = grid.as_character_lines();
                screen.pinned_now = Some(start + Duration::from_millis(900));
                render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
                assert_eq!(
                    screen.animation_clock_period(start),
                    (initial_style != PaneFrameStyle::None).then_some(PERIOD)
                );
                if initial_style != PaneFrameStyle::None {
                    let pane = screen.tabs[&TAB].get_pane_with_id(waiting_pane).unwrap();
                    let lines = grid.as_character_lines();
                    assert!(lines[pane.y()]
                        .iter()
                        .zip(&base[pane.y()])
                        .skip(pane.x())
                        .take(pane.cols())
                        .any(|(peak, base)| peak.styles.foreground != base.styles.foreground));
                    assert!(screen.tabs[&TAB]
                        .get_pane_with_id(waiting_pane)
                        .unwrap()
                        .waiting_pulse()
                        .is_some());
                    drain_background_jobs(&jobs);
                    screen
                        .tabs
                        .get_mut(&TAB)
                        .unwrap()
                        .set_pane_frames(PaneFrameStyle::None);
                    screen.render_to_clients().unwrap();
                    assert!(
                        drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock)
                    );
                } else {
                    assert!(!drain_background_jobs(&jobs)
                        .contains(&BackgroundJob::StartAnimationClock(PERIOD)));
                }
                assert_eq!(screen.animation_clock_period(start), None);
                let pane = screen.tabs[&TAB].get_pane_with_id(waiting_pane).unwrap();
                assert_eq!(pane.waiting_pulse(), None);
                assert_eq!(pane.waiting_pulse_started_at(), Some(start));
            }
        }
    }
}

#[test]
fn waiting_pulse_overrides_stop_the_clock_and_clearing_resumes_the_episode() {
    for (floating, stacked, style) in [
        (false, false, PaneFrameStyle::Full),
        (true, false, PaneFrameStyle::Full),
        (false, true, PaneFrameStyle::Full),
        (false, false, PaneFrameStyle::Titles),
    ] {
        let (mut screen, jobs) = pair(floating, stacked, false, style);
        apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
        let start = waiting_since(&screen).unwrap();
        let (server_tx, server_rx) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
        let mut grid = grid_for_render_snapshot(SIZE);
        screen.pinned_now = Some(start + Duration::from_millis(900));
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        let peak = grid.as_character_lines();
        let pane = screen.tabs[&TAB].get_pane_with_id(WAITING).unwrap();
        let (x, y) = (pane.x(), pane.y());
        let pulse_color = AnsiCode::RgbCode((127, 142, 157));
        let col = (x..x + pane.cols())
            .find(|col| peak[y][*col].styles.foreground == Some(pulse_color))
            .expect("waiting frame has a pulsing cell");
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));

        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .add_highlight_pane_frame_color_override(WAITING, None, None);
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert_eq!(
            grid.as_character_lines()[y][col].styles.foreground,
            Some(AnsiCode::from(
                screen.style.colors.frame_highlight.emphasis_0
            ))
        );
        assert_eq!(screen.animation_clock_period(start), None);
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(WAITING)
                .unwrap()
                .waiting_pulse(),
            None
        );
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
        let overridden = grid.as_character_lines();
        screen.pinned_now = Some(start);
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert!(grid.as_character_lines() == overridden);

        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .clear_pane_frame_color_override(WAITING, None);
        screen.pinned_now = Some(start + Duration::from_millis(900));
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        assert!(grid.as_character_lines() == peak);
        assert_eq!(waiting_since(&screen), Some(start));
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));
    }
}

#[test]
fn waiting_pulse_overrides_respect_other_viewers_and_watchers() {
    let (mut screen, jobs) = pair(false, false, false, PaneFrameStyle::Full);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .focus_pane_with_id(FOCUSED, false, false, other)
        .unwrap();
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    let start = waiting_since(&screen).unwrap();
    screen.pinned_now = Some(start + Duration::from_millis(900));
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .add_highlight_pane_frame_color_override(WAITING, None, Some(CLIENT));
    let (server_tx, server_rx) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    let mut grid = grid_for_render_snapshot(SIZE);
    let pane = screen.tabs[&TAB].get_pane_with_id(WAITING).unwrap();
    let (x, y) = (pane.x(), pane.y());
    for (client, expected) in [
        (
            CLIENT,
            AnsiCode::from(screen.style.colors.frame_highlight.emphasis_0),
        ),
        (other, AnsiCode::RgbCode((127, 142, 157))),
    ] {
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        render_frame_into(&mut screen, &server_rx, client, &mut grid);
        assert_eq!(
            grid.as_character_lines()[y][x].styles.foreground,
            Some(expected)
        );
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    }
    drain_background_jobs(&jobs);
    let watcher = ClientId::new(3);
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, SIZE);
    screen.set_followed_client(CLIENT).unwrap();
    screen.remove_client(other).unwrap();
    screen.remove_client(CLIENT).unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));

    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .clear_pane_frame_color_override(WAITING, Some(CLIENT));
    screen.tabs.get_mut(&TAB).unwrap().set_force_render();
    render_frame_into(&mut screen, &server_rx, watcher, &mut grid);
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    assert_eq!(
        grid.as_character_lines()[y][x].styles.foreground,
        Some(AnsiCode::RgbCode((127, 142, 157)))
    );
    assert_eq!(waiting_since(&screen), Some(start));
    assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));
}

#[test]
fn waiting_pulse_grouped_frames_boundaries_and_list_borders_keep_precedence_over_overrides() {
    for (floating, stacked, list, style, waiting_pane) in [
        (false, false, false, PaneFrameStyle::Full, WAITING),
        (true, false, false, PaneFrameStyle::Full, WAITING),
        (false, true, false, PaneFrameStyle::Full, WAITING),
        (false, false, false, PaneFrameStyle::Titles, WAITING),
        (false, false, false, PaneFrameStyle::None, WAITING),
        (false, true, true, PaneFrameStyle::Full, WAITING),
        (false, true, true, PaneFrameStyle::Full, FOCUSED),
        (false, true, true, PaneFrameStyle::None, WAITING),
        (false, true, true, PaneFrameStyle::None, FOCUSED),
    ] {
        let (mut screen, jobs) = pair(floating, stacked, list, style);
        if list {
            screen
                .tabs
                .get_mut(&TAB)
                .unwrap()
                .new_pane(
                    PaneId::Terminal(TerminalId::new(3)),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::default(),
                    Some(CLIENT),
                    None,
                )
                .unwrap();
        }
        apply_pane_agent_report(&mut screen, waiting_pane, Some(waiting())).unwrap();
        let start = screen.tabs[&TAB]
            .get_pane_with_id(waiting_pane)
            .unwrap()
            .waiting_pulse_started_at()
            .unwrap();
        screen.pinned_now = Some(start + Duration::from_millis(900));
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .add_highlight_pane_frame_color_override(waiting_pane, None, None);
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), None);
        drain_background_jobs(&jobs);
        screen
            .current_pane_group
            .borrow_mut()
            .add_pane_id_to_group(waiting_pane, SIZE, &CLIENT);
        let (server_tx, server_rx) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
        let mut grid = grid_for_render_snapshot(SIZE);
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
        let expected = AnsiCode::from(yinz_types::animation::blend(
            screen.style.colors.frame_highlight.emphasis_0,
            screen.style.colors.text_unselected.base,
            0.7,
        ));
        // Titles-only frames carry the pulse on title text rather than border strokes.
        assert!(grid
            .as_character_lines()
            .iter()
            .flatten()
            .any(|cell| cell.styles.foreground == Some(expected)),
            "floating={floating}, stacked={stacked}, list={list}, style={style:?}, pane={waiting_pane:?}, expected={expected:?}");
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(waiting_pane)
                .unwrap()
                .waiting_pulse_started_at(),
            Some(start)
        );
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));
    }
}

#[test]
fn waiting_pulse_wins_shared_idle_boundary_in_both_pane_id_orders() {
    for ids in [[1, 2, 3], [3, 2, 1]] {
        let [focused, idle, waiting_pane] = ids.map(|id| PaneId::Terminal(TerminalId::new(id)));
        let (mut screen, _jobs) = screen_with_background_jobs(SIZE);
        let mut unselected = screen.style.colors.text_unselected;
        unselected.base = PaletteColor::Rgb((100, 150, 200));
        screen.style.colors.frame_unselected = Some(unselected);
        new_tab(&mut screen, ids[0], TAB);
        let tab = screen.tabs.get_mut(&TAB).unwrap();
        tab.set_pane_frames(PaneFrameStyle::None);
        for pane in [idle, waiting_pane] {
            tab.new_pane(
                pane,
                None,
                None,
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: Some(Direction::Right),
                    borderless: None,
                },
                Some(CLIENT),
                None,
            )
            .unwrap();
        }
        tab.focus_pane_with_id(focused, false, false, CLIENT)
            .unwrap();
        let focused_boundary = tab.get_pane_with_id(idle).unwrap().x() - 1;
        let waiting_boundary = tab.get_pane_with_id(waiting_pane).unwrap().x() - 1;
        assert!(focused_boundary < waiting_boundary);
        apply_pane_agent_report(&mut screen, waiting_pane, Some(waiting())).unwrap();
        let start = screen.tabs[&TAB]
            .get_pane_with_id(waiting_pane)
            .unwrap()
            .waiting_pulse_started_at()
            .unwrap();
        let (server_tx, server_rx) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
        let mut grid = grid_for_render_snapshot(SIZE);
        screen.tabs.get_mut(&TAB).unwrap().set_force_render();
        for (elapsed, expected) in [
            (0, (100, 150, 200)),
            (900, (127, 142, 157)),
            (1800, (100, 150, 200)),
        ] {
            screen.pinned_now = Some(start + Duration::from_millis(elapsed));
            render_frame_into(&mut screen, &server_rx, CLIENT, &mut grid);
            assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
            let lines = grid.as_character_lines();
            for row in &lines {
                assert_eq!(row[waiting_boundary].character, '│');
                assert_eq!(
                    row[waiting_boundary].styles.foreground,
                    Some(AnsiCode::RgbCode(expected)),
                    "pane IDs {ids:?}, elapsed {elapsed}ms"
                );
                assert_eq!(row[focused_boundary].character, '│');
                assert_eq!(
                    row[focused_boundary].styles.foreground,
                    Some(AnsiCode::from(screen.style.colors.frame_selected.base)),
                    "focus stays dominant: pane IDs {ids:?}, elapsed {elapsed}ms"
                );
            }
        }
        // Focus the only neighbor: every shared cell now belongs to its focused border.
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(idle, false, false, CLIENT)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), None);
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(waiting_pane)
                .unwrap()
                .waiting_pulse(),
            None
        );
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(waiting_pane)
                .unwrap()
                .waiting_pulse_started_at(),
            Some(start)
        );
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(focused, false, false, CLIENT)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));

        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(idle, false, false, other)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(
            screen.animation_clock_period(start),
            Some(PERIOD),
            "the first client still sees the pulse"
        );
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(idle, false, false, CLIENT)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(
            screen.animation_clock_period(start),
            None,
            "both clients suppress the boundary"
        );
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(focused, false, false, other)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(
            screen.animation_clock_period(start),
            Some(PERIOD),
            "the second client now sees the pulse"
        );
        let watcher = ClientId::new(3);
        screen.add_watcher_client(watcher).unwrap();
        screen.set_followed_client(other).unwrap();
        screen.remove_client(CLIENT).unwrap();
        screen.remove_client(other).unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(
            screen.animation_clock_period(start),
            Some(PERIOD),
            "watchers still see the shared boundary after clients leave"
        );
        screen.remove_watcher_client(watcher);
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), None);
    }
}

#[test]
fn waiting_pulse_survives_focus_for_any_client_until_waiting_clears() {
    let (mut screen, jobs) = pair(false, false, false, PaneFrameStyle::Full);
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    screen.render_to_clients().unwrap();
    let start = waiting_since(&screen).unwrap();
    for pane in [WAITING, FOCUSED] {
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(pane, false, false, CLIENT)
            .unwrap();
        screen.render().unwrap();
    }
    assert_eq!(
        waiting_since(&screen),
        Some(start),
        "focus and leave before a frame preserves the waiting episode"
    );
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(waiting_since(&screen), Some(start));
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));

    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .focus_pane_with_id(WAITING, false, false, other)
        .unwrap();
    // A new wait also pulses while already selected by another client.
    apply_pane_agent_report(&mut screen, WAITING, None).unwrap();
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    let start = waiting_since(&screen).unwrap();
    screen.pinned_now = Some(start + Duration::from_millis(900));
    screen.render_to_clients().unwrap();
    assert_eq!(waiting_since(&screen), Some(start));
    let pane = screen.tabs[&TAB].get_pane_with_id(WAITING).unwrap();
    assert_eq!(pane.waiting_pulse().unwrap(), 0.7);
    assert!(pane.stack_list_entry_label(false).contains('🔔'));
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    drain_background_jobs(&jobs);

    apply_pane_agent_report(
        &mut screen,
        WAITING,
        Some(AgentInfo {
            state: AgentState::Busy,
            ..waiting()
        }),
    )
    .unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(waiting_since(&screen), None);
    let pane = screen.tabs[&TAB].get_pane_with_id(WAITING).unwrap();
    assert_eq!(pane.waiting_pulse(), None);
    assert!(!pane.stack_list_entry_label(false).contains('🔔'));
    assert_eq!(screen.animation_clock_period(start), None);
    assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
}

#[test]
fn waiting_pulse_clock_stops_for_hidden_borders_and_disabled_animations() {
    let (mut screen, jobs) = pair(false, false, false, PaneFrameStyle::Full);
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    let start = waiting_since(&screen).unwrap();
    screen.pinned_now = Some(start + Duration::from_millis(900));
    screen.render_to_clients().unwrap();
    screen.set_pane_animations(false);
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    assert_eq!(
        screen.tabs[&TAB]
            .get_pane_with_id(WAITING)
            .unwrap()
            .waiting_pulse(),
        None
    );
    assert_eq!(waiting_since(&screen), Some(start));
    assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
    screen.set_pane_animations(true);
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .toggle_pane_fullscreen(FOCUSED);
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .toggle_pane_fullscreen(FOCUSED);
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .set_pane_borderless(&WAITING, true)
        .unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .set_pane_borderless(&WAITING, false)
        .unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    // A fullscreen floating pane also hides every tiled border behind it.
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .new_pane(
            PaneId::Terminal(TerminalId::new(4)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(CLIENT),
            None,
        )
        .unwrap();
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .toggle_pane_fullscreen(PaneId::Terminal(TerminalId::new(4)));
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    screen.tabs.get_mut(&TAB).unwrap().hide_floating_panes();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));

    // Faster effects keep their cadence while a slow waiting pulse is active.
    screen.fading_frame_panes.insert(WAITING);
    assert_eq!(screen.animation_clock_period(start), Some(FRAME_PERIOD));
    screen.fading_frame_panes.clear();
    new_tab(&mut screen, 3, TabId::new(1));
    screen.render_to_clients().unwrap();
    assert_eq!(
        screen.animation_clock_period(start),
        None,
        "an inactive tab retains attention without ticks"
    );
    assert_eq!(waiting_since(&screen), Some(start));

    let (mut screen, _) = pair(true, false, false, PaneFrameStyle::Full);
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
    screen.tabs.get_mut(&TAB).unwrap().hide_floating_panes();
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
    assert!(waiting_since(&screen).is_some());
}

#[test]
fn waiting_pulse_counts_exterior_shared_boundaries_when_the_pane_is_covered() {
    for direction in [Direction::Right, Direction::Down] {
        let (mut screen, jobs) = screen_with_background_jobs(SIZE);
        new_tab(&mut screen, 2, TAB);
        let tab = screen.tabs.get_mut(&TAB).unwrap();
        tab.set_pane_frames(PaneFrameStyle::None);
        tab.new_pane(
            WAITING,
            None,
            None,
            false,
            false,
            NewPanePlacement::Tiled {
                direction: Some(direction),
                borderless: None,
            },
            Some(CLIENT),
            None,
        )
        .unwrap();
        let geom = tab.get_pane_with_id(WAITING).unwrap().current_geom();
        let cover = PaneId::Terminal(TerminalId::new(4));
        let mut coordinates = FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(geom.x)),
            y: Some(PercentOrFixed::Fixed(geom.y)),
            width: Some(PercentOrFixed::Fixed(geom.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(geom.rows.as_usize())),
            ..Default::default()
        };
        tab.new_pane(
            cover,
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(coordinates.clone())),
            Some(CLIENT),
            None,
        )
        .unwrap();
        apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
        let start = waiting_since(&screen).unwrap();
        screen.pinned_now = Some(start + Duration::from_millis(900));
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        assert!(screen.tabs[&TAB]
            .get_pane_with_id(WAITING)
            .unwrap()
            .waiting_pulse()
            .is_some());
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));

        // Extend the cover over the only exposed boundary, just outside the pane rectangle.
        if direction == Direction::Right {
            assert!(geom.x > 0);
            coordinates.x = Some(PercentOrFixed::Fixed(geom.x - 1));
            coordinates.width = Some(PercentOrFixed::Fixed(geom.cols.as_usize() + 1));
        } else {
            assert!(geom.y > 0);
            coordinates.y = Some(PercentOrFixed::Fixed(geom.y - 1));
            coordinates.height = Some(PercentOrFixed::Fixed(geom.rows.as_usize() + 1));
        }
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .change_floating_pane_coordinates(&cover, coordinates)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), None);
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(WAITING)
                .unwrap()
                .waiting_pulse(),
            None
        );
        assert_eq!(waiting_since(&screen), Some(start));
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
    }
}

#[test]
fn waiting_pulse_counts_only_the_hidden_stack_members_header() {
    for floating in [false, true] {
        let (mut screen, jobs) = pair(floating, false, false, PaneFrameStyle::Full);
        *screen.stacked_pane_list.borrow_mut() = true;
        let tab = screen.tabs.get_mut(&TAB).unwrap();
        tab.restore_warp_relationships(
            &[WAITING, FOCUSED],
            &WarpRelationships {
                hidden: vec![WarpHiddenPane {
                    pane: 0,
                    restored_by: None,
                    is_scrollback_editor: false,
                }],
                stacks: vec![WarpStackList {
                    members: vec![1, 0],
                    selected: 1,
                }],
            },
        )
        .unwrap();
        screen.render_to_clients().unwrap();
        assert!(screen.tabs[&TAB].pane_is_hidden_stack_list_member(&WAITING));
        apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
        let start = waiting_since(&screen).unwrap();
        screen.pinned_now = Some(start + Duration::from_millis(900));
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        let pulse = screen.tabs[&TAB]
            .get_pane_with_id(WAITING)
            .unwrap()
            .waiting_pulse();
        assert!(pulse.is_some());
        drain_background_jobs(&jobs);

        let tab = screen.tabs.get_mut(&TAB).unwrap();
        let visible = tab.get_pane_with_id(FOCUSED).unwrap();
        let geom = visible.current_geom();
        let offset = visible.get_content_offset();
        let cover = PaneId::Terminal(TerminalId::new(4));
        // WAITING is rank 1. Cover its header but leave the outer borders and lower content
        // exposed. Floating panes need at least five rows, so cover a few content rows too.
        let mut coordinates = FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(geom.x + offset.left)),
            y: Some(PercentOrFixed::Fixed(geom.y + 1)),
            width: Some(PercentOrFixed::Fixed(
                geom.cols.as_usize() - offset.left - offset.right,
            )),
            height: Some(PercentOrFixed::Fixed(5)),
            ..Default::default()
        };
        assert!(geom.rows.as_usize() > 6);
        tab.new_pane(
            cover,
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(coordinates.clone())),
            Some(CLIENT),
            None,
        )
        .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(
            screen.animation_clock_period(start),
            None,
            "floating={floating}"
        );
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(WAITING)
                .unwrap()
                .waiting_pulse(),
            None
        );
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StopAnimationClock));
        assert_eq!(waiting_since(&screen), Some(start));

        // The visible member still has exposed outer borders even with both headers covered.
        coordinates.y = Some(PercentOrFixed::Fixed(geom.y));
        coordinates.height = Some(PercentOrFixed::Fixed(6));
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .change_floating_pane_coordinates(&cover, coordinates.clone())
            .unwrap();
        apply_pane_agent_report(&mut screen, FOCUSED, Some(waiting())).unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        apply_pane_agent_report(&mut screen, FOCUSED, None).unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), None);
        drain_background_jobs(&jobs);

        coordinates.y = Some(PercentOrFixed::Fixed(geom.y + 2));
        screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .change_floating_pane_coordinates(&cover, coordinates)
            .unwrap();
        screen.render_to_clients().unwrap();
        assert_eq!(screen.animation_clock_period(start), Some(PERIOD));
        assert_eq!(waiting_since(&screen), Some(start));
        assert_eq!(
            screen.tabs[&TAB]
                .get_pane_with_id(WAITING)
                .unwrap()
                .waiting_pulse(),
            pulse
        );
        assert!(drain_background_jobs(&jobs).contains(&BackgroundJob::StartAnimationClock(PERIOD)));
    }
}

#[test]
fn waiting_pulse_survives_floating_focus_and_watchers() {
    let (mut screen, _) = pair(false, false, false, PaneFrameStyle::Full);
    let tab = screen.tabs.get_mut(&TAB).unwrap();
    tab.focus_pane_with_id(WAITING, false, false, CLIENT)
        .unwrap();
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(CLIENT),
        None,
    )
    .unwrap();
    apply_pane_agent_report(&mut screen, WAITING, Some(waiting())).unwrap();
    let start = waiting_since(&screen).unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(waiting_since(&screen), Some(start));
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .focus_pane_with_id(FOCUSED, false, false, CLIENT)
        .unwrap();
    let watcher = ClientId::new(2);
    screen.add_watcher_client(watcher).unwrap();
    screen.active_tab_ids.insert(watcher, TAB);
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .focus_pane_with_id(WAITING, false, false, watcher)
        .unwrap();
    screen.render().unwrap();
    assert_eq!(waiting_since(&screen), Some(start));
    screen.set_followed_client(CLIENT).unwrap();
    screen.active_tab_ids.remove(&watcher);
    screen.remove_client(CLIENT).unwrap();
    screen.render_to_clients().unwrap();
    assert_eq!(waiting_since(&screen), Some(start));
    assert_eq!(
        screen.animation_clock_period(start),
        Some(PERIOD),
        "a watcher still sees the border after the interactive client leaves"
    );
    screen.remove_watcher_client(watcher);
    screen.render_to_clients().unwrap();
    assert_eq!(screen.animation_clock_period(start), None);
}

#[test]
fn hook_flash_clock_repaints_collapsed_and_list_stack_titles() {
    use crate::screen::thread_main::apply_queued_agent_report;
    use yinz_types::agent::{
        report::{AgentReport, AgentReportState},
        AgentEvent, HookEvent,
    };
    for list in [false, true] {
        for style in [PaneFrameStyle::Full, PaneFrameStyle::None] {
            let (mut screen, _jobs) = pair(false, true, list, style);
            let mut agent = waiting();
            agent.state = AgentState::Busy;
            agent.wait_needs_a_person = false;
            apply_pane_agent_report(&mut screen, WAITING, Some(agent)).unwrap();
            let start = Instant::now();
            screen.pinned_now = Some(start);
            let hook = HookEvent {
                hook_event_name: "PreCompact".into(),
                ..Default::default()
            };
            apply_queued_agent_report(
                &mut screen,
                &AgentReport {
                    pane_id: WAITING,
                    report_id: "flash".into(),
                    process_id: ProcessId::new(1),
                    kind: "claude".into(),
                    state: AgentReportState::Unchanged,
                    event: Some(AgentEvent::from_hook(
                        "claude",
                        TerminalId::new(1),
                        &hook,
                        serde_json::to_string(&hook).unwrap(),
                    )),
                    trace: false,
                },
                start,
            )
            .unwrap();
            let (sender, receiver) = channels::unbounded();
            screen.bus.senders.to_server = Some(SenderWithContext::new(sender));
            let mut grid = grid_for_render_snapshot(SIZE);
            render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
            assert_eq!(
                screen.animation_clock_period(start),
                Some(FRAME_PERIOD),
                "list={list}, style={style:?}"
            );
            let color = |grid: &crate::panes::grid::Grid| {
                grid.as_character_lines()
                    .into_iter()
                    .flatten()
                    .find(|cell| cell.character == '●')
                    .unwrap()
                    .styles
                    .foreground
            };
            assert_eq!(
                color(&grid),
                Some(screen.style.colors.text_unselected.emphasis_0.into())
            );
            screen.pinned_now = Some(start + Duration::from_millis(150));
            screen.on_animation_tick(screen.frame_now()).unwrap();
            render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
            assert_eq!(screen.animation_clock_period(screen.frame_now()), None);
            assert_eq!(
                color(&grid),
                Some(screen.style.colors.text_unselected.emphasis_2.into())
            );
        }
    }
}
