#include "ohl/media/parser_process_session.hpp"

#include "isolated_worker_internal.hpp"
#include "synthetic_media_test_support.hpp"

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <iostream>
#include <limits>
#include <memory>
#include <mutex>
#include <span>
#include <string_view>
#include <utility>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;
using ohl::media::ParserProcessSession;
using ohl::media::ParserProcessSessionError;
using ohl::media::ParserProcessSessionState;
using ohl::media::ParserSessionIdAllocator;
using ohl::media::ParserSessionIdAllocatorError;
using ohl::media::ParserSourceReadLimits;
using ohl::parser::MessageType;
using ohl::platform::IsolatedWorker;
using ohl::platform::IsolatedWorkerCancellationToken;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerExitKind;
using ohl::platform::IsolatedWorkerIoResult;
using ohl::platform::IsolatedWorkerService;
using ohl::platform::IsolatedWorkerWaitResult;

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

void store_u16(const std::span<std::byte> out, const std::size_t offset,
               const std::uint16_t value) {
  out[offset] = static_cast<std::byte>(value & 0xffU);
  out[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}
void store_u32(const std::span<std::byte> out, const std::size_t offset,
               const std::uint32_t value) {
  for (std::size_t i = 0; i < 4; ++i) {
    out[offset + i] = static_cast<std::byte>((value >> (i * 8U)) & 0xffU);
  }
}
void store_u64(const std::span<std::byte> out, const std::size_t offset,
               const std::uint64_t value) {
  for (std::size_t i = 0; i < 8; ++i) {
    out[offset + i] = static_cast<std::byte>((value >> (i * 8U)) & 0xffU);
  }
}

[[nodiscard]] std::array<std::byte, 32> wire_header(
    const MessageType type, const std::uint32_t payload_size,
    const std::uint64_t session, const std::uint64_t request_id = 0) {
  std::array<std::byte, 32> out{};
  out[0] = std::byte{'O'};
  out[1] = std::byte{'H'};
  out[2] = std::byte{'L'};
  out[3] = std::byte{'P'};
  store_u16(out, 4, 1);
  store_u16(out, 8, static_cast<std::uint16_t>(type));
  store_u32(out, 12, payload_size);
  store_u64(out, 16, session);
  store_u64(out, 24, request_id);
  return out;
}

// A queued synthetic read chunk, in the order the fake backend hands bytes to
// ParserFrameChannel/ParserParentSession.
struct QueuedRead {
  std::vector<std::byte> bytes;
  IsolatedWorkerError error{IsolatedWorkerError::none};
};

// Shared, thread-safe observation/control block. The actual FakeBackend is
// owned by the IsolatedWorker returned from launch_isolated_worker() once
// launched, so tests configure and inspect state through this shared block
// instead of the backend object directly.
struct FakeState final {
  mutable std::mutex mutex;
  std::deque<QueuedRead> reads;
  std::size_t read_calls{0};
  std::size_t write_calls{0};
  std::size_t abort_calls{0};
  std::size_t close_calls{0};
  std::size_t termination_request_calls{0};
  std::size_t wait_calls{0};
  std::size_t terminate_calls{0};
  IsolatedWorkerError next_write_error{IsolatedWorkerError::none};
  IsolatedWorkerWaitResult wait_result{IsolatedWorkerExitKind::clean,
                                       IsolatedWorkerError::none};
  IsolatedWorkerWaitResult terminate_result{IsolatedWorkerExitKind::terminated,
                                            IsolatedWorkerError::none};
};

void push_frame(FakeState& state, const MessageType type,
                const std::uint64_t session,
                const std::uint64_t request_id = 0,
                const std::span<const std::byte> payload = {}) {
  const std::scoped_lock lock{state.mutex};
  const auto header =
      wire_header(type, static_cast<std::uint32_t>(payload.size()), session,
                 request_id);
  state.reads.push_back(
      {std::vector<std::byte>{header.begin(), header.end()},
       IsolatedWorkerError::none});
  if (!payload.empty()) {
    state.reads.push_back(
        {std::vector<std::byte>{payload.begin(), payload.end()},
         IsolatedWorkerError::none});
  }
}

class FakeBackend final : public ohl::platform::detail::IsolatedWorkerBackend {
 public:
  explicit FakeBackend(std::shared_ptr<FakeState> state) noexcept
      : state_(std::move(state)) {}

  [[nodiscard]] IsolatedWorkerIoResult read_exact(
      const std::span<std::byte> destination, Clock::time_point,
      IsolatedWorkerCancellationToken) noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->read_calls;
    if (state_->reads.empty()) {
      return {.bytes_transferred = 0, .error = IsolatedWorkerError::io_failure};
    }
    auto plan = std::move(state_->reads.front());
    state_->reads.pop_front();
    const auto copied = std::min(destination.size(), plan.bytes.size());
    std::copy_n(plan.bytes.begin(), copied, destination.begin());
    return {.bytes_transferred = destination.size(), .error = plan.error};
  }

  [[nodiscard]] IsolatedWorkerIoResult write_all(
      const std::span<const std::byte> source, Clock::time_point,
      IsolatedWorkerCancellationToken) noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->write_calls;
    const auto error = state_->next_write_error;
    if (error != IsolatedWorkerError::none) {
      return {.bytes_transferred = 0, .error = error};
    }
    return {.bytes_transferred = source.size(),
            .error = IsolatedWorkerError::none};
  }

  void abort_io() noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->abort_calls;
  }
  void close_channel() noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->close_calls;
  }
  void request_termination() noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->termination_request_calls;
  }
  [[nodiscard]] IsolatedWorkerWaitResult wait(
      Clock::time_point) noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->wait_calls;
    return state_->wait_result;
  }
  [[nodiscard]] IsolatedWorkerWaitResult terminate_and_wait(
      Clock::time_point) noexcept override {
    const std::scoped_lock lock{state_->mutex};
    ++state_->terminate_calls;
    return state_->terminate_result;
  }

 private:
  std::shared_ptr<FakeState> state_;
};

