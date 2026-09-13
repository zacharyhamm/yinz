//! Shared by the host and xtask; no package manager or PATH lookup at runtime.
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const ID: &str = "oxlint-1.82.0-oxfmt-0.67.0-tsgolint-7.0.2001";
pub const TOOLS: [&str; 3] = ["oxlint", "oxfmt", "tsgolint"];
pub const MANIFEST: &str = include_str!("provenance.json");

pub fn target() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        arch => bail!("plugin authoring tools are not bundled for {arch}"),
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" if cfg!(target_env = "musl") => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        os => bail!("plugin authoring tools are not bundled for {os}"),
    };
    Ok(format!("{arch}-{os}"))
}

pub fn artifacts() -> Result<serde_json::Value> {
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST)?;
    let target = target()?;
    manifest["targets"]
        .get(&target)
        .cloned()
        .context("unsupported authoring tools target")
}

pub fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    if Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        != expected
    {
        bail!("bundled authoring tool hash mismatch; run cargo x build or reinstall Zellij");
    }
    Ok(())
}

pub fn verify_dir(dir: &Path) -> Result<()> {
    let artifacts = artifacts()?;
    for tool in TOOLS {
        let path = dir.join(tool);
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "missing bundled authoring tool {}: run cargo x build or reinstall Zellij",
                path.display()
            )
        })?;
        verify(
            &bytes,
            artifacts[tool]["sha256"]
                .as_str()
                .context("missing tool hash")?,
        )?;
    }
    Ok(())
}
