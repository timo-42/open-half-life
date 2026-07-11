#include "ohl/media/parser_source_read_broker.hpp"

#include "synthetic_media_test_support.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <memory>
#include <span>
#include <string_view>
#include <utility>
#include <vector>

namespace {

using ohl::media::ParserResultSession;
using ohl::media::ParserResultSessionError;
using ohl::media::ParserSourceReadBroker;
using ohl::media::ParserSourceReadBrokerError;
using ohl::media::ParserSourceReadDisposition;
using ohl::media::ParserSourceReadLimits;
using ohl::media::ParserSourceReadOperations;
using ohl::media::ParserSourceReadPrepareResult;
using ohl::media::ParserSourceReadReplyTicket;
using ohl::media::test::SyntheticValidatedMedia;
using ohl::parser::FrameHeader;
using ohl::parser::FrameView;
using ohl::parser::MessageDirection;
using ohl::parser::MessageType;
using ohl::parser::PayloadWriter;
using ohl::parser::ProtocolError;
using ohl::parser::ProtocolPhase;
using ohl::parser::ProtocolStateValidator;
using ohl::parser::ProtocolStatus;
using ohl::parser::SessionState;

constexpr std::uint64_t kSession = 0x1726'3544'5362'7180ULL;
constexpr std::uint64_t kMarkerOffset =
    100U * ohl::media::test::kSyntheticSectorSize;

[[nodiscard]] std::uint64_t next_worker_epoch() noexcept {
  static std::uint64_t epoch = 0x6000'0000'0000'0001ULL;
  return epoch++;
}

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

[[nodiscard]] FrameView frame(const MessageType type,
                              const std::span<const std::byte> payload,
                              const std::uint64_t request_id) {
  return {
      .header = header(type, request_id, payload.size()),
      .payload = payload,
  };
}

[[nodiscard]] ProtocolStateValidator idle_protocol() {
  ProtocolStateValidator protocol{kSession};
  if (protocol.observe(MessageDirection::parent_to_worker,
                       header(MessageType::hello, 0,
                              ohl::parser::kHelloPayloadBytes)) !=
          ProtocolError::none ||
      protocol.observe(MessageDirection::worker_to_parent,
                       header(MessageType::ready, 0, 0)) !=
          ProtocolError::none) {
    std::abort();
  }
  return protocol;
}

[[nodiscard]] std::array<std::byte,
                         ohl::parser::kReadRequestPayloadBytes>
raw_read_request_payload(const std::uint32_t sequence,
                         const std::uint64_t offset,
                         const std::uint32_t length) {
  std::array<std::byte, ohl::parser::kReadRequestPayloadBytes> payload{};
  PayloadWriter writer{payload};
  if (!writer.write_u32(sequence) || !writer.write_u64(offset) ||
      !writer.write_u32(length)) {
    std::abort();
  }
  return payload;
}

[[nodiscard]] constexpr std::array<std::byte,
                                   ohl::parser::kCompletePayloadBytes>
complete_payload() {
  return {std::byte{0x00}, std::byte{0x00}, std::byte{0x04}, std::byte{0x00}};
}

struct Harness {
  explicit Harness(const ohl::media::ValidatedMedia& media,
                   const ParserSourceReadLimits limits = {},
                   const ParserSourceReadOperations operations = {})
      : session{idle_protocol(), next_worker_epoch()},
        broker{media, session, limits, operations} {}

