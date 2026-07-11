#pragma once

#include "ohl/platform/isolated_worker.hpp"

#include <chrono>
#include <cstddef>
#include <memory>
#include <span>

namespace ohl::platform::detail {

// Private seam between common state/lifetime enforcement and native process
// containment. Implementations must be thread-safe for one concurrent reader,
// one concurrent writer, asynchronous abort/close, and a termination request
// concurrent with wait(). abort_io(), close_channel(), and
// request_termination() must be idempotent. request_termination() must not
// block waiting for process exit. The destructor is the final ownership
// backstop and must terminate and reap any remaining child.
class IsolatedWorkerBackend {
 public:
  virtual ~IsolatedWorkerBackend() = default;

  [[nodiscard]] virtual IsolatedWorkerIoResult read_exact(
      std::span<std::byte> destination,
      std::chrono::steady_clock::time_point deadline,
      IsolatedWorkerCancellationToken cancellation) noexcept = 0;
  [[nodiscard]] virtual IsolatedWorkerIoResult write_all(
      std::span<const std::byte> source,
      std::chrono::steady_clock::time_point deadline,
      IsolatedWorkerCancellationToken cancellation) noexcept = 0;
  virtual void abort_io() noexcept = 0;
  virtual void close_channel() noexcept = 0;
  virtual void request_termination() noexcept = 0;
  [[nodiscard]] virtual IsolatedWorkerWaitResult wait(
      std::chrono::steady_clock::time_point deadline) noexcept = 0;
  [[nodiscard]] virtual IsolatedWorkerWaitResult terminate_and_wait(
      std::chrono::steady_clock::time_point deadline) noexcept = 0;
};

struct IsolatedWorkerBackendLaunchResult {
  std::unique_ptr<IsolatedWorkerBackend> backend;
  IsolatedWorkerError error{IsolatedWorkerError::none};
};

// Exactly one platform backend provides this function. It must validate the
// built-in service identity, establish confinement and IPC, complete bootstrap
// before the deadline, and return no backend on every failure.
[[nodiscard]] IsolatedWorkerBackendLaunchResult
launch_isolated_worker_backend(
    IsolatedWorkerService service,
    std::chrono::steady_clock::time_point startup_deadline) noexcept;

}  // namespace ohl::platform::detail
