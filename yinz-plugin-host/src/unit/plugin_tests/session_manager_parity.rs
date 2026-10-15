use super::runtime_parity::{default_reply, Pair, Responder};
use prost::Message;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use yinz_plugin_api::plugin_command::{
    save_session_response, ProtobufCurrentSessionLastSavedTimeResponse,
    ProtobufDeleteAllDeadSessionsResponse, ProtobufDeleteDeadSessionResponse,
    ProtobufGetSessionListResponse, ProtobufKillSessionsResponse,
    ProtobufListNotificationsResponse, ProtobufSaveSessionResponse,
};
use yinz_types::{
    data::{
        Event, GetSessionListResponse, LayoutInfo, ModeInfo, NotificationInfo, PaneInfo,
        PipeMessage, PipeSource, PluginCommand, SessionInfo, SessionListSnapshot, TabInfo,
    },
    internal_ids::TabPosition,
};

fn sessions() -> SessionListSnapshot {
    let live_sessions = ["current", "alpha", "beta", "日本語"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let mut session = SessionInfo::new(name.into());
            session.is_current_session = i == 0;
            session.connected_clients = i;
            session.creation_time = Duration::from_secs((i * 100) as u64);
            session.available_layouts = vec![
                LayoutInfo::BuiltIn("default".into()),
                LayoutInfo::BuiltIn("compact".into()),
            ];
            for tab in 0..2 {
                let position = TabPosition::new(tab);
                session.tabs.push(TabInfo {
                    position,
                    name: format!("tab-{tab}"),
                    ..Default::default()
                });
                session.panes.panes.insert(
                    position,
                    vec![PaneInfo {
                        title: format!("pane-{tab}"),
                        is_selectable: true,
                        ..Default::default()
                    }],
                );
            }
            session
        })
        .collect();
    SessionListSnapshot {
        live_sessions,
        resurrectable_sessions: vec![
            ("saved".into(), Duration::from_secs(120)),
            ("older".into(), Duration::from_secs(3660)),
        ],
    }
}
fn pair(config: BTreeMap<String, String>, snapshot: SessionListSnapshot) -> Pair {
    pair_with_replies(config, snapshot, Arc::new(|_| None))
}
fn pair_with_replies(
    config: BTreeMap<String, String>,
    snapshot: SessionListSnapshot,
    extra: Responder,
) -> Pair {
    let responder: Responder = Arc::new(move |command| {
        extra(command).or_else(|| match command {
            PluginCommand::GetSessionList => Some(
                ProtobufGetSessionListResponse::from(GetSessionListResponse::Ok(snapshot.clone()))
                    .encode_to_vec(),
            ),
            PluginCommand::CurrentSessionLastSavedTime => Some(
                ProtobufCurrentSessionLastSavedTimeResponse {
                    timestamp_millis: Some(120000),
                }
                .encode_to_vec(),
            ),
            PluginCommand::KillSessionsAndReply(_) => {
                Some(ProtobufKillSessionsResponse::default().encode_to_vec())
            },
            PluginCommand::DeleteDeadSessionAndReply(_) => {
                Some(ProtobufDeleteDeadSessionResponse::default().encode_to_vec())
            },
            PluginCommand::DeleteAllDeadSessionsAndReply => {
                Some(ProtobufDeleteAllDeadSessionsResponse::default().encode_to_vec())
            },
            PluginCommand::ListNotifications => {
                Some(ProtobufListNotificationsResponse::default().encode_to_vec())
            },
            PluginCommand::SaveSession => Some(
                ProtobufSaveSessionResponse {
                    result: Some(save_session_response::Result::Success(true)),
                }
                .encode_to_vec(),
            ),
            _ => default_reply(command),
        })
    });
    Pair::with_host("session-manager", config, responder, None)
}

