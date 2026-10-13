//! Whole-app graphics conformance: what an application running inside yinz observes, and what
//! the terminal hosting yinz is told.
//!
//! Derived from `graphics-protocol.rst` ("Querying support and available transmission mediums",
//! "Suppressing responses", "Transferring larger amounts of data", "The transmission medium",
//! "Interaction with other terminal actions").
//!
//! The unit levels can assert on internal state; what only this level can show is that the
//! replies actually reach the application's stdin and that the re-encoded commands actually reach
//! the client. Every test here passes; the deviations that are still outstanding are all at the
//! unit level, indexed in the module doc of `yinz-terminal-graphics/src/kitty_graphics/mod.rs`.
#![cfg(unix)]

use yinz_integration_tests::kitty_protocol::{
    apc_b64, base64_encode, contains_bytes, contains_graphics_apc, parse_replies, rgb_block,
    KittyReply,
};
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, start_yinz, FakePtyHandle, TestSession,
};

const KITTY_PROBE_ACK: &[u8] = b"\x1b_Gi=31;OK\x1b\\";

/// Complete the capability handshake so the pane believes the host terminal can show images.
fn setup_kitty_host(yinz: &TestSession, terminal: &FakePtyHandle) {
    terminal.disable_echo();
    yinz.send_stdin(KITTY_PROBE_ACK);
    // Pixel updates follow capability replies through the same server and screen queues.
    yinz.send_stdin(b"\x1b[6;21;8t");
    terminal.wait_for_cell_size(8, 21);
    yinz_integration_tests::kitty_protocol::wait_for_kitty_host(terminal);
}

/// The replies yinz has written back to the application, once one matching `predicate` arrives.
fn wait_for_reply(
    terminal: &FakePtyHandle,
    description: &str,
    predicate: impl Fn(&KittyReply) -> bool,
) -> KittyReply {
    let stdin = terminal.wait_for_stdin(description, |stdin_bytes| {
        parse_replies(stdin_bytes).iter().any(&predicate)
    });
    parse_replies(&stdin)
        .into_iter()
        .find(predicate)
        .expect("the reply we just waited for")
}

/// Drive the pane past a point in the stream, so a later assertion that something did *not*
/// happen has a barrier to stand on.
fn barrier(yinz: &mut TestSession, terminal: &FakePtyHandle, marker: &str) {
    terminal.output(marker.as_bytes());
    yinz.wait_until("the barrier text is rendered", |grid| grid.contains(marker));
}

#[test]
fn a_support_query_is_answered_to_the_application() {
    // Spec: a client detects support by transmitting a one-pixel image with `a=q` and looking
    // for a graphics response. Without this answer, every graphics client concludes yinz has
    // no image support at all.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    terminal.output(&apc_b64("a=q,i=77,s=1,v=1,t=d,f=24", &rgb_block(1, 1)));

    let reply = wait_for_reply(&terminal, "the query is answered", |reply| {
        reply.image_id == Some(77)
    });
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);

    yinz.quit();
}

#[test]
fn a_failed_transmission_is_reported_to_the_application() {
    // The application has to be able to tell "this terminal cannot do graphics" from "this
    // particular image was wrong", which is the whole point of the error codes.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    // Ten by ten pixels declared, one pixel sent.
    terminal.output(&apc_b64("a=T,f=24,s=10,v=10,i=78", &rgb_block(1, 1)));

    let reply = wait_for_reply(&terminal, "the short image is reported", |reply| {
        reply.image_id == Some(78)
    });
    assert_eq!(reply.code, "ENODATA");

    yinz.quit();
}

#[test]
fn a_quiet_transmission_is_not_answered() {
    // Spec, `q=2`: suppress both success and error responses. A client that sets it and then
    // reads its stdin must not find graphics replies mixed into the user's keystrokes.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);
    let before = terminal.stdin_bytes().len();

    terminal.output(&apc_b64("a=T,f=24,s=10,v=10,i=79,q=2", &rgb_block(1, 1)));
    barrier(&mut yinz, &terminal, "quiet-done");

    // Checked as "no graphics APC at all", not as "no *parseable* reply": this crate's
    // `parse_replies` skips what it cannot parse, so a malformed reply would otherwise read as
    // silence.
    let after = terminal.stdin_bytes();
    assert!(
        !contains_graphics_apc(&after[before..]),
        "a q=2 command must produce no reply, got {:?}",
        String::from_utf8_lossy(&after[before..])
    );

    yinz.quit();
}

