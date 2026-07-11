#include "ohl/platform/isolated_worker.hpp"

#include "isolated_worker_internal.hpp"

#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <iostream>
#include <memory>
#include <mutex>
#include <span>
#include <stop_token>
#include <string_view>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

static_assert(
    std::is_same_v<std::underlying_type_t<ohl::platform::IsolatedWorkerService>,
                   std::uint8_t>);
static_assert(
    std::is_same_v<std::underlying_type_t<ohl::platform::IsolatedWorkerError>,
                   std::uint8_t>);
static_assert(std::is_same_v<
              std::underlying_type_t<ohl::platform::IsolatedWorkerExitKind>,
              std::uint8_t>);

namespace {

using Clock = std::chrono::steady_clock;
using ohl::platform::IsolatedWorker;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerExitKind;
using ohl::platform::IsolatedWorkerIoResult;
using ohl::platform::IsolatedWorkerService;
using ohl::platform::IsolatedWorkerWaitResult;
using namespace std::chrono_literals;

struct FakeState final {
  std::atomic<int> read_calls{0};
  std::atomic<int> write_calls{0};
  std::atomic<int> wait_calls{0};
  std::atomic<int> terminate_wait_calls{0};
  std::atomic<int> abort_calls{0};
  std::atomic<int> close_calls{0};
  std::atomic<int> termination_calls{0};
  std::atomic<int> abort_effects{0};
  std::atomic<int> close_effects{0};
  std::atomic<int> termination_effects{0};
  std::atomic<int> destroyed{0};
  std::atomic_bool aborted{false};
  std::atomic_bool closed{false};
  std::atomic_bool termination_requested{false};

  IsolatedWorkerIoResult read_result{};
  IsolatedWorkerIoResult write_result{};
  bool use_fixed_read_result{false};
  bool use_fixed_write_result{false};
  IsolatedWorkerWaitResult wait_result{IsolatedWorkerExitKind::clean,
                                       IsolatedWorkerError::none};
  IsolatedWorkerWaitResult terminate_result{
      IsolatedWorkerExitKind::terminated, IsolatedWorkerError::none};

  std::mutex deadline_mutex;
  Clock::time_point launch_deadline{};
  Clock::time_point read_deadline{};
  Clock::time_point write_deadline{};
  Clock::time_point wait_deadline{};
  Clock::time_point terminate_deadline{};

