//! Layout loading and parsing. Pure layout models live in zellij-types.
#[cfg(not(target_family = "wasm"))]
use zellij_downloader::Downloader;
use {
    crate::config::Config, crate::config::ConfigError, zellij_runtime::home::default_layout_dir,
    zellij_runtime::home::find_default_config_dir, zellij_types::data::LayoutInfo,
    zellij_types::data::LayoutMetadata, zellij_types::data::LayoutParsingError,
    zellij_types::data::LayoutWithError, zellij_types::data::TabMetadata,
};

use std::cmp::Ordering;

use std::path::{Path, PathBuf};
use std::vec::Vec;
use std::{fs::File, io::prelude::*};

use zellij_types::input::layout::*;

pub fn list_available_layouts(
    layout_dir: Option<PathBuf>,
    default_layout_name: &Option<String>,
) -> (Vec<LayoutInfo>, Vec<LayoutWithError>) {
    let (mut available_layouts, layouts_with_errors) = layout_dir
        .clone()
        .or_else(default_layout_dir)
        .and_then(|layout_dir| match std::fs::read_dir(&layout_dir) {
            Ok(layout_files) => Some((layout_files, layout_dir)),
            Err(_) => None,
        })
        .map(|(layout_files, layout_dir)| {
            let mut available_layouts = vec![];
            let mut layouts_with_errors = vec![];
            for file in layout_files.flatten() {
                if file.path().extension().map(|e| e.to_ascii_lowercase())
                    == Some(std::ffi::OsString::from("kdl"))
                {
                    let layout_name = file
                        .path()
                        .file_stem()
                        .and_then(|f| f.to_str())
                        .map(|f| f.to_string())
                        .unwrap_or_default();

                    match crate::layout::from_path_or_default_without_config(
                        Some(&file.path()),
                        Some(layout_dir.clone()),
                    ) {
                        Ok(_layout) => {
                            let file_path = file.path();
                            available_layouts.push(LayoutInfo::File(
                                layout_name,
                                crate::layout::metadata_from_path(&file_path),
                            ))
                        },
                        Err(config_error) => {
                            let file_path = file.path();
                            let file_name = file_path.to_str().unwrap_or("unknown").to_string();
                            let source_code =
                                std::fs::read_to_string(&file_path).unwrap_or_default();

                            let error = match config_error {
                                ConfigError::KdlError(kdl_err) => LayoutParsingError::KdlError {
                                    kdl_error: Box::new(kdl_err.into()),
                                    file_name,
                                    source_code,
                                },
                                _ => LayoutParsingError::SyntaxError,
                            };
                            layouts_with_errors.push(LayoutWithError { layout_name, error });
                        },
                    }
                }
            }
            (available_layouts, layouts_with_errors)
        })
        .unwrap_or_else(|| (Default::default(), Default::default()));
    let default_layout_name = default_layout_name
        .as_ref()
        .map(|d| d.as_str())
        .unwrap_or("default");
    available_layouts.push(LayoutInfo::BuiltIn("default".to_owned()));
    available_layouts.push(LayoutInfo::BuiltIn("strider".to_owned()));
    available_layouts.push(LayoutInfo::BuiltIn("disable-status-bar".to_owned()));
    available_layouts.push(LayoutInfo::BuiltIn("compact".to_owned()));
    available_layouts.push(LayoutInfo::BuiltIn("classic".to_owned()));
    available_layouts.sort_by(|a, b| {
        let a_name = a.name();
        let b_name = b.name();
        if a_name == default_layout_name {
            Ordering::Less
        } else if b_name == default_layout_name {
            Ordering::Greater
        } else {
            a_name.cmp(b_name)
        }
    });
    (available_layouts, layouts_with_errors)
}
pub fn from_layout_info(
    layout_dir: &Option<PathBuf>,
    layout_info: LayoutInfo,
) -> Result<Layout, ConfigError> {
    let mut should_start_layout_commands_suspended = false;
    let (path_to_raw_layout, raw_layout, raw_swap_layouts) = match layout_info {
        LayoutInfo::File(layout_name_without_extension, _layout_metadata) => {
            let layout_dir = layout_dir.clone().or_else(default_layout_dir);
            let (path_to_layout, stringified_layout, swap_layouts) =
                crate::layout::stringified_from_dir(
                    &PathBuf::from(layout_name_without_extension),
                    layout_dir.as_ref(),
                )?;
            (Some(path_to_layout), stringified_layout, swap_layouts)
        },
        LayoutInfo::BuiltIn(layout_name) => {
            let (path_to_layout, stringified_layout, swap_layouts) =
                crate::layout::stringified_from_default_assets(&PathBuf::from(layout_name))?;
            (Some(path_to_layout), stringified_layout, swap_layouts)
        },
        LayoutInfo::Url(url) => {
            should_start_layout_commands_suspended = true;
            (
                Some(url.clone()),
                crate::layout::stringified_from_url(&url)?,
                None,
            )
        },
        LayoutInfo::Stringified(stringified_layout) => (None, stringified_layout, None),
    };
    let mut layout = crate::kdl::layout::from_kdl(
        &raw_layout,
        path_to_raw_layout,
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    );
    if should_start_layout_commands_suspended {
        if let Some(l) = layout.iter_mut().next() {
            l.recursively_add_start_suspended_including_template(Some(true))
        }
    }
    layout
}
pub fn from_layout_info_with_config(
    layout_dir: &Option<PathBuf>,
    layout_info: &LayoutInfo,
    config: Option<Config>,
) -> Result<(Layout, Config), ConfigError> {
    let mut should_start_layout_commands_suspended = false;
    let (path_to_raw_layout, raw_layout, raw_swap_layouts) = match layout_info {
        LayoutInfo::File(layout_name_without_extension, _layout_metadata) => {
            let layout_dir = layout_dir.clone().or_else(default_layout_dir);
            let (path_to_layout, stringified_layout, swap_layouts) =
                crate::layout::stringified_from_dir(
                    &PathBuf::from(layout_name_without_extension),
                    layout_dir.as_ref(),
                )?;
            (Some(path_to_layout), stringified_layout, swap_layouts)
        },
        LayoutInfo::BuiltIn(layout_name) => {
            let (path_to_layout, stringified_layout, swap_layouts) =
                crate::layout::stringified_from_default_assets(&PathBuf::from(layout_name))?;
            (Some(path_to_layout), stringified_layout, swap_layouts)
        },
        LayoutInfo::Url(url) => {
            should_start_layout_commands_suspended = true;
            (
                Some(url.clone()),
                crate::layout::stringified_from_url(url)?,
                None,
            )
        },
        LayoutInfo::Stringified(stringified_layout) => (None, stringified_layout.clone(), None),
    };
    let mut layout = crate::kdl::layout::from_kdl(
        &raw_layout,
        path_to_raw_layout,
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    );
    if should_start_layout_commands_suspended {
        if let Some(l) = layout.iter_mut().next() {
            l.recursively_add_start_suspended_including_template(Some(true))
        }
    }
    let config = Config::from_kdl(&raw_layout, config)?; // this merges the two config, with
    layout.map(|l| (l, config))
}
/// A layout file read to a string: `(layout path, layout text, Some((swap layout path, swap layout text)))`.
pub type StringifiedLayout = (String, String, Option<(String, String)>);

