//! Bounded, resumable source bundles carried by authenticated update requests.
use super::{journal, update};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::update_client::SourceArtifact;

pub(super) const CHUNK_SIZE: usize = 64 * 1024;
const MAX_SIZE: u64 = 512 * 1024 * 1024;

fn validate(artifact: &SourceArtifact) -> Result<()> {
    anyhow::ensure!(
        matches!(
            artifact.target.as_str(),
            "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu"
        ),
        "source artifacts support only Linux x86_64 and aarch64"
    );
    anyhow::ensure!(
        artifact.size > 0 && artifact.size <= MAX_SIZE,
        "source archive length is invalid"
    );
    for digest in [&artifact.sha256, &artifact.binary_sha256] {
        anyhow::ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "source artifact digest must be lowercase SHA-256"
        );
    }
    Ok(())
}

pub(super) fn digest_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("open source artifact for hashing")?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; CHUNK_SIZE];
    loop {
        let count = file
            .read(&mut buffer)
            .context("read source artifact for hashing")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(update::hex(&hash.finalize()))
}

pub(super) fn archive_path(data_dir: &Path, operation: &str, target: &str) -> Result<PathBuf> {
    uuid::Uuid::parse_str(operation).context("invalid source artifact operation")?;
    anyhow::ensure!(
        matches!(
            target,
            "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu"
        ),
        "unsupported source artifact target"
    );
    Ok(data_dir
        .join("updates")
        .join(operation)
        .join("source-artifacts")
        .join(format!("{target}.tar.gz")))
}

fn open_transfer(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .context("open source artifact transfer file")
}

fn check_archive(path: &Path, artifact: &SourceArtifact) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect staged source archive")?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() == artifact.size,
        "source archive length does not match pinned metadata"
    );
    anyhow::ensure!(
        digest_file(path)? == artifact.sha256,
        "source archive SHA-256 mismatch"
    );
    Ok(())
}

pub(super) fn receive(
    data_dir: &Path,
    operation: &str,
    artifact: &SourceArtifact,
    offset: u64,
    bytes: &[u8],
) -> Result<serde_json::Value> {
    validate(artifact)?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= CHUNK_SIZE,
        "source chunk length must be 1..=64 KiB"
    );
    let end = offset
        .checked_add(bytes.len() as u64)
        .context("source chunk offset overflow")?;
    anyhow::ensure!(
        end <= artifact.size,
        "source chunk exceeds pinned archive length"
    );
    let archive = archive_path(data_dir, operation, &artifact.target)?;
    let directory = archive
        .parent()
        .context("source archive has no directory")?;
    yinz_runtime::shared::ensure_secure_dir(directory).context("secure source artifact staging")?;
    let lock = open_transfer(&archive.with_extension("lock"))?;
    lock.try_lock()
        .context("source artifact transfer is busy")?;
    let metadata_path = archive.with_extension("json");
    let pinned = serde_json::to_value(artifact).context("encode source artifact identity")?;
    match fs::read(&metadata_path) {
        Ok(bytes) => {
            let previous: serde_json::Value =
                serde_json::from_slice(&bytes).context("decode pinned source artifact identity")?;
            anyhow::ensure!(
                previous == pinned,
                "source artifact metadata changed during transfer"
            );
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            journal::atomic_json(&metadata_path, &pinned)?
        },
        Err(error) => return Err(error).context("read pinned source artifact identity"),
    }
    let complete = archive
        .try_exists()
        .context("inspect completed source transfer")?;
    if complete {
        check_archive(&archive, artifact)?;
    }
    let partial = archive.with_extension("part");
    let progress_path = archive.with_extension("progress");
    let mut file = if complete {
        OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(&archive)
            .context("open completed source archive")?
    } else {
        open_transfer(&partial)?
    };
    let mut length = file
        .metadata()
        .context("read source transfer length")?
        .len();
    if !complete {
        let durable: u64 = match fs::read(&progress_path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("decode durable source transfer offset")?
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("read durable source transfer offset"),
        };
        anyhow::ensure!(
            durable <= artifact.size && length >= durable,
            "source transfer is shorter than its durable offset"
        );
        if length > durable {
            // A process may die midway through write_all, or after fsync but
            // before acknowledging the chunk. Retry from the durable boundary.
            file.set_len(durable)
                .context("discard interrupted source chunk tail")?;
            file.sync_all()
                .context("persist interrupted source chunk recovery")?;
            length = durable;
        }
    }
    anyhow::ensure!(
        length <= artifact.size,
        "source transfer exceeds pinned archive length"
    );
    if offset < length {
        anyhow::ensure!(end <= length, "source chunk overlaps the transfer boundary");
        file.seek(SeekFrom::Start(offset))
            .context("seek duplicate source chunk")?;
        let mut previous = vec![0; bytes.len()];
        file.read_exact(&mut previous)
            .context("read duplicate source chunk")?;
        anyhow::ensure!(
            previous == bytes,
            "retried source chunk differs from staged bytes"
        );
    } else {
        anyhow::ensure!(
            !complete && offset == length,
            "source chunk offset does not match staged length"
        );
        file.seek(SeekFrom::Start(offset))
            .context("seek source transfer offset")?;
        file.write_all(bytes)
            .context("write source artifact chunk")?;
        file.sync_all().context("persist source artifact chunk")?;
        journal::atomic_json(&progress_path, &end)?;
    }
    let length = file
        .metadata()
        .context("read durable source transfer length")?
        .len();
    drop(file);
    if length == artifact.size && !complete {
        check_archive(&partial, artifact)?;
        fs::rename(&partial, &archive).context("publish verified source archive")?;
        File::open(directory)
            .context("open source artifact directory")?
            .sync_all()
            .context("persist source artifact publication")?;
    }
    Ok(serde_json::json!({"offset": length, "complete": length == artifact.size}))
}

