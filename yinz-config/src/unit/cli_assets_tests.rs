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

#[test]
fn checked_native_first_client_uses_resolved_layout() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let layouts = dir.path().join("layouts");
    fs::create_dir(&layouts).unwrap();
    fs::write(
        layouts.join("default"),
        "layout { tab name=\"Different\" { pane; }; }",
    )
    .unwrap();
    let mut assets = CliAssets {
        config_dir: Some(dir.path().into()),
        checked_native_layout: true,
        layout: Some(LayoutInfo::BuiltIn("default".into())),
        ..Default::default()
    };
    assert_eq!(
        load_config_and_layout(&assets).unwrap().1,
        crate::layout::default_layout_asset()
    );
    assets.layout = Some(LayoutInfo::Stringified(
        "layout { tab name=\"Inline\" { pane; }; }\nterminal { underline_thickness 4; }".into(),
    ));
    let (config, layout) = load_config_and_layout(&assets).unwrap();
    assert_eq!(layout.tabs()[0].0.as_deref(), Some("Inline"));
    assert_eq!(config.terminal.underline_thickness, Some(4));

    // A user layout selected by the checked resolver may have the same
    // basename as a checkpoint without being part of one.
    let name = yinz_runtime::session_checkpoint::LAYOUT;
    let primary = dir.path().join(name);
    fs::write(&primary, "layout { tab name=\"User layout\" { pane; }; }").unwrap();
    fs::write(dir.path().join("current"), "").unwrap();
    let requested = primary.with_extension("missing");
    let (identity, _) = crate::config::load_checked_layout_and_config(
        None,
        Config::default(),
        None,
        None,
        None,
        Some(&requested),
        dir.path().into(),
    )
    .unwrap();
    assets.layout = identity;
    let (_, layout) = load_config_and_layout(&assets).unwrap();
    assert_eq!(layout.tabs()[0].0.as_deref(), Some("User layout"));
}

#[cfg(unix)]
#[test]
fn checked_native_first_client_refuses_replaced_layout() {
    use std::{fs, os::unix::fs::symlink};

    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("selected");
    let fallback = primary.with_extension("kdl");
    for path in [&primary, &fallback] {
        fs::write(path, "layout { pane; }").unwrap();
    }
    let (layout, _) = crate::config::load_checked_layout_and_config(
        None,
        Config::default(),
        None,
        None,
        None,
        Some(&primary),
        dir.path().into(),
    )
    .unwrap();
    let mut assets = CliAssets {
        layout,
        checked_native_layout: true,
        ..Default::default()
    };
    fs::remove_file(&primary).unwrap();
    let error = load_config_and_layout(&assets).unwrap_err().to_string();
    assert!(error.contains(primary.to_str().unwrap()), "{error}");
    // The ordinary loader still permits the extension fallback.
    assets.checked_native_layout = false;
    assert!(load_config_and_layout(&assets).is_ok());

    assets.checked_native_layout = true;
    let pipe = dir.path().join("pipe");
    let pipe_name = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(pipe_name.as_ptr(), 0o600) }, 0);
    symlink(&pipe, &primary).unwrap();
    let assert_refusal = |path: &std::path::Path| {
        let error = load_config_and_layout(&assets).unwrap_err().to_string();
        assert!(error.contains(path.to_str().unwrap()), "{error}");
        assert!(error.contains("not a regular file"), "{error}");
    };
    assert_refusal(&primary);
    fs::remove_file(&primary).unwrap();
    fs::write(&primary, "layout { pane contents_file=\"preview\"; }").unwrap();
    let swap = primary.with_extension("swap.kdl");
    symlink(&pipe, &swap).unwrap();
    assert_refusal(&swap);
    fs::remove_file(&swap).unwrap();
    let preview = dir.path().join("preview");
    symlink(&pipe, &preview).unwrap();
    assert_refusal(&preview);
    fs::remove_file(&preview).unwrap();
    fs::write(&preview, "beside the symlink").unwrap();
    let elsewhere = dir.path().join("real");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("preview"), "beside the target").unwrap();
    fs::write(
        elsewhere.join("source.kdl"),
        "layout { tab name=\"Symlink\" { pane contents_file=\"preview\"; }; }",
    )
    .unwrap();
    fs::remove_file(&primary).unwrap();
    symlink(elsewhere.join("source.kdl"), &primary).unwrap();
    let (_, layout) = load_config_and_layout(&assets).unwrap();
    let tab = &layout.tabs()[0];
    assert_eq!(tab.0.as_deref(), Some("Symlink"));
    assert!(format!("{:?}", tab.1).contains("beside the symlink"));
}
