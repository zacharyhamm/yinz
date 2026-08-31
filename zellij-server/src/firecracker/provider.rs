//! The shipped, single-user Firecracker provider.
//!
//! No daemon: each protocol invocation takes one file lock, reconciles the
//! small state directory, and leaves the Firecracker process running on its
//! own. The privileged part is a one-time pool of user-owned TAP devices.

use super::{EnsureResponse, VmEntry, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::hash_map::DefaultHasher,
    env,
    ffi::CString,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    os::{
        fd::AsRawFd,
        unix::process::CommandExt,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::validate_firecracker_vm_name;

const CONFIG_ENV: &str = "ZELLIJ_FIRECRACKER_CONFIG";
const TEST_NETWORK_ENV: &str = "ZELLIJ_FIRECRACKER_NETWORK_CONFIG";
const DEFAULT_SUBNET: &str = "172.31.0.0/24";
const DEFAULT_TAPS: u8 = 16;
const KERNEL_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/20260826-761f88fbb951-0/x86_64/vmlinux-6.1.182";
const KERNEL_SHA256: &str = "90f6c45a719c76c08f09ba5e80b4a7f1044e33152c92b9e45a22e87c0021bac5";
const ROOTFS_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/20260826-761f88fbb951-0/x86_64/ubuntu-24.04.squashfs";
const ROOTFS_SHA256: &str = "1e424bde9bfaaca5c74fcc2b1c78be6285ae515528ef4acc9e64ff78b4225b48";

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request {
    Ensure {
        version: u32,
        session: String,
        name: String,
    },
    Stop {
        version: u32,
        session: String,
        name: String,
    },
    StopSession {
        version: u32,
        session: String,
    },
    List {
        version: u32,
        session: Option<String>,
    },
}

impl Request {
    fn version(&self) -> u32 {
        match self {
            Self::Ensure { version, .. }
            | Self::Stop { version, .. }
            | Self::StopSession { version, .. }
            | Self::List { version, .. } => *version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkConfig {
    pub bridge: String,
    pub tap_prefix: String,
    pub subnet: String,
    pub taps: u8,
    pub dns: Ipv4Addr,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Config {
    firecracker: PathBuf,
    kernel: PathBuf,
    rootfs: PathBuf,
    ssh_private_key: PathBuf,
    #[serde(default = "default_vcpus")]
    vcpu_count: u8,
    #[serde(default = "default_memory")]
    memory_mib: usize,
    #[serde(default = "default_cwd")]
    guest_cwd: PathBuf,
    #[serde(default = "default_boot_timeout")]
    boot_timeout_secs: u64,
    #[serde(default)]
    network: Option<NetworkConfig>,
}

fn default_vcpus() -> u8 {
    2
}
fn default_memory() -> usize {
    2048
}
fn default_cwd() -> PathBuf {
    PathBuf::from("/workspace")
}
fn default_boot_timeout() -> u64 {
    60
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VmState {
    session: String,
    name: String,
    pid: i32,
    start_time: u64,
    slot: u8,
    guest_ip: Ipv4Addr,
    rootfs: PathBuf,
    api_socket: PathBuf,
    log: PathBuf,
    known_hosts: PathBuf,
}

struct LockedState {
    dir: PathBuf,
    _lock: File,
}

impl LockedState {
    fn new() -> Result<Self> {
        let dir = state_dir();
        secure_dir(&dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("lock"))
            .context("failed to open the Firecracker provider lock")?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to lock Firecracker provider state");
        }
        Ok(Self { dir, _lock: lock })
    }

    fn path(&self, session: &str, name: &str) -> PathBuf {
        self.dir.join(format!("{}.json", vm_id(session, name)))
    }

    fn entries(&self) -> Result<Vec<(PathBuf, VmState)>> {
        let mut entries = vec![];
        for entry in fs::read_dir(&self.dir).context("failed to read Firecracker state")? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path)
                .context("failed to read VM state")
                .and_then(|bytes| serde_json::from_slice(&bytes).context("invalid VM state"))
            {
                Ok(state) => entries.push((path, state)),
                Err(error) => eprintln!(
                    "warning: ignored invalid VM state {}: {error:#}",
                    path.display()
                ),
            }
        }
        Ok(entries)
    }
}

pub fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => serve_request(),
        [command] if command == "doctor" => doctor(),
        [group, command, rest @ ..] if group == "image" && command == "build" => image_build(rest),
        [group, command, rest @ ..] if group == "image" && command == "build-inner" => {
            image_build_inner(rest)
        },
        [group, command, rest @ ..] if group == "network" && command == "setup" => {
            network_setup(rest)
        },
        [group, command, rest @ ..] if group == "network" && command == "teardown" => {
            network_teardown(rest)
        },
        [group, command, rest @ ..] if group == "network" && command == "apply" => {
            network_apply_cli(rest)
        },
        _ => Err(anyhow!(
            "usage: zellij-firecracker [doctor | image build | network setup|teardown]"
        )),
    }
}

fn serve_request() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut input)
        .context("failed to read provider request")?;
    if input.len() > 1024 * 1024 {
        return Err(anyhow!("provider request exceeded 1 MiB"));
    }
    let request: Request = serde_json::from_slice(&input).context("invalid provider request")?;
    if request.version() != PROTOCOL_VERSION {
        return Err(anyhow!(
            "provider protocol mismatch: expected version {PROTOCOL_VERSION}, got {}",
            request.version()
        ));
    }
    let config = load_config()?;
    let network = load_network(&config)?;
    let state = LockedState::new()?;
    let response = match request {
        Request::Ensure { session, name, .. } => {
            serde_json::to_value(ensure(&state, &config, &network, &session, &name)?)?
        },
        Request::Stop { session, name, .. } => {
            stop_one(&state, &config, &session, &name)?;
            json!({"version": PROTOCOL_VERSION})
        },
        Request::StopSession { session, .. } => {
            stop_session(&state, &config, &session)?;
            json!({"version": PROTOCOL_VERSION})
        },
        Request::List { session, .. } => json!({
            "version": PROTOCOL_VERSION,
            "vms": list(&state, &config, session.as_deref())?,
        }),
    };
    serde_json::to_writer(std::io::stdout(), &response).context("failed to write response")
}

