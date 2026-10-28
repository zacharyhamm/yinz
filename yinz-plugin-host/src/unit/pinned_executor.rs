use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

/// The executor's shared state as `create_test_dependencies` builds it, in the order a
/// job receives it.
type JobDependencies = (
    ThreadSenders,
    Arc<Mutex<PluginMap>>,
    Arc<Mutex<Vec<ClientId>>>,
);

// Test fixtures
fn create_test_dependencies() -> JobDependencies {
    use yinz_runtime::channels::{self, SenderWithContext};

    let (send_to_pty, _receive_pty) = channels::bounded(1);
    let (send_to_screen, _receive_screen) = channels::bounded(1);
    let (send_to_plugin, _receive_plugin) = channels::bounded(1);
    let (send_to_server, _receive_server) = channels::bounded(1);
    let (send_to_pty_writer, _receive_pty_writer) = channels::bounded(1);
    let (send_to_background_jobs, _receive_background_jobs) = channels::bounded(1);

    let to_pty = SenderWithContext::new(send_to_pty);
    let to_screen = SenderWithContext::new(send_to_screen);
    let to_plugin = SenderWithContext::new(send_to_plugin);
    let to_server = SenderWithContext::new(send_to_server);
    let to_pty_writer = SenderWithContext::new(send_to_pty_writer);
    let to_background_jobs = SenderWithContext::new(send_to_background_jobs);

    let senders = ThreadSenders {
        to_pty: Some(to_pty),
        to_screen: Some(to_screen),
        to_plugin: Some(to_plugin),
        to_server: Some(to_server),
        to_pty_writer: Some(to_pty_writer),
        to_background_jobs: Some(to_background_jobs),
        session: Default::default(),
        session_checkpoint: Default::default(),
        session_ownership: None,
        should_silently_fail: false,
    };

    let plugin_map = Arc::new(Mutex::new(PluginMap::default()));
    let connected_clients = Arc::new(Mutex::new(vec![]));

    (senders, plugin_map, connected_clients)
}

fn create_test_executor(max_threads: usize) -> Arc<PinnedExecutor> {
    let (senders, plugin_map, clients) = create_test_dependencies();
    Arc::new(PinnedExecutor::new(
        max_threads,
        &senders,
        &plugin_map,
        &clients,
    ))
}

// Helper to create a job that signals completion via channel
fn make_signaling_job(tx: Sender<()>) -> JobWork {
    Box::new(move |_senders, _plugin_map, _clients| {
        tx.send(()).unwrap();
    })
}

