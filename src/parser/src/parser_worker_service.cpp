#include "parser_worker_service_internal.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <span>

namespace ohl::parser::detail {
namespace {

struct ReceivedFrame {
  ParserWorkerIoStatus io_status{ParserWorkerIoStatus::ok};
  ProtocolError protocol_error{ProtocolError::none};
  FrameView frame;

  [[nodiscard]] bool valid() const noexcept {
    return io_status == ParserWorkerIoStatus::ok &&
           protocol_error == ProtocolError::none && frame.valid();
  }
};

[[nodiscard]] bool ranges_overlap(const void* const first_data,
                                  const std::size_t first_size,
                                  const void* const second_data,
                                  const std::size_t second_size) noexcept {
  if (first_size == 0U || second_size == 0U) {
    return false;
  }
  if (first_data == nullptr || second_data == nullptr) {
    return true;
  }
  const auto first = reinterpret_cast<std::uintptr_t>(first_data);
  const auto second = reinterpret_cast<std::uintptr_t>(second_data);
  if (first_size > std::numeric_limits<std::uintptr_t>::max() - first ||
      second_size > std::numeric_limits<std::uintptr_t>::max() - second) {
    return true;
  }
  return first < second + second_size && second < first + first_size;
}

[[nodiscard]] bool valid_buffers(
    const ParserWorkerServiceBuffers buffers) noexcept {
  return buffers.receive_payload.data() != nullptr &&
         buffers.send_payload.data() != nullptr &&
         buffers.receive_payload.size() >= kMaximumFramePayloadBytes &&
         buffers.send_payload.size() >= kMaximumFramePayloadBytes &&
         !ranges_overlap(buffers.receive_payload.data(),
                         buffers.receive_payload.size(),
                         buffers.send_payload.data(), buffers.send_payload.size());
}

class ParserWorkerService final {
 public:
  ParserWorkerService(const ParserWorkerTransportOperations transport,
                      const ParserWorkerDispatchOperations dispatch,
                      const ParserWorkerServiceBuffers buffers,
                      const ParserWorkerServiceLimits limits) noexcept
      : transport_{transport},
        dispatch_{dispatch},
        buffers_{buffers},
        limits_{limits} {}

  [[nodiscard]] ParserWorkerServiceResult run() noexcept {
    if (!transport_.valid() || !dispatch_.valid() || !valid_buffers(buffers_) ||
        !limits_.valid() || limits_.protocol_budgets.maximum_messages < 2U ||
        limits_.protocol_budgets.maximum_payload_bytes < kHelloPayloadBytes) {
      return fail(ParserWorkerServiceError::invalid_configuration);
    }

    const ReceivedFrame hello_frame = receive_frame(0U);
    if (!hello_frame.valid()) {
      return fail_received(hello_frame);
    }
    const auto hello = decode_hello_payload(hello_frame.frame);
    if (!hello.valid()) {
      return fail_protocol(hello.error);
    }

    session_id_ = hello_frame.frame.header.session_id;
    source_policy_ = {
        .source_size = hello.message.source_size,
        .maximum_read_bytes = hello.message.maximum_read_bytes,
    };
    protocol_ = ProtocolStateValidator{session_id_, limits_.protocol_budgets};
    auto protocol_error = protocol_.observe(
        MessageDirection::parent_to_worker, hello_frame.frame.header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }

    const FrameHeader ready_header{
        .type = MessageType::ready,
        .payload_length = 0U,
        .session_id = session_id_,
        .request_id = 0U,
    };
    const auto ready_result = send_frame(ready_header, {});
    if (ready_result.error != ParserWorkerServiceError::none) {
      return ready_result;
    }

    while (true) {
      const auto state = protocol_.state();
      if (state == SessionState::enumerating ||
          state == SessionState::streaming) {
        const auto active_result = run_active_operation();
        if (active_result.error != ParserWorkerServiceError::none) {
          return active_result;
        }
        continue;
      }

      const ReceivedFrame received = receive_frame(session_id_);
      if (!received.valid()) {
        return fail_received(received);
      }
      const auto handled = handle_parent_frame(received.frame);
      if (handled.error != ParserWorkerServiceError::none ||
          handled.clean_shutdown) {
        return handled;
      }
    }
  }

