use super::*;
use kdl::KdlDocument;

fn doc(text: &str) -> KdlDocument {
    text.parse().unwrap()
}

/// A block replaces its namesake, except that blocks with children merge their children
/// by name - so one option, or one hook point, can be set without touching the rest.
#[test]
fn a_fragment_merges_by_block_and_then_by_child_name() {
    let mut managed =
        doc("mouse_mode true\ncopy_command \"x\"\nhooks {\n    agent_exit \"file:a.scm\"\n}\n");
    merge_kdl_fragment(
            &mut managed,
            &doc("mouse_mode false\nhooks {\n    context_menu \"\"\n}\nload_plugins {\n    \"file:x.scm\"\n}\n"),
        );
    let text = managed.to_string();
    assert!(text.contains("mouse_mode false"), "{text}");
    assert!(!text.contains("mouse_mode true"), "{text}");
    assert!(
        text.contains("copy_command \"x\""),
        "an untouched option stays: {text}"
    );
    assert!(text.contains("agent_exit \"file:a.scm\""), "{text}");
    assert!(text.contains("context_menu \"\""), "{text}");
    assert!(
        text.contains("load_plugins"),
        "a new block is appended: {text}"
    );
}

fn loaded(managed: &KdlDocument) -> Config {
    let defaults = Config::from_default_assets().unwrap();
    Config::from_kdl(&managed.to_string(), Some(defaults)).unwrap()
}

/// A hook point is keyed by its name: writing `agent_exit ""` over `agent_exit "file:a.scm"`
/// replaces the binding rather than leaving both for the loader to reconcile.
#[test]
fn a_child_with_new_arguments_replaces_its_namesake() {
    let mut managed = doc("hooks {\n    agent_exit \"file:a.scm\"\n}\n");
    merge_kdl_fragment(&mut managed, &doc("hooks {\n    agent_exit \"\"\n}\n"));
    let text = managed.to_string();
    assert_eq!(text.matches("agent_exit").count(), 1, "{text}");
    assert!(
        loaded(&managed)
            .hooks
            .get(yinz_types::input::hooks::HookPoint::AgentExit)
            .is_none(),
        "{text}"
    );
}

/// Under `keybinds` the positional arguments are the block's identity: `shared_except`
/// blocks with different mode lists coexist, and only one with the same list replaces it.
/// A property is not identity: `normal clear-defaults=true` replaces `normal`.
#[test]
fn keybinds_blocks_are_keyed_by_name_and_arguments() {
    let mut managed =
        doc("keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n");
    merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    normal clear-defaults=true {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
    let text = managed.to_string();
    assert_eq!(text.matches("normal").count(), 1, "{text}");
    assert!(!text.contains("Ctrl y"), "{text}");

    let mut managed = doc(
            "keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n",
        );
    merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    shared_except \"normal\" \"locked\" {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
    merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl i\" { NewPane; }\n    }\n}\n"),
        );
    let text = managed.to_string();
    assert_eq!(text.matches("shared_except").count(), 2, "{text}");
    assert!(!text.contains("Ctrl y"), "{text}");
    assert!(text.contains("Ctrl u") && text.contains("Ctrl i"), "{text}");
}

