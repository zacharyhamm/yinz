//! `<data_dir>/web/password`: the argon2id verifier, written atomically with 0600
//! permissions and read with `O_NOFOLLOW`.

use std::path::Path;

use yinz_runtime::errors::prelude::*;

/// A parsed, validated password file.
#[derive(Clone, PartialEq, Eq)]
pub struct Verifier {
    /// The PHC string. Never logged.
    pub phc: String,
    /// Rotated on every write and revocation; sessions must name the current one.
    pub verifier_id: String,
    pub updated_at_ms: i64,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("verifier_id", &self.verifier_id)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswordState {
    Missing,
    /// Present but unusable (permissions, owner, size, parse, parameters); the reason.
    Invalid(String),
    Ready(Verifier),
}

/// Set the password: a new `verifier_id`, and every session signed out.
pub fn write(data_dir: &Path, password: &zeroize::Zeroizing<String>) -> Result<()> {
    let _ = password;
    bail!(
        "setting the web password under {} is not supported by this build",
        data_dir.display()
    )
}

/// Keep the password, sign out every session: a new `verifier_id`.
pub fn revoke_sessions(data_dir: &Path) -> Result<()> {
    bail!(
        "revoking web sessions under {} is not supported by this build",
        data_dir.display()
    )
}

/// Remove the password and sessions files. True when anything was removed.
pub fn clear(data_dir: &Path) -> Result<bool> {
    bail!(
        "clearing the web password under {} is not supported by this build",
        data_dir.display()
    )
}

pub fn read_state(data_dir: &Path) -> PasswordState {
    let _ = data_dir;
    PasswordState::Missing
}

/// Check a candidate password. Blocking: run it off the async runtime.
pub fn verify(v: &Verifier, candidate: &[u8]) -> bool {
    let _ = (v, candidate);
    false
}
