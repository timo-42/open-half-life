//! The parent-side end-to-end import composition.
//!
//! [`run_import`] is the one place where every accepted boundary is joined:
//!
//! 1. [`locate`](crate::locate) finds container candidates in the mounted
//!    medium and [`choose_primary`] picks exactly one;
//! 2. a [`SourceWindow`] confines the worker's reads to that container;
//! 3. a [`ProcessSession`] launches, handshakes with, and finally reaps one
//!    confined worker;
//! 4. the worker enumerates; the parent validates and promotes the catalog;
//! 5. [`ohl_payload::selection`] applies the user's runtime-only recipe and
//!    [`ohl_payload::layout`] plans the destinations;
//! 6. [`ohl_payload::stage_payload`] streams each planned entry from the
//!    worker into create-new staging, reverifies the complete pinned source,
//!    and publishes with no-replace exactly once;
//! 7. the published tree identity is recorded next to the media's provenance
//!    entry.
//!
//! # Authority
//!
//! The worker never receives a path, a destination, the recipe, the cache, or
//! any publication capability. It answers `read_request`s inside one window
//! and offers entry metadata that the parent independently validates. Only
//! this module stages or publishes.
//!
//! # Failure and cleanup
//!
//! Every failure path terminates the worker exactly once — `ProcessSession`
//! caches the escalation, and `Drop` is the backstop — and discards the
//! staging transaction, which `stage_payload` owns. Nothing partial is ever
//! published.
//!
//! # Logging
//!
//! [`ImportReport`]'s counts are for the caller's own use, in memory. The
//! application logs fixed strings only: a count of entries or bytes derived
//! from a medium is media-derived data under `docs/MEDIA_IMPORT.md`.
//! [`ProgressSink`] therefore receives a fraction of the planned total and
//! nothing else.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ohl_media::{CacheLayout, ValidatedMedia};
use ohl_payload::layout::{PayloadImportLimits, PlannedPayloadEntry, plan_payload_layout};
use ohl_payload::selection::{SelectableEntry, SelectionRecipe, select};
use ohl_payload::stage::{
    PayloadStageError, PayloadStageRequest, PayloadStageStatus, stage_payload,
};
use ohl_payload::store::DirectoryPayloadStore;
use ohl_payload::stream::{PayloadByteSink, PayloadSource};
use ohl_platform::MediaSource;
use ohl_vfs::Mount;
use thiserror::Error;

use crate::catalog::{CatalogGeneration, SourceToken};
use crate::io::{CancellationToken, ExactIo};
use crate::locate::{ContainerCandidate, ContainerKind, LocateLimits, locate_containers};
use crate::parent_session::{
    CancelStep, Cancelled, Idle, ParserSession, RequestStep, SessionBuffers, SessionError,
    TerminalSession,
};
use crate::platform::ParserWorkerProcess;
use crate::process_session::{
    OpenError, ProcessSession, SessionAllocation, SessionConfig, SessionIdAllocator, WorkerProcess,
};
use crate::result_session::{ByteSink, SinkRejected};
use crate::source_window::SourceWindow;

/// The file, inside the media's provenance-cache entry, that records which
/// payload tree was published for that medium.
///
/// Its whole content is one payload staging identity — a one-way digest over
/// the accepted source identity, the recipe identity, and the planned layout
/// — followed by a newline. There is deliberately no name, no path, no count,
/// and no byte of payload in it.
pub const PAYLOAD_RECORD_FILE_NAME: &str = "payload-identity";

/// The ceiling applied when reading a payload record back.
const MAXIMUM_PAYLOAD_RECORD_BYTES: u64 = 4 * 1024;

/// A destination for coarse progress.
///
/// Implementations receive a monotonically non-decreasing fraction in
/// `0.0..=1.0` of the *planned* byte total. They never receive a name, a
/// path, a count, or a byte.
pub trait ProgressSink {
    /// Reports the fraction of planned bytes staged so far.
    fn report(&mut self, fraction: f32);
}

/// A progress sink that discards everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct DiscardProgress;

impl ProgressSink for DiscardProgress {
    fn report(&mut self, _fraction: f32) {}
}

