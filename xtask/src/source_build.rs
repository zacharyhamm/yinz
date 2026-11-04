//! Server-host compilation for source updates. Never installs into a running installation.
use anyhow::Context;
use std::path::{Path, PathBuf};
use xshell::{cmd, Shell};

fn validate_target(target: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(
            target,
            "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu"
        ),
        "source updates support only Linux x86_64 and aarch64"
    );
    Ok(())
}

fn validate_target_directory(root: &Path, configured: &Path) -> anyhow::Result<PathBuf> {
    let target = root
        .canonicalize()
        .context("resolve source checkout")?
        .join("target");
    let configured = configured
        .parent()
        .context("Cargo target directory has no parent")?
        .canonicalize()
        .context("resolve Cargo target directory parent")?
        .join(
            configured
                .file_name()
                .context("Cargo target directory has no name")?,
        );
    anyhow::ensure!(
        configured == target,
        "source builds require Cargo's target directory to be checkout-local: {}",
        target.display()
    );
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "source builds require a real checkout-local target directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error).context("inspect source build target directory"),
    }
    Ok(target)
}

pub fn build(sh: &Shell, target: &str) -> anyhow::Result<()> {
    validate_target(target)?;
    anyhow::ensure!(
        std::env::consts::OS == "linux",
        "source builds require a Linux server"
    );
    let _directory = sh.push_dir(crate::project_root());
    for tool in [
        "cargo-zigbuild",
        "zig",
        "protoc",
        "python3",
        "file",
        "readelf",
    ] {
        which::which(tool)
            .with_context(|| format!("source build requires {tool} on the server host"))?;
    }
    anyhow::ensure!(
        std::env::var_os("CARGO_TARGET_DIR").is_none(),
        "source builds do not accept CARGO_TARGET_DIR"
    );
    let metadata = crate::metadata::get_cargo_metadata(sh)?;
    let configured = metadata["target_directory"]
        .as_str()
        .context("Cargo metadata has no target directory")?;
    let target_dir = validate_target_directory(&crate::project_root(), Path::new(configured))?;
    std::fs::create_dir_all(&target_dir).context("create source build target directory")?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(target_dir.join(".source-build.lock"))
        .context("open source build lock")?;
    lock.try_lock()
        .context("another source build is already packaging this checkout")?;
    let cargo = crate::cargo()?;
    // Encoded flags take precedence over target rustflags, including the native
    // mold setting. Zig must own linking for both architectures.
    let _flags = sh.push_env("CARGO_ENCODED_RUSTFLAGS", "");
    let _openssl = sh.push_env("OPENSSL_NO_VENDOR", "0");
    // Even a native-architecture Zig build uses Zig's headers/sysroot. Host
    // pkg-config may find libz without exporting headers into that sysroot.
    let _zlib = sh.push_env("LIBZ_SYS_STATIC", "1");
    cmd!(
        sh,
        "{cargo} zigbuild --locked --release --bin yinz --target {target}"
    )
    .run()
    .context("compile source update with Zig; install the target with rustup target add first")?;
    cmd!(sh, "python3 scripts/release/package.py {target} --cross")
        .run()
        .context("package source update")
}

#[cfg(test)]
#[path = "unit/source_build.rs"]
mod tests;
