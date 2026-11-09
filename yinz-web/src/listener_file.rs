//! Private listener records, ownership-safe updates and bounded local status probes.
use crate::auth::password::{atomic_write, lock, private_dir, random_id, sync_dir};
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
    #[serde(default)]
    pub startup_config: Option<yinz_types::input::web::WebConfig>,
}
pub(crate) struct OwnedRecord {
    dir: PathBuf,
    name: String,
    instance: String,
    address: SocketAddr,
    reservation: (u64, u64),
    startup_config: Option<yinz_types::input::web::WebConfig>,
}
impl OwnedRecord {
    pub(crate) fn create(
        data_dir: &std::path::Path,
        address: SocketAddr,
        session: String,
        startup_config: Option<yinz_types::input::web::WebConfig>,
    ) -> Result<Self> {
        let web = private_dir(data_dir)?;
        let _lock = lock(&web)?;
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
            startup_config,
        };
        let published = owned.refresh_locked(session);
        drop(_lock);
        published?;
        Ok(owned)
    }
    pub(crate) fn refresh(&self, session: String) -> Result<()> {
        let _lock = lock(
            self.dir
                .parent()
                .ok_or_else(|| anyhow!("missing web directory"))?,
        )?;
        self.refresh_locked(session)
    }
    fn refresh_locked(&self, session: String) -> Result<()> {
        let path = self.dir.join(&self.name);
        let metadata = std::fs::symlink_metadata(&path).context("inspect listener ownership")?;
        if (metadata.dev(), metadata.ino()) != self.reservation {
            let (current, _) = read_record(&path)?;
            if current.instance != self.instance {
                bail!("listener record ownership changed");
            }
        }
        let record = ListenerRecord {
            version: 1,
            pid: std::process::id(),
            instance: self.instance.clone(),
            address: self.address,
            session,
            startup_config: self.startup_config.clone(),
        };
        let bytes = serde_json::to_vec(&record).context("encode listener record")?;
        if bytes.len() > 65536 {
            bail!("listener record exceeds 65536 bytes");
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
            let _lock = lock(
                self.dir
                    .parent()
                    .ok_or_else(|| anyhow!("missing web directory"))?,
            )?;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)
                .context("open owned listener record")?;
            crate::auth::password::validate_private(&file)?;
            let metadata = file.metadata().context("inspect listener record")?;
            // An unsuccessful first write still owns its empty reservation inode.
            if (metadata.dev(), metadata.ino()) != self.reservation {
                if metadata.size() > 65536 {
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

/// A record inspected by the local CLI. Failed probes never establish staleness.
#[derive(Debug)]
pub struct ObservedListener {
    pub record: ListenerRecord,
    pub responding_session: Option<String>,
}
fn dead(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal zero only tests existence/permission; it sends no signal.
    unsafe {
        libc::kill(pid, 0) != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}
fn signature(m: &std::fs::Metadata) -> (u64, u64, i64, i64, u64) {
    (m.dev(), m.ino(), m.ctime(), m.ctime_nsec(), m.len())
}
fn read_record(path: &std::path::Path) -> Result<(ListenerRecord, std::fs::Metadata)> {
    use std::io::Read;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .context("open listener record")?;
    crate::auth::password::validate_private(&file)?;
    let m = file.metadata().context("inspect listener record")?;
    if m.len() > 65536 {
        bail!("listener record too large");
    }
    let mut bytes = vec![];
    file.take(65537)
        .read_to_end(&mut bytes)
        .context("read listener record")?;
    if bytes.len() > 65536 {
        bail!("listener record too large");
    }
    let r: ListenerRecord =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid listener record"))?;
    if r.version != 1
        || r.instance.len() != 32
        || !r.instance.bytes().all(|b| b.is_ascii_hexdigit())
        || r.session.len() > 4096
    {
        bail!("invalid listener record");
    }
    Ok((r, m))
}
/// Scan at most 256 private records; remove only records with independently proven dead PIDs.
pub fn inspect(data: &std::path::Path) -> Result<Vec<ObservedListener>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    inspect_with(
        data,
        |address| {
            if std::time::Instant::now() < deadline {
                probe(address)
            } else {
                None
            }
        },
        dead,
    )
}
fn inspect_with(
    data: &std::path::Path,
    mut probe: impl FnMut(SocketAddr) -> Option<String>,
    mut dead: impl FnMut(u32) -> bool,
) -> Result<Vec<ObservedListener>> {
    let web = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(data.join("web"));
    let web = match web {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).context("open web directory"),
    };
    let m = web.metadata().context("inspect web directory")?;
    // SAFETY: geteuid has no preconditions.
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        bail!("unsafe web directory");
    }
    let dir = data.join("web/listeners");
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(&dir)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).context("open listeners directory"),
    };
    let m = directory
        .metadata()
        .context("inspect listeners directory")?;
    // SAFETY: geteuid has no preconditions.
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        bail!("unsafe listeners directory");
    }
    let mut result = vec![];
    for entry in std::fs::read_dir(&dir)
        .context("scan listener records")?
        .take(256)
    {
        let path = entry.context("inspect listener entry")?.path();
        if path.extension().is_none_or(|s| s != "json") {
            continue;
        }
        let Ok((record, metadata)) = read_record(&path) else {
            continue;
        };
        let responding_session = probe(record.address);
        if responding_session.is_none() && dead(record.pid) {
            let _lock = lock(&data.join("web"))?;
            if let Ok((current, m)) = read_record(&path)
                && signature(&metadata) == signature(&m)
                && current.instance == record.instance
                && dead(current.pid)
            {
                std::fs::remove_file(&path).context("remove stale listener record")?;
                sync_dir(&dir)?;
                continue;
            }
        }
        result.push(ObservedListener {
            record,
            responding_session,
        });
    }
    Ok(result)
}
fn probe(mut address: SocketAddr) -> Option<String> {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    if !address.ip().is_loopback() {
        return None;
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut socket =
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    socket
        .set_write_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    socket
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut bytes = vec![];
    loop {
        if Instant::now() >= deadline || bytes.len() > 16384 {
            return None;
        }
        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200 ")
        || head.to_ascii_lowercase().contains("transfer-encoding:")
    {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    if json.get("ok")?.as_bool() != Some(true) {
        return None;
    }
    json.get("session")?.as_str().map(str::to_owned)
}
