;; yinz plugin --floating -- file:$PWD/example-plugins/clock.scm
;; The clock declares its pane; a plugin without (yinz-declare-pane) runs headless.
(yinz-declare-pane)
(define label "the UTC time is")
(define (load configuration)
  (when (hash-contains? configuration "label")
    (set! label (hash-ref configuration "label")))
  (yinz-command (hash "Subscribe" '("Key" "Timer")))
  (yinz-command (hash "SetTimeout" 1.0))
  (yinz-wait-for-permissions '("ChangeApplicationState")))

(define (update event)
  (cond
    ((and (hash? event) (hash-contains? event "Timer"))
      (yinz-command (hash "SetTimeout" 1.0)) #t)
    ((and (hash? event) (hash-contains? event "Key")
        (equal? (hash-ref (hash-ref event "Key") "bare_key") "Esc"))
      (yinz-command "CloseSelf") #f)
    (else #f)))

(define (two-digits number)
  (define text (number->string number))
  (if (< number 10) (string-append "0" text) text))

(define (render rows columns)
  (define now (yinz-utc-date (yinz-wall-clock-ms)))
  (string-append label " " (two-digits (hash-ref now "hour")) ":"
    (two-digits (hash-ref now "minute")) ":" (two-digits (hash-ref now "second"))
    "\n\n[Esc] close   (" (number->string rows) "x" (number->string columns) ")"))
