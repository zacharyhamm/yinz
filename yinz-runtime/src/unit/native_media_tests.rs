use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

fn lease(bytes: usize) -> MediaLease {
    MediaBudget::viewer().try_reserve(bytes).unwrap()
}

fn staged(directory: &Path) -> Vec<PathBuf> {
    directory
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(IMAGE_PREFIX))
        })
        .collect()
}

#[test]
fn render_negotiation_media_budget_accounts_reservations_and_releases() {
    let budget = MediaBudget::new(3, 100);
    let first = budget.try_reserve(40).unwrap();
    let second = budget.clone().try_reserve(60).unwrap();
    assert_eq!(
        budget.usage(),
        MediaUsage {
            entries: 2,
            bytes: 100
        }
    );
    assert_eq!(budget.try_reserve(1).unwrap_err(), Refusal::Bytes);
    drop(first);
    assert_eq!(
        budget.usage(),
        MediaUsage {
            entries: 1,
            bytes: 60
        }
    );
    let third = budget.try_reserve(0).unwrap();
    let fourth = budget.try_reserve(40).unwrap();
    assert_eq!(budget.try_reserve(0).unwrap_err(), Refusal::Entries);
    drop((second, third, fourth));
    assert_eq!(budget.usage(), MediaUsage::default());
    assert_eq!(
        budget.limit(),
        MediaUsage {
            entries: 3,
            bytes: 100
        }
    );
}

#[test]
fn render_negotiation_media_budget_refuses_what_can_never_fit() {
    let budget = MediaBudget::new(4, 100);
    assert_eq!(budget.try_reserve(101).unwrap_err(), Refusal::TooLarge);
    let viewer = MediaBudget::viewer();
    assert_eq!(
        viewer.try_reserve(MAX_SHARED_IMAGE_BYTES + 1).unwrap_err(),
        Refusal::TooLarge
    );
    // A viewer with nothing outstanding always fits the largest readable image.
    let largest = viewer.try_reserve(MAX_SHARED_IMAGE_BYTES).unwrap();
    assert_eq!(
        viewer
            .try_reserve(VIEWER_MEDIA_BYTES - MAX_SHARED_IMAGE_BYTES + 1)
            .unwrap_err(),
        Refusal::Bytes
    );
    drop(largest);
    assert_eq!(
        MediaBudget::new(0, 100).try_reserve(0).unwrap_err(),
        Refusal::TooLarge
    );
    assert_eq!(budget.usage(), MediaUsage::default());
}

#[test]
fn render_negotiation_media_budget_ties_the_lease_to_the_staged_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = MediaRoot::open(directory.path()).unwrap();
    let budget = MediaBudget::new(1, 8);
    let image = root
        .write_image(&[1, 2, 3, 4], budget.try_reserve(4).unwrap())
        .unwrap();
    let path = image.path();
    assert!(path.is_file());
    assert_eq!(budget.usage().entries, 1);
    // A refused reservation never reaches the directory.
    assert_eq!(budget.try_reserve(4).unwrap_err(), Refusal::Entries);
    assert_eq!(staged(directory.path()), std::slice::from_ref(&path));
    drop(image);
    assert!(!path.exists());
    assert_eq!(budget.usage(), MediaUsage::default());
    // A lease for a different length is rejected before any file is created.
    let error = root
        .write_image(&[1, 2], budget.try_reserve(4).unwrap())
        .err()
        .expect("mismatched lease");
    assert!(error.to_string().contains("covers 4 bytes"), "{error}");
    assert!(staged(directory.path()).is_empty());
    assert_eq!(budget.usage(), MediaUsage::default());
}

#[test]
fn render_negotiation_media_budget_failed_write_returns_the_lease() {
    let directory = tempfile::tempdir().unwrap();
    let root = MediaRoot::open(directory.path()).unwrap();
    let budget = MediaBudget::new(2, 8);
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = root.write_image(&[1], budget.try_reserve(1).unwrap());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    // Root bypasses directory permissions; the accounting holds either way.
    drop(result);
    assert!(staged(directory.path()).is_empty());
    assert_eq!(budget.usage(), MediaUsage::default());
}

#[test]
fn render_negotiation_media_budget_viewers_are_independent() {
    let first = MediaBudget::new(1, 10);
    let second = MediaBudget::new(1, 10);
    let held = first.try_reserve(10).unwrap();
    assert_eq!(first.try_reserve(1).unwrap_err(), Refusal::Entries);
    let other = second.try_reserve(10).unwrap();
    assert_eq!(second.usage().bytes, 10);
    drop(held);
    assert_eq!(first.usage(), MediaUsage::default());
    assert_eq!(second.usage().bytes, 10);
    drop(other);
}

