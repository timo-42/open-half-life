//! Opening an archive at an arbitrary base offset, and streaming one entry.

use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, Limit, Result};
use crate::explode::{Explode, MAX_MATCH};
use crate::header::{ArchiveHeader, HEADER_SIZE};
use crate::limits::Limits;
use crate::source::{ArchiveSource, Cancellation, check_cancelled, read_exact_at};
use crate::toc::{self, Directory, Entry, TableOfContents};

/// An InstallShield 3 archive read at a base offset inside a larger
/// container.
///
/// The archive is commonly embedded in the overlay of an installer
/// executable, so nothing here assumes the archive starts at offset zero: the
/// caller locates the signature (see [`crate::find_signature`]) and passes
/// the resulting base offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    base: u64,
    header: ArchiveHeader,
    toc: TableOfContents,
    limits: Limits,
    remaining_total: u64,
}

impl Archive {
    /// Reads and validates the header and table of contents of the archive
    /// whose first byte is at `base` within `source`.
    ///
    /// # Errors
    ///
    /// Returns a sanitized [`Error`]: [`Error::BadSignature`] when `base`
    /// does not point at an archive, [`Error::Truncated`] when the container
    /// ends early, [`Error::LimitExceeded`] when a recorded count or size is
    /// above `limits`, [`Error::SplitArchiveUnsupported`] for a multi-volume
    /// archive, and [`Error::Cancelled`] when `cancel` is signalled.
    pub fn open<S: ArchiveSource + ?Sized, C: Cancellation + ?Sized>(
        source: &mut S,
        base: u64,
        limits: &Limits,
        cancel: &C,
    ) -> Result<Self> {
        limits.validate()?;
        check_cancelled(cancel)?;

        let mut header_bytes = [0u8; HEADER_SIZE];
        read_exact_at(source, base, &mut header_bytes)?;
        let header = ArchiveHeader::parse(&header_bytes, limits)?;
        if header.is_multi_volume() {
            return Err(Error::SplitArchiveUnsupported);
        }

        let toc_bytes = usize::try_from(header.toc_bytes()).map_err(|_| Error::OutOfRange)?;
        if header.toc_bytes() > limits.max_directory_bytes {
            return Err(Error::LimitExceeded(Limit::DirectoryBytes));
        }
        let toc_at = base
            .checked_add(u64::from(header.toc_offset))
            .ok_or(Error::OutOfRange)?;
        let mut buffer = vec![0u8; toc_bytes];
        read_exact_at(source, toc_at, &mut buffer)?;
        check_cancelled(cancel)?;
        let toc = toc::parse(&buffer, &header, limits, cancel)?;

        Ok(Self {
            base,
            header,
            toc,
            limits: *limits,
            remaining_total: limits.max_total_expanded_bytes,
        })
    }

    /// The archive's base offset inside the caller's container.
    #[must_use]
    pub const fn base_offset(&self) -> u64 {
        self.base
    }

    /// The validated header.
    #[must_use]
    pub const fn header(&self) -> &ArchiveHeader {
        &self.header
    }

    /// The caller-supplied ceilings in force.
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Every directory record, in archive order.
    #[must_use]
    pub fn directories(&self) -> &[Directory] {
        self.toc.directories()
    }

    /// Every entry record, in archive order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        self.toc.entries()
    }

    /// The directory at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when `index` is out of range.
    pub fn directory(&self, index: u32) -> Result<&Directory> {
        self.toc.directory(index)
    }

    /// The entry at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when `index` is out of range.
    pub fn entry(&self, index: u32) -> Result<&Entry> {
        self.toc.entry(index)
    }

    /// Expanded bytes still available under `max_total_expanded_bytes`.
    #[must_use]
    pub const fn remaining_total_bytes(&self) -> u64 {
        self.remaining_total
    }

    /// Opens entry `index` for streaming extraction, charging its expanded
    /// size against the archive-wide budget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] for an unknown index,
    /// [`Error::SplitArchiveUnsupported`] for an entry that spans volumes,
    /// and [`Error::LimitExceeded`] when the budget is exhausted.
    pub fn open_entry(&mut self, index: u32) -> Result<EntryReader> {
        let entry = self.toc.entry(index)?.clone();
        if entry.spans_volumes() {
            return Err(Error::SplitArchiveUnsupported);
        }
        let expanded = u64::from(entry.expanded_size);
        if expanded > self.limits.max_expanded_bytes_per_entry {
            return Err(Error::LimitExceeded(Limit::ExpandedBytesPerEntry));
        }
        if expanded > self.remaining_total {
            return Err(Error::LimitExceeded(Limit::TotalExpandedBytes));
        }
        self.remaining_total -= expanded;

        let chunk = self.limits.max_chunk_bytes.max(MAX_MATCH * 2);
        Ok(EntryReader {
            at: self
                .base
                .checked_add(u64::from(entry.offset))
                .ok_or(Error::OutOfRange)?,
            stored_left: u64::from(entry.stored_size),
            expanded_size: expanded,
            written: 0,
            stored: entry.is_stored(),
            chunk,
            input: Vec::new(),
            output: Vec::new(),
            output_position: 0,
            explode: Explode::new(),
            finished: false,
        })
    }
}

