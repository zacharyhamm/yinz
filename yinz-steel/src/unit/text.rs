use crate::Runtime;

fn check(source: &str, expected: &str) {
    let mut runtime = Runtime::new(Ok).unwrap();
    assert_eq!(
        runtime.eval(source).unwrap().values.last().unwrap(),
        expected,
        "{source}"
    );
}

#[test]
fn regex_unicode_indices_groups_and_simple_folding() {
    check(
        r#"(list (regex? (regex "x")) (regex? "x") (regex? (string-builder)))"#,
        "(#true #false #false)",
    );
    check(
        r#"(hash-get (regex-find (regex "(É)(x)?") "aÉσ") "groups")"#,
        r#"("É" "É" #false)"#,
    );
    check(
        r#"(define found (regex-find (regex "σ") "é🙂σ")) (list (hash-get found "start") (hash-get found "end"))"#,
        "(2 3)",
    );
    check(
        r#"(map (lambda (pair) (regex-match? (regex (car pair) "i") (cadr pair))) '(("s" "ſ") ("k" "K") ("σ" "ς") ("ǆ" "ǅ") ("é" "É") ("ss" "ß")))"#,
        "(#true #true #true #true #true #false)",
    );
    check(
        r#"(map (lambda (s) (regex-match? (regex "\\p{Lu}") s)) '("A" "é" "ǅ" "́" "Ж"))"#,
        "(#true #false #false #false #true)",
    );
    check(
        r#"(map char-upper-case? '(#\A #\a #\ǅ #\Ж))"#,
        "(#true #false #false #true)",
    );
    check(
        r#"(list (char-upper-case? #\Ⅰ) (regex-match? (regex "\\p{Lu}") "Ⅰ") (char-foldcase #\ſ))"#,
        "(#true #false #\\s)",
    );
    check(
        r#"(map char-alphabetic? '(#\A #\ǅ #\中 #\́))"#,
        "(#true #true #true #false)",
    );
    check(
        r#"(map (lambda (m) (hash-get m "start")) (regex-find-all (regex "") "éσ"))"#,
        "(0 1 2)",
    );
    check(
        r#"(regex-replace-all (regex "(?P<x>é)") "é-é" "${x}$$$0")"#,
        r#""é$é-é$é""#,
    );
    check(
        r#"(regex-match? (regex (regex-escape "[a]+")) "[a]+")"#,
        "#true",
    );
}

#[test]
fn regex_rejects_invalid_or_unbounded_work() {
    let mut runtime = Runtime::new(Ok).unwrap();
    for source in [
        r#"(regex "[")"#,
        r#"(regex "a" "g")"#,
        r#"(regex "(?=a)")"#,
        r#"(regex "(a)\\1")"#,
        r#"(regex (make-string 65537 #\x))"#,
        r#"(regex-find-all (regex "") (make-string 100000 #\x))"#,
        r#"(regex-replace-all (regex "x") (make-string 5000 #\x) (make-string 5000 #\y))"#,
    ] {
        assert!(runtime.eval(source).is_err(), "{source}");
    }
}