  std::mutex block_mutex;
  std::condition_variable block_condition;
  bool block_read{false};
  bool block_write{false};
  bool block_wait{false};
  bool read_entered{false};
  bool write_entered{false};
  bool wait_entered{false};
  bool release_read{false};
  bool release_write{false};
  bool release_wait{false};
};

class FakeBackend final
    : public ohl::platform::detail::IsolatedWorkerBackend {
 public:
  explicit FakeBackend(std::shared_ptr<FakeState> state) noexcept
      : state_(std::move(state)) {}

  ~FakeBackend() override { ++state_->destroyed; }

  [[nodiscard]] IsolatedWorkerIoResult read_exact(
      const std::span<std::byte> destination,
      const Clock::time_point deadline,
      std::stop_token) noexcept override {
    ++state_->read_calls;
    {
      const std::scoped_lock lock{state_->deadline_mutex};
      state_->read_deadline = deadline;
    }
    block_direction(state_->block_read, state_->read_entered,
                    state_->release_read);
    if (state_->use_fixed_read_result) {
      return state_->read_result;
    }
    return {.bytes_transferred = destination.size()};
  }

  [[nodiscard]] IsolatedWorkerIoResult write_all(
      const std::span<const std::byte> source,
      const Clock::time_point deadline,
      std::stop_token) noexcept override {
    ++state_->write_calls;
    {
      const std::scoped_lock lock{state_->deadline_mutex};
      state_->write_deadline = deadline;
    }
    block_direction(state_->block_write, state_->write_entered,
                    state_->release_write);
    if (state_->use_fixed_write_result) {
      return state_->write_result;
    }
    return {.bytes_transferred = source.size()};
  }

  void abort_io() noexcept override {
    ++state_->abort_calls;
    if (!state_->aborted.exchange(true)) {
      ++state_->abort_effects;
    }
  }

  void close_channel() noexcept override {
    ++state_->close_calls;
    if (!state_->closed.exchange(true)) {
      ++state_->close_effects;
    }
  }

  void request_termination() noexcept override {
    ++state_->termination_calls;
    if (!state_->termination_requested.exchange(true)) {
      ++state_->termination_effects;
    }
  }

  [[nodiscard]] IsolatedWorkerWaitResult wait(
      const Clock::time_point deadline) noexcept override {
    ++state_->wait_calls;
    {
      const std::scoped_lock lock{state_->deadline_mutex};
      state_->wait_deadline = deadline;
    }
    block_direction(state_->block_wait, state_->wait_entered,
                    state_->release_wait);
    return state_->wait_result;
  }

  [[nodiscard]] IsolatedWorkerWaitResult terminate_and_wait(
      const Clock::time_point deadline) noexcept override {
    ++state_->terminate_wait_calls;
    {
      const std::scoped_lock lock{state_->deadline_mutex};
      state_->terminate_deadline = deadline;
    }
    return state_->terminate_result;
  }

 private:
  void block_direction(const bool blocked, bool& entered,
                       const bool& released) noexcept {
    if (!blocked) {
      return;
    }
    std::unique_lock lock{state_->block_mutex};
    entered = true;
    state_->block_condition.notify_all();
    state_->block_condition.wait(lock, [&released] { return released; });
  }

  std::shared_ptr<FakeState> state_;
};

struct LaunchPlan final {
  std::shared_ptr<FakeState> state;
  IsolatedWorkerError error{IsolatedWorkerError::none};
  bool provide_backend{true};
};

std::mutex g_launch_mutex;
std::deque<LaunchPlan> g_launch_plans;
std::atomic<int> g_backend_launch_calls{0};

void queue_launch(const std::shared_ptr<FakeState>& state,
                  const IsolatedWorkerError error = IsolatedWorkerError::none,
                  const bool provide_backend = true) {
  const std::scoped_lock lock{g_launch_mutex};
  g_launch_plans.push_back({state, error, provide_backend});
}

[[nodiscard]] std::unique_ptr<IsolatedWorker> launch_worker(
    const std::shared_ptr<FakeState>& state,
    const Clock::time_point deadline = Clock::time_point::max()) {
  queue_launch(state);
  auto result = ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, deadline);
  if (!result.valid()) {
    return nullptr;
  }
  return std::move(result.worker);
}

class TestContext final {
 public:
  void expect(const bool condition, const std::string_view message) {
    if (!condition) {
      failed_ = true;
      std::cerr << "isolated worker common test failed: " << message << '\n';
    }
  }

  [[nodiscard]] bool passed() const noexcept { return !failed_; }

 private:
  bool failed_{false};
};

template <typename Predicate>
[[nodiscard]] bool wait_for_state(const std::shared_ptr<FakeState>& state,
                                  Predicate predicate) {
  std::unique_lock lock{state->block_mutex};
  return state->block_condition.wait_for(lock, 2s, predicate);
}

void release_blocked_operations(const std::shared_ptr<FakeState>& state) {
  {
    const std::scoped_lock lock{state->block_mutex};
    state->release_read = true;
    state->release_write = true;
    state->release_wait = true;
  }
  state->block_condition.notify_all();
}

[[nodiscard]] Clock::time_point recorded_deadline(
    const std::shared_ptr<FakeState>& state,
    Clock::time_point FakeState::*member) {
  const std::scoped_lock lock{state->deadline_mutex};
  return state.get()->*member;
}