 private:
  [[nodiscard]] ReceivedFrame receive_frame(
      const std::uint64_t expected_session_id) noexcept {
    std::array<std::byte, kFrameHeaderBytes> header_bytes{};
    auto io_status = transport_.read_exact(transport_.context, header_bytes);
    if (io_status != ParserWorkerIoStatus::ok) {
      return {.io_status = io_status,
              .protocol_error = ProtocolError::none,
              .frame = {}};
    }

    const auto decoded_header = decode_frame_header(header_bytes);
    if (!decoded_header.valid()) {
      return {.io_status = ParserWorkerIoStatus::ok,
              .protocol_error = decoded_header.error,
              .frame = {}};
    }
    if (expected_session_id != 0U &&
        decoded_header.header.session_id != expected_session_id) {
      return {.io_status = ParserWorkerIoStatus::ok,
              .protocol_error = ProtocolError::wrong_session_id,
              .frame = {}};
    }

    const auto payload = buffers_.receive_payload.first(
        static_cast<std::size_t>(decoded_header.header.payload_length));
    if (!payload.empty()) {
      io_status = transport_.read_exact(transport_.context, payload);
      if (io_status != ParserWorkerIoStatus::ok) {
        return {.io_status = io_status,
                .protocol_error = ProtocolError::none,
                .frame = {}};
      }
    }
    return {
        .frame = {
            .error = ProtocolError::none,
            .header = decoded_header.header,
            .payload = payload,
        },
    };
  }