fn ensure(
    locked: &LockedState,
    config: &Config,
    network: &NetworkConfig,
    session: &str,
    name: &str,
) -> Result<EnsureResponse> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let state_path = locked.path(session, name);
    if let Ok(existing) = read_state(&state_path) {
        if vm_is_running(&existing, config) {
            return Ok(response(config, &existing));
        }
        cleanup_files(&existing);
        let _ = fs::remove_file(&state_path);
    }

    let entries = locked.entries()?;
    let slot = (0..network.taps)
        .find(|slot| {
            !entries
                .iter()
                .any(|(_, state)| state.slot == *slot && vm_is_running(state, config))
        })
        .context("no free Firecracker TAP devices; increase the network tap pool")?;
    let (network_base, prefix) = parse_subnet(&network.subnet)?;
    if prefix != 24 {
        return Err(anyhow!(
            "the developer provider currently requires a /24 subnet"
        ));
    }
    let guest_ip = ipv4_add(network_base, u32::from(slot) + 2)?;
    let tap = format!("{}{slot:02}", network.tap_prefix);
    if !interface_exists(&tap) {
        return Err(anyhow!(
            "TAP {tap} is missing; run 'sudo zellij-firecracker network setup --user $USER'"
        ));
    }

    let id = vm_id(session, name);
    let vm_dir = locked.dir.join(&id);
    secure_dir(&vm_dir)?;
    let rootfs = vm_dir.join("rootfs.ext4");
    run(
        Command::new("cp")
            .args(["--reflink=auto", "--sparse=always"])
            .arg(&config.rootfs)
            .arg(&rootfs),
        "failed to clone the Firecracker rootfs",
    )?;
    let api_socket = runtime_dir().join(format!("{id}.sock"));
    secure_dir(api_socket.parent().unwrap_or(Path::new("/tmp")))?;
    let log = vm_dir.join("firecracker.log");
    let known_hosts = vm_dir.join("known_hosts");
    let vm_config = vm_dir.join("vm.json");
    let gateway = ipv4_add(network_base, 1)?;
    let mac = format!(
        "06:00:{:02X}:{:02X}:{:02X}:{:02X}",
        guest_ip.octets()[0],
        guest_ip.octets()[1],
        guest_ip.octets()[2],
        guest_ip.octets()[3]
    );
    let firecracker_config = json!({
        "boot-source": {
            "kernel_image_path": config.kernel,
            "boot_args": format!("console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw zellij_gateway={gateway} zellij_dns={}", network.dns),
        },
        "drives": [{
            "drive_id": "rootfs",
            "path_on_host": rootfs,
            "is_root_device": true,
            "is_read_only": false
        }],
        "machine-config": {
            "vcpu_count": config.vcpu_count,
            "mem_size_mib": config.memory_mib,
            "smt": false,
            "track_dirty_pages": false
        },
        "network-interfaces": [{
            "iface_id": "eth0",
            "guest_mac": mac,
            "host_dev_name": tap
        }]
    });
    fs::write(&vm_config, serde_json::to_vec_pretty(&firecracker_config)?)
        .context("failed to write Firecracker VM config")?;
    let log_file = File::create(&log).context("failed to create Firecracker log")?;
    let mut child = Command::new(&config.firecracker)
        .arg("--api-sock")
        .arg(&api_socket)
        .arg("--config-file")
        .arg(&vm_config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .process_group(0)
        .spawn()
        .context("failed to start Firecracker")?;
    let pid = child.id() as i32;
    let start_time = wait_for_process_start(pid, Duration::from_secs(2))?;
    let vm = VmState {
        session: session.to_owned(),
        name: name.to_owned(),
        pid,
        start_time,
        slot,
        guest_ip,
        rootfs,
        api_socket,
        log,
        known_hosts,
    };
    if let Err(error) = wait_for_ssh(&mut child, &vm, config.boot_timeout_secs) {
        let log = fs::read_to_string(&vm.log).unwrap_or_else(|read_error| {
            format!("<failed to read {}: {read_error}>", vm.log.display())
        });
        terminate(&vm, config);
        cleanup_files(&vm);
        return Err(error).context(format!("Firecracker log:\n{log}"));
    }
    write_state(&state_path, &vm)?;
    Ok(response(config, &vm))
}