  ParserResultSession session;
  ParserSourceReadBroker broker;
};

struct ReadFailureOperationsContext {
  const ohl::platform::MediaSource* expected_source{nullptr};
  std::uint64_t expected_offset{0};
  std::size_t expected_length{0};
  std::size_t verify_calls{0};
  std::size_t read_calls{0};
  bool contract_ok{true};
  bool wrote_partial_bytes{false};
};

[[nodiscard]] ohl::platform::MediaSourceError verify_stable_source(
    const ohl::platform::MediaSource& source, void* const opaque) noexcept {
  auto& context = *static_cast<ReadFailureOperationsContext*>(opaque);
  ++context.verify_calls;
  context.contract_ok =
      context.contract_ok && &source == context.expected_source;
  return source.verify_unchanged();
}

[[nodiscard]] ohl::platform::MediaSourceError fail_stable_source_read(
    const ohl::platform::MediaSource& source, const std::uint64_t offset,
    const std::span<std::byte> destination, void* const opaque) noexcept {
  auto& context = *static_cast<ReadFailureOperationsContext*>(opaque);
  ++context.read_calls;
  context.contract_ok = context.contract_ok &&
                        &source == context.expected_source &&
                        offset == context.expected_offset &&
                        destination.size() == context.expected_length;
  if (destination.size() >= 2) {
    destination[0] = std::byte{0xd1};
    destination[1] = std::byte{0xd2};
    context.wrote_partial_bytes = true;
  }
  return ohl::platform::MediaSourceError::read_failed;
}

[[nodiscard]] bool begin_enumeration(Harness& harness,
                                     const std::uint64_t request_id) {
  return harness.session
      .begin_enumeration(frame(MessageType::enumerate, {}, request_id))
      .valid();
}

[[nodiscard]] bool complete_enumeration(Harness& harness,
                                        const std::uint64_t request_id) {
  constexpr auto payload = complete_payload();
  return harness.session
      .complete_enumeration(frame(MessageType::complete, payload, request_id))
      .valid();
}

[[nodiscard]] bool shutdown(Harness& harness) {
  return harness.session
      .accept_shutdown(frame(MessageType::shutdown, {}, 0))
      .valid();
}

[[nodiscard]] ParserSourceReadPrepareResult prepare_read(
    Harness& harness, const std::uint64_t request_id,
    const std::uint32_t sequence, const std::uint64_t offset,
    const std::uint32_t length, const std::span<std::byte> scratch,
    const std::span<std::byte> reply) {
  const auto payload = raw_read_request_payload(sequence, offset, length);
  return harness.broker.prepare(
      frame(MessageType::read_request, payload, request_id), scratch, reply);
}

[[nodiscard]] ohl::parser::ReadReplyDecodeResult decode_reply(
    const ParserSourceReadPrepareResult& prepared,
    const std::uint32_t sequence, const std::uint32_t requested_length) {
  return ohl::parser::decode_read_reply_payload(
      {.header = prepared.reply_header, .payload = prepared.reply_payload},
      sequence, requested_length);
}

[[nodiscard]] bool all_bytes(const std::span<const std::byte> bytes,
                             const std::byte value) {
  return std::ranges::all_of(bytes,
                             [value](const auto byte) { return byte == value; });
}

[[nodiscard]] bool test_canonical_sequences_and_reset() {
  SyntheticValidatedMedia fixture{ohl::media::test::kSyntheticMinimumSectorCount,
                                  std::byte{0x5a}};
  Harness harness{fixture.media()};
  if (harness.broker.terminal() || !harness.broker.policy().valid() ||
      harness.broker.policy().source_size != fixture.media().fingerprint().size_bytes ||
      !begin_enumeration(harness, 10)) {
    return fail("canonical sequence fixture setup failed");
  }

  std::array<std::byte, 1> scratch{std::byte{0xa5}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
  reply.fill(std::byte{0xcc});
  const auto first = prepare_read(harness, 10, 1, kMarkerOffset, 1, scratch,
                                  reply);
  const auto decoded_first = decode_reply(first, 1, 1);
  if (!first.reply_ready() || first.disposition !=
                                  ParserSourceReadDisposition::reply_ready ||
      !first.ticket.valid() || first.status != ProtocolStatus::ok ||
      first.reply_header.type != MessageType::read_reply ||
      first.reply_header.request_id != 10 ||
      first.reply_header.payload_length != reply.size() ||
      first.reply_payload.data() != reply.data() ||
      first.reply_payload.size() != reply.size() || !decoded_first.valid() ||
      decoded_first.message.read_sequence != 1 ||
      decoded_first.message.status != ProtocolStatus::ok ||
      decoded_first.message.data.size() != 1 ||
      decoded_first.message.data[0] != std::byte{0x5a} ||
      reply[0] != std::byte{0x01} || reply[1] != std::byte{0x00} ||
      reply[2] != std::byte{0x00} || reply[3] != std::byte{0x00} ||
      reply[4] != std::byte{0x00} || reply[5] != std::byte{0x00} ||
      reply[6] != std::byte{0x5a} || scratch[0] != std::byte{0} ||
      harness.broker.requests_charged() != 1 ||
      harness.broker.reply_payload_bytes_charged() != reply.size() ||
      !harness.broker.reply_is_pending()) {
    return fail("canonical first read/reply contract failed");
  }

  std::array<std::byte, 1> blocked_scratch{std::byte{0x31}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> blocked_reply{};
  blocked_reply.fill(std::byte{0x32});
  const auto blocked = prepare_read(harness, 10, 2, kMarkerOffset, 1,
                                    blocked_scratch, blocked_reply);
  if (blocked.result.error != ParserSourceReadBrokerError::reply_pending ||
      blocked.disposition != ParserSourceReadDisposition::unavailable ||
      blocked.ticket.valid() || !blocked.reply_payload.empty() ||
      blocked_scratch[0] != std::byte{0x31} ||
      !all_bytes(blocked_reply, std::byte{0x32}) ||
      harness.broker.requests_charged() != 1 ||
      !harness.broker.reply_is_pending()) {
    return fail("pending reply advanced or touched a subsequent request");
  }
  if (!harness.broker.commit_reply_sent(first.ticket).valid() ||
      harness.broker.reply_is_pending()) {
    return fail("first canonical reply did not commit");
  }

  scratch.fill(std::byte{0xa5});
  reply.fill(std::byte{0xcc});
  const auto second = prepare_read(harness, 10, 2, kMarkerOffset, 1, scratch,
                                   reply);
  if (!second.reply_ready() || !decode_reply(second, 2, 1).valid() ||
      !harness.broker.commit_reply_sent(second.ticket).valid() ||
      harness.broker.requests_charged() != 2 ||
      harness.broker.reply_payload_bytes_charged() != 2 * reply.size() ||
      !complete_enumeration(harness, 10) ||
      !begin_enumeration(harness, 11)) {
    return fail("canonical subsequent sequence failed");
  }

  scratch.fill(std::byte{0xa5});
  reply.fill(std::byte{0xcc});
  const auto reset = prepare_read(harness, 11, 1, kMarkerOffset, 1, scratch,
                                  reply);
  if (!reset.reply_ready() || !decode_reply(reset, 1, 1).valid() ||
      !harness.broker.commit_reply_sent(reset.ticket).valid() ||
      !complete_enumeration(harness, 11) || !shutdown(harness) ||
      harness.session.protocol_state() != SessionState::closed) {
    return fail("new request did not reset the read sequence");
  }
  return true;
}

[[nodiscard]] bool test_sequence_and_request_rejections() {
  SyntheticValidatedMedia fixture;
  const auto rejected_first = [&](const std::uint32_t sequence,
                                  const std::uint64_t frame_request_id,
                                  const ProtocolError expected_protocol) {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 20)) {
      return false;
    }
    std::array<std::byte, 1> scratch{std::byte{0x41}};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    reply.fill(std::byte{0x42});
    const auto result = prepare_read(harness, frame_request_id, sequence, 0, 1,
                                     scratch, reply);
    return result.result.error ==
               ParserSourceReadBrokerError::protocol_failure &&
           result.result.protocol_error == expected_protocol &&
           result.disposition == ParserSourceReadDisposition::unavailable &&
           harness.broker.terminal() && harness.session.terminal() &&
           harness.broker.requests_charged() == 0 &&
           harness.broker.reply_payload_bytes_charged() == 0 &&
           scratch[0] == std::byte{0x41} &&
           all_bytes(reply, std::byte{0x42});
  };
  if (!rejected_first(0, 20, ProtocolError::noncanonical_value) ||
      !rejected_first(2, 20, ProtocolError::noncanonical_value) ||
      !rejected_first(1, 21, ProtocolError::wrong_request_id)) {
    return fail("zero/gap/wrong-request read was accepted or accessed source");
  }

  const auto rejects_after_first = [&](const std::uint32_t sequence) {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 22)) {
      return false;
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto first = prepare_read(harness, 22, 1, 0, 1, scratch, reply);
    if (!first.reply_ready() ||
        !harness.broker.commit_reply_sent(first.ticket).valid()) {
      return false;
    }
    scratch[0] = std::byte{0x51};
    reply.fill(std::byte{0x52});
    const auto rejected =
        prepare_read(harness, 22, sequence, 0, 1, scratch, reply);
    return rejected.result.error ==
               ParserSourceReadBrokerError::protocol_failure &&
           rejected.result.protocol_error ==
               ProtocolError::noncanonical_value &&
           harness.broker.requests_charged() == 1 &&
           harness.broker.reply_payload_bytes_charged() == reply.size() &&
           scratch[0] == std::byte{0x51} &&
           all_bytes(reply, std::byte{0x52}) && harness.broker.terminal();
  };
  return rejects_after_first(1) && rejects_after_first(3)
             ? true
             : fail("duplicate or nonconsecutive read sequence was accepted");
}

