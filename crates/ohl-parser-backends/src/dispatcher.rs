//! The container dispatcher the shipped worker hosts.
//!
//! [`ContainerDispatcher`] is the trusted parser back end of one OWP/1
//! session: it recognises the container in the pinned source window, offers
//! its entries as `entry_batch` messages, and streams one entry's bytes as
//! `data_chunk` messages. It owns no capability at all — every byte it reads
//! is one the parent chose to answer — and it is the same code on a host test
//! and inside the freestanding image.
//!
//! # The pull-model adapter
//!
//! The three decoder crates are written against blocking sources. A worker
//! cannot block, so a read the window does not hold becomes
//! [`DispatchAction::NeedRead`] and the decoder call is run again once the
//! reply lands. See [`crate::window`] for why re-running is exact rather than
//! merely safe.
//!
//! Every internal step produces an [`Emission`] — a plain description of the
//! one action to encode — and only [`ContainerDispatcher::step`] turns that
//! into a borrow of the dispatcher. That keeps the whole state machine a flat
//! loop instead of a recursion, which matters in an image whose stack is
//! fixed.
//!
//! # Spellings, tokens and sizes
//!
//! - a spelling is the container's recorded name, folded to `/` and validated
//!   by [`crate::spelling`]; an unnamed Wise stream is offered under the
//!   reserved `unnamed/<index>` directory;
//! - a token is the entry's index in its container, so it is stable for the
//!   session and meaningless outside it;
//! - a size is *measured*, not declared: a Wise entry's size is the inflated
//!   length its chain walk observed, which is exactly the number of bytes the
//!   stream then emits.
//!
//! # Failure discipline
//!
//! Every failure is [`DispatchError::Failed`], except a container this build
//! does not recognise at all, which is [`DispatchError::Unsupported`] and
//! terminal. A stream whose trailing checksum, declared checksum or measured
//! size does not verify fails the request instead of delivering the bytes.

use alloc::vec;
use alloc::vec::Vec;

use ohl_parser_protocol::{ArchiveSpelling, EntryBatchEntry, ReadReply, SourceReadPolicy};
use ohl_parser_worker_service::{DispatchAction, DispatchError, Dispatcher, Operation};
use ohl_wise::{ChecksumStatus, Error as WiseError, Limits as WiseLimits, NeverCancelled, StreamReader};

use crate::buffered::{
    BufferedEntry, ContainerBuffer, cabinet_entries, cabinet_extract, z_archive_entries,
    z_archive_extract,
};
use crate::spelling::{SpellingSet, unnamed_spelling};
use crate::window::{DEFAULT_WINDOW_BYTES, PendingRead, WindowSource};
use crate::wise::{Advance, UNNAMED_TOKEN_BASE, WiseBackend};

/// The largest `data_chunk` this dispatcher emits.
pub const CHUNK_BYTES: usize = 64 * 1024;

/// The largest number of entries in one `entry_batch`.
pub const BATCH_ENTRIES: usize = 64;

/// The bytes needed to recognise every container kind.
const DETECT_BYTES: usize = 4;

/// The container kinds this build recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// A Wise package: a PE image whose overlay holds the stream chain.
    WiseOverlay,
    /// A Microsoft cabinet, whose first bytes are the `MSCF` signature.
    MicrosoftCabinet,
    /// An InstallShield 3 Z archive, whose first bytes are its signature.
    InstallShieldZ,
}

/// The ceilings every back end runs under.
#[derive(Debug, Clone, Copy)]
pub struct BackendLimits {
    /// The Wise walk's ceilings.
    pub wise: WiseLimits,
    /// The cabinet decoder's ceilings.
    pub cabinet: ohl_mscab::Limits,
    /// The Z archive decoder's ceilings.
    pub z_archive: ohl_isz::Limits,
    /// The largest number of entries offered in one enumeration.
    pub maximum_entries: usize,
}

impl Default for BackendLimits {
    fn default() -> Self {
        Self {
            wise: WiseLimits::DEFAULT,
            cabinet: ohl_mscab::Limits::default(),
            z_archive: ohl_isz::Limits::default(),
            maximum_entries: 50_000,
        }
    }
}

