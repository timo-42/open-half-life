#pragma once

#include "ohl/media/iso_inspector.hpp"

#include <filesystem>
#include <string>
#include <string_view>

namespace ohl::media {

enum class ImportCacheError {
  none,
  invalid_request,
  source_read_failed,
  source_changed,
  unsafe_cache_path,
  cache_create_failed,
  manifest_conflict,
  manifest_write_failed,
};

[[nodiscard]] constexpr std::string_view to_string(
    const ImportCacheError error) noexcept {
  switch (error) {
    case ImportCacheError::none:
      return "none";
    case ImportCacheError::invalid_request:
      return "invalid cache request";
    case ImportCacheError::source_read_failed:
      return "source media could not be validated";
    case ImportCacheError::source_changed:
      return "source media changed after preflight";
    case ImportCacheError::unsafe_cache_path:
      return "cache path is relative, linked, or not a directory";
    case ImportCacheError::cache_create_failed:
      return "cache directory could not be created";
    case ImportCacheError::manifest_conflict:
      return "existing provenance manifest does not match the source";
    case ImportCacheError::manifest_write_failed:
      return "provenance manifest could not be published";
  }
  return "unknown import cache error";
}

struct ImportCacheResult {
  ImportCacheError error{ImportCacheError::none};
  std::filesystem::path source_directory;
  std::string source_sha256;
  bool cache_hit{false};

  [[nodiscard]] bool valid() const noexcept {
    return error == ImportCacheError::none;
  }
};

// Rehashes the same pinned source and publishes metadata only after the digest
// equals the validation fingerprint and all source checks pass. It never
// reopens a source pathname. No source bytes are copied and no media-provided
// code is run.
[[nodiscard]] ImportCacheResult prepare_import_cache(
    const ValidatedMedia& media,
    const std::filesystem::path& cache_root);

}  // namespace ohl::media
