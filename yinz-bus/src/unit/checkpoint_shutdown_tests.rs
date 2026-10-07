use super::*;
use yinz_runtime::channels::{unbounded, SenderWithContext};

#[test]
fn checkpoint_task_finishes_before_background_thread_on_exit_and_receive_error() {
    for disconnect in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (started, received) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let (jobs, receiver) = unbounded();
        let jobs = SenderWithContext::new(jobs);
        let mut bus = Bus::new(vec![receiver], None, None, None, None, None, None, None);
        bus.senders.should_silently_fail = true;
        bus.senders.session_checkpoint = Arc::new(SessionCheckpointWriter {
            cache_dir: root.path().to_owned(),
            name: Mutex::new(None),
            before_write: Some(Box::new(move || {
                started.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
            })),
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
