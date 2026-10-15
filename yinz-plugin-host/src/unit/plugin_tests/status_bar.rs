use super::bar_parity::{rgb_theme, shipped, Pair, NOW};
use serde_json::json;
use std::{collections::BTreeMap, sync::atomic::Ordering};
use yinz_types::data::Event;
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    data::{CopyDestination, InputMode, ModeInfo, Mouse, PaneInfo, PaneManifest, TabInfo},
    input::actions::Action,
    keys::KeyModifier,
};

fn visual_parity(classic: bool, rows: usize) {
    use strum::IntoEnumIterator;
    {
        let mut pair = Pair::new(classic);
        pair.event(Event::TabUpdate(vec![TabInfo {
            active: true,
            active_swap_layout_name: Some("vertical".into()),
            selectable_tiled_panes_count: 3,
            ..Default::default()
        }]));
        for mode in InputMode::iter() {
            for simple in [false, true] {
                let mut info = shipped(mode);
                info.capabilities.arrow_fonts = simple;
                pair.event(Event::ModeUpdate(info));
                {
                    for cols in [
                        0, 1, 4, 5, 15, 29, 40, 59, 60, 79, 80, 99, 100, 119, 120, 121, 150, 200,
                        400,
                    ] {
                        pair.render(rows, cols);
                    }
                }
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_one_line_visual_parity() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_one_line_visual_parity",
    );
    visual_parity(false, 1);
}
#[test]
fn status_bar_wasm_deno_classic_one_line_visual_parity() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_classic_one_line_visual_parity",
    );
    visual_parity(true, 1);
}
#[test]
fn status_bar_wasm_deno_two_line_visual_parity() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_two_line_visual_parity",
    );
    visual_parity(false, 2);
}

#[test]
fn quit_hint_tracks_configured_bindings_without_a_command_palette_hint_in_both_bars() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "quit_hint_tracks_configured_bindings_without_a_command_palette_hint_in_both_bars",
    );
    for (classic, rows) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        let mut pair = Pair::new(classic);
        let mut info = shipped(InputMode::Normal);
        for expected in [Some("q"), Some("x"), None] {
            if expected != Some("q") {
                for (mode, binds) in &mut info.keybinds {
                    if *mode == InputMode::Normal {
                        binds.retain(|(_, actions)| !actions.contains(&Action::Quit));
                        if expected.is_some() {
                            binds.push(("Ctrl x".parse().unwrap(), vec![Action::Quit]));
                        }
                    }
                }
            }
            pair.event(Event::ModeUpdate(info.clone()));
            for cols in [40, 80, 120, 200] {
                let text = pair.render(rows, cols);
                assert!(!text.contains("Do"), "{text}");
                if cols >= 120 {
                    assert!(text.contains("PANE") && text.contains("AGENT"), "{text}");
                    if let Some(key) = expected {
                        assert!(text.contains(&format!("<{key}> QUIT")), "{text}");
                        assert!(
                            text.find("SESS").unwrap() < text.find("QUIT").unwrap(),
                            "{text}"
                        );
                    } else {
                        assert!(!text.contains("QUIT"), "{text}");
                    }
                }
            }
        }
    }
}

