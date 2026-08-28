use crate::{build, flags};
use anyhow::{anyhow, Context};
use xshell::{cmd, Shell};

/// Workspace members `x test` does not own.
///
/// `zellij-integration-tests` belongs to `cargo x integration-test`, which brings the whole-app
/// fixtures up and runs it under its own nextest profile. Sweeping it in here would silently add
/// 46 heavyweight binaries to the gate.
pub const EXCLUDED: &[&str] = &["zellij-integration-tests"];

/// The feature set standing in for "everything except the web server".
///
/// The root crate is the only workspace member with a `default` list, so `--no-default-features`
/// is a no-op everywhere else; `vendored_curl` is the one default worth keeping, and enabling it
/// on the root propagates it to `zellij-utils`.
pub const NO_WEB_FEATURES: &str = "zellij/vendored_curl";

pub fn test(sh: &Shell, flags: flags::Test) -> anyhow::Result<()> {
    let err_context = "failed to run task 'test'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    build::build(
        sh,
        flags::Build {
            release: false,
            no_plugins: false,
            plugins_only: true,
            no_web: flags.no_web,
            args: vec![],
        },
    )
    .context(err_context)?;

    println!();
    let msg = ">> Testing the workspace";
    crate::status(msg);
    println!("{}", msg);

    // One invocation for the whole workspace, rather than one per crate. Cargo resolves features
    // once, so `zellij-utils` - and everything above it - is compiled once instead of once per
    // distinct feature set, and there is a single build/link tail rather than eight.
    //
    // The plugin crates pin themselves to wasm32-wasip1 in their own `.cargo/config.toml`, but
    // Cargo reads config from the invocation directory upward and this runs from the project
    // root, so that pin never applies here and they build for the host.
    //
    // Features are left at their defaults on purpose. The root crate's `default` already turns on
    // `web_server_capability` and `vendored_curl` for the whole graph, and `unstable` is dead, so
    // the defaults cover everything `--all-features` would - while keeping this graph identical
    // to the one `x build` produces, which is what lets the two share artifacts.
    let mut failed: Vec<&str> = Vec::new();
    let use_nextest = nextest_available(sh, &cargo);

    let mut cmd = if use_nextest {
        cmd!(sh, "{cargo} nextest run --profile unit --workspace")
    } else {
        println!(">> cargo-nextest not found, falling back to `cargo test`");
        println!(
            ">> the serial tests will run one at a time; install cargo-nextest for a parallel run"
        );
        cmd!(sh, "{cargo} test --workspace --no-fail-fast")
    };
    for excluded in EXCLUDED {
        cmd = cmd.args(["--exclude", excluded]);
    }
    if flags.no_web {
        cmd = cmd.args(["--no-default-features", "--features", NO_WEB_FEATURES]);
    }
    // nextest takes its own flags directly; libtest's live after `--`.
    if !use_nextest {
        cmd = cmd.arg("--");
    }
    if cmd.args(&flags.args).run().is_err() {
        failed.push("tests");
    }

    // nextest runs one process per test and so cannot run doctests at all. There are few of them,
    // but dropping them silently would be a coverage regression, so they get their own pass.
    if use_nextest {
        println!();
        let msg = ">> Testing doc examples";
        crate::status(msg);
        println!("{}", msg);

        let mut doc = cmd!(sh, "{cargo} test --doc --workspace");
        for excluded in EXCLUDED {
            doc = doc.args(["--exclude", excluded]);
        }
        if flags.no_web {
            doc = doc.args(["--no-default-features", "--features", NO_WEB_FEATURES]);
        }
        if doc.run().is_err() {
            failed.push("doctests");
        }
    }

    if !failed.is_empty() {
        println!();
        let msg = format!(">> Test failures in: {}", failed.join(", "));
        crate::status(&msg);
        println!("{}", msg);
        return Err(anyhow!("tests failed in: {}", failed.join(", "))).context(err_context);
    }
    Ok(())
}

pub fn nextest_available(sh: &Shell, cargo: &std::path::Path) -> bool {
    cmd!(sh, "{cargo} nextest --version")
        .quiet()
        .ignore_stderr()
        .read()
        .is_ok()
}
