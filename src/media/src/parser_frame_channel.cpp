#include "ohl/media/parser_frame_channel.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <span>

namespace ohl::media {
namespace {

[[nodiscard]] platform::IsolatedWorkerIoResult read_isolated_worker(
    void* const context, const std::span<std::byte> destination,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  return static_cast<platform::IsolatedWorker*>(context)->read_exact(
      destination, deadline, cancellation);
}

[[nodiscard]] platform::IsolatedWorkerIoResult write_isolated_worker(
    void* const context, const std::span<const std::byte> source,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  return static_cast<platform::IsolatedWorker*>(context)->write_all(
      source, deadline, cancellation);
}

void abort_isolated_worker(void* const context) noexcept {
  static_cast<platform::IsolatedWorker*>(context)->abort_io();
}

[[nodiscard]] platform::IsolatedWorkerIoResult normalize_io_result(
    platform::IsolatedWorkerIoResult result,
    const std::size_t expected_bytes) noexcept {
  if (result.bytes_transferred > expected_bytes ||
      (result.error == platform::IsolatedWorkerError::none &&
       result.bytes_transferred != expected_bytes)) {
    return {.bytes_transferred = 0,
            .error = platform::IsolatedWorkerError::io_failure};
  }
  return result;
}

[[nodiscard]] ParserFrameChannelResult transport_failure(
    const platform::IsolatedWorkerIoResult result) noexcept {
  return {
      .error = ParserFrameChannelError::transport_failure,
      .protocol_error = parser::ProtocolError::none,
      .worker_error = result.error == platform::IsolatedWorkerError::none
                          ? platform::IsolatedWorkerError::io_failure
                          : result.error,
  };
}

[[nodiscard]] ParserFrameReceiveResult unavailable(
    const ParserFrameChannelResult result) noexcept {
  return {
      .result = result,
      .frame = {},
  };
}

}  // namespace

ParserFrameChannelOperations isolated_worker_frame_channel_operations(
    platform::IsolatedWorker& worker) noexcept {
  return {
      .read_exact = read_isolated_worker,
      .write_all = write_isolated_worker,
      .abort_io = abort_isolated_worker,
      .context = &worker,
  };
}

ParserFrameChannel::ParserFrameChannel(
    const std::uint64_t session_id,
    const ParserFrameChannelOperations operations) noexcept
    : session_id_{session_id}, operations_{operations} {
  static_assert(parser::kFrameHeaderBytes <=
                platform::kMaximumIsolatedWorkerIoBytes);
  static_assert(parser::kMaximumFramePayloadBytes <=
                platform::kMaximumIsolatedWorkerIoBytes);
  static_assert(parser::kMaximumFramePayloadBytes <=
                std::numeric_limits<std::uint32_t>::max());
  if (session_id_ == 0 || !operations_.valid()) {
    terminal_ = true;
    failure_.error = ParserFrameChannelError::invalid_configuration;
  }
}

ParserFrameChannelResult ParserFrameChannel::send(
    const parser::FrameHeader& header,
    const std::span<const std::byte> payload,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  ParserFrameChannelResult begin_result;
  if (!begin_operation(send_active_, begin_result)) {
    return begin_result;
  }

  parser::ProtocolError protocol_error =
      parser::validate_frame_header(header);
  if (protocol_error == parser::ProtocolError::none &&
      header.session_id != session_id_) {
    protocol_error = parser::ProtocolError::wrong_session_id;
  }
  if (protocol_error == parser::ProtocolError::none &&
      (payload.size() > parser::kMaximumFramePayloadBytes ||
       payload.size() > std::numeric_limits<std::uint32_t>::max())) {
    protocol_error = parser::ProtocolError::payload_too_large;
  }
  if (protocol_error == parser::ProtocolError::none &&
      header.payload_length != payload.size()) {
    protocol_error = parser::ProtocolError::noncanonical_value;
  }

  std::array<std::byte, parser::kFrameHeaderBytes> encoded_header{};
  if (protocol_error == parser::ProtocolError::none) {
    protocol_error = parser::encode_frame_header(header, encoded_header);
  }
  if (protocol_error != parser::ProtocolError::none) {
    const auto failed = poison({
        .error = ParserFrameChannelError::protocol_failure,
        .protocol_error = protocol_error,
        .worker_error = platform::IsolatedWorkerError::none,
    });
    return finish_operation(send_active_, failed);
  }

  auto io_result = normalize_io_result(
      operations_.write_all(operations_.context, encoded_header, deadline,
                            cancellation),
      encoded_header.size());
  if (io_result.error != platform::IsolatedWorkerError::none) {
    const auto failed = poison(transport_failure(io_result));
    return finish_operation(send_active_, failed);
  }

  if (!payload.empty()) {
    io_result = normalize_io_result(
        operations_.write_all(operations_.context, payload, deadline,
                              cancellation),
        payload.size());
    if (io_result.error != platform::IsolatedWorkerError::none) {
      const auto failed = poison(transport_failure(io_result));
      return finish_operation(send_active_, failed);
    }
  }

  return finish_operation(send_active_, {});
}

ParserFrameReceiveResult ParserFrameChannel::receive(
    const std::span<std::byte> payload_storage,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  ParserFrameChannelResult begin_result;
  if (!begin_operation(receive_active_, begin_result)) {
    return unavailable(begin_result);
  }
  if (payload_storage.size() < parser::kMaximumFramePayloadBytes ||
      payload_storage.data() == nullptr) {
    const auto result = finish_operation(
        receive_active_,
        {.error = ParserFrameChannelError::output_too_small});
    return unavailable(result);
  }

  std::array<std::byte, parser::kFrameHeaderBytes> header_bytes{};
  auto io_result = normalize_io_result(
      operations_.read_exact(operations_.context, header_bytes, deadline,
                             cancellation),
      header_bytes.size());
  if (io_result.error != platform::IsolatedWorkerError::none) {
    const auto failed = poison(transport_failure(io_result));
    return unavailable(finish_operation(receive_active_, failed));
  }

  const auto decoded_header = parser::decode_frame_header(header_bytes);
  auto protocol_error = decoded_header.error;
  if (protocol_error == parser::ProtocolError::none &&
      decoded_header.header.session_id != session_id_) {
    protocol_error = parser::ProtocolError::wrong_session_id;
  }
  if (protocol_error != parser::ProtocolError::none) {
    const auto failed = poison({
        .error = ParserFrameChannelError::protocol_failure,
        .protocol_error = protocol_error,
        .worker_error = platform::IsolatedWorkerError::none,
    });
    return unavailable(finish_operation(receive_active_, failed));
  }

  const auto payload = payload_storage.first(
      static_cast<std::size_t>(decoded_header.header.payload_length));
  if (!payload.empty()) {
    io_result = normalize_io_result(
        operations_.read_exact(operations_.context, payload, deadline,
                               cancellation),
        payload.size());
    if (io_result.error != platform::IsolatedWorkerError::none) {
      const auto failed = poison(transport_failure(io_result));
      return unavailable(finish_operation(receive_active_, failed));
    }
  }

  const auto result = finish_operation(receive_active_, {});
  if (!result.valid()) {
    return unavailable(result);
  }
  return {
      .result = {},
      .frame = {
          .error = parser::ProtocolError::none,
          .header = decoded_header.header,
          .payload = payload,
      },
  };
}

void ParserFrameChannel::abort() noexcept {
  static_cast<void>(poison({.error = ParserFrameChannelError::aborted}));
}

bool ParserFrameChannel::terminal() const noexcept {
  const std::scoped_lock lock{state_mutex_};
  return terminal_;
}

ParserFrameChannelResult ParserFrameChannel::result() const noexcept {
  const std::scoped_lock lock{state_mutex_};
  return terminal_ ? failure_ : ParserFrameChannelResult{};
}

bool ParserFrameChannel::begin_operation(
    bool& active, ParserFrameChannelResult& result) noexcept {
  const std::scoped_lock lock{state_mutex_};
  if (terminal_) {
    result = failure_;
    return false;
  }
  if (active) {
    result.error = ParserFrameChannelError::concurrent_operation;
    return false;
  }
  active = true;
  return true;
}

ParserFrameChannelResult ParserFrameChannel::finish_operation(
    bool& active, const ParserFrameChannelResult result) noexcept {
  const std::scoped_lock lock{state_mutex_};
  active = false;
  return terminal_ ? failure_ : result;
}

ParserFrameChannelResult ParserFrameChannel::poison(
    const ParserFrameChannelResult result) noexcept {
  bool should_abort = false;
  ParserFrameChannelResult retained;
  {
    const std::scoped_lock lock{state_mutex_};
    if (!terminal_) {
      terminal_ = true;
      failure_ = result;
      should_abort = true;
    }
    retained = failure_;
  }
  if (should_abort) {
    operations_.abort_io(operations_.context);
  }
  return retained;
}

}  // namespace ohl::media
