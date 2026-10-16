(define-syntax and
  (syntax-rules () [(and) #t] [(and x) x] [(and x rest ...) (if x (and rest ...) #f)]))
(define-syntax or
  (syntax-rules () [(or) #f] [(or x) x] [(or x rest ...) (let ([value x]) (if value value (or rest ...)))]))
(define-syntax cond
  (syntax-rules (else =>)
    [(cond) (#%void)]
    [(cond (else body ...)) (begin body ...)]
    [(cond (test => receiver) rest ...)
     (let ([value test]) (if value (receiver value) (cond rest ...)))]
    [(cond (test) rest ...)
     (let ([value test]) (if value value (cond rest ...)))]
    [(cond (test body ...) rest ...) (if test (begin body ...) (cond rest ...))]))
(define-syntax when (syntax-rules () [(when test body ...) (if test (begin body ...) (#%void))]))
(define-syntax unless (syntax-rules () [(unless test body ...) (if test (#%void) (begin body ...))]))
(define-syntax let*
  (syntax-rules () [(let* () body ...) (let () body ...)] [(let* ([name value] rest ...) body ...) (let ([name value]) (let* (rest ...) body ...))]))
(define (void) (#%void))
(define (caar x) (car (car x)))
(define (cadr x) (car (cdr x)))
(define (cdar x) (cdr (car x)))
(define (cddr x) (cdr (cdr x)))
(define (caddr x) (car (cdr (cdr x))))
(define (cadddr x) (car (cdr (cdr (cdr x)))))
(define (add1 x) (+ x 1))
(define (sub1 x) (- x 1))
(define (zero? x) (= x 0))
(define (positive? x) (> x 0))
(define (negative? x) (< x 0))
(define (max a b) (if (> a b) a b))
(define (min a b) (if (< a b) a b))
;; Share recursive helpers so nested maps do not allocate self-referential closures per call.
(define (%map-one function accum items)
  (if (null? items) (reverse accum)
    (%map-one function (cons (function (car items)) accum) (cdr items))))
(define (%map-any-empty? lists)
  (and (pair? lists) (or (null? (car lists)) (%map-any-empty? (cdr lists)))))
(define (%map-many function accum lists)
  (if (%map-any-empty? lists) (reverse accum)
    (%map-many function (cons (apply function (%map-one car '() lists)) accum)
      (%map-one cdr '() lists))))
(define (map function items . more-lists)
  (if (null? more-lists) (%map-one function '() items)
    (%map-many function '() (cons items more-lists))))
(define (filter f xs) (if (null? xs) '() (if (f (car xs)) (cons (car xs) (filter f (cdr xs))) (filter f (cdr xs)))))
(define (for-each f xs) (unless (null? xs) (f (car xs)) (for-each f (cdr xs))))
(define (foldl f value xs) (if (null? xs) value (foldl f (f (car xs) value) (cdr xs))))
(define (foldr f value xs) (if (null? xs) value (f (car xs) (foldr f value (cdr xs)))))
(define (take xs count)
  (cond [(negative? count) (error "take expects a nonnegative count")]
        [(zero? count) '()]
        [else (cons (car xs) (take (cdr xs) (- count 1)))]))
(define (drop xs count)
  (cond [(negative? count) (error "drop expects a nonnegative count")]
        [(zero? count) xs]
        [else (drop (cdr xs) (- count 1))]))
(define (sort xs comparator)
  (define (split xs count)
    (if (= count 0) (cons '() xs)
        (let ([rest (split (cdr xs) (- count 1))])
          (cons (cons (car xs) (car rest)) (cdr rest)))))
  (define (merge left right)
    (cond [(null? left) right]
          [(null? right) left]
          [(comparator (car right) (car left)) (cons (car right) (merge left (cdr right)))]
          [else (cons (car left) (merge (cdr left) right))]))
  (if (or (null? xs) (null? (cdr xs))) xs
      (let ([parts (split xs (quotient (length xs) 2))])
        (merge (sort (car parts) comparator) (sort (cdr parts) comparator)))))
(define-syntax with-handler (syntax-rules () [(with-handler handler body ...) (call-with-exception-handler handler (lambda () body ...))]))

(define-syntax quasiquote
  (syntax-rules () [(quasiquote template) (%quasiquote template ())]))
(define-syntax %quasiquote
  (syntax-rules (quasiquote unquote #%unquote unquote-splicing #%unquote-splicing)
    [(%quasiquote (quasiquote template) depth)
     (list 'quasiquote (%quasiquote template (nested . depth)))]
    [(%quasiquote (unquote expression) ()) expression]
    [(%quasiquote (#%unquote expression) ()) expression]
    [(%quasiquote (unquote expression) (first . rest))
     (list 'unquote (%quasiquote expression rest))]
    [(%quasiquote (#%unquote expression) (first . rest))
     (list 'unquote (%quasiquote expression rest))]
    [(%quasiquote ((unquote-splicing expression) . tail) ())
     (append expression (%quasiquote tail ()))]
    [(%quasiquote ((#%unquote-splicing expression) . tail) ())
     (append expression (%quasiquote tail ()))]
    [(%quasiquote (unquote-splicing expression) (first . rest))
     (list 'unquote-splicing (%quasiquote expression rest))]
    [(%quasiquote (#%unquote-splicing expression) (first . rest))
     (list 'unquote-splicing (%quasiquote expression rest))]
    [(%quasiquote #(element ...) depth)
     (list->vector (%quasiquote (element ...) depth))]
    [(%quasiquote (head . tail) depth)
     (cons (%quasiquote head depth) (%quasiquote tail depth))]
    [(%quasiquote atom depth) 'atom]))

(define (cdddr x) (cdr (cdr (cdr x))))
(define (assoc key entries)
  (cond [(null? entries) #f]
        [(equal? key (car (car entries))) (car entries)]
        [else (assoc key (cdr entries))]))
(define (assq key entries)
  (cond [(null? entries) #f]
        [(eq? key (car (car entries))) (car entries)]
        [else (assq key (cdr entries))]))
(define (vector->list vector . bounds)
  (define size (vector-length vector))
  (define start (if (null? bounds) 0 (car bounds)))
  (define end (if (or (null? bounds) (null? (cdr bounds))) size (cadr bounds)))
  (unless (and (<= (length bounds) 2) (exact-integer? start) (exact-integer? end)
               (<= 0 start) (<= start end) (<= end size))
    (error "vector->list expects valid start and end indices"))
  (let loop ([i (- end 1)] [result '()])
    (if (< i start) result (loop (- i 1) (cons (vector-ref vector i) result)))))
(define (list-index predicate xs)
  (let loop ([xs xs] [i 0])
    (cond [(null? xs) #f] [(predicate (car xs)) i]
          [else (loop (cdr xs) (+ i 1))])))
(define (ormap predicate xs . more)
  (define lists (cons xs more))
  (let loop ([lists lists])
    (if (%map-any-empty? lists) #f
        (let ([value (apply predicate (map car lists))])
          (if value value (loop (map cdr lists)))))))
(define any ormap)
(define (andmap predicate xs . more)
  (define lists (cons xs more))
  (let loop ([lists lists] [last #t])
    (if (%map-any-empty? lists) last
        (let ([value (apply predicate (map car lists))])
          (if value (loop (map cdr lists) value) #f)))))
(define string-trim trim)
(define string-trim-left trim-start)
(define string-trim-right trim-end)
(define (string-index source match . bounds)
  (define start (if (null? bounds) 0 (car bounds)))
  (unless (and (<= (length bounds) 1) (exact-integer? start)
               (<= 0 start) (<= start (string-length source)))
    (error "string-index expects a valid start index"))
  (define predicate (if (char? match) (lambda (value) (equal? value match)) match))
  (let loop ([chars (drop (string->list source) start)] [i start])
    (cond [(null? chars) #f] [(predicate (car chars)) i]
          [else (loop (cdr chars) (+ i 1))])))
(define-syntax case
  (syntax-rules ()
    [(case key clauses ...) (let ([value key]) (%case value clauses ...))]))
(define-syntax %case
  (syntax-rules (else)
    [(%case value) (#%void)]
    [(%case value (else body ...)) (begin body ...)]
    [(%case value ((datum ...) body ...) rest ...)
     (if (ormap (lambda (candidate) (equal? value candidate)) '(datum ...))
         (begin body ...) (%case value rest ...))]))
(define-syntax letrec
  (syntax-rules ()
    [(letrec bindings body ...)
     (let () (%letrec-bind bindings body ...))]))
(define-syntax %letrec-bind
  (syntax-rules ()
    [(%letrec-bind () body ...) (begin body ...)]
    [(%letrec-bind ((name value) rest ...) body ...)
     (begin (define name value) (%letrec-bind (rest ...) body ...))]))
(define-syntax do
  (syntax-rules ()
    [(do ((var init step ...) ...) (test expr ...) command ...)
     (letrec ([loop (lambda (var ...)
                     (if test (begin (#%void) expr ...)
                         (begin command ... (loop (do "step" var step ...) ...))))])
       (loop init ...))]
    [(do "step" var) var]
    [(do "step" var step) step]))
(define values list)
(define (call-with-values producer consumer)
  (define result (producer))
  (if (list? result) (apply consumer result) (consumer result)))
(define (%values-list result)
  (if (list? result) result (list result)))
(define-syntax let-values
  (syntax-rules ()
    [(let-values ((names producer) ...) body ...)
     (let ([results (list (%values-list producer) ...)])
       (%let-values-bind results (names ...) body ...))]))
(define-syntax %let-values-bind
  (syntax-rules ()
    [(%let-values-bind results () body ...) (begin body ...)]
    [(%let-values-bind results ((name ...) rest ...) body ...)
     (apply (lambda (name ...) (%let-values-bind (cdr results) (rest ...) body ...))
            (car results))]))
(define-syntax define-values
  (syntax-rules ()
    [(define-values (name ...) producer)
     (begin
       (define results (%values-list producer))
       (unless (= (length results) (length '(name ...)))
         (error "define-values arity mismatch"))
       (%define-values-bind (name ...) results))]))
(define-syntax %define-values-bind
  (syntax-rules ()
    [(%define-values-bind () results) (#%void)]
    [(%define-values-bind (name rest ...) results)
     (begin (define name (car results))
            (%define-values-bind (rest ...) (cdr results)))]))