/// How one offered entry is streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// A Wise chain stream, with the record's declared checksum (zero when
    /// the record declares none).
    WiseStream {
        compressed_offset: u64,
        declared_crc32: u32,
        size_bytes: u64,
    },
    /// An entry of a container that is buffered whole.
    Buffered { token: u64, size_bytes: u64 },
}

/// The recognised container.
enum Container {
    Wise(WiseBackend),
    Cabinet(ContainerBuffer),
    ZArchive(ContainerBuffer),
}

impl core::fmt::Debug for Container {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Wise(_) => "Wise",
            Self::Cabinet(_) => "Cabinet",
            Self::ZArchive(_) => "ZArchive",
        })
    }
}

/// What the active enumeration is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnumerateStage {
    /// Reading the first bytes of the window to recognise the container.
    Detect,
    /// Reading a buffered container into memory.
    Load,
    /// Walking a Wise package.
    Walk,
    /// Emitting offers, starting at this index.
    Emit(usize),
    /// Every offer was emitted.
    Done,
}

/// The active stream request.
#[derive(Debug)]
struct StreamOp {
    plan: Plan,
    emitted: u64,
    /// The whole entry, for a container that is buffered whole.
    buffered: Vec<u8>,
    /// Whether the Wise reader was pointed at this stream.
    started: bool,
    /// Whether the stream's checksums were verified.
    verified: bool,
}

/// The active operation.
#[derive(Debug)]
enum Active {
    None,
    Enumerate(EnumerateStage),
    Stream(StreamOp),
}

/// The one action a step decided on, before it borrows the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emission {
    NeedRead { offset: u64, length: u32 },
    Batch { from: usize, to: usize },
    Chunk(usize),
    Complete,
}

/// A caller-owned byte arena the offered spellings are copied into.
///
/// The arena hands out one disjoint slice at a time, so a spelling borrows
/// the caller's storage for as long as the dispatcher lives and the
/// dispatcher never has to own a self-referential buffer. The freestanding
/// image sizes it once, out of its fixed heap.
#[derive(Debug, Default)]
pub struct SpellingArena<'arena> {
    free: Option<&'arena mut [u8]>,
}

impl<'arena> SpellingArena<'arena> {
    /// An arena over `storage`.
    #[must_use]
    pub fn new(storage: &'arena mut [u8]) -> Self {
        Self {
            free: Some(storage),
        }
    }

    /// Copies `bytes` into the arena and returns the copy.
    fn put(&mut self, bytes: &[u8]) -> Option<&'arena [u8]> {
        let free = self.free.take()?;
        if free.len() < bytes.len() {
            self.free = Some(free);
            return None;
        }
        let (head, rest) = free.split_at_mut(bytes.len());
        self.free = Some(rest);
        head.copy_from_slice(bytes);
        Some(head)
    }
}

/// The dispatcher.
pub struct ContainerDispatcher<'arena> {
    limits: BackendLimits,
    arena: SpellingArena<'arena>,
    window: Option<WindowSource>,
    container: Option<Container>,
    kind: Option<ContainerKind>,
    entries: Vec<EntryBatchEntry<'arena>>,
    plans: Vec<Plan>,
    active: Active,
    reader: Option<StreamReader>,
    chunk: Vec<u8>,
    /// The read this dispatcher last asked for and has not been answered.
    armed: Option<PendingRead>,
    /// Set once the container was recognised as one this build cannot serve.
    unsupported: bool,
}

