use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

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

    let image = root.write_image(&[1, 2, 3, 4]).unwrap();
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
    let new_image = root.write_image(&[5]).unwrap();
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
