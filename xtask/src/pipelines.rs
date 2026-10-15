//! Composite pipelines for the build system.
//!
//! Defines multiple "pipelines" that run specific individual steps in sequence.
use crate::flags;
use crate::{build, format, test};
use anyhow::Context;
use std::process::{Command, Stdio};
use xshell::{cmd, Shell};

/// Perform a default build.
///
/// Runs the following steps in sequence:
///
/// - format
/// - build
/// - test
pub fn make(sh: &Shell, flags: flags::Make) -> anyhow::Result<()> {
    let err_context = || format!("failed to run pipeline 'make' with args {flags:?}");

    if flags.clean {
        crate::cargo()
            .and_then(|cargo| cmd!(sh, "{cargo} clean").run().map_err(anyhow::Error::new))
            .with_context(err_context)?;
    }

    format::format(sh, flags::Format { check: false })
        .and_then(|_| {
            build::build(
                sh,
                flags::Build {
                    release: flags.release,
                    profile: None,
                    args: vec![],
                },
            )
        })
        .and_then(|_| test::test(sh, flags::Test { args: vec![] }))
        .with_context(err_context)
}

/// Install the native executable, including its embedded Scheme plugins.
pub fn install(sh: &Shell, flags: flags::Install) -> anyhow::Result<()> {
    let profile = flags.profile.as_deref().unwrap_or("release");
    build::build(
        sh,
        flags::Build {
            release: false,
            profile: Some(profile.to_owned()),
            args: flags.args,
        },
    )?;
    let destination = if flags.destination.is_absolute() {
        flags.destination
    } else {
        std::env::current_dir()
            .context("determine install working directory")?
            .join(flags.destination)
    };
    let destination = if destination.is_dir() {
        destination.join("yinz")
    } else {
        destination
    };
    let binary = crate::target_dir()
        .join(build::profile_dir(profile))
        .join("yinz");
    // Rename a fresh inode so macOS does not reuse an outdated cached code signature.
    let staged = destination.with_extension("xtask-install-tmp");
    sh.copy_file(&binary, &staged)
        .context("stage installed binary")?;
    std::fs::rename(staged, destination).context("install native binary")
}

/// Run yinz debug build.
pub fn run(sh: &Shell, flags: flags::Run) -> anyhow::Result<()> {
    let err_context =
        |flags: &flags::Run| format!("failed to run pipeline 'run' with args {:?}", flags);

    if flags.quick_run && flags.data_dir.is_some() {
        eprintln!("cannot use '--data-dir' and '--quick-run' at the same time!");
        std::process::exit(1);
    }

    let data_dir = if flags.quick_run {
        // Isolate development storage from the user's real plugin-data.db.
        let scratch = crate::target_dir().join("dev-data");
        sh.create_dir(&scratch)
            .with_context(|| err_context(&flags))?;
        Some(scratch)
    } else {
        flags
            .data_dir
            .as_ref()
            .map(|data_dir| sh.current_dir().join(data_dir))
    };
    build::run_proto_codegen(sh, false);

    let cargo = crate::cargo().with_context(|| err_context(&flags))?;
    let mut cmd = cmd!(sh, "{cargo} run").args(["--package", "yinz", "--"]);
    if let Some(data_dir) = data_dir {
        cmd = cmd.args(["--data-dir", &format!("{}", data_dir.display())]);
    }
    run_interactive(cmd.args(&flags.args)).with_context(|| err_context(&flags))
}

fn run_interactive(cmd: xshell::Cmd<'_>) -> anyhow::Result<()> {
    let description = cmd.to_string();
    let status = Command::from(cmd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to spawn {description}"))?;
    anyhow::ensure!(status.success(), "{description} failed with {status}");
    Ok(())
}
