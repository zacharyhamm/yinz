use super::*;

#[test]
fn closing_a_stale_chat_preserves_both_archived_transcripts() {
    let mut state = idle_state();
    state.messages = vec![
        Message::user("question"),
        Message::assistant("initial answer"),
    ];
    state.flush_save();
    let id = state.chat_id.unwrap();
    let revision = state.chat_revision;
    state.reset_chat();
    state.switch_to_chat(id);
    assert_eq!(
        state.chat_revision, revision,
        "reopening carries the author's revision"
    );

    // Another server opens the same chat and wins the next write.
    let winner = vec![
        Message::user("question"),
        Message::assistant("new answer"),
        Message::user("follow-up"),
    ];
    let mut other_id = Some(id);
    let mut other_revision = revision;
    history::save(
        &state.private_store(),
        &state.host_id,
        &mut other_id,
        &mut other_revision,
        &winner,
        0,
        None,
        None,
        Plan::Off,
    )
    .unwrap();

    // The stale writer shortens the transcript. Its DELETE must roll back along
    // with the conflicting update, and closing must save this version separately.
    let losing = vec![Message::user("my changed question")];
    state.messages = losing.clone();
    state.close_tab(0);
    assert!(state.messages.is_empty());
    assert_eq!(state.chat_revision, None);
    assert_eq!(history::load(&state.private_store(), id).unwrap(), winner);
    let entries = history::list(&state.private_store());
    assert_eq!(entries.len(), 2);
    let fork = entries.iter().find(|entry| entry.id != id).unwrap();
    assert_eq!(
        history::load(&state.private_store(), fork.id).unwrap(),
        losing
    );
}

#[test]
fn a_parked_chat_keeps_its_revision_and_forked_jobs_follow_its_new_id() {
    use hocus_proto::store::Backend;
    let mut state = idle_state();
    state.messages = vec![Message::user("question")];
    state.flush_save();
    let original = state.chat_id.unwrap();
    let revision = state.chat_revision;
    state.new_chat();
    state.switch_to_chat(original);
    assert_eq!(
        state.chat_revision, revision,
        "parking preserves the author revision"
    );
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));
    state.flush_save();
    let mut other_id = state.chat_id;
    let mut other_revision = state.chat_revision;
    history::save(
        &state.private_store(),
        &state.host_id,
        &mut other_id,
        &mut other_revision,
        &[Message::user("other writer")],
        0,
        None,
        None,
        Plan::Off,
    )
    .unwrap();
    state.messages.push(Message::assistant("local answer"));
    state.flush_save();
    assert_ne!(state.chat_id, Some(original));
    let rows = state
        .store()
        .fetch(&hocus_proto::jobs::owned_request(&state.owner()))
        .unwrap();
    let jobs = hocus_proto::jobs::from_rows(&rows);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].chat_id, state.chat_id);
    assert_eq!(
        history::load(&state.private_store(), original).unwrap(),
        vec![Message::user("other writer")]
    );
}

/// The point of the archive: starting over must not destroy what came
/// before. Ctrl-l detaches from the file rather than deleting it.
#[test]
fn a_new_chat_detaches_from_the_archived_file_rather_than_deleting_it() {
    let mut state = ready_state();
    settle(&mut state);
    state.chat_id = Some(7);
    state.new_chat();

    assert_eq!(state.chat_id, None, "the next save opens a new file");
    assert_eq!(
        state.saved,
        (0, 0, None, Plan::Off),
        "and is not skipped as unchanged"
    );
}

/// The symptom this all started from: Enter on a slash command mid-reply did
/// nothing at all, not even redraw.
#[test]
fn the_archive_opens_over_an_arriving_reply() {
    let mut state = ready_state();
    state.input.set_text("/history".to_owned());

    assert!(state.submit(), "handled, so the pane repaints");
    assert!(state.input.get_text().is_empty(), "and the line was taken");
    assert!(matches!(
        state.picker.as_ref().map(|picker| &picker.kind),
        Some(PickerKind::Chats(_))
    ));
    assert!(
        state.is_receiving(),
        "with the reply still arriving behind it"
    );
}

/// The fingerprint is what tells the transcript cache, the archive and every
/// other client that something changed. It watched the *last* message, which
/// a tool typed mid-reply is no longer — the reply would grow invisibly.
#[test]
fn the_fingerprint_follows_the_reply_and_not_the_last_message() {
    let mut state = ready_state();
    state.input.set_text("/list_session".to_owned());
    state.submit();
    assert!(
        !state.reply_is_last(),
        "the result is behind the placeholder"
    );

    let before = state.fingerprint();
    state.on_chunk("hocus-1", &delta_frame("more"), false, None);
    assert_ne!(state.fingerprint(), before, "and the reply still shows up");
}

