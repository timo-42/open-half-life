#include "ohl/platform/platform.hpp"

#include <cstdlib>
#include <string>

#if defined(_WIN32)
#include <windows.h>
#endif

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

#if defined(_WIN32)
[[nodiscard]] std::optional<std::filesystem::path> environment_path(
    const wchar_t* const name) {
  const auto required = GetEnvironmentVariableW(name, nullptr, 0);
  if (required == 0) {
    return std::nullopt;
  }
  std::wstring value(required, L'\0');
  const auto written = GetEnvironmentVariableW(name, value.data(), required);
  if (written == 0 || written >= required) {
    return std::nullopt;
  }
  value.resize(written);
  return std::filesystem::path{value};
}
#else
[[nodiscard]] std::optional<std::filesystem::path> environment_path(
    const char* const name) {
  const auto* const value = std::getenv(name);
  if (value == nullptr || *value == '\0') {
    return std::nullopt;
  }
  return std::filesystem::path{value};
}
#endif

}  // namespace

PlatformInfo current_platform() noexcept {
  return PlatformInfo{
      .operating_system = detect_operating_system(),
      .architecture = detect_architecture(),
  };
}

std::optional<std::filesystem::path> default_cache_directory() {
#if defined(_WIN32)
  auto override_path = environment_path(L"OHL_CACHE_DIR");
#else
  auto override_path = environment_path("OHL_CACHE_DIR");
#endif
  if (override_path.has_value() && override_path->is_absolute()) {
    return override_path;
  }

#if defined(_WIN32)
  auto base = environment_path(L"LOCALAPPDATA");
  if (base.has_value() && base->is_absolute()) {
    return *base / "OpenHalfLife" / "cache";
  }
#elif defined(__APPLE__)
  auto home = environment_path("HOME");
  if (home.has_value() && home->is_absolute()) {
    return *home / "Library" / "Caches" / "OpenHalfLife";
  }
#elif defined(__linux__)
  auto base = environment_path("XDG_CACHE_HOME");
  if (base.has_value() && base->is_absolute()) {
    return *base / "open-half-life";
  }
  auto home = environment_path("HOME");
  if (home.has_value() && home->is_absolute()) {
    return *home / ".cache" / "open-half-life";
  }
#endif
  return std::nullopt;
}

}  // namespace ohl::platform
