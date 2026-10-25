use super::*;
use crate::thread_bus::ThreadSenders;
use futures_lite::io::AsyncRead;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use yinz_runtime::channels::{unbounded, Receiver, SenderWithContext};
use yinz_types::diagnostics::ErrorContext;

const PLUGIN_ID: PluginId = PluginId::new(1);
const CLIENT_ID: ClientId = ClientId::new(2);
const GENERATION: u64 = 7;

type PluginRx = Receiver<(PluginInstruction, ErrorContext)>;
type JobRx = Receiver<(BackgroundJob, ErrorContext)>;

struct ControlledReader {
    chunks: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    waiting: Option<tokio::sync::oneshot::Sender<()>>,
    current: Vec<u8>,
    offset: usize,
}

impl ControlledReader {
    fn new() -> (
        Self,
        tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (sender, chunks) = tokio::sync::mpsc::unbounded_channel();
        let (waiting, reader_is_waiting) = tokio::sync::oneshot::channel();
        (
            Self {
                chunks,
                waiting: Some(waiting),
                current: vec![],
                offset: 0,
            },
            sender,
            reader_is_waiting,
        )
    }
}

impl AsyncRead for ControlledReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        loop {
            if this.offset < this.current.len() {
                let bytes_read = buf.len().min(this.current.len() - this.offset);
                let end = this.offset + bytes_read;
                buf[..bytes_read].copy_from_slice(&this.current[this.offset..end]);
                this.offset = end;
                return Poll::Ready(Ok(bytes_read));
            }

            match Pin::new(&mut this.chunks).poll_recv(cx) {
                Poll::Ready(Some(next)) => {
                    this.current = next;
                    this.offset = 0;
                },
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => {
                    if let Some(waiting) = this.waiting.take() {
                        let _ = waiting.send(());
                    }
                    return Poll::Pending;
                },
            }
        }
    }
}

fn test_senders() -> (ThreadSenders, PluginRx, JobRx) {
    let (to_plugin, plugin_rx) = unbounded();
    let (to_background_jobs, job_rx) = unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
        ..Default::default()
    };
    (senders, plugin_rx, job_rx)
}

/// Serves exactly one request, then responds with `status` and the given body parts, each
/// written as a separate `Transfer-Encoding: chunked` chunk with a gap in between so they
/// reach the client as distinct reads. Returns the url to hit.
async fn serve_once(
    status: &'static str,
    content_type: &'static str,
    parts: Vec<&'static str>,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // read the request headers so curl isn't left writing into a full buffer
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => return,
                Ok(_) => request.push(byte[0]),
            }
        }
        let head = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\n\r\n",
            status, content_type
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        for part in parts {
            let chunk = format!("{:x}\r\n{}\r\n", part.len(), part);
            socket.write_all(chunk.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
        let _ = socket.flush().await;
    });
    url
}

/// Reads one HTTP request off `socket`: the head up to the blank line, then as many body
/// bytes as its `Content-Length` announces, so a test can see whether a body was sent at all.
/// `None` if the peer hung up first.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        match socket.read(&mut byte).await {
            Ok(0) | Err(_) => return None,
            Ok(_) => request.push(byte[0]),
        }
    }
    let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
    let content_length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        socket.read_exact(&mut body).await.ok()?;
    }
    request.extend(body);
    Some(request)
}

fn header_line<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .split("\r\n\r\n")
        .next()
        .unwrap_or("")
        .lines()
        .find(|line| {
            line.to_ascii_lowercase()
                .starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
}

fn request_body(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// Serves one request with `status` and `location`, then a second one with `200 OK` and the
/// body `redirected`; the second request is handed back whole so the test can see what the
/// client sent after following the redirect. Returns the url to hit.
async fn serve_redirect(
    status: &'static str,
    location: String,
) -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        if read_request(&mut socket).await.is_none() {
            return;
        }
        socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        socket.flush().await.unwrap();
        drop(socket);

        let (mut socket, _) = listener.accept().await.unwrap();
        let Some(request) = read_request(&mut socket).await else {
            return;
        };
        let _ = request_tx.send(request);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nredirected")
            .await
            .unwrap();
    });
    (url, request_rx)
}

/// Serves one `200 OK` and hands back the request it received. A second origin for the
/// cross-origin redirect test: it listens on its own port, which is all `Url::origin`
/// needs to tell it apart from the redirecting server.
async fn serve_capture() -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let Some(request) = read_request(&mut socket).await else {
            return;
        };
        let _ = request_tx.send(request);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nredirected")
            .await
            .unwrap();
    });
    (url, request_rx)
}

