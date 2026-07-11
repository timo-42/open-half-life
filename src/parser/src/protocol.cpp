#include "ohl/parser/protocol.hpp"

#include <algorithm>
#include <array>
#include <limits>

namespace ohl::parser {
namespace {

constexpr std::array<std::byte, 4> kMagic{
    std::byte{'O'}, std::byte{'H'}, std::byte{'L'}, std::byte{'P'}};

[[nodiscard]] constexpr bool zero_request_type(
    const MessageType type) noexcept {
  return type == MessageType::hello || type == MessageType::ready ||
         type == MessageType::shutdown;
}

void store_u16(const std::span<std::byte> output, const std::size_t offset,
               const std::uint16_t value) noexcept {
  output[offset] = static_cast<std::byte>(value & 0xffU);
  output[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

void store_u32(const std::span<std::byte> output, const std::size_t offset,
               const std::uint32_t value) noexcept {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

void store_u64(const std::span<std::byte> output, const std::size_t offset,
               const std::uint64_t value) noexcept {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

[[nodiscard]] std::uint16_t load_u16(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  return static_cast<std::uint16_t>(
      std::to_integer<std::uint16_t>(input[offset]) |
      static_cast<std::uint16_t>(
          std::to_integer<std::uint16_t>(input[offset + 1]) << 8U));
}

[[nodiscard]] std::uint32_t load_u32(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  std::uint32_t result = 0;
  for (std::size_t index = 0; index < sizeof(result); ++index) {
    result |= std::to_integer<std::uint32_t>(input[offset + index])
              << static_cast<unsigned int>(index * 8U);
  }
  return result;
}

[[nodiscard]] std::uint64_t load_u64(
    const std::span<const std::byte> input,
    const std::size_t offset) noexcept {
  std::uint64_t result = 0;
  for (std::size_t index = 0; index < sizeof(result); ++index) {
    result |= std::to_integer<std::uint64_t>(input[offset + index])
              << static_cast<unsigned int>(index * 8U);
  }
  return result;
}

}  // namespace

std::string_view to_string(const ProtocolError error) noexcept {
  switch (error) {
    case ProtocolError::none:
      return "none";
    case ProtocolError::truncated_header:
      return "truncated header";
    case ProtocolError::invalid_magic:
      return "invalid magic";
    case ProtocolError::unsupported_version:
      return "unsupported version";
    case ProtocolError::unknown_message_type:
      return "unknown message type";
    case ProtocolError::reserved_flags:
      return "reserved flags";
    case ProtocolError::payload_too_large:
      return "payload too large";
    case ProtocolError::truncated_payload:
      return "truncated payload";
    case ProtocolError::trailing_bytes:
      return "trailing bytes";
    case ProtocolError::invalid_session_id:
      return "invalid session id";
    case ProtocolError::invalid_request_id:
      return "invalid request id";
    case ProtocolError::wrong_session_id:
      return "wrong session id";
    case ProtocolError::wrong_request_id:
      return "wrong request id";
    case ProtocolError::request_id_not_monotonic:
      return "request id not monotonic";
    case ProtocolError::unexpected_message:
      return "unexpected message";
    case ProtocolError::request_already_active:
      return "request already active";
    case ProtocolError::read_already_active:
      return "read already active";
    case ProtocolError::no_read_in_flight:
      return "no read in flight";
    case ProtocolError::message_budget_exceeded:
      return "message budget exceeded";
    case ProtocolError::byte_budget_exceeded:
      return "byte budget exceeded";
    case ProtocolError::invalid_budget:
      return "invalid budget";
    case ProtocolError::output_too_small:
      return "output too small";
    case ProtocolError::payload_underflow:
      return "payload underflow";
    case ProtocolError::payload_trailing_bytes:
      return "payload trailing bytes";
    case ProtocolError::noncanonical_value:
      return "noncanonical value";
    case ProtocolError::terminal_state:
      return "terminal state";
  }
  return "protocol error";
}

ProtocolError validate_frame_header(const FrameHeader& header) noexcept {
  if (header.major_version != kProtocolMajorVersion ||
      header.minor_version != kProtocolMinorVersion) {
    return ProtocolError::unsupported_version;
  }
  if (!known_message_type(header.type)) {
    return ProtocolError::unknown_message_type;
  }
  if (header.flags != 0) {
    return ProtocolError::reserved_flags;
  }
  if (header.payload_length > kMaximumFramePayloadBytes) {
    return ProtocolError::payload_too_large;
  }
  if (header.session_id == 0) {
    return ProtocolError::invalid_session_id;
  }
  if ((zero_request_type(header.type) && header.request_id != 0) ||
      (!zero_request_type(header.type) && header.request_id == 0)) {
    return ProtocolError::invalid_request_id;
  }
  return ProtocolError::none;
}

ProtocolError encode_frame_header(
    const FrameHeader& header,
    const std::span<std::byte, kFrameHeaderBytes> destination) noexcept {
  const auto error = validate_frame_header(header);
  if (error != ProtocolError::none) {
    return error;
  }
  std::copy(kMagic.begin(), kMagic.end(), destination.begin());
  store_u16(destination, 4, header.major_version);
  store_u16(destination, 6, header.minor_version);
  store_u16(destination, 8, static_cast<std::uint16_t>(header.type));
  store_u16(destination, 10, header.flags);
  store_u32(destination, 12, header.payload_length);
  store_u64(destination, 16, header.session_id);
  store_u64(destination, 24, header.request_id);
  return ProtocolError::none;
}

HeaderDecodeResult decode_frame_header(
    const std::span<const std::byte, kFrameHeaderBytes> bytes) noexcept {
  HeaderDecodeResult result;
  if (!std::equal(kMagic.begin(), kMagic.end(), bytes.begin())) {
    result.error = ProtocolError::invalid_magic;
    return result;
  }
  result.header.major_version = load_u16(bytes, 4);
  result.header.minor_version = load_u16(bytes, 6);
  result.header.type = static_cast<MessageType>(load_u16(bytes, 8));
  result.header.flags = load_u16(bytes, 10);
  result.header.payload_length = load_u32(bytes, 12);
  result.header.session_id = load_u64(bytes, 16);
  result.header.request_id = load_u64(bytes, 24);
  result.error = validate_frame_header(result.header);
  return result;
}

EncodeResult encode_frame(const FrameHeader& header,
                          const std::span<const std::byte> payload,
                          const std::span<std::byte> destination) noexcept {
  EncodeResult result;
  if (payload.size() > kMaximumFramePayloadBytes ||
      payload.size() > std::numeric_limits<std::uint32_t>::max()) {
    result.error = ProtocolError::payload_too_large;
    return result;
  }
  auto encoded_header = header;
  if (encoded_header.payload_length != payload.size()) {
    result.error = ProtocolError::noncanonical_value;
    return result;
  }
  result.error = validate_frame_header(encoded_header);
  if (result.error != ProtocolError::none) {
    return result;
  }
  const auto frame_size = kFrameHeaderBytes + payload.size();
  if (destination.size() < frame_size) {
    result.error = ProtocolError::output_too_small;
    return result;
  }
  const std::span<std::byte, kFrameHeaderBytes> header_output{
      destination.data(), kFrameHeaderBytes};
  result.error = encode_frame_header(encoded_header, header_output);
  if (result.error != ProtocolError::none) {
    return result;
  }
  std::copy(payload.begin(), payload.end(),
            destination.begin() +
                static_cast<std::ptrdiff_t>(kFrameHeaderBytes));
  result.bytes_written = frame_size;
  return result;
}

FrameView decode_frame(const std::span<const std::byte> bytes,
                       const std::uint64_t expected_session_id) noexcept {
  FrameView result;
  if (bytes.size() < kFrameHeaderBytes) {
    result.error = ProtocolError::truncated_header;
    return result;
  }
  const std::span<const std::byte, kFrameHeaderBytes> header_bytes{
      bytes.data(), kFrameHeaderBytes};
  const auto decoded_header = decode_frame_header(header_bytes);
  result.error = decoded_header.error;
  result.header = decoded_header.header;
  if (!decoded_header.valid()) {
    return result;
  }
  if (expected_session_id != 0 &&
      result.header.session_id != expected_session_id) {
    result.error = ProtocolError::wrong_session_id;
    return result;
  }
  const auto frame_size = kFrameHeaderBytes +
                          static_cast<std::size_t>(
                              result.header.payload_length);
  if (bytes.size() < frame_size) {
    result.error = ProtocolError::truncated_payload;
    return result;
  }
  if (bytes.size() != frame_size) {
    result.error = ProtocolError::trailing_bytes;
    return result;
  }
  result.payload = bytes.subspan(kFrameHeaderBytes,
                                 result.header.payload_length);
  return result;
}

PayloadWriter::PayloadWriter(
    const std::span<std::byte> destination) noexcept
    : destination_{destination},
      capacity_{std::min(destination.size(),
                         static_cast<std::size_t>(
                             kMaximumFramePayloadBytes))} {}

bool PayloadWriter::reserve(const std::size_t size) noexcept {
  if (error_ != ProtocolError::none) {
    return false;
  }
  if (size > capacity_ - position_) {
    error_ = ProtocolError::output_too_small;
    return false;
  }
  return true;
}

bool PayloadWriter::write_u8(const std::uint8_t value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  destination_[position_++] = static_cast<std::byte>(value);
  return true;
}

bool PayloadWriter::write_u16(const std::uint16_t value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  store_u16(destination_, position_, value);
  position_ += sizeof(value);
  return true;
}

bool PayloadWriter::write_u32(const std::uint32_t value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  store_u32(destination_, position_, value);
  position_ += sizeof(value);
  return true;
}

bool PayloadWriter::write_u64(const std::uint64_t value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  store_u64(destination_, position_, value);
  position_ += sizeof(value);
  return true;
}

bool PayloadWriter::write_bool(const bool value) noexcept {
  return write_u8(value ? 1U : 0U);
}

bool PayloadWriter::write_status(const ProtocolStatus value) noexcept {
  if (error_ != ProtocolError::none) {
    return false;
  }
  if (!known_protocol_status(value)) {
    error_ = ProtocolError::noncanonical_value;
    return false;
  }
  return write_u16(static_cast<std::uint16_t>(value));
}

bool PayloadWriter::write_phase(const ProtocolPhase value) noexcept {
  if (error_ != ProtocolError::none) {
    return false;
  }
  if (!known_protocol_phase(value)) {
    error_ = ProtocolError::noncanonical_value;
    return false;
  }
  return write_u16(static_cast<std::uint16_t>(value));
}

bool PayloadWriter::write_bytes(
    const std::span<const std::byte> value) noexcept {
  if (!reserve(value.size())) {
    return false;
  }
  std::copy(value.begin(), value.end(),
            destination_.begin() + static_cast<std::ptrdiff_t>(position_));
  position_ += value.size();
  return true;
}

std::span<const std::byte> PayloadWriter::written() const noexcept {
  return std::span<const std::byte>{destination_.data(), position_};
}

PayloadReader::PayloadReader(
    const std::span<const std::byte> payload) noexcept
    : payload_{payload} {
  if (payload.size() > kMaximumFramePayloadBytes) {
    error_ = ProtocolError::payload_too_large;
  }
}

bool PayloadReader::reserve(const std::size_t size) noexcept {
  if (error_ != ProtocolError::none) {
    return false;
  }
  if (size > payload_.size() - position_) {
    error_ = ProtocolError::payload_underflow;
    return false;
  }
  return true;
}

bool PayloadReader::read_u8(std::uint8_t& value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  value = std::to_integer<std::uint8_t>(payload_[position_++]);
  return true;
}

bool PayloadReader::read_u16(std::uint16_t& value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  value = load_u16(payload_, position_);
  position_ += sizeof(value);
  return true;
}

bool PayloadReader::read_u32(std::uint32_t& value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  value = load_u32(payload_, position_);
  position_ += sizeof(value);
  return true;
}

bool PayloadReader::read_u64(std::uint64_t& value) noexcept {
  if (!reserve(sizeof(value))) {
    return false;
  }
  value = load_u64(payload_, position_);
  position_ += sizeof(value);
  return true;
}

bool PayloadReader::read_bool(bool& value) noexcept {
  std::uint8_t encoded = 0;
  if (!read_u8(encoded)) {
    return false;
  }
  if (encoded > 1U) {
    error_ = ProtocolError::noncanonical_value;
    return false;
  }
  value = encoded != 0;
  return true;
}

bool PayloadReader::read_status(ProtocolStatus& value) noexcept {
  std::uint16_t encoded = 0;
  if (!read_u16(encoded)) {
    return false;
  }
  const auto status = static_cast<ProtocolStatus>(encoded);
  if (!known_protocol_status(status)) {
    error_ = ProtocolError::noncanonical_value;
    return false;
  }
  value = status;
  return true;
}

bool PayloadReader::read_phase(ProtocolPhase& value) noexcept {
  std::uint16_t encoded = 0;
  if (!read_u16(encoded)) {
    return false;
  }
  const auto phase = static_cast<ProtocolPhase>(encoded);
  if (!known_protocol_phase(phase)) {
    error_ = ProtocolError::noncanonical_value;
    return false;
  }
  value = phase;
  return true;
}

bool PayloadReader::read_bytes(
    const std::size_t size,
    std::span<const std::byte>& value) noexcept {
  if (!reserve(size)) {
    return false;
  }
  value = payload_.subspan(position_, size);
  position_ += size;
  return true;
}

bool PayloadReader::finish() noexcept {
  if (error_ != ProtocolError::none) {
    return false;
  }
  if (position_ != payload_.size()) {
    error_ = ProtocolError::payload_trailing_bytes;
    return false;
  }
  return true;
}

std::size_t PayloadReader::remaining() const noexcept {
  return payload_.size() - position_;
}

ProtocolStateValidator::ProtocolStateValidator(
    const std::uint64_t session_id,
    const ProtocolBudgets budgets) noexcept
    : session_id_{session_id}, budgets_{budgets} {
  if (session_id_ == 0 || !budgets_.valid()) {
    state_ = SessionState::failed;
    error_ = session_id_ == 0 ? ProtocolError::invalid_session_id
                              : ProtocolError::invalid_budget;
  }
}

ProtocolError ProtocolStateValidator::fail(
    const ProtocolError error) noexcept {
  error_ = error;
  state_ = SessionState::failed;
  return error_;
}

ProtocolError ProtocolStateValidator::charge(
    const FrameHeader& header) noexcept {
  if (message_count_ >= budgets_.maximum_messages) {
    return fail(ProtocolError::message_budget_exceeded);
  }
  ++message_count_;
  if (header.payload_length >
      budgets_.maximum_payload_bytes - payload_bytes_) {
    return fail(ProtocolError::byte_budget_exceeded);
  }
  payload_bytes_ += header.payload_length;
  return ProtocolError::none;
}

ProtocolError ProtocolStateValidator::begin_request(
    const FrameHeader& header, const SessionState state) noexcept {
  if (header.request_id <= last_request_id_) {
    return fail(ProtocolError::request_id_not_monotonic);
  }
  last_request_id_ = header.request_id;
  active_request_id_ = header.request_id;
  completed_request_id_ = 0;
  accept_late_cancel_ = false;
  read_in_flight_ = false;
  crossed_read_request_seen_ = false;
  active_result_type_ = state == SessionState::enumerating
                            ? MessageType::entry_batch
                            : MessageType::data_chunk;
  state_ = state;
  return ProtocolError::none;
}

void ProtocolStateValidator::complete_request(
    const bool accept_late_cancel) noexcept {
  completed_request_id_ = active_request_id_;
  active_request_id_ = 0;
  read_in_flight_ = false;
  crossed_read_request_seen_ = false;
  accept_late_cancel_ = accept_late_cancel;
  state_ = SessionState::idle;
}

ProtocolError ProtocolStateValidator::observe_active(
    const MessageDirection direction, const FrameHeader& header,
    const MessageType result_type) noexcept {
  if (header.type == MessageType::enumerate ||
      header.type == MessageType::stream_entry) {
    return fail(ProtocolError::request_already_active);
  }
  if (header.request_id == 0) {
    return fail(ProtocolError::unexpected_message);
  }
  if (header.request_id != active_request_id_) {
    return fail(ProtocolError::wrong_request_id);
  }
  if (header.type == MessageType::read_request &&
      direction == MessageDirection::worker_to_parent) {
    if (read_in_flight_) {
      return fail(ProtocolError::read_already_active);
    }
    read_in_flight_ = true;
    return ProtocolError::none;
  }
  if (header.type == MessageType::read_reply &&
      direction == MessageDirection::parent_to_worker) {
    if (!read_in_flight_) {
      return fail(ProtocolError::no_read_in_flight);
    }
    read_in_flight_ = false;
    return ProtocolError::none;
  }
  if (header.type == result_type &&
      direction == MessageDirection::worker_to_parent) {
    if (read_in_flight_) {
      return fail(ProtocolError::unexpected_message);
    }
    return ProtocolError::none;
  }
  if (header.type == MessageType::complete &&
      direction == MessageDirection::worker_to_parent) {
    if (read_in_flight_) {
      return fail(ProtocolError::unexpected_message);
    }
    // Completion wins a legitimate duplex race with cancellation. Until a
    // new top-level request starts, one same-request cancel may therefore be
    // observed after this completion and is consumed as stale without an ack.
    complete_request(true);
    return ProtocolError::none;
  }
  if (header.type == MessageType::cancel &&
      direction == MessageDirection::parent_to_worker) {
    crossed_read_request_seen_ = false;
    state_ = SessionState::cancelling;
    return ProtocolError::none;
  }
  return fail(ProtocolError::unexpected_message);
}

ProtocolError ProtocolStateValidator::observe(
    const MessageDirection direction, const FrameHeader& header) noexcept {
  if (state_ == SessionState::failed) {
    return error_;
  }
  if (state_ == SessionState::closed) {
    return fail(ProtocolError::terminal_state);
  }
  const auto header_error = validate_frame_header(header);
  if (header_error != ProtocolError::none) {
    return fail(header_error);
  }
  if (header.session_id != session_id_) {
    return fail(ProtocolError::wrong_session_id);
  }
  const auto budget_error = charge(header);
  if (budget_error != ProtocolError::none) {
    return budget_error;
  }

  switch (state_) {
    case SessionState::awaiting_hello:
      if (direction != MessageDirection::parent_to_worker ||
          header.type != MessageType::hello) {
        return fail(ProtocolError::unexpected_message);
      }
      state_ = SessionState::awaiting_ready;
      return ProtocolError::none;
    case SessionState::awaiting_ready:
      if (direction != MessageDirection::worker_to_parent ||
          header.type != MessageType::ready) {
        return fail(ProtocolError::unexpected_message);
      }
      state_ = SessionState::idle;
      return ProtocolError::none;
    case SessionState::idle:
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::enumerate) {
        return begin_request(header, SessionState::enumerating);
      }
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::stream_entry) {
        return begin_request(header, SessionState::streaming);
      }
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::shutdown) {
        state_ = SessionState::closed;
        return ProtocolError::none;
      }
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::cancel && accept_late_cancel_) {
        if (header.request_id != completed_request_id_) {
          return fail(ProtocolError::wrong_request_id);
        }
        accept_late_cancel_ = false;
        return ProtocolError::none;
      }
      return fail(ProtocolError::unexpected_message);
    case SessionState::enumerating:
      return observe_active(direction, header, MessageType::entry_batch);
    case SessionState::streaming:
      return observe_active(direction, header, MessageType::data_chunk);
    case SessionState::cancelling:
      if (header.request_id != active_request_id_) {
        return fail(ProtocolError::wrong_request_id);
      }
      if (direction == MessageDirection::worker_to_parent &&
          header.type == MessageType::cancel_ack) {
        active_request_id_ = 0;
        read_in_flight_ = false;
        crossed_read_request_seen_ = false;
        state_ = SessionState::cancelled;
        return ProtocolError::none;
      }
      if (direction == MessageDirection::worker_to_parent &&
          header.type == MessageType::complete) {
        if (read_in_flight_ || crossed_read_request_seen_) {
          return fail(ProtocolError::unexpected_message);
        }
        // The worker committed completion before observing the cancel. The
        // complete frame is the terminal response; no cancel_ack follows.
        complete_request(false);
        return ProtocolError::none;
      }
      if (direction == MessageDirection::worker_to_parent &&
          header.type == MessageType::read_request) {
        if (read_in_flight_ || crossed_read_request_seen_) {
          return fail(ProtocolError::read_already_active);
        }
        crossed_read_request_seen_ = true;
        return ProtocolError::none;
      }
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::read_reply) {
        // Only a reply already crossing for the read that preceded
        // cancellation can resolve that read and permit normal completion.
        if (!read_in_flight_) {
          return fail(ProtocolError::no_read_in_flight);
        }
        read_in_flight_ = false;
        return ProtocolError::none;
      }
      if (direction == MessageDirection::worker_to_parent &&
          header.type == active_result_type_) {
        if (read_in_flight_ || crossed_read_request_seen_) {
          return fail(ProtocolError::unexpected_message);
        }
        // Bounded result frames already in flight may precede the completion
        // that wins a race with cancellation.
        return ProtocolError::none;
      }
      return fail(ProtocolError::unexpected_message);
    case SessionState::cancelled:
      if (direction == MessageDirection::parent_to_worker &&
          header.type == MessageType::shutdown) {
        state_ = SessionState::closed;
        return ProtocolError::none;
      }
      return fail(ProtocolError::terminal_state);
    case SessionState::closed:
    case SessionState::failed:
      return fail(ProtocolError::terminal_state);
  }
  return fail(ProtocolError::unexpected_message);
}

}  // namespace ohl::parser
