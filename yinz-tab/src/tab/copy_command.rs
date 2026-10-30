use std::io::prelude::*;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use yinz_runtime::envs::{child_session_env, SessionState};
use yinz_runtime::errors::prelude::*;

type TransferCompletion = Arc<Mutex<mpsc::Receiver<()>>>;

// Tabs and reconfigured commands still write to the same clipboard.
static LAST_TRANSFER: Mutex<Option<TransferCompletion>> = Mutex::new(None);

pub struct CopyCommand {
    command: String,
    args: Vec<String>,
    session: SessionState,
}

impl CopyCommand {
    pub fn new(command: String, session: SessionState) -> Self {
        let mut command_with_args = command.split(' ').map(String::from);

        Self {
            command: command_with_args.next().expect("missing command"),
            args: command_with_args.collect(),
            session,
        }
    }

    pub fn set(
        &self,
        value: String,
        on_complete: impl FnOnce(Result<()>) + Send + 'static,
    ) -> Result<JoinHandle<()>> {
        let command = self.command.clone();
        let args = self.args.clone();
        let session = self.session.clone();
        let mut last_transfer = LAST_TRANSFER
            .lock()
            .map_err(|_| anyhow!("clipboard transfer queue poisoned"))?;
        let previous = last_transfer.clone();
        let (complete, completed) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                if let Some(previous) = previous {
                    // Disconnection releases the next transfer, including on error or panic.
                    let _ = previous.lock().unwrap_or_else(|e| e.into_inner()).recv();
                }
                let mut process = match Command::new(&command)
                    .args(args)
                    .envs(child_session_env(&session.current()))
                    .stdin(Stdio::piped())
                    .spawn()
                    .with_context(|| format!("couldn't spawn {command}"))
                {
                    Ok(process) => process,
                    Err(error) => {
                        on_complete(Err(error));
                        return;
                    },
                };
                // shortcut: stalled transfers retain a worker; add cancellation if limits are needed.
                let write_result = process
                    .stdin
                    .take()
                    .context("could not get stdin")
                    .and_then(|mut stdin| {
                        stdin
                            .write_all(value.as_bytes())
                            .with_context(|| format!("couldn't write to {command} stdin"))
                    });
                drop(value);
                on_complete(write_result);
                drop(complete);

                // Clipboard owners such as xclip must survive completion; killing them clears it.
                process.wait().context("Clipboard failure").non_fatal();
            })
            .context("couldn't start clipboard worker")?;
        *last_transfer = Some(Arc::new(Mutex::new(completed)));
        Ok(worker)
    }
}

#[cfg(all(test, unix))]
#[path = "unit/copy_command.rs"]
mod tests;
