//! Shared memory transmission, end to end through a pane. **These tests fail on purpose.** See
//! the master index in `kitty_graphics/unit/parser_spec_red.rs`.
//!
//! RED-7, the grid half. The parser-level cases live in
//! `kitty_graphics/unit/parser_spec_red.rs`; what these add is that the transfer works through a
//! real grid and that the resulting image behaves like any other.
//!
//! Spec, "The transmission medium": `t=s` means the payload names "a *shared memory object*,
//! which on POSIX systems is a POSIX shared memory object", and "the terminal emulator must read
//! the data from the memory object and then unlink and close it on POSIX". It is the fastest
//! medium for large images, and the only one that does not push the whole raster through the tty.
//!
//! Fix site: `parser.rs::run_payload_pipeline`, plus a `KittyMedium::SharedMemory` variant.
//!
//! Linux only for now: `/dev/shm` is the POSIX shm namespace there, so the fixture is a plain
//! file write. A macOS variant needs `shm_open` through `libc`.
#![cfg(target_os = "linux")]

use super::*;

/// A POSIX shared memory object, named the way a client would name one.
struct ShmObject {
    name: String,
}

impl ShmObject {
    fn new(bytes: &[u8]) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "/zellij-kitty-grid-shm-{}-{}",
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
fn red_shm_transmitted_image_is_displayed_like_any_other() {
    let mut session = KittySession::new(10, 20);
    let object = ShmObject::new(&rgb_raster(10, 20));

    let reply = session.reply("a=T,f=24,s=10,v=20,i=1,t=s", &object.payload());
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
    assert_eq!(session.placement_count(), 1);
    assert_eq!(session.image_count(), 1);
}

#[test]
fn red_shm_transmitted_image_can_be_displayed_again_later() {
    // Once read, the image is an ordinary stored image; the medium it arrived by is forgotten.
    let mut session = KittySession::new(10, 20);
    let object = ShmObject::new(&rgb_raster(10, 20));
    session.reply("a=t,f=24,s=10,v=20,i=1,t=s", &object.payload());

    assert!(session.can_still_display_by_placing_it(1));
}

#[test]
fn red_shm_object_is_unlinked_once_the_pane_has_read_it() {
    let mut session = KittySession::new(10, 20);
    let object = ShmObject::new(&rgb_raster(10, 20));

    session.send("a=T,f=24,s=10,v=20,i=1,t=s", &object.payload());

    assert!(
        !object.exists(),
        "the terminal must unlink the object after reading it"
    );
}

#[test]
fn red_shm_a_missing_object_is_reported_as_an_io_error() {
    let mut session = KittySession::new(10, 20);
    let reply = session.err(
        "a=T,f=24,s=10,v=20,i=1,t=s",
        b"/zellij-kitty-grid-shm-missing",
    );
    assert_eq!(reply.code, "EBADF");
}
