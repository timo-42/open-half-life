#include "ohl/media/parser_parent_session.hpp"

#include "ohl/parser/protocol_messages.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <new>
#include <span>
#include <utility>

namespace ohl::media {
namespace {

[[nodiscard]] bool import_limits_valid(
    const PayloadImportLimits& limits) noexcept {
  return limits.maximum_entries != 0 &&
         limits.maximum_entries <= parser::kMaximumEnumeratedEntries &&
         limits.maximum_path_bytes != 0 &&
         limits.maximum_path_bytes <= parser::kMaximumEnumeratedPathBytes &&
         limits.maximum_entry_bytes != 0 &&
         limits.maximum_entry_bytes <= parser::kMaximumEnumeratedEntryBytes &&
         limits.maximum_total_bytes != 0 &&
         limits.maximum_total_bytes <= parser::kMaximumEnumeratedTotalBytes &&
         limits.maximum_entry_bytes <= limits.maximum_total_bytes;
}

[[nodiscard]] bool overlaps(const std::span<std::byte> first,
                            const std::span<std::byte> second) noexcept {
  if (first.empty() || second.empty()) {
    return false;
  }
  const auto first_begin = reinterpret_cast<std::uintptr_t>(first.data());
  const auto second_begin = reinterpret_cast<std::uintptr_t>(second.data());
  return first_begin <= second_begin
             ? second_begin - first_begin < first.size()
             : first_begin - second_begin < second.size();
}

[[nodiscard]] parser::FrameView frame(
    const parser::FrameHeader& header,
    const std::span<const std::byte> payload) noexcept {
  return {
      .error = parser::ProtocolError::none,
      .header = header,
      .payload = payload,
  };
}

[[nodiscard]] ParserParentSessionResult simple_error(
    const ParserParentSessionError error,
    const parser::ProtocolError protocol_error =
        parser::ProtocolError::none) noexcept {
  return {
      .error = error,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = 0,
      .protocol_error = protocol_error,
      .channel_result = {},
      .session_result = {},
      .source_result = {},
  };
}

}  // namespace

ParserParentSession::ParserParentSession(
    ParserFrameChannel& channel, const ValidatedMedia& media,
    parser::ProtocolStateValidator&& protocol,
    const std::uint64_t worker_epoch,
    const PayloadImportLimits import_limits,
    const ParserSourceReadLimits source_read_limits) noexcept
    : channel_{channel},
      result_session_{std::move(protocol), worker_epoch, import_limits},
      source_reads_{media, result_session_, source_read_limits},
      source_read_limits_{source_read_limits} {}

ParserParentSession::~ParserParentSession() {
  bool abort_channel = false;
  {
    const std::scoped_lock lock{transaction_mutex_};
    if (state_ != ParserParentSessionState::closed &&
        state_ != ParserParentSessionState::terminal) {
      result_session_.worker_failed();
      retain_failure_locked({
          .error = ParserParentSessionError::worker_failure,
          .disposition = ParserParentSessionDisposition::unavailable,
          .request_id = current_request_id_locked(),
          .protocol_error = parser::ProtocolError::none,
          .channel_result = {},
          .session_result = result_session_.result(),
          .source_result = source_reads_.result(),
      });
      abort_channel = true;
    }
  }
  if (abort_channel) {
    channel_.abort();
  }
}

ParserParentSessionResult ParserParentSession::begin_enumeration(
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  std::unique_lock lock{transaction_mutex_};
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::none) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  if (state_ != ParserParentSessionState::idle) {
    return transient_locked(ParserParentSessionError::invalid_state);
  }
  if (receiver_active_) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }

  std::uint64_t request_id = 0;
  if (!allocate_request_id_locked(request_id)) {
    return transient_locked(ParserParentSessionError::request_id_exhausted);
  }
  const parser::FrameHeader header{
      .type = parser::MessageType::enumerate,
      .payload_length = 0,
      .session_id = channel_.session_id(),
      .request_id = request_id,
  };
  begin_outbound_locked(OutboundTransaction::begin_enumeration, request_id);
  const auto accepted = result_session_.begin_enumeration(frame(header, {}));
  if (!accepted.valid()) {
    return abort_after_unlock(lock, fail_result_locked(accepted));
  }

  lock.unlock();
  const auto sent = channel_.send(header, {}, deadline, cancellation);
  lock.lock();
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::begin_enumeration) {
    return abort_after_unlock(
        lock, fail_protocol_locked(parser::ProtocolError::unexpected_message));
  }
  if (!sent.valid()) {
    return abort_after_unlock(lock, fail_channel_locked(sent));
  }
  state_ = ParserParentSessionState::enumerating;
  active_operation_ = ActiveOperation::enumeration;
  active_request_id_ = request_id;
  end_outbound_locked();
  return success_locked(ParserParentSessionDisposition::request_sent,
                        request_id);
}

