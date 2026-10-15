(define ready #f)
(define (load config)
  (yinz-command (hash "Subscribe" (list "Key")))
  (yinz-then (yinz-wait-for-permissions (list "RunCommands"))
    (lambda (ignored) (set! ready #t))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
    (begin
  (when (equal? (hash-ref (hash-ref event "Key") "bare_key") (hash "Char" "c"))
    (yinz-command (hash "OpenCommandPane"
      (list (hash "path" "sh" "args" (list "-c" "date; sleep 30")) (hash)))))
      #t)
    #f))
(define (render rows cols) (if ready "command fixture ready" "waiting for permissions"))
