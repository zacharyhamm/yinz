//! The downloader against a raw TCP server, so that every status, header and connection close
//! is exactly what the test wrote. Each `download` future completes when its exchange does, so
//! nothing here waits on the clock.

use super::{parse_content_range, ContentRange, Downloader, DownloaderError};
use isahc::error::ErrorKind;
use isahc::http::StatusCode;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

const FILE_NAME: &str = "plugin.wasm";
const BODY: &[u8] = b"0123456789";

/// One canned response for one connection.
struct Exchange {
    /// Status line plus headers, without the terminating blank line.
    head: String,
    body: Vec<u8>,
    /// When set, the socket is dropped after this many body bytes.
    close_after: Option<usize>,
}

impl Exchange {
    fn new(head: &str, body: &[u8]) -> Self {
        Self {
            head: head.to_owned(),
            body: body.to_vec(),
            close_after: None,
        }
    }
    fn closing_after(mut self, bytes: usize) -> Self {
        self.close_after = Some(bytes);
        self
    }
}

/// Serves `exchanges` on sequential connections and hands back the URL to fetch plus a receiver
/// that yields each connection's request head, so a test can assert on the `Range` it carried.
/// Every response closes its connection, so a resume is a fresh `accept` and never a reuse of
/// a dead one.
async fn serve(exchanges: Vec<Exchange>) -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/{FILE_NAME}", listener.local_addr().unwrap());
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        for exchange in exchanges {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match socket.read(&mut byte).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => request.push(byte[0]),
                }
            }
            let _ = request_tx.send(String::from_utf8_lossy(&request).into_owned());
            let head = format!("{}\r\nConnection: close\r\n\r\n", exchange.head);
            socket.write_all(head.as_bytes()).await.unwrap();
            let written = exchange.close_after.unwrap_or(exchange.body.len());
            socket.write_all(&exchange.body[..written]).await.unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        }
    });
    (url, request_rx)
}

fn request_header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (header, value) = line.split_once(':')?;
        header
            .trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

fn range_header(head: &str) -> Option<String> {
    request_header(head, "range")
}

fn fixture() -> (TempDir, Downloader) {
    let dir = TempDir::new().unwrap();
    let downloader = Downloader::new(dir.path().to_path_buf());
    (dir, downloader)
}

fn fixture_without_client() -> (TempDir, Downloader) {
    let dir = TempDir::new().unwrap();
    let downloader = Downloader::with_client(
        Err(ErrorKind::ClientInitialization.into()),
        dir.path().to_path_buf(),
    );
    (dir, downloader)
}

fn cached(dir: &TempDir) -> PathBuf {
    dir.path().join(FILE_NAME)
}

fn part(dir: &TempDir) -> PathBuf {
    dir.path().join(format!("{FILE_NAME}.part"))
}

fn write_part(dir: &TempDir, bytes: &[u8]) {
    std::fs::write(part(dir), bytes).unwrap();
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

fn entries(dir: &TempDir) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn full_response_is_cached_at_the_returned_path() {
    let (dir, downloader) = fixture();
    let (url, mut requests) = serve(vec![Exchange::new(
        "HTTP/1.1 200 OK\r\nContent-Length: 10",
        BODY,
    )])
    .await;

    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(path, cached(&dir));
    assert_eq!(read(&path), BODY);
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()], "no .part left");
    let head = requests.recv().await.unwrap();
    assert_eq!(
        range_header(&head),
        None,
        "a fresh download asks for everything"
    );
    assert_eq!(
        request_header(&head, "accept-encoding").as_deref(),
        Some("identity"),
        "the bytes are requested as stored"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn range_not_satisfiable_on_resume_discards_the_partial_file_and_refetches() {
    let (dir, downloader) = fixture();
    write_part(&dir, BODY);
    let (url, mut requests) = serve(vec![
        Exchange::new(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */10\r\nContent-Length: 4",
            b"nope",
        ),
        Exchange::new("HTTP/1.1 200 OK\r\nContent-Length: 10", BODY),
    ])
    .await;

    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(read(&path), BODY);
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()], "no .part left");
    let first = requests.recv().await.unwrap();
    assert_eq!(range_header(&first).as_deref(), Some("bytes=10-"));
    let second = requests.recv().await.unwrap();
    assert_eq!(
        range_header(&second),
        None,
        "the refetch asks for the whole artifact"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn range_not_satisfiable_on_a_full_request_is_an_error() {
    let (dir, downloader) = fixture();
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 4",
        b"nope",
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::UnexpectedStatus { status, .. } if *status == StatusCode::RANGE_NOT_SATISFIABLE
        ),
        "{err:?}"
    );
    assert!(entries(&dir).is_empty(), "nothing was written");
}

