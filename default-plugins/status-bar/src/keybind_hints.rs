//! The keybinding hints a mode shows in the status bar, shared by both layouts.
//!
//! The classic two-line layout (`second_line`) and the one-line layout (`one_line_ui`) draw a
//! hint differently — `<n> New` in plain text versus a `<n>` key next to a ribbon — but *which*
//! hints a mode has, which keys they resolve to and how the list is squeezed into the width it is
//! given are one thing. They used to be two copies that drifted apart, so a hint added to one
//! layout silently never reached the other; the layout now only contributes a [`HintRenderer`].

use yinz_tile::prelude::actions::Action;
use yinz_tile::prelude::*;

use crate::{
    action_key, action_key_group, keybind_pipe_key, plugin_launch_key, the_modes_own_key, LinePart,
    MORE_MSG,
};

/// One hint: what to say when there is room, what to say when there is not, and the keys that
/// trigger it. A hint with no keys is not drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint {
    pub long: String,
    pub short: String,
    pub keys: Vec<KeyWithModifier>,
}

fn hint(long: &str, short: &str, keys: Vec<KeyWithModifier>) -> Hint {
    Hint {
        long: long.to_string(),
        short: short.to_string(),
        keys,
    }
}

/// How a layout draws one hint. `is_first` says whether the hint opens the line, for a layout
/// that puts a separator between hints; a renderer returns an empty [`LinePart`] for no keys.
pub type HintRenderer<'a> = &'a dyn Fn(bool, &str, &[KeyWithModifier]) -> LinePart;

/// The hints for `help.mode`, drawn to fit `max_width`: every hint with its long label if that
/// fits, else every hint with its short label, else as many short hints as fit, followed by
/// [`MORE_MSG`] when there is room for it.
pub fn render_hints(help: &ModeInfo, max_width: usize, render: HintRenderer) -> LinePart {
    let hints = hints_for_mode(help);

    let full = render_all(&hints, |hint| &hint.long, render);
    if full.len <= max_width {
        return full;
    }
    let shortened = render_all(&hints, |hint| &hint.short, render);
    if shortened.len <= max_width {
        return shortened;
    }
    render_as_many_as_fit(&hints, max_width, render)
}

fn render_all(hints: &[Hint], label: impl Fn(&Hint) -> &str, render: HintRenderer) -> LinePart {
    let mut line = LinePart::default();
    for hint in hints {
        line.append(&render(line.len == 0, label(hint), &hint.keys));
    }
    line
}

fn render_as_many_as_fit(hints: &[Hint], max_width: usize, render: HintRenderer) -> LinePart {
    let more_len = MORE_MSG.chars().count();
    let mut line = LinePart::default();
    for hint in hints {
        let shortcut = render(line.len == 0, &hint.short, &hint.keys);
        if line.len + shortcut.len + more_len > max_width {
            // Only append MORE_MSG if it fits within the budget; otherwise return what we have
            // (which may be empty), so the line never claims more columns than it was given.
            if line.len + more_len <= max_width {
                line.part = format!("{}{}", line.part, MORE_MSG);
                line.len += more_len;
            }
            break;
        }
        line.append(&shortcut);
    }
    line
}

/// The key whose binding starts with `action`, whatever follows it.
///
/// Nearly every hint is for a binding of the form "do the thing, then leave the mode", and where
/// it leaves *to* depends on the config: `SwitchToMode "Normal"` in the shipped config,
/// `"Locked"` in a locked-base-mode setup. The hint is about the thing, so the trailing switch
/// takes no part in the match — [`action_key`] compares the whole sequence, and left the hints
/// blank under a locked base mode. Among several keys, [`the_modes_own_key`] says which to name.
fn key_doing(keymap: &[(KeyWithModifier, Vec<Action>)], action: &Action) -> Vec<KeyWithModifier> {
    the_modes_own_key(
        keymap
            .iter()
            .filter(|(_, acvec)| acvec.first().is_some_and(|first| first.shallow_eq(action)))
            .map(|(key, _)| key),
    )
}

