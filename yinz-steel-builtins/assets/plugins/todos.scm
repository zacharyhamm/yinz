;; Todos owns its watches, drafts, selection, and guarded write lifecycle in the guest.
;; Pure shared model calls decode rows and plan mutations; every read/write uses the host.
(define (at object key fallback) (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))
(define (ormap proc values) (and (not (null? values)) (or (proc (car values)) (ormap proc (cdr values)))))
(define (string-find text needle) (let loop ([i 0]) (cond [(> (+ i (string-length needle)) (string-length text)) #f] [(equal? (substring text i (+ i (string-length needle))) needle) i] [else (loop (+ i 1))])))
(define (present value) (and value (not (json-null? value))))
(define (optional value) (if value value json-null))
(define (sat a b) (max 0 (- a b)))
(define (nth values n) (if (and (>= n 0) (< n (length values))) (list-ref values n) #f))
(define (take-n xs n) (if (or (<= n 0) (null? xs)) '() (cons (car xs) (take-n (cdr xs) (- n 1)))))
(define (drop-n xs n) (if (or (<= n 0) (null? xs)) xs (drop-n (cdr xs) (- n 1))))
(define (range-n a b) (if (>= a b) '() (cons a (range-n (+ a 1) b))))
(define (indexed proc xs) (let loop ([xs xs] [n 0]) (unless (null? xs) (proc n (car xs)) (loop (cdr xs) (+ n 1)))))
(define (position pred xs) (let loop ([xs xs] [n 0]) (cond [(null? xs) #f] [(pred (car xs)) n] [else (loop (cdr xs) (+ n 1))])))
(define (model op . args) (yinz-agent-model "todos" op args))
(define (panel op . args) (yinz-agent-model "panel" op args))
(define (failure-text failure) (trim (error-object-message failure)))
(define (try proc) (with-handler (lambda (failure) (hash "Err" (failure-text failure))) (hash "Ok" (proc))))
(define (result-value result) (if (hash-contains? result "Err") (error (hash-ref result "Err")) (hash-ref result "Ok")))
(define (empty-list) (hash "next_id" 0 "items" '()))
(define state (hash "list" (empty-list) "projects" (hash) "root" #f "project-watch" #f "todo-watch" #f
    "selected" #f "show-done" #f "status" "" "note" #f "drafts" '() "parked" (hash)
    "scroll" #f "window" 0 "list-rows" 0 "header-rows" 0 "rendered-ids" '() "rendered-projects" '()
    "cache" #f "overwrite-shown" #f "initial-cwd" "" "plugin-id" 0 "client-id" 0
    "pane-tabs" #f "bar-focused" '() "tabs-focused" #f "pane-shown" #f "keyboard" 'unknown
    "sheet" #f "sheet-rows" 0 "sheet-max" 0 "rows" 0 "cols" 0 "footer-row" #f "mouse" #f))
(define mode (hash "kind" "normal"))
(define (get key) (hash-ref state key))
(define (put key value) (set! state (hash-insert state key value)))
(define (mget key) (at mode key #f))
(define (mput key value) (set! mode (hash-insert mode key value)))
(define (kind? value) (equal? (mget "kind") value))
(define (normal!) (set! mode (hash "kind" "normal")))
(define (item id) (and id (let ([value (model "Item" (get "list") id)]) (and (present value) value))))
(define (selected) (item (get "selected")))
(define (text-of id) (at (item id) "text" ""))
(define (children value) (at value "children" '()))
(define (flatten items depth show-done)
  (foldr append '() (map (lambda (value) (if (and (at value "done" #f) (not show-done)) '()
          (cons (hash-insert value "depth" depth) (flatten (children value) (+ depth 1) show-done)))) items)))
(define (visible) (flatten (hash-ref (get "list") "items") 0 (get "show-done")))
(define (project-values) (map (lambda (key) (hash-ref (get "projects") key)) (sort (hash-keys->list (get "projects")) string<?)))
(define (invalidate) (put "cache" #f) (put "scroll" #f))
(define (nearest before after selected)
  (define ids (map (lambda (row) (hash-ref row "id")) after))
  (define index (and selected (position (lambda (row) (= selected (hash-ref row "id"))) before)))
  (cond [(and selected (member selected ids)) selected]
    [index (or (let ([found (filter (lambda (row) (member (hash-ref row "id") ids)) (append (drop-n before (+ index 1)) (reverse (take-n before index))))]) (and (not (null? found)) (hash-ref (car found) "id"))) (nth ids 0))]
    [else (nth ids 0)]))
(define (set-note text . hint) (put "note" (hash "lead" text "hint" (if (null? hint) "" (car hint)) "error" #t)))
(define (done-note verb text count)
  (hash "lead" verb "item" text "subtasks" count "error" #f "hint" (if (and (equal? verb "completed") (not (get "show-done"))) "v shows done" "")))
(define (request entity)
  (hash-insert (yinz-agent-model "store" "EntityRequest" entity) "limit" (yinz-agent-model "store" "FetchLimit" json-null)))
(define (todo-request)
  (hash-insert (request "todo") "filter" (hash "Eq" (list "root" (hash "Text" (or (get "root") ""))))))
(define (unwatch id) (when id (try (lambda () (yinz-command (hash "DataUnwatch" (hash "watch_id" id)))))))
(define (take-rows rows)
  (define before (visible))
  (define actual (if (hash-ref rows "truncated") (yinz-data-fetch-all "shared:yinz" (todo-request)) rows))
  (define lists (model "FromRows" actual))
  (put "list" (at lists (or (get "root") "") (empty-list)))
  (put "selected" (nearest before (visible) (get "selected")))
  (invalidate) (put "status" "") (check-mode))
(define (reload-list) (take-rows (yinz-data-fetch-all "shared:yinz" (todo-request))))
(define (watch-root root)
  (define same (equal? root (get "root")))
  (when (kind? "input") (close-input))
  (normal!) (invalidate) (put "note" #f)
  (unless same
    (unwatch (get "todo-watch")) (put "todo-watch" #f)
    (when (and (get "root") (not (null? (get "drafts")))) (put "parked" (hash-insert (get "parked") (get "root") (get "drafts"))))
    (put "root" root) (put "drafts" (if root (at (get "parked") root '()) '()))
    (when root (put "parked" (hash-remove (get "parked") root)))
    (put "list" (empty-list)) (put "selected" #f))
  (if (not root) (put "status" "")
    (with-handler (lambda (failure) (put "status" (failure-text failure)))
      (define handle (yinz-data-watch "shared:yinz" (hash-insert (todo-request) "sort" '(("id" "Asc"))) "Rows"))
      (define old (get "todo-watch"))
      (put "todo-watch" (hash-ref handle "watch_id"))
      (when (and old (not (= old (get "todo-watch")))) (unwatch old))
      (put "status" "") (take-rows (hash-ref handle "rows")))))
(define (take-projects rows)
  (define registry (yinz-agent-model "projects" "FromRows" (list rows)))
  (define previous (get "root"))
  (define kept (and previous (ormap (lambda (project) (equal? (hash-ref project "root") previous)) (hash-values->list registry))))
  (define project (and (not kept) (yinz-agent-model "projects" "ForCwd" (list registry (get "initial-cwd")))))
  (define root (if kept previous (and (present project) (hash-ref project "root"))))
  (put "projects" registry)
  (unless (equal? (get "root") root) (watch-root root)))
(define (watch-projects)
  (with-handler (lambda (failure) (put "status" (failure-text failure)))
    (yinz-data-define-all "shared:yinz" (yinz-agent-model "store" "SharedDefs" json-null))
    (define handle (yinz-data-watch "shared:yinz" (request "project") "Rows"))
    (put "status" "") (put "project-watch" (hash-ref handle "watch_id")) (take-projects (hash-ref handle "rows"))))
(define (action-target action) (list (hash-ref action "kind") (at action "id" (at action "parent" #f))))
(define (draft? action input)
  (define text (yinz-ui-value input "text"))
  (and (not (equal? (trim text) ""))
    (not (and (equal? (hash-ref action "kind") "edit")
        (or (equal? text (hash-ref action "original")) (equal? text (text-of (hash-ref action "id"))))))))
(define (begin-input action text)
  (define target (action-target action))
  (define kept (filter (lambda (draft) (equal? (action-target (hash-ref draft "action")) target)) (get "drafts")))
  (put "drafts" (filter (lambda (draft) (not (equal? (action-target (hash-ref draft "action")) target))) (get "drafts")))
  (set! mode (if (null? kept) (hash "kind" "input" "action" action "input" (yinz-ui-form (list (yinz-ui-input "text" "" text))) "refusal" #f)
      (hash-insert (car kept) "kind" "input"))) #t)
(define (close-input)
  (when (kind? "input")
    (when (draft? (mget "action") (mget "input"))
      (put "drafts" (append (get "drafts") (list (hash "action" (hash-insert (mget "action") "theirs" #f) "input" (mget "input") "refusal" #f))))))
  (normal!))
(define (orphaned action)
  (cond [(and (equal? (hash-ref action "kind") "edit") (not (item (hash-ref action "id")))) "deleted elsewhere · Enter adds it as new"]
    [(and (equal? (hash-ref action "kind") "add") (at action "parent" #f) (not (item (hash-ref action "parent")))) "its parent was deleted elsewhere · Enter adds it at the top"]
    [else #f]))
(define (check-mode)
  (when (and (member (mget "kind") '("delete" "complete" "move")) (not (item (mget "id")))) (normal!) (set-note "deleted elsewhere"))
  (when (kind? "move") (define targets (move-targets (mget "id"))) (when (and (mget "target") (not (member (mget "target") targets))) (mput "target" (nth targets 0))))
  (when (kind? "input")
    (define why (orphaned (mget "action")))
    (when why
      (if (and (equal? (at (mget "action") "kind" #f) "edit") (equal? (yinz-ui-value (mget "input") "text") (at (mget "action") "original" #f)))
        (begin (normal!) (set-note "deleted elsewhere"))
        (begin (mput "action" (hash "kind" "add" "parent" #f)) (mput "refusal" why)))))
  (put "drafts" (map (lambda (draft) (if (orphaned (hash-ref draft "action")) (hash-insert draft "action" (hash "kind" "add" "parent" #f)) draft)) (get "drafts"))))
(define (add-input parent)
  (define action (hash "kind" "add" "parent" parent))
  (define limit
    (cond [(not (get "root"))
        (cond [(= (length (hash-keys->list (get "projects"))) 0)
            (if (and (not (get "project-watch")) (not (equal? (get "status") ""))) (list "projects not loaded" "Ctrl-r retry") (list "no projects yet" "start an agent in a git checkout"))]
          [else (list "no project picked" "p to pick one")])]
      [(>= (car (model "Counts" (get "list"))) 200) '("200 open todos max" "complete some first")]
      [(and parent (>= (length (model "Trail" (get "list") parent)) 5)) '("too deep to nest" "todos nest 5 levels at most")]
      [else #f]))
  (cond [(not limit) (begin-input action "")]
    [(and (get "root") (ormap (lambda (draft) (equal? (action-target (hash-ref draft "action")) (action-target action))) (get "drafts")))
      (begin-input action "") (refuse (string-join limit " · ")) #t]
    [else (apply set-note limit) #t]))
(define (refuse text) (when (kind? "input") (mput "refusal" text) (put "overwrite-shown" #f)))
(define (plan-input action text)
  (if (equal? (hash-ref action "kind") "add") (model "PlanAdd" (get "list") (get "root") text (optional (at action "parent" #f)))
    (model "PlanEdit" (get "list") (hash-ref action "id") text (or (at action "theirs" #f) (hash-ref action "rev")))))
(define (apply-plan planned)
  (if (hash-contains? planned "Err") (hash "refused" (hash-ref planned "Err"))
    (let ([written (try (lambda () (yinz-data-apply "shared:yinz" (car (hash-ref planned "Ok")))))])
      (cond [(hash-contains? written "Ok")
          (define refreshed (try reload-list))
          (when (hash-contains? refreshed "Err") (put "status" (string-append "saved, but could not refresh: " (hash-ref refreshed "Err"))))
          (hash "saved" (hash-ref (hash-ref written "Ok") "ids"))]
        [(string-contains? (string-downcase (hash-ref written "Err")) "conflict") (hash "conflict" (try reload-list))]
        [else (hash "refused" (hash-ref written "Err"))]))))
(define (write-plan planned success)
  (define outcome (apply-plan planned))
  (cond [(hash-contains? outcome "saved") (put "note" success)]
    [(hash-contains? outcome "refused") (set-note (hash-ref outcome "refused"))]
    [else (set-note "changed elsewhere" (if (hash-contains? (hash-ref outcome "conflict") "Ok") "list reloaded, try again" "Ctrl-r reload"))]))
(define (trail-revs items parent)
  (if (not parent) '()
    (let walk ([items items])
      (if (null? items) #f
        (let* ([value (car items)] [entry (list (hash-ref value "id") (at value "rev" 0))]
            [below (if (= parent (hash-ref value "id")) '() (walk (children value)))])
          (if below (cons entry below) (walk (cdr items))))))))
(define (submit-input)
  (when (and (get "root") (kind? "input"))
    (define action (mget "action")) (define text (yinz-ui-value (mget "input") "text"))
    (cond [(equal? (trim text) "") (normal!)]
      [(and (at action "theirs" #f) (not (get "overwrite-shown"))) #t]
      [else
        (mput "refusal" #f)
        (define trail (and (equal? (hash-ref action "kind") "add") (trail-revs (hash-ref (get "list") "items") (at action "parent" #f))))
        (define outcome (apply-plan (plan-input action text)))
        (when (and (hash-contains? outcome "conflict") (hash-contains? (hash-ref outcome "conflict") "Ok")
            (kind? "input") (not (mget "refusal")) trail (equal? trail (trail-revs (hash-ref (get "list") "items") (at action "parent" #f))))
          (set! outcome (apply-plan (plan-input action text))))
        (cond [(hash-contains? outcome "saved")
            (normal!)
            (when (and (equal? (hash-ref action "kind") "add") (not (null? (hash-ref outcome "saved"))))
              (put "selected" (car (hash-ref outcome "saved"))) (put "scroll" #f))
            (put "note" (done-note (if (equal? (hash-ref action "kind") "add") "added" "saved") text 0))]
          [(hash-contains? outcome "refused") (when (kind? "input") (mput "action" (hash-insert (mget "action") "theirs" #f))) (refuse (hash-ref outcome "refused"))]
          [(hash-contains? (hash-ref outcome "conflict") "Err")
            (when (kind? "input") (mput "action" (hash-insert (mget "action") "theirs" #f)))
            (refuse (string-append "changed elsewhere · could not reload: " (hash-ref (hash-ref outcome "conflict") "Err")))]
          [(and (kind? "input") (not (mget "refusal")))
            (define theirs (and (equal? (hash-ref action "kind") "edit") (at (item (hash-ref action "id")) "rev" #f)))
            (mput "action" (hash-insert (mget "action") "theirs" theirs))
            (refuse (if theirs "changed elsewhere · Enter overwrites · Esc keeps theirs · yours stays a draft" "changed elsewhere · list reloaded · Enter tries again"))])])) #t)
(define (toggle)
  (define value (selected))
  (if (and (get "root") value)
    (let ([plan (model "PlanDone" (get "list") (get "root") (hash-ref value "id") (not (at value "done" #f)))])
      (cond [(hash-contains? plan "Err") (set-note (hash-ref plan "Err"))]
        [else (define change (cadr (hash-ref plan "Ok")))
          (if (and (not (at value "done" #f)) (not (null? (hash-ref change "cascaded"))))
            (set! mode (hash "kind" "complete" "id" (hash-ref value "id") "plan" plan "count" (length (hash-ref change "cascaded"))))
            (write-plan plan (done-note (if (at value "done" #f) "reopened" "completed") (hash-ref value "text") 0)))]) #t) #f))
(define (delete)
  (define id (get "selected"))
  (cond [(not (and id (get "root"))) #f] [(not (item id)) (set-note "deleted elsewhere") #t]
    [else (define plan (model "PlanDelete" (get "list") (get "root") id #t))
      (if (hash-contains? plan "Err") (set-note (hash-ref plan "Err"))
        (set! mode (hash "kind" "delete" "id" id "plan" plan "count" (- (length (cadr (hash-ref plan "Ok"))) 1)))) #t]))
(define (move-targets id)
  (define excluded (cons id (map (lambda (value) (hash-ref value "id")) (flatten (children (item id)) 0 #t))))
  (filter (lambda (id) (not (member id excluded))) (map (lambda (row) (hash-ref row "id")) (visible))))
(define (begin-move)
  (define value (selected))
  (if (not value) #f
    (let ([targets (move-targets (hash-ref value "id"))])
      (if (null? targets) (set-note "nowhere to move it" "add another todo first")
        (set! mode (hash "kind" "move" "id" (hash-ref value "id") "rev" (at value "rev" 0) "target" (car targets)))) #t)))
(define (parent-of items id parent)
  (if (null? items) 'missing
    (let ([value (car items)])
      (if (= (hash-ref value "id") id) parent
        (let ([child (parent-of (children value) id (hash-ref value "id"))])
          (if (equal? child 'missing) (parent-of (cdr items) id parent) child))))))
(define (commit-move target)
  (define id (mget "id")) (define rev (mget "rev")) (normal!)
  (when (get "root")
    (if (equal? target (parent-of (hash-ref (get "list") "items") id #f))
      (set-note (if target "already under it" "already top level"))
      (write-plan (model "PlanMove" (get "list") (get "root") id (optional target) rev) (done-note "moved" (text-of id) 0)))))
(define (focus-tabs bar) (when (get "pane-tabs") (try (lambda () (yinz-command (hash "PaneTabs" (hash "Focus" (hash "id" (get "pane-tabs") "bar" bar))))))))
(define (take-stop stop)
  (put "tabs-focused" (equal? stop "management_tabs"))
  (put "bar-focused" (filter (lambda (id) (not (= id (get "client-id")))) (get "bar-focused")))
  (when (get "tabs-focused") (put "bar-focused" (append (get "bar-focused") (list (get "client-id")))))
  (focus-tabs (get "tabs-focused")) #t)
(define (ring-key key)
  (and (equal? (hash-ref key "bare_key") "Tab")
    (begin (if (equal? (hash-ref key "key_modifiers") '("Shift")) (put "tabs-focused" #t) (take-stop "management_tabs")) #t)))
(define (end-visit)
  (put "tabs-focused" #f) (put "bar-focused" '()) (put "sheet" #f) (put "note" #f)
  (if (kind? "input") (close-input) (begin (when (kind? "projects") (put "scroll" #f)) (normal!))))
(define (step delta)
  (define ids (map (lambda (row) (hash-ref row "id")) (visible)))
  (define index (or (position (lambda (id) (equal? id (get "selected"))) ids) 0))
  (define value (nth ids (min (max 0 (+ index delta)) (sat (length ids) 1))))
  (define changed (not (equal? value (get "selected")))) (put "selected" value) (put "scroll" #f) changed)
(define (launch-agent)
  (define value (selected))
  (if (not (and value (get "root"))) #f
    (let* ([id (hash-ref value "id")] [ancestors (reverse (cdr (reverse (model "Trail" (get "list") id))))]
        [subtasks (flatten (children value) 0 #f)]
        [prompt (string-append "Todo #" (number->string id) ": " (hash-ref value "text")
            (if (null? ancestors) "" (string-append "\n\nParent context (for reference):" (apply string-append (map (lambda (text) (string-append "\n- " text)) ancestors))))
            (if (null? subtasks) "" (string-append "\n\nOpen subtasks:" (apply string-append (map (lambda (row) (string-append "\n" (make-string (* 2 (hash-ref row "depth")) #\space) "- [ ] " (hash-ref row "text"))) subtasks))))
            "\n\nImplement the task, run relevant checks, and summarize the changes and anything still outstanding.")])
      (yinz-command (hash "LoadNewPlugin" (hash "url" "new-agent" "config" (hash "caller_cwd" (get "root") "initial_prompt" prompt "overlay" "true" "modal" "true") "load_in_background" #f "skip_plugin_cache" #f))) #t)))
(define (input-key key)
  (define bare (hash-ref key "bare_key")) (define mods (hash-ref key "key_modifiers"))
  (cond [(equal? bare "Esc") (close-input)] [(equal? bare "Tab") (ring-key key)]
    [(and (equal? bare "Enter") (null? mods)) (submit-input)]
    [(and (equal? bare "Enter") (or (equal? mods '("Alt")) (equal? mods '("Shift")))) (yinz-ui-input-op (mget "input") "text" "InsertText" "\n")]
    [(equal? bare "Enter") #t]
    [(and (hash? bare) (not (or (null? mods) (equal? mods '("Shift")) (equal? mods '("Ctrl"))))) #t]
    [else (when (equal? (yinz-ui-input-op (mget "input") "text" "Key" key) "cancel") (close-input))]) #t)
(define (content-key key)
  (define bare (hash-ref key "bare_key")) (define mods (hash-ref key "key_modifiers")) (define plain (null? mods))
  (define (letter c) (and plain (equal? bare (hash "Char" c))))
  (define up (and plain (or (equal? bare "Up") (letter "k"))))
  (define down (and plain (or (equal? bare "Down") (letter "j"))))
  (cond [(kind? "input") (input-key key)]
    [(or (kind? "delete") (kind? "complete"))
      (define question mode) (normal!)
      (unless (ring-key key) (when (letter "y")
          (write-plan (hash-ref question "plan") (done-note (if (equal? (hash-ref question "kind") "delete") "deleted" "completed") (text-of (hash-ref question "id")) (hash-ref question "count"))))) #t]
    [(kind? "move")
      (cond [(ring-key key) #t] [(equal? bare "Esc") (normal!)]
        [(or up down) (define targets (move-targets (mget "id")))
          (define index (or (position (lambda (id) (equal? id (mget "target"))) targets) 0))
          (mput "target" (nth targets (min (max 0 (+ index (if up -1 1))) (sat (length targets) 1)))) (put "scroll" #f)]
        [(and plain (equal? bare "Enter") (mget "target")) (commit-move (mget "target"))]
        [(letter "t") (commit-move #f)]) #t]
    [(kind? "projects")
      (unless (ring-key key)
        (when plain (cond [(equal? bare "Esc") (normal!) (put "scroll" #f)]
            [up (mput "selected" (sat (mget "selected") 1)) (put "scroll" #f)]
            [down (mput "selected" (min (+ 1 (mget "selected")) (sat (length (project-values)) 1))) (put "scroll" #f)]
            [(equal? bare "Enter") (define project (nth (project-values) (mget "selected")))
              (if project (watch-root (hash-ref project "root")) (begin (normal!) (put "scroll" #f)))]))) #t]
    [else
      (define noted (get "note")) (put "note" #f)
      (define changed
        (cond [(ring-key key) #t]
          [(equal? mods '("Ctrl")) (if (equal? bare (hash "Char" "r")) (begin (if (and (not (get "root")) (not (get "project-watch"))) (watch-projects) (watch-root (get "root"))) #t) #f)]
          [(and (equal? bare (hash "Char" "A")) (or plain (equal? mods '("Shift")))) (and (selected) (add-input (hash-ref (selected) "id")))]
          [(not plain) #f]
          [(equal? bare "Esc") (end-visit) (put "pane-shown" #f) (yinz-command "HideSelf") #f]
          [up (step -1)] [down (step 1)]
          [(letter "p") (set! mode (hash "kind" "projects" "selected" (or (position (lambda (project) (equal? (hash-ref project "root") (get "root"))) (project-values)) 0))) (put "scroll" #f) #t]
          [(letter "v") (define before (visible)) (put "show-done" (not (get "show-done"))) (put "selected" (nearest before (visible) (get "selected"))) (invalidate) #t]
          [(letter "a") (add-input #f)]
          [(or (letter "e") (equal? bare "Enter")) (define value (selected))
            (and value (begin-input (hash "kind" "edit" "id" (hash-ref value "id") "rev" (at value "rev" 0) "original" (hash-ref value "text") "theirs" #f) (hash-ref value "text")))]
          [(letter " ") (toggle)] [(letter "d") (delete)] [(letter "m") (begin-move)] [(letter "n") (launch-agent)] [else #f]))
      (and (or changed noted) #t)]))
(define (dispatch-key key)
  (if (get "tabs-focused")
    (let ([bare (hash-ref key "bare_key")] [mods (hash-ref key "key_modifiers")])
      (if (and (equal? bare "Tab") (equal? mods '("Shift")))
        (begin (put "tabs-focused" #f) (focus-tabs #f) (take-stop "history"))
        (when (and (null? mods) (member bare (list "Left" "Right" "Enter" "Tab" "Esc" (hash "Char" "r"))))
          (when (member bare '("Enter" "Tab" "Esc")) (put "tabs-focused" #f))
          (try (lambda () (yinz-command (hash "RunAction" (list (hash "Write" (hash "key_with_modifier" key "bytes" '() "is_kitty_keyboard_protocol" #f)) (hash)))))))) #t)
    (content-key key)))
(define (scroll-sheet delta)
  (define old (get "sheet")) (define next (min (max 0 (+ old delta)) (get "sheet-max"))) (put "sheet" next) (not (= old next)))
(define (on-key key)
  (define bare (hash-ref key "bare_key")) (define mods (hash-ref key "key_modifiers")) (define plain (null? mods))
  (define question (and (equal? bare (hash "Char" "?")) (or plain (equal? mods '("Shift")))))
  (cond [(get "tabs-focused") (dispatch-key key)]
    [(and (equal? bare (hash "Char" "o")) (equal? mods '("Ctrl")))
      (yinz-command (hash "MessageToPlugin" (hash "message_name" "agent_strip_reposition" "plugin_url" json-null "destination_plugin_id" json-null "plugin_config" (hash) "message_payload" json-null "message_args" (hash) "new_plugin_args" json-null "floating_pane_coordinates" json-null))) #f]
    [(and (not (get "sheet")) question (not (kind? "input"))) (put "sheet" 0) #t]
    [(get "sheet")
      (cond [(or question (and plain (or (equal? bare "Esc") (equal? bare (hash "Char" "q"))))) (put "sheet" #f) #t]
        [(or (equal? bare "Down") (and plain (equal? bare (hash "Char" "j")))) (scroll-sheet 1)]
        [(or (equal? bare "Up") (and plain (equal? bare (hash "Char" "k")))) (scroll-sheet -1)]
        [(equal? bare "PageDown") (scroll-sheet (max 1 (sat (get "sheet-rows") 1)))]
        [(equal? bare "PageUp") (scroll-sheet (- 0 (max 1 (sat (get "sheet-rows") 1))))]
        [(equal? bare "Tab") (dispatch-key key)] [else (put "sheet" #f) (dispatch-key key) #t])]
    [else (dispatch-key key)]))
(define (on-paste text)
  (if (or (get "tabs-focused") (not (kind? "input"))) #f
    (let* ([characters (reverse (string->list text))]
        [trimmed (let loop ([xs characters]) (if (and (not (null? xs)) (member (car xs) '(#\return #\newline))) (loop (cdr xs)) (list->string (reverse xs))))]
        [value (list->string (map (lambda (c) (if (equal? c #\tab) #\space c)) (string->list trimmed)))])
      (yinz-ui-input-op (mget "input") "text" "InsertText" value) (not (equal? value "")))))
(define (on-mouse mouse)
  (define name (car (hash-keys->list mouse))) (define value (hash-ref mouse name))
  (when (member name '("Hover" "LeftClick" "RightClick" "Hold" "Release")) (put "mouse" value))
  (define point (get "mouse"))
  (cond [(or (= (get "rows") 0) (= (get "cols") 0)
        (and point (or (< (car point) 0) (>= (car point) (get "rows")) (>= (cadr point) (get "cols")) (equal? (car point) (get "footer-row"))))) #f]
    [(get "sheet") (cond [(member name '("LeftClick" "RightClick")) (put "sheet" #f) #t]
        [(equal? name "ScrollUp") (scroll-sheet (- 0 (max 1 value)))] [(equal? name "ScrollDown") (scroll-sheet (max 1 value))] [else #f])]
    [else
      (when (and (equal? name "LeftClick") (get "tabs-focused")) (put "tabs-focused" #f) (focus-tabs #f))
      (cond [(and (equal? name "LeftClick") (>= (car value) 0))
          (define row (- (car value) (get "header-rows")))
          (if (or (< row 0) (>= row (get "list-rows"))) #f
            (begin (cond [(kind? "projects") (when (< row (length (get "rendered-projects"))) (mput "selected" (+ (get "window") row)))]
                [else (define id (nth (get "rendered-ids") row))
                  (when id (cond [(kind? "normal") (put "selected" id)] [(and (kind? "move") (member id (move-targets (mget "id")))) (mput "target" id)]))]) #t))]
        [(equal? name "ScrollUp") (put "scroll" (sat (or (get "scroll") (get "window")) (max 1 value))) #t]
        [(equal? name "ScrollDown") (put "scroll" (+ (or (get "scroll") (get "window")) (max 1 value))) #t] [else #f])]))
(define (load configuration)
  (yinz-command (hash "Subscribe" '("Key" "Mouse" "PastedText" "PaneUpdate" "PaneTabsUpdate" "Visible" "PluginDataChanged")))
  (define ids (yinz-command "GetPluginIds"))
  (put "plugin-id" (hash-ref ids "plugin_id")) (put "client-id" (hash-ref ids "client_id")) (put "initial-cwd" (hash-ref ids "initial_cwd"))
  (yinz-command (hash "SetSelectable" #t)) (yinz-command (hash "SetSelfMouseSelectionSupport" #t))
  (yinz-command (hash "RenamePluginPane" (list (get "plugin-id") "Todos"))) (watch-projects))
(define (update event)
  (define name (car (hash-keys->list event))) (define value (hash-ref event name))
  (define input (member name '("Key" "Mouse" "PastedText")))
  (when input (put "client-id" (hash-ref (yinz-command "GetPluginIds") "client_id")) (put "tabs-focused" (and (member (get "client-id") (get "bar-focused")) #t)))
  (define had-bar (get "tabs-focused"))
  (define changed
    (cond [(equal? name "Key") (on-key value)] [(equal? name "Mouse") (on-mouse value)] [(equal? name "PastedText") (on-paste value)]
      [(equal? name "PaneTabsUpdate")
        (define own (filter (lambda (tabs) (ormap (lambda (slot) (equal? (at slot "plugin_id" #f) (get "plugin-id"))) (hash-ref tabs "tabs"))) value))
        (put "pane-tabs" (and (not (null? own)) (hash-ref (car own) "id")))
        (put "bar-focused" (if (null? own) '() (hash-ref (car own) "bar_focused")))
        (put "tabs-focused" (and (member (get "client-id") (get "bar-focused")) #t)) #t]
      [(equal? name "PaneUpdate")
        (define panes (apply append (hash-values->list (hash-ref value "panes"))))
        (define own (filter (lambda (pane) (and (hash-ref pane "is_plugin") (= (hash-ref pane "id") (get "plugin-id")))) panes))
        (if (null? own) #f
          (let* ([pane (car own)] [visible (not (hash-ref pane "is_suppressed"))] [focus (and visible (hash-ref pane "is_focused"))] [old (get "keyboard")])
            (when (and (get "pane-shown") (not visible)) (end-visit))
            (put "pane-shown" visible) (put "keyboard" focus)
            (and visible (not (equal? old 'unknown)) (not (equal? old focus)))))]
      [(and (equal? name "Visible") (not value)) (end-visit) #t]
      [(equal? name "PluginDataChanged")
        (cond [(equal? (hash-ref value "watch_id") (get "project-watch")) (take-projects (hash-ref value "rows")) #t]
          [(equal? (hash-ref value "watch_id") (get "todo-watch")) (with-handler (lambda (failure) (put "status" (failure-text failure))) (take-rows (hash-ref value "rows"))) #t] [else #f])]
      [else #f]))
  (when input
    (put "bar-focused" (filter (lambda (id) (not (= id (get "client-id")))) (get "bar-focused")))
    (when (get "tabs-focused") (put "bar-focused" (append (get "bar-focused") (list (get "client-id")))) (unless had-bar (focus-tabs #t))))
  changed)
(define (pipe message)
  (put "client-id" (hash-ref (yinz-command "GetPluginIds") "client_id"))
  (if (equal? (hash-ref message "name") "agent_strip_take") (take-stop (at (hash-ref message "args") "stop" #f)) #f))
(define bar-hints '("←/→ choose" "Enter open" "Tab/Esc list"))
(define (bar-keys?) (and (not (equal? (get "keyboard") #f)) (get "tabs-focused")))
(define (cut text width) (panel "Cut" text width))
(define (fit-segments segments width)
  (define values (filter (lambda (text) (not (equal? text ""))) segments))
  (if (null? values) ""
    (let loop ([line (cut (car values) width)] [rest (cdr values)])
      (if (or (null? rest) (> (+ (yinz-text-width line) 3 (yinz-text-width (car rest))) width)) line
        (loop (string-append line " · " (car rest)) (cdr rest))))))
(define (wrap-segments segments width rows)
  (let loop ([rest (filter (lambda (text) (not (equal? text ""))) segments)] [lines '()])
    (if (null? rest) (reverse lines)
      (cond [(and (not (null? lines)) (<= (+ (yinz-text-width (car lines)) 3 (yinz-text-width (car rest))) width))
          (loop (cdr rest) (cons (string-append (car lines) " · " (car rest)) (cdr lines)))]
        [(= (length lines) rows) (reverse lines)] [else (loop (cdr rest) (cons (cut (car rest) width) lines))]))))
(define (excerpt text width)
  (define line (yinz-text-sanitize (trim (car (split-many text "\n")))))
  (if (<= (yinz-text-width line) width) line (string-append (cut line (sat width 1)) "…")))
(define (subtasks count open)
  (string-append (number->string count) (if open " open subtask" " subtask") (if (= count 1) "" "s")))
(define (note-text note room)
  (define lead (hash-ref note "lead")) (define text (at note "item" #f))
  (if (not text) lead
    (let* ([count (hash-ref note "subtasks")] [tail (if (= count 0) "" (string-append " +" (subtasks count #f)))]
        [width (min 32 (sat room (+ (yinz-text-width lead) (yinz-text-width tail) 3)))])
      (cond [(>= width 6) (string-append lead " \"" (excerpt text width) "\"" tail)]
        [(or (= count 0) (<= (+ (yinz-text-width lead) (yinz-text-width tail)) room)) (string-append lead tail)]
        [else (string-append lead " +" (number->string count))]))))
(define (style text levels) (hash "text" text "levels" levels))
(define (color text level indices)
  (define base (if (string? text) (hash "text" text) text))
  (define levels (at base "levels" '()))
  (hash-insert base "levels" (map (lambda (i) (append (or (nth levels i) '()) (if (= i level) indices '()))) (range-n 0 (max (+ level 1) (length levels))))))
(define (all-color text level) (color text level (range-n 0 (string-length (if (string? text) text (hash-ref text "text"))))))
(define (paint text row cols) (yinz-ui-text text (yinz-ui-coordinates 0 row cols json-null)))
(define (row-style text bright error)
  (if (not bright) (all-color text 4)
    (let ([dim (append (range-n 0 (car bright)) (range-n (cadr bright) (string-length text)))])
      (define styled (color text 4 dim)) (if error (color styled 6 (range-n (car bright) (cadr bright))) styled))))
(define (project-name)
  (define found (filter (lambda (project) (equal? (hash-ref project "root") (get "root"))) (project-values)))
  (if (null? found) "todos" (hash-ref (car found) "name")))
(define (header cols)
  (if (and (not (kind? "projects")) (not (get "root"))) #f
    (let* ([label (if (kind? "projects") "Pick a project" (string-append "Project: " (project-name)))]
        [line (fit-segments (list label (if (kind? "normal") "p switch" "")) cols)])
      (row-style line (list 0 (min (string-length label) (string-length line))) #f))))
(define (draft-keys)
  (define found (map (lambda (draft)
        (define action (hash-ref draft "action"))
        (cond [(equal? (hash-ref action "kind") "edit") (and (equal? (hash-ref action "id") (get "selected")) "e")]
          [(not (at action "parent" #f)) "a"] [(equal? (hash-ref action "parent") (get "selected")) "A"] [else #f])) (get "drafts")))
  (filter (lambda (key) (member key found)) '("e" "A" "a")))
(define (normal-hints)
  (if (not (get "root")) (if (null? (project-values)) '("Ctrl-r reload") '("p pick a project" "Ctrl-r reload"))
    (let* ([drafts (draft-keys)] [value (selected)] [counts (model "Counts" (get "list"))])
      (append (map (lambda (key) (string-append (if (equal? key "e") "Enter/e" key) " resume draft")) drafts)
        (if value (append (list (if (at value "done" #f) "Space reopen" "Space done")) (if (member "e" drafts) '() '("Enter/e edit"))) '())
        (if (member "a" drafts) '() '("a add"))
        (if value (append (if (member "A" drafts) '() '("A subtask")) '("d delete" "n agent" "m move")) '())
        '("p project") (cond [(get "show-done") '("v hide done")] [(> (cadr counts) (car counts)) '("v show done")] [else '()]) '("Ctrl-r reload")))))
(define (divider cols above below)
  (when (or (kind? "delete") (kind? "complete") (and (kind? "normal") (get "note"))
      (and (kind? "move") (< cols (+ 4 (yinz-text-width (panel "Readout" above below)) (yinz-text-width "Enter nest · t top · Esc cancel")))))
    (set! above 0) (set! below 0))
  (define room (sat cols (+ 4 (yinz-text-width (panel "Readout" above below)))))
  (define lead #f) (define error #f)
  (define hints
    (cond [(kind? "delete") (set! error #t) (set! lead (if (= (mget "count") 0) "y/n delete this todo?" (string-append "y/n delete with " (subtasks (mget "count") #f) "?"))) '()]
      [(kind? "complete") (set! error #t) (set! lead (string-append "y/n complete with " (subtasks (mget "count") #t) "?")) '()]
      [(kind? "move") (append (if (bar-keys?) bar-hints (append (if (mget "target") '("Enter nest") '()) '("t top" "Esc cancel"))) (list (string-append "moving \"" (excerpt (text-of (mget "id")) 24) "\"")))]
      [(kind? "projects") (cond [(bar-keys?) bar-hints] [(null? (project-values)) '("Esc back")] [else '("Enter open" "Esc cancel")])]
      [else (define keys (if (bar-keys?) bar-hints (normal-hints)))
        (cond [(get "note") (set! lead (note-text (get "note") room)) (set! error (hash-ref (get "note") "error")) (cons (hash-ref (get "note") "hint") keys)]
          [(not (equal? (get "status") "")) (set! lead (get "status")) (set! error #t) (cons "Ctrl-r retry" (filter (lambda (hint) (not (equal? hint "Ctrl-r reload"))) keys))]
          [else keys])]))
  (define label (panel (if (bar-keys?) "FitKeys" "FitHints") (if lead (cons lead hints) hints) room))
  (define line (panel "Divider" label above below cols))
  (define start (or (string-find line label) 0))
  (row-style line (and lead (list start (+ start (min (string-length lead) (string-length label))))) error))
(define (empty-text)
  (cond [(or (not (get "root")) (kind? "projects"))
      (cond [(and (null? (project-values)) (not (get "project-watch")) (not (equal? (get "status") ""))) '(("projects not loaded" #f) ("Ctrl-r to retry" #t))]
        [(null? (project-values)) '(("no projects yet" #f) ("start an agent in a git checkout," #t) ("or: yinz action project-remember NAME --root DIR" #t))]
        [else '(("no project picked" #f) ("p to pick one" #t))])]
    [(and (not (get "show-done")) (> (cadr (model "Counts" (get "list"))) 0))
      (define count (cadr (model "Counts" (get "list"))))
      (list (list (string-append (number->string count) " done todo" (if (= count 1) "" "s") " hidden") #f) '("v to show them" #t))]
    [else '(("no todos yet" #f) ("a to add one" #t))]))
(define (empty-rows cols)
  (apply append (map (lambda (row) (map (lambda (line) (if (cadr row) (all-color line 4) (hash "text" line))) (panel "WrapWords" (car row) cols))) (empty-text))))
(define (input-prompt action room)
  (define short (cond [(equal? (hash-ref action "kind") "edit") "edit: "] [(at action "parent" #f) "subtask: "] [else "new: "]))
  (define long (cond [(equal? (hash-ref action "kind") "edit") "edit: "]
      [(at action "parent" #f) (string-append "subtask of \"" (excerpt (text-of (hash-ref action "parent")) 20) "\": ")] [else "new top-level todo: "]))
  (if (<= (+ (yinz-text-width long) 24) room) long short))
(define (input-row cols)
  (define room (sat cols 4))
  (if (or (not (kind? "input")) (= room 0)) #f
    (let* ([input (mget "input")] [action (mget "action")] [text (yinz-ui-value input "text")]
        [shown (list->string (map (lambda (c) (cond [(equal? c #\newline) #\⏎] [(or (< (char->integer c) 32) (= (char->integer c) 127)) #\space] [else c])) (string->list text)))]
        [hints (cond [(bar-keys?) bar-hints] [(equal? text "") '("Esc cancel")]
            [(draft? action input) '("Enter save" "Esc keep draft" "Alt+Enter new line")] [else '("Enter save" "Esc cancel" "Alt+Enter new line")])]
        [prompt (input-prompt action room)]
        [window (panel "InputWindow" prompt shown (yinz-ui-input-op input "text" "CursorPosition") room)]
        [label (car window)] [caret (cadr window)] [start (caddr window)]
        [end (string-length label)] [from (min (string-length prompt) end)] [appended 0])
      (when (and (= start 0) (= (- end from) (string-length shown)))
        (let loop ([rest hints])
          (when (and (not (null? rest)) (<= (+ (yinz-text-width label) 3 (yinz-text-width (car rest))) room))
            (set! label (string-append label " · " (car rest))) (set! appended (+ appended 1)) (loop (cdr rest)))))
      (define line (panel "Divider" label 0 0 cols))
      (define at (or (string-find line label) 0))
      (hash "text" (row-style line (list (+ at from) (+ at end)) #f) "cursor" (+ (yinz-text-width (substring line 0 at)) caret) "hints" hints "appended" appended))))
(define (cursor-id)
  (cond [(kind? "move") (mget "target")]
    [(kind? "input") (at (mget "action") "id" (at (mget "action") "parent" #f))]
    [(or (kind? "delete") (kind? "complete")) (mget "id")] [else (get "selected")]))
(define (prefix-style styled prefix)
  (define size (string-length prefix))
  (color (hash-insert (hash-insert styled "text" (string-append prefix (hash-ref styled "text"))) "levels"
      (map (lambda (level) (map (lambda (index) (+ index size)) level)) (at styled "levels" '()))) 5 (range-n 0 size)))
(define (todo-lines cols)
  (define cached (get "cache"))
  (if (and cached (= (car cached) cols)) (cadr cached)
    (let ([lines (apply append (map (lambda (row)
                (define marker (string-append (make-string (* 2 (hash-ref row "depth")) #\space) "[" (if (at row "done" #f) "x" " ") "] "))
                (define width (max 1 (sat cols (+ 1 (yinz-text-width marker)))))
                (define md (yinz-ui-markdown (hash-ref row "text") (hash "compact" #t)))
                (define rendered (yinz-ui-lines md width))
                (map (lambda (index) (hash "id" (hash-ref row "id") "first" (= index 0) "done" (at row "done" #f)
                      "styled" (prefix-style (list-ref rendered index) (if (= index 0) marker (make-string (yinz-text-width marker) #\space))))) (range-n 0 (length rendered)))) (visible)))])
      (put "cache" (list cols lines)) lines)))
(define (clear-dimensions)
  (for-each (lambda (key) (put key 0)) '("window" "list-rows" "header-rows"))
  (put "rendered-ids" '()) (put "rendered-projects" '()) (put "cache" #f) (put "overwrite-shown" #f))
(define (render-todos rows cols)
  (put "rendered-ids" '()) (put "rendered-projects" '())
  (if (or (= rows 0) (= cols 0)) (begin (clear-dimensions) "")
    (let* ([segments (split-many (or (mget "refusal") "") " · ")]
        [wanted (length (wrap-segments segments cols 3))]
        [heading (and (>= rows (+ 3 wanted)) (header cols))] [top (if heading 1 0)]
        [refusal (wrap-segments segments cols (min 3 (sat (- rows top) 2)))]
        [input (input-row cols)]
        [keys (and (kind? "input") (not (mget "refusal")) (>= (- rows top) 3) input
            (< (hash-ref input "appended") (length (hash-ref input "hints"))) (fit-segments (hash-ref input "hints") cols))]
        [list-rows (- rows top 1 (length refusal) (if keys 1 0))] [output ""])
      (define (emit styled row) (set! output (string-append output (paint styled row cols))))
      (when heading (emit heading 0))
      (put "header-rows" top) (put "list-rows" list-rows)
      (when (ormap (lambda (row) (member "Enter overwrites" (split-many row " · "))) refusal) (put "overwrite-shown" #t))
      (cond [(kind? "projects")
          (define projects (project-values))
          (define start (min (or (get "scroll") (sat (mget "selected") (quotient list-rows 2))) (sat (length projects) list-rows)))
          (when (get "scroll") (put "scroll" start)) (put "window" start)
          (define shown (take-n (drop-n projects start) list-rows))
          (put "rendered-projects" (map (lambda (project) (hash-ref project "root")) shown))
          (indexed (lambda (line project)
              (define selected (= (+ start line) (mget "selected")))
              (emit (hash "text" (cut (string-append (if selected "> " "  ") (hash-ref project "name") "  " (hash-ref project "root")) cols) "selected" selected) (+ top line))) shown)
          (when (null? projects) (indexed (lambda (i styled) (emit styled (+ top i))) (take-n (empty-rows cols) list-rows)))
          (emit (divider cols start (sat (length projects) (+ start list-rows))) (- rows 1))]
        [else
          (define lines (todo-lines cols)) (define cursor (cursor-id))
          (define first (or (and cursor (position (lambda (line) (equal? (hash-ref line "id") cursor)) lines)) 0))
          (define end (if cursor (+ first (or (position (lambda (line) (not (equal? (hash-ref line "id") cursor))) (drop-n lines first)) (- (length lines) first))) 0))
          (define centered (sat first (quotient list-rows 2)))
          (define start (min (or (get "scroll") (if (not cursor) (get "window") (if (> end (+ centered list-rows)) (min first (sat end list-rows)) centered))) (sat (length lines) list-rows)))
          (when (get "scroll") (put "scroll" start)) (put "window" start)
          (define shown (take-n (drop-n lines start) list-rows))
          (put "rendered-ids" (map (lambda (line) (hash-ref line "id")) shown))
          (indexed (lambda (row line)
              (define selected (equal? cursor (hash-ref line "id")))
              (define prefix (cond [(and selected (hash-ref line "first")) ">"] [(and (kind? "move") (equal? (mget "id") (hash-ref line "id")) (hash-ref line "first")) "↕"] [else " "]))
              (define styled (prefix-style (hash-ref line "styled") prefix))
              (when (hash-ref line "done") (set! styled (all-color styled 4)))
              (emit (hash-insert styled "selected" selected) (+ top row))) shown)
          (when (null? lines) (indexed (lambda (i styled) (emit styled (+ top i))) (take-n (empty-rows cols) list-rows)))
          (indexed (lambda (i line) (emit (all-color line 6) (+ top list-rows i))) refusal)
          (when keys (emit (all-color keys 4) (- rows 2)))
          (emit (if input (hash-ref input "text") (divider cols start (sat (length lines) (+ start list-rows)))) (- rows 1))]) output)))
(define box-keys
  '(("Enter" "in the box: save; on an empty box, close it")
    ("Alt+Enter / Shift+Enter" "in the box: start a new line")
    ("Esc / Ctrl-c" "in the box: close it, keeping the text as a draft")
    ("← / → / Home / End / Ctrl-a" "in the box: move the caret")
    ("Ctrl+← / Ctrl+→ / Alt+← / Alt+→" "in the box: move by a word")
    ("Backspace / Delete / Ctrl-d" "in the box: delete a character")
    ("Ctrl-w / Alt+Backspace" "in the box: delete the word before the caret")
    ("Ctrl-u / Ctrl-k" "in the box: delete to the start, or to the end")
    ("Ctrl-z / Ctrl-y" "in the box: undo, redo")
    ("Tab / Shift+Tab" "in the box: walk the panel, leaving the box open")))
(define (key-sheet)
  (if (bar-keys?)
    '(("←/→" "highlight a tab") ("Enter" "activate the highlighted tab") ("Tab/Esc" "return to the active tab's list")
      ("Shift+Tab" "to the active view's last region") ("other keys/paste" "ignored while the bar has focus") ("click a tab/arrow" "activate a tab/browse hidden tabs"))
    (append
      (cond [(kind? "delete") (list (list "y" (if (= (mget "count") 0) "delete the todo" "delete the todo and its subtasks")) '("any other key" "keep it"))]
        [(kind? "complete") '(("y" "complete the todo and its open subtasks") ("any other key" "leave them open"))]
        [(kind? "move") '(("↑ / ↓ / j / k" "choose the new parent, marked >") ("Enter" "move the todo under it") ("t" "move the todo to the top level")
            ("Esc" "cancel the move") ("click" "choose a parent") ("Tab / Shift+Tab" "walk the panel, keeping the pick"))]
        [(kind? "projects") '(("↑ / ↓ / j / k" "choose a project") ("Enter" "open its todos") ("Esc" "cancel")
            ("click / wheel" "choose, scroll") ("Tab / Shift+Tab" "walk the panel, keeping the pick"))]
        [(not (get "root")) '(("p" "pick a project") ("Ctrl-r" "reload") ("Tab" "go to the panel tab bar") ("Shift+Tab" "go to the panel's tabs") ("Esc" "hide the panel"))]
        [else (append
            (map (lambda (key) (list (if (equal? key "e") "Enter / e" key) "bring back the draft Esc kept")) (draft-keys))
            '(("Space" "complete the selected todo, or reopen it") ("Enter / e" "edit the selected todo") ("a" "add a todo")
              ("A" "add a subtask under the selected todo") ("d" "delete the selected todo, after y/n")
              ("n" "start an agent on the selected todo") ("m" "move the selected todo under another") ("p" "switch project"))
            (list (list "v" (if (get "show-done") "hide done todos" "show done todos")))
            '(("↑ / ↓ / j / k" "move the selection") ("Ctrl-r" "reload the list") ("Tab" "go to the panel tab bar")
              ("Shift+Tab" "go to the panel's tabs") ("Esc" "hide the panel") ("click / wheel" "select a todo, scroll"))
            (if (and (kind? "input") (at (mget "action") "theirs" #f))
              (cons '("Enter" "in the box: save, over the change made elsewhere") (cdr box-keys)) box-keys))])
      '(("Ctrl e o" "move the panel (Ctrl-o when locked)") ("?" "shut this sheet")))))
(define (render-sheet rows cols)
  (define keys (key-sheet))
  (define width (min (quotient (sat cols 1) 2) (foldl max 0 (map (lambda (row) (yinz-text-width (car row))) keys))))
  (define lines (apply append (map (lambda (row) (panel "KeySheetLines" (car row) (cadr row) width cols)) keys)))
  (put "sheet-rows" rows) (put "sheet-max" (sat (length lines) rows)) (put "sheet" (min (get "sheet") (get "sheet-max")))
  (apply string-append (map (lambda (index)
        (define row (list-ref lines (+ (get "sheet") index)))
        (paint (color (car row) 2 (range-n 0 (cadr row))) index cols)) (range-n 0 (min rows (- (length lines) (get "sheet")))))))
(define (render rows cols)
  (define client (hash-ref (yinz-command "GetPluginIds") "client_id"))
  (put "tabs-focused" (and (member client (get "bar-focused")) #t))
  (put "rows" rows) (put "cols" cols) (put "footer-row" #f)
  (define footer (and (get "sheet") (>= rows 2) (> cols 0) (panel "FitKeys" (if (bar-keys?) bar-hints '("Esc/? close" "j/k scroll")) cols)))
  (define body (- rows (if footer 1 0)))
  (if (or (= rows 0) (= cols 0)) (begin (clear-dimensions) "")
    (begin
      (define input (input-row cols))
      (yinz-command (hash "ShowCursor" (if (or (equal? (get "keyboard") #f) (get "sheet") (not input) (= body 0)) json-null (list (hash-ref input "cursor") (sat body 1)))))
      (when footer (put "footer-row" (- rows 1)))
      (string-append (if footer (paint (all-color footer 4) (- rows 1) cols) "")
        (if (get "sheet") (render-sheet body cols) (render-todos body cols))))))
