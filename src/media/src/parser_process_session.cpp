#include "ohl/media/parser_process_session.hpp"

#include <limits>
#include <utility>

namespace ohl::media {

namespace {
using Clock = std::chrono::steady_clock;
}  // namespace

ParserSessionIdAllocator::ParserSessionIdAllocator(
    const std::uint64_t first_session_id,
    const std::uint64_t first_worker_epoch) noexcept
    : next_session_id_(first_session_id),
      next_worker_epoch_(first_worker_epoch),
      exhausted_(first_session_id == 0 || first_worker_epoch == 0) {}

ParserSessionIdAllocationResult ParserSessionIdAllocator::allocate() noexcept {
  if (exhausted_ || next_session_id_ == 0 || next_worker_epoch_ == 0) {
    exhausted_ = true;
    return {.error = ParserSessionIdAllocatorError::exhausted, .allocation = {}};
  }

  const ParserSessionAllocation allocation{
      .session_id = next_session_id_,
      .worker_epoch = next_worker_epoch_,
  };

  constexpr auto kMaximum = std::numeric_limits<std::uint64_t>::max();
  if (next_session_id_ == kMaximum || next_worker_epoch_ == kMaximum) {
    // Fail closed instead of wrapping back to a previously issued or zero
    // value on the very next call.
    exhausted_ = true;
  } else {
    ++next_session_id_;
    ++next_worker_epoch_;
  }

  return {.error = ParserSessionIdAllocatorError::none,
          .allocation = allocation};
}

ParserProcessSession::ParserProcessSession(
    std::unique_ptr<platform::IsolatedWorker> worker,
    const std::uint64_t session_id, const std::uint64_t worker_epoch) noexcept
    : worker_(std::move(worker)),
      session_id_(session_id),
      worker_epoch_(worker_epoch) {}

ParserProcessSession::ParserProcessSession(
    ParserProcessSession&& other) noexcept
    : worker_(std::move(other.worker_)),
      channel_(std::move(other.channel_)),
      parent_session_(std::move(other.parent_session_)),
      session_id_(other.session_id_),
      worker_epoch_(other.worker_epoch_),
      state_(other.state_),
      terminated_once_(other.terminated_once_),
      cached_termination_(other.cached_termination_) {
  other.session_id_ = 0;
  other.worker_epoch_ = 0;
  other.state_ = ParserProcessSessionState::terminated;
  other.terminated_once_ = true;
}

ParserProcessSession& ParserProcessSession::operator=(
    ParserProcessSession&& other) noexcept {
  if (this == &other) {
    return *this;
  }
  if (worker_ != nullptr && !terminal()) {
    static_cast<void>(
        terminate_and_wait_once(Clock::now() + std::chrono::seconds{5}));
  }

  parent_session_ = std::move(other.parent_session_);
  channel_ = std::move(other.channel_);
  worker_ = std::move(other.worker_);
  session_id_ = other.session_id_;
  worker_epoch_ = other.worker_epoch_;
  state_ = other.state_;
  terminated_once_ = other.terminated_once_;
  cached_termination_ = other.cached_termination_;

  other.session_id_ = 0;
  other.worker_epoch_ = 0;
  other.state_ = ParserProcessSessionState::terminated;
  other.terminated_once_ = true;

  return *this;
}

ParserProcessSession::~ParserProcessSession() {
  if (worker_ != nullptr && !terminal()) {
    static_cast<void>(
        terminate_and_wait_once(Clock::now() + std::chrono::seconds{5}));
  }
}

platform::IsolatedWorkerWaitResult
ParserProcessSession::terminate_and_wait_once(
    const Clock::time_point deadline) noexcept {
  if (terminated_once_) {
    return cached_termination_;
  }
  terminated_once_ = true;
  state_ = ParserProcessSessionState::terminated;
  cached_termination_ = worker_->terminate_and_wait(deadline);
  return cached_termination_;
}

ParserProcessSessionOpenResult ParserProcessSession::open(
    const ValidatedMedia& media, const ParserSourceReadLimits source_read_limits,
    const parser::ProtocolBudgets protocol_budgets,
    const std::span<std::byte> handshake_receive_storage,
    const Clock::time_point deadline, const PayloadImportLimits import_limits,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  if (state_ != ParserProcessSessionState::idle) {
    return {.error = ParserProcessSessionError::invalid_state, .termination_result = {}};
  }
  if (worker_ == nullptr || session_id_ == 0 || worker_epoch_ == 0) {
    return {.error = ParserProcessSessionError::invalid_configuration, .termination_result = {}};
  }

  channel_ = std::make_unique<ParserFrameChannel>(
      session_id_, isolated_worker_frame_channel_operations(*worker_));

  auto handshake = perform_parser_parent_handshake(
      *channel_, media, source_read_limits, protocol_budgets,
      handshake_receive_storage, deadline, cancellation);
  if (!handshake.valid()) {
    const auto termination = terminate_and_wait_once(deadline);
    return {.error = ParserProcessSessionError::handshake_failed,
            .handshake_error = handshake.error,
            .termination_result = termination};
  }

  auto created = create_parser_parent_session(
      std::move(*handshake.proof), *channel_, media, worker_epoch_,
      import_limits);
  if (!created.valid()) {
    const auto termination = terminate_and_wait_once(deadline);
    return {.error = ParserProcessSessionError::session_create_failed,
            .session_error = created.result.error,
            .termination_result = termination};
  }

  parent_session_ = std::move(created.session);
  state_ = ParserProcessSessionState::open;
  return {};
}

ParserProcessSessionShutdownResult ParserProcessSession::orderly_shutdown(
    const Clock::time_point deadline,
    const platform::IsolatedWorkerCancellationToken cancellation) noexcept {
  if (state_ == ParserProcessSessionState::closed ||
      state_ == ParserProcessSessionState::terminated) {
    return {.error = ParserProcessSessionError::none,
            .wait_result = cached_termination_,
            .escalated = state_ == ParserProcessSessionState::terminated};
  }
  if (state_ != ParserProcessSessionState::open) {
    return {.error = ParserProcessSessionError::invalid_state, .wait_result = {}};
  }

  const auto session_result = parent_session_->shutdown(deadline, cancellation);
  if (!session_result.valid()) {
    const auto termination = terminate_and_wait_once(deadline);
    return {.error = ParserProcessSessionError::shutdown_failed,
            .session_error = session_result.error,
            .wait_result = termination,
            .escalated = true};
  }

  worker_->close_channel();
  const auto wait_result = worker_->wait(deadline);
  if (wait_result.error != platform::IsolatedWorkerError::none) {
    const auto termination = terminate_and_wait_once(deadline);
    return {.error = ParserProcessSessionError::shutdown_failed,
            .wait_result = termination,
            .escalated = true};
  }

  cached_termination_ = wait_result;
  state_ = ParserProcessSessionState::closed;
  return {.error = ParserProcessSessionError::none,
          .wait_result = wait_result,
          .escalated = false};
}

ParserParentSession* ParserProcessSession::session() noexcept {
  return parent_session_.get();
}

const ParserParentSession* ParserProcessSession::session() const noexcept {
  return parent_session_.get();
}

}  // namespace ohl::media
