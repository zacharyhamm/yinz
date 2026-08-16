use super::*;

#[test]
fn scrolling_down_stops_at_the_bottom() {
    let mut state = State::default();
    state.scroll_down(5);
    assert_eq!(state.scroll_offset(), 0);
}

/// One shared instance serves every client, so scrolling has to be told
/// apart by client or one person reading back drags everyone with them.
#[test]
fn each_client_scrolls_its_own_copy() {
    let mut state = State {
        client: 1,
        ..Default::default()
    };

    state.scroll_up(4);
    state.client = 2;
    state.scroll_up(9);

    assert_eq!(state.scroll_offset(), 9);
    state.client = 1;
    assert_eq!(state.scroll_offset(), 4);

    // A new message is everyone's, so it puts everyone back on the tail.
    state.pin_to_bottom();
    assert_eq!(state.scroll_offset(), 0);
    state.client = 2;
    assert_eq!(state.scroll_offset(), 0);
}

/// The anchor is what stops an arriving reply scrolling its own first line
/// off the top, and scrolling by hand is how you say you don't want it.
#[test]
fn scrolling_releases_only_the_client_that_scrolled() {
    let mut state = State::default();
    state.view.anchor = Some(3);

    state.client = 1;
    state.scroll_up(4);

    assert_eq!(state.anchor(), None, "the client that scrolled");
    state.client = 2;
    assert_eq!(state.anchor(), Some(3), "the client that did not");
}

/// Whatever just changed the conversation is not the reply anyone was being
/// held to, so the hold and the handover both go.
#[test]
fn pinning_to_the_bottom_drops_the_anchor_and_re_arms_everyone() {
    let mut state = State::default();
    state.view.anchor = Some(3);
    state.scroll_up(4);

    state.pin_to_bottom();

    assert_eq!(state.scroll_offset(), 0);
    assert_eq!(state.view.anchor, None);
    assert!(state.view.released.is_empty());
}

/// `pop_empty_placeholder` is the one place message indices move. An anchor
/// left behind would hold whatever slid into the removed message's place.
#[test]
fn dropping_the_placeholder_drops_the_anchor_with_it() {
    let mut state = State {
        messages: vec![Message::user("hi"), Message::assistant("")],
        reply_at: Some(1),
        ..Default::default()
    };
    state.view.anchor = Some(1);

    state.pop_empty_placeholder();

    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.view.anchor, None);
}

/// Nothing is removed when the reply did arrive, so the indices — and the
/// anchor — still hold. Cancelling one part way leaves its opening on screen.
#[test]
fn a_reply_cut_short_keeps_its_anchor() {
    let mut state = State {
        messages: vec![Message::user("hi"), Message::assistant("half a th")],
        reply_at: Some(1),
        ..Default::default()
    };
    state.view.anchor = Some(1);

    state.pop_empty_placeholder();

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.view.anchor, Some(1));
}

/// A parked chat pins and anchors as its own reply arrives. Those belong to
/// it, or someone else's tool result would move the screen you are reading.
#[test]
fn parking_a_chat_takes_its_viewport_with_it() {
    let mut state = State::default();
    state.view.anchor = Some(3);
    state.client = 1;
    state.scroll_up(4);

    let parked = state.take_chat();
    assert_eq!(state.anchor(), None, "the empty chat left behind");
    assert_eq!(state.scroll_offset(), 0);

    state.put_chat(parked);
    assert_eq!(state.view.anchor, Some(3));
    assert_eq!(state.scroll_offset(), 4);
    assert!(state.view.released.contains(&1));
}
