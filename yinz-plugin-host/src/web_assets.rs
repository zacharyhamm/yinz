//! Bounded, descriptor-anchored freezing of a public asset tree.
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path},
};
use yinz_runtime::{
    errors::prelude::*,
    fs_beneath::{open_beneath, open_root},
};
use yinz_types::web::StaticFile;

pub(crate) fn freeze(bytes: Vec<u8>, content_type: &'static str) -> StaticFile {
    let hash = Sha256::digest(&bytes);
    let hex: String = hash[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    StaticFile {
        bytes: bytes.into(),
        content_type,
        etag: format!("\"{hex}\""),
    }
}
fn mime(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webmanifest" => "application/manifest+json",
        "json" => "application/json",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}
/// Resolve only normal relative components from an already trusted source parent.
pub(crate) fn load_directory(
    data_dir: &Path,
    source: &Path,
    directory: &str,
) -> Result<BTreeMap<String, StaticFile>> {
    let _guard = yinz_runtime::builtin_generation::source_read_guard(source)?;
    let _builtin_guard = if directory.starts_with("builtin:") {
        Some(yinz_runtime::builtin_generation::read_guard(data_dir)?)
    } else {
        None
    };
    let (base, relative) = if let Some(relative) = directory.strip_prefix("builtin:") {
        let builtin = yinz_runtime::builtin_generation::source_path(
            &data_dir.join("plugins/builtins/web.scm"),
        )?;
        (
            builtin
                .parent()
                .context("builtin has no parent")?
                .to_owned(),
            Path::new(relative),
        )
    } else {
        let source = yinz_runtime::builtin_generation::source_path(source)?;
        (
            source
                .parent()
                .context("plugin source has no parent")?
                .to_owned(),
            Path::new(directory),
        )
    };
    anyhow::ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "asset directory must be a confined relative path"
    );
    let root = open_beneath(&open_root(&base)?, relative)?;
    anyhow::ensure!(
        root.metadata().context("inspect asset root")?.is_dir(),
        "asset root is not a directory"
    );
    load_tree(root)
}

// Enumerate the pinned directory itself, including on platforms without /proc/self/fd.
fn entries(directory: &File) -> Result<Vec<std::ffi::OsString>> {
    use std::os::{fd::IntoRawFd, unix::ffi::OsStringExt};
    let duplicate = directory
        .try_clone()
        .context("retain asset directory")?
        .into_raw_fd();
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(std::io::Error::last_os_error()).context("list asset directory");
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Directory(stream);
    let mut result = Vec::new();
    loop {
        // readdir's end/error distinction is platform-specific; errno is supplied by nix.
        nix::errno::Errno::clear();
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = nix::errno::Errno::last_raw();
            anyhow::ensure!(error == 0, "read asset directory failed: {error}");
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            result.push(std::ffi::OsString::from_vec(name.to_vec()));
        }
        anyhow::ensure!(result.len() <= 4096, "asset directory has too many entries");
    }
    result.sort();
    Ok(result)
}
pub(crate) fn load_tree(root: File) -> Result<BTreeMap<String, StaticFile>> {
    let mut pending = vec![(root, String::new(), 0usize)];
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    let mut count = 0usize;
    let mut visited = 0usize;
    while let Some((directory, prefix, depth)) = pending.pop() {
        anyhow::ensure!(depth <= 6, "asset tree exceeds depth 6");
        for name in entries(&directory)? {
            visited += 1;
            anyhow::ensure!(visited <= 4096, "asset tree has too many entries");
            let file = open_beneath(&directory, Path::new(&name))?;
            let metadata = file.metadata().context("inspect asset entry")?;
            let name = name.to_str().context("asset filename must be UTF-8")?;
            let path = format!("{prefix}{name}");
            if metadata.is_dir() {
                pending.push((file, format!("{path}/"), depth + 1));
                continue;
            }
            anyhow::ensure!(metadata.is_file(), "asset entry is not a regular file");
            let Some(content_type) = mime(Path::new(name)) else {
                continue;
            };
            count += 1;
            anyhow::ensure!(
                count <= 256 && metadata.len() <= 2 * 1024 * 1024,
                "asset file count or size exceeded"
            );
            let mut bytes = Vec::new();
            file.take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .context("freeze asset bytes")?;
            total += bytes.len();
            anyhow::ensure!(
                bytes.len() <= 2 * 1024 * 1024 && total <= 8 * 1024 * 1024,
                "asset tree size exceeded"
            );
            files.insert(path, freeze(bytes, content_type));
        }
    }
    Ok(files)
}
#[cfg(test)]
#[path = "unit/web_assets.rs"]
mod tests;