#[test]
fn status_bar_wasm_plugin_events_and_timing_parity() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_events_and_timing_parity",
    );
    let mut pair = Pair::new(false);
    pair.event(Event::InitialKeybinds(shipped(InputMode::Normal).keybinds));
    pair.event(Event::ModeUpdate(ModeInfo::default()));
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        selectable_tiled_panes_count: 2,
        ..Default::default()
    }]));
    pair.render(1, 500);
    for event in [
        Event::CopyToClipboard(CopyDestination::System),
        Event::CopyToClipboard(CopyDestination::System),
        Event::CopyToClipboard(CopyDestination::Primary),
        Event::CopyToClipboard(CopyDestination::Command),
        Event::InputReceived,
        Event::InputReceived,
        Event::SystemClipboardFailure,
        Event::InputReceived,
    ] {
        pair.event(event);
        pair.render(1, 500);
    }
    for (modifiers, is_live) in [
        (vec![KeyModifier::Alt], false),
        (vec![KeyModifier::Alt], true),
        (vec![KeyModifier::Alt], false),
        (vec![], false),
        (vec![], true),
    ] {
        pair.event(Event::ModifiersHeld {
            modifiers: modifiers.into_iter().collect(),
            is_live,
        });
        pair.render(1, 500);
    }
    pair.event(Event::Mouse(Mouse::Hover(0, 0)));
    let text = pair.render(1, 500);
    for label in ["New Pane", "Floating"] {
        let col = text
            .chars()
            .collect::<Vec<_>>()
            .windows(label.chars().count())
            .position(|s| s.iter().collect::<String>() == label)
            .unwrap_or_else(|| panic!("missing {label}: {text}"));
        pair.event(Event::Mouse(Mouse::Hover(0, col)));
        pair.render(1, 500);
        pair.event(Event::Mouse(Mouse::LeftClick(0, col)));
        pair.render(1, 500);
    }
    pair.event(Event::Mouse(Mouse::Hover(0, u16::MAX as usize)));
    pair.render(1, 500);
    let mut clock = NOW;
    for state in [
        AgentState::Busy,
        AgentState::Waiting,
        AgentState::Idle,
        AgentState::Exited,
    ] {
        let pane = PaneInfo {
            agent: Some(AgentInfo {
                state,
                wait_needs_a_person: true,
                kind: "codex".into(),
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                cwd: None,
                session_id: None,
                run_id: None,
            }),
            ..Default::default()
        };
        pair.event(Event::PaneUpdate(PaneManifest {
            panes: [(yinz_types::internal_ids::TabPosition::new(0), vec![pane])]
                .into_iter()
                .collect(),
        }));
        for _ in 0..9 {
            clock += 125;
            pair.at(clock);
            pair.render(1, 500);
        }
    }
    pair.event(Event::ModifiersHeld {
        modifiers: [KeyModifier::Alt].into_iter().collect(),
        is_live: false,
    });
    pair.at(clock + 4000);
    pair.render(1, 500);
    pair.set_offset(-18_000);
    pair.at((pair.now.load(Ordering::SeqCst) / 3_600_000 + 1) * 3_600_000);
    pair.render(1, 500);
    pair.render(2, 80);
    pair.render(1, 80);
    pair.event(Event::Mouse(Mouse::Hover(0, 0)));
    let deadline = pair.now.load(Ordering::SeqCst) + 30_000;
    pair.at(deadline - 1);
    pair.render(1, 500);
    pair.at(deadline);
    pair.render(1, 500);
    let midnight = (NOW / 86_400_000 + 2) * 86_400_000 + 18_000_000;
    pair.at(midnight - 1);
    pair.render(1, 500);
    pair.at(midnight);
    pair.render(1, 500);
}