/// Answers every request with a 302 back to itself, and counts them.
async fn serve_redirect_loop() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/loop", listener.local_addr().unwrap());
    let hops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hops.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            if read_request(&mut socket).await.is_none() {
                continue;
            }
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            let _ = socket.flush().await;
        }
    });
    (url, hops)
}

/// The allowlist is `http` and `https` with a host; a foreign scheme is refused whether or
/// not it names a file, and so is an `http` URL with nothing to connect to. The `url` crate
/// never yields a parsed `http` URL without a host (it refuses `http://` outright and reads
/// `http:///x` as `http://x/`), so the hostless cases are refused at parse time and the
/// `host_str().is_none()` clause in `web_url` is a second line behind that.
#[test]
fn web_requests_accept_only_http_and_https_urls() {
    assert!(web_url("https://example.com/path").is_ok());
    assert!(web_url("http://example.com/path").is_ok());
    for rejected in [
        "file://localhost/etc/passwd",
        "ftp://example.com/pub",
        "data:text/plain,hello",
        "http://",
        "http:///",
        "http://:8080/",
    ] {
        assert!(web_url(rejected).is_err(), "{rejected} should be refused");
    }
}

/// Only the safelisted headers are kept, in whatever case they were written: a credential
/// header and a custom one alike are dropped, since a custom header may be a key.
#[test]
fn scrub_redirect_headers_keeps_only_safelisted_headers() {
    let mut headers = BTreeMap::from([
        ("Authorization".to_owned(), "secret".to_owned()),
        ("cookie".to_owned(), "secret".to_owned()),
        ("Cookie2".to_owned(), "secret".to_owned()),
        ("PROXY-AUTHORIZATION".to_owned(), "secret".to_owned()),
        ("Www-Authenticate".to_owned(), "secret".to_owned()),
        ("X-Api-Key".to_owned(), "secret".to_owned()),
        ("X-Request-Id".to_owned(), "custom".to_owned()),
        ("Content-Type".to_owned(), "safe".to_owned()),
        ("USER-AGENT".to_owned(), "safe".to_owned()),
    ]);

    scrub_redirect_headers(&mut headers);

    assert_eq!(
        headers,
        BTreeMap::from([
            ("Content-Type".to_owned(), "safe".to_owned()),
            ("USER-AGENT".to_owned(), "safe".to_owned()),
        ])
    );
}

fn credentialed_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Authorization".to_owned(), "Bearer secret".to_owned()),
        ("Cookie".to_owned(), "session=secret".to_owned()),
        ("X-Api-Key".to_owned(), "test_secret".to_owned()),
        ("Accept".to_owned(), "application/json".to_owned()),
    ])
}

/// A redirect to another origin is followed without the credentials: the second request
/// carries neither the bearer token, the cookie nor the API key, but does keep the harmless
/// header.
#[tokio::test(flavor = "multi_thread")]
async fn a_cross_origin_redirect_drops_credentials() {
    let (target, redirected_request) = serve_capture().await;
    let (url, _) = serve_redirect("302 Found", target).await;
    let (status, _, body) = buffered_web_request(
        url,
        HttpVerb::Get,
        credentialed_headers(),
        vec![],
        web_http_client().unwrap(),
        None,
    )
    .await
    .unwrap();

    assert_eq!((status, body.as_slice()), (200, &b"redirected"[..]));
    let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
    assert!(
        header_line(&request, "authorization").is_none(),
        "the bearer token followed a cross-origin redirect:\n{request}"
    );
    assert!(
        header_line(&request, "cookie").is_none(),
        "the cookie followed a cross-origin redirect:\n{request}"
    );
    assert!(
        header_line(&request, "x-api-key").is_none(),
        "the API key followed a cross-origin redirect:\n{request}"
    );
    assert_eq!(
        header_line(&request, "accept"),
        Some("accept: application/json"),
        "{request}"
    );
}

