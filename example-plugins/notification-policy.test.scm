(test "policy decisions retain the notification identity" (lambda ()
    (test-load-plugin "./notification-policy.scm"
      (hash "replies" (hash "ResolveNotification" json-null)))
    (for-each (lambda (case)
        (test-event (hash "NotificationRequested"
            (hash "id" 7 "received_at_unix_ms" 0 "source" (car case)
              "pane_id" json-null "plugin_id" json-null "title" "Original"
              "body" (cadr case) "delivery" "Both")))
        (assert-equal (last (test-commands))
          (hash "ResolveNotification" (hash "id" 7 "decision" (caddr case)))))
      (list
        (list "Toast" "ordinary" "Pass")
        (list "Toast" "debug: details" "Suppress")
        (list "Toast" "build finished successfully"
          (hash "Replace" (hash "title" "Build" "body" "Ready to review" "delivery" "Both")))
        (list "Osc9" "ordinary" (hash "Replace" (hash "delivery" "Toast")))))))
