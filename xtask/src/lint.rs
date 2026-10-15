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
/// is a `(path, lint)` pair; a file-level `#![allow(..)]` or `#![expect(..)]` that no entry
/// names - written directly or through `#![cfg_attr(.., allow(..))]` - fails the lint task
/// before clippy runs, and an entry that no longer matches fails it too, so the list only ever
/// shrinks by hand. A `reason = ".."` argument is not a lint and is ignored when matching. An
/// inner attribute inside a `mod` or `fn` body is not file-level and is left to clippy.
///
/// `[workspace.lints.clippy]` in `Cargo.toml` is the place for a lint that is allowed
/// everywhere, with a reason. This list is for a lint that is wrong for one file: a generated
/// artifact, vendored code, or a test harness built around the thing the lint warns about. A
/// warning that is merely inconvenient is fixed, or suppressed on the one item with
/// `#[expect(lint, reason = "...")]`.
const FILE_LEVEL_ALLOWS: &[(&str, &str)] = &[
    // prost output, included as-is: it nests `mod api { mod api { .. } }` by construction
    (
        "yinz-ipc/src/client_server_contract/mod.rs",
        "clippy::module_inception",
    ),
    (
        "yinz-ipc/src/nested_session_contract/mod.rs",
        "clippy::module_inception",
    ),
    // vendored from termwiz; its doc comments are not ours to reflow
    (
        "yinz-key-codec/src/vendored/termwiz/input.rs",
        "clippy::doc_lazy_continuation",
    ),
    // the dockerised e2e smoke suite, `#[ignore]`d and behind the `e2e` feature; a scaffold
    // of helpers only some of its five tests reach
    ("src/tests/e2e/cases.rs", "unused"),
];

/// Every `.rs` file under `dir`, as a path relative to `root`, skipping the build output, hidden
/// directories (`.git`, sibling worktrees), vendored dependencies and symlinks. Only `dir` itself
/// failing to read is an error: an entry or a subdirectory this process cannot read is skipped,
/// as the caller skips a file it cannot read.
fn rust_sources(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || path == root.join("vendor")
            {
                continue;
            }
            // An unreadable subdirectory is skipped, like an unreadable file.
            let _ = rust_sources(&path, root, out);
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

/// The lints suppressed by the file-level inner attributes (`#![..]`) in `source`: those outside
/// any bracket, so not the ones a `mod` or `fn` body carries. An attribute counts whether written
/// directly or through `#![cfg_attr(<predicate>, allow(..))]`, and may span lines, as rustfmt
/// wraps a long list; a `reason = ".."` argument is not a lint and is dropped. Comments and
/// string and character literals are not code, so a `#![allow(..)]` quoted in one is not found,
/// and a bracket or quote inside one does not end an attribute early.
fn file_level_suppressions(source: &str) -> Vec<Suppression> {
    // A shebang is not an attribute, and rustc drops the line before lexing.
    let source = match source.strip_prefix("#!") {
        Some(rest) if !rest.starts_with('[') => rest.find('\n').map_or("", |i| &rest[i..]),
        _ => source,
    };
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut code = CodeChars::new(source);
    while let Some((i, c)) = code.next() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '#' if depth == 0 && source[i..].starts_with("#![") => {
                let body_start = i + 3;
                let body_end = matching_close(&source[body_start..], ']')
                    .map_or(source.len(), |close| body_start + close);
                found.extend(suppressions_in(
                    code_only(&source[body_start..body_end]).trim(),
                ));
                // Past the closing `]`, which is code again.
                code.skip_to((body_end + 1).min(source.len()));
            },
            _ => {},
        }
    }
    found
}

