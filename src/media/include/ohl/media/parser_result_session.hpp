#pragma once

#include "ohl/media/payload_layout.hpp"
#include "ohl/media/payload_stream.hpp"
#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <vector>

namespace ohl::media {

enum class ParserResultSessionError {
  none,
  invalid_configuration,
  invalid_state,
  protocol_failure,
  allocation_failure,
  internal_failure,
  result_validation_failure,
  unknown_source_token,
  downstream_failure,
  incomplete_stream,
  generation_exhausted,
  source_invalidated,
  source_read_failure,
  worker_failure,
};

struct ParserResultSessionResult {
  ParserResultSessionError error{ParserResultSessionError::none};
  parser::ProtocolError protocol_error{parser::ProtocolError::none};
  PayloadLayoutError layout_error{PayloadLayoutError::none};
  PayloadPathError path_error{PayloadPathError::none};
  std::optional<std::size_t> rejected_entry;

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserResultSessionError::none;
  }
};

enum class ParserReadRequestDisposition {
  unavailable,
  serviceable,
  ignored_after_cancel,
};

struct ParserReadRequestResult {
  ParserResultSessionResult result;
  ParserReadRequestDisposition disposition{
      ParserReadRequestDisposition::unavailable};
  // Populated only for serviceable requests. Ignored and failed requests
  // deliberately expose the default message so no offset can be acted upon.
  parser::ReadRequestMessage message;

  [[nodiscard]] bool valid() const noexcept { return result.valid(); }
  [[nodiscard]] bool serviceable() const noexcept {
    return valid() &&
           disposition == ParserReadRequestDisposition::serviceable;
  }
};

struct ParserResultCatalogGeneration {
  // The trusted caller assigns a nonzero epoch that is unique across every
  // worker/session lifetime in which a catalog handle could remain reachable.
  std::uint64_t worker_epoch{0};
  std::uint64_t enumeration{0};

  [[nodiscard]] bool valid() const noexcept {
    return worker_epoch != 0 && enumeration != 0;
  }

  friend bool operator==(const ParserResultCatalogGeneration&,
                         const ParserResultCatalogGeneration&) = default;
};

struct ParserResultCatalogView {
  // This identity never comes from the worker. It must accompany later stream
  // requests so stale catalog users are rejected even when a restarted worker
  // reuses the same source token and enumeration number.
  ParserResultCatalogGeneration generation;
  // Aliases storage owned by ParserResultSession. Any new enumeration,
  // cancellation, failure, source invalidation, shutdown, worker retirement,
  // or destruction invalidates this view.
  std::span<const PlannedPayloadEntry> entries;
  std::uint64_t total_bytes{0};
};

// Owns and validates worker result metadata without creating a transport,
// reading a source, opening a destination, staging files, or publishing data.
// The supplied protocol validator must already be in its idle state after a
// completely typed and validated handshake. This object then becomes the sole
// owner of that validator.
class ParserResultSession final {
 public:
  explicit ParserResultSession(
      parser::ProtocolStateValidator&& protocol,
      std::uint64_t worker_epoch,
      PayloadImportLimits limits = {}) noexcept;

  ParserResultSession(const ParserResultSession&) = delete;
  ParserResultSession& operator=(const ParserResultSession&) = delete;
  ParserResultSession(ParserResultSession&&) = delete;
  ParserResultSession& operator=(ParserResultSession&&) = delete;

  [[nodiscard]] ParserResultSessionResult begin_enumeration(
      const parser::FrameView& frame) noexcept;
  [[nodiscard]] ParserResultSessionResult accept_entry_batch(
      const parser::FrameView& frame) noexcept;
  [[nodiscard]] ParserResultSessionResult complete_enumeration(
      const parser::FrameView& frame) noexcept;

  [[nodiscard]] ParserResultSessionResult begin_stream_entry(
      const parser::FrameView& frame,
      ParserResultCatalogGeneration expected_generation) noexcept;
  [[nodiscard]] ParserResultSessionResult accept_data_chunk(
      const parser::FrameView& frame, PayloadByteSink& destination) noexcept;
  [[nodiscard]] ParserResultSessionResult complete_stream(
      const parser::FrameView& frame) noexcept;

  // These methods validate and order source-read messages only. They do not
  // read a source or grant the decoded offset any source authority.
  [[nodiscard]] ParserReadRequestResult accept_read_request(
      const parser::FrameView& frame, const parser::SourceReadPolicy& policy,
      std::uint32_t expected_sequence) noexcept;
  [[nodiscard]] ParserResultSessionResult accept_read_reply(
      const parser::FrameView& frame, std::uint32_t expected_sequence,
      std::uint32_t requested_length) noexcept;

  [[nodiscard]] ParserResultSessionResult accept_cancel(
      const parser::FrameView& frame) noexcept;
  [[nodiscard]] ParserResultSessionResult accept_cancel_ack(
      const parser::FrameView& frame) noexcept;
  [[nodiscard]] ParserResultSessionResult accept_shutdown(
      const parser::FrameView& frame) noexcept;

  // These are trusted out-of-band lifecycle events. Both are terminal and
  // immediately retire every catalog, candidate, and stream binding.
  void invalidate_source() noexcept;
  void worker_failed() noexcept;

  [[nodiscard]] bool terminal() const noexcept { return terminal_; }
  [[nodiscard]] ParserResultSessionResult result() const noexcept;
  [[nodiscard]] parser::SessionState protocol_state() const noexcept {
    return protocol_.state();
  }
  [[nodiscard]] std::optional<ParserResultCatalogView> catalog()
      const noexcept;
  [[nodiscard]] std::uint64_t remaining_stream_bytes() const noexcept {
    return remaining_stream_bytes_;
  }

 private:
  struct TokenIndexEntry {
    std::uint64_t source_token{0};
    std::size_t entry_index{0};
  };

  [[nodiscard]] ParserResultSessionResult succeed() const noexcept;
  [[nodiscard]] ParserResultSessionResult fail(
      ParserResultSessionError error,
      parser::ProtocolError protocol_error = parser::ProtocolError::none)
      noexcept;
  [[nodiscard]] ParserResultSessionResult fail_layout(
      const PayloadLayout& layout) noexcept;
  [[nodiscard]] bool configuration_valid() const noexcept;
  [[nodiscard]] const PlannedPayloadEntry* find_catalog_entry(
      std::uint64_t source_token) const noexcept;
  void retire_catalog() noexcept;
  void clear_candidate() noexcept;
  void clear_stream() noexcept;
  void retire_all() noexcept;
  void retire_for_cancel() noexcept;

  parser::ProtocolStateValidator protocol_;
  PayloadImportLimits limits_;
  ParserResultSessionResult failure_;
  bool terminal_{false};

  std::uint64_t worker_epoch_{0};
  std::uint64_t enumeration_counter_{0};
  ParserResultCatalogGeneration candidate_generation_;
  bool candidate_active_{false};
  bool candidate_promotable_{false};
  std::uint32_t remaining_entries_{0};
  std::uint64_t remaining_path_bytes_{0};
  std::uint64_t remaining_total_bytes_{0};
  bool has_previous_source_token_{false};
  std::uint64_t previous_source_token_{0};
  std::vector<PayloadEntryMetadata> candidate_entries_;

  ParserResultCatalogGeneration catalog_generation_;
  PayloadLayout catalog_;
  std::vector<TokenIndexEntry> token_index_;

  bool stream_active_{false};
  ParserResultCatalogGeneration stream_generation_;
  std::uint64_t stream_source_token_{0};
  std::uint64_t remaining_stream_bytes_{0};
};

}  // namespace ohl::media
