use crate::flags;
use anyhow::Context;
use std::ffi::OsString;
use xshell::{cmd, Shell};

fn hocus_args(args: &[OsString]) -> Vec<OsString> {
    args.iter()
        .map(|arg| match arg.to_str() {
            Some("--baseline") => OsString::from("--baseline-lenient"),
            Some(arg) if arg.starts_with("--baseline=") => {
                OsString::from(arg.replacen("--baseline=", "--baseline-lenient=", 1))
            },
            _ => arg.clone(),
        })
        .collect()
}

pub fn bench(sh: &Shell, flags: flags::Bench) -> anyhow::Result<()> {
    let err_context = "failed to run task 'bench'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    let msg = ">> Running render throughput benchmarks";
    crate::status(msg);
    println!("{}", msg);

    // No plugin build here: the benchmarks drive the server's render pipeline directly
    // and never load a wasm plugin. The hocus one measures its transcript wrapping natively for
    // the same reason - it is a relative figure for comparing revisions, not a real frame time.
    //
    // The trailing `--` is what makes `cargo x bench -- --save-baseline main` reach criterion
    // rather than cargo, which rejects criterion's flags as its own.
    //
    // Both run even if the first fails, so one red bench does not hide the other.
    let server = cmd!(
        sh,
        "{cargo} bench -p zellij-server --bench multi_pane_render --"
    )
    .args(&flags.args)
    .run()
    .context(err_context);

    // `main` predates the hocus benchmark, so its first comparison has no matching baseline.
    // Criterion's lenient mode still compares any hocus cases that do exist and measures new
    // ones without making the valid server comparison fail.
    let hocus_args = hocus_args(&flags.args);
    let hocus = cmd!(sh, "{cargo} bench -p hocus --bench transcript --")
        .args(&hocus_args)
        .run()
        .context(err_context);

    server?;
    hocus?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hocus_uses_lenient_named_baselines() {
        for (args, expected) in [
            (
                vec!["--baseline", "main", "streaming_frame"],
                "--baseline-lenient",
            ),
            (
                vec!["--baseline=main", "streaming_frame"],
                "--baseline-lenient=main",
            ),
        ] {
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            let actual = hocus_args(&args);
            assert_eq!(actual[0], expected);
            assert_eq!(actual.last().unwrap(), "streaming_frame");
        }
    }
}
