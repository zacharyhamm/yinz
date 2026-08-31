use super::*;

#[test]
fn ctrl_g_is_not_handled_by_hocus() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));

    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()));
    assert_eq!(state.jobs.len(), 1);
}

#[test]
fn ctrl_backtick_is_the_hocus_toggle_key() {
    assert!(State::is_toggle_key(
        &KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier()
    ));
    assert!(!State::is_toggle_key(
        &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()
    ));
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
    assert_eq!(state.chat.anchor, None);
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
