//! The one slot a pane keeps for "draw my frame differently for a while".

use std::time::{Duration, Instant};

use crate::ClientId;
use yinz_types::animation::{Easing, Tween};
use yinz_types::colors::PaletteColor;

/// How long a bell flash stays at full strength before it starts easing away.
///
/// A flash has to read as a *flash*: without a hold it becomes a slow throb, which is both less
/// noticeable and more distracting. The hold plus the release below still add up to the 400ms
/// (the Doherty threshold) the flash has always lasted, so the background job that tears it down
/// on that schedule stays exactly as it was.
const BELL_FLASH_HOLD: Duration = Duration::from_millis(120);
const BELL_FLASH_RELEASE: Duration = Duration::from_millis(280);

/// A pane frame drawn in a colour other than the one focus would give it, and
/// optionally under a title other than its own.
///
/// A pane has at most one of these at a time — a bell flash, a red exit status
/// and a plugin's highlight all compete for the same slot, last writer wins.
/// That is the long-standing contract; the only thing recorded here beyond the
/// colour and the text is *who* the override is for.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameColorOverride {
    pub color: PaletteColor,
    /// Replaces the pane title while the override is up. `None` leaves the
    /// pane's own title alone.
    pub text: Option<String>,
    /// The one client that sees it, or `None` for every client.
    ///
    /// A bell flash and a failed command are facts about the pane and belong to
    /// everyone. A plugin marking a pane for the person it is talking to is a
    /// fact about that conversation, and putting it on another client's screen
    /// would be telling them about something they did not ask for.
    pub client_id: Option<ClientId>,
    /// How strongly the override applies, over time. `None` for an override that does not fade -
    /// a red exit status and a plugin's highlight both stay until something clears them, and only
    /// a bell flash goes away on its own.
    fade: Option<Tween>,
}

impl FrameColorOverride {
    /// Replace the pane's override only when it changes, returning whether it needs rendering.
    pub fn set_if_changed(slot: &mut Option<Self>, value: Self) -> bool {
        if slot.as_ref() == Some(&value) {
            return false;
        }
        *slot = Some(value);
        true
    }

    pub fn new(color: PaletteColor, text: Option<String>, client_id: Option<ClientId>) -> Self {
        FrameColorOverride {
            color,
            text,
            client_id,
            fade: None,
        }
    }

    /// A bell flash: full strength through the hold, then eased back to nothing.
    ///
    /// Every client sees it, because a bell is a fact about the pane rather than about one
    /// person's conversation with it.
    pub fn flashing(color: PaletteColor, now: Instant) -> Self {
        FrameColorOverride {
            color,
            text: None,
            client_id: None,
            fade: Some(Tween::delayed(
                now,
                BELL_FLASH_HOLD,
                1.0,
                0.0,
                BELL_FLASH_RELEASE,
                // `EaseIn` - the flash should commit to leaving rather than lingering just below
                // visible for the back half of the release
                Easing::EaseIn,
            )),
        }
    }

    /// How strongly this override applies at `now`. Always `1.0` for one that does not fade.
    pub fn strength(&self, now: Instant) -> f32 {
        self.fade.map_or(1.0, |fade| fade.value(now))
    }

    /// Whether this override has finished fading and is contributing nothing.
    pub fn is_faded_out(&self, now: Instant) -> bool {
        self.fade.is_some_and(|fade| fade.is_settled(now))
    }

    /// Whether `client_id` is one of the clients this override is drawn for.
    pub fn applies_to(&self, client_id: ClientId) -> bool {
        self.client_id.is_none_or(|owner| owner == client_id)
    }

    /// Whether a clear aimed at `client_id` should take this override down.
    ///
    /// A clear with no client is unconditional: it is how the bell flash and the
    /// pane's own teardown put the slot back, and neither knows about clients.
    /// A clear *for* a client must not silently drop an override every client is
    /// looking at.
    pub fn is_cleared_by(&self, client_id: Option<ClientId>) -> bool {
        match client_id {
            Some(client_id) => self.client_id == Some(client_id),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
