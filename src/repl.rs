use yinz_cli::args::CliArgs;
use yinz_runtime::errors::prelude::*;
use yinz_steel::{repl::Evaluator, EvalOutput};

pub(crate) fn run(options: &CliArgs, expression: Option<&str>) -> Result<()> {
    use std::io::IsTerminal;
    let (session, caller) = crate::commands::resolve_session_for_command(
        options.session.clone(),
        "connect the REPL to",
    );
    let mut runtime = SessionEvaluator {
        client: yinz_client::repl::Client::connect(&session, caller)?,
        output: String::new(),
    };
    if expression.is_none() && std::io::stdin().is_terminal() {
        eprintln!("Connected to Yinz session {session}.");
    }
    let result = yinz_steel::repl::run_with_evaluator(&mut runtime, expression);
    let close = runtime.client.close();
    result.and(close)
}

struct SessionEvaluator {
    client: yinz_client::repl::Client,
    output: String,
}

impl Evaluator for SessionEvaluator {
    fn eval(&mut self, source: &str) -> Result<EvalOutput> {
        self.output.clear();
        let reply = self.client.eval(source)?;
        self.output = reply
            .get("output")
            .and_then(serde_json::Value::as_str)
            .context("REPL reply has no output string")?
            .to_owned();
        if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
            bail!("{error}");
        }
        let result = serde_json::from_value(reply).context("decode REPL values")?;
        self.output.clear();
        Ok(result)
    }

    fn take_output(&mut self) -> Result<String> {
        Ok(std::mem::take(&mut self.output))
    }
}
