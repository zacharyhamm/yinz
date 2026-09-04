use ansi_term::{
    unstyle, ANSIString, ANSIStrings,
    Color::{Fixed, RGB},
    Style,
};
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_tile_utils::palette_match;

use crate::{
    action_key,
    keybind_hints::render_hints,
    style_key_with_modifier,
    tip::{data::TIPS, TipBody},
    LinePart, TO_NORMAL,
};

fn full_length_shortcut(
    is_first_shortcut: bool,
    key: Vec<KeyWithModifier>,
    action: &str,
    palette: Styling,
) -> LinePart {
    if key.is_empty() {
        return LinePart::default();
    }

    let text_color = palette_match!(palette.text_unselected.base);

    let separator = if is_first_shortcut { " " } else { " / " };
    let mut bits: Vec<ANSIString> = vec![Style::new().fg(text_color).paint(separator)];
    bits.extend(style_key_with_modifier(&key, &palette, None));
    bits.push(
        Style::new()
            .fg(text_color)
            .bold()
            .paint(format!(" {}", action)),
    );
    let part = ANSIStrings(&bits);

    LinePart {
        part: part.to_string(),
        len: unstyle(&part).chars().count(),
    }
}

fn locked_interface_indication(palette: Styling) -> LinePart {
    let locked_text = " -- INTERFACE LOCKED -- ";
    let locked_text_len = locked_text.chars().count();
    let text_color = palette_match!(palette.text_unselected.base);
    let locked_styled_text = Style::new().fg(text_color).bold().paint(locked_text);
    LinePart {
        part: locked_styled_text.to_string(),
        len: locked_text_len,
    }
}

/// The tip in the longest of its three forms that fits `max_width`, or nothing.
fn tip_that_fits(help: &ModeInfo, tip: &TipBody, max_width: usize) -> LinePart {
    [tip.full, tip.medium, tip.short]
        .into_iter()
        .map(|render| render(help))
        .find(|line_part| line_part.len <= max_width)
        .unwrap_or_default()
}

pub fn keybinds(help: &ModeInfo, tip_name: &str, max_width: usize) -> LinePart {
    match help.mode {
        InputMode::Normal => {
            // It is assumed that there is at least one TIP data in the TIPS HasMap.
            let tip_body = TIPS
                .get(tip_name)
                .unwrap_or_else(|| TIPS.get("quicknav").unwrap());
            tip_that_fits(help, tip_body, max_width)
        },
        InputMode::Locked => {
            let line_part = locked_interface_indication(help.style.colors);
            if line_part.len <= max_width {
                line_part
            } else {
                LinePart::default()
            }
        },
        _ => render_hints(help, max_width, &|is_first, text, keys| {
            full_length_shortcut(is_first, keys.to_vec(), text, help.style.colors)
        }),
    }
}

pub fn descended_into_nested_session_hint(help: &ModeInfo, max_len: usize) -> LinePart {
    nested_session_status_hint(help, "Ascend: ", &help.nested_ascend_keys, max_len)
}

pub fn ascended_to_host_session_hint(help: &ModeInfo, max_len: usize) -> LinePart {
    nested_session_status_hint(help, "Descend: ", &help.nested_descend_keys, max_len)
}

fn nested_session_status_hint(
    help: &ModeInfo,
    label: &str,
    keys: &[KeyWithModifier],
    max_len: usize,
) -> LinePart {
    let palette = help.style.colors;
    let text_color = palette_match!(palette.text_unselected.base);
    let styled_label = Style::new().dimmed().italic().paint(format!(" {}", label));
    let mut key_bits: Vec<ANSIString> = vec![];
    if keys.is_empty() {
        key_bits.push(Style::new().dimmed().italic().paint("<unbound>"));
    } else {
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                key_bits.push(Style::new().fg(text_color).paint(" + "));
            }
            key_bits.extend(style_key_with_modifier(
                std::slice::from_ref(key),
                &palette,
                None,
            ));
        }
    }
    let mut bits: Vec<ANSIString> = vec![styled_label];
    bits.extend(key_bits.clone());
    let part = ANSIStrings(&bits);
    let len = unstyle(&part).chars().count();
    if len <= max_len {
        return LinePart {
            part: part.to_string(),
            len,
        };
    }
    let part = ANSIStrings(&key_bits);
    let len = unstyle(&part).chars().count();
    if len <= max_len {
        LinePart {
            part: part.to_string(),
            len,
        }
    } else {
        LinePart::default()
    }
}

