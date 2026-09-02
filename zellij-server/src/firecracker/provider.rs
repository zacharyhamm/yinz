//! The shipped, single-user Firecracker provider.
//!
//! No daemon: each protocol invocation takes one file lock, reconciles the
//! small state directory, and leaves the Firecracker process running on its
//! own. The privileged part is a one-time pool of user-owned TAP devices.

use super::{EnsureResponse, RecoveryEntry, VmEntry, PROGRESS_PREFIX, PROTOCOL_VERSION};
use crate::pty_host::ensure_secure_dir;
use crate::pty_host::remote::ssh::SshOptions;
use highway::{HighwayHash, Key, PortableHash};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
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
    process::{Child, Command, Stdio},
    str::FromStr,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zellij_utils::consts::{ZELLIJ_CACHE_DIR, ZELLIJ_PROJ_DIR};
use zellij_utils::errors::prelude::*;
use zellij_utils::home::{xdg_config_dir, xdg_data_dir};
use zellij_utils::input::command::validate_firecracker_vm_name;
use zellij_utils::input::config::Config as ZellijConfig;

const CONFIG_ENV: &str = "ZELLIJ_FIRECRACKER_CONFIG";
const TEST_NETWORK_ENV: &str = "ZELLIJ_FIRECRACKER_NETWORK_CONFIG";
const DEFAULT_TAPS: u8 = 16;
const DEFAULT_SETUP_IMAGE: &str = "zellij-dev";
const DEFAULT_SETUP_DISK_GIB: u64 = 64;
const INSTALLED_PROVIDER: &str = "/usr/local/bin/zellij-firecracker";
const GITIGNORE_FILTER: &str = ":- .gitignore";
const RECOVERY_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Beside the disk in a VM's directory: the per-path content hashes of the host worktree as
/// it was last known to match the guest. See `merge_worktrees`.
const BASELINE_FILE: &str = "baseline.json";
/// Scratch space beside the disk for copies in and out of the guest; see `Staging`.
const STAGING_DIR: &str = "staging";
/// Written into a recovery directory beside `state.json`, so `list` can say why a disk was
/// kept without parsing the directory name.
const RECOVERY_NOTE_FILE: &str = "recovery.json";
/// The guest copy of a file both sides changed is written beside the host copy with this suffix.
const GUEST_COPY_SUFFIX: &str = ".guest";
const KERNEL_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/20260826-761f88fbb951-0/x86_64/vmlinux-6.1.182";
const KERNEL_SHA256: &str = "90f6c45a719c76c08f09ba5e80b4a7f1044e33152c92b9e45a22e87c0021bac5";
const ROOTFS_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/20260826-761f88fbb951-0/x86_64/ubuntu-24.04.squashfs";
const ROOTFS_SHA256: &str = "1e424bde9bfaaca5c74fcc2b1c78be6285ae515528ef4acc9e64ff78b4225b48";
/// Where the customization script is staged inside the guest root.
const GUEST_CUSTOMIZE_SCRIPT: &str = "/tmp/zellij-firecracker-customize.sh";
/// apt drops privileges to `_apt` to download, which cannot work when the
/// builder's user namespace maps a single uid. Staged for the chroot only.
const APT_SANDBOX_OVERRIDE: &str = "etc/apt/apt.conf.d/99-zellij-image-build";
/// Keeps maintainer scripts from starting daemons in the chroot.
const POLICY_RC_D: &str = "usr/sbin/policy-rc.d";
/// Host resolvers to copy into the guest for the customization script. The
/// rootfs ships an empty `/etc/resolv.conf`, so without this there is no DNS.
const HOST_RESOLVERS: [&str; 2] = ["/run/systemd/resolve/resolv.conf", "/etc/resolv.conf"];
/// The device files a customization script needs. Bound one by one rather than
/// as a tree: a file bind cannot be unlinked (`EBUSY`), so a stray recursive
/// delete of the staging directory can never reach the host's devices.
const CHROOT_DEVICES: [&str; 6] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

use super::Request;

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
    /// Refuse to start a VM that cannot be given host-side memory, CPU and process
    /// limits. Off by default: a delegated cgroup subtree can only be granted by root,
    /// and a developer host that has none should still be able to run VMs. Turn it on
    /// to make the missing subtree fatal instead; see `place_in_cgroup`.
    #[serde(default = "default_require_resource_limits")]
    require_resource_limits: bool,
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
fn default_require_resource_limits() -> bool {
    false
}
fn default_images_dir() -> PathBuf {
    data_dir().join("images")
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

const LOCK_WAIT_PHASE: &str = "waiting for the provider lock";

/// Take the exclusive lock, reporting the wait as a phase for as long as it lasts. Another
/// provider - a second session's `ensure`, a `zellij firecracker sync` from a shell - may
/// hold the lock for minutes, and the server kills a provider that stays silent.
fn lock_exclusively(lock: &File, progress: impl FnOnce() -> Progress) -> Result<()> {
    let _progress = progress();
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to lock Firecracker provider state");
    }
    Ok(())
}

