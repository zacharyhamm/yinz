use crate::flags;
use anyhow::Context;
use xshell::{cmd, Shell};

pub fn bench(sh: &Shell, flags: flags::Bench) -> anyhow::Result<()> {
    let err_context = "failed to run task 'bench'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo().context(err_context)?;

    let msg = ">> Running render throughput benchmarks";
    crate::status(msg);
    println!("{}", msg);

    // No `ensure_plugin_assets` here: the benchmarks drive the server's render pipeline directly
    // and never load a wasm plugin.
    //
    // The trailing `--` is what makes `cargo x bench -- --save-baseline main` reach criterion
    // rather than cargo, which rejects criterion's flags as its own.
    cmd!(
        sh,
        "{cargo} bench -p zellij-server --bench multi_pane_render --"
    )
    .args(&flags.args)
    .run()
    .context(err_context)?;

    Ok(())
}
