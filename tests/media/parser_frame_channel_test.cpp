#include "ohl/media/parser_frame_channel.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <limits>
#include <mutex>
#include <span>
#include <string_view>
#include <thread>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;
using ohl::media::ParserFrameChannel;
using ohl::media::ParserFrameChannelError;
using ohl::media::ParserFrameChannelOperations;
using ohl::media::ParserFrameChannelResult;
using ohl::parser::FrameHeader;
using ohl::parser::MessageType;
using ohl::parser::ProtocolError;
using ohl::platform::IsolatedWorkerCancellationSource;
using ohl::platform::IsolatedWorkerCancellationToken;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerIoResult;

constexpr std::uint64_t kSession = 0x1020'3040'5060'7080ULL;
constexpr std::size_t kExactTransfer =
    std::numeric_limits<std::size_t>::max();
constexpr std::size_t kNoCancellationRequest =
    std::numeric_limits<std::size_t>::max();

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

[[nodiscard]] FrameHeader header(const MessageType type,
                                 const std::uint64_t request_id,
                                 const std::size_t payload_size) {
  return {
      .type = type,
      .payload_length = static_cast<std::uint32_t>(payload_size),
      .session_id = kSession,
      .request_id = request_id,
  };
}

[[nodiscard]] std::array<std::byte, ohl::parser::kFrameHeaderBytes>
encode_header(const FrameHeader& source) {
  std::array<std::byte, ohl::parser::kFrameHeaderBytes> encoded{};
  if (ohl::parser::encode_frame_header(source, encoded) !=
      ProtocolError::none) {
    std::abort();
  }
  return encoded;
}

