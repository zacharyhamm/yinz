use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const TEMP_PARENT: &str = "/tmp/yinz-test";
/// The hostname every test server reports, through `YINZ_HOSTNAME`.
pub const TEST_HOSTNAME: &str = "Yinz";
const TEST_ROOT_SUBDIRS: [&str; 8] = [
    "sock", "home", "cache", "data", "config", "runtime", "cwd", "tmp",
];

static TEST_ROOT: OnceLock<PathBuf> = OnceLock::new();
static OWN_PROCESS_LOCK: OnceLock<File> = OnceLock::new();
static SESSION_COUNTER: AtomicUsize = AtomicUsize::new(0);
static BACKGROUND_PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn init() -> &'static Path {
    TEST_ROOT
        .get_or_init(|| {
            let test_root = create_test_root();
            isolate_process_environment(&test_root);
            record_background_thread_panics();
            yinz_runtime::logging::configure_logger();
            test_root
        })
        .as_path()
}

/// The server threads are started with yinz's own panic hook disabled (it would take the test
/// process with it), so a `screen` or `pty` thread that dies leaves the session frozen and the
/// test that was driving it fails as a bare `wait_until` timeout with no hint of why. The default
/// hook still prints the panic, but under a runner that captures per-test output it lands in
/// whichever test happened to be capturing - not necessarily the one that then timed out.
///
/// Keep our own copy, named by thread, so every timeout message can say what died and a clean
/// server shutdown cannot hide a worker's discarded join error. Nextest runs one test per
/// process. Under `cargo test`, a panic can also fail a sibling session's shutdown; that suite
/// must fail either way, and the recorded thread and panic identify the original failure.
fn record_background_thread_panics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>").to_string();
        if let Ok(mut background_panics) = BACKGROUND_PANICS.lock() {
            // `panic_info` renders as "panicked at <location>:\n<message>"
            background_panics.push(format!("thread '{}' {}", name, panic_info));
        }
        default_hook(panic_info);
    }));
}

pub(crate) fn assert_no_background_panics() {
    let background_panics = BACKGROUND_PANICS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if !background_panics.is_empty() {
        let summary = format!(
            "background thread panicked during integration test:\n{}",
            background_panics.join("\n")
        );
        // Print explicitly and bypass the hook so this summary is not recorded again.
        eprintln!("{summary}");
        std::panic::resume_unwind(Box::new(summary));
    }
}

/// Every panic recorded so far, formatted for a timeout message. Empty when nothing has panicked,
/// so it costs a passing run nothing to include. A harness timeout reads this before raising its
/// own panic, so it never contains the timeout being reported.
pub fn panics_so_far() -> String {
    let background_panics = match BACKGROUND_PANICS.lock() {
        Ok(background_panics) => background_panics,
        Err(poisoned) => poisoned.into_inner(),
    };
    if background_panics.is_empty() {
        return String::new();
    }
    format!(
        "\n=== threads that panicked before this timeout (a dead server thread leaves the session \
         frozen, and every wait after it times out) ===\n{}\n",
        background_panics.join("\n")
    )
}

fn create_test_root() -> PathBuf {
    let run_root = std::env::var_os("YINZ_TEST_CACHE_ROOT");
    let parent = run_root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMP_PARENT));
    std::fs::create_dir_all(&parent).unwrap();
    if run_root.is_none() {
        hold_own_process_lock(&parent);
        remove_roots_of_dead_test_processes(&parent);
    }
    let test_root = parent.join(std::process::id().to_string());
    for subdir in TEST_ROOT_SUBDIRS {
        std::fs::create_dir_all(test_root.join(subdir)).unwrap();
    }
    test_root
}

fn process_lock_path(parent: &Path, pid: u32) -> PathBuf {
    parent.join(format!("{}.lock", pid))
}

fn hold_own_process_lock(parent: &Path) {
    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(process_lock_path(parent, std::process::id()))
        .unwrap();
    lock_file.lock().unwrap();
    let _ = OWN_PROCESS_LOCK.set(lock_file);
}