/// [`key_doing`], but `preferred` when it is one of the keys doing `action` — for a hint that
/// reads best as the arrow pointing the way the action goes, whichever key would otherwise win.
fn key_doing_preferring(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    action: &Action,
    preferred: BareKey,
) -> Vec<KeyWithModifier> {
    let preferred = KeyWithModifier::new(preferred);
    let preferred_does_it = keymap.iter().any(|(key, acvec)| {
        *key == preferred && acvec.first().is_some_and(|first| first.shallow_eq(action))
    });
    if preferred_does_it {
        vec![preferred]
    } else {
        key_doing(keymap, action)
    }
}

/// [`key_doing`] for several actions, in order — for hints like "Break pane left/right".
fn keys_doing_each(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    actions: &[Action],
) -> Vec<KeyWithModifier> {
    actions
        .iter()
        .flat_map(|action| key_doing(keymap, action))
        .collect()
}

/// The key that leaves the mode for the base mode. `Enter` is preferred when it is one of them,
/// since that is the one every mode shares.
fn back_to_base_mode_key(
    mi: &ModeInfo,
    keymap: &[(KeyWithModifier, Vec<Action>)],
) -> Vec<KeyWithModifier> {
    let base_mode = mi.base_mode.unwrap_or(InputMode::Normal);
    let keys = action_key(
        keymap,
        &[Action::SwitchToMode {
            input_mode: base_mode,
        }],
    );
    let enter = KeyWithModifier::new(BareKey::Enter);
    if keys.contains(&enter) {
        vec![enter]
    } else {
        // Yield `vec![key]` if there is at least one key, or an empty vec otherwise.
        keys.into_iter().take(1).collect()
    }
}

/// The key that leaves a mode back to the base mode, preferring Esc when it does.
///
/// For a mode whose Enter is taken by something else (the notification browser activates with it),
/// Esc is the key worth showing - otherwise any shared leader that also reaches the base mode, such
/// as the lock key under a locked base mode, could be picked in its place.
fn escape_to_base_mode_key(
    mi: &ModeInfo,
    keymap: &[(KeyWithModifier, Vec<Action>)],
) -> Vec<KeyWithModifier> {
    let esc = KeyWithModifier::new(BareKey::Esc);
    let keys = action_key(
        keymap,
        &[Action::SwitchToMode {
            input_mode: mi.base_mode.unwrap_or(InputMode::Normal),
        }],
    );
    if keys.contains(&esc) {
        vec![esc]
    } else {
        keys.into_iter().take(1).collect()
    }
}

