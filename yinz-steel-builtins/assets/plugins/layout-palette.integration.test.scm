(require "./testing/common.scm")
(define (open-layout-actions)
  (s-key "Ctrl Alt p") (s-text "Layouts") (test-wait-for-text "› Layouts…") (s-key "Enter")
  (test-wait-for-text "palette-layout") (s-text "palette-layout") (test-wait-for-text "› palette-layout") (s-key "Enter")
  (test-wait-for-text "Open layout in new tabs"))
(test "layout palette saves edits applies and deletes a layout in a real session"
  (hash "grants" '("OpenFiles" "RunCommands" "OpenTerminalsOrPlugins")) (lambda ()
    (define home (s-fixture (hash) (hash "vi"
          "printf 'LAYOUT EDITOR: %s\\r\\n' \"$1\"\nwhile IFS= read -r line; do printf 'EDITOR INPUT: %s\\r\\n' \"$line\"; done\n")))
    (s-start (hash "columns" 160 "rows" 40
        "backgroundPlugins" (list (hash "path" "yinz:layout-palette" "configuration" (hash "shared" "true")))))
    (test-wait-for-text "yinz-test>")
    (s-key "Ctrl Alt p") (s-text "Save session as layout") (test-wait-for-text "› Save session as layout…") (s-key "Enter")
    (test-wait-for-text "Name (optional):") (s-text "palette-layout") (s-key "Enter") (s-key "Enter") (s-key "Enter")
    (test-wait-for-text "Layout saved")
    (open-layout-actions) (s-text "Edit layout") (test-wait-for-text "› Edit layout in editor") (s-key "Enter")
    (test-wait-for-text "LAYOUT EDITOR:")
    ;; The path is read from the pane's title: the completion toast can cover it on screen.
    (define layout-path
      (string-append (substring home 0 (- (string-length home) 5)) "/config/layouts/palette-layout.kdl"))
    (test-wait-for "focused tiled editor" (lambda ()
        (t-find (lambda (pane) (and (s-terminal? pane) (hash-ref pane "is_focused")
              (string-contains? (hash-ref pane "title") layout-path)
              (not (hash-ref pane "is_floating")) (not (hash-ref pane "is_suppressed")))) (s-panes))))
    ;; The palette closes as the action completes, which can trail the editor opening. Until it
    ;; has, typed text goes to the palette and Ctrl Alt p closes it rather than opening it.
    (test-wait-for-text "Layout editor opened")
    (s-text "still-visible") (s-key "Enter") (test-wait-for-text "EDITOR INPUT: still-visible")
    (s-action "CloseFocus;")
    (test-wait-for "editor closes and original terminal remains" (lambda ()
        (and (= 1 (length (filter s-terminal? (s-panes)))) (not (string-contains? (s-screen) "LAYOUT EDITOR:")))))
    (s-action "NewPane;") (test-wait-for "two terminal panes" (lambda () (= 2 (length (filter s-terminal? (s-panes))))))
    (open-layout-actions) (s-text "Apply layout") (test-wait-for-text "› Apply layout…") (s-key "Enter") (test-wait-for-text "Retain existing panes:")
    (s-key "Enter") (s-key "Enter") (s-key "Down") (s-key "Enter")
    (test-wait-for "saved layout restores one terminal pane" (lambda () (= 1 (length (filter s-terminal? (s-panes))))))
    ;; The layout lands before the plugin's reply closes the palette; a Ctrl Alt p before that
    ;; would close it instead of opening a new one.
    (test-wait-for-text "Layout application requested")
    (open-layout-actions) (s-text "Delete layout") (test-wait-for-text "› Delete layout…") (s-key "Enter") (test-wait-for-text "Permanently removes this saved layout file")
    (s-key "Down") (s-key "Enter") (test-wait-for-text "Layout deleted")))
