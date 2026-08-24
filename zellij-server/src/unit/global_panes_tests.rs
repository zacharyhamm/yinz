use super::{allocate_global_panes, GlobalPaneManager};
use crate::panes::grid::Grid;
use crate::panes::terminal_character::AnsiCode;
use crate::{
    channels::SenderWithContext,
    output::Output,
    panes::{KittyImageStore, LinkHandler, PaneId, PluginPane, SixelImageStore},
    plugins::PluginInstruction,
    tab::Pane,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
};
use zellij_utils::data::{Palette, Style};
use zellij_utils::{
    channels,
    data::{
        GlobalPaneEdge, GlobalPanePlacement, ModeInfo, PaletteColor, PaneId as ZellijUtilsPaneId,
    },
    input::layout::PercentOrFixed,
    pane_size::{Dimension, Offset, PaneGeom, SizeInPixels, Viewport},
};

fn placement(edge: GlobalPaneEdge, size: PercentOrFixed) -> GlobalPanePlacement {
    GlobalPanePlacement {
        edge,
        size,
        length: None,
    }
}

fn geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
}

fn mock_pane(pane_id: PaneId) -> Box<dyn Pane> {
    let PaneId::Plugin(plugin_id) = pane_id else {
        panic!("global pane test mock must be a plugin")
    };
    let (plugin_sender, _plugin_receiver) = channels::unbounded();
    Box::new(PluginPane::new(
        plugin_id,
        PaneGeom::default(),
        SenderWithContext::<PluginInstruction>::new(plugin_sender),
        String::new(),
        String::new(),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None::<SizeInPixels>)),
        vec![],
        Style::default(),
        None,
        false,
        true,
        true,
    ))
}

fn left_percent(percent: usize) -> GlobalPanePlacement {
    placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(percent))
}

fn placement_with_length(
    edge: GlobalPaneEdge,
    size: PercentOrFixed,
    length: Option<PercentOrFixed>,
) -> GlobalPanePlacement {
    GlobalPanePlacement { edge, size, length }
}

#[test]
fn same_edge_panes_split_the_strip_equally_by_default() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 41,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        ),
        (
            ZellijUtilsPaneId::Plugin(2),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        ),
    ];

    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    // 41 rows split two ways: the earlier pane absorbs the odd cell
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(1)],
        geom(0, 0, 20, 21)
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(2)],
        geom(0, 21, 20, 20)
    );
    assert_eq!(
        layout.center,
        Viewport {
            x: 20,
            y: 0,
            cols: 80,
            rows: 41,
        }
    );
}

#[test]
fn explicit_lengths_take_their_share_and_none_panes_divide_the_rest() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement_with_length(
                GlobalPaneEdge::Left,
                PercentOrFixed::Fixed(20),
                Some(PercentOrFixed::Percent(25)),
            ),
        ),
        (
            ZellijUtilsPaneId::Plugin(2),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        ),
        (
            ZellijUtilsPaneId::Plugin(3),
            placement_with_length(
                GlobalPaneEdge::Left,
                PercentOrFixed::Fixed(20),
                Some(PercentOrFixed::Fixed(6)),
            ),
        ),
    ];

    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    // 25% of 40 rows = 10, fixed 6, the lone None pane takes the remaining 24
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(1)],
        geom(0, 0, 20, 10)
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(2)],
        geom(0, 10, 20, 24)
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(3)],
        geom(0, 34, 20, 6)
    );
}

#[test]
fn all_explicit_lengths_falling_short_stretch_the_last_pane() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement_with_length(
                GlobalPaneEdge::Bottom,
                PercentOrFixed::Fixed(10),
                Some(PercentOrFixed::Fixed(30)),
            ),
        ),
        (
            ZellijUtilsPaneId::Plugin(2),
            placement_with_length(
                GlobalPaneEdge::Bottom,
                PercentOrFixed::Fixed(10),
                Some(PercentOrFixed::Fixed(30)),
            ),
        ),
    ];

    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    // bottom strip panes sit side by side; 30 + 30 leaves 40 columns for the last pane
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(1)],
        geom(0, 30, 30, 10)
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(2)],
        geom(30, 30, 70, 10)
    );
}

#[test]
fn overflowing_lengths_error_strictly_and_fall_back_to_equal_shares_when_clamped() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement_with_length(
                GlobalPaneEdge::Left,
                PercentOrFixed::Fixed(20),
                Some(PercentOrFixed::Fixed(30)),
            ),
        ),
        (
            ZellijUtilsPaneId::Plugin(2),
            placement_with_length(
                GlobalPaneEdge::Left,
                PercentOrFixed::Fixed(20),
                Some(PercentOrFixed::Fixed(30)),
            ),
        ),
    ];

    let strict = allocate_global_panes(area, &panes, false, (0, 0));
    assert_eq!(
        strict.unwrap_err().to_string(),
        "global panes do not fit their strip"
    );

    let clamped = allocate_global_panes(area, &panes, true, (0, 0)).unwrap();
    assert_eq!(
        clamped.panes[&ZellijUtilsPaneId::Plugin(1)],
        geom(0, 0, 20, 20)
    );
    assert_eq!(
        clamped.panes[&ZellijUtilsPaneId::Plugin(2)],
        geom(0, 20, 20, 20)
    );
}