/// The loaders read children in order, so a replaced block moves to the end and the most
/// recent write wins: restating a mode block after a `clear-defaults=true` one must not
/// slide it back in front of the clear.
#[test]
fn a_replaced_child_moves_to_the_end() {
    let bind_y = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
    let mut managed = doc(bind_y);
    merge_kdl_fragment(
            &mut managed,
            &doc("keybinds {\n    normal clear-defaults=true {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
    merge_kdl_fragment(&mut managed, &doc(bind_y));
    let config = loaded(&managed);
    let normal = config
        .keybinds
        .0
        .get(&yinz_types::data::InputMode::Normal)
        .unwrap();
    let keys: Vec<String> = normal.keys().map(|key| key.to_string()).collect();
    assert!(
        keys.iter().any(|key| key == "Ctrl y"),
        "{keys:?}\n{managed}"
    );
}

/// A `clear-defaults=true` block is a complete keymap: nothing from the earlier block
/// survives under it.
#[test]
fn a_clear_defaults_block_replaces_its_namesake_whole() {
    let mut managed = doc(
            "keybinds {\n    shared_except \"locked\" {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n",
        );
    merge_kdl_fragment(
            &mut managed,
            &doc("keybinds clear-defaults=true {\n    normal {\n        bind \"Ctrl u\" { NewPane; }\n    }\n}\n"),
        );
    let text = managed.to_string();
    assert!(!text.contains("shared_except"), "{text}");
    assert!(text.contains("clear-defaults=true"), "{text}");
}

#[test]
fn a_managed_fragment_is_written_merged_and_only_when_it_loads() {
    let dir = tempfile::tempdir().unwrap();
    let overlay_dir = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
    let path = Config::write_managed_fragment(&overlay_dir, "mouse_mode false\n").unwrap();
    assert_eq!(path, overlay_dir.join(MANAGED_CONFIG_FILE_NAME));
    Config::write_managed_fragment(&overlay_dir, "hooks {\n    agent_exit \"\"\n}\n").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("mouse_mode false") && text.contains("agent_exit \"\""),
        "{text}"
    );

    let refused = Config::write_managed_fragment(&overlay_dir, "hooks {\n    on_coffee \"x\"\n}\n")
        .unwrap_err();
    assert!(refused.contains("would not load"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "a refused write changes nothing"
    );
    assert!(Config::write_managed_fragment(&overlay_dir, "hooks {").is_err());
}

#[test]
fn concurrent_managed_fragments_preserve_every_setting() {
    let dir = tempfile::tempdir().unwrap();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.path();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    Config::write_managed_fragment(
                        dir,
                        &format!("env {{ SETTING_{i} \"{i}\"; }}\n"),
                    )
                    .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let text = std::fs::read_to_string(dir.path().join(MANAGED_CONFIG_FILE_NAME)).unwrap();
    let managed: kdl::KdlDocument = text.parse().unwrap();
    let env = managed.get("env").unwrap().children().unwrap();
    for i in 0..8 {
        assert_eq!(
            env.get(&format!("SETTING_{i}"))
                .unwrap()
                .get(0)
                .unwrap()
                .value()
                .as_string(),
            Some(i.to_string().as_str())
        );
    }
}

/// Drop-ins layer in file-name order over the config, and a broken one is skipped.
#[test]
fn drop_ins_layer_in_name_order_and_a_broken_one_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let overlay_dir = dir.path().join(CONFIG_OVERLAY_DIR_NAME);
    std::fs::create_dir_all(&overlay_dir).unwrap();
    std::fs::write(overlay_dir.join("10-a.kdl"), "mouse_mode false\n").unwrap();
    std::fs::write(
        overlay_dir.join("20-b.kdl"),
        "mouse_mode true\nhooks {\n    agent_exit \"\"\n}\n",
    )
    .unwrap();
    std::fs::write(overlay_dir.join("30-broken.kdl"), "hooks {").unwrap();
    std::fs::write(overlay_dir.join("notes.txt"), "mouse_mode false\n").unwrap();
    let config = Config::from_default_assets()
        .unwrap()
        .with_overlay_dir(Some(&overlay_dir));
    assert_eq!(
        config.options.mouse_mode,
        Some(true),
        "the later drop-in wins"
    );
    assert!(config
        .hooks
        .get(yinz_types::input::hooks::HookPoint::AgentExit)
        .is_none());
    let untouched = Config::from_default_assets().unwrap();
    assert_eq!(
        untouched
            .clone()
            .with_overlay_dir(Some(&dir.path().join("missing"))),
        untouched
    );
}

#[test]
fn font_aliases_merge_through_sorted_drop_ins_and_blank_resets() {
    for (first, second) in [("font", "font_family"), ("font_family", "font")] {
        let dir = tempfile::tempdir().unwrap();
        let base = Config::from_kdl(
            &format!("terminal {{ {first} \"first\"; font_bold \"bold\"; }}"),
            None,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("20-reset.kdl"),
            format!("terminal {{ {second} \"  \"; font_italic \"italic\"; }}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("10-set.kdl"),
            format!("terminal {{ {second} \"second\"; }}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("30-invalid.kdl"),
            "terminal { font \"bad\"; font_family \"bad\"; }",
        )
        .unwrap();
        let config = base.with_overlay_dir(Some(dir.path()));
        assert_eq!(config.terminal.font_family.as_deref(), Some("  "));
        assert_eq!(config.terminal.font_bold.as_deref(), Some("bold"));
        assert_eq!(config.terminal.font_italic.as_deref(), Some("italic"));
        assert_eq!(config.terminal.font_bold_italic, None);
    }
}

#[test]
fn managed_font_alias_updates_replace_logical_key_and_reject_bad_fragments() {
    for (first, second) in [("font", "font_family"), ("font_family", "font")] {
        let dir = tempfile::tempdir().unwrap();
        let path = Config::write_managed_fragment(
            dir.path(),
            &format!("terminal {{ {first} \"old\"; font_bold \"bold\"; padding 2; }}"),
        )
        .unwrap();
        Config::write_managed_fragment(dir.path(), &format!("terminal {{ {second} \"\"; }}"))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::from_kdl(&text, None).unwrap();
        assert_eq!(config.terminal.font_family.as_deref(), Some(""));
        assert_eq!(config.terminal.font_bold.as_deref(), Some("bold"));
        assert_eq!(config.terminal.padding, Some(2.0));
        for bad in [
            "font \"x\"; font_family \"x\";",
            "font_family \"x\"; font \"y\";",
            "font \"x\"; font \"y\";",
        ] {
            assert!(
                Config::write_managed_fragment(dir.path(), &format!("terminal {{ {bad} }}"))
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }
}

#[test]
fn raw_font_writes_supersede_alternate_spelling_in_managed_edits() {
    for (first, second) in [("font", "font_family"), ("font_family", "font")] {
        for root in [false, true] {
            let mut managed = if root {
                doc(&format!("configuration_edits {{ set \"terminal\" {{ terminal {{ {first} \"old\"; font_size 14; }}; }}; }}"))
            } else {
                doc(&format!("configuration_edits {{ set \"terminal\" \"{first}\" {{ {first} \"old\"; }}; }}"))
            };
            merge_kdl_fragment(
                &mut managed,
                &doc(&format!("terminal {{ {second} \"new\"; }}")),
            );
            // The generic editor does not offer terminal settings. Exercise raw
            // reconciliation without expanding its catalog.
            let text = managed.to_string();
            assert!(!text.contains("old"), "{text}");
            assert!(text.contains(&format!("{second} \"new\"")), "{text}");
            if root {
                assert!(text.contains("font_size 14"), "{text}");
            } else {
                assert_eq!(
                    Config::from_kdl(&text, None)
                        .unwrap()
                        .terminal
                        .font_family
                        .as_deref(),
                    Some("new")
                );
            }
        }
    }
}

#[test]
fn managed_terminal_fragments_cannot_hide_errors_by_coalescing_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let path = Config::write_managed_fragment(dir.path(), "terminal { font \"old\"; }").unwrap();
    let before = std::fs::read(&path).unwrap();
    for fragment in [
        "terminal { font \"a\"; }\nterminal { font_family \"b\"; }",
        "terminal { font \"a\"; font_family \"b\"; }\nterminal { font \"c\"; }",
        "terminal { font \"a\"; }\nterminal { font \"b\"; font_family \"c\"; }",
    ] {
        assert!(Config::write_managed_fragment(dir.path(), fragment).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn each_face_can_be_independently_cleared_by_a_later_drop_in() {
    for name in ["font", "font_bold", "font_italic", "font_bold_italic"] {
        let dir = tempfile::tempdir().unwrap();
        let base = Config::from_kdl("terminal { font \"regular\"; font_bold \"bold\"; font_italic \"italic\"; font_bold_italic \"both\"; }", None).unwrap();
        std::fs::write(
            dir.path().join("10-reset.kdl"),
            format!("terminal {{ {name} \"  \"; }}"),
        )
        .unwrap();
        let config = base.with_overlay_dir(Some(dir.path()));
        let value = serde_json::to_value(&config.terminal).unwrap();
        for (key, expected) in [
            ("font", "regular"),
            ("font_bold", "bold"),
            ("font_italic", "italic"),
            ("font_bold_italic", "both"),
        ] {
            assert_eq!(value[key], if key == name { "  " } else { expected });
        }
    }
}

#[test]
fn ligature_aliases_layer_in_filename_order_and_skip_whole_invalid_fragments() {
    for (first, second) in [
        ("ligatures", "font_ligatures"),
        ("font_ligatures", "ligatures"),
    ] {
        for enabled in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let base = Config::from_kdl(
                &format!("terminal {{ {first} {enabled}; padding 2; }}"),
                None,
            )
            .unwrap();
            std::fs::write(
                dir.path().join("20-win.kdl"),
                format!("terminal {{ {first} {}; }}", !enabled),
            )
            .unwrap();
            std::fs::write(
                dir.path().join("10-set.kdl"),
                format!("terminal {{ {second} {enabled}; opacity 0.5; }}"),
            )
            .unwrap();
            for (file, bad) in [
                ("30-bad.kdl", format!("{first} true; {second} false;")),
                ("40-bad.kdl", format!("{second} \"false\";")),
            ] {
                std::fs::write(
                    dir.path().join(file),
                    format!("terminal {{ padding 99; {bad} }}"),
                )
                .unwrap();
            }
            let config = base.with_overlay_dir(Some(dir.path()));
            assert_eq!(config.terminal.font_ligatures, Some(!enabled));
            assert_eq!(config.terminal.padding, Some(2.0));
            assert_eq!(config.terminal.opacity, Some(0.5));
        }
    }
}

#[test]
fn managed_ligature_alias_writes_replace_one_setting_and_reject_errors_atomically() {
    for (first, second) in [
        ("ligatures", "font_ligatures"),
        ("font_ligatures", "ligatures"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = Config::write_managed_fragment(
            dir.path(),
            &format!("terminal {{ {first} true; padding 2; }}"),
        )
        .unwrap();
        for (name, enabled) in [(second, false), (first, true), (second, true)] {
            Config::write_managed_fragment(
                dir.path(),
                &format!("terminal {{ {name} {enabled}; }}"),
            )
            .unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert_eq!(text.matches("ligatures ").count(), 1, "{text}");
            let config = Config::from_kdl(&text, None).unwrap();
            assert_eq!(config.terminal.font_ligatures, Some(enabled));
            assert_eq!(config.terminal.padding, Some(2.0));
        }
        let before = std::fs::read(&path).unwrap();
        for fragment in [
            format!("terminal {{ {first} true; {second} true; }}"),
            format!("terminal {{ {second} false; {first} true; }}"),
            format!("terminal {{ {first} false; {first} false; }}"),
            format!("terminal {{ {second} 1; }}"),
            format!("terminal {{ {first} true; }}\nterminal {{ {second} false; }}"),
            format!("terminal {{ {first} true; {second} false; }}\nterminal {{ padding 9; }}"),
            format!("terminal {{ padding 9; }}\nterminal {{ {first} null; }}"),
        ] {
            assert!(
                Config::write_managed_fragment(dir.path(), &fragment).is_err(),
                "{fragment}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}

#[test]
fn raw_ligature_writes_reconcile_root_and_child_configuration_edits() {
    for (first, second) in [
        ("ligatures", "font_ligatures"),
        ("font_ligatures", "ligatures"),
    ] {
        for root in [false, true] {
            let mut managed = if root {
                doc(&format!("configuration_edits {{ set \"terminal\" {{ terminal {{ {first} true; padding 2; }}; }}; }}"))
            } else {
                doc(&format!(
                    "configuration_edits {{ set \"terminal\" \"{first}\" {{ {first} true; }}; }}"
                ))
            };
            merge_kdl_fragment(
                &mut managed,
                &doc(&format!("terminal {{ {second} false; }}")),
            );
            let text = managed.to_string();
            assert!(!text.contains("true"), "{text}");
            assert!(text.contains(&format!("{second} false")), "{text}");
            if root {
                assert!(text.contains("padding 2"), "{text}");
            } else {
                assert_eq!(
                    Config::from_kdl(&text, None)
                        .unwrap()
                        .terminal
                        .font_ligatures,
                    Some(false)
                );
            }
        }
    }
}
