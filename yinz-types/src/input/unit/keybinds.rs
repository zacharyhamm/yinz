use super::*;

fn key(letter: char) -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(letter))
}

fn switch(mode: InputMode) -> Action {
    Action::SwitchToMode { input_mode: mode }
}

fn quit_shortcut(keybinds: &KeybindsVec) -> Option<Vec<KeyWithModifier>> {
    shortcut_for_action(keybinds, InputMode::Normal, |action| {
        matches!(action, Action::Quit)
    })
}

#[test]
fn shortcut_prefers_a_direct_binding_and_sorts_keys() {
    let keybinds = vec![
        (
            InputMode::Normal,
            vec![
                (key('z'), vec![Action::Quit]),
                (key('a'), vec![switch(InputMode::Pane)]),
                (key('x'), vec![Action::NoOp, Action::Quit]),
            ],
        ),
        (InputMode::Pane, vec![(key('a'), vec![Action::Quit])]),
    ];
    assert_eq!(quit_shortcut(&keybinds), Some(vec![key('x')]));
    assert_eq!(
        shortcut_for_action(&keybinds, InputMode::Pane, |action| matches!(
            action,
            Action::Quit
        )),
        Some(vec![key('a')])
    );
}

#[test]
fn shortcut_orders_equal_length_routes_by_keys() {
    let keybinds = vec![
        (
            InputMode::Normal,
            vec![
                (key('b'), vec![switch(InputMode::Tab)]),
                (key('c'), vec![switch(InputMode::Pane)]),
                (key('a'), vec![switch(InputMode::Pane)]),
            ],
        ),
        (InputMode::Tab, vec![(key('a'), vec![Action::Quit])]),
        (
            InputMode::Pane,
            vec![
                (key('z'), vec![Action::Quit]),
                (key('y'), vec![Action::Quit]),
            ],
        ),
    ];
    assert_eq!(quit_shortcut(&keybinds), Some(vec![key('a'), key('y')]));
}

#[test]
fn shortcut_prefers_a_shorter_route_from_a_later_mode() {
    let keybinds = vec![
        (
            InputMode::Normal,
            vec![
                (key('a'), vec![switch(InputMode::Pane)]),
                (key('b'), vec![switch(InputMode::Tab)]),
            ],
        ),
        (
            InputMode::Pane,
            vec![(key('a'), vec![switch(InputMode::Resize)])],
        ),
        (InputMode::Resize, vec![(key('a'), vec![Action::Quit])]),
        (InputMode::Tab, vec![(key('z'), vec![Action::Quit])]),
    ];
    assert_eq!(quit_shortcut(&keybinds), Some(vec![key('b'), key('z')]));
}

#[test]
fn shortcut_handles_cycles_missing_modes_and_unbound_actions() {
    let mut keybinds = vec![
        (
            InputMode::Normal,
            vec![(
                key('a'),
                vec![switch(InputMode::Pane), switch(InputMode::Tab)],
            )],
        ),
        (
            InputMode::Pane,
            vec![(key('b'), vec![switch(InputMode::Normal)])],
        ),
    ];
    assert_eq!(quit_shortcut(&keybinds), None);
    keybinds[1].1.push((key('c'), vec![Action::Quit]));
    assert_eq!(quit_shortcut(&keybinds), Some(vec![key('a'), key('c')]));
    assert_eq!(quit_shortcut(&vec![]), None);
}

#[test]
fn shortcut_accepts_four_keys_but_not_five() {
    let mut keybinds = vec![
        (
            InputMode::Normal,
            vec![(key('a'), vec![switch(InputMode::Pane)])],
        ),
        (
            InputMode::Pane,
            vec![(key('b'), vec![switch(InputMode::Tab)])],
        ),
        (
            InputMode::Tab,
            vec![(key('c'), vec![switch(InputMode::Resize)])],
        ),
        (
            InputMode::Resize,
            vec![(key('d'), vec![switch(InputMode::Scroll)])],
        ),
        (InputMode::Scroll, vec![(key('e'), vec![Action::Quit])]),
    ];
    assert_eq!(quit_shortcut(&keybinds), None);
    keybinds[3].1[0].1.push(Action::Quit);
    assert_eq!(
        quit_shortcut(&keybinds),
        Some(vec![key('a'), key('b'), key('c'), key('d')])
    );
}
