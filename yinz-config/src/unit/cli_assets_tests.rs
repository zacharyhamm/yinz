use super::*;
use crate::theme::Theme;

fn theme(sourced_from_external_file: bool, base: u8) -> Theme {
    let mut theme = Theme {
        palette: Default::default(),
        terminal: None,
        sourced_from_external_file,
    };
    theme.palette.text_unselected.base = yinz_types::data::PaletteColor::EightBit(base);
    theme
}

fn base(config: &Config, name: &str) -> Option<yinz_types::data::PaletteColor> {
    config
        .themes
        .get_theme(name)
        .map(|theme| theme.palette.text_unselected.base)
}

/// An in-place update hands the replacement server its configuration as KDL, which leaves out
/// file-sourced themes; re-attaching must restore them beneath nothing the config itself defines.
#[test]
fn reattaching_themes_restores_file_themes_lost_by_serialization() {
    let dir = tempfile::tempdir().unwrap();
    let themes_dir = dir.path().join("themes");
    std::fs::create_dir(&themes_dir).unwrap();
    let mut file_themes = Config::default();
    file_themes
        .themes
        .insert("from-dir".into(), theme(false, 1));
    file_themes
        .themes
        .insert("shadowed".into(), theme(false, 2));
    std::fs::write(
        themes_dir.join("mine.kdl"),
        file_themes.to_string(false).unwrap(),
    )
    .unwrap();

    let mut running = Config::default();
    running.themes.insert("inline".into(), theme(false, 3));
    running.themes.insert("shadowed".into(), theme(false, 4));
    let assets = CliAssets {
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };
    attach_themes(&mut running, &assets);
    assert_eq!(
        base(&running, "from-dir"),
        Some(theme(true, 1).palette.text_unselected.base)
    );
    assert_eq!(
        base(&running, "shadowed"),
        Some(theme(true, 2).palette.text_unselected.base)
    );

    let mut replacement = Config::from_kdl(&running.to_string(false).unwrap(), None).unwrap();
    assert_eq!(
        base(&replacement, "from-dir"),
        None,
        "serialization drops file themes"
    );
    attach_themes(&mut replacement, &assets);
    assert_eq!(replacement.themes, running.themes);
    assert_eq!(
        base(&replacement, "inline"),
        Some(theme(false, 3).palette.text_unselected.base)
    );
}
