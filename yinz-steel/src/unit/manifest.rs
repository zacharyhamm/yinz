use super::*;
use crate::Runtime;
use hegel::generators;
use serde_json::{json, Value};
use steel::parser::{
    ast::ExprKind,
    lexer::TokenStream,
    parser::{ParseError, Parser},
    tokens::TokenType,
};

fn scan(source: &str) -> PaneDeclaration {
    pane_declaration(source).unwrap()
}

fn unless(key: &str, value: &str) -> PaneDeclaration {
    PaneDeclaration::Unless {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

#[test]
fn literal_top_level_forms_declare_a_pane() {
    for source in [
        "(yinz-declare-pane)",
        "[yinz-declare-pane]",
        // Steel evaluates a curly list as a call too.
        "{yinz-declare-pane}",
        "(require \"lib.scm\")\n(yinz-declare-pane)\n(define (render rows cols) \"\")",
        "( yinz-declare-pane ; trailing comment\n )",
        "(#| block |# yinz-declare-pane)",
        "(yinz-declare-pane #;(ignored argument))",
        "#!/usr/bin/env steel\n(yinz-declare-pane)",
    ] {
        assert_eq!(scan(source), PaneDeclaration::Always, "{source:?}");
    }
    assert_eq!(
        scan("(yinz-declare-pane-unless \"duplicate_focused\" \"true\")"),
        unless("duplicate_focused", "true")
    );
    assert_eq!(
        scan("[yinz-declare-pane-unless \"a\\\"b\" #;x \"\\\\\"]"),
        unless("a\"b", "\\")
    );
    assert_eq!(
        scan("{yinz-declare-pane-unless \"a\" \"b\"}"),
        unless("a", "b")
    );
}

#[test]
fn quoted_commented_and_nested_forms_do_not_declare_a_pane() {
    for source in [
        "",
        "; (yinz-declare-pane)",
        "#| (yinz-declare-pane) |#",
        "#;(yinz-declare-pane)",
        "#; #; x (yinz-declare-pane)",
        "#; 'x 'x #;(yinz-declare-pane)",
        "\"(yinz-declare-pane)\"",
        "(display \"(yinz-declare-pane)\")",
        "#\\( yinz-declare-pane #\\)",
        "'(yinz-declare-pane)",
        "`(yinz-declare-pane)",
        "#'(yinz-declare-pane)",
        "(quote (yinz-declare-pane))",
        "#(yinz-declare-pane)",
        "(begin (yinz-declare-pane))",
        "(when #t (yinz-declare-pane))",
        "(define (load configuration) (yinz-declare-pane))",
        "(\"yinz-declare-pane\")",
        "(yinz-declare-pane-extra)",
        "yinz-declare-pane",
    ] {
        assert_eq!(scan(source), PaneDeclaration::None, "{source:?}");
    }
    // A quote or datum comment applies to the next datum, not to the one after it.
    assert_eq!(scan("'x (yinz-declare-pane)"), PaneDeclaration::Always);
    assert_eq!(scan("#;x (yinz-declare-pane)"), PaneDeclaration::Always);
    assert_eq!(scan("#; 'x (yinz-declare-pane)"), PaneDeclaration::Always);
}

#[test]
fn malformed_and_duplicate_declarations_are_errors() {
    for (source, message) in [
        (
            "(yinz-declare-pane)\n[yinz-declare-pane]",
            "line 2: the pane is declared more than once (first on line 1)",
        ),
        (
            "(yinz-declare-pane)\n(yinz-declare-pane-unless \"a\" \"b\")",
            "declared more than once",
        ),
        ("(yinz-declare-pane #t)", "takes no arguments"),
        ("(yinz-declare-pane-unless \"a\")", "two string literals"),
        ("(yinz-declare-pane-unless \"a\" b)", "two string literals"),
        ("(yinz-declare-pane-unless \"a\" 'b)", "two string literals"),
        (
            "(yinz-declare-pane-unless \"a\" \"b\" \"c\")",
            "two string literals",
        ),
        (
            "(yinz-declare-pane-unless \"a\" (string \"b\"))",
            "two string literals",
        ),
        // Steel reads these as literal declarations; the scan rejects the dotted spelling.
        ("(yinz-declare-pane . ())", "takes no arguments"),
        (
            "(yinz-declare-pane-unless \"a\" \"b\" . ())",
            "two string literals",
        ),
    ] {
        let error = pane_declaration(source).unwrap_err().to_string();
        assert!(error.contains(message), "{source:?}: {error}");
    }
}

#[test]
fn large_sources_report_the_line_of_a_duplicate() {
    // Many top-level forms after a large literal: line numbers must not rescan the source.
    let forms = 100_000;
    let source = format!(
        "\"{}\"\n(yinz-declare-pane)\n{}(yinz-declare-pane)",
        "x\n".repeat(1 << 20),
        "()\n".repeat(forms)
    );
    let error = pane_declaration(&source).unwrap_err().to_string();
    let first = (1 << 20) + 2;
    assert_eq!(
        error,
        format!(
            "line {}: the pane is declared more than once (first on line {first})",
            first + forms + 1
        )
    );
}

#[test]
fn declarations_apply_to_the_load_configuration() {
    let empty = BTreeMap::new();
    let duplicate = BTreeMap::from([("duplicate_focused".to_owned(), "true".to_owned())]);
    let other = BTreeMap::from([("duplicate_focused".to_owned(), "false".to_owned())]);
    let conditional = unless("duplicate_focused", "true");
    assert!(!PaneDeclaration::None.applies(&duplicate));
    assert!(PaneDeclaration::Always.applies(&duplicate));
    assert!(conditional.applies(&empty));
    assert!(conditional.applies(&other));
    assert!(!conditional.applies(&duplicate));
    assert_eq!(PaneDeclaration::None.describe(), "none");
    assert_eq!(PaneDeclaration::Always.describe(), "always");
    assert_eq!(conditional.describe(), "unless duplicate_focused=true");
}

fn declared(source: &str, configuration: Value) -> Value {
    let mut runtime = Runtime::new(Ok).unwrap();
    runtime.eval(source).unwrap();
    runtime
        .invoke("yinz-pane-declared?", vec![configuration])
        .unwrap()
}

#[test]
fn sdk_reports_the_evaluated_declaration() {
    let conditional = "(yinz-declare-pane-unless \"duplicate_focused\" \"true\")";
    assert_eq!(declared("(define x 1)", json!({})), json!(false));
    assert_eq!(declared("(yinz-declare-pane)", json!({})), json!(true));
    assert_eq!(declared(conditional, json!({})), json!(true));
    assert_eq!(
        declared(conditional, json!({"duplicate_focused": "false"})),
        json!(true)
    );
    assert_eq!(
        declared(conditional, json!({"duplicate_focused": "true"})),
        json!(false)
    );
    // The scan misses this one; the host's comparison then fails the load.
    assert_eq!(
        declared("(when #t (yinz-declare-pane))", json!({})),
        json!(true)
    );
}

#[test]
fn sdk_rejects_repeated_late_and_non_literal_declarations() {
    let mut runtime = Runtime::new(Ok).unwrap();
    let error = runtime
        .eval("(yinz-declare-pane) (yinz-declare-pane)")
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("declares its pane once"),
        "{error:#}"
    );

    let mut runtime = Runtime::new(Ok).unwrap();
    let error = runtime
        .eval("(yinz-declare-pane-unless 'key \"value\")")
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("two string literals"),
        "{error:#}"
    );

    let mut runtime = Runtime::new(Ok).unwrap();
    runtime
        .eval("(define (load configuration) (yinz-declare-pane))")
        .unwrap();
    assert_eq!(
        runtime
            .invoke("yinz-pane-declared?", vec![json!({})])
            .unwrap(),
        json!(false)
    );
    let error = runtime.invoke("load", vec![json!({})]).unwrap_err();
    assert!(
        format!("{error:#}").contains("must be a top-level form"),
        "{error:#}"
    );
}

