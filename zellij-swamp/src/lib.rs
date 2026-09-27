//! Managed local Swamp execution. The supervisor owns no API connection: the host multiplexes it.
use futures_util::{SinkExt, StreamExt};
use isahc::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use zellij_runtime::errors::prelude::*;

pub mod definitions;

pub const SUPERVISOR_FLAG: &str = "--swamp-server";
const SERVICE: &str = "zellij-service";
const MAX_OUTPUT: usize = 1_048_576;
const STARTUP: Duration = Duration::from_secs(90);

fn timing_enabled() -> bool {
    std::env::var("ZELLIJ_SWAMP_TIMING").as_deref() == Ok("1")
}

// No payload or error text: either can include credentials or command output.
struct Timing {
    start: Instant,
    record: Option<Value>,
}
impl Timing {
    fn new(phase: &str, id: &str, operation: &str, enabled: bool) -> Self {
        Self { start: Instant::now(), record: enabled.then(|| json!({
            "layer":"transport", "phase":phase, "requestId":id,
            "method":operation, "attempt":uuid::Uuid::new_v4().to_string(), "outcome":"error"
        })) }
    }
    fn phase(&self, phase: &str) -> Self {
        let mut record = self.record.clone();
        if let Some(record) = &mut record {
            record["phase"] = json!(phase);
            record["outcome"] = json!("error");
        }
        Self {
            start: Instant::now(),
            record,
        }
    }
    fn emit(&self, phase: &str, outcome: &str, run: Option<&str>) {
        if let Some(mut record) = self.record.clone() {
            record["phase"] = json!(phase);
            record["outcome"] = json!(outcome);
            record["elapsedMs"] = json!(self.start.elapsed().as_secs_f64() * 1000.0);
            record["run"] = json!(run);
            log::info!("swamp_timing {}", record);
        }
    }
    fn ok(&mut self) {
        if let Some(record) = &mut self.record {
            record["outcome"] = json!("ok");
        }
    }
}
impl Drop for Timing {
    fn drop(&mut self) {
        if let Some(record) = &self.record {
            self.emit(
                record["phase"].as_str().unwrap_or("total"),
                record["outcome"].as_str().unwrap_or("error"),
                None,
            );
        }
    }
}

const CAPTURE_LIMIT: usize = 10 * 1024 * 1024;
struct Capture {
    path: PathBuf,
    file: File,
    size: usize,
}
impl Capture {
    fn open(path: PathBuf) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .context("cannot open Swamp diagnostic capture")?;
        let meta = file.metadata().context("cannot inspect Swamp capture")?;
        if !meta.is_file()
            || meta.uid() != nix::unistd::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
            return Err(anyhow!("Swamp capture must be a private regular file"));
        }
        Ok(Self {
            path,
            file,
            size: meta.len() as usize,
        })
    }
    fn write(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            if self.size >= CAPTURE_LIMIT {
                std::fs::rename(&self.path, self.path.with_extension("log.1"))
                    .context("cannot rotate Swamp capture")?;
                *self = Self::open(self.path.clone())?;
            }
            let count = bytes.len().min(CAPTURE_LIMIT - self.size);
            self.file
                .write_all(&bytes[..count])
                .context("cannot write Swamp capture")?;
            self.size += count;
            bytes = &bytes[count..];
        }
        Ok(())
    }
}
async fn drain_capture(mut reader: impl tokio::io::AsyncRead + Unpin, path: Option<PathBuf>) {
    use tokio::io::AsyncReadExt;
    let mut capture = path.and_then(|path| match Capture::open(path) {
        Ok(capture) => Some(capture),
        Err(error) => {
            Err::<(), _>(error).non_fatal();
            None
        },
    });
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => return,
            Ok(count) => {
                if let Some(writer) = &mut capture {
                    if let Err(error) = writer.write(&buffer[..count]) {
                        Err::<(), _>(error).non_fatal();
                        capture = None;
                    }
                }
            },
            Err(error) => {
                Err::<(), _>(error)
                    .context("cannot drain Swamp capture")
                    .non_fatal();
                return;
            },
        }
    }
}

// Deliberately not Debug: credentials must not enter diagnostic output.
#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    root: PathBuf,
    pid: u32,
    started: u64,
    child_pid: u32,
    child_started: u64,
    url: String,
    #[serde(default = "default_bind")]
    bind: IpAddr,
    token: String,
    #[serde(default)]
    extensions: Option<String>,
}

