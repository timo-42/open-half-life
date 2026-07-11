#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <cwchar>
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

// GetTokenInformation(TokenSecurityAttributes) returns the native
// TOKEN_SECURITY_ATTRIBUTES_INFORMATION layout with counted, not
// null-terminated, attribute names — not the CLAIM_SECURITY_ATTRIBUTES
// layout from winnt.h. Declare the documented native layout with
// fixed-width types so both the launcher and the worker attestation parse
// it identically.
struct TokenSecurityAttributeName final {
  std::uint16_t length_bytes;
  std::uint16_t maximum_length_bytes;
  const wchar_t* buffer;
};

struct TokenSecurityAttributeV1 final {
  TokenSecurityAttributeName name;
  std::uint16_t value_type;
  std::uint16_t reserved;
  std::uint32_t flags;
  std::uint32_t value_count;
  union {
    const std::int64_t* p_int64;
    const std::uint64_t* p_uint64;
  } values;
};

struct TokenSecurityAttributesInformation final {
  std::uint16_t version;
  std::uint16_t reserved;
  std::uint32_t attribute_count;
  union {
    const TokenSecurityAttributeV1* p_attribute_v1;
  } attribute;
};

inline constexpr std::uint16_t kTokenSecurityAttributesVersionV1 = 1U;
inline constexpr std::uint16_t kTokenSecurityAttributeTypeInt64 = 0x0001U;
inline constexpr std::uint16_t kTokenSecurityAttributeTypeUint64 = 0x0002U;
// The kernel marks less privileged AppContainer tokens with this security
// attribute.
inline constexpr std::wstring_view kLpacSecurityAttributeName =
    L"WIN://NOALLAPPPKG";

// Walks a TokenSecurityAttributes result buffer and reports whether the
// kernel's LPAC marker is present with a nonzero integer value. Any absent,
// malformed, or unexpectedly typed marker reads as not LPAC.
[[nodiscard]] inline bool token_security_attributes_mark_lpac(
    const void* const data) noexcept {
  const auto* information =
      static_cast<const TokenSecurityAttributesInformation*>(data);
  if (information->version != kTokenSecurityAttributesVersionV1) {
    return false;
  }
  if (information->attribute_count != 0 &&
      information->attribute.p_attribute_v1 == nullptr) {
    return false;
  }
  for (std::uint32_t index = 0; index < information->attribute_count;
       ++index) {
    const TokenSecurityAttributeV1& attribute =
        information->attribute.p_attribute_v1[index];
    const TokenSecurityAttributeName& name = attribute.name;
    if (name.buffer == nullptr ||
        name.length_bytes !=
            kLpacSecurityAttributeName.size() * sizeof(wchar_t) ||
        std::wmemcmp(name.buffer, kLpacSecurityAttributeName.data(),
                     kLpacSecurityAttributeName.size()) != 0) {
      continue;
    }
    if (attribute.value_count == 0) {
      return false;
    }
    if (attribute.value_type == kTokenSecurityAttributeTypeUint64) {
      return attribute.values.p_uint64 != nullptr &&
             attribute.values.p_uint64[0] != 0U;
    }
    if (attribute.value_type == kTokenSecurityAttributeTypeInt64) {
      return attribute.values.p_int64 != nullptr &&
             attribute.values.p_int64[0] != 0;
    }
    return false;
  }
  return false;
}

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

enum class IsolatedWorkerCreateProcessFailure : std::uint8_t {
  none,
  invalid_parameter,
  access_denied_or_job_nesting,
  unsupported_attribute,
  bad_image,
  file_or_path,
  other,
};

[[nodiscard]] IsolatedWorkerLaunchStage last_isolated_worker_launch_stage()
    noexcept;
[[nodiscard]] IsolatedWorkerCreateProcessFailure
last_isolated_worker_create_process_failure() noexcept;
// Raw Win32 error from the last failed CreateProcessW, for test diagnostics
// only; ERROR_SUCCESS when no creation failure has been recorded.
[[nodiscard]] std::uint32_t last_isolated_worker_create_process_error()
    noexcept;
// Worker exit code observed when the last bootstrap wait failed, for test
// diagnostics only; 259 (STILL_ACTIVE) reports a live worker, 0xFFFFFFFF
// that no bootstrap failure has been recorded, and 0xFFFFFFFE that the
// query itself failed.
[[nodiscard]] std::uint32_t last_isolated_worker_bootstrap_exit_code()
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

[[nodiscard]] constexpr std::string_view
isolated_worker_create_process_failure_name(
    const IsolatedWorkerCreateProcessFailure failure) noexcept {
  switch (failure) {
    case IsolatedWorkerCreateProcessFailure::none:
      return "none";
    case IsolatedWorkerCreateProcessFailure::invalid_parameter:
      return "invalid_parameter";
    case IsolatedWorkerCreateProcessFailure::access_denied_or_job_nesting:
      return "access_denied_or_job_nesting";
    case IsolatedWorkerCreateProcessFailure::unsupported_attribute:
      return "unsupported_attribute";
    case IsolatedWorkerCreateProcessFailure::bad_image:
      return "bad_image";
    case IsolatedWorkerCreateProcessFailure::file_or_path:
      return "file_or_path";
    case IsolatedWorkerCreateProcessFailure::other:
      return "other";
  }
  return "unknown";
}
#endif
}  // namespace ohl::platform::detail::windows
