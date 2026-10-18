//! The remote-pane tests: a real sshd, in a container, over a cuttable link.
//!
//! Everything under `yinz-process/src/pty_host/remote/` that needs another
//! machine is verified here and nowhere else. The unit tests stop at a
//! `UnixStream::pair()`; this is where an actual `ssh` runs, an actual broker
//! daemonises on the far side, and an actual pty is allocated over there.
//!
//! Three phases, mirroring `ci e2e`:
//!
//!   * `--build` — Debian GNU binary, generated keypair and ssh config, containers up
//!     (`--build --native` uses a native debug binary on Linux)
//!   * `--test`  — the `#[ignore]`d tests in `yinz-server/tests/`
//!   * `--clean` — containers down, generated credentials removed
//!
//! The tests are `#[ignore]`d, so `cargo x test` and `cargo x integration-test`
//! never touch them and never need docker.

use crate::flags;
use anyhow::Context;
use std::ffi::OsString;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use xshell::{cmd, Shell};

/// Kept in step with `remote_pane_ssh.rs`, which reads the same paths. The test
/// binary cannot be handed arguments, so the two sides agree on a location
/// instead.
const COMPOSE_FILE: &str = "docker-compose.remote-panes.yml";
/// A project name of our own, rather than the directory basename compose would
/// otherwise pick.
///
/// Both compose files here are invoked from the repository root, so without this
/// they land in one project named `yinz` — and each file's containers are
/// orphans of the other. `clean` runs `down --remove-orphans`, which would then
/// take the e2e suite's sshd down with ours. The header of the compose file
/// promises the opposite; this is what makes that true.
const COMPOSE_PROJECT: &str = "yinz-remote-panes";
/// Where sshd is published, through the proxy. Matches
/// `docker/remote-pane-toxiproxy.json` and the compose port mapping.
const SSH_PORT: u16 = 2223;
/// How long to wait for sshd to start accepting connections.
const SSHD_READY_TIMEOUT: Duration = Duration::from_secs(60);

pub fn main(sh: &Shell, flags: flags::RemoteTest) -> anyhow::Result<()> {
    let err_context = "failed to run the remote-pane tests";

    if flags.native {
        if !flags.build || flags.test || flags.clean {
            anyhow::bail!(
                "'--native' is only valid with '--build': run \
                 'cargo x remote-test --build --native', then 'cargo x remote-test --test'"
            );
        }
        if !cfg!(target_os = "linux") {
            anyhow::bail!(
                "'--native' requires a Linux host; use 'cargo x remote-test --build' \
                 for the Docker-based Debian build"
            );
        }
    }

    match (flags.build, flags.test, flags.clean) {
        (false, false, false) => Err(anyhow::anyhow!(
            "one of '--build', '--test' or '--clean' must be provided!"
        )),
        (_, _, true) if flags.build || flags.test => Err(anyhow::anyhow!(
            "'--clean' cannot be combined with '--build' or '--test'!"
        )),
        (true, true, _) => Err(anyhow::anyhow!(
            "flags '--build' and '--test' are mutually exclusive!"
        )),
        (_, _, true) => clean(sh),
        (true, _, _) => build(sh, flags.native),
        (_, true, _) => test(sh, flags.args),
    }
    .context(err_context)
}

/// The directory both sides agree on, under `target/` for the same reason the
/// integration tests' fixtures are: it is already ignored, already per-checkout,
/// and already what the containers bind-mount.
pub fn remote_test_dir() -> PathBuf {
    crate::target_dir().join("remote-test")
}