/// Resume local staging from the receiver's durable acknowledgement, including
/// jumping straight to EOF when an earlier attempt published the archive.
pub(super) fn stage_local(
    data_dir: &Path,
    operation: &str,
    artifact: &SourceArtifact,
    input: &mut (impl Read + Seek),
) -> Result<()> {
    let mut offset = 0;
    let mut bytes = [0; CHUNK_SIZE];
    loop {
        let count = input.read(&mut bytes).context("stage source archive")?;
        if count == 0 {
            break;
        }
        let reply = receive(data_dir, operation, artifact, offset, &bytes[..count])?;
        let acknowledged = reply["offset"]
            .as_u64()
            .context("missing source staging offset")?;
        anyhow::ensure!(
            acknowledged >= offset + count as u64 && acknowledged <= artifact.size,
            "source artifact acknowledgement offset mismatch"
        );
        input
            .seek(SeekFrom::Start(acknowledged))
            .context("resume local source staging")?;
        offset = acknowledged;
    }
    anyhow::ensure!(offset == artifact.size, "source artifact length changed");
    Ok(())
}

fn prepare_inner(
    release: &update::Release,
    target: &str,
    data_dir: &Path,
    operation: &str,
    execute: bool,
) -> Result<update::PreparedRelease> {
    let source = release
        .source
        .as_ref()
        .context("missing source release identity")?;
    let matches: Vec<_> = source
        .artifacts
        .iter()
        .filter(|a| a.target == target)
        .collect();
    anyhow::ensure!(
        matches.len() == 1,
        "source release must contain exactly one artifact for {target}"
    );
    let artifact = matches[0];
    validate(artifact)?;
    let archive = archive_path(data_dir, operation, target)?;
    check_archive(&archive, artifact)?;
    let mut local = release.clone();
    let name = format!("yinz-{target}.tar.gz");
    local.assets = vec![
        update::Asset {
            name: name.clone(),
            browser_download_url: "source-archive".into(),
        },
        update::Asset {
            name: "SHA256SUMS".into(),
            browser_download_url: "source-checksums".into(),
        },
    ];
    let mut read = |url: &str, output: &mut dyn Write| -> Result<()> {
        match url {
            "source-archive" => {
                std::io::copy(
                    &mut File::open(&archive).context("open verified source archive")?,
                    output,
                )
                .context("copy source archive into preparation")?;
            },
            "source-checksums" => writeln!(output, "{}  {name}", artifact.sha256)
                .context("supply pinned source checksum")?,
            _ => bail!("unexpected source artifact request"),
        }
        Ok(())
    };
    // Extract and inspect foreign binaries before any execution, including when
    // the kernel happens to support their architecture through emulation.
    let prepared = update::prepare_artifact(&local, target, &mut read, &mut |_| {})?;
    let binary = prepared.source.join("yinz");
    anyhow::ensure!(
        digest_file(&binary)? == artifact.binary_sha256,
        "source executable SHA-256 mismatch"
    );
    let mut header = [0u8; 20];
    File::open(&binary)
        .context("open source ELF header")?
        .read_exact(&mut header)
        .context("read source ELF header")?;
    let machine = match target {
        "x86_64-unknown-linux-gnu" => 62,
        "aarch64-unknown-linux-gnu" => 183,
        _ => unreachable!(),
    };
    anyhow::ensure!(
        &header[..6] == b"\x7fELF\x02\x01"
            && u16::from_le_bytes([header[18], header[19]]) == machine,
        "source executable architecture does not match {target}"
    );
    if execute {
        let output = yinz_process::ssh::bootstrap_output(
            std::process::Command::new(&binary).arg("--version"),
            std::time::Duration::from_secs(20),
            None,
        )
        .context("validate source executable on participant host")?;
        anyhow::ensure!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim()
                    == format!("yinz {}", prepared.version),
            "source executable did not report the pinned version"
        );
    }
    Ok(prepared)
}

pub(super) fn prepare(
    release: &update::Release,
    target: &str,
    data_dir: &Path,
    operation: &str,
) -> Result<update::PreparedRelease> {
    prepare_inner(release, target, data_dir, operation, true)
}

pub(super) fn prepare_artifact(
    release: &update::Release,
    target: &str,
    data_dir: &Path,
    operation: &str,
) -> Result<update::PreparedRelease> {
    prepare_inner(release, target, data_dir, operation, false)
}

#[cfg(test)]
#[path = "../unit/source_artifacts.rs"]
mod tests;
