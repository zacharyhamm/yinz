//! The managed launcher resolves in its final environment and supervises the provider.
use super::*;
use base64::Engine;
use nix::libc;
use yinz_types::agent::theme::{AgentThemeEvent, AgentThemeSnapshot, Availability, SourceColor};

fn provider_command(root: &Path, executable: &Path, kind: &str) -> Command {
    let mut cmd = command(root);
    cmd.env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("YINZ_AGENT_THEME_REF")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .env("YINZ_PANE_ID", "99")
        .args(["agent", kind, "--command"])
        .arg(executable)
        .args(["--", "argument preserved"]);
    cmd
}

fn terminal_output(command: &mut Command) -> Output {
    terminal_output_with_input(command, b"")
}

fn terminal_output_with_input(command: &mut Command, input: &[u8]) -> Output {
    use std::{io::Read, os::unix::process::CommandExt};
    let pty = nix::pty::openpty(None, None).unwrap();
    let mut termios = nix::sys::termios::tcgetattr(&pty.slave).unwrap();
    nix::sys::termios::cfmakeraw(&mut termios);
    nix::sys::termios::tcsetattr(&pty.slave, nix::sys::termios::SetArg::TCSANOW, &termios).unwrap();
    let slave = std::fs::File::from(pty.slave);
    // Bash uses stderr for job control, so it must share the test terminal.
    command
        .stdin(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap())
        .stdout(slave);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    // Close the Command's slave handles before waiting for PTY EOF.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut master = std::fs::File::from(pty.master);
    master.write_all(input).unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = vec![];
        let mut buffer = [0; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => panic!("PTY read: {e}"),
            }
        }
        bytes
    });
    let mut output = child.wait_with_output().unwrap();
    output.stdout = reader.join().unwrap();
    output
}

fn events(bytes: &[u8]) -> Vec<AgentThemeEvent> {
    String::from_utf8_lossy(bytes)
        .split("\x1b]777;yinz-agent-theme;")
        .skip(1)
        .map(|part| {
            serde_json::from_slice(
                &base64::engine::general_purpose::STANDARD
                    .decode(part.split("\x1b\\").next().unwrap())
                    .unwrap(),
            )
            .unwrap()
        })
        .collect()
}

fn launch(root: &Path, executable: &Path, target: &str) -> (AgentThemeSnapshot, Output) {
    let output = terminal_output(
        provider_command(root, executable, "claude").env("YINZ_BUBBLEWRAP", target),
    );
    let events = events(&output.stdout);
    assert_eq!(events.len(), 2, "{:?}", output);
    assert_eq!(events[0].launch_id, events[1].launch_id);
    assert!(events[1].snapshot.is_none());
    (events[0].snapshot.clone().unwrap(), output)
}

#[test]
fn managed_launches_snapshot_each_final_home_and_exec_after_resolution_failure() {
    let homes = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let executable = homes[0].path().join("claude");
    yinz_test_support::write_warm_script(
        &executable,
        r#"
if [ "$1" = --version ]; then printf '%s\n' '2.1.296 (Claude Code)'; exit; fi
printf '\033[32mprovider:%s\033[0m' "$1"
exit 42
"#,
    );
    for (index, theme) in ["dark", "light-ansi"].iter().enumerate() {
        std::fs::write(
            homes[index].path().join(".claude.json"),
            json!({"theme":theme}).to_string(),
        )
        .unwrap();
    }
    let (first, first_output) = launch(homes[0].path(), &executable, "0");
    let (second, _) = launch(homes[1].path(), &executable, "1");
    assert_eq!(first.theme, "dark");
    assert_eq!(second.theme, "light-ansi");
    assert_eq!(first_output.status.code(), Some(42));
    assert!(first_output
        .stdout
        .windows(b"\x1b[32mprovider:argument preserved\x1b[0m".len())
        .any(|bytes| bytes == b"\x1b[32mprovider:argument preserved\x1b[0m"));
    assert!(first
        .definitions
        .iter()
        .any(|d| d.token == "success" && matches!(d.color, SourceColor::Rgb(..))));
    std::fs::write(homes[0].path().join(".claude.json"), "{").unwrap();
    let (unavailable, output) = launch(homes[0].path(), &executable, "0");
    assert!(matches!(
        unavailable.availability,
        Availability::Unavailable(_)
    ));
    assert!(unavailable.definitions.is_empty());
    assert_eq!(output.status.code(), Some(42));
    assert!(output
        .stdout
        .windows(b"\x1b[32mprovider:argument preserved\x1b[0m".len())
        .any(|bytes| bytes == b"\x1b[32mprovider:argument preserved\x1b[0m"));
    assert_eq!(first.theme, "dark");
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "requires Bubblewrap user namespaces"]
fn real_bubblewrap_launch_reads_the_mounted_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    let inside = temp.path().join("inside");
    for (home, theme) in [(&outside, "dark"), (&inside, "light-ansi")] {
        std::fs::create_dir(home).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            json!({"theme":theme}).to_string(),
        )
        .unwrap();
    }
    let executable = temp.path().join("claude");
    yinz_test_support::write_warm_script(
        &executable,
        "if [ \"$1\" = --version ]; then printf '%s\\n' '2.1.296 (Claude Code)'; fi\n",
    );
    let output = terminal_output(
        Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "--bind"])
            .arg(temp.path())
            .arg(temp.path())
            .arg("--bind")
            .arg(&inside)
            .arg(&outside)
            .args(["--dev", "/dev", "--unshare-user", "--chdir"])
            .arg(&outside)
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_yinz"))
            .args(["agent", "claude", "--command"])
            .arg(&executable)
            .env("HOME", &outside)
            .env("TMPDIR", temp.path())
            .env("YINZ_SOCKET_DIR", temp.path())
            .env("YINZ_PANE_ID", "99")
            .env("YINZ_BUBBLEWRAP", "1")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("YINZ_PANE_REF")
            .env_remove("YINZ_REMOTE_PANE_TOKEN")
            .env_remove("YINZ_AGENT_THEME_REF")
            .stdin(Stdio::null()),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot = events(&output.stdout)[0].snapshot.clone().unwrap();
    assert!(
        !matches!(snapshot.availability, Availability::Unavailable(_)),
        "{:?}",
        snapshot.availability
    );
    assert_eq!(snapshot.theme, "light-ansi");
    assert_eq!(
        std::fs::read_to_string(outside.join(".claude.json")).unwrap(),
        r#"{"theme":"dark"}"#
    );
}