  [[nodiscard]] ParserWorkerServiceResult send_frame(
      const FrameHeader& header,
      const std::span<const std::byte> payload) noexcept {
    if (payload.size() != header.payload_length ||
        payload.size() > kMaximumFramePayloadBytes) {
      return fail_protocol(ProtocolError::noncanonical_value);
    }
    const auto protocol_error =
        protocol_.observe(MessageDirection::worker_to_parent, header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }

    std::array<std::byte, kFrameHeaderBytes> header_bytes{};
    const auto encoded = encode_frame_header(header, header_bytes);
    if (encoded != ProtocolError::none) {
      return fail_protocol(encoded);
    }
    auto io_status = transport_.write_all(transport_.context, header_bytes);
    if (io_status != ParserWorkerIoStatus::ok) {
      return fail_transport(io_status);
    }
    if (!payload.empty()) {
      io_status = transport_.write_all(transport_.context, payload);
      if (io_status != ParserWorkerIoStatus::ok) {
        return fail_transport(io_status);
      }
    }
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult handle_parent_frame(
      const FrameView& frame) noexcept {
    switch (frame.header.type) {
      case MessageType::enumerate:
        return begin_operation(frame, ParserWorkerOperation::enumerate);
      case MessageType::stream_entry:
        return begin_operation(frame, ParserWorkerOperation::stream);
      case MessageType::cancel:
        return handle_cancel(frame);
      case MessageType::read_reply:
        return handle_late_read_reply(frame);
      case MessageType::shutdown:
        return handle_shutdown(frame);
      default:
        return fail_protocol(ProtocolError::unexpected_message);
    }
  }

  [[nodiscard]] ParserWorkerServiceResult begin_operation(
      const FrameView& frame, const ParserWorkerOperation operation) noexcept {
    std::uint64_t source_token = 0U;
    if (operation == ParserWorkerOperation::enumerate) {
      const auto decoded = decode_enumerate_payload(frame);
      if (!decoded.valid()) {
        return fail_protocol(decoded.error);
      }
    } else {
      const auto decoded = decode_stream_entry_payload(frame);
      if (!decoded.valid()) {
        return fail_protocol(decoded.error);
      }
      source_token = decoded.message.source_token;
    }

    const auto protocol_error =
        protocol_.observe(MessageDirection::parent_to_worker, frame.header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }

    const auto begun =
        dispatch_.begin(dispatch_.context, operation, source_token, source_policy_);
    if (begun.status != ParserWorkerDispatchStatus::ok) {
      return fail_dispatch(begun.status);
    }
    if ((operation == ParserWorkerOperation::enumerate &&
         begun.stream_size != 0U) ||
        (operation == ParserWorkerOperation::stream &&
         begun.stream_size > kMaximumEnumeratedEntryBytes)) {
      dispatch_.cancel(dispatch_.context);
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value,
                  ParserWorkerIoStatus::ok,
                  ParserWorkerDispatchStatus::failed);
    }

    dispatch_active_ = true;
    active_operation_ = operation;
    active_request_id_ = frame.header.request_id;
    remaining_stream_bytes_ = begun.stream_size;
    entry_policy_ = {
        .remaining_entries = kMaximumEnumeratedEntries,
        .remaining_path_bytes = kMaximumEnumeratedPathBytes,
        .maximum_entry_bytes = kMaximumEnumeratedEntryBytes,
        .remaining_total_bytes = kMaximumEnumeratedTotalBytes,
    };
    next_read_sequence_ = 1U;
    pending_read_ = false;
    late_read_reply_ = false;
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult run_active_operation() noexcept {
    const auto input = transport_.probe_input(transport_.context);
    if (input == ParserWorkerInputStatus::peer_closed) {
      return fail_transport(ParserWorkerIoStatus::peer_closed);
    }
    if (input == ParserWorkerInputStatus::failed) {
      return fail_transport(ParserWorkerIoStatus::failed);
    }
    if (input == ParserWorkerInputStatus::available) {
      const ReceivedFrame received = receive_frame(session_id_);
      if (!received.valid()) {
        return fail_received(received);
      }
      if (received.frame.header.type != MessageType::cancel) {
        return fail_protocol(ProtocolError::unexpected_message);
      }
      return handle_cancel(received.frame);
    }
    if (input != ParserWorkerInputStatus::unavailable) {
      return fail(ParserWorkerServiceError::internal_failure);
    }

    if (dispatch_steps_ >= limits_.maximum_dispatch_steps) {
      return fail(ParserWorkerServiceError::dispatch_budget_exceeded);
    }
    ++dispatch_steps_;
    ParserWorkerDispatchStep step;
    const auto status = dispatch_.step(dispatch_.context, step);
    if (status != ParserWorkerDispatchStatus::ok) {
      return fail_dispatch(status);
    }

    switch (step.kind) {
      case ParserWorkerDispatchStepKind::need_read:
        return send_read_request(step);
      case ParserWorkerDispatchStepKind::entry_batch:
        return send_entry_batch(step);
      case ParserWorkerDispatchStepKind::data_chunk:
        return send_data_chunk(step);
      case ParserWorkerDispatchStepKind::complete:
        return send_complete(step);
    }
    return fail(ParserWorkerServiceError::dispatch_failure);
  }

  [[nodiscard]] bool unused_step_fields_empty(
      const ParserWorkerDispatchStep& step,
      const bool allow_read,
      const bool allow_entries,
      const bool allow_data) const noexcept {
    return (allow_read ||
            (step.read_offset == 0U && step.read_length == 0U)) &&
           (allow_entries || step.entries.empty()) &&
           (allow_data || step.data.empty());
  }

  [[nodiscard]] ParserWorkerServiceResult send_read_request(
      const ParserWorkerDispatchStep& step) noexcept {
    if (!unused_step_fields_empty(step, true, false, false) || pending_read_ ||
        step.read_length == 0U ||
        step.read_length > source_policy_.maximum_read_bytes ||
        step.read_offset >= source_policy_.source_size ||
        step.read_length > source_policy_.source_size - step.read_offset ||
        next_read_sequence_ == 0U) {
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value);
    }

    const ReadRequestMessage message{
        .read_sequence = next_read_sequence_,
        .offset = step.read_offset,
        .length = step.read_length,
    };
    const auto encoded = encode_read_request_payload(
        message, source_policy_, next_read_sequence_, buffers_.send_payload);
    if (!encoded.valid()) {
      return fail(ParserWorkerServiceError::dispatch_failure, encoded.error);
    }
    const FrameHeader header{
        .type = MessageType::read_request,
        .payload_length = static_cast<std::uint32_t>(encoded.bytes_written),
        .session_id = session_id_,
        .request_id = active_request_id_,
    };
    pending_read_ = true;
    pending_read_sequence_ = next_read_sequence_;
    pending_read_length_ = step.read_length;
    const auto sent = send_frame(
        header, buffers_.send_payload.first(encoded.bytes_written));
    if (sent.error != ParserWorkerServiceError::none) {
      return sent;
    }
    return await_read_reply_or_cancel();
  }