/// The two cooperative tokens one import observes.
///
/// The transport token unblocks worker I/O; the staging token is polled by
/// `ohl-payload` at its own boundaries. A caller signals both from the same
/// user request.
#[derive(Debug, Clone, Copy)]
pub struct ImportCancellation<'a> {
    /// Observed by every worker transfer.
    pub transport: &'a CancellationToken,
    /// Observed by streaming, staging, and publication.
    pub staging: &'a ohl_payload::CancellationToken,
}

impl ImportCancellation<'_> {
    fn stop_requested(&self) -> bool {
        self.transport.is_cancelled() || self.staging.stop_requested()
    }
}

/// The bounded envelope one import runs under.
#[derive(Debug, Clone, Copy)]
pub struct ImportConfig {
    /// The worker session's read, protocol, and enumeration quotas.
    pub session: SessionConfig,
    /// The container search's ceilings.
    pub locate: LocateLimits,
    /// The destination plan's ceilings.
    pub payload: PayloadImportLimits,
    /// How long the worker has to launch and attest readiness.
    pub startup_timeout: Duration,
    /// How long any one protocol operation has.
    pub operation_timeout: Duration,
    /// How long an orderly shutdown or an escalation has.
    pub shutdown_timeout: Duration,
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            session: SessionConfig::default(),
            locate: LocateLimits::default(),
            payload: PayloadImportLimits::default(),
            startup_timeout: Duration::from_secs(10),
            operation_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

/// Whether this run published the payload or found it already published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportOutcome {
    /// This run staged and published the payload.
    Published,
    /// The exact payload was already published; nothing was written.
    AlreadyPublished,
}

/// The sanitized, in-memory outcome of one import.
///
/// The counts describe the caller's own run. They are media-derived and must
/// not be logged; see the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// How the run ended.
    pub outcome: ImportOutcome,
    /// The published payload's staging identity.
    pub payload_identity: String,
    /// Entries the recipe selected and layout accepted.
    pub entries_planned: u64,
    /// Entries that streamed to their exact declared size.
    pub entries_imported: u64,
    /// The planned total in bytes.
    pub bytes_planned: u64,
    /// Bytes the store accepted in full.
    pub bytes_imported: u64,
}

/// Why an import did not publish a payload.
///
/// Every variant is a fixed, payload-free code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum ImportError {
    /// The medium holds no container this build recognises.
    #[error("no supported payload container was found in the media")]
    NoContainer,
    /// The mounted medium could not be read.
    #[error("the mounted media could not be read")]
    Media,
    /// No confined parser worker could be launched on this build.
    #[error("no parser worker could be launched")]
    WorkerUnavailable,
    /// The worker refused the operation. The shipped worker's dispatcher
    /// answers `unsupported` for every enumeration and stream, emits no
    /// frame, and exits, which the parent observes as the peer closing.
    #[error("the parser worker does not support this container")]
    Unsupported,
    /// The worker, the transport, or the protocol failed.
    #[error("the parser worker session failed")]
    Worker,
    /// Session identities are exhausted.
    #[error("parser session identities are exhausted")]
    SessionsExhausted,
    /// The recipe did not produce a usable selection.
    #[error("the selection recipe produced no usable plan")]
    Selection,
    /// The selected entries are not a valid destination layout.
    #[error("the selected entries are not a valid payload layout")]
    Layout,
    /// The payload store could not be opened or written.
    #[error("the payload store could not be used")]
    Store,
    /// Staging refused, failed, or found a conflicting published payload.
    #[error("payload staging did not publish")]
    Staging,
    /// The provenance record could not be written.
    #[error("the payload provenance record could not be written")]
    Provenance,
    /// A stop was requested.
    #[error("the import was cancelled")]
    Cancelled,
}