#[test]
fn status_bar_wasm_deno_locked_base_rgb_and_missing_bindings() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_locked_base_rgb_and_missing_bindings",
    );
    use strum::IntoEnumIterator;
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    for missing in [false, true] {
        for mode in InputMode::iter() {
            let mut info = shipped(mode);
            info.base_mode = Some(InputMode::Locked);
            if missing {
                for (_, bindings) in &mut info.keybinds {
                    bindings.clear();
                }
            }
            rgb_theme(&mut info);
            pair.event(Event::ModeUpdate(info));
            for rows in [1, 2] {
                for cols in [5, 30, 80, 120, 200, 400] {
                    pair.render(rows, cols);
                }
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_custom_bindings_unicode_and_reconfiguration() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_custom_bindings_unicode_and_reconfiguration",
    );
    use strum::IntoEnumIterator;
    let config = yinz_config::config::Config::from_kdl(
        r#"
        keybinds clear-defaults=true {
            shared {
                bind "Ctrl Alt 界" { SwitchToMode "Pane"; }
                bind "Alt 🦀" { SwitchToMode "Tab"; }
                bind "Super F12" { SwitchToMode "Session"; }
                bind "Ctrl Shift a" { SwitchToMode "Agent"; }
                bind "Ctrl g" { SwitchToMode "Locked"; }
                bind "Enter" "Esc" { SwitchToMode "Normal"; }
            }
            pane {
                bind "Ctrl n" "N" "Alt n" "F2" { NewPane; SwitchToMode "Normal"; }
                bind "Ctrl Shift h" { MoveFocus "Left"; }
                bind "Ctrl Shift l" { MoveFocus "Right"; }
                bind "x" { CloseFocus; }
                bind "Alt x" { CloseFocus; SwitchToMode "Normal"; }
                bind "Alt z" { NewPane "Left"; }
            }
            normal {
                bind "Alt 界" { NewPane; }
                bind "Alt 🦀" { ToggleFloatingPanes; }
                bind "Shift +" { Resize "Increase"; }
                bind "Ctrl -" { Resize "Decrease"; }
            }
            resize {
                bind "Ctrl 界" { Resize "Increase Left"; }
                bind "Ctrl 🦀" { Resize "Increase Right"; }
            }
            tab {
                bind "h" "Left" "Up" { GoToPreviousTab; SwitchToMode "Normal"; }
                bind "l" "Right" "Down" { GoToNextTab; SwitchToMode "Normal"; }
            }
            session {
                bind "Ctrl q" { LaunchOrFocusPlugin "unrelated"; }
                bind "Ctrl a" { LaunchPlugin "session-manager"; SwitchToMode "Normal"; }
                bind "c" { LaunchOrFocusPlugin "configuration"; }
                bind "d" { LaunchOrFocusPlugin "yinz:layout-manager"; }
                bind "e" { LaunchOrFocusPlugin "plugin-manager"; }
            }
            agent {
                bind "n" { LaunchOrFocusPlugin "new-agent"; }
                bind "d" { LaunchOrFocusPlugin "duplicate-agent"; }
                bind "f" { LaunchOrFocusPlugin "fork-agent"; }
                bind "a" { LaunchOrFocusPlugin "agents"; }
                bind "c" { LaunchOrFocusPlugin "agents"; }
            }
        }
    "#,
        None,
    )
    .unwrap();
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        selectable_tiled_panes_count: 2,
        ..Default::default()
    }]));
    let mut bindings = config.keybinds.to_keybinds_vec();
    // KDL bindings live in maps; give both original runtimes and Steel a stable fixture order.
    bindings.sort_by_key(|(mode, _)| serde_json::to_string(mode).unwrap());
    for (_, keys) in &mut bindings {
        keys.sort_by_key(|(key, _)| serde_json::to_string(key).unwrap());
    }
    pair.event(Event::InitialKeybinds(bindings));
    for mode in InputMode::iter() {
        let mut info = ModeInfo {
            mode,
            ..Default::default()
        };
        rgb_theme(&mut info);
        pair.event(Event::ModeUpdate(info));
        for rows in [1, 2] {
            for cols in [1, 25, 60, 100, 160, 300] {
                pair.render(rows, cols);
            }
        }
    }
    // Rust key equality treats ASCII capitals as Shift plus the lowercase key.
    for (i, spelling) in ["N", "Shift n"].into_iter().enumerate() {
        pair.event(Event::ModeUpdate(ModeInfo {
            keybinds: vec![(
                InputMode::Normal,
                vec![(
                    spelling.parse().unwrap(),
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Pane,
                    }],
                )],
            )],
            ..Default::default()
        }));
        pair.render(1, 80 + i); // Resize invalidates Deno's unchanged-frame cache.
    }
    pair.event(Event::InitialKeybinds(shipped(InputMode::Normal).keybinds));
    pair.event(Event::ModeUpdate(ModeInfo::default()));
    pair.render(1, 500);
    pair.event(Event::ModifiersHeld {
        modifiers: [KeyModifier::Alt].into_iter().collect(),
        is_live: true,
    });
    pair.render(1, 500);
}

