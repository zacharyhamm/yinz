use super::*;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;

#[test]
fn client_history_preserves_boundaries_and_rejects_narrowing() {
    for value in [0, u16::MAX] {
        let session = SessionInfo {
            tab_history: BTreeMap::from([(
                ClientId::new(value),
                vec![TabId::new(0), TabId::new(1)],
            )]),
            pane_history: BTreeMap::from([(ClientId::new(value), vec![PaneId::default()])]),
            ..Default::default()
        };
        let encoded = crate::metadata::session_info::to_string(&session).unwrap();
        let decoded = crate::metadata::session_info::from_string(
            &encoded,
            &yinz_types::internal_ids::SessionName::new("another session"),
        )
        .unwrap();
        assert_eq!(decoded.tab_history, session.tab_history);
        assert_eq!(decoded.pane_history, session.pane_history);
        for invalid in [-1_i64, 65536, i64::MAX] {
            let invalid = encoded.replace(&format!("id {value}"), &format!("id {invalid}"));
            assert!(crate::metadata::session_info::from_string(
                &invalid,
                &yinz_types::internal_ids::SessionName::new("another session")
            )
            .is_err());
        }
    }
}

#[test]
fn serialize_and_deserialize_session_info() {
    let session_info = SessionInfo::default();
    let serialized = crate::metadata::session_info::to_string(&session_info).unwrap();
    let deserealized = crate::metadata::session_info::from_string(
        &serialized,
        &yinz_types::internal_ids::SessionName::new("not this session"),
    )
    .unwrap();
    assert_eq!(session_info, deserealized);
    insta::assert_snapshot!(serialized);
}