[[nodiscard]] bool test_ranges_and_maximum_length() {
  constexpr auto kSectors =
      (ohl::parser::kMaximumReadBytes /
       ohl::media::test::kSyntheticSectorSize) +
      2U;
  SyntheticValidatedMedia fixture{kSectors, std::byte{0x67}};
  Harness harness{fixture.media()};
  if (!begin_enumeration(harness, 30)) {
    return fail("maximum read fixture setup failed");
  }

  std::vector<std::byte> expected(ohl::parser::kMaximumReadBytes);
  std::vector<std::byte> scratch(ohl::parser::kMaximumReadBytes,
                                 std::byte{0xa5});
  std::vector<std::byte> reply(ohl::parser::kReadReplyPrefixBytes +
                               ohl::parser::kMaximumReadBytes);
  if (fixture.media().source()->read_exact_at(0, expected) !=
      ohl::platform::MediaSourceError::none) {
    return fail("maximum read expected-byte fixture failed");
  }
  const auto maximum = prepare_read(harness, 30, 1, 0,
                                    ohl::parser::kMaximumReadBytes, scratch,
                                    reply);
  const auto decoded =
      decode_reply(maximum, 1, ohl::parser::kMaximumReadBytes);
  if (!maximum.reply_ready() || maximum.reply_payload.size() != reply.size() ||
      maximum.reply_header.payload_length != reply.size() || !decoded.valid() ||
      !std::ranges::equal(decoded.message.data, expected) ||
      !all_bytes(scratch, std::byte{0}) ||
      !harness.broker.commit_reply_sent(maximum.ticket).valid()) {
    return fail("absolute maximum read length failed");
  }

  const auto final_offset = harness.broker.policy().source_size - 1U;
  std::array<std::byte, 1> final_expected{};
  std::array<std::byte, 1> final_scratch{std::byte{0xa5}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> final_reply{};
  if (fixture.media().source()->read_exact_at(final_offset, final_expected) !=
      ohl::platform::MediaSourceError::none) {
    return fail("exact final range expected-byte fixture failed");
  }
  const auto final = prepare_read(harness, 30, 2, final_offset, 1,
                                  final_scratch, final_reply);
  const auto final_decoded = decode_reply(final, 2, 1);
  if (!final.reply_ready() || !final_decoded.valid() ||
      final_decoded.message.data[0] != final_expected[0] ||
      !harness.broker.commit_reply_sent(final.ticket).valid() ||
      !complete_enumeration(harness, 30) || !shutdown(harness)) {
    return fail("exact source-end range failed");
  }
  return true;
}

[[nodiscard]] bool test_malformed_and_out_of_range_requests() {
  SyntheticValidatedMedia fixture;
  const auto rejected = [&](const std::span<const std::byte> payload,
                            const ProtocolError expected) {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 40)) {
      return false;
    }
    std::array<std::byte, 2> scratch{};
    scratch.fill(std::byte{0x71});
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 2> reply{};
    reply.fill(std::byte{0x72});
    const auto result = harness.broker.prepare(
        frame(MessageType::read_request, payload, 40), scratch, reply);
    return result.result.error ==
               ParserSourceReadBrokerError::protocol_failure &&
           result.result.protocol_error == expected && harness.broker.terminal() &&
           harness.broker.requests_charged() == 0 &&
           harness.broker.reply_payload_bytes_charged() == 0 &&
           all_bytes(scratch, std::byte{0x71}) &&
           all_bytes(reply, std::byte{0x72});
  };

  const std::array<std::byte, ohl::parser::kReadRequestPayloadBytes - 1> short_payload{};
  std::array<std::byte, ohl::parser::kReadRequestPayloadBytes + 1> long_payload{};
  const auto out_of_range = raw_read_request_payload(
      1, fixture.media().fingerprint().size_bytes, 1);
  const auto zero_length = raw_read_request_payload(1, 0, 0);
  const auto over_max = raw_read_request_payload(
      1, 0, ohl::parser::kMaximumReadBytes + 1U);
  if (!rejected(short_payload, ProtocolError::payload_underflow) ||
      !rejected(long_payload, ProtocolError::payload_trailing_bytes) ||
      !rejected(out_of_range, ProtocolError::noncanonical_value) ||
      !rejected(zero_length, ProtocolError::noncanonical_value) ||
      !rejected(over_max, ProtocolError::noncanonical_value)) {
    return fail("malformed/range-invalid request touched source or output");
  }
  return true;
}

