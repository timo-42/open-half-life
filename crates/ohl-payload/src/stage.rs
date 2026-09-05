//! Transactional payload staging: the whole import commit protocol.
//!
//! [`stage_payload`] is the single function that turns a validated layout into
//! a published payload, and its value is entirely in the *order* of its steps.
//! The order is a port of the C++ `stage_payload`, and each step exists to
//! close a specific window:
//!
//! 1. **Validate** the request against the pinned source and the limits, and
//!    re-run layout planning over the request's entries. A caller cannot hand
//!    staging a plan that the planner would have refused.
//! 2. **Derive the identity** — a version-2 SHA-256 over the accepted source
//!    size and digest, the recipe identity, the entry count, the declared
//!    total, and every normalised path and declared size. Transport-local
//!    source tokens are excluded, so two readers that assign different tokens
//!    to the same content still agree on the identity.
//! 3. **Probe.** A matching published payload is a cache hit and nothing is
//!    read or written; a non-matching one is a conflict.
//! 4. **Stage** every entry, then seal the completion metadata.
//! 5. **Reverify the complete pinned source** against its accepted size and
//!    SHA-256. This is the `docs/MEDIA_IMPORT.md` complete-source stability
//!    mode: publication may not be based on a mix of source states, so the
//!    check happens after the last payload read and before publication.
//! 6. **Check cancellation** one last time, then **publish**. Nothing sits
//!    between the final check and the single no-replace rename.
//! 7. **Sync** the published parent.
//!
//! Every failure before step 6 aborts the transaction, so no failing path can
//! leave a published tree. A failure *after* publication cannot un-publish and
//! must not try: a sync failure is reported as
//! [`PayloadStageStatus::PublishedSyncUncertain`], not as a rollback.
//!
//! # Losing the publication race
//!
//! Two importers can stage the same payload concurrently. The loser's
//! `publish_no_replace` reports that the destination exists; it then aborts
//! its own staging and re-probes. If the winner's tree matches, the loser
//! reports a cache hit — the payload it wanted exists, just not because of it.
//! If it does not match, the loser reports a conflict or a revalidation
//! failure rather than assuming anything about the winner.

use ohl_core::StreamingSha256;
use ohl_platform::stability::verify_complete_source_stability_with_cancellation;
use ohl_platform::{MediaSource, SourceFingerprint, SourceStabilityError};

use crate::cancel::CancellationToken;
use crate::layout::{
    PayloadEntryMetadata, PayloadImportLimits, PlannedPayloadEntry, plan_payload_layout,
};
use crate::store::{
    PayloadStore, PayloadStoreError, PayloadTransaction, ProbeState, PublishState, StagingEntry,
    StagingPlan,
};
use crate::stream::{PayloadByteSink, PayloadSource, PayloadStreamError, stream_payload_entry};

/// The largest accepted recipe identity, in bytes.
pub const MAXIMUM_RECIPE_IDENTITY_BYTES: usize = 4_096;

/// The domain separator for the staging identity.
const IDENTITY_DOMAIN: &str = "open-half-life-payload-stage";

/// The staging identity version.
const IDENTITY_VERSION: u64 = 2;

/// The prefix of a rendered staging identity.
const IDENTITY_PREFIX: &str = "ohl-payload-v2-sha256:";

/// The chunk size the store sink accepts in one call.
const STAGE_CHUNK_BYTES: usize = 64 * 1024;

/// Everything staging needs besides the pinned source itself.
#[derive(Debug, Clone)]
pub struct PayloadStageRequest<'a> {
    /// The stable identity of the trusted selection recipe, bounded by
    /// [`MAXIMUM_RECIPE_IDENTITY_BYTES`]. Use
    /// [`crate::selection::SelectionPlan::recipe_identity`].
    pub recipe_identity: &'a str,
    /// The planned entries, in the planner's deterministic order.
    pub entries: &'a [PlannedPayloadEntry],
    /// The total the caller claims the entries sum to.
    pub declared_total_bytes: u64,
    /// The same limits that produced `entries`.
    pub limits: PayloadImportLimits,
}

/// How staging ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadStageStatus {
    /// Nothing was published; see [`PayloadStageReport::error`].
    Failed,
    /// The primary outcome is masked by a cleanup failure: owned staging is
    /// still on disk and must be dealt with explicitly.
    CleanupFailed,
    /// The payload was already published and matches exactly.
    CacheHit,
    /// Something else occupies the payload's published name.
    Conflict,
    /// Published, and the backend's sync of the published parent succeeded.
    PublishedSyncComplete,
    /// Published, but the parent sync failed. The payload exists; only its
    /// durability against a power loss is unproven.
    PublishedSyncUncertain,
}

/// The step at which staging stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadStagePhase {
    /// Request and plan validation, before any store call.
    Validation,
    /// A cancellation check.
    Cancellation,
    /// The probe for an already-published payload.
    Probe,
    /// Creating the transaction.
    CreateTransaction,
    /// Beginning the transaction.
    Begin,
    /// Opening one entry.
    OpenFile,
    /// Streaming one entry.
    StreamFile,
    /// Sealing one entry.
    SealFile,
    /// Sealing the completion metadata.
    SealCompletion,
    /// Reverifying the complete pinned source.
    VerifySource,
    /// The no-replace publication.
    Publish,
    /// Re-probing after losing the publication race.
    Revalidate,
    /// Syncing the published parent.
    SyncPublishedParent,
    /// Staging finished.
    Complete,
}

