;; Preset KDL preserved from the configuration plugin, including Agent and Notifications modes.
(provide configuration-default-keybinds configuration-unlock-first-keybinds)

(define (group-navigation-keybinds secondary-modifier)
  (if (or (equal? secondary-modifier "")
      (member "Shift" (split-whitespace secondary-modifier)))
    ""
    (string-append "        bind \"" secondary-modifier " Shift h\" { GoToPreviousTabGroup; }
        bind \"" secondary-modifier " Shift l\" { GoToNextTabGroup; }
")))

(define (agent-keybinds entry-key exit-mode plain-entry)
  (define excluded-modes
    (if plain-entry
      (string-append "\"agent\" \"locked\" " (if (equal? exit-mode "Locked") "" "\"normal\" ")
        "\"renametab\" \"renamepane\" \"entersearch\"")
      "\"agent\" \"locked\""))
  (string-append
    "
    shared_except "
    excluded-modes
    " {
        bind \""
    entry-key
    "\" { SwitchToMode \"Agent\"; }
    }
    agent {
        bind \""
    entry-key
    "\" { SwitchToMode \"Normal\"; }
        bind \"Enter\" \"Esc\" { SwitchToMode \""
    exit-mode
    "\"; }
        bind \"n\" {
            LaunchOrFocusPlugin \"new-agent\" {
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            };
            SwitchToMode \""
    exit-mode
    "\"
        }
        bind \"d\" {
            LaunchOrFocusPlugin \"duplicate-agent\" {
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            };
            SwitchToMode \""
    exit-mode
    "\"
        }
        bind \"f\" {
            LaunchOrFocusPlugin \"fork-agent\" {
                floating true
                overlay true
                modal true
                move_to_focused_tab true
            };
            SwitchToMode \""
    exit-mode
    "\"
        }
        bind \"a\" {
            LaunchOrFocusPlugin \"agents\" {
                floating true
                overlay true
                awaits_placement true
                move_to_focused_tab true
            };
            SwitchToMode \""
    exit-mode
    "\"
        }
        bind \"o\" {
            MessagePlugin { name \"agent_strip_reposition\"; };
            SwitchToMode \""
    exit-mode
    "\"
        }
    }
"))