fn response(config: &Config, state: &VmState) -> EnsureResponse {
    EnsureResponse {
        version: PROTOCOL_VERSION,
        ssh_host: format!("ssh://root@{}:22", state.guest_ip),
        cwd: Some(config.guest_cwd.clone()),
        ssh_identity_file: Some(config.ssh_private_key.clone()),
        ssh_known_hosts_file: Some(state.known_hosts.clone()),
    }
}

fn wait_for_ssh(child: &mut std::process::Child, state: &VmState, timeout_secs: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Some(status) = child.try_wait().context("failed waiting for Firecracker")? {
            return Err(anyhow!(
                "Firecracker exited before SSH was ready ({status}); log: {}",
                state.log.display()
            ));
        }
        if TcpStream::connect_timeout(
            &SocketAddrV4::new(state.guest_ip, 22).into(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            let output = Command::new("ssh-keyscan")
                .args(["-T", "2"])
                .arg(state.guest_ip.to_string())
                .output()
                .context("failed to run ssh-keyscan")?;
            if output.status.success() && !output.stdout.is_empty() {
                fs::write(&state.known_hosts, output.stdout)
                    .context("failed to write the VM known_hosts file")?;
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out after {timeout_secs}s waiting for SSH; Firecracker log: {}",
                state.log.display()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn list(locked: &LockedState, config: &Config, session: Option<&str>) -> Result<Vec<VmEntry>> {
    let mut vms = vec![];
    for (path, state) in locked.entries()? {
        if !vm_is_running(&state, config) {
            cleanup_files(&state);
            let _ = fs::remove_file(path);
            continue;
        }
        if session.is_none_or(|wanted| wanted == state.session) {
            vms.push(VmEntry {
                session: state.session,
                name: state.name,
                state: "running".to_owned(),
            });
        }
    }
    vms.sort_by(|a, b| (&a.session, &a.name).cmp(&(&b.session, &b.name)));
    Ok(vms)
}

fn stop_one(locked: &LockedState, config: &Config, session: &str, name: &str) -> Result<()> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let path = locked.path(session, name);
    let Ok(state) = read_state(&path) else {
        return Ok(());
    };
    terminate(&state, config);
    cleanup_files(&state);
    let _ = fs::remove_file(path);
    Ok(())
}

fn stop_session(locked: &LockedState, config: &Config, session: &str) -> Result<()> {
    for (path, state) in locked.entries()? {
        if state.session == session {
            terminate(&state, config);
            cleanup_files(&state);
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn terminate(state: &VmState, config: &Config) {
    if !vm_is_running(state, config) {
        return;
    }
    unsafe { libc::kill(state.pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_start_time(state.pid).is_some() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if process_start_time(state.pid).is_some() {
        unsafe { libc::kill(state.pid, libc::SIGKILL) };
    }
}

fn cleanup_files(state: &VmState) {
    let _ = fs::remove_file(&state.api_socket);
    if let Some(dir) = state.rootfs.parent() {
        let _ = fs::remove_dir_all(dir);
    }
}

fn vm_is_running(state: &VmState, config: &Config) -> bool {
    process_start_time(state.pid) == Some(state.start_time)
        && proc_exe(state.pid).and_then(|path| path.canonicalize().ok())
            == config.firecracker.canonicalize().ok()
}

fn process_start_time(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn proc_exe(pid: i32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

fn wait_for_process_start(pid: i32, timeout: Duration) -> Result<u64> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(start) = process_start_time(pid) {
            return Ok(start);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("Firecracker process {pid} did not appear in /proc"));
        }
        thread::yield_now();
    }
}

fn read_state(path: &Path) -> Result<VmState> {
    serde_json::from_slice(&fs::read(path)?).context("invalid VM state")
}

fn write_state(path: &Path, state: &VmState) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(tmp, path).context("failed to commit VM state")
}

fn vm_id(session: &str, name: &str) -> String {
    let mut hasher = DefaultHasher::new();
    session.hash(&mut hasher);
    0u8.hash(&mut hasher);
    name.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn load_config() -> Result<Config> {
    let path = env::var_os(CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("firecracker.json"));
    let config: Config = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("invalid provider config {}", path.display()))?;
    for (name, path) in [
        ("firecracker", &config.firecracker),
        ("kernel", &config.kernel),
        ("rootfs", &config.rootfs),
        ("ssh_private_key", &config.ssh_private_key),
    ] {
        if !path.is_absolute() || !path.is_file() {
            return Err(anyhow!(
                "provider config {name} must be an existing absolute file, got {}",
                path.display()
            ));
        }
    }
    Ok(config)
}

fn load_network(config: &Config) -> Result<NetworkConfig> {
    if let Some(network) = &config.network {
        return Ok(network.clone());
    }
    let path = env::var_os(TEST_NETWORK_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| network_config_path(unsafe { libc::geteuid() }));
    serde_json::from_slice(&fs::read(&path).with_context(|| {
        format!(
            "failed to read {}; run 'sudo zellij-firecracker network setup --user $USER'",
            path.display()
        )
    })?)
    .context("invalid Firecracker network config")
}

fn parse_subnet(subnet: &str) -> Result<(Ipv4Addr, u8)> {
    let (address, prefix) = subnet
        .split_once('/')
        .context("network subnet must be CIDR, for example 172.31.0.0/24")?;
    let address = Ipv4Addr::from_str(address).context("invalid network address")?;
    let prefix: u8 = prefix.parse().context("invalid network prefix")?;
    Ok((address, prefix))
}

fn ipv4_add(address: Ipv4Addr, offset: u32) -> Result<Ipv4Addr> {
    u32::from(address)
        .checked_add(offset)
        .map(Ipv4Addr::from)
        .context("Firecracker subnet address overflow")
}

fn secure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure {}", path.display()))
}

fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("zellij")
}

fn data_dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("zellij/firecracker")
}

fn cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("zellij/firecracker")
}

fn state_dir() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("zellij/firecracker/vms")
}

fn runtime_dir() -> PathBuf {
    let fallback = || {
        PathBuf::from(format!("/tmp/zellij-firecracker-{}", unsafe {
            libc::geteuid()
        }))
    };
    let candidate = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|path| path.join("zellij-firecracker"))
        .unwrap_or_else(&fallback);
    // A slash, the 16-character VM id, and ".sock" must also fit in sun_path.
    if candidate.as_os_str().as_bytes().len() + 22 < 108 {
        candidate
    } else {
        fallback()
    }
}

