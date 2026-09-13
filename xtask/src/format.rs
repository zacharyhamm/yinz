//! Handle running `cargo fmt` on the sources.
use crate::flags;
use anyhow::{anyhow, Context};
use std::path::PathBuf;
use xshell::{cmd, Shell};

pub fn format(sh: &Shell, flags: flags::Format) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());

    let cargo = check_rustfmt()
        .and_then(|_| crate::cargo())
        .context("failed to run task 'format'")?;

    println!();
    let msg = ">> Formatting the workspace";
    crate::status(msg);
    println!("{}", msg);

    // `--all` covers every workspace member in one process. There is a single `.rustfmt.toml`, at
    // the repo root, so visiting each crate in turn bought nothing but 30 process spawns - and it
    // stopped at the first offender, hiding the rest.
    let mut cmd = cmd!(sh, "{cargo} fmt --all");
    if flags.check {
        cmd = cmd.arg("--check");
    }
    cmd.run()
        .map_err(|_| anyhow!("formatting check failed"))
        .context("failed to run task 'format'")?;
    crate::authoring::format(sh, flags.check)
}

fn check_rustfmt() -> anyhow::Result<PathBuf> {
    which::which("rustfmt").context(
        "Couldn't find 'rustfmt' executable. Please install it with `cargo install rustfmt`",
    )
}
