use super::super::actions::*;
use super::super::keybinds::*;
use crate::input::config::Config;
use insta::assert_snapshot;
use strum::IntoEnumIterator;
use zellij_types::geometry::Direction;
use zellij_types::keys::{BareKey, KeyWithModifier};

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
    use crate::data::{InputMode, KeyModifier};
    use hegel::generators;
    use std::collections::HashMap;

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