/// Picks the one container an import will use.
///
/// The policy is deterministic and total:
///
/// 1. an InstallShield 3 Z archive wins over any cabinet, because on media
///    that carry both, the cabinet is a secondary payload of the same
///    installer;
/// 2. among candidates of the winning kind, the largest wins, because a
///    self-extracting stub's small trailing archive is never the payload;
/// 3. ties are broken by the candidates' deterministic order — kind, then
///    normalized path, then offset — so the same medium always yields the
///    same choice.
#[must_use]
pub fn choose_primary(candidates: &[ContainerCandidate]) -> Option<&ContainerCandidate> {
    let preferred = if candidates
        .iter()
        .any(|candidate| candidate.kind == ContainerKind::InstallShieldZ)
    {
        ContainerKind::InstallShieldZ
    } else {
        ContainerKind::MicrosoftCabinet
    };
    candidates
        .iter()
        .filter(|candidate| candidate.kind == preferred)
        // `max_by_key` keeps the last maximum, so the fold below is written
        // to keep the first one in the candidates' deterministic order.
        .fold(
            None,
            |best: Option<&ContainerCandidate>, candidate| match best {
                Some(best) if best.length >= candidate.length => Some(best),
                _ => Some(candidate),
            },
        )
}

/// The path of one medium's payload provenance record.
#[must_use]
pub fn payload_record_path(layout: &CacheLayout, media: &ValidatedMedia) -> PathBuf {
    layout
        .entry_directory(media.digest())
        .join(PAYLOAD_RECORD_FILE_NAME)
}

/// Reads the payload identity already recorded for `media`, if any.
///
/// A missing, oversized, or unreadable record simply means "not recorded":
/// the record is a cache, and the payload store remains the authority.
#[must_use]
pub fn recorded_payload_identity(layout: &CacheLayout, media: &ValidatedMedia) -> Option<String> {
    let path = payload_record_path(layout, media);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAXIMUM_PAYLOAD_RECORD_BYTES {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    let identity = text.trim();
    (!identity.is_empty()).then(|| identity.to_owned())
}

/// Records `identity` next to the medium's provenance manifest.
fn record_payload_identity(
    layout: &CacheLayout,
    media: &ValidatedMedia,
    identity: &str,
) -> Result<(), ImportError> {
    let path = payload_record_path(layout, media);
    let Some(parent) = path.parent() else {
        return Err(ImportError::Provenance);
    };
    fs::create_dir_all(parent).map_err(|_| ImportError::Provenance)?;
    fs::write(&path, format!("{identity}\n")).map_err(|_| ImportError::Provenance)
}

/// Bridges the session's chunk sink onto the staging sink.
///
/// The chunk sequence is the worker's; the accounting is ours. Every accepted
/// byte advances the fraction reported to the caller.
struct StagingBridge<'a> {
    destination: &'a mut dyn PayloadByteSink,
    progress: &'a mut dyn ProgressSink,
    total_bytes: u64,
    staged_bytes: &'a mut u64,
}

impl ByteSink for StagingBridge<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkRejected> {
        if !self.destination.write_chunk(bytes) {
            return Err(SinkRejected);
        }
        *self.staged_bytes = self
            .staged_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.total_bytes != 0 {
            #[allow(clippy::cast_precision_loss)]
            let fraction = (*self.staged_bytes as f64 / self.total_bytes as f64).min(1.0);
            #[allow(clippy::cast_possible_truncation)]
            self.progress.report(fraction as f32);
        }
        Ok(())
    }
}

/// The session between two streams.
enum SessionSlot<T: ExactIo> {
    /// Ready for the next `stream_entry`.
    Idle(Box<ParserSession<Idle, T, SourceWindow>>),
    /// A cancellation was acknowledged; only shutdown remains.
    Cancelled(Box<ParserSession<Cancelled, T, SourceWindow>>),
    /// The session failed terminally.
    Terminal(Box<TerminalSession<T, SourceWindow>>),
    /// Moved out for one transition.
    Taken,
}

/// Streams one planned entry per call by driving the parser session.
struct WorkerPayloadSource<'a, T: ExactIo> {
    slot: SessionSlot<T>,
    buffers: SessionBuffers,
    generation: CatalogGeneration,
    cancellation: ImportCancellation<'a>,
    operation_timeout: Duration,
    progress: &'a mut dyn ProgressSink,
    total_bytes: u64,
    staged_bytes: u64,
    entries_streamed: u64,
    /// The first sanitized reason a stream stopped, which outlives the
    /// staging report's generic `StreamFailure`.
    failure: Option<ImportError>,
}

