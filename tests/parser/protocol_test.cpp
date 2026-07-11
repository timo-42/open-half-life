#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <limits>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace {

using ohl::parser::CompleteMessage;
using ohl::parser::DataChunkMessage;
using ohl::parser::EntryBatchEntry;
using ohl::parser::EntryBatchMessage;
using ohl::parser::EntryBatchPolicy;
using ohl::parser::FrameHeader;
using ohl::parser::FrameView;
using ohl::parser::HelloMessage;
using ohl::parser::MessageDirection;
using ohl::parser::MessageType;
using ohl::parser::PayloadReader;
using ohl::parser::PayloadWriter;
using ohl::parser::ProtocolBudgets;
using ohl::parser::ProtocolError;
using ohl::parser::ProtocolPhase;
using ohl::parser::ProtocolStateValidator;
using ohl::parser::ProtocolStatus;
using ohl::parser::ReadReplyMessage;
using ohl::parser::ReadRequestMessage;
using ohl::parser::ReadyMessage;
using ohl::parser::SessionState;
using ohl::parser::SourceReadPolicy;
using ohl::parser::StreamEntryMessage;

constexpr std::uint64_t kSession = 0x0102'0304'0506'0708ULL;

[[nodiscard]] constexpr EntryBatchPolicy maximum_entry_batch_policy() {
  return {
      .remaining_entries = ohl::parser::kMaximumEnumeratedEntries,
      .remaining_path_bytes = ohl::parser::kMaximumEnumeratedPathBytes,
      .maximum_entry_bytes = ohl::parser::kMaximumEnumeratedEntryBytes,
      .remaining_total_bytes = ohl::parser::kMaximumEnumeratedTotalBytes,
  };
}

[[nodiscard]] constexpr bool equal_entry_batch_policy(
    const EntryBatchPolicy& left, const EntryBatchPolicy& right) {
  return left.remaining_entries == right.remaining_entries &&
         left.remaining_path_bytes == right.remaining_path_bytes &&
         left.maximum_entry_bytes == right.maximum_entry_bytes &&
         left.remaining_total_bytes == right.remaining_total_bytes &&
         left.has_previous_source_token == right.has_previous_source_token &&
         left.previous_source_token == right.previous_source_token;
}

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

[[nodiscard]] bool equal_bytes(const std::span<const std::byte> left,
                               const std::span<const std::byte> right) {
  return left.size() == right.size() &&
         std::equal(left.begin(), left.end(), right.begin());
}

[[nodiscard]] FrameHeader frame(const MessageType type,
                                const std::uint64_t request_id = 0,
                                const std::uint32_t payload_length = 0,
                                const std::uint64_t session_id = kSession) {
  return {
      .major_version = ohl::parser::kProtocolMajorVersion,
      .minor_version = ohl::parser::kProtocolMinorVersion,
      .type = type,
      .flags = 0,
      .payload_length = payload_length,
      .session_id = session_id,
      .request_id = request_id,
  };
}

[[nodiscard]] bool observe(ProtocolStateValidator& validator,
                           const MessageDirection direction,
                           const MessageType type,
                           const std::uint64_t request_id = 0,
                           const std::uint32_t payload_length = 0) {
  const auto error =
      validator.observe(direction, frame(type, request_id, payload_length));
  if (error != ProtocolError::none) {
    std::cerr << "state transition failed: "
              << ohl::parser::to_string(error) << '\n';
    return false;
  }
  return true;
}

[[nodiscard]] bool handshake(ProtocolStateValidator& validator) {
  return observe(validator, MessageDirection::parent_to_worker,
                 MessageType::hello) &&
         observe(validator, MessageDirection::worker_to_parent,
                 MessageType::ready) &&
         validator.state() == SessionState::idle;
}

[[nodiscard]] ProtocolError decode_hello_then_observe(
    ProtocolStateValidator& validator, const FrameView& candidate) {
  const auto decoded = ohl::parser::decode_hello_payload(candidate);
  if (!decoded.valid()) {
    return decoded.error;
  }
  return validator.observe(MessageDirection::parent_to_worker,
                           candidate.header);
}

[[nodiscard]] ProtocolError decode_complete_then_observe(
    ProtocolStateValidator& validator, const FrameView& candidate,
    const ProtocolPhase expected_operation_phase) {
  const auto decoded = ohl::parser::decode_complete_payload(
      candidate, expected_operation_phase);
  if (!decoded.valid()) {
    return decoded.error;
  }
  return validator.observe(MessageDirection::worker_to_parent,
                           candidate.header);
}

[[nodiscard]] ProtocolError decode_entry_batch_then_observe(
    ProtocolStateValidator& validator, const FrameView& candidate,
    const EntryBatchPolicy& policy,
    const std::span<EntryBatchEntry> entry_storage) {
  const auto decoded = ohl::parser::decode_entry_batch_payload(
      candidate, policy, entry_storage);
  if (!decoded.valid()) {
    return decoded.error;
  }
  return validator.observe(MessageDirection::worker_to_parent,
                           candidate.header);
}

[[nodiscard]] FrameView payload_frame(
    const MessageType type, const std::span<const std::byte> payload,
    const std::uint64_t request_id = 0) {
  return {
      .error = ProtocolError::none,
      .header = frame(type, request_id,
                      static_cast<std::uint32_t>(payload.size())),
      .payload = payload,
  };
}

[[nodiscard]] bool test_header_encoding() {
  const auto source = frame(MessageType::enumerate, 0x1112'1314'1516'1718ULL,
                            0x0001'0203U);
  std::array<std::byte, ohl::parser::kFrameHeaderBytes> encoded{};
  if (ohl::parser::encode_frame_header(source, encoded) !=
      ProtocolError::none) {
    return fail("valid header was not encoded");
  }
  const std::array<std::byte, ohl::parser::kFrameHeaderBytes> expected{
      std::byte{0x4f}, std::byte{0x48}, std::byte{0x4c}, std::byte{0x50},
      std::byte{0x01}, std::byte{0x00}, std::byte{0x00}, std::byte{0x00},
      std::byte{0x10}, std::byte{0x00}, std::byte{0x00}, std::byte{0x00},
      std::byte{0x03}, std::byte{0x02}, std::byte{0x01}, std::byte{0x00},
      std::byte{0x08}, std::byte{0x07}, std::byte{0x06}, std::byte{0x05},
      std::byte{0x04}, std::byte{0x03}, std::byte{0x02}, std::byte{0x01},
      std::byte{0x18}, std::byte{0x17}, std::byte{0x16}, std::byte{0x15},
      std::byte{0x14}, std::byte{0x13}, std::byte{0x12}, std::byte{0x11},
  };
  const auto decoded = ohl::parser::decode_frame_header(encoded);
  return encoded == expected && decoded.valid() &&
                 decoded.header.payload_length == source.payload_length &&
                 decoded.header.session_id == source.session_id &&
                 decoded.header.request_id == source.request_id
             ? true
             : fail("header was not canonical LE");
}

[[nodiscard]] bool test_frame_round_trip() {
  const std::array payload{std::byte{0x00}, std::byte{0x7f},
                           std::byte{0xff}};
  auto header = frame(MessageType::data_chunk, 7,
                      static_cast<std::uint32_t>(payload.size()));
  std::array<std::byte, ohl::parser::kFrameHeaderBytes + payload.size()>
      encoded{};
  const auto encode = ohl::parser::encode_frame(header, payload, encoded);
  if (!encode.valid() || encode.bytes_written != encoded.size()) {
    return fail("valid frame was not encoded");
  }
  const auto decoded = ohl::parser::decode_frame(encoded, kSession);
  return decoded.valid() && decoded.header.type == MessageType::data_chunk &&
                 decoded.header.request_id == 7 &&
                 equal_bytes(decoded.payload, payload)
             ? true
             : fail("frame round trip failed");
}

[[nodiscard]] bool test_frame_encode_rejections() {
  const std::array payload{std::byte{0x01}, std::byte{0x02}};
  std::array<std::byte, ohl::parser::kFrameHeaderBytes + payload.size()>
      encoded{};
  auto header = frame(MessageType::data_chunk, 1, 1);
  if (ohl::parser::encode_frame(header, payload, encoded).error !=
      ProtocolError::noncanonical_value) {
    return fail("frame payload-length mismatch was encoded");
  }
  header.payload_length = static_cast<std::uint32_t>(payload.size());
  if (ohl::parser::encode_frame(
          header, payload,
          std::span<std::byte>{encoded}.first(encoded.size() - 1))
          .error != ProtocolError::output_too_small) {
    return fail("frame was encoded into undersized output");
  }
  header.flags = 1;
  if (ohl::parser::encode_frame(header, payload, encoded).error !=
      ProtocolError::reserved_flags) {
    return fail("frame with reserved flags was encoded");
  }
  return true;
}

[[nodiscard]] bool test_frame_rejections() {
  std::array<std::byte, ohl::parser::kFrameHeaderBytes> encoded{};
  if (ohl::parser::encode_frame_header(frame(MessageType::hello), encoded) !=
      ProtocolError::none) {
    return fail("test header encoding failed");
  }
  if (ohl::parser::decode_frame(
          std::span<const std::byte>{encoded}.first(encoded.size() - 1))
          .error != ProtocolError::truncated_header) {
    return fail("truncated header was accepted");
  }

  auto invalid = encoded;
  invalid[0] = std::byte{0};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::invalid_magic) {
    return fail("invalid magic was accepted");
  }
  invalid = encoded;
  invalid[4] = std::byte{2};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::unsupported_version) {
    return fail("unsupported version was accepted");
  }
  invalid = encoded;
  invalid[8] = std::byte{0xff};
  invalid[9] = std::byte{0xff};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::unknown_message_type) {
    return fail("unknown message type was accepted");
  }
  invalid = encoded;
  invalid[10] = std::byte{1};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::reserved_flags) {
    return fail("reserved flags were accepted");
  }
  invalid = encoded;
  constexpr auto kOver = ohl::parser::kMaximumFramePayloadBytes + 1U;
  invalid[12] = static_cast<std::byte>(kOver & 0xffU);
  invalid[13] = static_cast<std::byte>((kOver >> 8U) & 0xffU);
  invalid[14] = static_cast<std::byte>((kOver >> 16U) & 0xffU);
  invalid[15] = static_cast<std::byte>((kOver >> 24U) & 0xffU);
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::payload_too_large) {
    return fail("one-over payload header was not rejected first");
  }
  invalid = encoded;
  invalid[16] = std::byte{0};
  invalid[17] = std::byte{0};
  invalid[18] = std::byte{0};
  invalid[19] = std::byte{0};
  invalid[20] = std::byte{0};
  invalid[21] = std::byte{0};
  invalid[22] = std::byte{0};
  invalid[23] = std::byte{0};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::invalid_session_id) {
    return fail("zero session id was accepted");
  }
  invalid = encoded;
  invalid[24] = std::byte{1};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::invalid_request_id) {
    return fail("handshake request id was accepted");
  }
  invalid = encoded;
  invalid[8] = std::byte{0x10};
  invalid[9] = std::byte{0x00};
  if (ohl::parser::decode_frame(invalid).error !=
      ProtocolError::invalid_request_id) {
    return fail("zero operation request id was accepted");
  }
  if (ohl::parser::decode_frame(encoded, kSession + 1).error !=
      ProtocolError::wrong_session_id) {
    return fail("wrong expected session was accepted");
  }

  auto payload_header = frame(MessageType::entry_batch, 1, 1);
  if (ohl::parser::encode_frame_header(payload_header, encoded) !=
      ProtocolError::none) {
    return fail("payload header encoding failed");
  }
  if (ohl::parser::decode_frame(encoded).error !=
      ProtocolError::truncated_payload) {
    return fail("truncated payload was accepted");
  }
  std::array<std::byte, ohl::parser::kFrameHeaderBytes + 2> trailing{};
  payload_header.payload_length = 1;
  std::span<std::byte, ohl::parser::kFrameHeaderBytes> header_span{
      trailing.data(), ohl::parser::kFrameHeaderBytes};
  if (ohl::parser::encode_frame_header(payload_header, header_span) !=
      ProtocolError::none ||
      ohl::parser::decode_frame(trailing).error !=
          ProtocolError::trailing_bytes) {
    return fail("trailing frame bytes were accepted");
  }
  return true;
}

[[nodiscard]] bool test_maximum_frame() {
  std::vector<std::byte> payload(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0x5a});
  std::vector<std::byte> encoded(ohl::parser::kFrameHeaderBytes +
                                 payload.size());
  const auto header = frame(MessageType::data_chunk, 1,
                            ohl::parser::kMaximumFramePayloadBytes);
  const auto result = ohl::parser::encode_frame(header, payload, encoded);
  const auto decoded = ohl::parser::decode_frame(encoded);
  return result.valid() && decoded.valid() &&
                 decoded.payload.size() == payload.size()
             ? true
             : fail("exact maximum frame failed");
}

[[nodiscard]] bool test_payload_codec() {
  const std::array canonical{
      std::byte{0x12}, std::byte{0x56}, std::byte{0x34}, std::byte{0xde},
      std::byte{0xbc}, std::byte{0x9a}, std::byte{0x78}, std::byte{0x08},
      std::byte{0x07}, std::byte{0x06}, std::byte{0x05}, std::byte{0x04},
      std::byte{0x03}, std::byte{0x02}, std::byte{0x01}, std::byte{0x01},
      std::byte{0x06}, std::byte{0x00}, std::byte{0x03}, std::byte{0x00},
      std::byte{0xaa}, std::byte{0xbb},
  };
  std::array<std::byte, 32> storage{};
  PayloadWriter writer{storage};
  const std::array tail{std::byte{0xaa}, std::byte{0xbb}};
  if (!writer.write_u8(0x12) || !writer.write_u16(0x3456) ||
      !writer.write_u32(0x789a'bcdeU) ||
      !writer.write_u64(0x0102'0304'0506'0708ULL) ||
      !writer.write_bool(true) ||
      !writer.write_status(ProtocolStatus::source_changed) ||
      !writer.write_phase(ProtocolPhase::source_read) ||
      !writer.write_bytes(tail)) {
    return fail("payload writer rejected bounded values");
  }
  if (!equal_bytes(writer.written(), canonical)) {
    return fail("payload writer was not canonical little-endian");
  }

  PayloadReader reader{canonical};
  std::uint8_t u8 = 0;
  std::uint16_t u16 = 0;
  std::uint32_t u32 = 0;
  std::uint64_t u64 = 0;
  bool boolean = false;
  ProtocolStatus status = ProtocolStatus::ok;
  ProtocolPhase phase = ProtocolPhase::handshake;
  std::span<const std::byte> bytes;
  if (!reader.read_u8(u8) || !reader.read_u16(u16) ||
      !reader.read_u32(u32) || !reader.read_u64(u64) ||
      !reader.read_bool(boolean) || !reader.read_status(status) ||
      !reader.read_phase(phase) || !reader.read_bytes(tail.size(), bytes) ||
      !reader.finish()) {
    return fail("payload reader rejected canonical payload");
  }
  return u8 == 0x12 && u16 == 0x3456 && u32 == 0x789a'bcdeU &&
                 u64 == 0x0102'0304'0506'0708ULL && boolean &&
                 status == ProtocolStatus::source_changed &&
                 phase == ProtocolPhase::source_read &&
                 equal_bytes(bytes, tail)
             ? true
             : fail("payload codec changed values");
}

[[nodiscard]] bool test_terminal_and_cancel_rejections() {
  {
    ProtocolStateValidator validator{kSession};
    if (validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::hello)) !=
        ProtocolError::unexpected_message) {
      return fail("hello from worker direction was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!observe(validator, MessageDirection::parent_to_worker,
                 MessageType::hello) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::ready)) !=
            ProtocolError::unexpected_message) {
      return fail("ready from parent direction was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    const auto first = validator.observe(MessageDirection::worker_to_parent,
                                          frame(MessageType::ready));
    const auto repeated = validator.observe(
        MessageDirection::parent_to_worker, frame(MessageType::hello));
    if (first != ProtocolError::unexpected_message || repeated != first ||
        validator.error() != first ||
        validator.state() != SessionState::failed) {
      return fail("protocol failure was not sticky");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::shutdown) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::shutdown)) !=
            ProtocolError::terminal_state ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::hello)) !=
            ProtocolError::terminal_state ||
        validator.state() != SessionState::failed) {
      return fail("closed terminal rejection was not sticky");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::cancel_ack, 1) ||
        validator.state() != SessionState::cancelled ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::stream_entry, 2)) !=
            ProtocolError::terminal_state) {
      return fail("cancelled state accepted a non-shutdown message");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::cancel, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("cancel in idle state was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::cancel, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("cancel from worker direction was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::cancel, 2)) !=
            ProtocolError::wrong_request_id) {
      return fail("cancel with wrong request id was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::cancel_ack, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("cancel acknowledgement before cancel was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::cancel_ack, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("cancel acknowledgement from parent was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::cancel_ack, 2)) !=
            ProtocolError::wrong_request_id) {
      return fail("cancel acknowledgement with wrong id was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::complete, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::cancel, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("repeated stale cancel was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::complete, 1) ||
        validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::cancel, 2)) !=
            ProtocolError::wrong_request_id) {
      return fail("wrong request cancelled a completed race window");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::stream_entry, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::complete, 1) ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::cancel_ack, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("cancel acknowledgement after completion was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::stream_entry, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::cancel_ack, 1) ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::cancel_ack, 1)) !=
            ProtocolError::terminal_state) {
      return fail("cancelled terminal state accepted repeated ack");
    }
  }
  return true;
}