void test_launch_validation_and_ownership(TestContext& test) {
  const auto launches_before = g_backend_launch_calls.load();
  const auto invalid = ohl::platform::launch_isolated_worker(
      static_cast<IsolatedWorkerService>(0xffU), Clock::time_point::max());
  test.expect(!invalid.valid() && invalid.worker == nullptr &&
                  invalid.error == IsolatedWorkerError::invalid_argument,
              "unknown service is rejected without a worker");
  test.expect(g_backend_launch_calls.load() == launches_before,
              "unknown service does not reach the backend");

  auto failed_state = std::make_shared<FakeState>();
  queue_launch(failed_state, IsolatedWorkerError::confinement_unavailable);
  const auto failed = ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, Clock::time_point::max());
  test.expect(!failed.valid() && failed.worker == nullptr &&
                  failed.error ==
                      IsolatedWorkerError::confinement_unavailable,
              "backend launch error is preserved without a worker");
  test.expect(failed_state->destroyed.load() == 1,
              "backend returned with an error is immediately destroyed");

  auto missing_state = std::make_shared<FakeState>();
  queue_launch(missing_state, IsolatedWorkerError::none, false);
  const auto missing = ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, Clock::time_point::max());
  test.expect(!missing.valid() && missing.worker == nullptr &&
                  missing.error == IsolatedWorkerError::bootstrap_failed,
              "missing successful backend is classified as bootstrap failure");
  test.expect(missing_state->destroyed.load() == 0,
              "missing backend has no phantom ownership");

  const auto expired = ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, Clock::now() - 1s);
  test.expect(!expired.valid() && expired.error == IsolatedWorkerError::timeout,
              "expired startup deadline is rejected");
  test.expect(g_backend_launch_calls.load() == launches_before + 2,
              "expired startup does not reach the backend");
}

void test_deadlines(TestContext& test) {
  const auto early_launch = Clock::now() + 2s;
  auto early_state = std::make_shared<FakeState>();
  auto worker = launch_worker(early_state, early_launch);
  test.expect(worker != nullptr, "worker launches for deadline tests");
  if (worker == nullptr) {
    return;
  }
  test.expect(recorded_deadline(early_state, &FakeState::launch_deadline) ==
                  early_launch,
              "earlier caller startup deadline wins exactly");

  std::array<std::byte, 4> bytes{};
  const auto early_io = Clock::now() + 2s;
  const auto write = worker->write_all(bytes, early_io);
  test.expect(write.error == IsolatedWorkerError::none &&
                  recorded_deadline(early_state,
                                    &FakeState::write_deadline) == early_io,
              "earlier caller I/O deadline wins exactly");

  const auto read_before = Clock::now();
  const auto read = worker->read_exact(bytes, Clock::time_point::max());
  const auto read_after = Clock::now();
  const auto read_deadline =
      recorded_deadline(early_state, &FakeState::read_deadline);
  test.expect(read.error == IsolatedWorkerError::none &&
                  read_deadline >= read_before + 29s &&
                  read_deadline <= read_after + 30s &&
                  read_deadline != Clock::time_point::max(),
              "maximum I/O time point is overflow-safe and clamped");

  early_state->wait_result = {IsolatedWorkerExitKind::running,
                              IsolatedWorkerError::timeout};
  const auto early_wait = Clock::now() + 2s;
  const auto waited = worker->wait(early_wait);
  test.expect(waited.error == IsolatedWorkerError::timeout &&
                  recorded_deadline(early_state,
                                    &FakeState::wait_deadline) == early_wait,
              "earlier caller observe deadline wins exactly");

  const auto termination_before = Clock::now();
  const auto terminated =
      worker->terminate_and_wait(Clock::time_point::max());
  const auto termination_after = Clock::now();
  const auto termination_deadline =
      recorded_deadline(early_state, &FakeState::terminate_deadline);
  test.expect(terminated.error == IsolatedWorkerError::none &&
                  termination_deadline >= termination_before + 4s &&
                  termination_deadline <= termination_after + 5s &&
                  termination_deadline != Clock::time_point::max(),
              "maximum termination time point is overflow-safe and clamped");

  auto maximum_state = std::make_shared<FakeState>();
  const auto launch_before = Clock::now();
  auto maximum_worker =
      launch_worker(maximum_state, Clock::time_point::max());
  const auto launch_after = Clock::now();
  test.expect(maximum_worker != nullptr, "maximum startup deadline launches");
  if (maximum_worker != nullptr) {
    const auto launch_deadline =
        recorded_deadline(maximum_state, &FakeState::launch_deadline);
    test.expect(launch_deadline >= launch_before + 9s &&
                    launch_deadline <= launch_after + 10s &&
                    launch_deadline != Clock::time_point::max(),
                "maximum startup time point is overflow-safe and clamped");

    maximum_state->wait_result = {IsolatedWorkerExitKind::running,
                                  IsolatedWorkerError::timeout};
    const auto wait_before = Clock::now();
    const auto maximum_wait =
        maximum_worker->wait(Clock::time_point::max());
    const auto wait_after = Clock::now();
    const auto wait_deadline =
        recorded_deadline(maximum_state, &FakeState::wait_deadline);
    test.expect(maximum_wait.error == IsolatedWorkerError::timeout &&
                    wait_deadline >= wait_before + 29s &&
                    wait_deadline <= wait_after + 30s &&
                    wait_deadline != Clock::time_point::max(),
                "maximum observe time point is overflow-safe and clamped");
  }
}

