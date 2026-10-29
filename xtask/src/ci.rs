//! Tasks related to yinz CI
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

    let project_root = crate::project_root();
    let data_dir = crate::target_dir().join("e2e-data");
    sh.remove_path(&data_dir)
        .and_then(|_| sh.create_dir(&data_dir))
        .context(err_context)?;

    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir)?;
    let _pd = sh.push_dir(project_root);
    let binary = build_linux_release(sh).context(err_context)?;
    link_e2e_binary(&binary).context(err_context)
}

/// Keep the path used in E2E snapshots stable across architectures.
fn link_e2e_binary(binary: &Path) -> anyhow::Result<()> {
    let link = crate::target_dir().join("yinz");
    match std::fs::remove_file(&link) {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e).with_context(|| format!("failed to remove {}", link.display())),
    }
    std::os::unix::fs::symlink(binary, &link)
        .with_context(|| format!("failed to link {} -> {}", link.display(), binary.display()))
}

fn linux_target(arch: &str) -> anyhow::Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        _ => anyhow::bail!("the Debian test builder does not support {arch}"),
    }
}

/// Build in the same Debian release as the test guests, including on macOS.
/// Return the artifact path relative to target/ for all three test suites.
pub fn build_linux_release(sh: &Shell) -> anyhow::Result<PathBuf> {
    let target = linux_target(std::env::consts::ARCH)?;
    let platform = if std::env::consts::ARCH == "aarch64" {
        "linux/arm64"
    } else {
        "linux/amd64"
    };
    let project = crate::project_root()
        .canonicalize()
        .context("failed to resolve checkout")?;
    let _pd = sh.push_dir(&project);
    let target_dir = crate::target_dir();
    sh.create_dir(&target_dir)
        .context("failed to create target directory")?;
    let target_dir = target_dir
        .canonicalize()
        .context("failed to resolve target directory")?;
    let cache = target_dir.join("linux-build");
    sh.create_dir(cache.join("home"))
        .context("failed to create builder home")?;
    sh.create_dir(cache.join("cargo"))
        .context("failed to create builder Cargo cache")?;
    let project_mount = format!("{}:{}", project.display(), project.display());
    // Mount at its real path as well: target/ may be an absolute or relative symlink.
    let target_mount = format!("{}:{}", target_dir.display(), target_dir.display());
    let home = format!("HOME={}", cache.join("home").display());
    let cargo_home = format!("CARGO_HOME={}", cache.join("cargo").display());
    let user = format!("{}:{}", unsafe { libc::getuid() }, unsafe {
        libc::getgid()
    });
    let image = "yinz-test-builder:trixie-slim";
    cmd!(
        sh,
        "docker build --platform {platform} -t {image} -f docker/linux-builder.Dockerfile docker"
    )
    .run()
    .context("failed to build the Debian test builder; Docker is required")?;
    let binary = Path::new(target).join("release/yinz");
    let executable = target_dir.join(&binary);
    // Whole-program LTO exhausts an 8 GiB Docker VM; test binaries need no link-time optimization.
    // Podman also exposes a `docker` CLI; keep its rootless mapping aligned with --user.
    cmd!(sh, "docker run --rm --platform {platform} --user {user} -v {project_mount} -v {target_mount} -w {project} -e {home} -e {cargo_home} {image} cargo build --locked --release --config profile.release.lto=false --config profile.release.codegen-units=16 --target {target} --bin yinz")
        .env("PODMAN_USERNS", "keep-id")
        .run().context("failed to build the GNU/Linux test binary in Debian")?;
    cmd!(sh, "docker run --rm --platform {platform} --user {user} -v {target_mount} {image} {executable} --version")
        .env("PODMAN_USERNS", "keep-id")
        .run().context("the built binary cannot start in Debian trixie-slim")?;
    Ok(binary)
}

#[cfg(test)]
#[path = "unit/ci.rs"]
mod tests;

/// Native release build: generates protobufs and runs
/// `cargo build --release` without cross-compilation.
fn build_release(sh: &Shell) -> anyhow::Result<()> {
    let err_context = "failed to perform native release build";

    build::run_proto_codegen(sh, false);

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
    crate::test_run::private_creation_mask();

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
            let _pd = sh.push_dir(Path::new("yinz-plugin-host"));
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

    build::run_proto_codegen(sh, false);

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
    if let Err(error) = crate::integration_test::check_shared_features(sh) {
        eprintln!("Integration-test features: {error:#}");
        failed.push("integration-test features");
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