fn network_config_path(uid: u32) -> PathBuf {
    PathBuf::from(format!("/etc/zellij-firecracker/{uid}.json"))
}

fn doctor() -> Result<()> {
    let config = load_config()?;
    let network = load_network(&config)?;
    if !OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
    {
        return Err(anyhow!(
            "/dev/kvm is not readable and writable by this user"
        ));
    }
    for slot in 0..network.taps {
        let tap = format!("{}{slot:02}", network.tap_prefix);
        if !interface_exists(&tap) {
            return Err(anyhow!("network TAP {tap} is missing"));
        }
    }
    let output = Command::new(&config.firecracker)
        .arg("--version")
        .output()
        .context("failed to run Firecracker")?;
    if !output.status.success() {
        return Err(anyhow!("Firecracker --version failed"));
    }
    println!(
        "KVM, Firecracker, image, key and {} TAPs are ready.",
        network.taps
    );
    Ok(())
}

fn image_build(args: &[String]) -> Result<()> {
    if env::var_os("ZELLIJ_FIRECRACKER_IMAGE_INNER").is_none() && unsafe { libc::geteuid() } != 0 {
        let exe = env::current_exe().context("failed to find zellij-firecracker")?;
        let status = Command::new("unshare")
            .args(["--user", "--map-root-user", "--mount", "--"])
            .arg(exe)
            .args(["image", "build-inner"])
            .args(args)
            .env("ZELLIJ_FIRECRACKER_IMAGE_INNER", "1")
            .status()
            .context("failed to enter the image-builder user namespace")?;
        return status
            .success()
            .then_some(())
            .context("the Firecracker image build failed");
    }
    image_build_inner(args)
}

