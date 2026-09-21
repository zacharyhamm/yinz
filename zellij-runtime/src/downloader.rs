use isahc::http::header::{
    HeaderName, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
};
use isahc::http::{HeaderMap, StatusCode};
use isahc::prelude::*;
use isahc::{config::RedirectPolicy, HttpClient, Request};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::{io::AsyncWriteExt as _, sync::Mutex};
use tokio_stream::StreamExt as _;
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tokio_util::io::ReaderStream;
use url::Url;

const STREAM_BUFFER_SIZE_BYTES: usize = 65535;
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Error, Debug)]
pub enum DownloaderError {
    #[error("HTTP client could not be initialised: {0}")]
    ClientUnavailable(#[source] isahc::Error),
    #[error("RequestError: {0}")]
    Request(#[from] isahc::Error),
    #[error("HttpError: {0}")]
    HttpError(#[from] isahc::http::Error),
    #[error("IoError: {0}")]
    Io(#[source] std::io::Error),
    #[error("transfer from {url} failed after {received} bytes: {source}")]
    Transfer {
        url: String,
        received: u64,
        #[source]
        source: std::io::Error,
    },
    #[error("{url} responded with HTTP {status}")]
    UnexpectedStatus { url: String, status: StatusCode },
    #[error("{url} sent a {encoding}-encoded body although an identity encoding was requested")]
    UnexpectedEncoding { url: String, encoding: String },
    #[error(
        "{url} sent a partial response that does not continue the {expected_offset} bytes already downloaded (Content-Range: {})",
        .content_range.as_deref().unwrap_or("missing")
    )]
    InvalidRange {
        url: String,
        expected_offset: u64,
        content_range: Option<String>,
    },
    #[error("download of {url} ended with {received} bytes but {expected} were expected")]
    BodyLengthMismatch {
        url: String,
        expected: u64,
        received: u64,
    },
    #[error("File name cannot be found in URL: {0}")]
    NotFoundFileName(String),
    #[error("Failed to parse URL body: {0}")]
    InvalidUrlBody(String),
}

#[derive(Debug, Clone)]
pub struct Downloader {
    // A failed build is kept rather than dropped: it is the error every request would need to
    // report, and a cached file must still be served without a client.
    client: Result<HttpClient, isahc::Error>,
    location: PathBuf,
    // the whole thing is an Arc/Mutex so that Downloader is thread safe, and the individual values of
    // the HashMap are Arc/Mutexes (Mutexi?) to represent that individual downloads should not
    // happen concurrently
    download_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

fn build_client(stall_timeout: Duration) -> Result<HttpClient, isahc::Error> {
    HttpClient::builder()
        .connect_timeout(Duration::from_secs(10))
        // Bound stalled headers and bodies without limiting an active large download.
        .low_speed_timeout(1, stall_timeout)
        .redirect_policy(RedirectPolicy::Follow)
        .build()
}

/// A satisfied `Content-Range: bytes <first>-<last>/<complete-length | *>` value (RFC 9110 §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRange {
    first: u64,
    last: u64,
    complete_length: Option<u64>,
}

/// Parses a `Content-Range` value, accepting only a satisfied byte range. Returns `None` for a
/// missing or non-`bytes` unit, the unsatisfied `bytes */N` form (only legal on a 416), a range
/// whose end precedes its start, whose exclusive end cannot be represented, or which reaches past
/// the complete length, or unparsable numbers. A `*` complete length is accepted as unknown.
fn parse_content_range(value: &str) -> Option<ContentRange> {
    let value = value.trim();
    let (unit, rest) = value.split_once(|c: char| c.is_ascii_whitespace())?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, complete) = rest.trim().split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let first = first.trim().parse::<u64>().ok()?;
    let last = last.trim().parse::<u64>().ok()?;
    if last < first || last == u64::MAX {
        return None;
    }
    let complete_length = match complete.trim() {
        "*" => None,
        n => {
            let n = n.parse::<u64>().ok()?;
            if last >= n {
                return None;
            }
            Some(n)
        },
    };
    Some(ContentRange {
        first,
        last,
        complete_length,
    })
}

/// Anything outside 2xx is an error naming the URL and the status. The body is not read: the
/// status line is the diagnostic, and an error page must never be mistaken for the artifact.
fn ensure_success(url: &str, status: StatusCode) -> Result<(), DownloaderError> {
    if status.is_success() {
        Ok(())
    } else {
        Err(DownloaderError::UnexpectedStatus {
            url: url.to_owned(),
            status,
        })
    }
}

/// Sends a `GET` for `url` asking for the bytes as stored: no content coding, so that
/// `Content-Length`, `Content-Range` and the `Range` offset all describe what is written to disk.
/// `resume_from` above zero asks for the rest of the artifact from that offset.
async fn send_identity_request(
    client: &HttpClient,
    url: &str,
    resume_from: u64,
) -> Result<isahc::Response<isahc::AsyncBody>, DownloaderError> {
    let mut request = Request::get(url)
        .header("Content-Type", "application/octet-stream")
        .header(ACCEPT_ENCODING, "identity")
        .automatic_decompression(false);
    if resume_from > 0 {
        log::debug!("Resuming download from {} bytes", resume_from);
        request = request.header("Range", format!("bytes={}-", resume_from));
    }
    Ok(client.send_async(request.body(())?).await?)
}

/// Lossy, so a malformed value still shows up in the error that quotes it.
fn header_string(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

fn header_u64(headers: &HeaderMap, name: HeaderName) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// Best effort: the caller's own error is what gets reported, this only stops the next attempt
/// from resuming on bytes that are no longer a trusted prefix of the artifact.
async fn discard_partial_file(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => log::warn!("Failed to remove partial download {:?}: {}", path, e),
    }
}

impl Downloader {
    pub fn new(location: PathBuf) -> Self {
        Self::with_client(build_client(DOWNLOAD_STALL_TIMEOUT), location)
    }

