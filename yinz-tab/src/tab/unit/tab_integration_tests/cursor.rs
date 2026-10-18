//! What a frame tells the host terminal about the cursor.
//!
//! The rule these pin down is that a frame says only what changed. Frames used to arrive when the
//! user did something, so re-sending the whole cursor state on each one cost nothing; they now
//! arrive on whatever cadence the busiest animation on screen wants - the status bar's agent
//! spinner repaints eight times a second - and a cursor told to show itself and re-adopt its shape
//! at that rate never finishes a blink.

use super::*;

/// Paint a frame and hand back what the client would have been sent.
fn frame(tab: &mut Tab, client_id: ClientId) -> String {
    render(tab, client_id, Output::default())
}

/// The same, for a client whose host presents each frame atomically (DECSET 2026) and so is not
/// sent a cursor hide to begin with.
fn atomic_frame(tab: &mut Tab, client_id: ClientId) -> String {
    let synchronized_output_capabilities = Rc::new(RefCell::new(HashMap::new()));
    synchronized_output_capabilities
        .borrow_mut()
        .insert(client_id, true);
    let output = Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(None)),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        synchronized_output_capabilities,
    );
    render(tab, client_id, output)
}

/// The frame the watchers get: built from the followed client's panes, carrying that client's id,
/// and delivered to somebody else's terminal entirely.
fn watcher_frame(tab: &mut Tab, followed_client_id: ClientId) -> String {
    let mut output = Output::default();
    tab.render(&mut output, Some(followed_client_id)).unwrap();
    output
        .serialize()
        .unwrap()
        .get(&followed_client_id)
        .cloned()
        .unwrap_or_default()
}

fn render(tab: &mut Tab, client_id: ClientId, mut output: Output) -> String {
    tab.render(&mut output, None).unwrap();
    output
        .serialize()
        .unwrap()
        .get(&client_id)
        .cloned()
        .unwrap_or_default()
}

/// Make the pane paint something without moving its cursor, the way an animating plugin elsewhere
/// on screen makes a frame happen.
fn repaint(tab: &mut Tab) {
    tab.set_force_render();
}

const SHOW_CURSOR: &str = "\u{1b}[?25h";
const HIDE_CURSOR: &str = "\u{1b}[?25l";
/// DECSCUSR. Every shape the terminal pane can ask for ends in ` q`.
const SHAPE_SUFFIX: &str = " q";

#[test]
fn a_repeated_frame_does_not_re_send_the_cursor_shape() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();

    // The first frame has to say everything: the host has been told nothing yet.
    let first = frame(&mut tab, client_id);
    assert!(
        first.contains(SHOW_CURSOR) && first.contains(SHAPE_SUFFIX),
        "the first frame places the cursor from scratch, got: {first:?}"
    );

    // A second frame that repaints the same cells with the cursor in the same place still has to
    // put the cursor back - the chunks moved it - but must not restate the rest.
    repaint(&mut tab);
    let second = frame(&mut tab, client_id);
    assert!(
        second.contains("\u{1b}[6;6H"),
        "the cursor still has to be put back after the chunks, got: {second:?}"
    );
    assert!(
        !second.contains(SHAPE_SUFFIX),
        "re-sending the shape restarts the host's blink phase, got: {second:?}"
    );
}

/// On a host that shows the whole frame at once there is no hide to undo either, so a repeated
/// frame carries nothing about the cursor but the goto that puts it back after the chunks.
#[test]
fn a_repeated_frame_on_an_atomic_host_only_puts_the_cursor_back() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();
    let first = atomic_frame(&mut tab, client_id);
    assert!(
        !first.contains(HIDE_CURSOR),
        "an atomic host is never sent the pre-frame hide, got: {first:?}"
    );

    repaint(&mut tab);
    let second = atomic_frame(&mut tab, client_id);
    assert!(
        second.contains("\u{1b}[6;6H"),
        "the chunks moved the cursor and it has to go back, got: {second:?}"
    );
    assert!(
        !second.contains(HIDE_CURSOR) && !second.contains(SHOW_CURSOR),
        "nothing hid the cursor, so nothing has to show it again, got: {second:?}"
    );
    assert!(
        !second.contains(SHAPE_SUFFIX),
        "the shape did not change, got: {second:?}"
    );
}

#[test]
fn a_changed_shape_is_sent_again() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();
    let _ = frame(&mut tab, client_id);

    // A blinking bar, DECSCUSR 5.
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5 q".as_bytes()),
    )
    .unwrap();
    let after = frame(&mut tab, client_id);
    assert!(
        after.contains("\u{1b}[5 q"),
        "a shape the host has not been told about must be sent, got: {after:?}"
    );
}