pub fn text_copied_hint(copy_destination: CopyDestination) -> LinePart {
    let hint = match copy_destination {
        CopyDestination::Command => "Text piped to external command",
        #[cfg(not(target_os = "macos"))]
        CopyDestination::Primary => "Text copied to system primary selection",
        #[cfg(target_os = "macos")] // primary selection does not exist on macos
        CopyDestination::Primary => "Text copied to system clipboard",
        CopyDestination::System => "Text copied to system clipboard",
    };
    LinePart {
        part: serialize_text(&Text::new(hint).color_range(2, ..).opaque()),
        len: hint.len(),
    }
}

pub fn system_clipboard_error(palette: &Styling) -> LinePart {
    let hint = " Error using the system clipboard.";
    let red_color = palette_match!(palette.text_unselected.emphasis_3);
    LinePart {
        part: Style::new().fg(red_color).bold().paint(hint).to_string(),
        len: hint.len(),
    }
}

pub fn fullscreen_panes_to_hide(palette: &Styling, panes_to_hide: usize) -> LinePart {
    let text_color = palette_match!(palette.text_unselected.base);
    let green_color = palette_match!(palette.text_unselected.emphasis_2);
    let orange_color = palette_match!(palette.text_unselected.emphasis_0);
    let shortcut_left_separator = Style::new().fg(text_color).bold().paint(" (");
    let shortcut_right_separator = Style::new().fg(text_color).bold().paint("): ");
    let fullscreen = "FULLSCREEN";
    let puls = "+ ";
    let panes = panes_to_hide.to_string();
    let hide = " hidden panes";
    let len = fullscreen.chars().count()
        + puls.chars().count()
        + panes.chars().count()
        + hide.chars().count()
        + 5; // 3 for ():'s around shortcut, 2 for the space
    LinePart {
        part: format!(
            "{}{}{}{}{}{}",
            shortcut_left_separator,
            Style::new().fg(orange_color).bold().paint(fullscreen),
            shortcut_right_separator,
            Style::new().fg(text_color).bold().paint(puls),
            Style::new().fg(green_color).bold().paint(panes),
            Style::new().fg(text_color).bold().paint(hide)
        ),
        len,
    }
}

pub fn floating_panes_are_visible(mode_info: &ModeInfo) -> LinePart {
    let palette = mode_info.style.colors;
    let km = &mode_info.get_mode_keybinds();
    let white_color = palette_match!(palette.text_unselected.base);
    let green_color = palette_match!(palette.text_unselected.emphasis_2);
    let orange_color = palette_match!(palette.text_unselected.emphasis_0);
    let shortcut_left_separator = Style::new().fg(white_color).bold().paint(" (");
    let shortcut_right_separator = Style::new().fg(white_color).bold().paint("): ");
    let floating_panes = "FLOATING PANES VISIBLE";
    let press = "Press ";
    let pane_mode = format!(
        "{}",
        action_key(
            km,
            &[Action::SwitchToMode {
                input_mode: InputMode::Pane
            }]
        )
        .first()
        .unwrap_or(&KeyWithModifier::new(BareKey::Char('?')))
    );
    let plus = ", ";
    let p_left_separator = "<";
    let p = format!(
        "{}",
        action_key(
            &mode_info.get_keybinds_for_mode(InputMode::Pane),
            &[Action::ToggleFloatingPanes, TO_NORMAL]
        )
        .first()
        .unwrap_or(&KeyWithModifier::new(BareKey::Char('?')))
    );
    let p_right_separator = "> ";
    let to_hide = "to hide.";

    let len = floating_panes.chars().count()
        + press.chars().count()
        + pane_mode.chars().count()
        + plus.chars().count()
        + p_left_separator.chars().count()
        + p.chars().count()
        + p_right_separator.chars().count()
        + to_hide.chars().count()
        + 5; // 3 for ():'s around floating_panes, 2 for the space
    LinePart {
        part: format!(
            "{}{}{}{}{}{}{}{}{}{}",
            shortcut_left_separator,
            Style::new().fg(orange_color).bold().paint(floating_panes),
            shortcut_right_separator,
            Style::new().fg(white_color).bold().paint(press),
            Style::new().fg(green_color).bold().paint(pane_mode),
            Style::new().fg(white_color).bold().paint(plus),
            Style::new().fg(white_color).bold().paint(p_left_separator),
            Style::new().fg(green_color).bold().paint(p),
            Style::new().fg(white_color).bold().paint(p_right_separator),
            Style::new().fg(white_color).bold().paint(to_hide),
        ),
        len,
    }
}

