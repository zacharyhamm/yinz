//! Some general utility functions.

pub use zellij_runtime::shared::set_permissions;

pub use zellij_types::text::{adjust_to_size, ansi_len, clean_string_from_control_and_linebreak};

pub use zellij_runtime::shared::make_terminal_title;

pub use zellij_types::colors::{default_colors as colors, eightbit_to_rgb, parse_hex_rgb};

pub use zellij_types::style::{default_palette, detect_theme_hue};

// (this was shamelessly copied from alacritty)
//
// This returns the current terminal version as a unique number based on the
// semver version. The different versions are padded to ensure that a higher semver version will
// always report a higher version number.
pub fn version_number(mut version: &str) -> usize {
    if let Some(separator) = version.rfind('-') {
        version = &version[..separator];
    }

    let mut version_number = 0;

    let semver_versions = version.split('.');
    for (i, semver_version) in semver_versions.rev().enumerate() {
        let semver_number = semver_version.parse::<usize>().unwrap_or(0);
        version_number += usize::pow(100, i as u32) * semver_number;
    }

    version_number
}

pub use zellij_runtime::shared::{parse_base_url, ServerAddress};
