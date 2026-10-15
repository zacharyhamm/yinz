(define-syntax and
  (syntax-rules () [(and) #t] [(and x) x] [(and x rest ...) (if x (and rest ...) #f)]))
(define-syntax or
  (syntax-rules () [(or) #f] [(or x) x] [(or x rest ...) (let ([value x]) (if value value (or rest ...)))]))
(define-syntax cond
  (syntax-rules (else)
    [(cond) (#%void)]
    [(cond (else body ...)) (begin body ...)]
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
(define (map f xs) (if (null? xs) '() (cons (f (car xs)) (map f (cdr xs)))))
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
