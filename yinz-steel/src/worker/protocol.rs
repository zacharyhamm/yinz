use std::{
    io::{Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::MAX_FRAME;
use crate::EvalOutput;

#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Call(Operation),
    HostReply(std::result::Result<Value, String>),
    SourceReply(std::result::Result<crate::modules::ModuleSource, String>),
    ResolverReply(std::result::Result<String, String>),
}

#[derive(Serialize, Deserialize)]
pub(super) enum Operation {
    Eval {
        source: String,
        path: Option<PathBuf>,
        origin: Option<String>,
    },
    Metadata(crate::ServerMetadata),
    Value(String),
    Invoke(String, Vec<Value>),
    Load(Value),
    Update(Value),
    Pipe(Value),
    Palette(Value),
    Render(usize, usize),
    Unload,
    Output,
    Logs,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    HostCall(Value),
    SourceRequest { origin: String, specifier: String },
    ResolverRequest(Option<String>),
    Finished(std::result::Result<Output, String>),
}

#[derive(Serialize, Deserialize)]
pub(super) enum Output {
    Ready,
    Unit,
    Eval(EvalOutput),
    Value(Value),
    Changed(bool),
    Text(String),
}

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    struct Frame(Vec<u8>);
    impl Write for Frame {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_FRAME + 4 {
                return Err(std::io::Error::other("Steel worker frame exceeds 96 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut frame = Frame(vec![0; 4]);
    serde_json::to_writer(&mut frame, value).context("serialize Steel worker frame")?;
    let length = (frame.0.len() - 4) as u32;
    frame.0[..4].copy_from_slice(&length.to_be_bytes());
    Ok(frame.0)
}

pub(super) fn read<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut prefix = [0; 4];
    reader
        .read_exact(&mut prefix)
        .context("read Steel worker frame length")?;
    let size = u32::from_be_bytes(prefix) as usize;
    anyhow::ensure!(size <= MAX_FRAME, "Steel worker frame exceeds 96 MiB");
    let mut bytes = vec![0; size];
    reader
        .read_exact(&mut bytes)
        .context("read Steel worker frame")?;
    serde_json::from_slice(&bytes).context("decode Steel worker frame")
}

pub(super) fn write(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    writer
        .write_all(&encode(value)?)
        .context("write Steel worker frame")?;
    writer.flush().context("flush Steel worker frame")
}
