use super::protocol::{self, Operation, Request};
use std::io::Cursor;

#[test]
fn protocol_roundtrips_and_rejects_oversized_or_truncated_frames() {
    let request = Request::Call(Operation::Eval {
        source: "(+ 1 2)".into(),
        path: None,
        origin: None,
    });
    let encoded = protocol::encode(&request).unwrap();
    let decoded: Request = protocol::read(&mut Cursor::new(&encoded)).unwrap();
    assert!(
        matches!(decoded, Request::Call(Operation::Eval { source, .. }) if source == "(+ 1 2)")
    );
    assert!(protocol::read::<Request>(&mut Cursor::new(&encoded[..encoded.len() - 1])).is_err());
    assert!(protocol::read::<Request>(&mut Cursor::new(u32::MAX.to_be_bytes())).is_err());
}

pub(super) fn binary() -> std::path::PathBuf {
    if let Some(binary) = std::env::var_os("YINZ_COMPILATION_WORKER") {
        return binary.into();
    }
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug");
    let example = target.join("examples/worker");
    assert!(example.is_file(), "build the isolated worker with cargo build -p yinz-steel --example worker, or use cargo x test");
    example
}

#[test]
fn large_image_frames_fit_transport_and_keep_the_worker_healthy() {
    use super::protocol::{Output, Response};
    use yinz_tile::ui_components::png_transmit_sequence;
    let png = vec![255; crate::MAX_PNG_BYTES];
    let expected = png_transmit_sequence(1, &png);
    let frame = protocol::encode(&Response::Finished(Ok(Output::Text(expected.clone())))).unwrap();
    assert!(frame.len() < super::MAX_FRAME);
    assert!(
        frame.len() < 64 * 1024 * 1024,
        "image fits application IPC too"
    );
    let decoded: Response = protocol::read(&mut Cursor::new(frame)).unwrap();
    assert!(matches!(decoded, Response::Finished(Ok(Output::Text(text))) if text == expected));

    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker.eval("(define opened 0) (define (pipe message) (set! opened (+ opened 1)) #t) (define (render rows columns) (yinz-ui-image-png 1 (make-bytes 8388608 255)))").unwrap();
    let pid = worker.process_id();
    for opened in 1..=12 {
        assert_eq!(worker.render(24, 80).unwrap(), expected);
        assert!(worker.pipe(serde_json::json!({"name":"open"})).unwrap());
        assert_eq!(worker.value("opened").unwrap(), opened);
        assert_eq!(worker.process_id(), pid);
    }
}

#[test]
fn layout_diagnostics_never_read_tiled_or_floating_contents_files() {
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let fifo = directory.path().join("contents.fifo");
    let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut worker = super::WorkerRuntime::new(&binary(), |_| {
        panic!("layout diagnostics must not call the host")
    })
    .unwrap();
    // With no writer, opening this FIFO would block until the supervisor kills the worker.
    for pane in [
        format!("pane contents_file={:?};", fifo.to_str().unwrap()),
        format!(
            "floating_panes {{ pane contents_file={:?}; }};",
            fifo.to_str().unwrap()
        ),
    ] {
        assert_eq!(
            worker
                .invoke(
                    "yinz-format-layout-error",
                    vec![json!(format!("layout {{ {pane} }}"))]
                )
                .unwrap(),
            ""
        );
        assert_eq!(worker.eval("(+ 40 2)").unwrap().values, ["42"]);
    }
}