#[test]
fn pipes_and_redirected_files_receive_exact_json_without_palette_probes() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("provider");
    yinz_test_support::write_warm_script(
        &executable,
        r#"
if [ "$1" = --version ]; then touch "$HOME/probed"; exit 9; fi
printf '%s' '{"result":"argument preserved"}'
"#,
    );
    // Unavailable/malformed resolution must be entirely bypassed with nonterminal stdout.
    std::fs::write(root.path().join(".claude.json"), "{").unwrap();
    for kind in ["claude", "codex-untracked", "codex"] {
        let output = provider_command(root.path(), &executable, kind)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, br#"{"result":"argument preserved"}"#);
        let file = tempfile::tempfile().unwrap();
        let status = provider_command(root.path(), &executable, kind)
            .stdout(file.try_clone().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        use std::io::{Read, Seek};
        let mut file = file;
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, output.stdout);
        assert!(!root.path().join("probed").exists());
    }
}

#[test]
fn terminal_spawn_failures_and_signals_close_the_launch() {
    use std::os::unix::process::ExitStatusExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("missing");
    for kind in ["claude", "codex-untracked", "codex"] {
        let output = terminal_output(&mut provider_command(root.path(), &executable, kind));
        assert!(!output.status.success());
        let events = events(&output.stdout);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].launch_id, events[1].launch_id);
        assert!(events[1].snapshot.is_none());
    }
    yinz_test_support::write_warm_script(
        &executable,
        r#"
if [ "$1" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi
kill -TERM $$
"#,
    );
    for kind in ["claude", "codex-untracked", "codex"] {
        let output = terminal_output(&mut provider_command(root.path(), &executable, kind));
        assert_eq!(output.status.signal(), Some(libc::SIGTERM));
        let events = events(&output.stdout);
        assert_eq!(events.len(), 2);
        assert!(events[1].snapshot.is_none());
    }
}