impl<T: ExactIo> WorkerPayloadSource<'_, T> {
    fn deadline(&self) -> Instant {
        Instant::now()
            .checked_add(self.operation_timeout)
            .unwrap_or_else(Instant::now)
    }

    fn fail(&mut self, error: ImportError) -> bool {
        self.failure.get_or_insert(error);
        false
    }

    /// Drains an acknowledged cancellation, leaving the session shutdownable.
    fn drain_cancellation(
        &mut self,
        mut session: ParserSession<crate::parent_session::Cancelling, T, SourceWindow>,
        destination: &mut dyn PayloadByteSink,
    ) -> bool {
        loop {
            let deadline = self.deadline();
            let mut staged = self.staged_bytes;
            let mut bridge = StagingBridge {
                destination,
                progress: self.progress,
                total_bytes: self.total_bytes,
                staged_bytes: &mut staged,
            };
            let step = session.receive_one(
                &mut self.buffers,
                Some(&mut bridge),
                true,
                deadline,
                self.cancellation.transport,
            );
            self.staged_bytes = staged;
            match step {
                Ok(CancelStep::Acknowledged(cancelled)) => {
                    self.slot = SessionSlot::Cancelled(Box::new(cancelled));
                    return self.fail(ImportError::Cancelled);
                }
                Ok(CancelStep::Complete(idle)) => {
                    // Completion won the race; the entry is still whole, but
                    // the user asked to stop, so the import stops here.
                    self.slot = SessionSlot::Idle(Box::new(idle));
                    return self.fail(ImportError::Cancelled);
                }
                Ok(
                    CancelStep::Progress(next)
                    | CancelStep::ReadIgnored(next)
                    | CancelStep::ReadReplied(next),
                ) => session = next,
                Err(terminal) => {
                    self.slot = SessionSlot::Terminal(Box::new(terminal));
                    // The drain only runs after a stop was requested.
                    return self.fail(ImportError::Cancelled);
                }
            }
        }
    }
}

impl<T: ExactIo> PayloadSource for WorkerPayloadSource<'_, T> {
    fn stream(
        &mut self,
        _media_source: &MediaSource,
        source_token: u64,
        _cancellation: &ohl_payload::CancellationToken,
        destination: &mut dyn PayloadByteSink,
    ) -> bool {
        let SessionSlot::Idle(session) = core::mem::replace(&mut self.slot, SessionSlot::Taken)
        else {
            return self.fail(ImportError::Worker);
        };
        if self.cancellation.stop_requested() {
            self.slot = SessionSlot::Idle(session);
            return self.fail(ImportError::Cancelled);
        }

        let deadline = self.deadline();
        let mut streaming = match session.begin_stream(
            self.generation,
            SourceToken(source_token),
            deadline,
            self.cancellation.transport,
        ) {
            Ok(streaming) => streaming,
            Err(terminal) => {
                self.slot = SessionSlot::Terminal(Box::new(terminal));
                return self.fail(classify_session_failure(terminal_error(&self.slot)));
            }
        };

        loop {
            if self.cancellation.stop_requested() {
                let deadline = self.deadline();
                return match streaming.request_cancel(deadline, self.cancellation.transport) {
                    Ok(cancelling) => self.drain_cancellation(cancelling, destination),
                    Err(terminal) => {
                        self.slot = SessionSlot::Terminal(Box::new(terminal));
                        self.fail(ImportError::Cancelled)
                    }
                };
            }

            let deadline = self.deadline();
            let mut staged = self.staged_bytes;
            let mut bridge = StagingBridge {
                destination,
                progress: self.progress,
                total_bytes: self.total_bytes,
                staged_bytes: &mut staged,
            };
            let step = streaming.receive_one(
                &mut self.buffers,
                Some(&mut bridge),
                deadline,
                self.cancellation.transport,
            );
            self.staged_bytes = staged;
            match step {
                Ok(RequestStep::Progress(next) | RequestStep::ReadReplied(next)) => {
                    streaming = next;
                }
                Ok(RequestStep::Complete(idle)) => {
                    self.slot = SessionSlot::Idle(idle.into());
                    self.entries_streamed += 1;
                    self.buffers.scrub_reply();
                    return true;
                }
                Err(terminal) => {
                    self.slot = SessionSlot::Terminal(Box::new(terminal));
                    // A stop already requested explains the failure: the
                    // staging sink refuses chunks once it observes one.
                    let error = if self.cancellation.stop_requested() {
                        ImportError::Cancelled
                    } else {
                        classify_session_failure(terminal_error(&self.slot))
                    };
                    return self.fail(error);
                }
            }
        }
    }
}