/// Within one origin the credentials stay: an authenticated API that redirects must keep
/// working, so over-scrubbing is as much a bug as under-scrubbing.
#[tokio::test(flavor = "multi_thread")]
async fn a_same_origin_redirect_keeps_credentials() {
    let (url, redirected_request) = serve_redirect("302 Found", "/final".to_owned()).await;
    buffered_web_request(
        url,
        HttpVerb::Get,
        credentialed_headers(),
        vec![],
        web_http_client().unwrap(),
        None,
    )
    .await
    .unwrap();

    let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
    assert!(request.starts_with("GET /final "), "{request}");
    assert_eq!(
        header_line(&request, "authorization"),
        Some("authorization: Bearer secret"),
        "{request}"
    );
    assert_eq!(
        header_line(&request, "cookie"),
        Some("cookie: session=secret"),
        "{request}"
    );
    assert_eq!(
        header_line(&request, "x-api-key"),
        Some("x-api-key: test_secret"),
        "{request}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_http_redirect_to_a_file_url_is_rejected() {
    let (url, _) = serve_redirect("302 Found", "file://localhost/etc/passwd".to_owned()).await;
    let error = buffered_web_request(
        url,
        HttpVerb::Get,
        BTreeMap::new(),
        vec![],
        web_http_client().unwrap(),
        None,
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("web requests only support http and https"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_safe_http_redirect_is_followed() {
    let (url, redirected_request) = serve_redirect("302 Found", "/final".to_owned()).await;
    let (status, _, body) = buffered_web_request(
        url,
        HttpVerb::Get,
        BTreeMap::new(),
        vec![],
        web_http_client().unwrap(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, 200);
    assert_eq!(body, b"redirected");
    assert!(redirected_request
        .await
        .unwrap()
        .starts_with(b"GET /final "));
}

/// The status and the verb together decide whether the redirected request is rewritten to a
/// bodiless GET: 303 rewrites any verb, 301 and 302 rewrite only POST, and 307 and 308
/// rewrite nothing - so PUT and PATCH keep their verb and body on everything but a 303.
/// Rewriting drops the payload too, so a POST body is never replayed as a GET.
#[tokio::test(flavor = "multi_thread")]
async fn redirect_status_and_verb_decide_the_rewrite() {
    for (status, verb, expected_method) in [
        ("301 Moved Permanently", HttpVerb::Post, "GET"),
        ("302 Found", HttpVerb::Post, "GET"),
        ("302 Found", HttpVerb::Put, "PUT"),
        ("303 See Other", HttpVerb::Put, "GET"),
        ("301 Moved Permanently", HttpVerb::Patch, "PATCH"),
        ("302 Found", HttpVerb::Patch, "PATCH"),
        ("303 See Other", HttpVerb::Patch, "GET"),
        ("307 Temporary Redirect", HttpVerb::Patch, "PATCH"),
        ("308 Permanent Redirect", HttpVerb::Patch, "PATCH"),
        ("307 Temporary Redirect", HttpVerb::Post, "POST"),
    ] {
        let (url, redirected_request) = serve_redirect(status, "/final".to_owned()).await;
        tokio::time::timeout(
            yinz_test_support::default_timeout(),
            buffered_web_request(
                url,
                verb,
                BTreeMap::from([
                    ("Content-Length".into(), "7".into()),
                    ("Content-Type".into(), "text/plain".into()),
                    ("Content-Encoding".into(), "identity".into()),
                    ("Content-Language".into(), "en".into()),
                    ("Content-Location".into(), "/original".into()),
                ]),
                b"payload".to_vec(),
                web_http_client().unwrap(),
                None,
            ),
        )
        .await
        .expect("a redirected request must not wait for a discarded body")
        .unwrap();

        let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
        let case = format!("{status} + {verb:?}");
        assert!(
            request.starts_with(&format!("{expected_method} /final ")),
            "{case}: {request}"
        );
        if expected_method == "GET" {
            assert!(
                header_line(&request, "content-length").is_none(),
                "{case}: a rewritten request must not announce a body:\n{request}"
            );
            assert_eq!(request_body(&request), "", "{case}: body replayed as GET");
            for header in [
                "content-type",
                "content-encoding",
                "content-language",
                "content-location",
            ] {
                assert!(
                    header_line(&request, header).is_none(),
                    "{case}: stale {header}: {request}"
                );
            }
        } else {
            assert_eq!(
                request_body(&request),
                "payload",
                "{case}: the body must survive a redirect that keeps the verb"
            );
            assert!(header_line(&request, "content-type")
                .unwrap()
                .ends_with(": text/plain"));
        }
    }
}

/// A redirect loop is a hang without the cap; with it the request fails after exactly
/// `MAX_WEB_REQUEST_REDIRECTS` hops.
#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_loop_fails_at_the_redirect_cap() {
    let (url, hops) = serve_redirect_loop().await;
    let error = buffered_web_request(
        url,
        HttpVerb::Get,
        BTreeMap::new(),
        vec![],
        web_http_client().unwrap(),
        None,
    )
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains(&format!("exceeded {MAX_WEB_REQUEST_REDIRECTS} redirects")),
        "{error}"
    );
    assert_eq!(
        hops.load(Ordering::SeqCst),
        MAX_WEB_REQUEST_REDIRECTS + 1,
        "the first request plus one per permitted redirect"
    );
}

#[tokio::test]
async fn buffered_web_bodies_stop_at_the_limit() {
    let (mut reader, sender, _reader_is_waiting) = ControlledReader::new();
    sender.send(b"five!".to_vec()).unwrap();
    drop(sender);

    let error = read_bounded_web_body(&mut reader, 4).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("exceeded the 4-byte buffered-body limit"));
}

/// The limit is inclusive: a body of exactly the maximum size is the largest one accepted,
/// and it comes back whole.
#[tokio::test]
async fn buffered_web_bodies_of_exactly_the_limit_are_returned_whole() {
    let (mut reader, sender, _reader_is_waiting) = ControlledReader::new();
    sender.send(b"four".to_vec()).unwrap();
    drop(sender);

    let body = read_bounded_web_body(&mut reader, 4).await.unwrap();
    assert_eq!(body, b"four");
}

/// Frames arriving back-to-back are joined into one event rather than one apiece. Every one
/// used to be its own `PluginInstruction`, its own plugin `update()` and its own place in the
/// plugin thread's queue - ahead of whatever the user was typing.
#[tokio::test(start_paused = true)]
async fn a_burst_of_frames_arrives_as_one_event_after_the_coalescing_window() {
    assert_eq!(
        WEB_REQUEST_STREAM_COALESCE_WINDOW,
        Duration::from_millis(20)
    );
    let parts: &[&[u8]] = &[
        b"data: {\"n\":1}\n\n",
        b"data: {\"n\":2}\n\n",
        b"data: {\"n\":3}\n\n",
        b"data: {\"n\":4}\n\n",
        b"data: [DONE]\n\n",
    ];
    let expected = parts.concat();
    let (mut reader, reader_handle, reader_is_waiting) = ControlledReader::new();
    for part in parts {
        reader_handle.send(part.to_vec()).unwrap();
    }
    let state = Arc::new(Mutex::new(WebRequestStreamState::default()));
    let (emitted, mut emitted_chunks) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        forward_streamed_body(&mut reader, "stream-1", &state, move |event| match event {
            Event::WebRequestStreamChunk { chunk, done, .. } => {
                assert!(!done, "the task must never emit the terminator itself");
                emitted.send(chunk).unwrap();
            },
            other => panic!("expected a stream chunk, got {:?}", other),
        })
        .await
    });

    reader_is_waiting.await.unwrap();
    tokio::time::advance(WEB_REQUEST_STREAM_COALESCE_WINDOW - Duration::from_millis(1)).await;
    assert!(
        matches!(
            emitted_chunks.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "the burst must wait for the coalescing deadline"
    );

    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(emitted_chunks.recv().await.unwrap(), expected);
    assert!(
        matches!(
            emitted_chunks.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "the burst must produce exactly one chunk"
    );

    drop(reader_handle);
    assert_eq!(task.await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn cancellation_flushes_bytes_read_before_the_coalescing_deadline() {
    let (mut reader, reader_handle, reader_is_waiting) = ControlledReader::new();
    reader_handle.send(b"final bytes".to_vec()).unwrap();
    let (senders, plugin_rx, _job_rx) = test_senders();
    let state = Arc::new(Mutex::new(WebRequestStreamState {
        response_sent: true,
        ..Default::default()
    }));
    let task_state = state.clone();
    let task_senders = senders.clone();
    let task = tokio::spawn(async move {
        forward_streamed_body(&mut reader, "stream-1", &task_state, move |event| {
            let _ = task_senders.send_to_plugin(PluginInstruction::Update(vec![(
                Some(PLUGIN_ID),
                Some(CLIENT_ID),
                event,
            )]));
        })
        .await
    });

    reader_is_waiting.await.unwrap();
    assert_eq!(state.lock().unwrap().pending, b"final bytes");
    let mut active_streams = HashMap::from([(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: task.abort_handle(),
            generation: GENERATION,
            runtime_generation: None,
            state,
            context: BTreeMap::new(),
        },
    )]);
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        None,
        Some("cancelled".to_owned()),
    );

    let events = drain_events(&plugin_rx);
    assert!(matches!(
        &events[..],
        [
            Event::WebRequestStreamChunk { chunk, done: false, error: None, .. },
            Event::WebRequestStreamChunk { chunk: done_chunk, done: true, error: Some(error), .. }
        ] if chunk == b"final bytes" && done_chunk.is_empty() && error == "cancelled"
    ));
    assert!(task.await.unwrap_err().is_cancelled());
}

async fn serve_truncated_response() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => return,
                Ok(_) => request.push(byte[0]),
            }
        }
        socket
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100\r\n\r\nshort")
            .await
            .unwrap();
    });
    url
}

