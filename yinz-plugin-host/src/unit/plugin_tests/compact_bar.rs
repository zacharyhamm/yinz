use super::bar_parity::{rgb_theme, shipped, Pair};
use strum::IntoEnumIterator;
use yinz_types::data::Event;
use yinz_types::{
    data::{
        ClientId, CopyDestination, InputMode, ModeInfo, Mouse, PaneId, PaneInfo, PaneManifest,
        PipeMessage, PipeSource, TabInfo,
    },
    geometry::Direction,
    input::{
        actions::{Action, RunCommandAction},
        layout::{PluginAlias, RunPluginOrAlias},
    },
    internal_ids::{PluginId, TabId, TabPosition},
    keys::{BareKey, KeyWithModifier},
    position::{Column, ColumnCount, RowCount},
};

fn pair(configuration: &[(&str, &str)], hostname: &str) -> Pair {
    Pair::builtin(
        "compact-bar",
        configuration
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
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
            display_area_rows: RowCount::new(30),
            display_area_columns: ColumnCount::new(120),
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

fn manifest(own_tab: usize, tooltip: bool) -> PaneManifest {
    PaneManifest {
        panes: [(
            TabPosition::new(own_tab),
            vec![
                PaneInfo {
                    id: PaneId::Plugin(PluginId::new(0)),
                    plugin_url: Some("yinz:compact-bar".into()),
                    ..Default::default()
                },
                PaneInfo {
                    id: PaneId::Plugin(PluginId::new(1)),
                    plugin_url: Some("yinz:compact-bar".into()),
                    pane_content_x: Column::new(usize::from(tooltip)),
                    ..Default::default()
                },
            ],
        )]
        .into_iter()
        .collect(),
    }
}

fn message(name: &str, private: bool, client: Option<&str>) -> PipeMessage {
    PipeMessage::new(
        PipeSource::Keybind,
        name,
        &client.map(str::to_owned),
        &None,
        private,
    )
}

#[test]
fn compact_bar_wasm_deno_titles_overflow_and_modes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_titles_overflow_and_modes",
    );
    let mut pair = pair(&[], "");
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
    for count in [12, 102, 10_002] {
        for active in [0, count - 1] {
            pair.event(Event::TabUpdate(tabs(&vec!["tab"; count], active)));
            pair.render(1, 80);
        }
    }
}

#[test]
fn compact_bar_wasm_deno_markers_nested_hints_and_themes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_markers_nested_hints_and_themes",
    );
    let mut pair = pair(&[("tooltip", "Ctrl t")], "host-日本");
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
            for bound in [false, true] {
                let keys = if bound {
                    vec![
                        KeyWithModifier::new(BareKey::Char('界')).with_alt_modifier(),
                        KeyWithModifier::new(BareKey::Left).with_ctrl_modifier(),
                    ]
                } else {
                    vec![]
                };
                info.nested_ascend_keys = keys.clone();
                info.nested_descend_keys = keys;
                pair.event(Event::ModeUpdate(info.clone()));
                for active in 0..4 {
                    let mut tabs = tabs(
                        &[
                            "one",
                            "two",
                            "three",
                            "a very long active tab title that is truncated",
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
                    pair.event(Event::PaneUpdate(manifest(active, bound)));
                    widths(&mut pair);
                }
            }
        }
    }
}

#[test]
fn compact_bar_wasm_deno_mouse_ranges_and_breadcrumbs() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_mouse_ranges_and_breadcrumbs",
    );
    let mut pair = pair(&[], "host");
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
                            pair.event(Event::Mouse(Mouse::LeftClick(0, col)));
                        }
                    }
                    for mouse in [
                        Mouse::ScrollUp(2),
                        Mouse::ScrollDown(2),
                        Mouse::RightClick(0, 10),
                        Mouse::Hold(0, 10),
                        Mouse::Release(0, 10),
                        Mouse::LeftClick(0, usize::MAX),
                    ] {
                        pair.event(Event::Mouse(mouse));
                    }
                }
            }
        }
    }
}

