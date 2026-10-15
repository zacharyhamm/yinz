//! Naming a plugin in the user's plugin folder, for everything that writes one: `yinz plugins
//! write`, and agent's `plugin_write`, which spells the same rules in host paths.
//!
//! A name is `keys` or `folder/module` - a file, or one folder deep for a module a plugin
//! imports - and defaults to `<dir>/<name>.scm`. The CLI also accepts explicit Scheme
//! filenames. Path validation prevents a name from escaping the folder.

use std::path::{Component, Path, PathBuf};
use strum::IntoEnumIterator;
use yinz_types::data::PermissionType;
/// The store's own columns, and the prefix that makes a namespace a shared one. Re-exported
/// here because a plugin that writes plugins - and the guide's tests - check a source against
/// the same two rules the store itself enforces, and a second copy of either would drift.
pub use {
    yinz_types::plugin_data::RESERVED_ATTR_NAMES, yinz_types::plugin_data::SHARED_NAMESPACE_PREFIX,
};

/// The longest plugin name accepted, so a name cannot be used to build an unreasonable path.
pub const MAX_PLUGIN_NAME_LEN: usize = 64;

/// Whether a path names a Scheme plugin source.
pub fn is_script_plugin(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("scm" | "ss" | "sld")
    )
}

/// `<dir>/<name>.scm`, or an explicit Scheme filename, after validating the name.
pub fn resolve_plugin_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is required and must not be empty".to_owned());
    }
    let (name, extension) = match name.rsplit_once('.') {
        Some((stem, ext @ ("scm" | "ss" | "sld"))) => (stem, ext),
        Some((_, "lua")) => {
            return Err("Lua plugins are no longer supported; migrate to Steel Scheme".into())
        },
        _ => (name, "scm"),
    };
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
    let path = dir.join(format!("{name}.{extension}"));
    // Belt and braces: the rules above admit no `..`, and this says so a second way.
    if path.components().any(|c| matches!(c, Component::ParentDir)) || !path.starts_with(dir) {
        return Err(format!("{name:?} is not a plugin name"));
    }
    Ok(path)
}

/// The backup a write keeps of the file it replaces: `keys` becomes `keys.bak`, which is
/// `keys.bak.scm` on disk and `plugin_read name=keys.bak` from a tool.
///
/// A sibling rather than a dotfile or a folder of versions: one previous version is what a
/// rewrite that lost a feature needs, and naming it as a plugin name means every rule that
/// already governs a plugin path governs this one too - including the refusal to write through
/// a symlink.
pub fn backup_name(name: &str) -> String {
    let name = name.trim();
    if let Some((stem, extension @ ("scm" | "ss" | "sld"))) = name.rsplit_once('.') {
        return format!("{stem}.bak.{extension}");
    }
    format!("{name}.bak")
}

/// Whether `name` names a backup rather than a plugin, so a listing can leave it out.
pub fn is_backup_name(name: &str) -> bool {
    let name = name.trim();
    let name = if is_script_plugin(Path::new(name)) {
        name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name)
    } else {
        name
    };
    name.ends_with(".bak")
}

fn parse_permission(name: &str) -> Option<PermissionType> {
    let pascal: String = name
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    pascal.parse::<PermissionType>().ok()
}

/// Permissions as a plugin author spells them: `read_files`, not `ReadFiles`.
pub fn permission_names(permissions: &[PermissionType]) -> Vec<String> {
    permissions.iter().map(|p| permission_name(*p)).collect()
}

/// One permission, spelled the way a plugin asks for it.
pub fn permission_name(permission: PermissionType) -> String {
    let mut out = String::new();
    for (i, c) in permission.to_string().chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Every permission a plugin may ask for, in the spelling it asks for it.
pub fn all_permission_names() -> Vec<String> {
    PermissionType::iter().map(permission_name).collect()
}

/// [`nearest_permission`] for a name in either spelling, answering the permission itself so
/// the caller can spell the suggestion its own way (`ReadFiles` for Scheme).
pub fn nearest_permission_type(typo: &str) -> Option<PermissionType> {
    let mut snake = String::new();
    for (i, c) in typo.trim().chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            snake.push('_');
        }
        snake.push(c.to_ascii_lowercase());
    }
    nearest_permission(&snake).and_then(|name| parse_permission(&name))
}

