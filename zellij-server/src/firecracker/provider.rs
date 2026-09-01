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
    ffi::{CString, OsStr},
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::validate_firecracker_vm_name;
use zellij_utils::input::config::Config as ZellijConfig;

const CONFIG_ENV: &str = "ZELLIJ_FIRECRACKER_CONFIG";
const TEST_NETWORK_ENV: &str = "ZELLIJ_FIRECRACKER_NETWORK_CONFIG";
const DEFAULT_SUBNET: &str = "172.31.0.0/24";
const DEFAULT_TAPS: u8 = 16;
const DEFAULT_SETUP_IMAGE: &str = "zellij-dev";
const DEFAULT_SETUP_DISK_GIB: u64 = 64;
const INSTALLED_PROVIDER: &str = "/usr/local/bin/zellij-firecracker";
const GITIGNORE_FILTER: &str = ":- .gitignore";
const RECOVERY_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
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
        image: Option<String>,
        worktree: Option<PathBuf>,
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
    Sync {
        version: u32,
        session: String,
        name: String,
    },
}

impl Request {
    fn version(&self) -> u32 {
        match self {
            Self::Ensure { version, .. }
            | Self::Stop { version, .. }
            | Self::StopSession { version, .. }
            | Self::List { version, .. }
            | Self::Sync { version, .. } => *version,
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
    #[serde(default)]
    pub cgroup: Option<PathBuf>,
    #[serde(default)]
    pub gid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Config {
    firecracker: PathBuf,
    kernel: PathBuf,
    rootfs: PathBuf,
    #[serde(default = "default_images_dir")]
    images_dir: PathBuf,
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
    #[serde(default)]
    credential_files: Vec<CredentialFile>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct CredentialFile {
    host: PathBuf,
    guest: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct SetupOptions {
    project: PathBuf,
    image: String,
    zellij: Option<PathBuf>,
    disk_size_gib: u64,
    rebuild: bool,
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
fn default_images_dir() -> PathBuf {
    data_dir().join("images")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VmState {
    ready: bool,
    session: String,
    #[serde(default)]
    leases: Vec<String>,
    name: String,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    worktree: Option<PathBuf>,
    #[serde(default)]
    host_fingerprint: Option<u64>,
    pid: i32,
    start_time: u64,
    slot: u8,
    guest_ip: Ipv4Addr,
    rootfs: PathBuf,
    api_socket: PathBuf,
    log: PathBuf,
    known_hosts: PathBuf,
    #[serde(default)]
    cgroup: Option<PathBuf>,
}

struct LockedState {
    dir: PathBuf,
    _lock: File,
}

impl LockedState {
    fn new() -> Result<Self> {
        let dir = state_dir();
        secure_dir(&dir)?;
        prune_recoveries(&dir)?;
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

    fn identity_path(&self, session: &str, name: &str, worktree: Option<&Path>) -> PathBuf {
        match worktree {
            Some(worktree) => self.dir.join(format!(
                "{}.json",
                vm_id("worktree", &worktree.to_string_lossy())
            )),
            None => self.path(session, name),
        }
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
        [command, rest @ ..] if command == "setup" => setup(rest),
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
        [group, command, rest @ ..] if group == "network" && command == "remove" => {
            network_remove_cli(rest)
        },
        _ => Err(anyhow!(
            "usage: zellij-firecracker [setup | doctor | image build | network setup|apply|remove|teardown]"
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
    recover_unusable_vms(&state, &config)?;
    let response = match request {
        Request::Ensure {
            session,
            name,
            image,
            worktree,
            ..
        } => serde_json::to_value(ensure(
            &state,
            &config,
            &network,
            &session,
            &name,
            image.as_deref(),
            worktree.as_deref(),
        )?)?,
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
            "vms": list(&state, session.as_deref())?,
        }),
        Request::Sync { session, name, .. } => {
            sync_one(&state, &config, &session, &name)?;
            json!({"version": PROTOCOL_VERSION})
        },
    };
    serde_json::to_writer(std::io::stdout(), &response).context("failed to write response")
}

fn ensure(
    locked: &LockedState,
    config: &Config,
    network: &NetworkConfig,
    session: &str,
    name: &str,
    image: Option<&str>,
    worktree: Option<&Path>,
) -> Result<EnsureResponse> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    if let Some(image) = image {
        validate_firecracker_vm_name(image).map_err(|error| anyhow!(error))?;
    }
    let worktree = worktree
        .map(|path| {
            path.canonicalize()
                .with_context(|| format!("failed to resolve worktree {}", path.display()))
        })
        .transpose()?;
    if worktree.as_ref().is_some_and(|path| !path.is_dir()) {
        return Err(anyhow!("Firecracker worktree must be a directory"));
    }
    let host_fingerprint = worktree.as_deref().map(tree_fingerprint).transpose()?;
    let state_path = locked.identity_path(session, name, worktree.as_deref());
    if let Ok(mut existing) = read_state(&state_path) {
        if vm_is_running(&existing, config) {
            if !existing.ready {
                terminate(&existing, config);
                retain_recovery(locked, &state_path, &existing, "unready")?;
                let _ = fs::remove_file(&state_path);
            } else {
                if existing.image.as_deref() != image {
                    return Err(anyhow!(
                        "worktree VM is already running with image {:?}, not {:?}",
                        existing.image,
                        image
                    ));
                }
                if !existing.leases.iter().any(|lease| lease == session) {
                    existing.leases.push(session.to_owned());
                    write_state(&state_path, &existing)?;
                }
                return Ok(response(config, &existing));
            }
        } else {
            retain_recovery(locked, &state_path, &existing, "crashed")?;
            let _ = fs::remove_file(&state_path);
        }
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

    let id = state_path
        .file_stem()
        .and_then(|id| id.to_str())
        .context("invalid VM state path")?
        .to_owned();
    let vm_dir = locked.dir.join(&id);
    secure_dir(&vm_dir)?;
    let rootfs = vm_dir.join("rootfs.ext4");
    run(
        Command::new("cp")
            .args(["--reflink=auto", "--sparse=always"])
            .arg(image_path(config, image)?.as_path())
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
    let mut firecracker = Command::new("unshare");
    firecracker
        .args(["--user", "--map-current-user", "--mount", "--"])
        .arg(&config.firecracker)
        .arg("--api-sock")
        .arg(&api_socket)
        .arg("--config-file")
        .arg(&vm_config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .process_group(0);
    unsafe {
        firecracker.pre_exec(|| {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = libc::rlimit {
                rlim_cur: 1024,
                rlim_max: 1024,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = firecracker.spawn().context("failed to start Firecracker")?;
    let pid = child.id() as i32;
    let start_time = match wait_for_process_start(pid, Duration::from_secs(2)) {
        Ok(start_time) => start_time,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_vm_paths(&rootfs, &api_socket);
            return Err(error);
        },
    };
    let cgroup = match network.cgroup.as_deref() {
        Some(parent) => {
            match place_in_cgroup(parent, &id, pid, config.memory_mib, config.vcpu_count) {
                Ok(cgroup) => Some(cgroup),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    cleanup_vm_paths(&rootfs, &api_socket);
                    return Err(error);
                },
            }
        },
        None => None,
    };
    let mut vm = VmState {
        ready: false,
        session: session.to_owned(),
        leases: vec![session.to_owned()],
        name: name.to_owned(),
        image: image.map(str::to_owned),
        worktree: worktree.clone(),
        host_fingerprint,
        pid,
        start_time,
        slot,
        guest_ip,
        rootfs,
        api_socket,
        log,
        known_hosts,
        cgroup,
    };
    if let Err(error) = write_state(&state_path, &vm) {
        terminate(&vm, config);
        cleanup_files(&vm);
        return Err(error);
    }
    if let Err(error) = wait_for_ssh(&mut child, &vm, config.boot_timeout_secs) {
        let log = fs::read_to_string(&vm.log).unwrap_or_else(|read_error| {
            format!("<failed to read {}: {read_error}>", vm.log.display())
        });
        terminate(&vm, config);
        cleanup_files(&vm);
        let _ = fs::remove_file(&state_path);
        return Err(error).context(format!("Firecracker log:\n{log}"));
    }
    if let Err(error) = provision_guest(&vm, config) {
        terminate(&vm, config);
        retain_recovery(locked, &state_path, &vm, "provision-failed")?;
        let _ = fs::remove_file(&state_path);
        return Err(error);
    }
    vm.ready = true;
    if let Err(error) = write_state(&state_path, &vm) {
        terminate(&vm, config);
        retain_recovery(locked, &state_path, &vm, "state-commit-failed")?;
        let _ = fs::remove_file(&state_path);
        return Err(error);
    }
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

fn image_path(config: &Config, image: Option<&str>) -> Result<PathBuf> {
    let path = match image {
        Some(image) => config.images_dir.join(format!("{image}.ext4")),
        None => config.rootfs.clone(),
    };
    path.is_file().then_some(path.clone()).with_context(|| {
        format!(
            "Firecracker image {} does not exist; build it explicitly",
            path.display()
        )
    })
}

fn provision_guest(state: &VmState, config: &Config) -> Result<()> {
    if let Some(worktree) = state.worktree.as_deref() {
        sync_to_guest(state, config, worktree)?;
    }
    for credential in &config.credential_files {
        copy_credential(state, config, credential)?;
    }
    Ok(())
}

fn sync_to_guest(state: &VmState, config: &Config, worktree: &Path) -> Result<()> {
    let staging = tempfile::tempdir().context("failed to stage the Firecracker worktree")?;
    let staged = staging.path().join("workspace");
    if worktree.join(".git").exists() {
        run(
            Command::new("git")
                .args(["clone", "--no-hardlinks", "--quiet", "--"])
                .arg(worktree)
                .arg(&staged),
            "failed to clone the worktree for the guest",
        )?;
    } else {
        fs::create_dir(&staged)?;
    }
    stage_filtered_worktree(worktree, &staged)?;
    run(
        filtered_rsync()
            .args(["-a", "--delete", "-e", &rsync_remote_shell(state, config)])
            .arg(with_trailing_slash(&staged))
            .arg(format!("root@{}:/workspace/", state.guest_ip)),
        "failed to copy the worktree into the guest",
    )
}

fn sync_from_guest(state: &VmState, config: &Config) -> Result<()> {
    let Some(worktree) = state.worktree.as_deref() else {
        return Ok(());
    };
    let expected = state
        .host_fingerprint
        .context("VM has no host worktree baseline")?;
    ensure_host_unchanged(worktree, expected)?;
    let staging = tempfile::tempdir().context("failed to stage guest changes")?;
    run(
        filtered_rsync()
            .args([
                "-a",
                "--delay-updates",
                "--delete-delay",
                "-e",
                &rsync_remote_shell(state, config),
            ])
            .arg(format!("root@{}:/workspace/", state.guest_ip))
            .arg(with_trailing_slash(staging.path())),
        "failed to copy changes out of the guest",
    )?;
    ensure_host_unchanged(worktree, expected)?;
    apply_filtered_worktree(staging.path(), worktree)
}

fn stage_filtered_worktree(source: &Path, destination: &Path) -> Result<()> {
    run(
        filtered_rsync()
            .args(["-a", "--delete", "--exclude=.git"])
            .arg(with_trailing_slash(source))
            .arg(with_trailing_slash(destination)),
        "failed to stage the filtered worktree",
    )
}

fn apply_filtered_worktree(source: &Path, destination: &Path) -> Result<()> {
    run(
        filtered_rsync()
            .args(["-a", "--delay-updates", "--delete-delay", "--exclude=.git"])
            .arg(with_trailing_slash(source))
            .arg(with_trailing_slash(destination)),
        "failed to apply guest changes to the host worktree",
    )
}

fn filtered_rsync() -> Command {
    let mut command = Command::new("rsync");
    command.arg(format!("--filter={GITIGNORE_FILTER}"));
    command
}

fn ensure_host_unchanged(worktree: &Path, expected: u64) -> Result<()> {
    (tree_fingerprint(worktree)? == expected)
        .then_some(())
        .with_context(|| {
            format!(
                "refusing to overwrite host changes in {}; the guest disk was retained for recovery",
                worktree.display()
            )
        })
}

fn copy_credential(state: &VmState, config: &Config, credential: &CredentialFile) -> Result<()> {
    let host = expand_home(&credential.host)?;
    if !host.is_file() {
        return Err(anyhow!("credential file {} does not exist", host.display()));
    }
    validate_guest_path(&credential.guest)?;
    let parent = credential
        .guest
        .parent()
        .context("credential guest path has no parent")?;
    run(
        ssh_command(state, config)
            .arg("mkdir")
            .arg("-p")
            .arg("--")
            .arg(parent),
        "failed to create the guest credential directory",
    )?;
    run(
        Command::new("rsync")
            .args([
                "-a",
                "--chmod=F600",
                "-e",
                &rsync_remote_shell(state, config),
            ])
            .arg(&host)
            .arg(format!(
                "root@{}:{}",
                state.guest_ip,
                credential.guest.display()
            )),
        "failed to copy a configured credential into the guest",
    )
}

fn ssh_command(state: &VmState, config: &Config) -> Command {
    let mut command = Command::new("ssh");
    command
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
        ])
        .arg(format!(
            "UserKnownHostsFile={}",
            state.known_hosts.display()
        ))
        .arg("-i")
        .arg(&config.ssh_private_key)
        .arg(format!("root@{}", state.guest_ip));
    command
}

fn rsync_remote_shell(state: &VmState, config: &Config) -> String {
    format!(
        "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile={} -i {}",
        shell_quote(&state.known_hosts),
        shell_quote(&config.ssh_private_key)
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn with_trailing_slash(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}/", path.display()))
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME is unavailable for credential expansion")?;
    expand_home_from(path, Path::new(&home))
}

fn validate_guest_path(path: &Path) -> Result<()> {
    let valid = path.is_absolute()
        && path
            .as_os_str()
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(byte))
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    valid
        .then_some(())
        .with_context(|| format!("unsafe credential guest path {}", path.display()))
}

fn tree_fingerprint(root: &Path) -> Result<u64> {
    let staging = tempfile::tempdir().context("failed to stage the worktree fingerprint")?;
    stage_filtered_worktree(root, staging.path())?;

    fn visit(root: &Path, path: &Path, hasher: &mut DefaultHasher) -> Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if path == root && entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            path.strip_prefix(root)?.hash(hasher);
            let metadata = fs::symlink_metadata(&path)?;
            metadata.file_type().is_dir().hash(hasher);
            metadata.file_type().is_symlink().hash(hasher);
            metadata.len().hash(hasher);
            if !metadata.is_dir() {
                metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .hash(hasher);
            }
            if metadata.file_type().is_symlink() {
                fs::read_link(&path)?.hash(hasher);
            } else if metadata.is_dir() {
                visit(root, &path, hasher)?;
            } else if metadata.is_file() {
                let mut file = File::open(&path)?;
                let mut buffer = [0; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    buffer[..read].hash(hasher);
                }
            }
        }
        Ok(())
    }
    let mut hasher = DefaultHasher::new();
    visit(staging.path(), staging.path(), &mut hasher)?;
    Ok(hasher.finish())
}

fn list(locked: &LockedState, session: Option<&str>) -> Result<Vec<VmEntry>> {
    let mut vms = vec![];
    for (_, state) in locked.entries()? {
        let leases = if state.leases.is_empty() {
            vec![state.session.clone()]
        } else {
            state.leases.clone()
        };
        for lease in leases
            .into_iter()
            .filter(|lease| session.is_none_or(|wanted| wanted == lease))
        {
            vms.push(VmEntry {
                session: lease,
                name: state.name.clone(),
                state: "running".to_owned(),
            });
        }
    }
    vms.sort_by(|a, b| (&a.session, &a.name).cmp(&(&b.session, &b.name)));
    Ok(vms)
}

fn recover_unusable_vms(locked: &LockedState, config: &Config) -> Result<()> {
    for (path, state) in locked.entries()? {
        if state.ready && vm_is_running(&state, config) {
            continue;
        }
        terminate(&state, config);
        retain_recovery(
            locked,
            &path,
            &state,
            if state.ready { "crashed" } else { "unready" },
        )?;
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn stop_one(locked: &LockedState, config: &Config, session: &str, name: &str) -> Result<()> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let Some((path, state)) = locked.entries()?.into_iter().find(|(_, state)| {
        state.name == name
            && (state.leases.iter().any(|lease| lease == session)
                || (state.leases.is_empty() && state.session == session))
    }) else {
        return Ok(());
    };
    release(locked, config, path, state, session)
}

fn stop_session(locked: &LockedState, config: &Config, session: &str) -> Result<()> {
    let mut first_error = None;
    for (path, state) in locked.entries()? {
        if state.session == session || state.leases.iter().any(|lease| lease == session) {
            if let Err(error) = release(locked, config, path, state, session) {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn release(
    locked: &LockedState,
    config: &Config,
    path: PathBuf,
    mut state: VmState,
    session: &str,
) -> Result<()> {
    state.leases.retain(|lease| lease != session);
    if !state.leases.is_empty() {
        return write_state(&path, &state);
    }
    if let Err(error) = sync_from_guest(&state, config) {
        terminate(&state, config);
        retain_recovery(locked, &path, &state, "sync-conflict")?;
        let _ = fs::remove_file(path);
        return Err(error);
    }
    terminate(&state, config);
    cleanup_files(&state);
    let _ = fs::remove_file(path);
    Ok(())
}

fn sync_one(locked: &LockedState, config: &Config, session: &str, name: &str) -> Result<()> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let Some((path, mut state)) = locked.entries()?.into_iter().find(|(_, state)| {
        state.name == name
            && (state.leases.iter().any(|lease| lease == session)
                || (state.leases.is_empty() && state.session == session))
    }) else {
        return Err(anyhow!(
            "Firecracker VM {name:?} is not running for session {session:?}"
        ));
    };
    if !vm_is_running(&state, config) {
        return Err(anyhow!("Firecracker VM {name:?} is not running"));
    }
    sync_from_guest(&state, config)?;
    state.host_fingerprint = state
        .worktree
        .as_deref()
        .map(tree_fingerprint)
        .transpose()?;
    write_state(&path, &state)
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
    cleanup_vm_paths(&state.rootfs, &state.api_socket);
    cleanup_recovery_resources(state);
}

fn cleanup_recovery_resources(state: &VmState) {
    let _ = fs::remove_file(&state.api_socket);
    if let Some(cgroup) = &state.cgroup {
        let _ = fs::remove_dir(cgroup);
    }
}

fn cleanup_vm_paths(rootfs: &Path, api_socket: &Path) {
    let _ = fs::remove_file(api_socket);
    if let Some(dir) = rootfs.parent() {
        let _ = fs::remove_dir_all(dir);
    }
}

fn place_in_cgroup(
    parent: &Path,
    id: &str,
    pid: i32,
    memory_mib: usize,
    vcpu_count: u8,
) -> Result<PathBuf> {
    if !parent.starts_with("/sys/fs/cgroup") || !parent.is_dir() {
        return Err(anyhow!(
            "delegated Firecracker cgroup {} is unavailable; rerun network setup",
            parent.display()
        ));
    }
    let cgroup = parent.join(id);
    fs::create_dir(&cgroup)?;
    let configured = (|| -> Result<()> {
        fs::write(
            cgroup.join("memory.max"),
            ((memory_mib + 512) * 1024 * 1024).to_string(),
        )?;
        fs::write(cgroup.join("pids.max"), "64")?;
        fs::write(
            cgroup.join("cpu.max"),
            format!("{} 100000", 100000 * usize::from(vcpu_count)),
        )?;
        fs::write(cgroup.join("cgroup.procs"), pid.to_string())?;
        Ok(())
    })();
    if let Err(error) = configured {
        let _ = fs::remove_dir(&cgroup);
        return Err(error).context("failed to configure the Firecracker cgroup");
    }
    Ok(cgroup)
}

fn retain_recovery(
    locked: &LockedState,
    state_path: &Path,
    state: &VmState,
    reason: &str,
) -> Result<()> {
    if let Some(vm_dir) = state.rootfs.parent().filter(|dir| dir.exists()) {
        let recovery = locked.dir.join("recovery");
        secure_dir(&recovery)?;
        let id = state_path
            .file_stem()
            .and_then(|id| id.to_str())
            .unwrap_or("vm");
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let destination = recovery.join(format!("{id}-{timestamp}-{reason}"));
        fs::rename(vm_dir, &destination).with_context(|| {
            format!(
                "failed to retain Firecracker disk at {}",
                destination.display()
            )
        })?;
        fs::write(
            destination.join("state.json"),
            serde_json::to_vec_pretty(state)?,
        )?;
    }
    cleanup_recovery_resources(state);
    Ok(())
}

fn prune_recoveries(state: &Path) -> Result<()> {
    let recovery = state.join("recovery");
    secure_dir(&recovery)?;
    let now = SystemTime::now();
    for entry in fs::read_dir(&recovery)? {
        let entry = entry?;
        let age = entry
            .metadata()?
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        if age.is_some_and(|age| age > RECOVERY_RETENTION) {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
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
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error).context("failed to commit VM state");
    }
    Ok(())
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
    if !config.images_dir.is_absolute() {
        return Err(anyhow!("provider config images_dir must be absolute"));
    }
    for credential in &config.credential_files {
        expand_home(&credential.host)?;
        validate_guest_path(&credential.guest)?;
    }
    Ok(config)
}

fn load_network(config: &Config) -> Result<NetworkConfig> {
    if let Some(network) = &config.network {
        validate_network_config(network)?;
        return Ok(network.clone());
    }
    let path = env::var_os(TEST_NETWORK_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| network_config_path(unsafe { libc::geteuid() }));
    let network = serde_json::from_slice(&fs::read(&path).with_context(|| {
        format!(
            "failed to read {}; run 'sudo zellij-firecracker network setup --user $USER'",
            path.display()
        )
    })?)
    .context("invalid Firecracker network config")?;
    validate_network_config(&network)?;
    Ok(network)
}

fn validate_network_config(config: &NetworkConfig) -> Result<()> {
    let valid_name = |name: &str| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    if !valid_name(&config.bridge)
        || !valid_name(&config.tap_prefix)
        || config.bridge.len() > 15
        || config.tap_prefix.len() + 2 > 15
    {
        return Err(anyhow!("invalid Firecracker bridge or TAP interface name"));
    }
    let (base, prefix) = parse_subnet(&config.subnet)?;
    if prefix != 24 || base.octets()[3] != 0 || config.taps == 0 || config.taps > 253 {
        return Err(anyhow!(
            "Firecracker networking requires an aligned /24 and between 1 and 253 TAPs"
        ));
    }
    Ok(())
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

fn setup(args: &[String]) -> Result<()> {
    let options = parse_setup_options(args)?;
    setup_with(&options, &mut RealSetup)
}

trait SetupRuntime {
    fn project(&mut self, project: &Path) -> Result<PathBuf>;
    fn zellij(&mut self, explicit: Option<&Path>) -> Result<PathBuf>;
    fn provider(&mut self) -> Result<(PathBuf, PathBuf)>;
    fn network(&mut self, provider: &Path) -> Result<()>;
    fn image(
        &mut self,
        options: &SetupOptions,
        project: &Path,
        zellij: &Path,
        current_provider: &Path,
    ) -> Result<()>;
    fn config(&mut self, provider: &Path) -> Result<()>;
    fn doctor(&mut self, provider: &Path) -> Result<()>;
}

struct RealSetup;

impl SetupRuntime for RealSetup {
    fn project(&mut self, project: &Path) -> Result<PathBuf> {
        let project = project
            .canonicalize()
            .with_context(|| format!("failed to resolve setup project {}", project.display()))?;
        project
            .is_dir()
            .then_some(project)
            .context("setup project must be a directory")
    }

    fn zellij(&mut self, explicit: Option<&Path>) -> Result<PathBuf> {
        resolve_zellij(explicit)
    }

    fn provider(&mut self) -> Result<(PathBuf, PathBuf)> {
        let current = env::current_exe()
            .context("failed to find the running zellij-firecracker")?
            .canonicalize()
            .context("failed to resolve the running zellij-firecracker")?;
        let provider = ensure_installed_provider(&current)?;
        Ok((current, provider))
    }

    fn network(&mut self, provider: &Path) -> Result<()> {
        let user = current_user()?;
        println!(
            "sudo: configuring Firecracker networking for {user} with {}",
            provider.display()
        );
        run(
            Command::new("sudo")
                .arg(provider)
                .args(["network", "setup", "--user", &user]),
            "failed to configure Firecracker networking",
        )
    }

    fn image(
        &mut self,
        options: &SetupOptions,
        project: &Path,
        zellij: &Path,
        current_provider: &Path,
    ) -> Result<()> {
        setup_image(options, project, zellij, current_provider)
    }

    fn config(&mut self, provider: &Path) -> Result<()> {
        configure_credentials()?;
        let overlay_dir = zellij_utils::home::get_config_overlay_dir(Some(
            env::var_os(zellij_utils::consts::ZELLIJ_CONFIG_DIR_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(zellij_utils::home::xdg_config_dir),
        ))
        .context("could not determine the Zellij config.d directory")?;
        write_zellij_provider_config(&overlay_dir, provider)
    }

    fn doctor(&mut self, provider: &Path) -> Result<()> {
        run(
            Command::new(provider).arg("doctor"),
            "Firecracker doctor failed",
        )
    }
}

fn setup_with(options: &SetupOptions, runtime: &mut impl SetupRuntime) -> Result<()> {
    let project = runtime.project(&options.project)?;
    let zellij = runtime.zellij(options.zellij.as_deref())?;
    let (current_provider, provider) = runtime.provider()?;
    runtime.network(&provider)?;
    runtime.image(options, &project, &zellij, &current_provider)?;
    runtime.config(&provider)?;
    runtime.doctor(&provider)?;
    println!(
        "Firecracker setup is ready. Use image {:?} for this project.",
        options.image
    );
    Ok(())
}

fn setup_image(
    options: &SetupOptions,
    project: &Path,
    zellij: &Path,
    current_provider: &Path,
) -> Result<()> {
    setup_image_with(
        options,
        project,
        zellij,
        current_provider,
        setup_image_is_valid,
        run,
    )
}

fn setup_image_with<V, R>(
    options: &SetupOptions,
    project: &Path,
    zellij: &Path,
    current_provider: &Path,
    mut image_is_valid: V,
    mut execute: R,
) -> Result<()>
where
    V: FnMut(&str) -> bool,
    R: FnMut(&mut Command, &str) -> Result<()>,
{
    if should_build_image(options.rebuild, image_is_valid(&options.image)) {
        let mut command = Command::new(current_provider);
        command
            .args(["image", "build", "--name", &options.image, "--project"])
            .arg(project)
            .arg("--zellij")
            .arg(zellij)
            .args(["--disk-size-gib", &options.disk_size_gib.to_string()]);
        execute(
            &mut command,
            "failed to build the Firecracker development image",
        )?;
    } else {
        println!(
            "Reusing existing Firecracker image {0}; pass --rebuild to recreate it.",
            options.image
        );
    }
    if !image_is_valid(&options.image) {
        return Err(anyhow!(
            "Firecracker image {:?} was not created correctly",
            options.image
        ));
    }
    Ok(())
}

fn parse_setup_options(args: &[String]) -> Result<SetupOptions> {
    let mut options = SetupOptions {
        project: env::current_dir().context("failed to read the current directory")?,
        image: DEFAULT_SETUP_IMAGE.to_owned(),
        zellij: None,
        disk_size_gib: DEFAULT_SETUP_DISK_GIB,
        rebuild: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--project" | "--name" | "--zellij" | "--disk-size-gib" => {
                let option = args[index].as_str();
                let value = args
                    .get(index + 1)
                    .with_context(|| format!("{option} requires a value"))?;
                match option {
                    "--project" => options.project = PathBuf::from(value),
                    "--name" => options.image = value.clone(),
                    "--zellij" => options.zellij = Some(PathBuf::from(value)),
                    "--disk-size-gib" => {
                        options.disk_size_gib = value.parse().context("invalid --disk-size-gib")?;
                    },
                    _ => unreachable!(),
                }
                index += 2;
            },
            "--rebuild" => {
                options.rebuild = true;
                index += 1;
            },
            argument => return Err(anyhow!("unknown setup argument {argument:?}")),
        }
    }
    validate_firecracker_vm_name(&options.image).map_err(|error| anyhow!(error))?;
    if options.disk_size_gib == 0 {
        return Err(anyhow!("--disk-size-gib must be greater than zero"));
    }
    Ok(options)
}

fn resolve_zellij(explicit: Option<&Path>) -> Result<PathBuf> {
    let provider = env::current_exe().context("failed to find the running zellij-firecracker")?;
    resolve_zellij_from(explicit, &provider, env::var_os("PATH").as_deref())
}

fn resolve_zellij_from(
    explicit: Option<&Path>,
    provider: &Path,
    search_path: Option<&OsStr>,
) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        let path = if explicit.components().count() == 1 {
            find_in_path_from(explicit, search_path).unwrap_or_else(|| explicit.to_owned())
        } else {
            explicit.to_owned()
        };
        return path
            .canonicalize()
            .with_context(|| format!("could not find Zellij at {}", explicit.display()));
    }
    if let Some(sibling) = provider
        .parent()
        .map(|parent| parent.join("zellij"))
        .filter(|path| path.is_file())
    {
        return sibling
            .canonicalize()
            .context("failed to resolve the Zellij binary beside the provider");
    }
    find_in_path_from(Path::new("zellij"), search_path)
        .context("could not find Zellij beside zellij-firecracker or on PATH")?
        .canonicalize()
        .context("failed to resolve the Zellij binary on PATH")
}

fn ensure_installed_provider(current: &Path) -> Result<PathBuf> {
    let installed = Path::new(INSTALLED_PROVIDER);
    ensure_installed_provider_with(
        current,
        installed,
        trusted_provider,
        confirm,
        |source, target| {
            println!("sudo: installing the provider at {INSTALLED_PROVIDER}");
            run(
                Command::new("sudo")
                    .args(["install", "-o", "root", "-g", "root", "-m", "0755", "--"])
                    .arg(source)
                    .arg(target),
                "failed to install zellij-firecracker",
            )
        },
    )
}

fn ensure_installed_provider_with<T, C, I>(
    current: &Path,
    installed: &Path,
    mut trusted: T,
    mut ask: C,
    mut install: I,
) -> Result<PathBuf>
where
    T: FnMut(&Path) -> bool,
    C: FnMut(&str) -> Result<bool>,
    I: FnMut(&Path, &Path) -> Result<()>,
{
    if trusted(current) {
        return Ok(current.to_owned());
    }
    if trusted(installed) && files_equal(current, installed)? {
        return Ok(installed.to_owned());
    }
    if !ask(&format!(
        "Install the provider at {} with sudo?",
        installed.display()
    ))? {
        return Err(anyhow!(
            "setup requires a trusted provider at {}",
            installed.display()
        ));
    }
    install(current, installed)?;
    trusted(installed)
        .then(|| installed.to_owned())
        .context("the installed provider is not a trusted root-owned executable")
}

fn trusted_provider(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return false;
    };
    trusted_provider_attributes(
        &path,
        metadata.uid(),
        metadata.permissions().mode(),
        metadata.is_file(),
    )
}

fn trusted_provider_attributes(path: &Path, uid: u32, mode: u32, is_file: bool) -> bool {
    path.starts_with("/usr/") && is_file && uid == 0 && mode & 0o022 == 0 && mode & 0o111 != 0
}

fn files_equal(left: &Path, right: &Path) -> Result<bool> {
    if left.canonicalize().ok() == right.canonicalize().ok() {
        return Ok(true);
    }
    if fs::metadata(left)?.len() != fs::metadata(right)?.len() {
        return Ok(false);
    }
    let mut left = File::open(left)?;
    let mut right = File::open(right)?;
    let mut left_buffer = [0; 64 * 1024];
    let mut right_buffer = [0; 64 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn current_user() -> Result<String> {
    let output = Command::new("id")
        .arg("-un")
        .output()
        .context("failed to determine the current user")?;
    if !output.status.success() {
        return Err(anyhow!("id -un failed"));
    }
    String::from_utf8(output.stdout)
        .context("id -un returned invalid text")
        .map(|user| user.trim().to_owned())
}

fn setup_image_is_valid(image: &str) -> bool {
    load_config()
        .and_then(|config| image_path(&config, Some(image)))
        .is_ok()
}

fn should_build_image(rebuild: bool, valid: bool) -> bool {
    rebuild || !valid
}

fn configure_credentials() -> Result<()> {
    let path = config_dir().join("firecracker.json");
    let home = env::var_os("HOME").context("HOME is unavailable for credential setup")?;
    configure_credentials_at(&path, Path::new(&home), confirm)
}

fn configure_credentials_at<F>(path: &Path, home: &Path, ask: F) -> Result<()>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut config: Config = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("invalid provider config {}", path.display()))?;
    let candidates = [
        (
            CredentialFile {
                host: PathBuf::from("~/.codex/auth.json"),
                guest: PathBuf::from("/root/.codex/auth.json"),
            },
            home.join(".codex/auth.json"),
            "Copy Codex credentials into Firecracker VMs?",
        ),
        (
            CredentialFile {
                host: PathBuf::from("~/.claude/.credentials.json"),
                guest: PathBuf::from("/root/.claude/.credentials.json"),
            },
            home.join(".claude/.credentials.json"),
            "Copy Claude Code credentials into Firecracker VMs?",
        ),
    ];
    let changed = merge_credentials(&mut config, &candidates, home, ask)?;
    if changed {
        fs::write(path, serde_json::to_vec_pretty(&config)?)
            .with_context(|| format!("failed to update {}", path.display()))?;
        println!("Updated credential mappings in {}.", path.display());
    }
    Ok(())
}

fn merge_credentials<F>(
    config: &mut Config,
    candidates: &[(CredentialFile, PathBuf, &str)],
    home: &Path,
    mut ask: F,
) -> Result<bool>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut changed = false;
    for (credential, detected, question) in candidates {
        let duplicate = config.credential_files.iter().any(|existing| {
            existing.guest == credential.guest
                && expand_home_from(&existing.host, home).ok().as_deref()
                    == Some(detected.as_path())
        });
        if detected.is_file() && !duplicate && ask(question)? {
            config.credential_files.push(credential.clone());
            changed = true;
        }
    }
    Ok(changed)
}

fn expand_home_from(path: &Path, home: &Path) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    if let Some(relative) = text.strip_prefix("~/") {
        Ok(home.join(relative))
    } else if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Err(anyhow!(
            "credential host path must be absolute or start with ~/"
        ))
    }
}

fn write_zellij_provider_config(overlay_dir: &Path, provider: &Path) -> Result<()> {
    let provider = serde_json::to_string(&provider.to_string_lossy())?;
    let fragment =
        format!("firecracker_provider {provider}\nfirecracker_provider_timeout_secs 90\n");
    let path = ZellijConfig::write_managed_fragment(overlay_dir, &fragment)
        .map_err(|error| anyhow!(error))?;
    println!("Wrote Firecracker settings to {}.", path.display());
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
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
    for command in [
        "cp",
        "git",
        "iptables",
        "rsync",
        "ssh",
        "ssh-keyscan",
        "unshare",
    ] {
        if find_in_path(Path::new(command)).is_none() {
            return Err(anyhow!("required command {command:?} is not on PATH"));
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
    let image = option_value(args, "--name")?;
    if let Some(image) = image {
        validate_firecracker_vm_name(image).map_err(|error| anyhow!(error))?;
    }
    let project = option_path(args, "--project")?;
    let explicit_customize = option_path(args, "--customize")?;
    if project.is_some() && explicit_customize.is_some() {
        return Err(anyhow!("use only one of --project and --customize"));
    }
    let customize = explicit_customize
        .or_else(|| project.map(|project| project.join(".zellij/firecracker-image.sh")));
    if customize.as_ref().is_some_and(|path| !path.is_file()) {
        return Err(anyhow!(
            "Firecracker customization script does not exist: {}",
            customize.as_deref().unwrap().display()
        ));
    }
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
    let network_service = root.join("etc/systemd/system/zellij-firecracker-network.service");
    fs::write(
        &network_service,
        "[Unit]\nDescription=Configure Zellij Firecracker guest network\nAfter=network-pre.target\nBefore=network.target ssh.service\n\n[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/usr/local/bin/fcnet-setup.sh\n\n[Install]\nWantedBy=multi-user.target\n",
    )?;
    let wants = root.join("etc/systemd/system/multi-user.target.wants");
    fs::create_dir_all(&wants)?;
    std::os::unix::fs::symlink(
        "../zellij-firecracker-network.service",
        wants.join("zellij-firecracker-network.service"),
    )?;
    fs::create_dir_all(root.join("workspace"))?;
    if let Some(customize) = customize {
        let guest_script = root.join("tmp/zellij-firecracker-customize.sh");
        fs::copy(&customize, &guest_script).with_context(|| {
            format!(
                "failed to stage Firecracker customization script {}",
                customize.display()
            )
        })?;
        run(
            Command::new("chroot")
                .arg(&root)
                .args(["/bin/sh", "/tmp/zellij-firecracker-customize.sh"]),
            "Firecracker customization script failed",
        )?;
        fs::remove_file(guest_script)?;
    }
    let rootfs = match image {
        Some(image) => {
            let images = output.join("images");
            secure_dir(&images)?;
            images.join(format!("{image}.ext4"))
        },
        None => output.join("ubuntu-24.04.ext4"),
    };
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
    secure_dir(&config_dir())?;
    let config_path = config_dir().join("firecracker.json");
    let config = if config_path.exists() {
        let mut config: Config = serde_json::from_slice(&fs::read(&config_path)?)?;
        config.firecracker = firecracker;
        config.kernel = output.join("vmlinux-6.1.182");
        config.images_dir = output.join("images");
        config.ssh_private_key = private_key;
        if image.is_none() {
            config.rootfs = rootfs;
        }
        config
    } else {
        Config {
            firecracker,
            kernel: output.join("vmlinux-6.1.182"),
            rootfs,
            images_dir: output.join("images"),
            ssh_private_key: private_key,
            vcpu_count: default_vcpus(),
            memory_mib: default_memory(),
            guest_cwd: default_cwd(),
            boot_timeout_secs: default_boot_timeout(),
            network: None,
            credential_files: vec![],
            extra: serde_json::Map::new(),
        }
    };
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
    let gid = id_value(user, "-g")?;
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
        cgroup: Some(PathBuf::from(format!(
            "/sys/fs/cgroup/zellij-firecracker-{uid}"
        ))),
        gid: Some(gid),
    };
    validate_network_config(&config)?;
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
    let path = option_path(args, "--config")?.unwrap_or_else(|| network_config_path(uid));
    let config: NetworkConfig = serde_json::from_slice(&fs::read(&path)?)?;
    validate_network_config(&config)?;
    network_apply(uid, &config)
}

fn network_remove_cli(args: &[String]) -> Result<()> {
    require_root()?;
    let path = option_path(args, "--config")?.context("network remove requires --config PATH")?;
    let config: NetworkConfig = serde_json::from_slice(&fs::read(&path)?)?;
    validate_network_config(&config)?;
    network_remove(&config);
    Ok(())
}

fn network_apply(uid: u32, config: &NetworkConfig) -> Result<()> {
    let result = network_apply_inner(uid, config);
    if result.is_err() {
        network_remove(config);
    }
    result
}

fn network_apply_inner(uid: u32, config: &NetworkConfig) -> Result<()> {
    if let Some(cgroup) = &config.cgroup {
        delegate_cgroup(cgroup, uid, config.gid.unwrap_or(uid))?;
    }
    let (base, _) = parse_subnet(&config.subnet)?;
    let gateway = ipv4_add(base, 1)?;
    fs::write("/proc/sys/net/ipv4/ip_forward", "1")?;
    if !interface_exists(&config.bridge) {
        run(
            Command::new("ip").args(["link", "add", &config.bridge, "type", "bridge"]),
            "failed to create the Firecracker bridge",
        )?;
    }
    run(
        Command::new("ip").args([
            "addr",
            "replace",
            &format!("{gateway}/24"),
            "dev",
            &config.bridge,
        ]),
        "failed to assign the Firecracker bridge address",
    )?;
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
    // Remove the pre-iptables table left by older installations.
    let _ = Command::new("nft")
        .args(["delete", "table", "ip", &format!("zellij_fc_{uid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    remove_firewall(config);
    for args in firewall_rules(config) {
        run(
            Command::new("iptables").arg("-w").args(&args),
            "failed to configure Firecracker firewall rules",
        )?;
    }
    Ok(())
}

fn firewall_chain(config: &NetworkConfig) -> String {
    format!(
        "ZFC_{}",
        config.bridge.replace('-', "_").to_ascii_uppercase()
    )
}

fn firewall_rules(config: &NetworkConfig) -> Vec<Vec<String>> {
    let chain = firewall_chain(config);
    let mut rules = vec![vec!["-N".to_owned(), chain.clone()]];
    for protocol in ["udp", "tcp"] {
        rules.push(strings(&[
            "-A",
            &chain,
            "-d",
            &config.dns.to_string(),
            "-p",
            protocol,
            "--dport",
            "53",
            "-j",
            "ACCEPT",
        ]));
    }
    for private in [
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.168.0.0/16",
    ] {
        rules.push(strings(&["-A", &chain, "-d", private, "-j", "DROP"]));
    }
    rules.push(strings(&[
        "-A",
        &chain,
        "-p",
        "tcp",
        "-m",
        "multiport",
        "--dports",
        "22,80,443",
        "-j",
        "ACCEPT",
    ]));
    rules.push(strings(&["-A", &chain, "-j", "DROP"]));
    rules.extend([
        strings(&[
            "-I",
            "INPUT",
            "1",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-j",
            "DROP",
        ]),
        strings(&[
            "-I",
            "INPUT",
            "1",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ]),
        strings(&[
            "-I",
            "FORWARD",
            "1",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-j",
            &chain,
        ]),
        strings(&[
            "-I",
            "FORWARD",
            "1",
            "-o",
            &config.bridge,
            "-d",
            &config.subnet,
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ]),
        strings(&[
            "-t",
            "nat",
            "-I",
            "POSTROUTING",
            "1",
            "-s",
            &config.subnet,
            "-j",
            "MASQUERADE",
        ]),
    ]);
    rules
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn remove_firewall(config: &NetworkConfig) {
    let chain = firewall_chain(config);
    for args in [
        strings(&[
            "-t",
            "nat",
            "-D",
            "POSTROUTING",
            "-s",
            &config.subnet,
            "-j",
            "MASQUERADE",
        ]),
        strings(&[
            "-D",
            "FORWARD",
            "-o",
            &config.bridge,
            "-d",
            &config.subnet,
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ]),
        strings(&[
            "-D",
            "FORWARD",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-j",
            &chain,
        ]),
        strings(&[
            "-D",
            "INPUT",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ]),
        strings(&[
            "-D",
            "INPUT",
            "-i",
            &config.bridge,
            "-s",
            &config.subnet,
            "-j",
            "DROP",
        ]),
        strings(&["-F", &chain]),
        strings(&["-X", &chain]),
    ] {
        let _ = Command::new("iptables")
            .arg("-w")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn network_remove(config: &NetworkConfig) {
    remove_firewall(config);
    let _ = Command::new("ip")
        .args(["link", "delete", &config.bridge])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Some(cgroup) = &config.cgroup {
        let _ = fs::remove_dir(cgroup);
    }
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    network_remove(&config);
    fs::remove_file(path)?;
    Ok(())
}

fn delegate_cgroup(path: &Path, uid: u32, gid: u32) -> Result<()> {
    if !path.starts_with("/sys/fs/cgroup") {
        return Err(anyhow!("refusing to create cgroup outside /sys/fs/cgroup"));
    }
    fs::create_dir_all(path)?;
    fs::write(path.join("cgroup.subtree_control"), "+cpu +memory +pids")?;
    chown(path, uid, gid)?;
    for file in ["cgroup.procs", "cgroup.subtree_control"] {
        chown(&path.join(file), uid, gid)?;
    }
    Ok(())
}

fn chown(path: &Path, uid: u32, gid: u32) -> Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    if unsafe { libc::chown(path.as_ptr(), uid, gid) } != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to delegate cgroup");
    }
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
    find_in_path_from(program, env::var_os("PATH").as_deref())
}

fn find_in_path_from(program: &Path, path: Option<&OsStr>) -> Option<PathBuf> {
    path.and_then(|path| {
        env::split_paths(path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SetupStage {
        Provider,
        Network,
        Image,
        Config,
        Doctor,
    }

    struct RecordingSetup {
        stages: Vec<SetupStage>,
        fail: Option<SetupStage>,
    }

    impl RecordingSetup {
        fn new(fail: Option<SetupStage>) -> Self {
            Self {
                stages: vec![],
                fail,
            }
        }

        fn stage(&mut self, stage: SetupStage) -> Result<()> {
            self.stages.push(stage);
            (self.fail != Some(stage))
                .then_some(())
                .context("injected setup failure")
        }
    }

    impl SetupRuntime for RecordingSetup {
        fn project(&mut self, _project: &Path) -> Result<PathBuf> {
            Ok(PathBuf::from("/project"))
        }

        fn zellij(&mut self, _explicit: Option<&Path>) -> Result<PathBuf> {
            Ok(PathBuf::from("/bin/zellij"))
        }

        fn provider(&mut self) -> Result<(PathBuf, PathBuf)> {
            self.stage(SetupStage::Provider)?;
            Ok((
                PathBuf::from("/build/zellij-firecracker"),
                PathBuf::from(INSTALLED_PROVIDER),
            ))
        }

        fn network(&mut self, _provider: &Path) -> Result<()> {
            self.stage(SetupStage::Network)
        }

        fn image(
            &mut self,
            _options: &SetupOptions,
            _project: &Path,
            _zellij: &Path,
            _current_provider: &Path,
        ) -> Result<()> {
            self.stage(SetupStage::Image)
        }

        fn config(&mut self, _provider: &Path) -> Result<()> {
            self.stage(SetupStage::Config)
        }

        fn doctor(&mut self, _provider: &Path) -> Result<()> {
            self.stage(SetupStage::Doctor)
        }
    }

    fn vm_state(rootfs: PathBuf, api_socket: PathBuf, cgroup: Option<PathBuf>) -> VmState {
        VmState {
            ready: false,
            session: "work".to_owned(),
            leases: vec!["work".to_owned()],
            name: "review".to_owned(),
            image: None,
            worktree: None,
            host_fingerprint: None,
            pid: i32::MAX,
            start_time: 0,
            slot: 0,
            guest_ip: Ipv4Addr::new(172, 31, 0, 2),
            rootfs,
            api_socket,
            log: PathBuf::from("firecracker.log"),
            known_hosts: PathBuf::from("known_hosts"),
            cgroup,
        }
    }

    fn provider_config(root: &Path) -> Config {
        Config {
            firecracker: root.join("firecracker"),
            kernel: root.join("kernel"),
            rootfs: root.join("rootfs"),
            images_dir: root.join("images"),
            ssh_private_key: root.join("id_ed25519"),
            vcpu_count: default_vcpus(),
            memory_mib: default_memory(),
            guest_cwd: default_cwd(),
            boot_timeout_secs: default_boot_timeout(),
            network: None,
            credential_files: vec![],
            extra: serde_json::Map::new(),
        }
    }

    fn setup_options(rebuild: bool) -> SetupOptions {
        SetupOptions {
            project: PathBuf::from("project"),
            image: "zellij-dev".to_owned(),
            zellij: None,
            disk_size_gib: 64,
            rebuild,
        }
    }

    #[test]
    fn setup_runs_provider_network_image_config_and_doctor_in_order() {
        let expected = vec![
            SetupStage::Provider,
            SetupStage::Network,
            SetupStage::Image,
            SetupStage::Config,
            SetupStage::Doctor,
        ];
        let mut runtime = RecordingSetup::new(None);
        setup_with(&setup_options(false), &mut runtime).unwrap();
        assert_eq!(runtime.stages, expected);

        for (index, failed) in expected.iter().copied().enumerate() {
            let mut runtime = RecordingSetup::new(Some(failed));
            assert!(setup_with(&setup_options(false), &mut runtime).is_err());
            assert_eq!(runtime.stages, expected[..=index]);
        }
    }

    #[test]
    fn setup_reuses_valid_images_and_rebuilds_with_exact_arguments() {
        let project = Path::new("/workspace/project");
        let zellij = Path::new("/build/zellij");
        let provider = Path::new("/build/zellij-firecracker");
        let mut commands = vec![];
        setup_image_with(
            &setup_options(false),
            project,
            zellij,
            provider,
            |_| true,
            |command, _| {
                commands.push(
                    command
                        .get_args()
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>(),
                );
                Ok(())
            },
        )
        .unwrap();
        assert!(commands.is_empty());

        setup_image_with(
            &setup_options(true),
            project,
            zellij,
            provider,
            |_| true,
            |command, _| {
                assert_eq!(command.get_program(), provider);
                commands.push(
                    command
                        .get_args()
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>(),
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            commands,
            vec![[
                "image",
                "build",
                "--name",
                "zellij-dev",
                "--project",
                "/workspace/project",
                "--zellij",
                "/build/zellij",
                "--disk-size-gib",
                "64",
            ]
            .map(std::ffi::OsString::from)
            .into_iter()
            .collect::<Vec<_>>()]
        );
    }

    #[test]
    fn setup_arguments_have_development_defaults_and_accept_overrides() {
        let defaults = parse_setup_options(&[]).unwrap();
        assert_eq!(defaults.project, env::current_dir().unwrap());
        assert_eq!(defaults.image, "zellij-dev");
        assert_eq!(defaults.disk_size_gib, 64);
        assert!(!defaults.rebuild);

        let args = [
            "--project",
            "/tmp/project",
            "--name",
            "custom",
            "--zellij",
            "/tmp/zellij",
            "--disk-size-gib",
            "32",
            "--rebuild",
        ]
        .map(str::to_owned);
        let options = parse_setup_options(&args).unwrap();
        assert_eq!(options.project, Path::new("/tmp/project"));
        assert_eq!(options.image, "custom");
        assert_eq!(options.zellij.as_deref(), Some(Path::new("/tmp/zellij")));
        assert_eq!(options.disk_size_gib, 32);
        assert!(options.rebuild);
        assert!(parse_setup_options(&["--unknown".to_owned()]).is_err());
    }

    #[test]
    fn setup_reuses_only_valid_images_unless_rebuild_was_requested() {
        assert!(!should_build_image(false, true));
        assert!(should_build_image(false, false));
        assert!(should_build_image(true, true));
    }

    #[test]
    fn only_root_owned_provider_paths_under_usr_are_trusted() {
        assert!(trusted_provider_attributes(
            Path::new("/usr/local/bin/zellij-firecracker"),
            0,
            0o100755,
            true
        ));
        assert!(!trusted_provider_attributes(
            Path::new("/usr/local/bin/zellij-firecracker"),
            1000,
            0o100755,
            true
        ));
        assert!(!trusted_provider_attributes(
            Path::new("/tmp/zellij-firecracker"),
            0,
            0o100755,
            true
        ));
        let root = tempfile::tempdir().unwrap();
        let provider = root.path().join("zellij-firecracker");
        fs::write(&provider, "provider").unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!trusted_provider(&provider));
        assert!(files_equal(&provider, &provider).unwrap());
        assert!(!trusted_provider_attributes(
            Path::new("/usr/local/bin/zellij-firecracker"),
            0,
            0o100644,
            true
        ));
    }

    #[test]
    fn setup_selects_only_a_trusted_current_or_matching_installed_provider() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let installed = root.path().join("installed");
        fs::write(&current, "provider").unwrap();

        let selected = ensure_installed_provider_with(
            &current,
            &installed,
            |path| path == current,
            |_| panic!("a trusted current provider must not prompt"),
            |_, _| panic!("a trusted current provider must not be installed"),
        )
        .unwrap();
        assert_eq!(selected, current);

        fs::write(&installed, "provider").unwrap();
        let selected = ensure_installed_provider_with(
            &current,
            &installed,
            |path| path == installed,
            |_| panic!("a matching installed provider must not prompt"),
            |_, _| panic!("a matching installed provider must not be replaced"),
        )
        .unwrap();
        assert_eq!(selected, installed);

        for case in ["missing", "mismatched", "untrusted"] {
            if case == "missing" {
                fs::remove_file(&installed).unwrap();
            } else {
                fs::write(
                    &installed,
                    if case == "mismatched" {
                        "different"
                    } else {
                        "provider"
                    },
                )
                .unwrap();
            }
            let trusted_installed = case == "mismatched";
            let error = ensure_installed_provider_with(
                &current,
                &installed,
                |path| path == installed && trusted_installed,
                |_| Ok(false),
                |_, _| panic!("a rejected install must not run"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("requires a trusted provider"));
        }
    }

    #[test]
    fn provider_install_prompt_and_command_failures_stop_setup() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let installed = root.path().join("installed");
        fs::write(&current, "provider").unwrap();

        let prompt = ensure_installed_provider_with(
            &current,
            &installed,
            |_| false,
            |_| Err(anyhow!("prompt failed")),
            |_, _| panic!("a failed prompt must not install"),
        )
        .unwrap_err();
        assert!(prompt.to_string().contains("prompt failed"));

        let install = ensure_installed_provider_with(
            &current,
            &installed,
            |_| false,
            |_| Ok(true),
            |_, _| Err(anyhow!("install failed")),
        )
        .unwrap_err();
        assert!(install.to_string().contains("install failed"));
    }

    #[test]
    fn zellij_resolution_prefers_explicit_then_sibling_then_path() {
        let root = tempfile::tempdir().unwrap();
        let provider_dir = root.path().join("provider");
        let explicit_dir = root.path().join("explicit");
        let path_dir = root.path().join("path");
        for directory in [&provider_dir, &explicit_dir, &path_dir] {
            fs::create_dir(directory).unwrap();
        }
        let provider = provider_dir.join("zellij-firecracker");
        let sibling = provider_dir.join("zellij");
        let explicit = explicit_dir.join("zellij");
        let path_zellij = path_dir.join("zellij");
        for binary in [&provider, &sibling, &explicit, &path_zellij] {
            fs::write(binary, "binary").unwrap();
        }
        let path = env::join_paths([&path_dir]).unwrap();

        assert_eq!(
            resolve_zellij_from(Some(&explicit), &provider, Some(&path)).unwrap(),
            explicit.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_zellij_from(None, &provider, Some(&path)).unwrap(),
            sibling.canonicalize().unwrap()
        );
        fs::remove_file(&sibling).unwrap();
        assert_eq!(
            resolve_zellij_from(None, &provider, Some(&path)).unwrap(),
            path_zellij.canonicalize().unwrap()
        );
        fs::remove_file(&path_zellij).unwrap();
        assert!(resolve_zellij_from(None, &provider, Some(&path)).is_err());
    }

    #[test]
    fn managed_provider_config_preserves_other_managed_settings() {
        let root = tempfile::tempdir().unwrap();
        ZellijConfig::write_managed_fragment(root.path(), "mouse_mode false\n").unwrap();
        write_zellij_provider_config(root.path(), Path::new("/usr/local/bin/zellij-firecracker"))
            .unwrap();
        let managed = fs::read_to_string(root.path().join("zellij-managed.kdl")).unwrap();
        assert!(managed.contains("mouse_mode false"));
        assert!(managed.contains("firecracker_provider \"/usr/local/bin/zellij-firecracker\""));
        assert!(managed.contains("firecracker_provider_timeout_secs 90"));
    }

    #[test]
    fn credential_setup_handles_each_prompt_missing_files_and_tilde_deduplication() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let codex = root.path().join("codex.json");
        let claude = root.path().join("claude.json");
        fs::write(&codex, "secret").unwrap();
        fs::write(&claude, "secret").unwrap();
        let mut config = provider_config(root.path());
        config.credential_files.push(CredentialFile {
            host: root.path().join("existing.json"),
            guest: PathBuf::from("/root/existing.json"),
        });
        let candidates = [
            (
                CredentialFile {
                    host: codex.clone(),
                    guest: PathBuf::from("/root/.codex/auth.json"),
                },
                codex,
                "Codex?",
            ),
            (
                CredentialFile {
                    host: claude.clone(),
                    guest: PathBuf::from("/root/.claude/.credentials.json"),
                },
                claude.clone(),
                "Claude?",
            ),
        ];
        assert!(
            merge_credentials(&mut config, &candidates, home, |question| {
                Ok(question == "Codex?")
            })
            .unwrap()
        );
        assert_eq!(config.credential_files.len(), 2);
        assert_eq!(
            config.credential_files[1].guest,
            Path::new("/root/.codex/auth.json")
        );

        fs::remove_file(&claude).unwrap();
        let mut prompts = 0;
        assert!(!merge_credentials(&mut config, &candidates, home, |_| {
            prompts += 1;
            Ok(true)
        })
        .unwrap());
        assert_eq!(prompts, 0, "missing and duplicate files must not prompt");

        let tilde = CredentialFile {
            host: PathBuf::from("~/.codex/auth.json"),
            guest: PathBuf::from("/root/.codex/auth.json"),
        };
        let detected = home.join(".codex/auth.json");
        fs::create_dir_all(detected.parent().unwrap()).unwrap();
        fs::write(&detected, "secret").unwrap();
        config.credential_files.push(tilde.clone());
        assert!(!merge_credentials(
            &mut config,
            &[(tilde, detected, "duplicate?")],
            home,
            |_| panic!("equivalent tilde mappings must not prompt"),
        )
        .unwrap());
    }

    #[test]
    fn credential_prompt_failures_propagate_without_adding_a_mapping() {
        let root = tempfile::tempdir().unwrap();
        let detected = root.path().join("credential.json");
        fs::write(&detected, "secret").unwrap();
        let mut config = provider_config(root.path());
        let candidate = CredentialFile {
            host: detected.clone(),
            guest: PathBuf::from("/root/credential.json"),
        };
        let error = merge_credentials(
            &mut config,
            &[(candidate, detected, "Copy?")],
            root.path(),
            |_| Err(anyhow!("prompt failed")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("prompt failed"));
        assert!(config.credential_files.is_empty());
    }

    #[test]
    fn credential_config_updates_preserve_unknown_json() {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("firecracker.json");
        let credential = root.path().join(".codex/auth.json");
        fs::create_dir_all(credential.parent().unwrap()).unwrap();
        fs::write(&credential, "secret").unwrap();
        let mut config = provider_config(root.path());
        config.extra.insert(
            "future_setting".to_owned(),
            serde_json::json!({"enabled": true}),
        );
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

        configure_credentials_at(&config_path, root.path(), |_| Ok(true)).unwrap();

        let updated: serde_json::Value =
            serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
        assert_eq!(
            updated["future_setting"],
            serde_json::json!({"enabled": true})
        );
        assert_eq!(updated["credential_files"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn vm_ids_are_stable_and_do_not_expose_session_paths() {
        assert_eq!(
            vm_id("work/../../other", "review"),
            vm_id("work/../../other", "review")
        );
        assert_ne!(vm_id("work", "review"), vm_id("other", "review"));
        assert!(!vm_id("work/../../other", "review").contains('/'));
    }

    #[test]
    fn worktree_fingerprint_ignores_root_outputs() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitignore"), "target/\n").unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/output"), "one").unwrap();
        let original = tree_fingerprint(root.path()).unwrap();

        fs::write(root.path().join("target/output"), "two").unwrap();
        assert_eq!(tree_fingerprint(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_fingerprint_ignores_nested_outputs() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/.gitignore"), "generated/\n").unwrap();
        fs::create_dir(root.path().join("nested/generated")).unwrap();
        fs::write(root.path().join("nested/generated/output"), "one").unwrap();
        let original = tree_fingerprint(root.path()).unwrap();

        fs::write(root.path().join("nested/generated/output"), "two").unwrap();
        assert_eq!(tree_fingerprint(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_fingerprint_detects_non_ignored_host_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        let original = tree_fingerprint(root.path()).unwrap();

        fs::write(root.path().join("source.rs"), "two").unwrap();
        assert_ne!(tree_fingerprint(root.path()).unwrap(), original);
        assert!(ensure_host_unchanged(root.path(), original).is_err());
    }

    #[test]
    fn worktree_fingerprint_ignores_git_metadata() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        let without_git = tree_fingerprint(root.path()).unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/HEAD"), "ref: changed").unwrap();
        assert_eq!(tree_fingerprint(root.path()).unwrap(), without_git);
    }

    #[test]
    fn worktree_sync_honors_root_and_nested_ignores_and_protects_host_outputs() {
        let host = tempfile::tempdir().unwrap();
        let staged = tempfile::tempdir().unwrap();
        fs::write(host.path().join(".gitignore"), "target/\n").unwrap();
        fs::write(host.path().join("source.rs"), "host").unwrap();
        fs::create_dir(host.path().join("target")).unwrap();
        fs::write(host.path().join("target/host-output"), "ignored").unwrap();
        fs::create_dir(host.path().join("nested")).unwrap();
        fs::write(host.path().join("nested/.gitignore"), "generated/\n").unwrap();
        fs::create_dir(host.path().join("nested/generated")).unwrap();
        fs::write(
            host.path().join("nested/generated/host-output"),
            "protected",
        )
        .unwrap();

        stage_filtered_worktree(host.path(), staged.path()).unwrap();
        assert_eq!(
            fs::read_to_string(staged.path().join("source.rs")).unwrap(),
            "host"
        );
        assert!(!staged.path().join("target").exists());
        assert!(!staged.path().join("nested/generated").exists());

        fs::write(staged.path().join("source.rs"), "guest").unwrap();
        fs::create_dir(staged.path().join("target")).unwrap();
        fs::write(staged.path().join("target/guest-output"), "ignored").unwrap();
        fs::create_dir(staged.path().join("nested/generated")).unwrap();
        fs::write(
            staged.path().join("nested/generated/host-output"),
            "guest overwrite",
        )
        .unwrap();
        fs::write(
            staged.path().join("nested/generated/guest-output"),
            "guest output",
        )
        .unwrap();
        apply_filtered_worktree(staged.path(), host.path()).unwrap();
        assert_eq!(
            fs::read_to_string(host.path().join("source.rs")).unwrap(),
            "guest"
        );
        assert!(!host.path().join("target/guest-output").exists());
        assert_eq!(
            fs::read_to_string(host.path().join("target/host-output")).unwrap(),
            "ignored"
        );
        assert_eq!(
            fs::read_to_string(host.path().join("nested/generated/host-output")).unwrap(),
            "protected"
        );
        assert!(!host.path().join("nested/generated/guest-output").exists());
    }

    #[test]
    fn readiness_is_required_in_persisted_vm_state() {
        let state = vm_state(
            PathBuf::from("rootfs.ext4"),
            PathBuf::from("firecracker.sock"),
            None,
        );
        let mut serialized = serde_json::to_value(state).unwrap();
        serialized.as_object_mut().unwrap().remove("ready");
        assert!(serde_json::from_value::<VmState>(serialized).is_err());
    }

    #[test]
    fn retaining_a_vm_removes_its_socket_and_cgroup() {
        let root = tempfile::tempdir().unwrap();
        let vm_dir = root.path().join("vm");
        let rootfs = vm_dir.join("rootfs.ext4");
        let api_socket = root.path().join("firecracker.sock");
        let cgroup = root.path().join("cgroup");
        fs::create_dir(&vm_dir).unwrap();
        fs::write(&rootfs, "disk").unwrap();
        fs::write(&api_socket, "socket").unwrap();
        fs::create_dir(&cgroup).unwrap();
        let lock = File::create(root.path().join("lock")).unwrap();
        let locked = LockedState {
            dir: root.path().to_owned(),
            _lock: lock,
        };
        let state = vm_state(rootfs, api_socket.clone(), Some(cgroup.clone()));

        retain_recovery(&locked, &root.path().join("vm.json"), &state, "unready").unwrap();

        assert!(!api_socket.exists());
        assert!(!cgroup.exists());
        let recovered = fs::read_dir(root.path().join("recovery"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(recovered.join("rootfs.ext4").exists());
        assert!(recovered.join("state.json").exists());
    }

    #[test]
    fn provider_security_boundaries_reject_unsafe_credentials_and_protect_private_networks() {
        assert!(validate_guest_path(Path::new("/root/.config/codex/auth.json")).is_ok());
        assert!(validate_guest_path(Path::new("/root/../etc/shadow")).is_err());
        assert!(validate_guest_path(Path::new("/root/key;touch-pwned")).is_err());

        let config = NetworkConfig {
            bridge: "zfc1000".to_owned(),
            tap_prefix: "zft1000-".to_owned(),
            subnet: "172.31.0.0/24".to_owned(),
            taps: 2,
            dns: Ipv4Addr::new(10, 0, 0, 53),
            cgroup: None,
            gid: None,
        };
        let rules = firewall_rules(&config)
            .into_iter()
            .map(|args| args.join(" "))
            .collect::<Vec<_>>();
        let dns = rules
            .iter()
            .position(|rule| rule.contains("-d 10.0.0.53 -p udp --dport 53 -j ACCEPT"))
            .unwrap();
        let private = rules
            .iter()
            .position(|rule| rule.contains("-d 10.0.0.0/8 -j DROP"))
            .unwrap();
        assert!(
            dns < private,
            "the configured DNS exception must precede the private-network drop"
        );
        assert!(rules
            .iter()
            .any(|rule| rule.contains("INPUT 1 -i zfc1000 -s 172.31.0.0/24 -j DROP")));
        assert!(rules.iter().any(|rule| rule.contains(
            "INPUT 1 -i zfc1000 -s 172.31.0.0/24 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT"
        )));
        assert!(rules
            .iter()
            .any(|rule| rule.contains("--dports 22,80,443 -j ACCEPT")));
        assert!(rules
            .iter()
            .any(|rule| rule.contains("-t nat -I POSTROUTING 1 -s 172.31.0.0/24 -j MASQUERADE")));
    }
}
