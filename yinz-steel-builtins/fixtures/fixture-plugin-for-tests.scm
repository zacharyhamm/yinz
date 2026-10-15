;; Test-only fixture. This directory is deliberately outside the embedded builtin tree.
;; Commands pass through host permissions, including the explicit WriteFiles request.
(define received-events '())
(define received-payload #f)
(define configuration (hash))
(define message-to-plugin-payload #f)
(define explicit-text #f)
(define worker-count 0)
(define (get object key fallback)
  (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))
(define (text value) (if (string? value) value (yinz-json-string value)))
;; Ordinary denied fixture actions remain observable as a live plugin, like the old shim.
(define (send command)
  (with-handler (lambda (failure) json-null) (yinz-command command)))
(define commands
  (hash
    "d" "GoToNextTab"
    "e" "GoToPreviousTab"
    "h" "FocusNextPane"
    "i" "FocusPreviousPane"
    "l" "Detach"
    "m" "EditScrollback"
    "p" "ToggleTab"
    "q" "MovePane"
    "s" "ClearScreen"
    "t" "ScrollUp"
    "u" "ScrollDown"
    "v" "ScrollToTop"
    "w" "ScrollToBottom"
    "x" "PageScrollUp"
    "y" "PageScrollDown"
    "z" "ToggleFocusFullscreen"
    "1" "TogglePaneFrames"
    "2" "TogglePaneEmbedOrEject"
    "3" "UndoRenamePane"
    "4" "CloseFocus"
    "5" "ToggleActiveTabSync"
    "6" "CloseFocusedTab"
    "7" "UndoRenameTab"
    "8" "QuitYinz"
    "9" "NewPane"
    "a" (hash "SwitchToMode" "Tab")
    "b" (hash "NewTabsWithLayout" "layout {\n                        tab {\n                            pane\n                            pane\n                        }\n                        tab split_direction=\"vertical\" {\n                            pane\n                            pane\n                        }\n                    }")
    "c" (hash "NewTab" (hash "name" "new_tab_name" "cwd" "/path/to/my/cwd"))
    "f" (hash "Resize" "Increase")
    "g" (hash "ResizeWithDirection" (hash "resize" "Increase" "direction" "Left" "invert_on_boundaries" #f))
    "j" (hash "MoveFocus" "Left")
    "k" (hash "MoveFocusOrTab" "Left")
    "r" (hash "MovePaneWithDirection" "Left")
    "n" (hash "Write" (list 102 111 111))
    "o" (hash "WriteChars" "foo")
    "0" (hash "ToggleFloatingPanes" (hash "tab_id" json-null))
    "Ctrl:a" "PreviousSwapLayout"
    "Ctrl:b" "NextSwapLayout"
    "Ctrl:p" "HideSelf"
    "Ctrl:6" "DisconnectOtherClients"
    "Ctrl:c" (hash "GoToTabName" "my tab name")
    "Ctrl:d" (hash "FocusOrCreateTab" "my tab name")
    "Ctrl:e" (hash "GoToTab" 2)
    "Ctrl:f" (hash "StartOrReloadPlugin" "file:/path/to/my/plugin.scm")
    "Ctrl:o" (hash "SwitchTabTo" 1)
    "Ctrl:q" (hash "ShowSelf" #f)
    "Ctrl:r" (hash "CloseTerminalPane" 1)
    "Ctrl:s" (hash "ClosePluginPane" 1)
    "Ctrl:t" (hash "FocusTerminalPane" (list 1 #f #f))
    "Ctrl:u" (hash "FocusPluginPane" (list 1 #f #f))
    "Ctrl:v" (hash "RenameTerminalPane" (list 1 "new terminal_pane_name"))
    "Ctrl:w" (hash "RenamePluginPane" (list 1 "new plugin_pane_name"))
    "Ctrl:x" (hash "RenameTab" (list 0 "new tab name"))
    "Ctrl:1" (hash "RequestPluginPermissions" (list "ReadApplicationState"))
    "Ctrl:g" (hash "OpenFile" (list (hash "path" "/path/to/my/file.rs" "line_number" json-null "cwd" json-null) (hash)))
    "Ctrl:h" (hash "OpenFileFloating" (list (hash "path" "/path/to/my/file.rs" "line_number" json-null "cwd" json-null) json-null (hash)))
    "Ctrl:i" (hash "OpenFile" (list (hash "path" "/path/to/my/file.rs" "line_number" 42 "cwd" json-null) (hash)))
    "Ctrl:j" (hash "OpenFileFloating" (list (hash "path" "/path/to/my/file.rs" "line_number" 42 "cwd" json-null) json-null (hash)))
    "Ctrl:k" (hash "OpenTerminal" (hash "path" "/path/to/my/file.rs" "line_number" json-null "cwd" json-null))
    "Ctrl:l" (hash "OpenTerminalFloating" (list (hash "path" "/path/to/my/file.rs" "line_number" json-null "cwd" json-null) json-null))
    "Ctrl:m" (hash "OpenCommandPane" (list (hash "path" "/path/to/my/file.rs" "args" (list "arg1" "arg2")) (hash)))
    "Ctrl:n" (hash "OpenCommandPaneFloating" (list (hash "path" "/path/to/my/file.rs" "args" (list "arg1" "arg2")) json-null (hash)))
    "Ctrl:2" (hash "RunCommand" (list (list "ls" "-l") (hash) "." (hash "user_key_1" "user_value_1") json-null json-null))
    "Ctrl:3" (hash "RunCommand" (list (list "ls" "-l") (hash "VAR1" "some_value") "/some/custom/folder" (hash "user_key_2" "user_value_2") json-null json-null))
    "Ctrl:4" (hash "WebRequest" (list "https://example.com/foo?arg1=val1&arg2=val2" "Post" (hash "header1" "value1" "header2" "value2") (list 1 2 3) (hash "user_key_1" "user_value1" "user_key_2" "user_value2") json-null json-null))
    "Ctrl:5" (hash "SwitchSession" (hash "name" "my_new_session" "tab_position" json-null "pane_id" json-null "layout" json-null "cwd" json-null))
    "Ctrl:7" (hash "SwitchSession" (hash "name" "my_other_new_session" "tab_position" json-null "pane_id" json-null "layout" (hash "BuiltIn" "compact") "cwd" json-null))
    "Ctrl:9" (hash "SwitchSession" (hash "name" "my_other_new_session_with_cwd" "tab_position" json-null "pane_id" json-null "layout" (hash "BuiltIn" "compact") "cwd" "/foo/bar"))
    "Ctrl:8" (hash "FsWrite" (hash "path" "/host/hi-from-plugin.txt" "contents" "Hi there!" "append" #f "create_dirs" #f))
    "Ctrl:0" (hash "Reconfigure" (list "\n                        keybinds {\n                            locked {\n                                bind \"a\" { NewTab; }\n                            }\n                        }\n                    " #t))
    "Alt:a" (hash "HidePaneWithId" (hash "Terminal" 1))
    "Alt:b" (hash "ShowPaneWithId" (list (hash "Terminal" 1) #t #t))
    "Alt:c" (hash "OpenCommandPaneBackground" (list (hash "path" "/path/to/my/file.rs" "args" (list "arg1" "arg2")) (hash)))
    "Alt:d" (hash "RerunCommandPane" 1)
    "Alt:e" (hash "ResizePaneIdWithDirection" (list (hash "resize" "Increase" "direction" "Left" "invert_on_boundaries" #f) (hash "Terminal" 2)))
    "Alt:f" (hash "EditScrollbackForPaneWithId" (hash "Terminal" 2))
    "Alt:g" (hash "WriteToPaneId" (list (list 102 111 111) (hash "Terminal" 2)))
    "Alt:h" (hash "WriteCharsToPaneId" (list "foo\n" (hash "Terminal" 2)))
    "Alt:i" (hash "MovePaneWithPaneId" (hash "Terminal" 2))
    "Alt:j" (hash "MovePaneWithPaneIdInDirection" (list (hash "Terminal" 2) "Left"))
    "Alt:t" (hash "CloseTabWithIndex" 2)
    "Alt:u" (hash "BreakPanesToNewTab" (list (list (hash "Terminal" 1) (hash "Plugin" 2)) "new_tab_name" #t))
    "Alt:v" (hash "BreakPanesToTabWithIndex" (list (list (hash "Terminal" 1) (hash "Plugin" 2)) 2 #t))
    "Alt:w" (hash "ReloadPlugin" 0)
    "Alt:x" (hash "LoadNewPlugin" (hash "url" "yinz:OWN_URL" "config" (hash) "load_in_background" #t "skip_plugin_cache" #t))
    "Alt:k" (hash "ClearScreenForPaneId" (hash "Terminal" 2))
    "Alt:l" (hash "ScrollUpInPaneId" (hash "Terminal" 2))
    "Alt:m" (hash "ScrollDownInPaneId" (hash "Terminal" 2))
    "Alt:n" (hash "ScrollToTopInPaneId" (hash "Terminal" 2))
    "Alt:o" (hash "ScrollToBottomInPaneId" (hash "Terminal" 2))
    "Alt:p" (hash "PageScrollUpInPaneId" (hash "Terminal" 2))
    "Alt:q" (hash "PageScrollDownInPaneId" (hash "Terminal" 2))
    "Alt:r" (hash "TogglePaneIdFullscreen" (hash "Terminal" 2))
    "Alt:s" (hash "TogglePaneEmbedOrEjectForPaneId" (hash "Terminal" 2))
    "Alt:y" (hash "RebindKeys" (hash "keys_to_unbind" (list (list "Locked" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Normal" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Pane" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Tab" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Resize" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Move" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Search" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl"))) (list "Session" (hash "bare_key" (hash "Char" "g") "key_modifiers" (list "Ctrl")))) "keys_to_rebind" (list (list "Locked" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Normal")))) (list "Normal" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Pane" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Tab" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Resize" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Move" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Search" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked")))) (list "Session" (hash "bare_key" (hash "Char" "a") "key_modifiers" (list "Ctrl")) (list (hash "SwitchToMode" (hash "input_mode" "Locked"))))) "write_config_to_disk" #t))
    "Super:a" (hash "ShowCursor" (list 5 10))
    "Super:b" (hash "ShowCursor" json-null)
    "Super:c" (hash "CopyToClipboard" "test clipboard text")
    "Super:d" (hash "RunAction" (list (hash "MoveFocus" (hash "direction" "Left")) (hash "test_key" "test_value")))
    "Super:e" (hash "SendSigintToPaneId" (hash "Terminal" 1))
    "Super:f" (hash "SendSigkillToPaneId" (hash "Terminal" 1))
    "Super:g" (hash "SetPaneRegexHighlights" (list (hash "Terminal" 1) (list (hash "pattern" "test_pattern" "style" "Emphasis0" "layer" "Hint" "context" (hash) "on_hover" #f "bold" #f "italic" #t "underline" #t "tooltip_text" json-null))))
    "Super:h" (hash "ClearPaneHighlights" (hash "Terminal" 1))
    "Super:i" (hash "ToggleFloatingPanes" (hash "tab_id" 2))
    "Alt+Ctrl:l" "FocusLastPane"
    "Alt+Ctrl:m" (hash "BreakPanesToTabWithId" (list (list (hash "Terminal" 1) (hash "Plugin" 2)) 2 #t))
))

(yinz-register-workers
  (hash "test" (lambda (name payload)
    (when (equal? name "ping")
      (set! worker-count (+ worker-count 1))
      (yinz-post-plugin "pong"
        (string-append payload ", received " (number->string worker-count) " messages"))))))

(define (load config)
  (set! configuration config)
  (send (hash "RequestPluginPermissions"
    '("ChangeApplicationState" "ReadApplicationState" "OpenFiles" "RunCommands"
      "OpenTerminalsOrPlugins" "WriteToStdin" "WebAccess" "ReadCliPipes"
      "MessageAndLaunchOtherPlugins" "Reconfigure" "WriteToClipboard" "RunActionsAsUser"
      "ReadSessionEnvironmentVariables" "ReadPaneContents" "PluginData" "WriteFiles")))
  (send (hash "Subscribe"
    '("InputReceived" "Key" "SystemClipboardFailure" "CustomMessage"
      "FileSystemCreate" "FileSystemUpdate" "FileSystemDelete" "BeforeClose"
      "PluginConfigurationChanged" "HighlightClicked" "QueueJobStarted" "QueueJobFinished")))
  (for-each
    (lambda (entry)
      (when (equal? (get config (car entry) #f) "true")
        (send (hash "Subscribe" (cdr entry)))))
    (list (cons "subscribe_agent_event" '("AgentEvent"))
          (cons "subscribe_initial_keybinds" '("InitialKeybinds" "ModeUpdate"))
          (cons "subscribe_mode_update" '("ModeUpdate"))
          (cons "subscribe_host_terminal_theme" '("HostTerminalThemeChanged"))))
  (send "WatchFilesystem"))

(define (result-text failure-prefix thunk)
  (set! explicit-text
    (with-handler
      (lambda (failure) (string-append failure-prefix (error-object-message failure)))
      (thunk))))
(define (layout-result label command success)
  (result-text (string-append label " error: ")
    (lambda () (yinz-command command) success)))
(define (save name kdl overwrite label)
  (layout-result label
    (hash "SaveLayout" (hash "layout_name" name "layout_kdl" kdl "overwrite" overwrite))
    (string-append label " success")))

(define (special-key key)
  (cond
    [(equal? key "Ctrl:z") (send (hash "GoToTabName" (yinz-json-string configuration)))]
    [(equal? key "Alt:z") (send (hash "Subscribe" '("ListClients"))) (send "ListClients")]
    [(equal? key "Ctrl+Shift:a")
     (define name (send "GenerateRandomName"))
     (set! explicit-text (if (and (string? name) (not (equal? name "")))
                            "Generated name" "Error: got empty name"))]
    [(equal? key "Ctrl+Shift:l")
     (define pane (send (hash "OpenCommandPane" (list (hash "path" "/path/to/my/file.rs") (hash)))))
     (set! explicit-text (string-append "open_command_pane: "
       (if (json-null? pane) "None" (string-append "Some(" (yinz-json-string pane) ")"))))]
    [(or (equal? key "Ctrl+Shift:b") (equal? key "Ctrl+Shift:c"))
     (result-text "Layout dump error: "
       (lambda () (string-append "Layout dump success: "
         (yinz-command (hash "DumpLayout" (if (equal? key "Ctrl+Shift:b") "default" "nonexistent_layout_xyz"))))))]
    [(equal? key "Ctrl+Shift:d")
     (define folder (send "GetLayoutDir"))
     (set! explicit-text (if (and (string? folder) (not (equal? folder "")))
                            "Got layout folder" "Error: Got empty layout folder!"))]
    [(equal? key "Ctrl+Shift:e")
     (with-handler
       (lambda (failure) (set! explicit-text (string-append "Focused pane error: " (error-object-message failure))))
       (define info (yinz-command "GetFocusedPaneInfo"))
       (displayln (string-append "Focused pane: tab=" (text (hash-ref info "tab_id")) ", pane=" (text (hash-ref info "pane_id")))))]
    [(or (equal? key "Ctrl+Shift:f") (equal? key "Ctrl+Shift:g"))
     (define whole (equal? key "Ctrl+Shift:f"))
     (result-text (if whole "Session layout error: " "Tab layout error: ")
       (lambda ()
         (define result (yinz-command (hash "DumpSessionLayout" (hash "tab_index" (if whole json-null 0)))))
         (string-append (if whole "Session layout: " "Tab 0 layout: ")
           (number->string (string-length (hash-ref result "layout")))
           (if whole " tabs, metadata: " " bytes, metadata: ") (yinz-json-string (hash-ref result "metadata")))))]
    [(or (equal? key "Ctrl+Shift:h") (equal? key "Ctrl+Shift:i"))
     (define valid (equal? key "Ctrl+Shift:h"))
     (result-text "Parse error: "
       (lambda ()
         (define result (yinz-command (hash "ParseLayout"
           (if valid "layout { tab { pane; }; }" "layout { this is not valid kdl }"))))
         (if valid
             (if (= (length (hash-ref result "tabs")) 1) "Parse success" "Parse failure: got wrong metadata")
             (string-append "Parse success: " (yinz-json-string result)))))]
    [(equal? key "Ctrl+Shift:j")
     (define vars (send "GetSessionEnvironmentVariables"))
     (set! explicit-text (if (and (hash? vars) (> (length (hash-keys->list vars)) 0))
       (string-append "Got environment variables: " (yinz-json-string vars)) "Error: Got empty env vars"))]
    [(equal? key "Ctrl+Shift:k") (result-text "Data store error: " data-store-roundtrip)]
    [(or (equal? key "Alt+Ctrl:a") (equal? key "Alt+Ctrl:b"))
     (save "test_layout" "layout { tab { pane; }; }" #f "Save layout")]
    [(equal? key "Alt+Ctrl:c")
     (save "test_layout2" "layout { tab { pane; pane; }; }" #t "Save layout with overwrite")]
    [(equal? key "Alt+Ctrl:d") (save "invalid_layout" "not valid kdl at all" #f "Save invalid layout")]
    [(or (equal? key "Alt+Ctrl:e") (equal? key "Alt+Ctrl:f"))
     (define exists (equal? key "Alt+Ctrl:e"))
     (define label (if exists "Rename layout" "Rename nonexistent layout"))
     (layout-result label
       (hash "RenameLayout" (hash "old_layout_name" (if exists "test_layout" "nonexistent_layout")
                                 "new_layout_name" (if exists "renamed_layout" "new_name")))
       (string-append label " success"))]
    [(or (equal? key "Alt+Ctrl:g") (equal? key "Alt+Ctrl:h"))
     (define exists (equal? key "Alt+Ctrl:g"))
     (define label (if exists "Delete layout" "Delete nonexistent layout"))
     (layout-result label (hash "DeleteLayout" (hash "layout_name" (if exists "renamed_layout" "nonexistent_layout")))
       (string-append label " success"))]
    [(equal? key "Alt+Ctrl:i") (save "../evil_layout" "layout { tab { pane; } }" #f "Path traversal save")]
    [(equal? key "Alt+Ctrl:j")
     (layout-result "Edit layout"
       (hash "EditLayout" (hash "layout_name" "test_layout2" "context" (hash "test_key" "test_value")))
       "Edit layout command sent")]
    [(equal? key "Alt+Ctrl:k")
     (send (hash "OverrideLayout" (list (hash "BuiltIn" "compact") #f #f #f (hash "override_test" "value"))))
     (set! explicit-text "Override layout command sent")]))

(define (update event)
  (cond
    [(and (hash? event) (hash-contains? event "Key"))
     (when (equal? (get configuration "runaway" #f) "true")
       (send (hash "SetSelectable" #f))
       (let loop () (yinz-command "GetPluginIds") (loop)))
     (define key (hash-ref event "Key"))
     (define char (get (hash-ref key "bare_key") "Char" #f))
     (when char
       (define modifiers (sort (hash-ref key "key_modifiers") string<?))
       (define id (string-append
         (if (null? modifiers) "" (string-append (string-join modifiers "+") ":")) char))
       (if (hash-contains? commands id) (send (hash-ref commands id)) (special-key id)))]
    [(and (hash? event) (hash-contains? event "CustomMessage"))
     (define message (hash-ref event "CustomMessage"))
     (when (equal? (car message) "pong") (set! received-payload (cadr message)))]
    [(equal? event "BeforeClose")
     (send (hash "HighlightAndUnhighlightPanes" (list (list (hash "Terminal" 1)) (list (hash "Plugin" 1)))))]
    [(and (hash? event) (hash-contains? event "PluginConfigurationChanged"))
     (set! configuration (hash-ref event "PluginConfigurationChanged"))]
    [(equal? event "SystemClipboardFailure") (yinz-post-worker "test" "ping" "gimme_back_my_payload")])
  (set! received-events (append received-events (list event)))
  #t)

(define (pipe message)
  (define source (hash-ref message "source"))
  (define input (cond [(equal? source "Keybind") "keybind"]
                     [(hash-contains? source "Cli") (hash-ref source "Cli")]
                     [else (number->string (hash-ref source "Plugin"))]))
  (define name (hash-ref message "name"))
  (define payload (hash-ref message "payload"))
  (cond
    [(and (equal? name "message_name") (equal? payload "message_payload"))
     (send (hash "UnblockCliPipeInput" input))]
    [(equal? name "message_name_block")
     (send (hash "BlockCliPipeInput" input))
     (set! explicit-text (string-append input " blocked"))]
    [(equal? name "pipe_output") (send (hash "CliPipeOutput" (list name "this_is_my_output")))]
    [(equal? name "pipe_message_to_plugin")
     (send (hash "MessageToPlugin" (hash "message_name" "message_to_plugin" "message_payload" "my_cool_payload")))]
    [(equal? name "message_to_plugin") (set! message-to-plugin-payload (if (json-null? payload) #f payload))]
    [(equal? name "panic_while_handling_pipe") (error "intentional panic for the pipe-release-on-crash test")]
    [(equal? name "enqueue_queue_job")
     (result-text "enqueue failed: "
       (lambda () (string-append "enqueued job "
         (text (yinz-command (hash "EnqueueQueueJob"
           (hash "queue" "fixture_queue" "plugin_url" (if (json-null? payload) "" payload)
                 "config" (hash) "context" (hash "origin" "fixture"))))))))]
    [(equal? name "finish_queue_job") (send (hash "FinishQueueJob" (hash "success" #t "payload" payload)))])
  #t)

(define (render rows cols)
  (string-append
    (cond
      [received-payload (string-append "Payload from worker: " (yinz-json-string received-payload))]
      [message-to-plugin-payload
       (define result (string-append "Payload from self: " (yinz-json-string message-to-plugin-payload)))
       (set! message-to-plugin-payload #f) result]
      [explicit-text (define result (trim explicit-text)) (set! explicit-text #f) result]
      [else (string-append "Rows: " (number->string rows) ", Cols: " (number->string cols)
              ", Received events: " (yinz-json-string received-events))]) "\n"))

(define (data-attr name kind indexed full-text)
  (hash "name" name "kind" kind "indexed" indexed "full_text" full-text
        "unique" #f "required" #f "default" json-null))
(define (data-request)
  (hash "namespace" json-null "entity" "fixture_agent" "filter" json-null "search" json-null
        "sort" '() "select" '() "limit" 500 "offset" 0 "after_id" json-null))
(define (data-store-roundtrip)
  (define info (yinz-command (hash "DataDefineEntity"
    (hash "namespace" json-null "allow_destructive" #f
      "def" (hash "name" "fixture_agent" "version" 1 "attrs"
        (list (data-attr "project" "Text" #t #f) (data-attr "summary" "Text" #f #t)
              (data-attr "updated_at_ms" "Int" #t #f)))))))
  (define request (data-request))
  (define watch (yinz-command (hash "DataWatch" (hash "request" request "mode" "Rows"))))
  ;; Carry the revision read here into each put, including repeated key presses.
  (define existing (yinz-command (hash "DataFetch"
    (hash "request" (hash-insert request "select" '("uid" "rev"))))))
  (define (revision uid)
    (define found (filter (lambda (row) (equal? (hash-ref (car row) "Text") uid)) (hash-ref existing "rows")))
    (if (null? found) 0 (hash-ref (cadr (car found)) "Int")))
  (define (put uid summary timestamp)
    (hash "Put" (hash "entity" "fixture_agent" "uid" uid "expect_rev" (revision uid)
      "values" (list (list "project" (hash "Text" "yinz")) (list "summary" (hash "Text" summary))
                     (list "updated_at_ms" (hash "Int" timestamp))))))
  (with-handler
    (lambda (failure)
      (send (hash "DataUnwatch" (hash "watch_id" (hash-ref watch "watch_id"))))
      (error (error-object-message failure)))
    (yinz-command (hash "DataApply" (hash "namespace" json-null "mutations"
      (list (put "fixture-a" "fixing a rebase conflict" 2) (put "fixture-b" "writing the changelog" 1)))))
    (define rows (yinz-command (hash "DataFetch"
      (hash "request" (hash-insert (hash-insert (hash-insert request
        "filter" (hash "Eq" (list "project" (hash "Text" "yinz"))))
        "sort" (list (list "updated_at_ms" "Desc"))) "select" '("summary"))))))
    (define found (map (lambda (row) (hash-ref (car row) "Text")) (hash-ref rows "rows")))
    (define hits (yinz-command (hash "DataFetch" (hash "request" (hash-insert request "search" "rebase")))))
    (define count (yinz-command (hash "DataCount" (hash "request" request))))
    (yinz-command (hash "DataUnwatch" (hash "watch_id" (hash-ref watch "watch_id"))))
    (string-append "Data store: created " (if (hash-ref info "created") "true" "false")
      ", rows [" (string-join (map yinz-json-string found) ", ") "]"
      ", search hits " (number->string (length (hash-ref hits "rows")))
      ", count " (number->string count) ", watch " (if (> (hash-ref watch "watch_id") 0) "true" "false"))))
