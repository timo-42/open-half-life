#pragma once

#include <cstddef>
#include <cstdint>
#include <span>
#include <string_view>

namespace ohl::parser {

inline constexpr std::size_t kFrameHeaderBytes = 32;
inline constexpr std::uint16_t kProtocolMajorVersion = 1;
inline constexpr std::uint16_t kProtocolMinorVersion = 0;
inline constexpr std::uint32_t kMaximumFramePayloadBytes = 1U << 20U;
inline constexpr std::uint64_t kMaximumProtocolMessages = 1U << 20U;
inline constexpr std::uint64_t kMaximumCumulativePayloadBytes =
    64ULL * 1'024ULL * 1'024ULL * 1'024ULL;

enum class MessageType : std::uint16_t {
  hello = 0x0001,
  ready = 0x0002,
  enumerate = 0x0010,
  stream_entry = 0x0011,
  read_request = 0x0020,
  read_reply = 0x0021,
  entry_batch = 0x0030,
  data_chunk = 0x0031,
  complete = 0x0032,
  cancel = 0x0040,
  cancel_ack = 0x0041,
  shutdown = 0x0042,
};

enum class ProtocolStatus : std::uint16_t {
  ok = 0,
  unsupported = 1,
  invalid_request = 2,
  parser_rejected = 3,
  budget_exceeded = 4,
  cancelled = 5,
  source_changed = 6,
  source_read_failed = 7,
  result_validation_failed = 8,
  internal_failure = 9,
};

enum class ProtocolPhase : std::uint16_t {
  handshake = 0,
  enumerate = 1,
  stream = 2,
  source_read = 3,
  complete = 4,
};

enum class ProtocolError : std::uint16_t {
  none = 0,
  truncated_header,
  invalid_magic,
  unsupported_version,
  unknown_message_type,
  reserved_flags,
  payload_too_large,
  truncated_payload,
  trailing_bytes,
  invalid_session_id,
  invalid_request_id,
  wrong_session_id,
  wrong_request_id,
  request_id_not_monotonic,
  unexpected_message,
  request_already_active,
  read_already_active,
  no_read_in_flight,
  message_budget_exceeded,
  byte_budget_exceeded,
  invalid_budget,
  output_too_small,
  payload_underflow,
  payload_trailing_bytes,
  noncanonical_value,
  terminal_state,
};

[[nodiscard]] constexpr bool known_message_type(
    const MessageType type) noexcept {
  switch (type) {
    case MessageType::hello:
    case MessageType::ready:
    case MessageType::enumerate:
    case MessageType::stream_entry:
    case MessageType::read_request:
    case MessageType::read_reply:
    case MessageType::entry_batch:
    case MessageType::data_chunk:
    case MessageType::complete:
    case MessageType::cancel:
    case MessageType::cancel_ack:
    case MessageType::shutdown:
      return true;
  }
  return false;
}

[[nodiscard]] constexpr bool known_protocol_status(
    const ProtocolStatus status) noexcept {
  switch (status) {
    case ProtocolStatus::ok:
    case ProtocolStatus::unsupported:
    case ProtocolStatus::invalid_request:
    case ProtocolStatus::parser_rejected:
    case ProtocolStatus::budget_exceeded:
    case ProtocolStatus::cancelled:
    case ProtocolStatus::source_changed:
    case ProtocolStatus::source_read_failed:
    case ProtocolStatus::result_validation_failed:
    case ProtocolStatus::internal_failure:
      return true;
  }
  return false;
}

[[nodiscard]] constexpr bool known_protocol_phase(
    const ProtocolPhase phase) noexcept {
  switch (phase) {
    case ProtocolPhase::handshake:
    case ProtocolPhase::enumerate:
    case ProtocolPhase::stream:
    case ProtocolPhase::source_read:
    case ProtocolPhase::complete:
      return true;
  }
  return false;
}

[[nodiscard]] std::string_view to_string(ProtocolError error) noexcept;

struct FrameHeader {
  std::uint16_t major_version{kProtocolMajorVersion};
  std::uint16_t minor_version{kProtocolMinorVersion};
  MessageType type{MessageType::hello};
  std::uint16_t flags{0};
  std::uint32_t payload_length{0};
  std::uint64_t session_id{0};
  std::uint64_t request_id{0};
};

struct FrameView {
  ProtocolError error{ProtocolError::none};
  FrameHeader header;
  std::span<const std::byte> payload;

  [[nodiscard]] bool valid() const noexcept {
    return error == ProtocolError::none;
  }
};

struct HeaderDecodeResult {
  ProtocolError error{ProtocolError::none};
  FrameHeader header;

  [[nodiscard]] bool valid() const noexcept {
    return error == ProtocolError::none;
  }
};

struct EncodeResult {
  ProtocolError error{ProtocolError::none};
  std::size_t bytes_written{0};

