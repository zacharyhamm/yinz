use super::*;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

#[test]
fn live_identity_qualifies_host_session_and_provider_session_but_allows_metadata_refresh() {
    let original = LiveAgent {
        session: "one".into(),
        host: "host-a".into(),
        metadata: BTreeMap::new(),
        pane: PaneInfo {
            agent: Some(AgentInfo {
                kind: "codex".into(),
                state: AgentState::Idle,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: Some("provider-session".into()),
                run_id: None,
            }),
            ..Default::default()
        },
    };
    assert_eq!(original.run().as_deref(), Some("codex:provider-session"));
    let mut refreshed = original.clone();
    refreshed.pane.agent.as_mut().unwrap().state = AgentState::Busy;
    refreshed.pane.agent.as_mut().unwrap().run_id = Some("recorded-run".into());
    assert!(original.same(&refreshed));
    assert_eq!(refreshed.run().as_deref(), Some("recorded-run"));
    let mut changed = refreshed.clone();
    changed.session = "two".into();
    assert_ne!(changed.id(), original.id());
    assert!(!original.same(&changed));
    changed = refreshed.clone();
    changed.host = "host-b".into();
    assert_ne!(changed.id(), original.id());
    assert!(!original.same(&changed));
    changed = refreshed.clone();
    changed.pane.agent.as_mut().unwrap().session_id = Some("replacement".into());
    assert!(!original.same(&changed));
    changed = refreshed.clone();
    changed.pane.agent.as_mut().unwrap().run_id = Some("replacement-run".into());
    assert!(!refreshed.same(&changed));
    assert!(original.text().contains("directory: Unavailable"));
}
