;; Each render and key event uses the host's acting client, including shared instances.
(define plugin-id 0)
(define presses (hash))
(define total 0)
(define last-presser #f)
(define sweep '())
(define last-sweep '())

(define (client-id) (hash-ref (yinz-command "GetPluginIds") "client_id"))
(define (count-for client)
  (define key (number->string client))
  (if (hash-contains? presses key) (hash-ref presses key) 0))

(define (load configuration)
  (yinz-command (hash "Subscribe" '("Key")))
  (define permissions (yinz-wait-for-permissions '("ChangeApplicationState")))
  (set! plugin-id (hash-ref (yinz-command "GetPluginIds") "plugin_id"))
  (yinz-then permissions
    (lambda (ignored)
      (yinz-command (hash "RenamePluginPane" (list plugin-id "shared-test"))))))

(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
    (let ([client (client-id)] [key (hash-ref (hash-ref event "Key") "bare_key")])
      (set! presses (hash-insert presses (number->string client) (+ (count-for client) 1)))
      (set! total (+ total 1))
      (set! last-presser client)
      (cond
        [(equal? key (hash "Char" "m")) (yinz-command (hash "SwitchToMode" "Locked"))]
        [(equal? key (hash "Char" "f"))
          (yinz-command (hash "OpenTerminalFloating"
              (list (hash "path" "." "line_number" 'json-null "cwd" 'json-null) 'json-null)))]
        [(equal? key "Esc") (yinz-command "CloseSelf")])
      #t)
    #f))

(define (render rows columns)
  (define client (client-id))
  (when (member client sweep)
    (set! last-sweep sweep)
    (set! sweep '()))
  (set! sweep (append sweep (list client)))
  (define rendered-for (if (null? last-sweep) sweep last-sweep))
  (define clients (sort (hash-keys->list presses)
      (lambda (left right) (< (string->number left) (string->number right)))))
  (string-append
    "\u{1b}[1mshared-test\u{1b}[0m   plugin_id=" (number->string plugin-id) "\n"
    "you are client " (number->string client) "\towner: " (number->string (car sweep))
    "\trendered for: [" (string-join (map (lambda (id) (string-append "ClientId(" (number->string id) ")")) rendered-for) ", ") "]\n"
    "your presses: " (number->string (count-for client)) "\ttotal: " (number->string total)
    "\tlast presser: " (if last-presser (number->string last-presser) "-") "\n"
    "per client: " (string-join (map (lambda (client) (string-append client " -> " (number->string (hash-ref presses client)))) clients) "   ") "\n"
    "[m] locked mode   [f] floating pane   [Esc] close\n"))