void test_input_validation(TestContext& test) {
  auto state = std::make_shared<FakeState>();
  auto worker = launch_worker(state);
  test.expect(worker != nullptr, "worker launches for input validation");
  if (worker == nullptr) {
    return;
  }

  const std::span<std::byte> empty_mutable;
  const std::span<const std::byte> empty_const;
  test.expect(worker->read_exact(empty_mutable, Clock::time_point::max()).error ==
                  IsolatedWorkerError::invalid_argument,
              "empty read is rejected");
  test.expect(worker->write_all(empty_const, Clock::time_point::max()).error ==
                  IsolatedWorkerError::invalid_argument,
              "empty write is rejected");

  std::vector<std::byte> oversized(
      ohl::platform::kMaximumIsolatedWorkerIoBytes + 1U);
  test.expect(worker->read_exact(oversized, Clock::time_point::max()).error ==
                  IsolatedWorkerError::transfer_too_large,
              "oversized read is rejected");
  test.expect(worker->write_all(oversized, Clock::time_point::max()).error ==
                  IsolatedWorkerError::transfer_too_large,
              "oversized write is rejected");
  test.expect(state->read_calls.load() == 0 && state->write_calls.load() == 0,
              "invalid I/O never reaches the backend");
}

void test_direction_concurrency(TestContext& test) {
  auto state = std::make_shared<FakeState>();
  state->block_read = true;
  state->block_write = true;
  auto worker = launch_worker(state);
  test.expect(worker != nullptr, "worker launches for direction concurrency");
  if (worker == nullptr) {
    return;
  }

  std::array<std::byte, 8> read_bytes{};
  std::array<std::byte, 8> write_bytes{};
  IsolatedWorkerIoResult read_result;
  IsolatedWorkerIoResult write_result;
  std::thread reader{[&] {
    read_result = worker->read_exact(read_bytes, Clock::time_point::max());
  }};
  std::thread writer{[&] {
    write_result = worker->write_all(write_bytes, Clock::time_point::max());
  }};

  const auto both_entered = wait_for_state(
      state, [&] { return state->read_entered && state->write_entered; });
  test.expect(both_entered, "one reader and one writer can enter concurrently");
  if (both_entered) {
    std::array<std::byte, 1> duplicate{};
    test.expect(worker->read_exact(duplicate, Clock::time_point::max()).error ==
                    IsolatedWorkerError::concurrent_operation,
                "duplicate reader is rejected");
    test.expect(worker->write_all(duplicate, Clock::time_point::max()).error ==
                    IsolatedWorkerError::concurrent_operation,
                "duplicate writer is rejected");
  }

  release_blocked_operations(state);
  reader.join();
  writer.join();
  test.expect(read_result.error == IsolatedWorkerError::none &&
                  read_result.bytes_transferred == read_bytes.size() &&
                  write_result.error == IsolatedWorkerError::none &&
                  write_result.bytes_transferred == write_bytes.size(),
              "independent reader and writer complete successfully");
}