/// The declaration a full parse sees, or `None` when the source does not parse.
fn reference(source: &str) -> Option<std::result::Result<PaneDeclaration, ()>> {
    parsed(source).ok()
}

/// The declaration a full parse sees, or the parse error.
fn parsed(
    source: &str,
) -> std::result::Result<std::result::Result<PaneDeclaration, ()>, ParseError> {
    let expressions = Parser::new_flat(source, None).collect::<std::result::Result<Vec<_>, _>>()?;
    let identifier = |expression: &ExprKind| match expression {
        ExprKind::Atom(atom) => match &atom.syn.ty {
            TokenType::Identifier(name) => Some(name.resolve().to_owned()),
            _ => None,
        },
        _ => None,
    };
    let string = |expression: &ExprKind| match expression {
        ExprKind::Atom(atom) => match &atom.syn.ty {
            TokenType::StringLiteral(text) => Some(text.resolve().to_owned()),
            _ => None,
        },
        _ => None,
    };
    let mut found = None;
    for expression in &expressions {
        let ExprKind::List(list) = expression else {
            continue;
        };
        let Some((head, arguments)) = list.args.split_first() else {
            continue;
        };
        let declaration = match identifier(head).as_deref() {
            Some(DECLARE) if arguments.is_empty() => PaneDeclaration::Always,
            Some(DECLARE_UNLESS) => match arguments {
                [key, value] => match (string(key), string(value)) {
                    (Some(key), Some(value)) => PaneDeclaration::Unless { key, value },
                    _ => return Ok(Err(())),
                },
                _ => return Ok(Err(())),
            },
            Some(DECLARE) => return Ok(Err(())),
            _ => continue,
        };
        if found.replace(declaration).is_some() {
            return Ok(Err(()));
        }
    }
    Ok(Ok(found.unwrap_or(PaneDeclaration::None)))
}

