use std::io::prelude::*;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

use yinz_runtime::errors::prelude::*;

pub struct CopyCommand {
    command: String,
    args: Vec<String>,
}

impl CopyCommand {
    pub fn new(command: String) -> Self {
        let mut command_with_args = command.split(' ').map(String::from);

        Self {
            command: command_with_args.next().expect("missing command"),
            args: command_with_args.collect(),
        }
    }
    pub fn set(&self, value: String) -> Result<()> {
        let mut process = Command::new(self.command.clone())
            .args(self.args.clone())
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("couldn't spawn {}", self.command))?;
        let write_result = process
            .stdin
            .take()
            .context("could not get stdin")
            .and_then(|mut stdin| {
                stdin
                    .write_all(value.as_bytes())
                    .with_context(|| format!("couldn't write to {} stdin", self.command))
            });

        reap(process);

        write_result
    }
}

/// Wait for the copy command's process in the background, so that it does not linger as a zombie
/// once it exits.
///
/// The process is deliberately not killed if it outlives the copy operation: X11 and Wayland
/// clipboard helpers (eg. `xsel`, `xclip` or `wl-copy`) keep running for as long as they own the
/// selection, and killing them clears the clipboard. Copy commands that do exit by themselves (eg.
/// `pbcopy`) are reaped as soon as they do.
fn reap(mut process: Child) -> JoinHandle<()> {
    std::thread::spawn(move || {
        process.wait().context("Clipboard failure").non_fatal();
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nix::errno::Errno;
    use nix::sys::wait::{waitpid, WaitPidFlag};
    use nix::unistd::Pid;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn fake_clipboard_helper(dir: &Path, received: &Path, still_alive: &Path) -> String {
        let script = dir.join("fake-clipboard-helper");
        std::fs::write(
            &script,
            format!(
                "cat > '{}'\nsleep 2\ntouch '{}'\n",
                received.display(),
                still_alive.display()
            ),
        )
        .unwrap();
        format!("sh {}", script.display())
    }

    #[test]
    fn copy_command_is_not_killed_when_it_outlives_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let received = dir.path().join("received");
        let still_alive = dir.path().join("still-alive");
        let copy_command = fake_clipboard_helper(dir.path(), &received, &still_alive);

        CopyCommand::new(copy_command)
            .set("some copied text".to_owned())
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        while !still_alive.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(
            std::fs::read_to_string(&received).unwrap(),
            "some copied text",
        );
        assert!(still_alive.exists());
    }

    #[test]
    fn copy_command_is_reaped_once_it_exits() {
        let process = Command::new("sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = Pid::from_raw(process.id() as i32);

        reap(process).join().unwrap();

        assert_eq!(
            waitpid(pid, Some(WaitPidFlag::WNOHANG)).err(),
            Some(Errno::ECHILD),
        );
    }

    #[test]
    fn copy_command_is_reaped_after_a_failed_stdin_write() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("close-stdin");
        let pid_file = dir.path().join("pid");
        std::fs::write(&script, "echo $$ > \"$1\"\nexec 0<&-\nexit 0\n").unwrap();
        let command = CopyCommand {
            command: "sh".to_owned(),
            args: vec![script.display().to_string(), pid_file.display().to_string()],
        };

        // Larger than a pipe buffer: the helper closes its read end before this can finish.
        let error = command.set("x".repeat(4 * 1024 * 1024)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        let pid = Pid::from_raw(
            std::fs::read_to_string(pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while nix::sys::signal::kill(pid, None).is_ok() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(
            waitpid(pid, Some(WaitPidFlag::WNOHANG)).err(),
            Some(Errno::ECHILD),
            "even a failed clipboard write must reap its child"
        );
    }
}
