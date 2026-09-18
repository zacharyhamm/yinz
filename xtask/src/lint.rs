//! Handle running `cargo clippy` on the sources.
use crate::flags;
use anyhow::{anyhow, Context};
use std::path::{Path, PathBuf};
use xshell::{cmd, Shell};

pub fn lint(sh: &Shell, flags: flags::Lint) -> anyhow::Result<()> {
    let err_context = "failed to run task 'lint'";

    let _pd = sh.push_dir(crate::project_root());
    let cargo = check_clippy()
        .and_then(|_| crate::cargo())
        .context(err_context)?;
    check_file_level_allows().context(err_context)?;

    println!();
    let msg = ">> Linting the workspace";
    crate::status(msg);
    println!("{}", msg);

    // One invocation, so shared dependencies are analysed once. As in `x test`, the plugin
    // crates' wasm pin lives in their own `.cargo/config.toml` and Cargo reads config from the
    // invocation directory upward, so running from the root lints them against the host.
    //
    // Unlike `x test` this asks for `--all-features` when it can: lint wants the widest surface
    // available, and clippy's artifacts live in their own fingerprint namespace anyway, so the
    // wider graph costs nothing that could otherwise have been shared.
    let mut cmd = cmd!(sh, "{cargo} clippy --workspace --all-targets");
    for excluded in crate::test::EXCLUDED {
        cmd = cmd.args(["--exclude", excluded]);
    }
    cmd = cmd.arg("--all-features");
    if flags.fix {
        cmd = cmd.args(["--fix", "--allow-dirty", "--allow-staged"]);
    }
    if cmd
        .arg("--")
        .arg("-D")
        .arg("warnings")
        .args(&flags.args)
        .run()
        .is_err()
    {
        let msg = ">> Lint failures";
        crate::status(msg);
        println!("\n{}", msg);
        return Err(anyhow!("lints failed")).context(err_context);
    }
    crate::authoring::lint(sh)
}

fn check_clippy() -> anyhow::Result<PathBuf> {
    which::which("cargo-clippy").context(
        "Couldn't find 'cargo-clippy' executable. Please install it with `rustup component add clippy`",
    )
}

/// The file-level lint suppressions the workspace tolerates, each with its reason. Every entry
/// is a `(path, lint)` pair; a `#![allow(..)]` or `#![expect(..)]` anywhere else - written
/// directly or through `#![cfg_attr(.., allow(..))]` - fails the lint task before clippy runs,
/// and an entry that no longer matches fails it too, so the list only ever shrinks by hand.
/// A `reason = ".."` argument is not a lint and is ignored when matching.
///
/// `[workspace.lints.clippy]` in `Cargo.toml` is the place for a lint that is allowed
/// everywhere, with a reason. This list is for a lint that is wrong for one file: a generated
/// artifact, vendored code, or a test harness built around the thing the lint warns about. A
/// warning that is merely inconvenient is fixed, or suppressed on the one item with
/// `#[expect(lint, reason = "...")]`.
const FILE_LEVEL_ALLOWS: &[(&str, &str)] = &[
    // prost output, included as-is: it nests `mod api { mod api { .. } }` by construction
    (
        "zellij-ipc/src/client_server_contract/mod.rs",
        "clippy::module_inception",
    ),
    (
        "zellij-ipc/src/nested_session_contract/mod.rs",
        "clippy::module_inception",
    ),
    // vendored from termwiz; its doc comments are not ours to reflow
    (
        "zellij-runtime/src/vendored/termwiz/input.rs",
        "clippy::doc_lazy_continuation",
    ),
    // the store's test files run once per engine, so one file is deliberately two modules
    (
        "zellij-storage/src/plugin_data_store/tests/mod.rs",
        "clippy::duplicate_mod",
    ),
    // the dockerised e2e smoke suite, `#[ignore]`d and behind the `e2e` feature; a scaffold
    // of helpers only some of its five tests reach
    ("src/tests/e2e/cases.rs", "unused"),
    // plugin crates linted for the host triple, where nothing reaches what only the wasm
    // entry points call
    ("default-plugins/hocus/src/tools.rs", "dead_code"),
    (
        "default-plugins/fixture-plugin-for-tests/src/main.rs",
        "dead_code",
    ),
];

