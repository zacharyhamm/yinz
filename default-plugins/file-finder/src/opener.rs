//! Choosing what to open a selected file with.
//!
//! The mapping comes from the plugin's KDL block as one key per extension:
//!
//! ```kdl
//! open_md  "glow -p"
//! open_png "chafa"
//! open_pdf "zathura {}"
//! ```
//!
//! Anything unmatched falls through to Zellij's own editor resolution
//! (`scrollback_editor` → `$EDITOR` → `$VISUAL` → `vi`), which the server applies when
//! it receives a `TerminalAction::OpenFile`. The plugin deliberately never picks an
//! editor itself.

use std::collections::BTreeMap;
use std::path::Path;

/// Config key prefix for a per-extension opener.
const OPEN_PREFIX: &str = "open_";

/// Config key for the opener used when no extension matches.
const OPEN_DEFAULT: &str = "open_default";

/// Placeholder replaced by the selected file's path. If a configured command contains
/// no placeholder, the path is appended as a final argument instead.
const PLACEHOLDER: &str = "{}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opener {
    /// Hand the file to the host, which substitutes the configured editor.
    ZellijEditor,
    /// Run a specific command, already resolved down to argv.
    Command { path: String, args: Vec<String> },
}

/// Pick an opener for `file` out of the plugin's configuration map.
///
/// Extensions are matched case-insensitively, so `open_png` covers `SCREENSHOT.PNG`.
/// A file with no extension, or one whose extension has no `open_<ext>` entry, uses
/// `open_default` if set and Zellij's editor otherwise.
pub fn opener_for(config: &BTreeMap<String, String>, file: &Path) -> Opener {
    let by_extension = file
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!("{}{}", OPEN_PREFIX, ext.to_lowercase()))
        .and_then(|key| config.get(&key));

    let command = match by_extension.or_else(|| config.get(OPEN_DEFAULT)) {
        Some(command) => command,
        None => return Opener::ZellijEditor,
    };

    // An empty or whitespace-only value is how a user disables an inherited entry;
    // treat it as "no opener configured" rather than trying to exec the empty string.
    match build_command(command, file) {
        Some(opener) => opener,
        None => Opener::ZellijEditor,
    }
}

