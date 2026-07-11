#include "ohl/media/import_cache.hpp"

#include "import_cache_internal.hpp"
#include "ohl/core/sha256.hpp"
#include "source_stability_internal.hpp"

#include <array>
#include <cstddef>
#include <fstream>
#include <random>
#include <span>
#include <sstream>
#include <system_error>

namespace ohl::media {
namespace {

[[nodiscard]] std::string escape_json(const std::string_view value) {
  constexpr char kHex[] = "0123456789abcdef";
  std::string result;
  result.reserve(value.size());
  for (const auto character : value) {
    const auto byte = static_cast<unsigned char>(character);
    switch (character) {
      case '"':
        result += "\\\"";
        break;
      case '\\':
        result += "\\\\";
        break;
      case '\b':
        result += "\\b";
        break;
      case '\f':
        result += "\\f";
        break;
      case '\n':
        result += "\\n";
        break;
      case '\r':
        result += "\\r";
        break;
      case '\t':
        result += "\\t";
        break;
      default:
        if (byte < 0x20U) {
          result += "\\u00";
          result.push_back(kHex[byte >> 4U]);
          result.push_back(kHex[byte & 0x0fU]);
        } else {
          result.push_back(character);
        }
        break;
    }
  }
  return result;
}

[[nodiscard]] std::string make_manifest(const IsoInspection& inspection,
                                        const SourceFingerprint& fingerprint) {
  std::ostringstream output;
  output << "{\n"
         << "  \"format_version\": 1,\n"
         << "  \"source\": {\n"
         << "    \"sha256\": \"" << fingerprint.sha256 << "\",\n"
         << "    \"size_bytes\": " << fingerprint.size_bytes << ",\n"
         << "    \"filesystem\": \""
         << escape_json(inspection.filesystem) << "\",\n"
         << "    \"volume_label\": \""
         << escape_json(inspection.volume_label) << "\"\n"
         << "  },\n"
         << "  \"payload_state\": \"not-imported\"\n"
         << "}\n";
  return output.str();
}

[[nodiscard]] bool directory_is_safe(const std::filesystem::path& path,
                                     bool& exists) {
  std::error_code error_code;
  const auto status = std::filesystem::symlink_status(path, error_code);
  if (error_code == std::errc::no_such_file_or_directory) {
    exists = false;
    return true;
  }
  if (error_code) {
    return false;
  }
  exists = std::filesystem::exists(status);
  return !exists || (std::filesystem::is_directory(status) &&
                     !std::filesystem::is_symlink(status));
}

[[nodiscard]] ImportCacheError ensure_directory_tree(
    const std::filesystem::path& path) {
  if (!path.is_absolute() || path.filename().empty()) {
    return ImportCacheError::unsafe_cache_path;
  }

  auto current = path.root_path();
  for (const auto& component : path.relative_path()) {
    if (component == "." || component == "..") {
      return ImportCacheError::unsafe_cache_path;
    }
    current /= component;
    bool exists = false;
    if (!directory_is_safe(current, exists)) {
      return ImportCacheError::unsafe_cache_path;
    }
    if (!exists) {
      std::error_code error_code;
      const auto created =
          std::filesystem::create_directory(current, error_code);
      if (error_code || (!created && !directory_is_safe(current, exists))) {
        return ImportCacheError::cache_create_failed;
      }
      if (!directory_is_safe(current, exists) || !exists) {
        return ImportCacheError::unsafe_cache_path;
      }
    }
  }
  return ImportCacheError::none;
}

[[nodiscard]] ImportCacheError verify_validated_source(
    const SharedMediaSource& source) noexcept {
  if (source == nullptr) {
    return ImportCacheError::invalid_request;
  }
  const auto error = source->verify_unchanged();
  if (error == ohl::platform::MediaSourceError::none) {
    return ImportCacheError::none;
  }
  return error == ohl::platform::MediaSourceError::changed
             ? ImportCacheError::source_changed
             : ImportCacheError::source_read_failed;
}

[[nodiscard]] ImportCacheError map_source_stability_error(
    const detail::SourceStabilityError error) noexcept {
  switch (error) {
    case detail::SourceStabilityError::none:
      return ImportCacheError::none;
    case detail::SourceStabilityError::invalid_capability:
      return ImportCacheError::invalid_request;
    case detail::SourceStabilityError::source_changed:
    case detail::SourceStabilityError::digest_mismatch:
      return ImportCacheError::source_changed;
    case detail::SourceStabilityError::read_failure:
    case detail::SourceStabilityError::cancelled:
      return ImportCacheError::source_read_failed;
  }
  return ImportCacheError::source_read_failed;
}

[[nodiscard]] ImportCacheError inspect_existing_manifest(
    const std::filesystem::path& path, const std::string_view expected,
    bool& exists) {
  exists = false;
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid()) {
    if (opened.error == ohl::platform::MediaSourceError::not_found) {
      return ImportCacheError::none;
    }
    return opened.error == ohl::platform::MediaSourceError::not_regular_file
               ? ImportCacheError::unsafe_cache_path
               : ImportCacheError::manifest_write_failed;
  }
  exists = true;
  if (opened.source->size() != expected.size()) {
    return ImportCacheError::manifest_conflict;
  }
  if (opened.source->verify_unchanged() !=
      ohl::platform::MediaSourceError::none) {
    return ImportCacheError::manifest_write_failed;
  }

