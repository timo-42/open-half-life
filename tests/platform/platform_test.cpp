#include "ohl/platform/platform.hpp"

#include <iostream>

int main() {
  const auto platform = ohl::platform::current_platform();
  if (platform.operating_system == ohl::platform::OperatingSystem::unknown) {
    std::cerr << "the test runner operating system was not detected\n";
    return 1;
  }
  if (platform.architecture == ohl::platform::Architecture::unknown) {
    std::cerr << "the test runner architecture was not detected\n";
    return 1;
  }

  const auto cache_directory = ohl::platform::default_cache_directory();
  if (!cache_directory.has_value() || !cache_directory->is_absolute()) {
    std::cerr << "absolute default cache directory is unavailable\n";
    return 1;
  }

#if defined(OHL_EXPECTED_OS) && defined(OHL_EXPECTED_ARCH)
  if (ohl::platform::to_string(platform.operating_system) != OHL_EXPECTED_OS ||
      ohl::platform::to_string(platform.architecture) != OHL_EXPECTED_ARCH) {
    std::cerr << "runner architecture does not match the CI matrix: "
              << ohl::platform::to_string(platform.operating_system) << ' '
              << ohl::platform::to_string(platform.architecture) << '\n';
    return 1;
  }
#endif

  return 0;
}
