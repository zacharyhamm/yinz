use super::*;

const COLOR: PaletteColor = PaletteColor::EightBit(4);

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn setting_an_override_only_reports_actual_changes() {
    let mut slot = None;
    for value in [
        FrameColorOverride::new(COLOR, None, None),
        FrameColorOverride::new(COLOR, Some("error".into()), None),
        FrameColorOverride::new(COLOR, Some("error".into()), Some(ClientId::new(1))),
        FrameColorOverride::new(PaletteColor::EightBit(1), None, None),
        FrameColorOverride::flashing(COLOR, Instant::now()),
    ] {
        assert!(FrameColorOverride::set_if_changed(&mut slot, value.clone()));
        assert_eq!(slot.as_ref(), Some(&value));
        assert!(!FrameColorOverride::set_if_changed(&mut slot, value));
    }
}

#[test]
fn an_override_that_does_not_fade_applies_at_full_strength_forever() {
    let now = Instant::now();
    let override_ = FrameColorOverride::new(COLOR, None, None);
    assert_eq!(override_.strength(now), 1.0);
    assert_eq!(override_.strength(at(now, 10_000)), 1.0);
    assert!(
        !override_.is_faded_out(at(now, 10_000)),
        "a plugin highlight waits to be cleared rather than expiring"
    );
}

#[test]
fn a_bell_flash_holds_at_full_strength_then_eases_away() {
    let now = Instant::now();
    let flash = FrameColorOverride::flashing(COLOR, now);

    assert_eq!(flash.strength(now), 1.0);
    assert_eq!(
        flash.strength(at(now, 119)),
        1.0,
        "still holding - a flash has to read as a flash, not a throb"
    );
    let mid = flash.strength(at(now, 260));
    assert!(mid > 0.0 && mid < 1.0, "easing away by now, got {mid}");
    // 260ms is the midpoint of the 120..400 release: a linear release would read exactly
    // 0.5 here. The release eases in - it holds near full for the front half and commits
    // to leaving in the back half - so it is still well above that
    assert!(
        mid > 0.5,
        "the release eases in, so it is not yet halfway gone at the midpoint, got {mid}"
    );
    assert_eq!(flash.strength(at(now, 400)), 0.0);
}

#[test]
fn a_bell_flash_is_faded_out_on_the_schedule_the_teardown_still_runs_on() {
    // the background job clears the slot at FLASH_DURATION_MS (400ms); the fade must have
    // reached zero by then, or the clear would visibly cut a still-showing flash short
    let now = Instant::now();
    let flash = FrameColorOverride::flashing(COLOR, now);
    assert!(!flash.is_faded_out(at(now, 399)));
    assert!(flash.is_faded_out(at(now, 400)));
}

#[test]
fn a_bell_flash_is_shown_to_every_client() {
    let flash = FrameColorOverride::flashing(COLOR, Instant::now());
    assert!(flash.applies_to(ClientId::new(1)));
    assert!(flash.applies_to(ClientId::new(2)));
    assert!(
        flash.is_cleared_by(None),
        "the background job's unconditional clear takes it down"
    );
    assert!(
        !flash.is_cleared_by(Some(ClientId::new(1))),
        "one client's clear must not take down a bell everybody can see"
    );
}

#[test]
fn a_client_scoped_override_is_seen_and_cleared_by_that_client_only() {
    let override_ = FrameColorOverride::new(COLOR, None, Some(ClientId::new(1)));
    assert!(override_.applies_to(ClientId::new(1)));
    assert!(
        !override_.applies_to(ClientId::new(2)),
        "a mark for the person the plugin is talking to stays off everyone else's screen"
    );
    assert!(override_.is_cleared_by(Some(ClientId::new(1))));
    assert!(
        !override_.is_cleared_by(Some(ClientId::new(2))),
        "another client's clear is not this client's"
    );
    assert!(
        override_.is_cleared_by(None),
        "an unconditional clear takes down every override"
    );
}

#[test]
fn an_override_carries_the_text_that_replaces_the_title() {
    let override_ = FrameColorOverride::new(COLOR, Some("exit 1".to_string()), None);
    assert_eq!(override_.text.as_deref(), Some("exit 1"));
    let flash = FrameColorOverride::flashing(COLOR, Instant::now());
    assert!(flash.text.is_none(), "a bell flash leaves the title alone");
}
