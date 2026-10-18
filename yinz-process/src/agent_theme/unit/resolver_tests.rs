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
    snapshot.provider = "claude".into();
    snapshot.finish();
    assert_eq!(
        snapshot.compile().0[&MatchKey {
            channel: Channel::Foreground,
            color: SourceColor::Rgb(18, 52, 86),
            pair: None
        }],
        Some(Role::Success.into())
    );
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
fn claude_auto_keeps_both_palettes_and_resolves_collisions_deterministically() {
    for settings in [".claude.json", ".claude/settings.json"] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), settings, r#"{"theme":"auto"}"#);
        let mut source = snapshot();
        source.provider = "claude".into();
        claude::resolve(&context(dir.path()), &mut source).unwrap();
        source.finish();
        assert_eq!(source.theme, "auto");
        assert_eq!(source.provenance, ["claude-2.1.296/palettes-v2"]);
        assert!(matches!(source.availability, Availability::Partial(_)));
        let compiled = source.compile();
        assert_eq!(
            source
                .definitions
                .iter()
                .filter(|d| d.token == "permission")
                .map(|d| (d.color, d.target))
                .collect::<Vec<_>>(),
            [
                (SourceColor::Rgb(87, 105, 247), Some(Role::Accent.into())),
                (SourceColor::Rgb(177, 185, 249), Some(Role::Accent.into())),
            ]
        );
        for (channel, color, role) in [
            (
                Channel::Foreground,
                SourceColor::Rgb(87, 105, 247),
                Role::Accent,
            ),
            (
                Channel::Foreground,
                SourceColor::Rgb(177, 185, 249),
                Role::Accent,
            ),
            (Channel::Foreground, SourceColor::Default, Role::Foreground),
            (Channel::Background, SourceColor::Default, Role::Background),
            (
                Channel::Foreground,
                SourceColor::Rgb(0, 0, 0),
                Role::Foreground,
            ),
            (
                Channel::Foreground,
                SourceColor::Rgb(255, 255, 255),
                Role::Foreground,
            ),
        ] {
            let key = MatchKey {
                channel,
                color,
                pair: None,
            };
            assert_eq!(compiled.0[&key], Some(role.into()));
        }
        for color in [SourceColor::Rgb(0, 0, 0), SourceColor::Rgb(255, 255, 255)] {
            assert!(source.collisions.iter().any(|collision| {
                collision.key.channel == Channel::Foreground
                    && collision.key.color == color
                    && collision.tokens.iter().any(|t| t == "inverseText")
                    && collision.tokens.iter().any(|t| t == "text")
            }));
        }
        source.definitions.reverse();
        assert_eq!(source.compile(), compiled);
    }
}

