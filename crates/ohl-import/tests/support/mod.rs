//! Shared synthetic fixtures for the import-session tests.
//!
//! Every byte here is project-authored filler produced by a fixed arithmetic
//! rule. Nothing in this directory is derived from real media, and no test in
//! this crate reads any.

#![allow(dead_code, reason = "each integration test binary uses a subset")]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ohl_import::io::CancellationToken;
use ohl_import::testing::SyntheticTransport;
use ohl_import::{
    ByteSink, FrameBuffer, FrameChannel, Idle, ImportLimits, ParserSession, ResultSession,
    SessionAllocation, SessionIdAllocator, SinkRejected, SourceReadLimits, WorkerEpoch,
    create_parser_session, perform_parent_handshake,
};
use ohl_media::{MediaClass, MediaDescription, ValidatedMedia, VolumeLabel};
use ohl_parser_protocol::messages::{
    COMPLETE_PAYLOAD_BYTES, ENTRY_BATCH_ENTRY_PREFIX_BYTES, ENTRY_BATCH_PREFIX_BYTES,
    READ_REQUEST_PAYLOAD_BYTES, encode_complete_payload, encode_data_chunk_payload,
    encode_entry_batch_payload, encode_read_request_payload,
};
use ohl_parser_protocol::{
    ArchiveSpelling, Complete, DataChunk, Direction, EntryBatch, EntryBatchEntry, EntryBatchPolicy,
    FrameHeader, FrameView, MessageType, OperationPhase, ProtocolBudgets, ProtocolPhase,
    ProtocolStatus, ReadRequest, SessionId, SessionState, SessionValidator, SourceReadPolicy,
};
use ohl_platform::MediaSource;

/// The session id every fixture frame carries.
pub const SESSION: u64 = 0x1020_3040_5060_7080;

/// The read quota the fixtures advertise: small, so buffers stay cheap.
pub const READ_BYTES: u32 = 4_096;

/// Deterministic project-authored filler bytes.
#[must_use]
pub fn synthetic_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from((index * 31 + 7) % 251).unwrap_or(0))
        .collect()
}

/// A generous deadline for tests that must not time out.
#[must_use]
pub fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// A deadline that has already passed.
#[must_use]
pub fn expired_deadline() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now)
}

/// A token that is never signalled.
#[must_use]
pub fn no_cancellation() -> CancellationToken {
    CancellationToken::default()
}

/// The fixture session id.
#[must_use]
pub fn session_id() -> SessionId {
    SessionId::new(SESSION).expect("non-zero fixture session id")
}

/// The fixture read quotas.
#[must_use]
pub fn read_limits() -> SourceReadLimits {
    SourceReadLimits::new(READ_BYTES, 8, 1 << 20).expect("valid fixture read limits")
}

/// Bounded fixture import quotas.
#[must_use]
pub fn import_limits() -> ImportLimits {
    ImportLimits::new(16, 4_096, 1 << 20, 1 << 21).expect("valid fixture import limits")
}

/// Bounded fixture protocol budgets.
#[must_use]
pub fn budgets() -> ProtocolBudgets {
    ProtocolBudgets::new(1_024, 1 << 22).expect("valid fixture budgets")
}

/// The first allocation of a fresh allocator.
#[must_use]
pub fn allocation() -> SessionAllocation {
    SessionIdAllocator::new()
        .allocate()
        .expect("a fresh allocator issues an identity")
}

/// A temporary directory whose path has been fully resolved.
pub struct TemporaryRoot {
    directory: tempfile::TempDir,
    path: std::path::PathBuf,
}

impl TemporaryRoot {
    /// Creates and resolves a temporary directory.
    #[must_use]
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = std::fs::canonicalize(directory.path()).expect("resolved temporary directory");
        Self { directory, path }
    }

    /// The resolved root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        debug_assert!(self.directory.path().exists());
        &self.path
    }
}

impl Default for TemporaryRoot {
    fn default() -> Self {
        Self::new()
    }
}

/// A pinned synthetic source and the proof bound to it.
pub struct Fixture {
    root: TemporaryRoot,
    media: ValidatedMedia,
    content: Vec<u8>,
}