pub fn stringified_from_path_or_default(
    layout_path: Option<&PathBuf>,
    layout_dir: Option<PathBuf>,
) -> Result<StringifiedLayout, ConfigError> {
    match layout_path {
        Some(layout_path) => {
            // The way we determine where to look for the layout is similar to
            // how a path would look for an executable.
            // See the gh issue for more: https://github.com/zellij-org/zellij/issues/1412#issuecomment-1131559720
            if layout_path.extension().is_some() || layout_path.components().count() > 1 {
                // We look localy!
                crate::layout::stringified_from_path(layout_path)
            } else {
                // We look in the default dir
                crate::layout::stringified_from_dir(layout_path, layout_dir.as_ref())
            }
        },
        None => crate::layout::stringified_from_dir(
            &std::path::PathBuf::from("default"),
            layout_dir.as_ref(),
        ),
    }
}
#[cfg(not(target_family = "wasm"))]
pub fn stringified_from_url(url: &str) -> Result<String, ConfigError> {
    Downloader::download_without_cache_blocking(url)
        .map_err(|e| ConfigError::DownloadError(format!("{}", e)))
}
#[cfg(target_family = "wasm")]
pub fn stringified_from_url(_url: &str) -> Result<String, ConfigError> {
    // silently fail - this should not happen in plugins and legacy architecture is hard
    let raw_layout = String::new();
    Ok(raw_layout)
}
pub fn from_path_without_config(layout_path: &Path) -> Result<Layout, ConfigError> {
    // (path_to_layout as String, stringified_layout, Option<path_to_swap_layout as String, stringified_swap_layout>)
    let (path_to_layout, raw_layout, raw_swap_layouts) =
        crate::layout::stringified_from_path(layout_path)?;
    crate::kdl::layout::from_kdl(
        &raw_layout,
        Some(path_to_layout),
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    )
}
pub fn from_path_or_default(
    layout_path: Option<&PathBuf>,
    layout_dir: Option<PathBuf>,
    config: Config,
) -> Result<(Layout, Config), ConfigError> {
    let (path_to_raw_layout, raw_layout, raw_swap_layouts) =
        crate::layout::stringified_from_path_or_default(layout_path, layout_dir)?;
    let layout = crate::kdl::layout::from_kdl(
        &raw_layout,
        Some(path_to_raw_layout),
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    )?;
    let config = Config::from_kdl(&raw_layout, Some(config))?; // this merges the two config, with
    Ok((layout, config))
}
#[cfg(not(target_family = "wasm"))]
pub fn from_url(url: &str, config: Config) -> Result<(Layout, Config), ConfigError> {
    let raw_layout = Downloader::download_without_cache_blocking(url)
        .map_err(|e| ConfigError::DownloadError(format!("{}", e)))?;
    let mut layout = crate::kdl::layout::from_kdl(&raw_layout, Some(url.into()), None, None)?;
    layout.recursively_add_start_suspended_including_template(Some(true));
    let config = Config::from_kdl(&raw_layout, Some(config))?; // this merges the two config, with
    Ok((layout, config))
}
pub fn from_stringified_layout(
    stringified_layout: &str,
    config: Config,
) -> Result<(Layout, Config), ConfigError> {
    let layout = crate::kdl::layout::from_kdl(stringified_layout, None, None, None)?;
    let config = Config::from_kdl(stringified_layout, Some(config))?; // this merges the two config, with
    Ok((layout, config))
}
#[cfg(target_family = "wasm")]
pub fn from_url(_url: &str, _config: Config) -> Result<(Layout, Config), ConfigError> {
    Err(ConfigError::DownloadError(
        "Unsupported platform, cannot download layout from the web".to_owned(),
    ))
}
pub fn from_path_or_default_without_config(
    layout_path: Option<&PathBuf>,
    layout_dir: Option<PathBuf>,
) -> Result<Layout, ConfigError> {
    let (path_to_raw_layout, raw_layout, raw_swap_layouts) =
        crate::layout::stringified_from_path_or_default(layout_path, layout_dir)?;
    let layout = crate::kdl::layout::from_kdl(
        &raw_layout,
        Some(path_to_raw_layout),
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    )?;
    Ok(layout)
}
pub fn from_default_assets(
    layout_name: &Path,
    _layout_dir: Option<PathBuf>,
    config: Config,
) -> Result<(Layout, Config), ConfigError> {
    let (path_to_raw_layout, raw_layout, raw_swap_layouts) =
        crate::layout::stringified_from_default_assets(layout_name)?;
    let layout = crate::kdl::layout::from_kdl(
        &raw_layout,
        Some(path_to_raw_layout),
        raw_swap_layouts
            .as_ref()
            .map(|(r, f)| (r.as_str(), f.as_str())),
        None,
    )?;
    let config = Config::from_kdl(&raw_layout, Some(config))?; // this merges the two config, with
    Ok((layout, config))
}
pub fn default_layout_asset() -> Layout {
    crate::kdl::layout::from_kdl(
        &crate::layout::stringified_default_from_assets().unwrap(),
        None,
        Some((
            "",
            crate::layout::stringified_default_swap_from_assets()
                .unwrap()
                .as_str(),
        )),
        None,
    )
    .unwrap()
}
pub fn from_str(
    raw: &str,
    path_to_raw_layout: String,
    swap_layouts: Option<(&str, &str)>, // Option<path_to_swap_layout, stringified_swap_layout>
    cwd: Option<PathBuf>,
) -> Result<Layout, ConfigError> {
    crate::kdl::layout::from_kdl(raw, Some(path_to_raw_layout), swap_layouts, cwd)
}
pub fn stringified_from_dir(
    layout: &PathBuf,
    layout_dir: Option<&PathBuf>,
) -> Result<StringifiedLayout, ConfigError> {
    match layout_dir {
        Some(dir) => {
            let layout_path = &dir.join(layout);
            if layout_path.with_extension("kdl").exists() {
                crate::layout::stringified_from_path(layout_path)
            } else {
                crate::layout::stringified_from_default_assets(layout)
            }
        },
        None => {
            let home = find_default_config_dir();
            let Some(home) = home else {
                return crate::layout::stringified_from_default_assets(layout);
            };

            let layout_path = &home.join(layout);
            crate::layout::stringified_from_path(layout_path)
        },
    }
}
pub fn stringified_from_path(layout_path: &Path) -> Result<StringifiedLayout, ConfigError> {
    let mut layout_file = File::open(layout_path)
        .or_else(|_| File::open(layout_path.with_extension("kdl")))
        .map_err(|e| ConfigError::IoPath(e, layout_path.into()))?;

    let swap_layout_and_path = crate::layout::swap_layout_and_path(layout_path);

    let mut kdl_layout = String::new();
    layout_file
        .read_to_string(&mut kdl_layout)
        .map_err(|e| ConfigError::IoPath(e, layout_path.into()))?;
    Ok((
        layout_path.as_os_str().to_string_lossy().into(),
        kdl_layout,
        swap_layout_and_path,
    ))
}
pub fn stringified_from_default_assets(path: &Path) -> Result<StringifiedLayout, ConfigError> {
    // TODO: ideally these should not be hard-coded
    // we should load layouts by name from the config
    // and load them from a hashmap or some such
    match path.to_str() {
        Some("default") => Ok((
            "Default layout".into(),
            crate::layout::stringified_default_from_assets()?,
            Some((
                "Default swap layout".into(),
                crate::layout::stringified_default_swap_from_assets()?,
            )),
        )),
        Some("strider") => Ok((
            "Strider layout".into(),
            crate::layout::stringified_strider_from_assets()?,
            Some((
                "Strider swap layout".into(),
                crate::layout::stringified_strider_swap_from_assets()?,
            )),
        )),
        Some("disable-status-bar") => Ok((
            "Disable Status Bar layout".into(),
            crate::layout::stringified_disable_status_from_assets()?,
            None,
        )),
        Some("compact") => Ok((
            "Compact layout".into(),
            crate::layout::stringified_compact_from_assets()?,
            Some((
                "Compact layout swap".into(),
                crate::layout::stringified_compact_swap_from_assets()?,
            )),
        )),
        Some("classic") => Ok((
            "Classic layout".into(),
            crate::layout::stringified_classic_from_assets()?,
            Some((
                "Classiclayout swap".into(),
                crate::layout::stringified_classic_swap_from_assets()?,
            )),
        )),
        Some("welcome") => Ok((
            "Welcome screen layout".into(),
            crate::layout::stringified_welcome_from_assets()?,
            None,
        )),
        None | Some(_) => Err(ConfigError::IoPath(
            std::io::Error::other("The layout was not found"),
            path.into(),
        )),
    }
}
pub fn stringified_default_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(crate::assets::DEFAULT_LAYOUT.to_vec())?)
}
pub fn stringified_default_swap_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(
        crate::assets::DEFAULT_SWAP_LAYOUT.to_vec(),
    )?)
}
pub fn stringified_strider_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(crate::assets::STRIDER_LAYOUT.to_vec())?)
}
pub fn stringified_strider_swap_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(
        crate::assets::STRIDER_SWAP_LAYOUT.to_vec(),
    )?)
}
pub fn stringified_disable_status_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(crate::assets::NO_STATUS_LAYOUT.to_vec())?)
}
pub fn stringified_compact_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(
        crate::assets::COMPACT_BAR_LAYOUT.to_vec(),
    )?)
}
pub fn stringified_compact_swap_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(
        crate::assets::COMPACT_BAR_SWAP_LAYOUT.to_vec(),
    )?)
}
pub fn stringified_classic_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(crate::assets::CLASSIC_LAYOUT.to_vec())?)
}
pub fn stringified_classic_swap_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(
        crate::assets::CLASSIC_SWAP_LAYOUT.to_vec(),
    )?)
}
pub fn stringified_welcome_from_assets() -> Result<String, ConfigError> {
    Ok(String::from_utf8(crate::assets::WELCOME_LAYOUT.to_vec())?)
}