[[nodiscard]] bool test_output_buffers_and_scratch_scrub() {
  SyntheticValidatedMedia fixture{ohl::media::test::kSyntheticMinimumSectorCount,
                                  std::byte{0x6b}};
  Harness harness{fixture.media(),
                  {.maximum_read_bytes = 4,
                   .maximum_requests = 8,
                   .maximum_reply_payload_bytes = 80}};
  if (!begin_enumeration(harness, 50)) {
    return fail("buffer fixture setup failed");
  }
  const auto payload = raw_read_request_payload(1, kMarkerOffset, 4);

  std::array<std::byte, 3> short_scratch{};
  short_scratch.fill(std::byte{0x11});
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 4> full_reply{};
  full_reply.fill(std::byte{0x12});
  const auto scratch_short = harness.broker.prepare(
      frame(MessageType::read_request, payload, 50), short_scratch, full_reply);
  if (scratch_short.result.error !=
          ParserSourceReadBrokerError::output_too_small ||
      harness.broker.terminal() || harness.broker.requests_charged() != 0 ||
      !all_bytes(short_scratch, std::byte{0x11}) ||
      !all_bytes(full_reply, std::byte{0x12})) {
    return fail("short scratch was not a nonmutating transient failure");
  }

  std::array<std::byte, 4> full_scratch{};
  full_scratch.fill(std::byte{0x21});
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 3> short_reply{};
  short_reply.fill(std::byte{0x22});
  const auto reply_short = harness.broker.prepare(
      frame(MessageType::read_request, payload, 50), full_scratch, short_reply);
  if (reply_short.result.error !=
          ParserSourceReadBrokerError::output_too_small ||
      harness.broker.terminal() || harness.broker.requests_charged() != 0 ||
      !all_bytes(full_scratch, std::byte{0x21}) ||
      !all_bytes(short_reply, std::byte{0x22})) {
    return fail("short reply storage was not a nonmutating transient failure");
  }

  std::array<std::byte, 20> shared{};
  shared.fill(std::byte{0x31});
  const auto overlap = harness.broker.prepare(
      frame(MessageType::read_request, payload, 50),
      std::span<std::byte>{shared}.first(4),
      std::span<std::byte>{shared}.subspan(2, 10));
  if (overlap.result.error !=
          ParserSourceReadBrokerError::overlapping_buffers ||
      harness.broker.terminal() || harness.broker.requests_charged() != 0 ||
      !all_bytes(shared, std::byte{0x31})) {
    return fail("overlapping buffers were accepted or mutated");
  }

  shared.fill(std::byte{0x41});
  const auto exact = harness.broker.prepare(
      frame(MessageType::read_request, payload, 50),
      std::span<std::byte>{shared}.first(4),
      std::span<std::byte>{shared}.subspan(4, 10));
  const auto decoded = decode_reply(exact, 1, 4);
  if (!exact.reply_ready() || exact.reply_payload.data() != shared.data() + 4 ||
      exact.reply_payload.size() != 10 || !decoded.valid() ||
      decoded.message.data[0] != std::byte{0x6b} ||
      !all_bytes(std::span<const std::byte>{shared}.first(4), std::byte{0}) ||
      !all_bytes(std::span<const std::byte>{shared}.subspan(14),
                 std::byte{0x41}) ||
      !harness.broker.commit_reply_sent(exact.ticket).valid() ||
      !complete_enumeration(harness, 50) || !shutdown(harness)) {
    return fail("exact nonoverlapping buffers or scratch scrub failed");
  }
  return true;
}