#[test]
fn a_moved_cursor_is_followed() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();
    let _ = frame(&mut tab, client_id);

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[10;20H".as_bytes()),
    )
    .unwrap();
    let after = frame(&mut tab, client_id);
    // the pane sits inside its frame, so its (10, 20) is the screen's (11, 21)
    assert!(
        after.contains("\u{1b}[11;21H"),
        "the new position must be sent, got: {after:?}"
    );
    assert!(
        !after.contains(SHAPE_SUFFIX),
        "moving is not a reason to restate the shape, got: {after:?}"
    );
}

#[test]
fn a_cursor_the_app_hid_is_shown_again_when_it_comes_back() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H\u{1b}[?25l".as_bytes()),
    )
    .unwrap();
    let hidden = frame(&mut tab, client_id);
    assert!(
        !hidden.contains(SHOW_CURSOR),
        "the app hid it, got: {hidden:?}"
    );

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[?25h".as_bytes()),
    )
    .unwrap();
    let shown = frame(&mut tab, client_id);
    assert!(
        shown.contains(SHOW_CURSOR),
        "the host was last told the cursor was hidden and has to be told otherwise, got: {shown:?}"
    );
}

#[test]
fn a_tab_coming_into_view_places_the_cursor_from_scratch() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();
    let _ = frame(&mut tab, client_id);

    // The cursor is one host-wide resource and the memo of what it was last told is per tab, so a
    // tab that was away has no idea what the tab it replaced left behind.
    tab.visible(false).unwrap();
    tab.visible(true).unwrap();
    let after = frame(&mut tab, client_id);
    assert!(
        after.contains(SHOW_CURSOR) && after.contains(SHAPE_SUFFIX),
        "everything must be restated, got: {after:?}"
    );
}

/// Client ids are recycled, and a client joining a tab that already has clients does not go
/// through `visible(true)` - so without dropping the memo on the way out, the arriving terminal
/// inherits what the previous holder of its id was told and is never sent a `?25h` or a shape.
#[test]
fn a_client_rejoining_a_populated_tab_is_told_the_whole_cursor_again() {
    let size = Size { cols: 80, rows: 24 };
    let leaving_client_id = ClientId::new(1);
    let staying_client_id = ClientId::new(2);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.add_client(staying_client_id, None).unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();

    // Whatever the departing client was told, the tab now remembers.
    let _ = frame(&mut tab, leaving_client_id);

    tab.remove_client(leaving_client_id);
    tab.add_client(leaving_client_id, None).unwrap();

    let rejoined = frame(&mut tab, leaving_client_id);
    assert!(
        rejoined.contains(SHOW_CURSOR) && rejoined.contains(SHAPE_SUFFIX),
        "a fresh terminal has been told nothing about its cursor, got: {rejoined:?}"
    );
}

/// The watchers' frame is a second byte stream going to different terminals, and it is named after
/// the client it follows. Sharing one memo between the two let each pass conclude "nothing
/// changed" from what the other one had said - and since the passes disagree about occlusion (the
/// session's toasts are drawn into the regular clients' frame only), that left a real client's
/// cursor hidden until something else dropped the memo.
#[test]
fn the_watchers_frame_and_the_followed_clients_frame_keep_their_own_cursor_state() {
    let size = Size { cols: 80, rows: 24 };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();

    // The followed client's own terminal now knows where its cursor is and what shape it has.
    let first = frame(&mut tab, client_id);
    assert!(
        first.contains(SHOW_CURSOR) && first.contains(SHAPE_SUFFIX),
        "the first frame places the cursor from scratch, got: {first:?}"
    );

    // The watchers' terminals have been told none of that.
    let watched = watcher_frame(&mut tab, client_id);
    assert!(
        watched.contains(SHOW_CURSOR) && watched.contains(SHAPE_SUFFIX),
        "a watcher has been told nothing about this cursor yet, got: {watched:?}"
    );

    // ...and saying it to them must not count as having said it to the client.
    repaint(&mut tab);
    let second = frame(&mut tab, client_id);
    assert!(
        second.contains("\u{1b}[6;6H"),
        "the cursor still has to be put back after the chunks, got: {second:?}"
    );
    assert!(
        !second.contains(SHAPE_SUFFIX),
        "the client's own memo is intact and the shape did not change, got: {second:?}"
    );
}

