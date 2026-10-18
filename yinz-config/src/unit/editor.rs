use super::*;
use serde_json::json;

fn patch(path: &[&str], value: Value) -> Patch {
    Patch {
        reset: false,
        path: path.iter().map(|s| (*s).into()).collect(),
        node: Some(Node {
            name: path.last().unwrap().to_string(),
            arguments: vec![value],
            properties: Default::default(),
            children: vec![],
        }),
    }
}

#[test]
fn ambiguous_collection_edits_leave_repeated_entries_unchanged() {
    let entry = patch(&["separator"], json!(true)).node.unwrap();
    let mut nodes = vec![Node {
        name: "context_menu".into(),
        arguments: vec![],
        properties: Default::default(),
        children: vec![entry.clone(), entry],
    }];
    let original = nodes.clone();
    for replacement in [
        None,
        Some(patch(&["separator"], json!(false)).node.unwrap()),
    ] {
        let edit = Patch {
            reset: false,
            path: vec!["context_menu".into(), "separator".into()],
            node: replacement,
        };
        assert_eq!(
            patch_document(&mut nodes, &edit).unwrap_err(),
            "Edit the ordered parent collection instead"
        );
        assert_eq!(nodes, original);
    }
}

#[test]
fn catalog_covers_and_validates_every_option() {
    let fields = serde_json::to_value(yinz_types::input::options::Options::default()).unwrap();
    let catalog = catalog();
    assert_eq!(fields.as_object().unwrap().len(), catalog.len());
    for setting in catalog {
        assert!(
            !setting.help.is_empty(),
            "missing explanation for {}",
            setting.name
        );
        let value = match setting.kind {
            "number" => json!(setting.minimum.unwrap_or(0)),
            "boolean" => json!(true),
            "choice" => json!(setting.choices[0]),
            "theme" => json!("default"),
            _ if setting.name == "host_id" => json!("test-host"),
            _ => json!("test"),
        };
        apply(&Config::default(), &[patch(&[&setting.name], value)])
            .unwrap_or_else(|error| panic!("{}: {error}", setting.name));
    }
}

#[test]
fn numeric_boundaries_and_whitespace_are_validated_by_kdl() {
    for value in [-1, 101] {
        assert!(apply(
            &Config::default(),
            &[patch(&["background_opacity"], json!(value))]
        )
        .is_err());
    }
    assert!(apply(
        &Config::default(),
        &[patch(&["serialization_interval"], json!(0))]
    )
    .is_err());
    let config = apply(
        &Config::default(),
        &[patch(&["word_separators"], json!(" \t"))],
    )
    .unwrap();
    assert_eq!(config.options.word_separators.as_deref(), Some(" \t"));
    let empty = apply(&config, &[patch(&["word_separators"], json!(""))]).unwrap();
    assert_eq!(empty.options.word_separators.as_deref(), Some(""));
    let unset = apply(
        &config,
        &[Patch {
            reset: false,
            path: vec!["word_separators".into()],
            node: None,
        }],
    )
    .unwrap();
    assert_eq!(unset.options.word_separators, None);
}

#[test]
fn removals_and_targeted_edits_survive_reload() {
    let base = Config::from_kdl("env {\n A \"secret\"\n B \"keep\"\n}\n", None).unwrap();
    let edits = vec![Patch {
        reset: false,
        path: vec!["env".into(), "A".into()],
        node: None,
    }];
    let fragment = patches_to_kdl(&edits).unwrap().to_string();
    let changed = Config::from_kdl(&fragment, Some(base.clone())).unwrap();
    let nodes = document(&changed).unwrap();
    let env = nodes.iter().find(|n| n.name == "env").unwrap();
    assert_eq!(env.children.len(), 1);
    assert_eq!(env.children[0].name, "B");
    let mut managed: KdlDocument = fragment.parse().unwrap();
    crate::config::merge_kdl_fragment(
        &mut managed,
        &"configuration_edits { reset \"env\" \"A\"; }"
            .parse()
            .unwrap(),
    );
    assert_eq!(
        Config::from_kdl(&managed.to_string(), Some(base.clone())).unwrap(),
        base
    );
}

#[test]
fn disk_preview_preserves_other_layers_and_reports_precedence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config.d")).unwrap();
    std::fs::write(dir.path().join("config.kdl"), "mouse_mode true\n").unwrap();
    std::fs::write(dir.path().join("config.d/z-last.kdl"), "mouse_mode true\n").unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let (fragment, config, underlay) = disk
        .preview(&[patch(&["mouse_mode"], json!(false))], &[])
        .unwrap();
    assert_eq!(config.options.mouse_mode, Some(true));
    Config::try_write_editor_fragment(&disk.overlay, &fragment, underlay, || Err("stale".into()))
        .unwrap_err();
    assert!(!disk
        .overlay
        .join(crate::config::MANAGED_CONFIG_FILE_NAME)
        .exists());
}

