//! One offline policy for CLI and Hocus. Only owned copies reach native tools.
use super::{checker, loader, worker};
use std::{
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use zellij_runtime::{errors::prelude::*, owned_dir::OwnedDir};
use zellij_types::authoring::PluginCheckMode;
#[path = "authoring/bundle.rs"]
pub(crate) mod bundle;

pub(super) fn run(path: &Path, source: &str, mode: PluginCheckMode) -> Result<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    // This compiler host resolves every import using the runtime loader's confinement checks.
    // It does not evaluate source, discover configs, or resolve packages.
    let graph = checker::authoring_graph(path, source, false, None)?;
    let tools = tools_dir()?;
    let scratch = OwnedDir::new(&zellij_runtime::consts::ZELLIJ_CACHE_DIR.join("authoring-tmp"))?;
    let root = scratch.path();
    let source_root = root.join("source");
    let stage_path = |original: &Path| -> Result<PathBuf> {
        if original == Path::new(loader::WORKFLOW_PATH) {
            return Ok(root.join("sdk/workflow-view.ts"));
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
    let mut lint_files = Vec::new();
    for (original, contents) in &graph.files {
        let staged = stage_path(original)?;
        std::fs::create_dir_all(staged.parent().context("staged file has no directory")?)?;
        let report = worker::analyze(original, contents)?;
        if !report.diagnostics.is_empty() {
            bail!("{}", report.diagnostics.join("\n"));
        }
        std::fs::write(&staged, contents)?;
        if original != Path::new(loader::WORKFLOW_PATH) {
            lint_files.push(staged.clone());
        }
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
    files.push(root.join("sdk/sdk.d.ts"));
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("authoring/tsconfig.json"))?;
    // tsgolint embeds its compiler libraries. Loading TS 5.9 libs as ordinary source
    // with noLib disables intrinsic Promise recognition and silently loses promise rules.
    config["compilerOptions"]["lib"] = serde_json::json!(["es2022"]);
    config["compilerOptions"]["paths"] = serde_json::json!({
        "zellij:plugin": ["./sdk/sdk.d.ts"],
        "zellij:workflow-view": ["./sdk/workflow-view.ts"]
    });
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
        checker::authoring_graph(
            &entry,
            formatted.as_deref().unwrap_or(source),
            true,
            Some(&source_root),
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
        )?;
    }
    Ok(formatted)
}

fn tools_dir() -> Result<PathBuf> {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/authoring-tools")
        .join(bundle::ID);
    let candidates = [
        checkout,
        zellij_runtime::home::xdg_data_dir()
            .join("authoring-tools")
            .join(zellij_runtime::consts::VERSION)
            .join(bundle::ID),
        zellij_runtime::home::system_data_dir()
            .join("authoring-tools")
            .join(zellij_runtime::consts::VERSION)
            .join(bundle::ID),
    ];
    let dir = candidates
        .iter()
        .find(|path| path.is_dir())
        .context("bundled authoring tools are missing; run cargo x build or reinstall Zellij")?;
    bundle::verify_dir(dir)?;
    dir.canonicalize()
        .context("resolve bundled authoring tools")
}

#[cfg(test)]
#[path = "unit/authoring.rs"]
mod tests;
