//! The file work behind `yinz plugins`: writing a script plugin into the user's plugin folder
//! and reading its lines back out of the log. Kept apart from the command so it can be tested on
//! a folder of its own, and in this crate rather than the binary's so the integration tests can
//! run the same code the CLI does.

use anyhow::Context;
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use yinz_plugin_api::authoring::is_script_plugin;

/// Check a source file in the supervised compilation worker.
pub fn check_plugin(path: &Path) -> anyhow::Result<()> {
    check_plugin_report(path).map(|_| ())
}

/// Return diagnostics, warnings, inferred permissions and an outline for a source file.
pub fn check_plugin_report(
    path: &Path,
) -> anyhow::Result<yinz_types::authoring::PluginCheckReport> {
    if !is_script_plugin(path) {
        anyhow::bail!(
            "{}",
            check_source(path, "", yinz_types::authoring::PluginCheckMode::Syntax)
                .diagnostics
                .join("\n")
        );
    }
    let source = crate::limits::read_source(path)?;
    let report = check_source(
        path,
        &source,
        yinz_types::authoring::PluginCheckMode::Semantic,
    );
    if !report.diagnostics.is_empty() {
        anyhow::bail!("{}", report.diagnostics.join("\n"));
    }
    Ok(report)
}

/// Analyze only the proposed entrypoint; dependency reads are confined by the semantic checker.
pub fn check_source(
    path: &Path,
    source: &str,
    mode: yinz_types::authoring::PluginCheckMode,
) -> yinz_types::authoring::PluginCheckReport {
    if !is_script_plugin(path) {
        return yinz_types::authoring::PluginCheckReport {
            diagnostics: vec![
                if path.extension().and_then(|ext| ext.to_str()) == Some("lua") {
                    "Lua plugins are no longer supported; migrate to Steel Scheme".into()
                } else {
                    "plugin checks require a .scm, .ss or .sld file".into()
                },
            ],
            ..Default::default()
        };
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if mode != yinz_types::authoring::PluginCheckMode::Syntax && !parent.exists() {
        let checked = (|| -> anyhow::Result<_> {
            let scratch = yinz_runtime::owned_dir::OwnedDir::new(
                &yinz_runtime::consts::YINZ_CACHE_DIR.join("authoring-tmp"),
            )?;
            let staged = scratch.path().join(
                path.file_name()
                    .ok_or_else(|| anyhow::anyhow!("missing plugin filename"))?,
            );
            let mut report = crate::analysis::check(&staged, source, mode);
            for diagnostic in &mut report.diagnostics {
                *diagnostic =
                    diagnostic.replace(&*staged.to_string_lossy(), &path.to_string_lossy());
            }
            Ok(report)
        })();
        return checked.unwrap_or_else(|error| yinz_types::authoring::PluginCheckReport {
            diagnostics: vec![format!("{error:#}")],
            ..Default::default()
        });
    }
    crate::analysis::check(path, source, mode)
}

/// Check a repository entrypoint with the same import boundary as its runtime.
pub fn check_repository(
    installed: &crate::git::InstalledPlugin,
) -> anyhow::Result<yinz_types::authoring::PluginCheckReport> {
    let source = crate::limits::read_source(&installed.entrypoint)?;
    Ok(crate::analysis::check_with_root(
        &installed.entrypoint,
        &source,
        yinz_types::authoring::PluginCheckMode::Semantic,
        Some(&installed.root),
    ))
}

/// A plugin written, and the copy kept of whatever it replaced.
#[derive(Debug, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    /// Where the previous version went, when this write replaced one.
    pub replaced: Option<PathBuf>,
}