ParserParentSessionResult ParserParentSession::begin_stream(
    const ParserResultCatalogGeneration generation,
    const std::uint64_t source_token, PayloadByteSink& sink,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  std::array<std::byte, parser::kStreamEntryPayloadBytes> payload{};
  const auto encoded =
      parser::encode_stream_entry_payload({.source_token = source_token},
                                          payload);
  if (!encoded.valid() || encoded.bytes_written != payload.size()) {
    return simple_error(ParserParentSessionError::internal_failure,
                        encoded.error);
  }

  std::unique_lock lock{transaction_mutex_};
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::none) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  if (state_ != ParserParentSessionState::idle) {
    return transient_locked(ParserParentSessionError::invalid_state);
  }
  if (receiver_active_) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  std::uint64_t request_id = 0;
  if (!allocate_request_id_locked(request_id)) {
    return transient_locked(ParserParentSessionError::request_id_exhausted);
  }
  const parser::FrameHeader header{
      .type = parser::MessageType::stream_entry,
      .payload_length = static_cast<std::uint32_t>(payload.size()),
      .session_id = channel_.session_id(),
      .request_id = request_id,
  };
  begin_outbound_locked(OutboundTransaction::begin_stream, request_id);
  const auto accepted = result_session_.begin_stream_entry(
      frame(header, payload), generation);
  if (!accepted.valid()) {
    return abort_after_unlock(lock, fail_result_locked(accepted));
  }

  lock.unlock();
  const auto sent = channel_.send(header, payload, deadline, cancellation);
  lock.lock();
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::begin_stream) {
    return abort_after_unlock(
        lock, fail_protocol_locked(parser::ProtocolError::unexpected_message));
  }
  if (!sent.valid()) {
    return abort_after_unlock(lock, fail_channel_locked(sent));
  }
  state_ = ParserParentSessionState::streaming;
  active_operation_ = ActiveOperation::stream;
  active_request_id_ = request_id;
  stream_sink_ = &sink;
  end_outbound_locked();
  return success_locked(ParserParentSessionDisposition::request_sent,
                        request_id);
}

