#include "ohl/parser/protocol.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <limits>
#include <span>
#include <string_view>
#include <vector>

namespace {

using ohl::parser::FrameHeader;
using ohl::parser::MessageDirection;
using ohl::parser::MessageType;
using ohl::parser::PayloadReader;
using ohl::parser::PayloadWriter;
using ohl::parser::ProtocolBudgets;
using ohl::parser::ProtocolError;
using ohl::parser::ProtocolPhase;
using ohl::parser::ProtocolStateValidator;
using ohl::parser::ProtocolStatus;
using ohl::parser::SessionState;

constexpr std::uint64_t kSession = 0x0102'0304'0506'0708ULL;

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
                 test_cancellation_and_budgets()
             ? 0
             : 1;
}