// Helper to verify thread assignment by capturing thread name in job
fn get_thread_name_for_plugin(executor: &Arc<PinnedExecutor>, plugin_id: PluginId) -> String {
    let (tx, rx) = channel();
    executor.execute_for_plugin(plugin_id, move |_s, _p, _c| {
        let name = thread::current().name().unwrap().to_string();
        tx.send(name).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("Thread name should be received")
}

#[test]
fn test_new_creates_one_thread() {
    let executor = create_test_executor(4);
    assert_eq!(executor.thread_count(), 1);
}

#[test]
fn test_new_respects_min_threads() {
    let executor = create_test_executor(0);
    assert_eq!(
        executor.thread_count(),
        1,
        "Executor should enforce minimum of 1 thread"
    );
}

#[test]
fn test_first_plugin_assigned_to_thread_zero() {
    let executor = create_test_executor(4);
    let thread_idx = executor.register_plugin(PluginId::new(1));
    assert_eq!(thread_idx, 0);
}

#[test]
fn test_plugin_name_does_not_wait_for_plugin_map() {
    let (_senders, plugin_map, _clients) = create_test_dependencies();
    let plugin_map_guard = plugin_map.lock().unwrap();
    let plugin_map_clone = plugin_map.clone();
    let (tx, rx) = channel();
    let lookup = thread::spawn(move || {
        tx.send(plugin_name(&plugin_map_clone, PluginId::new(42)))
            .unwrap()
    });

    let name = rx.recv_timeout(Duration::from_secs(1));
    drop(plugin_map_guard);
    lookup.join().unwrap();

    assert_eq!(name.unwrap(), "plugin[42]");
}

#[test]
fn test_plugins_spread_across_threads_before_sharing() {
    // A plugin's jobs queue strictly behind its thread-mates', so plugins
    // only share a thread once every thread has a tenant. The old policy
    // put everything registered at an idle moment on thread 0.
    let executor = create_test_executor(3);
    assert_eq!(executor.register_plugin(PluginId::new(1)), 0);
    assert_eq!(
        executor.register_plugin(PluginId::new(2)),
        1,
        "own thread while one can be spawned"
    );
    assert_eq!(executor.register_plugin(PluginId::new(3)), 2);
    let fourth = executor.register_plugin(PluginId::new(4));
    assert!(
        fourth <= 2,
        "at capacity, the least-loaded thread is shared"
    );
    assert_eq!(executor.thread_count(), 3, "never exceeds max_threads");
}

#[test]
fn test_vacant_thread_slot_is_reused_before_sharing() {
    let executor = create_test_executor(2);
    assert_eq!(executor.register_plugin(PluginId::new(1)), 0);
    assert_eq!(executor.register_plugin(PluginId::new(2)), 1);

    executor.unregister_plugin(PluginId::new(2));

    assert_eq!(executor.thread_count(), 1);
    assert_eq!(executor.register_plugin(PluginId::new(3)), 1);
    assert_eq!(executor.thread_count(), 2);
}

#[test]
fn test_input_jobs_run_before_other_plugins_queued_work() {
    // One thread, two plugins. While the thread is held, plugin 2 queues a
    // pile of work and plugin 1 queues a normal job and then an input job.
    // The input job makes plugin 1's *whole queue* go first — but never
    // reorders plugin 1's own jobs, which is the per-plugin FIFO the
    // batching tests pin.
    let executor = create_test_executor(1);
    executor.register_plugin(PluginId::new(1));
    executor.register_plugin(PluginId::new(2));

    let (started_tx, started_rx) = channel();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(2), move |_s, _p, _c| {
        started_tx.send(()).unwrap();
        barrier_clone.wait();
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the blocker job should start");

    let order = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let queue = |plugin_id: PluginId, label: &'static str, is_input: bool| {
        let order = order.clone();
        let tx = tx.clone();
        executor.execute_for_plugin_tagged(plugin_id, label, is_input, move |_s, _p, _c| {
            order.lock().unwrap().push(label);
            tx.send(()).unwrap();
        });
    };
    queue(PluginId::new(2), "b1", false);
    queue(PluginId::new(2), "b2", false);
    queue(PluginId::new(1), "a1", false);
    queue(PluginId::new(1), "a2-input", true);
    queue(PluginId::new(2), "b3", false);
    queue(PluginId::new(1), "a3", false);
    queue(PluginId::new(1), "a4-input", true);

    barrier.wait();
    for _ in 0..7 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("queued job should complete");
    }

    assert_eq!(
        *order.lock().unwrap(),
        vec!["a1", "a2-input", "a3", "a4-input", "b1", "b2", "b3"],
        "the input-bearing plugin's queue runs first, in its own order"
    );
}

#[test]
fn test_without_input_jobs_the_queue_stays_fifo() {
    let executor = create_test_executor(1);
    executor.register_plugin(PluginId::new(1));
    executor.register_plugin(PluginId::new(2));

    let (started_tx, started_rx) = channel();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(2), move |_s, _p, _c| {
        started_tx.send(()).unwrap();
        barrier_clone.wait();
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the blocker job should start");

    let order = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let queue = |plugin_id: PluginId, label: &'static str| {
        let order = order.clone();
        let tx = tx.clone();
        executor.execute_for_plugin_tagged(plugin_id, label, false, move |_s, _p, _c| {
            order.lock().unwrap().push(label);
            tx.send(()).unwrap();
        });
    };
    queue(PluginId::new(2), "b1");
    queue(PluginId::new(1), "a1");
    queue(PluginId::new(2), "b2");

    barrier.wait();
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("queued job should complete");
    }

    assert_eq!(
        *order.lock().unwrap(),
        vec!["b1", "a1", "b2"],
        "no input anywhere means plain arrival order"
    );
}

