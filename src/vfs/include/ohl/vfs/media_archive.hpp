#pragma once

#include "ohl/vfs/iso9660_archive.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <string>
#include <string_view>

namespace ohl::vfs {

// The read-only media classes the engine can mount. The value is chosen by the
// media preflight; the reader never guesses it from a pathname.
enum class MediaFormat {
  udf,
  iso9660,
};

[[nodiscard]] constexpr std::string_view to_string(
    const MediaFormat format) noexcept {
  switch (format) {
    case MediaFormat::udf:
      return "udf";
    case MediaFormat::iso9660:
      return "iso9660";
  }

  return "unknown";
}

// Class-independent read-only file handle. It keeps the mounted archive, and
// therefore the pinned media source, alive.
class MediaFile {
 public:
  MediaFile() = default;
  virtual ~MediaFile();

  MediaFile(const MediaFile&) = delete;
  MediaFile& operator=(const MediaFile&) = delete;
  MediaFile(MediaFile&&) = delete;
  MediaFile& operator=(MediaFile&&) = delete;

  [[nodiscard]] virtual std::uint64_t size() const noexcept = 0;
  [[nodiscard]] virtual std::int64_t tell() const noexcept = 0;
  [[nodiscard]] virtual std::int64_t read(
      std::span<std::byte> destination) = 0;
  [[nodiscard]] virtual bool seek(std::uint64_t offset) = 0;
};

// One read-only archive facade over every supported media class. All callers
// outside src/vfs use this type so that adding a class does not change them.
// Directory, cursor, entry and limit semantics are identical for each class.
class MediaArchive final {
 public:
  MediaArchive();
  ~MediaArchive();
  MediaArchive(MediaArchive&&) noexcept;
  MediaArchive& operator=(MediaArchive&&) noexcept;

  MediaArchive(const MediaArchive&) = delete;
  MediaArchive& operator=(const MediaArchive&) = delete;

  // Mounts a pinned source capability using the reader for format.
  [[nodiscard]] VfsError open(MediaFormat format, SharedMediaSource source,
                              UdfArchiveLimits limits = {});
  [[nodiscard]] MediaArchive share() const;
  void close() noexcept;
  [[nodiscard]] bool is_open() const noexcept;
  [[nodiscard]] MediaFormat format() const noexcept;
  [[nodiscard]] std::string volume_label() const;
  [[nodiscard]] DirectoryPage list_page(std::string_view path) const;
  [[nodiscard]] DirectoryPage continue_list(DirectoryCursor cursor) const;
  [[nodiscard]] DirectoryListing list(std::string_view path) const;
  [[nodiscard]] std::unique_ptr<MediaFile> open_file(
      std::string_view path) const;
  [[nodiscard]] std::unique_ptr<MediaFile> open_file_at(
      std::string_view directory, std::string_view entry_name) const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

}  // namespace ohl::vfs