[[nodiscard]] bool test_payload_rejections() {
  std::array<std::byte, 1> small{};
  PayloadWriter writer{small};
  if (writer.write_u16(1) || writer.write_u8(1) || writer.size() != 0 ||
      writer.error() != ProtocolError::output_too_small) {
    return fail("payload writer one-over was accepted");
  }

  std::array<std::byte, 4> invalid_storage{};
  PayloadWriter invalid_writer{invalid_storage};
  if (invalid_writer.write_status(static_cast<ProtocolStatus>(0xffffU)) ||
      invalid_writer.write_phase(ProtocolPhase::handshake) ||
      invalid_writer.size() != 0 ||
      invalid_writer.error() != ProtocolError::noncanonical_value) {
    return fail("payload writer error was not sticky");
  }
  PayloadWriter invalid_phase_writer{invalid_storage};
  if (invalid_phase_writer.write_phase(
          static_cast<ProtocolPhase>(0xffffU)) ||
      invalid_phase_writer.error() != ProtocolError::noncanonical_value) {
    return fail("unknown phase was written");
  }

  const std::array invalid_bool{std::byte{2}};
  PayloadReader bool_reader{invalid_bool};
  bool boolean = false;
  if (bool_reader.read_bool(boolean) ||
      bool_reader.error() != ProtocolError::noncanonical_value) {
    return fail("noncanonical boolean was accepted");
  }
  std::uint8_t sticky_byte = 0;
  if (bool_reader.read_u8(sticky_byte) || bool_reader.remaining() != 0 ||
      bool_reader.error() != ProtocolError::noncanonical_value) {
    return fail("payload reader error was not sticky");
  }
  const std::array invalid_status{std::byte{0xff}, std::byte{0xff}};
  PayloadReader status_reader{invalid_status};
  ProtocolStatus status = ProtocolStatus::ok;
  if (status_reader.read_status(status) ||
      status_reader.error() != ProtocolError::noncanonical_value) {
    return fail("unknown status was accepted");
  }
  PayloadReader phase_reader{invalid_status};
  ProtocolPhase phase = ProtocolPhase::handshake;
  if (phase_reader.read_phase(phase) ||
      phase_reader.error() != ProtocolError::noncanonical_value) {
    return fail("unknown phase was accepted");
  }
  PayloadReader underflow{small};
  std::uint16_t value = 0;
  if (underflow.read_u16(value) ||
      underflow.error() != ProtocolError::payload_underflow) {
    return fail("payload underflow was accepted");
  }
  const std::array trailing{std::byte{0}, std::byte{1}};
  PayloadReader trailing_reader{trailing};
  std::uint8_t byte = 0;
  if (!trailing_reader.read_u8(byte) || trailing_reader.finish() ||
      trailing_reader.error() != ProtocolError::payload_trailing_bytes) {
    return fail("payload trailing bytes were accepted");
  }
  std::vector<std::byte> oversized(
      static_cast<std::size_t>(ohl::parser::kMaximumFramePayloadBytes) + 1U);
  PayloadReader oversized_reader{oversized};
  return oversized_reader.error() == ProtocolError::payload_too_large
             ? true
             : fail("one-over payload reader was accepted");
}

[[nodiscard]] bool test_crossed_completion_and_cancellation() {
  const std::array operations{MessageType::enumerate,
                              MessageType::stream_entry};
  for (const auto operation : operations) {
    const auto result = operation == MessageType::enumerate
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;

    // Parent-side ordering: cancellation is committed while a read is
    // outstanding. Its already-crossing reply resolves the read before
    // bounded worker traffic and completion arrive.
    ProtocolStateValidator cancel_first{kSession};
    if (!handshake(cancel_first) ||
        !observe(cancel_first, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(cancel_first, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        !observe(cancel_first, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(cancel_first, MessageDirection::parent_to_worker,
                 MessageType::read_reply, 1) ||
        !observe(cancel_first, MessageDirection::worker_to_parent,
                 result, 1) ||
        !observe(cancel_first, MessageDirection::worker_to_parent,
                 MessageType::complete, 1) ||
        cancel_first.state() != SessionState::idle) {
      return fail("completion did not win cancel-first duplex race");
    }

    // Worker-side ordering of the same race: completion is committed before
    // the already-sent cancel arrives. The stale cancel is consumed once.
    ProtocolStateValidator complete_first{kSession};
    if (!handshake(complete_first) ||
        !observe(complete_first, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(complete_first, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        !observe(complete_first, MessageDirection::parent_to_worker,
                 MessageType::read_reply, 1) ||
        !observe(complete_first, MessageDirection::worker_to_parent,
                 result, 1) ||
        !observe(complete_first, MessageDirection::worker_to_parent,
                 MessageType::complete, 1) ||
        !observe(complete_first, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        complete_first.state() != SessionState::idle ||
        !observe(complete_first, MessageDirection::parent_to_worker,
                 operation, 2)) {
      return fail("completion did not win complete-first duplex race");
    }

    // A read request may itself cross cancellation. It is bounded and
    // ignored; cancel_ack remains the cancellation terminal response.
    ProtocolStateValidator crossed_read{kSession};
    if (!handshake(crossed_read) ||
        !observe(crossed_read, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(crossed_read, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(crossed_read, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        !observe(crossed_read, MessageDirection::worker_to_parent,
                 MessageType::cancel_ack, 1) ||
        crossed_read.state() != SessionState::cancelled) {
      return fail("crossed read/cancel sequence failed");
    }
  }
  return true;
}

[[nodiscard]] bool test_unresolved_reads_block_crossed_completion() {
  for (const auto operation :
       {MessageType::enumerate, MessageType::stream_entry}) {
    const auto result = operation == MessageType::enumerate
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;

    ProtocolStateValidator pre_cancel_read{kSession};
    if (!handshake(pre_cancel_read) ||
        !observe(pre_cancel_read, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(pre_cancel_read, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        !observe(pre_cancel_read, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        pre_cancel_read.observe(MessageDirection::worker_to_parent,
                                frame(MessageType::complete, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("completion bypassed a pre-cancel unresolved read");
    }

    ProtocolStateValidator post_cancel_read{kSession};
    if (!handshake(post_cancel_read) ||
        !observe(post_cancel_read, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(post_cancel_read, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(post_cancel_read, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        post_cancel_read.observe(MessageDirection::worker_to_parent,
                                 frame(MessageType::complete, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("completion bypassed a post-cancel crossed read");
    }

    ProtocolStateValidator result_before_reply{kSession};
    if (!handshake(result_before_reply) ||
        !observe(result_before_reply, MessageDirection::parent_to_worker,
                 operation, 1) ||
        !observe(result_before_reply, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1) ||
        !observe(result_before_reply, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        result_before_reply.observe(MessageDirection::worker_to_parent,
                                    frame(result, 1)) !=
            ProtocolError::unexpected_message) {
      return fail("result bypassed a pre-cancel unresolved read");
    }
  }
  return true;
}

[[nodiscard]] bool test_late_read_reply_drain() {
  for (const auto operation :
       {MessageType::enumerate, MessageType::stream_entry}) {
    const auto acknowledged = [operation](const bool pre_cancel_read,
                                          const bool post_cancel_read) {
      ProtocolStateValidator validator{kSession};
      if (!handshake(validator) ||
          !observe(validator, MessageDirection::parent_to_worker,
                   operation, 1) ||
          (pre_cancel_read &&
           !observe(validator, MessageDirection::worker_to_parent,
                    MessageType::read_request, 1)) ||
          !observe(validator, MessageDirection::parent_to_worker,
                   MessageType::cancel, 1) ||
          (post_cancel_read &&
           !observe(validator, MessageDirection::worker_to_parent,
                    MessageType::read_request, 1)) ||
          !observe(validator, MessageDirection::worker_to_parent,
                   MessageType::cancel_ack, 1)) {
        return validator;
      }
      return validator;
    };

    // cancel_ack may overtake the reply that was already queued in the
    // opposite direction. The cancelled session drains it once and remains
    // terminal until shutdown.
    auto valid = acknowledged(true, false);
    if (valid.state() != SessionState::cancelled ||
        valid.active_request_id() != 0 ||
        !observe(valid, MessageDirection::parent_to_worker,
                 MessageType::read_reply, 1) ||
        valid.state() != SessionState::cancelled ||
        !observe(valid, MessageDirection::parent_to_worker,
                 MessageType::shutdown) ||
        valid.state() != SessionState::closed) {
      return fail("late pre-cancel read reply was not drained once");
    }

    auto wrong_id = acknowledged(true, false);
    const auto wrong_id_error = wrong_id.observe(
        MessageDirection::parent_to_worker,
        frame(MessageType::read_reply, 2));
    if (wrong_id_error != ProtocolError::wrong_request_id ||
        wrong_id.observe(MessageDirection::parent_to_worker,
                         frame(MessageType::read_reply, 1)) != wrong_id_error ||
        wrong_id.state() != SessionState::failed) {
      return fail("wrong-id late read reply did not fail stickily");
    }

    auto wrong_direction = acknowledged(true, false);
    const auto wrong_direction_error = wrong_direction.observe(
        MessageDirection::worker_to_parent,
        frame(MessageType::read_reply, 1));
    if (wrong_direction_error != ProtocolError::terminal_state ||
        wrong_direction.observe(MessageDirection::parent_to_worker,
                                frame(MessageType::read_reply, 1)) !=
            wrong_direction_error ||
        wrong_direction.state() != SessionState::failed) {
      return fail("wrong-direction late read reply did not fail stickily");
    }

    auto duplicate = acknowledged(true, false);
    if (!observe(duplicate, MessageDirection::parent_to_worker,
                 MessageType::read_reply, 1) ||
        duplicate.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::read_reply, 1)) !=
            ProtocolError::terminal_state) {
      return fail("duplicate late read reply was accepted");
    }

    auto no_outstanding = acknowledged(false, false);
    if (no_outstanding.observe(MessageDirection::parent_to_worker,
                               frame(MessageType::read_reply, 1)) !=
        ProtocolError::terminal_state) {
      return fail("cancel without an outstanding read opened a drain window");
    }

    auto post_cancel_read = acknowledged(false, true);
    if (post_cancel_read.observe(MessageDirection::parent_to_worker,
                                 frame(MessageType::read_reply, 1)) !=
        ProtocolError::terminal_state) {
      return fail("post-cancel read request opened a drain window");
    }

    auto shutdown = acknowledged(true, false);
    if (!observe(shutdown, MessageDirection::parent_to_worker,
                 MessageType::shutdown) ||
        shutdown.observe(MessageDirection::parent_to_worker,
                         frame(MessageType::read_reply, 1)) !=
            ProtocolError::terminal_state) {
      return fail("shutdown did not close the late-reply drain window");
    }
  }
  return true;
}

[[nodiscard]] bool test_direction_and_active_terminal_rejections() {
  const std::array operations{MessageType::enumerate,
                              MessageType::stream_entry};
  for (const auto operation : operations) {
    const auto result = operation == MessageType::enumerate
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;
    const auto rejects = [operation](const MessageDirection direction,
                                     const MessageType type,
                                     const bool outstanding_read = false) {
      ProtocolStateValidator validator{kSession};
      if (!handshake(validator) ||
          !observe(validator, MessageDirection::parent_to_worker,
                   operation, 1) ||
          (outstanding_read &&
           !observe(validator, MessageDirection::worker_to_parent,
                    MessageType::read_request, 1))) {
        return false;
      }
      const auto request_id = type == MessageType::shutdown ? 0U : 1U;
      return validator.observe(direction, frame(type, request_id)) ==
             ProtocolError::unexpected_message;
    };
    if (!rejects(MessageDirection::parent_to_worker,
                 MessageType::read_request) ||
        !rejects(MessageDirection::worker_to_parent,
                 MessageType::read_reply) ||
        !rejects(MessageDirection::parent_to_worker, result) ||
        !rejects(MessageDirection::parent_to_worker,
                 MessageType::complete) ||
        !rejects(MessageDirection::parent_to_worker,
                 MessageType::shutdown) ||
        !rejects(MessageDirection::worker_to_parent,
                 MessageType::complete, true)) {
      return fail("wrong-direction or active terminal message was accepted");
    }
  }
  return true;
}

[[nodiscard]] bool test_cancelling_crossed_traffic_rejections() {
  const auto cancelling = [](const MessageType operation,
                             const bool outstanding_read = false) {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 operation, 1) ||
        (outstanding_read &&
         !observe(validator, MessageDirection::worker_to_parent,
                  MessageType::read_request, 1)) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1)) {
      return validator;
    }
    return validator;
  };

  for (const auto operation :
       {MessageType::enumerate, MessageType::stream_entry}) {
    const auto result = operation == MessageType::enumerate
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;
    const auto other_result = operation == MessageType::enumerate
                                  ? MessageType::data_chunk
                                  : MessageType::entry_batch;
    auto wrong_result_direction = cancelling(operation);
    auto wrong_result_type = cancelling(operation);
    auto wrong_complete_direction = cancelling(operation);
    auto wrong_read_request_direction = cancelling(operation);
    auto wrong_read_reply_direction = cancelling(operation, true);
    auto unsolicited_read_reply = cancelling(operation);
    auto repeated_read_request = cancelling(operation, true);
    if (wrong_result_direction.observe(MessageDirection::parent_to_worker,
                                       frame(result, 1)) !=
            ProtocolError::unexpected_message ||
        wrong_result_type.observe(MessageDirection::worker_to_parent,
                                  frame(other_result, 1)) !=
            ProtocolError::unexpected_message ||
        wrong_complete_direction.observe(MessageDirection::parent_to_worker,
                                         frame(MessageType::complete, 1)) !=
            ProtocolError::unexpected_message ||
        wrong_read_request_direction.observe(
            MessageDirection::parent_to_worker,
            frame(MessageType::read_request, 1)) !=
            ProtocolError::unexpected_message ||
        wrong_read_reply_direction.observe(
            MessageDirection::worker_to_parent,
            frame(MessageType::read_reply, 1)) !=
            ProtocolError::unexpected_message ||
        unsolicited_read_reply.observe(
            MessageDirection::parent_to_worker,
            frame(MessageType::read_reply, 1)) !=
            ProtocolError::no_read_in_flight ||
        repeated_read_request.observe(
            MessageDirection::worker_to_parent,
            frame(MessageType::read_request, 1)) !=
            ProtocolError::read_already_active) {
      return fail("invalid crossed traffic was accepted while cancelling");
    }
  }
  return true;
}

[[nodiscard]] bool test_valid_state_sequence() {
  ProtocolStateValidator validator{kSession};
  if (!handshake(validator) ||
      !observe(validator, MessageDirection::parent_to_worker,
               MessageType::enumerate, 1, 3) ||
      !observe(validator, MessageDirection::worker_to_parent,
               MessageType::read_request, 1, 2) ||
      !observe(validator, MessageDirection::parent_to_worker,
               MessageType::read_reply, 1, 4) ||
      !observe(validator, MessageDirection::worker_to_parent,
               MessageType::entry_batch, 1, 5) ||
      !observe(validator, MessageDirection::worker_to_parent,
               MessageType::complete, 1, 1) ||
      !observe(validator, MessageDirection::parent_to_worker,
               MessageType::stream_entry, 2, 2) ||
      !observe(validator, MessageDirection::worker_to_parent,
               MessageType::data_chunk, 2, 8) ||
      !observe(validator, MessageDirection::worker_to_parent,
               MessageType::complete, 2, 1) ||
      !observe(validator, MessageDirection::parent_to_worker,
               MessageType::shutdown)) {
    return false;
  }
  return validator.state() == SessionState::closed &&
                 validator.message_count() == 11 &&
                 validator.payload_bytes() == 26
             ? true
             : fail("valid sequence accounting failed");
}

[[nodiscard]] bool test_state_rejections() {
  {
    ProtocolStateValidator validator{kSession};
    if (validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::ready)) !=
        ProtocolError::unexpected_message) {
      return fail("ready-before-hello was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator)) {
      return false;
    }
    if (validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::enumerate, 1, 0,
                                kSession + 1)) !=
        ProtocolError::wrong_session_id) {
      return fail("wrong state session was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1)) {
      return false;
    }
    if (validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::read_reply, 1)) !=
        ProtocolError::no_read_in_flight) {
      return fail("read reply without request was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 2) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::complete, 2)) {
      return false;
    }
    if (validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::stream_entry, 2)) !=
        ProtocolError::request_id_not_monotonic) {
      return fail("nonmonotonic request id was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1)) {
      return false;
    }
    if (validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::read_request, 1)) !=
        ProtocolError::read_already_active) {
      return fail("second in-flight read was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::read_request, 1)) {
      return false;
    }
    if (validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::entry_batch, 1)) !=
        ProtocolError::unexpected_message) {
      return fail("result while read was in flight was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1)) {
      return false;
    }
    if (validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::entry_batch, 2)) !=
        ProtocolError::wrong_request_id) {
      return fail("wrong active request id was accepted");
    }
  }
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1)) {
      return false;
    }
    if (validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::stream_entry, 2)) !=
        ProtocolError::request_already_active) {
      return fail("second top-level request was accepted");
    }
  }
  return true;
}

[[nodiscard]] bool test_cancellation_and_budgets() {
  {
    ProtocolStateValidator validator{kSession};
    if (!handshake(validator) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::cancel_ack, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::shutdown) ||
        validator.state() != SessionState::closed) {
      return fail("valid cancellation sequence failed");
    }
  }
  {
    ProtocolStateValidator validator{
        kSession, {.maximum_messages = 3, .maximum_payload_bytes = 5}};
    if (!observe(validator, MessageDirection::parent_to_worker,
                 MessageType::hello, 0, 2) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::ready, 0, 1) ||
        !observe(validator, MessageDirection::parent_to_worker,
                 MessageType::enumerate, 1, 2)) {
      return false;
    }
    if (validator.message_count() != 3 || validator.payload_bytes() != 5 ||
        validator.observe(MessageDirection::worker_to_parent,
                          frame(MessageType::complete, 1)) !=
            ProtocolError::message_budget_exceeded) {
      return fail("message budget exact/one-over failed");
    }
  }
  {
    ProtocolStateValidator validator{
        kSession, {.maximum_messages = 4, .maximum_payload_bytes = 4}};
    if (!observe(validator, MessageDirection::parent_to_worker,
                 MessageType::hello, 0, 2) ||
        !observe(validator, MessageDirection::worker_to_parent,
                 MessageType::ready, 0, 2)) {
      return false;
    }
    if (validator.observe(MessageDirection::parent_to_worker,
                          frame(MessageType::enumerate, 1, 1)) !=
        ProtocolError::byte_budget_exceeded) {
      return fail("byte budget exact/one-over failed");
    }
  }
  const ProtocolStateValidator zero_message_budget{
      kSession, {.maximum_messages = 0, .maximum_payload_bytes = 1}};
  const ProtocolStateValidator raised_message_budget{
      kSession,
      {.maximum_messages = ohl::parser::kMaximumProtocolMessages + 1,
       .maximum_payload_bytes = 1}};
  const ProtocolStateValidator zero_payload_budget{
      kSession, {.maximum_messages = 1, .maximum_payload_bytes = 0}};
  const ProtocolStateValidator raised_payload_budget{
      kSession,
      {.maximum_messages = 1,
       .maximum_payload_bytes =
           ohl::parser::kMaximumCumulativePayloadBytes + 1}};
  const ProtocolStateValidator zero_session{0};
  return zero_message_budget.error() == ProtocolError::invalid_budget &&
                 raised_message_budget.error() ==
                     ProtocolError::invalid_budget &&
                 zero_payload_budget.error() == ProtocolError::invalid_budget &&
                 raised_payload_budget.error() ==
                     ProtocolError::invalid_budget &&
                 zero_session.error() == ProtocolError::invalid_session_id
             ? true
             : fail("invalid budgets or session were accepted");
}

