//! Validate the executable generation while holding the builtin installation lock.
use std::path::Path;
use yinz_runtime::errors::prelude::*;

#[cfg(target_os = "linux")]
pub(super) fn verify(binary: &Path) -> Result<()> {
    verify_linux_paths(binary, Path::new("/proc/self/exe"))
}

#[cfg(target_os = "linux")]
fn is_retired_installation(binary: &Path) -> bool {
    binary.file_name().is_some_and(|name| name == "binary")
        && binary
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b".yinz-update-"))
}

#[cfg(target_os = "linux")]
fn verify_linux_paths(installed: &Path, running: &Path) -> Result<()> {
    // /proc/self/exe follows the old inode when exchange moves it into the
    // updater's backup. If startup first resolved current_exe after that move,
    // comparing it with /proc/self/exe would otherwise compare the backup with
    // itself and incorrectly accept the new installed builtin tree.
    anyhow::ensure!(
        !is_retired_installation(installed),
        "Yinz was updated while this server was starting; restart it from its installed executable"
    );
    use std::os::unix::fs::MetadataExt;
    let installed = std::fs::metadata(installed).context("inspect installed server generation")?;
    let running = std::fs::metadata(running).context("inspect executing server generation")?;
    anyhow::ensure!(installed.dev() == running.dev() && installed.ino() == running.ino(),
        "Yinz was updated while this server was starting; restart it to load a matching builtin generation");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn verify(binary: &Path) -> Result<()> {
    unsafe extern "C" {
        // A byte pointer avoids libc's deprecated historical Mach header types.
        fn _dyld_get_image_header(index: u32) -> *const u8;
    }
    // SAFETY: dyld retains the main image and its validated load commands for
    // the process lifetime. Yinz's supported macOS architectures are 64-bit.
    let running = unsafe {
        let header = _dyld_get_image_header(0);
        anyhow::ensure!(
            !header.is_null(),
            "cannot inspect the executing Mach-O image"
        );
        let prefix = std::slice::from_raw_parts(header, 32);
        let length = commands_length(prefix)?;
        std::slice::from_raw_parts(header, length)
    };
    let cpu = number(running, 4)?;
    let subtype = number(running, 8)?;
    let installed = read_image(binary, cpu, subtype)?;
    anyhow::ensure!(uuid(running)? == uuid(&installed)?,
        "Yinz was updated while this server was starting; restart it to load a matching builtin generation");
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn verify(_: &Path) -> Result<()> {
    bail!("cannot verify an installed server generation on this platform")
}

#[cfg(any(target_os = "macos", test))]
fn number(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated Mach-O image header")?
            .try_into()
            .context("invalid Mach-O word")?,
    ))
}

#[cfg(any(target_os = "macos", test))]
fn commands_length(header: &[u8]) -> Result<usize> {
    anyhow::ensure!(
        number(header, 0)? == 0xfeed_facf,
        "unsupported Mach-O executable header"
    );
    let size = usize::try_from(number(header, 20)?).context("Mach-O command size overflow")?;
    anyhow::ensure!(
        size <= 1024 * 1024 && number(header, 16)? <= 65536,
        "Mach-O load command table is too large"
    );
    Ok(32 + size)
}

#[cfg(any(target_os = "macos", test))]
fn uuid(image: &[u8]) -> Result<[u8; 16]> {
    let end = commands_length(image)?;
    anyhow::ensure!(image.len() >= end, "truncated Mach-O load commands");
    let mut offset = 32;
    let mut found = None;
    for _ in 0..number(image, 16)? {
        let command = number(image, offset)?;
        let size = usize::try_from(number(image, offset + 4)?)
            .context("Mach-O command length overflow")?;
        anyhow::ensure!(
            size >= 8 && offset.checked_add(size).is_some_and(|next| next <= end),
            "invalid Mach-O load command length"
        );
        if command == 0x1b {
            // LC_UUID: uuid_command has sixteen identifier bytes.
            anyhow::ensure!(
                size == 24 && found.is_none(),
                "invalid or duplicate Mach-O UUID"
            );
            found = Some(
                image[offset + 8..offset + 24]
                    .try_into()
                    .context("invalid Mach-O UUID bytes")?,
            );
        }
        offset += size;
    }
    found.context("executing image has no build UUID; cannot safely pin its builtin generation")
}

#[cfg(any(target_os = "macos", test))]
fn read_image(path: &Path, cpu: u32, subtype: u32) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).context("open installed Mach-O executable")?;
    let mut header = [0; 32];
    file.read_exact(&mut header)
        .context("read installed executable header")?;
    let magic = u32::from_be_bytes(header[..4].try_into().context("invalid executable magic")?);
    let offset = if matches!(magic, 0xcafe_babe | 0xcafe_babf) {
        let count = u32::from_be_bytes(
            header[4..8]
                .try_into()
                .context("invalid fat architecture count")?,
        );
        anyhow::ensure!(count <= 64, "too many universal executable architectures");
        file.seek(SeekFrom::Start(8))
            .context("read universal executable table")?;
        let size = if magic == 0xcafe_babf { 32 } else { 20 };
        let mut found = None;
        for _ in 0..count {
            let mut architecture = [0; 32];
            file.read_exact(&mut architecture[..size])
                .context("read universal executable architecture")?;
            let arch_cpu = u32::from_be_bytes(
                architecture[..4]
                    .try_into()
                    .context("invalid architecture CPU")?,
            );
            let arch_subtype = u32::from_be_bytes(
                architecture[4..8]
                    .try_into()
                    .context("invalid architecture subtype")?,
            );
            if arch_cpu == cpu && arch_subtype & 0x00ff_ffff == subtype & 0x00ff_ffff {
                anyhow::ensure!(
                    found.is_none(),
                    "ambiguous universal executable architecture"
                );
                found = Some(if size == 32 {
                    u64::from_be_bytes(
                        architecture[8..16]
                            .try_into()
                            .context("invalid 64-bit architecture offset")?,
                    )
                } else {
                    u64::from(u32::from_be_bytes(
                        architecture[8..12]
                            .try_into()
                            .context("invalid architecture offset")?,
                    ))
                });
            }
        }
        found.context("installed executable lacks the running architecture")?
    } else {
        0
    };
    file.seek(SeekFrom::Start(offset))
        .context("seek installed architecture")?;
    file.read_exact(&mut header)
        .context("read installed architecture header")?;
    anyhow::ensure!(
        number(&header, 4)? == cpu && number(&header, 8)? & 0x00ff_ffff == subtype & 0x00ff_ffff,
        "installed executable architecture differs from the running image"
    );
    let mut image = vec![0; commands_length(&header)?];
    image[..32].copy_from_slice(&header);
    file.read_exact(&mut image[32..])
        .context("read installed Mach-O load commands")?;
    Ok(image)
}

#[cfg(test)]
#[path = "../unit/worker_generation_identity.rs"]
mod tests;