    fn with_client(client: Result<HttpClient, isahc::Error>, location: PathBuf) -> Self {
        Self {
            client,
            location,
            download_locks: Default::default(),
        }
    }

    /// Downloads `url` into this downloader's location, or returns the path it already occupies
    /// there. On success the returned path holds exactly the bytes the server sent for the URL.
    ///
    /// A partial download is kept as `<file_name>.part` and resumed with a `Range` request. A
    /// `206` that continues it is appended; a `200` or `203` replaces it; a `416` means the server
    /// has nothing past the bytes on disk, so the partial file is discarded and the whole artifact
    /// fetched again; any other status is an error that leaves the partial file and the cache
    /// untouched. The bytes are requested with an identity encoding and a response encoded any
    /// other way is rejected before anything is written.
    pub async fn download(
        &self,
        url: &str,
        file_name: Option<&str>,
    ) -> Result<PathBuf, DownloaderError> {
        let file_name = match file_name {
            Some(name) => name.to_string(),
            None => self.parse_name(url)?,
        };

        // we do this to make sure only one download of a specific url is happening at a time
        // otherwise the downloads corrupt each other (and we waste lots of system resources)
        let download_lock = self.acquire_download_lock(&file_name).await;
        // it's important that _lock remains in scope, otherwise it gets dropped and the lock is
        // released before the download is complete
        let _lock = download_lock.lock().await;

        let file_path = self.location.join(file_name.as_str());
        if file_path.exists() {
            log::debug!("File already exists: {:?}", file_path);
            return Ok(file_path);
        }
        let client = self
            .client
            .as_ref()
            .map_err(|e| DownloaderError::ClientUnavailable(e.clone()))?;

        // Only the size is needed to ask for the rest; nothing is opened until the response says
        // what it is.
        let file_part_path = self.location.join(format!("{}.part", file_name));
        let file_part_size = match tokio::fs::metadata(&file_part_path).await {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(DownloaderError::Io(e)),
        };

        let mut file_part_size = file_part_size;
        let mut res = send_identity_request(client, url, file_part_size).await?;
        if res.status() == StatusCode::RANGE_NOT_SATISFIABLE && file_part_size > 0 {
            // The server has nothing past the bytes on disk: the partial file is already the
            // whole artifact, or the artifact has changed under it. Either way it is not a
            // trusted prefix, so start over rather than resend the same Range next time.
            log::debug!(
                "Range from {} bytes not satisfiable; refetching",
                file_part_size
            );
            discard_partial_file(&file_part_path).await;
            file_part_size = 0;
            res = send_identity_request(client, url, 0).await?;
        }

        // Nothing is opened until the status and encoding say the body is the artifact.
        let status = res.status();
        if let Some(encoding) = header_string(res.headers(), CONTENT_ENCODING)
            .filter(|encoding| !encoding.trim().eq_ignore_ascii_case("identity"))
        {
            return Err(DownloaderError::UnexpectedEncoding {
                url: url.to_owned(),
                encoding: encoding.trim().to_owned(),
            });
        }
        // resume_from: Some(offset) appends to the partial file, None replaces it.
        // expected_after: what the partial file must hold once this response has been written.
        // complete: the artifact's full length when the server said it.
        let (resume_from, expected_after, complete) = match status {
            // 203 is a complete body that passed through a transforming proxy. The other 2xx
            // codes carry no body (204, 205) or not the artifact's bytes (226), so they fall
            // through to the error below along with everything outside 2xx.
            StatusCode::OK | StatusCode::NON_AUTHORITATIVE_INFORMATION => {
                let content_length = header_u64(res.headers(), CONTENT_LENGTH);
                (None, content_length, content_length)
            },
            StatusCode::PARTIAL_CONTENT => {
                let content_range = header_string(res.headers(), CONTENT_RANGE);
                let range = content_range
                    .as_deref()
                    .and_then(parse_content_range)
                    .filter(|range| file_part_size > 0 && range.first == file_part_size);
                let Some(range) = range else {
                    // A partial body that cannot be placed after the bytes on disk means those
                    // bytes are no longer a trusted prefix.
                    discard_partial_file(&file_part_path).await;
                    return Err(DownloaderError::InvalidRange {
                        url: url.to_owned(),
                        expected_offset: file_part_size,
                        content_range,
                    });
                };
                (
                    Some(file_part_size),
                    Some(range.last + 1),
                    range.complete_length,
                )
            },
            other => {
                return Err(DownloaderError::UnexpectedStatus {
                    url: url.to_owned(),
                    status: other,
                })
            },
        };

        let mut target = match resume_from {
            None => tokio::fs::File::create(&file_part_path).await,
            Some(_) => {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&file_part_path)
                    .await
            },
        }
        .map_err(DownloaderError::Io)?;
        let mut received = resume_from.unwrap_or(0);

