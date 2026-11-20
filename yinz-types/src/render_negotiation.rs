//! Revision 1 of the per-attachment structured render handshake.
use serde::{Deserialize, Serialize};

pub const REVISION: u32 = 1;
pub const GEOMETRY: u32 = 1;
pub const LINKS: u32 = 2;
pub const KITTY: u32 = 4;
pub const SIXEL: u32 = 8;
pub const SHIFTS: u32 = 16;
pub const SCROLL: u32 = 32;
pub const INLINE: u32 = 1;
pub const SHARED: u32 = 2;
const PRODUCED: u32 = GEOMETRY | LINKS | KITTY | SIXEL;
const READABLE: u32 = PRODUCED | SHIFTS;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProof {
    pub root: String,
    pub basename: String,
    pub challenge: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub versions: Vec<u32>,
    pub features: u32,
    pub media: u32,
    pub proof: Option<MediaProof>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Offer(Offer),
    Withdraw,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Selected(Agreement),
    Rejected(String),
    Withdrawn(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreement {
    pub version: u32,
    pub features: u32,
    pub media: u32,
    pub proof_response: Vec<u8>,
}

impl Default for Agreement {
    // The shipped boolean client expects all implemented sections, and cannot prove a shared root.
    fn default() -> Self {
        Self {
            version: crate::structured_render::WIRE_VERSION.into(),
            features: PRODUCED,
            media: INLINE,
            proof_response: Vec::new(),
        }
    }
}

impl Offer {
    pub fn reader(proof: Option<MediaProof>) -> Self {
        Self {
            versions: vec![4, 3, 2],
            features: READABLE,
            media: INLINE | if proof.is_some() { SHARED } else { 0 },
            proof,
        }
    }

    pub fn select(&self, response: Option<Vec<u8>>) -> Result<Agreement, String> {
        self.intersect(
            &[crate::structured_render::WIRE_VERSION.into()],
            PRODUCED,
            response,
        )
    }

    fn intersect(
        &self,
        writable: &[u32],
        supported: u32,
        response: Option<Vec<u8>>,
    ) -> Result<Agreement, String> {
        let version = writable
            .iter()
            .copied()
            .filter(|v| self.versions.contains(v))
            .max()
            .ok_or_else(|| {
                const MAX_DISPLAYED: usize = 8;
                let shown = &self.versions[..self.versions.len().min(MAX_DISPLAYED)];
                let omitted = self.versions.len() - shown.len();
                let remainder = if omitted == 0 {
                    String::new()
                } else {
                    format!(" (+{omitted} more)")
                };
                format!(
                    "no common native frame version: offered {shown:?}{remainder}, writable {writable:?}"
                )
            })?;
        let mut features = self.features & supported;
        if features & GEOMETRY == 0 || self.media & INLINE == 0 {
            features &= !(KITTY | SIXEL);
        }
        if features & GEOMETRY == 0 || version < 3 {
            features &= !SCROLL;
        }
        let mut media = if features & (KITTY | SIXEL) != 0 {
            INLINE
        } else {
            0
        };
        let proof_response = response
            .filter(|_| features & KITTY != 0 && self.media & SHARED != 0 && self.proof.is_some())
            .unwrap_or_default();
        if !proof_response.is_empty() {
            media |= SHARED;
        }
        Ok(Agreement {
            version,
            features,
            media,
            proof_response,
        })
    }

    pub fn accepts(&self, agreement: &Agreement, expected: Option<&[u8]>) -> bool {
        if agreement.features & !READABLE != 0 || agreement.media & !self.media != 0 {
            return false;
        }
        let proof = if agreement.media & SHARED != 0 {
            if expected != Some(agreement.proof_response.as_slice())
                || agreement.proof_response.is_empty()
            {
                return false;
            }
            Some(agreement.proof_response.clone())
        } else {
            None
        };
        self.intersect(&[agreement.version], agreement.features, proof)
            .as_ref()
            == Ok(agreement)
    }
}

#[cfg(test)]
#[path = "unit/render_negotiation.rs"]
mod tests;
