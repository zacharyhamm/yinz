use super::*;

// The tab bar: every open chat, in a fixed order, with the foreground one
// spliced into `parked` at `chat_tab`.

fn shift_tab() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Tab).with_shift_modifier()
}

fn key(bare: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare)
}

fn chat(id: u64, first: &str) -> Chat {
    Chat {
        messages: vec![Message::user(first)],
        chat_id: Some(id),
        ..Chat::default()
    }
}

/// Three settled chats open, the middle one on screen: bar = [1, 2*, 3].
fn three_tabs() -> State {
    let mut state = ready_state();
    settle(&mut state);
    state.messages = vec![Message::user("two")];
    state.chat_id = Some(2);
    state.parked = vec![chat(1, "one"), chat(3, "three")];
    state.chat_tab = 1;
    state
}

/// The bar as the user reads it: chat ids left to right.
fn bar(state: &State) -> Vec<Option<u64>> {
    (0..state.chat_tab_count())
        .map(|tab| match state.parked_at_tab(tab) {
            None => state.chat_id,
            Some(index) => state.parked[index].chat_id,
        })
        .collect()
}

// -- focus ------------------------------------------------------------------

/// Shift+Tab is Tab with a modifier, which the input box would otherwise take
/// as a completion request.
#[test]
fn shift_tab_focuses_the_bar_on_the_active_chat_without_typing() {
    let mut state = three_tabs();
    state.input.set_text("/his".to_owned());

    assert!(state.on_key(shift_tab()));
    assert_eq!(state.chat_tab_focus, Some(1), "opens on the chat on screen");
    assert_eq!(state.input.get_text(), "/his", "and completed nothing");
}

#[test]
fn the_highlight_moves_with_the_arrows_and_stops_at_both_ends() {
    let mut state = three_tabs();
    state.on_key(shift_tab());

    state.on_key(key(BareKey::Left));
    assert_eq!(state.chat_tab_focus, Some(0));
    state.on_key(key(BareKey::Left));
    assert_eq!(state.chat_tab_focus, Some(0), "already at the left edge");

    state.on_key(key(BareKey::Right));
    state.on_key(key(BareKey::Tab));
    assert_eq!(state.chat_tab_focus, Some(2), "Tab walks right too");
    state.on_key(key(BareKey::Right));
    assert_eq!(state.chat_tab_focus, Some(2), "and stops at the right edge");

    state.on_key(shift_tab());
    assert_eq!(state.chat_tab_focus, Some(1), "Shift+Tab walks left");
}

/// A keystroke falling through to the input box behind the bar would be typed
/// invisibly, exactly as it would behind the picker.
#[test]
fn the_bar_swallows_keys_it_does_not_use_and_esc_gives_the_keyboard_back() {
    let mut state = three_tabs();
    state.on_key(shift_tab());

    assert!(state.on_key(key(BareKey::Char('q'))));
    assert_eq!(state.input.get_text(), "", "nothing was typed");
    assert!(!state.on_paste("pasted".to_owned()));
    assert_eq!(state.input.get_text(), "", "nor pasted");

    state.on_key(key(BareKey::Esc));
    assert_eq!(state.chat_tab_focus, None);
    state.on_key(key(BareKey::Char('q')));
    assert_eq!(state.input.get_text(), "q", "the box has the keys again");
}

// -- switching --------------------------------------------------------------

#[test]
fn enter_switches_to_the_highlighted_chat_and_the_bar_keeps_its_order() {
    let mut state = three_tabs();
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3)]);

    state.on_key(shift_tab());
    state.on_key(key(BareKey::Right));
    assert!(state.on_key(key(BareKey::Enter)));

    assert_eq!(state.chat_id, Some(3), "three is on screen");
    assert_eq!(state.chat_tab, 2, "in the same place it was in the bar");
    assert_eq!(state.chat_tab_focus, None, "and the keyboard is back");
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3)]);

    state.switch_to_tab(0);
    assert_eq!(state.chat_id, Some(1));
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3)]);

    state.switch_to_tab(1);
    assert_eq!(state.chat_id, Some(2));
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3)], "round trip");
    assert_eq!(state.messages[0].content, "two", "with its own transcript");
}

#[test]
fn the_draft_follows_its_chat() {
    let mut state = three_tabs();
    state.input.set_text("half a thou".to_owned());

    state.switch_to_tab(0);
    assert_eq!(state.input.get_text(), "", "the other chat has no draft");
    state.input.set_text("something else".to_owned());

    state.switch_to_tab(1);
    assert_eq!(state.input.get_text(), "half a thou");
    state.switch_to_tab(0);
    assert_eq!(state.input.get_text(), "something else");
}

