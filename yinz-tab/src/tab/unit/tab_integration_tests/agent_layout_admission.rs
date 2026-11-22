//! A managed agent spawn into a tab whose swap layout is active is placed by that layout, and the
//! layout stays active so that closing a pane re-applies it. A dirty layout keeps the user's
//! arrangement: the spawn is declined and the caller splits a region instead.

use super::*;
use crate::tab::swap_layouts::SwapLayouts;
use crate::tab::LayoutAdmission;
use yinz_types::agent::launch::LaunchReason;
use yinz_types::input::layout::{Layout, Run, RunPluginOrAlias};
use yinz_types::internal_ids::PluginId;

const TAB_BAR: PaneId = PaneId::Plugin(PluginId::new(1));
const STATUS_BAR: PaneId = PaneId::Plugin(PluginId::new(2));
const SHELL: PaneId = PaneId::Terminal(TerminalId::new(1));

fn agent(id: u32) -> PaneId {
    PaneId::Terminal(TerminalId::new(id))
}

/// The shipped default layout and swap layouts, with the plugin aliases resolved as the default
/// config resolves them.
fn shipped_default_layout() -> Layout {
    let resolve = |kdl: &[u8]| {
        String::from_utf8(kdl.to_vec())
            .unwrap()
            .replace(r#"location="tab-bar""#, r#"location="yinz:tab-bar""#)
            .replace(r#"location="status-bar""#, r#"location="yinz:status-bar""#)
    };
    let layout = resolve(yinz_config::assets::DEFAULT_LAYOUT);
    let swap = resolve(yinz_config::assets::DEFAULT_SWAP_LAYOUT);
    yinz_config::kdl::layout::from_kdl(
        &layout,
        Some("default.kdl".into()),
        Some(("default.swap.kdl", &swap)),
        None,
    )
    .unwrap()
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

fn agent_run() -> Option<Run> {
    Some(Run::Command(RunCommand {
        command: "claude".into(),
        ..Default::default()
    }))
}

/// A tab with the shipped default layout holding one shell, its UI plugins loaded.
fn default_tab() -> Tab {
    let layout = shipped_default_layout();
    let mut tab = create_new_tab_with_swap_layouts(
        Size {
            cols: 240,
            rows: 50,
        },
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
    let (base_layout, floating_layout) = layout.template.clone().unwrap();
    tab.apply_layout(
        base_layout,
        floating_layout,
        vec![(TerminalId::new(1), None)],
        vec![],
        plugin_ids(),
        ClientId::new(1),
        None,
    )
    .unwrap();
    tab.set_pane_selectable(TAB_BAR, false);
    tab.set_pane_selectable(STATUS_BAR, false);
    assert!(tab.get_pane_with_id(SHELL).is_some());
    tab
}

fn select_swap_layout(tab: &mut Tab, name: &str) {
    for _ in 0..10 {
        if tab.swap_layout_info().0.as_deref() == Some(name) {
            assert!(!tab.swap_layouts.is_tiled_damaged());
            return;
        }
        tab.next_swap_layout().unwrap();
    }
    panic!("swap layout {name} never became current");
}

fn admit(tab: &mut Tab, id: PaneId) -> Result<LayoutAdmission, LaunchReason> {
    tab.admit_agent_pane_into_layout(id, None, agent_run())
}

/// The distinct left edges of the content panes: one per column of a columnar layout.
fn content_columns(tab: &Tab) -> Vec<usize> {
    let mut columns: Vec<usize> = tab
        .get_tiled_panes()
        .filter(|(id, _)| **id != TAB_BAR && **id != STATUS_BAR)
        .map(|(_, pane)| pane.position_and_size().x)
        .collect();
    columns.sort_unstable();
    columns.dedup();
    columns
}

fn assert_active_layout(tab: &Tab, name: &str) {
    assert_eq!(
        tab.swap_layout_info(),
        (Some(name.to_owned()), false),
        "the swap layout stays selected and clean"
    );
}

#[test]
fn agents_follow_the_selected_layout_and_a_close_reapplies_it() {
    let mut tab = default_tab();
    select_swap_layout(&mut tab, "three-columns");
    assert_eq!(content_columns(&tab).len(), 1);

    for (id, columns) in [(2, 2), (3, 3), (4, 3)] {
        assert!(matches!(
            admit(&mut tab, agent(id)),
            Ok(LayoutAdmission::Placed)
        ));
        assert!(tab.tiled_panes.panes_contain(&agent(id)));
        assert_active_layout(&tab, "three-columns");
        assert_eq!(content_columns(&tab).len(), columns, "after agent {id}");
    }

    tab.close_pane(agent(4), false, None);
    assert_active_layout(&tab, "three-columns");
    assert_eq!(content_columns(&tab).len(), 3);
    tab.close_pane(agent(3), false, None);
    assert_active_layout(&tab, "three-columns");
    assert_eq!(content_columns(&tab).len(), 2);
}

#[test]
fn an_agent_does_not_take_focus() {
    let mut tab = default_tab();
    let client_id = ClientId::new(1);
    let focused = tab.get_active_pane_id(client_id);
    assert!(matches!(
        admit(&mut tab, agent(2)),
        Ok(LayoutAdmission::Placed)
    ));
    assert_eq!(tab.get_active_pane_id(client_id), focused);
}

#[test]
fn a_dirty_layout_keeps_the_users_arrangement() {
    let mut tab = default_tab();
    select_swap_layout(&mut tab, "three-columns");
    assert!(matches!(
        admit(&mut tab, agent(2)),
        Ok(LayoutAdmission::Placed)
    ));
    tab.resize(
        ClientId::new(1),
        ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
    )
    .unwrap();
    assert!(tab.swap_layouts.is_tiled_damaged());
    let geoms: Vec<_> = tab
        .get_tiled_panes()
        .map(|(id, pane)| (*id, pane.position_and_size()))
        .collect();

    let declined = admit(&mut tab, agent(3));
    assert!(
        matches!(&declined, Ok(LayoutAdmission::Declined { run, .. }) if *run == agent_run()),
        "{declined:?}"
    );
    assert!(tab.get_pane_with_id(agent(3)).is_none());
    let after: Vec<_> = tab
        .get_tiled_panes()
        .map(|(id, pane)| (*id, pane.position_and_size()))
        .collect();
    assert_eq!(after, geoms);

    // Nor does a close re-apply a dirty layout.
    tab.close_pane(agent(2), false, None);
    assert!(tab.swap_layouts.is_tiled_damaged());
}

#[test]
fn without_auto_layout_an_agent_is_declined() {
    let mut tab = default_tab();
    tab.update_auto_layout(false);
    assert!(matches!(
        admit(&mut tab, agent(2)),
        Ok(LayoutAdmission::Declined { .. })
    ));
    assert!(tab.get_pane_with_id(agent(2)).is_none());
}

#[test]
fn an_agent_no_swap_layout_can_hold_is_declined_without_moving_the_selection() {
    // Only the base layout, which holds exactly the panes it was created with.
    let mut tab = create_new_tab_with_swap_layouts(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
        (vec![], vec![]),
        None,
        true,
        true,
    );
    let before = tab.swap_layout_info();
    assert!(matches!(
        admit(&mut tab, agent(2)),
        Ok(LayoutAdmission::Declined { .. })
    ));
    assert!(tab.get_pane_with_id(agent(2)).is_none());
    assert_eq!(tab.swap_layout_info(), before);
}

#[test]
fn an_agent_past_the_pane_limit_is_rejected() {
    let mut tab = default_tab();
    tab.max_panes = Some(tab.get_tiled_pane_ids().len());
    assert!(matches!(
        admit(&mut tab, agent(2)),
        Err(LaunchReason::PaneLimit)
    ));
    assert!(tab.get_pane_with_id(agent(2)).is_none());
}
