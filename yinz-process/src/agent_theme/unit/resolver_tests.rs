use super::*;

fn context(root: &Path) -> LaunchContext {
    LaunchContext {
        executable: "/usr/bin/true".into(),
        arguments: vec![],
        environment: BTreeMap::from([("HOME".into(), root.as_os_str().to_owned())]),
        cwd: root.to_owned(),
        target: ExecutionTarget::Local,
    }
}
fn snapshot() -> AgentThemeSnapshot {
    AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "fixture".into(),
        provider_version: "fixture".into(),
        theme: String::new(),
        provenance: vec![],
        definitions: vec![],
        collisions: vec![],
        availability: Availability::Available,
    }
}
fn write(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

#[test]
fn non_utf8_environment_survives_resolution_and_terminal_launch() {
    use std::os::unix::ffi::OsStringExt;

    const TEST: &str =
        "agent_theme::tests::non_utf8_environment_survives_resolution_and_terminal_launch";
    const MODE: &str = "YINZ_TEST_RAW_ENV_MODE";
    let key = OsString::from_vec(b"YINZ_TEST_RAW_\xff".to_vec());
    let value = OsString::from_vec(vec![0xff]);
    if let Ok(mode) = std::env::var(MODE) {
        assert_eq!(std::env::var_os(&key), Some(value.clone()));
        assert_eq!(std::env::var_os("YINZ_TEST_RAW_VALUE"), Some(value.clone()));
        if mode == "resolve" {
            assert!(std::io::stdout().is_terminal());
            let context = LaunchContext::current(&std::env::current_exe().unwrap(), &[]).unwrap();
            assert_eq!(context.environment.get(&key), Some(&value));
            let output = context
                .command()
                .args(["--exact", TEST, "--nocapture"])
                .env(MODE, "verify")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            assert_eq!(
                launch("claude", Path::new("/usr/bin/true"), &[]).unwrap(),
                0
            );
        }
        return;
    }
    let pty = nix::pty::openpty(None, None).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(MODE, "resolve")
        .env(&key, &value)
        .env("YINZ_TEST_RAW_VALUE", &value)
        .env("YINZ_PANE_ID", "1")
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_AGENT_THEME_REF")
        .stdout(std::fs::File::from(pty.slave))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn claude_resolves_non_utf8_configuration_paths() {
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(OsString::from_vec(vec![b'c', 0xff]));
    write(&root, ".claude.json", r#"{"theme":"light"}"#);
    let mut launch = context(dir.path());
    launch
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), root.into_os_string());
    let mut source = snapshot();
    claude::resolve(&launch, &mut source).unwrap();
    assert_eq!(source.theme, "light");
}

#[test]
fn documented_color_forms_are_exact_and_invalid_values_are_rejected() {
    for value in ["#12abEF", "rgb(18, 171, 239)"] {
        assert_eq!(color(value), Some(SourceColor::Rgb(18, 171, 239)));
    }
    assert_eq!(color("#a1f"), Some(SourceColor::Rgb(170, 17, 255)));
    assert_eq!(color("ansi256(233)"), Some(SourceColor::Index(233)));
    assert_eq!(color("ansi:cyanBright"), Some(SourceColor::Index(14)));
    for value in [
        "red",
        "#ffff",
        "#💜aa",
        "rgb(999,0,0)",
        "ansi256(256)",
        "ansi:purple",
        "#12abEFjunk",
    ] {
        assert_eq!(color(value), None, "{value}");
    }
}

#[test]
fn claude_inherits_base_and_tracks_unmapped_identity_collisions() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"custom:test"}"#);
    write(
        dir.path(),
        ".claude/themes/test.json",
        r##"{"base":"light","overrides":{"success":"#123456","red_FOR_SUBAGENTS_ONLY":"#123456","inactive":"ansi256(240)","error":"invalid","unknown":"#abcdef"}}"##,
    );
    let mut snapshot = snapshot();
    claude::resolve(&context(dir.path()), &mut snapshot).unwrap();
    snapshot.finish();
    assert_eq!(snapshot.theme, "custom:test");
    assert!(snapshot
        .definitions
        .iter()
        .any(|d| d.token == "inactive" && d.color == SourceColor::Index(240)));
    assert!(snapshot
        .definitions
        .iter()
        .any(|d| d.token == "error" && d.color == SourceColor::Rgb(171, 43, 63)));
    assert!(!snapshot.definitions.iter().any(|d| d.token == "unknown"));
    assert!(snapshot
        .collisions
        .iter()
        .any(|c| c.tokens == ["red_FOR_SUBAGENTS_ONLY", "success"]));
}

