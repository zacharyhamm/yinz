//! Handle running `cargo clippy` on the sources.
use crate::flags;
use anyhow::{anyhow, Context};
use std::path::PathBuf;
use xshell::{cmd, Shell};

pub fn lint(sh: &Shell, flags: flags::Lint) -> anyhow::Result<()> {
    let err_context = "failed to run task 'lint'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = check_clippy()
        .and_then(|_| crate::cargo())
        .context(err_context)?;

    println!();
    let msg = ">> Linting the workspace";
    crate::status(msg);
    println!("{}", msg);

    // One invocation, so shared dependencies are analysed once. As in `x test`, the plugin
    // crates' wasm pin lives in their own `.cargo/config.toml` and Cargo reads config from the
    // invocation directory upward, so running from the root lints them against the host.
    //
    // Unlike `x test` this asks for `--all-features` when it can: lint wants the widest surface
    // available, and clippy's artifacts live in their own fingerprint namespace anyway, so the
    // wider graph costs nothing that could otherwise have been shared.
    let mut cmd = cmd!(sh, "{cargo} clippy --workspace --all-targets");
    for excluded in crate::test::EXCLUDED {
        cmd = cmd.args(["--exclude", excluded]);
    }
    cmd = cmd.arg("--all-features");
    if flags.fix {
        cmd = cmd.args(["--fix", "--allow-dirty", "--allow-staged"]);
    }
    if cmd
        .arg("--")
        .arg("-D")
        .arg("warnings")
        .args(&flags.args)
        .run()
        .is_err()
    {
        let msg = ">> Lint failures";
        crate::status(msg);
        println!("\n{}", msg);
        return Err(anyhow!("lints failed")).context(err_context);
    }
    Ok(())
}

fn check_clippy() -> anyhow::Result<PathBuf> {
    which::which("cargo-clippy").context(
        "Couldn't find 'cargo-clippy' executable. Please install it with `rustup component add clippy`",
    )
}
