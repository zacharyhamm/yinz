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
