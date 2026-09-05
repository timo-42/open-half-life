//! Per-file streaming extraction across volumes.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use ohl_cabinet_format::{CabinetHeader, FileDescriptor, Layout};

use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::obfuscation::deobfuscate;
use crate::volume::{NO_LAST_FILE_OFFSET, VolumeHeader, VolumeSource, read_volume_header};

/// Owns the total-expanded-bytes budget shared by every file read from one
/// cabinet.
#[derive(Debug)]
pub struct CabinetReader<'h, 'd> {
    header: &'h CabinetHeader<'d>,
    limits: Limits,
    remaining_total: u64,
}

impl<'h, 'd> CabinetReader<'h, 'd> {
    /// A reader over `header` with `limits`.
    #[must_use]
    pub fn new(header: &'h CabinetHeader<'d>, limits: Limits) -> Self {
        Self {
            header,
            limits,
            remaining_total: limits.max_total_expanded_bytes,
        }
    }

    /// The header being read.
    #[must_use]
    pub const fn header(&self) -> &'h CabinetHeader<'d> {
        self.header
    }

    /// Expanded bytes still available under `max_total_expanded_bytes`.
    #[must_use]
    pub const fn remaining_total_bytes(&self) -> u64 {
        self.remaining_total
    }

    /// Resolves file `index` through its split-link chain.
    ///
    /// A descriptor flagged `LINK_PREV` continues an earlier one; the walk
    /// follows those links to the head, refusing cycles, self-references and
    /// chains longer than `max_link_steps`.
    pub fn resolve_link_head(&self, index: u32) -> Result<u32, Error> {
        let mut current = index;
        let mut visited = BTreeSet::new();
        visited.insert(current);
        for step in 0..self.limits.max_link_steps {
            let descriptor = self.header.file_descriptor(current)?;
            if !descriptor.link_flags.has_previous() {
                return Ok(current);
            }
            let previous = descriptor.link_previous;
            if previous == current {
                return Err(Error::LinkCycle);
            }
            if !visited.insert(previous) {
                return Err(Error::LinkCycle);
            }
            current = previous;
            let _ = step;
        }
        Err(Error::LimitExceeded(Limit::LinkSteps))
    }

    /// Opens file `index` for streaming extraction.
    pub fn open<'r, S: VolumeSource>(
        &'r mut self,
        index: u32,
        source: &mut S,
    ) -> Result<FileReader<'r>, Error> {
        let header = self.header;
        let limits = self.limits;
        let head = self.resolve_link_head(index)?;
        let descriptor = header.file_descriptor(head)?;

        if !descriptor.is_extractable() {
            return Err(Error::InvalidFile);
        }
        if descriptor.expanded_size > limits.max_expanded_bytes_per_file {
            return Err(Error::LimitExceeded(Limit::ExpandedBytesPerFile));
        }
        if descriptor.expanded_size > self.remaining_total {
            return Err(Error::LimitExceeded(Limit::TotalExpandedBytes));
        }
        if descriptor.flags.is_compressed() && !cfg!(feature = "inflate") {
            return Err(Error::CompressionUnsupported);
        }
        if descriptor.volume > limits.max_volumes {
            return Err(Error::LimitExceeded(Limit::Volumes));
        }

        let mut reader = FileReader {
            budget: &mut self.remaining_total,
            index: head,
            file_count: header.file_count(),
            major: header.version().major(),
            layout: header.version().layout(),
            limits,
            descriptor,
            volume: descriptor.volume,
            volume_bytes_left: 0,
            cursor: 0,
            hops: 0,
            obfuscation_seed: 0,
            stored_left: descriptor.stored_size(),
            staging: Vec::new(),
            staging_pos: 0,
            #[cfg(feature = "inflate")]
            scratch: Vec::new(),
            written: 0,
            #[cfg(feature = "inflate")]
            inflater: crate::inflate::ChunkInflater::new(),
            #[cfg(feature = "md5")]
            digest: <md5::Md5 as md5::Digest>::new(),
        };
        reader.start(source)?;
        Ok(reader)
    }

    /// Extracts file `index` completely into a new vector.
    pub fn extract_to_vec<S: VolumeSource>(
        &mut self,
        index: u32,
        source: &mut S,
    ) -> Result<Vec<u8>, Error> {
        let chunk = self.limits.max_chunk_bytes.max(1);
        let mut reader = self.open(index, source)?;
        let mut output = Vec::new();
        let mut buffer = alloc::vec![0u8; chunk];
        loop {
            let read = reader.read(source, &mut buffer)?;
            if read == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..read]);
        }
        reader.finish()?;
        Ok(output)
    }
}