impl LockedState {
    fn new() -> Result<Self> {
        let dir = state_dir();
        ensure_secure_dir(&dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("lock"))
            .context("failed to open the Firecracker provider lock")?;
        lock_exclusively(&lock, || Progress::start(LOCK_WAIT_PHASE))?;
        {
            let _progress = Progress::start("pruning old recoveries");
            prune_recoveries(&dir)?;
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
            // A file that cannot be read is not skipped: it may describe a running VM, and
            // a request that cannot see it would reuse its TAP or write over its disk. The
            // error names the file so the user can repair or remove it.
            if let Some(state) = read_state_opt(&path)? {
                entries.push((path, state));
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
        [group, command, rest @ ..] if group == "image" && command == "customize-inner" => {
            image_customize_inner(rest)
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
    let request = parse_request(&input)?;
    let config = load_config()?;
    let network = load_network(&config)?;
    let state = LockedState::new()?;
    let response = handle_request(&state, &config, &network, &RsyncOverSsh, request)?;
    serde_json::to_writer(std::io::stdout(), &response).context("failed to write response")
}

fn parse_request(input: &[u8]) -> Result<Request<'static>> {
    let request: Request<'static> =
        serde_json::from_slice(input).context("invalid provider request")?;
    if request.version() != PROTOCOL_VERSION {
        return Err(anyhow!(
            "provider protocol mismatch: expected version {PROTOCOL_VERSION}, got {}",
            request.version()
        ));
    }
    Ok(request)
}

/// One request against the locked state: the dead VMs are retained first, then the request
/// is dispatched. `copy` is how worktrees move between the host and a guest.
fn handle_request(
    state: &LockedState,
    config: &Config,
    network: &NetworkConfig,
    copy: &dyn GuestCopy,
    request: Request,
) -> Result<serde_json::Value> {
    recover_unusable_vms(state)?;
    let response = match request {
        Request::Ensure {
            session,
            name,
            image,
            worktree,
            ..
        } => serde_json::to_value(ensure(
            state,
            config,
            network,
            &session,
            &name,
            image.as_deref(),
            worktree.as_deref(),
        )?)?,
        Request::Stop { session, name, .. } => json!({
            "version": PROTOCOL_VERSION,
            "warnings": stop_one(state, config, copy, &session, &name)?,
        }),
        Request::StopSession { session, .. } => json!({
            "version": PROTOCOL_VERSION,
            "warnings": stop_session(state, config, copy, &session)?,
        }),
        Request::List { session, .. } => json!({
            "version": PROTOCOL_VERSION,
            "vms": list(state, session.as_deref())?,
            "recoveries": list_recoveries(state, session.as_deref())?,
        }),
        Request::Sync { session, name, .. } => json!({
            "version": PROTOCOL_VERSION,
            "warnings": sync_one(state, config, copy, &session, &name)?,
        }),
        Request::RenameSession { from, to, .. } => {
            rename_session(state, &from, &to)?;
            json!({ "version": PROTOCOL_VERSION })
        },
    };
    Ok(response)
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
    let state_path = locked.identity_path(session, name, worktree.as_deref());
    let entries = locked.entries()?;
    ensure_vm_identity_available(&entries, &state_path, session, name)?;
    if let Some(mut existing) = read_state_opt(&state_path)? {
        if vm_is_running(&existing) {
            match existing_vm_disposition(&existing, name, image) {
                Disposition::Join => {
                    if !existing.leases.iter().any(|lease| lease == session) {
                        existing.leases.push(session.to_owned());
                        write_state(&state_path, &existing)?;
                    }
                    return Ok(response(config, &existing));
                },
                Disposition::Resume => {
                    // The lease goes on record before another attempt at provisioning,
                    // so a session that asked for the VM releases it however that ends.
                    if !existing.leases.iter().any(|lease| lease == session) {
                        existing.leases.push(session.to_owned());
                    }
                    write_state(&state_path, &existing)?;
                    if let Some(worktree) = existing.worktree.as_deref() {
                        // The baseline has to describe what the guest is about to get,
                        // and the host may have moved on since the interrupted attempt.
                        let _progress = Progress::start("hashing the worktree");
                        write_baseline(&baseline_path(&existing)?, &worktree_baseline(worktree)?)?;
                    }
                    return finish_provisioning(locked, config, &state_path, existing, None);
                },
                Disposition::Refuse(why) => return Err(anyhow!(why)),
                Disposition::Replace => {
                    terminate(&existing, None);
                    retain_recovery(locked, &state_path, &existing, "unready")?;
                    let _ = fs::remove_file(&state_path);
                },
            }
        } else {
            retain_recovery(locked, &state_path, &existing, "crashed")?;
            let _ = fs::remove_file(&state_path);
        }
    }

    // Whether this user may run the VM under a cgroup of its own is settled here, before
    // the clone and the boot: a host that refuses is told so at once, not after the
    // expensive work has been done and undone.
    let cgroup_root = cgroup_root_or_skip(delegated_cgroup_root(), config.require_resource_limits)?;

    let baseline = {
        let _progress = Progress::start("hashing the worktree");
        worktree.as_deref().map(worktree_baseline).transpose()?
    };

    let slot = (0..network.taps)
        .find(|slot| {
            !entries
                .iter()
                .any(|(_, state)| state.slot == *slot && vm_is_running(state))
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
    // No state file names this directory, or it would have been handled above, so anything
    // here is what an interrupted attempt left behind. It is moved aside rather than
    // written over: a clone into a directory whose disk a VMM may still hold open would
    // truncate that disk under it.
    retain_orphaned_vm_dir(locked, &vm_dir)?;
    ensure_secure_dir(&vm_dir)?;
    if let Some(baseline) = &baseline {
        if let Err(error) = write_baseline(&vm_dir.join(BASELINE_FILE), baseline) {
            let _ = fs::remove_dir_all(&vm_dir);
            return Err(error);
        }
    }
    let rootfs = vm_dir.join("rootfs.ext4");
    {
        let _progress = Progress::start("cloning the rootfs image");
        run(
            Command::new("cp")
                .args(["--reflink=auto", "--sparse=always"])
                .arg(image_path(config, image)?.as_path())
                .arg(&rootfs),
            "failed to clone the Firecracker rootfs",
        )?;
    }
    let api_socket = runtime_dir().join(format!("{id}.sock"));
    ensure_secure_dir(api_socket.parent().unwrap_or(Path::new("/tmp")))?;
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
    let cgroup = match place_in_cgroup(
        &id,
        pid,
        config.memory_mib,
        config.vcpu_count,
        cgroup_root.as_deref(),
    ) {
        Ok(cgroup) => cgroup,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_vm_paths(&rootfs, &api_socket);
            return Err(error);
        },
    };
    let vm = VmState {
        ready: false,
        session: session.to_owned(),
        leases: vec![session.to_owned()],
        name: name.to_owned(),
        image: image.map(str::to_owned),
        worktree: worktree.clone(),
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
        terminate(&vm, Some(&mut child));
        cleanup_files(&vm);
        return Err(error);
    }
    finish_provisioning(locked, config, &state_path, vm, Some(&mut child))
}

/// What to do about a VM already on record for the identity an `ensure` names.
#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    /// Ready and the same VM: add the lease and hand it over.
    Join,
    /// The same VM, but an earlier attempt never finished provisioning it - the provider
    /// was killed part-way, most likely - so it is picked up where that left off. The
    /// copy into the guest is incremental, which is what lets a large worktree converge
    /// over attempts that each run out of time.
    Resume,
    /// Ready, but not the VM that was asked for; a running VM is not replaced.
    Refuse(String),
    /// Unready and not the VM that was asked for: nothing in it is worth keeping.
    Replace,
}

fn existing_vm_disposition(existing: &VmState, name: &str, image: Option<&str>) -> Disposition {
    let mismatch = if existing.name != name {
        Some(format!(
            "worktree VM is already running as {:?}, not {name:?}",
            existing.name
        ))
    } else if existing.image.as_deref() != image {
        Some(format!(
            "worktree VM is already running with image {:?}, not {:?}",
            existing.image, image
        ))
    } else {
        None
    };
    match (existing.ready, mismatch) {
        (true, None) => Disposition::Join,
        (true, Some(why)) => Disposition::Refuse(why),
        (false, None) => Disposition::Resume,
        (false, Some(_)) => Disposition::Replace,
    }
}

/// Wait for the guest and provision it, then mark the VM ready. Shared by a VM started
/// just now, whose `Child` is still in hand, and one resumed from an interrupted attempt.
/// A failure here is a real one - the guest never answered, or a copy failed - and the VM is
/// torn down and its disk retained; only an attempt cut short by the provider being killed
/// leaves a VM to resume.
fn finish_provisioning(
    locked: &LockedState,
    config: &Config,
    state_path: &Path,
    mut vm: VmState,
    mut child: Option<&mut Child>,
) -> Result<EnsureResponse> {
    let booted = {
        let _progress = Progress::start("waiting for the guest's ssh");
        wait_for_ssh(child.as_deref_mut(), &vm, config.boot_timeout_secs)
    };
    if let Err(error) = booted {
        // The tail is where a boot failure shows; the whole console log of a guest that
        // looped on a panic would push the error past what the server keeps of stderr.
        let log = tail_of_file(&vm.log, CONSOLE_LOG_TAIL).unwrap_or_else(|read_error| {
            format!("<failed to read {}: {read_error}>", vm.log.display())
        });
        let log_path = vm.log.display().to_string();
        terminate(&vm, child.as_deref_mut());
        cleanup_files(&vm);
        let _ = fs::remove_file(state_path);
        return Err(error).context(format!(
            "Firecracker log (last {CONSOLE_LOG_TAIL} bytes, full log was at {log_path}):\n{log}"
        ));
    }
    if let Err(error) = provision_guest(&vm, config) {
        terminate(&vm, child.as_deref_mut());
        retain_recovery(locked, state_path, &vm, "provision-failed")?;
        let _ = fs::remove_file(state_path);
        return Err(error);
    }
    vm.ready = true;
    if let Err(error) = write_state(state_path, &vm) {
        terminate(&vm, child);
        retain_recovery(locked, state_path, &vm, "state-commit-failed")?;
        let _ = fs::remove_file(state_path);
        return Err(error);
    }
    Ok(response(config, &vm))
}

/// How much of a failed guest's console log an error carries.
const CONSOLE_LOG_TAIL: u64 = 64 * 1024;

/// The last `limit` bytes of a file, from the first whole line inside them.
fn tail_of_file(path: &Path, limit: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut bytes = Vec::new();
    if len > limit {
        file.seek(SeekFrom::Start(len - limit))?;
        file.read_to_end(&mut bytes)?;
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    } else {
        file.read_to_end(&mut bytes)?;
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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

/// Wait for the guest's sshd and record its host key. `child` is the Firecracker process
/// when this provider started it; a resumed VM is watched through its pid instead.
fn wait_for_ssh(mut child: Option<&mut Child>, state: &VmState, timeout_secs: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.as_deref_mut() {
            Some(child) => {
                if let Some(status) = child.try_wait().context("failed waiting for Firecracker")? {
                    return Err(anyhow!(
                        "Firecracker exited before SSH was ready ({status}); log: {}",
                        state.log.display()
                    ));
                }
            },
            None => {
                if !vm_is_running(state) {
                    return Err(anyhow!(
                        "Firecracker exited before SSH was ready; log: {}",
                        state.log.display()
                    ));
                }
            },
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

/// The rsync spelling of the guest directory the worktree is mirrored into, with the trailing
/// slash that makes rsync copy the directory's contents rather than the directory itself.
fn guest_workspace(state: &VmState, config: &Config) -> String {
    format!("root@{}:{}/", state.guest_ip, config.guest_cwd.display())
}

fn ensure_guest_dir(state: &VmState, config: &Config, path: &Path) -> Result<()> {
    run(
        ssh_command(state, config)
            .arg("mkdir")
            .arg("-p")
            .arg("--")
            .arg(path),
        "failed to create a directory in the guest",
    )
}

fn sync_to_guest(state: &VmState, config: &Config, worktree: &Path) -> Result<()> {
    let staging = Staging::fresh(state)?;
    let staged = staging.path().join("workspace");
    if worktree.join(".git").exists() {
        let _progress = Progress::start("cloning the worktree");
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
    {
        let _progress = Progress::start("staging the worktree");
        stage_filtered_worktree(worktree, &staged)?;
    }
    let _progress = Progress::start("copying the worktree into the guest");
    // The image only pre-creates the default `/workspace`; a configured `guest_cwd` may not
    // exist yet, and rsync does not create the parents of its destination.
    ensure_guest_dir(state, config, &config.guest_cwd)?;
    run(
        filtered_rsync()
            .args(["-a", "--delete"])
            .args(guest_rsync_args(state, config))
            .arg(with_trailing_slash(&staged))
            .arg(guest_workspace(state, config)),
        "failed to copy the worktree into the guest",
    )
}

/// The copies of a worktree between the host and a running guest, after provisioning.
/// Production is rsync over ssh; the tests stand a directory in for the guest.
///
/// A push names its paths. The guest keeps running through a sync, so a push that made the
/// guest's tree match the host's - `--delete` and all - would revert or remove whatever the
/// guest wrote between the pull and the push, and the baseline written afterwards would
/// hide the loss. Naming exactly the paths the merge decided leaves everything else alone,
/// and the next sync reads it as the guest change it is.
trait GuestCopy {
    /// Copy the guest's worktree, without its `.git`, into the empty directory `dest`.
    fn pull(&self, state: &VmState, config: &Config, dest: &Path) -> Result<()>;
    /// Copy `paths`, relative to `worktree`, into the guest's worktree, replacing whatever
    /// the guest holds at each. A directory is copied as an entry of its own, not with its
    /// contents; the merge names every entry it wants.
    fn push_paths(
        &self,
        state: &VmState,
        config: &Config,
        worktree: &Path,
        paths: &[String],
    ) -> Result<()>;
    /// Remove `paths`, relative to the guest's worktree, from the guest.
    fn remove_paths(&self, state: &VmState, config: &Config, paths: &[String]) -> Result<()>;
}

struct RsyncOverSsh;

impl GuestCopy for RsyncOverSsh {
    fn pull(&self, state: &VmState, config: &Config, dest: &Path) -> Result<()> {
        // The guest's `.git` is its own standalone clone; nothing on the host mirrors it,
        // and pulling it would cost the whole repository on every release.
        run(
            filtered_rsync()
                .args(["-a", "--exclude=.git"])
                .args(guest_rsync_args(state, config))
                .arg(guest_workspace(state, config))
                .arg(with_trailing_slash(dest)),
            "failed to copy changes out of the guest",
        )
    }

    fn push_paths(
        &self,
        state: &VmState,
        config: &Config,
        worktree: &Path,
        paths: &[String],
    ) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut list = Vec::new();
        for path in paths {
            list.extend_from_slice(path.as_bytes());
            list.push(0);
        }
        // `--files-from` makes the list exact: a directory named in it is sent as an entry,
        // not recursed into, and `--relative` recreates its parents. `--force` lets a file
        // replace a directory the guest has already been told to empty.
        run_with_input(
            filtered_rsync()
                .args([
                    "-a",
                    "--force",
                    "--exclude=.git",
                    "--files-from=-",
                    "--from0",
                ])
                .args(guest_rsync_args(state, config))
                .arg(with_trailing_slash(worktree))
                .arg(guest_workspace(state, config)),
            &list,
            "failed to copy the merged paths into the guest",
        )
    }

    fn remove_paths(&self, state: &VmState, config: &Config, paths: &[String]) -> Result<()> {
        // A few hundred quoted paths per command keeps well inside the argument limit.
        for chunk in paths.chunks(256) {
            let script = format!(
                "cd {} && rm -rf -- {}",
                shell_quote(&config.guest_cwd.display().to_string()),
                chunk
                    .iter()
                    .map(|path| shell_quote(path))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            run(
                ssh_command(state, config).arg(script),
                "failed to remove paths from the guest",
            )?;
        }
        Ok(())
    }
}

/// What a sync learnt from the guest: the merge's outcome, the host worktree as it now
/// stands, and what the guest has to be told to match it.
#[derive(Debug, Default)]
struct GuestSync {
    report: MergeReport,
    /// `None` for a VM without a worktree.
    baseline: Option<Baseline>,
    push: Vec<String>,
    remove: Vec<String>,
}

/// Pull the guest's copy of the worktree and merge it into the host's.
fn sync_from_guest(state: &VmState, config: &Config, copy: &dyn GuestCopy) -> Result<GuestSync> {
    let Some(worktree) = state.worktree.as_deref() else {
        return Ok(GuestSync::default());
    };
    let baseline = read_baseline(state)?;
    let staging = Staging::fresh(state)?;
    let guest = staging.subdir("guest")?;
    let undo = staging.subdir("undo")?;
    {
        let _progress = Progress::start("copying changes out of the guest");
        copy.pull(state, config, &guest)?;
    }
    let _progress = Progress::start("merging the guest's changes");
    let merged = merge_worktrees(&baseline, worktree, &guest, &undo)?;
    Ok(GuestSync {
        report: merged.report,
        baseline: Some(merged.baseline),
        push: merged.push,
        remove: merged.remove,
    })
}

/// A scratch directory beside the VM's disk for the copies in and out of the guest.
///
/// Beside the disk rather than under `TMPDIR` so that nothing is left behind when the
/// provider is killed mid-copy: the next attempt clears it, and removing the VM removes it.
struct Staging(PathBuf);

impl Staging {
    fn fresh(state: &VmState) -> Result<Self> {
        let path = staging_path(state)?;
        remove_staging(&path)?;
        fs::create_dir(&path).with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// A fresh directory of its own inside the staging area.
    fn subdir(&self, name: &str) -> Result<PathBuf> {
        let path = self.0.join(name);
        fs::create_dir(&path).with_context(|| format!("failed to create {}", path.display()))?;
        Ok(path)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn staging_path(state: &VmState) -> Result<PathBuf> {
    Ok(state
        .rootfs
        .parent()
        .context("VM rootfs has no parent directory")?
        .join(STAGING_DIR))
}

fn remove_staging(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to clear {}", path.display())),
    }
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

fn filtered_rsync() -> Command {
    let mut command = Command::new("rsync");
    command.arg(format!("--filter={GITIGNORE_FILTER}"));
    command
}

/// One content hash per path in the filtered view of a worktree, keyed by the path relative
/// to its root. Equal maps mean the same files with the same contents and modes. Timestamps
/// are deliberately not part of it: a save that changes nothing is not a change.
type Baseline = BTreeMap<String, u64>;

fn baseline_path(state: &VmState) -> Result<PathBuf> {
    Ok(state
        .rootfs
        .parent()
        .context("VM rootfs has no parent directory")?
        .join(BASELINE_FILE))
}

fn read_baseline(state: &VmState) -> Result<Baseline> {
    let path = baseline_path(state)?;
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read the worktree baseline {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid worktree baseline {}", path.display()))
}

fn write_baseline(path: &Path, baseline: &Baseline) -> Result<()> {
    write_json_atomically(path, baseline)
        .with_context(|| format!("failed to write the worktree baseline {}", path.display()))
}

/// Write `value` beside `path` and rename it into place, so a provider killed mid-write
/// leaves the previous file rather than a truncated one.
fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error).context("failed to commit the file");
    }
    Ok(())
}

/// The baseline of a worktree as the guest sees it: staged through the gitignore filter
/// first, so ignored outputs and `.git` are not part of it.
fn worktree_baseline(worktree: &Path) -> Result<Baseline> {
    let staging = tempfile::tempdir().context("failed to stage the worktree baseline")?;
    stage_filtered_worktree(worktree, staging.path())?;
    tree_entries(staging.path())
}

fn tree_entries(root: &Path) -> Result<Baseline> {
    fn visit(root: &Path, path: &Path, entries: &mut Baseline) -> Result<()> {
        let mut children = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            if path == root && entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
            let metadata = fs::symlink_metadata(&path)?;
            entries.insert(relative, entry_hash(&path, &metadata)?);
            if metadata.is_dir() {
                visit(root, &path, entries)?;
            }
        }
        Ok(())
    }
    let mut entries = Baseline::new();
    visit(root, root, &mut entries)?;
    Ok(entries)
}

/// Kind, mode and content (or link target) of one entry, with a portable hash so a baseline
/// written by one build of the provider reads back the same in the next.
fn entry_hash(path: &Path, metadata: &fs::Metadata) -> Result<u64> {
    let mut hasher = PortableHash::new(Key::default());
    let kind = metadata.file_type();
    hasher.append(&[
        u8::from(kind.is_dir()),
        u8::from(kind.is_symlink()),
        u8::from(kind.is_file()),
    ]);
    hasher.append(&metadata.permissions().mode().to_le_bytes());
    if kind.is_symlink() {
        hasher.append(fs::read_link(path)?.as_os_str().as_bytes());
    } else if kind.is_file() {
        hasher.append(&metadata.len().to_le_bytes());
        let mut file = File::open(path)?;
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.append(&buffer[..read]);
        }
    }
    Ok(hasher.finalize64())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MergeReport {
    /// Paths whose guest change was copied into the host worktree, deletions included.
    applied: Vec<String>,
    conflicts: Vec<Conflict>,
}

/// A path both sides changed since the baseline. The host copy stays; the guest copy, when
/// the guest still has one, is written beside it.
#[derive(Debug, PartialEq, Eq)]
struct Conflict {
    path: String,
    guest_copy: Option<String>,
}

impl MergeReport {
    fn warnings(&self, worktree: &Path) -> Vec<String> {
        self.conflicts
            .iter()
            .map(|conflict| {
                let path = worktree.join(&conflict.path);
                match &conflict.guest_copy {
                    Some(copy) => format!(
                        "{}: changed on both sides; kept the host copy, the guest copy is {}",
                        path.display(),
                        worktree.join(copy).display()
                    ),
                    None => format!(
                        "{}: changed on the host and deleted in the guest; kept the host copy",
                        path.display()
                    ),
                }
            })
            .collect()
    }
}

/// The outcome of a merge: the report for the user, the host worktree as it now stands, and
/// what the guest has to take from the host to agree with it.
#[derive(Debug, Default)]
struct Merged {
    report: MergeReport,
    baseline: Baseline,
    /// Paths the guest must now take from the host.
    push: Vec<String>,
    /// Paths the guest must now drop.
    remove: Vec<String>,
}

/// Everything a merge is going to do, decided before anything is touched.
#[derive(Debug, Default, PartialEq, Eq)]
struct MergePlan {
    /// Guest-only changes to copy onto the host, parents first.
    copies: Vec<String>,
    /// Guest-only deletions to apply on the host, deepest first.
    deletions: Vec<String>,
    /// Both sides changed: the host copy stays, the guest copy goes beside it.
    conflicts: Vec<Conflict>,
    /// `copies` and `deletions` in path order, as the report lists them.
    applied: Vec<String>,
    /// What the guest lacks of the host's side: host-only changes, the host copy of every
    /// conflict, and the guest copies written beside them.
    host_wins: Vec<String>,
    /// What the guest still holds that the host no longer does.
    host_deleted: Vec<String>,
}

/// Decide a three-way merge of the guest's copy of a worktree into the host's, against the
/// baseline both started from. Per path: agreement needs nothing; a change on one side only
/// wins; a change on both sides keeps the host copy and is reported. Pure: what the plan
/// does to the two trees is `apply_merge`'s business.
fn plan_merge(baseline: &Baseline, host: &Baseline, guest: &Baseline) -> MergePlan {
    let mut plan = MergePlan::default();
    let paths: BTreeSet<&String> = baseline
        .keys()
        .chain(host.keys())
        .chain(guest.keys())
        .collect();
    for path in paths {
        let before = baseline.get(path);
        let on_host = host.get(path);
        let on_guest = guest.get(path);
        if on_guest == on_host {
            continue;
        }
        if on_guest == before {
            match on_host {
                Some(_) => plan.host_wins.push(path.clone()),
                None => plan.host_deleted.push(path.clone()),
            }
            continue;
        }
        if on_host == before {
            match on_guest {
                Some(_) => plan.copies.push(path.clone()),
                None => plan.deletions.push(path.clone()),
            }
            plan.applied.push(path.clone());
            continue;
        }
        let guest_copy = on_guest.map(|_| format!("{path}{GUEST_COPY_SUFFIX}"));
        // The guest converges on the host's view of the path: the host copy where there is
        // one, nothing where the host deleted it - its own copy survives beside it.
        match on_host {
            Some(_) => plan.host_wins.push(path.clone()),
            None => plan.host_deleted.push(path.clone()),
        }
        if let Some(copy) = &guest_copy {
            plan.host_wins.push(copy.clone());
        }
        plan.conflicts.push(Conflict {
            path: path.clone(),
            guest_copy,
        });
    }
    // Deepest first, so a directory is only removed once everything the guest deleted under
    // it is gone; one that still holds something — a host-only addition, an ignored output —
    // stays. Copies go parents first, which is the order the set handed them out in.
    plan.deletions.sort_by(|a, b| b.cmp(a));
    plan
}

/// Refuse a plan that could not be carried out in full before any of it is. What is checked
/// is what `apply_merge` would otherwise fail on halfway: a copy whose source has gone, and a
/// file or link taking the place of a directory the plan does not empty - one still holding
/// something the merge never sees, an ignored output say, which `copy_entry` refuses to
/// remove.
fn validate_merge(plan: &MergePlan, worktree: &Path, guest: &Path) -> Result<()> {
    let deletions: HashSet<&str> = plan.deletions.iter().map(String::as_str).collect();
    let sources = plan
        .copies
        .iter()
        .map(|path| (path.as_str(), path.as_str()))
        .chain(plan.conflicts.iter().filter_map(|conflict| {
            conflict
                .guest_copy
                .as_deref()
                .map(|copy| (conflict.path.as_str(), copy))
        }));
    for (source, destination) in sources {
        let metadata = fs::symlink_metadata(guest.join(source)).with_context(|| {
            format!("the guest's copy of {source} could not be read; nothing was merged")
        })?;
        if metadata.is_dir() {
            continue;
        }
        let target = worktree.join(destination);
        if fs::symlink_metadata(&target).is_ok_and(|existing| existing.is_dir()) {
            if let Some(kept) = first_survivor(worktree, destination, &deletions)? {
                return Err(anyhow!(
                    "the guest replaced the directory {destination} with a file, but the host \
                     still holds {kept} in it; nothing was merged"
                ));
            }
        }
    }
    Ok(())
}

/// The first entry under `dir` (relative to `worktree`) that the plan's deletions would
/// leave in place, if any.
fn first_survivor(worktree: &Path, dir: &str, deletions: &HashSet<&str>) -> Result<Option<String>> {
    for entry in fs::read_dir(worktree.join(dir))? {
        let entry = entry?;
        let relative = format!("{dir}/{}", entry.file_name().to_string_lossy());
        if !deletions.contains(relative.as_str()) {
            return Ok(Some(relative));
        }
        if entry.file_type()?.is_dir() {
            if let Some(kept) = first_survivor(worktree, &relative, deletions)? {
                return Ok(Some(kept));
            }
        }
    }
    Ok(None)
}

/// One step of a merge, recorded before the step so it can be undone.
#[derive(Debug)]
enum UndoStep {
    /// Nothing was there: whatever the merge put there goes.
    Remove(PathBuf),
    /// A file or link was there, and a copy of it is kept under the undo directory.
    RestoreFile { path: PathBuf, saved: PathBuf },
    /// A directory was there. It is only ever removed once empty or replaced when empty, so
    /// an empty directory with the same mode restores it.
    RestoreDir { path: PathBuf, mode: u32 },
}

fn snapshot(log: &mut Vec<UndoStep>, worktree: &Path, relative: &str, undo: &Path) -> Result<()> {
    let path = worktree.join(relative);
    let step = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => UndoStep::Remove(path),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        },
        Ok(metadata) if metadata.is_dir() => UndoStep::RestoreDir {
            path,
            mode: metadata.permissions().mode(),
        },
        Ok(_) => {
            let saved = undo.join(relative);
            copy_entry(&path, &saved)?;
            UndoStep::RestoreFile { path, saved }
        },
    };
    log.push(step);
    Ok(())
}

/// Undo the recorded steps, last first. Every step is attempted; the first failure is
/// what is reported.
fn roll_back(log: Vec<UndoStep>) -> Result<()> {
    let mut first_error = None;
    for step in log.into_iter().rev() {
        let result = match step {
            UndoStep::Remove(path) => remove_entry(&path),
            UndoStep::RestoreFile { path, saved } => copy_entry(&saved, &path),
            UndoStep::RestoreDir { path, mode } => (|| -> Result<()> {
                match fs::symlink_metadata(&path) {
                    Ok(existing) if existing.is_dir() => {},
                    Ok(_) => {
                        fs::remove_file(&path)?;
                        fs::create_dir(&path)?;
                    },
                    Err(_) => fs::create_dir_all(&path)?,
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
                Ok(())
            })()
            .with_context(|| format!("failed to restore {}", path.display())),
        };
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Carry out the plan on the host worktree, or leave the worktree as it was. Every step is
/// recorded before it is taken; the first that fails rolls the ones before it back, so a
/// merge is applied whole or not at all.
fn apply_merge(plan: &MergePlan, worktree: &Path, guest: &Path, undo: &Path) -> Result<()> {
    let mut log = Vec::new();
    let applied = (|| -> Result<()> {
        for conflict in &plan.conflicts {
            if let Some(copy) = &conflict.guest_copy {
                snapshot(&mut log, worktree, copy, undo)?;
                copy_entry(&guest.join(&conflict.path), &worktree.join(copy))?;
            }
        }
        for path in &plan.deletions {
            snapshot(&mut log, worktree, path, undo)?;
            remove_entry(&worktree.join(path))?;
        }
        for path in &plan.copies {
            snapshot(&mut log, worktree, path, undo)?;
            copy_entry(&guest.join(path), &worktree.join(path))?;
        }
        Ok(())
    })();
    match applied {
        Ok(()) => Ok(()),
        Err(error) => match roll_back(log) {
            Ok(()) => Err(error.context("nothing was merged: the host worktree was rolled back")),
            Err(rollback_error) => Err(error.context(format!(
                "the host worktree was left partly merged; rolling it back failed too: \
                 {rollback_error:#}"
            ))),
        },
    }
}

/// The host worktree's baseline after `plan` was applied to it, derived from the trees the
/// plan was made from rather than hashed again: the host's entries, less what was deleted,
/// with the guest's entry for every path it won and every copy written beside a conflict.
fn merged_baseline(
    plan: &MergePlan,
    host: &Baseline,
    guest: &Baseline,
    worktree: &Path,
) -> Baseline {
    let mut baseline = host.clone();
    for path in &plan.deletions {
        // A directory that still held something stayed.
        if fs::symlink_metadata(worktree.join(path)).is_err() {
            baseline.remove(path);
        }
    }
    for path in &plan.copies {
        if let Some(hash) = guest.get(path) {
            baseline.insert(path.clone(), *hash);
        }
    }
    for conflict in &plan.conflicts {
        if let (Some(copy), Some(hash)) = (&conflict.guest_copy, guest.get(&conflict.path)) {
            baseline.insert(copy.clone(), *hash);
        }
    }
    baseline
}

/// Three-way merge of the guest's copy of a worktree into the host's, against the baseline
/// both started from: `plan_merge`, `validate_merge`, `apply_merge`. Both trees are read
/// through the gitignore filter and deletions are decided from that view, so ignored host
/// outputs are neither compared nor touched. `undo` is an empty directory the pre-images
/// of a rolled-back merge are kept in.
fn merge_worktrees(
    baseline: &Baseline,
    worktree: &Path,
    guest: &Path,
    undo: &Path,
) -> Result<Merged> {
    let host = worktree_baseline(worktree)?;
    let guest_entries = worktree_baseline(guest)?;
    let plan = plan_merge(baseline, &host, &guest_entries);
    validate_merge(&plan, worktree, guest)?;
    apply_merge(&plan, worktree, guest, undo)?;
    let baseline = merged_baseline(&plan, &host, &guest_entries, worktree);
    Ok(Merged {
        report: MergeReport {
            applied: plan.applied,
            conflicts: plan.conflicts,
        },
        baseline,
        push: plan.host_wins,
        remove: plan.host_deleted,
    })
}

fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to read {}", source.display()))?;
    let context = || {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    };
    if let Ok(existing) = fs::symlink_metadata(destination) {
        if existing.is_dir() && metadata.is_dir() {
            fs::set_permissions(destination, metadata.permissions()).with_context(context)?;
            return Ok(());
        }
        // A directory here is one the guest replaced with a file or link; whatever it still
        // holds was kept on purpose, so refuse rather than remove it recursively.
        if existing.is_dir() {
            fs::remove_dir(destination).with_context(context)?;
        } else {
            fs::remove_file(destination).with_context(context)?;
        }
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(context)?;
    }
    if metadata.is_dir() {
        fs::create_dir(destination).with_context(context)?;
        fs::set_permissions(destination, metadata.permissions()).with_context(context)?;
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(source).with_context(context)?;
        std::os::unix::fs::symlink(target, destination).with_context(context)?;
    } else {
        fs::copy(source, destination).with_context(context)?;
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let context = || format!("failed to remove {}", path.display());
    if metadata.is_dir() {
        if fs::read_dir(path).with_context(context)?.next().is_none() {
            fs::remove_dir(path).with_context(context)?;
        }
    } else {
        fs::remove_file(path).with_context(context)?;
    }
    Ok(())
}

fn copy_credential(state: &VmState, config: &Config, credential: &CredentialFile) -> Result<()> {
    let host = expand_home(&credential.host)?;
    if !host.is_file() {
        return Err(anyhow!("credential file {} does not exist", host.display()));
    }
    validate_guest_path(&credential.guest, "credential guest path")?;
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
    let _progress = Progress::start("copying credentials into the guest");
    run(
        Command::new("rsync")
            .args(["-a", "--chmod=F600"])
            .args(guest_rsync_args(state, config))
            .arg(&host)
            .arg(format!(
                "root@{}:{}",
                state.guest_ip,
                credential.guest.display()
            )),
        "failed to copy a configured credential into the guest",
    )
}

/// The ssh arguments for reaching a guest: the key and known-hosts handling every remote pane
/// uses, plus what a non-interactive provider needs - no prompts, a bounded connect.
fn guest_ssh_args(state: &VmState, config: &Config) -> Vec<String> {
    let mut args = strings(&["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
    args.extend(
        SshOptions {
            identity_file: Some(config.ssh_private_key.clone()),
            known_hosts_file: Some(state.known_hosts.clone()),
        }
        .args(),
    );
    args
}

fn ssh_command(state: &VmState, config: &Config) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(guest_ssh_args(state, config))
        .arg(format!("root@{}", state.guest_ip));
    command
}

/// The `-e` remote shell for rsync. rsync splits it on whitespace honouring
/// shell-style quotes and strips them, so each token is single-quoted for
/// rsync's benefit, and the known_hosts value is double-quoted inside that for
/// ssh's own option tokenizer.
fn rsync_remote_shell(state: &VmState, config: &Config) -> String {
    std::iter::once("ssh".to_owned())
        .chain(
            guest_ssh_args(state, config)
                .iter()
                .map(|arg| shell_quote(arg)),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

/// What every rsync that crosses into the guest takes: the ssh to cross with, and an I/O
/// timeout so a guest that stops answering fails the copy rather than hanging it - the
/// progress reports would otherwise keep a stuck copy alive indefinitely.
fn guest_rsync_args(state: &VmState, config: &Config) -> [String; 3] {
    [
        "--timeout=60".to_owned(),
        "-e".to_owned(),
        rsync_remote_shell(state, config),
    ]
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn with_trailing_slash(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}/", path.display()))
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME is unavailable for credential expansion")?;
    expand_home_from(path, Path::new(&home))
}

fn validate_guest_path(path: &Path, what: &str) -> Result<()> {
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
        .with_context(|| format!("unsafe {what} {}", path.display()))
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
                state: if state.ready {
                    "running".to_owned()
                } else {
                    "provisioning".to_owned()
                },
            });
        }
    }
    vms.sort_by(|a, b| (&a.session, &a.name).cmp(&(&b.session, &b.name)));
    Ok(vms)
}

#[derive(Debug, Deserialize, Serialize)]
struct RecoveryNote {
    reason: String,
    retained_at: u64,
}

/// The disks kept under `recovery/`, oldest first. A directory from before the note was
/// written is still listed, with what its `state.json` and mtime can say.
fn list_recoveries(locked: &LockedState, session: Option<&str>) -> Result<Vec<RecoveryEntry>> {
    let recovery = locked.dir.join("recovery");
    let mut entries = vec![];
    for entry in fs::read_dir(&recovery).context("failed to read Firecracker recoveries")? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let state: Option<VmState> = fs::read(path.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if let Some(wanted) = session {
            let leased = state.as_ref().is_some_and(|state| {
                state.session == wanted || state.leases.iter().any(|lease| lease == wanted)
            });
            if !leased {
                continue;
            }
        }
        let note = read_recovery_note(&path);
        let retained_at = note
            .as_ref()
            .map(|note| note.retained_at)
            .or_else(|| {
                entry
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|age| age.as_secs())
            })
            .unwrap_or(0);
        entries.push(RecoveryEntry {
            path,
            reason: note
                .map(|note| note.reason)
                .unwrap_or_else(|| "unknown".to_owned()),
            retained_at,
            session: state.as_ref().map(|state| state.session.clone()),
            name: state.as_ref().map(|state| state.name.clone()),
            worktree: state.and_then(|state| state.worktree),
        });
    }
    entries.sort_by(|a, b| (a.retained_at, &a.path).cmp(&(b.retained_at, &b.path)));
    Ok(entries)
}

fn read_recovery_note(recovery_dir: &Path) -> Option<RecoveryNote> {
    fs::read(recovery_dir.join(RECOVERY_NOTE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn find_vm(
    entries: &[(PathBuf, VmState)],
    session: &str,
    name: &str,
) -> Result<Option<(PathBuf, VmState)>> {
    let mut matching = entries.iter().filter(|(_, state)| {
        state.name == name
            && (state.leases.iter().any(|lease| lease == session)
                || (state.leases.is_empty() && state.session == session))
    });
    let found = matching.next().cloned();
    if matching.next().is_some() {
        return Err(anyhow!(
            "multiple Firecracker VMs named {name:?} are running for session {session:?}"
        ));
    }
    Ok(found)
}

fn ensure_vm_identity_available(
    entries: &[(PathBuf, VmState)],
    state_path: &Path,
    session: &str,
    name: &str,
) -> Result<()> {
    if find_vm(entries, session, name)?.is_some_and(|(path, _)| path != state_path) {
        return Err(anyhow!(
            "Firecracker VM {name:?} for session {session:?} already identifies another running VM"
        ));
    }
    Ok(())
}

/// Retain the disks of VMs whose process has gone. A live VM is left alone whether or not
/// it is ready: an unready one is either being provisioned right now or waiting for the
/// `ensure` that resumes it, and its session's release covers it either way.
fn recover_unusable_vms(locked: &LockedState) -> Result<()> {
    for (path, state) in locked.entries()? {
        if vm_is_running(&state) {
            continue;
        }
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

fn stop_one(
    locked: &LockedState,
    config: &Config,
    copy: &dyn GuestCopy,
    session: &str,
    name: &str,
) -> Result<Vec<String>> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let Some((path, state)) = find_vm(&locked.entries()?, session, name)? else {
        return Ok(vec![]);
    };
    release(locked, config, copy, path, state, session)
}

/// Move every VM `from` owns or leases to `to`: the Zellij session was renamed, and the
/// management commands resolve the session by its current name.
///
/// A VM identified by session and name lives at `path(session, name)`, so its state file
/// moves; one identified by its worktree keeps its path and only its fields change. Every
/// conflict is found before anything is written, so a refused rename changes nothing. The
/// VM directory, disk and cgroup stay where they are - the state records them by absolute
/// path and nothing derives them from the state file's name after creation.
fn rename_session(locked: &LockedState, from: &str, to: &str) -> Result<()> {
    if from == to {
        return Ok(());
    }
    let entries = locked.entries()?;
    let mut moves = vec![];
    for (path, state) in &entries {
        let owned = state.session == from;
        let leased = state.leases.iter().any(|lease| lease == from);
        if !owned && !leased {
            continue;
        }
        let destination = if *path == locked.path(from, &state.name) {
            let destination = locked.path(to, &state.name);
            ensure_vm_identity_available(&entries, &destination, to, &state.name)?;
            if destination.exists() {
                return Err(anyhow!(
                    "Firecracker VM {:?} already has state for session {to:?}",
                    state.name
                ));
            }
            Some(destination)
        } else {
            None
        };
        moves.push((path, state, destination));
    }
    for (path, state, destination) in moves {
        let mut state = state.clone();
        if state.session == from {
            state.session = to.to_owned();
        }
        for lease in &mut state.leases {
            if lease == from {
                *lease = to.to_owned();
            }
        }
        let mut seen = HashSet::new();
        state.leases.retain(|lease| seen.insert(lease.clone()));
        match destination {
            Some(destination) => {
                write_state(&destination, &state)?;
                fs::remove_file(path).with_context(|| {
                    format!("failed to remove the old state file {}", path.display())
                })?;
            },
            None => write_state(path, &state)?,
        }
    }
    Ok(())
}

/// Releases every VM the session holds, returning the merge warnings of all of them. One
/// VM's failure does not stop the others from being released; the first error is reported.
fn stop_session(
    locked: &LockedState,
    config: &Config,
    copy: &dyn GuestCopy,
    session: &str,
) -> Result<Vec<String>> {
    let mut first_error = None;
    let mut warnings = vec![];
    for (path, state) in locked.entries()? {
        if state.session == session || state.leases.iter().any(|lease| lease == session) {
            match release(locked, config, copy, path, state, session) {
                Ok(released) => warnings.extend(released),
                Err(error) => {
                    first_error.get_or_insert(error);
                },
            }
        }
    }
    first_error.map_or(Ok(warnings), Err)
}

/// Gives a session's lease back and, when it was the last one, merges the guest's changes into
/// the host worktree and shuts the VM down. Returns the merge warnings. A merge that could not
/// be attempted at all — the guest unreachable, rsync failing — keeps the disk for recovery.
fn release(
    locked: &LockedState,
    config: &Config,
    copy: &dyn GuestCopy,
    path: PathBuf,
    mut state: VmState,
    session: &str,
) -> Result<Vec<String>> {
    state.leases.retain(|lease| lease != session);
    if !state.leases.is_empty() {
        write_state(&path, &state)?;
        return Ok(vec![]);
    }
    if !state.ready {
        // A guest that was never fully provisioned holds a partial copy of the worktree,
        // and merging that back would read every file it is missing as a deletion.
        terminate(&state, None);
        retain_recovery(locked, &path, &state, "unready")?;
        let _ = fs::remove_file(path);
        return Ok(vec![]);
    }
    let report = match sync_from_guest(&state, config, copy) {
        Ok(synced) => synced.report,
        Err(error) => {
            terminate(&state, None);
            retain_recovery(locked, &path, &state, "sync-failed")?;
            let _ = fs::remove_file(path);
            return Err(error);
        },
    };
    terminate(&state, None);
    cleanup_files(&state);
    let _ = fs::remove_file(path);
    Ok(state
        .worktree
        .as_deref()
        .map(|worktree| report.warnings(worktree))
        .unwrap_or_default())
}

/// A manual sync while the VM keeps running: merge the guest into the host, then bring the
/// guest up to the merged tree and record it as the new baseline, so both sides agree again.
fn sync_one(
    locked: &LockedState,
    config: &Config,
    copy: &dyn GuestCopy,
    session: &str,
    name: &str,
) -> Result<Vec<String>> {
    validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    let Some((_, state)) = find_vm(&locked.entries()?, session, name)? else {
        return Err(anyhow!(
            "Firecracker VM {name:?} is not running for session {session:?}"
        ));
    };
    if !vm_is_running(&state) {
        return Err(anyhow!("Firecracker VM {name:?} is not running"));
    }
    if !state.ready {
        return Err(anyhow!(
            "Firecracker VM {name:?} is still being provisioned; open a pane in it first"
        ));
    }
    let synced = sync_from_guest(&state, config, copy)?;
    let (Some(worktree), Some(baseline)) = (state.worktree.as_deref(), synced.baseline) else {
        return Ok(vec![]);
    };
    // Removals first: a directory the host turned into a file has to be emptied before the
    // file can take its place.
    {
        let _progress = Progress::start("removing the host's deletions from the guest");
        copy.remove_paths(&state, config, &synced.remove)?;
    }
    {
        let _progress = Progress::start("copying the merged paths into the guest");
        copy.push_paths(&state, config, worktree, &synced.push)?;
    }
    write_baseline(&baseline_path(&state)?, &baseline)?;
    Ok(synced.report.warnings(worktree))
}

fn terminate(state: &VmState, mut child: Option<&mut Child>) {
    if !vm_is_running(state) {
        if let Some(child) = child.as_mut() {
            let _ = child.try_wait();
        }
        return;
    }
    unsafe { libc::kill(state.pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(child) = child.as_mut() {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
        } else if !vm_is_running(state) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    } else if vm_is_running(state) {
        unsafe {
            libc::kill(state.pid, libc::SIGKILL);
        }
        // Whoever called is about to remove the VM's cgroup and disk, and a process that
        // has been signalled is not yet gone: the cgroup refuses to go while it is still a
        // member, and the disk is still mapped. Give it the moment it needs.
        wait_for_process_exit(state, Duration::from_secs(2));
    }
}

/// Wait for a VM process that is not this provider's child to leave `/proc`, up to
/// `timeout`. True once it has.
fn wait_for_process_exit(state: &VmState, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !vm_is_running(state) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
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

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The cgroup v2 subtree this process may manage on its own: the highest ancestor of its
/// own cgroup whose `cgroup.procs` it owns. systemd delegates `user@<uid>.service` to the
/// user, so a provider started from a login session finds that. Nothing outside such a
/// subtree will do - moving a process needs write access to `cgroup.procs` of the common
/// ancestor of where it is and where it is going, and above the delegation boundary that
/// file is root's - which is why `network setup` cannot prepare a cgroup for us.
fn delegated_cgroup_root() -> Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let own = fs::read_to_string("/proc/self/cgroup")?
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| Path::new(CGROUP_ROOT).join(path.trim().trim_start_matches('/')))
        .context("this process is not in a cgroup v2 hierarchy")?;
    delegation_root(&own, &|dir| {
        fs::metadata(dir.join("cgroup.procs")).is_ok_and(|metadata| metadata.uid() == uid)
    })
}

fn delegation_root(own: &Path, owned: &dyn Fn(&Path) -> bool) -> Result<PathBuf> {
    if !owned(own) {
        return Err(anyhow!(
            "cgroup {} is not delegated to this user, so a VM's memory, CPU and process \
             limits cannot be applied",
            own.display()
        ));
    }
    let mut root = own.to_path_buf();
    while let Some(parent) = root.parent() {
        if parent == Path::new(CGROUP_ROOT) || !owned(parent) {
            break;
        }
        root = parent.to_path_buf();
    }
    Ok(root)
}

/// Put the VMM in a cgroup of its own under `root`, the delegated subtree `ensure` resolved
/// before it did any expensive work. `None` means the host has no cgroup subtree this user
/// may manage and the config allows running without one: the guest's own RAM and vCPUs are
/// still capped by Firecracker, but nothing bounds the VMM's page cache, threads or file
/// descriptors.
fn place_in_cgroup(
    id: &str,
    pid: i32,
    memory_mib: usize,
    vcpu_count: u8,
    root: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let Some(root) = root else {
        return Ok(None);
    };
    let parent = root.join("zellij-firecracker");
    fs::create_dir_all(&parent)?;
    fs::write(parent.join("cgroup.subtree_control"), "+cpu +memory +pids").with_context(|| {
        format!(
            "failed to enable cpu, memory and pids control under {}",
            parent.display()
        )
    })?;
    let cgroup = parent.join(id);
    fs::create_dir_all(&cgroup)?;
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
    Ok(Some(cgroup))
}

/// Only the *absence* of a delegated subtree is optional. A subtree that exists but cannot
/// be written to is a broken host, not a missing feature, so those errors stay fatal in
/// `place_in_cgroup` whatever the config says. Both arms speak for `doctor` as well as for
/// a VM start, so neither says which one it is.
fn cgroup_root_or_skip(root: Result<PathBuf>, require: bool) -> Result<Option<PathBuf>> {
    match root {
        Ok(root) => Ok(Some(root)),
        Err(error) if !require => {
            eprintln!("warning: {error:#}; running without them");
            Ok(None)
        },
        Err(error) => Err(anyhow!(
            "{error:#}; start Zellij from a systemd user session, or unset \
             require_resource_limits in the provider config to run VMs without them"
        )),
    }
}

fn retain_recovery(
    locked: &LockedState,
    state_path: &Path,
    state: &VmState,
    reason: &str,
) -> Result<()> {
    if let Some(vm_dir) = state.rootfs.parent().filter(|dir| dir.exists()) {
        // A copy the provider was killed in the middle of has nothing to say about the disk.
        remove_staging(&vm_dir.join(STAGING_DIR))?;
        let recovery = locked.dir.join("recovery");
        ensure_secure_dir(&recovery)?;
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
        fs::write(
            destination.join(RECOVERY_NOTE_FILE),
            serde_json::to_vec_pretty(&RecoveryNote {
                reason: reason.to_owned(),
                retained_at: timestamp,
            })?,
        )?;
    }
    cleanup_recovery_resources(state);
    Ok(())
}

/// Move a VM directory that no state file describes into the recoveries, so a fresh attempt
/// for the same identity starts from an empty directory. Nothing to do when it is absent.
fn retain_orphaned_vm_dir(locked: &LockedState, vm_dir: &Path) -> Result<()> {
    if !vm_dir.exists() {
        return Ok(());
    }
    remove_staging(&vm_dir.join(STAGING_DIR))?;
    let recovery = locked.dir.join("recovery");
    ensure_secure_dir(&recovery)?;
    let id = vm_dir
        .file_name()
        .and_then(|id| id.to_str())
        .unwrap_or("vm");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let destination = recovery.join(format!("{id}-{timestamp}-orphaned"));
    fs::rename(vm_dir, &destination).with_context(|| {
        format!(
            "failed to move the orphaned Firecracker VM directory {} to {}",
            vm_dir.display(),
            destination.display()
        )
    })?;
    fs::write(
        destination.join(RECOVERY_NOTE_FILE),
        serde_json::to_vec_pretty(&RecoveryNote {
            reason: "orphaned".to_owned(),
            retained_at: timestamp,
        })?,
    )?;
    Ok(())
}

/// Remove the recoveries older than the retention period, aged by the note `retain_recovery`
/// wrote: a directory's mtime changes whenever something inside it is looked at or copied
/// out, so it only stands in for a directory whose note cannot be read.
fn prune_recoveries(state: &Path) -> Result<()> {
    let recovery = state.join("recovery");
    ensure_secure_dir(&recovery)?;
    let now = SystemTime::now();
    for entry in fs::read_dir(&recovery)? {
        let entry = entry?;
        // Only directories are recoveries; a stray file here (an editor's swap file, a
        // note) is nothing to remove, and `remove_dir_all` on it would fail every request.
        if !entry.path().is_dir() {
            continue;
        }
        let retained_at = match read_recovery_note(&entry.path()) {
            Some(note) => Some(UNIX_EPOCH + Duration::from_secs(note.retained_at)),
            None => entry.metadata()?.modified().ok(),
        };
        let age = retained_at.and_then(|retained_at| now.duration_since(retained_at).ok());
        if age.is_some_and(|age| age > RECOVERY_RETENTION) {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn vm_is_running(state: &VmState) -> bool {
    process_start_time(state.pid) == Some(state.start_time)
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
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "invalid VM state in {}; repair or remove it",
            path.display()
        )
    })
}

/// A state file that is not there is a VM that is not there. One that is there but cannot
/// be read or parsed is not: it may describe a VM that is running right now, and treating
/// it as absent would hand the VM's slot and disk to a fresh attempt.
fn read_state_opt(path: &Path) -> Result<Option<VmState>> {
    match fs::metadata(path) {
        Ok(_) => read_state(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_state(path: &Path, state: &VmState) -> Result<()> {
    write_json_atomically(path, state).context("failed to commit VM state")
}

fn vm_id(session: &str, name: &str) -> String {
    let mut hasher = PortableHash::new(Key::default());
    hasher.append(session.as_bytes());
    hasher.append(&[0]);
    hasher.append(name.as_bytes());
    format!("{:016x}", hasher.finalize64())
}

/// Where the provider's own configuration lives. One answer for every reader and writer of
/// it: `load_config` honoured `CONFIG_ENV` while the writers hardcoded the default path, so
/// a run with the override set could verify one file and rewrite another.
fn config_path() -> PathBuf {
    env::var_os(CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join("firecracker.json"))
}

fn load_config() -> Result<Config> {
    let path = config_path();
    let config: Config = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("invalid provider config {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

/// `rootfs` is only the default image for a VM that names none, so a dangling one is not
/// this config's problem: `image_path` reports it where it is actually used, and until
/// then a config whose named images are all present stays usable.
fn validate_config(config: &Config) -> Result<()> {
    for (name, path) in [
        ("firecracker", &config.firecracker),
        ("kernel", &config.kernel),
        ("ssh_private_key", &config.ssh_private_key),
    ] {
        if !path.is_absolute() || !path.is_file() {
            return Err(anyhow!(
                "provider config {name} must be an existing absolute file, got {}",
                path.display()
            ));
        }
    }
    for (name, path) in [
        ("rootfs", &config.rootfs),
        ("images_dir", &config.images_dir),
    ] {
        if !path.is_absolute() {
            return Err(anyhow!("provider config {name} must be absolute"));
        }
    }
    validate_guest_path(&config.guest_cwd, "provider config guest_cwd")?;
    for credential in &config.credential_files {
        expand_home(&credential.host)?;
        validate_guest_path(&credential.guest, "credential guest path")?;
    }
    Ok(())
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

/// The provider's directories are Zellij's own, one level down: the same `ProjectDirs` the
/// rest of Zellij resolves, so a config, image or cache is where a user of Zellij expects it.
fn config_dir() -> PathBuf {
    xdg_config_dir()
}

fn data_dir() -> PathBuf {
    xdg_data_dir().join("firecracker")
}

fn cache_dir() -> PathBuf {
    ZELLIJ_CACHE_DIR.join("firecracker")
}

fn state_dir() -> PathBuf {
    ZELLIJ_PROJ_DIR
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/tmp/zellij"))
        .join("firecracker/vms")
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
    /// Returns the image the step built or reused, so that `config` can make it this
    /// machine's default.
    fn image(
        &mut self,
        options: &SetupOptions,
        project: &Path,
        zellij: &Path,
        current_provider: &Path,
    ) -> Result<PathBuf>;
    fn config(&mut self, provider: &Path, image: &Path) -> Result<()>;
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
    ) -> Result<PathBuf> {
        setup_image(options, project, zellij, current_provider)
    }

    fn config(&mut self, provider: &Path, image: &Path) -> Result<()> {
        // Before the credential prompts: this one is non-interactive, so a failure here
        // does not throw away answers the user has already typed.
        set_default_image(image)?;
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
    let image = runtime.image(options, &project, &zellij, &current_provider)?;
    runtime.config(&provider, &image)?;
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
) -> Result<PathBuf> {
    setup_image_with(
        options,
        project,
        zellij,
        current_provider,
        setup_image_status,
        run,
    )
}

fn setup_image_with<V, R>(
    options: &SetupOptions,
    project: &Path,
    zellij: &Path,
    current_provider: &Path,
    mut image_status: V,
    mut execute: R,
) -> Result<PathBuf>
where
    V: FnMut(&str) -> Result<PathBuf>,
    R: FnMut(&mut Command, &str) -> Result<()>,
{
    if should_build_image(options.rebuild, image_status(&options.image).is_ok()) {
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
    image_status(&options.image).with_context(|| {
        format!(
            "Firecracker image {:?} was not created correctly",
            options.image
        )
    })
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

/// The dynamic loader an ELF executable asks for, from its `PT_INTERP` program header;
/// `None` for a static binary, and for anything that is not a 64-bit little-endian ELF.
fn elf_interpreter(path: &Path) -> Result<Option<PathBuf>> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let u16_at = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let u64_at = |at: usize| {
        bytes
            .get(at..at + 8)
            .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    };
    if bytes.get(..4) != Some(b"\x7fELF") || bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        return Ok(None);
    }
    let (Some(offset), Some(entry_size), Some(count)) = (u64_at(0x20), u16_at(0x36), u16_at(0x38))
    else {
        return Ok(None);
    };
    for index in 0..usize::from(count) {
        let header = usize::try_from(offset)?.saturating_add(index * usize::from(entry_size));
        if u32_at(header) != Some(3) {
            continue;
        }
        let (Some(start), Some(len)) = (u64_at(header + 8), u64_at(header + 0x20)) else {
            return Ok(None);
        };
        let start = usize::try_from(start)?;
        let end = start.saturating_add(usize::try_from(len)?);
        let interpreter = bytes.get(start..end).context("truncated ELF interpreter")?;
        let interpreter = interpreter.strip_suffix(&[0]).unwrap_or(interpreter);
        return Ok(Some(PathBuf::from(OsStr::from_bytes(interpreter))));
    }
    Ok(None)
}

/// Say so when the Zellij going into a guest is dynamically linked. A static build runs in
/// any guest; a dynamic one needs the guest's loader and a glibc at least as new as the
/// host's, and a host newer than the guest's Ubuntu 24.04 fails inside the VM only, with
/// nothing on this side to explain it. With the staged `root` in hand the loader can at
/// least be looked for.
fn warn_about_dynamic_zellij(zellij: &Path, root: Option<&Path>) {
    let interpreter = match elf_interpreter(zellij) {
        Ok(Some(interpreter)) => interpreter,
        Ok(None) => return,
        Err(error) => {
            eprintln!(
                "warning: could not tell whether {} is statically linked: {error:#}",
                zellij.display()
            );
            return;
        },
    };
    let loader_missing = root.is_some_and(|root| {
        !root
            .join(interpreter.strip_prefix("/").unwrap_or(&interpreter))
            .exists()
    });
    eprintln!(
        "warning: {} is dynamically linked (loader {}){}; a guest whose glibc is older than \
         this host's cannot run it, and every pane in the VM then fails to start. Build a \
         static Zellij (for example --target x86_64-unknown-linux-musl) and pass it with \
         --zellij",
        zellij.display(),
        interpreter.display(),
        if loader_missing {
            " which the guest image does not have"
        } else {
            ""
        }
    );
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

/// Which image a VM that names none should boot. A named build leaves an existing default
/// alone, but adopts its own image when that default is gone: a machine whose only image is
/// a named one is better served by a working default than by a dangling path.
fn default_rootfs(existing: &Path, built: &Path, named: bool) -> PathBuf {
    if named && existing.is_file() {
        existing.to_owned()
    } else {
        built.to_owned()
    }
}

fn setup_image_status(image: &str) -> Result<PathBuf> {
    load_config().and_then(|config| image_path(&config, Some(image)))
}

fn should_build_image(rebuild: bool, valid: bool) -> bool {
    rebuild || !valid
}

/// Point this machine's default image at the one `setup` just built or reused.
///
/// `rootfs` is what a Firecracker pane that names no image boots, and until now nothing in
/// `setup` maintained it: a build wrote it once through `default_rootfs`, and a reused image
/// wrote nothing at all — so a default left over from an earlier layout survived every later
/// run, pointing at a file that need not still exist. After this, `setup --name <image>` is
/// how a machine picks its default.
fn set_default_image(image: &Path) -> Result<()> {
    set_default_image_at(&config_path(), image).map(|_| ())
}

/// The write behind [`set_default_image`], with the config path given so it can be tested.
/// Returns whether the file changed.
///
/// The config is read raw rather than through `load_config`: this is a repair, and a
/// validating read would let a config refuse to fix the very field that makes it unusable.
fn set_default_image_at(path: &Path, image: &Path) -> Result<bool> {
    let mut config: Config = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("invalid provider config {}", path.display()))?;
    if config.rootfs == image {
        return Ok(false);
    }
    config.rootfs = image.to_owned();
    fs::write(path, serde_json::to_vec_pretty(&config)?)
        .with_context(|| format!("failed to update {}", path.display()))?;
    println!(
        "Firecracker VMs that name no image now boot {}.",
        image.display()
    );
    Ok(true)
}

fn configure_credentials() -> Result<()> {
    let path = config_path();
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

/// The images `doctor` can see. `validate_config` deliberately tolerates a dangling default
/// (a machine whose VMs all name an image keeps working without one), but `doctor` answers
/// "is this machine ready", and it claims the image is, so it has to look.
fn check_image_paths(config: &Config) -> Result<()> {
    if !config.images_dir.is_dir() {
        return Err(anyhow!(
            "Firecracker images directory {} does not exist; run zellij-firecracker setup",
            config.images_dir.display()
        ));
    }
    if !config.rootfs.is_file() {
        return Err(anyhow!(
            "default Firecracker image {} does not exist; run zellij-firecracker setup",
            config.rootfs.display()
        ));
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let config = load_config()?;
    check_image_paths(&config)?;
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
        "chroot",
        "cp",
        "curl",
        "git",
        "iptables",
        "mkfs.ext4",
        "rsync",
        "sha256sum",
        "ssh",
        "ssh-keyscan",
        "unshare",
        "unsquashfs",
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
    if let Ok(provider) = env::current_exe() {
        if let Ok(zellij) = resolve_zellij_from(None, &provider, env::var_os("PATH").as_deref()) {
            warn_about_dynamic_zellij(&zellij, None);
        }
    }
    match cgroup_root_or_skip(delegated_cgroup_root(), config.require_resource_limits)? {
        Some(cgroup) => println!(
            "KVM, Firecracker, image, key, {} TAPs and the cgroup {} are ready.",
            network.taps,
            cgroup.display()
        ),
        // `cgroup_root_or_skip` has already said which cgroup is not delegated.
        None => println!(
            "KVM, Firecracker, image, key and {} TAPs are ready; VMs will run without \
             host-side memory, CPU and process limits.",
            network.taps
        ),
    }
    Ok(())
}

/// Map the caller's subordinate id ranges into the builder's user namespace
/// when the host allows it. Packages routinely `chown` to system users --
/// `libpam-modules-bin` gives `pam_extrausers_chkpwd` to `root:shadow` -- and
/// with only uid 0 mapped every one of those fails with `EINVAL`. A host with
/// no `/etc/subuid` entry or no setuid `newuidmap` falls back to mapping root
/// alone, which is enough for a script that installs nothing.
fn image_build_namespace_args() -> Vec<&'static str> {
    const MAPPED: [&str; 4] = ["--user", "--map-auto", "--map-root-user", "--mount"];
    let usable = Command::new("unshare")
        .args(MAPPED)
        .args(["--", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if usable {
        return MAPPED.to_vec();
    }
    println!(
        "No subordinate id range for this user; building with only root mapped. \
         Packages that chown to a system user will fail; see docs/FIRECRACKER.md."
    );
    vec!["--user", "--map-root-user", "--mount"]
}

fn image_build(args: &[String]) -> Result<()> {
    if env::var_os("ZELLIJ_FIRECRACKER_IMAGE_INNER").is_none() && unsafe { libc::geteuid() } != 0 {
        let exe = env::current_exe().context("failed to find zellij-firecracker")?;
        let status = Command::new("unshare")
            .args(image_build_namespace_args())
            .arg("--")
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
    ensure_secure_dir(&output)?;
    ensure_secure_dir(&cache_dir())?;
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
    warn_about_dynamic_zellij(&zellij, Some(&root));
    fs::copy(&zellij, root.join("usr/local/bin/zellij"))
        .context("failed to install Zellij in the guest")?;
    fs::set_permissions(
        root.join("usr/local/bin/zellij"),
        fs::Permissions::from_mode(0o755),
    )?;
    let ssh_dir = root.join("root/.ssh");
    ensure_secure_dir(&ssh_dir)?;
    fs::copy(
        private_key.with_extension("pub"),
        ssh_dir.join("authorized_keys"),
    )?;
    fs::set_permissions(
        ssh_dir.join("authorized_keys"),
        fs::Permissions::from_mode(0o600),
    )?;
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
        let guest_script = root.join(GUEST_CUSTOMIZE_SCRIPT.trim_start_matches('/'));
        fs::copy(&customize, &guest_script).with_context(|| {
            format!(
                "failed to stage Firecracker customization script {}",
                customize.display()
            )
        })?;
        fs::set_permissions(&guest_script, fs::Permissions::from_mode(0o755))?;
        stage_apt_directories(&root)?;
        let apt_override = write_apt_build_overrides(&root)?;
        let policy = write_policy_rc_d(&root)?;
        let saved_resolv = stage_resolv_conf(&root)?;
        let devices = stage_chroot_devices(&root)?;
        let result = image_customize(&root);
        // Undo the staging whether or not the script worked, so a failed build
        // leaves a tree the next attempt can reuse and a successful one ships
        // stock apt behaviour.
        restore_resolv_conf(&root, saved_resolv)?;
        for device in devices {
            let _ = fs::remove_file(device);
        }
        let _ = fs::remove_file(&policy);
        let _ = fs::remove_file(&apt_override);
        let _ = fs::remove_file(&guest_script);
        result?;
        // The chroot's mounts live in a child's mount namespace and die with
        // it. If any survived, `mkfs.ext4 -d` below would walk them into the
        // image, so check rather than trust.
        if fs::read_dir(root.join("proc"))?.next().is_some() {
            return Err(anyhow!(
                "the image customization left /proc mounted; refusing to write it into the image"
            ));
        }
    }
    // Last, so that the customization script installing openssh-server cannot
    // put its own `ssh-keygen -A` keys back: every VM generates its own at boot.
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
    let rootfs = match image {
        Some(image) => {
            let images = output.join("images");
            ensure_secure_dir(&images)?;
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
    ensure_secure_dir(&config_dir())?;
    let config_path = config_path();
    let config = if config_path.exists() {
        let mut config: Config = serde_json::from_slice(&fs::read(&config_path)?)?;
        config.firecracker = firecracker;
        config.kernel = output.join("vmlinux-6.1.182");
        config.images_dir = output.join("images");
        config.ssh_private_key = private_key;
        config.rootfs = default_rootfs(&config.rootfs, &rootfs, image.is_some());
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
            require_resource_limits: default_require_resource_limits(),
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

/// Two things apt cannot work out for itself here. Its download sandbox drops
/// to `_apt`, whose uid may not be mapped in the builder's user namespace, so
/// every fetch would die with `setgroups`/`seteuid` errors; and a conffile
/// dpkg considers locally modified stops for a prompt no one can answer, so
/// pin the existing version.
fn write_apt_build_overrides(root: &Path) -> Result<PathBuf> {
    let path = root.join(APT_SANDBOX_OVERRIDE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        "APT::Sandbox::User \"root\";\nDpkg::Options { \"--force-confdef\"; \"--force-confold\"; };\n",
    )
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// The base rootfs is built for containers, so apt's spool directories have
/// been stripped and apt refuses to download into them. Recreate them; they are
/// standard on any Ubuntu system, so they stay in the finished image.
fn stage_apt_directories(root: &Path) -> Result<()> {
    for directory in [
        "var/cache/apt/archives/partial",
        "var/lib/apt/lists/partial",
        "var/log/apt",
    ] {
        let path = root.join(directory);
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
    }
    Ok(())
}

/// The exit status Debian's `invoke-rc.d` reads as "do not start this service".
fn write_policy_rc_d(root: &Path) -> Result<PathBuf> {
    let path = root.join(POLICY_RC_D);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "#!/bin/sh\nexit 101\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// The first candidate that actually names a nameserver.
fn resolv_conf_from(candidates: &[PathBuf]) -> Result<Vec<u8>> {
    for candidate in candidates {
        let Ok(contents) = fs::read(candidate) else {
            continue;
        };
        if String::from_utf8_lossy(&contents)
            .lines()
            .any(|line| line.split_whitespace().next() == Some("nameserver"))
        {
            return Ok(contents);
        }
    }
    Err(anyhow!(
        "no host resolver with a nameserver entry; the image build needs DNS (checked {})",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Give the chroot the host's DNS, returning what was there before.
fn stage_resolv_conf(root: &Path) -> Result<Option<Vec<u8>>> {
    let path = root.join("etc/resolv.conf");
    let saved = fs::read(&path).ok();
    let candidates: Vec<PathBuf> = HOST_RESOLVERS.iter().map(PathBuf::from).collect();
    let contents = resolv_conf_from(&candidates)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // The guest file may be a symlink into /run; replace it outright so the
    // write cannot escape the staged root.
    let _ = fs::remove_file(&path);
    fs::write(&path, contents).with_context(|| format!("failed to stage {}", path.display()))?;
    Ok(saved)
}

fn restore_resolv_conf(root: &Path, saved: Option<Vec<u8>>) -> Result<()> {
    let path = root.join("etc/resolv.conf");
    match saved {
        Some(contents) => fs::write(&path, contents)
            .with_context(|| format!("failed to restore {}", path.display())),
        None => {
            let _ = fs::remove_file(&path);
            Ok(())
        },
    }
}

/// Create the files the device binds mount over, returning the ones that were
/// not already there so the caller can take them out again: an empty regular
/// file left at `/dev/null` would shadow the real device until the guest mounts
/// devtmpfs over it.
fn stage_chroot_devices(root: &Path) -> Result<Vec<PathBuf>> {
    let mut created = vec![];
    for (source, target) in chroot_binds(root) {
        if !source.exists() || target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(&target).with_context(|| format!("failed to stage {}", target.display()))?;
        created.push(target);
    }
    Ok(created)
}

/// The device files to bind into the chroot, as (host, guest) pairs.
fn chroot_binds(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    CHROOT_DEVICES
        .iter()
        .map(|device| {
            (
                PathBuf::from(device),
                root.join(device.trim_start_matches('/')),
            )
        })
        .collect()
}

/// The chroot's environment. Built from scratch rather than inherited: the
/// host's `HOME` does not exist in the guest, and a script that installs rustup
/// into `$HOME/.cargo` and then links `/root/.cargo/bin` would leave dangling
/// links. Proxies are the one thing worth carrying over.
fn chroot_env_from(host: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        (
            "PATH",
            "/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        ),
        ("HOME", "/root"),
        ("USER", "root"),
        ("LOGNAME", "root"),
        ("SHELL", "/bin/sh"),
        ("TMPDIR", "/tmp"),
        ("TERM", "dumb"),
        ("LC_ALL", "C.UTF-8"),
        ("DEBIAN_FRONTEND", "noninteractive"),
    ]
    .iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    for name in [
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Some(value) = host.get(name) {
            env.push((name.to_string(), value.clone()));
        }
    }
    env
}

fn image_customize_command(exe: &Path, unshare: &Path, root: &Path) -> Command {
    let mut command = Command::new(unshare);
    command
        // A PID namespace as well as a mount one: the kernel only lets an
        // unprivileged namespace mount a fresh procfs when it owns a PID
        // namespace, and the chroot's processes have no business in the host's.
        .args([
            "--mount",
            "--pid",
            "--fork",
            "--propagation",
            "private",
            "--",
        ])
        .arg(exe)
        .args(["image", "customize-inner", "--root"])
        .arg(root);
    command
}

/// Run the customization script in a mount namespace of its own, so the mounts
/// the chroot needs are gone by the time the image is written.
fn image_customize(root: &Path) -> Result<()> {
    let exe = env::current_exe().context("failed to find zellij-firecracker")?;
    let unshare = find_in_path(Path::new("unshare")).context("unshare is not installed")?;
    run(
        &mut image_customize_command(&exe, &unshare, root),
        "Firecracker customization script failed",
    )
}

fn image_customize_inner(args: &[String]) -> Result<()> {
    let root = option_path(args, "--root")?.context("image customize-inner requires --root")?;
    mount_proc(&root)?;
    // Only so dpkg can open a pty for its terminal log; a host that refuses is
    // not a reason to fail the build.
    let _ = mount_devpts(&root);
    for (source, target) in chroot_binds(&root) {
        bind_file(&source, &target)?;
    }
    let chroot = find_in_path(Path::new("chroot")).context("chroot is not installed")?;
    let mut command = Command::new(chroot);
    command
        .arg(&root)
        .args(["/bin/sh", GUEST_CUSTOMIZE_SCRIPT])
        // Nothing in the guest may prompt: a maintainer script that reads from
        // the terminal would hang the build forever.
        .stdin(Stdio::null())
        .env_clear();
    for (key, value) in chroot_env_from(&env::vars().collect()) {
        command.env(key, value);
    }
    run(&mut command, "the customization script failed")
    // No unmount: this process exits and its mount namespace goes with it.
}

/// A fresh procfs, not a bind of the host's: binding a mount with locked
/// submounts is refused inside the builder's user namespace.
fn mount_proc(root: &Path) -> Result<()> {
    let target = root.join("proc");
    fs::create_dir_all(&target)?;
    mount_at(Path::new("proc"), &target, Some("proc"), 0)
        .with_context(|| format!("failed to mount proc at {}", target.display()))
}

fn mount_devpts(root: &Path) -> Result<()> {
    let target = root.join("dev/pts");
    fs::create_dir_all(&target)?;
    mount_at(Path::new("devpts"), &target, Some("devpts"), 0)
        .with_context(|| format!("failed to mount devpts at {}", target.display()))
}

fn bind_file(source: &Path, target: &Path) -> Result<()> {
    // The parent staged the targets, so a missing one means the host has no
    // such device and there is nothing to bind.
    if !source.exists() || !target.exists() {
        return Ok(());
    }
    mount_at(source, target, None, libc::MS_BIND)
        .with_context(|| format!("failed to bind {} into the image", source.display()))
}

fn mount_at(
    source: &Path,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
) -> Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    let fstype = fstype.map(CString::new).transpose()?;
    let fstype_ptr = fstype
        .as_ref()
        .map(|fstype| fstype.as_ptr())
        .unwrap_or(std::ptr::null());
    if unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype_ptr,
            flags,
            std::ptr::null(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error()).context("mount failed");
    }
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

fn network_subnet(uid: u32, explicit: Option<&str>) -> Result<String> {
    if let Some(subnet) = explicit {
        return Ok(subnet.to_owned());
    }
    if uid > u16::MAX.into() {
        return Err(anyhow!(
            "UID {uid} requires an explicit --subnet to avoid address collisions"
        ));
    }
    Ok(format!("10.{}.{}.0/24", uid >> 8, uid & 0xff))
}

fn network_setup(args: &[String]) -> Result<()> {
    require_root()?;
    let user = option_value(args, "--user")?.context("network setup requires --user USER")?;
    let uid = id_value(user, "-u")?;
    let subnet = network_subnet(uid, option_value(args, "--subnet")?)?;
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
        subnet,
        taps,
        dns,
    };
    validate_network_config(&config)?;
    write_network_unit()?;
    // The recorded configuration is written only once the host actually matches it. A run that
    // dies half way must not leave behind a file describing a state that was never applied: the
    // next run would then clean up by the wrong subnet and wedge the host.
    let undo = network_apply(uid, &config)?;
    if let Err(error) = record_network_config(uid, &config).and_then(|()| {
        run(
            Command::new("systemctl").args([
                "enable",
                &format!("zellij-firecracker-network@{uid}.service"),
            ]),
            "failed to enable Firecracker networking at boot",
        )
    }) {
        rollback(uid, &config, undo);
        return Err(error);
    }
    println!("Configured {} TAPs on {} for {user}.", taps, config.bridge);
    Ok(())
}

fn record_network_config(uid: u32, config: &NetworkConfig) -> Result<()> {
    fs::create_dir_all("/etc/zellij-firecracker")?;
    fs::write(network_config_path(uid), serde_json::to_vec_pretty(config)?)
        .context("failed to record the Firecracker network configuration")
}

/// The configuration a UID gets without one recorded on disk: everything but the subnet is
/// derived from the UID, which is what lets `teardown` clean up after a run that never got as
/// far as writing its config file.
fn default_network_config(uid: u32) -> Result<NetworkConfig> {
    Ok(NetworkConfig {
        bridge: format!("zfc{uid}"),
        tap_prefix: format!("zft{uid}-"),
        subnet: network_subnet(uid, None)?,
        taps: DEFAULT_TAPS,
        dns: "1.1.1.1".parse()?,
    })
}

fn network_apply_cli(args: &[String]) -> Result<()> {
    require_root()?;
    let uid: u32 = option_value(args, "--uid")?
        .context("network apply requires --uid")?
        .parse()?;
    let path = option_path(args, "--config")?.unwrap_or_else(|| network_config_path(uid));
    let config: NetworkConfig = serde_json::from_slice(&fs::read(&path)?)?;
    validate_network_config(&config)?;
    network_apply(uid, &config).map(|_| ())
}

fn network_remove_cli(args: &[String]) -> Result<()> {
    require_root()?;
    let path = option_path(args, "--config")?.context("network remove requires --config PATH")?;
    // `--uid` names whose recorded network the subnet cleanup may consult, as it does for
    // `apply`; under sudo the effective uid is root's, which recorded nothing.
    let uid = match option_value(args, "--uid")? {
        Some(uid) => uid
            .parse()
            .context("network remove --uid must be a number")?,
        None => unsafe { libc::geteuid() },
    };
    let config: NetworkConfig = serde_json::from_slice(&fs::read(&path)?)?;
    validate_network_config(&config)?;
    network_remove(uid, &config, FirewallScope::ThisNetwork);
    Ok(())
}

/// What a partially applied run has to undo, in the order it must be undone.
///
/// Only resources *this* run created appear here. A bridge or TAP that was already up belongs to
/// whatever created it - deleting it would cut off VMs another session is running - so a failed
/// re-run leaves it exactly as it found it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Undo {
    Firewall,
    Interface(String),
}

fn undo_steps(created_bridge: Option<String>, created_taps: Vec<String>) -> Vec<Undo> {
    let mut steps = vec![Undo::Firewall];
    steps.extend(created_taps.into_iter().rev().map(Undo::Interface));
    steps.extend(created_bridge.map(Undo::Interface));
    steps
}

fn rollback(uid: u32, config: &NetworkConfig, steps: Vec<Undo>) {
    for step in steps {
        match step {
            Undo::Firewall => {
                let subnets = known_subnets(uid, config);
                if let Err(error) = remove_firewall(&mut RealIptables, None, config, &subnets) {
                    eprintln!("warning: could not undo the Firecracker firewall rules: {error:#}");
                }
            },
            Undo::Interface(name) => {
                let status = Command::new("ip")
                    .args(["link", "delete", &name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                if !matches!(status, Ok(status) if status.success()) {
                    eprintln!("warning: could not remove {name}; run 'sudo ip link delete {name}'");
                }
            },
        }
    }
}

fn network_apply(uid: u32, config: &NetworkConfig) -> Result<Vec<Undo>> {
    let mut created_bridge = None;
    let mut created_taps = Vec::new();
    let result = network_apply_inner(uid, config, &mut created_bridge, &mut created_taps);
    let steps = undo_steps(created_bridge, created_taps);
    match result {
        Ok(()) => Ok(steps),
        Err(error) => {
            rollback(uid, config, steps);
            Err(error)
        },
    }
}

fn network_apply_inner(
    uid: u32,
    config: &NetworkConfig,
    created_bridge: &mut Option<String>,
    created_taps: &mut Vec<String>,
) -> Result<()> {
    let (base, _) = parse_subnet(&config.subnet)?;
    let gateway = ipv4_add(base, 1)?;
    fs::write("/proc/sys/net/ipv4/ip_forward", "1")?;
    if !interface_exists(&config.bridge) {
        run(
            Command::new("ip").args(["link", "add", &config.bridge, "type", "bridge"]),
            "failed to create the Firecracker bridge",
        )?;
        *created_bridge = Some(config.bridge.clone());
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
            created_taps.push(tap.clone());
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
    apply_firewall(&mut RealIptables, uid, config, &known_subnets(uid, config))
}

/// One `iptables` invocation. Injected so the firewall logic - which is where every idempotency
/// bug has lived - is unit testable against a fake table, without root.
trait Iptables {
    /// Runs `iptables -w <args>`.
    fn run(&mut self, args: &[String]) -> Result<IptablesOutput>;
}

struct IptablesOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

struct RealIptables;

impl Iptables for RealIptables {
    fn run(&mut self, args: &[String]) -> Result<IptablesOutput> {
        let output = Command::new("iptables")
            .arg("-w")
            .args(args)
            .output()
            .context("failed to run iptables")?;
        Ok(IptablesOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn iptables_must(ipt: &mut dyn Iptables, args: &[String], context: &str) -> Result<()> {
    let output = ipt.run(args)?;
    if output.success {
        return Ok(());
    }
    Err(anyhow!(
        "{context}: iptables {}: {}",
        args.join(" "),
        output.stderr.trim()
    ))
}

fn firewall_chain(config: &NetworkConfig) -> String {
    format!(
        "ZFC_{}",
        config.bridge.replace('-', "_").to_ascii_uppercase()
    )
}

const FIREWALL_TAG_PREFIX: &str = "zellij-firecracker:";

fn firewall_tag(uid: u32) -> String {
    format!("{FIREWALL_TAG_PREFIX}{uid}")
}

/// The rules inside our own chain. They need no tag: the chain name identifies them, and the
/// chain is flushed as a whole.
fn firewall_chain_rules(config: &NetworkConfig) -> Vec<Vec<String>> {
    let chain = firewall_chain(config);
    let mut rules = Vec::new();
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
    rules
}

/// A rule this provider owns in one of the built-in chains, carrying the tag that lets cleanup
/// recognise it again whatever the subnet, DNS server or TAP count was when it was written.
struct BaseRule {
    table: Option<&'static str>,
    chain: &'static str,
    spec: Vec<String>,
}

impl BaseRule {
    fn args(&self, verb: &str, position: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(table) = self.table {
            args.extend(strings(&["-t", table]));
        }
        args.push(verb.to_owned());
        args.push(self.chain.to_owned());
        if let Some(position) = position {
            args.push(position.to_owned());
        }
        args.extend(self.spec.iter().cloned());
        args
    }

    fn insert_args(&self) -> Vec<String> {
        self.args("-I", Some("1"))
    }

    fn check_args(&self) -> Vec<String> {
        self.args("-C", None)
    }
}

fn tagged(uid: u32, matches: &[&str], target: &str) -> Vec<String> {
    let mut spec = strings(matches);
    spec.extend(strings(&["-m", "comment", "--comment", &firewall_tag(uid)]));
    spec.extend(strings(&["-j", target]));
    spec
}

/// The rules we insert into INPUT, FORWARD and nat POSTROUTING, in insertion order. Each goes in
/// at position 1, so the later ones end up above the earlier ones.
fn firewall_base_rules(uid: u32, config: &NetworkConfig) -> Vec<BaseRule> {
    let chain = firewall_chain(config);
    vec![
        BaseRule {
            table: None,
            chain: "INPUT",
            spec: tagged(uid, &["-i", &config.bridge, "-s", &config.subnet], "DROP"),
        },
        BaseRule {
            table: None,
            chain: "INPUT",
            spec: tagged(
                uid,
                &[
                    "-i",
                    &config.bridge,
                    "-s",
                    &config.subnet,
                    "-m",
                    "conntrack",
                    "--ctstate",
                    "ESTABLISHED,RELATED",
                ],
                "ACCEPT",
            ),
        },
        BaseRule {
            table: None,
            chain: "FORWARD",
            spec: tagged(uid, &["-i", &config.bridge, "-s", &config.subnet], &chain),
        },
        BaseRule {
            table: None,
            chain: "FORWARD",
            spec: tagged(
                uid,
                &[
                    "-o",
                    &config.bridge,
                    "-d",
                    &config.subnet,
                    "-m",
                    "conntrack",
                    "--ctstate",
                    "ESTABLISHED,RELATED",
                ],
                "ACCEPT",
            ),
        },
        BaseRule {
            table: Some("nat"),
            chain: "POSTROUTING",
            spec: tagged(uid, &["-s", &config.subnet], "MASQUERADE"),
        },
    ]
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Brings the firewall to the configured state from whatever state it is in.
///
/// Everything of ours is swept out first, then written fresh, so re-running this - which the boot
/// unit does on every boot - converges instead of accumulating or colliding.
fn apply_firewall(
    ipt: &mut dyn Iptables,
    uid: u32,
    config: &NetworkConfig,
    subnets: &[String],
) -> Result<()> {
    // Only rules of this network are swept: a rule that carries this user's tag but names
    // another bridge belongs to another network of theirs - the real pool, while a test run
    // applies a network of its own - and stays.
    let cleanup = remove_firewall(ipt, None, config, subnets)?;
    cleanup.report();
    let chain = firewall_chain(config);
    if !chain_exists(&read_rules(ipt, None)?, &chain) {
        iptables_must(
            ipt,
            &strings(&["-N", &chain]),
            "failed to create the Firecracker firewall chain",
        )?;
    } else {
        iptables_must(
            ipt,
            &strings(&["-F", &chain]),
            "failed to flush the Firecracker firewall chain",
        )?;
    }
    for rule in firewall_chain_rules(config) {
        iptables_must(ipt, &rule, "failed to configure Firecracker firewall rules")?;
    }
    for rule in firewall_base_rules(uid, config) {
        if ipt.run(&rule.check_args())?.success {
            continue;
        }
        iptables_must(
            ipt,
            &rule.insert_args(),
            "failed to configure Firecracker firewall rules",
        )?;
    }
    Ok(())
}

/// What a sweep could not do on its own.
#[derive(Debug, Default, PartialEq, Eq)]
struct FirewallCleanup {
    /// NAT rules that masquerade a subnet we no longer know about. They carry neither our tag nor
    /// our bridge name, so nothing proves they are ours and we refuse to guess.
    orphans: Vec<String>,
}

impl FirewallCleanup {
    fn report(&self) {
        for orphan in &self.orphans {
            eprintln!(
                "warning: a NAT rule for a subnet this host no longer uses was left in place \
                 because it cannot be proven to be ours; remove it with 'sudo iptables -w -t nat {}'",
                orphan.replacen("-A", "-D", 1)
            );
        }
    }
}

/// Removes every firewall rule this provider owns, whatever it was configured with when the rule
/// was written.
///
/// Rules are found in the live table rather than reconstructed from the current configuration,
/// because the two drift: a rule written for one subnet cannot be deleted by a spec naming
/// another, and the leftover jump then keeps the chain alive so that re-creating it fails with
/// "Chain already exists".
/// Remove the rules of `config`'s network: those naming its bridge or chain, and the
/// MASQUERADE for any of `subnets`. With a `tag`, every rule carrying it goes too, whatever it
/// names - the teardown of a user's whole setup, not the reconfiguration of one network.
fn remove_firewall(
    ipt: &mut dyn Iptables,
    tag: Option<&str>,
    config: &NetworkConfig,
    subnets: &[String],
) -> Result<FirewallCleanup> {
    let chain = firewall_chain(config);
    // Each -D removes one match, so duplicates need more than one pass; the bound just stops a
    // pathological table from looping forever.
    for _ in 0..32 {
        let ours = our_rules(
            &read_rules(ipt, None)?,
            &read_rules(ipt, Some("nat"))?,
            &chain,
            &config.bridge,
            tag,
            subnets,
        );
        if ours.is_empty() {
            break;
        }
        for rule in &ours {
            let output = ipt.run(&rule.delete_args())?;
            if !output.success {
                return Err(anyhow!(
                    "failed to remove the Firecracker firewall rule {:?}: {}",
                    rule.line,
                    output.stderr.trim()
                ));
            }
        }
    }
    let filter = read_rules(ipt, None)?;
    let nat = read_rules(ipt, Some("nat"))?;
    let leftover = our_rules(&filter, &nat, &chain, &config.bridge, tag, subnets);
    if !leftover.is_empty() {
        return Err(anyhow!(
            "could not remove these Firecracker firewall rules: {}",
            leftover
                .iter()
                .map(|rule| rule.line.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if chain_exists(&filter, &chain) {
        let _ = ipt.run(&strings(&["-F", &chain]))?;
        let output = ipt.run(&strings(&["-X", &chain]))?;
        if !output.success && chain_exists(&read_rules(ipt, None)?, &chain) {
            return Err(anyhow!(
                "could not remove the Firecracker firewall chain {chain}: {}",
                output.stderr.trim()
            ));
        }
    }
    Ok(FirewallCleanup {
        orphans: orphan_masquerades(&nat, subnets),
    })
}

/// The subnets a rule of ours may name: the one we are configuring, and the one last recorded for
/// this UID, which is what a run that changes the subnet has to clean up after. The recorded one
/// counts only when it describes the same network - a run applying a network of its own, as the
/// test harness does, must not take the recorded pool's NAT rule with it.
fn known_subnets(uid: u32, config: &NetworkConfig) -> Vec<String> {
    let recorded = fs::read(network_config_path(uid))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<NetworkConfig>(&bytes).ok());
    known_subnets_with(config, recorded.as_ref())
}

fn known_subnets_with(config: &NetworkConfig, recorded: Option<&NetworkConfig>) -> Vec<String> {
    let mut subnets = vec![config.subnet.clone()];
    if let Some(recorded) = recorded {
        if recorded.bridge == config.bridge && !subnets.contains(&recorded.subnet) {
            subnets.push(recorded.subnet.clone());
        }
    }
    subnets
}

fn read_rules(ipt: &mut dyn Iptables, table: Option<&str>) -> Result<Vec<String>> {
    let mut args = Vec::new();
    if let Some(table) = table {
        args.extend(strings(&["-t", table]));
    }
    args.push("-S".to_owned());
    let output = ipt.run(&args)?;
    if !output.success {
        return Err(anyhow!(
            "failed to read the iptables {} table: {}",
            table.unwrap_or("filter"),
            output.stderr.trim()
        ));
    }
    Ok(output.stdout.lines().map(|line| line.to_owned()).collect())
}

fn chain_exists(filter: &[String], chain: &str) -> bool {
    filter
        .iter()
        .any(|line| split_rule(line) == strings(&["-N", chain]))
}

/// A rule of ours found in the live table, ready to be deleted by the exact spec iptables printed.
#[derive(Debug, PartialEq, Eq)]
struct OwnedRule {
    line: String,
    argv: Vec<String>,
    table: Option<&'static str>,
}

impl OwnedRule {
    fn delete_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(table) = self.table {
            args.extend(strings(&["-t", table]));
        }
        args.push("-D".to_owned());
        args.extend(self.argv.iter().skip(1).cloned());
        args
    }
}

fn our_rules(
    filter: &[String],
    nat: &[String],
    chain: &str,
    bridge: &str,
    tag: Option<&str>,
    subnets: &[String],
) -> Vec<OwnedRule> {
    let mut rules = Vec::new();
    for (table, lines) in [(None, filter), (Some("nat"), nat)] {
        for line in lines {
            let argv = split_rule(line);
            if argv.first().map(String::as_str) != Some("-A") {
                continue;
            }
            // Rules inside our own chain go with the chain when it is flushed.
            if argv.get(1).map(String::as_str) == Some(chain) {
                continue;
            }
            if !rule_is_ours(&argv, chain, bridge, tag, subnets) {
                continue;
            }
            rules.push(OwnedRule {
                line: line.clone(),
                argv,
                table,
            });
        }
    }
    rules
}

fn rule_is_ours(
    argv: &[String],
    chain: &str,
    bridge: &str,
    tag: Option<&str>,
    subnets: &[String],
) -> bool {
    let follows = |flag: &str, value: &str| {
        argv.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    };
    tag.is_some_and(|tag| follows("--comment", tag))
        || follows("-j", chain)
        || follows("-i", bridge)
        || follows("-o", bridge)
        || (argv.get(1).map(String::as_str) == Some("POSTROUTING")
            && follows("-j", "MASQUERADE")
            && subnets.iter().any(|subnet| follows("-s", subnet)))
}

/// Bare `POSTROUTING -s <subnet> -j MASQUERADE` rules for a subnet we do not recognise.
///
/// A NAT rule names no interface, so once the subnet it was written for is forgotten there is
/// nothing left to identify it by. The shape test - a source and nothing else - keeps Docker's and
/// Tailscale's rules, which all carry an interface or a mark match, out of the report.
fn orphan_masquerades(nat: &[String], subnets: &[String]) -> Vec<String> {
    nat.iter()
        .filter(|line| {
            let argv = split_rule(line);
            argv.len() == 6
                && argv[0] == "-A"
                && argv[1] == "POSTROUTING"
                && argv[2] == "-s"
                && argv[4..] == strings(&["-j", "MASQUERADE"])[..]
                && !subnets.contains(&argv[3])
        })
        .cloned()
        .collect()
}

/// Splits a line of `iptables -S` back into arguments. Only comments are quoted, and iptables
/// escapes a quote or a backslash inside one with a backslash.
fn split_rule(line: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => {
                quoted = !quoted;
                started = true;
            },
            character if character.is_ascii_whitespace() && !quoted => {
                if started {
                    argv.push(std::mem::take(&mut word));
                    started = false;
                }
            },
            character => {
                word.push(character);
                started = true;
            },
        }
    }
    if started {
        argv.push(word);
    }
    argv
}

/// The `ip` commands that undo `network_apply_inner`'s interfaces: every TAP of the pool,
/// then the bridge. Deleting the bridge alone detaches the TAPs and leaves them behind.
fn network_remove_commands(config: &NetworkConfig) -> Vec<Vec<String>> {
    (0..config.taps)
        .map(|slot| strings(&["link", "delete", &format!("{}{slot:02}", config.tap_prefix)]))
        .chain([strings(&["link", "delete", &config.bridge])])
        .collect()
}

/// How far `network_remove` reaches into the firewall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirewallScope {
    /// The rules of the network `config` describes.
    ThisNetwork,
    /// Every rule this user's provider ever tagged, whatever network it was for.
    EverythingTagged,
}

fn network_remove(uid: u32, config: &NetworkConfig, scope: FirewallScope) {
    let tag = firewall_tag(uid);
    let tag = match scope {
        FirewallScope::ThisNetwork => None,
        FirewallScope::EverythingTagged => Some(tag.as_str()),
    };
    match remove_firewall(&mut RealIptables, tag, config, &known_subnets(uid, config)) {
        Ok(cleanup) => cleanup.report(),
        Err(error) => eprintln!("warning: {error:#}"),
    }
    for args in network_remove_commands(config) {
        let _ = Command::new("ip")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn network_teardown(args: &[String]) -> Result<()> {
    require_root()?;
    let user = option_value(args, "--user")?.context("network teardown requires --user USER")?;
    let uid = id_value(user, "-u")?;
    let path = network_config_path(uid);
    // Teardown is the recovery path for a host a failed setup left half configured, so a missing
    // or unreadable config file must not stop it: everything but the subnet follows from the UID.
    let config = match fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<NetworkConfig>(&bytes).ok())
    {
        Some(config) => config,
        None => default_network_config(uid)?,
    };
    validate_network_config(&config)?;
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
    network_remove(uid, &config, FirewallScope::EverythingTagged);
    let _ = fs::remove_file(path);
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

/// `run`, with `input` on the command's stdin.
fn run_with_input(command: &mut Command, input: &[u8], context: &str) -> Result<()> {
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| context.to_owned())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .with_context(|| format!("{context}: failed to write its input"))?;
    }
    let status = child.wait().with_context(|| context.to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{context}: {status}"))
    }
}

/// Says what the provider is doing, for the server's inactivity timeout.
///
/// One line on stderr as the phase starts and another every few seconds until the guard is
/// dropped, so a slow boot or a large copy is never mistaken for a hung provider. Anything
/// else the provider writes to stderr is an error message; the prefix keeps the two apart on
/// the other side.
struct Progress {
    stopped: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Progress {
    const INTERVAL: Duration = Duration::from_secs(5);

    fn start(phase: &str) -> Self {
        Self::start_into(phase, Box::new(std::io::stderr()), Self::INTERVAL)
    }

    fn start_into(phase: &str, mut sink: Box<dyn Write + Send>, interval: Duration) -> Self {
        let line = format!("{PROGRESS_PREFIX}{phase}\n");
        let stopped = Arc::new((Mutex::new(false), Condvar::new()));
        let thread = thread::spawn({
            let stopped = stopped.clone();
            move || {
                let (flag, wake) = &*stopped;
                let mut done = flag.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*done {
                    let _ = sink.write_all(line.as_bytes()).and_then(|_| sink.flush());
                    done = wake
                        .wait_timeout_while(done, interval, |done| !*done)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0;
                }
            }
        });
        Self {
            stopped,
            thread: Some(thread),
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stopped;
        *flag.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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

    const RECORDED_IMAGE: &str = "/images/zellij-dev.ext4";

    struct RecordingSetup {
        stages: Vec<SetupStage>,
        fail: Option<SetupStage>,
        /// The image the config stage was handed, so a test can check that it is the one
        /// the image stage produced.
        config_image: Option<PathBuf>,
    }

    impl RecordingSetup {
        fn new(fail: Option<SetupStage>) -> Self {
            Self {
                stages: vec![],
                fail,
                config_image: None,
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
        ) -> Result<PathBuf> {
            self.stage(SetupStage::Image)?;
            Ok(PathBuf::from(RECORDED_IMAGE))
        }

        fn config(&mut self, _provider: &Path, image: &Path) -> Result<()> {
            self.config_image = Some(image.to_owned());
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
            require_resource_limits: default_require_resource_limits(),
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
    fn setup_hands_the_config_stage_the_image_it_used() {
        let mut runtime = RecordingSetup::new(None);
        setup_with(&setup_options(false), &mut runtime).unwrap();
        assert_eq!(runtime.config_image, Some(PathBuf::from(RECORDED_IMAGE)));
    }

    #[test]
    fn setup_reuses_valid_images_and_rebuilds_with_exact_arguments() {
        let project = Path::new("/workspace/project");
        let zellij = Path::new("/build/zellij");
        let provider = Path::new("/build/zellij-firecracker");
        let mut commands = vec![];
        let reused = setup_image_with(
            &setup_options(false),
            project,
            zellij,
            provider,
            |image| Ok(PathBuf::from(format!("/images/{image}.ext4"))),
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
        // The reused image is reported just as a built one is: it is what the config step
        // makes this machine's default.
        assert_eq!(reused, PathBuf::from("/images/zellij-dev.ext4"));

        let built = setup_image_with(
            &setup_options(true),
            project,
            zellij,
            provider,
            |image| Ok(PathBuf::from(format!("/images/{image}.ext4"))),
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
        assert_eq!(built, PathBuf::from("/images/zellij-dev.ext4"));
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
    fn setup_reports_why_an_image_is_unusable_and_builds_once_to_fix_it() {
        let project = Path::new("/workspace/project");
        let zellij = Path::new("/build/zellij");
        let provider = Path::new("/build/zellij-firecracker");

        // The first probe fails, the build fixes it: one build, no error.
        let mut probes = 0;
        let mut builds = 0;
        setup_image_with(
            &setup_options(false),
            project,
            zellij,
            provider,
            |image| {
                probes += 1;
                (probes > 1)
                    .then(|| PathBuf::from(format!("/images/{image}.ext4")))
                    .context("no such image")
            },
            |_, _| {
                builds += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!((probes, builds), (2, 1));

        // A build that leaves the image unusable says why, rather than hiding the cause.
        let error = setup_image_with(
            &setup_options(false),
            project,
            zellij,
            provider,
            |_| {
                Err(anyhow!(
                    "provider config kernel must be an existing absolute file"
                ))
            },
            |_, _| Ok(()),
        )
        .unwrap_err();
        let report = format!("{error:#}");
        assert!(report.contains("was not created correctly"), "{report}");
        assert!(report.contains("provider config kernel"), "{report}");
    }

    #[test]
    fn a_dangling_default_image_does_not_hide_a_named_one() {
        let root = tempfile::tempdir().unwrap();
        let mut config = provider_config(root.path());
        for path in [&config.firecracker, &config.kernel, &config.ssh_private_key] {
            fs::write(path, "file").unwrap();
        }
        fs::create_dir_all(&config.images_dir).unwrap();
        fs::write(config.images_dir.join("zellij-dev.ext4"), "image").unwrap();
        // The default image is gone, as it is on a machine that only ever built named ones.
        assert!(!config.rootfs.exists());

        validate_config(&config).unwrap();
        assert_eq!(
            image_path(&config, Some("zellij-dev")).unwrap(),
            config.images_dir.join("zellij-dev.ext4")
        );
        // A VM that names no image still gets a precise error where it is actually used.
        assert!(image_path(&config, None).is_err());

        config.rootfs = PathBuf::from("relative.ext4");
        assert!(validate_config(&config).is_err());
        config = provider_config(root.path());
        fs::remove_file(&config.kernel).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn a_named_build_adopts_the_default_image_only_when_there_is_none() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("ubuntu-24.04.ext4");
        let built = root.path().join("images/zellij-dev.ext4");

        assert_eq!(default_rootfs(&existing, &built, true), built);
        assert_eq!(default_rootfs(&existing, &built, false), built);
        fs::write(&existing, "image").unwrap();
        assert_eq!(default_rootfs(&existing, &built, true), existing);
        assert_eq!(default_rootfs(&existing, &built, false), built);
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
    fn setup_repoints_a_dangling_default_at_the_image_it_reused() {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("firecracker.json");
        let mut config = provider_config(root.path());
        // The bug: a default left over from an earlier layout, naming a file that is gone.
        config.rootfs = root.path().join("ubuntu-24.04.ext4");
        config.credential_files.push(CredentialFile {
            host: PathBuf::from("~/.codex/auth.json"),
            guest: PathBuf::from("/root/.codex/auth.json"),
        });
        config.extra.insert(
            "future_setting".to_owned(),
            serde_json::json!({"enabled": true}),
        );
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let image = root.path().join("images/zellij-dev.ext4");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        fs::write(&image, "image").unwrap();

        assert!(set_default_image_at(&config_path, &image).unwrap());

        let updated: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(
            updated["rootfs"],
            serde_json::json!(image.to_str().unwrap())
        );
        assert_eq!(
            updated["future_setting"],
            serde_json::json!({"enabled": true})
        );
        assert_eq!(updated["credential_files"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn repointing_the_default_image_writes_only_when_it_moves() {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("firecracker.json");
        let image = root.path().join("images/zellij-dev.ext4");
        let mut config = provider_config(root.path());
        config.rootfs = image.clone();
        // Deliberately not the bytes `to_vec_pretty` produces: a rewrite would normalise the
        // trailing whitespace, so identical bytes afterwards prove nothing was written.
        let mut bytes = serde_json::to_vec_pretty(&config).unwrap();
        bytes.extend_from_slice(b"\n\n");
        fs::write(&config_path, &bytes).unwrap();

        assert!(!set_default_image_at(&config_path, &image).unwrap());

        assert_eq!(fs::read(&config_path).unwrap(), bytes);
    }

    #[test]
    fn a_missing_provider_config_fails_the_repoint_instead_of_creating_one() {
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("firecracker.json");
        let error = set_default_image_at(&config_path, Path::new("/images/zellij-dev.ext4"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to read"), "{error}");
        assert!(!config_path.exists());
    }

    #[test]
    fn doctor_requires_the_default_image_and_the_images_directory() {
        let root = tempfile::tempdir().unwrap();
        let mut config = provider_config(root.path());
        config.rootfs = root.path().join("images/zellij-dev.ext4");
        let error = check_image_paths(&config).unwrap_err().to_string();
        assert!(error.contains("images directory"), "{error}");

        fs::create_dir_all(root.path().join("images")).unwrap();
        let error = check_image_paths(&config).unwrap_err().to_string();
        assert!(error.contains("default Firecracker image"), "{error}");
        assert!(error.contains("run zellij-firecracker setup"), "{error}");

        fs::write(&config.rootfs, "image").unwrap();
        check_image_paths(&config).unwrap();
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
        assert_eq!(vm_id("work/../../other", "review"), "9cc0eea7664ed86b");
        assert_ne!(vm_id("work", "review"), vm_id("other", "review"));
        assert!(!vm_id("work/../../other", "review").contains('/'));
    }

    #[test]
    fn default_network_subnets_are_uid_specific() {
        assert_eq!(network_subnet(1, None).unwrap(), "10.0.1.0/24");
        assert_eq!(network_subnet(256, None).unwrap(), "10.1.0.0/24");
        assert_eq!(network_subnet(65_535, None).unwrap(), "10.255.255.0/24");
        assert!(network_subnet(65_536, None).is_err());
        assert_eq!(
            network_subnet(65_536, Some("172.31.0.0/24")).unwrap(),
            "172.31.0.0/24"
        );
    }

    /// A minimal ELF64 with one `PT_INTERP` header, as a linker would write it.
    fn elf_with_interpreter(interpreter: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2; // 64-bit
        bytes[5] = 1; // little-endian
        bytes[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        bytes[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        bytes[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let mut header = vec![0u8; 56];
        header[..4].copy_from_slice(&3u32.to_le_bytes()); // PT_INTERP
        let interp = format!("{interpreter}\0");
        header[8..16].copy_from_slice(&120u64.to_le_bytes()); // p_offset
        header[0x20..0x28].copy_from_slice(&(interp.len() as u64).to_le_bytes()); // p_filesz
        bytes.extend(header);
        bytes.extend(interp.as_bytes());
        bytes
    }

    #[test]
    fn elf_interpreter_reads_pt_interp_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let dynamic = dir.path().join("dynamic");
        fs::write(
            &dynamic,
            elf_with_interpreter("/lib64/ld-linux-x86-64.so.2"),
        )
        .unwrap();
        assert_eq!(
            elf_interpreter(&dynamic).unwrap(),
            Some(PathBuf::from("/lib64/ld-linux-x86-64.so.2"))
        );

        let mut static_elf = elf_with_interpreter("/lib64/ld-linux-x86-64.so.2");
        static_elf[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD instead
        let static_path = dir.path().join("static");
        fs::write(&static_path, static_elf).unwrap();
        assert_eq!(elf_interpreter(&static_path).unwrap(), None);

        let script = dir.path().join("script");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        assert_eq!(elf_interpreter(&script).unwrap(), None);
        assert!(elf_interpreter(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn terminating_an_owned_child_reaps_it_without_the_grace_period() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let mut state = vm_state(PathBuf::new(), PathBuf::new(), None);
        state.pid = child.id() as i32;
        state.start_time = wait_for_process_start(state.pid, Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        terminate(&state, Some(&mut child));

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }

    fn locked_state_in(root: &Path) -> LockedState {
        LockedState {
            dir: root.to_owned(),
            _lock: File::create(root.join("lock")).unwrap(),
        }
    }

    /// A session-identified VM moves to its new path; a worktree-identified one keeps its
    /// path and only its lease changes, with other sessions' leases untouched.
    #[test]
    fn rename_session_moves_state_files_and_leases() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let owned = vm_state(root.path().join("a/rootfs.ext4"), PathBuf::new(), None);
        write_state(&locked.path("work", "review"), &owned).unwrap();
        let mut shared = vm_state(root.path().join("b/rootfs.ext4"), PathBuf::new(), None);
        shared.name = "shared".to_owned();
        shared.worktree = Some(PathBuf::from("/repo"));
        shared.leases = vec!["other".to_owned(), "work".to_owned()];
        let shared_path = locked.identity_path("work", "shared", Some(Path::new("/repo")));
        write_state(&shared_path, &shared).unwrap();

        rename_session(&locked, "work", "renamed").unwrap();

        assert!(!locked.path("work", "review").exists());
        let moved = read_state(&locked.path("renamed", "review")).unwrap();
        assert_eq!(moved.session, "renamed");
        assert_eq!(moved.leases, vec!["renamed".to_owned()]);
        assert_eq!(moved.rootfs, owned.rootfs, "the disk stays where it is");
        let leased = read_state(&shared_path).unwrap();
        assert_eq!(leased.session, "renamed");
        assert_eq!(
            leased.leases,
            vec!["other".to_owned(), "renamed".to_owned()]
        );

        rename_session(&locked, "renamed", "renamed").unwrap();
        assert!(locked.path("renamed", "review").exists());
    }

    /// The new name already owning a VM of the same name refuses the whole rename before
    /// any file is touched.
    #[test]
    fn rename_session_refuses_a_name_collision_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let ours = vm_state(PathBuf::from("a/rootfs.ext4"), PathBuf::new(), None);
        write_state(&locked.path("work", "review"), &ours).unwrap();
        let mut theirs = ours.clone();
        theirs.session = "renamed".to_owned();
        theirs.leases = vec!["renamed".to_owned()];
        write_state(&locked.path("renamed", "review"), &theirs).unwrap();
        let mut leased = ours.clone();
        leased.name = "shared".to_owned();
        leased.worktree = Some(PathBuf::from("/repo"));
        let leased_path = locked.identity_path("work", "shared", Some(Path::new("/repo")));
        write_state(&leased_path, &leased).unwrap();

        assert!(rename_session(&locked, "work", "renamed").is_err());

        assert_eq!(read_state(&locked.path("work", "review")).unwrap(), ours);
        assert_eq!(
            read_state(&locked.path("renamed", "review")).unwrap(),
            theirs
        );
        assert_eq!(read_state(&leased_path).unwrap(), leased);
    }

    #[test]
    fn vm_identity_rejects_ambiguous_session_names() {
        let mut state = vm_state(PathBuf::from("rootfs.ext4"), PathBuf::from("vm.sock"), None);
        state.ready = true;
        let entries = vec![(PathBuf::from("first.json"), state.clone())];

        assert!(
            ensure_vm_identity_available(&entries, Path::new("second.json"), "work", "review")
                .is_err()
        );
        assert!(
            ensure_vm_identity_available(&entries, Path::new("first.json"), "work", "review")
                .is_ok()
        );
        assert!(
            ensure_vm_identity_available(&entries, Path::new("first.json"), "other", "review")
                .is_ok(),
            "another session may lease the same worktree VM"
        );

        let duplicates = vec![
            (PathBuf::from("first.json"), state.clone()),
            (PathBuf::from("second.json"), state),
        ];
        assert!(find_vm(&duplicates, "work", "review").is_err());
    }

    #[test]
    fn worktree_baseline_ignores_root_outputs() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitignore"), "target/\n").unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/output"), "one").unwrap();
        let original = worktree_baseline(root.path()).unwrap();
        assert!(original.contains_key("source.rs"));
        assert!(!original.contains_key("target/output"));

        fs::write(root.path().join("target/output"), "two").unwrap();
        assert_eq!(worktree_baseline(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_baseline_ignores_nested_outputs() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/.gitignore"), "generated/\n").unwrap();
        fs::create_dir(root.path().join("nested/generated")).unwrap();
        fs::write(root.path().join("nested/generated/output"), "one").unwrap();
        let original = worktree_baseline(root.path()).unwrap();

        fs::write(root.path().join("nested/generated/output"), "two").unwrap();
        assert_eq!(worktree_baseline(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_baseline_detects_non_ignored_host_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        let original = worktree_baseline(root.path()).unwrap();

        fs::write(root.path().join("source.rs"), "two").unwrap();
        assert_ne!(worktree_baseline(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_baseline_detects_permission_changes() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("script.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let original = worktree_baseline(root.path()).unwrap();

        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(worktree_baseline(root.path()).unwrap(), original);
    }

    #[test]
    fn worktree_baseline_ignores_git_metadata() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        let without_git = worktree_baseline(root.path()).unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/HEAD"), "ref: changed").unwrap();
        assert_eq!(worktree_baseline(root.path()).unwrap(), without_git);
    }

    /// An editor save that writes the same bytes, a `touch`, a checkout that restores a file:
    /// none of them is a change worth refusing a merge over.
    #[test]
    fn worktree_baseline_ignores_timestamps() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.rs");
        fs::write(&source, "one").unwrap();
        let original = worktree_baseline(root.path()).unwrap();

        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
            .unwrap();
        assert_eq!(worktree_baseline(root.path()).unwrap(), original);
    }

    #[test]
    fn baseline_round_trips_through_its_file() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source.rs"), "one").unwrap();
        fs::create_dir(root.path().join("dir")).unwrap();
        std::os::unix::fs::symlink("source.rs", root.path().join("link")).unwrap();
        let baseline = worktree_baseline(root.path()).unwrap();
        assert_eq!(
            baseline.keys().collect::<Vec<_>>(),
            ["dir", "link", "source.rs"]
        );

        let vm_dir = root.path().join("vm");
        fs::create_dir(&vm_dir).unwrap();
        let state = vm_state(vm_dir.join("rootfs.ext4"), PathBuf::from("vm.sock"), None);
        write_baseline(&baseline_path(&state).unwrap(), &baseline).unwrap();
        assert_eq!(read_baseline(&state).unwrap(), baseline);
    }

    /// A host worktree and the guest's copy of it, both starting from the same baseline.
    struct MergeFixture {
        host: tempfile::TempDir,
        guest: tempfile::TempDir,
        undo: tempfile::TempDir,
        baseline: Baseline,
    }

    impl MergeFixture {
        fn new(files: &[(&str, &str)]) -> Self {
            let host = tempfile::tempdir().unwrap();
            let guest = tempfile::tempdir().unwrap();
            for (path, content) in files {
                for root in [host.path(), guest.path()] {
                    let path = root.join(path);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, content).unwrap();
                }
            }
            let baseline = worktree_baseline(host.path()).unwrap();
            assert_eq!(worktree_baseline(guest.path()).unwrap(), baseline);
            Self {
                host,
                guest,
                undo: tempfile::tempdir().unwrap(),
                baseline,
            }
        }

        fn merge(&self) -> MergeReport {
            self.merge_fully().report
        }

        fn merge_fully(&self) -> Merged {
            let merged = merge_worktrees(
                &self.baseline,
                self.host.path(),
                self.guest.path(),
                self.undo.path(),
            )
            .unwrap();
            assert_eq!(
                merged.baseline,
                worktree_baseline(self.host.path()).unwrap(),
                "the derived baseline must describe the host worktree as merged"
            );
            merged
        }

        fn try_merge(&self) -> Result<Merged> {
            merge_worktrees(
                &self.baseline,
                self.host.path(),
                self.guest.path(),
                self.undo.path(),
            )
        }

        fn host_tree(&self) -> Baseline {
            worktree_baseline(self.host.path()).unwrap()
        }

        fn host_file(&self, path: &str) -> Option<String> {
            fs::read_to_string(self.host.path().join(path)).ok()
        }
    }

    #[test]
    fn merge_applies_guest_only_changes_and_deletions() {
        let fixture = MergeFixture::new(&[
            ("kept.rs", "same"),
            ("edited.rs", "before"),
            ("removed.rs", "gone"),
            ("dir/removed.rs", "gone"),
        ]);
        fs::write(fixture.guest.path().join("edited.rs"), "after").unwrap();
        fs::write(fixture.guest.path().join("added.rs"), "new").unwrap();
        fs::create_dir(fixture.guest.path().join("new-dir")).unwrap();
        fs::write(fixture.guest.path().join("new-dir/inner.rs"), "new").unwrap();
        fs::remove_file(fixture.guest.path().join("removed.rs")).unwrap();
        fs::remove_dir_all(fixture.guest.path().join("dir")).unwrap();

        let report = fixture.merge();

        assert_eq!(
            report.applied,
            [
                "added.rs",
                "dir",
                "dir/removed.rs",
                "edited.rs",
                "new-dir",
                "new-dir/inner.rs",
                "removed.rs"
            ]
        );
        assert!(report.conflicts.is_empty());
        assert_eq!(fixture.host_file("kept.rs").as_deref(), Some("same"));
        assert_eq!(fixture.host_file("edited.rs").as_deref(), Some("after"));
        assert_eq!(fixture.host_file("added.rs").as_deref(), Some("new"));
        assert_eq!(
            fixture.host_file("new-dir/inner.rs").as_deref(),
            Some("new")
        );
        assert!(!fixture.host.path().join("removed.rs").exists());
        assert!(!fixture.host.path().join("dir").exists());
        assert_eq!(
            worktree_baseline(fixture.host.path()).unwrap(),
            worktree_baseline(fixture.guest.path()).unwrap()
        );
    }

    #[test]
    fn merge_keeps_host_only_changes() {
        let fixture = MergeFixture::new(&[("edited.rs", "before"), ("removed.rs", "gone")]);
        fs::write(fixture.host.path().join("edited.rs"), "host").unwrap();
        fs::write(fixture.host.path().join("added.rs"), "host").unwrap();
        fs::remove_file(fixture.host.path().join("removed.rs")).unwrap();

        let report = fixture.merge();

        assert_eq!(report, MergeReport::default());
        assert_eq!(fixture.host_file("edited.rs").as_deref(), Some("host"));
        assert_eq!(fixture.host_file("added.rs").as_deref(), Some("host"));
        assert!(!fixture.host.path().join("removed.rs").exists());
    }

    #[test]
    fn merge_keeps_the_host_copy_of_a_conflict_and_writes_the_guest_copy_beside_it() {
        let fixture = MergeFixture::new(&[("both.rs", "before"), ("src/deep.rs", "before")]);
        fs::write(fixture.host.path().join("both.rs"), "host").unwrap();
        fs::write(fixture.guest.path().join("both.rs"), "guest").unwrap();
        fs::write(fixture.host.path().join("src/deep.rs"), "host").unwrap();
        fs::write(fixture.guest.path().join("src/deep.rs"), "guest").unwrap();
        // Added on both sides with different contents is a conflict too.
        fs::write(fixture.host.path().join("new.rs"), "host").unwrap();
        fs::write(fixture.guest.path().join("new.rs"), "guest").unwrap();
        // Added on both sides with the same contents is not.
        fs::write(fixture.host.path().join("agreed.rs"), "same").unwrap();
        fs::write(fixture.guest.path().join("agreed.rs"), "same").unwrap();

        let report = fixture.merge();

        assert!(report.applied.is_empty());
        assert_eq!(
            report.conflicts,
            [
                Conflict {
                    path: "both.rs".to_owned(),
                    guest_copy: Some("both.rs.guest".to_owned()),
                },
                Conflict {
                    path: "new.rs".to_owned(),
                    guest_copy: Some("new.rs.guest".to_owned()),
                },
                Conflict {
                    path: "src/deep.rs".to_owned(),
                    guest_copy: Some("src/deep.rs.guest".to_owned()),
                },
            ]
        );
        assert_eq!(fixture.host_file("both.rs").as_deref(), Some("host"));
        assert_eq!(fixture.host_file("both.rs.guest").as_deref(), Some("guest"));
        assert_eq!(fixture.host_file("new.rs").as_deref(), Some("host"));
        assert_eq!(fixture.host_file("new.rs.guest").as_deref(), Some("guest"));
        assert_eq!(fixture.host_file("src/deep.rs").as_deref(), Some("host"));
        assert_eq!(
            fixture.host_file("src/deep.rs.guest").as_deref(),
            Some("guest")
        );
        assert_eq!(fixture.host_file("agreed.rs").as_deref(), Some("same"));
        assert!(!fixture.host.path().join("agreed.rs.guest").exists());

        let warnings = report.warnings(Path::new("/repo"));
        assert_eq!(warnings.len(), 3);
        assert_eq!(
            warnings[0],
            "/repo/both.rs: changed on both sides; kept the host copy, the guest copy is /repo/both.rs.guest"
        );
    }

    #[test]
    fn merge_reports_a_guest_deletion_of_a_host_edit_and_keeps_the_host_copy() {
        let fixture = MergeFixture::new(&[("edited.rs", "before")]);
        fs::write(fixture.host.path().join("edited.rs"), "host").unwrap();
        fs::remove_file(fixture.guest.path().join("edited.rs")).unwrap();

        let report = fixture.merge();

        assert_eq!(
            report.conflicts,
            [Conflict {
                path: "edited.rs".to_owned(),
                guest_copy: None,
            }]
        );
        assert_eq!(fixture.host_file("edited.rs").as_deref(), Some("host"));
        assert_eq!(
            report.warnings(Path::new("/repo")),
            ["/repo/edited.rs: changed on the host and deleted in the guest; kept the host copy"]
        );
    }

    #[test]
    fn merge_keeps_a_directory_the_guest_deleted_while_the_host_added_to_it() {
        let fixture = MergeFixture::new(&[("dir/old.rs", "old")]);
        fs::write(fixture.host.path().join("dir/new.rs"), "host").unwrap();
        fs::remove_dir_all(fixture.guest.path().join("dir")).unwrap();

        let report = fixture.merge();

        assert_eq!(report.applied, ["dir", "dir/old.rs"]);
        assert!(!fixture.host.path().join("dir/old.rs").exists());
        assert_eq!(fixture.host_file("dir/new.rs").as_deref(), Some("host"));
    }

    #[test]
    fn merge_replaces_a_file_with_a_directory_and_back() {
        let fixture = MergeFixture::new(&[("was-file", "file"), ("was-dir/inner", "inner")]);
        fs::remove_file(fixture.guest.path().join("was-file")).unwrap();
        fs::create_dir(fixture.guest.path().join("was-file")).unwrap();
        fs::write(fixture.guest.path().join("was-file/inner"), "now a dir").unwrap();
        fs::remove_dir_all(fixture.guest.path().join("was-dir")).unwrap();
        fs::write(fixture.guest.path().join("was-dir"), "now a file").unwrap();

        fixture.merge();

        assert_eq!(
            fixture.host_file("was-file/inner").as_deref(),
            Some("now a dir")
        );
        assert_eq!(fixture.host_file("was-dir").as_deref(), Some("now a file"));
    }

    /// A merge is applied whole or not at all. When a step fails halfway - here a copy into
    /// a directory the host has made read-only - the steps before it are undone and the host
    /// worktree is exactly what it was.
    #[test]
    fn a_merge_that_fails_midway_rolls_the_host_back() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root is not refused by a read-only directory
        }
        let fixture = MergeFixture::new(&[
            ("kept.rs", "same"),
            ("removed.rs", "gone"),
            ("ro/existing.rs", "x"),
        ]);
        fs::remove_file(fixture.guest.path().join("removed.rs")).unwrap();
        fs::write(fixture.guest.path().join("added.rs"), "new").unwrap();
        fs::write(fixture.guest.path().join("ro/new.rs"), "new").unwrap();
        let before = fixture.host_tree();
        let read_only = fixture.host.path().join("ro");
        let mode = fs::metadata(&read_only).unwrap().permissions().mode();
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555)).unwrap();

        let error = fixture.try_merge().unwrap_err();
        fs::set_permissions(&read_only, fs::Permissions::from_mode(mode)).unwrap();

        assert!(format!("{error:#}").contains("rolled back"), "{error:#}");
        assert_eq!(fixture.host_tree(), before);
        assert_eq!(fixture.host_file("removed.rs").as_deref(), Some("gone"));
        assert_eq!(fixture.host_file("added.rs"), None);
        assert_eq!(fixture.host_file("ro/new.rs"), None);
    }

    /// The guest turning a directory into a file while the host still keeps ignored outputs
    /// in it is the merge that used to fail halfway, with the tracked files already deleted.
    /// It is refused before anything is touched.
    #[test]
    fn a_file_replacing_a_directory_the_host_still_uses_is_refused_up_front() {
        let fixture = MergeFixture::new(&[
            (".gitignore", "d/target/\n"),
            ("d/f.rs", "f"),
            ("other.rs", "before"),
        ]);
        fs::create_dir_all(fixture.host.path().join("d/target")).unwrap();
        fs::write(fixture.host.path().join("d/target/out"), "built").unwrap();
        fs::remove_dir_all(fixture.guest.path().join("d")).unwrap();
        fs::write(fixture.guest.path().join("d"), "now a file").unwrap();
        fs::write(fixture.guest.path().join("other.rs"), "after").unwrap();
        let before = fixture.host_tree();

        let error = fixture.try_merge().unwrap_err().to_string();

        assert!(error.contains("nothing was merged"), "{error}");
        assert!(error.contains("d/target"), "{error}");
        assert_eq!(fixture.host_tree(), before);
        assert_eq!(fixture.host_file("other.rs").as_deref(), Some("before"));
        assert_eq!(fixture.host_file("d/f.rs").as_deref(), Some("f"));
        assert_eq!(fixture.host_file("d/target/out").as_deref(), Some("built"));
    }

    /// The plan says what the guest has to take and drop to agree with the merged host: the
    /// host's own changes, the host copy of every conflict with the guest copy beside it,
    /// and the host's deletions. Guest-only changes are already on both sides.
    #[test]
    fn merge_plans_what_the_guest_takes_and_drops() {
        let fixture = MergeFixture::new(&[
            ("host-edit.rs", "before"),
            ("host-gone.rs", "old"),
            ("both.rs", "before"),
            ("guest-edit.rs", "before"),
            ("host-edit-guest-gone.rs", "before"),
        ]);
        fs::write(fixture.host.path().join("host-edit.rs"), "host").unwrap();
        fs::remove_file(fixture.host.path().join("host-gone.rs")).unwrap();
        fs::write(fixture.host.path().join("both.rs"), "host").unwrap();
        fs::write(fixture.host.path().join("host-edit-guest-gone.rs"), "host").unwrap();
        fs::write(fixture.host.path().join("host-new.rs"), "new").unwrap();
        fs::write(fixture.guest.path().join("both.rs"), "guest").unwrap();
        fs::write(fixture.guest.path().join("guest-edit.rs"), "guest").unwrap();
        fs::remove_file(fixture.guest.path().join("host-edit-guest-gone.rs")).unwrap();

        let merged = fixture.merge_fully();

        assert_eq!(
            merged.push,
            [
                "both.rs",
                "both.rs.guest",
                "host-edit-guest-gone.rs",
                "host-edit.rs",
                "host-new.rs",
            ]
        );
        assert_eq!(merged.remove, ["host-gone.rs"]);
        assert_eq!(merged.report.applied, ["guest-edit.rs"]);
        assert_eq!(merged.report.conflicts.len(), 2);
    }

    #[test]
    fn merge_copies_symlinks_and_modes() {
        let fixture = MergeFixture::new(&[("script.sh", "#!/bin/sh\n"), ("target.rs", "t")]);
        fs::set_permissions(
            fixture.guest.path().join("script.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("target.rs", fixture.guest.path().join("link")).unwrap();

        fixture.merge();

        assert_eq!(
            fs::metadata(fixture.host.path().join("script.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_link(fixture.host.path().join("link")).unwrap(),
            PathBuf::from("target.rs")
        );
    }

    #[test]
    fn guest_workspace_follows_the_configured_cwd() {
        let root = tempfile::tempdir().unwrap();
        let state = vm_state(PathBuf::from("rootfs.ext4"), PathBuf::from("vm.sock"), None);
        let mut config = provider_config(root.path());
        assert_eq!(
            guest_workspace(&state, &config),
            "root@172.31.0.2:/workspace/"
        );
        config.guest_cwd = PathBuf::from("/srv/work");
        assert_eq!(
            guest_workspace(&state, &config),
            "root@172.31.0.2:/srv/work/"
        );
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
        let baseline = worktree_baseline(host.path()).unwrap();
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
        let undo = tempfile::tempdir().unwrap();
        let report = merge_worktrees(&baseline, host.path(), staged.path(), undo.path())
            .unwrap()
            .report;
        assert_eq!(report.applied, ["source.rs"]);
        assert!(report.conflicts.is_empty());
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
    fn vm_liveness_uses_pid_and_start_time_only() {
        let mut state = vm_state(PathBuf::new(), PathBuf::new(), None);
        state.pid = std::process::id() as i32;
        state.start_time = process_start_time(state.pid).unwrap();

        assert!(vm_is_running(&state));
        state.start_time = state.start_time.wrapping_add(1);
        assert!(!vm_is_running(&state));
    }

    /// The state dir follows `$XDG_STATE_HOME` or `$HOME`, either of which may
    /// contain a space. ssh's option tokenizer would cut an unquoted
    /// `UserKnownHostsFile=` there, and rsync's `-e` splitter strips one layer
    /// of quotes before ssh sees the string at all.
    #[test]
    fn guest_ssh_commands_quote_the_known_hosts_path() {
        let root = tempfile::tempdir().unwrap();
        let mut state = vm_state(
            root.path().join("rootfs"),
            root.path().join("api.sock"),
            None,
        );
        state.known_hosts = PathBuf::from("/home/a user/state/known_hosts");
        let mut config = provider_config(root.path());
        config.ssh_private_key = PathBuf::from("/home/a user/id_ed25519");

        let command = ssh_command(&state, &config);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&r#"UserKnownHostsFile="/home/a user/state/known_hosts""#.to_owned()),
            "{args:?}"
        );
        assert!(args.contains(&"/home/a user/id_ed25519".to_owned()));

        let shell = rsync_remote_shell(&state, &config);
        assert!(
            shell.contains(r#"'-o' 'UserKnownHostsFile="/home/a user/state/known_hosts"'"#),
            "{shell}"
        );
        assert!(shell.contains("'-i' '/home/a user/id_ed25519'"), "{shell}");
        assert!(shell.starts_with("ssh '-o' 'BatchMode=yes'"), "{shell}");
    }

    /// A VM the last attempt left unready is the same VM: provisioning picks up where it
    /// stopped rather than starting a new disk, which is what lets a worktree too large for
    /// one attempt converge. Only a VM of a different name or image is replaced.
    #[test]
    fn an_unready_vm_of_the_same_identity_is_resumed_not_replaced() {
        let root = tempfile::tempdir().unwrap();
        let mut state = vm_state(
            root.path().join("rootfs"),
            root.path().join("api.sock"),
            None,
        );
        assert_eq!(
            existing_vm_disposition(&state, "review", None),
            Disposition::Resume
        );
        assert_eq!(
            existing_vm_disposition(&state, "other", None),
            Disposition::Replace
        );
        assert_eq!(
            existing_vm_disposition(&state, "review", Some("rust")),
            Disposition::Replace
        );

        state.ready = true;
        assert_eq!(
            existing_vm_disposition(&state, "review", None),
            Disposition::Join
        );
        assert!(matches!(
            existing_vm_disposition(&state, "other", None),
            Disposition::Refuse(why) if why.contains("\"other\"")
        ));
        state.image = Some("rust".to_owned());
        assert!(matches!(
            existing_vm_disposition(&state, "review", None),
            Disposition::Refuse(why) if why.contains("image")
        ));
        assert_eq!(
            existing_vm_disposition(&state, "review", Some("rust")),
            Disposition::Join
        );
    }

    /// The server times a provider out for silence, so a phase that takes a while has to
    /// keep saying so - and stop saying so the moment it ends.
    #[test]
    fn progress_is_reported_until_the_phase_ends() {
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink(Arc::new(Mutex::new(vec![])));
        let progress = Progress::start_into(
            "copying the worktree into the guest",
            Box::new(sink.clone()),
            Duration::from_millis(20),
        );
        thread::sleep(Duration::from_millis(100));
        drop(progress);

        let written = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = written.lines().collect();
        assert!(lines.len() >= 2, "{written:?}");
        assert!(
            lines
                .iter()
                .all(|line| *line == "progress: copying the worktree into the guest"),
            "{written:?}"
        );
        let after = sink.0.lock().unwrap().len();
        thread::sleep(Duration::from_millis(60));
        assert_eq!(
            sink.0.lock().unwrap().len(),
            after,
            "progress kept reporting after the phase ended"
        );
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
        // A copy the provider was killed in the middle of.
        fs::create_dir(vm_dir.join(STAGING_DIR)).unwrap();
        fs::write(vm_dir.join(STAGING_DIR).join("half"), "copied").unwrap();
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
        assert!(
            !recovered.join(STAGING_DIR).exists(),
            "the staging scratch space was retained with the disk"
        );
        assert!(recovered.join("state.json").exists());
        let note: RecoveryNote =
            serde_json::from_slice(&fs::read(recovered.join(RECOVERY_NOTE_FILE)).unwrap()).unwrap();
        assert_eq!(note.reason, "unready");
        let listed = list_recoveries(&locked, None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, recovered);
        assert_eq!(listed[0].reason, "unready");
        assert_eq!(listed[0].retained_at, note.retained_at);
        assert_eq!(listed[0].session.as_deref(), Some("work"));
        assert_eq!(listed[0].name.as_deref(), Some("review"));
        assert!(list_recoveries(&locked, Some("other")).unwrap().is_empty());
        assert_eq!(list_recoveries(&locked, Some("work")).unwrap().len(), 1);
    }

    #[test]
    fn provider_security_boundaries_reject_unsafe_credentials_and_protect_private_networks() {
        assert!(validate_guest_path(Path::new("/root/.config/codex/auth.json"), "path").is_ok());
        assert!(validate_guest_path(Path::new("/root/../etc/shadow"), "path").is_err());
        assert!(validate_guest_path(Path::new("/root/key;touch-pwned"), "path").is_err());
        assert!(validate_guest_path(Path::new("workspace"), "guest_cwd").is_err());

        let config = NetworkConfig {
            bridge: "zfc1000".to_owned(),
            tap_prefix: "zft1000-".to_owned(),
            subnet: "172.31.0.0/24".to_owned(),
            taps: 2,
            dns: Ipv4Addr::new(10, 0, 0, 53),
        };
        let chain_rules = firewall_chain_rules(&config)
            .into_iter()
            .map(|args| args.join(" "))
            .collect::<Vec<_>>();
        let dns = chain_rules
            .iter()
            .position(|rule| rule.contains("-d 10.0.0.53 -p udp --dport 53 -j ACCEPT"))
            .unwrap();
        let private = chain_rules
            .iter()
            .position(|rule| rule.contains("-d 10.0.0.0/8 -j DROP"))
            .unwrap();
        assert!(
            dns < private,
            "the configured DNS exception must precede the private-network drop"
        );
        assert!(chain_rules
            .iter()
            .any(|rule| rule.contains("--dports 22,80,443 -j ACCEPT")));

        let base = firewall_base_rules(1000, &config)
            .iter()
            .map(|rule| rule.insert_args().join(" "))
            .collect::<Vec<_>>();
        let tag = "-m comment --comment zellij-firecracker:1000";
        assert!(base
            .iter()
            .any(|rule| rule == &format!("-I INPUT 1 -i zfc1000 -s 172.31.0.0/24 {tag} -j DROP")));
        assert!(base.iter().any(|rule| rule
            == &format!(
                "{} {tag} -j ACCEPT",
                "-I INPUT 1 -i zfc1000 -s 172.31.0.0/24 -m conntrack --ctstate ESTABLISHED,RELATED"
            )));
        assert!(base
            .iter()
            .any(|rule| rule
                == &format!("-t nat -I POSTROUTING 1 -s 172.31.0.0/24 {tag} -j MASQUERADE")));
        assert!(
            base.iter().all(|rule| rule.contains(tag)),
            "every rule outside our own chain must carry the tag that identifies it: {base:?}"
        );
    }

    #[test]
    fn the_apt_build_overrides_are_written_where_apt_reads_them() {
        let root = tempfile::tempdir().unwrap();
        let path = write_apt_build_overrides(root.path()).unwrap();

        assert_eq!(path, root.path().join(APT_SANDBOX_OVERRIDE));
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("APT::Sandbox::User \"root\";"),
            "{contents}"
        );
        assert!(contents.contains("--force-confold"), "{contents}");
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_policy_override_refuses_to_start_services() {
        let root = tempfile::tempdir().unwrap();
        let path = write_policy_rc_d(root.path()).unwrap();

        assert!(fs::read_to_string(&path).unwrap().ends_with("exit 101\n"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn apt_spool_directories_are_recreated_for_a_container_rootfs() {
        let root = tempfile::tempdir().unwrap();
        stage_apt_directories(root.path()).unwrap();

        assert!(root.path().join("var/cache/apt/archives/partial").is_dir());
        assert!(root.path().join("var/lib/apt/lists/partial").is_dir());
        assert!(root.path().join("var/log/apt").is_dir());
    }

    #[test]
    fn the_first_host_resolver_naming_a_nameserver_wins() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let empty = dir.path().join("empty");
        let comments = dir.path().join("comments");
        let real = dir.path().join("real");
        fs::write(&empty, "").unwrap();
        fs::write(&comments, "# nothing to see\n").unwrap();
        fs::write(&real, "search example\nnameserver 10.0.0.1\n").unwrap();

        assert_eq!(
            resolv_conf_from(&[
                missing.clone(),
                empty.clone(),
                comments.clone(),
                real.clone()
            ])
            .unwrap(),
            b"search example\nnameserver 10.0.0.1\n"
        );

        let error = resolv_conf_from(&[missing.clone(), empty, comments]).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }

    #[test]
    fn staging_a_resolver_is_undone_afterwards() {
        let root = tempfile::tempdir().unwrap();
        let host = tempfile::tempdir().unwrap();
        let resolver = host.path().join("resolv.conf");
        fs::write(&resolver, "nameserver 10.0.0.1\n").unwrap();
        let guest = root.path().join("etc/resolv.conf");
        fs::create_dir_all(guest.parent().unwrap()).unwrap();
        fs::write(&guest, "\n").unwrap();

        let saved = fs::read(&guest).ok();
        let contents = resolv_conf_from(&[resolver]).unwrap();
        fs::write(&guest, contents).unwrap();
        assert_eq!(fs::read_to_string(&guest).unwrap(), "nameserver 10.0.0.1\n");

        restore_resolv_conf(root.path(), saved).unwrap();
        assert_eq!(fs::read_to_string(&guest).unwrap(), "\n");
    }

    #[test]
    fn a_resolver_staged_where_there_was_none_is_removed_again() {
        let root = tempfile::tempdir().unwrap();
        let guest = root.path().join("etc/resolv.conf");
        fs::create_dir_all(guest.parent().unwrap()).unwrap();
        fs::write(&guest, "nameserver 10.0.0.1\n").unwrap();

        restore_resolv_conf(root.path(), None).unwrap();

        assert!(!guest.exists());
    }

    #[test]
    fn chroot_binds_stay_inside_the_staged_root() {
        let root = tempfile::tempdir().unwrap();
        let binds = chroot_binds(root.path());

        assert!(!binds.is_empty());
        for (source, target) in &binds {
            assert!(source.starts_with("/dev"), "{}", source.display());
            assert!(target.starts_with(root.path()), "{}", target.display());
            assert!(!target
                .components()
                .any(|component| component == std::path::Component::ParentDir));
        }
        assert!(binds
            .iter()
            .any(|(source, _)| source == Path::new("/dev/null")));
    }

    #[test]
    fn staged_devices_report_only_what_they_created() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("dev/null");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "").unwrap();

        let created = stage_chroot_devices(root.path()).unwrap();

        assert!(!created.contains(&existing));
        assert!(created.contains(&root.path().join("dev/zero")));
        for path in &created {
            assert!(path.is_file(), "{}", path.display());
        }
    }

    #[test]
    fn the_chroot_environment_does_not_inherit_the_host_home() {
        let host = HashMap::from([
            ("HOME".to_owned(), "/home/someone".to_owned()),
            ("CARGO_HOME".to_owned(), "/home/someone/.cargo".to_owned()),
            ("RUSTUP_HOME".to_owned(), "/home/someone/.rustup".to_owned()),
            ("TMPDIR".to_owned(), "/scratch".to_owned()),
            ("https_proxy".to_owned(), "http://proxy:3128".to_owned()),
        ]);

        let env = chroot_env_from(&host);
        let value = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };

        assert_eq!(value("HOME").as_deref(), Some("/root"));
        assert_eq!(value("TMPDIR").as_deref(), Some("/tmp"));
        assert_eq!(value("CARGO_HOME"), None);
        assert_eq!(value("RUSTUP_HOME"), None);
        assert_eq!(value("https_proxy").as_deref(), Some("http://proxy:3128"));
        assert!(value("PATH").unwrap().starts_with("/root/.cargo/bin:"));
    }

    #[test]
    fn the_customization_runs_in_its_own_mount_and_pid_namespace() {
        let command = image_customize_command(
            Path::new("/build/zellij-firecracker"),
            Path::new("/usr/bin/unshare"),
            Path::new("/staging/root"),
        );

        assert_eq!(command.get_program(), "/usr/bin/unshare");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--mount",
                "--pid",
                "--fork",
                "--propagation",
                "private",
                "--",
                "/build/zellij-firecracker",
                "image",
                "customize-inner",
                "--root",
                "/staging/root",
            ]
        );
    }

    fn network_config() -> NetworkConfig {
        NetworkConfig {
            bridge: "zfc1000".to_owned(),
            tap_prefix: "zft1000-".to_owned(),
            subnet: "172.31.0.0/24".to_owned(),
            taps: 2,
            dns: Ipv4Addr::new(1, 1, 1, 1),
        }
    }

    /// Enough of an iptables table to exercise the real command sequence: chain creation refuses
    /// a duplicate, `-X` refuses a chain something still jumps to, and `-D` refuses a rule that
    /// is not there - the three failures that produced the wedged host this all came from.
    #[derive(Default)]
    struct FakeIptables {
        chains: Vec<(String, String)>,
        rules: Vec<(String, String, Vec<String>)>,
        log: Vec<String>,
    }

    impl FakeIptables {
        fn seed(&mut self, table: &str, chain: &str, spec: &str) {
            self.rules.push((
                table.to_owned(),
                chain.to_owned(),
                spec.split_whitespace().map(str::to_owned).collect(),
            ));
        }

        fn add_chain(&mut self, table: &str, chain: &str) {
            self.chains.push((table.to_owned(), chain.to_owned()));
        }

        /// The table as `iptables -S` prints it, comments quoted the way iptables quotes them.
        fn dump(&self, table: &str) -> String {
            let builtins: &[&str] = if table == "nat" {
                &["PREROUTING", "INPUT", "OUTPUT", "POSTROUTING"]
            } else {
                &["INPUT", "FORWARD", "OUTPUT"]
            };
            let mut lines: Vec<String> = builtins
                .iter()
                .map(|chain| format!("-P {chain} ACCEPT"))
                .collect();
            lines.extend(
                self.chains
                    .iter()
                    .filter(|(owner, _)| owner == table)
                    .map(|(_, chain)| format!("-N {chain}")),
            );
            lines.extend(
                self.rules
                    .iter()
                    .filter(|(owner, _, _)| owner == table)
                    .map(|(_, chain, spec)| format!("-A {chain} {}", render_spec(spec))),
            );
            lines.join("\n")
        }

        fn lines(&self, table: &str) -> Vec<String> {
            self.dump(table)
                .lines()
                .filter(|line| line.starts_with("-A"))
                .map(str::to_owned)
                .collect()
        }

        fn fail(stderr: &str) -> Result<IptablesOutput> {
            Ok(IptablesOutput {
                success: false,
                stdout: String::new(),
                stderr: stderr.to_owned(),
            })
        }

        fn ok(stdout: String) -> Result<IptablesOutput> {
            Ok(IptablesOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn render_spec(spec: &[String]) -> String {
        let mut rendered: Vec<String> = Vec::new();
        for (index, word) in spec.iter().enumerate() {
            if index > 0 && spec[index - 1] == "--comment" {
                rendered.push(format!("\"{word}\""));
            } else {
                rendered.push(word.clone());
            }
        }
        rendered.join(" ")
    }

    impl Iptables for FakeIptables {
        fn run(&mut self, args: &[String]) -> Result<IptablesOutput> {
            self.log.push(args.join(" "));
            let (table, rest) = match args {
                [flag, table, rest @ ..] if flag == "-t" => (table.clone(), rest),
                rest => ("filter".to_owned(), rest),
            };
            let chain = rest.get(1).cloned().unwrap_or_default();
            let known = |fake: &Self, chain: &str| {
                fake.chains
                    .iter()
                    .any(|(owner, name)| owner == &table && name == chain)
            };
            match rest[0].as_str() {
                "-S" => Self::ok(self.dump(&table)),
                "-N" => {
                    if known(self, &chain) {
                        return Self::fail("iptables: Chain already exists.");
                    }
                    self.chains.push((table, chain));
                    Self::ok(String::new())
                },
                "-F" => {
                    self.rules
                        .retain(|(owner, name, _)| owner != &table || name != &chain);
                    Self::ok(String::new())
                },
                "-X" => {
                    if !known(self, &chain) {
                        return Self::fail("iptables: No chain/target/match by that name.");
                    }
                    if self.rules.iter().any(|(owner, _, spec)| {
                        owner == &table
                            && spec
                                .windows(2)
                                .any(|pair| pair[0] == "-j" && pair[1] == chain)
                    }) {
                        return Self::fail("iptables: Too many links.");
                    }
                    self.chains
                        .retain(|(owner, name)| owner != &table || name != &chain);
                    Self::ok(String::new())
                },
                verb @ ("-A" | "-I" | "-C" | "-D") => {
                    let spec: Vec<String> = if verb == "-I" {
                        rest[3..].to_vec()
                    } else {
                        rest[2..].to_vec()
                    };
                    let position = self.rules.iter().position(|(owner, name, existing)| {
                        owner == &table && name == &chain && existing == &spec
                    });
                    match verb {
                        "-A" => self.rules.push((table, chain, spec)),
                        "-I" => {
                            let at = self
                                .rules
                                .iter()
                                .position(|(owner, name, _)| owner == &table && name == &chain)
                                .unwrap_or(self.rules.len());
                            self.rules.insert(at, (table, chain, spec));
                        },
                        "-C" => {
                            if position.is_none() {
                                return Self::fail("iptables: Bad rule.");
                            }
                        },
                        _ => match position {
                            Some(at) => {
                                self.rules.remove(at);
                            },
                            None => return Self::fail("iptables: Bad rule."),
                        },
                    }
                    Self::ok(String::new())
                },
                other => panic!("unexpected iptables verb {other}"),
            }
        }
    }

    /// The host that produced this work: rules written for a retired subnet, plus an empty chain
    /// they still jump to, plus unrelated Docker and Tailscale rules that must survive.
    fn wedged_table() -> FakeIptables {
        let mut fake = FakeIptables::default();
        fake.add_chain("filter", "ZFC_ZFC1000");
        fake.add_chain("filter", "DOCKER-USER");
        fake.seed(
            "filter",
            "INPUT",
            "-s 172.31.0.0/24 -i zfc1000 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT",
        );
        fake.seed("filter", "INPUT", "-s 172.31.0.0/24 -i zfc1000 -j DROP");
        fake.seed("filter", "INPUT", "-p tcp -m tcp --dport 4000 -j ACCEPT");
        fake.seed(
            "filter",
            "FORWARD",
            "-d 172.31.0.0/24 -o zfc1000 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT",
        );
        fake.seed(
            "filter",
            "FORWARD",
            "-s 172.31.0.0/24 -i zfc1000 -j ZFC_ZFC1000",
        );
        fake.seed("filter", "FORWARD", "-j DOCKER-USER");
        fake.seed("filter", "DOCKER-USER", "-o core-vm-harness -j ACCEPT");
        fake.seed("nat", "POSTROUTING", "-s 172.31.0.0/24 -j MASQUERADE");
        fake.seed(
            "nat",
            "POSTROUTING",
            "-s 172.17.0.0/16 ! -o docker0 -j MASQUERADE",
        );
        fake
    }

    fn uid_network_config() -> NetworkConfig {
        NetworkConfig {
            bridge: "zfc1000".to_owned(),
            tap_prefix: "zft1000-".to_owned(),
            subnet: "10.3.232.0/24".to_owned(),
            taps: 2,
            dns: Ipv4Addr::new(1, 1, 1, 1),
        }
    }

    /// The regression this whole change exists for: a host whose rules were written for a subnet
    /// the configuration no longer names used to fail with "Chain already exists" forever, because
    /// cleanup deleted by spec and so matched nothing.
    #[test]
    fn applying_the_firewall_recovers_a_host_left_on_a_retired_subnet() {
        let mut fake = wedged_table();
        let config = uid_network_config();

        apply_firewall(&mut fake, 1000, &config, &strings(&["10.3.232.0/24"])).unwrap();

        let filter = fake.lines("filter");
        assert!(
            !filter.iter().any(|rule| rule.contains("172.31.0.0/24")),
            "the retired subnet's rules must be gone: {filter:#?}"
        );
        assert!(filter
            .iter()
            .any(|rule| rule == "-A INPUT -p tcp -m tcp --dport 4000 -j ACCEPT"));
        assert!(filter
            .iter()
            .any(|rule| rule == "-A FORWARD -j DOCKER-USER"));
        assert!(filter
            .iter()
            .any(|rule| rule == "-A DOCKER-USER -o core-vm-harness -j ACCEPT"));
        assert!(filter.iter().any(|rule| rule
            == "-A FORWARD -i zfc1000 -s 10.3.232.0/24 -m comment --comment \
                \"zellij-firecracker:1000\" -j ZFC_ZFC1000"));
        assert!(filter
            .iter()
            .any(|rule| rule == "-A ZFC_ZFC1000 -d 10.0.0.0/8 -j DROP"));
        let nat = fake.lines("nat");
        assert!(nat.iter().any(|rule| rule
            == "-A POSTROUTING -s 10.3.232.0/24 -m comment --comment \
                \"zellij-firecracker:1000\" -j MASQUERADE"));
        assert!(
            nat.iter()
                .any(|rule| rule == "-A POSTROUTING -s 172.17.0.0/16 ! -o docker0 -j MASQUERADE"),
            "another tool's NAT rule must survive: {nat:#?}"
        );
    }

    /// A NAT rule for a forgotten subnet names no interface and carries no tag, so nothing proves
    /// it is ours. It is reported rather than guessed at - and it does not block setup.
    #[test]
    fn a_masquerade_for_a_forgotten_subnet_is_reported_and_left_alone() {
        let mut fake = wedged_table();
        let config = uid_network_config();

        let cleanup =
            remove_firewall(&mut fake, None, &config, &strings(&["10.3.232.0/24"])).unwrap();

        assert_eq!(
            cleanup.orphans,
            vec!["-A POSTROUTING -s 172.31.0.0/24 -j MASQUERADE".to_owned()],
            "Docker's and Tailscale's rules carry an interface or a mark and must not be reported"
        );
        assert!(fake
            .lines("nat")
            .iter()
            .any(|rule| rule == "-A POSTROUTING -s 172.31.0.0/24 -j MASQUERADE"));
        assert!(
            !fake
                .dump("filter")
                .lines()
                .any(|line| line == "-N ZFC_ZFC1000"),
            "the chain must be gone once nothing jumps to it"
        );
    }

    /// The boot unit re-applies on every boot, so a second application has to be a no-op rather
    /// than a duplicate or a collision.
    #[test]
    fn applying_the_firewall_twice_converges_on_the_same_rules() {
        let config = uid_network_config();
        let subnets = strings(&["10.3.232.0/24"]);
        let mut fake = FakeIptables::default();

        apply_firewall(&mut fake, 1000, &config, &subnets).unwrap();
        let first = fake.lines("filter");
        let first_nat = fake.lines("nat");
        fake.log.clear();
        apply_firewall(&mut fake, 1000, &config, &subnets).unwrap();

        assert_eq!(fake.lines("filter"), first);
        assert_eq!(fake.lines("nat"), first_nat);
        let mut sorted = first.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            first.len(),
            "re-applying must not duplicate a rule: {first:#?}"
        );
    }

    /// `-D` removes one match, so a table that accumulated duplicates needs more than one pass.
    #[test]
    fn cleanup_drains_duplicate_rules() {
        let config = uid_network_config();
        let mut fake = FakeIptables::default();
        for _ in 0..3 {
            fake.seed(
                "filter",
                "FORWARD",
                "-i zfc1000 -s 10.3.232.0/24 -m comment --comment zellij-firecracker:1000 -j ACCEPT",
            );
        }

        remove_firewall(&mut fake, None, &config, &strings(&["10.3.232.0/24"])).unwrap();

        assert!(fake.lines("filter").is_empty());
    }

    /// The test harness applies a network of its own with the user's uid, so every rule it writes
    /// carries the same tag as the user's real pool. Applying and removing that network must
    /// leave the pool's rules - its INPUT/FORWARD rules, its jump and its NAT - exactly as found.
    #[test]
    fn applying_a_network_does_not_sweep_another_networks_rules() {
        let pool = uid_network_config();
        let harness = NetworkConfig {
            bridge: "zfcb4d2".to_owned(),
            tap_prefix: "zfct4d2-".to_owned(),
            subnet: "10.204.210.0/24".to_owned(),
            taps: 2,
            dns: Ipv4Addr::new(1, 1, 1, 1),
        };
        let mut fake = FakeIptables::default();
        apply_firewall(&mut fake, 1000, &pool, &strings(&[&pool.subnet])).unwrap();
        let pool_filter = fake.lines("filter");
        let pool_nat = fake.lines("nat");
        assert!(!pool_filter.is_empty() && !pool_nat.is_empty());

        apply_firewall(&mut fake, 1000, &harness, &strings(&[&harness.subnet])).unwrap();
        for rule in pool_filter.iter().chain(&pool_nat) {
            assert!(
                fake.lines("filter").contains(rule) || fake.lines("nat").contains(rule),
                "the pool's rule {rule:?} must survive another network's apply"
            );
        }

        remove_firewall(&mut fake, None, &harness, &strings(&[&harness.subnet])).unwrap();
        assert_eq!(fake.lines("filter"), pool_filter);
        assert_eq!(fake.lines("nat"), pool_nat);

        // Only a teardown by tag reaches the pool's rules.
        let tag = firewall_tag(1000);
        let ours = our_rules(
            &fake.lines("filter"),
            &fake.lines("nat"),
            &firewall_chain(&harness),
            &harness.bridge,
            Some(&tag),
            &strings(&[&harness.subnet]),
        );
        assert!(
            !ours.is_empty(),
            "a tagged sweep must still find the pool's rules"
        );
    }

    /// A subnet recorded for the uid is swept only when it belongs to the network being applied.
    #[test]
    fn a_recorded_subnet_counts_only_for_the_same_bridge() {
        let recorded = uid_network_config();
        let same_bridge = NetworkConfig {
            subnet: "10.9.9.0/24".to_owned(),
            ..recorded.clone()
        };
        let other_bridge = NetworkConfig {
            bridge: "zfcbeef".to_owned(),
            subnet: "10.9.9.0/24".to_owned(),
            ..recorded.clone()
        };
        assert_eq!(
            known_subnets_with(&same_bridge, Some(&recorded)),
            strings(&["10.9.9.0/24", "10.3.232.0/24"])
        );
        assert_eq!(
            known_subnets_with(&other_bridge, Some(&recorded)),
            strings(&["10.9.9.0/24"])
        );
    }

    /// Rollback undoes this run's work and nothing else: a bridge that was already up belongs to
    /// whatever created it, and deleting it would cut off VMs another session is running.
    #[test]
    fn rollback_only_undoes_what_the_run_created() {
        assert_eq!(
            undo_steps(None, strings(&["zft1000-00"])),
            vec![Undo::Firewall, Undo::Interface("zft1000-00".to_owned())],
            "a pre-existing bridge must survive a failed re-run"
        );
        assert_eq!(
            undo_steps(
                Some("zfc1000".to_owned()),
                strings(&["zft1000-00", "zft1000-01"])
            ),
            vec![
                Undo::Firewall,
                Undo::Interface("zft1000-01".to_owned()),
                Undo::Interface("zft1000-00".to_owned()),
                Undo::Interface("zfc1000".to_owned()),
            ],
            "TAPs come off before the bridge they are attached to"
        );
    }

    #[test]
    fn iptables_output_is_split_back_into_the_arguments_that_produced_it() {
        assert_eq!(
            split_rule(
                "-A INPUT -i zfc1000 -m comment --comment \"zellij-firecracker:1000\" -j DROP"
            ),
            strings(&[
                "-A",
                "INPUT",
                "-i",
                "zfc1000",
                "-m",
                "comment",
                "--comment",
                "zellij-firecracker:1000",
                "-j",
                "DROP",
            ])
        );
        assert_eq!(
            split_rule("--comment \"two words\""),
            strings(&["--comment", "two words"])
        );
        assert_eq!(
            split_rule("--comment \"a \\\"quoted\\\" word\""),
            strings(&["--comment", "a \"quoted\" word"])
        );
        assert_eq!(split_rule("--comment \"\""), strings(&["--comment", ""]));
    }

    /// Teardown is the recovery path for a host a failed setup left without a config file.
    #[test]
    fn a_uid_alone_is_enough_to_describe_the_network_to_remove() {
        let config = default_network_config(1000).unwrap();

        assert_eq!(config.bridge, "zfc1000");
        assert_eq!(config.tap_prefix, "zft1000-");
        assert_eq!(config.subnet, "10.3.232.0/24");
        validate_network_config(&config).unwrap();
    }

    #[test]
    fn network_removal_deletes_every_tap_and_then_the_bridge() {
        let commands = network_remove_commands(&network_config());
        assert_eq!(
            commands,
            [
                strings(&["link", "delete", "zft1000-00"]),
                strings(&["link", "delete", "zft1000-01"]),
                strings(&["link", "delete", "zfc1000"]),
            ]
        );
    }

    /// The delegation root is the top of the run of owned ancestors, and a process whose
    /// own cgroup is not delegated gets told so rather than silently ranging over the host.
    #[test]
    fn the_delegated_cgroup_root_is_the_highest_owned_ancestor() {
        let owned = |dir: &Path| {
            [
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service",
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice",
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/x.scope",
            ]
            .contains(&dir.to_str().unwrap())
        };
        let own = Path::new(
            "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/x.scope",
        );
        assert_eq!(
            delegation_root(own, &owned).unwrap(),
            Path::new("/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service")
        );

        // Owning everything up to the top of the hierarchy still stops at the mount.
        assert_eq!(
            delegation_root(Path::new("/sys/fs/cgroup/a/b"), &|_| true).unwrap(),
            Path::new("/sys/fs/cgroup/a")
        );

        let session = Path::new("/sys/fs/cgroup/user.slice/user-1000.slice/session-3.scope");
        let error = delegation_root(session, &owned).unwrap_err().to_string();
        assert!(error.contains("not delegated to this user"), "{error}");
        assert!(error.contains("session-3.scope"), "{error}");
    }

    /// Only a host with nowhere to put the VM is optional. Whether limits are required
    /// decides what an undelegated cgroup costs - the VM, or just the limits - and a
    /// delegated one is used either way.
    #[test]
    fn an_undelegated_cgroup_stops_a_vm_only_while_limits_are_required() {
        let root = PathBuf::from("/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service");
        assert_eq!(
            cgroup_root_or_skip(Ok(root.clone()), true).unwrap(),
            Some(root.clone())
        );
        assert_eq!(
            cgroup_root_or_skip(Ok(root.clone()), false).unwrap(),
            Some(root)
        );

        let error = || {
            Err(anyhow!(
                "cgroup /session-3.scope is not delegated to this user"
            ))
        };
        assert_eq!(cgroup_root_or_skip(error(), false).unwrap(), None);
        let message = cgroup_root_or_skip(error(), true).unwrap_err().to_string();
        assert!(message.contains("not delegated to this user"), "{message}");
    }

    /// A delegated cgroup subtree needs root to create, so a config that does not ask for
    /// the limits gets VMs rather than an error; only an explicit `true` insists on them.
    #[test]
    fn resource_limits_are_optional_unless_the_config_asks_for_them() {
        let root = tempfile::tempdir().unwrap();
        let mut config = provider_config(root.path());
        config.require_resource_limits = true;
        let mut json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["require_resource_limits"], serde_json::json!(true));

        let strict: Config = serde_json::from_value(json.clone()).unwrap();
        assert!(strict.require_resource_limits);

        json.as_object_mut()
            .unwrap()
            .remove("require_resource_limits");
        let inherited: Config = serde_json::from_value(json).unwrap();
        assert!(!inherited.require_resource_limits);
    }

    fn recovery_dir_with_note(recovery: &Path, name: &str, retained_at: SystemTime) -> PathBuf {
        let dir = recovery.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("rootfs.ext4"), "disk").unwrap();
        fs::write(
            dir.join(RECOVERY_NOTE_FILE),
            serde_json::to_vec(&RecoveryNote {
                reason: "unready".to_owned(),
                retained_at: retained_at.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            })
            .unwrap(),
        )
        .unwrap();
        dir
    }

    fn set_modified(dir: &Path, modified: SystemTime) {
        File::open(dir).unwrap().set_modified(modified).unwrap();
    }

    /// A recovery directory's mtime moves whenever something is copied out of it or dropped
    /// in beside it, so the note written when the disk was retained is what ages it.
    #[test]
    fn recoveries_are_pruned_by_the_time_they_were_retained_not_their_mtime() {
        let root = tempfile::tempdir().unwrap();
        let recovery = root.path().join("recovery");
        let day = Duration::from_secs(24 * 60 * 60);
        let now = SystemTime::now();
        let stale = recovery_dir_with_note(&recovery, "a-stale", now - 8 * day);
        let recent = recovery_dir_with_note(&recovery, "b-recent", now - day);
        set_modified(&recent, now - 30 * day);
        let noteless = recovery.join("c-noteless");
        fs::create_dir(&noteless).unwrap();
        set_modified(&noteless, now - 30 * day);
        assert!(stale.metadata().unwrap().modified().unwrap() > now - day);

        prune_recoveries(root.path()).unwrap();

        assert!(!stale.exists(), "a note eight days old is past retention");
        assert!(recent.exists(), "a note from yesterday keeps the disk");
        assert!(!noteless.exists(), "without a note the mtime still counts");
    }

    /// Only directories are recoveries. A file left beside them - an editor's swap file, a
    /// note - is not something to remove, and must not fail every provider request until
    /// someone finds it.
    #[test]
    fn a_stray_file_among_the_recoveries_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let recovery = root.path().join("recovery");
        fs::create_dir_all(&recovery).unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        let stray = recovery.join("notes.txt");
        fs::write(&stray, "remember to look at this").unwrap();
        set_modified(&stray, SystemTime::now() - 30 * day);

        prune_recoveries(root.path()).unwrap();

        assert!(stray.exists());
    }

    /// The VM directory of an attempt that died before it wrote a state file is moved aside,
    /// never cloned over: a VMM may still hold its disk open.
    #[test]
    fn an_orphaned_vm_directory_is_retained_before_a_fresh_attempt_reuses_its_path() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let vm_dir = root.path().join("abc123");
        fs::create_dir_all(&vm_dir).unwrap();
        fs::write(vm_dir.join("rootfs.ext4"), "disk").unwrap();

        retain_orphaned_vm_dir(&locked, &vm_dir).unwrap();

        assert!(!vm_dir.exists());
        let retained = recoveries(&locked);
        assert_eq!(retained.len(), 1, "{retained:?}");
        assert!(retained[0].ends_with("-orphaned"), "{retained:?}");
        let note = read_recovery_note(&root.path().join("recovery").join(&retained[0])).unwrap();
        assert_eq!(note.reason, "orphaned");
        assert!(root
            .path()
            .join("recovery")
            .join(&retained[0])
            .join("rootfs.ext4")
            .exists());

        // Nothing there is nothing to do.
        retain_orphaned_vm_dir(&locked, &vm_dir).unwrap();
        assert_eq!(recoveries(&locked).len(), 1);
    }

    /// A state file that cannot be parsed may describe a running VM. It is an error that
    /// names the file, not a VM that is not there.
    #[test]
    fn a_corrupt_state_file_is_an_error_rather_than_an_absent_vm() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let path = locked.path("work", "review");
        fs::write(&path, "{ not json").unwrap();

        let error = format!("{:#}", locked.entries().unwrap_err());
        assert!(error.contains(&path.display().to_string()), "{error}");
        let error = format!("{:#}", read_state_opt(&path).unwrap_err());
        assert!(error.contains("repair or remove"), "{error}");
        assert!(read_state_opt(&root.path().join("missing.json"))
            .unwrap()
            .is_none());

        let config = provider_config(root.path());
        let error = format!(
            "{:#}",
            ensure(
                &locked,
                &config,
                &network_config(),
                "work",
                "review",
                None,
                None
            )
            .unwrap_err()
        );
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(!root.path().join(vm_id("work", "review")).exists());
    }

    /// Without a `Child` to reap, `terminate` has to watch the process leave: the caller is
    /// about to remove its cgroup and its disk, and both are refused while it lingers.
    #[test]
    fn terminating_by_pid_waits_for_the_process_to_be_gone() {
        let process = LiveProcess::spawn();
        let mut state = vm_state(PathBuf::new(), PathBuf::new(), None);
        state.pid = process.pid;
        state.start_time = process.start_time;
        assert!(vm_is_running(&state));

        terminate(&state, None);

        assert!(!vm_is_running(&state));
    }

    /// The wait for the lock is a phase like any other, reported from its first moment, so a
    /// provider queued behind another's long `ensure` is not mistaken for a hung one.
    #[test]
    fn waiting_for_the_provider_lock_reports_progress() {
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lock");
        let holder = File::create(&path).unwrap();
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);

        let sink = Sink(Arc::new(Mutex::new(vec![])));
        let waiter = thread::spawn({
            let sink = sink.clone();
            let path = path.clone();
            move || {
                let lock = File::open(path).unwrap();
                lock_exclusively(&lock, || {
                    Progress::start_into(LOCK_WAIT_PHASE, Box::new(sink), Duration::from_secs(5))
                })
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while sink.0.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let reported = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(!waiter.is_finished(), "the lock is still held");
        assert_eq!(reported, format!("{PROGRESS_PREFIX}{LOCK_WAIT_PHASE}\n"));

        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) }, 0);
        waiter.join().unwrap().unwrap();
    }

    /// Guest worktrees kept as plain directories, one per VM name: what `release` and `sync`
    /// pull from and push to instead of rsyncing over ssh.
    #[derive(Default)]
    struct FakeGuests {
        dirs: HashMap<String, PathBuf>,
        /// Whose pull fails, as it does when a guest cannot be reached.
        unreachable: HashSet<String>,
        /// What a sync told the guests to take and to drop.
        pushed: Mutex<Vec<String>>,
        removed: Mutex<Vec<String>>,
        /// Files a guest writes the moment its worktree has been pulled - what an agent
        /// still working in the guest does during a sync. `(vm name, path, content)`.
        late_writes: Mutex<Vec<(String, String, String)>>,
    }

    impl GuestCopy for FakeGuests {
        fn pull(&self, state: &VmState, _config: &Config, dest: &Path) -> Result<()> {
            if self.unreachable.contains(&state.name) {
                return Err(anyhow!("injected pull failure for {}", state.name));
            }
            let guest = self
                .dirs
                .get(&state.name)
                .context("no fake guest for this VM")?;
            stage_filtered_worktree(guest, dest)?;
            let mut late = self.late_writes.lock().unwrap();
            for (_, path, content) in late.iter().filter(|(name, _, _)| *name == state.name) {
                fs::write(guest.join(path), content)?;
            }
            late.retain(|(name, _, _)| *name != state.name);
            Ok(())
        }

        fn push_paths(
            &self,
            state: &VmState,
            _config: &Config,
            worktree: &Path,
            paths: &[String],
        ) -> Result<()> {
            let guest = self
                .dirs
                .get(&state.name)
                .context("no fake guest for this VM")?;
            self.pushed.lock().unwrap().extend(paths.iter().cloned());
            for path in paths {
                copy_entry(&worktree.join(path), &guest.join(path))?;
            }
            Ok(())
        }

        fn remove_paths(&self, state: &VmState, _config: &Config, paths: &[String]) -> Result<()> {
            let guest = self
                .dirs
                .get(&state.name)
                .context("no fake guest for this VM")?;
            self.removed.lock().unwrap().extend(paths.iter().cloned());
            for path in paths {
                let target = guest.join(path);
                match fs::symlink_metadata(&target) {
                    Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&target)?,
                    Ok(_) => fs::remove_file(&target)?,
                    Err(_) => {},
                }
            }
            Ok(())
        }
    }

    /// A ready VM on record: its host worktree, the guest's copy of it, the baseline both
    /// started from, and the disk and socket a release cleans up.
    struct LeasedVm {
        path: PathBuf,
        state: VmState,
        host: PathBuf,
        guest: PathBuf,
        vm_dir: PathBuf,
    }

    impl LeasedVm {
        fn new(
            locked: &LockedState,
            guests: &mut FakeGuests,
            name: &str,
            files: &[(&str, &str)],
        ) -> Self {
            let root = &locked.dir;
            let host = root.join(format!("{name}-host"));
            let guest = root.join(format!("{name}-guest"));
            for (path, content) in files {
                for tree in [&host, &guest] {
                    let path = tree.join(path);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, content).unwrap();
                }
            }
            for tree in [&host, &guest] {
                fs::create_dir_all(tree).unwrap();
            }
            let vm_dir = root.join(name);
            fs::create_dir(&vm_dir).unwrap();
            let rootfs = vm_dir.join("rootfs.ext4");
            fs::write(&rootfs, "disk").unwrap();
            let api_socket = root.join(format!("{name}.sock"));
            fs::write(&api_socket, "socket").unwrap();
            let mut state = vm_state(rootfs, api_socket, None);
            state.name = name.to_owned();
            state.ready = true;
            state.worktree = Some(host.clone());
            write_baseline(
                &baseline_path(&state).unwrap(),
                &worktree_baseline(&host).unwrap(),
            )
            .unwrap();
            let path = locked.path("work", name);
            write_state(&path, &state).unwrap();
            guests.dirs.insert(name.to_owned(), guest.clone());
            Self {
                path,
                state,
                host,
                guest,
                vm_dir,
            }
        }

        fn write_state(&self) {
            write_state(&self.path, &self.state).unwrap();
        }

        fn host_file(&self, path: &str) -> Option<String> {
            fs::read_to_string(self.host.join(path)).ok()
        }

        fn guest_file(&self, path: &str) -> Option<String> {
            fs::read_to_string(self.guest.join(path)).ok()
        }
    }

    fn recoveries(locked: &LockedState) -> Vec<String> {
        let Ok(entries) = fs::read_dir(locked.dir.join("recovery")) else {
            return vec![];
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn release_keeps_a_vm_another_session_still_leases() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(&locked, &mut guests, "review", &[("shared.rs", "before")]);
        vm.state.leases = vec!["work".to_owned(), "other".to_owned()];
        vm.write_state();
        fs::write(vm.guest.join("shared.rs"), "guest").unwrap();

        let warnings = release(
            &locked,
            &provider_config(root.path()),
            &guests,
            vm.path.clone(),
            vm.state.clone(),
            "work",
        )
        .unwrap();

        assert!(warnings.is_empty());
        assert_eq!(read_state(&vm.path).unwrap().leases, ["other"]);
        assert_eq!(
            vm.host_file("shared.rs").as_deref(),
            Some("before"),
            "nothing is merged while another session still has the VM"
        );
        assert!(vm.vm_dir.join("rootfs.ext4").exists());
        assert!(vm.state.api_socket.exists());
    }

    #[test]
    fn releasing_an_unready_vm_retains_its_disk_without_merging() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(&locked, &mut guests, "review", &[("edited.rs", "before")]);
        vm.state.ready = false;
        vm.write_state();
        fs::write(vm.guest.join("edited.rs"), "guest").unwrap();

        let warnings = release(
            &locked,
            &provider_config(root.path()),
            &guests,
            vm.path.clone(),
            vm.state.clone(),
            "work",
        )
        .unwrap();

        assert!(warnings.is_empty());
        assert_eq!(vm.host_file("edited.rs").as_deref(), Some("before"));
        assert!(!vm.path.exists());
        assert!(!vm.state.api_socket.exists());
        let retained = recoveries(&locked);
        assert_eq!(retained.len(), 1, "{retained:?}");
        assert!(retained[0].ends_with("-unready"), "{retained:?}");
        assert!(locked
            .dir
            .join("recovery")
            .join(&retained[0])
            .join("rootfs.ext4")
            .exists());
    }

    #[test]
    fn a_release_whose_pull_fails_retains_the_disk_and_reports_the_failure() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let vm = LeasedVm::new(&locked, &mut guests, "review", &[("edited.rs", "before")]);
        fs::write(vm.guest.join("edited.rs"), "guest").unwrap();
        guests.unreachable.insert("review".to_owned());

        let error = release(
            &locked,
            &provider_config(root.path()),
            &guests,
            vm.path.clone(),
            vm.state.clone(),
            "work",
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("injected pull failure"),
            "{error}"
        );
        assert_eq!(vm.host_file("edited.rs").as_deref(), Some("before"));
        assert!(!vm.path.exists());
        let retained = recoveries(&locked);
        assert_eq!(retained.len(), 1, "{retained:?}");
        assert!(retained[0].ends_with("-sync-failed"), "{retained:?}");
    }

    #[test]
    fn a_successful_release_merges_the_guest_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let vm = LeasedVm::new(
            &locked,
            &mut guests,
            "review",
            &[
                ("kept.rs", "same"),
                ("guest-edit.rs", "before"),
                ("both.rs", "before"),
            ],
        );
        fs::write(vm.guest.join("guest-edit.rs"), "guest").unwrap();
        fs::write(vm.guest.join("both.rs"), "guest").unwrap();
        fs::write(vm.host.join("both.rs"), "host").unwrap();

        let warnings = release(
            &locked,
            &provider_config(root.path()),
            &guests,
            vm.path.clone(),
            vm.state.clone(),
            "work",
        )
        .unwrap();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains(&vm.host.join("both.rs").display().to_string())
                && warnings[0].contains("both.rs.guest"),
            "{warnings:?}"
        );
        assert_eq!(vm.host_file("kept.rs").as_deref(), Some("same"));
        assert_eq!(vm.host_file("guest-edit.rs").as_deref(), Some("guest"));
        assert_eq!(vm.host_file("both.rs").as_deref(), Some("host"));
        assert_eq!(vm.host_file("both.rs.guest").as_deref(), Some("guest"));
        assert!(!vm.path.exists());
        assert!(
            !vm.vm_dir.exists(),
            "the disk and its directory are removed"
        );
        assert!(!vm.state.api_socket.exists());
        assert!(recoveries(&locked).is_empty());
    }

    #[test]
    fn stop_session_releases_every_vm_the_session_holds() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let owned = LeasedVm::new(&locked, &mut guests, "owned", &[("both.rs", "before")]);
        fs::write(owned.guest.join("both.rs"), "guest").unwrap();
        fs::write(owned.host.join("both.rs"), "host").unwrap();
        let mut shared = LeasedVm::new(&locked, &mut guests, "shared", &[("edited.rs", "before")]);
        shared.state.session = "other".to_owned();
        shared.state.leases = vec!["other".to_owned(), "work".to_owned()];
        shared.write_state();
        fs::write(shared.guest.join("edited.rs"), "guest").unwrap();
        let mut elsewhere = LeasedVm::new(&locked, &mut guests, "elsewhere", &[]);
        elsewhere.state.session = "other".to_owned();
        elsewhere.state.leases = vec!["other".to_owned()];
        elsewhere.write_state();

        let warnings =
            stop_session(&locked, &provider_config(root.path()), &guests, "work").unwrap();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("both.rs"), "{warnings:?}");
        assert!(!owned.path.exists());
        assert_eq!(owned.host_file("both.rs.guest").as_deref(), Some("guest"));
        assert_eq!(read_state(&shared.path).unwrap().leases, ["other"]);
        assert_eq!(shared.host_file("edited.rs").as_deref(), Some("before"));
        assert_eq!(read_state(&elsewhere.path).unwrap(), elsewhere.state);
        assert!(recoveries(&locked).is_empty());
    }

    #[test]
    fn stop_session_continues_past_a_failure_and_reports_the_first_error() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let broken = LeasedVm::new(&locked, &mut guests, "broken", &[("edited.rs", "before")]);
        let fine = LeasedVm::new(&locked, &mut guests, "fine", &[("edited.rs", "before")]);
        for vm in [&broken, &fine] {
            fs::write(vm.guest.join("edited.rs"), "guest").unwrap();
        }
        guests.unreachable.insert("broken".to_owned());

        let error =
            stop_session(&locked, &provider_config(root.path()), &guests, "work").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected pull failure for broken"),
            "{error}"
        );
        assert!(!fine.path.exists(), "the other VM was still released");
        assert_eq!(fine.host_file("edited.rs").as_deref(), Some("guest"));
        assert!(!broken.path.exists());
        assert_eq!(broken.host_file("edited.rs").as_deref(), Some("before"));
        let retained = recoveries(&locked);
        assert_eq!(retained.len(), 1, "{retained:?}");
        assert!(retained[0].ends_with("-sync-failed"), "{retained:?}");
    }

    /// A process that is not this test's child, so `terminate` without a `Child` in hand
    /// watches it leave `/proc` the way a Firecracker that outlived its provider does.
    struct LiveProcess {
        pid: i32,
        start_time: u64,
    }

    impl LiveProcess {
        fn spawn() -> Self {
            let output = Command::new("sh")
                .args(["-c", "sleep 60 </dev/null >/dev/null 2>&1 & echo $!"])
                .output()
                .unwrap();
            let pid = String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let start_time = wait_for_process_start(pid, Duration::from_secs(2)).unwrap();
            Self { pid, start_time }
        }

        fn attach(&self, vm: &mut LeasedVm) {
            vm.state.pid = self.pid;
            vm.state.start_time = self.start_time;
            vm.write_state();
        }
    }

    impl Drop for LiveProcess {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
        }
    }

    #[test]
    fn sync_merges_the_guest_pushes_the_result_back_and_rewrites_the_baseline() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(
            &locked,
            &mut guests,
            "review",
            &[("guest-edit.rs", "before"), ("host-edit.rs", "before")],
        );
        let live = LiveProcess::spawn();
        live.attach(&mut vm);
        fs::write(vm.guest.join("guest-edit.rs"), "guest").unwrap();
        fs::write(vm.host.join("host-edit.rs"), "host").unwrap();

        let warnings = sync_one(
            &locked,
            &provider_config(root.path()),
            &guests,
            "work",
            "review",
        )
        .unwrap();

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(vm.host_file("guest-edit.rs").as_deref(), Some("guest"));
        assert_eq!(vm.guest_file("host-edit.rs").as_deref(), Some("host"));
        assert_eq!(vm.guest_file("guest-edit.rs").as_deref(), Some("guest"));
        assert_eq!(
            read_baseline(&vm.state).unwrap(),
            worktree_baseline(&vm.host).unwrap(),
            "the merged tree is the new baseline"
        );
        assert!(vm.path.exists(), "the VM keeps running");
        assert!(vm_is_running(&vm.state));
    }

    /// The guest keeps working through a sync. What it writes between the pull and the push
    /// is not swept away by the push - only the paths the merge decided are sent - and the
    /// next sync merges it as the guest change it is.
    #[test]
    fn sync_leaves_what_the_guest_wrote_after_the_pull_and_pushes_only_the_merge() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(
            &locked,
            &mut guests,
            "review",
            &[
                ("host-edit.rs", "before"),
                ("gone.rs", "old"),
                ("both.rs", "before"),
            ],
        );
        let live = LiveProcess::spawn();
        live.attach(&mut vm);
        fs::write(vm.host.join("host-edit.rs"), "host").unwrap();
        fs::remove_file(vm.host.join("gone.rs")).unwrap();
        fs::write(vm.host.join("both.rs"), "host").unwrap();
        fs::write(vm.guest.join("both.rs"), "guest").unwrap();
        guests.late_writes.lock().unwrap().push((
            "review".to_owned(),
            "late.rs".to_owned(),
            "written after the pull".to_owned(),
        ));
        let config = provider_config(root.path());

        let warnings = sync_one(&locked, &config, &guests, "work", "review").unwrap();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(
            vm.guest_file("late.rs").as_deref(),
            Some("written after the pull"),
            "a file the guest wrote during the sync survives the push"
        );
        assert_eq!(vm.host_file("late.rs"), None);
        assert_eq!(vm.guest_file("host-edit.rs").as_deref(), Some("host"));
        assert_eq!(vm.guest_file("gone.rs"), None);
        assert_eq!(vm.guest_file("both.rs").as_deref(), Some("host"));
        assert_eq!(vm.guest_file("both.rs.guest").as_deref(), Some("guest"));
        let mut pushed = guests.pushed.lock().unwrap().clone();
        pushed.sort();
        assert_eq!(pushed, ["both.rs", "both.rs.guest", "host-edit.rs"]);
        assert_eq!(*guests.removed.lock().unwrap(), ["gone.rs"]);
        assert_eq!(
            read_baseline(&vm.state).unwrap(),
            worktree_baseline(&vm.host).unwrap()
        );

        let warnings = sync_one(&locked, &config, &guests, "work", "review").unwrap();

        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            vm.host_file("late.rs").as_deref(),
            Some("written after the pull"),
            "the next sync merges it"
        );
    }

    #[test]
    fn sync_refuses_an_unready_or_unknown_vm() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(&locked, &mut guests, "review", &[("edited.rs", "before")]);
        let live = LiveProcess::spawn();
        vm.state.ready = false;
        live.attach(&mut vm);
        fs::write(vm.guest.join("edited.rs"), "guest").unwrap();
        let config = provider_config(root.path());

        let error = sync_one(&locked, &config, &guests, "work", "review").unwrap_err();
        assert!(
            error.to_string().contains("still being provisioned"),
            "{error}"
        );
        let error = sync_one(&locked, &config, &guests, "work", "missing").unwrap_err();
        assert!(error.to_string().contains("is not running"), "{error}");
        let error = sync_one(&locked, &config, &guests, "other", "review").unwrap_err();
        assert!(error.to_string().contains("is not running"), "{error}");
        assert_eq!(vm.host_file("edited.rs").as_deref(), Some("before"));
    }

    #[test]
    fn requests_of_another_protocol_version_are_rejected() {
        let error = parse_request(br#"{"action":"list","version":999}"#).unwrap_err();
        assert!(error.to_string().contains("protocol mismatch"), "{error}");
        assert!(parse_request(b"not json").is_err());
        let request = parse_request(
            format!(r#"{{"action":"list","version":{PROTOCOL_VERSION}}}"#).as_bytes(),
        )
        .unwrap();
        assert!(matches!(request, Request::List { session: None, .. }));
    }

    #[test]
    fn list_requests_answer_with_the_vms_and_the_recoveries() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut running = LeasedVm::new(&locked, &mut guests, "review", &[]);
        let live = LiveProcess::spawn();
        live.attach(&mut running);
        // A VM whose process is gone is retained before the request is answered.
        LeasedVm::new(&locked, &mut guests, "crashed", &[]);

        let response = handle_request(
            &locked,
            &provider_config(root.path()),
            &network_config(),
            &guests,
            Request::List {
                version: PROTOCOL_VERSION,
                session: None,
            },
        )
        .unwrap();

        assert_eq!(response["version"], PROTOCOL_VERSION);
        assert_eq!(
            response["vms"],
            json!([{ "session": "work", "name": "review", "state": "running" }])
        );
        let recoveries = response["recoveries"].as_array().unwrap();
        assert_eq!(recoveries.len(), 1, "{recoveries:?}");
        assert_eq!(recoveries[0]["reason"], "crashed");
        assert_eq!(recoveries[0]["name"], "crashed");
        assert_eq!(recoveries[0]["session"], "work");
    }

    #[test]
    fn rename_requests_move_the_session_state() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let mut guests = FakeGuests::default();
        let mut vm = LeasedVm::new(&locked, &mut guests, "review", &[]);
        let live = LiveProcess::spawn();
        live.attach(&mut vm);

        let response = handle_request(
            &locked,
            &provider_config(root.path()),
            &network_config(),
            &guests,
            Request::RenameSession {
                version: PROTOCOL_VERSION,
                from: "work".into(),
                to: "renamed".into(),
            },
        )
        .unwrap();

        assert_eq!(response, json!({ "version": PROTOCOL_VERSION }));
        assert!(!vm.path.exists());
        let moved = read_state(&locked.path("renamed", "review")).unwrap();
        assert_eq!(moved.session, "renamed");
        assert_eq!(moved.leases, ["renamed"]);
    }

    #[test]
    fn stop_and_sync_requests_answer_with_their_warnings() {
        let root = tempfile::tempdir().unwrap();
        let locked = locked_state_in(root.path());
        let config = provider_config(root.path());
        let network = network_config();
        let mut guests = FakeGuests::default();
        let mut first = LeasedVm::new(&locked, &mut guests, "first", &[("both.rs", "before")]);
        let first_live = LiveProcess::spawn();
        first_live.attach(&mut first);
        fs::write(first.guest.join("both.rs"), "guest").unwrap();
        fs::write(first.host.join("both.rs"), "host").unwrap();
        let mut second = LeasedVm::new(&locked, &mut guests, "second", &[("both.rs", "before")]);
        let second_live = LiveProcess::spawn();
        second_live.attach(&mut second);
        fs::write(second.guest.join("both.rs"), "guest").unwrap();
        fs::write(second.host.join("both.rs"), "host").unwrap();

        let synced = handle_request(
            &locked,
            &config,
            &network,
            &guests,
            Request::Sync {
                version: PROTOCOL_VERSION,
                session: "work".into(),
                name: "first".into(),
            },
        )
        .unwrap();
        let warnings = synced["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{synced}");
        assert!(
            warnings[0].as_str().unwrap().contains("both.rs"),
            "{synced}"
        );
        assert!(vm_is_running(&first.state), "a sync keeps the VM running");

        let stopped = handle_request(
            &locked,
            &config,
            &network,
            &guests,
            Request::Stop {
                version: PROTOCOL_VERSION,
                session: "work".into(),
                name: "first".into(),
            },
        )
        .unwrap();
        assert_eq!(
            stopped,
            json!({ "version": PROTOCOL_VERSION, "warnings": [] })
        );
        assert!(!first.path.exists());
        assert!(!vm_is_running(&first.state), "a stop terminates the VM");

        let stopped = handle_request(
            &locked,
            &config,
            &network,
            &guests,
            Request::StopSession {
                version: PROTOCOL_VERSION,
                session: "work".into(),
            },
        )
        .unwrap();
        let warnings = stopped["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{stopped}");
        assert!(
            warnings[0].as_str().unwrap().contains("both.rs"),
            "{stopped}"
        );
        assert!(!second.path.exists());
        assert!(!vm_is_running(&second.state));
        assert!(locked.entries().unwrap().is_empty());
    }
}
