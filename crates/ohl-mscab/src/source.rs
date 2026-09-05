//! The caller-supplied byte source and cancellation vocabulary.
//!
//! This crate never opens a path, never allocates a whole cabinet, and never
//! decides which volume of a cabinet *set* a continuation lives on. It reads
//! through [`VolumeSource`], whose `volume` index is opaque to this crate: the
//! caller maps `CFHEADER.iCabinet` / the next-cabinet name of a set onto its
//! own volume indices and hands back the bytes.

use crate::error::{CabError, Result};

/// A random-access, read-only view over one or more cabinet volumes.
///
/// Offsets are relative to the **start of the cabinet** on that volume, so a
/// cabinet embedded inside a larger file is presented by the caller as a view
/// whose offset 0 is the `MSCF` signature.
pub trait VolumeSource {
    /// The number of readable bytes on `volume`, counted from the cabinet
    /// start. May exceed `CFHEADER.cbCabinet` when the cabinet is embedded in
    /// a larger container.
    fn volume_len(&self, volume: u32) -> Result<u64>;

    /// Fills `buf` completely from `offset` on `volume`.
    ///
    /// Implementations must return [`CabError::OutOfBounds`] rather than a
    /// short read when the range is not fully available.
    fn read_at(&self, volume: u32, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl<T: VolumeSource + ?Sized> VolumeSource for &T {
    fn volume_len(&self, volume: u32) -> Result<u64> {
        (**self).volume_len(volume)
    }

    fn read_at(&self, volume: u32, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(volume, offset, buf)
    }
}

/// The convenience source for a single, already-bounded in-memory cabinet.
///
/// Only volume 0 exists; any other index reports [`CabError::Unsupported`],
/// which is what a caller sees when a file continues into a volume it did not
/// supply.
#[derive(Debug, Clone, Copy)]
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    /// Wraps `bytes`, whose first byte must be the `MSCF` signature.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl VolumeSource for SliceSource<'_> {
    fn volume_len(&self, volume: u32) -> Result<u64> {
        if volume == 0 {
            Ok(self.bytes.len() as u64)
        } else {
            Err(CabError::Unsupported)
        }
    }

    fn read_at(&self, volume: u32, offset: u64, buf: &mut [u8]) -> Result<()> {
        if volume != 0 {
            return Err(CabError::Unsupported);
        }
        let start = usize::try_from(offset).map_err(|_| CabError::OutOfBounds)?;
        let end = start.checked_add(buf.len()).ok_or(CabError::OutOfBounds)?;
        let slice = self.bytes.get(start..end).ok_or(CabError::OutOfBounds)?;
        buf.copy_from_slice(slice);
        Ok(())
    }
}

/// A source over an ordered set of in-memory cabinet volumes, used to resolve
/// files that continue from one cabinet of a set into the next.
#[derive(Debug, Clone, Copy)]
pub struct SliceSetSource<'a> {
    volumes: &'a [&'a [u8]],
}

impl<'a> SliceSetSource<'a> {
    /// Wraps `volumes`, indexed by the volume index the caller passes to the
    /// extraction API.
    #[must_use]
    pub const fn new(volumes: &'a [&'a [u8]]) -> Self {
        Self { volumes }
    }
}

impl SliceSetSource<'_> {
    fn volume(&self, volume: u32) -> Result<&[u8]> {
        let index = usize::try_from(volume).map_err(|_| CabError::Unsupported)?;
        self.volumes
            .get(index)
            .copied()
            .ok_or(CabError::Unsupported)
    }
}

impl VolumeSource for SliceSetSource<'_> {
    fn volume_len(&self, volume: u32) -> Result<u64> {
        Ok(self.volume(volume)?.len() as u64)
    }

    fn read_at(&self, volume: u32, offset: u64, buf: &mut [u8]) -> Result<()> {
        SliceSource::new(self.volume(volume)?).read_at(0, offset, buf)
    }
}

/// A cooperative cancellation check, consulted between data blocks.
pub trait Cancellation {
    /// Returns `true` once the caller wants extraction to stop.
    fn is_cancelled(&self) -> bool;
}

/// A [`Cancellation`] that never cancels.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancelled;

impl Cancellation for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl<T: Cancellation + ?Sized> Cancellation for &T {
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}