pub fn locked_fullscreen_panes_to_hide(palette: &Styling, panes_to_hide: usize) -> LinePart {
    let text_color = palette_match!(palette.text_unselected.base);
    let green_color = palette_match!(palette.text_unselected.emphasis_2);
    let orange_color = palette_match!(palette.text_unselected.emphasis_0);
    let locked_text = " -- INTERFACE LOCKED -- ";
    let shortcut_left_separator = Style::new().fg(text_color).bold().paint(" (");
    let shortcut_right_separator = Style::new().fg(text_color).bold().paint("): ");
    let fullscreen = "FULLSCREEN";
    let puls = "+ ";
    let panes = panes_to_hide.to_string();
    let hide = " hidden panes";
    let len = locked_text.chars().count()
        + fullscreen.chars().count()
        + puls.chars().count()
        + panes.chars().count()
        + hide.chars().count()
        + 5; // 3 for ():'s around shortcut, 2 for the space
    LinePart {
        part: format!(
            "{}{}{}{}{}{}{}",
            Style::new().fg(text_color).bold().paint(locked_text),
            shortcut_left_separator,
            Style::new().fg(orange_color).bold().paint(fullscreen),
            shortcut_right_separator,
            Style::new().fg(text_color).bold().paint(puls),
            Style::new().fg(green_color).bold().paint(panes),
            Style::new().fg(text_color).bold().paint(hide)
        ),
        len,
    }
}

pub fn locked_floating_panes_are_visible(palette: &Styling) -> LinePart {
    let white_color = palette_match!(palette.text_unselected.base);
    let orange_color = palette_match!(palette.text_unselected.emphasis_0);
    let shortcut_left_separator = Style::new().fg(white_color).bold().paint(" (");
    let shortcut_right_separator = Style::new().fg(white_color).bold().paint(")");
    let locked_text = " -- INTERFACE LOCKED -- ";
    let floating_panes = "FLOATING PANES VISIBLE";

    let len = locked_text.chars().count() + floating_panes.chars().count();
    LinePart {
        part: format!(
            "{}{}{}{}",
            Style::new().fg(white_color).bold().paint(locked_text),
            shortcut_left_separator,
            Style::new().fg(orange_color).bold().paint(floating_panes),
            shortcut_right_separator,
        ),
        len,
    }
}

#[cfg(test)]
/// Unit tests.
///
/// Note that we cheat a little here, because the number of things one may want to test is endless,
/// and creating a Mockup of [`ModeInfo`] by hand for all these testcases is nothing less than
/// torture. Hence, we test the most atomic unit thoroughly ([`full_length_shortcut`] and then test
/// the public API ([`keybinds`]) to ensure correct operation.
mod tests {
    use super::*;

    // Strip style information from `LinePart` and return a raw String instead
    fn unstyle(line_part: LinePart) -> String {
        let string = line_part.to_string();

        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let string = re.replace_all(&string, "".to_string());

        string.to_string()
    }

    mod props {
        use super::*;
        use hegel::generators;

        fn all_input_modes() -> Vec<InputMode> {
            vec![
                InputMode::Normal,
                InputMode::Locked,
                InputMode::Resize,
                InputMode::Pane,
                InputMode::Tab,
                InputMode::Scroll,
                InputMode::EnterSearch,
                InputMode::Search,
                InputMode::RenameTab,
                InputMode::RenamePane,
                InputMode::Session,
                InputMode::Move,
                InputMode::Prompt,
                InputMode::Tmux,
                InputMode::Agent,
                InputMode::Select,
            ]
        }

        fn all_tip_names() -> Vec<&'static str> {
            vec![
                "quicknav",
                "compact_layout",
                "edit_scrollbuffer",
                "floating_panes_mouse",
                "move_focus_hjkl_tab_switch",
                "move_tabs",
                "send_mouse_clicks_to_terminal",
                "sync_tab",
                "use_mouse",
                "zellij_setup_check",
                // not a real tip: `keybinds` documents a fallback for this
                "no_such_tip",
            ]
        }

