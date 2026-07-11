#include "ohl/platform/isolated_worker.hpp"

#include "isolated_worker_windows_internal.hpp"

#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <array>
#include <chrono>
#include <cstddef>
#include <filesystem>
#include <iostream>
#include <string_view>
#include <system_error>
#include <thread>
#include <utility>

#ifndef OHL_WINDOWS_TEST_WORKER_READY_PATH
#error "OHL_WINDOWS_TEST_WORKER_READY_PATH must identify the synthetic worker"
#endif
#ifndef OHL_WINDOWS_TEST_WORKER_BAD_READY_PATH
#error "OHL_WINDOWS_TEST_WORKER_BAD_READY_PATH must identify the bad worker"
#endif
#ifndef OHL_WINDOWS_TEST_WORKER_NO_READY_PATH
#error "OHL_WINDOWS_TEST_WORKER_NO_READY_PATH must identify the silent worker"
#endif
#ifndef OHL_WINDOWS_TEST_WORKER_EXIT_PATH
#error "OHL_WINDOWS_TEST_WORKER_EXIT_PATH must identify the exiting worker"
#endif

namespace {

using Clock = std::chrono::steady_clock;
using namespace std::chrono_literals;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerExitKind;
using ohl::platform::IsolatedWorkerService;

class TestContext final {
 public:
  void expect(const bool condition, const std::string_view message) {
    if (!condition) {
      ++failures_;
      std::cerr << "isolated worker Windows test failed: " << message << '\n';
    }
  }

  [[nodiscard]] int result() const noexcept { return failures_ == 0 ? 0 : 1; }