/// A string literal, sometimes spelled with escapes.
fn string_literal(tc: &hegel::TestCase) -> String {
    let text = tc.draw(generators::text().max_size(8));
    let mut literal = format!("\"{}", text.replace('\\', "\\\\").replace('"', "\\\""));
    if tc.draw(generators::booleans()) {
        literal.push_str(tc.draw(generators::sampled_from(vec![
            "\\n",
            "\\t\\a\\b\\f\\0\\r",
            "\\|",
            "\\x41;",
            "\\x3bb;",
            "\\u{1F600}",
            "\\u3bb;",
            // Line continuations, with and without trailing spaces.
            "\\\n   after",
            "\\  \n\tafter",
            "\\\t\nafter",
        ])));
    }
    literal.push('"');
    literal
}

fn noise(tc: &hegel::TestCase, depth: u32) -> String {
    match tc.draw(generators::integers::<u8>().max_value(7)) {
        0 => "; (yinz-declare-pane)\n".to_owned(),
        1 => "#| (yinz-declare-pane) |#".to_owned(),
        2 => format!("#; {}", datum(tc, depth + 1)),
        3 => "\n".to_owned(),
        4 => "#| outer #| (yinz-declare-pane) |# still a comment |#".to_owned(),
        5 => "#||#".to_owned(),
        _ => " ".to_owned(),
    }
}

fn atom(tc: &hegel::TestCase) -> String {
    match tc.draw(generators::integers::<u8>().max_value(3)) {
        0 => return string_literal(tc),
        // A here-string runs to its delimiter, so its body may look like anything.
        1 => {
            let body = tc.draw(generators::sampled_from(vec![
                "(yinz-declare-pane)",
                "\" unbalanced ( [ {",
                "",
                "EN",
            ]));
            let delimiter = tc.draw(generators::sampled_from(vec!["END", "E", "\r\rE\r"]));
            let close = delimiter.replace('\r', "");
            return format!("#<<{delimiter}\n{body}{close}");
        },
        _ => {},
    }
    tc.draw(generators::sampled_from(vec![
        "x",
        "1",
        "-1",
        "+1",
        ".5",
        "1/2",
        "1e3",
        "1+2i",
        "+i",
        "1@2",
        "#x1F",
        "#b101",
        "+inf.0",
        "-nan.0",
        "...",
        "+",
        "-",
        "+x",
        "+yinz-declare-pane",
        "-yinz-declare-pane",
        "1yinz",
        "#t",
        "#false",
        "#:key",
        "#\\(",
        "#\\)",
        "#\\;",
        "#\\\"",
        "#\\|",
        "#\\space",
        "#\\NEWLINE",
        "#\\x41",
        "#\\u{3bb}",
        "#\\x",
        "#\\λ",
        "a#|b",
        "a|b",
        "a\\ b",
        "|a b|",
        "|a\\|b|",
        "|a\\x41;b|",
        "||",
        "|yinz-declare-pane|",
        "|yinz-declare\\x2d;pane|",
        "#u8(1 2)",
        "#(x)",
        "define",
        "quote",
        DECLARE,
        DECLARE_UNLESS,
    ]))
    .to_owned()
}