impl Fixture {
    /// Writes `length` synthetic bytes, pins them, and fingerprints them.
    #[must_use]
    pub fn new(length: usize) -> Self {
        let root = TemporaryRoot::new();
        let content = synthetic_bytes(length);
        let path = root.path().join("synthetic.img");
        std::fs::write(&path, &content).expect("synthetic fixture");
        let source = Arc::new(MediaSource::open(&path).expect("pinned source"));
        let media = ValidatedMedia::fingerprinting(
            source,
            MediaDescription::new(MediaClass::Udf, "udf", VolumeLabel::sanitized("SYNTHETIC")),
        )
        .expect("stable synthetic source");
        Self {
            root,
            media,
            content,
        }
    }

    /// The validated-media proof.
    #[must_use]
    pub const fn media(&self) -> &ValidatedMedia {
        &self.media
    }

    /// The exact bytes written to the pinned source.
    #[must_use]
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// The pinned size.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.media.size_bytes()
    }

    /// The read policy the fixture limits imply.
    #[must_use]
    pub fn policy(&self) -> SourceReadPolicy {
        SourceReadPolicy::new(self.size(), READ_BYTES).expect("valid fixture policy")
    }

    /// The resolved temporary root, for tests that mutate the pinned file.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.path()
    }
}

/// A channel over a fresh synthetic transport.
#[must_use]
pub fn new_channel() -> (
    Arc<SyntheticTransport>,
    FrameChannel<Arc<SyntheticTransport>>,
) {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = FrameChannel::new(session_id(), Arc::clone(&transport));
    (transport, channel)
}

/// A header for the fixture session.
#[must_use]
pub fn header(message_type: MessageType, request_id: u64, payload_length: u32) -> FrameHeader {
    FrameHeader::new(message_type, SESSION, request_id, payload_length)
}

/// The `ready` frame a well-behaved worker answers `hello` with.
#[must_use]
pub fn ready_frame() -> FrameHeader {
    header(MessageType::Ready, 0, 0)
}

/// Encodes a `complete` payload for `phase`.
#[must_use]
pub fn complete_payload(phase: OperationPhase) -> Vec<u8> {
    let mut payload = vec![0_u8; COMPLETE_PAYLOAD_BYTES];
    let written = encode_complete_payload(
        &Complete {
            status: ProtocolStatus::Ok,
            phase: ProtocolPhase::Complete,
        },
        phase,
        &mut payload,
    )
    .expect("encodable complete payload");
    payload.truncate(written);
    payload
}

/// One `(source_token, path, size)` triple for an entry batch.
pub struct BatchEntry<'a> {
    /// The worker-assigned token.
    pub source_token: u64,
    /// The untrusted archive spelling.
    pub archive_path: &'a str,
    /// The declared size.
    pub size_bytes: u64,
}

