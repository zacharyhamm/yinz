//! Tasks related to zellij CI
use crate::{
    build,
    flags::{self, BuildRelease, Check, CiCmd, Cross, E2e},
};
use anyhow::Context;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use xshell::{cmd, Shell};

pub fn main(sh: &Shell, flags: flags::Ci) -> anyhow::Result<()> {
    let err_context = "failed to run CI task";

    match flags.subcommand {
        CiCmd::Check(Check {}) => check(sh).context(err_context),
        CiCmd::E2e(E2e {
            build: false,
            test: false,
            ..
        }) => Err(anyhow::anyhow!(
            "either '--build' or '--test' must be provided!"
        )),
        CiCmd::E2e(E2e {
            build: true,
            test: true,
            ..
        }) => Err(anyhow::anyhow!(
            "flags '--build' and '--test' are mutually exclusive!"
        )),
        CiCmd::E2e(E2e {
            build: true,
            test: false,
            ..
        }) => e2e_build(sh),
        CiCmd::E2e(E2e {
            build: false,
            test: true,
            args,
        }) => e2e_test(sh, args),
        CiCmd::Cross(Cross { triple }) => cross_compile(sh, &triple),
        CiCmd::BuildRelease(BuildRelease {}) => build_release(sh),
    }
    .context(err_context)
}

fn e2e_build(sh: &Shell) -> anyhow::Result<()> {
    let err_context = "failed to build E2E binary";

    build::build(
        sh,
        flags::Build {
            release: true,
            profile: None,
            no_plugins: false,
            plugins_only: true,
            args: vec![],
        },
    )
    .context(err_context)?;

    // Copy plugins to e2e data-dir. The test plugins are included: the e2e fixture configs
    // load `fixture-plugin-for-tests`.
    let project_root = crate::project_root();
    let data_dir = crate::target_dir().join("e2e-data");

    sh.remove_path(&data_dir)
        .and_then(|_| sh.create_dir(&data_dir))
        .and_then(|_| sh.create_dir(data_dir.join("plugins")))
        .context(err_context)?;

    for name in build::plugin_names() {
        sh.copy_file(build::release_plugin_path(name), data_dir.join("plugins"))
            .context(err_context)?;
    }

    let _pd = sh.push_dir(project_root);
    build_musl_release(sh).context(err_context)?;
    link_e2e_binary().context(err_context)
}

/// Put the musl binary where the container looks for it.
///
/// `remote_runner.rs` runs `/usr/src/zellij/zellij`, which is `target/zellij`
/// through the bind mount — but `build_musl_release` writes to
/// `target/<triple>/release/zellij`. A symlink bridges the two.
///
/// It has to be written the way the *container* reads it, not the way this
/// process does: the target is the absolute in-container path, which resolves to
/// nothing on the host. So this cannot check the link for validity, only replace
/// it, and `symlink` rather than `sh.copy_file` because the point is to follow
/// each rebuild without one.
fn link_e2e_binary() -> anyhow::Result<()> {
    let link = crate::target_dir().join("zellij");
    let target = format!("/usr/src/zellij/{}/release/zellij", musl_target());

    // `symlink` refuses to overwrite, and an existing link may point at the
    // wrong triple after a move between machines.
    match std::fs::remove_file(&link) {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e).with_context(|| format!("failed to remove {}", link.display())),
    }
    std::os::unix::fs::symlink(&target, &link)
        .with_context(|| format!("failed to link {} -> {target}", link.display()))
}

/// The musl triple a container on this host runs natively.
///
/// Matching the host arch is what keeps the container off emulation — arm64 on
/// Apple Silicon, amd64 on Intel and on Linux CI.
pub fn musl_target() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-musl"
    } else {
        "x86_64-unknown-linux-musl"
    }
}

/// A statically linked release binary for the container to run.
///
/// Shared by the e2e suite and the remote-pane suite: both need a Linux binary
/// built from this tree, and both get it into their container through a bind
/// mount of `target/` rather than a copy.
pub fn build_musl_release(sh: &Shell) -> anyhow::Result<()> {
    let target = musl_target();
    // On macOS: use cargo-zigbuild which handles musl cross-compilation for any arch without
    //   needing a separate linker toolchain.
    //   Install with: cargo install cargo-zigbuild && brew install zig
    // On Linux: cross-compile for musl.
    if cfg!(target_os = "macos") {
        crate::cargo().and_then(|cargo| {
            if which::which("cargo-zigbuild").is_err() {
                eprintln!("!! 'cargo-zigbuild' wasn't found but is needed on macOS.");
                eprintln!("!! Please install it with:");
                eprintln!("!!   cargo install cargo-zigbuild");
                eprintln!("!!   brew install zig");
                return Err(anyhow::anyhow!("couldn't find 'cargo-zigbuild'"));
            }
            cmd!(sh, "rustup target add {target}")
                .run()
                .map_err(anyhow::Error::new)?;
            cmd!(sh, "{cargo} zigbuild --release --target {target}")
                .run()
                .map_err(anyhow::Error::new)
        })
    } else {
        crate::cargo().and_then(|cargo| {
            cmd!(sh, "{cargo} build --release --target {target}")
                .run()
                .map_err(anyhow::Error::new)
        })
    }
}