impl core::fmt::Debug for ContainerDispatcher<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ContainerDispatcher")
            .field("kind", &self.kind)
            .field("entries", &self.entries.len())
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl<'arena> ContainerDispatcher<'arena> {
    /// A dispatcher that copies its spellings into `arena`.
    #[must_use]
    pub fn new(arena: &'arena mut [u8], limits: BackendLimits) -> Self {
        Self {
            limits,
            arena: SpellingArena::new(arena),
            window: None,
            container: None,
            kind: None,
            entries: Vec::new(),
            plans: Vec::new(),
            active: Active::None,
            reader: None,
            chunk: vec![0u8; CHUNK_BYTES],
            armed: None,
            unsupported: false,
        }
    }

    /// The recognised container kind, once enumeration got that far.
    #[must_use]
    pub const fn kind(&self) -> Option<ContainerKind> {
        self.kind
    }

    /// How many entries the enumeration offered.
    #[must_use]
    pub fn offered_entries(&self) -> usize {
        self.entries.len()
    }

    /// Arms `pending` and describes it as an emission.
    fn arm(&mut self, pending: PendingRead) -> Emission {
        self.armed = Some(pending);
        Emission::NeedRead {
            offset: pending.offset,
            length: pending.length,
        }
    }

    /// Takes the window's armed miss, which every miss path has just set.
    fn arm_window(&mut self) -> Result<Emission, DispatchError> {
        let pending = self
            .window
            .as_mut()
            .and_then(WindowSource::take_pending)
            .ok_or(DispatchError::Failed)?;
        Ok(self.arm(pending))
    }

    /// Runs the state machine until it has exactly one action to emit.
    fn advance(&mut self) -> Result<Emission, DispatchError> {
        loop {
            match self.active {
                Active::None => return Err(DispatchError::Failed),
                Active::Stream(_) => return self.stream_step(),
                Active::Enumerate(EnumerateStage::Detect) => {
                    if let Some(emission) = self.detect()? {
                        return Ok(emission);
                    }
                }
                Active::Enumerate(EnumerateStage::Load) => {
                    if let Some(emission) = self.load()? {
                        return Ok(emission);
                    }
                }
                Active::Enumerate(EnumerateStage::Walk) => {
                    if let Some(emission) = self.walk()? {
                        return Ok(emission);
                    }
                }
                Active::Enumerate(EnumerateStage::Emit(from)) => {
                    let to = from.saturating_add(BATCH_ENTRIES).min(self.entries.len());
                    if from >= to {
                        self.active = Active::Enumerate(EnumerateStage::Done);
                        return Ok(Emission::Complete);
                    }
                    self.active = Active::Enumerate(EnumerateStage::Emit(to));
                    return Ok(Emission::Batch { from, to });
                }
                Active::Enumerate(EnumerateStage::Done) => return Err(DispatchError::Failed),
            }
        }
    }

    /// Recognises the container from the first bytes of the window.
    fn detect(&mut self) -> Result<Option<Emission>, DispatchError> {
        let mut head = [0u8; DETECT_BYTES];
        let window = self.window.as_mut().ok_or(DispatchError::Failed)?;
        let source_size = window.source_size();
        match window.read(0, &mut head) {
            Ok(DETECT_BYTES) => {}
            Ok(_) => {
                self.unsupported = true;
                return Err(DispatchError::Unsupported);
            }
            Err(_) => return self.arm_window().map(Some),
        }
        let kind = if head == *b"MSCF" {
            ContainerKind::MicrosoftCabinet
        } else if head == ohl_isz::header::SIGNATURE_WORD_1.to_le_bytes() {
            ContainerKind::InstallShieldZ
        } else if head[..2] == *b"MZ" {
            ContainerKind::WiseOverlay
        } else {
            self.unsupported = true;
            return Err(DispatchError::Unsupported);
        };
        self.kind = Some(kind);
        self.container = Some(match kind {
            ContainerKind::WiseOverlay => Container::Wise(WiseBackend::new(self.limits.wise)),
            ContainerKind::MicrosoftCabinet => Container::Cabinet(
                ContainerBuffer::new(source_size).map_err(|()| DispatchError::Unsupported)?,
            ),
            ContainerKind::InstallShieldZ => Container::ZArchive(
                ContainerBuffer::new(source_size).map_err(|()| DispatchError::Unsupported)?,
            ),
        });
        self.active = Active::Enumerate(match kind {
            ContainerKind::WiseOverlay => EnumerateStage::Walk,
            _ => EnumerateStage::Load,
        });
        Ok(None)
    }

    /// Reads more of a buffered container, then enumerates it.
    fn load(&mut self) -> Result<Option<Emission>, DispatchError> {
        let maximum_read = u32::try_from(
            self.window
                .as_ref()
                .ok_or(DispatchError::Failed)?
                .capacity(),
        )
        .unwrap_or(u32::MAX);
        let pending = match self.container.as_ref() {
            Some(Container::Cabinet(buffer) | Container::ZArchive(buffer)) => {
                (!buffer.is_complete()).then(|| buffer.next_read(maximum_read))
            }
            _ => return Err(DispatchError::Failed),
        };
        if let Some(pending) = pending {
            return Ok(Some(self.arm(pending)));
        }
        let entries = match self.container.as_ref() {
            Some(Container::Cabinet(buffer)) => {
                cabinet_entries(buffer.bytes(), &self.limits.cabinet)
                    .map_err(|_| DispatchError::Failed)?
            }
            Some(Container::ZArchive(buffer)) => {
                z_archive_entries(buffer.bytes(), &self.limits.z_archive)
                    .map_err(|_| DispatchError::Failed)?
            }
            _ => return Err(DispatchError::Failed),
        };
        self.offer_buffered(&entries)?;
        self.active = Active::Enumerate(EnumerateStage::Emit(0));
        Ok(None)
    }

    /// Turns buffered-container entries into offers.
    fn offer_buffered(&mut self, entries: &[BufferedEntry]) -> Result<(), DispatchError> {
        let mut spellings = SpellingSet::new();
        for entry in entries {
            if self.entries.len() >= self.limits.maximum_entries {
                break;
            }
            let Ok(spelling) = spellings.accept(&entry.name) else {
                continue;
            };
            self.offer(
                entry.token,
                entry.size_bytes,
                spelling.as_bytes(),
                Plan::Buffered {
                    token: entry.token,
                    size_bytes: entry.size_bytes,
                },
            )?;
        }
        Ok(())
    }

    /// Walks a Wise package one bounded unit at a time.
    fn walk(&mut self) -> Result<Option<Emission>, DispatchError> {
        let Some(Container::Wise(mut backend)) = self.container.take() else {
            return Err(DispatchError::Failed);
        };
        let window = self.window.as_mut().ok_or(DispatchError::Failed)?;
        let advance = backend.advance(window);
        self.container = Some(Container::Wise(backend));
        match advance {
            Ok(Advance::Working) => Ok(None),
            Ok(Advance::NeedRead) => self.arm_window().map(Some),
            Ok(Advance::Ready) => {
                self.offer_wise()?;
                self.active = Active::Enumerate(EnumerateStage::Emit(0));
                Ok(None)
            }
            Err(_) => Err(DispatchError::Failed),
        }
    }

    /// Turns a walked Wise package into offers.
    fn offer_wise(&mut self) -> Result<(), DispatchError> {
        let Some(Container::Wise(backend)) = self.container.take() else {
            return Err(DispatchError::Failed);
        };
        let mut spellings = SpellingSet::new();
        let mut offers = Vec::new();
        for entry in backend.entries() {
            if offers.len() >= self.limits.maximum_entries {
                break;
            }
            let recorded = backend.recorded_name(entry.token);
            let accepted = match recorded {
                Some(bytes) => spellings.accept(bytes),
                None => {
                    let index = u32::try_from(entry.token.saturating_sub(UNNAMED_TOKEN_BASE))
                        .unwrap_or(u32::MAX);
                    spellings.accept(unnamed_spelling(index).as_bytes())
                }
            };
            // A name this build would not offer is simply not offered; the
            // rest of the package is still importable.
            if let Ok(spelling) = accepted {
                offers.push((entry, spelling));
            }
        }
        self.container = Some(Container::Wise(backend));
        for (entry, spelling) in offers {
            self.offer(
                entry.token,
                entry.size_bytes,
                spelling.as_bytes(),
                Plan::WiseStream {
                    compressed_offset: entry.compressed_offset,
                    declared_crc32: entry.declared_crc32,
                    size_bytes: entry.size_bytes,
                },
            )?;
        }
        Ok(())
    }

    /// Records one offer, copying its spelling into the arena.
    fn offer(
        &mut self,
        token: u64,
        size_bytes: u64,
        spelling: &[u8],
        plan: Plan,
    ) -> Result<(), DispatchError> {
        let stored = self.arena.put(spelling).ok_or(DispatchError::Failed)?;
        let archive_path = ArchiveSpelling::new(stored).map_err(|_| DispatchError::Failed)?;
        self.entries.push(EntryBatchEntry {
            source_token: token,
            size_bytes,
            archive_path,
        });
        self.plans.push(plan);
        Ok(())
    }

    /// One step of an active stream request.
    fn stream_step(&mut self) -> Result<Emission, DispatchError> {
        let Active::Stream(mut operation) = core::mem::replace(&mut self.active, Active::None)
        else {
            return Err(DispatchError::Failed);
        };
        let emission = match operation.plan {
            Plan::Buffered { size_bytes, .. } => self.buffered_step(&mut operation, size_bytes),
            Plan::WiseStream {
                compressed_offset,
                declared_crc32,
                size_bytes,
            } => self.wise_step(&mut operation, compressed_offset, declared_crc32, size_bytes),
        };
        self.active = Active::Stream(operation);
        emission
    }

    /// Emits the next slice of an already-decoded entry.
    fn buffered_step(
        &mut self,
        operation: &mut StreamOp,
        size_bytes: u64,
    ) -> Result<Emission, DispatchError> {
        if operation.emitted >= size_bytes {
            return Ok(Emission::Complete);
        }
        let start = usize::try_from(operation.emitted).map_err(|_| DispatchError::Failed)?;
        let end = start
            .saturating_add(CHUNK_BYTES)
            .min(operation.buffered.len());
        if start >= end {
            return Err(DispatchError::Failed);
        }
        self.chunk[..end - start].copy_from_slice(&operation.buffered[start..end]);
        operation.emitted = operation.emitted.saturating_add((end - start) as u64);
        Ok(Emission::Chunk(end - start))
    }

    /// Inflates the next chunk of a Wise chain stream.
    fn wise_step(
        &mut self,
        operation: &mut StreamOp,
        compressed_offset: u64,
        declared_crc32: u32,
        size_bytes: u64,
    ) -> Result<Emission, DispatchError> {
        if operation.verified {
            return Ok(Emission::Complete);
        }
        if self.reader.is_none() {
            self.reader = Some(StreamReader::new(compressed_offset, self.limits.wise));
        }
        let reader = self.reader.as_mut().ok_or(DispatchError::Failed)?;
        if !operation.started {
            reader.restart(compressed_offset);
            operation.started = true;
        }
        let window = self.window.as_mut().ok_or(DispatchError::Failed)?;
        let chunk = &mut self.chunk;
        loop {
            if reader.is_finished() {
                let metrics = match reader.finish(window) {
                    Ok(metrics) => metrics,
                    Err(WiseError::SourceFailed) => break,
                    Err(_) => return Err(DispatchError::Failed),
                };
                // Nothing is delivered on trust: the trailing checksum, the
                // record's own checksum and the measured size must all agree
                // with what was streamed.
                if metrics.checksum != ChecksumStatus::Match
                    || (declared_crc32 != 0 && declared_crc32 != metrics.computed_crc32)
                    || metrics.inflated_len != size_bytes
                    || operation.emitted != size_bytes
                {
                    return Err(DispatchError::Failed);
                }
                operation.verified = true;
                return Ok(Emission::Complete);
            }
            match reader.read(window, &NeverCancelled, chunk) {
                Ok(0) => {
                    if !reader.is_finished() {
                        return Err(DispatchError::Failed);
                    }
                }
                Ok(written) => {
                    operation.emitted = operation.emitted.saturating_add(written as u64);
                    if operation.emitted > size_bytes {
                        return Err(DispatchError::Failed);
                    }
                    return Ok(Emission::Chunk(written));
                }
                Err(WiseError::SourceFailed) => break,
                Err(_) => return Err(DispatchError::Failed),
            }
        }
        self.arm_window()
    }

    /// Prepares one stream request.
    fn begin_stream(&mut self, token: u64) -> Result<u64, DispatchError> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.source_token == token)
            .ok_or(DispatchError::Failed)?;
        let plan = *self.plans.get(index).ok_or(DispatchError::Failed)?;
        let (size_bytes, buffered) = match plan {
            Plan::WiseStream { size_bytes, .. } => (size_bytes, Vec::new()),
            Plan::Buffered { token, size_bytes } => {
                let bytes = match self.container.as_ref() {
                    Some(Container::Cabinet(buffer)) => {
                        cabinet_extract(buffer.bytes(), self.limits.cabinet, token)
                            .map_err(|_| DispatchError::Failed)?
                    }
                    Some(Container::ZArchive(buffer)) => {
                        z_archive_extract(buffer.bytes(), &self.limits.z_archive, token)
                            .map_err(|_| DispatchError::Failed)?
                    }
                    _ => return Err(DispatchError::Failed),
                };
                // The declared size is what the parent planned for, so a
                // container that produces a different number of bytes fails
                // the request instead of the staging transaction.
                if bytes.len() as u64 != size_bytes {
                    return Err(DispatchError::Failed);
                }
                (size_bytes, bytes)
            }
        };
        self.active = Active::Stream(StreamOp {
            plan,
            emitted: 0,
            buffered,
            started: false,
            verified: false,
        });
        Ok(size_bytes)
    }
}

