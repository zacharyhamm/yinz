//! Time-sampled animation primitives.
//!
//! Every animated surface in the server shares these: an [`Easing`] curve, a [`Tween`] that carries
//! one scalar from where it is to where it is going, and [`blend`] for the colour channel.
//!
//! The one rule that makes the rest work: **nothing here reads the clock**. `now` is always a
//! parameter, so a single frame samples every live animation at one instant - and a test drives an
//! animation by handing in fabricated `Instant`s rather than by sleeping. Sampling rather than
//! accumulating a per-tick step is also what makes the result frame-rate independent: a late,
//! early, dropped or doubled frame changes how much motion you *see*, never where the animation is
//! or when it ends.

use std::time::{Duration, Instant};

use crate::data::PaletteColor;
use crate::shared::eightbit_to_rgb;

/// The shared animation clock's fast cadence - roughly 60fps, and the frame budget every
/// continuously moving surface is sampled against.
///
/// This only guarantees a *wakeup*. Nothing advances on a tick: the frame samples every live
/// animation at one `Instant` just before it composites, so a tick the screen thread was too busy
/// to service costs a frame and never a position. That is also why this is well inside the render
/// debounce (`REPAINT_DELAY_MS` in `zellij-server/src/background_jobs.rs`) rather than a multiple
/// of it - the debounce coalescing two ticks into one frame is a dropped frame, not a stretched
/// animation.
///
/// It is the *fast* cadence rather than the only one: a surface whose appearance changes on a
/// coarse schedule - the plugin loading dots - asks the same clock for its own, slower period. See
/// `Screen::animation_clock_period`.
pub const FRAME_PERIOD: Duration = Duration::from_millis(16);

/// How long one phase of a blinking indicator lasts - lit for this, then dark for this, so a
/// blink is a one-second cycle.
///
/// Slow on purpose. A blink is a thing in the corner of the eye asking to be noticed, not a
/// thing being read; at anything much faster it stops reading as a signal and starts reading as
/// a fault.
pub const BLINK_PERIOD: Duration = Duration::from_millis(500);

/// The cadence a blinking indicator asks the shared clock for.
///
/// Half a phase, so every phase is certain of a frame. Ticking at exactly [`BLINK_PERIOD`] would
/// put every wakeup on a transition, where the render debounce and a millisecond of jitter decide
/// which side of the edge the frame lands on - and a blink that drops a phase reads as a stutter
/// rather than as a slower blink.
pub const BLINK_TICK: Duration = Duration::from_millis(250);

/// Whether a blink that started at `since` is lit at `now`.
///
/// Lit first: an indicator that appears dark for half a second before its first flash has spent
/// that half-second saying nothing.
pub fn blink_is_lit(since: Instant, now: Instant) -> bool {
    let phases = now.saturating_duration_since(since).as_millis() / BLINK_PERIOD.as_millis();
    phases.is_multiple_of(2)
}

/// The shortest a re-aimed tween is allowed to take.
///
/// Three frames at the animation clock's 16ms. A correction that finishes inside a frame or two
/// reads as a snap, which is the thing this module exists to avoid - so the speed policy in
/// [`scaled_duration`] is floored here rather than allowed to reach zero.
pub const MIN_TWEEN_DURATION: Duration = Duration::from_millis(48);

/// How a [`Tween`] distributes its motion across its duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    /// Constant rate. Right for something that is genuinely uniform - a spinner, a marquee - and
    /// wrong for anything that starts or stops, which is most things.
    Linear,
    /// `t^3` - accelerates away from rest. For something leaving: it commits to going.
    EaseIn,
    /// `1-(1-t)^3` - fast at the start, settles gently. For something arriving, and for any
    /// *re-aim*: the value is already moving when a redirect happens, and easing back in from a
    /// non-zero velocity reads as a stall.
    EaseOut,
    /// Smoothstep - eases in and out. The default, and right for a thing that moves from one
    /// resting place to another.
    #[default]
    EaseInOut,
}

impl Easing {
    /// `progress` is clamped to `[0.0, 1.0]` first, so a caller that overshoots - the last frame of
    /// an animation commonly does - gets a safe endpoint rather than a curve value outside it.
    ///
    /// Every curve is identity at both endpoints, which is what lets a settled animation be
    /// indistinguishable from an unanimated one.
    pub fn apply(self, progress: f32) -> f32 {
        let t = if progress.is_nan() {
            0.0
        } else {
            progress.clamp(0.0, 1.0)
        };
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t * t,
            Easing::EaseOut => {
                let inverted = 1.0 - t;
                1.0 - inverted * inverted * inverted
            },
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// One scalar moving from `from` to `to` over `duration`, sampled by wall clock.
///
/// Sample it with [`Tween::value`]; it holds at `to` forever afterwards, so a caller that stops
/// asking and comes back later still gets the right answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    easing: Easing,
}

impl Tween {
    pub fn new(from: f32, to: f32, duration: Duration, easing: Easing, now: Instant) -> Self {
        Tween {
            from,
            to,
            start: now,
            duration,
            easing,
        }
    }

    /// `from` until `now + hold`, then `from` to `to` over `duration`.
    ///
    /// A hold is expressed as a start stamp in the future rather than as a second tween, so
    /// sampling stays a single comparison.
    pub fn delayed(
        now: Instant,
        hold: Duration,
        from: f32,
        to: f32,
        duration: Duration,
        easing: Easing,
    ) -> Self {
        Tween::new(from, to, duration, easing, now + hold)
    }

