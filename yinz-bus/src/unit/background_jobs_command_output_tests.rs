use super::*;
use tokio::io::AsyncWriteExt;

/// A payload no two windows of which are alike, so a reader that kept the wrong bytes -
/// the tail, or a middle - could not pass as one that kept the head.
fn distinguishable_payload() -> Vec<u8> {
    (0..100_000u32).map(|i| (i % 251) as u8).collect()
}

/// Runs the bounded reader against a writer that never stops early, and returns the output
/// once the writer has confirmed every byte went in - so a reader that dropped the pipe
/// before EOF fails as "not drained" rather than as a join error.
async fn read_bounded(payload: Vec<u8>, limit: usize) -> Vec<u8> {
    let (mut writer, reader) = tokio::io::duplex(8);
    let expected_len = payload.len();
    let write = tokio::spawn(async move {
        writer.write_all(&payload).await?;
        Ok::<usize, std::io::Error>(expected_len)
    });

    let output = tokio::time::timeout(
        Duration::from_secs(1),
        read_command_stream(reader, Some(limit), |_| {}),
    )
    .await
    .expect("the reader must reach EOF")
    .unwrap();
    let written = write
        .await
        .unwrap()
        .expect("the pipe was not drained: the reader hung up before EOF");
    assert_eq!(written, expected_len, "the pipe was not drained");
    output
}

#[tokio::test]
async fn bounded_command_output_is_truncated_and_drained() {
    let payload = distinguishable_payload();
    let limit = 40;
    let output = read_bounded(payload.clone(), limit).await;

    assert_eq!(output.len(), limit);
    let kept = limit - OUTPUT_TRUNCATED.len();
    assert_eq!(
        &output[..kept],
        &payload[..kept],
        "the kept bytes must be the first {kept} of the output"
    );
    assert!(output.ends_with(OUTPUT_TRUNCATED));
}

/// A limit smaller than the marker keeps only as much of the marker as fits, and never more
/// than the limit.
#[tokio::test]
async fn bounded_command_output_honours_a_limit_smaller_than_the_marker() {
    assert!(OUTPUT_TRUNCATED.len() > 5);
    for limit in [5, 0] {
        let output = read_bounded(distinguishable_payload(), limit).await;
        assert_eq!(output.len(), limit, "limit {limit}");
        assert_eq!(output, &OUTPUT_TRUNCATED[..limit], "limit {limit}");
    }
}

#[tokio::test]
async fn small_command_output_without_a_requested_limit_is_unchanged() {
    let (mut writer, reader) = tokio::io::duplex(8);
    let write = tokio::spawn(async move {
        writer.write_all(b"complete output").await.unwrap();
    });

    assert_eq!(
        read_command_stream(reader, None, |_| {}).await.unwrap(),
        b"complete output"
    );
    write.await.unwrap();
}

#[test]
fn streaming_spawn_failure_returns_one_correlated_completion() {
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let (jobs, receiver) = unbounded();
    let jobs = SenderWithContext::new(jobs);
    let (events, received) = unbounded();
    let events = SenderWithContext::new(events);
    let mut bus = Bus::new(
        vec![receiver],
        None,
        None,
        Some(&events),
        None,
        None,
        None,
        None,
    );
    bus.senders.should_silently_fail = true;
    let worker = std::thread::spawn(move || background_jobs_main(bus, None, true));
    let context = BTreeMap::from([("operation".to_owned(), "missing".to_owned())]);
    let directory = tempfile::tempdir().unwrap();
    jobs.send(BackgroundJob::RunCommand(
        PluginId::new(1),
        ClientId::new(1),
        directory
            .path()
            .join("absent-command")
            .display()
            .to_string(),
        vec![],
        BTreeMap::new(),
        directory.path().to_owned(),
        context.clone(),
        Some(64),
        Some(true),
        None,
    ))
    .unwrap();
    let (event, _) = received.recv_timeout(Duration::from_secs(10)).unwrap();
    jobs.send(BackgroundJob::Exit).unwrap();
    worker.join().unwrap().unwrap();
    match event {
        PluginInstruction::Update(events) => {
            assert_eq!(events.len(), 1);
            match &events[0].2 {
                Event::RunCommandResult(Some(2), stdout, stderr, result_context) => {
                    assert!(stdout.is_empty());
                    assert!(!stderr.is_empty());
                    assert_eq!(result_context, &context);
                },
                event => panic!("expected completion, got {event:?}"),
            }
        },
        event => panic!("expected plugin update, got {event:?}"),
    }
    assert!(received.try_recv().is_err());
}