#[test]
fn action_catalog_covers_parser_spellings() {
    let parser = include_str!("../kdl/action.rs")
        .split("pub fn from_kdl")
        .nth(1)
        .unwrap();
    let mut names: Vec<_> = parser
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("        ")?;
            let line = line.strip_prefix("| ").unwrap_or(line);
            line.starts_with('"').then_some(line)
        })
        .flat_map(|line| {
            line.split(" | ").filter_map(|alternative| {
                alternative
                    .strip_prefix('"')
                    .map(|name| name.split('"').next().unwrap())
            })
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut catalog = ACTION_NAMES.to_vec();
    catalog.sort_unstable();
    catalog.dedup();
    assert_eq!(names, catalog);
}

#[test]
fn binding_edits_do_not_capture_other_bindings() {
    let base = Config::from_kdl("keybinds clear-defaults=true { normal { bind \"Ctrl a\" { Quit; }; bind \"Ctrl b\" { NoOp; }; }; }", None).unwrap();
    let mut bind = document(&base)
        .unwrap()
        .into_iter()
        .find(|n| n.name == "keybinds")
        .unwrap()
        .children[0]
        .children[0]
        .clone();
    bind.children[0].name = "Copy".into();
    let edit = Patch {
        reset: false,
        path: vec!["keybinds".into(), "normal".into(), node_key(&bind)],
        node: Some(bind),
    };
    let changed = apply(&base, std::slice::from_ref(&edit)).unwrap();
    let serialized = patches_to_kdl(&[edit]).unwrap().to_string();
    assert!(!serialized.contains("Ctrl b"));
    assert_eq!(Config::from_kdl(&serialized, Some(base)).unwrap(), changed);
}

#[test]
fn reset_nested_override_reveals_underlying_config_and_preserves_siblings() {
    let base = Config::from_kdl("env { A \"configured\"; B \"original\"; }", None).unwrap();
    let whole = Patch {
        reset: false,
        path: vec!["env".into()],
        node: Some(Node {
            name: "env".into(),
            arguments: vec![],
            properties: Default::default(),
            children: vec![
                patch(&["A"], json!("temporary")).node.unwrap(),
                patch(&["B"], json!("temporary-b")).node.unwrap(),
            ],
        }),
    };
    let reset = Patch {
        reset: true,
        path: vec!["env".into(), "A".into()],
        node: None,
    };
    let changed = apply(&base, &[whole.clone(), reset.clone()]).unwrap();
    let nodes = document(&changed).unwrap();
    assert_eq!(
        find_node(&nodes, &reset.path).unwrap().arguments,
        vec![json!("configured")]
    );
    assert_eq!(
        find_node(&nodes, &["env".into(), "B".into()])
            .unwrap()
            .arguments,
        vec![json!("temporary-b")]
    );
    let mut managed: KdlDocument = patches_to_kdl(&[whole])
        .unwrap()
        .to_string()
        .parse()
        .unwrap();
    crate::config::merge_kdl_fragment(
        &mut managed,
        &patches_to_kdl(&[reset])
            .unwrap()
            .to_string()
            .parse()
            .unwrap(),
    );
    assert_eq!(
        Config::from_kdl(&managed.to_string(), Some(base)).unwrap(),
        changed
    );
    let mut legacy: KdlDocument = "mouse_mode false\nenv { A \"managed\"; B \"keep\"; }"
        .parse()
        .unwrap();
    crate::config::merge_kdl_fragment(
        &mut legacy,
        &"configuration_edits { reset \"mouse_mode\"; reset \"env\" \"A\"; }"
            .parse()
            .unwrap(),
    );
    assert!(legacy.get("mouse_mode").is_none());
    assert!(legacy
        .get("env")
        .unwrap()
        .children()
        .unwrap()
        .get("A")
        .is_none());
}

#[test]
fn edits_to_multiple_options_accumulate_and_theme_edits_survive_theme_loading() {
    let changed = apply(
        &Config::default(),
        &[
            patch(&["mouse_mode"], json!(false)),
            patch(&["pane_frames"], json!(false)),
        ],
    )
    .unwrap();
    assert_eq!(changed.options.mouse_mode, Some(false));
    assert_eq!(changed.options.pane_frames, Some(false));
    let mut base = Config::default();
    base.themes.insert(
        "default".into(),
        crate::theme::Theme {
            palette: Default::default(),
            terminal: None,
            sourced_from_external_file: true,
        },
    );
    let mut changed = apply(
        &base,
        &[patch(
            &["themes", "default", "text_unselected", "base"],
            json!(7),
        )],
    )
    .unwrap();
    crate::config::apply_themes(&mut changed, None, None).unwrap();
    assert_eq!(
        changed
            .themes
            .get_theme("default")
            .unwrap()
            .palette
            .text_unselected
            .base,
        yinz_types::data::PaletteColor::EightBit(7)
    );
}

#[test]
fn effective_values_use_the_same_defaults_as_the_screen() {
    let document = effective_document(&Config::default()).unwrap();
    assert_eq!(
        find_node(&document, &["pane_animations".into()])
            .unwrap()
            .arguments,
        [json!(true)]
    );
    assert_eq!(
        find_node(&document, &["agent_pane_min_columns".into()])
            .unwrap()
            .arguments,
        [json!(80)]
    );
    assert_eq!(
        find_node(&document, &["focus_follows_mouse".into()])
            .unwrap()
            .arguments,
        [json!(false)]
    );
}

#[test]
fn theme_discovery_defers_color_fields_until_a_theme_is_opened() {
    let mut config = Config::default();
    for name in ["first", "second"] {
        config.themes.insert(
            name.into(),
            crate::theme::Theme {
                palette: Default::default(),
                terminal: None,
                sourced_from_external_file: true,
            },
        );
    }
    let summary = inspection_document(&config, None, false).unwrap();
    let names = find_node(&summary, &["themes".into()]).unwrap();
    assert_eq!(names.children.len(), 2);
    assert!(names.children.iter().all(|node| node.children.is_empty()));
    let detail = inspection_document(&config, Some("second"), false).unwrap();
    assert!(find_node(&detail, &["themes".into(), "first".into()])
        .unwrap()
        .children
        .is_empty());
    assert!(!find_node(&detail, &["themes".into(), "second".into()])
        .unwrap()
        .children
        .is_empty());
}

#[test]
fn directory_theme_edits_reload_and_preview_reports_directory_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let themes_dir = dir.path().join("themes");
    std::fs::create_dir(&themes_dir).unwrap();
    let mut definition = Config::default();
    definition.themes.insert(
        "custom".into(),
        crate::theme::Theme {
            palette: Default::default(),
            terminal: None,
            sourced_from_external_file: false,
        },
    );
    std::fs::write(
        themes_dir.join("custom.kdl"),
        definition.to_string(false).unwrap(),
    )
    .unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let (fragment, preview, underlay) = disk
        .preview(
            &[patch(
                &["themes", "custom", "text_unselected", "base"],
                json!(7),
            )],
            &[],
        )
        .unwrap();
    Config::try_write_editor_fragment(&disk.overlay, &fragment, underlay, || Ok(())).unwrap();
    let mut loaded = Config::load(None, Some(dir.path()), false).unwrap();
    assert_eq!(
        loaded
            .themes
            .get_theme("custom")
            .unwrap()
            .palette
            .text_unselected
            .base,
        yinz_types::data::PaletteColor::EightBit(7)
    );
    crate::config::apply_themes(&mut loaded, None, Some(dir.path().into())).unwrap();
    assert_eq!(loaded.themes, preview.themes);
    Config::write_managed_fragment(&disk.overlay, "mouse_mode false").unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .mouse_mode,
        Some(false)
    );
}

