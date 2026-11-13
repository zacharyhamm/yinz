//! Exact process start identity that every process on the host computes identically.
//!
//! sysinfo's Linux `start_time()` adds a boot time that each `System` reads anew. The kernel
//! truncates that boot time to whole seconds and moves it with clock adjustments, so separate
//! processes can disagree about one process's start by a second or more. Hooks run in fresh
//! reporter processes and observers build their own Systems, so identity keys must not use it.

/// An exact start token for `process`: equal for one incarnation everywhere on the host and
/// increasing across incarnations within one boot. It is not a timestamp; convert it with
/// [`started_unix_ms`].
///
/// Linux uses `/proc/<pid>/stat` `starttime` (clock ticks since boot). macOS's
/// `start_time()` already comes from the kernel's absolute start and is stable.
pub fn start_token(process: &sysinfo::Process) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", process.pid())).ok()?;
        parse_stat_starttime(&stat)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(process.start_time())
    }
}

/// The approximate Unix time, in milliseconds, at which the process with `token` started.
/// The boot time it uses can be off by a second, so callers must allow for that.
pub fn started_unix_ms(token: u64) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        // SAFETY: sysconf has no preconditions and only reads a configuration value.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        unix_ms_from_ticks(parse_btime(&stat)?, token, u64::try_from(ticks).ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        token.checked_mul(1000)
    }
}

#[cfg(any(target_os = "linux", test))]
/// Field 22 of a `/proc/<pid>/stat` line. Field two is the executable name in parentheses,
/// which may itself contain spaces and parentheses, so split only after the *last* `)`;
/// the state (field three) follows it.
fn parse_stat_starttime(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(22 - 3)?
        .parse()
        .ok()
}

#[cfg(any(target_os = "linux", test))]
/// The `btime` line of `/proc/stat`: boot time in Unix seconds.
fn parse_btime(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

#[cfg(any(target_os = "linux", test))]
fn unix_ms_from_ticks(btime: u64, ticks: u64, ticks_per_second: u64) -> Option<u64> {
    if ticks_per_second == 0 {
        return None;
    }
    btime
        .checked_mul(1000)?
        .checked_add(ticks.checked_mul(1000)? / ticks_per_second)
}

#[cfg(test)]
#[path = "unit/process_start_tests.rs"]
mod tests;