#[test]
fn real_command_streams_before_exit_with_generation_and_drains_beyond_cap() {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let directory = tempfile::tempdir().unwrap();
    let fifo = directory.path().join("release");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut release = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    let (jobs, receiver) = unbounded();
    let jobs = SenderWithContext::new(jobs);
    let (events, received) = unbounded();
    let events = SenderWithContext::new(events);
    let mut bus = Bus::new(
        vec![receiver],
        None,
        None,
        Some(&events),
        None,
        None,
        None,
        None,
    );
    bus.senders.should_silently_fail = true;
    let worker = std::thread::spawn(move || background_jobs_main(bus, None, true));
    let context = BTreeMap::from([("operation".into(), "live".into())]);
    jobs.send(BackgroundJob::RunCommand(
        PluginId::new(1),
        ClientId::new(2),
        "/bin/sh".into(),
        vec![
            "-c".into(),
            "printf out; printf err >&2; read line < \"$1\"; printf tail; printf tail >&2".into(),
            "sh".into(),
            fifo.display().to_string(),
        ],
        BTreeMap::new(),
        directory.path().to_owned(),
        context.clone(),
        Some(2),
        Some(true),
        Some(42),
    ))
    .unwrap();
    let mut stdout = vec![];
    let mut stderr = vec![];
    let mut released = false;
    loop {
        let (instruction, _) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        let PluginInstruction::PluginEvent(plugin, client, generation, event) = instruction else {
            panic!("live output must carry its generation: {instruction:?}");
        };
        assert_eq!(
            (plugin, client, generation),
            (PluginId::new(1), ClientId::new(2), 42)
        );
        match event {
            Event::RunCommandOutput {
                data,
                stderr: is_stderr,
                context: actual,
            } => {
                assert_eq!(actual, context);
                if is_stderr {
                    stderr.extend(data);
                } else {
                    stdout.extend(data);
                }
                if !released && stdout == b"out" && stderr == b"err" {
                    // The child cannot exit until we observe both live chunks.
                    release.write_all(b"go\n").unwrap();
                    released = true;
                }
            },
            Event::RunCommandResult(Some(0), out, err, actual) => {
                assert!(released);
                assert_eq!(actual, context);
                assert!(out.len() <= 2 && err.len() <= 2);
                break;
            },
            event => panic!("unexpected event: {event:?}"),
        }
    }
    assert_eq!(stdout, b"outtail");
    assert_eq!(stderr, b"errtail");
    jobs.send(BackgroundJob::Exit).unwrap();
    worker.join().unwrap().unwrap();
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn chunks_arrive_before_exit_and_both_streams_drain_after_release() {
    let (mut out, stdout) = tokio::io::duplex(8);
    let (mut err, stderr) = tokio::io::duplex(8);
    let (chunks, mut received) = tokio::sync::mpsc::unbounded_channel();
    let reading = tokio::spawn(async move {
        tokio::join!(
            read_command_stream(stdout, Some(4), |data| {
                chunks.send((false, data.to_vec())).unwrap();
            }),
            read_command_stream(stderr, Some(4), |data| {
                chunks.send((true, data.to_vec())).unwrap();
            }),
        )
    });
    out.write_all(b"out").await.unwrap();
    err.write_all(b"err").await.unwrap();
    let mut first = vec![];
    for _ in 0..2 {
        first.push(
            tokio::time::timeout(Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    first.sort();
    assert_eq!(
        first,
        vec![(false, b"out".to_vec()), (true, b"err".to_vec())]
    );
    assert!(!reading.is_finished(), "output must precede stream closure");
    out.write_all(b"more").await.unwrap();
    err.write_all(b"more").await.unwrap();
    drop((out, err));
    let (stdout, stderr) = reading.await.unwrap();
    assert_eq!(stdout.unwrap().len(), 4);
    assert_eq!(stderr.unwrap().len(), 4);
    let mut count = 0;
    while received.recv().await.is_some() {
        count += 1;
    }
    assert!(count >= 2, "caps must not suppress live chunks");
}
