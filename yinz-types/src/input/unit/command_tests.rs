use super::*;

use std::path::Path;

#[test]
fn a_bubblewrap_target_is_on_this_machine_and_resolves_off_the_pty_thread() {
    assert!(PaneTarget::bubblewrap(Path::new("relative/repo")).is_err());
    let target = PaneTarget::bubblewrap(Path::new("/repo")).unwrap();
    assert!(!target.is_local() && !target.is_remote() && target.is_bubblewrap());
    assert_eq!(target.bubblewrap_root(), Some(Path::new("/repo")));
    // Not a host: nothing that groups panes by machine sees a sandbox as one.
    assert_eq!(target.display(), None);

    let command = RunCommand {
        command: PathBuf::from("claude"),
        target: target.clone(),
        ..Default::default()
    };
    assert!(command.defers_spawn());
    assert!(command.shows_start_failure());
    assert!(!command.is_remote_default_shell());
    let held = RunCommand {
        hold_on_start: true,
        ..command
    };
    assert!(!held.defers_spawn());
    assert!(!RunCommand::default().defers_spawn());

    let mut inherited = PaneTarget::Local;
    inherited.merge_from(&target);
    assert_eq!(inherited, target);
}
