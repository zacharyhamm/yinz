use super::*;
use nix::{
    sys::{
        signal::{killpg, Signal},
        wait::{waitpid, WaitPidFlag},
    },
    unistd::Pid,
};
use std::{
    io::{BufRead, Write},
    os::unix::{net::UnixListener, process::CommandExt},
    process::{Command, Stdio},
    sync::Mutex,
    time::Instant,
};

#[test]
fn fatal_exit_reaps_owned_forwards_with_inherited_stderr() {
    const TEST: &str =
        "remote_sessions::cleanup_tests::fatal_exit_reaps_owned_forwards_with_inherited_stderr";
    const ROLE: &str = "YINZ_TEST_FORWARD_ROLE";
    const ROOT: &str = "YINZ_TEST_FORWARD_ROOT";
    const SOCKET: &str = "YINZ_TEST_FORWARD_SOCKET";
    const WRITING: &str = "YINZ_TEST_FORWARD_WRITING";
    let role = std::env::var(ROLE).unwrap_or_default();
    if role == "writer" {
        let socket = PathBuf::from(std::env::var_os(SOCKET).unwrap());
        let hold = UnixListener::bind(socket.with_extension("writer")).unwrap();
        println!("writer ready");
        if std::env::var(WRITING).as_deref() == Ok("1") {
            while std::io::stderr().write_all(&[b'x'; 8192]).is_ok() {}
        } else {
            // Keep the inherited descriptor open after the fake SSH parent is killed.
            let _ = hold.accept();
        }
        return;
    }
    if role == "ssh" {
        let socket = PathBuf::from(std::env::var_os(SOCKET).unwrap());
        #[expect(
            clippy::zombie_processes,
            reason = "the fixture kills SSH before its writer; the outer process-group guard cleans up descendants"
        )]
        let mut writer = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(ROLE, "writer")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let ready = std::io::BufReader::new(writer.stdout.take().unwrap());
        assert!(ready
            .lines()
            .any(|line| line.unwrap().ends_with("writer ready")));
        std::fs::write(socket.with_extension("pid"), std::process::id().to_string()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        drop(listener.accept().unwrap());
        let _ = listener.accept();
        return;
    }
    if role == "owner" {
        static CHILDREN: Mutex<Vec<(Pid, PathBuf)>> = Mutex::new(Vec::new());
        extern "C" fn check_reaped() {
            if CHILDREN.lock().unwrap().iter().all(|(pid, socket)| {
                waitpid(*pid, Some(WaitPidFlag::WNOHANG)) == Err(nix::errno::Errno::ECHILD)
                    && !socket.exists()
            }) {
                println!("owned forwards reaped");
            }
        }
        // atexit runs in reverse registration order, after the production cleanup callback.
        assert_eq!(unsafe { nix::libc::atexit(check_reaped) }, 0);
        let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
        let start = |name: &str| {
            let socket = root.join(name).with_extension("sock");
            let env = yinz_types::envs::EnvironmentVariables::from_data(
                [
                    ("PATH".into(), root.to_str().unwrap().into()),
                    (ROLE.into(), "ssh".into()),
                    (SOCKET.into(), socket.to_str().unwrap().into()),
                ]
                .into(),
            );
            let forward = HostForward::start_until(
                "host",
                &Default::default(),
                &socket,
                "/remote/socket",
                None,
                Instant::now() + yinz_test_support::default_timeout(),
                &env,
            )
            .unwrap();
            let pid = std::fs::read_to_string(socket.with_extension("pid"))
                .unwrap()
                .parse()
                .unwrap();
            CHILDREN.lock().unwrap().push((Pid::from_raw(pid), socket));
            own_forward(forward)
        };
        set_owned_forward(Some(start("replaced")));
        set_owned_forward(Some(start("released")));
        assert!(!root.join("replaced.sock").exists());
        release_forward();
        assert!(!root.join("released.sock").exists());
        set_owned_forward(Some(start("concurrent-old")));
        let replacement = start("concurrent-new");
        let replacing = std::thread::spawn(move || set_owned_forward(Some(replacement)));
        cleanup_forwards();
        replacing.join().unwrap();
        assert!(!root.join("concurrent-old.sock").exists());
        assert!(!root.join("concurrent-new.sock").exists());
        set_owned_forward(Some(start("active")));
        let _prepared = start("prepared");
        std::process::exit(23);
    }

    struct Group(Pid);
    impl Drop for Group {
        fn drop(&mut self) {
            let _ = killpg(self.0, Signal::SIGKILL);
        }
    }
    for writing in ["0", "1"] {
        let dir = tempfile::tempdir().unwrap();
        let unrelated = dir.path().join("unrelated.sock");
        let _listener = UnixListener::bind(&unrelated).unwrap();
        yinz_test_support::write_warm_script(
            &dir.path().join("ssh"),
            &format!(
                "exec '{}' --exact {TEST} --nocapture\n",
                std::env::current_exe().unwrap().display(),
            ),
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(ROLE, "owner")
            .env(ROOT, dir.path())
            .env(WRITING, writing)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = Group(Pid::from_raw(child.id() as i32));
        let finished = yinz_test_support::Watched::new(None);
        let observed = finished.clone();
        let waiter = std::thread::spawn(move || {
            *observed.lock() = Some(child.wait_with_output().unwrap());
        });
        finished.wait_until(
            "fatal exit to release every owned SSH forward",
            yinz_test_support::default_timeout(),
            |output| output.as_ref().map(|_| ()),
        );
        waiter.join().unwrap();
        let output = finished.lock().take().unwrap();
        assert_eq!(output.status.code(), Some(23), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("owned forwards reaped"),
            "{output:?}"
        );
        assert!(unrelated.exists());
        drop(group);
    }
}