/// Encodes an `entry_batch` payload under a permissive policy.
#[must_use]
pub fn entry_batch_payload(entries: &[BatchEntry<'_>], limits: ImportLimits) -> Vec<u8> {
    let decoded: Vec<EntryBatchEntry<'_>> = entries
        .iter()
        .map(|entry| EntryBatchEntry {
            source_token: entry.source_token,
            size_bytes: entry.size_bytes,
            archive_path: ArchiveSpelling::new(entry.archive_path.as_bytes())
                .expect("printable fixture spelling"),
        })
        .collect();
    let policy = EntryBatchPolicy::new(
        limits.maximum_entries(),
        limits.maximum_path_bytes(),
        limits.maximum_entry_bytes(),
        limits.maximum_total_bytes(),
        None,
    )
    .expect("valid fixture batch policy");
    let size = ENTRY_BATCH_PREFIX_BYTES
        + entries
            .iter()
            .map(|entry| ENTRY_BATCH_ENTRY_PREFIX_BYTES + entry.archive_path.len())
            .sum::<usize>();
    let mut payload = vec![0_u8; size];
    let written =
        encode_entry_batch_payload(&EntryBatch { entries: &decoded }, &policy, &mut payload)
            .expect("encodable fixture batch");
    payload.truncate(written);
    payload
}

/// Encodes a `data_chunk` payload.
#[must_use]
pub fn data_chunk_payload(data: &[u8], remaining: u64) -> Vec<u8> {
    let mut payload = vec![0_u8; data.len()];
    let written = encode_data_chunk_payload(&DataChunk { data }, remaining, &mut payload)
        .expect("encodable fixture chunk");
    payload.truncate(written);
    payload
}

/// Encodes a `read_request` payload.
#[must_use]
pub fn read_request_payload(
    sequence: u32,
    offset: u64,
    length: u32,
    policy: &SourceReadPolicy,
) -> Vec<u8> {
    let mut payload = vec![0_u8; READ_REQUEST_PAYLOAD_BYTES];
    let written = encode_read_request_payload(
        &ReadRequest {
            read_sequence: sequence,
            offset,
            length,
        },
        policy,
        sequence,
        &mut payload,
    )
    .expect("encodable fixture read request");
    payload.truncate(written);
    payload
}

/// The fixture worker epoch.
#[must_use]
pub fn worker_epoch() -> WorkerEpoch {
    WorkerEpoch::new(9).expect("non-zero fixture epoch")
}

/// A validator that has observed a complete typed handshake.
#[must_use]
pub fn idle_validator() -> SessionValidator {
    let mut protocol = SessionValidator::new(session_id(), budgets());
    let hello = header(MessageType::Hello, 0, 12);
    protocol
        .observe(Direction::ParentToWorker, &hello)
        .expect("hello is legal first");
    protocol
        .observe(Direction::WorkerToParent, &ready_frame())
        .expect("ready answers hello");
    assert_eq!(protocol.state(), SessionState::Idle);
    protocol
}

/// A result session over an idle validator and the fixture quotas.
#[must_use]
pub fn result_session() -> ResultSession {
    ResultSession::new(idle_validator(), worker_epoch(), import_limits())
        .expect("an idle validator opens a result session")
}

/// A sink that records every chunk, or refuses them all.
#[derive(Debug, Default)]
pub struct RecordingSink {
    accepted: Vec<u8>,
    refuse: bool,
}

impl RecordingSink {
    /// A sink that accepts everything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A sink that refuses every chunk.
    #[must_use]
    pub fn refusing() -> Self {
        Self {
            accepted: Vec::new(),
            refuse: true,
        }
    }

    /// Everything the sink accepted, in order.
    #[must_use]
    pub fn accepted(&self) -> &[u8] {
        &self.accepted
    }
}

impl ByteSink for RecordingSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkRejected> {
        if self.refuse {
            return Err(SinkRejected);
        }
        self.accepted.extend_from_slice(bytes);
        Ok(())
    }
}

/// Builds a frame view over a header and payload.
#[must_use]
pub fn frame<'payload>(header: &FrameHeader, payload: &'payload [u8]) -> FrameView<'payload> {
    FrameView::new(*header, payload)
}

/// Encodes an `entry_batch` payload **without** the encoder's policy checks,
/// so a test can present a hostile batch the worker could have sent.
#[must_use]
pub fn raw_entry_batch_payload(entries: &[BatchEntry<'_>]) -> Vec<u8> {
    let count = u16::try_from(entries.len()).expect("bounded fixture batch");
    let mut payload = Vec::new();
    payload.extend_from_slice(&count.to_le_bytes());
    for entry in entries {
        payload.extend_from_slice(&entry.source_token.to_le_bytes());
        payload.extend_from_slice(&entry.size_bytes.to_le_bytes());
        let length = u16::try_from(entry.archive_path.len()).expect("bounded fixture spelling");
        payload.extend_from_slice(&length.to_le_bytes());
        payload.extend_from_slice(entry.archive_path.as_bytes());
    }
    payload
}

/// The transport of a session opened over a synthetic worker.
pub type Transport = Arc<SyntheticTransport>;

/// One handshaken parent session and the pieces behind it.
pub struct OpenSession {
    /// The synthetic worker end of the channel.
    pub transport: Transport,
    /// The shared frame channel.
    pub channel: Arc<FrameChannel<Transport>>,
    /// The idle session the handshake produced.
    pub session: ParserSession<Idle, Transport>,
}

/// Performs the handshake over a synthetic transport and composes a session.
#[must_use]
pub fn open_session(fixture: &Fixture) -> OpenSession {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = Arc::new(FrameChannel::new(session_id(), Arc::clone(&transport)));
    transport.push_frame(&ready_frame(), &[]);
    let mut buffer = FrameBuffer::new();
    let proof = perform_parent_handshake(
        &channel,
        fixture.media(),
        read_limits(),
        budgets(),
        &mut buffer,
        deadline(),
        &no_cancellation(),
    )
    .expect("a canonical fixture handshake");
    let session = create_parser_session(
        proof,
        Arc::clone(&channel),
        fixture.media(),
        worker_epoch(),
        import_limits(),
    )
    .expect("a composed fixture session");
    transport.clear_written();
    OpenSession {
        transport,
        channel,
        session,
    }
}

// ---------------------------------------------------------------------------
// Synthetic PE/ISO fixtures
//
// Every byte below is project-authored. The PE layout follows Microsoft's
// public "PE Format" specification (see `docs/FORMAT_SOURCES.md`); the names
// are invented and correspond to nothing on any real medium.
// ---------------------------------------------------------------------------

#[allow(
    unused_imports,
    reason = "each integration test binary re-exports a subset"
)]
pub use ohl_import::testing::{
    PE_HEADER_OFFSET, PE_OPTIONAL_HEADER_BYTES, PE_OVERLAY_OFFSET, synthetic_pe,
    synthetic_pe_with_z_overlay,
};