fn extension_directories(
    root: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<PathBuf>> {
    let marker: Value = match std::fs::read_to_string(root.join(".swamp.yaml")) {
        Ok(marker) => {
            serde_saphyr::from_str(&marker).context("cannot parse Swamp repository settings")?
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(e).context("cannot read Swamp repository settings"),
    };
    Ok(["models", "vaults", "datastores", "reports"]
        .into_iter()
        .map(|kind| {
            let directory = env(&format!("SWAMP_{}_DIR", kind.to_uppercase()))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    marker[format!("{kind}Dir")]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| format!("extensions/{kind}"));
            root.join(directory)
        })
        .collect())
}

// Hash source contents, not the mutable bundle catalog or Git revision. Helpers,
// new/deleted files and symlink targets all participate in the same generation.
fn extension_fingerprint(root: &Path) -> Result<String> {
    fn hash_path(path: &Path, hash: &mut Sha256, ancestors: &mut Vec<PathBuf>) -> Result<()> {
        let name = path.as_os_str().as_encoded_bytes();
        hash.update(name.len().to_le_bytes());
        hash.update(name);
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hash.update(b"missing");
                return Ok(());
            },
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("cannot inspect extension {}", path.display()))
            },
        };
        if meta.is_dir() {
            let canonical = path
                .canonicalize()
                .context("cannot resolve extension directory")?;
            if ancestors.contains(&canonical) {
                return Err(anyhow!(
                    "extension directory symlink cycle: {}",
                    path.display()
                ));
            }
            ancestors.push(canonical);
            let mut entries = std::fs::read_dir(path)
                .context("cannot list extension sources")?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            entries.sort();
            for entry in entries {
                if matches!(
                    entry.file_name().and_then(|n| n.to_str()),
                    Some(".git" | ".swamp" | "node_modules" | "target" | "tests")
                ) {
                    continue;
                }
                hash_path(&entry, hash, ancestors)?;
            }
            ancestors.pop();
        } else if meta.is_file() {
            let bytes = std::fs::read(path)
                .with_context(|| format!("cannot read extension {}", path.display()))?;
            hash.update(bytes.len().to_le_bytes());
            hash.update(bytes);
        } else {
            return Err(anyhow!(
                "extension source is not a regular file: {}",
                path.display()
            ));
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    let mut paths = vec![
        root.join("extensions"),
        root.join(".swamp/pulled-extensions"),
        root.join(".swamp/config/pulled-extensions"),
        root.join(".swamp/upstream_extensions.json"),
        root.join(".swamp/config/upstream_extensions.json"),
        root.join(".swamp-sources.yaml"),
        root.join(".swamp.yaml"),
    ];
    if root.join(".swamp-sources.yaml").exists() {
        let sources = cli(
            root,
            "extension source list",
            &["extension", "source", "list"],
        )?;
        for source in sources["sources"]
            .as_array()
            .context("invalid extension source list")?
        {
            for path in source["expandedPaths"]
                .as_array()
                .context("missing expanded extension paths")?
            {
                let path = root.join(path.as_str().context("invalid extension source path")?);
                paths.push(path.join(".swamp.yaml"));
                paths.extend(extension_directories(&path, |_| None)?);
                paths.push(if path.join("extensions").is_dir() {
                    path.join("extensions")
                } else {
                    path
                });
            }
        }
    }
    paths.extend(extension_directories(root, |name| {
        std::env::var(name).ok()
    })?);
    paths.sort();
    paths.dedup();
    for path in paths {
        hash_path(&path, &mut hash, &mut Vec::new())?;
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn check_extensions(root: &Path) -> Result<()> {
    let output = Command::new("swamp")
        .current_dir(root)
        .args(["doctor", "extensions", "--json", "--no-telemetry"])
        .env_remove("SWAMP_REPO_DIR")
        .env_remove("SWAMP_SERVER_URL")
        .env_remove("SWAMP_SERVE_URL")
        .stdin(Stdio::null())
        .output()
        .context("cannot check Swamp extensions")?;
    let report: Value =
        serde_json::from_slice(&output.stdout).context("invalid Swamp extension health report")?;
    if !output.status.success() || report["overallStatus"] != "pass" {
        let errors = report["aggregateState"]["sourceDetails"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|source| source["lastError"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow!(
            "Cannot load updated Swamp extensions. Run swamp doctor extensions in {}. {}",
            root.display(),
            errors
        ));
    }
    check_extension_sources(root, &report)
}

fn check_extension_sources(root: &Path, report: &Value) -> Result<()> {
    // Swamp may retain an old bundle after a broken edit and still report a
    // healthy catalog. Check the actual import graphs with its bundled Deno.
    for source in report["aggregateState"]["sourceDetails"]
        .as_array()
        .context("missing Swamp extension sources")?
    {
        if matches!(
            source["stateTag"].as_str(),
            Some("OrphanedBundleOnly" | "Tombstoned")
        ) {
            continue;
        }
        let path = root.join(
            source["sourcePath"]
                .as_str()
                .context("missing extension source path")?,
        );
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read extension {}", path.display()))
            },
        };
        let mut command = Command::new(
            report["denoPath"]
                .as_str()
                .context("missing bundled Deno path")?,
        );
        command.current_dir(root).args([
            "bundle",
            "--no-lock",
            "--node-modules-dir=none",
            "--external",
            "npm:zod@4",
            "--external",
            "npm:zod",
        ]);
        // Like Swamp's default bundler, externalize inline pinned Zod imports
        // too: Deno matches external specifiers exactly. Bundle other packages.
        for specifier in contents
            .split(['\'', '"'])
            .filter(|specifier| specifier.starts_with("npm:zod@4."))
        {
            command.args(["--external", specifier]);
        }
        let output = command
            .args(["--platform", "deno"])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("cannot verify extension import graph")?;
        if !output.status.success() {
            return Err(anyhow!(
                "Cannot load updated Swamp extensions: {}\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
        Err(error) => return Err(error).context("cannot create Swamp runtime directory"),
    }
    let meta = std::fs::symlink_metadata(path).context("cannot inspect Swamp runtime directory")?;
    if !meta.is_dir() || meta.uid() != nix::unistd::geteuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(anyhow!(
            "Swamp runtime directory must be private and owned by this user"
        ));
    }
    Ok(())
}

fn directory(root: &Path) -> Result<PathBuf> {
    let base = PathBuf::from(format!("/tmp/zellij-swamp-{}", nix::unistd::geteuid()));
    private_dir(&base)?;
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let dir = base.join(digest);
    private_dir(&dir)?;
    Ok(dir)
}

fn lock(root: &Path) -> Result<File> {
    named_lock(root, "lock")
}

fn named_lock(root: &Path, name: &str) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory(root)?.join(name))
        .context("cannot open managed Swamp lock")?;
    file.lock().context("cannot lock managed Swamp checkout")?;
    Ok(file)
}

fn endpoint(root: &Path) -> Result<Option<Endpoint>> {
    let path = directory(root)?.join("endpoint.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let endpoint: Endpoint =
                serde_json::from_slice(&bytes).context("invalid managed Swamp state")?;
            if endpoint.root != root
                || !valid_url(&endpoint.url)
                || url_address(&endpoint.url)
                    .is_none_or(|address| address.ip() != connection_ip(endpoint.bind))
            {
                return Err(anyhow!("managed Swamp checkout or endpoint mismatch"));
            }
            Ok(Some(endpoint))
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("cannot read managed Swamp state"),
    }
}

fn default_bind() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn connection_ip(bind: IpAddr) -> IpAddr {
    match bind {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    }
}

fn url_address(value: &str) -> Option<SocketAddr> {
    let uri = value.parse::<isahc::http::Uri>().ok()?;
    if !matches!(uri.scheme_str(), Some("ws" | "wss")) || uri.path() != "/" || uri.query().is_some()
    {
        return None;
    }
    let address: SocketAddr = uri.authority()?.as_str().parse().ok()?;
    (address.port() != 0).then_some(address)
}

fn valid_url(value: &str) -> bool {
    url_address(value).is_some()
}

fn announced_url(announcement: &Value, bind: IpAddr) -> Result<String> {
    let url = announcement["url"]
        .as_str()
        .context("Swamp announced no endpoint")?;
    let ip: IpAddr = announcement["host"]
        .as_str()
        .context("Swamp announced no host")?
        .parse()
        .context("Swamp announced an invalid host")?;
    let port = u16::try_from(
        announcement["port"]
            .as_u64()
            .context("Swamp announced no port")?,
    )
    .context("Swamp announced an invalid port")?;
    let address = SocketAddr::new(ip, port);
    let scheme = if url.starts_with("wss://") {
        "wss"
    } else {
        "ws"
    };
    // Older Swamp builds announce IPv6 without brackets; use the structured fields.
    if port == 0
        || (url != format!("{scheme}://{address}") && url != format!("{scheme}://{ip}:{port}"))
    {
        return Err(anyhow!("Swamp announced an inconsistent endpoint"));
    }
    if address.ip() != bind
        || (!bind.is_loopback() && scheme != "wss")
        || announcement["detachRuns"] != true
        || announcement["schedulingEnabled"] != false
    {
        return Err(anyhow!("Swamp is incompatible: authenticated detached execution on the requested bind address is required"));
    }
    Ok(format!(
        "{scheme}://{}",
        SocketAddr::new(connection_ip(bind), address.port())
    ))
}

impl Endpoint {
    fn public_metadata(&self) -> Value {
        let port = url_address(&self.url).map(|address| address.port());
        json!({"bind":self.bind,"port":port,"dashboardUrl":format!("{}/dashboard", self.url.replacen("ws", "http", 1).trim_end_matches('/'))})
    }
}

/// Public endpoint data only; service credentials stay in the host.
pub fn public_endpoint(root: &Path) -> Result<Value> {
    Ok(endpoint(root)?
        .filter(alive)
        .map(|state| state.public_metadata())
        .unwrap_or_else(|| json!({})))
}

fn process_started(pid: u32) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.start_time())
}

fn alive(endpoint: &Endpoint) -> bool {
    process_started(endpoint.child_pid) == Some(endpoint.child_started)
}

fn publish(endpoint: &Endpoint) -> Result<()> {
    let dir = directory(&endpoint.root)?;
    let mut file =
        tempfile::NamedTempFile::new_in(&dir).context("cannot stage Swamp credentials")?;
    serde_json::to_writer(file.as_file_mut(), endpoint)
        .context("cannot serialize Swamp credentials")?;
    file.as_file_mut()
        .flush()
        .context("cannot flush Swamp credentials")?;
    file.persist(dir.join("endpoint.json"))
        .map_err(|e| e.error)
        .context("cannot publish Swamp credentials")?;
    Ok(())
}

fn cli(root: &Path, operation: &'static str, args: &[&str]) -> Result<Value> {
    let mut command = Command::new("swamp");
    for (key, _) in std::env::vars_os() {
        if matches!(
            key.to_str(),
            Some(
                "SWAMP_REPO_DIR"
                    | "SWAMP_SERVER_URL"
                    | "SWAMP_SERVE_URL"
                    | "SWAMP_SERVER_TOKEN"
                    | "SWAMP_SERVE_EXTRA_HEADERS"
            )
        ) {
            command.env_remove(key);
        }
    }
    let output = command
        .current_dir(root)
        .args(args)
        .args(["--json", "--no-telemetry"])
        .stdin(Stdio::null())
        .output();
    bootstrap_response(root, operation, output)
}

fn bootstrap_response(
    root: &Path,
    operation: &'static str,
    output: std::io::Result<std::process::Output>,
) -> Result<Value> {
    // Never include arguments, command output, or parser errors: they can contain credentials.
    let context = format!("Swamp bootstrap {operation} in {}", root.display());
    let output = output
        .map_err(|error| anyhow!("{context}: cannot execute command ({:?})", error.kind()))?;
    if !output.status.success() {
        return Err(anyhow!("{context}: command failed ({})", output.status));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| anyhow!("{context}: invalid JSON response ({})", output.status))
}

fn credential(root: &Path) -> Result<String> {
    service_token(root, |operation, args| cli(root, operation, args))
}

fn service_token(
    root: &Path,
    mut run: impl FnMut(&'static str, &[&str]) -> Result<Value>,
) -> Result<String> {
    let listing = run("token list", &["access", "token", "list"])?;
    let invalid_listing = || {
        anyhow!(
            "Swamp bootstrap token list in {}: invalid token listing",
            root.display()
        )
    };
    let tokens = listing.as_array().ok_or_else(invalid_listing)?;
    // Validate every row before treating a missing name as permission to mint.
    for token in tokens {
        if !token["name"].as_str().is_some_and(|name| !name.is_empty())
            || !matches!(
                token["state"].as_str(),
                Some("active" | "expired" | "revoked")
            )
        {
            return Err(invalid_listing());
        }
    }
    if let Some(token) = tokens.iter().find(|token| token["name"] == SERVICE) {
        if token["state"] == "revoked" {
            return Err(anyhow!(
                "Swamp bootstrap token list in {}: zellij-service token is revoked",
                root.display()
            ));
        }
        run(
            "token rotate",
            &["access", "token", "rotate", SERVICE, "--duration", "7d"],
        )?;
    } else {
        run(
            "token mint",
            &[
                "access",
                "token",
                "mint",
                SERVICE,
                "--principal",
                "user:zellij-service",
                "--duration",
                "7d",
            ],
        )?;
    }
    let value = run(
        "token reveal",
        &["access", "token", "reveal", SERVICE, "--yes"],
    )?;
    value["token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .with_context(|| {
            format!(
                "Swamp bootstrap token reveal in {}: missing service credential",
                root.display()
            )
        })
}

