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
fn layout_diagnostic_keeps_its_serialized_fields() {
    use crate::data::LayoutParsingError;
    // KdlError's diagnostic source was never serialized. It remains separate from the payload.
    let json = r#"{"KdlError":{"kdl_error":{"error_message":"invalid layout","offset":9,"len":5,"help_message":null},"file_name":"layout.kdl","source_code":"layout { bad }"}}"#;
    let error: LayoutParsingError = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&error).unwrap(), json);
}

#[test]
fn session_pane_selectors_keep_legacy_tuples() {
    use crate::data::ConnectToSession;
    use crate::input::actions::Action;
    use crate::pane_id::PaneId;
    for (value, pane) in [
        ("null", None),
        ("[0,false]", Some(PaneId::from_legacy(0, false))),
        (
            "[4294967295,true]",
            Some(PaneId::from_legacy(u32::MAX, true)),
        ),
    ] {
        let json = format!(
            r#"{{"name":"main","tab_position":null,"pane_id":{value},"layout":null,"cwd":null}}"#
        );
        let session: ConnectToSession = serde_json::from_str(&json).unwrap();
        assert_eq!(session.pane_id, pane);
        assert_eq!(serde_json::to_string(&session).unwrap(), json);
        let json = format!(r#"{{"SwitchSession":{json}}}"#);
        let action: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&action).unwrap(), json);
    }
}

#[test]
fn pane_manifest_identity_keeps_flat_numeric_fields() {
    use crate::data::PaneInfo;
    use crate::pane_id::PaneId;
    for (id, is_plugin) in [(0, false), (u32::MAX, true)] {
        let pane = PaneInfo {
            id: PaneId::from_legacy(id, is_plugin),
            ..Default::default()
        };
        let mut value = serde_json::to_value(&pane).unwrap();
        assert_eq!(value["id"], id);
        assert_eq!(value["is_plugin"], is_plugin);
        assert_eq!(
            serde_json::from_value::<PaneInfo>(value.clone()).unwrap(),
            pane
        );
        for invalid in [serde_json::json!(-1), serde_json::json!(4294967296_u64)] {
            value["id"] = invalid;
            assert!(serde_json::from_value::<PaneInfo>(value.clone()).is_err());
        }
    }
}

#[test]
fn plugin_data_ids_and_revisions_preserve_signed_values_and_absent_guards() {
    use crate::internal_ids::{DataRevision, DataRowId};
    use crate::plugin_data::{ApplyReport, DataValue, Mutation, REV_ABSENT};
    let json = r#"{"rows_written":4,"rows_deleted":0,"ids":[-9223372036854775808,-1,0,9223372036854775807],"per_mutation":[1,1,1,1]}"#;
    let report: ApplyReport = serde_json::from_str(json).unwrap();
    assert_eq!(report.ids, [i64::MIN, -1, 0, i64::MAX].map(DataRowId::new));
    assert_eq!(serde_json::to_string(&report).unwrap(), json);
    let guard = Mutation::put("item")
        .uid("a")
        .set("text", "x")
        .expect_absent()
        .build();
    let json =
        r#"{"Put":{"entity":"item","uid":"a","values":[["text",{"Text":"x"}]],"expect_rev":0}}"#;
    assert_eq!(serde_json::to_string(&guard).unwrap(), json);
    assert_eq!(serde_json::from_str::<Mutation>(json).unwrap(), guard);
    assert_eq!(REV_ABSENT, DataRevision::new(0));
    assert_eq!(DataValue::from(DataRevision::new(7)), DataValue::Int(7));
    assert_eq!(DataValue::from(DataRowId::new(7)), DataValue::Int(7));
    assert!(DataRowId::try_from(u64::MAX).is_err());
    assert!(DataRevision::try_from(u64::MAX).is_err());
}

#[test]
fn session_names_preserve_strings_and_map_keys() {
    use crate::internal_ids::SessionName;
    use std::collections::BTreeMap;

    for raw in ["", "main", "日本語", "a/b"] {
        // Construction does not move the creation/rename validation boundary.
        let name: SessionName = raw.parse().unwrap();
        assert_eq!(name.as_str(), raw);
        assert_eq!(name.to_string(), raw);
        assert_eq!(serde_json::to_value(&name).unwrap(), raw);
        assert_eq!(name.into_string(), raw);
    }
    let json = r#"{"main":1}"#;
    let names: BTreeMap<SessionName, u32> = serde_json::from_str(json).unwrap();
    assert_eq!(names.get("main"), Some(&1));
    assert_eq!(serde_json::to_string(&names).unwrap(), json);
    let json = r#"{"RenameSession":{"name":"main"}}"#;
    let action: crate::input::actions::Action = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&action).unwrap(), json);
}

