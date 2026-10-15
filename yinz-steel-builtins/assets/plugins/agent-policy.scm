;; Headless hook: report each firing and focus a waiting agent from its toast.
(define (get object key fallback)
  (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))

(define (nonempty value fallback)
  (if (and (string? value) (not (equal? value ""))) value fallback))

(define (load configuration)
  (yinz-command (hash "Subscribe" '("Hook"))))

(define (update event)
  (when (hash-contains? event "Hook")
    (define hook (hash-ref event "Hook"))
    (define point (hash-ref hook "point"))
    (define payload (hash-ref hook "payload"))
    (yinz-log (string-append "hook " point " " (yinz-json-string payload)))
    (when (and (equal? point "agent_waiting") (hash-contains? payload "agent"))
      (define agent (hash-ref payload "agent"))
      (define pane (hash-ref payload "pane_id"))
      (define message
        (string-append (nonempty (get agent "kind" #f) "agent")
          " in pane " (number->string pane) ": "
          (nonempty (get agent "detail" #f) "is waiting for you")))
      (yinz-command (hash "Notify" (list message (hash "Terminal" pane) 'json-null)))))
  #f)
