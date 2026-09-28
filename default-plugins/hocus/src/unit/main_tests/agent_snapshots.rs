use super::*;

/// Snapshots are unsolicited, so ordering is all hocus has: one that is no
/// newer than the last merged arrived late and would resurrect panes the
/// newer one already saw close.
#[test]
fn an_out_of_order_agent_snapshot_is_dropped() {
    let mut state = state_with_panes();

    // merging never repaints hocus itself - the strip lives in the
    // hocus-agents pane now
    assert!(!state.pipe(agent_snapshot(4)));
    assert!(state.panes.is_empty(), "a newer snapshot merges");
    assert_eq!(state.agent_seq, 4);

    state.set_panes(panes_with_agent());
    assert!(!state.pipe(agent_snapshot(3)), "an older one is dropped");
    assert!(!state.panes.is_empty(), "stale snapshot must not merge");
    assert!(
        !state.pipe(agent_snapshot(4)),
        "nor is a repeat of the last"
    );
    assert!(!state.panes.is_empty());
    assert_eq!(state.agent_seq, 4);
}

/// The prune path used to reset the watermark on its way past, which threw
/// away the one defence against reordered delivery at exactly the moment it
/// was needed: a pane closes, its manual kind is pruned, and the older
/// snapshot still in flight — the one that predates the close — is admitted
/// and puts the agent back on screen.
#[test]
fn a_prune_does_not_reset_the_sequence_watermark() {
    let mut state = state_with_panes();
    state.run_local(&agent_call(
        "agent_register",
        r#"{"pane_id":4,"kind":"reviewer"}"#,
    ));
    assert!(state.manual_kinds.contains_key(&4));

    // Pane 4 closed. Merging this prunes its registration.
    state.pipe(agent_snapshot(5));
    assert!(state.manual_kinds.is_empty(), "the prune really happened");
    assert_eq!(state.agent_seq, 5, "the watermark survives it");

    // The snapshot that was already in flight when it closed.
    assert!(!state.pipe(agent_snapshot_with(4, panes_with_agent())));
    assert!(
        state.panes.is_empty(),
        "a stale snapshot must not bring the closed pane back",
    );
}

/// A reloaded daemon restarts its counter, so the watermark has to be
/// dropped whenever we re-subscribe or its snapshots would never be taken
/// again.
#[test]
fn re_subscribing_forgets_the_sequence_watermark() {
    let mut state = state_with_panes();
    state.pipe(agent_snapshot(9));
    assert_eq!(state.agent_seq, 9);

    state.subscribe_to_agentd();
    assert_eq!(state.agent_seq, 0, "a restarted daemon starts at one again");
    state.set_panes(panes_with_agent());
    state.pipe(agent_snapshot(1));
    assert_eq!(
        state.agent_seq, 1,
        "the restarted daemon is heard once more"
    );
    assert!(state.panes.is_empty());
}

/// The same, reached the way a reloaded daemon really reaches it: it
/// announces itself with `AGENT_HELLO`, and the handshake is what has to drop
/// the watermark. Testing only `subscribe_to_agentd` left the wiring between
/// the two free to be deleted, and a hocus that ignores every snapshot for the
/// rest of the session shows a strip frozen at the moment the daemon reloaded.
#[test]
fn a_daemon_that_announces_itself_is_heard_again() {
    let mut state = state_with_panes();
    state.pipe(agent_snapshot(9));
    assert_eq!(state.agent_seq, 9);

    state.pipe(PipeMessage {
        source: PipeSource::Plugin(yinz_tile::prelude::PluginId::new(1)),
        name: wire::AGENT_HELLO.to_owned(),
        payload: None,
        args: BTreeMap::new(),
        is_private: true,
        pane_id: None,
    });

    state.pipe(agent_snapshot_with(1, panes_with_agent()));
    assert_eq!(state.agent_seq, 1, "the restarted daemon is merged again");
    assert!(!state.panes.is_empty());
}