#[test]
fn strings_count_characters_and_preserve_fields() {
    check(
        r#"(list (string-contains "é🙂é" "é" 1) (string-search-forward "é🙂" "" 2) (string-contains "é" "x"))"#,
        "(2 2 #false)",
    );
    check(
        r#"(list (string-index "éσ🙂" #\🙂) (string-index "aB" char-upper-case?) (string-index "é" #\é 1))"#,
        "(2 1 #false)",
    );
    check(r#"(string-split " é\x0c;σ\t🙂 ")"#, r#"("é" "σ" "🙂")"#);
    check(
        r#"(list (string-split "é,,σ," ",") (string-split "" ",") (string-split ""))"#,
        r#"(("é" "" "σ" "") ("") ())"#,
    );
    check(
        r#"(list (string-trim "  a  b ") (string-trim-left " a ") (string-trim-right " a "))"#,
        r#"("a  b" "a\u{2003}" "\u{2003}a")"#,
    );
    check(
        r#"(list (string-copy "é🙂σ" 1 2) (string-copy "é" 1) (string-pad-left "é🙂" 4 #\σ) (string-pad-left "long" 1))"#,
        r#"("🙂" "" "σσé🙂" "long")"#,
    );
    check(
        r#"(define builder (string-builder)) (string-builder-append! builder "é") (string-builder-append! builder "🙂") (string-builder->string builder)"#,
        r#""é🙂""#,
    );
    let mut runtime = Runtime::new(Ok).unwrap();
    for source in [
        r#"(string-copy "é" 2)"#,
        r#"(string-copy "é" 1 0)"#,
        r#"(string-contains "é" "" -1)"#,
        r#"(string-index "é" #\é 2)"#,
        r#"(string-index "é" #\é 1.0)"#,
        r#"(vector->list (vector 1) 0.0)"#,
        r#"(string-pad-left "" 16777217)"#,
        r#"(define b (string-builder)) (string-builder-append! b (make-string 16777216 #\x)) (string-builder-append! b "x")"#,
    ] {
        assert!(runtime.eval(source).is_err(), "{source}");
    }
}

#[test]
fn collections_and_multiple_lists_preserve_short_circuit_values() {
    check(
        "(list (map + '(1 2 3) '(10 20)) (map + '() '(1)) (map add1 '(1 2)))",
        "((11 22) () (2 3))",
    );
    check("(list (assoc '(1) '(((1) . yes))) (assq 'a '((b . no) (a . yes))) (assoc 'x '()) (cdddr '(1 2 3 4)))", "(((1) . yes) (a . yes) #false (4))");
    check("(list (vector->list (vector 1 2 3) 1 3) (vector->list (vector) 0 0) (list-index even? '(1 3 4)) (list-index even? '()))", "((2 3) () 2 #false)");
    check("(list (ormap (lambda (x) (if x x (error \"short circuit\"))) '(7 #f)) (andmap (lambda (x) x) '(7 8)) (andmap (lambda (x) (if x (error \"short circuit\") #f)) '(#f #t)) (any positive? '()) (andmap positive? '()))", "(7 8 #false #false #true)");
}

#[test]
fn forms_evaluate_once_and_keep_bindings_local() {
    check(
        "(define n 0) (list (case (begin (set! n (+ n 1)) 2) [(1) 'a] [(2 3) 'b]) n)",
        "(b 1)",
    );
    check("(define n 0) (list (cond [(begin (set! n (+ n 1)) 4) => (lambda (x) (+ x 1))]) n (cond [#f => (error \"receiver must stay lazy\")] [else 9]))", "(5 1 9)");
    check("(do ([x 0 (+ x 1)] [y 1 x]) [(= x 3) (list x y)])", "(3 2)");
    check("(letrec ([even (lambda (x) (or (= x 0) (odd (- x 1))))] [odd (lambda (x) (and (> x 0) (even (- x 1))))]) (odd 9))", "#true");
    check("(let ([x 3]) `(a ,x ,@(list 4 5)))", "(a 3 4 5)");
    check("(let ([x 3]) (quasiquote (a (unquote x))))", "(a 3)");
    check("(let ([x 3]) `(a . ,x))", "(a . 3)");
    check(
        "(let ([x 3]) `(a `(b ,x ,,x)))",
        "(a (quasiquote (b (unquote x) (unquote 3))))",
    );
    check(
        "(let ([x 3]) (vector->list `#(a ,x ,@(list 4 5))))",
        "(a 3 4 5)",
    );
    check(
        "(let ([xs (list 3 4)]) `(a `(b ,@xs ,@,xs)))",
        "(a (quasiquote (b (unquote-splicing xs) (unquote-splicing (3 4)))))",
    );
    check("(let ([value 10] [loop 7]) (list (cond [(+ value loop) => (lambda (value) value)]) value loop))", "(17 10 7)");
}

#[test]
fn multiple_values_validate_arity_and_simultaneous_scope() {
    check("(list (call-with-values (lambda () (values 1 2)) +) (call-with-values (lambda () (values)) list) (call-with-values (lambda () 7) list))", "(3 () (7))");
    check("(define count 0) (define-values (a b) (begin (set! count (+ count 1)) (values 3 4))) (list a b count)", "(3 4 1)");
    check("(let ([a 9]) (let-values ([(a b) (values 1 2)] [(c) (values a)] [() (values)]) (list a b c)))", "(1 2 9)");
    let mut runtime = Runtime::new(Ok).unwrap();
    for source in [
        "(let-values ([(a b) (values 1)]) a)",
        "(define-values (a) (values 1 2))",
        "(define-values () (values 1))",
    ] {
        assert!(runtime.eval(source).is_err(), "{source}");
    }
}

#[test]
fn define_values_expansions_are_independent_and_hygienic() {
    check(
        "(define-values (a b) (values 1 2)) (define-values (c d) (values 3 4)) (list a b c d)",
        "(1 2 3 4)",
    );
    check("(define count 0) (define-values () (begin (set! count (+ count 1)) (values))) (define-values () (begin (set! count (+ count 1)) (values))) (define-values (x y) (begin (set! count (+ count 1)) (values 8 9))) (list count x y)", "(3 8 9)");
    check("(let () (define-values (a b) (values 1 2)) (define-values () (values)) (define-values (c) (values 3)) (list a b c))", "(1 2 3)");
    check("(define results 40) (define-values (a b) (values results (+ results 1))) (define-values (c) results) (list a b c results)", "(40 41 40 40)");
    check("(let ([#%values-destructure #f] [#%values-ref #f] [results 9]) (define-values (a b) (values results 10)) (list a b results))", "(9 10 9)");
    check("(define (unpack producer #%values-destructure #%values-ref) (define-values (a b) producer) (list a b)) (unpack (values 11 12) #f #f)", "(11 12)");
    check("(let () (define trace 0) (define-values (a) (begin (set! trace (+ (* trace 10) 1)) (values 3))) (set! trace (+ (* trace 10) 2)) (define-values (b) (begin (set! trace (+ (* trace 10) 3)) (values a))) (define-values () (begin (set! trace (+ (* trace 10) 4)) (values))) (list trace a b))", "(1234 3 3)");
    check(
        "(define-values (even?) (values (lambda (_) 77))) (even? 2)",
        "77",
    );
    check("(define-syntax definitions (syntax-rules () [(definitions x y) (begin (define-values (x) (values 6)) (define-values (y) (values 7)))])) (definitions a b) (list a b)", "(6 7)");
    for source in [
        "(define-values (recur-even recur-odd) (values (lambda (n) (or (= n 0) (recur-odd (- n 1)))) (lambda (n) (and (> n 0) (recur-even (- n 1)))))) (list (recur-even 8) (recur-odd 9))",
        "(let () (define-values (recur-even) (values (lambda (n) (or (= n 0) (recur-odd (- n 1)))))) (define-values (recur-odd) (values (lambda (n) (and (> n 0) (recur-even (- n 1)))))) (list (recur-even 8) (recur-odd 9)))",
    ] {
        check(source, "(#true #true)");
    }
}

#[test]
fn define_values_arity_errors_retain_the_form_source_location() {
    let mut runtime = Runtime::new(Ok).unwrap();
    let error = runtime
        .eval_with_path(
            "\n\n(define-values (first second) (values 1))",
            Some(std::path::Path::new("values.scm")),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("define-values arity mismatch"), "{error}");
    assert!(error.contains("values.scm:3:"), "{error}");
}

#[test]
fn structs_are_distinct_immutable_and_confined() {
    check("(struct point (x y)) (struct other (x y)) (define value (point 3 4)) (list (point? value) (other? value) (point-x value) (point-y value))", "(#true #false 3 4)");
    check("(let ([list-ref #f] [make-struct-type #f]) (struct point (value)) (point-value (point 9)))", "9");
    let mut runtime = Runtime::new(Ok).unwrap();
    runtime
        .eval("(struct point (x)) (struct other (x))")
        .unwrap();
    for source in [
        "(point-x (other 1))",
        "(struct bad (x) #:mutable)",
        "(struct bad (x x))",
        "(struct bad (1))",
        "(point 1 2)",
        "make-struct-type",
        "struct->list",
        "(let-values ([(x) (values 1)]) (open-input-file \"/etc/passwd\"))",
        "`(,(open-input-file \"/etc/passwd\"))",
    ] {
        assert!(runtime.eval(source).is_err(), "{source}");
    }
}

#[test]
fn structs_and_multiple_values_export_from_confined_modules() {
    let mut root = crate::modules::SourceRoot::embedded("main.scm", std::collections::BTreeMap::from([(
        "values.scm".into(),
        "(provide point point? point-x a b c) (struct point (x)) (define-values (a b) (values 3 4)) (define-values () (values)) (define-values (c) (values 5))".into(),
    )])).unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    let output = runtime
        .eval_with_source_root(
            "(require \"./values.scm\") (list (point? (point a)) (point-x (point b)) a b c)",
            &mut root,
        )
        .unwrap();
    assert_eq!(output.values.last().unwrap(), "(#true 4 3 4 5)");
}

#[test]
fn form_feed_parses_formats_and_reparses() {
    let source = r#"(define page "one\ftwo") (string-split page "\f")"#;
    Runtime::new(Ok).unwrap().check(source).unwrap();
    check(source, r#"("one" "two")"#);
    let formatted = crate::format::format(source).unwrap();
    assert!(formatted.contains(r"\f"));
    check(&formatted, r#"("one" "two")"#);
}