/// The terminal session's recorded cause, if the slot holds one.
fn terminal_error<T: ExactIo>(slot: &SessionSlot<T>) -> Option<SessionError> {
    match slot {
        SessionSlot::Terminal(terminal) => Some(terminal.error()),
        _ => None,
    }
}

/// Maps a retired session onto the import vocabulary.
///
/// A peer that closed the channel without answering is exactly what this
/// build's `unsupported` dispatcher does: it emits no frame for the request
/// it refuses and exits. Every other failure is one generic worker failure.
fn classify_session_failure(error: Option<SessionError>) -> ImportError {
    match error {
        Some(SessionError::Channel(channel)) => match channel {
            crate::frame_channel::ChannelError::Transport(crate::io::IoError::PeerClosed) => {
                ImportError::Unsupported
            }
            crate::frame_channel::ChannelError::Transport(crate::io::IoError::Cancelled) => {
                ImportError::Cancelled
            }
            _ => ImportError::Worker,
        },
        _ => ImportError::Worker,
    }
}

/// Runs the whole import against a freshly launched confined worker.
///
/// # Errors
/// One fixed [`ImportError`]. Nothing partial is published on any path.
pub fn run_import(
    media: &ValidatedMedia,
    mount: &Mount,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    let config = ImportConfig::default();
    let mut allocator = SessionIdAllocator::new();
    let allocation = allocator
        .allocate()
        .map_err(|_| ImportError::SessionsExhausted)?;
    run_import_inner(
        media,
        mount,
        recipe,
        payload_root,
        cache_layout,
        &config,
        // Launched only once a container has actually been located, so a
        // medium this build recognises nothing in never starts a process.
        || {
            let startup = Instant::now()
                .checked_add(config.startup_timeout)
                .unwrap_or_else(Instant::now);
            ParserWorkerProcess::launch(startup).map_err(|_| ImportError::WorkerUnavailable)
        },
        allocation,
        cancellation,
        progress,
    )
}

/// [`run_import`] against a caller-supplied worker and identity.
///
/// This is the seam the crate's own tests drive with
/// [`FakeWorker`](crate::testing::FakeWorker). It grants no extra authority:
/// the worker is still only reachable through the sealed
/// [`WorkerProcess`] trait, which no code outside this crate can implement.
///
/// # Errors
/// One fixed [`ImportError`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn run_import_with_worker<W: WorkerProcess>(
    media: &ValidatedMedia,
    mount: &Mount,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    config: &ImportConfig,
    worker: W,
    allocation: SessionAllocation,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    run_import_inner(
        media,
        mount,
        recipe,
        payload_root,
        cache_layout,
        config,
        || Ok(worker),
        allocation,
        cancellation,
        progress,
    )
}

