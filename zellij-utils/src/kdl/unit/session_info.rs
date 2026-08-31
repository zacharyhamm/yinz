use super::*;

#[test]
fn serialize_and_deserialize_session_info() {
    let session_info = SessionInfo::default();
    let serialized = session_info.to_string();
    let deserealized = SessionInfo::from_string(&serialized, "not this session").unwrap();
    assert_eq!(session_info, deserealized);
    insta::assert_snapshot!(serialized);
}

#[test]
fn serialize_and_deserialize_session_info_with_data() {
    let panes_list = vec![
        PaneInfo {
            host: None,
            firecracker_image: None,
            firecracker_worktree: None,
            agent: None,
            id: 1,
            is_plugin: false,
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            title: "pane 1".to_owned(),
            exited: false,
            exit_status: None,
            is_held: false,
            pane_x: 0,
            pane_content_x: 1,
            pane_y: 0,
            pane_content_y: 1,
            pane_rows: 5,
            pane_content_rows: 4,
            pane_columns: 22,
            pane_content_columns: 21,
            cursor_coordinates_in_pane: Some((0, 0)),
            terminal_command: Some("foo".to_owned()),
            plugin_url: None,
            is_selectable: true,
            index_in_pane_group: Default::default(), // we don't serialize this
            default_fg: None,
            default_bg: None,
            cwd: Some("/work/hocus".to_owned()),
        },
        PaneInfo {
            host: None,
            firecracker_image: None,
            firecracker_worktree: None,
            agent: None,
            id: 1,
            is_plugin: true,
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            title: "pane 1".to_owned(),
            exited: false,
            exit_status: None,
            is_held: false,
            pane_x: 0,
            pane_content_x: 1,
            pane_y: 0,
            pane_content_y: 1,
            pane_rows: 5,
            pane_content_rows: 4,
            pane_columns: 22,
            pane_content_columns: 21,
            cursor_coordinates_in_pane: Some((0, 0)),
            terminal_command: None,
            plugin_url: Some("i_am_a_fake_plugin".to_owned()),
            is_selectable: true,
            index_in_pane_group: Default::default(), // we don't serialize this
            default_fg: None,
            default_bg: None,
            cwd: None,
        },
    ];
    let mut panes = HashMap::new();
    panes.insert(0, panes_list);
    let session_info = SessionInfo {
        name: "my session name".to_owned(),
        tabs: vec![
            TabInfo {
                position: 0,
                name: "tab 1".to_owned(),
                active: true,
                panes_to_hide: 1,
                is_fullscreen_active: true,
                is_sync_panes_active: false,
                are_floating_panes_visible: true,
                other_focused_clients: vec![2, 3],
                active_swap_layout_name: Some("BASE".to_owned()),
                is_swap_layout_dirty: true,
                viewport_rows: 10,
                viewport_columns: 10,
                display_area_rows: 10,
                display_area_columns: 10,
                selectable_tiled_panes_count: 10,
                selectable_floating_panes_count: 10,
                tab_id: 0,
                is_flashing_bell: false,
                has_bell_notification: false,
            },
            TabInfo {
                position: 1,
                name: "tab 2".to_owned(),
                active: true,
                panes_to_hide: 0,
                is_fullscreen_active: false,
                is_sync_panes_active: true,
                are_floating_panes_visible: true,
                other_focused_clients: vec![2, 3],
                active_swap_layout_name: None,
                is_swap_layout_dirty: false,
                viewport_rows: 10,
                viewport_columns: 10,
                display_area_rows: 10,
                display_area_columns: 10,
                selectable_tiled_panes_count: 10,
                selectable_floating_panes_count: 10,
                tab_id: 1,
                is_flashing_bell: false,
                has_bell_notification: false,
            },
        ],
        panes: PaneManifest { panes },
        connected_clients: 2,
        is_current_session: false,
        available_layouts: vec![
            LayoutInfo::File("layout1".to_owned(), LayoutMetadata::default()),
            LayoutInfo::BuiltIn("layout2".to_owned()),
            LayoutInfo::File("layout3".to_owned(), LayoutMetadata::default()),
        ],
        plugins: Default::default(),
        tab_history: Default::default(),
        pane_history: Default::default(),
        creation_time: Duration::from_secs(300),
    };
    let serialized = session_info.to_string();
    let deserealized = SessionInfo::from_string(&serialized, "not this session").unwrap();
    assert_eq!(session_info, deserealized);
    insta::assert_snapshot!(serialized);
}
