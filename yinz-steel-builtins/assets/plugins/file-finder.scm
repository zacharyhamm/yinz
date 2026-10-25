;; Headless driver for the fzf overlay. The host owns files, panes, focus, and picker replies.
(require "./file-finder/common.scm" "./file-finder/opener.scm" "./file-finder/script.scm")

(define config (hash))
(define out-path "")
(define plugin-id 0)
(define stage "idle")
(define driving #f)
(define pending-cwd #f)
(define listing #f)
(define picker #f)
(define (host-output) (ff-host-path out-path))
(define (close-terminal id) (yinz-command (hash "CloseTerminalPane" id)))
(define (focus-terminal id) (yinz-command (hash "FocusTerminalPane" (list id #t #f))))
(define (notify-error error) (yinz-command (hash "Notify" (list error json-null json-null))))
(define (complete-pick result)
  (set! stage "idle") (set! driving #f)
  (ff-remove (host-output))
  (yinz-command (hash "Pick" (hash "Complete" (hash "result" result)))))
(define (finish)
  (if picker (complete-pick (hash "Ok" json-null))
    (begin (set! stage "idle") (set! driving #f) (ff-remove (host-output)) (yinz-command "CloseSelf"))))
(define (opening-failed finder reason)
  (if picker (complete-pick (hash "Err" reason))
    (begin
      (define message (string-append "file-finder: " reason))
      (displayln message) (notify-error message)
      (set! stage "idle") (set! driving #f)
      (when finder (close-terminal finder)) (finish))))

(define (load configuration)
  ;; Builtins are pre-granted; permission prompts are screen modals.
  (set! config configuration)
  (define ids (yinz-command "GetPluginIds"))
  (set! plugin-id (hash-ref ids "plugin_id"))
  (set! out-path (string-append "/tmp/yinz-file-finder-" (number->string (hash-ref ids "yinz_pid"))
      "-" (number->string plugin-id) ".sel"))
  (yinz-command (hash "Subscribe" '("HostFolderChanged" "FailedToChangeHostFolder" "CommandPaneExited"
        "EditPaneExited" "PaneClosed" "PluginConfigurationChanged")))
  (with-handler (lambda (failure) (displayln (string-append "file-finder: " (trim (error-object-message failure)))) (yinz-command "CloseSelf"))
    (define request (yinz-command (hash "Pick" "Request")))
    (if (ff-present? request)
      (begin (set! picker #t) (set! listing (hash-ref request "options")) (set! pending-cwd (hash-ref request "cwd")))
      (set! pending-cwd (ff-join (hash-ref ids "initial_cwd")
          (ff-safe (lambda ()
              (define focus (yinz-command "GetFocusedPaneInfo"))
              (yinz-command (hash "GetPaneCwd" (hash "pane_id" (hash-ref focus "pane_id"))))) "."))))
    (yinz-command (hash "ChangeHostFolder" "/"))))

(define (open-finder cwd)
  (ff-remove (host-output))
  (with-handler (lambda (failure) (opening-failed #f (trim (error-object-message failure))))
    (define label (ff-project-label cwd))
    (define title (if label (string-append "Find file — " label) "Find file"))
    (define raw (trim (ff-get config "listing_json" "")))
    (when (and (not listing) (not (equal? raw "")))
      (set! listing (ff-validate-options (yinz-json-parse raw))))
    (define script (if listing (ff-structured-script config listing cwd out-path) (ff-script config out-path)))
    (define pane (yinz-command (hash "OpenCommandPaneOverlay"
          (list (hash "path" "/bin/sh" "args" (list "-c" script) "cwd" (ff-host-path cwd))
            (hash "kind" "Dismissible" "coordinates" (ff-geometry config) "background_opacity" json-null) (hash)))))
    (define terminal (ff-get pane "Terminal" #f))
    (if terminal
      (begin (yinz-command (hash "RenameTerminalPane" (list terminal title)))
        (set! stage "finding") (set! driving terminal))
      (begin
        (displayln "file-finder: failed to open the finder pane")
        (if picker (complete-pick (hash "Err" "failed to open finder pane"))
          (yinz-command "CloseSelf"))))))

(define (open-reader finder selection opener)
  (define id (hash-ref opener "plugin_id"))
  (define ids (if (ff-present? id) (list id)
      (yinz-command (hash "GetPluginIdsForUrlWithConfiguration"
          (hash "url" (hash-ref opener "plugin") "configuration" (hash-ref opener "config"))))))
  (define reader (if (null? ids) #f (car (sort ids <))))
  (define delivery (hash-ref opener "delivery"))
  (define background
    (if reader
      (let ([info (yinz-command (hash "GetPluginInfo" reader))])
        (unless (ff-present? info) (error "reader disappeared before handoff"))
        (hash-ref info "background"))
      (begin (when (equal? delivery "message") (error "message destination is not running")) #f)))
  (set! stage "idle") (set! driving #f)
  ;; Retire the overlay before focusing the reader; overlay cleanup restores focus.
  (close-terminal finder)
  (unless reader
    (define pane (yinz-command (hash "OpenPluginPaneFloating"
          (hash "plugin_url" (hash-ref opener "plugin")
            "configuration" (hash-insert (hash-ref opener "config") "caller_cwd" (ff-parent selection))
            "floating_pane_coordinates" json-null "context" (hash)))))
    (set! reader (ff-get pane "Plugin" #f))
    (unless reader (error (string-append "failed to open reader " (hash-ref opener "plugin")))))
  (yinz-command (hash "MessageToPlugin"
      (hash "destination_plugin_id" reader "message_name" (hash-ref opener "message")
        "message_args" (hash-ref opener "args") "message_payload" selection)))
  (when (and (equal? delivery "auto") (not background))
    (yinz-command (hash "FocusPluginPane" (list reader #t #f))))
  (finish))

(define (handle-selection finder)
  (with-handler (lambda (failure) (opening-failed finder (trim (error-object-message failure))))
    (define contents (if listing
        (with-handler (lambda (failure) (error (string-append "cannot read picker output: " (trim (error-object-message failure)))))
          (ff-read (host-output)))
        (ff-safe (lambda () (ff-read (host-output))) "")))
    (ff-remove (host-output))
    (define selection (ff-selection contents listing))
    (cond
      [(json-null? selection) (unless picker (close-terminal finder)) (finish)]
      [picker (complete-pick (hash "Ok" selection))]
      [else
        (define cwd (ff-host-path (ff-parent selection)))
        (define extension-path (if (and listing (equal? (hash-ref listing "kind") "directory")) "/" selection))
        (define opener (ff-opener config extension-path selection))
        (if (and (hash? opener) (hash-contains? opener "plugin"))
          (open-reader finder selection opener)
          (let ([opened (yinz-command
                  (if (equal? opener "editor")
                    (hash "OpenEditPaneInPlaceOfPaneId"
                      (list (hash "Terminal" finder)
                        (hash "path" (ff-host-path selection) "line_number" json-null "cwd" cwd) #t (hash)))
                    (hash "OpenCommandPaneInPlaceOfPaneId"
                      (list (hash "Terminal" finder) (hash-insert (hash-ref opener "command") "cwd" cwd) #t (hash)))))])
            (define terminal (ff-get opened "Terminal" #f))
            (if terminal
              (begin (set! stage "opened") (set! driving terminal) (focus-terminal terminal))
              (begin
                (displayln (string-append "file-finder: failed to open " selection))
                (close-terminal finder) (finish)))))])))

;; The driver has no pane, so a second LaunchOrFocusPlugin (Alt e) finds nothing to focus. The
;; host tells it with a private `launch_or_focus` pipe instead; bring back the finder or editor.
(define (pipe message)
  (when (and driving (ff-get message "is_private" #f) (equal? (ff-get message "name" #f) "launch_or_focus"))
    (focus-terminal driving))
  #f)

(define (pane-exited pane)
  (when (and driving (= pane driving))
    (cond [(equal? stage "finding") (handle-selection pane)]
      [(equal? stage "opened") (close-terminal pane) (finish)])))
(define (update event)
  (cond
    [(and pending-cwd (equal? (ff-get event "HostFolderChanged" #f) "/"))
      (define cwd pending-cwd) (set! pending-cwd #f) (open-finder cwd)]
    [(and pending-cwd (hash? event) (hash-contains? event "FailedToChangeHostFolder"))
      (set! pending-cwd #f)
      (if picker (complete-pick (hash "Err" "failed to mount filesystem root")) (finish))]
    [(ff-get event "CommandPaneExited" #f)
      (define result (hash-ref event "CommandPaneExited"))
      (define pane (car result)) (define status (cadr result))
      (if (and listing driving (equal? stage "finding") (= pane driving) (not (equal? status 0)))
        (opening-failed pane (cond [(equal? status 10) "listing failed"] [(equal? status 11) "fzf is not installed"] [else "fzf failed"]))
        (pane-exited pane))]
    [(ff-get event "EditPaneExited" #f) (pane-exited (car (hash-ref event "EditPaneExited")))]
    [(and driving (equal? (ff-get event "PaneClosed" #f) (hash "Terminal" driving))) (finish)]
    [(and (not picker) (ff-get event "PluginConfigurationChanged" #f))
      (set! config (hash-ref event "PluginConfigurationChanged"))])
  #f)
