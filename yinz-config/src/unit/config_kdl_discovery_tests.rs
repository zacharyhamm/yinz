use super::*;
#[test]
fn discovery_ignores_lua_and_reloads_kdl_and_overlays() {
    let dir = tempfile::tempdir().unwrap();
    let lua = dir.path().join("config.lua");
    std::fs::write(&lua, "error('must never execute')").unwrap();
    let defaults = Config::from_default_assets().unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false).unwrap(),
        defaults
    );
    let kdl = dir.path().join("config.kdl");
    assert_eq!(
        Config::config_file_path(None, Some(dir.path())),
        Some(kdl.clone())
    );
    std::fs::write(&kdl, "mouse_mode false\n").unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .mouse_mode,
        Some(false)
    );
    let overlays = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
    std::fs::create_dir(&overlays).unwrap();
    std::fs::write(overlays.join("10-override.kdl"), "mouse_mode true\n").unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .mouse_mode,
        Some(true)
    );
    std::fs::write(overlays.join("10-override.kdl"), "mouse_mode false\n").unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .mouse_mode,
        Some(false)
    );
    assert!(Config::load(Some(&lua), Some(dir.path()), false)
        .unwrap_err()
        .to_string()
        .contains("Lua configuration is no longer supported"));
    assert_eq!(
        std::fs::read_to_string(&lua).unwrap(),
        "error('must never execute')"
    );
}
