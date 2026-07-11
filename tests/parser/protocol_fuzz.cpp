#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <span>

namespace {

constexpr std::uint64_t kFuzzSession = 1;
constexpr std::size_t kRecordBytes = 4;
constexpr std::size_t kMaximumTranscriptRecords = 64;
constexpr std::size_t kFramedRecordHeaderBytes = 3;
constexpr std::uint32_t kMaximumFuzzReadBytes = 4'096;
constexpr std::uint32_t kMaximumFuzzSourceBytes = 65'536;
constexpr std::uint32_t kMaximumFuzzMismatchSequence = 64;

[[nodiscard]] std::uint32_t load_u32_context(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  if (offset > input.size() || input.size() - offset < sizeof(std::uint32_t)) {
    return 0;
  }
  std::uint32_t value = 0;
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    value |= std::to_integer<std::uint32_t>(input[offset + index])
             << static_cast<unsigned int>(index * 8U);
  }
  return value;
}

[[nodiscard]] std::uint64_t load_u64_context(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  if (offset > input.size() || input.size() - offset < sizeof(std::uint64_t)) {
    return 0;
  }
  std::uint64_t value = 0;
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    value |= std::to_integer<std::uint64_t>(input[offset + index])
             << static_cast<unsigned int>(index * 8U);
  }
  return value;
}

[[nodiscard]] std::uint16_t load_u16_context(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  if (offset > input.size() || input.size() - offset < sizeof(std::uint16_t)) {
    return 0;
  }
  std::uint16_t value = 0;
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    value |= static_cast<std::uint16_t>(
        std::to_integer<std::uint16_t>(input[offset + index])
        << static_cast<unsigned int>(index * 8U));
  }
  return value;
}

[[nodiscard]] std::uint32_t bounded_context(
    const std::span<const std::byte> input, const std::size_t offset,
    const std::uint32_t maximum) noexcept {
  std::uint32_t value = 0;
  const auto available = offset < input.size() ? input.size() - offset : 0;
  const auto bytes = std::min(available, sizeof(value));
  for (std::size_t index = 0; index < bytes; ++index) {
    value |= std::to_integer<std::uint32_t>(input[offset + index])
             << static_cast<unsigned int>(index * 8U);
  }
  return (value % maximum) + 1U;
}

struct SequenceContext {
  std::uint32_t expected_sequence{1};
  bool matches_wire{false};
};

[[nodiscard]] SequenceContext sequence_context(
    const ohl::parser::FrameView& frame) noexcept {
  const auto wire_sequence = load_u32_context(frame.payload, 0);
  if (wire_sequence != 0 && (frame.header.request_id & 1U) != 0) {
    return {.expected_sequence = wire_sequence, .matches_wire = true};
  }

  auto mismatch =
      bounded_context(frame.payload, 4, kMaximumFuzzMismatchSequence);
  if (mismatch == wire_sequence) {
    mismatch = mismatch == kMaximumFuzzMismatchSequence ? 1U : mismatch + 1U;
  }
  return {.expected_sequence = mismatch, .matches_wire = false};
}

[[nodiscard]] ohl::parser::SourceReadPolicy request_policy(
    const ohl::parser::FrameView& frame,
    const bool matching_sequence) noexcept {
  const auto fallback = ohl::parser::SourceReadPolicy{
      bounded_context(frame.payload, 4, kMaximumFuzzSourceBytes),
      bounded_context(frame.payload, 12, kMaximumFuzzReadBytes),
  };
  if (!matching_sequence) {
    return fallback;
  }

  const auto wire_offset = load_u64_context(frame.payload, 4);
  const auto wire_length = load_u32_context(frame.payload, 12);
  if (wire_length == 0 || wire_length > kMaximumFuzzReadBytes ||
      wire_offset >= kMaximumFuzzSourceBytes ||
      wire_length > kMaximumFuzzSourceBytes - wire_offset) {
    return fallback;
  }
  return {
      .source_size = wire_offset + wire_length,
      .maximum_read_bytes = wire_length,
  };
}

[[nodiscard]] std::uint32_t reply_requested_length(
    const ohl::parser::FrameView& frame,
    const bool matching_sequence) noexcept {
  if (matching_sequence &&
      frame.payload.size() >= ohl::parser::kReadReplyPrefixBytes) {
    const auto status = static_cast<ohl::parser::ProtocolStatus>(
        load_u16_context(frame.payload, sizeof(std::uint32_t)));
    const auto data_size =
        frame.payload.size() - ohl::parser::kReadReplyPrefixBytes;
    if (status == ohl::parser::ProtocolStatus::ok && data_size != 0 &&
        data_size <= kMaximumFuzzReadBytes) {
      return static_cast<std::uint32_t>(data_size);
    }
    if ((status == ohl::parser::ProtocolStatus::source_changed ||
         status == ohl::parser::ProtocolStatus::source_read_failed) &&
        data_size == 0) {
      return 1;
    }
  }
  return bounded_context(frame.payload, ohl::parser::kReadReplyPrefixBytes,
                         kMaximumFuzzReadBytes);
}