/// A list headed by a declaration name, with arguments that may or may not be valid.
fn declaration_form(tc: &hegel::TestCase, depth: u32) -> String {
    let (open, close) = tc.draw(generators::sampled_from(vec![
        ("(", ")"),
        ("[", "]"),
        ("{", "}"),
    ]));
    let mut form = open.to_owned();
    for _ in 0..tc.draw(generators::integers::<u8>().max_value(2)) {
        form.push_str(&noise(tc, depth));
    }
    form.push_str(tc.draw(generators::sampled_from(vec![
        DECLARE,
        DECLARE,
        DECLARE_UNLESS,
        DECLARE_UNLESS,
        "|yinz-declare-pane|",
        "|yinz-declare-pane-unless|",
        "|yinz-declare-\\x70;ane|",
        // Read as `+` and then the name: not a declaration's head.
        "+yinz-declare-pane",
    ])));
    for _ in 0..tc.draw(generators::integers::<u8>().max_value(3)) {
        form.push(' ');
        if tc.draw(generators::booleans()) {
            form.push_str(&noise(tc, depth));
            form.push(' ');
        }
        form.push_str(&if tc.draw(generators::booleans()) {
            string_literal(tc)
        } else {
            datum(tc, depth + 1)
        });
    }
    form.push_str(close);
    form
}

fn datum(tc: &hegel::TestCase, depth: u32) -> String {
    if depth > 3 {
        return atom(tc);
    }
    match tc.draw(generators::integers::<u8>().max_value(9)) {
        0 | 1 => atom(tc),
        2 | 3 => declaration_form(tc, depth),
        4 => {
            let prefix = tc.draw(generators::sampled_from(vec![
                "'", "`", ",", ",@", "#'", "#`", "#,", "#,@",
            ]));
            format!("{prefix}{}", datum(tc, depth + 1))
        },
        5 => format!("(begin {})", datum(tc, depth + 1)),
        6 => format!("(when #t {})", datum(tc, depth + 1)),
        7 => {
            let (open, close) = tc.draw(generators::sampled_from(vec![
                ("#(", ")"),
                ("{", "}"),
                ("[", "]"),
            ]));
            format!("{open}{}{close}", datum(tc, depth + 1))
        },
        8 => {
            let inner = declaration_form(tc, depth);
            format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""))
        },
        _ => {
            let items = (0..tc.draw(generators::integers::<u8>().max_value(3)))
                .map(|_| datum(tc, depth + 1))
                .collect::<Vec<_>>();
            format!("({})", items.join(" "))
        },
    }
}

/// Text that Steel's reader rejects wherever it appears outside a string or comment, and some
/// that only unbalances the source.
fn damage(tc: &hegel::TestCase) -> &'static str {
    tc.draw(generators::sampled_from(vec![
        "\"\\d+\"",
        "\"unterminated",
        "#| unterminated",
        "|unterminated",
        "#\\xZZ",
        "#\\abc",
        "#\\",
        "1/0",
        "+5/0",
        "##",
        "\"\\x41\"",
        "\"\\u{110000}\"",
        "\"\\  x\"",
        "#<<END\nno end",
        "#<< a\n a",
        "#t'|x|",
        ")",
        "(",
        "]",
        "'",
        "#;",
    ]))
}

/// Whether Steel's lexer rejects some token of `source`.
fn lexer_rejects(source: &str) -> bool {
    TokenStream::new(source, false, None).any(|token| token.is_err())
}

