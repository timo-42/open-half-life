#pragma once

#include "ohl/platform/media_source.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <cstdint>
#include <memory>

struct udfread_block_input;

namespace ohl::vfs::detail {

struct UdfMediaInputDeleter {
  void operator()(udfread_block_input* input) const noexcept;
};

using UdfMediaInputHandle =
    std::unique_ptr<udfread_block_input, UdfMediaInputDeleter>;

struct UdfMediaInputCreateResult {
  UdfMediaInputHandle input;
  VfsError error{VfsError::none};

  [[nodiscard]] bool valid() const noexcept {
    return input != nullptr && error == VfsError::none;
  }
};

[[nodiscard]] VfsError validate_udf_media_source_size(
    std::uint64_t source_size, const UdfArchiveLimits& limits,
    std::uint32_t& block_count) noexcept;

[[nodiscard]] UdfMediaInputCreateResult create_udf_media_input(
    SharedMediaSource source, const UdfArchiveLimits& limits);

// These helpers are the implementation used by the C callbacks and provide a
// narrow seam for synthetic boundary tests without exposing libudfread types
// through the public VFS API.
[[nodiscard]] int udf_media_input_read_blocks(
    udfread_block_input* input, std::uint32_t lba, void* destination,
    std::uint32_t block_count) noexcept;
[[nodiscard]] std::uint32_t udf_media_input_size_blocks(
    udfread_block_input* input) noexcept;
[[nodiscard]] platform::MediaSourceError udf_media_input_source_error(
    const udfread_block_input* input) noexcept;

}  // namespace ohl::vfs::detail
