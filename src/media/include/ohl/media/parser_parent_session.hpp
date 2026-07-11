#pragma once

#include "ohl/media/parser_parent_handshake.hpp"

#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <optional>
#include <span>

namespace ohl::media {

enum class ParserParentSessionError : std::uint8_t {
  none,
  invalid_configuration,
  invalid_state,
  concurrent_operation,
  output_too_small,
  overlapping_buffers,
  request_id_exhausted,
  allocation_failure,
  protocol_failure,
  channel_failure,
  result_failure,
  source_failure,
  worker_failure,
  source_invalidated,
  internal_failure,
};

enum class ParserParentSessionState : std::uint8_t {
  idle,
  enumerating,
  streaming,
  cancelling,
  cancelled,
  closed,
  terminal,
};

enum class ParserParentSessionDisposition : std::uint8_t {
  unavailable,
  request_sent,
  progress,
  enumeration_complete,
  stream_complete,
  read_replied,
  read_ignored_after_cancel,
  cancellation_acknowledged,
  shutdown_sent,
};

struct ParserParentSessionResult {
  ParserParentSessionError error{ParserParentSessionError::none};
  ParserParentSessionDisposition disposition{
      ParserParentSessionDisposition::unavailable};
  std::uint64_t request_id{0};
  parser::ProtocolError protocol_error{parser::ProtocolError::none};
  ParserFrameChannelResult channel_result;
  ParserResultSessionResult session_result;
  ParserSourceReadBrokerResult source_result;

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserParentSessionError::none;
  }
};

class ParserParentSession;

struct ParserParentSessionCreateResult {
  ParserParentSessionResult result;
  std::unique_ptr<ParserParentSession> session;

  [[nodiscard]] bool valid() const noexcept {
    return result.valid() && session != nullptr;
  }
};

// Disconnected trusted parent composition for one handshaken worker lifetime.
// It owns result/source-read protocol state but borrows the exact frame channel.
// The channel must outlive this object and every active call, and the caller
// must not use it directly for protocol I/O while this object exists.
// Destruction must not race a call. No method or destruction may be initiated
// from a supplied sink callback. Frame-channel callbacks may re-enter only
// terminal(), state(), result(), and catalog(). All mutating methods,
// notify_worker_failed(), invalidate_source(), and destruction are prohibited
// from frame-channel callbacks.
class ParserParentSession final {
 public:
  ~ParserParentSession();

  ParserParentSession(const ParserParentSession&) = delete;
  ParserParentSession& operator=(const ParserParentSession&) = delete;
  ParserParentSession(ParserParentSession&&) = delete;
  ParserParentSession& operator=(ParserParentSession&&) = delete;

