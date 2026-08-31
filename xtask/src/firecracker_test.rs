//! Real Firecracker coverage. Kept ignored so ordinary tests need no KVM or Linux tooling.

use crate::flags;
use anyhow::Context;
use std::{ffi::OsString, fs, path::PathBuf};
use xshell::{cmd, Shell};

pub fn main(sh: &Shell, flags: flags::FirecrackerTest) -> anyhow::Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        anyhow::bail!("the Firecracker test currently requires x86_64 Linux");
    }
    match (flags.build, flags.test, flags.clean) {
        (false, false, false) => anyhow::bail!("one of --build, --test or --clean is required"),
        (_, _, true) if flags.build || flags.test => {
            anyhow::bail!("--clean cannot be combined with --build or --test")
        },
        (true, true, _) => anyhow::bail!("--build and --test are mutually exclusive"),
        (_, _, true) => clean(),
        (true, _, _) => build(sh),
        (_, true, _) => test(sh, flags.args),
    }
}

fn dir() -> PathBuf {
    crate::target_dir().join("firecracker-test")
}

fn build(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;
    let target = crate::ci::musl_target();

    println!(">> Building the provider and static guest binary");
    cmd!(sh, "{cargo} build --bin zellij-firecracker").run()?;
    cmd!(sh, "{cargo} build --release --target {target} --bin zellij").run()?;

    let test_dir = dir();
    for name in ["config", "data", "cache", "state", "runtime"] {
        fs::create_dir_all(test_dir.join(name))?;
    }
    let provider = crate::target_dir().join("debug/zellij-firecracker");
    let guest = crate::target_dir().join(target).join("release/zellij");
    let image = test_dir.join("data/zellij/firecracker");
    let project = crate::project_root();
    cmd!(
        sh,
        "{provider} image build --name zellij-dev --project {project} --zellij {guest} --output {image}"
    )
    .envs(test_env())
    .run()
    .context("failed to build the Firecracker test image")?;

    let network = serde_json::json!({
        "bridge": "zfctest",
        "tap_prefix": "zftest-",
        "subnet": "172.31.0.0/24",
        "taps": 2,
        "dns": "1.1.1.1"
    });
    fs::write(
        test_dir.join("network.json"),
        serde_json::to_vec_pretty(&network)?,
    )?;
    println!(">> Ready. Run 'cargo x firecracker-test --test'.");
    Ok(())
}

fn test(sh: &Shell, args: Vec<OsString>) -> anyhow::Result<()> {
    let test_dir = dir();
    let config = test_dir.join("config/zellij/firecracker.json");
    if !config.exists() {
        anyhow::bail!(
            "{} is missing; run 'cargo x firecracker-test --build' first",
            config.display()
        );
    }
    if !std::path::Path::new("/dev/kvm").exists() {
        anyhow::bail!("/dev/kvm is missing");
    }

    let _pd = sh.push_dir(crate::project_root());
    let cargo = crate::cargo()?;
    cmd!(sh, "{cargo} build --bin zellij-firecracker").run()?;
    let script = r#"set -eu
ip link set lo up
ip link add zfctest type bridge
ip addr add 172.31.0.1/24 dev zfctest
ip link set zfctest up
for tap in zftest-00 zftest-01; do
  ip tuntap add dev "$tap" mode tap
  ip link set "$tap" master zfctest
  ip link set "$tap" up
done
exec "$@""#;
    cmd!(sh, "unshare --user --map-root-user --net -- sh -c {script} firecracker-test {cargo} test -p zellij-server --test firecracker_vm -- --ignored --nocapture --test-threads 1")
        .args(args)
        .envs(test_env())
        .run()
        .map_err(anyhow::Error::new)
        .context("the Firecracker VM test failed")
}

fn clean() -> anyhow::Result<()> {
    let test_dir = dir();
    if test_dir.exists() {
        fs::remove_dir_all(&test_dir)
            .with_context(|| format!("failed to remove {}", test_dir.display()))?;
    }
    println!(">> Removed {}.", test_dir.display());
    Ok(())
}

fn test_env() -> Vec<(OsString, OsString)> {
    let test_dir = dir();
    [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ]
    .into_iter()
    .map(|(key, value)| (OsString::from(key), test_dir.join(value).into_os_string()))
    .chain([(
        OsString::from("ZELLIJ_FIRECRACKER_NETWORK_CONFIG"),
        test_dir.join("network.json").into_os_string(),
    )])
    .chain([(
        OsString::from("ZELLIJ_SOCKET_DIR"),
        OsString::from(format!("/tmp/zfc-test-{}", std::process::id())),
    )])
    .collect()
}
