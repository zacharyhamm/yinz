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
//! Anything unmatched falls through to Yinz's own editor resolution
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
    YinzEditor,
    /// Run a specific command, already resolved down to argv.
    Command { path: String, args: Vec<String> },
    Plugin {
        destination: String,
        message: String,
    },
}

/// Pick an opener for `file` out of the plugin's configuration map.
///
/// Extensions are matched case-insensitively, so `open_png` covers `SCREENSHOT.PNG`.
/// Structured extension entries precede legacy extension entries, followed by
/// structured and legacy defaults, then the editor. Validate the entire structured
/// map before choosing, so invalid configuration never silently runs a fallback.
pub fn opener_for(config: &BTreeMap<String, String>, file: &Path) -> Result<Opener, String> {
    // Validate every entry, including entries unrelated to this selection.
    let mut structured = BTreeMap::new();
    if let Some(json) = config
        .get("openers_json")
        .filter(|json| !json.trim().is_empty())
    {
        let entries: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(json).map_err(|error| format!("openers_json: {error}"))?;
        for (extension, value) in entries {
            let entry = || -> Result<Opener, String> {
                if extension.is_empty()
                    || extension.starts_with('.')
                    || extension != extension.to_lowercase()
                    || extension.contains('\0')
                {
                    return Err(
                        "expected a lowercase, nonempty extension without a leading dot".into(),
                    );
                }
                if value.is_null() {
                    return Ok(Opener::YinzEditor);
                }
                let entry: Structured = serde_json::from_value(value)
                    .map_err(|error| format!("expected argv or plugin/message fields: {error}"))?;
                match entry {
                    Structured::Argv { argv } => {
                        let Some((executable, arguments)) = argv.split_first() else {
                            return Err("argv must contain an executable".into());
                        };
                        nonblank(executable, "executable")?;
                        if argv.iter().any(|arg| arg.contains('\0')) {
                            return Err("argv must not contain NUL".into());
                        }
                        let raw_path = file.to_string_lossy();
                        let mut args: Vec<_> = arguments
                            .iter()
                            .map(|arg| arg.replace(PLACEHOLDER, &raw_path))
                            .collect();
                        if !arguments.iter().any(|arg| arg.contains(PLACEHOLDER)) {
                            args.push(raw_path.into_owned());
                        }
                        Ok(Opener::Command {
                            path: executable.clone(),
                            args,
                        })
                    },
                    Structured::Plugin { plugin, message } => {
                        nonblank(&message, "message")?;
                        Ok(Opener::Plugin {
                            destination: plugin_destination(&plugin)?,
                            message,
                        })
                    },
                }
            };
            structured.insert(
                extension.clone(),
                entry().map_err(|error| format!("openers_json.{extension}: {error}"))?,
            );
        }
    }
    let extension = file
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase);
    if let Some(extension) = extension {
        if let Some(opener) = structured.remove(&extension) {
            return Ok(opener);
        }
        if let Some(command) = config.get(&format!("{OPEN_PREFIX}{extension}")) {
            return Ok(build_command(command, file).unwrap_or(Opener::YinzEditor));
        }
    }
    Ok(structured.remove("default").unwrap_or_else(|| {
        config
            .get(OPEN_DEFAULT)
            .and_then(|command| build_command(command, file))
            .unwrap_or(Opener::YinzEditor)
    }))
}

#[derive(serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Structured {
    Argv {
        argv: Vec<String>,
    },
    Plugin {
        plugin: String,
        #[serde(default = "default_message")]
        message: String,
    },
}

fn default_message() -> String {
    "open".into()
}

fn nonblank(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(format!("{name} must be nonblank and contain no NUL"))
    } else {
        Ok(())
    }
}