  [[nodiscard]] ParserWorkerServiceResult await_read_reply_or_cancel() noexcept {
    const ReceivedFrame received = receive_frame(session_id_);
    if (!received.valid()) {
      return fail_received(received);
    }
    if (received.frame.header.type == MessageType::cancel) {
      return handle_cancel(received.frame);
    }
    if (received.frame.header.type != MessageType::read_reply) {
      return fail_protocol(ProtocolError::unexpected_message);
    }
    return accept_read_reply(received.frame, false);
  }

  [[nodiscard]] ParserWorkerServiceResult accept_read_reply(
      const FrameView& frame, const bool discard) noexcept {
    if (!pending_read_) {
      return fail_protocol(ProtocolError::no_read_in_flight);
    }
    const auto decoded = decode_read_reply_payload(
        frame, pending_read_sequence_, pending_read_length_);
    if (!decoded.valid()) {
      return fail_protocol(decoded.error);
    }
    const auto protocol_error =
        protocol_.observe(MessageDirection::parent_to_worker, frame.header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }

    pending_read_ = false;
    late_read_reply_ = false;
    if (pending_read_sequence_ == std::numeric_limits<std::uint32_t>::max()) {
      next_read_sequence_ = 0U;
    } else {
      next_read_sequence_ = pending_read_sequence_ + 1U;
    }
    pending_read_sequence_ = 0U;
    pending_read_length_ = 0U;
    if (discard) {
      return current_result();
    }
    if (decoded.message.status != ProtocolStatus::ok) {
      return fail(ParserWorkerServiceError::source_failure);
    }
    const auto status =
        dispatch_.accept_read_reply(dispatch_.context, decoded.message);
    if (status != ParserWorkerDispatchStatus::ok) {
      return fail_dispatch(status);
    }
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult send_entry_batch(
      const ParserWorkerDispatchStep& step) noexcept {
    if (active_operation_ != ParserWorkerOperation::enumerate ||
        !unused_step_fields_empty(step, false, true, false) ||
        step.entries.empty() ||
        step.entries.size() > kMaximumEntryBatchEntries ||
        step.entries.size() > entry_policy_.remaining_entries) {
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value);
    }

    // Validate every bounded scalar length before computing the span's bulk
    // size, examining any path bytes, or handing the batch to the encoder.
    for (const auto& entry : step.entries) {
      if (entry.archive_path.empty() ||
          entry.archive_path.size() > kMaximumEntryBatchPathBytes) {
        return fail(ParserWorkerServiceError::dispatch_failure,
                    ProtocolError::noncanonical_value);
      }
    }

    if (ranges_overlap(step.entries.data(), step.entries.size_bytes(),
                       buffers_.send_payload.data(),
                       buffers_.send_payload.size()) ||
        ranges_overlap(step.entries.data(), step.entries.size_bytes(),
                       buffers_.receive_payload.data(),
                       buffers_.receive_payload.size())) {
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value);
    }
    for (const auto& entry : step.entries) {
      if (ranges_overlap(entry.archive_path.data(), entry.archive_path.size(),
                         buffers_.send_payload.data(),
                         buffers_.send_payload.size()) ||
          ranges_overlap(entry.archive_path.data(), entry.archive_path.size(),
                         buffers_.receive_payload.data(),
                         buffers_.receive_payload.size())) {
        return fail(ParserWorkerServiceError::dispatch_failure,
                    ProtocolError::noncanonical_value);
      }
    }

    const auto encoded = encode_entry_batch_payload(
        {.entries = step.entries}, entry_policy_, buffers_.send_payload);
    if (!encoded.valid()) {
      return fail(ParserWorkerServiceError::dispatch_failure, encoded.error);
    }

