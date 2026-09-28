use super::*;

/// A `pid -> (ppid, comm)` reader over a fixed chain, youngest first.
fn chain<'a>(processes: &'a [(u32, u32, &'a str)]) -> impl Fn(u32) -> Option<(u32, String)> + 'a {
    move |pid| {
        processes
            .iter()
            .find(|(this, _, _)| *this == pid)
            .map(|(_, parent, name)| (*parent, name.to_string()))
    }
}

#[test]
fn a_client_under_mosh_is_detected_through_its_shell() {
    let processes = [
        (100, 99, "yinz"),
        (99, 98, "zsh"),
        (98, 1, "mosh-server"),
        (1, 0, "systemd"),
    ];
    assert!(ancestry_contains_opaque_transport(100, chain(&processes)));
}

#[test]
fn a_client_under_a_normal_terminal_is_not() {
    let processes = [
        (100, 99, "yinz"),
        (99, 98, "zsh"),
        (98, 1, "kitty"),
        (1, 0, "systemd"),
    ];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

/// ssh is a pipe, not an emulator: it passes the probe through to the real terminal, so it must
/// not be swept up with mosh.
#[test]
fn a_client_over_plain_ssh_is_not_detected() {
    let processes = [
        (100, 99, "yinz"),
        (99, 98, "bash"),
        (98, 1, "sshd"),
        (1, 0, "systemd"),
    ];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

#[test]
fn the_walk_stops_at_init() {
    let processes = [(100, 1, "yinz"), (1, 0, "systemd")];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

/// A process that vanished mid-walk, or a `/proc` we cannot read: not knowing means "not mosh",
/// because the startup probe is still there to catch it.
#[test]
fn a_chain_that_runs_out_is_not_a_match() {
    let processes = [(100, 99, "yinz")];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

#[test]
fn a_self_parenting_process_does_not_loop() {
    let processes = [(100, 100, "yinz")];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

#[test]
fn a_cycle_does_not_loop() {
    let processes = [(100, 101, "yinz"), (101, 100, "zsh")];
    assert!(!ancestry_contains_opaque_transport(100, chain(&processes)));
}

/// Deeper than `MAX_ANCESTRY_DEPTH`, with the match sitting past the bound: the walk gives up
/// rather than spinning, and gives up in the safe direction.
#[test]
fn the_walk_is_bounded() {
    let mut processes: Vec<(u32, u32, &str)> = (0..1000).map(|i| (i + 2, i + 3, "zsh")).collect();
    processes.push((1002, 1, "mosh-server"));
    assert!(!ancestry_contains_opaque_transport(2, chain(&processes)));
}

/// `mosh-server` is 11 bytes, inside the 15 Linux's `comm` keeps, so the name survives whole.
#[test]
fn the_matched_names_survive_linux_comm_truncation() {
    for name in OPAQUE_TRANSPORT_PROCESS_NAMES {
        assert!(name.len() <= 15, "{name} would be truncated in /proc/comm");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_process_name_with_spaces_and_parens_parses() {
    // Field two is `(comm)` and is not escaped, so splitting on whitespace would shift every
    // field after it. Everything after the last `)` is state, then ppid.
    let stat = "1234 (weird ) name) S 4321 1234 1234 0 -1 4194304 100 0 0 0 1 2 3 4";
    assert_eq!(parse_ppid_from_stat(stat), Some(4321));
}

#[cfg(target_os = "linux")]
#[test]
fn an_ordinary_stat_line_parses() {
    let stat = "1234 (yinz) S 4321 1234 1234 0 -1 4194304 100 0 0 0 1 2 3 4";
    assert_eq!(parse_ppid_from_stat(stat), Some(4321));
}

#[test]
fn a_ps_table_is_parsed_by_basename() {
    let table = parse_process_table("  100    99 /usr/bin/mosh-server\n   99     1 login\n");
    assert_eq!(table.get(&100), Some(&(99, "mosh-server".to_string())));
    assert_eq!(table.get(&99), Some(&(1, "login".to_string())));
}