void exercise_typed_decoder(const ohl::parser::FrameView& frame) {
  using namespace ohl::parser;
  switch (frame.header.type) {
    case MessageType::hello:
      (void)decode_hello_payload(frame);
      break;
    case MessageType::ready:
      (void)decode_ready_payload(frame);
      break;
    case MessageType::enumerate:
      (void)decode_enumerate_payload(frame);
      break;
    case MessageType::stream_entry:
      (void)decode_stream_entry_payload(frame);
      break;
    case MessageType::read_request: {
      const auto sequence = sequence_context(frame);
      const auto policy = request_policy(frame, sequence.matches_wire);
      (void)decode_read_request_payload(frame, policy,
                                        sequence.expected_sequence);
      break;
    }
    case MessageType::read_reply: {
      const auto sequence = sequence_context(frame);
      const auto requested_length =
          reply_requested_length(frame, sequence.matches_wire);
      (void)decode_read_reply_payload(frame, sequence.expected_sequence,
                                      requested_length);
      break;
    }
    case MessageType::cancel:
      (void)decode_cancel_payload(frame);
      break;
    case MessageType::cancel_ack:
      (void)decode_cancel_ack_payload(frame);
      break;
    case MessageType::shutdown:
      (void)decode_shutdown_payload(frame);
      break;
    case MessageType::entry_batch:
    case MessageType::data_chunk:
    case MessageType::complete:
      break;
  }
}

[[nodiscard]] bool canonical_typed_contexts_reachable() noexcept {
  using namespace ohl::parser;
  std::array<std::byte, kReadRequestPayloadBytes> request_payload{};
  PayloadWriter request_writer{request_payload};
  if (!request_writer.write_u32(7) || !request_writer.write_u64(32) ||
      !request_writer.write_u32(16)) {
    return false;
  }
  const FrameView request_frame{
      .header = {.type = MessageType::read_request,
                 .payload_length =
                     static_cast<std::uint32_t>(request_payload.size()),
                 .session_id = kFuzzSession,
                 .request_id = 1},
      .payload = request_payload,
  };
  const auto request_sequence = sequence_context(request_frame);
  const auto request_decode = decode_read_request_payload(
      request_frame,
      request_policy(request_frame, request_sequence.matches_wire),
      request_sequence.expected_sequence);

  constexpr std::size_t kReplyDataBytes = 3;
  std::array<std::byte, kReadReplyPrefixBytes + kReplyDataBytes>
      reply_payload{};
  const std::array reply_data{
      std::byte{0xaa}, std::byte{0xbb}, std::byte{0xcc}};
  PayloadWriter reply_writer{reply_payload};
  if (!reply_writer.write_u32(9) ||
      !reply_writer.write_status(ProtocolStatus::ok) ||
      !reply_writer.write_bytes(reply_data)) {
    return false;
  }
  const FrameView reply_frame{
      .header = {.type = MessageType::read_reply,
                 .payload_length =
                     static_cast<std::uint32_t>(reply_payload.size()),
                 .session_id = kFuzzSession,
                 .request_id = 1},
      .payload = reply_payload,
  };
  const auto reply_sequence = sequence_context(reply_frame);
  const auto reply_decode = decode_read_reply_payload(
      reply_frame, reply_sequence.expected_sequence,
      reply_requested_length(reply_frame, reply_sequence.matches_wire));

  const auto request_mismatch = sequence_context(FrameView{
      .header = {.type = MessageType::read_request,
                 .payload_length =
                     static_cast<std::uint32_t>(request_payload.size()),
                 .session_id = kFuzzSession,
                 .request_id = 2},
      .payload = request_payload,
  });
  const auto reply_mismatch = sequence_context(FrameView{
      .header = {.type = MessageType::read_reply,
                 .payload_length =
                     static_cast<std::uint32_t>(reply_payload.size()),
                 .session_id = kFuzzSession,
                 .request_id = 2},
      .payload = reply_payload,
  });
  return request_sequence.matches_wire &&
         request_sequence.expected_sequence == 7 && request_decode.valid() &&
         reply_sequence.matches_wire && reply_sequence.expected_sequence == 9 &&
         reply_decode.valid() && !request_mismatch.matches_wire &&
         request_mismatch.expected_sequence != 7 &&
         !reply_mismatch.matches_wire && reply_mismatch.expected_sequence != 9;
}