/// `/history` on a chat that is already open switches to its tab rather than
/// loading a second copy of it from the archive.
#[test]
fn opening_a_chat_that_is_already_open_switches_to_its_tab() {
    let mut state = three_tabs();
    state.switch_to_chat(3);

    assert_eq!(state.chat_id, Some(3));
    assert_eq!(state.chat_tab_count(), 3, "no tab was added");
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3)]);
}

/// One loaded from the archive is a new tab on the end.
#[test]
fn opening_an_archived_chat_adds_a_tab_on_the_end() {
    let mut state = three_tabs();
    state.messages = vec![Message::user("was here"), Message::assistant("yes")];
    state.flush_save();
    let saved = state.chat_id.expect("the flush allocated an id");
    state.new_chat();
    assert_eq!(state.chat_tab_count(), 4, "a fresh tab on the end");
    // Forget the saved one so the only way back is the archive.
    let index = state
        .parked
        .iter()
        .position(|chat| chat.chat_id == Some(saved))
        .expect("parked");
    state.parked.remove(index);
    state.chat_tab -= 1;

    state.switch_to_chat(saved);
    assert_eq!(state.chat_id, Some(saved));
    assert_eq!(state.chat_tab, 3, "on the end");
    assert_eq!(state.messages.len(), 2, "read back whole");
}

// -- new chat ---------------------------------------------------------------

#[test]
fn ctrl_l_opens_a_new_tab_and_keeps_the_old_one_as_it_was() {
    let mut state = three_tabs();
    state.input.set_text("unsent".to_owned());
    assert!(state.on_key(key(BareKey::Char('l')).with_ctrl_modifier()));

    assert_eq!(state.chat_tab_count(), 4);
    assert_eq!(state.chat_tab, 3, "the new chat is on the end");
    assert_eq!(state.chat_id, None);
    assert!(state.messages.is_empty());
    assert_eq!(state.input.get_text(), "", "with an empty prompt");
    assert_eq!(bar(&state), [Some(1), Some(2), Some(3), None]);

    state.switch_to_tab(1);
    assert_eq!(state.messages[0].content, "two");
    assert_eq!(
        state.input.get_text(),
        "unsent",
        "the draft came back with it"
    );
}

/// Nothing typed and nothing saved is not worth a tab.
#[test]
fn a_new_chat_from_an_empty_one_opens_no_second_tab() {
    let mut state = ready_state();
    settle(&mut state);
    state.messages.clear();

    state.new_chat();
    assert_eq!(state.chat_tab_count(), 1);
}

/// A reply still arriving when Ctrl-l is pressed keeps arriving, into the tab
/// that asked for it.
#[test]
fn a_new_tab_leaves_the_old_reply_streaming() {
    let mut state = ready_state();
    state.new_chat();

    assert_eq!(state.chat_tab_count(), 2);
    assert!(!state.is_streaming(), "the new chat is idle");
    state.handle(Event::WebRequestStreamChunk {
        stream_id: "hocus-1".into(),
        chunk: delta_frame("still coming"),
        done: false,
        error: None,
    });
    assert_eq!(state.parked[0].messages[1].content, "still coming");
    assert!(state.messages.is_empty(), "and nothing landed here");
}

// -- closing ----------------------------------------------------------------

#[test]
fn closing_a_background_tab_removes_it_and_keeps_the_foreground_in_place() {
    let mut state = three_tabs();
    state.on_key(shift_tab());
    state.on_key(key(BareKey::Left));
    assert!(state.on_key(key(BareKey::Char('x'))));

    assert_eq!(bar(&state), [Some(2), Some(3)]);
    assert_eq!(state.chat_id, Some(2), "still looking at the same chat");
    assert_eq!(state.chat_tab, 0, "which moved left with the bar");
    assert_eq!(state.chat_tab_focus, Some(0), "the highlight stays put");
}

#[test]
fn closing_the_foreground_tab_moves_to_its_right_hand_neighbour() {
    let mut state = three_tabs();
    state.close_tab(1);

    assert_eq!(bar(&state), [Some(1), Some(3)]);
    assert_eq!(state.chat_id, Some(3));
    assert_eq!(state.chat_tab, 1);
}