fn drain_events(plugin_rx: &PluginRx) -> Vec<Event> {
    drain_events_for_generation(plugin_rx, None)
}

fn drain_events_for_generation(plugin_rx: &PluginRx, generation: Option<u64>) -> Vec<Event> {
    plugin_rx
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => {
                assert_eq!(generation, None);
                updates
                    .into_iter()
                    .map(|(plugin_id, client_id, event)| {
                        assert_eq!(plugin_id, Some(PLUGIN_ID));
                        assert_eq!(client_id, Some(CLIENT_ID));
                        event
                    })
                    .collect::<Vec<_>>()
            },
            PluginInstruction::PluginEvent(plugin_id, client_id, actual_generation, event) => {
                assert_eq!(generation, Some(actual_generation));
                assert_eq!(plugin_id, PLUGIN_ID);
                assert_eq!(client_id, CLIENT_ID);
                vec![event]
            },
            other => panic!("unexpected plugin instruction: {other:?}"),
        })
        .collect()
}

fn stream_key() -> StreamKey {
    (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned())
}

async fn run_stream(url: String) -> (Vec<Event>, Vec<BackgroundJob>) {
    let (senders, plugin_rx, job_rx) = test_senders();
    streamed_web_request(
        stream_key(),
        GENERATION,
        Arc::new(Mutex::new(WebRequestStreamState::default())),
        senders,
        url,
        HttpVerb::Get,
        BTreeMap::new(),
        vec![],
        BTreeMap::new(),
        web_http_client().unwrap(),
        None,
        None,
    )
    .await;
    let events = drain_events(&plugin_rx);
    let jobs = job_rx.try_iter().map(|(job, _)| job).collect();
    (events, jobs)
}