#[hegel::test(test_cases = 1000)]
fn scan_agrees_with_a_full_parse(tc: hegel::TestCase) {
    let mut source = String::new();
    for _ in 0..tc.draw(generators::integers::<u8>().max_value(6)) {
        // Stacked prefixes at the top level exercise which datum each one applies to.
        for _ in 0..tc.draw(generators::integers::<u8>().max_value(2)) {
            source.push_str(tc.draw(generators::sampled_from(vec!["'", "#;", "`"])));
            source.push(' ');
        }
        source.push_str(&if tc.draw(generators::booleans()) {
            declaration_form(&tc, 0)
        } else {
            datum(&tc, 0)
        });
        source.push_str(&noise(&tc, 0));
    }
    // Trailing prefixes need a datum to apply to for the source to parse.
    source.push_str(" 0");
    if tc.draw(generators::booleans()) {
        // Damage anywhere: inside a string or comment it may be harmless.
        let boundaries = source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
            .collect::<Vec<_>>();
        let at = tc.draw(generators::sampled_from(boundaries));
        source.insert_str(at, damage(&tc));
    }
    tc.note(&format!("source: {source:?}"));
    let scanned = pane_declaration(&source).map_err(|_| ());
    // Steel's parser trips its own debug assertion on some quote placements; such source is
    // no reference.
    let Ok(parsed) = std::panic::catch_unwind(|| parsed(&source)) else {
        return;
    };
    if lexer_rejects(&source) {
        // Evaluation reports the lexer's error, which the scan must not hide behind its own.
        assert!(parsed.is_err(), "Steel parsed what its lexer rejects");
        assert!(scanned.is_ok(), "a declaration error for unreadable source");
        return;
    }
    match parsed {
        Ok(expected) => assert_eq!(scanned, expected),
        // Unbalanced brackets, or a prefix with nothing to apply to: the reader's errors too.
        Err(
            ParseError::MismatchedParen(..)
            | ParseError::UnexpectedEOF(..)
            | ParseError::UnexpectedChar(..),
        ) => assert!(scanned.is_ok(), "a declaration error for unreadable source"),
        Err(ParseError::SyntaxError(message, ..))
            if message == "unfinished commented-out expression" =>
        {
            assert!(scanned.is_ok(), "a declaration error for unreadable source")
        },
        // Malformed special forms are the evaluator's to report.
        Err(_) => {},
    }
}

#[test]
fn reader_syntax_ends_tokens_where_steel_does() {
    for source in [
        // A `|..|` symbol is the name between its bars, escapes applied.
        "(|yinz-declare-pane|)",
        "(|yinz-declare\\x2d;pane|)",
        "(yinz-declare-pane #;#\\) #;#\\()",
        // Steel reads `+name` as `+` and then `name`: a datum comment can take the `+`.
        "(#; +yinz-declare-pane)",
        "#<<END\n(yinz-declare-pane) \" ( END (yinz-declare-pane)",
        "#<<E\r\r\n)E (yinz-declare-pane)",
        "#| #| (yinz-declare-pane) |# |# (yinz-declare-pane)",
        "\"\\\"(yinz-declare-pane)\\\\\" (yinz-declare-pane)",
        "\"a \\\n   b\" (yinz-declare-pane)",
        "(define c #\\x41) (define d #\\u{3bb}) (yinz-declare-pane)",
        "#u8(1 2) (yinz-declare-pane)",
        "(yinz-declare-pane #;1/0x)",
        "a#|b (yinz-declare-pane)",
    ] {
        assert_eq!(
            pane_declaration(source).unwrap_or_else(|error| panic!("{source:?}: {error}")),
            PaneDeclaration::Always,
            "{source:?}"
        );
        assert_eq!(
            reference(source),
            Some(Ok(PaneDeclaration::Always)),
            "{source:?}"
        );
    }
    for source in [
        "(+yinz-declare-pane)",
        "(|yinz-declare-pane |)",
        "#<<END\n(yinz-declare-pane)\nEND",
        "#(yinz-declare-pane)",
        "(yinz-declare-pane\\ x)",
    ] {
        assert_eq!(scan(source), PaneDeclaration::None, "{source:?}");
        assert_eq!(
            reference(source),
            Some(Ok(PaneDeclaration::None)),
            "{source:?}"
        );
    }
    assert_eq!(
        scan("(yinz-declare-pane-unless \"\\x6b;\\u{65}y\" \"v\\\n  al\")"),
        unless("key", "val")
    );
    // `+x` is two datums: a second argument after the key.
    assert!(pane_declaration("(yinz-declare-pane-unless \"k\" +v)")
        .unwrap_err()
        .to_string()
        .contains("two string literals"));
    assert!(pane_declaration("(yinz-declare-pane +)")
        .unwrap_err()
        .to_string()
        .contains("takes no arguments"));
}