#[test]
fn session_manager_actual_wasm_steel_notifications_detail_refresh_and_failures() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_notifications_detail_refresh_and_failures",
    );
    let notifications = Arc::new(Mutex::new(vec![NotificationInfo {
        id: yinz_types::internal_ids::NotificationId::new(1),
        message: "safe\u{1b}[2J\t日本語\nnext line\nlast".into(),
        received_at_unix_ms: yinz_types::units::UnixMillis::new(u64::MAX / 2),
        outcome: None,
    }]));
    let fail = Arc::new(Mutex::new(false));
    let rows = notifications.clone();
    let failure = fail.clone();
    let replies: Responder = Arc::new(move |command| match command {
        PluginCommand::ListNotifications => Some(
            ProtobufListNotificationsResponse {
                notifications: rows
                    .lock()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                error: failure.lock().unwrap().then(|| "not available".into()),
            }
            .encode_to_vec(),
        ),
        _ => None,
    });
    let mut pair = pair_with_replies(BTreeMap::new(), sessions(), replies);
    for key in ["Ctrl v", "Down", "Up", "Enter", "PageDown", "Up", "PageUp"] {
        pair.key(key);
        for size in [(20, 100), (8, 30), (5, 5), (1, 1)] {
            pair.render(size.0, size.1);
        }
    }
    *fail.lock().unwrap() = true;
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(1.0)));
    pair.render(10, 40);
    *fail.lock().unwrap() = false;
    let mut next = notifications.lock().unwrap()[0].clone();
    next.id = yinz_types::internal_ids::NotificationId::new(2);
    notifications.lock().unwrap().push(next);
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(1.0)));
    for key in ["Esc", "Up", "Enter", "Ctrl c", "Esc"] {
        pair.key(key);
        pair.render(10, 60);
    }
}

#[test]
fn session_manager_actual_wasm_steel_failed_mutations_preserve_selection_and_rows() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_failed_mutations_preserve_selection_and_rows",
    );
    let replies: Responder = Arc::new(|command| match command {
        PluginCommand::KillSessionsAndReply(_) => Some(
            ProtobufKillSessionsResponse {
                error: Some("denied".into()),
            }
            .encode_to_vec(),
        ),
        PluginCommand::DeleteDeadSessionAndReply(_) => Some(
            ProtobufDeleteDeadSessionResponse {
                error: Some("denied".into()),
            }
            .encode_to_vec(),
        ),
        PluginCommand::SaveSession => Some(
            ProtobufSaveSessionResponse {
                result: Some(save_session_response::Result::Error("denied".into())),
            }
            .encode_to_vec(),
        ),
        _ => None,
    });
    for multi in [false, true] {
        let mut pair = pair_with_replies(
            BTreeMap::from([("multi_screen".into(), multi.to_string())]),
            sessions(),
            replies.clone(),
        );
        for key in [
            "Down", "Delete", "Esc", "Ctrl d", "y", "Esc", "Ctrl a", "Esc", "Down", "Down", "Down",
            "Delete",
        ] {
            pair.key(key);
            pair.render(15, 110);
        }
    }
}

#[test]
fn session_manager_actual_wasm_steel_search_tree_small_viewports_and_rgb_styles() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_search_tree_small_viewports_and_rgb_styles",
    );
    let mut mode = ModeInfo::default();
    mode.style.colors.text_unselected.emphasis_0 =
        yinz_types::colors::PaletteColor::Rgb((111, 23, 180));
    mode.style.colors.list_selected.background =
        yinz_types::colors::PaletteColor::Rgb((13, 27, 41));
    for multi in [false, true] {
        let mut pair = pair(
            BTreeMap::from([("multi_screen".into(), multi.to_string())]),
            sessions(),
        );
        pair.event(Event::ModeUpdate(mode.clone()));
        for key in [
            "Down",
            "Down",
            "Right",
            "Right",
            "Down",
            "Left",
            "Up",
            "p",
            "Down",
            "Down",
            "a",
            "Backspace",
            "Ctrl c",
        ] {
            pair.key(key);
            for size in [(20, 120), (10, 75), (7, 35), (5, 20), (2, 3)] {
                pair.render(size.0, size.1);
            }
        }
    }
}

