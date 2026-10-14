//! The most recently selected project, shared by sessions using a data directory.

use crate::errors::prelude::*;
use std::path::PathBuf;

#[derive(Default)]
pub struct LastProject {
    data_dir: Option<PathBuf>,
    name: Option<String>,
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
        Self {
            data_dir: Some(data_dir),
            name,
        }
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
        self.persist(name).non_fatal();
    }

    fn persist(&self, name: &str) -> Result<()> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(());
        };
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

#[cfg(test)]
#[path = "unit/last_project_tests.rs"]
mod tests;
