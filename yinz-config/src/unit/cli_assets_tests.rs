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

/// Carried file themes come back marked as file-sourced, so the replacement keeps them out of any
/// configuration it serializes, and they shadow the configuration's own themes as the directory
/// did. A carried value, even an empty one, means the theme directory is not reread.
#[test]
fn carried_file_themes_restore_without_reading_the_theme_directory() {
    let dir = tempfile::tempdir().unwrap();
    let themes_dir = dir.path().join("themes");
    std::fs::create_dir(&themes_dir).unwrap();
    let mut on_disk = Config::default();
    on_disk.themes.insert("from-dir".into(), theme(false, 9));
    std::fs::write(
        themes_dir.join("mine.kdl"),
        on_disk.to_string(false).unwrap(),
    )
    .unwrap();
    let assets = CliAssets {
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };

    let mut running = Config::default();
    running.themes.insert("inline".into(), theme(false, 3));
    running.themes.insert("from-dir".into(), theme(true, 1));
    let carried = file_themes_to_string(&running);
    let text = running.to_string(false).unwrap();

    let mut replacement = Config::from_kdl(&text, None).unwrap();
    restore_themes(&mut replacement, Some(&carried), &assets).unwrap();
    assert_eq!(replacement.themes, running.themes);
    assert_eq!(replacement.to_string(false).unwrap(), text);

    let mut without_file_themes = Config::default();
    without_file_themes
        .themes
        .insert("inline".into(), theme(false, 3));
    let carried = file_themes_to_string(&without_file_themes);
    assert_eq!(carried, "");
    let mut replacement = Config::from_kdl(&text, None).unwrap();
    restore_themes(&mut replacement, Some(&carried), &assets).unwrap();
    assert_eq!(base(&replacement, "from-dir"), None);
}
