#include "ohl/platform/isolated_worker.hpp"

#include "isolated_worker_internal.hpp"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <memory>
#include <mutex>
#include <new>
#include <span>
#include <utility>

namespace ohl::platform {
namespace {

using Clock = std::chrono::steady_clock;
using namespace std::chrono_literals;

constexpr auto kMaximumStartupDuration = 10s;
constexpr auto kMaximumIoDuration = 30s;
constexpr auto kMaximumWaitDuration = 30s;
constexpr auto kMaximumTerminationDuration = 5s;

template <typename Duration>
[[nodiscard]] Clock::time_point clamp_deadline(
    const Clock::time_point caller_deadline,
    const Duration maximum_duration) noexcept {
  const auto now = Clock::now();
  const auto ceiling =
      std::chrono::duration_cast<Clock::duration>(maximum_duration);
  const auto latest_start = Clock::time_point::max() - ceiling;
  const auto internal_deadline =
      now >= latest_start ? Clock::time_point::max() : now + ceiling;
  return std::min(caller_deadline, internal_deadline);
}

[[nodiscard]] constexpr bool is_known_service(
    const IsolatedWorkerService service) noexcept {
  switch (service) {
    case IsolatedWorkerService::media_parser:
      return true;
  }
  return false;
}

[[nodiscard]] constexpr bool is_terminal_exit(
    const IsolatedWorkerExitKind exit) noexcept {
  return exit != IsolatedWorkerExitKind::running;
}

[[nodiscard]] IsolatedWorkerWaitResult normalize_wait_result(
    IsolatedWorkerWaitResult result) noexcept {
  if (result.error == IsolatedWorkerError::none) {
    if (!is_terminal_exit(result.exit)) {
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }
    return result;
  }

  if (result.error == IsolatedWorkerError::timeout) {
    result.exit = IsolatedWorkerExitKind::running;
  } else {
    result.exit = IsolatedWorkerExitKind::unknown;
  }
  return result;
}

}  // namespace

struct IsolatedWorker::Impl {
  explicit Impl(std::unique_ptr<detail::IsolatedWorkerBackend> native_backend)
      noexcept
      : backend(std::move(native_backend)) {}

  std::unique_ptr<detail::IsolatedWorkerBackend> backend;
  std::mutex state_mutex;
  std::mutex lifecycle_mutex;
  bool read_active{false};
  bool write_active{false};
  std::atomic_bool channel_closed{false};
  std::atomic_bool stream_poisoned{false};
  bool reaped{false};
  IsolatedWorkerExitKind cached_exit{IsolatedWorkerExitKind::unknown};

  [[nodiscard]] IsolatedWorkerError begin_io(bool& operation_active) noexcept {
    try {
      const std::scoped_lock lock{state_mutex};
      if (stream_poisoned.load() || channel_closed.load()) {
        return IsolatedWorkerError::invalid_state;
      }
      if (operation_active) {
        return IsolatedWorkerError::concurrent_operation;
      }
      operation_active = true;
      return IsolatedWorkerError::none;
    } catch (...) {
      stream_poisoned.store(true);
      channel_closed.store(true);
      backend->abort_io();
      backend->close_channel();
      return IsolatedWorkerError::resource_exhausted;
    }
  }

  [[nodiscard]] bool finish_io(bool& operation_active,
                               const bool poison) noexcept {
    try {
      const std::scoped_lock lock{state_mutex};
      operation_active = false;
      if (poison) {
        stream_poisoned.store(true);
        channel_closed.store(true);
      }
    } catch (...) {
      stream_poisoned.store(true);
      channel_closed.store(true);
      backend->abort_io();
      backend->close_channel();
      return false;
    }

    if (poison) {
      backend->abort_io();
      backend->close_channel();
    }
    return true;
  }

  void abort_stream() noexcept {
    stream_poisoned.store(true);
    channel_closed.store(true);
    backend->abort_io();
    backend->close_channel();
  }