/// A streaming reader over one entry's expanded bytes.
///
/// Bytes are produced in bounded chunks; the caller's cancellation token is
/// polled before each source read and each decode step.
#[derive(Debug)]
pub struct EntryReader {
    at: u64,
    stored_left: u64,
    expanded_size: u64,
    written: u64,
    stored: bool,
    chunk: usize,
    input: Vec<u8>,
    output: Vec<u8>,
    output_position: usize,
    explode: Explode,
    finished: bool,
}

impl EntryReader {
    /// Expanded bytes produced so far.
    #[must_use]
    pub const fn written(&self) -> u64 {
        self.written
    }

    /// The entry's recorded expanded size.
    #[must_use]
    pub const fn expanded_size(&self) -> u64 {
        self.expanded_size
    }

    /// Reads up to `out.len()` expanded bytes, returning zero at the end of
    /// the entry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Source`] when the container fails,
    /// [`Error::Truncated`] when it ends early,
    /// [`Error::DecompressionFailed`] for a malformed imploded stream,
    /// [`Error::SizeMismatch`] when the expanded byte count disagrees with
    /// the entry record, and [`Error::Cancelled`] when `cancel` is signalled.
    pub fn read<S: ArchiveSource + ?Sized, C: Cancellation + ?Sized>(
        &mut self,
        source: &mut S,
        cancel: &C,
        out: &mut [u8],
    ) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let pending = self.output.len() - self.output_position;
            if pending > 0 {
                let take = pending.min(out.len());
                let from = self.output_position;
                out[..take].copy_from_slice(&self.output[from..from + take]);
                self.output_position += take;
                return Ok(take);
            }
            if self.finished {
                return Ok(0);
            }
            check_cancelled(cancel)?;
            self.step(source)?;
        }
    }

    /// Reads the whole entry into a new vector.
    ///
    /// # Errors
    ///
    /// As [`Self::read`].
    pub fn read_to_vec<S: ArchiveSource + ?Sized, C: Cancellation + ?Sized>(
        &mut self,
        source: &mut S,
        cancel: &C,
    ) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buffer = vec![0u8; self.chunk];
        loop {
            let read = self.read(source, cancel, &mut buffer)?;
            if read == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buffer[..read]);
        }
    }

    /// Reads one bounded unit of work into `self.output`.
    fn step<S: ArchiveSource + ?Sized>(&mut self, source: &mut S) -> Result<()> {
        self.output.clear();
        self.output_position = 0;

        if self.stored {
            if self.stored_left == 0 {
                self.finished = true;
                return self.finish();
            }
            let take = usize::try_from(self.stored_left)
                .unwrap_or(usize::MAX)
                .min(self.chunk);
            self.output.resize(take, 0);
            read_exact_at(source, self.at, &mut self.output)?;
            self.at = self.at.checked_add(take as u64).ok_or(Error::OutOfRange)?;
            self.stored_left -= take as u64;
            self.written = self
                .written
                .checked_add(take as u64)
                .ok_or(Error::OutOfRange)?;
            if self.written > self.expanded_size {
                return Err(Error::SizeMismatch);
            }
            return Ok(());
        }

        let read = self.fill_input(source)?;
        let last = self.stored_left == 0;
        let progress = self
            .explode
            .decode(&self.input, last, &mut self.output, self.chunk)?;
        if progress.consumed > self.input.len() {
            return Err(Error::DecompressionFailed);
        }
        self.input.drain(..progress.consumed);
        self.written = self
            .written
            .checked_add(self.output.len() as u64)
            .ok_or(Error::OutOfRange)?;
        if self.written > self.expanded_size {
            return Err(Error::SizeMismatch);
        }
        if progress.finished {
            self.finished = true;
            return self.finish();
        }
        if progress.consumed == 0 && self.output.is_empty() && read == 0 {
            return Err(Error::DecompressionFailed);
        }
        Ok(())
    }

    /// Appends up to one chunk of stored bytes to the decoder's input queue.
    fn fill_input<S: ArchiveSource + ?Sized>(&mut self, source: &mut S) -> Result<usize> {
        if self.stored_left == 0 || self.input.len() >= self.chunk {
            return Ok(0);
        }
        let take = usize::try_from(self.stored_left)
            .unwrap_or(usize::MAX)
            .min(self.chunk - self.input.len());
        let start = self.input.len();
        self.input.resize(start + take, 0);
        read_exact_at(source, self.at, &mut self.input[start..])?;
        self.at = self.at.checked_add(take as u64).ok_or(Error::OutOfRange)?;
        self.stored_left -= take as u64;
        Ok(take)
    }

    fn finish(&mut self) -> Result<()> {
        if self.written == self.expanded_size {
            Ok(())
        } else {
            Err(Error::SizeMismatch)
        }
    }
}
