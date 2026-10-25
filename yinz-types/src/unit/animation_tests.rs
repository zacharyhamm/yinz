use super::*;

const DURATION: Duration = Duration::from_millis(200);

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn every_easing_is_identity_at_the_endpoints_and_clamps_outside_them() {
    for easing in [
        Easing::Linear,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
    ] {
        assert_eq!(easing.apply(0.0), 0.0, "{easing:?}");
        assert_eq!(easing.apply(1.0), 1.0, "{easing:?}");
        assert_eq!(easing.apply(-0.5), 0.0, "{easing:?}");
        assert_eq!(easing.apply(1.5), 1.0, "{easing:?}");
        assert_eq!(easing.apply(f32::NAN), 0.0, "{easing:?}");
    }
}

#[test]
fn the_eased_curves_are_slower_than_linear_where_they_should_be() {
    // in eases out of rest, out eases into it, in-out does both
    assert!(Easing::EaseIn.apply(0.1) < 0.1);
    assert!(Easing::EaseOut.apply(0.9) > 0.9);
    assert!(Easing::EaseInOut.apply(0.1) < 0.1);
    assert!(Easing::EaseInOut.apply(0.9) > 0.9);
}

#[test]
fn a_tween_reaches_its_endpoints_and_holds_at_the_target() {
    let start = Instant::now();
    let tween = Tween::new(0.0, 1.0, DURATION, Easing::EaseInOut, start);
    assert_eq!(tween.value(start), 0.0);
    assert_eq!(tween.value(at(start, 200)), 1.0);
    assert_eq!(
        tween.value(at(start, 10_000)),
        1.0,
        "a tween nobody sampled for a while is still at its target"
    );
}

#[test]
fn a_tween_sampled_before_it_started_reads_as_not_started() {
    let start = Instant::now();
    let tween = Tween::new(3.0, 9.0, DURATION, Easing::EaseInOut, at(start, 100));
    assert_eq!(tween.value(start), 3.0);
}

#[test]
fn a_tween_moves_monotonically_between_its_endpoints() {
    let start = Instant::now();
    let tween = Tween::new(20.0, 4.0, DURATION, Easing::EaseInOut, start);
    let mut previous = 20.0;
    for millis in (0..=200).step_by(10) {
        let current = tween.value(at(start, millis));
        assert!(
            current <= previous,
            "should descend towards the target, got {current} after {previous}"
        );
        assert!((4.0..=20.0).contains(&current), "stayed in range");
        previous = current;
    }
    assert_eq!(previous, 4.0);
}

#[test]
fn is_settled_agrees_with_the_sampled_value() {
    let start = Instant::now();
    let tween = Tween::new(0.0, 1.0, DURATION, Easing::EaseOut, start);
    assert!(!tween.is_settled(at(start, 199)));
    assert!(tween.is_settled(at(start, 200)));
    assert!(tween.is_settled(at(start, 201)));
    assert_eq!(tween.value(at(start, 200)), tween.target());
}

#[test]
fn a_settled_tween_is_already_at_its_value() {
    let now = Instant::now();
    let tween = Tween::settled(7.0);
    assert_eq!(tween.value(now), 7.0);
    assert_eq!(tween.target(), 7.0);
    assert!(tween.is_settled(now));
}

#[test]
fn a_delayed_tween_holds_before_it_moves() {
    let start = Instant::now();
    let tween = Tween::delayed(
        start,
        Duration::from_millis(120),
        1.0,
        0.0,
        DURATION,
        Easing::EaseOut,
    );
    assert_eq!(tween.value(start), 1.0);
    assert_eq!(tween.value(at(start, 119)), 1.0);
    assert!(
        !tween.is_settled(at(start, 119)),
        "still holding, not settled"
    );
    assert!(tween.value(at(start, 220)) < 1.0, "moving by now");
    assert_eq!(tween.value(at(start, 320)), 0.0);
    assert!(tween.is_settled(at(start, 320)));
}

#[test]
fn progress_is_the_uneased_parameter() {
    let start = Instant::now();
    let tween = Tween::new(0.0, 1.0, DURATION, Easing::EaseInOut, start);
    assert_eq!(tween.progress(at(start, 100)), 0.5);
    assert_eq!(
        tween.value(at(start, 100)),
        0.5,
        "smoothstep is symmetric at the midpoint"
    );
    assert_eq!(tween.progress(at(start, 50)), 0.25);
    assert_ne!(
        tween.value(at(start, 50)),
        0.25,
        "the eased value is not the raw parameter"
    );
}

