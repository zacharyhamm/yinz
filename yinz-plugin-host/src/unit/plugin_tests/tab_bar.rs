use super::bar_parity::{rgb_theme, shipped, Pair};
use std::collections::BTreeMap;
use yinz_types::data::Event;
use yinz_types::{
    data::{ClientId, InputMode, ModeInfo, Mouse, StyledText, TabInfo},
    input::{actions::Action, options::PaneFrameStyle},
    internal_ids::TabPosition,
    keys::{BareKey, KeyWithModifier},
    units::FractionalSeconds,
};

fn pair(hostname: &str, hide_swap_layout: bool) -> Pair {
    Pair::builtin(
        "tab-bar",
        BTreeMap::from([(
            "hide_swap_layout_indication".into(),
            hide_swap_layout.to_string(),
        )]),
        hostname,
    )
}

fn tabs(names: &[&str], active: usize) -> Vec<TabInfo> {
    names
        .iter()
        .enumerate()
        .map(|(position, name)| TabInfo {
            position: TabPosition::new(position),
            name: (*name).into(),
            active: position == active,
            ..Default::default()
        })
        .collect()
}

fn widths(pair: &mut Pair) {
    for cols in 0..=180 {
        pair.render(1, cols);
    }
    for rows in [0, 2] {
        pair.render(rows, 240);
    }
}

#[test]
fn tab_bar_wasm_deno_titles_overflow_and_modes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_titles_overflow_and_modes",
    );
    use strum::IntoEnumIterator;
    let mut pair = pair("", false);
    assert!(pair.render(1, 80).trim().is_empty());
    let names = [
        "",
        "main",
        "a very long tab title that exceeds the forty column ceiling",
        "日本語の長いタイトル日本語の長いタイトル日本語の長いタイトル",
        "a\u{301}\u{301}bcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
        "🦀👨‍👩‍👧‍👦workspace",
        "last",
    ];
    for simple in [false, true] {
        for mode in InputMode::iter() {
            let mut info = shipped(mode);
            info.capabilities.arrow_fonts = simple;
            pair.event(Event::ModeUpdate(info));
            for active in [0, 3, 6] {
                pair.event(Event::TabUpdate(tabs(&names, active)));
                if matches!(mode, InputMode::Normal | InputMode::RenameTab) {
                    widths(&mut pair);
                } else {
                    for cols in [0, 1, 15, 30, 60, 80, 120, 180, 240] {
                        pair.render(1, cols);
                    }
                }
            }
        }
    }
}

#[test]
fn tab_bar_wasm_deno_large_overflow_counters() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_large_overflow_counters",
    );
    let mut pair = pair("", false);
    for count in [12, 102, 10_002] {
        for active in [0, count - 1] {
            pair.event(Event::TabUpdate(tabs(&vec!["tab"; count], active)));
            pair.render(1, 80);
        }
    }
}

#[test]
fn tab_bar_wasm_deno_markers_multiplayer_and_themes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_markers_multiplayer_and_themes",
    );
    let mut pair = pair("host-日本", false);
    for simple in [false, true] {
        for dim in [0, 1, 2] {
            let mut info = shipped(InputMode::Normal);
            rgb_theme(&mut info);
            info.capabilities.arrow_fonts = simple;
            info.session_name = Some("work".into());
            info.session_dimmed = Some(dim == 1);
            info.session_ascended = Some(dim == 2);
            info.host_fullscreen = Some(dim == 2);
            info.session_ancestry = vec!["parent".into()];
            pair.event(Event::ModeUpdate(info));
            for active in 0..4 {
                let mut tabs = tabs(
                    &[
                        "one",
                        "two",
                        "three",
                        "very long active tab title that needs to be truncated",
                    ],
                    active,
                );
                for (index, tab) in tabs.iter_mut().enumerate() {
                    tab.is_fullscreen_active = index == 0;
                    tab.is_sync_panes_active = index <= 1;
                    tab.has_bell_notification = index % 2 == 0;
                    tab.is_flashing_bell = index == 3;
                    tab.other_focused_clients =
                        [0, 1, 2, 10, 11].into_iter().map(ClientId::new).collect();
                }
                pair.event(Event::TabUpdate(tabs));
                widths(&mut pair);
            }
        }
    }
}

#[test]
fn tab_bar_wasm_deno_mouse_ranges_and_breadcrumbs() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_mouse_ranges_and_breadcrumbs",
    );
    let mut pair = pair("host", false);
    for simple in [false, true] {
        for nested in [false, true] {
            for hide_name in [false, true] {
                let mut info = shipped(InputMode::Normal);
                info.capabilities.arrow_fonts = simple;
                info.session_name = Some("child".into());
                info.host_fullscreen = Some(nested);
                info.session_ancestry = vec!["root".into(), "parent-日本".into()];
                info.style.hide_session_name = hide_name;
                pair.event(Event::ModeUpdate(info));
                for active in [0, 2, 4] {
                    pair.event(Event::TabUpdate(tabs(
                        &["one", "two", "日本", "four", "five"],
                        active,
                    )));
                    for cols in [15, 30, 45, 60, 80, 120] {
                        pair.render(1, cols);
                        for col in 0..=cols + 1 {
                            pair.event(Event::Mouse(Mouse::Hover(0, col)));
                            pair.render(1, cols);
                            pair.event(Event::Mouse(Mouse::Hover(0, col)));
                            pair.event(Event::Mouse(Mouse::LeftClick(0, col)));
                        }
                        pair.event(Event::Mouse(Mouse::Hover(0, usize::MAX)));
                        pair.render(1, cols);
                    }
                    for mouse in [
                        Mouse::ScrollUp(2),
                        Mouse::ScrollDown(2),
                        Mouse::RightClick(0, 10),
                        Mouse::Hold(0, 10),
                        Mouse::Release(0, 10),
                    ] {
                        pair.event(Event::Mouse(mouse));
                    }
                }
            }
        }
    }
}

