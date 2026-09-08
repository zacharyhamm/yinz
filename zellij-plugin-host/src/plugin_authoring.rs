//! The file work behind `zellij plugins`: writing a script plugin into the user's plugin folder
//! and reading its lines back out of the log. Kept apart from the command so it can be tested on
//! a folder of its own, and in this crate rather than the binary's so the integration tests can
//! run the same code the CLI does.

use std::path::{Path, PathBuf};
use zellij_plugin_api::authoring::is_deno_plugin;

/// Check a file using its runtime: Lua compilation or Deno's semantic module check.
pub fn check_plugin(path: &Path) -> anyhow::Result<()> {
    if is_deno_plugin(path) {
        crate::deno::check_plugin(path)
    } else {
        let source = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        let chunk = path.file_name().unwrap_or_default().to_string_lossy();
        crate::check_lua_plugin(&source, &chunk).map_err(|e| anyhow::anyhow!(e))
    }
}

/// A plugin written, and the copy kept of whatever it replaced.
#[derive(Debug, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    /// Where the previous version went, when this write replaced one.
    pub replaced: Option<PathBuf>,
}

/// Write `source` with its explicit script extension (default `.lua`) and one trailing newline, creating the
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
    if check {
        let chunk = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_deno_plugin(&path) {
            crate::deno::check_syntax(&path, source)
        } else {
            crate::check_lua_plugin(source, &chunk).map_err(|e| anyhow::anyhow!(e))
        }
        .map_err(|e| anyhow::anyhow!("{} does not compile: {e:#}", chunk))?;
    }
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
        true => keep_backup(dir, name, &path, &contents)?,
        false => None,
    };
    std::fs::write(&path, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(Written { path, replaced })
}