fn build(sh: &Shell, native: bool) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());

    let relative_binary = if native {
        crate::status(">> Building the native debug binary for the far machine");
        println!(">> Building the native debug binary for the far machine");
        let cargo = crate::cargo()?;
        cmd!(sh, "{cargo} build --bin yinz")
            .run()
            .context("failed to build the native binary the container runs")?;
        PathBuf::from("debug/yinz")
    } else {
        crate::status(">> Building the Debian GNU binary for the far machine");
        println!(">> Building the Debian GNU binary for the far machine");
        crate::ci::build_linux_release(sh)
            .context("failed to build the binary the container runs")?
    };
    let binary = crate::target_dir().join(&relative_binary);
    if !binary.exists() {
        return Err(anyhow::anyhow!(
            "the build reported success but {} is missing",
            binary.display()
        ));
    }

    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &remote_test_dir().join("data"),
    )?;
    generate_credentials(sh).context("failed to generate the test's ssh credentials")?;

    // Relative to target/remote-test on both the host and the container's bind mount.
    // Keep the entrypoint independent of the selected target and build profile.
    let link = remote_test_dir().join("yinz");
    match std::fs::remove_file(&link) {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e).with_context(|| format!("failed to remove {}", link.display())),
    }
    let target = Path::new("..").join(relative_binary);
    std::os::unix::fs::symlink(&target, &link)
        .with_context(|| format!("failed to link {} -> {}", link.display(), target.display()))?;

    crate::status(">> Bringing the remote-pane containers up");
    println!(">> Bringing the remote-pane containers up");
    compose(sh, &["up", "-d", "--build"])
        .run()
        .context("failed to start the containers. Is docker running?")?;

    // The entrypoint installs the authorized key and writes the `yinz`
    // wrapper, so an already-running container is holding whatever the *last*
    // build generated. `up -d` will not recreate it — nothing about the compose
    // file changed — so say it explicitly rather than debug a "Permission
    // denied (publickey)" later.
    compose(sh, &["restart", "remote"])
        .run()
        .context("failed to restart the far machine")?;

    wait_for_sshd().context("the containers came up but sshd never started answering")?;
    compose(sh, &["exec", "-T", "remote", "/usr/local/bin/yinz", "--version"])
        .run()
        .context("the built binary cannot run in the SSH container; native builds need compatible runtime libraries")?;

    println!(">> Ready. Run the tests with 'cargo x remote-test --test'.");
    Ok(())
}

fn test(sh: &Shell, args: Vec<OsString>) -> anyhow::Result<()> {
    let run = crate::test_run::TestRun::new()?;
    let _cache_root = sh.push_env(crate::owned_dir::TEST_CACHE_ROOT_ENV, run.path());
    let dir = remote_test_dir();
    if !dir.join("bin/ssh").exists() {
        return Err(anyhow::anyhow!(
            "no ssh shim at {} — run 'cargo x remote-test --build' first",
            dir.join("bin/ssh").display()
        ));
    }
    // Fail here, with an explanation, rather than inside a test as a timeout.
    wait_for_sshd().context(
        "nothing is answering on the test's ssh port. Are the containers up? \
         ('cargo x remote-test --build')",
    )?;

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;

    // `--ignored` (not `--include-ignored`) runs *only* these tests, exactly as
    // the e2e phase does.
    //
    // Single-threaded on purpose, twice over: the harness sets process-wide
    // environment (`PATH`, so `ssh` resolves to the shim), and the tests share
    // one far machine whose broker and forwards are process-global state.
    cmd!(
        sh,
        "{cargo} test -p yinz-server --test remote_pane_ssh -- --ignored --nocapture --test-threads 1"
    )
    .args(args)
    .run()
    .map_err(anyhow::Error::new)
    .context("the remote-pane tests failed")
}

fn clean(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    // `-v` deliberately omitted: the only volumes are bind mounts of the
    // checkout, and removing those would mean removing `target/`.
    let _ = compose(sh, &["down", "--remove-orphans"]).run();
    let dir = remote_test_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove {}", dir.display()))?;
    }
    println!(">> Containers down, generated credentials removed.");
    Ok(())
}

fn compose<'a>(sh: &'a Shell, args: &[&str]) -> xshell::Cmd<'a> {
    cmd!(sh, "docker compose -p {COMPOSE_PROJECT} -f {COMPOSE_FILE}").args(args)
}

