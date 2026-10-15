use crate::flags;
use anyhow::Context;
use std::collections::{BTreeMap, BTreeSet};
use xshell::{cmd, Shell};

pub fn integration_test(sh: &Shell, flags: flags::IntegrationTest) -> anyhow::Result<()> {
    let run = crate::test_run::TestRun::new()?;
    let _cache_root = sh.push_env(crate::owned_dir::TEST_CACHE_ROOT_ENV, run.path());
    let err_context = "failed to run task 'integration-test'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    let msg = ">> Running whole-app integration tests";
    crate::status(msg);
    println!("{}", msg);

    crate::build::run_proto_codegen(sh, false);
    // Native compiler workers must re-exec this checkout's production binary, never libtest.
    cmd!(sh, "{cargo} build --bin yinz")
        .run()
        .context(err_context)?;
    let _worker = sh.push_env(
        "YINZ_COMPILATION_WORKER",
        crate::project_root().join("target/debug/yinz"),
    );

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
        cmd!(sh, "{cargo} nextest run -p yinz-integration-tests")
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
            "{cargo} test -p yinz-integration-tests -- --test-threads=1"
        )
        .args(&flags.args)
        .run()
        .context(err_context)?;
    }
    Ok(())
}

/// Check that the two builds `integration_test` runs resolve the same features for every package
/// they share.
///
/// `cargo build --bin yinz` and the suite's `-p yinz-integration-tests` build resolve
/// features separately. A feature only one of them asks for, such as nix's `ioctl` from the
/// root-only yinz-server-os, builds that package and every crate above it a second time in each
/// `x integration-test`. No build fails over it, so this compares what the two resolve.
pub fn check_shared_features(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;
    let format = ["--prefix", "none", "-f", "{p} [{f}]"];
    let binary = cmd!(sh, "{cargo} tree -q -p yinz -e normal,build")
        .args(format)
        .read()
        .context("failed to resolve the yinz binary's dependencies")?;
    // Dev-dependencies too: the suite's tests are what gets built.
    let suite = cmd!(
        sh,
        "{cargo} tree -q -p yinz-integration-tests -e normal,build,dev"
    )
    .args(format)
    .read()
    .context("failed to resolve yinz-integration-tests' dependencies")?;
    let diverging = diverging_features(&binary, &suite);
    anyhow::ensure!(
        diverging.is_empty(),
        "these packages resolve different features for the yinz binary and for \
         yinz-integration-tests, so `x integration-test` builds them and every crate above them \
         twice. Request the feature where both graphs see it, such as [workspace.dependencies]:\n{}",
        diverging.join("\n")
    );
    Ok(())
}

/// The shared packages of two `cargo tree --prefix none -f '{p} [{f}]'` listings whose feature
/// sets differ, one line each. A package can appear with several sets, because build
/// dependencies and proc macros resolve apart from the rest; each distinct set is one build.
fn diverging_features(binary: &str, suite: &str) -> Vec<String> {
    let binary = resolved_features(binary);
    let suite = resolved_features(suite);
    let show = |sets: &BTreeSet<&str>| {
        sets.iter()
            .map(|features| format!("[{features}]"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    binary
        .iter()
        .filter_map(|(package, in_binary)| {
            let in_suite = suite.get(package)?;
            (in_binary != in_suite).then(|| {
                format!(
                    "  {package}: binary {}, suite {}",
                    show(in_binary),
                    show(in_suite)
                )
            })
        })
        .collect()
}

/// The feature sets of each `name version` in a `cargo tree` listing.
fn resolved_features(tree: &str) -> BTreeMap<String, BTreeSet<&str>> {
    let mut packages: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for line in tree.lines() {
        // A repeated package is marked `(*)` instead of listing its dependencies again.
        let line = line.strip_suffix(" (*)").unwrap_or(line);
        let Some((package, features)) = line.rsplit_once(" [") else {
            continue;
        };
        let package = package.split_whitespace().take(2).collect::<Vec<_>>();
        packages
            .entry(package.join(" "))
            .or_default()
            .insert(features.strip_suffix(']').unwrap_or(features));
    }
    packages
}

#[cfg(test)]
#[path = "unit/integration_test.rs"]
mod tests;
