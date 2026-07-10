#pragma once

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

inline constexpr std::size_t kMaximumPayloadStageInputIdentityBytes = 4'096;

struct PayloadStageRequest {
  // Stable caller-defined identities for the validated media and selection
  // recipe. Each is bounded by kMaximumPayloadStageInputIdentityBytes and is
  // length-prefixed by the canonical stage identity encoder.
  std::string_view source_identity;
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
  probe,
  create_transaction,
  begin,
  open_file,
  stream_file,
  seal_file,
  seal_completion,
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
  publish_failure,
  revalidation_failure,
  cleanup_failure,
  published_sync_failure,
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

// Validates and prepares the complete request before the first store call.
// Identity is a versioned SHA-256 digest over length-prefixed source/recipe
// identities plus normalized paths and declared sizes; source tokens are not
// included. The injected store remains responsible for all native safety and
// atomicity guarantees.
[[nodiscard]] PayloadStageResult stage_payload(
    const PayloadStageRequest& request, PayloadSource& source,
    platform::AtomicDirectoryStore& store);

}  // namespace ohl::media
