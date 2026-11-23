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

#[test]
fn cell_metrics_sorted_layers_inherit_override_and_explicitly_reset() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl(
        "terminal { cell_width 1.5; line_height 1.25; padding 2; }",
        None,
    )
    .unwrap();
    for (file, text) in [
        ("20-reset.kdl", "terminal { cell_width 1; }"),
        ("10-set.kdl", "terminal { cell_width 2; line_height 1.5; }"),
        ("30-invalid.kdl", "terminal { cell_width 4; padding 99; }"),
        (
            "40-duplicate.kdl",
            "terminal { line_height 1; line_height 1; padding 98; }",
        ),
    ] {
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    let config = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(config.terminal.cell_width, Some(1.0));
    assert_eq!(config.terminal.line_height, Some(1.5));
    assert_eq!(config.terminal.padding, Some(2.0));
    let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config.clone())).unwrap();
    assert_eq!(inherited.terminal.cell_width, Some(1.0));
    assert_eq!(inherited.terminal.line_height, Some(1.5));
    let reset = Config::from_kdl("terminal { line_height 1; }", Some(inherited)).unwrap();
    assert_eq!(reset.terminal.cell_width, Some(1.0));
    assert_eq!(reset.terminal.line_height, Some(1.0));
    assert_eq!(
        Config::from_kdl(&reset.to_string(false).unwrap(), None)
            .unwrap()
            .terminal,
        reset.terminal
    );
}