ParserParentSessionResult ParserParentSession::receive_one(
    const std::span<std::byte> receive_storage,
    const std::span<std::byte> read_scratch,
    const std::span<std::byte> reply_storage,
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  const auto required_reply = parser::kReadReplyPrefixBytes +
                              static_cast<std::size_t>(
                                  source_read_limits_.maximum_read_bytes);
  {
    const std::scoped_lock lock{transaction_mutex_};
    if (state_ == ParserParentSessionState::terminal) {
      return failure_;
    }
    if (outbound_transaction_ != OutboundTransaction::none) {
      return transient_locked(ParserParentSessionError::concurrent_operation);
    }
    if (state_ != ParserParentSessionState::enumerating &&
        state_ != ParserParentSessionState::streaming &&
        state_ != ParserParentSessionState::cancelling) {
      return transient_locked(ParserParentSessionError::invalid_state);
    }
    if (receiver_active_) {
      return transient_locked(ParserParentSessionError::concurrent_operation);
    }
    if (receive_storage.size() < parser::kMaximumFramePayloadBytes ||
        read_scratch.size() < source_read_limits_.maximum_read_bytes ||
        reply_storage.size() < required_reply ||
        receive_storage.data() == nullptr ||
        read_scratch.data() == nullptr || reply_storage.data() == nullptr) {
      return simple_error(ParserParentSessionError::output_too_small);
    }
    if (overlaps(receive_storage, read_scratch) ||
        overlaps(receive_storage, reply_storage) ||
        overlaps(read_scratch, reply_storage)) {
      return simple_error(ParserParentSessionError::overlapping_buffers);
    }
    receiver_active_ = true;
  }

  const auto received =
      channel_.receive(receive_storage, deadline, cancellation);

  std::unique_lock lock{transaction_mutex_};
  transaction_condition_.wait(lock, [this] {
    return outbound_transaction_ == OutboundTransaction::none ||
           state_ == ParserParentSessionState::terminal;
  });
  const auto finish_receive = [this](
                                  ParserParentSessionResult result) noexcept {
    receiver_active_ = false;
    return result;
  };
  const auto abort_receive =
      [this, &lock](ParserParentSessionResult result) noexcept {
        receiver_active_ = false;
        return abort_after_unlock(lock, result);
      };
  if (state_ == ParserParentSessionState::terminal) {
    return finish_receive(failure_);
  }
  if (!received.valid()) {
    return abort_receive(fail_channel_locked(received.result));
  }

  switch (received.frame.header.type) {
    case parser::MessageType::entry_batch: {
      const auto accepted =
          result_session_.accept_entry_batch(received.frame);
      if (!accepted.valid()) {
        return abort_receive(fail_result_locked(accepted));
      }
      return finish_receive(success_locked(
          ParserParentSessionDisposition::progress, active_request_id_));
    }
    case parser::MessageType::data_chunk: {
      if (stream_sink_ == nullptr) {
        return abort_receive(
            fail_protocol_locked(parser::ProtocolError::unexpected_message));
      }
      const auto accepted =
          result_session_.accept_data_chunk(received.frame, *stream_sink_);
      if (!accepted.valid()) {
        return abort_receive(fail_result_locked(accepted));
      }
      return finish_receive(success_locked(
          ParserParentSessionDisposition::progress, active_request_id_));
    }
    case parser::MessageType::complete: {
      ParserResultSessionResult accepted;
      auto disposition = ParserParentSessionDisposition::unavailable;
      if (active_operation_ == ActiveOperation::enumeration) {
        accepted = result_session_.complete_enumeration(received.frame);
        disposition =
            ParserParentSessionDisposition::enumeration_complete;
      } else if (active_operation_ == ActiveOperation::stream) {
        accepted = result_session_.complete_stream(received.frame);
        disposition = ParserParentSessionDisposition::stream_complete;
      } else {
        return abort_receive(
            fail_protocol_locked(parser::ProtocolError::unexpected_message));
      }
      if (!accepted.valid()) {
        return abort_receive(fail_result_locked(accepted));
      }
      const auto request_id = active_request_id_;
      clear_operation_locked(ParserParentSessionState::idle);
      return finish_receive(success_locked(disposition, request_id));
    }
    case parser::MessageType::read_request: {
      const auto prepared = source_reads_.prepare(
          received.frame, read_scratch, reply_storage);
      if (!prepared.valid()) {
        return abort_receive(fail_source_locked(prepared.result));
      }
      if (prepared.disposition ==
          ParserSourceReadDisposition::ignored_after_cancel) {
        return finish_receive(success_locked(
            ParserParentSessionDisposition::read_ignored_after_cancel,
            active_request_id_));
      }
      if (!prepared.reply_ready()) {
        return abort_receive(
            fail_protocol_locked(parser::ProtocolError::unexpected_message));
      }
      begin_outbound_locked(OutboundTransaction::read_reply,
                            active_request_id_);
      lock.unlock();
      const auto sent = channel_.send(prepared.reply_header,
                                      prepared.reply_payload, deadline,
                                      cancellation);
      lock.lock();
      if (state_ == ParserParentSessionState::terminal) {
        if (sent.valid()) {
          static_cast<void>(
              source_reads_.commit_reply_sent(prepared.ticket));
        } else {
          static_cast<void>(source_reads_.abandon_reply(prepared.ticket));
        }
        return finish_receive(failure_);
      }
      if (outbound_transaction_ != OutboundTransaction::read_reply) {
        static_cast<void>(source_reads_.abandon_reply(prepared.ticket));
        return abort_receive(
            fail_protocol_locked(parser::ProtocolError::unexpected_message));
      }
      if (!sent.valid()) {
        const auto abandoned =
            source_reads_.abandon_reply(prepared.ticket);
        return abort_receive(fail_channel_locked(sent, abandoned));
      }
      const auto committed =
          source_reads_.commit_reply_sent(prepared.ticket);
      if (!committed.valid()) {
        return abort_receive(fail_source_locked(committed));
      }
      end_outbound_locked();
      return finish_receive(success_locked(
          ParserParentSessionDisposition::read_replied, active_request_id_));
    }
    case parser::MessageType::cancel_ack: {
      const auto accepted =
          result_session_.accept_cancel_ack(received.frame);
      if (!accepted.valid()) {
        return abort_receive(fail_result_locked(accepted));
      }
      const auto request_id = active_request_id_;
      clear_operation_locked(ParserParentSessionState::cancelled);
      return finish_receive(success_locked(
          ParserParentSessionDisposition::cancellation_acknowledged,
          request_id));
    }
    case parser::MessageType::hello:
    case parser::MessageType::ready:
    case parser::MessageType::enumerate:
    case parser::MessageType::stream_entry:
    case parser::MessageType::read_reply:
    case parser::MessageType::cancel:
    case parser::MessageType::shutdown:
      return abort_receive(
          fail_protocol_locked(parser::ProtocolError::unexpected_message));
  }
  return abort_receive(
      fail_protocol_locked(parser::ProtocolError::unexpected_message));
}

