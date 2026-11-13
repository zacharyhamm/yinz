use super::*;
use sysinfo::{ProcessesToUpdate, System};

#[test]
fn stat_starttime_splits_after_the_last_parenthesis() {
    let fields = |comm: &str| {
        format!("42 ({comm}) S 1 42 42 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 1000 200")
    };
    for comm in ["claude", "a b", "x) (y", ") S 1 2 3"] {
        assert_eq!(parse_stat_starttime(&fields(comm)), Some(987_654), "{comm}");
    }
    assert_eq!(parse_stat_starttime("42 (claude) S 1 42"), None);
    assert_eq!(parse_stat_starttime("no parenthesis"), None);
}

#[test]
fn btime_and_ticks_convert_to_unix_milliseconds() {
    assert_eq!(
        parse_btime("cpu  1 2 3\nbtime 1700000000\nprocesses 9\n"),
        Some(1_700_000_000)
    );
    assert_eq!(parse_btime("cpu  1 2 3\n"), None);
    assert_eq!(
        unix_ms_from_ticks(1_700_000_000, 12_345, 100),
        Some(1_700_000_123_450)
    );
    assert_eq!(unix_ms_from_ticks(1, 1, 0), None);
}

#[test]
fn the_start_token_does_not_depend_on_which_system_read_it() {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let token = |system: &mut System| {
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        start_token(system.process(pid).unwrap()).unwrap()
    };
    let first = token(&mut System::new());
    for _ in 0..3 {
        assert_eq!(token(&mut System::new()), first);
    }
}

#[test]
fn the_start_token_converts_to_about_the_sysinfo_start_time() {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let process = system.process(pid).unwrap();
    let started_ms = started_unix_ms(start_token(process).unwrap()).unwrap();
    // sysinfo truncates to seconds and reads its own boot time.
    let sysinfo_ms = process.start_time() * 1000;
    assert!(
        started_ms.abs_diff(sysinfo_ms) <= 2000,
        "{started_ms} vs {sysinfo_ms}"
    );
}
