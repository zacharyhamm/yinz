//! Inspect only the outer field extents needed to admit a bounded frame. Prost's
//! generated oneof decoder merges fields and decodes their bodies, so it cannot
//! discriminate ownership or ambiguous declarations before decoding.
use super::{generation, AttachmentGeneration};
use anyhow::ensure;
use prost::encoding::{decode_key, decode_varint, skip_field, DecodeContext, WireType};
use yinz_runtime::errors::prelude::*;

pub(super) enum Direction {
    Client,
    Server,
}

impl Direction {
    fn owner_field(&self) -> u32 {
        match self {
            Self::Client => 46,
            Self::Server => 38,
        }
    }

    fn message_wire_type(&self, field: u32) -> Option<WireType> {
        match (self, field) {
            (Self::Client, 38) | (Self::Server, 26 | 29) => Some(WireType::Varint),
            (Self::Client, 1..=13 | 16..=20 | 22..=25 | 27..=45)
            | (Self::Server, 1..=10 | 12..=16 | 18 | 20..=37) => Some(WireType::LengthDelimited),
            _ => None,
        }
    }
}

fn fields<'a>(
    mut bytes: &'a [u8],
    mut visit: impl FnMut(u32, WireType, &'a [u8]) -> Result<()>,
) -> Result<()> {
    while !bytes.is_empty() {
        let (field, wire) = decode_key(&mut bytes).context("read attachment field key")?;
        let body = if wire == WireType::LengthDelimited {
            let len = usize::try_from(decode_varint(&mut bytes).context("read field length")?)
                .context("oversized attachment field")?;
            let (body, rest) = bytes
                .split_at_checked(len)
                .context("truncated attachment field")?;
            bytes = rest;
            body
        } else {
            skip_field(wire, field, &mut bytes, DecodeContext::default())
                .context("skip attachment field")?;
            &[]
        };
        visit(field, wire, body)?;
    }
    Ok(())
}

pub(super) struct Envelope<'a> {
    pub kind: u32,
    pub body: &'a [u8],
    pub owner: Option<&'a [u8]>,
}

impl<'a> Envelope<'a> {
    pub fn inspect(bytes: &'a [u8], direction: Direction) -> Result<Self> {
        let mut payload = None;
        let mut owner = None;
        fields(bytes, |field, wire, body| {
            if field == direction.owner_field() {
                ensure!(
                    owner.is_none() && wire == WireType::LengthDelimited,
                    "invalid attachment field"
                );
                owner = Some(body);
            } else if let Some(expected) = direction.message_wire_type(field) {
                ensure!(
                    expected == wire && payload.is_none(),
                    "invalid IPC message discriminator"
                );
                payload = Some((field, body));
            }
            Ok(())
        })?;
        let (kind, body) = payload.context("missing IPC message discriminator")?;
        Ok(Self { kind, body, owner })
    }

    // Called only on a native request whose outer owner field had the wrong length.
    pub fn is_offer(&self) -> Result<bool> {
        if self.kind != 45 {
            return Ok(false);
        }
        let mut arm = None;
        fields(self.body, |field, wire, _| {
            if matches!(field, 1 | 2) {
                ensure!(
                    wire == WireType::LengthDelimited && arm.is_none(),
                    "ambiguous native request arm"
                );
                arm = Some(field);
            }
            Ok(())
        })?;
        Ok(arm == Some(1))
    }

    pub fn advertised_generation(&self) -> Result<Option<AttachmentGeneration>> {
        let mut bytes = None;
        fields(self.body, |field, wire, body| {
            if field == 10 {
                ensure!(
                    wire == WireType::LengthDelimited && bytes.is_none(),
                    "invalid advertised attachment field"
                );
                bytes = Some(body);
            }
            Ok(())
        })?;
        generation(bytes)
    }
}

#[cfg(test)]
#[path = "unit/attachment_envelope.rs"]
mod tests;