/// Every `.rs` file under `dir`, as a path relative to `root`, skipping the build output, hidden
/// directories (`.git`, sibling worktrees) and anything a symlink points at.
fn rust_sources(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            rust_sources(&path, root, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
    Ok(())
}

/// A file-level lint suppression found in a source file: the attribute it came through
/// (`allow` or `expect`) and one lint it names.
#[derive(Debug, PartialEq, Eq)]
struct Suppression {
    kind: &'static str,
    lint: String,
}

/// The lints suppressed by the inner attributes (`#![..]`) in `source`, whether written
/// directly or through `#![cfg_attr(<predicate>, allow(..))]`; a `reason = ".."` argument is
/// not a lint and is dropped. Attributes may span lines, as rustfmt wraps a long list.
fn file_level_suppressions(source: &str) -> Vec<Suppression> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("#![") {
        let line_start = rest[..at].rfind('\n').map_or(0, |i| i + 1);
        if !rest[line_start..at].trim().is_empty() {
            // Not at the start of a line: a doc comment or a string mentioning one.
            rest = &rest[at + 3..];
            continue;
        }
        let body_start = at + 3;
        let body_end =
            matching_close(&rest[body_start..], ']').map_or(rest.len(), |i| body_start + i);
        let body = rest[body_start..body_end].trim();
        rest = &rest[body_end..];
        let Some((name, args)) = call(body) else {
            continue;
        };
        match name {
            "allow" | "expect" => found.extend(lints(name, args)),
            "cfg_attr" => {
                for attr in split_top_level(args).skip(1) {
                    if let Some((name @ ("allow" | "expect"), args)) = call(attr.trim()) {
                        found.extend(lints(name, args));
                    }
                }
            },
            _ => {},
        }
    }
    found
}

/// The lints in `args`, the inside of an `allow(..)` or `expect(..)`, tagged with `kind`.
fn lints<'a>(kind: &str, args: &'a str) -> impl Iterator<Item = Suppression> + 'a {
    let kind = if kind == "allow" { "allow" } else { "expect" };
    split_top_level(args)
        .map(str::trim)
        .filter(|arg| !arg.is_empty() && !arg.starts_with("reason"))
        .map(move |lint| Suppression {
            kind,
            lint: lint.to_owned(),
        })
}

/// `name(args)` split into its parts, when `text` is exactly one such call.
fn call(text: &str) -> Option<(&str, &str)> {
    let (name, rest) = text.split_once('(')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let close = matching_close(rest, ')')?;
    Some((name, &rest[..close]))
}

/// The index in `text` of the `close` that ends the bracket opened just before it, skipping
/// nested brackets and the contents of string literals.
fn matching_close(text: &str, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => skip_string(&mut chars),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            c if c == close => return Some(i),
            _ => {},
        }
    }
    None
}

/// `text` split on the commas outside brackets and string literals.
fn split_top_level(text: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    let mut chars = text.char_indices();
    let mut cuts = Vec::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => skip_string(&mut chars),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => cuts.push(i),
            _ => {},
        }
    }
    let mut from = 0;
    cuts.into_iter()
        .chain(std::iter::once(text.len()))
        .map(move |cut| {
            let piece = &text[from..cut];
            from = (cut + 1).min(text.len());
            piece
        })
}

/// Advances `chars` past the string literal whose opening quote was just read.
fn skip_string(chars: &mut std::str::CharIndices<'_>) {
    while let Some((_, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            },
            '"' => return,
            _ => {},
        }
    }
}

