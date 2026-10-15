(provide lm-get lm-present? lm-sat lm-at lm-take lm-drop lm-indexed lm-sum lm-max lm-range
  lm-style lm-color lm-all-color lm-draw lm-truncate lm-truncate-start lm-wrap lm-layout
  lm-layout-error lm-sort lm-modified lm-cursor lm-error-message)
(define (lm-get object key fallback) (if (and (hash? object) (hash-contains? object key)) (hash-ref object key) fallback))
(define (lm-present? value) (and value (not (json-null? value))))
(define (lm-sat a b) (max 0 (- a b)))
(define (lm-at values index) (if (and (>= index 0) (< index (length values))) (list-ref values index) #f))
(define (lm-take values count) (if (or (<= count 0) (null? values)) '() (cons (car values) (lm-take (cdr values) (- count 1)))))
(define (lm-drop values count) (if (or (<= count 0) (null? values)) values (lm-drop (cdr values) (- count 1))))
(define (lm-indexed proc values) (let loop ([rest values] [index 0]) (unless (null? rest) (proc index (car rest)) (loop (cdr rest) (+ index 1)))))
(define (lm-sum values) (foldl + 0 values))
(define (lm-max values) (foldl max 0 values))
(define (lm-range start end) (if (>= start end) '() (cons start (lm-range (+ start 1) end))))
(define (lm-style text) (if (string? text) (hash "text" text) text))
(define (lm-color styled level indices)
  (define text (lm-style styled))
  (define levels (lm-get text "levels" '()))
  (hash-insert text "levels" (map (lambda (i) (if (= i level) (append (or (lm-at levels i) '()) indices) (or (lm-at levels i) '()))) (lm-range 0 (max (+ level 1) (length levels))))))
(define (lm-all-color text level) (lm-color text level (lm-range 0 (string-length (hash-ref (lm-style text) "text")))))
(define (lm-draw text x y . width)
  (yinz-ui-text text (yinz-ui-coordinates x y (if (null? width) json-null (car width)) json-null)))
(define (lm-truncate text width)
  (if (<= width 3) (yinz-ui-truncate text width "") (yinz-ui-truncate text width "...")))
(define (lm-truncate-start text width)
  (define reversed (list->string (reverse (string->list text))))
  (list->string (reverse (string->list (lm-truncate reversed width)))))
(define (lm-wrap text width)
  (cond [(= width 0) '()] [(< width 3) (map string (string->list text))]
    [else
      (let loop ([words (split-whitespace text)] [line ""] [lines '()])
        (if (null? words) (reverse (if (equal? line "") lines (cons line lines)))
          (let* ([word (lm-truncate (car words) width)] [joined (string-append line " " word)])
            (cond [(equal? line "") (loop (cdr words) word lines)]
              [(<= (yinz-text-width joined) width) (loop (cdr words) joined lines)]
              [else (loop (cdr words) word (cons line lines))]))))]))
(define (lm-layout info)
  (define file (lm-get info "File" #f))
  (define builtin (lm-get info "BuiltIn" #f))
  (hash "info" info "name" (cond [file (car (reverse (split-many (car file) "/")))] [builtin builtin] [else "raw"])
    "file" (if file (car file) #f) "metadata" (if file (cadr file) #f) "builtin" (and builtin #t)))
(define (lm-layout-error error)
  (define detail (hash-ref error "error"))
  (hash "name" (hash-ref error "layout_name") "file" (hash-ref error "layout_name") "builtin" #f
    "error" (yinz-format-layout-error detail)
    "error_message" (if (equal? detail "SyntaxError") "KDL parsing error"
      (string-append "Layout Error: " (hash-ref (hash-ref (hash-ref detail "KdlError") "kdl_error") "error_message")))))
(define (lm-priority layout) (define name (string-downcase (hash-ref layout "name"))) (cond [(equal? name "default") 0] [(equal? name "compact") 1] [else 2]))
(define (lm-time layout) (define value (lm-get (lm-get layout "metadata" #f) "update_time" "")) (define time (string->number value)) (and time (integer? time) time))
(define (lm-less a b)
  (define ab (lm-get a "builtin" #f)) (define bb (lm-get b "builtin" #f))
  (define (name-less) (string<? (string-downcase (hash-ref a "name")) (string-downcase (hash-ref b "name"))))
  (cond [(and ab bb) (if (= (lm-priority a) (lm-priority b)) (name-less) (< (lm-priority a) (lm-priority b)))]
    [ab #f] [bb #t]
    [else (define at (lm-time a)) (define bt (lm-time b))
      (cond [(and at bt) (> at bt)] [at #t] [bt #f] [else (name-less)])]))
(define (lm-sort layouts) (sort layouts lm-less))
(define (lm-modified layout)
  (define time (lm-time layout))
  (cond [time (define diff (- (quotient (yinz-wall-clock-ms) 1000) time))
      (cond [(< diff 60) "now"] [(< diff 3600) (string-append (number->string (quotient diff 60)) "m ago")]
        [(< diff 86400) (string-append (number->string (quotient diff 3600)) "h ago")]
        [else (string-append (number->string (quotient diff 86400)) "d ago")])]
    [(lm-get layout "builtin" #f) "Built-in"] [else " "]))
(define (lm-cursor position) (yinz-command (hash "ShowCursor" position)))
(define (lm-error-message failure) (trim (error-object-message failure)))
(provide lm-subcolor lm-controls-text)
(define (lm-subcolor styled level pattern)
  (define value (lm-style styled)) (define text (hash-ref value "text"))
  (define size (string-length pattern))
  (define indices
    (let loop ([offset 0])
      (cond [(or (= size 0) (> (+ offset size) (string-length text))) '()]
        [(equal? (substring text offset (+ offset size)) pattern)
          (append (lm-range offset (+ offset size)) (loop (+ offset size)))]
        [else (loop (+ offset 1))])))
  (lm-color value level indices))
(define (lm-controls-text text keys) (foldl (lambda (key value) (lm-subcolor value 3 key)) (lm-style text) keys))