#[test]
fn worker_keeps_state_and_transports_host_commands_and_errors() {
    use serde_json::json;
    let mut worker = super::WorkerRuntime::new(&binary(), |value| {
        if value == json!("fail") {
            anyhow::bail!("permission denied");
        }
        Ok(value)
    })
    .unwrap();
    worker
        .set_server_metadata(crate::ServerMetadata {
            binary: "/opt/yinz/bin/yinz".into(),
            session: Some("worker-session".into()),
            incarnation: None,
        })
        .unwrap();
    assert_eq!(
        worker.invoke("yinz-server-session", vec![]).unwrap(),
        json!("worker-session")
    );
    assert_eq!(
        worker.invoke("yinz-server-incarnation", vec![]).unwrap(),
        serde_json::Value::Null
    );
    worker
        .eval_with_path(
            "(define x 40)",
            Some(std::path::Path::new("builtin:example.scm")),
        )
        .unwrap();
    assert_eq!(worker.eval("(+ x 2)").unwrap().values, ["42"]);
    assert_eq!(
        worker
            .eval("(yinz-command 18446744073709551615)")
            .unwrap()
            .values,
        ["18446744073709551615"]
    );
    assert!(worker
        .eval("(display \"before\") (yinz-command \"fail\")")
        .is_err());
    assert_eq!(worker.take_output().unwrap(), "before");
    worker.eval("(define (load config) (set! x (hash-ref config \"x\"))) (define (update event) #t) (define (render rows columns) (display x))").unwrap();
    worker.load(json!({"x": 12})).unwrap();
    assert!(worker.update(json!({})).unwrap());
    assert_eq!(worker.render(10, 20).unwrap(), "12");
    worker
        .eval("(define (render rows columns) (yinz-log \"worker diagnostic\") \"frame\")")
        .unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "frame");
    assert_eq!(worker.take_logs().unwrap(), "worker diagnostic\n");
}

#[test]
fn worker_returns_deferred_text_settled_during_the_same_render() {
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker
        .eval(
            r#"(define render-calls 0)
               (define (render rows columns)
                 (set! render-calls (+ render-calls 1))
                 (yinz-then "ready" (lambda (s) s)))"#,
        )
        .unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "ready");
    assert_eq!(worker.render(10, 20).unwrap(), "ready");
    assert_eq!(worker.value("render-calls").unwrap(), 1);

    worker
        .eval(
            r#"(define (render rows columns)
                 (set! render-calls (+ render-calls 1))
                 (yinz-then "fresh" (lambda (s) s)))
               (yinz-request-render)"#,
        )
        .unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "fresh");
    assert_eq!(worker.value("render-calls").unwrap(), 2);
    worker.unload().unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "");
    assert_eq!(worker.value("render-calls").unwrap(), 2);
}

#[test]
fn worker_retains_previous_frame_until_deferred_render_completes() {
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker
        .eval(
            r#"(define render-calls 0)
               (define frame "previous")
               (define (render rows columns)
                 (set! render-calls (+ render-calls 1))
                 frame)"#,
        )
        .unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "previous");
    worker
        .eval("(set! frame (yinz-deferred)) (yinz-request-render)")
        .unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "previous");
    assert_eq!(worker.render(10, 20).unwrap(), "previous");
    assert_eq!(worker.value("render-calls").unwrap(), 2);
    worker.eval("(yinz-resolve! frame \"completed\")").unwrap();
    assert_eq!(worker.render(10, 20).unwrap(), "completed");
    assert_eq!(worker.render(10, 20).unwrap(), "completed");
    assert_eq!(worker.value("render-calls").unwrap(), 2);
}

#[test]
fn worker_cancellation_kills_infinite_bytecode_and_reaps_process() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (send, receive) = mpsc::channel();
    let mut worker = super::WorkerRuntime::new(&binary(), move |value| {
        send.send(()).unwrap();
        Ok(value)
    })
    .unwrap();
    let interrupt = worker.interruption_handle();
    let process_id = worker.process_id();
    let thread =
        std::thread::spawn(move || worker.eval("(define (spin) (spin)) (yinz-command #t) (spin)"));
    receive.recv_timeout(Duration::from_secs(10)).unwrap();
    interrupt.interrupt();
    assert!(thread.join().unwrap().is_err());
    assert_eq!(
        unsafe {
            libc::waitpid(
                process_id as libc::pid_t,
                std::ptr::null_mut(),
                libc::WNOHANG,
            )
        },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    interrupt.interrupt();
}