[[nodiscard]] bool test_typed_hello_and_ready() {
  const HelloMessage canonical{0x0102'0304'0506'0708ULL, 0x0001'0203U};
  const std::array<std::byte, ohl::parser::kHelloPayloadBytes> expected{
      std::byte{0x08}, std::byte{0x07}, std::byte{0x06}, std::byte{0x05},
      std::byte{0x04}, std::byte{0x03}, std::byte{0x02}, std::byte{0x01},
      std::byte{0x03}, std::byte{0x02}, std::byte{0x01}, std::byte{0x00},
  };
  std::array<std::byte, ohl::parser::kHelloPayloadBytes> encoded{};
  const auto encode = ohl::parser::encode_hello_payload(canonical, encoded);
  const auto decode = ohl::parser::decode_hello_payload(
      payload_frame(MessageType::hello, encoded));
  if (!encode.valid() || encode.bytes_written != encoded.size() ||
      encoded != expected || !decode.valid() ||
      decode.message.source_size != canonical.source_size ||
      decode.message.maximum_read_bytes != canonical.maximum_read_bytes) {
    return fail("typed hello canonical encoding failed");
  }

  for (const auto message : {
           HelloMessage{1, 1},
           HelloMessage{std::numeric_limits<std::uint64_t>::max(),
                        ohl::parser::kMaximumReadBytes},
       }) {
    if (!ohl::parser::encode_hello_payload(message, encoded).valid() ||
        !ohl::parser::decode_hello_payload(
             payload_frame(MessageType::hello, encoded))
             .valid()) {
      return fail("typed hello min/max round trip failed");
    }
  }

  std::array<std::byte, ohl::parser::kHelloPayloadBytes + 1> malformed{};
  for (std::size_t size = 0; size < ohl::parser::kHelloPayloadBytes; ++size) {
    const auto result = ohl::parser::decode_hello_payload(payload_frame(
        MessageType::hello,
        std::span<const std::byte>{malformed}.first(size)));
    if (result.error != ProtocolError::payload_underflow ||
        result.message.source_size != 0 ||
        result.message.maximum_read_bytes != 0) {
      return fail("short typed hello was not rejected atomically");
    }
  }
  if (ohl::parser::decode_hello_payload(payload_frame(
          MessageType::hello, malformed))
          .error != ProtocolError::payload_trailing_bytes) {
    return fail("long typed hello was accepted");
  }

  const auto hello_error = [&](const HelloMessage message) {
    if (!ohl::parser::encode_hello_payload(message, encoded).valid()) {
      return true;
    }
    return !ohl::parser::decode_hello_payload(
                payload_frame(MessageType::hello, encoded))
                .valid();
  };
  if (!hello_error({0, 1}) || !hello_error({1, 0}) ||
      hello_error({1, ohl::parser::kMaximumReadBytes}) ||
      !hello_error({1, ohl::parser::kMaximumReadBytes + 1U})) {
    return fail("typed hello value bounds changed");
  }
  const auto decode_hello_values = [&](const HelloMessage message) {
    PayloadWriter writer{encoded};
    return writer.write_u64(message.source_size) &&
           writer.write_u32(message.maximum_read_bytes)
               ? ohl::parser::decode_hello_payload(
                     payload_frame(MessageType::hello, encoded))
                     .error
               : ProtocolError::output_too_small;
  };
  if (decode_hello_values({0, 1}) != ProtocolError::noncanonical_value ||
      decode_hello_values({1, 0}) != ProtocolError::noncanonical_value ||
      decode_hello_values({1, ohl::parser::kMaximumReadBytes}) !=
          ProtocolError::none ||
      decode_hello_values({1, ohl::parser::kMaximumReadBytes + 1U}) !=
          ProtocolError::noncanonical_value) {
    return fail("typed hello decoder value bounds changed");
  }

  const std::array<std::byte, 1> nonempty_ready{std::byte{0}};
  if (!ohl::parser::encode_ready_payload(ReadyMessage{}, {}).valid() ||
      !ohl::parser::decode_ready_payload(
           payload_frame(MessageType::ready, {}))
           .valid() ||
      ohl::parser::decode_ready_payload(
          payload_frame(MessageType::ready, nonempty_ready))
              .error != ProtocolError::payload_trailing_bytes ||
      ohl::parser::decode_hello_payload(
          payload_frame(MessageType::ready, expected))
              .error != ProtocolError::unexpected_message ||
      ohl::parser::decode_ready_payload(
          payload_frame(MessageType::hello, {}))
              .error != ProtocolError::unexpected_message) {
    return fail("typed hello/ready type or empty-payload validation failed");
  }
  return true;
}

[[nodiscard]] bool test_typed_exact_empty_messages() {
  static_assert(ohl::parser::kReadyPayloadBytes == 0);
  static_assert(ohl::parser::kEnumeratePayloadBytes == 0);
  static_assert(ohl::parser::kCancelPayloadBytes == 0);
  static_assert(ohl::parser::kCancelAckPayloadBytes == 0);
  static_assert(ohl::parser::kShutdownPayloadBytes == 0);

  std::array<std::byte, 8> destination;
  destination.fill(std::byte{0xa5});
  const auto ready_encode =
      ohl::parser::encode_ready_payload(ReadyMessage{}, destination);
  const auto enumerate_encode = ohl::parser::encode_enumerate_payload(
      ohl::parser::EnumerateMessage{}, destination);
  const auto cancel_encode = ohl::parser::encode_cancel_payload(
      ohl::parser::CancelMessage{}, destination);
  const auto cancel_ack_encode = ohl::parser::encode_cancel_ack_payload(
      ohl::parser::CancelAckMessage{}, destination);
  const auto shutdown_encode = ohl::parser::encode_shutdown_payload(
      ohl::parser::ShutdownMessage{}, destination);
  const auto untouched = std::all_of(
      destination.begin(), destination.end(), [](const std::byte value) {
        return value == std::byte{0xa5};
      });
  if (!ready_encode.valid() || ready_encode.bytes_written != 0 ||
      !enumerate_encode.valid() || enumerate_encode.bytes_written != 0 ||
      !cancel_encode.valid() || cancel_encode.bytes_written != 0 ||
      !cancel_ack_encode.valid() || cancel_ack_encode.bytes_written != 0 ||
      !shutdown_encode.valid() || shutdown_encode.bytes_written != 0 ||
      !untouched) {
    return fail("empty typed encoder wrote payload bytes");
  }

  if (!ohl::parser::decode_enumerate_payload(
           payload_frame(MessageType::enumerate, {}, 1))
           .valid() ||
      !ohl::parser::decode_cancel_payload(
           payload_frame(MessageType::cancel, {}, 1))
           .valid() ||
      !ohl::parser::decode_cancel_ack_payload(
           payload_frame(MessageType::cancel_ack, {}, 1))
           .valid() ||
      !ohl::parser::decode_shutdown_payload(
           payload_frame(MessageType::shutdown, {}))
           .valid()) {
    return fail("canonical empty typed payload was rejected");
  }

  const std::array<std::byte, 1> nonempty{std::byte{0x5a}};
  if (ohl::parser::decode_enumerate_payload(
          payload_frame(MessageType::enumerate, nonempty, 1))
          .error != ProtocolError::payload_trailing_bytes ||
      ohl::parser::decode_cancel_payload(
          payload_frame(MessageType::cancel, nonempty, 1))
          .error != ProtocolError::payload_trailing_bytes ||
      ohl::parser::decode_cancel_ack_payload(
          payload_frame(MessageType::cancel_ack, nonempty, 1))
          .error != ProtocolError::payload_trailing_bytes ||
      ohl::parser::decode_shutdown_payload(
          payload_frame(MessageType::shutdown, nonempty))
          .error != ProtocolError::payload_trailing_bytes) {
    return fail("nonempty exact-empty typed payload was accepted");
  }

  if (ohl::parser::decode_enumerate_payload(
          payload_frame(MessageType::cancel, {}, 1))
          .error != ProtocolError::unexpected_message ||
      ohl::parser::decode_cancel_payload(
          payload_frame(MessageType::cancel_ack, {}, 1))
          .error != ProtocolError::unexpected_message ||
      ohl::parser::decode_cancel_ack_payload(
          payload_frame(MessageType::enumerate, {}, 1))
          .error != ProtocolError::unexpected_message ||
      ohl::parser::decode_shutdown_payload(
          payload_frame(MessageType::ready, {}))
          .error != ProtocolError::unexpected_message) {
    return fail("wrong typed empty message was accepted");
  }

  auto enumerate_truncated =
      payload_frame(MessageType::enumerate, {}, 1);
  auto cancel_truncated = payload_frame(MessageType::cancel, {}, 1);
  auto cancel_ack_truncated =
      payload_frame(MessageType::cancel_ack, {}, 1);
  auto shutdown_truncated = payload_frame(MessageType::shutdown, {});
  auto enumerate_trailing =
      payload_frame(MessageType::enumerate, nonempty, 1);
  auto cancel_trailing = payload_frame(MessageType::cancel, nonempty, 1);
  auto cancel_ack_trailing =
      payload_frame(MessageType::cancel_ack, nonempty, 1);
  auto shutdown_trailing =
      payload_frame(MessageType::shutdown, nonempty);
  enumerate_truncated.header.payload_length = 1;
  cancel_truncated.header.payload_length = 1;
  cancel_ack_truncated.header.payload_length = 1;
  shutdown_truncated.header.payload_length = 1;
  enumerate_trailing.header.payload_length = 0;
  cancel_trailing.header.payload_length = 0;
  cancel_ack_trailing.header.payload_length = 0;
  shutdown_trailing.header.payload_length = 0;
  if (ohl::parser::decode_enumerate_payload(enumerate_truncated).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_cancel_payload(cancel_truncated).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_cancel_ack_payload(cancel_ack_truncated).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_shutdown_payload(shutdown_truncated).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_enumerate_payload(enumerate_trailing).error !=
          ProtocolError::trailing_bytes ||
      ohl::parser::decode_cancel_payload(cancel_trailing).error !=
          ProtocolError::trailing_bytes ||
      ohl::parser::decode_cancel_ack_payload(cancel_ack_trailing).error !=
          ProtocolError::trailing_bytes ||
      ohl::parser::decode_shutdown_payload(shutdown_trailing).error !=
          ProtocolError::trailing_bytes) {
    return fail("empty typed decoder accepted declared-length mismatch");
  }

  auto enumerate_bad_id = payload_frame(MessageType::enumerate, {});
  auto cancel_bad_id = payload_frame(MessageType::cancel, {});
  auto cancel_ack_bad_id = payload_frame(MessageType::cancel_ack, {});
  auto shutdown_bad_id = payload_frame(MessageType::shutdown, {}, 1);
  auto enumerate_bad_flags =
      payload_frame(MessageType::enumerate, {}, 1);
  enumerate_bad_flags.header.flags = 1;
  if (ohl::parser::decode_enumerate_payload(enumerate_bad_id).error !=
          ProtocolError::invalid_request_id ||
      ohl::parser::decode_cancel_payload(cancel_bad_id).error !=
          ProtocolError::invalid_request_id ||
      ohl::parser::decode_cancel_ack_payload(cancel_ack_bad_id).error !=
          ProtocolError::invalid_request_id ||
      ohl::parser::decode_shutdown_payload(shutdown_bad_id).error !=
          ProtocolError::invalid_request_id ||
      ohl::parser::decode_enumerate_payload(enumerate_bad_flags).error !=
          ProtocolError::reserved_flags) {
    return fail("empty typed decoder accepted invalid frame header");
  }

  const FrameView prior_error{
      .error = ProtocolError::trailing_bytes,
      .header = {},
      .payload = {},
  };
  const auto enumerate_error =
      ohl::parser::decode_enumerate_payload(prior_error);
  const auto cancel_error = ohl::parser::decode_cancel_payload(prior_error);
  const auto cancel_ack_error =
      ohl::parser::decode_cancel_ack_payload(prior_error);
  const auto shutdown_error =
      ohl::parser::decode_shutdown_payload(prior_error);
  if (enumerate_error.valid() ||
      enumerate_error.error != ProtocolError::trailing_bytes ||
      cancel_error.valid() ||
      cancel_error.error != ProtocolError::trailing_bytes ||
      cancel_ack_error.valid() ||
      cancel_ack_error.error != ProtocolError::trailing_bytes ||
      shutdown_error.valid() ||
      shutdown_error.error != ProtocolError::trailing_bytes) {
    return fail("empty typed decoder did not preserve prior frame error");
  }
  return true;
}

