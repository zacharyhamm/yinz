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
