;; Configured menu IDs retain their actions; invented items return their context here.
(define opener #f)
(define pending #f)

(define (get object key fallback)
  (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))

(define (present? value) (and value (not (equal? value 'json-null))))

(define (menu-whitespace? character)
  (define code (char->integer character))
  (or (and (>= code 9) (<= code 13)) (and (>= code 8192) (<= code 8202))
    (member code '(32 160 5760 8232 8233 8239 8287 12288 65279))))

(define (shorten text)
  (define normalized
    (let loop ((chars (string->list text)) (result '()) (space? #f))
      (cond
        ((null? chars) (reverse result))
        ((menu-whitespace? (car chars)) (loop (cdr chars) result (not (null? result))))
        (else (loop (cdr chars)
            (cons (car chars) (if space? (cons #\space result) result)) #f)))))
  (let loop ((chars normalized) (remaining 24) (result '()))
    (cond
      ((null? chars) (list->string (reverse result)))
      ((zero? remaining) (string-append (list->string (reverse result)) "…"))
      (else (loop (cdr chars) (- remaining 1) (cons (car chars) result))))))

(define (run command context)
  (yinz-command (hash "RunCommand" (list command (hash) "" context 64 'json-null))))

(define (open-link url)
  (if opener (run (list opener url) (hash)) (set! pending url)))

(define (agent-pane? pane)
  (and (hash-contains? pane "Terminal")
    (present? (get (yinz-command (hash "GetPaneInfo" pane)) "agent" #f))))

(define (separator? item) (equal? (get item "separator" #f) #t))

(define (tidy items)
  (let loop ((rest items) (result '()))
    (cond
      ((null? rest) (reverse (if (and (pair? result) (separator? (car result)))
            (cdr result) result)))
      ((and (separator? (car rest))
          (or (null? result) (separator? (car result))))
        (loop (cdr rest) result))
      (else (loop (cdr rest) (cons (car rest) result))))))

(define (menu-requested event)
  (define pane (hash-ref event "pane_id"))
  (define selected (get event "selected_text" 'json-null))
  (define matched (get (get event "link" #f) "matched" #f))
  (define agent-items
    (if (agent-pane? pane)
      (list (hash "label" "Duplicate agent" "context" (hash "action" "duplicate_agent"))
        (hash "label" "Fork agent" "context" (hash "action" "fork_agent"))
        (hash "separator" #t))
      '()))
  (define link-items
    (if (and (string? matched) (not (equal? matched "")))
      (let ((label (shorten matched)))
        (list (hash "label" (string-append "Open " (if (equal? label "") matched label))
            "context" (hash "action" "open_link" "url" matched))
          (hash "separator" #t)))
      '()))
  (define configured-items
    (filter (lambda (item) item)
      (map (lambda (item)
          (if (and (not (separator? item)) (equal? (get item "label" #f) "Copy"))
            (and (present? selected)
              (let ((quoted (shorten selected)))
                (if (equal? quoted "") item
                  (hash-insert item "label" (string-append "Copy \"" quoted "\"")))))
            item))
        (hash-ref event "items"))))
  (yinz-command
    (hash "SetContextMenu"
      (hash "request_id" (hash-ref event "request_id")
        "items" (tidy (append agent-items link-items configured-items))))))

(define (menu-selected event)
  (define context (hash-ref event "item_context"))
  (define action (get context "action" #f))
  (when (and (equal? action "open_link") (get context "url" #f))
    (open-link (hash-ref context "url")))
  (when (or (equal? action "duplicate_agent") (equal? action "fork_agent"))
    (define pane (hash-ref (hash-ref event "context") "pane_id"))
    (when (agent-pane? pane)
      (define configuration
        (hash "caller_terminal_pane_id" (number->string (hash-ref pane "Terminal"))
          (if (equal? action "fork_agent") "fork_focused" "duplicate_focused") "true"
          "overlay" "true" "modal" "true"))
      (yinz-command
        (hash "RunAction"
          (list
            (hash "LaunchPlugin"
              (hash "plugin"
                (hash "RunPlugin"
                  (hash "location" (hash "Yinz" "new-agent")
                    "configuration" configuration "initial_cwd" 'json-null))
                "should_float" #t "should_open_in_place" #f "close_replaced_pane" #f
                "skip_cache" #f "cwd" 'json-null "no_focus" #f "tab_id" 'json-null))
            (hash)))))))

(define (command-result result)
  (define executable (get (list-ref result 3) "probe" #f))
  (when (and executable (equal? (car result) 0))
    (when (or (equal? executable "xdg-open") (not opener)) (set! opener executable))
    (when pending
      (define url pending)
      (set! pending #f)
      (open-link url))))

(define (load configuration)
  (set! opener #f)
  (set! pending #f)
  (yinz-command (hash "Subscribe" '("ContextMenuRequested" "ContextMenuItemSelected" "RunCommandResult")))
  (run '("xdg-open" "--help") (hash "probe" "xdg-open"))
  (run '("open" "--help") (hash "probe" "open")))

(define (update event)
  (cond
    ((hash-contains? event "ContextMenuRequested") (menu-requested (hash-ref event "ContextMenuRequested")))
    ((hash-contains? event "ContextMenuItemSelected") (menu-selected (hash-ref event "ContextMenuItemSelected")))
    ((hash-contains? event "RunCommandResult") (command-result (hash-ref event "RunCommandResult"))))
  #f)