[[nodiscard]] bool test_typed_stream_entry() {
  static_assert(ohl::parser::kStreamEntryPayloadBytes == 8);

  constexpr StreamEntryMessage canonical{0x0102'0304'0506'0708ULL};
  const std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes> expected{
      std::byte{0x08}, std::byte{0x07}, std::byte{0x06}, std::byte{0x05},
      std::byte{0x04}, std::byte{0x03}, std::byte{0x02}, std::byte{0x01},
  };
  std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes> encoded{};
  const auto encode =
      ohl::parser::encode_stream_entry_payload(canonical, encoded);
  const auto decode = ohl::parser::decode_stream_entry_payload(
      payload_frame(MessageType::stream_entry, encoded, 1));
  if (!encode.valid() || encode.bytes_written != encoded.size() ||
      encoded != expected || !decode.valid() ||
      decode.message.source_token != canonical.source_token) {
    return fail("typed stream entry canonical encoding failed");
  }

  for (const auto token : {
           std::uint64_t{0}, std::numeric_limits<std::uint64_t>::max(),
           std::uint64_t{0x89ab'cdef'0123'4567ULL},
       }) {
    const auto round_trip_encode = ohl::parser::encode_stream_entry_payload(
        StreamEntryMessage{token}, encoded);
    const auto round_trip_decode = ohl::parser::decode_stream_entry_payload(
        payload_frame(MessageType::stream_entry, encoded, 1));
    if (!round_trip_encode.valid() ||
        round_trip_encode.bytes_written != encoded.size() ||
        !round_trip_decode.valid() ||
        round_trip_decode.message.source_token != token) {
      return fail("typed stream entry opaque-token round trip failed");
    }
  }

  std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes + 1>
      malformed{};
  for (std::size_t size = 0;
       size < ohl::parser::kStreamEntryPayloadBytes; ++size) {
    const auto result = ohl::parser::decode_stream_entry_payload(payload_frame(
        MessageType::stream_entry,
        std::span<const std::byte>{malformed}.first(size), 1));
    if (result.error != ProtocolError::payload_underflow ||
        result.message.source_token != 0) {
      return fail("short typed stream entry was not rejected atomically");
    }
  }
  const auto long_payload = ohl::parser::decode_stream_entry_payload(
      payload_frame(MessageType::stream_entry, malformed, 1));
  const auto wrong_type = ohl::parser::decode_stream_entry_payload(
      payload_frame(MessageType::read_request, expected, 1));
  if (long_payload.error != ProtocolError::payload_trailing_bytes ||
      long_payload.message.source_token != 0 ||
      wrong_type.error != ProtocolError::unexpected_message ||
      wrong_type.message.source_token != 0) {
    return fail("typed stream entry shape or type validation failed");
  }

  auto declared_longer =
      payload_frame(MessageType::stream_entry, expected, 1);
  auto declared_shorter =
      payload_frame(MessageType::stream_entry, expected, 1);
  ++declared_longer.header.payload_length;
  --declared_shorter.header.payload_length;
  const auto truncated =
      ohl::parser::decode_stream_entry_payload(declared_longer);
  const auto trailing =
      ohl::parser::decode_stream_entry_payload(declared_shorter);
  if (truncated.error != ProtocolError::truncated_payload ||
      truncated.message.source_token != 0 ||
      trailing.error != ProtocolError::trailing_bytes ||
      trailing.message.source_token != 0) {
    return fail("typed stream entry accepted declared-length mismatch");
  }

  auto bad_request_id =
      payload_frame(MessageType::stream_entry, expected, 0);
  auto bad_flags = payload_frame(MessageType::stream_entry, expected, 1);
  bad_flags.header.flags = 1;
  const auto invalid_request =
      ohl::parser::decode_stream_entry_payload(bad_request_id);
  const auto invalid_flags =
      ohl::parser::decode_stream_entry_payload(bad_flags);
  const FrameView prior_error{
      .error = ProtocolError::payload_too_large,
      .header = {},
      .payload = {},
  };
  const auto inherited_error =
      ohl::parser::decode_stream_entry_payload(prior_error);
  if (invalid_request.error != ProtocolError::invalid_request_id ||
      invalid_request.message.source_token != 0 ||
      invalid_flags.error != ProtocolError::reserved_flags ||
      invalid_flags.message.source_token != 0 ||
      inherited_error.error != ProtocolError::payload_too_large ||
      inherited_error.message.source_token != 0) {
    return fail("typed stream entry trust-boundary validation failed");
  }

  std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes> destination;
  destination.fill(std::byte{0xa5});
  const auto undersized = ohl::parser::encode_stream_entry_payload(
      canonical, std::span<std::byte>{destination}.first(
                     ohl::parser::kStreamEntryPayloadBytes - 1));
  const auto unchanged = std::all_of(
      destination.begin(), destination.end(), [](const std::byte value) {
        return value == std::byte{0xa5};
      });
  if (undersized.error != ProtocolError::output_too_small ||
      undersized.bytes_written != 0 || !unchanged) {
    return fail("short stream entry encoding changed destination");
  }
  return true;
}

[[nodiscard]] bool test_typed_read_request() {
  constexpr SourceReadPolicy policy{std::numeric_limits<std::uint64_t>::max(),
                                    ohl::parser::kMaximumReadBytes};
  constexpr ReadRequestMessage canonical{0x0102'0304U,
                                         0x1112'1314'1516'1718ULL,
                                         0x0001'0203U};
  const std::array<std::byte, ohl::parser::kReadRequestPayloadBytes> expected{
      std::byte{0x04}, std::byte{0x03}, std::byte{0x02}, std::byte{0x01},
      std::byte{0x18}, std::byte{0x17}, std::byte{0x16}, std::byte{0x15},
      std::byte{0x14}, std::byte{0x13}, std::byte{0x12}, std::byte{0x11},
      std::byte{0x03}, std::byte{0x02}, std::byte{0x01}, std::byte{0x00},
  };
  std::array<std::byte, ohl::parser::kReadRequestPayloadBytes> encoded{};
  const auto encode = ohl::parser::encode_read_request_payload(
      canonical, policy, canonical.read_sequence, encoded);
  const auto decode = ohl::parser::decode_read_request_payload(
      payload_frame(MessageType::read_request, encoded, 7), policy,
      canonical.read_sequence);
  if (!encode.valid() || encoded != expected || !decode.valid() ||
      decode.message.read_sequence != canonical.read_sequence ||
      decode.message.offset != canonical.offset ||
      decode.message.length != canonical.length) {
    return fail("typed read request canonical encoding failed");
  }

  for (const auto item : {
           ReadRequestMessage{1, 0, 1},
           ReadRequestMessage{std::numeric_limits<std::uint32_t>::max(),
                              std::numeric_limits<std::uint64_t>::max() -
                                  ohl::parser::kMaximumReadBytes,
                              ohl::parser::kMaximumReadBytes},
       }) {
    const SourceReadPolicy bounds{
        item.length == 1 ? 1 : std::numeric_limits<std::uint64_t>::max(),
        item.length};
    if (!ohl::parser::encode_read_request_payload(
             item, bounds, item.read_sequence, encoded)
             .valid() ||
        !ohl::parser::decode_read_request_payload(
             payload_frame(MessageType::read_request, encoded, 1), bounds,
             item.read_sequence)
             .valid()) {
      return fail("typed read request min/max round trip failed");
    }
  }

  std::array<std::byte, ohl::parser::kReadRequestPayloadBytes + 1> malformed{};
  for (std::size_t size = 0; size < ohl::parser::kReadRequestPayloadBytes;
       ++size) {
    if (ohl::parser::decode_read_request_payload(
            payload_frame(MessageType::read_request,
                          std::span<const std::byte>{malformed}.first(size), 1),
            {1, 1}, 1)
            .error != ProtocolError::payload_underflow) {
      return fail("short typed read request was accepted");
    }
  }
  if (ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_request, malformed, 1), {1, 1}, 1)
          .error != ProtocolError::payload_trailing_bytes) {
    return fail("long typed read request was accepted");
  }

  const auto rejects = [&](const ReadRequestMessage message,
                           const SourceReadPolicy bounds,
                           const std::uint32_t expected_sequence) {
    return ohl::parser::encode_read_request_payload(
               message, bounds, expected_sequence, encoded)
               .error != ProtocolError::none;
  };
  if (!rejects({0, 0, 1}, {1, 1}, 1) ||
      !rejects({2, 0, 1}, {1, 1}, 1) ||
      !rejects({1, 0, 0}, {1, 1}, 1) ||
      !rejects({1, 0, 1}, {0, 1}, 1) ||
      !rejects({1, 0, 1}, {1, 0}, 1) ||
      !rejects({1, 0, 1},
               {1, ohl::parser::kMaximumReadBytes + 1U}, 1) ||
      !rejects({1, 0, 1}, {1, 1}, 0) ||
      !rejects({1, 0, ohl::parser::kMaximumReadBytes + 1U},
               {std::numeric_limits<std::uint64_t>::max(),
                ohl::parser::kMaximumReadBytes},
               1) ||
      rejects({1, 4, 4}, {8, 4}, 1) ||
      !rejects({1, 4, 5}, {8, 5}, 1) ||
      !rejects({1, std::numeric_limits<std::uint64_t>::max() - 1, 2},
               {std::numeric_limits<std::uint64_t>::max(), 2}, 1)) {
    return fail("typed read request value/range bounds changed");
  }

  const auto decode_request_values = [&](const ReadRequestMessage message,
                                         const SourceReadPolicy bounds,
                                         const std::uint32_t sequence) {
    PayloadWriter writer{encoded};
    if (!writer.write_u32(message.read_sequence) ||
        !writer.write_u64(message.offset) ||
        !writer.write_u32(message.length)) {
      return ProtocolError::output_too_small;
    }
    return ohl::parser::decode_read_request_payload(
               payload_frame(MessageType::read_request, encoded, 1), bounds,
               sequence)
        .error;
  };
  if (decode_request_values({0, 0, 1}, {1, 1}, 1) !=
          ProtocolError::noncanonical_value ||
      decode_request_values({2, 0, 1}, {1, 1}, 1) !=
          ProtocolError::noncanonical_value ||
      decode_request_values({1, 0, 0}, {1, 1}, 1) !=
          ProtocolError::noncanonical_value ||
      decode_request_values({1, 0, ohl::parser::kMaximumReadBytes + 1U},
                            {std::numeric_limits<std::uint64_t>::max(),
                             ohl::parser::kMaximumReadBytes},
                            1) != ProtocolError::noncanonical_value ||
      decode_request_values({1, 4, 4}, {8, 4}, 1) != ProtocolError::none ||
      decode_request_values({1, 4, 5}, {8, 5}, 1) !=
          ProtocolError::noncanonical_value ||
      decode_request_values(
          {1, std::numeric_limits<std::uint64_t>::max() - 1, 2},
          {std::numeric_limits<std::uint64_t>::max(), 2}, 1) !=
          ProtocolError::noncanonical_value) {
    return fail("typed read request decoder value/range bounds changed");
  }

  if (ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_request, encoded, 1), {0, 1}, 1)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_request, encoded, 1), {1, 0}, 1)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_request, encoded, 1),
          {1, ohl::parser::kMaximumReadBytes + 1U}, 1)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_request, encoded, 1), {1, 1}, 0)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_request_payload(
          payload_frame(MessageType::read_reply, encoded, 1), policy,
          canonical.read_sequence)
          .error != ProtocolError::unexpected_message) {
    return fail("typed read request context/type validation failed");
  }
  return true;
}