// -- background chats ---------------------------------------------------

/// A reply for a chat that is not on screen lands in that chat, and the one
/// you are looking at is untouched.
#[test]
fn a_chunk_for_a_parked_chat_lands_in_it() {
    let mut state = ready_state();
    state.parked.push(Chat {
        messages: vec![Message::user("older"), Message::assistant("")],
        reply_at: Some(1),
        chat_id: Some(4),
        stream: Some(StreamState {
            id: "hocus-7".into(),
            finishing: false,
            usage: None,
        }),
        ..Chat::default()
    });

    state.handle(Event::WebRequestStreamChunk {
        stream_id: "hocus-7".into(),
        chunk: delta_frame("background"),
        done: false,
        error: None,
    });

    assert_eq!(state.parked[0].messages[1].content, "background");
    assert_eq!(
        state.messages[1].content, "",
        "the visible chat is untouched"
    );
}

/// A background chat that settles stays open: it is a tab in the bar until it
/// is closed, and its reply is there waiting when you switch to it.
#[test]
fn a_parked_chat_that_settles_stays_open() {
    let mut state = ready_state();
    state.parked.push(Chat {
        messages: vec![Message::user("older"), Message::assistant("done")],
        reply_at: Some(1),
        chat_id: Some(4),
        stream: Some(StreamState {
            id: "hocus-7".into(),
            finishing: false,
            usage: None,
        }),
        ..Chat::default()
    });

    state.handle(Event::WebRequestStreamChunk {
        stream_id: "hocus-7".into(),
        chunk: Vec::new(),
        done: true,
        error: None,
    });

    assert_eq!(state.parked.len(), 1, "settled, and still open");
    assert!(!state.parked[0].is_live(), "with nothing left owed to it");
    assert_eq!(state.parked[0].messages[1].content, "done");
    assert!(state.is_receiving(), "and ours is still going");
}

/// Switching to a chat that is still working picks it back up where it was —
/// its transcript, its stream and its place in the queue — and the one being
/// left is parked rather than dropped.
#[test]
fn switching_to_a_parked_chat_swaps_the_two() {
    let mut state = ready_state();
    state.chat_id = Some(1);
    state.parked.push(Chat {
        messages: vec![Message::user("older question")],
        chat_id: Some(4),
        stream: Some(StreamState {
            id: "hocus-7".into(),
            finishing: false,
            usage: None,
        }),
        ..Chat::default()
    });

    state.switch_to_chat(4);
    assert_eq!(state.chat_id, Some(4));
    assert_eq!(state.messages.len(), 1, "its transcript, not the other's");
    assert_eq!(state.active_stream_id().as_deref(), Some("hocus-7"));
    assert_eq!(state.parked.len(), 1, "and the one we left is held");
    assert_eq!(state.parked[0].chat_id, Some(1));

    state.switch_to_chat(1);
    assert_eq!(state.messages.len(), 2, "back to where we started");
    assert_eq!(state.active_stream_id().as_deref(), Some("hocus-1"));
}

/// Leaving a settled chat parks it like any other: the tab stays, and coming
/// back finds the same transcript rather than a reload from the archive.
#[test]
fn a_settled_chat_is_parked_when_you_leave_it() {
    let mut state = ready_state();
    settle(&mut state);
    state.chat_id = Some(1);
    state.parked.push(Chat {
        messages: vec![Message::user("older")],
        chat_id: Some(4),
        ..Chat::default()
    });

    state.switch_to_chat(4);
    assert_eq!(state.chat_id, Some(4));
    assert_eq!(state.parked.len(), 1, "settled, and still held");
    assert_eq!(state.parked[0].chat_id, Some(1));
}

/// The gate that keeps a stream from writing the file once per socket read.
#[test]
fn autosave_holds_off_until_the_reply_has_landed() {
    let mut state = ready_state();
    state.messages = vec![Message::user("hi"), Message::assistant("partial")];
    state.autosave();
    assert_eq!(state.saved, (0, 0, None, Plan::Off), "still streaming");

    settle(&mut state);
    state.autosave();
    assert_eq!(state.saved, state.fingerprint(), "settled, so recorded");

    let recorded = state.saved;
    state.autosave();
    assert_eq!(
        state.saved, recorded,
        "and an unchanged transcript is a no-op"
    );
}

#[test]
fn an_untouched_pane_leaves_nothing_behind() {
    let mut state = State::default();
    state.autosave();
    assert_eq!(state.chat_id, None, "no file was ever opened");
}
