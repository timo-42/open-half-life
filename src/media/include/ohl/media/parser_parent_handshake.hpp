#pragma once

#include "ohl/media/parser_frame_channel.hpp"
#include "ohl/media/parser_source_read_broker.hpp"

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>

namespace ohl::media {

enum class ParserParentHandshakeError : std::uint8_t {
  none,
  invalid_configuration,
  output_too_small,
  protocol_failure,
  channel_failure,
  internal_failure,
};

struct ParserParentHandshakeResult;

// A successful, move-only binding between the typed hello, the exact borrowed
// frame-channel object, and the protocol validator that observed it. The
// channel must outlive this proof through its consumption. This proof owns no
// source or process capability. Read the policy/limits before taking the
// validator, then construct the source broker from the same ValidatedMedia and
// these exact limits.
class ParserParentHandshakeProof final {
 public:
  ParserParentHandshakeProof(ParserParentHandshakeProof&& other) noexcept;
  ParserParentHandshakeProof& operator=(
      ParserParentHandshakeProof&& other) noexcept;

  ParserParentHandshakeProof(const ParserParentHandshakeProof&) = delete;
  ParserParentHandshakeProof& operator=(
      const ParserParentHandshakeProof&) = delete;

  [[nodiscard]] bool valid() const noexcept { return valid_; }
  [[nodiscard]] bool matches_channel(
      const ParserFrameChannel& channel) const noexcept {
    return valid_ && channel_ == &channel &&
           session_id_ == channel.session_id();
  }
  [[nodiscard]] std::uint64_t session_id() const noexcept {
    return session_id_;
  }
  [[nodiscard]] ParserSourceReadLimits source_read_limits() const noexcept {
    return source_read_limits_;
  }
  [[nodiscard]] parser::SourceReadPolicy source_read_policy() const noexcept {
    return source_read_policy_;
  }

  // Transfers the idle, already-charged validator exactly once.
  [[nodiscard]] std::optional<parser::ProtocolStateValidator>
  take_protocol() noexcept;

 private:
  ParserParentHandshakeProof(
      parser::ProtocolStateValidator&& protocol,
      const ParserFrameChannel& channel,
      std::uint64_t session_id,
      ParserSourceReadLimits source_read_limits,
      parser::SourceReadPolicy source_read_policy) noexcept;

  parser::ProtocolStateValidator protocol_;
  const ParserFrameChannel* channel_{nullptr};
  std::uint64_t session_id_{0};
  ParserSourceReadLimits source_read_limits_;
  parser::SourceReadPolicy source_read_policy_;
  bool valid_{true};

  friend struct ParserParentHandshakeResult;
  friend ParserParentHandshakeResult perform_parser_parent_handshake(
      ParserFrameChannel& channel, const ValidatedMedia& media,
      ParserSourceReadLimits source_read_limits,
      parser::ProtocolBudgets protocol_budgets,
      std::span<std::byte> receive_storage,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation) noexcept;
};

struct ParserParentHandshakeResult {
  ParserParentHandshakeError error{ParserParentHandshakeError::none};
  parser::ProtocolError protocol_error{parser::ProtocolError::none};
  ParserFrameChannelResult channel_result;
  std::optional<ParserParentHandshakeProof> proof;

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserParentHandshakeError::none && proof.has_value() &&
           proof->valid();
  }
};

// Performs exactly one synchronous parent hello/ready exchange. The caller
// must provide exclusive access to a fresh frame channel for the whole call.
// All arguments are borrowed. Receive storage is not scrubbed, and no frame or
// receive-buffer view escapes. After payload I/O begins or typed-ready
// validation fails, the storage may contain an attacker-controlled partial or
// full prefix followed by stale prior bytes. The entire buffer is untrusted and
// invalid as a frame until the caller reinitializes it. A failure after channel
// interaction terminally aborts the channel; process termination and reap
// remain the caller's responsibility.
[[nodiscard]] ParserParentHandshakeResult perform_parser_parent_handshake(
    ParserFrameChannel& channel, const ValidatedMedia& media,
    ParserSourceReadLimits source_read_limits,
    parser::ProtocolBudgets protocol_budgets,
    std::span<std::byte> receive_storage,
    std::chrono::steady_clock::time_point deadline,
    platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

}  // namespace ohl::media
