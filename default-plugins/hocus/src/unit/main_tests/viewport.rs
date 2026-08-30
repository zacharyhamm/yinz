use super::*;

#[test]
fn scrolling_down_stops_at_the_bottom() {
    let mut state = State::default();
    state.scroll_down(5);
    assert_eq!(state.scroll_offset(), 0);
}

/// The first scroll releases the client from the anchor, which is visible;
/// spinning the wheel against the bottom after that is not, and asking the
/// host for a render per notch is what made a long chat feel sluggish.
#[test]
fn scrolling_down_at_the_bottom_does_not_ask_for_a_render() {
    let mut state = State::default();
    assert!(state.scroll_down(5), "the first scroll releases the client");
    assert!(!state.scroll_down(5), "already released and at the bottom");
    assert_eq!(state.scroll_offset(), 0);
}

/// Before a render has measured the transcript there is nothing to clamp
/// against, so a scroll up must assume it can move.
#[test]
fn an_unrendered_state_always_renders_on_scroll_up() {
    let mut state = State::default();
    assert!(state.scroll_up(3));
    assert!(state.scroll_up(3), "still nothing measured");
    assert_eq!(state.scroll_offset(), 6);
}

/// A maximum the render measured is only worth clamping against while the
/// transcript is what the render saw. Once a message grows or arrives — a
/// chunk in the same batch as the notch — the notch has somewhere to go, and
/// the render will clamp it against the real count.
#[test]
fn a_transcript_that_changed_since_the_render_makes_the_measured_max_stale() {
    let mut state = State {
        messages: vec![Message::user("hi"), Message::assistant("there")],
        ..Default::default()
    };
    ui::cached_transcript(&mut state, 40);
    state.set_max_scroll_offset(5);

    assert!(state.scroll_up(5), "up to the measured top");
    assert!(
        !state.scroll_up(1),
        "measured and unchanged: nothing to show"
    );
    assert_eq!(state.scroll_offset(), 5);

    state
        .messages
        .last_mut()
        .unwrap()
        .content
        .push_str(" (more)");
    assert!(state.scroll_up(1), "the reply grew, so the top moved");
    assert_eq!(state.scroll_offset(), 6);

    ui::cached_transcript(&mut state, 40);
    state.set_max_scroll_offset(6);
    state.messages.push(Message::user("again"));
    assert!(state.scroll_up(1), "a new message, likewise");
    assert_eq!(state.scroll_offset(), 7);
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

/// The measured maximum is deliberately *one* number for every client, unlike
/// the positions around it. The host renders a shared plugin off a single
/// instance at a single size (`render_plugin_for_clients` reads `rows`/`cols`
/// once, then loops the clients) and the session is reflowed to the smallest
/// attached client, so every client measures the same transcript against the
/// same height and would only ever write the same number back. What stays per
/// client is where each of them is standing in it.
#[test]
fn two_clients_share_one_measured_maximum_but_not_a_position() {
    let mut state = State {
        client: 1,
        messages: vec![Message::user("hi"), Message::assistant("there")],
        ..Default::default()
    };
    ui::cached_transcript(&mut state, 40);
    state.set_max_scroll_offset(5);

    assert!(state.scroll_up(5), "client 1 up to the measured top");
    assert!(!state.scroll_up(1), "and clamped there");
    assert_eq!(state.scroll_offset(), 5);

    state.client = 2;
    assert_eq!(state.scroll_offset(), 0, "client 2 was never moved");
    assert!(
        state.scroll_up(9),
        "and reaches the same top from its own place"
    );
    assert_eq!(
        state.scroll_offset(),
        5,
        "clamped by the same maximum, not by client 1's position"
    );

    state.client = 1;
    assert_eq!(state.scroll_offset(), 5, "client 1 stayed where it was");
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