/// Native release build: builds plugins, generates protobufs, and runs
/// `cargo build --release` without cross-compilation.
fn build_release(sh: &Shell) -> anyhow::Result<()> {
    let err_context = "failed to perform native release build";

    // Build plugins and generate protobufs
    build::build(
        sh,
        flags::Build {
            release: true,
            profile: None,
            no_plugins: false,
            plugins_only: true,
            args: vec![],
        },
    )
    .context(err_context)?;

    // Build the main binary natively
    let _pd = sh.push_dir(crate::project_root());
    crate::cargo()
        .and_then(|cargo| {
            cmd!(sh, "{cargo} build --verbose --release")
                .run()
                .map_err(anyhow::Error::new)
        })
        .context(err_context)
}

fn e2e_test(sh: &Shell, args: Vec<OsString>) -> anyhow::Result<()> {
    let run = crate::test_run::TestRun::new()?;
    let _cache_root = sh.push_env(crate::owned_dir::TEST_CACHE_ROOT_ENV, run.path());
    let err_context = "failed to run E2E tests";

    e2e_build(sh).context(err_context)?;

    let _pd = sh.push_dir(crate::project_root());

    crate::cargo()
        .and_then(|cargo| {
            // e2e tests
            cmd!(
                sh,
                "{cargo} test --features e2e -- --ignored --nocapture --test-threads 1"
            )
            .args(args.clone())
            .run()
            .map_err(anyhow::Error::new)?;

            // plugin system tests are run here because they're medium-slow
            let _pd = sh.push_dir(Path::new("zellij-plugin-host"));
            println!();
            let msg = ">> Testing Plugin System".to_string();
            crate::status(&msg);
            println!("{}", msg);

            // `--lib` matters: it keeps this to the unit tests in `src/`, which is
            // where the plugin-system tests live. Without it, `--ignored` sweeps
            // in every ignored test in the crate's `tests/` directory too — which
            // today means the whole remote-pane suite, whose fixtures are a far
            // machine this task knows nothing about and never brought up.
            cmd!(
                sh,
                "{cargo} test --lib -- --ignored --nocapture --test-threads 1"
            )
            .args(args.clone())
            .run()
            .with_context(|| "Failed to run tests for the Plugin System".to_string())?;
            Ok(())
        })
        .context(err_context)
}

fn cross_compile(sh: &Shell, target: &OsString) -> anyhow::Result<()> {
    let err_context = || format!("failed to cross-compile for {target:?}");

    crate::cargo()
        .and_then(|cargo| {
            cmd!(sh, "{cargo} install cross")
                .run()
                .map_err(anyhow::Error::new)
        })
        .with_context(err_context)?;

    build::build(
        sh,
        flags::Build {
            release: true,
            profile: None,
            no_plugins: false,
            plugins_only: true,
            args: vec![],
        },
    )
    .with_context(err_context)?;

    cross()
        .and_then(|cross| {
            cmd!(sh, "{cross} build --verbose --release --target {target}")
                .run()
                .map_err(anyhow::Error::new)
        })
        .with_context(err_context)
}

fn cross() -> anyhow::Result<PathBuf> {
    match which::which("cross") {
        Ok(path) => Ok(path),
        Err(e) => {
            eprintln!("!! 'cross' wasn't found but is needed for this build step.");
            eprintln!("!! Please install it with: `cargo install cross`");
            Err(e).context("couldn't find 'cross' executable")
        },
    }
}

/// The pre-handoff gate.
///
/// This fork has no CI - `.github/workflows` was deleted deliberately - so nothing runs these
/// checks unless a person does. This is the one command to run before handing off a branch, and
/// the reason it exists as a command rather than as a list in a document is that a list in a
/// document is not runnable.
///
/// The phases are ordered cheapest-first, and each is allowed to fail so that one failure does
/// not hide the others.
fn check(sh: &Shell) -> anyhow::Result<()> {
    let mut failed: Vec<&str> = Vec::new();

    if let Err(error) = crate::metadata::check_dependency_boundaries(sh) {
        eprintln!("Dependency boundaries: {error:#}");
        failed.push("dependency boundaries");
    }

    if crate::format::format(sh, flags::Format { check: true }).is_err() {
        failed.push("format --check");
    }
    if crate::lint::lint(
        sh,
        flags::Lint {
            fix: false,
            args: vec![],
        },
    )
    .is_err()
    {
        failed.push("lint");
    }
    if crate::test::test(sh, flags::Test { args: vec![] }).is_err() {
        failed.push("test");
    }

    {
        let _threads = sh.push_env("NEXTEST_TEST_THREADS", "16");
        if crate::integration_test::integration_test(
            sh,
            flags::IntegrationTest {
                serial: false,
                args: vec![],
            },
        )
        .is_err()
        {
            failed.push("integration-test");
        }
    }

    if failed.is_empty() {
        crate::status(">> All checks passed");
        println!("\n>> All checks passed");
        return Ok(());
    }
    let msg = format!(">> Failed: {}", failed.join(", "));
    crate::status(&msg);
    println!("\n{msg}");
    Err(anyhow::anyhow!("checks failed: {}", failed.join(", ")))
}
