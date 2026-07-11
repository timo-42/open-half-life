#include "ohl/media/parser_parent_handshake.hpp"

#include "synthetic_media_test_support.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <limits>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;
using ohl::media::ParserFrameChannel;
using ohl::media::ParserFrameChannelError;
using ohl::media::ParserFrameChannelOperations;
using ohl::media::ParserFrameChannelResult;
using ohl::media::ParserParentHandshakeError;
using ohl::media::ParserParentHandshakeProof;
using ohl::media::ParserParentHandshakeResult;
using ohl::media::ParserSourceReadLimits;
using ohl::parser::MessageType;
using ohl::parser::ProtocolBudgets;
using ohl::parser::ProtocolError;
using ohl::parser::SessionState;
using ohl::platform::IsolatedWorkerCancellationSource;
using ohl::platform::IsolatedWorkerCancellationToken;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerIoResult;

constexpr std::uint64_t kSession = 0x0123'4567'89ab'cdefULL;
constexpr std::size_t kExactTransfer =
    std::numeric_limits<std::size_t>::max();

static_assert(!std::is_copy_constructible_v<ParserParentHandshakeProof>);
static_assert(!std::is_copy_assignable_v<ParserParentHandshakeProof>);
static_assert(std::is_nothrow_move_constructible_v<ParserParentHandshakeProof>);
static_assert(std::is_nothrow_move_assignable_v<ParserParentHandshakeProof>);

template <typename T>
concept HasFrameMember = requires(T value) { value.frame; };

template <typename T>
concept HasPayloadMember = requires(T value) { value.payload; };

static_assert(!HasFrameMember<ParserParentHandshakeResult>);
static_assert(!HasPayloadMember<ParserParentHandshakeResult>);

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

