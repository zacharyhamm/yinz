use super::*;

#[test]
fn formatting_indents_code_and_preserves_literal_and_comment_contents() {
    let source = "  ; comment\n  (define (render rows columns)\n(display \"first\n    second\") ; inline\n#| block\n    inside |#\n(cond\n[(= rows 1) \"one\"]\n[else \"many\"])\n)\n\n";
    let expected = "; comment\n(define (render rows columns)\n  (display \"first\n    second\") ; inline\n  #| block\n    inside |#\n  (cond\n    [(= rows 1) \"one\"]\n    [else \"many\"])\n)\n";
    let formatted = format(source).unwrap();
    assert_eq!(formatted, expected);
    assert_eq!(format(&formatted).unwrap(), formatted);
}

#[test]
fn formatting_preserves_blank_lines_inside_multiline_literals() {
    let source = "(define text \"first\n  \nlast\")\n";
    assert_eq!(format(source).unwrap(), source);
}
