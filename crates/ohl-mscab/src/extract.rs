//! Streaming extraction of a cabinet folder and of single files inside it.

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::cabinet::{Cabinet, Compression, FileEntry};
use crate::data::{DATA_FIXED_LEN, DataHeader, verify_block_checksum};
use crate::error::{CabError, Result};
use crate::limits::{Limits, MAX_BLOCK_UNCOMPRESSED};
use crate::lzx::LzxDecoder;
use crate::mszip::MsZipDecoder;
use crate::source::{Cancellation, VolumeSource};

/// One cabinet's contribution to a (possibly cabinet-spanning) folder.
///
/// A folder that continues across a cabinet set is expressed as an ordered
/// list of segments. This crate never decides which volume holds the next
/// cabinet: the caller reads `CFHEADER.iCabinet`/`setID` and the
/// next-cabinet flag and supplies the volume index for each continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderSegment {
    /// The volume index passed to [`VolumeSource`].
    pub volume: u32,
    /// `CFFOLDER.coffCabStart` in that cabinet.
    pub first_data_offset: u32,
    /// `CFFOLDER.cCFData` in that cabinet.
    pub data_block_count: u16,
    /// `CFHEADER.cbCabinet` in that cabinet, the read bound for the segment.
    pub cabinet_bytes: u32,
    /// `CFHEADER.cbCFData` in that cabinet.
    pub data_reserve_bytes: u8,
}

impl FolderSegment {
    /// Describes `folder_index` of `cabinet`, which the caller has parsed
    /// from `volume`.
    pub fn new(cabinet: &Cabinet, volume: u32, folder_index: u16) -> Result<Self> {
        let folder = cabinet
            .folders()
            .get(folder_index as usize)
            .ok_or(CabError::FolderIndexOutOfRange)?;
        Ok(Self {
            volume,
            first_data_offset: folder.first_data_offset,
            data_block_count: folder.data_block_count,
            cabinet_bytes: cabinet.header().cabinet_bytes,
            data_reserve_bytes: cabinet.header().data_reserve_bytes,
        })
    }
}

/// Counters describing what a stream has done so far. They carry no
/// cabinet-derived content, so they are safe to log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtractionStats {
    /// `CFDATA` blocks decoded.
    pub blocks_read: u64,
    /// Blocks whose non-zero checksum matched.
    pub checksums_verified: u64,
    /// Blocks whose `csum` was zero ("not supplied").
    pub checksums_absent: u64,
    /// Uncompressed bytes produced.
    pub uncompressed_bytes: u64,
}

enum Decoder {
    Stored,
    // Boxed: `miniz_oxide`'s decompressor state is an order of magnitude
    // larger than the other variants.
    MsZip(Box<MsZipDecoder>),
    Lzx(Box<LzxDecoder>),
}

/// A forward-only reader over one folder's uncompressed byte stream.
///
/// The folder is decoded block by block; at most one `CFDATA` block's worth of
/// output is buffered at a time, so peak memory does not grow with the folder
/// size. Cancellation is polled once per block.
pub struct FolderStream<'a, S: VolumeSource + ?Sized> {
    source: &'a S,
    limits: Limits,
    segments: Vec<FolderSegment>,
    segment: usize,
    blocks_left: u16,
    offset: u64,
    decoder: Decoder,
    compressed: Vec<u8>,
    reserve: Vec<u8>,
    block: Vec<u8>,
    block_position: usize,
    position: u64,
    stats: ExtractionStats,
    finished: bool,
}

impl<'a, S: VolumeSource + ?Sized> FolderStream<'a, S> {
    /// Opens the folder described by `segments`, decoded with `compression`.
    pub fn new(
        source: &'a S,
        compression: Compression,
        limits: Limits,
        segments: Vec<FolderSegment>,
    ) -> Result<Self> {
        let first = *segments.first().ok_or(CabError::InvalidField)?;
        let decoder = match compression {
            Compression::None => Decoder::Stored,
            Compression::MsZip => Decoder::MsZip(Box::new(MsZipDecoder::new())),
            Compression::Lzx { window_bits } => {
                Decoder::Lzx(Box::new(LzxDecoder::new(window_bits, &limits)?))
            }
            Compression::Quantum { .. } | Compression::Unknown { .. } => {
                return Err(CabError::Unsupported);
            }
        };
        let mut total_blocks: u64 = 0;
        for segment in &segments {
            total_blocks += u64::from(segment.data_block_count);
        }
        if total_blocks > u64::from(limits.max_blocks_per_folder) {
            return Err(CabError::LimitExceeded);
        }
        Ok(Self {
            source,
            limits,
            segment: 0,
            blocks_left: first.data_block_count,
            offset: u64::from(first.first_data_offset),
            segments,
            decoder,
            compressed: Vec::new(),
            reserve: Vec::new(),
            block: Vec::new(),
            block_position: 0,
            position: 0,
            stats: ExtractionStats::default(),
            finished: false,
        })
    }

