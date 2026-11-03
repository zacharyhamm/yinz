//! Explicit, local installation of the latest stable release.
use flate2::read::GzDecoder;
use isahc::{config::RedirectPolicy, prelude::*, HttpClient};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
    time::Duration,
};
use yinz_runtime::errors::prelude::*;

const RELEASE_URL: &str = "https://api.github.com/repos/zacharyhamm/yinz/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn target(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => Err(anyhow!("no Yinz release available for {os}/{arch}")),
    }
}
fn version(value: &str) -> Result<semver::Version> {
    semver::Version::parse(value.strip_prefix('v').unwrap_or(value))
        .context("parse release version")
}
fn asset<'a>(release: &'a Release, name: &str) -> Result<&'a str> {
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.browser_download_url.as_str())
        .with_context(|| format!("release {} is missing {name}", release.tag_name))
}
fn download(client: &HttpClient, url: &str, output: &mut dyn std::io::Write) -> Result<()> {
    anyhow::ensure!(url.starts_with("https://"), "release URL must use HTTPS");
    let request = isahc::Request::get(url)
        .header("User-Agent", concat!("yinz/", env!("CARGO_PKG_VERSION")))
        .body(())
        .context("build release request")?;
    let mut response = client
        .send(request)
        .context("fetch GitHub release; check network connectivity")?;
    anyhow::ensure!(
        response.status().is_success(),
        "GitHub request failed: {} ({url})",
        response.status()
    );
    std::io::copy(response.body_mut(), output).context("download release content")?;
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                DIGITS[(b >> 4) as usize] as char,
                DIGITS[(b & 15) as usize] as char,
            ]
        })
        .collect()
}
fn verify(archive: &Path, sums: &str, name: &str) -> Result<()> {
    let matches: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let filename = fields
                .next()?
                .trim_start_matches('*')
                .trim_start_matches("./");
            (filename == name && fields.next().is_none()).then_some(hash)
        })
        .collect();
    anyhow::ensure!(
        matches.len() == 1,
        "missing or ambiguous checksum for {name}"
    );
    let expected = matches[0];
    anyhow::ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA-256 checksum"
    );
    let mut file = fs::File::open(archive).context("open downloaded archive")?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).context("hash downloaded archive")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    anyhow::ensure!(
        hex(&hash.finalize()).eq_ignore_ascii_case(expected),
        "SHA-256 mismatch for {name}"
    );
    Ok(())
}
fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let input = fs::File::open(archive).context("open release archive")?;
    let mut archive = tar::Archive::new(GzDecoder::new(input));
    for entry in archive.entries().context("read release archive")? {
        let mut entry = entry.context("read release entry")?;
        let path = entry.path().context("read release path")?;
        anyhow::ensure!(
            path.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "unsafe archive path: {}",
            path.display()
        );
        let kind = entry.header().entry_type();
        anyhow::ensure!(
            kind.is_file() || kind.is_dir(),
            "release archive contains a link or special file"
        );
        anyhow::ensure!(
            entry
                .unpack_in(destination)
                .context("extract release entry")?,
            "entry escapes release directory"
        );
    }
    Ok(())
}
fn install(source: &Path, executable: &Path, data_dir: &Path) -> Result<()> {
    let executable = executable
        .canonicalize()
        .context("resolve current executable")?;
    let parent = executable
        .parent()
        .context("executable has no parent directory")?;
    let builtins = source.join("plugins/builtins");
    anyhow::ensure!(
        source.join("yinz").is_file() && builtins.is_dir(),
        "release is missing its executable or builtin plugins"
    );
    for path in [
        data_dir.to_path_buf(),
        data_dir.join("plugins"),
        data_dir.join("plugins/builtins"),
    ] {
        anyhow::ensure!(
            !path.is_symlink(),
            "builtin installation directory is a symlink: {}",
            path.display()
        );
    }
    let staged = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "cannot write {}; install with an account that owns this installation",
            parent.display()
        )
    })?;
    fs::copy(source.join("yinz"), staged.path()).context("stage new executable")?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o755))
        .context("set executable permissions")?;
    fs::create_dir_all(data_dir).context("create builtin data directory")?;
    let _probe = tempfile::NamedTempFile::new_in(data_dir)
        .context("builtin data directory is not writable")?;
    yinz_steel_builtins::install(&builtins, data_dir).context(
        "install builtins (some files may have been updated; executable was not replaced)",
    )?;
    staged.persist(&executable).with_context(|| {
        format!(
            "replace {} (builtins have been updated)",
            executable.display()
        )
    })?;
    Ok(())
}

pub fn run(data_dir: Option<&Path>) -> Result<()> {
    let target = target(std::env::consts::OS, std::env::consts::ARCH)?;
    let client = HttpClient::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .redirect_policy(RedirectPolicy::Limit(10))
        .build()
        .context("initialize release downloader")?;
    run_with(
        data_dir,
        &std::env::current_exe().context("locate current executable")?,
        target,
        |url, output| download(&client, url, output),
    )
}

fn run_with(
    data_dir: Option<&Path>,
    executable: &Path,
    target: &str,
    mut download: impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
) -> Result<()> {
    println!("Checking latest stable Yinz release...");
    let mut metadata = Vec::new();
    download(RELEASE_URL, &mut metadata)?;
    let release: Release = serde_json::from_slice(&metadata).context("parse GitHub release")?;
    let latest = version(&release.tag_name)?;
    anyhow::ensure!(latest.pre.is_empty(), "latest release is a prerelease");
    if version(yinz_runtime::consts::VERSION)? >= latest {
        println!(
            "Yinz {} is up to date (latest stable: {latest}).",
            yinz_runtime::consts::VERSION
        );
        return Ok(());
    }
    let name = format!("yinz-{target}.tar.gz");
    let archive_url = asset(&release, &name)?;
    let sums_url = asset(&release, "SHA256SUMS")?;
    let executable = executable.canonicalize().context("resolve executable")?;
    let data_dir = data_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
    println!(
        "Installing {latest} to {}\nBuiltin data directory: {}",
        executable.display(),
        data_dir.display()
    );
    let temp = tempfile::tempdir().context("create update staging directory")?;
    let archive = temp.path().join(&name);
    download(
        archive_url,
        &mut fs::File::create(&archive).context("create archive download")?,
    )?;
    let mut sums = Vec::new();
    download(sums_url, &mut sums)?;
    verify(
        &archive,
        std::str::from_utf8(&sums).context("decode checksums")?,
        &name,
    )?;
    let source = temp.path().join("extracted");
    fs::create_dir(&source).context("create extraction directory")?;
    extract(&archive, &source)?;
    let output = std::process::Command::new(source.join("yinz"))
        .arg("--version")
        .output()
        .context("validate downloaded executable compatibility")?;
    anyhow::ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("yinz {latest}"),
        "downloaded executable did not report expected version {latest}"
    );
    install(&source, &executable, &data_dir)?;
    println!("Installed Yinz {latest}. Restart existing sessions to use the new binary.");
    Ok(())
}

#[cfg(test)]
#[path = "unit/update.rs"]
mod tests;
