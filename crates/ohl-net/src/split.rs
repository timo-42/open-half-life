//! The fixed GoldSource split header from the public protocol table.

use crate::{
    Error, MAX_DATAGRAM_BYTES, MAX_FRAGMENTS, MAX_RESPONSE_BYTES, SINGLE_HEADER, SPLIT_HEADER,
};

/// Bounded assembly of one GoldSource response from one known sender.
///
/// The caller must isolate instances by UDP sender and impose a deadline and
/// datagram count. Fragment zero must begin with a complete standard single
/// response header and known response type. That deliberately rejects ambiguous
/// Source split layouts instead of guessing an engine from byte patterns.
/// Reordering and identical duplicates are supported; conflicting duplicates,
/// changing IDs/counts and high-bit IDs are rejected. Errors clear pending data.
#[derive(Debug, Default)]
pub struct GoldSrcReassembler {
    pending: Option<Assembly>,
}

#[derive(Debug)]
struct Assembly {
    id: u32,
    fragments: Vec<Option<Vec<u8>>>,
    bytes: usize,
    received: usize,
}

impl GoldSrcReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Discard an incomplete reply, for example before a challenge retry.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// Return a full `FF FF FF FF` datagram once all fragments are present.
    pub fn push(&mut self, datagram: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let result = self.push_inner(datagram);
        if result.is_err() {
            self.reset();
        }
        result
    }

    fn push_inner(&mut self, datagram: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        if datagram.len() > MAX_DATAGRAM_BYTES {
            return Err(Error::Limit("datagram bytes"));
        }
        let header = datagram.get(..4).ok_or(Error::Truncated)?;
        if header == SINGLE_HEADER {
            self.reset();
            return Ok(Some(datagram.to_vec()));
        }
        if header != SPLIT_HEADER {
            return Err(Error::Invalid("connectionless header"));
        }
        let id = u32::from_le_bytes(
            datagram
                .get(4..8)
                .ok_or(Error::Truncated)?
                .try_into()
                .map_err(|_| Error::Truncated)?,
        );
        if id & 0x8000_0000 != 0 {
            return Err(Error::Unsupported(
                "compressed or high-bit split identifier",
            ));
        }
        let packed = *datagram.get(8).ok_or(Error::Truncated)?;
        let index = usize::from(packed >> 4);
        let count = usize::from(packed & 15);
        if !(2..=MAX_FRAGMENTS).contains(&count) || index >= count {
            return Err(Error::Invalid("GoldSource fragment index or count"));
        }
        let payload = &datagram[9..];
        if payload.is_empty() {
            return Err(Error::Truncated);
        }
        if index == 0
            && (payload.get(..4) != Some(SINGLE_HEADER.as_slice())
                || !matches!(payload.get(4), Some(b'A' | b'I' | b'm' | b'D' | b'E')))
        {
            return Err(Error::Unsupported("ambiguous split response framing"));
        }
        let pending = self.pending.get_or_insert_with(|| Assembly {
            id,
            fragments: vec![None; count],
            bytes: 0,
            received: 0,
        });
        if pending.id != id || pending.fragments.len() != count {
            return Err(Error::Invalid("split response identity changed"));
        }
        if let Some(previous) = &pending.fragments[index] {
            if previous != payload {
                return Err(Error::Invalid("conflicting duplicate fragment"));
            }
            return Ok(None);
        }
        if pending.bytes + payload.len() > MAX_RESPONSE_BYTES {
            return Err(Error::Limit("assembled response bytes"));
        }
        pending.bytes += payload.len();
        pending.received += 1;
        pending.fragments[index] = Some(payload.to_vec());
        if pending.received != count {
            return Ok(None);
        }
        let mut result = Vec::with_capacity(pending.bytes);
        for part in pending.fragments.iter().flatten() {
            result.extend_from_slice(part);
        }
        self.reset();
        Ok(Some(result))
    }
}
