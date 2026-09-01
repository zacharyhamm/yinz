//! Real Firecracker coverage. Kept ignored so ordinary tests need no KVM or Linux tooling.

use crate::flags;
use anyhow::Context;
use std::{ffi::OsString, fs, path::PathBuf, process::Command};
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
    let provider = crate::target_dir().join("debug/zellij-firecracker");
    let network = TemporaryNetwork::start(sh, provider, test_dir.join("network.json"))?;
    let test_result = cmd!(sh, "{cargo} test -p zellij-server --test firecracker_vm -- --ignored --nocapture --test-threads 1")
        .args(args)
        .envs(test_env())
        .run()
        .map_err(anyhow::Error::new)
        .context("the Firecracker VM test failed");
    let cleanup_result = network.stop(sh);
    test_result?;
    cleanup_result
}

struct TemporaryNetwork {
    provider: PathBuf,
    config: PathBuf,
    active: bool,
}

impl TemporaryNetwork {
    fn start(sh: &Shell, provider: PathBuf, config: PathBuf) -> anyhow::Result<Self> {
        cmd!(sh, "sudo -v").run()?;
        let uid = cmd!(sh, "id -u").read()?;
        let uid = uid.trim();
        fs::write(
            &config,
            serde_json::to_vec_pretty(&test_network(std::process::id()))?,
        )?;
        cmd!(
            sh,
            "sudo {provider} network apply --uid {uid} --config {config}"
        )
        .run()
        .context("failed to create the temporary Firecracker test network")?;
        Ok(Self {
            provider,
            config,
            active: true,
        })
    }

    fn stop(mut self, sh: &Shell) -> anyhow::Result<()> {
        let provider = &self.provider;
        let config = &self.config;
        let result = cmd!(sh, "sudo {provider} network remove --config {config}")
            .run()
            .context("failed to remove the temporary Firecracker test network");
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}

impl Drop for TemporaryNetwork {
    fn drop(&mut self) {
        if self.active {
            let _ = Command::new("sudo")
                .arg("-n")
                .arg(&self.provider)
                .args(["network", "remove", "--config"])
                .arg(&self.config)
                .status();
        }
    }
}

fn test_network(pid: u32) -> serde_json::Value {
    serde_json::json!({
        "bridge": format!("zfcb{pid:x}"),
        "tap_prefix": format!("zfct{pid:x}-"),
        "subnet": format!("10.{}.{}.0/24", (pid >> 8) & 0xff, pid & 0xff),
        "taps": 2,
        "dns": "1.1.1.1"
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_network_names_fit_linux_interfaces_and_use_a_process_subnet() {
        let network = test_network(u32::MAX);
        assert!(network["bridge"].as_str().unwrap().len() <= 15);
        assert!(network["tap_prefix"].as_str().unwrap().len() + 2 <= 15);
        assert_eq!(network["subnet"], "10.255.255.0/24");
    }
}
