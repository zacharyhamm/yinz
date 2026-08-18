//! hocus-ask — a one-line popup that hands a question to the running hocus.
//!
//! Asking hocus something used to mean opening the sidebar, typing, and then
//! finding your way back to the pane you were working in. This is the short
//! way round: a keybinding opens a small floating pane, you type a question,
//! and on Enter the popup closes and focus lands back where it came from. The
//! sidebar is deliberately *not* revealed — the answer is waiting there when
//! you next open it.
//!
//! Two things carry the whole design:
//!
//! **Focus restoration is the host's job, not ours.** Closing the last floating
//! pane in a tab makes Zellij re-focus every client's retained tiled pane —
//! `Tab::close_pane` → `hide_floating_panes` → `tiled_panes.focus_all_panes`.
//! So `close_self()` *is* the return-to-my-pane behaviour. Recording the
//! previously focused pane ourselves would not merely be racy but wrong:
//! `AddPlugin` is queued to the screen thread before `load` starts, and
//! `get_focused_pane_info` is a request on that same thread, so it would answer
//! with this plugin's own pane.
//!
//! **The message to hocus has to be a broadcast.** Addressing a pipe by url
//! makes the host inject `caller_cwd` into the target's configuration, and
//! plugin identity is (location, configuration) — so a url would *launch a
//! second hocus* rather than find the running one. A broadcast reaches only
//! already-running plugins, which is exactly what is wanted, at the price of
//! reaching nobody at all when hocus is not up. That silence is what the
//! acknowledgement below exists to make visible.
//!
//! **The popup goes when your attention does.** Selecting another pane, or
//! leaving the tab, dismisses it exactly as Esc would. Without that it merely
//! stops being focused — or, for `ToggleFloatingPanes`, merely stops being
//! drawn — and since `LaunchOrFocusPlugin` matches on (location,
//! configuration), the next keypress would re-focus that abandoned instance
//! with the half-typed question still in it. `focus_verdict` and the
//! `Event::Visible` arm below are the two halves of that, and they are two
//! rather than one because `PaneUpdate` does not reach a plugin outside the
//! client's active tab.
//!
//! Known wart: the popup is one pane shared by per-client plugin instances, so
//! with two clients attached each has their own buffer but either one's Esc —
//! or either one's glance at another pane — closes the pane for both. Two
//! people asking at the same instant is not a case worth engineering for.

use std::collections::BTreeMap;

use hocus_proto::wire::{self, Ask, AskAck, AskOutcome};
use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{viewport, InputAction, TextInput};

/// Shown at the head of the input line. Duplicated by the pane title, which is
/// the point: frames can be turned off, and then this is the only thing saying
/// what the pane is.
const PROMPT: &str = "ask> ";

/// The frame title.
const PANE_TITLE: &str = "ask hocus";

/// Config keys for the popup's geometry, and the defaults if unset. Small: it
/// holds one line, and it is covering work the user wants to get back to.
const DEFAULT_X: &str = "20%";
const DEFAULT_Y: &str = "30%";
const DEFAULT_WIDTH: &str = "60%";
const DEFAULT_HEIGHT: &str = "4";

/// How long to wait for hocus to acknowledge a question before concluding
/// there is no hocus. Generous for two channel hops on a busy session, short
/// enough that the pane does not feel stuck when the answer is "nobody heard".
const ACK_TIMEOUT_SECONDS: f64 = 1.5;

/// How often the countdown above is decremented.
const TICK_SECONDS: f64 = 0.25;

/// What the popup says when nothing answered.
const NO_HOCUS: &str = "hocus is not running — open it first, then ask again";

/// Where the popup is in its one-question life.
#[derive(Debug, PartialEq)]
enum Phase {
    /// Taking keys. `note` is why the last attempt failed, if it did.
    Editing { note: Option<String> },
    /// The question is out; waiting for hocus to say what became of it.
    Sending { remaining: f64 },
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Editing { note: None }
    }
}

/// What a reading of the client's focused pane means for the popup.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// Stay up.
    Wait,
    /// Attention has moved on, and so should the popup.
    Dismiss,
}

