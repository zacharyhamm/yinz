use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[test]
fn discovery_is_progressive_bounded_and_joins_cancelled_workers() {
    let (started, starts) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = std::sync::Mutex::new(released);
    let finished = Arc::new(AtomicUsize::new(0));
    let done = finished.clone();
    let mut discovery = Discovery::start_with(
        (0..8).map(|n| n.to_string()).collect(),
        move |host, cancelled| {
            started.send(host.to_owned()).unwrap();
            if host == "0" {
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap();
                done.fetch_add(1, Ordering::SeqCst);
                return Ok(Vec::new());
            }
            while !cancelled.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(10));
            }
            done.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("cancelled")
        },
    );
    let mut first = (0..4)
        .map(|_| starts.recv_timeout(Duration::from_secs(30)).unwrap())
        .collect::<Vec<_>>();
    first.sort();
    assert_eq!(first, ["0", "1", "2", "3"]);
    assert!(starts.try_recv().is_err());
    assert!(discovery
        .rows(&[])
        .iter()
        .all(|row| row.status.as_deref() == Some("loading…")));
    release.send(()).unwrap();
    assert_eq!(starts.recv_timeout(Duration::from_secs(30)).unwrap(), "4");
    assert!(discovery.update());
    assert_eq!(
        discovery.rows(&[])[0].status.as_deref(),
        Some("no live sessions")
    );
    drop(discovery);
    assert_eq!(finished.load(Ordering::SeqCst), 5);
    assert!(starts.try_recv().is_err());
}