  [[nodiscard]] ParserParentSessionResult begin_enumeration(
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // The sink is retained through stream completion, cancellation
  // acknowledgement, or terminal failure. It must outlive that interval,
  // remain synchronous/noexcept, and must not re-enter or destroy this object.
  [[nodiscard]] ParserParentSessionResult begin_stream(
      ParserResultCatalogGeneration generation, std::uint64_t source_token,
      PayloadByteSink& sink,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Receives and synchronously consumes exactly one worker frame for an active
  // operation. Receiving while idle is rejected so a worker cannot pre-send a
  // guessed next-request result. All three caller buffers must meet the
  // documented maximum capacities and be pairwise disjoint before any channel
  // bytes are consumed. No frame or payload view escapes this call. Once
  // receive I/O begins, storage may contain an attacker-controlled partial or
  // full prefix followed by a stale suffix; the whole receive buffer remains
  // invalid until caller reinitialization. Any scratch prefix written for a
  // source read is scrubbed before return. Reply storage is not scrubbed, may
  // retain private source bytes, and must be sanitized before logging or reuse
  // outside the private transport path.
  [[nodiscard]] ParserParentSessionResult receive_one(
      std::span<std::byte> receive_storage,
      std::span<std::byte> read_scratch,
      std::span<std::byte> reply_storage,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  [[nodiscard]] ParserParentSessionResult request_cancel(
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Shutdown requires no receive call to be active. Sending shutdown does not
  // close, terminate, wait for, or reap the borrowed worker/channel.
  [[nodiscard]] ParserParentSessionResult shutdown(
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Trusted out-of-band lifecycle notifications. They retire all authority
  // and interrupt active channel I/O but own no process lifecycle operation.
  void notify_worker_failed() noexcept;
  void invalidate_source() noexcept;

  [[nodiscard]] bool terminal() const noexcept;
  [[nodiscard]] ParserParentSessionState state() const noexcept;
  [[nodiscard]] ParserParentSessionResult result() const noexcept;

  // The returned view aliases result-session storage. The caller must prevent
  // concurrent mutation and obey the underlying catalog invalidation contract.
  [[nodiscard]] std::optional<ParserResultCatalogView> catalog()
      const noexcept;

 private:
  enum class ActiveOperation : std::uint8_t {
    none,
    enumeration,
    stream,
  };

  enum class OutboundTransaction : std::uint8_t {
    none,
    begin_enumeration,
    begin_stream,
    cancel,
    shutdown,
    read_reply,
  };

  ParserParentSession(
      ParserFrameChannel& channel, const ValidatedMedia& media,
      parser::ProtocolStateValidator&& protocol, std::uint64_t worker_epoch,
      PayloadImportLimits import_limits,
      ParserSourceReadLimits source_read_limits) noexcept;

  [[nodiscard]] ParserParentSessionResult success_locked(
      ParserParentSessionDisposition disposition,
      std::uint64_t request_id = 0) const noexcept;
  [[nodiscard]] ParserParentSessionResult transient_locked(
      ParserParentSessionError error) const noexcept;
  [[nodiscard]] ParserParentSessionResult fail_channel_locked(
      ParserFrameChannelResult channel_result,
      ParserSourceReadBrokerResult source_result = {}) noexcept;
  [[nodiscard]] ParserParentSessionResult fail_protocol_locked(
      parser::ProtocolError protocol_error) noexcept;
  [[nodiscard]] ParserParentSessionResult fail_result_locked(
      ParserResultSessionResult session_result) noexcept;
  [[nodiscard]] ParserParentSessionResult fail_source_locked(
      ParserSourceReadBrokerResult source_result) noexcept;
  [[nodiscard]] bool allocate_request_id_locked(
      std::uint64_t& request_id) noexcept;
  void begin_outbound_locked(OutboundTransaction transaction,
                             std::uint64_t request_id) noexcept;
  void end_outbound_locked() noexcept;
  [[nodiscard]] std::uint64_t current_request_id_locked() const noexcept;
  [[nodiscard]] ParserParentSessionResult abort_after_unlock(
      std::unique_lock<std::mutex>& lock,
      ParserParentSessionResult result) noexcept;
  void clear_operation_locked(ParserParentSessionState state) noexcept;
  void retain_failure_locked(ParserParentSessionResult failure) noexcept;

  ParserFrameChannel& channel_;
  mutable std::mutex transaction_mutex_;
  std::condition_variable transaction_condition_;
  ParserResultSession result_session_;
  ParserSourceReadBroker source_reads_;
  ParserSourceReadLimits source_read_limits_;
  ParserParentSessionResult failure_;
  ParserParentSessionState state_{ParserParentSessionState::idle};
  ActiveOperation active_operation_{ActiveOperation::none};
  std::uint64_t active_request_id_{0};
  std::uint64_t next_request_id_{1};
  bool request_ids_exhausted_{false};
  bool receiver_active_{false};
  OutboundTransaction outbound_transaction_{OutboundTransaction::none};
  std::uint64_t outbound_request_id_{0};
  PayloadByteSink* stream_sink_{nullptr};

  friend ParserParentSessionCreateResult create_parser_parent_session(
      ParserParentHandshakeProof&& proof, ParserFrameChannel& channel,
      const ValidatedMedia& media, std::uint64_t worker_epoch,
      PayloadImportLimits import_limits) noexcept;
};

// Consumes proof only after all trusted configuration and channel bindings
// validate. The same ValidatedMedia used for handshake must be supplied.
[[nodiscard]] ParserParentSessionCreateResult create_parser_parent_session(
    ParserParentHandshakeProof&& proof, ParserFrameChannel& channel,
    const ValidatedMedia& media, std::uint64_t worker_epoch,
    PayloadImportLimits import_limits = {}) noexcept;

}  // namespace ohl::media
