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

use crate::colors::{eightbit_to_rgb, PaletteColor};

/// The shared animation clock's fast cadence - roughly 60fps, and the frame budget every
/// continuously moving surface is sampled against.
///
/// This only guarantees a *wakeup*. Nothing advances on a tick: the frame samples every live
/// animation at one `Instant` just before it composites, so a tick the screen thread was too busy
/// to service costs a frame and never a position. That is also why this is well inside the render
/// debounce (`REPAINT_DELAY_MS` in `yinz-bus/src/background_jobs.rs`) rather than a multiple
/// of it - the debounce coalescing two ticks into one frame is a dropped frame, not a stretched
/// animation.
///
/// It is the *fast* cadence rather than the only one: a surface whose appearance changes on a
/// coarse schedule - the plugin loading dots - asks the same clock for its own, slower period. See
/// `Screen::animation_clock_period`.
pub const FRAME_PERIOD: Duration = Duration::from_millis(16);

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
#[path = "unit/animation_tests.rs"]
mod tests;
