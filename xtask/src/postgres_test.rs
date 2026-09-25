//! The plugin-data store suite against a real Postgres, in a container.
//!
//! `zellij-storage/src/plugin_data_store/tests/` runs every test once per database engine. The
//! SQLite half runs under `cargo x test` like anything else; the Postgres half needs a server,
//! sits behind the `postgres-tests` cargo feature, and runs here.
//!
//! Three phases, mirroring `remote-test`:
//!
//!   * `--build` — the container up, and waiting until it answers
//!   * `--test`  — `cargo nextest run` with the feature on and the URL in the environment
//!   * `--clean` — the container and its volume down
//!
//! Nothing here is part of `cargo x ci check`, which must not need docker. `cargo x lint` compiles
//! the suite with `--all-features`, so it cannot rot unnoticed between runs of this.

use crate::flags;
use anyhow::Context;
use std::ffi::OsString;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use xshell::{cmd, Shell};

const COMPOSE_FILE: &str = "docker-compose.postgres.yml";
/// A project name of our own, for the reason `remote_test.rs` gives: without one every compose
/// file invoked from the repository root lands in one project, and each file's containers are
/// orphans of the other.
const COMPOSE_PROJECT: &str = "zellij-postgres";
/// Where the server is published. Matches the compose file's port mapping.
const PORT: u16 = 54329;
/// The connection string the tests use. Credentials match the compose file's environment.
const URL: &str = "postgres://zellij:zellij@127.0.0.1:54329/zellij_test?sslmode=disable";
/// How the test binary learns the URL; read by `plugin_data_store/tests/postgres.rs`.
const URL_ENV: &str = "ZELLIJ_PLUGIN_DATA_TEST_URL";
const CA_ENV: &str = "ZELLIJ_PLUGIN_DATA_TEST_CA";
/// How long to wait for the server to accept connections after `up`.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// One process per test, each with a connection of its own; the compose file allows 300.
const PARALLELISM: &str = "16";

pub fn main(sh: &Shell, flags: flags::PostgresTest) -> anyhow::Result<()> {
    let err_context = "failed to run the Postgres plugin-data tests";

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
        (true, _, _) => build(sh),
        (_, true, _) => test(sh, flags.args),
    }
    .context(err_context)
}

fn build(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    crate::status(">> Bringing the Postgres container up");
    println!(">> Bringing the Postgres container up");
    compose(sh, &["up", "-d", "--build"])
        .run()
        .context("failed to start the container. Is docker running?")?;
    wait_for_postgres(sh).context("the container came up but Postgres never started answering")?;
    println!(">> Ready. Run the tests with 'cargo x postgres-test --test'.");
    Ok(())
}

fn test(sh: &Shell, args: Vec<OsString>) -> anyhow::Result<()> {
    let run = crate::test_run::TestRun::new()?;
    let _cache_root = sh.push_env(crate::owned_dir::TEST_CACHE_ROOT_ENV, run.path());
    // Fail here, with an explanation, rather than inside every test as a connection error.
    wait_for_postgres(sh).context(
        "nothing is answering on the test's Postgres port. Is the container up? \
         ('cargo x postgres-test --build')",
    )?;

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;
    let ca = run.path().join("postgres-ca.pem");
    compose(sh, &["cp", "postgres:/tls/ca-bundle.pem"])
        .arg(&ca)
        .run()
        .context("failed to export the test CA; run 'cargo x postgres-test --build'")?;
    let _ca = sh.push_env(CA_ENV, &ca);

    crate::status(">> Running the plugin-data store suite against Postgres");
    println!(">> Running the plugin-data store suite against Postgres");
    // Only the Postgres half: the SQLite half of the same files is what `cargo x test` runs.
    let filter = "test(/plugin_data_store::(tests::)?postgres::/)";
    let result = if crate::test::nextest_available(sh, &cargo) {
        cmd!(
            sh,
            "{cargo} nextest run -p zellij-storage --features postgres-tests --profile unit -j {PARALLELISM} -E {filter}"
        )
        .env(URL_ENV, URL)
        .args(args)
        .run()
    } else {
        eprintln!(
            ">> cargo-nextest is not installed; falling back to `cargo test`, which runs the \
             tests as threads of one process rather than one process each"
        );
        cmd!(
            sh,
            "{cargo} test -p zellij-storage --features postgres-tests --lib postgres"
        )
        .env(URL_ENV, URL)
        // The bootstrap deadline test deliberately holds the shared schema lock.
        .env("RUST_TEST_THREADS", "1")
        .args(args)
        .run()
    };
    result
        .map_err(anyhow::Error::new)
        .context("the Postgres plugin-data tests failed")
}

fn clean(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    // `-v` on purpose: the only volume is the server's tmpfs data directory, which holds nothing
    // but test schemas that were dropped anyway.
    let _ = compose(sh, &["down", "--remove-orphans", "-v"]).run();
    println!(">> Container down.");
    Ok(())
}

fn compose<'a>(sh: &'a Shell, args: &[&str]) -> xshell::Cmd<'a> {
    cmd!(sh, "docker compose -p {COMPOSE_PROJECT} -f {COMPOSE_FILE}").args(args)
}

/// Wait for the port to accept a connection, then for the server behind it to say it is ready.
///
/// Two steps because the port answers as soon as the container's network is up, a moment before
/// `postgres` has finished its own startup, and `pg_isready` is asked inside the container
/// rather than assumed to be installed on the host.
fn wait_for_postgres(sh: &Shell) -> anyhow::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let last_error = match TcpStream::connect(("127.0.0.1", PORT)) {
            Ok(_) => break,
            Err(e) => e,
        };
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "nothing accepted a connection on 127.0.0.1:{PORT} within {READY_TIMEOUT:?}: \
                 {last_error}"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _pd = sh.push_dir(crate::project_root());
    loop {
        let ready = compose(
            sh,
            &[
                "exec",
                "-T",
                "postgres",
                "pg_isready",
                "-U",
                "zellij",
                "-d",
                "zellij_test",
            ],
        )
        .quiet()
        .ignore_stdout()
        .ignore_stderr()
        .run()
        .is_ok();
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "Postgres on 127.0.0.1:{PORT} did not report ready within {READY_TIMEOUT:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
