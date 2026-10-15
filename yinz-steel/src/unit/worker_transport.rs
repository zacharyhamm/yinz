use super::*;
use serde_json::json;

fn worker() -> WorkerRuntime {
    WorkerRuntime::new(&super::super::tests::binary(), Ok).unwrap()
}

fn stop(worker: &WorkerRuntime) {
    let pid = worker.process_id() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
        pid
    );
    assert!(libc::WIFSTOPPED(status));
}

fn fill_input(worker: &mut WorkerRuntime, frame: &[u8]) -> usize {
    let mut sent = 0;
    loop {
        match worker.input.write(&frame[sent..]) {
            Ok(count) => {
                assert!(count > 0);
                sent += count;
                assert!(sent < frame.len(), "test frame must exceed pipe capacity");
            },
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return sent,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
            Err(error) => panic!("fill worker input: {error}"),
        }
    }
}

#[test]
fn worker_transports_large_requests_and_host_replies_after_forced_backpressure() {
    let mut worker = worker();
    worker.eval("(define (echo value) value)").unwrap();
    let text = "界".repeat(3 * 1024 * 1024);
    let frame = protocol::encode(&Request::Call(Operation::Invoke(
        "echo".into(),
        vec![json!(text)],
    )))
    .unwrap();
    stop(&worker);
    let sent = fill_input(&mut worker, &frame);
    assert_eq!(
        unsafe { libc::kill(worker.process_id() as libc::pid_t, libc::SIGCONT) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    worker.send_frame(&frame[sent..], deadline).unwrap();
    assert!(
        matches!(worker.receive(deadline).unwrap(), Response::Finished(Ok(Output::Value(value))) if value == text)
    );
    worker.host = Box::new(move |_| Ok(json!(text)));
    assert_eq!(
        worker
            .eval("(utf8-length (yinz-command \"large reply\"))")
            .unwrap()
            .values,
        ["9437184"]
    );
    assert_eq!(worker.eval("(+ 40 2)").unwrap().values, ["42"]);
}

#[test]
fn worker_send_deadline_kills_a_child_with_a_full_input_pipe() {
    let mut worker = worker();
    stop(&worker);
    let frame = protocol::encode(&Request::Call(Operation::Load(json!(
        "x".repeat(8 * 1024 * 1024)
    ))))
    .unwrap();
    let sent = fill_input(&mut worker, &frame);
    let error = worker
        .send_frame(&frame[sent..], Instant::now() + Duration::from_millis(100))
        .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error:#}");
    assert!(worker.process.status().unwrap().is_some());
}

#[test]
fn worker_send_cancellation_releases_a_full_input_pipe() {
    let mut worker = worker();
    stop(&worker);
    let frame = protocol::encode(&Request::Call(Operation::Load(json!(
        "x".repeat(8 * 1024 * 1024)
    ))))
    .unwrap();
    let sent = fill_input(&mut worker, &frame);
    let interrupt = worker.interruption_handle();
    let thread = std::thread::spawn(move || {
        worker.send_frame(&frame[sent..], Instant::now() + Duration::from_secs(30))
    });
    interrupt.interrupt();
    assert!(thread.join().unwrap().is_err());
}
