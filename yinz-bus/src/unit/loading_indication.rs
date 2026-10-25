use super::*;

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn the_dot_count_cycles_with_elapsed_time() {
    let indication = LoadingIndication::new("plugin".to_owned());
    let started_at = indication.started_at.expect("new() stamps a start time");
    assert_eq!(indication.animation_offset(started_at), 0);
    assert_eq!(indication.animation_offset(at(started_at, 499)), 0);
    assert_eq!(indication.animation_offset(at(started_at, 500)), 1);
    assert_eq!(indication.animation_offset(at(started_at, 1500)), 3);
    assert_eq!(
        indication.animation_offset(at(started_at, 2000)),
        0,
        "and back round"
    );
}

#[test]
fn a_dropped_wakeup_changes_nothing_about_the_dot_count() {
    // the whole reason the offset is derived rather than counted: sampling at every 16ms tick
    // and sampling at only some of them must give the same answer at the same instant
    let indication = LoadingIndication::new("plugin".to_owned());
    let started_at = indication.started_at.expect("new() stamps a start time");
    let every_tick: Vec<usize> = (0..120)
        .map(|tick| indication.animation_offset(at(started_at, tick * 16)))
        .collect();
    for (tick, offset) in every_tick.iter().enumerate() {
        assert_eq!(
            indication.animation_offset(at(started_at, tick as u64 * 16)),
            *offset
        );
    }
    assert!(
        every_tick.contains(&0) && every_tick.contains(&3),
        "it did cycle"
    );
}

#[test]
fn merging_keeps_the_earlier_start_so_the_dots_do_not_restart() {
    let mut indication = LoadingIndication::new("plugin".to_owned());
    let original = indication.started_at.expect("a start time");

    // a later stage arriving must not reset the clock, or the dots restart and - worse - the
    // 400ms grace restarts with them and the indicator blinks back off
    let mut later = LoadingIndication::new("plugin".to_owned());
    later.started_at = Some(at(original, 700));
    indication.merge(later);
    assert_eq!(indication.started_at, Some(original));

    // ...and an *earlier* one wins, because that is when the load really began: an indication
    // raised before the pane existed is stamped before the pane's own
    let mut earlier = LoadingIndication::new("plugin".to_owned());
    earlier.started_at = Some(original - Duration::from_millis(300));
    indication.merge(earlier);
    assert_eq!(
        indication.started_at,
        Some(original - Duration::from_millis(300))
    );
}

#[test]
fn merging_onto_an_indication_with_no_start_takes_the_incoming_one() {
    let mut indication = LoadingIndication::default();
    assert_eq!(indication.started_at, None);
    let incoming = LoadingIndication::new("plugin".to_owned());
    let incoming_start = incoming.started_at;
    indication.merge(incoming);
    assert_eq!(indication.started_at, incoming_start);
}