fn http(endpoint: &Endpoint, path: &str, post: bool) -> Result<Value> {
    let url = format!(
        "{}{}",
        endpoint.url.replacen("ws", "http", 1).trim_end_matches('/'),
        path
    );
    let mut request = isahc::Request::builder()
        .method(if post { "POST" } else { "GET" })
        .uri(url)
        .header("Authorization", format!("Bearer {}", endpoint.token))
        .timeout(Duration::from_secs(10));
    if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
        request = request.ssl_ca_certificate(isahc::config::CaCertificate::file(path));
    }
    let request = request
        .body(())
        .context("cannot build managed Swamp request")?;
    let mut response =
        isahc::send(request).map_err(|_| anyhow!("managed Swamp HTTP connection failed"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "managed Swamp request failed with HTTP {}",
            response.status()
        ));
    }
    let body = response
        .text()
        .context("cannot read managed Swamp response")?;
    serde_json::from_str(&body).context("invalid managed Swamp response")
}

/// Spawned before the application initializes. This process only supervises Swamp and renews its token.
pub fn supervise(root: PathBuf) -> Result<()> {
    unsafe {
        close_fds::close_open_fds(3, &[]);
    }
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp control checkout")?;
    let startup_lock = named_lock(&root, "startup-lock")?;
    if let Some(state) = endpoint(&root)? {
        if alive(&state) {
            return Ok(());
        }
        return Err(anyhow!(
            "managed Swamp server exited; inspect runs, then use zellij workflow server restart"
        ));
    }
    let help = cli(&root, "serve help", &["help", "serve"])?;
    if help["version"]
        .as_str()
        .is_some_and(|version| version.ends_with("375d7eca"))
    {
        return Err(anyhow!("installed Swamp cannot resume inline model types; use a compatible build and run extensions/tests/serve_compatibility.ts"));
    }
    let config = zellij_project_config::access_root(&root, None).map_err(|e| anyhow!(e))?;
    if let Some(error) = config.error {
        return Err(anyhow!(error));
    }
    let bind = config.config.server_bind.unwrap_or_else(default_bind);
    let extensions = extension_fingerprint(&root)?;
    check_extensions(&root)?;
    let token = credential(&root)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start Swamp supervisor runtime")?;
    runtime.block_on(async move {
        let mut command = tokio::process::Command::new("swamp");
        for (key, _) in std::env::vars_os() {
            if matches!(key.to_str(), Some("SWAMP_REPO_DIR" | "SWAMP_SERVER_URL" | "SWAMP_SERVE_URL" | "SWAMP_SERVER_TOKEN" | "SWAMP_SERVE_EXTRA_HEADERS")) { command.env_remove(key); }
        }
        let capture_dir = if timing_enabled() {
            command.env("OTEL_TRACES_EXPORTER", "console");
            Some(directory(&root)?)
        } else { None };
        let mut child = command.current_dir(&root).args(["serve", "--host", &bind.to_string(), "--port", "0", "--dashboard",
            "--auth-mode", "token", "--admins", "user:zellij-service", "--no-schedule", "--json", "--no-telemetry"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(if capture_dir.is_some() { Stdio::piped() } else { Stdio::null() }).kill_on_drop(true)
            .spawn().context("cannot start Swamp serve")?;
        let mut captures = Vec::new();
        if let Some(stderr) = child.stderr.take() {
            captures.push(tokio::spawn(drain_capture(stderr, capture_dir.as_ref().map(|dir| dir.join("stderr.log")))));
        }
        let mut lines = BufReader::new(child.stdout.take().context("missing Swamp startup pipe")?).lines();
        let announcement = tokio::time::timeout(STARTUP, async {
            while let Some(line) = lines.next_line().await.context("cannot read Swamp startup")? {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if value["status"] == "listening" { return Ok(value); }
                }
            }
            Err(anyhow!("Swamp exited before announcing an endpoint"))
        }).await.context("Swamp startup timed out")??;
        // Keep the buffered reader: bytes after the announcement may already be buffered.
        let capturing = capture_dir.is_some();
        let stdout = tokio::spawn(drain_capture(lines.into_inner(), capture_dir.map(|dir| dir.join("stdout.log"))));
        if capturing { captures.push(stdout); }
        let url = announced_url(&announcement, bind)?;
        let child_pid = child.id().context("Swamp exited during startup")?;
        let mut state = Endpoint { root, pid: std::process::id(), started: process_started(std::process::id()).context("cannot identify Swamp supervisor")?, child_pid,
            child_started: process_started(child_pid).context("cannot identify Swamp process")?, url, bind, token, extensions: Some(extensions) };
        let check = state.clone();
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + STARTUP;
            while Instant::now() < deadline {
                if http(&check, "/ready", false).is_ok() { return Ok(()); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(anyhow!("Swamp did not become ready"))
        }).await.context("Swamp readiness task failed")??;
        if state.extensions.as_deref() != Some(&extension_fingerprint(&state.root)?) {
            state.extensions = None;
        }
        publish(&state)?;
        drop(startup_lock);
        let mut renewal = tokio::time::interval(Duration::from_secs(24 * 3600));
        renewal.tick().await;
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).context("cannot watch supervisor termination")?;
        let result = loop {
            tokio::select! {
                status = child.wait() => { break status.context("cannot reap Swamp").map(|_| ()); },
                _ = term.recv() => {
                    if let Some(pid) = child.id() {
                        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGTERM).context("cannot stop Swamp")?;
                    }
                    if tokio::time::timeout(Duration::from_secs(10), child.wait()).await.is_err() { child.kill().await.context("cannot reap stopped Swamp")?; }
                    break Ok(());
                },
                _ = renewal.tick() => {
                    let root = state.root.clone();
                    match tokio::task::spawn_blocking(move || credential(&root)).await.context("credential renewal task failed").and_then(|result| result) {
                        Ok(token) => { state.token = token; publish(&state).non_fatal(); },
                        Err(error) => { Err::<(), _>(error).context("Swamp credential renewal failed; existing executions remain active").non_fatal(); },
                    }
                }
            }
        };
        // Drain final spans after exit. A descendant holding a pipe must not stall shutdown.
        for capture in captures {
            tokio::time::timeout(Duration::from_secs(1), capture).await
                .context("Swamp capture drain timed out")
                .and_then(|result| result.context("Swamp capture task failed")).non_fatal();
        }
        result
    })
}