    /// A tween already at `value` with nothing left to animate.
    ///
    /// This is the one constructor that reads the clock, and it is harmless: a zero duration means
    /// the stamp is never compared against anything.
    pub fn settled(value: f32) -> Self {
        Tween {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::ZERO,
            easing: Easing::Linear,
        }
    }

    /// Where the tween is at `now`, eased. Clamped to `[from, to]` at both ends.
    pub fn value(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        // `saturating_duration_since` rather than subtraction: a frame may sample with a `now`
        // captured just before this tween was created, and that should read as "not started yet"
        // rather than panic.
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let t = elapsed / self.duration.as_secs_f32();
        self.from + (self.to - self.from) * self.easing.apply(t)
    }

    pub fn is_settled(&self, now: Instant) -> bool {
        // `saturating_duration_since` is zero for a start in the future, so a held tween
        // correctly reads as unsettled throughout its hold.
        now.saturating_duration_since(self.start) >= self.duration
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// The raw, un-eased position in `[0.0, 1.0]`. Callers that want the parameter rather than
    /// the value - a damage fingerprint, a test - want this.
    pub fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    /// Re-aim mid-flight, from an explicitly given `from`.
    ///
    /// Passing `self.value(now)` makes the value continuous across the call, which is what stops
    /// a surface that changes its mind from jumping. `from` is a parameter rather than read from
    /// `self` because the interesting caller needs to re-aim *and* shift the current value in one
    /// step: a toast stack whose slots moved sets `from = value(now) + rows_the_slot_moved`, so
    /// the painted row stays put while the target underneath it changes.
    ///
    /// `duration` is given, not derived. The speed policy differs per surface - a toast's exit is
    /// proportional to how much of it is showing, a stack glide to how many rows moved - and
    /// burying that inside `Tween` would hide the one decision a reviewer needs to see. See
    /// [`scaled_duration`] for the usual policy.
    pub fn redirect(
        &mut self,
        now: Instant,
        from: f32,
        to: f32,
        duration: Duration,
        easing: Easing,
    ) {
        *self = Tween::new(from, to, duration, easing, now);
    }
}

/// How long a move of `span` should take at the speed that covers `full_span` in `full_duration`,
/// floored at [`MIN_TWEEN_DURATION`] and capped at `full_duration`.
///
/// This is the answer to "what happens to the duration when a tween is re-aimed": **speed is
/// preserved, not duration**. Giving a one-row correction a full 220ms crawls; giving it none
/// snaps. A `full_span` of zero has no speed to preserve and gets `full_duration`.
pub fn scaled_duration(span: f32, full_span: f32, full_duration: Duration) -> Duration {
    if full_span.is_nan() || full_span <= f32::EPSILON || !span.is_finite() {
        return full_duration;
    }
    let fraction = (span.abs() / full_span).clamp(0.0, 1.0);
    full_duration
        .mul_f32(fraction)
        .max(MIN_TWEEN_DURATION)
        .min(full_duration)
}

/// `from` blended `t` of the way towards `to`.
///
/// At `t <= 0.0` and `t >= 1.0` this returns the endpoint *as the original [`PaletteColor`]*,
/// variant included. That is what keeps a settled surface byte-identical to an unanimated one, and
/// keeps an `EightBit` theme - whose indices a terminal is free to remap - exact everywhere except
/// during the handful of frames it is actually moving.
pub fn blend(from: PaletteColor, to: PaletteColor, t: f32) -> PaletteColor {
    // a NaN belongs here rather than falling through into arithmetic that would produce one
    if t.is_nan() || t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let (from_r, from_g, from_b) = as_rgb(from);
    let (to_r, to_g, to_b) = as_rgb(to);
    let channel = |from: u8, to: u8| -> u8 {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * t).round() as u8
    };
    PaletteColor::Rgb((
        channel(from_r, to_r),
        channel(from_g, to_g),
        channel(from_b, to_b),
    ))
}

fn as_rgb(color: PaletteColor) -> (u8, u8, u8) {
    match color {
        PaletteColor::Rgb(rgb) => rgb,
        PaletteColor::EightBit(index) => eightbit_to_rgb(index),
    }
}

#[cfg(test)]
mod tests {
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

    /// A blink is lit first, then dark, in equal halves — and stays on that cadence however
    /// long the wait behind it lasts. A permission prompt can sit unanswered all day.
    #[test]
    fn a_blink_starts_lit_and_keeps_even_halves() {
        let since = Instant::now();
        let at = |ms| since + Duration::from_millis(ms);
        assert!(blink_is_lit(since, since), "lit from the first frame");
        assert!(blink_is_lit(since, at(499)));
        assert!(!blink_is_lit(since, at(500)));
        assert!(!blink_is_lit(since, at(999)));
        assert!(blink_is_lit(since, at(1000)));
        // Ten minutes in, still on the beat rather than drifted or settled.
        assert!(blink_is_lit(since, at(600_000)));
        assert!(!blink_is_lit(since, at(600_500)));
        // A clock that appears to run backwards - `now` before `since` - is the first frame,
        // not a panic.
        assert!(blink_is_lit(at(500), since));
    }
}
