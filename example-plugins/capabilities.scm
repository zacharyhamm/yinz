;; Demonstrates permissions, command replies, and persistent render state.
(define output "Waiting for permission")
(define (load configuration)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("RunCommands"))
      (lambda (_)
        (yinz-then
          (yinz-run-command (hash "command" '("/usr/bin/printf" "Hello from Steel")))
          (lambda (result)
            (set! output (yinz-decode-utf8 (hash-ref result "stdout")))
            (yinz-request-render)))))
    (lambda (failure)
      (set! output (string-append "Command failed: " failure))
      (yinz-request-render))))

(define (render rows columns) output)
