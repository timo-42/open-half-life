#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <string_view>

namespace ohl::platform::detail::windows {

// This contract is deliberately fixed in the engine. Callers cannot select a
// program, mode, native handle, or resource policy.
inline constexpr std::wstring_view kMediaParserExecutableName =
    L"ohl_media_parser_worker.exe";
inline constexpr std::wstring_view kMediaParserModeArgument =
    L"--ohl-isolated-worker=media-parser";
inline constexpr std::wstring_view kIpcHandleArgumentPrefix =
    L"--ohl-ipc-handle=";
inline constexpr std::wstring_view kBootstrapArgument =
    L"--ohl-bootstrap=1";

inline constexpr std::array<std::byte, 8> kBootstrapReady = {
    static_cast<std::byte>('O'), static_cast<std::byte>('H'),
    static_cast<std::byte>('L'), static_cast<std::byte>('W'),
    static_cast<std::byte>('R'), static_cast<std::byte>('D'),
    static_cast<std::byte>('Y'), static_cast<std::byte>(1)};

inline constexpr std::size_t kPipeBufferBytes = 64U * 1024U;
inline constexpr std::size_t kProcessMemoryLimitBytes = 256U * 1024U * 1024U;
inline constexpr std::size_t kJobMemoryLimitBytes = 256U * 1024U * 1024U;
inline constexpr std::int64_t kProcessCpuTimeLimit100ns =
    60LL * 10'000'000LL;
inline constexpr std::uint32_t kCpuHardCapHundredthsOfPercent = 2'500U;
inline constexpr std::uint32_t kTerminationExitCode = 0xE04F484CU;
inline constexpr std::uint32_t kCancellationObservationMilliseconds = 10U;

[[nodiscard]] constexpr bool policy_contract_is_valid() noexcept {
  return !kMediaParserExecutableName.empty() &&
         kMediaParserExecutableName.front() != L'.' &&
         kProcessMemoryLimitBytes == kJobMemoryLimitBytes &&
         kProcessMemoryLimitBytes >= kPipeBufferBytes &&
         kProcessCpuTimeLimit100ns > 0 &&
         kCpuHardCapHundredthsOfPercent > 0U &&
         kCpuHardCapHundredthsOfPercent <= 10'000U &&
         kCancellationObservationMilliseconds > 0U &&
         kCancellationObservationMilliseconds <= 100U;
}

static_assert(policy_contract_is_valid());

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
enum class IsolatedWorkerLaunchStage : std::uint8_t {
  idle,
  launch_started,
  worker_identity_pending,
  worker_identity_verified,
  profile_creation_pending,
  profile_created,
  pipe_creation_pending,
  pipe_connect_pending,
  pipe_connected,
  pipe_created,
  job_configuration_pending,
  job_configured,
  process_policy_pending,
  process_policy_created,
  process_creation_pending,
  process_created,
  image_verification_pending,
  image_verified,
  machine_verification_pending,
  machine_verified,
  token_verification_pending,
  token_open_pending,
  token_opened,
  app_container_verification_pending,
  app_container_verified,
  lpac_verification_pending,
  lpac_verified,
  app_sid_verification_pending,
  app_sid_verified,
  capability_verification_pending,
  capabilities_verified,
  token_verified,
  mitigation_verification_pending,
  mitigations_verified,
  job_verification_pending,
  job_verified,
  resume_pending,
  resumed,
  ready_pending,
  ready,
};

[[nodiscard]] IsolatedWorkerLaunchStage last_isolated_worker_launch_stage()
    noexcept;

[[nodiscard]] constexpr std::string_view isolated_worker_launch_stage_name(
    const IsolatedWorkerLaunchStage stage) noexcept {
  switch (stage) {
    case IsolatedWorkerLaunchStage::idle:
      return "idle";
    case IsolatedWorkerLaunchStage::launch_started:
      return "launch_started";
    case IsolatedWorkerLaunchStage::worker_identity_pending:
      return "worker_identity_pending";
    case IsolatedWorkerLaunchStage::worker_identity_verified:
      return "worker_identity_verified";
    case IsolatedWorkerLaunchStage::profile_creation_pending:
      return "profile_creation_pending";
    case IsolatedWorkerLaunchStage::profile_created:
      return "profile_created";
    case IsolatedWorkerLaunchStage::pipe_creation_pending:
      return "pipe_creation_pending";
    case IsolatedWorkerLaunchStage::pipe_connect_pending:
      return "pipe_connect_pending";
    case IsolatedWorkerLaunchStage::pipe_connected:
      return "pipe_connected";
    case IsolatedWorkerLaunchStage::pipe_created:
      return "pipe_created";
    case IsolatedWorkerLaunchStage::job_configuration_pending:
      return "job_configuration_pending";
    case IsolatedWorkerLaunchStage::job_configured:
      return "job_configured";
    case IsolatedWorkerLaunchStage::process_policy_pending:
      return "process_policy_pending";
    case IsolatedWorkerLaunchStage::process_policy_created:
      return "process_policy_created";
    case IsolatedWorkerLaunchStage::process_creation_pending:
      return "process_creation_pending";
    case IsolatedWorkerLaunchStage::process_created:
      return "process_created";
    case IsolatedWorkerLaunchStage::image_verification_pending:
      return "image_verification_pending";
    case IsolatedWorkerLaunchStage::image_verified:
      return "image_verified";
    case IsolatedWorkerLaunchStage::machine_verification_pending:
      return "machine_verification_pending";
    case IsolatedWorkerLaunchStage::machine_verified:
      return "machine_verified";
    case IsolatedWorkerLaunchStage::token_verification_pending:
      return "token_verification_pending";
    case IsolatedWorkerLaunchStage::token_open_pending:
      return "token_open_pending";
    case IsolatedWorkerLaunchStage::token_opened:
      return "token_opened";
    case IsolatedWorkerLaunchStage::app_container_verification_pending:
      return "app_container_verification_pending";
    case IsolatedWorkerLaunchStage::app_container_verified:
      return "app_container_verified";
    case IsolatedWorkerLaunchStage::lpac_verification_pending:
      return "lpac_verification_pending";
    case IsolatedWorkerLaunchStage::lpac_verified:
      return "lpac_verified";
    case IsolatedWorkerLaunchStage::app_sid_verification_pending:
      return "app_sid_verification_pending";
    case IsolatedWorkerLaunchStage::app_sid_verified:
      return "app_sid_verified";
    case IsolatedWorkerLaunchStage::capability_verification_pending:
      return "capability_verification_pending";
    case IsolatedWorkerLaunchStage::capabilities_verified:
      return "capabilities_verified";
    case IsolatedWorkerLaunchStage::token_verified:
      return "token_verified";
    case IsolatedWorkerLaunchStage::mitigation_verification_pending:
      return "mitigation_verification_pending";
    case IsolatedWorkerLaunchStage::mitigations_verified:
      return "mitigations_verified";
    case IsolatedWorkerLaunchStage::job_verification_pending:
      return "job_verification_pending";
    case IsolatedWorkerLaunchStage::job_verified:
      return "job_verified";
    case IsolatedWorkerLaunchStage::resume_pending:
      return "resume_pending";
    case IsolatedWorkerLaunchStage::resumed:
      return "resumed";
    case IsolatedWorkerLaunchStage::ready_pending:
      return "ready_pending";
    case IsolatedWorkerLaunchStage::ready:
      return "ready";
  }
  return "unknown";
}
#endif
}  // namespace ohl::platform::detail::windows
