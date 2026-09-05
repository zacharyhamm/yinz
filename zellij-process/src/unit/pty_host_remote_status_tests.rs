//! The probe, against real processes on this machine.
//!
//! These run the same syscalls the broker runs, so they are as close to the real
//! thing as a unit test gets without a second machine.

use super::*;

use std::os::fd::AsRawFd;

/// A pty with a shell on it, so the probe has something real to look at.
struct Pane {
    pty: nix::pty::OpenptyResult,
    child: std::process::Child,
}

impl Pane {
    /// Run `script` under `sh` on a fresh pty, with the pane's process in its
    /// own process group and owning the terminal — exactly how a supervisor
    /// arranges it, which is what makes `tcgetpgrp` meaningful.
    fn new(script: &str) -> Self {
        use std::os::unix::process::CommandExt;
        let pty = nix::pty::openpty(None, None).expect("openpty failed");
        let slave_fd = pty.slave.as_raw_fd();
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        unsafe {
            cmd.pre_exec(move || {
                if libc::login_tty(slave_fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("spawn failed");
        Self { pty, child }
    }

    fn master_fd(&self) -> std::os::fd::RawFd {
        self.pty.master.as_raw_fd()
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Give the child time to reach the state the test is about.
    fn settle(&self) {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_pane_reports_its_own_command_and_directory() {
    let pane = Pane::new("cd /tmp && sleep 5");
    pane.settle();

    let probe = probe(pane.master_fd(), pane.pid());
    let shell = probe.shell_argv.expect("no shell argv");
    assert!(
        shell.iter().any(|a| a.contains("sh")),
        "expected the pane's own command, got {shell:?}"
    );
    // Resolved on both sides: `/tmp` is a symlink to `/private/tmp` on macOS and
    // the kernel answers with the physical path, so the literal never matches.
    let expected = std::fs::canonicalize("/tmp").expect("could not resolve /tmp");
    assert_eq!(
        probe.cwd.as_deref().map(std::path::Path::new),
        Some(expected.as_path()),
        "the cwd should follow the process, not the directory it was started from"
    );
}

/// The case agent detection depends on: something started *from* the pane's
/// shell has taken the terminal, and that is what should be reported as running.
#[test]
fn a_foreground_process_is_reported_separately_from_the_shell() {
    // `sh` execs nothing here — `sleep` runs as a child in the foreground.
    let pane = Pane::new("sleep 5 < /dev/tty");
    pane.settle();

    let probe = probe(pane.master_fd(), pane.pid());
    match probe.foreground_argv {
        Some(argv) => assert!(
            argv.iter().any(|a| a.contains("sleep")),
            "expected the foreground command, got {argv:?}"
        ),
        // Some shells exec the last command rather than forking, in which case
        // the pane's own process *is* `sleep` and there is no separate
        // foreground — which the shell argv then shows instead.
        None => {
            let shell = probe.shell_argv.expect("no argv at all");
            assert!(
                shell.iter().any(|a| a.contains("sleep")),
                "with no foreground process the pane's own command should be the \
                 one running, got {shell:?}"
            );
        },
    }
}

/// A pane whose own process owns the terminal has no *separate* foreground
/// command, and reporting one would just be the shell twice — which downstream
/// reads as "an agent is running" when nothing is.
#[test]
fn a_pane_with_nothing_in_the_foreground_reports_no_foreground_command() {
    let pane = Pane::new("exec sleep 5");
    pane.settle();

    let probe = probe(pane.master_fd(), pane.pid());
    assert_eq!(
        probe.foreground_argv, None,
        "the pane's own process must not be reported as a foreground command"
    );
    let shell = probe.shell_argv.expect("no shell argv");
    assert!(shell.iter().any(|a| a.contains("sleep")));
}

/// A dead process is the ordinary case during teardown; the probe must come
/// back empty rather than failing or inventing something.
#[test]
fn probing_a_process_that_is_gone_reports_nothing() {
    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    // `/usr/bin/true`, not `/bin/true`: the latter is a Linux-ism. macOS has no
    // `/bin/true` at all, and this spawn simply failed there.
    let mut child = std::process::Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn failed");
    let pid = child.id();
    let _ = child.wait();
    // Give the kernel a moment to reap it.
    std::thread::sleep(std::time::Duration::from_millis(100));

    let probe = probe(pty.master.as_raw_fd(), pid);
    assert_eq!(probe.cwd, None);
    assert_eq!(probe.shell_argv, None);
}

/// The report is what crosses the wire, so the mapping has to be exact — a
/// swapped pair here would silently show the shell as the running agent.
#[test]
fn a_probe_becomes_the_report_it_claims_to() {
    let probe = Probe {
        cwd: Some("/home/me/work".to_owned()),
        shell_argv: Some(vec!["/bin/zsh".to_owned()]),
        foreground_argv: Some(vec!["claude".to_owned()]),
    };
    match probe.into_report() {
        BrokerToServerMsg::StatusReport {
            cwd,
            shell_argv,
            foreground_argv,
        } => {
            assert_eq!(cwd.as_deref(), Some("/home/me/work"));
            assert_eq!(shell_argv, Some(vec!["/bin/zsh".to_owned()]));
            assert_eq!(foreground_argv, Some(vec!["claude".to_owned()]));
        },
        other => panic!("expected a status report, got {other:?}"),
    }
}

// ---- the git probe ----

/// The probe runs the same two invocations `hocus-agentd` does, and the parser
/// on the other end is that plugin's. What matters is that the raw bytes are the
/// ones that parser expects, so probe this very repository.
#[test]
fn a_checkout_reports_its_identity() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let git = probe_git(repo);

    let metadata = String::from_utf8_lossy(&git.metadata);
    let lines: Vec<&str> = metadata.lines().collect();
    assert!(
        lines.len() >= 3,
        "expected at least root, git-dir and common-dir, got {lines:?}"
    );
    assert!(
        std::path::Path::new(lines[0]).is_absolute(),
        "--path-format=absolute must yield an absolute root, got {:?}",
        lines[0]
    );
    assert_eq!(git.cwd, repo);
}

/// A directory that is not a checkout is the ordinary case for a pane, and must
/// come back as a clean answer rather than an error. Git exits 128 there, which
/// the plugin's parser reads as "not a repository" — distinct from a genuine
/// failure — so the code has to survive the trip.
#[test]
fn a_directory_outside_any_checkout_is_not_a_failure() {
    let git = probe_git("/");
    assert!(
        git.metadata.is_empty() || git.exit_code == Some(128),
        "expected no identity for a non-repository, got {:?} (exit {:?})",
        String::from_utf8_lossy(&git.metadata),
        git.exit_code
    );
}

/// A cwd that no longer exists happens whenever a directory is removed under a
/// running pane. It must not panic or hang.
#[test]
fn probing_a_directory_that_is_gone_reports_nothing_useful() {
    let git = probe_git("/definitely/not/a/real/directory");
    assert!(git.metadata.is_empty());
    assert_eq!(git.cwd, "/definitely/not/a/real/directory");
}

/// The premise the pump's "send only on difference" rests on: a folder nobody
/// has touched answers identically every time. Were the probe unstable — a
/// timestamp, an ordering that drifted — an idle pane would send a frame a
/// minute for the life of the session, which is exactly what the pump was built
/// to avoid.
#[test]
fn probing_an_unchanged_checkout_twice_answers_the_same() {
    let repo = env!("CARGO_MANIFEST_DIR");
    assert_eq!(probe_git(repo), probe_git(repo));
}
