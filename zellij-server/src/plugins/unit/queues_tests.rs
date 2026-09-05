use super::*;
use std::collections::BTreeMap;

fn run_plugin(url: &str) -> RunPluginOrAlias {
    RunPluginOrAlias::from_url(url, &None, None, None).expect("a valid plugin url")
}

fn enqueue(queues: &mut Queues, queue: &str, job: &str) -> JobId {
    let mut context = BTreeMap::new();
    context.insert("job".to_owned(), job.to_owned());
    queues.enqueue(
        queue,
        (crate::PluginId(1), crate::ClientId(1)),
        run_plugin(&format!("file:/plugins/{job}.lua")),
        context,
        None,
    )
}

/// Pop and launch every job the queue will give us, assigning plugin ids from `next_pid`.
fn launch_all(queues: &mut Queues, queue: &str, next_pid: &mut u32) -> Vec<JobId> {
    let mut launched = vec![];
    while let Some(job) = queues.pop_launchable(queue) {
        launched.push(job.job_id);
        queues.on_job_launched(job, crate::PluginId(*next_pid));
        *next_pid += 1;
    }
    launched
}

#[test]
fn an_unconfigured_queue_runs_one_job_at_a_time() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![1]);
    let snapshot = queues.queue_snapshot("merge").unwrap();
    assert_eq!(snapshot.concurrency, DEFAULT_CONCURRENCY);
    assert_eq!(snapshot.running.len(), 1);
    assert_eq!(snapshot.pending.len(), 1);
    assert_eq!(snapshot.running[0].plugin_id, Some(10));
    assert_eq!(snapshot.running[0].state, QueueJobState::Running);
    assert_eq!(snapshot.pending[0].state, QueueJobState::Pending);
    assert_eq!(snapshot.pending[0].plugin_id, None);
}

#[test]
fn job_ids_are_monotonic_and_never_reused() {
    let mut queues = Queues::default();
    let first = enqueue(&mut queues, "merge", "a");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    queues.on_plugin_unloaded(crate::PluginId(10));
    let second = enqueue(&mut queues, "other", "b");
    assert_eq!((first, second), (1, 2));
}

#[test]
fn pop_launchable_honours_concurrency_and_fifo_order() {
    let mut queues = Queues::default();
    queues.configure("merge", 2);
    for job in ["a", "b", "c"] {
        enqueue(&mut queues, "merge", job);
    }
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![1, 2]);
    assert!(queues.pop_launchable("merge").is_none());
    let snapshot = queues.queue_snapshot("merge").unwrap();
    assert_eq!(snapshot.pending[0].context["job"], "c");
}

#[test]
fn finishing_a_job_frees_its_slot() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    assert!(queues.record_outcome(
        crate::PluginId(10),
        QueueJobOutcome::Succeeded,
        Some("done".to_owned())
    ));
    let finished = queues.on_plugin_unloaded(crate::PluginId(10)).unwrap();
    assert_eq!(finished.job_id, 1);
    assert_eq!(finished.outcome, QueueJobOutcome::Succeeded);
    assert_eq!(finished.payload.as_deref(), Some("done"));
    assert_eq!(finished.context["job"], "a");
    assert_eq!(finished.enqueuer, (PluginId(1), ClientId(1)));
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![2]);
}

#[test]
fn raising_concurrency_releases_pending_jobs() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![1]);
    queues.configure("merge", 2);
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![2]);
}

#[test]
fn lowering_concurrency_evicts_nothing_and_zero_means_one() {
    let mut queues = Queues::default();
    queues.configure("merge", 3);
    for job in ["a", "b", "c", "d"] {
        enqueue(&mut queues, "merge", job);
    }
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![1, 2, 3]);
    queues.configure("merge", 0);
    let snapshot = queues.queue_snapshot("merge").unwrap();
    assert_eq!(snapshot.concurrency, 1);
    assert_eq!(snapshot.running.len(), 3);
    assert!(queues.pop_launchable("merge").is_none());
    queues.on_plugin_unloaded(crate::PluginId(10));
    queues.on_plugin_unloaded(crate::PluginId(11));
    assert!(
        queues.pop_launchable("merge").is_none(),
        "still at the limit"
    );
    queues.on_plugin_unloaded(crate::PluginId(12));
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![4]);
}

