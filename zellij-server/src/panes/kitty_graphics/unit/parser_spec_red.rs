//! Parser-level deviations from the kitty graphics specification. **These tests fail on purpose.**
//!
//! Every test here asserts what `graphics-protocol.rst` requires; the implementation does
//! something else. A failure is the to-do item, not a regression. When one starts passing, drop
//! its `red_` prefix and move it into `parser_spec_tests.rs`.
//!
//! Filter on the module path, not the test-name prefix: `red_` also appears inside unrelated
//! names such as `test_has_rendered_assets`, while every expected-red test — and nothing else —
//! lives in a module ending `_red`.
//!
//! ```sh
//! cargo x test -- --skip _red::                          # the green gate
//! cargo test -p zellij-server --all-features -- _red::   # exactly the red set
//! cargo x integration-test -- -E 'test(/^red_/)'         # the whole-app red set
//! ```
//!
//! # Master index of the red set
//!
//! | id | tests | spec | today | fix site |
//! |----|-------|------|-------|----------|
//! | RED-1 | `red_temp_file_*` (here), `red_temp_file_*` (integration) | "the terminal emulator should only delete the file if it is in a known temporary directory [...] **and** the file has the string `tty-graphics-protocol` in its full file path" | the two conditions are ORed, so any file under `/tmp` is deleted | `parser.rs::should_delete_temp_file` |
//! | RED-2 | `red_ed3_*` (grid, integration) | only `ESC[2J` and a terminal reset clear images; "the other commands to erase text must have no effect on graphics" | `ESC[3J` calls `clear_all_placements`, wiping visible images too | `grid.rs`, the ED-3 branch |
//! | RED-3 | `red_scaling_*` (grid_state, grid) | when scaling with only `c` or only `r`, the other axis follows the aspect ratio; the `X`/`Y` cell offsets "are not added to the number of columns/rows" | the perpendicular offset is folded into the derived axis | `grid_state.rs::place`, the `(c, 0)` and `(0, r)` branches |
//! | RED-4 | `red_relative_*` (grid) | `P`/`Q` position a placement relative to a parent, with `ENOPARENT`, `EINVAL`, `ECYCLE` and `ETOODEEP` responses | `P`/`Q`/`H`/`V` fall into the ignore arm, so the placement is accepted and positioned absolutely | `parser.rs` key match, `grid_state.rs::place` |
//! | RED-5 | `red_anim_*` (grid) | animation: `a=f` frame transmission, `a=a` control, `a=c` compose, `d=f`/`d=F` frame deletion | `a=f/a/c` answer ENOTSUPPORTED; `d=f` is not even a recognised delete specifier | `parser.rs`, `store.rs`, `grid_state.rs` |
//! | RED-6 | `red_uph_*` (grid) | unicode placeholders: `U=1` virtual placements materialised by `U+10EEEE` cells, ids from the foreground colour, rows/columns from combining marks | `U!=0` answers ENOTSUPPORTED; the grid has no placeholder handling | `parser.rs`, `grid.rs::print`, `grid_state.rs` |
//! | RED-7 | `red_shm_*` (here, grid) | `t=s` reads a POSIX shared memory object and unlinks it afterwards | answers ENOTSUPPORTED | `parser.rs::run_payload_pipeline` |
//! | RED-8 | `red_an_image_larger_than_the_whole_store_reports_enospc` (grid) | *not a spec rule* — a chosen improvement: an image too big for the whole quota is storage pressure, and `ENOSPC` says "retry smaller" where `EINVAL` says "your command was malformed" | `store_image` answers `EINVAL`, indistinguishable from a malformed command | `store.rs::store_image`, a new `KittyErrorCode::Enospc` |
//! | RED-9 | `red_png_*` (here) | kitty answers `EBADPNG` for malformed PNG data (`graphics_tests.py::test_load_png_simple`, including the short-chunk heap-overflow regression) | `decode_png` answers `EINVAL` for every PNG failure | `parser.rs::decode_png`, a new `KittyErrorCode::Ebadpng` |
//!
//! 65 red tests in `zellij-server` plus 2 in `zellij-integration-tests` — 57 in `zellij-server`
//! on non-Linux, where the eight `red_shm_*` tests are `#[cfg(target_os = "linux")]`.
//!
//! RED-1 and RED-2 are ordinary bugs in supported features and are the ones worth fixing first;
//! RED-4 is a bug in that a client is told a relative placement worked when it did not; RED-5,
//! RED-6 and RED-7 are whole features that were never implemented. RED-6 matters most for zellij
//! specifically: unicode placeholders are how a program displays an image without controlling the
//! cursor, which is the position every program inside a multiplexer is in. Two of the RED-4 tests
//! — `red_relative_a_virtual_placement_cannot_also_be_relative` and
//! `red_relative_a_child_of_a_virtual_parent_sits_at_the_placeholder_minimum` — need RED-6 as
//! well, and cannot go green before it does.
//!
//! RED-8 and RED-9 are the two entries the specification does not itself require: it names no
//! error code for either case. They are here because both codes are ones a client can act on,
//! where today's `EINVAL` tells it only that something was wrong. Each has a green counterpart
//! asserting the case is refused at all, so the refusal itself stays in the green gate and only
//! the choice of code is outstanding.
//!
//! # Judgement calls, deliberately left green
//!
//! Eight places where the specification does not say enough to call the current behaviour wrong,
//! or where two of its rules pull in opposite directions. They are pinned as-is so a future
//! change to any of them is a visible decision:
//!
//! * `ESC[2J` scrolls the screen into the scrollback rather than discarding it, so a short image
//!   at the top follows its text out of the viewport instead of being freed, while one too tall to
//!   fit in the scrolled-away line is cut by the viewport top and freed outright. Nothing is left
//!   on screen either way, which is all the spec asks of `[2J`. See the three tests under
//!   "interaction with erase commands" in `kitty_spec`, in particular
//!   `erasing_the_whole_screen_lets_a_short_image_follow_its_line_into_the_scrollback`.
//! * `i=0,I=<n>` is accepted, although "Specifying both `i` and `I` keys in any command is an
//!   error". `i=0` is the protocol's own way of spelling "no image id" — it is the default, and
//!   `p` is documented as ignored for an image with `id=0` — so reading it as absent rather than
//!   as a specified id is the more useful of the two readings. See
//!   `parser_spec_tests::an_explicit_zero_image_id_does_not_count_as_specifying_one`.
//! * `ESC[3J` *does* free images that live only in the scrollback, even though RED-2 reads the
//!   same sentence as "erase commands must not touch graphics at all". An image anchored to a
//!   line that no longer exists has nowhere to live. Fixing RED-2 must not overshoot into this:
//!   `kitty_spec::erasing_the_scrollback_removes_the_images_that_lived_only_there` is the guard.
//! * A terminal reset drops the scrollback, and with it images that had scrolled into it. kitty
//!   keeps its history buffer across a reset and so keeps those images; zellij's behaviour is
//!   self-consistent, and kitty's assertion is deliberately not ported.
//! * `X`/`Y` offsets at or beyond the cell size are clamped. The spec says they "must be smaller
//!   than the size of the cell" but gives the terminal no way to say otherwise.
//! * `N=1`, the transient-storage hint, is accepted and ignored. The spec allows either.
//! * An image whose width fills the pane wraps the cursor to the start of the next line. The spec
//!   says that when the advance would take the cursor off the screen "the exact positioning of
//!   the cursor is undefined, and up to implementations". See
//!   `kitty_spec::an_image_reaching_the_right_edge_wraps_the_cursor_to_the_next_line`.
//! * Control keys this terminal does not implement are ignored rather than rejected. The spec is
//!   silent and kitty is stricter (EINVAL on an unknown key character); a multiplexer sits
//!   between clients and a possibly-newer host terminal, so lenience is the safer default. See
//!   `parser_spec_tests::keys_this_terminal_does_not_implement_are_ignored_rather_than_failing`.