#[test]
fn closing_the_last_tab_on_the_right_moves_left() {
    let mut state = three_tabs();
    state.switch_to_tab(2);
    state.close_tab(2);

    assert_eq!(bar(&state), [Some(1), Some(2)]);
    assert_eq!(state.chat_id, Some(2));
    assert_eq!(state.chat_tab, 1);
}

#[test]
fn closing_a_working_chat_stops_it() {
    let mut state = three_tabs();
    state.parked[1].stream = Some(StreamState {
        id: "hocus-9".into(),
        finishing: false,
    });
    state.parked[1].messages.push(Message::assistant(""));
    state.parked[1].reply_at = Some(1);

    state.close_tab(2);
    assert_eq!(bar(&state), [Some(1), Some(2)]);

    // A late chunk for the closed chat has nowhere to land.
    let before = state.messages.clone();
    state.handle(Event::WebRequestStreamChunk {
        stream_id: "hocus-9".into(),
        chunk: delta_frame("late"),
        done: false,
        error: None,
    });
    assert_eq!(state.messages, before);
    assert_eq!(state.parked[0].messages.len(), 1);
}

#[test]
fn closing_the_only_tab_leaves_a_fresh_empty_chat() {
    let mut state = ready_state();
    settle(&mut state);
    state.chat_id = Some(1);

    state.close_tab(0);
    assert_eq!(state.chat_tab_count(), 1);
    assert_eq!(state.chat_id, None);
    assert!(state.messages.is_empty());
}

/// Mid-reply, the only tab is still closed rather than parked behind a new
/// one: closing is the one thing that does throw a chat away.
#[test]
fn closing_the_only_tab_mid_reply_does_not_park_it() {
    let mut state = ready_state();
    state.close_tab(0);

    assert_eq!(state.chat_tab_count(), 1, "no second tab");
    assert!(!state.is_streaming());
    assert!(state.messages.is_empty());
}

// -- what travels with a chat ----------------------------------------------

/// A job armed in one chat fires there, whether or not it is on screen.
#[test]
fn a_parked_chats_job_fires_into_that_chat() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check pane 3"}"#));
    state.new_chat();
    assert!(state.jobs.is_empty(), "the new chat has no jobs of its own");
    assert_eq!(state.parked[0].jobs.len(), 1, "the old one kept it");

    assert!(state.on_timer(60.0), "due, and fired off-screen");
    assert!(state.messages.is_empty(), "nothing landed in the new chat");
    let older = &state.parked[0];
    assert!(older
        .messages
        .iter()
        .any(|m| m.content == "[scheduled] check pane 3"));
    assert!(older.stream.is_some(), "and it sent");
}

/// A question queued behind one chat's reply is that chat's to send, not the
/// next tool batch's wherever it happens to run.
#[test]
fn a_queued_question_stays_with_the_chat_it_was_asked_in() {
    let mut state = ready_state();
    state.input.set_text("one more thing".to_owned());
    state.submit();
    assert_eq!(state.asks.len(), 1);

    state.new_chat();
    assert!(state.asks.is_empty(), "the new chat has no queue");
    assert_eq!(state.parked[0].asks.len(), 1);

    // The old reply ends off-screen, and its question goes out on the next tick.
    state.handle(Event::WebRequestStreamChunk {
        stream_id: "hocus-1".into(),
        chunk: Vec::new(),
        done: true,
        error: None,
    });
    state.on_timer(FRAME_SECONDS);
    assert!(state.parked[0].asks.is_empty(), "sent");
    assert!(state.parked[0].stream.is_some(), "from the chat that asked");
    assert!(!state.is_streaming(), "and not from this one");
}

/// Opening `/history` writes every tab, but an idle one that has not changed
/// since its last write, or that has nothing in it, is left alone.
#[test]
fn save_parked_skips_empty_and_unchanged_chats() {
    let mut state = three_tabs();
    state.parked.push(Chat::default());
    state.save_parked();
    let ids: Vec<Option<u64>> = state.parked.iter().map(|c| c.chat_id).collect();
    assert_eq!(ids[2], None, "an empty tab leaves no row");
    assert_eq!(
        state.parked[0].saved,
        state.parked[0].fingerprint(),
        "a written one is recorded as such"
    );

    let listed = history::list(&state.private_store()).len();
    state.save_parked();
    assert_eq!(
        history::list(&state.private_store()).len(),
        listed,
        "and writing again changes nothing"
    );
}
