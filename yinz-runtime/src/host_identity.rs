//! A stable, opaque identifier for the machine this server runs on.
//!
//! # Why a row needs to say which machine wrote it
//!
//! Servers share `plugin-data.db` within a data directory. Rows carry both session and
//! installation identity because pane ids and session names are not globally unique.
//!
//! [`host_id`] is what makes those rows distinguishable. It identifies an *installation* — a data
//! dir on a machine — rather than a host in the network sense, which is the granularity that
//! matters: two servers sharing a data dir share pane ids and session names and really are one
//! system, while two data dirs on one machine are as separate as two machines.
//!
//! # Where it is kept, and why that survives a reinstall
//!
//! In `<data dir>/host-id`, beside the `plugin-data.db` it labels. Rebuilding and reinstalling
//! yinz does not disturb it: `cargo x install` replaces the binary and never touches
//! the data dir root. So the id is exactly as
//! durable as the data it identifies — if the rows are still there, the id that owns them is too.
//!
//! What *does* rotate it is a wiped data dir, a fresh container, or a `--data-dir` pointed
//! somewhere new. For those, and for tests that must not depend on the machine they run on,
//! `YINZ_HOST_ID` and the `host_id` config option pin it. That is the same escape hatch
//! `YINZ_HOSTNAME` provides for the display name, and it exists for the same reason.
//!
//! Deliberately not `/etc/machine-id`: it is absent in many containers, and cloned VMs share one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use yinz_types::internal_ids::HostId;

use uuid::Uuid;

/// The file under the data dir that holds the id.
pub const HOST_ID_FILE: &str = "host-id";

/// The environment variable that overrides the id.
pub const HOST_ID_ENV: &str = "YINZ_HOST_ID";

/// The longest a host id may be. It is carried in uids and in an indexed column, and a uid is
/// compared byte-wise, so there is no reason to allow a long one.
pub const MAX_HOST_ID_LEN: usize = 64;

/// Whether a string is usable as a host id.
///
/// Alphanumerics, `_` and `-` only. A `:` is excluded because the uids that carry a host id
/// join their parts with one, and a host id containing a separator would make
/// `{host}:{session}:{pane}` ambiguous. Whitespace and NUL are excluded by the same rule.
pub fn is_valid_host_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_HOST_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A freshly minted id: a v4 uuid with the hyphens taken out, so it needs no quoting anywhere.
fn mint() -> String {
    Uuid::new_v4().simple().to_string()
}

fn host_id_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HOST_ID_FILE)
}

/// Read the id from `<data_dir>/host-id`, if it holds a usable one.
///
/// A file that is missing, unreadable or holds something invalid reads as absent rather than as
/// an error: the caller's next move is to write a new one either way, and refusing to start a
/// server over a corrupt id file would be a poor trade.
fn read_file(data_dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(host_id_path(data_dir)).ok()?;
    let trimmed = contents.trim();
    is_valid_host_id(trimmed).then(|| trimmed.to_owned())
}

/// Adopt a concurrently installed id or atomically publish this one while holding the lock.
fn persist_id(data_dir: &Path, id: &str) -> std::io::Result<String> {
    use std::io::Write;

    let started = std::time::Instant::now();
    log::info!(
        "startup: host identity persistence begin path={}",
        data_dir.display()
    );
    std::fs::create_dir_all(data_dir)?;
    // Keep this inode: removing it would let concurrent servers lock different files.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(".host-id.lock"))?;
    lock.lock()?;
    log::info!(
        "startup: host identity lock acquired after {:?}",
        started.elapsed()
    );
    if let Some(stored) = read_file(data_dir) {
        return Ok(stored);
    }
    let mut temp = tempfile::NamedTempFile::new_in(data_dir)?;
    temp.write_all(id.as_bytes())?;
    log::info!(
        "startup: host identity sync begin after {:?}",
        started.elapsed()
    );
    temp.as_file().sync_all()?;
    log::info!(
        "startup: host identity sync complete after {:?}",
        started.elapsed()
    );
    temp.persist(host_id_path(data_dir))?;
    Ok(id.to_owned())
}

