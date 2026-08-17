//! Animation.
//!
//! Spec, "Animation": an image can carry additional frames, composed and played by the
//! terminal, using three actions and one delete specifier:
//!
//! * `a=f` transmits frame data. `c` names the frame to use as the base canvas, `r` the frame
//!   being edited (absent means "make a new frame"), `x`/`y`/`s`/`v` the rectangle the data
//!   covers, `X` the composition mode, `Y` the background colour and `z` the gap in
//!   milliseconds. New frames default to a 40ms gap; the root frame's default is 0.
//! * `a=a` controls playback: `s=1/2/3` to stop, run, or run and loop, `v` for the loop count,
//!   `c` to make a frame current, and `r` with `z` to change one frame's gap.
//! * `a=c` composes one frame onto another: `r` source, `c` destination, `w`/`h` the size, `x`/`y`
//!   the destination offset and `X`/`Y` the source offset — note that the polarity is the
//!   opposite of frame loading.
//! * `d=f` deletes frames of an image; `d=F` deletes the image outright.
//!
//! These tests are written against externally observable behaviour only — reply codes, the
//! reported frame number, and whether an image can still be displayed afterwards. Frames are real
//! rasters, held as [`KittyFrame`](crate::panes::kitty_graphics::KittyFrame) alongside the root
//! image, so `a=a,c=<n>` changes what is drawn.
//!
//! What this terminal deliberately does not have is a playback *clock*: `s=2`/`s=3` and the `z`
//! gaps are validated and recorded, but nothing advances a frame on its own. Client-driven
//! animation works; terminal-driven animation is inert. See the judgement calls in
//! `kitty_graphics/mod.rs`.

use super::*;

/// A 4x3 RGB frame, kitty's fixture size for the animation tests.
fn frame() -> Vec<u8> {
    rgb_raster(4, 3)
}

/// A session with a 4x3 root image transmitted as image 1.
fn session_with_a_root_image() -> KittySession {
    let mut session = KittySession::new(10, 20);
    session.ok("a=t,f=24,s=4,v=3,i=1", &frame());
    session
}

#[test]
fn anim_transmitting_a_frame_for_an_unknown_image_reports_enoent() {
    // A frame has nowhere to live without its image, and the client needs to know that rather
    // than being told the whole feature is missing.
    let mut session = KittySession::new(10, 20);
    let reply = session.err("a=f,f=24,s=4,v=3,i=7", &frame());
    assert_eq!(reply.code, "ENOENT");
    assert_eq!(reply.image_id, Some(7));
}

#[test]
fn anim_the_first_added_frame_is_reported_as_frame_two() {
    // Spec: frame numbers are 1-based and frame 1 is the root image, so the first frame a client
    // adds is frame 2. The reply carries it in the `r` key.
    let mut session = session_with_a_root_image();
    let reply = session.reply("a=f,f=24,s=4,v=3,i=1", &frame());
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert_eq!(reply.frame_number, Some(2));
}

#[test]
fn anim_each_new_frame_gets_the_next_number() {
    let mut session = session_with_a_root_image();
    assert_eq!(
        session.reply("a=f,f=24,s=4,v=3,i=1", &frame()).frame_number,
        Some(2)
    );
    assert_eq!(
        session.reply("a=f,f=24,s=4,v=3,i=1", &frame()).frame_number,
        Some(3)
    );
}

#[test]
fn anim_a_frame_can_be_transmitted_in_chunks() {
    // Spec: frame data chunks like image data, except that "when sending animation frame data,
    // subsequent chunks must also specify the a=f key". What gets split is the base64 stream, on a
    // four-character boundary — "all chunks, except the last, must have a size that is a multiple
    // of 4".
    let mut session = session_with_a_root_image();
    let encoded = BASE64_ENCODER.encode(frame());
    let (first, second) = encoded.split_at(encoded.len() / 2 / 4 * 4);

    let first_chunk_replies = session.send_raw("a=f,f=24,s=4,v=3,i=1,m=1", first);
    assert!(
        first_chunk_replies.is_empty(),
        "a chunk with more to come must not be answered, got {:?}",
        first_chunk_replies
    );
    let replies = session.send_raw("a=f,m=0", second);
    let reply = replies
        .into_iter()
        .next()
        .expect("the final chunk is answered");
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert_eq!(reply.frame_number, Some(2));
}

