use super::*;
use nix::fcntl::{fcntl, FcntlArg};
use nix::sys::termios::{tcgetattr, LocalFlags};

nix::ioctl_read_bad!(tiocgwinsz, libc::TIOCGWINSZ, Winsize);

#[test]
fn allocation_preserves_settings_and_protects_both_descriptors() {
    let source = nix::pty::openpty(None, None).unwrap();
    let mut termios = tcgetattr(&source.slave).unwrap();
    termios.local_flags.remove(LocalFlags::ECHO);
    let size = Winsize {
        ws_row: 37,
        ws_col: 91,
        ws_xpixel: 728,
        ws_ypixel: 666,
    };
    let pty = open(Some(&size), Some(&termios)).unwrap();
    for fd in [&pty.master, &pty.slave] {
        assert_ne!(fcntl(fd, FcntlArg::F_GETFD).unwrap() & libc::FD_CLOEXEC, 0);
    }
    assert_eq!(tcgetattr(&pty.slave).unwrap(), termios);
    let mut reported = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { tiocgwinsz(pty.slave.as_raw_fd(), &mut reported) }.unwrap();
    assert_eq!(
        (
            reported.ws_col,
            reported.ws_row,
            reported.ws_xpixel,
            reported.ws_ypixel
        ),
        (size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel)
    );
}

#[test]
fn a_winsize_set_on_the_pty_master_is_what_the_pty_reports() {
    let pty = open(None, None).unwrap();
    let size = Winsize {
        ws_row: 37,
        ws_col: 91,
        ws_xpixel: 728,
        ws_ypixel: 666,
    };
    resize(pty.master.as_raw_fd(), &size).unwrap();
    let mut reported = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { tiocgwinsz(pty.slave.as_raw_fd(), &mut reported) }.unwrap();
    assert_eq!(
        (
            reported.ws_col,
            reported.ws_row,
            reported.ws_xpixel,
            reported.ws_ypixel
        ),
        (size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel)
    );
}

#[test]
fn a_failed_winsize_ioctl_is_reported_rather_than_dropped() {
    assert_eq!(
        resize(
            -1,
            &Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0
            }
        ),
        Err(Errno::EBADF)
    );
}

#[test]
fn invalid_process_ids_are_rejected_before_group_or_leader_signalling() {
    for raw in [0, 1, i32::MAX as u32 + 1, u32::MAX] {
        assert!(signal_group_or_leader(ProcessId::new(raw), Signal::SIGCONT).is_err());
    }
}

#[test]
fn signalling_a_live_group_succeeds_and_an_exited_pane_is_not_an_error() {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = ProcessId::new(child.id());
    signal_group_or_leader(pid, Signal::SIGKILL).unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    // Both the group and its leader are gone: the user's action raced the exit.
    signal_group_or_leader(pid, Signal::SIGCONT).unwrap();
}