#[test]
fn a_chunked_image_from_a_real_pane_reaches_the_client_as_one_image() {
    // Spec: a large image is split into chunks of at most 4096 base64 bytes. The pane has to
    // reassemble them before anything is shown, and what reaches the host is one image.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    terminal.output(b"\x1b[H");
    let raster = rgb_block(40, 40);
    let encoded = base64_encode(&raster);
    let chunk_size = 1024;
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(chunk_size)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect();
    assert!(chunks.len() > 2, "the fixture must actually be chunked");

    for (index, chunk) in chunks.iter().enumerate() {
        let control = if index == 0 {
            "a=T,f=24,s=40,v=40,i=80,C=1,q=2,m=1".to_owned()
        } else if index == chunks.len() - 1 {
            "m=0".to_owned()
        } else {
            "m=1".to_owned()
        };
        let mut bytes = format!("\x1b_G{};", control).into_bytes();
        bytes.extend_from_slice(chunk.as_bytes());
        bytes.extend_from_slice(b"\x1b\\");
        terminal.output(&bytes);
    }

    yinz.wait_until_raw_output("the reassembled image reaches the client", |bytes| {
        contains_bytes(bytes, b"\x1b_Ga=p,q=2,i=2000000000,p=1,")
    });

    yinz.quit();
}

#[test]
fn a_refused_command_is_answered_and_the_pane_carries_on() {
    // A client that asks for something yinz will not do must get a refusal it can act on, and
    // the bytes that follow must still be processed normally.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    // An animation frame for an image that was never transmitted: the frame has nowhere to live.
    terminal.output(&apc_b64("a=f,f=24,s=2,v=2,i=81", &rgb_block(2, 2)));

    let reply = wait_for_reply(&terminal, "the command is refused", |reply| {
        reply.image_id == Some(81)
    });
    assert_eq!(reply.code, "ENOENT");

    barrier(&mut yinz, &terminal, "after-refusal");

    yinz.quit();
}

#[test]
fn erasing_the_screen_withdraws_the_images_from_the_host() {
    // Spec: `ESC[2J` removes the images along with the screen contents, so the host terminal
    // must be told to drop them rather than leaving them painted over the next screen.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    terminal.output(b"\x1b[H");
    terminal.output(&apc_b64(
        "a=T,f=24,s=2,v=96,i=82,C=1,q=2",
        &rgb_block(2, 96),
    ));
    yinz.wait_until_raw_output("the placement reaches the client", |bytes| {
        contains_bytes(bytes, b"\x1b_Ga=p,q=2,i=2000000000,p=1,")
    });

    terminal.output(b"\x1b[2J");
    yinz.wait_until_raw_output("the host is told to delete the image", |bytes| {
        contains_bytes(bytes, b"\x1b_Ga=d,q=2,d=I,i=2000000000\x1b\\")
    });

    yinz.quit();
}

#[test]
fn erasing_the_scrollback_leaves_the_images_on_the_host() {
    // Spec, "Interaction with other terminal actions": "The other commands to erase text must have
    // no effect on graphics". `ESC[3J` erases the scrollback and does not touch the screen, so the
    // image on screen must stay — and the host must not be told to delete it.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    terminal.output(b"\x1b[H");
    terminal.output(&apc_b64(
        "a=T,f=24,s=2,v=96,i=83,C=1,q=2",
        &rgb_block(2, 96),
    ));
    let baseline = yinz
        .wait_until_raw_output("the placement reaches the client", |bytes| {
            contains_bytes(bytes, b"\x1b_Ga=p,q=2,i=2000000000,p=1,")
        })
        .len();

    terminal.output(b"\x1b[3J");
    barrier(&mut yinz, &terminal, "after-ed3");

    let bytes = yinz.raw_bytes();
    assert!(
        !contains_bytes(&bytes[baseline..], b"\x1b_Ga=d,q=2,d=I,i=2000000000\x1b\\"),
        "erasing the scrollback must not delete the image on screen"
    );

    yinz.quit();
}