#[test]
fn slow_host_callbacks_do_not_retire_a_waiting_worker() {
    use serde_json::json;
    use std::time::{Duration, Instant};

    let (send_deadline, deadlines) = std::sync::mpsc::channel::<Instant>();
    let mut worker = super::WorkerRuntime::new(&binary(), move |command| {
        if command == "spin" {
            return Ok(command);
        }
        // Timing is the behavior: finish the host call after the original guest deadline.
        let deadline = deadlines.recv().unwrap();
        std::thread::sleep(
            deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(50),
        );
        if command == "fail" {
            anyhow::bail!("host operation failed");
        }
        Ok(command)
    })
    .unwrap();
    worker
        .eval("(define events 0) (define (update event) (yinz-command event) (set! events (+ events 1)) #f)")
        .unwrap();
    for command in [json!({"DataApply": {}}), json!("fail")] {
        let deadline = Instant::now() + Duration::from_secs(2);
        send_deadline.send(deadline).unwrap();
        let result = worker.call_before(Operation::Update(command.clone()), deadline);
        if command == "fail" {
            let error = result.err().unwrap();
            assert!(
                error.to_string().contains("host operation failed"),
                "{error:#}"
            );
        } else {
            assert!(matches!(
                result.unwrap(),
                super::protocol::Output::Changed(false)
            ));
        }
        assert_eq!(worker.value("events").unwrap(), 1);
    }
    assert_eq!(worker.eval("(+ events 1)").unwrap().values, ["2"]);
    let error = worker
        .call_before(
            Operation::Eval {
                source: "(let loop () (yinz-command \"spin\") (loop))".into(),
                path: None,
                origin: None,
            },
            Instant::now() + Duration::from_millis(100),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("deadline"), "{error:#}");
}

#[test]
fn worker_deadline_bounds_recursive_macro_expansion() {
    use std::time::{Duration, Instant};
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    let result = worker.call_before(
        Operation::Eval {
            source: "(define-syntax grow (syntax-rules () [(grow x) (grow (x x))])) (grow 1)"
                .into(),
            path: None,
            origin: None,
        },
        Instant::now() + Duration::from_millis(100),
    );
    assert!(result.err().unwrap().to_string().contains("deadline"));
    let mut healthy = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    assert_eq!(healthy.eval("(+ 1 2)").unwrap().values, ["3"]);
}

#[test]
fn worker_memory_limit_contains_single_huge_native_allocation() {
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    let error = worker.eval("(make-string 1073741824 #\\x)").unwrap_err();
    assert!(error.to_string().contains("Steel worker"), "{error:#}");
    let mut healthy = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    assert_eq!(healthy.eval("(+ 1 2)").unwrap().values, ["3"]);
}

#[test]
fn worker_imports_confined_files_and_keeps_unknown_modules_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("helpers")).unwrap();
    std::fs::write(
        directory.path().join("helpers/value.scm"),
        "(require \"../constant.scm\") (provide answer) (define answer (+ constant 2))",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("constant.scm"),
        "(provide constant) (define constant 40)",
    )
    .unwrap();
    let root = crate::modules::SourceRoot::file(&directory.path().join("main.scm"), None).unwrap();
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker.set_source_root(root);
    assert_eq!(
        worker
            .eval("(require \"./helpers/value.scm\") answer")
            .unwrap()
            .values
            .last()
            .unwrap(),
        "42"
    );
    for source in [
        "(require \"/etc/passwd\")",
        "(require \"../outside.scm\")",
        "(require \"yinz-module:constant.scm\")",
        "(require-builtin steel/process)",
    ] {
        assert!(worker.eval(source).is_err(), "accepted {source}");
    }
    assert_eq!(worker.eval("answer").unwrap().values, ["42"]);
    worker.eval("(define own-location (yinz-plugin-location)) (define sibling-location (yinz-plugin-resolve \"./constant.scm\"))").unwrap();
    assert_eq!(
        worker.value("own-location").unwrap(),
        url::Url::from_file_path(directory.path().canonicalize().unwrap().join("main.scm"))
            .unwrap()
            .as_str()
    );
    assert_eq!(
        worker.value("sibling-location").unwrap(),
        url::Url::from_file_path(
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join("constant.scm")
        )
        .unwrap()
        .as_str()
    );
    assert!(worker
        .eval("(yinz-plugin-resolve \"../escape.scm\")")
        .is_err());
}

