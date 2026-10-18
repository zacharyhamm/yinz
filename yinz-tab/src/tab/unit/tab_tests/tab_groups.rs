use super::*;
use yinz_types::input::layout::{PluginAlias, Run, RunPluginOrAlias};
use yinz_types::internal_ids::PluginId;
use yinz_types::pane_size::Constraint;

fn bar_layout(height: usize, alias: bool) -> (TiledPaneLayout, RunPluginOrAlias) {
    let mut plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    if alias {
        plugin = RunPluginOrAlias::Alias(PluginAlias {
            name: "my-bar".into(),
            run_plugin: plugin.get_run_plugin(),
            ..Default::default()
        });
    }
    (
        TiledPaneLayout {
            children: vec![
                TiledPaneLayout {
                    split_size: Some(SplitSize::Fixed(height)),
                    borderless: Some(true),
                    run: Some(Run::Plugin(plugin.clone())),
                    ..Default::default()
                },
                TiledPaneLayout::default(),
            ],
            ..Default::default()
        },
        plugin,
    )
}

#[test]
fn tab_group_toggle_reclaims_a_row_and_restores_declared_height() {
    for alias in [false, true] {
        for initially_enabled in [false, true] {
            let (sender, _receiver) = unbounded();
            let mut senders = ThreadSenders::default().silently_fail_on_send();
            senders.replace_to_plugin(SenderWithContext::new(sender));
            let mut tab = create_new_tab_with_senders(
                Size {
                    cols: 100,
                    rows: 30,
                },
                false,
                senders,
            );
            let client = ClientId::new(1);
            let terminal = tab.get_active_pane_id(client);
            tab.update_tab_groups(initially_enabled).unwrap();
            let (layout, plugin) = bar_layout(2, alias);
            tab.override_layout(
                layout,
                vec![],
                None,
                None,
                vec![],
                vec![],
                HashMap::from([(plugin, vec![PluginId::new(1)])]),
                true,
                true,
                client,
                None,
            )
            .unwrap();
            let bar = PaneId::Plugin(PluginId::new(1));
            let ids = tab.get_all_pane_ids();
            for enabled in [false, true, false, true] {
                tab.update_tab_groups(enabled).unwrap();
                let geom = tab.get_pane_with_id(bar).unwrap().position_and_size();
                assert_eq!(geom.rows.as_usize(), if enabled { 2 } else { 1 });
                assert_eq!(tab.get_viewport().y, if enabled { 2 } else { 1 });
                assert_eq!(tab.get_viewport().rows, if enabled { 28 } else { 29 });
                assert_eq!(
                    tab.tab_bar_serialization_geom(bar, geom).rows.constraint,
                    Constraint::Fixed(2)
                );
                assert_eq!(tab.get_active_pane_id(client), terminal);
                assert_eq!(tab.get_all_pane_ids(), ids);
            }
            tab.update_tab_groups(false).unwrap();
            let (swap, _) = bar_layout(2, alias);
            tab.swap_layouts.set_swap_tiled_layouts(vec![(
                std::collections::BTreeMap::from([(
                    yinz_types::input::layout::LayoutConstraint::ExactPanes(2),
                    swap,
                )]),
                Some("test".into()),
            )]);
            tab.next_swap_layout().unwrap();
            assert_eq!(tab.swap_layout_info().0.as_deref(), Some("test"));
            assert_eq!(
                tab.get_pane_with_id(bar)
                    .unwrap()
                    .position_and_size()
                    .rows
                    .as_usize(),
                1
            );
            tab.update_tab_groups(true).unwrap();
            tab.toggle_active_pane_no_ui_fullscreen(client);
            tab.update_tab_groups(false).unwrap();
            assert!(tab.is_fullscreen_active());
            tab.toggle_active_pane_no_ui_fullscreen(client);
            assert_eq!(tab.get_viewport().y, 1);
            let (layout, _) = bar_layout(1, alias);
            tab.override_layout(
                layout,
                vec![],
                None,
                None,
                vec![],
                vec![],
                HashMap::new(),
                true,
                true,
                client,
                None,
            )
            .unwrap();
            tab.update_tab_groups(true).unwrap();
            assert_eq!(
                tab.get_pane_with_id(bar)
                    .unwrap()
                    .position_and_size()
                    .rows
                    .as_usize(),
                1
            );
            let (two_rows, _) = bar_layout(2, alias);
            tab.swap_layouts.set_swap_tiled_layouts(vec![(
                std::collections::BTreeMap::from([(
                    yinz_types::input::layout::LayoutConstraint::ExactPanes(2),
                    two_rows,
                )]),
                Some("two rows".into()),
            )]);
            tab.next_swap_layout().unwrap();
            tab.update_tab_groups(false).unwrap();
            let (one_row, _) = bar_layout(1, alias);
            tab.swap_layouts.set_swap_tiled_layouts(vec![(
                std::collections::BTreeMap::from([(
                    yinz_types::input::layout::LayoutConstraint::ExactPanes(2),
                    one_row,
                )]),
                Some("one row".into()),
            )]);
            tab.next_swap_layout().unwrap();
            tab.update_tab_groups(true).unwrap();
            assert_eq!(
                tab.get_pane_with_id(bar)
                    .unwrap()
                    .position_and_size()
                    .rows
                    .as_usize(),
                1
            );
        }
    }
}