#[test]
fn more_panes_than_strip_cells_is_rejected_in_both_modes() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 2,
    };
    let panes: Vec<_> = (1..=3)
        .map(|index| {
            (
                ZellijUtilsPaneId::Plugin(index),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
            )
        })
        .collect();

    for clamp in [false, true] {
        let result = allocate_global_panes(area, &panes, clamp, (0, 0));
        assert_eq!(
            result.unwrap_err().to_string(),
            "global panes do not fit their strip"
        );
    }
}

#[test]
fn same_edge_joiners_adopt_the_first_panes_thickness() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        ),
        (
            ZellijUtilsPaneId::Plugin(2),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(35)),
        ),
    ];

    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(2)].cols.as_usize(),
        20
    );
    assert_eq!(layout.center.cols, 80);
}

#[test]
fn left_then_bottom_allocates_outer_to_inner() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Plugin(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        ),
        (
            ZellijUtilsPaneId::Terminal(2),
            placement(GlobalPaneEdge::Bottom, PercentOrFixed::Fixed(10)),
        ),
    ];

    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Plugin(1)],
        geom(0, 0, 20, 40)
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Terminal(2)],
        geom(20, 30, 80, 10)
    );
    assert_eq!(
        layout.center,
        Viewport {
            x: 20,
            y: 0,
            cols: 80,
            rows: 30
        }
    );
}

#[test]
fn percentages_use_the_remaining_axis() {
    let area = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        (
            ZellijUtilsPaneId::Terminal(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(20)),
        ),
        (
            ZellijUtilsPaneId::Terminal(2),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
        ),
    ];
    let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Terminal(1)]
            .cols
            .as_usize(),
        20
    );
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Terminal(2)]
            .cols
            .as_usize(),
        20
    );
    assert_eq!(layout.center.cols, 60);
}

#[test]
fn allocation_rejects_a_zero_sized_center() {
    let result = allocate_global_panes(
        Viewport {
            x: 0,
            y: 0,
            cols: 20,
            rows: 10,
        },
        &[(
            ZellijUtilsPaneId::Terminal(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        )],
        false,
        (0, 0),
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        "global panes leave no usable tab area"
    );
}

#[test]
fn clamped_allocation_keeps_an_oversized_pane_and_a_one_cell_tab_area() {
    let layout = allocate_global_panes(
        Viewport {
            x: 0,
            y: 0,
            cols: 20,
            rows: 10,
        },
        &[(
            ZellijUtilsPaneId::Terminal(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
        )],
        true,
        (0, 0),
    )
    .unwrap();
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Terminal(1)],
        geom(0, 0, 19, 10)
    );
    assert_eq!(
        layout.center,
        Viewport {
            x: 19,
            y: 0,
            cols: 1,
            rows: 10
        }
    );
}

#[test]
fn clamped_allocation_keeps_a_percentage_that_rounds_to_zero_at_one_cell() {
    let layout = allocate_global_panes(
        Viewport {
            x: 0,
            y: 0,
            cols: 4,
            rows: 10,
        },
        &[(
            ZellijUtilsPaneId::Terminal(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(20)),
        )],
        true,
        (0, 0),
    )
    .unwrap();
    assert_eq!(
        layout.panes[&ZellijUtilsPaneId::Terminal(1)],
        geom(0, 0, 1, 10)
    );
    assert_eq!(layout.center.cols, 3);
}

#[test]
fn clamped_allocation_still_rejects_an_axis_too_small_for_a_pane_and_a_tab_area() {
    let result = allocate_global_panes(
        Viewport {
            x: 0,
            y: 0,
            cols: 1,
            rows: 10,
        },
        &[(
            ZellijUtilsPaneId::Terminal(1),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(1)),
        )],
        true,
        (0, 0),
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        "global panes leave no usable tab area"
    );
}

#[test]
fn splitting_an_unconstrained_pane_preserves_its_siblings_size() {
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        Default::default(),
    );
    let placement = placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20));
    manager
        .insert(mock_pane(PaneId::Plugin(1)), placement, false)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), placement, false)
        .unwrap();
    manager.resize(area).unwrap();

    manager
        .insert_split(mock_pane(PaneId::Plugin(3)), PaneId::Plugin(1), false, None)
        .unwrap();

    assert_eq!(
        manager
            .pane(PaneId::Plugin(1))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10
    );
    assert_eq!(
        manager
            .pane(PaneId::Plugin(3))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10
    );
    assert_eq!(
        manager
            .pane(PaneId::Plugin(2))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20
    );
}

