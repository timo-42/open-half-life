#include "ohl/media/parser_source_read_broker.hpp"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <span>

namespace ohl::media {
namespace {

struct SourceOutcome {
  parser::ProtocolStatus status{parser::ProtocolStatus::ok};
  platform::MediaSourceError error{platform::MediaSourceError::none};
};

[[nodiscard]] platform::MediaSourceError verify_native_source(
    const platform::MediaSource& source, void*) noexcept {
  return source.verify_unchanged();
}

[[nodiscard]] platform::MediaSourceError read_native_source(
    const platform::MediaSource& source, const std::uint64_t offset,
    const std::span<std::byte> destination, void*) noexcept {
  return source.read_exact_at(offset, destination);
}

[[nodiscard]] ParserSourceReadOperations normalize_operations(
    const ParserSourceReadOperations operations) noexcept {
  if (operations.verify_unchanged == nullptr &&
      operations.read_exact_at == nullptr && operations.context == nullptr) {
    return {
        .verify_unchanged = verify_native_source,
        .read_exact_at = read_native_source,
        .context = nullptr,
    };
  }
  return operations;
}

[[nodiscard]] ParserSourceReadPrepareResult unavailable(
    const ParserSourceReadBrokerResult result) noexcept {
  return {
      .result = result,
      .disposition = ParserSourceReadDisposition::unavailable,
      .ticket = {},
      .status = parser::ProtocolStatus::internal_failure,
      .reply_header = {},
      .reply_payload = {},
  };
}

[[nodiscard]] SourceOutcome boundary_outcome(
    const platform::MediaSourceError error) noexcept {
  if (error == platform::MediaSourceError::none) {
    return {};
  }
  return {
      .status = error == platform::MediaSourceError::changed
                    ? parser::ProtocolStatus::source_changed
                    : parser::ProtocolStatus::source_read_failed,
      .error = error,
  };
}

[[nodiscard]] SourceOutcome read_outcome(
    const platform::MediaSourceError error) noexcept {
  if (error == platform::MediaSourceError::none) {
    return {};
  }
  const auto changed = error == platform::MediaSourceError::changed ||
                       error == platform::MediaSourceError::unexpected_eof ||
                       error == platform::MediaSourceError::out_of_range;
  return {
      .status = changed ? parser::ProtocolStatus::source_changed
                        : parser::ProtocolStatus::source_read_failed,
      .error = error,
  };
}

[[nodiscard]] bool overlaps(const std::span<std::byte> first,
                            const std::span<std::byte> second) noexcept {
  if (first.empty() || second.empty()) {
    return false;
  }
  const auto first_begin =
      reinterpret_cast<std::uintptr_t>(first.data());
  const auto second_begin =
      reinterpret_cast<std::uintptr_t>(second.data());
  return first_begin <= second_begin
             ? second_begin - first_begin < first.size()
             : first_begin - second_begin < second.size();
}

}  // namespace

bool ParserSourceReadLimits::valid() const noexcept {
  return maximum_read_bytes != 0 &&
         maximum_read_bytes <= parser::kMaximumReadBytes &&
         maximum_requests != 0 &&
         maximum_requests <= parser::kMaximumProtocolMessages / 2U &&
         maximum_reply_payload_bytes >=
             parser::kReadReplyPrefixBytes + maximum_read_bytes &&
         maximum_reply_payload_bytes <=
             parser::kMaximumCumulativePayloadBytes;
}

ParserSourceReadBroker::ParserSourceReadBroker(
    const ValidatedMedia& media, ParserResultSession& session,
    const ParserSourceReadLimits limits,
    const ParserSourceReadOperations operations) noexcept
    : session_{session},
      limits_{limits},
      operations_{normalize_operations(operations)} {
  if (media.valid()) {
    source_ = media.source();
    policy_ = {
        .source_size = media.fingerprint().size_bytes,
        .maximum_read_bytes = limits_.maximum_read_bytes,
    };
  }
  if (source_ == nullptr || source_->size() != policy_.source_size ||
      !policy_.valid() || !limits_.valid() ||
      operations_.verify_unchanged == nullptr ||
      operations_.read_exact_at == nullptr || session_.terminal() ||
      session_.protocol_state() != parser::SessionState::idle) {
    failure_.error = ParserSourceReadBrokerError::invalid_configuration;
    failure_.session_error = session_.result().error;
    terminal_ = true;
    if (!session_.terminal()) {
      session_.worker_failed();
    }
  }
}

ParserSourceReadBroker::~ParserSourceReadBroker() {
  const auto state = session_.protocol_state();
  if (!session_.terminal() && state != parser::SessionState::closed &&
      state != parser::SessionState::cancelled) {
    session_.worker_failed();
  }
}

ParserSourceReadPrepareResult ParserSourceReadBroker::prepare(
    const parser::FrameView& read_request,
    const std::span<std::byte> read_scratch,
    const std::span<std::byte> reply_payload_storage) noexcept {
  if (terminal_) {
    return unavailable(failure_);
  }
  if (pending_) {
    return unavailable(
        transient(ParserSourceReadBrokerError::reply_pending));
  }

  const auto same_request =
      have_committed_request_ &&
      read_request.header.request_id == committed_request_id_;
  if (same_request && sequence_exhausted_) {
    return unavailable(
        fail(ParserSourceReadBrokerError::sequence_exhausted));
  }
  const auto expected_sequence = same_request ? next_sequence_ : 1U;
  const auto decoded = parser::decode_read_request_payload(
      read_request, policy_, expected_sequence);
  if (!decoded.valid()) {
    const auto session_result = session_.accept_read_request(
        read_request, policy_, expected_sequence);
    return unavailable(fail(ParserSourceReadBrokerError::protocol_failure,
                            session_result.result.protocol_error,
                            session_result.result.error));
  }

  const auto cancellation_pending =
      session_.protocol_state() == parser::SessionState::cancelling;
  const auto reply_size = parser::kReadReplyPrefixBytes +
                          static_cast<std::size_t>(decoded.message.length);
  if (!cancellation_pending) {
    const auto scratch = read_scratch.first(
        std::min(read_scratch.size(),
                 static_cast<std::size_t>(decoded.message.length)));
    const auto reply = reply_payload_storage.first(
        std::min(reply_payload_storage.size(), reply_size));
    if (read_scratch.size() < decoded.message.length ||
        reply_payload_storage.size() < reply_size) {
      return unavailable(
          transient(ParserSourceReadBrokerError::output_too_small));
    }
    if (overlaps(scratch, reply)) {
      return unavailable(
          transient(ParserSourceReadBrokerError::overlapping_buffers));
    }
    if (requests_charged_ >= limits_.maximum_requests) {
      return unavailable(
          fail(ParserSourceReadBrokerError::request_budget_exceeded));
    }
    if (reply_size > limits_.maximum_reply_payload_bytes -
                         reply_payload_bytes_charged_) {
      return unavailable(
          fail(ParserSourceReadBrokerError::byte_budget_exceeded));
    }
    if (ticket_counter_ == std::numeric_limits<std::uint64_t>::max()) {
      return unavailable(
          fail(ParserSourceReadBrokerError::ticket_exhausted));
    }
  }

  const auto accepted = session_.accept_read_request(
      read_request, policy_, expected_sequence);
  if (!accepted.valid()) {
    return unavailable(fail(ParserSourceReadBrokerError::protocol_failure,
                            accepted.result.protocol_error,
                            accepted.result.error));
  }
  if (accepted.disposition ==
      ParserReadRequestDisposition::ignored_after_cancel) {
    if (!cancellation_pending || accepted.serviceable()) {
      return unavailable(
          fail(ParserSourceReadBrokerError::internal_failure));
    }
    return {
        .result = {},
        .disposition = ParserSourceReadDisposition::ignored_after_cancel,
        .ticket = {},
        .status = parser::ProtocolStatus::internal_failure,
        .reply_header = {},
        .reply_payload = {},
    };
  }
  if (cancellation_pending || !accepted.serviceable() ||
      accepted.message.read_sequence != decoded.message.read_sequence ||
      accepted.message.offset != decoded.message.offset ||
      accepted.message.length != decoded.message.length) {
    return unavailable(fail(ParserSourceReadBrokerError::internal_failure));
  }

  ++requests_charged_;
  reply_payload_bytes_charged_ += reply_size;

  auto source_outcome = boundary_outcome(
      operations_.verify_unchanged(*source_, operations_.context));
  const auto scratch = read_scratch.first(decoded.message.length);
  if (source_outcome.status == parser::ProtocolStatus::ok) {
    const auto read_error = operations_.read_exact_at(
        *source_, decoded.message.offset, scratch, operations_.context);
    if (read_error != platform::MediaSourceError::none) {
      const auto verification = boundary_outcome(
          operations_.verify_unchanged(*source_, operations_.context));
      source_outcome = verification.status == parser::ProtocolStatus::ok
                           ? read_outcome(read_error)
                           : verification;
    } else {
      source_outcome = boundary_outcome(
          operations_.verify_unchanged(*source_, operations_.context));
    }
  }

  const auto reply_data =
      source_outcome.status == parser::ProtocolStatus::ok
          ? std::span<const std::byte>{scratch}
          : std::span<const std::byte>{};
  const auto encoded = parser::encode_read_reply_payload(
      {.read_sequence = expected_sequence,
       .status = source_outcome.status,
       .data = reply_data},
      expected_sequence, decoded.message.length, reply_payload_storage);
  std::fill(scratch.begin(), scratch.end(), std::byte{0});
  if (!encoded.valid()) {
    return unavailable(fail(ParserSourceReadBrokerError::internal_failure,
                            encoded.error));
  }

  ++ticket_counter_;
  pending_ = true;
  pending_ticket_ = {.value = ticket_counter_};
  pending_request_id_ = read_request.header.request_id;
  pending_sequence_ = expected_sequence;
  pending_requested_length_ = decoded.message.length;
  pending_status_ = source_outcome.status;
  pending_source_error_ = source_outcome.error;
  pending_header_ = {
      .type = parser::MessageType::read_reply,
      .payload_length = static_cast<std::uint32_t>(encoded.bytes_written),
      .session_id = read_request.header.session_id,
      .request_id = read_request.header.request_id,
  };
  pending_payload_ = reply_payload_storage.first(encoded.bytes_written);
  return {
      .result = {
          .error = ParserSourceReadBrokerError::none,
          .protocol_error = parser::ProtocolError::none,
          .session_error = ParserResultSessionError::none,
          .source_error = source_outcome.error,
      },
      .disposition = ParserSourceReadDisposition::reply_ready,
      .ticket = pending_ticket_,
      .status = pending_status_,
      .reply_header = pending_header_,
      .reply_payload = pending_payload_,
  };
}

ParserSourceReadBrokerResult ParserSourceReadBroker::commit_reply_sent(
    const ParserSourceReadReplyTicket ticket) noexcept {
  if (terminal_) {
    return failure_;
  }
  if (!pending_) {
    return fail(ParserSourceReadBrokerError::invalid_state);
  }
  if (!ticket.valid() || ticket != pending_ticket_) {
    return fail(ParserSourceReadBrokerError::invalid_ticket);
  }

  const auto frame = parser::FrameView{
      .header = pending_header_,
      .payload = pending_payload_,
  };
  const auto status = pending_status_;
  const auto request_id = pending_request_id_;
  const auto sequence = pending_sequence_;
  const auto requested_length = pending_requested_length_;
  const auto source_error = pending_source_error_;
  const auto accepted =
      session_.accept_read_reply(frame, sequence, requested_length);
  clear_pending();

  if (status == parser::ProtocolStatus::ok) {
    if (!accepted.valid()) {
      return fail(ParserSourceReadBrokerError::protocol_failure,
                  accepted.protocol_error, accepted.error);
    }
    have_committed_request_ = true;
    committed_request_id_ = request_id;
    if (sequence == std::numeric_limits<std::uint32_t>::max()) {
      sequence_exhausted_ = true;
    } else {
      next_sequence_ = sequence + 1U;
      sequence_exhausted_ = false;
    }
    return {};
  }
  if (status == parser::ProtocolStatus::source_changed &&
      accepted.error == ParserResultSessionError::source_invalidated) {
    return fail(ParserSourceReadBrokerError::source_changed,
                accepted.protocol_error, accepted.error, source_error);
  }
  if (status == parser::ProtocolStatus::source_read_failed &&
      accepted.error == ParserResultSessionError::source_read_failure) {
    return fail(ParserSourceReadBrokerError::source_read_failure,
                accepted.protocol_error, accepted.error, source_error);
  }
  return fail(ParserSourceReadBrokerError::protocol_failure,
              accepted.protocol_error, accepted.error);
}

ParserSourceReadBrokerResult ParserSourceReadBroker::abandon_reply(
    const ParserSourceReadReplyTicket ticket) noexcept {
  if (terminal_) {
    return failure_;
  }
  if (!pending_) {
    return fail(ParserSourceReadBrokerError::invalid_state);
  }
  if (!ticket.valid() || ticket != pending_ticket_) {
    return fail(ParserSourceReadBrokerError::invalid_ticket);
  }
  clear_pending();
  return fail(ParserSourceReadBrokerError::transport_abandoned);
}

ParserSourceReadBrokerResult ParserSourceReadBroker::result() const noexcept {
  return terminal_ ? failure_ : ParserSourceReadBrokerResult{};
}

ParserSourceReadBrokerResult ParserSourceReadBroker::fail(
    const ParserSourceReadBrokerError error,
    const parser::ProtocolError protocol_error,
    const ParserResultSessionError session_error,
    const platform::MediaSourceError source_error) noexcept {
  if (!terminal_) {
    failure_ = {
        .error = error,
        .protocol_error = protocol_error,
        .session_error = session_error,
        .source_error = source_error,
    };
    terminal_ = true;
    clear_pending();
    if (!session_.terminal()) {
      session_.worker_failed();
    }
  }
  return failure_;
}

ParserSourceReadBrokerResult ParserSourceReadBroker::transient(
    const ParserSourceReadBrokerError error) const noexcept {
  return {.error = error};
}

void ParserSourceReadBroker::clear_pending() noexcept {
  pending_ = false;
  pending_ticket_ = {};
  pending_request_id_ = 0;
  pending_sequence_ = 0;
  pending_requested_length_ = 0;
  pending_status_ = parser::ProtocolStatus::internal_failure;
  pending_source_error_ = platform::MediaSourceError::none;
  pending_header_ = {};
  pending_payload_ = {};
}

}  // namespace ohl::media
