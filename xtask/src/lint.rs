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
    check_add_plugin_owners().context(err_context)?;

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
        "yinz-host-protocol/src/nested_session_contract/mod.rs",
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
fn lints<'a>(kind: &str, args: &'a str) -> impl Iterator<Item = Suppression> + 'a + use<'a> {
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

/// The only files outside tests that may construct `ScreenInstruction::AddPlugin`: the plugin
/// host's pane ledger, which decides whether a plugin gets a pane at all (an undeclared plugin
/// must never get one), and the screen's re-park of one that waits for a client.
const ADD_PLUGIN_OWNERS: &[&str] = &[
    "yinz-plugin-host/src/plugin_panes.rs",
    "yinz-screen/src/screen/plugins.rs",
];

/// Fails when code outside [`ADD_PLUGIN_OWNERS`] and outside tests constructs
/// `ScreenInstruction::AddPlugin`, by whatever path, or imports the variant.
fn check_add_plugin_owners() -> anyhow::Result<()> {
    let offenders = add_plugin_offenders(&crate::project_root())?;
    if offenders.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "`ScreenInstruction::AddPlugin` is constructed only by the pane ledger \
         (yinz-plugin-host/src/plugin_panes.rs): ask it for a pane with a `PaneRequest`\n",
    );
    for offender in &offenders {
        msg.push_str("  ");
        msg.push_str(offender);
        msg.push('\n');
    }
    Err(anyhow!(msg))
}

