/// The Agent-mode fragment shared by every preset. A plain (unmodified) entry key would
/// swallow typed text, so it is not bound in the mode keys are typed in - the exit mode,
/// which is Locked for the unlock-first preset and Normal for the presets without a primary
/// modifier - nor in the text-entry modes.
fn agent_keybinds(entry_key: &str, exit_mode: &str, plain_entry: bool) -> String {
    let excluded_modes = if plain_entry {
        let typing_mode = exit_mode.to_lowercase();
        let mut excluded = vec!["agent".to_owned(), "locked".to_owned()];
        if typing_mode != "locked" {
            excluded.push(typing_mode);
        }
        excluded.extend(
            ["renametab", "renamepane", "entersearch"]
                .into_iter()
                .map(str::to_owned),
        );
        excluded
            .iter()
            .map(|mode| format!("{mode:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        r#""agent" "locked""#.to_owned()
    };
    format!(
        r#"
    shared_except {excluded_modes} {{
        bind "{entry_key}" {{ SwitchToMode "Agent"; }}
    }}
    agent {{
        bind "{entry_key}" {{ SwitchToMode "Normal"; }}
        bind "Enter" "Esc" {{ SwitchToMode "{exit_mode}"; }}
        bind "n" {{
            LaunchOrFocusPlugin "new-agent" {{
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            }};
            SwitchToMode "{exit_mode}"
        }}
        bind "d" {{
            LaunchOrFocusPlugin "duplicate-agent" {{
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            }};
            SwitchToMode "{exit_mode}"
        }}
        bind "f" {{
            LaunchOrFocusPlugin "fork-agent" {{
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            }};
            SwitchToMode "{exit_mode}"
        }}
        bind "a" {{
            LaunchOrFocusPlugin "agents" {{
                floating true
                overlay true
                awaits_placement true
                move_to_focused_tab true
            }};
            SwitchToMode "{exit_mode}"
        }}
        bind "o" {{
            MessagePlugin {{ name "agent_strip_reposition"; }};
            SwitchToMode "{exit_mode}"
        }}
    }}
"#
    )
}

/// The Notifications-mode fragment shared by every preset: the browser's own keys. It is entered
/// with `n` from Session mode, bound in each preset's `session` block. Enter needs no mode switch:
/// the server leaves the mode itself once a notification is activated.
fn notifications_keybinds(exit_mode: &str) -> String {
    format!(
        r#"
    notifications {{
        bind "Esc" {{ SwitchToMode "{exit_mode}"; }}
        bind "Down" "j" {{ MoveNotificationSelection "Down"; }}
        bind "Up" "k" {{ MoveNotificationSelection "Up"; }}
        bind "Space" {{ ToggleNotificationDetail; }}
        bind "Enter" {{ ActivateNotification; }}
    }}
"#
    )
}

pub fn unlock_first_keybinds(primary_modifier: String, secondary_modifier: String) -> String {
    let agent_keybinds = agent_keybinds("e", "Locked", true);
    let notifications_keybinds = notifications_keybinds("Locked");
    format!(
        r#"
default_mode "locked"
keybinds clear-defaults=true {{
{agent_keybinds}
{notifications_keybinds}
    normal {{
    }}
    locked {{
        bind "{primary_modifier} g" {{ SwitchToMode "Normal"; }}
    }}
    resize {{
        bind "r" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ Resize "Increase Left"; }}
        bind "j" "Down" {{ Resize "Increase Down"; }}
        bind "k" "Up" {{ Resize "Increase Up"; }}
        bind "l" "Right" {{ Resize "Increase Right"; }}
        bind "H" {{ Resize "Decrease Left"; }}
        bind "J" {{ Resize "Decrease Down"; }}
        bind "K" {{ Resize "Decrease Up"; }}
        bind "L" {{ Resize "Decrease Right"; }}
        bind "=" "+" {{ Resize "Increase"; }}
        bind "-" {{ Resize "Decrease"; }}
    }}
    pane {{
        bind "p" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ MoveFocus "Left"; }}
        bind "l" "Right" {{ MoveFocus "Right"; }}
        bind "j" "Down" {{ MoveFocus "Down"; }}
        bind "k" "Up" {{ MoveFocus "Up"; }}
        bind "Tab" {{ SwitchFocus; }}
        bind ";" {{ FocusLastPane; }}
        bind "n" {{ NewPane; SwitchToMode "Locked"; }}
        bind "d" {{ NewPane "Down"; SwitchToMode "Locked"; }}
        bind "r" {{ NewPane "Right"; SwitchToMode "Locked"; }}
        bind "s" {{ NewPane "stacked"; SwitchToMode "Locked"; }}
        bind "x" {{ CloseFocus; SwitchToMode "Locked"; }}
        bind "f" {{ ToggleFocusFullscreen; SwitchToMode "Locked"; }}
        bind "Shift f" {{ ToggleFocusNoUiFullscreen; SwitchToMode "Locked"; }}
        bind "z" {{ TogglePaneFrames; SwitchToMode "Locked"; }}
        bind "w" {{ ToggleFloatingPanes; SwitchToMode "Locked"; }}
        bind "e" {{ TogglePaneEmbedOrFloating; SwitchToMode "Locked"; }}
        bind "c" {{ SwitchToMode "RenamePane"; PaneNameInput 0;}}
        bind "i" {{ TogglePanePinned; SwitchToMode "Locked"; }}
        bind "v" {{ SwitchToMode "Select"; }}
    }}
    select {{
        bind "h" "Left" {{ MoveTextSelection "Left"; }}
        bind "j" "Down" {{ MoveTextSelection "Down"; }}
        bind "k" "Up" {{ MoveTextSelection "Up"; }}
        bind "l" "Right" {{ MoveTextSelection "Right"; }}
        bind "o" {{ SwapTextSelectionEnds; }}
        bind "Enter" {{ Copy; SwitchToMode "Locked"; }}
        bind "Ctrl c" {{ SwitchToMode "Locked"; }}
    }}
    move {{
        bind "m" {{ SwitchToMode "Normal"; }}
        bind "n" "Tab" {{ MovePane; }}
        bind "p" {{ MovePaneBackwards; }}
        bind "h" "Left" {{ MovePane "Left"; }}
        bind "j" "Down" {{ MovePane "Down"; }}
        bind "k" "Up" {{ MovePane "Up"; }}
        bind "l" "Right" {{ MovePane "Right"; }}
    }}
    tab {{
        bind "t" {{ SwitchToMode "Normal"; }}
        bind "r" {{ SwitchToMode "RenameTab"; TabNameInput 0; }}
        bind "h" "Left" "Up" "k" {{ GoToPreviousTab; }}
        bind "l" "Right" "Down" "j" {{ GoToNextTab; }}
        bind "n" {{ NewTab; SwitchToMode "Locked"; }}
        bind "x" {{ CloseTab; SwitchToMode "Locked"; }}
        bind "s" {{ ToggleActiveSyncTab; SwitchToMode "Locked"; }}
        bind "b" {{ BreakPane; SwitchToMode "Locked"; }}
        bind "]" {{ BreakPaneRight; SwitchToMode "Locked"; }}
        bind "[" {{ BreakPaneLeft; SwitchToMode "Locked"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Locked"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Locked"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Locked"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Locked"; }}
        bind "5" {{ GoToTab 5; SwitchToMode "Locked"; }}
        bind "6" {{ GoToTab 6; SwitchToMode "Locked"; }}
        bind "7" {{ GoToTab 7; SwitchToMode "Locked"; }}
        bind "8" {{ GoToTab 8; SwitchToMode "Locked"; }}
        bind "9" {{ GoToTab 9; SwitchToMode "Locked"; }}
        bind "Tab" {{ ToggleTab; }}
    }}
    scroll {{
        bind "s" {{ SwitchToMode "Normal"; }}
        bind "e" {{ EditScrollback; SwitchToMode "Locked"; }}
        bind "f" {{ SwitchToMode "EnterSearch"; SearchInput 0; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Locked"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "Alt left" {{ MoveFocusOrTab "left"; SwitchToMode "locked"; }}
        bind "Alt down" {{ MoveFocus "down"; SwitchToMode "locked"; }}
        bind "Alt up" {{ MoveFocus "up"; SwitchToMode "locked"; }}
        bind "Alt right" {{ MoveFocusOrTab "right"; SwitchToMode "locked"; }}
        bind "Alt h" {{ MoveFocusOrTab "left"; SwitchToMode "locked"; }}
        bind "Alt j" {{ MoveFocus "down"; SwitchToMode "locked"; }}
        bind "Alt k" {{ MoveFocus "up"; SwitchToMode "locked"; }}
        bind "Alt l" {{ MoveFocusOrTab "right"; SwitchToMode "locked"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
        bind "c" {{ CopyLastCommandOutput; SwitchToMode "Locked"; }}
    }}
    search {{
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Locked"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "n" {{ Search "down"; }}
        bind "p" {{ Search "up"; }}
        bind "c" {{ SearchToggleOption "CaseSensitivity"; }}
        bind "w" {{ SearchToggleOption "Wrap"; }}
        bind "o" {{ SearchToggleOption "WholeWord"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
    }}
    entersearch {{
        bind "Ctrl c" "Esc" {{ SwitchToMode "Scroll"; }}
        bind "Enter" {{ SwitchToMode "Search"; }}
    }}
    renametab {{
        bind "Ctrl c" "Enter" {{ SwitchToMode "Locked"; }}
        bind "Esc" {{ UndoRenameTab; SwitchToMode "Tab"; }}
    }}
    renamepane {{
        bind "Ctrl c" "Enter" {{ SwitchToMode "Locked"; }}
        bind "Esc" {{ UndoRenamePane; SwitchToMode "Pane"; }}
    }}
    session {{
        bind "n" {{ SwitchToMode "Notifications"; }}
        bind "o" {{ SwitchToMode "Normal"; }}
        bind "d" {{ Detach; }}
        bind "]" {{ FocusHostSession; SwitchToMode "Locked"; }}
        bind "[" {{ FocusGuestSession; SwitchToMode "Locked"; }}
        bind "f" {{ ToggleHostFullscreen; SwitchToMode "Locked"; }}
        bind "w" {{
            LaunchOrFocusPlugin "session-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Locked"
        }}
        bind "c" {{
            LaunchOrFocusPlugin "configuration" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Locked"
        }}
        bind "p" {{
            LaunchOrFocusPlugin "plugin-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Locked"
        }}
        bind "l" {{
            LaunchOrFocusPlugin "yinz:layout-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Locked"
        }}
    }}
    shared_except "locked" "renametab" "renamepane" {{
        bind "{primary_modifier} g" {{ SwitchToMode "Locked"; }}
        bind "{primary_modifier} q" {{ Quit; }}
    }}
    shared_except "renamepane" "renametab" "entersearch" "locked" "notifications" {{
        bind "esc" {{ SwitchToMode "locked"; }}
    }}
    shared_among "normal" "locked" {{
        bind "{secondary_modifier} n" {{ NewPane; }}
        bind "{secondary_modifier} f" {{ ToggleFloatingPanes; }}
        bind "{secondary_modifier} i" {{ MoveTab "Left"; }}
        bind "{secondary_modifier} o" {{ MoveTab "Right"; }}
        bind "{secondary_modifier} h" "{secondary_modifier} Left" {{ MoveFocusOrTab "Left"; }}
        bind "{secondary_modifier} l" "{secondary_modifier} Right" {{ MoveFocusOrTab "Right"; }}
        bind "{secondary_modifier} j" "{secondary_modifier} Down" {{ MoveFocus "Down"; }}
        bind "{secondary_modifier} k" "{secondary_modifier} Up" {{ MoveFocus "Up"; }}
        bind "{secondary_modifier} =" "{secondary_modifier} +" {{ Resize "Increase"; }}
        bind "{secondary_modifier} -" {{ Resize "Decrease"; }}
        bind "{secondary_modifier} [" {{ PreviousSwapLayout; }}
        bind "{secondary_modifier} ]" {{ NextSwapLayout; }}
        bind "{secondary_modifier} p" {{ TogglePaneInGroup; }}
        bind "{secondary_modifier} Shift p" {{ ToggleGroupMarking; }}
    }}
    shared_except "locked" "renametab" "renamepane" "notifications" {{
        bind "Enter" {{ SwitchToMode "Locked"; }}
    }}
    shared_except "pane" "locked" "renametab" "renamepane" "entersearch" {{
        bind "p" {{ SwitchToMode "Pane"; }}
    }}
    shared_except "resize" "locked" "renametab" "renamepane" "entersearch" {{
        bind "r" {{ SwitchToMode "Resize"; }}
    }}
    shared_except "scroll" "locked" "renametab" "renamepane" "entersearch" {{
        bind "s" {{ SwitchToMode "Scroll"; }}
    }}
    shared_except "session" "locked" "renametab" "renamepane" "entersearch" {{
        bind "o" {{ SwitchToMode "Session"; }}
    }}
    shared_except "tab" "locked" "renametab" "renamepane" "entersearch" {{
        bind "t" {{ SwitchToMode "Tab"; }}
    }}
    shared_except "move" "locked" "renametab" "renamepane" "entersearch" {{
        bind "m" {{ SwitchToMode "Move"; }}
    }}
}}"#
    )
}

pub fn default_keybinds(primary_modifier: String, secondary_modifier: String) -> String {
    if primary_modifier.is_empty() && secondary_modifier.is_empty() {
        return default_keybinds_no_modifiers();
    } else if primary_modifier == secondary_modifier {
        return non_colliding_default_keybinds(primary_modifier, secondary_modifier);
    } else if primary_modifier.is_empty() {
        return default_keybinds_no_primary_modifier(secondary_modifier);
    } else if secondary_modifier.is_empty() {
        return default_keybinds_no_secondary_modifier(primary_modifier);
    }
    let agent_keybinds = agent_keybinds(&format!("{primary_modifier} e"), "Normal", false);
    let notifications_keybinds = notifications_keybinds("Normal");
    format!(
        r#"
default_mode "normal"
keybinds clear-defaults=true {{
{agent_keybinds}
{notifications_keybinds}
    normal {{}}
    locked {{
        bind "{primary_modifier} g" {{ SwitchToMode "Normal"; }}
    }}
    resize {{
        bind "{primary_modifier} n" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ Resize "Increase Left"; }}
        bind "j" "Down" {{ Resize "Increase Down"; }}
        bind "k" "Up" {{ Resize "Increase Up"; }}
        bind "l" "Right" {{ Resize "Increase Right"; }}
        bind "H" {{ Resize "Decrease Left"; }}
        bind "J" {{ Resize "Decrease Down"; }}
        bind "K" {{ Resize "Decrease Up"; }}
        bind "L" {{ Resize "Decrease Right"; }}
        bind "=" "+" {{ Resize "Increase"; }}
        bind "-" {{ Resize "Decrease"; }}
    }}
    pane {{
        bind "{primary_modifier} p" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ MoveFocus "Left"; }}
        bind "l" "Right" {{ MoveFocus "Right"; }}
        bind "j" "Down" {{ MoveFocus "Down"; }}
        bind "k" "Up" {{ MoveFocus "Up"; }}
        bind "p" {{ SwitchFocus; }}
        bind ";" {{ FocusLastPane; }}
        bind "n" {{ NewPane; SwitchToMode "Normal"; }}
        bind "d" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "r" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "s" {{ NewPane "stacked"; SwitchToMode "Normal"; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "Shift f" {{ ToggleFocusNoUiFullscreen; SwitchToMode "Normal"; }}
        bind "z" {{ TogglePaneFrames; SwitchToMode "Normal"; }}
        bind "w" {{ ToggleFloatingPanes; SwitchToMode "Normal"; }}
        bind "e" {{ TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }}
        bind "c" {{ SwitchToMode "RenamePane"; PaneNameInput 0;}}
        bind "i" {{ TogglePanePinned; SwitchToMode "Normal"; }}
        bind "v" {{ SwitchToMode "Select"; }}
    }}
    select {{
        bind "h" "Left" {{ MoveTextSelection "Left"; }}
        bind "j" "Down" {{ MoveTextSelection "Down"; }}
        bind "k" "Up" {{ MoveTextSelection "Up"; }}
        bind "l" "Right" {{ MoveTextSelection "Right"; }}
        bind "o" {{ SwapTextSelectionEnds; }}
        bind "Enter" {{ Copy; SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
    }}
    move {{
        bind "{primary_modifier} h" {{ SwitchToMode "Normal"; }}
        bind "n" "Tab" {{ MovePane; }}
        bind "p" {{ MovePaneBackwards; }}
        bind "h" "Left" {{ MovePane "Left"; }}
        bind "j" "Down" {{ MovePane "Down"; }}
        bind "k" "Up" {{ MovePane "Up"; }}
        bind "l" "Right" {{ MovePane "Right"; }}
    }}
    tab {{
        bind "{primary_modifier} t" {{ SwitchToMode "Normal"; }}
        bind "r" {{ SwitchToMode "RenameTab"; TabNameInput 0; }}
        bind "h" "Left" "Up" "k" {{ GoToPreviousTab; }}
        bind "l" "Right" "Down" "j" {{ GoToNextTab; }}
        bind "n" {{ NewTab; SwitchToMode "Normal"; }}
        bind "x" {{ CloseTab; SwitchToMode "Normal"; }}
        bind "s" {{ ToggleActiveSyncTab; SwitchToMode "Normal"; }}
        bind "b" {{ BreakPane; SwitchToMode "Normal"; }}
        bind "]" {{ BreakPaneRight; SwitchToMode "Normal"; }}
        bind "[" {{ BreakPaneLeft; SwitchToMode "Normal"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Normal"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Normal"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Normal"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Normal"; }}
        bind "5" {{ GoToTab 5; SwitchToMode "Normal"; }}
        bind "6" {{ GoToTab 6; SwitchToMode "Normal"; }}
        bind "7" {{ GoToTab 7; SwitchToMode "Normal"; }}
        bind "8" {{ GoToTab 8; SwitchToMode "Normal"; }}
        bind "9" {{ GoToTab 9; SwitchToMode "Normal"; }}
        bind "Tab" {{ ToggleTab; }}
    }}
    scroll {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "e" {{ EditScrollback; SwitchToMode "Normal"; }}
        bind "s" {{ SwitchToMode "EnterSearch"; SearchInput 0; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "{secondary_modifier} left" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} down" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} up" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} right" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} h" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} j" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} k" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} l" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
        bind "c" {{ CopyLastCommandOutput; SwitchToMode "Normal"; }}
    }}
    search {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "n" {{ Search "down"; }}
        bind "p" {{ Search "up"; }}
        bind "c" {{ SearchToggleOption "CaseSensitivity"; }}
        bind "w" {{ SearchToggleOption "Wrap"; }}
        bind "o" {{ SearchToggleOption "WholeWord"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
    }}
    entersearch {{
        bind "Ctrl c" "Esc" {{ SwitchToMode "Scroll"; }}
        bind "Enter" {{ SwitchToMode "Search"; }}
    }}
    renametab {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenameTab; SwitchToMode "Tab"; }}
    }}
    renamepane {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenamePane; SwitchToMode "Pane"; }}
    }}
    session {{
        bind "n" {{ SwitchToMode "Notifications"; }}
        bind "{primary_modifier} o" {{ SwitchToMode "Normal"; }}
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
        bind "d" {{ Detach; }}
        bind "]" {{ FocusHostSession; SwitchToMode "Normal"; }}
        bind "[" {{ FocusGuestSession; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleHostFullscreen; SwitchToMode "Normal"; }}
        bind "w" {{
            LaunchOrFocusPlugin "session-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "c" {{
            LaunchOrFocusPlugin "configuration" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "p" {{
            LaunchOrFocusPlugin "plugin-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "l" {{
            LaunchOrFocusPlugin "yinz:layout-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
    }}
    tmux {{
        bind "[" {{ SwitchToMode "Scroll"; }}
        bind "{primary_modifier} b" {{ Write 2; SwitchToMode "Normal"; }}
        bind "\"" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "%" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "z" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "c" {{ NewTab; SwitchToMode "Normal"; }}
        bind "," {{ SwitchToMode "RenameTab"; }}
        bind "p" {{ GoToPreviousTab; SwitchToMode "Normal"; }}
        bind "n" {{ GoToNextTab; SwitchToMode "Normal"; }}
        bind "Left" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "Right" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "Down" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "Up" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "h" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "l" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "j" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "k" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "o" {{ FocusNextPane; }}
        bind "d" {{ Detach; }}
        bind "Space" {{ NextSwapLayout; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
    }}
    shared_except "locked" "select" {{
        bind "{primary_modifier} g" {{ SwitchToMode "Locked"; }}
        bind "{primary_modifier} q" {{ Quit; }}
        bind "{secondary_modifier} f" {{ ToggleFloatingPanes; }}
        bind "{secondary_modifier} n" {{ NewPane; }}
        bind "{secondary_modifier} i" {{ MoveTab "Left"; }}
        bind "{secondary_modifier} o" {{ MoveTab "Right"; }}
        bind "{secondary_modifier} h" "{secondary_modifier} Left" {{ MoveFocusOrTab "Left"; }}
        bind "{secondary_modifier} l" "{secondary_modifier} Right" {{ MoveFocusOrTab "Right"; }}
        bind "{secondary_modifier} j" "{secondary_modifier} Down" {{ MoveFocus "Down"; }}
        bind "{secondary_modifier} k" "{secondary_modifier} Up" {{ MoveFocus "Up"; }}
        bind "{secondary_modifier} =" "{secondary_modifier} +" {{ Resize "Increase"; }}
        bind "{secondary_modifier} -" {{ Resize "Decrease"; }}
        bind "{secondary_modifier} [" {{ PreviousSwapLayout; }}
        bind "{secondary_modifier} ]" {{ NextSwapLayout; }}
        bind "{secondary_modifier} p" {{ TogglePaneInGroup; }}
        bind "{secondary_modifier} Shift p" {{ ToggleGroupMarking; }}
    }}
    shared_except "normal" "locked" "notifications" {{
        bind "Enter" "Esc" {{ SwitchToMode "Normal"; }}
    }}
    shared_except "pane" "locked" {{
        bind "{primary_modifier} p" {{ SwitchToMode "Pane"; }}
    }}
    shared_except "resize" "locked" {{
        bind "{primary_modifier} n" {{ SwitchToMode "Resize"; }}
    }}
    shared_except "scroll" "locked" {{
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
    }}
    shared_except "session" "locked" {{
        bind "{primary_modifier} o" {{ SwitchToMode "Session"; }}
    }}
    shared_except "tab" "locked" {{
        bind "{primary_modifier} t" {{ SwitchToMode "Tab"; }}
    }}
    shared_except "move" "locked" {{
        bind "{primary_modifier} h" {{ SwitchToMode "Move"; }}
    }}
    shared_except "tmux" "locked" {{
        bind "{primary_modifier} b" {{ SwitchToMode "Tmux"; }}
    }}
}}
"#
    )
}

pub fn default_keybinds_no_primary_modifier(secondary_modifier: String) -> String {
    let agent_keybinds = agent_keybinds("e", "Normal", true);
    let notifications_keybinds = notifications_keybinds("Normal");
    format!(
        r#"
default_mode "normal"
keybinds clear-defaults=true {{
{agent_keybinds}
{notifications_keybinds}
    normal {{}}
    locked {{}}
    resize {{
        bind "h" "Left" {{ Resize "Increase Left"; }}
        bind "j" "Down" {{ Resize "Increase Down"; }}
        bind "k" "Up" {{ Resize "Increase Up"; }}
        bind "l" "Right" {{ Resize "Increase Right"; }}
        bind "H" {{ Resize "Decrease Left"; }}
        bind "J" {{ Resize "Decrease Down"; }}
        bind "K" {{ Resize "Decrease Up"; }}
        bind "L" {{ Resize "Decrease Right"; }}
        bind "=" "+" {{ Resize "Increase"; }}
        bind "-" {{ Resize "Decrease"; }}
    }}
    pane {{
        bind "h" "Left" {{ MoveFocus "Left"; }}
        bind "l" "Right" {{ MoveFocus "Right"; }}
        bind "j" "Down" {{ MoveFocus "Down"; }}
        bind "k" "Up" {{ MoveFocus "Up"; }}
        bind "p" {{ SwitchFocus; }}
        bind ";" {{ FocusLastPane; }}
        bind "n" {{ NewPane; SwitchToMode "Normal"; }}
        bind "d" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "r" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "s" {{ NewPane "stacked"; SwitchToMode "Normal"; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "Shift f" {{ ToggleFocusNoUiFullscreen; SwitchToMode "Normal"; }}
        bind "z" {{ TogglePaneFrames; SwitchToMode "Normal"; }}
        bind "w" {{ ToggleFloatingPanes; SwitchToMode "Normal"; }}
        bind "e" {{ TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }}
        bind "c" {{ SwitchToMode "RenamePane"; PaneNameInput 0;}}
        bind "i" {{ TogglePanePinned; SwitchToMode "Normal"; }}
        bind "v" {{ SwitchToMode "Select"; }}
    }}
    select {{
        bind "h" "Left" {{ MoveTextSelection "Left"; }}
        bind "j" "Down" {{ MoveTextSelection "Down"; }}
        bind "k" "Up" {{ MoveTextSelection "Up"; }}
        bind "l" "Right" {{ MoveTextSelection "Right"; }}
        bind "o" {{ SwapTextSelectionEnds; }}
        bind "Enter" {{ Copy; SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
    }}
    move {{
        bind "n" "Tab" {{ MovePane; }}
        bind "p" {{ MovePaneBackwards; }}
        bind "h" "Left" {{ MovePane "Left"; }}
        bind "j" "Down" {{ MovePane "Down"; }}
        bind "k" "Up" {{ MovePane "Up"; }}
        bind "l" "Right" {{ MovePane "Right"; }}
    }}
    tab {{
        bind "r" {{ SwitchToMode "RenameTab"; TabNameInput 0; }}
        bind "h" "Left" "Up" "k" {{ GoToPreviousTab; }}
        bind "l" "Right" "Down" "j" {{ GoToNextTab; }}
        bind "n" {{ NewTab; SwitchToMode "Normal"; }}
        bind "x" {{ CloseTab; SwitchToMode "Normal"; }}
        bind "s" {{ ToggleActiveSyncTab; SwitchToMode "Normal"; }}
        bind "b" {{ BreakPane; SwitchToMode "Normal"; }}
        bind "]" {{ BreakPaneRight; SwitchToMode "Normal"; }}
        bind "[" {{ BreakPaneLeft; SwitchToMode "Normal"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Normal"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Normal"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Normal"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Normal"; }}
        bind "5" {{ GoToTab 5; SwitchToMode "Normal"; }}
        bind "6" {{ GoToTab 6; SwitchToMode "Normal"; }}
        bind "7" {{ GoToTab 7; SwitchToMode "Normal"; }}
        bind "8" {{ GoToTab 8; SwitchToMode "Normal"; }}
        bind "9" {{ GoToTab 9; SwitchToMode "Normal"; }}
        bind "Tab" {{ ToggleTab; }}
    }}
    scroll {{
        bind "e" {{ EditScrollback; SwitchToMode "Normal"; }}
        bind "s" {{ SwitchToMode "EnterSearch"; SearchInput 0; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "{secondary_modifier} left" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} down" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} up" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} right" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} h" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} j" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} k" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} l" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
        bind "c" {{ CopyLastCommandOutput; SwitchToMode "Normal"; }}
    }}
    search {{
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "n" {{ Search "down"; }}
        bind "p" {{ Search "up"; }}
        bind "c" {{ SearchToggleOption "CaseSensitivity"; }}
        bind "w" {{ SearchToggleOption "Wrap"; }}
        bind "o" {{ SearchToggleOption "WholeWord"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
    }}
    entersearch {{
        bind "Ctrl c" "Esc" {{ SwitchToMode "Scroll"; }}
        bind "Enter" {{ SwitchToMode "Search"; }}
    }}
    renametab {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenameTab; SwitchToMode "Tab"; }}
    }}
    renamepane {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenamePane; SwitchToMode "Pane"; }}
    }}
    session {{
        bind "n" {{ SwitchToMode "Notifications"; }}
        bind "d" {{ Detach; }}
        bind "]" {{ FocusHostSession; SwitchToMode "Normal"; }}
        bind "[" {{ FocusGuestSession; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleHostFullscreen; SwitchToMode "Normal"; }}
        bind "w" {{
            LaunchOrFocusPlugin "session-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "c" {{
            LaunchOrFocusPlugin "configuration" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "p" {{
            LaunchOrFocusPlugin "plugin-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "l" {{
            LaunchOrFocusPlugin "yinz:layout-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
    }}
    tmux {{
        bind "[" {{ SwitchToMode "Scroll"; }}
        bind "\"" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "%" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "z" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "c" {{ NewTab; SwitchToMode "Normal"; }}
        bind "," {{ SwitchToMode "RenameTab"; }}
        bind "p" {{ GoToPreviousTab; SwitchToMode "Normal"; }}
        bind "n" {{ GoToNextTab; SwitchToMode "Normal"; }}
        bind "Left" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "Right" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "Down" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "Up" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "h" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "l" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "j" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "k" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "o" {{ FocusNextPane; }}
        bind "d" {{ Detach; }}
        bind "Space" {{ NextSwapLayout; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
    }}
    shared_except "locked" "select" {{
        bind "{secondary_modifier} n" {{ NewPane; }}
        bind "{secondary_modifier} f" {{ ToggleFloatingPanes; }}
        bind "{secondary_modifier} i" {{ MoveTab "Left"; }}
        bind "{secondary_modifier} o" {{ MoveTab "Right"; }}
        bind "{secondary_modifier} h" "{secondary_modifier} Left" {{ MoveFocusOrTab "Left"; }}
        bind "{secondary_modifier} l" "{secondary_modifier} Right" {{ MoveFocusOrTab "Right"; }}
        bind "{secondary_modifier} j" "{secondary_modifier} Down" {{ MoveFocus "Down"; }}
        bind "{secondary_modifier} k" "{secondary_modifier} Up" {{ MoveFocus "Up"; }}
        bind "{secondary_modifier} =" "{secondary_modifier} +" {{ Resize "Increase"; }}
        bind "{secondary_modifier} -" {{ Resize "Decrease"; }}
        bind "{secondary_modifier} [" {{ PreviousSwapLayout; }}
        bind "{secondary_modifier} ]" {{ NextSwapLayout; }}
        bind "{secondary_modifier} p" {{ TogglePaneInGroup; }}
        bind "{secondary_modifier} Shift p" {{ ToggleGroupMarking; }}
    }}
    shared_except "normal" "locked" "notifications" {{
        bind "Enter" "Esc" {{ SwitchToMode "Normal"; }}
    }}
}}
"#
    )
}

pub fn default_keybinds_no_secondary_modifier(primary_modifier: String) -> String {
    let agent_keybinds = agent_keybinds(&format!("{primary_modifier} e"), "Normal", false);
    let notifications_keybinds = notifications_keybinds("Normal");
    format!(
        r#"
default_mode "normal"
keybinds clear-defaults=true {{
{agent_keybinds}
{notifications_keybinds}
    normal {{}}
    locked {{
        bind "{primary_modifier} g" {{ SwitchToMode "Normal"; }}
    }}
    resize {{
        bind "{primary_modifier} n" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ Resize "Increase Left"; }}
        bind "j" "Down" {{ Resize "Increase Down"; }}
        bind "k" "Up" {{ Resize "Increase Up"; }}
        bind "l" "Right" {{ Resize "Increase Right"; }}
        bind "H" {{ Resize "Decrease Left"; }}
        bind "J" {{ Resize "Decrease Down"; }}
        bind "K" {{ Resize "Decrease Up"; }}
        bind "L" {{ Resize "Decrease Right"; }}
        bind "=" "+" {{ Resize "Increase"; }}
        bind "-" {{ Resize "Decrease"; }}
    }}
    pane {{
        bind "{primary_modifier} p" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ MoveFocus "Left"; }}
        bind "l" "Right" {{ MoveFocus "Right"; }}
        bind "j" "Down" {{ MoveFocus "Down"; }}
        bind "k" "Up" {{ MoveFocus "Up"; }}
        bind "p" {{ SwitchFocus; }}
        bind ";" {{ FocusLastPane; }}
        bind "n" {{ NewPane; SwitchToMode "Normal"; }}
        bind "d" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "r" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "s" {{ NewPane "stacked"; SwitchToMode "Normal"; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "Shift f" {{ ToggleFocusNoUiFullscreen; SwitchToMode "Normal"; }}
        bind "z" {{ TogglePaneFrames; SwitchToMode "Normal"; }}
        bind "w" {{ ToggleFloatingPanes; SwitchToMode "Normal"; }}
        bind "e" {{ TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }}
        bind "c" {{ SwitchToMode "RenamePane"; PaneNameInput 0;}}
        bind "i" {{ TogglePanePinned; SwitchToMode "Normal"; }}
        bind "v" {{ SwitchToMode "Select"; }}
    }}
    select {{
        bind "h" "Left" {{ MoveTextSelection "Left"; }}
        bind "j" "Down" {{ MoveTextSelection "Down"; }}
        bind "k" "Up" {{ MoveTextSelection "Up"; }}
        bind "l" "Right" {{ MoveTextSelection "Right"; }}
        bind "o" {{ SwapTextSelectionEnds; }}
        bind "Enter" {{ Copy; SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
    }}
    move {{
        bind "{primary_modifier} h" {{ SwitchToMode "Normal"; }}
        bind "n" "Tab" {{ MovePane; }}
        bind "p" {{ MovePaneBackwards; }}
        bind "h" "Left" {{ MovePane "Left"; }}
        bind "j" "Down" {{ MovePane "Down"; }}
        bind "k" "Up" {{ MovePane "Up"; }}
        bind "l" "Right" {{ MovePane "Right"; }}
    }}
    tab {{
        bind "{primary_modifier} t" {{ SwitchToMode "Normal"; }}
        bind "r" {{ SwitchToMode "RenameTab"; TabNameInput 0; }}
        bind "h" "Left" "Up" "k" {{ GoToPreviousTab; }}
        bind "l" "Right" "Down" "j" {{ GoToNextTab; }}
        bind "n" {{ NewTab; SwitchToMode "Normal"; }}
        bind "x" {{ CloseTab; SwitchToMode "Normal"; }}
        bind "s" {{ ToggleActiveSyncTab; SwitchToMode "Normal"; }}
        bind "b" {{ BreakPane; SwitchToMode "Normal"; }}
        bind "]" {{ BreakPaneRight; SwitchToMode "Normal"; }}
        bind "[" {{ BreakPaneLeft; SwitchToMode "Normal"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Normal"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Normal"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Normal"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Normal"; }}
        bind "5" {{ GoToTab 5; SwitchToMode "Normal"; }}
        bind "6" {{ GoToTab 6; SwitchToMode "Normal"; }}
        bind "7" {{ GoToTab 7; SwitchToMode "Normal"; }}
        bind "8" {{ GoToTab 8; SwitchToMode "Normal"; }}
        bind "9" {{ GoToTab 9; SwitchToMode "Normal"; }}
        bind "Tab" {{ ToggleTab; }}
    }}
    scroll {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "e" {{ EditScrollback; SwitchToMode "Normal"; }}
        bind "s" {{ SwitchToMode "EnterSearch"; SearchInput 0; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
        bind "c" {{ CopyLastCommandOutput; SwitchToMode "Normal"; }}
    }}
    search {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "n" {{ Search "down"; }}
        bind "p" {{ Search "up"; }}
        bind "c" {{ SearchToggleOption "CaseSensitivity"; }}
        bind "w" {{ SearchToggleOption "Wrap"; }}
        bind "o" {{ SearchToggleOption "WholeWord"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
    }}
    entersearch {{
        bind "Ctrl c" "Esc" {{ SwitchToMode "Scroll"; }}
        bind "Enter" {{ SwitchToMode "Search"; }}
    }}
    renametab {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenameTab; SwitchToMode "Tab"; }}
    }}
    renamepane {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenamePane; SwitchToMode "Pane"; }}
    }}
    session {{
        bind "n" {{ SwitchToMode "Notifications"; }}
        bind "{primary_modifier} o" {{ SwitchToMode "Normal"; }}
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
        bind "d" {{ Detach; }}
        bind "]" {{ FocusHostSession; SwitchToMode "Normal"; }}
        bind "[" {{ FocusGuestSession; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleHostFullscreen; SwitchToMode "Normal"; }}
        bind "w" {{
            LaunchOrFocusPlugin "session-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "c" {{
            LaunchOrFocusPlugin "configuration" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "p" {{
            LaunchOrFocusPlugin "plugin-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "l" {{
            LaunchOrFocusPlugin "yinz:layout-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
    }}
    tmux {{
        bind "[" {{ SwitchToMode "Scroll"; }}
        bind "{primary_modifier} b" {{ Write 2; SwitchToMode "Normal"; }}
        bind "\"" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "%" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "z" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "c" {{ NewTab; SwitchToMode "Normal"; }}
        bind "," {{ SwitchToMode "RenameTab"; }}
        bind "p" {{ GoToPreviousTab; SwitchToMode "Normal"; }}
        bind "n" {{ GoToNextTab; SwitchToMode "Normal"; }}
        bind "Left" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "Right" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "Down" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "Up" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "h" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "l" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "j" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "k" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "o" {{ FocusNextPane; }}
        bind "d" {{ Detach; }}
        bind "Space" {{ NextSwapLayout; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
    }}
    shared_except "locked" {{
        bind "{primary_modifier} g" {{ SwitchToMode "Locked"; }}
        bind "{primary_modifier} q" {{ Quit; }}
    }}
    shared_except "normal" "locked" "notifications" {{
        bind "Enter" "Esc" {{ SwitchToMode "Normal"; }}
    }}
    shared_except "pane" "locked" {{
        bind "{primary_modifier} p" {{ SwitchToMode "Pane"; }}
    }}
    shared_except "resize" "locked" {{
        bind "{primary_modifier} n" {{ SwitchToMode "Resize"; }}
    }}
    shared_except "scroll" "locked" {{
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
    }}
    shared_except "session" "locked" {{
        bind "{primary_modifier} o" {{ SwitchToMode "Session"; }}
    }}
    shared_except "tab" "locked" {{
        bind "{primary_modifier} t" {{ SwitchToMode "Tab"; }}
    }}
    shared_except "move" "locked" {{
        bind "{primary_modifier} h" {{ SwitchToMode "Move"; }}
    }}
    shared_except "tmux" "locked" {{
        bind "{primary_modifier} b" {{ SwitchToMode "Tmux"; }}
    }}
}}
"#
    )
}

pub fn default_keybinds_no_modifiers() -> String {
    [
        r#"
default_mode "normal"
keybinds clear-defaults=true {
"#,
        &agent_keybinds("e", "Normal", true),
        &notifications_keybinds("Normal"),
        r#"    normal {}
    locked {}
    resize {
        bind "h" "Left" { Resize "Increase Left"; }
        bind "j" "Down" { Resize "Increase Down"; }
        bind "k" "Up" { Resize "Increase Up"; }
        bind "l" "Right" { Resize "Increase Right"; }
        bind "H" { Resize "Decrease Left"; }
        bind "J" { Resize "Decrease Down"; }
        bind "K" { Resize "Decrease Up"; }
        bind "L" { Resize "Decrease Right"; }
        bind "=" "+" { Resize "Increase"; }
        bind "-" { Resize "Decrease"; }
    }
    pane {
        bind "h" "Left" { MoveFocus "Left"; }
        bind "l" "Right" { MoveFocus "Right"; }
        bind "j" "Down" { MoveFocus "Down"; }
        bind "k" "Up" { MoveFocus "Up"; }
        bind "p" { SwitchFocus; }
        bind ";" { FocusLastPane; }
        bind "n" { NewPane; SwitchToMode "Normal"; }
        bind "d" { NewPane "Down"; SwitchToMode "Normal"; }
        bind "r" { NewPane "Right"; SwitchToMode "Normal"; }
        bind "s" { NewPane "stacked"; SwitchToMode "Normal"; }
        bind "x" { CloseFocus; SwitchToMode "Normal"; }
        bind "f" { ToggleFocusFullscreen; SwitchToMode "Normal"; }
        bind "Shift f" { ToggleFocusNoUiFullscreen; SwitchToMode "Normal"; }
        bind "z" { TogglePaneFrames; SwitchToMode "Normal"; }
        bind "w" { ToggleFloatingPanes; SwitchToMode "Normal"; }
        bind "e" { TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }
        bind "c" { SwitchToMode "RenamePane"; PaneNameInput 0;}
        bind "i" { TogglePanePinned; SwitchToMode "Normal"; }
        bind "v" { SwitchToMode "Select"; }
    }
    select {
        bind "h" "Left" { MoveTextSelection "Left"; }
        bind "j" "Down" { MoveTextSelection "Down"; }
        bind "k" "Up" { MoveTextSelection "Up"; }
        bind "l" "Right" { MoveTextSelection "Right"; }
        bind "o" { SwapTextSelectionEnds; }
        bind "Enter" { Copy; SwitchToMode "Normal"; }
        bind "Ctrl c" { SwitchToMode "Normal"; }
    }
    move {
        bind "n" "Tab" { MovePane; }
        bind "p" { MovePaneBackwards; }
        bind "h" "Left" { MovePane "Left"; }
        bind "j" "Down" { MovePane "Down"; }
        bind "k" "Up" { MovePane "Up"; }
        bind "l" "Right" { MovePane "Right"; }
    }
    tab {
        bind "r" { SwitchToMode "RenameTab"; TabNameInput 0; }
        bind "h" "Left" "Up" "k" { GoToPreviousTab; }
        bind "l" "Right" "Down" "j" { GoToNextTab; }
        bind "n" { NewTab; SwitchToMode "Normal"; }
        bind "x" { CloseTab; SwitchToMode "Normal"; }
        bind "s" { ToggleActiveSyncTab; SwitchToMode "Normal"; }
        bind "b" { BreakPane; SwitchToMode "Normal"; }
        bind "]" { BreakPaneRight; SwitchToMode "Normal"; }
        bind "[" { BreakPaneLeft; SwitchToMode "Normal"; }
        bind "1" { GoToTab 1; SwitchToMode "Normal"; }
        bind "2" { GoToTab 2; SwitchToMode "Normal"; }
        bind "3" { GoToTab 3; SwitchToMode "Normal"; }
        bind "4" { GoToTab 4; SwitchToMode "Normal"; }
        bind "5" { GoToTab 5; SwitchToMode "Normal"; }
        bind "6" { GoToTab 6; SwitchToMode "Normal"; }
        bind "7" { GoToTab 7; SwitchToMode "Normal"; }
        bind "8" { GoToTab 8; SwitchToMode "Normal"; }
        bind "9" { GoToTab 9; SwitchToMode "Normal"; }
        bind "Tab" { ToggleTab; }
    }
    scroll {
        bind "e" { EditScrollback; SwitchToMode "Normal"; }
        bind "s" { SwitchToMode "EnterSearch"; SearchInput 0; }
        bind "Ctrl c" { ScrollToBottom; SwitchToMode "Normal"; }
        bind "j" "Down" { ScrollDown; }
        bind "k" "Up" { ScrollUp; }
        bind "Ctrl f" "PageDown" "Right" "l" { PageScrollDown; }
        bind "Ctrl b" "PageUp" "Left" "h" { PageScrollUp; }
        bind "d" { HalfPageScrollDown; }
        bind "u" { HalfPageScrollUp; }
        bind "[" { ScrollToPreviousPrompt; }
        bind "]" { ScrollToNextPrompt; }
        bind "m" { SelectCommandAtScrollPosition; }
        bind "c" { CopyLastCommandOutput; SwitchToMode "Normal"; }
    }
    search {
        bind "Ctrl c" { ScrollToBottom; SwitchToMode "Normal"; }
        bind "j" "Down" { ScrollDown; }
        bind "k" "Up" { ScrollUp; }
        bind "Ctrl f" "PageDown" "Right" "l" { PageScrollDown; }
        bind "Ctrl b" "PageUp" "Left" "h" { PageScrollUp; }
        bind "d" { HalfPageScrollDown; }
        bind "u" { HalfPageScrollUp; }
        bind "n" { Search "down"; }
        bind "p" { Search "up"; }
        bind "c" { SearchToggleOption "CaseSensitivity"; }
        bind "w" { SearchToggleOption "Wrap"; }
        bind "o" { SearchToggleOption "WholeWord"; }
        bind "[" { ScrollToPreviousPrompt; }
        bind "]" { ScrollToNextPrompt; }
        bind "m" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind "Ctrl c" "Esc" { SwitchToMode "Scroll"; }
        bind "Enter" { SwitchToMode "Search"; }
    }
    renametab {
        bind "Ctrl c" { SwitchToMode "Normal"; }
        bind "Esc" { UndoRenameTab; SwitchToMode "Tab"; }
    }
    renamepane {
        bind "Ctrl c" { SwitchToMode "Normal"; }
        bind "Esc" { UndoRenamePane; SwitchToMode "Pane"; }
    }
    session {
        bind "n" { SwitchToMode "Notifications"; }
        bind "d" { Detach; }
        bind "]" { FocusHostSession; SwitchToMode "Normal"; }
        bind "[" { FocusGuestSession; SwitchToMode "Normal"; }
        bind "f" { ToggleHostFullscreen; SwitchToMode "Normal"; }
        bind "w" {
            LaunchOrFocusPlugin "session-manager" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode "Normal"
        }
        bind "c" {
            LaunchOrFocusPlugin "configuration" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode "Normal"
        }
        bind "p" {
            LaunchOrFocusPlugin "plugin-manager" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode "Normal"
        }
        bind "l" {
            LaunchOrFocusPlugin "yinz:layout-manager" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode "Normal"
        }
    }
    tmux {
        bind "[" { SwitchToMode "Scroll"; }
        bind "\"" { NewPane "Down"; SwitchToMode "Normal"; }
        bind "%" { NewPane "Right"; SwitchToMode "Normal"; }
        bind "z" { ToggleFocusFullscreen; SwitchToMode "Normal"; }
        bind "c" { NewTab; SwitchToMode "Normal"; }
        bind "," { SwitchToMode "RenameTab"; }
        bind "p" { GoToPreviousTab; SwitchToMode "Normal"; }
        bind "n" { GoToNextTab; SwitchToMode "Normal"; }
        bind "Left" { MoveFocus "Left"; SwitchToMode "Normal"; }
        bind "Right" { MoveFocus "Right"; SwitchToMode "Normal"; }
        bind "Down" { MoveFocus "Down"; SwitchToMode "Normal"; }
        bind "Up" { MoveFocus "Up"; SwitchToMode "Normal"; }
        bind "h" { MoveFocus "Left"; SwitchToMode "Normal"; }
        bind "l" { MoveFocus "Right"; SwitchToMode "Normal"; }
        bind "j" { MoveFocus "Down"; SwitchToMode "Normal"; }
        bind "k" { MoveFocus "Up"; SwitchToMode "Normal"; }
        bind "o" { FocusNextPane; }
        bind "d" { Detach; }
        bind "Space" { NextSwapLayout; }
        bind "x" { CloseFocus; SwitchToMode "Normal"; }
    }
    shared_except "normal" "locked" "notifications" {
        bind "Enter" "Esc" { SwitchToMode "Normal"; }
    }
}
"#,
    ]
    .concat()
}

pub fn non_colliding_default_keybinds(
    primary_modifier: String,
    secondary_modifier: String,
) -> String {
    let agent_keybinds = agent_keybinds(&format!("{primary_modifier} e"), "Normal", false);
    let notifications_keybinds = notifications_keybinds("Normal");
    format!(
        r#"
default_mode "normal"
keybinds clear-defaults=true {{
{agent_keybinds}
{notifications_keybinds}
    normal {{}}
    locked {{
        bind "{primary_modifier} g" {{ SwitchToMode "Normal"; }}
    }}
    resize {{
        bind "{primary_modifier} r" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ Resize "Increase Left"; }}
        bind "j" "Down" {{ Resize "Increase Down"; }}
        bind "k" "Up" {{ Resize "Increase Up"; }}
        bind "l" "Right" {{ Resize "Increase Right"; }}
        bind "H" {{ Resize "Decrease Left"; }}
        bind "J" {{ Resize "Decrease Down"; }}
        bind "K" {{ Resize "Decrease Up"; }}
        bind "L" {{ Resize "Decrease Right"; }}
        bind "=" "+" {{ Resize "Increase"; }}
        bind "-" {{ Resize "Decrease"; }}
    }}
    pane {{
        bind "{primary_modifier} p" {{ SwitchToMode "Normal"; }}
        bind "h" "Left" {{ MoveFocus "Left"; }}
        bind "l" "Right" {{ MoveFocus "Right"; }}
        bind "j" "Down" {{ MoveFocus "Down"; }}
        bind "k" "Up" {{ MoveFocus "Up"; }}
        bind "p" {{ SwitchFocus; }}
        bind ";" {{ FocusLastPane; }}
        bind "n" {{ NewPane; SwitchToMode "Normal"; }}
        bind "d" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "r" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "s" {{ NewPane "stacked"; SwitchToMode "Normal"; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "Shift f" {{ ToggleFocusNoUiFullscreen; SwitchToMode "Normal"; }}
        bind "z" {{ TogglePaneFrames; SwitchToMode "Normal"; }}
        bind "w" {{ ToggleFloatingPanes; SwitchToMode "Normal"; }}
        bind "e" {{ TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }}
        bind "c" {{ SwitchToMode "RenamePane"; PaneNameInput 0;}}
        bind "i" {{ TogglePanePinned; SwitchToMode "Normal"; }}
        bind "v" {{ SwitchToMode "Select"; }}
    }}
    select {{
        bind "h" "Left" {{ MoveTextSelection "Left"; }}
        bind "j" "Down" {{ MoveTextSelection "Down"; }}
        bind "k" "Up" {{ MoveTextSelection "Up"; }}
        bind "l" "Right" {{ MoveTextSelection "Right"; }}
        bind "o" {{ SwapTextSelectionEnds; }}
        bind "Enter" {{ Copy; SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
    }}
    move {{
        bind "{primary_modifier} m" {{ SwitchToMode "Normal"; }}
        bind "n" "Tab" {{ MovePane; }}
        bind "p" {{ MovePaneBackwards; }}
        bind "h" "Left" {{ MovePane "Left"; }}
        bind "j" "Down" {{ MovePane "Down"; }}
        bind "k" "Up" {{ MovePane "Up"; }}
        bind "l" "Right" {{ MovePane "Right"; }}
    }}
    tab {{
        bind "{primary_modifier} t" {{ SwitchToMode "Normal"; }}
        bind "r" {{ SwitchToMode "RenameTab"; TabNameInput 0; }}
        bind "h" "Left" "Up" "k" {{ GoToPreviousTab; }}
        bind "l" "Right" "Down" "j" {{ GoToNextTab; }}
        bind "n" {{ NewTab; SwitchToMode "Normal"; }}
        bind "x" {{ CloseTab; SwitchToMode "Normal"; }}
        bind "s" {{ ToggleActiveSyncTab; SwitchToMode "Normal"; }}
        bind "b" {{ BreakPane; SwitchToMode "Normal"; }}
        bind "]" {{ BreakPaneRight; SwitchToMode "Normal"; }}
        bind "[" {{ BreakPaneLeft; SwitchToMode "Normal"; }}
        bind "1" {{ GoToTab 1; SwitchToMode "Normal"; }}
        bind "2" {{ GoToTab 2; SwitchToMode "Normal"; }}
        bind "3" {{ GoToTab 3; SwitchToMode "Normal"; }}
        bind "4" {{ GoToTab 4; SwitchToMode "Normal"; }}
        bind "5" {{ GoToTab 5; SwitchToMode "Normal"; }}
        bind "6" {{ GoToTab 6; SwitchToMode "Normal"; }}
        bind "7" {{ GoToTab 7; SwitchToMode "Normal"; }}
        bind "8" {{ GoToTab 8; SwitchToMode "Normal"; }}
        bind "9" {{ GoToTab 9; SwitchToMode "Normal"; }}
        bind "Tab" {{ ToggleTab; }}
    }}
    scroll {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "e" {{ EditScrollback; SwitchToMode "Normal"; }}
        bind "s" {{ SwitchToMode "EnterSearch"; SearchInput 0; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "{secondary_modifier} left" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} down" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} up" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} right" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} h" {{ MoveFocusOrTab "left"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} j" {{ MoveFocus "down"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} k" {{ MoveFocus "up"; SwitchToMode "normal"; }}
        bind "{secondary_modifier} l" {{ MoveFocusOrTab "right"; SwitchToMode "normal"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
        bind "c" {{ CopyLastCommandOutput; SwitchToMode "Normal"; }}
    }}
    search {{
        bind "{primary_modifier} s" {{ SwitchToMode "Normal"; }}
        bind "Ctrl c" {{ ScrollToBottom; SwitchToMode "Normal"; }}
        bind "j" "Down" {{ ScrollDown; }}
        bind "k" "Up" {{ ScrollUp; }}
        bind "Ctrl f" "PageDown" "Right" "l" {{ PageScrollDown; }}
        bind "Ctrl b" "PageUp" "Left" "h" {{ PageScrollUp; }}
        bind "d" {{ HalfPageScrollDown; }}
        bind "u" {{ HalfPageScrollUp; }}
        bind "n" {{ Search "down"; }}
        bind "p" {{ Search "up"; }}
        bind "c" {{ SearchToggleOption "CaseSensitivity"; }}
        bind "w" {{ SearchToggleOption "Wrap"; }}
        bind "o" {{ SearchToggleOption "WholeWord"; }}
        bind "[" {{ ScrollToPreviousPrompt; }}
        bind "]" {{ ScrollToNextPrompt; }}
        bind "m" {{ SelectCommandAtScrollPosition; }}
    }}
    entersearch {{
        bind "Ctrl c" "Esc" {{ SwitchToMode "Scroll"; }}
        bind "Enter" {{ SwitchToMode "Search"; }}
    }}
    renametab {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenameTab; SwitchToMode "Tab"; }}
    }}
    renamepane {{
        bind "Ctrl c" {{ SwitchToMode "Normal"; }}
        bind "Esc" {{ UndoRenamePane; SwitchToMode "Pane"; }}
    }}
    session {{
        bind "n" {{ SwitchToMode "Notifications"; }}
        bind "{primary_modifier} o" {{ SwitchToMode "Normal"; }}
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
        bind "d" {{ Detach; }}
        bind "]" {{ FocusHostSession; SwitchToMode "Normal"; }}
        bind "[" {{ FocusGuestSession; SwitchToMode "Normal"; }}
        bind "f" {{ ToggleHostFullscreen; SwitchToMode "Normal"; }}
        bind "w" {{
            LaunchOrFocusPlugin "session-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "c" {{
            LaunchOrFocusPlugin "configuration" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "p" {{
            LaunchOrFocusPlugin "plugin-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
        bind "l" {{
            LaunchOrFocusPlugin "yinz:layout-manager" {{
                floating true
                move_to_focused_tab true
            }};
            SwitchToMode "Normal"
        }}
    }}
    tmux {{
        bind "[" {{ SwitchToMode "Scroll"; }}
        bind "{primary_modifier} b" {{ Write 2; SwitchToMode "Normal"; }}
        bind "\"" {{ NewPane "Down"; SwitchToMode "Normal"; }}
        bind "%" {{ NewPane "Right"; SwitchToMode "Normal"; }}
        bind "z" {{ ToggleFocusFullscreen; SwitchToMode "Normal"; }}
        bind "c" {{ NewTab; SwitchToMode "Normal"; }}
        bind "," {{ SwitchToMode "RenameTab"; }}
        bind "p" {{ GoToPreviousTab; SwitchToMode "Normal"; }}
        bind "n" {{ GoToNextTab; SwitchToMode "Normal"; }}
        bind "Left" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "Right" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "Down" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "Up" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "h" {{ MoveFocus "Left"; SwitchToMode "Normal"; }}
        bind "l" {{ MoveFocus "Right"; SwitchToMode "Normal"; }}
        bind "j" {{ MoveFocus "Down"; SwitchToMode "Normal"; }}
        bind "k" {{ MoveFocus "Up"; SwitchToMode "Normal"; }}
        bind "o" {{ FocusNextPane; }}
        bind "d" {{ Detach; }}
        bind "Space" {{ NextSwapLayout; }}
        bind "x" {{ CloseFocus; SwitchToMode "Normal"; }}
    }}
    shared_except "locked" "select" {{
        bind "{primary_modifier} g" {{ SwitchToMode "Locked"; }}
        bind "{primary_modifier} q" {{ Quit; }}
        bind "{secondary_modifier} f" {{ ToggleFloatingPanes; }}
        bind "{secondary_modifier} n" {{ NewPane; }}
        bind "{secondary_modifier} i" {{ MoveTab "Left"; }}
        bind "{secondary_modifier} o" {{ MoveTab "Right"; }}
        bind "{secondary_modifier} h" "{secondary_modifier} Left" {{ MoveFocusOrTab "Left"; }}
        bind "{secondary_modifier} l" "{secondary_modifier} Right" {{ MoveFocusOrTab "Right"; }}
        bind "{secondary_modifier} j" "{secondary_modifier} Down" {{ MoveFocus "Down"; }}
        bind "{secondary_modifier} k" "{secondary_modifier} Up" {{ MoveFocus "Up"; }}
        bind "{secondary_modifier} =" "{secondary_modifier} +" {{ Resize "Increase"; }}
        bind "{secondary_modifier} -" {{ Resize "Decrease"; }}
        bind "{secondary_modifier} [" {{ PreviousSwapLayout; }}
        bind "{secondary_modifier} ]" {{ NextSwapLayout; }}
        bind "{secondary_modifier} p" {{ TogglePaneInGroup; }}
        bind "{secondary_modifier} Shift p" {{ ToggleGroupMarking; }}
    }}
    shared_except "normal" "locked" "notifications" {{
        bind "Enter" "Esc" {{ SwitchToMode "Normal"; }}
    }}
    shared_except "pane" "locked" {{
        bind "{primary_modifier} p" {{ SwitchToMode "Pane"; }}
    }}
    shared_except "resize" "locked" {{
        bind "{primary_modifier} r" {{ SwitchToMode "Resize"; }}
    }}
    shared_except "scroll" "locked" {{
        bind "{primary_modifier} s" {{ SwitchToMode "Scroll"; }}
    }}
    shared_except "session" "locked" "tab" {{
        bind "{primary_modifier} o" {{ SwitchToMode "Session"; }}
    }}
    shared_except "tab" "locked" {{
        bind "{primary_modifier} t" {{ SwitchToMode "Tab"; }}
    }}
    shared_except "move" "locked" {{
        bind "{primary_modifier} m" {{ SwitchToMode "Move"; }}
    }}
    shared_except "tmux" "locked" {{
        bind "{primary_modifier} b" {{ SwitchToMode "Tmux"; }}
    }}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_selection_bindings(config: String, exit_mode: &str) {
        assert!(config.contains(r#"bind "v" { SwitchToMode "Select"; }"#));
        assert_eq!(config.matches("MoveTextSelection").count(), 4);
        assert_eq!(config.matches("SwapTextSelectionEnds").count(), 1);
        assert!(config.contains(&format!(
            r#"bind "Enter" {{ Copy; SwitchToMode "{exit_mode}"; }}"#
        )));
        assert!(config.contains(&format!(
            r#"bind "Ctrl c" {{ SwitchToMode "{exit_mode}"; }}"#
        )));
    }

    #[test]
    fn generated_presets_include_selection_bindings() {
        assert_selection_bindings(unlock_first_keybinds("Ctrl".into(), "Alt".into()), "Locked");
        for config in [
            default_keybinds("Ctrl".into(), "Alt".into()),
            default_keybinds("".into(), "Alt".into()),
            default_keybinds("Ctrl".into(), "".into()),
            default_keybinds("".into(), "".into()),
            default_keybinds("Ctrl".into(), "Ctrl".into()),
        ] {
            assert_selection_bindings(config, "Normal");
        }
    }

    #[test]
    fn every_preset_and_the_default_config_bind_the_notification_browser() {
        use yinz_config::config::Config;
        use yinz_types::data::{InputMode, KeyWithModifier};
        use yinz_types::geometry::Direction;
        use yinz_types::input::actions::Action;

        let canonical_text = include_str!("../../../yinz-config/assets/config/default.kdl");
        let canonical = Config::from_kdl(canonical_text, None).unwrap();
        for (text, exit_mode) in [
            (canonical_text.to_owned(), InputMode::Normal),
            (
                unlock_first_keybinds("Ctrl".into(), "Alt".into()),
                InputMode::Locked,
            ),
            (
                default_keybinds("Ctrl".into(), "Alt".into()),
                InputMode::Normal,
            ),
            (default_keybinds("".into(), "Alt".into()), InputMode::Normal),
            (
                default_keybinds("Ctrl".into(), "".into()),
                InputMode::Normal,
            ),
            (default_keybinds("".into(), "".into()), InputMode::Normal),
            (
                default_keybinds("Ctrl".into(), "Ctrl".into()),
                InputMode::Normal,
            ),
        ] {
            let config = Config::from_kdl(&text, Some(canonical.clone())).unwrap();
            let reparsed = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
            for config in [&config, &reparsed] {
                let key = |key: &str| -> KeyWithModifier { key.parse().unwrap() };
                assert_eq!(
                    config.keybinds.0[&InputMode::Session][&key("n")],
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Notifications
                    }]
                );
                let browser = &config.keybinds.0[&InputMode::Notifications];
                for (keys, action) in [
                    (
                        ["j", "Down"],
                        Action::MoveNotificationSelection {
                            direction: Direction::Down,
                        },
                    ),
                    (
                        ["k", "Up"],
                        Action::MoveNotificationSelection {
                            direction: Direction::Up,
                        },
                    ),
                ] {
                    for bound in keys {
                        assert_eq!(browser[&key(bound)], vec![action.clone()], "{bound}");
                    }
                }
                assert_eq!(
                    browser[&key("Space")],
                    vec![Action::ToggleNotificationDetail]
                );
                assert_eq!(browser[&key("Enter")], vec![Action::ActivateNotification]);
                assert_eq!(
                    browser[&key("Esc")],
                    vec![Action::SwitchToMode {
                        input_mode: exit_mode
                    }]
                );
            }
        }
    }

    #[test]
    fn generated_presets_preserve_agent_bindings_after_parsing_and_saving() {
        use {yinz_config::config::Config, yinz_types::input::actions::Action};
        use {yinz_types::data::InputMode, yinz_types::data::KeyWithModifier};

        let canonical = Config::from_kdl(
            include_str!("../../../yinz-config/assets/config/default.kdl"),
            None,
        )
        .unwrap();
        let canonical_agent = &canonical.keybinds.0[&InputMode::Agent];
        for (text, entry, exit_mode) in [
            (
                unlock_first_keybinds("Ctrl".into(), "Alt".into()),
                "e",
                InputMode::Locked,
            ),
            (
                default_keybinds("Ctrl".into(), "Alt".into()),
                "Ctrl e",
                InputMode::Normal,
            ),
            (
                default_keybinds("".into(), "Alt".into()),
                "e",
                InputMode::Normal,
            ),
            (
                default_keybinds("Ctrl".into(), "".into()),
                "Ctrl e",
                InputMode::Normal,
            ),
            (
                default_keybinds("".into(), "".into()),
                "e",
                InputMode::Normal,
            ),
            (
                default_keybinds("Ctrl".into(), "Ctrl".into()),
                "Ctrl e",
                InputMode::Normal,
            ),
            (
                default_keybinds("Alt".into(), "Ctrl".into()),
                "Alt e",
                InputMode::Normal,
            ),
            (
                default_keybinds("Ctrl Alt".into(), "Shift".into()),
                "Ctrl Alt e",
                InputMode::Normal,
            ),
        ] {
            let config = Config::from_kdl(&text, Some(canonical.clone())).unwrap();
            let reparsed = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
            assert_eq!(
                config.keybinds.0[&InputMode::Agent],
                reparsed.keybinds.0[&InputMode::Agent]
            );
            for config in [&config, &reparsed] {
                let key: KeyWithModifier = entry.parse().unwrap();
                let plain_entry = key.has_no_modifiers();
                assert_eq!(config.options.default_mode, Some(exit_mode));
                // A plain entry key must stay out of the mode keys are typed in (the exit
                // mode) and enter Agent mode from the others; a modified one enters from
                // Normal as well.
                let entry_mode = if plain_entry && exit_mode == InputMode::Normal {
                    InputMode::Tab
                } else {
                    InputMode::Normal
                };
                assert_eq!(
                    config.keybinds.0[&entry_mode][&key],
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Agent
                    }]
                );
                let mut typing_modes = vec![InputMode::Locked];
                if plain_entry {
                    typing_modes.push(exit_mode);
                }
                for mode in typing_modes {
                    assert!(
                        config
                            .keybinds
                            .0
                            .get(&mode)
                            .and_then(|bindings| bindings.get(&key))
                            .is_none(),
                        "{entry} bound in {mode:?}"
                    );
                }
                let agent = &config.keybinds.0[&InputMode::Agent];
                assert_eq!(
                    agent[&key],
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Normal
                    }]
                );
                for key in ["Enter", "Esc"] {
                    assert_eq!(
                        agent[&key.parse().unwrap()],
                        vec![Action::SwitchToMode {
                            input_mode: exit_mode
                        }]
                    );
                }
                // `o` asks a running agent to move the panel: a message that names no plugin,
                // so it reaches the plugins already running and launches none.
                assert!(matches!(
                    &agent[&"o".parse().unwrap()][0],
                    Action::KeybindPipe { name: Some(name), plugin: None, launch_new: false, .. }
                        if name == "agent_strip_reposition"
                ));
                for key in ["n", "d", "a", "o"] {
                    let key: KeyWithModifier = key.parse().unwrap();
                    let actions = &agent[&key];
                    assert_eq!(actions.len(), 2);
                    assert_eq!(actions[0], canonical_agent[&key][0]);
                    assert_eq!(
                        actions[1],
                        Action::SwitchToMode {
                            input_mode: exit_mode
                        }
                    );
                }
                if plain_entry {
                    for mode in [
                        InputMode::RenameTab,
                        InputMode::RenamePane,
                        InputMode::EnterSearch,
                    ] {
                        assert!(!config.keybinds.0[&mode].contains_key(&key));
                    }
                    // The shared plain-e entry must not replace Pane mode's embed action
                    // or Scroll mode's scrollback editor.
                    assert_eq!(
                        config.keybinds.0[&InputMode::Pane][&key][0],
                        Action::TogglePaneEmbedOrFloating
                    );
                    assert!(matches!(
                        config.keybinds.0[&InputMode::Scroll][&key][0],
                        Action::EditScrollback { .. }
                    ));
                }
            }
        }
    }
}
