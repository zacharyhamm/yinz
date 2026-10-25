use super::*;

const CLIENT: ClientId = ClientId::new(1);

#[test]
fn oversized_posts_count_retained_capacity_of_both_strings() {
    let (sender, receiver) = mpsc::channel();
    let used = Arc::default();
    for (name, payload) in [
        (String::with_capacity(POST_BUDGET), String::new()),
        (String::new(), String::with_capacity(POST_BUDGET)),
    ] {
        let error = post_job(&sender, &used, CLIENT, name, payload).unwrap_err();
        assert!(error.to_string().contains("64 MiB queue budget"));
        assert_eq!(used.load(Ordering::Relaxed), 0);
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn cumulative_posts_recover_capacity_on_dequeue_and_preserve_fifo() {
    let (sender, receiver) = mpsc::channel();
    let used = Arc::default();
    let post = |name: &str| {
        post_job(
            &sender,
            &used,
            CLIENT,
            name.into(),
            String::with_capacity(POST_BUDGET / 2 - POST_OVERHEAD - name.len()),
        )
    };
    post("first").unwrap();
    post("second").unwrap();
    assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
    assert!(post("refused").is_err());
    let Message::Job(client, name, payload, reservation) = receiver.recv().unwrap() else {
        panic!("expected post");
    };
    assert_eq!((client, name.as_str()), (CLIENT, "first"));
    drop(reservation);
    // The executing job may still own its payload; only queued jobs count.
    post("third").unwrap();
    drop(payload);
    for expected in ["second", "third"] {
        let Message::Job(_, name, _, _) = receiver.recv().unwrap() else {
            panic!("expected post");
        };
        assert_eq!(name, expected);
    }
    assert_eq!(used.load(Ordering::Relaxed), 0);
}

#[test]
fn empty_posts_are_bounded_and_shutdown_releases_the_queue() {
    let (sender, receiver) = mpsc::channel();
    let used = Arc::default();
    for _ in 0..POST_BUDGET / POST_OVERHEAD {
        post_job(&sender, &used, CLIENT, String::new(), String::new()).unwrap();
    }
    assert!(post_job(&sender, &used, CLIENT, String::new(), String::new()).is_err());
    assert_eq!(
        used.load(Ordering::Relaxed),
        POST_BUDGET / POST_OVERHEAD * POST_OVERHEAD
    );
    drop(receiver);
    assert_eq!(used.load(Ordering::Relaxed), 0);
}

#[test]
fn send_failure_releases_the_reservation() {
    let (sender, receiver) = mpsc::channel();
    let used = Arc::default();
    drop(receiver);
    for _ in 0..2 {
        let error = post_job(
            &sender,
            &used,
            CLIENT,
            String::new(),
            String::with_capacity(POST_BUDGET - POST_OVERHEAD),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stopped"));
        assert_eq!(used.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn concurrent_senders_cannot_exceed_the_budget() {
    let (sender, receiver) = mpsc::channel();
    let used = Arc::default();
    let accepted = AtomicUsize::new(0);
    let start = std::sync::Barrier::new(8);
    std::thread::scope(|threads| {
        for _ in 0..8 {
            threads.spawn(|| {
                start.wait();
                for _ in 0..16 {
                    if post_job(
                        &sender,
                        &used,
                        CLIENT,
                        String::new(),
                        String::with_capacity(POST_BUDGET / 64 - POST_OVERHEAD),
                    )
                    .is_ok()
                    {
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                    assert!(used.load(Ordering::Relaxed) <= POST_BUDGET);
                }
            });
        }
    });
    assert_eq!(accepted.load(Ordering::Relaxed), 64);
    assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
    assert_eq!(receiver.try_iter().count(), 64);
    assert_eq!(used.load(Ordering::Relaxed), 0);
}