#[test]
fn overlay_validation_uses_the_selected_base_and_preceding_theme_definitions() {
    for source in ["base", "explicit", "overlay", "directory"] {
        let dir = tempfile::tempdir().unwrap();
        let overlay = dir.path().join("config.d");
        std::fs::create_dir(&overlay).unwrap();
        let explicit = dir.path().join("selected.kdl");
        let name = "custom";
        let mut definition = Config::default();
        definition.themes.insert(
            name.into(),
            crate::theme::Theme {
                palette: Default::default(),
                terminal: None,
                sourced_from_external_file: false,
            },
        );
        let fragment = patches_to_kdl(&[patch(
            &["themes", name, "text_unselected", "base"],
            json!(7),
        )])
        .unwrap()
        .to_string();
        // Create the edit first: validation must sort filenames, not use directory order.
        std::fs::write(overlay.join("20-edit.kdl"), &fragment).unwrap();
        let path = match source {
            "base" => Some(dir.path().join("config.kdl")),
            "explicit" => Some(explicit.clone()),
            "overlay" => Some(overlay.join("10-theme.kdl")),
            "directory" => {
                std::fs::create_dir(dir.path().join("themes")).unwrap();
                Some(dir.path().join("themes/custom.kdl"))
            },
            _ => None,
        };
        if let Some(path) = path {
            std::fs::write(path, definition.to_string(false).unwrap()).unwrap();
        }
        let config_path = (source == "explicit").then_some(explicit.as_path());
        Config::validate_plugin_overlays(config_path, Some(dir.path()))
            .unwrap_or_else(|error| panic!("{source}: {error}"));
        let loaded = Config::load(config_path, Some(dir.path()), false).unwrap();
        assert_eq!(
            loaded
                .themes
                .get_theme(name)
                .unwrap()
                .palette
                .text_unselected
                .base,
            yinz_types::data::PaletteColor::EightBit(7)
        );
        for malformed in [
            "mouse_mode 1",
            "configuration_edits {",
            "plugins { bad repo=1; }",
        ] {
            std::fs::write(overlay.join("30-broken.kdl"), malformed).unwrap();
            assert!(Config::validate_plugin_overlays(config_path, Some(dir.path())).is_err());
        }
        if source == "overlay" {
            std::fs::remove_file(overlay.join("30-broken.kdl")).unwrap();
            std::fs::rename(overlay.join("10-theme.kdl"), overlay.join("30-theme.kdl")).unwrap();
            assert!(Config::validate_plugin_overlays(config_path, Some(dir.path())).is_err());
        }
    }
}

#[test]
fn legacy_writes_supersede_matching_edits_and_keep_other_managed_entries() {
    let mut managed: KdlDocument = r#"configuration_edits {
        set "mouse_mode" { mouse_mode true; }
        set "env" { env { A "old"; B "keep"; }; }
        set "env" "A" { A "override"; }
    }"#
    .parse()
    .unwrap();
    crate::config::merge_kdl_fragment(
        &mut managed,
        &r#"mouse_mode false
env { A "latest"; }
"#
        .parse()
        .unwrap(),
    );
    let config = Config::from_kdl(&managed.to_string(), None).unwrap();
    assert_eq!(config.options.mouse_mode, Some(false));
    assert_eq!(
        config.env.inner().get("A").map(String::as_str),
        Some("latest")
    );
    assert_eq!(
        config.env.inner().get("B").map(String::as_str),
        Some("keep")
    );
}

#[test]
fn choice_catalog_accepts_every_value_and_covers_every_input_mode() {
    use strum::IntoEnumIterator;
    let catalog = catalog();
    let modes = &catalog
        .iter()
        .find(|setting| setting.name == "default_mode")
        .unwrap()
        .choices;
    for mode in yinz_types::data::InputMode::iter() {
        assert!(modes.contains(&format!("{mode:?}").to_lowercase().as_str()));
    }
    for setting in catalog.iter().filter(|s| s.kind == "choice") {
        for value in &setting.choices {
            apply(&Config::default(), &[patch(&[&setting.name], json!(value))])
                .unwrap_or_else(|error| panic!("{}={value}: {error}", setting.name));
        }
    }
}

#[test]
fn disk_revisions_include_theme_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    std::fs::create_dir(&themes).unwrap();
    std::fs::write(themes.join("custom.kdl"), "themes {}\n").unwrap();
    let before = Disk::read(None, Some(dir.path())).unwrap();
    std::fs::write(themes.join("custom.kdl"), "themes {}\n// changed\n").unwrap();
    let after = Disk::read(None, Some(dir.path())).unwrap();
    assert_ne!(before, after);
}

#[test]
fn editor_binding_paths_survive_becoming_shared() {
    let base = Config::from_kdl(
        "keybinds clear-defaults=true { normal { bind \"Ctrl a\" { Quit; }; }; locked { bind \"Ctrl a\" { Copy; }; }; }",
        None,
    ).unwrap();
    let path = vec![
        "keybinds".into(),
        "normal".into(),
        "bind:[\"Ctrl a\"]".into(),
    ];
    let mut binding = find_node(&document(&base).unwrap(), &path).unwrap().clone();
    binding.children[0].name = "Copy".into();
    let edit = Patch {
        path: path.clone(),
        node: Some(binding.clone()),
        reset: false,
    };
    let changed = apply(&base, std::slice::from_ref(&edit)).unwrap();
    let nodes = document(&changed).unwrap();
    assert_eq!(find_node(&nodes, &path), Some(&binding));
    let locked = vec!["keybinds".into(), "locked".into(), path[2].clone()];
    assert_eq!(find_node(&nodes, &locked), Some(&binding));
    let reconciled = apply(
        &base,
        &[Patch {
            path,
            node: find_node(&nodes, &edit.path).cloned(),
            reset: false,
        }],
    )
    .unwrap();
    assert_eq!(reconciled, changed);
    let fragment = patches_to_kdl(&[edit]).unwrap().to_string();
    assert_eq!(Config::from_kdl(&fragment, Some(base)).unwrap(), changed);
}