fn remove_roots_of_dead_test_processes(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(owner_pid) = file_name
            .to_str()
            .and_then(|name| name.strip_suffix(".lock"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if owner_pid == std::process::id() {
            continue;
        }
        let Ok(lock_file) = OpenOptions::new().write(true).open(entry.path()) else {
            continue;
        };
        if lock_file.try_lock().is_ok() {
            let _ = std::fs::remove_dir_all(parent.join(owner_pid.to_string()));
            let _ = std::fs::remove_file(entry.path());
            let _ = lock_file.unlock();
        }
    }
}

fn isolate_process_environment(test_root: &Path) {
    std::env::set_var("TMPDIR", test_root.join("tmp"));
    std::env::set_var("YINZ_SOCKET_DIR", test_root.join("sock"));
    std::env::set_var("HOME", test_root.join("home"));
    std::env::set_var("XDG_CACHE_HOME", test_root.join("cache"));
    std::env::set_var("XDG_DATA_HOME", test_root.join("data"));
    std::env::set_var("XDG_CONFIG_HOME", test_root.join("config"));
    std::env::set_var("XDG_RUNTIME_DIR", test_root.join("runtime"));
    // The tab bar and the terminal title print the machine's hostname. Without this every
    // snapshot of them would only match on the machine that recorded it.
    std::env::set_var("YINZ_HOSTNAME", TEST_HOSTNAME);
    // Fake hosts need an identity, not a fresh durable file before every startup deadline.
    std::env::set_var("YINZ_HOST_ID", "yinz-integration-test");
    std::env::remove_var("YINZ");
    std::env::remove_var("YINZ_SESSION_NAME");
    // An outer yinz pane would otherwise make CLI requests claim that pane as their caller
    // once the in-process server sets YINZ_SESSION_NAME to the test session.
    std::env::remove_var("YINZ_PANE_ID");
    std::env::remove_var("YINZ_PANE_REF");
    std::env::remove_var("YINZ_AGENT_PANE");
    std::env::remove_var("YINZ_CONFIG_FILE");
    std::env::remove_var("YINZ_CONFIG_DIR");
    std::env::set_current_dir(test_root.join("cwd")).unwrap();
}

/// The checked-in Scheme fixture stays outside the production builtin source tree.
pub fn fixture_plugin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../yinz-steel-builtins/fixtures/fixture-plugin-for-tests.scm")
}

pub fn unique_session_name() -> String {
    let session_index = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("test-{}", session_index)
}

