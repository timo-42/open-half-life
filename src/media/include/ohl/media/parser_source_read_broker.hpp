#pragma once

#include "ohl/media/iso_inspector.hpp"
#include "ohl/media/parser_result_session.hpp"

#include <cstddef>
#include <cstdint>
#include <span>

namespace ohl::media {

struct ParserSourceReadLimits {
  // This value must be identical to the maximum_read_bytes advertised by the
  // accepted typed hello for the session.
  std::uint32_t maximum_read_bytes{parser::kMaximumReadBytes};
  std::uint64_t maximum_requests{parser::kMaximumProtocolMessages / 2U};
  std::uint64_t maximum_reply_payload_bytes{
      parser::kMaximumCumulativePayloadBytes};

  [[nodiscard]] bool valid() const noexcept;
};

// Optional operation seam for deterministic fault handling. Callbacks receive
// the exact pinned source retained from ValidatedMedia; they cannot replace it
// or select a path. An all-null value selects the native MediaSource methods.
// A non-null context remains caller-owned and must outlive the broker.
struct ParserSourceReadOperations {
  using VerifyUnchanged = platform::MediaSourceError (*)(
      const platform::MediaSource& source, void* context) noexcept;
  using ReadExactAt = platform::MediaSourceError (*)(
      const platform::MediaSource& source, std::uint64_t offset,
      std::span<std::byte> destination, void* context) noexcept;

  VerifyUnchanged verify_unchanged{nullptr};
  ReadExactAt read_exact_at{nullptr};
  void* context{nullptr};
};

enum class ParserSourceReadBrokerError {
  none,
  invalid_configuration,
  invalid_state,
  reply_pending,
  output_too_small,
  overlapping_buffers,
  request_budget_exceeded,
  byte_budget_exceeded,
  sequence_exhausted,
  ticket_exhausted,
  protocol_failure,
  internal_failure,
  invalid_ticket,
  transport_abandoned,
  source_changed,
  source_read_failure,
};

enum class ParserSourceReadDisposition {
  unavailable,
  ignored_after_cancel,
  reply_ready,
};

struct ParserSourceReadReplyTicket {
  std::uint64_t value{0};

  [[nodiscard]] bool valid() const noexcept { return value != 0; }

  friend bool operator==(const ParserSourceReadReplyTicket&,
                         const ParserSourceReadReplyTicket&) = default;
};

struct ParserSourceReadBrokerResult {
  ParserSourceReadBrokerError error{ParserSourceReadBrokerError::none};
  parser::ProtocolError protocol_error{parser::ProtocolError::none};
  ParserResultSessionError session_error{ParserResultSessionError::none};
  platform::MediaSourceError source_error{platform::MediaSourceError::none};

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserSourceReadBrokerError::none;
  }
};

struct ParserSourceReadPrepareResult {
  ParserSourceReadBrokerResult result;
  ParserSourceReadDisposition disposition{
      ParserSourceReadDisposition::unavailable};
  ParserSourceReadReplyTicket ticket;
  parser::ProtocolStatus status{parser::ProtocolStatus::internal_failure};
  parser::FrameHeader reply_header;
  // Aliases caller-owned reply storage. It must remain alive and unchanged
  // until commit_reply_sent() or abandon_reply() consumes ticket. Successful
  // replies contain source bytes and must remain private rather than logged.
  std::span<const std::byte> reply_payload;

  [[nodiscard]] bool valid() const noexcept { return result.valid(); }
  [[nodiscard]] bool reply_ready() const noexcept {
    return valid() && disposition == ParserSourceReadDisposition::reply_ready;
  }
};

// Synchronously brokers bounded reads from the exact pinned source carried by
// ValidatedMedia. It neither creates a transport nor sends a frame. Calls must
// be serialized with every direct call to the associated ParserResultSession.
// The session and any caller-owned buffers must outlive their documented use.
class ParserSourceReadBroker final {
 public:
  ParserSourceReadBroker(const ValidatedMedia& media,
                         ParserResultSession& session,
                         ParserSourceReadLimits limits = {},
                         ParserSourceReadOperations operations = {}) noexcept;
  ~ParserSourceReadBroker();

  ParserSourceReadBroker(const ParserSourceReadBroker&) = delete;
  ParserSourceReadBroker& operator=(const ParserSourceReadBroker&) = delete;
  ParserSourceReadBroker(ParserSourceReadBroker&&) = delete;
  ParserSourceReadBroker& operator=(ParserSourceReadBroker&&) = delete;

  [[nodiscard]] parser::SourceReadPolicy policy() const noexcept {
    return policy_;
  }

  [[nodiscard]] ParserSourceReadPrepareResult prepare(
      const parser::FrameView& read_request,
      std::span<std::byte> read_scratch,
      std::span<std::byte> reply_payload_storage) noexcept;

  // Call only after a future transport has accepted the exact prepared header
  // and payload in full. This performs the parent-to-worker state observation.
  [[nodiscard]] ParserSourceReadBrokerResult commit_reply_sent(
      ParserSourceReadReplyTicket ticket) noexcept;

  // A failed or partial transport delivery makes session ordering unknowable
  // and therefore retires both broker and result session terminally.
  [[nodiscard]] ParserSourceReadBrokerResult abandon_reply(
      ParserSourceReadReplyTicket ticket) noexcept;

  [[nodiscard]] bool terminal() const noexcept { return terminal_; }
  [[nodiscard]] bool reply_is_pending() const noexcept { return pending_; }
  [[nodiscard]] ParserSourceReadBrokerResult result() const noexcept;
  [[nodiscard]] std::uint64_t requests_charged() const noexcept {
    return requests_charged_;
  }
  [[nodiscard]] std::uint64_t reply_payload_bytes_charged() const noexcept {
    return reply_payload_bytes_charged_;
  }

 private:
  [[nodiscard]] ParserSourceReadBrokerResult fail(
      ParserSourceReadBrokerError error,
      parser::ProtocolError protocol_error = parser::ProtocolError::none,
      ParserResultSessionError session_error =
          ParserResultSessionError::none,
      platform::MediaSourceError source_error =
          platform::MediaSourceError::none) noexcept;
  [[nodiscard]] ParserSourceReadBrokerResult transient(
      ParserSourceReadBrokerError error) const noexcept;
  void clear_pending() noexcept;

  SharedMediaSource source_;
  ParserResultSession& session_;
  ParserSourceReadLimits limits_;
  parser::SourceReadPolicy policy_;
  ParserSourceReadOperations operations_;
  ParserSourceReadBrokerResult failure_;
  bool terminal_{false};

  bool have_committed_request_{false};
  std::uint64_t committed_request_id_{0};
  std::uint32_t next_sequence_{1};
  bool sequence_exhausted_{false};
  std::uint64_t requests_charged_{0};
  std::uint64_t reply_payload_bytes_charged_{0};
  std::uint64_t ticket_counter_{0};

  bool pending_{false};
  ParserSourceReadReplyTicket pending_ticket_;
  std::uint64_t pending_request_id_{0};
  std::uint32_t pending_sequence_{0};
  std::uint32_t pending_requested_length_{0};
  parser::ProtocolStatus pending_status_{
      parser::ProtocolStatus::internal_failure};
  platform::MediaSourceError pending_source_error_{
      platform::MediaSourceError::none};
  parser::FrameHeader pending_header_;
  std::span<const std::byte> pending_payload_;
};

}  // namespace ohl::media