[[nodiscard]] bool test_typed_read_reply() {
  constexpr std::uint32_t kCanonicalReplyBytes = 3;
  const std::array<std::byte, kCanonicalReplyBytes> data{
      std::byte{0xaa}, std::byte{0xbb}, std::byte{0xcc}};
  const ReadReplyMessage canonical{0x0102'0304U, ProtocolStatus::ok, data};
  const std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + data.size()>
      expected{std::byte{0x04}, std::byte{0x03}, std::byte{0x02},
               std::byte{0x01}, std::byte{0x00}, std::byte{0x00},
               std::byte{0xaa}, std::byte{0xbb}, std::byte{0xcc}};
  std::array<std::byte, expected.size()> encoded{};
  const auto encode = ohl::parser::encode_read_reply_payload(
      canonical, canonical.read_sequence, kCanonicalReplyBytes, encoded);
  const auto decode = ohl::parser::decode_read_reply_payload(
      payload_frame(MessageType::read_reply, encoded, 7),
      canonical.read_sequence, kCanonicalReplyBytes);
  if (!encode.valid() || encoded != expected || !decode.valid() ||
      decode.message.read_sequence != canonical.read_sequence ||
      decode.message.status != ProtocolStatus::ok ||
      !equal_bytes(decode.message.data, data) ||
      decode.message.data.data() !=
          encoded.data() + ohl::parser::kReadReplyPrefixBytes) {
    return fail("typed read reply canonical encoding failed");
  }

  const std::array<std::byte, 1> one_byte{std::byte{0x5a}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> minimum{};
  if (!ohl::parser::encode_read_reply_payload(
           {1, ProtocolStatus::ok, one_byte}, 1, 1, minimum)
           .valid() ||
      !ohl::parser::decode_read_reply_payload(
           payload_frame(MessageType::read_reply, minimum, 1), 1, 1)
           .valid()) {
    return fail("typed read reply minimum round trip failed");
  }
  std::vector<std::byte> maximum_data(ohl::parser::kMaximumReadBytes,
                                      std::byte{0x5a});
  std::vector<std::byte> maximum_payload(ohl::parser::kMaximumFramePayloadBytes);
  const auto maximum_encode = ohl::parser::encode_read_reply_payload(
      {std::numeric_limits<std::uint32_t>::max(), ProtocolStatus::ok,
       maximum_data},
      std::numeric_limits<std::uint32_t>::max(),
      ohl::parser::kMaximumReadBytes, maximum_payload);
  const auto maximum_decode = ohl::parser::decode_read_reply_payload(
      payload_frame(MessageType::read_reply, maximum_payload, 1),
      std::numeric_limits<std::uint32_t>::max(),
      ohl::parser::kMaximumReadBytes);
  if (!maximum_encode.valid() || !maximum_decode.valid() ||
      maximum_decode.message.data.size() != maximum_data.size()) {
    return fail("typed read reply maximum round trip failed");
  }

  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes> prefix{};
  for (std::size_t size = 0; size < ohl::parser::kReadReplyPrefixBytes;
       ++size) {
    if (ohl::parser::decode_read_reply_payload(
            payload_frame(MessageType::read_reply,
                          std::span<const std::byte>{prefix}.first(size), 1),
            1, 1)
            .error != ProtocolError::payload_underflow) {
      return fail("short typed read reply was accepted");
    }
  }

  const auto reply_rejects = [&](const ReadReplyMessage& message,
                                 const std::uint32_t sequence,
                                 const std::uint32_t length) {
    std::array<std::byte, 32> output{};
    return ohl::parser::encode_read_reply_payload(message, sequence, length,
                                                   output)
               .error != ProtocolError::none;
  };
  if (!reply_rejects({0, ProtocolStatus::ok, one_byte}, 1, 1) ||
      !reply_rejects({2, ProtocolStatus::ok, one_byte}, 1, 1) ||
      !reply_rejects({1, ProtocolStatus::ok, one_byte}, 0, 1) ||
      !reply_rejects({1, ProtocolStatus::ok, one_byte}, 1, 0) ||
      !reply_rejects({1, ProtocolStatus::ok, one_byte}, 1,
                     ohl::parser::kMaximumReadBytes + 1U) ||
      !reply_rejects({1, ProtocolStatus::ok, {}}, 1, 1) ||
      !reply_rejects({1, ProtocolStatus::ok, data}, 1, 2) ||
      !reply_rejects({1, ProtocolStatus::ok, one_byte}, 1, 2) ||
      !reply_rejects({1, ProtocolStatus::source_changed, one_byte}, 1, 1) ||
      !reply_rejects({1, ProtocolStatus::source_read_failed, one_byte}, 1, 1) ||
      reply_rejects({1, ProtocolStatus::source_changed, {}}, 1, 1) ||
      reply_rejects({1, ProtocolStatus::source_read_failed, {}}, 1, 1)) {
    return fail("typed read reply shape validation changed");
  }

  for (const auto status : {
           ProtocolStatus::unsupported,
           ProtocolStatus::invalid_request,
           ProtocolStatus::parser_rejected,
           ProtocolStatus::budget_exceeded,
           ProtocolStatus::cancelled,
           ProtocolStatus::result_validation_failed,
           ProtocolStatus::internal_failure,
       }) {
    if (!reply_rejects({1, status, {}}, 1, 1)) {
      return fail("disallowed typed read reply status was accepted");
    }
  }
  if (!reply_rejects(
          {1, static_cast<ProtocolStatus>(0xffffU), {}}, 1, 1)) {
    return fail("unknown typed read reply status was accepted");
  }

  const auto decode_reply_values = [&](const std::uint32_t message_sequence,
                                       const ProtocolStatus status,
                                       const std::span<const std::byte> bytes,
                                       const std::uint32_t expected_sequence,
                                       const std::uint32_t requested_length) {
    std::vector<std::byte> payload(ohl::parser::kReadReplyPrefixBytes +
                                   bytes.size());
    PayloadWriter writer{payload};
    if (!writer.write_u32(message_sequence) || !writer.write_status(status) ||
        !writer.write_bytes(bytes)) {
      return ProtocolError::noncanonical_value;
    }
    return ohl::parser::decode_read_reply_payload(
               payload_frame(MessageType::read_reply, payload, 1),
               expected_sequence, requested_length)
        .error;
  };
  if (decode_reply_values(0, ProtocolStatus::ok, one_byte, 1, 1) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(2, ProtocolStatus::ok, one_byte, 1, 1) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::ok, {}, 1, 1) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::ok, one_byte, 1, 2) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::ok, data, 1, 2) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::source_changed, one_byte, 1, 1) !=
          ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::source_read_failed, one_byte, 1,
                          1) != ProtocolError::noncanonical_value ||
      decode_reply_values(1, ProtocolStatus::source_changed, {}, 1, 1) !=
          ProtocolError::none ||
      decode_reply_values(1, ProtocolStatus::source_read_failed, {}, 1, 1) !=
          ProtocolError::none) {
    return fail("typed read reply decoder shape validation changed");
  }
  for (const auto status : {
           ProtocolStatus::unsupported,
           ProtocolStatus::invalid_request,
           ProtocolStatus::parser_rejected,
           ProtocolStatus::budget_exceeded,
           ProtocolStatus::cancelled,
           ProtocolStatus::result_validation_failed,
           ProtocolStatus::internal_failure,
       }) {
    if (decode_reply_values(1, status, {}, 1, 1) !=
        ProtocolError::noncanonical_value) {
      return fail("disallowed typed read reply status decoded");
    }
  }
  auto unknown_status = prefix;
  unknown_status[0] = std::byte{1};
  unknown_status[4] = std::byte{0xff};
  unknown_status[5] = std::byte{0xff};
  if (ohl::parser::decode_read_reply_payload(
          payload_frame(MessageType::read_reply, unknown_status, 1), 1, 1)
          .error != ProtocolError::noncanonical_value) {
    return fail("unknown typed read reply status decoded");
  }

  if (ohl::parser::decode_read_reply_payload(
          payload_frame(MessageType::read_reply, minimum, 1), 0, 1)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_reply_payload(
          payload_frame(MessageType::read_reply, minimum, 1), 1, 0)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_reply_payload(
          payload_frame(MessageType::read_reply, minimum, 1), 1,
          ohl::parser::kMaximumReadBytes + 1U)
          .error != ProtocolError::invalid_budget ||
      ohl::parser::decode_read_reply_payload(
          payload_frame(MessageType::read_request, minimum, 1), 1, 1)
          .error != ProtocolError::unexpected_message) {
    return fail("typed read reply context/type validation failed");
  }
  return true;
}

[[nodiscard]] bool test_typed_entry_batch_wire_and_bounds() {
  static_assert(ohl::parser::kEntryBatchPrefixBytes == 2);
  static_assert(ohl::parser::kEntryBatchEntryPrefixBytes == 18);
  static_assert(ohl::parser::kMaximumEntryBatchEntries == 256);
  static_assert(ohl::parser::kMaximumEntryBatchPathBytes == 4'096);

  const auto policy = maximum_entry_batch_policy();
  const std::array canonical_entries{EntryBatchEntry{
      0x0102'0304'0506'0708ULL, 0x0000'0001'1516'1718ULL, "A~"}};
  const std::array<std::byte, 22> canonical_bytes{
      std::byte{0x01}, std::byte{0x00},
      std::byte{0x08}, std::byte{0x07}, std::byte{0x06}, std::byte{0x05},
      std::byte{0x04}, std::byte{0x03}, std::byte{0x02}, std::byte{0x01},
      std::byte{0x18}, std::byte{0x17}, std::byte{0x16}, std::byte{0x15},
      std::byte{0x01}, std::byte{0x00}, std::byte{0x00}, std::byte{0x00},
      std::byte{0x02}, std::byte{0x00}, std::byte{'A'}, std::byte{'~'},
  };
  std::array<std::byte, canonical_bytes.size()> canonical_output{};
  std::array<EntryBatchEntry, 1> canonical_storage{};
  const auto canonical_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{canonical_entries}, policy, canonical_output);
  const auto canonical_frame =
      payload_frame(MessageType::entry_batch, canonical_output, 1);
  const auto canonical_decode = ohl::parser::decode_entry_batch_payload(
      canonical_frame, policy, canonical_storage);
  // Decoded entries alias canonical_storage and their paths alias the frame.
  // Keep both backing objects alive and unchanged through every assertion.
  if (!canonical_encode.valid() ||
      canonical_encode.bytes_written != canonical_output.size() ||
      canonical_output != canonical_bytes || !canonical_decode.valid() ||
      canonical_decode.message.entries.data() != canonical_storage.data() ||
      canonical_decode.message.entries.size() != canonical_storage.size() ||
      canonical_storage[0].source_token !=
          canonical_entries[0].source_token ||
      canonical_storage[0].size_bytes != canonical_entries[0].size_bytes ||
      canonical_storage[0].archive_path != canonical_entries[0].archive_path ||
      canonical_storage[0].archive_path.data() !=
          reinterpret_cast<const char*>(
              canonical_frame.payload.data() +
              ohl::parser::kEntryBatchPrefixBytes +
              ohl::parser::kEntryBatchEntryPrefixBytes)) {
    return fail("typed entry batch canonical field order failed");
  }

  const std::array extreme_entries{
      EntryBatchEntry{0, 0, " "},
      EntryBatchEntry{std::numeric_limits<std::uint64_t>::max(),
                      ohl::parser::kMaximumEnumeratedEntryBytes, "~"},
  };
  std::array<std::byte, 40> extreme_output{};
  std::array<EntryBatchEntry, extreme_entries.size()> extreme_storage{};
  const auto extreme_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{extreme_entries}, policy, extreme_output);
  const auto extreme_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, extreme_output, 1), policy,
      extreme_storage);
  if (!extreme_encode.valid() || !extreme_decode.valid() ||
      extreme_storage[0].source_token != 0 ||
      extreme_storage[0].size_bytes != 0 ||
      extreme_storage[0].archive_path != " " ||
      extreme_storage[1].source_token !=
          std::numeric_limits<std::uint64_t>::max() ||
      extreme_storage[1].size_bytes !=
          ohl::parser::kMaximumEnumeratedEntryBytes ||
      extreme_storage[1].archive_path != "~") {
    return fail("typed entry batch token/size/ASCII extrema failed");
  }

  std::vector<EntryBatchEntry> maximum_count_entries;
  maximum_count_entries.reserve(ohl::parser::kMaximumEntryBatchEntries);
  for (std::uint16_t token = 0;
       token < ohl::parser::kMaximumEntryBatchEntries; ++token) {
    maximum_count_entries.push_back({token, 0, "a"});
  }
  const auto maximum_count_payload_size =
      ohl::parser::kEntryBatchPrefixBytes +
      maximum_count_entries.size() *
          (ohl::parser::kEntryBatchEntryPrefixBytes + 1U);
  std::vector<std::byte> maximum_count_output(maximum_count_payload_size);
  std::array<EntryBatchEntry, ohl::parser::kMaximumEntryBatchEntries>
      maximum_count_storage{};
  const auto maximum_count_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{maximum_count_entries}, policy, maximum_count_output);
  const auto maximum_count_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, maximum_count_output, 1), policy,
      maximum_count_storage);
  if (!maximum_count_encode.valid() || !maximum_count_decode.valid() ||
      maximum_count_decode.message.entries.size() !=
          ohl::parser::kMaximumEntryBatchEntries ||
      maximum_count_storage.front().source_token != 0 ||
      maximum_count_storage.back().source_token !=
          ohl::parser::kMaximumEntryBatchEntries - 1U) {
    return fail("typed entry batch maximum count failed");
  }

  const EntryBatchMessage empty_message{};
  std::array<std::byte, 2> small_output{};
  const std::array<std::byte, 2> zero_count{};
  const std::array<std::byte, 2> over_count{std::byte{0x01}, std::byte{0x01}};
  std::array<EntryBatchEntry, 1> small_storage{};
  const auto empty_encode = ohl::parser::encode_entry_batch_payload(
      empty_message, policy, small_output);
  const auto empty_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, zero_count, 1), policy,
      small_storage);
  std::vector<EntryBatchEntry> over_count_entries;
  over_count_entries.reserve(ohl::parser::kMaximumEntryBatchEntries + 1U);
  for (std::uint16_t token = 0;
       token <= ohl::parser::kMaximumEntryBatchEntries; ++token) {
    over_count_entries.push_back({token, 0, "a"});
  }
  const auto over_count_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{over_count_entries}, policy, {});
  const auto over_count_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, over_count, 1), policy,
      small_storage);
  if (empty_encode.error != ProtocolError::noncanonical_value ||
      empty_decode.error != ProtocolError::noncanonical_value ||
      over_count_encode.error != ProtocolError::noncanonical_value ||
      over_count_decode.error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch count bounds failed");
  }

  const std::string maximum_path(ohl::parser::kMaximumEntryBatchPathBytes,
                                 '~');
  const std::array maximum_path_entries{
      EntryBatchEntry{1, 0, maximum_path}};
  std::vector<std::byte> maximum_path_output(
      ohl::parser::kEntryBatchPrefixBytes +
      ohl::parser::kEntryBatchEntryPrefixBytes + maximum_path.size());
  std::array<EntryBatchEntry, 1> maximum_path_storage{};
  const auto maximum_path_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{maximum_path_entries}, policy, maximum_path_output);
  const auto maximum_path_frame =
      payload_frame(MessageType::entry_batch, maximum_path_output, 1);
  const auto maximum_path_decode = ohl::parser::decode_entry_batch_payload(
      maximum_path_frame, policy, maximum_path_storage);
  if (!maximum_path_encode.valid() || !maximum_path_decode.valid() ||
      maximum_path_storage[0].archive_path.size() != maximum_path.size() ||
      maximum_path_storage[0].archive_path.data() !=
          reinterpret_cast<const char*>(
              maximum_path_frame.payload.data() +
              ohl::parser::kEntryBatchPrefixBytes +
              ohl::parser::kEntryBatchEntryPrefixBytes)) {
    return fail("typed entry batch maximum path failed");
  }

  const std::string overlong_path(
      ohl::parser::kMaximumEntryBatchPathBytes + 1U, 'a');
  const std::array empty_path_entries{EntryBatchEntry{1, 0, {}}};
  const std::array overlong_path_entries{
      EntryBatchEntry{1, 0, overlong_path}};
  if (ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{empty_path_entries}, policy, {})
          .error != ProtocolError::noncanonical_value ||
      ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{overlong_path_entries}, policy, {})
          .error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch path length encoding bounds failed");
  }

  const auto raw_single_entry = [](const std::uint64_t token,
                                   const std::uint64_t size,
                                   const std::span<const std::byte> path) {
    std::vector<std::byte> payload(
        ohl::parser::kEntryBatchPrefixBytes +
        ohl::parser::kEntryBatchEntryPrefixBytes + path.size());
    PayloadWriter writer{payload};
    if (!writer.write_u16(1) || !writer.write_u64(token) ||
        !writer.write_u64(size) ||
        !writer.write_u16(static_cast<std::uint16_t>(path.size())) ||
        !writer.write_bytes(path)) {
      payload.clear();
    }
    return payload;
  };
  const auto empty_path_payload =
      raw_single_entry(1, 0, std::span<const std::byte>{});
  const auto overlong_path_payload = raw_single_entry(
      1, 0, std::as_bytes(std::span<const char>{overlong_path}));
  if (ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, empty_path_payload, 1),
          policy, small_storage)
          .error != ProtocolError::noncanonical_value ||
      ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, overlong_path_payload, 1),
          policy, small_storage)
          .error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch path length decoding bounds failed");
  }

  for (const auto invalid_byte : {
           std::byte{0x1f}, std::byte{0x7f}, std::byte{0x80}}) {
    const std::array invalid_path{invalid_byte};
    const std::string invalid_string(
        1, static_cast<char>(std::to_integer<unsigned char>(invalid_byte)));
    const std::array invalid_entries{
        EntryBatchEntry{1, 0, invalid_string}};
    const auto invalid_payload = raw_single_entry(1, 0, invalid_path);
    if (ohl::parser::encode_entry_batch_payload(
            EntryBatchMessage{invalid_entries}, policy, {})
            .error != ProtocolError::noncanonical_value ||
        ohl::parser::decode_entry_batch_payload(
            payload_frame(MessageType::entry_batch, invalid_payload, 1),
            policy, small_storage)
            .error != ProtocolError::noncanonical_value) {
      return fail("typed entry batch accepted non-printable path bytes");
    }
  }

  const std::array unsafe_printable_entries{
      EntryBatchEntry{1, 0, "../x"}, EntryBatchEntry{2, 0, "/abs"},
      EntryBatchEntry{3, 0, "C:\\x"}, EntryBatchEntry{4, 0, "a//b"},
  };
  std::vector<std::byte> unsafe_output(128);
  std::array<EntryBatchEntry, unsafe_printable_entries.size()> unsafe_storage{};
  const auto unsafe_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{unsafe_printable_entries}, policy, unsafe_output);
  if (!unsafe_encode.valid()) {
    return fail("typed entry batch rejected printable unsafe spellings");
  }
  const auto unsafe_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(
          MessageType::entry_batch,
          std::span<const std::byte>{unsafe_output}.first(
              unsafe_encode.bytes_written),
          1),
      policy, unsafe_storage);
  if (!unsafe_decode.valid()) {
    return fail("typed entry batch did not decode unsafe printable spellings");
  }
  for (std::size_t index = 0; index < unsafe_printable_entries.size();
       ++index) {
    if (unsafe_storage[index].archive_path !=
        unsafe_printable_entries[index].archive_path) {
      return fail("typed entry batch changed an unsafe printable spelling");
    }
  }
  return true;
}