#[test]
fn tab_group_toggle_preserves_floating_fullscreen() {
    for covers_ui in [false, true] {
        let (sender, _receiver) = unbounded();
        let mut senders = ThreadSenders::default().silently_fail_on_send();
        senders.replace_to_plugin(SenderWithContext::new(sender));
        let mut tab = create_new_tab_with_senders(
            Size {
                cols: 100,
                rows: 30,
            },
            false,
            senders,
        );
        let client = ClientId::new(1);
        let (layout, plugin) = bar_layout(2, false);
        tab.override_layout(
            layout,
            vec![],
            None,
            None,
            vec![],
            vec![],
            HashMap::from([(plugin, vec![PluginId::new(1)])]),
            true,
            true,
            client,
            None,
        )
        .unwrap();
        let floating = PaneId::Terminal(TerminalId::new(2));
        tab.new_floating_pane(floating, None, None, false, true, None, None)
            .unwrap();
        let original_geom = floating_pane_geom(&tab, floating);
        if covers_ui {
            tab.toggle_active_pane_no_ui_fullscreen(client);
        } else {
            tab.toggle_active_pane_fullscreen(client);
        }

        for enabled in [false, true, false, true] {
            tab.update_tab_groups(enabled).unwrap();
            assert!(tab.is_fullscreen_active());
            assert_eq!(tab.fullscreen_covers_ui(), covers_ui);
            assert_eq!(tab.fullscreen_pane_id(), Some(floating));
            assert_eq!(tab.get_active_pane_id(client), Some(floating));
            let viewport = tab.get_viewport();
            assert_eq!(viewport.y, if enabled { 2 } else { 1 });
            assert_eq!(viewport.rows, if enabled { 28 } else { 29 });
            let geom = floating_pane_geom(&tab, floating);
            assert_eq!(geom.x, 0);
            assert_eq!(geom.cols.as_usize(), 100);
            assert_eq!(geom.y, if covers_ui { 0 } else { viewport.y });
            assert_eq!(
                geom.rows.as_usize(),
                if covers_ui { 30 } else { viewport.rows }
            );
        }

        if covers_ui {
            tab.toggle_active_pane_no_ui_fullscreen(client);
        } else {
            tab.toggle_active_pane_fullscreen(client);
        }
        assert!(!tab.is_fullscreen_active());
        assert_eq!(tab.get_active_pane_id(client), Some(floating));
        assert_eq!(floating_pane_geom(&tab, floating), original_geom);
    }
}