/// Write the keypair the container mounts, the ssh config that points at it, and
/// the shim that gets that config in front of the code under test.
///
/// The shim needs explaining. The production path runs `ssh <host>` with no
/// `-F`, so its configuration is whatever the user's own is — and a test may not
/// write to that. `$HOME` is no way in either: OpenSSH resolves `~/.ssh/config`
/// through `getpwuid()`, not through the environment, so overriding `HOME` moves
/// nothing.
///
/// What *is* honoured is `PATH`, because `Command::new("ssh")` resolves through
/// it. So the shim is an `ssh` earlier on the path that execs the real one with
/// `-F <test config>` in front of whatever it was given. It supplies exactly
/// what a developer's `~/.ssh/config` would supply and nothing else: every other
/// argument — `BatchMode`, `ControlMaster`, `-N -T -L` — still comes from the
/// code being tested.
fn generate_credentials(sh: &Shell) -> anyhow::Result<()> {
    let dir = remote_test_dir();
    let keys = dir.join("keys");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&keys)
        .with_context(|| format!("failed to create {}", keys.display()))?;
    std::fs::create_dir_all(&bin).with_context(|| format!("failed to create {}", bin.display()))?;

    let private = dir.join("id_ed25519");
    // Only the public half goes in `keys/`: that directory is mounted into the
    // container, and a private key has no business crossing over.
    let public = keys.join("id_ed25519.pub");
    if !private.exists() || !public.exists() {
        crate::status(">> Generating the test's ssh keypair");
        println!(">> Generating the test's ssh keypair");
        let _ = std::fs::remove_file(&private);
        let _ = std::fs::remove_file(private.with_extension("pub"));
        let private_arg = private.display().to_string();
        cmd!(
            sh,
            "ssh-keygen -q -t ed25519 -N '' -C yinz-remote-pane-test -f {private_arg}"
        )
        .run()
        .context("failed to generate an ssh keypair. Is ssh-keygen installed?")?;
        std::fs::copy(private.with_extension("pub"), &public)
            .with_context(|| format!("failed to publish the public key to {}", public.display()))?;
    }
    set_private_mode(&private)?;

    // `StrictHostKeyChecking no` and a throwaway known_hosts: the container is
    // rebuilt with fresh host keys whenever the image changes, and with
    // `BatchMode=yes` a host-key prompt is not a prompt, it is a failure.
    let config = format!(
        "Host {host}\n    \
             HostName 127.0.0.1\n    \
             Port {port}\n    \
             User test\n    \
             IdentityFile {identity}\n    \
             IdentitiesOnly yes\n    \
             StrictHostKeyChecking no\n    \
             UserKnownHostsFile /dev/null\n    \
             LogLevel ERROR\n",
        host = HOST_ALIAS,
        port = SSH_PORT,
        identity = private.display(),
    );
    let config_path = dir.join("ssh_config");
    std::fs::write(&config_path, config)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    set_private_mode(&config_path)?;

    // Absolute, and resolved now: the shim is itself named `ssh` and will be
    // first on the PATH, so anything less would find itself.
    let real_ssh = which::which("ssh").context("could not find an ssh binary on the PATH")?;
    let shim = bin.join("ssh");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\n\
             # Generated by `cargo x remote-test --build`. Puts the test's ssh\n\
             # config in front of the code under test, which runs a bare `ssh`.\n\
             exec {real} -F {config} \"$@\"\n",
            real = real_ssh.display(),
            config = config_path.display(),
        ),
    )
    .with_context(|| format!("failed to write {}", shim.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", shim.display()))?;
    }
    Ok(())
}

/// The ssh destination the tests pass to `--remote-host`. An alias rather than
/// a `user@host`, because resolving one is part of what is being tested.
pub const HOST_ALIAS: &str = "yinz-remote-pane";

fn set_private_mode(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // ssh refuses to use a private key others can read, and says so in a way
        // that looks like an authentication failure.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn wait_for_sshd() -> anyhow::Result<()> {
    let deadline = Instant::now() + SSHD_READY_TIMEOUT;
    loop {
        let last_error = match TcpStream::connect(("127.0.0.1", SSH_PORT)) {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "nothing accepted a connection on 127.0.0.1:{SSH_PORT} within \
                 {SSHD_READY_TIMEOUT:?}: {last_error}"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
