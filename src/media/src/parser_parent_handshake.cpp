#include "ohl/media/parser_parent_handshake.hpp"

#include "ohl/parser/protocol_messages.hpp"

#include <array>
#include <cstddef>
#include <optional>
#include <utility>

namespace ohl::media {
namespace {

[[nodiscard]] ParserParentHandshakeResult channel_failure(
    ParserFrameChannel& channel,
    const ParserFrameChannelResult result) noexcept {
  channel.abort();
  return {
      .error = ParserParentHandshakeError::channel_failure,
      .protocol_error = result.protocol_error,
      .channel_result = result,
      .proof = std::nullopt,
  };
}

[[nodiscard]] ParserParentHandshakeResult protocol_failure(
    ParserFrameChannel& channel,
    const parser::ProtocolError error) noexcept {
  channel.abort();
  return {
      .error = ParserParentHandshakeError::protocol_failure,
      .protocol_error = error,
      .channel_result = channel.result(),
      .proof = std::nullopt,
  };
}

[[nodiscard]] ParserParentHandshakeResult internal_failure(
    ParserFrameChannel& channel) noexcept {
  channel.abort();
  return {
      .error = ParserParentHandshakeError::internal_failure,
      .protocol_error = parser::ProtocolError::none,
      .channel_result = channel.result(),
      .proof = std::nullopt,
  };
}

}  // namespace

ParserParentHandshakeProof::ParserParentHandshakeProof(
    parser::ProtocolStateValidator&& protocol,
    const ParserSourceReadLimits source_read_limits,
    const parser::SourceReadPolicy source_read_policy) noexcept
    : protocol_{std::move(protocol)},
      source_read_limits_{source_read_limits},
      source_read_policy_{source_read_policy} {}

ParserParentHandshakeProof::ParserParentHandshakeProof(
    ParserParentHandshakeProof&& other) noexcept
    : protocol_{std::move(other.protocol_)},
      source_read_limits_{other.source_read_limits_},
      source_read_policy_{other.source_read_policy_},
      valid_{std::exchange(other.valid_, false)} {}

ParserParentHandshakeProof& ParserParentHandshakeProof::operator=(
    ParserParentHandshakeProof&& other) noexcept {
  if (this != &other) {
    protocol_ = std::move(other.protocol_);
    source_read_limits_ = other.source_read_limits_;
    source_read_policy_ = other.source_read_policy_;
    valid_ = std::exchange(other.valid_, false);
  }
  return *this;
}

std::optional<parser::ProtocolStateValidator>
ParserParentHandshakeProof::take_protocol() noexcept {
  if (!valid_) {
    return std::nullopt;
  }
  valid_ = false;
  return std::optional<parser::ProtocolStateValidator>{
      std::in_place, std::move(protocol_)};
}

ParserParentHandshakeResult perform_parser_parent_handshake(
    ParserFrameChannel& channel, const ValidatedMedia& media,
    const ParserSourceReadLimits source_read_limits,
    const parser::ProtocolBudgets protocol_budgets,
    const std::span<std::byte> receive_storage,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  if (channel.terminal()) {
    return {
        .error = ParserParentHandshakeError::channel_failure,
        .protocol_error = channel.result().protocol_error,
        .channel_result = channel.result(),
        .proof = std::nullopt,
    };
  }
  if (!media.valid() || media.source() == nullptr ||
      media.source()->size() != media.fingerprint().size_bytes ||
      !source_read_limits.valid() || !protocol_budgets.valid() ||
      protocol_budgets.maximum_messages < 2 ||
      protocol_budgets.maximum_payload_bytes < parser::kHelloPayloadBytes) {
    return {
        .error = ParserParentHandshakeError::invalid_configuration,
        .protocol_error = parser::ProtocolError::none,
        .channel_result = {},
        .proof = std::nullopt,
    };
  }
  if (receive_storage.size() < parser::kMaximumFramePayloadBytes ||
      receive_storage.data() == nullptr) {
    return {
        .error = ParserParentHandshakeError::output_too_small,
        .protocol_error = parser::ProtocolError::none,
        .channel_result = {},
        .proof = std::nullopt,
    };
  }

  const parser::SourceReadPolicy source_read_policy{
      .source_size = media.fingerprint().size_bytes,
      .maximum_read_bytes = source_read_limits.maximum_read_bytes,
  };
  if (!source_read_policy.valid()) {
    return {
        .error = ParserParentHandshakeError::invalid_configuration,
        .protocol_error = parser::ProtocolError::none,
        .channel_result = {},
        .proof = std::nullopt,
    };
  }

  parser::ProtocolStateValidator protocol{channel.session_id(),
                                          protocol_budgets};
  if (protocol.error() != parser::ProtocolError::none) {
    return {
        .error = ParserParentHandshakeError::invalid_configuration,
        .protocol_error = protocol.error(),
        .channel_result = {},
        .proof = std::nullopt,
    };
  }

  std::array<std::byte, parser::kHelloPayloadBytes> hello_payload{};
  const auto encoded = parser::encode_hello_payload(
      {
          .source_size = source_read_policy.source_size,
          .maximum_read_bytes = source_read_policy.maximum_read_bytes,
      },
      hello_payload);
  if (!encoded.valid() || encoded.bytes_written != hello_payload.size()) {
    return {
        .error = ParserParentHandshakeError::protocol_failure,
        .protocol_error = encoded.valid()
                              ? parser::ProtocolError::noncanonical_value
                              : encoded.error,
        .channel_result = {},
        .proof = std::nullopt,
    };
  }

  const parser::FrameHeader hello_header{
      .type = parser::MessageType::hello,
      .payload_length = static_cast<std::uint32_t>(hello_payload.size()),
      .session_id = channel.session_id(),
      .request_id = 0,
  };
  const auto sent =
      channel.send(hello_header, hello_payload, deadline, cancellation);
  if (!sent.valid()) {
    return channel_failure(channel, sent);
  }

  auto protocol_error = protocol.observe(
      parser::MessageDirection::parent_to_worker, hello_header);
  if (protocol_error != parser::ProtocolError::none) {
    return protocol_failure(channel, protocol_error);
  }

  const auto received =
      channel.receive(receive_storage, deadline, cancellation);
  if (!received.valid()) {
    return channel_failure(channel, received.result);
  }
  const auto ready = parser::decode_ready_payload(received.frame);
  if (!ready.valid()) {
    return protocol_failure(channel, ready.error);
  }

  protocol_error = protocol.observe(
      parser::MessageDirection::worker_to_parent, received.frame.header);
  if (protocol_error != parser::ProtocolError::none) {
    return protocol_failure(channel, protocol_error);
  }
  if (protocol.state() != parser::SessionState::idle ||
      protocol.message_count() != 2 ||
      protocol.payload_bytes() != parser::kHelloPayloadBytes) {
    return internal_failure(channel);
  }

  ParserParentHandshakeResult result;
  result.proof = ParserParentHandshakeProof{
      std::move(protocol), source_read_limits, source_read_policy};
  return result;
}

}  // namespace ohl::media
