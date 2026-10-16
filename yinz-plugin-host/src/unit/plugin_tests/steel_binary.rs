//! Byte fidelity and bounded file ranges cross the real Scheme host transport.
use super::script_harness::Harness;
use super::*;
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2166136261u32, |hash, byte| {
        (hash ^ *byte as u32).wrapping_mul(16777619)
    })
}

#[test]
#[serial(plugin_tests)]
fn steel_large_png_survives_repeated_navigation_and_open_in_the_same_instance() {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1024, 1024);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut state = 1u32;
        let pixels: Vec<u8> = (0..1024 * 1024 * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
    assert!(png.len() > 2_572_749 && png.len() <= 8 * 1024 * 1024);
    let expected = yinz_tile::ui_components::png_transmit_sequence(1, &png);
    let harness = Harness::new(
        "scm",
        r#"
        (define image #f)
        (define instance (yinz-instance-id))
        (define page 0)
        (define opens 0)
        (define (load config)
          (yinz-command (hash "Subscribe" '("CustomMessage")))
          (yinz-then (yinz-wait-for-permissions '("ReadFiles" "ReadCliPipes"))
            (lambda (_)
              (set! image (yinz-command (hash "FsRead" (hash "path" "/host/page.png"
                "max_bytes" json-null "offset" json-null))))
              (yinz-request-render))))
        (define (update event)
          (when (and (hash? event) (hash-contains? event "CustomMessage"))
            (set! page (+ page 1))) #t)
        (define (pipe message)
          (when (equal? (hash-ref message "name") "open") (set! opens (+ opens 1))) #t)
        (define (render rows cols)
          (if image (string-append "instance=" instance " page=" (number->string page)
            " opens=" (number->string opens) "\n"
            (yinz-ui-image-png 1 image) (yinz-ui-image-place 1 80 8)) "loading image"))
    "#,
        PermissionStatus::Granted,
    );
    std::fs::write(harness.directory.path().join("page.png"), &png).unwrap();
    harness.load();
    let initial = harness.rendered("page=0 opens=0");
    assert!(
        initial.contains(&expected),
        "PNG bytes must retain their original resolution"
    );
    let identity = initial.split(" page=").next().unwrap().to_owned();
    #[cfg(target_os = "linux")]
    let (worker, baseline) = image_worker_memory();
    for page in 1..=24 {
        harness.log.lock().clear();
        harness.event(Event::CustomMessage("next-page".into(), String::new()));
        let frame = harness.rendered(&format!("page={page} opens=0"));
        assert!(frame.starts_with(&identity));
        assert!(frame.contains(&expected));
    }
    harness.log.lock().clear();
    harness
        .sender
        .send(PluginInstruction::CliPipe {
            focus_plugin: false,
            is_eof: false,
            pipe_id: "large-png-open".into(),
            name: "open".into(),
            payload: Some("next.pdf".into()),
            plugin: None,
            args: None,
            configuration: None,
            floating: None,
            pane_id_to_replace: None,
            pane_title: None,
            cwd: None,
            skip_cache: false,
            cli_client_id: ClientId::new(1),
            source_pane_id: None,
        })
        .unwrap();
    let frame = harness.rendered("page=24 opens=1");
    assert!(frame.starts_with(&identity));
    assert!(frame.contains(&expected));
    #[cfg(target_os = "linux")]
    {
        let (current_worker, current) = image_worker_memory();
        assert_eq!(
            worker, current_worker,
            "navigation must retain the same supervised worker"
        );
        assert!(
            current <= baseline + 64 * 1024,
            "large-image buffers accumulated: baseline={baseline} KiB current={current} KiB"
        );
    }
    harness.finish();
}

#[cfg(target_os = "linux")]
fn image_worker_memory() -> (u32, usize) {
    let mut children = std::collections::BTreeSet::new();
    for task in std::fs::read_dir("/proc/self/task").unwrap() {
        let path = task.unwrap().path().join("children");
        if let Ok(pids) = std::fs::read_to_string(path) {
            children.extend(
                pids.split_whitespace()
                    .map(|pid| pid.parse::<u32>().unwrap()),
            );
        }
    }
    let worker = children
        .into_iter()
        .find(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|command| {
                command
                    .split(|byte| *byte == 0)
                    .any(|arg| arg == b"--steel-runtime-worker")
            })
        })
        .expect("supervised Steel worker");
    let status = std::fs::read_to_string(format!("/proc/{worker}/status")).unwrap();
    let rss = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    (worker, rss)
}

#[test]
fn steel_file_ranges_read_large_files_without_weakening_whole_file_limits() {
    use std::io::{Seek, SeekFrom, Write};

    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(define cap 8388608)
(define (check condition) (unless condition (error "file range assertion")))
(define (refused thunk) (with-handler (lambda (failure) #t) (thunk) #f))
(define (read-range path offset limit)
  (yinz-command (hash "FsRead" (hash "path" path "offset" offset "max_bytes" limit))))
(define (load config)
  (check (refused (lambda () (read-range "/host/large.bin" 0 5))))
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions (list "ReadFiles"))
      (lambda (ignored)
        (check (refused (lambda () (read-range "/host/large.bin" json-null json-null))))
        (check (refused (lambda () (read-range "/host/large.bin" json-null (* cap 2)))))
        (check (equal? (yinz-decode-utf8 (read-range "/host/large.bin" 0 5)) "start"))
        (check (equal? (bytes->list (read-range "/host/large.bin" (- cap 2) 8)) '(1 2 3 4 5 6)))
        (check (equal? (bytes->list (read-range "/host/large.bin" cap 3)) '(3 4 5)))
        (check (equal? (bytes->list (read-range "/host/inside-link" cap json-null)) '(3 4 5 6)))
        (check (zero? (bytes-length (read-range "/host/large.bin" (+ cap 4) json-null))))
        (check (zero? (bytes-length (read-range "/host/large.bin" (+ cap 100) 1))))
        (check (zero? (bytes-length (read-range "/host/large.bin" 0 0))))
        (for-each (lambda (offset) (check (refused (lambda () (read-range "/host/large.bin" offset json-null)))))
          (list -1 1.5 18446744073709551616))
        (check (refused (lambda () (read-range "/host/large.bin" 0 -1))))
        (check (refused (lambda () (read-range "/host" 0 1))))
        (check (refused (lambda () (read-range "/host/fifo" 0 1))))
        (set! state "file ranges verified")))
    (lambda (failure) (set! state (string-append "file ranges failed: " failure)))))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let path = harness.directory.path().join("large.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"start").unwrap();
    file.seek(SeekFrom::Start(8 * 1024 * 1024 - 2)).unwrap();
    file.write_all(&[1, 2, 3, 4, 5, 6]).unwrap();
    std::os::unix::fs::symlink(&path, harness.directory.path().join("inside-link")).unwrap();
    nix::unistd::mkfifo(
        &harness.directory.path().join("fifo"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    harness.load();
    let frame = harness.rendered("file ranges");
    assert!(frame.contains("file ranges verified"), "{frame}");
}

#[test]
#[serial(plugin_tests)]
fn steel_streaming_output_arrives_as_bytevectors_and_a_64_kib_chunk_round_trips_byte_exact() {
    let harness = Harness::new(
        "scm",
        r#"
(define (fnv bytes)
  (foldl (lambda (byte hash) (modulo (* (bitwise-xor hash byte) 16777619) 4294967296)) 2166136261 (bytes->list bytes)))

(define state "waiting")
(define (load config)
  (yinz-command (hash "Subscribe" (list "PaneOutput" "RunCommandOutput" "WebRequestStreamChunk")))
  (yinz-then (yinz-wait-for-permissions (list "ReadPaneContents" "RunCommands" "WebAccess"))
    (lambda (ignored) (set! state "ready") (yinz-request-render))))
(define (update event)
  (define pane? (and (hash? event) (hash-contains? event "PaneOutput")))
  (define web? (and (hash? event) (hash-contains? event "WebRequestStreamChunk")))
  (define payload (cond (pane? (hash-ref event "PaneOutput"))
    (web? (hash-ref event "WebRequestStreamChunk"))
    ((and (hash? event) (hash-contains? event "RunCommandOutput")) (hash-ref event "RunCommandOutput")) (else #f)))
  (if payload
    (let* ((data (hash-ref payload (if web? "chunk" "data")))
           (kind (cond (pane? (string-append "pane " (number->string (hash-ref payload "pane_id"))))
                     (web? (string-append "web done=" (yinz-json-string (hash-ref payload "done"))))
                     (else (string-append "command stderr=" (yinz-json-string (hash-ref payload "stderr")))))))
      (set! state (string-append kind " " (if web? (hash-ref payload "stream_id") (hash-ref (hash-ref payload "context") "work"))
        " typed=" (yinz-json-string (bytevector? data)) " len=" (number->string (bytes-length data))
        " hash=" (number->string (fnv data)) " first=" (if (zero? (bytes-length data)) "undefined" (number->string (bytes-ref data 0)))
        " last=" (if (zero? (bytes-length data)) "undefined" (number->string (bytes-ref data (- (bytes-length data) 1)))))) #t)
    #f))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("ready");
    let chunk: Vec<u8> = (0..65536u32)
        .map(|i| (i.wrapping_mul(7) ^ (i >> 8)) as u8)
        .collect();
    let expected = format!(
        "typed=true len={} hash={} first={} last={}",
        chunk.len(),
        fnv1a(&chunk),
        chunk[0],
        chunk[chunk.len() - 1]
    );
    harness.event(Event::PaneOutput {
        pane_id: 7,
        data: chunk.clone(),
        context: BTreeMap::from([("work".into(), "one".into())]),
    });
    harness.rendered(&format!("pane 7 one {expected}"));
    harness.event(Event::RunCommandOutput {
        data: chunk.clone(),
        stderr: true,
        context: BTreeMap::from([("work".into(), "two".into())]),
    });
    harness.rendered(&format!("command stderr=true two {expected}"));
    harness.event(Event::WebRequestStreamChunk {
        stream_id: "stream".into(),
        chunk,
        done: false,
        error: None,
    });
    harness.rendered(&format!("web done=false stream {expected}"));
    harness.event(Event::WebRequestStreamChunk {
        stream_id: "stream".into(),
        chunk: vec![],
        done: true,
        error: None,
    });
    harness.rendered(
        "web done=true stream typed=true len=0 hash=2166136261 first=undefined last=undefined",
    );
    harness.event(Event::PaneOutput {
        pane_id: 8,
        data: Vec::new(),
        context: BTreeMap::from([("work".into(), "empty".into())]),
    });
    harness
        .rendered("pane 8 empty typed=true len=0 hash=2166136261 first=undefined last=undefined");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_byte_replies_are_bytevectors_without_losing_byte_values() {
    let harness = Harness::new(
        "scm",
        r#"
(define (fnv bytes)
  (foldl (lambda (byte hash) (modulo (* (bitwise-xor hash byte) 16777619) 4294967296)) 2166136261 (bytes->list bytes)))

(define state "waiting")
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "ReadFiles"))
    (lambda (ignored)
      (define data (yinz-command (hash "FsRead" (hash "path" "/host/blob.bin" "max_bytes" json-null "offset" json-null))))
      (define raw (yinz-command (hash "FsRead" (hash "path" "/host/blob.bin" "max_bytes" json-null "offset" json-null))))
      (set! state (string-append "typed=" (yinz-json-string (bytevector? data)) " raw=" (yinz-json-string (bytevector? raw))
        " len=" (number->string (bytes-length data)) " hash=" (number->string (fnv data))
        " same=" (yinz-json-string (equal? raw data)))))))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let blob: Vec<u8> = (0..65536u32)
        .map(|i| (i.wrapping_mul(13) ^ (i >> 7)) as u8)
        .collect();
    std::fs::write(harness.directory.path().join("blob.bin"), &blob).unwrap();
    harness.load();
    let frame = harness.rendered("typed=");
    assert_eq!(
        frame.trim_end(),
        format!(
            "typed=true raw=true len={} hash={} same=true",
            blob.len(),
            fnv1a(&blob)
        )
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_file_read_and_write_preserve_the_full_eight_mib_limit() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(define cap 8388608)
(define (check condition) (unless condition (error "full file assertion")))
(define (read-file path offset)
  (yinz-command (hash "FsRead" (hash "path" path "max_bytes" json-null "offset" offset))))
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("ReadFiles" "WriteFiles"))
      (lambda (ignored)
        (define data (read-file "/host/full.bin" json-null))
        (check (and (bytevector? data) (= (bytes-length data) cap)))
        (check (and (= (bytes-ref data 0) 0) (= (bytes-ref data 4194304) 255)
                    (= (bytes-ref data (- cap 1)) 128)))
        (check (= (string-length (yinz-decode-utf8 data)) cap))
        (yinz-command (hash "FsWrite" (hash "path" "/host/copied.bin" "contents" data
          "append" #f "create_dirs" #f)))
        (check (equal? data (read-file "/host/copied.bin" json-null)))
        (check (with-handler (lambda (failure) (string-contains? (error-object-message failure) "byte limit"))
                 (read-file "/host/oversized.bin" json-null) #f))
        (check (equal? data (read-file "/host/oversized.bin" 0)))
        (check (= (bytes-length (read-file "/host/full.bin" cap)) 0))
        (set! state "full file verified")))
    (lambda (failure) (set! state (string-append "full file failed: " failure)))))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let mut bytes = vec![b'x'; 8 * 1024 * 1024];
    bytes[0] = 0;
    bytes[4 * 1024 * 1024] = 255;
    *bytes.last_mut().unwrap() = 128;
    std::fs::write(harness.directory.path().join("full.bin"), &bytes).unwrap();
    let mut oversized = bytes.clone();
    oversized.push(1);
    std::fs::write(harness.directory.path().join("oversized.bin"), oversized).unwrap();
    harness.load();
    let frame = harness.rendered("full file ");
    assert!(frame.contains("full file verified"), "{frame}");
    assert_eq!(
        std::fs::read(harness.directory.path().join("copied.bin")).unwrap(),
        bytes
    );
    harness.finish();
}

fn binary_job_harness(
    source: &str,
) -> (
    Harness,
    Watched<Vec<BackgroundJob>>,
    std::thread::JoinHandle<()>,
) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("binary-jobs.scm");
    std::fs::write(&path, source).unwrap();
    let (sender, jobs_receiver, receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(directory.path().to_owned()), None);
    let log = Watched::new(vec![]);
    let logger = permissions_and_log_until_disconnect(
        &log,
        receiver,
        directory.path().join("permissions.kdl"),
        sender.clone(),
        ClientId::new(1),
        PermissionStatus::Granted,
    );
    let jobs = Watched::new(vec![]);
    let job_logger = log_until_disconnect(&jobs, jobs_receiver);
    sender
        .send(PluginInstruction::AddClient(ClientId::new(1)))
        .unwrap();
    (
        Harness {
            sender,
            log,
            server_log: Watched::new(vec![]),
            server_logger: None,
            directory,
            path,
            teardown: Some(teardown),
            logger: Some(logger),
        },
        jobs,
        job_logger,
    )
}

#[test]
#[serial(plugin_tests)]
fn steel_command_helper_receives_eight_mib_in_each_output_stream() {
    let (harness, jobs, logger) = binary_job_harness(
        r#"
(define state "starting")
(define (check condition) (unless condition (error "large command assertion")))
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("RunCommands"))
      (lambda (ignored)
        (yinz-then (yinz-run-command
          (hash "command" '("/usr/bin/true") "context" (hash "work" "binary")))
          (lambda (result)
            (check (= (hash-ref result "exit-code") 7))
            (check (equal? (hash-ref result "context") (hash "work" "binary")))
            (check (bytevector? (hash-ref result "stdout")))
            (check (bytevector? (hash-ref result "stderr")))
            (check (equal? (hash-ref result "stdout") (make-bytes 8388608 111)))
            (check (equal? (hash-ref result "stderr") (make-bytes 8388608 255)))
            (check (= (string-length (yinz-decode-utf8 (hash-ref result "stdout"))) 8388608))
            (set! state "large command verified")))))
    (lambda (failure) (set! state (string-append "large command failed: " failure)))))
(define (render rows cols) state)
"#,
    );
    harness.load();
    let (plugin, client, generation, context) = jobs.wait_until(
        "the large-output command request",
        default_timeout(),
        |jobs| {
            jobs.iter().find_map(|job| match job {
                BackgroundJob::RunCommand(
                    plugin,
                    client,
                    _,
                    _,
                    _,
                    _,
                    context,
                    limit,
                    _,
                    generation,
                ) => {
                    assert_eq!(*limit, None, "uncapped output is the default");
                    Some((*plugin, *client, *generation, context.clone()))
                },
                _ => None,
            })
        },
    );
    harness
        .sender
        .send(PluginInstruction::for_plugin_event(
            plugin,
            client,
            generation,
            Event::RunCommandResult(
                Some(7),
                vec![b'o'; 8 * 1024 * 1024],
                vec![255; 8 * 1024 * 1024],
                context,
            ),
        ))
        .unwrap();
    let frame = harness.rendered("large command ");
    assert!(frame.contains("large command verified"), "{frame}");
    harness.finish();
    logger.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_web_helper_receives_the_full_sixty_four_mib_buffered_body() {
    let (harness, jobs, logger) = binary_job_harness(
        r#"
(define state "starting")
(define (check condition) (unless condition (error "full web body assertion")))
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("WebAccess"))
      (lambda (ignored)
        (yinz-then (yinz-web-request
          (hash "url" "https://binary.invalid/body" "context" (hash "work" "full-body")
            "body" (bytes 0 255 128 195 40 10 0)))
          (lambda (result)
            (check (= (hash-ref result "status") 206))
            (check (equal? (hash-ref result "headers") (hash "content-type" "application/octet-stream")))
            (check (equal? (hash-ref result "context") (hash "work" "full-body")))
            (define body (hash-ref result "body"))
            (check (and (bytevector? body) (= (bytes-length body) 67108864)))
            (define expected (make-bytes 67108864 255))
            (bytes-set! expected 0 0)
            (bytes-set! expected 33554432 128)
            (bytes-set! expected 67108863 17)
            (check (equal? body expected))
            (set! state "full web body verified")))))
    (lambda (failure) (set! state (string-append "full web body failed: " failure)))))
(define (render rows cols) state)
"#,
    );
    harness.load();
    let (plugin, client, generation, context) =
        jobs.wait_until("the full-body web request", default_timeout(), |jobs| {
            jobs.iter().find_map(|job| match job {
                BackgroundJob::WebRequest(
                    plugin,
                    client,
                    _,
                    _,
                    _,
                    body,
                    context,
                    _,
                    _,
                    generation,
                ) => {
                    assert_eq!(body, &[0, 255, 128, 195, 40, 10, 0]);
                    Some((*plugin, *client, *generation, context.clone()))
                },
                _ => None,
            })
        });
    // This is the existing inclusive buffered-body limit. The real async service reply
    // takes the same typed event, pinned worker, and SDK promise path as a network response.
    let mut body = vec![255; 64 * 1024 * 1024];
    body[0] = 0;
    body[32 * 1024 * 1024] = 128;
    *body.last_mut().unwrap() = 17;
    harness
        .sender
        .send(PluginInstruction::for_plugin_event(
            plugin,
            client,
            generation,
            Event::WebRequestResult(
                206,
                BTreeMap::from([("content-type".into(), "application/octet-stream".into())]),
                body,
                context,
            ),
        ))
        .unwrap();
    let frame = harness.rendered("full web body ");
    assert!(frame.contains("full web body verified"), "{frame}");
    harness.finish();
    logger.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_data_blobs_preserve_eight_mib_through_apply_fetch_and_watch() {
    let source = [
        include_str!("fixtures/data_helpers.scm"),
        r#"
(define state "starting")
(define expected (make-bytes 8388608 255))
(bytes-set! expected 0 0)
(bytes-set! expected 4194304 128)
(bytes-set! expected 8388607 17)
(define literal "{\"Blob\":{\"$yinz-bytes\":\"literal\"}}")
(define (check-blob rows)
  (define blob (data-value rows "payload"))
  (assert-data (bytevector? blob))
  (assert-data (equal? blob expected))
  (assert-data (equal? (data-value rows "literal") literal)))
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("PluginData"))
      (lambda (ignored)
        (yinz-data-define json-null (data-definition "binary"
          '(("payload" "Blob") ("literal" "Json"))))
        (define values (list (list "payload" (hash "Blob" expected))
                             (list "literal" (hash "Text" literal))))
        (yinz-data-apply json-null (list (data-put "binary" "one" values 0)))
        (define query (data-query "binary"))
        (define first (yinz-data-fetch json-null query))
        (check-blob first)
        (define watch (yinz-data-watch-callback json-null query
          (lambda (rows event)
            (check-blob rows)
            (set! state "full data blob verified") #t)))
        (check-blob (hash-ref watch "initial"))
        (bytes-set! expected 8388607 19)
        (yinz-data-apply json-null (list (data-update "binary" (data-eq "uid" (hash "Text" "one"))
          (list (list "payload" (hash "Blob" expected))) (data-value first "rev"))))))
    (lambda (failure) (set! state (string-append "full data blob failed: " failure)))))
(define (render rows cols) state)
"#,
    ]
    .concat();
    let harness = Harness::new("scm", &source, PermissionStatus::Granted);
    harness.load();
    let frame = harness.rendered("full data blob ");
    assert!(frame.contains("full data blob verified"), "{frame}");
    harness.finish();
}