use super::super::parser::*;
use super::super::test_support::*;

fn parse(control: &str, payload: &[u8]) -> Result<KittyCommand, KittyError> {
    let mut parser = KittyCommandParser::new();
    parser
        .parse(unwrap_apc(&apc_b64(control, payload)))
        .expect("a complete command produces a result")
}

// ---- RED-1: the temp-file deletion policy is an OR where the spec requires an AND ----

#[test]
fn red_temp_file_without_the_marker_survives_even_inside_a_temp_directory() {
    // Spec, "the transmission medium": for `t=t` the terminal deletes the file only if it is in
    // a known temporary directory *and* its path contains `tty-graphics-protocol`. The marker is
    // what distinguishes a file the client created for this transfer from any other file the
    // client happened to name, so deleting without it destroys unrelated user data.
    let raster = rgb_block(2, 2);
    let file = unmarked_temp_file(&raster);

    let command = parse("a=T,f=24,s=2,v=2,i=1,t=t", &file.payload()).expect("transfer succeeds");
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(
        file.exists(),
        "a temp-directory file without the tty-graphics-protocol marker must not be deleted"
    );
}

#[test]
fn red_temp_file_with_the_marker_outside_a_temp_directory_survives() {
    // The other half of the same rule: the marker alone does not license deletion, because a
    // client can put that string in any path it likes.
    let raster = rgb_block(2, 2);
    let file = marked_non_temp_file(&raster);

    let command = parse("a=T,f=24,s=2,v=2,i=1,t=t", &file.payload()).expect("transfer succeeds");
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(
        file.exists(),
        "a file outside any temporary directory must not be deleted"
    );
}