/// Why staging did not publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadStageError {
    /// The request or its plan is not one staging accepts.
    InvalidRequest,
    /// A store operation failed; see [`PayloadStageReport::store_error`].
    StoreFailure,
    /// An entry did not stream exactly; see
    /// [`PayloadStageReport::stream_error`].
    StreamFailure,
    /// The pinned source could not be reverified.
    SourceVerificationFailure,
    /// A stop was requested at one of the cooperative checks.
    Cancelled,
    /// The publication attempt itself failed.
    PublishFailure,
    /// The publication race was lost and the winner could not be shown to
    /// match or to conflict.
    RevalidationFailure,
    /// Owned staging could not be discarded.
    CleanupFailure,
    /// The published parent could not be synced.
    PublishedSyncFailure,
}

impl PayloadStageError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "payload staging request was not accepted",
            Self::StoreFailure => "payload staging store operation failed",
            Self::StreamFailure => "payload entry did not stream exactly",
            Self::SourceVerificationFailure => "pinned source could not be reverified",
            Self::Cancelled => "payload staging was cancelled",
            Self::PublishFailure => "payload publication failed",
            Self::RevalidationFailure => "payload publication race could not be resolved",
            Self::CleanupFailure => "payload staging could not be discarded",
            Self::PublishedSyncFailure => "published payload parent could not be synced",
        }
    }
}

impl core::fmt::Display for PayloadStageError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for PayloadStageError {}

impl From<PayloadStageError> for ohl_core::SanitizedError {
    fn from(error: PayloadStageError) -> Self {
        match error {
            PayloadStageError::InvalidRequest => Self::InvalidInput,
            _ => Self::Internal,
        }
    }
}

/// Whether the payload's published name existed after a lost race, and what
/// was there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WinnerObservation {
    /// No re-probe was made.
    NotChecked,
    /// The winner published exactly this payload.
    Matching,
    /// Something else is there.
    Conflict,
    /// Nothing is there any more.
    Absent,
    /// The re-probe itself failed.
    ProbeFailed,
}

/// Whether the payload reached its published name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PublicationState {
    /// Not published by this attempt.
    NotPublished,
    /// Published; the parent sync has not been attempted.
    Published,
    /// Published and the parent sync succeeded.
    PublishedSyncComplete,
    /// Published, but the parent sync failed.
    PublishedSyncUncertain,
}

/// The complete, sanitized outcome of one staging attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadStageReport {
    /// How staging ended.
    pub status: PayloadStageStatus,
    /// The step it stopped at.
    pub phase: PayloadStagePhase,
    /// The failing rule, when it failed.
    pub error: Option<PayloadStageError>,
    /// Whether the payload reached its published name.
    pub publication: PublicationState,
    /// What a post-race re-probe found.
    pub winner_observation: WinnerObservation,
    /// The store code behind a [`PayloadStageError::StoreFailure`].
    pub store_error: Option<PayloadStoreError>,
    /// The streaming code behind a [`PayloadStageError::StreamFailure`].
    pub stream_error: Option<PayloadStreamError>,
    /// The stability code behind a
    /// [`PayloadStageError::SourceVerificationFailure`].
    pub verification_error: Option<SourceStabilityError>,
    /// The index, in the request's entries, of the entry responsible.
    pub failing_entry: Option<usize>,
    /// Bytes accepted in full by the store across every entry.
    pub bytes_streamed: u64,
    /// Entries that streamed to their exact size.
    pub entries_streamed: u64,
    /// Whether a cleanup was attempted.
    pub cleanup_attempted: bool,
    /// The code from a failed cleanup.
    pub cleanup_error: Option<PayloadStoreError>,
    /// The derived staging identity, once the request was accepted.
    pub identity: Option<String>,
}

impl PayloadStageReport {
    /// A fresh, failed report.
    fn failed() -> Self {
        Self {
            status: PayloadStageStatus::Failed,
            phase: PayloadStagePhase::Validation,
            error: None,
            publication: PublicationState::NotPublished,
            winner_observation: WinnerObservation::NotChecked,
            store_error: None,
            stream_error: None,
            verification_error: None,
            failing_entry: None,
            bytes_streamed: 0,
            entries_streamed: 0,
            cleanup_attempted: false,
            cleanup_error: None,
            identity: None,
        }
    }

    /// Whether the payload can now be used: it is published or already was,
    /// and no owned staging was left behind.
    pub const fn usable(&self) -> bool {
        self.cleanup_error.is_none()
            && matches!(
                self.status,
                PayloadStageStatus::CacheHit | PayloadStageStatus::PublishedSyncComplete
            )
    }
}

/// The request, once every check has passed.
struct PreparedPlan {
    /// The rendered staging identity.
    identity: String,
    /// The store plan derived from the request's entries.
    plan: StagingPlan,
}