[[nodiscard]] bool test_typed_entry_batch_policy_and_atomicity() {
  const auto raw_batch = [](const std::span<const EntryBatchEntry> entries) {
    std::size_t payload_size = ohl::parser::kEntryBatchPrefixBytes;
    for (const auto& entry : entries) {
      payload_size += ohl::parser::kEntryBatchEntryPrefixBytes +
                      entry.archive_path.size();
    }
    std::vector<std::byte> payload(payload_size);
    PayloadWriter writer{payload};
    if (!writer.write_u16(static_cast<std::uint16_t>(entries.size()))) {
      payload.clear();
      return payload;
    }
    for (const auto& entry : entries) {
      if (!writer.write_u64(entry.source_token) ||
          !writer.write_u64(entry.size_bytes) ||
          !writer.write_u16(
              static_cast<std::uint16_t>(entry.archive_path.size())) ||
          !writer.write_bytes(std::as_bytes(std::span<const char>{
              entry.archive_path.data(), entry.archive_path.size()}))) {
        payload.clear();
        return payload;
      }
    }
    return payload;
  };
  const auto is_default = [](const auto& result) {
    return result.message.entries.data() == nullptr &&
           result.message.entries.empty();
  };
  const EntryBatchEntry sentinel_entry{
      0xa5a5'a5a5'a5a5'a5a5ULL, 0xa5a5'a5a5'a5a5'a5a5ULL, "sentinel"};
  const auto all_sentinel = [&sentinel_entry](const auto& storage) {
    return std::all_of(storage.begin(), storage.end(),
                       [&sentinel_entry](const EntryBatchEntry& entry) {
                         return entry.source_token ==
                                    sentinel_entry.source_token &&
                                entry.size_bytes == sentinel_entry.size_bytes &&
                                entry.archive_path ==
                                    sentinel_entry.archive_path;
                       });
  };

  const std::array budget_entries{EntryBatchEntry{5, 2, "ab"},
                                  EntryBatchEntry{9, 3, "c"}};
  const auto budget_payload = raw_batch(budget_entries);
  const EntryBatchPolicy exact_policy{
      .remaining_entries = 2,
      .remaining_path_bytes = 3,
      .maximum_entry_bytes = 3,
      .remaining_total_bytes = 5,
  };
  std::vector<std::byte> budget_output(budget_payload.size());
  std::array<EntryBatchEntry, budget_entries.size()> budget_storage{};
  const auto exact_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{budget_entries}, exact_policy, budget_output);
  const auto exact_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, budget_payload, 1), exact_policy,
      budget_storage);
  if (!exact_encode.valid() || !exact_decode.valid() ||
      budget_output != budget_payload) {
    return fail("typed entry batch exact budgets were rejected");
  }

  const std::array exceeded_policies{
      EntryBatchPolicy{1, 3, 3, 5, false, 0},
      EntryBatchPolicy{2, 2, 3, 5, false, 0},
      EntryBatchPolicy{2, 3, 2, 5, false, 0},
      EntryBatchPolicy{2, 3, 3, 4, false, 0},
  };
  for (const auto& exceeded : exceeded_policies) {
    if (ohl::parser::encode_entry_batch_payload(
            EntryBatchMessage{budget_entries}, exceeded, budget_output)
            .error != ProtocolError::noncanonical_value ||
        ohl::parser::decode_entry_batch_payload(
            payload_frame(MessageType::entry_batch, budget_payload, 1),
            exceeded, budget_storage)
            .error != ProtocolError::noncanonical_value) {
      return fail("typed entry batch exceeded a trusted budget");
    }
  }

  const auto maximum_policy = maximum_entry_batch_policy();
  const std::array invalid_policies{
      EntryBatchPolicy{0, 1, 1, 0, false, 0},
      EntryBatchPolicy{ohl::parser::kMaximumEnumeratedEntries + 1U, 1, 1, 0,
                       false, 0},
      EntryBatchPolicy{1, 0, 1, 0, false, 0},
      EntryBatchPolicy{1, ohl::parser::kMaximumEnumeratedPathBytes + 1U, 1, 0,
                       false, 0},
      EntryBatchPolicy{1, 1, 0, 0, false, 0},
      EntryBatchPolicy{1, 1,
                       ohl::parser::kMaximumEnumeratedEntryBytes + 1U, 0,
                       false, 0},
      EntryBatchPolicy{1, 1, 1,
                       ohl::parser::kMaximumEnumeratedTotalBytes + 1U, false,
                       0},
      EntryBatchPolicy{1, 1, 1, 0, false, 1},
  };
  for (const auto& invalid : invalid_policies) {
    if (invalid.valid() ||
        ohl::parser::encode_entry_batch_payload(
            EntryBatchMessage{budget_entries}, invalid, budget_output)
            .error != ProtocolError::invalid_budget ||
        ohl::parser::decode_entry_batch_payload(
            payload_frame(MessageType::entry_batch, budget_payload, 1),
            invalid, budget_storage)
            .error != ProtocolError::invalid_budget) {
      return fail("typed entry batch accepted an invalid policy");
    }
  }

  const std::array increasing_entries{EntryBatchEntry{1, 0, "a"},
                                      EntryBatchEntry{2, 0, "b"}};
  const std::array duplicate_entries{EntryBatchEntry{1, 0, "a"},
                                     EntryBatchEntry{1, 0, "b"}};
  const std::array reordered_entries{EntryBatchEntry{2, 0, "a"},
                                     EntryBatchEntry{1, 0, "b"}};
  const auto duplicate_payload = raw_batch(duplicate_entries);
  const auto reordered_payload = raw_batch(reordered_entries);
  std::array<EntryBatchEntry, 2> order_storage{};
  if (!ohl::parser::encode_entry_batch_payload(
           EntryBatchMessage{increasing_entries}, maximum_policy, budget_output)
           .valid() ||
      ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{duplicate_entries}, maximum_policy, budget_output)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{reordered_entries}, maximum_policy, budget_output)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, duplicate_payload, 1),
          maximum_policy, order_storage)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, reordered_payload, 1),
          maximum_policy, order_storage)
              .error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch within-batch ordering failed");
  }

  EntryBatchPolicy prior_policy = maximum_policy;
  prior_policy.has_previous_source_token = true;
  prior_policy.previous_source_token = 5;
  const std::array next_entry{EntryBatchEntry{6, 0, "n"}};
  const std::array replay_entry{EntryBatchEntry{5, 0, "r"}};
  const std::array lower_entry{EntryBatchEntry{4, 0, "l"}};
  const auto next_payload = raw_batch(next_entry);
  const auto replay_payload = raw_batch(replay_entry);
  const auto lower_payload = raw_batch(lower_entry);
  std::array<EntryBatchEntry, 1> cross_batch_storage{};
  if (!ohl::parser::encode_entry_batch_payload(
           EntryBatchMessage{next_entry}, prior_policy, budget_output)
           .valid() ||
      !ohl::parser::decode_entry_batch_payload(
           payload_frame(MessageType::entry_batch, next_payload, 1),
           prior_policy, cross_batch_storage)
           .valid() ||
      ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{replay_entry}, prior_policy, budget_output)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{lower_entry}, prior_policy, budget_output)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, replay_payload, 1),
          prior_policy, cross_batch_storage)
              .error != ProtocolError::noncanonical_value ||
      ohl::parser::decode_entry_batch_payload(
          payload_frame(MessageType::entry_batch, lower_payload, 1),
          prior_policy, cross_batch_storage)
              .error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch cross-batch replay ordering failed");
  }
  auto maximum_prior_policy = maximum_policy;
  maximum_prior_policy.has_previous_source_token = true;
  maximum_prior_policy.previous_source_token =
      std::numeric_limits<std::uint64_t>::max();
  const std::array maximum_token_entry{EntryBatchEntry{
      std::numeric_limits<std::uint64_t>::max(), 0, "m"}};
  if (ohl::parser::encode_entry_batch_payload(
          EntryBatchMessage{maximum_token_entry}, maximum_prior_policy,
          budget_output)
          .error != ProtocolError::noncanonical_value) {
    return fail("typed entry batch accepted replay after maximum token");
  }

  std::array<std::byte, 21> single_payload{};
  const std::array single_entry{EntryBatchEntry{0, 0, "a"}};
  const auto single_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{single_entry}, maximum_policy, single_payload);
  if (!single_encode.valid()) {
    return fail("typed entry batch truncation fixture encoding failed");
  }
  std::array<EntryBatchEntry, 2> atomic_storage;
  for (std::size_t size = 0; size < single_payload.size(); ++size) {
    atomic_storage.fill(sentinel_entry);
    const auto decode = ohl::parser::decode_entry_batch_payload(
        payload_frame(MessageType::entry_batch,
                      std::span<const std::byte>{single_payload}.first(size),
                      1),
        maximum_policy, atomic_storage);
    if (decode.error != ProtocolError::payload_underflow ||
        !is_default(decode) || !all_sentinel(atomic_storage)) {
      return fail("typed entry batch truncated field was not atomic");
    }
  }
  std::array<std::byte, single_payload.size() + 1U> trailing_payload{};
  std::copy(single_payload.begin(), single_payload.end(),
            trailing_payload.begin());
  trailing_payload.back() = std::byte{0xa5};
  atomic_storage.fill(sentinel_entry);
  const auto trailing_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, trailing_payload, 1),
      maximum_policy, atomic_storage);
  if (trailing_decode.error != ProtocolError::payload_trailing_bytes ||
      !is_default(trailing_decode) || !all_sentinel(atomic_storage)) {
    return fail("typed entry batch trailing byte was not rejected atomically");
  }

  const auto two_entry_payload = raw_batch(increasing_entries);
  std::array<EntryBatchEntry, 3> capacity_storage;
  capacity_storage.fill(sentinel_entry);
  const auto short_storage_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, two_entry_payload, 1),
      maximum_policy,
      std::span<EntryBatchEntry>{capacity_storage}.first(1));
  if (short_storage_decode.error != ProtocolError::output_too_small ||
      !is_default(short_storage_decode) || !all_sentinel(capacity_storage)) {
    return fail("short entry storage was not failure atomic");
  }
  capacity_storage.fill(sentinel_entry);
  const auto exact_storage_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, two_entry_payload, 1),
      maximum_policy,
      std::span<EntryBatchEntry>{capacity_storage}.first(2));
  if (!exact_storage_decode.valid() ||
      exact_storage_decode.message.entries.data() != capacity_storage.data() ||
      exact_storage_decode.message.entries.size() != 2 ||
      capacity_storage[0].source_token != 1 ||
      capacity_storage[1].source_token != 2 ||
      capacity_storage[2].archive_path != sentinel_entry.archive_path) {
    return fail("exact entry storage capacity failed");
  }
  capacity_storage.fill(sentinel_entry);
  const auto larger_storage_decode = ohl::parser::decode_entry_batch_payload(
      payload_frame(MessageType::entry_batch, two_entry_payload, 1),
      maximum_policy, capacity_storage);
  if (!larger_storage_decode.valid() ||
      larger_storage_decode.message.entries.data() != capacity_storage.data() ||
      larger_storage_decode.message.entries.size() != 2 ||
      capacity_storage[2].source_token != sentinel_entry.source_token ||
      capacity_storage[2].size_bytes != sentinel_entry.size_bytes ||
      capacity_storage[2].archive_path != sentinel_entry.archive_path) {
    return fail("larger entry storage capacity failed");
  }

  std::vector<std::byte> sentinel_output(two_entry_payload.size(),
                                         std::byte{0xa5});
  const auto unchanged_output = [&sentinel_output]() {
    return std::all_of(sentinel_output.begin(), sentinel_output.end(),
                       [](const std::byte value) {
                         return value == std::byte{0xa5};
                       });
  };
  const auto invalid_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{duplicate_entries}, maximum_policy, sentinel_output);
  if (invalid_encode.error != ProtocolError::noncanonical_value ||
      invalid_encode.bytes_written != 0 || !unchanged_output()) {
    return fail("invalid entry batch encoding changed destination");
  }
  const auto short_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{increasing_entries}, maximum_policy,
      std::span<std::byte>{sentinel_output}.first(
          two_entry_payload.size() - 1U));
  if (short_encode.error != ProtocolError::output_too_small ||
      short_encode.bytes_written != 0 || !unchanged_output()) {
    return fail("short entry batch encoding changed destination");
  }

  constexpr std::size_t kFrameEntryCount = 255;
  const auto exact_path_bytes = ohl::parser::kMaximumFramePayloadBytes -
                                ohl::parser::kEntryBatchPrefixBytes -
                                kFrameEntryCount *
                                    ohl::parser::kEntryBatchEntryPrefixBytes;
  static_assert(exact_path_bytes == 1'043'984);
  std::vector<std::string> frame_paths;
  frame_paths.reserve(kFrameEntryCount);
  for (std::size_t index = 0; index + 1U < kFrameEntryCount; ++index) {
    frame_paths.emplace_back(ohl::parser::kMaximumEntryBatchPathBytes, 'a');
  }
  const auto final_path_bytes =
      exact_path_bytes -
      (kFrameEntryCount - 1U) * ohl::parser::kMaximumEntryBatchPathBytes;
  frame_paths.emplace_back(final_path_bytes, 'b');
  std::vector<EntryBatchEntry> frame_entries;
  frame_entries.reserve(kFrameEntryCount);
  for (std::size_t index = 0; index < frame_paths.size(); ++index) {
    frame_entries.push_back(
        {static_cast<std::uint64_t>(index), 0, frame_paths[index]});
  }
  std::vector<std::byte> frame_output(
      ohl::parser::kMaximumFramePayloadBytes);
  const auto frame_exact_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{frame_entries}, maximum_policy, frame_output);
  if (!frame_exact_encode.valid() ||
      frame_exact_encode.bytes_written !=
          ohl::parser::kMaximumFramePayloadBytes) {
    return fail("exact frame-sized entry batch was rejected");
  }
  frame_paths.back().push_back('c');
  frame_entries.back().archive_path = frame_paths.back();
  std::array<std::byte, 8> frame_sentinel;
  frame_sentinel.fill(std::byte{0xa5});
  const auto frame_over_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{frame_entries}, maximum_policy, frame_sentinel);
  if (frame_over_encode.error != ProtocolError::payload_too_large ||
      frame_over_encode.bytes_written != 0 ||
      !std::all_of(frame_sentinel.begin(), frame_sentinel.end(),
                   [](const std::byte value) {
                     return value == std::byte{0xa5};
                   })) {
    return fail("one-over frame entry batch was not rejected atomically");
  }
  return true;
}

