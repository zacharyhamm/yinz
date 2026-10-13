use super::*;

#[test]
fn layout_query_progress_is_shared_bounded_and_ordered() {
    let caller = LayoutQueryTrace::default();
    let downstream = caller.clone();
    caller.mark("queued");
    for _ in 0..20 {
        downstream.mark("map lock");
    }
    downstream.mark("response");
    let stages = caller.stages.lock().unwrap();
    assert_eq!(stages.len(), 16);
    assert_eq!(stages.front().unwrap().0, "queued");
    assert_eq!(stages.back().unwrap().0, "response");
    assert!(stages
        .iter()
        .zip(stages.iter().skip(1))
        .all(|(a, b)| a.1 <= b.1));
}
