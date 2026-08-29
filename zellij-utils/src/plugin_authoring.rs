//! Naming a plugin in the user's plugin folder, for everything that writes one: `zellij plugins
//! write`, and hocus's `plugin_write`, which spells the same rules in wasm-flavoured paths.
//!
//! A name is `keys` or `folder/module` - a file, or one folder deep for a module a plugin
//! `require`s - and becomes `<dir>/<name>.lua`. Everything else is refused before anything
//! touches the disk, so a name cannot be used to build a path outside the folder.

use std::path::{Component, Path, PathBuf};

/// The longest plugin name accepted, so a name cannot be used to build an unreasonable path.
pub const MAX_PLUGIN_NAME_LEN: usize = 64;

/// `<dir>/<name>.lua`, or why `name` is not a plugin name.
pub fn resolve_plugin_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is required and must not be empty".to_owned());
    }
    let name = name.strip_suffix(".lua").unwrap_or(name);
    if name.len() > MAX_PLUGIN_NAME_LEN {
        return Err(format!(
            "a plugin name may be at most {MAX_PLUGIN_NAME_LEN} characters"
        ));
    }
    let components: Vec<&str> = name.split('/').collect();
    if components.len() > 2 {
        return Err(format!(
            "{name:?} is not a plugin name: at most one folder, for a module a plugin requires"
        ));
    }
    for component in &components {
        if component.is_empty() || *component == "." || *component == ".." {
            return Err(format!("{name:?} is not a plugin name"));
        }
        // One rule that refuses `\`, NUL, spaces and every traversal at once.
        if !component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(
                "a plugin name is a file name, or folder/file: letters, digits, '.', '_' and \
                 '-' only"
                    .to_owned(),
            );
        }
        // Never option-shaped, whatever a shell later does with it.
        if component.starts_with('-') {
            return Err("a plugin name may not start with '-'".to_owned());
        }
    }
    if !dir.is_absolute() {
        return Err("the plugin folder is not an absolute path".to_owned());
    }
    let path = dir.join(format!("{name}.lua"));
    // Belt and braces: the rules above admit no `..`, and this says so a second way.
    if path.components().any(|c| matches!(c, Component::ParentDir)) || !path.starts_with(dir) {
        return Err(format!("{name:?} is not a plugin name"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_file_or_one_folder_deep_and_nothing_else() {
        let dir = Path::new("/home/someone/.config/zellij/plugins");
        assert_eq!(
            resolve_plugin_file(dir, "keys").unwrap(),
            dir.join("keys.lua")
        );
        assert_eq!(
            resolve_plugin_file(dir, " keys.lua ").unwrap(),
            dir.join("keys.lua")
        );
        assert_eq!(
            resolve_plugin_file(dir, "app/ui").unwrap(),
            dir.join("app/ui.lua")
        );
        for bad in [
            "",
            "../x",
            "a/../b",
            "a/b/c",
            "-x",
            "a b",
            "/etc/passwd",
            "x\0",
        ] {
            assert!(resolve_plugin_file(dir, bad).is_err(), "{bad:?}");
        }
        assert!(resolve_plugin_file(dir, &"a".repeat(MAX_PLUGIN_NAME_LEN + 1)).is_err());
        assert!(resolve_plugin_file(Path::new("plugins"), "keys").is_err());
    }
}