fn ensure_locked(root: &Path) -> Result<Endpoint> {
    if let Some(state) = endpoint(root)? {
        if alive(&state) {
            return Ok(state);
        }
        return Err(anyhow!(
            "managed Swamp server exited; inspect runs, then use zellij workflow server restart"
        ));
    }
    let binary = std::env::current_exe().context("cannot locate Zellij supervisor binary")?;
    #[cfg(test)]
    let binary = std::env::var_os("ZELLIJ_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or(binary);
    let mut command = Command::new(binary);
    command
        .args([std::ffi::OsStr::new(SUPERVISOR_FLAG), root.as_os_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("cannot start managed Swamp supervisor")?;
    let deadline = Instant::now() + STARTUP;
    loop {
        if let Some(state) = endpoint(root)? {
            // Reap without binding supervisor lifetime to this application.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(state);
        }
        if child
            .try_wait()
            .context("cannot inspect Swamp startup")?
            .is_some()
        {
            let mut diagnostic = String::new();
            if let Some(stderr) = child.stderr.take() {
                stderr
                    .take(16_384)
                    .read_to_string(&mut diagnostic)
                    .context("cannot read Swamp startup diagnostic")?;
            }
            return Err(anyhow!(
                "managed Swamp startup failed: {}",
                diagnostic.trim()
            ));
        }
        if Instant::now() >= deadline {
            // This is our newly created process group, never an ID from persisted state.
            nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            )
            .ok();
            child
                .wait()
                .context("cannot reap timed-out Swamp startup")?;
            return Err(anyhow!("managed Swamp startup timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn prewarm(root: &Path) -> Result<()> {
    prewarm_request(root, "")
}

pub fn prewarm_request(root: &Path, id: &str) -> Result<()> {
    let mut timing = Timing::new("prewarm", id, "prewarm", timing_enabled());
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp checkout")?;
    let _guard = lock(&root)?;
    ensure_locked(&root)?;
    timing.ok();
    Ok(())
}

fn allowed(operation: &str) -> bool {
    matches!(
        operation,
        "workflow.search"
            | "workflow.get"
            | "workflow.schema"
            | "workflow.create"
            | "workflow.edit"
            | "workflow.delete"
            | "workflow.evaluate"
            | "workflow.approvals"
            | "workflow.reject"
            | "report.search"
            | "report.get"
            | "model.search"
            | "model.get"
            | "model.type.search"
            | "model.type.describe"
            | "model.method.describe"
            | "workflow.validate"
            | "workflow.run"
            | "workflow.history.get"
            | "workflow.history.search"
            | "workflow.history.logs"
            | "workflow.approve"
            | "workflow.resume"
            | "model.method.run"
            | "data.query"
            | "run.attach"
            | "run.history"
    )
}
fn read_request(operation: &str) -> bool {
    matches!(
        operation,
        "workflow.search"
            | "workflow.get"
            | "workflow.schema"
            | "workflow.approvals"
            | "report.search"
            | "report.get"
            | "model.search"
            | "model.get"
            | "model.type.search"
            | "model.type.describe"
            | "model.method.describe"
            | "workflow.validate"
            | "workflow.history.get"
            | "workflow.history.search"
            | "workflow.history.logs"
            | "data.query"
            | "run.history"
    )
}

struct Request {
    id: String,
    operation: String,
    payload: Value,
    events: mpsc::UnboundedSender<Value>,
    run: Option<String>,
    cursor: u64,
    guard: Option<File>,
    sent_at: Option<Instant>,
    server_pid: u32,
    timing: Timing,
    first_response: bool,
}
type Registry = Mutex<HashMap<PathBuf, mpsc::Sender<Request>>>;
static CONNECTIONS: OnceLock<Registry> = OnceLock::new();

/// Each caller has a correlated stream; exactly one socket reader/writer runs per checkout.
pub async fn request(
    root: PathBuf,
    id: String,
    operation: String,
    payload: Value,
) -> Result<mpsc::UnboundedReceiver<Value>> {
    if !allowed(&operation)
        || id.is_empty()
        || id.len() > 4096
        || !payload.is_object()
        || payload.to_string().len() > 2 * MAX_OUTPUT
    {
        return Err(anyhow!("invalid or unsupported managed Swamp request"));
    }
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp checkout")?;
    let (events, receiver) = mpsc::unbounded_channel();
    let sender = {
        let mut registry = CONNECTIONS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow!("Swamp connection registry poisoned"))?;
        registry
            .entry(root.clone())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(128);
                tokio::spawn(connection(root.clone(), receiver));
                sender
            })
            .clone()
    };
    // Return the stream before admission: waiting for an idle server is progress,
    // not an execution timeout, and must not block callbacks or observation.
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut payload = payload;
            let callback = payload
                .as_object_mut()
                .and_then(|p| p.remove("_zellij_active_callback"))
                == Some(json!(true));
            let fresh = !callback
                && matches!(
                    operation.as_str(),
                    "workflow.validate" | "workflow.run" | "workflow.resume" | "model.method.run"
                );
            let startup = fresh || endpoint(&root)?.is_none();
            let queued = startup || (!read_request(&operation) && operation != "run.attach");
            if queued {
                let _ = events.send(
                    json!({"id":id,"type":"managed.progress","payload":{"status":"waiting"}}),
                );
            }
            if startup {
                let warm = root.clone();
                let warm_id = id.clone();
                tokio::task::spawn_blocking(move || prewarm_request(&warm, &warm_id))
                    .await
                    .context("Swamp startup task failed")??;
            }
            let guard = if fresh {
                Some(admit(&root, &id, &events).await?)
            } else if read_request(&operation) || operation == "run.attach" {
                None
            } else {
                let root = root.clone();
                Some(
                    tokio::task::spawn_blocking(move || lock(&root))
                        .await
                        .context("Swamp dispatch lock task failed")??,
                )
            };
            if events.is_closed() {
                return Ok(());
            }
            if queued {
                let _ = events
                    .send(json!({"id":id,"type":"managed.progress","payload":{"status":"ready"}}));
            }
            let run = (operation == "run.attach")
                .then(|| payload["runId"].as_str().map(str::to_owned))
                .flatten();
            let cursor = payload["afterSeq"].as_u64().unwrap_or(0);
            let timing = Timing::new("total", &id, &operation, timing_enabled());
            let server_pid = endpoint(&root)?
                .context("managed Swamp has no endpoint")?
                .child_pid;
            sender
                .send(Request {
                    id: id.clone(),
                    operation,
                    payload,
                    events: events.clone(),
                    run,
                    cursor,
                    guard,
                    sent_at: None,
                    server_pid,
                    timing,
                    first_response: false,
                })
                .await
                .context("managed Swamp connection is unavailable")?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = events.send(json!({"type":"error", "id":id, "error":{"code":"not_dispatched", "message":format!("{error:#}")}}));
        }
    });
    Ok(receiver)
}

fn running(
    root: &Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool>> + Send + '_>> {
    // Reuse the shared socket: repeated handshakes hit Swamp's IP rate limit.
    // Reads bypass admission, so this is safe while holding the dispatch lock.
    // Boxing breaks the request -> admit -> running -> request future type cycle.
    Box::pin(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut events = request(
                root.to_owned(),
                uuid::Uuid::new_v4().to_string(),
                "run.history".into(),
                json!({"active":true}),
            )
            .await?;
            while let Some(frame) = events.recv().await {
                if frame["type"] == "managed.progress" {
                    continue;
                }
                if frame["type"] == "error" {
                    return Err(anyhow!("cannot verify Swamp activity: {}", frame["error"]));
                }
                if let Some(payload) = frame.get("payload") {
                    return Ok(payload.get("data").unwrap_or(payload)["runs"]
                        .as_array()
                        .context("cannot verify active Swamp executions")?
                        .iter()
                        .any(|run| run["status"] == "running"));
                }
            }
            Err(anyhow!("missing Swamp activity response"))
        })
        .await
        .context("Swamp activity check timed out")?
    })
}