#[test]
fn render_negotiation_rejects_an_image_with_masked_owner_permissions() {
    const SERVER_ROOT: &str = "YINZ_TEST_MASKED_IMAGE_ROOT";
    if let Some(path) = std::env::var_os(SERVER_ROOT) {
        let root = MediaRoot::open(Path::new(&path)).unwrap();
        nix::sys::stat::umask(Mode::from_bits_truncate(0o477));
        let budget = MediaBudget::viewer();
        let error = root
            .write_image(&[1], budget.try_reserve(1).unwrap())
            .err()
            .expect("reject mode 0200");
        assert!(error.to_string().contains("requires mode 0600"), "{error}");
        // The rejected file was created first; it is unlinked and its lease returned.
        assert!(staged(Path::new(&path)).is_empty());
        assert_eq!(budget.usage(), MediaUsage::default());
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let probe = Probe::create(directory.path()).unwrap();
    let root = MediaRoot::open(directory.path()).unwrap();
    assert_eq!(root.answer(&probe.offer).unwrap(), probe.expected);
    // The server has its own process-wide umask, independent of the probe's creator.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", std::thread::current().name().unwrap()])
        .env(SERVER_ROOT, directory.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(probe);
    assert!(directory.path().read_dir().unwrap().next().is_none());
}

#[test]
fn render_negotiation_proof_and_image_use_the_pinned_directory() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("root");
    let alias = temp.path().join("alias");
    std::fs::create_dir(&directory).unwrap();
    symlink(&directory, &alias).unwrap();
    let probe = Probe::create(&alias).unwrap();
    let root = MediaRoot::open(&directory).unwrap();
    assert_eq!(root.answer(&probe.offer).unwrap(), probe.expected);
    assert!(MediaRoot::open(temp.path())
        .unwrap()
        .answer(&probe.offer)
        .is_err());
    let mut invalid = probe.offer.clone();
    invalid.challenge[0] ^= 1;
    assert!(root.answer(&invalid).is_err());
    invalid.basename = "../elsewhere".into();
    assert!(root.answer(&invalid).is_err());

    let image = root.write_image(&[1, 2, 3, 4], lease(4)).unwrap();
    let image_path = image.path();
    let path = image_path.to_str().unwrap().as_bytes();
    assert_eq!(probe.root.read_image(path, 4).unwrap(), [1, 2, 3, 4]);
    assert!(probe.root.read_image(path, 8).is_err());
    std::fs::set_permissions(image.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(probe.root.read_image(path, 4).is_err());
    std::fs::set_permissions(image.path(), std::fs::Permissions::from_mode(0o600)).unwrap();

    let outside = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
    std::fs::write(outside.path(), [9, 8, 7, 6]).unwrap();
    let link = directory.join("yinz-native-image-abcdef");
    symlink(outside.path(), &link).unwrap();
    assert!(probe
        .root
        .read_image(link.to_str().unwrap().as_bytes(), 4)
        .is_err());
    std::fs::remove_file(&link).unwrap();
    nix::unistd::mkfifo(&link, nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert!(probe
        .root
        .read_image(link.to_str().unwrap().as_bytes(), 0)
        .is_err());
    std::fs::remove_file(&link).unwrap();
    std::fs::create_dir(&link).unwrap();
    assert!(probe
        .root
        .read_image(link.to_str().unwrap().as_bytes(), 0)
        .is_err());
    std::fs::remove_dir(&link).unwrap();
    let original = temp.path().join("original");
    std::fs::rename(&directory, &original).unwrap();
    std::fs::create_dir(&directory).unwrap();
    symlink(outside.path(), image.path()).unwrap();
    assert_eq!(probe.root.read_image(path, 4).unwrap(), [1, 2, 3, 4]);
    let new_image = root.write_image(&[5], lease(1)).unwrap();
    assert_eq!(
        probe
            .root
            .read_image(new_image.path().to_str().unwrap().as_bytes(), 1)
            .unwrap(),
        [5]
    );
    assert!(!new_image.path().exists());

    // A probe created after replacement must still prove the descriptor it will retain.
    let new_probe = Probe::create_in(root.clone()).unwrap();
    assert_eq!(root.answer(&new_probe.offer).unwrap(), new_probe.expected);
    assert!(MediaRoot::open(&directory)
        .unwrap()
        .answer(&new_probe.offer)
        .is_err());

    let replacement_probe = directory.join(&probe.offer.basename);
    std::fs::write(&replacement_probe, b"replacement").unwrap();
    drop((probe, new_probe, image, new_image));
    assert!(original.read_dir().unwrap().next().is_none());
    assert_eq!(std::fs::read(&replacement_probe).unwrap(), b"replacement");
    assert_eq!(std::fs::read_link(image_path).unwrap(), outside.path());
}

#[test]
fn native_media_portable_frame_allowance_fits_the_ipc_frame_with_headroom() {
    let limit = crate::consts::MAX_IPC_FRAME_LEN;
    assert!(PORTABLE_FRAME_MEDIA_BYTES + PORTABLE_FRAME_HEADROOM_BYTES <= limit);
    // Every transfer the reader accepts spans whole frames: the largest image takes more
    // than one, and a threshold-sized image fits in one.
    const { assert!(MAX_SHARED_IMAGE_BYTES > PORTABLE_FRAME_MEDIA_BYTES) };
    const { assert!(SHARED_MEDIA_MIN_BYTES <= PORTABLE_FRAME_MEDIA_BYTES) };
    assert_eq!(
        MAX_SHARED_IMAGE_BYTES.div_ceil(PORTABLE_FRAME_MEDIA_BYTES),
        13
    );
}
