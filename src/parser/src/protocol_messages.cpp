#include "ohl/parser/protocol_messages.hpp"

#include <algorithm>
#include <limits>

namespace ohl::parser {
namespace {

[[nodiscard]] ProtocolError validate_frame(
    const FrameView& frame, const MessageType expected_type) noexcept {
  if (!frame.valid()) {
    return frame.error;
  }
  const auto header_error = validate_frame_header(frame.header);
  if (header_error != ProtocolError::none) {
    return header_error;
  }
  if (frame.payload.size() > kMaximumFramePayloadBytes) {
    return ProtocolError::payload_too_large;
  }
  if (frame.payload.size() < frame.header.payload_length) {
    return ProtocolError::truncated_payload;
  }
  if (frame.payload.size() > frame.header.payload_length) {
    return ProtocolError::trailing_bytes;
  }
  if (frame.header.type != expected_type) {
    return ProtocolError::unexpected_message;
  }
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_read_request(
    const ReadRequestMessage& message, const SourceReadPolicy& policy,
    const std::uint32_t expected_sequence) noexcept {
  if (!policy.valid() || expected_sequence == 0) {
    return ProtocolError::invalid_budget;
  }
  if (message.read_sequence == 0 ||
      message.read_sequence != expected_sequence || message.length == 0 ||
      message.length > policy.maximum_read_bytes ||
      message.offset >= policy.source_size ||
      message.length > policy.source_size - message.offset) {
    return ProtocolError::noncanonical_value;
  }
  return ProtocolError::none;
}

[[nodiscard]] bool reply_status_allowed(const ProtocolStatus status) noexcept {
  return status == ProtocolStatus::ok ||
         status == ProtocolStatus::source_changed ||
         status == ProtocolStatus::source_read_failed;
}

[[nodiscard]] ProtocolError validate_reply_context(
    const std::uint32_t expected_sequence,
    const std::uint32_t requested_length) noexcept {
  if (expected_sequence == 0 || requested_length == 0 ||
      requested_length > kMaximumReadBytes) {
    return ProtocolError::invalid_budget;
  }
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_read_reply(
    const ReadReplyMessage& message, const std::uint32_t expected_sequence,
    const std::uint32_t requested_length) noexcept {
  const auto context_error =
      validate_reply_context(expected_sequence, requested_length);
  if (context_error != ProtocolError::none) {
    return context_error;
  }
  if (message.read_sequence == 0 ||
      message.read_sequence != expected_sequence ||
      !known_protocol_status(message.status) ||
      !reply_status_allowed(message.status)) {
    return ProtocolError::noncanonical_value;
  }
  if ((message.status == ProtocolStatus::ok &&
       message.data.size() != requested_length) ||
      (message.status != ProtocolStatus::ok && !message.data.empty())) {
    return ProtocolError::noncanonical_value;
  }
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_data_chunk(
    const DataChunkMessage& message,
    const std::uint64_t remaining_entry_bytes) noexcept {
  if (remaining_entry_bytes == 0) {
    return ProtocolError::invalid_budget;
  }
  if (message.data.empty() ||
      message.data.size() > kMaximumDataChunkBytes ||
      static_cast<std::uint64_t>(message.data.size()) >
          remaining_entry_bytes) {
    return ProtocolError::noncanonical_value;
  }
  return ProtocolError::none;
}

}  // namespace

EncodeResult encode_hello_payload(
    const HelloMessage& message,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  const SourceReadPolicy policy{message.source_size,
                                message.maximum_read_bytes};
  if (!policy.valid()) {
    result.error = ProtocolError::noncanonical_value;
    return result;
  }
  if (destination.size() < kHelloPayloadBytes) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(kHelloPayloadBytes)};
  (void)writer.write_u64(message.source_size);
  (void)writer.write_u32(message.maximum_read_bytes);
  result.bytes_written = kHelloPayloadBytes;
  return result;
}

HelloDecodeResult decode_hello_payload(const FrameView& frame) noexcept {
  HelloDecodeResult result;
  result.error = validate_frame(frame, MessageType::hello);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  HelloMessage message;
  if (!reader.read_u64(message.source_size) ||
      !reader.read_u32(message.maximum_read_bytes) || !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  if (!SourceReadPolicy{message.source_size, message.maximum_read_bytes}
           .valid()) {
    result.error = ProtocolError::noncanonical_value;
    return result;
  }
  result.message = message;
  return result;
}

EncodeResult encode_ready_payload(
    const ReadyMessage& /*message*/,
    const std::span<std::byte> /*destination*/) noexcept {
  return {};
}

ReadyDecodeResult decode_ready_payload(const FrameView& frame) noexcept {
  ReadyDecodeResult result;
  result.error = validate_frame(frame, MessageType::ready);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  if (!reader.finish()) {
    result.error = reader.error();
  }
  return result;
}

EncodeResult encode_enumerate_payload(
    const EnumerateMessage& /*message*/,
    const std::span<std::byte> /*destination*/) noexcept {
  return {};
}

EnumerateDecodeResult decode_enumerate_payload(
    const FrameView& frame) noexcept {
  EnumerateDecodeResult result;
  result.error = validate_frame(frame, MessageType::enumerate);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  if (!reader.finish()) {
    result.error = reader.error();
  }
  return result;
}

EncodeResult encode_stream_entry_payload(
    const StreamEntryMessage& message,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  if (destination.size() < kStreamEntryPayloadBytes) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(kStreamEntryPayloadBytes)};
  (void)writer.write_u64(message.source_token);
  result.bytes_written = kStreamEntryPayloadBytes;
  return result;
}

StreamEntryDecodeResult decode_stream_entry_payload(
    const FrameView& frame) noexcept {
  StreamEntryDecodeResult result;
  result.error = validate_frame(frame, MessageType::stream_entry);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  StreamEntryMessage message;
  if (!reader.read_u64(message.source_token) || !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  result.message = message;
  return result;
}

EncodeResult encode_read_request_payload(
    const ReadRequestMessage& message, const SourceReadPolicy& policy,
    const std::uint32_t expected_sequence,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  result.error = validate_read_request(message, policy, expected_sequence);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (destination.size() < kReadRequestPayloadBytes) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(kReadRequestPayloadBytes)};
  (void)writer.write_u32(message.read_sequence);
  (void)writer.write_u64(message.offset);
  (void)writer.write_u32(message.length);
  result.bytes_written = kReadRequestPayloadBytes;
  return result;
}

ReadRequestDecodeResult decode_read_request_payload(
    const FrameView& frame, const SourceReadPolicy& policy,
    const std::uint32_t expected_sequence) noexcept {
  ReadRequestDecodeResult result;
  result.error = validate_frame(frame, MessageType::read_request);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (!policy.valid() || expected_sequence == 0) {
    result.error = ProtocolError::invalid_budget;
    return result;
  }
  PayloadReader reader{frame.payload};
  ReadRequestMessage message;
  if (!reader.read_u32(message.read_sequence) ||
      !reader.read_u64(message.offset) || !reader.read_u32(message.length) ||
      !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  result.error = validate_read_request(message, policy, expected_sequence);
  if (result.error == ProtocolError::none) {
    result.message = message;
  }
  return result;
}

EncodeResult encode_read_reply_payload(
    const ReadReplyMessage& message, const std::uint32_t expected_sequence,
    const std::uint32_t requested_length,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  if (message.data.size() >
      std::numeric_limits<std::size_t>::max() - kReadReplyPrefixBytes) {
    result.error = ProtocolError::payload_too_large;
    return result;
  }
  const auto payload_size = kReadReplyPrefixBytes + message.data.size();
  if (payload_size > kMaximumFramePayloadBytes) {
    result.error = ProtocolError::payload_too_large;
    return result;
  }
  result.error =
      validate_read_reply(message, expected_sequence, requested_length);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (destination.size() < payload_size) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(payload_size)};
  (void)writer.write_u32(message.read_sequence);
  (void)writer.write_status(message.status);
  (void)writer.write_bytes(message.data);
  result.bytes_written = payload_size;
  return result;
}

ReadReplyDecodeResult decode_read_reply_payload(
    const FrameView& frame, const std::uint32_t expected_sequence,
    const std::uint32_t requested_length) noexcept {
  ReadReplyDecodeResult result;
  result.error = validate_frame(frame, MessageType::read_reply);
  if (result.error != ProtocolError::none) {
    return result;
  }
  result.error = validate_reply_context(expected_sequence, requested_length);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  ReadReplyMessage message;
  if (!reader.read_u32(message.read_sequence) ||
      !reader.read_status(message.status) ||
      !reader.read_bytes(reader.remaining(), message.data) ||
      !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  result.error =
      validate_read_reply(message, expected_sequence, requested_length);
  if (result.error == ProtocolError::none) {
    result.message = message;
  }
  return result;
}

EncodeResult encode_data_chunk_payload(
    const DataChunkMessage& message,
    const std::uint64_t remaining_entry_bytes,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  result.error = validate_data_chunk(message, remaining_entry_bytes);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (destination.size() < message.data.size()) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(message.data.size())};
  (void)writer.write_bytes(message.data);
  result.bytes_written = message.data.size();
  return result;
}

DataChunkDecodeResult decode_data_chunk_payload(
    const FrameView& frame,
    const std::uint64_t remaining_entry_bytes) noexcept {
  DataChunkDecodeResult result;
  result.error = validate_frame(frame, MessageType::data_chunk);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  DataChunkMessage message;
  if (!reader.read_bytes(reader.remaining(), message.data) ||
      !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  result.error = validate_data_chunk(message, remaining_entry_bytes);
  if (result.error == ProtocolError::none) {
    result.message = message;
  }
  return result;
}

EncodeResult encode_cancel_payload(
    const CancelMessage& /*message*/,
    const std::span<std::byte> /*destination*/) noexcept {
  return {};
}

CancelDecodeResult decode_cancel_payload(const FrameView& frame) noexcept {
  CancelDecodeResult result;
  result.error = validate_frame(frame, MessageType::cancel);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  if (!reader.finish()) {
    result.error = reader.error();
  }
  return result;
}

EncodeResult encode_cancel_ack_payload(
    const CancelAckMessage& /*message*/,
    const std::span<std::byte> /*destination*/) noexcept {
  return {};
}

CancelAckDecodeResult decode_cancel_ack_payload(
    const FrameView& frame) noexcept {
  CancelAckDecodeResult result;
  result.error = validate_frame(frame, MessageType::cancel_ack);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  if (!reader.finish()) {
    result.error = reader.error();
  }
  return result;
}

EncodeResult encode_shutdown_payload(
    const ShutdownMessage& /*message*/,
    const std::span<std::byte> /*destination*/) noexcept {
  return {};
}

ShutdownDecodeResult decode_shutdown_payload(
    const FrameView& frame) noexcept {
  ShutdownDecodeResult result;
  result.error = validate_frame(frame, MessageType::shutdown);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  if (!reader.finish()) {
    result.error = reader.error();
  }
  return result;
}

}  // namespace ohl::parser
