//! Passing a pty master descriptor between the supervisor and zellij-server over
//! the control socket, via `SCM_RIGHTS`.
//!
//! The handoff is deliberately its own one-byte `sendmsg`/`recvmsg` pair rather
//! than ancillary data bolted onto a framed message. Ancillary data is delivered
//! to whichever `recvmsg` consumes the byte it was attached to, and is silently
//! dropped if a plain `read` gets there first — so the control socket is always
//! read with exact-sized reads and never wrapped in a `BufReader`.

use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, FdFlag},
    sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags},
};
use zellij_runtime::errors::prelude::*;

/// Payload byte carrying the descriptor. The value is arbitrary; `sendmsg` needs
/// at least one byte for the ancillary data to have somewhere to attach.
const FD_SENTINEL: u8 = b'F';

/// How many descriptors a single handoff is willing to receive. Only the first is
/// kept; the rest are closed. A peer that attaches more than this fails the
/// receive outright on macOS, which is a reported error rather than a leak.
const MAX_HANDOFF_FDS: usize = 4;

pub fn send_fd(sock: &UnixStream, fd: RawFd) -> Result<()> {
    let err_context = || "failed to hand off pty descriptor".to_string();
    let fds = [fd];
    let cmsgs = [ControlMessage::ScmRights(&fds)];
    let payload = [FD_SENTINEL];
    let iov = [IoSlice::new(&payload)];
    loop {
        match sendmsg::<()>(sock.as_raw_fd(), &iov, &cmsgs, MsgFlags::empty(), None) {
            Ok(0) => {
                return Err(anyhow!("peer accepted no bytes of the descriptor handoff"))
                    .with_context(err_context)
            },
            Ok(_) => return Ok(()),
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e).with_context(err_context),
        }
    }
}

pub fn recv_fd(sock: &UnixStream) -> Result<OwnedFd> {
    let err_context = || "failed to receive pty descriptor".to_string();
    // MSG_CMSG_CLOEXEC where it exists, so a concurrently spawned process cannot
    // inherit the master. macOS has no such flag, so it gets a (racy, but
    // best-available) fcntl right after the receive.
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd"
    ))]
    let flags = MsgFlags::MSG_CMSG_CLOEXEC;
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd"
    )))]
    let flags = MsgFlags::empty();

    loop {
        let mut buf = [0u8; 1];
        let mut iov = [IoSliceMut::new(&mut buf)];
        // Room for more than the one descriptor a correct peer sends: Linux
        // truncates an oversized set of rights and closes the excess, but macOS
        // refuses the whole `recvmsg` with ENOBUFS and strands them. Receiving the
        // extras is what lets the loop below close them.
        let mut cmsg_buffer = nix::cmsg_space!([RawFd; MAX_HANDOFF_FDS]);
        let msg = match recvmsg::<()>(
            sock.as_raw_fd(),
            &mut iov,
            Some(cmsg_buffer.as_mut_slice()),
            flags,
        ) {
            Ok(msg) => msg,
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e).with_context(err_context),
        };
        if msg.bytes == 0 {
            return Err(anyhow!(
                "control channel closed before the pty descriptor arrived"
            ))
            .with_context(err_context);
        }
        let mut received: Option<OwnedFd> = None;
        for cmsg in msg.cmsgs().with_context(err_context)? {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                for fd in fds {
                    // SAFETY: the kernel just installed these descriptors in this
                    // process and nothing else owns them.
                    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
                    if received.is_none() {
                        received = Some(owned);
                    }
                    // Anything past the first is a protocol violation; dropping
                    // `owned` closes it.
                }
            }
        }
        let received = received
            .ok_or_else(|| anyhow!("handoff carried no descriptor"))
            .with_context(err_context)?;
        set_cloexec(received.as_raw_fd()).with_context(err_context)?;
        return Ok(received);
    }
}

/// No-op where `MSG_CMSG_CLOEXEC` already did the job, and the close-the-window
/// fallback where it does not exist.
pub fn set_cloexec(fd: RawFd) -> Result<()> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let current = fcntl(borrowed, FcntlArg::F_GETFD).context("failed to read fd flags")?;
    let mut flags = FdFlag::from_bits_truncate(current);
    if flags.contains(FdFlag::FD_CLOEXEC) {
        return Ok(());
    }
    flags.insert(FdFlag::FD_CLOEXEC);
    fcntl(borrowed, FcntlArg::F_SETFD(flags)).context("failed to set FD_CLOEXEC")?;
    Ok(())
}

/// Clear `FD_CLOEXEC` so a descriptor survives `exec`. Used to hand the control
/// socket to the supervisor on fd 3.
pub fn clear_cloexec(fd: RawFd) -> Result<()> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let current = fcntl(borrowed, FcntlArg::F_GETFD).context("failed to read fd flags")?;
    let mut flags = FdFlag::from_bits_truncate(current);
    flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(borrowed, FcntlArg::F_SETFD(flags)).context("failed to clear FD_CLOEXEC")?;
    Ok(())
}

#[cfg(test)]
#[path = "../unit/pty_host_fdpass_tests.rs"]
mod pty_host_fdpass_tests;