(define (notifications-keybinds exit-mode secondary-modifier)
  (string-append
    (if (equal? secondary-modifier "") ""
      (string-append "
    shared_except \"locked\" \"select\" {
        bind \"" secondary-modifier " a\" { ActivateLastNotification; }
    }
"))
    "
    notifications {
        bind \"Esc\" { SwitchToMode \""
    exit-mode
    "\"; }
        bind \"Down\" \"j\" { MoveNotificationSelection \"Down\"; }
        bind \"Up\" \"k\" { MoveNotificationSelection \"Up\"; }
        bind \"Space\" { ToggleNotificationDetail; }
        bind \"Enter\" { ActivateNotification; }
    }
"))

(define (template-unlock-first-keybinds primary-modifier secondary-modifier)
  (define agent-fragment (agent-keybinds "e" "Locked" #t))
  (define notification-fragment (notifications-keybinds "Locked" secondary-modifier))
  (string-append
    "
default_mode \"locked\"
keybinds clear-defaults=true {
"
    agent-fragment
    "
"
    notification-fragment
    "
    normal {
    }
    locked {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Normal\"; }
    }
    resize {
        bind \"r\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \"p\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"Tab\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Locked\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Locked\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Locked\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Locked\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Locked\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Locked\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Locked\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Locked\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Locked\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Locked\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Locked\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Locked\"; }
        bind \"Ctrl c\" { SwitchToMode \"Locked\"; }
    }
    move {
        bind \"m\" { SwitchToMode \"Normal\"; }
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \"t\" { SwitchToMode \"Normal\"; }
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Locked\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Locked\"; }
"
    (if (equal? secondary-modifier "")
      "        bind \"H\" { GoToPreviousTabGroup; }
        bind \"L\" { GoToNextTabGroup; }
"
      "")
    "        bind \"x\" { CloseTab; SwitchToMode \"Locked\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Locked\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Locked\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Locked\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Locked\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Locked\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Locked\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Locked\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Locked\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Locked\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Locked\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Locked\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Locked\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Locked\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \"s\" { SwitchToMode \"Normal\"; }
        bind \"e\" { EditScrollback; SwitchToMode \"Locked\"; }
        bind \"f\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Locked\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"Alt left\" { MoveFocusOrTab \"left\"; SwitchToMode \"locked\"; }
        bind \"Alt down\" { MoveFocus \"down\"; SwitchToMode \"locked\"; }
        bind \"Alt up\" { MoveFocus \"up\"; SwitchToMode \"locked\"; }
        bind \"Alt right\" { MoveFocusOrTab \"right\"; SwitchToMode \"locked\"; }
        bind \"Alt h\" { MoveFocusOrTab \"left\"; SwitchToMode \"locked\"; }
        bind \"Alt j\" { MoveFocus \"down\"; SwitchToMode \"locked\"; }
        bind \"Alt k\" { MoveFocus \"up\"; SwitchToMode \"locked\"; }
        bind \"Alt l\" { MoveFocusOrTab \"right\"; SwitchToMode \"locked\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Locked\"; }
    }
    search {
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Locked\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" \"Enter\" { SwitchToMode \"Locked\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" \"Enter\" { SwitchToMode \"Locked\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \"o\" { SwitchToMode \"Normal\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Locked\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Locked\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Locked\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Locked\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Locked\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Locked\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Locked\"
        }
    }
    shared_except \"locked\" \"renametab\" \"renamepane\" {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Locked\"; }
        bind \""
    primary-modifier
    " q\" { Quit; }
    }
    shared_except \"renamepane\" \"renametab\" \"entersearch\" \"locked\" \"notifications\" {
        bind \"esc\" { SwitchToMode \"locked\"; }
    }
    shared_among \"normal\" \"locked\" {
        bind \""
    secondary-modifier
    " n\" { NewPane; }
        bind \""
    secondary-modifier
    " f\" { ToggleFloatingPanes; }
        bind \""
    secondary-modifier
    " i\" { MoveTab \"Left\"; }
        bind \""
    secondary-modifier
    " o\" { MoveTab \"Right\"; }
        bind \""
    secondary-modifier
    " h\" \""
    secondary-modifier
    " Left\" { MoveFocusOrTab \"Left\"; }
        bind \""
    secondary-modifier
    " l\" \""
    secondary-modifier
    " Right\" { MoveFocusOrTab \"Right\"; }
"
    (group-navigation-keybinds secondary-modifier)
    "        bind \""
    secondary-modifier
    " j\" \""
    secondary-modifier
    " Down\" { MoveFocus \"Down\"; }
        bind \""
    secondary-modifier
    " k\" \""
    secondary-modifier
    " Up\" { MoveFocus \"Up\"; }
        bind \""
    secondary-modifier
    " =\" \""
    secondary-modifier
    " +\" { Resize \"Increase\"; }
        bind \""
    secondary-modifier
    " -\" { Resize \"Decrease\"; }
        bind \""
    secondary-modifier
    " [\" { PreviousSwapLayout; }
        bind \""
    secondary-modifier
    " ]\" { NextSwapLayout; }
        bind \""
    secondary-modifier
    " p\" { TogglePaneInGroup; }
        bind \""
    secondary-modifier
    " Shift p\" { ToggleGroupMarking; }
    }
    shared_except \"locked\" \"renametab\" \"renamepane\" \"notifications\" {
        bind \"Enter\" { SwitchToMode \"Locked\"; }
    }
    shared_except \"pane\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"p\" { SwitchToMode \"Pane\"; }
    }
    shared_except \"resize\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"r\" { SwitchToMode \"Resize\"; }
    }
    shared_except \"scroll\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"s\" { SwitchToMode \"Scroll\"; }
    }
    shared_except \"session\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"o\" { SwitchToMode \"Session\"; }
    }
    shared_except \"tab\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"t\" { SwitchToMode \"Tab\"; }
    }
    shared_except \"move\" \"locked\" \"renametab\" \"renamepane\" \"entersearch\" {
        bind \"m\" { SwitchToMode \"Move\"; }
    }
}"))