#[test]
fn claude_launch_overrides_and_config_roots_are_invocation_local() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"dark"}"#);
    write(dir.path(), "isolated/.claude.json", r#"{"theme":"light"}"#);
    write(
        dir.path(),
        "isolated/settings.json",
        r#"{"theme":"dark-ansi"}"#,
    );
    write(
        dir.path(),
        ".claude/settings.local.json",
        r#"{"theme":"light-ansi"}"#,
    );
    let mut launch = context(dir.path());
    launch.environment.insert(
        "CLAUDE_CONFIG_DIR".into(),
        dir.path().join("isolated").into_os_string(),
    );
    launch.arguments = vec!["--setting-sources".into(), "user".into()];
    launch.target = ExecutionTarget::Remote;
    let mut first = snapshot();
    claude::resolve(&launch, &mut first).unwrap();
    assert_eq!(first.theme, "dark-ansi");
    launch.arguments.extend([
        "--settings".into(),
        r#"{"theme":"light-daltonized"}"#.into(),
    ]);
    launch.target = ExecutionTarget::Sandbox;
    let mut second = snapshot();
    claude::resolve(&launch, &mut second).unwrap();
    assert_eq!(second.theme, "light-daltonized");
    assert_eq!(first.theme, "dark-ansi");
}

#[test]
fn claude_inherited_local_theme_is_unavailable_unless_local_settings_are_disabled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir(dir.path().join("child")).unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"dark"}"#);
    write(
        dir.path(),
        ".claude/settings.local.json",
        r#"{"theme":"light"}"#,
    );
    let mut launch = context(dir.path());
    launch.cwd = dir.path().join("child");
    assert!(claude::resolve(&launch, &mut snapshot())
        .unwrap_err()
        .to_string()
        .contains("inherited repository-local theme"));
    launch.arguments = vec!["--setting-sources=user".into()];
    let mut resolved = snapshot();
    claude::resolve(&launch, &mut resolved).unwrap();
    assert_eq!(resolved.theme, "dark");
    launch
        .environment
        .insert("CLAUDE_CODE_USE_COWORK_PLUGINS".into(), "1".into());
    assert!(claude::resolve(&launch, &mut snapshot()).is_err());
}

#[test]
fn malformed_and_unsupported_themes_fail_without_changing_provider_configuration() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", "{");
    assert!(claude::resolve(&context(dir.path()), &mut snapshot()).is_err());
    let snapshot = resolve("claude", &context(dir.path())).unwrap();
    assert!(matches!(
        snapshot.availability,
        Availability::Unavailable(_)
    ));
    assert!(snapshot.definitions.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".claude.json")).unwrap(),
        "{"
    );
    assert!(resolve("other", &context(dir.path())).is_none());
}

fn fake_codex(root: &Path, response: serde_json::Value) -> LaunchContext {
    let program = root.join("codex");
    let response = serde_json::json!({"id":2,"result":response}).to_string();
    let script = format!(
        r#"if [ "$1" = --version ]; then printf '%s\n' 'codex-cli 0.162.1'; exit; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    *'"id":2'*) printf '%s\n' '{}' ;;
  esac
done"#,
        response.replace('\'', "'\"'\"'")
    );
    yinz_test_support::write_warm_script(&program, &script);
    LaunchContext {
        executable: program,
        ..context(root)
    }
}

#[test]
fn codex_uses_provider_resolved_theme_and_overrides_shared_syntax_colors() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".codex/themes/custom.tmTheme",
        r##"<?xml version="1.0"?><plist version="1.0"><dict><key>settings</key><array>
      <dict><key>settings</key><dict><key>foreground</key><string>#dddddd</string></dict></dict>
      <dict><key>scope</key><string>codex.accent</string><key>settings</key><dict><key>foreground</key><string>#112233</string></dict></dict>
      <dict><key>scope</key><string>markup.inserted</string><key>settings</key><dict><key>foreground</key><string>#112233</string><key>background</key><string>#223344</string></dict></dict>
      <dict><key>scope</key><string>string</string><key>settings</key><dict><key>foreground</key><string>#112233</string></dict></dict>
    </array></dict></plist>"##,
    );
    let launch = fake_codex(
        dir.path(),
        serde_json::json!({"config":{"tui":{"theme":"custom"}},"layers":[]}),
    );
    let source = resolve("codex", &launch).unwrap();
    assert_eq!(source.theme, "custom");
    assert!(
        !matches!(source.availability, Availability::Unavailable(_)),
        "{:?}",
        source.availability
    );
    let (mapping, collisions) = source.compile();
    let pair = MatchKey {
        channel: Channel::Foreground,
        color: SourceColor::Rgb(17, 34, 51),
        pair: Some((SourceColor::Rgb(17, 34, 51), SourceColor::Rgb(34, 51, 68))),
    };
    assert_eq!(mapping[&pair], Some(Role::Success.into()));
    assert_eq!(
        mapping[&MatchKey { pair: None, ..pair }],
        Some(Role::Accent.into())
    );
    assert!(collisions
        .iter()
        .any(|c| c.tokens == ["codex.accent", "markup.inserted", "string"]));
}

