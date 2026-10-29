;; Load as a background plugin, then search for "Table example" in Do.
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
      (yinz-palette-register "table-example" "Table example"
        (list (hash "id" "records" "label" "Open table example"
            "run" (lambda (values context)
              (hash "type" "actions" "title" "Records"
                "columns" (list
                  (hash "header" "Number" "width" 8)
                  (hash "header" "Type" "width" 10)
                  (hash "header" "Subject" "maxLines" 3)
                  (hash "header" "Submitter" "width" 14)
                  (hash "header" "Updated (UTC)" "width" 16))
                "actions" (list
                  (hash "id" "42" "label" "Record 42: Unicode rendering"
                    "aliases" '("雪" "Zoë")
                    "cells" (list "42" "Feature"
                      "Unicode 雪 and 🦀: a long subject demonstrating aligned metadata and wrapping across three lines, with an ellipsis when this subject exceeds the available space in the command palette. More details continue here to demonstrate truncation."
                      "Zoë" "2026-10-29 12:00")
                    "run" (lambda (values context)
                      (hash "type" "text" "title" "Record 42" "text" "Opened the original table action.")))
                  (hash "id" "jump" "label" "Jump to number…"
                    "run" (lambda (values context)
                      (hash "type" "text" "title" "Jump" "text" "An ordinary full-width action in a table view."))))))))))))