/// The known permission closest to `typo`, when one is close enough to name in a suggestion.
///
/// Ranked by shared words before edit distance, because these names are compounds and the word
/// someone got right says more than the letters they got wrong: `shared_data` is one word away
/// from `shared_plugin_data` and six letters away from `plugin_data`, and it is the first of
/// those that they meant.
fn nearest_permission(typo: &str) -> Option<String> {
    let words: Vec<&str> = typo.split('_').filter(|word| !word.is_empty()).collect();
    let mut best: Option<(usize, usize, String)> = None;
    for candidate in all_permission_names() {
        let shared = candidate
            .split('_')
            .filter(|word| words.contains(word))
            .count();
        let distance = edit_distance(typo, &candidate);
        let better = best
            .as_ref()
            .is_none_or(|(s, d, _)| shared > *s || (shared == *s && distance < *d));
        if better {
            best = Some((shared, distance, candidate));
        }
    }
    // No word in common and far enough away, and a suggestion is a guess: `weather` is not a
    // misspelling of anything.
    best.filter(|(shared, distance, name)| *shared > 0 || *distance * 2 <= name.len())
        .map(|(_, _, name)| name)
}

/// Levenshtein distance, for suggesting the permission someone meant.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            let next = (row[j] + 1).min(row[j + 1] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

pub use yinz_types::authoring::{
    OutlineEntry, OutlineKind, PluginCheckMode, PluginCheckReport, PluginTestKind,
    PluginTestOptions, PluginTestReport, PluginTestResult,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_names_and_backups_preserve_extensions_and_name_guards() {
        let dir = Path::new("/plugins");
        for extension in ["scm", "ss", "sld"] {
            let name = format!("app/keys.{extension}");
            let path = resolve_plugin_file(dir, &format!(" {name} ")).unwrap();
            assert_eq!(path, dir.join(&name));
            assert!(is_script_plugin(&path));
            let backup = backup_name(&name);
            assert_eq!(backup, format!("app/keys.bak.{extension}"));
            assert_eq!(
                resolve_plugin_file(dir, &backup).unwrap(),
                dir.join(&backup)
            );
            assert!(is_backup_name(&backup));
            assert!(!is_backup_name(&name));
            for invalid in [
                "../keys",
                "app/../keys",
                "/keys",
                "-keys",
                "app/-keys",
                "a/b/keys",
                "",
                ".",
                "..",
                "app/",
            ] {
                assert!(
                    resolve_plugin_file(dir, &format!("{invalid}.{extension}")).is_err(),
                    "{invalid}.{extension}"
                );
            }
            assert!(resolve_plugin_file(
                dir,
                &format!("{}.{extension}", "a".repeat(MAX_PLUGIN_NAME_LEN + 1))
            )
            .is_err());
        }
        assert_eq!(
            resolve_plugin_file(dir, "keys").unwrap(),
            dir.join("keys.scm")
        );
        assert_eq!(
            resolve_plugin_file(dir, "keys.jsx").unwrap(),
            dir.join("keys.jsx.scm")
        );
    }

    #[test]
    fn a_name_is_a_file_or_one_folder_deep_and_nothing_else() {
        for name in ["keys.lua", " app/keys.lua "] {
            assert!(resolve_plugin_file(Path::new("/plugins"), name)
                .unwrap_err()
                .contains("Lua plugins are no longer supported"));
        }
        let dir = Path::new("/home/someone/.config/yinz/plugins");
        assert_eq!(
            resolve_plugin_file(dir, "keys").unwrap(),
            dir.join("keys.scm")
        );
        assert_eq!(
            resolve_plugin_file(dir, " keys.scm ").unwrap(),
            dir.join("keys.scm")
        );
        assert_eq!(
            resolve_plugin_file(dir, "app/ui").unwrap(),
            dir.join("app/ui.scm")
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

    #[test]
    fn a_backup_is_a_plugin_name_that_a_listing_leaves_out() {
        assert_eq!(backup_name("keys"), "keys.bak");
        assert_eq!(backup_name(" keys.scm "), "keys.bak.scm");
        assert_eq!(backup_name("app/ui"), "app/ui.bak");
        let dir = Path::new("/home/someone/.config/yinz/plugins");
        assert_eq!(
            resolve_plugin_file(dir, &backup_name("keys")).unwrap(),
            dir.join("keys.bak.scm")
        );
        assert!(is_backup_name("keys.bak.scm"));
        assert!(is_backup_name("app/ui.bak"));
        assert!(!is_backup_name("keys.scm"));
    }
}

pub mod edits;