#[test]
fn managed_cell_metrics_preserve_siblings_and_reject_whole_invalid_fragments() {
    let dir = tempfile::tempdir().unwrap();
    let path = Config::write_managed_fragment(
        dir.path(),
        "terminal { cell_width 1.5; line_height 2; padding 3; }",
    )
    .unwrap();
    Config::write_managed_fragment(dir.path(), "terminal { cell_width 1; }").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let config = Config::from_kdl(&before, None).unwrap();
    assert_eq!(config.terminal.cell_width, Some(1.0));
    assert_eq!(config.terminal.line_height, Some(2.0));
    assert_eq!(config.terminal.padding, Some(3.0));
    for bad in [
        "line_height 0.49;",
        "cell_width true;",
        "cell_width 1; cell_width 1;",
        "line_height (float)1.5;",
    ] {
        assert!(Config::write_managed_fragment(
            dir.path(),
            &format!("terminal {{ padding 99; {bad} }}")
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}

fn feature_list(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|tag| tag.to_string()).collect())
}

#[test]
fn font_features_layer_wholesale_in_filename_order_and_skip_invalid_fragments() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl(
        "terminal { font_features \"ss01\" \"zero\"; padding 2; }",
        None,
    )
    .unwrap();
    for (file, text) in [
        ("20-replace.kdl", "terminal { font_features \"-calt\"; }"),
        (
            "10-set.kdl",
            "terminal { font_features \"ss02\" \"+dlig\"; opacity 0.5; }",
        ),
        (
            "30-invalid.kdl",
            "terminal { font_features \"ss01\" \"ss1\"; padding 99; }",
        ),
        (
            "35-duplicate.kdl",
            "terminal { font_features \"zero\"; font_features \"ss01\"; padding 98; }",
        ),
    ] {
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    let config = base.clone().with_overlay_dir(Some(dir.path()));
    // The later list replaces the earlier one whole: no per-tag merging.
    assert_eq!(config.terminal.font_features, feature_list(&["-calt"]));
    assert_eq!(config.terminal.padding, Some(2.0));
    assert_eq!(config.terminal.opacity, Some(0.5));

    std::fs::write(
        dir.path().join("40-clear.kdl"),
        "terminal { font_features; }",
    )
    .unwrap();
    let cleared = base.clone().with_overlay_dir(Some(dir.path()));
    assert_eq!(cleared.terminal.font_features, feature_list(&[]));
    assert_eq!(cleared.terminal.padding, Some(2.0));

    let untouched = tempfile::tempdir().unwrap();
    std::fs::write(
        untouched.path().join("10-other.kdl"),
        "terminal { opacity 0.7; }",
    )
    .unwrap();
    let inherited = base.with_overlay_dir(Some(untouched.path()));
    assert_eq!(
        inherited.terminal.font_features,
        feature_list(&["ss01", "zero"])
    );
}

#[test]
fn managed_font_feature_writes_replace_one_node_and_reject_errors_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = Config::write_managed_fragment(
        dir.path(),
        "terminal { font_features \"ss01\" \"zero\"; padding 2; }",
    )
    .unwrap();
    for (fragment, expected) in [
        (
            "terminal { font_features \"-calt\" \"+dlig\"; }",
            vec!["-calt", "+dlig"],
        ),
        ("terminal { font_features; }", vec![]),
        ("terminal { font_features \"ss01\"; }", vec!["ss01"]),
    ] {
        Config::write_managed_fragment(dir.path(), fragment).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("font_features").count(), 1, "{text}");
        let config = Config::from_kdl(&text, None).unwrap();
        assert_eq!(config.terminal.font_features, feature_list(&expected));
        assert_eq!(config.terminal.padding, Some(2.0));
    }
    let before = std::fs::read(&path).unwrap();
    for fragment in [
        "terminal { padding 9; font_features \"ss1\"; }",
        "terminal { padding 9; font_features 4; }",
        "terminal { padding 9; font_features \"cv01=2\"; }",
        "terminal { font_features \"zero\"; font_features \"ss01\"; }",
        "terminal { font_features \"zero\"; }\nterminal { font_features \"ss01\"; }",
        "terminal { padding 9; }\nterminal { font_features \"--calt\"; }",
    ] {
        assert!(
            Config::write_managed_fragment(dir.path(), fragment).is_err(),
            "{fragment}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn native_terminal_baseline_offset_layers_reset_zero_and_invalid_managed_writes_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl("terminal { baseline_offset -2; padding 3; }", None).unwrap();
    for (name, text) in [
        ("20-reset.kdl", "terminal { baseline_offset 0; }"),
        ("10-set.kdl", "terminal { baseline_offset 2; }"),
        (
            "30-invalid.kdl",
            "terminal { baseline_offset 65; padding 99; }",
        ),
        (
            "40-invalid.kdl",
            "terminal { baseline_offset 2; baseline_offset 2; padding 98; }",
        ),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let config = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(config.terminal.baseline_offset, Some(0));
    assert_eq!(config.terminal.padding, Some(3.0));
    let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config)).unwrap();
    assert_eq!(inherited.terminal.baseline_offset, Some(0));
    let path =
        Config::write_managed_fragment(dir.path(), "terminal { baseline_offset -2; padding 4; }")
            .unwrap();
    Config::write_managed_fragment(dir.path(), "terminal { baseline_offset 0; }").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let saved = Config::from_kdl(&before, None).unwrap();
    assert_eq!(saved.terminal.baseline_offset, Some(0));
    assert_eq!(saved.terminal.padding, Some(4.0));
    for bad in ["65", "2.0", "true", "(int)2", "2; baseline_offset 2"] {
        assert!(Config::write_managed_fragment(
            dir.path(),
            &format!("terminal {{ baseline_offset {bad}; padding 99; }}")
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}

#[test]
fn underline_metrics_use_independent_layer_winners_and_atomic_managed_writes() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl(
        "terminal { underline_offset -2; underline_thickness 1; padding 3; }",
        None,
    )
    .unwrap();
    for (name, text) in [
        ("20-thickness.kdl", "terminal { underline_thickness -3; }"),
        ("10-offset.kdl", "terminal { underline_offset 2; }"),
        ("30-reset.kdl", "terminal { underline_offset 0; }"),
        (
            "40-invalid.kdl",
            "terminal { underline_thickness 65; underline_offset 9; padding 99; }",
        ),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let config = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(
        (
            config.terminal.underline_offset,
            config.terminal.underline_thickness,
            config.terminal.padding
        ),
        (Some(0), Some(-3), Some(3.0))
    );
    let path = Config::write_managed_fragment(
        dir.path(),
        "terminal { underline_offset -2; underline_thickness 2; padding 4; }",
    )
    .unwrap();
    Config::write_managed_fragment(dir.path(), "terminal { underline_offset 0; }").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let saved = Config::from_kdl(&before, None).unwrap().terminal;
    assert_eq!(
        (
            saved.underline_offset,
            saved.underline_thickness,
            saved.padding
        ),
        (Some(0), Some(2), Some(4.0))
    );
    for name in ["underline_offset", "underline_thickness"] {
        for bad in ["65", "2.0", "true", "(int)2", &format!("2; {name} 2")] {
            let fragment = format!("terminal {{ {name} {bad}; padding 99; }}");
            assert!(
                Config::write_managed_fragment(dir.path(), &fragment).is_err(),
                "{fragment}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        }
    }
}

#[test]
fn underline_metrics_invalid_drop_in_discards_preloaded_themes() {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    let overlays = dir.path().join("config.d");
    std::fs::create_dir(&themes).unwrap();
    std::fs::create_dir(&overlays).unwrap();
    let source =
        include_str!("../../assets/themes/adwaita.kdl").replacen("adwaita", "underline-fixture", 1);
    std::fs::write(themes.join("fixture.kdl"), source).unwrap();
    let base = Config::default();
    let mut preload = base.clone();
    apply_themes(&mut preload, None, Some(dir.path().to_path_buf())).unwrap();
    assert!(preload.themes.get_theme("underline-fixture").is_some());
    let invalid = overlays.join("10-invalid.kdl");
    std::fs::write(
        &invalid,
        r#"
        configuration_edits { remove "themes" "unrelated"; }
        terminal { underline_thickness 65; }
    "#,
    )
    .unwrap();
    std::fs::write(
        overlays.join("20-valid.kdl"),
        "terminal { underline_offset -2; }",
    )
    .unwrap();
    let logs = captured_config_logs();
    let loaded = base.with_overlay_dir(Some(&overlays));
    assert!(logs.lock().iter().any(|line| {
        line.contains("Skipping config drop-in")
            && line.contains(invalid.to_str().unwrap())
            && line.contains("underline_thickness")
            && line.contains("between -64 and 64")
    }));
    assert!(loaded.themes.get_theme("underline-fixture").is_none());
    assert_eq!(loaded.terminal.underline_offset, Some(-2));
    let saved = Config::load_saved(None, Some(dir.path())).unwrap();
    assert_eq!(saved.terminal.underline_offset, Some(-2));
    assert_eq!(saved.terminal.underline_thickness, None);
}

const MANAGED_LAYOUT_EDITS: &str = r#"
        configuration_edits {
            set "keybinds" "normal" "bind:[\"Ctrl Alt y\"]" {
                bind "Ctrl Alt y" { Quit; }
            }
        }
        terminal { underline_thickness 3; padding 4; }
    "#;

#[cfg(unix)]
#[test]
fn underline_metrics_managed_save_preserves_required_layout_fallback() {
    use crate::editor::Disk;
    use std::{
        fs,
        os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    };
    use yinz_types::{data::InputMode, input::actions::Action};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let overlay = root.join("config.d");
    let layouts = root.join("layouts");
    fs::create_dir(&overlay).unwrap();
    fs::create_dir(&layouts).unwrap();
    let managed = overlay.join(MANAGED_CONFIG_FILE_NAME);
    let first = layouts.join("default");
    let fallback = layouts.join("default.kdl");
    let first_text = "layout { pane; }";
    let fallback_text = "layout { pane; pane; }";
    fs::write(&managed, MANAGED_LAYOUT_EDITS).unwrap();
    fs::write(&first, first_text).unwrap();
    fs::write(&fallback, fallback_text).unwrap();
    fs::set_permissions(&first, fs::Permissions::from_mode(0o000)).unwrap();

    match fs::File::open(&first) {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
        Ok(_) => {
            // A privileged runner can bypass mode bits. Exercise the saved selection
            // with the corresponding failed open and the real on-disk candidates.
            let denied = io::Error::from(io::ErrorKind::PermissionDenied);
            let (selected, raw) = crate::layout::read_saved_layout(&first, Err(denied)).unwrap();
            assert_eq!(selected, fallback);
            assert_eq!(raw, fallback_text);
        },
    }
    let preview = || Disk::read(None, Some(root)).unwrap().preview(&[], &[]);
    let (_, before, _) = preview().unwrap();
    assert_eq!(before.terminal.underline_offset, None);
    let fragment = "terminal { underline_offset -2; }";
    Config::write_managed_fragment(&overlay, fragment).unwrap();
    let (_, saved, _) = preview().unwrap();
    let effective = Config::load_saved(None, Some(root)).unwrap();
    for config in [saved, effective] {
        assert_eq!(config.terminal.underline_offset, Some(-2));
        assert_eq!(config.terminal.underline_thickness, Some(3));
        assert_eq!(config.terminal.padding, Some(4.0));
        assert_eq!(
            config.keybinds.0[&InputMode::Normal][&"Ctrl Alt y".parse().unwrap()],
            [Action::Quit]
        );
    }
    assert_eq!(
        fs::metadata(&first).unwrap().permissions().mode() & 0o777,
        0
    );
    fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(&first).unwrap(), first_text);
    assert_eq!(fs::read_to_string(&fallback).unwrap(), fallback_text);

    let committed = fs::read(&managed).unwrap();
    let assert_refused = |diagnostic: &str| {
        let error = Config::write_managed_fragment(&overlay, "terminal { underline_offset 3; }")
            .unwrap_err();
        assert!(error.contains(first.to_str().unwrap()), "{error}");
        assert!(error.contains(diagnostic), "{error}");
        assert_eq!(fs::read(&managed).unwrap(), committed);
        assert_eq!(fs::read_to_string(&fallback).unwrap(), fallback_text);
    };

    let invalid_utf8 = b"invalid UTF-8: \xff";
    fs::write(&first, invalid_utf8).unwrap();
    assert_refused("UTF-8");
    assert_eq!(fs::read(&first).unwrap(), invalid_utf8);

    let invalid_kdl = "layout {";
    fs::write(&first, invalid_kdl).unwrap();
    assert_refused("Kdl Deserialization Error");
    assert_eq!(fs::read_to_string(&first).unwrap(), invalid_kdl);

    fs::remove_file(&first).unwrap();
    let name = std::ffi::CString::new(first.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_refused("not a regular file");
}

#[test]
fn underline_metrics_managed_save_accepts_optional_layout_text_fallbacks() {
    use crate::editor::Disk;
    use std::fs;
    use yinz_types::{data::InputMode, input::actions::Action};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let overlay = root.join("config.d");
    let layouts = root.join("layouts");
    fs::create_dir(&overlay).unwrap();
    fs::create_dir(&layouts).unwrap();
    fs::write(
        layouts.join("default.kdl"),
        "layout { pane contents_file=\"preview.txt\"; }",
    )
    .unwrap();
    let managed = overlay.join(MANAGED_CONFIG_FILE_NAME);
    let swap = layouts.join("default.swap.kdl");
    let contents = layouts.join("preview.txt");
    let valid_swap = include_bytes!("../../assets/layouts/default.swap.kdl").as_slice();
    let valid_contents = b"initial contents".as_slice();
    let invalid_text = b"invalid UTF-8: \xff".as_slice();
    let fragment = "terminal { underline_offset -2; }";

    for files in [
        [Some(valid_swap), Some(valid_contents)],
        [Some(invalid_text), Some(valid_contents)],
        [Some(valid_swap), Some(invalid_text)],
        [None, None],
    ] {
        fs::write(&managed, MANAGED_LAYOUT_EDITS).unwrap();
        for (path, bytes) in [&swap, &contents].into_iter().zip(files) {
            if let Some(bytes) = bytes {
                fs::write(path, bytes).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }
        let preview = || Disk::read(None, Some(root)).unwrap().preview(&[], &[]);
        let (_, before, _) = preview().unwrap();
        assert_eq!(before.terminal.underline_offset, None);
        Config::write_managed_fragment(&overlay, fragment).unwrap();
        let (_, ordinary, _) = preview().unwrap();
        let effective = Config::load_saved(None, Some(root)).unwrap();
        for config in [ordinary, effective] {
            assert_eq!(config.terminal.underline_offset, Some(-2));
            assert_eq!(config.terminal.underline_thickness, Some(3));
            assert_eq!(config.terminal.padding, Some(4.0));
            assert_eq!(
                config.keybinds.0[&InputMode::Normal][&"Ctrl Alt y".parse().unwrap()],
                [Action::Quit]
            );
        }
        for (path, bytes) in [&swap, &contents].into_iter().zip(files) {
            if let Some(bytes) = bytes {
                assert_eq!(fs::read(path).unwrap(), bytes);
            } else {
                assert!(!path.exists());
            }
        }
    }

    fs::write(&managed, MANAGED_LAYOUT_EDITS).unwrap();
    fs::write(&swap, "swap_tiled_layout {").unwrap();
    let error = Config::write_managed_fragment(&overlay, fragment).unwrap_err();
    assert!(error.contains(swap.to_str().unwrap()), "{error}");
    assert_eq!(fs::read_to_string(&managed).unwrap(), MANAGED_LAYOUT_EDITS);
    assert_eq!(fs::read_to_string(&swap).unwrap(), "swap_tiled_layout {");
}

#[cfg(unix)]
#[test]
fn underline_metrics_managed_save_rejects_nonregular_validation_inputs() {
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{symlink, FileTypeExt},
    };
    use yinz_types::{data::InputMode, input::actions::Action};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let overlay = root.join("config.d");
    let themes = root.join("themes");
    let layouts = root.join("layouts");
    for directory in [&overlay, &themes, &layouts] {
        std::fs::create_dir(directory).unwrap();
    }
    let fifo = root.join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let managed = overlay.join(MANAGED_CONFIG_FILE_NAME);
    let fragment = "terminal { underline_offset -2; }\nmouse_mode false";
    let assert_refused = |path: &Path| {
        let error = Config::write_managed_fragment(&overlay, fragment).unwrap_err();
        for expected in [path.to_str().unwrap(), "not a regular file", "then retry"] {
            assert!(error.contains(expected), "{error}");
        }
    };

    std::fs::rename(&fifo, &managed).unwrap();
    assert_refused(&managed);
    assert!(std::fs::symlink_metadata(&managed)
        .unwrap()
        .file_type()
        .is_fifo());
    std::fs::rename(&managed, &fifo).unwrap();

    let main = root.join("config.kdl");
    let theme = themes.join("fixture.kdl");
    let layout = layouts.join("default.kdl");
    let swap = layouts.join("default.swap.kdl");
    let contents = layouts.join("preview.txt");
    let action_layout = layouts.join("new-tab.kdl");
    let main_text = format!(
        "keybinds {{ normal {{ bind \"Ctrl u\" {{ NewTab {{ layout \"{}\"; }}; }}; }}; }}",
        action_layout.display()
    );
    let original = MANAGED_LAYOUT_EDITS;
    std::fs::write(&managed, original).unwrap();
    let files = [
        (&main, main_text.as_str()),
        (&theme, include_str!("../../assets/themes/adwaita.kdl")),
        (&layout, "layout { pane contents_file=\"preview.txt\"; }"),
        (&swap, include_str!("../../assets/layouts/default.swap.kdl")),
        (&contents, "initial contents"),
        (&action_layout, "layout { pane; }"),
    ];
    for (path, content) in files {
        std::fs::write(path, content).unwrap();
    }
    for (path, content) in files {
        std::fs::remove_file(path).unwrap();
        symlink(&fifo, path).unwrap();
        assert_refused(path);
        assert_eq!(std::fs::read_link(path).unwrap(), fifo);
        assert_eq!(std::fs::read_to_string(&managed).unwrap(), original);
        std::fs::remove_file(path).unwrap();
        std::fs::write(path, content).unwrap();
    }

    // A symlink to a regular input is still readable by the managed writer.
    let regular = root.join("regular.kdl");
    std::fs::rename(&managed, &regular).unwrap();
    symlink(&regular, &managed).unwrap();
    Config::try_write_managed_fragment_guarded(&overlay, fragment, || Ok(())).unwrap();
    let loaded = Config::load(None, Some(root), false).unwrap();
    assert_eq!(loaded.terminal.underline_offset, Some(-2));
    assert_eq!(loaded.terminal.underline_thickness, Some(3));
    assert_eq!(loaded.terminal.padding, Some(4.0));
    assert_eq!(loaded.options.mouse_mode, Some(false));
    assert_eq!(
        loaded.keybinds.0[&InputMode::Normal][&"Ctrl Alt y".parse().unwrap()],
        [Action::Quit]
    );
    assert_eq!(std::fs::read_to_string(regular).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn underline_metrics_saved_comparison_rejects_indeterminate_theme_preload() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    let overlays = dir.path().join("config.d");
    std::fs::create_dir(&themes).unwrap();
    std::fs::create_dir(&overlays).unwrap();
    let theme = themes.join("fixture.kdl");
    let source = include_str!("../../assets/themes/adwaita.kdl");
    std::fs::write(&theme, source).unwrap();
    std::fs::write(
        overlays.join("10-theme-edit.kdl"),
        r#"
        configuration_edits { remove "themes" "unrelated"; }
        terminal { underline_thickness 2; }
    "#,
    )
    .unwrap();
    std::fs::write(
        overlays.join("20-valid.kdl"),
        "terminal { underline_offset -2; }",
    )
    .unwrap();
    let regular = Config::load_saved(None, Some(dir.path())).unwrap();
    assert_eq!(
        (
            regular.terminal.underline_offset,
            regular.terminal.underline_thickness
        ),
        (Some(-2), Some(2))
    );

    std::fs::remove_file(&theme).unwrap();
    let fifo = std::ffi::CString::new(theme.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let error = Config::load_saved(None, Some(dir.path())).unwrap_err();
    assert!(
        matches!(&error, ConfigError::IoPath(cause, path)
            if path == &theme && cause.to_string().contains("not a regular file")),
        "{error}"
    );
}

#[test]
fn native_startup_dimensions_overlay_contract() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl(
        "terminal { initial_columns 80; initial_rows 24; padding 3; }",
        None,
    )
    .unwrap();
    for (name, text) in [
        (
            "40-invalid.kdl",
            "terminal { initial_columns 9; initial_rows 1001; padding 99; }",
        ),
        ("30-rows.kdl", "terminal { initial_rows 33; }"),
        ("20-columns.kdl", "terminal { initial_columns 110; }"),
        ("10-columns.kdl", "terminal { initial_columns 95; }"),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let loaded = base.clone().with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.initial_columns, Some(110));
    assert_eq!(loaded.terminal.initial_rows, Some(33));
    assert_eq!(loaded.terminal.padding, Some(3.0));

    let path = Config::write_managed_fragment(
        dir.path(),
        "terminal { initial_columns 115; initial_rows 35; }",
    )
    .unwrap();
    Config::write_managed_fragment(dir.path(), "terminal { initial_columns 120; }").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let saved = Config::from_kdl(&before, None).unwrap();
    assert_eq!(saved.terminal.initial_columns, Some(120));
    assert_eq!(saved.terminal.initial_rows, Some(35));
    for fragment in [
        "terminal { initial_columns 1; initial_rows 36; }",
        "terminal { initial_columns 11; initial_columns 12; }",
    ] {
        assert!(Config::write_managed_fragment(dir.path(), fragment).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
    let loaded = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.initial_columns, Some(120));
    assert_eq!(loaded.terminal.initial_rows, Some(35));
}

#[test]
fn native_terminal_minimum_contrast_drop_ins_override_inherit_and_reject_invalid_writes() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl("terminal { minimum_contrast 3; padding 2; }", None).unwrap();
    for (name, text) in [
        ("30-invalid.kdl", "terminal { minimum_contrast 22; }"),
        ("20-other.kdl", "terminal { opacity 0.5; }"),
        ("10-contrast.kdl", "terminal { minimum_contrast 4.5; }"),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let loaded = base.clone().with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.minimum_contrast, Some(4.5));
    assert_eq!(loaded.terminal.padding, Some(2.0));
    assert_eq!(loaded.terminal.opacity, Some(0.5));

    let path =
        Config::write_managed_fragment(dir.path(), "terminal { minimum_contrast 7; }").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        Config::from_kdl(&before, None)
            .unwrap()
            .terminal
            .minimum_contrast,
        Some(7.0)
    );
    for fragment in [
        "terminal { minimum_contrast 0.5; }",
        "terminal { minimum_contrast 4.5; minimum_contrast 5; }",
    ] {
        assert!(Config::write_managed_fragment(dir.path(), fragment).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
    let loaded = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.minimum_contrast, Some(7.0));
    assert_eq!(loaded.terminal.padding, Some(2.0));
}

#[test]
fn native_terminal_opacity_mode_drop_ins_override_inherit_and_reject_invalid_writes() {
    use yinz_types::input::native_terminal::OpacityMode;
    let dir = tempfile::tempdir().unwrap();
    let base = Config::from_kdl(
        "terminal { opacity 0.6; opacity_mode \"everything\"; }",
        None,
    )
    .unwrap();
    for (name, text) in [
        ("30-invalid.kdl", "terminal { opacity_mode \"sideways\"; }"),
        ("25-invalid-opacity.kdl", "terminal { opacity 1.5; }"),
        ("20-other.kdl", "terminal { padding 2; }"),
        ("10-mode.kdl", "terminal { opacity_mode \"Background\"; }"),
    ] {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let loaded = base.clone().with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.opacity_mode, Some(OpacityMode::Background));
    assert_eq!(loaded.terminal.opacity, Some(0.6));
    assert_eq!(loaded.terminal.padding, Some(2.0));

    let path = Config::write_managed_fragment(
        dir.path(),
        "terminal { opacity_mode \"everything\"; opacity 0.4; }",
    )
    .unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let saved = Config::from_kdl(&before, None).unwrap();
    assert_eq!(saved.terminal.opacity_mode, Some(OpacityMode::Everything));
    assert_eq!(saved.terminal.opacity, Some(0.4));
    for fragment in [
        "terminal { opacity_mode \"sideways\"; }",
        "terminal { opacity_mode \"everything\"; opacity_mode \"background\"; }",
        "terminal { opacity -0.1; }",
    ] {
        assert!(Config::write_managed_fragment(dir.path(), fragment).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
    let loaded = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(loaded.terminal.opacity_mode, Some(OpacityMode::Everything));
    assert_eq!(loaded.terminal.opacity, Some(0.4));
    assert_eq!(loaded.terminal.padding, Some(2.0));
}
