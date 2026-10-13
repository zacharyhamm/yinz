use super::*;
use yinz_runtime::channels::{unbounded, SenderWithContext};

fn write_barrier(
    started: std::sync::mpsc::Sender<()>,
    released: std::sync::mpsc::Receiver<()>,
) -> Box<dyn Fn() + Send + Sync> {
    let released = Mutex::new(Some(released));
    Box::new(move || {
        // Catch-up writes must not consume another release during shutdown.
        if let Some(released) = released.lock().unwrap().take() {
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    })
}

#[test]
fn checkpoint_write_barrier_only_blocks_the_first_write() {
    let (started, received) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let barrier = write_barrier(started, released);
    release.send(()).unwrap();
    barrier();
    received.try_recv().unwrap();
    barrier();
    assert!(received.try_recv().is_err());
}

#[test]
fn checkpoint_task_finishes_before_background_thread_on_exit_and_receive_error() {
    for disconnect in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (started, received) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (jobs, receiver) = unbounded();
        let jobs = SenderWithContext::new(jobs);
        let mut bus = Bus::new(vec![receiver], None, None, None, None, None, None, None);
        bus.senders.should_silently_fail = true;
        bus.senders.session_checkpoint = Arc::new(SessionCheckpointWriter {
            cache_dir: root.path().to_owned(),
            name: Mutex::new(None),
            progress: Mutex::default(),
            before_write: Some(write_barrier(started, released)),
        });
        let worker = std::thread::spawn(move || background_jobs_main(bus, None, false));
        jobs.send(BackgroundJob::ReportSessionInfo(
            "owned".into(),
            SessionInfo::default(),
        ))
        .unwrap();
        received.recv_timeout(Duration::from_secs(10)).unwrap();
        if !disconnect {
            jobs.send(BackgroundJob::Exit).unwrap();
        }
        drop(jobs);
        let (finished, completion) = std::sync::mpsc::channel();
        let joiner = std::thread::spawn(move || finished.send(worker.join().unwrap()).unwrap());
        // The checkpoint is inside a synchronous write. Shutdown cannot finish until
        // it is released, even though the async task has received cancellation.
        assert!(matches!(
            completion.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        release.send(()).unwrap();
        assert_eq!(
            completion
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err(),
            disconnect
        );
        joiner.join().unwrap();
        assert!(root.path().join("owned/current").exists());
        fs::remove_dir_all(root.path().join("owned")).unwrap();
        assert!(!root.path().join("owned").exists());
    }
}
