use super::*;

// The tab bar: every open chat, in a fixed order, with the foreground one
// spliced into `parked` at `chat_tab`.

fn shift_tab() -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Tab).with_shift_modifier()
}

fn key(bare: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare)
}

/// Three settled chats open, the middle one on screen: bar = [1, 2*, 3].
fn three_tabs() -> State {
    let mut state = idle_state();
    for (index, text) in ["one", "two", "three"].into_iter().enumerate() {
        if index > 0 {
            state.new_chat();
        }
        state.messages = vec![Message::user(text)];
        state.flush_save();
    }
    state.switch_to_tab(1);
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
///
/// Also the ring's backward step out of the box, and the one that works with a draft in it:
/// there is no reverse-completion for Shift+Tab to collide with, so it never has to be
/// released first.
#[test]
fn shift_tab_focuses_the_bar_on_the_active_chat_without_typing() {
    let mut state = three_tabs();
    state.input.set_text("/his".to_owned());

    assert!(state.on_key(shift_tab()));
    assert_eq!(state.chat_tab_focus, Some(1), "opens on the chat on screen");
    assert_eq!(state.input.get_text(), "/his", "and completed nothing");
}

/// The arrows choose a chat; Tab and Shift+Tab leave the bar, because it is a stop on the
/// panel's ring rather than a list to walk. This used to be four keys doing two jobs.
#[test]
fn the_highlight_moves_with_the_arrows_and_stops_at_both_ends() {
    let mut state = three_tabs();
    state.on_key(shift_tab());

    state.on_key(key(BareKey::Left));
    assert_eq!(state.chat_tab_focus, Some(0));
    state.on_key(key(BareKey::Left));
    assert_eq!(state.chat_tab_focus, Some(0), "already at the left edge");

    state.on_key(key(BareKey::Right));
    state.on_key(key(BareKey::Right));
    assert_eq!(state.chat_tab_focus, Some(2), "walked right to the far end");
    state.on_key(key(BareKey::Right));
    assert_eq!(state.chat_tab_focus, Some(2), "and stops at the right edge");
}

/// Forward off the bar is the box below it, typing.
#[test]
fn tab_at_the_bar_steps_down_into_the_box() {
    let mut state = three_tabs();
    state.on_key(shift_tab());
    state.on_key(key(BareKey::Right));

    assert!(state.on_key(key(BareKey::Tab)));
    assert_eq!(state.chat_tab_focus, None, "the bar let the keyboard go");
    assert_eq!(state.chat_tab, 1, "and switched to no chat on the way");

    state.on_key(key(BareKey::Char('q')));
    assert_eq!(state.input.get_text(), "q", "the box has the keys, editing");
}

/// Backward off the bar is the agent list's history half - or, with no list, round to the box.
#[test]
fn shift_tab_at_the_bar_steps_back_to_the_agent_list() {
    let mut state = three_tabs();
    state.agents_plugin = Some(zellij_tile::prelude::PluginId::new(7));
    state.on_key(shift_tab());

    assert!(state.on_key(shift_tab()));
    assert_eq!(
        state.ring_focus_calls,
        [(
            PaneId::Plugin(zellij_tile::prelude::PluginId::new(7)),
            "history"
        )],
        "the keyboard went to the list, landing in its history half"
    );
    assert_eq!(state.chat_tab_focus, None, "and this half is off its stop");
}

/// A panel with one half is a ring with two stops, and it wraps between them rather than
/// growing the half it is missing: opening a list is what a reveal does, not a Tab.
#[test]
fn the_ring_wraps_within_the_chat_when_there_is_no_agent_list() {
    let mut state = three_tabs();
    assert_eq!(state.agents_plugin, None);
    let launches = state.agents_launches;

    assert!(state.on_key(key(BareKey::Tab)));
    assert_eq!(state.chat_tab_focus, Some(1), "forward wrapped to the bar");
    assert!(state.ring_focus_calls.is_empty());

    assert!(state.on_key(shift_tab()));
    assert_eq!(
        state.chat_tab_focus, None,
        "and backward wrapped to the box"
    );
    assert_eq!(state.agents_launches, launches, "no list was launched");
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

/// The bar and the box are two stops, not one: stepping up to the bar spends the box's release,
/// or the Esc coming back down would land on a box with no caret and take a second press to
/// type in.
#[test]
fn shift_tab_off_a_released_box_leaves_it_holding_the_keys_again() {
    let mut state = three_tabs();
    state.input.set_text("hello".to_owned());
    state.on_key(key(BareKey::Esc));
    assert!(state.chat_input_released, "the box let go");

    state.on_key(shift_tab());
    assert!(
        !state.chat_input_released,
        "and the bar took the stop with it"
    );

    state.on_key(key(BareKey::Esc));
    assert_eq!(state.chat_tab_focus, None, "back off the bar");
    state.on_key(key(BareKey::Char('!')));
    assert_eq!(state.input.get_text(), "hello!", "typing, first press");
    assert!(!state.hidden);
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
fn clicking_a_tab_switches_to_it() {
    let mut state = three_tabs();
    state.chat_tab_width = 40;
    state.chat_tab_focus = Some(1);
    let first = ui::chat_tabs(&state, state.chat_tab_width)
        .into_iter()
        .find(|cell| cell.target == ui::TabTarget::Tab(0))
        .unwrap();

    assert!(state.on_mouse(Mouse::LeftClick(0, first.x + 1)));
    assert_eq!(state.chat_id, Some(1));
    assert_eq!(state.chat_tab, 0);
    assert_eq!(state.chat_tab_focus, None, "the input has focus again");
}

#[test]
fn clicking_an_overflow_arrow_reveals_tabs_without_switching() {
    let mut state = three_tabs();
    state.chat_tab_width = 12;
    state.chat_tab_focus = Some(1);
    let right = ui::chat_tabs(&state, state.chat_tab_width)
        .into_iter()
        .find(|cell| cell.target == ui::TabTarget::Reveal(2))
        .unwrap();

    assert!(state.on_mouse(Mouse::LeftClick(0, right.x)));
    assert_eq!(state.chat_id, Some(2), "the active chat did not change");
    assert_eq!(state.chat_tab_view.get(&state.client), Some(&2));
    assert_eq!(
        state.chat_tab_focus, None,
        "the clicked view now takes effect"
    );
    assert!(ui::chat_tabs(&state, state.chat_tab_width)
        .iter()
        .any(|cell| cell.target == ui::TabTarget::Tab(2)));
}

#[test]
fn overflow_tab_reveal_is_per_client() {
    let mut state = three_tabs();
    state.chat_tab_width = 12;
    state.client = ClientId::new(1);
    let right = ui::chat_tabs(&state, state.chat_tab_width)
        .into_iter()
        .find(|cell| cell.target == ui::TabTarget::Reveal(2))
        .unwrap();

    assert!(state.on_mouse(Mouse::LeftClick(0, right.x)));
    assert!(ui::chat_tabs(&state, state.chat_tab_width)
        .iter()
        .any(|cell| cell.target == ui::TabTarget::Tab(2)));

    state.client = ClientId::new(2);
    assert!(ui::chat_tabs(&state, state.chat_tab_width)
        .iter()
        .any(|cell| cell.target == ui::TabTarget::Tab(1)));

    state.client = ClientId::new(1);
    assert!(ui::chat_tabs(&state, state.chat_tab_width)
        .iter()
        .any(|cell| cell.target == ui::TabTarget::Tab(2)));
}

#[test]
fn hidden_tab_bars_do_not_keep_their_old_click_targets() {
    let mut state = three_tabs();
    ui::render(&mut state, ui::MIN_ROWS, 40);
    let first = ui::chat_tabs(&state, state.chat_tab_width)
        .into_iter()
        .find(|cell| cell.target == ui::TabTarget::Tab(0))
        .unwrap();

    ui::render(&mut state, ui::MIN_ROWS - 1, 40);
    assert_ne!(
        state.chat_tab_width, 0,
        "a pane one row short of the whole layout still draws its bar"
    );

    // Three rows: the title bar is the third thing a short pane gives up, and a
    // bar nobody can see must not go on taking the clicks it would have taken.
    ui::render(&mut state, 3, 40);
    assert_eq!(state.chat_tab_width, 0);
    assert!(!state.on_mouse(Mouse::LeftClick(0, first.x + 1)));

    state.perms = false;
    state.chat_tab_width = 40;
    ui::render(&mut state, ui::MIN_ROWS, 40);
    assert_eq!(state.chat_tab_width, 0);
}

#[test]
fn clicks_outside_the_tab_bar_do_nothing() {
    let mut state = three_tabs();
    state.chat_tab_width = 20;

    assert!(!state.on_mouse(Mouse::LeftClick(1, 0)));
    assert!(!state.on_mouse(Mouse::LeftClick(0, 30)));
    assert_eq!(state.chat_id, Some(2));
}

#[test]
fn changing_tabs_resets_the_scrolled_view() {
    let mut state = three_tabs();
    state.chat_tab_view.insert(state.client, 2);

    state.switch_to_tab(0);
    assert!(!state.chat_tab_view.contains_key(&state.client));

    state.chat_tab_view.insert(state.client, 2);
    state.close_tab(2);
    assert!(!state.chat_tab_view.contains_key(&state.client));

    state.chat_tab_view.insert(state.client, 1);
    state.open_tab(Chat::default());
    assert!(!state.chat_tab_view.contains_key(&state.client));
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
    assert_eq!(state.messages.len(), 2, "read back whole");
    // The empty tab it was picked from is not worth keeping, so the loaded
    // chat takes its place on the end rather than leaving a blank one behind.
    assert_eq!(state.chat_tab_count(), 3, "no lingering empty tab");
    assert_eq!(state.chat_tab, 2, "on the end");
}

/// `/history` from a tab with something in it keeps that tab.
#[test]
fn opening_an_archived_chat_from_a_used_tab_keeps_the_tab() {
    let mut state = three_tabs();
    state.messages = vec![Message::user("was here"), Message::assistant("yes")];
    state.flush_save();
    let saved = state.chat_id.expect("the flush allocated an id");
    let index = state
        .parked
        .iter()
        .position(|chat| chat.chat_id == Some(saved));
    assert!(index.is_none(), "the saved one is the foreground");
    state.new_chat();
    state.parked.retain(|chat| chat.chat_id != Some(saved));
    state.chat_tab -= 1;
    state.input.set_text("half a thought".to_owned());
    state.messages = vec![Message::user("typed")];

    state.switch_to_chat(saved);
    assert_eq!(state.chat_tab_count(), 4, "the used tab is parked");
    assert_eq!(state.chat_tab, 3, "and the loaded one is on the end");
    assert_eq!(state.parked[2].draft.0, "half a thought", "with its draft");
    assert_eq!(state.input.get_text(), "", "the loaded chat has none");
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
        usage: None,
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
    state.flush_save();

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
        archive::fingerprint(
            &state.parked[0].messages,
            state.parked[0].reply_at,
            state.parked[0].usage,
            state.parked[0].plan,
        ),
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

// -- servicing another tab costs the foreground nothing ---------------------

/// A parked chat with a job is ticked on every timer, through the same swap
/// that services its stream. The person typing in the foreground must not
/// notice: the undo stack and the Up/Down history draft are the input box's,
/// and the box is not part of the swap.
#[test]
fn a_tick_for_another_tab_leaves_the_input_undo_and_history_alone() {
    let mut state = three_tabs();
    state.parked[0].jobs.push(Job {
        id: 1,
        every: 60.0,
        due_in: 60.0,
        prompt: "check".into(),
        show: false,
        auto_confirm: false,
    });
    state.input.set_history(vec!["earlier".to_owned()]);
    for ch in "draft".chars() {
        state.on_key(key(BareKey::Char(ch)));
    }
    assert!(state.input.can_undo());

    state.on_key(key(BareKey::Up));
    assert_eq!(state.input.get_text(), "earlier", "recalled");
    state.on_timer(1.0);
    state.on_key(key(BareKey::Down));
    assert_eq!(
        state.input.get_text(),
        "draft",
        "the draft survived the tick"
    );

    state.on_timer(1.0);
    assert!(state.input.can_undo(), "and so did the undo stack");
    state.on_key(KeyWithModifier::new(BareKey::Char('z')).with_ctrl_modifier());
    assert_ne!(state.input.get_text(), "draft", "Ctrl-z still undoes");
}

/// The render memo travels with its chat rather than being dropped, so a
/// frame with nothing new on screen re-wraps nothing.
#[test]
fn a_tick_for_another_tab_keeps_the_foreground_transcript_cache() {
    let mut state = three_tabs();
    state.parked[0]
        .asks
        .push_back(Ask::text("later", UserOrigin::Human));
    ui::render(&mut state, 24, 80);
    let cached = state
        .transcript_cache
        .as_ref()
        .map(ui::TranscriptCache::len);
    assert_eq!(cached, Some(state.messages.len()), "the frame filled it");

    state.on_timer(FRAME_SECONDS);
    assert_eq!(
        state
            .transcript_cache
            .as_ref()
            .map(ui::TranscriptCache::len),
        cached,
        "still cached"
    );
    assert!(state.parked[0].asks.is_empty(), "and the ask went out");
}

/// A chat with nothing but a queued question is still worth a tab: the asker
/// was told it had been taken.
#[test]
fn a_new_chat_keeps_a_tab_that_only_has_a_queued_question() {
    let mut state = idle_state();
    state.asks.push_back(Ask::text("later", UserOrigin::Human));
    state.new_chat();
    assert_eq!(state.chat_tab_count(), 2);
    assert_eq!(state.parked[0].asks.len(), 1);
}

// -- what closing writes ----------------------------------------------------

/// Closing a tab whose reply grew after its last save archives the reply as
/// it stands: the rewrite starts at the placeholder, not at the end.
#[test]
fn closing_a_parked_tab_mid_reply_archives_the_grown_reply() {
    let mut state = three_tabs();
    state.parked[1].stream = Some(StreamState {
        id: "hocus-9".into(),
        finishing: false,
        usage: None,
    });
    state.parked[1].messages.push(Message::assistant(""));
    state.parked[1].reply_at = Some(1);
    state.save_parked();
    state.parked[1].messages[1].content = "the whole reply".to_owned();

    state.close_tab(2);
    let archived = history::load(&state.private_store(), 3).expect("archived");
    assert_eq!(archived.len(), 2);
    assert_eq!(archived[1].content, "the whole reply");
}

/// A tab closed before its first token arrived archives no empty reply.
#[test]
fn closing_a_parked_tab_before_its_first_token_archives_no_empty_reply() {
    let mut state = three_tabs();
    state.parked[1].stream = Some(StreamState {
        id: "hocus-9".into(),
        finishing: false,
        usage: None,
    });
    state.parked[1].messages.push(Message::assistant(""));
    state.parked[1].reply_at = Some(1);

    state.close_tab(2);
    let archived = history::load(&state.private_store(), 3).expect("archived");
    assert_eq!(archived.len(), 1, "only the question");
}

/// Queued questions do not vanish with the tab they were queued at: they are
/// archived as notices, from either close path.
#[test]
fn closing_a_tab_archives_its_queued_questions_as_notices() {
    // A parked tab.
    let mut state = three_tabs();
    state.parked[1]
        .asks
        .push_back(Ask::text("one more thing", UserOrigin::Human));
    state.close_tab(2);
    let archived = history::load(&state.private_store(), 3).expect("archived");
    assert_eq!(
        archived.last().map(|m| m.content.as_str()),
        Some("unsent question: one more thing")
    );

    // The only tab, mid-reply.
    let mut state = ready_state();
    state.flush_save();
    let id = state.chat_id.expect("the initial transcript was archived");
    state
        .asks
        .push_back(Ask::text("and another", UserOrigin::Human));
    state.close_tab(0);
    assert!(state.messages.is_empty(), "reset in place");
    let archived = history::load(&state.private_store(), id).expect("archived");
    let contents: Vec<&str> = archived.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(contents, ["hi", "unsent question: and another"]);
}

// -- jobs across tabs -------------------------------------------------------

fn job_in_a_parked_tab(arguments: &str) -> State {
    let mut state = idle_state();
    state.run_local(&schedule_call(arguments));
    state.new_chat();
    assert!(state.jobs.is_empty());
    assert_eq!(state.parked[0].jobs.len(), 1);
    state
}

/// `/stop_jobs` stops every job in every tab, from whichever tab it is typed in.
#[test]
fn stop_jobs_reaches_a_job_in_another_tab() {
    let mut state = job_in_a_parked_tab(r#"{"seconds":60,"prompt":"check pane 3"}"#);
    assert!(state.stop_jobs());
    assert!(state.parked[0].jobs.is_empty());
    assert!(!state.any_jobs());
    assert_eq!(
        state.messages.last().map(|m| m.content.as_str()),
        Some("cancelled 1 scheduled job(s)")
    );
}

/// The model's `unschedule` finds a job by id wherever it lives.
#[test]
fn unschedule_by_id_reaches_a_job_in_another_tab() {
    let mut state = job_in_a_parked_tab(r#"{"seconds":60,"prompt":"check pane 3"}"#);
    assert_eq!(state.unschedule(Some(1)), "cancelled job 1");
    assert!(state.parked[0].jobs.is_empty());
    assert_eq!(state.unschedule(Some(1)), "no job 1");
}

/// A settled tab holding a job is working, as far as the bar is concerned.
#[test]
fn a_tab_with_a_job_counting_down_is_live() {
    let state = job_in_a_parked_tab(r#"{"seconds":60,"prompt":"check pane 3"}"#);
    assert!(state.parked[0].is_live());
    assert_eq!(state.parked_count(), 1);
}

/// A job that asked to be seen and fires off-screen brings its tab to the
/// front first, so the prompt — and any confirmation the run raises — is
/// where the keyboard is.
#[test]
fn a_showing_job_that_fires_in_a_parked_tab_switches_to_it() {
    let mut state = job_in_a_parked_tab(r#"{"seconds":60,"prompt":"check pane 3","show":true}"#);
    assert!(state.on_timer(60.0));
    assert_eq!(state.chat_tab, 0, "the job's tab is on screen");
    assert!(state
        .messages
        .iter()
        .any(|m| m.content == "[scheduled] check pane 3"));
    assert!(state.is_streaming(), "and its run is the foreground's");
    assert!(
        state.parked[0].messages.is_empty(),
        "the new chat was parked"
    );
}

/// One that did not ask stays where it was.
#[test]
fn a_quiet_job_that_fires_in_a_parked_tab_stays_there() {
    let mut state = job_in_a_parked_tab(r#"{"seconds":60,"prompt":"check pane 3"}"#);
    assert!(state.on_timer(60.0));
    assert_eq!(state.chat_tab, 1, "still on the new chat");
    assert!(state.messages.is_empty());
    assert!(state.parked[0].stream.is_some());
}

// -- hiding -----------------------------------------------------------------

/// The bar's highlight would otherwise swallow every key on the next reveal.
#[test]
fn hiding_the_panel_gives_the_keyboard_back_from_the_bar() {
    let mut state = three_tabs();
    state.on_key(shift_tab());
    assert!(state.chat_tab_focus.is_some());
    state.hide_panel();
    assert_eq!(state.chat_tab_focus, None);
}
