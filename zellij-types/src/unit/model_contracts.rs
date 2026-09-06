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

#[test]
fn recording_models_preserve_signed_values_and_wire_fields() {
    use crate::recording::{RecordingSummary, SearchHit};
    for value in [i64::MIN, -1, 0, i64::MAX] {
        let summary = format!(
            r#"{{"id":{value},"session_name":"s","tab_name":"t","terminal_id":4294967295,"pane_title":"p","started_at_us":{value},"duration_us":{value},"is_running":false,"capture_input":true,"error":null}}"#
        );
        let decoded: RecordingSummary = serde_json::from_str(&summary).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), summary);
        let hit = format!(
            r#"{{"recording_id":{value},"session_name":"s","tab_name":"t","pane_title":"p","elapsed_us":{value},"started_at_us":{value},"text":"match"}}"#
        );
        let decoded: SearchHit = serde_json::from_str(&hit).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), hit);
    }
}

#[test]
fn geometry_reports_rejected_operations_without_changing_dimensions() {
    let mut fixed = Dimension::fixed(80);
    assert!(fixed.reduce_by(10.0, 10).is_err());
    assert_eq!(fixed, Dimension::fixed(80));
    let mut percent = Dimension::percent(33.0);
    percent.set_inner(3);
    percent.reduce_by(13.0, 10).unwrap();
    assert_eq!(percent.as_percent(), Some(20.0));
    assert_eq!(percent.as_usize(), 0);
    let fixed = PaneGeom {
        rows: Dimension::fixed(1),
        cols: Dimension::fixed(1),
        ..Default::default()
    };
    let flexible = PaneGeom {
        rows: Dimension::percent(50.0),
        cols: Dimension::percent(50.0),
        ..Default::default()
    };
    assert!(fixed.combine_vertically_with(&flexible).is_none());
    assert!(flexible.combine_horizontally_with(&fixed).is_none());
    assert!(flexible
        .combine_vertically_with_many(&vec![flexible, fixed])
        .is_none());
    assert!(flexible
        .combine_horizontally_with_many(&vec![flexible, fixed])
        .is_none());
    assert!(flexible.combine_vertically_with_many(&vec![]).is_some());
    for value in [0, 100, 101, usize::MAX] {
        for result in [
            FloatingPaneCoordinates::default().with_x_percent(value),
            FloatingPaneCoordinates::default().with_y_percent(value),
            FloatingPaneCoordinates::default().with_width_percent(value),
            FloatingPaneCoordinates::default().with_height_percent(value),
        ] {
            assert_eq!(result.is_ok(), value <= 100);
        }
    }
}

#[test]
fn recording_units_preserve_scalars_and_recording_map_keys() {
    use crate::internal_ids::RecordingId;
    use crate::units::{ElapsedMicros, UnixMicros};
    use std::collections::BTreeMap;
    for value in [i64::MIN, -1, 0, i64::MAX] {
        let id = RecordingId::new(value);
        assert_eq!(id.get(), value);
        assert_eq!(value.to_string().parse::<RecordingId>().unwrap(), id);
        let json = format!(r#"{{"{value}":"recording"}}"#);
        let map = BTreeMap::from([(id, "recording".to_owned())]);
        assert_eq!(serde_json::to_string(&map).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<BTreeMap<RecordingId, String>>(&json).unwrap(),
            map
        );
        let timestamp = UnixMicros::new(value);
        let elapsed = ElapsedMicros::new(value);
        assert_eq!(timestamp.get(), value);
        assert_eq!(elapsed.get(), value);
        assert_eq!(
            serde_json::to_string(&timestamp).unwrap(),
            value.to_string()
        );
        assert_eq!(serde_json::to_string(&elapsed).unwrap(), value.to_string());
    }
    assert!("9223372036854775808".parse::<RecordingId>().is_err());
    assert!(serde_json::from_str::<ElapsedMicros>("-9223372036854775809").is_err());
    assert!(serde_json::from_str::<UnixMicros>("9223372036854775808").is_err());
}