(define (template-default-keybinds primary-modifier secondary-modifier)
  (define agent-fragment (agent-keybinds (string-append primary-modifier " e") "Normal" #f))
  (define notification-fragment (notifications-keybinds "Normal" secondary-modifier))
  (string-append
    "
default_mode \"normal\"
keybinds clear-defaults=true {
"
    agent-fragment
    "
"
    notification-fragment
    "
    normal {}
    locked {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Normal\"; }
    }
    resize {
        bind \""
    primary-modifier
    " n\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"p\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Normal\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Normal\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Normal\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Normal\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Normal\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Normal\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
    }
    move {
        bind \""
    primary-modifier
    " h\" { SwitchToMode \"Normal\"; }
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Normal\"; }
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Normal\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseTab; SwitchToMode \"Normal\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Normal\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Normal\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Normal\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Normal\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Normal\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Normal\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Normal\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Normal\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Normal\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Normal\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Normal\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Normal\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Normal\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"e\" { EditScrollback; SwitchToMode \"Normal\"; }
        bind \"s\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \""
    secondary-modifier
    " left\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " down\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " up\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " right\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " h\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " j\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " k\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " l\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Normal\"; }
    }
    search {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Normal\"; }
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Normal\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Normal\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
    }
    tmux {
        bind \"[\" { SwitchToMode \"Scroll\"; }
        bind \""
    primary-modifier
    " b\" { Write 2; SwitchToMode \"Normal\"; }
        bind \"\\\"\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"%\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"z\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"c\" { NewTab; SwitchToMode \"Normal\"; }
        bind \",\" { SwitchToMode \"RenameTab\"; }
        bind \"p\" { GoToPreviousTab; SwitchToMode \"Normal\"; }
        bind \"n\" { GoToNextTab; SwitchToMode \"Normal\"; }
        bind \"Left\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"Right\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"Down\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"Up\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"h\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"l\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"j\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"k\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"o\" { FocusNextPane; }
        bind \"d\" { Detach; }
        bind \"Space\" { NextSwapLayout; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
    }
    shared_except \"locked\" \"select\" {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Locked\"; }
        bind \""
    primary-modifier
    " q\" { Quit; }
        bind \""
    secondary-modifier
    " f\" { ToggleFloatingPanes; }
        bind \""
    secondary-modifier
    " n\" { NewPane; }
        bind \""
    secondary-modifier
    " i\" { MoveTab \"Left\"; }
        bind \""
    secondary-modifier
    " o\" { MoveTab \"Right\"; }
        bind \""
    secondary-modifier
    " h\" \""
    secondary-modifier
    " Left\" { MoveFocusOrTab \"Left\"; }
        bind \""
    secondary-modifier
    " l\" \""
    secondary-modifier
    " Right\" { MoveFocusOrTab \"Right\"; }
"
    (group-navigation-keybinds secondary-modifier)
    "        bind \""
    secondary-modifier
    " j\" \""
    secondary-modifier
    " Down\" { MoveFocus \"Down\"; }
        bind \""
    secondary-modifier
    " k\" \""
    secondary-modifier
    " Up\" { MoveFocus \"Up\"; }
        bind \""
    secondary-modifier
    " =\" \""
    secondary-modifier
    " +\" { Resize \"Increase\"; }
        bind \""
    secondary-modifier
    " -\" { Resize \"Decrease\"; }
        bind \""
    secondary-modifier
    " [\" { PreviousSwapLayout; }
        bind \""
    secondary-modifier
    " ]\" { NextSwapLayout; }
        bind \""
    secondary-modifier
    " p\" { TogglePaneInGroup; }
        bind \""
    secondary-modifier
    " Shift p\" { ToggleGroupMarking; }
    }
    shared_except \"normal\" \"locked\" \"notifications\" {
        bind \"Enter\" \"Esc\" { SwitchToMode \"Normal\"; }
    }
    shared_except \"pane\" \"locked\" {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Pane\"; }
    }
    shared_except \"resize\" \"locked\" {
        bind \""
    primary-modifier
    " n\" { SwitchToMode \"Resize\"; }
    }
    shared_except \"scroll\" \"locked\" {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
    }
    shared_except \"session\" \"locked\" {
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Session\"; }
    }
    shared_except \"tab\" \"locked\" {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Tab\"; }
    }
    shared_except \"move\" \"locked\" {
        bind \""
    primary-modifier
    " h\" { SwitchToMode \"Move\"; }
    }
    shared_except \"tmux\" \"locked\" {
        bind \""
    primary-modifier
    " b\" { SwitchToMode \"Tmux\"; }
    }
}
"))

