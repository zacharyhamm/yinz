use crate::flags;
use anyhow::Context;
use xshell::{cmd, Shell};

pub fn integration_test(sh: &Shell, flags: flags::IntegrationTest) -> anyhow::Result<()> {
    let err_context = "failed to run task 'integration-test'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    let msg = ">> Running whole-app integration tests";
    crate::status(msg);
    println!("{}", msg);

    // Debug plugins, as `x test` builds them: the suite runs a debug binary, which looks in
    // `target/wasm32-wasip1/debug` before anything installed, so this tests the plugins in
    // the tree rather than the last release build.
    crate::build::build(
        sh,
        flags::Build {
            release: false,
            profile: None,
            no_plugins: false,
            plugins_only: true,
            args: vec![],
        },
    )
    .context(err_context)?;

    let nextest_available = cmd!(sh, "{cargo} nextest --version")
        .quiet()
        .ignore_stderr()
        .read()
        .is_ok();
    if nextest_available {
        let serial_args: &[&str] = if flags.serial {
            &["--test-threads=1"]
        } else {
            &[]
        };
        cmd!(sh, "{cargo} nextest run -p zellij-integration-tests")
            .args(serial_args)
            .args(&flags.args)
            .run()
            .context(err_context)?;
    } else {
        println!(">> cargo-nextest not found, falling back to `cargo test -- --test-threads=1`");
        // The fallback shares one process per test binary, and session names come from a
        // counter that is process-global, so snapshots that record a session name match only
        // when their test is the first in its binary to start one. Those failures are the
        // runner's, not the code's, and are confusing enough to be worth this line.
        println!(">> expect false failures in snapshot tests that record session names; install cargo-nextest for a run that means something");
        cmd!(
            sh,
            "{cargo} test -p zellij-integration-tests -- --test-threads=1"
        )
        .args(&flags.args)
        .run()
        .context(err_context)?;
    }
    Ok(())
}
