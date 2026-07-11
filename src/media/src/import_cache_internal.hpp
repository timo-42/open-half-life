#pragma once

#include "ohl/media/import_cache.hpp"

namespace ohl::media::detail {

using BeforeImportCachePublish = void (*)(void* context) noexcept;

// Source-private dependency injection for deterministic publication-race
// tests. The callback is invoked synchronously and does not outlive the call.
struct ImportCachePublishHook {
  BeforeImportCachePublish before_publish{nullptr};
  void* context{nullptr};
};

[[nodiscard]] ImportCacheResult prepare_import_cache_with_hook(
    const ValidatedMedia& media, const std::filesystem::path& cache_root,
    ImportCachePublishHook hook);

}  // namespace ohl::media::detail