enum class Direction : std::uint8_t { read, write };

struct PoisonCase final {
  std::string_view name;
  Direction direction;
  IsolatedWorkerIoResult backend_result;
  IsolatedWorkerError expected_error;
  std::size_t expected_bytes;
};

void test_backend_io_poisoning(TestContext& test) {
  constexpr std::array<PoisonCase, 7> cases{{
      {"partial read", Direction::read,
       {.bytes_transferred = 3, .error = IsolatedWorkerError::none},
       IsolatedWorkerError::io_failure, 3},
      {"over-count write", Direction::write,
       {.bytes_transferred = 9, .error = IsolatedWorkerError::none},
       IsolatedWorkerError::io_failure, 0},
      {"timeout", Direction::read,
       {.bytes_transferred = 0, .error = IsolatedWorkerError::timeout},
       IsolatedWorkerError::timeout, 0},
      {"cancel", Direction::write,
       {.bytes_transferred = 0, .error = IsolatedWorkerError::cancelled},
       IsolatedWorkerError::cancelled, 0},
      {"peer close", Direction::read,
       {.bytes_transferred = 0, .error = IsolatedWorkerError::peer_closed},
       IsolatedWorkerError::peer_closed, 0},
      {"native I/O error", Direction::write,
       {.bytes_transferred = 0, .error = IsolatedWorkerError::io_failure},
       IsolatedWorkerError::io_failure, 0},
      {"native resource error", Direction::read,
       {.bytes_transferred = 0,
        .error = IsolatedWorkerError::resource_exhausted},
       IsolatedWorkerError::resource_exhausted, 0},
  }};

  for (const auto& poison_case : cases) {
    auto state = std::make_shared<FakeState>();
    if (poison_case.direction == Direction::read) {
      state->use_fixed_read_result = true;
      state->read_result = poison_case.backend_result;
    } else {
      state->use_fixed_write_result = true;
      state->write_result = poison_case.backend_result;
    }
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, poison_case.name);
    if (worker == nullptr) {
      continue;
    }

    std::array<std::byte, 8> bytes{};
    const auto result = poison_case.direction == Direction::read
                            ? worker->read_exact(bytes, Clock::time_point::max())
                            : worker->write_all(bytes, Clock::time_point::max());
    test.expect(result.error == poison_case.expected_error &&
                    result.bytes_transferred == poison_case.expected_bytes,
                poison_case.name);
    test.expect(state->abort_effects.load() == 1 &&
                    state->close_effects.load() == 1,
                "failed transfer poisons and closes the stream");
    const auto calls_before =
        state->read_calls.load() + state->write_calls.load();
    test.expect(worker->write_all(bytes, Clock::time_point::max()).error ==
                    IsolatedWorkerError::invalid_state,
                "poisoned stream rejects later I/O");
    test.expect(state->read_calls.load() + state->write_calls.load() ==
                    calls_before,
                "poisoned stream does not call the backend again");
  }
}

void test_caller_timeout_and_cancel_poisoning(TestContext& test) {
  std::array<std::byte, 2> bytes{};
  {
    auto state = std::make_shared<FakeState>();
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for caller timeout");
    if (worker != nullptr) {
      const auto result = worker->read_exact(bytes, Clock::now() - 1s);
      test.expect(result.error == IsolatedWorkerError::timeout &&
                      state->read_calls.load() == 0 &&
                      state->abort_effects.load() == 1 &&
                      state->close_effects.load() == 1,
                  "expired caller I/O poisons without backend transfer");
    }
  }
  {
    auto state = std::make_shared<FakeState>();
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for caller cancellation");
    if (worker != nullptr) {
      std::stop_source stop;
      stop.request_stop();
      const auto result =
          worker->write_all(bytes, Clock::time_point::max(), stop.get_token());
      test.expect(result.error == IsolatedWorkerError::cancelled &&
                      state->write_calls.load() == 0 &&
                      state->abort_effects.load() == 1 &&
                      state->close_effects.load() == 1,
                  "pre-cancelled caller I/O poisons without backend transfer");
    }
  }
}

