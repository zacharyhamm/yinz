// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
//! Bounded, condition-based waiting for tests.
//!
//! Tests that drive a real thread (a screen thread, a plugin thread) have to wait for that thread
//! to get somewhere before they assert. Sleeping for a fixed duration is the tempting way to do
//! that and it is wrong twice over: it is too slow when the machine is idle and too short when the
//! machine is loaded, and it never says what it was waiting *for*.
//!
//! [`Watched`] replaces the sleep with the condition itself. A test blocks until the shared state
//! satisfies a predicate, up to a deadline, and on expiry panics with a description of what it
//! wanted and what it actually saw.
//!
//! This is deliberately a leaf crate with no dependencies. `yinz-integration-tests` already has
//! good helpers of this shape, but it depends on `yinz-server`, so `yinz-server`'s own unit
//! tests cannot reach them.

use std::fmt;
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Environment variable overriding [`default_timeout`], in milliseconds.
pub const TIMEOUT_ENV_VAR: &str = "YINZ_TEST_WAIT_TIMEOUT_MS";

/// How long [`Watched::wait_until`] waits before declaring a test stuck.
///
/// Generous on purpose. These waits sit in front of real work — compiling and instantiating a wasm
/// plugin, starting a screen thread — so the point is only to bound the wait, not to police it. A
/// correct test returns as soon as its condition holds and never approaches this.
pub fn default_timeout() -> Duration {
    match std::env::var(TIMEOUT_ENV_VAR) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                panic!("{TIMEOUT_ENV_VAR} must be a whole number of milliseconds, got {raw:?}")
            },
        },
        Err(_) => Duration::from_secs(60),
    }
}

/// Re-run the current libtest in its own process, returning true in the parent.
/// Call before changing process-global environment or initializing environment-dependent statics.
pub fn reexec_test() -> bool {
    const CHILD: &str = "YINZ_ISOLATED_TEST";
    let thread = std::thread::current();
    let name = thread.name().expect("libtest thread must have a name");
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let output = Command::new(std::env::current_exe().expect("locate test executable"))
        .args(["--exact", name, "--include-ignored", "--nocapture"])
        .env(CHILD, name)
        .output()
        .expect("run isolated test");
    assert!(
        output.status.success(),
        "isolated test {name} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// Shared state a test can block on until it satisfies a predicate.
///
/// Clone it to share the same underlying state; every clone observes the same value and any
/// clone's write wakes every waiter.
pub struct Watched<T> {
    inner: Arc<(Mutex<T>, Condvar)>,
}

impl<T> Clone for Watched<T> {
    fn clone(&self) -> Self {
        Watched {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Default> Default for Watched<T> {
    fn default() -> Self {
        Watched::new(T::default())
    }
}

impl<T: fmt::Debug> fmt::Debug for Watched<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Watched").field(&*self.lock()).finish()
    }
}

impl<T> Watched<T> {
    pub fn new(value: T) -> Self {
        Watched {
            inner: Arc::new((Mutex::new(value), Condvar::new())),
        }
    }

    /// Locks the state for reading or writing. Waiters are woken when the guard drops.
    ///
    /// A poisoned mutex is recovered rather than propagated: the panic that poisoned it is the
    /// real failure and should surface from that thread's `join`, not as every later waiter
    /// reporting a poison it had nothing to do with.
    pub fn lock(&self) -> NotifyGuard<'_, T> {
        let (mutex, condvar) = &*self.inner;
        NotifyGuard {
            guard: Some(
                mutex
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ),
            condvar,
        }
    }

    /// Blocks until `predicate` returns `Some`, then yields that value.
    ///
    /// Panics on timeout. `what` completes the sentence "timed out waiting for ..." — describe the
    /// condition ("the plugin to load"), not the call site.
    pub fn wait_until<R>(
        &self,
        what: &str,
        timeout: Duration,
        predicate: impl Fn(&T) -> Option<R>,
    ) -> R {
        self.wait_until_described(what, timeout, predicate, |_| {
            String::from("<no description>")
        })
    }

    /// [`Watched::wait_until`], with a `describe` that renders the state into the timeout panic.
    ///
    /// Worth supplying whenever the state can say why it fell short — how many events arrived, what
    /// the last one was. A timeout is usually the first thing a reader sees about a broken test, so
    /// it should carry the evidence.
    pub fn wait_until_described<R>(
        &self,
        what: &str,
        timeout: Duration,
        predicate: impl Fn(&T) -> Option<R>,
        describe: impl Fn(&T) -> String,
    ) -> R {
        let (mutex, condvar) = &*self.inner;
        let started_at = Instant::now();
        let mut state = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        loop {
            if let Some(found) = predicate(&state) {
                return found;
            }
            let remaining = timeout.saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                panic!(
                    "timed out after {timeout:?} waiting for {what}.\nstate at timeout: {}",
                    describe(&state)
                );
            }
            let (next, outcome) = condvar
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            // Re-check before trusting the timeout flag: the value may have landed in the same
            // instant the wait expired.
            if outcome.timed_out() {
                if let Some(found) = predicate(&state) {
                    return found;
                }
                panic!(
                    "timed out after {timeout:?} waiting for {what}.\nstate at timeout: {}",
                    describe(&state)
                );
            }
        }
    }
}

impl<T: Clone> Watched<T> {
    /// A snapshot of the current state.
    pub fn get(&self) -> T {
        self.lock().clone()
    }
}

/// A [`Watched`] guard that wakes waiters when it drops.
///
/// Waking on drop rather than on write means a caller cannot mutate the state and forget to
/// announce it — the commonest way a condition-variable protocol goes wrong.
pub struct NotifyGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    condvar: &'a Condvar,
}

