use super::*;
use yinz_runtime::channels::{unbounded, Receiver, SenderWithContext};
use yinz_types::diagnostics::ErrorContext;
use yinz_types::internal_ids::{TabId, TerminalId};

struct Worker {
    runtime: Arc<tokio::runtime::Runtime>,
    jobs: Option<SenderWithContext<BackgroundJob>>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
    screen: Receiver<(ScreenInstruction, ErrorContext)>,
    pty: Receiver<(PtyInstruction, ErrorContext)>,
    plugin: Receiver<(PluginInstruction, ErrorContext)>,
}

impl Worker {
    fn new() -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .start_paused(true)
                .build()
                .unwrap(),
        );
        let (jobs, receiver) = unbounded();
        let (screen, screen_rx) = unbounded();
        let (pty, pty_rx) = unbounded();
        let (plugin, plugin_rx) = unbounded();
        let bus = Bus::new(
            vec![receiver],
            Some(&SenderWithContext::new(screen)),
            Some(&SenderWithContext::new(pty)),
            Some(&SenderWithContext::new(plugin)),
            None,
            None,
            None,
            None,
        );
        let worker_runtime = runtime.clone();
        let thread = std::thread::spawn(move || {
            background_jobs_with_runtime(bus, Some(ElapsedSeconds::new(1)), true, &worker_runtime)
        });
        let worker = Self {
            runtime,
            jobs: Some(SenderWithContext::new(jobs)),
            thread: Some(thread),
            screen: screen_rx,
            pty: pty_rx,
            plugin: plugin_rx,
        };
        worker.sync();
        worker
    }

    fn send(&self, job: BackgroundJob) {
        self.jobs.as_ref().unwrap().send(job).unwrap();
        self.sync();
    }

    fn sync(&self) {
        // This synchronous reply acknowledges all preceding jobs before driving their timers.
        self.jobs
            .as_ref()
            .unwrap()
            .send(BackgroundJob::ReportLayoutInfo(Default::default()))
            .unwrap();
        assert!(matches!(
            self.plugin
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
                .0,
            PluginInstruction::UpdateSessionSaveTime(_)
        ));
        self.runtime.block_on(tokio::task::yield_now());
    }

    fn advance(&self, milliseconds: u64) {
        self.runtime.block_on(async {
            tokio::time::advance(Duration::from_millis(milliseconds)).await;
            tokio::task::yield_now().await;
        });
    }

    fn screen_events(&self) -> Vec<ScreenInstruction> {
        self.screen.try_iter().map(|(event, _)| event).collect()
    }

    fn stop(&mut self, disconnect: bool) {
        if !disconnect {
            self.jobs
                .as_ref()
                .unwrap()
                .send(BackgroundJob::Exit)
                .unwrap();
        }
        self.jobs.take();
        assert_eq!(
            self.thread.take().unwrap().join().unwrap().is_err(),
            disconnect
        );
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(jobs) = self.jobs.take() {
            let _ = jobs.send(BackgroundJob::Exit);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const A: PaneId = PaneId::Terminal(TerminalId::new(1));
const B: PaneId = PaneId::Terminal(TerminalId::new(2));
const C: PaneId = PaneId::Terminal(TerminalId::new(3));
const TAB: TabId = TabId::new(1);
const HALF_FLASH: u64 = FLASH_DURATION_MS / 2 + 1;

#[test]
fn stopping_one_pane_keeps_the_other_batch_timers() {
    let worker = Worker::new();
    worker.send(BackgroundJob::FlashPaneBell(vec![A, B]));
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::FlashPaneFrameColor(panes)] if panes == &[A, B]
    ));
    worker.advance(HALF_FLASH);
    worker.send(BackgroundJob::StopFlashPaneBell(vec![A]));
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::ClearPaneFrameColorOverride(panes)] if panes == &[A]
    ));
    worker.advance(HALF_FLASH);
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::ClearPaneFrameColorOverride(panes)] if panes == &[B]
    ));
}

#[test]
fn overlapping_batches_only_clear_panes_they_still_own() {
    let worker = Worker::new();
    worker.send(BackgroundJob::FlashPaneBell(vec![A, B]));
    worker.advance(HALF_FLASH);
    worker.send(BackgroundJob::FlashPaneBell(vec![B, C]));
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::FlashPaneFrameColor(first), ScreenInstruction::FlashPaneFrameColor(second)]
            if first == &[A, B] && second == &[B, C]
    ));
    worker.advance(HALF_FLASH);
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::ClearPaneFrameColorOverride(panes)] if panes == &[A]
    ));
    worker.advance(HALF_FLASH);
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::ClearPaneFrameColorOverride(panes)] if panes == &[B, C]
    ));
}

