//! One offline policy for CLI and Agent. Only owned copies reach native tools.
use super::{checker, loader, worker};
use std::{
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use yinz_runtime::{errors::prelude::*, owned_dir::OwnedDir};
use yinz_types::authoring::PluginCheckMode;
#[path = "authoring/bundle.rs"]
pub(crate) mod bundle;

pub(super) fn run(
    path: &Path,
    source: &str,
    mode: PluginCheckMode,
    repository_root: Option<&Path>,
) -> Result<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    // This compiler host resolves every import using the runtime loader's confinement checks.
    // It does not evaluate source, discover configs, or resolve packages.
    let test_declarations = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".test.ts"));
    let graph = checker::authoring_graph(path, source, false, repository_root, test_declarations)?;
    let tools = tools_dir()?;
    let scratch = OwnedDir::new(&yinz_runtime::consts::YINZ_CACHE_DIR.join("authoring-tmp"))?;
    let root = scratch.path();
    let source_root = root.join("source");
    let stage_path = |original: &Path| -> Result<PathBuf> {
        if let Ok(relative) = original.strip_prefix(loader::EMBEDDED_ROOT) {
            return Ok(root.join("sdk/builtins").join(relative));
        }
        let relative = original
            .strip_prefix(&graph.root)
            .context("checked file escapes plugin root")?;
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("checked file escapes plugin root: {}", original.display());
        }
        Ok(source_root.join(relative))
    };
    let mut mappings = Vec::new();
    for (original, contents) in &graph.files {
        let staged = stage_path(original)?;
        std::fs::create_dir_all(staged.parent().context("staged file has no directory")?)?;
        let report = worker::analyze(original, contents)?;
        if !report.diagnostics.is_empty() {
            bail!("{}", report.diagnostics.join("\n"));
        }
        std::fs::write(&staged, contents)?;
        mappings.push((staged, original.clone()));
    }
    // Preserve in-root symlink import semantics without exposing any original filesystem path.
    for (alias, target) in &graph.aliases {
        let alias = stage_path(alias)?;
        let target = stage_path(target)?;
        if !alias.exists() {
            std::fs::create_dir_all(alias.parent().context("alias has no directory")?)?;
            std::os::unix::fs::symlink(target, alias)?;
        }
    }
    std::fs::create_dir_all(root.join("sdk"))?;
    std::fs::write(root.join("sdk/sdk.d.ts"), include_str!("sdk.d.ts"))?;
    std::fs::write(
        root.join("oxlint.json"),
        include_str!("authoring/oxlint.json"),
    )?;
    std::fs::write(
        root.join("oxfmt.json"),
        include_str!("authoring/oxfmt.json"),
    )?;
    let mut files: Vec<PathBuf> = graph
        .files
        .keys()
        .map(|p| stage_path(p))
        .collect::<Result<_>>()?;
    let mut declarations = vec![root.join("sdk/sdk.d.ts")];
    if test_declarations {
        let testing = root.join("sdk/testing.d.ts");
        std::fs::write(&testing, include_str!("testing/testing.d.ts"))?;
        declarations.push(testing);
    }
    files.extend(declarations.iter().cloned());
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("authoring/tsconfig.json"))?;
    // tsgolint embeds its compiler libraries. Loading TS 5.9 libs as ordinary source
    // with noLib disables intrinsic Promise recognition and silently loses promise rules.
    config["compilerOptions"]["lib"] = serde_json::json!(["es2022"]);
    config["compilerOptions"]["paths"] = serde_json::json!({
        "yinz:plugin": ["./sdk/sdk.d.ts"]
    });
    if test_declarations {
        config["compilerOptions"]["paths"]["yinz:testing"] =
            serde_json::json!(["./sdk/testing.d.ts"]);
    }
    config["files"] = serde_json::to_value(files)?;
    std::fs::write(root.join("tsconfig.json"), serde_json::to_vec(&config)?)?;
    let entry = stage_path(&graph.entry)?;
    let execute = |tool: &str, args: &[&str], paths: &[PathBuf]| -> Result<()> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut command = Command::new(tools.join(tool));
        command
            .current_dir(root)
            .env_clear()
            .env("PATH", "")
            .env("HOME", root)
            .env("TMPDIR", root)
            .env("NO_COLOR", "1")
            .env("OXLINT_TSGOLINT_PATH", tools.join("tsgolint"))
            .args(args)
            .args(paths);
        let (status, stdout, stderr) = worker::supervise(&mut command, &[], remaining)?;
        if !status.success() {
            let mut message = format!(
                "{tool} exited with {status}:\n{}{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
            for (staged, original) in &mappings {
                message = message.replace(&*staged.to_string_lossy(), &original.to_string_lossy());
                if let Ok(relative) = staged.strip_prefix(root) {
                    message =
                        message.replace(&*relative.to_string_lossy(), &original.to_string_lossy());
                }
            }
            bail!("{message}");
        }
        Ok(())
    };
    let formatted = if matches!(mode, PluginCheckMode::Format | PluginCheckMode::Prepare) {
        execute(
            "oxfmt",
            &[
                "--config",
                "oxfmt.json",
                "--disable-nested-config",
                "--threads",
                "1",
                "--write",
            ],
            std::slice::from_ref(&entry),
        )?;
        Some(super::limits::read_source(&entry)?)
    } else {
        None
    };
    if mode != PluginCheckMode::Format {
        // TypeScript's established checker remains authoritative alongside tsgolint.
        // A symlinked entrypoint can sit below the original loader root; keep that boundary.
        let checked = checker::authoring_graph(
            &entry,
            formatted.as_deref().unwrap_or(source),
            true,
            Some(&source_root),
            test_declarations,
        )
        .map_err(|error| {
            let mut message = format!("{error:#}");
            for (staged, original) in &mappings {
                message = message.replace(&*staged.to_string_lossy(), &original.to_string_lossy());
                message = message.replace(
                    staged.to_string_lossy().trim_start_matches('/'),
                    &original.to_string_lossy(),
                );
            }
            anyhow!(message)
        })?;
        // Separate directories prevent TypeScript's extension substitution from choosing
        // foo.ts (or foo.d.ts) when the runtime loaded foo.js.
        let native_paths: std::collections::HashMap<_, _> = checked
            .files
            .keys()
            .enumerate()
            .map(|(index, path)| {
                (
                    path.clone(),
                    root.join("native")
                        .join(index.to_string())
                        .join(path.file_name().expect("module filename")),
                )
            })
            .collect();
        let mut native_mappings = Vec::new();
        let mut native_files = Vec::new();
        let mut lint_files = Vec::new();
        for (path, contents) in &checked.files {
            let native = &native_paths[path];
            let mut rewritten = contents.clone();
            let mut edits = Vec::new();
            for import in checked.imports.get(path).into_iter().flatten().rev() {
                // The SDK is a virtual runtime module backed by declarations through paths.
                // A literal .d.ts value import is illegal in TypeScript.
                if import.target == Path::new("/$yinz/sdk.d.ts")
                    || (test_declarations && import.target == Path::new("/$yinz/testing.d.ts"))
                {
                    continue;
                }
                let target = native_paths
                    .get(&import.target)
                    .context("missing resolved native import")?;
                let relative = target
                    .strip_prefix(root)
                    .context("native import escapes scratch")?;
                let replacement = serde_json::to_string(&format!("../../{}", relative.display()))?;
                let byte_offset = |offset| {
                    contents
                        .char_indices()
                        .scan(0, |units, (byte, ch)| {
                            let previous = *units;
                            *units += ch.len_utf16();
                            Some((previous, byte))
                        })
                        .find(|(units, _)| *units == offset)
                        .map(|(_, byte)| byte)
                        .unwrap_or(contents.len())
                };
                rewritten.replace_range(
                    byte_offset(import.start)..byte_offset(import.end),
                    &replacement,
                );
                // TypeScript spans use UTF-16; Oxlint's unix reporter uses byte columns.
                edits.push((
                    byte_offset(import.start),
                    byte_offset(import.end) - byte_offset(import.start),
                    replacement.len(),
                ));
            }
            edits.reverse();
            std::fs::create_dir_all(native.parent().context("native file has no parent")?)?;
            std::fs::write(native, &rewritten)?;
            let original = mappings
                .iter()
                .find(|(staged, _)| staged == path)
                .map(|(_, original)| original)
                .unwrap_or(path);
            native_mappings.push((native.clone(), original.clone(), contents, rewritten, edits));
            native_files.push(native.clone());
            if !original.starts_with(loader::EMBEDDED_ROOT) {
                lint_files.push(native.clone());
            }
        }
        config["files"] = serde_json::to_value(
            native_files
                .iter()
                .chain(declarations.iter())
                .collect::<Vec<_>>(),
        )?;
        std::fs::write(root.join("tsconfig.json"), serde_json::to_vec(&config)?)?;
        execute(
            "oxlint",
            &[
                "--config",
                "oxlint.json",
                "--disable-nested-config",
                "--no-ignore",
                "--threads",
                "1",
                "--type-aware",
                "--type-check",
                "--report-unused-disable-directives-severity",
                "error",
                "--deny-warnings",
                "--format",
                "unix",
            ],
            &lint_files,
        )
        .map_err(|error| {
            let mut message = format!("{error:#}");
            for (native, original, contents, rewritten, edits) in &native_mappings {
                for name in [
                    native.to_string_lossy().into_owned(),
                    native
                        .strip_prefix(root)
                        .expect("native root")
                        .to_string_lossy()
                        .into_owned(),
                ] {
                    message = message
                        .lines()
                        .map(|line| {
                            if let Some(rest) = line.strip_prefix(&format!("{name}:")) {
                                let mut parts = rest.splitn(3, ':');
                                if let (Some(row), Some(column), Some(tail)) =
                                    (parts.next(), parts.next(), parts.next())
                                {
                                    if let (Ok(row), Ok(mut column)) =
                                        (row.parse::<usize>(), column.parse::<usize>())
                                    {
                                        let mut offset = rewritten
                                            .split_inclusive('\n')
                                            .take(row.saturating_sub(1))
                                            .map(str::len)
                                            .sum::<usize>()
                                            + column.saturating_sub(1);
                                        let mut delta: isize = 0;
                                        for &(start, old, new) in edits {
                                            let staged_start = start.saturating_add_signed(delta);
                                            if offset < staged_start {
                                                break;
                                            }
                                            if offset < staged_start + new {
                                                offset = start
                                                    + (offset - staged_start)
                                                        .min(old.saturating_sub(1));
                                                delta = 0;
                                                break;
                                            }
                                            delta += new as isize - old as isize;
                                        }
                                        offset = offset.saturating_add_signed(-delta);
                                        let mut row = 1;
                                        column = 1;
                                        for line in contents.split_inclusive('\n') {
                                            if offset < line.len() {
                                                column = offset + 1;
                                                break;
                                            }
                                            offset -= line.len();
                                            row += 1;
                                        }
                                        return format!(
                                            "{}:{row}:{column}:{tail}",
                                            original.display()
                                        );
                                    }
                                }
                            }
                            line.replace(&name, &original.to_string_lossy())
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
            anyhow!(message)
        })?;
    }
    Ok(formatted)
}

fn tools_dir() -> Result<PathBuf> {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/authoring-tools")
        .join(bundle::ID);
    let candidates = [
        checkout,
        yinz_runtime::home::xdg_data_dir()
            .join("authoring-tools")
            .join(yinz_runtime::consts::VERSION)
            .join(bundle::ID),
        yinz_runtime::home::system_data_dir()
            .join("authoring-tools")
            .join(yinz_runtime::consts::VERSION)
            .join(bundle::ID),
    ];
    let dir = candidates
        .iter()
        .find(|path| path.is_dir())
        .context("bundled authoring tools are missing; run cargo x build or reinstall Yinz")?;
    bundle::verify_dir(dir)?;
    dir.canonicalize()
        .context("resolve bundled authoring tools")
}

#[cfg(test)]
#[path = "unit/authoring.rs"]
mod tests;
