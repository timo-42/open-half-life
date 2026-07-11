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
                          frame(MessageType::hello)) !=
            ProtocolError::terminal_state) {
      return fail("closed state accepted another message");
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
  return true;
}

[[nodiscard]] bool test_payload_rejections() {
  std::array<std::byte, 1> small{};
  PayloadWriter writer{small};
  if (writer.write_u16(1) ||
      writer.error() != ProtocolError::output_too_small) {
    return fail("payload writer one-over was accepted");
  }

  const std::array invalid_bool{std::byte{2}};
  PayloadReader bool_reader{invalid_bool};
  bool boolean = false;
  if (bool_reader.read_bool(boolean) ||
      bool_reader.error() != ProtocolError::noncanonical_value) {
    return fail("noncanonical boolean was accepted");
  }
  const std::array invalid_status{std::byte{0xff}, std::byte{0xff}};
  PayloadReader status_reader{invalid_status};
  ProtocolStatus status = ProtocolStatus::ok;
  if (status_reader.read_status(status) ||
      status_reader.error() != ProtocolError::noncanonical_value) {
    return fail("unknown status was accepted");
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
                 test_frame_rejections() && test_maximum_frame() &&
                 test_payload_codec() && test_payload_rejections() &&
                 test_valid_state_sequence() && test_state_rejections() &&
                 test_terminal_and_cancel_rejections() &&
                 test_cancellation_and_budgets()
             ? 0
             : 1;
}
