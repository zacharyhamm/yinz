//! Additive provider listing fields shared by native and wasm consumers.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The lease and VM incarnation the user selected, independent of changing measurements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub session: String,
    pub name: String,
    pub vm_id: Option<String>,
    #[serde(default)]
    pub generation: Option<String>,
    pub worktree: Option<PathBuf>,
}

impl Identity {
    pub fn validate(&self) -> Result<(), String> {
        crate::command::validate_firecracker_vm_name(&self.name)?;
        if self.session.trim().is_empty() {
            return Err("VM session identity is required".into());
        }
        if self.vm_id.as_deref().is_none_or(|id| id.trim().is_empty())
            || self
                .generation
                .as_deref()
                .is_none_or(|generation| generation.trim().is_empty())
        {
            return Err("Provider did not report a VM identity and generation; upgrade the provider and refresh before continuing".into());
        }
        Ok(())
    }

    pub fn validate_target(&self, session: &str, name: &str) -> Result<(), String> {
        self.validate()?;
        if self.session != session || self.name != name {
            return Err("VM identity does not match the requested session and name".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VmEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    pub session: String,
    pub name: String,
    pub state: String,
    /// The image the VM was booted from, and the host worktree it mirrors.
    ///
    /// Both are additive to protocol version 2, the way `warnings` and `recoveries` are: a
    /// provider that does not send them is read as not knowing them rather than as having
    /// none, which is also what an older provider's listing means. A VM with no worktree is
    /// one an agent got to itself, and has no host directory to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
}

impl VmEntry {
    pub fn identity(&self) -> Identity {
        Identity {
            session: self.session.clone(),
            name: self.name.clone(),
            vm_id: self.vm_id.clone(),
            generation: self
                .metrics
                .as_ref()
                .map(|metrics| metrics.generation.clone())
                .filter(|generation| !generation.is_empty()),
            worktree: self.worktree.clone(),
        }
    }
}

/// How long the provider keeps a `RecoveryEntry` before deleting it. Every provider call
/// prunes older disks, a listing included, so a surface that shows retained disks takes the
/// deadline from here rather than repeating the number.
pub const RECOVERY_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

/// A guest disk the provider kept instead of deleting: a VM that crashed, failed to
/// provision, or could not be merged back into its worktree when it was released. It is
/// deleted `RECOVERY_RETENTION_SECS` after `retained_at`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecoveryEntry {
    pub path: PathBuf,
    pub reason: String,
    /// Unix seconds.
    pub retained_at: u64,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub worktree: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Listing {
    pub vms: Vec<VmEntry>,
    pub recoveries: Vec<RecoveryEntry>,
}

/// Host measurements; CPU is cumulative microseconds, disk is host allocation.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Metrics {
    pub generation: String,
    pub source: String,
    pub sample_time_us: u64,
    pub cpu_usage_us: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub memory_source: Option<String>,
    pub disk_allocated_bytes: Option<u64>,
    pub disk_logical_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pane {
    pub id: u32,
    pub title: String,
    pub agent: bool,
}
