use crate::args::{CliArgs, Command};
use clap::Parser;

#[test]
fn repl_accepts_program_or_interactive_input() {
    // Clap's large command tree needs the same stack as the other CLI parser tests.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(check_repl_arguments)
        .unwrap()
        .join()
        .unwrap();
}

fn check_repl_arguments() {
    for flag in ["--eval", "-e"] {
        let args = CliArgs::try_parse_from(["yinz", "repl", flag, "(+ 1 2)"]).unwrap();
        assert!(
            matches!(args.command, Some(Command::Repl { eval: Some(source), connect: false }) if source == "(+ 1 2)")
        );
    }
    let args = CliArgs::try_parse_from(["yinz", "repl"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Repl {
            eval: None,
            connect: false
        })
    ));
    let args = CliArgs::try_parse_from(["yinz", "repl", "--connect"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Repl {
            eval: None,
            connect: true
        })
    ));
}