#[test]
fn a_temp_file_without_the_marker_is_not_deleted() {
    // Spec, "The transmission medium": a `t=t` transfer deletes the file only when it is in a
    // known temporary directory *and* its path contains `tty-graphics-protocol`. Here only the
    // first condition holds, so the file must survive.
    //
    // End to end because this is the case where the cost of getting it wrong is real: an
    // application pointing yinz at a file it did not create for this transfer loses that file.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    let path = std::env::temp_dir().join(format!(
        "yinz-kitty-conformance-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, rgb_block(2, 2)).expect("write the fixture file");

    terminal.output(&apc_b64(
        "a=T,f=24,s=2,v=2,i=84,C=1,t=t",
        path.to_string_lossy().as_bytes(),
    ));
    wait_for_reply(&terminal, "the transfer is answered", |reply| {
        reply.image_id == Some(84)
    });

    let survived = path.exists();
    let _ = std::fs::remove_file(&path);
    yinz.quit();

    assert!(
        survived,
        "a temp-directory file without the tty-graphics-protocol marker must not be deleted"
    );
}

#[test]
fn a_marked_temp_file_is_deleted_after_the_transfer() {
    // The conforming half of the same rule: both conditions hold, so the file is the terminal's
    // to clean up.
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    setup_kitty_host(&yinz, &terminal);

    let path = std::env::temp_dir().join(format!(
        "tty-graphics-protocol-yinz-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, rgb_block(2, 2)).expect("write the fixture file");

    terminal.output(&apc_b64(
        "a=T,f=24,s=2,v=2,i=85,C=1,t=t",
        path.to_string_lossy().as_bytes(),
    ));
    wait_for_reply(&terminal, "the transfer is answered", |reply| {
        reply.image_id == Some(85)
    });

    let deleted = !path.exists();
    let _ = std::fs::remove_file(&path);
    yinz.quit();

    assert!(deleted, "a marked temp file must be deleted after the read");
}

#[test]
fn kitty_handshake_waits_for_a_late_positive_host_reply() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.disable_echo();
    yinz.send_stdin(b"\x1b[6;21;8t");
    // A DA barrier without a probe reply reports unsupported but permits a late OK.
    yinz.send_stdin(b"\x1b[?62;22c");
    let replies = terminal.tap_stdin();
    let deadline = std::time::Instant::now() + yinz_integration_tests::default_timeout();
    let mut bytes = Vec::new();
    terminal.output(b"\x1b_Ga=q,i=32,s=1,v=1,t=d,f=24;AAAA\x1b\\");
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "host refusal was not applied"
        );
        bytes.extend(
            replies
                .recv_deadline(deadline)
                .expect("initial probe reply"),
        );
        if let Some(reply) = parse_replies(&bytes)
            .into_iter()
            .find(|r| r.image_id == Some(32))
        {
            if reply.code == "ENOTSUPPORTED" {
                break;
            }
            assert!(reply.is_ok(), "{reply:?}");
            bytes.clear();
            terminal.output(b"\x1b_Ga=q,i=32,s=1,v=1,t=d,f=24;AAAA\x1b\\");
        }
    }
    let before = terminal.stdin_bytes().len();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            terminal.wait_for_stdin("application saw unsupported before host reply", |bytes| {
                parse_replies(&bytes[before..])
                    .iter()
                    .any(|reply| reply.image_id == Some(31) && reply.code == "ENOTSUPPORTED")
            });
            yinz.send_stdin(KITTY_PROBE_ACK);
        });
        yinz_integration_tests::kitty_protocol::wait_for_kitty_host(&terminal);
    });
    yinz.quit();
}