#[test]
fn cancelling_a_pending_job_drops_it() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    let second = enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    match queues.cancel(second) {
        CancelResult::DroppedPending(finished) => {
            assert_eq!(finished.job_id, second);
            assert_eq!(finished.outcome, QueueJobOutcome::Cancelled);
            assert_eq!(finished.payload, None);
        },
        other => panic!("expected DroppedPending, got {other:?}"),
    }
    assert!(queues.queue_snapshot("merge").unwrap().pending.is_empty());
}

#[test]
fn cancelling_a_running_job_asks_for_an_unload_and_reports_cancelled() {
    let mut queues = Queues::default();
    let first = enqueue(&mut queues, "merge", "a");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    assert_eq!(
        queues.cancel(first),
        CancelResult::UnloadRunning(crate::PluginId(10))
    );
    let finished = queues.on_plugin_unloaded(crate::PluginId(10)).unwrap();
    assert_eq!(finished.outcome, QueueJobOutcome::Cancelled);
}

#[test]
fn cancelling_an_unknown_job_is_not_found() {
    let mut queues = Queues::default();
    assert_eq!(queues.cancel(42), CancelResult::NotFound);
}

#[test]
fn a_job_that_unloads_without_reporting_is_aborted() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    let finished = queues.on_plugin_unloaded(crate::PluginId(10)).unwrap();
    assert_eq!(finished.outcome, QueueJobOutcome::Aborted);
    assert_eq!(finished.payload, None);
    assert_eq!(queues.queue_of_plugin(crate::PluginId(10)), None);
}

#[test]
fn unloading_a_plugin_that_is_not_a_job_is_nothing() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    assert_eq!(queues.on_plugin_unloaded(crate::PluginId(99)), None);
    assert!(!queues.record_outcome(crate::PluginId(99), QueueJobOutcome::Failed, None));
}

#[test]
fn a_failed_launch_never_takes_a_slot() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let job = queues.pop_launchable("merge").unwrap();
    let finished = queues.on_launch_failed(job, "no such file".to_owned());
    assert_eq!(finished.job_id, 1);
    assert_eq!(finished.outcome, QueueJobOutcome::Failed);
    assert_eq!(finished.payload.as_deref(), Some("no such file"));
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![2]);
}

#[test]
fn a_reused_plugin_id_belongs_to_the_new_job() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    launch_all(&mut queues, "merge", &mut pid);
    queues.on_plugin_unloaded(crate::PluginId(10));
    let job = queues.pop_launchable("merge").unwrap();
    queues.on_job_launched(job, crate::PluginId(10));
    assert_eq!(queues.queue_of_plugin(crate::PluginId(10)), Some("merge"));
    assert_eq!(
        queues
            .on_plugin_unloaded(crate::PluginId(10))
            .unwrap()
            .job_id,
        2
    );
}

#[test]
fn snapshots_cover_every_queue_or_just_one() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "deploy", "b");
    let all = queues.snapshot(None);
    assert_eq!(
        all.iter().map(|s| s.queue.as_str()).collect::<Vec<_>>(),
        vec!["deploy", "merge"]
    );
    let one = queues.snapshot(Some("merge"));
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].pending[0].plugin_url, "file:/plugins/a.lua");
    assert!(queues.snapshot(Some("nope")).is_empty());
}

mod properties {
    use super::*;
    use hegel::generators;

    #[derive(Debug, Clone)]
    enum Op {
        Enqueue,
        Configure(usize),
        LaunchAll,
        Finish(usize),
        Cancel(u64),
    }