#[test]
fn removing_last_binding_preserves_empty_mode_blocks() {
    let base = Config::from_kdl(
        "keybinds clear-defaults=true { normal { bind \"Ctrl a\" { Quit; }; }; }",
        None,
    )
    .unwrap();
    let remove = Patch {
        path: vec![
            "keybinds".into(),
            "normal".into(),
            "bind:[\"Ctrl a\"]".into(),
        ],
        node: None,
        reset: false,
    };
    let changed = apply(&base, std::slice::from_ref(&remove)).unwrap();
    assert!(changed.keybinds.0[&yinz_types::data::InputMode::Normal].is_empty());
    let fragment = patches_to_kdl(&[remove]).unwrap().to_string();
    assert_eq!(
        Config::from_kdl(&fragment, Some(base.clone())).unwrap(),
        changed
    );
    for mode in ["normal", "shared", "shared_except", "shared_among"] {
        let node = Node {
            name: mode.into(),
            arguments: vec![],
            properties: Default::default(),
            children: vec![],
        };
        let edit = Patch {
            path: vec!["keybinds".into(), mode.into()],
            node: Some(node),
            reset: false,
        };
        let serialized = patches_to_kdl(&[edit]).unwrap();
        let set = &serialized.children().unwrap().nodes()[0];
        assert!(set.children().unwrap().nodes()[0].children().is_some());
        Config::from_kdl(&serialized.to_string(), Some(base.clone())).unwrap();
    }
}

#[test]
fn legacy_binding_fragments_preserve_siblings_and_override_ancestors() {
    for ancestor in [false, true] {
        let source = r#"keybinds clear-defaults=true {
            normal { bind "Ctrl a" { Quit; }; bind "Ctrl c" { Copy; }; }
            locked { bind "Ctrl a" { Quit; }; }
        }"#;
        let config = Config::from_kdl(source, None).unwrap();
        let nodes = document(&config).unwrap();
        let patches = if ancestor {
            vec![Patch {
                path: vec!["keybinds".into()],
                node: find_node(&nodes, &["keybinds".into()]).cloned(),
                reset: false,
            }]
        } else {
            ["normal", "locked"]
                .into_iter()
                .flat_map(|mode| {
                    find_node(&nodes, &["keybinds".into(), mode.into()])
                        .unwrap()
                        .children
                        .iter()
                        .map(move |binding| Patch {
                            path: vec!["keybinds".into(), mode.into(), node_key(binding)],
                            node: Some(binding.clone()),
                            reset: false,
                        })
                })
                .collect()
        };
        for (fragment, normal_a, locked_a, keep_c) in [
            (
                r#"keybinds { normal { bind "Ctrl b" { Copy; }; }; }"#,
                Some("Quit"),
                Some("Quit"),
                true,
            ),
            (
                r#"keybinds { normal { bind "Ctrl a" { Copy; }; }; }"#,
                Some("Copy"),
                Some("Quit"),
                true,
            ),
            (
                r#"keybinds { shared_among "Normal" "locked" { bind "Ctrl a" { Copy; }; }; }"#,
                Some("Copy"),
                Some("Copy"),
                true,
            ),
            (
                r#"keybinds { shared_except "locked" { unbind "Ctrl a"; }; }"#,
                None,
                Some("Quit"),
                true,
            ),
            (r#"keybinds { unbind "Ctrl a"; }"#, None, None, true),
            (
                r#"keybinds { normal clear-defaults=true { bind "Ctrl b" { Copy; }; }; }"#,
                None,
                Some("Quit"),
                false,
            ),
            (
                r#"keybinds clear-defaults=true { normal { bind "Ctrl b" { Copy; }; }; }"#,
                None,
                None,
                false,
            ),
        ] {
            let mut managed = KdlDocument::new();
            managed.nodes_mut().push(patches_to_kdl(&patches).unwrap());
            crate::config::merge_kdl_fragment(&mut managed, &fragment.parse().unwrap());
            let result = Config::from_kdl(&managed.to_string(), None).unwrap();
            let result = document(&result).unwrap();
            for (mode, expected) in [("normal", normal_a), ("locked", locked_a)] {
                let binding = find_node(
                    &result,
                    &["keybinds".into(), mode.into(), "bind:[\"Ctrl a\"]".into()],
                );
                assert_eq!(
                    binding.map(|n| n.children[0].name.as_str()),
                    expected,
                    "{ancestor}: {fragment}"
                );
            }
            assert_eq!(
                find_node(
                    &result,
                    &[
                        "keybinds".into(),
                        "normal".into(),
                        "bind:[\"Ctrl c\"]".into()
                    ]
                )
                .is_some(),
                keep_c,
                "{ancestor}: {fragment}"
            );
        }
    }
}

#[test]
fn binding_aliases_are_canonical_in_managed_edits_and_resets() {
    let mut managed: KdlDocument = r#"configuration_edits {
        set "keybinds" "normal" "bind:[\"ESC\"]" { bind "ESC" { Quit; }; }
    }"#
    .parse()
    .unwrap();
    let edits = patches_from_kdl(managed.get(EDITS_BLOCK).unwrap()).unwrap();
    assert_eq!(edits[0].path[2], "bind:[\"esc\"]");
    assert_eq!(
        edits[0].node.as_ref().unwrap().arguments,
        vec![json!("esc")]
    );
    let mut reset = edits[0].clone();
    reset.node = None;
    reset.reset = true;
    let mut fragment = KdlDocument::new();
    fragment.nodes_mut().push(patches_to_kdl(&[reset]).unwrap());
    crate::config::merge_kdl_fragment(&mut managed, &fragment);
    assert!(patches_from_kdl(managed.get(EDITS_BLOCK).unwrap())
        .unwrap()
        .is_empty());
    let mut invalid = edits[0].clone();
    invalid.path[2] = "bind:[\"not-a-key\"]".into();
    assert!(normalize_patch(&mut invalid).is_err());
}