[[nodiscard]] bool test_ticket_lifecycle_and_mutation() {
  SyntheticValidatedMedia fixture{ohl::media::test::kSyntheticMinimumSectorCount,
                                  std::byte{0x79}};

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 60)) {
      return fail("stale ticket fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto first = prepare_read(harness, 60, 1, kMarkerOffset, 1, scratch,
                                    reply);
    const auto old_ticket = first.ticket;
    if (!first.reply_ready() ||
        !harness.broker.commit_reply_sent(old_ticket).valid()) {
      return fail("stale ticket fixture first commit failed");
    }
    const auto second = prepare_read(harness, 60, 2, kMarkerOffset, 1, scratch,
                                     reply);
    const auto stale = harness.broker.commit_reply_sent(old_ticket);
    if (!second.reply_ready() ||
        stale.error != ParserSourceReadBrokerError::invalid_ticket ||
        !harness.broker.terminal() || harness.broker.reply_is_pending() ||
        harness.session.result().error !=
            ParserResultSessionError::worker_failure) {
      return fail("stale reply ticket was accepted");
    }
  }

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 61)) {
      return fail("zero ticket fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto prepared = prepare_read(harness, 61, 1, kMarkerOffset, 1,
                                       scratch, reply);
    const auto invalid =
        harness.broker.commit_reply_sent(ParserSourceReadReplyTicket{});
    if (!prepared.reply_ready() ||
        invalid.error != ParserSourceReadBrokerError::invalid_ticket ||
        !harness.broker.terminal() || harness.broker.reply_is_pending()) {
      return fail("zero reply ticket was accepted");
    }
  }

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 62)) {
      return fail("abandon fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto prepared = prepare_read(harness, 62, 1, kMarkerOffset, 1,
                                       scratch, reply);
    const auto abandoned = harness.broker.abandon_reply(prepared.ticket);
    if (!prepared.reply_ready() ||
        abandoned.error != ParserSourceReadBrokerError::transport_abandoned ||
        !harness.broker.terminal() || harness.broker.reply_is_pending() ||
        harness.session.result().error !=
            ParserResultSessionError::worker_failure) {
      return fail("prepared reply abandonment did not retire the session");
    }
  }

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 63)) {
      return fail("mutated reply fixture setup failed");
    }
    std::array<std::byte, 1> scratch{std::byte{0xa5}};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto prepared = prepare_read(harness, 63, 1, kMarkerOffset, 1,
                                       scratch, reply);
    reply[0] = std::byte{0x02};
    const auto mutated = harness.broker.commit_reply_sent(prepared.ticket);
    if (!prepared.reply_ready() || scratch[0] != std::byte{0} ||
        mutated.error != ParserSourceReadBrokerError::protocol_failure ||
        mutated.protocol_error != ProtocolError::noncanonical_value ||
        mutated.session_error != ParserResultSessionError::protocol_failure ||
        !harness.broker.terminal() || harness.broker.reply_is_pending()) {
      return fail("mutated borrowed reply was committed");
    }
  }

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 64)) {
      return fail("missing pending state fixture setup failed");
    }
    const auto invalid =
        harness.broker.commit_reply_sent({.value = 1});
    if (invalid.error != ParserSourceReadBrokerError::invalid_state ||
        !harness.broker.terminal()) {
      return fail("commit without a pending reply was accepted");
    }
  }
  return true;
}