#[test]
fn test_new_thread_spawns_when_all_busy() {
    let executor = create_test_executor(3);

    // Register plugin 1, gets thread 0
    let thread_idx1 = executor.register_plugin(PluginId::new(1));
    assert_eq!(thread_idx1, 0);

    // Make thread 0 busy with a barrier
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });

    // Give the job a moment to start executing and block
    thread::sleep(Duration::from_millis(50));

    // Register plugin 2 while thread 0 is busy
    let thread_idx2 = executor.register_plugin(PluginId::new(2));
    assert_eq!(
        thread_idx2, 1,
        "Plugin 2 should get new thread 1 when thread 0 is busy"
    );

    // Verify thread count
    assert_eq!(executor.thread_count(), 2);

    // Release barrier
    barrier.wait();
}

#[test]
fn test_respects_max_threads_limit() {
    let executor = create_test_executor(2);

    // Register plugin 1, gets thread 0
    executor.register_plugin(PluginId::new(1));

    // Make thread 0 busy
    let barrier1 = Arc::new(Barrier::new(2));
    let barrier1_clone = barrier1.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier1_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    // Register plugin 2, gets thread 1
    executor.register_plugin(PluginId::new(2));

    // Make thread 1 busy
    let barrier2 = Arc::new(Barrier::new(2));
    let barrier2_clone = barrier2.clone();
    executor.execute_for_plugin(PluginId::new(2), move |_s, _p, _c| {
        barrier2_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    // Register plugin 3 when all threads busy
    let thread_idx3 = executor.register_plugin(PluginId::new(3));
    assert!(
        thread_idx3 == 0 || thread_idx3 == 1,
        "Plugin 3 should be assigned to existing thread"
    );
    assert_eq!(executor.thread_count(), 2, "Should not exceed max_threads");

    // Release barriers
    barrier1.wait();
    barrier2.wait();
}

#[test]
fn test_duplicate_registration_returns_same_thread() {
    let executor = create_test_executor(4);
    let thread_idx1 = executor.register_plugin(PluginId::new(1));
    let thread_idx2 = executor.register_plugin(PluginId::new(1));
    assert_eq!(
        thread_idx1, thread_idx2,
        "Duplicate registration should return same thread"
    );
}

#[test]
fn test_load_balancing_prefers_least_loaded() {
    let executor = create_test_executor(2);

    // The first two get threads of their own.
    assert_eq!(executor.register_plugin(PluginId::new(1)), 0);
    assert_eq!(executor.register_plugin(PluginId::new(2)), 1);

    // At capacity, each newcomer lands on the least-loaded thread, so the
    // tenancy stays balanced.
    let third = executor.register_plugin(PluginId::new(3));
    let fourth = executor.register_plugin(PluginId::new(4));
    assert_ne!(
        third, fourth,
        "with one tenant each, consecutive registrations balance out"
    );
    assert_eq!(executor.thread_count(), 2, "never exceeds max_threads");
}

#[test]
fn test_execute_for_plugin_runs_on_correct_thread() {
    let executor = create_test_executor(3);

    // Register plugin 1 to thread 0
    executor.register_plugin(PluginId::new(1));

    // Make thread 0 busy to force plugin 2 to thread 1
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    // Register plugin 2 to thread 1
    executor.register_plugin(PluginId::new(2));

    // Release barrier
    barrier.wait();
    thread::sleep(Duration::from_millis(50));

    // Get thread names for both plugins
    let thread_name1 = get_thread_name_for_plugin(&executor, PluginId::new(1));
    let thread_name2 = get_thread_name_for_plugin(&executor, PluginId::new(2));

    assert_eq!(thread_name1, "plugin-exec-0");
    assert_eq!(thread_name2, "plugin-exec-1");
}

#[test]
fn test_execute_for_plugin_unregistered() {
    let executor = create_test_executor(4);
    let (tx, rx) = channel();

    // Execute job for unregistered plugin
    executor.execute_for_plugin(PluginId::new(999), make_signaling_job(tx));

    // Try to receive with timeout - should timeout
    let result = rx.recv_timeout(Duration::from_millis(100));
    assert!(
        result.is_err(),
        "Job for unregistered plugin should not execute"
    );
}

#[test]
fn test_job_execution_order_per_thread() {
    let executor = create_test_executor(4);
    executor.register_plugin(PluginId::new(1));

    let order = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();

    // Execute 3 jobs for plugin 1
    for i in 1..=3 {
        let order_clone = order.clone();
        let tx_clone = tx.clone();
        executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
            order_clone.lock().unwrap().push(i);
            tx_clone.send(()).unwrap();
        });
    }

    // Wait for all 3 jobs to complete
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Job should complete");
    }

    assert_eq!(
        *order.lock().unwrap(),
        vec![1, 2, 3],
        "Jobs should execute in order"
    );
}

