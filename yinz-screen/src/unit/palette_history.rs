use super::*;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

fn live_agent() -> LiveAgent {
    LiveAgent {
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
    }
}

#[test]
fn live_identity_qualifies_host_session_and_provider_session_but_allows_metadata_refresh() {
    let original = live_agent();
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

#[test]
fn retained_prompt_identity_changes_with_target_but_survives_metadata_and_run_discovery() {
    let mut browser = Browser {
        live: Some(live_agent()),
        action: HistoryAction::Run {
            id: "codex:provider-session".into(),
            from: None,
            query: String::new(),
        },
        rows: HashMap::new(),
        generation: 0,
        cached_history: HashMap::new(),
        history_scope: None,
        continuation: None,
        pages_since_tail: 0,
        events: BTreeMap::new(),
        usage_checked: None,
        usage: None,
        prompt: None,
    };
    let session = "one".into();
    let values = vec!["continue".into()];
    let id = |action| {
        let AgentAction::Prompt { submission_id, .. } = action else {
            panic!("expected prompt");
        };
        submission_id
    };
    let original = id(browser.prompt_action(&session, &values).unwrap());
    let live = browser.live.as_mut().unwrap();
    let agent = live.pane.agent.as_mut().unwrap();
    agent.state = AgentState::Busy;
    agent.run_id = Some("recorded-run".into());
    assert_eq!(
        id(browser.prompt_action(&session, &values).unwrap()),
        original
    );
    let mut previous = original;
    for target in ["session", "conversation", "provider"] {
        let live = browser.live.as_mut().unwrap();
        match target {
            "session" => live.session = "peer".into(),
            "conversation" => live.pane.agent.as_mut().unwrap().session_id = Some("other".into()),
            _ => live.pane.agent.as_mut().unwrap().kind = "claude".into(),
        }
        let changed = browser.prompt_action(&session, &values).unwrap();
        assert!(
            matches!(&changed, AgentAction::Prompt { target_session: Some(peer), .. } if peer.as_str() == "peer")
        );
        assert_ne!(id(changed.clone()), previous);
        assert_eq!(
            id(browser.prompt_action(&session, &values).unwrap()),
            id(changed.clone())
        );
        previous = id(changed);
    }
}