/// Steel's lexer stops at its first error without resynchronising, so what follows can read as
/// anything. Its parser then rejects the source, and evaluation reports that error: the scan
/// must not report a declaration error of its own in its place. It keeps what the complete forms
/// before the bad text declare, so that a plugin with a pane keeps it through a syntax error.
#[test]
fn unreadable_source_reports_no_declaration_error() {
    let always = PaneDeclaration::Always;
    let none = PaneDeclaration::None;
    for (source, expected) in [
        // An invalid escape: the rest of the string would lex as code, and its closing quote
        // open a string, leaving a second declaration at the top level.
        (
            "(yinz-declare-pane)\n(define re \"\\d\")\n(define a \")\")\n(define b \"(yinz-declare-pane)\")",
            &always,
        ),
        ("(yinz-declare-pane) (define (broken syntax", &always),
        ("(yinz-declare-pane) (yinz-declare-pane] (yinz-declare-pane)", &always),
        ("(yinz-declare-pane))", &always),
        ("(yinz-declare-pane) '", &always),
        ("(yinz-declare-pane) #;", &always),
        // The second form never ends.
        ("(yinz-declare-pane) (yinz-declare-pane #;)", &always),
        ("(define (broken \"\\d\")) (yinz-declare-pane)", &none),
        ("(yinz-declare-pane \"\\d\")", &none),
        // A mistake before the bad text is not reported either; it declares nothing.
        ("(yinz-declare-pane #t) \"unterminated", &none),
        ("(yinz-declare-pane-unless \"a\") (", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) #| unterminated", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) )", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) '", &none),

        ("(yinz-declare-pane) (yinz-declare-pane) #\\xZZ", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) 1/0", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) |unterminated", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) #<<END\nnever ends", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) ##", &none),
        ("(yinz-declare-pane) (yinz-declare-pane) \"\\x41\"", &none),
    ] {
        assert_eq!(
            pane_declaration(source).unwrap_or_else(|error| panic!("{source:?}: {error}")),
            *expected,
            "{source:?}"
        );
        assert!(reference(source).is_none(), "Steel accepts {source:?}");
        // A caller comparing declarations is told the declaration is only a guess.
        assert_eq!(readable_pane_declaration(source).unwrap(), None, "{source:?}");
    }
    assert_eq!(
        readable_pane_declaration("(yinz-declare-pane)").unwrap(),
        Some(PaneDeclaration::Always)
    );
    assert_eq!(
        readable_pane_declaration("(define x 1)").unwrap(),
        Some(PaneDeclaration::None)
    );
    assert!(readable_pane_declaration("(yinz-declare-pane #t)").is_err());
}

