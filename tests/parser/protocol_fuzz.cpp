#include "ohl/parser/protocol.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <span>

namespace {

constexpr std::uint64_t kFuzzSession = 1;
constexpr std::size_t kRecordBytes = 4;
constexpr std::size_t kMaximumTranscriptRecords = 64;
constexpr std::size_t kFramedRecordHeaderBytes = 3;

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
  const auto bytes = std::as_bytes(std::span{data, size});
  exercise_seeded_transcript(std::span{data, size});
  exercise_framed_transcript(std::span{data, size});
  const auto frame = ohl::parser::decode_frame(bytes);
  if (!frame.valid()) {
    return 0;
  }

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