(define (template-default-keybinds-no-primary-modifier secondary-modifier)
  (define agent-fragment (agent-keybinds "e" "Normal" #t))
  (define notification-fragment (notifications-keybinds "Normal" secondary-modifier))
  (string-append
    "
default_mode \"normal\"
keybinds clear-defaults=true {
"
    agent-fragment
    "
"
    notification-fragment
    "
    normal {}
    locked {}
    resize {
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"p\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Normal\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Normal\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Normal\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Normal\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Normal\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Normal\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
    }
    move {
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Normal\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseTab; SwitchToMode \"Normal\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Normal\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Normal\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Normal\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Normal\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Normal\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Normal\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Normal\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Normal\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Normal\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Normal\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Normal\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Normal\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Normal\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \"e\" { EditScrollback; SwitchToMode \"Normal\"; }
        bind \"s\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \""
    secondary-modifier
    " left\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " down\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " up\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " right\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " h\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " j\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " k\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " l\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Normal\"; }
    }
    search {
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Normal\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Normal\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
    }
    tmux {
        bind \"[\" { SwitchToMode \"Scroll\"; }
        bind \"\\\"\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"%\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"z\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"c\" { NewTab; SwitchToMode \"Normal\"; }
        bind \",\" { SwitchToMode \"RenameTab\"; }
        bind \"p\" { GoToPreviousTab; SwitchToMode \"Normal\"; }
        bind \"n\" { GoToNextTab; SwitchToMode \"Normal\"; }
        bind \"Left\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"Right\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"Down\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"Up\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"h\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"l\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"j\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"k\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"o\" { FocusNextPane; }
        bind \"d\" { Detach; }
        bind \"Space\" { NextSwapLayout; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
    }
    shared_except \"locked\" \"select\" {
        bind \""
    secondary-modifier
    " n\" { NewPane; }
        bind \""
    secondary-modifier
    " f\" { ToggleFloatingPanes; }
        bind \""
    secondary-modifier
    " i\" { MoveTab \"Left\"; }
        bind \""
    secondary-modifier
    " o\" { MoveTab \"Right\"; }
        bind \""
    secondary-modifier
    " h\" \""
    secondary-modifier
    " Left\" { MoveFocusOrTab \"Left\"; }
        bind \""
    secondary-modifier
    " l\" \""
    secondary-modifier
    " Right\" { MoveFocusOrTab \"Right\"; }
"
    (group-navigation-keybinds secondary-modifier)
    "        bind \""
    secondary-modifier
    " j\" \""
    secondary-modifier
    " Down\" { MoveFocus \"Down\"; }
        bind \""
    secondary-modifier
    " k\" \""
    secondary-modifier
    " Up\" { MoveFocus \"Up\"; }
        bind \""
    secondary-modifier
    " =\" \""
    secondary-modifier
    " +\" { Resize \"Increase\"; }
        bind \""
    secondary-modifier
    " -\" { Resize \"Decrease\"; }
        bind \""
    secondary-modifier
    " [\" { PreviousSwapLayout; }
        bind \""
    secondary-modifier
    " ]\" { NextSwapLayout; }
        bind \""
    secondary-modifier
    " p\" { TogglePaneInGroup; }
        bind \""
    secondary-modifier
    " Shift p\" { ToggleGroupMarking; }
    }
    shared_except \"normal\" \"locked\" \"notifications\" {
        bind \"Enter\" \"Esc\" { SwitchToMode \"Normal\"; }
    }
}
"))

(define (template-default-keybinds-no-secondary-modifier primary-modifier)
  (define agent-fragment (agent-keybinds (string-append primary-modifier " e") "Normal" #f))
  (define notification-fragment (notifications-keybinds "Normal" ""))
  (string-append
    "
default_mode \"normal\"
keybinds clear-defaults=true {
"
    agent-fragment
    "
"
    notification-fragment
    "
    normal {}
    locked {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Normal\"; }
    }
    resize {
        bind \""
    primary-modifier
    " n\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"p\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Normal\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Normal\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Normal\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Normal\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Normal\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Normal\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
    }
    move {
        bind \""
    primary-modifier
    " h\" { SwitchToMode \"Normal\"; }
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Normal\"; }
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Normal\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Normal\"; }
        bind \"H\" { GoToPreviousTabGroup; }
        bind \"L\" { GoToNextTabGroup; }
        bind \"x\" { CloseTab; SwitchToMode \"Normal\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Normal\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Normal\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Normal\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Normal\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Normal\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Normal\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Normal\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Normal\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Normal\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Normal\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Normal\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Normal\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Normal\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"e\" { EditScrollback; SwitchToMode \"Normal\"; }
        bind \"s\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Normal\"; }
    }
    search {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Normal\"; }
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Normal\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Normal\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
    }
    tmux {
        bind \"[\" { SwitchToMode \"Scroll\"; }
        bind \""
    primary-modifier
    " b\" { Write 2; SwitchToMode \"Normal\"; }
        bind \"\\\"\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"%\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"z\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"c\" { NewTab; SwitchToMode \"Normal\"; }
        bind \",\" { SwitchToMode \"RenameTab\"; }
        bind \"p\" { GoToPreviousTab; SwitchToMode \"Normal\"; }
        bind \"n\" { GoToNextTab; SwitchToMode \"Normal\"; }
        bind \"Left\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"Right\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"Down\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"Up\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"h\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"l\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"j\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"k\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"o\" { FocusNextPane; }
        bind \"d\" { Detach; }
        bind \"Space\" { NextSwapLayout; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
    }
    shared_except \"locked\" {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Locked\"; }
        bind \""
    primary-modifier
    " q\" { Quit; }
    }
    shared_except \"normal\" \"locked\" \"notifications\" {
        bind \"Enter\" \"Esc\" { SwitchToMode \"Normal\"; }
    }
    shared_except \"pane\" \"locked\" {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Pane\"; }
    }
    shared_except \"resize\" \"locked\" {
        bind \""
    primary-modifier
    " n\" { SwitchToMode \"Resize\"; }
    }
    shared_except \"scroll\" \"locked\" {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
    }
    shared_except \"session\" \"locked\" {
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Session\"; }
    }
    shared_except \"tab\" \"locked\" {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Tab\"; }
    }
    shared_except \"move\" \"locked\" {
        bind \""
    primary-modifier
    " h\" { SwitchToMode \"Move\"; }
    }
    shared_except \"tmux\" \"locked\" {
        bind \""
    primary-modifier
    " b\" { SwitchToMode \"Tmux\"; }
    }
}
"))

