//! The replay buffer, which is what makes a reconnect a continuation of the
//! stream rather than a guess at one.

use super::*;
use yinz_types::internal_ids::ProcessId;

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

/// A session with a socketpair standing in for the supervisor and another for
/// the pty. The peer ends come back so the caller can keep them alive.
#[cfg(test)]
fn test_session() -> (
    PaneSession,
    std::os::unix::net::UnixStream,
    std::os::unix::net::UnixStream,
) {
    use crate::pty_host::client::PtyHostHandle;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let (control, control_peer) = UnixStream::pair().expect("socketpair failed");
    let (master, master_peer) = UnixStream::pair().expect("socketpair failed");
    let session = PaneSession::new(
        "token".to_owned(),
        "/bin/sh".to_owned(),
        PtyHostHandle::for_test(control),
        OwnedFd::from(master),
        ProcessId::new(std::process::id()),
    );
    (session, control_peer, master_peer)
}

mod exit_reporting {
    use super::*;

    /// The server stops reading at `ChildExited`, so saying it early does not
    /// merely reorder the output — it loses it. A pane that prints and exits in
    /// the same breath is the ordinary case, not a corner one.
    #[test]
    fn an_exit_is_not_reportable_until_the_pty_has_been_drained() {
        let (session, _control, _master) = test_session();
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
        let (session, _control, _master) = test_session();
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
        let (session, _control, _master) = test_session();
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
        let (session, _control, _master) = test_session();
        assert!(!session.is_attached(), "a pane is born detached");
        assert!(
            session.orphaned_for().is_none(),
            "the orphan clock must not start until a connection has actually been lost"
        );
    }
}

mod attaching {
    use super::*;
    use crate::pty_host::remote::session::ResumeStep;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    fn wire() -> crate::pty_host::remote::broker::Wire {
        let (a, _b) = UnixStream::pair().expect("socketpair failed");
        // Leaked deliberately: nothing writes to it, and closing the peer would
        // only give these tests an error path they are not about.
        std::mem::forget(_b);
        Arc::new(Mutex::new(a))
    }

