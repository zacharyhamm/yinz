//! Check and format shipped Scheme sources with this checkout's native authoring tools.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use xshell::{cmd, Shell};

const PLUGIN_FOLDERS: [&str; 2] = ["yinz-steel-builtins/assets/plugins", "example-plugins"];

fn is_plugin_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("scm" | "ss" | "sld")
    ) && !path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains(".bak."))
}

pub fn plugin_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for folder in PLUGIN_FOLDERS {
        for entry in std::fs::read_dir(crate::project_root().join(folder))? {
            let entry = entry?;
            if entry.file_type()?.is_file() && is_plugin_source(&entry.path()) {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn plugin_sources() -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), files)?;
            } else if kind.is_file() && is_plugin_source(&entry.path()) {
                files.push(entry.path());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    for folder in PLUGIN_FOLDERS {
        walk(&crate::project_root().join(folder), &mut files)?;
    }
    files.sort();
    Ok(files)
}

fn binary(sh: &Shell) -> Result<PathBuf> {
    let cargo = crate::cargo()?;
    cmd!(sh, "{cargo} build --bin yinz")
        .run()
        .context("build Scheme authoring tools")?;
    Ok(crate::project_root().join("target/debug/yinz"))
}

pub fn format(sh: &Shell, check: bool) -> Result<()> {
    let binary = binary(sh)?;
    // Source checks must not load the developer's plugin aliases or repository entries.
    let config = tempfile::tempdir().context("create isolated authoring configuration")?;
    let config_dir = config.path();
    let mut failed = false;
    for file in plugin_sources()? {
        let result = if check {
            cmd!(
                sh,
                "{binary} --config-dir {config_dir} plugins format --file {file} --check"
            )
            .run()
            .map_err(anyhow::Error::from)
        } else {
            // User-facing formatting keeps backups; repository formatting stages them outside
            // the source tree so they cannot be installed as additional builtin assets.
            let staged = tempfile::tempdir()?;
            let source = staged
                .path()
                .join(file.file_name().context("Scheme source filename")?);
            std::fs::copy(&file, &source)?;
            cmd!(
                sh,
                "{binary} --config-dir {config_dir} plugins format --file {source}"
            )
            .run()
            .map_err(anyhow::Error::from)
            .and_then(|_| {
                std::fs::copy(&source, &file)
                    .map(|_| ())
                    .context("write formatted Scheme source")
            })
        };
        if result
            .with_context(|| format!("format {}", file.display()))
            .is_err()
        {
            failed = true;
        }
    }
    anyhow::ensure!(!failed, "plugin formatting failed");
    Ok(())
}

pub fn lint(sh: &Shell) -> Result<()> {
    let binary = binary(sh)?;
    // Source checks must not load the developer's plugin aliases or repository entries.
    let config = tempfile::tempdir().context("create isolated authoring configuration")?;
    let config_dir = config.path();
    let mut failed = false;
    for file in plugin_files()? {
        // Installed-test cases are compiled by the confined test runner with its explicit helpers.
        if file
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().ends_with(".test"))
        {
            continue;
        }
        if cmd!(
            sh,
            "{binary} --config-dir {config_dir} plugins check --file {file}"
        )
        .run()
        .is_err()
        {
            failed = true;
        }
    }
    anyhow::ensure!(!failed, "plugin authoring checks failed");
    Ok(())
}