#[test]
fn codex_profile_precedence_respects_enabled_project_layers() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".codex/work.config.toml",
        "[tui]\ntheme = 'catppuccin-latte'\n",
    );
    for disabled in [false, true] {
        let mut launch = fake_codex(
            dir.path(),
            serde_json::json!({"config":{"tui":{"theme":"dracula"}},"layers":[{"name":{"type":"project"},"disabledReason": if disabled { Some("untrusted") } else { None },"config":{"tui":{"theme":"dracula"}}}]}),
        );
        launch.arguments = vec!["--profile".into(), "work".into()];
        let source = resolve("codex", &launch).unwrap();
        assert_eq!(
            source.theme,
            if disabled {
                "catppuccin-latte"
            } else {
                "dracula"
            }
        );
        assert!(!source.definitions.is_empty());
    }
}

#[test]
fn codex_builtins_include_model_palettes_and_custom_files_override_them() {
    let dir = tempfile::tempdir().unwrap();
    let launch = fake_codex(
        dir.path(),
        serde_json::json!({"config":{"tui":{"theme":"ada"}},"layers":[]}),
    );
    let first = resolve("codex", &launch).unwrap();
    let accent = first
        .definitions
        .iter()
        .find(|d| d.token == "codex.accent")
        .unwrap();
    assert_eq!(
        first.compile().0[&MatchKey {
            channel: accent.channel,
            color: accent.color,
            pair: None,
        }],
        Some(Role::Accent.into())
    );
    assert!(first
        .definitions
        .iter()
        .any(|d| d.token == "markup.inserted" && d.pair.is_some()));
    assert!(first
        .definitions
        .iter()
        .any(|d| d.token == "diff.add.dark.index"));
    write(
        dir.path(),
        ".codex/themes/ada.tmTheme",
        r##"<?xml version="1.0"?><plist version="1.0"><dict><key>settings</key><array><dict><key>settings</key><dict><key>foreground</key><string>#dddddd</string></dict></dict><dict><key>scope</key><string>codex.accent</string><key>settings</key><dict><key>foreground</key><string>#112233</string></dict></dict></array></dict></plist>"##,
    );
    let second = resolve("codex", &launch).unwrap();
    assert!(second
        .definitions
        .iter()
        .any(|d| d.token == "codex.accent" && d.color == SourceColor::Rgb(17, 34, 51)));
    assert_ne!(first.definitions, second.definitions);
    // The first run remains a value snapshot after files change.
    assert!(!first
        .definitions
        .iter()
        .any(|d| d.token == "codex.accent" && d.color == SourceColor::Rgb(17, 34, 51)));
}

#[test]
fn claude_empty_remote_policy_preserves_user_palette() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"light"}"#);
    let launch = context(dir.path());
    let mut expected = snapshot();
    claude::resolve(&launch, &mut expected).unwrap();

    write(dir.path(), ".claude/remote-settings.json", "{}");
    let mut actual = snapshot();
    claude::resolve(&launch, &mut actual).unwrap();
    assert_eq!(actual.theme, "light");
    assert_eq!(actual.definitions, expected.definitions);
    assert!(!actual.definitions.is_empty());
}

#[test]
fn uncertain_claude_layers_and_codex_automatic_palettes_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".claude/remote-settings.json",
        r#"{"theme":"light"}"#,
    );
    assert!(claude::resolve(&context(dir.path()), &mut snapshot()).is_err());
    let launch = fake_codex(dir.path(), serde_json::json!({"config":{},"layers":[]}));
    let result = resolve("codex", &launch).unwrap();
    assert_eq!(result.theme, "automatic");
    assert!(matches!(result.availability, Availability::Partial(_)));
    assert!(result
        .definitions
        .iter()
        .any(|d| d.token == "status.success"));
}

#[test]
fn codex_launch_options_preserve_clap_compact_values_and_final_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("project")).unwrap();
    for args in [
        vec!["-ctui.theme='ada'", "-pwork", "-Cproject"],
        vec!["-c=tui.theme='ada'", "-p=work", "-C=project"],
        vec!["--config=tui.theme='ada'", "--profile=work", "--cd=project"],
        vec![
            "--config",
            "tui.theme='ada'",
            "--profile",
            "work",
            "--cd",
            "project",
        ],
    ] {
        let launch = LaunchContext {
            arguments: args.into_iter().map(str::to_owned).collect(),
            ..context(dir.path())
        };
        let (cwd, overrides, profile) = codex::launch_options(&launch).unwrap();
        assert_eq!(cwd, dir.path().join("project").canonicalize().unwrap());
        assert_eq!(overrides, ["tui.theme='ada'"]);
        assert_eq!(profile.as_deref(), Some("work"));
    }
}