/// Fails when a Rust source file carries a file-level lint suppression that
/// [`FILE_LEVEL_ALLOWS`] does not name, or when an entry there names a file that no longer
/// carries it.
fn check_file_level_allows() -> anyhow::Result<()> {
    let root = crate::project_root();
    let mut files = Vec::new();
    rust_sources(&root, &root, &mut files)?;
    let mut offenders = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut present = std::collections::HashSet::new();
    for path in files {
        let Ok(source) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let path = path.to_string_lossy();
        let path = path.as_ref();
        present.insert(path.to_owned());
        for Suppression { kind, lint } in file_level_suppressions(&source) {
            if FILE_LEVEL_ALLOWS.contains(&(path, lint.as_str())) {
                seen.insert((path.to_owned(), lint));
            } else {
                offenders.push(format!("{path}: #![{kind}({lint})]"));
            }
        }
    }
    // An entry whose file is gone is left alone: this also runs in the scaffolded projects the
    // xtask tests build, which hold none of these files.
    let stale: Vec<String> = FILE_LEVEL_ALLOWS
        .iter()
        .filter(|(path, _)| present.contains(*path))
        .filter(|(path, lint)| !seen.contains(&((*path).to_owned(), (*lint).to_owned())))
        .map(|(path, lint)| format!("{path}: {lint}"))
        .collect();
    if offenders.is_empty() && stale.is_empty() {
        return Ok(());
    }
    let mut msg = String::new();
    if !offenders.is_empty() {
        msg.push_str(
            "file-level lint suppressions are not allowed: fix the warning, or suppress it on \
             the one item with `#[expect(lint, reason = \"...\")]`; a lint that is wrong for a \
             whole file goes in `FILE_LEVEL_ALLOWS` in xtask/src/lint.rs with its reason, and \
             one that is wrong everywhere in `[workspace.lints.clippy]` with its reason\n",
        );
        for o in &offenders {
            msg.push_str("  ");
            msg.push_str(o);
            msg.push('\n');
        }
    }
    if !stale.is_empty() {
        msg.push_str(
            "entries in `FILE_LEVEL_ALLOWS` that no longer match a suppression - delete them\n",
        );
        for s in &stale {
            msg.push_str("  ");
            msg.push_str(s);
            msg.push('\n');
        }
    }
    Err(anyhow!(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<(&'static str, String)> {
        file_level_suppressions(source)
            .into_iter()
            .map(|s| (s.kind, s.lint))
            .collect()
    }

    #[test]
    fn a_one_line_allow_is_found() {
        assert_eq!(
            found("//! docs\n#![allow(clippy::module_inception)]\nmod api;\n"),
            vec![("allow", "clippy::module_inception".to_owned())]
        );
    }

    #[test]
    fn an_allow_rustfmt_wrapped_across_lines_is_found_whole() {
        let source =
            "#![allow(\n    clippy::too_many_arguments,\n    clippy::type_complexity,\n    \
                      clippy::needless_borrow\n)]\nfn main() {}\n";
        assert_eq!(
            found(source),
            vec![
                ("allow", "clippy::too_many_arguments".to_owned()),
                ("allow", "clippy::type_complexity".to_owned()),
                ("allow", "clippy::needless_borrow".to_owned()),
            ]
        );
    }

    #[test]
    fn an_expect_keeps_its_kind_and_drops_its_reason() {
        let source = "#![expect(clippy::duplicate_mod, reason = \"one file, two engines\")]\n";
        assert_eq!(
            found(source),
            vec![("expect", "clippy::duplicate_mod".to_owned())]
        );
    }

    #[test]
    fn a_reason_containing_brackets_does_not_end_the_attribute_early() {
        let source =
            "#![allow(dead_code, reason = \"see foo(), bar)] and \\\" baz\")]\n#![allow(unused)]\n";
        assert_eq!(
            found(source),
            vec![
                ("allow", "dead_code".to_owned()),
                ("allow", "unused".to_owned())
            ]
        );
    }

    #[test]
    fn an_allow_inside_cfg_attr_is_found() {
        let source = "// why\n#![cfg_attr(not(target_arch = \"wasm32\"), allow(dead_code))]\n\
                      #![cfg_attr(test, expect(unused, reason = \"x\"), allow(clippy::x))]\n";
        assert_eq!(
            found(source),
            vec![
                ("allow", "dead_code".to_owned()),
                ("expect", "unused".to_owned()),
                ("allow", "clippy::x".to_owned()),
            ]
        );
    }

    #[test]
    fn other_inner_attributes_and_item_attributes_are_not_suppressions() {
        let source = "#![cfg(unix)]\n#![deny(missing_docs)]\n#![doc = \"#![allow(x)]\"]\n\
                      /// a `#![allow(..)]` in a doc comment\n#[allow(dead_code)]\nfn f() {}\n\
                      fn g() { let s = \"#![allow(y)]\"; }\n";
        assert_eq!(found(source), vec![]);
    }
}