pub struct ConcurrencySlot {
    _slot_file: File,
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn slot_count() -> usize {
    if std::env::var("YINZ_TEST_SERIAL")
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
    {
        return 1;
    }
    if let Some(explicit) = std::env::var("YINZ_TEST_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return explicit.max(1);
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cpus / 4).max(1)
}

pub fn acquire_concurrency_slot() -> Option<ConcurrencySlot> {
    // Nextest admits tests before starting their deadlines. A second, unfair gate here
    // can starve tests that start several sessions while their test timeout keeps running.
    // Keep explicit harness limits and the fallback for bare cargo test.
    if std::env::var_os("NEXTEST").is_some()
        && std::env::var_os("YINZ_TEST_CONCURRENCY").is_none()
        && std::env::var_os("YINZ_TEST_SERIAL").is_none()
    {
        return None;
    }
    let slots = slot_count();
    let slot_dir = std::env::var_os("YINZ_TEST_CACHE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMP_PARENT))
        .join("concurrency-slots");
    std::fs::create_dir_all(&slot_dir).unwrap();
    loop {
        for slot_index in 0..slots {
            let slot_file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(slot_dir.join(format!("slot-{}", slot_index)))
                .unwrap();
            if slot_file.try_lock().is_ok() {
                return Some(ConcurrencySlot {
                    _slot_file: slot_file,
                });
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// The fake default shell has unknown argv. Tests of process discovery supply an
// explicit command, so unrelated snapshots do not depend on the host's SHELL or
// on whether the command-discovery tick ran before the snapshot.
pub const DEFAULT_TEST_CONFIG: &str = r#"
default_shell ""
session_serialization false
disable_session_metadata true
visual_bell false
mouse_mode false
advanced_mouse_actions false
pane_frame_style "titles"
theme "default"
"#;

pub fn write_config(session_name: &str, extra_config_kdl: &str) -> PathBuf {
    let test_root = init();
    let config_path = test_root
        .join("config")
        .join(format!("{}-config.kdl", session_name));
    let contents = format!(
        "{}\n{}\n",
        defaults_overridden_by(extra_config_kdl),
        extra_config_kdl
    );
    std::fs::write(&config_path, contents).unwrap();
    config_path
}

/// A real Git checkout on a branch, with one commit, for a pane to sit in.
///
/// Real because the daemon's probe runs a real `git`: it goes out through
/// `BackgroundJob::RunCommand`, which is one of the few things the fake server
/// does not stand in for. `HOME` is the test root's by then, so there is no user
/// config to trip over.
///
/// Under `target/` rather than under the test root, which is the one thing here
/// that is not about Git: a plugin's `run_command` has its cwd put through
/// `translate_plugin_path`, which rewrites anything under `/tmp` into the
/// plugin's own sandboxed tmp folder. The whole test root is under `/tmp`, so a
/// checkout there would be probed at a path that does not exist.
///
/// The commit matters. `git rev-parse --abbrev-ref HEAD` cannot resolve an
/// unborn branch, so a repository with none reports no branch at all.
pub fn write_git_repository(name: &str, branch: &str) -> PathBuf {
    init();
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/test-git"))
        .join(std::process::id().to_string())
        .join(name);
    // A rerun in the same process must not inherit the last one's history.
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap_or_else(|error| panic!("could not run git {args:?}: {error}"));
        assert!(status.success(), "git {args:?} failed in {root:?}");
    };
    git(&["init", "-b", branch]);
    git(&[
        "-c",
        "user.email=test@example.com",
        "-c",
        "user.name=test",
        "commit",
        "--allow-empty",
        "-m",
        "initial",
    ]);
    root
}

/// A linked worktree of `primary`, checked out on a new branch, nested inside
/// the checkout it came from.
///
/// Nested deliberately, at `.claude/worktrees/<name>` — that is where this
/// repository keeps its own, and it is the layout that used to give an agent the
/// wrong branch. A worktree beside its primary is told apart by its path alone;
/// one inside it looks, to anything holding only the list of probed folders,
/// exactly like a subdirectory of the primary.
///
/// `write_git_repository` has already made the commit that `git worktree add`
/// needs — it cannot branch from an unborn HEAD.
pub fn write_git_worktree(primary: &std::path::Path, name: &str, branch: &str) -> PathBuf {
    let path = primary.join(".claude/worktrees").join(name);
    let status = std::process::Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(&path)
        .current_dir(primary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|error| panic!("could not run git worktree add: {error}"));
    assert!(status.success(), "git worktree add failed in {primary:?}");
    path
}

fn defaults_overridden_by(extra_config_kdl: &str) -> String {
    let overridden_keys: std::collections::HashSet<&str> = extra_config_kdl
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    DEFAULT_TEST_CONFIG
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_none_or(|key| !overridden_keys.contains(key))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn log_file_path() -> PathBuf {
    init();
    yinz_runtime::consts::YINZ_TMP_LOG_FILE.clone()
}

pub fn log_tail(max_lines: usize) -> String {
    let contents = std::fs::read_to_string(log_file_path()).unwrap_or_default();
    startup_and_tail(&contents, max_lines)
}

fn startup_and_tail(contents: &str, max_lines: usize) -> String {
    let startup = contents
        .lines()
        .filter(|line| {
            line.contains("startup:")
                || line.contains("FirstClientConnected")
                || line.contains("attach trace:")
        })
        .take(256)
        .collect::<Vec<_>>()
        .join("\n");
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    format!(
        "=== startup trace (first 256 events) ===\n{startup}\n=== recent events ===\n{}",
        lines[start..].join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_host_identity_does_not_wait_for_persistent_identity_io() {
        let data_dir = init().join("data");
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(data_dir.join(".host-id.lock"))
            .unwrap();
        lock.lock().unwrap();
        let (completed, completion) = std::sync::mpsc::channel();
        let identity_dir = data_dir.clone();
        let worker = std::thread::spawn(move || {
            completed
                .send(yinz_runtime::host_identity::init_host_id(
                    &identity_dir,
                    None,
                ))
                .unwrap();
        });
        let identity = completion.recv_timeout(yinz_test_support::default_timeout());
        drop(lock);
        worker.join().unwrap();
        assert_eq!(identity.unwrap().as_str(), "yinz-integration-test");
        assert!(!data_dir.join("host-id").exists());
    }

    #[test]
    fn startup_evidence_survives_the_recent_log_tail() {
        let evidence = startup_and_tail("startup: opening database\nnoise\nlast", 1);
        assert!(evidence.contains("startup: opening database"));
        assert!(evidence.ends_with("last"));
        assert!(!evidence.contains("noise"));
    }

    #[test]
    fn shutdown_panic_summaries_do_not_grow_recorded_history() {
        const PROBE_ENV: &str = "YINZ_TEST_PANIC_HISTORY_PROBE";
        if std::env::var_os(PROBE_ENV).is_some() {
            record_background_thread_panics();
            assert_no_background_panics();
            let worker = std::thread::Builder::new()
                .name("history_probe_worker".into())
                .spawn(|| panic!("original worker failure"))
                .unwrap();
            assert!(worker.join().is_err());

            let original = BACKGROUND_PANICS.lock().unwrap().clone();
            assert_eq!(original.len(), 1);
            assert!(original[0].contains("history_probe_worker"));
            assert!(original[0].contains("original worker failure"));
            let summary = format!(
                "background thread panicked during integration test:\n{}",
                original.join("\n")
            );
            for _ in 0..35 {
                let panic = std::panic::catch_unwind(assert_no_background_panics).unwrap_err();
                assert_eq!(*panic.downcast::<String>().unwrap(), summary);
                let recorded = BACKGROUND_PANICS.lock().unwrap().clone();
                assert_eq!(recorded, original);
            }
            return;
        }

        // A separate process keeps the deliberate worker panic out of sibling tests' history.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "test_env::tests::shutdown_panic_summaries_do_not_grow_recorded_history",
                "--nocapture",
            ])
            .env(PROBE_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "panic history probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