#[test]
fn managed_binding_resets_expand_only_explicit_legacy_bindings() {
    use strum::IntoEnumIterator;
    use yinz_types::data::InputMode;
    let base = Config::from_kdl(
        r#"keybinds clear-defaults=true {
        shared { bind "Ctrl a" "Ctrl c" "Ctrl d" { Quit; }; }
    }"#,
        None,
    )
    .unwrap();
    for (source, clears_normal) in [
        (
            r#"keybinds { shared { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            false,
        ),
        (
            r#"keybinds { shared_among "Normal" "locked" { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            false,
        ),
        (
            r#"keybinds { shared_except "locked" { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            false,
        ),
        (
            r#"keybinds { shared "locked" { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            false,
        ),
        (
            r#"keybinds {
            normal { bind "Ctrl a" "Ctrl b" { ToggleFocusFullscreen; }; }
            shared { unbind "Ctrl c"; bind "Ctrl a" "Ctrl b" "Ctrl c" { Copy; }; }
            shared_among "normal" { bind "Ctrl c" { Copy; }; }
            unbind "Ctrl d";
            unbind "Ctrl c";
        }"#,
            false,
        ),
        (
            r#"keybinds { shared { bind "Ctrl a" "Ctrl b" { Copy; }; }; normal clear-defaults=true { bind "Ctrl a" "Ctrl c" { Copy; }; }; }"#,
            true,
        ),
        (
            r#"keybinds clear-defaults=true { shared { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            true,
        ),
        (
            r#"keybinds { normal { bind "Ctrl a" "Ctrl b" { Copy; }; }; }"#,
            false,
        ),
    ] {
        let mut managed: KdlDocument = source.parse().unwrap();
        let mut expected = Config::from_kdl(source, Some(base.clone())).unwrap();
        let key = "Ctrl a".parse().unwrap();
        let normal = expected.keybinds.0.get_mut(&InputMode::Normal).unwrap();
        if clears_normal {
            normal.remove(&key);
        } else {
            normal.insert(
                key.clone(),
                base.keybinds.0[&InputMode::Normal][&key].clone(),
            );
        }
        crate::config::merge_kdl_fragment(
            &mut managed,
            &r#"configuration_edits {
            reset "keybinds" "normal" "bind:[\"Ctrl a\"]";
        }"#
            .parse()
            .unwrap(),
        );
        let actual = Config::from_kdl(&managed.to_string(), Some(base.clone())).unwrap();
        for mode in InputMode::iter() {
            assert_eq!(
                actual.keybinds.0.get(&mode),
                expected.keybinds.0.get(&mode),
                "{mode:?}: {source}"
            );
        }
        let alone = Config::from_kdl(&managed.to_string(), None).unwrap();
        assert!(
            !alone
                .keybinds
                .0
                .get(&InputMode::Normal)
                .is_some_and(|keys| keys.contains_key(&key)),
            "reset copied an inherited binding: {source}"
        );
    }
}

#[test]
fn shared_binding_saves_keep_their_precedence_after_repeated_resets() {
    use yinz_types::{data::InputMode, input::actions::Action};

    for shared in [
        "shared",
        "shared_among \"Normal\" \"locked\"",
        "shared_except \"pane\"",
        "shared \"pane\"",
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            r#"keybinds clear-defaults=true {
                shared { bind "Ctrl a" { NoOp; }; }
            }"#,
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        Config::write_managed_fragment(
            &overlay,
            &format!(
                r#"keybinds {{
                    normal {{ bind "Ctrl a" {{ ToggleFocusFullscreen; }}; }}
                    {shared} {{ bind "Ctrl a" "Ctrl b" {{ Copy; }}; }}
                    shared_among "normal" {{ bind "Ctrl c" {{ Quit; }}; }}
                    shared_among "normal" "pane" {{ bind "Ctrl c" {{ Copy; }}; }}
                    locked {{ bind "Ctrl b" {{ ToggleFocusFullscreen; }}; }}
                }}"#
            ),
        )
        .unwrap();
        for _ in 0..2 {
            Config::write_managed_fragment(
                &overlay,
                r#"configuration_edits { reset "keybinds" "normal" "bind:[\"Ctrl a\"]"; }"#,
            )
            .unwrap();
        }
        Config::write_managed_fragment(
            &overlay,
            &format!(r#"keybinds {{ {shared} {{ bind "Ctrl b" {{ Quit; }}; }}; }}"#),
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        let normal = &loaded.keybinds.0[&InputMode::Normal];
        assert_eq!(
            normal[&"Ctrl a".parse().unwrap()],
            vec![Action::NoOp],
            "{shared}"
        );
        assert_eq!(
            normal[&"Ctrl b".parse().unwrap()],
            vec![Action::Quit],
            "{shared}"
        );
        assert_eq!(
            normal[&"Ctrl c".parse().unwrap()],
            vec![Action::Copy],
            "{shared}"
        );
        assert_eq!(
            loaded.keybinds.0[&InputMode::Locked][&"Ctrl b".parse().unwrap()],
            vec![Action::ToggleFocusFullscreen],
            "{shared}"
        );
    }
}

#[test]
fn resetting_shared_binding_preserves_identity_on_later_save() {
    use yinz_types::{data::InputMode, input::actions::Action};

    for shared in [
        "shared",
        "shared_among \"Normal\" \"locked\"",
        "shared_except \"pane\"",
        "shared \"pane\"",
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            r#"keybinds clear-defaults=true { shared { bind "Ctrl a" { NoOp; }; }; }"#,
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        Config::write_managed_fragment(
            &overlay,
            &format!(
                r#"keybinds {{
                    {shared} {{ bind "Ctrl a" "Ctrl b" {{ Copy; }}; }}
                    shared_among "normal" {{ bind "Ctrl c" {{ Quit; }}; }}
                    locked {{ bind "Ctrl b" {{ ToggleFocusFullscreen; }}; }}
                }}"#,
            ),
        )
        .unwrap();
        for _ in 0..2 {
            let disk = Disk::read(None, Some(dir.path())).unwrap();
            let (fragment, expected, underlay) = disk
                .preview(
                    &[],
                    &[vec![
                        "keybinds".into(),
                        "normal".into(),
                        "bind:[\"Ctrl a\"]".into(),
                    ]],
                )
                .unwrap();
            Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
            let loaded = Config::load(None, Some(dir.path()), false).unwrap();
            assert_eq!(loaded.keybinds, expected.keybinds, "{shared}");
            assert_eq!(
                loaded.keybinds.0[&InputMode::Normal][&"Ctrl a".parse().unwrap()],
                vec![Action::NoOp]
            );
        }
        let mut expected = Config::load(None, Some(dir.path()), false).unwrap();
        expected
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .insert("Ctrl c".parse().unwrap(), vec![Action::NoOp]);
        Config::write_managed_fragment(
            &overlay,
            r#"keybinds { shared_among "normal" { bind "Ctrl c" { NoOp; }; }; }"#,
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, expected.keybinds, "{shared}");
        assert_eq!(
            loaded.keybinds.0[&InputMode::Normal][&"Ctrl b".parse().unwrap()],
            vec![Action::Copy]
        );
        assert_eq!(
            loaded.keybinds.0[&InputMode::Locked][&"Ctrl a".parse().unwrap()],
            vec![Action::Copy]
        );
    }
}