[[nodiscard]] bool test_typed_entry_batch_state_and_batches() {
  const std::array first_entries{EntryBatchEntry{0, 1, "a"},
                                 EntryBatchEntry{2, 2, "b"}};
  const std::array second_entries{
      EntryBatchEntry{3, 3, "c"},
      EntryBatchEntry{std::numeric_limits<std::uint64_t>::max(), 0, "d"},
  };
  EntryBatchPolicy first_policy{
      .remaining_entries = 4,
      .remaining_path_bytes = 4,
      .maximum_entry_bytes = 3,
      .remaining_total_bytes = 6,
  };
  const auto first_policy_before = first_policy;
  std::array<std::byte, 40> first_payload{};
  std::array<std::byte, 40> second_payload{};
  const auto first_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{first_entries}, first_policy, first_payload);
  if (!first_encode.valid() || first_encode.bytes_written != first_payload.size()) {
    return fail("first stateful entry batch fixture failed");
  }

  auto second_policy = first_policy;
  second_policy.remaining_entries -=
      static_cast<std::uint32_t>(first_entries.size());
  second_policy.remaining_path_bytes -= 2;
  second_policy.remaining_total_bytes -= 3;
  second_policy.has_previous_source_token = true;
  second_policy.previous_source_token = first_entries.back().source_token;
  const auto second_policy_before = second_policy;
  const auto second_encode = ohl::parser::encode_entry_batch_payload(
      EntryBatchMessage{second_entries}, second_policy, second_payload);
  if (!second_encode.valid() ||
      second_encode.bytes_written != second_payload.size()) {
    return fail("second stateful entry batch fixture failed");
  }

  ProtocolStateValidator validator{kSession};
  std::array<EntryBatchEntry, 2> first_storage{};
  std::array<EntryBatchEntry, 2> second_storage{};
  if (!handshake(validator) ||
      !observe(validator, MessageDirection::parent_to_worker,
               MessageType::enumerate, 1) ||
      decode_entry_batch_then_observe(
          validator, payload_frame(MessageType::entry_batch, first_payload, 1),
          first_policy, first_storage) != ProtocolError::none ||
      !equal_entry_batch_policy(first_policy, first_policy_before) ||
      validator.state() != SessionState::enumerating ||
      decode_entry_batch_then_observe(
          validator,
          payload_frame(MessageType::entry_batch, second_payload, 1),
          second_policy, second_storage) != ProtocolError::none ||
      !equal_entry_batch_policy(second_policy, second_policy_before) ||
      validator.state() != SessionState::enumerating ||
      validator.message_count() != 5) {
    return fail("multiple typed entry batches did not preserve caller policy");
  }

  const std::array<std::byte, ohl::parser::kCompletePayloadBytes>
      complete_payload{std::byte{0x00}, std::byte{0x00}, std::byte{0x04},
                       std::byte{0x00}};
  if (decode_complete_then_observe(
          validator, payload_frame(MessageType::complete, complete_payload, 1),
          ProtocolPhase::enumerate) != ProtocolError::none ||
      validator.state() != SessionState::idle ||
      validator.message_count() != 6) {
    return fail("multiple typed entry batches did not complete enumeration");
  }

  ProtocolStateValidator invalid_validator{kSession};
  auto replay_policy = maximum_entry_batch_policy();
  replay_policy.has_previous_source_token = true;
  replay_policy.previous_source_token = 0;
  const auto replay_policy_before = replay_policy;
  const EntryBatchEntry sentinel{
      0xa5a5'a5a5'a5a5'a5a5ULL, 0xa5a5'a5a5'a5a5'a5a5ULL, "sentinel"};
  std::array<EntryBatchEntry, 2> invalid_storage;
  invalid_storage.fill(sentinel);
  if (!handshake(invalid_validator) ||
      !observe(invalid_validator, MessageDirection::parent_to_worker,
               MessageType::enumerate, 1) ||
      decode_entry_batch_then_observe(
          invalid_validator,
          payload_frame(MessageType::entry_batch, first_payload, 1),
          replay_policy, invalid_storage) !=
          ProtocolError::noncanonical_value ||
      invalid_validator.state() != SessionState::enumerating ||
      invalid_validator.message_count() != 3 ||
      invalid_validator.error() != ProtocolError::none ||
      !equal_entry_batch_policy(replay_policy, replay_policy_before) ||
      !std::all_of(invalid_storage.begin(), invalid_storage.end(),
                   [&sentinel](const EntryBatchEntry& entry) {
                     return entry.source_token == sentinel.source_token &&
                            entry.size_bytes == sentinel.size_bytes &&
                            entry.archive_path == sentinel.archive_path;
                   })) {
    return fail("invalid entry batch changed state, policy, or storage");
  }
  return true;
}

[[nodiscard]] bool test_typed_data_chunk() {
  static_assert(ohl::parser::kMaximumDataChunkBytes == 256U * 1'024U);

  const auto empty_result = [](const auto& result) {
    return result.message.data.data() == nullptr &&
           result.message.data.empty();
  };
  const std::array one_byte{std::byte{0x5a}};
  std::array<std::byte, one_byte.size()> minimum_output{};
  const auto minimum_encode = ohl::parser::encode_data_chunk_payload(
      DataChunkMessage{one_byte}, one_byte.size(), minimum_output);
  const auto minimum_frame =
      payload_frame(MessageType::data_chunk, minimum_output, 1);
  const auto minimum_decode = ohl::parser::decode_data_chunk_payload(
      minimum_frame, minimum_output.size());
  // DataChunkMessage is a non-owning view. Keep minimum_output alive while
  // checking that the decoded span preserves the frame payload's identity.
  if (!minimum_encode.valid() ||
      minimum_encode.bytes_written != minimum_output.size() ||
      minimum_output != one_byte || !minimum_decode.valid() ||
      minimum_decode.message.data.data() != minimum_frame.payload.data() ||
      minimum_decode.message.data.size() != minimum_frame.payload.size()) {
    return fail("typed data chunk minimum round trip failed");
  }

  std::vector<std::byte> maximum_data(ohl::parser::kMaximumDataChunkBytes,
                                      std::byte{0xa6});
  std::vector<std::byte> maximum_output(ohl::parser::kMaximumDataChunkBytes);
  const auto maximum_encode = ohl::parser::encode_data_chunk_payload(
      DataChunkMessage{maximum_data}, maximum_data.size(), maximum_output);
  const auto maximum_frame =
      payload_frame(MessageType::data_chunk, maximum_output, 1);
  const auto maximum_decode = ohl::parser::decode_data_chunk_payload(
      maximum_frame, maximum_output.size());
  if (!maximum_encode.valid() ||
      maximum_encode.bytes_written != maximum_output.size() ||
      !equal_bytes(maximum_output, maximum_data) || !maximum_decode.valid() ||
      maximum_decode.message.data.data() != maximum_frame.payload.data() ||
      maximum_decode.message.data.size() != maximum_frame.payload.size()) {
    return fail("typed data chunk maximum round trip failed");
  }

  const std::array two_bytes{std::byte{0x12}, std::byte{0x34}};
  const auto exact_remainder = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::data_chunk, two_bytes, 1), two_bytes.size());
  const auto short_remainder = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::data_chunk, two_bytes, 1),
      two_bytes.size() - 1U);
  const auto zero_remainder = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::data_chunk, two_bytes, 1), 0);
  if (!exact_remainder.valid() ||
      exact_remainder.message.data.data() != two_bytes.data() ||
      exact_remainder.message.data.size() != two_bytes.size() ||
      short_remainder.error != ProtocolError::noncanonical_value ||
      !empty_result(short_remainder) ||
      zero_remainder.error != ProtocolError::invalid_budget ||
      !empty_result(zero_remainder)) {
    return fail("typed data chunk remainder validation failed");
  }

  const auto empty_decode = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::data_chunk, {}, 1), 1);
  std::vector<std::byte> oversized_data(
      ohl::parser::kMaximumDataChunkBytes + 1U, std::byte{0x5c});
  const auto oversized_decode = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::data_chunk, oversized_data, 1),
      oversized_data.size());
  if (empty_decode.error != ProtocolError::noncanonical_value ||
      !empty_result(empty_decode) ||
      oversized_decode.error != ProtocolError::noncanonical_value ||
      !empty_result(oversized_decode)) {
    return fail("typed data chunk size validation failed");
  }

  auto declared_longer =
      payload_frame(MessageType::data_chunk, two_bytes, 1);
  auto declared_shorter =
      payload_frame(MessageType::data_chunk, two_bytes, 1);
  ++declared_longer.header.payload_length;
  --declared_shorter.header.payload_length;
  const auto truncated =
      ohl::parser::decode_data_chunk_payload(declared_longer, 0);
  const auto trailing =
      ohl::parser::decode_data_chunk_payload(declared_shorter, 0);
  auto invalid_header =
      payload_frame(MessageType::data_chunk, two_bytes, 1);
  invalid_header.header.flags = 1;
  const auto bad_header =
      ohl::parser::decode_data_chunk_payload(invalid_header, 0);
  const auto wrong_type = ohl::parser::decode_data_chunk_payload(
      payload_frame(MessageType::read_reply, two_bytes, 1), 0);
  const FrameView prior_error{
      .error = ProtocolError::payload_too_large,
      .header = {},
      .payload = two_bytes,
  };
  const auto inherited_error =
      ohl::parser::decode_data_chunk_payload(prior_error, 0);
  if (truncated.error != ProtocolError::truncated_payload ||
      !empty_result(truncated) ||
      trailing.error != ProtocolError::trailing_bytes ||
      !empty_result(trailing) ||
      bad_header.error != ProtocolError::reserved_flags ||
      !empty_result(bad_header) ||
      wrong_type.error != ProtocolError::unexpected_message ||
      !empty_result(wrong_type) ||
      inherited_error.error != ProtocolError::payload_too_large ||
      !empty_result(inherited_error)) {
    return fail("typed data chunk frame validation was not failure atomic");
  }

  std::array<std::byte, two_bytes.size()> destination;
  const auto unchanged = [&destination]() {
    return std::all_of(destination.begin(), destination.end(),
                       [](const std::byte value) {
                         return value == std::byte{0xa5};
                       });
  };
  const auto rejects_atomically = [&](const DataChunkMessage message,
                                      const std::uint64_t remainder,
                                      const std::span<std::byte> output,
                                      const ProtocolError expected) {
    destination.fill(std::byte{0xa5});
    const auto result = ohl::parser::encode_data_chunk_payload(
        message, remainder, output);
    return result.error == expected && result.bytes_written == 0 &&
           unchanged();
  };
  if (!rejects_atomically(DataChunkMessage{}, 1, destination,
                          ProtocolError::noncanonical_value) ||
      !rejects_atomically(DataChunkMessage{oversized_data},
                          oversized_data.size(), destination,
                          ProtocolError::noncanonical_value) ||
      !rejects_atomically(DataChunkMessage{two_bytes}, two_bytes.size() - 1U,
                          destination, ProtocolError::noncanonical_value) ||
      !rejects_atomically(DataChunkMessage{two_bytes}, 0, destination,
                          ProtocolError::invalid_budget) ||
      !rejects_atomically(DataChunkMessage{two_bytes}, two_bytes.size(),
                          std::span<std::byte>{destination}.first(1),
                          ProtocolError::output_too_small)) {
    return fail("typed data chunk encoding was not failure atomic");
  }
  return true;
}