    auto next_entry_policy = entry_policy_;
    for (const auto& entry : step.entries) {
      --next_entry_policy.remaining_entries;
      next_entry_policy.remaining_path_bytes -= entry.archive_path.size();
      next_entry_policy.remaining_total_bytes -= entry.size_bytes;
      next_entry_policy.has_previous_source_token = true;
      next_entry_policy.previous_source_token = entry.source_token;
    }
    const FrameHeader header{
        .type = MessageType::entry_batch,
        .payload_length = static_cast<std::uint32_t>(encoded.bytes_written),
        .session_id = session_id_,
        .request_id = active_request_id_,
    };
    const auto sent = send_frame(
        header, buffers_.send_payload.first(encoded.bytes_written));
    if (sent.error != ParserWorkerServiceError::none) {
      return sent;
    }
    entry_policy_ = next_entry_policy;
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult send_data_chunk(
      const ParserWorkerDispatchStep& step) noexcept {
    if (active_operation_ != ParserWorkerOperation::stream ||
        !unused_step_fields_empty(step, false, false, true) ||
        step.data.empty() ||
        ranges_overlap(step.data.data(), step.data.size(),
                       buffers_.send_payload.data(),
                       buffers_.send_payload.size()) ||
        ranges_overlap(step.data.data(), step.data.size(),
                       buffers_.receive_payload.data(),
                       buffers_.receive_payload.size())) {
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value);
    }
    const auto encoded = encode_data_chunk_payload(
        {.data = step.data}, remaining_stream_bytes_, buffers_.send_payload);
    if (!encoded.valid()) {
      return fail(ParserWorkerServiceError::dispatch_failure, encoded.error);
    }
    const auto next_remaining_stream_bytes =
        remaining_stream_bytes_ - step.data.size();
    const FrameHeader header{
        .type = MessageType::data_chunk,
        .payload_length = static_cast<std::uint32_t>(encoded.bytes_written),
        .session_id = session_id_,
        .request_id = active_request_id_,
    };
    const auto sent = send_frame(
        header, buffers_.send_payload.first(encoded.bytes_written));
    if (sent.error != ParserWorkerServiceError::none) {
      return sent;
    }
    remaining_stream_bytes_ = next_remaining_stream_bytes;
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult send_complete(
      const ParserWorkerDispatchStep& step) noexcept {
    if (!unused_step_fields_empty(step, false, false, false) ||
        (active_operation_ == ParserWorkerOperation::stream &&
         remaining_stream_bytes_ != 0U)) {
      return fail(ParserWorkerServiceError::dispatch_failure,
                  ProtocolError::noncanonical_value);
    }
    const auto encoded = encode_complete_payload(
        {.status = ProtocolStatus::ok, .phase = ProtocolPhase::complete},
        active_operation_ == ParserWorkerOperation::enumerate
            ? ProtocolPhase::enumerate
            : ProtocolPhase::stream,
        buffers_.send_payload);
    if (!encoded.valid()) {
      return fail(ParserWorkerServiceError::internal_failure, encoded.error);
    }
    const FrameHeader header{
        .type = MessageType::complete,
        .payload_length = static_cast<std::uint32_t>(encoded.bytes_written),
        .session_id = session_id_,
        .request_id = active_request_id_,
    };
    const auto sent = send_frame(
        header, buffers_.send_payload.first(encoded.bytes_written));
    if (sent.error != ParserWorkerServiceError::none) {
      return sent;
    }
    dispatch_.end(dispatch_.context);
    dispatch_active_ = false;
    active_request_id_ = 0U;
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult handle_cancel(
      const FrameView& frame) noexcept {
    const auto decoded = decode_cancel_payload(frame);
    if (!decoded.valid()) {
      return fail_protocol(decoded.error);
    }
    const auto prior_state = protocol_.state();
    const auto protocol_error =
        protocol_.observe(MessageDirection::parent_to_worker, frame.header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }
    if (prior_state == SessionState::idle) {
      return current_result();
    }
    if (!dispatch_active_) {
      return fail(ParserWorkerServiceError::internal_failure);
    }

    dispatch_.cancel(dispatch_.context);
    dispatch_active_ = false;
    late_read_reply_ = pending_read_;
    const FrameHeader ack_header{
        .type = MessageType::cancel_ack,
        .payload_length = 0U,
        .session_id = session_id_,
        .request_id = frame.header.request_id,
    };
    const auto sent = send_frame(ack_header, {});
    if (sent.error != ParserWorkerServiceError::none) {
      return sent;
    }
    active_request_id_ = 0U;
    return current_result();
  }

  [[nodiscard]] ParserWorkerServiceResult handle_late_read_reply(
      const FrameView& frame) noexcept {
    if (protocol_.state() != SessionState::cancelled || !late_read_reply_) {
      return fail_protocol(ProtocolError::unexpected_message);
    }
    return accept_read_reply(frame, true);
  }

  [[nodiscard]] ParserWorkerServiceResult handle_shutdown(
      const FrameView& frame) noexcept {
    const auto decoded = decode_shutdown_payload(frame);
    if (!decoded.valid()) {
      return fail_protocol(decoded.error);
    }
    const auto protocol_error =
        protocol_.observe(MessageDirection::parent_to_worker, frame.header);
    if (protocol_error != ProtocolError::none) {
      return fail_protocol(protocol_error);
    }
    pending_read_ = false;
    late_read_reply_ = false;
    transport_.close_io(transport_.context);
    auto result = current_result();
    result.clean_shutdown = true;
    return result;
  }

  [[nodiscard]] ParserWorkerServiceResult fail_received(
      const ReceivedFrame& received) noexcept {
    if (received.io_status != ParserWorkerIoStatus::ok) {
      return fail_transport(received.io_status);
    }
    return fail_protocol(received.protocol_error == ProtocolError::none
                             ? ProtocolError::unexpected_message
                             : received.protocol_error);
  }

  [[nodiscard]] ParserWorkerServiceResult fail_transport(
      const ParserWorkerIoStatus status) noexcept {
    return fail(ParserWorkerServiceError::transport_failure,
                ProtocolError::none, status);
  }

  [[nodiscard]] ParserWorkerServiceResult fail_protocol(
      const ProtocolError error) noexcept {
    return fail(ParserWorkerServiceError::protocol_failure, error);
  }

  [[nodiscard]] ParserWorkerServiceResult fail_dispatch(
      const ParserWorkerDispatchStatus status) noexcept {
    return fail(status == ParserWorkerDispatchStatus::unsupported
                    ? ParserWorkerServiceError::dispatch_unsupported
                    : ParserWorkerServiceError::dispatch_failure,
                ProtocolError::none, ParserWorkerIoStatus::ok, status);
  }

  [[nodiscard]] ParserWorkerServiceResult fail(
      const ParserWorkerServiceError error,
      const ProtocolError protocol_error = ProtocolError::none,
      const ParserWorkerIoStatus io_status = ParserWorkerIoStatus::ok,
      const ParserWorkerDispatchStatus dispatch_status =
          ParserWorkerDispatchStatus::ok) noexcept {
    if (dispatch_active_) {
      dispatch_.cancel(dispatch_.context);
      dispatch_active_ = false;
    }
    if (transport_.abort_io != nullptr && transport_.context != nullptr) {
      transport_.abort_io(transport_.context);
    }
    return {
        .error = error,
        .protocol_error = protocol_error,
        .io_status = io_status,
        .dispatch_status = dispatch_status,
        .session_id = session_id_,
        .dispatch_steps = dispatch_steps_,
        .clean_shutdown = false,
    };
  }

  [[nodiscard]] ParserWorkerServiceResult current_result() const noexcept {
    return {
        .session_id = session_id_,
        .dispatch_steps = dispatch_steps_,
    };
  }

  ParserWorkerTransportOperations transport_;
  ParserWorkerDispatchOperations dispatch_;
  ParserWorkerServiceBuffers buffers_;
  ParserWorkerServiceLimits limits_;
  ProtocolStateValidator protocol_{0U};
  SourceReadPolicy source_policy_;
  EntryBatchPolicy entry_policy_;
  ParserWorkerOperation active_operation_{ParserWorkerOperation::enumerate};
  std::uint64_t session_id_{0U};
  std::uint64_t active_request_id_{0U};
  std::uint64_t remaining_stream_bytes_{0U};
  std::uint64_t dispatch_steps_{0U};
  std::uint32_t next_read_sequence_{1U};
  std::uint32_t pending_read_sequence_{0U};
  std::uint32_t pending_read_length_{0U};
  bool dispatch_active_{false};
  bool pending_read_{false};
  bool late_read_reply_{false};
};

}  // namespace

ParserWorkerServiceResult run_parser_worker_service(
    const ParserWorkerTransportOperations transport,
    const ParserWorkerDispatchOperations dispatch,
    const ParserWorkerServiceBuffers buffers,
    const ParserWorkerServiceLimits limits) noexcept {
  ParserWorkerService service{transport, dispatch, buffers, limits};
  return service.run();
}

}  // namespace ohl::parser::detail
