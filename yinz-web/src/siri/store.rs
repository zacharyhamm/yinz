//! Pairing, revocation and dialogue checkpoints share the web password lock with logout.
use super::Dialogue;
use crate::auth::{password, sessions, SessionInfo};
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::Path};
use yinz_runtime::errors::prelude::*;

const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Device {
    owner: String,
    // Legacy rows contain only a session name and cannot identify a listener.
    #[serde(default)]
    listener_instance: String,
    code: Option<(String, i64)>,
    token_hash: Option<String>,
    revision: u64,
    dialogue: Dialogue,
}

fn access<T>(
    data: &Path,
    now: i64,
    f: impl FnOnce(&mut Vec<Device>, &[sessions::Row], i64) -> Result<(T, bool)>,
) -> Result<T> {
    let dir = password::private_dir(data).context("open Siri store directory")?;
    let _lock = password::lock(&dir).context("lock Siri store")?;
    let password::PasswordState::Ready(verifier) = password::read_state(data) else {
        bail!("web sign-in unavailable")
    };
    let sessions = sessions::read(data, &verifier.verifier_id)
        .context("read web sessions for Siri")?
        .sessions;
    let rows: Vec<serde_json::Value> = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(dir.join("siri.json"))
    {
        Ok(file) => {
            password::validate_private(&file).context("validate Siri store")?;
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .context("read Siri devices")?;
            anyhow::ensure!(bytes.len() <= MAX_BYTES, "Siri store too large");
            serde_json::from_slice(&bytes).context("decode Siri devices")?
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).context("open Siri devices"),
    };
    let count = rows.len();
    // A row from another release can fail to decode. Revoke only that pairing, so its
    // phone pairs again, instead of failing every phone's commands, setup and revocation.
    let mut devices: Vec<Device> = rows
        .into_iter()
        .filter_map(|row| {
            match serde_json::from_value(row).context("drop an undecodable Siri pairing") {
                Ok(device) => Some(device),
                Err(error) => {
                    Err::<(), _>(error).non_fatal();
                    None
                },
            }
        })
        .collect();
    devices.retain(|d| {
        !d.listener_instance.is_empty() && sessions.iter().any(|s| s.id == d.owner && s.valid(now))
    });
    let (result, changed) = f(&mut devices, &sessions, now)?;
    if changed || count != devices.len() {
        let bytes = serde_json::to_vec(&devices).context("encode Siri devices")?;
        anyhow::ensure!(bytes.len() <= MAX_BYTES, "Siri store too large");
        password::atomic_write(&dir, "siri.json", &bytes).context("write Siri devices")?;
    }
    Ok(result)
}

pub(super) fn configure(
    data: &Path,
    owner: &str,
    listener_instance: &str,
    revoke: bool,
    now: i64,
) -> Result<Option<String>> {
    access(data, now, |devices, sessions, now| {
        anyhow::ensure!(
            sessions.iter().any(|s| s.id == owner && s.valid(now)),
            "sign-in expired"
        );
        if revoke {
            devices.retain(|d| d.owner != owner);
            return Ok((None, true));
        }
        let code = password::random_id().context("generate Siri pairing code")?;
        let pairing = Some((sessions::hash(&code), now + 5 * 60_000));
        if let Some(device) = devices
            .iter_mut()
            .find(|d| d.owner == owner && d.listener_instance == listener_instance)
        {
            device.code = pairing;
        } else {
            devices.retain(|d| d.owner != owner);
            devices.push(Device {
                owner: owner.into(),
                listener_instance: listener_instance.into(),
                code: pairing,
                token_hash: None,
                revision: 0,
                dialogue: Dialogue::default(),
            });
        }
        Ok((Some(code), true))
    })
}

pub(super) fn redeem(
    data: &Path,
    listener_instance: &str,
    code: &str,
    now: i64,
) -> Result<Option<String>> {
    let hash = sessions::hash(code);
    access(data, now, |devices, _, now| {
        let Some(device) = devices.iter_mut().find(|d| {
            d.listener_instance == listener_instance
                && d.code
                    .as_ref()
                    .is_some_and(|(c, exp)| c == &hash && now < *exp)
        }) else {
            return Ok((None, false));
        };
        let token = format!(
            "yzs1_{}",
            password::random_id().context("generate Siri credential")?
        );
        device.code = None;
        device.token_hash = Some(sessions::hash(&token));
        device.dialogue = Dialogue::default();
        device.revision = 0;
        Ok((Some(token), true))
    })
}

pub(super) struct Phone {
    pub session: SessionInfo,
    pub revision: u64,
    pub dialogue: Dialogue,
}
pub(super) fn load(
    data: &Path,
    listener_instance: &str,
    hash: &str,
    now: i64,
) -> Result<Option<Phone>> {
    access(data, now, |devices, sessions, _| {
        let device = devices.iter().find(|d| {
            d.listener_instance == listener_instance && d.token_hash.as_deref() == Some(hash)
        });
        let phone = device.and_then(|d| {
            Some(Phone {
                session: sessions.iter().find(|s| s.id == d.owner)?.info(),
                revision: d.revision,
                dialogue: d.dialogue.clone(),
            })
        });
        Ok((phone, false))
    })
}

pub(super) fn save(
    data: &Path,
    listener_instance: &str,
    hash: &str,
    revision: u64,
    dialogue: Dialogue,
    now: i64,
) -> Result<()> {
    access(data, now, |devices, _, _| {
        let device = devices
            .iter_mut()
            .find(|d| {
                d.listener_instance == listener_instance && d.token_hash.as_deref() == Some(hash)
            })
            .context("Siri pairing revoked")?;
        anyhow::ensure!(
            device.revision == revision,
            "another Siri command is in progress"
        );
        device.dialogue = dialogue;
        device.revision += 1;
        Ok(((), true))
    })
}
