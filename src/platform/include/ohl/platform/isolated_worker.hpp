#pragma once

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <stop_token>

namespace ohl::platform {

inline constexpr std::size_t kMaximumIsolatedWorkerIoBytes = 1U << 20U;

// Services are fixed application capabilities. Callers cannot select an
// executable, arguments, environment, working directory, or native handles.
enum class IsolatedWorkerService : std::uint8_t {
  media_parser,
};

enum class IsolatedWorkerError : std::uint8_t {
  none,
  invalid_argument,
  invalid_state,
  concurrent_operation,
  unsupported,
  service_unavailable,
  service_identity_mismatch,
  confinement_unavailable,
  channel_creation_failed,
  process_creation_failed,
  bootstrap_failed,
  resource_exhausted,
  transfer_too_large,
  timeout,
  cancelled,
  peer_closed,
  io_failure,
  termination_failed,
  reap_failed,
};

enum class IsolatedWorkerExitKind : std::uint8_t {
  running,
  clean,
  failed,
  crashed,
  resource_limit,
  terminated,
  unknown,
};

struct IsolatedWorkerIoResult {
  std::size_t bytes_transferred{0};
  IsolatedWorkerError error{IsolatedWorkerError::none};
};

struct IsolatedWorkerWaitResult {
  IsolatedWorkerExitKind exit{IsolatedWorkerExitKind::unknown};
  IsolatedWorkerError error{IsolatedWorkerError::none};
};

struct IsolatedWorkerLaunchResult;

// A single confined child and its private full-duplex byte channel. At most
// one read and one write may be active concurrently. Successful operations
// transfer the entire non-empty span. Any partial transfer, timeout,
// cancellation, peer closure, or I/O failure permanently poisons and closes
// the channel; callers must launch another worker instead of resuming it.
class IsolatedWorker final {
 public:
  ~IsolatedWorker();

  IsolatedWorker(const IsolatedWorker&) = delete;
  IsolatedWorker& operator=(const IsolatedWorker&) = delete;
  IsolatedWorker(IsolatedWorker&&) = delete;
  IsolatedWorker& operator=(IsolatedWorker&&) = delete;

  // Caller deadlines earlier than the internal 30-second operation ceiling
  // win.
  [[nodiscard]] IsolatedWorkerIoResult read_exact(
      std::span<std::byte> destination,
      std::chrono::steady_clock::time_point deadline,
      std::stop_token stop = {}) noexcept;
  [[nodiscard]] IsolatedWorkerIoResult write_all(
      std::span<const std::byte> source,
      std::chrono::steady_clock::time_point deadline,
      std::stop_token stop = {}) noexcept;

  // Both operations are idempotent and safe to call while I/O is active.
  // abort_io() permanently poisons the channel. close_channel() performs an
  // orderly local close when no further protocol traffic is required.
  void abort_io() noexcept;
  void close_channel() noexcept;

  // wait() normally observes only; an internal synchronization failure fails
  // closed by requesting termination. A successful terminal result is cached.
  // Observe waits are capped at 30 seconds. terminate_and_wait() closes IPC,
  // requests containment termination, and allows at most 5 seconds to reap.
  // The destructor retains ownership and never intentionally abandons a live
  // child.
  [[nodiscard]] IsolatedWorkerWaitResult wait(
      std::chrono::steady_clock::time_point deadline) noexcept;
  [[nodiscard]] IsolatedWorkerWaitResult terminate_and_wait(
      std::chrono::steady_clock::time_point deadline) noexcept;

 private:
  struct Impl;
  explicit IsolatedWorker(std::unique_ptr<Impl> implementation) noexcept;

  std::unique_ptr<Impl> implementation_;

  friend struct IsolatedWorkerLaunchResult;
  friend IsolatedWorkerLaunchResult launch_isolated_worker(
      IsolatedWorkerService service,
      std::chrono::steady_clock::time_point startup_deadline) noexcept;
};

struct IsolatedWorkerLaunchResult {
  std::unique_ptr<IsolatedWorker> worker;
  IsolatedWorkerError error{IsolatedWorkerError::none};

  [[nodiscard]] bool valid() const noexcept {
    return worker != nullptr && error == IsolatedWorkerError::none;
  }
};

// Launch is all-or-nothing: success returns one fully bootstrapped confined
// worker, and every failure returns no worker. Startup is capped at 10 seconds.
// Service executable identity and confinement policy are selected and verified
// by the native backend.
[[nodiscard]] IsolatedWorkerLaunchResult launch_isolated_worker(
    IsolatedWorkerService service,
    std::chrono::steady_clock::time_point startup_deadline) noexcept;

}  // namespace ohl::platform
