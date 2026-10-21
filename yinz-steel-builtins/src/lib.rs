//! Disk locations and installation of builtin Steel Scheme plugins.
use std::path::{Path, PathBuf};
use yinz_runtime::errors::prelude::*;

pub fn builtin_path(data_dir: &Path, tag: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        yinz_runtime::consts::BUILTIN_PLUGIN_NAMES.contains(&tag),
        "unknown builtin plugin '{tag}'"
    );
    let path = data_dir.join("plugins/builtins").join(format!("{tag}.scm"));
    anyhow::ensure!(
        path.exists(),
        "builtin plugin is missing at {}; reinstall Yinz with --data-dir {}",
        path.display(),
        data_dir.display()
    );
    Ok(path)
}

/// Install the shipped tree, replacing shipped files but preserving unrelated files.
pub fn install(source: &Path, data_dir: &Path) -> Result<()> {
    copy_tree(source, &data_dir.join("plugins/builtins"))
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    anyhow::ensure!(
        !destination.is_symlink(),
        "builtin directory is a symlink: {}",
        destination.display()
    );
    std::fs::create_dir_all(destination).context("create builtin directory")?;
    for entry in std::fs::read_dir(source).context("read builtin assets")? {
        let entry = entry.context("read builtin asset")?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().context("inspect builtin asset")?;
        anyhow::ensure!(
            kind.is_dir() || kind.is_file(),
            "builtin asset must be a regular file or directory: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            anyhow::ensure!(
                !target.is_symlink(),
                "builtin directory is a symlink: {}",
                target.display()
            );
            copy_tree(&entry.path(), &target)?;
        } else {
            let mut staged =
                tempfile::NamedTempFile::new_in(destination).context("stage builtin asset")?;
            let mut input = std::fs::File::open(entry.path()).context("open builtin asset")?;
            std::io::copy(&mut input, &mut staged).context("copy builtin asset")?;
            staged
                .as_file()
                .set_permissions(
                    input
                        .metadata()
                        .context("inspect builtin permissions")?
                        .permissions(),
                )
                .context("set builtin permissions")?;
            staged
                .persist(&target)
                .with_context(|| format!("install builtin {}", target.display()))?;
        }
    }
    Ok(())
}

/// Source package assets, for development and test fixtures only.
pub fn asset_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/plugins")
}

/// Read a source-package fixture, never used for runtime builtin lookup.
pub fn fixture_source(tag: &str) -> Option<String> {
    std::fs::read_to_string(asset_dir().join(format!("{tag}.scm"))).ok()
}

#[cfg(test)]
#[path = "unit/shared.rs"]
mod shared_tests;

#[cfg(test)]
#[path = "unit/plugins.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/palette.rs"]
mod palette_tests;

#[cfg(test)]
#[path = "unit/configuration.rs"]
mod configuration_tests;

#[cfg(test)]
#[path = "unit/agent_exit.rs"]
mod agent_exit_tests;

#[cfg(test)]
#[path = "unit/bars.rs"]
mod bar_tests;

#[cfg(test)]
#[path = "unit/status_bar.rs"]
mod status_bar_tests;

#[cfg(test)]
#[path = "unit/compact_bar.rs"]
mod compact_bar_tests;

#[cfg(test)]
#[path = "unit/bar_tips.rs"]
mod bar_tips_tests;

#[cfg(test)]
#[path = "unit/strider.rs"]
mod strider_tests;

#[cfg(test)]
#[path = "unit/fixture.rs"]
mod fixture_tests;

#[cfg(test)]
#[path = "unit/file_finder.rs"]
mod file_finder_tests;

#[cfg(test)]
#[path = "unit/configuration_ui.rs"]
mod configuration_ui_tests;

#[cfg(test)]
#[path = "unit/file_finder_listing.rs"]
mod file_finder_listing_tests;

#[cfg(test)]
#[path = "unit/agent_service.rs"]
mod agent_service_tests;

#[cfg(test)]
#[path = "unit/project_store.rs"]
mod project_store_tests;

#[cfg(test)]
#[path = "unit/todos.rs"]
mod todos_tests;

#[cfg(test)]
#[path = "unit/install.rs"]
mod install_tests;
