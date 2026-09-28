//! What sits between this client's stdout and the terminal a human is looking at.
//!
//! Most of the time: nothing, or an ssh pipe, which passes bytes through untouched and lets the
//! startup probes in [`crate::stdin_handler`] measure the real terminal. Mosh is the exception. It
//! is not a pipe but a pair of terminal emulators - mosh-server keeps a screen model on this side
//! and mosh-client repaints it on the other - so an escape sequence mosh does not itself implement
//! never reaches the terminal underneath, and no reply comes back.
//!
//! Kitty graphics is one of those. Worse than the image simply not arriving: the protocol draws
//! through `U+10EEEE` placeholder cells, which *are* ordinary text and do reach the far terminal,
//! so a client that emits them over mosh gets a block of unassigned codepoints and a screen model
//! that no longer matches what is on screen.
//!
//! The probe would normally catch this on its own - mosh answers the Primary-DA barrier that
//! backstops it - but detecting mosh directly means the answer does not depend on that, is known
//! before the first frame rather than a round trip later, and costs nothing when it is wrong,
//! since the probe is still there.

use std::collections::HashMap;

/// Transports whose own terminal emulator repaints the screen and cannot pass a kitty graphics
/// APC through to the real terminal underneath.
///
/// mosh 1.4 exports nothing that identifies itself to the program it runs - `MOSH_KEY` is consumed
/// by mosh-client, and the `MOSH_SERVER_*` variables are read by mosh-server rather than exported -
/// so the process tree is the only reliable signal. Matched against Linux's `comm`, which truncates
/// at 15 bytes; `mosh-server` is 11, so it survives whole. Anything longer added here would not.
const OPAQUE_TRANSPORT_PROCESS_NAMES: [&str; 1] = ["mosh-server"];

/// The real chain is `yinz <- shell <- mosh-server`, depth two. The bound only exists so a
/// pathological or racing process tree cannot spin.
const MAX_ANCESTRY_DEPTH: usize = 32;

/// Whether this client is running under a transport that eats kitty graphics escape sequences.
pub fn host_swallows_kitty_graphics() -> bool {
    ancestry_contains_opaque_transport(std::process::id(), platform_lookup())
}

/// Walk from `start` towards pid 1, asking `lookup` for each pid's parent and name.
///
/// Split from the platform lookup so the walk itself is testable without spawning anything.
fn ancestry_contains_opaque_transport<F>(start: u32, lookup: F) -> bool
where
    F: Fn(u32) -> Option<(u32, String)>,
{
    let mut seen = Vec::with_capacity(MAX_ANCESTRY_DEPTH);
    let mut pid = start;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        // A pid we have already visited means the tree lied to us; stop rather than loop.
        if seen.contains(&pid) {
            return false;
        }
        seen.push(pid);
        let Some((parent, name)) = lookup(pid) else {
            // A process that vanished mid-walk, or a /proc we cannot read. Not knowing means
            // "not mosh": the probe is the backstop, and guessing the other way would cost every
            // ordinary terminal its images.
            return false;
        };
        if OPAQUE_TRANSPORT_PROCESS_NAMES.contains(&name.as_str()) {
            return true;
        }
        if parent == 0 || parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

#[cfg(target_os = "linux")]
fn platform_lookup() -> impl Fn(u32) -> Option<(u32, String)> {
    |pid| {
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some((parse_ppid_from_stat(&stat)?, name.trim().to_string()))
    }
}

/// The ppid out of a `/proc/<pid>/stat` line.
///
/// Field two is the executable name in parentheses and may itself contain spaces and parentheses,
/// so the fields cannot simply be split on whitespace. Everything after the *last* `)` is
/// unambiguous: state, then ppid.
#[cfg(target_os = "linux")]
fn parse_ppid_from_stat(stat: &str) -> Option<u32> {
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// One `ps` snapshot for the whole tree, rather than one call per level: a per-level `ps` would
/// fork up to [`MAX_ANCESTRY_DEPTH`] times on every client startup.
#[cfg(not(target_os = "linux"))]
fn platform_lookup() -> impl Fn(u32) -> Option<(u32, String)> {
    let table = process_table_from_ps();
    move |pid| table.get(&pid).cloned()
}

#[cfg(not(target_os = "linux"))]
fn process_table_from_ps() -> HashMap<u32, (u32, String)> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
    else {
        return HashMap::new();
    };
    parse_process_table(&String::from_utf8_lossy(&output.stdout))
}

/// `pid ppid comm` per line. BSD `ps` may print a path for `comm`, so take the basename.
#[allow(dead_code)]
fn parse_process_table(output: &str) -> HashMap<u32, (u32, String)> {
    let mut table = HashMap::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent), Some(command)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>()) else {
            continue;
        };
        let name = command.rsplit('/').next().unwrap_or(command).to_string();
        table.insert(pid, (parent, name));
    }
    table
}

#[cfg(test)]
#[path = "./unit/host_transport_tests.rs"]
mod tests;
