use crate::{source_complete, Runtime};
use anyhow::{Context, Result};
use std::io::{self, BufRead, IsTerminal, Read, Write};

const MAX_INPUT_BYTES: usize = crate::MAX_SOURCE_BYTES;

pub trait Evaluator {
    fn eval(&mut self, source: &str) -> Result<crate::EvalOutput>;
    fn take_output(&mut self) -> Result<String>;
    fn take_logs(&mut self) -> Result<String> {
        Ok(String::new())
    }
}

impl Evaluator for Runtime {
    fn eval(&mut self, source: &str) -> Result<crate::EvalOutput> {
        Runtime::eval(self, source)
    }
    fn take_output(&mut self) -> Result<String> {
        Runtime::take_output(self)
    }
    fn take_logs(&mut self) -> Result<String> {
        Runtime::take_logs(self)
    }
}

pub fn run_with_evaluator(runtime: &mut impl Evaluator, expression: Option<&str>) -> Result<()> {
    let stdin = io::stdin();
    let interactive = stdin.is_terminal() && expression.is_none();
    run_with_io(
        runtime,
        expression,
        interactive,
        &mut stdin.lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

fn evaluate(
    runtime: &mut impl Evaluator,
    source: &str,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> Result<()> {
    anyhow::ensure!(source.len() <= MAX_INPUT_BYTES, "REPL input exceeds 8 MiB");
    let result = runtime.eval(source);
    write!(errors, "{}", runtime.take_logs()?).context("write Scheme diagnostics")?;
    errors.flush().context("flush Scheme diagnostics")?;
    match result {
        Ok(result) => {
            write!(output, "{}", result.output).context("write Scheme output")?;
            for value in result.values {
                writeln!(output, "{value}").context("write Scheme value")?;
            }
            output.flush().context("flush Scheme output")
        },
        Err(error) => {
            write!(output, "{}", runtime.take_output()?).context("write Scheme output")?;
            output.flush().context("flush Scheme output")?;
            Err(error)
        },
    }
}

fn run_with_io(
    runtime: &mut impl Evaluator,
    expression: Option<&str>,
    interactive: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> Result<()> {
    if let Some(expression) = expression {
        return evaluate(runtime, expression, output, errors);
    }
    let mut source = String::new();
    if !interactive {
        input
            .take(MAX_INPUT_BYTES as u64 + 1)
            .read_to_string(&mut source)
            .context("read Scheme program from stdin")?;
        return evaluate(runtime, &source, output, errors);
    }
    writeln!(output, "Yinz Steel REPL. Use :quit or Ctrl-D to exit.")
        .context("write REPL greeting")?;
    loop {
        write!(
            output,
            "{}",
            if source.is_empty() { "yinz> " } else { "...> " }
        )
        .context("write REPL prompt")?;
        output.flush().context("flush REPL prompt")?;
        let read = (&mut *input)
            .take((MAX_INPUT_BYTES - source.len()) as u64 + 1)
            .read_line(&mut source)
            .context("read Scheme expression")?;
        anyhow::ensure!(source.len() <= MAX_INPUT_BYTES, "REPL input exceeds 8 MiB");
        if read == 0 {
            if !source.trim().is_empty() {
                evaluate(runtime, &source, output, errors)?;
            }
            return Ok(());
        }
        if source.trim() == ":quit" {
            return Ok(());
        }
        if !source_complete(&source) {
            continue;
        }
        if let Err(error) = evaluate(runtime, &source, output, errors) {
            writeln!(errors, "Error: {error:#}").context("write Scheme error")?;
        }
        source.clear();
    }
}

#[cfg(test)]
#[path = "unit/repl.rs"]
mod tests;
