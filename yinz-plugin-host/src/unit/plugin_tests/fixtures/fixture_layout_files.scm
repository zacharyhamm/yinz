;; Holds ChangeApplicationState but not the permissions a layout's panes can use, and tries to
;; plant layouts that run commands. ReadApplicationState only reports the layout folder.
(define status "waiting for permissions")
(define (load config)
  (yinz-command (hash "Subscribe" (list "Key")))
  (yinz-then (yinz-wait-for-permissions (list "ChangeApplicationState" "ReadApplicationState"))
    (lambda (ignored) (set! status (string-append "layout dir: " (yinz-command "GetLayoutDir"))))))
(define (attempt label command)
  (set! status
    (with-handler
      (lambda (failure) (string-append label " refused: " (error-object-message failure)))
      (yinz-command command)
      (string-append label " written"))))
(define (save label kdl)
  (attempt label (hash "SaveLayout" (hash "layout_name" "default" "layout_kdl" kdl "overwrite" #t))))
(define (update event)
  (when (hash-contains? event "Key")
    (define key (hash-ref (hash-ref event "Key") "bare_key"))
    (cond
      ((equal? key (hash "Char" "c")) (save "command layout" "layout { pane command=\"sh\"; }"))
      ((equal? key (hash "Char" "t"))
       (save "template layout" "layout { new_tab_template { pane command=\"sh\"; }; tab { pane; }; }"))
      ((equal? key (hash "Char" "r"))
       (attempt "rename"
         (hash "RenameLayout" (hash "old_layout_name" "planted" "new_layout_name" "default"))))))
  #t)
(define (render rows cols) status)
