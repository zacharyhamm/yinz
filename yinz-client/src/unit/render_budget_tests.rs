use crate::render_budget::RenderBudget;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// Reserve on another thread, reporting when it returns.
fn reserve_in_background(budget: &Arc<RenderBudget>, bytes: usize) -> mpsc::Receiver<()> {
    let (done, finished) = mpsc::channel();
    let budget = budget.clone();
    std::thread::spawn(move || {
        budget.reserve(bytes);
        let _ = done.send(());
    });
    finished
}

#[test]
fn a_full_budget_holds_the_next_frame_until_one_is_written() {
    let budget = Arc::new(RenderBudget::new(10));
    budget.reserve(6);
    let next = reserve_in_background(&budget, 6);
    // Nothing else releases bytes, so a return here would be the bug.
    assert!(next.recv_timeout(Duration::from_millis(200)).is_err());
    budget.release(6);
    next.recv_timeout(Duration::from_secs(30))
        .expect("the frame fits once the first is written");
}

#[test]
fn a_frame_larger_than_the_budget_goes_through_alone() {
    let budget = RenderBudget::new(10);
    budget.reserve(100);
    budget.release(100);
    budget.reserve(3);
}

#[test]
fn closing_releases_a_router_waiting_for_room() {
    let budget = Arc::new(RenderBudget::new(10));
    budget.reserve(10);
    let next = reserve_in_background(&budget, 1);
    budget.close();
    next.recv_timeout(Duration::from_secs(30))
        .expect("a closed budget never blocks");
}