        #[hegel::composite]
        fn any_mode_info(tc: &hegel::TestCase) -> ModeInfo {
            let mode = tc.draw(generators::sampled_from(all_input_modes()));
            let bindings = tc.draw(generators::vecs(generators::tuples!(
                generators::characters(),
                generators::sampled_from(all_input_modes()),
                generators::booleans(),
            )));
            let keybinds = vec![(
                mode,
                bindings
                    .into_iter()
                    .map(|(key, target, ctrl)| {
                        let bare = KeyWithModifier::new(BareKey::Char(key));
                        let bare = if ctrl {
                            bare.with_ctrl_modifier()
                        } else {
                            bare
                        };
                        (bare, vec![Action::SwitchToMode { input_mode: target }])
                    })
                    .collect(),
            )];
            ModeInfo {
                mode,
                keybinds,
                ..ModeInfo::default()
            }
        }

        /// The three `keybinds_*` tests this replaces were three points on this
        /// curve (500 columns, a tight width, and one odd keyset). The status
        /// bar overdrawing its budget corrupts the line below it, so the bound
        /// has to hold at *every* width.
        #[hegel::test]
        fn keybinds_never_claims_more_columns_than_it_was_given(tc: hegel::TestCase) {
            let mode_info = tc.draw(any_mode_info());
            let tip_name = tc.draw(generators::sampled_from(all_tip_names()));
            let max_width = tc.draw(generators::integers::<usize>().max_value(500));

            let part = keybinds(&mode_info, tip_name, max_width);
            assert!(
                part.len <= max_width,
                "keybinds claimed {} columns but was given {} (mode {:?}, tip {:?})",
                part.len,
                max_width,
                mode_info.mode,
                tip_name
            );
        }

        /// `LinePart::len` is what the caller budgets with, so it has to match
        /// what actually gets rendered - styles excluded.
        #[hegel::test]
        fn keybinds_reports_the_width_it_actually_renders(tc: hegel::TestCase) {
            let mode_info = tc.draw(any_mode_info());
            let tip_name = tc.draw(generators::sampled_from(all_tip_names()));
            let max_width = tc.draw(generators::integers::<usize>().max_value(500));

            let part = keybinds(&mode_info, tip_name, max_width);
            let claimed = part.len;
            let rendered = unstyle(part);
            assert_eq!(
                claimed,
                rendered.chars().count(),
                "keybinds claimed {} columns but rendered {:?} (mode {:?}, tip {:?}, max {})",
                claimed,
                rendered,
                mode_info.mode,
                tip_name,
                max_width
            );
        }

