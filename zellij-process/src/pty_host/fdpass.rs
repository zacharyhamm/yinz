//! Passing a pty master descriptor between the supervisor and zellij-server over
//! the control socket, via `SCM_RIGHTS`.
//!
//! The handoff is deliberately its own one-byte `sendmsg`/`recvmsg` pair rather
//! than ancillary data bolted onto a framed message. Ancillary data is delivered
//! to whichever `recvmsg` consumes the byte it was attached to, and is silently
//! dropped if a plain `read` gets there first — so the control socket is always
//! read with exact-sized reads and never wrapped in a `BufReader`.

use std::io::IoSlice;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, FdFlag},
    sys::socket::{sendmsg, ControlMessage, MsgFlags},
};
use zellij_runtime::errors::prelude::*;

/// Payload byte carrying the descriptor. The value is arbitrary; `sendmsg` needs
/// at least one byte for the ancillary data to have somewhere to attach.
const FD_SENTINEL: u8 = b'F';

/// How many descriptors a single handoff is willing to receive. Only the first is
/// kept; the rest are closed, including the descriptors installed before an
/// oversized ancillary message is truncated.
const MAX_HANDOFF_FDS: usize = 4;

// XNU installs every descriptor before copying out ancillary bytes and does not
// close the ones hidden by MSG_CTRUNC. Its sockargs(MT_CONTROL) rejects buffers
// larger than MCLBYTES (2048), so receive that full bound on Apple platforms.
// https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_syscalls.c
#[cfg(target_vendor = "apple")]
const HANDOFF_CONTROL_BYTES: usize = 2048;
#[cfg(not(target_vendor = "apple"))]
const HANDOFF_CONTROL_BYTES: usize = nix::sys::socket::cmsg_space::<[RawFd; MAX_HANDOFF_FDS]>();

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
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // nix refuses to iterate any SCM_RIGHTS after MSG_CTRUNC, but the kernel
        // has already installed the descriptors that fit. Receive directly so
        // they acquire owners even on that error path. usize supplies alignment.
        let mut cmsg_buffer =
            [0usize; HANDOFF_CONTROL_BYTES.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: msghdr is a C struct valid when zeroed; all buffers below stay
        // alive for the syscall and the subsequent ancillary-data iteration.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buffer.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&cmsg_buffer) as _;
        let bytes = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, flags.bits()) };
        if bytes < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).with_context(err_context);
        }
        let mut received: Option<OwnedFd> = None;
        let mut received_count = 0;
        // SAFETY: recvmsg populated msg and its aligned control buffer. Bound
        // each payload by the returned buffer length even on truncation; only
        // complete RawFds supplied by the kernel become owned descriptors.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&msg);
            while !header.is_null() {
                let offset = header.cast::<u8>().offset_from(msg.msg_control.cast()) as usize;
                let len = ((*header).cmsg_len as usize)
                    .min(msg.msg_controllen as usize - offset)
                    .saturating_sub(libc::CMSG_LEN(0) as usize);
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let data = libc::CMSG_DATA(header).cast::<RawFd>();
                    for index in 0..len / std::mem::size_of::<RawFd>() {
                        let owned = OwnedFd::from_raw_fd(data.add(index).read_unaligned());
                        received_count += 1;
                        if received.is_none() {
                            received = Some(owned);
                        }
                    }
                }
                header = libc::CMSG_NXTHDR(&msg, header);
            }
        }
        if bytes == 0 {
            return Err(anyhow!(
                "control channel closed before the pty descriptor arrived"
            ))
            .with_context(err_context);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(anyhow!("descriptor handoff was truncated")).with_context(err_context);
        }
        if received_count > MAX_HANDOFF_FDS {
            return Err(anyhow!("descriptor handoff carried too many descriptors"))
                .with_context(err_context);
        }
        if buf[0] != FD_SENTINEL {
            return Err(anyhow!("invalid descriptor handoff sentinel")).with_context(err_context);
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
