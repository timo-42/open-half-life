#pragma once

#include "ohl/parser/protocol.hpp"

#include <cstddef>
#include <cstdint>
#include <span>
#include <string_view>

namespace ohl::parser {

inline constexpr std::size_t kHelloPayloadBytes = 12;
inline constexpr std::size_t kReadyPayloadBytes = 0;
inline constexpr std::size_t kEnumeratePayloadBytes = 0;
inline constexpr std::size_t kStreamEntryPayloadBytes = 8;
inline constexpr std::size_t kReadRequestPayloadBytes = 16;
inline constexpr std::size_t kReadReplyPrefixBytes = 6;
inline constexpr std::size_t kEntryBatchPrefixBytes = 2;
inline constexpr std::size_t kEntryBatchEntryPrefixBytes = 18;
inline constexpr std::size_t kCompletePayloadBytes = 4;
inline constexpr std::size_t kCancelPayloadBytes = 0;
inline constexpr std::size_t kCancelAckPayloadBytes = 0;
inline constexpr std::size_t kShutdownPayloadBytes = 0;
inline constexpr std::uint32_t kMaximumReadBytes =
    kMaximumFramePayloadBytes - kReadReplyPrefixBytes;
inline constexpr std::size_t kMaximumDataChunkBytes = 256U * 1'024U;
inline constexpr std::uint16_t kMaximumEntryBatchEntries = 256;
inline constexpr std::uint32_t kMaximumEnumeratedEntries = 50'000;
inline constexpr std::uint64_t kMaximumEntryBatchPathBytes = 4'096;
inline constexpr std::uint64_t kMaximumEnumeratedPathBytes =
    64ULL * 1'024ULL * 1'024ULL;
inline constexpr std::uint64_t kMaximumEnumeratedEntryBytes =
    8ULL * 1'024ULL * 1'024ULL * 1'024ULL;
inline constexpr std::uint64_t kMaximumEnumeratedTotalBytes =
    32ULL * 1'024ULL * 1'024ULL * 1'024ULL;

struct SourceReadPolicy {
  std::uint64_t source_size{0};
  std::uint32_t maximum_read_bytes{0};

  [[nodiscard]] bool valid() const noexcept {
    return source_size != 0 && maximum_read_bytes != 0 &&
           maximum_read_bytes <= kMaximumReadBytes;
  }
};

struct EntryBatchPolicy {
  std::uint32_t remaining_entries{0};
  std::uint64_t remaining_path_bytes{0};
  std::uint64_t maximum_entry_bytes{0};
  std::uint64_t remaining_total_bytes{0};
  bool has_previous_source_token{false};
  std::uint64_t previous_source_token{0};

  [[nodiscard]] bool valid() const noexcept {
    return remaining_entries != 0 &&
           remaining_entries <= kMaximumEnumeratedEntries &&
           remaining_path_bytes != 0 &&
           remaining_path_bytes <= kMaximumEnumeratedPathBytes &&
           maximum_entry_bytes != 0 &&
           maximum_entry_bytes <= kMaximumEnumeratedEntryBytes &&
           remaining_total_bytes <= kMaximumEnumeratedTotalBytes &&
           (has_previous_source_token || previous_source_token == 0);
  }
};

struct HelloMessage {
  std::uint64_t source_size{0};
  std::uint32_t maximum_read_bytes{0};
};

struct ReadyMessage {};

struct EnumerateMessage {};

struct StreamEntryMessage {
  std::uint64_t source_token{0};
};

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

struct EntryBatchEntry {
  std::uint64_t source_token{0};
  std::uint64_t size_bytes{0};
  // Printable-ASCII archive spelling. This is not a validated destination
  // path and conveys no filesystem authority.
  std::string_view archive_path;
};

struct EntryBatchMessage {
  // For decoded messages this aliases caller-provided entry storage, while
  // every archive_path aliases the frame payload. Both must remain alive and
  // unchanged while entries are used.
  std::span<const EntryBatchEntry> entries;
};

struct DataChunkMessage {
  // Non-owning view into the payload supplied to decode_data_chunk_payload.
  // The payload storage must remain alive and unchanged while data is used.
  std::span<const std::byte> data;
};

struct CompleteMessage {
  ProtocolStatus status{ProtocolStatus::internal_failure};
  ProtocolPhase phase{ProtocolPhase::handshake};
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
using StreamEntryDecodeResult = MessageDecodeResult<StreamEntryMessage>;
using ReadRequestDecodeResult = MessageDecodeResult<ReadRequestMessage>;
using ReadReplyDecodeResult = MessageDecodeResult<ReadReplyMessage>;
using EntryBatchDecodeResult = MessageDecodeResult<EntryBatchMessage>;
using DataChunkDecodeResult = MessageDecodeResult<DataChunkMessage>;
using CompleteDecodeResult = MessageDecodeResult<CompleteMessage>;
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

[[nodiscard]] EncodeResult encode_stream_entry_payload(
    const StreamEntryMessage& message,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] StreamEntryDecodeResult decode_stream_entry_payload(
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

[[nodiscard]] EncodeResult encode_entry_batch_payload(
    const EntryBatchMessage& message, const EntryBatchPolicy& policy,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] EntryBatchDecodeResult decode_entry_batch_payload(
    const FrameView& frame, const EntryBatchPolicy& policy,
    std::span<EntryBatchEntry> entry_storage) noexcept;

[[nodiscard]] EncodeResult encode_data_chunk_payload(
    const DataChunkMessage& message, std::uint64_t remaining_entry_bytes,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] DataChunkDecodeResult decode_data_chunk_payload(
    const FrameView& frame,
    std::uint64_t remaining_entry_bytes) noexcept;

[[nodiscard]] EncodeResult encode_complete_payload(
    const CompleteMessage& message,
    ProtocolPhase expected_operation_phase,
    std::span<std::byte> destination) noexcept;
[[nodiscard]] CompleteDecodeResult decode_complete_payload(
    const FrameView& frame,
    ProtocolPhase expected_operation_phase) noexcept;

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