    fn any_op(tc: &hegel::TestCase) -> Op {
        match tc.draw(generators::integers::<u8>().max_value(4)) {
            0 => Op::Enqueue,
            1 => Op::Configure(tc.draw(generators::integers::<usize>().max_value(4))),
            2 => Op::LaunchAll,
            3 => Op::Finish(tc.draw(generators::integers::<usize>().max_value(3))),
            _ => Op::Cancel(tc.draw(generators::integers::<u64>().max_value(8))),
        }
    }

    fn running_and_limit(queues: &Queues) -> (usize, usize) {
        queues
            .queue_snapshot("q")
            .map(|s| (s.running.len(), s.concurrency))
            .unwrap_or((0, DEFAULT_CONCURRENCY))
    }

    /// Lowering the limit below the running count evicts nothing, so `running <= concurrency`
    /// is not an invariant. What is: a launch only ever fills free slots, and a job ends once.
    #[hegel::test]
    fn launches_only_fill_free_slots_and_every_job_ends_once(tc: hegel::TestCase) {
        let mut queues = Queues::default();
        let mut next_pid: u32 = 100;
        let mut enqueued = 0u64;
        let mut ended: Vec<JobId> = vec![];
        let ops: Vec<Op> = (0..tc.draw(generators::integers::<usize>().max_value(24)))
            .map(|_| any_op(&tc))
            .collect();
        for op in ops {
            match op {
                Op::Enqueue => {
                    enqueue(&mut queues, "q", "job");
                    enqueued += 1;
                },
                Op::Configure(n) => queues.configure("q", n),
                Op::LaunchAll => {
                    let (running_before, limit) = running_and_limit(&queues);
                    let launched = launch_all(&mut queues, "q", &mut next_pid);
                    assert!(
                        launched.len() <= limit.saturating_sub(running_before),
                        "launched {} with {running_before} running at limit {limit}",
                        launched.len()
                    );
                },
                Op::Finish(nth) => {
                    let running: Vec<PluginId> = queues
                        .queue_snapshot("q")
                        .map(|s| {
                            s.running
                                .iter()
                                .filter_map(|j| j.plugin_id)
                                .map(PluginId)
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(pid) = running.get(nth % running.len().max(1)) {
                        if let Some(finished) = queues.on_plugin_unloaded(*pid) {
                            ended.push(finished.job_id);
                        }
                    }
                },
                Op::Cancel(job_id) => match queues.cancel(job_id) {
                    CancelResult::DroppedPending(finished) => ended.push(finished.job_id),
                    CancelResult::UnloadRunning(pid) => {
                        let finished = queues.on_plugin_unloaded(pid).unwrap();
                        assert_eq!(finished.outcome, QueueJobOutcome::Cancelled);
                        ended.push(finished.job_id);
                    },
                    CancelResult::NotFound => {},
                },
            }
        }
        let mut seen = ended.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ended.len(), "a job ended twice: {ended:?}");
        assert!(ended.iter().all(|id| *id >= 1 && *id <= enqueued));
    }
}

#[test]
fn shut_down_drops_pending_jobs_and_launches_nothing_more() {
    let mut queues = Queues::default();
    enqueue(&mut queues, "merge", "a");
    enqueue(&mut queues, "merge", "b");
    let mut pid = 10;
    assert_eq!(launch_all(&mut queues, "merge", &mut pid), vec![1]);

    queues.shut_down();
    assert!(queues.is_shut_down());
    assert!(queues.queue_snapshot("merge").unwrap().pending.is_empty());

    // the running job still ends the normal way when the host unloads it...
    let finished = queues.on_plugin_unloaded(crate::PluginId(10)).unwrap();
    assert_eq!(finished.job_id, 1);
    assert_eq!(finished.outcome, QueueJobOutcome::Aborted);
    // ...but nothing takes its slot, not even a job enqueued afterwards
    assert!(queues.pop_launchable("merge").is_none());
    enqueue(&mut queues, "merge", "c");
    assert!(queues.pop_launchable("merge").is_none());
}
