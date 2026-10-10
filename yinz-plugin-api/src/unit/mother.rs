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