#[test]
fn compact_bar_wasm_deno_swap_layout_and_clipboard_lifetimes() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_swap_layout_and_clipboard_lifetimes",
    );
    for tooltip in ["", "Ctrl t", "Alt 界"] {
        let mut pair = pair(&[("tooltip", tooltip)], "");
        for mode in [InputMode::Normal, InputMode::Locked, InputMode::RenameTab] {
            for simple in [false, true] {
                let mut info = shipped(mode);
                rgb_theme(&mut info);
                info.capabilities.arrow_fonts = simple;
                pair.event(Event::ModeUpdate(info));
                for dirty in [false, true] {
                    for layout in [None, Some("vertical"), Some("日本straße")] {
                        let mut tabs = tabs(&["main", "other"], 0);
                        tabs[0].active_swap_layout_name = layout.map(str::to_owned);
                        tabs[0].is_swap_layout_dirty = dirty;
                        pair.event(Event::TabUpdate(tabs));
                        for cols in [20, 40, 60, 80, 120] {
                            pair.render(1, cols);
                        }
                    }
                }
                for event in [
                    Event::CopyToClipboard(CopyDestination::System),
                    Event::CopyToClipboard(CopyDestination::System),
                    Event::SystemClipboardFailure,
                    Event::CopyToClipboard(CopyDestination::Primary),
                    Event::CopyToClipboard(CopyDestination::Command),
                    Event::InputReceived,
                    Event::InputReceived,
                    Event::SystemClipboardFailure,
                    Event::SystemClipboardFailure,
                    Event::InputReceived,
                ] {
                    pair.event(event);
                    for cols in [0, 1, 20, 40, 80] {
                        pair.render(1, cols);
                    }
                    pair.event(Event::Mouse(Mouse::LeftClick(0, 30)));
                }
            }
        }
    }
}

#[test]
fn compact_bar_wasm_deno_startup_duplicates_and_cached_keybinds() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_startup_duplicates_and_cached_keybinds",
    );
    for configuration in [vec![], vec![("is_tooltip", "invalid")]] {
        let mut pair = pair(&configuration, "host");
        for event in [
            Event::ModeUpdate(ModeInfo::default()),
            Event::InitialKeybinds(vec![]),
            Event::TabUpdate(vec![]),
            Event::PaneUpdate(PaneManifest::default()),
            Event::Mouse(Mouse::ScrollUp(1)),
            Event::Mouse(Mouse::ScrollDown(1)),
            Event::InputReceived,
        ] {
            pair.event(event);
        }
        let tab_update = tabs(&["main"], 0);
        pair.event(Event::TabUpdate(tab_update.clone()));
        pair.event(Event::TabUpdate(tab_update));
        let mut info = shipped(InputMode::Normal);
        pair.event(Event::InitialKeybinds(info.keybinds.clone()));
        info.keybinds.clear();
        pair.event(Event::ModeUpdate(info.clone()));
        pair.event(Event::ModeUpdate(info));
        pair.event(Event::InitialKeybinds(vec![]));
        pair.event(Event::TabUpdate(vec![]));
        pair.event(Event::TabUpdate(tabs(&["inactive"], 1)));
        widths(&mut pair);
        pair.pipe(message("toggle_tooltip", true, Some("0")));
    }
}

#[test]
fn compact_bar_wasm_deno_tooltip_modes_and_dimensions() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_tooltip_modes_and_dimensions",
    );
    let mut pair = pair(&[("is_tooltip", "true")], "host");
    // A persisted tooltip survives all mode transitions; commands still record repositioning.
    pair.pipe(message("toggle_persisted_tooltip", true, None));
    pair.event(Event::TabUpdate(tabs(&["main", "other"], 0)));
    for simple in [false, true] {
        for mode in InputMode::iter() {
            let mut info = shipped(mode);
            rgb_theme(&mut info);
            info.capabilities.arrow_fonts = simple;
            pair.event(Event::ModeUpdate(info.clone()));
            pair.event(Event::ModeUpdate(info));
            for rows in [0, 1, 3, 8, 24] {
                for cols in [0, 1, 10, 30, 60, 80, 120, 240] {
                    pair.render(rows, cols);
                }
            }
        }
    }
    pair.event(Event::InitialKeybinds(vec![]));
    pair.event(Event::ModeUpdate(ModeInfo::default()));
    pair.render(8, 120);
}

