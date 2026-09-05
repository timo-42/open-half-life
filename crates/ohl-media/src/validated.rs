//! The move-only proof that one pinned source was fingerprinted.
//!
//! [`ValidatedMedia`] is the Rust port of the C++ `media::ValidatedMedia`. It
//! binds together, and only together:
//!
//! - the pinned [`MediaSource`] capability the bytes were read through;
//! - the size that capability was acquired with;
//! - the SHA-256 of its complete content at validation time;
//! - the [`MediaDescription`] a preflight crate produced for it.
//!
//! What it is and is not:
//!
//! > It is evidence that the source passed those gates at validation time,
//! > not a promise that future content cannot change.
//! > --- `docs/ARCHITECTURE.md`
//!
//! Consequently every consumer — the provenance cache above all — re-verifies
//! the pinned source before it acts, and the type is **not** [`Clone`]: a
//! proof is moved to its single consumer, so a stale copy of it cannot
//! outlive the check that produced it. Rust's move semantics make the C++
//! "moved-from value is invalid" state unrepresentable rather than merely
//! rejected.

use std::sync::Arc;

use ohl_platform::{MediaSource, SourceFingerprint};

use crate::description::MediaDescription;
use crate::digest::{MediaDigest, fingerprint, source_fingerprint};
use crate::error::MediaError;

/// Proof that one pinned media source was fingerprinted and described.
///
/// Deliberately not [`Clone`] and not [`Copy`]; see the
/// [module documentation](self). The absence of `Clone` is part of the
/// contract, so it is asserted here rather than left to review:
///
/// ```compile_fail
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<ohl_media::ValidatedMedia>();
/// ```
#[derive(Debug)]
pub struct ValidatedMedia {
    source: Arc<MediaSource>,
    size_bytes: u64,
    digest: MediaDigest,
    description: MediaDescription,
}

impl ValidatedMedia {
    /// Binds an already computed digest to the capability it came from.
    ///
    /// The constructor performs the stability check itself: the capability's
    /// pinned size must equal `size_bytes`, and
    /// [`MediaSource::verify_unchanged`] must succeed. A caller therefore
    /// cannot mint a proof for a source that has already moved on.
    ///
    /// The `Arc` is shared on purpose: the application mounts the same
    /// capability through the VFS while handing this proof to the cache, and
    /// it must not reopen the pathname to do so.
    ///
    /// # Errors
    ///
    /// [`MediaError::InvalidCapability`] when the capability and `size_bytes`
    /// disagree, [`MediaError::SourceChanged`] when the pinned object has
    /// changed, and [`MediaError::SourceReadFailed`] when it cannot be
    /// re-queried.
    pub fn new(
        source: Arc<MediaSource>,
        size_bytes: u64,
        digest: MediaDigest,
        description: MediaDescription,
    ) -> Result<Self, MediaError> {
        if source.size() != size_bytes {
            return Err(MediaError::InvalidCapability);
        }
        source.verify_unchanged()?;
        Ok(Self {
            source,
            size_bytes,
            digest,
            description,
        })
    }

    /// Fingerprints `source` now and returns the resulting proof.
    ///
    /// This is the ordinary entry point after a preflight crate has produced
    /// a [`MediaDescription`]: it hashes the complete content through
    /// [`fingerprint`] (which brackets the hash with stability checks) and
    /// binds the result.
    ///
    /// # Errors
    ///
    /// See [`fingerprint`] and [`ValidatedMedia::new`].
    pub fn fingerprinting(
        source: Arc<MediaSource>,
        description: MediaDescription,
    ) -> Result<Self, MediaError> {
        let size_bytes = source.size();
        let digest = fingerprint(source.as_ref())?;
        Self::new(source, size_bytes, digest, description)
    }

    /// The pinned capability the proof was made from.
    #[must_use]
    pub fn source(&self) -> &Arc<MediaSource> {
        &self.source
    }

    /// The pinned size, in bytes.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// The SHA-256 of the complete content at validation time.
    #[must_use]
    pub const fn digest(&self) -> &MediaDigest {
        &self.digest
    }

    /// What a preflight crate recognised about the source.
    #[must_use]
    pub const fn description(&self) -> &MediaDescription {
        &self.description
    }

    /// The proof restated as the `ohl-platform` fingerprint value, for
    /// [`ohl_platform::verify_complete_source_stability`].
    #[must_use]
    pub const fn source_fingerprint(&self) -> SourceFingerprint {
        source_fingerprint(self.size_bytes, &self.digest)
    }

    /// Re-verifies the pinned object without rehashing it.
    ///
    /// # Errors
    ///
    /// [`MediaError::SourceChanged`] or [`MediaError::SourceReadFailed`].
    pub fn verify_unchanged(&self) -> Result<(), MediaError> {
        self.source.verify_unchanged().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::ValidatedMedia;

    /// The proof travels to the thread that publishes the cache entry, so it
    /// must be `Send`, and the shared capability must stay `Sync`.
    #[test]
    fn the_proof_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ValidatedMedia>();
    }
}
