//! The end-to-end parent composition, driven by a scripted synthetic worker.
//!
//! The medium is a project-authored synthetic ISO holding one synthetic PE
//! whose overlay starts with the InstallShield 3 Z signature; the worker is
//! [`FakeWorker`], whose enumeration and chunk bytes are invented here. No
//! real medium, name, or byte is involved, and no process is launched.

mod support;

use std::sync::Arc;

use ohl_import::pipeline::{
    ImportCancellation, ImportConfig, ImportError, ImportOutcome, PAYLOAD_RECORD_FILE_NAME,
    ProgressSink, recorded_payload_identity, run_import_with_worker,
};
use ohl_import::process_session::SessionConfig;
use ohl_import::testing::{FakeWorker, SyntheticTransport};
use ohl_import::{CancellationToken, ImportReport};
use ohl_media::CacheLayout;
use ohl_parser_protocol::{MessageType, OperationPhase};
use ohl_payload::selection::SelectionRecipe;
use ohl_payload::{CancellationSource as StagingSource, CancellationToken as StagingToken};
use support::{
    BatchEntry, MountedFixture, TemporaryRoot, budgets, complete_payload, data_chunk_payload,
    entry_batch_payload, import_limits, read_limits, synthetic_bytes, synthetic_pe_with_z_overlay,
};

/// Two invented entries: nothing here names a real component or file.
const FIRST_PATH: &str = "invented/alpha.dat";
const SECOND_PATH: &str = "invented/beta.dat";
const FIRST_BYTES: usize = 3_000;
const SECOND_BYTES: usize = 1_500;

fn config() -> ImportConfig {
    ImportConfig {
        session: SessionConfig {
            source_read_limits: read_limits(),
            protocol_budgets: budgets(),
            import_limits: import_limits(),
        },
        ..ImportConfig::default()
    }
}

fn recipe() -> SelectionRecipe {
    SelectionRecipe::parse("version = 1\ndefault_decision = \"include\"\n").expect("fixture recipe")
}

/// The bytes the fixture worker streams for each entry.
fn entry_bytes() -> (Vec<u8>, Vec<u8>) {
    (synthetic_bytes(FIRST_BYTES), synthetic_bytes(SECOND_BYTES))
}

/// Queues a complete, well-behaved enumeration for request id 1.
fn push_enumeration(transport: &SyntheticTransport, session: u64) {
    let batch = entry_batch_payload(
        &[
            BatchEntry {
                source_token: 1,
                archive_path: FIRST_PATH,
                size_bytes: FIRST_BYTES as u64,
            },
            BatchEntry {
                source_token: 2,
                archive_path: SECOND_PATH,
                size_bytes: SECOND_BYTES as u64,
            },
        ],
        import_limits(),
    );
    transport.push_frame(
        &ohl_parser_protocol::FrameHeader::new(
            MessageType::EntryBatch,
            session,
            1,
            u32::try_from(batch.len()).expect("bounded fixture batch"),
        ),
        &batch,
    );
    let complete = complete_payload(OperationPhase::Enumerate);
    transport.push_frame(
        &ohl_parser_protocol::FrameHeader::new(
            MessageType::Complete,
            session,
            1,
            u32::try_from(complete.len()).expect("bounded fixture completion"),
        ),
        &complete,
    );
}

/// Queues one entry's chunks and its completion for `request_id`.
fn push_stream(transport: &SyntheticTransport, session: u64, request_id: u64, data: &[u8]) {
    let mut remaining = data.len() as u64;
    for chunk in data.chunks(1_024) {
        let payload = data_chunk_payload(chunk, remaining);
        transport.push_frame(
            &ohl_parser_protocol::FrameHeader::new(
                MessageType::DataChunk,
                session,
                request_id,
                u32::try_from(payload.len()).expect("bounded fixture chunk"),
            ),
            &payload,
        );
        remaining -= chunk.len() as u64;
    }
    let complete = complete_payload(OperationPhase::Stream);
    transport.push_frame(
        &ohl_parser_protocol::FrameHeader::new(
            MessageType::Complete,
            session,
            request_id,
            u32::try_from(complete.len()).expect("bounded fixture completion"),
        ),
        &complete,
    );
}

/// A progress sink that records every fraction it is handed.
#[derive(Debug, Default)]
struct RecordingProgress {
    fractions: Vec<f32>,
}

impl ProgressSink for RecordingProgress {
    fn report(&mut self, fraction: f32) {
        self.fractions.push(fraction);
    }
}

/// A progress sink that asks the import to stop after the first chunk.
struct CancellingProgress {
    source: Arc<StagingSource>,
    seen: usize,
}

impl ProgressSink for CancellingProgress {
    fn report(&mut self, _fraction: f32) {
        self.seen += 1;
        if self.seen == 1 {
            self.source.request_stop();
        }
    }
}

/// Everything one pipeline run needs.
struct Harness {
    fixture: Arc<MountedFixture>,
    roots: Arc<TemporaryRoot>,
    worker: FakeWorker,
    transport: Arc<SyntheticTransport>,
    session: u64,
    allocation: ohl_import::SessionAllocation,
}

