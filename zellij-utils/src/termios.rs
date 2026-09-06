use serde::{Deserialize, Serialize};

/// A same-platform, serializable snapshot of a `termios`.
///
/// Captured before the client enters raw mode and passed to the server and local
/// pane supervisors, which cannot recover the original settings themselves.
///
/// The four flag words and the control characters come from nix's public
/// `Termios` fields, which are the authoritative copy — `Termios` syncs them into
/// its private `libc::termios` before handing it to `openpty`. Baud rates are not
/// exposed as public fields (on BSD/macOS they live in `c_ispeed`/`c_ospeed`, on
/// Linux inside `c_cflag`), so they go through `cfgetispeed`/`cfsetispeed`, which
/// are correct on both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTermios {
    pub iflag: u64,
    pub oflag: u64,
    pub cflag: u64,
    pub lflag: u64,
    pub control_chars: Vec<u8>,
    pub ispeed: u64,
    pub ospeed: u64,
    #[serde(default)]
    pub line_discipline: u8,
}

impl WireTermios {
    // `tcflag_t` is `c_uint` on Linux and `c_ulong` on macOS, so the widening
    // casts onto the `u64` wire fields below are no-ops on one and required on
    // the other. Clippy only ever sees one platform at a time.
    #[allow(clippy::unnecessary_cast)]
    pub fn from_termios(termios: &nix::sys::termios::Termios) -> Self {
        // `From<Termios> for libc::termios` hands back the inner struct, which is
        // where the baud rate lives on platforms that keep it out of `c_cflag`.
        let inner: libc::termios = termios.clone().into();
        let ispeed = unsafe { libc::cfgetispeed(&inner) } as u64;
        let ospeed = unsafe { libc::cfgetospeed(&inner) } as u64;
        WireTermios {
            iflag: termios.input_flags.bits() as u64,
            oflag: termios.output_flags.bits() as u64,
            cflag: termios.control_flags.bits() as u64,
            lflag: termios.local_flags.bits() as u64,
            control_chars: termios.control_chars.to_vec(),
            ispeed,
            ospeed,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            line_discipline: termios.line_discipline,
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            line_discipline: 0,
        }
    }

    pub fn to_termios(&self) -> nix::sys::termios::Termios {
        let mut raw: libc::termios = unsafe { std::mem::zeroed() };
        raw.c_iflag = self.iflag as libc::tcflag_t;
        raw.c_oflag = self.oflag as libc::tcflag_t;
        raw.c_cflag = self.cflag as libc::tcflag_t;
        raw.c_lflag = self.lflag as libc::tcflag_t;
        let to_copy = self.control_chars.len().min(raw.c_cc.len());
        raw.c_cc[..to_copy].copy_from_slice(&self.control_chars[..to_copy]);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            raw.c_line = self.line_discipline;
        }
        // After the flag words, never before: on Linux the baud rate is encoded
        // into `c_cflag`, so setting it first would be overwritten above.
        unsafe {
            libc::cfsetispeed(&mut raw, self.ispeed as libc::speed_t);
            libc::cfsetospeed(&mut raw, self.ospeed as libc::speed_t);
        }
        nix::sys::termios::Termios::from(raw)
    }
}