/// The server scans every plugin it loads. Steel's lexer would intern each identifier and
/// string literal into a table the process never frees; the scan must touch no part of it.
#[test]
fn the_scan_interns_nothing() {
    use steel::parser::interner::{get_interner, InternedString};
    // Make sure the table exists, so that an interning scan would be visible.
    InternedString::from_static("yinz-manifest-interner-probe");
    let interner = get_interner().unwrap();
    let unique = format!(
        "yinz-manifest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let symbol = format!("{unique}-symbol");
    let text = format!("{unique}-string");
    let source = format!(
        "(yinz-declare-pane-unless \"{text}\" \"v\")\n(define ({symbol}) |{symbol}-escaped|)"
    );
    assert_eq!(scan(&source), unless(&text, "v"));
    for name in [&symbol, &text, &format!("{symbol}-escaped")] {
        assert!(interner.get(name.as_str()).is_none(), "{name} was interned");
    }
    // The probe sees an interning lexer.
    TokenStream::new(&source, false, None).for_each(drop);
    assert!(interner.get(symbol.as_str()).is_some());
    assert!(interner.get(text.as_str()).is_some());
}

/// The scan's code must not reach Steel's reader at all, outside its tests.
#[test]
fn the_scan_does_not_use_steels_reader() {
    let source = include_str!("../manifest.rs");
    let code = &source[..source.find("#[cfg(test)]").unwrap()];
    for forbidden in ["steel", "validate_source", "TokenStream"] {
        assert!(!code.contains(forbidden), "manifest.rs uses {forbidden}");
    }
}

/// Inputs that make a naive reader superlinear, each of a few megabytes, with what the scan
/// finds in each, or the error it gives. Steel's lexer took minutes on the first two at this
/// size.
fn adversarial_sources() -> Vec<(
    &'static str,
    String,
    std::result::Result<PaneDeclaration, &'static str>,
)> {
    const SIZE: usize = 4 << 20;
    let declared = |rest: String| format!("(yinz-declare-pane)\n{rest}");
    // Unreadable after the declaration, as evaluation reports.
    let always = Ok(PaneDeclaration::Always);
    let nesting = || Err("Steel source nesting exceeds 128");
    let tokens = || Err("Steel source exceeds token limit");
    vec![
        (
            "a here-string delimiter that never appears",
            declared(format!(
                "(define s #<<{}b\n{})",
                "a".repeat(SIZE / 4),
                "a".repeat(SIZE)
            )),
            always.clone(),
        ),
        (
            "a here-string delimiter of carriage returns",
            declared(format!(
                "(define s #<<{}b\n{})",
                "a\r".repeat(SIZE / 8),
                "a".repeat(SIZE)
            )),
            always.clone(),
        ),
        (
            "a long here-string delimiter",
            declared(format!(
                "(define s #<<{delimiter}\n{}{delimiter})",
                "a".repeat(SIZE),
                delimiter = "a".repeat(SIZE / 4) + "b",
            )),
            always.clone(),
        ),
        (
            "an integer",
            declared(format!("(define n {})", "7".repeat(SIZE))),
            always.clone(),
        ),
        (
            "a rational",
            declared(format!(
                "(define n {}/{})",
                "7".repeat(SIZE / 2),
                "3".repeat(SIZE / 2)
            )),
            always.clone(),
        ),
        (
            "a zero denominator",
            declared(format!("(define n 1/{})", "0".repeat(SIZE))),
            always.clone(),
        ),
        (
            "a float",
            declared(format!(
                "(define n {}.{}e5)",
                "7".repeat(SIZE / 2),
                "3".repeat(SIZE / 2)
            )),
            always.clone(),
        ),
        (
            "a hex escape",
            declared(format!("(define s \"\\x{};\")", "1".repeat(SIZE))),
            always.clone(),
        ),
        (
            "a character",
            declared(format!("(define c #\\x{})", "1".repeat(SIZE))),
            always.clone(),
        ),
        (
            "deep nesting",
            declared(format!("{}{}", "(".repeat(SIZE / 2), ")".repeat(SIZE / 2))),
            nesting(),
        ),
        (
            "deep nesting under prefixes",
            declared(format!("{}{}", "'(".repeat(SIZE / 4), ")".repeat(SIZE / 4))),
            nesting(),
        ),
        (
            "stacked quotes",
            declared(format!("{}x", "'".repeat(SIZE))),
            nesting(),
        ),
        (
            "nested block comments",
            declared(format!(
                "{}{}",
                "#|".repeat(SIZE / 4),
                "|#".repeat(SIZE / 4)
            )),
            always.clone(),
        ),
        ("many comments", declared(";\n".repeat(SIZE / 2)), tokens()),
        ("many forms", declared("()\n".repeat(SIZE / 3)), tokens()),
        (
            "many declarations",
            "(yinz-declare-pane-unless \"k\" \"v\")\n".repeat(SIZE / 36),
            tokens(),
        ),
        (
            "a symbol",
            declared(format!("|{}|", "a\\|".repeat(SIZE / 3))),
            always.clone(),
        ),
        (
            "a word",
            declared(format!("+{}", "a\\ ".repeat(SIZE / 3))),
            always.clone(),
        ),
        (
            "a string",
            declared(format!("\"{}\"", "\\n".repeat(SIZE / 2))),
            always.clone(),
        ),
        (
            "a comment",
            declared(format!(";{}", "(\"#|".repeat(SIZE / 4))),
            always,
        ),
    ]
}

/// Each adversarial input scans in time proportional to its length. The bound is relative to
/// a plain source of the same size scanned just before, so a busy machine slows both alike; a
/// superlinear scan of inputs this size exceeds it many times over.
#[test]
fn adversarial_sources_scan_in_linear_time() {
    let time = |source: &str| {
        let started = std::time::Instant::now();
        let declaration = pane_declaration(source);
        (started.elapsed(), declaration)
    };
    // Within the token limit, which a source this size of short forms would exceed.
    let plain = |length: usize| {
        let form = format!("(define (f x) \"{}\")\n", "a b ".repeat(64));
        form.repeat(length / form.len())
    };
    for (name, source, expected) in adversarial_sources() {
        let (baseline, plain_declaration) = time(&plain(source.len()));
        assert_eq!(plain_declaration.unwrap(), PaneDeclaration::None);
        let (elapsed, declaration) = time(&source);
        let bound = baseline * 20 + std::time::Duration::from_secs(1);
        assert!(
            elapsed <= bound,
            "{name}: {elapsed:?} for {} bytes; a plain source took {baseline:?}",
            source.len()
        );
        match expected {
            Ok(expected) => assert_eq!(declaration.unwrap(), expected, "{name}"),
            Err(message) => {
                let error = declaration.unwrap_err().to_string();
                assert!(error.contains(message), "{name}: {error}");
            },
        }
    }
}

/// The scan refuses what Steel's own check of a source refuses for its size, tokens or nesting,
/// with the same words, and nothing else: the plugin then fails to load with that error rather
/// than being given up as one that declares no pane.
#[test]
fn the_scan_enforces_steels_source_limits_exactly() {
    let at_most = |count: usize, unit: &str| unit.repeat(count);
    let cases = [
        format!("{}0{}", at_most(128, "("), at_most(128, ")")),
        format!("{}0{}", at_most(129, "("), at_most(129, ")")),
        format!("{}x", at_most(128, "'")),
        format!("{}x", at_most(129, "'")),
        format!("{}0{}", at_most(64, "'("), at_most(64, ")")),
        format!("{}0{}", at_most(65, "'("), at_most(65, ")")),
        format!("{}0{}", at_most(42, "#;'("), at_most(42, ")")),
        format!("{}0{}", at_most(43, "#;'("), at_most(43, ")")),
        // Comments neither add to nor reset the waiting prefixes.
        format!("{}x", at_most(128, "' ; c\n#| c |#")),
        format!("{}x", at_most(129, "' ; c\n#| c |#")),
        // Lists that close give their nesting back.
        at_most(1000, "(((0)))"),
        at_most(500_000, "a "),
        at_most(500_001, "a "),
        at_most(250_000, "; c\na "),
        format!("{} ; c", at_most(499_999, "a ")),
        format!("{} ; c", at_most(500_000, "a ")),
        // `+x` is two tokens.
        at_most(250_000, "+x "),
        at_most(250_001, "+x "),
        format!(
            "(yinz-declare-pane){}",
            " ".repeat(crate::MAX_SOURCE_BYTES - 19)
        ),
        format!(
            "(yinz-declare-pane){}",
            " ".repeat(crate::MAX_SOURCE_BYTES - 18)
        ),
    ];
    for source in &cases {
        let shown = &source[..source.len().min(40)];
        match crate::validate_source(source) {
            Ok(()) => assert!(pane_declaration(source).is_ok(), "{shown:?}"),
            Err(expected) => {
                let error = pane_declaration(source).unwrap_err();
                assert_eq!(error.to_string(), expected.to_string(), "{shown:?}");
            },
        }
    }
    // Both sides of each limit appear.
    let refused = cases
        .iter()
        .filter(|source| crate::validate_source(source).is_err())
        .count();
    assert_eq!(refused, cases.len() / 2 - 1);
}