    /// Opens `folder_index` of a single, non-spanning cabinet.
    pub fn from_cabinet(
        cabinet: &Cabinet,
        source: &'a S,
        volume: u32,
        folder_index: u16,
        limits: Limits,
    ) -> Result<Self> {
        let folder = cabinet
            .folders()
            .get(folder_index as usize)
            .ok_or(CabError::FolderIndexOutOfRange)?;
        let segment = FolderSegment::new(cabinet, volume, folder_index)?;
        Self::new(source, folder.compression, limits, vec![segment])
    }

    /// Uncompressed bytes produced so far, i.e. the current offset within the
    /// folder's byte stream.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// The counters gathered so far.
    #[must_use]
    pub const fn stats(&self) -> ExtractionStats {
        self.stats
    }

    /// Fills as much of `out` as the folder still yields, returning the number
    /// of bytes written. A return of `0` means the folder is exhausted.
    pub fn read<C: Cancellation + ?Sized>(&mut self, out: &mut [u8], cancel: &C) -> Result<usize> {
        let mut written = 0usize;
        while written < out.len() {
            if self.block_position == self.block.len() {
                if !self.fill_next_block(cancel)? {
                    break;
                }
                continue;
            }
            let available = self.block.len() - self.block_position;
            let take = available.min(out.len() - written);
            out[written..written + take]
                .copy_from_slice(&self.block[self.block_position..self.block_position + take]);
            self.block_position += take;
            written += take;
        }
        self.position += written as u64;
        Ok(written)
    }

    /// Discards `count` bytes of the folder stream.
    pub fn skip<C: Cancellation + ?Sized>(&mut self, count: u64, cancel: &C) -> Result<u64> {
        let mut skipped = 0u64;
        while skipped < count {
            if self.block_position == self.block.len() {
                if !self.fill_next_block(cancel)? {
                    break;
                }
                continue;
            }
            let available = (self.block.len() - self.block_position) as u64;
            let take = available.min(count - skipped);
            self.block_position += usize::try_from(take).map_err(|_| CabError::Internal)?;
            skipped += take;
        }
        self.position += skipped;
        Ok(skipped)
    }

    /// Decodes the next `CFDATA` block, returning `false` at the end of the
    /// folder.
    fn fill_next_block<C: Cancellation + ?Sized>(&mut self, cancel: &C) -> Result<bool> {
        loop {
            if self.finished {
                return Ok(false);
            }
            if self.blocks_left == 0 {
                self.segment += 1;
                let Some(segment) = self.segments.get(self.segment) else {
                    self.finished = true;
                    return Ok(false);
                };
                self.blocks_left = segment.data_block_count;
                self.offset = u64::from(segment.first_data_offset);
                continue;
            }
            if cancel.is_cancelled() {
                return Err(CabError::Cancelled);
            }
            let segment = *self.segments.get(self.segment).ok_or(CabError::Internal)?;
            let reserve_len = usize::from(segment.data_reserve_bytes);
            let header_len = DATA_FIXED_LEN + reserve_len;
            let mut raw = [0u8; DATA_FIXED_LEN];
            read_within(self.source, &segment, self.offset, &mut raw)?;
            let header = DataHeader::parse(&raw)?;

            self.reserve.clear();
            self.reserve.resize(reserve_len, 0);
            if reserve_len > 0 {
                read_within(
                    self.source,
                    &segment,
                    self.offset + DATA_FIXED_LEN as u64,
                    &mut self.reserve,
                )?;
            }

            self.compressed.clear();
            self.compressed
                .resize(usize::from(header.compressed_bytes), 0);
            read_within(
                self.source,
                &segment,
                self.offset + header_len as u64,
                &mut self.compressed,
            )?;

            if verify_block_checksum(header, &self.reserve, &self.compressed)? {
                self.stats.checksums_verified += 1;
            } else {
                self.stats.checksums_absent += 1;
            }

            let expected = usize::from(header.uncompressed_bytes);
            if self.stats.uncompressed_bytes + expected as u64
                > self.limits.max_folder_uncompressed_bytes
            {
                return Err(CabError::LimitExceeded);
            }
            match &mut self.decoder {
                Decoder::Stored => {
                    if usize::from(header.compressed_bytes) != expected {
                        return Err(CabError::InvalidField);
                    }
                    self.block.clear();
                    self.block.extend_from_slice(&self.compressed);
                }
                Decoder::MsZip(decoder) => {
                    decoder.decode_block(&self.compressed, expected, &mut self.block)?;
                }
                Decoder::Lzx(decoder) => {
                    decoder.decode_block(&self.compressed, expected, &mut self.block)?;
                }
            }
            self.block_position = 0;
            self.stats.blocks_read += 1;
            self.stats.uncompressed_bytes += expected as u64;
            self.blocks_left -= 1;
            self.offset = self
                .offset
                .checked_add(header_len as u64 + u64::from(header.compressed_bytes))
                .ok_or(CabError::OutOfBounds)?;
            if expected == 0 {
                // A zero-length block makes no progress; keep decoding rather
                // than reporting end-of-folder.
                continue;
            }
            return Ok(true);
        }
    }
}