impl Dispatcher for ContainerDispatcher<'_> {
    fn begin(
        &mut self,
        operation: Operation,
        source_token: u64,
        source_policy: &SourceReadPolicy,
    ) -> Result<u64, DispatchError> {
        if self.unsupported {
            return Err(DispatchError::Unsupported);
        }
        if self.window.is_none() {
            let capacity = usize::try_from(source_policy.maximum_read_bytes())
                .unwrap_or(DEFAULT_WINDOW_BYTES)
                .min(DEFAULT_WINDOW_BYTES);
            self.window = Some(WindowSource::new(source_policy.source_size(), capacity));
        }
        self.armed = None;
        match operation {
            Operation::Enumerate => {
                self.active = Active::Enumerate(if self.entries.is_empty() {
                    match self.kind {
                        None => EnumerateStage::Detect,
                        Some(ContainerKind::WiseOverlay) => EnumerateStage::Walk,
                        Some(_) => EnumerateStage::Load,
                    }
                } else {
                    // A second enumeration re-offers what the first resolved:
                    // the container is walked exactly once per session.
                    EnumerateStage::Emit(0)
                });
                Ok(0)
            }
            Operation::Stream => self.begin_stream(source_token),
        }
    }

    fn step(&mut self) -> Result<DispatchAction<'_>, DispatchError> {
        let emission = self.advance()?;
        Ok(match emission {
            Emission::NeedRead { offset, length } => DispatchAction::NeedRead { offset, length },
            Emission::Batch { from, to } => DispatchAction::EntryBatch(&self.entries[from..to]),
            Emission::Chunk(length) => DispatchAction::DataChunk(&self.chunk[..length]),
            Emission::Complete => DispatchAction::Complete,
        })
    }

    fn accept_read_reply(&mut self, reply: &ReadReply<'_>) -> Result<(), DispatchError> {
        let armed = self.armed.take().ok_or(DispatchError::Failed)?;
        if reply.data.len() != armed.length as usize {
            return Err(DispatchError::Failed);
        }
        if matches!(self.active, Active::Enumerate(EnumerateStage::Load)) {
            let buffer = match self.container.as_mut() {
                Some(Container::Cabinet(buffer) | Container::ZArchive(buffer)) => buffer,
                _ => return Err(DispatchError::Failed),
            };
            return buffer
                .append(reply.data)
                .then_some(())
                .ok_or(DispatchError::Failed);
        }
        let window = self.window.as_mut().ok_or(DispatchError::Failed)?;
        window
            .deliver(armed.offset, reply.data)
            .then_some(())
            .ok_or(DispatchError::Failed)
    }

    fn cancel(&mut self) {
        self.active = Active::None;
        self.armed = None;
        if let Some(window) = self.window.as_mut() {
            let _ = window.take_pending();
        }
    }

    fn end(&mut self) {
        self.active = Active::None;
        self.armed = None;
    }
}