#[derive(Default)]
struct State {
    /// The plugin's whole KDL block, kept so `PluginConfigurationChanged` can
    /// re-apply the geometry.
    config: BTreeMap<String, String>,
    /// This plugin's own pane id, needed to resize and retitle it.
    plugin_id: u32,
    input: TextInput,
    phase: Phase,
    /// Whether focus has ever been seen on the popup. See `focus_verdict` for
    /// why nothing may be concluded from a focus reading before it has.
    was_focused: bool,
    /// Latched once a close is on its way to the host, so a second one is not.
    dismissed: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // No `request_permission` here, deliberately, for the same two reasons
        // file-finder gives: a builtin — one loaded from a `zellij:` location —
        // is granted every gated call unconditionally by
        // `check_command_permission`, and the permission dialog is drawn *in
        // the requesting plugin's pane*, which here is a four-row box the user
        // is about to type a question into.
        self.config = configuration;
        self.plugin_id = get_plugin_ids().plugin_id;

        // `PastedText` has to be subscribed for: unsubscribed, the host drops
        // bracketed paste outright rather than degrading it to key events, and
        // pasting an error message into the box would silently do nothing.
        // `PaneUpdate` and `Visible` are between them how the popup notices it
        // has been left behind — see `focus_verdict`.
        subscribe(&[
            EventType::Key,
            EventType::PastedText,
            EventType::Timer,
            EventType::PluginConfigurationChanged,
            EventType::PaneUpdate,
            EventType::Visible,
        ]);
        set_selectable(true);
        rename_plugin_pane(self.plugin_id, PANE_TITLE);
        self.apply_geometry();
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::PastedText(text) => {
                if matches!(self.phase, Phase::Editing { .. }) {
                    self.input.insert_text(&text);
                    true
                } else {
                    false
                }
            },
            Event::Timer(elapsed) => self.on_tick(elapsed),
            // Only ever the trigger to go and ask: the manifest itself cannot
            // answer the question, see `focus_verdict`.
            Event::PaneUpdate(_) => {
                let focused = get_focused_pane_info().ok().map(|(_, pane)| pane);
                if self.focus_verdict(focused) == Verdict::Dismiss {
                    self.dismiss();
                }
                false
            },
            // The tab the popup lives in was left behind. `PaneUpdate` is only
            // delivered to plugins in the client's *active* tab, so this is the
            // only word the popup gets that it is no longer in front of anyone.
            Event::Visible(false) => {
                self.dismiss();
                false
            },
            Event::PluginConfigurationChanged(config) => {
                self.config = config;
                self.apply_geometry();
                false
            },
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name != wire::ASK_ACK {
            return false;
        }
        let Some(ack) = pipe_message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<AskAck>(json).ok())
        else {
            // Unreadable: leave the countdown running rather than close on it.
            // Timing out into "hocus is not running" is a worse answer than the
            // truth, but it is at least an answer the user can act on.
            return false;
        };
        match ack.outcome {
            // Queued is as good as sent from here: hocus has the question and
            // has said so, and the popup's job is over either way.
            AskOutcome::Sent | AskOutcome::Queued => {
                self.dismiss();
                false
            },
            AskOutcome::Refused => {
                // The text stays in the buffer: a refusal is usually something
                // the user can fix, and retyping the question is a poor reward
                // for asking it.
                self.phase = Phase::Editing {
                    note: Some(ack.detail),
                };
                true
            },
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let (line, column) = viewport(
            PROMPT,
            self.input.get_text(),
            self.input.get_cursor_position(),
            cols,
        );

        // Must precede any print_*: the host reads the cursor position out of
        // the render output stream in order.
        match self.phase {
            Phase::Editing { .. } => show_cursor(Some((column, 0))),
            // Nothing to type into while the question is in the air.
            Phase::Sending { .. } => show_cursor(None),
        }

        print_text_with_coordinates(Text::new(&line), 0, 0, Some(cols), None);

        if rows > 1 {
            let note = match &self.phase {
                Phase::Sending { .. } => Some("sending…".to_owned()),
                Phase::Editing { note } => note.clone(),
            };
            if let Some(note) = note {
                print_text_with_coordinates(Text::new(&note).dim_range(..), 0, 1, Some(cols), None);
            }
        }
    }
}

