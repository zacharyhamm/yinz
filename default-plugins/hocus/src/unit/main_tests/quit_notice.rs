//! What hocus tells the session about a quit.
//!
//! A quit takes the whole session with it, and the only thing that knows whether that would
//! throw away a reply in flight is hocus. These are the four answers it can give and the
//! memo that keeps it from repeating itself.

use super::*;

/// A chat with something in it and nothing outstanding: parked and settled.
fn settled_chat() -> crate::Chat {
    crate::Chat {
        messages: vec![Message::user("hello")],
        ..Default::default()
    }
}

/// A parked chat still waiting on its reply.
fn working_chat() -> crate::Chat {
    crate::Chat {
        messages: vec![Message::user("hello"), Message::assistant("")],
        stream: Some(StreamState {
            id: "hocus-parked".into(),
            finishing: false,
            usage: None,
        }),
        ..Default::default()
    }
}

/// What was last published, unwrapped.
fn published(state: &State) -> (Option<String>, bool) {
    state
        .last_quit_notice
        .clone()
        .expect("nothing has been published")
}

#[test]
fn an_empty_hocus_says_nothing_and_guards_nothing() {
    let mut state = idle_state();
    state.publish_quit_notice();
    assert_eq!(published(&state), (None, false));
}

#[test]
fn a_streaming_reply_guards_the_quit() {
    let mut state = ready_state();
    state.publish_quit_notice();
    let (notice, guard) = published(&state);
    assert!(guard, "a reply in flight is exactly what must not be lost");
    assert_eq!(
        notice.as_deref(),
        Some("hocus is still working on a conversation")
    );
}

/// A tool waiting on `y`/`n` is waiting on the *user*, which is the case where an accidental
/// quit is most likely and least recoverable: the question goes with the session.
#[test]
fn a_tool_awaiting_confirmation_guards_the_quit() {
    let mut state = idle_state();
    state.messages = vec![Message::user("delete it")];
    state.status = Status::ConfirmingTool;
    state.publish_quit_notice();
    assert!(published(&state).1);
}

#[test]
fn plan_mode_guards_the_quit() {
    let mut state = idle_state();
    state.messages = vec![Message::user("plan it")];
    state.plan = Plan::AwaitingApproval;
    state.publish_quit_notice();
    assert!(published(&state).1);
}

/// The foreground chat can be perfectly settled while a chat the user set aside is not. The
/// guard is about the session, not about what happens to be on screen.
#[test]
fn a_parked_chat_still_working_guards_the_quit() {
    let mut state = idle_state();
    state.messages = vec![Message::user("done here")];
    state.parked.push(working_chat());
    state.publish_quit_notice();
    let (notice, guard) = published(&state);
    assert!(guard);
    assert_eq!(
        notice.as_deref(),
        Some("hocus is still working on a conversation")
    );
}

#[test]
fn two_working_conversations_are_counted() {
    let mut state = ready_state();
    state.parked.push(working_chat());
    state.publish_quit_notice();
    assert_eq!(
        published(&state).0.as_deref(),
        Some("hocus is still working - 2 conversations")
    );
}

/// Settled conversations are still worth a line - the same confirmation goes up for `Ctrl-q`
/// and knowing what is about to go is worth saying - but they do not stand in the way.
#[test]
fn settled_conversations_are_reported_without_guarding() {
    let mut state = idle_state();
    state.messages = vec![Message::user("thanks")];
    state.parked.push(settled_chat());
    state.publish_quit_notice();
    let (notice, guard) = published(&state);
    assert!(!guard, "a quit the user asked for is theirs to make");
    assert_eq!(
        notice.as_deref(),
        Some("hocus: 2 conversations open, all settled")
    );
}

#[test]
fn one_settled_conversation_reads_as_one() {
    let mut state = idle_state();
    state.messages = vec![Message::user("thanks")];
    state.publish_quit_notice();
    assert_eq!(
        published(&state).0.as_deref(),
        Some("hocus: one conversation open, settled")
    );
}

/// The guard has to come down, or the session can never close its last pane again.
#[test]
fn the_guard_is_withdrawn_when_the_reply_lands() {
    let mut state = ready_state();
    state.publish_quit_notice();
    assert!(published(&state).1);

    settle(&mut state);
    state.publish_quit_notice();
    let (notice, guard) = published(&state);
    assert!(!guard);
    assert_eq!(
        notice.as_deref(),
        Some("hocus: one conversation open, settled")
    );
}

/// This runs on every update - every keystroke and every streamed chunk - and each publish is
/// a host instruction. Restating an unchanged notice ten times a second is what buried the
/// user's input behind our own traffic the last time a per-update host call went unmemoised.
#[test]
fn an_unchanged_notice_is_not_republished() {
    let mut state = ready_state();
    state.publish_quit_notice();
    let after_first = state.quit_notice_publishes;
    for _ in 0..5 {
        state.publish_quit_notice();
    }
    assert_eq!(
        state.quit_notice_publishes, after_first,
        "nothing changed, so nothing should have gone out"
    );

    settle(&mut state);
    state.publish_quit_notice();
    assert_eq!(
        state.quit_notice_publishes,
        after_first + 1,
        "the guard coming down is a change and must go out"
    );
}

/// A guard outliving the plugin that set it is a session that will not close its last pane and
/// cannot say why. The server drops the notice when the pane goes, but hocus does not rely on
/// that ordering.
#[test]
fn closing_withdraws_the_guard() {
    let mut state = ready_state();
    state.publish_quit_notice();
    assert!(published(&state).1);

    state.handle(Event::BeforeClose);
    assert_eq!(published(&state), (None, false));
}