#[test]
fn manager_preserves_order_and_ownership_through_visibility_changes() {
    let full_area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::default();

    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    assert_eq!(manager.center(full_area).unwrap().cols, 90);
    manager.hide(PaneId::Plugin(1)).unwrap();
    assert_eq!(manager.center(full_area).unwrap().cols, 120);
    manager.show(PaneId::Plugin(1), full_area).unwrap();
    assert_eq!(
        manager.pane(PaneId::Plugin(1)).unwrap().pid(),
        PaneId::Plugin(1)
    );
    manager.focus_pane(PaneId::Plugin(1), 7).unwrap();
    assert_eq!(manager.focused_pane_id(7), Some(PaneId::Plugin(1)));
    manager.defocus_pane(7);
    assert_eq!(manager.focused_pane_id(7), None);
    manager.focus_pane(PaneId::Plugin(1), 7).unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(10), true)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(3)), left_percent(10), true)
        .unwrap();
    assert_eq!(
        manager.remove(PaneId::Plugin(2)).unwrap().0.pid(),
        PaneId::Plugin(2)
    );
    assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
    assert_eq!(manager.pane_indices.get(&PaneId::Plugin(3)), Some(&1));
    assert_eq!(
        manager
            .panes
            .iter()
            .map(|global_pane| global_pane.pane.pid())
            .collect::<Vec<_>>(),
        vec![PaneId::Plugin(1), PaneId::Plugin(3)]
    );
    assert!(manager.remove(PaneId::Plugin(1)).is_some());
    assert_eq!(manager.focused_pane_id(7), None);
}

#[test]
fn failed_show_preserves_hidden_state() {
    let full_area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::default();
    manager
        .insert(
            mock_pane(PaneId::Plugin(1)),
            placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(120)),
            true,
        )
        .unwrap();

    assert!(manager.show(PaneId::Plugin(1), full_area).is_err());
    assert_eq!(manager.center(full_area).unwrap(), full_area);
}

#[test]
fn failed_placement_change_preserves_prior_placement() {
    let full_area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::default();
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();

    assert!(manager
        .set_placement(
            PaneId::Plugin(1),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(120)),
            full_area,
        )
        .is_err());
    assert_eq!(manager.center(full_area).unwrap().cols, 90);
}

#[test]
fn hidden_placement_change_is_validated_without_revealing_the_pane() {
    let full_area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::default();
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), true)
        .unwrap();

    assert!(manager
        .set_placement(
            PaneId::Plugin(1),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(120)),
            full_area,
        )
        .is_err());

    assert_eq!(manager.placement(PaneId::Plugin(1)), Some(left_percent(25)));
    assert!(manager.is_hidden(PaneId::Plugin(1)).unwrap());
    assert_eq!(manager.center(full_area).unwrap(), full_area);
}

#[test]
fn resize_send_failure_restores_manager_and_pane_state() {
    let full_area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let (plugin_sender, plugin_receiver) = channels::unbounded();
    drop(plugin_receiver);
    let senders = crate::thread_bus::ThreadSenders {
        to_plugin: Some(SenderWithContext::new(plugin_sender)),
        ..Default::default()
    };
    let mut manager =
        GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), true)
        .unwrap();
    let original_geom = manager.pane(PaneId::Plugin(1)).unwrap().position_and_size();
    let original_offset = manager
        .pane(PaneId::Plugin(1))
        .unwrap()
        .get_content_offset();
    let original_should_render = manager.pane(PaneId::Plugin(1)).unwrap().should_render();

    assert!(manager.show(PaneId::Plugin(1), full_area).is_err());

    assert!(manager.visible_pane_ids().is_empty());
    assert_eq!(manager.area, None);
    assert_eq!(manager.panes[0].placement, left_percent(25));
    assert!(manager.panes[0].hidden);
    assert!(!manager.panes[0].layout_applied);
    assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
    let pane = manager.pane(PaneId::Plugin(1)).unwrap();
    assert_eq!(pane.position_and_size(), original_geom);
    assert_eq!(pane.get_content_offset(), original_offset);
    assert_eq!(pane.should_render(), original_should_render);

    manager.resize(full_area).unwrap();
    assert!(manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
        .is_err());
    assert!(manager.pane(PaneId::Plugin(2)).is_none());
    assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
    assert_eq!(manager.panes.len(), 1);

    let (plugin_sender, plugin_receiver) = channels::unbounded();
    let senders = crate::thread_bus::ThreadSenders {
        to_plugin: Some(SenderWithContext::new(plugin_sender)),
        ..Default::default()
    };
    let mut visible_manager =
        GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
    visible_manager
        .insert(mock_pane(PaneId::Plugin(10)), left_percent(25), true)
        .unwrap();
    visible_manager.show(PaneId::Plugin(10), full_area).unwrap();
    plugin_receiver.recv().unwrap();
    let original_geom = visible_manager
        .pane(PaneId::Plugin(10))
        .unwrap()
        .position_and_size();
    let original_offset = visible_manager
        .pane(PaneId::Plugin(10))
        .unwrap()
        .get_content_offset();
    let original_should_render = visible_manager
        .pane(PaneId::Plugin(10))
        .unwrap()
        .should_render();
    drop(plugin_receiver);

    assert!(visible_manager
        .set_placement(
            PaneId::Plugin(10),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(20)),
            full_area,
        )
        .is_err());
    assert_eq!(visible_manager.panes[0].placement, left_percent(25));
    assert_eq!(visible_manager.area, Some(full_area));
    let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
    assert_eq!(pane.position_and_size(), original_geom);
    assert_eq!(pane.get_content_offset(), original_offset);
    assert_eq!(pane.should_render(), original_should_render);

    assert!(visible_manager
        .resize(Viewport {
            cols: 160,
            rows: 50,
            ..Default::default()
        })
        .is_err());
    assert_eq!(visible_manager.area, Some(full_area));
    let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
    assert_eq!(pane.position_and_size(), original_geom);
    assert_eq!(pane.get_content_offset(), original_offset);
    assert_eq!(pane.should_render(), original_should_render);

    assert!(visible_manager
        .set_pane_frame_style(zellij_utils::input::options::PaneFrameStyle::Full)
        .is_err());
    assert_eq!(
        visible_manager.pane_frame_style,
        zellij_utils::input::options::PaneFrameStyle::Titles
    );
    let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
    assert_eq!(pane.position_and_size(), original_geom);
    assert_eq!(pane.get_content_offset(), original_offset);
    assert_eq!(pane.should_render(), original_should_render);
}

