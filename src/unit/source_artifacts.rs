use super::*;

fn artifact(bytes: &[u8]) -> SourceArtifact {
    SourceArtifact {
        target: "aarch64-unknown-linux-gnu".into(),
        sha256: update::hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
        binary_sha256: "0".repeat(64),
    }
}

#[test]
fn source_chunks_resume_and_duplicate_without_rewriting_pinned_artifacts() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let bytes = b"first chunk and second chunk";
    let artifact = artifact(bytes);
    let first = receive(data.path(), &operation, &artifact, 0, &bytes[..10]).unwrap();
    assert_eq!(first["offset"], 10);
    assert_eq!(first["complete"], false);
    assert_eq!(
        receive(data.path(), &operation, &artifact, 0, &bytes[..10]).unwrap(),
        first
    );
    let final_reply = receive(data.path(), &operation, &artifact, 10, &bytes[10..]).unwrap();
    assert_eq!(final_reply["complete"], true);
    assert_eq!(
        receive(data.path(), &operation, &artifact, 10, &bytes[10..]).unwrap(),
        final_reply
    );
    assert_eq!(
        fs::read(archive_path(data.path(), &operation, &artifact.target).unwrap()).unwrap(),
        bytes
    );
}

#[test]
fn source_chunks_reject_wrong_offsets_content_length_and_changed_identity() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let artifact = artifact(b"abcdef");
    assert!(receive(data.path(), &operation, &artifact, 1, b"bc").is_err());
    receive(data.path(), &operation, &artifact, 0, b"abc").unwrap();
    assert!(receive(data.path(), &operation, &artifact, 0, b"abd").is_err());
    assert!(receive(data.path(), &operation, &artifact, 2, b"cd").is_err());
    assert!(receive(data.path(), &operation, &artifact, 3, b"defg").is_err());
    assert!(receive(data.path(), &operation, &artifact, u64::MAX, b"d").is_err());
    let mut changed = artifact.clone();
    changed.binary_sha256 = "1".repeat(64);
    assert!(receive(data.path(), &operation, &changed, 3, b"def").is_err());
    receive(data.path(), &operation, &artifact, 3, b"def").unwrap();
}

#[test]
fn source_transfer_recovers_unacknowledged_partial_writes() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let artifact = artifact(b"abcdef");
    receive(data.path(), &operation, &artifact, 0, b"abc").unwrap();
    let archive = archive_path(data.path(), &operation, &artifact.target).unwrap();
    let mut partial = OpenOptions::new()
        .append(true)
        .open(archive.with_extension("part"))
        .unwrap();
    partial.write_all(b"d").unwrap();
    partial.sync_all().unwrap();
    drop(partial);
    receive(data.path(), &operation, &artifact, 3, b"def").unwrap();
    assert_eq!(fs::read(archive).unwrap(), b"abcdef");
}

#[test]
fn source_transfer_never_publishes_corrupt_bytes() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let artifact = artifact(b"expected");
    assert!(receive(data.path(), &operation, &artifact, 0, b"corrupt!")
        .unwrap_err()
        .to_string()
        .contains("SHA-256"));
    assert!(!archive_path(data.path(), &operation, &artifact.target)
        .unwrap()
        .exists());
}

#[test]
fn source_transfer_bounds_requests_and_rejects_path_traversal() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut artifact = artifact(&vec![0; CHUNK_SIZE + 1]);
    assert!(receive(
        data.path(),
        &operation,
        &artifact,
        0,
        &vec![0; CHUNK_SIZE + 1]
    )
    .is_err());
    assert!(receive(data.path(), &operation, &artifact, 0, &[]).is_err());
    assert!(receive(data.path(), "../escape", &artifact, 0, b"x").is_err());
    artifact.size = MAX_SIZE + 1;
    assert!(receive(data.path(), &operation, &artifact, 0, b"x").is_err());
    artifact.size = 1;
    artifact.target = "../../escape".into();
    assert!(receive(data.path(), &operation, &artifact, 0, b"x").is_err());
    assert_eq!(fs::read_dir(data.path()).unwrap().count(), 0);
}

fn bundle(machine: u16) -> (Vec<u8>, Vec<u8>) {
    let mut binary = vec![0; 32];
    binary[..6].copy_from_slice(b"\x7fELF\x02\x01");
    binary[18..20].copy_from_slice(&machine.to_le_bytes());
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    for (name, bytes) in [
        ("yinz", binary.as_slice()),
        (
            "plugins/builtins/fixture.scm",
            b"(define pinned #t)".as_slice(),
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes).unwrap();
    }
    (archive.into_inner().unwrap().finish().unwrap(), binary)
}