/// The override names one client and means it: the frame is going to the watchers' terminals, so
/// nobody else's cursor may be placed in it - and, more to the point, nobody else's memo may be
/// written for bytes their terminal never receives. (The panes themselves still draw for every
/// connected client; Screen throws away every stream but the followed one.)
#[test]
fn the_watchers_frame_places_only_the_followed_clients_cursor() {
    let size = Size { cols: 80, rows: 24 };
    let followed_client_id = ClientId::new(1);
    let other_client_id = ClientId::new(2);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.add_client(other_client_id, None).unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[5;5H".as_bytes()),
    )
    .unwrap();

    let mut output = Output::default();
    tab.render(&mut output, Some(followed_client_id)).unwrap();
    let theirs_in_the_watchers_frame = output
        .serialize()
        .unwrap()
        .get(&other_client_id)
        .cloned()
        .unwrap_or_default();
    assert!(
        !theirs_in_the_watchers_frame.contains(SHOW_CURSOR)
            && !theirs_in_the_watchers_frame.contains(SHAPE_SUFFIX),
        "a client who is not being watched gets no cursor in this frame, got: \
         {theirs_in_the_watchers_frame:?}"
    );

    // And having been left out of it, that client's own next frame still states everything.
    let theirs = frame(&mut tab, other_client_id);
    assert!(
        theirs.contains(SHOW_CURSOR) && theirs.contains(SHAPE_SUFFIX),
        "this terminal has been told nothing about its cursor, got: {theirs:?}"
    );
}

#[test]
fn an_atomic_app_hidden_cursor_settles_and_still_moves_for_ime() {
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, ModeInfo::default());
    tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[5;5H\x1b[?25l".to_vec())
        .unwrap();
    let first = atomic_frame(&mut tab, client_id);
    assert_eq!(first.matches(HIDE_CURSOR).count(), 1);
    assert!(first.contains("\x1b[6;6H"));
    assert_eq!(atomic_frame(&mut tab, client_id), "");
    tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[7;8H".to_vec())
        .unwrap();
    let moved = atomic_frame(&mut tab, client_id);
    assert!(moved.contains("\x1b[8;9H"));
    assert!(!moved.contains(HIDE_CURSOR));
    assert_eq!(atomic_frame(&mut tab, client_id), "");
    tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[?25h".to_vec())
        .unwrap();
    let restored = atomic_frame(&mut tab, client_id);
    assert!(restored.contains(SHOW_CURSOR) && restored.contains(SHAPE_SUFFIX));
}

#[test]
fn hidden_cursor_memos_settle_and_restore_all_state() {
    use yinz_terminal::ui::damage::CellRect;

    let client_id = ClientId::new(1);
    for reason in ["occluded", "missing", "mid-frame"] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, ModeInfo::default());
        tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[5;5H\x1b[5 q".to_vec())
            .unwrap();
        let _ = atomic_frame(&mut tab, client_id);
        if reason == "mid-frame" {
            tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[?2026h".to_vec())
                .unwrap();
        }
        let active_panes = tab.tiled_panes.active_panes();
        if reason == "missing" {
            tab.tiled_panes.clear_active_panes();
        }
        // Only the cursor pass: no pane output can accidentally supply the restoring goto.
        for iteration in 0..2 {
            let mut output = Output::default();
            output.set_host_presents_frames_atomically(true);
            output.add_clients(
                &[client_id].into_iter().collect(),
                tab.link_handler.clone(),
                None,
            );
            if reason == "occluded" {
                output.set_notification_rects(
                    [(
                        client_id,
                        vec![CellRect {
                            x: 0,
                            y: 0,
                            cols: 80,
                            rows: 24,
                        }],
                    )]
                    .into_iter()
                    .collect(),
                );
            }
            tab.render_cursor_last(&mut output, None);
            let bytes = output
                .serialize()
                .unwrap()
                .remove(&client_id)
                .unwrap_or_default();
            assert_eq!(
                bytes,
                if iteration == 0 { HIDE_CURSOR } else { "" },
                "{reason}"
            );
        }
        if reason == "missing" {
            tab.tiled_panes.set_active_panes(active_panes);
        }
        if reason == "mid-frame" {
            tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[?2026l".to_vec())
                .unwrap();
        }
        let mut output = Output::default();
        output.set_host_presents_frames_atomically(true);
        output.add_clients(
            &[client_id].into_iter().collect(),
            tab.link_handler.clone(),
            None,
        );
        tab.render_cursor_last(&mut output, None);
        let restored = output.serialize().unwrap().remove(&client_id).unwrap();
        assert!(
            restored.contains(SHOW_CURSOR)
                && restored.contains("\x1b[6;6H")
                && restored.contains("\x1b[5 q"),
            "{reason}: {restored:?}"
        );
    }
}