#[test]
fn session_manager_actual_wasm_steel_folder_picker_correlates_results_and_layout_cwd() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_folder_picker_correlates_results_and_layout_cwd",
    );
    use serde_json::{json, Value};
    use yinz_types::keys::KeyWithModifier;
    for multi in [false, true] {
        let mut pair = pair(
            BTreeMap::from([("multi_screen".into(), multi.to_string())]),
            sessions(),
        );
        if multi {
            pair.key("Ctrl w");
        }
        pair.key("n");
        pair.key("Enter");
        let event = Event::Key("Ctrl f".parse::<KeyWithModifier>().unwrap());
        assert_eq!(pair.steel.update(&event), pair.wasm.update(&event));
        let mut expected = pair.wasm.take_commands();
        let mut actual = pair.steel.take_commands();
        let normalize = |commands: &mut Vec<Value>| {
            let message = commands[0].get_mut("MessageToPlugin").unwrap();
            let id = message["message_args"]["request_id"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(uuid::Uuid::parse_str(&id).is_ok());
            assert_eq!(message["plugin_config"]["request_id"], id);
            message["message_args"]["request_id"] = json!("request");
            message["plugin_config"]["request_id"] = json!("request");
            id
        };
        let expected_id = normalize(&mut expected);
        let actual_id = normalize(&mut actual);
        assert_eq!(actual, expected);
        let result = |id| PipeMessage {
            source: PipeSource::Plugin(yinz_types::internal_ids::PluginId::new(9)),
            name: "filepicker_result".into(),
            payload: Some("/very/long/folder/日本語/project".into()),
            args: BTreeMap::from([("request_id".into(), id)]),
            is_private: true,
            pane_id: None,
        };
        assert_eq!(
            pair.steel.pipe(&result(actual_id)),
            pair.wasm.pipe(&result(expected_id))
        );
        pair.commands();
        for size in [(20, 120), (10, 70), (8, 40), (5, 10)] {
            pair.render(size.0, size.1);
        }
        pair.key("Enter");
    }
}

#[test]
fn session_manager_actual_wasm_steel_empty_and_populated_viewports() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_empty_and_populated_viewports",
    );
    for snapshot in [SessionListSnapshot::default(), sessions()] {
        for welcome in [false, true] {
            for multi in [false, true] {
                let mut pair = pair(
                    BTreeMap::from([
                        ("welcome_screen".into(), welcome.to_string()),
                        ("multi_screen".into(), multi.to_string()),
                    ]),
                    snapshot.clone(),
                );
                for (rows, columns) in [
                    (20, 100),
                    (10, 80),
                    (8, 40),
                    (30, 140),
                    (50, 110),
                    (5, 20),
                    (1, 1),
                ] {
                    pair.render(rows, columns);
                }
            }
        }
    }
}

#[test]
fn session_manager_actual_wasm_steel_single_search_selection_and_creation() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_single_search_selection_and_creation",
    );
    let mut pair = pair(BTreeMap::new(), sessions());
    for key in [
        "Down",
        "Down",
        "Up",
        "Esc",
        "a",
        "l",
        "Tab",
        "Enter",
        "Ctrl c",
        "Ctrl c",
        "n",
        "e",
        "w",
        "Enter",
        "Down",
        "c",
        "Backspace",
        "Esc",
        "Enter",
        "Enter",
    ] {
        pair.key(key);
        pair.render(20, 100);
    }
    pair.event(Event::Visible(false));
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(1.0)));
    pair.event(Event::Visible(true));
    pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(1.0)));
    pair.render(20, 100);
}

#[test]
fn session_manager_actual_wasm_steel_multi_tree_and_resurrection() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_multi_tree_and_resurrection",
    );
    let mut pair = pair(
        BTreeMap::from([("multi_screen".into(), "true".into())]),
        sessions(),
    );
    for key in [
        "Down",
        "Right",
        "Down",
        "Right",
        "Up",
        "Enter",
        "Down",
        "Down",
        "Enter",
        "p",
        "a",
        "Down",
        "Up",
        "Enter",
        "Tab",
        "Down",
        "Up",
        "s",
        "Backspace",
        "Enter",
        "Ctrl d",
        "n",
        "Down",
        "Delete",
        "Ctrl d",
        "y",
        "Ctrl w",
        "n",
        "Enter",
        "Down",
        "Enter",
    ] {
        pair.key(key);
        pair.render(20, 100);
    }
}