[[nodiscard]] ohl::parser::MessageType message_type(
    const std::uint8_t symbol) noexcept {
  using ohl::parser::MessageType;
  switch (symbol) {
    case 'H': return MessageType::hello;
    case 'R': return MessageType::ready;
    case 'E': return MessageType::enumerate;
    case 'S': return MessageType::stream_entry;
    case 'Q': return MessageType::read_request;
    case 'Y': return MessageType::read_reply;
    case 'B': return MessageType::entry_batch;
    case 'D': return MessageType::data_chunk;
    case 'C': return MessageType::complete;
    case 'X': return MessageType::cancel;
    case 'A': return MessageType::cancel_ack;
    case 'Z': return MessageType::shutdown;
    default:
      return static_cast<MessageType>(symbol);
  }
}

void exercise_seeded_transcript(const std::span<const std::uint8_t> input) {
  using namespace ohl::parser;
  ProtocolStateValidator validator{
      kFuzzSession,
      {.maximum_messages = kMaximumTranscriptRecords,
       .maximum_payload_bytes = kMaximumTranscriptRecords * 15U}};
  const auto records =
      std::min(input.size() / kRecordBytes, kMaximumTranscriptRecords);
  for (std::size_t index = 0; index < records; ++index) {
    const auto record = input.subspan(index * kRecordBytes, kRecordBytes);
    const auto direction = record[0] == 'P'
                               ? MessageDirection::parent_to_worker
                               : MessageDirection::worker_to_parent;
    const auto type = message_type(record[1]);
    const auto request_id = static_cast<std::uint64_t>(record[2] & 0x0fU);
    const auto payload_size = static_cast<std::size_t>(record[3] & 0x0fU);
    std::array<std::byte, 15> payload{};
    std::array<std::byte, kFrameHeaderBytes + payload.size()> encoded{};
    const FrameHeader header{
        .type = type,
        .payload_length = static_cast<std::uint32_t>(payload_size),
        .session_id = kFuzzSession,
        .request_id = request_id,
    };
    const auto encode = encode_frame(
        header, std::span<const std::byte>{payload}.first(payload_size),
        encoded);
    if (!encode.valid()) {
      continue;
    }
    const auto decoded = decode_frame(
        std::span<const std::byte>{encoded}.first(encode.bytes_written),
        kFuzzSession);
    if (decoded.valid()) {
      exercise_typed_decoder(decoded);
      (void)validator.observe(direction, decoded.header);
    }
  }
}

void exercise_framed_transcript(const std::span<const std::uint8_t> input) {
  using namespace ohl::parser;
  ProtocolStateValidator validator{kFuzzSession};
  std::size_t offset = 0;
  std::size_t records = 0;
  while (records < kMaximumTranscriptRecords &&
         input.size() - offset >= kFramedRecordHeaderBytes) {
    const auto direction = (input[offset] & 1U) == 0
                               ? MessageDirection::parent_to_worker
                               : MessageDirection::worker_to_parent;
    const auto frame_size =
        static_cast<std::size_t>(input[offset + 1]) |
        (static_cast<std::size_t>(input[offset + 2]) << 8U);
    offset += kFramedRecordHeaderBytes;
    if (frame_size > input.size() - offset) {
      break;
    }
    const auto frame_bytes = std::as_bytes(input.subspan(offset, frame_size));
    const auto decoded = decode_frame(frame_bytes, kFuzzSession);
    if (decoded.valid()) {
      exercise_typed_decoder(decoded);
      (void)validator.observe(direction, decoded.header);
    }
    offset += frame_size;
    ++records;
  }
}

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const std::uint8_t* data,
                                       const std::size_t size) {
  if (data == nullptr) {
    return 0;
  }
  static const bool canonical_contexts_reachable =
      canonical_typed_contexts_reachable();
  if (!canonical_contexts_reachable) {
    std::abort();
  }
  const auto bytes = std::as_bytes(std::span{data, size});
  exercise_seeded_transcript(std::span{data, size});
  exercise_framed_transcript(std::span{data, size});
  const auto frame = ohl::parser::decode_frame(bytes);
  if (!frame.valid()) {
    return 0;
  }

  exercise_typed_decoder(frame);

  ohl::parser::PayloadReader reader{frame.payload};
  while (reader.remaining() >= sizeof(std::uint64_t)) {
    std::uint64_t value = 0;
    if (!reader.read_u64(value)) {
      break;
    }
  }
  (void)reader.finish();

  ohl::parser::ProtocolStateValidator validator{frame.header.session_id};
  const auto direction = frame.payload.empty() ||
                                 (std::to_integer<std::uint8_t>(
                                      frame.payload.front()) &
                                  1U) == 0
                             ? ohl::parser::MessageDirection::parent_to_worker
                             : ohl::parser::MessageDirection::worker_to_parent;
  (void)validator.observe(direction, frame.header);
  return 0;
}
