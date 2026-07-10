#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <string_view>

namespace ohl::platform {

// This interface describes the operations a native, race-resistant directory
// store must eventually provide. It does not provide a filesystem backend.
// All calls are synchronous, must not throw, and receive validated individual
// path components rather than joined native paths. A store must copy any view
// it needs beyond the call; a transaction may retain begin() plan views only
// until that transaction is destroyed. Implementations must perform required
// allocation before entering noexcept operations or translate allocation
// failure to resource_exhausted; exceptions must never escape this boundary.

enum class AtomicDirectoryStoreError {
  none,
  invalid_state,
  io_failure,
  unsafe_destination,
  resource_exhausted,
  unsupported,
};

struct AtomicDirectoryEntry {
  std::span<const std::string_view> components;
  std::uint64_t size_bytes{0};
};

struct AtomicDirectoryPlan {
  std::string_view identity;
  std::span<const AtomicDirectoryEntry> entries;
};

struct AtomicDirectoryCompletion {
  std::string_view identity;
  std::uint64_t entry_count{0};
  std::uint64_t total_bytes{0};
};

enum class AtomicDirectoryProbeState {
  absent,
  matching,
  conflict,
};

struct AtomicDirectoryProbeResult {
  AtomicDirectoryProbeState state{AtomicDirectoryProbeState::absent};
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

// A write returns none only after the entire chunk has been accepted. Native
// implementations must handle partial OS writes internally. An error may leave
// partial data in private staging, which abort() must discard. The returned
// error must remain valid after this sink is destroyed.
class AtomicDirectoryByteSink {
 public:
  virtual ~AtomicDirectoryByteSink() = default;

  [[nodiscard]] virtual AtomicDirectoryStoreError write(
      std::span<const std::byte> bytes) noexcept = 0;
};

struct AtomicDirectoryOpenResult {
  std::unique_ptr<AtomicDirectoryByteSink> sink;
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

enum class AtomicDirectoryPublishState {
  published,
  destination_exists,
};

struct AtomicDirectoryPublishResult {
  AtomicDirectoryPublishState state{AtomicDirectoryPublishState::published};
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

class AtomicDirectoryTransaction {
 public:
  virtual ~AtomicDirectoryTransaction() = default;

  // Once begin() has been attempted, abort() is valid and must be called after
  // any begin or later prepublication failure. begin() never publishes.
  [[nodiscard]] virtual AtomicDirectoryStoreError begin(
      const AtomicDirectoryPlan& plan) noexcept = 0;

  // At most one returned sink may be live. seal_file() consumes and closes the
  // exact file and must not retain the sink after returning. Destroying a sink
  // instead discards the open file before abort.
  [[nodiscard]] virtual AtomicDirectoryOpenResult open_file(
      std::span<const std::string_view> components,
      std::uint64_t expected_size) noexcept = 0;
  [[nodiscard]] virtual AtomicDirectoryStoreError seal_file(
      std::unique_ptr<AtomicDirectoryByteSink> sink) noexcept = 0;

  // Completion metadata is sealed after every ordinary file and immediately
  // before the single atomic no-replace publication attempt. A publish error
  // or destination_exists leaves the final destination unchanged and staging
  // owned and abortable. published is the commit point; subsequent sync
  // failure cannot undo publication and abort must not be called. A completed
  // sync reports only completion of the backend's defined sync operation, not
  // a universal durability guarantee.
  [[nodiscard]] virtual AtomicDirectoryStoreError seal_completion(
      const AtomicDirectoryCompletion& completion) noexcept = 0;
  [[nodiscard]] virtual AtomicDirectoryPublishResult publish_no_replace()
      noexcept = 0;
  [[nodiscard]] virtual AtomicDirectoryStoreError sync_published_parent()
      noexcept = 0;

  // abort() is explicit, idempotent, and removes only staging owned by this
  // transaction. It must never remove a successfully published destination.
  [[nodiscard]] virtual AtomicDirectoryStoreError abort() noexcept = 0;
};

struct AtomicDirectoryTransactionResult {
  std::unique_ptr<AtomicDirectoryTransaction> transaction;
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

class AtomicDirectoryStore {
 public:
  virtual ~AtomicDirectoryStore() = default;

  // A matching probe means the exact safe tree and completion metadata match;
  // incomplete, extra, linked, wrong-type, or mismatched trees are conflicts.
  [[nodiscard]] virtual AtomicDirectoryProbeResult probe(
      const AtomicDirectoryPlan& plan) noexcept = 0;
  [[nodiscard]] virtual AtomicDirectoryTransactionResult create_transaction()
      noexcept = 0;
};

}  // namespace ohl::platform
