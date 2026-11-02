use super::*;

#[test]
fn recovery_survives_reopen_and_acknowledgements_are_per_run() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(pending(dir.path()).unwrap(), None);
    let first = begin(dir.path()).unwrap();
    assert_eq!(pending(dir.path()).unwrap(), Some(first.clone()));
    acknowledge(dir.path(), &first).unwrap();
    assert_eq!(pending(dir.path()).unwrap(), None);
    let second = begin(dir.path()).unwrap();
    acknowledge(dir.path(), &first).unwrap();
    assert_eq!(pending(dir.path()).unwrap(), Some(second.clone()));
    acknowledge(dir.path(), &second).unwrap();
    acknowledge(dir.path(), &first).unwrap();
    assert_eq!(pending(dir.path()).unwrap(), None);
}

#[test]
fn recovery_rejects_invalid_run_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    assert!(acknowledge(dir.path(), "../outside").is_err());
}