#[test]
fn test_jobs_complete_successfully() {
    let executor = create_test_executor(4);
    executor.register_plugin(PluginId::new(1));

    let (tx, rx) = channel();
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx));

    let result = rx.recv_timeout(Duration::from_secs(5));
    assert!(result.is_ok(), "Job should complete successfully");
}

#[test]
fn test_concurrent_jobs_on_different_threads() {
    let executor = create_test_executor(3);

    // Register plugin 1 to thread 0
    executor.register_plugin(PluginId::new(1));

    // Make thread 0 busy to force plugin 2 to thread 1
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    // Register plugin 2 to thread 1
    executor.register_plugin(PluginId::new(2));

    // Release barrier
    barrier.wait();
    thread::sleep(Duration::from_millis(50));

    // Execute jobs on both threads concurrently
    let sync_barrier = Arc::new(Barrier::new(3)); // 2 jobs + test thread
    let (tx1, rx1) = channel();
    let (tx2, rx2) = channel();

    let barrier1 = sync_barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier1.wait();
        tx1.send(()).unwrap();
    });

    let barrier2 = sync_barrier.clone();
    executor.execute_for_plugin(PluginId::new(2), move |_s, _p, _c| {
        barrier2.wait();
        tx2.send(()).unwrap();
    });

    // Release both jobs simultaneously
    sync_barrier.wait();

    // Both should complete
    assert!(rx1.recv_timeout(Duration::from_secs(5)).is_ok());
    assert!(rx2.recv_timeout(Duration::from_secs(5)).is_ok());
}

#[test]
fn test_execute_plugin_load_registers_and_executes() {
    let executor = create_test_executor(4);

    let (tx, rx) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx));

    // Wait for load to complete
    rx.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Verify plugin is registered
    let thread_idx = executor.register_plugin(PluginId::new(1));
    assert_eq!(thread_idx, 0, "Plugin should already be registered");
}

#[test]
fn test_execute_plugin_unload_runs_cleanup_before_unregister() {
    let executor = create_test_executor(4);

    // Load plugin
    let (tx_load, rx_load) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx_load));
    rx_load
        .recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Unload plugin with cleanup
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let (tx_unload, rx_unload) = channel();

    executor.execute_plugin_unload(PluginId::new(1), move |_s, _p, _c| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        tx_unload.send(()).unwrap();
    });

    // Wait for unload to complete
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    // Verify cleanup ran
    assert_eq!(counter.load(Ordering::SeqCst), 1, "Cleanup should have run");

    // Give unregister a moment to complete
    thread::sleep(Duration::from_millis(100));

    // Plugin should be unregistered - registering again should assign new thread
    let thread_idx = executor.register_plugin(PluginId::new(1));
    assert_eq!(thread_idx, 0, "Plugin should be re-registered to thread 0");
}

#[test]
fn test_unload_sequence_is_correct() {
    let executor = create_test_executor(4);

    // Load plugin
    let (tx_load, rx_load) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx_load));
    rx_load
        .recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Unload with sequence tracking
    let sequence = Arc::new(Mutex::new(Vec::new()));
    let sequence_clone = sequence.clone();
    let (tx_unload, rx_unload) = channel();

    executor.execute_plugin_unload(PluginId::new(1), move |_s, _p, _c| {
        sequence_clone.lock().unwrap().push("cleanup");
        tx_unload.send(()).unwrap();
    });

    // Wait for unload to complete
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");
    sequence.lock().unwrap().push("after");

    assert_eq!(*sequence.lock().unwrap(), vec!["cleanup", "after"]);
}

/// `PluginBridge::unload_plugin` queues one cleanup per client of a plugin. Only
/// the last may go through `execute_plugin_unload`: the jobs are FIFO on the
/// plugin's thread, so every earlier cleanup (via `execute_for_plugin`) has
/// run by the time the last one unregisters the plugin.
#[test]
fn several_cleanups_all_run_when_only_the_last_unregisters() {
    let executor = create_test_executor(4);
    let (tx_load, rx_load) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx_load));
    rx_load
        .recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    let (tx, rx) = channel();
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx.clone()));
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx.clone()));
    executor.execute_plugin_unload(PluginId::new(1), make_signaling_job(tx));
    for n in 1..=3 {
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("cleanup {n} of 3 never ran"));
    }
    // The channel closing is the last job having dropped its sender.
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
}