fn image_build_inner(args: &[String]) -> Result<()> {
    let zellij = option_path(args, "--zellij")?.unwrap_or_else(|| PathBuf::from("zellij"));
    let output = option_path(args, "--output")?.unwrap_or_else(data_dir);
    let disk_gib = option_value(args, "--disk-size-gib")?
        .map(|value| value.parse::<u64>().context("invalid --disk-size-gib"))
        .transpose()?
        .unwrap_or(8);
    secure_dir(&output)?;
    secure_dir(&cache_dir())?;
    let kernel = download_verified(KERNEL_URL, KERNEL_SHA256, "vmlinux-6.1.182")?;
    let squashfs = download_verified(ROOTFS_URL, ROOTFS_SHA256, "ubuntu-24.04.squashfs")?;
    let private_key = output.join("id_ed25519");
    if !private_key.exists() {
        run(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&private_key),
            "failed to generate the Firecracker SSH key",
        )?;
    }
    fs::set_permissions(&private_key, fs::Permissions::from_mode(0o600))?;
    let temp = tempfile::tempdir().context("failed to create image staging directory")?;
    let root = temp.path().join("root");
    run(
        Command::new("unsquashfs")
            .args(["-d"])
            .arg(&root)
            .arg(&squashfs),
        "failed to unpack the Firecracker rootfs",
    )?;
    let zellij = if zellij.is_absolute() {
        zellij
    } else {
        find_in_path(&zellij).with_context(|| format!("could not find {}", zellij.display()))?
    };
    fs::copy(&zellij, root.join("usr/local/bin/zellij"))
        .context("failed to install Zellij in the guest")?;
    fs::set_permissions(
        root.join("usr/local/bin/zellij"),
        fs::Permissions::from_mode(0o755),
    )?;
    let ssh_dir = root.join("root/.ssh");
    secure_dir(&ssh_dir)?;
    fs::copy(
        private_key.with_extension("pub"),
        ssh_dir.join("authorized_keys"),
    )?;
    fs::set_permissions(
        ssh_dir.join("authorized_keys"),
        fs::Permissions::from_mode(0o600),
    )?;
    for entry in fs::read_dir(root.join("etc/ssh"))? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ssh_host_"))
        {
            fs::remove_file(path)?;
        }
    }
    let ssh_dropin = root.join("etc/ssh/sshd_config.d/99-zellij-firecracker.conf");
    fs::write(
        ssh_dropin,
        "PermitRootLogin prohibit-password\nPasswordAuthentication no\nAllowStreamLocalForwarding yes\n",
    )?;
    let service_dropin = root.join("etc/systemd/system/ssh.service.d");
    fs::create_dir_all(&service_dropin)?;
    fs::write(
        service_dropin.join("keys.conf"),
        "[Service]\nExecStartPre=\nExecStartPre=/usr/bin/ssh-keygen -A\nExecStartPre=/usr/sbin/sshd -t\n",
    )?;
    fs::write(
        root.join("usr/local/bin/fcnet-setup.sh"),
        r#"#!/bin/bash
set -eu
dev=$(ls /sys/class/net | grep -v '^lo$' | head -1)
hex=$(cat "/sys/class/net/$dev/address" | cut -d: -f3-6 | tr ':' ' ')
read -r a b c d <<< "$hex"
ip=$(printf '%d.%d.%d.%d' "0x$a" "0x$b" "0x$c" "0x$d")
gateway=$(sed -n 's/.*zellij_gateway=\([^ ]*\).*/\1/p' /proc/cmdline)
dns=$(sed -n 's/.*zellij_dns=\([^ ]*\).*/\1/p' /proc/cmdline)
ip addr add "$ip/24" dev "$dev"
ip link set "$dev" up
ip route replace default via "$gateway"
printf 'nameserver %s\n' "$dns" > /etc/resolv.conf
"#,
    )?;
    fs::set_permissions(
        root.join("usr/local/bin/fcnet-setup.sh"),
        fs::Permissions::from_mode(0o755),
    )?;
    fs::create_dir_all(root.join("workspace"))?;
    let rootfs = output.join("ubuntu-24.04.ext4");
    let file = File::create(&rootfs)?;
    file.set_len(disk_gib * 1024 * 1024 * 1024)?;
    run(
        Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(&root)
            .arg(&rootfs),
        "failed to create the Firecracker ext4 image",
    )?;
    fs::copy(kernel, output.join("vmlinux-6.1.182"))?;
    let firecracker =
        find_in_path(Path::new("firecracker")).context("firecracker is not installed")?;
    let config = Config {
        firecracker,
        kernel: output.join("vmlinux-6.1.182"),
        rootfs,
        ssh_private_key: private_key,
        vcpu_count: default_vcpus(),
        memory_mib: default_memory(),
        guest_cwd: default_cwd(),
        boot_timeout_secs: default_boot_timeout(),
        network: None,
    };
    secure_dir(&config_dir())?;
    let config_path = config_dir().join("firecracker.json");
    fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
    println!("Wrote image and key to {}", output.display());
    println!("Wrote provider config to {}", config_path.display());
    Ok(())
}

