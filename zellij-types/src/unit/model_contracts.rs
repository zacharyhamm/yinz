//! Fixed serialized values retained across the model extraction.
use crate::{
    agent::{AgentInfo, AgentSource, AgentState},
    command::{CommandToRun, PaneTarget},
    geometry::{FloatingPaneCoordinates, PercentOrFixed, SplitSize},
    pane_size::{Dimension, PaneGeom, Size},
};

#[test]
fn geometry_preserves_persisted_fields_and_rounding() {
    let json = r#"{"x":0,"y":1,"rows":{"constraint":{"Percent":33.0},"inner":1},"cols":{"constraint":{"Fixed":80},"inner":80},"stacked":0,"is_pinned":true,"logical_position":null}"#;
    let geom: PaneGeom = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&geom).unwrap(), json);
    assert!(geom.is_pinned);
    assert_eq!(geom.stacked, Some(0));
    assert_eq!(geom.logical_position, None);
    assert_eq!(geom.rows, Dimension::percent(33.0));
    assert_eq!(geom.cols, Dimension::fixed(80));
    for value in [0, usize::MAX] {
        let json = format!(r#"{{"rows":{value},"cols":{value}}}"#);
        let size: Size = serde_json::from_str(&json).unwrap();
        assert_eq!(
            size,
            Size {
                rows: value,
                cols: value
            }
        );
        assert_eq!(serde_json::to_string(&size).unwrap(), json);
    }
    let json = r#"{"x":{"Percent":0},"y":{"Fixed":0},"width":{"Percent":100},"height":null,"pinned":false,"borderless":null}"#;
    let coordinates: FloatingPaneCoordinates = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&coordinates).unwrap(), json);
    assert_eq!(PercentOrFixed::Percent(33).to_position(10), 4);
    assert_eq!(PercentOrFixed::Percent(33).to_fixed(10), 3);
    assert_eq!(SplitSize::Percent(33).to_fixed(10), 3);
    assert_eq!(
        "0%".parse::<PercentOrFixed>().unwrap(),
        PercentOrFixed::Percent(0)
    );
    assert!("0%".parse::<SplitSize>().is_err());
    assert!("101%".parse::<PercentOrFixed>().is_err());
}

#[test]
fn agent_records_preserve_defaults_and_explicit_waits() {
    let old: AgentInfo = serde_json::from_str(r#"{"kind":"codex","state":"Idle"}"#).unwrap();
    assert_eq!(old.source, AgentSource::Spawned);
    assert!(!old.is_waiting_on_a_person());
    let json = r#"{"kind":"codex","state":"Waiting","source":"Reported","detail":null,"waiting_on":"Continue?","wait_needs_a_person":true,"cwd":null,"session_id":"s","run_id":"r"}"#;
    let mut record: AgentInfo = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&record).unwrap(), json);
    assert!(record.is_waiting_on_a_person());
    record.state = AgentState::Idle;
    assert!(!record.is_waiting_on_a_person());
}

#[test]
fn command_targets_preserve_legacy_input_and_canonical_output() {
    let command: CommandToRun =
        serde_json::from_str(r#"{"path":"sh","remote_host":"build","remote_default_shell":true}"#)
            .unwrap();
    assert_eq!(
        command.target,
        PaneTarget::Ssh {
            host: "build".into()
        }
    );
    let json = r#"{"path":"sh","args":[],"cwd":null,"target":{"Ssh":{"host":"build"}},"remote_default_shell":true}"#;
    assert_eq!(serde_json::to_string(&command).unwrap(), json);
    assert_eq!(serde_json::from_str::<CommandToRun>(json).unwrap(), command);
    assert!(serde_json::from_str::<CommandToRun>(
        r#"{"remote_host":"build","firecracker_vm":"vm"}"#,
    )
    .is_err());
    assert!(serde_json::from_str::<CommandToRun>(
        r#"{"firecracker_vm":"vm","firecracker_worktree":"relative"}"#,
    )
    .is_err());
}