ParserParentSessionResult ParserParentSession::request_cancel(
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  std::unique_lock lock{transaction_mutex_};
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::none) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  if (state_ != ParserParentSessionState::enumerating &&
      state_ != ParserParentSessionState::streaming) {
    return transient_locked(ParserParentSessionError::invalid_state);
  }
  const parser::FrameHeader header{
      .type = parser::MessageType::cancel,
      .payload_length = 0,
      .session_id = channel_.session_id(),
      .request_id = active_request_id_,
  };
  begin_outbound_locked(OutboundTransaction::cancel, active_request_id_);
  const auto accepted = result_session_.accept_cancel(frame(header, {}));
  if (!accepted.valid()) {
    return abort_after_unlock(lock, fail_result_locked(accepted));
  }
  lock.unlock();
  const auto sent = channel_.send(header, {}, deadline, cancellation);
  lock.lock();
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::cancel) {
    return abort_after_unlock(
        lock, fail_protocol_locked(parser::ProtocolError::unexpected_message));
  }
  if (!sent.valid()) {
    return abort_after_unlock(lock, fail_channel_locked(sent));
  }
  state_ = ParserParentSessionState::cancelling;
  end_outbound_locked();
  return success_locked(ParserParentSessionDisposition::request_sent,
                        active_request_id_);
}

ParserParentSessionResult ParserParentSession::shutdown(
    const std::chrono::steady_clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  std::unique_lock lock{transaction_mutex_};
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::none) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  if (state_ == ParserParentSessionState::closed) {
    return transient_locked(ParserParentSessionError::invalid_state);
  }
  if (receiver_active_) {
    return transient_locked(ParserParentSessionError::concurrent_operation);
  }
  if (state_ != ParserParentSessionState::idle &&
      state_ != ParserParentSessionState::cancelled) {
    return transient_locked(ParserParentSessionError::invalid_state);
  }
  const parser::FrameHeader header{
      .type = parser::MessageType::shutdown,
      .payload_length = 0,
      .session_id = channel_.session_id(),
      .request_id = 0,
  };
  begin_outbound_locked(OutboundTransaction::shutdown, 0);
  const auto accepted = result_session_.accept_shutdown(frame(header, {}));
  if (!accepted.valid()) {
    return abort_after_unlock(lock, fail_result_locked(accepted));
  }
  lock.unlock();
  const auto sent = channel_.send(header, {}, deadline, cancellation);
  lock.lock();
  if (state_ == ParserParentSessionState::terminal) {
    return failure_;
  }
  if (outbound_transaction_ != OutboundTransaction::shutdown) {
    return abort_after_unlock(
        lock, fail_protocol_locked(parser::ProtocolError::unexpected_message));
  }
  if (!sent.valid()) {
    return abort_after_unlock(lock, fail_channel_locked(sent));
  }
  clear_operation_locked(ParserParentSessionState::closed);
  end_outbound_locked();
  return success_locked(ParserParentSessionDisposition::shutdown_sent);
}

void ParserParentSession::notify_worker_failed() noexcept {
  bool abort_channel = false;
  {
    const std::scoped_lock lock{transaction_mutex_};
    if (state_ == ParserParentSessionState::terminal ||
        state_ == ParserParentSessionState::closed) {
      return;
    }
    result_session_.worker_failed();
    ParserParentSessionResult failure{
        .error = ParserParentSessionError::worker_failure,
        .disposition = ParserParentSessionDisposition::unavailable,
        .request_id = current_request_id_locked(),
        .protocol_error = parser::ProtocolError::none,
        .channel_result = {},
        .session_result = result_session_.result(),
        .source_result = source_reads_.result(),
    };
    retain_failure_locked(failure);
    abort_channel = true;
  }
  if (abort_channel) {
    channel_.abort();
  }
}

