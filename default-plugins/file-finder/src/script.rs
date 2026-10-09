//! The shell script that runs in the finder pane.
//!
//! The git-root walk happens here rather than in the plugin on purpose. Resolving the
//! root plugin-side would mean an async `run_command` round-trip and a second
//! state-machine stage before fzf could even start; doing it inside the same `sh -c`
//! that runs fzf collapses that to zero extra events. The plugin recovers the root from
//! the absolute path the script writes out.
//!
//! POSIX `sh` only — no bashisms, and no GNU-only `find -printf`, since macOS is a
//! build target too.

use std::collections::BTreeMap;

/// Config key overriding the command that produces the candidate list.
const FILE_COMMAND: &str = "file_command";

/// Config key appending extra flags to the fzf invocation.
const FZF_ARGS: &str = "fzf_args";

/// Lists tracked files plus untracked-but-not-ignored ones, which is what a person
/// means by "the files in this project".
const GIT_LIST: &str = r"git ls-files --cached --others --exclude-standard -z | tr '\000' '\n'";

/// Outside a git checkout there is no ignore file to honour, so list everything except
/// the `.git` directory itself. `sed` strips the `./` that `find .` prefixes.
const FIND_LIST: &str = r"find . -type f -not -path '*/.git/*' | sed 's|^\./||'";