  std::string existing(expected.size(), '\0');
  auto bytes = std::span<std::byte>{
      reinterpret_cast<std::byte*>(existing.data()), existing.size()};
  if (opened.source->read_exact_at(0, bytes) !=
          ohl::platform::MediaSourceError::none ||
      opened.source->verify_unchanged() !=
          ohl::platform::MediaSourceError::none) {
    return ImportCacheError::manifest_write_failed;
  }
  return existing == expected ? ImportCacheError::none
                              : ImportCacheError::manifest_conflict;
}

[[nodiscard]] std::string temporary_suffix() {
  std::array<std::byte, 16> entropy{};
  std::random_device random;
  for (auto& byte : entropy) {
    byte = static_cast<std::byte>(random() & 0xffU);
  }
  return ohl::core::hex_encode(entropy);
}

[[nodiscard]] ImportCacheError publish_manifest(
    const std::filesystem::path& directory,
    const std::string_view manifest,
    bool& cache_hit, const detail::ImportCachePublishHook hook) {
  const auto destination = directory / "provenance.json";
  bool destination_exists = false;
  auto result = inspect_existing_manifest(destination, manifest,
                                          destination_exists);
  if (result != ImportCacheError::none) {
    return result;
  }
  if (destination_exists) {
    cache_hit = true;
    return ImportCacheError::none;
  }

  std::error_code error_code;
  std::filesystem::path temporary;
  try {
    temporary = directory / (".provenance-" + temporary_suffix() + ".tmp");
  } catch (...) {
    return ImportCacheError::manifest_write_failed;
  }
  const auto temporary_status =
      std::filesystem::symlink_status(temporary, error_code);
  if (error_code == std::errc::no_such_file_or_directory) {
    error_code.clear();
  }
  if (error_code || std::filesystem::exists(temporary_status)) {
    return ImportCacheError::manifest_write_failed;
  }
  {
    std::ofstream output{temporary, std::ios::binary | std::ios::trunc};
    output.write(manifest.data(), static_cast<std::streamsize>(manifest.size()));
    output.flush();
    if (!output) {
      output.close();
      std::filesystem::remove(temporary, error_code);
      return ImportCacheError::manifest_write_failed;
    }
  }
  // A hard-link insertion is atomic and fails if destination already exists;
  // unlike rename on POSIX, it never replaces a concurrent manifest.
  if (hook.before_publish != nullptr) {
    hook.before_publish(hook.context);
  }
  std::filesystem::create_hard_link(temporary, destination, error_code);
  if (!error_code) {
    std::filesystem::remove(temporary, error_code);
    return ImportCacheError::none;
  }
  std::filesystem::remove(temporary, error_code);

  destination_exists = false;
  result = inspect_existing_manifest(destination, manifest,
                                     destination_exists);
  if (result != ImportCacheError::none) {
    return result;
  }
  if (destination_exists) {
    cache_hit = true;
    return ImportCacheError::none;
  }
  return ImportCacheError::manifest_write_failed;
}

}  // namespace

ImportCacheResult detail::prepare_import_cache_with_hook(
    const ValidatedMedia& media,
    const std::filesystem::path& cache_root,
    const ImportCachePublishHook hook) {
  ImportCacheResult result;
  if (!media.valid() || !cache_root.is_absolute()) {
    result.error = ImportCacheError::invalid_request;
    return result;
  }

  const auto& source = media.source();
  const auto& inspection = media.inspection();
  const auto& fingerprint = media.fingerprint();
  if (source == nullptr || inspection.size_bytes != fingerprint.size_bytes ||
      inspection.source_sha256 != fingerprint.sha256) {
    result.error = ImportCacheError::invalid_request;
    return result;
  }
  result.error = verify_validated_source(source);
  if (result.error != ImportCacheError::none) {
    return result;
  }
  result.source_sha256 = fingerprint.sha256;
  result.source_directory =
      cache_root / "sources" / result.source_sha256;
  result.error = ensure_directory_tree(result.source_directory);
  if (result.error != ImportCacheError::none) {
    return result;
  }
  result.error = map_source_stability_error(
      detail::verify_complete_source_stability(media));
  if (result.error != ImportCacheError::none) {
    return result;
  }
  const auto manifest = make_manifest(inspection, fingerprint);
  result.error = publish_manifest(result.source_directory, manifest,
                                  result.cache_hit, hook);
  return result;
}

ImportCacheResult prepare_import_cache(
    const ValidatedMedia& media,
    const std::filesystem::path& cache_root) {
  return detail::prepare_import_cache_with_hook(media, cache_root, {});
}

}  // namespace ohl::media
