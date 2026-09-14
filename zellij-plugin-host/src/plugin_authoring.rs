//! The file work behind `zellij plugins`: writing a script plugin into the user's plugin folder
//! and reading its lines back out of the log. Kept apart from the command so it can be tested on
//! a folder of its own, and in this crate rather than the binary's so the integration tests can
//! run the same code the CLI does.

use std::path::{Path, PathBuf};
use zellij_plugin_api::authoring::is_deno_plugin;

/// Check a file with Deno's semantic module checker.
pub fn check_plugin(path: &Path) -> anyhow::Result<()> {
    check_plugin_report(path).map(|_| ())
}

/// Return diagnostics, warnings, inferred permissions and an outline for a source file.
pub fn check_plugin_report(
    path: &Path,
) -> anyhow::Result<zellij_types::authoring::PluginCheckReport> {
    if !is_deno_plugin(path) {
        anyhow::bail!(
            "{}",
            check_source(path, "", zellij_types::authoring::PluginCheckMode::Syntax)
                .diagnostics
                .join("\n")
        );
    }
    let source = crate::deno::read_authoring_source(path)?;
    let report = check_source(
        path,
        &source,
        zellij_types::authoring::PluginCheckMode::Semantic,
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
    mode: zellij_types::authoring::PluginCheckMode,
) -> zellij_types::authoring::PluginCheckReport {
    if !is_deno_plugin(path) {
        return zellij_types::authoring::PluginCheckReport {
            diagnostics: vec![
                if path.extension().and_then(|ext| ext.to_str()) == Some("lua") {
                    "Lua plugins are no longer supported; migrate to TypeScript or Wasm".into()
                } else {
                    "plugin checks require a .ts, .mts, .js or .mjs file".into()
                },
            ],
            ..Default::default()
        };
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if mode != zellij_types::authoring::PluginCheckMode::Syntax && !parent.exists() {
        let checked = (|| -> anyhow::Result<_> {
            let scratch = zellij_runtime::owned_dir::OwnedDir::new(
                &zellij_runtime::consts::ZELLIJ_CACHE_DIR.join("authoring-tmp"),
            )?;
            let staged = scratch.path().join(
                path.file_name()
                    .ok_or_else(|| anyhow::anyhow!("missing plugin filename"))?,
            );
            let mut report = crate::deno::analysis::check(&staged, source, mode);
            for diagnostic in &mut report.diagnostics {
                *diagnostic =
                    diagnostic.replace(&*staged.to_string_lossy(), &path.to_string_lossy());
            }
            Ok(report)
        })();
        return checked.unwrap_or_else(|error| zellij_types::authoring::PluginCheckReport {
            diagnostics: vec![format!("{error:#}")],
            ..Default::default()
        });
    }
    crate::deno::analysis::check(path, source, mode)
}

/// A plugin written, and the copy kept of whatever it replaced.
#[derive(Debug, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    /// Where the previous version went, when this write replaced one.
    pub replaced: Option<PathBuf>,
}

/// Write `source` with its explicit script extension (default `.ts`) and one trailing newline, creating the
/// module folder if the name has one. Refuses a name that is not a plugin name, an existing
/// file unless `overwrite`, a symlink in place of the file or its folder, and - when `check`
/// - a source that does not compile.
///
/// An overwrite keeps the version it replaced as `<name>.bak.<extension>`, which is the same thing
/// hocus's `plugin_write` does: a rewrite that dropped a feature is then one read away from
/// the file it dropped it from, whichever surface did the rewriting. A backup that cannot be
/// kept refuses the write rather than proceeding without one - see [`keep_backup`].
pub fn write(
    dir: &Path,
    name: &str,
    source: &str,
    overwrite: bool,
    check: bool,
) -> anyhow::Result<Written> {
    let path = zellij_plugin_api::authoring::resolve_plugin_file(dir, name)
        .map_err(|e| anyhow::anyhow!(e))?;
    if source.trim().is_empty() {
        anyhow::bail!("the plugin source is empty");
    }
    let prepared;
    let source = if check {
        let report = check_source(
            &path,
            source,
            zellij_types::authoring::PluginCheckMode::Prepare,
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
    let replaced = match path.is_file() {
        true => keep_backup(&path, &contents)?,
        false => None,
    };
    std::fs::write(&path, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(Written { path, replaced })
}

/// Format only the named file. Read-only verification never creates backups.
pub fn format_file(path: &Path, check: bool) -> anyhow::Result<()> {
    if path.is_symlink() || path.parent().is_some_and(Path::is_symlink) {
        anyhow::bail!("refusing to format through a symlink");
    }
    let source = crate::deno::read_authoring_source(path)?;
    let report = check_source(
        path,
        &source,
        zellij_types::authoring::PluginCheckMode::Format,
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
    std::fs::write(path, formatted)?;
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
    if std::fs::read_to_string(path).is_ok_and(|previous| previous == contents) {
        return Ok(None);
    }
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
    std::fs::copy(path, &backup).map_err(|e| {
        anyhow::anyhow!(
            "failed to keep a backup of {} as {}: {e}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(Some(backup))
}

/// The last matching log lines, including a registered builtin's exact bare Deno tag.
pub fn log_tail(log_file: &Path, path: &Path, lines: usize) -> anyhow::Result<String> {
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let deno_builtin_tag = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| zellij_config::consts::builtin_deno_plugin(stem).is_some())
        .map(|stem| format!("[deno:{stem}]"));
    let log = match std::fs::read_to_string(log_file) {
        Ok(log) => log,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("no zellij log at {}", log_file.display())
        },
        Err(e) => anyhow::bail!("failed to read {}: {e}", log_file.display()),
    };
    let matching: Vec<&str> = log
        .lines()
        .filter(|line| {
            line.contains(&file_name)
                || deno_builtin_tag
                    .as_ref()
                    .is_some_and(|tag| line.contains(tag))
        })
        .collect();
    let start = matching.len().saturating_sub(lines.max(1));
    let mut out = matching[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_plugin_report_carries_deno_warnings() {
        let dir = tempfile::tempdir().unwrap();
        // A computed permission request typechecks and cannot be inferred: a warning, not an error.
        let deno = dir.path().join("computed.ts");
        std::fs::write(
            &deno,
            "import zellij, {type Permission} from 'zellij:plugin';\nconst wanted: Permission[] = ['ReadFiles'];\nexport default { load(): void { zellij.requestPermission(wanted); }, render(): string { return ''; } };\n",
        )
        .unwrap();
        let report = check_plugin_report(&deno).unwrap();
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert!(
            report.warnings.iter().any(|w| w.contains("computed")),
            "{report:?}"
        );
        std::fs::write(&deno, "export const = ;").unwrap();
        assert!(check_plugin_report(&deno).is_err());
    }

    #[test]
    fn deno_writes_check_syntax_and_preserve_previous_versions() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["js", "mjs", "ts", "mts"] {
            let name = format!("app/keys.{extension}");
            // Dependencies must exist before authoring their importer.
            assert!(write(dir.path(), &name, "import './missing.js';", false, true).is_err());
            assert!(!dir.path().join(&name).exists());
            std::fs::create_dir_all(dir.path().join("app")).unwrap();
            std::fs::write(
                dir.path().join("app/dep.js"),
                "export const dependency = 1;\n",
            )
            .unwrap();
            let source = "import \"./dep.js\";\nexport default { before: true };";
            let written = write(dir.path(), &name, source, false, true).unwrap();
            assert_eq!(written.path, dir.path().join(&name));
            assert!(
                check_plugin(&written.path).is_ok(),
                "semantic check resolves existing imports"
            );
            assert!(write(dir.path(), &name, "export default {};", false, true).is_err());
            assert!(write(dir.path(), &name, "export default {", true, true).is_err());
            assert_eq!(
                std::fs::read_to_string(&written.path).unwrap(),
                format!("{source}\n")
            );
            let replaced = write(dir.path(), &name, "export default {};", true, true).unwrap();
            let backup = dir.path().join(format!("app/keys.bak.{extension}"));
            assert_eq!(replaced.replaced, Some(backup.clone()));
            assert_eq!(
                std::fs::read_to_string(&backup).unwrap(),
                format!("{source}\n")
            );
            assert!(
                write(dir.path(), &name, "export default {};\n\n", true, true)
                    .unwrap()
                    .replaced
                    .is_none()
            );
            assert_eq!(
                std::fs::read_to_string(&backup).unwrap(),
                format!("{source}\n")
            );
            let recursive = format!(
                "export default {}0{};",
                "[".repeat(10_000),
                "]".repeat(10_000)
            );
            assert!(write(dir.path(), &name, &recursive, true, true).is_err());
            assert_eq!(
                std::fs::read_to_string(&written.path).unwrap(),
                "export default {};\n"
            );
            assert_eq!(
                std::fs::read_to_string(&backup).unwrap(),
                format!("{source}\n")
            );
            #[cfg(unix)]
            {
                let linked = format!("linked.{extension}");
                std::os::unix::fs::symlink(&written.path, dir.path().join(&linked)).unwrap();
                assert!(write(dir.path(), &linked, "export default {};", true, true)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink"));
                let protected = format!("protected.{extension}");
                std::fs::copy(dir.path().join("app/dep.js"), dir.path().join("dep.js")).unwrap();
                let original = write(dir.path(), &protected, source, false, true).unwrap();
                std::os::unix::fs::symlink(
                    &written.path,
                    dir.path().join(format!("protected.bak.{extension}")),
                )
                .unwrap();
                assert!(
                    write(dir.path(), &protected, "export default {};", true, true)
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
        let typed = write(
            dir.path(),
            "typed.mts",
            "const value: number = 1; export { value };",
            false,
            true,
        )
        .unwrap();
        assert!(typed.path.ends_with("typed.mts"));
        assert!(write(
            dir.path(),
            "typed.js",
            "const value: number = 1;",
            false,
            true
        )
        .is_err());
        write(dir.path(), "broken.ts", "export default {", false, false).unwrap();
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
            zellij_types::authoring::PluginCheckMode::Syntax
        )
        .diagnostics[0]
            .contains("Lua plugins are no longer supported"));
    }

    #[test]
    fn a_write_compiles_first_and_never_clobbers_or_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let written = write(&plugins, "keys", "export default {};\n\n", false, true).unwrap();
        assert_eq!(written.path, plugins.join("keys.ts"));
        assert!(written.replaced.is_none(), "nothing was there to replace");
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            "export default {};\n"
        );
        let refused = write(&plugins, "keys", "export default {};", false, true).unwrap_err();
        assert!(refused.to_string().contains("--overwrite"), "{refused}");
        let replaced = write(&plugins, "keys", "export default { x: 1 };", true, true).unwrap();
        assert_eq!(
            replaced.replaced,
            Some(plugins.join("keys.bak.ts")),
            "an overwrite keeps what it replaced"
        );
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.ts")).unwrap(),
            "export default {};\n"
        );
        // A rewrite that changes nothing must not overwrite the backup: what that would throw
        // away is the last version that actually differed.
        let identical =
            write(&plugins, "keys", "export default { x: 1 };\n\n", true, true).unwrap();
        assert!(identical.replaced.is_none(), "nothing was replaced");
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.ts")).unwrap(),
            "export default {};\n",
            "the backup still holds the version before this one"
        );
        let refused = write(&plugins, "broken", "export default {", false, true).unwrap_err();
        assert!(
            refused.to_string().contains("does not compile"),
            "{refused}"
        );
        assert!(!plugins.join("broken.ts").exists());
        write(&plugins, "broken", "export default {", false, false).unwrap();
        let module = write(&plugins, "app/ui", "export default {};", false, true).unwrap();
        assert_eq!(module.path, plugins.join("app").join("ui.ts"));
        assert!(write(&plugins, "../escape", "export default {};", false, true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), plugins.join("linked")).unwrap();
            let refused =
                write(&plugins, "linked/x", "export default {};", false, true).unwrap_err();
            assert!(refused.to_string().contains("symlink"), "{refused}");
            // A backup that cannot be kept refuses the write: the alternative is destroying the
            // only copy of the file and saying nothing about it.
            write(
                &plugins,
                "linked-bak",
                "export default { first: 1 };",
                false,
                true,
            )
            .unwrap();
            std::os::unix::fs::symlink(
                dir.path().join("elsewhere.ts"),
                plugins.join("linked-bak.bak.ts"),
            )
            .unwrap();
            let refused = write(
                &plugins,
                "linked-bak",
                "export default { second: 2 };",
                true,
                true,
            )
            .unwrap_err();
            assert!(refused.to_string().contains("backup"), "{refused}");
            assert_eq!(
                std::fs::read_to_string(plugins.join("linked-bak.ts")).unwrap(),
                "export default { first: 1 };\n",
                "the file it could not back up is still there"
            );
            assert!(!dir.path().join("elsewhere.ts").exists());
        }
    }

    #[test]
    fn the_log_tail_is_the_plugins_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("zellij.log");
        std::fs::write(
            &log,
            "a [deno:keys.ts] one\nb other\nc [deno:keys.ts] two\nd [deno:keys.ts] three\n",
        )
        .unwrap();
        let tail = log_tail(&log, Path::new("/p/keys.ts"), 2).unwrap();
        assert_eq!(tail, "c [deno:keys.ts] two\nd [deno:keys.ts] three\n");
        // A builtin logs under its bare tag; asking for it by name finds those lines and not
        // a written plugin's that happens to share the prefix.
        std::fs::write(
            &log,
            "a [deno:agent-policy] hook\nb [deno:agent-policy-mine.ts] x\nc [deno:agent-policy] two\n",
        )
        .unwrap();
        assert_eq!(
            log_tail(&log, Path::new("/p/agent-policy.ts"), 5).unwrap(),
            "a [deno:agent-policy] hook\nc [deno:agent-policy] two\n"
        );
        assert_eq!(log_tail(&log, Path::new("/p/other.ts"), 5).unwrap(), "");
        assert!(log_tail(&dir.path().join("missing"), Path::new("/p/keys.ts"), 5).is_err());
    }
}