#[test]
fn redirecting_from_the_current_value_is_continuous() {
    let start = Instant::now();
    let now = at(start, 80);
    for target in [1.0, 0.0, 5.0] {
        let mut tween = Tween::new(0.0, 1.0, DURATION, Easing::EaseInOut, start);
        let before = tween.value(now);
        tween.redirect(now, before, target, DURATION, Easing::EaseOut);
        assert_eq!(
            tween.value(now),
            before,
            "redirecting to {target} must not jump"
        );
        assert_eq!(tween.target(), target);
        assert_eq!(tween.value(at(now, 200)), target, "and it gets there");
    }
}

#[test]
fn redirecting_can_shift_the_current_value_and_re_aim_in_one_step() {
    // the toast-stack case: the slot underneath a toast moved by two rows, so the painted
    // row must stay put while the target it is heading for changes
    let start = Instant::now();
    let now = at(start, 80);
    let mut offset = Tween::new(3.0, 0.0, DURATION, Easing::EaseOut, start);
    let painted = offset.value(now);
    offset.redirect(now, painted + 2.0, 0.0, DURATION, Easing::EaseOut);
    assert_eq!(offset.value(now), painted + 2.0);
    assert_eq!(offset.value(at(now, 200)), 0.0);
}

#[test]
fn scaled_duration_preserves_speed_rather_than_duration() {
    // a fifth of the span takes about a fifth of the time (f32 arithmetic, so "about")
    let fifth = scaled_duration(1.0, 5.0, Duration::from_millis(500));
    assert!(
        fifth.abs_diff(Duration::from_millis(100)) < Duration::from_millis(1),
        "expected ~100ms, got {fifth:?}"
    );
    // ...capped at the full duration, however far the caller claims to be going
    assert_eq!(
        scaled_duration(50.0, 5.0, Duration::from_millis(500)),
        Duration::from_millis(500)
    );
    // ...and floored, so a sliver still reads as a movement
    assert_eq!(
        scaled_duration(0.01, 5.0, Duration::from_millis(500)),
        MIN_TWEEN_DURATION
    );
}

#[test]
fn scaled_duration_survives_a_degenerate_span() {
    let full = Duration::from_millis(220);
    assert_eq!(
        scaled_duration(3.0, 0.0, full),
        full,
        "no speed to preserve"
    );
    assert_eq!(scaled_duration(f32::NAN, 5.0, full), full);
    assert_eq!(
        scaled_duration(-2.0, 5.0, full),
        scaled_duration(2.0, 5.0, full)
    );
}

#[test]
fn a_full_duration_never_drops_below_the_floor_it_is_capped_by() {
    // a full duration shorter than the floor is honoured rather than stretched: the cap wins
    let brief = Duration::from_millis(20);
    assert_eq!(scaled_duration(1.0, 2.0, brief), brief);
}

#[test]
fn blending_returns_the_original_colour_at_the_endpoints() {
    let from = PaletteColor::EightBit(42);
    let to = PaletteColor::Rgb((10, 20, 30));
    for t in [0.0, -1.0, f32::NAN] {
        assert_eq!(blend(from, to, t), from, "t = {t}");
    }
    for t in [1.0, 2.0] {
        assert_eq!(blend(from, to, t), to, "t = {t}");
    }
}

#[test]
fn blending_between_the_endpoints_interpolates_each_channel() {
    let from = PaletteColor::Rgb((0, 100, 200));
    let to = PaletteColor::Rgb((100, 100, 0));
    assert_eq!(blend(from, to, 0.5), PaletteColor::Rgb((50, 100, 100)));

    let mut previous = (0u8, 100u8, 200u8);
    for step in 1..=10 {
        let PaletteColor::Rgb(current) = blend(from, to, step as f32 / 11.0) else {
            panic!("a mid-blend is always Rgb");
        };
        assert!(current.0 >= previous.0, "red climbs");
        assert_eq!(current.1, 100, "an unchanged channel stays put");
        assert!(current.2 <= previous.2, "blue falls");
        previous = current;
    }
}

#[test]
fn blending_resolves_an_eightbit_endpoint_to_rgb() {
    let from = PaletteColor::EightBit(0);
    let to = PaletteColor::EightBit(15);
    assert!(
        matches!(blend(from, to, 0.5), PaletteColor::Rgb(_)),
        "a palette index cannot express a half-way colour"
    );
}