#[test]
fn tab_bar_wasm_deno_swap_scroll_and_styled_hint_lifetimes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_swap_scroll_and_styled_hint_lifetimes",
    );
    for hidden in [false, true] {
        let mut pair = pair("workstation", hidden);
        for simple in [false, true] {
            for dim in [false, true] {
                for frames in [None, Some(PaneFrameStyle::Full)] {
                    let mut info = shipped(InputMode::Normal);
                    info.capabilities.arrow_fonts = simple;
                    info.session_dimmed = Some(dim);
                    info.pane_frame_style = frames;
                    pair.event(Event::ModeUpdate(info));
                    for dirty in [false, true] {
                        for layout in [None, Some("vertical"), Some("日本straße")] {
                            let mut tabs = tabs(&["main", "other"], 0);
                            tabs[0].active_swap_layout_name = layout.map(str::to_owned);
                            tabs[0].is_swap_layout_dirty = dirty;
                            pair.event(Event::TabUpdate(tabs));
                            for scroll in [None, Some((0, 0)), Some((12, 100))] {
                                pair.event(Event::ActivePaneScroll(scroll));
                                for cols in [20, 40, 60, 80, 120] {
                                    pair.render(1, cols);
                                }
                            }
                            let hints = BTreeMap::from([
                                (
                                    4,
                                    StyledText {
                                        text: "日本".into(),
                                        indices: vec![vec![0], vec![1]],
                                    },
                                ),
                                (
                                    18,
                                    StyledText {
                                        text: "Styled hint text".into(),
                                        indices: vec![
                                            vec![0],
                                            vec![1],
                                            vec![2],
                                            vec![3],
                                            vec![4],
                                            vec![5],
                                            vec![6],
                                            vec![7],
                                            vec![8],
                                        ],
                                    },
                                ),
                            ]);
                            for event in [
                                Event::HintText(hints.clone()),
                                Event::HintText(hints.clone()),
                                Event::Timer(FractionalSeconds::new(5.0)),
                                Event::Timer(FractionalSeconds::new(5.0)),
                                Event::HintText(hints.clone()),
                                Event::InputReceived,
                                Event::InputReceived,
                                Event::HintText(hints),
                                Event::HintText(BTreeMap::new()),
                                Event::Timer(FractionalSeconds::new(5.0)),
                                Event::Timer(FractionalSeconds::new(5.0)),
                            ] {
                                pair.event(event);
                                for cols in [20, 40, 60, 80, 120] {
                                    pair.render(1, cols);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn tab_bar_wasm_deno_startup_duplicates_and_cached_keybinds() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "tab_bar_wasm_deno_startup_duplicates_and_cached_keybinds",
    );
    let mut pair = pair("host", false);
    for event in [
        Event::ModeUpdate(ModeInfo::default()),
        Event::InitialKeybinds(vec![]),
        Event::TabUpdate(vec![]),
        Event::ActivePaneScroll(Some((5, 10))),
        Event::Mouse(Mouse::ScrollUp(1)),
        Event::Mouse(Mouse::ScrollDown(1)),
    ] {
        pair.event(event);
    }
    let mut tabs = tabs(&["main"], 0);
    tabs[0].active_swap_layout_name = Some("vertical".into());
    pair.event(Event::TabUpdate(tabs.clone()));
    pair.event(Event::TabUpdate(tabs));
    let mut info = shipped(InputMode::Normal);
    pair.event(Event::InitialKeybinds(info.keybinds.clone()));
    info.keybinds.clear();
    pair.event(Event::ModeUpdate(info.clone()));
    pair.event(Event::ModeUpdate(info.clone()));
    widths(&mut pair);
    for keys in [
        vec![],
        vec![
            (
                KeyWithModifier::new(BareKey::Char('界')).with_alt_modifier(),
                vec![Action::PreviousSwapLayout],
            ),
            (
                KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
                vec![Action::NextSwapLayout],
            ),
            (
                KeyWithModifier::new(BareKey::Left).with_ctrl_modifier(),
                vec![Action::PreviousSwapLayout],
            ),
        ],
    ] {
        info.keybinds = vec![(InputMode::Normal, keys)];
        pair.event(Event::ModeUpdate(info.clone()));
        widths(&mut pair);
        pair.event(Event::InitialKeybinds(vec![]));
    }
    pair.event(Event::TabUpdate(vec![])); // No active tab retains the last valid state.
    pair.render(1, 80);
}
