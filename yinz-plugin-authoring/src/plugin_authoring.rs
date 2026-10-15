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

/// Format only the named file. Read-only verification never creates backups.
pub fn format_file(path: &Path, check: bool) -> anyhow::Result<()> {
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
        return Ok(());
    }
    if check {
        anyhow::bail!("{} needs formatting", path.display());
    }
    keep_backup(path, &formatted)?;
    publish_file(path, formatted.as_bytes(), true, None)?;
    Ok(())
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
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unchecked_overwrite_backs_up_source_permissions() {
        for (existing_backup, mode) in [(true, 0o600), (false, 0o754)] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("keys.scm");
            let backup = dir.path().join("keys.bak.scm");
            let original = "(define before #t)\n";
            let replacement = "(define after #t)\n";
            std::fs::write(&path, original).unwrap();
            let permissions = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(&path, permissions).unwrap();
            let permissions = std::fs::metadata(&path).unwrap().permissions();
            if existing_backup {
                std::fs::write(&backup, "older backup").unwrap();
                std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o644)).unwrap();
            }

            let written = write(dir.path(), "keys", replacement, true, false).unwrap();

            assert_eq!(written.replaced.as_ref(), Some(&backup));
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
            assert_eq!(
                std::fs::metadata(&backup).unwrap().permissions(),
                permissions
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions(), permissions);
        }
    }

    #[test]
    fn formatting_backs_up_source_permissions() {
        for (existing_backup, mode) in [(true, 0o600), (false, 0o754)] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("keys.scm");
            let backup = dir.path().join("keys.bak.scm");
            let original = "(define before #t)";
            std::fs::write(&path, original).unwrap();
            let permissions = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(&path, permissions).unwrap();
            let permissions = std::fs::metadata(&path).unwrap().permissions();
            if existing_backup {
                std::fs::write(&backup, "older backup").unwrap();
                std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o644)).unwrap();
            }

            format_file(&path, false).unwrap();

            assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
            assert_eq!(
                std::fs::metadata(&backup).unwrap().permissions(),
                permissions
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "(define before #t)\n"
            );
            assert_eq!(std::fs::metadata(&path).unwrap().permissions(), permissions);
        }
    }

    #[test]
    fn plugin_replacement_preserves_hard_linked_sources_and_backups() {
        let dir = tempfile::tempdir().unwrap();
        let original = "(define before #t)\n";
        let replacement = "(define after #t)\n";
        let written = write(dir.path(), "keys", original, false, false).unwrap();
        let alias = dir.path().join("alias.scm");
        let backup = dir.path().join("keys.bak.scm");
        std::fs::hard_link(&written.path, &alias).unwrap();
        std::fs::hard_link(&written.path, &backup).unwrap();

        write(dir.path(), "keys", replacement, true, false).unwrap();

        assert_eq!(std::fs::read_to_string(&written.path).unwrap(), replacement);
        assert_eq!(std::fs::read_to_string(&alias).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
    }

    #[test]
    fn concurrent_plugin_creators_never_replace_the_winner() {
        let dir = tempfile::tempdir().unwrap();
        let gate = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            let writers: Vec<_> = (0..16)
                .map(|index| {
                    let gate = &gate;
                    let dir = dir.path();
                    scope.spawn(move || {
                        let source = format!("(define value {index})\n");
                        gate.wait();
                        write(dir, "keys", &source, false, false).map(|_| source)
                    })
                })
                .collect();
            let successful: Vec<_> = writers
                .into_iter()
                .filter_map(|writer| writer.join().unwrap().ok())
                .collect();
            assert_eq!(successful.len(), 1);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("keys.scm")).unwrap(),
                successful[0]
            );
            assert!(!dir.path().join("keys.bak.scm").exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        });
    }

    #[test]
    fn check_plugin_report_carries_computed_permission_warnings() {
        let dir = tempfile::tempdir().unwrap();
        // A computed permission request typechecks and cannot be inferred: a warning, not an error.
        let plugin = dir.path().join("computed.scm");
        std::fs::write(
            &plugin,
            r#"(define wanted '("ReadFiles")) (define (load config) (yinz-wait-for-permissions wanted))"#,
        )
        .unwrap();
        let report = check_plugin_report(&plugin).unwrap();
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert!(
            report.warnings.iter().any(|w| w.contains("computed")),
            "{report:?}"
        );
        std::fs::write(&plugin, "(define value").unwrap();
        assert!(check_plugin_report(&plugin).is_err());
    }

    #[test]
    fn scheme_writes_check_before_replacing_and_preserve_backups() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["scm", "ss", "sld"] {
            let name = format!("example.{extension}");
            let source = "; original\n(define (render rows columns) \"hello\")\n";
            let written = write(dir.path(), &name, source, false, true).unwrap();
            assert_eq!(std::fs::read_to_string(&written.path).unwrap(), source);
            assert!(write(dir.path(), &name, "(define (broken", true, true).is_err());
            assert_eq!(std::fs::read_to_string(&written.path).unwrap(), source);
            let replacement = "; replacement\n(define (render rows columns) \"updated\")\n";
            let written = write(dir.path(), &name, replacement, true, true).unwrap();
            assert_eq!(std::fs::read_to_string(&written.path).unwrap(), replacement);
            assert_eq!(
                std::fs::read_to_string(written.replaced.unwrap()).unwrap(),
                source
            );
            assert!(check_plugin_report(&written.path)
                .unwrap()
                .outline
                .iter()
                .any(|entry| entry.text == "(render rows columns)"));
        }
    }

    #[test]
    fn scheme_import_writes_preserve_previous_versions() {
        for extension in ["scm", "ss", "sld"] {
            assert_scheme_writes_preserve_previous_versions(extension);
        }
    }

    fn assert_scheme_writes_preserve_previous_versions(extension: &str) {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("app/keys.{extension}");
        // Dependencies must exist before authoring their importer.
        assert!(write(
            dir.path(),
            &name,
            r#"(require "./missing.scm")"#,
            false,
            true
        )
        .is_err());
        assert!(!dir.path().join(&name).exists());
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(
            dir.path().join("app/dep.scm"),
            "(provide dependency) (define dependency 1)\n",
        )
        .unwrap();
        let source = r#"(require "./dep.scm")
(define before #t)"#;
        let written = write(dir.path(), &name, source, false, true).unwrap();
        assert_eq!(written.path, dir.path().join(&name));
        assert!(
            check_plugin(&written.path).is_ok(),
            "semantic check resolves existing imports"
        );
        assert!(write(dir.path(), &name, "(define value 1)", false, true).is_err());
        assert!(write(dir.path(), &name, "(define value", true, true).is_err());
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            format!("{source}\n")
        );
        let replaced = write(dir.path(), &name, "(define value 1)", true, true).unwrap();
        let backup = dir.path().join(format!("app/keys.bak.{extension}"));
        assert_eq!(replaced.replaced, Some(backup.clone()));
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            format!("{source}\n")
        );
        assert!(write(dir.path(), &name, "(define value 1)\n\n", true, true)
            .unwrap()
            .replaced
            .is_none());
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            format!("{source}\n")
        );
        let recursive = format!("{}0{}", "(".repeat(10_000), ")".repeat(10_000));
        assert!(write(dir.path(), &name, &recursive, true, true).is_err());
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            "(define value 1)\n"
        );
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            format!("{source}\n")
        );
        #[cfg(unix)]
        {
            let linked = format!("linked.{extension}");
            std::os::unix::fs::symlink(&written.path, dir.path().join(&linked)).unwrap();
            assert!(write(dir.path(), &linked, "(define value 1)", true, true)
                .unwrap_err()
                .to_string()
                .contains("symlink"));
            let protected = format!("protected.{extension}");
            std::fs::copy(dir.path().join("app/dep.scm"), dir.path().join("dep.scm")).unwrap();
            let original = write(dir.path(), &protected, source, false, true).unwrap();
            std::os::unix::fs::symlink(
                &written.path,
                dir.path().join(format!("protected.bak.{extension}")),
            )
            .unwrap();
            assert!(
                write(dir.path(), &protected, "(define value 1)", true, true)
                    .unwrap_err()
                    .to_string()
                    .contains("backup")
            );
            assert_eq!(
                std::fs::read_to_string(original.path).unwrap(),
                format!("{source}\n")
            );
        }
    }

    #[test]
    fn unchecked_writes_preserve_invalid_source_for_later_editing() {
        let dir = tempfile::tempdir().unwrap();
        let written = write(dir.path(), "broken.scm", "(define value", false, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(written.path).unwrap(),
            "(define value\n"
        );
    }

    #[test]
    fn lua_names_are_rejected_even_without_a_syntax_check() {
        let dir = tempfile::tempdir().unwrap();
        for check in [false, true] {
            let error = write(dir.path(), "old.lua", "return {}", false, check).unwrap_err();
            assert!(error
                .to_string()
                .contains("Lua plugins are no longer supported"));
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(check_plugin(&dir.path().join("missing.lua"))
            .unwrap_err()
            .to_string()
            .contains("Lua plugins are no longer supported"));
        assert!(check_source(
            Path::new("old.lua"),
            "return {}",
            yinz_types::authoring::PluginCheckMode::Syntax
        )
        .diagnostics[0]
            .contains("Lua plugins are no longer supported"));
    }

    #[test]
    fn a_write_compiles_first_and_never_clobbers_or_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let written = write(&plugins, "keys", "(define value 1)\n\n", false, true).unwrap();
        assert_eq!(written.path, plugins.join("keys.scm"));
        assert!(written.replaced.is_none(), "nothing was there to replace");
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            "(define value 1)\n"
        );
        let refused = write(&plugins, "keys", "(define value 1)", false, true).unwrap_err();
        assert!(refused.to_string().contains("--overwrite"), "{refused}");
        let replaced = write(&plugins, "keys", "(define x 1)", true, true).unwrap();
        assert_eq!(
            replaced.replaced,
            Some(plugins.join("keys.bak.scm")),
            "an overwrite keeps what it replaced"
        );
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.scm")).unwrap(),
            "(define value 1)\n"
        );
        // A rewrite that changes nothing must not overwrite the backup: what that would throw
        // away is the last version that actually differed.
        let identical = write(&plugins, "keys", "(define x 1)\n\n", true, true).unwrap();
        assert!(identical.replaced.is_none(), "nothing was replaced");
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.scm")).unwrap(),
            "(define value 1)\n",
            "the backup still holds the version before this one"
        );
        let refused = write(&plugins, "broken", "(define value", false, true).unwrap_err();
        assert!(
            refused.to_string().contains("does not compile"),
            "{refused}"
        );
        assert!(!plugins.join("broken.scm").exists());
        write(&plugins, "broken", "(define value", false, false).unwrap();
        let module = write(&plugins, "app/ui", "(define value 1)", false, true).unwrap();
        assert_eq!(module.path, plugins.join("app").join("ui.scm"));
        assert!(write(&plugins, "../escape", "(define value 1)", false, true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), plugins.join("linked")).unwrap();
            let refused = write(&plugins, "linked/x", "(define value 1)", false, true).unwrap_err();
            assert!(refused.to_string().contains("symlink"), "{refused}");
            // A backup that cannot be kept refuses the write: the alternative is destroying the
            // only copy of the file and saying nothing about it.
            write(&plugins, "linked-bak", "(define first 1)", false, true).unwrap();
            std::os::unix::fs::symlink(
                dir.path().join("elsewhere.scm"),
                plugins.join("linked-bak.bak.scm"),
            )
            .unwrap();
            let refused =
                write(&plugins, "linked-bak", "(define second 2)", true, true).unwrap_err();
            assert!(refused.to_string().contains("backup"), "{refused}");
            assert_eq!(
                std::fs::read_to_string(plugins.join("linked-bak.scm")).unwrap(),
                "(define first 1)\n",
                "the file it could not back up is still there"
            );
            assert!(!dir.path().join("elsewhere.scm").exists());
        }
    }

    #[test]
    fn the_log_tail_is_the_plugins_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("yinz.log");
        std::fs::write(
            &log,
            "a [steel:keys.scm] one\nb other\nc [steel:keys.scm] two\nd [steel:keys.scm] three\n",
        )
        .unwrap();
        let tail = log_tail(&log, Path::new("/p/keys.scm"), 2).unwrap();
        assert_eq!(tail, "c [steel:keys.scm] two\nd [steel:keys.scm] three\n");
        // A builtin logs under its bare tag; asking for it by name finds those lines and not
        // a written plugin's that happens to share the prefix.
        std::fs::write(
            &log,
            "a [steel:agent-policy] hook\nb [steel:agent-policy-mine.scm] x\nc [steel:agent-policy] two\n",
        )
        .unwrap();
        assert_eq!(
            log_tail(&log, Path::new("/p/agent-policy.scm"), 5).unwrap(),
            "a [steel:agent-policy] hook\nc [steel:agent-policy] two\n"
        );
        assert_eq!(log_tail(&log, Path::new("/p/other.scm"), 5).unwrap(), "");
        assert!(log_tail(&dir.path().join("missing"), Path::new("/p/keys.scm"), 5).is_err());
    }
}
