//! Release discovery and preparation, separated from installation.
//! Preparing a release never modifies the installed binary or builtin plugins.
use flate2::read::GzDecoder;
use isahc::{config::RedirectPolicy, prelude::*, HttpClient};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path},
    time::Duration,
};
use yinz_runtime::errors::prelude::*;

const RELEASE_URL: &str = "https://api.github.com/repos/zacharyhamm/yinz/releases/latest";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    pub(crate) assets: Vec<Asset>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
}

pub(crate) fn target(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => Err(anyhow!("no Yinz release available for {os}/{arch}")),
    }
}
pub(crate) fn version(value: &str) -> Result<semver::Version> {
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
pub(crate) fn download(
    client: &HttpClient,
    url: &str,
    output: &mut dyn std::io::Write,
) -> Result<()> {
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
pub(crate) fn hex(bytes: &[u8]) -> String {
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
pub(crate) mod transaction;

#[cfg(test)]
fn install(source: &Path, executable: &Path, data_dir: &Path) -> Result<()> {
    install_with_target(source, executable, data_dir, None)
}

fn install_with_target(
    source: &Path,
    executable: &Path,
    data_dir: &Path,
    target: Option<&semver::Version>,
) -> Result<()> {
    transaction::recover_standalone(executable, data_dir)?;
    let operation = format!("standalone-{}", uuid::Uuid::new_v4());
    let mut transaction =
        transaction::Transaction::prepare(source, executable, data_dir, &operation)?;
    if let Some(target) = target {
        let validation = (|| {
            let installed = installed_version(executable)
                .context("recheck installed version under the installation lock")?;
            anyhow::ensure!(
                &installed <= target,
                "another update installed newer Yinz {installed}; refusing downgrade to {target}"
            );
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = validation {
            transaction
                .rollback()
                .context("discard superseded standalone update preparation")?;
            return Err(error);
        }
    }
    if let Err(error) = transaction.publish() {
        return match transaction.rollback() {
            Ok(()) => Err(error.context("installation failed; restored the previous generation")),
            Err(recovery) => Err(error.context(format!(
                "installation failed and recovery is incomplete: {recovery:#}; retry yinz update"
            ))),
        };
    }
    transaction.commit()
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

/// A validated release whose staging directory stays alive until installation finishes.
/// Keeping preparation separate lets a session coordinator prepare every participant
/// before changing any installation or releasing any terminal.
pub(crate) struct PreparedRelease {
    _staging: tempfile::TempDir,
    pub(crate) source: std::path::PathBuf,
    pub(crate) version: semver::Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Checking,
    Downloading,
    Verifying,
    Validating,
    Installing,
}

pub(crate) fn discover(
    download: &mut impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
) -> Result<Release> {
    let mut metadata = Vec::new();
    download(RELEASE_URL, &mut metadata)?;
    let release: Release = serde_json::from_slice(&metadata).context("parse GitHub release")?;
    let latest = version(&release.tag_name)?;
    anyhow::ensure!(latest.pre.is_empty(), "latest release is a prerelease");
    Ok(release)
}

/// Uses the already resolved release, never another lookup of `latest`.
pub(crate) fn prepare(
    release: &Release,
    target: &str,
    download: &mut impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
    progress: &mut impl FnMut(Progress),
) -> Result<PreparedRelease> {
    let prepared = prepare_artifact(release, target, download, progress)?;
    progress(Progress::Validating);
    let output = yinz_process::ssh::bootstrap_output(
        std::process::Command::new(prepared.source.join("yinz")).arg("--version"),
        Duration::from_secs(20),
        None,
    )
    .context("validate downloaded executable compatibility")?;
    anyhow::ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim()
                == format!("yinz {}", prepared.version),
        "downloaded executable did not report expected version {}",
        prepared.version
    );
    Ok(prepared)
}

/// Prepare a foreign platform's artifact without executing it on this host.
/// Its executable and capabilities must be validated on the destination machine.
pub(crate) fn prepare_artifact(
    release: &Release,
    target: &str,
    download: &mut impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
    progress: &mut impl FnMut(Progress),
) -> Result<PreparedRelease> {
    let latest = version(&release.tag_name)?;
    anyhow::ensure!(latest.pre.is_empty(), "release is a prerelease");
    let name = format!("yinz-{target}.tar.gz");
    let archive_url = asset(release, &name)?;
    let sums_url = asset(release, "SHA256SUMS")?;
    let temp = tempfile::tempdir().context("create update staging directory")?;
    let archive = temp.path().join(&name);
    progress(Progress::Downloading);
    download(
        archive_url,
        &mut fs::File::create(&archive).context("create archive download")?,
    )?;
    let mut sums = Vec::new();
    download(sums_url, &mut sums)?;
    progress(Progress::Verifying);
    verify(
        &archive,
        std::str::from_utf8(&sums).context("decode checksums")?,
        &name,
    )?;
    let source = temp.path().join("extracted");
    fs::create_dir(&source).context("create extraction directory")?;
    extract(&archive, &source)?;
    anyhow::ensure!(
        source.join("yinz").is_file() && source.join("plugins/builtins").is_dir(),
        "release is missing its executable or builtin plugins"
    );
    Ok(PreparedRelease {
        _staging: temp,
        source,
        version: latest,
    })
}

fn installed_version(executable: &Path) -> Result<semver::Version> {
    let output = yinz_process::ssh::bootstrap_output(
        std::process::Command::new(executable).arg("--version"),
        Duration::from_secs(20),
        None,
    )
    .context("query installed executable version")?;
    anyhow::ensure!(
        output.status.success(),
        "installed executable version query failed"
    );
    let stdout = std::str::from_utf8(&output.stdout).context("decode installed version")?;
    version(
        stdout
            .trim()
            .strip_prefix("yinz ")
            .context("invalid installed version response")?,
    )
}

fn run_with(
    data_dir: Option<&Path>,
    executable: &Path,
    target: &str,
    mut download: impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
) -> Result<()> {
    let mut progress = |phase| match phase {
        Progress::Checking => println!("Checking latest stable Yinz release..."),
        Progress::Downloading => println!("Downloading release..."),
        Progress::Verifying => println!("Verifying release checksum..."),
        Progress::Validating => println!("Validating release..."),
        Progress::Installing => println!("Installing release..."),
    };
    let executable = executable.canonicalize().context("resolve executable")?;
    let data_dir = data_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
    transaction::recover_standalone(&executable, &data_dir)?;
    progress(Progress::Checking);
    let release = discover(&mut download)?;
    let latest = version(&release.tag_name)?;
    let installed = installed_version(&executable)?;
    if installed >= latest {
        println!("Yinz {installed} is up to date (latest stable: {latest}).");
        return Ok(());
    }
    let prepared = prepare(&release, target, &mut download, &mut progress)?;
    println!(
        "Installing {} to {}\nBuiltin data directory: {}",
        prepared.version,
        executable.display(),
        data_dir.display()
    );
    progress(Progress::Installing);
    install_with_target(
        &prepared.source,
        &executable,
        &data_dir,
        Some(&prepared.version),
    )?;
    println!("Installed Yinz {latest}. Restart existing sessions to use the new binary.");
    Ok(())
}

#[cfg(test)]
#[path = "unit/update.rs"]
mod tests;