 private:
  int failures_{0};
};

[[nodiscard]] std::filesystem::path worker_stage_path() {
  std::wstring module_path(32'768U, L'\0');
  const DWORD length = GetModuleFileNameW(
      nullptr, module_path.data(), static_cast<DWORD>(module_path.size()));
  if (length == 0 ||
      static_cast<std::size_t>(length) >= module_path.size()) {
    return {};
  }
  module_path.resize(length);
  return std::filesystem::path{module_path}.parent_path() /
         std::wstring{
             ohl::platform::detail::windows::kMediaParserExecutableName};
}

class WorkerStage final {
 public:
  explicit WorkerStage(std::filesystem::path path) noexcept
      : path_(std::move(path)) {
    remove();
  }

  ~WorkerStage() { remove(); }

  WorkerStage(const WorkerStage&) = delete;
  WorkerStage& operator=(const WorkerStage&) = delete;

  [[nodiscard]] bool copy_from(const std::filesystem::path& source) noexcept {
    remove();
    std::error_code error;
    const bool copied = std::filesystem::copy_file(
        source, path_, std::filesystem::copy_options::overwrite_existing,
        error);
    return copied && !error;
  }

  [[nodiscard]] bool hard_link_from(
      const std::filesystem::path& source) noexcept {
    remove();
    return CreateHardLinkW(path_.c_str(), source.c_str(), nullptr) != FALSE;
  }

  void remove() noexcept {
    std::error_code ignored;
    static_cast<void>(std::filesystem::remove(path_, ignored));
  }

 private:
  std::filesystem::path path_;
};

[[nodiscard]] ohl::platform::IsolatedWorkerLaunchResult launch_worker(
    const Clock::duration timeout = 5s) noexcept {
  return ohl::platform::launch_isolated_worker(
      IsolatedWorkerService::media_parser, Clock::now() + timeout);
}

void terminate_worker(TestContext& test,
                      ohl::platform::IsolatedWorkerLaunchResult& launch) {
  if (!launch.valid()) {
    return;
  }
  const auto terminated =
      launch.worker->terminate_and_wait(Clock::now() + 2s);
  test.expect(terminated.error == IsolatedWorkerError::none &&
                  terminated.exit == IsolatedWorkerExitKind::terminated,
              "the job must terminate, reap, and clean its profile");
  launch.worker.reset();
}

}  // namespace

int main() {
  namespace native = ohl::platform::detail::windows;
  TestContext test;

  test.expect(native::policy_contract_is_valid(),
              "the fixed native confinement policy must remain valid");
  test.expect(native::kMediaParserExecutableName ==
                  L"ohl_media_parser_worker.exe",
              "the service executable identity must be fixed");
  test.expect(native::kBootstrapReady.size() == 8U,
              "the startup handshake must have a fixed width");
  test.expect(native::kProcessMemoryLimitBytes ==
                  native::kJobMemoryLimitBytes,
              "process and job memory ceilings must agree");
  test.expect(native::kCancellationObservationMilliseconds <= 100U,
              "token cancellation must be observed promptly");

  const auto stage_path = worker_stage_path();
  test.expect(!stage_path.empty(), "the test executable path must be known");
  if (stage_path.empty()) {
    return test.result();
  }
  WorkerStage stage{stage_path};

  const auto missing = launch_worker();
  test.expect(!missing.valid() && missing.worker == nullptr &&
                  missing.error == IsolatedWorkerError::service_unavailable,
              "a missing fixed worker executable must fail closed");

  const std::filesystem::path ready_source{
      OHL_WINDOWS_TEST_WORKER_READY_PATH};
  test.expect(stage.hard_link_from(ready_source),
              "the adversarial hard-link worker must stage");
  const auto hard_link = launch_worker();
  test.expect(!hard_link.valid() && hard_link.worker == nullptr &&
                  hard_link.error ==
                      IsolatedWorkerError::service_identity_mismatch,
              "a multiply-linked executable must fail provenance checks");

  test.expect(stage.copy_from(ready_source),
              "the project-authored ready worker must stage");
  auto ready = launch_worker();
  test.expect(ready.valid(),
              "the fixed worker must pass LPAC, job, machine, and bootstrap checks");
  if (ready.valid()) {
    std::array<std::byte, 4> request{
        std::byte{'O'}, std::byte{'H'}, std::byte{'L'}, std::byte{1}};
    const auto written =
        ready.worker->write_all(request, Clock::now() + 2s);
    std::array<std::byte, 4> response{};
    const auto read = ready.worker->read_exact(response, Clock::now() + 2s);
    test.expect(written.error == IsolatedWorkerError::none &&
                    written.bytes_transferred == request.size() &&
                    read.error == IsolatedWorkerError::none &&
                    read.bytes_transferred == response.size() &&
                    response == request,
                "the private overlapped channel must round-trip exact bytes");
  }
  terminate_worker(test, ready);

  auto cancellable = launch_worker();
  test.expect(cancellable.valid(),
              "a second fixed worker must launch for cancellation coverage");
  if (cancellable.valid()) {
    ohl::platform::IsolatedWorkerCancellationSource cancellation_source;
    std::thread canceller{[&cancellation_source] {
      std::this_thread::sleep_for(50ms);
      static_cast<void>(cancellation_source.request_cancellation());
    }};
    std::array<std::byte, 1> blocked_read{};
    const auto started = Clock::now();
    const auto cancelled = cancellable.worker->read_exact(
        blocked_read, Clock::now() + 2s, cancellation_source.token());
    const auto elapsed = Clock::now() - started;
    canceller.join();
    test.expect(cancelled.error == IsolatedWorkerError::cancelled &&
                    elapsed < 1s,
                "token cancellation must promptly CancelIoEx and drain the read");
  }
  terminate_worker(test, cancellable);

  test.expect(stage.copy_from(OHL_WINDOWS_TEST_WORKER_BAD_READY_PATH),
              "the bad-attestation worker must stage");
  const auto bad_ready = launch_worker();
  test.expect(!bad_ready.valid() && bad_ready.worker == nullptr &&
                  bad_ready.error == IsolatedWorkerError::bootstrap_failed,
              "an adversarial bootstrap attestation must fail closed");

  test.expect(stage.copy_from(OHL_WINDOWS_TEST_WORKER_NO_READY_PATH),
              "the silent worker must stage");
  const auto no_ready = launch_worker(3s);
  test.expect(!no_ready.valid() && no_ready.worker == nullptr &&
                  no_ready.error == IsolatedWorkerError::timeout,
              "a silent bootstrap must honor the startup deadline");

  test.expect(stage.copy_from(OHL_WINDOWS_TEST_WORKER_EXIT_PATH),
              "the early-exit worker must stage");
  auto exited = launch_worker();
  test.expect(exited.valid(), "a valid bootstrap may precede a worker failure");
  if (exited.valid()) {
    const auto waited = exited.worker->wait(Clock::now() + 2s);
    test.expect(waited.error == IsolatedWorkerError::none &&
                    waited.exit == IsolatedWorkerExitKind::failed,
                "a synthetic nonzero exit must be sanitized as failed");
  }

  return test.result();
}
