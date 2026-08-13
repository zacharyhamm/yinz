use super::*;

use nix::pty::openpty;
use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};
use nix::sys::stat::fstat;
use std::io::{Read, Write};
use std::os::fd::{AsFd, IntoRawFd};
use std::time::{Duration, Instant};

fn stream_pair() -> (UnixStream, UnixStream) {
    let (a, b) = socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::SOCK_CLOEXEC,
    )
    .expect("socketpair failed");
    (UnixStream::from(a), UnixStream::from(b))
}

fn is_cloexec(fd: RawFd) -> bool {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags = fcntl(borrowed, FcntlArg::F_GETFD).expect("F_GETFD failed");
    FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC)
}

/// The whole design rests on this: the supervisor opens the pty and the server
/// drives the master it was handed, with no byte proxying in between.
#[test]
fn a_handed_off_pty_master_is_usable_by_the_receiver() {
    let (sender, receiver) = stream_pair();
    let pty = openpty(None, &None).expect("openpty failed");

    send_fd(&sender, pty.master.as_fd().as_raw_fd()).expect("send_fd failed");
    let received = recv_fd(&receiver).expect("recv_fd failed");

    // Writing to the received master must show up on the slave, proving it is the
    // same pty and not a copy of something else.
    let mut master = unsafe { std::fs::File::from_raw_fd(received.into_raw_fd()) };
    master
        .write_all(b"hello\n")
        .expect("write to master failed");
    master.flush().expect("flush failed");

    let mut slave = unsafe { std::fs::File::from_raw_fd(pty.slave.into_raw_fd()) };
    let mut buf = [0u8; 6];
    slave.read_exact(&mut buf).expect("read from slave failed");
    assert_eq!(&buf, b"hello\n");
}

/// Without this a pty master briefly exists as an inheritable descriptor, and a
/// supervisor spawned concurrently for another pane would inherit it — keeping a
/// dead pane's pty open and suppressing its EOF.
#[test]
fn received_descriptors_are_close_on_exec() {
    let (sender, receiver) = stream_pair();
    let pty = openpty(None, &None).expect("openpty failed");

    send_fd(&sender, pty.master.as_fd().as_raw_fd()).expect("send_fd failed");
    let received = recv_fd(&receiver).expect("recv_fd failed");

    assert!(
        is_cloexec(received.as_raw_fd()),
        "handed-off pty master must not be inheritable across exec"
    );
}

