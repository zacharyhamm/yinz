use super::*;
#[test]
fn reinstall_replaces_legacy_reports_and_keeps_other_handlers() {
    for events in [CLAUDE_EVENTS, CODEX_EVENTS, MUSE_EVENTS] {
        let mut settings = json!({"hooks": {}, "model": "custom"});
        for event in events {
            settings["hooks"][*event] = json!([{"hooks": [
                {"command": "'/home/me/hocus/hocus-hook.sh'", "type": "command"},
                {"command": "user-handler", "type": "command"}
            ]}]);
        }
        let installed = install_events_into(settings, "yinz-hook.sh", events, 10).unwrap();
        let repeated = install_events_into(installed.clone(), "yinz-hook.sh", events, 10).unwrap();
        assert_eq!(installed, repeated);
        assert_eq!(installed["model"], "custom");
        for event in events {
            let text = installed["hooks"][*event].to_string();
            assert!(!text.contains("hocus-hook"));
            assert!(text.contains("user-handler"));
            assert_eq!(text.matches("yinz-hook.sh").count(), 1);
        }
    }
}
#[test]
fn reinstall_removes_only_the_owned_legacy_opencode_file() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("yinz.js");
    let legacy = dir.path().join("hocus.js");
    std::fs::write(&legacy, "// hocus-opencode-plugin\n").unwrap();
    remove_legacy_opencode(&destination, "yinz-opencode-plugin", true).unwrap();
    assert!(legacy.exists());
    remove_legacy_opencode(&destination, "yinz-opencode-plugin", false).unwrap();
    assert!(!legacy.exists());
    std::fs::write(&legacy, "// user plugin\n").unwrap();
    remove_legacy_opencode(&destination, "yinz-opencode-plugin", false).unwrap();
    assert!(legacy.exists());
}