fn download_verified(url: &str, sha256: &str, name: &str) -> Result<PathBuf> {
    let path = cache_dir().join(name);
    if path.exists() && sha256_of(&path)? == sha256 {
        return Ok(path);
    }
    let temp = path.with_extension("download");
    run(
        Command::new("curl")
            .args(["-fL", "--retry", "3", "-o"])
            .arg(&temp)
            .arg(url),
        "failed to download a Firecracker image artifact",
    )?;
    let actual = sha256_of(&temp)?;
    if actual != sha256 {
        let _ = fs::remove_file(&temp);
        return Err(anyhow!(
            "checksum mismatch for {name}: expected {sha256}, got {actual}"
        ));
    }
    fs::rename(&temp, &path)?;
    Ok(path)
}

fn sha256_of(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(anyhow!("sha256sum failed for {}", path.display()));
    }
    String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .context("sha256sum returned no digest")
}

fn option_value<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    args.get(index + 1)
        .map(|value| Some(value.as_str()))
        .context(format!("{name} requires a value"))
}

fn option_path(args: &[String], name: &str) -> Result<Option<PathBuf>> {
    Ok(option_value(args, name)?.map(PathBuf::from))
}

fn network_setup(args: &[String]) -> Result<()> {
    require_root()?;
    let user = option_value(args, "--user")?.context("network setup requires --user USER")?;
    let uid = id_value(user, "-u")?;
    let subnet = option_value(args, "--subnet")?.unwrap_or(DEFAULT_SUBNET);
    let taps = option_value(args, "--taps")?
        .map(|value| value.parse().context("invalid --taps"))
        .transpose()?
        .unwrap_or(DEFAULT_TAPS);
    let dns = option_value(args, "--dns")?
        .unwrap_or("1.1.1.1")
        .parse()
        .context("invalid --dns")?;
    let config = NetworkConfig {
        bridge: format!("zfc{uid}"),
        tap_prefix: format!("zft{uid}-"),
        subnet: subnet.to_owned(),
        taps,
        dns,
    };
    if config.tap_prefix.len() + 2 > 15 || config.bridge.len() > 15 {
        return Err(anyhow!("UID {uid} is too long for Linux interface names"));
    }
    let (base, prefix) = parse_subnet(&config.subnet)?;
    if prefix != 24 || base.octets()[3] != 0 || taps > 253 {
        return Err(anyhow!(
            "network setup requires an aligned /24 and at most 253 TAPs"
        ));
    }
    fs::create_dir_all("/etc/zellij-firecracker")?;
    fs::write(
        network_config_path(uid),
        serde_json::to_vec_pretty(&config)?,
    )?;
    write_network_unit()?;
    network_apply(uid, &config)?;
    run(
        Command::new("systemctl").args([
            "enable",
            &format!("zellij-firecracker-network@{uid}.service"),
        ]),
        "failed to enable Firecracker networking at boot",
    )?;
    println!("Configured {} TAPs on {} for {user}.", taps, config.bridge);
    Ok(())
}