fn swap_layout_and_path(path: &Path) -> Option<(String, String)> {
    // Option<path, stringified_swap_layout>
    let mut swap_layout_path = PathBuf::from(path);
    swap_layout_path.set_extension("swap.kdl");
    match File::open(&swap_layout_path) {
        Ok(mut stringified_swap_layout_file) => {
            let mut swap_kdl_layout = String::new();
            match stringified_swap_layout_file.read_to_string(&mut swap_kdl_layout) {
                Ok(..) => Some((
                    swap_layout_path.as_os_str().to_string_lossy().into(),
                    swap_kdl_layout,
                )),
                Err(_e) => None,
            }
        },
        Err(_e) => None,
    }
}

pub fn metadata_from_path(path: &PathBuf) -> LayoutMetadata {
    match crate::layout::stringified_from_path(path) {
        Ok((path_str, stringified_layout, _swap_layouts)) => {
            match crate::kdl::layout::from_kdl(&stringified_layout, Some(path_str), None, None) {
                Ok(layout) => {
                    let layout_tabs = layout.tabs();
                    let tabs = if layout_tabs.is_empty() {
                        let (tiled_pane_layout, floating_pane_layout) = layout.new_tab();
                        vec![TabMetadata::from(&(
                            None,
                            tiled_pane_layout,
                            floating_pane_layout,
                        ))]
                    } else {
                        layout
                            .tabs()
                            .into_iter()
                            .map(|tab| TabMetadata::from(&tab))
                            .collect()
                    };

                    // Get file metadata for creation and modification times as Unix epochs
                    let (creation_time, update_time) =
                        crate::layout::creation_and_update_times(path);

                    LayoutMetadata {
                        tabs,
                        creation_time,
                        update_time,
                    }
                },
                Err(e) => {
                    log::error!("Failed to parse layout: {}", e);
                    LayoutMetadata::default()
                },
            }
        },
        Err(e) => {
            log::error!("Failed to read layout file: {}", e);
            LayoutMetadata::default()
        },
    }
}

