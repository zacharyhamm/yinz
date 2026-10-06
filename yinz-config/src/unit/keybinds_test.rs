use crate::config::Config;
use insta::assert_snapshot;
use strum::IntoEnumIterator;
use yinz_types::data::InputMode;
use yinz_types::geometry::Direction;
use yinz_types::input::actions::*;
use yinz_types::keys::{BareKey, KeyWithModifier};

#[test]
fn default_command_palette_binding_is_shared_by_every_mode() {
    let config = Config::from_default_assets().unwrap();
    for mode in InputMode::iter() {
        assert_eq!(
            config
                .keybinds
                .get_actions_for_key_in_mode(&mode, &"Ctrl Alt p".parse().unwrap()),
            Some(&vec![Action::ToggleCommandPalette]),
            "{mode:?}"
        );
    }
    for mode in [InputMode::Normal, InputMode::Locked] {
        assert_eq!(
            config
                .keybinds
                .get_actions_for_key_in_mode(&mode, &"Ctrl d".parse().unwrap()),
            None,
            "{mode:?}"
        );
    }
    assert_eq!(
        config
            .keybinds
            .get_actions_for_key_in_mode(&InputMode::Locked, &"Ctrl q".parse().unwrap()),
        None
    );
}

#[test]
fn default_command_palette_binding_can_be_overridden() {
    for (binding, expected) in [
        (r#"unbind "Ctrl Alt p";"#, None),
        (
            r#"bind "Ctrl Alt p" { WriteChars "custom"; }"#,
            Some(vec![Action::WriteChars {
                chars: "custom".into(),
            }]),
        ),
    ] {
        let config = Config::from_kdl(
            &format!("keybinds {{\n shared {{\n {binding}\n }}\n}}"),
            Some(Config::from_default_assets().unwrap()),
        )
        .unwrap();
        for mode in InputMode::iter() {
            assert_eq!(
                config
                    .keybinds
                    .get_actions_for_key_in_mode(&mode, &"Ctrl Alt p".parse().unwrap()),
                expected.as_ref(),
                "{mode:?}"
            );
        }
    }
}

#[test]
fn can_define_shared_keybinds_for_all_modes() {
    let config_contents = r#"
        keybinds {
            shared {
                bind "Ctrl g" { SwitchToMode "Locked"; }
            }
        }
    "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    for mode in InputMode::iter() {
        let action_in_mode = config.keybinds.get_actions_for_key_in_mode(
            &mode,
            &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier(),
        );
        assert_eq!(
            action_in_mode,
            Some(&vec![Action::SwitchToMode {
                input_mode: InputMode::Locked
            }]),
            "Keybind bound in mode"
        );
    }
}

#[test]
fn can_define_shared_keybinds_with_exclusion() {
    let config_contents = r#"
        keybinds {
            shared_except "locked" {
                bind "Ctrl g" { SwitchToMode "Locked"; }
            }
        }
    "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    for mode in InputMode::iter() {
        let action_in_mode = config.keybinds.get_actions_for_key_in_mode(
            &mode,
            &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier(),
        );
        if mode == InputMode::Locked {
            assert_eq!(action_in_mode, None, "Keybind unbound in excluded mode");
        } else {
            assert_eq!(
                action_in_mode,
                Some(&vec![Action::SwitchToMode {
                    input_mode: InputMode::Locked
                }]),
                "Keybind bound in mode"
            );
        }
    }
}

#[test]
fn select_mode_does_not_inherit_default_focus_bindings() {
    let config = Config::from_default_assets().unwrap();
    for bare_key in [
        BareKey::Char('h'),
        BareKey::Char('j'),
        BareKey::Char('k'),
        BareKey::Char('l'),
        BareKey::Char('i'),
        BareKey::Char('o'),
        BareKey::Left,
        BareKey::Down,
        BareKey::Up,
        BareKey::Right,
    ] {
        assert_eq!(
            config.keybinds.get_actions_for_key_in_mode(
                &InputMode::Select,
                &KeyWithModifier::new(bare_key).with_alt_modifier(),
            ),
            None,
        );
    }
}

#[test]
fn can_define_shared_keybinds_with_inclusion() {
    let config_contents = r#"
        keybinds {
            shared_among "normal" "resize" "pane" {
                bind "Ctrl g" { SwitchToMode "Locked"; }
            }
        }
    "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    for mode in InputMode::iter() {
        let action_in_mode = config.keybinds.get_actions_for_key_in_mode(
            &mode,
            &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier(),
        );
        if mode == InputMode::Normal || mode == InputMode::Resize || mode == InputMode::Pane {
            assert_eq!(
                action_in_mode,
                Some(&vec![Action::SwitchToMode {
                    input_mode: InputMode::Locked
                }]),
                "Keybind bound in included mode"
            );
        } else {
            assert_eq!(action_in_mode, None, "Keybind unbound in other modes");
        }
    }
}