#[test]
fn shared_reset_splits_do_not_collide_with_legacy_save_identities() {
    use strum::IntoEnumIterator;
    use yinz_types::{data::InputMode, input::actions::Action};

    for (source, collision, nested) in [
        ("shared", "shared \"normal\"", false),
        (
            "shared_except \"pane\"",
            "shared_except \"pane\" \"normal\"",
            false,
        ),
        (
            "shared_among \"Normal\" \"locked\"",
            "shared_among \"locked\"",
            false,
        ),
        ("shared", "shared_among \"normal\"", true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            "keybinds clear-defaults=true { shared { bind \"Ctrl a\" { NoOp; }; }; }",
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        Config::write_managed_fragment(
            &overlay,
            &format!(
                r#"keybinds {{
                    {source} {{ bind "Ctrl a" {{ Copy; Quit; }}; }}
                    {collision} {{ bind "Ctrl b" {{ Quit; }}; }}
                }}"#
            ),
        )
        .unwrap();
        let original = Config::load(None, Some(dir.path()), false).unwrap();
        let mut expected = original.clone();
        expected
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .insert(
                "Ctrl a".parse().unwrap(),
                if nested {
                    vec![Action::Quit]
                } else {
                    vec![Action::NoOp]
                },
            );
        for _ in 0..2 {
            let mut path = vec![
                "keybinds".into(),
                "normal".into(),
                "bind:[\"Ctrl a\"]".into(),
            ];
            if nested {
                path.push("Copy".into());
            }
            let disk = Disk::read(None, Some(dir.path())).unwrap();
            let (fragment, preview, underlay) = disk.preview(&[], &[path]).unwrap();
            Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
            let loaded = Config::load(None, Some(dir.path()), false).unwrap();
            assert_eq!(loaded.keybinds, preview.keybinds, "{source}");
            assert_eq!(loaded.keybinds, expected.keybinds, "{source}");
        }
        // Splitting an already split block must retain the first source identity.
        expected
            .keybinds
            .0
            .get_mut(&InputMode::Locked)
            .unwrap()
            .insert("Ctrl a".parse().unwrap(), vec![Action::NoOp]);
        Config::write_managed_fragment(
            &overlay,
            r#"configuration_edits { reset "keybinds" "locked" "bind:[\"Ctrl a\"]"; }"#,
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, expected.keybinds, "{source}");
        for bindings in expected.keybinds.0.values_mut() {
            if let Some(actions) = bindings.get_mut(&"Ctrl b".parse().unwrap()) {
                *actions = vec![Action::NoOp];
            }
        }
        Config::write_managed_fragment(
            &overlay,
            &format!(r#"keybinds {{ {collision} {{ bind "Ctrl b" {{ NoOp; }}; }}; }}"#),
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, expected.keybinds, "{source}");

        // Replacing the source must replace all its split pieces, too.
        for mode in InputMode::iter() {
            if original.keybinds.0[&mode][&"Ctrl a".parse().unwrap()]
                == vec![Action::Copy, Action::Quit]
            {
                expected.keybinds.0.get_mut(&mode).unwrap().insert(
                    "Ctrl a".parse().unwrap(),
                    vec![Action::ToggleFocusFullscreen],
                );
            }
        }
        Config::write_managed_fragment(
            &overlay,
            &format!(
                r#"keybinds {{ {source} {{ bind "Ctrl a" {{ ToggleFocusFullscreen; }}; }}; }}"#
            ),
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, expected.keybinds, "{source}");
    }
}

