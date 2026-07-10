#include "ohl/platform/platform.hpp"

namespace ohl::platform {
namespace {

constexpr OperatingSystem detect_operating_system() noexcept {
#if defined(_WIN32)
  return OperatingSystem::windows;
#elif defined(__APPLE__)
  return OperatingSystem::macos;
#elif defined(__linux__)
  return OperatingSystem::linux_os;
#else
  return OperatingSystem::unknown;
#endif
}

constexpr Architecture detect_architecture() noexcept {
#if defined(_M_X64) || defined(__x86_64__)
  return Architecture::x86_64;
#elif defined(_M_ARM64) || defined(__aarch64__)
  return Architecture::arm64;
#else
  return Architecture::unknown;
#endif
}

}  // namespace

PlatformInfo current_platform() noexcept {
  return PlatformInfo{
      .operating_system = detect_operating_system(),
      .architecture = detect_architecture(),
  };
}

}  // namespace ohl::platform
