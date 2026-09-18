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
    check_file_level_allows(sh).context(err_context)?;

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
/// is a `(path, lint)` pair; a `#![allow(..)]` or `#![expect(..)]` anywhere else fails the
/// lint task before clippy runs, and an entry that no longer matches fails it too, so the list
/// only ever shrinks by hand.
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

/// Fails when a Rust source file carries a file-level lint suppression that
/// [`FILE_LEVEL_ALLOWS`] does not name, or when an entry there names a file that no longer
/// carries it.
fn check_file_level_allows(_sh: &Shell) -> anyhow::Result<()> {
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
        for line in source.lines().map(str::trim_start) {
            let Some(rest) = line
                .strip_prefix("#![allow(")
                .or_else(|| line.strip_prefix("#![expect("))
            else {
                continue;
            };
            let Some(lints) = rest.split_once(')').map(|(lints, _)| lints) else {
                continue;
            };
            for lint in lints.split(',').map(str::trim).filter(|l| !l.is_empty()) {
                if FILE_LEVEL_ALLOWS.contains(&(path, lint)) {
                    seen.insert((path.to_owned(), lint.to_owned()));
                } else {
                    offenders.push(format!("{path}: #![allow({lint})]"));
                }
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
