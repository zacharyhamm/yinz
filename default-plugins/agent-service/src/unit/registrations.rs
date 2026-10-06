use super::*;

#[test]
fn registrations_are_pruned_only_after_an_observed_closure() {
    for close_while_suppressed in [false, true] {
        let mut state = State::default();
        state.manual_kinds.insert(7, "codex".into());
        for _ in 0..2 {
            state.rebuild_panes();
            assert_eq!(
                state.manual_kinds.get(&7).map(String::as_str),
                Some("codex")
            );
        }

        for suppressed in [false, true, close_while_suppressed] {
            state.manifest.panes.insert(
                TabPosition::new(0),
                vec![PaneInfo {
                    id: PaneId::from_legacy(7, false),
                    is_suppressed: suppressed,
                    ..Default::default()
                }],
            );
            state.rebuild_panes();
            assert_eq!(
                state.manual_kinds.get(&7).map(String::as_str),
                Some("codex")
            );
        }

        // An unobserved registration must survive the same rebuild that removes 7.
        state.manual_kinds.insert(8, "claude".into());
        state.manifest = PaneManifest::default();
        state.rebuild_panes();
        assert!(!state.manual_kinds.contains_key(&7));
        assert_eq!(
            state.manual_kinds.get(&8).map(String::as_str),
            Some("claude")
        );
    }
}

#[test]
fn watched_metadata_republishes_an_identical_snapshot() {
    let watch_id = DataWatchId::new(7);
    let mut state = State {
        panes: Some(vec![]),
        metadata_watches: BTreeSet::from([watch_id]),
        ..State::default()
    };
    state.publish();
    let payload = state.last_published.clone();
    let seq = state.seq;
    let changed = |watch_id| Event::PluginDataChanged {
        watch_id,
        rows: DataRows::default(),
        truncated: false,
    };

    state.update(changed(DataWatchId::new(8)));
    assert_eq!(state.seq, seq, "unrelated watches are ignored");
    state.update(changed(watch_id));
    assert_eq!(state.last_published, payload);
    assert_eq!(state.seq, seq + 1, "metadata still notifies subscribers");
    state.publish();
    assert_eq!(
        state.seq,
        seq + 1,
        "ordinary publication stays deduplicated"
    );
}
