use super::*;
use crate::thread_bus::ThreadSenders;
use yinz_runtime::channels::{unbounded, SenderWithContext};

#[tokio::test(start_paused = true)]
async fn render_deadlines_fire_once_at_the_latest_deadline_and_none_cancels() {
    let (to_screen, screen_rx) = unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(to_screen)),
        ..Default::default()
    };
    let runtime = tokio::runtime::Handle::current();
    let fired = || -> Vec<Instant> {
        screen_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ScreenInstruction::RenderDeadline(deadline) => Some(deadline),
                _ => None,
            })
            .collect()
    };
    let mut timer = RenderDeadlineTimer::default();
    let start = tokio::time::Instant::now();
    let first = (start + Duration::from_secs(10)).into_std();
    let second = (start + Duration::from_secs(20)).into_std();

    timer.set(&runtime, &senders, Some(first));
    tokio::time::sleep(Duration::from_secs(9)).await;
    assert_eq!(fired(), vec![]);
    // Replaced before it fires: the first deadline never reaches the screen.
    timer.set(&runtime, &senders, Some(second));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(fired(), vec![]);
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(fired(), vec![second]);

    let third = (tokio::time::Instant::now() + Duration::from_secs(5)).into_std();
    timer.set(&runtime, &senders, Some(third));
    timer.set(&runtime, &senders, None);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(fired(), vec![]);
}
