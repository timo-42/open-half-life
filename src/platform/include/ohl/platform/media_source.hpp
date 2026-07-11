#pragma once

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <span>

namespace ohl::platform {

enum class MediaSourceError {
  none,
  not_found,
  not_regular_file,
  open_failed,
  read_failed,
  unexpected_eof,
  out_of_range,
  changed,
  resource_exhausted,
  unsupported,
};

struct MediaSourceOpenResult;

// A read-only capability for one native file object. The selected path is used
// only by open_media_source(); it is not retained or reopened. Sharing the
// returned object shares the same pinned native identity. Positional reads do
// not use a shared seek cursor and are safe to call concurrently.
//
// A pinned handle prevents pathname replacement from retargeting the source,
// but cannot prevent an external writer from changing the underlying object.
// Call verify_unchanged() at defined phase boundaries and perform the required
// end-to-end content verification before publishing imported data.
class MediaSource final {
 public:
  ~MediaSource();

  MediaSource(const MediaSource&) = delete;
  MediaSource& operator=(const MediaSource&) = delete;
  MediaSource(MediaSource&&) = delete;
  MediaSource& operator=(MediaSource&&) = delete;

  [[nodiscard]] std::uint64_t size() const noexcept;

  // Reads exactly destination.size() bytes at offset without changing shared
  // cursor state. Empty reads are valid at offsets through size(). Ranges past
  // the captured size return out_of_range; an early native EOF returns
  // unexpected_eof.
  [[nodiscard]] MediaSourceError read_exact_at(
      std::uint64_t offset,
      std::span<std::byte> destination) const noexcept;

  // Re-queries the pinned object and compares its regular-file type, stable
  // native identity, size, and native content-change indicators with the
  // acquisition snapshot. It never resolves the original pathname.
  [[nodiscard]] MediaSourceError verify_unchanged() const noexcept;

 private:
  struct Impl;
  explicit MediaSource(std::unique_ptr<Impl> implementation) noexcept;

  std::unique_ptr<Impl> implementation_;

  friend MediaSourceOpenResult open_media_source(
      const std::filesystem::path& path) noexcept;
};

struct MediaSourceOpenResult {
  std::shared_ptr<const MediaSource> source;
  MediaSourceError error{MediaSourceError::none};

  [[nodiscard]] bool valid() const noexcept {
    return source != nullptr && error == MediaSourceError::none;
  }
};

// Opens the selected path with one native acquisition, atomically rejecting a
// symbolic link or reparse point in the final path component before accepting
// and pinning a regular-file object. Intermediate path components use normal
// platform resolution; this contract does not promise no-follow handling for
// them. The path and native handle are not exposed.
[[nodiscard]] MediaSourceOpenResult open_media_source(
    const std::filesystem::path& path) noexcept;

}  // namespace ohl::platform
