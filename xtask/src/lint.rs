//! Handle running `cargo clippy` on the sources.
use crate::{flags, metadata, test::host_target_triple, WorkspaceMember};
use anyhow::{anyhow, Context};
use std::path::{Path, PathBuf};
use xshell::{cmd, Shell};

pub fn lint(sh: &Shell, flags: flags::Lint) -> anyhow::Result<()> {
    let err_context = "failed to run task 'lint'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = check_clippy()
        .and_then(|_| crate::cargo())
        .context(err_context)?;
    let host_triple = host_target_triple(sh).context(err_context)?;

    // Every phase runs even if an earlier one fails, so one noisy crate can't hide the rest.
    // Mirrors `x test`.
    let mut failed: Vec<String> = Vec::new();

    // The plugin crates pin themselves to wasm32-wasip1 in their own `.cargo/config.toml`, and
    // clippy has nothing to say about a target it can't check the host cfg for - so they are
    // linted against the host triple, exactly as `x test` runs them. One invocation for all of
    // them, so shared dependencies are analysed once.
    {
        let plugin_names: Vec<&str> = crate::workspace_members()
            .iter()
            .filter(|m| m.crate_name.contains("plugins"))
            .map(|m| m.crate_name.rsplit_once('/').unwrap().1)
            .collect();

        if !plugin_names.is_empty() {
            println!();
            let msg = ">> Linting plugins";
            crate::status(msg);
            println!("{}", msg);

            let mut cmd = cmd!(sh, "{cargo} clippy --target {host_triple} --all-targets");
            for name in &plugin_names {
                cmd = cmd.args(["-p", name]);
            }
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
                failed.push("plugins".to_string());
            }
        }
    }

    for WorkspaceMember { crate_name, .. } in crate::workspace_members()
        .iter()
        .filter(|m| !m.crate_name.contains("plugins"))
    {
        let _pd = sh.push_dir(Path::new(crate_name));
        println!();
        let msg = format!(">> Linting '{}'", crate_name);
        crate::status(&msg);
        println!("{}", msg);

        let mut cmd = cmd!(sh, "{cargo} clippy --all-targets");
        if flags.no_web {
            match metadata::get_no_web_features(sh, crate_name)
                .context("Failed to check web features")?
            {
                Some(features) if features.is_empty() => {
                    cmd = cmd.arg("--no-default-features");
                },
                Some(features) => {
                    cmd = cmd
                        .arg("--no-default-features")
                        .arg("--features")
                        .arg(features);
                },
                None => {
                    cmd = cmd.arg("--all-features");
                },
            }
        } else {
            cmd = cmd.arg("--all-features");
        }
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
            failed.push(crate_name.to_string());
        }
    }

    if !failed.is_empty() {
        println!();
        let msg = format!(">> Lint failures in: {}", failed.join(", "));
        crate::status(&msg);
        println!("{}", msg);
        return Err(anyhow!("lints failed in: {}", failed.join(", "))).context(err_context);
    }
    Ok(())
}

fn check_clippy() -> anyhow::Result<PathBuf> {
    which::which("cargo-clippy").context(
        "Couldn't find 'cargo-clippy' executable. Please install it with `rustup component add clippy`",
    )
}