#[test]
fn serialize_and_deserialize_session_info_with_data() {
    let panes_list = vec![
        PaneInfo {
            multipane: None,
            pane_tabs: None,
            host: None,
            target: Default::default(),
            agent: None,
            id: yinz_types::pane_id::PaneId::from_legacy(1, false),
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            is_stack_list_member: true,
            title: "pane 1".to_owned(),
            display_title: None,
            exited: false,
            exit_status: None,
            restart_error: None,
            is_held: false,
            pane_x: yinz_types::position::Column::new(0),
            pane_content_x: yinz_types::position::Column::new(1),
            pane_y: yinz_types::position::Row::new(0),
            pane_content_y: yinz_types::position::Row::new(1),
            pane_rows: yinz_types::position::RowCount::new(5),
            pane_content_rows: yinz_types::position::RowCount::new(4),
            pane_columns: yinz_types::position::ColumnCount::new(22),
            pane_content_columns: yinz_types::position::ColumnCount::new(21),
            cursor_coordinates_in_pane: Some((0, 0).into()),
            terminal_command: Some("foo".to_owned()),
            plugin_url: None,
            is_selectable: true,
            index_in_pane_group: Default::default(), // we don't serialize this
            default_fg: None,
            default_bg: None,
            cwd: Some("/work/agent".to_owned()),
        },
        PaneInfo {
            multipane: None,
            pane_tabs: None,
            host: None,
            target: Default::default(),
            agent: None,
            id: yinz_types::pane_id::PaneId::from_legacy(1, true),
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            is_stack_list_member: false,
            title: "pane 1".to_owned(),
            display_title: None,
            exited: false,
            exit_status: None,
            restart_error: None,
            is_held: false,
            pane_x: yinz_types::position::Column::new(0),
            pane_content_x: yinz_types::position::Column::new(1),
            pane_y: yinz_types::position::Row::new(0),
            pane_content_y: yinz_types::position::Row::new(1),
            pane_rows: yinz_types::position::RowCount::new(5),
            pane_content_rows: yinz_types::position::RowCount::new(4),
            pane_columns: yinz_types::position::ColumnCount::new(22),
            pane_content_columns: yinz_types::position::ColumnCount::new(21),
            cursor_coordinates_in_pane: Some((0, 0).into()),
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
    panes.insert(TabPosition::new(0), panes_list);
    let session_info = SessionInfo {
        name: "my session name".to_owned().into(),
        tabs: vec![
            TabInfo {
                position: TabPosition::new(0),
                name: "tab 1".to_owned(),
                active: true,
                panes_to_hide: 1,
                is_fullscreen_active: true,
                is_sync_panes_active: false,
                are_floating_panes_visible: true,
                other_focused_clients: vec![
                    yinz_types::data::ClientId::new(2),
                    yinz_types::data::ClientId::new(3),
                ],
                active_swap_layout_name: Some("BASE".to_owned()),
                is_swap_layout_dirty: true,
                viewport_rows: yinz_types::position::RowCount::new(10),
                viewport_columns: yinz_types::position::ColumnCount::new(10),
                display_area_rows: yinz_types::position::RowCount::new(10),
                display_area_columns: yinz_types::position::ColumnCount::new(10),
                selectable_tiled_panes_count: 10,
                selectable_floating_panes_count: 10,
                tab_id: TabId::new(0),
                is_flashing_bell: false,
                has_bell_notification: false,
            },
            TabInfo {
                position: TabPosition::new(1),
                name: "tab 2".to_owned(),
                active: true,
                panes_to_hide: 0,
                is_fullscreen_active: false,
                is_sync_panes_active: true,
                are_floating_panes_visible: true,
                other_focused_clients: vec![
                    yinz_types::data::ClientId::new(2),
                    yinz_types::data::ClientId::new(3),
                ],
                active_swap_layout_name: None,
                is_swap_layout_dirty: false,
                viewport_rows: yinz_types::position::RowCount::new(10),
                viewport_columns: yinz_types::position::ColumnCount::new(10),
                display_area_rows: yinz_types::position::RowCount::new(10),
                display_area_columns: yinz_types::position::ColumnCount::new(10),
                selectable_tiled_panes_count: 10,
                selectable_floating_panes_count: 10,
                tab_id: TabId::new(1),
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
    let serialized = crate::metadata::session_info::to_string(&session_info).unwrap();
    let deserealized = crate::metadata::session_info::from_string(
        &serialized,
        &yinz_types::internal_ids::SessionName::new("not this session"),
    )
    .unwrap();
    assert_eq!(session_info, deserealized);
    insta::assert_snapshot!(serialized);
}

#[test]
fn pane_identity_checks_kdl_range() {
    let pane = PaneInfo::default();
    let encoded = crate::metadata::pane_info::encode_to_kdl(&pane)
        .unwrap()
        .to_string();
    for id in [-1_i64, i64::from(u32::MAX) + 1, i64::MAX] {
        let document = encoded
            .replacen("id 0", &format!("id {id}"), 1)
            .parse()
            .unwrap();
        assert!(crate::metadata::pane_info::decode_from_kdl(&document).is_err());
    }
}

#[test]
fn tab_identity_checks_kdl_range() {
    let mut session = SessionInfo {
        tabs: vec![TabInfo::default()],
        tab_history: BTreeMap::from([(ClientId::new(1), vec![TabId::new(0)])]),
        ..Default::default()
    };
    let encoded = crate::metadata::session_info::to_string(&session).unwrap();
    assert!(crate::metadata::session_info::from_string(
        &encoded.replace("tab_id 0", "tab_id -1"),
        &yinz_types::internal_ids::SessionName::new("")
    )
    .is_err());
    assert!(crate::metadata::session_info::from_string(
        &encoded.replace("history 0", "history -1"),
        &yinz_types::internal_ids::SessionName::new("")
    )
    .is_err());
    if usize::BITS > i64::BITS - 1 {
        session.tabs[0].tab_id = TabId::new(usize::MAX);
        assert!(crate::metadata::session_info::to_string(&session).is_err());
        session.tabs.clear();
        session
            .tab_history
            .insert(ClientId::new(1), vec![TabId::new(usize::MAX)]);
        assert!(crate::metadata::session_info::to_string(&session).is_err());
    }
}

#[test]
fn manifest_coordinates_preserve_kdl_and_reject_out_of_range_values() {
    use yinz_types::position::{CellCoordinates, Column, ColumnCount, Row};
    let pane = PaneInfo {
        pane_x: Column::new(7),
        cursor_coordinates_in_pane: Some(CellCoordinates::new(Column::new(5), Row::new(3))),
        ..Default::default()
    };
    let encoded = pane_info::encode_to_kdl(&pane).unwrap().to_string();
    assert!(encoded.contains("pane_x 7"));
    assert!(encoded.contains("cursor_coordinates_in_pane 5 3"));
    let (_, decoded) = pane_info::decode_from_kdl(&encoded.parse().unwrap()).unwrap();
    assert_eq!(decoded.pane_x, pane.pane_x);
    assert_eq!(
        decoded.cursor_coordinates_in_pane,
        pane.cursor_coordinates_in_pane
    );
    for invalid in [
        encoded.replace("pane_x 7", "pane_x -1"),
        encoded.replace(
            "cursor_coordinates_in_pane 5 3",
            "cursor_coordinates_in_pane 5 -1",
        ),
    ] {
        assert!(pane_info::decode_from_kdl(&invalid.parse().unwrap()).is_err());
    }
    if usize::BITS == 64 {
        let pane = PaneInfo {
            pane_columns: ColumnCount::new(usize::MAX),
            ..Default::default()
        };
        assert!(pane_info::encode_to_kdl(&pane).is_err());
    }
}

#[test]
fn pane_display_title_round_trips_and_old_metadata_keeps_raw_title() {
    for display_title in [None, Some("host project:worktree: 界 shell".to_owned())] {
        let pane = PaneInfo {
            title: "shell".into(),
            display_title,
            ..Default::default()
        };
        let encoded = pane_info::encode_to_kdl(&pane).unwrap();
        assert_eq!(pane_info::decode_from_kdl(&encoded).unwrap().1, pane);
        let json = serde_json::to_value(&pane).unwrap();
        assert_eq!(
            serde_json::from_value::<PaneInfo>(json.clone()).unwrap(),
            pane
        );
        if pane.display_title.is_none() {
            assert!(!encoded.to_string().contains("display_title"));
            assert!(json.get("display_title").is_none());
        }
    }
}

#[test]
fn removed_supervisor_fields_do_not_prevent_loading_session_metadata() {
    let session = SessionInfo::new("legacy".into());
    let encoded = session_info::to_string(&session).unwrap();
    let legacy = format!(
        r#"{encoded}
mother 7
mother_supervision "{{\"state\":\"watching\",\"pending\":2,\"blockers\":1}}"
"#
    );
    let decoded = session_info::from_string(&legacy, &"current".into()).unwrap();
    assert_eq!(decoded, session);
    let mut json = serde_json::to_value(&session).unwrap();
    json["mother"] = serde_json::json!(7);
    json["mother_supervision"] =
        serde_json::json!({"state": "watching", "pending": 2, "blockers": 1});
    assert_eq!(
        serde_json::from_value::<SessionInfo>(json).unwrap(),
        session
    );
    let rewritten = session_info::to_string(&decoded).unwrap();
    assert!(!rewritten.contains("mother"));
}
