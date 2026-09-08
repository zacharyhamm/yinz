//! The management panel's direct requests and host snapshots. No conversation state.
use serde::{Deserialize, Serialize};
use zellij_types::firecracker::{Metrics, RecoveryEntry, VmEntry};

pub const REQUEST: &str = "vm_panel_request";
pub const RESULT: &str = "vm_panel_result";
pub const VISIBILITY: &str = "vm_panel_visibility";
pub const STOP_TABS: &str = "management_tabs";

pub use zellij_types::firecracker::Identity;

/// Only a unique builtin overlay with a pane can own the management companion.
/// Suppressed panes still own it; visibility does not change plugin identity.
pub fn panel_owner(
    session: &zellij_types::data::SessionInfo,
) -> Option<zellij_types::internal_ids::PluginId> {
    use zellij_types::data::PaneId;
    if !session.is_current_session {
        return None;
    }
    let mut owners = session.plugins.iter().filter_map(|(id, plugin)| {
        (plugin.location == "zellij:hocus"
            && plugin.configuration.get("overlay").map(String::as_str) == Some("true")
            && session
                .panes
                .panes
                .values()
                .flatten()
                .any(|pane| pane.id == PaneId::Plugin(*id) && pane.is_floating))
        .then_some(*id)
    });
    let owner = owners.next()?;
    owners.next().is_none().then_some(owner)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    List,
    Changes,
    Sync,
    Close,
}
impl Operation {
    pub fn tool(self) -> &'static str {
        match self {
            Self::List => "vm_list",
            Self::Changes => "vm_changes",
            Self::Sync => "vm_sync",
            Self::Close => "vm_close",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub request_id: u64,
    pub operation: Operation,
    pub target: Option<Identity>,
}
pub use zellij_types::firecracker::Pane;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lease {
    #[serde(flatten)]
    pub entry: VmEntry,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub panes_error: Option<String>,
    #[serde(default)]
    pub cpu_percent: Option<f64>,
}
impl Lease {
    pub fn identity(&self) -> Identity {
        self.entry.identity()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub current_session: Option<String>,
    pub vms: Vec<Lease>,
    #[serde(default)]
    pub recoveries: Vec<RecoveryEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultMessage {
    pub request_id: u64,
    pub operation: Operation,
    pub target: Option<Identity>,
    pub snapshot: Option<Snapshot>,
    pub output: String,
    pub error: String,
}

/// 100% is one host core. A different source or process begins a new baseline.
pub fn cpu_percent(previous: &Metrics, current: &Metrics) -> Option<f64> {
    if current.generation.is_empty()
        || previous.generation != current.generation
        || previous.source != current.source
    {
        return None;
    }
    let elapsed = current
        .sample_time_us
        .checked_sub(previous.sample_time_us)?;
    if elapsed == 0 {
        return None;
    }
    let cpu = current.cpu_usage_us?.checked_sub(previous.cpu_usage_us?)?;
    Some(cpu as f64 * 100.0 / elapsed as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_samples_reset_on_restart_source_change_and_clock_regression() {
        let a = Metrics {
            generation: "42:100".into(),
            source: "process".into(),
            sample_time_us: 100,
            cpu_usage_us: Some(50),
            ..Default::default()
        };
        let mut b = Metrics {
            sample_time_us: 200,
            cpu_usage_us: Some(175),
            ..a.clone()
        };
        assert_eq!(cpu_percent(&a, &b), Some(125.0));
        b.generation = "42:101".into();
        assert_eq!(cpu_percent(&a, &b), None);
        b = a.clone();
        b.source = "cgroup".into();
        assert_eq!(cpu_percent(&a, &b), None);
        assert_eq!(cpu_percent(&a, &a), None);
        assert_eq!(cpu_percent(&b, &a), None);
    }
    #[test]
    fn older_provider_json_needs_no_metrics_or_recoveries() {
        let snapshot: Snapshot =
            serde_json::from_str(r#"{"vms":[{"session":"s","name":"v","state":"running"}]}"#)
                .unwrap();
        assert!(snapshot.vms[0].entry.metrics.is_none());
        assert!(snapshot.vms[0].entry.vm_id.is_none());
        assert!(snapshot.recoveries.is_empty());
    }
}
