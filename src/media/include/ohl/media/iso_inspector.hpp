#pragma once

#include <cstdint>
#include <filesystem>
#include <string>
#include <string_view>

namespace ohl::media {

enum class MediaError {
  none,
  not_found,
  not_regular_file,
  too_small,
  unsupported_filesystem,
  invalid_structure,
  io_error,
};

[[nodiscard]] constexpr std::string_view to_string(
    const MediaError error) noexcept {
  switch (error) {
    case MediaError::none:
      return "none";
    case MediaError::not_found:
      return "file not found";
    case MediaError::not_regular_file:
      return "path is not a regular file";
    case MediaError::too_small:
      return "file is too small to contain a supported UDF image";
    case MediaError::unsupported_filesystem:
      return "media does not contain a supported ECMA-167 NSR02 structure";
    case MediaError::invalid_structure:
      return "UDF descriptor structure is invalid or truncated";
    case MediaError::io_error:
      return "media could not be read";
  }

  return "unknown media error";
}

struct IsoInspection {
  MediaError error{MediaError::none};
  std::uint64_t size_bytes{0};
  std::string filesystem;
  std::string volume_label;

  [[nodiscard]] bool valid() const noexcept {
    return error == MediaError::none;
  }
};

[[nodiscard]] IsoInspection inspect_iso(const std::filesystem::path& path);

}  // namespace ohl::media