#[test]
fn compact_bar_wasm_deno_tooltip_grouped_custom_keys() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_tooltip_grouped_custom_keys",
    );
    let mut pair = pair(&[("is_tooltip", "true")], "");
    pair.pipe(message("toggle_persisted_tooltip", true, None));
    for mode in [InputMode::Normal, InputMode::Pane, InputMode::Locked] {
        let action = if mode == InputMode::Pane {
            Action::MoveFocus {
                direction: Direction::Left,
            }
        } else {
            Action::SwitchToMode {
                input_mode: if mode == InputMode::Normal {
                    InputMode::Locked
                } else {
                    InputMode::Normal
                },
            }
        };
        for keys in [
            vec![BareKey::Esc, BareKey::Enter],
            vec![BareKey::Char('界')],
            vec![
                BareKey::Esc,
                BareKey::Enter,
                BareKey::Char('l'),
                BareKey::Char('h'),
                BareKey::Char('h'),
                BareKey::Char('k'),
                BareKey::Char('j'),
                BareKey::Char('L'),
                BareKey::Char('H'),
                BareKey::Left,
                BareKey::Left,
                BareKey::Down,
                BareKey::Right,
                BareKey::Up,
                BareKey::Char(']'),
                BareKey::Char('['),
                BareKey::Char('='),
                BareKey::Char('-'),
                BareKey::Char('+'),
                BareKey::PageDown,
                BareKey::PageUp,
                BareKey::Char('界'),
            ],
        ] {
            let mut keybinds: Vec<_> = keys
                .into_iter()
                .map(|key| (KeyWithModifier::new(key), vec![action.clone()]))
                .collect();
            if keybinds.len() > 2 {
                keybinds.push((
                    KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
                    vec![action.clone()],
                ));
                keybinds.push((
                    KeyWithModifier::new(BareKey::Char('z')),
                    vec![Action::Quit, action.clone()],
                ));
            }
            pair.event(Event::InitialKeybinds(vec![(mode, keybinds)]));
            pair.event(Event::ModeUpdate(ModeInfo {
                mode,
                ..Default::default()
            }));
            for cols in [0, 1, 10, 30, 60, 80, 120, 240] {
                pair.render(8, cols);
            }
        }
    }
}

#[test]
fn compact_bar_wasm_deno_tooltip_action_wire_payloads() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_tooltip_action_wire_payloads",
    );
    let mut pair = pair(&[("is_tooltip", "true")], "");
    pair.pipe(message("toggle_persisted_tooltip", true, None));
    // Wasm's action protobuf omits these pane/launch fields. Key hints must still match
    // the reference when Deno receives the richer native event.
    let launch = |extra_fields: bool, should_float| Action::LaunchPlugin {
        plugin: RunPluginOrAlias::Alias(PluginAlias::new(
            "agents",
            &None,
            extra_fields.then(|| "/tmp/alias".into()),
        )),
        should_float,
        should_open_in_place: false,
        close_replaced_pane: extra_fields,
        skip_cache: false,
        cwd: extra_fields.then(|| "/tmp/launch".into()),
        no_focus: extra_fields,
        tab_id: extra_fields.then(|| TabId::new(7)),
    };
    for (mode, bindings) in [
        (
            InputMode::Pane,
            vec![
                (
                    'n',
                    Action::NewPane {
                        direction: None,
                        pane_name: None,
                        start_suppressed: true,
                    },
                ),
                (
                    'r',
                    Action::NewPane {
                        direction: Some(Direction::Right),
                        pane_name: None,
                        start_suppressed: true,
                    },
                ),
                (
                    'd',
                    Action::NewPane {
                        direction: Some(Direction::Down),
                        pane_name: None,
                        start_suppressed: true,
                    },
                ),
                (
                    's',
                    Action::NewStackedPane {
                        command: Some(RunCommandAction {
                            command: "/usr/bin/true".into(),
                            ..Default::default()
                        }),
                        pane_name: Some("custom".into()),
                        near_current_pane: true,
                        no_focus: true,
                        tab_id: Some(TabId::new(7)),
                    },
                ),
            ],
        ),
        (
            InputMode::Agent,
            vec![
                ('a', launch(false, false)),
                ('b', launch(true, false)),
                ('c', launch(false, true)),
            ],
        ),
    ] {
        let keybinds = vec![(
            mode,
            bindings
                .into_iter()
                .map(|(key, action)| (KeyWithModifier::new(BareKey::Char(key)), vec![action]))
                .collect(),
        )];
        pair.event(Event::InitialKeybinds(keybinds));
        pair.event(Event::ModeUpdate(ModeInfo {
            mode,
            ..Default::default()
        }));
        for cols in [1, 20, 40, 80, 160] {
            pair.render(12, cols);
        }
    }
}

