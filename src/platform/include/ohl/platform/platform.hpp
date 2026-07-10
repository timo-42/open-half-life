#pragma once

#include <string_view>

namespace ohl::platform {

enum class OperatingSystem {
  windows,
  linux_os,
  macos,
  unknown,
};

enum class Architecture {
  x86_64,
  arm64,
  unknown,
};

struct PlatformInfo {
  OperatingSystem operating_system;
  Architecture architecture;
};

[[nodiscard]] constexpr std::string_view to_string(
    const OperatingSystem operating_system) noexcept {
  switch (operating_system) {
    case OperatingSystem::windows:
      return "Windows";
    case OperatingSystem::linux_os:
      return "Linux";
    case OperatingSystem::macos:
      return "macOS";
    case OperatingSystem::unknown:
      return "Unknown OS";
  }

  return "Unknown OS";
}

[[nodiscard]] constexpr std::string_view to_string(
    const Architecture architecture) noexcept {
  switch (architecture) {
    case Architecture::x86_64:
      return "x86_64";
    case Architecture::arm64:
      return "arm64";
    case Architecture::unknown:
      return "unknown architecture";
  }

  return "unknown architecture";
}

[[nodiscard]] PlatformInfo current_platform() noexcept;

}  // namespace ohl::platform
