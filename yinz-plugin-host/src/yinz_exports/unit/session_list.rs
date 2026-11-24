use super::*;
use yinz_types::data::{SessionInfo, SessionListSnapshot};

#[test]
fn pane_tab_cwd_must_be_utf8_in_pane_and_session_replies() {
    use super::super::queries::pane_info_reply;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use yinz_types::data::PaneInfo;
    use yinz_types::pane_tabs::{
        PaneTabDescriptor, PaneTabInfo, PaneTabLoadState, PaneTabsId, PaneTabsInfo,
    };

    let pane_with_cwd = |cwd: &OsStr| PaneInfo {
        pane_tabs: Some(PaneTabsInfo {
            id: PaneTabsId(1),
            pane_id: PaneId::default(),
            overlay_group: true,
            tabs: vec![PaneTabInfo {
                descriptor: PaneTabDescriptor {
                    label: "Files".into(),
                    location: "yinz:strider".into(),
                    configuration: Default::default(),
                    initial_cwd: Some(cwd.into()),
                },
                plugin_id: None,
                terminal_id: None,
                state: PaneTabLoadState::Unloaded,
            }],
            active: 0,
            bar_focused: vec![],
        }),
        ..Default::default()
    };
    let invalid = pane_with_cwd(OsStr::from_bytes(b"/work/\xff"));
    let valid = pane_with_cwd(OsStr::new("/work/λ"));
    assert_eq!(
        pane_info_reply(Some(invalid.clone()))
            .unwrap()
            .expect_json(),
        serde_json::Value::Null
    );
    assert_eq!(
        pane_info_reply(Some(valid.clone())).unwrap().expect_json(),
        serde_json::json!(valid)
    );

    let mut affected = SessionInfo::new("affected".into());
    affected
        .panes
        .panes
        .insert(TabPosition::new(0), vec![invalid]);
    let mut peer = SessionInfo::new("peer".into());
    peer.panes.panes.insert(TabPosition::new(0), vec![valid]);
    let expected = serde_json::json!({"live_sessions": [peer], "resurrectable_sessions": []});
    let reply = session_list_reply(SessionListSnapshot {
        live_sessions: vec![affected, peer],
        ..Default::default()
    })
    .unwrap()
    .expect_json();
    assert_eq!(reply, expected);
}

#[test]
fn session_list_projects_each_age_independently_and_skips_unrepresentable_peers() {
    let mut live = SessionInfo::new("live".into());
    live.creation_time = Duration::new(19, 314);
    let mut live_sessions = vec![live];
    if let Ok(overflow) = usize::try_from(u64::from(u32::MAX) + 1) {
        let mut invalid = SessionInfo::new("out-of-range".into());
        invalid.tabs.push(yinz_types::data::TabInfo {
            tab_id: yinz_types::internal_ids::TabId::new(overflow),
            ..Default::default()
        });
        live_sessions.push(invalid);
    }
    let reply = session_list_reply(SessionListSnapshot {
        live_sessions,
        resurrectable_sessions: vec![("dead".into(), Duration::new(41, 999))],
    })
    .unwrap()
    .expect_json();
    assert_eq!(reply["live_sessions"].as_array().unwrap().len(), 1);
    assert_eq!(reply["live_sessions"][0]["name"], "live");
    assert_eq!(
        reply["live_sessions"][0]["creation_time"],
        serde_json::json!({"secs":19,"nanos":0})
    );
    assert_eq!(
        reply["resurrectable_sessions"],
        serde_json::json!([["dead",{"secs":41,"nanos":0}]])
    );
}

/// A bulk delete may report success only after a complete scan. An unreadable inventory
/// directory is the error rather than an empty inventory; a missing one is empty.
#[cfg(unix)]
#[test]
fn dead_session_inventory_reports_unreadable_directories() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let sock_dir = root.path().join("sock");
    let cache_dir = root.path().join("cache");
    assert_eq!(
        super::dead_sessions(&sock_dir, &cache_dir).unwrap(),
        Vec::<SessionName>::new()
    );
    std::fs::create_dir_all(cache_dir.join("dead")).unwrap();
    std::fs::create_dir_all(&sock_dir).unwrap();
    assert_eq!(
        super::dead_sessions(&sock_dir, &cache_dir).unwrap(),
        vec![SessionName::new("dead")]
    );
    for unreadable in [&cache_dir, &sock_dir] {
        let set_mode = |mode| {
            std::fs::set_permissions(unreadable, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        set_mode(0o000);
        // A privileged user reads the directory regardless; there is nothing to observe.
        let refused = std::fs::read_dir(unreadable).is_err();
        let result = super::dead_sessions(&sock_dir, &cache_dir);
        set_mode(0o755);
        if refused {
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains(&unreadable.display().to_string()), "{error}");
        }
    }
}