#[test]
fn invalid_shared_source_metadata_is_rejected_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = Config::write_managed_fragment(dir.path(), "mouse_mode false").unwrap();
    let before = std::fs::read(&path).unwrap();
    for source in [
        Value::Bool(true),
        Value::from("invalid json"),
        Value::from(r#"["normal", []]"#),
        Value::from(r#"["shared", ["invalid-mode"]]"#),
        Value::from(r#"["shared", [42]]"#),
    ] {
        let node = Node {
            name: "shared".into(),
            arguments: vec![],
            properties: [(KEYBIND_BLOCK_SOURCE.into(), source)].into(),
            children: vec![],
        };
        let block = to_kdl(&node, 0, Some("keybinds")).unwrap();
        let fragment = format!("keybinds {{\n{block}\n}}\n");
        let error = Config::from_kdl(&fragment, None).unwrap_err();
        assert!(
            format!("{error:?}").contains("Invalid configuration-editor-source"),
            "{error:?}"
        );
        assert!(Config::write_managed_fragment(dir.path(), &fragment).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn shared_resets_preserve_unbind_order_and_nested_actions() {
    use strum::IntoEnumIterator;
    use yinz_types::{data::InputMode, input::actions::Action};

    for (suffix, normal_actions) in [
        (r#""bind:[\"Ctrl b\"]""#, vec![]),
        (r#""bind:[\"Ctrl b\"]" "Copy""#, vec![Action::Quit]),
        (r#""unbind:[\"Ctrl a\"]""#, vec![Action::Copy, Action::Quit]),
        ("", vec![]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            "keybinds clear-defaults=true {}",
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        Config::write_managed_fragment(
            &overlay,
            r#"keybinds { shared {
                unbind "Ctrl a";
                bind "Ctrl a" "Ctrl b" { Copy; Quit; }
            }; }"#,
        )
        .unwrap();
        let mut expected = Config::load(None, Some(dir.path()), false).unwrap();
        let normal = expected.keybinds.0.get_mut(&InputMode::Normal).unwrap();
        if suffix.is_empty() {
            normal.clear();
        } else {
            let key = if suffix.contains("unbind") {
                "Ctrl a"
            } else {
                "Ctrl b"
            }
            .parse()
            .unwrap();
            if normal_actions.is_empty() {
                normal.remove(&key);
            } else {
                normal.insert(key, normal_actions);
            }
        }
        for _ in 0..2 {
            Config::write_managed_fragment(
                &overlay,
                &format!(r#"configuration_edits {{ reset "keybinds" "normal" {suffix}; }}"#),
            )
            .unwrap();
            let loaded = Config::load(None, Some(dir.path()), false).unwrap();
            for mode in InputMode::iter() {
                assert_eq!(
                    loaded.keybinds.0.get(&mode).cloned().unwrap_or_default(),
                    expected.keybinds.0.get(&mode).cloned().unwrap_or_default(),
                    "{suffix}: {mode:?}"
                );
            }
        }
    }
}

#[test]
fn binding_reset_uses_configured_layout_directory() {
    use yinz_types::{data::InputMode, input::actions::Action};

    let dir = tempfile::tempdir().unwrap();
    let layouts = tempfile::tempdir().unwrap();
    std::fs::write(layouts.path().join("custom.kdl"), "layout { pane; }").unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        format!(
            "layout_dir {:?}\nkeybinds clear-defaults=true {{}}",
            layouts.path()
        ),
    )
    .unwrap();
    let overlay = dir.path().join("config.d");
    std::fs::create_dir(&overlay).unwrap();
    std::fs::write(
        overlay.join(crate::config::MANAGED_CONFIG_FILE_NAME),
        r#"keybinds { normal {
            bind "Ctrl a" { NewTab { layout "custom"; }; }
            bind "Ctrl b" { Copy; }
        }; }"#,
    )
    .unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let (fragment, expected, underlay) = disk
        .preview(
            &[],
            &[vec![
                "keybinds".into(),
                "normal".into(),
                "bind:[\"Ctrl b\"]".into(),
            ]],
        )
        .unwrap();
    Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
    let loaded = Config::load(None, Some(dir.path()), false).unwrap();
    assert_eq!(loaded.keybinds, expected.keybinds);
    let bindings = &loaded.keybinds.0[&InputMode::Normal];
    assert!(!bindings.contains_key(&"Ctrl b".parse().unwrap()));
    assert!(matches!(
        bindings[&"Ctrl a".parse().unwrap()].as_slice(),
        [Action::NewTab { .. }]
    ));
}

#[test]
fn binding_reset_preserves_empty_action_blocks() {
    use yinz_types::data::InputMode;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        "keybinds clear-defaults=true {}",
    )
    .unwrap();
    let overlay = dir.path().join("config.d");
    Config::write_managed_fragment(
        &overlay,
        r#"keybinds { normal { bind "Ctrl a" {}; bind "Ctrl b" { Copy; }; }; }"#,
    )
    .unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let (fragment, expected, underlay) = disk
        .preview(
            &[],
            &[vec![
                "keybinds".into(),
                "normal".into(),
                "bind:[\"Ctrl b\"]".into(),
            ]],
        )
        .unwrap();
    Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
    let loaded = Config::load(None, Some(dir.path()), false).unwrap();
    assert_eq!(loaded.keybinds, expected.keybinds);
    let bindings = &loaded.keybinds.0[&InputMode::Normal];
    assert!(!bindings.contains_key(&"Ctrl b".parse().unwrap()));
    assert!(bindings[&"Ctrl a".parse().unwrap()].is_empty());
}

#[test]
fn binding_resets_leave_malformed_configuration_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let overlay = dir.path().join("config.d");
    std::fs::create_dir(&overlay).unwrap();
    let path = overlay.join(crate::config::MANAGED_CONFIG_FILE_NAME);
    for source in [
        "keybinds",
        "keybinds { normal; }",
        "keybinds { unknown {}; }",
        "keybinds { shared_among \"unknown\" {}; }",
        "keybinds { normal { bind \"Ctrl a\"; }; }",
        "keybinds { normal { bind \"not-a-key\" {}; }; }",
        "keybinds { normal { unknown; }; }",
        "keybinds { normal { bind \"Ctrl b\" { UnknownAction; }; }; }",
    ] {
        std::fs::write(&path, source).unwrap();
        let disk = Disk::read(None, Some(dir.path())).unwrap();
        assert!(
            disk.preview(
                &[],
                &[vec![
                    "keybinds".into(),
                    "normal".into(),
                    "bind:[\"Ctrl a\"]".into()
                ]]
            )
            .is_err(),
            "{source}"
        );
        assert!(
            Config::write_managed_fragment(
                &overlay,
                r#"configuration_edits { reset "keybinds" "normal" "bind:[\"Ctrl a\"]"; }"#,
            )
            .is_err(),
            "{source}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }
}

#[test]
fn raw_grouped_resets_remove_each_stored_edit_and_restore_inherited_bindings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        r#"keybinds clear-defaults=true {
            normal { bind "esc" "Ctrl a" { Copy; }; }
        }"#,
    )
    .unwrap();
    let inherited = Config::load(None, Some(dir.path()), false).unwrap();
    let overlay = dir.path().join("config.d");
    let path = Config::write_managed_fragment(
        &overlay,
        r#"configuration_edits {
            set "keybinds" "normal" "bind:[\"ESC\",\"Ctrl a\"]" {
                bind "ESC" "Ctrl a" { Quit; }
            }
            set "mouse_mode" { mouse_mode false; }
        }"#,
    )
    .unwrap();
    let stored: KdlDocument = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(
        patches_from_kdl(stored.get(EDITS_BLOCK).unwrap())
            .unwrap()
            .len(),
        3
    );
    for _ in 0..2 {
        Config::write_managed_fragment(
            &overlay,
            r#"configuration_edits {
                reset "keybinds" "normal" "bind:[\"esc\",\"Ctrl a\"]";
            }"#,
        )
        .unwrap();
        let stored: KdlDocument = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let edits = patches_from_kdl(stored.get(EDITS_BLOCK).unwrap()).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, vec!["mouse_mode"]);
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, inherited.keybinds);
        assert_eq!(loaded.options.mouse_mode, Some(false));
    }
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(Config::write_managed_fragment(
        &overlay,
        r#"configuration_edits { reset "keybinds" "normal" "bind:[\"not-a-key\"]"; }"#,
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn persisted_resets_preserve_unrelated_kdl_subtrees() {
    use yinz_types::data::InputMode;

    for block in ["normal", "shared", "shared_among \"normal\" \"locked\""] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            "keybinds clear-defaults=true {}",
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        let path = Config::write_managed_fragment(
            &overlay,
            &format!(r#"keybinds {{
                {block} {{
                    bind "Ctrl a" "Ctrl b" {{ Copy; }}
                    bind "Ctrl c" {{ LaunchOrFocusPlugin "foo" {{ custom {{}}; }}; }}
                    bind "Ctrl d" {{ NewTab {{}}; }}
                }}
                pane {{ bind "Ctrl e" {{ LaunchOrFocusPlugin "foo" {{ custom {{}}; }}; NewTab {{}}; }}; }}
            }}"#),
        ).unwrap();
        let before = Config::load(None, Some(dir.path()), false).unwrap();
        let disk = Disk::read(None, Some(dir.path())).unwrap();
        let (fragment, preview, underlay) = disk
            .preview(
                &[],
                &[vec![
                    "keybinds".into(),
                    "normal".into(),
                    "bind:[\"Ctrl a\"]".into(),
                ]],
            )
            .unwrap();
        Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(loaded.keybinds, preview.keybinds);
        for (mode, keys) in [
            (InputMode::Normal, vec!["Ctrl b", "Ctrl c", "Ctrl d"]),
            (InputMode::Pane, vec!["Ctrl e"]),
        ] {
            for key in keys {
                assert_eq!(
                    loaded.keybinds.0[&mode][&key.parse().unwrap()],
                    before.keybinds.0[&mode][&key.parse().unwrap()]
                );
            }
        }
        let persisted: KdlDocument = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        fn check_empty_blocks(node: &KdlNode) -> usize {
            if matches!(node.name().value(), "custom" | "NewTab") {
                assert!(node.children().unwrap().nodes().is_empty());
                return 1;
            }
            node.children()
                .into_iter()
                .flat_map(|children| children.nodes())
                .map(check_empty_blocks)
                .sum()
        }
        assert_eq!(check_empty_blocks(persisted.get("keybinds").unwrap()), 4);
    }
}