#[test]
fn worker_sdk_discovers_binds_and_dispatches_named_worker() {
    use serde_json::json;
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker.eval("(yinz-register-workers (hash \"probe\" (lambda (name payload) (display name) (display payload))))").unwrap();
    assert_eq!(
        worker.invoke("yinz-worker-names", vec![]).unwrap(),
        json!(["probe"])
    );
    worker
        .invoke("yinz-worker-bind", vec![json!("probe")])
        .unwrap();
    worker
        .invoke(
            "yinz-worker-message",
            vec![json!("message"), json!("payload")],
        )
        .unwrap();
    assert_eq!(worker.take_output().unwrap(), "messagepayload");
}

#[test]
fn worker_keeps_opaque_widget_state_inside_the_interpreter() {
    let mut worker = super::WorkerRuntime::new(&binary(), |_| {
        anyhow::bail!("pure render must not invoke host commands")
    })
    .unwrap();
    worker.eval("(define form (yinz-ui-form (list (yinz-ui-input \"name\" \"Name\" \"initial\")))) (define (render rows columns) (hash-ref (yinz-ui-render-buffer form (yinz-ui-coordinates 0 0 columns rows)) \"text\"))").unwrap();
    worker
        .eval("(yinz-ui-set-value! form \"name\" \"edited\")")
        .unwrap();
    let frame = worker.render(2, 30).unwrap();
    assert!(frame.contains(yinz_types::ui_components::UI_COMPONENT_DCS_PREFIX));
    assert!(worker.value("form").is_err());
    assert_eq!(
        worker.eval("(yinz-ui-value form \"name\")").unwrap().values,
        ["\"edited\""]
    );
}

#[test]
fn worker_collects_large_live_mutable_graphs_within_its_resource_budget() {
    let mut worker = super::WorkerRuntime::new(&binary(), Ok).unwrap();
    worker
        .eval("(define retained (map (lambda (n) (vector n)) (range 12000)))")
        .unwrap();
    for _ in 0..4 {
        worker
            .eval("(for-each (lambda (n) (vector n n n)) (range 30000))")
            .unwrap();
    }
    assert_eq!(
        worker
            .eval("(list (length retained) (vector-ref (list-ref retained 11999) 0))")
            .unwrap()
            .values,
        ["(12000 11999)"]
    );
}

#[test]
fn oversized_host_replies_and_requests_do_not_desynchronize_the_worker() {
    let mut worker = super::WorkerRuntime::new(&binary(), |_| {
        // Escaping each NUL needs six JSON bytes: the text fits the logical value budget,
        // but its encoded frame plus envelope exceeds the private transport limit.
        Ok(serde_json::Value::String("\0".repeat(super::MAX_FRAME / 6)))
    })
    .unwrap();
    let failure = worker
        .eval("(yinz-command \"GetYinzVersion\")")
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("host reply exceeds worker transport limit"),
        "{failure:#}"
    );
    assert_eq!(worker.eval("(+ 20 22)").unwrap().values, ["42"]);
    assert!(worker
        .load(serde_json::Value::String("\0".repeat(super::MAX_FRAME / 6)))
        .is_err());
    assert_eq!(worker.eval("(+ 40 2)").unwrap().values, ["42"]);
}