#[test]
fn title_frames_reserve_the_top_content_row() {
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Titles,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager
        .resize(Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        })
        .unwrap();

    let pane = manager.pane(PaneId::Plugin(1)).unwrap();
    // right: 1 is the boundary cell, see `the_edge_facing_the_tab_reserves_a_boundary_cell`
    assert_eq!(
        pane.get_content_offset(),
        Offset::shift_right_top_and_bottom(1, 1, 0)
    );
    assert_eq!(pane.get_content_rows(), 39);
}

#[test]
fn global_boundaries_reserve_only_the_trailing_edges_something_draws_on() {
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::None,
    );
    manager.set_chrome_rows((2, 3));
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();

    let left = manager.pane(PaneId::Plugin(1)).unwrap();
    let left_geom = left.position_and_size();
    // only the column facing the tab: the band stops above the tab's bottom chrome, so nothing
    // draws a line under it and the last row stays the pane's own
    assert_eq!(
        left.get_content_offset(),
        Offset {
            right: 1,
            ..Default::default()
        }
    );
    assert!(!manager.owns_boundary_cell(left_geom.x, left_geom.y - 1));
    assert!(!manager.owns_boundary_cell(
        left_geom.x + left_geom.cols.as_usize() - 1,
        left_geom.y + left_geom.rows.as_usize() - 1,
    ));
    assert!(!manager.owns_boundary_cell(
        left_geom.x + left_geom.cols.as_usize() - 2,
        left_geom.y + left_geom.rows.as_usize() - 1,
    ));

    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::None,
    );
    manager
        .insert(
            mock_pane(PaneId::Plugin(2)),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
            false,
        )
        .unwrap();
    manager
        .insert(
            mock_pane(PaneId::Plugin(3)),
            placement(GlobalPaneEdge::Top, PercentOrFixed::Percent(25)),
            false,
        )
        .unwrap();
    manager.resize(area).unwrap();

    assert_eq!(
        manager
            .pane(PaneId::Plugin(3))
            .unwrap()
            .get_content_offset(),
        Offset {
            right: 1,
            bottom: 1,
            ..Default::default()
        }
    );

    let top_geom = manager.pane(PaneId::Plugin(3)).unwrap().position_and_size();
    assert!(!manager.owns_boundary_cell(
        top_geom.x + top_geom.cols.as_usize() - 1,
        top_geom.y + top_geom.rows.as_usize() - 1,
    ));
}

#[test]
fn a_band_that_stops_above_the_chrome_keeps_its_last_row_for_contents() {
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::None,
    );
    manager.set_chrome_rows((0, 3));
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();

    let left = manager.pane(PaneId::Plugin(1)).unwrap();
    let geom = left.position_and_size();
    assert!(
        geom.y + geom.rows.as_usize() < area.y + area.rows,
        "the band should stop above the tab's bottom chrome"
    );
    assert_eq!(left.get_content_offset().bottom, 0);
    assert_eq!(left.get_content_rows(), geom.rows.as_usize());
    assert!(!manager.owns_boundary_cell(geom.x, geom.y + geom.rows.as_usize() - 1));
}

#[test]
fn a_band_that_abuts_a_bottom_global_pane_keeps_its_separator() {
    // the Bottom pane's top boundary and the band's bottom boundary are the same cell, so the
    // band has to leave it - without the reservation the line would land on its contents
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::None,
    );
    manager
        .insert(
            mock_pane(PaneId::Plugin(1)),
            placement(GlobalPaneEdge::Bottom, PercentOrFixed::Percent(25)),
            false,
        )
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();

    let left = manager.pane(PaneId::Plugin(2)).unwrap();
    let geom = left.position_and_size();
    let bottom_geom = manager.pane(PaneId::Plugin(1)).unwrap().position_and_size();
    assert_eq!(
        geom.y + geom.rows.as_usize(),
        bottom_geom.y,
        "the band should abut the bottom strip with no chrome between them"
    );
    assert_eq!(left.get_content_offset().bottom, 1);
    assert!(manager.owns_boundary_cell(geom.x, geom.y + geom.rows.as_usize() - 1));
}