/// The shared body: the worker is created only after a container is located.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_import_inner<W: WorkerProcess, F: FnOnce() -> Result<W, ImportError>>(
    media: &ValidatedMedia,
    mount: &Mount,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    config: &ImportConfig,
    launch: F,
    allocation: SessionAllocation,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    if cancellation.stop_requested() {
        return Err(ImportError::Cancelled);
    }

    // 1. Find the one container this import will read.
    let candidates = locate_containers(mount, &config.locate, cancellation.transport)
        .map_err(|_| ImportError::Media)?;
    let primary = choose_primary(&candidates).ok_or(ImportError::NoContainer)?;
    let file = mount
        .open_file(primary.archive_path.as_str())
        .map_err(|_| ImportError::Media)?;
    let window =
        SourceWindow::new(file, primary.offset, primary.length).map_err(|_| ImportError::Media)?;

    // 2. Own the worker for exactly one session.
    let mut process = ProcessSession::new(launch()?, allocation);
    let mut open_buffer = crate::frame_channel::FrameBuffer::new();
    let deadline = |timeout: Duration| {
        Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now)
    };
    let session = process
        .open_with_ops(
            media,
            config.session,
            &mut open_buffer,
            deadline(config.operation_timeout),
            cancellation.transport,
            window,
        )
        .map_err(|failure| match failure.error() {
            OpenError::Handshake(_) => ImportError::Unsupported,
            _ => ImportError::Worker,
        })?;

    // 3. Enumerate. The catalog is promoted only after complete validation.
    let mut buffers = SessionBuffers::new(config.session.source_read_limits);
    let mut enumerating = match session
        .begin_enumeration(deadline(config.operation_timeout), cancellation.transport)
    {
        Ok(enumerating) => enumerating,
        Err(terminal) => {
            let error = classify_session_failure(Some(terminal.error()));
            let _ = process.abandon(terminal, deadline(config.shutdown_timeout));
            return Err(error);
        }
    };
    let idle = loop {
        let step = enumerating.receive_one(
            &mut buffers,
            None,
            deadline(config.operation_timeout),
            cancellation.transport,
        );
        match step {
            Ok(RequestStep::Progress(next) | RequestStep::ReadReplied(next)) => enumerating = next,
            Ok(RequestStep::Complete(idle)) => break idle,
            Err(terminal) => {
                let error = classify_session_failure(Some(terminal.error()));
                let _ = process.abandon(terminal, deadline(config.shutdown_timeout));
                return Err(error);
            }
        }
    };

    // 4. Select, then plan. Selection precedes layout: layout must never see
    //    an entry the user did not select.
    let catalog = idle.catalog().ok_or(ImportError::Worker)?;
    let generation = catalog.generation();
    let selectable: Vec<SelectableEntry> = catalog
        .entries()
        .iter()
        .map(|entry| {
            let archive_path = entry.relative_path().as_str().trim_start_matches('/');
            SelectableEntry {
                source_token: entry.source_token().0,
                // The recipe addresses components; the worker's catalog is
                // flat, so the leading path element is the component name.
                component: archive_path
                    .split_once('/')
                    .map_or(archive_path, |(head, _)| head)
                    .to_owned(),
                archive_path: archive_path.to_owned(),
                size_bytes: entry.size_bytes(),
            }
        })
        .collect();

    let plan = select(&selectable, recipe).map_err(|_| ImportError::Selection)?;
    let recipe_identity = plan.recipe_identity();
    let layout = plan_payload_layout(plan.entries(), &config.payload).map_err(|_| {
        // A layout the planner refuses is an untrusted-metadata rejection,
        // not a bug in the recipe.
        ImportError::Layout
    })?;
    let planned: Vec<PlannedPayloadEntry> = layout.entries().to_vec();
    let bytes_planned = layout.total_bytes();
    let entries_planned = u64::try_from(planned.len()).unwrap_or(u64::MAX);

    // 5. Stream every planned entry into staging and publish once.
    // The store root is application-owned, not media-derived: creating it on
    // demand is the same courtesy the provenance cache already extends.
    fs::create_dir_all(payload_root).map_err(|_| ImportError::Store)?;
    let mut store = DirectoryPayloadStore::open(payload_root).map_err(|_| ImportError::Store)?;
    let mut source = WorkerPayloadSource {
        slot: SessionSlot::Idle(Box::new(idle)),
        buffers,
        generation,
        cancellation,
        operation_timeout: config.operation_timeout,
        progress,
        total_bytes: bytes_planned,
        staged_bytes: 0,
        entries_streamed: 0,
        failure: None,
    };
    let request = PayloadStageRequest {
        recipe_identity: &recipe_identity,
        entries: &planned,
        declared_total_bytes: bytes_planned,
        limits: config.payload,
    };
    let stage_report = stage_payload(
        media.source(),
        &media.source_fingerprint(),
        &request,
        &mut source,
        &mut store,
        cancellation.staging,
    );

    // 6. End the worker's life exactly once, whatever staging decided.
    let stream_failure = source.failure;
    let entries_imported = source.entries_streamed;
    let slot = core::mem::replace(&mut source.slot, SessionSlot::Taken);
    drop(source);
    let shutdown_deadline = deadline(config.shutdown_timeout);
    let published = matches!(
        stage_report.status,
        PayloadStageStatus::CacheHit
            | PayloadStageStatus::PublishedSyncComplete
            | PayloadStageStatus::PublishedSyncUncertain
    );
    match (published, slot) {
        // Only a run that got what it came for is worth an orderly protocol
        // shutdown; every failure path terminates instead, exactly once.
        (true, SessionSlot::Idle(session)) => {
            let _ = process.orderly_shutdown(*session, shutdown_deadline, cancellation.transport);
        }
        (true, SessionSlot::Cancelled(session)) => {
            let _ = process.orderly_shutdown(*session, shutdown_deadline, cancellation.transport);
        }
        (_, other) => {
            drop(other);
            let _ = process.terminate(shutdown_deadline);
        }
    }
    drop(process);

    // 7. Report, and record the published identity.
    let outcome = match stage_report.status {
        PayloadStageStatus::CacheHit => ImportOutcome::AlreadyPublished,
        PayloadStageStatus::PublishedSyncComplete | PayloadStageStatus::PublishedSyncUncertain => {
            ImportOutcome::Published
        }
        _ => {
            return Err(stream_failure.unwrap_or(match stage_report.error {
                Some(PayloadStageError::Cancelled) => ImportError::Cancelled,
                _ => ImportError::Staging,
            }));
        }
    };
    let payload_identity = stage_report.identity.ok_or(ImportError::Staging)?;
    record_payload_identity(cache_layout, media, &payload_identity)?;

    Ok(ImportReport {
        outcome,
        payload_identity,
        entries_planned,
        entries_imported,
        bytes_planned,
        bytes_imported: stage_report.bytes_streamed,
    })
}

