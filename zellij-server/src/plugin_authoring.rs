//! The file work behind `zellij plugins`: writing a Lua plugin into the user's plugin folder
//! and reading its lines back out of the log. Kept apart from the command so it can be tested on
//! a folder of its own, and in this crate rather than the binary's so the integration tests can
//! run the same code the CLI does.

use std::path::{Path, PathBuf};

/// Write `source` as `<dir>/<name>.lua` with exactly one trailing newline, creating the
/// module folder if the name has one. Refuses a name that is not a plugin name, an existing
/// file unless `overwrite`, a symlink in place of the file or its folder, and - when `check`
/// - a source that does not compile.
pub fn write(
    dir: &Path,
    name: &str,
    source: &str,
    overwrite: bool,
    check: bool,
) -> anyhow::Result<PathBuf> {
    let path = zellij_utils::plugin_authoring::resolve_plugin_file(dir, name)
        .map_err(|e| anyhow::anyhow!(e))?;
    if source.trim().is_empty() {
        anyhow::bail!("the plugin source is empty");
    }
    if check {
        let chunk = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        crate::check_lua_plugin(source, &chunk)
            .map_err(|e| anyhow::anyhow!("{} does not compile: {e}", chunk))?;
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
    std::fs::write(&path, contents)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(path)
}

/// The last `lines` lines of `log_file` about the plugin at `path`: the runtime tags a
/// plugin's `zellij.log` and `print` output `[lua:<path>]`, and its loader errors name the
/// path, so both are matched on the file name.
pub fn log_tail(log_file: &Path, path: &Path, lines: usize) -> anyhow::Result<String> {
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
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
        .filter(|line| line.contains(&file_name))
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
    fn a_write_compiles_first_and_never_clobbers_or_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        let written = write(&plugins, "keys", "return {}\n\n", false, true).unwrap();
        assert_eq!(written, plugins.join("keys.lua"));
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "return {}\n");
        let refused = write(&plugins, "keys", "return {}", false, true).unwrap_err();
        assert!(refused.to_string().contains("--overwrite"), "{refused}");
        write(&plugins, "keys", "return { x = 1 }", true, true).unwrap();
        let refused = write(&plugins, "broken", "return {", false, true).unwrap_err();
        assert!(
            refused.to_string().contains("does not compile"),
            "{refused}"
        );
        assert!(!plugins.join("broken.lua").exists());
        write(&plugins, "broken", "return {", false, false).unwrap();
        let module = write(&plugins, "app/ui", "return {}", false, true).unwrap();
        assert_eq!(module, plugins.join("app").join("ui.lua"));
        assert!(write(&plugins, "../escape", "return {}", false, true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), plugins.join("linked")).unwrap();
            let refused = write(&plugins, "linked/x", "return {}", false, true).unwrap_err();
            assert!(refused.to_string().contains("symlink"), "{refused}");
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
        assert_eq!(log_tail(&log, Path::new("/p/other.lua"), 5).unwrap(), "");
        assert!(log_tail(&dir.path().join("missing"), Path::new("/p/keys.lua"), 5).is_err());
    }
}
