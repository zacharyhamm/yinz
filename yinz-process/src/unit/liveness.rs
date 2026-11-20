use super::*;

#[test]
fn process_exit_observation_refuses_live_processes_and_invalid_pids() {
    assert!(!has_exited(std::process::id()).unwrap());
    assert!(has_exited(0).is_err());
    assert!(has_exited(1).is_err());
}

#[test]
fn process_exit_observation_recognizes_unreaped_children() {
    use std::time::{Duration, Instant};
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let exited = yinz_test_support::Watched::new(false);
    std::thread::scope(|scope| {
        let exited = &exited;
        let pid = child.id();
        scope.spawn(move || {
            let deadline = Instant::now() + yinz_test_support::default_timeout();
            while Instant::now() < deadline {
                if has_exited(pid).unwrap() {
                    *exited.lock() = true;
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(5));
            }
        });
        exited.wait_until(
            "unreaped child to be observed exited",
            yinz_test_support::default_timeout(),
            |exited| (*exited).then_some(()),
        );
    });
    assert!(child.wait().unwrap().success());
    assert!(has_exited(child.id()).unwrap());
}

#[test]
fn a_zombie_reaped_between_probes_is_observed_exited() {
    let probes = std::cell::Cell::new(0);
    let present = || {
        probes.set(probes.get() + 1);
        Ok(probes.get() == 1)
    };
    assert!(observe_exit(present, || None).unwrap());
    assert_eq!(probes.get(), 2);
}

#[test]
fn an_unreadable_process_that_remains_present_is_not_observed_exited() {
    assert!(!observe_exit(|| Ok(true), || None).unwrap());
    assert!(!observe_exit(|| Ok(true), || Some(false)).unwrap());
    assert!(observe_exit(|| Ok(true), || Some(true)).unwrap());
    assert!(observe_exit(|| Ok(false), || unreachable!()).unwrap());
}

#[test]
fn zombie_and_reaping_processes_are_terminated() {
    use sysinfo::ProcessStatus;
    assert!(is_terminated(ProcessStatus::Zombie));
    assert!(is_terminated(ProcessStatus::Dead));
    assert!(!is_terminated(ProcessStatus::Run));
    assert!(!is_terminated(ProcessStatus::Sleep));
    assert!(!is_terminated(ProcessStatus::UninterruptibleDiskSleep));
}