impl State {
    /// Decide what a reading of the client's focused pane means for the popup.
    ///
    /// The `PaneManifest` that arrives with `PaneUpdate` cannot answer this,
    /// which is why the caller pays for a round trip to the host instead: a
    /// floating pane's `is_focused` there is true if *any* client focuses it,
    /// and the question being asked is about one client. `get_focused_pane_info`
    /// resolves through the client's own active pane, across the tiled and
    /// floating layers alike. This is `file-finder`'s trick run backwards — it
    /// bounces focus off itself, this closes when focus is off it.
    ///
    /// `was_focused` is not a defensive flag but the thing that makes any of
    /// this safe. The popup is one pane behind a plugin instance per client
    /// (see the module doc), and each instance is told about *its own* client's
    /// focus; an instance belonging to a client who never opened the popup
    /// would otherwise close it on its first update, out from under whoever is
    /// typing into it. It also means nothing has to be assumed about whether
    /// the pane was focused before the first event arrived.
    fn focus_verdict(&mut self, focused: Option<PaneId>) -> Verdict {
        if focused == Some(PaneId::Plugin(self.plugin_id)) {
            self.was_focused = true;
            return Verdict::Wait;
        }
        // The host could not say where focus is. Reading that as "elsewhere"
        // would throw away a half-typed question on no evidence, so it waits —
        // the same call `pipe` makes about an unreadable acknowledgement.
        if focused.is_none() {
            return Verdict::Wait;
        }
        if self.was_focused {
            Verdict::Dismiss
        } else {
            Verdict::Wait
        }
    }

    /// Close the popup, at most once.
    ///
    /// The latch is for `Event::Visible`, which the host broadcasts to every
    /// client's instance at once: without it each would send a `ClosePane` for
    /// the same pane. It also settles the race between a keypress and a focus
    /// reading that both decide the popup is over.
    fn dismiss(&mut self) {
        if !self.dismissed {
            self.dismissed = true;
            close_self();
        }
    }

    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if matches!(self.phase, Phase::Sending { .. }) {
            // The question is already out and cannot be recalled; the only
            // thing left to offer is a way out of the box.
            if key.bare_key == BareKey::Esc {
                self.dismiss();
            }
            return false;
        }
        match self.input.handle_key(key) {
            InputAction::Submit => self.send(),
            InputAction::Cancel => {
                self.dismiss();
                false
            },
            InputAction::Continue => {
                // Any edit retires the last failure: it described the text as
                // it was, and the user is answering it.
                self.phase = Phase::Editing { note: None };
                true
            },
            // Completing `@agents` and `/commands` needs the agent snapshot
            // that only hocus subscribes to. Out of scope until the popup has
            // a reason to hold one of its own.
            InputAction::Complete | InputAction::NoAction => false,
        }
    }

    /// Hand the question to whichever hocus is listening.
    ///
    /// Note this does *not* close the popup. It closes when hocus says it has
    /// the question — see `pipe` — because a broadcast into an empty session
    /// looks exactly like a successful one from here.
    fn send(&mut self) -> bool {
        let text = self.input.get_text().trim().to_owned();
        if text.is_empty() {
            // A bare Enter on an empty box is the same as changing your mind.
            self.dismiss();
            return false;
        }
        let Ok(payload) = serde_json::to_string(&Ask { text }) else {
            self.phase = Phase::Editing {
                note: Some("could not encode the question".to_owned()),
            };
            return true;
        };
        pipe_message_to_plugin(MessageToPlugin::new(wire::ASK).with_payload(payload));
        self.phase = Phase::Sending {
            remaining: ACK_TIMEOUT_SECONDS,
        };
        set_timeout(TICK_SECONDS);
        true
    }

    /// Count the wait down rather than act on a timer's arrival.
    ///
    /// Zellij has no `cancel_timeout`, so the timer armed for a question that
    /// was refused is still outstanding when the next one goes out. Keeping the
    /// deadline in state makes a stale timer cost at most one tick instead of
    /// cutting the new wait short.
    fn on_tick(&mut self, elapsed: f64) -> bool {
        let Phase::Sending { remaining } = &mut self.phase else {
            return false;
        };
        *remaining -= elapsed;
        if *remaining > 0.0 {
            set_timeout(TICK_SECONDS);
            return false;
        }
        self.phase = Phase::Editing {
            note: Some(NO_HOCUS.to_owned()),
        };
        true
    }

    /// Shrink the pane to popup size.
    ///
    /// The keybinding cannot do this: `LaunchOrFocusPlugin` has no coordinates,
    /// and the pipe paths hardcode them to `None`. Resizing ourselves is the
    /// only route to a *small* floating pane, and it is race-free — `AddPlugin`
    /// reaches the screen thread before this call can, so the pane always
    /// exists by the time the resize arrives.
    fn apply_geometry(&self) {
        let Some(coordinates) = floating_geometry(&self.config) else {
            return;
        };
        change_floating_panes_coordinates(vec![(PaneId::Plugin(self.plugin_id), coordinates)]);
    }
}

