//! A tab opened for an agent pane gets its layout before the pane arrives. The pane must take
//! the layout's content slot whenever it arrives relative to the tab's UI plugins loading.

use super::*;
use crate::tab::swap_layouts::SwapLayouts;
use crate::tab::ExtractedPanePlacement;
use yinz_types::input::layout::{Layout, Run, RunPluginOrAlias};
use yinz_types::internal_ids::PluginId;

const COLS: usize = 240;
const ROWS: usize = 50;
const TAB_BAR: PaneId = PaneId::Plugin(PluginId::new(1));
const STATUS_BAR: PaneId = PaneId::Plugin(PluginId::new(2));
const AGENT: PaneId = PaneId::Terminal(TerminalId::new(7));

fn size(rows: usize) -> Size {
    Size { cols: COLS, rows }
}

/// The shipped default layout and swap layouts, with the plugin aliases resolved as the default
/// config resolves them: the tab bar is the builtin one, whose height follows the tab groups.
fn shipped_default_layout() -> Layout {
    let resolve = |kdl: &[u8]| {
        String::from_utf8(kdl.to_vec())
            .unwrap()
            .replace(r#"location="tab-bar""#, r#"location="yinz:tab-bar""#)
            .replace(r#"location="status-bar""#, r#"location="yinz:status-bar""#)
    };
    let layout = resolve(yinz_config::assets::DEFAULT_LAYOUT);
    let swap = resolve(yinz_config::assets::DEFAULT_SWAP_LAYOUT);
    let layout = yinz_config::kdl::layout::from_kdl(
        &layout,
        Some("default.kdl".into()),
        Some(("default.swap.kdl", &swap)),
        None,
    )
    .unwrap();
    assert!(layout
        .swap_tiled_layouts
        .iter()
        .any(|(_, name)| name.as_deref() == Some("vertical")));
    layout
}

fn plugin_ids() -> HashMap<RunPluginOrAlias, Vec<crate::PluginId>> {
    [("yinz:tab-bar", TAB_BAR), ("yinz:status-bar", STATUS_BAR)]
        .into_iter()
        .map(|(url, id)| {
            let run =
                yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, None)
                    .unwrap();
            let PaneId::Plugin(id) = id else {
                unreachable!()
            };
            (run, vec![id])
        })
        .collect()
}

fn agent_run() -> Run {
    Run::Command(RunCommand {
        command: "claude".into(),
        ..Default::default()
    })
}

/// The two runs an agent tab's layout ignores: `None` from an explicit new tab
/// (`resolve_agent_tab`), and the placeholder command from an automatic re-route out of a
/// crowded tab (`agent_pane_target`).
fn ignored_run_shapes() -> [Option<Run>; 2] {
    [None, Some(agent_run())]
}

/// A pending tab like the one the screen creates, before its layout lands.
fn pending_tab(layout: &Layout) -> Tab {
    let mut tab = create_new_tab_with_swap_layouts(
        size(ROWS),
        ModeInfo::default(),
        (vec![], vec![]),
        None,
        true,
        true,
    );
    tab.tiled_panes.drain();
    tab.is_pending = true;
    tab.swap_layouts = SwapLayouts::new(
        (
            layout.swap_tiled_layouts.clone(),
            layout.swap_floating_layouts.clone(),
        ),
        tab.display_area.clone(),
    );
    tab
}

fn land_layout(tab: &mut Tab, layout: &Layout, ignored: &Option<Run>) {
    let (mut base_layout, floating_layout) = layout.template.clone().unwrap();
    base_layout.ignore_run_instruction(ignored.clone());
    tab.apply_layout(
        base_layout,
        floating_layout,
        vec![],
        vec![],
        plugin_ids(),
        ClientId::new(1),
        None,
    )
    .unwrap();
}

/// The tab an agent spawn opens, with its layout landed and the agent pane still to come.
fn agent_tab(ignored: &Option<Run>) -> Tab {
    let layout = shipped_default_layout();
    let mut tab = pending_tab(&layout);
    tab.agent_layout_slot_expected = true;
    land_layout(&mut tab, &layout, ignored);
    assert!(!tab.agent_layout_slot_expected);
    assert!(tab.has_reserved_layout_slot());
    tab
}

fn load_plugins(tab: &mut Tab) {
    tab.set_pane_selectable(TAB_BAR, false);
    tab.set_pane_selectable(STATUS_BAR, false);
}

fn arrive(tab: &mut Tab, id: PaneId) {
    tab.new_pane(
        id,
        None,
        Some(agent_run()),
        false,
        false,
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
}

fn geom(tab: &Tab, id: PaneId) -> (usize, usize, usize, usize) {
    let geom = tab.get_pane_with_id(id).unwrap().position_and_size();
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

/// The tab bar on top, the status bar as a single bottom row, the agent filling the rows
/// between them and the tab back on its clean base layout.
fn assert_agent_fills_content(tab: &mut Tab, rows: usize, tab_bar_rows: usize) {
    let assert_geometry = |tab: &Tab, when: &str| {
        let content_rows = rows - tab_bar_rows - 1;
        assert!(tab.get_pane_with_id(TAB_BAR).unwrap().is_builtin_tab_bar());
        assert_eq!(
            geom(tab, TAB_BAR),
            (0, 0, COLS, tab_bar_rows),
            "tab bar {when}"
        );
        assert_eq!(
            geom(tab, AGENT),
            (0, tab_bar_rows, COLS, content_rows),
            "agent {when}"
        );
        assert_eq!(
            geom(tab, STATUS_BAR),
            (0, rows - 1, COLS, 1),
            "status bar {when}"
        );
        let viewport = tab.get_viewport();
        assert_eq!(
            (viewport.x, viewport.y, viewport.cols, viewport.rows),
            (0, tab_bar_rows, COLS, content_rows),
            "viewport {when}"
        );
        assert_eq!(
            tab.swap_layout_info(),
            (Some("BASE".into()), false),
            "swap layout {when}"
        );
        assert!(!tab.has_reserved_layout_slot(), "reservation {when}");
        assert!(
            tab.get_tiled_panes().any(|(id, _)| *id == AGENT),
            "agent tiled {when}"
        );
        assert_eq!(tab.get_suppressed_panes().count(), 0, "suppressed {when}");
    };
    assert_geometry(tab, "after arrival");
    // A tab-bar sync or relayout after the arrival must have nothing left to change.
    tab.resize_whole_tab_to(size(rows)).unwrap();
    assert_geometry(tab, "after a same-size resize");
}

#[test]
fn agent_arriving_while_ui_plugins_are_selectable_fills_the_content_slot() {
    for ignored in ignored_run_shapes() {
        let mut tab = agent_tab(&ignored);
        assert_eq!(tab.tiled_content_pane_count(), 0, "{ignored:?}");
        assert_eq!(tab.worker_split_anchor(), None, "{ignored:?}");

        arrive(&mut tab, AGENT);
        load_plugins(&mut tab);

        assert_agent_fills_content(&mut tab, ROWS, 1);
    }
}

#[test]
fn agent_arriving_after_ui_plugins_load_fills_the_content_slot() {
    for ignored in ignored_run_shapes() {
        let mut tab = agent_tab(&ignored);
        load_plugins(&mut tab);

        arrive(&mut tab, AGENT);

        assert_agent_fills_content(&mut tab, ROWS, 1);
    }
}

#[test]
fn agent_arriving_after_a_resize_fills_the_content_slot() {
    for ignored in ignored_run_shapes() {
        for plugins_load_before_resize in [true, false] {
            let mut tab = agent_tab(&ignored);
            if plugins_load_before_resize {
                load_plugins(&mut tab);
            }
            tab.resize_whole_tab_to(size(88)).unwrap();
            if !plugins_load_before_resize {
                load_plugins(&mut tab);
            }
            assert!(tab.has_reserved_layout_slot());

            arrive(&mut tab, AGENT);

            assert_agent_fills_content(&mut tab, 88, 1);
        }
    }
}

#[test]
fn agent_arriving_with_two_tab_groups_keeps_the_taller_tab_bar() {
    for ignored in ignored_run_shapes() {
        let mut tab = agent_tab(&ignored);
        load_plugins(&mut tab);
        tab.set_tab_group_count(2).unwrap();

        arrive(&mut tab, AGENT);

        assert_agent_fills_content(&mut tab, ROWS, 2);
    }
}

#[test]
fn only_tabs_opened_for_an_agent_reserve_a_layout_slot() {
    let layout = shipped_default_layout();

    // As project arrangement lands a new tab: the content run is ignored with no pane present,
    // and no agent is expected.
    let mut tab = pending_tab(&layout);
    land_layout(&mut tab, &layout, &None);
    assert!(!tab.has_reserved_layout_slot());

    // A pane that is already in the pending tab takes its slot; nothing is held.
    let mut tab = pending_tab(&layout);
    tab.agent_layout_slot_expected = true;
    let pane = tab
        .build_pane(AGENT, None, Some(agent_run()), None)
        .unwrap();
    tab.add_tiled_pane(pane, AGENT, false, None).unwrap();
    land_layout(&mut tab, &layout, &Some(agent_run()));
    assert!(!tab.agent_layout_slot_expected);
    assert!(!tab.has_reserved_layout_slot());
    load_plugins(&mut tab);
    assert_agent_fills_content(&mut tab, ROWS, 1);
}

#[test]
fn arranged_panes_take_the_layout_and_leave_swap_layouts_working() {
    let layout = shipped_default_layout();
    for agent_expected in [false, true] {
        let mut tab = pending_tab(&layout);
        tab.agent_layout_slot_expected = agent_expected;
        land_layout(&mut tab, &layout, &None);
        assert_eq!(tab.has_reserved_layout_slot(), agent_expected);
        load_plugins(&mut tab);

        let moved = [
            PaneId::Terminal(TerminalId::new(11)),
            PaneId::Terminal(TerminalId::new(12)),
        ];
        for id in moved {
            let pane = tab.build_pane(id, None, None, None).unwrap();
            assert!(matches!(
                tab.accept_arranged_pane(pane, ExtractedPanePlacement::Tiled),
                crate::tab::AcceptedPane::Accepted
            ));
        }
        tab.arrange_existing_panes(&TiledPaneLayout::default(), &moved)
            .unwrap();
        assert!(!tab.has_reserved_layout_slot(), "{agent_expected}");

        // An ordinary new pane finds room beside the arranged ones.
        let third = PaneId::Terminal(TerminalId::new(13));
        let pane = tab.build_pane(third, None, None, None).unwrap();
        tab.add_tiled_pane(pane, third, false, None).unwrap();
        assert_eq!(tab.tiled_content_pane_count(), 3, "{agent_expected}");
        assert_eq!(tab.get_suppressed_panes().count(), 0, "{agent_expected}");
        for id in moved {
            assert_ne!(geom(&tab, id), geom(&tab, third), "{agent_expected}");
        }

        let before = tab.swap_layout_info();
        tab.next_swap_layout().unwrap();
        assert_ne!(tab.swap_layout_info(), before, "{agent_expected}");
    }
}

#[test]
fn a_tiled_pane_without_room_is_parked_rather_than_dropped() {
    let mut tab = create_new_tab(Size { cols: 20, rows: 10 }, ModeInfo::default());
    assert!(!tab.is_pending());
    let mut next = 2;
    while tab.tiled_panes.has_room_for_new_pane() {
        assert!(next < 100, "the tab never ran out of room");
        let id = PaneId::Terminal(TerminalId::new(next));
        let pane = tab.build_pane(id, None, None, None).unwrap();
        tab.add_tiled_pane(pane, id, false, None).unwrap();
        next += 1;
    }
    assert_eq!(tab.get_suppressed_panes().count(), 0);
    let id = PaneId::Terminal(TerminalId::new(next));
    let pane = tab.build_pane(id, None, None, None).unwrap();
    tab.add_tiled_pane(pane, id, false, None).unwrap();

    assert!(!tab.get_tiled_panes().any(|(pane_id, _)| *pane_id == id));
    assert!(tab
        .get_suppressed_panes()
        .any(|(pane_id, _)| *pane_id == id));
    assert!(tab.has_pane_with_pid(&id));
}

fn plugin_tab(plugin_borderless: bool, with_terminal: bool) -> Tab {
    let (size, terminal) = if with_terminal {
        ("size=10", "pane")
    } else {
        ("", "")
    };
    let layout = yinz_config::kdl::layout::from_kdl(
        &format!(
            r#"
            layout {{
                pane {size} borderless={plugin_borderless} {{ plugin location="yinz:tab-bar"; }}
                {terminal}
            }}
            "#
        ),
        None,
        None,
        None,
    )
    .unwrap();
    let mut tab = pending_tab(&layout);
    let (base_layout, floating_layout) = layout.template.clone().unwrap();
    let terminals = if with_terminal {
        vec![(TerminalId::new(1), None)]
    } else {
        vec![]
    };
    let mut plugin_ids = plugin_ids();
    plugin_ids.retain(|_, ids| ids.contains(&PluginId::new(1)));
    tab.apply_layout(
        base_layout,
        floating_layout,
        terminals,
        vec![],
        plugin_ids,
        ClientId::new(1),
        None,
    )
    .unwrap();
    tab
}

#[test]
fn borderless_plugin_panes_are_never_split_targets() {
    // The plugin has not loaded, so it is still selectable.
    let mut tab = plugin_tab(true, true);
    assert!(tab.get_pane_with_id(TAB_BAR).unwrap().selectable());
    assert_eq!(tab.tiled_content_pane_count(), 1);
    assert_eq!(
        tab.worker_split_anchor(),
        Some(PaneId::Terminal(TerminalId::new(1)))
    );
    let plugin_before = geom(&tab, TAB_BAR);
    let id = PaneId::Terminal(TerminalId::new(2));
    let pane = tab.build_pane(id, None, None, None).unwrap();
    tab.add_tiled_pane(pane, id, true, None).unwrap();
    assert_eq!(geom(&tab, TAB_BAR), plugin_before);
    assert!(tab.get_tiled_panes().any(|(pane_id, _)| *pane_id == id));

    // A borderless plugin alone offers no room at all.
    let mut tab = plugin_tab(true, false);
    assert_eq!(tab.tiled_content_pane_count(), 0);
    assert_eq!(tab.worker_split_anchor(), None);
}

#[test]
fn framed_selectable_plugin_panes_remain_split_targets() {
    let mut tab = plugin_tab(false, false);
    assert_eq!(tab.tiled_content_pane_count(), 1);
    assert_eq!(tab.worker_split_anchor(), Some(TAB_BAR));
    assert!(tab.tiled_panes.has_room_for_new_pane());
}
