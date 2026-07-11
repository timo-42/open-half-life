#pragma once

#if !defined(__linux__) || !defined(__x86_64__)
#error "The native isolated-worker backend supports Linux x86-64 only"
#endif

#include <array>
#include <cstddef>
#if !defined(OHL_LINUX_ISOLATED_WORKER_FREESTANDING)
#include "ohl/platform/isolated_worker.hpp"

#include <string_view>

#include <sys/types.h>
#endif

namespace ohl::platform::detail::linux_isolated_worker {

inline constexpr int kWorkerChannelDescriptor = 3;
inline constexpr int kWorkerReadyDescriptor = 4;
inline constexpr std::array<std::byte, 16> kWorkerReadyAttestation{
    std::byte{'O'}, std::byte{'H'}, std::byte{'L'}, std::byte{'I'},
    std::byte{'S'}, std::byte{'O'}, std::byte{'L'}, std::byte{'A'},
    std::byte{'T'}, std::byte{'E'}, std::byte{'D'}, std::byte{0},
    std::byte{1},   std::byte{0},   std::byte{0},   std::byte{0}};

// This path is selected by the build, never by an API caller or environment
// variable. The descriptor opened for it is pinned and verified before fork.
#if !defined(OHL_LINUX_ISOLATED_WORKER_FREESTANDING)
[[nodiscard]] std::string_view service_executable_path(
    IsolatedWorkerService service) noexcept;

// Converts waitpid(2)'s sanitized child status into the public vocabulary.
[[nodiscard]] IsolatedWorkerExitKind classify_wait_status(
    int wait_status, bool termination_requested) noexcept;
#endif

}  // namespace ohl::platform::detail::linux_isolated_worker