impl<T> Deref for NotifyGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard taken before drop")
    }
}

impl<T> DerefMut for NotifyGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard taken before drop")
    }
}

impl<T: fmt::Debug> fmt::Debug for NotifyGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> Drop for NotifyGuard<'_, T> {
    fn drop(&mut self) {
        // Release the lock before waking, so a woken thread does not immediately block on us.
        drop(self.guard.take());
        self.condvar.notify_all();
    }
}

/// Write an executable script for a test to run, from a child process rather than with
/// `fs::write`.
///
/// Under `cargo test` the tests of one crate share a process, and a child another test forks
/// in the meantime inherits every open descriptor until it execs - a script still open for
/// writing here then fails that exec with `Text file busy`. A writer in its own process leaves
/// nothing to inherit.
///
/// A test that runs the script against a deadline wants [`write_warm_script`] instead.
pub fn write_script(path: &Path, contents: &str) {
    let mut writer = Command::new("sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("failed to start the script writer");
    writer
        .stdin
        .take()
        .expect("the script writer has no stdin")
        .write_all(contents.as_bytes())
        .expect("failed to write the script");
    assert!(
        writer
            .wait()
            .expect("failed to wait for the script writer")
            .success(),
        "the script writer failed"
    );
}

/// Write a `/bin/sh` stub and pay its first-launch cost before the test needs it.
///
/// macOS evaluates a newly written executable the first time it runs, and under the suite's own
/// spawn storm - nextest gives every test a process, and a stub-driving test spawns `sh` twice -
/// that evaluation costs seconds: measured here, the first exec of a fresh stub took 2.4s while
/// every later exec of the same path took 5ms. A test that holds a child to a deadline has to
/// run it once first, or it is timing that check rather than the behaviour it means to.
///
/// `body` is the script without its `#!` line. The stub is written with a guard that makes a
/// `warmup` argument a no-op, so the warm run cannot log a request, touch a file or start a
/// background child the test would then see.
pub fn write_warm_script(path: &Path, body: &str) {
    write_script(
        path,
        &format!("#!/bin/sh\n[ \"$1\" = warmup ] && exit 0\n{body}\n"),
    );
    let status = Command::new(path)
        .arg("warmup")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed to warm the script");
    assert!(status.success(), "the script warmup run failed");
}

#[cfg(test)]
mod tests {
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
}