#[tokio::test(flavor = "multi_thread")]
async fn non_authoritative_full_response_is_cached() {
    let (dir, downloader) = fixture();
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 203 Non-Authoritative Information\r\nContent-Length: 10",
        BODY,
    )])
    .await;

    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(read(&path), BODY);
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn success_statuses_without_a_body_are_rejected() {
    let (dir, downloader) = fixture();
    write_part(&dir, b"old");
    let (url, _requests) = serve(vec![Exchange::new("HTTP/1.1 204 No Content", b"")]).await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::UnexpectedStatus { status, .. } if *status == StatusCode::NO_CONTENT
        ),
        "{err:?}"
    );
    assert!(!cached(&dir).exists(), "an empty body is not the artifact");
    assert_eq!(read(&part(&dir)), b"old", "the partial file is left alone");
}

#[tokio::test(flavor = "multi_thread")]
async fn compressed_response_is_rejected_before_anything_is_written() {
    let (dir, downloader) = fixture();
    let gzip_bytes = b"\x1f\x8b\x08\x00not really gzip";
    let (url, mut requests) = serve(vec![Exchange::new(
        &format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}",
            gzip_bytes.len()
        ),
        gzip_bytes,
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::UnexpectedEncoding { url: u, encoding } if *u == url && encoding == "gzip"
        ),
        "{err:?}"
    );
    assert!(
        entries(&dir).is_empty(),
        "encoded bytes must never reach the cache or the partial file"
    );
    let head = requests.recv().await.unwrap();
    assert_eq!(
        request_header(&head, "accept-encoding").as_deref(),
        Some("identity")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn error_status_is_rejected_without_touching_the_partial_file() {
    let (dir, downloader) = fixture();
    write_part(&dir, b"old");
    let (url, mut requests) = serve(vec![Exchange::new(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: 22",
        b"<html>not here</html>\n",
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::UnexpectedStatus { url: u, status } if *u == url && *status == StatusCode::NOT_FOUND
        ),
        "{err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains(&url) && message.contains("404"),
        "{message}"
    );
    assert!(
        !cached(&dir).exists(),
        "an error page must not become the artifact"
    );
    assert_eq!(
        read(&part(&dir)),
        b"old",
        "a failed status leaves the partial file alone"
    );
    let head = requests.recv().await.unwrap();
    assert_eq!(range_header(&head).as_deref(), Some("bytes=3-"));
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_sends_range_and_appends_a_matching_partial_response() {
    let (dir, downloader) = fixture();
    write_part(&dir, &BODY[..4]);
    let (url, mut requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-9/10\r\nContent-Length: 6",
        &BODY[4..],
    )])
    .await;

    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(read(&path), BODY);
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()]);
    let head = requests.recv().await.unwrap();
    assert_eq!(range_header(&head).as_deref(), Some("bytes=4-"));
}