#[cfg(test)]
mod tests {
    use super::{ContainerCandidate, ContainerKind, choose_primary};
    use crate::catalog::NormalizedPath;

    fn candidate(path: &str, kind: ContainerKind, length: u64) -> ContainerCandidate {
        ContainerCandidate {
            archive_path: NormalizedPath::from_normalized(path.to_owned()),
            kind,
            offset: 0,
            length,
        }
    }

    #[test]
    fn a_z_archive_wins_over_a_larger_cabinet() {
        let candidates = vec![
            candidate("/a", ContainerKind::InstallShieldZ, 10),
            candidate("/b", ContainerKind::MicrosoftCabinet, 1_000),
        ];
        assert_eq!(
            choose_primary(&candidates).map(|found| found.kind),
            Some(ContainerKind::InstallShieldZ)
        );
    }

    #[test]
    fn the_largest_cabinet_wins_when_there_is_no_z_archive() {
        let candidates = vec![
            candidate("/a", ContainerKind::MicrosoftCabinet, 10),
            candidate("/b", ContainerKind::MicrosoftCabinet, 1_000),
            candidate("/c", ContainerKind::MicrosoftCabinet, 999),
        ];
        assert_eq!(
            choose_primary(&candidates).map(|found| found.length),
            Some(1_000)
        );
    }

    #[test]
    fn an_exact_size_tie_keeps_the_first_deterministic_candidate() {
        let candidates = vec![
            candidate("/a", ContainerKind::MicrosoftCabinet, 1_000),
            candidate("/b", ContainerKind::MicrosoftCabinet, 1_000),
        ];
        assert_eq!(
            choose_primary(&candidates).map(|found| found.archive_path.as_str().to_owned()),
            Some(String::from("/a"))
        );
    }

    #[test]
    fn no_candidate_is_no_choice() {
        assert!(choose_primary(&[]).is_none());
    }
}