/// The lints suppressed by one inner attribute, given `body`: what sits between `#![` and `]`.
fn suppressions_in(body: &str) -> Vec<Suppression> {
    let Some((name, args)) = call(body) else {
        return Vec::new();
    };
    match name {
        "allow" | "expect" => lints(name, args).collect(),
        "cfg_attr" => split_top_level(args)
            .skip(1)
            .flat_map(|attr| suppressions_in(code_only(attr).trim()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The lints in `args`, the inside of an `allow(..)` or `expect(..)`, tagged with `kind`.
fn lints<'a>(kind: &str, args: &'a str) -> impl Iterator<Item = Suppression> + 'a {
    let kind = if kind == "allow" { "allow" } else { "expect" };
    split_top_level(args)
        .map(|arg| code_only(arg).trim().to_owned())
        .filter(|arg| !arg.is_empty() && !arg.starts_with("reason"))
        .map(move |lint| Suppression { kind, lint })
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
/// nested brackets and anything inside comments and literals.
fn matching_close(text: &str, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in CodeChars::new(text) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            c if c == close => return Some(i),
            _ => {},
        }
    }
    None
}

/// `text` split on the commas outside brackets, comments and literals. The pieces are raw, so
/// a comment inside one is still there; `code_only` strips it.
fn split_top_level(text: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    let mut cuts = Vec::new();
    for (i, c) in CodeChars::new(text) {
        match c {
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

/// `text` with its comments and literals removed: what [`CodeChars`] yields, as a string.
fn code_only(text: &str) -> String {
    CodeChars::new(text).map(|(_, c)| c).collect()
}

/// The characters of a piece of Rust source that are code: outside comments, and outside
/// string, byte-string, raw-string and character literals. Each comes with its byte index in
/// the text. A lifetime's quote is code (`'a` is not a literal); a comment's newline is not.
struct CodeChars<'a> {
    text: &'a str,
    offset: usize,
    chars: std::str::CharIndices<'a>,
}

impl<'a> CodeChars<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            offset: 0,
            chars: text.char_indices(),
        }
    }

    /// Continues from byte index `at`, which must be a char boundary in code: not inside a
    /// comment or a literal.
    fn skip_to(&mut self, at: usize) {
        self.offset = at;
        self.chars = self.text[at..].char_indices();
    }

    /// Advances past the end of the block comment whose `/*` was just read, honouring nesting.
    fn skip_block_comment(&mut self) {
        let mut depth = 1usize;
        while let Some((i, c)) = self.chars.next() {
            let rest = &self.text[self.offset + i..];
            if c == '/' && rest.starts_with("/*") {
                self.chars.next();
                depth += 1;
            } else if c == '*' && rest.starts_with("*/") {
                self.chars.next();
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }

    /// Advances past the end of the string literal whose opening quote was just read. `hashes`
    /// is the number of `#` a raw string closes with, or `None` for a string with escapes.
    fn skip_string(&mut self, hashes: Option<usize>) {
        while let Some((i, c)) = self.chars.next() {
            match (c, hashes) {
                ('\\', None) => {
                    self.chars.next();
                },
                ('"', None) => return,
                ('"', Some(n)) => {
                    let after = &self.text.as_bytes()[self.offset + i + 1..];
                    if after.len() >= n && after[..n].iter().all(|&b| b == b'#') {
                        for _ in 0..n {
                            self.chars.next();
                        }
                        return;
                    }
                },
                _ => {},
            }
        }
    }

    /// Advances past the character literal whose opening quote was just read and says so, or
    /// leaves a lifetime or label alone: `'a'` and `'\n'` are literals, `'a` is not.
    fn skip_char_literal(&mut self) -> bool {
        let mut look = self.chars.clone();
        match look.next() {
            Some((_, '\\')) => {
                // The backslash, the escaped char, then whatever precedes the closing quote.
                self.chars.next();
                self.chars.next();
                while self.chars.next().is_some_and(|(_, c)| c != '\'') {}
                true
            },
            Some(_) if matches!(look.next(), Some((_, '\''))) => {
                self.chars.next();
                self.chars.next();
                true
            },
            _ => false,
        }
    }
}

impl Iterator for CodeChars<'_> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<(usize, char)> {
        loop {
            let (i, c) = self.chars.next()?;
            let at = self.offset + i;
            let rest = &self.text[at..];
            match c {
                '/' if rest.starts_with("//") => {
                    while self.chars.next().is_some_and(|(_, c)| c != '\n') {}
                },
                '/' if rest.starts_with("/*") => {
                    self.chars.next();
                    self.skip_block_comment();
                },
                '"' => self.skip_string(raw_string_hashes(&self.text[..at])),
                '\'' if self.skip_char_literal() => {},
                _ => return Some((at, c)),
            }
        }
    }
}