async fn admit(root: &Path, id: &str, events: &mpsc::UnboundedSender<Value>) -> Result<File> {
    let mut last = "waiting";
    loop {
        if events.is_closed() {
            return Err(anyhow!("queued Swamp request was abandoned"));
        }
        let path = root.to_owned();
        let (guard, state, fingerprint) = tokio::task::spawn_blocking(move || {
            let guard = lock(&path)?;
            let state = endpoint(&path)?.context("managed Swamp has no endpoint")?;
            if !alive(&state) { return Err(anyhow!("managed Swamp server exited; inspect runs, then use zellij workflow server restart")); }
            Ok::<_, anyhow::Error>((guard, state, extension_fingerprint(&path)?))
        }).await.context("Swamp admission task failed")??;
        if state.extensions.as_ref() == Some(&fingerprint) {
            return Ok(guard);
        }
        let busy = running(root).await?;
        let status = if busy { "waiting" } else { "reloading" };
        if last != status {
            let _ =
                events.send(json!({"id":id,"type":"managed.progress","payload":{"status":status}}));
            last = status;
        }
        if busy {
            drop(guard);
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let path = root.to_owned();
        tokio::task::spawn_blocking(move || check_extensions(&path))
            .await
            .context("Swamp extension check failed")??;
        stop_locked(root, &state).await?;
        // Keep dispatch excluded until publication; supervisor startup uses its
        // own lock so simultaneous startup contenders still share one process.
        let path = root.to_owned();
        tokio::task::spawn_blocking(move || ensure_locked(&path))
            .await
            .context("Swamp refresh failed")??;
        drop(guard);
    }
}

async fn stop_locked(root: &Path, state: &Endpoint) -> Result<()> {
    if alive(state) {
        let pid = if process_started(state.pid) == Some(state.started) {
            state.pid
        } else {
            state.child_pid
        };
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).context("invalid supervisor PID")?),
            nix::sys::signal::Signal::SIGTERM,
        )
        .context("cannot stop Swamp supervisor")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while alive(state) {
            if Instant::now() > deadline {
                return Err(anyhow!("Swamp supervisor did not stop"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    std::fs::remove_file(directory(root)?.join("endpoint.json"))
        .context("cannot remove stopped Swamp endpoint")?;
    Ok(())
}

fn failure(id: &str, message: &str) -> Value {
    json!({"type":"error", "id":id, "error":{"code":"transport_unknown", "message":message}})
}

fn bound_frame(mut frame: Value) -> Value {
    // History inputs duplicate the full saved test log. Its exact version is queried separately.
    if frame["type"] == "workflow.history.get" || frame["type"] == "workflow.history.search" {
        let data = &mut frame["payload"]["data"];
        fn strip(row: &mut Value) {
            if let Some(test) = row
                .pointer_mut("/inputs/state/test")
                .and_then(Value::as_object_mut)
            {
                test.remove("output");
            }
        }
        strip(data);
        if let Some(rows) = data["results"].as_array_mut() {
            for row in rows {
                strip(row);
            }
        }
    }
    if frame.to_string().len() > MAX_OUTPUT {
        failure(
            frame["id"].as_str().unwrap_or(""),
            "Swamp response exceeds the bounded preview; retrieve saved data by exact version",
        )
    } else {
        frame
    }
}

async fn socket(
    root: &Path,
) -> Result<(
    Endpoint,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
)> {
    let root = root.to_owned();
    // Startup has its own lock. Dispatchers acquire their mutation lock only after prewarming.
    let state = tokio::task::spawn_blocking(move || {
        endpoint(&root)?.context("managed Swamp has no endpoint")
    })
    .await
    .context("Swamp endpoint task failed")??;
    let mut request = state
        .url
        .clone()
        .into_client_request()
        .map_err(|_| anyhow!("invalid managed Swamp endpoint"))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("bearer.{}", state.token)
            .parse()
            .map_err(|_| anyhow!("invalid Swamp credential"))?,
    );
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(16 * MAX_OUTPUT));
    let address = url_address(&state.url).context("invalid managed Swamp endpoint")?;
    let (socket, _) = tokio::time::timeout(Duration::from_secs(10), async {
        let stream = tokio::net::TcpStream::connect(address).await?;
        stream.set_nodelay(true)?;
        let stream = if state.url.starts_with("wss://") {
            let connector = tokio_native_tls::TlsConnector::from(
                tokio_native_tls::native_tls::TlsConnector::new()?,
            );
            // TLS verifies an IP literal without the brackets required by an IPv6 URL.
            tokio_tungstenite::MaybeTlsStream::NativeTls(
                connector.connect(&address.ip().to_string(), stream).await?,
            )
        } else {
            tokio_tungstenite::MaybeTlsStream::Plain(stream)
        };
        Ok::<_, anyhow::Error>(
            tokio_tungstenite::client_async_with_config(request, stream, Some(config)).await?,
        )
    })
    .await
    .context("Swamp connection timed out")?
    .map_err(|_| anyhow!("managed Swamp WebSocket connection failed"))?;
    Ok((state, socket))
}

async fn connection(root: PathBuf, mut incoming: mpsc::Receiver<Request>) {
    let mut pending: HashMap<String, Request> = HashMap::new();
    let mut deadlines = tokio::time::interval(Duration::from_secs(1));
    loop {
        if pending.is_empty() {
            let Some(request) = incoming.recv().await else {
                return;
            };
            request
                .timing
                .emit("queue_wait", "ok", request.run.as_deref());
            pending.insert(request.id.clone(), request);
        }
        let connected = Instant::now();
        let result = socket(&root).await;
        for request in pending.values() {
            let mut record = Timing {
                start: connected,
                record: request.timing.record.clone(),
            };
            record.emit(
                "connect",
                if result.is_ok() { "ok" } else { "error" },
                request.run.as_deref(),
            );
            record.record = None;
        }
        let (connected_state, mut ws) = match result {
            Ok(socket) => socket,
            Err(error) => {
                for (_, request) in pending.drain() {
                    let _ = request
                        .events
                        .send(failure(&request.id, &error.to_string()));
                }
                continue;
            },
        };
        let mut broken = false;
        for request in pending.values_mut() {
            request.first_response = false;
            let frame = if let Some(run) = &request.run {
                json!({"type":"run.attach", "id":request.id, "payload":{"runId":run,"afterSeq":request.cursor}})
            } else {
                json!({"type":request.operation,"id":request.id,"payload":request.payload})
            };
            let mut send = request.timing.phase("send");
            request.sent_at = Some(Instant::now());
            if ws
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                broken = true;
                break;
            }
            send.ok();
        }
        while !broken {
            tokio::select! {
                _ = deadlines.tick() => {
                    pending.retain(|id, request| {
                        if request.run.is_none() && request.sent_at.is_some_and(|sent| sent.elapsed() > STARTUP) {
                            let _ = request.events.send(failure(id,"Swamp request timed out; reconcile before dispatching a mutation again"));
                            false
                        } else { true }
                    });
                },
                request = incoming.recv() => {
                    let Some(mut request) = request else { return; };
                    if pending.contains_key(&request.id) {
                        let _ = request.events.send(failure(&request.id, "request identity already in flight; reconcile the existing operation"));
                        continue;
                    }
                    request.timing.emit("queue_wait", "ok", request.run.as_deref());
                    if request.server_pid != connected_state.child_pid {
                        pending.insert(request.id.clone(), request);
                        broken = true;
                        continue;
                    }
                    let mut send = request.timing.phase("send");
                    let frame = json!({"type":request.operation,"id":request.id,"payload":request.payload});
                    request.sent_at = Some(Instant::now());
                    pending.insert(request.id.clone(), request);
                    broken = ws.send(Message::Text(frame.to_string().into())).await.is_err();
                    if !broken { send.ok(); }
                },
                message = ws.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            let Ok(frame) = serde_json::from_str::<Value>(&text) else { broken = true; continue; };
                            let Some(id) = frame["id"].as_str().map(str::to_owned) else { continue; };
                            let terminal = frame["type"] != "event" && frame["type"] != "run.attached";
                            if let Some(request) = pending.get_mut(&id) {
                                if !request.first_response {
                                    request.timing.emit("first_response", "ok", request.run.as_deref());
                                    request.first_response = true;
                                }
                                if let Some(run) = frame["event"]["runId"].as_str() {
                                    if request.run.is_none() { request.timing.emit("acknowledgement", "ok", Some(run)); }
                                    request.run = Some(run.to_owned()); request.guard.take();
                                }
                                if terminal {
                                    let outcome = if frame["type"] == "error" || frame["type"] == "run.interrupted" { "error" } else { "ok" };
                                    request.timing.emit("terminal", outcome, request.run.as_deref());
                                    if outcome == "ok" { request.timing.ok(); }
                                }
                                if let Some(seq) = frame["event"]["seq"].as_u64() { request.cursor = seq; }
                                let _ = request.events.send(bound_frame(frame));
                            }
                            if terminal { pending.remove(&id); }
                        },
                        Some(Ok(Message::Ping(bytes))) => { broken = ws.send(Message::Pong(bytes)).await.is_err(); },
                        Some(Ok(Message::Pong(_))) => {},
                        _ => { broken = true; },
                    }
                }
            }
        }
        pending.retain(|id, request| {
            request.timing.emit("disconnect", "error", request.run.as_deref());
            if let Some(record) = request.timing.record.as_mut() { record["attempt"] = json!(uuid::Uuid::new_v4().to_string()); }
            if request.sent_at.is_some() { request.guard.take(); }
            if request.sent_at.is_none() || request.run.is_some() || read_request(&request.operation) { true } else {
                let _ = request.events.send(failure(id, "delivery is uncertain; reconcile persisted run/submission state before dispatch"));
                false
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn call(root: PathBuf, operation: &str, payload: Value) -> Result<Value> {
    call_with_progress(root, operation, payload, &|_| {}).await
}

pub async fn call_with_progress(
    root: PathBuf,
    operation: &str,
    payload: Value,
    progress: &(impl Fn(Value) + Sync),
) -> Result<Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut events = request(root, id, operation.to_owned(), payload).await?;
    while let Some(frame) = events.recv().await {
        if frame["type"] == "managed.progress" {
            progress(frame["payload"].clone());
            continue;
        }
        if frame["type"] == "error" {
            return Err(anyhow!("{}", frame["error"]));
        }
        if frame.get("payload").is_some() {
            return Ok(frame["payload"]
                .get("data")
                .unwrap_or(&frame["payload"])
                .clone());
        }
    }
    Err(anyhow!("Swamp request ended without a result"))
}

pub async fn cancel(root: PathBuf, run: String) -> Result<Value> {
    uuid::Uuid::parse_str(&run).context("cancellation requires a run UUID")?;
    tokio::task::spawn_blocking(move || {
        let state = endpoint(&root)?.context("managed Swamp is not running")?;
        http(&state, &format!("/api/v1/cancel/workflow-run/{run}"), true)
    })
    .await
    .context("Swamp cancellation task failed")?
}

pub async fn lifecycle(root: PathBuf, action: &str) -> Result<Value> {
    let root = root
        .canonicalize()
        .context("cannot resolve control checkout")?;
    if action == "status" {
        return Ok(match endpoint(&root)? {
            Some(state) => {
                let freshness = match extension_fingerprint(&root) {
                    Ok(current) if state.extensions.as_ref() == Some(&current) => "current",
                    Ok(_) => "stale",
                    Err(_) => "unavailable",
                };
                json!({"status":if alive(&state) {"running"} else {"exited"},"pid":state.pid,"checkout":root,"extensions":freshness,"bind":state.bind,"port":state.public_metadata()["port"],"dashboardUrl":state.public_metadata()["dashboardUrl"]})
            },
            None => json!({"status":"stopped","checkout":root}),
        });
    }
    if !matches!(action, "stop" | "restart") {
        return Err(anyhow!("unknown managed Swamp lifecycle action"));
    }
    let locked_root = root.clone();
    let guard = tokio::task::spawn_blocking(move || lock(&locked_root))
        .await
        .context("Swamp lifecycle lock failed")??;
    if let Some(state) = endpoint(&root)? {
        if alive(&state) && running(&root).await? {
            return Err(anyhow!("Swamp has active executions; stop/restart refused"));
        }
        stop_locked(&root, &state).await?;
    }
    if action == "restart" {
        tokio::task::spawn_blocking(move || ensure_locked(&root))
            .await
            .context("Swamp restart task failed")??;
    }
    drop(guard);
    Ok(json!({"status":if action == "restart" {"running"} else {"stopped"}}))
}

#[cfg(test)]
#[path = "unit/swamp_refresh_tests.rs"]
mod refresh_tests;

#[cfg(test)]
mod tests {
    // tungstenite requires a full HTTP response as the handshake error.
    use super::*;

    #[test]
    fn service_token_mints_only_when_absent_and_rotates_existing_tokens() {
        for state in [None, Some("active"), Some("expired")] {
            let mut calls = Vec::new();
            let token = service_token(Path::new("/control"), |operation, args| {
                calls.push(operation);
                match operation {
                    "token list" => {
                        assert_eq!(args, ["access", "token", "list"]);
                        let mut listing = json!([{"name":"unrelated","state":"active"}]);
                        if let Some(state) = state {
                            listing
                                .as_array_mut()
                                .unwrap()
                                .push(json!({"name":SERVICE,"state":state}));
                        }
                        Ok(listing)
                    },
                    "token mint" => {
                        assert_eq!(
                            args,
                            [
                                "access",
                                "token",
                                "mint",
                                SERVICE,
                                "--principal",
                                "user:zellij-service",
                                "--duration",
                                "7d",
                            ]
                        );
                        Ok(json!({}))
                    },
                    "token rotate" => {
                        assert_eq!(
                            args,
                            ["access", "token", "rotate", SERVICE, "--duration", "7d"]
                        );
                        Ok(json!({}))
                    },
                    "token reveal" => {
                        assert_eq!(args, ["access", "token", "reveal", SERVICE, "--yes"]);
                        Ok(json!({"token":"test-credential"}))
                    },
                    _ => panic!("unexpected operation"),
                }
            })
            .unwrap();
            assert_eq!(token, "test-credential");
            assert_eq!(
                calls,
                [
                    "token list",
                    if state.is_some() {
                        "token rotate"
                    } else {
                        "token mint"
                    },
                    "token reveal"
                ]
            );
        }
    }

    #[test]
    fn service_token_rejects_revoked_tokens_and_malformed_listings_before_mutating() {
        for listing in [
            json!([{"name":SERVICE,"state":"revoked"}]),
            json!(null),
            json!({}),
            json!({"results":[]}),
            json!([null]),
            json!([{"state":"active"}]),
            json!([{"name":"","state":"active"}]),
            json!([{"name":42,"state":"active"}]),
            json!([{"name":SERVICE}]),
            json!([{"name":SERVICE,"state":"unknown"}]),
            json!([{"name":SERVICE,"state":"active"},{"name":"other","state":null}]),
        ] {
            let mut calls = Vec::new();
            let error = service_token(Path::new("/control"), |operation, _| {
                calls.push(operation);
                Ok(listing.clone())
            })
            .unwrap_err();
            assert_eq!(calls, ["token list"]);
            let message = format!("{error:#}");
            assert!(message.contains("token list") && message.contains("/control"));
            assert!(message.contains(if listing[0]["state"] == "revoked" {
                "revoked"
            } else {
                "invalid token listing"
            }));
        }
    }

    #[test]
    fn service_token_stops_at_each_command_failure_without_fallback_minting() {
        for existing in [false, true] {
            let operations = [
                "token list",
                if existing {
                    "token rotate"
                } else {
                    "token mint"
                },
                "token reveal",
            ];
            for failed in 0..operations.len() {
                let mut calls = Vec::new();
                let error = service_token(Path::new("/control"), |operation, _| {
                    calls.push(operation);
                    if operation == operations[failed] {
                        return Err(anyhow!("injected command failure"));
                    }
                    Ok(if operation == "token list" && existing {
                        json!([{"name":SERVICE,"state":"active"}])
                    } else {
                        json!([])
                    })
                })
                .unwrap_err();
                assert_eq!(calls, operations[..=failed]);
                assert_eq!(error.to_string(), "injected command failure");
            }
        }
    }

    #[test]
    fn service_token_rejects_missing_or_empty_revealed_credentials() {
        for revealed in [
            json!({}),
            json!({"token":""}),
            json!({"token":null}),
            json!({"token":42}),
        ] {
            let mut calls = Vec::new();
            let error = service_token(Path::new("/control"), |operation, _| {
                calls.push(operation);
                Ok(if operation == "token reveal" {
                    revealed.clone()
                } else {
                    json!([])
                })
            })
            .unwrap_err();
            assert_eq!(calls, ["token list", "token mint", "token reveal"]);
            let message = format!("{error:#}");
            assert!(message.contains("token reveal") && message.contains("/control"));
            assert!(message.contains("missing service credential"));
        }
    }

    #[test]
    fn bootstrap_response_preserves_context_without_leaking_secrets() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{ExitStatus, Output};

        let root = Path::new("/control checkout");
        let output = |status, stdout: &[u8]| {
            Ok(Output {
                status: ExitStatus::from_raw(status),
                stdout: stdout.to_vec(),
                stderr: b"stderr-secret-sentinel".to_vec(),
            })
        };
        assert_eq!(
            bootstrap_response(
                root,
                "token reveal",
                output(0, br#"{"token":"test-credential"}"#)
            )
            .unwrap(),
            json!({"token":"test-credential"})
        );
        for (response, expected) in [
            (
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "underlying-secret-sentinel",
                )),
                "cannot execute command (PermissionDenied)",
            ),
            (
                output(7 << 8, b"stdout-secret-sentinel"),
                "command failed (exit status: 7)",
            ),
            (
                output(0, b"stdout-secret-sentinel"),
                "invalid JSON response (exit status: 0)",
            ),
        ] {
            let error = bootstrap_response(root, "token reveal", response).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("token reveal") && message.contains("/control checkout"));
            assert!(message.contains(expected), "{message}");
            for cause in error.chain() {
                assert!(!format!("{cause:?}").contains("secret-sentinel"));
            }
            assert!(!message.contains("secret-sentinel"));
        }
    }

    #[test]
    fn timing_records_are_opt_in_and_correlated_on_success_and_failure() {
        struct Logger(Mutex<Vec<String>>);
        impl log::Log for Logger {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                self.0.lock().unwrap().push(record.args().to_string());
            }
            fn flush(&self) {}
        }
        static LOGGER: Logger = Logger(Mutex::new(Vec::new()));
        log::set_logger(&LOGGER).unwrap();
        log::set_max_level(log::LevelFilter::Info);
        drop(Timing::new("total", "id", "data.query", false));
        assert!(LOGGER.0.lock().unwrap().is_empty());
        let mut timing = Timing::new("total", "id", "data.query", true);
        let mut send = timing.phase("send");
        send.ok();
        drop(send);
        timing.emit("terminal", "error", Some("run"));
        timing.ok();
        drop(timing);
        let records: Vec<Value> = LOGGER
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|line| serde_json::from_str(line.strip_prefix("swamp_timing ").unwrap()).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["outcome"], "ok");
        assert_eq!(records[1]["outcome"], "error");
        assert_eq!(records[1]["run"], "run");
        for record in &records {
            assert_eq!(record["requestId"], "id");
            assert_eq!(record["attempt"], records[0]["attempt"]);
            assert!(record["elapsedMs"].as_f64().unwrap() >= 0.0);
            assert!(record.as_object().unwrap().keys().all(|key| [
                "layer",
                "phase",
                "requestId",
                "method",
                "attempt",
                "outcome",
                "elapsedMs",
                "run"
            ]
            .contains(&key.as_str())));
        }
    }

    #[tokio::test]
    async fn capture_is_private_bounded_and_disabled_without_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stdout.log");
        drain_capture(&b"discarded"[..], None).await;
        assert!(!path.exists());
        let mut capture = Capture::open(path.clone()).unwrap();
        capture.write(&vec![b'a'; CAPTURE_LIMIT + 7]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"aaaaaaa");
        assert_eq!(
            std::fs::metadata(path.with_extension("log.1"))
                .unwrap()
                .len(),
            CAPTURE_LIMIT as u64
        );
        capture.write(&vec![b'b'; CAPTURE_LIMIT]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"bbbbbbb");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let link = dir.path().join("stderr.log");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Capture::open(link).is_err());
    }

    async fn fixture() -> (tempfile::TempDir, PathBuf, tokio::net::TcpListener) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pid = std::process::id();
        let started = process_started(pid).unwrap();
        publish(&Endpoint {
            root: root.clone(),
            pid,
            started,
            child_pid: pid,
            child_started: started,
            url: format!("ws://{}", listener.local_addr().unwrap()),
            bind: default_bind(),
            token: "test.secret".into(),
            extensions: Some(extension_fingerprint(&root).unwrap()),
        })
        .unwrap();
        (dir, root, listener)
    }

    async fn next(events: &mut mpsc::UnboundedReceiver<Value>) -> Value {
        loop {
            let frame = next_frame(events).await;
            if frame["type"] != "managed.progress" {
                return frame;
            }
        }
    }

    async fn next_frame(events: &mut mpsc::UnboundedReceiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[test]
    fn extension_generation_tracks_contents_helpers_and_membership_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sources = root.join("extensions/models/_lib");
        std::fs::create_dir_all(&sources).unwrap();
        let helper = sources.join("helper.ts");
        std::fs::write(&helper, "first").unwrap();
        let original = extension_fingerprint(root).unwrap();
        std::fs::write(root.join("README.md"), "unrelated").unwrap();
        std::fs::create_dir_all(root.join(".swamp/bundles")).unwrap();
        std::fs::write(root.join(".swamp/bundles/generated.js"), "cache").unwrap();
        assert_eq!(extension_fingerprint(root).unwrap(), original);
        let modified = std::fs::metadata(&helper).unwrap().modified().unwrap();
        std::fs::write(&helper, "other").unwrap();
        File::options()
            .write(true)
            .open(&helper)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_ne!(
            extension_fingerprint(root).unwrap(),
            original,
            "same size and timestamp still detects changed bytes"
        );
        std::fs::write(&helper, "first").unwrap();
        assert_eq!(extension_fingerprint(root).unwrap(), original);
        std::fs::rename(&helper, sources.join("renamed.ts")).unwrap();
        assert_ne!(extension_fingerprint(root).unwrap(), original);
        std::fs::remove_file(sources.join("renamed.ts")).unwrap();
        assert_ne!(extension_fingerprint(root).unwrap(), original);
        std::fs::write(&helper, "first").unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("model.ts"), "one").unwrap();
        std::os::unix::fs::symlink(external.path(), root.join("extensions/external")).unwrap();
        let linked = extension_fingerprint(root).unwrap();
        std::fs::write(external.path().join("model.ts"), "two").unwrap();
        assert_ne!(extension_fingerprint(root).unwrap(), linked);
        std::os::unix::fs::symlink(root.join("extensions"), root.join("extensions/cycle")).unwrap();
        assert!(extension_fingerprint(root)
            .unwrap_err()
            .to_string()
            .contains("cycle"));
    }

    #[test]
    fn configured_extension_directories_track_all_kinds_and_environment_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = dir.path();
        let marker = format!("modelsDir: custom/models\nvaultsDir: 'custom/vaults'\ndatastoresDir: \"{}\" # absolute\nreportsDir: custom/reports\n", external.path().display());
        std::fs::write(root.join(".swamp.yaml"), marker).unwrap();
        let paths = extension_directories(root, |_| None).unwrap();
        assert_eq!(
            paths,
            vec![
                root.join("custom/models"),
                root.join("custom/vaults"),
                external.path().to_owned(),
                root.join("custom/reports")
            ]
        );
        assert_eq!(
            extension_directories(root, |_| Some(String::new())).unwrap(),
            paths
        );
        let overridden = extension_directories(root, |_| Some("override".into())).unwrap();
        assert!(overridden.iter().all(|path| path == &root.join("override")));
        assert!(
            extension_directories(root, |_| Some(external.path().to_str().unwrap().into()))
                .unwrap()
                .iter()
                .all(|path| path == external.path())
        );
        for path in paths {
            std::fs::create_dir_all(&path).unwrap();
            let before = extension_fingerprint(root).unwrap();
            let file = path.join("helper.ts");
            std::fs::write(&file, "one").unwrap();
            let added = extension_fingerprint(root).unwrap();
            assert_ne!(before, added);
            std::fs::write(&file, "two").unwrap();
            assert_ne!(added, extension_fingerprint(root).unwrap());
            std::fs::remove_file(file).unwrap();
            assert_eq!(before, extension_fingerprint(root).unwrap());
        }
    }

    #[test]
    fn extension_directory_settings_accept_yaml_strings_aliases_and_flow_maps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for marker in [
            "modelsDir: &path 'custom/models'\nvaultsDir: *path\nreportsDir: >-\n  custom/reports\ndatastoresDir: \"null\"\n",
            "{modelsDir: custom/models, vaultsDir: custom/models, reportsDir: custom/reports, datastoresDir: 'null'}",
        ] {
            std::fs::write(root.join(".swamp.yaml"), marker).unwrap();
            assert_eq!(extension_directories(root, |_| None).unwrap(), vec![
                root.join("custom/models"), root.join("custom/models"), root.join("null"), root.join("custom/reports")
            ]);
        }
        std::fs::write(root.join(".swamp.yaml"), "modelsDir: null\nvaultsDir: ''\n").unwrap();
        assert_eq!(
            extension_directories(root, |_| None).unwrap()[0],
            root.join("extensions/models")
        );
        assert_eq!(
            extension_directories(root, |_| None).unwrap()[1],
            root.join("extensions/vaults")
        );
        std::fs::write(root.join(".swamp.yaml"), "modelsDir: [").unwrap();
        assert!(extension_directories(root, |_| None).is_err());
    }

    #[test]
    fn extension_checker_skips_deleted_and_orphaned_sources_but_checks_live_imports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let deno = root.join("deno");
        zellij_test_support::write_warm_script(
            &deno,
            r#"#!/bin/sh
[ "$1" = bundle ] || exit 0
for source do :; done
[ "$*" = "bundle --no-lock --node-modules-dir=none --external npm:zod@4 --external npm:zod --external npm:zod@4.3.6 --platform deno $source" ] || exit 1
! grep -q 'broken import' "$source"
"#,
        );
        let live = root.join("source.ts");
        let valid = r#"import { z } from "npm:zod@4.3.6";
