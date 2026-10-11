use super::*;

#[test]
fn mother_roundtrips_and_defaults_when_absent() {
    use yinz_types::internal_ids::TerminalId;
    for id in [0, u32::MAX] {
        let session = SessionInfo {
            mother: Some(TerminalId::new(id)),
            ..Default::default()
        };
        let wire = ProtobufSessionManifest::try_from(session.clone()).unwrap();
        assert_eq!(SessionInfo::try_from(wire).unwrap(), session);
        let mut json = serde_json::to_value(&session).unwrap();
        json.as_object_mut().unwrap().remove("mother");
        assert!(serde_json::from_value::<SessionInfo>(json)
            .unwrap()
            .mother
            .is_none());
    }
    assert!(SessionInfo::try_from(ProtobufSessionManifest::default())
        .unwrap()
        .mother
        .is_none());
}

#[test]
fn supervision_roundtrips_and_is_additive() {
    let session = SessionInfo {
        mother_supervision: Some(yinz_types::agent::mother::MotherStatus {
            state: "queued".into(),
            pending: 3,
            blockers: 1,
        }),
        ..Default::default()
    };
    let wire: ProtobufSessionManifest = session.clone().try_into().unwrap();
    let decoded: SessionInfo = wire.try_into().unwrap();
    assert_eq!(decoded.mother_supervision, session.mother_supervision);
    let mut value = serde_json::to_value(session).unwrap();
    value.as_object_mut().unwrap().remove("mother_supervision");
    assert!(serde_json::from_value::<SessionInfo>(value)
        .unwrap()
        .mother_supervision
        .is_none());
}
