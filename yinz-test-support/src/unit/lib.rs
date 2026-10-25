use super::*;

const SHORT: Duration = Duration::from_millis(50);
const PATIENT: Duration = Duration::from_secs(30);

#[test]
fn reexec_isolates_environment() {
    let original = std::env::var_os("YINZ_ISOLATION_SENTINEL");
    if reexec_test() {
        assert_eq!(std::env::var_os("YINZ_ISOLATION_SENTINEL"), original);
        return;
    }
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("YINZ_ISOLATION_SENTINEL", "child-only") };
}

#[test]
fn returns_immediately_when_the_condition_already_holds() {
    let watched = Watched::new(vec![1, 2, 3]);
    let started = Instant::now();
    let found = watched.wait_until("three items", PATIENT, |v| (v.len() == 3).then_some(v[2]));
    assert_eq!(found, 3);
    assert!(started.elapsed() < SHORT, "should not have waited at all");
}

#[test]
fn wakes_when_another_thread_writes() {
    let watched: Watched<Vec<u32>> = Watched::default();
    let writer = watched.clone();
    let handle = std::thread::spawn(move || {
        for n in 0..5 {
            writer.lock().push(n);
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    let found = watched.wait_until("the fifth item", PATIENT, |v| v.get(4).copied());
    assert_eq!(found, 4);
    handle.join().unwrap();
}

#[test]
#[should_panic(expected = "timed out")]
fn panics_on_timeout() {
    let watched = Watched::new(0u32);
    watched.wait_until("a value that never arrives", SHORT, |v| {
        (*v == 1).then_some(())
    });
}

#[test]
fn the_timeout_panic_carries_the_description() {
    let watched = Watched::new(vec!["only-this"]);
    let panicked = std::panic::catch_unwind(|| {
        watched.wait_until_described(
            "two items",
            SHORT,
            |v| (v.len() == 2).then_some(()),
            |v| format!("{} item(s): {v:?}", v.len()),
        )
    })
    .unwrap_err();

    let message = panicked
        .downcast_ref::<String>()
        .expect("panic payload should be a String");
    assert!(message.contains("two items"), "{message}");
    assert!(message.contains("only-this"), "{message}");
}

#[test]
fn a_poisoned_lock_is_recovered_rather_than_propagated() {
    let watched = Watched::new(0u32);
    let poisoner = watched.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poisoning the mutex");
    })
    .join();

    // The panic above is the real failure and surfaces from that thread's join. Later waiters
    // should still see the state rather than a poison error.
    *watched.lock() = 7;
    assert_eq!(watched.get(), 7);
}

#[test]
fn writes_from_a_guard_are_visible_to_waiters() {
    let watched: Watched<Vec<u32>> = Watched::default();
    let writer = watched.clone();
    let handle = std::thread::spawn(move || {
        let mut guard = writer.lock();
        guard.push(1);
        guard.push(2);
        // Both pushes land before any waiter observes the state.
    });

    let len = watched.wait_until("a non-empty vec", PATIENT, |v| {
        (!v.is_empty()).then_some(v.len())
    });
    assert_eq!(len, 2, "a waiter should never observe a half-written guard");
    handle.join().unwrap();
}