/// The same contract with the input-priority pick in play: an input job for a
/// thread-mate must not let the unload overtake this plugin's earlier cleanups.
/// Per-plugin order is what `execute_plugin_unload`'s "last one unregisters"
/// relies on.
#[test]
fn cleanups_stay_ordered_when_a_thread_mate_has_input_waiting() {
    let executor = create_test_executor(1);
    executor.register_plugin(PluginId::new(1));
    executor.register_plugin(PluginId::new(2));

    let (started_tx, started_rx) = channel();
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(2), move |_s, _p, _c| {
        started_tx.send(()).unwrap();
        barrier_clone.wait();
    });
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the blocker job should start");

    let order = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let record = |label: &'static str| {
        let order = order.clone();
        let tx = tx.clone();
        move |_s: ThreadSenders, _p: Arc<Mutex<PluginMap>>, _c: Arc<Mutex<Vec<ClientId>>>| {
            order.lock().unwrap().push(label);
            tx.send(()).unwrap();
        }
    };
    executor.execute_for_plugin(PluginId::new(1), record("cleanup-1"));
    executor.execute_for_plugin_tagged(PluginId::new(2), "key", true, record("input-2"));
    executor.execute_for_plugin(PluginId::new(1), record("cleanup-2"));
    executor.execute_plugin_unload(PluginId::new(1), record("unload-1"));
    drop(tx);

    barrier.wait();
    for _ in 0..4 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("queued job should complete");
    }

    let order = order.lock().unwrap().clone();
    let position = |label: &str| order.iter().position(|l| *l == label).unwrap();
    assert!(
        position("cleanup-1") < position("cleanup-2")
            && position("cleanup-2") < position("unload-1"),
        "plugin 1's jobs must run in submission order, got {order:?}"
    );
}

/// The shape `unload_plugin` must avoid: once an unload job has finished, the
/// plugin is unregistered and a later job for it is dropped rather than run.
#[test]
fn a_job_submitted_after_the_unload_finished_is_dropped() {
    let executor = create_test_executor(4);
    let (tx_load, rx_load) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx_load));
    rx_load
        .recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    let (tx_unload, rx_unload) = channel();
    executor.execute_plugin_unload(PluginId::new(1), make_signaling_job(tx_unload));
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");
    // The unregister runs after the signal, on the same thread; wait for it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while executor
        .state
        .lock()
        .unwrap()
        .assignments
        .contains_key(&PluginId::new(1))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "plugin 1 never unregistered"
        );
        thread::yield_now();
    }

    let (tx_late, rx_late) = channel();
    executor.execute_plugin_unload(PluginId::new(1), make_signaling_job(tx_late));
    assert!(
        rx_late.recv_timeout(Duration::from_millis(500)).is_err(),
        "a job for an unregistered plugin must not run"
    );
}

#[test]
fn test_shrink_removes_idle_threads() {
    let executor = create_test_executor(4);

    // Load plugin 1 to thread 0
    let (tx1, rx1) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx1));
    rx1.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Make thread 0 busy to force plugin 2 to thread 1
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    // Load plugin 2 to thread 1
    let (tx2, rx2) = channel();
    executor.execute_plugin_load(PluginId::new(2), make_signaling_job(tx2));
    rx2.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Release barrier
    barrier.wait();
    thread::sleep(Duration::from_millis(50));

    let thread_count_before = executor.thread_count();
    assert!(thread_count_before >= 2, "Should have at least 2 threads");

    // Unload plugin 2
    let (tx_unload2, rx_unload2) = channel();
    executor.execute_plugin_unload(PluginId::new(2), make_signaling_job(tx_unload2));
    rx_unload2
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    // Give shrinking a moment to complete
    thread::sleep(Duration::from_millis(100));

    // Thread count should decrease after unloading
    let thread_count_after = executor.thread_count();
    assert!(
        thread_count_after < thread_count_before,
        "Idle threads should be removed"
    );
    assert!(thread_count_after >= 1, "Thread 0 should remain");
}