fn plugin_destination(value: &str) -> Result<String, String> {
    nonblank(value, "plugin destination")?;
    if Path::new(value).is_absolute() {
        return url::Url::from_file_path(value)
            .map(String::from)
            .map_err(|()| "plugin path must be absolute".into());
    }
    if !value.contains(':') {
        return if value.contains('/') || value.contains('\\') || value.starts_with('.') {
            Err("plugin destination must be an alias or an absolute path".into())
        } else {
            Ok(value.into())
        };
    }
    let url = url::Url::parse(value).map_err(|error| format!("invalid plugin URL: {error}"))?;
    match url.scheme() {
        "yinz" if !url.path().is_empty() && url.host_str().is_none()
            && url.query().is_none() && url.fragment().is_none() => Ok(value.into()),
        "file" if value.starts_with("file:/") && url.host_str().is_none()
            && url.query().is_none() && url.fragment().is_none() => Ok(value.into()),
        _ => Err("expected an alias, yinz: URL, absolute path, or absolute local file: URL without query/fragment; escape literal # and ? in URL paths".into()),
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
    fn unconfigured_extension_uses_the_yinz_editor() {
        let config = config(&[("open_md", "glow")]);
        let opener = opener_for(&config, &PathBuf::from("/p/src/main.rs")).unwrap();
        assert_eq!(opener, Opener::YinzEditor);
    }

    #[test]
    fn empty_config_uses_the_yinz_editor() {
        let opener = opener_for(&BTreeMap::new(), &PathBuf::from("/p/README.md")).unwrap();
        assert_eq!(opener, Opener::YinzEditor);
    }

    #[test]
    fn matching_extension_selects_its_command() {
        let config = config(&[("open_md", "glow -p")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md")).unwrap();
        assert_eq!(opener, command("glow", &["-p", "/p/README.md"]));
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let config = config(&[("open_png", "chafa")]);
        let opener = opener_for(&config, &PathBuf::from("/p/Shot.PNG")).unwrap();
        assert_eq!(opener, command("chafa", &["/p/Shot.PNG"]));
    }

    #[test]
    fn placeholder_is_substituted_in_place_rather_than_appended() {
        let config = config(&[("open_pdf", "zathura {} --mode fullscreen")]);
        let opener = opener_for(&config, &PathBuf::from("/p/book.pdf")).unwrap();
        assert_eq!(
            opener,
            command("zathura", &["/p/book.pdf", "--mode", "fullscreen"])
        );
    }

    #[test]
    fn placeholder_embedded_in_an_argument_is_substituted() {
        let config = config(&[("open_log", "less +F{}")]);
        let opener = opener_for(&config, &PathBuf::from("/p/run.log")).unwrap();
        assert_eq!(opener, command("less", &["+F/p/run.log"]));
    }

    #[test]
    fn open_default_covers_unmatched_extensions() {
        let config = config(&[("open_md", "glow"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/main.rs")).unwrap();
        assert_eq!(opener, command("bat", &["/p/main.rs"]));
    }

    #[test]
    fn extension_entry_wins_over_open_default() {
        let config = config(&[("open_md", "glow"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md")).unwrap();
        assert_eq!(opener, command("glow", &["/p/README.md"]));
    }

    #[test]
    fn file_without_an_extension_uses_open_default() {
        let config = config(&[("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/Makefile")).unwrap();
        assert_eq!(opener, command("bat", &["/p/Makefile"]));
    }

    #[test]
    fn dotfile_without_an_extension_uses_open_default() {
        // Path::extension() returns None for ".gitignore" — the leading dot marks a
        // hidden file, not an extension.
        let config = config(&[("open_gitignore", "never"), ("open_default", "bat")]);
        let opener = opener_for(&config, &PathBuf::from("/p/.gitignore")).unwrap();
        assert_eq!(opener, command("bat", &["/p/.gitignore"]));
    }

    #[test]
    fn blank_opener_falls_back_to_the_yinz_editor() {
        let config = config(&[("open_md", "   ")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md")).unwrap();
        assert_eq!(opener, Opener::YinzEditor);
    }

    #[test]
    fn unrelated_config_keys_are_ignored() {
        let config = config(&[("floating", "true"), ("width", "80%")]);
        let opener = opener_for(&config, &PathBuf::from("/p/README.md")).unwrap();
        assert_eq!(opener, Opener::YinzEditor);
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

        match opener_for(&config, &file).unwrap() {
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
            Opener::YinzEditor | Opener::Plugin { .. } => panic!(
                "a configured, non-blank opener {:?} should not fall through to the editor",
                command
            ),
        }
    }
}

#[cfg(test)]
#[path = "unit/structured_openers.rs"]
mod structured_tests;