#[test]
fn global_unbind_resets_keep_identity_and_precedence_across_legacy_saves() {
    use strum::IntoEnumIterator;
    use yinz_types::{data::InputMode, input::actions::Action};

    for binding_only in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.kdl"),
            r#"keybinds clear-defaults=true {
            shared { bind "Ctrl a" "Ctrl c" { NoOp; }; }
        }"#,
        )
        .unwrap();
        let overlay = dir.path().join("config.d");
        let path = Config::write_managed_fragment(
            &overlay,
            r#"keybinds {
            unbind "Ctrl a";
            unbind "Ctrl c";
        }"#,
        )
        .unwrap();
        let mut reset = vec!["keybinds".into(), "normal".into()];
        if binding_only {
            reset.push("unbind:[\"Ctrl a\"]".into());
        }
        for _ in 0..2 {
            let disk = Disk::read(None, Some(dir.path())).unwrap();
            let (fragment, preview, underlay) = disk.preview(&[], &[reset.clone()]).unwrap();
            Config::try_write_editor_fragment(&overlay, &fragment, underlay, || Ok(())).unwrap();
            assert_eq!(
                Config::load(None, Some(dir.path()), false)
                    .unwrap()
                    .keybinds,
                preview.keybinds
            );
        }
        for binding in [
            r#"bind "Ctrl b" { Copy; }"#,
            r#"bind "Ctrl a" { Quit; }; bind "Ctrl b" { Copy; }"#,
        ] {
            Config::write_managed_fragment(
                &overlay,
                &format!("keybinds {{ locked {{ {binding}; }}; }}"),
            )
            .unwrap();
            let loaded = Config::load(None, Some(dir.path()), false).unwrap();
            for mode in InputMode::iter() {
                let bindings = &loaded.keybinds.0[&mode];
                assert_eq!(
                    bindings.contains_key(&"Ctrl a".parse().unwrap()),
                    mode == InputMode::Normal
                );
                assert_eq!(bindings[&"Ctrl c".parse().unwrap()], vec![Action::NoOp]);
            }
            assert_eq!(
                loaded.keybinds.0[&InputMode::Locked][&"Ctrl b".parse().unwrap()],
                vec![Action::Copy]
            );
        }
        // Reconciliation of an ancestor editor edit must use the same ordering as loading.
        let persisted: KdlDocument = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let edits: KdlDocument = r#"configuration_edits { set "keybinds" { keybinds {}; }; }"#
            .parse()
            .unwrap();
        let reconciled = reconcile_legacy_keybinds(
            edits.get(EDITS_BLOCK).unwrap(),
            persisted.get("keybinds").unwrap(),
        )
        .unwrap();
        let reconciled = Config::from_kdl(&reconciled.to_string(), None).unwrap();
        assert!(!reconciled.keybinds.0[&InputMode::Locked].contains_key(&"Ctrl a".parse().unwrap()));
        assert_eq!(
            reconciled.keybinds.0[&InputMode::Locked][&"Ctrl b".parse().unwrap()],
            vec![Action::Copy]
        );

        // Resetting another generated descendant must retain the original source identity.
        Config::write_managed_fragment(
            &overlay,
            r#"configuration_edits { reset "keybinds" "pane" "unbind:[\"Ctrl a\"]"; }"#,
        )
        .unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(
            loaded.keybinds.0[&InputMode::Pane][&"Ctrl a".parse().unwrap()],
            vec![Action::NoOp]
        );
        Config::write_managed_fragment(&overlay, r#"keybinds { unbind "Ctrl a"; }"#).unwrap();
        let loaded = Config::load(None, Some(dir.path()), false).unwrap();
        for bindings in loaded.keybinds.0.values() {
            assert!(!bindings.contains_key(&"Ctrl a".parse().unwrap()));
        }
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains(KEYBIND_BLOCK_SOURCE));
    }
}

#[test]
fn invalid_global_unbind_source_metadata_is_rejected_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = Config::write_managed_fragment(dir.path(), "mouse_mode false").unwrap();
    let before = std::fs::read(&path).unwrap();
    for block in [
        r##"normal configuration-editor-source=true { unbind "Ctrl a"; }"##,
        r##"normal configuration-editor-source=r#"["unbind", ["invalid-key"]]"# { unbind "Ctrl a"; }"##,
        r##"normal configuration-editor-source=r#"["unbind", [42]]"# { unbind "Ctrl a"; }"##,
        r##"normal configuration-editor-source=r#"["unbind", ["Ctrl a"]]"# { bind "Ctrl a" { Quit; }; }"##,
        r##"normal clear-defaults=true configuration-editor-source=r#"["unbind", ["Ctrl a"]]"# { unbind "Ctrl a"; }"##,
        r##"normal configuration-editor-source=r#"["unbind", ["Ctrl a"]]"#"##,
        r##"shared configuration-editor-source=r#"["unbind", ["Ctrl a"]]"# { unbind "Ctrl a"; }"##,
    ] {
        let fragment = format!("keybinds {{ {block}; }}");
        let error = Config::from_kdl(&fragment, None).unwrap_err();
        assert!(
            format!("{error:?}").contains("Invalid configuration-editor-source"),
            "{error:?}"
        );
        assert!(Config::write_managed_fragment(dir.path(), &fragment).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
