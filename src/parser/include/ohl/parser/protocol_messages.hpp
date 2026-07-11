#pragma once

#include "ohl/parser/protocol.hpp"

#include <cstddef>
#include <cstdint>
#include <span>

namespace ohl::parser {

inline constexpr std::size_t kHelloPayloadBytes = 12;
inline constexpr std::size_t kReadyPayloadBytes = 0;
inline constexpr std::size_t kEnumeratePayloadBytes = 0;
inline constexpr std::size_t kReadRequestPayloadBytes = 16;
inline constexpr std::size_t kReadReplyPrefixBytes = 6;
inline constexpr std::size_t kCancelPayloadBytes = 0;
inline constexpr std::size_t kCancelAckPayloadBytes = 0;
inline constexpr std::size_t kShutdownPayloadBytes = 0;
inline constexpr std::uint32_t kMaximumReadBytes =
    kMaximumFramePayloadBytes - kReadReplyPrefixBytes;

struct SourceReadPolicy {
  std::uint64_t source_size{0};
  std::uint32_t maximum_read_bytes{0};

  [[nodiscard]] bool valid() const noexcept {
    return source_size != 0 && maximum_read_bytes != 0 &&
           maximum_read_bytes <= kMaximumReadBytes;
  }
};

struct HelloMessage {
  std::uint64_t source_size{0};
  std::uint32_t maximum_read_bytes{0};
};

struct ReadyMessage {};

struct EnumerateMessage {};

struct ReadRequestMessage {
  std::uint32_t read_sequence{0};
  std::uint64_t offset{0};
  std::uint32_t length{0};
};

struct ReadReplyMessage {
  std::uint32_t read_sequence{0};
  ProtocolStatus status{ProtocolStatus::ok};
  // Non-owning view into the payload supplied to decode_read_reply_payload.
  // The payload storage must remain alive and unchanged while data is used.
  std::span<const std::byte> data;
};

struct CancelMessage {};

struct CancelAckMessage {};

struct ShutdownMessage {};

template <typename Message>
struct MessageDecodeResult {
  ProtocolError error{ProtocolError::none};
  Message message{};

  [[nodiscard]] bool valid() const noexcept {
    return error == ProtocolError::none;
  }
};

using HelloDecodeResult = MessageDecodeResult<HelloMessage>;
using ReadyDecodeResult = MessageDecodeResult<ReadyMessage>;
using EnumerateDecodeResult = MessageDecodeResult<EnumerateMessage>;
using ReadRequestDecodeResult = MessageDecodeResult<ReadRequestMessage>;
using ReadReplyDecodeResult = MessageDecodeResult<ReadReplyMessage>;
using CancelDecodeResult = MessageDecodeResult<CancelMessage>;
using CancelAckDecodeResult = MessageDecodeResult<CancelAckMessage>;
using ShutdownDecodeResult = MessageDecodeResult<ShutdownMessage>;

[[nodiscard]] EncodeResult encode_hello_payload(
    const HelloMessage& message, std::span<std::byte> destination) noexcept;
[[nodiscard]] HelloDecodeResult decode_hello_payload(
    const FrameView& frame) noexcept;

[[nodiscard]] EncodeResult encode_ready_payload(
    const ReadyMessage& message, std::span<std::byte> destination) noexcept;
[[nodiscard]] ReadyDecodeResult decode_ready_payload(
    const FrameView& frame) noexcept;

[[nodiscard]] EncodeResult encode_enumerate_payload(
    const EnumerateMessage& message,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] EnumerateDecodeResult decode_enumerate_payload(
    const FrameView& frame) noexcept;

[[nodiscard]] EncodeResult encode_read_request_payload(
    const ReadRequestMessage& message, const SourceReadPolicy& policy,
    std::uint32_t expected_sequence,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] ReadRequestDecodeResult decode_read_request_payload(
    const FrameView& frame, const SourceReadPolicy& policy,
    std::uint32_t expected_sequence) noexcept;

[[nodiscard]] EncodeResult encode_read_reply_payload(
    const ReadReplyMessage& message, std::uint32_t expected_sequence,
    std::uint32_t requested_length,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] ReadReplyDecodeResult decode_read_reply_payload(
    const FrameView& frame, std::uint32_t expected_sequence,
    std::uint32_t requested_length) noexcept;

[[nodiscard]] EncodeResult encode_cancel_payload(
    const CancelMessage& message, std::span<std::byte> destination) noexcept;
[[nodiscard]] CancelDecodeResult decode_cancel_payload(
    const FrameView& frame) noexcept;

[[nodiscard]] EncodeResult encode_cancel_ack_payload(
    const CancelAckMessage& message,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] CancelAckDecodeResult decode_cancel_ack_payload(
    const FrameView& frame) noexcept;

[[nodiscard]] EncodeResult encode_shutdown_payload(
    const ShutdownMessage& message,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] ShutdownDecodeResult decode_shutdown_payload(
    const FrameView& frame) noexcept;

}  // namespace ohl::parser