struct LaunchPlan final {
  std::shared_ptr<FakeState> state;
};

std::mutex g_launch_mutex;
std::deque<LaunchPlan> g_launch_plans;

[[nodiscard]] std::unique_ptr<IsolatedWorker> launch_fake_worker(
    const std::shared_ptr<FakeState>& state) {
  {
    const std::scoped_lock lock{g_launch_mutex};
    g_launch_plans.push_back({state});
  }
  auto result = ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, Clock::time_point::max());
  if (!result.valid()) {
    return nullptr;
  }
  return std::move(result.worker);
}

[[nodiscard]] ParserSourceReadLimits small_read_limits() {
  return {.maximum_read_bytes = 4,
          .maximum_requests = 16,
          .maximum_reply_payload_bytes = 160};
}

struct Fixture final {
  explicit Fixture(const std::uint64_t session_id_in,
                    const std::uint64_t worker_epoch_in)
      : media{ohl::media::test::kSyntheticMinimumSectorCount},
        state{std::make_shared<FakeState>()},
        session_id{session_id_in},
        worker_epoch{worker_epoch_in} {}

  ohl::media::test::SyntheticValidatedMedia media;
  std::shared_ptr<FakeState> state;
  std::uint64_t session_id;
  std::uint64_t worker_epoch;
};

[[nodiscard]] std::unique_ptr<ParserProcessSession> make_session(
    Fixture& fixture) {
  auto worker = launch_fake_worker(fixture.state);
  if (worker == nullptr) return nullptr;
  return std::make_unique<ParserProcessSession>(
      std::move(worker), fixture.session_id, fixture.worker_epoch);
}

[[nodiscard]] bool open_session(Fixture& fixture, ParserProcessSession& session) {
  push_frame(*fixture.state, MessageType::ready, fixture.session_id);
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto opened = session.open(fixture.media.media(), small_read_limits(),
                                    {}, storage, Clock::time_point::max());
  return opened.valid();
}