void test_abort_and_close_idempotency(TestContext& test) {
  {
    auto state = std::make_shared<FakeState>();
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for close idempotency");
    if (worker != nullptr) {
      worker->close_channel();
      worker->close_channel();
      test.expect(state->abort_effects.load() == 0 &&
                      state->close_effects.load() == 1,
                  "repeated orderly close has one backend effect");
      std::array<std::byte, 1> bytes{};
      test.expect(worker->read_exact(bytes, Clock::time_point::max()).error ==
                      IsolatedWorkerError::invalid_state,
                  "orderly close rejects later I/O");
    }
  }
  {
    auto state = std::make_shared<FakeState>();
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for abort idempotency");
    if (worker != nullptr) {
      worker->abort_io();
      worker->abort_io();
      test.expect(state->abort_effects.load() == 1 &&
                      state->close_effects.load() == 1,
                  "repeated abort has one backend abort and close effect");
      std::array<std::byte, 1> bytes{};
      test.expect(worker->read_exact(bytes, Clock::time_point::max()).error ==
                      IsolatedWorkerError::invalid_state,
                  "abort permanently rejects later I/O");
    }
  }
}

void test_wait_normalization_and_cache(TestContext& test) {
  {
    auto state = std::make_shared<FakeState>();
    state->wait_result = {IsolatedWorkerExitKind::crashed,
                          IsolatedWorkerError::none};
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for terminal wait cache");
    if (worker != nullptr) {
      const auto first = worker->wait(Clock::time_point::max());
      const auto second = worker->wait(Clock::now() - 1s);
      const auto terminated =
          worker->terminate_and_wait(Clock::time_point::max());
      test.expect(first.exit == IsolatedWorkerExitKind::crashed &&
                      first.error == IsolatedWorkerError::none &&
                      second.exit == IsolatedWorkerExitKind::crashed &&
                      second.error == IsolatedWorkerError::none &&
                      terminated.exit == IsolatedWorkerExitKind::crashed &&
                      terminated.error == IsolatedWorkerError::none,
                  "terminal wait result is cached across later waits");
      test.expect(state->wait_calls.load() == 1 &&
                      state->terminate_wait_calls.load() == 0,
                  "cached terminal result avoids another reap call");
    }
  }
  {
    auto state = std::make_shared<FakeState>();
    state->wait_result = {IsolatedWorkerExitKind::running,
                          IsolatedWorkerError::none};
    auto worker = launch_worker(state);
    if (worker != nullptr) {
      const auto malformed = worker->wait(Clock::time_point::max());
      test.expect(malformed.exit == IsolatedWorkerExitKind::unknown &&
                      malformed.error == IsolatedWorkerError::reap_failed,
                  "nonterminal successful reap is failed closed");
    }
  }
  {
    auto state = std::make_shared<FakeState>();
    state->wait_result = {IsolatedWorkerExitKind::clean,
                          IsolatedWorkerError::timeout};
    auto worker = launch_worker(state);
    if (worker != nullptr) {
      const auto timeout = worker->wait(Clock::time_point::max());
      test.expect(timeout.exit == IsolatedWorkerExitKind::running &&
                      timeout.error == IsolatedWorkerError::timeout,
                  "wait timeout is normalized to running");
    }
  }
  {
    auto state = std::make_shared<FakeState>();
    state->wait_result = {IsolatedWorkerExitKind::clean,
                          IsolatedWorkerError::io_failure};
    auto worker = launch_worker(state);
    if (worker != nullptr) {
      const auto failed = worker->wait(Clock::time_point::max());
      test.expect(failed.exit == IsolatedWorkerExitKind::unknown &&
                      failed.error == IsolatedWorkerError::io_failure,
                  "wait native error is normalized to unknown");
    }
  }
}

