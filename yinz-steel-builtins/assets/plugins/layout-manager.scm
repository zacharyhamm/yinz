(require "./layout-manager/common.scm" "./layout-manager/list-render.scm" "./layout-manager/forms-render.scm")
(define layouts '())
(define (text-input value) (yinz-ui-form (list (yinz-ui-input "name" "" value))))
(define (new-list selected renamed)
  (hash "kind" "list" "selected" selected "retain-terminal" renamed "retain-plugin" #f "active-tab" #f "more" #f
    "filter" (text-input "") "typing" #t "active" #f "results" '() "search-index" 0 "rows" 0 "cols" 0))
(define screen (new-list 0 #f))
(define (get key) (hash-ref screen key))
(define (put key value) (set! screen (hash-insert screen key value)))
(define (selected) (lm-at layouts (get "selected")))
(define (active-tab-default)
  (define entry (selected))
  (and entry (or (hash-ref entry "builtin") (= (length (lm-get (lm-get entry "metadata" #f) "tabs" '())) 1))))
(define (filter-text) (yinz-ui-value (get "filter") "name"))
(define (filter-refresh)
  (define geo (lm-list-geometry screen layouts (get "rows") (get "cols")))
  (define y (+ (hash-ref geo "y") (if (or (get "typing") (get "active")) 1 0)))
  (define input (filter-text))
  (if (and (equal? input "") (not (get "typing")))
    (begin (put "results" '()) (put "search-index" 0))
    (begin
      (put "results" (yinz-ui-fuzzy-filter (map (lambda (entry) (hash-ref entry "name")) layouts) input))
      (put "active" (not (null? (get "results"))))
      (when (>= (get "search-index") (length (get "results"))) (put "search-index" (lm-sat (length (get "results")) 1)))
      (lm-cursor (if (get "typing") (list (+ 8 (yinz-ui-input-op (get "filter") "name" "CursorPosition") (hash-ref geo "x")) y) json-null))))
  (define result (lm-at (get "results") (get "search-index")))
  (when result (put "selected" (hash-ref result "index"))))
(define (selection-refresh)
  (when (>= (get "selected") (length layouts)) (put "selected" (lm-sat (length layouts) 1)))
  (when (and (get "active") (not (equal? (filter-text) ""))) (filter-refresh))
  (put "active-tab" (active-tab-default)))
(define (clear-filter)
  (put "typing" #f) (put "active" #f) (put "results" '()) (put "search-index" 0)
  (yinz-ui-set-value! (get "filter") "name" "") (lm-cursor json-null) (put "selected" 0))
(define (navigate direction)
  (define searching (lm-searching? screen)) (define key (if searching "search-index" "selected"))
  (define count (length (if searching (get "results") layouts)))
  (define old (get key))
  (put key (if (< direction 0) (if (= old 0) (lm-sat count 1) (- old 1)) (if (>= (+ old 1) count) 0 (+ old 1))))
  (when searching (define result (lm-at (get "results") (get "search-index"))) (put "selected" (if result (hash-ref result "index") 0)))
  (put "active-tab" (active-tab-default)))
(define (error-screen message return) (set! screen (hash "kind" "error" "message" message "return" return)) #t)
(define (show-error message) (error-screen message screen))
(define (optimistic kind old value)
  (set! layouts (lm-sort (cond
        [(equal? kind "delete") (filter (lambda (entry) (not (equal? (lm-get entry "file" #f) old))) layouts)]
        [(equal? kind "add") (append layouts (list (lm-layout (hash "File" (list old value)))))]
        [else (map (lambda (entry) (if (equal? (lm-get entry "file" #f) old)
                (if (lm-get entry "error" #f) (hash-insert (hash-insert entry "file" value) "name" value)
                  (lm-layout (hash "File" (list value (hash-ref entry "metadata"))))) entry)) layouts)])))
  (when (equal? (get "kind") "list") (selection-refresh)))
(define (open-layout apply)
  (define entry (selected))
  (when (and entry (not (lm-get entry "error" #f)))
    (if apply
      (yinz-command (hash "OverrideLayout" (list (hash-ref entry "info") (get "retain-terminal") (get "retain-plugin") (get "active-tab") (hash))))
      (let ([tabs (yinz-command (hash "NewTabsWithLayoutInfo" (hash-ref entry "info")))])
        (when (and (active-tab-default) (not (null? tabs)))
          (yinz-command (hash "RenameTabWithId" (list (car tabs) (hash-ref entry "name")))))))
    (yinz-command "CloseSelf")) #f)
(define (new-form layout metadata)
  (hash "kind" "new" "layout" layout "metadata" metadata
    "form" (yinz-ui-form (list (yinz-ui-input "name" "Save as" "")
        (yinz-ui-select "target" "Save" '("All Tabs" "Current Tab Only") 0)
        (yinz-ui-button "save" "Save") (yinz-ui-button "update" "Update Layout") (yinz-ui-button "cancel" "Cancel")))))
(define (start-new)
  (define return screen)
  (with-handler (lambda (failure) (error-screen (string-append "Failed to dump session layout: " (lm-error-message failure)) return))
    (define dump (yinz-command (hash "DumpSessionLayout" (hash "tab_index" json-null))))
    (if (lm-present? (hash-ref dump "metadata")) (set! screen (new-form (hash-ref dump "layout") (hash-ref dump "metadata")))
      (error-screen "Failed to retrieve session layout metadata" return))))
(define (list-key key)
  (define bare (hash-ref key "bare_key")) (define mods (hash-ref key "key_modifiers")) (define plain (null? mods))
  (define (char? letter) (and plain (equal? bare (hash "Char" letter))))
  (cond [(and plain (equal? bare "Up")) (navigate -1) #t]
    [(and plain (equal? bare "Down")) (navigate 1) #t]
    [(and plain (equal? bare "Enter")) (open-layout #f)]
    [(and (equal? mods '("Alt")) (equal? bare (hash "Char" "w"))) (open-layout #t)]
    [(get "typing")
      (cond [(and plain (equal? bare "Esc"))
          (if (equal? (filter-text) "") (clear-filter) (begin (yinz-ui-set-value! (get "filter") "name" "") (filter-refresh))) #t]
        [(and plain (equal? bare "Tab"))
          (define result (lm-at (get "results") (get "search-index")))
          (when result (define name (hash-ref (list-ref layouts (hash-ref result "index")) "name"))
            (unless (equal? name (filter-text)) (yinz-ui-set-value! (get "filter") "name" name) (filter-refresh))) #t]
        [else (define action (yinz-ui-input-op (get "filter") "name" "Key" key))
          (cond [(equal? action "continue") (filter-refresh) #t]
            [(equal? action "cancel")
              (if (equal? (filter-text) "") (clear-filter) (begin (yinz-ui-set-value! (get "filter") "name" "") (filter-refresh))) #t]
            [else #f])])]
    [(char? "/") (put "typing" #t) (filter-refresh) #t]
    [(and plain (equal? bare "Esc")) (yinz-command "CloseSelf") #f]
    [(and plain (equal? bare "Delete"))
      (define file (lm-get (selected) "file" #f))
      (if file (with-handler (lambda (failure) (show-error (lm-error-message failure)))
          (yinz-command (hash "DeleteLayout" (hash "layout_name" file))) (optimistic "delete" file #f) #t) #f)]
    [(char? "e") (define file (lm-get (selected) "file" #f))
      (when file (with-handler (lambda (failure) #f) (yinz-command (hash "EditLayout" (hash "layout_name" file "context" (hash)))))) #f]
    [(char? "i") (set! screen (hash "kind" "import" "input" (text-input "") "editing" #f "pasted" #f "metadata" #f "parse-error" #f)) #t]
    [(char? "n") (start-new) #t]
    [(char? "r") (define file (lm-get (selected) "file" #f))
      (when file (set! screen (hash "kind" "rename" "old" file "selected" (get "selected")
            "form" (yinz-ui-form (list (yinz-ui-input "name" "Rename Layout" file) (yinz-ui-button "save" "Save") (yinz-ui-button "cancel" "Cancel")))))) #t]
    [(char? "t") (define t (get "retain-terminal")) (define p (get "retain-plugin"))
      (put "retain-terminal" (not t)) (put "retain-plugin" (if t (not p) p)) #t]
    [(char? "a") (put "active-tab" (not (get "active-tab"))) #t]
    [(char? "?") (put "more" (not (get "more"))) #t]
    [(char? "m") (define entry (selected))
      (when (lm-get entry "error" #f) (set! screen (hash "kind" "error-detail" "name" (hash-ref entry "name") "message" (hash-ref entry "error") "return" screen))) #t]
    [else #f]))
(define (cancel-form)
  (lm-cursor json-null)
  (set! screen (new-list (if (equal? (get "kind") "rename") (get "selected") 0) (equal? (get "kind") "rename"))) #t)
(define (rename-save)
  (define name (yinz-ui-value (get "form") "name"))
  (with-handler (lambda (failure) (lm-cursor json-null) (show-error (lm-error-message failure)))
    (when (equal? name "") (error "Layout name cannot be empty"))
    (yinz-command (hash "RenameLayout" (hash "old_layout_name" (get "old") "new_layout_name" name)))
    (optimistic "rename" (get "old") name) (cancel-form)))
(define (save-layout)
  (define importing (equal? (get "kind") "import"))
  (define input (if importing (get "input") (get "form")))
  (cond [(and importing (not (get "pasted"))) (show-error "Please paste a layout first")]
    [(and importing (get "parse-error")) (show-error "Cannot save an invalid layout")]
    [(and importing (not (get "metadata"))) (show-error "Cannot save layout without metadata")]
    [else
      (define name (yinz-ui-value input "name"))
      (when (equal? name "") (set! name (yinz-command "GenerateRandomName")))
      (with-handler (lambda (failure) (show-error (string-append "Failed to save layout '" name "': " (lm-error-message failure))))
        (yinz-command (hash "SaveLayout" (hash "layout_name" name "layout_kdl" (if importing (get "pasted") (get "layout")) "overwrite" #f)))
        (optimistic "add" name (get "metadata"))
        (unless importing (lm-cursor json-null))
        (set! screen (new-list 0 #f)) #t)]))
(define (refresh-session)
  (define focus (with-handler (lambda (failure) #f) (yinz-command "GetFocusedPaneInfo")))
  (when focus
    (with-handler (lambda (failure) (error-screen (string-append "Failed to dump session layout: " (lm-error-message failure)) (new-list 0 #f)))
      (define dump (yinz-command (hash "DumpSessionLayout" (hash "tab_index" (if (equal? (yinz-ui-value (get "form") "target") 1) (hash-ref focus "tab_id") json-null)))))
      (if (lm-present? (hash-ref dump "metadata")) (begin (put "layout" (hash-ref dump "layout")) (put "metadata" (hash-ref dump "metadata")))
        (error-screen "Failed to retrieve session layout metadata" (new-list 0 #f))))) #t)
(define (form-event action mouse)
  (define kind (hash-ref action "kind")) (define id (lm-get action "id" #f))
  (cond [(equal? kind "buttonPressed")
      (cond [(equal? id "save") (if (equal? (get "kind") "rename") (rename-save) (save-layout))]
        [(equal? id "update") (refresh-session)] [else (cancel-form)])]
    [(equal? kind "cancel") (cancel-form)]
    [(and (equal? kind "fieldChanged") (equal? id "target") (equal? (get "kind") "new")) (refresh-session)]
    [(member kind '("fieldChanged" "focusMoved")) #t]
    [else (and mouse (not (equal? kind "ignored")))]))
(define (import-key key)
  (define bare (hash-ref key "bare_key")) (define plain (null? (hash-ref key "key_modifiers")))
  (cond [(and plain (equal? bare "Esc")) (if (get "editing") (put "editing" #f) (set! screen (new-list 0 #f))) #t]
    [(get "editing")
      (define action (yinz-ui-input-op (get "input") "name" "Key" key))
      (cond [(member action '("submit" "cancel")) (put "editing" #f) #t] [(equal? action "continue") #t] [else #f])]
    [(and plain (equal? bare "Enter")) (save-layout)]
    [(and plain (equal? bare (hash "Char" "r"))) (put "editing" #t) #t]
    [else #f]))
(define (load config)
  (yinz-command (hash "Subscribe" '("PastedText" "SessionUpdate" "Key" "Mouse" "AvailableLayoutInfo" "PermissionRequestResult")))
  (define ids (yinz-command "GetPluginIds"))
  (yinz-command (hash "RenamePluginPane" (list (hash-ref ids "plugin_id") "Layout Manager"))))
(define (update event)
  (cond [(lm-get event "AvailableLayoutInfo" #f)
      (define payload (hash-ref event "AvailableLayoutInfo"))
      (set! layouts (lm-sort (append (map lm-layout (car payload)) (map lm-layout-error (cadr payload)))))
      (when (equal? (get "kind") "list") (selection-refresh)) #t]
    [(lm-get event "PastedText" #f)
      (define text (hash-ref event "PastedText"))
      (cond [(equal? (get "kind") "import")
          (put "pasted" text)
          (with-handler (lambda (failure) (put "metadata" #f) (put "parse-error" (yinz-format-layout-error text)))
            (put "metadata" (yinz-command (hash "ParseLayout" text))) (put "parse-error" #f)) #t]
        [(member (get "kind") '("new" "rename")) (not (equal? (hash-ref (yinz-ui-paste! (get "form") text) "kind") "ignored"))] [else #f])]
    [(lm-get event "Key" #f)
      (define key (hash-ref event "Key"))
      (cond [(equal? (get "kind") "list") (list-key key)] [(equal? (get "kind") "import") (import-key key)]
        [(member (get "kind") '("new" "rename")) (form-event (yinz-ui-handle-key (get "form") key) #f)]
        [else (set! screen (get "return")) #t])]
    [(and (lm-get event "Mouse" #f) (member (get "kind") '("new" "rename"))) (form-event (yinz-ui-handle-mouse (get "form") (hash-ref event "Mouse")) #t)]
    [else #f]))
(define (render rows cols)
  (if (equal? (get "kind") "list")
    (begin (lm-cursor json-null) (put "rows" rows) (put "cols" cols) (filter-refresh) (lm-list-render screen layouts rows cols))
    (lm-screen-render screen rows cols)))
