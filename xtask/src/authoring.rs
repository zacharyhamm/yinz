//! Acquire standalone authoring tools at build time; installed checks remain offline.
use anyhow::{Context, Result};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use xshell::{cmd, Shell};
#[path = "../../yinz-plugin-host/src/deno/authoring/bundle.rs"]
pub(crate) mod bundle;

pub fn prepare(sh: &Shell) -> Result<PathBuf> {
    let dir = crate::project_root()
        .join("target/authoring-tools")
        .join(bundle::ID);
    if bundle::verify_dir(&dir).is_ok()
        && ["provenance.json", "LICENSE-oxc", "LICENSE-tsgolint"]
            .iter()
            .all(|name| dir.join(name).is_file())
    {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    let artifacts = bundle::artifacts()?;
    for tool in bundle::TOOLS {
        let artifact = &artifacts[tool];
        let hash = artifact["sha256"].as_str().context("tool hash")?;
        let destination = dir.join(tool);
        if std::fs::read(&destination).is_ok_and(|bytes| bundle::verify(&bytes, hash).is_ok()) {
            continue;
        }
        let download = tempfile::tempdir_in(&dir)?;
        let archive = download.path().join("download.tar.gz");
        let url = artifact["url"].as_str().context("tool URL")?;
        eprintln!(">> Acquiring pinned {tool}");
        cmd!(
            sh,
            "curl --fail --location --retry 2 --max-time 120 --output {archive} {url}"
        )
        .run()?;
        bundle::verify(
            &std::fs::read(&archive)?,
            artifact["archive_sha256"]
                .as_str()
                .context("archive hash")?,
        )?;
        let member = artifact["member"].as_str().context("archive member")?;
        // Extract exactly one verified member to bytes, never arbitrary paths from the archive.
        let bytes = cmd!(sh, "tar -xOzf {archive} {member}").output()?.stdout;
        bundle::verify(&bytes, hash)?;
        let staged = download.path().join(tool);
        std::fs::write(&staged, bytes)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(staged, destination)?;
    }
    std::fs::write(dir.join("provenance.json"), bundle::MANIFEST)?;
    std::fs::write(
        dir.join("LICENSE-oxc"),
        include_str!("../../yinz-plugin-host/src/deno/authoring/LICENSE-oxc"),
    )?;
    std::fs::write(
        dir.join("LICENSE-tsgolint"),
        include_str!("../../yinz-plugin-host/src/deno/authoring/LICENSE-tsgolint"),
    )?;
    bundle::verify_dir(&dir)?;
    Ok(dir)
}

pub fn install(sh: &Shell, builtins: &Path) -> Result<()> {
    let source = prepare(sh)?;
    // The binary supplies its own platform and version data roots through the existing setup API.
    let version = builtins.file_name().context("builtin version")?;
    let data = builtins
        .parent()
        .and_then(Path::parent)
        .context("application data directory")?;
    let dir = data.join("authoring-tools").join(version).join(bundle::ID);
    std::fs::create_dir_all(&dir)?;
    for name in
        bundle::TOOLS
            .into_iter()
            .chain(["provenance.json", "LICENSE-oxc", "LICENSE-tsgolint"])
    {
        let bytes = std::fs::read(source.join(name))?;
        let mut staged = tempfile::NamedTempFile::new_in(&dir)?;
        use std::io::Write;
        staged.write_all(&bytes)?;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(
                if bundle::TOOLS.contains(&name) {
                    0o755
                } else {
                    0o644
                },
            ))?;
        staged.persist(dir.join(name))?;
    }
    bundle::verify_dir(&dir)
}

pub fn plugin_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for folder in ["yinz-deno-builtins/assets/plugins", "example-plugins"] {
        for entry in std::fs::read_dir(crate::project_root().join(folder))? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "ts")
                && !path.to_string_lossy().contains(".bak.")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn format(sh: &Shell, check: bool) -> Result<()> {
    let tool = prepare(sh)?.join("oxfmt");
    let config = crate::project_root().join("yinz-plugin-host/src/deno/authoring/oxfmt.json");
    let files = plugin_files()?;
    let scratch = tempfile::tempdir()?;
    let _pd = sh.push_dir(scratch.path());
    let flag = if check { "--check" } else { "--write" };
    cmd!(
        sh,
        "{tool} --config {config} --disable-nested-config --threads 1 {flag} {files...}"
    )
    .run()
    .context("plugin formatting failed")
}

pub fn lint(sh: &Shell) -> Result<()> {
    prepare(sh)?;
    let cargo = crate::cargo()?;
    cmd!(sh, "{cargo} build --bin yinz").run()?;
    let binary = crate::project_root().join("target/debug/yinz");
    let mut failed = false;
    for file in plugin_files()? {
        // Test modules use yinz:testing and are checked by the embedded test runner.
        if file.to_string_lossy().ends_with(".test.ts") {
            continue;
        }
        if cmd!(sh, "{binary} plugins check --file {file}")
            .run()
            .is_err()
        {
            failed = true;
        }
    }
    if failed {
        anyhow::bail!("strict plugin authoring checks failed");
    }
    Ok(())
}