void test_expired_termination_requests_kill(TestContext& test) {
  auto state = std::make_shared<FakeState>();
  auto worker = launch_worker(state);
  test.expect(worker != nullptr, "worker launches for expired termination");
  if (worker == nullptr) {
    return;
  }
  const auto result = worker->terminate_and_wait(Clock::now() - 1s);
  test.expect(result.exit == IsolatedWorkerExitKind::running &&
                  result.error == IsolatedWorkerError::timeout,
              "expired termination returns a timeout");
  test.expect(state->termination_effects.load() == 1 &&
                  state->abort_effects.load() == 1 &&
                  state->close_effects.load() == 1 &&
                  state->terminate_wait_calls.load() == 0,
              "expired termination still closes IPC and requests kill");
}

void test_termination_concurrent_with_wait(TestContext& test) {
  auto state = std::make_shared<FakeState>();
  state->block_wait = true;
  state->wait_result = {IsolatedWorkerExitKind::clean,
                        IsolatedWorkerError::none};
  auto worker = launch_worker(state);
  test.expect(worker != nullptr, "worker launches for concurrent termination");
  if (worker == nullptr) {
    return;
  }

  IsolatedWorkerWaitResult wait_result;
  std::thread waiter{[&] {
    wait_result = worker->wait(Clock::time_point::max());
  }};
  const auto entered =
      wait_for_state(state, [&] { return state->wait_entered; });
  test.expect(entered, "observe wait reaches the blocked backend");
  if (entered) {
    const auto terminated =
        worker->terminate_and_wait(Clock::time_point::max());
    test.expect(terminated.exit == IsolatedWorkerExitKind::unknown &&
                    terminated.error ==
                        IsolatedWorkerError::concurrent_operation,
                "termination reports the concurrent reap");
    test.expect(state->termination_effects.load() == 1 &&
                    state->abort_effects.load() == 1 &&
                    state->close_effects.load() == 1,
                "concurrent termination requests kill before lock failure");
  }
  release_blocked_operations(state);
  waiter.join();
  test.expect(wait_result.exit == IsolatedWorkerExitKind::clean &&
                  wait_result.error == IsolatedWorkerError::none,
              "original blocked wait can finish and cache its reap");
}

void test_destructor_cleanup(TestContext& test) {
  auto state = std::make_shared<FakeState>();
  {
    auto worker = launch_worker(state);
    test.expect(worker != nullptr, "worker launches for destructor cleanup");
  }
  test.expect(state->abort_effects.load() == 1 &&
                  state->close_effects.load() == 1 &&
                  state->termination_effects.load() == 1 &&
                  state->terminate_wait_calls.load() == 1 &&
                  state->destroyed.load() == 1,
              "destructor closes, kills, reaps, and releases backend ownership");
}

}  // namespace

namespace ohl::platform::detail {

IsolatedWorkerBackendLaunchResult launch_isolated_worker_backend(
    const IsolatedWorkerService,
    const Clock::time_point startup_deadline) noexcept {
  ++g_backend_launch_calls;
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
  {
    const std::scoped_lock lock{plan.state->deadline_mutex};
    plan.state->launch_deadline = startup_deadline;
  }
  if (!plan.provide_backend) {
    return {.backend = nullptr, .error = plan.error};
  }
  return {.backend = std::make_unique<FakeBackend>(std::move(plan.state)),
          .error = plan.error};
}

}  // namespace ohl::platform::detail

int main() {
  TestContext test;
  test_launch_validation_and_ownership(test);
  test_deadlines(test);
  test_input_validation(test);
  test_direction_concurrency(test);
  test_backend_io_poisoning(test);
  test_caller_timeout_and_cancel_poisoning(test);
  test_abort_and_close_idempotency(test);
  test_wait_normalization_and_cache(test);
  test_expired_termination_requests_kill(test);
  test_termination_concurrent_with_wait(test);
  test_destructor_cleanup(test);

  {
    const std::scoped_lock lock{g_launch_mutex};
    test.expect(g_launch_plans.empty(), "all queued fake launches were consumed");
  }
  return test.passed() ? 0 : 1;
}
