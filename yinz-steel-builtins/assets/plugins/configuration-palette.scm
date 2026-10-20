;; Draft nodes are mutable vectors so open editors share their in-memory tree.
;; Only explicit destination submission writes through the guarded host API.
(define (get object key fallback)
  (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))
(define (present? value) (and value (not (equal? value 'json-null))))
(define (clean value)
  (string-replace (string-replace (yinz-text-sanitize value) "\n" " ") "\t" " "))
(define (find predicate items)
  (cond ((null? items) #f) ((predicate (car items)) (car items)) (else (find predicate (cdr items)))))
(define (every predicate items) (or (null? items) (and (predicate (car items)) (every predicate (cdr items)))))
(define (flat-map procedure items) (apply append (map procedure items)))
(define (indexed-map procedure items)
  (let loop ((items items) (index 0))
    (if (null? items) '() (cons (procedure (car items) index) (loop (cdr items) (+ index 1))))))
(define (unique items)
  (foldl (lambda (item result) (if (member item result) result (append result (list item)))) '() items))
(define (without-index items index)
  (append (take items index) (list-tail items (+ index 1))))
(define (insert-at items index item)
  (append (take items index) (list item) (list-tail items index)))
(define (index-of object items)
  (let loop ((items items) (index 0))
    (cond ((null? items) #f) ((eq? object (car items)) index) (else (loop (cdr items) (+ index 1))))))
(define (string-value value)
  (cond ((string? value) value) ((boolean? value) (if value "true" "false")) (else (number->string value))))
(define (action id label run) (hash "id" id "label" (yinz-markdown-escape label) "search" label "run" run))
(define (defer thunk) (lambda (values context) (thunk)))
(define (read-config) (yinz-command (hash "InspectConfiguration" 'json-null)))

(define (node name arguments children) (vector name arguments (hash) children))
(define (node-name value) (vector-ref value 0))
(define (node-arguments value) (vector-ref value 1))
(define (node-properties value) (vector-ref value 2))
(define (node-children value) (vector-ref value 3))
(define (read-node value)
  (vector (hash-ref value "name") (hash-ref value "arguments") (hash-ref value "properties")
    (map read-node (hash-ref value "children"))))
(define (write-node value)
  (if value (hash "name" (node-name value) "arguments" (node-arguments value)
      "properties" (node-properties value) "children" (map write-node (node-children value))) 'json-null))
(define (copy-node value) (and value (read-node (write-node value))))
(define (replace-node target replacement)
  (for-each (lambda (index) (vector-set! target index (vector-ref replacement index))) '(0 1 2 3)))
(define (node-key value)
  (if (member (node-name value) '("bind" "unbind"))
    (string-append (node-name value) ":" (yinz-json-string (node-arguments value))) (node-name value)))
(define (find-node nodes path)
  (define found (find (lambda (value) (equal? (node-key value) (car path))) nodes))
  (if (and found (pair? (cdr path))) (find-node (node-children found) (cdr path)) found))
(define (effective snapshot client)
  (map read-node (get (hash-ref snapshot "clients") (number->string client) (hash-ref snapshot "saved"))))
(define (saved-node snapshot path) (find-node (map read-node (hash-ref snapshot "saved")) path))

(define (list-view title collect)
  (hash "type" "actions" "title" (yinz-markdown-escape title)
    "source" (lambda (query context) (search-actions (collect) query))))
(define (section-help root)
  (get (hash
      "keybinds" "Assign keyboard shortcuts to actions in each input mode."
      "env" "Set environment variables for panes. Changes take effect when the next session starts."
      "plugins" "Define plugin aliases and their default configuration."
      "load_plugins" "Choose plugins to load in the background when a session starts."
      "overlay_tab_plugins" "Choose and order the plugin tabs in the shared overlay."
      "hooks" "Run commands when session events occur, such as a pane opening or closing."
      "context_menu" "Choose and order the actions shown in the pane context menu."
      "ui" "Customize pane frame corners and whether frames show the session name.") root ""))
(define (section-view path collect)
  (hash-insert (list-view (clean (string-join path " / ")) collect)
    "description" (section-help (car path))))
(define (search-actions actions query)
  (define words (split-whitespace (string-downcase query)))
  (define found
    (filter (lambda (item)
        (define text (string-downcase (string-append (get item "search" (hash-ref item "label")) " " (string-join (get item "aliases" '()) " "))))
        (every (lambda (word) (string-contains? text word)) words)) actions))
  (take found (min 100 (length found))))
(define (confirm title run)
  (hash "type" "form" "title" (yinz-markdown-escape title) "fields" '() "confirmation" #t "submitLabel" "Confirm" "onSubmit" (defer run)))

(define (scalar-field id label value kind choices help)
  (define text (string-value value))
  (cond
    ((boolean? value) (hash "type" "choice" "id" id "label" (yinz-markdown-escape label) "search" label "options" '("Enabled" "Disabled")
        "initialValue" (if value "Enabled" "Disabled")))
    ((pair? choices) (hash "type" "choice" "id" id "label" (yinz-markdown-escape label) "search" label
        "options" (if (or (equal? text "") (member text choices)) choices (cons text choices))
        "initialValue" (if (equal? text "") (car choices) text)))
    (else
      (define type (cond ((or (equal? kind "number") (number? value)) "number")
          ((member kind '("file" "directory" "key")) kind) (else "multiline")))
      (hash "type" type "id" id "label" (yinz-markdown-escape label) "search" label "initialValue" text
        "help" (if (not (equal? help "")) help
          (cond ((equal? type "key") "Type a chord, or press F2 then the desired key. Tab moves to the next field.")
            ((equal? type "multiline") "Shift+Enter inserts a newline. Whitespace is preserved.") (else "")))))))
(define (parse-scalar text old)
  (cond
    ((boolean? old) (equal? text "Enabled"))
    ((number? old)
      (define digits (if (starts-with? text "-") (cdr (string->list text)) (string->list text)))
      (unless (and (pair? digits) (every (lambda (c) (and (>= (char->integer c) 48) (<= (char->integer c) 57))) digits))
        (error "Enter a whole number in range"))
      (define value (string->number text))
      (unless (and value (<= (abs value) 9007199254740991)) (error "Enter a whole number in range"))
      value)
    (else text)))

(define (diff before after path)
  (cond
    ((equal? (write-node before) (write-node after)) '())
    ((or (not before) (not after)
        (not (equal? (node-arguments before) (node-arguments after)))
        (not (equal? (node-properties before) (node-properties after)))
        (member (car path) '("context_menu" "overlay_tab_plugins")) (equal? (node-name before) "bind")
        (not (= (length (unique (map node-key (node-children before)))) (length (node-children before))))
        (not (= (length (unique (map node-key (node-children after)))) (length (node-children after)))))
      (list (hash "path" path "node" (write-node after))))
    (else
      (flat-map (lambda (name)
          (diff (find (lambda (child) (equal? (node-key child) name)) (node-children before))
            (find (lambda (child) (equal? (node-key child) name)) (node-children after))
            (append path (list name))))
        (unique (map node-key (append (node-children before) (node-children after))))))))

(define (new-draft snapshot client root)
  (define original (find-node (effective snapshot client) (list root)))
  (vector snapshot client root original (if original (copy-node original) (node root '() '()))))
(define (draft-snapshot draft) (vector-ref draft 0))
(define (draft-client draft) (vector-ref draft 1))
(define (draft-root draft) (vector-ref draft 2))
(define (draft-original draft) (vector-ref draft 3))
(define (draft-value draft) (vector-ref draft 4))

(define (commit-draft draft changes . fields)
  (define snapshot (draft-snapshot draft))
  (define root (draft-root draft))
  (define setting (find (lambda (setting) (equal? (hash-ref setting "name") root)) (hash-ref snapshot "catalog")))
  (define timing (get setting "timing" (get (hash-ref snapshot "section_timing") root "Applies to this session")))
  (define startup? (equal? timing "On next session start"))
  (hash "type" "form" "title" (string-replace (clean root) "_" " ")
    "description" (get setting "help" (section-help root))
    "fields" (append (if (null? fields) '() (car fields)) (list (hash "type" "choice" "id" "destination" "label" "Destination"
          "options" (if startup? '("Save permanently") '("Apply to session" "Save permanently"))
          "initialValue" (if startup? "Save permanently" "Apply to session"))))
    "submitLabel" "Apply change"
    "actions" (list (action "reload" "Reload configuration (keep draft)"
        (lambda (values context)
          (define pending (if (procedure? changes) (changes values) changes))
          (define fresh (read-config))
          (confirm "Configuration reloaded. Reapply this draft to the current configuration?"
            (lambda () (commit-draft (new-draft fresh (draft-client draft) root) pending))))))
    "onSubmit" (lambda (values context)
      (define pending (if (procedure? changes) (changes values) changes))
      (define patches (hash-ref pending "patches"))
      (define permanent? (equal? (get values "destination" #f) "Save permanently"))
      (define result (yinz-command (hash "EditConfiguration"
            (hash "revision" (hash-ref snapshot "revision") "destination" (if permanent? "permanent" "session")
              "patches" (if permanent? (get pending "permanentPatches" patches) patches) "reset" (get pending "reset" '())))))
      (hash "type" "complete" "notification" (hash-ref result "notice") "view" 'json-null))))
(define (finish-draft draft)
  (define patches (diff (draft-original draft) (draft-value draft) (list (draft-root draft))))
  (commit-draft draft (hash "patches" patches)))
(define (prefix? a b)
  (or (null? a) (and (pair? b) (equal? (car a) (car b)) (prefix? (cdr a) (cdr b)))))
(define (state-help draft path)
  (define snapshot (draft-snapshot draft))
  (define saved (saved-node snapshot path))
  (define clients (hash-ref snapshot "clients"))
  (define lines (map (lambda (id)
        (define value (find-node (map read-node (hash-ref clients id)) path))
        (string-append "Client " id ": " (yinz-json-string (if value (node-arguments value) "unset"))))
      (sort (hash-keys->list clients) string<?)))
  (define override? (find (lambda (patch) (or (prefix? path (hash-ref patch "path")) (prefix? (hash-ref patch "path") path)))
      (hash-ref snapshot "overrides")))
  (yinz-markdown-escape (string-append "Saved: " (yinz-json-string (if saved (node-arguments saved) "unset")) "\n"
      (string-join lines "\n") "\nSession override: " (if override? "Yes" "No"))))

(define (setting-form draft setting)
  (define kind (hash-ref setting "kind"))
  (define args (node-arguments (draft-value draft)))
  (define value (if (pair? args) (car args) (cond ((equal? kind "boolean") #f)
        ((equal? kind "number") (get setting "minimum" 0)) (else ""))))
  (when (equal? value 'json-null) (set! value 0))
  (when (and (equal? kind "number") (string? value)) (set! value (string->number (trim-end-matches value "%"))))
  (define choices (if (equal? kind "boolean") '() (hash-ref setting "choices")))
  (define help (state-help draft (list (hash-ref setting "name"))))
  (define field (scalar-field "value" (hash-ref setting "label") value kind choices ""))
  (when (equal? (hash-ref field "type") "number")
    (for-each (lambda (key) (when (present? (get setting key 'json-null)) (set! field (hash-insert field key (hash-ref setting key)))))
      '("minimum" "maximum")))
  (define form (commit-draft draft
      (lambda (values)
        (vector-set! (draft-value draft) 1 (list (parse-scalar (get values "value" "") value)))
        (define path (list (draft-root draft)))
        (define effective (diff (draft-original draft) (draft-value draft) path))
        (define saved (diff (saved-node (draft-snapshot draft) path) (draft-value draft) path))
        (hash "patches" effective "permanentPatches" (if (pair? saved) saved effective)))
      (list field)))
  (hash-insert (hash-insert form "title" (hash-ref setting "label"))
    "actions" (append (hash-ref form "actions")
      (list (action "state" "Current values and effect timing…"
          (defer (lambda () (hash "type" "text" "title" (hash-ref setting "label")
                "text" (string-append (hash-ref setting "help") "\n" (hash-ref setting "timing") "\n" help)))))
        (action "unset" "Remove configured value…"
          (defer (lambda () (confirm "Remove this configured value? The setting's automatic behavior will apply."
                (lambda () (define patches (list (hash "path" (list (hash-ref setting "name")) "node" 'json-null)))
                  (commit-draft draft (hash "patches" patches)))))))
        (action "reset" "Reset override…" (defer (lambda () (commit-draft draft (hash "patches" '() "reset" (list (list (hash-ref setting "name"))))))))))))

(define (templates draft path parent)
  (define snapshot (draft-snapshot draft))
  (define children (get (hash-ref snapshot "action_children") (node-name parent) #f))
  (define patterns (hash-ref snapshot "templates"))
  (define match
    (find (lambda (pattern)
        (define parts (split-many pattern "/"))
        (and (= (length parts) (length path))
          (every (lambda (pair) (or (equal? (car pair) "*") (equal? (car pair) (cadr pair))))
            (indexed-map (lambda (part index) (list part (list-ref path index))) parts))))
      (hash-keys->list patterns)))
  (cond (children (map read-node children)) (match (map read-node (hash-ref patterns match)))
    (else (list (node "entry" '("") '())))))

(define (child-list draft parent path)
  (section-view path
    (lambda ()
      (append
        (list
          (action "save" "Review and apply draft…" (defer (lambda () (finish-draft draft))))
          (action "add" "Add entry…"
            (defer (lambda ()
                (list-view "Choose entry"
                  (lambda ()
                    (indexed-map
                      (lambda (template index)
                        (action (string-append "template:" (number->string index)) (node-name template)
                          (defer (lambda ()
                              (define child (copy-node template))
                              (node-form draft child (append path (list (node-key child)))
                                (lambda ()
                                  (define conflicts (filter (lambda (other) (and (not (eq? other child))
                                          (equal? (node-key other) (node-key child))))
                                      (node-children parent)))
                                  (define ordered? (or (member (car path) '("context_menu" "overlay_tab_plugins")) (equal? (node-name parent) "bind")))
                                  (define (append-child)
                                    (unless ordered? (vector-set! parent 3 (filter (lambda (other) (not (equal? (node-key other) (node-key child))))
                                          (node-children parent))))
                                    (vector-set! parent 3 (append (node-children parent) (list child)))
                                    (child-list draft parent path))
                                  (if (and (pair? conflicts) (not ordered?))
                                    (confirm (string-append "Replace " (clean (node-key child)) "? Existing actions: "
                                        (string-join (flat-map (lambda (item) (map node-name (node-children item))) conflicts) ", ")) append-child)
                                    (append-child))))))))
                      (templates draft path parent)))))))
          (action "reset" "Reset override…" (defer (lambda () (commit-draft draft (hash "patches" '() "reset" (list path)))))))
        (indexed-map (lambda (child index)
            (action (string-append "entry:" (number->string index))
              (clean (string-append (number->string (+ index 1)) ". " (node-name child)
                  (if (equal? (node-name child) "bind")
                    (string-append " " (string-join (map string-value (node-arguments child)) " ")) "")))
              (defer (lambda () (entry draft parent index path)))))
          (node-children parent))))))

(define (entry draft parent index path)
  (unless (and index (< index (length (node-children parent)))) (error "Entry was removed from the draft"))
  (define child (list-ref (node-children parent) index))
  (define child-path (append path (list (node-key child))))
  (section-view child-path
    (lambda ()
      (append
        (list
          (action "state" "Current values…" (defer (lambda () (hash "type" "text" "title" (yinz-markdown-escape (clean (string-join child-path " / ")))
                  "text" (state-help draft child-path)))))
          (action "edit" "Edit fields…" (defer (lambda () (node-form draft child child-path
                  (lambda () (entry draft parent (index-of child (node-children parent)) path))))))
          (action "children" "Edit nested entries / actions…" (defer (lambda () (child-list draft child child-path))))
          (action "remove" "Remove entry…"
            (defer (lambda () (confirm (string-append "Remove " (clean (node-name child)) " from this draft?")
                  (lambda () (vector-set! parent 3 (without-index (node-children parent) index))
                    (child-list draft parent path)))))))
        (if (equal? (car path) "hooks")
          (list (action "disable" "Disable hook" (defer (lambda () (vector-set! child 1 '("")) (vector-set! child 3 '())
                  (child-list draft parent path))))) '())
        (list
          (action "up" "Move earlier"
            (defer (lambda () (when (> index 0)
                  (vector-set! parent 3 (insert-at (without-index (node-children parent) index) (- index 1) child)))
                (child-list draft parent path))))
          (action "down" "Move later"
            (defer (lambda () (when (< (+ index 1) (length (node-children parent)))
                  (vector-set! parent 3 (insert-at (without-index (node-children parent) index) (+ index 1) child)))
                (child-list draft parent path))))
          (action "save" "Review and apply draft…" (defer (lambda () (finish-draft draft)))))))))

(define (new-value title done)
  (hash "type" "form" "title" (yinz-markdown-escape title)
    "fields" (list (hash "type" "text" "id" "name" "label" "Name" "initialValue" "value")
      (hash "type" "choice" "id" "type" "label" "Type" "options" '("Text" "Number" "Boolean"))
      (hash "type" "multiline" "id" "value" "label" "Value"
        "help" "Shift+Enter inserts a newline. Boolean values are true or false."))
    "onSubmit" (lambda (values context)
      (define kind (get values "type" "Text"))
      (define value (get values "value" ""))
      (when (and (equal? kind "Boolean") (not (member value '("true" "false")))) (error "Enter true or false"))
      (done (get values "name" "value")
        (cond ((equal? kind "Boolean") (equal? value "true"))
          ((equal? kind "Number") (parse-scalar value 0)) (else value))))))

(define (node-form draft target path done)
  (define local (copy-node target))
  (define keys (hash-keys->list (node-properties local)))
  (define fields
    (append (list (hash "type" "text" "id" "name" "label" "Name" "initialValue" (node-name local)))
      (indexed-map
        (lambda (value index)
          (scalar-field (string-append "arg:" (number->string index))
            (if (equal? (node-name local) "bind") "Key chord" (string-append "Value " (number->string (+ index 1)))) value
            (cond ((equal? (node-name local) "bind") "key") ((equal? (node-name local) "cwd") "directory") (else #f)) '() ""))
        (node-arguments local))
      (map (lambda (key) (scalar-field (string-append "prop:" key) key (hash-ref (node-properties local) key) #f '() "")) keys)))
  (define (assign values)
    (when (hash-contains? values "name")
      (when (equal? (trim (hash-ref values "name")) "") (error "Name is required"))
      (vector-set! local 0 (hash-ref values "name")))
    (vector-set! local 1
      (indexed-map (lambda (value index)
          (define key (string-append "arg:" (number->string index)))
          (if (hash-contains? values key) (parse-scalar (hash-ref values key) value) value))
        (node-arguments local)))
    (for-each (lambda (key)
        (define field (string-append "prop:" key))
        (when (hash-contains? values field)
          (vector-set! local 2 (hash-insert (node-properties local) key
              (parse-scalar (hash-ref values field) (hash-ref (node-properties local) key)))))) keys))
  (define (local-done) (replace-node target local) (done))
  (define (reopen-local) (node-form draft local path local-done))
  (if (> (length fields) 16)
    (section-view path
      (lambda () (map (lambda (field)
            (action (hash-ref field "id") (get field "search" (hash-ref field "label"))
              (defer (lambda () (hash "type" "form" "title" (hash-ref field "label") "description" (section-help (car path)) "fields" (list field)
                    "onSubmit" (lambda (values context) (assign values) (local-done))))))) fields)))
    (hash "type" "form" "title" (yinz-markdown-escape (clean (string-join path " / "))) "description" (section-help (car path)) "fields" fields "submitLabel" "Keep in draft"
      "onSubmit" (lambda (values context)
        (assign values)
        (define parent (and (equal? (car path) "keybinds") (= (length path) 3)
            (find (lambda (mode) (equal? (node-key mode) (cadr path))) (node-children (draft-value draft)))))
        (define conflicts
          (if (and parent (equal? (node-name local) "bind"))
            (filter (lambda (binding)
                (and (not (eq? binding target)) (equal? (node-name binding) "bind")
                  (find (lambda (chord) (member chord (node-arguments local))) (node-arguments binding))))
              (node-children parent)) '()))
        (define (apply-change)
          (replace-node target local)
          (when (and parent (pair? conflicts))
            (for-each (lambda (binding)
                (vector-set! binding 1 (filter (lambda (chord) (not (member chord (node-arguments local)))) (node-arguments binding)))) conflicts)
            (vector-set! parent 3 (filter (lambda (binding) (or (not (equal? (node-name binding) "bind")) (pair? (node-arguments binding))))
                (node-children parent))))
          (done))
        (if (equal? (node-name local) "bind")
          (confirm (string-append "Replace binding " (clean (string-join (map string-value (node-arguments local)) " ")) "? Existing actions: "
              (string-join (flat-map (lambda (binding) (map node-name (node-children binding))) (cons target conflicts)) ", ")) apply-change)
          (apply-change)))
      "actions"
      (list
        (action "add-argument" "Add argument…"
          (lambda (values context) (assign values)
            (new-value "Argument" (lambda (name value) (vector-set! local 1 (append (node-arguments local) (list value))) (reopen-local)))))
        (action "add-property" "Add property…"
          (lambda (values context)
            (assign values)
            (define properties (get (hash-ref (draft-snapshot draft) "action_properties") (node-name local) #f))
            (if properties
              (list-view "Action properties"
                (lambda () (map (lambda (name)
                      (define initial (hash-ref properties name))
                      (action name name
                        (defer (lambda ()
                            (hash "type" "form" "title" (yinz-markdown-escape (string-append (node-name local) ": " name))
                              "fields" (list (scalar-field "value" name (get (node-properties local) name initial) #f '() ""))
                              "onSubmit" (lambda (values context)
                                (vector-set! local 2 (hash-insert (node-properties local) name (parse-scalar (get values "value" "") initial)))
                                (reopen-local)))))))
                    (hash-keys->list properties))))
              (new-value "Property" (lambda (name value) (vector-set! local 2 (hash-insert (node-properties local) name value)) (reopen-local))))))
        (action "remove-value" "Remove argument or property…"
          (lambda (values context)
            (assign values)
            (list-view "Remove value"
              (lambda ()
                (append
                  (indexed-map (lambda (value index)
                      (action (string-append "arg:" (number->string index)) (string-append "Argument " (number->string (+ index 1)))
                        (defer (lambda () (vector-set! local 1 (without-index (node-arguments local) index)) (reopen-local)))))
                    (node-arguments local))
                  (map (lambda (key) (action (string-append "prop:" key) key
                        (defer (lambda () (vector-set! local 2 (hash-remove (node-properties local) key)) (reopen-local)))))
                    (hash-keys->list (node-properties local))))))))))))

(define (discovery-draft snapshot client root)
  (define fresh (read-config))
  (unless (equal? (hash-ref fresh "revision") (hash-ref snapshot "revision"))
    (error "Configuration changed. Search again before selecting an entry."))
  (new-draft fresh client root))

(define (root-actions snapshot client)
  (define roots (effective snapshot client))
  (define keybinds (find-node roots '("keybinds")))
  (append
    (map (lambda (setting)
        (define name (hash-ref setting "name"))
        (hash "id" name "label" (string-append (hash-ref setting "label") "…")
          "aliases" (append (list name "configuration settings") (if (starts-with? name "copy_") '("clipboard") '()))
          "run" (defer (lambda () (setting-form (new-draft (read-config) client name) setting)))))
      (filter (lambda (setting) (not (member (hash-ref setting "name")
              '("theme" "theme_dark" "theme_light" "theme_dir" "explicit_theme_hue" "agent_theme_mapping"))))
        (hash-ref snapshot "catalog")))
    (flat-map
      (lambda (root)
        (define parent (find-node roots (list root)))
        (cons
          (hash "id" root "label" (string-append (string-replace root "_" " ") "…")
            "aliases" (append (list root "configuration settings") (if (equal? root "keybinds") '("keybindings") '()))
            "run" (defer (lambda () (define draft (new-draft (read-config) client root))
                (child-list draft (draft-value draft) (list root)))))
          (indexed-map
            (lambda (child index)
              (define label (string-append root ": " (clean (node-name child)) "…"))
              (hash "id" (string-append root ":" (number->string index) ":" (node-name child))
                "label" (yinz-markdown-escape label) "search" label
                "aliases" (list (string-append root " " (clean (node-name child))))
                "run" (defer (lambda () (define draft (discovery-draft snapshot client root))
                    (entry draft (draft-value draft) index (list root))))))
            (if parent (node-children parent) '()))))
      (filter (lambda (section) (not (equal? section "themes"))) (hash-ref snapshot "sections")))
    (apply append
      (indexed-map
        (lambda (mode mode-index)
          (filter (lambda (item) item)
            (indexed-map
              (lambda (binding index)
                (and (equal? (node-name binding) "bind")
                  (hash "id" (string-append "keybinds:" (number->string mode-index) ":" (node-name mode) ":"
                      (number->string index) ":" (node-key binding))
                    "label" (yinz-markdown-escape (string-append "keybinding " (clean (node-name mode)) ": "
                        (clean (string-join (map string-value (node-arguments binding)) " ")) "…"))
                    "search" (string-append "keybinding " (clean (node-name mode)) ": "
                      (clean (string-join (map string-value (node-arguments binding)) " ")) "…")
                    "aliases" (cons "keybindings keybinds" (map node-name (node-children binding)))
                    "run" (defer (lambda ()
                        (define draft (discovery-draft snapshot client "keybinds"))
                        (define modes (node-children (draft-value draft)))
                        (if (< mode-index (length modes))
                          (entry draft (list-ref modes mode-index) index (list "keybinds" (node-name mode)))
                          (child-list draft (draft-value draft) '("keybinds"))))))))
              (node-children mode))))
        (if keybinds (node-children keybinds) '())))))

(define (load configuration)
  (yinz-palette-register "configuration" "Configuration"
    (list (hash "id" "configuration" "label" "Configuration…" "aliases" '("settings")
        "run" (lambda (values context)
          (list-view "Configuration" (lambda () (root-actions (read-config) (hash-ref context "clientId")))))))))
