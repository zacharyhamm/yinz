//! The most recently selected project, shared by sessions using a data directory.

use crate::errors::prelude::*;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::path::PathBuf;
use std::thread::JoinHandle;

#[derive(Default)]
pub struct LastProject {
    name: Option<String>,
    writer: Option<Writer>,
}

struct Writer {
    sender: Sender<String>,
    pending: Receiver<String>,
    thread: JoinHandle<()>,
}

impl Writer {
    fn start(mut persist: impl FnMut(&str) -> Result<()> + Send + 'static) -> Result<Self> {
        let (sender, pending) = bounded::<String>(1);
        let receiver = pending.clone();
        let thread = std::thread::Builder::new()
            .name("last-project".into())
            .spawn(move || {
                for name in receiver {
                    persist(&name).non_fatal();
                }
            })
            .context("start last project writer")?;
        Ok(Self {
            sender,
            pending,
            thread,
        })
    }
}

impl LastProject {
    pub fn load(data_dir: PathBuf) -> Self {
        let name = match Self::read(&data_dir) {
            Ok(name) => name,
            Err(error) => {
                Err::<(), _>(error).non_fatal();
                None
            },
        };
        let writer = Writer::start(move |name| Self::persist(&data_dir, name))
            .to_log()
            .ok();
        Self { name, writer }
    }

    fn read(data_dir: &std::path::Path) -> Result<Option<String>> {
        let contents = match std::fs::read(data_dir.join("last-project.json")) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read last project"),
        };
        serde_json::from_slice(&contents)
            .map(Some)
            .context("parse last project")
    }

    pub fn name(&self) -> Option<&String> {
        self.name.as_ref()
    }

    pub fn remember(&mut self, name: &str) {
        self.name = Some(name.to_owned());
        if let Some(writer) = &self.writer {
            // Only the latest selection matters; disk latency must never hold up tab switching.
            let _ = writer.pending.try_recv();
            writer
                .sender
                .try_send(name.to_owned())
                .context("queue last project save")
                .non_fatal();
        }
    }

    fn persist(data_dir: &std::path::Path, name: &str) -> Result<()> {
        std::fs::create_dir_all(data_dir).context("create last project directory")?;
        let mut staged = tempfile::NamedTempFile::new_in(data_dir).context("stage last project")?;
        serde_json::to_writer(&mut staged, name).context("write last project")?;
        staged.as_file().sync_all().context("sync last project")?;
        staged
            .persist(data_dir.join("last-project.json"))
            .context("replace last project")?;
        Ok(())
    }
}

impl Drop for LastProject {
    fn drop(&mut self) {
        if let Some(Writer {
            sender,
            pending,
            thread,
        }) = self.writer.take()
        {
            drop(sender);
            drop(pending);
            // Finish the final selection before a clean session shutdown returns.
            thread
                .join()
                .map_err(|_| anyhow!("last project writer panicked"))
                .non_fatal();
        }
    }
}

#[cfg(test)]
#[path = "unit/last_project_tests.rs"]
mod tests;