#[test]
fn session_manager_actual_wasm_steel_rename_and_delete_guards() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_rename_and_delete_guards",
    );
    for multi in [false, true] {
        let mut pair = pair(
            BTreeMap::from([("multi_screen".into(), multi.to_string())]),
            sessions(),
        );
        for key in [
            "Ctrl r", "Enter", "a", "Ctrl r", "/", "Enter", "a", "Ctrl r", "x", "Enter", "Delete",
            "a", "Ctrl d", "n", "Down", "Delete", "Ctrl d", "y", "Ctrl x",
        ] {
            pair.key(key);
            pair.render(20, 100);
        }
    }
}

#[test]
fn session_manager_actual_wasm_steel_name_limits_layout_search_and_modifiers() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_name_limits_layout_search_and_modifiers",
    );
    for multi in [false, true] {
        for welcome in [false, true] {
            let mut pair = pair(
                BTreeMap::from([
                    ("multi_screen".into(), multi.to_string()),
                    ("welcome_screen".into(), welcome.to_string()),
                ]),
                sessions(),
            );
            if multi && !welcome {
                pair.key("Ctrl w");
            }
            for text in ["/", &"日".repeat(27)] {
                for ch in text.chars() {
                    pair.key(&ch.to_string());
                }
                pair.key("Enter");
                pair.render(20, 100);
                pair.key("Esc");
                if multi {
                    pair.key("Esc");
                } else {
                    pair.key("Ctrl c");
                }
            }
            pair.key("n");
            pair.key("Enter");
            for key in [
                "c",
                "Down",
                "Up",
                "Backspace",
                "Down",
                "Up",
                "Shift Enter",
                "Alt Enter",
                "Ctrl Enter",
                "Esc",
                "Enter",
                "Enter",
            ] {
                pair.key(key);
                pair.render(10, 55);
            }
        }
    }
}

#[test]
fn session_manager_actual_wasm_steel_large_tree_paging_and_event_refresh() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_wasm_steel_large_tree_paging_and_event_refresh",
    );
    let mut snapshot = sessions();
    for session in &mut snapshot.live_sessions {
        for panes in session.panes.panes.values_mut() {
            for i in 1..10 {
                panes.push(PaneInfo {
                    id: yinz_types::pane_id::PaneId::from_legacy(i, i % 2 == 0),
                    title: format!("日本語-pane-{i}"),
                    exit_status: Some(if i == 1 { 0 } else { 1 }),
                    is_selectable: true,
                    ..Default::default()
                });
            }
        }
    }
    for multi in [false, true] {
        let mut pair = pair(
            BTreeMap::from([("multi_screen".into(), multi.to_string())]),
            snapshot.clone(),
        );
        for key in [
            "Down", "Right", "Right", "Down", "Down", "Down", "Down", "Down", "Up", "Enter", "p",
            "Down", "Down", "Down", "Ctrl c",
        ] {
            pair.key(key);
            for size in [(20, 110), (12, 50), (8, 28), (3, 4)] {
                pair.render(size.0, size.1);
            }
        }
        let mut updated = snapshot.clone();
        updated.live_sessions.pop();
        updated.resurrectable_sessions.pop();
        pair.event(Event::SessionUpdate(
            updated.live_sessions,
            updated.resurrectable_sessions,
        ));
        pair.render(9, 80);
    }
}

#[test]
fn session_manager_actual_search_stops_before_a_result_that_cannot_fit() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "session_manager_actual_search_stops_before_a_result_that_cannot_fit",
    );
    let mut snapshot = sessions();
    snapshot.live_sessions.truncate(1);
    snapshot.live_sessions[0].name = "x-long-session".into();
    for panes in snapshot.live_sessions[0].panes.panes.values_mut() {
        panes[0].title = "x".into();
    }
    let mut pair = pair(
        BTreeMap::from([("multi_screen".into(), "true".into())]),
        snapshot,
    );
    pair.key("x");
    // The exact pane match takes three hierarchy rows; the later session match takes one.
    for rows in 1..16 {
        pair.render(rows, 100);
    }
}
