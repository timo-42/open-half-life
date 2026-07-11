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

}  // namespace ohl::platform::detail::windows