// ---- RED-9: malformed PNG data is EBADPNG, not EINVAL ----
//
// The protocol reserves no code for this, but kitty has one and clients that already speak to
// kitty will recognise it. It is worth distinguishing: `EINVAL` says "your control data was
// malformed", which a client answers by fixing its command, where `EBADPNG` says "your control
// data was fine and your image was not", which it answers by re-encoding the image.
//
// That each of these payloads is refused *at all* — in particular that the short-chunk one does
// not read out of bounds — is asserted in the green gate by
// `parser_spec_tests::malformed_png_payloads_are_all_refused_rather_than_decoded`. Only the code
// is outstanding here.
//
// Asserted on the wire spelling rather than on a `KittyErrorCode` variant, because there is no
// `Ebadpng` variant to name yet: a red test has to compile and fail, not fail to compile.

/// The errno-style name a malformed-PNG refusal carries to the client.
fn png_error_code(payload: &[u8]) -> &'static str {
    parse("a=T,f=100,i=1", payload)
        .expect_err("a malformed png is refused")
        .code
        .as_str()
}

#[test]
fn red_png_data_that_is_not_a_png_is_reported_as_a_bad_png() {
    assert_eq!(png_error_code(b"definitely not a png"), "EBADPNG");
}

#[test]
fn red_png_a_truncated_png_is_reported_as_a_bad_png() {
    let image = png(4, 4, PngKind::Rgba8);
    assert_eq!(png_error_code(&image[..image.len() / 2]), "EBADPNG");
}

#[test]
fn red_png_a_corrupt_pixel_stream_is_reported_as_a_bad_png() {
    let mut image = png(4, 4, PngKind::Rgba8);
    let len = image.len();
    // Scribble over the compressed image data, leaving the header intact.
    for byte in image[len / 2..len - 12].iter_mut() {
        *byte = 0;
    }
    assert_eq!(png_error_code(&image), "EBADPNG");
}

#[test]
fn red_png_a_short_chunk_is_reported_as_a_bad_png() {
    // kitty regression (`test_load_png_simple`): a PNG payload shorter than the decoder's initial
    // buffer must produce an error rather than an out-of-bounds read. kitty answers EBADPNG.
    assert_eq!(
        png_error_code(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a]),
        "EBADPNG"
    );
}

// ---- RED-7: shared memory transmission ----

#[cfg(target_os = "linux")]
mod shared_memory {
    use super::*;

    /// A POSIX shared memory object. On Linux `/dev/shm` *is* the shm namespace, so an object can
    /// be created and inspected with ordinary file operations.
    struct ShmObject {
        name: String,
    }

    impl ShmObject {
        fn new(bytes: &[u8]) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                "/zellij-kitty-shm-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            std::fs::write(format!("/dev/shm{}", name), bytes).expect("create shm object");
            ShmObject { name }
        }
        fn payload(&self) -> Vec<u8> {
            self.name.as_bytes().to_vec()
        }
        fn exists(&self) -> bool {
            std::path::Path::new(&format!("/dev/shm{}", self.name)).exists()
        }
    }

    impl Drop for ShmObject {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(format!("/dev/shm{}", self.name));
        }
    }

    #[test]
    fn red_shm_transfer_reads_the_object_contents() {
        // Spec, "the transmission medium": `t=s` names "a *shared memory object*, which on POSIX
        // systems is a POSIX shared memory object". It exists so a client can hand over a large
        // image without pushing it through the tty.
        let raster = rgb_block(4, 4);
        let object = ShmObject::new(&raster);

        let command =
            parse("a=T,f=24,s=4,v=4,i=1,t=s", &object.payload()).expect("transfer succeeds");
        assert_eq!(command.image.unwrap().bytes, raster);
    }

    #[test]
    fn red_shm_object_is_unlinked_after_being_read() {
        // Spec: "The terminal emulator must read the data from the memory object and then unlink
        // and close it on POSIX" — otherwise every image a client sends leaks memory until reboot.
        let object = ShmObject::new(&rgb_block(4, 4));
        let _ = parse("a=T,f=24,s=4,v=4,i=1,t=s", &object.payload());
        assert!(
            !object.exists(),
            "the object must be unlinked after the read"
        );
    }

    #[test]
    fn red_shm_transfer_honours_the_size_and_offset_keys() {
        let raster = rgb_block(2, 2);
        let mut padded = vec![0xffu8; 5];
        padded.extend_from_slice(&raster);
        let object = ShmObject::new(&padded);

        let command = parse(
            &format!("a=T,f=24,s=2,v=2,i=1,t=s,O=5,S={}", raster.len()),
            &object.payload(),
        )
        .expect("transfer succeeds");
        assert_eq!(command.image.unwrap().bytes, raster);
    }

    #[test]
    fn red_shm_missing_object_is_an_io_error() {
        // A client racing against its own cleanup should get EBADF, not a blanket "unsupported"
        // that tells it nothing about what went wrong.
        let error = parse(
            "a=T,f=24,s=2,v=2,i=1,t=s",
            b"/zellij-kitty-shm-does-not-exist",
        )
        .expect_err("a missing object cannot be read");
        assert_eq!(error.code, KittyErrorCode::Ebadf);
    }
}
