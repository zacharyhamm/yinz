use super::*;
use crate::test_support::fake_os_api::FakeClientOsApi;

#[tokio::test]
async fn stopping_an_exhausted_announcer_releases_the_client_input() {
    // The worker owns the only receiver; its closure tells us that shutdown
    // released the old client's I/O, even when there is no host to wake it.
    for _ in 0..32 {
        let (input, receiver) = tokio::sync::mpsc::unbounded_channel();
        let announcer = NestedReannounce::spawn(
            Box::new(FakeClientOsApi::new().with_async_stdin(receiver)),
            SessionName::from("stopped-client"),
        );
        *announcer.scheduler.lock().unwrap() =
            ReannounceScheduler::with_settings(Instant::now(), Duration::ZERO, 1);
        announcer.wakeup.notify_all();
        announcer.stop();
        tokio::time::timeout(Duration::from_secs(10), input.closed())
            .await
            .expect("stopped announcer retained the client's input");
    }
}