[[nodiscard]] bool test_typed_complete() {
  static_assert(ohl::parser::kCompletePayloadBytes == 4);

  constexpr CompleteMessage canonical{ProtocolStatus::ok,
                                      ProtocolPhase::complete};
  const std::array<std::byte, ohl::parser::kCompletePayloadBytes>
      canonical_bytes{std::byte{0x00}, std::byte{0x00}, std::byte{0x04},
                      std::byte{0x00}};
  constexpr std::array valid_contexts{ProtocolPhase::enumerate,
                                      ProtocolPhase::stream};
  constexpr std::array statuses{
      ProtocolStatus::ok,
      ProtocolStatus::unsupported,
      ProtocolStatus::invalid_request,
      ProtocolStatus::parser_rejected,
      ProtocolStatus::budget_exceeded,
      ProtocolStatus::cancelled,
      ProtocolStatus::source_changed,
      ProtocolStatus::source_read_failed,
      ProtocolStatus::result_validation_failed,
      ProtocolStatus::internal_failure,
  };
  constexpr std::array phases{
      ProtocolPhase::handshake, ProtocolPhase::enumerate,
      ProtocolPhase::stream, ProtocolPhase::source_read,
      ProtocolPhase::complete,
  };
  static_assert(statuses.size() == 10);
  static_assert(phases.size() == 5);
  const auto is_default = [](const auto& result) {
    return result.message.status == ProtocolStatus::internal_failure &&
           result.message.phase == ProtocolPhase::handshake;
  };
  const auto all_sentinel = [](const auto& destination) {
    return std::all_of(destination.begin(), destination.end(),
                       [](const std::byte value) {
                         return value == std::byte{0xa5};
                       });
  };

  for (const auto context : valid_contexts) {
    std::array<std::byte, ohl::parser::kCompletePayloadBytes> encoded{};
    const auto encode =
        ohl::parser::encode_complete_payload(canonical, context, encoded);
    const auto decode = ohl::parser::decode_complete_payload(
        payload_frame(MessageType::complete, encoded, 1), context);
    if (!encode.valid() || encode.bytes_written != encoded.size() ||
        encoded != canonical_bytes || !decode.valid() ||
        decode.message.status != canonical.status ||
        decode.message.phase != canonical.phase) {
      return fail("typed complete canonical encoding failed");
    }

    for (const auto status : statuses) {
      for (const auto phase : phases) {
        std::array<std::byte, ohl::parser::kCompletePayloadBytes> payload{};
        PayloadWriter writer{payload};
        if (!writer.write_status(status) || !writer.write_phase(phase)) {
          return fail("typed complete matrix fixture encoding failed");
        }

        std::array<std::byte, ohl::parser::kCompletePayloadBytes>
            destination;
        destination.fill(std::byte{0xa5});
        const auto matrix_encode = ohl::parser::encode_complete_payload(
            CompleteMessage{status, phase}, context, destination);
        const auto matrix_decode = ohl::parser::decode_complete_payload(
            payload_frame(MessageType::complete, payload, 1), context);
        const auto allowed = status == ProtocolStatus::ok &&
                             phase == ProtocolPhase::complete;
        if (allowed) {
          if (!matrix_encode.valid() ||
              matrix_encode.bytes_written != destination.size() ||
              destination != canonical_bytes || !matrix_decode.valid() ||
              matrix_decode.message.status != status ||
              matrix_decode.message.phase != phase) {
            return fail("typed complete allowed matrix pair was rejected");
          }
        } else if (matrix_encode.error !=
                       ProtocolError::noncanonical_value ||
                   matrix_encode.bytes_written != 0 ||
                   !all_sentinel(destination) ||
                   matrix_decode.error !=
                       ProtocolError::noncanonical_value ||
                   !is_default(matrix_decode)) {
          return fail("typed complete disallowed matrix pair was accepted");
        }
      }
    }
  }

  const std::array<std::byte, ohl::parser::kCompletePayloadBytes>
      unknown_status{std::byte{0xff}, std::byte{0xff}, std::byte{0x04},
                     std::byte{0x00}};
  const std::array<std::byte, ohl::parser::kCompletePayloadBytes> unknown_phase{
      std::byte{0x00}, std::byte{0x00}, std::byte{0xff}, std::byte{0xff}};
  for (const auto context : valid_contexts) {
    std::array<std::byte, ohl::parser::kCompletePayloadBytes> destination;
    destination.fill(std::byte{0xa5});
    const auto encode_unknown_status = ohl::parser::encode_complete_payload(
        {static_cast<ProtocolStatus>(0xffffU), ProtocolPhase::complete},
        context, destination);
    if (encode_unknown_status.error != ProtocolError::noncanonical_value ||
        encode_unknown_status.bytes_written != 0 ||
        !all_sentinel(destination)) {
      return fail("typed complete unknown status was encoded");
    }
    destination.fill(std::byte{0xa5});
    const auto encode_unknown_phase = ohl::parser::encode_complete_payload(
        {ProtocolStatus::ok, static_cast<ProtocolPhase>(0xffffU)}, context,
        destination);
    const auto decode_unknown_status = ohl::parser::decode_complete_payload(
        payload_frame(MessageType::complete, unknown_status, 1), context);
    const auto decode_unknown_phase = ohl::parser::decode_complete_payload(
        payload_frame(MessageType::complete, unknown_phase, 1), context);
    if (encode_unknown_phase.error != ProtocolError::noncanonical_value ||
        encode_unknown_phase.bytes_written != 0 ||
        !all_sentinel(destination) ||
        decode_unknown_status.error != ProtocolError::noncanonical_value ||
        !is_default(decode_unknown_status) ||
        decode_unknown_phase.error != ProtocolError::noncanonical_value ||
        !is_default(decode_unknown_phase)) {
      return fail("typed complete unknown enum value was accepted");
    }
  }

  constexpr std::array invalid_contexts{
      ProtocolPhase::handshake,
      ProtocolPhase::source_read,
      ProtocolPhase::complete,
      static_cast<ProtocolPhase>(0xffffU),
  };
  for (const auto context : invalid_contexts) {
    std::array<std::byte, ohl::parser::kCompletePayloadBytes> destination;
    destination.fill(std::byte{0xa5});
    const auto encode =
        ohl::parser::encode_complete_payload(canonical, context, destination);
    const auto decode = ohl::parser::decode_complete_payload(
        payload_frame(MessageType::complete, canonical_bytes, 1), context);
    if (encode.error != ProtocolError::invalid_budget ||
        encode.bytes_written != 0 || !all_sentinel(destination) ||
        decode.error != ProtocolError::invalid_budget ||
        !is_default(decode)) {
      return fail("typed complete invalid operation context was accepted");
    }
  }

  std::array<std::byte, ohl::parser::kCompletePayloadBytes + 1> shaped_payload{
      std::byte{0x00}, std::byte{0x00}, std::byte{0x04}, std::byte{0x00},
      std::byte{0xa5}};
  for (const auto context : valid_contexts) {
    for (std::size_t size = 0; size <= shaped_payload.size(); ++size) {
      const auto decode = ohl::parser::decode_complete_payload(
          payload_frame(MessageType::complete,
                        std::span<const std::byte>{shaped_payload}.first(size),
                        1),
          context);
      if (size == ohl::parser::kCompletePayloadBytes) {
        if (!decode.valid() || decode.message.status != canonical.status ||
            decode.message.phase != canonical.phase) {
          return fail("typed complete exact payload size was rejected");
        }
      } else {
        const auto expected =
            size < ohl::parser::kCompletePayloadBytes
                ? ProtocolError::payload_underflow
                : ProtocolError::payload_trailing_bytes;
        if (decode.error != expected || !is_default(decode)) {
          return fail("typed complete payload size was accepted");
        }
      }
    }
  }

  std::array<std::byte, ohl::parser::kCompletePayloadBytes + 1> destination;
  for (std::size_t size = 0; size <= destination.size(); ++size) {
    destination.fill(std::byte{0xa5});
    const auto encode = ohl::parser::encode_complete_payload(
        canonical, ProtocolPhase::enumerate,
        std::span<std::byte>{destination}.first(size));
    if (size < ohl::parser::kCompletePayloadBytes) {
      if (encode.error != ProtocolError::output_too_small ||
          encode.bytes_written != 0 || !all_sentinel(destination)) {
        return fail("short complete output changed its destination");
      }
    } else if (!encode.valid() ||
               encode.bytes_written != ohl::parser::kCompletePayloadBytes ||
               !std::equal(canonical_bytes.begin(), canonical_bytes.end(),
                           destination.begin()) ||
               destination.back() != std::byte{0xa5}) {
      return fail("typed complete output size handling failed");
    }
  }

  auto declared_longer =
      payload_frame(MessageType::complete, canonical_bytes, 1);
  auto declared_shorter =
      payload_frame(MessageType::complete, canonical_bytes, 1);
  auto invalid_flags =
      payload_frame(MessageType::complete, canonical_bytes, 1);
  ++declared_longer.header.payload_length;
  --declared_shorter.header.payload_length;
  invalid_flags.header.flags = 1;
  const FrameView prior_error{
      .error = ProtocolError::payload_too_large,
      .header = {},
      .payload = canonical_bytes,
  };
  const auto truncated = ohl::parser::decode_complete_payload(
      declared_longer, ProtocolPhase::handshake);
  const auto trailing = ohl::parser::decode_complete_payload(
      declared_shorter, ProtocolPhase::handshake);
  const auto bad_flags = ohl::parser::decode_complete_payload(
      invalid_flags, ProtocolPhase::handshake);
  const auto wrong_type = ohl::parser::decode_complete_payload(
      payload_frame(MessageType::data_chunk, canonical_bytes, 1),
      ProtocolPhase::handshake);
  const auto inherited_error = ohl::parser::decode_complete_payload(
      prior_error, ProtocolPhase::handshake);
  if (truncated.error != ProtocolError::truncated_payload ||
      !is_default(truncated) ||
      trailing.error != ProtocolError::trailing_bytes ||
      !is_default(trailing) ||
      bad_flags.error != ProtocolError::reserved_flags ||
      !is_default(bad_flags) ||
      wrong_type.error != ProtocolError::unexpected_message ||
      !is_default(wrong_type) ||
      inherited_error.error != ProtocolError::payload_too_large ||
      !is_default(inherited_error)) {
    return fail("typed complete frame validation was not failure atomic");
  }

  const std::array<std::byte, ohl::parser::kCompletePayloadBytes>
      invalid_payload{std::byte{0x01}, std::byte{0x00}, std::byte{0x04},
                      std::byte{0x00}};
  for (const auto context : valid_contexts) {
    const auto operation = context == ProtocolPhase::enumerate
                               ? MessageType::enumerate
                               : MessageType::stream_entry;
    const auto state = context == ProtocolPhase::enumerate
                           ? SessionState::enumerating
                           : SessionState::streaming;
    const auto result = context == ProtocolPhase::enumerate
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;

    ProtocolStateValidator normal{kSession};
    if (!handshake(normal) ||
        !observe(normal, MessageDirection::parent_to_worker, operation, 1) ||
        decode_complete_then_observe(
            normal, payload_frame(MessageType::complete, invalid_payload, 1),
            context) != ProtocolError::noncanonical_value ||
        normal.state() != state || normal.message_count() != 3 ||
        normal.error() != ProtocolError::none ||
        decode_complete_then_observe(
            normal, payload_frame(MessageType::complete, canonical_bytes, 1),
            context) != ProtocolError::none ||
        normal.state() != SessionState::idle || normal.message_count() != 4) {
      return fail("complete dispatch observed an invalid typed payload");
    }

    ProtocolStateValidator cancel_first{kSession};
    if (!handshake(cancel_first) ||
        !observe(cancel_first, MessageDirection::parent_to_worker, operation,
                 1) ||
        !observe(cancel_first, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        !observe(cancel_first, MessageDirection::worker_to_parent, result, 1) ||
        decode_complete_then_observe(
            cancel_first,
            payload_frame(MessageType::complete, canonical_bytes, 1),
            context) != ProtocolError::none ||
        cancel_first.state() != SessionState::idle) {
      return fail("typed completion did not win cancel-first race");
    }

    ProtocolStateValidator complete_first{kSession};
    if (!handshake(complete_first) ||
        !observe(complete_first, MessageDirection::parent_to_worker, operation,
                 1) ||
        decode_complete_then_observe(
            complete_first,
            payload_frame(MessageType::complete, canonical_bytes, 1),
            context) != ProtocolError::none ||
        !observe(complete_first, MessageDirection::parent_to_worker,
                 MessageType::cancel, 1) ||
        complete_first.state() != SessionState::idle) {
      return fail("typed completion did not win complete-first race");
    }
  }
  return true;
}

[[nodiscard]] bool test_typed_failure_atomicity_and_ordering() {
  const auto unchanged = [](const auto& bytes) {
    return std::all_of(bytes.begin(), bytes.end(), [](const std::byte value) {
      return value == std::byte{0xa5};
    });
  };
  std::array<std::byte, 32> output;
  output.fill(std::byte{0xa5});
  if (ohl::parser::encode_hello_payload({0, 1}, output).bytes_written != 0 ||
      !unchanged(output)) {
    return fail("invalid hello encoding changed destination");
  }
  if (ohl::parser::encode_hello_payload(
          {1, 1}, std::span<std::byte>{output}.first(
                      ohl::parser::kHelloPayloadBytes - 1))
          .error != ProtocolError::output_too_small ||
      !unchanged(output)) {
    return fail("short hello encoding changed destination");
  }
  if (ohl::parser::encode_read_request_payload(
          {1, 0, 0}, {1, 1}, 1, output)
          .bytes_written != 0 ||
      !unchanged(output) ||
      ohl::parser::encode_read_request_payload(
          {1, 0, 1}, {1, 1}, 1,
          std::span<std::byte>{output}.first(
              ohl::parser::kReadRequestPayloadBytes - 1))
              .error != ProtocolError::output_too_small ||
      !unchanged(output)) {
    return fail("read request encoding was not failure atomic");
  }
  const std::array data{std::byte{1}};
  if (ohl::parser::encode_read_reply_payload(
          {1, ProtocolStatus::cancelled, {}}, 1, 1, output)
          .bytes_written != 0 ||
      !unchanged(output) ||
      ohl::parser::encode_read_reply_payload(
          {1, ProtocolStatus::ok, data}, 1, 1,
          std::span<std::byte>{output}.first(
              ohl::parser::kReadReplyPrefixBytes))
              .error != ProtocolError::output_too_small ||
      !unchanged(output)) {
    return fail("read reply encoding was not failure atomic");
  }
  std::vector<std::byte> oversized_reply_data(
      ohl::parser::kMaximumReadBytes + 1U, std::byte{1});
  if (ohl::parser::encode_read_reply_payload(
          {1, ProtocolStatus::source_changed, oversized_reply_data}, 1, 1,
          output)
          .error != ProtocolError::payload_too_large ||
      !unchanged(output)) {
    return fail("oversized read reply did not fail atomically");
  }

  const FrameView prior_error{
      .error = ProtocolError::truncated_payload,
      .header = {},
      .payload = {},
  };
  if (ohl::parser::decode_hello_payload(prior_error).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_ready_payload(prior_error).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_read_request_payload(prior_error, {1, 1}, 1).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_read_reply_payload(prior_error, 1, 1).error !=
          ProtocolError::truncated_payload) {
    return fail("typed decoder did not preserve frame error");
  }
  std::vector<std::byte> oversized(ohl::parser::kMaximumFramePayloadBytes + 1);
  if (ohl::parser::decode_ready_payload(
          payload_frame(MessageType::ready, oversized))
          .error != ProtocolError::payload_too_large) {
    return fail("typed decoder did not preserve global payload ceiling");
  }

  std::array<std::byte, ohl::parser::kHelloPayloadBytes> valid_hello{};
  PayloadWriter writer{valid_hello};
  (void)writer.write_u64(1);
  (void)writer.write_u32(1);

  auto invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.major_version =
      ohl::parser::kProtocolMajorVersion + 1U;
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::unsupported_version) {
    return fail("typed decoder accepted invalid header version");
  }
  invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.flags = 1;
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::reserved_flags) {
    return fail("typed decoder accepted reserved header flags");
  }
  invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.session_id = 0;
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::invalid_session_id) {
    return fail("typed decoder accepted zero header session");
  }
  invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.request_id = 1;
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::invalid_request_id) {
    return fail("typed decoder accepted invalid header request id");
  }
  invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.type = static_cast<MessageType>(0xffffU);
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::unknown_message_type) {
    return fail("typed decoder accepted unknown header message type");
  }
  invalid_header = payload_frame(MessageType::hello, valid_hello);
  invalid_header.header.payload_length =
      ohl::parser::kMaximumFramePayloadBytes + 1U;
  if (ohl::parser::decode_hello_payload(invalid_header).error !=
      ProtocolError::payload_too_large) {
    return fail("typed decoder accepted oversized declared payload");
  }

  auto truncated = payload_frame(MessageType::hello, valid_hello);
  ++truncated.header.payload_length;
  auto trailing = payload_frame(MessageType::hello, valid_hello);
  --trailing.header.payload_length;
  if (ohl::parser::decode_hello_payload(truncated).error !=
          ProtocolError::truncated_payload ||
      ohl::parser::decode_hello_payload(trailing).error !=
          ProtocolError::trailing_bytes) {
    return fail("typed hello accepted header/payload length mismatch");
  }
  const std::array<std::byte, ohl::parser::kReadRequestPayloadBytes>
      request_payload{};
  auto invalid_request_header =
      payload_frame(MessageType::read_request, request_payload, 1);
  invalid_request_header.header.request_id = 0;
  if (ohl::parser::decode_read_request_payload(
          invalid_request_header, {1, 1}, 1)
          .error != ProtocolError::invalid_request_id) {
    return fail("typed request decoder accepted invalid header request id");
  }
  ++invalid_request_header.header.payload_length;
  invalid_request_header.header.flags = 1;
  if (ohl::parser::decode_read_request_payload(
          invalid_request_header, {1, 1}, 1)
          .error != ProtocolError::reserved_flags) {
    return fail("invalid header did not precede payload mismatch");
  }
  invalid_request_header =
      payload_frame(MessageType::read_request, request_payload, 1);
  ++invalid_request_header.header.payload_length;
  if (ohl::parser::decode_read_request_payload(
          invalid_request_header, {1, 1}, 1)
          .error != ProtocolError::truncated_payload) {
    return fail("typed request accepted header/payload length mismatch");
  }
  const std::array<std::byte, ohl::parser::kReadReplyPrefixBytes>
      reply_payload{};
  auto reply_trailing =
      payload_frame(MessageType::read_reply, reply_payload, 1);
  --reply_trailing.header.payload_length;
  if (ohl::parser::decode_read_reply_payload(reply_trailing, 1, 1).error !=
      ProtocolError::trailing_bytes) {
    return fail("typed reply accepted header/payload length mismatch");
  }
  auto ready_truncated = payload_frame(MessageType::ready, {});
  ready_truncated.header.payload_length = 1;
  if (ohl::parser::decode_ready_payload(ready_truncated).error !=
      ProtocolError::truncated_payload) {
    return fail("typed ready accepted header/payload length mismatch");
  }

  std::array<std::byte, ohl::parser::kHelloPayloadBytes> invalid_hello{};
  PayloadWriter invalid_writer{invalid_hello};
  (void)invalid_writer.write_u64(0);
  (void)invalid_writer.write_u32(1);
  ProtocolStateValidator validator{kSession};
  if (decode_hello_then_observe(
          validator, payload_frame(MessageType::hello, invalid_hello)) !=
          ProtocolError::noncanonical_value ||
      validator.state() != SessionState::awaiting_hello ||
      validator.message_count() != 0) {
    return fail("dispatch observed a message before typed acceptance");
  }
  if (decode_hello_then_observe(
          validator, payload_frame(MessageType::hello, valid_hello)) !=
          ProtocolError::none ||
      validator.state() != SessionState::awaiting_ready ||
      validator.message_count() != 1) {
    return fail("dispatch did not observe a typed-valid message");
  }
  return true;
}

}  // namespace

int main() {
  static_assert(ohl::parser::kFrameHeaderBytes == 32);
  static_assert(ohl::parser::kMaximumFramePayloadBytes == 1U * 1'024U * 1'024U);
  static_assert(ohl::parser::kMaximumProtocolMessages == 1'048'576U);
  static_assert(ohl::parser::kMaximumCumulativePayloadBytes ==
                64ULL * 1'024ULL * 1'024ULL * 1'024ULL);

  return test_header_encoding() && test_frame_round_trip() &&
                 test_frame_encode_rejections() &&
                 test_frame_rejections() && test_maximum_frame() &&
                 test_payload_codec() && test_payload_rejections() &&
                 test_valid_state_sequence() && test_state_rejections() &&
                 test_crossed_completion_and_cancellation() &&
                 test_unresolved_reads_block_crossed_completion() &&
                 test_late_read_reply_drain() &&
                 test_direction_and_active_terminal_rejections() &&
                 test_cancelling_crossed_traffic_rejections() &&
                 test_terminal_and_cancel_rejections() &&
                 test_cancellation_and_budgets() &&
                 test_typed_hello_and_ready() &&
                 test_typed_exact_empty_messages() &&
                 test_typed_stream_entry() &&
                 test_typed_read_request() && test_typed_read_reply() &&
                 test_typed_entry_batch_wire_and_bounds() &&
                 test_typed_entry_batch_policy_and_atomicity() &&
                 test_typed_entry_batch_state_and_batches() &&
                 test_typed_data_chunk() &&
                 test_typed_complete() &&
                 test_typed_failure_atomicity_and_ordering()
             ? 0
             : 1;
}