[[nodiscard]] bool test_allocator_uniqueness_and_exhaustion() {
  ParserSessionIdAllocator allocator;
  const auto first = allocator.allocate();
  const auto second = allocator.allocate();
  if (!first.valid() || !second.valid() ||
      first.allocation.session_id == 0 || first.allocation.worker_epoch == 0 ||
      first.allocation.session_id == second.allocation.session_id ||
      first.allocation.worker_epoch == second.allocation.worker_epoch ||
      second.allocation.session_id <= first.allocation.session_id ||
      second.allocation.worker_epoch <= first.allocation.worker_epoch)
    return fail("allocator did not issue unique monotonic values");

  constexpr auto kMaximum = std::numeric_limits<std::uint64_t>::max();
  ParserSessionIdAllocator near_exhaustion{kMaximum, kMaximum};
  const auto last_valid = near_exhaustion.allocate();
  if (!last_valid.valid() || last_valid.allocation.session_id != kMaximum)
    return fail("allocator rejected a valid maximum-value allocation");
  const auto exhausted = near_exhaustion.allocate();
  if (exhausted.valid() ||
      exhausted.error != ParserSessionIdAllocatorError::exhausted)
    return fail("allocator did not fail closed at exhaustion");
  const auto still_exhausted = near_exhaustion.allocate();
  if (still_exhausted.valid())
    return fail("exhausted allocator resumed issuing values");

  ParserSessionIdAllocator zero_start{0, 1};
  if (zero_start.allocate().valid())
    return fail("zero starting session ID was not treated as exhausted");
  return true;
}

[[nodiscard]] bool test_open_shutdown_clean_reap() {
  Fixture fixture{11, 4100};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  if (!open_session(fixture, *session) ||
      session->state() != ParserProcessSessionState::open)
    return fail("open did not reach the open state");

  const auto shutdown = session->orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  if (!shutdown.valid() || shutdown.escalated ||
      session->state() != ParserProcessSessionState::closed ||
      fixture.state->close_calls != 1 || fixture.state->wait_calls != 1 ||
      fixture.state->terminate_calls != 0)
    return fail("orderly shutdown did not perform a clean close/wait reap");
  return true;
}

[[nodiscard]] bool test_open_failure_terminates_exactly_once() {
  Fixture fixture{12, 4101};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  // No ready frame queued: the handshake read fails, so open() must fail and
  // escalate to terminate_and_wait exactly once.
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  const auto opened = session->open(fixture.media.media(), small_read_limits(),
                                    {}, storage, Clock::time_point::max());
  {
    const std::scoped_lock lock{fixture.state->mutex};
    if (opened.valid() ||
        opened.error != ParserProcessSessionError::handshake_failed ||
        session->state() != ParserProcessSessionState::terminated ||
        fixture.state->terminate_calls != 1)
      return fail("handshake failure did not escalate to a single terminate");
  }

  // A repeated destructive attempt (simulated by calling shutdown again)
  // must not invoke terminate_and_wait a second time.
  const auto again = session->orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  if (fixture.state->terminate_calls != 1 || !again.escalated)
    return fail("post-failure shutdown call re-terminated the worker");
  return true;
}

[[nodiscard]] bool test_shutdown_send_failure_terminates_exactly_once() {
  Fixture fixture{13, 4102};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  if (!open_session(fixture, *session))
    return fail("open failed for shutdown-failure fixture");

  {
    const std::scoped_lock lock{fixture.state->mutex};
    fixture.state->next_write_error = IsolatedWorkerError::peer_closed;
  }
  const auto shutdown = session->orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  // The channel is defensively poisoned (abort_io/close_channel) by more than
  // one layer on a send failure; the one contract this test enforces is that
  // terminate_and_wait() itself runs exactly once.
  if (shutdown.valid() || !shutdown.escalated ||
      session->state() != ParserProcessSessionState::terminated ||
      fixture.state->terminate_calls != 1)
    return fail("protocol shutdown send failure did not escalate exactly once");
  return fixture.state->terminate_calls == 1;
}