void ParserParentSession::invalidate_source() noexcept {
  bool abort_channel = false;
  {
    const std::scoped_lock lock{transaction_mutex_};
    if (state_ == ParserParentSessionState::terminal ||
        state_ == ParserParentSessionState::closed) {
      return;
    }
    result_session_.invalidate_source();
    ParserParentSessionResult failure{
        .error = ParserParentSessionError::source_invalidated,
        .disposition = ParserParentSessionDisposition::unavailable,
        .request_id = current_request_id_locked(),
        .protocol_error = parser::ProtocolError::none,
        .channel_result = {},
        .session_result = result_session_.result(),
        .source_result = source_reads_.result(),
    };
    retain_failure_locked(failure);
    abort_channel = true;
  }
  if (abort_channel) {
    channel_.abort();
  }
}

bool ParserParentSession::terminal() const noexcept {
  const std::scoped_lock lock{transaction_mutex_};
  return state_ == ParserParentSessionState::terminal;
}

ParserParentSessionState ParserParentSession::state() const noexcept {
  const std::scoped_lock lock{transaction_mutex_};
  return state_;
}

ParserParentSessionResult ParserParentSession::result() const noexcept {
  const std::scoped_lock lock{transaction_mutex_};
  return state_ == ParserParentSessionState::terminal
             ? failure_
             : ParserParentSessionResult{};
}

std::optional<ParserResultCatalogView> ParserParentSession::catalog()
    const noexcept {
  const std::scoped_lock lock{transaction_mutex_};
  if (outbound_transaction_ != OutboundTransaction::none) {
    return std::nullopt;
  }
  return result_session_.catalog();
}

ParserParentSessionResult ParserParentSession::success_locked(
    const ParserParentSessionDisposition disposition,
    const std::uint64_t request_id) const noexcept {
  return {
      .error = ParserParentSessionError::none,
      .disposition = disposition,
      .request_id = request_id,
      .protocol_error = parser::ProtocolError::none,
      .channel_result = {},
      .session_result = {},
      .source_result = {},
  };
}

ParserParentSessionResult ParserParentSession::transient_locked(
    const ParserParentSessionError error) const noexcept {
  return {
      .error = error,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = active_request_id_,
      .protocol_error = parser::ProtocolError::none,
      .channel_result = {},
      .session_result = {},
      .source_result = {},
  };
}

ParserParentSessionResult ParserParentSession::fail_channel_locked(
    const ParserFrameChannelResult channel_result,
    const ParserSourceReadBrokerResult source_result) noexcept {
  result_session_.worker_failed();
  ParserParentSessionResult failure{
      .error = ParserParentSessionError::channel_failure,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = current_request_id_locked(),
      .protocol_error = channel_result.protocol_error,
      .channel_result = channel_result,
      .session_result = result_session_.result(),
      .source_result = source_result,
  };
  retain_failure_locked(failure);
  return failure_;
}

ParserParentSessionResult ParserParentSession::fail_protocol_locked(
    const parser::ProtocolError protocol_error) noexcept {
  result_session_.worker_failed();
  ParserParentSessionResult failure{
      .error = ParserParentSessionError::protocol_failure,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = current_request_id_locked(),
      .protocol_error = protocol_error,
      .channel_result = {},
      .session_result = result_session_.result(),
      .source_result = source_reads_.result(),
  };
  retain_failure_locked(failure);
  return failure_;
}

ParserParentSessionResult ParserParentSession::fail_result_locked(
    const ParserResultSessionResult session_result) noexcept {
  auto error = ParserParentSessionError::result_failure;
  if (session_result.error ==
      ParserResultSessionError::protocol_failure) {
    error = ParserParentSessionError::protocol_failure;
  } else if (session_result.error ==
             ParserResultSessionError::allocation_failure) {
    error = ParserParentSessionError::allocation_failure;
  }
  ParserParentSessionResult failure{
      .error = error,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = current_request_id_locked(),
      .protocol_error = session_result.protocol_error,
      .channel_result = {},
      .session_result = session_result,
      .source_result = source_reads_.result(),
  };
  retain_failure_locked(failure);
  return failure_;
}

ParserParentSessionResult ParserParentSession::fail_source_locked(
    const ParserSourceReadBrokerResult source_result) noexcept {
  ParserParentSessionResult failure{
      .error = ParserParentSessionError::source_failure,
      .disposition = ParserParentSessionDisposition::unavailable,
      .request_id = current_request_id_locked(),
      .protocol_error = source_result.protocol_error,
      .channel_result = {},
      .session_result = result_session_.result(),
      .source_result = source_result,
  };
  retain_failure_locked(failure);
  return failure_;
}