fn stage_bundle(data: &Path, operation: &str, machine: u16) -> update::Release {
    let (bytes, binary) = bundle(machine);
    let mut artifact = artifact(&bytes);
    artifact.binary_sha256 = update::hex(&Sha256::digest(&binary));
    receive(data, operation, &artifact, 0, &bytes).unwrap();
    update::Release {
        tag_name: "0.1.1".into(),
        assets: vec![],
        source: Some(update::SourceRelease {
            commit: "a".repeat(40),
            artifacts: vec![artifact],
        }),
    }
}

#[test]
fn source_preparation_validates_foreign_binary_and_matching_builtins_without_execution() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let release = stage_bundle(data.path(), &operation, 183);
    let prepared = prepare_artifact(
        &release,
        "aarch64-unknown-linux-gnu",
        data.path(),
        &operation,
    )
    .unwrap();
    assert_eq!(
        fs::read(prepared.source.join("plugins/builtins/fixture.scm")).unwrap(),
        b"(define pinned #t)"
    );
    let mut corrupt = release.clone();
    corrupt.source.as_mut().unwrap().artifacts[0].binary_sha256 = "1".repeat(64);
    assert!(prepare_artifact(
        &corrupt,
        "aarch64-unknown-linux-gnu",
        data.path(),
        &operation
    )
    .err()
    .unwrap()
    .to_string()
    .contains("executable SHA-256"));
    let archive = archive_path(data.path(), &operation, "aarch64-unknown-linux-gnu").unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    bytes[0] ^= 1;
    fs::write(&archive, bytes).unwrap();
    assert!(prepare_artifact(
        &release,
        "aarch64-unknown-linux-gnu",
        data.path(),
        &operation
    )
    .err()
    .unwrap()
    .to_string()
    .contains("archive SHA-256"));
}

#[test]
fn source_preparation_refuses_a_correctly_hashed_wrong_architecture() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let release = stage_bundle(data.path(), &operation, 62);
    assert!(prepare_artifact(
        &release,
        "aarch64-unknown-linux-gnu",
        data.path(),
        &operation
    )
    .err()
    .unwrap()
    .to_string()
    .contains("architecture"));
}

#[test]
fn local_staging_resumes_partial_and_skips_completed_archives_after_acknowledgement() {
    // Count actual bytes read so replaying every completed chunk cannot pass.
    struct TrackedInput {
        cursor: std::io::Cursor<Vec<u8>>,
        read: usize,
    }
    impl Read for TrackedInput {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let count = self.cursor.read(bytes)?;
            self.read += count;
            Ok(count)
        }
    }
    impl Seek for TrackedInput {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.cursor.seek(position)
        }
    }
    let bytes: Vec<_> = (0..CHUNK_SIZE * 3 + 17).map(|i| (i % 251) as u8).collect();
    let artifact = artifact(&bytes);
    for completed in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let staged = if completed {
            bytes.len()
        } else {
            CHUNK_SIZE * 2
        };
        for (index, chunk) in bytes[..staged].chunks(CHUNK_SIZE).enumerate() {
            receive(
                data.path(),
                &operation,
                &artifact,
                (index * CHUNK_SIZE) as u64,
                chunk,
            )
            .unwrap();
        }
        let mut input = TrackedInput {
            cursor: std::io::Cursor::new(bytes.clone()),
            read: 0,
        };
        stage_local(data.path(), &operation, &artifact, &mut input).unwrap();
        assert_eq!(input.read, CHUNK_SIZE + bytes.len() - staged);
        let archive = archive_path(data.path(), &operation, &artifact.target).unwrap();
        assert_eq!(fs::read(&archive).unwrap(), bytes);
        let mut corrupt = bytes.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        fs::write(archive, corrupt).unwrap();
        input.cursor.set_position(0);
        assert!(stage_local(data.path(), &operation, &artifact, &mut input)
            .unwrap_err()
            .to_string()
            .contains("SHA-256"));
    }
}

#[test]
fn local_staging_rejects_truncated_input_and_corrupt_partial_archives() {
    let data = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let bytes = vec![42; CHUNK_SIZE * 2];
    let artifact = artifact(&bytes);
    let mut truncated = std::io::Cursor::new(&bytes[..CHUNK_SIZE]);
    assert!(
        stage_local(data.path(), &operation, &artifact, &mut truncated)
            .unwrap_err()
            .to_string()
            .contains("length changed")
    );
    let archive = archive_path(data.path(), &operation, &artifact.target).unwrap();
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    fs::write(archive.with_extension("part"), &corrupt[..CHUNK_SIZE]).unwrap();
    assert!(stage_local(
        data.path(),
        &operation,
        &artifact,
        &mut std::io::Cursor::new(bytes)
    )
    .is_err());
    assert!(!archive.exists());
}