#[tokio::test(flavor = "multi_thread")]
async fn streamed_results_and_finish_keep_the_originating_generation() {
    const RUNTIME_GENERATION: u64 = 101; // distinct from the stream registry generation
    let url = serve_once("200 OK", "text/plain", vec!["body"]).await;
    let (senders, plugin_rx, job_rx) = test_senders();
    let state = Arc::new(Mutex::new(WebRequestStreamState::default()));
    streamed_web_request(
        stream_key(),
        GENERATION,
        state.clone(),
        senders.clone(),
        url,
        HttpVerb::Get,
        BTreeMap::new(),
        vec![],
        BTreeMap::new(),
        HttpClient::new().unwrap(),
        None,
        Some(RUNTIME_GENERATION),
    )
    .await;
    let mut active_streams = HashMap::from([(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: tokio::spawn(async {}).abort_handle(),
            generation: GENERATION,
            runtime_generation: Some(RUNTIME_GENERATION),
            state,
            context: BTreeMap::new(),
        },
    )]);
    let (job, _) = job_rx.try_recv().unwrap();
    let BackgroundJob::WebRequestStreamFinished(key, generation, error) = job else {
        panic!("stream must report completion");
    };
    assert_eq!(generation, GENERATION);
    finish_stream(&mut active_streams, &senders, key, Some(generation), error);
    let events = drain_events_for_generation(&plugin_rx, Some(RUNTIME_GENERATION));
    assert!(matches!(&events[..], [
            Event::WebRequestResult(200, _, body, _),
            Event::WebRequestStreamChunk { chunk, done: false, .. },
            Event::WebRequestStreamChunk { done: true, error: None, .. },
        ] if body.is_empty() && chunk == b"body"));
    assert!(active_streams.is_empty());
}

