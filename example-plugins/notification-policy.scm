;; Bind with: hooks { notification "file:/path/to/notification-policy.scm"; }
(define (load configuration)
  (yinz-on-event "NotificationRequested" (lambda (notification)
      (define body (hash-ref notification "body"))
      (define decision
        (cond
          [(starts-with? body "debug:") "Suppress"]
          [(string-contains? body "build finished")
            (hash "Replace" (hash "title" "Build" "body" "Ready to review" "delivery" "Both"))]
          [(not (equal? (hash-ref notification "source") "Toast"))
            (hash "Replace" (hash "delivery" "Toast"))]
          [else "Pass"]))
      (yinz-command (hash "ResolveNotification"
          (hash "id" (hash-ref notification "id") "decision" decision)))
      #f))
  (void))