/// For a `"` preceded by `before`: the number of `#` the raw string opened there closes with,
/// or `None` when the quote opens an ordinary string. A raw string is `r`, then its hashes,
/// then the quote (`br##"` and `cr"` included).
fn raw_string_hashes(before: &str) -> Option<usize> {
    let unhashed = before.trim_end_matches('#');
    unhashed
        .ends_with('r')
        .then(|| before.len() - unhashed.len())
}

/// Fails when a Rust source file carries a file-level lint suppression that
/// [`FILE_LEVEL_ALLOWS`] does not name, or when an entry there names a file that no longer
/// carries it.
fn check_file_level_allows() -> anyhow::Result<()> {
    let root = crate::project_root();
    let mut files = Vec::new();
    rust_sources(&root, &root, &mut files)
        .with_context(|| format!("failed to read {}", root.display()))?;
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

    fn allow(lint: &str) -> (&'static str, String) {
        ("allow", lint.to_owned())
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
    fn nested_cfg_attr_suppressions_are_found() {
        assert_eq!(
            found("#![cfg_attr(all(), cfg_attr(all(), allow(dead_code)))]"),
            vec![allow("dead_code")]
        );

        let source = r#"#![cfg_attr(
            any(),
            allow(unused_imports),
            cfg_attr(
                all(),
                // A nested attribute can contain siblings and further nesting.
                expect(unused_variables, reason = "see foo(), bar)]"),
                cfg_attr(
                    not(target_arch = "wasm32"),
                    /* why */ allow(dead_code, unused_mut, reason = "nested",),
                ),
                deny(missing_docs),
            ),
            expect(clippy::needless_borrow),
        )]"#;
        assert_eq!(
            found(source),
            vec![
                allow("unused_imports"),
                ("expect", "unused_variables".to_owned()),
                allow("dead_code"),
                allow("unused_mut"),
                ("expect", "clippy::needless_borrow".to_owned()),
            ]
        );
    }

    #[test]
    fn nested_cfg_attr_without_file_level_suppressions_is_ignored() {
        let source = r#"
            #![cfg_attr(all(), cfg_attr(all(), deny(missing_docs), doc = "allow(x)"))]
            #[cfg_attr(all(), cfg_attr(all(), allow(dead_code)))]
            fn f() {}
            mod m {
                #![cfg_attr(all(), cfg_attr(all(), expect(unused)))]
            }
        "#;
        assert_eq!(found(source), vec![]);
    }

    #[test]
    fn other_inner_attributes_and_item_attributes_are_not_suppressions() {
        let source = "#![cfg(unix)]\n#![deny(missing_docs)]\n#![doc = \"#![allow(x)]\"]\n\
                      /// a `#![allow(..)]` in a doc comment\n#[allow(dead_code)]\nfn f() {}\n\
                      fn g() { let s = \"#![allow(y)]\"; }\n";
        assert_eq!(found(source), vec![]);
    }

    #[test]
    fn code_chars_skip_comments_and_literals() {
        // Every `]` below is inside a comment or a literal; everything else is code.
        let source = "a '\"' \"]\" // ]\nb /* ] /* ] */ ] */ c r#\"]\"# b'\\'' '\\u{1F600}' 'é' \
                      impl<'a> 'l: loop {} /*/ ] */ d /**/ e";
        let code = code_only(source);
        assert_eq!(
            code.split_whitespace().collect::<Vec<_>>().join(" "),
            "a b c r# b impl<'a> 'l: loop {} d e"
        );
        // Neither an unterminated literal nor a lone trailing quote panics.
        assert_eq!(code_only("x '"), "x '");
        assert_eq!(code_only("x \"unterminated"), "x ");
        assert_eq!(code_only("/* unterminated"), "");
        assert_eq!(code_only("'é' => {}"), " => {}");
    }

    #[test]
    fn an_allow_inside_a_comment_or_string_is_not_a_suppression() {
        for source in [
            "/*\n#![allow(dead_code)]\n*/\n",
            "/* /* */ #![allow(x)] */\n",
            "const S: &str = \"\n#![allow(x)]\n\";\n",
            "const S: &str = r##\"\"# #![allow(x)]\"##;\n",
            "// #![allow(x)]\n//! #![allow(y)]\n",
        ] {
            assert_eq!(found(source), vec![], "{source:?}");
        }
    }

    #[test]
    fn a_char_literal_or_multiline_string_in_an_attribute_does_not_hide_later_ones() {
        for source in [
            "#![doc = '\"']\n#![allow(x)]\n",
            "#![doc = \"a\nb\"]\n#![allow(x)]\n",
            "#![allow(x, reason = \"see http://x/*y*/\")]\n",
        ] {
            assert_eq!(found(source), vec![allow("x")], "{source:?}");
        }
    }

    #[test]
    fn inner_attributes_inside_an_item_body_are_not_file_level() {
        let source = "#![allow(a)]\nmod t {\n    #![allow(b)]\n}\nfn f() {\n    #![allow(c)]\n}\n\
                      macro_rules! m { () => { #![allow(d)] }; }\n";
        assert_eq!(found(source), vec![allow("a")]);
    }

    #[test]
    fn a_comment_inside_a_wrapped_list_is_not_a_lint() {
        assert_eq!(
            found("#![allow(\n    dead_code, // why\n    unused,\n)]\n"),
            vec![allow("dead_code"), allow("unused")]
        );
        assert_eq!(
            found("#![cfg_attr(test, // why\n    allow(x))]\n"),
            vec![allow("x")]
        );
    }

    #[test]
    fn comments_around_attribute_names_do_not_hide_suppressions() {
        for (attribute, kind) in [
            ("allow(dead_code)", "allow"),
            ("expect(dead_code)", "expect"),
            ("cfg_attr(test, allow(dead_code))", "allow"),
        ] {
            for comment in ["/* why */", "// why\n"] {
                for body in [
                    format!("{comment}{attribute}"),
                    attribute.replacen('(', &format!("{comment}("), 1),
                ] {
                    let source = format!("#![{body}]");
                    assert_eq!(
                        found(&source),
                        vec![(kind, "dead_code".to_owned())],
                        "{source}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_shebang_and_adjacent_or_unterminated_attributes() {
        assert_eq!(
            found("#!/usr/bin/env cargo\n#![allow(x)]\n"),
            vec![allow("x")]
        );
        assert_eq!(
            found("#![allow(x)]#![allow(y)]"),
            vec![allow("x"), allow("y")]
        );
        assert_eq!(found("#![allow(x"), vec![]);
    }

    /// Restores `path`'s mode on drop, so a failed assertion cannot leave a tempdir that
    /// cannot be deleted.
    #[cfg(unix)]
    struct RestoreMode<'a>(&'a Path);

    #[cfg(unix)]
    impl Drop for RestoreMode<'_> {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// Makes `path` unreadable until the guard drops.
    #[cfg(unix)]
    fn locked(path: &Path) -> RestoreMode<'_> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        RestoreMode(path)
    }

    /// Root reads a mode-000 directory regardless, so the two tests below prove nothing there.
    #[cfg(unix)]
    fn is_root() -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    #[cfg(unix)]
    #[test]
    fn rust_sources_skips_an_unreadable_subdirectory() {
        if is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let locked_dir = root.path().join("locked");
        std::fs::create_dir(&locked_dir).unwrap();
        std::fs::write(root.path().join("a.rs"), "").unwrap();
        std::fs::write(locked_dir.join("b.rs"), "").unwrap();
        let _restore = locked(&locked_dir);

        let mut files = Vec::new();
        rust_sources(root.path(), root.path(), &mut files).unwrap();

        assert!(files.contains(&PathBuf::from("a.rs")), "{files:?}");
        assert!(!files.contains(&PathBuf::from("locked/b.rs")), "{files:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rust_sources_fails_when_the_root_is_unreadable() {
        if is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let _restore = locked(root.path());

        let mut files = Vec::new();
        assert!(rust_sources(root.path(), root.path(), &mut files).is_err());
    }

    #[test]
    fn rust_sources_skips_vendored_dependencies() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("vendor/steel-core")).unwrap();
        std::fs::create_dir_all(root.path().join("src/vendor")).unwrap();
        std::fs::write(root.path().join("vendor/steel-core/lib.rs"), "").unwrap();
        std::fs::write(root.path().join("src/vendor/adapter.rs"), "").unwrap();
        let mut files = Vec::new();
        rust_sources(root.path(), root.path(), &mut files).unwrap();
        assert_eq!(files, vec![PathBuf::from("src/vendor/adapter.rs")]);
    }
}