#[test]
fn error_received_on_unknown_input_mode() {
    let config_contents = r#"
        keybinds {
            i_do_not_exist {
                bind "z" { SwitchToMode "Resize"; }
            }
        }
    "#;
    let config_error = Config::from_kdl(config_contents, None).unwrap_err();
    assert_snapshot!(format!("{:?}", config_error));
}

#[test]
fn error_received_on_unknown_key_instruction() {
    let config_contents = r#"
        keybinds {
            pane {
                i_am_not_bind_or_unbind
                bind "z" { SwitchToMode "Resize"; }
            }
        }
    "#;
    let config_error = Config::from_kdl(config_contents, None).unwrap_err();
    assert_snapshot!(format!("{:?}", config_error));
}

mod props {
    use super::*;
    use hegel::generators;
    use std::collections::HashMap;
    use {yinz_types::data::InputMode, yinz_types::data::KeyModifier};

    /// The actions a generated `bind` may use, paired with their KDL spelling.
    ///
    /// Deliberately a small fixed table rather than an arbitrary `Action`: we render the
    /// KDL ourselves, so we can only emit actions whose spelling we write here. Parsing
    /// arbitrary actions is what the `input/actions.rs` properties cover.
    fn action_table() -> Vec<(&'static str, Action)> {
        vec![
            (
                "SwitchToMode \"Locked\";",
                Action::SwitchToMode {
                    input_mode: InputMode::Locked,
                },
            ),
            (
                "SwitchToMode \"Resize\";",
                Action::SwitchToMode {
                    input_mode: InputMode::Resize,
                },
            ),
            ("TogglePaneFrames;", Action::TogglePaneFrames),
            ("ToggleCommandPalette;", Action::ToggleCommandPalette),
            ("ToggleFocusFullscreen;", Action::ToggleFocusFullscreen),
            (
                "MoveFocusOrTab \"Left\";",
                Action::MoveFocusOrTab {
                    direction: Direction::Left,
                },
            ),
            ("Quit;", Action::Quit),
        ]
    }

    /// A deliberately tiny key pool, so that generated binds and unbinds collide often
    /// enough to exercise override/unbind/clear-defaults rather than each landing on its
    /// own key. The `Char` arm keeps to characters KDL needs no escaping for — escaping
    /// is not what this property is about — and includes an upper/lower pair so the
    /// case-folding in `KeyWithModifier`'s `Eq`/`Hash` is exercised.
    #[hegel::composite]
    fn any_key(tc: &hegel::TestCase) -> KeyWithModifier {
        let bare_key = tc.draw(generators::sampled_from(vec![
            BareKey::Char('a'),
            BareKey::Char('z'),
            BareKey::Char('Z'),
            BareKey::Char('1'),
            BareKey::Esc,
            BareKey::Enter,
            BareKey::Left,
            BareKey::F(1),
        ]));
        let key_modifiers = tc
            .draw(generators::vecs(generators::sampled_from(vec![
                KeyModifier::Ctrl,
                KeyModifier::Alt,
                KeyModifier::Shift,
            ])))
            .into_iter()
            .collect();
        KeyWithModifier {
            bare_key,
            key_modifiers,
        }
    }

    #[hegel::composite]
    fn any_keys(tc: &hegel::TestCase) -> Vec<KeyWithModifier> {
        // capped for allocation sanity only; a single key stays reachable
        tc.draw(generators::vecs(any_key()).min_size(1).max_size(3))
    }

    #[derive(Debug, Clone)]
    enum Op {
        Bind {
            keys: Vec<KeyWithModifier>,
            actions: Vec<usize>,
        },
        Unbind {
            keys: Vec<KeyWithModifier>,
        },
    }

    #[derive(Debug, Clone)]
    struct ModeBlock {
        mode: InputMode,
        clear_defaults: bool,
        ops: Vec<Op>,
    }

    #[derive(Debug, Clone)]
    struct KeybindsBlock {
        clear_defaults: bool,
        modes: Vec<ModeBlock>,
        global_unbind: Vec<KeyWithModifier>,
    }

    #[hegel::composite]
    fn any_op(tc: &hegel::TestCase) -> Op {
        if tc.draw(generators::booleans()) {
            let keys = tc.draw(any_keys());
            let actions = tc.draw(
                generators::vecs(
                    generators::integers::<usize>().max_value(action_table().len() - 1),
                )
                .min_size(1)
                .max_size(3),
            );
            Op::Bind { keys, actions }
        } else {
            Op::Unbind {
                keys: tc.draw(any_keys()),
            }
        }
    }