impl Harness {
    fn new() -> Self {
        Self::over(
            Arc::new(MountedFixture::new(
                "SETUP.EXE",
                synthetic_pe_with_z_overlay(128 * 1024),
            )),
            Arc::new(TemporaryRoot::new()),
        )
    }

    /// A second worker over the same medium, payload store, and cache.
    fn again(&self) -> Self {
        Self::over(Arc::clone(&self.fixture), Arc::clone(&self.roots))
    }

    fn over(fixture: Arc<MountedFixture>, roots: Arc<TemporaryRoot>) -> Self {
        let transport = Arc::new(SyntheticTransport::new());
        let worker = FakeWorker::new(Arc::clone(&transport));
        let allocation = ohl_import::SessionIdAllocator::new()
            .allocate()
            .expect("a fresh identity");
        let session = allocation.session_id.get();
        // The handshake's `ready` always comes first.
        transport.push_frame(
            &ohl_parser_protocol::FrameHeader::new(MessageType::Ready, session, 0, 0),
            &[],
        );
        Self {
            fixture,
            roots,
            worker,
            transport,
            session,
            allocation,
        }
    }

    fn payload_root(&self) -> std::path::PathBuf {
        self.roots.path().join("payload")
    }

    fn cache_layout(&self) -> CacheLayout {
        CacheLayout::with_root(self.roots.path().join("cache")).expect("fixture cache layout")
    }

    fn run(
        &self,
        cancellation: ImportCancellation<'_>,
        progress: &mut dyn ProgressSink,
    ) -> Result<ImportReport, ImportError> {
        run_import_with_worker(
            self.fixture.media(),
            self.fixture.mount(),
            &recipe(),
            &self.payload_root(),
            &self.cache_layout(),
            &config(),
            self.worker.clone(),
            self.allocation,
            cancellation,
            progress,
        )
    }
}

fn tokens() -> (CancellationToken, StagingToken) {
    (CancellationToken::default(), StagingToken::default())
}

