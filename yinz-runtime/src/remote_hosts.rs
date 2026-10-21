//! SSH destinations remembered after a successful interactive attachment.
use crate::errors::prelude::*;
use std::path::Path;

pub fn load(cache: &Path) -> Result<Vec<String>> {
    let bytes = match std::fs::read(cache.join("remote-hosts.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("read remembered SSH hosts"),
    };
    let mut hosts: Vec<String> =
        serde_json::from_slice(&bytes).context("decode remembered SSH hosts")?;
    hosts.sort();
    hosts.dedup();
    Ok(hosts)
}

pub fn remember(cache: &Path, host: &str) -> Result<()> {
    std::fs::create_dir_all(cache).context("create SSH host cache directory")?;
    // Keep the lock inode so simultaneous writers always lock the same file.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join(".remote-hosts.lock"))
        .context("open SSH host cache lock")?;
    lock.lock().context("lock SSH host cache")?;
    // A corrupt cache is preserved for recovery rather than silently overwritten.
    let mut hosts = load(cache)?;
    if hosts.iter().any(|known| known == host) {
        return Ok(());
    }
    hosts.push(host.to_owned());
    hosts.sort();
    let mut staged = tempfile::NamedTempFile::new_in(cache).context("stage SSH host cache")?;
    serde_json::to_writer(&mut staged, &hosts).context("encode SSH host cache")?;
    staged.as_file().sync_all().context("sync SSH host cache")?;
    staged
        .persist(cache.join("remote-hosts.json"))
        .context("replace SSH host cache")?;
    Ok(())
}

#[cfg(test)]
#[path = "unit/remote_hosts.rs"]
mod tests;