#[tokio::test(flavor = "multi_thread")]
async fn full_response_replaces_a_partial_file() {
    let (dir, downloader) = fixture();
    write_part(&dir, b"garbage!");
    let (url, mut requests) = serve(vec![Exchange::new(
        "HTTP/1.1 200 OK\r\nContent-Length: 10",
        BODY,
    )])
    .await;

    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(
        read(&path),
        BODY,
        "a server that ignores Range sends the whole artifact"
    );
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()]);
    let head = requests.recv().await.unwrap();
    assert_eq!(range_header(&head).as_deref(), Some("bytes=8-"));
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_response_at_the_wrong_offset_is_rejected_and_discards_the_partial_file() {
    let (dir, downloader) = fixture();
    write_part(&dir, &BODY[..4]);
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/10\r\nContent-Length: 10",
        BODY,
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::InvalidRange { expected_offset: 4, content_range: Some(range), .. }
                if range == "bytes 0-9/10"
        ),
        "{err:?}"
    );
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_response_without_content_range_is_rejected() {
    let (dir, downloader) = fixture();
    write_part(&dir, &BODY[..4]);
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 6",
        &BODY[4..],
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::InvalidRange {
                expected_offset: 4,
                content_range: None,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("missing"), "{err}");
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_response_to_a_full_request_is_rejected() {
    let (dir, downloader) = fixture();
    let (url, mut requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/10\r\nContent-Length: 10",
        BODY,
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::InvalidRange {
                expected_offset: 0,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
    let head = requests.recv().await.unwrap();
    assert_eq!(range_header(&head), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_transfer_keeps_the_validated_prefix_and_is_not_promoted() {
    let (dir, downloader) = fixture();
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 200 OK\r\nContent-Length: 10",
        BODY,
    )
    .closing_after(4)])
    .await;

    // curl reports the short body as a transfer error today; a length check catches it if it
    // ever reads as a clean EOF instead. Either way the outcome below is what matters.
    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(err.to_string().contains(&url), "{err}");
    assert!(!cached(&dir).exists());
    assert_eq!(read(&part(&dir)), &BODY[..4]);
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_transfer_resumes_with_a_partial_response() {
    let (dir, downloader) = fixture();
    let (url, mut requests) = serve(vec![
        Exchange::new("HTTP/1.1 200 OK\r\nContent-Length: 10", BODY).closing_after(4),
        Exchange::new(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-9/10\r\nContent-Length: 6",
            &BODY[4..],
        ),
    ])
    .await;

    downloader
        .download(&url, Some(FILE_NAME))
        .await
        .expect_err("the first attempt is cut short");
    let path = downloader.download(&url, Some(FILE_NAME)).await.unwrap();

    assert_eq!(read(&path), BODY);
    assert_eq!(entries(&dir), vec![FILE_NAME.to_owned()]);
    let first = requests.recv().await.unwrap();
    let second = requests.recv().await.unwrap();
    assert_eq!(range_header(&first), None);
    assert_eq!(range_header(&second).as_deref(), Some("bytes=4-"));
}

#[tokio::test(flavor = "multi_thread")]
async fn body_longer_than_the_declared_range_is_rejected() {
    let (dir, downloader) = fixture();
    write_part(&dir, &BODY[..4]);
    // With a Content-Length curl never delivers surplus bytes, so chunked framing is the only
    // way a body can outrun its Content-Range.
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-9/10\r\nTransfer-Encoding: chunked",
        b"8\r\n456789ab\r\n0\r\n\r\n",
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::BodyLengthMismatch {
                expected: 10,
                received: 12,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn short_partial_response_keeps_the_prefix() {
    let (dir, downloader) = fixture();
    write_part(&dir, &BODY[..4]);
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/10\r\nContent-Length: 4",
        &BODY[4..8],
    )])
    .await;

    let err = downloader
        .download(&url, Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::BodyLengthMismatch {
                expected: 10,
                received: 8,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!cached(&dir).exists());
    assert_eq!(
        read(&part(&dir)),
        &BODY[..8],
        "a shorter sub-range is still a prefix"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_file_is_returned_without_a_client_or_request() {
    let (dir, downloader) = fixture_without_client();
    std::fs::write(cached(&dir), b"already here").unwrap();

    let path = downloader
        .download("http://127.0.0.1:9/plugin.wasm", Some(FILE_NAME))
        .await
        .unwrap();

    assert_eq!(path, cached(&dir));
    assert_eq!(read(&path), b"already here");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_construction_failure_is_an_error() {
    let (dir, downloader) = fixture_without_client();

    let err = downloader
        .download("http://127.0.0.1:9/plugin.wasm", Some(FILE_NAME))
        .await
        .unwrap_err();

    assert!(
        matches!(err, DownloaderError::ClientUnavailable(_)),
        "{err:?}"
    );
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[test]
fn parse_content_range_accepts_only_a_satisfied_byte_range() {
    let range = |first, last, complete_length| {
        Some(ContentRange {
            first,
            last,
            complete_length,
        })
    };
    assert_eq!(parse_content_range("bytes 4-9/10"), range(4, 9, Some(10)));
    assert_eq!(parse_content_range("bytes 4-9/*"), range(4, 9, None));
    assert_eq!(parse_content_range("BYTES 0-0/1"), range(0, 0, Some(1)));
    assert_eq!(
        parse_content_range("  bytes  4-9/10 "),
        range(4, 9, Some(10))
    );
    for rejected in [
        "bytes */10",
        "4-9/10",
        "bytes 9-4/10",
        "bytes 4-10/10",
        "bytes 4-9",
        "bytes a-b/c",
        "items 4-9/10",
        "",
    ] {
        assert_eq!(parse_content_range(rejected), None, "{rejected:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn download_without_cache_rejects_error_statuses() {
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4",
        b"oops",
    )])
    .await;

    let err = Downloader::download_without_cache(&url).await.unwrap_err();

    assert!(
        matches!(
            &err,
            DownloaderError::UnexpectedStatus { status, .. } if *status == StatusCode::INTERNAL_SERVER_ERROR
        ),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn download_without_cache_returns_a_successful_body() {
    let (url, _requests) = serve(vec![Exchange::new(
        "HTTP/1.1 200 OK\r\nContent-Length: 9",
        b"layout {}",
    )])
    .await;

    let body = Downloader::download_without_cache(&url).await.unwrap();

    assert_eq!(body, "layout {}");
}