        // From here on every byte written is a prefix of the artifact, so the partial file is
        // kept on failure for the next attempt to resume.
        let body = res.body_mut();
        let mut stream = ReaderStream::with_capacity(body.compat(), STREAM_BUFFER_SIZE_BYTES);
        let transferred = async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|source| DownloaderError::Transfer {
                    url: url.to_owned(),
                    received,
                    source,
                })?;
                target
                    .write_all(&chunk)
                    .await
                    .map_err(DownloaderError::Io)?;
                received += chunk.len() as u64;
            }
            Ok::<(), DownloaderError>(())
        }
        .await;
        // A tokio file completes its writes in the background: flush before the partial file is
        // measured, renamed, or left behind for a resume, whatever the transfer's outcome.
        let flushed = target.flush().await.map_err(DownloaderError::Io);
        drop(target);
        transferred?;
        flushed?;

        // curl already fails a body shorter than its Content-Length; this also covers chunked
        // bodies and a Content-Range that promised a different count than arrived.
        if let Some(expected) = expected_after {
            if received != expected {
                if received > expected {
                    discard_partial_file(&file_part_path).await;
                }
                return Err(DownloaderError::BodyLengthMismatch {
                    url: url.to_owned(),
                    expected,
                    received,
                });
            }
        }
        if let Some(complete) = complete {
            if received < complete {
                // The server honoured a shorter sub-range: a valid prefix, resumed next time.
                return Err(DownloaderError::BodyLengthMismatch {
                    url: url.to_owned(),
                    expected: complete,
                    received,
                });
            }
        }

        log::debug!("Download complete: {:?}", file_part_path);

        tokio::fs::rename(file_part_path, &file_path)
            .await
            .map_err(DownloaderError::Io)?;

        Ok(file_path)
    }
    pub async fn download_without_cache(url: &str) -> Result<String, DownloaderError> {
        let request = Request::get(url)
            .header("Content-Type", "application/octet-stream")
            .body(())?;
        let client = build_client(DOWNLOAD_STALL_TIMEOUT)?;

        let mut res = client.send_async(request).await?;
        ensure_success(url, res.status())?;

        let mut downloaded_bytes: Vec<u8> = vec![];
        let body = res.body_mut();
        let mut stream = ReaderStream::with_capacity(body.compat(), STREAM_BUFFER_SIZE_BYTES);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(DownloaderError::Io)?;
            downloaded_bytes.extend_from_slice(&chunk);
        }

        log::debug!("Download complete");
        let stringified = String::from_utf8(downloaded_bytes)
            .map_err(|e| DownloaderError::InvalidUrlBody(format!("{}", e)))?;

        Ok(stringified)
    }

    /// Download the content of a URL and block for the result.
    ///
    /// Wraps the `async` call to [`download_without_cache`] such that it can be used from sync
    /// code. This is achieved by either:
    ///
    /// 1. Reusing an existing async runtime in case one is present in the current thread, or
    /// 2. Spawning a new async runtime on the current thread
    ///
    /// If neither of these works, an error is returned instead.
    ///
    /// # Note
    ///
    /// At the moment, this function is only here to bridge the gap between the async
    /// [`Downloader`] impl and the sync [`Layout`] code that ultimately calls this function. This
    /// is needed since the Layout code can't trivially be turned `async` without a lot of
    /// refactoring, while the Downloader is used in many other places with async code and can't
    /// sensibly be sync. Maybe in the future, when more code around here is async, we can drop
    /// this function.
    pub fn download_without_cache_blocking(url: &str) -> Result<String, DownloaderError> {
        let runtime_handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.clone(),
            Err(e) if e.is_missing_context() => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .thread_name("ephemeral runtime for downloader implementation")
                    .build()
                    .map_err(DownloaderError::Io)?;
                runtime.handle().clone()
            },
            _ => {
                return Err(DownloaderError::Io(std::io::Error::other(
                    "failed to spawn runtime for download task",
                )))
            },
        };
        runtime_handle.block_on(async move { Downloader::download_without_cache(url).await })
    }

    fn parse_name(&self, url: &str) -> Result<String, DownloaderError> {
        Url::parse(url)
            .map_err(|_| DownloaderError::NotFoundFileName(url.to_string()))?
            .path_segments()
            .ok_or_else(|| DownloaderError::NotFoundFileName(url.to_string()))?
            .next_back()
            .ok_or_else(|| DownloaderError::NotFoundFileName(url.to_string()))
            .map(|s| s.to_string())
    }
    async fn acquire_download_lock(&self, file_name: &str) -> Arc<Mutex<()>> {
        let mut lock_dict = self.download_locks.lock().await;
        let download_lock = lock_dict
            .entry(file_name.to_owned())
            .or_insert_with(Default::default);
        download_lock.clone()
    }
}

#[cfg(test)]
#[path = "unit/downloader_tests.rs"]
mod tests;