#[test]
fn status_bar_wasm_deno_nested_fullscreen_floating_and_swap_layouts() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_nested_fullscreen_floating_and_swap_layouts",
    );
    let mut pair = Pair::new(false);
    for mode in [InputMode::Normal, InputMode::Locked, InputMode::Pane] {
        for (dimmed, ascended) in [(false, false), (true, false), (false, true)] {
            let mut info = shipped(mode);
            info.session_dimmed = Some(dimmed);
            info.session_ascended = Some(ascended);
            info.nested_ascend_keys = vec!["Ctrl Alt 界".parse().unwrap(), "Esc".parse().unwrap()];
            info.nested_descend_keys = vec!["Shift 🦀".parse().unwrap()];
            pair.event(Event::ModeUpdate(info));
            for (fullscreen, floating) in [(false, false), (true, false), (false, true)] {
                pair.event(Event::TabUpdate(vec![TabInfo {
                    active: true,
                    is_fullscreen_active: fullscreen,
                    are_floating_panes_visible: floating,
                    panes_to_hide: 3,
                    selectable_floating_panes_count: 2,
                    active_swap_layout_name: Some("vertical 界".into()),
                    is_swap_layout_dirty: floating,
                    ..Default::default()
                }]));
                for rows in [1, 2] {
                    for cols in [1, 15, 30, 80, 200, 400] {
                        pair.render(rows, cols);
                    }
                }
            }
        }
    }
    let mut info = shipped(InputMode::Normal);
    info.session_dimmed = Some(true);
    pair.event(Event::ModeUpdate(info)); // Unbound nested-session hint.
    pair.render(1, 80);
    pair.render(2, 80);
}

#[test]
fn status_bar_wasm_deno_all_cached_tips() {
    let _reference_trace =
        crate::plugin_tests::reference_trace::Scenario::new("status_bar_wasm_deno_all_cached_tips");
    let names = [
        "quicknav",
        "floating_panes_mouse",
        "send_mouse_clicks_to_terminal",
        "move_focus_hjkl_tab_switch",
        "yinz_setup_check",
        "use_mouse",
        "sync_tab",
        "edit_scrollbuffer",
        "compact_layout",
        "move_tabs",
    ];
    let mut pair = Pair::new(false);
    pair.event(Event::ModeUpdate(shipped(InputMode::Normal)));
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    let cache = json!({"yinz_version":"test", "cached_data": names.iter().map(|name| ((*name).to_owned(), 21)).collect::<BTreeMap<_,_>>()});
    let configuration = BTreeMap::from([("classic".to_owned(), "false".to_owned())]);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..128 {
        let chosen = pair.reload_tip(cache.clone(), &configuration);
        if seen.insert(chosen.clone()) {
            for cols in [20, 60, 80, 100, 120, 160, 200, 400] {
                pair.render(2, cols);
            }
        }
        if seen.len() == names.len() {
            break;
        }
    }
    assert_eq!(seen, names.into_iter().map(str::to_owned).collect());
}

