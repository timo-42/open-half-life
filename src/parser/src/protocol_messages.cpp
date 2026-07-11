#include "ohl/parser/protocol_messages.hpp"

#include <algorithm>
#include <limits>

namespace ohl::parser {
namespace {

struct EntryBatchAccumulator {
  std::uint64_t path_bytes{0};
  std::uint64_t total_bytes{0};
  bool has_previous_source_token{false};
  std::uint64_t previous_source_token{0};
};

struct EntryBatchValidation {
  std::uint16_t entry_count{0};
  std::size_t payload_size{0};
};

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

[[nodiscard]] bool printable_ascii(const std::string_view value) noexcept {
  return std::ranges::all_of(value, [](const char character) {
    const auto byte = static_cast<unsigned char>(character);
    return byte >= 0x20U && byte <= 0x7eU;
  });
}

[[nodiscard]] bool printable_ascii(
    const std::span<const std::byte> value) noexcept {
  return std::ranges::all_of(value, [](const std::byte character) {
    const auto byte = std::to_integer<unsigned int>(character);
    return byte >= 0x20U && byte <= 0x7eU;
  });
}

[[nodiscard]] ProtocolError validate_entry_batch_entry(
    const std::uint64_t source_token, const std::uint64_t size_bytes,
    const std::uint64_t path_bytes, const bool path_is_printable_ascii,
    const EntryBatchPolicy& policy,
    EntryBatchAccumulator& accumulator) noexcept {
  if (path_bytes == 0 || path_bytes > kMaximumEntryBatchPathBytes ||
      !path_is_printable_ascii || size_bytes > policy.maximum_entry_bytes ||
      path_bytes > policy.remaining_path_bytes - accumulator.path_bytes ||
      size_bytes > policy.remaining_total_bytes - accumulator.total_bytes ||
      (accumulator.has_previous_source_token &&
       source_token <= accumulator.previous_source_token)) {
    return ProtocolError::noncanonical_value;
  }
  accumulator.path_bytes += path_bytes;
  accumulator.total_bytes += size_bytes;
  accumulator.has_previous_source_token = true;
  accumulator.previous_source_token = source_token;
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_entry_batch(
    const EntryBatchMessage& message, const EntryBatchPolicy& policy,
    EntryBatchValidation& validation) noexcept {
  if (!policy.valid()) {
    return ProtocolError::invalid_budget;
  }
  if (message.entries.empty() ||
      message.entries.size() > kMaximumEntryBatchEntries ||
      message.entries.size() > policy.remaining_entries) {
    return ProtocolError::noncanonical_value;
  }

  EntryBatchAccumulator accumulator{
      .has_previous_source_token = policy.has_previous_source_token,
      .previous_source_token = policy.previous_source_token,
  };
  std::size_t payload_size = kEntryBatchPrefixBytes;
  for (const auto& entry : message.entries) {
    const auto path_size = static_cast<std::uint64_t>(
        entry.archive_path.size());
    const auto entry_error = validate_entry_batch_entry(
        entry.source_token, entry.size_bytes, path_size,
        printable_ascii(entry.archive_path), policy, accumulator);
    if (entry_error != ProtocolError::none) {
      return entry_error;
    }
    payload_size += kEntryBatchEntryPrefixBytes + entry.archive_path.size();
  }
  if (payload_size > kMaximumFramePayloadBytes) {
    return ProtocolError::payload_too_large;
  }
  validation.entry_count =
      static_cast<std::uint16_t>(message.entries.size());
  validation.payload_size = payload_size;
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_entry_batch_payload(
    const std::span<const std::byte> payload,
    const EntryBatchPolicy& policy,
    EntryBatchValidation& validation) noexcept {
  PayloadReader reader{payload};
  std::uint16_t entry_count = 0;
  if (!reader.read_u16(entry_count)) {
    return reader.error();
  }
  if (entry_count == 0 || entry_count > kMaximumEntryBatchEntries ||
      entry_count > policy.remaining_entries) {
    return ProtocolError::noncanonical_value;
  }

  EntryBatchAccumulator accumulator{
      .has_previous_source_token = policy.has_previous_source_token,
      .previous_source_token = policy.previous_source_token,
  };
  for (std::uint16_t index = 0; index < entry_count; ++index) {
    std::uint64_t source_token = 0;
    std::uint64_t size_bytes = 0;
    std::uint16_t path_size = 0;
    std::span<const std::byte> path;
    if (!reader.read_u64(source_token) || !reader.read_u64(size_bytes) ||
        !reader.read_u16(path_size) || !reader.read_bytes(path_size, path)) {
      return reader.error();
    }
    const auto entry_error = validate_entry_batch_entry(
        source_token, size_bytes, path_size, printable_ascii(path), policy,
        accumulator);
    if (entry_error != ProtocolError::none) {
      return entry_error;
    }
  }
  if (!reader.finish()) {
    return reader.error();
  }
  validation.entry_count = entry_count;
  validation.payload_size = payload.size();
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

[[nodiscard]] ProtocolError validate_complete_context(
    const ProtocolPhase expected_operation_phase) noexcept {
  if (expected_operation_phase != ProtocolPhase::enumerate &&
      expected_operation_phase != ProtocolPhase::stream) {
    return ProtocolError::invalid_budget;
  }
  return ProtocolError::none;
}

[[nodiscard]] ProtocolError validate_complete(
    const CompleteMessage& message,
    const ProtocolPhase expected_operation_phase) noexcept {
  const auto context_error =
      validate_complete_context(expected_operation_phase);
  if (context_error != ProtocolError::none) {
    return context_error;
  }
  if (!known_protocol_status(message.status) ||
      !known_protocol_phase(message.phase) ||
      message.status != ProtocolStatus::ok ||
      message.phase != ProtocolPhase::complete) {
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

EncodeResult encode_entry_batch_payload(
    const EntryBatchMessage& message, const EntryBatchPolicy& policy,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  EntryBatchValidation validation;
  result.error = validate_entry_batch(message, policy, validation);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (destination.size() < validation.payload_size) {
    result.error = ProtocolError::output_too_small;
    return result;
  }

  PayloadWriter writer{destination.first(validation.payload_size)};
  (void)writer.write_u16(validation.entry_count);
  for (const auto& entry : message.entries) {
    (void)writer.write_u64(entry.source_token);
    (void)writer.write_u64(entry.size_bytes);
    (void)writer.write_u16(
        static_cast<std::uint16_t>(entry.archive_path.size()));
    (void)writer.write_bytes(std::as_bytes(std::span<const char>{
        entry.archive_path.data(), entry.archive_path.size()}));
  }
  result.bytes_written = validation.payload_size;
  return result;
}

EntryBatchDecodeResult decode_entry_batch_payload(
    const FrameView& frame, const EntryBatchPolicy& policy,
    const std::span<EntryBatchEntry> entry_storage) noexcept {
  EntryBatchDecodeResult result;
  result.error = validate_frame(frame, MessageType::entry_batch);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (!policy.valid()) {
    result.error = ProtocolError::invalid_budget;
    return result;
  }

  EntryBatchValidation validation;
  result.error =
      validate_entry_batch_payload(frame.payload, policy, validation);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (entry_storage.size() < validation.entry_count) {
    result.error = ProtocolError::output_too_small;
    return result;
  }

  // The first pass proved that every operation below succeeds. Populate
  // caller storage only after full validation and the capacity check.
  PayloadReader reader{frame.payload};
  std::uint16_t entry_count = 0;
  (void)reader.read_u16(entry_count);
  for (std::uint16_t index = 0; index < entry_count; ++index) {
    std::uint64_t source_token = 0;
    std::uint64_t size_bytes = 0;
    std::uint16_t path_size = 0;
    std::span<const std::byte> path;
    (void)reader.read_u64(source_token);
    (void)reader.read_u64(size_bytes);
    (void)reader.read_u16(path_size);
    (void)reader.read_bytes(path_size, path);
    entry_storage[index] = EntryBatchEntry{
        .source_token = source_token,
        .size_bytes = size_bytes,
        .archive_path = std::string_view{
            reinterpret_cast<const char*>(path.data()), path.size()},
    };
  }
  (void)reader.finish();
  result.message.entries = entry_storage.first(entry_count);
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

EncodeResult encode_complete_payload(
    const CompleteMessage& message,
    const ProtocolPhase expected_operation_phase,
    const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  result.error = validate_complete(message, expected_operation_phase);
  if (result.error != ProtocolError::none) {
    return result;
  }
  if (destination.size() < kCompletePayloadBytes) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  PayloadWriter writer{destination.first(kCompletePayloadBytes)};
  (void)writer.write_status(message.status);
  (void)writer.write_phase(message.phase);
  result.bytes_written = kCompletePayloadBytes;
  return result;
}

CompleteDecodeResult decode_complete_payload(
    const FrameView& frame,
    const ProtocolPhase expected_operation_phase) noexcept {
  CompleteDecodeResult result;
  result.error = validate_frame(frame, MessageType::complete);
  if (result.error != ProtocolError::none) {
    return result;
  }
  result.error = validate_complete_context(expected_operation_phase);
  if (result.error != ProtocolError::none) {
    return result;
  }
  PayloadReader reader{frame.payload};
  CompleteMessage message;
  if (!reader.read_status(message.status) ||
      !reader.read_phase(message.phase) || !reader.finish()) {
    result.error = reader.error();
    return result;
  }
  result.error = validate_complete(message, expected_operation_phase);
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
