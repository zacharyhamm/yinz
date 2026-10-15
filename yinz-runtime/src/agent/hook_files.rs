//! Installation of provider-owned hook files. Agent-specific content comes from Scheme providers.
use crate::errors::prelude::*;
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use yinz_types::agent::provider::{AgentHookArtifact, AgentHookBase};

pub fn path(hook: &AgentHookArtifact, home: &Path, config: &Path) -> PathBuf {
    match hook.base {
        AgentHookBase::Config => config,
        AgentHookBase::Home => home,
    }
    .join(&hook.path)
}

/// Placeholder quoting is explicit: JSON for JavaScript/TypeScript and POSIX for shell.
pub fn render(hook: &AgentHookArtifact, binary: &Path, token_env: &str) -> Result<String> {
    let binary = binary.to_string_lossy();
    Ok(hook
        .content
        .replace(
            "__YINZ_BIN__",
            &serde_json::to_string(binary.as_ref()).context("quote hook binary")?,
        )
        .replace(
            "__YINZ_SHELL__",
            &format!("'{}'", binary.replace('\'', "'\\''")),
        )
        .replace(
            "__TOKEN_ENV__",
            &serde_json::to_string(token_env).context("quote pane token name")?,
        ))
}

pub fn read_owned(path: &Path, marker: &str) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!("{} is not a regular hook file", path.display())
        },
        Ok(metadata) if metadata.len() > 1024 * 1024 => {
            bail!("{} exceeds hook file size limit", path.display())
        },
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect existing hook file"),
    }
    let text = std::fs::read_to_string(path).context("read existing hook file")?;
    if !text.contains(marker) {
        bail!(
            "{} was not written by this agent provider; leaving it alone",
            path.display()
        );
    }
    Ok(Some(text))
}

pub fn install(path: &Path, hook: &AgentHookArtifact, content: &str) -> Result<()> {
    let existing = read_owned(path, &hook.marker)?;
    let parent = path.parent().context("hook file has no directory")?;
    std::fs::create_dir_all(parent).context("create hook directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).context("stage hook file")?;
    staged
        .write_all(content.as_bytes())
        .context("write hook file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(if hook.executable {
                0o755
            } else {
                0o644
            }))
            .context("set hook permissions")?;
    }
    if read_owned(path, &hook.marker)? != existing {
        bail!("{} changed while installing the hook", path.display());
    }
    if let Some(existing) = existing {
        let mut backup_name = path
            .file_name()
            .context("hook file has no name")?
            .to_os_string();
        backup_name.push(".bak");
        let backup = path.with_file_name(backup_name);
        read_owned(&backup, &hook.marker)?;
        let mut saved = tempfile::NamedTempFile::new_in(parent).context("stage hook backup")?;
        saved
            .write_all(existing.as_bytes())
            .context("write hook backup")?;
        saved
            .persist(&backup)
            .map_err(|error| error.error)
            .context("replace hook backup")?;
    }
    staged
        .persist(path)
        .map_err(|error| error.error)
        .context("replace hook file")?;
    Ok(())
}

pub fn uninstall(path: &Path, hook: &AgentHookArtifact) -> Result<()> {
    if read_owned(path, &hook.marker)?.is_some() {
        std::fs::remove_file(path).context("remove agent hook")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/hook_files_tests.rs"]
mod tests;
