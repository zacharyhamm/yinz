use super::*;

fn image(identifier: [u8; 16]) -> Vec<u8> {
    let mut image = vec![0; 56];
    for (offset, value) in [
        (0, 0xfeed_facfu32),
        (4, 0x0100_0007),
        (8, 3),
        (16, 1),
        (20, 24),
        (32, 0x1b),
        (36, 24),
    ] {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    image[40..56].copy_from_slice(&identifier);
    image
}

#[test]
fn worker_generation_identity_reads_thin_and_universal_macho_builds() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("yinz");
    let thin = image([7; 16]);
    std::fs::write(&binary, &thin).unwrap();
    assert_eq!(
        uuid(&read_image(&binary, 0x0100_0007, 3).unwrap()).unwrap(),
        [7; 16]
    );
    for (magic, table_size) in [(0xcafe_babeu32, 20), (0xcafe_babf, 32)] {
        let mut fat = vec![0; 128];
        fat[..4].copy_from_slice(&magic.to_be_bytes());
        fat[4..8].copy_from_slice(&1u32.to_be_bytes());
        fat[8..12].copy_from_slice(&0x0100_0007u32.to_be_bytes());
        fat[12..16].copy_from_slice(&3u32.to_be_bytes());
        if table_size == 20 {
            fat[16..20].copy_from_slice(&128u32.to_be_bytes());
        } else {
            fat[16..24].copy_from_slice(&128u64.to_be_bytes());
        }
        fat.extend_from_slice(&thin);
        std::fs::write(&binary, fat).unwrap();
        assert_eq!(
            uuid(&read_image(&binary, 0x0100_0007, 3).unwrap()).unwrap(),
            [7; 16]
        );
        assert!(read_image(&binary, 0x0100_000c, 0).is_err());
    }
}

#[test]
fn worker_generation_identity_rejects_missing_or_malformed_macho_ids() {
    let mut missing = image([3; 16]);
    missing[32..36].copy_from_slice(&1u32.to_le_bytes());
    assert!(uuid(&missing).is_err());
    let mut malformed = image([3; 16]);
    malformed[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(uuid(&malformed).is_err());
    assert!(uuid(&image([3; 16])[..50]).is_err());
    assert_ne!(
        uuid(&image([3; 16])).unwrap(),
        uuid(&image([4; 16])).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn worker_generation_identity_rejects_current_exe_resolved_after_exchange() {
    let root = tempfile::tempdir().unwrap();
    let backup = root.path().join(".yinz-update-example");
    std::fs::create_dir(&backup).unwrap();
    let moved_running = backup.join("binary");
    std::fs::write(&moved_running, b"old").unwrap();
    // Linux updates /proc/self/exe to the displaced inode's backup pathname.
    // First resolving the installed path after exchange must not validate that
    // inode against itself while builtin loading reads the newer installation.
    assert!(verify_linux_paths(&moved_running, &moved_running).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn worker_generation_identity_detects_installation_exchange_before_startup() {
    let root = tempfile::tempdir().unwrap();
    let running = root.path().join("running");
    let installed = root.path().join("installed");
    std::fs::write(&running, b"old").unwrap();
    std::fs::hard_link(&running, &installed).unwrap();
    verify_linux_paths(&installed, &running).unwrap();
    std::fs::remove_file(&installed).unwrap();
    std::fs::write(&installed, b"new").unwrap();
    assert!(verify_linux_paths(&installed, &running).is_err());
}
