use super::*;

#[test]
fn rollback_omits_a_false_unconfirmed_preparation_for_peers_without_the_field() {
    let legacy = serde_json::json!({"Rollback": {"operation": "op"}});
    let strict = ParticipantRequest::Rollback {
        operation: "op".into(),
        prepare_unconfirmed: false,
    };
    assert_eq!(serde_json::to_value(&strict).unwrap(), legacy);
    assert_eq!(
        serde_json::from_value::<ParticipantRequest>(legacy).unwrap(),
        strict
    );
    let unconfirmed = ParticipantRequest::Rollback {
        operation: "op".into(),
        prepare_unconfirmed: true,
    };
    let encoded = serde_json::to_value(&unconfirmed).unwrap();
    assert_eq!(encoded["Rollback"]["prepare_unconfirmed"], true);
    assert_eq!(
        serde_json::from_value::<ParticipantRequest>(encoded).unwrap(),
        unconfirmed
    );
}