#[test]
fn test_thread_zero_never_removed() {
    let executor = create_test_executor(4);

    // Load a plugin and then unload it
    let (tx_load, rx_load) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx_load));
    rx_load
        .recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    let (tx_unload, rx_unload) = channel();
    executor.execute_plugin_unload(PluginId::new(1), make_signaling_job(tx_unload));
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    // Give shrinking a moment
    thread::sleep(Duration::from_millis(100));

    // Thread 0 should remain
    assert!(
        executor.thread_count() >= 1,
        "Thread 0 should never be removed"
    );
}

#[test]
fn test_active_threads_not_removed() {
    let executor = create_test_executor(4);

    // Load plugin 1 to thread 0
    let (tx1, rx1) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx1));
    rx1.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Force plugin 2 to thread 1 by making thread 0 busy
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    let (tx2, rx2) = channel();
    executor.execute_plugin_load(PluginId::new(2), make_signaling_job(tx2));
    rx2.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    barrier.wait();
    thread::sleep(Duration::from_millis(100));

    let thread_count_with_both = executor.thread_count();
    assert!(
        thread_count_with_both >= 2,
        "Should have at least 2 threads with 2 plugins"
    );

    // Unload plugin 2
    let (tx_unload, rx_unload) = channel();
    executor.execute_plugin_unload(PluginId::new(2), make_signaling_job(tx_unload));
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    thread::sleep(Duration::from_millis(100));

    // Plugin 1's thread should still work (verify active threads not affected)
    let (tx_test, rx_test) = channel();
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx_test));
    assert!(
        rx_test.recv_timeout(Duration::from_secs(5)).is_ok(),
        "Plugin 1's thread should still work"
    );

    // Thread count should decrease after unloading
    let thread_count_after = executor.thread_count();
    assert!(
        thread_count_after < thread_count_with_both,
        "Idle thread should be removed"
    );
    assert!(thread_count_after >= 1, "Active threads should remain");
}

#[test]
fn test_shrink_does_not_affect_remaining_threads() {
    let executor = create_test_executor(4);

    // Load plugin 1 to thread 0
    let (tx1, rx1) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx1));
    rx1.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Force plugin 2 to thread 1
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    let (tx2, rx2) = channel();
    executor.execute_plugin_load(PluginId::new(2), make_signaling_job(tx2));
    rx2.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    barrier.wait();
    thread::sleep(Duration::from_millis(50));

    // Unload plugin 2 (shrinks pool)
    let (tx_unload, rx_unload) = channel();
    executor.execute_plugin_unload(PluginId::new(2), make_signaling_job(tx_unload));
    rx_unload
        .recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    thread::sleep(Duration::from_millis(100));

    // Execute job for plugin 1
    let (tx_test, rx_test) = channel();
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx_test));

    assert!(
        rx_test.recv_timeout(Duration::from_secs(5)).is_ok(),
        "Plugin 1's thread should still work"
    );
}

#[test]
fn test_drop_cleans_up_gracefully() {
    let executor = create_test_executor(4);

    // Load multiple plugins on different threads
    let (tx1, rx1) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx1));
    rx1.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });
    thread::sleep(Duration::from_millis(50));

    let (tx2, rx2) = channel();
    executor.execute_plugin_load(PluginId::new(2), make_signaling_job(tx2));
    rx2.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    barrier.wait();

    // Drop executor
    drop(executor);

    // Test completes without panic
}

#[test]
fn test_drop_with_jobs_in_flight() {
    let executor = create_test_executor(4);
    executor.register_plugin(PluginId::new(1));

    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();
    executor.execute_for_plugin(PluginId::new(1), move |_s, _p, _c| {
        barrier_clone.wait();
    });

    // Drop executor while job is blocked
    drop(executor);

    // Release barrier (job may or may not complete, but shouldn't panic)
    barrier.wait();

    // Test completes without panic
}

#[test]
fn test_concurrent_plugin_registrations() {
    let executor = create_test_executor(4);

    let handles: Vec<_> = (1..=10)
        .map(|i| {
            let exec = executor.clone();
            thread::spawn(move || exec.register_plugin(PluginId::new(i)))
        })
        .collect();

    for handle in handles {
        let thread_idx = handle.join().expect("Thread should not panic");
        assert!(thread_idx < 4, "Thread index should be valid");
    }
}