fn network_apply_cli(args: &[String]) -> Result<()> {
    require_root()?;
    let uid: u32 = option_value(args, "--uid")?
        .context("network apply requires --uid")?
        .parse()?;
    let config: NetworkConfig = serde_json::from_slice(&fs::read(network_config_path(uid))?)?;
    network_apply(uid, &config)
}

fn network_apply(uid: u32, config: &NetworkConfig) -> Result<()> {
    let (base, _) = parse_subnet(&config.subnet)?;
    let gateway = ipv4_add(base, 1)?;
    fs::write("/proc/sys/net/ipv4/ip_forward", "1")?;
    if !interface_exists(&config.bridge) {
        run(
            Command::new("ip").args(["link", "add", &config.bridge, "type", "bridge"]),
            "failed to create the Firecracker bridge",
        )?;
    }
    let _ = Command::new("ip")
        .args([
            "addr",
            "replace",
            &format!("{gateway}/24"),
            "dev",
            &config.bridge,
        ])
        .status();
    run(
        Command::new("ip").args(["link", "set", &config.bridge, "up"]),
        "failed to start the Firecracker bridge",
    )?;
    for slot in 0..config.taps {
        let tap = format!("{}{slot:02}", config.tap_prefix);
        if !interface_exists(&tap) {
            run(
                Command::new("ip").args([
                    "tuntap",
                    "add",
                    "dev",
                    &tap,
                    "mode",
                    "tap",
                    "user",
                    &uid.to_string(),
                ]),
                "failed to create a Firecracker TAP",
            )?;
        }
        run(
            Command::new("ip").args(["link", "set", &tap, "master", &config.bridge]),
            "failed to attach a Firecracker TAP",
        )?;
        run(
            Command::new("ip").args(["link", "set", &tap, "up"]),
            "failed to start a Firecracker TAP",
        )?;
    }
    let table = format!("zellij_fc_{uid}");
    let _ = Command::new("nft")
        .args(["delete", "table", "ip", &table])
        .status();
    let rules = format!(
        "table ip {table} {{\n chain forward {{ type filter hook forward priority filter; policy accept; ip saddr {} ip daddr 169.254.169.254 drop }}\n chain postrouting {{ type nat hook postrouting priority srcnat; ip saddr {} masquerade }}\n}}\n",
        config.subnet, config.subnet
    );
    let mut nft = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .spawn()?;
    nft.stdin
        .take()
        .context("nft stdin unavailable")?
        .write_all(rules.as_bytes())?;
    if !nft.wait()?.success() {
        return Err(anyhow!("failed to configure Firecracker nftables rules"));
    }
    Ok(())
}