/// Write `source` with its explicit script extension (default `.scm`) and one trailing newline, creating the
/// module folder if the name has one. Refuses a name that is not a plugin name, an existing
/// file unless `overwrite`, a symlink in place of the file or its folder, and - when `check`
/// - a source that does not compile.
///
/// An overwrite keeps the version it replaced as `<name>.bak.<extension>`, which is the same thing
/// agent's `plugin_write` does: a rewrite that dropped a feature is then one read away from
/// the file it dropped it from, whichever surface did the rewriting. A backup that cannot be
/// kept refuses the write rather than proceeding without one - see [`keep_backup`].
pub fn write(
    dir: &Path,
    name: &str,
    source: &str,
    overwrite: bool,
    check: bool,
) -> anyhow::Result<Written> {
    let path = yinz_plugin_api::authoring::resolve_plugin_file(dir, name)
        .map_err(|e| anyhow::anyhow!(e))?;
    if source.trim().is_empty() {
        anyhow::bail!("the plugin source is empty");
    }
    let prepared;
    let source = if check {
        let report = check_source(
            &path,
            source,
            yinz_types::authoring::PluginCheckMode::Prepare,
        );
        if !report.diagnostics.is_empty() {
            anyhow::bail!(
                "{} does not compile or pass authoring checks: {}",
                path.display(),
                report.diagnostics.join("\n")
            );
        }
        prepared = report
            .formatted_source
            .ok_or_else(|| anyhow::anyhow!("authoring check returned no formatted source"))?;
        &prepared
    } else {
        source
    };
    let parent = path.parent().expect("a plugin file has a folder");
    if parent.is_symlink() {
        anyhow::bail!(
            "{} is a symlink; refusing to write through it",
            parent.display()
        );
    }
    if path.is_symlink() {
        anyhow::bail!(
            "{} is a symlink; refusing to write through it",
            path.display()
        );
    }
    if path.exists() && !overwrite {
        anyhow::bail!("{} exists; pass --overwrite to replace it", path.display());
    }
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    let contents = format!("{}\n", source.trim_end_matches('\n'));
    // Before the write, and a refusal when it cannot be made: the write is the only thing that
    // destroys the old version, so a backup that silently did not happen would be worse than no
    // backup at all - the file it was meant to hold would already be gone.
    let replaced = match overwrite && path.is_file() {
        true => keep_backup(&path, &contents)?,
        false => None,
    };
    publish_file(&path, contents.as_bytes(), overwrite, None)?;
    Ok(Written { path, replaced })
}

/// Apply exact edits and strict checks before replacing the source.
pub fn edit(
    dir: &Path,
    name: &str,
    edits: &[yinz_plugin_api::authoring::edits::EditPair],
) -> anyhow::Result<Written> {
    let path =
        yinz_plugin_api::authoring::resolve_plugin_file(dir, name).map_err(anyhow::Error::msg)?;
    let original = crate::limits::read_source(&path)?;
    let edited = yinz_plugin_api::authoring::edits::apply_edits(&original, edits)
        .map_err(anyhow::Error::msg)?;
    let report = check_source(
        &path,
        &edited.source,
        yinz_types::authoring::PluginCheckMode::Prepare,
    );
    anyhow::ensure!(
        report.diagnostics.is_empty(),
        "{}",
        report.diagnostics.join("\n")
    );
    let source = report
        .formatted_source
        .ok_or_else(|| anyhow::anyhow!("checker returned no formatted source"))?;
    anyhow::ensure!(
        crate::limits::read_source(&path)? == original,
        "plugin changed during checks; read it again before editing"
    );
    write(dir, name, &source, true, false)
}

/// Format only the named file. Read-only verification never creates backups. Returns the
/// checker's warnings, such as a malformed pane declaration, which do not stop formatting.
pub fn format_file(path: &Path, check: bool) -> anyhow::Result<Vec<String>> {
    if path.is_symlink() || path.parent().is_some_and(Path::is_symlink) {
        anyhow::bail!("refusing to format through a symlink");
    }
    let source = crate::limits::read_source(path)?;
    let report = check_source(
        path,
        &source,
        yinz_types::authoring::PluginCheckMode::Format,
    );
    if !report.diagnostics.is_empty() {
        anyhow::bail!("{}", report.diagnostics.join("\n"));
    }
    let formatted = report
        .formatted_source
        .ok_or_else(|| anyhow::anyhow!("formatter returned no source"))?;
    if source == formatted {
        return Ok(report.warnings);
    }
    if check {
        anyhow::bail!("{} needs formatting", path.display());
    }
    keep_backup(path, &formatted)?;
    publish_file(path, formatted.as_bytes(), true, None)?;
    Ok(report.warnings)
}