#[test]
fn test_unregister_nonexistent_plugin() {
    let executor = create_test_executor(4);

    // Unregister non-existent plugin
    executor.unregister_plugin(PluginId::new(999));

    // Executor should still work
    let (tx, rx) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx));
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
}

#[test]
fn test_max_threads_one() {
    let executor = create_test_executor(1);

    // Register multiple plugins
    let thread_idx1 = executor.register_plugin(PluginId::new(1));
    let thread_idx2 = executor.register_plugin(PluginId::new(2));
    let thread_idx3 = executor.register_plugin(PluginId::new(3));

    assert_eq!(thread_idx1, 0);
    assert_eq!(thread_idx2, 0);
    assert_eq!(thread_idx3, 0);
    assert_eq!(executor.thread_count(), 1);
}

#[test]
fn test_rapid_load_unload_cycles() {
    let executor = create_test_executor(4);

    // Load plugin 1
    let (tx1, rx1) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx1));
    rx1.recv_timeout(Duration::from_secs(5))
        .expect("Load should complete");

    // Unload plugin 1
    let (tx2, rx2) = channel();
    executor.execute_plugin_unload(PluginId::new(1), make_signaling_job(tx2));
    rx2.recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    thread::sleep(Duration::from_millis(100));

    // Load plugin 1 again
    let (tx3, rx3) = channel();
    executor.execute_plugin_load(PluginId::new(1), make_signaling_job(tx3));
    rx3.recv_timeout(Duration::from_secs(5))
        .expect("Second load should complete");

    // Execute job for plugin 1
    let (tx4, rx4) = channel();
    executor.execute_for_plugin(PluginId::new(1), make_signaling_job(tx4));
    assert!(
        rx4.recv_timeout(Duration::from_secs(5)).is_ok(),
        "Executor should handle cycles correctly"
    );
}

#[test]
fn test_many_plugins_limited_threads() {
    let executor = create_test_executor(4);
    let (tx, rx) = channel();

    // Load 20 plugins
    for i in 1..=20 {
        let tx_clone = tx.clone();
        executor.execute_plugin_load(PluginId::new(i), make_signaling_job(tx_clone));
    }

    // Collect 20 completion signals
    for _ in 1..=20 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");
    }

    assert!(
        executor.thread_count() <= 4,
        "Should not exceed max_threads"
    );
}

#[test]
fn test_full_lifecycle() {
    let executor = create_test_executor(4);
    let (tx, rx) = channel();

    // Load 5 plugins
    for i in 1..=5 {
        let tx_clone = tx.clone();
        executor.execute_plugin_load(PluginId::new(i), make_signaling_job(tx_clone));
    }

    // Wait for all loads
    for _ in 0..5 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");
    }

    // Execute 2 jobs per plugin (10 total)
    for i in 1..=5 {
        for _ in 0..2 {
            let tx_clone = tx.clone();
            executor.execute_for_plugin(PluginId::new(i), make_signaling_job(tx_clone));
        }
    }

    // Wait for all jobs
    for _ in 0..10 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Job should complete");
    }

    let thread_count_before = executor.thread_count();

    // Unload 3 plugins
    for i in 1..=3 {
        let tx_clone = tx.clone();
        executor.execute_plugin_unload(PluginId::new(i), make_signaling_job(tx_clone));
    }

    // Wait for unloads
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");
    }

    thread::sleep(Duration::from_millis(100));

    // Thread count should decrease or stay the same
    let thread_count_after = executor.thread_count();
    assert!(
        thread_count_after <= thread_count_before,
        "Thread count should decrease after unloads"
    );

    // Execute jobs for remaining plugins
    for i in 4..=5 {
        let tx_clone = tx.clone();
        executor.execute_for_plugin(PluginId::new(i), make_signaling_job(tx_clone));
    }

    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Job should complete");
    }

    // Drop executor
    drop(executor);
}