#[tokio::test]
async fn cancelled_stream_tags_synthetic_result_pending_bytes_and_terminator() {
    const RUNTIME_GENERATION: u64 = 101;
    for response_sent in [false, true] {
        let (senders, plugin_rx, _) = test_senders();
        let mut active_streams = HashMap::from([(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: tokio::spawn(async {}).abort_handle(),
                generation: GENERATION,
                runtime_generation: Some(RUNTIME_GENERATION),
                state: Arc::new(Mutex::new(WebRequestStreamState {
                    response_sent,
                    pending: if response_sent {
                        b"pending".to_vec()
                    } else {
                        vec![]
                    },
                    ..Default::default()
                })),
                context: BTreeMap::new(),
            },
        )]);
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            None,
            Some("cancelled".into()),
        );
        let events = drain_events_for_generation(&plugin_rx, Some(RUNTIME_GENERATION));
        assert_eq!(events.len(), 2);
        if response_sent {
            assert!(matches!(&events[0], Event::WebRequestStreamChunk {
                    chunk, done: false, ..
                } if chunk == b"pending"));
        } else {
            assert!(matches!(&events[0], Event::WebRequestResult(400, ..)));
        }
        assert!(matches!(&events[1], Event::WebRequestStreamChunk {
                done: true, error: Some(error), ..
            } if error == "cancelled"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_body_in_chunks_and_reports_finished_once() {
    let url = serve_once(
        "200 OK",
        "text/event-stream",
        vec![
            "data: {\"n\":1}\n\n",
            "data: {\"n\":2}\n\n",
            "data: [DONE]\n\n",
        ],
    )
    .await;
    let (events, jobs) = run_stream(url).await;

    // the leading WebRequestResult carries the status and headers with an empty body
    match &events[0] {
        Event::WebRequestResult(status, headers, body, _) => {
            assert_eq!(*status, 200);
            assert_eq!(
                headers.get("content-type").map(|s| s.as_str()),
                Some("text/event-stream")
            );
            assert!(body.is_empty(), "streamed body must not be buffered");
        },
        other => panic!("expected a leading WebRequestResult, got {:?}", other),
    }

    let chunks: Vec<&Vec<u8>> = events[1..]
        .iter()
        .map(|e| match e {
            Event::WebRequestStreamChunk { chunk, done, .. } => {
                assert!(!done, "the task must never emit the terminator itself");
                chunk
            },
            other => panic!("expected a chunk, got {:?}", other),
        })
        .collect();
    let joined: Vec<u8> = chunks.into_iter().flatten().copied().collect();
    assert_eq!(
        String::from_utf8(joined).unwrap(),
        "data: {\"n\":1}\n\ndata: {\"n\":2}\n\ndata: [DONE]\n\n"
    );

    assert_eq!(
        jobs,
        vec![BackgroundJob::WebRequestStreamFinished(
            (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned()),
            GENERATION,
            None
        )],
        "exactly one finish report, with no error"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn non_2xx_response_is_buffered_whole_with_no_chunks() {
    let url = serve_once(
        "401 Unauthorized",
        "application/json",
        vec!["{\"error\":", "\"bad key\"}"],
    )
    .await;
    let (events, jobs) = run_stream(url).await;

    assert_eq!(events.len(), 1, "an error response is not a stream");
    match &events[0] {
        Event::WebRequestResult(status, _, body, _) => {
            assert_eq!(*status, 401);
            assert_eq!(
                String::from_utf8(body.clone()).unwrap(),
                "{\"error\":\"bad key\"}"
            );
        },
        other => panic!("expected a buffered WebRequestResult, got {:?}", other),
    }
    assert_eq!(
        jobs,
        vec![BackgroundJob::WebRequestStreamFinished(
            (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned()),
            GENERATION,
            None
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_host_reports_an_error_and_still_finishes() {
    let (events, jobs) = run_stream("http://127.0.0.1:1/".to_owned()).await;

    // one synthetic result and nothing after it: no chunk, no second result
    match &events[..] {
        [Event::WebRequestResult(400, _, body, _)] => {
            assert!(
                !body.is_empty(),
                "the body is what the plugin shows for the failure"
            );
        },
        other => panic!("expected exactly one 400 result, got {other:?}"),
    }
    match &jobs[..] {
        [BackgroundJob::WebRequestStreamFinished(_, GENERATION, Some(_))] => {},
        other => panic!(
            "expected one finish report carrying an error, got {:?}",
            other
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn truncated_non_2xx_response_finishes_with_an_error() {
    let (events, jobs) = run_stream(serve_truncated_response().await).await;

    // The partial body is discarded on purpose: the plugin gets the error text in its
    // place, and the same text in the finish report, rather than a body that looks
    // complete but is not.
    let error = match &jobs[..] {
        [BackgroundJob::WebRequestStreamFinished(_, GENERATION, Some(error))] => error,
        other => panic!("expected one finish report carrying an error, got {other:?}"),
    };
    match &events[..] {
        [Event::WebRequestResult(500, _, body, _)] => {
            assert_ne!(body, b"short", "the partial body must not pass as the body");
            assert_eq!(
                std::str::from_utf8(body).unwrap(),
                error,
                "the body is the error text"
            );
        },
        other => panic!("expected exactly one 500 result, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminator_is_emitted_exactly_once_per_stream() {
    let (senders, plugin_rx, _job_rx) = test_senders();
    let mut active_streams = HashMap::new();
    let task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
    active_streams.insert(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: task.abort_handle(),
            generation: GENERATION,
            runtime_generation: None,
            state: Arc::new(Mutex::new(WebRequestStreamState::default())),
            context: BTreeMap::from([("request_id".to_owned(), "7".to_owned())]),
        },
    );

    // a cancel arrives, then the task's own finish report races in behind it - as would
    // happen if the body ended just as the user hit the stop key
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        None,
        Some("cancelled".to_owned()),
    );
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        Some(GENERATION),
        None,
    );

    let events = drain_events(&plugin_rx);
    assert_eq!(
        events.len(),
        2,
        "cancellation before headers must still lead with a result"
    );
    assert!(matches!(
        &events[0],
        Event::WebRequestResult(400, _, _, context)
            if context.get("request_id").map(String::as_str) == Some("7")
    ));
    assert!(matches!(
        &events[1],
        Event::WebRequestStreamChunk { done: true, error: Some(e), .. } if e == "cancelled"
    ));
    assert!(
        task.await.unwrap_err().is_cancelled(),
        "task should be aborted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_stream_terminates_without_a_second_result() {
    let (senders, plugin_rx, _job_rx) = test_senders();
    let mut active_streams = HashMap::new();
    active_streams.insert(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: tokio::spawn(async {}).abort_handle(),
            generation: GENERATION,
            runtime_generation: None,
            state: Arc::new(Mutex::new(WebRequestStreamState {
                // the task already emitted the leading result and its chunks
                response_sent: true,
                ..Default::default()
            })),
            context: BTreeMap::new(),
        },
    );

    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        Some(GENERATION),
        None,
    );

    let events = drain_events(&plugin_rx);
    assert!(
        matches!(
            &events[..],
            [Event::WebRequestStreamChunk {
                done: true,
                error: None,
                chunk,
                ..
            }] if chunk.is_empty()
        ),
        "a completed stream ends with only the terminator, got {:?}",
        events
    );
    assert!(active_streams.is_empty(), "the stream must be deregistered");
}

/// `finish_stream`'s supersede semantics: ending a stream with an error flushes what it
/// had pending before the terminator, and a stale finish for the old generation cannot touch
/// the replacement registered under the same key. Whether the `WebRequest` arm *uses* these
/// semantics is `the_web_request_arm_supersedes_and_cancels_through_the_bus`'s business.
#[tokio::test(flavor = "multi_thread")]
async fn finish_stream_supersede_flushes_pending_bytes_then_terminates() {
    let (senders, plugin_rx, _job_rx) = test_senders();
    let mut active_streams = HashMap::new();
    let old_task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
    active_streams.insert(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: old_task.abort_handle(),
            generation: GENERATION,
            runtime_generation: None,
            state: Arc::new(Mutex::new(WebRequestStreamState {
                response_sent: true,
                pending: b"final bytes".to_vec(),
                ..Default::default()
            })),
            context: BTreeMap::new(),
        },
    );

    // the call the WebRequest arm makes before spawning a replacement under the same key -
    // made by hand here, so this test says nothing about whether the arm makes it
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        None,
        Some("superseded".to_owned()),
    );
    let new_state = Arc::new(Mutex::new(WebRequestStreamState::default()));
    let new_task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
    active_streams.insert(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: new_task.abort_handle(),
            generation: GENERATION + 1,
            runtime_generation: None,
            state: new_state,
            context: BTreeMap::new(),
        },
    );

    let events = drain_events(&plugin_rx);
    assert!(matches!(
        &events[..],
        [
            Event::WebRequestStreamChunk { chunk, done: false, error: None, .. },
            Event::WebRequestStreamChunk { done: true, error: Some(error), .. }
        ] if chunk == b"final bytes" && error == "superseded"
    ));
    assert!(old_task.await.unwrap_err().is_cancelled());

    // the old stream's finish report must not disturb its replacement
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        Some(GENERATION),
        None,
    );
    assert!(drain_events(&plugin_rx).is_empty());
    assert_eq!(
        active_streams.get(&stream_key()).map(|s| s.generation),
        Some(GENERATION + 1)
    );
    new_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_unknown_stream_is_a_no_op() {
    let (senders, plugin_rx, _job_rx) = test_senders();
    finish_stream(&mut HashMap::new(), &senders, stream_key(), None, None);
    assert!(drain_events(&plugin_rx).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_finish_does_not_cancel_replacement_stream() {
    let (senders, plugin_rx, _job_rx) = test_senders();
    let mut active_streams = HashMap::new();
    let task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
    active_streams.insert(
        stream_key(),
        ActiveWebRequestStream {
            abort_handle: task.abort_handle(),
            generation: GENERATION + 1,
            runtime_generation: None,
            state: Arc::new(Mutex::new(WebRequestStreamState::default())),
            context: BTreeMap::new(),
        },
    );

    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        Some(GENERATION),
        None,
    );

    assert!(active_streams.contains_key(&stream_key()));
    assert!(drain_events(&plugin_rx).is_empty());
    finish_stream(
        &mut active_streams,
        &senders,
        stream_key(),
        None,
        Some("cancelled".to_owned()),
    );
    assert!(task.await.unwrap_err().is_cancelled());
}

/// Replies to every connection with a `200 OK` chunked head and a first chunk `hello`, then
/// holds the socket open without ending the body, so a stream against it stays in flight
/// until something on the server side ends it.
async fn serve_open_stream() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = vec![];
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            if read_request(&mut socket).await.is_none() {
                continue;
            }
            socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
                    )
                    .await
                    .unwrap();
            socket.flush().await.unwrap();
            held.push(socket);
        }
    });
    url
}

/// Blocks for the next `count` plugin events, however they are batched.
fn recv_events(plugin_rx: &PluginRx, count: usize) -> Vec<Event> {
    let mut events = vec![];
    while events.len() < count {
        let (instruction, _) = plugin_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("waiting for {count} plugin events, got {events:?}: {e}"));
        match instruction {
            PluginInstruction::Update(updates) => {
                events.extend(updates.into_iter().map(|(_, _, event)| event));
            },
            other => panic!("unexpected plugin instruction {other:?}"),
        }
    }
    events
}

fn terminator(error: Option<&str>) -> Event {
    Event::WebRequestStreamChunk {
        stream_id: "stream-1".to_owned(),
        chunk: vec![],
        done: true,
        error: error.map(str::to_owned),
    }
}

/// Drives the real `WebRequest`, `CancelWebRequest` and `WebRequestStreamFinished` arms of
/// `background_jobs_main` over a bus, rather than calling `finish_stream` by hand. Reusing a
/// stream id must end the stream that holds it before the replacement starts, cancelling
/// must end the replacement, and a stream that completes must be terminated by its own
/// finish report arriving back on the bus. That report carries the generation the arm
/// assigned, so a generation counter that stopped advancing would let a stale report from
/// the superseded stream terminate its replacement here.
#[tokio::test(flavor = "multi_thread")]
async fn the_web_request_arm_supersedes_and_cancels_through_the_bus() {
    let open_url = serve_open_stream().await;
    let (to_plugin, plugin_rx) = unbounded();
    let (to_jobs, job_rx) = unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let to_jobs = SenderWithContext::new(to_jobs);
    let bus = Bus::new(
        vec![job_rx],
        None,
        None,
        Some(&to_plugin),
        None,
        None,
        Some(&to_jobs),
        None,
    )
    .should_silently_fail();
    let main = std::thread::spawn(move || background_jobs_main(bus, None, true));
    let request = |url: &str, stream_id: &str| {
        BackgroundJob::WebRequest(
            PLUGIN_ID,
            CLIENT_ID,
            url.to_owned(),
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            Some(stream_id.to_owned()),
            None,
            None,
        )
    };

    let hello = Event::WebRequestStreamChunk {
        stream_id: "stream-1".to_owned(),
        chunk: b"hello".to_vec(),
        done: false,
        error: None,
    };
    // waiting for the first chunk as well as the result makes the sequence below exact:
    // the stream has nothing pending to flush when it is superseded
    to_jobs.send(request(&open_url, "stream-1")).unwrap();
    let events = recv_events(&plugin_rx, 2);
    assert!(
        matches!(events[0], Event::WebRequestResult(200, ..)),
        "the first stream must be in flight before its id is reused, got {events:?}"
    );
    assert_eq!(events[1], hello);

    // the same id again: the first stream is terminated as superseded before the second
    // one reports in
    to_jobs.send(request(&open_url, "stream-1")).unwrap();
    let events = recv_events(&plugin_rx, 3);
    assert_eq!(events[0], terminator(Some("superseded")));
    assert!(
        matches!(events[1], Event::WebRequestResult(200, ..)),
        "expected the replacement's result, got {:?}",
        events[1]
    );
    assert_eq!(events[2], hello);

    to_jobs
        .send(BackgroundJob::CancelWebRequest(
            PLUGIN_ID,
            CLIENT_ID,
            "stream-1".to_owned(),
        ))
        .unwrap();
    assert_eq!(
        recv_events(&plugin_rx, 1),
        vec![terminator(Some("cancelled"))]
    );

    // a stream that runs to completion is terminated by the finish report the task sends
    // back to this same bus, with no error
    let done_url = serve_once("200 OK", "text/event-stream", vec!["data: 1\n\n"]).await;
    to_jobs.send(request(&done_url, "stream-1")).unwrap();
    let events = recv_events(&plugin_rx, 3);
    assert!(
        matches!(events[0], Event::WebRequestResult(200, ..)),
        "{events:?}"
    );
    assert_eq!(
        events[1],
        Event::WebRequestStreamChunk {
            stream_id: "stream-1".to_owned(),
            chunk: b"data: 1\n\n".to_vec(),
            done: false,
            error: None,
        }
    );
    assert_eq!(events[2], terminator(None));

    to_jobs.send(BackgroundJob::Exit).unwrap();
    main.join().unwrap().unwrap();
    assert!(
        plugin_rx.try_recv().is_err(),
        "nothing may follow a terminator"
    );
}