/// Resolve the id without consulting or filling the cache, and without reading the environment.
///
/// The environment arrives as an argument rather than being read here so that the precedence
/// rules can be tested without any test mutating a process-global that every other test in the
/// binary can see. See [`resolve`].
fn resolve_from(env: Option<&str>, data_dir: &Path, configured: Option<&str>) -> String {
    if let Some(id) = env.filter(|id| is_valid_host_id(id)) {
        return id.to_owned();
    }
    if let Some(id) = configured.filter(|id| is_valid_host_id(id)) {
        return id.to_owned();
    }
    if let Some(id) = read_file(data_dir) {
        return id;
    }
    let minted = mint();
    persist_id(data_dir, &minted).unwrap_or_else(|error| {
        // An unwritable data dir is not a reason to refuse to run. The id lasts for this process
        // instead of for this installation, which costs the rows written by this server their
        // continuity with the next one — bad, but better than not starting, and the same trade
        // the store makes when it reports a database it cannot open on first use rather than at
        // construction.
        log::warn!(
            "Could not persist host identity in {}: {error}",
            data_dir.display()
        );
        minted
    })
}

/// [`resolve_from`] against the real environment.
fn resolve(data_dir: &Path, configured: Option<&str>) -> String {
    resolve_from(
        std::env::var(HOST_ID_ENV).ok().as_deref(),
        data_dir,
        configured,
    )
}

/// The process-wide cache behind [`init_host_id`] and [`host_id`].
///
/// Cached because none of the inputs can change under a running server, and because the id ends
/// up in the uid of every row a session-scoped entity writes — resolving it per write would mean
/// a filesystem round trip on a hot path.
static HOST_ID: OnceLock<HostId> = OnceLock::new();

/// Resolve this installation's host id and cache it, returning what was cached.
///
/// Resolution order: `YINZ_HOST_ID`, then the `host_id` config option, then
/// `<data_dir>/host-id`, then a newly minted id written there.
///
/// Called once by the server at startup, where the data dir and the options are both in hand.
/// The first call decides; a later one returns what the first cached and ignores its arguments,
/// which is why this is the server's job rather than something each reader repeats.
pub fn init_host_id(data_dir: &Path, configured: Option<&HostId>) -> HostId {
    HOST_ID
        .get_or_init(|| HostId::new(resolve(data_dir, configured.map(|id| id.as_str()))))
        .clone()
}

/// This installation's host id.
///
/// Normally this is what [`init_host_id`] already cached. When nothing has initialized it — a
/// test, or a binary that is not the server — it resolves against the default data dir, which is
/// the one a server with no `--data-dir` would have used, so a reader and a server that never
/// spoke still agree.
pub fn host_id() -> HostId {
    HOST_ID
        .get_or_init(|| HostId::new(resolve(&crate::home::get_default_data_dir(), None)))
        .clone()
}

/// The environment variable that overrides [`host_display_name`].
pub const HOSTNAME_ENV: &str = "YINZ_HOSTNAME";

/// The first DNS label of `hostname`, or `None` if nothing is left.
fn short_hostname(hostname: &str) -> Option<String> {
    hostname
        .split('.')
        .next()
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
}

#[cfg(unix)]
fn system_hostname() -> Option<String> {
    nix::unistd::gethostname()
        .ok()
        .and_then(|hostname| hostname.into_string().ok())
}

#[cfg(not(unix))]
fn system_hostname() -> Option<String> {
    None
}

/// Short hostname (first DNS label) of the machine running this server, for UI display: the tab
/// bar's prefix and the host terminal's title. Empty if the host cannot be queried. Cached, since
/// it can't change under us.
///
/// `YINZ_HOSTNAME` overrides it. The tests need this: every snapshot of the tab bar would
/// otherwise record whichever machine recorded it, and none of them would match anywhere else.
pub fn host_display_name() -> String {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME
        .get_or_init(|| {
            std::env::var(HOSTNAME_ENV)
                .ok()
                .filter(|hostname| !hostname.is_empty())
                .or_else(system_hostname)
                .and_then(|hostname| short_hostname(&hostname))
                .unwrap_or_default()
        })
        .clone()
}

#[cfg(test)]
#[path = "unit/host_identity_tests.rs"]
mod host_identity_tests;
