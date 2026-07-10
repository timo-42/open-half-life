#include "ohl/platform/atomic_directory_store.hpp"

#if !defined(__linux__)

namespace ohl::platform {

AtomicDirectoryStoreOpenResult open_atomic_directory_store(
    const std::filesystem::path&) noexcept {
  return {.store = nullptr,
          .error = AtomicDirectoryStoreError::unsupported};
}

}  // namespace ohl::platform

#endif
