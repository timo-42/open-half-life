#pragma once

#include "ohl/media/cancellation.hpp"
#include "ohl/media/iso_inspector.hpp"
#include "ohl/media/payload_layout.hpp"
#include "ohl/media/payload_stream.hpp"
#include "ohl/platform/atomic_directory_store.hpp"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <string>
#include <string_view>

namespace ohl::media {

inline constexpr std::size_t kMaximumPayloadStageRecipeIdentityBytes = 4'096;

struct PayloadStageRequest {
  // Stable identity for the trusted selection recipe. It is bounded by
  // kMaximumPayloadStageRecipeIdentityBytes and length-prefixed by the
  // canonical stage identity encoder. Source identity is derived exclusively
  // from ValidatedMedia.
  std::string_view recipe_identity;
  std::span<const PlannedPayloadEntry> entries;
  std::uint64_t declared_total_bytes{0};
  // The same limits used to produce entries. Quotas are checked without
  // allocation before paths or metadata are copied.
  PayloadImportLimits limits{};
};

// "sync complete" means only that sync_published_parent() returned success;
// it does not claim universal crash durability for every filesystem.
enum class PayloadStageStatus {
  failed,
  cleanup_failed,
  cache_hit,
  conflict,
  published_sync_complete,
  published_sync_uncertain,
};

enum class PayloadStagePhase {
  validation,
  cancellation,
  probe,
  create_transaction,
  begin,
  open_file,
  stream_file,
  seal_file,
  seal_completion,
  verify_source,
  publish,
  revalidate,
  sync_published_parent,
  complete,
};

enum class PayloadStageError {
  none,
  invalid_request,
  store_failure,
  stream_failure,
  source_verification_failure,
  cancelled,
  publish_failure,
  revalidation_failure,
  cleanup_failure,
  published_sync_failure,
};

// Sanitized result of the complete pinned-source verification. It intentionally
// contains no source identity, path, bytes, or digest material.
enum class PayloadStageVerificationError {
  none,
  invalid_capability,
  source_changed,
  read_failure,
  digest_mismatch,
  cancelled,
};

enum class PayloadPublicationState {
  not_published,
  published,
  published_sync_complete,
  published_sync_uncertain,
};

enum class PayloadWinnerObservation {
  not_checked,
  matching,
  conflict,
  absent,
  probe_failed,
};

struct PayloadStageResult {
  PayloadStageStatus status{PayloadStageStatus::failed};
  PayloadStagePhase phase{PayloadStagePhase::validation};
  PayloadStageError error{PayloadStageError::none};
  PayloadPublicationState publication{PayloadPublicationState::not_published};
  PayloadWinnerObservation winner_observation{
      PayloadWinnerObservation::not_checked};
  platform::AtomicDirectoryStoreError store_error{
      platform::AtomicDirectoryStoreError::none};
  PayloadStreamError stream_error{PayloadStreamError::none};
  PayloadStageVerificationError verification_error{
      PayloadStageVerificationError::none};
  std::optional<std::size_t> failing_entry;
  // Counts fully accepted chunks and exact successful source streams. An entry
  // remains counted if a later seal, completion, or publication step fails.
  std::uint64_t bytes_streamed{0};
  std::uint64_t entries_streamed{0};
  bool cleanup_attempted{false};
  platform::AtomicDirectoryStoreError cleanup_error{
      platform::AtomicDirectoryStoreError::none};
  std::string identity;

  [[nodiscard]] bool usable() const noexcept {
    return cleanup_error == platform::AtomicDirectoryStoreError::none &&
           (status == PayloadStageStatus::cache_hit ||
            status == PayloadStageStatus::published_sync_complete);
  }
};

// Validates the media capability and prepares the complete request before the
// first store call. Identity is a versioned SHA-256 digest over the validated
// source size and SHA-256, the length-prefixed recipe identity, normalized
// paths and declared sizes, entry count, and total size; transport-local source
// tokens are not included. After staging is sealed, the complete pinned source
// is reverified and cancellation is checked immediately before publication.
// The injected store remains responsible for all native safety and atomicity
// guarantees.
[[nodiscard]] PayloadStageResult stage_payload(
    const ValidatedMedia& media, const PayloadStageRequest& request,
    PayloadSource& source, platform::AtomicDirectoryStore& store,
    CancellationToken cancellation = {});

}  // namespace ohl::media