  void close_stream() noexcept {
    channel_closed.store(true);
    backend->close_channel();
  }
};

IsolatedWorker::IsolatedWorker(std::unique_ptr<Impl> implementation) noexcept
    : implementation_(std::move(implementation)) {}

IsolatedWorker::~IsolatedWorker() {
  if (implementation_ == nullptr) {
    return;
  }

  const auto ignored = terminate_and_wait(Clock::time_point::max());
  static_cast<void>(ignored);
}

IsolatedWorkerIoResult IsolatedWorker::read_exact(
    const std::span<std::byte> destination,
    const Clock::time_point deadline) noexcept {
  const auto effective_deadline =
      clamp_deadline(deadline, kMaximumIoDuration);
  if (destination.empty() || destination.data() == nullptr) {
    return {.error = IsolatedWorkerError::invalid_argument};
  }
  if (destination.size() > kMaximumIsolatedWorkerIoBytes) {
    return {.error = IsolatedWorkerError::transfer_too_large};
  }

  const auto begin_error =
      implementation_->begin_io(implementation_->read_active);
  if (begin_error != IsolatedWorkerError::none) {
    return {.error = begin_error};
  }

  IsolatedWorkerIoResult result;
  if (effective_deadline <= Clock::now()) {
    result.error = IsolatedWorkerError::timeout;
  } else {
    result =
        implementation_->backend->read_exact(destination, effective_deadline);
  }

  if (result.bytes_transferred > destination.size()) {
    result = {.error = IsolatedWorkerError::io_failure};
  } else if (result.error == IsolatedWorkerError::none &&
             result.bytes_transferred != destination.size()) {
    result.error = IsolatedWorkerError::io_failure;
  }

  if (!implementation_->finish_io(
          implementation_->read_active,
          result.error != IsolatedWorkerError::none)) {
    result.error = IsolatedWorkerError::io_failure;
  }
  return result;
}

IsolatedWorkerIoResult IsolatedWorker::write_all(
    const std::span<const std::byte> source,
    const Clock::time_point deadline) noexcept {
  const auto effective_deadline =
      clamp_deadline(deadline, kMaximumIoDuration);
  if (source.empty() || source.data() == nullptr) {
    return {.error = IsolatedWorkerError::invalid_argument};
  }
  if (source.size() > kMaximumIsolatedWorkerIoBytes) {
    return {.error = IsolatedWorkerError::transfer_too_large};
  }

  const auto begin_error =
      implementation_->begin_io(implementation_->write_active);
  if (begin_error != IsolatedWorkerError::none) {
    return {.error = begin_error};
  }

  IsolatedWorkerIoResult result;
  if (effective_deadline <= Clock::now()) {
    result.error = IsolatedWorkerError::timeout;
  } else {
    result = implementation_->backend->write_all(source, effective_deadline);
  }

  if (result.bytes_transferred > source.size()) {
    result = {.error = IsolatedWorkerError::io_failure};
  } else if (result.error == IsolatedWorkerError::none &&
             result.bytes_transferred != source.size()) {
    result.error = IsolatedWorkerError::io_failure;
  }

  if (!implementation_->finish_io(
          implementation_->write_active,
          result.error != IsolatedWorkerError::none)) {
    result.error = IsolatedWorkerError::io_failure;
  }
  return result;
}

void IsolatedWorker::abort_io() noexcept {
  implementation_->abort_stream();
}

void IsolatedWorker::close_channel() noexcept {
  implementation_->close_stream();
}

IsolatedWorkerWaitResult IsolatedWorker::wait(
    const Clock::time_point deadline) noexcept {
  const auto effective_deadline =
      clamp_deadline(deadline, kMaximumWaitDuration);
  try {
    const std::unique_lock lifecycle_lock{implementation_->lifecycle_mutex,
                                          std::try_to_lock};
    if (!lifecycle_lock.owns_lock()) {
      return {.exit = IsolatedWorkerExitKind::running,
              .error = IsolatedWorkerError::concurrent_operation};
    }
    if (implementation_->reaped) {
      return {.exit = implementation_->cached_exit};
    }
    if (effective_deadline <= Clock::now()) {
      return {.exit = IsolatedWorkerExitKind::running,
              .error = IsolatedWorkerError::timeout};
    }

    const auto result = normalize_wait_result(
        implementation_->backend->wait(effective_deadline));
    if (result.error == IsolatedWorkerError::none) {
      implementation_->reaped = true;
      implementation_->cached_exit = result.exit;
    }
    return result;
  } catch (...) {
    implementation_->abort_stream();
    implementation_->backend->request_termination();
    return {.exit = IsolatedWorkerExitKind::unknown,
            .error = IsolatedWorkerError::resource_exhausted};
  }
}

IsolatedWorkerWaitResult IsolatedWorker::terminate_and_wait(
    const Clock::time_point deadline) noexcept {
  const auto effective_deadline =
      clamp_deadline(deadline, kMaximumTerminationDuration);
  implementation_->backend->request_termination();
  abort_io();

  try {
    const std::unique_lock lifecycle_lock{implementation_->lifecycle_mutex,
                                          std::try_to_lock};
    if (!lifecycle_lock.owns_lock()) {
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::concurrent_operation};
    }
    if (implementation_->reaped) {
      return {.exit = implementation_->cached_exit};
    }
    if (effective_deadline <= Clock::now()) {
      return {.exit = IsolatedWorkerExitKind::running,
              .error = IsolatedWorkerError::timeout};
    }

    const auto result = normalize_wait_result(
        implementation_->backend->terminate_and_wait(effective_deadline));
    if (result.error == IsolatedWorkerError::none) {
      implementation_->reaped = true;
      implementation_->cached_exit = result.exit;
    }
    return result;
  } catch (...) {
    implementation_->backend->request_termination();
    return {.exit = IsolatedWorkerExitKind::unknown,
            .error = IsolatedWorkerError::resource_exhausted};
  }
}

IsolatedWorkerLaunchResult launch_isolated_worker(
    const IsolatedWorkerService service,
    const Clock::time_point startup_deadline) noexcept {
  const auto effective_deadline =
      clamp_deadline(startup_deadline, kMaximumStartupDuration);
  if (!is_known_service(service)) {
    return {.worker = nullptr,
            .error = IsolatedWorkerError::invalid_argument};
  }
  if (effective_deadline <= Clock::now()) {
    return {.worker = nullptr, .error = IsolatedWorkerError::timeout};
  }

  auto backend_result =
      detail::launch_isolated_worker_backend(service, effective_deadline);
  if (backend_result.error != IsolatedWorkerError::none) {
    backend_result.backend.reset();
    return {.worker = nullptr, .error = backend_result.error};
  }
  if (backend_result.backend == nullptr) {
    return {.worker = nullptr,
            .error = IsolatedWorkerError::bootstrap_failed};
  }

  try {
    auto implementation =
        std::make_unique<IsolatedWorker::Impl>(std::move(backend_result.backend));
    auto worker = std::unique_ptr<IsolatedWorker>{
        new IsolatedWorker(std::move(implementation))};
    return {.worker = std::move(worker), .error = IsolatedWorkerError::none};
  } catch (const std::bad_alloc&) {
    return {.worker = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  } catch (...) {
    return {.worker = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  }
}

}  // namespace ohl::platform