/// Each source file under `root`, outside the owners and tests, that constructs or imports
/// `ScreenInstruction::AddPlugin`, with how many times. A file that cannot be read is an error:
/// the check cannot vouch for what it did not see.
fn add_plugin_offenders(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    rust_sources(root, root, &mut files)
        .with_context(|| format!("failed to read {}", root.display()))?;
    let mut offenders = Vec::new();
    for path in files {
        let shown = path.to_string_lossy().into_owned();
        let in_tests = path
            .components()
            .any(|component| matches!(component.as_os_str().to_str(), Some("unit" | "tests")));
        if in_tests || ADD_PLUGIN_OWNERS.contains(&shown.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(root.join(&path)).with_context(|| {
            format!("failed to read {shown} to check who constructs `ScreenInstruction::AddPlugin`")
        })?;
        let constructed = add_plugin_constructions(&source);
        if constructed > 0 {
            offenders.push(format!("{shown}: {constructed}"));
        }
    }
    Ok(offenders)
}

const ADD_PLUGIN: &str = "AddPlugin";

/// How many times `source` constructs `ScreenInstruction::AddPlugin` or imports it, outside
/// comments, literals and inline `#[cfg(test)]` modules. Any path to the variant counts: an
/// alias of the enum (`I::AddPlugin(..)`), `Self::AddPlugin(..)`, a bare `AddPlugin(..)` after
/// an import, spaces around `::`, and the variant passed as a function. A pattern that matches
/// one (followed by `=>`, `|`, `if` or `=`, or the last argument of `matches!`) is not a
/// construction, nor is the variant's declaration in `enum ScreenInstruction` or the unit
/// variant `ScreenContext::AddPlugin`. A glob import of the enum's variants
/// (`use ScreenInstruction::*`) counts too, since the bare name it brings in can be used in
/// ways no text match can tell from another `AddPlugin`.
fn add_plugin_constructions(source: &str) -> usize {
    let code = without_inline_test_modules(&code_only(source));
    let declarations = item_bodies(&code, "enum ScreenInstruction");
    let imports = use_items(&code);
    let mut count = imports
        .iter()
        .filter(|item| globs_screen_instruction_variants(&code[(*item).clone()]))
        .count();
    for (start, _) in code.match_indices(ADD_PLUGIN) {
        let end = start + ADD_PLUGIN.len();
        let identifier_char = |c: char| c.is_alphanumeric() || c == '_';
        if code[..start].ends_with(identifier_char) || code[end..].starts_with(identifier_char) {
            continue;
        }
        if declarations.iter().any(|body| body.contains(&start)) {
            continue;
        }
        if imports.iter().any(|item| item.contains(&start)) {
            count += 1;
            continue;
        }
        let (path, qualifier) = path_before(&code, start);
        if qualifier == Some("ScreenContext") {
            continue;
        }
        let after = code[end..].trim_start();
        if !after.starts_with('(') {
            // Named without arguments: the variant as a constructor function, as in
            // `.map(ScreenInstruction::AddPlugin)`.
            count += usize::from(qualifier.is_some());
            continue;
        }
        let open = code.len() - after.len();
        let Some(close) = matching(&code, open, b'(', b')') else {
            count += 1;
            continue;
        };
        let after = code[close + 1..].trim_start();
        let before = code[..path].trim_end();
        let pattern = after.starts_with("=>")
            || after.starts_with('|')
            || after.starts_with("if ")
            || (after.starts_with('=') && !after.starts_with("=="))
            || (before.ends_with(',') && after.starts_with(')') && in_matches(&code, path));
        if !pattern {
            count += 1;
        }
    }
    count
}

/// Where the path ending in the segment at `at` starts (`a::b::` before it, spaces allowed),
/// and the segment just before that one, if any.
fn path_before(code: &str, at: usize) -> (usize, Option<&str>) {
    let mut start = at;
    let mut qualifier = None;
    while let Some(rest) = code[..start].trim_end().strip_suffix("::") {
        let rest = rest.trim_end();
        let segment = rest.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        start = segment.len();
        if qualifier.is_none() {
            qualifier = Some(&rest[segment.len()..]);
        }
        if segment.len() == rest.len() {
            // A leading `::`.
            break;
        }
    }
    (start, qualifier.filter(|segment| !segment.is_empty()))
}

/// Whether the `use` item `item` imports every variant of `ScreenInstruction`, directly
/// (`ScreenInstruction::*`) or in a group (`ScreenInstruction::{self, *}`).
fn globs_screen_instruction_variants(item: &str) -> bool {
    let item: String = item.chars().filter(|c| !c.is_whitespace()).collect();
    item.match_indices("ScreenInstruction::").any(|(at, name)| {
        let identifier_char = |c: char| c.is_alphanumeric() || c == '_';
        if item[..at].ends_with(identifier_char) {
            return false;
        }
        let rest = &item[at + name.len()..];
        if rest.starts_with('*') {
            return true;
        }
        rest.starts_with('{')
            && matching(rest, 0, b'{', b'}').is_some_and(|close| rest[..close].contains('*'))
    })
}

/// The byte ranges of the `use` items in `code`, keyword to semicolon.
fn use_items(code: &str) -> Vec<std::ops::Range<usize>> {
    code.match_indices("use")
        .filter(|(at, _)| {
            let before = code[..*at].chars().next_back();
            let after = code[at + 3..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && after.is_some_and(char::is_whitespace)
        })
        .map(|(at, _)| at..code[at..].find(';').map_or(code.len(), |end| at + end))
        .collect()
}

/// The byte ranges of the braced bodies of the items `header` introduces, such as
/// `enum ScreenInstruction`.
fn item_bodies(code: &str, header: &str) -> Vec<std::ops::Range<usize>> {
    code.match_indices(header)
        .filter(|(at, _)| {
            !code[at + header.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        })
        .filter_map(|(at, _)| {
            let open = at + code[at..].find('{')?;
            let close = matching(code, open, b'{', b'}')?;
            Some(open..close)
        })
        .collect()
}

/// Whether `at` is inside the arguments of a `matches!`: a pattern there is not a
/// construction, while the last argument of any other call is.
fn in_matches(code: &str, at: usize) -> bool {
    let mut depth = 0usize;
    for (index, byte) in code[..at].bytes().enumerate().rev() {
        match byte {
            b')' => depth += 1,
            b'(' if depth == 0 => return code[..index].trim_end().ends_with("matches!"),
            b'(' => depth -= 1,
            _ => {},
        }
    }
    false
}

/// `code` without the bodies of `#[cfg(test)] mod name { .. }` modules.
fn without_inline_test_modules(code: &str) -> String {
    let mut code = code.to_owned();
    let mut from = 0;
    while let Some(found) = code[from..].find("#[cfg(test)]") {
        let start = from + found;
        from = start + 1;
        let mut rest = code[start + "#[cfg(test)]".len()..].trim_start();
        // Other attributes may sit between the gate and the module.
        while rest.starts_with("#[") {
            let at = code.len() - rest.len() + 1;
            let Some(close) = matching(&code, at, b'[', b']') else {
                break;
            };
            rest = code[close + 1..].trim_start();
        }
        let Some(item) = rest.strip_prefix("mod ") else {
            continue;
        };
        let header = &item[..item.find(['{', ';']).unwrap_or(item.len())];
        if !item[header.len()..].starts_with('{') {
            continue;
        }
        let open = code.len() - item.len() + header.len();
        let end = matching(&code, open, b'{', b'}').map_or(code.len(), |close| close + 1);
        code.replace_range(start..end, "");
        from = start;
    }
    code
}

/// The index of the bracket closing the one at `open`.
fn matching(code: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in code.bytes().enumerate().skip(open) {
        if byte == opening {
            depth += 1;
        } else if byte == closing {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "unit/lint.rs"]
mod tests;
