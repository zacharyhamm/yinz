use crate::flags;
use anyhow::{anyhow, Context};
use std::ffi::OsString;
use xshell::{cmd, Shell};

/// Workspace members `x test` does not own.
///
/// `yinz-integration-tests` belongs to `cargo x integration-test`, which brings the whole-app
/// fixtures up and runs it under its own nextest profile. Sweeping it in here would silently add
/// the integration binaries to standalone unit-test runs. The gate runs both phases.
pub const EXCLUDED: &[&str] = &["yinz-integration-tests"];

pub fn test(sh: &Shell, flags: flags::Test) -> anyhow::Result<()> {
    crate::test_run::private_creation_mask();
    let run = crate::test_run::TestRun::new()?;
    let _cache_root = sh.push_env(crate::owned_dir::TEST_CACHE_ROOT_ENV, run.path());
    let err_context = "failed to run task 'test'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    crate::build::run_proto_codegen(sh, false);
    // Native compiler workers must re-exec this checkout's production binary, never libtest.
    cmd!(sh, "{cargo} build --bin yinz")
        .run()
        .context(err_context)?;
    let _worker = sh.push_env(
        "YINZ_COMPILATION_WORKER",
        crate::project_root().join("target/debug/yinz"),
    );

    println!();
    let msg = ">> Testing the workspace";
    crate::status(msg);
    println!("{}", msg);

    // One invocation for the whole workspace, rather than one per crate. Cargo resolves features
    // once, so `yinz-types` - and everything above it - is compiled once instead of once per
    // distinct feature set, and there is a single build/link tail rather than eight.
    //
    // Features are left at their defaults on purpose, keeping this graph identical to the one
    // `x build` produces so the two share artifacts.
    let mut failed: Vec<&str> = Vec::new();
    let use_nextest = nextest_available(sh, &cargo);

    // `--workspace` and `-p` are additive in cargo, so leaving `--workspace` in would undo a
    // caller's package selection and link the whole tree anyway. When the caller narrowed to
    // packages, theirs is the only selection - and the `EXCLUDED` sweep, which only exists to keep
    // `--workspace` off the integration tests, has nothing left to do.
    let (selection, filtered) = selection(&flags.args);
    let narrowed_to_packages = matches!(selection, Selection::Packages(_));

    let mut cmd = if use_nextest {
        cmd!(sh, "{cargo} nextest run --profile unit")
    } else {
        println!(">> cargo-nextest not found, falling back to `cargo test`");
        println!(
            ">> the serial tests will run one at a time; install cargo-nextest for a parallel run"
        );
        cmd!(sh, "{cargo} test --no-fail-fast")
    };
    if !narrowed_to_packages {
        cmd = cmd.arg("--workspace");
        for excluded in EXCLUDED {
            cmd = cmd.args(["--exclude", excluded]);
        }
    }
    // nextest takes its own flags directly; libtest's live after `--`.
    if !use_nextest {
        cmd = cmd.arg("--");
    }
    if cmd.args(&flags.args).run().is_err() {
        failed.push("tests");
    }

    // nextest runs one process per test and so cannot run doctests at all. There are few of them,
    // but dropping them silently would be a coverage regression, so they get their own pass -
    // narrowed the same way the nextest run was, as far as `cargo test --doc` can express it.
    if use_nextest {
        match &selection {
            _ if filtered => {
                println!();
                println!(">> skipping doctests: run narrowed by a test filter");
            },
            Selection::Packages(selectors) if !has_lib_target(sh, selectors) => {
                println!();
                println!(">> skipping doctests: no library targets in the selected packages");
            },
            selection => {
                println!();
                let msg = ">> Testing doc examples";
                crate::status(msg);
                println!("{}", msg);

                let mut doc = cmd!(sh, "{cargo} test --doc");
                match selection {
                    Selection::Packages(selectors) => {
                        doc = doc.args(selectors);
                    },
                    // The `EXCLUDED` sweep only means anything at workspace scope.
                    Selection::Workspace(extra_excludes) => {
                        doc = doc.arg("--workspace");
                        for excluded in EXCLUDED {
                            doc = doc.args(["--exclude", excluded]);
                        }
                        doc = doc.args(extra_excludes);
                    },
                }
                if doc.run().is_err() {
                    failed.push("doctests");
                }
            },
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

/// Whether any package the caller selected has a library target.
///
/// `cargo test --doc -p <bin-only crate>` is a hard error ("no library targets found in package"),
/// which would turn a perfectly good narrowed run red. A glob selector (`-p 'yinz-*'`) is not
/// resolved here - cargo is left to decide - and a metadata failure is not worth failing over
/// either, so both answer "yes" and let the real command speak.
fn has_lib_target(sh: &Shell, selectors: &[OsString]) -> bool {
    let names = selected_package_names(selectors);
    if names.is_empty() || names.iter().any(|name| name.contains(['*', '?', '['])) {
        return true;
    }
    let Ok(metadata) = crate::metadata::get_cargo_metadata(sh) else {
        return true;
    };
    let Some(packages) = metadata["packages"].as_array() else {
        return true;
    };
    packages.iter().any(|package| {
        package["name"]
            .as_str()
            .is_some_and(|name| names.iter().any(|selected| selected == name))
            && package["targets"].as_array().is_some_and(|targets| {
                targets.iter().any(|target| {
                    target["kind"].as_array().is_some_and(|kinds| {
                        kinds.iter().any(|kind| {
                            matches!(kind.as_str(), Some("lib" | "rlib" | "proc-macro"))
                        })
                    })
                })
            })
    })
}

/// The package names behind `-p`/`--package` selectors, in either spelling. `--exclude` names are
/// deliberately left out: they say what not to test, not what to test.
fn selected_package_names(selectors: &[OsString]) -> Vec<String> {
    let mut names = Vec::new();
    let mut value_next = false;
    for selector in selectors {
        let text = selector.to_string_lossy();
        if value_next {
            names.push(text.into_owned());
            value_next = false;
            continue;
        }
        match text.split_once('=') {
            Some((name, value)) if name == "-p" || name == "--package" => {
                names.push(value.to_string())
            },
            Some(_) => {},
            None => value_next = text == "-p" || text == "--package",
        }
    }
    names
}

/// How far the arguments forwarded to nextest narrow the run.
#[derive(Debug, PartialEq)]
enum Selection {
    /// No package was named. The run keeps its workspace scope; any `--exclude` the caller passed
    /// rides along, so the doctest pass honours it too.
    Workspace(Vec<OsString>),
    /// The caller named packages - the one selection `cargo test --doc` shares with nextest. The
    /// tokens are ready to hand to it verbatim.
    Packages(Vec<OsString>),
}

/// Flags that take a separate value, so the token after them is not a test filter.
///
/// This list cannot be complete - nextest and cargo have many - so the fallback is the other way
/// round: a bare token is only read as a filter when the token before it is not an unrecognised
/// flag. Getting it wrong only changes whether an inner-loop run also runs doctests.
const VALUE_FLAGS: &[&str] = &[
    "-j",
    "--jobs",
    "--test-threads",
    "-E",
    "--filterset",
    "--filter-expr",
    "-P",
    "--profile",
    "--partition",
    "--retries",
    "--run-ignored",
    "--failure-output",
    "--success-output",
    "--status-level",
    "--final-status-level",
    "--message-format",
    "--config-file",
    "--tool-config-file",
    "--target",
    "--cargo-profile",
    "--build-jobs",
    "-F",
    "--features",
];

/// Flags known to take no value, so a bare token following one really is a filter.
const BOOLEAN_FLAGS: &[&str] = &[
    "--no-capture",
    "--nocapture",
    "--no-fail-fast",
    "--fail-fast",
    "--no-run",
    "--all-features",
    "--no-default-features",
    "--release",
    "--offline",
    "--locked",
    "--frozen",
    "--workspace",
    "--all",
    "--all-targets",
    "--lib",
    "--bins",
    "--tests",
    "--benches",
    "--hide-progress-bar",
    "--cargo-quiet",
    "-v",
    "--verbose",
    "-q",
    "--quiet",
];

const PACKAGE_FLAGS: &[&str] = &["-p", "--package", "--exclude"];
const FILTER_FLAGS: &[&str] = &["-E", "--filterset", "--filter-expr"];

// Package scope and test filtering are independent: a filter skips doctests, but must
// never widen a package-selected build back to the whole workspace.
fn selection(args: &[OsString]) -> (Selection, bool) {
    let mut selectors: Vec<OsString> = Vec::new();
    let mut named_a_package = false;
    let mut previous: Option<&OsString> = None;
    let mut skip_next = false;
    let mut package_value_next = false;
    let mut filtered = false;

    for arg in args {
        if skip_next || package_value_next {
            if package_value_next {
                selectors.push(arg.clone());
            }
            skip_next = false;
            package_value_next = false;
            previous = Some(arg);
            continue;
        }
        let text = arg.to_string_lossy();
        let (name, has_inline_value) = match text.split_once('=') {
            Some((name, _)) => (name.to_string(), true),
            None => (text.to_string(), false),
        };

        if FILTER_FLAGS.contains(&name.as_str()) {
            filtered = true;
        }
        if PACKAGE_FLAGS.contains(&name.as_str()) {
            named_a_package |= name != "--exclude";
            selectors.push(arg.clone());
            if !has_inline_value {
                // `-p foo`: the name comes next, and belongs to the doctest command too.
                package_value_next = true;
            }
            previous = Some(arg);
            continue;
        }
        if text.starts_with('-') {
            skip_next = !has_inline_value && VALUE_FLAGS.contains(&name.as_str());
            previous = Some(arg);
            continue;
        }

        // A bare token. It is a test-name filter unless the token before it was a flag this
        // function does not know, in which case it is more likely that flag's value.
        let is_value_of_unknown_flag = previous.is_some_and(|prev| {
            let prev = prev.to_string_lossy();
            prev.starts_with('-') && !prev.contains('=') && !BOOLEAN_FLAGS.contains(&prev.as_ref())
        });
        if !is_value_of_unknown_flag {
            filtered = true;
        }
        previous = Some(arg);
    }

    let scope = if named_a_package {
        Selection::Packages(selectors)
    } else {
        Selection::Workspace(selectors)
    };
    (scope, filtered)
}

pub fn nextest_available(sh: &Shell, cargo: &std::path::Path) -> bool {
    cmd!(sh, "{cargo} nextest --version")
        .quiet()
        .ignore_stderr()
        .read()
        .is_ok()
}

#[cfg(test)]
#[path = "unit/test.rs"]
mod tests;
