//! One private, owned record per bind. Records belonging to other servers are never scanned.
use crate::auth::password::{atomic_write, private_dir, random_id, sync_dir};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};
use yinz_runtime::errors::prelude::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListenerRecord {
    pub version: u32,
    pub pid: u32,
    pub instance: String,
    pub address: SocketAddr,
    pub session: String,
}
pub(crate) struct OwnedRecord {
    dir: PathBuf,
    name: String,
    instance: String,
    address: SocketAddr,
    reservation: (u64, u64),
}
impl OwnedRecord {
    pub(crate) fn create(
        data_dir: &std::path::Path,
        address: SocketAddr,
        session: String,
    ) -> Result<Self> {
        let dir = private_dir(data_dir)?.join("listeners");
        yinz_runtime::shared::ensure_secure_dir(&dir).context("secure web listeners directory")?;
        let instance = random_id()?;
        let suffix =
            u64::from_str_radix(&instance[..16], 16).context("allocate listener record name")?;
        let name = format!("{}-{suffix}.json", std::process::id());
        // Reserve without replacing an existing record, even if a random name collides.
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join(&name))
            .context("reserve listener record")?;
        let metadata = file.metadata().context("inspect listener reservation")?;
        let reservation = (metadata.dev(), metadata.ino());
        drop(file);
        let owned = Self {
            dir,
            name,
            instance,
            address,
            reservation,
        };
        owned.refresh(session)?;
        Ok(owned)
    }
    pub(crate) fn refresh(&self, session: String) -> Result<()> {
        let record = ListenerRecord {
            version: 1,
            pid: std::process::id(),
            instance: self.instance.clone(),
            address: self.address,
            session,
        };
        let bytes = serde_json::to_vec(&record).context("encode listener record")?;
        if bytes.len() > 4096 {
            bail!("listener record exceeds 4096 bytes");
        }
        atomic_write(&self.dir, &self.name, &bytes)
    }
}
impl Drop for OwnedRecord {
    fn drop(&mut self) {
        // Directory is private. Check the instance as well so an externally replaced record
        // is never deleted by an old generation's cleanup.
        let path = self.dir.join(&self.name);
        let result = (|| -> Result<()> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)
                .context("open owned listener record")?;
            crate::auth::password::validate_private(&file)?;
            let metadata = file.metadata().context("inspect listener record")?;
            // An unsuccessful first write still owns its empty reservation inode.
            if (metadata.dev(), metadata.ino()) != self.reservation {
                if metadata.size() > 4096 {
                    bail!("listener record is too large");
                }
                let record: ListenerRecord =
                    serde_json::from_reader(file).context("read owned listener record")?;
                if record.instance != self.instance {
                    bail!("listener record ownership changed");
                }
            }
            std::fs::remove_file(path).context("remove owned listener record")?;
            sync_dir(&self.dir)
        })();
        result.context("clean up web listener record").non_fatal();
    }
}

#[cfg(test)]
#[path = "unit/listener_file.rs"]
mod tests;