/// A single file's bounded byte stream.
#[derive(Debug)]
pub struct FileReader<'r> {
    budget: &'r mut u64,
    index: u32,
    file_count: u32,
    major: u16,
    layout: Layout,
    limits: Limits,
    descriptor: FileDescriptor,
    volume: u16,
    volume_bytes_left: u64,
    cursor: u64,
    hops: u16,
    obfuscation_seed: u32,
    stored_left: u64,
    staging: Vec<u8>,
    staging_pos: usize,
    #[cfg(feature = "inflate")]
    scratch: Vec<u8>,
    written: u64,
    #[cfg(feature = "inflate")]
    inflater: crate::inflate::ChunkInflater,
    #[cfg(feature = "md5")]
    digest: md5::Md5,
}

impl FileReader<'_> {
    /// The resolved descriptor being extracted.
    #[must_use]
    pub const fn descriptor(&self) -> &FileDescriptor {
        &self.descriptor
    }

    /// The resolved file index, which differs from the requested index when
    /// a split link was followed.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Expanded bytes produced so far.
    #[must_use]
    pub const fn expanded_bytes(&self) -> u64 {
        self.written
    }

    /// The volume currently being read.
    #[must_use]
    pub const fn volume(&self) -> u16 {
        self.volume
    }

    fn start<S: VolumeSource>(&mut self, source: &mut S) -> Result<(), Error> {
        // InstallShield 5 media may record a starting volume that precedes
        // the volume actually holding the file.
        loop {
            let volume_header = self.open_volume(source, self.volume)?;
            if self.major <= 5 && u64::from(self.index) > u64::from(volume_header.last_file_index) {
                self.advance_volume()?;
                continue;
            }
            return Ok(());
        }
    }

    fn advance_volume(&mut self) -> Result<(), Error> {
        self.hops = self
            .hops
            .checked_add(1)
            .ok_or(Error::LimitExceeded(Limit::VolumeHops))?;
        if self.hops > self.limits.max_volume_hops {
            return Err(Error::LimitExceeded(Limit::VolumeHops));
        }
        self.volume = self
            .volume
            .checked_add(1)
            .ok_or(Error::LimitExceeded(Limit::Volumes))?;
        if self.volume > self.limits.max_volumes {
            return Err(Error::LimitExceeded(Limit::Volumes));
        }
        Ok(())
    }

    fn open_volume<S: VolumeSource>(
        &mut self,
        source: &mut S,
        volume: u16,
    ) -> Result<VolumeHeader, Error> {
        if volume > self.limits.max_volumes {
            return Err(Error::LimitExceeded(Limit::Volumes));
        }
        let volume_header = read_volume_header(source, volume, self.layout)?;

        // InstallShield 5 does not record the split flag; it is inferred from
        // the volume header disagreeing with the descriptor.
        if self.major == 5 && !self.descriptor.flags.is_split() {
            let is_last_in_volume = u64::from(self.index) + 1 < u64::from(self.file_count)
                && self.index == volume_header.last_file_index
                && volume_header.last_file_size_compressed != self.descriptor.compressed_size;
            let is_first_in_volume = self.index > 0
                && self.index == volume_header.first_file_index
                && volume_header.first_file_size_compressed != self.descriptor.compressed_size;
            if is_last_in_volume || is_first_in_volume {
                self.descriptor.flags = self.descriptor.flags.with_split();
            }
        }

        let (data_offset, expanded, compressed) = if self.descriptor.flags.is_split() {
            if self.index == volume_header.last_file_index
                && volume_header.last_file_offset != NO_LAST_FILE_OFFSET
            {
                (
                    volume_header.last_file_offset,
                    volume_header.last_file_size_expanded,
                    volume_header.last_file_size_compressed,
                )
            } else if self.index == volume_header.first_file_index {
                (
                    volume_header.first_file_offset,
                    volume_header.first_file_size_expanded,
                    volume_header.first_file_size_compressed,
                )
            } else {
                // The volume claims neither end of this file, so it holds no
                // bytes for it and the split chain is inconsistent.
                return Err(Error::MalformedVolumeHeader);
            }
        } else {
            (
                self.descriptor.data_offset,
                self.descriptor.expanded_size,
                self.descriptor.compressed_size,
            )
        };

        self.volume_bytes_left = if self.descriptor.flags.is_compressed() {
            compressed
        } else {
            expanded
        };
        self.cursor = data_offset;
        self.volume = volume;
        Ok(volume_header)
    }

    /// Reads exactly `buffer.len()` stored bytes, crossing volumes as needed
    /// and removing obfuscation.
    fn read_stored<S: VolumeSource>(
        &mut self,
        source: &mut S,
        buffer: &mut [u8],
    ) -> Result<(), Error> {
        let mut filled = 0usize;
        while filled < buffer.len() {
            if self.volume_bytes_left == 0 {
                self.advance_volume()?;
                let volume = self.volume;
                self.open_volume(source, volume)?;
                if self.volume_bytes_left == 0 {
                    return Err(Error::TruncatedVolume);
                }
                continue;
            }

            let wanted = usize::try_from(self.volume_bytes_left)
                .unwrap_or(usize::MAX)
                .min(buffer.len() - filled);
            let read = source.read_at(
                self.volume,
                self.cursor,
                &mut buffer[filled..filled + wanted],
            )?;
            if read == 0 {
                return Err(Error::TruncatedVolume);
            }
            filled += read;
            self.cursor = self
                .cursor
                .checked_add(read as u64)
                .ok_or(Error::OffsetOutOfRange)?;
            self.volume_bytes_left -= read as u64;
        }

        if self.descriptor.flags.is_obfuscated() {
            deobfuscate(buffer, &mut self.obfuscation_seed);
        }
        Ok(())
    }

    fn refill<S: VolumeSource>(&mut self, source: &mut S) -> Result<bool, Error> {
        if self.stored_left == 0 {
            return Ok(false);
        }
        if self.descriptor.flags.is_compressed() {
            return self.refill_compressed(source);
        }

        let wanted = usize::try_from(self.stored_left)
            .unwrap_or(usize::MAX)
            .min(self.limits.max_chunk_bytes.max(1));
        let mut staging = core::mem::take(&mut self.staging);
        staging.clear();
        staging.resize(wanted, 0);
        let result = self.read_stored(source, &mut staging);
        self.staging = staging;
        result?;
        self.staging_pos = 0;
        self.stored_left -= wanted as u64;
        Ok(true)
    }

    #[cfg(not(feature = "inflate"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
    fn refill_compressed<S: VolumeSource>(&mut self, _source: &mut S) -> Result<bool, Error> {
        Err(Error::CompressionUnsupported)
    }

    #[cfg(feature = "inflate")]
    fn refill_compressed<S: VolumeSource>(&mut self, source: &mut S) -> Result<bool, Error> {
        if self.stored_left < 2 {
            return Err(Error::TruncatedVolume);
        }
        let mut length = [0u8; 2];
        self.read_stored(source, &mut length)?;
        let length = usize::from(u16::from_le_bytes(length));
        if length == 0 {
            return Err(Error::DecompressionFailed);
        }
        if u64::try_from(length).unwrap_or(u64::MAX) + 2 > self.stored_left {
            return Err(Error::TruncatedVolume);
        }
        if length > self.limits.max_chunk_bytes {
            return Err(Error::LimitExceeded(Limit::ChunkBytes));
        }

        let mut scratch = core::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(length + 1, 0);
        let result = self.read_stored(source, &mut scratch[..length]);
        // The reference implementation appends a NUL so the inflater sees a
        // terminated stream; keep that behaviour.
        scratch[length] = 0;
        self.scratch = scratch;
        result?;

        let max_output = self.limits.max_chunk_bytes;
        let mut staging = core::mem::take(&mut self.staging);
        let scratch = core::mem::take(&mut self.scratch);
        let result = self
            .inflater
            .inflate_chunk(&scratch, &mut staging, max_output);
        self.scratch = scratch;
        self.staging = staging;
        result?;
        self.staging_pos = 0;
        self.stored_left -= 2 + length as u64;
        Ok(true)
    }

    /// Reads up to `out.len()` expanded bytes, returning zero at end of file.
    pub fn read<S: VolumeSource>(
        &mut self,
        source: &mut S,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.staging_pos < self.staging.len() {
                let available = self.staging.len() - self.staging_pos;
                let count = available.min(out.len());
                let slice = &self.staging[self.staging_pos..self.staging_pos + count];
                out[..count].copy_from_slice(slice);
                self.staging_pos += count;

                let produced = count as u64;
                if self.written + produced > self.limits.max_expanded_bytes_per_file {
                    return Err(Error::LimitExceeded(Limit::ExpandedBytesPerFile));
                }
                if produced > *self.budget {
                    return Err(Error::LimitExceeded(Limit::TotalExpandedBytes));
                }
                *self.budget -= produced;
                self.written += produced;
                #[cfg(feature = "md5")]
                {
                    md5::Digest::update(&mut self.digest, slice);
                }
                return Ok(count);
            }

            if !self.refill(source)? {
                return Ok(0);
            }
        }
    }

    /// Verifies the expanded size and, with the `md5` feature, the recorded
    /// digest.
    pub fn finish(self) -> Result<(), Error> {
        if self.written != self.descriptor.expanded_size {
            return Err(Error::SizeMismatch);
        }
        #[cfg(feature = "md5")]
        {
            // The digest field is only populated from major version 6 on.
            if self.major >= 6 {
                let computed = md5::Digest::finalize(self.digest);
                if computed.as_slice() != self.descriptor.md5 {
                    return Err(Error::DigestMismatch);
                }
            }
        }
        Ok(())
    }
}
