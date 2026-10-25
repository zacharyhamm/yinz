use super::*;

#[test]
fn fake_host_identity_does_not_wait_for_persistent_identity_io() {
    let data_dir = init().join("data");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(data_dir.join(".host-id.lock"))
        .unwrap();
    lock.lock().unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let identity_dir = data_dir.clone();
    let worker = std::thread::spawn(move || {
        completed
            .send(yinz_runtime::host_identity::init_host_id(
                &identity_dir,
                None,
            ))
            .unwrap();
    });
    let identity = completion.recv_timeout(yinz_test_support::default_timeout());
    drop(lock);
    worker.join().unwrap();
    assert_eq!(identity.unwrap().as_str(), "yinz-integration-test");
    assert!(!data_dir.join("host-id").exists());
}

#[test]
fn startup_evidence_survives_the_recent_log_tail() {
    let evidence = startup_and_tail("startup: opening database\nnoise\nlast", 1);
    assert!(evidence.contains("startup: opening database"));
    assert!(evidence.ends_with("last"));
    assert!(!evidence.contains("noise"));
}

#[test]
fn shutdown_panic_summaries_do_not_grow_recorded_history() {
    const PROBE_ENV: &str = "YINZ_TEST_PANIC_HISTORY_PROBE";
    if std::env::var_os(PROBE_ENV).is_some() {
        record_background_thread_panics();
        assert_no_background_panics();
        let worker = std::thread::Builder::new()
            .name("history_probe_worker".into())
            .spawn(|| panic!("original worker failure"))
            .unwrap();
        assert!(worker.join().is_err());

        let original = BACKGROUND_PANICS.lock().unwrap().clone();
        assert_eq!(original.len(), 1);
        assert!(original[0].contains("history_probe_worker"));
        assert!(original[0].contains("original worker failure"));
        let summary = format!(
            "background thread panicked during integration test:\n{}",
            original.join("\n")
        );
        for _ in 0..35 {
            let panic = std::panic::catch_unwind(assert_no_background_panics).unwrap_err();
            assert_eq!(*panic.downcast::<String>().unwrap(), summary);
            let recorded = BACKGROUND_PANICS.lock().unwrap().clone();
            assert_eq!(recorded, original);
        }
        return;
    }

    // A separate process keeps the deliberate worker panic out of sibling tests' history.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "test_env::tests::shutdown_panic_summaries_do_not_grow_recorded_history",
            "--nocapture",
        ])
        .env(PROBE_ENV, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "panic history probe failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
