use super::*;

#[test]
fn units_keep_scalars_and_reject_overflow() {
    assert_eq!(
        serde_json::to_string(&UnixMillis::new(u64::MAX)).unwrap(),
        u64::MAX.to_string()
    );
    assert_eq!(
        serde_json::to_string(&SignedUnixMillis::new(i64::MIN)).unwrap(),
        i64::MIN.to_string()
    );
    assert_eq!(
        serde_json::to_string(&FractionalSeconds::new(0.125)).unwrap(),
        "0.125"
    );
    assert_eq!(
        serde_json::from_str::<FractionalSeconds>("-0.25")
            .unwrap()
            .get(),
        -0.25
    );
    assert!(UnixMillis::try_from(u128::MAX).is_err());
    assert!(ElapsedMillis::try_from(u128::MAX).is_err());
    assert!(SignedUnixMillis::try_from(u128::MAX).is_err());
    assert!(UnixMillis::try_from(SignedUnixMillis::new(-1)).is_err());
    assert!(SignedUnixMillis::try_from(UnixMillis::new(u64::MAX)).is_err());
    assert_eq!(
        ElapsedSeconds::new(0).checked_millis(),
        Some(ElapsedMillis::new(0))
    );
    assert_eq!(
        ElapsedSeconds::new(2).checked_millis(),
        Some(ElapsedMillis::new(2000))
    );
    assert!(ElapsedSeconds::new(u64::MAX).checked_millis().is_none());
    assert!(UnixSeconds::new(u64::MAX).checked_millis().is_none());
    for raw in [i64::MIN, -1, 0, i64::MAX] {
        assert_eq!(
            ElapsedSeconds::from_legacy_signed_bits(raw).legacy_signed_bits(),
            raw
        );
    }
    let json = r#"{"SetTimeout":0.125}"#;
    let command: crate::data::PluginCommand = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&command).unwrap(), json);
    let json = r#"{"Timer":0.125}"#;
    let event: crate::data::Event = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&event).unwrap(), json);
}