#[test]
fn shell_launches_have_distinct_lifetimes_and_stop_resume_preserves_terminal_input() {
    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    yinz_test_support::write_warm_script(
        &provider,
        r#"
if [ "$1" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi
if [ "$STOP" = yes ]; then
    printf 'BEFORE_STOP'
    kill -STOP $$
    read -r answer
    printf 'AFTER_RESUME:%s' "$answer"
else
    printf 'PROVIDER'
fi
"#,
    );
    let mut shell = Command::new("/bin/bash");
    shell
        .env("HOME", root.path())
        .env("TMPDIR", root.path())
        .env("YINZ_SOCKET_DIR", root.path())
        .env("YINZ_PANE_ID", "99")
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_AGENT_THEME_REF")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("YINZ_LAUNCHER", env!("CARGO_BIN_EXE_yinz"))
        .env("PROVIDER", &provider)
        .current_dir(root.path());
    shell.args([
        "-c",
        r#"
"$YINZ_LAUNCHER" agent claude --command "$PROVIDER"
printf 'SHELL_BETWEEN'
"$YINZ_LAUNCHER" agent claude --command "$PROVIDER"
"#,
    ]);
    let output = terminal_output(&mut shell);
    assert!(output.status.success(), "{output:?}");
    let launches = events(&output.stdout);
    assert_eq!(launches.len(), 4);
    assert_ne!(launches[0].launch_id, launches[2].launch_id);
    assert_eq!(launches[0].launch_id, launches[1].launch_id);
    assert_eq!(launches[2].launch_id, launches[3].launch_id);
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("\x1b\\SHELL_BETWEEN\x1b]777;yinz-agent-theme;"));

    let mut shell = Command::new("/bin/bash");
    shell
        .env("HOME", root.path())
        .env("TMPDIR", root.path())
        .env("YINZ_SOCKET_DIR", root.path())
        .env("YINZ_PANE_ID", "99")
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_AGENT_THEME_REF")
        .env("YINZ_LAUNCHER", env!("CARGO_BIN_EXE_yinz"))
        .env("PROVIDER", &provider)
        .env("STOP", "yes")
        .current_dir(root.path())
        .args([
            "-ic",
            r#"
set -m
"$YINZ_LAUNCHER" agent claude --command "$PROVIDER" &
job=$!
wait "$job"
printf 'LAUNCHER_STOPPED'
fg
"#,
        ]);
    let output = terminal_output_with_input(&mut shell, b"terminal-input\n");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("BEFORE_STOP")
            && stdout.contains("LAUNCHER_STOPPED")
            && stdout.contains("AFTER_RESUME:terminal-input"),
        "{stdout} STDERR: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let launches = events(&output.stdout);
    assert_eq!(launches.len(), 2);
    assert!(launches[1].snapshot.is_none());
}

#[test]
fn signals_sent_to_launcher_reach_the_provider_and_end_the_palette() {
    use std::os::unix::process::ExitStatusExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("provider");
    yinz_test_support::write_warm_script(
        &executable,
        r#"
if [ "$1" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi
kill -TERM "$PPID"
while :; do read -r line; done
"#,
    );
    let output = terminal_output(&mut provider_command(root.path(), &executable, "claude"));
    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
    let launches = events(&output.stdout);
    assert_eq!(launches.len(), 2);
    assert_eq!(launches[0].launch_id, launches[1].launch_id);
    assert!(launches[1].snapshot.is_none());
}

#[test]
fn redirected_stdin_and_spawn_failure_restore_foreground_with_tostop() {
    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    yinz_test_support::write_warm_script(
        &provider,
        r#"
if [ "$1" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi
printf 'PROVIDER_FOREGROUND'
"#,
    );
    let mut shell = Command::new("/bin/bash");
    shell
        .env("HOME", root.path())
        .env("TMPDIR", root.path())
        .env("YINZ_SOCKET_DIR", root.path())
        .env("YINZ_PANE_ID", "99")
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_AGENT_THEME_REF")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("YINZ_LAUNCHER", env!("CARGO_BIN_EXE_yinz"))
        .env("PROVIDER", &provider)
        .current_dir(root.path())
        .args([
            "-ic",
            r#"
set -m
stty tostop
"$YINZ_LAUNCHER" agent claude --command "$PROVIDER" < /dev/null
"$YINZ_LAUNCHER" agent claude --command "$PROVIDER.missing"
printf 'SHELL_FOREGROUND'
"#,
        ]);
    let output = terminal_output(&mut shell);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PROVIDER_FOREGROUND"), "{output:?}");
    assert!(stdout.ends_with("SHELL_FOREGROUND"), "{output:?}");
    let launches = events(&output.stdout);
    assert_eq!(launches.len(), 4, "{output:?}");
    assert!(launches[1].snapshot.is_none());
    assert!(launches[3].snapshot.is_none());
}

#[test]
fn providers_keep_inherited_ignored_signals() {
    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    yinz_test_support::write_warm_script(
        &provider,
        r#"
if [ "$1" = --version ]; then echo '2.1.296 (Claude Code)'; exit; fi
kill -INT $$
printf 'IGNORED_INT'
"#,
    );
    for kind in ["claude", "codex-untracked", "codex"] {
        let mut shell = Command::new("/bin/sh");
        shell
            .env("HOME", root.path())
            .env("TMPDIR", root.path())
            .env("YINZ_SOCKET_DIR", root.path())
            .env("YINZ_PANE_ID", "99")
            .env_remove("YINZ_PANE_REF")
            .env_remove("YINZ_AGENT_THEME_REF")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("YINZ_LAUNCHER", env!("CARGO_BIN_EXE_yinz"))
            .env("PROVIDER", &provider)
            .env("KIND", kind)
            .current_dir(root.path())
            .args([
                "-c",
                r#"trap '' INT; exec "$YINZ_LAUNCHER" agent "$KIND" --command "$PROVIDER""#,
            ]);
        let output = terminal_output(&mut shell);
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("IGNORED_INT"),
            "{output:?}"
        );
        assert_eq!(events(&output.stdout).len(), 2, "{output:?}");
    }
}
