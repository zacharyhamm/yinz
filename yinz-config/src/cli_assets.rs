use yinz_types::data::LayoutInfo;
use {
    crate::assets::get_default_themes, crate::config::Config, crate::theme::Themes,
    yinz_runtime::home::find_default_config_dir, yinz_runtime::home::get_theme_dir,
    yinz_types::input::layout::Layout,
};

use yinz_types::input::cli_assets::CliAssets;

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

    config_with_merged_layout_opts.themes =
        get_default_themes().merge(config_with_merged_layout_opts.themes);

    let user_theme_dir = assets
        .configuration_options
        .as_ref()
        .and_then(|o| o.theme_dir.clone())
        .or_else(|| {
            config_with_merged_layout_opts
                .options
                .theme_dir
                .clone()
                .or_else(|| {
                    get_theme_dir(assets.config_dir.clone().or_else(find_default_config_dir))
                })
                .filter(|dir| dir.exists())
        });
    if let Some(themes) = user_theme_dir.and_then(|u| Themes::from_dir(u).ok()) {
        config_with_merged_layout_opts.themes = config_with_merged_layout_opts.themes.merge(themes);
    }

    let config = config_with_merged_layout_opts
        .resolve_plugin_repositories(
            assets.config_file_path.as_deref(),
            assets.config_dir.as_deref(),
        )
        .map_err(|error| anyhow::anyhow!("failed to resolve plugin repositories: {error}"))?;
    Ok((config, layout))
}