#[test]
fn anim_a_frame_may_cover_only_part_of_the_image() {
    // Spec: x, y, s and v give "the rectangle of the frame that this data covers", which is how
    // an animation sends only the pixels that changed.
    let mut session = session_with_a_root_image();
    let reply = session.reply("a=f,f=24,s=2,v=2,x=1,y=1,i=1", &rgb_raster(2, 2));
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_an_existing_frame_can_be_edited_in_place() {
    // Spec: `r` names the frame being edited rather than creating a new one.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=f,f=24,s=4,v=3,i=1,r=2", &frame());
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert_eq!(
        reply.frame_number,
        Some(2),
        "editing frame 2 must not create a frame 3"
    );
}

#[test]
fn anim_a_new_frame_can_be_based_on_an_existing_one() {
    // Spec: `c` is "the 1-based frame number of the frame whose data should be used as the base
    // canvas", so a client can send a delta against any earlier frame.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=f,f=24,s=2,v=2,x=1,y=1,c=2,i=1", &rgb_raster(2, 2));
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert_eq!(reply.frame_number, Some(3));
}

#[test]
fn anim_editing_a_frame_that_does_not_exist_reports_enoent() {
    let mut session = session_with_a_root_image();
    let reply = session.err("a=f,f=24,s=4,v=3,i=1,r=9", &frame());
    assert_eq!(reply.code, "ENOENT");
}

#[test]
fn anim_playback_can_be_stopped_and_started() {
    // Spec, `a=a` with `s`: 1 stops, 2 runs while waiting for more frames, 3 runs and loops.
    // These are control commands with no data, so success is the whole observable contract.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    for state in [1, 2, 3] {
        let reply = session.reply(&format!("a=a,i=1,s={}", state), &[]);
        assert!(
            reply.is_ok(),
            "s={} rejected: {}:{}",
            state,
            reply.code,
            reply.message
        );
    }
}

#[test]
fn anim_a_loop_count_can_be_set() {
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());
    let reply = session.reply("a=a,i=1,s=3,v=3", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_a_frames_gap_can_be_changed_after_the_fact() {
    // Spec, `a=a` with `r` and `z`: set the gap of frame `r` to `z` milliseconds.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=a,i=1,r=2,z=77", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_a_frame_can_be_made_current() {
    // Spec, `a=a` with `c`: "the 1-based frame number of the frame that should be made current".
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=a,i=1,c=2", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_composing_one_frame_onto_another_is_accepted() {
    // Spec, `a=c`: blit a rectangle from frame `r` onto frame `c`.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=c,i=1,r=1,c=2", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_composing_a_sub_rectangle_is_accepted() {
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=c,i=1,r=1,c=2,w=2,h=2,x=1,y=1", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn anim_composing_from_a_frame_that_does_not_exist_reports_enoent() {
    let mut session = session_with_a_root_image();
    let reply = session.err("a=c,i=1,r=9,c=1", &[]);
    assert_eq!(reply.code, "ENOENT");
}

#[test]
fn anim_composing_out_of_bounds_is_rejected_rather_than_overflowing() {
    // kitty regression: offsets near u32::MAX must be reported as EINVAL, not wrapped around
    // into a rectangle that reads outside the frame buffer.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    for key in ["x", "y", "X", "Y"] {
        let reply = session.err(&format!("a=c,i=1,r=1,c=2,w=2,h=2,{}=4294967295", key), &[]);
        assert_eq!(reply.code, "EINVAL", "{} was not bounds-checked", key);
    }
}

#[test]
fn anim_deleting_a_frame_leaves_the_image_displayable() {
    // Spec, `d=f`: delete the frames of an image, optionally just the one named by `r`. The
    // image itself survives — it still has its root frame.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    let reply = session.reply("a=d,d=f,i=1,r=2", &[]);
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert!(session.can_still_display_by_placing_it(1));
}

#[test]
fn anim_deleting_with_the_uppercase_specifier_removes_the_image() {
    // Spec: uppercase frees the image data, as everywhere else in the delete matrix.
    let mut session = session_with_a_root_image();
    session.reply("a=f,f=24,s=4,v=3,i=1", &frame());

    session.reply("a=d,d=F,i=1", &[]);
    let reply = session.err("a=p,i=1", &[]);
    assert_eq!(reply.code, "ENOENT");
}
