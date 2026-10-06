use crate::flags;
use anyhow::Context;
use xshell::{cmd, Shell};
pub fn bench(sh: &Shell, flags: flags::Bench) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;
    crate::status(">> Running render throughput benchmarks");
    cmd!(
        sh,
        "{cargo} bench -p yinz-server --bench multi_pane_render --"
    )
    .args(&flags.args)
    .run()
    .context("failed to run task 'bench'")
}