#[test]
fn a_complete_run_publishes_the_planned_tree_and_records_its_identity() {
    let harness = Harness::new();
    let (first, second) = entry_bytes();
    push_enumeration(&harness.transport, harness.session);
    // The layout's deterministic order is by portability key, so `alpha`
    // streams first as request 2 and `beta` second as request 3.
    push_stream(&harness.transport, harness.session, 2, &first);
    push_stream(&harness.transport, harness.session, 3, &second);

    let (transport_token, staging_token) = tokens();
    let mut progress = RecordingProgress::default();
    let report = harness
        .run(
            ImportCancellation {
                transport: &transport_token,
                staging: &staging_token,
            },
            &mut progress,
        )
        .expect("a complete run publishes");

    assert_eq!(report.outcome, ImportOutcome::Published);
    assert_eq!(report.entries_planned, 2);
    assert_eq!(report.entries_imported, 2);
    assert_eq!(report.bytes_planned, (FIRST_BYTES + SECOND_BYTES) as u64);
    assert_eq!(report.bytes_imported, report.bytes_planned);
    assert!(
        report
            .payload_identity
            .starts_with("ohl-payload-v2-sha256:"),
        "{}",
        report.payload_identity
    );

    // The published tree holds exactly the streamed bytes.
    let published = harness.payload_root().join(
        std::fs::read_dir(harness.payload_root())
            .expect("payload root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .find(|name| name.to_string_lossy().starts_with("ohl-tree-"))
            .expect("one published tree"),
    );
    assert_eq!(
        std::fs::read(published.join("files").join("invented").join("alpha.dat"))
            .expect("first entry"),
        first
    );
    assert_eq!(
        std::fs::read(published.join("files").join("invented").join("beta.dat"))
            .expect("second entry"),
        second
    );

    // The provenance record names the tree, and nothing else.
    let layout = harness.cache_layout();
    let record = layout
        .entry_directory(harness.fixture.media().digest())
        .join(PAYLOAD_RECORD_FILE_NAME);
    let contents = std::fs::read_to_string(&record).expect("payload record");
    assert_eq!(contents.trim(), report.payload_identity);
    assert!(!contents.contains("alpha"), "{contents}");
    assert!(!contents.contains("SETUP"), "{contents}");
    assert_eq!(
        recorded_payload_identity(&layout, harness.fixture.media()).as_deref(),
        Some(report.payload_identity.as_str())
    );

    // Progress is a fraction only, and it never runs backwards or past one.
    assert!(!progress.fractions.is_empty());
    assert!(
        progress
            .fractions
            .windows(2)
            .all(|pair| pair[1] >= pair[0] && pair[1] <= 1.0)
    );
    assert!((progress.fractions.last().copied().unwrap_or(0.0) - 1.0).abs() < f32::EPSILON);

    // The worker was shut down in order, not killed.
    assert_eq!(harness.worker.terminate_calls(), 0);
}

#[test]
fn a_second_run_over_the_same_plan_reports_the_payload_as_already_published() {
    let first_run = Harness::new();
    let (first, second) = entry_bytes();
    push_enumeration(&first_run.transport, first_run.session);
    push_stream(&first_run.transport, first_run.session, 2, &first);
    push_stream(&first_run.transport, first_run.session, 3, &second);
    let (transport_token, staging_token) = tokens();
    let cancellation = ImportCancellation {
        transport: &transport_token,
        staging: &staging_token,
    };
    let published = first_run
        .run(cancellation, &mut ohl_import::DiscardProgress)
        .expect("a complete run publishes");

    // A fresh worker over the same medium, the same recipe, and the same
    // payload root: the probe finds the identical tree and nothing is
    // written or streamed.
    let second_run = first_run.again();
    push_enumeration(&second_run.transport, second_run.session);

    let report = second_run
        .run(cancellation, &mut ohl_import::DiscardProgress)
        .expect("a repeat run is a cache hit");
    assert_eq!(report.outcome, ImportOutcome::AlreadyPublished);
    assert_eq!(report.entries_imported, 0);
    assert_eq!(report.payload_identity, published.payload_identity);
    assert_eq!(second_run.worker.terminate_calls(), 0);
}

#[test]
fn a_cancellation_mid_stream_discards_the_stage_and_terminates_the_worker_once() {
    let harness = Harness::new();
    let (first, _) = entry_bytes();
    push_enumeration(&harness.transport, harness.session);
    push_stream(&harness.transport, harness.session, 2, &first);

    let staging = Arc::new(StagingSource::new());
    let staging_token = staging.token();
    let transport_token = CancellationToken::default();
    let mut progress = CancellingProgress {
        source: Arc::clone(&staging),
        seen: 0,
    };

    let error = harness
        .run(
            ImportCancellation {
                transport: &transport_token,
                staging: &staging_token,
            },
            &mut progress,
        )
        .expect_err("a cancelled run publishes nothing");
    assert_eq!(error, ImportError::Cancelled);

    // Nothing was published and no owned staging was left behind.
    let published: Vec<_> = std::fs::read_dir(harness.payload_root())
        .expect("payload root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        published.iter().all(|name| !name.starts_with("ohl-tree-")),
        "{published:?}"
    );
    assert!(recorded_payload_identity(&harness.cache_layout(), harness.fixture.media()).is_none());
    assert_eq!(harness.worker.terminate_calls(), 1);
}

#[test]
fn a_worker_that_closes_without_answering_the_enumeration_is_unsupported() {
    // The shipped worker's dispatcher refuses every enumeration, emits no
    // frame, and exits: the parent sees its peer close.
    let harness = Harness::new();
    let (transport_token, staging_token) = tokens();
    let error = harness
        .run(
            ImportCancellation {
                transport: &transport_token,
                staging: &staging_token,
            },
            &mut ohl_import::DiscardProgress,
        )
        .expect_err("an unanswered enumeration publishes nothing");
    assert_eq!(error, ImportError::Unsupported);
    assert_eq!(harness.worker.terminate_calls(), 1);
}

#[test]
fn a_transport_failure_during_enumeration_is_a_worker_failure() {
    let harness = Harness::new();
    harness
        .transport
        .script_reads([ohl_import::testing::IoStep::Transfer]);
    harness
        .transport
        .script_reads([ohl_import::testing::IoStep::Fail(
            ohl_import::io::IoError::IoFailure,
        )]);
    let (transport_token, staging_token) = tokens();
    let error = harness
        .run(
            ImportCancellation {
                transport: &transport_token,
                staging: &staging_token,
            },
            &mut ohl_import::DiscardProgress,
        )
        .expect_err("a failed transport publishes nothing");
    assert_eq!(error, ImportError::Worker);
    assert_eq!(harness.worker.terminate_calls(), 1);
}

#[test]
fn media_with_no_recognised_container_never_starts_a_worker() {
    let fixture = MountedFixture::new("PLAIN.DAT", synthetic_bytes(96 * 1024));
    let roots = TemporaryRoot::new();
    let transport = Arc::new(SyntheticTransport::new());
    let worker = FakeWorker::new(Arc::clone(&transport));
    let allocation = ohl_import::SessionIdAllocator::new()
        .allocate()
        .expect("identity");
    let (transport_token, staging_token) = tokens();

    let error = run_import_with_worker(
        fixture.media(),
        fixture.mount(),
        &recipe(),
        &roots.path().join("payload"),
        &CacheLayout::with_root(roots.path().join("cache")).expect("cache layout"),
        &config(),
        worker.clone(),
        allocation,
        ImportCancellation {
            transport: &transport_token,
            staging: &staging_token,
        },
        &mut ohl_import::DiscardProgress,
    )
    .expect_err("no container is not an import");
    assert_eq!(error, ImportError::NoContainer);
    assert_eq!(transport.call_counts(), (0, 0, 0));
    assert!(worker.calls().is_empty());
}
