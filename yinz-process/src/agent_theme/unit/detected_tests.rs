use super::*;
use std::{
    io::Read,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    time::Duration,
};
use yinz_types::agent::theme::{AgentThemeSnapshot, Availability};

struct Fixture {
    child: Child,
    _pty: nix::pty::OpenptyResult,
    _root: tempfile::TempDir,
}
impl Fixture {
    fn new(redirected: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("claude");
        yinz_test_support::write_warm_script(&script,
            "if [ \"$1\" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi\nprintf ready\nread -r hold");
        std::fs::write(root.path().join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
        std::fs::write(root.path().join("settings.json"), r#"{"theme":"light"}"#).unwrap();
        let pty = nix::pty::openpty(None, None).unwrap();
        // Model a recognized interpreter/script invocation without depending on
        // a particular interpreter installation. The shell keeps this argv intact.
        let mut command = Command::new("/bin/sh");
        command
            .arg0("python3")
            .arg(&script)
            .args(["--settings", "settings.json"])
            .env_clear()
            .env("HOME", root.path())
            .current_dir(root.path())
            .stdin(Stdio::piped())
            .stdout(if redirected {
                Stdio::piped()
            } else {
                Stdio::from(pty.slave.try_clone().unwrap())
            });
        let mut child = command.spawn().unwrap();
        let ready = yinz_test_support::Watched::new(false);
        let observed = ready.clone();
        let mut output: Box<dyn Read + Send> = if redirected {
            Box::new(child.stdout.take().unwrap())
        } else {
            Box::new(std::fs::File::from(pty.master.try_clone().unwrap()))
        };
        std::thread::spawn(move || {
            let mut bytes = [0; 5];
            output.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes, b"ready");
            *observed.lock() = true;
        });
        ready.wait_until(
            "fixture is waiting for input",
            Duration::from_secs(20),
            |ready| ready.then_some(()),
        );
        Self {
            child,
            _pty: pty,
            _root: root,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn resolved(detector: &mut Detector) -> AgentThemeSnapshot {
    let snapshot = detector
        .pending
        .as_ref()
        .unwrap()
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    // Feed the completed worker back through poll's result delivery.
    let (sender, receiver) = mpsc::channel();
    sender.send(Some(snapshot.clone())).unwrap();
    detector.pending = Some(receiver);
    snapshot
}

#[test]
fn execution_host_context_and_launch_identity_survive_idle_polling() {
    let fixture = Fixture::new(false);
    let mut detector = Detector::default();
    let state = detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Remote)
        .unwrap();
    assert!(state.snapshot.is_none());
    let snapshot = resolved(&mut detector);
    assert!(
        !matches!(snapshot.availability, Availability::Unavailable(_)),
        "{:?}",
        snapshot.availability
    );
    assert_eq!(
        snapshot.theme, "light",
        "argv settings resolve relative to the child's cwd and HOME"
    );
    let next = detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Remote)
        .unwrap();
    assert_eq!(next.launch_id, state.launch_id);
    assert_eq!(next.snapshot, Some(snapshot));
    assert!(
        detector.pending.is_none(),
        "unchanged observations never repeat probes"
    );
    assert!(detector.poll(None, ExecutionTarget::Remote).is_none());
    let resumed = detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Remote)
        .unwrap();
    assert_ne!(resumed.launch_id, state.launch_id);
}

#[test]
fn ending_an_interval_discards_its_pending_result() {
    let fixture = Fixture::new(false);
    let mut detector = Detector::default();
    detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Local)
        .unwrap();
    let _ = resolved(&mut detector);
    assert!(detector.poll(None, ExecutionTarget::Local).is_none());
    assert!(detector.pending.is_none());
    assert!(detector.state.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn redirected_agents_have_no_palette_interval_or_probes() {
    let fixture = Fixture::new(true);
    let mut detector = Detector::default();
    assert!(detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Local)
        .is_none());
    assert!(detector.pending.is_none());
}

#[test]
fn adoption_preserves_a_resolved_palette_only_for_the_same_process() {
    let fixture = Fixture::new(false);
    let mut detector = Detector::default();
    let initial = detector
        .poll(Some(fixture.child.id()), ExecutionTarget::Local)
        .unwrap();
    let snapshot = resolved(&mut detector);
    let mut adopted = Detector::default();
    adopted.adopt(yinz_types::agent::theme::AgentThemeLaunch {
        launch_id: initial.launch_id.clone(),
        snapshot: snapshot.clone(),
    });
    let current = adopted
        .poll(Some(fixture.child.id()), ExecutionTarget::Local)
        .unwrap();
    assert_eq!(current.launch_id, initial.launch_id);
    assert_eq!(current.snapshot, Some(snapshot.clone()));
    assert!(adopted.pending.is_none());
    let mut stale = Detector::default();
    stale.adopt(yinz_types::agent::theme::AgentThemeLaunch {
        launch_id: "detected:another-process".into(),
        snapshot,
    });
    assert_ne!(
        stale
            .poll(Some(fixture.child.id()), ExecutionTarget::Local)
            .unwrap()
            .launch_id,
        "detected:another-process"
    );
}

#[test]
fn adoption_continues_a_pending_interval_with_a_fresh_worker() {
    let fixture = Fixture::new(false);
    let mut original = Detector::default();
    let pending = original
        .poll(Some(fixture.child.id()), ExecutionTarget::Local)
        .unwrap();
    let mut snapshot = resolved(&mut original);
    snapshot.provider_version.clear();
    snapshot.availability =
        Availability::Unavailable(yinz_types::agent::theme::DETECTED_THEME_PENDING.into());
    let mut adopted = Detector::default();
    adopted.adopt(yinz_types::agent::theme::AgentThemeLaunch {
        launch_id: pending.launch_id.clone(),
        snapshot,
    });
    assert_eq!(
        adopted
            .poll(Some(fixture.child.id()), ExecutionTarget::Local)
            .unwrap()
            .launch_id,
        pending.launch_id
    );
    assert_eq!(resolved(&mut adopted).theme, "light");
}
