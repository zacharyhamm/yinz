use yinz_types::data::LayoutInfo;
use {
    crate::assets::get_default_themes, crate::config::Config, crate::theme::Themes,
    yinz_runtime::errors::prelude::*, yinz_runtime::home::find_default_config_dir,
    yinz_types::input::layout::Layout,
};

use yinz_types::input::cli_assets::CliAssets;

/// Merges the built-in themes beneath `config`'s own `themes` block and the user's theme
/// directory above it. An unreadable theme directory is logged and skipped.
///
/// Serialized configuration omits file-sourced themes, so a configuration rebuilt from its own
/// KDL - as an in-place update's replacement server does - needs this to regain them.
pub fn attach_themes(config: &mut Config, assets: &CliAssets) {
    crate::config::apply_themes(
        config,
        assets.configuration_options.clone(),
        assets.config_dir.clone(),
    )
    .map_err(|error| anyhow!("{error}"))
    .context("load the user's theme directory")
    .non_fatal();
}

/// `config`'s file-sourced themes as a KDL `themes` document for an in-place update payload, or
/// empty when it has none. Serialized configuration omits them, and the theme directory may have
/// changed since the running server read it. Unmodified built-in themes are left out: the
/// replacement ships its own.
pub fn file_themes_to_string(config: &Config) -> String {
    let builtins = get_default_themes();
    let mut document = kdl::KdlDocument::new();
    if let Some(node) = config.themes.to_kdl_where(|name, theme| {
        theme.sourced_from_external_file && builtins.get_theme(name) != Some(theme)
    }) {
        document.nodes_mut().push(node);
    }
    document.to_string()
}

/// Gives a configuration decoded from an in-place update's KDL the themes serialization left out:
/// the built-ins beneath its own `themes` block and `carried`, from [`file_themes_to_string`],
/// above it. Sources that predate carrying themes send `None`; the theme directory is reread.
pub fn restore_themes(
    config: &mut Config,
    carried: Option<&str>,
    assets: &CliAssets,
) -> anyhow::Result<()> {
    let Some(carried) = carried else {
        attach_themes(config, assets);
        return Ok(());
    };
    config.themes = get_default_themes().merge(std::mem::take(&mut config.themes));
    if !carried.trim().is_empty() {
        let sourced_from_external_file = true;
        let themes = Themes::from_string(carried, sourced_from_external_file)
            .map_err(|error| anyhow!("{error}"))
            .context("decode carried file themes")?;
        config.themes = config.themes.merge(themes);
    }
    Ok(())
}

pub fn load_config_and_layout(assets: &CliAssets) -> anyhow::Result<(Config, Layout)> {
    let config = {
        if assets.should_ignore_config {
            Config::from_default_assets().unwrap_or_else(|_| Default::default())
        } else if let Some(ref path) = assets.config_file_path {
            let default_config =
                Config::from_default_assets().unwrap_or_else(|_| Default::default());
            let overlay_dir = yinz_runtime::home::get_config_overlay_dir(
                assets
                    .config_dir
                    .clone()
                    .or_else(yinz_runtime::home::find_default_config_dir),
            );
            Config::from_path(path, Some(default_config.clone()))
                .unwrap_or(default_config)
                .with_overlay_dir(overlay_dir.as_deref())
        } else {
            Config::from_default_assets().unwrap_or_else(|_| Default::default())
        }
    };

    let (mut layout, mut config_with_merged_layout_opts) = {
        let layout_dir = assets
            .configuration_options
            .as_ref()
            .and_then(|e| e.layout_dir.clone())
            .or_else(|| config.options.layout_dir.clone())
            .or_else(|| {
                assets
                    .config_dir
                    .clone()
                    .or_else(find_default_config_dir)
                    .map(|dir| dir.join("layouts"))
            });
        // Resurrection paths cross IPC before they are opened. Resolve and lock
        // the checkpoint here, where parsing also opens its pane-content files.
        let checkpoint = match assets.layout.as_ref() {
            Some(LayoutInfo::File(path, _))
                if std::path::Path::new(path).is_absolute()
                    && std::path::Path::new(path).file_name()
                        == Some(std::ffi::OsStr::new(
                            yinz_runtime::session_checkpoint::LAYOUT,
                        )) =>
            {
                Some(yinz_runtime::session_checkpoint::SessionCheckpoint::read(
                    std::path::Path::new(path)
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new(".")),
                )?)
            },
            _ => None,
        };
        if let Some(checkpoint) = checkpoint.as_ref() {
            Some(
                crate::layout::from_path_or_default(
                    Some(&checkpoint.layout_path()),
                    None,
                    config.clone(),
                )
                .map_err(|error| anyhow::anyhow!("failed to parse checkpoint layout: {error}"))?,
            )
        } else {
            assets.layout.as_ref().and_then(|layout_info| {
                crate::layout::from_layout_info_with_config(
                    &layout_dir,
                    layout_info,
                    Some(config.clone()),
                )
                .ok()
            })
        }
    }
    .unwrap_or_else(|| (crate::layout::default_layout_asset(), config));

    if assets.force_run_layout_commands {
        layout.recursively_add_start_suspended(Some(false));
    }

    attach_themes(&mut config_with_merged_layout_opts, assets);

    let config = config_with_merged_layout_opts
        .resolve_plugin_repositories(
            assets.config_file_path.as_deref(),
            assets.config_dir.as_deref(),
        )
        .map_err(|error| anyhow::anyhow!("failed to resolve plugin repositories: {error}"))?;
    Ok((config, layout))
}

#[cfg(test)]
#[path = "unit/cli_assets_tests.rs"]
mod tests;