#[test]
fn claude_auto_is_not_a_custom_theme_base() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"custom:test"}"#);
    write(dir.path(), ".claude/themes/test.json", r#"{"base":"auto"}"#);
    assert!(claude::resolve(&context(dir.path()), &mut snapshot()).is_err());
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

#[test]
fn claude_resolution_is_gated_on_provider_identity_not_release() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".claude.json", r#"{"theme":"light"}"#);
    let program = dir.path().join("claude");
    let resolve_version = |output: &str| {
        yinz_test_support::write_warm_script(
            &program,
            &format!("if [ \"$1\" = --version ]; then printf '%s\\n' '{output}'; fi\n"),
        );
        resolve(
            "claude",
            &LaunchContext {
                executable: program.clone(),
                ..context(dir.path())
            },
        )
        .unwrap()
    };
    let verified = resolve_version("2.1.296 (Claude Code)");
    let Availability::Partial(detail) = &verified.availability else {
        panic!("{:?}", verified.availability);
    };
    assert!(!detail.contains("descriptor verified against"), "{detail}");
    let newer = resolve_version("2.1.299 (Claude Code)");
    assert_eq!(newer.provider_version, "2.1.299");
    assert_eq!(newer.definitions, verified.definitions);
    let Availability::Partial(detail) = &newer.availability else {
        panic!("{:?}", newer.availability);
    };
    assert!(
        detail.ends_with("descriptor verified against 2.1.296, running 2.1.299"),
        "{detail}"
    );
    // Wrappers may print notices first; the version comes from the identifying line.
    let noisy = resolve_version("Node 18 is deprecated\nv2.1.296 (Claude Code)");
    assert_eq!(noisy.provider_version, "2.1.296");
    assert_eq!(noisy.definitions, verified.definitions);
    // Notices may be long and not valid UTF-8; neither hides the identifying line.
    yinz_test_support::write_warm_script(
        &program,
        "if [ \"$1\" = --version ]; then printf 'caf\\303\\n'; i=0; \
         while [ $i -lt 100 ]; do printf '%080d' 0; i=$((i + 1)); done; \
         printf '\\nv2.1.296 (Claude Code)\\n'; fi\n",
    );
    let verbose = resolve(
        "claude",
        &LaunchContext {
            executable: program.clone(),
            ..context(dir.path())
        },
    )
    .unwrap();
    assert_eq!(verbose.provider_version, "2.1.296");
    assert_eq!(verbose.definitions, verified.definitions);
    for unidentified in [
        "2.1.299 (Something Else)",
        "dev (Claude Code)",
        "Update available (Claude Code)",
    ] {
        let other = resolve_version(unidentified);
        assert!(
            matches!(other.availability, Availability::Unavailable(_)),
            "{unidentified}"
        );
        assert!(other.definitions.is_empty());
    }
    write(dir.path(), ".claude.json", r#"{"theme":1}"#);
    let unreadable = resolve_version("2.1.299 (Claude Code)");
    let Availability::Unavailable(detail) = &unreadable.availability else {
        panic!("{:?}", unreadable.availability);
    };
    assert!(
        detail.ends_with("descriptor verified against 2.1.296, running 2.1.299"),
        "{detail}"
    );
}

#[test]
fn codex_resolution_is_gated_on_provider_identity_not_release() {
    let dir = tempfile::tempdir().unwrap();
    let automatic = serde_json::json!({"config":{},"layers":[]});
    let verified = resolve("codex", &fake_codex(dir.path(), automatic.clone())).unwrap();
    let newer = resolve(
        "codex",
        &fake_codex_release(dir.path(), "codex-cli 0.170.0", automatic.clone()),
    )
    .unwrap();
    assert_eq!(newer.provider_version, "0.170.0");
    assert_eq!(newer.definitions, verified.definitions);
    let Availability::Partial(detail) = &newer.availability else {
        panic!("{:?}", newer.availability);
    };
    assert!(
        detail.ends_with("descriptor verified against 0.162.1, running 0.170.0"),
        "{detail}"
    );
    // An update notice names a release too, but it isn't the version line.
    let notified = resolve(
        "codex",
        &fake_codex_release(
            dir.path(),
            "codex-cli 0.171.0 is available (you have 0.170.0)\ncodex-cli 0.170.0",
            automatic.clone(),
        ),
    )
    .unwrap();
    assert_eq!(notified.provider_version, "0.170.0");
    let unversioned = resolve(
        "codex",
        &fake_codex_release(dir.path(), "codex-cli dev", automatic.clone()),
    )
    .unwrap();
    assert!(matches!(
        unversioned.availability,
        Availability::Unavailable(_)
    ));
    assert!(unversioned.definitions.is_empty());
    write(dir.path(), ".codex/themes/broken.tmTheme", "<plist");
    let unreadable = resolve(
        "codex",
        &fake_codex_release(
            dir.path(),
            "codex-cli 0.170.0",
            serde_json::json!({"config":{"tui":{"theme":"broken"}},"layers":[]}),
        ),
    )
    .unwrap();
    let Availability::Unavailable(detail) = &unreadable.availability else {
        panic!("{:?}", unreadable.availability);
    };
    assert!(unreadable.definitions.is_empty());
    assert!(
        detail.ends_with("descriptor verified against 0.162.1, running 0.170.0"),
        "{detail}"
    );
}

fn fake_codex(root: &Path, response: serde_json::Value) -> LaunchContext {
    fake_codex_release(root, "codex-cli 0.162.1", response)
}

fn fake_codex_release(root: &Path, version: &str, response: serde_json::Value) -> LaunchContext {
    let program = root.join("codex");
    let response = serde_json::json!({"id":2,"result":response}).to_string();
    let script = format!(
        r#"if [ "$1" = --version ]; then printf '%s\n' '{version}'; exit; fi
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

#[test]
fn claude_presets_and_custom_overrides_map_ui_and_both_diff_channels() {
    let presets: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_str(include_str!("../descriptors/claude-2.1.296.json")).unwrap();
    assert_eq!(presets.len(), 6);
    let dir = tempfile::tempdir().unwrap();
    for preset in presets.keys() {
        for custom in [false, true] {
            let theme = if custom { "custom:test" } else { preset };
            write(
                dir.path(),
                ".claude.json",
                &serde_json::json!({"theme": theme}).to_string(),
            );
            write(
                dir.path(),
                ".claude/themes/test.json",
                &serde_json::json!({
                    "base": preset,
                    "overrides": {
                        "diffAddedWord": "ansi:greenBright",
                        "diffRemovedWord": "ansi:redBright",
                        "rate_limit_empty": "rgb(12,34,56)"
                    }
                })
                .to_string(),
            );
            let mut source = snapshot();
            source.provider = "claude".into();
            claude::resolve(&context(dir.path()), &mut source).unwrap();
            let result = source.compile();
            for (token, channel, role) in [
                ("briefLabelYou", Channel::Foreground, Role::Accent),
                ("briefLabelClaude", Channel::Foreground, Role::Accent),
                ("clawd_body", Channel::Foreground, Role::Accent),
                ("professionalBlue", Channel::Foreground, Role::Accent),
                ("chromeYellow", Channel::Foreground, Role::Accent),
                (
                    "claudeBlue_FOR_SYSTEM_SPINNER",
                    Channel::Foreground,
                    Role::Accent,
                ),
                ("rate_limit_fill", Channel::Foreground, Role::Accent),
                ("rate_limit_empty", Channel::Foreground, Role::Subtle),
                ("rate_limit_empty", Channel::Background, Role::Subtle),
                ("diffAddedWord", Channel::Foreground, Role::Success),
                ("diffRemovedWord", Channel::Foreground, Role::Error),
                ("diffAddedWord", Channel::Background, Role::AdditionWord),
                ("diffRemovedWord", Channel::Background, Role::RemovalWord),
                ("diffAdded", Channel::Background, Role::AdditionBackground),
                ("diffRemoved", Channel::Background, Role::RemovalBackground),
                ("diffAddedDimmed", Channel::Background, Role::AdditionDimmed),
                (
                    "diffRemovedDimmed",
                    Channel::Background,
                    Role::RemovalDimmed,
                ),
            ] {
                let d = source
                    .definitions
                    .iter()
                    .find(|d| d.token == token && d.channel == channel)
                    .unwrap();
                assert_eq!(d.target, Some(role.into()), "{preset}: {token}");
                assert!(
                    result.0[&MatchKey {
                        channel,
                        color: d.color,
                        pair: None
                    }]
                        .is_some(),
                    "{preset}: {token}"
                );
                if custom && token == "diffAddedWord" {
                    assert_eq!(d.color, SourceColor::Index(10));
                }
                if custom && token == "diffRemovedWord" {
                    assert_eq!(d.color, SourceColor::Index(9));
                }
                if token == "rate_limit_empty" {
                    if custom {
                        assert_eq!(d.color, SourceColor::Rgb(12, 34, 56));
                    }
                    if channel == Channel::Background {
                        assert_eq!(
                            result.0[&MatchKey {
                                channel,
                                color: d.color,
                                pair: None
                            }],
                            Some(Role::Subtle.into()),
                            "{preset}: {token}"
                        );
                    }
                }
            }
            let shimmer = source
                .definitions
                .iter()
                .find(|d| d.token == "claudeBlueShimmer_FOR_SYSTEM_SPINNER")
                .unwrap();
            assert_eq!(
                shimmer.target,
                Some(Target {
                    role: Role::Accent,
                    shimmer: true
                })
            );
            assert!(source
                .definitions
                .iter()
                .filter(|d| d.token.contains("SUBAGENTS_ONLY") || d.token.starts_with("rainbow_"))
                .all(|d| d.target.is_none()));
            source.definitions.reverse();
            assert_eq!(source.compile(), result);
        }
    }
}

#[path = "claude_code_tests.rs"]
mod claude_code;

#[test]
fn codex_footer_resolves_automatic_bundled_and_custom_scopes() {
    let dir = tempfile::tempdir().unwrap();
    // Expected softened colors also come from the pinned provider fixture generator.
    for (name, variant, model, path) in [
        (None, "dark", (246, 226, 183), (171, 223, 167)),
        (None, "light", (213, 144, 48), (72, 154, 54)),
        (
            Some("catppuccin-mocha"),
            "selected",
            (246, 226, 183),
            (171, 223, 167),
        ),
        (
            Some("catppuccin-latte"),
            "selected",
            (213, 144, 48),
            (72, 154, 54),
        ),
        (Some("custom"), "selected", (236, 210, 168), (193, 227, 185)),
    ] {
        write(
            dir.path(),
            ".codex/themes/custom.tmTheme",
            r##"<?xml version="1.0"?><plist version="1.0"><dict><key>settings</key><array>
          <dict><key>settings</key><dict><key>foreground</key><string>#dddddd</string></dict></dict>
          <dict><key>scope</key><string>entity.name</string><key>settings</key><dict><key>foreground</key><string>#f0d2a0</string></dict></dict>
          <dict><key>scope</key><string>support.type</string><key>settings</key><dict><key>foreground</key><string>#ff0000</string></dict></dict>
          <dict><key>scope</key><string>markup.underline.link</string><key>settings</key><dict><key>foreground</key><string>#bee6b4</string></dict></dict>
        </array></dict></plist>"##,
        );
        let launch = fake_codex(
            dir.path(),
            serde_json::json!({"config":{"tui":{"theme":name}},"layers":[]}),
        );
        let snapshot = resolve("codex", &launch).unwrap();
        for (item, (r, g, b)) in [("model", model), ("path", path)] {
            let definition = snapshot
                .definitions
                .iter()
                .find(|d| d.token == format!("codex.footer.{variant}.{item}"))
                .unwrap();
            assert_eq!(
                definition.color,
                SourceColor::Rgb(r, g, b),
                "{name:?}/{item}"
            );
            assert_eq!(definition.target, None);
        }
    }
}

#[test]
fn codex_footer_scope_fallback_skips_default_and_preserves_indexed_colors() {
    use syntect::highlighting::{Color, StyleModifier, Theme, ThemeItem};
    let mut theme = Theme::default();
    let mut definitions = vec![];
    codex::footer_definitions(&theme, "selected", &mut definitions);
    assert_eq!(definitions[0].color, SourceColor::Index(6));
    assert_eq!(definitions[1].color, SourceColor::Index(2));
    for (scope, color) in [
        (
            "entity.name.type",
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 1,
            },
        ),
        (
            "support.type",
            Color {
                r: 14,
                g: 0,
                b: 0,
                a: 0,
            },
        ),
        (
            "variable",
            Color {
                r: 200,
                g: 0,
                b: 0,
                a: 0,
            },
        ),
        (
            "string",
            Color {
                r: 151,
                g: 0,
                b: 0,
                a: 0,
            },
        ),
    ] {
        theme.scopes.push(ThemeItem {
            scope: scope.parse().unwrap(),
            style: StyleModifier {
                foreground: Some(color),
                ..StyleModifier::default()
            },
        });
    }
    definitions.clear();
    codex::footer_definitions(&theme, "selected", &mut definitions);
    assert_eq!(definitions[0].color, SourceColor::Index(14));
    assert_eq!(definitions[1].color, SourceColor::Index(151));
}