/// Collect all relevant keybindings and hints to display for the current [`InputMode`].
///
/// Each [`Hint`] is found by searching the mode's keybindings for a pre-defined pattern of
/// `Action`s. For example, the `InputMode::Pane` input mode determines which keys to display for
/// the "Change Focus" hint by searching the keybindings for anything that matches the
/// `Action::MoveFocus(_)` action. Since by default multiple keybindings map to some action
/// patterns (e.g. `Action::MoveFocus(_)` is bound to "hjkl", the arrow keys and "Alt + <hjkl>"),
/// we deduplicate the vector of all keybindings before processing it.
///
/// Therefore we sort it by the [`Key`]s of the current keymap and deduplicate the resulting sorted
/// vector by the `Vec<Action>` action vectors bound to the keys. As such, when multiple keys map
/// to the same sequence of actions, the keys that appear first in the [`Key`] structure will be
/// displayed.
// Please don't let rustfmt play with the formatting. It will stretch out the function to about
// three times the length and all the keybinding vectors we generate become virtually unreadable
// for humans.
#[rustfmt::skip]
pub fn hints_for_mode(mi: &ModeInfo) -> Vec<Hint> {
    use Action as A;
    use InputMode as IM;
    use Direction as Dir;
    use actions::SearchDirection as SDir;
    use actions::SearchOption as SOpt;

    let mut old_keymap = mi.get_mode_keybinds();

    // Find the key that leaves the mode. Do it here before we dedupe the keymap below!
    let to_base_mode = back_to_base_mode_key(mi, &old_keymap);
    let escape_to_base_mode = escape_to_base_mode_key(mi, &old_keymap);
    let copy_and_exit = match key_doing(&old_keymap, &A::Copy) {
        keys if !keys.is_empty() => keys,
        _ => to_base_mode.clone(),
    };

    // Sort and deduplicate the keybindings first. We sort after the `Key`s, and deduplicate by
    // their `Action` vectors. An unstable sort is fine here because if the user maps anything to
    // the same key again, anything will happen...
    old_keymap.sort_unstable_by(|(keya, _), (keyb, _)| keya.partial_cmp(keyb).unwrap());

    let mut known_actions: Vec<Vec<Action>> = vec![];
    let mut km = vec![];
    for (key, acvec) in old_keymap {
        if known_actions.contains(&acvec) {
            // This action is known already
            continue;
        } else {
            known_actions.push(acvec.to_vec());
            km.push((key, acvec));
        }
    }

    let move_focus = [
        A::MoveFocus{direction: Dir::Left}, A::MoveFocus{direction: Dir::Down},
        A::MoveFocus{direction: Dir::Up}, A::MoveFocus{direction: Dir::Right},
    ];
    let new_tab = A::NewTab{
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: None,
        should_change_focus_to_new_tab: true,
        cwd: None,
        initial_panes: None,
        first_pane_unblock_condition: None,
    };

    if mi.mode == IM::Pane { vec![
        hint("New", "New", key_doing(&km, &A::NewPane{direction: None, pane_name: None, start_suppressed: false})),
        hint("Change Focus", "Move", keys_doing_each(&km, &move_focus)),
        hint("Close", "Close", key_doing(&km, &A::CloseFocus)),
        hint("Rename", "Rename", key_doing(&km, &A::SwitchToMode{input_mode: IM::RenamePane})),
        hint("Toggle Fullscreen", "Fullscreen", key_doing(&km, &A::ToggleFocusFullscreen)),
        hint("Toggle Floating", "Floating", key_doing(&km, &A::ToggleFloatingPanes)),
        hint("Toggle Embed", "Embed", key_doing(&km, &A::TogglePaneEmbedOrFloating)),
        hint("Split Right", "Right", key_doing(&km, &A::NewPane{direction: Some(Dir::Right), pane_name: None, start_suppressed: false})),
        hint("Split Down", "Down", key_doing(&km, &A::NewPane{direction: Some(Dir::Down), pane_name: None, start_suppressed: false})),
        hint("Stack", "Stack", key_doing(&km, &A::NewStackedPane{command: None, pane_name: None, near_current_pane: false, no_focus: false, tab_id: None})),
        hint("Select text", "Text", key_doing(&km, &A::SwitchToMode{input_mode: IM::Select})),
        hint("Select pane", "Select", to_base_mode),
    ]} else if mi.mode == IM::Tab {
        // The shipped config binds every arrow to tab focus (left/up go left, right/down go
        // right), and arrows sort Left, Down, Up, Right, so the deduplicated keymap would name
        // Down for "next". Name the arrow pointing each way when it goes that way, looking in the
        // whole keymap because the dedupe drops Right.
        let old_keymap = mi.get_mode_keybinds();
        let focus_keys = [
            key_doing_preferring(&old_keymap, &A::GoToPreviousTab, BareKey::Left),
            key_doing_preferring(&old_keymap, &A::GoToNextTab, BareKey::Right),
        ].concat();

        vec![
        hint("New", "New", key_doing(&km, &new_tab)),
        hint("Change focus", "Move", focus_keys),
        hint("Close", "Close", key_doing(&km, &A::CloseTab)),
        hint("Rename", "Rename", key_doing(&km, &A::SwitchToMode{input_mode: IM::RenameTab})),
        hint("Sync", "Sync", key_doing(&km, &A::ToggleActiveSyncTab)),
        hint("Break pane to new tab", "Break out", key_doing(&km, &A::BreakPane)),
        hint("Break pane left/right", "Break", keys_doing_each(&km, &[A::BreakPaneLeft, A::BreakPaneRight])),
        hint("Toggle", "Toggle", key_doing(&km, &A::ToggleTab)),
        hint("Select pane", "Select", to_base_mode),
    ]} else if mi.mode == IM::Resize { vec![
        hint("Increase/Decrease size", "Increase/Decrease",
            action_key_group(&km, &[
                &[A::Resize{resize: Resize::Increase, direction: None}],
                &[A::Resize{resize: Resize::Decrease, direction: None}]
            ])),
        hint("Increase to", "Increase", action_key_group(&km, &[
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Left)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Down)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Up)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Right)}]
            ])),
        hint("Decrease from", "Decrease", action_key_group(&km, &[
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Left)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Down)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Up)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Right)}]
            ])),
        hint("Select pane", "Select", to_base_mode),
    ]} else if mi.mode == IM::Move { vec![
        hint("Switch Location", "Move", action_key_group(&km, &[
            &[A::MovePane{direction: Some(Dir::Left)}], &[A::MovePane{direction: Some(Dir::Down)}],
            &[A::MovePane{direction: Some(Dir::Up)}], &[A::MovePane{direction: Some(Dir::Right)}]])),
        hint("When done", "Back", to_base_mode),
    ]} else if mi.mode == IM::Scroll { vec![
        hint("Enter search term", "Search",
            action_key(&km, &[A::SwitchToMode{input_mode: IM::EnterSearch}, A::SearchInput{input: vec![0]}])),
        hint("Scroll", "Scroll", action_key_group(&km, &[
            &[A::ScrollDown], &[A::ScrollUp],
            &[A::PageScrollDown], &[A::PageScrollUp],
            &[A::HalfPageScrollDown], &[A::HalfPageScrollUp]])),
        hint("Edit scrollback in default editor", "Edit",
            key_doing(&km, &A::EditScrollback { ansi: false })),
        hint("Select pane", "Select", to_base_mode),
        hint("Scroll commands", "Commands",
            action_key_group(&km, &[&[A::ScrollToPreviousPrompt], &[A::ScrollToNextPrompt]])),
    ]} else if mi.mode == IM::Select { vec![
        hint("Move selection", "Move", action_key_group(&km, &[
            &[A::MoveTextSelection{direction: Dir::Left}], &[A::MoveTextSelection{direction: Dir::Down}],
            &[A::MoveTextSelection{direction: Dir::Up}], &[A::MoveTextSelection{direction: Dir::Right}]])),
        hint("Swap ends", "Swap", key_doing(&km, &A::SwapTextSelectionEnds)),
        hint("Copy and exit", "Copy", copy_and_exit),
    ]} else if mi.mode == IM::EnterSearch { vec![
        hint("When done", "Done", action_key(&km, &[A::SwitchToMode{input_mode: IM::Search}])),
        hint("Cancel", "Cancel",
            action_key(&km, &[A::SearchInput{input: vec![27]}, A::SwitchToMode{input_mode: IM::Scroll}])),
    ]} else if mi.mode == IM::Search { vec![
        hint("Enter Search term", "Search",
            action_key(&km, &[A::SwitchToMode{input_mode: IM::EnterSearch}, A::SearchInput{input: vec![0]}])),
        hint("Scroll", "Scroll",
            action_key_group(&km, &[&[A::ScrollDown], &[A::ScrollUp]])),
        hint("Scroll page", "Scroll",
            action_key_group(&km, &[&[A::PageScrollDown], &[A::PageScrollUp]])),
        hint("Scroll half page", "Scroll",
            action_key_group(&km, &[&[A::HalfPageScrollDown], &[A::HalfPageScrollUp]])),
        hint("Search down", "Down", action_key(&km, &[A::Search{direction: SDir::Down}])),
        hint("Search up", "Up", action_key(&km, &[A::Search{direction: SDir::Up}])),
        hint("Case sensitive", "Case",
            action_key(&km, &[A::SearchToggleOption{option: SOpt::CaseSensitivity}])),
        hint("Wrap", "Wrap",
            action_key(&km, &[A::SearchToggleOption{option: SOpt::Wrap}])),
        hint("Whole words", "Whole",
            action_key(&km, &[A::SearchToggleOption{option: SOpt::WholeWord}])),
    ]} else if mi.mode == IM::Session { vec![
        hint("Detach", "Detach", action_key(&km, &[A::Detach])),
        hint("Session Manager", "Manager", plugin_launch_key(&km, "session-manager")),
        hint("Configure", "Config", plugin_launch_key(&km, "configuration")),
        hint("Layout Manager", "Layouts", plugin_launch_key(&km, "yinz:layout-manager")),
        hint("Plugin Manager", "Plugins", plugin_launch_key(&km, "plugin-manager")),
        hint("Notifications", "Notif",
            action_key(&km, &[A::SwitchToMode{input_mode: IM::Notifications}])),
        hint("Select pane", "Select", to_base_mode),
    ]} else if mi.mode == IM::Notifications { vec![
        hint("Select", "Select", action_key_group(&km, &[
            &[A::MoveNotificationSelection{direction: Dir::Down}],
            &[A::MoveNotificationSelection{direction: Dir::Up}]])),
        hint("Open", "Open", key_doing(&km, &A::ActivateNotification)),
        hint("Full text", "Full", key_doing(&km, &A::ToggleNotificationDetail)),
        hint("Exit", "Exit", escape_to_base_mode),
    ]} else if mi.mode == IM::Agent { vec![
        hint("New agent", "New", plugin_launch_key(&km, "new-agent")),
        hint("Duplicate agent", "Duplicate", plugin_launch_key(&km, "duplicate-agent")),
        hint("Fork agent", "Fork", plugin_launch_key(&km, "fork-agent")),
        hint("Agents", "Agents", plugin_launch_key(&km, "agents")),
        hint("Move panel", "Move", keybind_pipe_key(&km, "agent_strip_reposition")),
        hint("Select pane", "Select", to_base_mode),
    ]} else if mi.mode == IM::Tmux { vec![
        hint("Move focus", "Move", keys_doing_each(&km, &move_focus)),
        hint("Split down", "Down", key_doing(&km, &A::NewPane{direction: Some(Dir::Down), pane_name: None, start_suppressed: false})),
        hint("Split right", "Right", key_doing(&km, &A::NewPane{direction: Some(Dir::Right), pane_name: None, start_suppressed: false})),
        hint("Fullscreen", "Fullscreen", key_doing(&km, &A::ToggleFocusFullscreen)),
        hint("New tab", "New", key_doing(&km, &new_tab)),
        hint("Rename tab", "Rename", key_doing(&km, &A::SwitchToMode{input_mode: IM::RenameTab})),
        hint("Previous Tab", "Previous", key_doing(&km, &A::GoToPreviousTab)),
        hint("Next Tab", "Next", key_doing(&km, &A::GoToNextTab)),
        hint("Select pane", "Select", to_base_mode),
    ]} else if matches!(mi.mode, IM::RenamePane | IM::RenameTab) { vec![
        hint("When done", "Done", to_base_mode),
        hint("Select pane", "Select", keys_doing_each(&km, &move_focus)),
    ]} else { vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{plain_text, with_shipped_keybinds};
    use crate::{one_line_ui, second_line, TO_NORMAL};

    /// The modes that have hints, in the order they appear in [`hints_for_mode`].
    fn modes_with_hints() -> Vec<InputMode> {
        vec![
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Resize,
            InputMode::Move,
            InputMode::Scroll,
            InputMode::EnterSearch,
            InputMode::Search,
            InputMode::Session,
            InputMode::Notifications,
            InputMode::Agent,
            InputMode::Select,
            InputMode::Tmux,
            InputMode::RenamePane,
            InputMode::RenameTab,
        ]
    }

    fn describe(hints: &[Hint]) -> String {
        hints
            .iter()
            .map(|hint| {
                let keys = hint
                    .keys
                    .iter()
                    .map(|key| key.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} ({}): {}", hint.long, hint.short, keys)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_mode_has_the_same_hints_in_both_layouts() {
        let listing = modes_with_hints()
            .into_iter()
            .map(|mode| {
                let hints = hints_for_mode(&with_shipped_keybinds(mode));
                format!("== {:?}\n{}", mode, describe(&hints))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        insta::assert_snapshot!(listing);
    }

    #[test]
    fn classic_layout_renders_the_hints_as_text() {
        let listing = [
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Session,
            InputMode::Move,
        ]
        .into_iter()
        .map(|mode| {
            let mode_info = with_shipped_keybinds(mode);
            format!(
                "== {:?}\nwide:  {}\ntight: {}",
                mode,
                plain_text(second_line::keybinds(&mode_info, "quicknav", 500)),
                plain_text(second_line::keybinds(&mode_info, "quicknav", 60)),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
        insta::assert_snapshot!(listing);
    }

    #[test]
    fn one_line_layout_renders_the_hints_as_ribbons() {
        let listing = [
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Session,
            InputMode::Move,
        ]
        .into_iter()
        .map(|mode| {
            let mode_info = with_shipped_keybinds(mode);
            format!(
                "== {:?}\nwide:  {}\ntight: {}",
                mode,
                plain_text(one_line_ui::keybinds(&mode_info, 500)),
                plain_text(one_line_ui::keybinds(&mode_info, 60)),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
        insta::assert_snapshot!(listing);
    }

    // A locked base mode is configured by ending every binding with `SwitchToMode "Locked"`
    // instead of `"Normal"`. The hints are about what the binding does, not where it goes
    // afterwards, so they must come out the same - this is what the classic layout got wrong
    // while it had its own copy of the list.
    #[test]
    fn a_locked_base_mode_gets_the_same_hints() {
        let to_locked = Action::SwitchToMode {
            input_mode: InputMode::Locked,
        };
        for mode in modes_with_hints() {
            let with_normal_base = with_shipped_keybinds(mode);
            let mut with_locked_base = with_shipped_keybinds(mode);
            with_locked_base.update_default_mode(InputMode::Locked);
            for (_, binds) in with_locked_base.keybinds.iter_mut() {
                for (_, actions) in binds.iter_mut() {
                    for action in actions.iter_mut() {
                        if *action == TO_NORMAL {
                            *action = to_locked.clone();
                        }
                    }
                }
            }

            assert_eq!(
                hints_for_mode(&with_locked_base),
                hints_for_mode(&with_normal_base),
                "hints differ in {:?} mode",
                mode
            );
        }
    }

    // Ctrl o moves the panel only in Locked mode - everywhere else it enters Session mode - so
    // `Ctrl e o` is the way to move it, and Agent mode should say so. The key sends a message to
    // the running plugins rather than launching one, and it goes unmentioned when nothing binds it.
    #[test]
    fn agent_mode_names_the_key_that_moves_the_panel() {
        let move_panel = |mode_info: &ModeInfo| {
            hints_for_mode(mode_info)
                .into_iter()
                .find(|hint| hint.long == "Move panel")
                .expect("Agent mode has a Move panel hint")
                .keys
        };

        let shipped = with_shipped_keybinds(InputMode::Agent);
        assert_eq!(
            move_panel(&shipped),
            vec![KeyWithModifier::new(BareKey::Char('o'))]
        );

        let mut without_the_pipe = with_shipped_keybinds(InputMode::Agent);
        for (_, binds) in without_the_pipe.keybinds.iter_mut() {
            binds.retain(|(_, actions)| {
                !actions
                    .iter()
                    .any(|action| matches!(action, Action::KeybindPipe { .. }))
            });
        }
        assert_eq!(move_panel(&without_the_pipe), vec![]);
    }

    // Tab focus reads best as ←→, so each way names its arrow when that arrow goes that way -
    // whatever the binding does afterwards, and not merely because both arrows are bound.
    #[test]
    fn tab_focus_names_the_arrow_that_points_each_way() {
        let focus = |mode_info: &ModeInfo| {
            hints_for_mode(mode_info)
                .into_iter()
                .find(|hint| hint.long == "Change focus")
                .expect("Tab mode has a Change focus hint")
                .keys
        };
        let left = KeyWithModifier::new(BareKey::Left);
        let right = KeyWithModifier::new(BareKey::Right);

        let shipped = with_shipped_keybinds(InputMode::Tab);
        assert_eq!(focus(&shipped), vec![left.clone(), right.clone()]);

        let mut leaving_the_mode = with_shipped_keybinds(InputMode::Tab);
        for (_, binds) in leaving_the_mode.keybinds.iter_mut() {
            for (_, actions) in binds.iter_mut() {
                if matches!(
                    actions.first(),
                    Some(Action::GoToPreviousTab | Action::GoToNextTab)
                ) {
                    actions.push(TO_NORMAL);
                }
            }
        }
        assert_eq!(focus(&leaving_the_mode), vec![left.clone(), right.clone()]);

        let swapped = ModeInfo {
            mode: InputMode::Tab,
            keybinds: vec![(
                InputMode::Tab,
                vec![
                    (left.clone(), vec![Action::GoToNextTab]),
                    (right.clone(), vec![Action::GoToPreviousTab]),
                ],
            )],
            ..ModeInfo::default()
        };
        assert_eq!(focus(&swapped), vec![right, left]);
    }

    #[test]
    fn one_line_layout_never_claims_more_columns_than_it_was_given() {
        let mode_info = with_shipped_keybinds(InputMode::Pane);
        for max_width in 0..200 {
            let line_part = one_line_ui::keybinds(&mode_info, max_width);
            assert!(
                line_part.len <= max_width,
                "claimed {} columns of {}",
                line_part.len,
                max_width
            );
        }
    }
}
