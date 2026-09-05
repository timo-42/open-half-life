//! A bounded byte window over the pinned source, filled by the protocol.
//!
//! The decoders in `ohl-wise`, `ohl-mscab` and `ohl-isz` are written against
//! ordinary *blocking* random-access sources, but the worker cannot block: it
//! only ever returns one bounded [`DispatchAction`](
//! ohl_parser_worker_service::DispatchAction) per `step`, and the bytes it
//! asked for arrive later, in a separate `accept_read_reply`.
//!
//! [`WindowSource`] is the adapter between the two models. It holds one
//! fixed-capacity buffer over a contiguous span of the pinned source and
//! answers a read either from that buffer or with a *miss*: it records the
//! range it wants, returns a source failure, and the caller turns that into a
//! `need_read`. When the reply arrives the buffer is refilled and the decoder
//! call is simply run again.
//!
//! Re-running is correct because every decoder entry point this crate drives
//! reads through the source *before* it mutates its own state, so a call that
//! ends in a miss leaves nothing behind: `ohl_wise::StreamReader::read` and
//! `::finish` both perform their `read_at` first and propagate its error
//! unchanged. The cost of a miss is therefore one repeated call, never a
//! repeated inflate.
//!
//! Nothing here retains, logs or interprets a source byte: the buffer is
//! scratch storage and `Debug` prints only bounds.

use alloc::vec;
use alloc::vec::Vec;

use ohl_wise::{Error as WiseError, ImageSource};

/// The default sliding-window capacity, and therefore the largest single
/// `need_read` this crate asks for.
pub const DEFAULT_WINDOW_BYTES: usize = 256 * 1024;

/// The smallest useful window; anything less cannot hold one header scan.
pub const MINIMUM_WINDOW_BYTES: usize = 8 * 1024;

/// One outstanding source read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingRead {
    /// The offset in the pinned source.
    pub offset: u64,
    /// The number of bytes wanted; never zero and never past the source end.
    pub length: u32,
}

/// A fixed-capacity read window over the pinned source.
pub struct WindowSource {
    buffer: Vec<u8>,
    /// Offset of `buffer[0]` inside the pinned source.
    base: u64,
    /// Bytes of `buffer` that hold source data.
    filled: usize,
    source_size: u64,
    pending: Option<PendingRead>,
    /// While pinned, the window never moves: a read it cannot serve in full
    /// is served short, and one it cannot serve at all reports end of source.
    pinned: bool,
}

