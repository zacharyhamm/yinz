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
            let line = line.strip_prefix('"')?;
            Some(line.split('"').next().unwrap())
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