void store_u16(const std::span<std::byte> output, const std::size_t offset,
               const std::uint16_t value) {
  output[offset] = static_cast<std::byte>(value & 0xffU);
  output[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

void store_u32(const std::span<std::byte> output, const std::size_t offset,
               const std::uint32_t value) {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

void store_u64(const std::span<std::byte> output, const std::size_t offset,
               const std::uint64_t value) {
  for (std::size_t index = 0; index < sizeof(value); ++index) {
    output[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

// These wire builders deliberately do not use either production encoder.
[[nodiscard]] std::array<std::byte, 32> canonical_header(
    const MessageType type, const std::uint32_t payload_length,
    const std::uint64_t session_id, const std::uint64_t request_id) {
  std::array<std::byte, 32> bytes{};
  bytes[0] = std::byte{'O'};
  bytes[1] = std::byte{'H'};
  bytes[2] = std::byte{'L'};
  bytes[3] = std::byte{'P'};
  store_u16(bytes, 4, 1);
  store_u16(bytes, 6, 0);
  store_u16(bytes, 8, static_cast<std::uint16_t>(type));
  store_u16(bytes, 10, 0);
  store_u32(bytes, 12, payload_length);
  store_u64(bytes, 16, session_id);
  store_u64(bytes, 24, request_id);
  return bytes;
}

[[nodiscard]] std::array<std::byte, 12> canonical_hello(
    const std::uint64_t source_size,
    const std::uint32_t maximum_read_bytes) {
  std::array<std::byte, 12> bytes{};
  store_u64(bytes, 0, source_size);
  store_u32(bytes, 8, maximum_read_bytes);
  return bytes;
}

[[nodiscard]] std::uint64_t checksum(
    const std::span<const std::byte> bytes) noexcept {
  std::uint64_t value = 0;
  for (const auto byte : bytes) {
    value = value * 131U + std::to_integer<std::uint8_t>(byte);
  }
  return value;
}

struct IoPlan {
  std::span<const std::byte> input;
  std::size_t bytes_transferred{kExactTransfer};
  IsolatedWorkerError error{IsolatedWorkerError::none};
};

struct IoObservation {
  std::size_t size{0};
  Clock::time_point deadline{};
  IsolatedWorkerCancellationToken cancellation;
  std::uint64_t data_checksum{0};
  std::array<std::byte, 32> prefix{};
  std::size_t prefix_size{0};
};

class ScriptedIo final {
 public:
  void add_read(const std::span<const std::byte> input,
                const std::size_t bytes_transferred = kExactTransfer,
                const IsolatedWorkerError error = IsolatedWorkerError::none) {
    read_plans_[read_plan_count_++] = {input, bytes_transferred, error};
  }

  void add_write(const std::size_t bytes_transferred = kExactTransfer,
                 const IsolatedWorkerError error = IsolatedWorkerError::none) {
    write_plans_[write_plan_count_++] = {{}, bytes_transferred, error};
  }

  [[nodiscard]] ParserFrameChannelOperations operations() noexcept {
    return {
        .read_exact = read_exact,
        .write_all = write_all,
        .abort_io = abort_io,
        .context = this,
    };
  }

  [[nodiscard]] std::size_t read_calls() const noexcept { return read_calls_; }
  [[nodiscard]] std::size_t write_calls() const noexcept {
    return write_calls_;
  }
  [[nodiscard]] std::size_t abort_calls() const noexcept {
    return abort_calls_;
  }
  [[nodiscard]] bool empty_io_seen() const noexcept { return empty_io_seen_; }
  [[nodiscard]] const IoObservation& read_observation(
      const std::size_t index) const noexcept {
    return read_observations_[index];
  }
  [[nodiscard]] const IoObservation& write_observation(
      const std::size_t index) const noexcept {
    return write_observations_[index];
  }

 private:
  template <typename Byte>
  static void observe(IoObservation& observation, const std::span<Byte> bytes,
                      const Clock::time_point deadline,
                      const IsolatedWorkerCancellationToken cancellation) {
    observation.size = bytes.size();
    observation.deadline = deadline;
    observation.cancellation = cancellation;
    observation.data_checksum =
        checksum(std::as_bytes(std::span{bytes.data(), bytes.size()}));
    observation.prefix_size =
        std::min(bytes.size(), observation.prefix.size());
    std::copy_n(bytes.begin(), observation.prefix_size,
                observation.prefix.begin());
  }

  [[nodiscard]] static IsolatedWorkerIoResult read_exact(
      void* const opaque, const std::span<std::byte> destination,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept {
    auto& io = *static_cast<ScriptedIo*>(opaque);
    const auto call = io.read_calls_++;
    io.empty_io_seen_ = io.empty_io_seen_ || destination.empty();
    observe(io.read_observations_[call], destination, deadline, cancellation);
    const auto plan = call < io.read_plan_count_
                          ? io.read_plans_[call]
                          : IoPlan{{}, 0, IsolatedWorkerError::io_failure};
    const auto transferred = plan.bytes_transferred == kExactTransfer
                                 ? destination.size()
                                 : plan.bytes_transferred;
    const auto copied =
        std::min({destination.size(), plan.input.size(), transferred});
    std::copy_n(plan.input.begin(), copied, destination.begin());
    return {.bytes_transferred = transferred, .error = plan.error};
  }

  [[nodiscard]] static IsolatedWorkerIoResult write_all(
      void* const opaque, const std::span<const std::byte> source,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept {
    auto& io = *static_cast<ScriptedIo*>(opaque);
    const auto call = io.write_calls_++;
    io.empty_io_seen_ = io.empty_io_seen_ || source.empty();
    observe(io.write_observations_[call], source, deadline, cancellation);
    const auto plan = call < io.write_plan_count_
                          ? io.write_plans_[call]
                          : IoPlan{{}, 0, IsolatedWorkerError::io_failure};
    return {
        .bytes_transferred = plan.bytes_transferred == kExactTransfer
                                 ? source.size()
                                 : plan.bytes_transferred,
        .error = plan.error,
    };
  }

  static void abort_io(void* const opaque) noexcept {
    ++static_cast<ScriptedIo*>(opaque)->abort_calls_;
  }

  std::array<IoPlan, 8> read_plans_{};
  std::array<IoPlan, 8> write_plans_{};
  std::array<IoObservation, 8> read_observations_{};
  std::array<IoObservation, 8> write_observations_{};
  std::size_t read_plan_count_{0};
  std::size_t write_plan_count_{0};
  std::size_t read_calls_{0};
  std::size_t write_calls_{0};
  std::size_t abort_calls_{0};
  bool empty_io_seen_{false};
};

[[nodiscard]] bool same_channel_result(
    const ParserFrameChannelResult& result, const ParserFrameChannelError error,
    const ProtocolError protocol_error,
    const IsolatedWorkerError worker_error) noexcept {
  return result.error == error && result.protocol_error == protocol_error &&
         result.worker_error == worker_error;
}

[[nodiscard]] bool untouched(const std::span<const std::byte> storage,
                             const std::byte sentinel) {
  return std::all_of(storage.begin(), storage.end(),
                     [sentinel](const auto byte) { return byte == sentinel; });
}

[[nodiscard]] ParserParentHandshakeResult make_minimum_success(
    const ohl::media::ValidatedMedia& media,
    const ParserSourceReadLimits limits) {
  const auto ready = canonical_header(MessageType::ready, 0, kSession, 0);
  ScriptedIo io;
  io.add_write();
  io.add_write();
  io.add_read(ready);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  return ohl::media::perform_parser_parent_handshake(
      channel, media, limits,
      {.maximum_messages = 2, .maximum_payload_bytes = 12}, storage,
      Clock::time_point::max());
}

[[nodiscard]] bool test_canonical_handshake_and_proof() {
  ohl::media::test::SyntheticValidatedMedia fixture;
  const ParserSourceReadLimits limits{
      .maximum_read_bytes = 0x0001'0203U,
      .maximum_requests = 17,
      .maximum_reply_payload_bytes =
          ohl::parser::kReadReplyPrefixBytes + 0x0001'0203U,
  };
  constexpr ProtocolBudgets budgets{
      .maximum_messages = 2,
      .maximum_payload_bytes = 12,
  };
  const auto expected_ready = canonical_header(MessageType::ready, 0, kSession,
                                                0);
  ScriptedIo io;
  io.add_write();
  io.add_write();
  io.add_read(expected_ready);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0xa5});
  const auto deadline = Clock::now() + std::chrono::hours{4};
  IsolatedWorkerCancellationSource cancellation;

  auto result = ohl::media::perform_parser_parent_handshake(
      channel, fixture.media(), limits, budgets, storage, deadline,
      cancellation.token());
  const auto expected_header = canonical_header(
      MessageType::hello, 12, kSession, 0);
  const auto expected_payload = canonical_hello(
      fixture.media().fingerprint().size_bytes, limits.maximum_read_bytes);
  const auto& header_write = io.write_observation(0);
  const auto& payload_write = io.write_observation(1);
  const auto& header_read = io.read_observation(0);
  if (!result.valid() || !result.channel_result.valid() || channel.terminal() ||
      io.write_calls() != 2 || io.read_calls() != 1 || io.abort_calls() != 0 ||
      io.empty_io_seen() || header_write.size != expected_header.size() ||
      header_write.prefix != expected_header ||
      payload_write.size != expected_payload.size() ||
      payload_write.data_checksum != checksum(expected_payload) ||
      !std::equal(expected_payload.begin(), expected_payload.end(),
                  payload_write.prefix.begin()) ||
      header_read.size != expected_ready.size() ||
      header_write.deadline != deadline || payload_write.deadline != deadline ||
      header_read.deadline != deadline ||
      header_write.cancellation.cancellation_requested() ||
      payload_write.cancellation.cancellation_requested() ||
      header_read.cancellation.cancellation_requested() ||
      !untouched(storage, std::byte{0xa5})) {
    return fail("canonical handshake framing or forwarding failed");
  }
  if (!cancellation.request_cancellation() ||
      !header_write.cancellation.cancellation_requested() ||
      !payload_write.cancellation.cancellation_requested() ||
      !header_read.cancellation.cancellation_requested()) {
    return fail("handshake did not preserve cancellation token identity");
  }

  const auto returned_limits = result.proof->source_read_limits();
  const auto policy = result.proof->source_read_policy();
  if (returned_limits.maximum_read_bytes != limits.maximum_read_bytes ||
      returned_limits.maximum_requests != limits.maximum_requests ||
      returned_limits.maximum_reply_payload_bytes !=
          limits.maximum_reply_payload_bytes ||
      policy.source_size != fixture.media().fingerprint().size_bytes ||
      policy.maximum_read_bytes != limits.maximum_read_bytes || !policy.valid()) {
    return fail("handshake proof did not retain exact source limits and policy");
  }
  auto protocol = result.proof->take_protocol();
  if (!protocol.has_value() || result.valid() || result.proof->valid() ||
      result.proof->take_protocol().has_value() ||
      protocol->error() != ProtocolError::none ||
      protocol->state() != SessionState::idle ||
      protocol->active_request_id() != 0 || protocol->message_count() != 2 ||
      protocol->payload_bytes() != 12) {
    return fail("handshake proof was not idle, exact, and single-consumption");
  }

  ohl::media::ParserResultSession session{std::move(*protocol), 0x42U};
  ohl::media::ParserSourceReadBroker broker{fixture.media(), session,
                                             returned_limits};
  if (session.terminal() || session.protocol_state() != SessionState::idle ||
      broker.terminal() || !broker.policy().valid() ||
      broker.policy().source_size != policy.source_size ||
      broker.policy().maximum_read_bytes != policy.maximum_read_bytes) {
    return fail("handshake proof did not construct the downstream session/broker");
  }

  auto move_source_result = make_minimum_success(fixture.media(), limits);
  auto move_destination_result = make_minimum_success(fixture.media(), limits);
  if (!move_source_result.valid() || !move_destination_result.valid()) {
    return fail("proof move fixtures did not handshake successfully");
  }
  ParserParentHandshakeProof move_source{
      std::move(*move_source_result.proof)};
  ParserParentHandshakeProof move_destination{
      std::move(*move_destination_result.proof)};
  if (!move_source.valid() || !move_destination.valid() ||
      move_source_result.valid() || move_destination_result.valid() ||
      move_source_result.proof->valid() ||
      move_destination_result.proof->valid()) {
    return fail("proof move construction did not invalidate both sources");
  }
  move_destination = std::move(move_source);
  auto assigned_protocol = move_destination.take_protocol();
  if (move_source.valid() || !assigned_protocol.has_value() ||
      move_destination.valid() ||
      move_destination.take_protocol().has_value() ||
      assigned_protocol->state() != SessionState::idle ||
      assigned_protocol->message_count() != 2 ||
      assigned_protocol->payload_bytes() != 12) {
    return fail("proof move assignment or single consumption failed");
  }
  return true;
}

[[nodiscard]] bool expect_preflight_failure(
    const ohl::media::ValidatedMedia& media,
    const ParserSourceReadLimits limits, const ProtocolBudgets budgets,
    const std::span<std::byte> storage,
    const ParserParentHandshakeError expected_error) {
  ScriptedIo io;
  ParserFrameChannel channel{kSession, io.operations()};
  const auto result = ohl::media::perform_parser_parent_handshake(
      channel, media, limits, budgets, storage, Clock::time_point::max());
  return !result.valid() && !result.proof.has_value() &&
         result.error == expected_error && result.protocol_error ==
                                               ProtocolError::none &&
         result.channel_result.valid() && !channel.terminal() &&
         io.read_calls() == 0 && io.write_calls() == 0 &&
         io.abort_calls() == 0 && !io.empty_io_seen();
}

[[nodiscard]] bool test_preflight_zero_io_failures() {
  ohl::media::test::SyntheticValidatedMedia fixture;
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0xa5});
  constexpr auto valid_limits = ParserSourceReadLimits{};
  constexpr auto valid_budgets = ProtocolBudgets{};

  ohl::media::test::SyntheticValidatedMedia moved_fixture;
  ohl::media::ValidatedMedia retained{std::move(moved_fixture.media())};
  if (!retained.valid() || moved_fixture.media().valid() ||
      !expect_preflight_failure(moved_fixture.media(), valid_limits,
                                valid_budgets, storage,
                                ParserParentHandshakeError::invalid_configuration)) {
    return fail("moved-from media was not rejected before I/O");
  }

  const std::array invalid_limits{
      ParserSourceReadLimits{.maximum_read_bytes = 0},
      ParserSourceReadLimits{
          .maximum_read_bytes = ohl::parser::kMaximumReadBytes + 1U},
      ParserSourceReadLimits{.maximum_requests = 0},
      ParserSourceReadLimits{
          .maximum_requests = ohl::parser::kMaximumProtocolMessages / 2U + 1U},
      ParserSourceReadLimits{
          .maximum_read_bytes = 4,
          .maximum_reply_payload_bytes =
              ohl::parser::kReadReplyPrefixBytes + 3U},
      ParserSourceReadLimits{
          .maximum_reply_payload_bytes =
              ohl::parser::kMaximumCumulativePayloadBytes + 1U},
  };
  for (const auto limits : invalid_limits) {
    if (!expect_preflight_failure(
            fixture.media(), limits, valid_budgets, storage,
            ParserParentHandshakeError::invalid_configuration)) {
      return fail("invalid source limits performed handshake I/O");
    }
  }

  const std::array invalid_budgets{
      ProtocolBudgets{.maximum_messages = 0},
      ProtocolBudgets{
          .maximum_messages = ohl::parser::kMaximumProtocolMessages + 1U},
      ProtocolBudgets{.maximum_payload_bytes = 0},
      ProtocolBudgets{
          .maximum_payload_bytes =
              ohl::parser::kMaximumCumulativePayloadBytes + 1U},
      ProtocolBudgets{.maximum_messages = 1, .maximum_payload_bytes = 12},
      ProtocolBudgets{.maximum_messages = 2, .maximum_payload_bytes = 11},
  };
  for (const auto budgets : invalid_budgets) {
    if (!expect_preflight_failure(
            fixture.media(), valid_limits, budgets, storage,
            ParserParentHandshakeError::invalid_configuration)) {
      return fail("invalid or insufficient protocol budget performed I/O");
    }
  }

  if (!expect_preflight_failure(
          fixture.media(), valid_limits, valid_budgets,
          std::span<std::byte>{storage}.first(storage.size() - 1U),
          ParserParentHandshakeError::output_too_small) ||
      !expect_preflight_failure(fixture.media(), valid_limits, valid_budgets,
                                {}, ParserParentHandshakeError::output_too_small)) {
    return fail("short or null receive storage performed handshake I/O");
  }

  {
    ScriptedIo io;
    ParserFrameChannel channel{0, io.operations()};
    const auto result = ohl::media::perform_parser_parent_handshake(
        channel, fixture.media(), valid_limits, valid_budgets, storage,
        Clock::time_point::max());
    if (result.valid() || result.proof.has_value() ||
        result.error != ParserParentHandshakeError::channel_failure ||
        !same_channel_result(result.channel_result,
                             ParserFrameChannelError::invalid_configuration,
                             ProtocolError::none, IsolatedWorkerError::none) ||
        io.read_calls() != 0 || io.write_calls() != 0 ||
        io.abort_calls() != 0) {
      return fail("terminal invalid channel did not preserve its zero-I/O cause");
    }
  }
  {
    ScriptedIo io;
    ParserFrameChannel channel{kSession, io.operations()};
    channel.abort();
    const auto result = ohl::media::perform_parser_parent_handshake(
        channel, fixture.media(), valid_limits, valid_budgets, storage,
        Clock::time_point::max());
    if (result.valid() || result.proof.has_value() ||
        result.error != ParserParentHandshakeError::channel_failure ||
        !same_channel_result(result.channel_result,
                             ParserFrameChannelError::aborted,
                             ProtocolError::none, IsolatedWorkerError::none) ||
        io.read_calls() != 0 || io.write_calls() != 0 ||
        io.abort_calls() != 1) {
      return fail("pre-aborted channel did not remain zero-I/O and sticky");
    }
  }
  return true;
}

struct ResponseCase {
  std::string_view name;
  std::array<std::byte, 32> header;
  std::span<const std::byte> payload;
  ParserParentHandshakeError handshake_error;
  ParserFrameChannelError channel_error;
  ProtocolError protocol_error;
};

[[nodiscard]] bool test_invalid_responses() {
  ohl::media::test::SyntheticValidatedMedia fixture;
  const std::array one_byte{std::byte{0x5a}};
  auto malformed = canonical_header(MessageType::ready, 0, kSession, 0);
  malformed[0] = std::byte{'X'};
  const std::array cases{
      ResponseCase{"wrong session",
                   canonical_header(MessageType::ready, 0, kSession + 1U, 0),
                   {}, ParserParentHandshakeError::channel_failure,
                   ParserFrameChannelError::protocol_failure,
                   ProtocolError::wrong_session_id},
      ResponseCase{"wrong request",
                   canonical_header(MessageType::ready, 0, kSession, 1), {},
                   ParserParentHandshakeError::channel_failure,
                   ParserFrameChannelError::protocol_failure,
                   ProtocolError::invalid_request_id},
      ResponseCase{"wrong type",
                   canonical_header(MessageType::shutdown, 0, kSession, 0), {},
                   ParserParentHandshakeError::protocol_failure,
                   ParserFrameChannelError::aborted,
                   ProtocolError::unexpected_message},
      ResponseCase{"nonempty ready",
                   canonical_header(MessageType::ready, 1, kSession, 0),
                   one_byte, ParserParentHandshakeError::protocol_failure,
                   ParserFrameChannelError::aborted,
                   ProtocolError::payload_trailing_bytes},
      ResponseCase{"malformed header", malformed, {},
                   ParserParentHandshakeError::channel_failure,
                   ParserFrameChannelError::protocol_failure,
                   ProtocolError::invalid_magic},
  };

  for (const auto& test_case : cases) {
    ScriptedIo io;
    io.add_write();
    io.add_write();
    io.add_read(test_case.header);
    if (!test_case.payload.empty()) {
      io.add_read(test_case.payload);
    }
    ParserFrameChannel channel{kSession, io.operations()};
    std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                   std::byte{0xa5});
    const auto result = ohl::media::perform_parser_parent_handshake(
        channel, fixture.media(), {}, {}, storage, Clock::time_point::max());
    const auto reads_before = io.read_calls();
    const auto writes_before = io.write_calls();
    const auto later = channel.send(
        {.type = MessageType::shutdown,
         .payload_length = 0,
         .session_id = kSession,
         .request_id = 0},
        {}, Clock::time_point::max());
    if (result.valid() || result.proof.has_value() ||
        result.error != test_case.handshake_error ||
        result.protocol_error != test_case.protocol_error ||
        !same_channel_result(result.channel_result, test_case.channel_error,
                             test_case.channel_error ==
                                     ParserFrameChannelError::protocol_failure
                                 ? test_case.protocol_error
                                 : ProtocolError::none,
                             IsolatedWorkerError::none) ||
        !channel.terminal() || io.abort_calls() != 1 ||
        io.read_calls() != reads_before || io.write_calls() != writes_before ||
        later.error != test_case.channel_error ||
        (test_case.payload.empty() && !untouched(storage, std::byte{0xa5}))) {
      std::cerr << "invalid response case failed: " << test_case.name << '\n';
      return false;
    }
    std::fill(storage.begin(), storage.end(), std::byte{0});
  }
  return true;
}

enum class FailureStage { send_header, send_payload, receive_header };

struct TransportCase {
  std::string_view name;
  FailureStage stage;
  std::size_t bytes_transferred;
  IsolatedWorkerError reported_error;
  IsolatedWorkerError expected_error;
};

[[nodiscard]] bool test_transport_failures() {
  ohl::media::test::SyntheticValidatedMedia fixture;
  const auto ready = canonical_header(MessageType::ready, 0, kSession, 0);
  const std::array cases{
      TransportCase{"header timeout", FailureStage::send_header, 0,
                    IsolatedWorkerError::timeout, IsolatedWorkerError::timeout},
      TransportCase{"payload cancellation", FailureStage::send_payload, 0,
                    IsolatedWorkerError::cancelled,
                    IsolatedWorkerError::cancelled},
      TransportCase{"partial peer close", FailureStage::receive_header, 7,
                    IsolatedWorkerError::peer_closed,
                    IsolatedWorkerError::peer_closed},
      TransportCase{"exact I/O failure", FailureStage::receive_header, 32,
                    IsolatedWorkerError::io_failure,
                    IsolatedWorkerError::io_failure},
      TransportCase{"short success", FailureStage::receive_header, 31,
                    IsolatedWorkerError::none,
                    IsolatedWorkerError::io_failure},
      TransportCase{"over success", FailureStage::send_payload, 13,
                    IsolatedWorkerError::none,
                    IsolatedWorkerError::io_failure},
  };

  for (const auto& test_case : cases) {
    ScriptedIo io;
    if (test_case.stage == FailureStage::send_header) {
      io.add_write(test_case.bytes_transferred, test_case.reported_error);
    } else {
      io.add_write();
      io.add_write(test_case.stage == FailureStage::send_payload
                       ? test_case.bytes_transferred
                       : kExactTransfer,
                   test_case.stage == FailureStage::send_payload
                       ? test_case.reported_error
                       : IsolatedWorkerError::none);
      if (test_case.stage == FailureStage::receive_header) {
        io.add_read(ready, test_case.bytes_transferred,
                    test_case.reported_error);
      }
    }
    ParserFrameChannel channel{kSession, io.operations()};
    std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                   std::byte{0xa5});
    const auto result = ohl::media::perform_parser_parent_handshake(
        channel, fixture.media(), {}, {}, storage, Clock::time_point::max());
    const auto reads_before = io.read_calls();
    const auto writes_before = io.write_calls();
    const auto later = channel.send(
        {.type = MessageType::shutdown,
         .payload_length = 0,
         .session_id = kSession,
         .request_id = 0},
        {}, Clock::time_point::max());
    if (result.valid() || result.proof.has_value() ||
        result.error != ParserParentHandshakeError::channel_failure ||
        result.protocol_error != ProtocolError::none ||
        !same_channel_result(result.channel_result,
                             ParserFrameChannelError::transport_failure,
                             ProtocolError::none, test_case.expected_error) ||
        !same_channel_result(later,
                             ParserFrameChannelError::transport_failure,
                             ProtocolError::none, test_case.expected_error) ||
        !channel.terminal() || io.abort_calls() != 1 ||
        io.read_calls() != reads_before || io.write_calls() != writes_before ||
        !untouched(storage, std::byte{0xa5})) {
      std::cerr << "transport failure case failed: " << test_case.name << '\n';
      return false;
    }
  }
  return true;
}

[[nodiscard]] bool test_partial_payload_failure_no_leak() {
  ohl::media::test::SyntheticValidatedMedia fixture;
  const auto nonempty_ready = canonical_header(MessageType::ready, 4, kSession,
                                                0);
  const std::array payload{std::byte{0x11}, std::byte{0x22}, std::byte{0x33},
                           std::byte{0x44}};
  ScriptedIo io;
  io.add_write();
  io.add_write();
  io.add_read(nonempty_ready);
  io.add_read(payload, 2, IsolatedWorkerError::peer_closed);
  ParserFrameChannel channel{kSession, io.operations()};
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes,
                                 std::byte{0xa5});
  const auto result = ohl::media::perform_parser_parent_handshake(
      channel, fixture.media(), {}, {}, storage, Clock::time_point::max());
  const auto reads_before = io.read_calls();
  const auto writes_before = io.write_calls();
  const auto later = channel.send(
      {.type = MessageType::shutdown,
       .payload_length = 0,
       .session_id = kSession,
       .request_id = 0},
      {}, Clock::time_point::max());
  if (result.valid() || result.proof.has_value() ||
      result.error != ParserParentHandshakeError::channel_failure ||
      result.protocol_error != ProtocolError::none ||
      !same_channel_result(result.channel_result,
                           ParserFrameChannelError::transport_failure,
                           ProtocolError::none,
                           IsolatedWorkerError::peer_closed) ||
      !same_channel_result(later,
                           ParserFrameChannelError::transport_failure,
                           ProtocolError::none,
                           IsolatedWorkerError::peer_closed) ||
      storage[0] != payload[0] || storage[1] != payload[1] ||
      !untouched(std::span<const std::byte>{storage}.subspan(2),
                 std::byte{0xa5}) ||
      !channel.terminal() || io.abort_calls() != 1 ||
      io.read_calls() != reads_before || io.write_calls() != writes_before) {
    return fail("partial payload failure leaked a view or lost buffer state");
  }
  std::fill(storage.begin(), storage.end(), std::byte{0});
  return true;
}

}  // namespace

int main() {
  const bool ok = test_canonical_handshake_and_proof() &&
                  test_preflight_zero_io_failures() &&
                  test_invalid_responses() && test_transport_failures() &&
                  test_partial_payload_failure_no_leak();
  return ok ? 0 : 1;
}
