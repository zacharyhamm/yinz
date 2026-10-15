//! The session REPL accepts evaluation only from the CLI pipe transport.
use super::*;
use yinz_types::data::PipeSource;
use yinz_types::input::layout::RunPluginLocation;

impl SteelPlugin {
    pub(super) fn repl_pipe(&mut self, message: &PipeMessage) -> Result<Option<bool>> {
        if !matches!(&self.env().plugin_config.location, RunPluginLocation::Yinz(tag) if tag.to_string() == "repl")
            || !matches!(
                message.name.as_str(),
                "__yinz_repl_eval" | "__yinz_repl_close"
            )
        {
            return Ok(None);
        }
        let PipeSource::Cli(pipe) = &message.source else {
            return Ok(Some(false));
        };
        if let Some(bound) = &self.repl_pipe {
            anyhow::ensure!(bound == pipe, "REPL belongs to a different CLI pipe");
        } else {
            self.repl_pipe = Some(pipe.clone());
        }
        let close = message.name == "__yinz_repl_close";
        let reply = if close {
            serde_json::json!({"values": [], "output": ""})
        } else {
            let source = message.payload.as_deref().unwrap_or_default();
            self.with_runtime(true, |runtime| {
                let mut reply = match runtime.eval(source) {
                    Ok(result) => serde_json::to_value(result).context("encoding REPL result")?,
                    Err(error) => serde_json::json!({
                        "values": [],
                        "output": runtime.take_output().unwrap_or_default(),
                        "error": format!("{error:#}"),
                    }),
                };
                if let Err(error) = runtime.invoke("yinz-async-poll", vec![]) {
                    reply["error"] = Value::String(format!("{error:#}"));
                }
                for output in [runtime.take_output(), runtime.take_logs()] {
                    let output = output.unwrap_or_default();
                    if !output.is_empty() {
                        reply["output"] = Value::String(format!(
                            "{}{output}",
                            reply["output"].as_str().unwrap_or_default(),
                        ));
                    }
                }
                Ok(reply)
            })?
        };
        let output = serde_json::to_string(&reply).context("encoding REPL pipe response")?;
        dispatch(
            self.env_mut(),
            serde_json::to_value(PluginCommand::CliPipeOutput(pipe.clone(), output))?,
        )?;
        dispatch(
            self.env_mut(),
            serde_json::to_value(PluginCommand::UnblockCliPipeInput(pipe.clone()))?,
        )?;
        if close {
            dispatch(
                self.env_mut(),
                serde_json::to_value(PluginCommand::CloseSelf)?,
            )?;
        }
        Ok(Some(false))
    }
}
