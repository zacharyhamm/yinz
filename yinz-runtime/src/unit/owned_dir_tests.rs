use super::*;
use std::io::{BufRead, Read};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn private_parent() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

struct Owner(Child);

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_owner(parent: &Path) -> (Owner, mpsc::Receiver<PathBuf>) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "owned_dir::tests::subprocess_owner",
            "--nocapture",
        ])
        .env("YINZ_OWNED_DIR_TEST_PARENT", parent)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if let Some(path) = line.strip_prefix("READY ") {
                send.send(PathBuf::from(path)).unwrap();
            }
        }
    });
    (Owner(child), receive)
}

#[test]
fn subprocess_owner() {
    let Some(parent) = std::env::var_os("YINZ_OWNED_DIR_TEST_PARENT") else {
        return;
    };
    let root = OwnedDir::new(Path::new(&parent)).unwrap();
    fs::write(root.path().join("data"), "in use").unwrap();
    println!("READY {}", root.path().display());
    let _ = io::stdin().read(&mut [0]);
}

#[test]
fn concurrent_creation_preserves_live_roots_and_reclaims_killed_owners() {
    let parent = private_parent();
    let children: Vec<_> = (0..4).map(|_| spawn_owner(parent.path())).collect();
    let roots: Vec<_> = children
        .iter()
        .map(|(_, ready)| ready.recv_timeout(Duration::from_secs(30)).unwrap())
        .collect();
    for _ in 0..10 {
        let root = OwnedDir::new(parent.path()).unwrap();
        for path in &roots {
            assert_eq!(fs::read_to_string(path.join("data")).unwrap(), "in use");
        }
        drop(root);
    }
    drop(children); // SIGKILL each owner, then reap it: no destructor could run.
    let root = OwnedDir::new(parent.path()).unwrap();
    for path in roots {
        assert!(!path.exists(), "abandoned root: {path:?}");
    }
    drop(root);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1); // permanent registry lock
}

#[test]
fn last_user_removes_child_then_host_even_during_unwinding() {
    let parent = private_parent();
    let root = OwnedDir::new(parent.path()).unwrap();
    let root_path = root.path().to_path_buf();
    let one = root.child().unwrap();
    let one_path = one.path().to_path_buf();
    let two = root.child().unwrap();
    let user = one.clone();
    drop(one);
    assert!(one_path.exists());
    drop(user);
    assert!(!one_path.exists());
    assert!(two.path().exists());
    drop(root);
    assert!(root_path.exists());
    let _ = std::panic::catch_unwind(|| {
        let _two = two;
        panic!("host error");
    });
    assert!(!root_path.exists());
}

#[test]
fn inherited_lease_protects_children_after_runner_disappears() {
    let parent = private_parent();
    let root = OwnedDir::new(parent.path()).unwrap();
    root.inherit_lock().unwrap();
    let path = root.path().to_path_buf();
    let mut child = Owner(
        Command::new("sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    // Model a dead runner: close its descriptor without running directory cleanup.
    let mut root = std::mem::ManuallyDrop::new(Arc::try_unwrap(root).unwrap());
    drop(root.lease.take());
    drop(OwnedDir::new(parent.path()).unwrap());
    assert!(
        path.exists(),
        "a live child still holds the inherited lease"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    drop(OwnedDir::new(parent.path()).unwrap());
    assert!(!path.exists());
}

#[test]
fn reclamation_does_not_follow_symlinks_or_sweep_unmarked_directories() {
    use std::os::unix::fs::symlink;
    let parent = private_parent();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), "persistent").unwrap();
    let unmarked = parent.path().join("eaa93fef-8f25-40eb-81b1-1c4e03321070");
    fs::create_dir(&unmarked).unwrap();
    symlink(outside.path(), parent.path().join("rsymlink")).unwrap();
    File::create(parent.path().join("rsymlink.lease")).unwrap();
    symlink(
        outside.path().join("keep"),
        parent.path().join("rfile.lease"),
    )
    .unwrap();
    let root = OwnedDir::new(parent.path()).unwrap();
    symlink(outside.path(), root.path().join("nested")).unwrap();
    drop(root);
    assert_eq!(
        fs::read_to_string(outside.path().join("keep")).unwrap(),
        "persistent"
    );
    assert!(unmarked.exists());
    assert!(parent.path().join("rfile.lease").is_symlink());
}

#[test]
fn externally_removed_directories_are_already_clean() {
    let parent = private_parent();
    let root = OwnedDir::new(parent.path()).unwrap();
    fs::remove_dir_all(root.path()).unwrap();
    drop(root);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn registry_symlinks_and_writable_shared_parents_are_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let parent = private_parent();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(OwnedDir::new(parent.path()).is_err());
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "persistent").unwrap();
    symlink(outside.path(), parent.path().join(".registry.lock")).unwrap();
    assert!(OwnedDir::new(parent.path()).is_err());
    assert_eq!(fs::read_to_string(outside.path()).unwrap(), "persistent");
}
