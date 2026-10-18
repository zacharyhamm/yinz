use super::*;
use anyhow::anyhow;
use std::io::Cursor;

fn runtime() -> Runtime {
    Runtime::new(|_| Err(anyhow!("no session"))).unwrap()
}

#[test]
fn interactive_repl_preserves_definitions_and_recovers_from_errors() {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    run_with_io(
        &mut runtime(),
        None,
        true,
        &mut Cursor::new("(define answer\n  42)\nmissing-name\n(+ answer 1)\n:quit\n"),
        &mut output,
        &mut errors,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("...> "), "{output}");
    assert!(output.contains("43\n"), "{output}");
    assert!(String::from_utf8(errors).unwrap().contains("missing-name"));
}

#[test]
fn piped_program_has_no_prompt_and_failure_is_reported() {
    let mut output = Vec::new();
    run_with_io(
        &mut runtime(),
        None,
        false,
        &mut Cursor::new("(define answer 6)\n(* answer 7)\n"),
        &mut output,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
    assert!(run_with_io(
        &mut runtime(),
        None,
        false,
        &mut Cursor::new("(error \"failed\")"),
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .is_err());
}

#[test]
fn expression_does_not_consume_stdin_and_keeps_output_before_failure() {
    let mut input = Cursor::new("unread input");
    let mut output = Vec::new();
    assert!(run_with_io(
        &mut runtime(),
        Some("(display \"before\") (error \"failed\")"),
        false,
        &mut input,
        &mut output,
        &mut Vec::new(),
    )
    .is_err());
    assert_eq!(input.position(), 0);
    assert_eq!(output, b"before");
}

#[test]
fn incomplete_expression_at_eof_is_an_error() {
    assert!(run_with_io(
        &mut runtime(),
        None,
        true,
        &mut Cursor::new("(+ 1\n"),
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .is_err());
}

#[test]
fn diagnostics_go_to_stderr_even_when_evaluation_fails() {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    assert!(run_with_io(
        &mut runtime(),
        Some("(yinz-log \"diagnostic\") (display \"value\") (error \"failed\")"),
        false,
        &mut Cursor::new(""),
        &mut output,
        &mut errors
    )
    .is_err());
    assert_eq!(output, b"value");
    assert_eq!(errors, b"diagnostic\n");
}