fn creation_and_update_times(path: &PathBuf) -> (String, String) {
    // (creation_time, update_time) returns stringified unix epoch
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let creation_time = metadata
                .created()
                .ok()
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs().to_string())
                })
                .unwrap_or_default();

            let update_time = metadata
                .modified()
                .ok()
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs().to_string())
                })
                .unwrap_or_default();

            (creation_time, update_time)
        },
        Err(_) => (String::new(), String::new()),
    }
}

pub fn layout_info_from_cli(
    layout_dir: &Option<PathBuf>,
    maybe_layout_path: &Option<PathBuf>,
    cwd: PathBuf,
) -> Option<LayoutInfo> {
    // If we're not given a layout path, fall back to "default". Since we cannot tell ahead of
    // time whether the user has a layout named "default.kdl" in their layout directory, we
    // cannot blindly assume that this is indeed the builtin default layout. The layout
    // resolution below will correctly handle this.
    // The docs promise this behavior, so we have to abide:
    // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
    let layout_path = maybe_layout_path
        .clone()
        .unwrap_or(PathBuf::from("default"));

    if layout_path.starts_with("http://") || layout_path.starts_with("https://") {
        Some(LayoutInfo::Url(layout_path.display().to_string()))
    } else if layout_path.extension().is_some() || layout_path.components().count() > 1 {
        let layout_dir = cwd;
        let file_path = layout_dir.join(layout_path);
        Some(LayoutInfo::File(
            // layout_dir.join(layout_path).display().to_string(),
            file_path.display().to_string(),
            crate::layout::metadata_from_path(&file_path),
        ))
    } else {
        // Attempt to interpret the layout as bare layout name from the layout application
        // directory. This is described in the docs:
        // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
        if let Some(layout_dir) = layout_dir.clone().or_else(default_layout_dir) {
            let file_path = layout_dir.join(&layout_path);
            if file_path.exists() {
                return Some(LayoutInfo::File(
                    file_path.display().to_string(),
                    crate::layout::metadata_from_path(&file_path),
                ));
            }
            let file_path_with_ext = file_path.with_extension("kdl");
            if file_path_with_ext.exists() {
                return Some(LayoutInfo::File(
                    file_path_with_ext.display().to_string(),
                    crate::layout::metadata_from_path(&file_path_with_ext),
                ));
            }
        }
        // Assume a builtin layout by default
        Some(LayoutInfo::BuiltIn(layout_path.display().to_string()))
    }
}
pub fn layout_info_from_config(
    layout_dir: &Option<PathBuf>,
    maybe_layout_path: &Option<PathBuf>,
) -> Option<LayoutInfo> {
    // If we're not given a layout path, fall back to "default". Since we cannot tell ahead of
    // time whether the user has a layout named "default.kdl" in their layout directory, we
    // cannot blindly assume that this is indeed the builtin default layout. The layout
    // resolution below will correctly handle this.
    // The docs promise this behavior, so we have to abide:
    // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
    let layout_path = maybe_layout_path
        .clone()
        .unwrap_or(PathBuf::from("default"));

    if layout_path.starts_with("http://") || layout_path.starts_with("https://") {
        Some(LayoutInfo::Url(layout_path.display().to_string()))
    } else if layout_path.extension().is_some() || layout_path.components().count() > 1 {
        let layout_dir = layout_dir.clone().or_else(default_layout_dir)?;
        let file_path = layout_dir.join(layout_path);
        Some(LayoutInfo::File(
            // layout_dir.join(layout_path).display().to_string(),
            file_path.display().to_string(),
            crate::layout::metadata_from_path(&file_path),
        ))
    } else {
        // Attempt to interpret the layout as bare layout name from the layout application
        // directory. This is described in the docs:
        // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
        if let Some(layout_dir) = layout_dir.clone().or_else(default_layout_dir) {
            let file_path = layout_dir.join(&layout_path);
            if file_path.exists() {
                return Some(LayoutInfo::File(
                    file_path.display().to_string(),
                    crate::layout::metadata_from_path(&file_path),
                ));
            }
            let file_path_with_ext = file_path.with_extension("kdl");
            if file_path_with_ext.exists() {
                return Some(LayoutInfo::File(
                    file_path_with_ext.display().to_string(),
                    crate::layout::metadata_from_path(&file_path_with_ext),
                ));
            }
        }
        // Assume a builtin layout by default
        Some(LayoutInfo::BuiltIn(layout_path.display().to_string()))
    }
}
// The unit test location.
#[path = "./unit/layout_test.rs"]
#[cfg(test)]
mod layout_test;
