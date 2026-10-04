use super::*;

#[test]
fn control_agents_roundtrip_and_default_when_absent() {
    use yinz_types::internal_ids::TerminalId;
    let session = SessionInfo {
        control_agents: vec![TerminalId::new(0), TerminalId::new(u32::MAX)],
        ..Default::default()
    };
    let wire = ProtobufSessionManifest::try_from(session.clone()).unwrap();
    assert_eq!(SessionInfo::try_from(wire).unwrap(), session);
    assert!(SessionInfo::try_from(ProtobufSessionManifest::default())
        .unwrap()
        .control_agents
        .is_empty());
    let mut json = serde_json::to_value(&session).unwrap();
    json.as_object_mut().unwrap().remove("control_agents");
    assert!(serde_json::from_value::<SessionInfo>(json)
        .unwrap()
        .control_agents
        .is_empty());
}
