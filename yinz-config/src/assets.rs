#[cfg(not(test))]
use crate::consts::YINZ_DEFAULT_THEMES;
use crate::theme::Themes;
#[cfg(not(test))]
pub fn get_default_themes() -> Themes {
    // Embedded assets are immutable; repeated config loads can reuse the parsed themes.
    static THEMES: std::sync::LazyLock<Themes> = std::sync::LazyLock::new(|| {
        let mut themes = Themes::default();
        for file in YINZ_DEFAULT_THEMES.files() {
            if let Some(content) = file.contents_utf8() {
                let sourced_from_external_file = true;
                if let Ok(theme) = Themes::from_string(content, sourced_from_external_file) {
                    themes = themes.merge(theme)
                }
            }
        }
        themes
    });
    THEMES.clone()
}

#[cfg(test)]
pub fn get_default_themes() -> Themes {
    Themes::default()
}

pub const DEFAULT_CONFIG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/config/default.kdl"
));

pub const DEFAULT_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/default.kdl"
));

pub const DEFAULT_SWAP_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/default.swap.kdl"
));

pub const STRIDER_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/strider.kdl"
));

pub const STRIDER_SWAP_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/strider.swap.kdl"
));

pub const NO_STATUS_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/disable-status-bar.kdl"
));

pub const COMPACT_BAR_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/compact.kdl"
));

pub const COMPACT_BAR_SWAP_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/compact.swap.kdl"
));

pub const CLASSIC_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/classic.kdl"
));

pub const CLASSIC_SWAP_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/classic.swap.kdl"
));

pub const WELCOME_LAYOUT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    "assets/layouts/welcome.kdl"
));

pub fn add_layout_ext(s: &str) -> String {
    match s {
        c if s.ends_with(".kdl") => c.to_owned(),
        _ => {
            let mut s = s.to_owned();
            s.push_str(".kdl");
            s
        },
    }
}