        /// Widening the terminal must never render *less*. This is the property
        /// the "note how it leaves out elements" / "note how Move focus becomes
        /// Move" comments on the old tests were gesturing at.
        #[hegel::test]
        fn a_wider_budget_never_renders_fewer_columns(tc: hegel::TestCase) {
            let mode_info = tc.draw(any_mode_info());
            let tip_name = tc.draw(generators::sampled_from(all_tip_names()));
            let narrow = tc.draw(generators::integers::<usize>().max_value(500));
            let extra = tc.draw(generators::integers::<usize>().max_value(500));
            let wide = narrow + extra;

            let narrow_part = keybinds(&mode_info, tip_name, narrow);
            let wide_part = keybinds(&mode_info, tip_name, wide);
            assert!(
                wide_part.len >= narrow_part.len,
                "keybinds rendered {} columns at width {} but only {} at width {} \
                 (mode {:?}, tip {:?})",
                narrow_part.len,
                narrow,
                wide_part.len,
                wide,
                mode_info.mode,
                tip_name
            );
        }
    }

    #[test]
    fn full_length_shortcut_with_key() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Char('a'))];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / <a> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_key_first_element() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Char('a'))];
        let palette = Styling::default();

        let ret = full_length_shortcut(true, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " <a> Foobar");
    }

    #[test]
    // When there is no binding, we print no shortcut either
    fn full_length_shortcut_without_key() {
        let keyvec = vec![];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, "");
    }

    #[test]
    fn full_length_shortcut_with_key_unprintable_1() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Enter)];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / <ENTER> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_key_unprintable_2() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Backspace)];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / <BACKSPACE> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_ctrl_key() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier()];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / Ctrl + <a> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_alt_key() {
        let keyvec = vec![KeyWithModifier::new(BareKey::Char('a')).with_alt_modifier()];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / Alt + <a> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_homogenous_key_group() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')),
            KeyWithModifier::new(BareKey::Char('b')),
            KeyWithModifier::new(BareKey::Char('c')),
        ];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / <a|b|c> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_heterogenous_key_group() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')),
            KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Enter),
        ];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / <a|Ctrl b|ENTER> Foobar");
    }

    #[test]
    fn full_length_shortcut_with_key_group_shared_ctrl_modifier() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(),
        ];
        let palette = Styling::default();

        let ret = full_length_shortcut(false, keyvec, "Foobar", palette);
        let ret = unstyle(ret);

        assert_eq!(ret, " / Ctrl + <a|b|c> Foobar");
    }
    //pub fn keybinds(help: &ModeInfo, tip_name: &str, max_width: usize) -> LinePart {

    #[test]
    // Note how it leaves out elements that don't exist!
    fn keybinds_wide() {
        let mode_info = ModeInfo {
            mode: InputMode::Pane,
            keybinds: vec![(
                InputMode::Pane,
                vec![
                    (
                        KeyWithModifier::new(BareKey::Left),
                        vec![Action::MoveFocus {
                            direction: Direction::Left,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Down),
                        vec![Action::MoveFocus {
                            direction: Direction::Down,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Up),
                        vec![Action::MoveFocus {
                            direction: Direction::Up,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Right),
                        vec![Action::MoveFocus {
                            direction: Direction::Right,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('n')),
                        vec![
                            Action::NewPane {
                                direction: None,
                                pane_name: None,
                                start_suppressed: false,
                            },
                            TO_NORMAL,
                        ],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('x')),
                        vec![Action::CloseFocus, TO_NORMAL],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('f')),
                        vec![Action::ToggleFocusFullscreen, TO_NORMAL],
                    ),
                ],
            )],
            ..ModeInfo::default()
        };

        let ret = keybinds(&mode_info, "quicknav", 500);
        let ret = unstyle(ret);

        assert_eq!(
            ret,
            " <n> New / <←↓↑→> Change Focus / <x> Close / <f> Toggle Fullscreen",
        );
    }

    #[test]
    // Note how "Move focus" becomes "Move"
    fn keybinds_tight_width() {
        let mode_info = ModeInfo {
            mode: InputMode::Pane,
            keybinds: vec![(
                InputMode::Pane,
                vec![
                    (
                        KeyWithModifier::new(BareKey::Left),
                        vec![Action::MoveFocus {
                            direction: Direction::Left,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Down),
                        vec![Action::MoveFocus {
                            direction: Direction::Down,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Up),
                        vec![Action::MoveFocus {
                            direction: Direction::Up,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Right),
                        vec![Action::MoveFocus {
                            direction: Direction::Right,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('n')),
                        vec![
                            Action::NewPane {
                                direction: None,
                                pane_name: None,
                                start_suppressed: false,
                            },
                            TO_NORMAL,
                        ],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('x')),
                        vec![Action::CloseFocus, TO_NORMAL],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('f')),
                        vec![Action::ToggleFocusFullscreen, TO_NORMAL],
                    ),
                ],
            )],
            ..ModeInfo::default()
        };

        let ret = keybinds(&mode_info, "quicknav", 35);
        let ret = unstyle(ret);

        assert_eq!(ret, " <n> New / <←↓↑→> Move ... ");
    }

    #[test]
    fn keybinds_wide_weird_keys() {
        let mode_info = ModeInfo {
            mode: InputMode::Pane,
            keybinds: vec![(
                InputMode::Pane,
                vec![
                    (
                        KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(),
                        vec![Action::MoveFocus {
                            direction: Direction::Left,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier(),
                        vec![Action::MoveFocus {
                            direction: Direction::Down,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier(),
                        vec![Action::MoveFocus {
                            direction: Direction::Up,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char(' ')).with_ctrl_modifier(),
                        vec![Action::MoveFocus {
                            direction: Direction::Right,
                        }],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Backspace),
                        vec![
                            Action::NewPane {
                                direction: None,
                                pane_name: None,
                                start_suppressed: false,
                            },
                            TO_NORMAL,
                        ],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Esc),
                        vec![Action::CloseFocus, TO_NORMAL],
                    ),
                    (
                        KeyWithModifier::new(BareKey::End),
                        vec![Action::ToggleFocusFullscreen, TO_NORMAL],
                    ),
                ],
            )],
            ..ModeInfo::default()
        };

        let ret = keybinds(&mode_info, "quicknav", 500);
        let ret = unstyle(ret);

        assert_eq!(ret, " <BACKSPACE> New / Ctrl + <a|ENTER|1|SPACE> Change Focus / <ESC> Close / <END> Toggle Fullscreen");
    }
}