#[test]
fn the_last_row_of_a_chrome_inset_strip_renders_no_separator() {
    let ((separator_row, cols, grid), last_row) = render_left_strip(None, true, (0, 3));
    for x in 0..cols - 1 {
        assert_eq!(
            grid.viewport[separator_row].columns[x].character, '─',
            "column {x} of the row the two panes share",
        );
        // a cell nothing wrote is not in the grid at all, which is just as much "no line"
        assert_ne!(
            grid.viewport
                .get(last_row)
                .and_then(|row| row.columns.get(x))
                .map(|column| column.character),
            Some('─'),
            "column {x} of the strip's last row, which has only the tab's chrome below it",
        );
    }
}

#[test]
fn quiet_global_pane_render_diffs_boundaries() {
    let client_id = 1;
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::None,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();

    let mut output = Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(None)),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    );
    output.add_clients(
        &HashSet::from([client_id]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    let mode_info = BTreeMap::new();
    let default_mode_info = ModeInfo::default();

    manager
        .render(
            &mut output,
            &[client_id],
            &mode_info,
            &default_mode_info,
            Style::default(),
            false,
            false,
        )
        .unwrap();
    assert!(output.is_dirty());
    output.serialize().unwrap();
    output.add_clients(
        &HashSet::from([client_id]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    manager.resize(area).unwrap();

    manager
        .render(
            &mut output,
            &[client_id],
            &mode_info,
            &default_mode_info,
            Style::default(),
            false,
            false,
        )
        .unwrap();
    assert!(!output.is_dirty());

    manager.set_force_render();
    manager
        .render(
            &mut output,
            &[client_id],
            &mode_info,
            &default_mode_info,
            Style::default(),
            false,
            false,
        )
        .unwrap();
    assert!(output.is_dirty());
}

#[test]
fn borderless_full_global_panes_render_reserved_boundaries() {
    let client_id = 1;
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Full,
    );
    manager.set_chrome_rows((2, 3));
    let mut pane = mock_pane(PaneId::Plugin(1));
    pane.set_borderless(true);
    manager.insert(pane, left_percent(25), false).unwrap();
    let mut second_pane = mock_pane(PaneId::Plugin(2));
    second_pane.set_borderless(true);
    manager
        .insert(second_pane, left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();

    let mut output = Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(None)),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    );
    output.add_clients(
        &HashSet::from([client_id]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );

    manager
        .render(
            &mut output,
            &[client_id],
            &BTreeMap::new(),
            &ModeInfo::default(),
            Style::default(),
            false,
            false,
        )
        .unwrap();
    let rendered = output.serialize().unwrap().remove(&client_id).unwrap();
    assert!(rendered.contains('─'));
}

/// Two panes in a Left strip, `Titles` frames, `pane_id` focused for client 1. Returns the
/// separator row's y and the strip's width, plus the rendered output replayed into a grid.
fn render_left_strip_with_focus(focus: Option<PaneId>, force_render: bool) -> (usize, usize, Grid) {
    render_left_strip(focus, force_render, (0, 0)).0
}

/// As `render_left_strip_with_focus`, with the tab chrome the band gives back. Also returns the
/// lower pane's last row, which is a boundary only when something draws there.
fn render_left_strip(
    focus: Option<PaneId>,
    force_render: bool,
    chrome_rows: (usize, usize),
) -> ((usize, usize, Grid), usize) {
    let client_id = 1;
    let area = Viewport {
        cols: 40,
        rows: 20,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Titles,
    );
    manager.set_chrome_rows(chrome_rows);
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();
    if let Some(pane_id) = focus {
        manager.focus_pane(pane_id, client_id).unwrap();
    }
    if force_render {
        // what `Screen::focus_global_pane` does on every focus change
        manager.set_force_render();
    }

    let mut output = Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(None)),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    );
    output.add_clients(
        &HashSet::from([client_id]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    manager
        .render(
            &mut output,
            &[client_id],
            &BTreeMap::new(),
            &ModeInfo::default(),
            Style::default(),
            false,
            false,
        )
        .unwrap();
    let rendered = output.serialize().unwrap().remove(&client_id).unwrap();

    let upper = manager.pane(PaneId::Plugin(1)).unwrap().position_and_size();
    let separator_row = upper.y + upper.rows.as_usize() - 1;
    let mut grid = Grid::new(
        area.rows,
        area.cols,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, rendered.as_bytes());
    let lower = manager.pane(PaneId::Plugin(2)).unwrap().position_and_size();
    let lower_last_row = lower.y + lower.rows.as_usize() - 1;
    ((separator_row, upper.cols.as_usize(), grid), lower_last_row)
}

#[test]
fn a_focused_global_pane_colors_the_separator_it_shares_with_its_strip_sibling() {
    // whichever side of the separator is focused wins the row: for the upper pane it is its
    // bottom boundary, for the lower one its top boundary, merged by precedence
    for focused in [PaneId::Plugin(1), PaneId::Plugin(2)] {
        let (separator_row, cols, grid) = render_left_strip_with_focus(Some(focused), true);
        let row = &grid.viewport[separator_row];
        // the last column is the cell the strip cedes to the tab, which renders nothing here
        for x in 0..cols - 1 {
            assert_eq!(
                row.columns[x].character, '─',
                "column {x} of the separator row with {focused:?} focused"
            );
            assert_eq!(
                row.columns[x].styles.foreground,
                Some(AnsiCode::ColorIndex(154)),
                "column {x} of the separator row is not the focused frame colour with \
                 {focused:?} focused"
            );
        }
    }
}

#[test]
fn an_unfocused_strip_draws_its_separator_without_a_colour() {
    let (separator_row, cols, grid) = render_left_strip_with_focus(None, true);
    let row = &grid.viewport[separator_row];
    for x in 0..cols - 1 {
        assert_eq!(row.columns[x].character, '─');
        // `frame_unselected` is None by default, so the glyph is drawn with the default
        // foreground rather than a colour of its own
        assert_eq!(
            row.columns[x].styles.foreground,
            Some(AnsiCode::Reset),
            "column {x} of the separator row is coloured while nothing is focused"
        );
    }
}

#[test]
fn tab_facing_borders_carry_each_global_panes_own_frame_colour() {
    let area = Viewport {
        cols: 40,
        rows: 20,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Titles,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();
    manager.focus_pane(PaneId::Plugin(2), 1).unwrap();
    let layout = manager.tab_layout(area).unwrap();

    let borders = manager.tab_facing_borders(
        &layout,
        &[1, 2],
        &BTreeMap::new(),
        &ModeInfo::default(),
        Style::default(),
        // mirrored, so both clients share `frame_selected` rather than taking per-user colours
        true,
        true,
    );
    assert_eq!(borders.len(), 2);
    // only the focused pane contributes, and only for the client focused on it
    assert!(borders[0].colors.is_empty());
    assert_eq!(
        borders[1].colors,
        BTreeMap::from([(1, (PaletteColor::EightBit(154), 3))])
    );
    // the line each of them wants coloured is the column the strip cedes to the tab
    for border in &borders {
        let (x, y, length, vertical) = border.tab_facing_line();
        let geom = &border.geom;
        assert!(vertical);
        assert_eq!(x, geom.x + geom.cols.as_usize() - 1);
        assert_eq!((y, length), (geom.y, geom.rows.as_usize()));
        assert!(!manager.owns_boundary_cell(x, y));
    }
}

#[test]
fn tab_facing_borders_follow_the_clients_input_mode() {
    let area = Viewport {
        cols: 40,
        rows: 20,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Titles,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();
    manager.focus_pane(PaneId::Plugin(1), 1).unwrap();
    let layout = manager.tab_layout(area).unwrap();
    let mode_info = BTreeMap::from([(
        1,
        ModeInfo {
            mode: zellij_utils::data::InputMode::Pane,
            ..Default::default()
        },
    )]);

    let borders = manager.tab_facing_borders(
        &layout,
        &[1],
        &mode_info,
        &ModeInfo::default(),
        Style::default(),
        false,
        false,
    );
    // a non-Normal mode swaps `frame_selected` for `frame_highlight` - the same rule a tab pane
    // follows, because both go through `ui::pane_contents_and_ui::frame_color`
    assert_eq!(
        borders[0].colors,
        BTreeMap::from([(1, (PaletteColor::EightBit(166), 3))])
    );
}

#[test]
fn full_frames_share_no_border_with_the_tab() {
    let area = Viewport {
        cols: 40,
        rows: 20,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Full,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager.resize(area).unwrap();
    manager.focus_pane(PaneId::Plugin(1), 1).unwrap();
    let layout = manager.tab_layout(area).unwrap();

    // each pane draws its own frame over its reserved cells, so there is nothing to recolour
    assert!(manager
        .tab_facing_borders(
            &layout,
            &[1],
            &BTreeMap::new(),
            &ModeInfo::default(),
            Style::default(),
            false,
            false,
        )
        .is_empty());
}

#[test]
fn the_edge_facing_the_tab_reserves_a_boundary_cell() {
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    let mut manager = GlobalPaneManager::new(
        crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        Rc::new(RefCell::new(None)),
        zellij_utils::input::options::PaneFrameStyle::Titles,
    );
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
        .unwrap();
    manager
        .insert(
            mock_pane(PaneId::Plugin(2)),
            placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
            false,
        )
        .unwrap();
    manager.resize(area).unwrap();

    // a Left pane reserves its own last column for the line the tab's edge pane draws there
    assert_eq!(
        manager
            .pane(PaneId::Plugin(1))
            .unwrap()
            .get_content_offset(),
        Offset {
            top: 1,
            right: 1,
            ..Default::default()
        }
    );
    // a Right pane reserves nothing: there the line falls on the tab pane's own last column
    assert_eq!(
        manager
            .pane(PaneId::Plugin(2))
            .unwrap()
            .get_content_offset(),
        Offset {
            top: 1,
            ..Default::default()
        }
    );
}

#[test]
fn borderless_panes_still_reserve_the_edge_facing_the_tab() {
    // `borderless` drops the pane's own frame, not the tab/global separator: the tab draws that
    // line one cell inside a Left/Top global pane regardless, so the cell stays reserved.
    let area = Viewport {
        cols: 120,
        rows: 40,
        ..Default::default()
    };
    for pane_frame_style in [
        zellij_utils::input::options::PaneFrameStyle::None,
        zellij_utils::input::options::PaneFrameStyle::Titles,
        zellij_utils::input::options::PaneFrameStyle::Full,
    ] {
        let mut manager = GlobalPaneManager::new(
            crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
            Rc::new(RefCell::new(None)),
            pane_frame_style,
        );
        let mut left = mock_pane(PaneId::Plugin(1));
        left.set_borderless(true);
        let mut top = mock_pane(PaneId::Plugin(2));
        top.set_borderless(true);
        let mut right = mock_pane(PaneId::Plugin(3));
        right.set_borderless(true);
        manager.insert(left, left_percent(25), false).unwrap();
        manager
            .insert(
                top,
                placement(GlobalPaneEdge::Top, PercentOrFixed::Percent(25)),
                false,
            )
            .unwrap();
        manager
            .insert(
                right,
                placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
                false,
            )
            .unwrap();
        manager.resize(area).unwrap();

        // no title row is reserved for a borderless pane, in any frame style
        assert_eq!(
            manager
                .pane(PaneId::Plugin(1))
                .unwrap()
                .get_content_offset(),
            Offset {
                right: 1,
                ..Default::default()
            },
            "borderless Left pane in {pane_frame_style:?}"
        );
        assert_eq!(
            manager
                .pane(PaneId::Plugin(2))
                .unwrap()
                .get_content_offset(),
            Offset {
                bottom: 1,
                ..Default::default()
            },
            "borderless Top pane in {pane_frame_style:?}"
        );
        assert_eq!(
            manager
                .pane(PaneId::Plugin(3))
                .unwrap()
                .get_content_offset(),
            Offset::default(),
            "borderless Right pane in {pane_frame_style:?}"
        );
    }
}

#[test]
fn the_boundary_viewport_grows_only_over_adjacent_strips() {
    use super::{expand_viewport_over_global_panes, GlobalPaneStrip};

    let tab_viewport = Viewport {
        x: 20,
        y: 1,
        cols: 100,
        rows: 38,
    };
    let strip = |edge, x, y, cols, rows| GlobalPaneStrip {
        geom: geom(x, y, cols, rows),
        edge,
    };

    // no strips at all leaves the viewport untouched
    assert_eq!(
        expand_viewport_over_global_panes(tab_viewport, &[]),
        tab_viewport
    );

    // a left strip ending exactly where the tab begins is adjacent
    assert_eq!(
        expand_viewport_over_global_panes(
            tab_viewport,
            &[strip(GlobalPaneEdge::Left, 0, 0, 20, 40)]
        ),
        Viewport {
            x: 0,
            y: 1,
            cols: 120,
            rows: 38
        }
    );

    // a right strip grows the far edge, which is what makes the tab's rightmost pane reserve
    // its own last column for the boundary (see `pane_content_offset`)
    assert_eq!(
        expand_viewport_over_global_panes(
            tab_viewport,
            &[strip(GlobalPaneEdge::Right, 120, 0, 20, 40)]
        ),
        Viewport {
            x: 20,
            y: 1,
            cols: 120,
            rows: 38
        }
    );

    // a top strip with a tab bar between it and the tab's panes is not adjacent, so nothing
    // grows and no boundary is drawn over the tab bar
    assert_eq!(
        expand_viewport_over_global_panes(
            tab_viewport,
            &[strip(GlobalPaneEdge::Top, 0, 0, 120, 5)]
        ),
        tab_viewport
    );
}

#[test]
fn mode_updates_reach_each_global_plugin_once_per_client() {
    let (plugin_sender, plugin_receiver) = channels::unbounded();
    let senders = crate::thread_bus::ThreadSenders {
        to_plugin: Some(SenderWithContext::new(plugin_sender)),
        should_silently_fail: true,
        ..Default::default()
    };
    let mut manager =
        GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
    manager
        .insert(mock_pane(PaneId::Plugin(1)), left_percent(10), true)
        .unwrap();
    manager
        .insert(mock_pane(PaneId::Plugin(2)), left_percent(10), true)
        .unwrap();

    manager
        .update_input_modes(&[(1, ModeInfo::default()), (2, ModeInfo::default())])
        .unwrap();

    let (PluginInstruction::Update(updates), _) = plugin_receiver.recv().unwrap() else {
        panic!("expected plugin mode updates")
    };
    let recipients: Vec<_> = updates
        .iter()
        .map(|(plugin_id, client_id, _event)| (*plugin_id, *client_id))
        .collect();
    assert_eq!(
        recipients,
        vec![
            (Some(1), Some(1)),
            (Some(2), Some(1)),
            (Some(1), Some(2)),
            (Some(2), Some(2)),
        ]
    );
}

#[test]
fn layout_declaration_order_survives_out_of_order_creation() {
    let mut manager = GlobalPaneManager::default();
    manager
        .insert_with_order(
            mock_pane(PaneId::Plugin(2)),
            left_percent(10),
            true,
            Some(1),
        )
        .unwrap();
    manager
        .insert_with_order(
            mock_pane(PaneId::Plugin(1)),
            left_percent(10),
            true,
            Some(0),
        )
        .unwrap();

    assert_eq!(manager.plugin_ids(), vec![1, 2]);
}

// ---- property-based tests (hegel) ----

use hegel::generators;

/// A `(PaneId, GlobalPanePlacement)` over every edge and both size flavours, unbounded.
#[hegel::composite]
fn any_placement(
    tc: &hegel::TestCase,
    pane_index: u32,
) -> (ZellijUtilsPaneId, GlobalPanePlacement) {
    let edge = tc.draw(generators::sampled_from(vec![
        GlobalPaneEdge::Left,
        GlobalPaneEdge::Right,
        GlobalPaneEdge::Top,
        GlobalPaneEdge::Bottom,
    ]));
    let size = if tc.draw(generators::booleans()) {
        PercentOrFixed::Percent(tc.draw(generators::integers::<usize>()))
    } else {
        PercentOrFixed::Fixed(tc.draw(generators::integers::<usize>()))
    };
    let pane_id = if tc.draw(generators::booleans()) {
        ZellijUtilsPaneId::Terminal(pane_index)
    } else {
        ZellijUtilsPaneId::Plugin(pane_index)
    };
    (
        pane_id,
        GlobalPanePlacement {
            edge,
            size,
            length: None,
        },
    )
}

/// Distinct pane ids - `allocate_global_panes` collects into a `BTreeMap`, so duplicate ids
/// would silently collapse and make "one rect per input" meaningless.
#[hegel::composite]
fn any_placements(tc: &hegel::TestCase) -> Vec<(ZellijUtilsPaneId, GlobalPanePlacement)> {
    let count = tc.draw(generators::integers::<usize>().max_value(6));
    (0..count as u32)
        .map(|i| tc.draw(any_placement(i)))
        .collect()
}

/// A viewport is derived from the terminal size the OS reports, not from config, so it is
/// bounded in practice. `allocate_global_panes` does plain `center.y + center.rows`
/// arithmetic on it; a `usize::MAX` viewport is not an input it is required to survive.
#[hegel::composite]
fn any_viewport(tc: &hegel::TestCase) -> Viewport {
    let coordinate = || generators::integers::<usize>().max_value(10_000);
    Viewport {
        x: tc.draw(coordinate()),
        y: tc.draw(coordinate()),
        cols: tc.draw(coordinate()),
        rows: tc.draw(coordinate()),
    }
}

fn rect_of(geom: &PaneGeom) -> (usize, usize, usize, usize) {
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

fn rects_overlap(a: (usize, usize, usize, usize), b: (usize, usize, usize, usize)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax < bx.saturating_add(bw)
        && bx < ax.saturating_add(aw)
        && ay < by.saturating_add(bh)
        && by < ay.saturating_add(ah)
}

fn contains(area: &Viewport, r: (usize, usize, usize, usize)) -> bool {
    let (x, y, w, h) = r;
    x >= area.x
        && y >= area.y
        && x.saturating_add(w) <= area.x.saturating_add(area.cols)
        && y.saturating_add(h) <= area.y.saturating_add(area.rows)
}

#[hegel::test]
fn allocated_global_panes_tile_the_area_without_overlapping(tc: hegel::TestCase) {
    let area = tc.draw(any_viewport());
    let placements = tc.draw(any_placements());
    let clamp_to_screen = tc.draw(generators::booleans());
    let chrome_rows = (
        tc.draw(generators::integers::<usize>()),
        tc.draw(generators::integers::<usize>()),
    );

    let Ok(allocated) = allocate_global_panes(area, &placements, clamp_to_screen, chrome_rows)
    else {
        return; // an `Err` is always an acceptable answer
    };

    let rects: Vec<_> = allocated
        .panes
        .iter()
        .map(|(pane_id, geom)| (*pane_id, rect_of(geom)))
        .collect();

    for (pane_id, rect) in &rects {
        assert!(
            contains(&area, *rect),
            "{:?} at {:?} escapes area {:?} (placements {:?}, clamp {}, chrome {:?})",
            pane_id,
            rect,
            area,
            placements,
            clamp_to_screen,
            chrome_rows
        );
    }
    for (i, (id_a, a)) in rects.iter().enumerate() {
        for (id_b, b) in rects.iter().skip(i + 1) {
            assert!(
                !rects_overlap(*a, *b),
                "{:?} at {:?} overlaps {:?} at {:?} (area {:?}, placements {:?}, clamp {}, chrome {:?})",
                id_a,
                a,
                id_b,
                b,
                area,
                placements,
                clamp_to_screen,
                chrome_rows
            );
        }
    }
    let center = allocated.center;
    let center_rect = (center.x, center.y, center.cols, center.rows);
    for (pane_id, rect) in &rects {
        assert!(
            !rects_overlap(center_rect, *rect),
            "center {:?} overlaps {:?} at {:?} (area {:?}, placements {:?}, clamp {}, chrome {:?})",
            center_rect,
            pane_id,
            rect,
            area,
            placements,
            clamp_to_screen,
            chrome_rows
        );
    }
    assert!(
        contains(&area, center_rect),
        "center {:?} escapes area {:?} (placements {:?}, clamp {}, chrome {:?})",
        center_rect,
        area,
        placements,
        clamp_to_screen,
        chrome_rows
    );
}
