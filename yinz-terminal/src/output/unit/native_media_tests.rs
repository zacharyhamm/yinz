use super::*;
#[test]
fn shared_uploads_survive_until_the_frame_is_acknowledged() {
    let first = tempfile::NamedTempFile::new().unwrap();
    let second = tempfile::NamedTempFile::new().unwrap();
    let first_path = first.path().to_owned();
    let second_path = second.path().to_owned();
    let mut state = NativeRenderState {
        seq: 2,
        media: vec![(1, first), (2, second)],
        ..Default::default()
    };
    state.acknowledge(1);
    assert!(!first_path.exists());
    assert!(second_path.exists());
    state.acknowledge(3);
    assert!(second_path.exists());
    state.acknowledge(2);
    assert!(!second_path.exists());
}