/// Build the script for the finder pane.
///
/// `out_path` receives the absolute path of the selection, or nothing at all if the
/// user cancels. The plugin distinguishes the two by whether the file is non-empty.
///
/// Note that `file_command` and `fzf_args` are interpolated as raw shell fragments.
/// They have to be — they are commands and flag lists by construction — so they are
/// documented as such in the default config.
pub fn build(config: &BTreeMap<String, String>, out_path: &str) -> String {
    let fzf_args = config
        .get(FZF_ARGS)
        .map(|args| args.trim())
        .filter(|args| !args.is_empty())
        .unwrap_or("");

    // A configured file_command replaces both branches: the user has told us how to
    // list files, so the git/non-git distinction stops being ours to make. The root
    // still resolves to the git toplevel where there is one, so relative paths from
    // their command land in the right place.
    let list_body = match config
        .get(FILE_COMMAND)
        .map(|cmd| cmd.trim())
        .filter(|cmd| !cmd.is_empty())
    {
        Some(custom) => format!(
            "\
if root=$(git rev-parse --show-toplevel 2>/dev/null) && [ -n \"$root\" ]; then
  cd \"$root\" || exit 1
else
  root=$PWD
fi
list() {{ {custom}; }}"
        ),
        None => format!(
            "\
if root=$(git rev-parse --show-toplevel 2>/dev/null) && [ -n \"$root\" ]; then
  cd \"$root\" || exit 1
  list() {{ {GIT_LIST}; }}
else
  root=$PWD
  list() {{ {FIND_LIST}; }}
fi"
        ),
    };

    let fzf = if fzf_args.is_empty() {
        "fzf --prompt 'file> '".to_string()
    } else {
        format!("fzf --prompt 'file> ' {fzf_args}")
    };

    let quoted_out = single_quote(out_path);

    format!(
        "\
set -u
{list_body}
sel=$(list | {fzf}) || exit 0
[ -n \"$sel\" ] || exit 0
case \"$sel\" in
  /*) printf '%s\\n' \"$sel\" ;;
  *)  printf '%s/%s\\n' \"$root\" \"$sel\" ;;
esac > {quoted_out}
"
    )
}

/// Wrap a value in single quotes for `sh`, escaping any single quotes it contains.
fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Interpret what the script left in the selection file.
///
/// Returns `None` when the user cancelled — fzf exiting non-zero, an empty selection,
/// or the file never being written at all.
pub fn parse_selection(contents: &str) -> Option<&str> {
    let selection = contents.split('\n').next().unwrap_or("");
    if selection.is_empty() {
        None
    } else {
        Some(selection)
    }
}

/// Structured traversal runs in the command process, with paths always shell-quoted.
pub fn build_structured(
    config: &BTreeMap<String, String>,
    options: &yinz_types::pick::PickOptions,
    cwd: &std::path::Path,
    out_path: &str,
) -> Result<String, String> {
    use yinz_types::pick::PickKind;
    options.validate()?;
    let roots = options
        .roots
        .clone()
        .unwrap_or_else(|| vec![cwd.to_string_lossy().into_owned()]);
    let mut listings = vec![];
    for root in roots {
        let root = cwd.join(root);
        let root = single_quote(&root.to_string_lossy());
        let hidden = if options.hidden.unwrap_or(false) {
            ""
        } else {
            r#"\( ! -path "$root_pattern" -name '.*' \) -prune -o "#
        };
        let recursive = options.recursive.unwrap_or(true);
        // Prune every non-root entry after evaluating it in nonrecursive mode.
        let kind = if options.kind == PickKind::Directory {
            "d"
        } else {
            "f"
        };
        let suffixes = options
            .extensions
            .as_ref()
            .filter(|exts| !exts.is_empty())
            .map(|exts| {
                format!(
                    "\\( {} \\) ",
                    exts.iter()
                        .map(|ext| format!("-iname {}", single_quote(&format!("*.{ext}"))))
                        .collect::<Vec<_>>()
                        .join(" -o ")
                )
            })
            .unwrap_or_default();
        let select = format!("-type {kind} {suffixes}-print0");
        let expression = if recursive {
            select
        } else {
            format!(r#"\( -path "$root_pattern" -o -prune \) -a \( {select} \)"#)
        };
        listings.push(format!(
            // The trailing slash preserves root newlines through command substitution.
            r#"(cd -P {root} && root=$PWD && root_pattern=$(printf '%s/' "$root" | sed 's/[][\\*?]/\\&/g') && root_pattern=${{root_pattern%/}} && find "$root" {hidden}{expression})"#
        ));
    }
    let listing = listings.join(" &&\n");
    let out = single_quote(out_path);
    let candidates = single_quote(&format!("{out_path}.candidates"));
    let args = config.get(FZF_ARGS).map(String::as_str).unwrap_or("");
    Ok(format!(
        r#"set -u
out={out}
candidates={candidates}
trap 'rm -f "$candidates"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
: > "$out" || exit 10
command -v fzf >/dev/null 2>&1 || exit 11
{{ {listing}; }} > "$candidates" || exit 10
fzf {args} --no-multi --read0 --print0 --no-print-query --no-expect < "$candidates" > "$out"
status=$?
case "$status" in
  0) exit 0 ;;
  1|130) : > "$out"; exit 0 ;;
  *) : > "$out"; exit 12 ;;
esac
"#
    ))
}

pub fn parse_structured_selection(contents: &str) -> Result<Option<&str>, String> {
    if contents.is_empty() {
        return Ok(None);
    }
    let selection = contents
        .strip_suffix('\0')
        .ok_or("finder output is not NUL-terminated")?;
    if selection.contains('\0') || !std::path::Path::new(selection).is_absolute() {
        return Err("finder must return exactly one absolute path".into());
    }
    Ok(Some(selection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn default_script_has_both_a_git_and_a_find_branch() {
        let script = build(&BTreeMap::new(), "/tmp/sel");
        assert!(script.contains("git rev-parse --show-toplevel"));
        assert!(script.contains(GIT_LIST));
        assert!(script.contains("find . -type f"));
    }

    #[test]
    fn default_script_writes_to_the_given_path() {
        let script = build(&BTreeMap::new(), "/tmp/yinz-file-finder-7.sel");
        assert!(script.contains("esac > '/tmp/yinz-file-finder-7.sel'"));
    }

    #[test]
    fn relative_selections_are_joined_onto_the_root() {
        let script = build(&BTreeMap::new(), "/tmp/sel");
        assert!(script.contains(r#"printf '%s/%s\n' "$root" "$sel""#));
    }

    #[test]
    fn file_command_replaces_the_listing_branches() {
        let script = build(
            &config(&[("file_command", "rg --files --hidden")]),
            "/tmp/sel",
        );
        assert!(script.contains("list() { rg --files --hidden; }"));
        assert!(!script.contains(GIT_LIST));
        assert!(!script.contains("find . -type f"));
        // The root walk survives, so relative paths still resolve correctly.
        assert!(script.contains("git rev-parse --show-toplevel"));
    }

    #[test]
    fn blank_file_command_falls_back_to_the_defaults() {
        let script = build(&config(&[("file_command", "  ")]), "/tmp/sel");
        assert!(script.contains(GIT_LIST));
    }

    #[test]
    fn fzf_args_are_appended_to_the_invocation() {
        let script = build(
            &config(&[("fzf_args", "--preview 'head -50 {}'")]),
            "/tmp/sel",
        );
        assert!(script.contains("fzf --prompt 'file> ' --preview 'head -50 {}'"));
    }

    #[test]
    fn without_fzf_args_the_invocation_is_bare() {
        let script = build(&BTreeMap::new(), "/tmp/sel");
        assert!(script.contains("sel=$(list | fzf --prompt 'file> ') || exit 0"));
    }

    #[test]
    fn out_path_with_a_quote_is_escaped() {
        let script = build(&BTreeMap::new(), "/tmp/it's");
        assert!(script.contains(r"esac > '/tmp/it'\''s'"));
    }

    #[test]
    fn selection_is_trimmed_of_its_trailing_newline() {
        assert_eq!(parse_selection("/p/src/main.rs\n"), Some("/p/src/main.rs"));
    }

    #[test]
    fn only_the_first_line_of_the_selection_is_used() {
        assert_eq!(parse_selection("/p/a.rs\n/p/b.rs\n"), Some("/p/a.rs"));
    }

    #[test]
    fn empty_selection_reads_as_a_cancel() {
        assert_eq!(parse_selection(""), None);
        assert_eq!(parse_selection("\n"), None);
        assert_eq!(parse_selection("   \n"), Some("   "));
    }

    /// Whatever the finder pane left behind — a crashed fzf, a partial write, a file
    /// full of blank lines — the plugin turns a `Some` straight into a `PathBuf` and
    /// opens it. So a `Some` must always be a usable one-line path: non-empty,
    /// preserved, no embedded newline, and genuinely taken from the file's first line.
    ///
    /// The generator is deliberately unconstrained; garbage input is exactly the case
    /// this guards.
    #[hegel::test]
    fn a_parsed_selection_is_always_a_usable_single_line(tc: hegel::TestCase) {
        let contents = tc.draw(generators::text());

        let Some(selection) = parse_selection(&contents) else {
            return; // a cancel is always a valid answer
        };

        assert!(!selection.is_empty(), "a selection must not be empty");
        // Only the line delimiter is removed; a trailing CR can belong to the filename.
        assert!(
            !selection.contains('\n'),
            "a selection must not span lines: {:?}",
            selection
        );
        let first_line = contents
            .split('\n')
            .next()
            .expect("a Some means there was a line to take it from");
        assert!(
            first_line == selection,
            "{:?} did not come from the first line {:?}",
            selection,
            first_line
        );
    }
}

#[cfg(test)]
#[path = "unit/listing.rs"]
mod listing_tests;

#[cfg(test)]
#[path = "unit/structured_listing.rs"]
mod structured_listing_tests;