    #[test]
    fn a_disconnect_racing_a_new_attachment_cannot_leave_an_orphan_clock() {
        use std::sync::Barrier;
        let (session, _control, _master) = test_session();
        let recipient = wire();
        let phase = Barrier::new(3);
        let old_generation = AtomicU64::new(0);
        let mut orphaned_attachments = 0;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..2000 {
                    phase.wait();
                    session.detach_if(old_generation.load(Ordering::Acquire));
                    phase.wait();
                }
            });
            scope.spawn(|| {
                for _ in 0..2000 {
                    phase.wait();
                    session.attach(recipient.clone());
                    phase.wait();
                }
            });
            for _ in 0..2000 {
                old_generation.store(session.attach(recipient.clone()), Ordering::Release);
                phase.wait();
                phase.wait();
                orphaned_attachments += usize::from(session.orphaned_for().is_some());
            }
        });
        assert_eq!(
            orphaned_attachments, 0,
            "a live connection inherited the old disconnect's timer"
        );
    }

    /// A connection that has already been replaced must not clear its
    /// successor's wire. Notice of a disconnect can arrive long after a
    /// reconnect has taken over — the control pump only learns of it when its
    /// blocking read finally returns — and acting on it then would silence a
    /// healthy pane and start an orphan clock that reaps it out from under the
    /// server watching it.
    #[test]
    fn a_stale_connection_cannot_detach_a_live_one() {
        let (session, _control, _master) = test_session();
        let first = session.attach(wire());
        let second = session.attach(wire());

        session.detach_if(first);

        assert!(
            session.is_attached(),
            "a superseded connection detached the one that replaced it"
        );
        assert!(
            session.orphaned_for().is_none(),
            "a superseded connection started an orphan clock on a pane that has a server on it"
        );

        session.detach_if(second);
        assert!(!session.is_attached(), "the current connection may detach");
    }

    /// A pane being caught up is spoken for — a second resume has to be refused
    /// — but must not be written to yet, or new output would overtake the
    /// replay and the server would see an `Output` frame where it demanded a
    /// `Resumed`.
    #[test]
    fn a_pane_being_caught_up_is_claimed_but_not_yet_writable() {
        let (session, _control, _master) = test_session();
        let generation = session.try_begin_attach().expect("claim failed");

        assert!(
            session.is_attached(),
            "a pane being caught up must refuse a second resume"
        );
        assert!(
            session.wire().is_none(),
            "the pumps must stay quiet until the replay has landed"
        );

        assert!(matches!(
            session.resume_step(0, &wire(), generation),
            ResumeStep::Attached
        ));
        assert!(
            session.wire().is_some(),
            "a caught-up connection must be writable"
        );
    }

    /// The bug this shape exists to prevent: output produced between the replay
    /// snapshot and the attach belongs to neither, and is silently lost.
    #[test]
    fn output_produced_while_catching_up_is_replayed_not_dropped() {
        let (session, _control, _master) = test_session();
        let generation = session.try_begin_attach().expect("claim failed");

        // What a resume would send first.
        let (replay, _) = session.replay_from(0);
        assert!(replay.is_empty());

        // The pane says something while that is in flight. The pumps cannot send
        // it — there is no wire yet — so the catch-up has to.
        assert!(session.append_output(b"while you were out").is_none());

        match session.resume_step(0, &wire(), generation) {
            ResumeStep::Replay { bytes, .. } => assert_eq!(
                bytes, b"while you were out",
                "output produced during the catch-up was not replayed"
            ),
            _ => panic!("a pane with unsent output must not attach yet"),
        }
    }

    /// The other half: once attached, the pumps own the stream, and the
    /// catch-up must not send the same bytes again.
    #[test]
    fn a_caught_up_connection_attaches_rather_than_replaying_forever() {
        let (session, _control, _master) = test_session();
        let generation = session.try_begin_attach().expect("claim failed");
        assert!(session.append_output(b"first").is_none());

        let sent = match session.resume_step(0, &wire(), generation) {
            ResumeStep::Replay { bytes, .. } => bytes.len() as u64,
            _ => panic!("expected a replay"),
        };
        assert!(matches!(
            session.resume_step(sent, &wire(), generation),
            ResumeStep::Attached
        ));
    }

    #[test]
    fn output_chooses_replay_or_live_delivery_before_a_reconnect_can_attach() {
        let (session, _control, _master) = test_session();
        let generation = session.try_begin_attach().unwrap();
        let recipient = wire();
        let replayed_recipient = session.append_output(b"replayed");
        assert!(matches!(
            session.resume_step(0, &recipient, generation),
            ResumeStep::Replay { bytes, dropped: 0 } if bytes == b"replayed"
        ));
        assert!(matches!(
            session.resume_step(8, &recipient, generation),
            ResumeStep::Attached
        ));
        assert!(
            replayed_recipient.is_none(),
            "a delayed pump must not send the replayed chunk to the newly attached wire"
        );
        let (live, live_generation) = session.append_output(b"live").unwrap();
        assert!(Arc::ptr_eq(&live, &recipient));
        assert_eq!(live_generation, generation);
        assert_eq!(session.replay_from(8), (b"live".to_vec(), 0));
    }

    /// A resume that lost the pane while catching up must be told, not left to
    /// write into a stream another connection now owns.
    #[test]
    fn a_superseded_catch_up_is_told_so() {
        let (session, _control, _master) = test_session();
        let generation = session.try_begin_attach().expect("claim failed");
        let _newer = session.attach(wire());

        assert!(matches!(
            session.resume_step(0, &wire(), generation),
            ResumeStep::Superseded
        ));
    }

    #[test]
    fn only_one_resume_can_claim_a_detached_pane() {
        let (session, _control, _master) = test_session();
        assert!(session.try_begin_attach().is_ok());
        assert_eq!(
            session.try_begin_attach(),
            Err(ResumeRefusal::AlreadyAttached),
            "a second resume claimed a pane that was already being caught up"
        );
    }

    #[test]
    fn resume_and_reaping_are_mutually_exclusive() {
        let (resumed, _control, _master) = test_session();
        let generation = resumed.attach(wire());
        resumed.detach_if(generation);
        assert!(resumed.try_begin_attach().is_ok());
        assert!(!resumed.try_claim_expired_orphan(Duration::ZERO));

        let (reaped, _control, _master) = test_session();
        let generation = reaped.attach(wire());
        reaped.detach_if(generation);
        assert!(reaped.try_claim_expired_orphan(Duration::ZERO));
        assert_eq!(
            reaped.try_begin_attach(),
            Err(ResumeRefusal::Unknown),
            "a reaper-claimed pane must not look transiently attached"
        );
    }
}