/// Split a configured opener string into argv and place the file path within it.
///
/// Splitting is on whitespace, so an argument containing a space cannot be expressed.
/// This matches the limitation of the server's own `separate_command_arguments`, which
/// parses `scrollback_editor` the same way.
fn build_command(command: &str, file: &Path) -> Option<Opener> {
    let path = file.to_string_lossy().to_string();

    let mut words = command.split_whitespace().map(|word| {
        if word.contains(PLACEHOLDER) {
            word.replace(PLACEHOLDER, &path)
        } else {
            word.to_string()
        }
    });

    let executable = words.next()?;
    let mut args: Vec<String> = words.collect();

    let has_placeholder = command.contains(PLACEHOLDER);
    if !has_placeholder {
        args.push(path);
    }

    Some(Opener::Command {
        path: executable,
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;
    use std::path::PathBuf;

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn command(path: &str, args: &[&str]) -> Opener {
        Opener::Command {
            path: path.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn unconfigured_extension_uses_the_zellij_editor() {
        let config = config(&[("open_md", "glow")]);
        let opener = opener_for(&config, &PathBuf::from("/p/src/main.rs"));
        assert_eq!(opener, Opener::ZellijEditor);
    }

    #[test]
    fn empty_config_uses_the_zellij_editor() {
        let opener = opener_for(&BTreeMap::new(), &PathBuf::from("/p/README.md"));
        assert_eq!(opener, Opener::ZellijEditor);
    }

    #[test]
    fn matching_extension_selects_its_command() {
        let config = config(&[("open_md", "glow -p")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md"));
        assert_eq!(opener, command("glow", &["-p", "/p/README.md"]));
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let config = config(&[("open_png", "chafa")]);
        let opener = opener_for(&config, &PathBuf::from("/p/Shot.PNG"));
        assert_eq!(opener, command("chafa", &["/p/Shot.PNG"]));
    }

    #[test]
    fn placeholder_is_substituted_in_place_rather_than_appended() {
        let config = config(&[("open_pdf", "zathura {} --mode fullscreen")]);
        let opener = opener_for(&config, &PathBuf::from("/p/book.pdf"));
        assert_eq!(
            opener,
            command("zathura", &["/p/book.pdf", "--mode", "fullscreen"])
        );
    }

    #[test]
    fn placeholder_embedded_in_an_argument_is_substituted() {
        let config = config(&[("open_log", "less +F{}")]);
        let opener = opener_for(&config, &PathBuf::from("/p/run.log"));
        assert_eq!(opener, command("less", &["+F/p/run.log"]));
    }

    #[test]
    fn open_default_covers_unmatched_extensions() {
        let config = config(&[("open_md", "glow"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/main.rs"));
        assert_eq!(opener, command("bat", &["/p/main.rs"]));
    }

    #[test]
    fn extension_entry_wins_over_open_default() {
        let config = config(&[("open_md", "glow"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md"));
        assert_eq!(opener, command("glow", &["/p/README.md"]));
    }

    #[test]
    fn file_without_an_extension_uses_open_default() {
        let config = config(&[("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/Makefile"));
        assert_eq!(opener, command("bat", &["/p/Makefile"]));
    }

    #[test]
    fn dotfile_without_an_extension_uses_open_default() {
        // Path::extension() returns None for ".gitignore" — the leading dot marks a
        // hidden file, not an extension.
        let config = config(&[("open_gitignore", "never"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/.gitignore"));
        assert_eq!(opener, command("bat", &["/p/.gitignore"]));
    }

    #[test]
    fn blank_opener_falls_back_to_the_zellij_editor() {
        let config = config(&[("open_md", "   ")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md"));
        assert_eq!(opener, Opener::ZellijEditor);
    }

    #[test]
    fn unrelated_config_keys_are_ignored() {
        let config = config(&[("floating", "true"), ("width", "80%")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md"));
        assert_eq!(opener, Opener::ZellijEditor);
    }

    /// `build_command` documents that a command without a `{}` gets the file path
    /// appended. Whatever whitespace, unicode or word count the configured command has,
    /// the file must end up as the final argument exactly once — otherwise the opener
    /// would be launched against the wrong file, or against no file at all.
    ///
    /// The path is built to be non-empty because that is the function's real domain:
    /// `opener_for` is only ever called with a selection, and `script::parse_selection`
    /// never yields an empty one.
    #[hegel::test]
    fn a_command_without_a_placeholder_takes_the_path_as_its_last_argument(tc: hegel::TestCase) {
        let extension = tc.draw(generators::sampled_from(vec!["rs", "md", "PNG", "Rs"]));
        let stem = tc.draw(generators::text().min_size(1));
        let command = tc.draw(generators::text());

        tc.assume(!command.contains(PLACEHOLDER));
        tc.assume(!command.trim().is_empty());

        let file = PathBuf::from(format!("/p/{}.{}", stem, extension));
        // Keep only the stems that actually leave the extension we configured for.
        tc.assume(file.extension().and_then(|ext| ext.to_str()) == Some(extension));

        let key = format!("{}{}", OPEN_PREFIX, extension.to_lowercase());
        let config = BTreeMap::from([(key, command.clone())]);

        let expected_argv: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
        let path = file.to_string_lossy().to_string();

        match opener_for(&config, &file) {
            Opener::Command {
                path: executable,
                args,
            } => {
                assert_eq!(
                    executable, expected_argv[0],
                    "executable should be the command's first word"
                );
                assert_eq!(
                    args.last(),
                    Some(&path),
                    "the file must be the final argument of {:?}",
                    command
                );
                assert_eq!(
                    args.iter().filter(|arg| **arg == path).count(),
                    1,
                    "the file must be passed exactly once"
                );
                assert_eq!(args.len(), expected_argv.len(), "no words may be dropped");
            },
            Opener::ZellijEditor => panic!(
                "a configured, non-blank opener {:?} should not fall through to the editor",
                command
            ),
        }
    }
}
