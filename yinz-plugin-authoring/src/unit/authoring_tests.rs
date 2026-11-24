use super::*;
#[test]
fn script_names_and_backups_preserve_extensions_and_name_guards() {
    let dir = Path::new("/plugins");
    for extension in ["scm", "ss", "sld"] {
        let name = format!("app/keys.{extension}");
        let path = resolve_plugin_file(dir, &format!(" {name} ")).unwrap();
        assert_eq!(path, dir.join(&name));
        assert!(is_script_plugin(&path));
        let backup = backup_name(&name);
        assert_eq!(backup, format!("app/keys.bak.{extension}"));
        assert_eq!(
            resolve_plugin_file(dir, &backup).unwrap(),
            dir.join(&backup)
        );
        assert!(is_backup_name(&backup));
        assert!(!is_backup_name(&name));
        for invalid in [
            "../keys",
            "app/../keys",
            "/keys",
            "-keys",
            "app/-keys",
            "a/b/keys",
            "",
            ".",
            "..",
            "app/",
        ] {
            assert!(
                resolve_plugin_file(dir, &format!("{invalid}.{extension}")).is_err(),
                "{invalid}.{extension}"
            );
        }
        assert!(resolve_plugin_file(
            dir,
            &format!("{}.{extension}", "a".repeat(MAX_PLUGIN_NAME_LEN + 1))
        )
        .is_err());
    }
    assert_eq!(
        resolve_plugin_file(dir, "keys").unwrap(),
        dir.join("keys.scm")
    );
    assert_eq!(
        resolve_plugin_file(dir, "keys.jsx").unwrap(),
        dir.join("keys.jsx.scm")
    );
}

#[test]
fn a_name_is_a_file_or_one_folder_deep_and_nothing_else() {
    for name in ["keys.lua", " app/keys.lua "] {
        assert!(resolve_plugin_file(Path::new("/plugins"), name)
            .unwrap_err()
            .contains("Lua plugins are no longer supported"));
    }
    let dir = Path::new("/home/someone/.config/yinz/plugins");
    assert_eq!(
        resolve_plugin_file(dir, "keys").unwrap(),
        dir.join("keys.scm")
    );
    assert_eq!(
        resolve_plugin_file(dir, " keys.scm ").unwrap(),
        dir.join("keys.scm")
    );
    assert_eq!(
        resolve_plugin_file(dir, "app/ui").unwrap(),
        dir.join("app/ui.scm")
    );
    for bad in [
        "",
        "../x",
        "a/../b",
        "a/b/c",
        "-x",
        "a b",
        "/etc/passwd",
        "x\0",
    ] {
        assert!(resolve_plugin_file(dir, bad).is_err(), "{bad:?}");
    }
    assert!(resolve_plugin_file(dir, &"a".repeat(MAX_PLUGIN_NAME_LEN + 1)).is_err());
    assert!(resolve_plugin_file(Path::new("plugins"), "keys").is_err());
}

#[test]
fn a_backup_is_a_plugin_name_that_a_listing_leaves_out() {
    assert_eq!(backup_name("keys"), "keys.bak");
    assert_eq!(backup_name(" keys.scm "), "keys.bak.scm");
    assert_eq!(backup_name("app/ui"), "app/ui.bak");
    let dir = Path::new("/home/someone/.config/yinz/plugins");
    assert_eq!(
        resolve_plugin_file(dir, &backup_name("keys")).unwrap(),
        dir.join("keys.bak.scm")
    );
    assert!(is_backup_name("keys.bak.scm"));
    assert!(is_backup_name("app/ui.bak"));
    assert!(!is_backup_name("keys.scm"));
}