/// A forward-only reader over one file's bytes inside its folder.
pub struct FileStream<'a, S: VolumeSource + ?Sized> {
    folder: FolderStream<'a, S>,
    remaining: u64,
}

impl<'a, S: VolumeSource + ?Sized> FileStream<'a, S> {
    /// Positions `folder` at `file` and limits reads to `file`'s length.
    ///
    /// Cost: a cabinet folder is a single compressed stream, so reaching a
    /// file means decompressing every byte of the folder that precedes it.
    /// Extracting *n* files from one folder one at a time is therefore
    /// quadratic; extract a whole folder in one pass where possible.
    pub fn seek_to<C: Cancellation + ?Sized>(
        mut folder: FolderStream<'a, S>,
        file: &FileEntry,
        cancel: &C,
    ) -> Result<Self> {
        let start = u64::from(file.folder_offset);
        if folder.position() > start {
            return Err(CabError::Internal);
        }
        let skipped = folder.skip(start - folder.position(), cancel)?;
        if folder.position() != start || skipped != start {
            return Err(CabError::OutOfBounds);
        }
        Ok(Self {
            folder,
            remaining: u64::from(file.uncompressed_bytes),
        })
    }

    /// Bytes of this file not yet read.
    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.remaining
    }

    /// The counters of the underlying folder stream.
    #[must_use]
    pub const fn stats(&self) -> ExtractionStats {
        self.folder.stats()
    }

    /// Reads up to `out.len()` bytes of the file. A return of `0` means the
    /// file is complete.
    pub fn read<C: Cancellation + ?Sized>(&mut self, out: &mut [u8], cancel: &C) -> Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let want = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(out.len());
        let read = self.folder.read(&mut out[..want], cancel)?;
        if read == 0 {
            // The folder ended before `cbFile` bytes were produced.
            return Err(CabError::Truncated);
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

/// Extracts one file, handing bounded chunks to `sink`.
///
/// `segments` must describe the file's folder, including any continuation
/// segments the caller resolved. Returns the number of bytes delivered.
pub fn extract_file<S, C, F>(
    source: &S,
    compression: Compression,
    limits: Limits,
    segments: Vec<FolderSegment>,
    file: &FileEntry,
    cancel: &C,
    mut sink: F,
) -> Result<u64>
where
    S: VolumeSource + ?Sized,
    C: Cancellation + ?Sized,
    F: FnMut(&[u8]) -> Result<()>,
{
    let folder = FolderStream::new(source, compression, limits, segments)?;
    let mut stream = FileStream::seek_to(folder, file, cancel)?;
    let mut buffer = vec![0u8; MAX_BLOCK_UNCOMPRESSED as usize];
    let mut total = 0u64;
    loop {
        let read = stream.read(&mut buffer, cancel)?;
        if read == 0 {
            break;
        }
        sink(&buffer[..read])?;
        total += read as u64;
    }
    Ok(total)
}

fn read_within<S: VolumeSource + ?Sized>(
    source: &S,
    segment: &FolderSegment,
    offset: u64,
    buf: &mut [u8],
) -> Result<()> {
    let end = offset
        .checked_add(buf.len() as u64)
        .ok_or(CabError::OutOfBounds)?;
    if end > u64::from(segment.cabinet_bytes) {
        return Err(CabError::OutOfBounds);
    }
    source.read_at(segment.volume, offset, buf)
}