#[test]
fn recv_fd_reports_a_closed_channel_rather_than_blocking() {
    let (sender, receiver) = stream_pair();
    drop(sender);
    let err = recv_fd(&receiver).expect_err("expected an error on a closed channel");
    assert!(
        format!("{err:#}").contains("closed"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn cloexec_helpers_round_trip() {
    let (sock, _peer) = stream_pair();
    let fd = sock.as_raw_fd();
    assert!(is_cloexec(fd), "socketpair was created with SOCK_CLOEXEC");
    clear_cloexec(fd).expect("clear_cloexec failed");
    assert!(!is_cloexec(fd));
    set_cloexec(fd).expect("set_cloexec failed");
    assert!(is_cloexec(fd));
}

/// `set_cloexec` returns early when the flag is already set while `clear_cloexec`
/// always writes, so the two are not symmetric in shape. Both are called on
/// descriptors whose state we do not know — `recv_fd` calls `set_cloexec` on one
/// that `MSG_CMSG_CLOEXEC` may already have marked — so both have to be safe to
/// repeat.
#[test]
fn the_cloexec_helpers_are_idempotent() {
    let (sock, _peer) = stream_pair();
    let fd = sock.as_raw_fd();

    set_cloexec(fd).expect("set_cloexec failed");
    set_cloexec(fd).expect("set_cloexec should be repeatable");
    assert!(
        is_cloexec(fd),
        "set_cloexec twice should leave the flag set"
    );

    clear_cloexec(fd).expect("clear_cloexec failed");
    clear_cloexec(fd).expect("clear_cloexec should be repeatable");
    assert!(
        !is_cloexec(fd),
        "clear_cloexec twice should leave the flag clear"
    );

    set_cloexec(fd).expect("set_cloexec failed");
    assert!(is_cloexec(fd), "the pair should compose back to set");
}

/// A peer that sends the sentinel byte without attaching a descriptor — a
/// supervisor that got its `sendmsg` wrong, or a plain `write` racing the
/// ancillary data — must be reported rather than leaving the server holding a
/// descriptor it never received.
#[test]
fn a_handoff_that_carries_no_descriptor_is_reported() {
    let (mut sender, receiver) = stream_pair();
    sender.write_all(b"F").expect("write failed");

    let err = recv_fd(&receiver).expect_err("a payload with no descriptor should be refused");
    assert!(
        format!("{err:#}").contains("no descriptor"),
        "unexpected error: {err:#}"
    );
}

fn send_two_fds(sock: &UnixStream, first: RawFd, second: RawFd) {
    let fds = [first, second];
    let cmsgs = [ControlMessage::ScmRights(&fds)];
    let payload = [b'F'];
    let iov = [IoSlice::new(&payload)];
    sendmsg::<()>(sock.as_raw_fd(), &iov, &cmsgs, MsgFlags::empty(), None)
        .expect("sendmsg with two descriptors failed");
}

/// The kernel installs every descriptor a peer attaches, asked for or not, so a
/// supervisor that sent two would leak one into the server for the life of the
/// session. `recv_fd` keeps the first and drops the rest, and dropping an
/// `OwnedFd` closes it.
///
/// Proven by what the extra descriptor keeps alive rather than by counting open
/// files: `/proc/self/fd` is process-wide, and the rest of the suite is opening
/// and closing descriptors on other threads throughout. Here the second
/// descriptor is one end of a socketpair whose only other copy we drop, so the
/// probe reads EOF once the descriptor is gone.
///
/// Retried against a deadline rather than read once: other tests in this binary
/// fork, and a child that has forked but not yet exec'd holds a dup of every
/// descriptor open in this process, this handoff's extra one included. That keeps
/// the probe's peer alive for the microseconds until the exec, which has nothing
/// to do with the rule under test. A descriptor `recv_fd` really leaked is held
/// for the life of the test process and never reports EOF, so the deadline still
/// fails the case this test is for.
#[test]
fn only_the_first_descriptor_of_a_handoff_is_kept() {
    let (sender, receiver) = stream_pair();
    let pty = openpty(None, &None).expect("openpty failed");
    let (probe, extra) = stream_pair();
    probe
        .set_nonblocking(true)
        .expect("set_nonblocking failed on the probe");

    send_two_fds(&sender, pty.master.as_fd().as_raw_fd(), extra.as_raw_fd());
    // Our own copy goes, so the descriptor the kernel installed for `recv_fd` is
    // the only one left holding the probe's peer open.
    drop(extra);

    let received = recv_fd(&receiver).expect("recv_fd failed");
    assert!(
        is_cloexec(received.as_raw_fd()),
        "the descriptor that was kept must not be inheritable across exec"
    );

    // Which of the two was kept, not merely that one was: both arrive marked
    // cloexec, so the flag alone would be just as happy if `recv_fd` had kept the
    // socket that followed the master. A descriptor the kernel installs from
    // `SCM_RIGHTS` shares its open file description with the one that was sent, so
    // device and inode match the master exactly and match nothing else.
    let sent = fstat(pty.master.as_fd()).expect("fstat on the sent master failed");
    let kept = fstat(received.as_fd()).expect("fstat on the received descriptor failed");
    assert_eq!(
        (kept.st_dev, kept.st_ino),
        (sent.st_dev, sent.st_ino),
        "the kept descriptor should be the pty master, the first of the two sent"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0u8; 1];
    loop {
        match (&probe).read(&mut buf) {
            Ok(0) => break,
            Ok(n) => panic!("the probe unexpectedly carried {n} bytes"),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    panic!(
                        "the second descriptor of the handoff was still open for the whole \
                         deadline — recv_fd leaked it, and a supervisor could hold a pane's \
                         pty open forever"
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            },
            Err(e) => panic!("reading the probe failed: {e}"),
        }
    }
}
