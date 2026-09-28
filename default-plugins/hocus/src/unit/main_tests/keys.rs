use super::*;

#[test]
fn ctrl_g_is_not_handled_by_hocus() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));

    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()));
    assert_eq!(state.jobs.len(), 1);
}

/// Ctrl-` was a second, hide-only dismissal beside Esc, bound in no shipped keymap. Esc is the
/// whole of that gesture now, so the key is the terminal's again and hocus types it.
#[test]
fn ctrl_backtick_is_not_a_hocus_gesture() {
    let mut state = idle_state();
    state.on_key(KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier());
    assert!(!state.hidden, "no longer a dismissal");
}

#[test]
fn ctrl_o_is_the_reposition_key() {
    assert!(State::is_reposition_key(
        &KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier()
    ));
    assert!(!State::is_reposition_key(&KeyWithModifier::new(
        BareKey::Char('o')
    )));
}

/// Ctrl-o is taken before the input box sees it, and asks for a redraw of nothing: a hocus that
/// has no manifest yet has nowhere to go, and the key must still not be typed.
#[test]
fn ctrl_o_is_not_typed_into_the_input() {
    let mut state = idle_state();
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier()));
    assert!(state.input.get_text().is_empty());
    assert_eq!(state.panel.placed_corner, None);
}

// -- Esc hides, Ctrl-c interrupts ---------------------------------------
//
// The two used to be one key, resolved by precedence: a busy tool queue took
// Esc, then a live stream, and only an idle hocus was put away by it. That left
// no way at all to set the overlay aside while a reply was arriving - the very
// state you most want it gone from - without also killing the reply. The tests
// below pin both halves of the split.

fn esc() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Esc)
}

fn ctrl_c() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier()
}

/// A state at the y/n prompt, with `call_1` waiting on the answer.
fn confirming_state() -> State {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":3,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::ConfirmingTool));
    state
}

#[test]
fn esc_hides_while_streaming_rather_than_cancelling() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");

    state.on_key(esc());

    assert!(state.hidden, "Esc puts the conversation away");
    assert!(
        state.is_streaming() && state.is_receiving(),
        "and the reply is still arriving into the hidden pane"
    );
}

#[test]
fn esc_hides_while_a_tool_runs() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;

    state.on_key(esc());

    assert!(state.hidden);
    assert!(
        matches!(state.status, Status::AwaitingCommand),
        "the run is untouched"
    );
}

#[test]
fn esc_at_the_confirmation_prompt_hides_with_the_call_still_pending() {
    let mut state = confirming_state();

    state.on_key(esc());

    assert!(state.hidden);
    assert!(
        matches!(state.status, Status::ConfirmingTool),
        "the question is still being asked"
    );
    assert_eq!(state.pending.len(), 1, "and the call still waits on it");
}

/// The key the user actually presses: `n` at the prompt answers the call and
/// leaves the chat waiting on them, not on the model.
#[test]
fn n_at_the_confirmation_prompt_declines_and_waits() {
    let mut state = confirming_state();

    state.on_key(KeyWithModifier::new(BareKey::Char('n')));

    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty(), "the call was answered");
    assert!(!state.is_receiving(), "and no follow-up request went out");
}

// -- the input box lets go of the ring, and takes it back --------------------
//
// Bare Tab in the box is completion, which leaves nothing for the ring's forward step to
// mean while a draft is standing in it. Esc is what lets go: the text stays, the keys go
// back to the ring, and anything typed takes the box again. The tests above are the other
// half of that split - every one of them presses Esc on an *empty* box, which is why they
// still hide on the first press.

/// The draft is the one thing a dismissal cannot get back, so Esc protects it instead.
#[test]
fn esc_with_a_draft_releases_the_box_rather_than_hiding_it() {
    let mut state = ready_state();
    state.input.set_text("half a thought".to_owned());

    assert!(state.on_key(esc()));

    assert!(!state.hidden, "the panel is still up");
    assert!(state.chat_input_released, "and the box has let the keys go");
    assert_eq!(
        state.input.get_text(),
        "half a thought",
        "with the draft in it"
    );
}

/// And the way out is the same key again - a two-press escape hatch, not a lost one.
#[test]
fn a_second_esc_hides_the_released_panel() {
    let mut state = ready_state();
    state.input.set_text("half a thought".to_owned());
    state.on_key(esc());

    state.on_key(esc());

    assert!(state.hidden);
    assert_eq!(
        state.input.get_text(),
        "half a thought",
        "the draft survives"
    );
}

/// A box with only spaces in it has nothing to complete and so nothing to protect.
#[test]
fn esc_on_a_box_of_whitespace_hides() {
    let mut state = ready_state();
    state.input.set_text("   ".to_owned());

    state.on_key(esc());

    assert!(state.hidden);
}

/// Typing is how the box takes the keys back, and the key that says so is also typed: the
/// arm that clears the flag does not consume anything.
#[test]
fn typing_into_a_released_box_puts_the_keys_back() {
    let mut state = ready_state();
    state.input.set_text("hi".to_owned());
    state.on_key(esc());

    state.on_key(KeyWithModifier::new(BareKey::Char('!')));

    assert!(!state.chat_input_released);
    assert_eq!(state.input.get_text(), "hi!");
}

/// Enter is typing too, and means what it always means in the box.
#[test]
fn enter_in_a_released_box_sends_it() {
    let mut state = ready_state();
    state.input.set_text("ship it".to_owned());
    state.on_key(esc());

    state.on_key(KeyWithModifier::new(BareKey::Enter));

    assert!(!state.chat_input_released);
    assert_eq!(state.input.get_text(), "", "the box was sent and cleared");
}