/// Writes a single-file ISO 9660 image and pins it.
///
/// The image is produced by the `hadris-iso` writer from project-authored
/// bytes; nothing in it comes from any real medium.
#[must_use]
pub fn synthetic_iso(file_name: &str, contents: Vec<u8>) -> Vec<u8> {
    use hadris_iso::read::PathSeparator;
    use hadris_iso::write::options::{BaseIsoLevel, CreationFeatures, IsoFormatOptions};
    use hadris_iso::write::{File as IsoFile, InputFiles, IsoImageWriter};

    let capacity = contents.len() + 4 * 1024 * 1024;
    let files = InputFiles {
        path_separator: PathSeparator::ForwardSlash,
        files: vec![IsoFile::File {
            name: Arc::new(file_name.to_owned()),
            contents,
        }],
    };
    let options = IsoFormatOptions {
        volume_name: "SYNTHETIC".to_owned(),
        system_id: None,
        volume_set_id: None,
        publisher_id: None,
        preparer_id: None,
        application_id: None,
        sector_size: 2_048,
        path_separator: PathSeparator::ForwardSlash,
        features: CreationFeatures {
            filenames: BaseIsoLevel::Level1 {
                supports_lowercase: false,
                supports_rrip: false,
            },
            long_filenames: false,
            joliet: None,
            rock_ridge: None,
            el_torito: None,
            hybrid_boot: None,
        },
        strict_charset: false,
    };
    let mut buffer = std::io::Cursor::new(vec![0u8; capacity]);
    IsoImageWriter::create(&mut buffer, files, options).expect("synthetic iso image");
    buffer.into_inner()
}

/// A pinned, mounted synthetic ISO holding one container-bearing file.
pub struct MountedFixture {
    root: TemporaryRoot,
    media: ValidatedMedia,
    mount: ohl_vfs::Mount,
}

impl MountedFixture {
    /// Builds, writes, pins, fingerprints, and mounts a synthetic ISO whose
    /// only file is `contents` under `file_name`.
    #[must_use]
    pub fn new(file_name: &str, contents: Vec<u8>) -> Self {
        let root = TemporaryRoot::new();
        let image = synthetic_iso(file_name, contents);
        let path = root.path().join("synthetic.iso");
        std::fs::write(&path, &image).expect("synthetic iso fixture");
        let source = Arc::new(MediaSource::open(&path).expect("pinned iso source"));
        let media = ValidatedMedia::fingerprinting(
            Arc::clone(&source),
            MediaDescription::new(
                MediaClass::Iso9660,
                "iso9660",
                VolumeLabel::sanitized("SYNTHETIC"),
            ),
        )
        .expect("stable synthetic iso");
        let mount = ohl_vfs::Mount::open(source, ohl_vfs::DirectoryLimits::default())
            .expect("mounted synthetic iso");
        Self { root, media, mount }
    }

    /// The validated-media proof.
    #[must_use]
    pub const fn media(&self) -> &ValidatedMedia {
        &self.media
    }

    /// The read-only mount.
    #[must_use]
    pub const fn mount(&self) -> &ohl_vfs::Mount {
        &self.mount
    }

    /// The resolved temporary root.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.path()
    }
}
