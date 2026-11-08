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
        history: HistoryCache::default(),
        usage_checked: None,
        usage: None,
        prompt: None,
        search_return: None,
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

#[test]
fn activity_replacements_clear_split_and_empty_intervals_before_merging() {
    use yinz_bus::command_palette::HistoryEvent;
    use yinz_types::agent::history::{Cursor, Range};
    for at in [None, Some(1234)] {
        for worker in [None, Some("worker")] {
            let cursor = |revision| Cursor {
                created_at_ms: at,
                revision,
            };
            let range = |start: Option<u64>, end: Option<u64>| Range {
                start: start.map(cursor),
                end: end.map(cursor),
                worker: worker.map(str::to_owned),
            };
            let group = |revision, calls, range| HistoryEvent {
                revision,
                tool_calls: calls,
                range,
                created_at_ms: at,
                kind: "tools".into(),
                time: String::new(),
                markdown: String::new(),
                truncated: false,
            };
            let mut cache = HistoryCache {
                latest_received_revision: 10,
                ..Default::default()
            };
            let prose = group(10, 0, None);
            let other_worker = group(
                99,
                100,
                Some(Range {
                    worker: Some("other".into()),
                    ..range(None, None)
                }),
            );
            cache.merge(
                &[],
                vec![
                    prose.clone(),
                    other_worker.clone(),
                    group(2, 2, Some(range(None, Some(10)))),
                    group(20, 9, Some(range(Some(10), None))),
                ],
            );
            cache.merge(
                &[
                    // No event accompanies the first interval: it became empty.
                    range(Some(10), Some(30)),
                    range(Some(30), Some(50)),
                    range(Some(50), None),
                ],
                vec![
                    group(40, 2, Some(range(Some(30), Some(50)))),
                    group(60, 1, Some(range(Some(50), None))),
                ],
            );
            assert!(!cache.events.contains_key(&20));
            assert_eq!(
                cache.events.keys().copied().collect::<Vec<_>>(),
                [2, 10, 40, 60, 99]
            );
            assert_eq!(cache.events[&10], prose);
            assert_eq!(cache.events[&99], other_worker);
            assert_eq!(cache.events[&40].tool_calls, 2);
            cache.merge(&[range(Some(30), Some(50))], vec![]);
            assert_eq!(
                cache.events.keys().copied().collect::<Vec<_>>(),
                [2, 10, 60, 99]
            );
            assert_eq!(
                cache.latest_received_revision, 10,
                "activity does not acknowledge prose"
            );
        }
    }
}
