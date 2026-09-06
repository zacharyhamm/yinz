//! A stable, opaque identifier for the machine this server runs on.
//!
//! # Why a row needs to say which machine wrote it
//!
//! With `plugin_data_url` pointing at Postgres, several zellij servers on *different machines*
//! share one plugin data store as one user. The store was already shared between the servers on
//! one machine — `plugin-data.db` lives in the data dir rather than a session cache dir — and the
//! entities that care about it scope their rows by `zellij_session_name` for that reason. A
//! session name does not survive the widening: two machines can each run a session called `main`,
//! so a row written by one is indistinguishable from a row written by the other, and a pane id
//! means nothing outside the server that allocated it.
//!
//! [`host_id`] is what makes those rows distinguishable. It identifies an *installation* — a data
//! dir on a machine — rather than a host in the network sense, which is the granularity that
//! matters: two servers sharing a data dir share pane ids and session names and really are one
//! system, while two data dirs on one machine are as separate as two machines.
//!
//! # Where it is kept, and why that survives a reinstall
//!
//! In `<data dir>/host-id`, beside the `plugin-data.db` it labels. Rebuilding and reinstalling
//! zellij does not disturb it: `cargo x install` writes and prunes `*.wasm` inside
//! `<data dir>/builtins/<version>/` and never touches the data dir root. So the id is exactly as
//! durable as the data it identifies — if the rows are still there, the id that owns them is too.
//!
//! What *does* rotate it is a wiped data dir, a fresh container, or a `--data-dir` pointed
//! somewhere new. For those, and for tests that must not depend on the machine they run on,
//! `ZELLIJ_HOST_ID` and the `host_id` config option pin it. That is the same escape hatch
//! `ZELLIJ_HOSTNAME` provides for the display name, and it exists for the same reason.
//!
//! Deliberately not `/etc/machine-id`: it is absent in many containers, and cloned VMs share one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use uuid::Uuid;

/// The file under the data dir that holds the id.
pub const HOST_ID_FILE: &str = "host-id";

/// The environment variable that overrides the id.
pub const HOST_ID_ENV: &str = "ZELLIJ_HOST_ID";

/// The longest a host id may be. It is carried in uids and in an indexed column, and a uid is
/// compared byte-wise, so there is no reason to allow a long one.
pub const MAX_HOST_ID_LEN: usize = 64;

/// Whether a string is usable as a host id.
///
/// Alphanumerics, `_` and `-` only. A `:` is excluded because the uids that carry a host id
/// join their parts with one, and a host id containing a separator would make
/// `{host}:{session}:{pane}` ambiguous. Everything else — whitespace, NUL, which Postgres cannot
/// store at all — falls out of the same rule.
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

/// Write `id` to `<data_dir>/host-id`, atomically.
///
/// Through a temporary file and a rename, so a second server starting concurrently reads either
/// nothing or a whole id, never half of one. Both servers may mint an id and race to install it;
/// the loser's `read_file` on the next call returns the winner's, which is why the caller re-reads
/// rather than trusting what it wrote.
fn write_file(data_dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let final_path = host_id_path(data_dir);
    let tmp_path = data_dir.join(format!(".{}.{}.tmp", HOST_ID_FILE, std::process::id()));
    std::fs::write(&tmp_path, id)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&tmp_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&tmp_path, perms);
        }
    }
    match std::fs::rename(&tmp_path, &final_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e)
        },
    }
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
    if write_file(data_dir, &minted).is_err() {
        // An unwritable data dir is not a reason to refuse to run. The id lasts for this process
        // instead of for this installation, which costs the rows written by this server their
        // continuity with the next one — bad, but better than not starting, and the same trade
        // the store makes when it reports a database it cannot open on first use rather than at
        // construction.
        return minted;
    }
    // Re-read rather than returning what was written: another server may have installed its own
    // id between the check and the rename, and the file is the authority.
    read_file(data_dir).unwrap_or(minted)
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
static HOST_ID: OnceLock<String> = OnceLock::new();

/// Resolve this installation's host id and cache it, returning what was cached.
///
/// Resolution order: `ZELLIJ_HOST_ID`, then the `host_id` config option, then
/// `<data_dir>/host-id`, then a newly minted id written there.
///
/// Called once by the server at startup, where the data dir and the options are both in hand.
/// The first call decides; a later one returns what the first cached and ignores its arguments,
/// which is why this is the server's job rather than something each reader repeats.
pub fn init_host_id(data_dir: &Path, configured: Option<&str>) -> String {
    HOST_ID
        .get_or_init(|| resolve(data_dir, configured))
        .clone()
}

/// This installation's host id.
///
/// Normally this is what [`init_host_id`] already cached. When nothing has initialized it — a
/// test, or a binary that is not the server — it resolves against the default data dir, which is
/// the one a server with no `--data-dir` would have used, so a reader and a server that never
/// spoke still agree.
pub fn host_id() -> String {
    HOST_ID
        .get_or_init(|| resolve(&crate::home::get_default_data_dir(), None))
        .clone()
}

#[cfg(test)]
#[path = "unit/host_identity_tests.rs"]
mod host_identity_tests;