[[nodiscard]] bool test_request_and_byte_budgets() {
  SyntheticValidatedMedia fixture;
  {
    Harness harness{fixture.media(),
                    {.maximum_read_bytes = 1,
                     .maximum_requests = 1,
                     .maximum_reply_payload_bytes = 7}};
    if (!begin_enumeration(harness, 70)) {
      return fail("request budget fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, 7> reply{};
    const auto first = prepare_read(harness, 70, 1, 0, 1, scratch, reply);
    if (!first.reply_ready() ||
        !harness.broker.commit_reply_sent(first.ticket).valid()) {
      return fail("request budget first reply failed");
    }
    scratch[0] = std::byte{0x81};
    reply.fill(std::byte{0x82});
    const auto over = prepare_read(harness, 70, 2, 0, 1, scratch, reply);
    if (over.result.error !=
            ParserSourceReadBrokerError::request_budget_exceeded ||
        harness.broker.requests_charged() != 1 ||
        harness.broker.reply_payload_bytes_charged() != 7 ||
        scratch[0] != std::byte{0x81} ||
        !all_bytes(reply, std::byte{0x82}) || !harness.broker.terminal()) {
      return fail("request budget boundary was not enforced before access");
    }
  }

  {
    Harness harness{fixture.media(),
                    {.maximum_read_bytes = 1,
                     .maximum_requests = 3,
                     .maximum_reply_payload_bytes = 14}};
    if (!begin_enumeration(harness, 71)) {
      return fail("byte budget fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, 7> reply{};
    for (std::uint32_t sequence = 1; sequence <= 2; ++sequence) {
      const auto prepared =
          prepare_read(harness, 71, sequence, 0, 1, scratch, reply);
      if (!prepared.reply_ready() ||
          !harness.broker.commit_reply_sent(prepared.ticket).valid()) {
        return fail("exact byte budget was rejected early");
      }
    }
    scratch[0] = std::byte{0x91};
    reply.fill(std::byte{0x92});
    const auto over = prepare_read(harness, 71, 3, 0, 1, scratch, reply);
    if (over.result.error !=
            ParserSourceReadBrokerError::byte_budget_exceeded ||
        harness.broker.requests_charged() != 2 ||
        harness.broker.reply_payload_bytes_charged() != 14 ||
        scratch[0] != std::byte{0x91} ||
        !all_bytes(reply, std::byte{0x92}) || !harness.broker.terminal()) {
      return fail("reply-byte budget boundary was not exact");
    }
  }
  return true;
}

[[nodiscard]] bool test_stable_source_read_failure() {
  SyntheticValidatedMedia fixture;
  ReadFailureOperationsContext context{
      .expected_source = fixture.media().source().get(),
      .expected_offset = kMarkerOffset,
      .expected_length = 4,
  };
  const ParserSourceReadOperations operations{
      .verify_unchanged = verify_stable_source,
      .read_exact_at = fail_stable_source_read,
      .context = &context,
  };
  Harness harness{fixture.media(),
                  {.maximum_read_bytes = 4,
                   .maximum_requests = 2,
                   .maximum_reply_payload_bytes = 20},
                  operations};
  if (harness.broker.terminal() || !begin_enumeration(harness, 79) ||
      harness.session.catalog().has_value()) {
    return fail("stable source-read failure fixture setup failed");
  }

  std::array<std::byte, 4> scratch{};
  scratch.fill(std::byte{0xa5});
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 4> reply{};
  reply.fill(std::byte{0xcc});
  const auto prepared =
      prepare_read(harness, 79, 1, kMarkerOffset, 4, scratch, reply);
  const auto decoded = decode_reply(prepared, 1, 4);
  if (!prepared.reply_ready() || !prepared.ticket.valid() ||
      prepared.status != ProtocolStatus::source_read_failed ||
      prepared.result.source_error !=
          ohl::platform::MediaSourceError::read_failed ||
      prepared.reply_header.type != MessageType::read_reply ||
      prepared.reply_header.request_id != 79 ||
      prepared.reply_header.payload_length !=
          ohl::parser::kReadReplyPrefixBytes ||
      prepared.reply_payload.data() != reply.data() ||
      prepared.reply_payload.size() != ohl::parser::kReadReplyPrefixBytes ||
      !decoded.valid() ||
      decoded.message.status != ProtocolStatus::source_read_failed ||
      decoded.message.read_sequence != 1 || !decoded.message.data.empty() ||
      reply[0] != std::byte{0x01} || reply[1] != std::byte{0x00} ||
      reply[2] != std::byte{0x00} || reply[3] != std::byte{0x00} ||
      reply[4] != std::byte{0x07} || reply[5] != std::byte{0x00} ||
      !all_bytes(std::span<const std::byte>{reply}.subspan(
                     ohl::parser::kReadReplyPrefixBytes),
                 std::byte{0xcc}) ||
      !all_bytes(scratch, std::byte{0}) || !context.contract_ok ||
      !context.wrote_partial_bytes || context.verify_calls != 2 ||
      context.read_calls != 1 || !harness.broker.reply_is_pending() ||
      harness.broker.requests_charged() != 1 ||
      harness.broker.reply_payload_bytes_charged() != reply.size() ||
      harness.session.catalog().has_value()) {
    return fail("stable read failure reply/scratch contract failed");
  }

  const auto committed = harness.broker.commit_reply_sent(prepared.ticket);
  if (committed.error != ParserSourceReadBrokerError::source_read_failure ||
      committed.source_error !=
          ohl::platform::MediaSourceError::read_failed ||
      committed.session_error !=
          ParserResultSessionError::source_read_failure ||
      !harness.broker.terminal() || harness.broker.reply_is_pending() ||
      !harness.session.terminal() ||
      harness.session.result().error !=
          ParserResultSessionError::source_read_failure ||
      harness.session.catalog().has_value()) {
    return fail("stable read failure commit did not retire authority");
  }
  return true;
}

[[nodiscard]] bool test_source_change_non_ok_reply() {
  SyntheticValidatedMedia fixture;
  Harness harness{fixture.media()};
  if (!begin_enumeration(harness, 80) ||
      !fixture.overwrite_byte(kMarkerOffset, std::byte{0xaa}, false)) {
    return fail("source-change fixture setup failed");
  }
  std::array<std::byte, 1> scratch{std::byte{0xa5}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
  reply.fill(std::byte{0xcc});
  const auto prepared =
      prepare_read(harness, 80, 1, kMarkerOffset, 1, scratch, reply);
  const auto decoded = decode_reply(prepared, 1, 1);
  if (!prepared.reply_ready() || prepared.status != ProtocolStatus::source_changed ||
      prepared.result.source_error != ohl::platform::MediaSourceError::changed ||
      prepared.reply_payload.size() != ohl::parser::kReadReplyPrefixBytes ||
      prepared.reply_header.payload_length != ohl::parser::kReadReplyPrefixBytes ||
      !decoded.valid() || decoded.message.status != ProtocolStatus::source_changed ||
      !decoded.message.data.empty() || reply[0] != std::byte{0x01} ||
      reply[4] != std::byte{0x06} || reply[5] != std::byte{0x00} ||
      reply[6] != std::byte{0xcc} || scratch[0] != std::byte{0}) {
    return fail("source change did not produce exact non-ok wire reply");
  }
  const auto committed = harness.broker.commit_reply_sent(prepared.ticket);
  if (committed.error != ParserSourceReadBrokerError::source_changed ||
      committed.source_error != ohl::platform::MediaSourceError::changed ||
      committed.session_error != ParserResultSessionError::source_invalidated ||
      !harness.broker.terminal() || !harness.session.terminal()) {
    return fail("source-changed reply did not retire broker and session");
  }
  return true;
}

[[nodiscard]] bool test_cancellation_races() {
  SyntheticValidatedMedia fixture{ohl::media::test::kSyntheticMinimumSectorCount,
                                  std::byte{0x44}};
  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 90)) {
      return fail("pre-cancel crossing fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto prepared = prepare_read(harness, 90, 1, kMarkerOffset, 1,
                                       scratch, reply);
    if (!prepared.reply_ready() ||
        !harness.session.accept_cancel(frame(MessageType::cancel, {}, 90)).valid() ||
        !harness.broker.commit_reply_sent(prepared.ticket).valid() ||
        harness.session.protocol_state() != SessionState::cancelling ||
        !harness.session
             .accept_cancel_ack(frame(MessageType::cancel_ack, {}, 90))
             .valid() ||
        harness.session.protocol_state() != SessionState::cancelled) {
      return fail("pre-cancel crossing reply was rejected");
    }
  }

  {
    Harness harness{fixture.media()};
    if (!begin_enumeration(harness, 91)) {
      return fail("cancel-ack overtake fixture setup failed");
    }
    std::array<std::byte, 1> scratch{};
    std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
    const auto prepared = prepare_read(harness, 91, 1, kMarkerOffset, 1,
                                       scratch, reply);
    if (!prepared.reply_ready() ||
        !harness.session.accept_cancel(frame(MessageType::cancel, {}, 91)).valid() ||
        !harness.session
             .accept_cancel_ack(frame(MessageType::cancel_ack, {}, 91))
             .valid() ||
        harness.session.protocol_state() != SessionState::cancelled ||
        !harness.broker.commit_reply_sent(prepared.ticket).valid() ||
        harness.session.protocol_state() != SessionState::cancelled ||
        harness.broker.terminal()) {
      return fail("cancel acknowledgement did not permit one crossing reply");
    }
  }

  {
    SyntheticValidatedMedia changed_fixture;
    Harness harness{changed_fixture.media()};
    if (!begin_enumeration(harness, 92) ||
        !harness.session.accept_cancel(frame(MessageType::cancel, {}, 92)).valid() ||
        !changed_fixture.overwrite_byte(kMarkerOffset, std::byte{0x55}, false)) {
      return fail("post-cancel ignored fixture setup failed");
    }
    std::array<std::byte, 1> scratch{std::byte{0xb1}};
    std::array<std::byte, 1> reply{std::byte{0xb2}};
    const auto ignored = prepare_read(harness, 92, 1, kMarkerOffset, 1,
                                      scratch, reply);
    if (!ignored.valid() ||
        ignored.disposition !=
            ParserSourceReadDisposition::ignored_after_cancel ||
        ignored.reply_ready() || ignored.ticket.valid() ||
        ignored.status != ProtocolStatus::internal_failure ||
        ignored.reply_header.type != MessageType::hello ||
        ignored.reply_header.payload_length != 0 ||
        ignored.reply_header.session_id != 0 ||
        ignored.reply_header.request_id != 0 ||
        !ignored.reply_payload.empty() || scratch[0] != std::byte{0xb1} ||
        reply[0] != std::byte{0xb2} ||
        harness.broker.requests_charged() != 0 ||
        harness.broker.reply_payload_bytes_charged() != 0 ||
        harness.broker.reply_is_pending() || harness.broker.terminal() ||
        !harness.session
             .accept_cancel_ack(frame(MessageType::cancel_ack, {}, 92))
             .valid()) {
      return fail("post-cancel read was serviced or touched caller storage");
    }
  }
  return true;
}

[[nodiscard]] bool test_capability_lifetime_and_destructor_retirement() {
  auto fixture = std::make_unique<SyntheticValidatedMedia>(
      ohl::media::test::kSyntheticMinimumSectorCount, std::byte{0x4d});
  ParserResultSession session{idle_protocol(), next_worker_epoch()};
  auto broker = std::make_unique<ParserSourceReadBroker>(fixture->media(),
                                                         session);
  fixture.reset();
  if (broker->terminal() ||
      !session.begin_enumeration(frame(MessageType::enumerate, {}, 100)).valid()) {
    return fail("retained capability fixture setup failed");
  }
  std::array<std::byte, 1> scratch{std::byte{0xa5}};
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> reply{};
  const auto payload = raw_read_request_payload(1, kMarkerOffset, 1);
  const auto prepared = broker->prepare(
      frame(MessageType::read_request, payload, 100), scratch, reply);
  if (!prepared.reply_ready() || prepared.reply_payload.data() != reply.data() ||
      decode_reply(prepared, 1, 1).message.data[0] != std::byte{0x4d} ||
      scratch[0] != std::byte{0}) {
    return fail("broker did not retain the pinned source capability");
  }
  broker.reset();
  if (!session.terminal() ||
      session.result().error != ParserResultSessionError::worker_failure) {
    return fail("broker destruction did not retire its active session");
  }
  std::fill(reply.begin(), reply.end(), std::byte{0});

  SyntheticValidatedMedia invalid_fixture;
  auto retained = std::move(invalid_fixture.media());
  ParserResultSession invalid_session{idle_protocol(), next_worker_epoch()};
  ParserSourceReadBroker invalid{invalid_fixture.media(), invalid_session};
  if (!retained.valid() || !invalid.terminal() ||
      invalid.result().error !=
          ParserSourceReadBrokerError::invalid_configuration ||
      invalid_session.result().error != ParserResultSessionError::worker_failure) {
    return fail("moved-from media capability was accepted by broker");
  }
  return true;
}

[[nodiscard]] bool test_invalid_limits() {
  SyntheticValidatedMedia fixture;
  const std::array invalid_limits{
      ParserSourceReadLimits{.maximum_read_bytes = 0,
                             .maximum_requests = 1,
                             .maximum_reply_payload_bytes = 7},
      ParserSourceReadLimits{.maximum_read_bytes = 1,
                             .maximum_requests = 0,
                             .maximum_reply_payload_bytes = 7},
      ParserSourceReadLimits{.maximum_read_bytes = 2,
                             .maximum_requests = 1,
                             .maximum_reply_payload_bytes = 7},
  };
  for (const auto limits : invalid_limits) {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    ParserSourceReadBroker broker{fixture.media(), session, limits};
    if (!broker.terminal() ||
        broker.result().error !=
            ParserSourceReadBrokerError::invalid_configuration ||
        session.result().error != ParserResultSessionError::worker_failure) {
      return fail("invalid broker limits were accepted");
    }
  }

  ReadFailureOperationsContext context{
      .expected_source = fixture.media().source().get(),
  };
  const std::array partial_operations{
      ParserSourceReadOperations{
          .verify_unchanged = verify_stable_source,
          .read_exact_at = nullptr,
          .context = &context,
      },
      ParserSourceReadOperations{
          .verify_unchanged = nullptr,
          .read_exact_at = fail_stable_source_read,
          .context = &context,
      },
      ParserSourceReadOperations{
          .verify_unchanged = nullptr,
          .read_exact_at = nullptr,
          .context = &context,
      },
  };
  for (const auto operations : partial_operations) {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    ParserSourceReadBroker broker{fixture.media(), session, {}, operations};
    if (!broker.terminal() ||
        broker.result().error !=
            ParserSourceReadBrokerError::invalid_configuration ||
        session.result().error != ParserResultSessionError::worker_failure ||
        context.verify_calls != 0 || context.read_calls != 0) {
      return fail("partial source operation table was accepted or invoked");
    }
  }
  return true;
}

}  // namespace

int main() {
  return test_canonical_sequences_and_reset() &&
                 test_sequence_and_request_rejections() &&
                 test_ranges_and_maximum_length() &&
                 test_malformed_and_out_of_range_requests() &&
                 test_output_buffers_and_scratch_scrub() &&
                 test_ticket_lifecycle_and_mutation() &&
                 test_request_and_byte_budgets() &&
                 test_stable_source_read_failure() &&
                 test_source_change_non_ok_reply() &&
                 test_cancellation_races() &&
                 test_capability_lifetime_and_destructor_retirement() &&
                 test_invalid_limits()
             ? 0
             : 1;
}