  [[nodiscard]] bool valid() const noexcept {
    return error == ProtocolError::none;
  }
};

[[nodiscard]] ProtocolError validate_frame_header(
    const FrameHeader& header) noexcept;
[[nodiscard]] ProtocolError encode_frame_header(
    const FrameHeader& header,
    std::span<std::byte, kFrameHeaderBytes> destination) noexcept;
// Validates the complete fixed header, including the payload ceiling, before a
// transport allocates or reads payload storage.
[[nodiscard]] HeaderDecodeResult decode_frame_header(
    std::span<const std::byte, kFrameHeaderBytes> bytes) noexcept;
[[nodiscard]] EncodeResult encode_frame(
    const FrameHeader& header, std::span<const std::byte> payload,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] FrameView decode_frame(
    std::span<const std::byte> bytes,
    std::uint64_t expected_session_id = 0) noexcept;

class PayloadWriter final {
 public:
  explicit PayloadWriter(std::span<std::byte> destination) noexcept;

  [[nodiscard]] bool write_u8(std::uint8_t value) noexcept;
  [[nodiscard]] bool write_u16(std::uint16_t value) noexcept;
  [[nodiscard]] bool write_u32(std::uint32_t value) noexcept;
  [[nodiscard]] bool write_u64(std::uint64_t value) noexcept;
  [[nodiscard]] bool write_bool(bool value) noexcept;
  [[nodiscard]] bool write_status(ProtocolStatus value) noexcept;
  [[nodiscard]] bool write_phase(ProtocolPhase value) noexcept;
  [[nodiscard]] bool write_bytes(std::span<const std::byte> value) noexcept;

  [[nodiscard]] ProtocolError error() const noexcept { return error_; }
  [[nodiscard]] std::size_t size() const noexcept { return position_; }
  [[nodiscard]] std::span<const std::byte> written() const noexcept;

 private:
  [[nodiscard]] bool reserve(std::size_t size) noexcept;

  std::span<std::byte> destination_;
  std::size_t capacity_{0};
  std::size_t position_{0};
  ProtocolError error_{ProtocolError::none};
};

class PayloadReader final {
 public:
  explicit PayloadReader(std::span<const std::byte> payload) noexcept;

  [[nodiscard]] bool read_u8(std::uint8_t& value) noexcept;
  [[nodiscard]] bool read_u16(std::uint16_t& value) noexcept;
  [[nodiscard]] bool read_u32(std::uint32_t& value) noexcept;
  [[nodiscard]] bool read_u64(std::uint64_t& value) noexcept;
  [[nodiscard]] bool read_bool(bool& value) noexcept;
  [[nodiscard]] bool read_status(ProtocolStatus& value) noexcept;
  [[nodiscard]] bool read_phase(ProtocolPhase& value) noexcept;
  [[nodiscard]] bool read_bytes(
      std::size_t size, std::span<const std::byte>& value) noexcept;
  [[nodiscard]] bool finish() noexcept;

  [[nodiscard]] ProtocolError error() const noexcept { return error_; }
  [[nodiscard]] std::size_t remaining() const noexcept;

 private:
  [[nodiscard]] bool reserve(std::size_t size) noexcept;

  std::span<const std::byte> payload_;
  std::size_t position_{0};
  ProtocolError error_{ProtocolError::none};
};

enum class MessageDirection : std::uint8_t {
  parent_to_worker,
  worker_to_parent,
};

enum class SessionState : std::uint8_t {
  awaiting_hello,
  awaiting_ready,
  idle,
  enumerating,
  streaming,
  cancelling,
  cancelled,
  closed,
  failed,
};

struct ProtocolBudgets {
  std::uint64_t maximum_messages{kMaximumProtocolMessages};
  std::uint64_t maximum_payload_bytes{kMaximumCumulativePayloadBytes};

  [[nodiscard]] bool valid() const noexcept {
    return maximum_messages != 0 &&
           maximum_messages <= kMaximumProtocolMessages &&
           maximum_payload_bytes != 0 &&
           maximum_payload_bytes <= kMaximumCumulativePayloadBytes;
  }
};

class ProtocolStateValidator final {
 public:
  explicit ProtocolStateValidator(
      std::uint64_t session_id, ProtocolBudgets budgets = {}) noexcept;

  [[nodiscard]] ProtocolError observe(
      MessageDirection direction, const FrameHeader& header) noexcept;

  [[nodiscard]] SessionState state() const noexcept { return state_; }
  [[nodiscard]] ProtocolError error() const noexcept { return error_; }
  [[nodiscard]] std::uint64_t active_request_id() const noexcept {
    return active_request_id_;
  }
  [[nodiscard]] std::uint64_t message_count() const noexcept {
    return message_count_;
  }
  [[nodiscard]] std::uint64_t payload_bytes() const noexcept {
    return payload_bytes_;
  }

 private:
  [[nodiscard]] ProtocolError fail(ProtocolError error) noexcept;
  [[nodiscard]] ProtocolError charge(const FrameHeader& header) noexcept;
  [[nodiscard]] ProtocolError begin_request(const FrameHeader& header,
                                            SessionState state) noexcept;
  [[nodiscard]] ProtocolError observe_active(
      MessageDirection direction, const FrameHeader& header,
      MessageType result_type) noexcept;

  std::uint64_t session_id_{0};
  ProtocolBudgets budgets_;
  SessionState state_{SessionState::awaiting_hello};
  ProtocolError error_{ProtocolError::none};
  std::uint64_t active_request_id_{0};
  std::uint64_t last_request_id_{0};
  std::uint64_t message_count_{0};
  std::uint64_t payload_bytes_{0};
  bool read_in_flight_{false};
};

}  // namespace ohl::parser
