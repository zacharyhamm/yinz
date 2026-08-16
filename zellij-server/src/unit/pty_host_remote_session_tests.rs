//! The replay buffer, which is what makes a reconnect a continuation of the
//! stream rather than a guess at one.

use super::*;

use hegel::generators;

fn ring(capacity: usize) -> OutputRing {
    OutputRing::new(capacity)
}

#[test]
fn a_caught_up_peer_is_given_nothing() {
    let mut r = ring(1024);
    r.append(b"hello world");
    let (replay, dropped) = r.replay_from(r.produced());
    assert!(replay.is_empty());
    assert_eq!(dropped, 0);
}

#[test]
fn a_peer_that_missed_the_end_is_given_exactly_the_tail() {
    let mut r = ring(1024);
    r.append(b"abcdefghij");
    // Took the first four bytes, then the link died.
    let (replay, dropped) = r.replay_from(4);
    assert_eq!(replay, b"efghij".to_vec());
    assert_eq!(dropped, 0, "nothing was lost, the buffer still held it all");
}

/// The case the whole design turns on. Once the gap is longer than the buffer,
/// the replay has a hole, and saying so is what lets the reader reset the grid
/// instead of splicing a torn middle into it.
#[test]
fn a_gap_longer_than_the_buffer_is_reported_as_dropped() {
    let mut r = ring(8);
    r.append(b"0123456789abcdef"); // 16 bytes into an 8-byte ring
    let (replay, dropped) = r.replay_from(0);
    assert_eq!(replay, b"89abcdef".to_vec(), "only the tail survives");
    assert_eq!(
        dropped, 8,
        "the first eight bytes are gone and it must say so"
    );
}

#[test]
fn the_buffer_never_grows_past_its_capacity() {
    let mut r = ring(16);
    for _ in 0..100 {
        r.append(b"0123456789");
    }
    let (replay, _) = r.replay_from(0);
    assert_eq!(replay.len(), 16);
    assert_eq!(r.produced(), 1000);
}

/// A peer claiming more than was ever produced is confused rather than
/// malicious, but the subtraction must not panic either way.
#[test]
fn a_peer_claiming_impossible_progress_is_treated_as_caught_up() {
    let mut r = ring(64);
    r.append(b"short");
    let (replay, dropped) = r.replay_from(9_999);
    assert!(replay.is_empty());
    assert_eq!(dropped, 0);
}

#[test]
fn an_empty_buffer_replays_nothing() {
    let r = ring(64);
    let (replay, dropped) = r.replay_from(0);
    assert!(replay.is_empty());
    assert_eq!(dropped, 0);
}

/// The invariant that matters across any history: what a peer is given, plus
/// what it is told was lost, exactly accounts for everything it had not yet
/// seen. If this drifts, a reconnect silently skips or duplicates output.
#[hegel::test]
fn replay_plus_dropped_accounts_for_everything_the_peer_missed(tc: hegel::TestCase) {
    let capacity = tc.draw(generators::integers::<usize>().min_value(1).max_value(64));
    let mut r = ring(capacity);

    let writes = tc.draw(generators::integers::<usize>().max_value(12));
    let mut produced = 0u64;
    for _ in 0..writes {
        let chunk = tc.draw(generators::binary().max_size(32));
        r.append(&chunk);
        produced += chunk.len() as u64;
    }
    assert_eq!(r.produced(), produced);

    let received = tc.draw(generators::integers::<u64>().max_value(produced.max(1)));
    let received = received.min(produced);
    let (replay, dropped) = r.replay_from(received);

    assert_eq!(
        replay.len() as u64 + dropped,
        produced - received,
        "a peer at {received} of {produced} was given {} bytes and told {dropped} were lost",
        replay.len()
    );
    assert!(
        replay.len() <= capacity,
        "replayed more than the buffer can hold"
    );
}

/// Bytes are not merely accounted for, they are the right ones and in order.
#[hegel::test]
fn the_replayed_bytes_are_the_ones_that_actually_followed(tc: hegel::TestCase) {
    let capacity = tc.draw(generators::integers::<usize>().min_value(4).max_value(64));
    let mut r = ring(capacity);

    let mut all: Vec<u8> = Vec::new();
    let writes = tc.draw(generators::integers::<usize>().max_value(8));
    for _ in 0..writes {
        let chunk = tc.draw(generators::binary().max_size(16));
        r.append(&chunk);
        all.extend_from_slice(&chunk);
    }
    if all.is_empty() {
        return;
    }
    let received = tc.draw(generators::integers::<u64>().max_value(all.len() as u64));
    let (replay, dropped) = r.replay_from(received);

    let expected_start = (received + dropped) as usize;
    assert_eq!(
        replay,
        all[expected_start..].to_vec(),
        "the replay must be the actual continuation from where the peer got to, \
         plus whatever was admitted as dropped"
    );
}

mod exit_reporting {
    use super::*;
    use crate::pty_host::client::PtyHostHandle;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    /// A session with a socketpair standing in for the supervisor and another
    /// for the pty. The peer ends come back so the caller can keep them alive.
    fn session() -> (PaneSession, UnixStream, UnixStream) {
        let (control, control_peer) = UnixStream::pair().expect("socketpair failed");
        let (master, master_peer) = UnixStream::pair().expect("socketpair failed");
        let session = PaneSession::new(
            "token".to_owned(),
            PtyHostHandle::for_test(control),
            OwnedFd::from(master),
            std::process::id(),
        );
        (session, control_peer, master_peer)
    }

    /// The server stops reading at `ChildExited`, so saying it early does not
    /// merely reorder the output — it loses it. A pane that prints and exits in
    /// the same breath is the ordinary case, not a corner one.
    #[test]
    fn an_exit_is_not_reportable_until_the_pty_has_been_drained() {
        let (session, _control, _master) = session();
        session.set_child_exit(Some(7));
        assert!(
            session.exit_is_reportable().is_none(),
            "the child's exit was reportable while its output could still be in flight"
        );

        session.mark_output_drained();
        assert_eq!(
            session.exit_is_reportable(),
            Some(Some(7)),
            "a drained pty and a reaped child is everything needed to report the exit"
        );
    }

    /// Draining is half of it, and on its own says nothing.
    #[test]
    fn a_drained_pty_alone_reports_nothing() {
        let (session, _control, _master) = session();
        session.mark_output_drained();
        assert!(
            session.exit_is_reportable().is_none(),
            "a pane whose pty closed but whose child has not been reaped has no status to give"
        );
    }

    /// Two things race to report it — the quit callback and the output pump —
    /// and the server must be told once.
    #[test]
    fn only_the_first_claim_may_report_the_exit() {
        let (session, _control, _master) = session();
        session.set_child_exit(Some(0));
        session.mark_output_drained();

        assert!(session.claim_exit_report(), "the first claim must succeed");
        assert!(
            !session.claim_exit_report(),
            "a second claim would send the server a duplicate ChildExited"
        );
    }

    /// A pane is built before the connection that asked for it attaches. If that
    /// gap started the orphan clock, the reaper could take a pane that is being
    /// set up.
    #[test]
    fn a_newly_built_pane_is_detached_but_not_yet_orphaned() {
        let (session, _control, _master) = session();
        assert!(!session.is_attached(), "a pane is born detached");
        assert!(
            session.orphaned_for().is_none(),
            "the orphan clock must not start until a connection has actually been lost"
        );
    }
}