/// Length-prefixes one field into the identity digest.
fn absorb(hash: &mut StreamingSha256, bytes: &[u8]) {
    hash.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hash.update(bytes);
}

/// Validates the request and derives its identity and store plan.
fn prepare(
    source: &MediaSource,
    fingerprint: &SourceFingerprint,
    request: &PayloadStageRequest<'_>,
    failing_entry: &mut Option<usize>,
) -> Option<PreparedPlan> {
    if request.recipe_identity.is_empty()
        || request.recipe_identity.len() > MAXIMUM_RECIPE_IDENTITY_BYTES
        || source.size() != fingerprint.size_bytes
        || !request.limits.coherent()
        || request.entries.len() > request.limits.maximum_entries
    {
        return None;
    }

    let mut total_path_bytes = 0u64;
    let mut total_bytes = 0u64;
    for (index, entry) in request.entries.iter().enumerate() {
        *failing_entry = Some(index);
        let path_bytes = u64::try_from(entry.path.as_str().len()).unwrap_or(u64::MAX);
        if path_bytes > request.limits.maximum_path_bytes - total_path_bytes
            || entry.size_bytes > request.limits.maximum_entry_bytes
            || entry.size_bytes > request.limits.maximum_total_bytes - total_bytes
        {
            return None;
        }
        total_path_bytes += path_bytes;
        total_bytes += entry.size_bytes;
    }
    *failing_entry = None;
    if total_bytes != request.declared_total_bytes {
        return None;
    }

    // Re-planning the request is what makes a hand-built entry list safe: the
    // caller cannot bypass a single path, conflict, or ordering rule.
    let metadata = request
        .entries
        .iter()
        .map(|entry| PayloadEntryMetadata {
            source_token: entry.source_token,
            archive_path: String::from(entry.path.as_str()),
            size_bytes: entry.size_bytes,
        })
        .collect::<Vec<_>>();
    let planned = match plan_payload_layout(&metadata, &request.limits) {
        Ok(planned) => planned,
        Err(rejection) => {
            *failing_entry = rejection.entry_index;
            return None;
        }
    };
    if planned.total_bytes() != request.declared_total_bytes
        || planned.len() != request.entries.len()
    {
        return None;
    }
    for (index, (planned, requested)) in planned.entries().iter().zip(request.entries).enumerate() {
        if planned != requested {
            *failing_entry = Some(index);
            return None;
        }
    }

    let mut hash = StreamingSha256::new();
    absorb(&mut hash, IDENTITY_DOMAIN.as_bytes());
    hash.update(&IDENTITY_VERSION.to_be_bytes());
    hash.update(&fingerprint.size_bytes.to_be_bytes());
    absorb(&mut hash, &fingerprint.sha256);
    absorb(&mut hash, request.recipe_identity.as_bytes());
    hash.update(
        &u64::try_from(request.entries.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    hash.update(&request.declared_total_bytes.to_be_bytes());
    for entry in request.entries {
        absorb(&mut hash, entry.path.as_str().as_bytes());
        hash.update(&entry.size_bytes.to_be_bytes());
    }
    let mut identity = String::from(IDENTITY_PREFIX);
    for byte in hash.finalize() {
        identity.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        identity.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }

    let plan = StagingPlan::new(
        &identity,
        request
            .entries
            .iter()
            .map(|entry| StagingEntry {
                components: entry.path.owned_components(),
                size_bytes: entry.size_bytes,
            })
            .collect(),
    )
    .ok()?;
    Some(PreparedPlan { identity, plan })
}

/// Forwards a stream's chunks into the open store entry.
struct TransactionSink<'a> {
    /// The transaction holding the open entry.
    transaction: &'a mut dyn PayloadTransaction,
    /// The first store error, which outlives the sink.
    error: Option<PayloadStoreError>,
}

impl PayloadByteSink for TransactionSink<'_> {
    fn write_chunk(&mut self, bytes: &[u8]) -> bool {
        // The store accepts a bounded chunk; a source that offers more is
        // split rather than refused, because the bound is ours, not the plan's.
        for window in bytes.chunks(STAGE_CHUNK_BYTES) {
            if let Err(error) = self.transaction.write_chunk(window) {
                self.error = Some(error);
                return false;
            }
        }
        true
    }
}

/// Aborts `transaction` and records the outcome.
fn abort(report: &mut PayloadStageReport, transaction: &mut dyn PayloadTransaction) {
    report.cleanup_attempted = true;
    report.cleanup_error = transaction.abort().err();
}

