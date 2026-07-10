#include "ohl/media/import_cache.hpp"

#include "ohl/core/sha256.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <fstream>
#include <iterator>
#include <random>
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
                                        const std::string_view digest) {
  std::ostringstream output;
  output << "{\n"
         << "  \"format_version\": 1,\n"
         << "  \"source\": {\n"
         << "    \"sha256\": \"" << digest << "\",\n"
         << "    \"size_bytes\": " << inspection.size_bytes << ",\n"
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

[[nodiscard]] ImportCacheError fingerprint_source(
    const std::filesystem::path& source_path,
    const std::uint64_t expected_size,
    const std::filesystem::file_time_type expected_write_time,
    std::string& digest) {
  std::error_code error_code;
  if (std::filesystem::file_size(source_path, error_code) != expected_size ||
      error_code ||
      std::filesystem::last_write_time(source_path, error_code) !=
          expected_write_time ||
      error_code) {
    return ImportCacheError::source_changed;
  }
  std::ifstream input{source_path, std::ios::binary};
  if (!input) {
    return ImportCacheError::source_read_failed;
  }

  ohl::core::Sha256 sha256;
  std::array<std::byte, 64 * 1'024> buffer{};
  std::uint64_t bytes_read = 0;
  while (input) {
    input.read(reinterpret_cast<char*>(buffer.data()),
               static_cast<std::streamsize>(buffer.size()));
    const auto count = input.gcount();
    if (count < 0) {
      return ImportCacheError::source_read_failed;
    }
    if (count != 0) {
      sha256.update(std::span{buffer}.first(static_cast<std::size_t>(count)));
      bytes_read += static_cast<std::uint64_t>(count);
    }
  }
  if (!input.eof()) {
    return ImportCacheError::source_read_failed;
  }
  if (bytes_read != expected_size) {
    return ImportCacheError::source_changed;
  }
  if (std::filesystem::file_size(source_path, error_code) != expected_size ||
      error_code ||
      std::filesystem::last_write_time(source_path, error_code) !=
          expected_write_time ||
      error_code) {
    return ImportCacheError::source_changed;
  }
  digest = ohl::core::hex_encode(sha256.finish());
  return ImportCacheError::none;
}

[[nodiscard]] bool read_text_file(const std::filesystem::path& path,
                                  std::string& contents) {
  std::ifstream input{path, std::ios::binary};
  if (!input) {
    return false;
  }
  contents.assign(std::istreambuf_iterator<char>{input},
                  std::istreambuf_iterator<char>{});
  return !input.bad();
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
    bool& cache_hit) {
  const auto destination = directory / "provenance.json";
  std::error_code error_code;
  const auto destination_status =
      std::filesystem::symlink_status(destination, error_code);
  if (error_code == std::errc::no_such_file_or_directory) {
    error_code.clear();
  } else if (error_code) {
    return ImportCacheError::manifest_write_failed;
  }
  if (std::filesystem::exists(destination_status)) {
    if (!std::filesystem::is_regular_file(destination_status) ||
        std::filesystem::is_symlink(destination_status)) {
      return ImportCacheError::unsafe_cache_path;
    }
    std::string existing;
    if (!read_text_file(destination, existing)) {
      return ImportCacheError::manifest_write_failed;
    }
    if (existing != manifest) {
      return ImportCacheError::manifest_conflict;
    }
    cache_hit = true;
    return ImportCacheError::none;
  }

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
  std::filesystem::rename(temporary, destination, error_code);
  if (error_code) {
    std::filesystem::remove(temporary, error_code);
    std::string existing;
    const auto status = std::filesystem::symlink_status(destination, error_code);
    if (!error_code && std::filesystem::is_regular_file(status) &&
        !std::filesystem::is_symlink(status) &&
        read_text_file(destination, existing) && existing == manifest) {
      cache_hit = true;
      return ImportCacheError::none;
    }
    return ImportCacheError::manifest_write_failed;
  }
  return ImportCacheError::none;
}

}  // namespace

ImportCacheResult prepare_import_cache(
    const std::filesystem::path& source_path,
    const IsoInspection& inspection,
    const std::filesystem::path& cache_root) {
  ImportCacheResult result;
  if (!inspection.valid() || inspection.size_bytes == 0 ||
      !cache_root.is_absolute()) {
    result.error = ImportCacheError::invalid_request;
    return result;
  }

  result.error = fingerprint_source(source_path, inspection.size_bytes,
                                    inspection.last_write_time,
                                    result.source_sha256);
  if (result.error != ImportCacheError::none) {
    return result;
  }
  if (inspection.source_sha256.size() != 64 ||
      result.source_sha256 != inspection.source_sha256) {
    result.error = ImportCacheError::source_changed;
    return result;
  }
  result.source_directory =
      cache_root / "sources" / result.source_sha256;
  result.error = ensure_directory_tree(result.source_directory);
  if (result.error != ImportCacheError::none) {
    return result;
  }
  const auto manifest = make_manifest(inspection, result.source_sha256);
  result.error = publish_manifest(result.source_directory, manifest,
                                  result.cache_hit);
  return result;
}

}  // namespace ohl::media