(define (template-non-colliding-default-keybinds primary-modifier secondary-modifier)
  (define agent-fragment (agent-keybinds (string-append primary-modifier " e") "Normal" #f))
  (define notification-fragment (notifications-keybinds "Normal" secondary-modifier))
  (string-append
    "
default_mode \"normal\"
keybinds clear-defaults=true {
"
    agent-fragment
    "
"
    notification-fragment
    "
    normal {}
    locked {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Normal\"; }
    }
    resize {
        bind \""
    primary-modifier
    " r\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Normal\"; }
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"p\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Normal\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Normal\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Normal\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Normal\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Normal\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Normal\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
    }
    move {
        bind \""
    primary-modifier
    " m\" { SwitchToMode \"Normal\"; }
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Normal\"; }
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Normal\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseTab; SwitchToMode \"Normal\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Normal\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Normal\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Normal\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Normal\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Normal\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Normal\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Normal\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Normal\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Normal\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Normal\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Normal\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Normal\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Normal\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"e\" { EditScrollback; SwitchToMode \"Normal\"; }
        bind \"s\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \""
    secondary-modifier
    " left\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " down\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " up\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " right\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " h\" { MoveFocusOrTab \"left\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " j\" { MoveFocus \"down\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " k\" { MoveFocus \"up\"; SwitchToMode \"normal\"; }
        bind \""
    secondary-modifier
    " l\" { MoveFocusOrTab \"right\"; SwitchToMode \"normal\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Normal\"; }
    }
    search {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Normal\"; }
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Normal\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Normal\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
    }
    tmux {
        bind \"[\" { SwitchToMode \"Scroll\"; }
        bind \""
    primary-modifier
    " b\" { Write 2; SwitchToMode \"Normal\"; }
        bind \"\\\"\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"%\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"z\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"c\" { NewTab; SwitchToMode \"Normal\"; }
        bind \",\" { SwitchToMode \"RenameTab\"; }
        bind \"p\" { GoToPreviousTab; SwitchToMode \"Normal\"; }
        bind \"n\" { GoToNextTab; SwitchToMode \"Normal\"; }
        bind \"Left\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"Right\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"Down\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"Up\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"h\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"l\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"j\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"k\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"o\" { FocusNextPane; }
        bind \"d\" { Detach; }
        bind \"Space\" { NextSwapLayout; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
    }
    shared_except \"locked\" \"select\" {
        bind \""
    primary-modifier
    " g\" { SwitchToMode \"Locked\"; }
        bind \""
    primary-modifier
    " q\" { Quit; }
        bind \""
    secondary-modifier
    " f\" { ToggleFloatingPanes; }
        bind \""
    secondary-modifier
    " n\" { NewPane; }
        bind \""
    secondary-modifier
    " i\" { MoveTab \"Left\"; }
        bind \""
    secondary-modifier
    " o\" { MoveTab \"Right\"; }
        bind \""
    secondary-modifier
    " h\" \""
    secondary-modifier
    " Left\" { MoveFocusOrTab \"Left\"; }
        bind \""
    secondary-modifier
    " l\" \""
    secondary-modifier
    " Right\" { MoveFocusOrTab \"Right\"; }
"
    (group-navigation-keybinds secondary-modifier)
    "        bind \""
    secondary-modifier
    " j\" \""
    secondary-modifier
    " Down\" { MoveFocus \"Down\"; }
        bind \""
    secondary-modifier
    " k\" \""
    secondary-modifier
    " Up\" { MoveFocus \"Up\"; }
        bind \""
    secondary-modifier
    " =\" \""
    secondary-modifier
    " +\" { Resize \"Increase\"; }
        bind \""
    secondary-modifier
    " -\" { Resize \"Decrease\"; }
        bind \""
    secondary-modifier
    " [\" { PreviousSwapLayout; }
        bind \""
    secondary-modifier
    " ]\" { NextSwapLayout; }
        bind \""
    secondary-modifier
    " p\" { TogglePaneInGroup; }
        bind \""
    secondary-modifier
    " Shift p\" { ToggleGroupMarking; }
    }
    shared_except \"normal\" \"locked\" \"notifications\" {
        bind \"Enter\" \"Esc\" { SwitchToMode \"Normal\"; }
    }
    shared_except \"pane\" \"locked\" {
        bind \""
    primary-modifier
    " p\" { SwitchToMode \"Pane\"; }
    }
    shared_except \"resize\" \"locked\" {
        bind \""
    primary-modifier
    " r\" { SwitchToMode \"Resize\"; }
    }
    shared_except \"scroll\" \"locked\" {
        bind \""
    primary-modifier
    " s\" { SwitchToMode \"Scroll\"; }
    }
    shared_except \"session\" \"locked\" \"tab\" {
        bind \""
    primary-modifier
    " o\" { SwitchToMode \"Session\"; }
    }
    shared_except \"tab\" \"locked\" {
        bind \""
    primary-modifier
    " t\" { SwitchToMode \"Tab\"; }
    }
    shared_except \"move\" \"locked\" {
        bind \""
    primary-modifier
    " m\" { SwitchToMode \"Move\"; }
    }
    shared_except \"tmux\" \"locked\" {
        bind \""
    primary-modifier
    " b\" { SwitchToMode \"Tmux\"; }
    }
}
"))