bool ParserParentSession::allocate_request_id_locked(
    std::uint64_t& request_id) noexcept {
  if (request_ids_exhausted_) {
    return false;
  }
  request_id = next_request_id_;
  if (next_request_id_ == std::numeric_limits<std::uint64_t>::max()) {
    request_ids_exhausted_ = true;
  } else {
    ++next_request_id_;
  }
  return true;
}

void ParserParentSession::begin_outbound_locked(
    const OutboundTransaction transaction,
    const std::uint64_t request_id) noexcept {
  outbound_transaction_ = transaction;
  outbound_request_id_ = request_id;
}

void ParserParentSession::end_outbound_locked() noexcept {
  outbound_transaction_ = OutboundTransaction::none;
  outbound_request_id_ = 0;
  transaction_condition_.notify_all();
}

std::uint64_t ParserParentSession::current_request_id_locked()
    const noexcept {
  return outbound_transaction_ == OutboundTransaction::none
             ? active_request_id_
             : outbound_request_id_;
}

ParserParentSessionResult ParserParentSession::abort_after_unlock(
    std::unique_lock<std::mutex>& lock,
    const ParserParentSessionResult result) noexcept {
  lock.unlock();
  channel_.abort();
  return result;
}

void ParserParentSession::clear_operation_locked(
    const ParserParentSessionState state) noexcept {
  state_ = state;
  active_operation_ = ActiveOperation::none;
  active_request_id_ = 0;
  stream_sink_ = nullptr;
}

void ParserParentSession::retain_failure_locked(
    const ParserParentSessionResult failure) noexcept {
  if (state_ != ParserParentSessionState::terminal) {
    failure_ = failure;
    state_ = ParserParentSessionState::terminal;
    active_operation_ = ActiveOperation::none;
    active_request_id_ = 0;
    stream_sink_ = nullptr;
    outbound_transaction_ = OutboundTransaction::none;
    outbound_request_id_ = 0;
    transaction_condition_.notify_all();
  }
}

ParserParentSessionCreateResult create_parser_parent_session(
    ParserParentHandshakeProof&& proof, ParserFrameChannel& channel,
    const ValidatedMedia& media, const std::uint64_t worker_epoch,
    const PayloadImportLimits import_limits) noexcept {
  if (!proof.valid() || !proof.matches_channel(channel) ||
      channel.terminal() || !media.valid() ||
      media.source() == nullptr ||
      media.source()->size() != media.fingerprint().size_bytes ||
      !import_limits_valid(import_limits) || worker_epoch == 0) {
    return {
        .result = simple_error(
            ParserParentSessionError::invalid_configuration),
        .session = nullptr,
    };
  }
  const auto source_read_limits = proof.source_read_limits();
  const auto source_read_policy = proof.source_read_policy();
  if (source_read_policy.source_size != media.fingerprint().size_bytes ||
      source_read_policy.maximum_read_bytes !=
          source_read_limits.maximum_read_bytes ||
      !source_read_policy.valid() || !source_read_limits.valid()) {
    return {
        .result = simple_error(
            ParserParentSessionError::invalid_configuration),
        .session = nullptr,
    };
  }

  auto protocol = proof.take_protocol();
  if (!protocol.has_value()) {
    return {
        .result = simple_error(
            ParserParentSessionError::invalid_configuration),
        .session = nullptr,
    };
  }

  try {
    auto session = std::unique_ptr<ParserParentSession>{
        new ParserParentSession(channel, media, std::move(*protocol),
                                worker_epoch, import_limits,
                                source_read_limits)};
    if (session->result_session_.terminal() ||
        session->source_reads_.terminal()) {
      const auto result = ParserParentSessionResult{
          .error = ParserParentSessionError::invalid_configuration,
          .disposition = ParserParentSessionDisposition::unavailable,
          .request_id = 0,
          .protocol_error = parser::ProtocolError::none,
          .channel_result = {},
          .session_result = session->result_session_.result(),
          .source_result = session->source_reads_.result(),
      };
      session.reset();
      channel.abort();
      return {.result = result, .session = nullptr};
    }
    return {.result = {}, .session = std::move(session)};
  } catch (const std::bad_alloc&) {
    channel.abort();
    return {
        .result = simple_error(
            ParserParentSessionError::allocation_failure),
        .session = nullptr,
    };
  } catch (...) {
    channel.abort();
    return {
        .result = simple_error(ParserParentSessionError::internal_failure),
        .session = nullptr,
    };
  }
}

}  // namespace ohl::media