void store_u16(std::array<std::byte, ohl::parser::kFrameHeaderBytes>& output,
               const std::size_t offset, const std::uint16_t value) {
  output[offset] = static_cast<std::byte>(value & 0xffU);
  output[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

void store_u32(std::array<std::byte, ohl::parser::kFrameHeaderBytes>& output,
               const std::size_t offset, const std::uint32_t value) {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

void store_u64(std::array<std::byte, ohl::parser::kFrameHeaderBytes>& output,
               const std::size_t offset, const std::uint64_t value) {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

// This deliberately does not call the production header encoder under test.
[[nodiscard]] std::array<std::byte, ohl::parser::kFrameHeaderBytes>
canonical_header_bytes(const FrameHeader& source) {
  std::array<std::byte, ohl::parser::kFrameHeaderBytes> expected{};
  expected[0] = std::byte{'O'};
  expected[1] = std::byte{'H'};
  expected[2] = std::byte{'L'};
  expected[3] = std::byte{'P'};
  store_u16(expected, 4, source.major_version);
  store_u16(expected, 6, source.minor_version);
  store_u16(expected, 8, static_cast<std::uint16_t>(source.type));
  store_u16(expected, 10, source.flags);
  store_u32(expected, 12, source.payload_length);
  store_u64(expected, 16, source.session_id);
  store_u64(expected, 24, source.request_id);
  return expected;
}

[[nodiscard]] std::uint64_t checksum(
    const std::span<const std::byte> bytes) noexcept {
  std::uint64_t value = 0;
  for (const auto byte : bytes) {
    value = (value * 131U) + std::to_integer<std::uint8_t>(byte);
  }
  return value;
}

struct IoPlan {
  std::span<const std::byte> input;
  std::size_t bytes_transferred{kExactTransfer};
  IsolatedWorkerError error{IsolatedWorkerError::none};
  bool block{false};
};

struct IoObservation {
  std::size_t size{0};
  Clock::time_point deadline{};
  bool cancellation_requested{false};
  std::uint64_t data_checksum{0};
  std::array<std::byte, ohl::parser::kFrameHeaderBytes> prefix{};
  std::size_t prefix_size{0};
};

class ScriptedIo final {
 public:
  void add_read(const std::span<const std::byte> input,
                const std::size_t bytes_transferred = kExactTransfer,
                const IsolatedWorkerError error = IsolatedWorkerError::none,
                const bool block = false) {
    read_plans_[read_plan_count_++] = {
        .input = input,
        .bytes_transferred = bytes_transferred,
        .error = error,
        .block = block,
    };
  }

  void add_write(const std::size_t bytes_transferred = kExactTransfer,
                 const IsolatedWorkerError error = IsolatedWorkerError::none,
                 const bool block = false) {
    write_plans_[write_plan_count_++] = {
        .input = {},
        .bytes_transferred = bytes_transferred,
        .error = error,
        .block = block,
    };
  }

  [[nodiscard]] ParserFrameChannelOperations operations() noexcept {
    return {
        .read_exact = read_exact,
        .write_all = write_all,
        .abort_io = abort_io,
        .context = this,
    };
  }

  void request_cancellation_after_read(
      const std::size_t call,
      IsolatedWorkerCancellationSource& source) noexcept {
    cancel_after_read_ = call;
    cancellation_source_ = &source;
  }

  void request_cancellation_after_write(
      const std::size_t call,
      IsolatedWorkerCancellationSource& source) noexcept {
    cancel_after_write_ = call;
    cancellation_source_ = &source;
  }

  [[nodiscard]] bool wait_until_entered(const std::size_t reads,
                                        const std::size_t writes) {
    std::unique_lock lock{mutex_};
    return condition_.wait_for(lock, std::chrono::seconds{5}, [&] {
      return entered_reads_ >= reads && entered_writes_ >= writes;
    });
  }

  void release_reads() {
    {
      const std::scoped_lock lock{mutex_};
      release_reads_ = true;
    }
    condition_.notify_all();
  }

  void release_writes() {
    {
      const std::scoped_lock lock{mutex_};
      release_writes_ = true;
    }
    condition_.notify_all();
  }

  void release_all() {
    {
      const std::scoped_lock lock{mutex_};
      release_reads_ = true;
      release_writes_ = true;
    }
    condition_.notify_all();
  }

  [[nodiscard]] std::size_t read_calls() const noexcept {
    const std::scoped_lock lock{mutex_};
    return read_calls_;
  }
  [[nodiscard]] std::size_t write_calls() const noexcept {
    const std::scoped_lock lock{mutex_};
    return write_calls_;
  }
  [[nodiscard]] std::size_t abort_calls() const noexcept {
    const std::scoped_lock lock{mutex_};
    return abort_calls_;
  }
  [[nodiscard]] bool empty_io_seen() const noexcept {
    const std::scoped_lock lock{mutex_};
    return empty_io_seen_;
  }
  [[nodiscard]] IoObservation read_observation(
      const std::size_t index) const noexcept {
    const std::scoped_lock lock{mutex_};
    return read_observations_[index];
  }
  [[nodiscard]] IoObservation write_observation(
      const std::size_t index) const noexcept {
    const std::scoped_lock lock{mutex_};
    return write_observations_[index];
  }

 private:
  [[nodiscard]] static IsolatedWorkerIoResult read_exact(
      void* const opaque, const std::span<std::byte> destination,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept {
    auto& context = *static_cast<ScriptedIo*>(opaque);
    std::unique_lock lock{context.mutex_};
    const auto call = context.read_calls_++;
    if (destination.empty()) {
      context.empty_io_seen_ = true;
    }
    context.observe(context.read_observations_[call], destination, deadline,
                    cancellation);
    ++context.entered_reads_;
    context.condition_.notify_all();
    const auto plan = call < context.read_plan_count_
                          ? context.read_plans_[call]
                          : IoPlan{.input = {},
                                   .bytes_transferred = kExactTransfer,
                                   .error = IsolatedWorkerError::io_failure,
                                   .block = false};
    if (context.cancel_after_read_ == call &&
        context.cancellation_source_ != nullptr) {
      (void)context.cancellation_source_->request_cancellation();
    }
    if (plan.block) {
      context.condition_.wait(lock, [&] {
        return context.release_reads_ || context.aborted_;
      });
    }
    if (context.aborted_) {
      return {.bytes_transferred = 0,
              .error = IsolatedWorkerError::cancelled};
    }
    const auto transferred = plan.bytes_transferred == kExactTransfer
                                 ? destination.size()
                                 : plan.bytes_transferred;
    const auto copied = std::min({destination.size(), plan.input.size(),
                                  transferred});
    std::copy_n(plan.input.begin(), copied, destination.begin());
    return {.bytes_transferred = transferred, .error = plan.error};
  }

  [[nodiscard]] static IsolatedWorkerIoResult write_all(
      void* const opaque, const std::span<const std::byte> source,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept {
    auto& context = *static_cast<ScriptedIo*>(opaque);
    std::unique_lock lock{context.mutex_};
    const auto call = context.write_calls_++;
    if (source.empty()) {
      context.empty_io_seen_ = true;
    }
    context.observe(context.write_observations_[call], source, deadline,
                    cancellation);
    ++context.entered_writes_;
    context.condition_.notify_all();
    const auto plan = call < context.write_plan_count_
                          ? context.write_plans_[call]
                          : IoPlan{.input = {},
                                   .bytes_transferred = kExactTransfer,
                                   .error = IsolatedWorkerError::io_failure,
                                   .block = false};
    if (context.cancel_after_write_ == call &&
        context.cancellation_source_ != nullptr) {
      (void)context.cancellation_source_->request_cancellation();
    }
    if (plan.block) {
      context.condition_.wait(lock, [&] {
        return context.release_writes_ || context.aborted_;
      });
    }
    if (context.aborted_) {
      return {.bytes_transferred = 0,
              .error = IsolatedWorkerError::cancelled};
    }
    const auto transferred = plan.bytes_transferred == kExactTransfer
                                 ? source.size()
                                 : plan.bytes_transferred;
    return {.bytes_transferred = transferred, .error = plan.error};
  }

  static void abort_io(void* const opaque) noexcept {
    auto& context = *static_cast<ScriptedIo*>(opaque);
    {
      const std::scoped_lock lock{context.mutex_};
      ++context.abort_calls_;
      context.aborted_ = true;
    }
    context.condition_.notify_all();
  }

  template <typename Byte>
  void observe(IoObservation& observation, const std::span<Byte> bytes,
               const Clock::time_point deadline,
               const IsolatedWorkerCancellationToken& cancellation) noexcept {
    observation.size = bytes.size();
    observation.deadline = deadline;
    observation.cancellation_requested =
        cancellation.cancellation_requested();
    observation.data_checksum = checksum(
        std::as_bytes(std::span{bytes.data(), bytes.size()}));
    observation.prefix_size =
        std::min(bytes.size(), observation.prefix.size());
    for (std::size_t index = 0; index < observation.prefix_size; ++index) {
      observation.prefix[index] = bytes[index];
    }
  }

  mutable std::mutex mutex_;
  std::condition_variable condition_;
  std::array<IoPlan, 16> read_plans_{};
  std::array<IoPlan, 16> write_plans_{};
  std::array<IoObservation, 16> read_observations_{};
  std::array<IoObservation, 16> write_observations_{};
  std::size_t read_plan_count_{0};
  std::size_t write_plan_count_{0};
  std::size_t read_calls_{0};
  std::size_t write_calls_{0};
  std::size_t entered_reads_{0};
  std::size_t entered_writes_{0};
  std::size_t abort_calls_{0};
  bool release_reads_{false};
  bool release_writes_{false};
  bool aborted_{false};
  bool empty_io_seen_{false};
  std::size_t cancel_after_read_{kNoCancellationRequest};
  std::size_t cancel_after_write_{kNoCancellationRequest};
  IsolatedWorkerCancellationSource* cancellation_source_{nullptr};
};

[[nodiscard]] bool same_result(const ParserFrameChannelResult& actual,
                               const ParserFrameChannelError error,
                               const ProtocolError protocol_error,
                               const IsolatedWorkerError worker_error) {
  return actual.error == error && actual.protocol_error == protocol_error &&
         actual.worker_error == worker_error;
}

[[nodiscard]] bool test_canonical_send_receive() {
  const auto deadline = Clock::now() + std::chrono::hours{1};
  const std::array payload{std::byte{0x10}, std::byte{0x20},
                           std::byte{0x30}};

  ScriptedIo outgoing;
  outgoing.add_write();
  outgoing.add_write();
  outgoing.add_write();
  IsolatedWorkerCancellationSource outgoing_cancellation;
  outgoing.request_cancellation_after_write(1, outgoing_cancellation);
  ParserFrameChannel sender{kSession, outgoing.operations()};
  if (!sender.send(header(MessageType::shutdown, 0, 0), {}, deadline).valid() ||
      !sender
           .send(header(MessageType::data_chunk, 1, payload.size()), payload,
                 deadline, outgoing_cancellation.token())
           .valid() ||
      outgoing.write_calls() != 3 || outgoing.abort_calls() != 0 ||
      outgoing.empty_io_seen()) {
    return fail("canonical empty/nonempty send failed");
  }
  const auto empty_header_write = outgoing.write_observation(0);
  const auto data_header_write = outgoing.write_observation(1);
  const auto payload_write = outgoing.write_observation(2);
  const auto expected_empty_header =
      canonical_header_bytes(header(MessageType::shutdown, 0, 0));
  const auto expected_data_header = canonical_header_bytes(
      header(MessageType::data_chunk, 1, payload.size()));
  const auto decoded_written_header = ohl::parser::decode_frame_header(
      std::span<const std::byte, ohl::parser::kFrameHeaderBytes>{
          data_header_write.prefix});
  if (empty_header_write.size != ohl::parser::kFrameHeaderBytes ||
      empty_header_write.prefix != expected_empty_header ||
      data_header_write.size != ohl::parser::kFrameHeaderBytes ||
      data_header_write.prefix != expected_data_header ||
      payload_write.size != payload.size() ||
      payload_write.data_checksum != checksum(payload) ||
      empty_header_write.deadline != deadline ||
      data_header_write.deadline != deadline ||
      payload_write.deadline != deadline ||
      data_header_write.cancellation_requested ||
      !payload_write.cancellation_requested || !decoded_written_header.valid() ||
      decoded_written_header.header.type != MessageType::data_chunk ||
      decoded_written_header.header.payload_length != payload.size()) {
    return fail("send segmentation/deadline/token propagation failed");
  }

  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  const auto data_header =
      encode_header(header(MessageType::data_chunk, 1, payload.size()));
  ScriptedIo incoming;
  incoming.add_read(empty_header);
  incoming.add_read(data_header);
  incoming.add_read(payload);
  IsolatedWorkerCancellationSource incoming_cancellation;
  incoming.request_cancellation_after_read(1, incoming_cancellation);
  ParserFrameChannel receiver{kSession, incoming.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0xa5});
  const auto empty = receiver.receive(storage, deadline);
  const auto received =
      receiver.receive(storage, deadline, incoming_cancellation.token());
  if (!empty.valid() || !empty.frame.payload.empty() || !received.valid() ||
      received.frame.header.type != MessageType::data_chunk ||
      received.frame.payload.data() != storage.data() ||
      !std::ranges::equal(received.frame.payload, payload) ||
      incoming.read_calls() != 3 || incoming.abort_calls() != 0 ||
      incoming.empty_io_seen()) {
    return fail("canonical empty/nonempty receive or aliasing failed");
  }
  const auto empty_header_read = incoming.read_observation(0);
  const auto data_header_read = incoming.read_observation(1);
  const auto payload_read = incoming.read_observation(2);
  if (empty_header_read.size != ohl::parser::kFrameHeaderBytes ||
      data_header_read.size != ohl::parser::kFrameHeaderBytes ||
      payload_read.size != payload.size() ||
      empty_header_read.deadline != deadline ||
      data_header_read.deadline != deadline ||
      payload_read.deadline != deadline ||
      data_header_read.cancellation_requested ||
      !payload_read.cancellation_requested) {
    return fail("receive segmentation/deadline/token propagation failed");
  }
  return true;
}

[[nodiscard]] bool test_maximum_payload() {
  std::vector<std::byte> payload(ohl::parser::kMaximumFramePayloadBytes);
  for (std::size_t index = 0; index < payload.size(); ++index) {
    payload[index] = static_cast<std::byte>(index & 0xffU);
  }
  const auto maximum_header =
      header(MessageType::data_chunk, 1, payload.size());
  const auto encoded_header = encode_header(maximum_header);
  const auto deadline = Clock::now() + std::chrono::hours{2};
  ScriptedIo io;
  io.add_write();
  io.add_write();
  io.add_read(encoded_header);
  io.add_read(payload);
  ParserFrameChannel channel{kSession, io.operations()};
  if (!channel.send(maximum_header, payload, deadline).valid()) {
    return fail("maximum payload send failed");
  }
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto received = channel.receive(storage, deadline);
  if (!received.valid() || received.frame.payload.data() != storage.data() ||
      received.frame.payload.size() != payload.size() || storage != payload ||
      io.write_calls() != 2 || io.read_calls() != 2 || io.empty_io_seen()) {
    return fail("maximum payload receive/alias failed");
  }
  const auto write_header = io.write_observation(0);
  const auto write_payload = io.write_observation(1);
  const auto read_header = io.read_observation(0);
  const auto read_payload = io.read_observation(1);
  const auto expected_header = canonical_header_bytes(maximum_header);
  return write_header.size == ohl::parser::kFrameHeaderBytes &&
                 write_header.prefix == expected_header &&
                 write_payload.size == payload.size() &&
                 write_payload.data_checksum == checksum(payload) &&
                 read_header.size == ohl::parser::kFrameHeaderBytes &&
                 read_payload.size == payload.size() &&
                 write_header.deadline == deadline &&
                 write_payload.deadline == deadline &&
                 read_header.deadline == deadline &&
                 read_payload.deadline == deadline
             ? true
             : fail("maximum frame was not segmented into exact operations");
}

[[nodiscard]] bool test_outgoing_validation_without_io() {
  const std::array one{std::byte{0x11}};
  const auto rejects = [&](FrameHeader invalid,
                           const std::span<const std::byte> payload,
                           const ProtocolError expected) {
    ScriptedIo io;
    ParserFrameChannel channel{kSession, io.operations()};
    const auto result = channel.send(invalid, payload, Clock::time_point::max());
    const auto calls = io.write_calls();
    const auto sticky = channel.send(header(MessageType::shutdown, 0, 0), {},
                                     Clock::time_point::max());
    channel.abort();
    return same_result(result, ParserFrameChannelError::protocol_failure,
                       expected, IsolatedWorkerError::none) &&
           same_result(sticky, ParserFrameChannelError::protocol_failure,
                       expected, IsolatedWorkerError::none) &&
           channel.terminal() && io.write_calls() == calls && calls == 0 &&
           io.read_calls() == 0 && io.abort_calls() == 1;
  };

  auto invalid = header(MessageType::data_chunk, 1, one.size());
  invalid.major_version = 2;
  if (!rejects(invalid, one, ProtocolError::unsupported_version)) {
    return fail("outgoing unsupported version reached I/O");
  }
  invalid = header(MessageType::data_chunk, 1, one.size());
  invalid.type = static_cast<MessageType>(0xffffU);
  if (!rejects(invalid, one, ProtocolError::unknown_message_type)) {
    return fail("outgoing unknown type reached I/O");
  }
  invalid = header(MessageType::data_chunk, 1, one.size());
  invalid.flags = 1;
  if (!rejects(invalid, one, ProtocolError::reserved_flags)) {
    return fail("outgoing reserved flags reached I/O");
  }
  invalid = header(MessageType::data_chunk, 0, one.size());
  if (!rejects(invalid, one, ProtocolError::invalid_request_id)) {
    return fail("outgoing invalid request reached I/O");
  }
  invalid = header(MessageType::data_chunk, 1, one.size());
  invalid.session_id = kSession + 1;
  if (!rejects(invalid, one, ProtocolError::wrong_session_id)) {
    return fail("outgoing wrong session reached I/O");
  }
  invalid = header(MessageType::data_chunk, 1, one.size());
  invalid.session_id = 0;
  if (!rejects(invalid, one, ProtocolError::invalid_session_id)) {
    return fail("outgoing invalid session reached I/O");
  }
  invalid = header(MessageType::data_chunk, 1, 0);
  if (!rejects(invalid, one, ProtocolError::noncanonical_value)) {
    return fail("outgoing length mismatch reached I/O");
  }
  std::vector<std::byte> oversized(ohl::parser::kMaximumFramePayloadBytes + 1U);
  invalid = header(MessageType::data_chunk, 1, oversized.size());
  if (!rejects(invalid, oversized, ProtocolError::payload_too_large)) {
    return fail("outgoing oversized payload reached I/O");
  }
  return true;
}

[[nodiscard]] bool test_incoming_header_rejections() {
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto canonical =
      encode_header(header(MessageType::data_chunk, 1, 1));
  const auto rejects = [&](const auto& invalid, const ProtocolError expected) {
    ScriptedIo io;
    io.add_read(invalid);
    ParserFrameChannel channel{kSession, io.operations()};
    const auto result = channel.receive(storage, Clock::time_point::max());
    const auto reads = io.read_calls();
    const auto sticky = channel.receive(storage, Clock::time_point::max());
    channel.abort();
    return same_result(result.result, ParserFrameChannelError::protocol_failure,
                       expected, IsolatedWorkerError::none) &&
           same_result(sticky.result,
                       ParserFrameChannelError::protocol_failure, expected,
                       IsolatedWorkerError::none) &&
           !result.valid() && result.frame.payload.empty() &&
           channel.terminal() && reads == 1 && io.read_calls() == reads &&
           io.write_calls() == 0 && io.abort_calls() == 1;
  };

  auto invalid = canonical;
  invalid[0] = std::byte{0};
  if (!rejects(invalid, ProtocolError::invalid_magic)) {
    return fail("incoming invalid magic consumed payload");
  }
  invalid = canonical;
  invalid[4] = std::byte{2};
  if (!rejects(invalid, ProtocolError::unsupported_version)) {
    return fail("incoming invalid version consumed payload");
  }
  invalid = canonical;
  invalid[8] = std::byte{0xff};
  invalid[9] = std::byte{0xff};
  if (!rejects(invalid, ProtocolError::unknown_message_type)) {
    return fail("incoming unknown type consumed payload");
  }
  invalid = canonical;
  invalid[10] = std::byte{1};
  if (!rejects(invalid, ProtocolError::reserved_flags)) {
    return fail("incoming flags consumed payload");
  }
  invalid = canonical;
  std::fill(invalid.begin() + 24, invalid.end(), std::byte{0});
  if (!rejects(invalid, ProtocolError::invalid_request_id)) {
    return fail("incoming invalid request consumed payload");
  }
  invalid = canonical;
  std::fill(invalid.begin() + 16, invalid.begin() + 24, std::byte{0});
  if (!rejects(invalid, ProtocolError::invalid_session_id)) {
    return fail("incoming invalid session consumed payload");
  }
  invalid = canonical;
  invalid[16] ^= std::byte{1};
  if (!rejects(invalid, ProtocolError::wrong_session_id)) {
    return fail("incoming wrong session consumed payload");
  }
  invalid = canonical;
  constexpr auto over = ohl::parser::kMaximumFramePayloadBytes + 1U;
  invalid[12] = static_cast<std::byte>(over & 0xffU);
  invalid[13] = static_cast<std::byte>((over >> 8U) & 0xffU);
  invalid[14] = static_cast<std::byte>((over >> 16U) & 0xffU);
  invalid[15] = static_cast<std::byte>((over >> 24U) & 0xffU);
  if (!rejects(invalid, ProtocolError::payload_too_large)) {
    return fail("incoming oversized header consumed payload");
  }
  return true;
}

[[nodiscard]] bool test_receive_capacity_is_nonterminal() {
  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  ScriptedIo io;
  io.add_read(empty_header);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> short_storage(
      ohl::parser::kMaximumFramePayloadBytes - 1U);
  const auto short_result =
      channel.receive(short_storage, Clock::time_point::max());
  const auto null_result = channel.receive({}, Clock::time_point::max());
  if (short_result.result.error != ParserFrameChannelError::output_too_small ||
      null_result.result.error != ParserFrameChannelError::output_too_small ||
      channel.terminal() || io.read_calls() != 0 || io.abort_calls() != 0) {
    return fail("receive capacity rejection consumed header or poisoned");
  }
  std::vector<std::byte> exact_storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto recovered =
      channel.receive(exact_storage, Clock::time_point::max());
  return recovered.valid() && io.read_calls() == 1 && !channel.terminal()
             ? true
             : fail("channel did not recover after capacity rejection");
}

[[nodiscard]] bool test_invalid_configuration() {
  ScriptedIo io;
  {
    ParserFrameChannel channel{0, io.operations()};
    if (!same_result(channel.result(),
                     ParserFrameChannelError::invalid_configuration,
                     ProtocolError::none, IsolatedWorkerError::none) ||
        !channel.terminal() || io.read_calls() != 0 || io.write_calls() != 0 ||
        io.abort_calls() != 0) {
      return fail("zero-session channel configuration was accepted");
    }
  }

  const auto complete = io.operations();
  auto missing_read = complete;
  missing_read.read_exact = nullptr;
  auto missing_write = complete;
  missing_write.write_all = nullptr;
  auto missing_abort = complete;
  missing_abort.abort_io = nullptr;
  auto missing_context = complete;
  missing_context.context = nullptr;
  for (const auto operations :
       {missing_read, missing_write, missing_abort, missing_context}) {
    ParserFrameChannel channel{kSession, operations};
    if (!same_result(channel.result(),
                     ParserFrameChannelError::invalid_configuration,
                     ProtocolError::none, IsolatedWorkerError::none) ||
        !channel.terminal()) {
      return fail("partial frame-channel operation table was accepted");
    }
  }
  return io.read_calls() == 0 && io.write_calls() == 0 &&
                 io.abort_calls() == 0
             ? true
             : fail("invalid configuration reached I/O callbacks");
}

enum class FailureStage {
  send_header,
  send_payload,
  receive_header,
  receive_payload,
};

enum class FailureKind {
  short_success,
  zero_success,
  over_success,
  partial_timeout,
  exact_timeout,
  cancelled,
  peer_closed,
};

[[nodiscard]] bool test_transport_failure_matrix() {
  const std::array payload{std::byte{0x61}, std::byte{0x62}};
  const auto encoded_header =
      encode_header(header(MessageType::data_chunk, 1, payload.size()));
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto run = [&](const FailureStage stage, const FailureKind kind) {
    ScriptedIo io;
    const bool header_stage = stage == FailureStage::send_header ||
                              stage == FailureStage::receive_header;
    const auto expected_size =
        header_stage ? ohl::parser::kFrameHeaderBytes : payload.size();
    std::size_t transferred = expected_size;
    IsolatedWorkerError error = IsolatedWorkerError::none;
    IsolatedWorkerError expected_worker = IsolatedWorkerError::io_failure;
    switch (kind) {
      case FailureKind::short_success:
        transferred = expected_size - 1U;
        break;
      case FailureKind::zero_success:
        transferred = 0;
        break;
      case FailureKind::over_success:
        transferred = expected_size + 1U;
        break;
      case FailureKind::partial_timeout:
        transferred = expected_size - 1U;
        error = IsolatedWorkerError::timeout;
        expected_worker = IsolatedWorkerError::timeout;
        break;
      case FailureKind::exact_timeout:
        error = IsolatedWorkerError::timeout;
        expected_worker = IsolatedWorkerError::timeout;
        break;
      case FailureKind::cancelled:
        transferred = 0;
        error = IsolatedWorkerError::cancelled;
        expected_worker = IsolatedWorkerError::cancelled;
        break;
      case FailureKind::peer_closed:
        transferred = 0;
        error = IsolatedWorkerError::peer_closed;
        expected_worker = IsolatedWorkerError::peer_closed;
        break;
    }
    if (stage == FailureStage::send_header) {
      io.add_write(transferred, error);
    } else if (stage == FailureStage::send_payload) {
      io.add_write();
      io.add_write(transferred, error);
    } else if (stage == FailureStage::receive_header) {
      io.add_read(encoded_header, transferred, error);
    } else {
      io.add_read(encoded_header);
      io.add_read(payload, transferred, error);
    }
    ParserFrameChannel channel{kSession, io.operations()};
    ParserFrameChannelResult result;
    if (stage == FailureStage::send_header ||
        stage == FailureStage::send_payload) {
      result = channel.send(
          header(MessageType::data_chunk, 1, payload.size()), payload,
          Clock::time_point::max());
    } else {
      result = channel.receive(storage, Clock::time_point::max()).result;
    }
    const auto reads = io.read_calls();
    const auto writes = io.write_calls();
    const auto sticky = channel.send(header(MessageType::shutdown, 0, 0), {},
                                     Clock::time_point::max());
    channel.abort();
    channel.abort();
    return same_result(result, ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           same_result(sticky, ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           same_result(channel.result(),
                       ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           channel.terminal() && io.read_calls() == reads &&
           io.write_calls() == writes && io.abort_calls() == 1;
  };

  for (const auto stage : {FailureStage::send_header,
                           FailureStage::send_payload,
                           FailureStage::receive_header,
                           FailureStage::receive_payload}) {
    for (const auto kind :
         {FailureKind::short_success, FailureKind::zero_success,
          FailureKind::over_success, FailureKind::partial_timeout,
          FailureKind::exact_timeout, FailureKind::cancelled,
          FailureKind::peer_closed}) {
      if (!run(stage, kind)) {
        return fail("transport normalization/failure matrix was not sticky");
      }
    }
  }
  return true;
}

[[nodiscard]] bool test_partial_payload_invalidates_entire_buffer() {
  const std::array payload{std::byte{0x91}, std::byte{0x92},
                           std::byte{0x93}, std::byte{0x94}};
  const auto encoded_header =
      encode_header(header(MessageType::data_chunk, 7, payload.size()));
  ScriptedIo io;
  io.add_read(encoded_header);
  io.add_read(payload, 2, IsolatedWorkerError::peer_closed);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0xa5});
  const auto result = channel.receive(storage, Clock::time_point::max());
  const auto reads = io.read_calls();
  const auto sticky = channel.receive(storage, Clock::time_point::max());
  if (result.valid() || result.result.error !=
                            ParserFrameChannelError::transport_failure ||
      result.result.protocol_error != ProtocolError::none ||
      result.result.worker_error != IsolatedWorkerError::peer_closed ||
      result.frame.header.type == MessageType::data_chunk ||
      !result.frame.payload.empty() || result.frame.payload.data() != nullptr ||
      storage[0] != payload[0] || storage[1] != payload[1] ||
      storage[2] != std::byte{0xa5} || storage[3] != std::byte{0xa5} ||
      !std::ranges::all_of(
          std::span<const std::byte>{storage}.subspan(payload.size()),
          [](const auto byte) { return byte == std::byte{0xa5}; }) ||
      !same_result(sticky.result,
                   ParserFrameChannelError::transport_failure,
                   ProtocolError::none, IsolatedWorkerError::peer_closed) ||
      !sticky.frame.payload.empty() || !channel.terminal() || reads != 2 ||
      io.read_calls() != reads || io.abort_calls() != 1) {
    return fail("partial payload failure exposed a usable frame or clean buffer");
  }
  std::fill(storage.begin(), storage.end(), std::byte{0});
  return true;
}

[[nodiscard]] bool test_same_direction_exclusion() {
  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  {
    ScriptedIo io;
    io.add_write(kExactTransfer, IsolatedWorkerError::none, true);
    ParserFrameChannel channel{kSession, io.operations()};
    ParserFrameChannelResult first;
    std::thread writer{[&] {
      first = channel.send(header(MessageType::shutdown, 0, 0), {},
                           Clock::time_point::max());
    }};
    if (!io.wait_until_entered(0, 1)) {
      channel.abort();
      writer.join();
      return fail("blocked writer did not enter deterministically");
    }
    const auto duplicate =
        channel.send(header(MessageType::shutdown, 0, 0), {},
                     Clock::time_point::max());
    const bool before_release =
        duplicate.error == ParserFrameChannelError::concurrent_operation &&
        !channel.terminal() && io.write_calls() == 1 && io.abort_calls() == 0;
    io.release_writes();
    writer.join();
    if (!before_release || !first.valid() || channel.terminal()) {
      return fail("duplicate writer poisoned or reached I/O");
    }
  }

  {
    ScriptedIo io;
    io.add_read(empty_header, kExactTransfer, IsolatedWorkerError::none, true);
    ParserFrameChannel channel{kSession, io.operations()};
    std::vector<std::byte> first_storage(
        ohl::parser::kMaximumFramePayloadBytes);
    std::vector<std::byte> second_storage(
        ohl::parser::kMaximumFramePayloadBytes);
    ohl::media::ParserFrameReceiveResult first;
    std::thread reader{[&] {
      first = channel.receive(first_storage, Clock::time_point::max());
    }};
    if (!io.wait_until_entered(1, 0)) {
      channel.abort();
      reader.join();
      return fail("blocked reader did not enter deterministically");
    }
    const auto duplicate =
        channel.receive(second_storage, Clock::time_point::max());
    const bool before_release =
        duplicate.result.error ==
            ParserFrameChannelError::concurrent_operation &&
        !channel.terminal() && io.read_calls() == 1 && io.abort_calls() == 0;
    io.release_reads();
    reader.join();
    if (!before_release || !first.valid() || channel.terminal()) {
      return fail("duplicate reader poisoned or reached I/O");
    }
  }
  return true;
}

[[nodiscard]] bool test_reader_writer_overlap() {
  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  ScriptedIo io;
  io.add_read(empty_header, kExactTransfer, IsolatedWorkerError::none, true);
  io.add_write(kExactTransfer, IsolatedWorkerError::none, true);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  ohl::media::ParserFrameReceiveResult received;
  ParserFrameChannelResult sent;
  std::thread reader{[&] {
    received = channel.receive(storage, Clock::time_point::max());
  }};
  std::thread writer{[&] {
    sent = channel.send(header(MessageType::shutdown, 0, 0), {},
                        Clock::time_point::max());
  }};
  if (!io.wait_until_entered(1, 1)) {
    channel.abort();
    reader.join();
    writer.join();
    return fail("reader/writer did not overlap deterministically");
  }
  const bool active_ok = !channel.terminal() && io.abort_calls() == 0;
  io.release_all();
  reader.join();
  writer.join();
  return active_ok && received.valid() && sent.valid() &&
                 io.read_calls() == 1 && io.write_calls() == 1
             ? true
             : fail("one reader plus one writer overlap failed");
}

[[nodiscard]] bool test_cross_direction_failure_wakes_peer() {
  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  const auto run = [&](const bool reader_fails) {
    ScriptedIo io;
    io.add_read(empty_header, kExactTransfer,
                reader_fails ? IsolatedWorkerError::peer_closed
                             : IsolatedWorkerError::none,
                true);
    io.add_write(kExactTransfer,
                 reader_fails ? IsolatedWorkerError::none
                              : IsolatedWorkerError::timeout,
                 true);
    ParserFrameChannel channel{kSession, io.operations()};
    std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
    ohl::media::ParserFrameReceiveResult received;
    ParserFrameChannelResult sent;
    std::thread reader{[&] {
      received = channel.receive(storage, Clock::time_point::max());
    }};
    std::thread writer{[&] {
      sent = channel.send(header(MessageType::shutdown, 0, 0), {},
                          Clock::time_point::max());
    }};
    if (!io.wait_until_entered(1, 1)) {
      channel.abort();
      reader.join();
      writer.join();
      return false;
    }
    if (reader_fails) {
      io.release_reads();
    } else {
      io.release_writes();
    }
    reader.join();
    writer.join();
    const auto expected_worker = reader_fails
                                     ? IsolatedWorkerError::peer_closed
                                     : IsolatedWorkerError::timeout;
    const auto reads = io.read_calls();
    const auto writes = io.write_calls();
    const auto later =
        channel.send(header(MessageType::shutdown, 0, 0), {},
                     Clock::time_point::max());
    return same_result(received.result,
                       ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           same_result(sent, ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           same_result(channel.result(),
                       ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           same_result(later, ParserFrameChannelError::transport_failure,
                       ProtocolError::none, expected_worker) &&
           channel.terminal() && io.abort_calls() == 1 &&
           io.read_calls() == reads && io.write_calls() == writes;
  };
  return run(true) && run(false)
             ? true
             : fail("cross-direction failure did not retain first cause/wake peer");
}

[[nodiscard]] bool test_abort_wakes_active_operations() {
  const auto empty_header = encode_header(header(MessageType::shutdown, 0, 0));
  ScriptedIo io;
  io.add_read(empty_header, kExactTransfer, IsolatedWorkerError::none, true);
  io.add_write(kExactTransfer, IsolatedWorkerError::none, true);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  ohl::media::ParserFrameReceiveResult received;
  ParserFrameChannelResult sent;
  std::thread reader{[&] {
    received = channel.receive(storage, Clock::time_point::max());
  }};
  std::thread writer{[&] {
    sent = channel.send(header(MessageType::shutdown, 0, 0), {},
                        Clock::time_point::max());
  }};
  if (!io.wait_until_entered(1, 1)) {
    channel.abort();
    reader.join();
    writer.join();
    return fail("abort fixtures did not enter active I/O");
  }
  channel.abort();
  reader.join();
  writer.join();
  channel.abort();
  const auto reads = io.read_calls();
  const auto writes = io.write_calls();
  const auto later_send =
      channel.send(header(MessageType::shutdown, 0, 0), {},
                   Clock::time_point::max());
  const auto later_receive =
      channel.receive(storage, Clock::time_point::max());
  const auto is_aborted = [](const ParserFrameChannelResult& result) {
    return same_result(result, ParserFrameChannelError::aborted,
                       ProtocolError::none, IsolatedWorkerError::none);
  };
  return is_aborted(sent) && is_aborted(received.result) &&
                 is_aborted(channel.result()) && is_aborted(later_send) &&
                 is_aborted(later_receive.result) && io.abort_calls() == 1 &&
                 io.read_calls() == reads && io.write_calls() == writes
             ? true
             : fail("abort did not wake, retain first cause, or stay idempotent");
}

}  // namespace

int main() {
  return test_canonical_send_receive() && test_maximum_payload() &&
                 test_outgoing_validation_without_io() &&
                 test_incoming_header_rejections() &&
                 test_receive_capacity_is_nonterminal() &&
                 test_invalid_configuration() &&
                 test_transport_failure_matrix() &&
                 test_partial_payload_invalidates_entire_buffer() &&
                 test_same_direction_exclusion() &&
                 test_reader_writer_overlap() &&
                 test_cross_direction_failure_wakes_peer() &&
                 test_abort_wakes_active_operations()
             ? 0
             : 1;
}