import "npm:zod@4";
import "npm:zod";
import "npm:lodash-es@4.17.21";
export const schema = z.string();"#;
        std::fs::write(&live, valid).unwrap();
        let mut report = json!({"denoPath":deno,"aggregateState":{"sourceDetails":[
            {"sourcePath":"deleted.ts","stateTag":"Indexed"},
            {"sourcePath":"source.ts","stateTag":"Indexed"},
            {"sourcePath":"retained.js","stateTag":"OrphanedBundleOnly"},
            {"sourcePath":"tombstone.ts","stateTag":"Tombstoned"}
        ]}});
        std::fs::write(root.join("retained.js"), "retained bundle").unwrap();
        std::fs::write(root.join("tombstone.ts"), "obsolete source").unwrap();
        check_extension_sources(root, &report).unwrap();
        std::fs::rename(&live, root.join("renamed.ts")).unwrap();
        check_extension_sources(root, &report).unwrap();
        report["aggregateState"]["sourceDetails"][1]["sourcePath"] = json!("renamed.ts");
        std::fs::write(root.join("renamed.ts"), valid.replace('"', "'")).unwrap();
        check_extension_sources(root, &report).unwrap();
        std::fs::write(
            root.join("renamed.ts"),
            format!("{valid}\n// broken import"),
        )
        .unwrap();
        assert!(check_extension_sources(root, &report).is_err());
        report["aggregateState"]["sourceDetails"][1]["sourcePath"] = json!(root.join("renamed.ts"));
        assert!(check_extension_sources(root, &report).is_err());
    }

    #[tokio::test]
    async fn stale_admission_waits_without_blocking_callbacks_or_consuming_a_dispatch_deadline() {
        let (_dir, root, listener) = fixture().await;
        let mut state = endpoint(&root).unwrap().unwrap();
        state.extensions = None; // an endpoint written by an older Zellij
        publish(&state).unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone()); Ok(response)
                    }).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = ws.next().await {
                        let request: Value = serde_json::from_str(&text).unwrap();
                        let payload = if request["type"] == "run.history" {
                            json!({"runs":[{"status":"running"}]})
                        } else {
                            json!({"passed":true})
                        };
                        if ws.send(Message::Text(json!({"id":request["id"],"type":request["type"],"payload":payload}).to_string().into())).await.is_err() { break; }
                    }
                });
            }
        });
        let mut queued = request(
            root.clone(),
            "queued".into(),
            "workflow.validate".into(),
            json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            next_frame(&mut queued).await["payload"]["status"],
            "waiting"
        );
        let mut callback = request(
            root.clone(),
            "callback".into(),
            "model.method.run".into(),
            json!({"_zellij_active_callback":true}),
        )
        .await
        .unwrap();
        assert_eq!(next(&mut callback).await["payload"]["passed"], true);
        let mut history = request(
            root.clone(),
            "history".into(),
            "run.history".into(),
            json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            next(&mut history).await["payload"]["runs"][0]["status"],
            "running"
        );
        assert!(queued.try_recv().is_err());
        // Model an independently completed refresh. Waiting requests must re-read
        // endpoint state rather than restart again using their earlier observation.
        let path = root.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = lock(&path).unwrap();
            state.extensions = Some(extension_fingerprint(&path).unwrap());
            publish(&state).unwrap();
        })
        .await
        .unwrap();
        assert_eq!(next_frame(&mut queued).await["payload"]["status"], "ready");
        assert_eq!(next(&mut queued).await["payload"]["passed"], true);
        server.abort();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn startup_lock_wait_returns_progress_and_preserves_read_access_before_one_dispatch() {
        let (_dir, root, listener) = fixture().await;
        let guard = lock(&root).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone()); Ok(response)
            }).await.unwrap();
            for operation in ["run.history", "workflow.run"] {
                let request: Value =
                    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(request["type"], operation);
                ws.send(Message::Text(
                    json!({"id":request["id"],"type":operation,"payload":{"passed":true}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            }
        });
        let mut queued = request(
            root.clone(),
            "queued".into(),
            "workflow.run".into(),
            json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            next_frame(&mut queued).await["payload"]["status"],
            "waiting"
        );
        assert!(
            queued.try_recv().is_err(),
            "prewarm cannot complete while the lock is held"
        );
        // This includes the activity check used by admission while it owns the lock.
        assert_eq!(
            call(root.clone(), "run.history", json!({})).await.unwrap()["passed"],
            true
        );
        drop(guard);
        assert_eq!(next_frame(&mut queued).await["payload"]["status"], "ready");
        assert_eq!(next_frame(&mut queued).await["payload"]["passed"], true);
        assert!(queued.recv().await.is_none());
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn multiplexes_responses_and_preserves_structured_errors() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let first: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            let second: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            ws.send(Message::Text(json!({"id":second["id"],"type":"error","error":{"code":"denied","message":"no access"}}).to_string().into())).await.unwrap();
            ws.send(Message::Text(
                json!({"id":first["id"],"type":"data.query","payload":{"data":{"answer":42}}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        });
        let mut first = request(
            root.clone(),
            "first".into(),
            "data.query".into(),
            json!({"predicate":"true"}),
        )
        .await
        .unwrap();
        let mut second = request(
            root.clone(),
            "second".into(),
            "data.query".into(),
            json!({"predicate":"false"}),
        )
        .await
        .unwrap();
        assert_eq!(next(&mut first).await["payload"]["data"]["answer"], 42);
        assert_eq!(next(&mut second).await["error"]["code"], "denied");
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn reconnect_attaches_with_cursor_without_replaying_mutation() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut first = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let frame: Value =
                serde_json::from_str(first.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(frame["type"], "workflow.run");
            first.send(Message::Text(json!({"id":"mutation","type":"event","event":{"kind":"started","runId":"run","seq":1}}).to_string().into())).await.unwrap();
            first.close(None).await.unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut second = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let frame: Value =
                serde_json::from_str(second.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(frame["type"], "run.attach");
            assert_eq!(frame["payload"], json!({"runId":"run","afterSeq":1}));
            second
                .send(Message::Text(
                    json!({"id":"mutation","type":"event","event":{"kind":"completed","seq":2}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            second
                .send(Message::Text(
                    json!({"id":"mutation","type":"done"}).to_string().into(),
                ))
                .await
                .unwrap();
        });
        let mut events = request(
            root.clone(),
            "mutation".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"workflow"}),
        )
        .await
        .unwrap();
        assert_eq!(next(&mut events).await["event"]["seq"], 1);
        assert_eq!(next(&mut events).await["event"]["seq"], 2);
        assert_eq!(next(&mut events).await["type"], "done");
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }
    #[tokio::test]
    async fn lost_acknowledgement_replays_reads_but_never_mutations() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                    Ok(response)
                }).await.unwrap();
                if attempt == 0 {
                    for _ in 0..2 {
                        ws.next().await.unwrap().unwrap();
                    }
                    ws.close(None).await.unwrap();
                } else {
                    let frame: Value =
                        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                            .unwrap();
                    assert_eq!(frame["type"], "data.query");
                    assert_eq!(frame["id"], "read");
                    ws.send(Message::Text(
                        json!({"id":"read","type":"data.query","payload":{"data":{"answer":42}}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
            }
        });
        let mut mutation = request(
            root.clone(),
            "unknown".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"workflow"}),
        )
        .await
        .unwrap();
        let mut read = request(
            root.clone(),
            "read".into(),
            "data.query".into(),
            json!({"predicate":"true"}),
        )
        .await
        .unwrap();
        assert_eq!(
            next(&mut mutation).await["error"]["code"],
            "transport_unknown"
        );
        assert!(mutation.recv().await.is_none());
        assert_eq!(next(&mut read).await["payload"]["data"]["answer"], 42);
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[test]
    fn announcements_validate_bind_and_normalize_actual_ports() {
        for (bind, local) in [
            ("127.0.0.1", "127.0.0.1"),
            ("0.0.0.0", "127.0.0.1"),
            ("192.0.2.1", "192.0.2.1"),
            ("::", "[::1]"),
            ("::1", "[::1]"),
            ("2001:db8::1", "[2001:db8::1]"),
        ] {
            let ip: IpAddr = bind.parse().unwrap();
            let scheme = if ip.is_loopback() { "ws" } else { "wss" };
            let announcement = json!({"host":bind,"port":43127,"url":format!("{scheme}://{bind}:43127"),"detachRuns":true,"schedulingEnabled":false});
            let url = announced_url(&announcement, ip).unwrap();
            assert_eq!(url, format!("{scheme}://{local}:43127"));
            assert!(valid_url(&url));
            if !ip.is_loopback() {
                let mut insecure = announcement.clone();
                insecure["url"] = json!(format!("ws://{bind}:43127"));
                assert!(announced_url(&insecure, ip).is_err());
            }
            assert!(announced_url(&announcement, "192.0.2.2".parse().unwrap()).is_err());
            for (key, invalid) in [
                ("port", json!(null)),
                ("port", json!(-1)),
                ("host", json!(null)),
                ("port", json!(0)),
                ("port", json!(65536)),
                ("url", json!("ws://evil.example:43127")),
                ("host", json!("localhost")),
                ("detachRuns", json!(false)),
                ("schedulingEnabled", json!(true)),
            ] {
                let mut bad = announcement.clone();
                bad[key] = invalid;
                assert!(announced_url(&bad, ip).is_err(), "{bad}");
            }
        }
    }

    #[tokio::test]
    async fn public_endpoint_reports_allocated_port_without_credentials() {
        let (_dir, root, listener) = fixture().await;
        let metadata = public_endpoint(&root).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            metadata,
            json!({"bind":"127.0.0.1","port":port,"dashboardUrl":format!("http://127.0.0.1:{port}/dashboard")})
        );
        let status = lifecycle(root.clone(), "status").await.unwrap();
        for key in ["bind", "port", "dashboardUrl"] {
            assert_eq!(status[key], metadata[key]);
        }
        assert!(!status.to_string().contains("test.secret"));
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[test]
    fn operations_and_previews_are_bounded() {
        assert!(valid_url("ws://127.0.0.1:12345"));
        for invalid in [
            "ws://127.0.0.1:0",
            "ws://127.0.0.1:123@evil.example",
            "ws://localhost:123",
            "ws://127.0.0.1:123/other",
        ] {
            assert!(!valid_url(invalid));
        }
        assert!(allowed("workflow.run"));
        assert!(!allowed("vault.read"));
        for operation in [
            "workflow.create",
            "workflow.edit",
            "workflow.delete",
            "workflow.evaluate",
            "workflow.resume",
            "workflow.approve",
            "workflow.reject",
        ] {
            assert!(allowed(operation));
            assert!(
                !read_request(operation),
                "{operation} must never replay after a lost acknowledgement"
            );
        }
        for operation in [
            "workflow.get",
            "workflow.search",
            "workflow.schema",
            "workflow.approvals",
            "report.get",
            "report.search",
            "model.get",
            "model.search",
            "model.type.search",
            "model.type.describe",
            "model.method.describe",
        ] {
            assert!(allowed(operation));
            assert!(read_request(operation));
        }
        for operation in [
            "vault.get",
            "extension.pull",
            "workflow.trigger.set",
            "model.delete",
            "serve.stop",
        ] {
            assert!(!allowed(operation));
        }
        let frame = bound_frame(
            json!({"id":"history","type":"workflow.history.get","payload":{"data":{"inputs":{"state":{"test":{"output":"large","log":"exact-version"}}}}}}),
        );
        assert!(frame
            .pointer("/payload/data/inputs/state/test/output")
            .is_none());
        assert_eq!(
            frame.pointer("/payload/data/inputs/state/test/log"),
            Some(&json!("exact-version"))
        );
        assert_eq!(
            bound_frame(
                json!({"id":"large","type":"event","event":{"output":"x".repeat(MAX_OUTPUT)}})
            )["type"],
            "error"
        );
    }
}