#[test]
fn pane_tabs_cwd_propagation_tolerates_invalid_selection() {
    use crate::input::layout::{
        FloatingPaneLayout, PluginAlias, Run, RunPluginOrAlias, TiledPaneLayout,
    };
    use crate::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
    use std::path::Path;

    for tabs in [
        vec![],
        vec![PaneTabDescriptor {
            label: "First".into(),
            location: "yinz:strider".into(),
            configuration: Default::default(),
            initial_cwd: None,
        }],
    ] {
        let pane_tabs = Some(PaneTabsLayout {
            active: tabs.len(),
            tabs,
        });
        let run = Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::new(
            "strider", &None, None,
        ))));
        let mut tiled = TiledPaneLayout {
            pane_tabs: pane_tabs.clone(),
            run: run.clone(),
            ..Default::default()
        };
        tiled.add_cwd_to_layout(Path::new("/tmp/origin"));
        let mut floating = FloatingPaneLayout {
            pane_tabs,
            run,
            ..Default::default()
        };
        floating.add_cwd_to_layout(Path::new("/tmp/origin"));
        for tabs in [tiled.pane_tabs.unwrap(), floating.pane_tabs.unwrap()] {
            assert!(tabs
                .tabs
                .iter()
                .all(|tab| tab.initial_cwd.as_deref() == Some(Path::new("/tmp/origin"))));
        }
    }
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
    assert!(serde_json::from_str::<CommandToRun>(r#"{"path":"sh","remote_host":" "}"#).is_err());
}

#[test]
fn commands_reject_retired_vm_fields_even_with_an_explicit_local_target() {
    for target in [serde_json::Value::Null, serde_json::json!("Local")] {
        let mut input = serde_json::json!({"path":"sh", "target":target});
        assert_eq!(
            serde_json::from_value::<CommandToRun>(input.clone())
                .unwrap()
                .target,
            PaneTarget::Local
        );
        for value in [serde_json::Value::Null, serde_json::json!("vm")] {
            input["firecracker_vm"] = value;
            let error = serde_json::from_value::<CommandToRun>(input.clone()).unwrap_err();
            assert!(error.to_string().contains("unknown field `firecracker_vm`"));
        }
    }
}

#[test]
fn agent_specs_reject_retired_vm_fields_regardless_of_permissions() {
    use crate::agent::AgentSpawnSpec;
    for permission in [serde_json::Value::Null, serde_json::json!("skip")] {
        let mut input = serde_json::json!({"kind":"codex", "permission_mode":permission});
        assert!(serde_json::from_value::<AgentSpawnSpec>(input.clone()).is_ok());
        for value in [serde_json::Value::Null, serde_json::json!({})] {
            input["firecracker"] = value;
            let error = serde_json::from_value::<AgentSpawnSpec>(input.clone()).unwrap_err();
            assert!(error.to_string().contains("unknown field `firecracker`"));
        }
    }
}

#[test]
fn live_agent_order_uses_state_recency_names_pane_and_session() {
    use crate::{agent::LiveAgentOrder, pane_id::PaneId};
    let key = |state, stamp, project, branch, pane, session| {
        LiveAgentOrder::new(
            state,
            stamp,
            project,
            branch,
            PaneId::from_legacy(pane, false),
            session,
        )
    };
    let ordered = [
        key(Some(AgentState::Waiting), None, None, None, 9, "z"),
        key(Some(AgentState::Busy), Some(20), None, None, 9, "z"),
        key(
            Some(AgentState::Busy),
            Some(10),
            Some("a"),
            Some("a"),
            1,
            "a",
        ),
        key(
            Some(AgentState::Busy),
            Some(10),
            Some("a"),
            Some("a"),
            1,
            "b",
        ),
        key(
            Some(AgentState::Busy),
            Some(10),
            Some("a"),
            Some("a"),
            2,
            "a",
        ),
        key(
            Some(AgentState::Busy),
            Some(10),
            Some("a"),
            Some("b"),
            1,
            "a",
        ),
        key(Some(AgentState::Busy), Some(10), Some("a"), None, 1, "a"),
        key(Some(AgentState::Busy), Some(10), Some("b"), None, 1, "a"),
        key(Some(AgentState::Busy), Some(10), None, None, 1, "a"),
        key(Some(AgentState::Busy), None, Some("a"), Some("a"), 1, "a"),
        key(
            Some(AgentState::Idle),
            Some(30),
            Some("a"),
            Some("a"),
            1,
            "a",
        ),
    ];
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn pane_hook_recency_is_optional_and_roundtrips() {
    use crate::data::PaneInfo;
    let mut value = serde_json::to_value(PaneInfo::default()).unwrap();
    assert!(value.get("last_hook_at_ms").is_none());
    assert_eq!(
        serde_json::from_value::<PaneInfo>(value.clone())
            .unwrap()
            .last_hook_at_ms,
        None
    );
    value["last_hook_at_ms"] = serde_json::json!(123);
    let pane: PaneInfo = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(pane.last_hook_at_ms, Some(123));
    assert_eq!(serde_json::to_value(pane).unwrap(), value);
}
