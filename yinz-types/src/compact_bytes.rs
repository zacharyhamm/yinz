//! Compact bytes for typed runtime transport fields. Ordinary JSON is never decoded implicitly.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use serde_json::Value;
use std::fmt;

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MARKER: &str = "$yinz-bytes";

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(&'static str);

pub fn encode(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error("binary payload exceeds 64 MiB"));
    }
    Ok(serde_json::json!({MARKER: STANDARD.encode(bytes)}))
}

pub fn decode(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    deserialize(value)
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, Error> {
    if encoded.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err(Error("binary payload exceeds 64 MiB"));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| Error("invalid binary payload base64"))?;
    if decoded.len() > MAX_BYTES {
        return Err(Error("binary payload exceeds 64 MiB"));
    }
    Ok(decoded)
}

/// Deserialize only a field whose declared type is bytes, without expanding its compact form.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    struct Bytes;
    impl<'de> Visitor<'de> for Bytes {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a byte array or a compact binary payload")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let capacity = sequence.size_hint().unwrap_or(0);
            if capacity > MAX_BYTES {
                return Err(A::Error::custom("binary payload exceeds 64 MiB"));
            }
            let mut bytes = Vec::with_capacity(capacity);
            while let Some(byte) = sequence.next_element::<u8>()? {
                if bytes.len() == MAX_BYTES {
                    return Err(A::Error::custom("binary payload exceeds 64 MiB"));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            if map.next_key::<String>()?.as_deref() != Some(MARKER) {
                return Err(A::Error::custom("invalid compact binary payload"));
            }
            let encoded = map.next_value::<String>()?;
            if map.next_key::<String>()?.is_some() {
                return Err(A::Error::custom("unexpected compact binary payload field"));
            }
            decode_base64(&encoded).map_err(A::Error::custom)
        }
    }
    deserializer.deserialize_any(Bytes)
}

#[cfg(test)]
#[path = "unit/compact_bytes.rs"]
mod tests;
