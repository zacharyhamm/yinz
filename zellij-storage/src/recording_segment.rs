//! The compressed block format cold recording events are packed into.
//!
//! A PTY read is usually a few hundred bytes, which compresses poorly on its own (deflate gets
//! 1.7× on 256 B pieces of coloured `git log -p`) and costs a table row and two index entries
//! regardless. Packed into ~1 MiB blocks the same output compresses 5.2×, and a block is one row.
//!
//! A block is deflate over the events laid end to end, each as
//! `zigzag id delta | zigzag elapsed delta | code byte | length | bytes`, every integer a LEB128
//! varint. Deltas run from the previous event (the first from zero), so the usual event costs
//! four or five bytes of framing.

use std::io::{Read, Write};

use anyhow::{bail, ensure, Context, Result};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};

/// The only codec written today, kept per segment so another can be added without a migration.
pub(crate) const DEFLATE: &str = "deflate";

/// One `recording_events` row, as a segment carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredEvent {
    pub id: i64,
    pub elapsed_us: i64,
    /// The asciicast code as its single ASCII byte: `o`, `i` or `r`.
    pub code: u8,
    pub data: Vec<u8>,
}

/// The block for `events`, in the order given, and the uncompressed length `decode` checks.
pub(crate) fn encode(events: &[StoredEvent]) -> Result<(Vec<u8>, usize)> {
    let mut raw = Vec::with_capacity(events.iter().map(|event| event.data.len() + 8).sum());
    let (mut id, mut elapsed_us) = (0i64, 0i64);
    for event in events {
        write_varint(&mut raw, zigzag(event.id.wrapping_sub(id)));
        write_varint(&mut raw, zigzag(event.elapsed_us.wrapping_sub(elapsed_us)));
        raw.push(event.code);
        write_varint(&mut raw, event.data.len() as u64);
        raw.extend_from_slice(&event.data);
        (id, elapsed_us) = (event.id, event.elapsed_us);
    }
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&raw)
        .context("failed to compress a recording segment")?;
    let compressed = encoder
        .finish()
        .context("failed to compress a recording segment")?;
    Ok((compressed, raw.len()))
}

/// The events `encode` packed. `raw_len` bounds the decompression, so a corrupt row fails here
/// instead of inflating without limit. It is not reserved up front: a corrupt one could ask for
/// more memory than the machine has before a byte had been inflated.
pub(crate) fn decode(codec: &str, compressed: &[u8], raw_len: usize) -> Result<Vec<StoredEvent>> {
    if codec != DEFLATE {
        bail!("recording segment uses unknown codec {codec:?}");
    }
    let mut raw = Vec::new();
    DeflateDecoder::new(compressed)
        .take((raw_len as u64).saturating_add(1))
        .read_to_end(&mut raw)
        .context("failed to decompress a recording segment")?;
    ensure!(
        raw.len() == raw_len,
        "recording segment decompressed to {} bytes, expected {raw_len}",
        raw.len()
    );
    let mut events = Vec::new();
    let mut rest = raw.as_slice();
    let (mut id, mut elapsed_us) = (0i64, 0i64);
    while !rest.is_empty() {
        id = id.wrapping_add(unzigzag(read_varint(&mut rest)?));
        elapsed_us = elapsed_us.wrapping_add(unzigzag(read_varint(&mut rest)?));
        let (&code, after_code) = rest
            .split_first()
            .context("recording segment ends inside an event")?;
        rest = after_code;
        let len = usize::try_from(read_varint(&mut rest)?)
            .ok()
            .filter(|len| *len <= rest.len())
            .context("recording segment event runs past the end of the segment")?;
        let (data, after_data) = rest.split_at(len);
        rest = after_data;
        events.push(StoredEvent {
            id,
            elapsed_us,
            code,
            data: data.to_vec(),
        });
    }
    Ok(events)
}

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input
            .split_first()
            .context("recording segment ends inside a number")?;
        *input = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("recording segment holds a number longer than 64 bits")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    #[hegel::composite]
    fn stored_event(tc: &hegel::TestCase) -> StoredEvent {
        StoredEvent {
            id: tc.draw(generators::integers::<i64>()),
            elapsed_us: tc.draw(generators::integers::<i64>()),
            code: tc.draw(generators::sampled_from(vec![b'o', b'i', b'r'])),
            data: tc.draw(generators::binary().max_size(300)),
        }
    }

    /// Any events, in any order, with any ids and timestamps — including the extremes, where a
    /// delta wraps — come back out exactly as they went in.
    #[hegel::test]
    fn segments_round_trip(tc: hegel::TestCase) {
        let events = tc.draw(generators::vecs(stored_event()).max_size(40));
        let (compressed, raw_len) = encode(&events).unwrap();
        assert_eq!(decode(DEFLATE, &compressed, raw_len).unwrap(), events);
    }

    /// A damaged row is an error, never a panic or an unbounded allocation.
    #[hegel::test]
    fn corrupt_segments_fail_cleanly(tc: hegel::TestCase) {
        let compressed = tc.draw(generators::binary().max_size(200));
        let raw_len = tc.draw(generators::integers::<usize>());
        let _ = decode(DEFLATE, &compressed, raw_len);
    }

    #[test]
    fn a_truncated_block_is_refused() -> Result<()> {
        let event = StoredEvent {
            id: 7,
            elapsed_us: 100,
            code: b'o',
            data: b"hello".to_vec(),
        };
        let (compressed, raw_len) = encode(&[event])?;
        assert!(decode(DEFLATE, &compressed, raw_len + 1).is_err());
        assert!(decode(DEFLATE, &compressed, raw_len - 1).is_err());
        assert!(decode(DEFLATE, &compressed, usize::MAX).is_err());
        assert!(decode("zstd", &compressed, raw_len).is_err());
        Ok(())
    }
}
