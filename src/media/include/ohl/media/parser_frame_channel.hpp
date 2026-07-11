#pragma once

#include "ohl/parser/protocol.hpp"
#include "ohl/platform/isolated_worker.hpp"

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <span>

namespace ohl::media {

enum class ParserFrameChannelError : std::uint8_t {
  none,
  invalid_configuration,
  concurrent_operation,
  output_too_small,
  protocol_failure,
  transport_failure,
  aborted,
};

struct ParserFrameChannelResult {
  ParserFrameChannelError error{ParserFrameChannelError::none};
  parser::ProtocolError protocol_error{parser::ProtocolError::none};
  platform::IsolatedWorkerError worker_error{
      platform::IsolatedWorkerError::none};

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserFrameChannelError::none;
  }
};

struct ParserFrameReceiveResult {
  ParserFrameChannelResult result;
  // On success, payload aliases caller-owned storage passed to receive(). The
  // storage must remain alive and unchanged while this view is in use.
  parser::FrameView frame;

  [[nodiscard]] bool valid() const noexcept { return result.valid(); }
};

// Non-owning trusted exact-I/O capability. The context and the channel it
// represents must outlive ParserFrameChannel and every active operation.
// Callbacks must support one read and one write concurrently. Successful I/O
// transfers the complete non-empty span; every other outcome is terminal. All
// callbacks must be noexcept. abort_io must be idempotent and concurrency-safe,
// may be called while either or both I/O callbacks are active, and must promptly
// cause both active callbacks to return. It must not re-enter or destroy the
// ParserFrameChannel.
struct ParserFrameChannelOperations {
  using ReadExact = platform::IsolatedWorkerIoResult (*)(
      void* context, std::span<std::byte> destination,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation) noexcept;
  using WriteAll = platform::IsolatedWorkerIoResult (*)(
      void* context, std::span<const std::byte> source,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation) noexcept;
  using AbortIo = void (*)(void* context) noexcept;

  ReadExact read_exact{nullptr};
  WriteAll write_all{nullptr};
  AbortIo abort_io{nullptr};
  void* context{nullptr};

  [[nodiscard]] bool valid() const noexcept {
    return read_exact != nullptr && write_all != nullptr &&
           abort_io != nullptr && context != nullptr;
  }
};

// Adapts an already-created worker byte channel. This grants no launch,
// termination, reap, executable-selection, or process ownership authority.
[[nodiscard]] ParserFrameChannelOperations
isolated_worker_frame_channel_operations(
    platform::IsolatedWorker& worker) noexcept;

// Canonically frames one OWP/1 session over a trusted exact byte channel. It
// owns no process or buffers. Calls may overlap only when one is send() and the
// other is receive(). Destruction must not race an active operation, and the
// caller remains responsible for orderly close or terminate-and-reap.
class ParserFrameChannel final {
 public:
  ParserFrameChannel(std::uint64_t session_id,
                     ParserFrameChannelOperations operations) noexcept;

  ParserFrameChannel(const ParserFrameChannel&) = delete;
  ParserFrameChannel& operator=(const ParserFrameChannel&) = delete;
  ParserFrameChannel(ParserFrameChannel&&) = delete;
  ParserFrameChannel& operator=(ParserFrameChannel&&) = delete;

  [[nodiscard]] ParserFrameChannelResult send(
      const parser::FrameHeader& header,
      std::span<const std::byte> payload,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Storage must have capacity for the protocol maximum. This is checked
  // before consuming a header so an untrusted length never drives allocation
  // or leaves the byte stream between frames. A pre-header capacity failure or
  // a rejected header does not begin payload mutation. Once payload I/O begins,
  // any receive failure may leave an untrusted partial prefix followed by stale
  // prior bytes in caller storage. The entire buffer is then invalid: no part
  // may be parsed or reused as a frame.
  [[nodiscard]] ParserFrameReceiveResult receive(
      std::span<std::byte> payload_storage,
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Terminally poisons the frame channel and interrupts active I/O. Repeated
  // calls have no additional effect.
  void abort() noexcept;

  [[nodiscard]] bool terminal() const noexcept;
  [[nodiscard]] ParserFrameChannelResult result() const noexcept;

 private:
  [[nodiscard]] bool begin_operation(
      bool& active, ParserFrameChannelResult& result) noexcept;
  [[nodiscard]] ParserFrameChannelResult finish_operation(
      bool& active, ParserFrameChannelResult result) noexcept;
  [[nodiscard]] ParserFrameChannelResult poison(
      ParserFrameChannelResult result) noexcept;

  std::uint64_t session_id_{0};
  ParserFrameChannelOperations operations_;
  mutable std::mutex state_mutex_;
  ParserFrameChannelResult failure_;
  bool terminal_{false};
  bool send_active_{false};
  bool receive_active_{false};
};

}  // namespace ohl::media