    /// One `keybinds` block. Each mode appears at most once: a mode repeated in the same
    /// block shares one map, which is a KDL authoring quirk rather than a keybind law.
    #[hegel::composite]
    fn any_keybinds_block(tc: &hegel::TestCase) -> KeybindsBlock {
        let mut modes = vec![];
        let mut used_modes = vec![];
        // only two modes, so blocks in the base and user configs land on the same mode
        // often enough to exercise overriding and per-mode clear-defaults. The assertions
        // still sweep every `InputMode`, so the untouched ones are checked to be empty.
        let mode_pool = vec![InputMode::Normal, InputMode::Pane];
        for _ in 0..tc.draw(generators::integers::<usize>().max_value(3)) {
            let mode = tc.draw(generators::sampled_from(mode_pool.clone()));
            if used_modes.contains(&mode) {
                continue;
            }
            used_modes.push(mode);
            modes.push(ModeBlock {
                mode,
                clear_defaults: tc.draw(generators::booleans()),
                // a mode block with no children is a parse error, so never empty
                ops: tc.draw(generators::vecs(any_op()).min_size(1).max_size(3)),
            });
        }
        KeybindsBlock {
            // biased away from true: a block-level clear wipes the base config, which
            // hides the per-mode clear-defaults and override rules behind it
            clear_defaults: tc.draw(generators::integers::<usize>().max_value(3)) == 0,
            modes,
            // only the first global `unbind` node is read, so generate at most one
            global_unbind: tc.draw(generators::vecs(any_key()).max_size(2)),
        }
    }

    fn render(block: &KeybindsBlock) -> String {
        let mut out = String::from("keybinds");
        if block.clear_defaults {
            out.push_str(" clear-defaults=true");
        }
        out.push_str(" {\n");
        for mode_block in &block.modes {
            out.push_str(&format!("{:?}", mode_block.mode));
            if mode_block.clear_defaults {
                out.push_str(" clear-defaults=true");
            }
            out.push_str(" {\n");
            for op in &mode_block.ops {
                match op {
                    Op::Bind { keys, actions } => {
                        out.push_str("bind");
                        for key in keys {
                            out.push_str(&format!(" \"{}\"", key));
                        }
                        out.push_str(" { ");
                        for action in actions {
                            out.push_str(action_table()[*action].0);
                            out.push(' ');
                        }
                        out.push_str("}\n");
                    },
                    Op::Unbind { keys } => {
                        out.push_str("unbind");
                        for key in keys {
                            out.push_str(&format!(" \"{}\"", key));
                        }
                        out.push('\n');
                    },
                }
            }
            out.push_str("}\n");
        }
        if !block.global_unbind.is_empty() {
            out.push_str("unbind");
            for key in &block.global_unbind {
                out.push_str(&format!(" \"{}\"", key));
            }
            out.push('\n');
        }
        out.push_str("}\n");
        out
    }

    type Model = HashMap<InputMode, HashMap<KeyWithModifier, Vec<Action>>>;

    /// The naive reading of the documented rules: clear-defaults wipes its scope, binds
    /// insert in order, all unbinds in a mode block happen after all of its binds, and
    /// the block-level `unbind` is applied last, to every mode.
    fn apply(block: &KeybindsBlock, base: Model) -> Model {
        let mut model = if block.clear_defaults {
            Model::new()
        } else {
            base
        };
        for mode_block in &block.modes {
            let mode_keybinds = model.entry(mode_block.mode).or_default();
            if mode_block.clear_defaults {
                mode_keybinds.clear();
            }
            for op in &mode_block.ops {
                if let Op::Bind { keys, actions } = op {
                    let actions: Vec<Action> = actions
                        .iter()
                        .map(|i| action_table()[*i].1.clone())
                        .collect();
                    for key in keys {
                        mode_keybinds.insert(key.clone(), actions.clone());
                    }
                }
            }
            for op in &mode_block.ops {
                if let Op::Unbind { keys } = op {
                    for key in keys {
                        mode_keybinds.remove(key);
                    }
                }
            }
        }
        for mode_keybinds in model.values_mut() {
            for key in &block.global_unbind {
                mode_keybinds.remove(key);
            }
        }
        model
    }