fn network_teardown(args: &[String]) -> Result<()> {
    require_root()?;
    let user = option_value(args, "--user")?.context("network teardown requires --user USER")?;
    let uid = id_value(user, "-u")?;
    let path = network_config_path(uid);
    let config: NetworkConfig = serde_json::from_slice(&fs::read(&path)?)?;
    run(
        Command::new("systemctl").args([
            "disable",
            "--now",
            &format!("zellij-firecracker-network@{uid}.service"),
        ]),
        "failed to disable Firecracker networking",
    )?;
    let _ = Command::new("nft")
        .args(["delete", "table", "ip", &format!("zellij_fc_{uid}")])
        .status();
    let _ = Command::new("ip")
        .args(["link", "delete", &config.bridge])
        .status();
    fs::remove_file(path)?;
    Ok(())
}

fn write_network_unit() -> Result<()> {
    let exe = env::current_exe()?.canonicalize()?;
    let metadata = fs::metadata(&exe)?;
    if !exe.starts_with("/usr/")
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(anyhow!(
            "network setup requires a root-owned zellij-firecracker installed under /usr"
        ));
    }
    fs::write(
        "/etc/systemd/system/zellij-firecracker-network@.service",
        format!(
            "[Unit]\nDescription=Zellij Firecracker network for UID %i\nAfter=network-pre.target\nBefore=network.target\n\n[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart={} network apply --uid %i\n\n[Install]\nWantedBy=multi-user.target\n",
            exe.display()
        ),
    )?;
    run(
        Command::new("systemctl").arg("daemon-reload"),
        "failed to reload systemd",
    )
}

fn require_root() -> Result<()> {
    (unsafe { libc::geteuid() } == 0)
        .then_some(())
        .context("this network command must run as root")
}

fn interface_exists(name: &str) -> bool {
    CString::new(name).is_ok_and(|name| unsafe { libc::if_nametoindex(name.as_ptr()) } != 0)
}

fn id_value(user: &str, flag: &str) -> Result<u32> {
    let output = Command::new("id").args([flag, user]).output()?;
    if !output.status.success() {
        return Err(anyhow!("unknown user {user:?}"));
    }
    String::from_utf8(output.stdout)?
        .trim()
        .parse()
        .context("invalid id output")
}

fn run(command: &mut Command, context: &str) -> Result<()> {
    let status = command.status().with_context(|| context.to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{context}: {status}"))
    }
}

fn find_in_path(program: &Path) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_ids_are_stable_and_do_not_expose_session_paths() {
        assert_eq!(
            vm_id("work/../../other", "review"),
            vm_id("work/../../other", "review")
        );
        assert_ne!(vm_id("work", "review"), vm_id("other", "review"));
        assert!(!vm_id("work/../../other", "review").contains('/'));
    }
}