impl core::fmt::Debug for WindowSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WindowSource")
            .field("base", &self.base)
            .field("filled", &self.filled)
            .field("source_size", &self.source_size)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl WindowSource {
    /// A window of `capacity` bytes over a source of `source_size` bytes.
    #[must_use]
    pub fn new(source_size: u64, capacity: usize) -> Self {
        let capacity = capacity.max(MINIMUM_WINDOW_BYTES);
        Self {
            buffer: vec![0u8; capacity],
            base: 0,
            filled: 0,
            source_size,
            pending: None,
            pinned: false,
        }
    }

    /// The pinned source's size in bytes.
    #[must_use]
    pub const fn source_size(&self) -> u64 {
        self.source_size
    }

    /// The window capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Freezes the window where it is.
    ///
    /// A decoder call that is *not* retry-safe against a moving window — the
    /// PE header walk and the first-stream scan, both of which treat a source
    /// failure as "not a package" rather than propagating it — runs pinned:
    /// it sees one fixed window, short reads at its end, and end of source
    /// past it. That makes those calls total instead of letting a refetch
    /// bounce the window between two offsets the same call needs.
    pub const fn pin(&mut self) {
        self.pinned = true;
    }

    /// Lets the window move again.
    pub const fn unpin(&mut self) {
        self.pinned = false;
    }

    /// Whether the window starts exactly at `offset` and holds bytes.
    ///
    /// When it does not, a read of one whole window at `offset` is armed, so
    /// the caller can answer it and try again. This is how a pinned stage
    /// positions its window before it starts.
    pub fn ensure_at(&mut self, offset: u64) -> bool {
        if self.filled > 0 && self.base == offset {
            return true;
        }
        if offset >= self.source_size {
            return true;
        }
        self.miss(offset);
        false
    }

    /// The buffered bytes, that is `[base, base + len)` of the source.
    #[must_use]
    pub fn buffered(&self) -> &[u8] {
        &self.buffer[..self.filled]
    }

    /// The offset the buffered bytes start at.
    #[must_use]
    pub const fn buffered_offset(&self) -> u64 {
        self.base
    }

    /// Takes the outstanding read, if a miss armed one.
    pub fn take_pending(&mut self) -> Option<PendingRead> {
        self.pending.take()
    }

    /// Whether a read is outstanding.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Accepts the bytes of the outstanding read.
    ///
    /// Returns `false` when `data` does not answer a read this window asked
    /// for, which the caller must treat as a dispatch failure.
    pub fn deliver(&mut self, offset: u64, data: &[u8]) -> bool {
        if data.is_empty() || data.len() > self.buffer.len() {
            return false;
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.base = offset;
        self.filled = data.len();
        true
    }

    /// Arms a miss at `offset` and reports it.
    ///
    /// The read is always as wide as the window, whatever the decoder asked
    /// for: the window exists precisely so that a small read pulls in the
    /// bytes the next ones will want.
    fn miss(&mut self, offset: u64) -> WiseError {
        debug_assert!(!self.pinned, "a pinned window never moves");
        let capacity = self.buffer.len() as u64;
        let available = self.source_size.saturating_sub(offset);
        let length = capacity.min(available);
        // `available` is non-zero at every call site, so the cast is exact and
        // the length is never zero.
        self.pending = Some(PendingRead {
            offset,
            length: u32::try_from(length).unwrap_or(u32::MAX),
        });
        WiseError::SourceFailed
    }

    /// Reads at `offset` from the window, or arms a miss.
    ///
    /// A read past the source end reports zero bytes, which every decoder
    /// this crate drives understands as the end of the container.
    ///
    /// # Errors
    /// [`WiseError::SourceFailed`], which means "a `need_read` is armed; run
    /// this call again once it is answered".
    pub fn read(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, WiseError> {
        if out.is_empty() {
            return Ok(0);
        }
        let available = self.source_size.saturating_sub(offset);
        if available == 0 {
            return Ok(0);
        }
        let wanted = (out.len() as u64).min(available);
        // How many buffered bytes start at `offset`, if any do.
        let held = offset
            .checked_sub(self.base)
            .and_then(|start| usize::try_from(start).ok())
            .and_then(|start| self.buffered().get(start..))
            .map_or(0, <[u8]>::len) as u64;
        // A short read at the window's end would look like the end of the
        // container, so it moves the window instead — unless the window is
        // pinned, or the request is simply wider than the window can ever be.
        let taken = held.min(wanted);
        let short = taken < wanted && (self.buffer.len() as u64) >= wanted;
        // When pinned, whatever the window holds is the whole answer.
        if (taken == 0 || short) && !self.pinned {
            return Err(self.miss(offset));
        }
        let taken = usize::try_from(taken).unwrap_or(0);
        let from = usize::try_from(offset.saturating_sub(self.base)).unwrap_or(0);
        out[..taken].copy_from_slice(&self.buffer[from..from + taken]);
        Ok(taken)
    }
}

impl ImageSource for WindowSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, WiseError> {
        self.read(offset, buf)
    }

    fn len(&mut self) -> Result<u64, WiseError> {
        Ok(self.source_size)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WINDOW_BYTES, WindowSource};
    use alloc::vec;

    #[test]
    fn a_miss_arms_exactly_one_bounded_read() {
        let mut window = WindowSource::new(1_000_000, DEFAULT_WINDOW_BYTES);
        let mut out = [0u8; 64];
        assert!(window.read(4096, &mut out).is_err());
        let pending = window.take_pending().expect("a miss arms a read");
        assert_eq!(pending.offset, 4096);
        assert_eq!(
            usize::try_from(pending.length).expect("window fits"),
            DEFAULT_WINDOW_BYTES
        );
        assert!(!window.has_pending());
    }

    #[test]
    fn a_delivered_window_serves_reads_inside_it() {
        let mut window = WindowSource::new(4096, 1024);
        assert!(window.deliver(1024, &vec![7u8; 1024]));
        let mut out = [0u8; 16];
        assert_eq!(window.read(1030, &mut out).expect("buffered"), 16);
        assert_eq!(out, [7u8; 16]);
        assert!(window.read(3000, &mut out).is_err());
    }

    #[test]
    fn a_read_past_the_source_end_is_end_of_container() {
        let mut window = WindowSource::new(64, 1024);
        let mut out = [0u8; 16];
        assert_eq!(window.read(64, &mut out).expect("past the end"), 0);
        assert_eq!(window.read(4096, &mut out).expect("past the end"), 0);
    }

    #[test]
    fn a_partial_window_end_is_a_miss_not_a_short_read() {
        let mut window = WindowSource::new(8192, 4096);
        assert!(window.deliver(0, &vec![1u8; 4096]));
        let mut out = [0u8; 64];
        // Ten bytes are buffered at 4086, but 64 are available in the source.
        assert!(window.read(4086, &mut out).is_err());
        assert_eq!(window.take_pending().expect("armed").offset, 4086);
    }

    #[test]
    fn a_short_tail_is_served_when_the_source_really_ends() {
        let mut window = WindowSource::new(4100, 4096);
        assert!(window.deliver(4096, &[9u8; 4]));
        let mut out = [0u8; 64];
        assert_eq!(window.read(4096, &mut out).expect("tail"), 4);
    }
}