[[nodiscard]] bool test_orderly_close_timeout_escalates() {
  Fixture fixture{14, 4103};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  if (!open_session(fixture, *session))
    return fail("open failed for timeout fixture");

  {
    const std::scoped_lock lock{fixture.state->mutex};
    fixture.state->wait_result = {IsolatedWorkerExitKind::running,
                                  IsolatedWorkerError::timeout};
  }
  const auto shutdown = session->orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  // The escalation path defensively re-poisons the channel, so close_calls
  // may exceed the single explicit close_channel() call; the contract this
  // test enforces is a single wait() attempt followed by exactly one
  // terminate_and_wait() escalation.
  if (shutdown.valid() || !shutdown.escalated ||
      session->state() != ParserProcessSessionState::terminated ||
      fixture.state->close_calls < 1 || fixture.state->wait_calls != 1 ||
      fixture.state->terminate_calls != 1)
    return fail("orderly-close timeout did not escalate to terminate_and_wait");
  return true;
}

[[nodiscard]] bool test_destructor_terminates_live_session() {
  Fixture fixture{15, 4104};
  {
    auto session = make_session(fixture);
    if (session == nullptr) return fail("fake worker did not launch");
    if (!open_session(fixture, *session))
      return fail("open failed for destructor fixture");
  }
  const std::scoped_lock lock{fixture.state->mutex};
  if (fixture.state->terminate_calls != 1)
    return fail("destructor of a live session did not terminate the worker");
  return true;
}

[[nodiscard]] bool test_double_shutdown_idempotent() {
  Fixture fixture{16, 4105};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  if (!open_session(fixture, *session))
    return fail("open failed for idempotency fixture");

  const auto first = session->orderly_shutdown(Clock::time_point::max());
  const auto second = session->orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  if (!first.valid() || !second.valid() || second.escalated ||
      fixture.state->close_calls != 1 || fixture.state->wait_calls != 1 ||
      fixture.state->terminate_calls != 0)
    return fail("repeated orderly shutdown was not idempotent");
  return true;
}

[[nodiscard]] bool test_move_transfers_ownership() {
  Fixture fixture{17, 4106};
  auto session = make_session(fixture);
  if (session == nullptr) return fail("fake worker did not launch");
  if (!open_session(fixture, *session))
    return fail("open failed for move fixture");

  ParserProcessSession moved{std::move(*session)};
  if (moved.state() != ParserProcessSessionState::open)
    return fail("moved-to session lost open state");
  // The moved-from object must not attempt to terminate a worker it no
  // longer owns.
  session.reset();
  {
    const std::scoped_lock lock{fixture.state->mutex};
    if (fixture.state->terminate_calls != 0)
      return fail("destroying a moved-from session terminated the worker");
  }
  const auto shutdown = moved.orderly_shutdown(Clock::time_point::max());
  const std::scoped_lock lock{fixture.state->mutex};
  return shutdown.valid() && fixture.state->close_calls == 1 &&
         fixture.state->wait_calls == 1;
}

}  // namespace

namespace ohl::platform::detail {

IsolatedWorkerBackendLaunchResult launch_isolated_worker_backend(
    const IsolatedWorkerService, const Clock::time_point) noexcept {
  LaunchPlan plan;
  {
    const std::scoped_lock lock{g_launch_mutex};
    if (g_launch_plans.empty()) {
      return {.backend = nullptr,
              .error = IsolatedWorkerError::service_unavailable};
    }
    plan = std::move(g_launch_plans.front());
    g_launch_plans.pop_front();
  }
  return {.backend = std::make_unique<FakeBackend>(std::move(plan.state)),
          .error = IsolatedWorkerError::none};
}

}  // namespace ohl::platform::detail

int main() {
  const bool ok = test_allocator_uniqueness_and_exhaustion() &&
                  test_open_shutdown_clean_reap() &&
                  test_open_failure_terminates_exactly_once() &&
                  test_shutdown_send_failure_terminates_exactly_once() &&
                  test_orderly_close_timeout_escalates() &&
                  test_destructor_terminates_live_session() &&
                  test_double_shutdown_idempotent() &&
                  test_move_transfers_ownership();
  return ok ? 0 : 1;
}