#[test]
fn status_bar_wasm_deno_narrow_width_transitions() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_narrow_width_transitions",
    );
    let mut pair = Pair::new(false);
    pair.event(Event::TabUpdate(vec![TabInfo {
        active: true,
        ..Default::default()
    }]));
    for mode in [InputMode::Normal, InputMode::Pane] {
        pair.event(Event::ModeUpdate(shipped(mode)));
        for rows in [1, 2] {
            for cols in 0..=220 {
                pair.render(rows, cols);
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_mixed_agents_and_animation_frames() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_mixed_agents_and_animation_frames",
    );
    let mut pair = Pair::new(false);
    let agent = |state, wait_needs_a_person| PaneInfo {
        agent: Some(AgentInfo {
            state,
            wait_needs_a_person,
            kind: "codex".into(),
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
        ..Default::default()
    };
    for simple in [false, true] {
        let mut info = shipped(InputMode::Normal);
        info.capabilities.arrow_fonts = simple;
        pair.event(Event::ModeUpdate(info));
        let manifest = PaneManifest {
            panes: [(
                yinz_types::internal_ids::TabPosition::new(0),
                vec![
                    agent(AgentState::Busy, false),
                    agent(AgentState::Waiting, false),
                    agent(AgentState::Waiting, true),
                    agent(AgentState::Idle, false),
                    agent(AgentState::Exited, false),
                ],
            )]
            .into_iter()
            .collect(),
        };
        pair.event(Event::PaneUpdate(manifest.clone()));
        pair.event(Event::PaneUpdate(manifest));
        for _ in 0..9 {
            pair.at(pair.now.load(Ordering::SeqCst) + 125);
            for cols in [4, 5, 10, 25, 60, 120, 150, 200, 300, 500] {
                pair.render(1, cols);
            }
        }
    }
}

#[test]
fn status_bar_wasm_deno_theme_colors_follow_mode_updates() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "status_bar_wasm_deno_theme_colors_follow_mode_updates",
    );
    use yinz_terminal::panes::AnsiCode;
    use yinz_types::colors::PaletteColor;

    let mut pair = Pair::new(false);
    pair.event(Event::PaneUpdate(PaneManifest {
        panes: [(
            yinz_types::internal_ids::TabPosition::new(0),
            [AgentState::Waiting, AgentState::Busy, AgentState::Idle]
                .into_iter()
                .map(|state| PaneInfo {
                    agent: Some(AgentInfo {
                        state,
                        wait_needs_a_person: true,
                        kind: "codex".into(),
                        source: AgentSource::Reported,
                        detail: None,
                        waiting_on: None,
                        cwd: None,
                        session_id: None,
                        run_id: None,
                    }),
                    ..Default::default()
                })
                .collect(),
        )]
        .into_iter()
        .collect(),
    }));
    // Change only the palette via ModeUpdate: existing counts and clock must repaint.
    for rgb in [false, true, false] {
        let mut info = shipped(InputMode::Normal);
        info.capabilities.arrow_fonts = false;
        if rgb {
            rgb_theme(&mut info);
            info.style.colors.exit_code_error.base = PaletteColor::Rgb((31, 32, 33));
        }
        let colors = info.style.colors;
        let text = colors.text_unselected;
        pair.event(Event::ModeUpdate(info.clone()));
        for phase in [0, 500] {
            pair.at(NOW / 60_000 * 60_000 + phase);
            for cols in [5, 30, 80, 120, 200, 500] {
                let cells = pair.render_cells(1, cols);
                let row = &cells[0];
                let plain: String = row.iter().map(|c| c.character).collect();
                let Some(start) = plain.chars().position(|c| c == '◆') else {
                    assert!(cols < 500, "agent counts must fit at {cols} columns");
                    continue;
                };
                let segments = [
                    (
                        "◆1",
                        if phase == 0 {
                            colors.exit_code_error.base
                        } else {
                            text.base
                        },
                        phase != 0,
                    ),
                    (" ", text.base, true),
                    (if phase == 0 { "⣾1" } else { "⡿1" }, text.emphasis_2, false),
                    (" ", text.base, true),
                    ("·1", text.base, true),
                    ("   ", text.base, true),
                    ("18:49", text.emphasis_1, false),
                    ("  ", text.base, true),
                    ("Fri 5 Sep", text.base, true),
                ];
                // Narrow forms shed the date, then the clock, while preserving counts.
                let segment_count = if plain.contains("Fri 5 Sep") {
                    9
                } else if plain.contains("18:49") {
                    7
                } else {
                    5
                };
                assert!(cols < 500 || segment_count == 9);
                let mut at = start;
                for (label, foreground, dim) in segments.into_iter().take(segment_count) {
                    assert!(row[at..]
                        .iter()
                        .map(|c| c.character)
                        .collect::<String>()
                        .starts_with(label));
                    for cell in &row[at..at + label.chars().count()] {
                        assert_eq!(cell.styles.foreground, Some(foreground.into()));
                        assert_eq!(cell.styles.background, Some(text.background.into()));
                        assert_eq!(cell.styles.dim == Some(AnsiCode::On), dim);
                        assert_eq!(cell.styles.bold == Some(AnsiCode::On), !dim);
                    }
                    at += label.chars().count();
                }
            }
        }
        info.session_dimmed = Some(true);
        pair.event(Event::ModeUpdate(info));
        let dimmed = pair.render(1, 500);
        assert!(dimmed.contains("Ascend:"));
        assert!(!dimmed.contains('◆'));
    }
}
