(yinz-declare-pane)
(define form
  (yinz-ui-form
    (list (yinz-ui-input "name" "Name" "日本語")
      (yinz-ui-checkbox "frames" "Pane frames" #t)
      (yinz-ui-radio "mode" "Mode" '("Local" "Remote") 0)
      (yinz-ui-select "layout" "Layout" '("Default" "Compact") 0)
      (yinz-ui-combo "branch" "Branch" '("main" "feature") "")
      (yinz-ui-button "save" "Save")
      (yinz-ui-button "unavailable" "Unavailable" (hash "disabled" #t)))))
(define dialog
  (yinz-ui-dialog "Review values"
    (list (yinz-ui-button "confirm" "Keep") (yinz-ui-button "cancel" "Cancel"))
    (hash "message" "Keep these values?" "help" "Esc: cancel")))
(define markdown
  (yinz-ui-markdown
    "# Shared widgets\n\nUse **Tab** to move, **Enter** to edit, and **Esc** to cancel.\n\n- Selections start at zero.\n- 日本語 uses terminal columns.\n\n|Control|Purpose|\n|-|-|\n|Combo|Edit or choose|\n|Select|Choose an option|"))
(define confirming #f)
(define status "Edit the form")
(define scroll 0)
(define (active) (if confirming dialog form))
(define (interact result)
  (cond
    [(equal? (hash-ref result "kind") "buttonPressed")
      (if confirming
        (begin
          (when (equal? (hash-ref result "id") "confirm") (set! status (yinz-json-string (yinz-ui-values form))))
          (set! confirming #f))
        (when (equal? (hash-ref result "id") "save")
          (yinz-ui-set-status! dialog "Values stay in this plugin.")
          (set! confirming #t))) #t]
    [(and (equal? (hash-ref result "kind") "cancel") confirming) (set! confirming #f) #t]
    [else (hash-ref result "shouldRender")]))
(define (load configuration)
  (yinz-command (hash "Subscribe" '("Key" "Mouse" "PastedText")))
  (yinz-on-event "Key" (lambda (key)
      (cond [(equal? (hash-ref key "bare_key") "PageDown") (set! scroll (+ scroll 1)) #t]
        [(equal? (hash-ref key "bare_key") "PageUp") (set! scroll (max 0 (- scroll 1))) #t]
        [else (interact (yinz-ui-handle-key (active) key))])))
  (yinz-on-event "Mouse" (lambda (mouse) (interact (yinz-ui-handle-mouse (active) mouse))))
  (yinz-on-event "PastedText" (lambda (text) (interact (yinz-ui-paste! (active) text))))
  (void))
(define (render rows columns)
  (if confirming (yinz-ui-render dialog (yinz-ui-coordinates 0 0 columns rows))
    (let ([form-rows (min 10 rows)])
      (define output (yinz-ui-render form (yinz-ui-coordinates 0 0 columns form-rows)))
      (when (> rows form-rows)
        (define label (yinz-text-truncate status columns))
        (set! output (string-append output
            (yinz-ui-text (yinz-ui-styled label (hash "levels" (list '() '() '() '() (range (string-length label)))))
              (yinz-ui-coordinates 0 form-rows columns 1))
            (hash-ref (yinz-ui-render-buffer markdown
                (yinz-ui-coordinates 0 (+ form-rows 1) columns (max 0 (- rows form-rows 1)) scroll)) "text"))))
      output)))