fn publish_file(
    path: &Path,
    contents: &[u8],
    overwrite: bool,
    permissions: Option<std::fs::Permissions>,
) -> anyhow::Result<()> {
    let resolved = path
        .canonicalize()
        .or_else(|_| {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .canonicalize()
                .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        })
        .with_context(|| format!("resolve authoring destination {}", path.display()))?;
    if resolved.ancestors().any(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "revisions")
            && ancestor
                .parent()
                .is_some_and(|parent| parent.join("repository.git").is_dir())
    }) {
        anyhow::bail!("managed plugin revisions are immutable; edit a local repository instead");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = match std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => Some(
            file.metadata()
                .with_context(|| format!("failed to inspect {}", path.display()))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to open {} for replacement", path.display()))
        },
    };
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    if let Some(metadata) = &metadata {
        anyhow::ensure!(
            metadata.is_file(),
            "{} is not a regular file",
            path.display()
        );
    }
    if let Some(permissions) =
        permissions.or_else(|| metadata.map(|metadata| metadata.permissions()))
    {
        staged
            .as_file()
            .set_permissions(permissions)
            .with_context(|| format!("failed to preserve permissions for {}", path.display()))?;
    }
    staged
        .write_all(contents)
        .and_then(|_| staged.as_file().sync_all())
        .with_context(|| format!("failed to stage contents for {}", path.display()))?;
    let published = if overwrite {
        staged.persist(path)
    } else {
        staged.persist_noclobber(path)
    };
    published.map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow::anyhow!("{} exists; pass --overwrite to replace it", path.display())
        } else {
            anyhow::anyhow!("failed to publish {}: {error}", path.display())
        }
    })?;
    Ok(())
}

/// Copy `path` to its `.bak` sibling before it is replaced, answering with where it went.
///
/// `None` when there is nothing worth keeping: the file already holds exactly what is about to
/// be written, and copying it would overwrite the backup of the last version that actually
/// differed. Anything else that stops the copy - a `.bak` that is a symlink, an unwritable
/// folder - is an error rather than a shrug, because the caller is about to overwrite the only
/// copy of the file.
fn keep_backup(path: &Path, contents: &str) -> anyhow::Result<Option<PathBuf>> {
    let previous = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("failed to read {} for backup: {e}", path.display()))?;
    if previous == contents.as_bytes() {
        return Ok(None);
    }
    let permissions = std::fs::metadata(path)
        .with_context(|| {
            format!(
                "failed to read permissions of {} for backup",
                path.display()
            )
        })?
        .permissions();
    let mut name = path
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("missing filename"))?
        .to_os_string();
    name.push(".bak");
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    let backup = path.with_file_name(name);
    if backup.is_symlink() {
        anyhow::bail!(
            "{} is a symlink; refusing to keep a backup through it",
            backup.display()
        );
    }
    publish_file(&backup, &previous, true, Some(permissions)).map_err(|e| {
        anyhow::anyhow!(
            "failed to keep a backup of {} as {}: {e}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(Some(backup))
}

/// The last matching log lines, including a registered builtin's exact bare builtin tag.
pub fn log_tail(log_file: &Path, path: &Path, lines: usize) -> anyhow::Result<String> {
    crate::log_target_tail(log_file, &crate::LogTarget::Legacy(path.to_owned()), lines)
}

#[cfg(test)]
#[path = "unit/plugin_authoring.rs"]
mod tests;
