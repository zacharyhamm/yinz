//! Durable per-run recovery offers. Acknowledgements name the run they dismiss.
use crate::errors::prelude::*;
use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub name: String,
    pub run: String,
    pub age: std::time::Duration,
}

fn publish(directory: &Path, name: &str, value: &str) -> Result<()> {
    fs::create_dir_all(directory).context("create recovery directory")?;
    if let Some(parent) = directory.parent() {
        File::open(parent)
            .context("open recovery parent")?
            .sync_all()
            .context("sync recovery parent")?;
    }
    let temporary = directory.join(format!(".recovery-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&temporary).context("create recovery record")?;
        file.write_all(value.as_bytes())
            .context("write recovery record")?;
        file.sync_all().context("sync recovery record")?;
        fs::rename(&temporary, directory.join(name)).context("publish recovery record")?;
        File::open(directory)
            .context("open recovery directory")?
            .sync_all()
            .context("sync recovery directory")
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn begin(directory: &Path) -> Result<String> {
    let run = uuid::Uuid::new_v4().to_string();
    publish(directory, "recovery-run", &run)?;
    Ok(run)
}

pub fn acknowledge(directory: &Path, run: &str) -> Result<()> {
    // Writing an old acknowledgement cannot hide a newer run.
    uuid::Uuid::parse_str(run).context("invalid recovery run identifier")?;
    publish(directory, &format!("recovery-ack-{run}"), "")
}

pub fn pending(directory: &Path) -> Result<Option<String>> {
    let run = match fs::read_to_string(directory.join("recovery-run")) {
        Ok(run) => run,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read recovery run"),
    };
    let ack = match fs::read_to_string(directory.join(format!("recovery-ack-{run}"))) {
        Ok(ack) => Some(ack),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("read recovery acknowledgement"),
    };
    Ok((ack.is_none()).then_some(run))
}

#[cfg(test)]
#[path = "unit/session_recovery_tests.rs"]
mod tests;