    /// Replaces `can_define_keybindings_in_configfile`,
    /// `can_define_multiple_keybinds_for_same_action`,
    /// `can_define_series_of_actions_for_same_keybinding`,
    /// `keybindings_bind_order_is_preserved`,
    /// `uppercase_and_lowercase_keybindings_are_distinct`, `can_override_keybindings`,
    /// `can_add_to_default_keybindings`, `can_clear_default_keybindings`,
    /// `can_clear_default_keybindings_per_single_mode`,
    /// `can_unbind_multiple_keys_globally`, `can_unbind_multiple_keys_per_single_mode`
    /// and `keybindings_unbinds_happen_after_binds`.
    #[hegel::test]
    fn parsed_keybinds_match_a_naive_apply_in_order_model(tc: hegel::TestCase) {
        let base_block = tc.draw(any_keybinds_block());
        let user_block = tc.draw(any_keybinds_block());

        // the model starts empty because `Config::from_kdl(_, None)` starts from
        // `Config::default()`, which carries no keybinds
        assert!(
            Config::default().keybinds.0.is_empty(),
            "the model assumes a default Config has no keybinds"
        );

        let base_kdl = render(&base_block);
        let user_kdl = render(&user_block);
        let base_config = Config::from_kdl(&base_kdl, None)
            .unwrap_or_else(|e| panic!("base config did not parse: {}\n{:?}", base_kdl, e));
        let config = Config::from_kdl(&user_kdl, Some(base_config))
            .unwrap_or_else(|e| panic!("user config did not parse: {}\n{:?}", user_kdl, e));

        let model = apply(&user_block, apply(&base_block, Model::new()));

        // sweep every mode and every key either config mentions, rather than drawing one
        let mut keys: Vec<KeyWithModifier> = vec![];
        for block in [&base_block, &user_block] {
            for mode_block in &block.modes {
                for op in &mode_block.ops {
                    match op {
                        Op::Bind { keys: k, .. } | Op::Unbind { keys: k } => {
                            keys.extend(k.iter().cloned())
                        },
                    }
                }
            }
            keys.extend(block.global_unbind.iter().cloned());
        }
        for mode in InputMode::iter() {
            for key in &keys {
                let expected = model.get(&mode).and_then(|binds| binds.get(key));
                assert_eq!(
                    config.keybinds.get_actions_for_key_in_mode(&mode, key),
                    expected,
                    "{:?} in {:?} mode\nbase:\n{}\nuser:\n{}",
                    key,
                    mode,
                    base_kdl,
                    user_kdl
                );
            }
        }

        // and no bind the model does not know about
        for (mode, binds) in &config.keybinds.0 {
            for (key, actions) in binds {
                assert_eq!(
                    Some(actions),
                    model.get(mode).and_then(|binds| binds.get(key)),
                    "unexpected bind {:?} in {:?} mode\nbase:\n{}\nuser:\n{}",
                    key,
                    mode,
                    base_kdl,
                    user_kdl
                );
            }
        }
    }
}

#[test]
fn message_plugin_id_checks_the_shared_identity_range() {
    use yinz_types::input::options::Options;
    use yinz_types::internal_ids::PluginId;
    for value in [-1_i64, i64::from(u32::MAX) + 1, i64::MAX] {
        let node: kdl::KdlNode = format!("MessagePluginId {value}").parse().unwrap();
        assert!(crate::kdl::action::from_kdl((&node, &Options::default())).is_err());
    }
    for value in [0_u32, u32::MAX] {
        let node: kdl::KdlNode = format!("MessagePluginId {value}").parse().unwrap();
        let action = crate::kdl::action::from_kdl((&node, &Options::default())).unwrap();
        match action {
            Action::KeybindPipe { plugin_id, .. } => {
                assert_eq!(plugin_id, Some(PluginId::new(value)))
            },
            other => panic!("unexpected action: {other:?}"),
        }
    }
}

#[test]
fn session_pane_selector_checks_kdl_range() {
    use yinz_types::input::options::Options;
    for id in [-1_i64, i64::from(u32::MAX) + 1, i64::MAX] {
        let node: kdl::KdlNode = format!(r#"SwitchSession name="main" pane_id={id}"#)
            .parse()
            .unwrap();
        assert!(crate::kdl::action::from_kdl((&node, &Options::default())).is_err());
    }
}

/// `example/default.kdl` is the config people copy to start their own, and nothing else loads
/// it, so a key the shipped config gains can go missing from it without anything failing.
/// `Ctrl e o` did: the example's comments said Ctrl o moved the panel, and its Agent mode had
/// no `o`.
#[test]
fn example_agent_mode_moves_the_panel() {
    let config = Config::from_kdl(include_str!("../../../example/default.kdl"), None)
        .expect("example/default.kdl parses");
    let actions = config
        .keybinds
        .get_actions_for_key_in_mode(&InputMode::Agent, &KeyWithModifier::new(BareKey::Char('o')))
        .expect("Agent mode binds o");
    // A message that names no plugin, so it reaches the agent already running and starts
    // none, then back to Normal as every other Agent-mode key goes.
    assert!(
        matches!(
            &actions[..],
            [
                Action::KeybindPipe {
                    name: Some(name),
                    plugin: None,
                    plugin_id: None,
                    launch_new: false,
                    ..
                },
                Action::SwitchToMode {
                    input_mode: InputMode::Normal
                },
            ] if name == "agent_strip_reposition"
        ),
        "{actions:?}"
    );
}