/// The arms that answer a prompt or interrupt a run return early, so the flag has to be down
/// before them: a `y` that ran a tool used to be able to leave the box released with nothing
/// on screen saying so.
#[test]
fn answering_a_prompt_from_a_released_box_takes_the_keys_back() {
    let mut state = confirming_state();
    state.input.set_text("wait".to_owned());
    state.on_key(esc());
    assert!(state.chat_input_released);

    state.on_key(KeyWithModifier::new(BareKey::Char('n')));

    assert!(!state.chat_input_released, "the box has the keys again");
    assert!(state.pending.is_empty(), "and the call was still answered");
}

#[test]
fn interrupting_from_a_released_box_takes_the_keys_back() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.input.set_text("wait".to_owned());
    state.on_key(esc());

    state.on_key(ctrl_c());

    assert!(!state.chat_input_released);
    assert!(!state.hidden, "interrupting is not hiding");
}

/// A paste is typing, arriving whole.
#[test]
fn a_paste_into_a_released_box_takes_the_keys_back() {
    let mut state = ready_state();
    state.input.set_text("see ".to_owned());
    state.on_key(esc());

    assert!(state.on_paste("this".to_owned()));

    assert!(!state.chat_input_released);
    assert_eq!(state.input.get_text(), "see this");
}

// -- the ring's forward step out of the box ---------------------------------

/// Nothing to complete, so Tab is the ring's: on round to the panel's own tab bar, the stop
/// after the box.
#[test]
fn tab_on_a_blank_box_steps_to_the_panel_tabs() {
    let mut state = ready_state();
    state.agents_plugin = Some(yinz_tile::prelude::PluginId::new(7));

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));

    assert_eq!(
        state.ring_focus_calls,
        [(
            PaneId::Plugin(yinz_tile::prelude::PluginId::new(7)),
            hocus_proto::vms::STOP_TABS
        )]
    );
}

/// A draft keeps Tab for what it is for. This is the whole of the conflict, and the reason
/// Esc grew a second job.
#[test]
fn tab_with_a_draft_still_completes() {
    let mut state = ready_state();
    state.agents_plugin = Some(yinz_tile::prelude::PluginId::new(7));
    state.input.set_text("/hist".to_owned());

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));

    assert_eq!(state.input.get_text(), "/history ");
    assert!(state.ring_focus_calls.is_empty(), "and went nowhere");
}

/// Once the box is let go of, the same press steps - the draft still standing in it.
#[test]
fn tab_from_a_released_box_steps_to_the_agent_list() {
    let mut state = ready_state();
    state.agents_plugin = Some(yinz_tile::prelude::PluginId::new(7));
    state.input.set_text("/hist".to_owned());
    state.on_key(esc());

    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));

    assert_eq!(
        state.ring_focus_calls,
        [(
            PaneId::Plugin(yinz_tile::prelude::PluginId::new(7)),
            hocus_proto::vms::STOP_TABS
        )]
    );
    assert_eq!(
        state.input.get_text(),
        "/hist",
        "completed nothing on the way"
    );
    assert!(
        !state.chat_input_released,
        "and leaving resets the stop: coming back should type"
    );
}

/// A hocus in the user's layout is on no ring - it owns no second half and can hide nothing -
/// so Tab there is the completion it always was, and Esc does nothing at all.
#[test]
fn a_tiled_hocus_has_no_ring() {
    let mut state = ready_state();
    state.own_is_floating = Some(false);
    state.agents_plugin = Some(yinz_tile::prelude::PluginId::new(7));

    assert!(
        !state.on_key(KeyWithModifier::new(BareKey::Tab)),
        "nothing to complete"
    );
    assert!(state.ring_focus_calls.is_empty(), "and nowhere to step");

    state.input.set_text("a draft".to_owned());
    state.on_key(esc());
    assert!(!state.hidden, "hide_panel refuses for a pane in a layout");
    assert!(
        !state.chat_input_released,
        "and there is no ring to release to"
    );
}

#[test]
fn ctrl_c_cancels_the_stream() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");

    state.on_key(ctrl_c());

    assert!(!state.hidden, "interrupting is not hiding");
    assert!(
        state.stream == finishing("hocus-1"),
        "cancelled, but still owed the host's last chunk"
    );
}

#[test]
fn ctrl_c_aborts_a_running_tool() {
    let mut state = confirming_state();

    state.on_key(ctrl_c());

    assert!(!state.hidden);
    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty(), "every call got an answer");
    assert!(
        state
            .messages
            .iter()
            .any(|message| message.content.contains("tools aborted")),
        "and the transcript says so"
    );
}

/// The same precedence Esc used to have, now Ctrl-c's: both outstanding means
/// the tool goes first, because it is the nearer thing.
#[test]
fn ctrl_c_takes_the_tool_before_the_stream() {
    let mut state = confirming_state();
    state.stream = Some(StreamState {
        id: "hocus-1".into(),
        finishing: false,
        usage: None,
    });

    state.on_key(ctrl_c());

    assert!(matches!(state.status, Status::Idle), "the tool was aborted");
    assert!(state.is_receiving(), "and the stream was left alone");
}

#[test]
fn ctrl_c_clears_the_prompt_when_there_is_nothing_to_interrupt() {
    let mut state = idle_state();
    state.input.insert_text("half a question");

    assert!(state.on_key(ctrl_c()), "the prompt is redrawn");

    assert_eq!(state.input.get_text(), "");
    assert!(!state.hidden, "and hocus stays up");
}