#[test]
fn test_realistic_plugin_churn() {
    let executor = create_test_executor(4);
    let (tx, rx) = channel();

    // Load plugins 1, 2, 3
    for i in 1..=3 {
        let tx_clone = tx.clone();
        executor.execute_plugin_load(PluginId::new(i), make_signaling_job(tx_clone));
    }
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");
    }

    // Execute jobs for each
    for i in 1..=3 {
        let tx_clone = tx.clone();
        executor.execute_for_plugin(PluginId::new(i), make_signaling_job(tx_clone));
    }
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Job should complete");
    }

    // Unload plugin 2
    let tx_clone = tx.clone();
    executor.execute_plugin_unload(PluginId::new(2), make_signaling_job(tx_clone));
    rx.recv_timeout(Duration::from_secs(5))
        .expect("Unload should complete");

    thread::sleep(Duration::from_millis(100));

    // Load plugins 4, 5
    for i in 4..=5 {
        let tx_clone = tx.clone();
        executor.execute_plugin_load(PluginId::new(i), make_signaling_job(tx_clone));
    }
    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");
    }

    // Execute jobs for plugins 1, 3, 4, 5
    for i in &[1, 3, 4, 5] {
        let tx_clone = tx.clone();
        executor.execute_for_plugin(PluginId::new(*i), make_signaling_job(tx_clone));
    }
    for _ in 0..4 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Job should complete");
    }

    // Unload plugins 1, 3
    for i in &[1, 3] {
        let tx_clone = tx.clone();
        executor.execute_plugin_unload(PluginId::new(*i), make_signaling_job(tx_clone));
    }
    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");
    }

    thread::sleep(Duration::from_millis(100));

    // Verify thread count reflects active plugins (4 and 5)
    assert!(
        executor.thread_count() >= 1,
        "Should have at least thread 0"
    );

    drop(executor);
}

/// The regression for the ABBA deadlock: `register_plugin` and the shrink path
/// once took two mutexes in opposite orders. Hammering both from several threads
/// would wedge under the old design; the watchdog turns that hang into a failure
/// instead of letting the test run forever.
///
/// Every registration here lands on an empty thread while one can be spawned, so
/// the growth, vacant-slot reuse and reclaim paths — the ones the shrink lock
/// used to guard — are all exercised and the cap assertion is not vacuous.
#[test]
fn concurrent_register_and_unregister_do_not_deadlock() {
    let executor = create_test_executor(4);
    let barrier = Arc::new(Barrier::new(4));
    let (done_tx, done_rx) = channel();

    let mut handles = Vec::new();
    for t in 0..4u32 {
        let exec = executor.clone();
        let barrier = barrier.clone();
        let done_tx = done_tx.clone();
        handles.push(thread::spawn(move || {
            barrier.wait(); // start all four at once, to maximise contention
            for i in 0..250u32 {
                let plugin_id = t * 10_000 + i;
                exec.register_plugin(PluginId::new(plugin_id));
                exec.unregister_plugin(PluginId::new(plugin_id));
            }
            done_tx.send(t).unwrap();
        }));
    }
    drop(done_tx);

    for _ in 0..4 {
        done_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("register/unregister deadlocked");
    }
    for h in handles {
        h.join().expect("a worker thread panicked");
    }
    assert!(executor.thread_count() >= 1);
    assert!(
        executor.thread_count() <= 4,
        "the pool never exceeds its cap"
    );
}

/// The regression for sparse-pool starvation: a reclaimed thread left a `None`
/// tombstone that the old length-based growth never reused, so `threads.len()`
/// ratcheted up to `max_threads` and later plugins were pinned onto thread 0.
/// The reclaimed slot must be reused on the next growth, and the pool must not
/// ratchet past its live count.
#[test]
fn a_reclaimed_thread_slot_is_reused_on_the_next_growth() {
    let executor = create_test_executor(4);

    assert_eq!(executor.register_plugin(PluginId::new(0)), 0);
    assert_eq!(
        executor.register_plugin(PluginId::new(1)),
        1,
        "a fresh thread is spawned while thread 0 has a tenant"
    );
    assert_eq!(executor.thread_count(), 2);

    // Reclaim thread 1 by unregistering its only plugin — leaves a `None` at 1.
    executor.unregister_plugin(PluginId::new(1));
    assert_eq!(executor.thread_count(), 1, "the idle thread is reclaimed");

    // Grow again: the reclaimed slot must be reused. The old growth returned
    // index 2.
    assert_eq!(
        executor.register_plugin(PluginId::new(2)),
        1,
        "the reclaimed slot must be reused, not grown past"
    );
    assert_eq!(executor.thread_count(), 2, "the pool did not ratchet up");
}
