use super::*;
use yinz_types::data::{SessionInfo, SessionListSnapshot};

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