#[test]
fn repeated_pane_flashes_last_until_the_latest_timer() {
    let worker = Worker::new();
    for _ in 0..2 {
        worker.send(BackgroundJob::FlashPaneBell(vec![A]));
        assert!(matches!(
            worker.screen_events().as_slice(),
            [ScreenInstruction::FlashPaneFrameColor(panes)] if panes == &[A]
        ));
        worker.advance(HALF_FLASH);
        assert!(worker.screen_events().is_empty());
    }
    worker.advance(HALF_FLASH);
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::ClearPaneFrameColorOverride(panes)] if panes == &[A]
    ));
}

#[test]
fn repeated_tab_flashes_last_until_the_latest_timer() {
    let worker = Worker::new();
    for _ in 0..2 {
        worker.send(BackgroundJob::FlashTabBell(TAB));
        assert!(matches!(
            worker.screen_events().as_slice(),
            [ScreenInstruction::SetTabBellFlash(TAB, true)]
        ));
        worker.advance(HALF_FLASH);
        assert!(worker.screen_events().is_empty());
    }
    worker.advance(HALF_FLASH);
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::SetTabBellFlash(TAB, false)]
    ));
}

#[test]
fn stopped_flashes_cannot_restart_when_their_timer_first_runs() {
    let worker = Worker::new();
    for job in [
        BackgroundJob::FlashPaneBell(vec![A]),
        BackgroundJob::FlashTabBell(TAB),
        BackgroundJob::StopFlashPaneBell(vec![A]),
        BackgroundJob::StopFlashTabBell(TAB),
    ] {
        worker.jobs.as_ref().unwrap().send(job).unwrap();
    }
    worker.sync();
    assert!(matches!(
        worker.screen_events().as_slice(),
        [ScreenInstruction::FlashPaneFrameColor(started),
         ScreenInstruction::SetTabBellFlash(TAB, true),
         ScreenInstruction::ClearPaneFrameColorOverride(stopped),
         ScreenInstruction::SetTabBellFlash(TAB, false)]
            if started == &[A] && stopped == &[A]
    ));
    worker.advance(FLASH_DURATION_MS + 1);
    assert!(worker.screen_events().is_empty());
}

#[test]
fn periodic_tasks_are_joined_on_exit_and_receive_error() {
    for disconnect in [false, true] {
        let mut worker = Worker::new();
        worker.advance(1001);
        assert!(matches!(
            worker.screen_events().as_slice(),
            [ScreenInstruction::SerializeLayoutForResurrection]
        ));
        assert!(matches!(
            worker.pty.try_recv().unwrap().0,
            PtyInstruction::UpdateAndReportCwds
        ));
        worker.stop(disconnect);
        assert_eq!(worker.runtime.metrics().num_alive_tasks(), 0);
        worker.advance(5000);
        assert!(worker.screen_events().is_empty());
        assert!(worker.pty.try_recv().is_err());
    }
}

#[test]
fn shutdown_cancels_flashes_animation_pings_and_palette_on_both_paths() {
    for disconnect in [false, true] {
        let mut worker = Worker::new();
        worker.send(BackgroundJob::FlashPaneBell(vec![A, B]));
        worker.send(BackgroundJob::FlashTabBell(TAB));
        worker.send(BackgroundJob::StartAnimationClock(Duration::from_millis(
            50,
        )));
        worker.send(BackgroundJob::StartNestedGuestPing(A));
        assert!(matches!(
            worker.screen_events().as_slice(),
            [ScreenInstruction::FlashPaneFrameColor(panes),
             ScreenInstruction::SetTabBellFlash(TAB, true),
             ScreenInstruction::AnimationTick,
             ScreenInstruction::NestedGuestPingTick { pane_id: A }]
                if panes == &[A, B]
        ));
        // Queue the palette before shutdown without driving a scan of real session directories.
        worker
            .jobs
            .as_ref()
            .unwrap()
            .send(BackgroundJob::WatchCommandPaletteSessions(true))
            .unwrap();
        worker.stop(disconnect);
        worker.advance(crate::nested_guest::ping_interval_ms().max(FLASH_DURATION_MS) + 1001);
        assert!(worker.screen_events().is_empty());
        assert_eq!(worker.runtime.metrics().num_alive_tasks(), 0);
    }
}