/// Read the popup's geometry out of the plugin's config.
///
/// `x`/`y`/`width`/`height` are not reserved words, so they reach `load`
/// untouched. Values are passed through verbatim: `FloatingPaneCoordinates::new`
/// decides whether `"60%"` or `"4"` is meaningful, and silently drops anything
/// it cannot parse — so a typo in the KDL costs a default-sized popup, not a
/// crash.
fn floating_geometry(config: &BTreeMap<String, String>) -> Option<FloatingPaneCoordinates> {
    let dimension = |key: &str, default: &str| {
        config
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or(default)
            .to_owned()
    };
    FloatingPaneCoordinates::new(
        Some(dimension("x", DEFAULT_X)),
        Some(dimension("y", DEFAULT_Y)),
        Some(dimension("width", DEFAULT_WIDTH)),
        Some(dimension("height", DEFAULT_HEIGHT)),
        None,
        None,
        None,
    )
    .map(FloatingPaneCoordinates::as_overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn ack(json: &str) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Plugin(1),
            name: wire::ASK_ACK.to_owned(),
            payload: Some(json.to_owned()),
            args: BTreeMap::new(),
            is_private: true,
        }
    }

    /// A popup that has been opened and focused, as every real one has been by
    /// the time a focus reading reaches it.
    fn open_popup() -> State {
        let mut state = State {
            plugin_id: 7,
            ..Default::default()
        };
        assert_eq!(
            state.focus_verdict(Some(PaneId::Plugin(7))),
            Verdict::Wait,
            "focus landing on the popup is what opening it looks like"
        );
        state
    }

    fn note(state: &State) -> Option<&str> {
        match &state.phase {
            Phase::Editing { note } => note.as_deref(),
            Phase::Sending { .. } => None,
        }
    }

    #[test]
    fn the_geometry_defaults_to_a_small_centred_popup() {
        let coordinates = floating_geometry(&BTreeMap::new()).expect("some geometry");
        let expected = FloatingPaneCoordinates::new(
            Some(DEFAULT_X.to_owned()),
            Some(DEFAULT_Y.to_owned()),
            Some(DEFAULT_WIDTH.to_owned()),
            Some(DEFAULT_HEIGHT.to_owned()),
            None,
            None,
            None,
        )
        .map(FloatingPaneCoordinates::as_overlay)
        .expect("some geometry");
        assert_eq!(coordinates, expected);
    }

    #[test]
    fn a_configured_dimension_wins_over_the_default() {
        let coordinates = floating_geometry(&config(&[("width", "30%")])).expect("some geometry");
        let expected = floating_geometry(&BTreeMap::new()).expect("some geometry");
        assert_ne!(coordinates.width, expected.width);
        assert_eq!(coordinates.height, expected.height);
    }

    /// A key left in the KDL with nothing after it is a half-finished edit, not
    /// a request for a zero-width pane.
    #[test]
    fn a_blank_dimension_falls_back_to_the_default() {
        let coordinates = floating_geometry(&config(&[("width", "   ")])).expect("some geometry");
        let expected = floating_geometry(&BTreeMap::new()).expect("some geometry");
        assert_eq!(coordinates, expected);
    }

    #[test]
    fn a_wait_that_runs_out_reports_that_nothing_heard_and_keeps_the_text() {
        let mut state = State::default();
        state.input.set_text("why is the build red?".to_owned());
        state.phase = Phase::Sending { remaining: 0.3 };

        assert!(!state.on_tick(0.2), "still waiting, nothing to redraw");
        assert_eq!(
            state.phase,
            Phase::Sending {
                remaining: 0.3 - 0.2
            }
        );

        assert!(state.on_tick(0.2), "gave up, so the note is new");
        assert_eq!(note(&state), Some(NO_HOCUS));
        assert_eq!(state.input.get_text(), "why is the build red?");
    }

    #[test]
    fn a_refusal_shows_its_reason_and_keeps_the_text() {
        let mut state = State::default();
        state.input.set_text("why is the build red?".to_owned());
        state.phase = Phase::Sending {
            remaining: ACK_TIMEOUT_SECONDS,
        };

        assert!(state.pipe(ack(r#"{"outcome":"Refused","detail":"hocus is busy"}"#)));
        assert_eq!(note(&state), Some("hocus is busy"));
        assert_eq!(state.input.get_text(), "why is the build red?");
    }

    /// The countdown has to survive an unreadable reply. Closing on one would
    /// swallow the question; clearing the phase would strand the popup with no
    /// deadline and no way out but Esc.
    #[test]
    fn an_unreadable_acknowledgement_leaves_the_wait_running() {
        let mut state = State {
            phase: Phase::Sending { remaining: 1.0 },
            ..Default::default()
        };

        assert!(!state.pipe(ack("not json")));
        assert_eq!(state.phase, Phase::Sending { remaining: 1.0 });
    }

    #[test]
    fn a_message_that_is_not_an_acknowledgement_is_ignored() {
        let mut state = State {
            phase: Phase::Sending { remaining: 1.0 },
            ..Default::default()
        };

        let mut other = ack(r#"{"outcome":"Sent"}"#);
        other.name = wire::AGENT_SNAPSHOT.to_owned();

        assert!(!state.pipe(other));
        assert_eq!(state.phase, Phase::Sending { remaining: 1.0 });
    }

    #[test]
    fn focus_on_the_popup_itself_keeps_it_up() {
        let mut state = open_popup();

        assert_eq!(state.focus_verdict(Some(PaneId::Plugin(7))), Verdict::Wait);
        assert!(state.was_focused);
    }

    #[test]
    fn focus_moving_to_another_pane_dismisses_the_popup() {
        let mut state = open_popup();

        assert_eq!(
            state.focus_verdict(Some(PaneId::Terminal(1))),
            Verdict::Dismiss
        );
    }

    /// The popup is one pane behind an instance per client, and each instance
    /// is told where *its* client is looking. An instance belonging to someone
    /// who never opened the popup must keep its opinion to itself, or it closes
    /// the box the other client is typing into.
    #[test]
    fn an_instance_that_never_had_focus_dismisses_nothing() {
        let mut state = State {
            plugin_id: 7,
            ..Default::default()
        };

        assert_eq!(
            state.focus_verdict(Some(PaneId::Terminal(1))),
            Verdict::Wait
        );
        assert_eq!(
            state.focus_verdict(Some(PaneId::Plugin(9))),
            Verdict::Wait,
            "another plugin's pane is no more evidence than a terminal's"
        );
    }

    /// Same reasoning as an unreadable acknowledgement: an answer that is not
    /// an answer is no grounds for throwing away what was typed.
    #[test]
    fn a_focus_reading_the_host_could_not_give_dismisses_nothing() {
        let mut state = open_popup();

        assert_eq!(state.focus_verdict(None), Verdict::Wait);
        assert!(
            state.was_focused,
            "and it does not disarm what was already established"
        );
        assert_eq!(
            state.focus_verdict(Some(PaneId::Terminal(1))),
            Verdict::Dismiss
        );
    }

    /// Walking away from a question already in the air still dismisses. The
    /// question reaches hocus either way; only the acknowledgement is lost, and
    /// there is by then nobody in front of the pane to read it.
    #[test]
    fn a_question_in_flight_does_not_hold_the_popup_open() {
        let mut state = open_popup();
        state.phase = Phase::Sending {
            remaining: ACK_TIMEOUT_SECONDS,
        };

        assert_eq!(
            state.focus_verdict(Some(PaneId::Terminal(1))),
            Verdict::Dismiss
        );
    }

    /// Typing is how you answer a complaint about what you typed.
    #[test]
    fn editing_after_a_refusal_retires_the_note() {
        let mut state = State {
            phase: Phase::Editing {
                note: Some("hocus is busy".to_owned()),
            },
            ..Default::default()
        };

        assert!(state.on_key(KeyWithModifier::new(BareKey::Char('x'))));
        assert_eq!(note(&state), None);
        assert_eq!(state.input.get_text(), "x");
    }
}