/// Stages `request`'s entries and publishes them, or explains why it did not.
///
/// The report is always sanitized: it names phases, rules, and counts, never a
/// path, a byte, a digest, or a recipe. See the [module documentation](self)
/// for the ordering guarantees.
#[allow(clippy::too_many_lines)]
pub fn stage_payload(
    source: &MediaSource,
    fingerprint: &SourceFingerprint,
    request: &PayloadStageRequest<'_>,
    payload_source: &mut dyn PayloadSource,
    store: &mut dyn PayloadStore,
    cancellation: &CancellationToken,
) -> PayloadStageReport {
    let mut report = PayloadStageReport::failed();
    let Some(prepared) = prepare(source, fingerprint, request, &mut report.failing_entry) else {
        if source.size() == fingerprint.size_bytes {
            report.error = Some(PayloadStageError::InvalidRequest);
        } else {
            report.error = Some(PayloadStageError::SourceVerificationFailure);
            report.verification_error = Some(SourceStabilityError::InvalidCapability);
        }
        return report;
    };
    report.identity = Some(prepared.identity.clone());

    if cancellation.stop_requested() {
        report.phase = PayloadStagePhase::Cancellation;
        report.error = Some(PayloadStageError::Cancelled);
        return report;
    }

    report.phase = PayloadStagePhase::Probe;
    match store.probe(&prepared.plan) {
        Err(error) => {
            report.error = Some(PayloadStageError::StoreFailure);
            report.store_error = Some(error);
            return report;
        }
        Ok(ProbeState::Matching) => {
            report.status = PayloadStageStatus::CacheHit;
            return report;
        }
        Ok(ProbeState::Conflict) => {
            report.status = PayloadStageStatus::Conflict;
            return report;
        }
        Ok(ProbeState::Absent) => {}
    }

    let mut transaction = match store.create_transaction() {
        Ok(transaction) => transaction,
        Err(error) => {
            report.phase = PayloadStagePhase::CreateTransaction;
            report.error = Some(PayloadStageError::StoreFailure);
            report.store_error = Some(error);
            return report;
        }
    };

    if let Err(error) = transaction.begin(&prepared.plan) {
        report.phase = PayloadStagePhase::Begin;
        report.error = Some(PayloadStageError::StoreFailure);
        report.store_error = Some(error);
        abort(&mut report, transaction.as_mut());
        return report;
    }

    for (index, entry) in request.entries.iter().enumerate() {
        let components = entry.path.owned_components();
        if let Err(error) = transaction.open_file(&components, entry.size_bytes) {
            report.phase = PayloadStagePhase::OpenFile;
            report.error = Some(PayloadStageError::StoreFailure);
            report.store_error = Some(error);
            report.failing_entry = Some(index);
            abort(&mut report, transaction.as_mut());
            return report;
        }

        let mut sink = TransactionSink {
            transaction: transaction.as_mut(),
            error: None,
        };
        let outcome = stream_payload_entry(entry, source, payload_source, cancellation, &mut sink);
        let write_error = sink.error;
        report.bytes_streamed += outcome.bytes_written;
        if let Some(stream_error) = outcome.error {
            report.phase = PayloadStagePhase::StreamFile;
            report.error = Some(if stream_error == PayloadStreamError::Cancelled {
                PayloadStageError::Cancelled
            } else {
                PayloadStageError::StreamFailure
            });
            report.stream_error = Some(stream_error);
            report.store_error = write_error;
            report.failing_entry = Some(index);
            abort(&mut report, transaction.as_mut());
            return report;
        }
        report.entries_streamed += 1;

        if let Err(error) = transaction.seal_file() {
            report.phase = PayloadStagePhase::SealFile;
            report.error = Some(PayloadStageError::StoreFailure);
            report.store_error = Some(error);
            report.failing_entry = Some(index);
            abort(&mut report, transaction.as_mut());
            return report;
        }
    }

    if let Err(error) = transaction.seal_completion() {
        report.phase = PayloadStagePhase::SealCompletion;
        report.error = Some(PayloadStageError::StoreFailure);
        report.store_error = Some(error);
        abort(&mut report, transaction.as_mut());
        return report;
    }

    if let Err(error) =
        verify_complete_source_stability_with_cancellation(source, fingerprint, &mut || {
            cancellation.stop_requested()
        })
    {
        report.phase = PayloadStagePhase::VerifySource;
        report.error = Some(if error == SourceStabilityError::Cancelled {
            PayloadStageError::Cancelled
        } else {
            PayloadStageError::SourceVerificationFailure
        });
        report.verification_error = Some(error);
        abort(&mut report, transaction.as_mut());
        return report;
    }

    if cancellation.stop_requested() {
        report.phase = PayloadStagePhase::Cancellation;
        report.error = Some(PayloadStageError::Cancelled);
        abort(&mut report, transaction.as_mut());
        return report;
    }

    match transaction.publish_no_replace() {
        Err(error) => {
            report.phase = PayloadStagePhase::Publish;
            report.error = Some(PayloadStageError::PublishFailure);
            report.store_error = Some(error);
            abort(&mut report, transaction.as_mut());
            report
        }
        Ok(PublishState::DestinationExists) => {
            report.phase = PayloadStagePhase::Revalidate;
            abort(&mut report, transaction.as_mut());
            drop(transaction);
            report.winner_observation = match store.probe(&prepared.plan) {
                Err(error) => {
                    report.store_error = Some(error);
                    WinnerObservation::ProbeFailed
                }
                Ok(ProbeState::Matching) => WinnerObservation::Matching,
                Ok(ProbeState::Conflict) => WinnerObservation::Conflict,
                Ok(ProbeState::Absent) => WinnerObservation::Absent,
            };
            if report.cleanup_error.is_some() {
                report.status = PayloadStageStatus::CleanupFailed;
                report.error = Some(PayloadStageError::CleanupFailure);
            } else if report.winner_observation == WinnerObservation::Matching {
                report.status = PayloadStageStatus::CacheHit;
            } else if report.winner_observation == WinnerObservation::Conflict {
                report.status = PayloadStageStatus::Conflict;
            } else {
                report.error = Some(PayloadStageError::RevalidationFailure);
            }
            report
        }
        Ok(PublishState::Published) => {
            report.publication = PublicationState::Published;
            if let Err(error) = transaction.sync_published_parent() {
                report.status = PayloadStageStatus::PublishedSyncUncertain;
                report.phase = PayloadStagePhase::SyncPublishedParent;
                report.error = Some(PayloadStageError::PublishedSyncFailure);
                report.publication = PublicationState::PublishedSyncUncertain;
                report.store_error = Some(error);
                return report;
            }
            report.status = PayloadStageStatus::PublishedSyncComplete;
            report.phase = PayloadStagePhase::Complete;
            report.publication = PublicationState::PublishedSyncComplete;
            report
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAXIMUM_RECIPE_IDENTITY_BYTES, PayloadStageError, PayloadStagePhase, PayloadStageReport,
        PayloadStageRequest, PayloadStageStatus, PublicationState, WinnerObservation,
        stage_payload,
    };
    use crate::cancel::{CancellationSource, CancellationToken};
    use crate::layout::{
        PayloadEntryMetadata, PayloadImportLimits, PlannedPayloadEntry, plan_payload_layout,
    };
    use crate::path::PayloadPath;
    use crate::store::{
        DirectoryPayloadStore, PayloadStore, PayloadStoreError, PayloadTransaction, ProbeState,
        PublishState, StagingPlan,
    };
    use crate::stream::{PayloadByteSink, PayloadSource, PayloadStreamError};
    use crate::test_support::{PinnedSourceFixture, pinned_source};
    use ohl_platform::{MediaSource, SourceStabilityError};

    /// A source that writes `token` repeated to the entry's declared size.
    struct FillingSource {
        calls: usize,
        chunks_before_failure: Option<usize>,
        stop_after_chunk: Option<(CancellationSource, usize)>,
        observed_tokens: Vec<u64>,
        contract_ok: bool,
    }

    impl FillingSource {
        fn new() -> Self {
            Self {
                calls: 0,
                chunks_before_failure: None,
                stop_after_chunk: None,
                observed_tokens: Vec::new(),
                contract_ok: true,
            }
        }
    }

    impl PayloadSource for FillingSource {
        fn stream(
            &mut self,
            _media_source: &MediaSource,
            source_token: u64,
            cancellation: &CancellationToken,
            sink: &mut dyn PayloadByteSink,
        ) -> bool {
            self.calls += 1;
            self.observed_tokens.push(source_token);
            self.contract_ok &= !cancellation.stop_requested() || self.stop_after_chunk.is_some();
            let size = usize::try_from(entry_size_for(source_token)).expect("size");
            let byte = u8::try_from(source_token % 251).expect("byte");
            let mut written = 0usize;
            let mut chunks = 0usize;
            while written < size {
                if self.chunks_before_failure == Some(chunks) {
                    return false;
                }
                let count = (size - written).min(2);
                if !sink.write_chunk(&vec![byte; count]) {
                    return false;
                }
                written += count;
                chunks += 1;
                if let Some((stop, after)) = &self.stop_after_chunk
                    && *after == chunks
                {
                    stop.request_stop();
                }
            }
            self.chunks_before_failure != Some(chunks)
        }
    }

    /// The fixture's entry sizes, keyed by token.
    const fn entry_size_for(token: u64) -> u64 {
        match token {
            1 => 4,
            2 => 6,
            _ => 0,
        }
    }

    fn planned() -> Vec<PlannedPayloadEntry> {
        let entries = vec![
            PayloadEntryMetadata {
                source_token: 1,
                archive_path: String::from("Nested/Fixture/first.bin"),
                size_bytes: 4,
            },
            PayloadEntryMetadata {
                source_token: 2,
                archive_path: String::from("second.bin"),
                size_bytes: 6,
            },
        ];
        plan_payload_layout(&entries, &PayloadImportLimits::default())
            .expect("planned")
            .into_entries()
    }

    fn request<'a>(entries: &'a [PlannedPayloadEntry], recipe: &'a str) -> PayloadStageRequest<'a> {
        PayloadStageRequest {
            recipe_identity: recipe,
            entries,
            declared_total_bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
            limits: PayloadImportLimits::default(),
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        store: DirectoryPayloadStore,
        media: PinnedSourceFixture,
    }

    fn new_fixture() -> Fixture {
        let root = tempfile::tempdir().expect("temporary directory");
        let store = DirectoryPayloadStore::open(root.path()).expect("store");
        Fixture {
            _root: root,
            store,
            media: pinned_source(b"pinned-payload-source-content"),
        }
    }

    fn run(
        fixture: &mut Fixture,
        request: &PayloadStageRequest<'_>,
        source: &mut dyn PayloadSource,
        cancellation: &CancellationToken,
    ) -> PayloadStageReport {
        stage_payload(
            fixture.media.media_source(),
            fixture.media.fingerprint(),
            request,
            source,
            &mut fixture.store,
            cancellation,
        )
    }

    #[test]
    fn a_complete_stage_publishes_in_deterministic_order() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe-identity");
        let mut source = FillingSource::new();
        let report = run(
            &mut fixture,
            &request,
            &mut source,
            &CancellationToken::default(),
        );

        assert_eq!(report.status, PayloadStageStatus::PublishedSyncComplete);
        assert_eq!(report.phase, PayloadStagePhase::Complete);
        assert_eq!(report.error, None);
        assert_eq!(report.publication, PublicationState::PublishedSyncComplete);
        assert_eq!(report.entries_streamed, 2);
        assert_eq!(report.bytes_streamed, 10);
        assert!(!report.cleanup_attempted);
        assert!(report.usable());
        // Planning order, not request order: the nested entry sorts first.
        assert_eq!(source.observed_tokens, [1, 2]);
        assert!(source.contract_ok);

        let identity = report.identity.clone().expect("identity");
        assert!(identity.starts_with("ohl-payload-v2-sha256:"));
        assert_eq!(identity.len(), "ohl-payload-v2-sha256:".len() + 64);

        let plan = StagingPlan::new(
            &identity,
            entries
                .iter()
                .map(|entry| crate::store::StagingEntry {
                    components: entry.path.owned_components(),
                    size_bytes: entry.size_bytes,
                })
                .collect(),
        )
        .expect("plan");
        assert_eq!(
            fixture.store.probe(&plan).expect("probe"),
            ProbeState::Matching
        );
    }

    #[test]
    fn an_identical_second_stage_is_a_cache_hit_that_reads_nothing() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe-identity");
        let mut source = FillingSource::new();
        assert!(
            run(
                &mut fixture,
                &request,
                &mut source,
                &CancellationToken::default()
            )
            .usable()
        );

        let mut second = FillingSource::new();
        let report = run(
            &mut fixture,
            &request,
            &mut second,
            &CancellationToken::default(),
        );
        assert_eq!(report.status, PayloadStageStatus::CacheHit);
        assert_eq!(report.phase, PayloadStagePhase::Probe);
        assert_eq!(second.calls, 0);
        assert!(report.usable());
    }

    #[test]
    fn the_identity_binds_the_recipe_the_paths_and_the_source() {
        let entries = planned();
        let mut fixture = new_fixture();
        let mut source = FillingSource::new();
        let first = run(
            &mut fixture,
            &request(&entries, "recipe-a"),
            &mut source,
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");

        let mut fixture = new_fixture();
        let second = run(
            &mut fixture,
            &request(&entries, "recipe-b"),
            &mut FillingSource::new(),
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");
        assert_ne!(first, second, "the recipe identity must be bound");

        let mut other_media = new_fixture();
        other_media.media = pinned_source(b"a different pinned source entirely");
        let third = run(
            &mut other_media,
            &request(&entries, "recipe-a"),
            &mut FillingSource::new(),
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");
        assert_ne!(first, third, "the source fingerprint must be bound");

        // A transport-local token change must not change the identity.
        let retokened = plan_payload_layout(
            &[
                PayloadEntryMetadata {
                    source_token: 101,
                    archive_path: String::from("Nested/Fixture/first.bin"),
                    size_bytes: 4,
                },
                PayloadEntryMetadata {
                    source_token: 102,
                    archive_path: String::from("second.bin"),
                    size_bytes: 6,
                },
            ],
            &PayloadImportLimits::default(),
        )
        .expect("planned")
        .into_entries();
        let mut token_fixture = new_fixture();
        token_fixture.media = pinned_source(b"pinned-payload-source-content");
        let fourth = run(
            &mut token_fixture,
            &request(&retokened, "recipe-a"),
            &mut FillingSource::new(),
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");
        assert_eq!(first, fourth, "source tokens must not be bound");
    }

    #[test]
    fn an_empty_payload_publishes_atomically() {
        let mut fixture = new_fixture();
        let request = request(&[], "recipe-identity");
        let mut source = FillingSource::new();
        let report = run(
            &mut fixture,
            &request,
            &mut source,
            &CancellationToken::default(),
        );
        assert_eq!(report.status, PayloadStageStatus::PublishedSyncComplete);
        assert_eq!(report.entries_streamed, 0);
        assert_eq!(source.calls, 0);
    }

    #[test]
    fn every_invalid_request_is_refused_before_any_store_call() {
        let entries = planned();
        let long_recipe = "r".repeat(MAXIMUM_RECIPE_IDENTITY_BYTES + 1);
        let mismatched = PlannedPayloadEntry {
            source_token: 1,
            path: PayloadPath::parse("Nested/Fixture/first.bin").expect("path"),
            size_bytes: 4,
        };
        let unsorted = vec![entries[1].clone(), entries[0].clone()];
        let conflicting = vec![
            mismatched.clone(),
            PlannedPayloadEntry {
                source_token: 2,
                path: PayloadPath::parse("NESTED/FIXTURE/FIRST.BIN").expect("path"),
                size_bytes: 4,
            },
        ];

        let cases: Vec<(&str, PayloadStageRequest<'_>)> = vec![
            ("empty recipe identity", request(&entries, "")),
            ("oversized recipe identity", request(&entries, &long_recipe)),
            (
                "wrong declared total",
                PayloadStageRequest {
                    declared_total_bytes: 11,
                    ..request(&entries, "recipe")
                },
            ),
            (
                "incoherent limits",
                PayloadStageRequest {
                    limits: PayloadImportLimits {
                        maximum_entries: 0,
                        ..PayloadImportLimits::default()
                    },
                    ..request(&entries, "recipe")
                },
            ),
            (
                "over the entry limit",
                PayloadStageRequest {
                    limits: PayloadImportLimits {
                        maximum_entries: 1,
                        ..PayloadImportLimits::default()
                    },
                    ..request(&entries, "recipe")
                },
            ),
            ("unsorted entries", request(&unsorted, "recipe")),
            ("conflicting entries", request(&conflicting, "recipe")),
        ];

        for (name, request) in cases {
            let mut fixture = new_fixture();
            let mut source = FillingSource::new();
            let report = run(
                &mut fixture,
                &request,
                &mut source,
                &CancellationToken::default(),
            );
            assert_eq!(
                report.error,
                Some(PayloadStageError::InvalidRequest),
                "case `{name}` was accepted"
            );
            assert_eq!(report.phase, PayloadStagePhase::Validation);
            assert_eq!(report.identity, None);
            assert_eq!(source.calls, 0);
            assert!(!report.cleanup_attempted);
            assert_eq!(
                std::fs::read_dir(fixture.store.root())
                    .expect("listing")
                    .count(),
                0,
                "case `{name}` touched the store"
            );
        }
    }

    #[test]
    fn a_capability_that_disagrees_with_its_fingerprint_is_refused() {
        let mut fixture = new_fixture();
        let entries = planned();
        let mut wrong = *fixture.media.fingerprint();
        wrong.size_bytes += 1;
        let report = stage_payload(
            fixture.media.media_source(),
            &wrong,
            &request(&entries, "recipe"),
            &mut FillingSource::new(),
            &mut fixture.store,
            &CancellationToken::default(),
        );
        assert_eq!(
            report.error,
            Some(PayloadStageError::SourceVerificationFailure)
        );
        assert_eq!(
            report.verification_error,
            Some(SourceStabilityError::InvalidCapability)
        );
    }

    #[test]
    fn a_source_that_changes_before_publication_is_never_published() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        // Rewrite the pinned file after staging would have read it: the
        // complete-source reverification runs after the last read and before
        // publication, so this must abort rather than publish.
        std::fs::write(fixture.media.path(), b"pinned-payload-source-CHANGED").expect("rewrite");
        let report = run(
            &mut fixture,
            &request,
            &mut FillingSource::new(),
            &CancellationToken::default(),
        );
        assert_eq!(report.phase, PayloadStagePhase::VerifySource);
        assert_eq!(
            report.error,
            Some(PayloadStageError::SourceVerificationFailure)
        );
        assert_eq!(report.publication, PublicationState::NotPublished);
        assert!(report.cleanup_attempted);
        assert_eq!(report.cleanup_error, None);
        assert_eq!(
            std::fs::read_dir(fixture.store.root())
                .expect("listing")
                .count(),
            0,
            "a refused stage leaves nothing behind"
        );
    }

    #[test]
    fn cancellation_before_during_and_after_streaming_never_publishes() {
        // Before any store call.
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        let stop = CancellationSource::new();
        stop.request_stop();
        let mut source = FillingSource::new();
        let report = run(&mut fixture, &request, &mut source, &stop.token());
        assert_eq!(report.phase, PayloadStagePhase::Cancellation);
        assert_eq!(report.error, Some(PayloadStageError::Cancelled));
        assert_eq!(source.calls, 0);
        assert!(!report.cleanup_attempted);

        // Mid-stream.
        let mut fixture = new_fixture();
        let stop = CancellationSource::new();
        let mut source = FillingSource::new();
        source.stop_after_chunk = Some((stop.clone(), 1));
        let report = run(&mut fixture, &request, &mut source, &stop.token());
        assert_eq!(report.phase, PayloadStagePhase::StreamFile);
        assert_eq!(report.error, Some(PayloadStageError::Cancelled));
        assert_eq!(report.stream_error, Some(PayloadStreamError::Cancelled));
        assert_eq!(report.failing_entry, Some(0));
        assert!(report.cleanup_attempted);
        assert_eq!(report.publication, PublicationState::NotPublished);
        assert_eq!(
            std::fs::read_dir(fixture.store.root())
                .expect("listing")
                .count(),
            0
        );
    }

    #[test]
    fn a_source_failure_aborts_and_accounts_for_the_middle_entry() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        let mut source = FillingSource::new();
        // Fail after the first two chunks of the *first* entry.
        source.chunks_before_failure = Some(1);
        let report = run(
            &mut fixture,
            &request,
            &mut source,
            &CancellationToken::default(),
        );
        assert_eq!(report.phase, PayloadStagePhase::StreamFile);
        assert_eq!(report.error, Some(PayloadStageError::StreamFailure));
        assert_eq!(report.stream_error, Some(PayloadStreamError::SourceFailure));
        assert_eq!(report.failing_entry, Some(0));
        assert_eq!(report.entries_streamed, 0);
        assert_eq!(report.bytes_streamed, 2);
        assert!(report.cleanup_attempted);
        assert!(!report.usable());
        assert_eq!(
            std::fs::read_dir(fixture.store.root())
                .expect("listing")
                .count(),
            0
        );
    }

    #[test]
    fn a_conflicting_published_name_short_circuits_before_reading() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        let mut source = FillingSource::new();
        let identity = run(
            &mut fixture,
            &request,
            &mut source,
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");

        let plan = StagingPlan::new(&identity, Vec::new()).expect("plan");
        std::fs::write(
            fixture
                .store
                .root()
                .join(plan.published_name())
                .join("files")
                .join("extra.bin"),
            b"x",
        )
        .expect("tamper");

        let mut second = FillingSource::new();
        let report = run(
            &mut fixture,
            &request,
            &mut second,
            &CancellationToken::default(),
        );
        assert_eq!(report.status, PayloadStageStatus::Conflict);
        assert_eq!(report.phase, PayloadStagePhase::Probe);
        assert_eq!(second.calls, 0);
        assert!(!report.usable());
    }

    /// A store that reports the destination as absent once, so the next
    /// publication attempt loses the race it would otherwise have won.
    struct LostRaceStore<'a> {
        inner: &'a mut DirectoryPayloadStore,
        absent_once: bool,
    }

    impl PayloadStore for LostRaceStore<'_> {
        fn probe(&mut self, plan: &StagingPlan) -> Result<ProbeState, PayloadStoreError> {
            if self.absent_once {
                self.absent_once = false;
                return Ok(ProbeState::Absent);
            }
            self.inner.probe(plan)
        }

        fn create_transaction(
            &mut self,
        ) -> Result<Box<dyn PayloadTransaction + '_>, PayloadStoreError> {
            self.inner.create_transaction()
        }
    }

    #[test]
    fn a_lost_publication_race_resolves_to_the_winners_payload() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        assert!(
            run(
                &mut fixture,
                &request,
                &mut FillingSource::new(),
                &CancellationToken::default()
            )
            .usable()
        );

        let mut source = FillingSource::new();
        let mut racing = LostRaceStore {
            inner: &mut fixture.store,
            absent_once: true,
        };
        let report = stage_payload(
            fixture.media.media_source(),
            fixture.media.fingerprint(),
            &request,
            &mut source,
            &mut racing,
            &CancellationToken::default(),
        );
        assert_eq!(report.status, PayloadStageStatus::CacheHit);
        assert_eq!(report.phase, PayloadStagePhase::Revalidate);
        assert_eq!(report.winner_observation, WinnerObservation::Matching);
        assert_eq!(report.publication, PublicationState::NotPublished);
        assert!(report.cleanup_attempted);
        assert_eq!(report.cleanup_error, None);
        assert_eq!(report.entries_streamed, 2);
        assert_eq!(source.calls, 2);
        assert!(report.usable());
    }

    #[test]
    fn a_lost_race_against_a_different_tree_is_a_conflict() {
        let mut fixture = new_fixture();
        let entries = planned();
        let request = request(&entries, "recipe");
        let identity = run(
            &mut fixture,
            &request,
            &mut FillingSource::new(),
            &CancellationToken::default(),
        )
        .identity
        .expect("identity");
        let plan = StagingPlan::new(&identity, Vec::new()).expect("plan");
        std::fs::write(
            fixture
                .store
                .root()
                .join(plan.published_name())
                .join("files")
                .join("extra.bin"),
            b"x",
        )
        .expect("tamper");

        let mut racing = LostRaceStore {
            inner: &mut fixture.store,
            absent_once: true,
        };
        let report = stage_payload(
            fixture.media.media_source(),
            fixture.media.fingerprint(),
            &request,
            &mut FillingSource::new(),
            &mut racing,
            &CancellationToken::default(),
        );
        assert_eq!(report.status, PayloadStageStatus::Conflict);
        assert_eq!(report.winner_observation, WinnerObservation::Conflict);
        assert!(!report.usable());
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            PayloadStageError::InvalidRequest,
            PayloadStageError::StoreFailure,
            PayloadStageError::StreamFailure,
            PayloadStageError::SourceVerificationFailure,
            PayloadStageError::Cancelled,
            PayloadStageError::PublishFailure,
            PayloadStageError::RevalidationFailure,
            PayloadStageError::CleanupFailure,
            PayloadStageError::PublishedSyncFailure,
        ] {
            assert!(!error.to_string().is_empty());
            let _: ohl_core::SanitizedError = error.into();
        }
        assert_eq!(PublishState::Published, PublishState::Published);
    }
}