#[test]
fn compact_bar_wasm_deno_tooltip_pipes_and_lifecycle() {
    let _reference_trace = crate::plugin_tests::reference_trace::Scenario::new(
        "compact_bar_wasm_deno_tooltip_pipes_and_lifecycle",
    );
    let mut startup = pair(&[("is_tooltip", "true")], "");
    startup.event(Event::ModeUpdate(ModeInfo {
        mode: InputMode::Pane,
        ..Default::default()
    }));
    startup.event(Event::InitialKeybinds(shipped(InputMode::Pane).keybinds));
    startup.render(4, 8);
    startup.event(Event::ModeUpdate(ModeInfo {
        mode: InputMode::Tab,
        ..Default::default()
    }));
    startup.render(24, 120);

    let mut main = pair(&[("tooltip", "Ctrl t")], "");
    for active in [0, 1] {
        main.event(Event::TabUpdate(tabs(&["main", "other"], active)));
        for own_tab in [0, 1] {
            for tooltip in [false, true] {
                main.event(Event::PaneUpdate(manifest(own_tab, tooltip)));
                main.event(Event::PaneUpdate(manifest(own_tab, tooltip)));
                main.render(1, 120);
                for private in [false, true] {
                    for client in [None, Some("0"), Some("1")] {
                        main.pipe(message("toggle_tooltip", private, client));
                    }
                }
            }
        }
    }
    main.pipe(message("unrelated", true, Some("0")));
    main.event(Event::PaneUpdate(PaneManifest::default()));
    main.pipe(message("toggle_tooltip", true, Some("0")));
    for base_mode in [None, Some(InputMode::Locked), Some(InputMode::Pane)] {
        for mode in InputMode::iter() {
            let mut info = shipped(mode);
            info.base_mode = base_mode;
            main.event(Event::ModeUpdate(info.clone()));
            main.event(Event::ModeUpdate(info));
        }
    }
    for persisted in [false, true] {
        let mut tooltip = pair(&[("is_tooltip", "true"), ("tooltip", "Ctrl t")], "");
        tooltip.pipe(message("toggle_persisted_tooltip", false, None));
        tooltip.pipe(message("unrelated", true, None));
        if persisted {
            tooltip.pipe(message("toggle_persisted_tooltip", true, None));
        }
        for mode in [
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Normal,
            InputMode::Locked,
        ] {
            tooltip.event(Event::ModeUpdate(shipped(mode)));
        }
        for active in [0, 1, 1, 0] {
            let mut tabs = tabs(&["main", "other"], active);
            tabs[active].display_area_rows = RowCount::new(active * 10);
            tooltip.event(Event::TabUpdate(tabs));
            tooltip.event(Event::ModeUpdate(shipped(InputMode::Pane)));
        }
        for event in [
            Event::CopyToClipboard(CopyDestination::System),
            Event::SystemClipboardFailure,
            Event::InputReceived,
            Event::Mouse(Mouse::ScrollUp(1)),
            Event::Mouse(Mouse::ScrollDown(1)),
            Event::Mouse(Mouse::LeftClick(0, 0)),
        ] {
            tooltip.event(event);
        }
        tooltip.pipe(message("toggle_persisted_tooltip", true, None));
    }
}