/// Copy `path` to its `.bak` sibling before it is replaced, answering with where it went.
///
/// `None` when there is nothing worth keeping: the file already holds exactly what is about to
/// be written, and copying it would overwrite the backup of the last version that actually
/// differed. Anything else that stops the copy - a `.bak` that is a symlink, an unwritable
/// folder - is an error rather than a shrug, because the caller is about to overwrite the only
/// copy of the file.
fn keep_backup(
    dir: &Path,
    name: &str,
    path: &Path,
    contents: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if std::fs::read_to_string(path).is_ok_and(|previous| previous == contents) {
        return Ok(None);
    }
    let backup = zellij_plugin_api::authoring::backup_name(name);
    let backup = zellij_plugin_api::authoring::resolve_plugin_file(dir, &backup)
        .map_err(|e| anyhow::anyhow!("failed to name a backup of {}: {e}", path.display()))?;
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

/// The last `lines` lines of `log_file` about the plugin at `path`: the runtime tags a
/// plugin's `zellij.log` and `print` output `[lua:<path>]`, and its loader errors name the
/// path, so both are matched on the file name. A builtin's path is its bare tag - `agent-policy`,
/// no extension - so the tag spelt with the file's stem is matched too, and `zellij plugins log
/// agent-policy` reads a shipped hook's lines the same way it reads a written one's.
pub fn log_tail(log_file: &Path, path: &Path, lines: usize) -> anyhow::Result<String> {
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let builtin_tag = path
        .file_stem()
        .map(|stem| format!("[lua:{}]", stem.to_string_lossy()))
        .unwrap_or_default();
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
            line.contains(&file_name) || (!is_deno_plugin(path) && line.contains(&builtin_tag))
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
    fn deno_writes_check_syntax_and_preserve_previous_versions() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["js", "mjs", "ts", "mts"] {
            let name = format!("app/keys.{extension}");
            // An unresolved import is syntactically valid: writes must not require its module yet.
            let source = "import './missing.js';\nexport default {};";
            let written = write(dir.path(), &name, source, false, true).unwrap();
            assert_eq!(written.path, dir.path().join(&name));
            assert!(
                check_plugin(&written.path).is_err(),
                "semantic check must resolve imports"
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
            #[cfg(unix)]
            {
                let linked = format!("linked.{extension}");
                std::os::unix::fs::symlink(&written.path, dir.path().join(&linked)).unwrap();
                assert!(write(dir.path(), &linked, "export default {};", true, true)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink"));
                let protected = format!("protected.{extension}");
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
    fn deno_logs_do_not_include_same_stem_lua_builtins() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("zellij.log");
        std::fs::write(&log, "[lua:keys] lua\n[deno:/plugins/keys.ts] one\n[deno:/plugins/other.ts] other\n[deno:/plugins/keys.ts] two\n").unwrap();
        assert_eq!(
            log_tail(&log, Path::new("/plugins/keys.ts"), 2).unwrap(),
            "[deno:/plugins/keys.ts] one\n[deno:/plugins/keys.ts] two\n"
        );
    }

    #[test]
    fn a_write_compiles_first_and_never_clobbers_or_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let written = write(&plugins, "keys", "return {}\n\n", false, true).unwrap();
        assert_eq!(written.path, plugins.join("keys.lua"));
        assert!(written.replaced.is_none(), "nothing was there to replace");
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            "return {}\n"
        );
        let refused = write(&plugins, "keys", "return {}", false, true).unwrap_err();
        assert!(refused.to_string().contains("--overwrite"), "{refused}");
        let replaced = write(&plugins, "keys", "return { x = 1 }", true, true).unwrap();
        assert_eq!(
            replaced.replaced,
            Some(plugins.join("keys.bak.lua")),
            "an overwrite keeps what it replaced"
        );
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.lua")).unwrap(),
            "return {}\n"
        );
        // A rewrite that changes nothing must not overwrite the backup: what that would throw
        // away is the last version that actually differed.
        let identical = write(&plugins, "keys", "return { x = 1 }\n\n", true, true).unwrap();
        assert!(identical.replaced.is_none(), "nothing was replaced");
        assert_eq!(
            std::fs::read_to_string(plugins.join("keys.bak.lua")).unwrap(),
            "return {}\n",
            "the backup still holds the version before this one"
        );
        let refused = write(&plugins, "broken", "return {", false, true).unwrap_err();
        assert!(
            refused.to_string().contains("does not compile"),
            "{refused}"
        );
        assert!(!plugins.join("broken.lua").exists());
        write(&plugins, "broken", "return {", false, false).unwrap();
        let module = write(&plugins, "app/ui", "return {}", false, true).unwrap();
        assert_eq!(module.path, plugins.join("app").join("ui.lua"));
        assert!(write(&plugins, "../escape", "return {}", false, true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), plugins.join("linked")).unwrap();
            let refused = write(&plugins, "linked/x", "return {}", false, true).unwrap_err();
            assert!(refused.to_string().contains("symlink"), "{refused}");
            // A backup that cannot be kept refuses the write: the alternative is destroying the
            // only copy of the file and saying nothing about it.
            write(&plugins, "linked-bak", "return { first = 1 }", false, true).unwrap();
            std::os::unix::fs::symlink(
                dir.path().join("elsewhere.lua"),
                plugins.join("linked-bak.bak.lua"),
            )
            .unwrap();
            let refused =
                write(&plugins, "linked-bak", "return { second = 2 }", true, true).unwrap_err();
            assert!(refused.to_string().contains("backup"), "{refused}");
            assert_eq!(
                std::fs::read_to_string(plugins.join("linked-bak.lua")).unwrap(),
                "return { first = 1 }\n",
                "the file it could not back up is still there"
            );
            assert!(!dir.path().join("elsewhere.lua").exists());
        }
    }

    #[test]
    fn the_log_tail_is_the_plugins_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("zellij.log");
        std::fs::write(
            &log,
            "a [lua:keys.lua] one\nb other\nc [lua:keys.lua] two\nd [lua:keys.lua] three\n",
        )
        .unwrap();
        let tail = log_tail(&log, Path::new("/p/keys.lua"), 2).unwrap();
        assert_eq!(tail, "c [lua:keys.lua] two\nd [lua:keys.lua] three\n");
        // A builtin logs under its bare tag; asking for it by name finds those lines and not
        // a written plugin's that happens to share the prefix.
        std::fs::write(
            &log,
            "a [lua:agent-policy] hook\nb [lua:agent-policy-mine.lua] x\nc [lua:agent-policy] two\n",
        )
        .unwrap();
        assert_eq!(
            log_tail(&log, Path::new("/p/agent-policy.lua"), 5).unwrap(),
            "a [lua:agent-policy] hook\nc [lua:agent-policy] two\n"
        );
        assert_eq!(log_tail(&log, Path::new("/p/other.lua"), 5).unwrap(), "");
        assert!(log_tail(&dir.path().join("missing"), Path::new("/p/keys.lua"), 5).is_err());
    }
}