(define (template-default-keybinds-no-modifiers)
  (string-append "
default_mode \"normal\"
keybinds clear-defaults=true {
" (agent-keybinds "e" "Normal" #t) (notifications-keybinds "Normal" "") "    normal {}
    locked {}
    resize {
        bind \"h\" \"Left\" { Resize \"Increase Left\"; }
        bind \"j\" \"Down\" { Resize \"Increase Down\"; }
        bind \"k\" \"Up\" { Resize \"Increase Up\"; }
        bind \"l\" \"Right\" { Resize \"Increase Right\"; }
        bind \"H\" { Resize \"Decrease Left\"; }
        bind \"J\" { Resize \"Decrease Down\"; }
        bind \"K\" { Resize \"Decrease Up\"; }
        bind \"L\" { Resize \"Decrease Right\"; }
        bind \"=\" \"+\" { Resize \"Increase\"; }
        bind \"-\" { Resize \"Decrease\"; }
    }
    pane {
        bind \"h\" \"Left\" { MoveFocus \"Left\"; }
        bind \"l\" \"Right\" { MoveFocus \"Right\"; }
        bind \"j\" \"Down\" { MoveFocus \"Down\"; }
        bind \"k\" \"Up\" { MoveFocus \"Up\"; }
        bind \"p\" { SwitchFocus; }
        bind \";\" { FocusLastPane; }
        bind \"n\" { NewPane; SwitchToMode \"Normal\"; }
        bind \"d\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"r\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"s\" { NewPane \"stacked\"; SwitchToMode \"Normal\"; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"Shift f\" { ToggleFocusNoUiFullscreen; SwitchToMode \"Normal\"; }
        bind \"z\" { TogglePaneFrames; SwitchToMode \"Normal\"; }
        bind \"w\" { ToggleFloatingPanes; SwitchToMode \"Normal\"; }
        bind \"e\" { TogglePaneEmbedOrFloating; SwitchToMode \"Normal\"; }
        bind \"c\" { SwitchToMode \"RenamePane\"; PaneNameInput 0;}
        bind \"i\" { TogglePanePinned; SwitchToMode \"Normal\"; }
        bind \"v\" { SwitchToMode \"Select\"; }
    }
    select {
        bind \"h\" \"Left\" { MoveTextSelection \"Left\"; }
        bind \"j\" \"Down\" { MoveTextSelection \"Down\"; }
        bind \"k\" \"Up\" { MoveTextSelection \"Up\"; }
        bind \"l\" \"Right\" { MoveTextSelection \"Right\"; }
        bind \"o\" { SwapTextSelectionEnds; }
        bind \"Enter\" { Copy; SwitchToMode \"Normal\"; }
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
    }
    move {
        bind \"n\" \"Tab\" { MovePane; }
        bind \"p\" { MovePaneBackwards; }
        bind \"h\" \"Left\" { MovePane \"Left\"; }
        bind \"j\" \"Down\" { MovePane \"Down\"; }
        bind \"k\" \"Up\" { MovePane \"Up\"; }
        bind \"l\" \"Right\" { MovePane \"Right\"; }
    }
    tab {
        bind \"r\" { SwitchToMode \"RenameTab\"; TabNameInput 0; }
        bind \"h\" \"Left\" \"Up\" \"k\" { GoToPreviousTab; }
        bind \"l\" \"Right\" \"Down\" \"j\" { GoToNextTab; }
        bind \"n\" { NewTab; SwitchToMode \"Normal\"; }
        bind \"g\" { NewTabGroup; SwitchToMode \"Normal\"; }
        bind \"H\" { GoToPreviousTabGroup; }
        bind \"L\" { GoToNextTabGroup; }
        bind \"x\" { CloseTab; SwitchToMode \"Normal\"; }
        bind \"s\" { ToggleActiveSyncTab; SwitchToMode \"Normal\"; }
        bind \"b\" { BreakPane; SwitchToMode \"Normal\"; }
        bind \"]\" { BreakPaneRight; SwitchToMode \"Normal\"; }
        bind \"[\" { BreakPaneLeft; SwitchToMode \"Normal\"; }
        bind \"{\" { MovePaneToPreviousTabGroup; SwitchToMode \"Normal\"; }
        bind \"}\" { MovePaneToNextTabGroup; SwitchToMode \"Normal\"; }
        bind \"1\" { GoToTabInGroup 1; SwitchToMode \"Normal\"; }
        bind \"2\" { GoToTabInGroup 2; SwitchToMode \"Normal\"; }
        bind \"3\" { GoToTabInGroup 3; SwitchToMode \"Normal\"; }
        bind \"4\" { GoToTabInGroup 4; SwitchToMode \"Normal\"; }
        bind \"5\" { GoToTabInGroup 5; SwitchToMode \"Normal\"; }
        bind \"6\" { GoToTabInGroup 6; SwitchToMode \"Normal\"; }
        bind \"7\" { GoToTabInGroup 7; SwitchToMode \"Normal\"; }
        bind \"8\" { GoToTabInGroup 8; SwitchToMode \"Normal\"; }
        bind \"9\" { GoToTabInGroup 9; SwitchToMode \"Normal\"; }
        bind \"Tab\" { ToggleTab; }
    }
    scroll {
        bind \"e\" { EditScrollback; SwitchToMode \"Normal\"; }
        bind \"s\" { SwitchToMode \"EnterSearch\"; SearchInput 0; }
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
        bind \"c\" { CopyLastCommandOutput; SwitchToMode \"Normal\"; }
    }
    search {
        bind \"Ctrl c\" { ScrollToBottom; SwitchToMode \"Normal\"; }
        bind \"j\" \"Down\" { ScrollDown; }
        bind \"k\" \"Up\" { ScrollUp; }
        bind \"Ctrl f\" \"PageDown\" \"Right\" \"l\" { PageScrollDown; }
        bind \"Ctrl b\" \"PageUp\" \"Left\" \"h\" { PageScrollUp; }
        bind \"d\" { HalfPageScrollDown; }
        bind \"u\" { HalfPageScrollUp; }
        bind \"n\" { Search \"down\"; }
        bind \"p\" { Search \"up\"; }
        bind \"c\" { SearchToggleOption \"CaseSensitivity\"; }
        bind \"w\" { SearchToggleOption \"Wrap\"; }
        bind \"o\" { SearchToggleOption \"WholeWord\"; }
        bind \"[\" { ScrollToPreviousPrompt; }
        bind \"]\" { ScrollToNextPrompt; }
        bind \"m\" { SelectCommandAtScrollPosition; }
    }
    entersearch {
        bind \"Ctrl c\" \"Esc\" { SwitchToMode \"Scroll\"; }
        bind \"Enter\" { SwitchToMode \"Search\"; }
    }
    renametab {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenameTab; SwitchToMode \"Tab\"; }
    }
    renamepane {
        bind \"Ctrl c\" { SwitchToMode \"Normal\"; }
        bind \"Esc\" { UndoRenamePane; SwitchToMode \"Pane\"; }
    }
    session {
        bind \"n\" { SwitchToMode \"Notifications\"; }
        bind \"d\" { Detach; }
        bind \"]\" { FocusHostSession; SwitchToMode \"Normal\"; }
        bind \"[\" { FocusGuestSession; SwitchToMode \"Normal\"; }
        bind \"f\" { ToggleHostFullscreen; SwitchToMode \"Normal\"; }
        bind \"w\" {
            LaunchOrFocusPlugin \"session-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"c\" {
            LaunchOrFocusPlugin \"configuration\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"p\" {
            LaunchOrFocusPlugin \"plugin-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
        bind \"l\" {
            LaunchOrFocusPlugin \"yinz:layout-manager\" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode \"Normal\"
        }
    }
    tmux {
        bind \"[\" { SwitchToMode \"Scroll\"; }
        bind \"\\\"\" { NewPane \"Down\"; SwitchToMode \"Normal\"; }
        bind \"%\" { NewPane \"Right\"; SwitchToMode \"Normal\"; }
        bind \"z\" { ToggleFocusFullscreen; SwitchToMode \"Normal\"; }
        bind \"c\" { NewTab; SwitchToMode \"Normal\"; }
        bind \",\" { SwitchToMode \"RenameTab\"; }
        bind \"p\" { GoToPreviousTab; SwitchToMode \"Normal\"; }
        bind \"n\" { GoToNextTab; SwitchToMode \"Normal\"; }
        bind \"Left\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"Right\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"Down\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"Up\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"h\" { MoveFocus \"Left\"; SwitchToMode \"Normal\"; }
        bind \"l\" { MoveFocus \"Right\"; SwitchToMode \"Normal\"; }
        bind \"j\" { MoveFocus \"Down\"; SwitchToMode \"Normal\"; }
        bind \"k\" { MoveFocus \"Up\"; SwitchToMode \"Normal\"; }
        bind \"o\" { FocusNextPane; }
        bind \"d\" { Detach; }
        bind \"Space\" { NextSwapLayout; }
        bind \"x\" { CloseFocus; SwitchToMode \"Normal\"; }
    }
    shared_except \"normal\" \"locked\" \"notifications\" {
        bind \"Enter\" \"Esc\" { SwitchToMode \"Normal\"; }
    }
}
"))

(define (configuration-unlock-first-keybinds primary secondary)
  (template-unlock-first-keybinds primary secondary))
(define (configuration-default-keybinds primary secondary)
  (cond ((and (equal? primary "") (equal? secondary "")) (template-default-keybinds-no-modifiers))
    ((equal? primary secondary) (template-non-colliding-default-keybinds primary secondary))
    ((equal? primary "") (template-default-keybinds-no-primary-modifier secondary))
    ((equal? secondary "") (template-default-keybinds-no-secondary-modifier primary))
    (else (template-default-keybinds primary secondary))))
